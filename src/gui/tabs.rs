//! 分块页签 / 蒙版组合页签.

use super::lists::TabInfo;
use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
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
        for i in 0..n {
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
        let n = self.doc.pages.len();
        let Some(p) = self.doc.pages.get(i) else {
            return "?".into();
        };
        let has_sel = p
            .regions
            .keys()
            .any(|rid| self.doc.selected_region_ids.contains(rid));
        let mark = if has_sel { "●" } else { "" };
        if n > TAB_VIRTUAL_THRESHOLD {
            format!("{mark}{}", p.tab_badge(i + 1)).into()
        } else {
            format!("{mark}{}:{}", p.tab_badge(i + 1), p.title()).into()
        }
    }

    /// 虚拟页签槽宽: 优先用已测到的页签宽度, 短页码模式不用 76px 长标签估宽.
    pub(super) fn tab_slot_px(&self) -> f32 {
        let n = self.doc.pages.len();
        let mut max_w = 0.0f32;
        let mut counted = 0usize;
        for (&i, b) in &self.tab_bounds {
            if i >= n {
                continue;
            }
            let w = f32::from(b.size.width);
            if w > 8.0 {
                max_w = max_w.max(w);
                counted += 1;
            }
        }
        let slot = if counted > 0 {
            max_w + TAB_GAP_PX
        } else if n > TAB_VIRTUAL_THRESHOLD {
            TAB_COMPACT_SLOT_PX
        } else {
            TAB_SLOT_PX
        };
        slot.clamp(32.0, 200.0)
    }

    pub(super) fn visible_tab_range(&self) -> (usize, usize) {
        let n = self.doc.pages.len();
        if n == 0 {
            return (0, 0);
        }
        if n <= TAB_VIRTUAL_THRESHOLD {
            return (0, n);
        }
        let slot = self.tab_slot_px();
        let view_w = f32::from(self.tab_scroll.bounds().size.width);
        let view_w = if view_w < 8.0 { 960.0 } else { view_w };
        let off = (-f32::from(self.tab_scroll.offset().x)).max(0.0);
        let mut start = ((off / slot).floor() as usize).saturating_sub(8);
        let mut end = (((off + view_w) / slot).ceil() as usize)
            .saturating_add(8)
            .min(n);
        let max_off = f32::from(self.tab_scroll.max_offset().width).max(0.0);
        // 短页签实际比占位窄时, 滚到右缘仍够不到按估宽算出的末页, 这里补上.
        if max_off <= 1.0 || off + slot >= max_off {
            end = n;
            let vis = ((view_w / slot).ceil() as usize).saturating_add(16);
            start = start.min(n.saturating_sub(vis));
        }
        let start = start.min(n);
        (start, end.max(start))
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
        let handle = &self.tab_scroll;
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_h = max_x > 1.0 && track_w > 1.0;

        let mut row = div()
            .id("mask_tab_bar_row")
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

    pub(super) fn page_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.doc.pages.len();
        let (start, end) = self.visible_tab_range();
        let tabs = self.tab_infos();
        let handle = &self.tab_scroll;
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_h = max_x > 1.0 && track_w > 1.0;
        let drag_from = match &self.drag {
            Some(DragKind::TabReorder {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };
        let (line_at, line_after) = match &self.drag {
            Some(DragKind::TabReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };

        let mut row = div()
            .id("tab_bar_row")
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
            let show_line = line_at == Some(idx);
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
                    .cursor_pointer()
                    .block_mouse_except_scroll()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !line_after, |d| {
                        d.border_l_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && line_after, |d| {
                        d.border_r_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "tab"))
                    .child(tab.label.clone())
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close-{idx}")))
                            .px_1()
                            .rounded_sm()
                            .hover(|s| s.bg(rgb(0x94a3b8)))
                            .block_mouse_except_scroll()
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    // 拖拽排序松手落在叉上时不关页
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
                            this.switch_page(idx, cx);
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) = Self::item_origin(
                                this.tab_bounds.get(&idx),
                                mx,
                                my,
                            );
                            this.tab_add_press = false;
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
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
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
                if let Some(DragKind::TabReorder {
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
                        this.resolve_tab_drop(from, x, y)
                    } else {
                        (from, None, false)
                    };
                    this.drag = Some(DragKind::TabReorder {
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
