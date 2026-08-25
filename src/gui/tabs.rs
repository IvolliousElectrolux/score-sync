//! 分块页签 / 蒙版组合页签.

use super::lists::TabInfo;
use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    /// 页签水平滚动条拖拽用的 handle: 蒙版面板用独立的 `mask_tab_scroll`,
    /// 其余面板 (分块/工程) 用 `tab_scroll`; 两者互不干扰, 切面板不抽搐.
    pub(super) fn tab_hscroll_handle(&self) -> ScrollHandle {
        if self.side_tool == SideTool::Mask {
            self.mask_tab_scroll.clone()
        } else {
            self.tab_scroll.clone()
        }
    }

    /// 页签拖拽跟手. 每个页签都有 `block_mouse_except_scroll`, 父级
    /// `tab_bar_row` 在页签上方时 `is_hovered` 为 false, 必须像「输出组合」
    /// 那样在命中的那一项上更新, 否则拖到其他页签只会触发 hover 整页重绘、虚影卡住.
    pub(super) fn apply_tab_reorder_at(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(DragKind::TabReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            mut armed,
            ..
        }) = self.drag.take()
        else {
            return;
        };
        if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
            armed = true;
        }
        let (to, line_at, line_after) = if armed {
            self.resolve_tab_drop(from, x, y)
        } else {
            (from, None, false)
        };
        self.drag = Some(DragKind::TabReorder {
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

    pub(super) fn resolve_tab_drop(
        &self,
        from: usize,
        x: f32,
        _y: f32,
    ) -> (usize, Option<usize>, bool) {
        let n = self.doc.pages.len();
        if n == 0 {
            return (from, None, false);
        }
        // 只扫当前虚拟化渲染出来的可见范围: 范围外的下标本来就不在
        // `tab_bounds` 里, 大工程 (几百页) 拖动排序时每帧全量扫一遍 `0..n`
        // 会白白浪费大量无意义的 HashMap 查找, 是页签拖拽卡顿的根因之一.
        let (start, end) = self.visible_tab_range();
        for i in start..end {
            let Some(b) = self.tab_bounds.get(&i) else {
                continue;
            };
            let left = f32::from(b.origin.x);
            let right = left + f32::from(b.size.width);
            if x < left || x > right {
                continue;
            }
            if i == from {
                return (from, None, false);
            }
            let mid = (left + right) * 0.5;
            let after = x >= mid;
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    /// 竖直列表 (成员): 原位无反应; 上半→该项上边, 下半→该项下边.
    pub(super) fn switch_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.doc.pages.len() {
            return;
        }
        self.doc.current_page_index = index;
        // 点选页签只激活, 不改栏内滚动 (与蒙版页签一致; 切入分块面板时再定位).
        if self.doc.pages[index].image.is_some() {
            self.request_page_window(cx);
            if self.pending_redetect {
                self.flush_pending_redetect(cx);
            } else {
                self.refresh_render(cx);
            }
        } else {
            self.render_image = None;
            self.img_w = self.doc.pages[index].width();
            self.img_h = self.doc.pages[index].height();
            self.request_page_window(cx);
            if self.pending_redetect {
                self.status = "页图加载中, 到齐后重新识别…".into();
                self.hint = self.status.clone();
            }
            cx.notify();
        }
    }

    pub(super) fn close_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.doc.pages.len() {
            return;
        }
        self.push_crop_undo_page_structure();
        let pid = self.doc.pages.get(index).map(|p| p.id.clone());
        if self.doc.close_page_at(index) {
            if let Some(id) = pid {
                self.crop_histories.remove(&id);
            }
            self.status = format!(
                "已关闭页面 ({}Z 可撤回).",
                apply_bg::primary_mod()
            )
            .into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
        } else {
            // close 失败则丢掉刚压的空操作
            self.page_struct_history.undo.pop();
        }
    }

    pub(super) fn copy_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(at) = self.doc.copy_page_at(index) {
            self.status = format!(
                "已复制第 {} 页 → 新标签 {}",
                index + 1,
                at + 1
            )
            .into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
        }
    }
    pub(super) fn tab_infos(&self) -> Vec<TabInfo> {
        let (start, end) = self.visible_tab_range();
        (start..end)
            .map(|i| TabInfo {
                index: i,
                label: self.page_tab_label(i),
                active: i == self.doc.current_page_index,
            })
            .collect()
    }

    pub(super) fn page_tab_label(&self, i: usize) -> SharedString {
        self.page_tab_caption(i).0
    }

    fn page_tab_caption(&self, i: usize) -> (SharedString, SharedString) {
        let Some(p) = self.doc.pages.get(i) else {
            return ("?".into(), "?".into());
        };
        let has_sel = p
            .regions
            .keys()
            .any(|rid| self.doc.selected_region_ids.contains(rid));
        let mark = if has_sel { "●" } else { "" };
        let (show, full) = format_page_tab_caption(mark, &p.tab_badge(i + 1), &p.title());
        (show.into(), full.into())
    }

    pub(super) fn note_tab_hover(&mut self, idx: usize, cx: &mut Context<Self>) {
        if matches!(self.drag, Some(DragKind::TabReorder { .. }) | Some(DragKind::TabHScroll { .. }))
        {
            self.clear_tab_hover(cx);
            return;
        }
        if self.tab_hover_idx == Some(idx) {
            return;
        }
        self.tab_hover_idx = Some(idx);
        self.tab_tooltip = None;
        self.tab_hover_gen = self.tab_hover_gen.wrapping_add(1);
        let gen = self.tab_hover_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            this.update(cx, |view, cx| {
                if view.tab_hover_gen != gen || view.tab_hover_idx != Some(idx) {
                    return;
                }
                let (show, full) = view.page_tab_caption(idx);
                if show == full {
                    return;
                }
                let (x, y) = view
                    .tab_bounds
                    .get(&idx)
                    .map(|b| {
                        (
                            f32::from(b.origin.x),
                            f32::from(b.origin.y) + f32::from(b.size.height) + 6.0,
                        )
                    })
                    .unwrap_or((0.0, 0.0));
                view.tab_tooltip = Some(TabTooltip {
                    page_index: idx,
                    x,
                    y,
                    text: full,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn clear_tab_hover(&mut self, cx: &mut Context<Self>) {
        if self.tab_hover_idx.is_none() && self.tab_tooltip.is_none() {
            return;
        }
        self.tab_hover_idx = None;
        self.tab_tooltip = None;
        self.tab_hover_gen = self.tab_hover_gen.wrapping_add(1);
        cx.notify();
    }

    /// 虚拟页签槽宽 (标签宽 + gap). 有实测则用平均值, 避免占位和真标签对不上在末尾抖动.
    pub(super) fn tab_slot_px(&self) -> f32 {
        const GAP: f32 = 4.0;
        let mut sum = 0.0;
        let mut n = 0u32;
        for b in self.tab_bounds.values() {
            let w = f32::from(b.size.width);
            if w > 8.0 {
                sum += w;
                n += 1;
            }
        }
        if n >= 3 {
            (sum / n as f32 + GAP).clamp(64.0, 360.0)
        } else {
            TAB_SLOT_PX + GAP
        }
    }

    pub(super) fn visible_tab_range(&self) -> (usize, usize) {
        let n = self.doc.pages.len();
        if n == 0 {
            return (0, 0);
        }
        if n <= TAB_VIRTUAL_THRESHOLD {
            return (0, n);
        }
        let slot = self.tab_slot_px().max(1.0);
        let view_w = f32::from(self.tab_scroll.bounds().size.width);
        let view_w = if view_w < 32.0 { 960.0 } else { view_w };
        let off = (-f32::from(self.tab_scroll.offset().x)).max(0.0);
        // 只按当前滚动窗口虚拟化, 不因「当前页」扩范围 (点选页签会跳).
        let start = ((off / slot).floor() as usize).saturating_sub(8);
        let end = (((off + view_w) / slot).ceil() as usize)
            .saturating_add(8)
            .min(n);
        let start = start.min(n);
        (start, end.max(start).min(n))
    }

    /// 将分块页签滚到指定页. 仅在切入分块/工程面板时用, 点选页签本身不要滚.
    pub(super) fn scroll_page_tabs_to_index(&self, ix: usize) {
        let n = self.doc.pages.len();
        if n == 0 {
            return;
        }
        let slot = self.tab_slot_px().max(1.0);
        let view_w = f32::from(self.tab_scroll.bounds().size.width);
        let view_w = if view_w < 32.0 { 960.0 } else { view_w };
        // 用槽宽估算 max, 不读上一帧 (蒙版页签/未布局完) 的 max_offset, 否则点末页会来回夹.
        let add_w = 36.0;
        let max = (n as f32 * slot + add_w - view_w).max(0.0);
        let target = (ix as f32 * slot - view_w * 0.35).clamp(0.0, max);
        self.tab_scroll
            .set_offset(point(px(-target), px(0.)));
    }
    pub(super) fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.side_tool == SideTool::Mask {
            self.mask_group_tab_bar(cx).into_any_element()
        } else {
            self.page_tab_bar(cx).into_any_element()
        }
    }

    /// 蒙版模式标签: 各组合 (含所属页提示).
    pub(super) fn mask_group_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_gid = self
            .mask_target
            .clone()
            .or_else(|| self.doc.active_group_id.clone());
        let handle = &self.mask_tab_scroll;
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_h = max_x > 1.0 && track_w > 1.0;

        let mut row = div()
            .id("mask_tab_bar_row")
            .w_full()
            .min_w(px(0.))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .overflow_x_scroll()
            .track_scroll(handle)
            .scrollbar_width(px(0.))
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        let n = self.doc.groups.len();
        let (start, end) = self.visible_mask_tab_range();
        if n > TAB_VIRTUAL_THRESHOLD && start > 0 {
            row = row.child(
                div()
                    .w(px(start as f32 * MASK_TAB_SLOT_PX))
                    .h(px(1.))
                    .flex_shrink_0(),
            );
        }
        for i in start..end {
            let Some(g) = self.doc.groups.get(i) else {
                continue;
            };
            let gid = g.id.clone();
            let active = active_gid.as_ref() == Some(&gid);
            let label = self.doc.group_crop_label(i);
            let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
            let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
            row = row.child(
                div()
                    .id(SharedString::from(format!("mask-tab-{gid}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_sm()
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // 拖标签栏滚动条时松手落在页签上, 不能当成点击跳转.
                            if matches!(this.drag, Some(DragKind::TabHScroll { .. })) {
                                return;
                            }
                            this.set_mask_target(gid.clone(), true, cx);
                        }),
                    ),
            );
        }
        if n > TAB_VIRTUAL_THRESHOLD && end < n {
            row = row.child(
                div()
                    .w(px((n - end) as f32 * MASK_TAB_SLOT_PX))
                    .h(px(1.))
                    .flex_shrink_0(),
            );
        }

        let mut wrap = div()
            .id("tab_bar")
            .flex()
            .flex_col()
            .w_full()
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .bg(rgb(0xf8fafc))
            .child(row);
        if show_h {
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let off_x = -f32::from(handle.offset().x);
            let frac = (off_x / max_x).clamp(0.0, 1.0);
            let thumb_left = frac * travel;
            wrap = wrap.child(
                div()
                    .id("mask_tab_htrack")
                    .h(px(8.))
                    .w_full()
                    .relative()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let handle = this.tab_hscroll_handle();
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
                            handle.set_offset(point(px(-(target / travel) * max), px(0.)));
                            this.drag = Some(DragKind::TabHScroll {
                                grab: thumb * 0.5,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id("mask_tab_hthumb")
                            .absolute()
                            .top_0()
                            .left(px(thumb_left))
                            .h_full()
                            .w(px(thumb_w))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let x = f32::from(ev.position.x);
                                    let handle = this.tab_hscroll_handle();
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
                                    this.drag = Some(DragKind::TabHScroll {
                                        grab: (x - cur_left).clamp(0.0, thumb),
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }
        wrap
    }

    pub(super) fn page_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.doc.pages.len();
        let (start, end) = self.visible_tab_range();
        let tabs = self.tab_infos();
        let handle = &self.tab_scroll;
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_h = max_x > 1.0 && track_w > 1.0;
        let tab_reordering = matches!(self.drag, Some(DragKind::TabReorder { .. }));
        let drag_from = match &self.drag {
            Some(DragKind::TabReorder {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };

        let mut row = div()
            .id("tab_bar_row")
            .w_full()
            .min_w(px(0.))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .overflow_x_scroll()
            .track_scroll(handle)
            .scrollbar_width(px(0.))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.clear_tab_hover(cx);
                cx.notify();
            }));
        let slot = self.tab_slot_px();
        if n > TAB_VIRTUAL_THRESHOLD && start > 0 {
            row = row.child(
                div()
                    .w(px(start as f32 * slot))
                    .h(px(1.))
                    .flex_shrink_0(),
            );
        }
        for tab in tabs {
            let idx = tab.index;
            let active = tab.active;
            let dragging = drag_from == Some(idx);
            let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
            let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
            row = row.child(
                div()
                    .id(SharedString::from(format!("tab-{idx}")))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_sm()
                    .when(!tab_reordering, |d| d.cursor_pointer())
                    .block_mouse_except_scroll()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(!tab_reordering, |d| {
                        d.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.note_tab_hover(idx, cx);
                            } else if this.tab_hover_idx == Some(idx) {
                                this.clear_tab_hover(cx);
                            }
                        }))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "tab"))
                    .child(tab.label.clone())
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close-{idx}")))
                            .px_1()
                            .rounded_sm()
                            .when(!tab_reordering, |d| {
                                d.hover(|s| s.bg(rgb(0x94a3b8)))
                                    .block_mouse_except_scroll()
                            })
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.tab_close_press = Some(idx);
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let press = this.tab_close_press.take();
                                    // 必须按下就在这一叉上: 从别的页签拖过来松手不算关闭.
                                    if press != Some(idx) {
                                        return;
                                    }
                                    if matches!(this.drag, Some(DragKind::TabReorder { .. })) {
                                        return;
                                    }
                                    this.close_page(idx, cx);
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            if matches!(this.drag, Some(DragKind::TabHScroll { .. })) {
                                return;
                            }
                            this.clear_tab_hover(cx);
                            this.switch_page(idx, cx);
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) = Self::item_origin(
                                this.tab_bounds.get(&idx),
                                mx,
                                my,
                            );
                            this.tab_add_press = false;
                            this.tab_close_press = None;
                            this.drag = Some(DragKind::TabReorder {
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
                    // 页签挡住了父级 hover, 必须在自身上跟手 (同输出组合每一行).
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        let x = f32::from(ev.position.x);
                        let y = f32::from(ev.position.y);
                        if this.forward_capture_drags(x, y, cx) {
                            return;
                        }
                        if !matches!(this.drag, Some(DragKind::TabReorder { .. })) {
                            return;
                        }
                        this.apply_tab_reorder_at(x, y, cx);
                        cx.stop_propagation();
                    }))
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                            this.clear_tab_hover(cx);
                            this.tab_menu = Some(TabContextMenu {
                                page_index: idx,
                                x: f32::from(ev.position.x),
                                y: f32::from(ev.position.y),
                            });
                            cx.notify();
                        }),
                    ),
            );
        }
        if n > TAB_VIRTUAL_THRESHOLD && end < n {
            row = row.child(
                div()
                    .w(px((n - end) as f32 * slot))
                    .h(px(1.))
                    .flex_shrink_0(),
            );
        }
        row = row.child(
            div()
                .id("tab-add")
                .px_2()
                .py_1()
                .rounded_md()
                .bg(rgb(0xcbd5e1))
                .cursor_pointer()
                .flex_shrink_0()
                .child("+")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.tab_add_press = this.drag.is_none();
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        let press = this.tab_add_press;
                        this.tab_add_press = false;
                        if !press {
                            return;
                        }
                        if matches!(
                            this.drag,
                            Some(DragKind::TabReorder { .. })
                                | Some(DragKind::TabHScroll { .. })
                                | Some(DragKind::Scrollbar { .. })
                        ) {
                            return;
                        }
                        this.open_file(window, cx);
                    }),
                ),
        );
        row = row
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let x = f32::from(ev.position.x);
                let y = f32::from(ev.position.y);
                if this.forward_capture_drags(x, y, cx) {
                    return;
                }
                if !matches!(this.drag, Some(DragKind::TabReorder { .. })) {
                    return;
                }
                this.apply_tab_reorder_at(x, y, cx);
                // 页签间隙仍由本行处理; 阻止再冒泡到根节点重复扫 resolve_tab_drop.
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if !matches!(this.drag, Some(DragKind::TabReorder { .. })) {
                        return;
                    }
                    if let Some(DragKind::TabReorder {
                        from, to, armed, ..
                    }) = this.drag.take()
                    {
                        if armed && from != to {
                            this.push_crop_undo_all_pages();
                            this.doc.move_page(from, to);
                            this.after_doc_change(cx);
                        } else {
                            cx.notify();
                        }
                    }
                }),
            );

        let mut wrap = div()
            .id("tab_bar")
            .flex()
            .flex_col()
            .w_full()
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .bg(rgb(0xf8fafc))
            .child(row);

        if show_h {
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let off_x = -f32::from(handle.offset().x);
            let frac = (off_x / max_x).clamp(0.0, 1.0);
            let thumb_left = frac * travel;
            wrap = wrap.child(
                div()
                    .id("tab_htrack")
                    .h(px(8.))
                    .w_full()
                    .relative()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let handle = this.tab_scroll.clone();
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
                            handle.set_offset(point(px(-(target / travel) * max), px(0.)));
                            this.drag = Some(DragKind::TabHScroll {
                                grab: thumb * 0.5,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id("tab_hthumb")
                            .absolute()
                            .top_0()
                            .left(px(thumb_left))
                            .h_full()
                            .w(px(thumb_w))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let x = f32::from(ev.position.x);
                                    let handle = this.tab_scroll.clone();
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
                                    this.drag = Some(DragKind::TabHScroll {
                                        grab: (x - cur_left).clamp(0.0, thumb),
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }
        wrap
    }

    /// 插入位置指示: 绝对定位细线, 不改页签宽度, 避免拖过其他页签时整栏回流.
    pub(super) fn tab_drop_line_overlay(&self) -> impl IntoElement {
        let Some(DragKind::TabReorder {
            line_at: Some(i),
            line_after,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let Some(b) = self.tab_bounds.get(i) else {
            return div().into_any_element();
        };
        let left = f32::from(b.origin.x);
        let x = if *line_after {
            left + f32::from(b.size.width)
        } else {
            left
        };
        div()
            .id("tab-drop-line")
            .absolute()
            .left(px(x - 1.0))
            .top(px(f32::from(b.origin.y)))
            .w(px(2.))
            .h(px(f32::from(b.size.height)))
            .bg(rgb(0xf59e0b))
            .into_any_element()
    }

    pub(super) fn tab_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::TabReorder {
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
        let label = self.page_tab_label(*from);
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        div()
            .id("tab-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x2563eb))
            .text_color(rgb(0xffffff))
            .text_sm()
            .border_1()
            .border_color(rgb(0x1e40af))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    pub(super) fn tab_tooltip_overlay(&self) -> impl IntoElement {
        let Some(ref tip) = self.tab_tooltip else {
            return div().into_any_element();
        };
        div()
            .id(SharedString::from(format!("tab-tooltip-{}", tip.page_index)))
            .absolute()
            .left(px(tip.x))
            .top(px(tip.y))
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0xffffe1))
            .border_1()
            .border_color(rgb(0x6b6b6b))
            .text_color(rgb(0x000000))
            .text_xs()
            .whitespace_nowrap()
            .shadow_sm()
            .child(tip.text.clone())
            .into_any_element()
    }

    pub(super) fn tab_context_menu_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ref menu) = self.tab_menu else {
            return div().into_any_element();
        };
        let idx = menu.page_index;
        let x = menu.x;
        let y = menu.y;
        div()
            .id("tab-ctx-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.tab_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.tab_menu = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("tab-ctx-menu")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .min_w(px(148.))
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id("tab-ctx-copy")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xdbeafe)))
                            .child("复制本页")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.tab_menu = None;
                                    this.copy_page(idx, cx);
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// 末尾 `_p` + 数字起至文件名结束, 例如 `_p007.png` / `_p007_copy.png`.
fn page_tab_png_suffix(title: &str) -> &str {
    let b = title.as_bytes();
    let mut i = 0;
    let mut last = None;
    while i + 2 < b.len() {
        if b[i] == b'_' && b[i + 1] == b'p' && b[i + 2].is_ascii_digit() {
            last = Some(i);
            i += 2;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    last.map(|s| &title[s..]).unwrap_or("")
}

/// 半角 / 窄字符占 1 列, 全角 / 宽字符占 2 列.
fn ch_cols(c: char) -> usize {
    let u = c as u32;
    if u < 0x1100 {
        return 1;
    }
    let wide = matches!(
        u,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2E80..=0x303E
            | 0x3040..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD
    );
    if wide { 2 } else { 1 }
}

fn str_cols(s: &str) -> usize {
    s.chars().map(ch_cols).sum()
}

fn take_cols(s: &str, max: usize) -> &str {
    let mut cols = 0;
    for (i, c) in s.char_indices() {
        let w = ch_cols(c);
        if cols + w > max {
            return &s[..i];
        }
        cols += w;
    }
    s
}

fn format_page_tab_caption(mark: &str, badge: &str, title: &str) -> (String, String) {
    let full = format!("{mark}{badge}:{title}");
    let prefix = format!("{mark}{badge}:");
    let suffix = page_tab_png_suffix(title);
    let middle = if !suffix.is_empty() && title.ends_with(suffix) {
        &title[..title.len() - suffix.len()]
    } else {
        title
    };
    if str_cols(middle) <= TAB_LABEL_NAME_COLS {
        return (full.clone(), full);
    }
    const ELLIPSIS: &str = "……";
    let mid = take_cols(middle, TAB_LABEL_NAME_COLS);
    (format!("{prefix}{mid}{ELLIPSIS}{suffix}"), full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_pdf_name_keeps_index_and_page_suffix() {
        let title = "Bach Fantasies, Preludes and Fugues, Henle.pdf_p007.png";
        let (show, full) = format_page_tab_caption("", "7", title);
        assert_eq!(full, format!("7:{title}"));
        assert_eq!(show, "7:Bach Fantas……_p007.png");
    }

    #[test]
    fn short_name_unchanged() {
        let (show, full) = format_page_tab_caption("", "3", "page.png");
        assert_eq!(show, full);
        assert_eq!(show, "3:page.png");
    }

    #[test]
    fn cjk_counts_as_two_cols() {
        let title = "贝多芬钢琴奏鸣曲全集.pdf_p001.png";
        let (show, _) = format_page_tab_caption("", "1", title);
        assert_eq!(show, "1:贝多芬钢琴……_p001.png");
        assert_eq!(str_cols("贝多芬钢琴"), TAB_LABEL_NAME_COLS - 1);
    }

    #[test]
    fn mixed_width_does_not_split_cjk() {
        let title = "Bach贝多芬Fantasies.pdf_p002.png";
        let (show, _) = format_page_tab_caption("", "2", title);
        assert_eq!(show, "2:Bach贝多芬F……_p002.png");
        assert_eq!(str_cols("Bach贝多芬F"), TAB_LABEL_NAME_COLS);
    }
}
