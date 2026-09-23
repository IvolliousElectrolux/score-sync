use gpui::Negate;
use gpui::{
    canvas, point, prelude::*, px, quad, radians, rgb, size, Bounds, ContentMask, Corners,
    CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Point,
    ScrollDelta, ScrollWheelEvent, TransformationMatrix, Window,
};

use super::*;
use crate::process::Selection;

impl PhotoEditApp {
    pub fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let layers = self.layer_paints();
        let paper = self
            .doc
            .as_ref()
            .map(|d| d.paper_rgb)
            .unwrap_or([226, 232, 240]);
        let paper_rgb = rgb(((paper[0] as u32) << 16) | ((paper[1] as u32) << 8) | paper[2] as u32);
        let zoom = self.zoom;
        let pan = self.pan;
        let user_zoomed = self.user_zoomed;
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w as f32, d.canvas_h as f32))
            .unwrap_or((1.0, 1.0));
        let sel = self.selection.clone();
        let drag_sel = match &self.drag {
            Some(DragKind::Select { x0, y0, x1, y1 }) => Some((*x0, *y0, *x1, *y1)),
            Some(DragKind::SelResize { cur, .. }) => {
                Some((cur.0 as f32, cur.1 as f32, cur.2 as f32, cur.3 as f32))
            }
            _ => None,
        };
        let sel_outline = if drag_sel.is_none() {
            sel.outline().map(|pts| pts.to_vec())
        } else {
            None
        };
        let sel_loops = if drag_sel.is_none() {
            sel.loops().map(|ls| ls.to_vec())
        } else {
            None
        };
        let show_sel_handles = self.mode.uses_selection() && drag_sel.is_none();
        let lasso_draft = self.lasso_draft.clone();
        let lasso_cursor = self.lasso_cursor;
        let show_canvas = self.mode == ToolMode::Canvas;
        let canvas_hot = match &self.drag {
            Some(DragKind::Canvas {
                edge,
                orig_w,
                orig_h,
                ..
            }) => Some((*edge, *orig_w as f32, *orig_h as f32)),
            _ => self.canvas_hover.map(|e| (e, 0.0, 0.0)),
        };
        let canvas_drag = match &self.drag {
            Some(DragKind::Canvas {
                orig_w,
                orig_h,
                orig_pos,
                orig_pan,
                ..
            }) => {
                let (ox, oy) = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.layers.first().zip(orig_pos.first()))
                    .map(|(l, (x, y))| (*x - l.x, *y - l.y))
                    .unwrap_or((0, 0));
                Some((*orig_w as f32, *orig_h as f32, *orig_pan, ox, oy))
            }
            _ => None,
        };
        let show_handles = self.mode == ToolMode::Move;
        let show_stamp = self.mode == ToolMode::Clone && self.clone_alt;
        let paint_fill = if self.mode == ToolMode::Paint {
            Some(self.paint_color)
        } else {
            None
        };
        let brush_cursor = if self.mode.uses_brush() && !show_stamp {
            self.brush_cursor
        } else {
            None
        };
        let stamp_img = if show_stamp { self.brush_cursor } else { None };
        let clone_mark = if self.mode == ToolMode::Clone && !show_stamp {
            match (self.clone_offset, self.brush_cursor) {
                (Some((ox, oy)), Some((bx, by))) => Some((bx + ox as f32, by + oy as f32)),
                _ => None,
            }
        } else {
            None
        };
        let brush_size = self.brush;
        let pointer = self.last_pointer_view;
        let scale_now = self.view_xform().scale.max(0.0001);
        let brush_r_img = brush_size * 0.5;
        let stamp_r_img = brush_r_img.max(8.0 / scale_now);
        let brush_ring = brush_cursor.map(|(bx, by)| {
            let samples = self.sample_ring_under(bx, by, brush_r_img);
            ring_style_from_under(&samples, paint_fill)
        });
        let stamp_ring = if show_stamp {
            let img_pt = stamp_img
                .or_else(|| pointer.map(|(vx, vy)| self.view_xform().screen_to_image(vx, vy)));
            img_pt.map(|(ix, iy)| {
                let samples = self.sample_ring_under(ix, iy, stamp_r_img);
                ring_style_from_under(&samples, None)
            })
        } else {
            None
        };
        let cross_ring = clone_mark.map(|(ix, iy)| {
            let samples = self.sample_ring_under(ix, iy, brush_r_img);
            ring_style_from_under(&samples, None)
        });
        let show_rotate_cur =
            self.hover_rotate || matches!(self.drag, Some(DragKind::LayerRotate { .. }));
        let live_cursor = if show_rotate_cur || show_stamp {
            CursorStyle::None
        } else {
            self.live_cursor()
        };

        div()
            .id("photo_image_view")
            .flex_1()
            .size_full()
            .min_w_0()
            .min_h(px(0.))
            .bg(rgb(0x2b2b2b))
            .overflow_hidden()
            .relative()
            .occlude()
            .cursor(live_cursor)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev, window, cx| {
                    this.focus_handle.focus(window);
                    this.on_mouse_down(ev, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    if this.eyedropper_armed {
                        this.cancel_eyedropper(cx);
                    } else if this.lasso_draft.is_some() {
                        this.cancel_lasso_draft(cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window);
                    this.drag = Some(DragKind::Pan {
                        last_x: f32::from(ev.position.x),
                        last_y: f32::from(ev.position.y),
                    });
                    this.notify_nav(cx);
                }),
            )
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_modifiers_changed(cx.listener(|this, ev: &gpui::ModifiersChangedEvent, _, cx| {
                this.last_shift = ev.modifiers.shift;
                    if this.clone_alt != ev.modifiers.alt {
                    this.clone_alt = ev.modifiers.alt;
                    if this.mode == ToolMode::Clone || this.mode == ToolMode::Paint {
                        this.notify_nav(cx);
                    }
                }
                if matches!(
                    this.drag,
                    Some(DragKind::LayerRotate { .. } | DragKind::LayerScale { .. })
                ) {
                    this.apply_live_pointer(cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(|this, _, _, cx| {
                this.drag = None;
                this.notify_nav(cx);
            }))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(|this, _, _, cx| {
                this.drag = None;
                this.notify_nav(cx);
            }))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(canvas(
                {
                    let entity = cx.entity().clone();
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| this.view_bounds = bounds);
                    }
                },
                move |bounds, _, window, _| {
                    let vw = f32::from(bounds.size.width);
                    let vh = f32::from(bounds.size.height);
                    let (fit_w, fit_h, use_pan, zoomed) = if let Some((ow, oh, opan, ox, oy)) =
                        canvas_drag
                    {
                        let base = ViewXform::compute(ow, oh, vw, vh, zoom, opan, true);
                        let use_pan = point(
                            opan.x + ox as f32 * base.scale,
                            opan.y + oy as f32 * base.scale,
                        );
                        (ow, oh, use_pan, true)
                    } else {
                        (cw, ch, pan, user_zoomed)
                    };
                    let xform = ViewXform::compute(fit_w, fit_h, vw, vh, zoom, use_pan, zoomed);
                    let img_bounds = Bounds {
                        origin: point(
                            bounds.origin.x + px(xform.origin_x),
                            bounds.origin.y + px(xform.origin_y),
                        ),
                        size: size(px(cw * xform.scale), px(ch * xform.scale)),
                    };
                    window.paint_quad(quad(
                        img_bounds,
                        px(0.),
                        paper_rgb,
                        px(0.),
                        paper_rgb,
                        Default::default(),
                    ));
                    let ox = f32::from(bounds.origin.x);
                    let oy = f32::from(bounds.origin.y);
                    for layer in &layers {
                        let (sx, sy) = xform.image_to_screen(layer.x as f32, layer.y as f32);
                        let lb = Bounds {
                            origin: point(bounds.origin.x + px(sx), bounds.origin.y + px(sy)),
                            size: size(
                                px(layer.w as f32 * xform.scale),
                                px(layer.h as f32 * xform.scale),
                            ),
                        };
                        let xf = if layer.rotate_deg.abs() > 0.05 {
                            let sf = window.scale_factor();
                            let cx = lb.origin.x + lb.size.width / 2.0;
                            let cy = lb.origin.y + lb.size.height / 2.0;
                            let center = point(cx, cy).scale(sf);
                            Some(
                                TransformationMatrix::unit()
                                    .translate(center)
                                    .rotate(radians(layer.rotate_deg.to_radians()))
                                    .translate(center.negate()),
                            )
                        } else {
                            None
                        };
                        let paint_img = |window: &mut Window, b: Bounds<gpui::Pixels>, tex, xf: &Option<TransformationMatrix>| {
                            if let Some(xf) = xf {
                                let _ = window.paint_image_transformed(
                                    b,
                                    Corners::default(),
                                    tex,
                                    0,
                                    false,
                                    *xf,
                                );
                            } else {
                                let _ = window.paint_image(b, Corners::default(), tex, 0, false);
                            }
                        };
                        let ox0 = f32::from(bounds.origin.x);
                            let oy0 = f32::from(bounds.origin.y);
                            for tile in &layer.tiles {
                                let ix = layer.x as f32 + tile.x as f32;
                                let iy = layer.y as f32 + tile.y as f32;
                                let mut logical = image_px_bounds(
                                    &xform,
                                    ox0,
                                    oy0,
                                    ix,
                                    iy,
                                    ix + tile.w as f32,
                                    iy + tile.h as f32,
                                );
                                // 相邻裁切各外扩 1 像素, 避免取整后块与块之间露出缝.
                                // 外扩仍落在 2 像素渗色边以内, 碰不到被图集污染的最外圈.
                                logical.origin.x -= px(1.0);
                                logical.origin.y -= px(1.0);
                                logical.size.width += px(2.0);
                                logical.size.height += px(2.0);
                                let padded = image_px_bounds(
                                    &xform,
                                    ox0,
                                    oy0,
                                    ix - tile.pad_l as f32,
                                    iy - tile.pad_t as f32,
                                    ix + tile.w as f32 + tile.pad_r as f32,
                                    iy + tile.h as f32 + tile.pad_b as f32,
                                );
                                // 未旋转时用逻辑块裁切, 把图集渗色留在裁切外.
                                // 旋转后裁切框是轴对齐的, 会切掉转过的角, 这时整块贴上去.
                                if layer.rotate_deg.abs() <= 0.05 {
                                    window.with_content_mask(
                                        Some(ContentMask { bounds: logical }),
                                        |window| {
                                            paint_img(window, padded, tile.tex.clone(), &xf);
                                        },
                                    );
                                } else {
                                    paint_img(window, padded, tile.tex.clone(), &xf);
                                }
                            }
                    }
                    if show_handles {
                    for layer in &layers {
                        if layer.active {
                            continue;
                        }
                        paint_layer_obb(
                            window,
                            &xform,
                            ox,
                            oy,
                            layer,
                            rgb(0x64748b),
                            1.2,
                            true,
                        );
                    }
                    for layer in &layers {
                        if !layer.active {
                            continue;
                        }
                        paint_layer_obb(
                            window,
                            &xform,
                            ox,
                            oy,
                            layer,
                            rgb(0x38bdf8),
                            2.2,
                            false,
                        );
                    }
                    }
                    let mut paint_rect = |x0: f32, y0: f32, x1: f32, y1: f32, color: gpui::Rgba, thick: f32, dash: bool, pad: f32, fill: bool| {
                        let a = x0.min(x1);
                        let b = y0.min(y1);
                        let c = x0.max(x1);
                        let d = y0.max(y1);
                        let (sx0, sy0) = xform.image_to_screen(a, b);
                        let (sx1, sy1) = xform.image_to_screen(c + pad, d + pad);
                        let p0 = point(px(ox + sx0), px(oy + sy0));
                        let p1 = point(px(ox + sx1), px(oy + sy0));
                        let p2 = point(px(ox + sx1), px(oy + sy1));
                        let p3 = point(px(ox + sx0), px(oy + sy1));
                        if fill {
                            let mut fill_path = PathBuilder::fill();
                            fill_path.move_to(p0);
                            fill_path.line_to(p1);
                            fill_path.line_to(p2);
                            fill_path.line_to(p3);
                            fill_path.close();
                            if let Ok(path) = fill_path.build() {
                                let mut fill_c = color;
                                fill_c.a = 0.16;
                                window.paint_path(path, fill_c);
                            }
                        }
                        let mut stroke = PathBuilder::stroke(px(thick));
                        if dash {
                            stroke = stroke.dash_array(&[px(5.), px(4.)]);
                        }
                        stroke.move_to(p0);
                        stroke.line_to(p1);
                        stroke.line_to(p2);
                        stroke.line_to(p3);
                        stroke.close();
                        if let Ok(path) = stroke.build() {
                            window.paint_path(path, color);
                        }
                    };
                    if show_canvas {
                        paint_rect(0.0, 0.0, cw - 1.0, ch - 1.0, rgb(0xf59e0b), 1.6, true, 1.0, false);
                    }
                    let has_path = sel_loops
                        .as_ref()
                        .is_some_and(|ls| ls.iter().any(|p| p.len() >= 3))
                        || sel_outline.as_ref().is_some_and(|p| p.len() >= 3);
                    let sel_box = if let Some((x0, y0, x1, y1)) = drag_sel {
                        Some((x0, y0, x1, y1))
                    } else if has_path {
                        None
                    } else {
                        sel.bounds(cw as u32, ch as u32).map(|(x0, y0, x1, y1)| {
                            (x0 as f32, y0 as f32, x1 as f32, y1 as f32)
                        })
                    };
                    let handle_box = if show_sel_handles {
                        if let Some((x0, y0, x1, y1)) = sel_box {
                            if drag_sel.is_none() {
                                Some((x0, y0, x1, y1))
                            } else {
                                None
                            }
                        } else if has_path {
                            sel.bounds(cw as u32, ch as u32).map(|(x0, y0, x1, y1)| {
                                (x0 as f32, y0 as f32, x1 as f32, y1 as f32)
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some((x0, y0, x1, y1)) = sel_box {
                        if drag_sel.is_some() {
                            paint_rect(x0, y0, x1, y1, rgb(0xf97316), 1.6, true, 1.0, true);
                        } else {
                            let (sx0, sy0) = xform.image_to_screen(x0.min(x1), y0.min(y1));
                            let (sx1, sy1) =
                                xform.image_to_screen(x0.max(x1) + 1.0, y0.max(y1) + 1.0);
                            let left = ox + sx0.min(sx1);
                            let right = ox + sx0.max(sx1);
                            let top = oy + sy0.min(sy1);
                            let bot = oy + sy0.max(sy1);
                            let mut fill_c = rgb(0xf97316);
                            fill_c.a = 0.16;
                            window.paint_quad(quad(
                                Bounds {
                                    origin: point(px(left), px(top)),
                                    size: size(px((right - left).max(1.0)), px((bot - top).max(1.0))),
                                },
                                px(0.),
                                fill_c,
                                px(0.),
                                fill_c,
                                Default::default(),
                            ));
                            let t = 1.6;
                            let edge = rgb(0xf97316);
                            let edges = [
                                (left, top, (right - left).max(1.0), t),
                                (left, (bot - t).max(top), (right - left).max(1.0), t),
                                (left, top, t, (bot - top).max(1.0)),
                                ((right - t).max(left), top, t, (bot - top).max(1.0)),
                            ];
                            for (ex, ey, ew, eh) in edges {
                                window.paint_quad(quad(
                                    Bounds {
                                        origin: point(px(ex), px(ey)),
                                        size: size(px(ew), px(eh)),
                                    },
                                    px(0.),
                                    edge,
                                    px(0.),
                                    edge,
                                    Default::default(),
                                ));
                            }
                        }
                    }
                    if let Some((x0, y0, x1, y1)) = handle_box {
                        let (sx0, sy0) = xform.image_to_screen(x0.min(x1), y0.min(y1));
                        let (sx1, sy1) = xform.image_to_screen(x0.max(x1) + 1.0, y0.max(y1) + 1.0);
                        let hs = 4.0;
                        let spots = [
                            (ox + sx0, oy + sy0),
                            (ox + sx1, oy + sy0),
                            (ox + sx0, oy + sy1),
                            (ox + sx1, oy + sy1),
                            (ox + (sx0 + sx1) * 0.5, oy + sy0),
                            (ox + (sx0 + sx1) * 0.5, oy + sy1),
                            (ox + sx0, oy + (sy0 + sy1) * 0.5),
                            (ox + sx1, oy + (sy0 + sy1) * 0.5),
                        ];
                        for (hx, hy) in spots {
                            window.paint_quad(quad(
                                Bounds {
                                    origin: point(px(hx - hs), px(hy - hs)),
                                    size: size(px(hs * 2.0), px(hs * 2.0)),
                                },
                                px(0.),
                                rgb(0xf8fafc),
                                px(1.2),
                                rgb(0xf97316),
                                Default::default(),
                            ));
                        }
                    }
                    {
                        let to_screen = |ix: f32, iy: f32| {
                            let (sx, sy) = xform.image_to_screen(ix, iy);
                            point(px(ox + sx), px(oy + sy))
                        };
                        let mut stroke = PathBuilder::stroke(px(1.6));
                        stroke = stroke.dash_array(&[px(5.), px(3.)]);
                        let mut any = false;
                        let mut emit = |stroke: &mut PathBuilder, pts: &[(f32, f32)]| {
                            if pts.len() < 3 {
                                return;
                            }
                            any = true;
                            stroke.move_to(to_screen(pts[0].0, pts[0].1));
                            for &(ix, iy) in pts.iter().skip(1) {
                                stroke.line_to(to_screen(ix, iy));
                            }
                            stroke.close();
                        };
                        if let Some(loops) = sel_loops.as_ref() {
                            for pts in loops {
                                emit(&mut stroke, pts);
                            }
                        }
                        if let Some(pts) = sel_outline.as_ref() {
                            emit(&mut stroke, pts);
                        }
                        drop(emit);
                        if any {
                            if let Ok(path) = stroke.build() {
                                window.paint_path(path, rgb(0xf97316));
                            }
                        }
                    }
                    if let Some(ref draft) = lasso_draft {
                        if !draft.is_empty() {
                            let to_screen = |ix: f32, iy: f32| {
                                let (sx, sy) = xform.image_to_screen(ix, iy);
                                point(px(ox + sx), px(oy + sy))
                            };
                            let mut stroke = PathBuilder::stroke(px(1.5));
                            stroke = stroke.dash_array(&[px(5.), px(3.)]);
                            let first = to_screen(draft[0].0, draft[0].1);
                            stroke.move_to(first);
                            for &(ix, iy) in draft.iter().skip(1) {
                                stroke.line_to(to_screen(ix, iy));
                            }
                            if let Some((cx_i, cy_i)) = lasso_cursor {
                                stroke.line_to(to_screen(cx_i, cy_i));
                            }
                            if let Ok(path) = stroke.build() {
                                window.paint_path(path, rgb(0x38bdf8));
                            }
                            let can_snap = draft.len() >= 3
                                && lasso_cursor
                                    .map(|(cx_i, cy_i)| {
                                        (cx_i - draft[0].0).abs() < 0.01
                                            && (cy_i - draft[0].1).abs() < 0.01
                                    })
                                    .unwrap_or(false);
                            let p = to_screen(draft[0].0, draft[0].1);
                            let sz = if can_snap { 10.0 } else { 6.0 };
                            window.paint_quad(quad(
                                Bounds {
                                    origin: point(p.x - px(sz * 0.5), p.y - px(sz * 0.5)),
                                    size: size(px(sz), px(sz)),
                                },
                                px(1.),
                                rgb(0xf97316),
                                px(1.),
                                rgb(0xffffff),
                                Default::default(),
                            ));
                        }
                    }
                    if show_canvas {
                        let hot = canvas_hot.map(|(e, _, _)| e);
                        for edge in [
                            CanvasEdge::Top,
                            CanvasEdge::Bottom,
                            CanvasEdge::Left,
                            CanvasEdge::Right,
                        ] {
                            let (a, b, c, d) = match edge {
                                CanvasEdge::Top => (0.0, 0.0, cw - 1.0, 0.0),
                                CanvasEdge::Bottom => (0.0, ch - 1.0, cw - 1.0, ch - 1.0),
                                CanvasEdge::Left => (0.0, 0.0, 0.0, ch - 1.0),
                                CanvasEdge::Right => (cw - 1.0, 0.0, cw - 1.0, ch - 1.0),
                            };
                            let on = hot == Some(edge);
                            let mut stroke = PathBuilder::stroke(px(if on { 4.0 } else { 2.4 }));
                            let (sx0, sy0) = xform.image_to_screen(a, b);
                            let (sx1, sy1) = xform.image_to_screen(c + 1.0, d + 1.0);
                            stroke.move_to(point(px(ox + sx0), px(oy + sy0)));
                            stroke.line_to(point(px(ox + sx1), px(oy + sy1)));
                            if let Ok(path) = stroke.build() {
                                window.paint_path(path, if on { rgb(0x38bdf8) } else { rgb(0xf59e0b) });
                            }
                        }
                    }
                    if show_handles {
                        for layer in &layers {
                            if !layer.active {
                                continue;
                            }
                            let corners = crate::geom::rotated_corners(
                                layer.x as f32,
                                layer.y as f32,
                                layer.w as f32,
                                layer.h as f32,
                                layer.rotate_deg,
                            );
                            let screen: [(f32, f32); 4] = corners.map(|(ix, iy)| {
                                let (sx, sy) = xform.image_to_screen(ix, iy);
                                (ox + sx, oy + sy)
                            });
                            let mids = [
                                (
                                    (screen[0].0 + screen[1].0) * 0.5,
                                    (screen[0].1 + screen[1].1) * 0.5,
                                ),
                                (
                                    (screen[1].0 + screen[2].0) * 0.5,
                                    (screen[1].1 + screen[2].1) * 0.5,
                                ),
                                (
                                    (screen[2].0 + screen[3].0) * 0.5,
                                    (screen[2].1 + screen[3].1) * 0.5,
                                ),
                                (
                                    (screen[3].0 + screen[0].0) * 0.5,
                                    (screen[3].1 + screen[0].1) * 0.5,
                                ),
                            ];
                            let hs = 4.0;
                            for (hx, hy) in screen.iter().chain(mids.iter()).copied() {
                                window.paint_quad(quad(
                                    Bounds {
                                        origin: point(
                                            px(hx - hs),
                                            px(hy - hs),
                                        ),
                                        size: size(px(hs * 2.0), px(hs * 2.0)),
                                    },
                                    px(0.),
                                    rgb(0xf8fafc),
                                    px(1.2),
                                    rgb(0x0f172a),
                                    Default::default(),
                                ));
                            }
                        }
                    }
                    if let Some(((bx, by), style)) = brush_cursor.zip(brush_ring) {
                        let screen_r = (brush_size * 0.5 * xform.scale).max(1.5);
                        let (sx, sy) = xform.image_to_screen(bx, by);
                        paint_brush_ring(window, ox + sx, oy + sy, screen_r, style, paint_fill);
                    }
                    if show_stamp {
                        let screen_r = (brush_size * 0.5 * xform.scale).max(8.0);
                        if let Some(style) = stamp_ring {
                            if let Some((bx, by)) = stamp_img {
                                let (sx, sy) = xform.image_to_screen(bx, by);
                                paint_stamp_cursor(window, ox + sx, oy + sy, screen_r, style);
                            } else if let Some((pxv, pyv)) = pointer {
                                paint_stamp_cursor(window, ox + pxv, oy + pyv, screen_r, style);
                            }
                        }
                    }
                    if let Some(((mx, my), style)) = clone_mark.zip(cross_ring) {
                        let half = (brush_size * 0.5 * xform.scale).max(2.0);
                        let (sx, sy) = xform.image_to_screen(mx, my);
                        paint_source_cross(window, ox + sx, oy + sy, half, style);
                    }
                    if show_rotate_cur {
                        if let Some((pxv, pyv)) = pointer {
                            paint_rotate_cursor(window, ox + pxv, oy + pyv);
                        }
                    }
                },
            )
            .size_full())
    }

    fn hit_sel_handle(&self, ix: f32, iy: f32) -> Option<BoxHandle> {
        let (cw, ch) = self.doc.as_ref().map(|d| (d.canvas_w, d.canvas_h))?;
        let (x0, y0, x1, y1) = self.selection.bounds(cw, ch)?;
        box_handle(
            ix,
            iy,
            x0 as f32,
            y0 as f32,
            x1 as f32 + 1.0,
            y1 as f32 + 1.0,
            self.view_xform().scale,
        )
    }

    fn sample_ring_under(&self, cx: f32, cy: f32, r: f32) -> Vec<[u8; 3]> {
        sample_ring_points(cx, cy, r)
            .map(|(x, y)| self.sample_under(x, y))
            .collect()
    }

    fn sample_under(&self, ix: f32, iy: f32) -> [u8; 3] {
        let Some(doc) = self.doc.as_ref() else {
            return [226, 232, 240];
        };
        for (i, layer) in doc.layers.iter().enumerate().rev() {
            if !layer.visible {
                continue;
            }
            let extra = if i == doc.active {
                self.extra_rotation()
            } else {
                0.0
            };
            let deg = layer.rotation + extra;
            let (lx, ly) = if deg.abs() > 0.05 {
                let (cx, cy) = layer.center();
                crate::geom::rotate_point(ix, iy, cx, cy, -deg)
            } else {
                (ix, iy)
            };
            let px = lx.floor() as i32;
            let py = ly.floor() as i32;
            let x1 = layer.x + layer.width() as i32;
            let y1 = layer.y + layer.height() as i32;
            if px < layer.x || py < layer.y || px >= x1 || py >= y1 {
                continue;
            }
            let p = layer
                .pixels
                .get_pixel((px - layer.x) as u32, (py - layer.y) as u32);
            if p[3] > 8 {
                return [p[0], p[1], p[2]];
            }
        }
        doc.paper_rgb
    }

    fn extra_rotation(&self) -> f32 {
        match &self.drag {
            Some(DragKind::LayerRotate { last_deg, .. }) => *last_deg,
            _ => 0.0,
        }
    }

    fn pointer_image(&self) -> Option<(f32, f32)> {
        let (vx, vy) = self.last_pointer_view?;
        Some(self.view_xform().screen_to_image(vx, vy))
    }

    pub(crate) fn apply_live_pointer(&mut self, cx: &mut Context<Self>) {
        let Some((ix, iy)) = self.pointer_image() else {
            return;
        };
        if matches!(self.drag, Some(DragKind::LayerRotate { .. })) {
            self.apply_live_rotate(ix, iy);
            self.notify_nav(cx);
            return;
        }
        if let Some(DragKind::LayerScale {
            handle,
            start,
            orig_x,
            orig_y,
            orig_w,
            orig_h,
            ..
        }) = &self.drag
        {
            let (nx, ny, nw, nh) = self.apply_layer_scale_live(
                *handle, *start, *orig_x, *orig_y, *orig_w, *orig_h, ix, iy,
            );
            if let Some(DragKind::LayerScale {
                cur_x,
                cur_y,
                cur_w,
                cur_h,
                ..
            }) = &mut self.drag
            {
                *cur_x = nx;
                *cur_y = ny;
                *cur_w = nw;
                *cur_h = nh;
            }
            self.notify_nav(cx);
        }
    }

    fn apply_live_rotate(&mut self, ix: f32, iy: f32) {
        let Some(DragKind::LayerRotate {
            last_ang,
            accum_deg,
            cx: lcx,
            cy: lcy,
            ..
        }) = &self.drag
        else {
            return;
        };
        let last_ang = *last_ang;
        let mut accum = *accum_deg;
        let lcx = *lcx;
        let lcy = *lcy;
        let ang = (iy - lcy).atan2(ix - lcx);
        accum += shortest_arc_rad(last_ang, ang).to_degrees();
        let base = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer())
            .map(|l| l.rotation)
            .unwrap_or(0.0);
        let extra = if self.last_shift {
            snap_deg(base + accum, 15.0) - base
        } else {
            accum
        };
        if let Some(DragKind::LayerRotate {
            last_ang,
            accum_deg,
            last_deg,
            ..
        }) = &mut self.drag
        {
            *last_ang = ang;
            *accum_deg = accum;
            *last_deg = extra;
        }
    }

    fn active_layer_rect(&self) -> Option<(f32, f32, f32, f32, f32)> {
        let l = self.doc.as_ref()?.active_layer()?;
        let extra = self.extra_rotation();
        let deg = l.rotation + extra;
        if let Some(DragKind::LayerScale {
            cur_x,
            cur_y,
            cur_w,
            cur_h,
            ..
        }) = &self.drag
        {
            return Some((
                *cur_x as f32,
                *cur_y as f32,
                *cur_w as f32,
                *cur_h as f32,
                deg,
            ));
        }
        Some((
            l.x as f32,
            l.y as f32,
            l.width() as f32,
            l.height() as f32,
            deg,
        ))
    }

    fn hit_layer_handle(&self, ix: f32, iy: f32) -> Option<BoxHandle> {
        let (x, y, w, h, deg) = self.active_layer_rect()?;
        let (lx, ly) = if deg.abs() > 0.05 {
            let cx = x + w * 0.5;
            let cy = y + h * 0.5;
            crate::geom::rotate_point(ix, iy, cx, cy, -deg)
        } else {
            (ix, iy)
        };
        box_handle(lx, ly, x, y, x + w, y + h, self.view_xform().scale)
    }

    fn hit_layer_rotate(&self, ix: f32, iy: f32) -> Option<f32> {
        let (x, y, w, h, deg) = self.active_layer_rect()?;
        let (lx, ly) = if deg.abs() > 0.05 {
            let cx = x + w * 0.5;
            let cy = y + h * 0.5;
            crate::geom::rotate_point(ix, iy, cx, cy, -deg)
        } else {
            (ix, iy)
        };
        box_rotate_hit(lx, ly, x, y, x + w, y + h, self.view_xform().scale)
    }

    pub(crate) fn hit_layer_at(&self, ix: f32, iy: f32) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        for (i, layer) in doc.layers.iter().enumerate().rev() {
            if !layer.visible {
                continue;
            }
            let extra = if i == doc.active {
                self.extra_rotation()
            } else {
                0.0
            };
            let deg = layer.rotation + extra;
            let (lx, ly) = if deg.abs() > 0.05 {
                let (cx, cy) = layer.center();
                crate::geom::rotate_point(ix, iy, cx, cy, -deg)
            } else {
                (ix, iy)
            };
            let px = lx.floor() as i32;
            let py = ly.floor() as i32;
            let x1 = layer.x + layer.width() as i32;
            let y1 = layer.y + layer.height() as i32;
            if px < layer.x || py < layer.y || px >= x1 || py >= y1 {
                continue;
            }
            let sx = (px - layer.x) as u32;
            let sy = (py - layer.y) as u32;
            if layer.pixels.get_pixel(sx, sy)[3] > 8 {
                return Some(i);
            }
        }
        None
    }

    pub(crate) fn view_xform(&self) -> ViewXform {
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        match &self.drag {
            Some(DragKind::Canvas {
                orig_w,
                orig_h,
                orig_pan,
                ..
            }) => ViewXform::compute(
                *orig_w as f32,
                *orig_h as f32,
                vw,
                vh,
                self.zoom,
                *orig_pan,
                true,
            ),
            _ => {
                let (cw, ch) = self
                    .doc
                    .as_ref()
                    .map(|d| (d.canvas_w as f32, d.canvas_h as f32))
                    .unwrap_or((1.0, 1.0));
                ViewXform::compute(cw, ch, vw, vh, self.zoom, self.pan, self.user_zoomed)
            }
        }
    }

    fn live_cursor(&self) -> CursorStyle {
        match &self.drag {
            Some(DragKind::SelResize { handle, .. } | DragKind::LayerScale { handle, .. }) => {
                handle.cursor()
            }
            Some(DragKind::Canvas { edge, .. }) => match edge {
                CanvasEdge::Left | CanvasEdge::Right => CursorStyle::ResizeLeftRight,
                CanvasEdge::Top | CanvasEdge::Bottom => CursorStyle::ResizeUpDown,
            },
            Some(DragKind::Move { .. }) => CursorStyle::PointingHand,
            Some(DragKind::LayerRotate { .. }) => CursorStyle::None,
            Some(DragKind::Pan { .. }) => CursorStyle::ClosedHand,
            Some(DragKind::Select { .. } | DragKind::LassoStroke { .. }) => CursorStyle::Crosshair,
            Some(DragKind::Brush | DragKind::Erase { .. }) => CursorStyle::None,
            Some(
                DragKind::Slider(_)
                | DragKind::LayerReorder { .. }
                | DragKind::PaletteSb
                | DragKind::PaletteHue,
            ) => self.hover_cursor,
            None => self.hover_cursor,
        }
    }

    fn refresh_hover_cursor(&mut self, ix: f32, iy: f32) {
        let scale = self.view_xform().scale;
        if self.mode == ToolMode::Canvas {
            self.hover_rotate = false;
            if let Some(doc) = self.doc.as_ref() {
                let edge = canvas_edge(ix, iy, doc.canvas_w, doc.canvas_h, scale);
                self.canvas_hover = edge;
                self.hover_cursor = match edge {
                    Some(CanvasEdge::Left | CanvasEdge::Right) => CursorStyle::ResizeLeftRight,
                    Some(CanvasEdge::Top | CanvasEdge::Bottom) => CursorStyle::ResizeUpDown,
                    None => CursorStyle::Arrow,
                };
            }
            return;
        }
        self.canvas_hover = None;
        self.hover_rotate = false;
        self.hover_cursor = if matches!(
            self.mode,
            ToolMode::Select | ToolMode::Wand | ToolMode::Lasso
        ) {
            self.hit_sel_handle(ix, iy)
                .map(BoxHandle::cursor)
                .unwrap_or(CursorStyle::Crosshair)
        } else if self.mode == ToolMode::Move {
            if let Some(h) = self.hit_layer_handle(ix, iy) {
                h.cursor()
            } else if self.hit_layer_rotate(ix, iy).is_some() {
                self.hover_rotate = true;
                CursorStyle::None
            } else {
                CursorStyle::PointingHand
            }
        } else if self.mode == ToolMode::Pan {
            CursorStyle::OpenHand
        } else if self.mode.uses_brush() {
            CursorStyle::None
        } else {
            CursorStyle::Arrow
        };
    }

    fn image_pos(&self, ev: &MouseDownEvent) -> (f32, f32) {
        let x = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
        let y = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
        self.view_xform().screen_to_image(x, y)
    }

    fn image_pos_move(&self, ev: &MouseMoveEvent) -> (f32, f32) {
        let x = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
        let y = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
        self.view_xform().screen_to_image(x, y)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_shift = ev.modifiers.shift;
        self.clone_alt = ev.modifiers.alt;
        let vx = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
        let vy = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
        self.last_pointer_view = Some((vx, vy));
        let (ix, iy) = self.image_pos(ev);
        if self.eyedropper_armed {
            self.confirm_eyedropper_at(ix, iy, cx);
            return;
        }
        if self.mode.uses_brush() {
            self.brush_cursor = Some((ix, iy));
        }
        match self.mode {
            ToolMode::Pan => {
                self.drag = Some(DragKind::Pan {
                    last_x: f32::from(ev.position.x),
                    last_y: f32::from(ev.position.y),
                });
            }
            ToolMode::Select | ToolMode::Wand | ToolMode::Lasso => {
                if let Some(handle) = self.hit_sel_handle(ix, iy) {
                    let (cw, ch) = self
                        .doc
                        .as_ref()
                        .map(|d| (d.canvas_w, d.canvas_h))
                        .unwrap_or((1, 1));
                    if let Some(orig) = self.selection.bounds(cw, ch) {
                        let orig_mask = match &self.selection {
                            Selection::Mask { data, .. } => Some(data.clone()),
                            _ => None,
                        };
                        self.drag = Some(DragKind::SelResize {
                            handle,
                            start: (ix, iy),
                            orig,
                            orig_mask,
                            cur: orig,
                        });
                    }
                } else if self.mode == ToolMode::Select {
                    self.drag = Some(DragKind::Select {
                        x0: ix,
                        y0: iy,
                        x1: ix,
                        y1: iy,
                    });
                } else if self.mode == ToolMode::Lasso {
                    self.begin_lasso_at(ix, iy, cx);
                } else {
                    self.wand_at(ix.round() as i32, iy.round() as i32, cx);
                }
            }
            ToolMode::Move => {
                if let Some(handle) = self.hit_layer_handle(ix, iy) {
                    self.push_undo();
                    let geom = self
                        .doc
                        .as_ref()
                        .and_then(|d| d.active_layer())
                        .map(|l| (l.x, l.y, l.width(), l.height()));
                    if let Some((ox, oy, ow, oh)) = geom {
                        self.drag = Some(DragKind::LayerScale {
                            handle,
                            start: (ix, iy),
                            orig_x: ox,
                            orig_y: oy,
                            orig_w: ow,
                            orig_h: oh,
                            cur_x: ox,
                            cur_y: oy,
                            cur_w: ow,
                            cur_h: oh,
                        });
                    }
                } else if self.hit_layer_rotate(ix, iy).is_some() {
                    let snap = self.doc.as_ref().and_then(|d| d.active_layer()).map(|l| {
                        let (lcx, lcy) = l.center();
                        (lcx, lcy)
                    });
                    if let Some((lcx, lcy)) = snap {
                        self.push_undo();
                        let start_ang = (iy - lcy).atan2(ix - lcx);
                        self.drag = Some(DragKind::LayerRotate {
                            last_ang: start_ang,
                            accum_deg: 0.0,
                            cx: lcx,
                            cy: lcy,
                            last_deg: 0.0,
                        });
                    }
                } else if let Some(i) = self.hit_layer_at(ix, iy) {
                    if let Some(doc) = self.doc.as_mut() {
                        doc.set_active(i);
                    }
                    let pos = self
                        .doc
                        .as_ref()
                        .and_then(|d| d.active_layer())
                        .map(|l| (l.x, l.y));
                    if let Some((ox, oy)) = pos {
                        self.push_undo();
                        self.drag = Some(DragKind::Move {
                            start_x: ix.round() as i32,
                            start_y: iy.round() as i32,
                            orig_x: ox,
                            orig_y: oy,
                        });
                    }
                }
            }
            ToolMode::Canvas => {
                let info = self.doc.as_ref().map(|d| {
                    (
                        d.canvas_w,
                        d.canvas_h,
                        d.layers.iter().map(|l| (l.x, l.y)).collect::<Vec<_>>(),
                    )
                });
                if let Some((cw, ch, orig_pos)) = info {
                    let edge = canvas_edge(ix, iy, cw, ch, self.view_xform().scale);
                    if let Some(edge) = edge {
                        self.push_undo();
                        self.user_zoomed = true;
                        self.drag = Some(DragKind::Canvas {
                            edge,
                            start: (ix, iy),
                            orig_w: cw,
                            orig_h: ch,
                            orig_pos,
                            orig_pan: self.pan,
                        });
                    }
                }
            }
            ToolMode::Eraser => {
                self.status = "".into();
                self.drag = Some(DragKind::Erase {
                    start_ix: ix,
                    start_iy: iy,
                    undid: false,
                    wiping: false,
                    hit: false,
                });
            }
            ToolMode::Heal | ToolMode::Clone | ToolMode::Paint => {
                if self.mode == ToolMode::Clone && self.clone_alt {
                    self.clone_src = Some((ix.round() as i32, iy.round() as i32));
                    self.clone_offset = None;
                    self.status = "已取样, 涂抹时源点跟着走".into();
                    self.notify_chrome(cx);
                    return;
                }
                if self.mode == ToolMode::Paint && self.clone_alt {
                    self.sample_paint_color(ix.round() as i32, iy.round() as i32, cx);
                    return;
                }
                if self.mode == ToolMode::Clone && self.clone_src.is_none() {
                    self.status = "先按住 Alt 点一下取样".into();
                    self.notify_chrome(cx);
                    return;
                }
                self.drag = Some(DragKind::Brush);
                self.begin_brush_stroke(ix.round() as i32, iy.round() as i32);
            }
        }
        self.notify_nav(cx);
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_shift = ev.modifiers.shift;
        let alt = ev.modifiers.alt;
        let alt_changed = self.clone_alt != alt;
        self.clone_alt = alt;
        let vx = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
        let vy = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
        self.last_pointer_view = Some((vx, vy));
        if let Some(DragKind::Slider(kind)) = self.drag {
            self.set_slider_from_x(f32::from(ev.position.x), kind, cx);
            return;
        }
        if matches!(self.drag, Some(DragKind::PaletteSb)) {
            self.set_palette_sb_from_pos(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            return;
        }
        if matches!(self.drag, Some(DragKind::PaletteHue)) {
            self.set_palette_hue_from_y(f32::from(ev.position.y), cx);
            return;
        }
        if matches!(self.drag, Some(DragKind::LayerReorder { .. })) {
            self.update_layer_reorder(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            return;
        }
        let (ix, iy) = self.image_pos_move(ev);
        let snap_tol = (EDGE_SNAP_SCREEN_PX / self.view_xform().scale.max(0.0001)).max(1.0);
        if self.eyedropper_armed {
            if let Some(rgb) = self.sample_canvas_rgb(ix.round() as i32, iy.round() as i32) {
                self.preview_eyedropper_rgb(rgb, cx);
            }
            return;
        }
        if self.mode.uses_brush() {
            self.brush_cursor = Some((ix, iy));
        }
        if matches!(self.drag, Some(DragKind::LassoStroke { .. })) {
            self.update_lasso_stroke(ix, iy, vx, vy, cx);
            return;
        }
        if let Some(DragKind::SelResize {
            handle,
            start,
            orig,
            ..
        }) = &self.drag
        {
            let handle = *handle;
            let start = *start;
            let orig = *orig;
            let cur = self.apply_sel_resize(handle, orig, start, ix, iy);
            if let Some(DragKind::SelResize { cur: slot, .. }) = &mut self.drag {
                *slot = cur;
            }
            self.notify_nav(cx);
            return;
        }
        if let Some(DragKind::LayerScale {
            handle,
            start,
            orig_x,
            orig_y,
            orig_w,
            orig_h,
            ..
        }) = &self.drag
        {
            let (nx, ny, nw, nh) = self.apply_layer_scale_live(
                *handle, *start, *orig_x, *orig_y, *orig_w, *orig_h, ix, iy,
            );
            if let Some(DragKind::LayerScale {
                cur_x,
                cur_y,
                cur_w,
                cur_h,
                ..
            }) = &mut self.drag
            {
                *cur_x = nx;
                *cur_y = ny;
                *cur_w = nw;
                *cur_h = nh;
            }
            self.notify_nav(cx);
            return;
        }
        if matches!(self.drag, Some(DragKind::LayerRotate { .. })) {
            self.apply_live_rotate(ix, iy);
            self.notify_nav(cx);
            return;
        }
        if matches!(self.drag, Some(DragKind::Erase { .. })) {
            self.apply_erase_move(ix, iy, cx);
            return;
        }
        match &mut self.drag {
            Some(
                DragKind::SelResize { .. }
                | DragKind::LayerScale { .. }
                | DragKind::LayerRotate { .. },
            ) => {}
            Some(DragKind::Select { x0, y0, x1, y1 }) => {
                let (cw, ch) = self
                    .doc
                    .as_ref()
                    .map(|d| (d.canvas_w as f32, d.canvas_h as f32))
                    .unwrap_or((1.0, 1.0));
                let xs = [0.0, (cw - 1.0).max(0.0), *x0];
                let ys = [0.0, (ch - 1.0).max(0.0), *y0];
                *x1 = snap_to_targets(ix, &xs, snap_tol);
                *y1 = snap_to_targets(iy, &ys, snap_tol);
                self.notify_nav(cx);
            }
            Some(DragKind::Move {
                start_x,
                start_y,
                orig_x,
                orig_y,
            }) => {
                let mut dx = ix.round() as i32 - *start_x;
                let mut dy = iy.round() as i32 - *start_y;
                if self.last_shift {
                    (dx, dy) = axis_lock_delta(dx, dy);
                }
                let nx = *orig_x + dx;
                let ny = *orig_y + dy;
                if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
                    layer.x = nx;
                    layer.y = ny;
                }
                self.dirty = true;
                self.notify_nav(cx);
            }
            Some(DragKind::Canvas {
                edge,
                start,
                orig_w,
                orig_h,
                orig_pos,
                ..
            }) => {
                let (sx, sy) = *start;
                let orig_w = *orig_w;
                let orig_h = *orig_h;
                let edge = *edge;
                let orig_pos = orig_pos.clone();
                let content = layers_aabb_at(self.doc.as_ref(), &orig_pos);
                let (w, h, ox, oy) =
                    snap_canvas_resize(edge, orig_w, orig_h, (sx, sy), ix, iy, content, snap_tol);
                if let Some(doc) = self.doc.as_mut() {
                    doc.resize_canvas_from(w as u32, h as u32, &orig_pos, ox, oy);
                }
                self.dirty = true;
                self.notify_nav(cx);
            }
            Some(DragKind::Brush) => {
                self.paint_brush_segment(ix, iy, false);
                self.notify_nav(cx);
            }
            Some(DragKind::Erase { .. }) => {}
            Some(DragKind::Slider(_)) => {}
            Some(DragKind::PaletteSb | DragKind::PaletteHue) => {}
            Some(DragKind::LassoStroke { .. }) => {}
            Some(DragKind::LayerReorder { .. }) => {}
            Some(DragKind::Pan { last_x, last_y }) => {
                let nx = f32::from(ev.position.x);
                let ny = f32::from(ev.position.y);
                let dx = nx - *last_x;
                let dy = ny - *last_y;
                *last_x = nx;
                *last_y = ny;
                self.user_zoomed = true;
                self.pan.x += dx;
                self.pan.y += dy;
                self.notify_nav(cx);
            }
            None => {
                let prev = self.hover_cursor;
                let prev_rot = self.hover_rotate;
                if self.mode == ToolMode::Lasso && self.lasso_draft.is_some() {
                    let (sx, sy, _) = self.lasso_maybe_snap(ix, iy);
                    self.lasso_cursor = Some((sx, sy));
                    self.notify_nav(cx);
                    return;
                }
                self.refresh_hover_cursor(ix, iy);
                if self.hover_cursor != prev
                    || self.hover_rotate
                    || prev_rot
                    || alt_changed
                    || self.mode == ToolMode::Canvas
                    || self.mode.uses_brush()
                {
                    self.notify_nav(cx);
                }
            }
        }
    }

    fn bake_zoom_after_canvas_resize(
        &mut self,
        orig_w: u32,
        orig_h: u32,
        orig_pan: Point<f32>,
        ox: i32,
        oy: i32,
    ) {
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w as f32, d.canvas_h as f32))
            .unwrap_or((1.0, 1.0));
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        let drag_scale = ViewXform::compute(
            orig_w as f32,
            orig_h as f32,
            vw,
            vh,
            self.zoom,
            orig_pan,
            true,
        )
        .scale;
        let new_fit = if cw > 0.0 && ch > 0.0 && vw > 1.0 && vh > 1.0 {
            (vw / cw).min(vh / ch).max(0.0001)
        } else {
            1.0
        };
        self.user_zoomed = true;
        self.zoom = (drag_scale / new_fit).clamp(0.05, 40.0);
        self.pan.x = orig_pan.x + ox as f32 * drag_scale + (cw - orig_w as f32) * drag_scale * 0.5;
        self.pan.y = orig_pan.y + oy as f32 * drag_scale + (ch - orig_h as f32) * drag_scale * 0.5;
    }

    fn apply_erase_move(&mut self, ix: f32, iy: f32, cx: &mut Context<Self>) {
        let Some(DragKind::Erase {
            start_ix,
            start_iy,
            mut undid,
            mut wiping,
            mut hit,
        }) = self.drag.take()
        else {
            return;
        };
        if !wiping {
            let dx = ix - start_ix;
            let dy = iy - start_iy;
            if dx * dx + dy * dy >= ERASE_DRAG_SLOP_IMG * ERASE_DRAG_SLOP_IMG {
                wiping = true;
                if !undid {
                    let before = self.undo_stack.len();
                    self.push_undo();
                    undid = self.undo_stack.len() > before;
                }
                self.brush_last = None;
                if self.erase_all_segment(start_ix, start_iy, true) {
                    hit = true;
                }
                if self.erase_all_segment(ix, iy, false) {
                    hit = true;
                }
            }
        } else if self.erase_all_segment(ix, iy, false) {
            hit = true;
        }
        self.drag = Some(DragKind::Erase {
            start_ix,
            start_iy,
            undid,
            wiping,
            hit,
        });
        self.notify_nav(cx);
    }

    fn finish_erase(&mut self, start_ix: f32, start_iy: f32, undid: bool, wiping: bool, hit: bool) {
        self.brush_last = None;
        if !wiping {
            let before = self.undo_stack.len();
            self.push_undo();
            let pushed = self.undo_stack.len() > before;
            self.status = "".into();
            if self.erase_top_at(start_ix, start_iy) {
                self.status = self.erase_hit_status().into();
            } else {
                if pushed {
                    self.undo_stack.pop();
                }
                if self.status.is_empty() {
                    self.status = "未点到可擦除内容".into();
                }
            }
        } else if !hit {
            if undid {
                self.undo_stack.pop();
            }
            if self.status.is_empty() {
                self.status = "未擦到内容".into();
            }
        } else {
            self.status = self.erase_hit_status().into();
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let kind = self.drag.take();
        let rotate_only = matches!(&kind, Some(DragKind::LayerRotate { .. }));
        match kind {
            Some(DragKind::LayerReorder {
                from, to, armed, ..
            }) => {
                self.commit_layer_reorder(from, to, armed, cx);
                return;
            }
            Some(DragKind::Select { x0, y0, x1, y1 }) => {
                let dx = (x1 - x0).abs();
                let dy = (y1 - y0).abs();
                let scale = self.view_xform().scale;
                if dx * scale < 4.0 && dy * scale < 4.0 {
                    // 单击不覆盖已有选区
                } else {
                    self.selection = Selection::Rect {
                        x0: x0.round() as i32,
                        y0: y0.round() as i32,
                        x1: x1.round() as i32,
                        y1: y1.round() as i32,
                    };
                    self.status = "已框选".into();
                }
            }
            Some(DragKind::LassoStroke { freehand, .. }) => {
                if freehand {
                    self.finalize_lasso(cx);
                }
            }
            Some(DragKind::PaletteSb | DragKind::PaletteHue) => {}
            Some(DragKind::SelResize {
                orig,
                orig_mask,
                cur,
                ..
            }) => {
                self.commit_sel_resize(orig, orig_mask, cur);
            }
            Some(DragKind::LayerScale {
                orig_x,
                orig_y,
                orig_w,
                orig_h,
                cur_x,
                cur_y,
                cur_w,
                cur_h,
                ..
            }) => {
                if cur_x == orig_x && cur_y == orig_y && cur_w == orig_w && cur_h == orig_h {
                    self.undo_stack.pop();
                } else {
                    self.bake_layer_scale(orig_w, orig_h, cur_x, cur_y, cur_w, cur_h);
                }
            }
            Some(DragKind::Brush) => {
                self.drop_empty_paint_stroke();
                self.brush_last = None;
                self.clone_snap = None;
                self.heal_offset = None;
            }
            Some(DragKind::Erase {
                start_ix,
                start_iy,
                undid,
                wiping,
                hit,
            }) => {
                self.finish_erase(start_ix, start_iy, undid, wiping, hit);
            }
            Some(DragKind::Slider(_)) => {}
            Some(DragKind::Canvas {
                orig_w,
                orig_h,
                orig_pos,
                orig_pan,
                ..
            }) => {
                let changed = self
                    .doc
                    .as_ref()
                    .is_some_and(|d| d.canvas_w != orig_w || d.canvas_h != orig_h);
                if !changed {
                    self.undo_stack.pop();
                } else {
                    let (ox, oy) = self
                        .doc
                        .as_ref()
                        .and_then(|d| d.layers.first().zip(orig_pos.first()))
                        .map(|(l, (x, y))| (*x - l.x, *y - l.y))
                        .unwrap_or((0, 0));
                    self.bake_zoom_after_canvas_resize(orig_w, orig_h, orig_pan, ox, oy);
                }
            }
            Some(DragKind::Move { orig_x, orig_y, .. }) => {
                let same = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.active_layer())
                    .is_some_and(|l| l.x == orig_x && l.y == orig_y);
                if same {
                    self.undo_stack.pop();
                }
            }
            Some(DragKind::LayerRotate { last_deg, .. }) => {
                if last_deg.abs() < 0.2 {
                    self.undo_stack.pop();
                } else {
                    self.commit_layer_rotate(last_deg);
                }
            }
            _ => {}
        }
        let x = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
        let y = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
        let (ix, iy) = self.view_xform().screen_to_image(x, y);
        self.refresh_hover_cursor(ix, iy);
        if rotate_only {
            self.notify_nav(cx);
        } else {
            self.notify_chrome(cx);
        }
    }

    pub(crate) fn on_scroll(
        &mut self,
        ev: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (delta_x, delta_y) = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(p) => (p.x * 30.0, p.y * 30.0),
        };
        if ev.modifiers.alt && self.mode.uses_brush() {
            let d = if delta_y.abs() > 0.01 {
                delta_y
            } else {
                delta_x
            };
            self.bump_brush_size(d > 0.0);
            self.notify_chrome(cx);
            cx.stop_propagation();
            return;
        }
        if apply_bg::is_primary_mod(&ev.modifiers) {
            let zoom_delta = if delta_y.abs() > 0.01 {
                delta_y
            } else {
                delta_x
            };
            let sx = f32::from(ev.position.x) - f32::from(self.view_bounds.origin.x);
            let sy = f32::from(ev.position.y) - f32::from(self.view_bounds.origin.y);
            let old = self.view_xform();
            let (ix, iy) = old.screen_to_image(sx, sy);
            let (cw, ch) = self
                .doc
                .as_ref()
                .map(|d| (d.canvas_w as f32, d.canvas_h as f32))
                .unwrap_or((1.0, 1.0));
            let vw = f32::from(self.view_bounds.size.width);
            let vh = f32::from(self.view_bounds.size.height);
            let fit = if cw > 0.0 && ch > 0.0 && vw > 1.0 && vh > 1.0 {
                (vw / cw).min(vh / ch).max(0.0001)
            } else {
                1.0
            };
            let factor = if zoom_delta > 0.0 { 1.15 } else { 1.0 / 1.15 };
            let current_zoom = if self.user_zoomed { self.zoom } else { 1.0 };
            self.user_zoomed = true;
            self.zoom = (current_zoom * factor).clamp(0.05, 40.0);
            let new_scale = fit * self.zoom;
            self.pan.x = sx - (vw - cw * new_scale) * 0.5 - ix * new_scale;
            self.pan.y = sy - (vh - ch * new_scale) * 0.5 - iy * new_scale;
        } else if ev.modifiers.shift {
            let dx = if delta_x.abs() > 0.01 {
                delta_x
            } else {
                delta_y
            };
            self.user_zoomed = true;
            self.pan.x += dx;
        } else {
            self.user_zoomed = true;
            self.pan.x += delta_x;
            self.pan.y += delta_y;
        }
        self.notify_nav(cx);
        cx.stop_propagation();
    }
}

fn layers_aabb_at(
    doc: Option<&crate::document::EditDocument>,
    orig_pos: &[(i32, i32)],
) -> Option<(f32, f32, f32, f32)> {
    let doc = doc?;
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    let mut any = false;
    for (layer, &(x, y)) in doc.layers.iter().zip(orig_pos.iter()) {
        if !layer.visible {
            continue;
        }
        let (a, b, c, d) = crate::geom::rotated_aabb(
            x as f32,
            y as f32,
            layer.width() as f32,
            layer.height() as f32,
            layer.rotation,
        );
        min_x = min_x.min(a);
        min_y = min_y.min(b);
        max_x = max_x.max(c);
        max_y = max_y.max(d);
        any = true;
    }
    if any {
        Some((min_x, min_y, max_x, max_y))
    } else {
        None
    }
}

fn canvas_edge(ix: f32, iy: f32, w: u32, h: u32, scale: f32) -> Option<CanvasEdge> {
    let tol = (10.0 / scale.max(0.0001)).max(3.0);
    let wf = w as f32;
    let hf = h as f32;
    let dl = ix.abs();
    let dr = (ix - wf).abs();
    let dt = iy.abs();
    let db = (iy - hf).abs();
    let in_y = iy >= -tol && iy <= hf + tol;
    let in_x = ix >= -tol && ix <= wf + tol;
    let mut best: Option<(CanvasEdge, f32)> = None;
    let mut consider = |edge: CanvasEdge, dist: f32, ok: bool| {
        if ok && dist <= tol && best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((edge, dist));
        }
    };
    consider(CanvasEdge::Left, dl, in_y);
    consider(CanvasEdge::Right, dr, in_y);
    consider(CanvasEdge::Top, dt, in_x);
    consider(CanvasEdge::Bottom, db, in_x);
    best.map(|(e, _)| e)
}

/// 图像矩形两边都走 `image_to_screen`, 相邻块共用同一条边, 避免各自取整留出缝.
fn image_px_bounds(
    xform: &ViewXform,
    ox: f32,
    oy: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) -> Bounds<gpui::Pixels> {
    let (sx0, sy0) = xform.image_to_screen(x0, y0);
    let (sx1, sy1) = xform.image_to_screen(x1, y1);
    let left = ox + sx0.min(sx1);
    let top = oy + sy0.min(sy1);
    let right = ox + sx0.max(sx1);
    let bot = oy + sy0.max(sy1);
    Bounds {
        origin: point(px(left), px(top)),
        size: size(px((right - left).max(1.0)), px((bot - top).max(1.0))),
    }
}

fn paint_layer_obb(
    window: &mut Window,
    xform: &ViewXform,
    ox: f32,
    oy: f32,
    layer: &LayerPaint,
    color: gpui::Rgba,
    thick: f32,
    dash: bool,
) {
    let corners = crate::geom::rotated_corners(
        layer.x as f32,
        layer.y as f32,
        layer.w as f32,
        layer.h as f32,
        layer.rotate_deg,
    );
    let mut stroke = PathBuilder::stroke(px(thick));
    if dash {
        stroke = stroke.dash_array(&[px(5.), px(4.)]);
    }
    for (i, (ix, iy)) in corners.iter().enumerate() {
        let (sx, sy) = xform.image_to_screen(*ix, *iy);
        let p = point(px(ox + sx), px(oy + sy));
        if i == 0 {
            stroke.move_to(p);
        } else {
            stroke.line_to(p);
        }
    }
    stroke.close();
    if let Ok(path) = stroke.build() {
        window.paint_path(path, color);
    }
}

fn paint_circle_stroke(
    window: &mut Window,
    cx: f32,
    cy: f32,
    r: f32,
    width: f32,
    color: gpui::Rgba,
) {
    let r = r.max(1.0);
    let mut clear = rgb(0x000000);
    clear.a = 0.0;
    window.paint_quad(quad(
        Bounds {
            origin: point(px(cx - r), px(cy - r)),
            size: size(px(r * 2.0), px(r * 2.0)),
        },
        px(r),
        clear,
        px(width),
        color,
        Default::default(),
    ));
}

fn paint_contrast_circle(
    window: &mut Window,
    cx: f32,
    cy: f32,
    r: f32,
    width: f32,
    style: RingStyle,
) {
    let r = r.max(1.0);
    let width = width.max(1.0);
    if let Some(halo) = style.halo {
        let halo_c = rgb8(halo);
        paint_circle_stroke(window, cx, cy, r + width, width, halo_c);
        paint_circle_stroke(window, cx, cy, r, width, rgb8(style.ring));
        paint_circle_stroke(window, cx, cy, (r - width).max(1.0), width, halo_c);
    } else {
        paint_circle_stroke(window, cx, cy, r, width, rgb8(style.ring));
    }
}

fn paint_brush_ring(
    window: &mut Window,
    cx: f32,
    cy: f32,
    r: f32,
    style: RingStyle,
    fill_rgb: Option<[u8; 3]>,
) {
    let r = r.max(1.5);
    let mut fill = fill_rgb.map(rgb8).unwrap_or(rgb(0xf8fafc));
    fill.a = if fill_rgb.is_some() { 0.35 } else { 0.16 };
    let ring_w = (r * 0.08).clamp(1.5, 2.5);
    let b = Bounds {
        origin: point(px(cx - r), px(cy - r)),
        size: size(px(r * 2.0), px(r * 2.0)),
    };
    window.paint_quad(quad(b, px(r), fill, px(0.), fill, Default::default()));
    paint_contrast_circle(window, cx, cy, r, ring_w, style);
}

fn paint_line(
    window: &mut Window,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width: f32,
    color: gpui::Rgba,
) {
    let mut stroke = PathBuilder::stroke(px(width));
    stroke.move_to(point(px(x0), px(y0)));
    stroke.line_to(point(px(x1), px(y1)));
    if let Ok(path) = stroke.build() {
        window.paint_path(path, color);
    }
}

fn paint_stamp_cursor(window: &mut Window, cx: f32, cy: f32, r: f32, style: RingStyle) {
    let outer = r.max(8.0);
    let inner = (outer * 0.52).max(3.5);
    let ring_w = (outer * 0.08).clamp(1.5, 2.5);
    let ink = rgb(0x1c1917);
    paint_circle_stroke(window, cx, cy, inner, ring_w, ink);
    for (dx, dy) in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
        let x0 = cx + dx * inner;
        let y0 = cy + dy * inner;
        let x1 = cx + dx * outer;
        let y1 = cy + dy * outer;
        paint_line(window, x0, y0, x1, y1, ring_w, ink);
    }
    paint_contrast_circle(window, cx, cy, outer, ring_w, style);
}

/// Inkscape `rotate.svg`: 白圆 + 一周双头弯箭头, 热点在圆心.
fn paint_rotate_cursor(window: &mut Window, cx: f32, cy: f32) {
    let s = 1.35;
    let disc_r = 6.5 * s;
    let mut shadow = rgb(0x000000);
    shadow.a = 0.35;
    let disc = |window: &mut Window, ox: f32, oy: f32, color: gpui::Rgba| {
        window.paint_quad(quad(
            Bounds {
                origin: point(px(ox - disc_r), px(oy - disc_r)),
                size: size(px(disc_r * 2.0), px(disc_r * 2.0)),
            },
            px(disc_r),
            color,
            px(0.),
            color,
            Default::default(),
        ));
    };
    disc(window, cx + 1.0, cy + 1.0, shadow);
    disc(window, cx, cy, rgb(0xffffff));
    paint_inkscape_rotate_arrows(window, cx, cy, s, rgb(0x111111), 1.15 * s);
}

fn paint_inkscape_rotate_arrows(
    window: &mut Window,
    cx: f32,
    cy: f32,
    s: f32,
    color: gpui::Rgba,
    width: f32,
) {
    let map = |x: f32, y: f32| point(px(cx + (x - 16.0) * s), px(cy + (y - 16.0) * s));
    let rad = point(px(4.5 * s), px(4.5 * s));
    let mut stroke = PathBuilder::stroke(px(width));
    stroke.move_to(map(11.5, 16.0));
    stroke.relative_arc_to(rad, px(0.), false, true, point(px(2.78 * s), px(-4.16 * s)));
    stroke.relative_arc_to(rad, px(0.), false, true, point(px(4.9 * s), px(0.98 * s)));
    stroke.move_to(map(17.0, 13.5));
    stroke.line_to(map(19.5, 13.5));
    stroke.line_to(map(19.5, 11.0));
    stroke.move_to(map(20.5, 16.0));
    stroke.relative_arc_to(rad, px(0.), false, true, point(px(-2.78 * s), px(4.16 * s)));
    stroke.relative_arc_to(rad, px(0.), false, true, point(px(-4.9 * s), px(-0.98 * s)));
    stroke.move_to(map(15.0, 18.5));
    stroke.line_to(map(12.5, 18.5));
    stroke.line_to(map(12.5, 21.0));
    if let Ok(path) = stroke.build() {
        window.paint_path(path, color);
    }
}

fn paint_source_cross(window: &mut Window, cx: f32, cy: f32, half: f32, style: RingStyle) {
    let half = half.max(2.0);
    let w = (half * 0.08).clamp(1.5, 2.5);
    let ring = rgb8(style.ring);
    if let Some(halo) = style.halo {
        let halo_c = rgb8(halo);
        paint_line(window, cx - half, cy, cx + half, cy, w + 1.6, halo_c);
        paint_line(window, cx, cy - half, cx, cy + half, w + 1.6, halo_c);
    }
    paint_line(window, cx - half, cy, cx + half, cy, w, ring);
    paint_line(window, cx, cy - half, cx, cy + half, w, ring);
}
