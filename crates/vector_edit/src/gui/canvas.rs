use gpui::{
    canvas, point, prelude::*, px, quad, rgb, size, Bounds, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, ScrollDelta, ScrollWheelEvent, Window,
};

use super::tools;
use super::*;
use crate::geom::Rect;

impl VectorEditApp {
    pub fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let base = self.base_tex.as_ref().map(|t| {
            (t.tex.clone(), t.doc_x, t.doc_y, t.doc_w, t.doc_h)
        });
        let node_drag = matches!(self.drag, Some(DragKind::Node { .. }));
        let scale_live = match &self.drag {
            Some(DragKind::Scale { handle, bounds, sx, sy }) => {
                let (ox, oy) = scale_anchor(*handle, *bounds);
                Some((ox, oy, *sx, *sy))
            }
            _ => None,
        };
        let sel = if node_drag {
            None
        } else {
            self.sel_sprite.as_ref().map(|s| {
                let (x, y, w, h) = if let Some((ox, oy, sx, sy)) = scale_live {
                    (
                        ox + (s.doc_x - ox) * sx,
                        oy + (s.doc_y - oy) * sy,
                        s.doc_w * sx,
                        s.doc_h * sy,
                    )
                } else {
                    (s.doc_x + s.dx, s.doc_y + s.dy, s.doc_w, s.doc_h)
                };
                (s.tex.clone(), x, y, w, h)
            })
        };
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w, d.canvas_h))
            .unwrap_or((1.0, 1.0));
        let zoom = self.zoom;
        let pan = self.pan;
        let user_zoomed = self.user_zoomed;
        let doc_bounds = tools::selection_rect(self);
        let group_frame = tools::drilled_frame(self);
        let pen = self.pen.clone();
        let pen_cursor = if self.pen.is_some() && !matches!(self.drag, Some(DragKind::PenPull { .. })) {
            self.hover_doc.map(|(x, y)| self.pen_point(x, y))
        } else {
            None
        };
        let lasso = self.lasso.clone();
        let mode = self.mode;
        let drag_box = match &self.drag {
            Some(DragKind::Marquee { x0, y0, x1, y1, .. }) => Some((*x0, *y0, *x1, *y1)),
            _ => None,
        };
        let pencil = match &self.drag {
            Some(DragKind::Pencil { pts }) | Some(DragKind::Lasso { pts }) => Some(pts.clone()),
            _ => None,
        };
        let ghost = match &self.drag {
            Some(DragKind::Canvas {
                new_w,
                new_h,
                shift_x,
                shift_y,
                ..
            }) => Some((*new_w, *new_h, *shift_x, *shift_y)),
            _ => None,
        };
        let sel_off = self
            .sel_sprite
            .as_ref()
            .map(|s| (s.dx, s.dy))
            .unwrap_or((0.0, 0.0));
        let hover = self.hover_doc;
        let hover_edge = self.hover_edge;
        let content_guide = if mode == ToolMode::Canvas {
            self.doc.as_ref().and_then(|d| d.content_bounds())
        } else {
            None
        };
        let snap_w = self.fit_hint.snap_w;
        let snap_h = self.fit_hint.snap_h;
        let backdrop = rgb(self.backdrop);
        let brush = self.brush.max(1.6);
        let show_brush = mode == ToolMode::Pencil;
        let entity = cx.entity().clone();
        let paint_entity = entity.clone();
        div()
            .id("vector_canvas")
            .relative()
            .flex_1()
            .size_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_hidden()
            .bg(backdrop)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, cx| {
                            let dw = (f32::from(this.view_bounds.size.width)
                                - f32::from(bounds.size.width))
                            .abs();
                            let dh = (f32::from(this.view_bounds.size.height)
                                - f32::from(bounds.size.height))
                            .abs();
                            this.view_bounds = bounds;
                            if dw > 0.5 || dh > 0.5 {
                                cx.notify();
                            }
                        });
                    },
                    move |bounds, _, window, app| {
                        paint_checker(window, bounds, backdrop);
                        let xform = Xform::compute(cw, ch, bounds, zoom, pan, user_zoomed);
                        let device_scale = window.scale_factor();
                        if let Some((tex, x, y, w, h)) = &base {
                            let b = snap_device_bounds(
                                xform.doc_bounds(bounds, *x, *y, *w, *h),
                                device_scale,
                            );
                            let _ = window.paint_image(
                                b,
                                gpui::Corners::default(),
                                tex.clone(),
                                0,
                                false,
                            );
                        }
                        if let Some((nw, nh, sx, sy)) = ghost {
                            paint_rect_outline(
                                window,
                                bounds,
                                &xform,
                                Rect::new(sx, sy, sx + nw, sy + nh),
                                rgb(0x2563eb),
                            );
                        }
                        if let Some((tex, x, y, w, h)) = &sel {
                            let b = snap_device_bounds(
                                xform.doc_bounds(bounds, *x, *y, *w, *h),
                                device_scale,
                            );
                            let _ = window.paint_image(
                                b,
                                gpui::Corners::default(),
                                tex.clone(),
                                0,
                                false,
                            );
                        }
                        if mode == ToolMode::Canvas && ghost.is_none() {
                            paint_rect_outline(
                                window,
                                bounds,
                                &xform,
                                Rect::new(0.0, 0.0, cw, ch),
                                rgb(0x64748b),
                            );
                            if let Some(edge) = hover_edge {
                                paint_edge(window, bounds, &xform, edge, cw, ch);
                            }
                        }
                        if mode == ToolMode::Canvas {
                            let fx = target_frame_edges(cw, snap_w);
                            let fy = target_frame_edges(ch, snap_h);
                            if fx.is_some() || fy.is_some() {
                                let (x0, x1) = fx.unwrap_or((0.0, cw));
                                let (y0, y1) = fy.unwrap_or((0.0, ch));
                                paint_rect_outline(
                                    window,
                                    bounds,
                                    &xform,
                                    Rect::new(x0, y0, x1, y1),
                                    rgb(0x0891b2),
                                );
                            }
                            if let Some(b) = content_guide {
                                paint_rect_outline(window, bounds, &xform, b, rgb(0xf97316));
                            }
                        }
                        if let Some(mut frame) = group_frame {
                            if let Some((ox, oy, sx, sy)) = scale_live {
                                frame.x0 = ox + (frame.x0 - ox) * sx;
                                frame.x1 = ox + (frame.x1 - ox) * sx;
                                frame.y0 = oy + (frame.y0 - oy) * sy;
                                frame.y1 = oy + (frame.y1 - oy) * sy;
                            }
                            paint_rect_outline(window, bounds, &xform, frame, rgb(0x2563eb));
                            for (hx, hy) in tools::handle_points(frame) {
                                paint_handle(window, bounds, &xform, hx, hy, rgb(0x2563eb));
                            }
                            let rot = (frame.cx(), frame.y0 - 14.0 / xform.scale.max(0.2));
                            paint_handle(window, bounds, &xform, rot.0, rot.1, rgb(0x16a34a));
                        }
                        if let Some(mut b) = doc_bounds {
                            if group_frame.is_none() {
                                if let Some((ox, oy, sx, sy)) = scale_live {
                                    b.x0 = ox + (b.x0 - ox) * sx;
                                    b.x1 = ox + (b.x1 - ox) * sx;
                                    b.y0 = oy + (b.y0 - oy) * sy;
                                    b.y1 = oy + (b.y1 - oy) * sy;
                                } else {
                                    b.x0 += sel_off.0;
                                    b.x1 += sel_off.0;
                                    b.y0 += sel_off.1;
                                    b.y1 += sel_off.1;
                                }
                                paint_rect_outline(window, bounds, &xform, b, rgb(0x2563eb));
                                for (hx, hy) in tools::handle_points(b) {
                                    paint_handle(window, bounds, &xform, hx, hy, rgb(0x2563eb));
                                }
                                let rot = (b.cx(), b.y0 - 14.0 / xform.scale.max(0.2));
                                paint_handle(window, bounds, &xform, rot.0, rot.1, rgb(0x16a34a));
                            }
                            paint_entity.update(app, |this, _| {
                                paint_selection_edges(this, window, bounds, &xform, sel_off, scale_live);
                            });
                        }
                        if mode == ToolMode::Node {
                            paint_entity.update(app, |this, _| {
                                let id = this.node_target.as_deref().or_else(|| {
                                    this.selected.first().map(String::as_str)
                                });
                                let Some(id) = id else {
                                    return;
                                };
                                let Some(obj) = this.doc.as_ref().and_then(|d| d.object(id)) else {
                                    return;
                                };
                                if node_drag {
                                    paint_obj_preview(window, bounds, &xform, obj);
                                }
                                paint_nodes(window, bounds, &xform, obj);
                            });
                        }
                        if let Some((x0, y0, x1, y1)) = drag_box {
                            paint_rect_outline(
                                window,
                                bounds,
                                &xform,
                                Rect::new(x0, y0, x1, y1),
                                rgb(0x0ea5e9),
                            );
                        }
                        if let Some(pen) = &pen {
                            paint_pen(window, bounds, &xform, pen, pen_cursor);
                        }
                        if let Some(pts) = &pencil {
                            paint_poly(window, bounds, &xform, pts, false);
                        }
                        if !lasso.is_empty() {
                            paint_poly(window, bounds, &xform, &lasso, true);
                        }
                        if show_brush {
                            if let Some((hx, hy)) = hover {
                                paint_brush(window, bounds, &xform, hx, hy, brush);
                            }
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_middle))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_canvas_menu(f32::from(ev.position.x), f32::from(ev.position.y), cx);
                }),
            )
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .cursor(self.canvas_cursor())
            .on_modifiers_changed(cx.listener(|this, ev: &gpui::ModifiersChangedEvent, _, cx| {
                this.note_pointer_modifiers(
                    ev.modifiers.shift,
                    ev.modifiers.alt,
                    apply_bg::is_primary_mod(&ev.modifiers),
                    cx,
                );
            }))
            .child(self.canvas_context_menu(cx))
    }

    fn open_canvas_menu(&mut self, wx: f32, wy: f32, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        self.note_canvas_target();
        let (dx, dy) = self.doc_pt(wx, wy);
        let before = self.selected.clone();
        if self.mode == ToolMode::Select {
            let tol = self.hit_tol();
            if let Some(leaf) = self.doc.as_ref().and_then(|d| d.hit_test(dx, dy, tol)) {
                let keep = self
                    .doc
                    .as_ref()
                    .is_some_and(|d| d.selection_anchor(&self.selected, &leaf).is_some());
                if !keep {
                    let id = self
                        .doc
                        .as_ref()
                        .map(|d| d.pick_target(&leaf))
                        .unwrap_or(leaf);
                    self.selected = vec![id.clone()];
                    self.outline_anchor = Some(id);
                }
            }
        }
        if self.selected != before {
            self.reveal_outline_selection();
        }
        let origin = self.view_bounds.origin;
        let size = self.view_bounds.size;
        let pw = f32::from(size.width);
        let ph = f32::from(size.height);
        let mut x = wx - f32::from(origin.x);
        let mut y = wy - f32::from(origin.y);
        if pw > 8.0 {
            x = x.clamp(0.0, (pw - 208.0).max(0.0));
        }
        if ph > 8.0 {
            y = y.clamp(0.0, (ph - 280.0).max(0.0));
        }
        self.outline_menu = Some(OutlineMenu {
            x,
            y,
            pin_right: false,
            on_canvas: true,
            target: self.selected.first().cloned().unwrap_or_default(),
        });
        cx.notify();
    }

    pub(crate) fn note_pointer_modifiers(&mut self, shift: bool, alt: bool, ctrl: bool, cx: &mut Context<Self>) {
        if self.shift_down == shift && self.alt_down == alt && self.ctrl_down == ctrl {
            return;
        }
        self.shift_down = shift;
        self.alt_down = alt;
        self.ctrl_down = ctrl;
        self.refresh_constrained_drag(cx);
    }

    /// Shift 在拖动中途按下时, 不等指针再动, 马上按锁定规则重算.
    fn refresh_constrained_drag(&mut self, cx: &mut Context<Self>) {
        let Some((x, y)) = self.hover_doc else {
            self.notify_nav(cx);
            return;
        };
        match self.drag.clone() {
            Some(DragKind::Scale { handle, bounds, .. }) => {
                let (sx, sy) = scale_factors(handle, bounds, x, y, self.shift_down);
                self.drag = Some(DragKind::Scale { handle, bounds, sx, sy });
            }
            Some(DragKind::Move { origin, .. }) => {
                let (dx, dy) = axis_lock(self.shift_down, x - origin.0, y - origin.1);
                if let Some(sprite) = self.sel_sprite.as_mut() {
                    sprite.dx = dx;
                    sprite.dy = dy;
                }
            }
            _ => {}
        }
        self.notify_nav(cx);
    }

    pub(crate) fn canvas_pointer(&self, window: &Window) -> Option<(f32, f32)> {
        let p = window.mouse_position();
        let b = self.view_bounds;
        let right = b.origin.x + b.size.width;
        let bottom = b.origin.y + b.size.height;
        if b.size.width < px(1.) || b.size.height < px(1.) {
            return None;
        }
        if p.x < b.origin.x || p.y < b.origin.y || p.x >= right || p.y >= bottom {
            return None;
        }
        let x = f32::from(p.x - b.origin.x);
        let y = f32::from(p.y - b.origin.y);
        Some(self.xform().screen_to_doc(x, y))
    }

    fn xform(&self) -> Xform {
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w, d.canvas_h))
            .unwrap_or((1.0, 1.0));
        Xform::compute(cw, ch, self.view_bounds, self.zoom, self.pan, self.user_zoomed)
    }

    pub(crate) fn hit_tol(&self) -> f32 {
        7.0 / self.xform().scale.max(0.12)
    }

    pub(crate) fn raster_plan(&self) -> RasterPlan {
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w, d.canvas_h))
            .unwrap_or((1.0, 1.0));
        let x = self.xform();
        plan_raster(
            cw,
            ch,
            f32::from(self.view_bounds.size.width),
            f32::from(self.view_bounds.size.height),
            x.scale,
            x.ox,
            x.oy,
            self.device_scale,
        )
    }

    fn visible_doc_rect(&self) -> (f32, f32, f32, f32) {
        let x = self.xform();
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        let (x0, y0) = x.screen_to_doc(0.0, 0.0);
        let (x1, y1) = x.screen_to_doc(vw, vh);
        (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
    }

    /// 现有贴图已经够锐, 并且盖住当前视口 (或整页) 时不必重画.
    pub(crate) fn raster_current(&self, need_px: f32) -> bool {
        let Some(tex) = &self.base_tex else {
            return false;
        };
        if self.raster_px < need_px * 0.90 {
            return false;
        }
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w, d.canvas_h))
            .unwrap_or((1.0, 1.0));
        if tex.doc_x <= 0.5
            && tex.doc_y <= 0.5
            && tex.doc_x + tex.doc_w >= cw - 0.5
            && tex.doc_y + tex.doc_h >= ch - 0.5
        {
            return true;
        }
        let (x0, y0, x1, y1) = self.visible_doc_rect();
        let x0 = x0.max(0.0);
        let y0 = y0.max(0.0);
        let x1 = x1.min(cw);
        let y1 = y1.min(ch);
        if x1 - x0 < 0.5 || y1 - y0 < 0.5 {
            return true;
        }
        // 贴图还要比视口多出一截, 平移一小段时边缘不会先露出空白.
        let mx = (x1 - x0) * 0.08;
        let my = (y1 - y0) * 0.08;
        let eps = 0.5;
        x0 - mx >= tex.doc_x - eps
            && y0 - my >= tex.doc_y - eps
            && x1 + mx <= tex.doc_x + tex.doc_w + eps
            && y1 + my <= tex.doc_y + tex.doc_h + eps
    }

    fn lock_user_view(&mut self) {
        if !self.user_zoomed {
            let x = self.xform();
            self.zoom = x.scale;
            self.pan = point(x.ox, x.oy);
            self.user_zoomed = true;
        }
    }

    fn canvas_cursor(&self) -> gpui::CursorStyle {
        if self.space_down || matches!(self.drag, Some(DragKind::Pan { .. })) {
            return if self.drag.is_some() {
                gpui::CursorStyle::ClosedHand
            } else {
                gpui::CursorStyle::OpenHand
            };
        }
        if self.mode == ToolMode::Canvas {
            return match self.hover_edge {
                Some(CanvasEdge::Left) | Some(CanvasEdge::Right) => gpui::CursorStyle::ResizeLeftRight,
                Some(CanvasEdge::Top) | Some(CanvasEdge::Bottom) => gpui::CursorStyle::ResizeUpDown,
                None => gpui::CursorStyle::Arrow,
            };
        }
        match self.mode {
            ToolMode::Hand => gpui::CursorStyle::OpenHand,
            ToolMode::Node => {
                let dragging = match &self.drag {
                    Some(DragKind::Node { role, .. }) => Some(*role),
                    _ => None,
                };
                node_pointer_cursor(dragging, self.hover_node, self.hover_hit)
            }
            ToolMode::Pen => {
                if self.near_pen_start() {
                    gpui::CursorStyle::PointingHand
                } else {
                    gpui::CursorStyle::Crosshair
                }
            }
            ToolMode::Pencil | ToolMode::Eraser | ToolMode::Lasso => {
                gpui::CursorStyle::Crosshair
            }
            _ => gpui::CursorStyle::Arrow,
        }
    }

    fn doc_pt(&self, ev_x: f32, ev_y: f32) -> (f32, f32) {
        let x = ev_x - f32::from(self.view_bounds.origin.x);
        let y = ev_y - f32::from(self.view_bounds.origin.y);
        self.xform().screen_to_doc(x, y)
    }

    fn on_middle(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
        self.shift_down = ev.modifiers.shift;
        self.alt_down = ev.modifiers.alt;
        self.ctrl_down = apply_bg::is_primary_mod(&ev.modifiers);
        self.outline_menu = None;
        self.lock_user_view();
        self.drag = Some(DragKind::Pan {
            origin: point(f32::from(ev.position.x), f32::from(ev.position.y)),
            pan: self.pan,
        });
        self.notify_nav(cx);
    }

    fn on_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
        self.note_canvas_target();
        self.outline_menu = None;
        self.shift_down = ev.modifiers.shift;
        self.alt_down = ev.modifiers.alt;
        self.ctrl_down = apply_bg::is_primary_mod(&ev.modifiers);
        let (x, y) = self.doc_pt(f32::from(ev.position.x), f32::from(ev.position.y));
        if self.space_down || self.mode == ToolMode::Hand {
            self.lock_user_view();
            self.drag = Some(DragKind::Pan {
                origin: point(f32::from(ev.position.x), f32::from(ev.position.y)),
                pan: self.pan,
            });
            self.notify_nav(cx);
            return;
        }
        if self.alt_down && matches!(self.mode, ToolMode::Pen | ToolMode::Pencil) && tools::sample_style(self, x, y)
        {
            self.palette_a = self.active_alpha();
            if self.palette_open {
                self.retarget_palette(cx);
            } else {
                self.notify_chrome(cx);
            }
            return;
        }
        let before = self.selected.clone();
        match self.mode {
            ToolMode::Hand => {}
            ToolMode::Select => self.down_select(x, y, cx),
            ToolMode::Node => self.down_node(x, y, ev.click_count, cx),
            ToolMode::Canvas => self.down_canvas(
                x,
                y,
                (f32::from(ev.position.x), f32::from(ev.position.y)),
            ),
            ToolMode::Pen => self.down_pen(x, y, cx),
            ToolMode::Pencil => {
                self.drag = Some(DragKind::Pencil { pts: vec![(x, y)] });
            }
            ToolMode::Eraser => tools::erase_at(self, x, y, cx),
            ToolMode::Lasso => {
                self.lasso.push((x, y));
                self.drag = Some(DragKind::Lasso { pts: self.lasso.clone() });
            }
        }
        if self.selected != before {
            self.reveal_outline_selection();
        }
        cx.notify();
    }

    /// 选区正好是这个叶子所在的最外层组合时, 松手可以改选叶子.
    fn drill_leaf(&self, leaf: Option<&str>) -> Option<String> {
        let leaf = leaf?;
        let doc = self.doc.as_ref()?;
        let top = doc.pick_target(leaf);
        if top == leaf {
            return None;
        }
        if self.selected.len() == 1 && self.selected[0] == top {
            Some(leaf.to_string())
        } else {
            None
        }
    }

    fn down_select(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let tol = self.hit_tol();
        self.pending_drill = None;
        let box_bounds = tools::drilled_frame(self).or_else(|| tools::selection_rect(self));
        if let Some(b) = box_bounds {
            for (i, (hx, hy)) in tools::handle_points(b).into_iter().enumerate() {
                if (hx - x).hypot(hy - y) <= tol {
                    self.push_undo();
                    self.preview_selection(true);
                    self.drag = Some(DragKind::Scale {
                        handle: i,
                        bounds: b,
                        sx: 1.0,
                        sy: 1.0,
                    });
                    return;
                }
            }
            let rot = (b.cx(), b.y0 - 14.0 / self.xform().scale.max(0.2));
            if (rot.0 - x).hypot(rot.1 - y) <= tol * 1.4 {
                let ang = (y - b.cy()).atan2(x - b.cx());
                self.push_undo();
                self.preview_selection(true);
                self.drag = Some(DragKind::Rotate {
                    center: (b.cx(), b.cy()),
                    start_ang: ang,
                    acc: 0.0,
                });
                return;
            }
        }
        let hit = self.doc.as_ref().and_then(|d| d.hit_test(x, y, tol));
        let on_selected = hit.as_deref().is_some_and(|leaf| {
            self.doc
                .as_ref()
                .is_some_and(|d| d.selection_anchor(&self.selected, leaf).is_some())
        });
        if on_selected && !self.shift_down && !self.ctrl_down {
            self.pending_drill = self.drill_leaf(hit.as_deref());
            if let Some(id) = hit.clone() {
                self.adopt_object_style(&id, cx);
            }
            self.push_undo();
            self.preview_selection(!self.alt_down);
            self.drag = Some(DragKind::Move {
                origin: (x, y),
                copy: self.alt_down,
            });
            return;
        }
        let (add, sub, toggle) = self.sel_mods();
        if hit.is_none() || add || sub || toggle {
            self.drag = Some(DragKind::Marquee {
                x0: x,
                y0: y,
                x1: x,
                y1: y,
                add,
                sub,
                toggle,
            });
            return;
        }
        tools::click_select(self, x, y);
        if let Some(id) = hit {
            self.adopt_object_style(&id, cx);
        }
        self.push_undo();
        self.preview_selection(true);
        self.drag = Some(DragKind::Move {
            origin: (x, y),
            copy: false,
        });
    }

    fn down_node(&mut self, x: f32, y: f32, clicks: usize, cx: &mut Context<Self>) {
        let tol = self.hit_tol();
        let current = self.node_target.clone().or_else(|| self.selected.first().cloned());
        if let Some(id) = current.clone() {
            if let Some(obj) = self.doc.as_ref().and_then(|d| d.object(&id)).cloned() {
                if let Some((sub, seg, role)) = tools::node_at(&obj, x, y, tol) {
                    self.node_target = Some(id.clone());
                    self.selected = vec![id.clone()];
                    self.adopt_object_style(&id, cx);
                    if matches!(role, super::types::NodeRole::Anchor | super::types::NodeRole::Head) {
                        self.node_hit = Some((sub, seg, role));
                    }
                    if clicks >= 2 && matches!(role, super::types::NodeRole::Anchor | super::types::NodeRole::Head)
                    {
                        self.push_undo();
                        tools::toggle_anchor_smooth(self, &id, sub, seg, role);
                        self.dirty = true;
                        self.rebuild_raster(cx);
                        return;
                    }
                    self.push_undo();
                    self.hide_selection_from_base();
                    self.drag = Some(DragKind::Node {
                        id,
                        sub,
                        seg,
                        role,
                        last: (x, y),
                    });
                    return;
                }
            }
        }
        let Some(id) = self.doc.as_ref().and_then(|d| d.hit_test(x, y, tol)) else {
            self.node_target = None;
            self.node_hit = None;
            self.selected.clear();
            return;
        };
        self.node_target = Some(id.clone());
        self.selected = vec![id.clone()];
        self.adopt_object_style(&id, cx);
        self.node_hit = None;
        if let Some(obj) = self.doc.as_ref().and_then(|d| d.object(&id)).cloned() {
            if let Some((sub, seg, role)) = tools::node_at(&obj, x, y, tol) {
                if matches!(role, super::types::NodeRole::Anchor | super::types::NodeRole::Head) {
                    self.node_hit = Some((sub, seg, role));
                }
                self.push_undo();
                self.hide_selection_from_base();
                self.drag = Some(DragKind::Node {
                    id,
                    sub,
                    seg,
                    role,
                    last: (x, y),
                });
            }
        }
    }

    fn down_canvas(&mut self, x: f32, y: f32, screen: (f32, f32)) {
        let tol = self.hit_tol();
        let (orig_w, orig_h, edge) = {
            let Some(doc) = self.doc.as_ref() else {
                return;
            };
            let orig_w = doc.canvas_w;
            let orig_h = doc.canvas_h;
            let edge = if x.abs() <= tol {
                Some(CanvasEdge::Left)
            } else if (x - orig_w).abs() <= tol {
                Some(CanvasEdge::Right)
            } else if y.abs() <= tol {
                Some(CanvasEdge::Top)
            } else if (y - orig_h).abs() <= tol {
                Some(CanvasEdge::Bottom)
            } else {
                None
            };
            let Some(edge) = edge else {
                return;
            };
            (orig_w, orig_h, edge)
        };
        self.lock_user_view();
        let scale = self.xform().scale.max(0.05);
        self.drag = Some(DragKind::Canvas {
            edge,
            orig_w,
            orig_h,
            origin: screen,
            scale,
            new_w: orig_w,
            new_h: orig_h,
            shift_x: 0.0,
            shift_y: 0.0,
        });
    }

    fn down_pen(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let tol = self.pen_snap_tol();
        let (x, y) = self.pen_point(x, y);
        if let Some(pen) = self.pen.as_ref() {
            if let Some(first) = pen.nodes.first() {
                if (first.p.0 - x).hypot(first.p.1 - y) <= tol && pen.nodes.len() >= 2 {
                    let pen = self.pen.take().unwrap();
                    tools::commit_pen(self, &pen, true, cx);
                    return;
                }
            }
            if let Some(last) = pen.nodes.last() {
                if (last.p.0 - x).hypot(last.p.1 - y) <= tol && pen.nodes.len() >= 2 {
                    let pen = self.pen.take().unwrap();
                    tools::commit_pen(self, &pen, false, cx);
                    return;
                }
            }
        }
        let node = PenNode {
            p: (x, y),
            hin: None,
            hout: None,
        };
        let index = if let Some(pen) = self.pen.as_mut() {
            pen.nodes.push(node);
            pen.nodes.len() - 1
        } else {
            self.pen = Some(PenDraft {
                nodes: vec![node],
                continue_id: None,
            });
            0
        };
        self.drag = Some(DragKind::PenPull { index });
    }

    fn pen_snap_tol(&self) -> f32 {
        12.0 / self.xform().scale.max(0.12)
    }

    /// 靠近起点时吸到该点, 优先于 Shift 的角度约束.
    fn pen_point(&self, x: f32, y: f32) -> (f32, f32) {
        if let Some(p) = self.pen_start_at(x, y) {
            return p;
        }
        self.constrain_pen(x, y)
    }

    fn near_pen_start(&self) -> bool {
        if matches!(self.drag, Some(DragKind::PenPull { .. })) {
            return false;
        }
        let Some((x, y)) = self.hover_doc else {
            return false;
        };
        self.pen_start_at(x, y).is_some()
    }

    fn pen_start_at(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let pen = self.pen.as_ref()?;
        if pen.nodes.len() < 2 {
            return None;
        }
        let first = pen.nodes.first()?;
        if (first.p.0 - x).hypot(first.p.1 - y) <= self.pen_snap_tol() {
            Some(first.p)
        } else {
            None
        }
    }

    fn constrain_pen(&self, x: f32, y: f32) -> (f32, f32) {
        if !self.shift_down {
            return (x, y);
        }
        let Some(prev) = self.pen.as_ref().and_then(|p| p.nodes.last().map(|n| n.p)) else {
            return (x, y);
        };
        let dx = x - prev.0;
        let dy = y - prev.1;
        let len = dx.hypot(dy);
        if len < 0.5 {
            return (x, y);
        }
        let step = std::f32::consts::FRAC_PI_4;
        let ang = (dy.atan2(dx) / step).round() * step;
        (prev.0 + ang.cos() * len, prev.1 + ang.sin() * len)
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.shift_down = ev.modifiers.shift;
        self.alt_down = ev.modifiers.alt;
        self.ctrl_down = apply_bg::is_primary_mod(&ev.modifiers);
        if let Some(DragKind::Slider(kind)) = self.drag {
            self.set_slider_from_x(f32::from(ev.position.x), kind, cx);
            return;
        }
        if matches!(
            self.drag,
            Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha)
        ) {
            self.palette_pointer_move(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            return;
        }
        if matches!(self.drag, Some(DragKind::LayerReorder { .. })) {
            self.update_layer_reorder(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            return;
        }
        let (x, y) = self.doc_pt(f32::from(ev.position.x), f32::from(ev.position.y));
        self.hover_doc = Some((x, y));
        if self.mode == ToolMode::Canvas {
            if let Some(doc) = self.doc.as_ref() {
                let tol = self.hit_tol();
                self.hover_edge = if x.abs() <= tol {
                    Some(CanvasEdge::Left)
                } else if (x - doc.canvas_w).abs() <= tol {
                    Some(CanvasEdge::Right)
                } else if y.abs() <= tol {
                    Some(CanvasEdge::Top)
                } else if (y - doc.canvas_h).abs() <= tol {
                    Some(CanvasEdge::Bottom)
                } else {
                    None
                };
            }
        } else {
            self.hover_edge = None;
        }
        let Some(drag) = self.drag.clone() else {
            if self.mode == ToolMode::Node {
                if self.refresh_node_hover(x, y) {
                    self.notify_nav(cx);
                }
            } else {
                self.hover_node = None;
                self.hover_hit = false;
                self.notify_nav(cx);
            }
            return;
        };
        match drag {
            DragKind::Pan { origin, pan } => {
                self.user_zoomed = true;
                self.pan = point(
                    pan.x + f32::from(ev.position.x) - origin.x,
                    pan.y + f32::from(ev.position.y) - origin.y,
                );
            }
            DragKind::Marquee { x0, y0, add, sub, toggle, .. } => {
                self.drag = Some(DragKind::Marquee {
                    x0,
                    y0,
                    x1: x,
                    y1: y,
                    add,
                    sub,
                    toggle,
                });
            }
            DragKind::Move { origin, copy } => {
                let (dx, dy) = axis_lock(self.shift_down, x - origin.0, y - origin.1);
                if let Some(sprite) = self.sel_sprite.as_mut() {
                    sprite.dx = dx;
                    sprite.dy = dy;
                }
                self.drag = Some(DragKind::Move { origin, copy });
            }
            DragKind::Scale { handle, bounds, .. } => {
                let (sx, sy) = scale_factors(handle, bounds, x, y, self.shift_down);
                self.drag = Some(DragKind::Scale { handle, bounds, sx, sy });
            }
            DragKind::Rotate { center, start_ang, acc: _ } => {
                self.preview_from_undo();
                let ang = (y - center.1).atan2(x - center.0);
                let mut delta = (ang - start_ang).to_degrees();
                if self.shift_down {
                    delta = (delta / 15.0).round() * 15.0;
                }
                if let Some(doc) = self.doc.as_mut() {
                    doc.rotate_ids(&self.selected, center.0, center.1, delta);
                }
                let ids = self.selected.clone();
                self.rebuild_sprite(&ids);
                self.drag = Some(DragKind::Rotate {
                    center,
                    start_ang,
                    acc: delta,
                });
                self.dirty = true;
            }
            DragKind::Node { id, sub, seg, role, last } => {
                tools::move_node(self, &id, sub, seg, role, x - last.0, y - last.1);
                self.drag = Some(DragKind::Node {
                    id,
                    sub,
                    seg,
                    role,
                    last: (x, y),
                });
                self.dirty = true;
            }
            DragKind::PenPull { index } => {
                if let Some(pen) = self.pen.as_mut() {
                    if let Some(node) = pen.nodes.get_mut(index) {
                        let anchor = node.p;
                        let (hx, hy) = if self.shift_down {
                            let dx = x - anchor.0;
                            let dy = y - anchor.1;
                            let len = dx.hypot(dy);
                            let step = std::f32::consts::FRAC_PI_4;
                            let ang = (dy.atan2(dx) / step).round() * step;
                            (anchor.0 + ang.cos() * len, anchor.1 + ang.sin() * len)
                        } else {
                            (x, y)
                        };
                        let dx = hx - anchor.0;
                        let dy = hy - anchor.1;
                        if dx.hypot(dy) >= 1.2 {
                            node.hout = Some((hx, hy));
                            node.hin = Some((anchor.0 - dx, anchor.1 - dy));
                        } else {
                            node.hout = None;
                            node.hin = None;
                        }
                    }
                }
                self.drag = Some(DragKind::PenPull { index });
            }
            DragKind::Pencil { mut pts } => {
                pts.push((x, y));
                self.drag = Some(DragKind::Pencil { pts });
            }
            DragKind::Canvas {
                edge,
                orig_w,
                orig_h,
                origin,
                scale,
                ..
            } => {
                let scale = scale.max(0.05);
                let dx = (f32::from(ev.position.x) - origin.0) / scale;
                let dy = (f32::from(ev.position.y) - origin.1) / scale;
                let content = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.content_bounds())
                    .map(|r| (r.x0, r.y0, r.x1, r.y1));
                let tol = (EDGE_SNAP_SCREEN_PX / scale).max(1.0);
                let (new_w, new_h, shift_x, shift_y) = canvas_edge_drag_snapped(
                    edge,
                    orig_w,
                    orig_h,
                    dx,
                    dy,
                    content,
                    (self.fit_hint.snap_w, self.fit_hint.snap_h),
                    tol,
                );
                self.drag = Some(DragKind::Canvas {
                    edge,
                    orig_w,
                    orig_h,
                    origin,
                    scale,
                    new_w,
                    new_h,
                    shift_x,
                    shift_y,
                });
            }
            DragKind::PaletteSb
            | DragKind::PaletteHue
            | DragKind::PaletteAlpha
            | DragKind::Slider(_)
            | DragKind::LayerReorder { .. }
            | DragKind::OutlineScroll { .. } => {}
            DragKind::Lasso { mut pts } => {
                pts.push((x, y));
                self.lasso = pts.clone();
                self.drag = Some(DragKind::Lasso { pts });
            }
        }
        self.notify_nav(cx);
    }

    fn on_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.shift_down = ev.modifiers.shift;
        self.alt_down = ev.modifiers.alt;
        self.ctrl_down = apply_bg::is_primary_mod(&ev.modifiers);
        let before = self.selected.clone();
        let drag = self.drag.take();
        let (x, y) = self.doc_pt(f32::from(ev.position.x), f32::from(ev.position.y));
        match drag {
            Some(DragKind::Marquee {
                x0,
                y0,
                x1,
                y1,
                add,
                sub,
                toggle,
            }) => {
                if (x1 - x0).abs() > 1.0 || (y1 - y0).abs() > 1.0 {
                    tools::apply_marquee(self, Rect::new(x0, y0, x1, y1), add, sub, toggle);
                } else {
                    self.alt_down = sub;
                    self.ctrl_down = toggle;
                    self.shift_down = add;
                    tools::click_select(self, x0, y0);
                }
            }
            Some(DragKind::Pencil { pts }) => tools::finish_pencil(self, &pts, cx),
            Some(DragKind::Lasso { pts }) => {
                if pts.len() >= 3 {
                    tools::apply_lasso(self, &pts);
                }
                self.lasso.clear();
            }
            Some(DragKind::Move { origin, copy }) => {
                let (dx, dy) = self
                    .sel_sprite
                    .as_ref()
                    .map(|s| (s.dx, s.dy))
                    .unwrap_or_else(|| axis_lock(self.shift_down, x - origin.0, y - origin.1));
                if dx.abs() < 0.2 && dy.abs() < 0.2 && !copy {
                    self.undo_stack.pop();
                    if let Some(leaf) = self.pending_drill.take() {
                        self.selected = vec![leaf.clone()];
                        self.outline_anchor = Some(leaf.clone());
                        self.adopt_object_style(&leaf, cx);
                    }
                } else if copy {
                    self.pending_drill = None;
                    if let Some(doc) = self.doc.as_mut() {
                        let ids = doc.duplicate_ids(&self.selected, dx, dy);
                        if !ids.is_empty() {
                            self.selected = ids;
                        }
                    }
                    self.dirty = true;
                } else {
                    self.pending_drill = None;
                    if let Some(doc) = self.doc.as_mut() {
                        doc.translate_ids(&self.selected, dx, dy);
                    }
                    self.dirty = true;
                }
                self.rebuild_raster(cx);
            }
            Some(DragKind::Canvas {
                edge,
                orig_w,
                orig_h,
                scale,
                new_w,
                new_h,
                ..
            }) => {
                let span = match edge {
                    CanvasEdge::Left | CanvasEdge::Right => new_w,
                    CanvasEdge::Top | CanvasEdge::Bottom => new_h,
                };
                let before = match edge {
                    CanvasEdge::Left | CanvasEdge::Right => orig_w,
                    CanvasEdge::Top | CanvasEdge::Bottom => orig_h,
                };
                if (span - before).abs() > 0.4 {
                    self.push_undo();
                    tools::resize_canvas(self, edge, span);
                    // 上边和左边改的是原点. 视口跟着补, 内容和屏幕上的对边才不会跳.
                    match edge {
                        CanvasEdge::Left => self.pan.x += (orig_w - new_w) * scale,
                        CanvasEdge::Top => self.pan.y += (orig_h - new_h) * scale,
                        CanvasEdge::Right | CanvasEdge::Bottom => {}
                    }
                    self.dirty = true;
                    self.status = format!("画布 {:.0} × {:.0}", self.doc.as_ref().map(|d| d.canvas_w).unwrap_or(span), self.doc.as_ref().map(|d| d.canvas_h).unwrap_or(span)).into();
                }
                self.rebuild_raster(cx);
            }
            Some(DragKind::Scale { handle, bounds, sx, sy }) => {
                if (sx - 1.0).abs() < 0.002 && (sy - 1.0).abs() < 0.002 {
                    self.undo_stack.pop();
                } else {
                    let (ox, oy) = scale_anchor(handle, bounds);
                    if let Some(doc) = self.doc.as_mut() {
                        doc.scale_ids(&self.selected, ox, oy, sx, sy);
                    }
                    self.dirty = true;
                }
                self.rebuild_raster(cx);
            }
            Some(DragKind::Rotate { .. }) | Some(DragKind::Node { .. }) => {
                self.rebuild_raster(cx);
            }
            Some(DragKind::Slider(_)) => self.finish_slider(cx),
            Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha) => {
                self.palette_pointer_up(cx);
            }
            Some(DragKind::LayerReorder {
                from, to, armed, ..
            }) => {
                self.commit_layer_reorder(from, to, armed, cx);
            }
            Some(DragKind::PenPull { .. })
            | Some(DragKind::Pan { .. })
            | Some(DragKind::OutlineScroll { .. })
            | None => {}
        }
        if self.mode == ToolMode::Node {
            self.refresh_node_hover(x, y);
        }
        if self.selected != before {
            self.reveal_outline_selection();
        }
        self.notify_chrome(cx);
    }

    pub(super) fn refresh_node_hover(&mut self, x: f32, y: f32) -> bool {
        let tol = self.hit_tol();
        let node = self
            .node_target
            .as_deref()
            .or_else(|| self.selected.first().map(String::as_str))
            .and_then(|id| self.doc.as_ref().and_then(|d| d.object(id)))
            .and_then(|obj| tools::node_at(obj, x, y, tol))
            .map(|(_, _, role)| role);
        let hit = node.is_none() && self.doc.as_ref().is_some_and(|d| d.hits(x, y, tol));
        let changed = self.hover_node != node || self.hover_hit != hit;
        self.hover_node = node;
        self.hover_hit = hit;
        changed
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (delta_x, delta_y) = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(p) => (p.x * 32.0, p.y * 32.0),
        };
        let wheel = if delta_y.abs() >= delta_x.abs() { delta_y } else { delta_x };
        if ev.modifiers.alt || self.alt_down {
            if wheel.abs() > 0.01 {
                self.bump_stroke_width(wheel > 0.0, cx);
            }
            cx.stop_propagation();
            return;
        }
        if apply_bg::is_primary_mod(&ev.modifiers) {
            let (doc_x, doc_y) = self.doc_pt(f32::from(ev.position.x), f32::from(ev.position.y));
            self.lock_user_view();
            let factor = if wheel > 0.0 { 1.12 } else { 1.0 / 1.12 };
            let new_scale = (self.zoom * factor).clamp(0.05, 48.0);
            let vx = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
            let vy = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
            self.zoom = new_scale;
            self.pan = point(vx - doc_x * new_scale, vy - doc_y * new_scale);
            self.invalidate_raster_wait();
            self.notify_nav(cx);
            cx.stop_propagation();
            return;
        }
        self.lock_user_view();
        if ev.modifiers.shift {
            self.pan.x += if delta_x.abs() > 0.01 { delta_x } else { delta_y };
        } else {
            self.pan.x += delta_x;
            self.pan.y += delta_y;
        }
        self.notify_nav(cx);
        cx.stop_propagation();
    }
}

fn scale_anchor(handle: usize, bounds: Rect) -> (f32, f32) {
    match handle {
        0 => (bounds.x1, bounds.y1),
        1 => (bounds.cx(), bounds.y1),
        2 => (bounds.x0, bounds.y1),
        3 => (bounds.x0, bounds.cy()),
        4 => (bounds.x0, bounds.y0),
        5 => (bounds.cx(), bounds.y0),
        6 => (bounds.x1, bounds.y0),
        _ => (bounds.x1, bounds.cy()),
    }
}

fn scale_factors(handle: usize, bounds: Rect, x: f32, y: f32, shift: bool) -> (f32, f32) {
    let sx = if bounds.w().abs() < 0.5 {
        1.0
    } else {
        match handle {
            0 | 6 | 7 => (bounds.x1 - x) / bounds.w(),
            2 | 3 | 4 => (x - bounds.x0) / bounds.w(),
            _ => 1.0,
        }
    };
    let sy = if bounds.h().abs() < 0.5 {
        1.0
    } else {
        match handle {
            0 | 1 | 2 => (bounds.y1 - y) / bounds.h(),
            4 | 5 | 6 => (y - bounds.y0) / bounds.h(),
            _ => 1.0,
        }
    };
    if shift && matches!(handle, 0 | 2 | 4 | 6) {
        let (ax, ay) = scale_anchor(handle, bounds);
        let (cx, cy) = match handle {
            0 => (bounds.x0, bounds.y0),
            2 => (bounds.x1, bounds.y0),
            4 => (bounds.x1, bounds.y1),
            _ => (bounds.x0, bounds.y1),
        };
        let vx = cx - ax;
        let vy = cy - ay;
        let denom = vx * vx + vy * vy;
        let s = if denom < 1.0 {
            1.0
        } else {
            ((x - ax) * vx + (y - ay) * vy) / denom
        };
        let s = s.max(0.05);
        return (s, s);
    }
    (sx.max(0.05), sy.max(0.05))
}

fn axis_lock(shift: bool, dx: f32, dy: f32) -> (f32, f32) {
    if !shift {
        return (dx, dy);
    }
    if dx.abs() >= dy.abs() {
        (dx, 0.0)
    } else {
        (0.0, dy)
    }
}

struct Xform {
    scale: f32,
    ox: f32,
    oy: f32,
}

impl Xform {
    fn compute(
        cw: f32,
        ch: f32,
        bounds: Bounds<gpui::Pixels>,
        zoom: f32,
        pan: gpui::Point<f32>,
        user_zoomed: bool,
    ) -> Self {
        let vw = f32::from(bounds.size.width).max(1.0);
        let vh = f32::from(bounds.size.height).max(1.0);
        if user_zoomed {
            Self {
                scale: zoom.max(0.05),
                ox: pan.x,
                oy: pan.y,
            }
        } else {
            let s = ((vw - 32.0) / cw.max(1.0))
                .min((vh - 32.0) / ch.max(1.0))
                .clamp(0.05, 64.0);
            Self {
                scale: s,
                ox: (vw - cw * s) * 0.5,
                oy: (vh - ch * s) * 0.5,
            }
        }
    }

    fn doc_to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.scale, self.oy + y * self.scale)
    }

    fn screen_to_doc(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.ox) / self.scale, (y - self.oy) / self.scale)
    }

    fn doc_bounds(&self, view: Bounds<gpui::Pixels>, x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        let (sx, sy) = self.doc_to_screen(x, y);
        Bounds {
            origin: point(view.origin.x + px(sx), view.origin.y + px(sy)),
            size: size(px(w * self.scale), px(h * self.scale)),
        }
    }
}

fn paint_checker(window: &mut Window, bounds: Bounds<gpui::Pixels>, color: gpui::Rgba) {
    let _ = window.paint_quad(quad(bounds, px(0.), color, px(0.), color, Default::default()));
}

fn paint_selection_edges(
    app: &VectorEditApp,
    window: &mut Window,
    view: Bounds<gpui::Pixels>,
    xform: &Xform,
    off: (f32, f32),
    scale: Option<(f32, f32, f32, f32)>,
) {
    let Some(doc) = app.doc.as_ref() else {
        return;
    };
    let leaves = doc.leaf_ids(&app.selected);
    if leaves.is_empty() || leaves.len() > 400 {
        return;
    }
    let map = |x: f32, y: f32| -> (f32, f32) {
        if let Some((ox, oy, sx, sy)) = scale {
            (ox + (x - ox) * sx, oy + (y - oy) * sy)
        } else {
            (x + off.0, y + off.1)
        }
    };
    for id in leaves {
        let Some(obj) = doc.object(&id) else {
            continue;
        };
        match &obj.kind {
            crate::document::ObjKind::Path(path) => {
                paint_mapped_subs(window, view, xform, &path.subpaths, &map, px(3.0), rgb(0xffffff));
                paint_mapped_subs(window, view, xform, &path.subpaths, &map, px(1.25), rgb(0x2563eb));
            }
            _ => {
                if let Some(b) = obj.bounds() {
                    let (x0, y0) = map(b.x0, b.y0);
                    let (x1, y1) = map(b.x1, b.y1);
                    let r = Rect::new(x0, y0, x1, y1);
                    paint_rect_outline_w(window, view, xform, r, px(3.0), rgb(0xffffff));
                    paint_rect_outline_w(window, view, xform, r, px(1.25), rgb(0x2563eb));
                }
            }
        }
    }
}

fn paint_mapped_subs(
    window: &mut Window,
    view: Bounds<gpui::Pixels>,
    xform: &Xform,
    subs: &[crate::document::Subpath],
    map: &impl Fn(f32, f32) -> (f32, f32),
    width: gpui::Pixels,
    color: gpui::Rgba,
) {
    let mut b = PathBuilder::stroke(width);
    let mut any = false;
    for sub in subs {
        let Some(first) = sub.segs.first() else {
            continue;
        };
        let start = map(first.start().0, first.start().1);
        let (x, y) = xform.doc_to_screen(start.0, start.1);
        b.move_to(point(view.origin.x + px(x), view.origin.y + px(y)));
        any = true;
        for seg in &sub.segs {
            match seg {
                crate::document::Seg::Line { b: end, .. } => {
                    let end = map(end.0, end.1);
                    let (x, y) = xform.doc_to_screen(end.0, end.1);
                    b.line_to(point(view.origin.x + px(x), view.origin.y + px(y)));
                }
                crate::document::Seg::Cubic { p1, p2, p3, .. } => {
                    let p1 = map(p1.0, p1.1);
                    let p2 = map(p2.0, p2.1);
                    let p3 = map(p3.0, p3.1);
                    let (x1, y1) = xform.doc_to_screen(p1.0, p1.1);
                    let (x2, y2) = xform.doc_to_screen(p2.0, p2.1);
                    let (x3, y3) = xform.doc_to_screen(p3.0, p3.1);
                    b.cubic_bezier_to(
                        point(view.origin.x + px(x3), view.origin.y + px(y3)),
                        point(view.origin.x + px(x1), view.origin.y + px(y1)),
                        point(view.origin.x + px(x2), view.origin.y + px(y2)),
                    );
                }
            }
        }
        if sub.closed {
            b.close();
        }
    }
    if any {
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    }
}

fn paint_rect_outline(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, r: Rect, color: gpui::Rgba) {
    paint_rect_outline_w(window, view, xform, r, px(1.), color);
}

fn paint_rect_outline_w(
    window: &mut Window,
    view: Bounds<gpui::Pixels>,
    xform: &Xform,
    r: Rect,
    width: gpui::Pixels,
    color: gpui::Rgba,
) {
    let (x0, y0) = xform.doc_to_screen(r.x0, r.y0);
    let (x1, y1) = xform.doc_to_screen(r.x1, r.y1);
    let b = Bounds {
        origin: point(view.origin.x + px(x0), view.origin.y + px(y0)),
        size: size(px((x1 - x0).abs()), px((y1 - y0).abs())),
    };
    let _ = window.paint_quad(quad(b, px(0.), gpui::transparent_black(), width, color, Default::default()));
}

fn paint_handle(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, x: f32, y: f32, color: gpui::Rgba) {
    paint_handle_sized(window, view, xform, x, y, 3.0, color);
}

fn paint_handle_sized(
    window: &mut Window,
    view: Bounds<gpui::Pixels>,
    xform: &Xform,
    x: f32,
    y: f32,
    half: f32,
    color: gpui::Rgba,
) {
    let (sx, sy) = xform.doc_to_screen(x, y);
    let b = Bounds {
        origin: point(view.origin.x + px(sx - half), view.origin.y + px(sy - half)),
        size: size(px(half * 2.0), px(half * 2.0)),
    };
    let _ = window.paint_quad(quad(b, px(1.), rgb(0xffffff), px(1.), color, Default::default()));
}

fn paint_obj_preview(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, obj: &crate::document::Obj) {
    let crate::document::ObjKind::Path(path) = &obj.kind else {
        return;
    };
    if let Some(fill) = &path.fill {
        let mut b = PathBuilder::fill();
        if path.fill_rule == crate::document::FillRule::EvenOdd {
            b.style = gpui::PathStyle::Fill(
                gpui::FillOptions::default().with_fill_rule(gpui::FillRule::EvenOdd),
            );
        }
        append_subpaths(&mut b, view, xform, &path.subpaths);
        if let Ok(built) = b.build() {
            window.paint_path(built, bytes_rgba(fill.rgba));
        }
    }
    if let Some(st) = &path.stroke {
        let width = (st.width * xform.scale).max(0.75);
        let mut b = PathBuilder::stroke(px(width));
        if !st.dash.is_empty() {
            let dashes: Vec<gpui::Pixels> = st.dash.iter().map(|d| px((*d * xform.scale).max(0.4))).collect();
            b = b.dash_array(&dashes);
        }
        append_subpaths(&mut b, view, xform, &path.subpaths);
        if let Ok(built) = b.build() {
            window.paint_path(built, bytes_rgba(st.paint.rgba));
        }
    }
}

fn append_subpaths(
    b: &mut PathBuilder,
    view: Bounds<gpui::Pixels>,
    xform: &Xform,
    subs: &[crate::document::Subpath],
) {
    for sub in subs {
        let Some(first) = sub.segs.first() else {
            continue;
        };
        let start = first.start();
        let (x, y) = xform.doc_to_screen(start.0, start.1);
        b.move_to(point(view.origin.x + px(x), view.origin.y + px(y)));
        for seg in &sub.segs {
            match seg {
                crate::document::Seg::Line { b: end, .. } => {
                    let (x, y) = xform.doc_to_screen(end.0, end.1);
                    b.line_to(point(view.origin.x + px(x), view.origin.y + px(y)));
                }
                crate::document::Seg::Cubic { p1, p2, p3, .. } => {
                    let (x1, y1) = xform.doc_to_screen(p1.0, p1.1);
                    let (x2, y2) = xform.doc_to_screen(p2.0, p2.1);
                    let (x3, y3) = xform.doc_to_screen(p3.0, p3.1);
                    b.cubic_bezier_to(
                        point(view.origin.x + px(x3), view.origin.y + px(y3)),
                        point(view.origin.x + px(x1), view.origin.y + px(y1)),
                        point(view.origin.x + px(x2), view.origin.y + px(y2)),
                    );
                }
            }
        }
        if sub.closed {
            b.close();
        }
    }
}

fn bytes_rgba(c: [u8; 4]) -> gpui::Rgba {
    gpui::Rgba {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: c[3] as f32 / 255.0,
    }
}

fn paint_nodes(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, obj: &crate::document::Obj) {
    let crate::document::ObjKind::Path(path) = &obj.kind else {
        return;
    };
    for sub in &path.subpaths {
        if let Some(seg) = sub.segs.first() {
            let st = seg.start();
            let skip = sub.closed
                && sub.segs.last().is_some_and(|s| {
                    let e = s.end();
                    (e.0 - st.0).hypot(e.1 - st.1) < 0.5
                });
            if !skip {
                paint_handle(window, view, xform, st.0, st.1, rgb(0x2563eb));
            }
        }
        for seg in &sub.segs {
            let end = seg.end();
            paint_handle(window, view, xform, end.0, end.1, rgb(0x2563eb));
            if let crate::document::Seg::Cubic { p0, p1, p2, p3 } = seg {
                if (p1.0 - p0.0).hypot(p1.1 - p0.1) > 0.6 {
                    paint_seg(window, view, xform, *p0, *p1);
                    paint_handle(window, view, xform, p1.0, p1.1, rgb(0xf97316));
                }
                if (p2.0 - p3.0).hypot(p2.1 - p3.1) > 0.6 {
                    paint_seg(window, view, xform, *p3, *p2);
                    paint_handle(window, view, xform, p2.0, p2.1, rgb(0xf97316));
                }
            }
        }
    }
}

fn paint_pen(
    window: &mut Window,
    view: Bounds<gpui::Pixels>,
    xform: &Xform,
    pen: &PenDraft,
    cursor: Option<(f32, f32)>,
) {
    let mut nodes = pen.nodes.clone();
    let closing = cursor.is_some_and(|p| {
        nodes.len() >= 2 && nodes.first().is_some_and(|n| (n.p.0 - p.0).hypot(n.p.1 - p.1) < 0.2)
    });
    if let Some(p) = cursor {
        if closing {
            if let Some(first) = nodes.first().cloned() {
                nodes.push(first);
            }
        } else if nodes.last().is_none_or(|n| (n.p.0 - p.0).hypot(n.p.1 - p.1) > 0.5) {
            nodes.push(PenNode {
                p,
                hin: None,
                hout: None,
            });
        }
    }
    paint_draft_segs(window, view, xform, &tools::nodes_to_segs(&nodes));
    for (i, n) in pen.nodes.iter().enumerate() {
        if closing && i == 0 {
            paint_handle_sized(window, view, xform, n.p.0, n.p.1, 5.0, rgb(0x16a34a));
        } else {
            paint_handle(window, view, xform, n.p.0, n.p.1, rgb(0x2563eb));
        }
        if let Some(h) = n.hin {
            paint_seg(window, view, xform, n.p, h);
            paint_handle(window, view, xform, h.0, h.1, rgb(0xf97316));
        }
        if let Some(h) = n.hout {
            paint_seg(window, view, xform, n.p, h);
            paint_handle(window, view, xform, h.0, h.1, rgb(0xf97316));
        }
    }
}

fn paint_draft_segs(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, segs: &[crate::document::Seg]) {
    let Some(first) = segs.first() else {
        return;
    };
    let mut b = PathBuilder::stroke(px(1.25));
    let start = first.start();
    let (x, y) = xform.doc_to_screen(start.0, start.1);
    b.move_to(point(view.origin.x + px(x), view.origin.y + px(y)));
    for seg in segs {
        match seg {
            crate::document::Seg::Line { b: end, .. } => {
                let (x, y) = xform.doc_to_screen(end.0, end.1);
                b.line_to(point(view.origin.x + px(x), view.origin.y + px(y)));
            }
            crate::document::Seg::Cubic { p1, p2, p3, .. } => {
                let (x1, y1) = xform.doc_to_screen(p1.0, p1.1);
                let (x2, y2) = xform.doc_to_screen(p2.0, p2.1);
                let (x3, y3) = xform.doc_to_screen(p3.0, p3.1);
                b.cubic_bezier_to(
                    point(view.origin.x + px(x3), view.origin.y + px(y3)),
                    point(view.origin.x + px(x1), view.origin.y + px(y1)),
                    point(view.origin.x + px(x2), view.origin.y + px(y2)),
                );
            }
        }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, rgb(0x0284c7));
    }
}

fn paint_seg(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, a: (f32, f32), b: (f32, f32)) {
    paint_poly(window, view, xform, &[a, b], false);
}

fn paint_edge(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, edge: CanvasEdge, cw: f32, ch: f32) {
    let (a, b) = match edge {
        CanvasEdge::Left => ((0.0, 0.0), (0.0, ch)),
        CanvasEdge::Right => ((cw, 0.0), (cw, ch)),
        CanvasEdge::Top => ((0.0, 0.0), (cw, 0.0)),
        CanvasEdge::Bottom => ((0.0, ch), (cw, ch)),
    };
    let mut path = PathBuilder::stroke(px(2.0));
    let (x, y) = xform.doc_to_screen(a.0, a.1);
    path.move_to(point(view.origin.x + px(x), view.origin.y + px(y)));
    let (x, y) = xform.doc_to_screen(b.0, b.1);
    path.line_to(point(view.origin.x + px(x), view.origin.y + px(y)));
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(0x2563eb));
    }
}

fn paint_brush(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, x: f32, y: f32, radius: f32) {
    let r = (radius * xform.scale).clamp(2.0, 80.0);
    let (sx, sy) = xform.doc_to_screen(x, y);
    let b = Bounds {
        origin: point(view.origin.x + px(sx - r), view.origin.y + px(sy - r)),
        size: size(px(r * 2.0), px(r * 2.0)),
    };
    let _ = window.paint_quad(quad(
        b,
        px(r),
        gpui::transparent_black(),
        px(1.25),
        rgb(0x0284c7),
        Default::default(),
    ));
}

fn snap_device_bounds(b: Bounds<gpui::Pixels>, device_scale: f32) -> Bounds<gpui::Pixels> {
    let sf = if device_scale.is_finite() && device_scale > 0.05 {
        device_scale
    } else {
        1.0
    };
    let x0 = f32::from(b.origin.x);
    let y0 = f32::from(b.origin.y);
    let x1 = x0 + f32::from(b.size.width);
    let y1 = y0 + f32::from(b.size.height);
    let sx0 = (x0 * sf).round() / sf;
    let sy0 = (y0 * sf).round() / sf;
    let sx1 = (x1 * sf).round() / sf;
    let sy1 = (y1 * sf).round() / sf;
    Bounds {
        origin: point(px(sx0), px(sy0)),
        size: size(
            px((sx1 - sx0).max(1.0 / sf)),
            px((sy1 - sy0).max(1.0 / sf)),
        ),
    }
}

/// 贴图最长边. 整页放得下就画整页; 放不下 (放大后) 只画视口, 密度仍按屏幕物理像素.
const RASTER_MAX_SIDE: f32 = 8192.0;
/// 视口外多画一圈, 方便小幅平移. 不要太大, 放大后这圈会把光栅面积抬上去.
const RASTER_VIEW_PAD: f32 = 0.22;

pub(crate) struct RasterPlan {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 物理像素 / point.
    pub px: f32,
}

pub(crate) fn plan_raster(
    canvas_w: f32,
    canvas_h: f32,
    view_w: f32,
    view_h: f32,
    scale: f32,
    ox: f32,
    oy: f32,
    device_scale: f32,
) -> RasterPlan {
    let cw = canvas_w.max(1.0);
    let ch = canvas_h.max(1.0);
    if view_w < 8.0 || view_h < 8.0 {
        return RasterPlan {
            x: 0.0,
            y: 0.0,
            w: cw,
            h: ch,
            px: 1.0,
        };
    }
    let scale = scale.max(0.05);
    let dpr = if device_scale.is_finite() && device_scale > 0.05 {
        device_scale
    } else {
        1.0
    };
    let mut px = (scale * dpr).clamp(0.05, 192.0);
    if cw.max(ch) * px <= RASTER_MAX_SIDE {
        return RasterPlan {
            x: 0.0,
            y: 0.0,
            w: cw,
            h: ch,
            px,
        };
    }
    let vx0 = (0.0 - ox) / scale;
    let vy0 = (0.0 - oy) / scale;
    let vx1 = (view_w - ox) / scale;
    let vy1 = (view_h - oy) / scale;
    let (vx0, vx1) = (vx0.min(vx1), vx0.max(vx1));
    let (vy0, vy1) = (vy0.min(vy1), vy0.max(vy1));
    let vw = (vx1 - vx0).max(1.0);
    let vh = (vy1 - vy0).max(1.0);
    let rw = (vw * (1.0 + RASTER_VIEW_PAD * 2.0)).max(1.0);
    let rh = (vh * (1.0 + RASTER_VIEW_PAD * 2.0)).max(1.0);
    let side = rw.max(rh) * px;
    if side > RASTER_MAX_SIDE {
        px *= RASTER_MAX_SIDE / side;
    }
    RasterPlan {
        x: vx0 - vw * RASTER_VIEW_PAD,
        y: vy0 - vh * RASTER_VIEW_PAD,
        w: rw,
        h: rh,
        px,
    }
}

fn paint_poly(window: &mut Window, view: Bounds<gpui::Pixels>, xform: &Xform, pts: &[(f32, f32)], _close: bool) {
    if pts.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(px(1.25));
    let (x, y) = xform.doc_to_screen(pts[0].0, pts[0].1);
    b.move_to(point(view.origin.x + px(x), view.origin.y + px(y)));
    for p in pts.iter().skip(1) {
        let (x, y) = xform.doc_to_screen(p.0, p.1);
        b.line_to(point(view.origin.x + px(x), view.origin.y + px(y)));
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, rgb(0x0284c7));
    }
}

#[cfg(test)]
mod tests {
    use super::{plan_raster, scale_anchor, scale_factors};
    use crate::geom::Rect;

    #[test]
    fn corner_scale_keeps_opposite_corner_and_shift_locks_ratio() {
        let b = Rect::new(0.0, 0.0, 100.0, 100.0);
        let (sx, sy) = scale_factors(4, b, 150.0, 80.0, false);
        assert!((sx - 1.5).abs() < 1e-4);
        assert!((sy - 0.8).abs() < 1e-4);
        assert_eq!(scale_anchor(4, b), (0.0, 0.0));
        let (sx, sy) = scale_factors(4, b, 150.0, 80.0, true);
        assert!((sx - sy).abs() < 1e-4);
        // 指针投到对角线上, 宽高用同一个比例.
        assert!((sx - 1.15).abs() < 1e-4, "sx {sx}");
        let wide = Rect::new(0.0, 0.0, 200.0, 50.0);
        let (sx, sy) = scale_factors(4, wide, 400.0, 50.0, true);
        assert!((sx - sy).abs() < 1e-4);
        assert!((sx - 1.941176).abs() < 1e-3, "sx {sx}");
        let (sx, sy) = scale_factors(4, wide, 400.0, 100.0, true);
        assert!((sx - 2.0).abs() < 1e-3 && (sy - 2.0).abs() < 1e-3);
    }

    #[test]
    fn fit_page_uses_device_pixels() {
        let plan = plan_raster(800.0, 600.0, 900.0, 700.0, 1.0, 50.0, 40.0, 2.0);
        assert!((plan.px - 2.0).abs() < 1e-4, "px {}", plan.px);
        assert_eq!(plan.x, 0.0);
        assert_eq!(plan.y, 0.0);
        assert!((plan.w - 800.0).abs() < 1e-3);
        assert!((plan.h - 600.0).abs() < 1e-3);
    }

    #[test]
    fn zoomed_view_stays_at_device_density() {
        let scale = 8.0;
        let dpr = 2.0;
        let ox = 100.0;
        let oy = 80.0;
        let vw = 800.0;
        let vh = 600.0;
        let plan = plan_raster(2000.0, 3000.0, vw, vh, scale, ox, oy, dpr);
        assert!((plan.px - scale * dpr).abs() < 1e-3, "px {}", plan.px);
        let vis_x0 = (0.0 - ox) / scale;
        let vis_y0 = (0.0 - oy) / scale;
        let vis_x1 = (vw - ox) / scale;
        let vis_y1 = (vh - oy) / scale;
        assert!(plan.x <= vis_x0 + 1e-3);
        assert!(plan.y <= vis_y0 + 1e-3);
        assert!(plan.x + plan.w >= vis_x1 - 1e-3);
        assert!(plan.y + plan.h >= vis_y1 - 1e-3);
        assert!(plan.w * plan.px <= 8192.5);
        assert!(plan.h * plan.px <= 8192.5);
    }

    #[test]
    fn huge_viewport_stays_under_texture_cap() {
        let plan = plan_raster(10000.0, 10000.0, 4000.0, 3000.0, 10.0, 0.0, 0.0, 2.0);
        let side = plan.w.max(plan.h) * plan.px;
        assert!(side <= 8192.5, "side {side}");
        assert!(plan.px < 20.0, "px {}", plan.px);
        assert!(plan.x <= 0.0);
        assert!(plan.x + plan.w >= 400.0);
    }
}
