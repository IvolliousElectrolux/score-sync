//! 窗口外拖拽转发、分隔条.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn apply_side_resize(&mut self, mouse_x: f32, cx: &mut Context<Self>) {
        let Some(DragKind::SideResize { start_x, start_w }) = self.drag else {
            return;
        };
        // 分隔条在侧栏左侧: 向左拖 → 侧栏变宽
        let new_w = (start_w + (start_x - mouse_x)).clamp(SIDE_PANEL_MIN, SIDE_PANEL_MAX);
        if (new_w - self.side_width).abs() > 0.5 {
            self.side_width = new_w;
            self.mask_tool.update(cx, |m, _| {
                m.set_embed_side_width(new_w);
            });
            cx.notify();
        }
    }

    /// 鼠标离开窗口后需由 window.on_mouse_event 转发到此.
    pub(super) fn handle_outside_window_mouse_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            if matches!(self.drag, Some(DragKind::Scrollbar { .. })) {
                self.apply_scrollbar_drag(x, y, cx);
            }
            return;
        }
        match self.side_tool {
            SideTool::Mask => {
                self.mask_tool
                    .update(cx, |m, cx| m.root_mouse_move(x, y, cx));
            }
            SideTool::Video => {
                self.score_video
                    .update(cx, |v, cx| v.root_mouse_move(x, y, cx));
            }
            _ => {}
        }
        self.apply_host_drag_at(x, y, cx);
    }

    pub(super) fn handle_outside_window_mouse_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            if matches!(self.drag, Some(DragKind::Scrollbar { .. })) {
                self.drag = None;
                cx.notify();
            }
            return;
        }
        match self.side_tool {
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.root_mouse_up(x, y, cx));
            }
            SideTool::Video => {
                self.score_video
                    .update(cx, |v, cx| v.root_mouse_up(x, y, cx));
            }
            _ => {}
        }
        self.finish_host_drag_at(x, y, cx);
    }

    /// 滚动条/分隔条拖拽时, 鼠标滑到列表或标签上仍继续, 不让那边的 take() 吃掉 drag.
    pub(super) fn forward_capture_drags(&mut self, x: f32, y: f32, cx: &mut Context<Self>) -> bool {
        match self.drag {
            Some(DragKind::Scrollbar { .. })
            | Some(DragKind::TabHScroll { .. })
            | Some(DragKind::SideResize { .. }) => {
                self.apply_host_drag_at(x, y, cx);
                true
            }
            _ => false,
        }
    }

    pub(super) fn apply_host_drag_at(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(DragKind::Scrollbar { .. }) => {
                self.apply_scrollbar_drag(x, y, cx);
            }
            Some(DragKind::SideResize { .. }) => {
                self.apply_side_resize(x, cx);
            }
            Some(DragKind::TabHScroll { grab }) => {
                let handle = self.tab_hscroll_handle();
                let b = handle.bounds();
                let max = f32::from(handle.max_offset().width);
                if max > 0.5 {
                    let tw = f32::from(b.size.width).max(1.0);
                    let thumb = ((tw * tw) / (tw + max)).clamp(24.0, tw);
                    let travel = (tw - thumb).max(1.0);
                    let track_left = f32::from(b.origin.x);
                    let thumb_left = (x - grab - track_left).clamp(0.0, travel);
                    handle.set_offset(point(px(-(thumb_left / travel) * max), px(0.)));
                    cx.notify();
                }
            }
            Some(DragKind::TabReorder {
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                mut armed,
                ..
            }) => {
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
            Some(DragKind::MemberReorder {
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                mut armed,
                ..
            }) => {
                if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                    armed = true;
                }
                let (to, line_at, line_after) = if armed {
                    self.resolve_member_drop(from, x, y)
                } else {
                    (from, None, false)
                };
                self.drag = Some(DragKind::MemberReorder {
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
            Some(DragKind::GroupReorder {
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                mut armed,
                ctrl,
                ..
            }) => {
                if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                    armed = true;
                }
                let (_to, line_at, line_after) = if armed {
                    self.resolve_group_drop(from, x, y)
                } else {
                    (from, None, false)
                };
                self.drag = Some(DragKind::GroupReorder {
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
            _ => {}
        }
    }

    pub(super) fn finish_host_drag_at(&mut self, _x: f32, _y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(DragKind::TabReorder { .. })
            | Some(DragKind::MemberReorder { .. })
            | Some(DragKind::GroupReorder { .. })
            | Some(DragKind::Scrollbar { .. })
            | Some(DragKind::SideResize { .. })
            | Some(DragKind::TabHScroll { .. }) => {}
            _ => return,
        }
        match self.drag.take() {
            Some(DragKind::TabReorder {
                from, to, armed, ..
            }) => {
                if armed && from != to {
                    self.push_crop_undo_all_pages();
                    self.doc.move_page(from, to);
                    self.after_doc_change(cx);
                } else {
                    cx.notify();
                }
            }
            Some(DragKind::MemberReorder {
                from, to, armed, ..
            }) => {
                if armed && from != to {
                    let Some(g) = self.doc.active_group() else {
                        cx.notify();
                        return;
                    };
                    let mut ids = g.region_ids.clone();
                    if from < ids.len() && to < ids.len() {
                        self.push_crop_undo_all_pages();
                        let item = ids.remove(from);
                        ids.insert(to, item);
                        self.doc.reorder_active_members(ids);
                        self.after_doc_change(cx);
                    } else {
                        cx.notify();
                    }
                } else {
                    cx.notify();
                }
            }
            Some(DragKind::GroupReorder {
                from,
                armed,
                ctrl,
                line_at,
                line_after,
                ..
            }) => {
                if armed {
                    if let Some(anchor) = line_at {
                        self.push_crop_undo_all_pages();
                        self.doc.reorder_groups_block(from, anchor, line_after);
                        self.after_doc_change(cx);
                    } else {
                        cx.notify();
                    }
                } else if let Some(gid) = self.doc.groups.get(from).map(|g| g.id.clone()) {
                    self.doc.click_group(&gid, ctrl);
                    self.refresh_render(cx);
                } else {
                    cx.notify();
                }
            }
            Some(
                DragKind::Scrollbar { .. }
                | DragKind::SideResize { .. }
                | DragKind::TabHScroll { .. },
            ) => {
                cx.notify();
            }
            _ => {}
        }
    }

    pub(super) fn outside_window_drag_capture(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _cx| {
                let entity_m = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || window.is_window_hovered() {
                        return;
                    }
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    entity_m.update(cx, |this, cx| {
                        this.handle_outside_window_mouse_move(x, y, cx);
                    });
                });
                let entity_u = entity.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || window.is_window_hovered() {
                        return;
                    }
                    if ev.button != MouseButton::Left {
                        return;
                    }
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    entity_u.update(cx, |this, cx| {
                        this.handle_outside_window_mouse_up(x, y, cx);
                    });
                });
            },
        )
        .absolute()
        .size(px(0.))
    }
}
