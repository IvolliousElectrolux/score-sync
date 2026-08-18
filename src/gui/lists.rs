//! 列表行与标签栏辅助渲染数据.

use gpui::SharedString;

#[derive(Clone)]
pub struct ListRow {
    pub id: String,
    pub label: SharedString,
    pub color: u32,
    pub selected: bool,
    /// 在源列表中的下标 (输出组合为 `doc.groups` 下标).
    pub src_index: usize,
}

#[derive(Clone)]
pub struct TabInfo {
    pub index: usize,
    pub label: SharedString,
    pub active: bool,
}

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn measure_item_bounds(
        entity: Entity<Self>,
        key: usize,
        kind: &'static str,
    ) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    match kind {
                        "tab" => {
                            this.tab_bounds.insert(key, bounds);
                        }
                        "group" => {
                            this.group_bounds.insert(key, bounds);
                        }
                        _ => {
                            this.member_bounds.insert(key, bounds);
                        }
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    pub(super) fn item_origin(bounds: Option<&Bounds<Pixels>>, mouse_x: f32, mouse_y: f32) -> (f32, f32) {
        bounds
            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
            .unwrap_or((mouse_x, mouse_y))
    }

    /// 将「落在 anchor 之前/之后」换算成 remove 后再 insert 的下标.
    pub(super) fn reorder_slop_exceeded(dx: f32, dy: f32) -> bool {
        dx * dx + dy * dy >= REORDER_DRAG_SLOP * REORDER_DRAG_SLOP
    }

    pub(super) fn reorder_to_index(from: usize, anchor: usize, after: bool) -> usize {
        if after {
            if from <= anchor {
                anchor
            } else {
                anchor + 1
            }
        } else if from < anchor {
            anchor - 1
        } else {
            anchor
        }
    }

    /// 水平列表 (标签): 原位无反应; 左半→该项左边, 右半→该项右边.
    pub(super) fn resolve_member_drop(
        &self,
        from: usize,
        _x: f32,
        y: f32,
    ) -> (usize, Option<usize>, bool) {
        let n = self.member_list_rows().len();
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.member_bounds.get(&i) else {
                continue;
            };
            let top = f32::from(b.origin.y);
            let bottom = top + f32::from(b.size.height);
            if y < top || y > bottom {
                continue;
            }
            if i == from {
                return (from, None, false);
            }
            let mid = (top + bottom) * 0.5;
            let after = y >= mid;
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    /// 竖直列表 (输出组合): 同成员; 多选时落点忽略移动块内其它项.
    pub(super) fn resolve_group_drop(
        &self,
        from: usize,
        _x: f32,
        y: f32,
    ) -> (usize, Option<usize>, bool) {
        let n = self.doc.groups.len();
        if n == 0 {
            return (from, None, false);
        }
        let moving: HashSet<usize> = self.doc.group_move_indices(from).into_iter().collect();
        for i in 0..n {
            let Some(b) = self.group_bounds.get(&i) else {
                continue;
            };
            let top = f32::from(b.origin.y);
            let bottom = top + f32::from(b.size.height);
            if y < top || y > bottom {
                continue;
            }
            if moving.contains(&i) {
                return (from, None, false);
            }
            let mid = (top + bottom) * 0.5;
            let after = y >= mid;
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }
    pub(super) fn scroll_group_list_to_active(&self) {
        let Some(gid) = self.doc.active_group_id.as_ref() else {
            return;
        };
        let Some(ix) = self.doc.groups.iter().position(|g| &g.id == gid) else {
            return;
        };
        let view_h = f32::from(self.group_scroll.bounds().size.height).max(120.0);
        let target = (ix as f32 * GROUP_ROW_PX - view_h * 0.35).max(0.0);
        self.group_scroll.set_offset(point(px(0.), px(-target)));
    }

    pub(super) fn mask_active_group_index(&self) -> Option<usize> {
        let gid = self
            .mask_target
            .as_ref()
            .or(self.doc.active_group_id.as_ref())?;
        self.doc.groups.iter().position(|g| &g.id == gid)
    }

    /// 将蒙版侧「编辑目标」列表滚到当前组合.
    pub(super) fn scroll_mask_picker_to_active(&self) {
        let Some(ix) = self.mask_active_group_index() else {
            return;
        };
        let picker_h = f32::from(self.mask_group_scroll.bounds().size.height).max(80.0);
        let picker_target = (ix as f32 * MASK_PICKER_ROW_PX - picker_h * 0.35).max(0.0);
        self.mask_group_scroll
            .set_offset(point(px(0.), px(-picker_target)));
    }

    /// 将顶部组合页签滚到当前组合. 仅在切入蒙版面板时用, 点选页签本身不要滚.
    pub(super) fn scroll_mask_tabs_to_active(&self) {
        let Some(ix) = self.mask_active_group_index() else {
            return;
        };
        let view_w = f32::from(self.tab_scroll.bounds().size.width).max(400.0);
        let tab_target = (ix as f32 * MASK_TAB_SLOT_PX - view_w * 0.35).max(0.0);
        self.tab_scroll
            .set_offset(point(px(-tab_target), px(0.)));
    }

    /// 切入蒙版面板时, 侧栏与页签栏都定位到当前组合.
    pub(super) fn scroll_mask_lists_to_active(&self) {
        self.scroll_mask_picker_to_active();
        self.scroll_mask_tabs_to_active();
    }
    pub(super) fn region_list_rows(&self) -> Vec<ListRow> {
        let Some(page) = self.doc.current_page() else {
            return Vec::new();
        };
        let pno = self.doc.page_no(&page.id);
        let mut regions: Vec<_> = page.regions.values().cloned().collect();
        regions.sort_by_key(|r| (r.y0, r.y1));
        regions
            .into_iter()
            .map(|r| ListRow {
                selected: self.doc.selected_region_ids.contains(&r.id),
                color: parse_color_hex(&r.color),
                label: r.label(Some(pno)).into(),
                id: r.id,
                src_index: 0,
            })
            .collect()
    }

    pub(super) fn group_list_rows_in(&self, start: usize, end: usize) -> Vec<ListRow> {
        let n = self.doc.groups.len();
        let start = start.min(n);
        let end = end.min(n);
        let mut by_page: HashMap<usize, Vec<(usize, i32, i32)>> = HashMap::new();
        for (i, g) in self.doc.groups.iter().enumerate() {
            let k = self.doc.group_top_key(g);
            by_page.entry(k.0).or_default().push((i, k.1, k.2));
        }
        for v in by_page.values_mut() {
            v.sort_by_key(|&(_, y0, y1)| (y0, y1));
        }
        (start..end)
            .filter_map(|i| {
                let g = self.doc.groups.get(i)?;
                let mut labels = Vec::new();
                let mut pages_in = HashSet::new();
                for rid in &g.region_ids {
                    if let Some((pi, r)) = self.doc.find_region(rid) {
                        let pno = pi + 1;
                        pages_in.insert(pno);
                        labels.push(format!("P{pno}:{}:{}-{}", r.kind, r.y0, r.y1));
                    }
                }
                let cross = if pages_in.len() > 1 { "跨页 " } else { "" };
                let top = self.doc.group_top_key(g);
                let c = by_page
                    .get(&top.0)
                    .and_then(|v| v.iter().position(|(gi, _, _)| *gi == i))
                    .map(|x| x + 1)
                    .unwrap_or(1);
                let page_no = if top.0 == usize::MAX { 0 } else { top.0 + 1 };
                let text = format!(
                    "{cross}{}. p{page_no}c{c} | [{}]",
                    i + 1,
                    labels.join(", ")
                );
                Some(ListRow {
                    id: g.id.clone(),
                    label: text.into(),
                    color: 0x0f172a,
                    selected: self.doc.group_has_selected_region(g),
                    src_index: i,
                })
            })
            .collect()
    }

    pub(super) fn visible_group_range(&self) -> (usize, usize) {
        let n = self.doc.groups.len();
        if n == 0 {
            return (0, 0);
        }
        if n <= GROUP_LIST_VIRTUAL_THRESHOLD {
            return (0, n);
        }
        let view_h = f32::from(self.group_scroll.bounds().size.height);
        let view_h = if view_h < 8.0 { 400.0 } else { view_h };
        let off = (-f32::from(self.group_scroll.offset().y)).max(0.0);
        let start = ((off / GROUP_ROW_PX).floor() as usize).saturating_sub(8);
        let end = (((off + view_h) / GROUP_ROW_PX).ceil() as usize)
            .saturating_add(8)
            .min(n);
        let start = start.min(n);
        (start, end.max(start))
    }

    pub(super) fn visible_mask_tab_range(&self) -> (usize, usize) {
        let n = self.doc.groups.len();
        if n == 0 {
            return (0, 0);
        }
        if n <= TAB_VIRTUAL_THRESHOLD {
            return (0, n);
        }
        let view_w = f32::from(self.tab_scroll.bounds().size.width);
        let view_w = if view_w < 8.0 { 960.0 } else { view_w };
        let off = (-f32::from(self.tab_scroll.offset().x)).max(0.0);
        let start = ((off / MASK_TAB_SLOT_PX).floor() as usize).saturating_sub(8);
        let end = (((off + view_w) / MASK_TAB_SLOT_PX).ceil() as usize)
            .saturating_add(8)
            .min(n);
        let start = start.min(n);
        (start, end.max(start))
    }

    pub(super) fn visible_mask_picker_range(&self) -> (usize, usize) {
        let n = self.doc.groups.len();
        if n == 0 {
            return (0, 0);
        }
        if n <= GROUP_LIST_VIRTUAL_THRESHOLD {
            return (0, n);
        }
        let view_h = f32::from(self.mask_group_scroll.bounds().size.height);
        let view_h = if view_h < 8.0 { 168.0 } else { view_h };
        let off = (-f32::from(self.mask_group_scroll.offset().y)).max(0.0);
        let start = ((off / MASK_PICKER_ROW_PX).floor() as usize).saturating_sub(8);
        let end = (((off + view_h) / MASK_PICKER_ROW_PX).ceil() as usize)
            .saturating_add(8)
            .min(n);
        let start = start.min(n);
        (start, end.max(start))
    }

    pub(super) fn member_list_rows(&self) -> Vec<ListRow> {
        let Some(g) = self.doc.active_group() else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let r = self.doc.get_region(rid)?;
                Some(ListRow {
                    id: rid.clone(),
                    label: r.label(Some(self.doc.page_no(&r.page_id))).into(),
                    color: parse_color_hex(&r.color),
                    selected: false,
                    src_index: 0,
                })
            })
            .collect()
    }
    pub(super) fn member_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::MemberReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let rows = self.member_list_rows();
        let (label, color) = rows
            .get(*from)
            .map(|r| (r.label.clone(), r.color))
            .unwrap_or_else(|| ("...".into(), 0x0f172a));
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        div()
            .id("member-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0xffffff))
            .text_color(rgb(color))
            .text_sm()
            .border_1()
            .border_color(rgb(0x94a3b8))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    pub(super) fn group_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::GroupReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let rows = self.group_list_rows_in(*from, *from + 1);
        let label = rows
            .first()
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "...".into());
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        div()
            .id("group-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x0f172a))
            .text_xs()
            .border_1()
            .border_color(rgb(0x94a3b8))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }
    pub(super) fn scroll_handle(&self, which: ScrollList) -> &ScrollHandle {
        match which {
            ScrollList::Region => &self.region_scroll,
            ScrollList::Group => &self.group_scroll,
            ScrollList::Member => &self.member_scroll,
            ScrollList::MaskGroup => &self.mask_group_scroll,
            ScrollList::Help => &self.help_scroll,
            ScrollList::Update => &self.update_scroll,
        }
    }

    pub(super) fn apply_scrollbar_drag(&mut self, mouse_x: f32, mouse_y: f32, cx: &mut Context<Self>) {
        let Some(DragKind::Scrollbar {
            which,
            grab,
            vertical,
        }) = self.drag
        else {
            return;
        };
        let handle = self.scroll_handle(which).clone();
        let bounds = handle.bounds();
        if vertical {
            let max_y = f32::from(handle.max_offset().height);
            if max_y <= 0.5 {
                return;
            }
            let track_h = f32::from(bounds.size.height).max(1.0);
            let track_top = f32::from(bounds.origin.y);
            let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
            let travel = (track_h - thumb_h).max(1.0);
            let thumb_top = (mouse_y - grab - track_top).clamp(0.0, travel);
            let frac = thumb_top / travel;
            let ox = handle.offset().x;
            handle.set_offset(point(ox, px(-frac * max_y)));
        } else {
            let max_x = f32::from(handle.max_offset().width);
            if max_x <= 0.5 {
                return;
            }
            let track_w = f32::from(bounds.size.width).max(1.0);
            let track_left = f32::from(bounds.origin.x);
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let thumb_left = (mouse_x - grab - track_left).clamp(0.0, travel);
            let frac = thumb_left / travel;
            let oy = handle.offset().y;
            handle.set_offset(point(px(-frac * max_x), oy));
        }
        cx.notify();
    }
    pub(super) fn attach_scrollbars(
        &self,
        wrap_id: SharedString,
        which: ScrollList,
        handle: &ScrollHandle,
        mut list: Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let max_y = f32::from(handle.max_offset().height);
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_v = max_y > 1.0 && track_h > 1.0;
        let show_h = max_x > 1.0 && track_w > 1.0;

        list = list
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_scroll()
            .track_scroll(handle)
            .scrollbar_width(px(0.));

        let mut row = div()
            .id(SharedString::from(format!("{wrap_id}-row")))
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(list);

        if show_v {
            let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
            let travel = (track_h - thumb_h).max(1.0);
            let off_y = -f32::from(handle.offset().y);
            let frac = (off_y / max_y).clamp(0.0, 1.0);
            let thumb_top = frac * travel;
            row = row.child(
                div()
                    .id(SharedString::from(format!("{wrap_id}-vtrack")))
                    .w(px(10.))
                    .h_full()
                    .flex_shrink_0()
                    .relative()
                    .rounded_sm()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let y = f32::from(ev.position.y);
                            let handle = this.scroll_handle(which).clone();
                            let b = handle.bounds();
                            let th = f32::from(b.size.height).max(1.0);
                            let max = f32::from(handle.max_offset().height);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((th * th) / (th + max)).clamp(24.0, th);
                            let travel = (th - thumb).max(1.0);
                            let track_top = f32::from(b.origin.y);
                            let target = (y - track_top - thumb * 0.5).clamp(0.0, travel);
                            let ox = handle.offset().x;
                            handle.set_offset(point(ox, px(-(target / travel) * max)));
                            this.drag = Some(DragKind::Scrollbar {
                                which,
                                grab: thumb * 0.5,
                                vertical: true,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{wrap_id}-vthumb")))
                            .absolute()
                            .left_0()
                            .top(px(thumb_top))
                            .w_full()
                            .h(px(thumb_h))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .hover(|s| s.bg(rgb(0x64748b)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let y = f32::from(ev.position.y);
                                    let handle = this.scroll_handle(which).clone();
                                    let b = handle.bounds();
                                    let th = f32::from(b.size.height).max(1.0);
                                    let max = f32::from(handle.max_offset().height);
                                    let thumb = if max > 0.5 {
                                        ((th * th) / (th + max)).clamp(24.0, th)
                                    } else {
                                        th
                                    };
                                    let travel = (th - thumb).max(1.0);
                                    let track_top = f32::from(b.origin.y);
                                    let off = -f32::from(handle.offset().y);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_top = track_top + frac * travel;
                                    this.drag = Some(DragKind::Scrollbar {
                                        which,
                                        grab: (y - cur_top).clamp(0.0, thumb),
                                        vertical: true,
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }

        let mut wrap = div()
            .id(wrap_id.clone())
            .relative()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(row);

        if show_h {
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let off_x = -f32::from(handle.offset().x);
            let frac = (off_x / max_x).clamp(0.0, 1.0);
            let thumb_left = frac * travel;
            wrap = wrap.child(
                div()
                    .id(SharedString::from(format!("{wrap_id}-htrack")))
                    .h(px(10.))
                    .w_full()
                    .flex_shrink_0()
                    .relative()
                    .rounded_sm()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let handle = this.scroll_handle(which).clone();
                            let b = handle.bounds();
                            let tw = f32::from(b.size.width).max(1.0);
                            let max = f32::from(handle.max_offset().width);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((tw * tw) / (tw + max)).clamp(24.0, tw);
                            let travel = (tw - thumb).max(1.0);
                            let track_left = f32::from(b.origin.x);
                            let target = (x - track_left - thumb * 0.5).clamp(0.0, travel);
                            let oy = handle.offset().y;
                            handle.set_offset(point(px(-(target / travel) * max), oy));
                            this.drag = Some(DragKind::Scrollbar {
                                which,
                                grab: thumb * 0.5,
                                vertical: false,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{wrap_id}-hthumb")))
                            .absolute()
                            .top_0()
                            .left(px(thumb_left))
                            .h_full()
                            .w(px(thumb_w))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .hover(|s| s.bg(rgb(0x64748b)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let x = f32::from(ev.position.x);
                                    let handle = this.scroll_handle(which).clone();
                                    let b = handle.bounds();
                                    let tw = f32::from(b.size.width).max(1.0);
                                    let max = f32::from(handle.max_offset().width);
                                    let thumb = if max > 0.5 {
                                        ((tw * tw) / (tw + max)).clamp(24.0, tw)
                                    } else {
                                        tw
                                    };
                                    let travel = (tw - thumb).max(1.0);
                                    let track_left = f32::from(b.origin.x);
                                    let off = -f32::from(handle.offset().x);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_left = track_left + frac * travel;
                                    this.drag = Some(DragKind::Scrollbar {
                                        which,
                                        grab: (x - cur_left).clamp(0.0, thumb),
                                        vertical: false,
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }

        wrap
    }

    pub(super) fn side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let region_rows = self.region_list_rows();
        let (g_start, g_end) = self.visible_group_range();
        let group_n = self.doc.groups.len();
        let group_rows = self.group_list_rows_in(g_start, g_end);
        let member_rows = self.member_list_rows();
        let region_open = self.region_panel_open;
        let margin = self.doc.margin;
        let thr = self.doc.ink_threshold;

        let mut panel = div()
            .id("side")
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(0xf1f5f9))
            .child(
                div()
                    .id("region_fold")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if region_open { "▼" } else { "▶" })
                    .child(if region_open {
                        "本页原子块 (点击 y 范围可编辑)"
                    } else {
                        "本页原子块 (折叠; 展开后可点 y 编辑)"
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            // 用 mouse_down: 别处拖拽后在标题上松开不应误触发展开/折叠
                            if this.drag.is_some() {
                                return;
                            }
                            this.region_panel_open = !this.region_panel_open;
                            cx.notify();
                        }),
                    ),
            );

        if region_open {
            let edit_y_input = self.edit_y_input.clone();
            let editing_rid = self.region_y_edit.clone();
            let mut list = div()
                .id("region_list")
                .flex()
                .flex_col()
                .gap_1()
                .border_1()
                .border_color(rgb(0xcbd5e1))
                .rounded_md()
                .p_1()
                .bg(rgb(0xffffff));
            for row in region_rows {
                let rid = row.id.clone();
                let rid_sel = row.id.clone();
                let rid_edit = row.id.clone();
                let editing = editing_rid.as_ref() == Some(&row.id);
                let bg = if row.selected {
                    rgb(0xdbeafe)
                } else {
                    rgb(0xffffff)
                };
                // 当前页原子块: 拆出可点编辑的 y 范围
                let pno = self
                    .doc
                    .current_page()
                    .map(|p| self.doc.page_no(&p.id))
                    .unwrap_or(1);
                let (y0, y1, kind) = self
                    .doc
                    .find_region(&row.id)
                    .map(|(_, r)| (r.y0, r.y1, r.kind.clone()))
                    .unwrap_or((0, 0, String::new()));
                let h = y1 - y0 + 1;
                let kind_pfx = format!("P{pno} {kind}  ");
                let y_label = format!("y={y0}-{y1}");
                let h_label = format!("  h={h}");
                list = list.child(
                    div()
                        .id(SharedString::from(format!("reg-{rid}")))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(bg)
                        .text_sm()
                        .text_color(rgb(row.color))
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .cursor_pointer()
                                .whitespace_nowrap()
                                .child(kind_pfx)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        if this.region_y_edit.is_some() {
                                            this.apply_edit_y(window, cx);
                                        }
                                        this.doc.click_region(&rid_sel, ev.modifiers.control);
                                        this.scroll_group_list_to_active();
                                        this.after_doc_change(cx);
                                    }),
                                ),
                        )
                        .child(if editing {
                            div()
                                .id(SharedString::from(format!("reg-y-edit-{rid}")))
                                .w(px(110.))
                                .h(px(24.))
                                .flex_shrink_0()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _, _, cx| {
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(edit_y_input.clone())
                                .into_any_element()
                        } else {
                            div()
                                .id(SharedString::from(format!("reg-y-{rid}")))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .px_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0xe2e8f0)).rounded_sm())
                                .child(y_label)
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.begin_edit_y(rid_edit.clone(), window, cx);
                                    }),
                                )
                                .into_any_element()
                        })
                        .child(
                            div()
                                .cursor_pointer()
                                .whitespace_nowrap()
                                .child(h_label)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        if this.region_y_edit.is_some() {
                                            this.apply_edit_y(window, cx);
                                        }
                                        this.doc.click_region(&rid, ev.modifiers.control);
                                        this.scroll_group_list_to_active();
                                        this.after_doc_change(cx);
                                    }),
                                ),
                        ),
                );
            }
            panel = panel.child(
                self.attach_scrollbars(
                    "region_scroll_wrap".into(),
                    ScrollList::Region,
                    &self.region_scroll,
                    list,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            );
        }

        panel = panel
            .child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("输出组合 (全部; 排序号全局; 拖拽调序)"),
            );

        let mut glist = div()
            .id("group_list")
            .flex()
            .flex_col()
            .gap_1()
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .p_1()
            .bg(rgb(0xffffff))
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        let group_moving: HashSet<usize> = match &self.drag {
            Some(DragKind::GroupReorder {
                from, armed: true, ..
            }) => self.doc.group_move_indices(*from).into_iter().collect(),
            _ => HashSet::new(),
        };
        let (group_line_at, group_line_after) = match &self.drag {
            Some(DragKind::GroupReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };
        if group_n > GROUP_LIST_VIRTUAL_THRESHOLD && g_start > 0 {
            glist = glist.child(
                div()
                    .h(px(g_start as f32 * GROUP_ROW_PX))
                    .w_full()
                    .flex_shrink_0(),
            );
        }
        for row in group_rows.iter() {
            let idx = row.src_index;
            let gid = row.id.clone();
            let dragging = group_moving.contains(&idx);
            let show_line = group_line_at == Some(idx);
            let bg = if row.selected {
                rgb(0xdbeafe)
            } else {
                rgb(0xffffff)
            };
            glist = glist.child(
                div()
                    .id(SharedString::from(format!("grp-{gid}")))
                    .relative()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(bg)
                    .text_xs()
                    .text_color(rgb(0x0f172a))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !group_line_after, |d| {
                        d.border_t_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && group_line_after, |d| {
                        d.border_b_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "group"))
                    .child(row.label.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) =
                                Self::item_origin(this.group_bounds.get(&idx), mx, my);
                            this.drag = Some(DragKind::GroupReorder {
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
                                ctrl: ev.modifiers.control,
                            });
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        let x = f32::from(ev.position.x);
                        let y = f32::from(ev.position.y);
                        if this.forward_capture_drags(x, y, cx) {
                            return;
                        }
                        if !matches!(this.drag, Some(DragKind::GroupReorder { .. })) {
                            return;
                        }
                        if let Some(DragKind::GroupReorder {
                            from,
                            start_x,
                            start_y,
                            origin_x,
                            origin_y,
                            mut armed,
                            ctrl,
                            ..
                        }) = this.drag.take()
                        {
                            let x = f32::from(ev.position.x);
                            let y = f32::from(ev.position.y);
                            if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                                armed = true;
                            }
                            let (_to, line_at, line_after) = if armed {
                                this.resolve_group_drop(from, x, y)
                            } else {
                                (from, None, false)
                            };
                            this.drag = Some(DragKind::GroupReorder {
                                from,
                                line_at,
                                line_after,
                                start_x,
                                start_y,
                                origin_x,
                                origin_y,
                                x,
                                y,
                                armed,
                                ctrl,
                            });
                            cx.notify();
                        }
                    })),
            );
        }
        if group_n > GROUP_LIST_VIRTUAL_THRESHOLD && g_end < group_n {
            glist = glist.child(
                div()
                    .h(px((group_n - g_end) as f32 * GROUP_ROW_PX))
                    .w_full()
                    .flex_shrink_0(),
            );
        }
        panel = panel
            .child(
                self.attach_scrollbars(
                    "group_scroll_wrap".into(),
                    ScrollList::Group,
                    &self.group_scroll,
                    glist,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            )
            .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("当前组合内成员 (拖拽调序; 可含多页)"),
        );

        let mut mlist = div()
            .id("member_list")
            .flex()
            .flex_col()
            .gap_1()
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .p_1()
            .bg(rgb(0xffffff));
        let drag_from = match &self.drag {
            Some(DragKind::MemberReorder {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };
        let (line_at, line_after) = match &self.drag {
            Some(DragKind::MemberReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };
        for (i, row) in member_rows.iter().enumerate() {
            let idx = i;
            let rid = row.id.clone();
            let dragging = drag_from == Some(idx);
            let show_line = line_at == Some(idx);
            mlist = mlist.child(
                div()
                    .id(SharedString::from(format!("mem-{rid}")))
                    .relative()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(0xffffff))
                    .text_sm()
                    .text_color(rgb(row.color))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !line_after, |d| {
                        d.border_t_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && line_after, |d| {
                        d.border_b_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "member"))
                    .child(row.label.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) =
                                Self::item_origin(this.member_bounds.get(&idx), mx, my);
                            this.drag = Some(DragKind::MemberReorder {
                                from: idx,
                                to: idx,
                                line_at: None,
                                line_after: false,
                                start_x: mx,
                                start_y: my,
                                origin_x: ox,
                                origin_y: oy,
                                x: mx,
                                y: my,
                                armed: false,
                            });
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        let x = f32::from(ev.position.x);
                        let y = f32::from(ev.position.y);
                        if this.forward_capture_drags(x, y, cx) {
                            return;
                        }
                        if !matches!(this.drag, Some(DragKind::MemberReorder { .. })) {
                            return;
                        }
                        if let Some(DragKind::MemberReorder {
                            from,
                            start_x,
                            start_y,
                            origin_x,
                            origin_y,
                            mut armed,
                            ..
                        }) = this.drag.take()
                        {
                            let x = f32::from(ev.position.x);
                            let y = f32::from(ev.position.y);
                            if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                                armed = true;
                            }
                            let (to, line_at, line_after) = if armed {
                                this.resolve_member_drop(from, x, y)
                            } else {
                                (from, None, false)
                            };
                            this.drag = Some(DragKind::MemberReorder {
                                from,
                                to,
                                line_at,
                                line_after,
                                start_x,
                                start_y,
                                origin_x,
                                origin_y,
                                x,
                                y,
                                armed,
                            });
                            cx.notify();
                        }
                    })),
            );
        }
        panel = panel.child(
            self.attach_scrollbars(
                "member_scroll_wrap".into(),
                ScrollList::Member,
                &self.member_scroll,
                mlist,
                cx,
            )
            .flex_1()
            .min_h(px(0.)),
        );

        // params (底部固定)
        let param_input = self.param_input.clone();
        let editing_margin = self.param_edit == Some(ParamEdit::Margin);
        let editing_thr = self.param_edit == Some(ParamEdit::Threshold);
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child("边距px")
                .child(
                    div()
                        .id("margin_dec")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("-")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.margin = (this.doc.margin - 1).max(0);
                                cx.notify();
                            }),
                        ),
                )
                .child(if editing_margin {
                    div()
                        .id("margin_edit")
                        .w(px(56.))
                        .h(px(24.))
                        .flex_shrink_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(param_input.clone())
                        .into_any_element()
                } else {
                    div()
                        .id("margin_val")
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .px_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xe2e8f0)).rounded_sm())
                        .child(format!("{margin}"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.begin_param_edit(ParamEdit::Margin, window, cx);
                            }),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .id("margin_inc")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("+")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.margin = (this.doc.margin + 1).min(80);
                                cx.notify();
                            }),
                        ),
                )
                .child("墨迹阈值")
                .child(
                    div()
                        .id("thr_dec")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("-")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.ink_threshold = (this.doc.ink_threshold - 1).max(1);
                                cx.notify();
                            }),
                        ),
                )
                .child(if editing_thr {
                    div()
                        .id("thr_edit")
                        .w(px(56.))
                        .h(px(24.))
                        .flex_shrink_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(param_input)
                        .into_any_element()
                } else {
                    div()
                        .id("thr_val")
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .px_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xe2e8f0)).rounded_sm())
                        .child(format!("{thr}"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.begin_param_edit(ParamEdit::Threshold, window, cx);
                            }),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .id("thr_inc")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("+")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.ink_threshold = (this.doc.ink_threshold + 1).min(254);
                                cx.notify();
                            }),
                        ),
                ),
            ),
        );
        panel
    }
}
