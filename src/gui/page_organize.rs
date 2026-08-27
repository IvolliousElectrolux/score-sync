//! 「组织页面」弹窗: Adobe 风格缩略图网格, 多选拖动排序.

use super::*;
use super::ScoreSyncApp;
use image::{Frame, ImageBuffer, RgbaImage};
use smallvec::smallvec;
use std::sync::atomic::AtomicUsize;

pub(super) struct OrganizeDrag {
    from: usize,
    line_at: Option<usize>,
    line_after: bool,
    start_x: f32,
    start_y: f32,
    origin_x: f32,
    origin_y: f32,
    x: f32,
    y: f32,
    armed: bool,
    /// 无 Shift/Ctrl: 未拖成就在松手时收成只选这一页
    collapse: bool,
}

pub(super) struct PageOrganizeState {
    selected: HashSet<String>,
    anchor: Option<String>,
    cell_bounds: HashMap<usize, Bounds<Pixels>>,
    drag: Option<OrganizeDrag>,
}

fn rgb_to_render_image(rgb: &image::RgbImage) -> Arc<RenderImage> {
    let (w, h) = rgb.dimensions();
    let src = rgb.as_raw();
    let n = src.len() / 3 * 4;
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(n);
    }
    for (dst, s) in buf.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
        dst[0] = s[2];
        dst[1] = s[1];
        dst[2] = s[0];
        dst[3] = 255;
    }
    let rgba: RgbaImage = ImageBuffer::from_raw(w, h, buf).expect("rgba buffer size matches w*h*4");
    Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]))
}

/// overlay 去掉四边 ORG_SLOT_PAD 后, 可放卡片的区域宽.
fn org_slot_host_w(viewport_w: f32) -> f32 {
    (viewport_w - ORG_SLOT_PAD * 2.0).max(0.0)
}

fn org_grid_inner_width(avail: f32, cell_w: f32, n: usize) -> f32 {
    if avail < 8.0 || n == 0 {
        return 0.0;
    }
    let cell_w = cell_w.max(1.0);
    let cols = ((avail + ORG_GRID_GAP) / (cell_w + ORG_GRID_GAP))
        .floor()
        .max(1.0) as usize;
    let cols = cols.min(n.max(1));
    cols as f32 * cell_w + (cols.saturating_sub(1) as f32) * ORG_GRID_GAP
}

/// 卡片外宽: 整列网格 + 滚动条 + padding/边框, 不超过可放区域.
fn org_card_width(slot_w: f32, n: usize) -> f32 {
    if slot_w < 8.0 {
        return 0.0;
    }
    let avail_grid = (slot_w - ORG_CARD_CHROME_X - ORG_SCROLLBAR_W).max(0.0);
    let inner = org_grid_inner_width(avail_grid, ORG_CELL_W, n.max(1));
    if inner <= 1.0 {
        return 0.0;
    }
    (inner + ORG_SCROLLBAR_W + ORG_CARD_CHROME_X).min(slot_w)
}

fn truncate_name(name: &str, max_chars: usize) -> SharedString {
    let n = name.chars().count();
    if n <= max_chars {
        return name.to_string().into();
    }
    let s: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{s}…").into()
}

impl ScoreSyncApp {
    pub(super) fn toggle_page_organize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pdf_import.is_some() {
            return;
        }
        if matches!(
            self.dialog,
            Some(DialogKind::UnsavedExit | DialogKind::UnsavedNew)
        ) {
            return;
        }
        if self.page_organize.is_some() {
            self.close_page_organize(cx);
            return;
        }
        self.open_page_organize(window, cx);
    }

    fn open_page_organize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.tab_menu = None;
        let cur = self.doc.current_page().map(|p| p.id.clone());
        let mut selected = HashSet::new();
        if let Some(id) = cur.clone() {
            selected.insert(id);
        }
        self.page_organize_scroll.set_offset(point(px(0.), px(0.)));
        self.page_organize = Some(PageOrganizeState {
            selected,
            anchor: cur,
            cell_bounds: HashMap::new(),
            drag: None,
        });
        self.focus_handle.focus(window);
        self.status = format!(
            "组织页面: 拖动排序, {}点击多选 / Shift 连选, Delete 删除.",
            apply_bg::primary_mod()
        )
        .into();
        self.hint = self.status.clone();
        self.request_organize_thumbs(cx);
        cx.notify();
    }

    pub(super) fn close_page_organize(&mut self, cx: &mut Context<Self>) {
        self.page_organize = None;
        if matches!(self.drag, Some(DragKind::Scrollbar { which: ScrollList::PageOrganize, .. })) {
            self.drag = None;
        }
        self.try_show_update_dialog(cx);
        cx.notify();
    }

    pub(super) fn clear_org_thumbs(&mut self) {
        self.org_thumb_gen = self.org_thumb_gen.wrapping_add(1);
        for (_, img) in std::mem::take(&mut self.org_thumbs) {
            self.gpu_drop.push(img);
        }
    }

    pub(super) fn select_all_organize_pages(&mut self, cx: &mut Context<Self>) {
        let ids: HashSet<String> = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        if let Some(st) = self.page_organize.as_mut() {
            st.selected = ids;
            if st.anchor.is_none() {
                st.anchor = self.doc.current_page().map(|p| p.id.clone());
            }
        }
        cx.notify();
    }

    pub(super) fn delete_organize_selected(&mut self, cx: &mut Context<Self>) {
        let idxs: Vec<usize> = {
            let Some(st) = self.page_organize.as_ref() else {
                return;
            };
            self.doc
                .pages
                .iter()
                .enumerate()
                .filter(|(_, p)| st.selected.contains(&p.id))
                .map(|(i, _)| i)
                .collect()
        };
        if idxs.is_empty() {
            return;
        }
        self.push_crop_undo_page_structure();
        let n = idxs.len();
        let dead = self.doc.close_pages_at(&idxs);
        for id in &dead {
            self.crop_histories.remove(id);
            if let Some(img) = self.org_thumbs.remove(id) {
                self.gpu_drop.push(img);
            }
            if let Some(st) = self.page_organize.as_mut() {
                st.selected.remove(id);
            }
        }
        self.status = format!(
            "已删除 {n} 页 ({}Z 可撤回).",
            apply_bg::primary_mod()
        )
        .into();
        self.hint = self.status.clone();
        self.sync_organize_after_doc(cx);
        self.refresh_render(cx);
    }

    pub(super) fn undo_organize(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.page_struct_history.undo.pop() {
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.redo.push(now);
            self.apply_crop_snap(prev);
            self.status = "已撤回页操作.".into();
            self.hint = self.status.clone();
            self.sync_organize_after_doc(cx);
            self.refresh_render(cx);
            return;
        }
        self.status = "没有可撤回的操作.".into();
        cx.notify();
    }

    pub(super) fn redo_organize(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.page_struct_history.redo.pop() {
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.undo.push(now);
            if self.page_struct_history.undo.len() > CROP_HISTORY_LIMIT {
                self.page_struct_history.undo.remove(0);
            }
            self.apply_crop_snap(next);
            self.status = "已重做页操作.".into();
            self.hint = self.status.clone();
            self.sync_organize_after_doc(cx);
            self.refresh_render(cx);
            return;
        }
        self.status = "没有可重做的操作.".into();
        cx.notify();
    }

    fn sync_organize_after_doc(&mut self, cx: &mut Context<Self>) {
        let live: HashSet<String> = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        let fallback = self.doc.current_page().map(|p| p.id.clone());
        if let Some(st) = self.page_organize.as_mut() {
            st.selected.retain(|id| live.contains(id));
            if st.selected.is_empty() {
                if let Some(id) = fallback.clone() {
                    st.selected.insert(id.clone());
                    st.anchor = Some(id);
                }
            } else if st.anchor.as_ref().is_some_and(|id| !live.contains(id)) {
                st.anchor = st.selected.iter().next().cloned().or(fallback);
            }
            st.cell_bounds.clear();
            st.drag = None;
        }
        self.request_organize_thumbs(cx);
        cx.notify();
    }

    pub(super) fn request_organize_thumbs(&mut self, cx: &mut Context<Self>) {
        enum OrgThumbJob {
            Jpeg { id: String, jpeg: PathBuf },
            Mem { id: String, rgb: Arc<image::RgbImage>, disk: PathBuf },
            Png { id: String, png: PathBuf },
        }
        let jobs: Vec<OrgThumbJob> = self
            .doc
            .pages
            .iter()
            .filter(|p| !self.org_thumbs.contains_key(&p.id))
            .map(|p| {
                let jpeg = crate::page_cache::org_thumb_path(&p.disk_path);
                if jpeg.is_file() {
                    OrgThumbJob::Jpeg {
                        id: p.id.clone(),
                        jpeg,
                    }
                } else if let Some(rgb) = p.image.clone() {
                    OrgThumbJob::Mem {
                        id: p.id.clone(),
                        rgb,
                        disk: p.disk_path.clone(),
                    }
                } else {
                    OrgThumbJob::Png {
                        id: p.id.clone(),
                        png: p.disk_path.clone(),
                    }
                }
            })
            .collect();
        if jobs.is_empty() {
            cx.notify();
            return;
        }
        let gen = self.org_thumb_gen.wrapping_add(1);
        self.org_thumb_gen = gen;
        let peak = self
            .doc
            .pages
            .iter()
            .map(|p| p.estimated_bytes())
            .max()
            .unwrap_or(32 * 1024 * 1024);
        let mem_n = crate::page_cache::concurrency_for_peak(peak);
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let workers = mem_n.min(cores).min(jobs.len()).max(1);
        let (tx, rx) = async_channel::unbounded::<(String, Arc<RenderImage>)>();
        let jobs = Arc::new(jobs);
        let next = Arc::new(AtomicUsize::new(0));
        for _ in 0..workers {
            let jobs = jobs.clone();
            let next = next.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= jobs.len() {
                        break;
                    }
                    let rendered = match &jobs[i] {
                        OrgThumbJob::Jpeg { id, jpeg } => crate::page_cache::load_rgb(jpeg)
                            .ok()
                            .map(|rgb| (id.clone(), rgb_to_render_image(&rgb))),
                        OrgThumbJob::Mem { id, rgb, disk } => {
                            let thumb =
                                crate::page_cache::shrink_rgb_max(rgb, ORG_THUMB_MAX_SIDE);
                            let _ = crate::page_cache::save_org_thumb(&thumb, disk);
                            Some((id.clone(), rgb_to_render_image(&thumb)))
                        }
                        OrgThumbJob::Png { id, png } => crate::page_cache::load_rgb(png)
                            .ok()
                            .map(|rgb| {
                                let thumb = crate::page_cache::shrink_rgb_max(
                                    &rgb,
                                    ORG_THUMB_MAX_SIDE,
                                );
                                let _ = crate::page_cache::save_org_thumb(&thumb, png);
                                (id.clone(), rgb_to_render_image(&thumb))
                            }),
                    };
                    if let Some(item) = rendered {
                        let _ = tx.send_blocking(item);
                    }
                }
            });
        }
        drop(tx);
        cx.spawn(async move |this, cx| {
            let mut n = 0u32;
            while let Ok((id, img)) = rx.recv().await {
                n += 1;
                let flush = n == 1 || n % 4 == 0;
                this.update(cx, |view, cx| {
                    if view.org_thumb_gen != gen {
                        return;
                    }
                    if !view.doc.pages.iter().any(|p| p.id == id) {
                        return;
                    }
                    if let Some(old) = view.org_thumbs.insert(id, img) {
                        view.gpu_drop.push(old);
                    }
                    if flush && view.page_organize.is_some() {
                        cx.notify();
                    }
                })
                .ok();
            }
            this.update(cx, |view, cx| {
                if view.org_thumb_gen == gen && view.page_organize.is_some() {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn organize_moving(&self, from: usize) -> Vec<usize> {
        let Some(st) = self.page_organize.as_ref() else {
            return vec![from];
        };
        let idxs: Vec<usize> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| st.selected.contains(&p.id))
            .map(|(i, _)| i)
            .collect();
        if idxs.contains(&from) {
            idxs
        } else {
            vec![from]
        }
    }

    fn organize_click(&mut self, idx: usize, ctrl: bool, shift: bool) {
        let ids: Vec<String> = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        if idx >= ids.len() {
            return;
        }
        let id = ids[idx].clone();
        let Some(st) = self.page_organize.as_mut() else {
            return;
        };
        if shift {
            let anchor_idx = st
                .anchor
                .as_ref()
                .and_then(|aid| ids.iter().position(|x| x == aid))
                .unwrap_or(idx);
            let lo = anchor_idx.min(idx);
            let hi = anchor_idx.max(idx);
            let range: HashSet<String> = ids[lo..=hi].iter().cloned().collect();
            if ctrl {
                st.selected.extend(range);
            } else {
                st.selected = range;
            }
        } else if ctrl {
            if !st.selected.remove(&id) {
                st.selected.insert(id.clone());
            }
            st.anchor = Some(id);
        } else {
            st.selected.clear();
            st.selected.insert(id.clone());
            st.anchor = Some(id);
        }
    }

    fn organize_drag_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(mut drag) = self.page_organize.as_mut().and_then(|st| st.drag.take()) else {
            return;
        };
        if !drag.armed && Self::reorder_slop_exceeded(x - drag.start_x, y - drag.start_y) {
            drag.armed = true;
        }
        let (line_at, line_after) = if drag.armed {
            self.resolve_organize_drop(drag.from, x, y)
        } else {
            (None, false)
        };
        drag.line_at = line_at;
        drag.line_after = line_after;
        drag.x = x;
        drag.y = y;
        if let Some(st) = self.page_organize.as_mut() {
            st.drag = Some(drag);
        }
        cx.notify();
    }

    fn resolve_organize_drop(
        &self,
        from: usize,
        x: f32,
        y: f32,
    ) -> (Option<usize>, bool) {
        let Some(st) = self.page_organize.as_ref() else {
            return (None, false);
        };
        let n = self.doc.pages.len();
        if n == 0 {
            return (None, false);
        }
        let moving: HashSet<usize> = self.organize_moving(from).into_iter().collect();
        let mut last_bottom: Option<f32> = None;
        for i in 0..n {
            let Some(b) = st.cell_bounds.get(&i) else {
                continue;
            };
            let left = f32::from(b.origin.x);
            let top = f32::from(b.origin.y);
            let right = left + f32::from(b.size.width);
            let bottom = top + f32::from(b.size.height);
            last_bottom = Some(bottom);
            if x < left || x > right || y < top || y > bottom {
                continue;
            }
            if moving.contains(&i) {
                return (None, false);
            }
            let after = x >= (left + right) * 0.5;
            return (Some(i), after);
        }
        if last_bottom.is_some_and(|bottom| y > bottom + 8.0) {
            if let Some(anchor) = (0..n).rev().find(|j| !moving.contains(j)) {
                return (Some(anchor), true);
            }
        }
        (None, false)
    }

    fn finish_organize_drag(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.page_organize.as_mut() else {
            return;
        };
        let Some(drag) = st.drag.take() else {
            return;
        };
        if !drag.armed {
            if drag.collapse {
                self.organize_click(drag.from, false, false);
            }
            cx.notify();
            return;
        }
        let Some(anchor) = drag.line_at else {
            cx.notify();
            return;
        };
        let moving = self.organize_moving(drag.from);
        if moving.is_empty() || moving.contains(&anchor) {
            cx.notify();
            return;
        }
        self.push_crop_undo_page_structure();
        self.doc
            .move_pages_block(&moving, anchor, drag.line_after);
        self.status = "已调整页面顺序.".into();
        self.hint = self.status.clone();
        self.after_doc_change(cx);
        self.sync_organize_after_doc(cx);
    }

    fn organize_clear_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(st) = self.page_organize.as_mut() {
            if st.drag.as_ref().is_some_and(|d| d.armed) {
                return;
            }
            st.drag = None;
            st.selected.clear();
        }
        cx.notify();
    }

    fn measure_org_cell(entity: Entity<Self>, idx: usize) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    if let Some(st) = this.page_organize.as_mut() {
                        st.cell_bounds.insert(idx, bounds);
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    pub(super) fn page_organize_overlay(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(st) = self.page_organize.as_ref() else {
            return div().into_any_element();
        };
        let n = self.doc.pages.len();
        let n_sel = st.selected.len();
        let drag_armed = st.drag.as_ref().is_some_and(|d| d.armed);
        let drag_from = st.drag.as_ref().and_then(|d| d.armed.then_some(d.from));
        let (line_at, line_after) = match &st.drag {
            Some(d) if d.armed => (d.line_at, d.line_after),
            _ => (None, false),
        };
        let moving: HashSet<usize> = drag_from
            .map(|from| self.organize_moving(from).into_iter().collect())
            .unwrap_or_default();
        let current_id = self.doc.current_page().map(|p| p.id.clone());
        let host_w = org_slot_host_w(f32::from(window.viewport_size().width));
        let card_w = org_card_width(host_w, n);
        let inner_w = if card_w > 1.0 {
            (card_w - ORG_CARD_CHROME_X - ORG_SCROLLBAR_W).max(0.0)
        } else {
            0.0
        };
        let entity = cx.entity();
        let cells: Vec<(usize, String, SharedString, bool, bool, Option<Arc<RenderImage>>)> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    i,
                    p.id.clone(),
                    truncate_name(&p.title(), 16),
                    st.selected.contains(&p.id),
                    current_id.as_deref() == Some(p.id.as_str()),
                    self.org_thumbs.get(&p.id).cloned(),
                )
            })
            .collect();

        let mut tiles = div()
            .id("org_pages_tiles")
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(ORG_GRID_GAP))
            .py_3()
            .when(inner_w > 1.0, |d| d.w(px(inner_w)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.organize_clear_selection(cx);
                }),
            );
        if n == 0 {
            tiles = tiles.child(
                div()
                    .w_full()
                    .py_8()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(0x64748b))
                    .child("还没有页面. 先打开图片或 PDF."),
            );
        }
        for (idx, _pid, caption, selected, is_current, thumb) in cells {
            let dragging = moving.contains(&idx);
            let show_line = line_at == Some(idx);
            let border = if selected {
                rgb(0x2563eb)
            } else if is_current {
                rgb(0x0f172a)
            } else {
                rgb(0xcbd5e1)
            };
            let bg = if selected { rgb(0xeff6ff) } else { rgb(0xffffff) };
            let paint = thumb.clone();
            tiles = tiles.child(
                div()
                    .id(SharedString::from(format!("org_page-{idx}")))
                    .relative()
                    .flex_shrink_0()
                    .w(px(ORG_CELL_W))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .rounded_md()
                    .bg(bg)
                    .border_2()
                    .border_color(border)
                    .cursor_pointer()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !line_after, |d| {
                        d.border_l_4().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && line_after, |d| {
                        d.border_r_4().border_color(rgb(0xf59e0b))
                    })
                    .hover(|s| {
                        if selected {
                            s
                        } else {
                            s.bg(rgb(0xf8fafc))
                        }
                    })
                    .child(Self::measure_org_cell(entity.clone(), idx))
                    .child(
                        div()
                            .id(SharedString::from(format!("org_page_thumb-{idx}")))
                            .w(px(ORG_THUMB_W))
                            .h(px(ORG_THUMB_H))
                            .flex_shrink_0()
                            .bg(rgb(0xf1f5f9))
                            .border_1()
                            .border_color(rgb(0xe2e8f0))
                            .overflow_hidden()
                            .relative()
                            .child(
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
                                .size_full(),
                            )
                            .when(thumb.is_none(), |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_xs()
                                        .text_color(rgb(0x94a3b8))
                                        .child("…"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(if selected {
                                        rgb(0x1d4ed8)
                                    } else {
                                        rgb(0x0f172a)
                                    })
                                    .child(format!("{}", idx + 1)),
                            )
                            .when(is_current, |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(rgb(0x2563eb))
                                        .text_color(rgb(0xffffff))
                                        .child("当前"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(caption),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            if ev.click_count >= 2 {
                                this.switch_page(idx, cx);
                                this.close_page_organize(cx);
                                return;
                            }
                            let Some(st) = this.page_organize.as_ref() else {
                                return;
                            };
                            let id = this.doc.pages.get(idx).map(|p| p.id.clone());
                            let already = id
                                .as_ref()
                                .is_some_and(|id| st.selected.contains(id));
                            let ctrl = is_primary_mod(&ev.modifiers);
                            let shift = ev.modifiers.shift;
                            if !(already && !ctrl && !shift) {
                                this.organize_click(idx, ctrl, shift);
                            }
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) = Self::item_origin(
                                this.page_organize
                                    .as_ref()
                                    .and_then(|s| s.cell_bounds.get(&idx)),
                                mx,
                                my,
                            );
                            if let Some(st) = this.page_organize.as_mut() {
                                st.drag = Some(OrganizeDrag {
                                    from: idx,
                                    line_at: None,
                                    line_after: false,
                                    start_x: mx,
                                    start_y: my,
                                    origin_x: ox,
                                    origin_y: oy,
                                    x: mx,
                                    y: my,
                                    armed: false,
                                    collapse: !ctrl && !shift,
                                });
                            }
                            let _ = window;
                            cx.notify();
                        }),
                    ),
            );
        }

        let header = if n_sel > 0 {
            format!("组织页面  ·  共 {n} 页  ·  已选 {n_sel}")
        } else {
            format!("组织页面  ·  共 {n} 页")
        };
        let hint = format!(
            "拖动排序 · {}点击多选 · Shift 连选 · Delete 删除 · {}Z/Y 撤重 · 双击跳转",
            apply_bg::primary_mod(),
            apply_bg::primary_mod()
        );
        let ghost = self.organize_drag_ghost(cx);

        div()
            .id("org_pages_backdrop")
            .absolute()
            .inset_0()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let x = f32::from(ev.position.x);
                let y = f32::from(ev.position.y);
                if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                    this.apply_scrollbar_drag(x, y, cx);
                } else {
                    this.organize_drag_move(x, y, cx);
                }
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                        this.drag = None;
                    }
                    this.finish_organize_drag(cx);
                    cx.stop_propagation();
                }),
            )
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
            .child(
                div()
                    .id("org_pages_slot")
                    .absolute()
                    .inset_0()
                    .p(px(ORG_SLOT_PAD))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .child(
                                div()
                                    .size_full()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .id("org_pages_card")
                                            .when(card_w > 1.0, |d| {
                                                d.w(px(card_w)).flex_shrink_0()
                                            })
                                            .when(card_w <= 1.0, |d| d.w_full())
                                            .h_full()
                                            .min_w(px(0.))
                                            .min_h(px(0.))
                                            .px(px(ORG_CARD_PAD_X))
                                            .py(px(ORG_CARD_PAD_X))
                                            .rounded_lg()
                                            .bg(rgb(0xffffff))
                                            .border_1()
                                            .border_color(rgb(0x94a3b8))
                                            .flex()
                                            .flex_col()
                                            .gap_3()
                                            .overflow_hidden()
                                            .on_mouse_move(cx.listener(
                                                |this, ev: &MouseMoveEvent, _, cx| {
                                                    let x = f32::from(ev.position.x);
                                                    let y = f32::from(ev.position.y);
                                                    if matches!(
                                                        this.drag,
                                                        Some(DragKind::Scrollbar { .. })
                                                    ) {
                                                        this.apply_scrollbar_drag(x, y, cx);
                                                    } else {
                                                        this.organize_drag_move(x, y, cx);
                                                    }
                                                },
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                                            )
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_lg()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child(header),
                                            )
                                            .child(
                                                div()
                                                    .id("org_pages_body")
                                                    .flex_1()
                                                    .w_full()
                                                    .min_h(px(0.))
                                                    .min_w(px(0.))
                                                    .flex()
                                                    .flex_col()
                                                    .overflow_hidden()
                                                    .bg(rgb(0xf8fafc))
                                                    .rounded_md()
                                                    .child(
                                                        self.attach_scrollbars(
                                                            "org_pages_scroll".into(),
                                                            ScrollList::PageOrganize,
                                                            &self.page_organize_scroll,
                                                            tiles,
                                                            cx,
                                                        )
                                                        .flex_1()
                                                        .min_h(px(0.))
                                                        .min_w(px(0.))
                                                        .w_full(),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.))
                                                            .text_xs()
                                                            .text_color(rgb(0x64748b))
                                                            .child(hint),
                                                    )
                                                    .child(self.btn(
                                                        "org_pages_done",
                                                        "完成",
                                                        true,
                                                        |this, _, cx| {
                                                            this.close_page_organize(cx);
                                                        },
                                                        cx,
                                                    )),
                                            ),
                                    ),
                            ),
                    ),
            )
            .when(drag_armed, |d| d.child(ghost))
            .into_any_element()
    }

    fn organize_drag_ghost(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(st) = self.page_organize.as_ref() else {
            return div().into_any_element();
        };
        let Some(OrganizeDrag {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &st.drag
        else {
            return div().into_any_element();
        };
        let count = self.organize_moving(*from).len().max(1);
        let thumb = self
            .doc
            .pages
            .get(*from)
            .and_then(|p| self.org_thumbs.get(&p.id).cloned());
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        let label = format!("{}", *from + 1);
        div()
            .id("org_page_drag_ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.88)
            .w(px(ORG_CELL_W))
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(rgb(0xffffff))
            .border_2()
            .border_color(rgb(0x2563eb))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.organize_drag_move(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_organize_drag(cx);
                }),
            )
            .child(
                div()
                    .w(px(ORG_THUMB_W))
                    .h(px(ORG_THUMB_H))
                    .bg(rgb(0xf1f5f9))
                    .overflow_hidden()
                    .relative()
                    .child({
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
                    .when(count > 1, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(4.))
                                .right(px(4.))
                                .px_1()
                                .rounded_sm()
                                .bg(rgb(0x2563eb))
                                .text_color(rgb(0xffffff))
                                .text_xs()
                                .child(format!("{count}")),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(label),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_host_matches_viewport_minus_pad() {
        assert!((org_slot_host_w(1600.0) - (1600.0 - ORG_SLOT_PAD * 2.0)).abs() < 0.01);
        assert_eq!(org_slot_host_w(10.0), 0.0);
    }

    #[test]
    fn card_width_matches_packed_columns() {
        for slot in [360.0, 800.0, 1280.0, 1600.0, 1920.0] {
            for n in [1usize, 3, 9, 33, 80] {
                let w = org_card_width(slot, n);
                if w <= 1.0 {
                    continue;
                }
                let inner = w - ORG_CARD_CHROME_X - ORG_SCROLLBAR_W;
                let packed = org_grid_inner_width(inner, ORG_CELL_W, n);
                assert!(
                    (inner - packed).abs() < 0.01,
                    "slot={slot} n={n} inner={inner} packed={packed}"
                );
                assert!(w <= slot + 0.01);
            }
        }
    }
}
