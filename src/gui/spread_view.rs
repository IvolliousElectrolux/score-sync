//! 对开分块工作区: 左右两页合成一册, 多册按窗口宽度折行.

use super::*;

pub(super) struct SpreadDrag {
    from: usize,
    start_x: f32,
    start_y: f32,
    x: f32,
    y: f32,
    origin_x: f32,
    origin_y: f32,
    armed: bool,
    collapse: bool,
    line_at: Option<usize>,
    line_after: bool,
}

pub(super) struct SpreadMenu {
    pub(super) page_index: usize,
    pub(super) x: f32,
    pub(super) y: f32,
    fly: Option<SpreadMenuFly>,
    main_bounds: Option<Bounds<Pixels>>,
    sub_bounds: Option<Bounds<Pixels>>,
    paste_row: Option<Bounds<Pixels>>,
    blank_row: Option<Bounds<Pixels>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpreadMenuFly {
    Paste,
    Blank,
}

#[derive(Clone, Copy)]
enum SpreadMenuSlot {
    Main,
    Sub,
    Paste,
    Blank,
}

#[derive(Clone, Copy)]
enum SpreadMenuCmd {
    Duplicate,
    Paste { after: bool },
    Blank { after: bool },
    Delete,
}

enum SpreadDropKind {
    Move { anchor: usize, after: bool },
    /// 右页拖进下一对左页的左缘: 原位置补空白, 左页单独留下.
    Isolate { at: usize },
}

fn glyph_px(c: char, em: f32) -> f32 {
    if (c as u32) > 0x7f {
        em
    } else {
        em * 0.55
    }
}

fn text_px(s: &str, em: f32) -> f32 {
    s.chars().map(|c| glyph_px(c, em)).sum()
}

fn take_prefix(s: &str, budget: f32, em: f32) -> &str {
    let mut used = 0.0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let w = glyph_px(c, em);
        if used + w > budget {
            break;
        }
        used += w;
        end = i + c.len_utf8();
    }
    &s[..end]
}

fn take_suffix(s: &str, budget: f32, em: f32) -> &str {
    let mut used = 0.0;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        let w = glyph_px(c, em);
        if used + w > budget {
            break;
        }
        used += w;
        start = i;
    }
    &s[start..]
}

/// 文件名里最后一处 `_p` + 数字的字节范围, 不含后面的 `_copy`.
fn page_token_range(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'_' && b[i + 1] == b'p' && b[i + 2].is_ascii_digit() {
            let start = i;
            let mut end = i + 2;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            found = Some((start, end));
            i = end;
        } else {
            i += 1;
        }
    }
    found
}

/// 缩略图下的文件名. `_p` 页码始终保留, 放不下时省略号放在页码前面.
fn clip_spread_title(s: &str, max_px: f32) -> String {
    const EM: f32 = 12.0;
    if text_px(s, EM) <= max_px {
        return s.to_string();
    }
    let ell = "…";
    let ell_px = text_px(ell, EM);
    let Some((a, b)) = page_token_range(s) else {
        let budget = (max_px - ell_px).max(0.0);
        let prefix = take_prefix(s, budget * 0.5, EM);
        let suffix = take_suffix(s, budget - text_px(prefix, EM), EM);
        if prefix.len() + suffix.len() >= s.len() {
            return s.to_string();
        }
        return format!("{prefix}{ell}{suffix}");
    };
    let token = &s[a..b];
    let tail = &s[b..];
    let head = &s[..a];
    let token_px = text_px(token, EM);
    let tail_px = text_px(tail, EM);
    if token_px + tail_px + ell_px <= max_px {
        let budget = (max_px - token_px - tail_px - ell_px).max(0.0);
        let prefix = take_prefix(head, budget, EM);
        if prefix.is_empty() {
            format!("{ell}{token}{tail}")
        } else if prefix.len() == head.len() {
            format!("{head}{token}{tail}")
        } else {
            format!("{prefix}{ell}{token}{tail}")
        }
    } else if token_px + ell_px <= max_px {
        let kept = take_prefix(tail, (max_px - token_px - ell_px).max(0.0), EM);
        format!("{ell}{token}{kept}")
    } else {
        token.to_string()
    }
}

/// 菜单宽度跟着最长的一条文字走. `arrow` 时右侧再留出二级菜单标记的位置.
fn spread_menu_width(labels: &[&str], arrow: bool) -> f32 {
    const EM: f32 = 14.0;
    let text = labels
        .iter()
        .map(|s| text_px(s, EM))
        .fold(0.0_f32, f32::max);
    let mark = if arrow { 18.0 } else { 0.0 };
    (text + 36.0 + mark).ceil()
}

/// 落点会不会改页序. 整块按原顺序抽出再插入, 和 `move_pages_block` 一致.
fn spread_block_moves(n: usize, moving: &[usize], anchor: usize, after: bool) -> bool {
    if n == 0 || moving.is_empty() || anchor >= n {
        return false;
    }
    let moving_set: HashSet<usize> = moving.iter().copied().filter(|&i| i < n).collect();
    if moving_set.is_empty() || moving_set.contains(&anchor) {
        return false;
    }
    let raw_insert = if after { anchor + 1 } else { anchor };
    let insert_in_remaining = raw_insert - moving_set.iter().filter(|&&i| i < raw_insert).count();
    let mut remaining = Vec::with_capacity(n - moving_set.len());
    let mut block = Vec::with_capacity(moving_set.len());
    for i in 0..n {
        if moving_set.contains(&i) {
            block.push(i);
        } else {
            remaining.push(i);
        }
    }
    let insert_at = insert_in_remaining.min(remaining.len());
    let mut out = remaining;
    for (j, p) in block.into_iter().enumerate() {
        out.insert(insert_at + j, p);
    }
    out.into_iter().enumerate().any(|(i, v)| i != v)
}

fn point_in_bounds(b: Bounds<Pixels>, x: f32, y: f32) -> bool {
    let left = f32::from(b.origin.x);
    let top = f32::from(b.origin.y);
    let right = left + f32::from(b.size.width);
    let bottom = top + f32::from(b.size.height);
    x >= left && x <= right && y >= top && y <= bottom
}

fn classify_spread_drop(
    n: usize,
    from: usize,
    moving: &[usize],
    anchor: usize,
    after: bool,
) -> Option<SpreadDropKind> {
    let single = moving.len() == 1 && moving[0] == from;
    if single && !after && from % 2 == 1 && anchor == from + 1 && anchor < n {
        return Some(SpreadDropKind::Isolate { at: from });
    }
    if spread_block_moves(n, moving, anchor, after) {
        Some(SpreadDropKind::Move { anchor, after })
    } else {
        None
    }
}

/// 按原册宽, 一行放得下几册. 窗口够宽时多册并排, 不够就折行.
fn spread_books_per_row(avail: f32, books: usize) -> usize {
    if books == 0 {
        return 1;
    }
    let book = SPREAD_BOOK_W.max(1.0);
    if avail < book {
        return 1;
    }
    let cols = ((avail + SPREAD_BOOK_GAP) / (book + SPREAD_BOOK_GAP)).floor() as usize;
    cols.max(1).min(books)
}

/// 一行 `cols` 册把可用宽度摊满后, 每一册的外宽. 册间距仍是原来的固定值.
fn spread_book_width(avail: f32, books: usize) -> f32 {
    if books == 0 || avail <= 1.0 {
        return SPREAD_BOOK_W;
    }
    let cols = spread_books_per_row(avail, books).max(1);
    let gaps = cols.saturating_sub(1) as f32 * SPREAD_BOOK_GAP;
    // 每册留 1px, 布局取整后才不会把最后一册挤到下一行.
    let slack = cols as f32;
    ((avail - gaps - slack) / cols as f32).max(1.0)
}

/// 摊满后的一册. 页, 缩略图, 书脊, 内边距按同一个比例缩放, 边框仍是 1px.
#[derive(Clone, Copy)]
struct SpreadFit {
    book_w: f32,
    page_w: f32,
    thumb_w: f32,
    thumb_h: f32,
    spine_w: f32,
    pad: f32,
    scale: f32,
}

fn spread_fit(avail: f32, books: usize) -> SpreadFit {
    let book_w = spread_book_width(avail, books);
    let border = SPREAD_BOOK_BORDER * 2.0;
    let inner = (SPREAD_BOOK_W - border).max(1.0);
    let scale = ((book_w - border) / inner).max(0.2);
    SpreadFit {
        book_w: border + inner * scale,
        page_w: SPREAD_PAGE_W * scale,
        thumb_w: SPREAD_THUMB_W * scale,
        thumb_h: SPREAD_THUMB_H * scale,
        spine_w: SPREAD_SPINE_W * scale,
        pad: SPREAD_BOOK_PAD * scale,
        scale,
    }
}

/// 页题按 12px 量宽, 显示时再乘上同一比例. 传入的是未缩放的像素预算.
fn spread_caption_px(fit: SpreadFit) -> f32 {
    ((fit.thumb_w - 4.0) / fit.scale.max(0.2)).max(8.0)
}

impl ScoreSyncApp {
    /// 对开网格真正能用来排册的宽度: 左栏去掉分隔条, 右侧滚动条和四周留白.
    fn spread_row_avail(&self) -> f32 {
        let left = (self.viewport_w - self.side_width - SIDE_SPLIT_W).max(0.0);
        (left - ORG_SCROLLBAR_W - SPREAD_SCROLL_PAD * 2.0).max(1.0)
    }

    pub(super) fn spread_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.queue_spread_thumbs(cx);
        let n = self.doc.pages.len();
        let rows = n.div_ceil(2);
        let fit = spread_fit(self.spread_row_avail(), rows);
        self.spread_cells.clear();
        let moving: HashSet<usize> = self
            .spread_drag
            .as_ref()
            .filter(|d| d.armed)
            .map(|d| self.spread_moving(d.from).into_iter().collect())
            .unwrap_or_default();
        // 滚动条量的是这一层的直接子元素. 再套一层且那层被拉成视口高时,
        // 折行后的册只溢出内层, 外层仍认为没有可滚距离.
        let mut books = div()
            .id("spread_scroll_body")
            .flex()
            .flex_row()
            .flex_wrap()
            .items_start()
            .content_start()
            .gap(px(SPREAD_BOOK_GAP))
            .p(px(SPREAD_SCROLL_PAD));
        if n == 0 {
            books = books.child(
                div()
                    .text_sm()
                    .text_color(ui_kit::chrome(0x64748b))
                    .child("还没有页面. 用「打开」导入 PDF 或图片, 将按顺序两两对开."),
            );
        }
        for row in 0..rows {
            books = books.child(self.spread_book(row, &moving, fit, cx));
        }
        div()
            .id("spread_workspace")
            .flex_1()
            .h_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(ui_kit::chrome(0xf1f5f9))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.spread_drag_move(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    this.spread_drag_up(f32::from(ev.position.x), f32::from(ev.position.y), cx);
                }),
            )
            .child(
                self.attach_scrollbars_pinned(
                    "spread_scroll".into(),
                    ScrollList::PageOrganize,
                    &self.page_organize_scroll,
                    books,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            )
    }

    fn spread_book(
        &self,
        row: usize,
        moving: &HashSet<usize>,
        fit: SpreadFit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let n = self.doc.pages.len();
        let left = row * 2;
        let right = left + 1;
        let active = self.doc.current_page_index / 2 == row;
        let border = if active { ui_kit::chrome(0x2563eb) } else { rgb(0xe7e5e4) };
        div()
            .id(SharedString::from(format!("spread_book_{row}")))
            .flex_shrink_0()
            .w(px(fit.book_w))
            .min_w(px(0.))
            .max_w(px(fit.book_w))
            .flex()
            .flex_col()
            .gap(px(4.0 * fit.scale))
            .p(px(fit.pad))
            .rounded_md()
            .bg(rgb(0xfffbf5))
            .border_1()
            .border_color(border)
            .shadow_sm()
            .child(
                div()
                    .px_1()
                    .h(px(16.0 * fit.scale))
                    .text_size(px(12.0 * fit.scale))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(if active { ui_kit::chrome(0x1d4ed8) } else { rgb(0x78716c) })
                    .child(format!("第 {} 对", row + 1)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .child(self.spread_page_half(left, moving.contains(&left), fit, cx))
                    .child(self.spread_spine(fit))
                    .when(right < n, |d| {
                        d.child(self.spread_page_half(right, moving.contains(&right), fit, cx))
                    })
                    .when(right >= n, |d| d.child(self.spread_empty_half(fit))),
            )
    }

    fn spread_spine(&self, fit: SpreadFit) -> impl IntoElement {
        div()
            .w(px(fit.spine_w))
            .h(px(fit.thumb_h))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .bg(rgb(0xe7e5e4))
            .child(div().flex_1())
            .child(div().w(px(2.0 * fit.scale)).h_full().bg(rgb(0xa8a29e)))
            .child(div().flex_1())
    }

    fn spread_empty_half(&self, fit: SpreadFit) -> impl IntoElement {
        div()
            .w(px(fit.page_w))
            .min_w(px(0.))
            .max_w(px(fit.page_w))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0 * fit.scale))
            .child(
                div()
                    .w(px(fit.thumb_w))
                    .h(px(fit.thumb_h))
                    .rounded_sm()
                    .bg(rgb(0xf5f5f4))
                    .border_1()
                    .border_color(rgb(0xe7e5e4))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(0xa8a29e))
                    .child("空页"),
            )
            .child(self.spread_caption_line("", fit.thumb_w))
    }

    fn spread_page_half(
        &self,
        idx: usize,
        dragging: bool,
        fit: SpreadFit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let page = &self.doc.pages[idx];
        let id = page.id.clone();
        let blank = page.blank;
        let selected = self.spread_has(&id);
        let current = self.doc.current_page_index == idx;
        let full_title = page.spread_caption().0;
        let title = clip_spread_title(&full_title, spread_caption_px(fit));
        let thumb = if blank {
            None
        } else {
            self.org_thumbs.get(&id).cloned()
        };
        let waiting = !blank && thumb.is_none();
        let side = if idx % 2 == 0 { "左" } else { "右" };
        let thumb_border = if selected {
            ui_kit::chrome(0x2563eb)
        } else if current {
            rgb(0x93c5fd)
        } else {
            rgb(0xe7e5e4)
        };
        let line_before = self
            .spread_drag
            .as_ref()
            .is_some_and(|d| d.armed && d.line_at == Some(idx) && !d.line_after);
        let line_after = self
            .spread_drag
            .as_ref()
            .is_some_and(|d| d.armed && d.line_at == Some(idx) && d.line_after);
        let entity = cx.entity().clone();
        div()
            .id(SharedString::from(format!("spread_card_{idx}")))
            .relative()
            .w(px(fit.page_w))
            .min_w(px(0.))
            .max_w(px(fit.page_w))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0 * fit.scale))
            .rounded_sm()
            .cursor_pointer()
            .when(selected, |d| d.bg(ui_kit::chrome(0xeff6ff)))
            .when(dragging, |d| d.opacity(0.4))
            .child(Self::measure_spread_cell(entity, idx))
            .child(
                div()
                    .w(px(fit.thumb_w))
                    .h(px(fit.thumb_h))
                    .rounded_sm()
                    .bg(if blank { rgb(0xf5f5f4) } else { rgb(0xffffff) })
                    .border_1()
                    .border_color(thumb_border)
                    .overflow_hidden()
                    .relative()
                    .when(blank, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(0xa8a29e))
                                .child("空页"),
                        )
                    })
                    .when(!blank, |d| {
                        d.child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let Some(img) = &thumb else {
                                    return;
                                };
                                let sz = img.size(0);
                                let iw = (sz.width.0 as f32).max(1.0);
                                let ih = (sz.height.0 as f32).max(1.0);
                                let vw = f32::from(bounds.size.width);
                                let vh = f32::from(bounds.size.height);
                                let fit = (vw / iw).min(vh / ih).max(0.0001);
                                let dw = iw * fit;
                                let dh = ih * fit;
                                let ox = bounds.origin.x + px((vw - dw) * 0.5);
                                let oy = bounds.origin.y + px((vh - dh) * 0.5);
                                let img_bounds = Bounds {
                                    origin: point(ox, oy),
                                    size: size(px(dw), px(dh)),
                                };
                                let _ = window.paint_image(
                                    img_bounds,
                                    gpui::Corners::default(),
                                    img.clone(),
                                    0,
                                    false,
                                );
                            },
                        )
                        .size_full(),
                    )
                    })
                    .when(waiting, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(0xa8a29e))
                                .child("…"),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .left(px(4.))
                            .px_1()
                            .rounded_sm()
                            .bg(rgb(0xf5f5f4))
                            .text_color(rgb(0x57534e))
                            .text_xs()
                            .child(side),
                    )
                    .when(current, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(4.))
                                .right(px(4.))
                                .px_1()
                                .rounded_sm()
                                .bg(ui_kit::chrome(0x2563eb))
                                .text_color(rgb(0xffffff))
                                .text_xs()
                                .child("当前"),
                        )
                    }),
            )
            .child(self.spread_caption_hover_line(idx, &full_title, &title, fit, cx))
            .when(line_before, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(-1.))
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .rounded_sm()
                        .bg(ui_kit::chrome(0xf59e0b)),
                )
            })
            .when(line_after, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(-1.))
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .rounded_sm()
                        .bg(ui_kit::chrome(0xf59e0b)),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.clear_tab_hover(cx);
                    this.spread_menu = None;
                    let ctrl = is_primary_mod(&ev.modifiers);
                    let shift = ev.modifiers.shift;
                    let already = this
                        .doc
                        .pages
                        .get(idx)
                        .is_some_and(|p| this.spread_has(&p.id));
                    if !(already && !ctrl && !shift) {
                        this.spread_click(idx, ctrl, shift);
                    }
                    this.focus_spread_page(idx);
                    let mx = f32::from(ev.position.x);
                    let my = f32::from(ev.position.y);
                    let (origin_x, origin_y) = this
                        .spread_cells
                        .get(&idx)
                        .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
                        .unwrap_or((mx, my));
                    this.spread_drag = Some(SpreadDrag {
                        from: idx,
                        start_x: mx,
                        start_y: my,
                        x: mx,
                        y: my,
                        origin_x,
                        origin_y,
                        armed: false,
                        collapse: !ctrl && !shift,
                        line_at: None,
                        line_after: false,
                    });
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.focus_spread_page(idx);
                    if !this
                        .doc
                        .pages
                        .get(idx)
                        .is_some_and(|p| this.spread_has(&p.id))
                    {
                        this.spread_click(idx, false, false);
                    }
                    this.spread_drag = None;
                    this.spread_menu = Some(SpreadMenu {
                        page_index: idx,
                        x: f32::from(ev.position.x),
                        y: f32::from(ev.position.y),
                        fly: None,
                        main_bounds: None,
                        sub_bounds: None,
                        paste_row: None,
                        blank_row: None,
                    });
                    cx.notify();
                }),
            )
    }

    /// 文件名被截断时, 悬停满 1 秒后用和页签一样的提示显示全称.
    fn spread_caption_hover_line(
        &self,
        idx: usize,
        full: &str,
        shown: &str,
        fit: SpreadFit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let truncated = shown != full;
        let entity = cx.entity();
        let clip_w = spread_caption_px(fit);
        div()
            .id(SharedString::from(format!("spread_cap_{idx}")))
            .relative()
            .w(px(fit.thumb_w))
            .min_w(px(0.))
            .max_w(px(fit.thumb_w))
            .h(px(16.0 * fit.scale))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(12.0 * fit.scale))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(0x292524))
            .when(truncated, |d| {
                d.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.note_spread_title_hover(idx, clip_w, cx);
                    } else if this.spread_title_hover == Some(idx) {
                        this.release_tab_hover(cx);
                    }
                }))
            })
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| {
                            this.spread_caption_bounds.insert(idx, bounds);
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(shown.to_string())
    }

    fn note_spread_title_hover(&mut self, idx: usize, clip_w: f32, cx: &mut Context<Self>) {
        if self.spread_drag.is_some() {
            self.clear_tab_hover(cx);
            return;
        }
        if self.spread_title_hover == Some(idx) {
            return;
        }
        let warm = self.tooltip_warm;
        self.spread_title_hover = Some(idx);
        self.tab_hover_idx = None;
        self.header_hover_id = None;
        self.tab_hover_gen = self.tab_hover_gen.wrapping_add(1);
        let gen = self.tab_hover_gen;
        if warm {
            self.show_spread_title_tooltip(idx, clip_w, cx);
            return;
        }
        self.tab_tooltip = None;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            this.update(cx, |view, cx| {
                if view.tab_hover_gen != gen || view.spread_title_hover != Some(idx) {
                    return;
                }
                if view.spread_drag.is_some() {
                    return;
                }
                view.show_spread_title_tooltip(idx, clip_w, cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_spread_title_tooltip(&mut self, idx: usize, clip_w: f32, cx: &mut Context<Self>) {
        let Some(page) = self.doc.pages.get(idx) else {
            return;
        };
        let full = page.spread_caption().0;
        let shown = clip_spread_title(&full, clip_w);
        if shown == full {
            if self.tab_tooltip.take().is_some() {
                cx.notify();
            }
            return;
        }
        let (ax, ay, aw, ah) = self
            .spread_caption_bounds
            .get(&idx)
            .map(|b| {
                (
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                )
            })
            .unwrap_or((0.0, 0.0, 0.0, 0.0));
        self.publish_hover_tooltip(
            TabTooltip {
                id: format!("spread-cap-{idx}").into(),
                anchor_x: ax,
                anchor_y: ay,
                anchor_w: aw,
                anchor_h: ah,
                text: full.into(),
                measured_w: 0.0,
                measured_h: 0.0,
            },
            cx,
        );
    }

    fn spread_caption_line(&self, title: &str, thumb_w: f32) -> impl IntoElement {
        let scale = thumb_w / SPREAD_THUMB_W.max(1.0);
        div()
            .w(px(thumb_w))
            .h(px(16.0 * scale))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(12.0 * scale))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(0x292524))
            .child(title.to_string())
    }

    fn spread_has(&self, id: &str) -> bool {
        self.spread_selected.iter().any(|x| x == id)
    }

    /// 点选顺序上的页下标. 已经不在的 id 略过.
    fn spread_selection_indices(&self) -> Vec<usize> {
        self.spread_selected
            .iter()
            .filter_map(|id| self.doc.pages.iter().position(|p| &p.id == id))
            .collect()
    }

    fn spread_click(&mut self, idx: usize, ctrl: bool, shift: bool) {
        let ids: Vec<String> = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        if idx >= ids.len() {
            return;
        }
        let id = ids[idx].clone();
        if shift {
            let anchor_idx = self
                .spread_anchor
                .as_ref()
                .and_then(|aid| ids.iter().position(|x| x == aid))
                .unwrap_or(idx);
            let mut range: Vec<String> = if anchor_idx <= idx {
                ids[anchor_idx..=idx].to_vec()
            } else {
                let mut v = ids[idx..=anchor_idx].to_vec();
                v.reverse();
                v
            };
            if ctrl {
                for rid in range {
                    if !self.spread_has(&rid) {
                        self.spread_selected.push(rid);
                    }
                }
            } else {
                range.retain(|rid| !rid.is_empty());
                self.spread_selected = range;
            }
        } else if ctrl {
            if let Some(at) = self.spread_selected.iter().position(|x| x == &id) {
                self.spread_selected.remove(at);
            } else {
                self.spread_selected.push(id.clone());
            }
            self.spread_anchor = Some(id);
        } else {
            self.spread_selected.clear();
            self.spread_selected.push(id.clone());
            self.spread_anchor = Some(id);
        }
    }

    fn focus_spread_page(&mut self, idx: usize) {
        if idx >= self.doc.pages.len() {
            return;
        }
        self.doc.current_page_index = idx;
        if let Some(g) = self.doc.groups.get(idx / 2) {
            self.doc.active_group_id = Some(g.id.clone());
        }
    }

    fn spread_drag_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(mut drag) = self.spread_drag.take() else {
            return;
        };
        if !drag.armed && Self::reorder_slop_exceeded(x - drag.start_x, y - drag.start_y) {
            drag.armed = true;
        }
        drag.x = x;
        drag.y = y;
        if drag.armed {
            match self.spread_hit_drop(drag.from, x, y) {
                Some(SpreadDropKind::Move { anchor, after }) => {
                    drag.line_at = Some(anchor);
                    drag.line_after = after;
                }
                Some(SpreadDropKind::Isolate { at }) => {
                    drag.line_at = Some(at + 1);
                    drag.line_after = false;
                }
                None => {
                    drag.line_at = None;
                    drag.line_after = false;
                }
            }
        }
        self.spread_drag = Some(drag);
        if self.spread_drag.as_ref().is_some_and(|d| d.armed) {
            cx.notify();
        }
    }

    fn spread_drag_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.spread_drag.take() else {
            return;
        };
        if drag.collapse && !drag.armed {
            self.spread_click(drag.from, false, false);
            self.focus_spread_page(drag.from);
            cx.notify();
            return;
        }
        if !drag.armed {
            cx.notify();
            return;
        }
        match self.spread_hit_drop(drag.from, x, y) {
            Some(SpreadDropKind::Isolate { at }) => {
                self.insert_spread_blank_with(at, "已在原位置补上空白页, 左页单独留下.", cx);
            }
            Some(SpreadDropKind::Move { anchor, after }) => {
                let moving = self.spread_moving(drag.from);
                self.push_crop_undo_page_structure();
                self.doc.move_pages_block(&moving, anchor, after);
                self.prune_spread_page_ids();
                self.mark_dirty();
                self.mark_video_pool_dirty_all();
                self.status = "已调整页序, 对开已按新顺序重配.".into();
                self.hint = self.status.clone();
                self.refresh_render(cx);
            }
            None => cx.notify(),
        }
    }

    /// 拖动时跟着指针的半透明页. 和落点高亮无关, 同一对里不会换位时也要在.
    pub(super) fn spread_drag_ghost(&self) -> impl IntoElement {
        let Some(drag) = self.spread_drag.as_ref().filter(|d| d.armed) else {
            return div().into_any_element();
        };
        let page = self.doc.pages.get(drag.from);
        let blank = page.is_some_and(|p| p.blank);
        let thumb = if blank {
            None
        } else {
            page.and_then(|p| self.org_thumbs.get(&p.id).cloned())
        };
        let metrics = spread_fit(
            self.spread_row_avail(),
            self.doc.pages.len().div_ceil(2),
        );
        let title = page
            .map(|p| clip_spread_title(&p.spread_caption().0, spread_caption_px(metrics)))
            .unwrap_or_default();
        let count = self.spread_moving(drag.from).len().max(1);
        let dx = (metrics.page_w - metrics.thumb_w) * 0.5;
        let gx = drag.origin_x + dx + (drag.x - drag.start_x);
        let gy = drag.origin_y + (drag.y - drag.start_y);
        div()
            .id("spread_drag_ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .w(px(metrics.thumb_w))
            .opacity(0.72)
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(
                div()
                    .w(px(metrics.thumb_w))
                    .h(px(metrics.thumb_h))
                    .rounded_sm()
                    .bg(if blank { rgb(0xf5f5f4) } else { rgb(0xffffff) })
                    .border_1()
                    .border_color(ui_kit::chrome(0x2563eb))
                    .shadow_md()
                    .overflow_hidden()
                    .relative()
                    .when(blank, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(0xa8a29e))
                                .child("空页"),
                        )
                    })
                    .when(!blank, |d| {
                        d.child({
                        let paint = thumb;
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let Some(img) = &paint else {
                                    return;
                                };
                                let sz = img.size(0);
                                let iw = (sz.width.0 as f32).max(1.0);
                                let ih = (sz.height.0 as f32).max(1.0);
                                let vw = f32::from(bounds.size.width);
                                let vh = f32::from(bounds.size.height);
                                let fit = (vw / iw).min(vh / ih).max(0.0001);
                                let dw = iw * fit;
                                let dh = ih * fit;
                                let ox = bounds.origin.x + px((vw - dw) * 0.5);
                                let oy = bounds.origin.y + px((vh - dh) * 0.5);
                                let img_bounds = Bounds {
                                    origin: point(ox, oy),
                                    size: size(px(dw), px(dh)),
                                };
                                let _ = window.paint_image(
                                    img_bounds,
                                    gpui::Corners::default(),
                                    img.clone(),
                                    0,
                                    false,
                                );
                            },
                        )
                        .size_full()
                        })
                    })
                    .when(count > 1, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(4.))
                                .right(px(4.))
                                .px_1()
                                .rounded_sm()
                                .bg(ui_kit::chrome(0x2563eb))
                                .text_color(rgb(0xffffff))
                                .text_xs()
                                .child(format!("{count}")),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(metrics.thumb_w))
                    .h(px(16.0 * metrics.scale))
                    .text_size(px(12.0 * metrics.scale))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0x292524))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(title),
            )
            .into_any_element()
    }

    fn spread_hit_drop(&self, from: usize, x: f32, y: f32) -> Option<SpreadDropKind> {
        let (anchor, after) = self.spread_drop_at(from, x, y)?;
        let moving = self.spread_moving(from);
        classify_spread_drop(self.doc.pages.len(), from, &moving, anchor, after)
    }

    fn spread_moving(&self, from: usize) -> Vec<usize> {
        let idxs: Vec<usize> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| self.spread_has(&p.id))
            .map(|(i, _)| i)
            .collect();
        if idxs.contains(&from) {
            idxs
        } else {
            vec![from]
        }
    }

    fn spread_drop_at(&self, from: usize, x: f32, y: f32) -> Option<(usize, bool)> {
        let n = self.doc.pages.len();
        let moving: HashSet<usize> = self.spread_moving(from).into_iter().collect();
        for i in 0..n {
            if moving.contains(&i) {
                continue;
            }
            let b = self.spread_cells.get(&i)?;
            let left = f32::from(b.origin.x);
            let top = f32::from(b.origin.y);
            let right = left + f32::from(b.size.width);
            let bottom = top + f32::from(b.size.height);
            if x >= left && x <= right && y >= top && y <= bottom {
                let after = x >= (left + right) * 0.5;
                return Some((i, after));
            }
        }
        None
    }

    pub(super) fn select_all_spread_pages(&mut self, cx: &mut Context<Self>) {
        self.spread_selected = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        if self.spread_anchor.is_none() {
            self.spread_anchor = self.doc.pages.first().map(|p| p.id.clone());
        }
        cx.notify();
    }

    pub(super) fn delete_spread_selected(&mut self, cx: &mut Context<Self>) {
        let idxs: Vec<usize> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| self.spread_has(&p.id))
            .map(|(i, _)| i)
            .collect();
        if idxs.is_empty() {
            return;
        }
        self.push_crop_undo_page_structure();
        let n = idxs.len();
        let dead = self.doc.close_pages_at(&idxs);
        for id in &dead {
            self.spread_selected.retain(|x| x != id);
            self.spread_clip.retain(|x| x != id);
            self.crop_histories.remove(id);
            if let Some(img) = self.org_thumbs.remove(id) {
                self.gpu_drop.push(img);
            }
        }
        self.spread_menu = None;
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        self.status = format!("已删除 {n} 页, 对开已重配.").into();
        self.hint = self.status.clone();
        self.refresh_render(cx);
    }

    fn spread_keys_live(&self) -> bool {
        self.crop_keys_live()
            && self.doc.is_spread()
            && self.dialog.is_none()
            && self.pdf_import.is_none()
    }

    /// 工具栏「复制」: 立刻把选中页按点选顺序收成一串, 贴在最后一页的右侧.
    pub(super) fn duplicate_spread_selected(&mut self, cx: &mut Context<Self>) {
        let order = self.spread_selection_indices();
        if order.is_empty() {
            self.status = "请先选中要复制的页面.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        let after = *order.last().unwrap();
        self.insert_spread_copies(&order, after, true, false, cx);
    }

    pub(super) fn copy_spread_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.spread_keys_live() {
            return;
        }
        let order = self.spread_selection_indices();
        if order.is_empty() {
            self.status = "请先选中要复制的页面.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.spread_clip = order
            .iter()
            .map(|&i| self.doc.pages[i].id.clone())
            .collect();
        self.spread_menu = None;
        self.status = format!(
            "已复制 {} 页. {}V 贴到当时选中页的右侧.",
            order.len(),
            apply_bg::primary_mod()
        )
        .into();
        self.hint = self.status.clone();
        cx.notify();
    }

    pub(super) fn paste_spread_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.spread_keys_live() {
            return;
        }
        if self.spread_clip.is_empty() {
            self.status = format!("还没有复制页面. 先按 {}C.", apply_bg::primary_mod()).into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        let order: Vec<usize> = self
            .spread_clip
            .iter()
            .filter_map(|id| self.doc.pages.iter().position(|p| &p.id == id))
            .collect();
        if order.is_empty() {
            self.status = "复制的页面已经不在了.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        let sel = self.spread_selection_indices();
        let Some(&after) = sel.last() else {
            self.status = "请先选中要贴在其右侧的页面.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        };
        self.insert_spread_copies(&order, after, true, true, cx);
    }

    fn insert_spread_copies(
        &mut self,
        order: &[usize],
        anchor: usize,
        place_after: bool,
        from_clip: bool,
        cx: &mut Context<Self>,
    ) {
        let anchor_name = self
            .doc
            .pages
            .get(anchor)
            .map(|p| p.spread_caption().0)
            .unwrap_or_else(|| "这一页".into());
        self.push_crop_undo_page_structure();
        let Some(new_ids) = self.doc.copy_pages_beside(order, anchor, place_after) else {
            self.page_struct_history.undo.pop();
            self.status = "复制页面失败.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        };
        self.spread_selected = new_ids;
        self.spread_anchor = self.spread_selected.last().cloned();
        self.spread_menu = None;
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        self.request_organize_thumbs(cx);
        let verb = if from_clip { "已粘贴" } else { "已复制" };
        let side = if place_after { "右侧" } else { "左侧" };
        self.status = format!(
            "{verb} {} 页, 紧贴在「{anchor_name}」{side}.",
            self.spread_selected.len()
        )
        .into();
        self.hint = self.status.clone();
        self.refresh_render(cx);
    }

    pub(super) fn insert_spread_blank(&mut self, index: usize, cx: &mut Context<Self>) {
        self.insert_spread_blank_with(index, "已插入空白页, 对开已重配.", cx);
    }

    fn prune_spread_page_ids(&mut self) {
        let live: std::collections::HashSet<&str> =
            self.doc.pages.iter().map(|p| p.id.as_str()).collect();
        self.spread_selected.retain(|id| live.contains(id.as_str()));
        self.spread_clip.retain(|id| live.contains(id.as_str()));
        if self
            .spread_anchor
            .as_ref()
            .is_some_and(|id| !live.contains(id.as_str()))
        {
            self.spread_anchor = None;
        }
    }

    fn insert_spread_blank_with(&mut self, index: usize, status: &str, cx: &mut Context<Self>) {
        self.push_crop_undo_page_structure();
        match self.doc.insert_blank_page(index) {
            Ok(stayed) => {
                self.spread_menu = None;
                self.spread_drag = None;
                self.prune_spread_page_ids();
                self.mark_dirty();
                self.mark_video_pool_dirty_all();
                self.request_organize_thumbs(cx);
                self.status = if stayed.is_some() {
                    status.to_string().into()
                } else {
                    "这一对都是空页, 已经去掉.".into()
                };
                self.hint = self.status.clone();
                self.refresh_render(cx);
            }
            Err(e) => {
                self.page_struct_history.undo.pop();
                self.status = format!("插入空白页失败: {e}").into();
                cx.notify();
            }
        }
    }

    pub(super) fn request_sheet_layout(&mut self, to_spread: bool, cx: &mut Context<Self>) {
        if self.doc.is_spread() == to_spread {
            return;
        }
        let dirty = !self.doc.groups.is_empty()
            || !self.doc.group_masks.is_empty()
            || !self.doc.group_block_layout.is_empty()
            || !self.doc.video_state.video_clips.is_empty();
        if dirty {
            self.dialog = Some(DialogKind::SwitchLayout { to_spread });
            cx.notify();
            return;
        }
        self.apply_sheet_layout(to_spread, cx);
    }

    pub(super) fn apply_sheet_layout(&mut self, to_spread: bool, cx: &mut Context<Self>) {
        self.dialog = None;
        self.clear_tab_hover(cx);
        self.spread_menu = None;
        self.spread_drag = None;
        self.spread_selected.clear();
        self.spread_anchor = None;
        self.spread_clip.clear();
        if to_spread {
            self.doc.enter_spread();
            self.request_organize_thumbs(cx);
            self.status = "已切换为双页对开. 页面按顺序两两成组.".into();
        } else {
            self.doc.leave_spread();
            self.status = "已切换为谱表分块. 请重新识别.".into();
        }
        self.hint = self.status.clone();
        self.mask_target = None;
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        self.canvas_tool = CanvasTool::Normal;
        self.refresh_render(cx);
        self.sync_video_pool(cx);
    }

    fn queue_spread_thumbs(&mut self, cx: &mut Context<Self>) {
        if self.org_thumb_busy || self.spread_thumb_queued {
            return;
        }
        let missing =
            self.doc.pages.iter().any(|p| {
                !p.blank
                    && !self.org_thumbs.contains_key(&p.id)
                    && !self.org_thumb_miss.contains(&p.id)
            });
        if !missing {
            return;
        }
        self.spread_thumb_queued = true;
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| {
                this.spread_thumb_queued = false;
                this.request_organize_thumbs(cx);
            })
            .ok();
        });
    }

    /// `sub` 为真时记下二级菜单的范围, 否则记下主菜单.
    fn track_spread_menu_bounds(entity: Entity<Self>, slot: SpreadMenuSlot) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    let Some(menu) = this.spread_menu.as_mut() else {
                        return;
                    };
                    match slot {
                        SpreadMenuSlot::Main => menu.main_bounds = Some(bounds),
                        SpreadMenuSlot::Sub => menu.sub_bounds = Some(bounds),
                        SpreadMenuSlot::Paste => menu.paste_row = Some(bounds),
                        SpreadMenuSlot::Blank => menu.blank_row = Some(bounds),
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    fn measure_spread_cell(entity: Entity<Self>, idx: usize) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    this.spread_cells.insert(idx, bounds);
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    pub(super) fn spread_menu_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.spread_menu.as_ref() else {
            return div().into_any_element();
        };
        let x = menu.x;
        let y = menu.y;
        let labels = ["复制选中", "粘贴到", "在此页插入空白", "删除选中"];
        let fly = menu.fly;
        let main_bounds = menu.main_bounds;
        let row_bounds = match fly {
            Some(SpreadMenuFly::Paste) => menu.paste_row,
            Some(SpreadMenuFly::Blank) => menu.blank_row,
            None => None,
        };
        let vw = self.viewport_w.max(1.0);
        let vh = self.viewport_h.max(1.0);
        div()
            .id("spread-ctx-backdrop")
            .absolute()
            .left_0()
            .top_0()
            .w(px(vw))
            .h(px(vh))
            .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.dismiss_spread_menu_if_outside(
                    f32::from(ev.position.x),
                    f32::from(ev.position.y),
                    cx,
                );
            }))
            .child(
                div()
                    .id("spread-ctx-menu")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(spread_menu_width(&labels, true)))
                    .occlude()
                    .flex()
                    .flex_col()
                    .p_1()
                    .rounded_md()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(rgb(0xe7e5e4))
                    .shadow_md()
                    .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        this.dismiss_spread_menu_if_outside(
                            f32::from(ev.position.x),
                            f32::from(ev.position.y),
                            cx,
                        );
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(Self::track_spread_menu_bounds(
                        cx.entity().clone(),
                        SpreadMenuSlot::Main,
                    ))
                    .child(self.spread_menu_leaf(
                        "spread_menu_dup",
                        "复制选中",
                        SpreadMenuCmd::Duplicate,
                        cx,
                    ))
                    .child(self.spread_menu_fly(
                        "spread_menu_paste",
                        "粘贴到",
                        SpreadMenuFly::Paste,
                        cx,
                    ))
                    .child(self.spread_menu_fly(
                        "spread_menu_blank",
                        "在此页插入空白",
                        SpreadMenuFly::Blank,
                        cx,
                    ))
                    .child(self.spread_menu_leaf(
                        "spread_menu_del",
                        "删除选中",
                        SpreadMenuCmd::Delete,
                        cx,
                    )),
            )
            .when_some(fly.zip(main_bounds).zip(row_bounds), |d, ((fly, main), row)| {
                d.child(self.spread_submenu(fly, main, row, cx))
            })
            .into_any_element()
    }

    fn spread_set_fly(&mut self, fly: Option<SpreadMenuFly>, cx: &mut Context<Self>) {
        let Some(menu) = self.spread_menu.as_mut() else {
            return;
        };
        if menu.fly == fly {
            return;
        }
        menu.fly = fly;
        menu.sub_bounds = None;
        cx.notify();
    }

    /// 点在主菜单和展开的二级菜单以外时关掉. 二级菜单还没量到时先留着.
    fn dismiss_spread_menu_if_outside(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(menu) = self.spread_menu.as_ref() else {
            return;
        };
        if menu.main_bounds.is_none() {
            return;
        }
        if menu.main_bounds.is_some_and(|b| point_in_bounds(b, x, y)) {
            return;
        }
        if menu.fly.is_some() {
            match menu.sub_bounds {
                Some(b) if point_in_bounds(b, x, y) => return,
                None => return,
                Some(_) => {}
            }
        }
        self.spread_menu = None;
        self.spread_drag = None;
        cx.notify();
    }

    fn spread_menu_act(&mut self, cmd: SpreadMenuCmd, cx: &mut Context<Self>) {
        let Some(idx) = self.spread_menu.as_ref().map(|m| m.page_index) else {
            return;
        };
        match cmd {
            SpreadMenuCmd::Duplicate => self.duplicate_spread_selected(cx),
            SpreadMenuCmd::Paste { after } => self.paste_spread_at(idx, after, cx),
            SpreadMenuCmd::Blank { after } => {
                let at = if after { idx + 1 } else { idx };
                let msg = if after {
                    "已在这一页后面插入空白页."
                } else {
                    "已在这一页前面插入空白页."
                };
                self.insert_spread_blank_with(at, msg, cx);
            }
            SpreadMenuCmd::Delete => self.delete_spread_selected(cx),
        }
    }

    fn paste_spread_at(&mut self, anchor: usize, after: bool, cx: &mut Context<Self>) {
        if self.spread_clip.is_empty() {
            self.spread_menu = None;
            self.status = format!("还没有复制页面. 先按 {}C.", apply_bg::primary_mod()).into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        let order: Vec<usize> = self
            .spread_clip
            .iter()
            .filter_map(|id| self.doc.pages.iter().position(|p| &p.id == id))
            .collect();
        if order.is_empty() {
            self.spread_menu = None;
            self.status = "复制的页面已经不在了.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        if anchor >= self.doc.pages.len() {
            return;
        }
        self.insert_spread_copies(&order, anchor, after, true, cx);
    }

    fn spread_menu_leaf(
        &self,
        id: &'static str,
        label: &'static str,
        cmd: SpreadMenuCmd,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .text_sm()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0xf5f5f4)))
            .child(label)
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if *hovered {
                    this.spread_set_fly(None, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.spread_menu_act(cmd, cx);
                }),
            )
    }

    fn spread_menu_fly(
        &self,
        id: &'static str,
        label: &'static str,
        fly: SpreadMenuFly,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.spread_menu.as_ref().is_some_and(|m| m.fly == Some(fly));
        let fly_hover = fly;
        let slot = match fly {
            SpreadMenuFly::Paste => SpreadMenuSlot::Paste,
            SpreadMenuFly::Blank => SpreadMenuSlot::Blank,
        };
        div()
            .id(id)
            .relative()
            .w_full()
            .px_2()
            .py_1()
            .rounded_sm()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .text_sm()
            .whitespace_nowrap()
            .cursor_pointer()
            .when(open, |d| d.bg(rgb(0xf5f5f4)))
            .hover(|s| s.bg(rgb(0xf5f5f4)))
            .child(Self::track_spread_menu_bounds(cx.entity().clone(), slot))
            .child(label)
            .child(
                div()
                    .ml_3()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(0x78716c))
                    .child("▸"),
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if *hovered {
                    this.spread_set_fly(Some(fly_hover), cx);
                }
            }))
    }

    /// 二级菜单贴在主菜单外缘, 和主菜单留一条缝, 不盖住主菜单.
    fn spread_submenu(
        &self,
        fly: SpreadMenuFly,
        main: Bounds<Pixels>,
        row: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let labels: &[&str] = match fly {
            SpreadMenuFly::Paste => &["左侧", "右侧"],
            SpreadMenuFly::Blank => &["前", "后"],
        };
        let sub_w = spread_menu_width(labels, false);
        let gap = 2.0;
        let mut left = f32::from(main.origin.x) + f32::from(main.size.width) + gap;
        if left + sub_w > self.viewport_w - 4.0 {
            left = (f32::from(main.origin.x) - sub_w - gap).max(4.0);
        }
        let top = f32::from(row.origin.y);
        let id = match fly {
            SpreadMenuFly::Paste => "spread_menu_paste_sub",
            SpreadMenuFly::Blank => "spread_menu_blank_sub",
        };
        div()
            .id(id)
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(sub_w))
            .occlude()
            .flex()
            .flex_col()
            .p_1()
            .rounded_md()
            .bg(ui_kit::card())
            .border_1()
            .border_color(rgb(0xe7e5e4))
            .shadow_md()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(Self::track_spread_menu_bounds(
                cx.entity().clone(),
                SpreadMenuSlot::Sub,
            ))
            .children(labels.iter().enumerate().map(|(i, label)| {
                let cmd = match fly {
                    SpreadMenuFly::Paste => SpreadMenuCmd::Paste { after: i == 1 },
                    SpreadMenuFly::Blank => SpreadMenuCmd::Blank { after: i == 1 },
                };
                div()
                    .id(SharedString::from(format!("{id}_{i}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_sm()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xf5f5f4)))
                    .child(*label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.spread_menu_act(cmd, cx);
                        }),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_window_fits_several_books_per_row() {
        let book = SPREAD_BOOK_W;
        let avail = book * 3.0 + SPREAD_BOOK_GAP * 2.0 + 8.0;
        assert_eq!(spread_books_per_row(avail, 8), 3);
        let w = spread_book_width(avail, 8);
        let used = w * 3.0 + SPREAD_BOOK_GAP * 2.0;
        assert!(used <= avail + 0.01);
        // 每册留 1px, 取整后最后一册才不会被挤到下一行.
        assert!((avail - used - 3.0).abs() < 0.01);
        assert!(w > book);
    }

    #[test]
    fn narrow_window_keeps_one_book() {
        assert_eq!(spread_books_per_row(SPREAD_BOOK_W - 4.0, 4), 1);
        assert_eq!(spread_books_per_row(SPREAD_BOOK_W + 4.0, 4), 1);
        let narrow = SPREAD_BOOK_W - 40.0;
        let w = spread_book_width(narrow, 4);
        assert!((w - (narrow - 1.0)).abs() < 0.01);
        assert_eq!(spread_book_width(0.0, 0), SPREAD_BOOK_W);
    }

    #[test]
    fn book_keeps_page_aspect() {
        let avail = SPREAD_BOOK_W * 2.0 + SPREAD_BOOK_GAP + 30.0;
        let m = spread_fit(avail, 4);
        let ratio = SPREAD_THUMB_W / SPREAD_THUMB_H;
        assert!((m.thumb_w / m.thumb_h - ratio).abs() < 0.001);
        let laid = m.pad * 2.0 + SPREAD_BOOK_BORDER * 2.0 + m.page_w * 2.0 + m.spine_w;
        assert!((laid - m.book_w).abs() < 0.05);
        let cols = spread_books_per_row(avail, 4);
        let used = m.book_w * cols as f32 + SPREAD_BOOK_GAP * cols.saturating_sub(1) as f32;
        assert!(used <= avail + 0.05);
    }

    #[test]
    fn caption_keeps_page_token_with_ellipsis_before_it() {
        let long = "Beethoven_Sonata_Op027_No2_Moonlight_p012";
        let shown = clip_spread_title(long, 120.0);
        assert!(shown.ends_with("_p012"), "{shown}");
        assert!(shown.contains('…'), "{shown}");
        assert!(!shown.starts_with('…'), "{shown}");
        let short = clip_spread_title("scan_p003", 200.0);
        assert_eq!(short, "scan_p003");
        assert_eq!(clip_spread_title("空白页", 200.0), "空白页");
        let copied = clip_spread_title("Beethoven_Sonata_Op027_No2_Moonlight_p012_copy", 150.0);
        assert!(copied.ends_with("_p012_copy"), "{copied}");
        assert!(copied.contains('…'), "{copied}");
    }

    #[test]
    fn caption_without_page_token_puts_ellipsis_in_the_middle() {
        let long = "a_very_long_photo_name_without_number";
        let shown = clip_spread_title(long, 80.0);
        assert!(shown.contains('…'), "{shown}");
        assert!(!shown.ends_with('…'), "{shown}");
        assert!(!shown.starts_with('…'), "{shown}");
        let (head, tail) = shown.split_once('…').unwrap();
        assert!(long.starts_with(head), "{shown}");
        assert!(long.ends_with(tail), "{shown}");
    }

    #[test]
    fn menu_width_follows_the_longest_label() {
        let short = spread_menu_width(&["前", "后"], false);
        let paste = spread_menu_width(&["左侧", "右侧"], false);
        let main = spread_menu_width(&["复制选中", "粘贴到", "在此页插入空白", "删除选中"], true);
        assert!(main > paste);
        assert!(paste > short);
        assert_ne!(main, 168.0);
    }

    #[test]
    fn same_pair_neighbor_drop_does_not_highlight() {
        assert!(classify_spread_drop(4, 1, &[1], 0, true).is_none());
        assert!(classify_spread_drop(4, 0, &[0], 1, false).is_none());
    }

    #[test]
    fn dragging_right_page_into_next_pair_isolates_the_left_page() {
        assert!(matches!(
            classify_spread_drop(4, 1, &[1], 2, false),
            Some(SpreadDropKind::Isolate { at: 1 })
        ));
        assert!(matches!(
            classify_spread_drop(4, 1, &[1, 3], 2, false),
            Some(SpreadDropKind::Move { .. })
        ));
        assert!(classify_spread_drop(4, 2, &[2], 1, true).is_none());
        assert!(matches!(
            classify_spread_drop(4, 3, &[3], 0, false),
            Some(SpreadDropKind::Move { anchor: 0, after: false })
        ));
    }
}
