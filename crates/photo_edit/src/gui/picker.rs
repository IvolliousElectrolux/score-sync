//! 浮动取色器 / 滴管 / RGB 手输 (与蒙版侧栏同交互).

use std::sync::Arc;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, Bounds, Corners, MouseButton,
    MouseDownEvent, MouseMoveEvent, PathBuilder, Pixels, RenderImage, SharedString,
};
use image::{Frame, ImageBuffer, RgbaImage};
use smallvec::smallvec;

use super::*;

pub(crate) fn eyedropper_icon(active: bool) -> impl IntoElement {
    let stroke = if active { rgb(0xf8fafc) } else { rgb(0xe2e8f0) };
    div().size(px(14.)).flex_shrink_0().child(
        canvas(|_, _, _| {}, {
            move |bounds, _, window, _| {
                let ox = f32::from(bounds.origin.x);
                let oy = f32::from(bounds.origin.y);
                let s = f32::from(bounds.size.width)
                    .min(f32::from(bounds.size.height))
                    .max(1.0);
                let p = |x: f32, y: f32| point(px(ox + x / 16.0 * s), px(oy + y / 16.0 * s));
                let thick = px((1.4_f32 * s / 14.0).max(1.0));
                let mut shaft = PathBuilder::stroke(thick);
                shaft.move_to(p(3.2, 12.8));
                shaft.line_to(p(10.2, 5.8));
                if let Ok(path) = shaft.build() {
                    window.paint_path(path, stroke);
                }
                let mut tip = PathBuilder::stroke(thick);
                tip.move_to(p(2.0, 11.2));
                tip.line_to(p(3.2, 12.8));
                tip.line_to(p(4.8, 11.4));
                if let Ok(path) = tip.build() {
                    window.paint_path(path, stroke);
                }
                let mut bulb = PathBuilder::stroke(thick);
                bulb.move_to(p(9.0, 4.6));
                bulb.line_to(p(11.0, 2.6));
                bulb.line_to(p(13.2, 4.8));
                bulb.line_to(p(11.2, 6.8));
                bulb.close();
                if let Ok(path) = bulb.build() {
                    window.paint_path(path, stroke);
                }
                let drop = Bounds {
                    origin: p(2.4, 13.0),
                    size: size(px(2.2 / 16.0 * s), px(2.2 / 16.0 * s)),
                };
                window.paint_quad(quad(
                    drop,
                    px(1.2 / 16.0 * s),
                    stroke,
                    px(0.),
                    stroke,
                    Default::default(),
                ));
            }
        })
        .size_full(),
    )
}

impl PhotoEditApp {
    pub(super) fn point_in_bounds(x: f32, y: f32, b: Bounds<Pixels>) -> bool {
        let bx = f32::from(b.origin.x);
        let by = f32::from(b.origin.y);
        let bw = f32::from(b.size.width);
        let bh = f32::from(b.size.height);
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    }

    pub(super) fn sync_picker_hsv_from_paint(&mut self) {
        let (h, s, v) = rgb_to_hsv(self.paint_color);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
    }

    pub(super) fn open_color_picker(&mut self, cx: &mut Context<Self>) {
        if self.color_picker_open {
            self.close_color_picker(cx);
            return;
        }
        self.color_picker_open = true;
        self.sync_picker_hsv_from_paint();
        self.rebuild_sb_image();
        if self.hue_image.is_none() {
            self.rebuild_hue_image();
        }
        self.sync_rgb_inputs_from_picker(cx);
        self.notify_chrome(cx);
    }

    pub(super) fn close_color_picker(&mut self, cx: &mut Context<Self>) {
        if !self.color_picker_open {
            return;
        }
        self.push_recent_color(self.paint_color);
        self.eyedropper_armed = false;
        self.eyedropper_backup = None;
        self.color_picker_open = false;
        if matches!(self.drag, Some(DragKind::PaletteSb | DragKind::PaletteHue)) {
            self.drag = None;
        }
        self.notify_chrome(cx);
    }

    pub(super) fn picker_pop_w() -> f32 {
        (SB_SIZE + HUE_BAR_W + 8.0 + 16.0 + 4.0).max(232.0)
    }

    pub(super) fn color_picker_placement(&self) -> Option<(f32, f32, bool, f32)> {
        let layer = if f32::from(self.picker_layer_bounds.size.width) >= 8.0 {
            self.picker_layer_bounds
        } else {
            self.side_bounds
        };
        let pw = f32::from(layer.size.width);
        let ph = f32::from(layer.size.height);
        if pw < 8.0 || ph < 8.0 {
            return None;
        }
        let anchor = self.paint_swatch_bounds;
        let aw = f32::from(anchor.size.width);
        let ah = f32::from(anchor.size.height);
        if aw < 1.0 || ah < 1.0 {
            return None;
        }
        let pop_w = Self::picker_pop_w();
        let pop_h = 22.0 + 8.0 + SB_SIZE + 40.0 + 16.0;
        let caret = 8.0;
        let gap = 6.0;
        let layer_left = f32::from(layer.origin.x);
        let layer_top = f32::from(layer.origin.y);
        let ax = f32::from(anchor.origin.x);
        let ay = f32::from(anchor.origin.y);
        let anchor_cx = ax + aw * 0.5 - layer_left;
        let anchor_top = ay - layer_top;
        let anchor_bot = ay + ah - layer_top;
        let space_below = ph - anchor_bot;
        let space_above = anchor_top;
        let place_below = space_below >= pop_h + caret + gap || space_below >= space_above;
        let left = (anchor_cx - pop_w * 0.5).clamp(4.0, (pw - pop_w - 4.0).max(4.0));
        let stack_h = pop_h + caret;
        let top = if place_below {
            anchor_bot + gap
        } else {
            (anchor_top - gap - stack_h).max(4.0)
        };
        let caret_x = (anchor_cx - left).clamp(10.0, pop_w - 10.0);
        Some((left, top, place_below, caret_x))
    }

    pub(super) fn picker_caret(place_below: bool, caret_x: f32) -> impl IntoElement {
        let h = 8.0_f32;
        let half = 8.0_f32;
        div().w_full().h(px(h)).relative().child(
            canvas(|_, _, _| {}, {
                move |bounds, _, window, _| {
                    let ox = f32::from(bounds.origin.x);
                    let oy = f32::from(bounds.origin.y);
                    let cx = ox + caret_x;
                    let mut builder = PathBuilder::fill();
                    if place_below {
                        builder.move_to(point(px(cx), px(oy)));
                        builder.line_to(point(px(cx - half), px(oy + h)));
                        builder.line_to(point(px(cx + half), px(oy + h)));
                    } else {
                        builder.move_to(point(px(cx), px(oy + h)));
                        builder.line_to(point(px(cx - half), px(oy)));
                        builder.line_to(point(px(cx + half), px(oy)));
                    }
                    builder.close();
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, rgb(0x1e293b));
                    }
                }
            })
            .absolute()
            .size_full(),
        )
    }

    pub(super) fn color_picker_floating(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (left, top, place_below, caret_x) = self
            .color_picker_placement()
            .unwrap_or((8.0, 120.0, true, 40.0));
        let pop_w = Self::picker_pop_w();
        div()
            .id("photo_color_picker_layer")
            .absolute()
            .inset_0()
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, cx| {
                                let prev = this.picker_layer_bounds;
                                this.picker_layer_bounds = bounds;
                                let changed = f32::from(prev.size.width) < 1.0
                                    || (f32::from(prev.origin.x) - f32::from(bounds.origin.x))
                                        .abs()
                                        > 0.5
                                    || (f32::from(prev.origin.y) - f32::from(bounds.origin.y))
                                        .abs()
                                        > 0.5;
                                if changed && this.color_picker_open {
                                    cx.notify();
                                }
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    if Self::point_in_bounds(x, y, this.paint_swatch_bounds) {
                        this.open_color_picker(cx);
                        return;
                    }
                    this.close_color_picker(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_color_picker(cx);
                }),
            )
            .child(
                div()
                    .id("photo_color_picker_float")
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(pop_w))
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .when(place_below, |d| d.child(Self::picker_caret(true, caret_x)))
                    .child(self.color_picker_popover(cx))
                    .when(!place_below, |d| {
                        d.child(Self::picker_caret(false, caret_x))
                    }),
            )
    }

    pub(super) fn color_picker_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sb_img = self.sb_image.clone();
        let hue_img = self.hue_image.clone();
        let picker_s = self.picker_s;
        let picker_v = self.picker_v;
        let picker_h = self.picker_h;
        let recent: Vec<[u8; 3]> = self
            .recent_colors
            .iter()
            .copied()
            .take(RECENT_COLORS_MAX)
            .collect();
        let drop_on = self.eyedropper_armed;
        let r_in = self.rgb_r_input.clone();
        let g_in = self.rgb_g_input.clone();
        let b_in = self.rgb_b_input.clone();

        div()
            .id("photo_color_picker_popover")
            .w_full()
            .p_2()
            .rounded_md()
            .bg(rgb(0x1e293b))
            .border_1()
            .border_color(rgb(0x334155))
            .flex()
            .flex_col()
            .gap_2()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::PaletteSb)) {
                    this.set_palette_sb_from_pos(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                } else if matches!(this.drag, Some(DragKind::PaletteHue)) {
                    this.set_palette_hue_from_y(f32::from(ev.position.y), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if matches!(this.drag, Some(DragKind::PaletteSb | DragKind::PaletteHue)) {
                        this.drag = None;
                        this.notify_chrome(cx);
                    }
                }),
            )
            .child(div().flex().flex_row().flex_wrap().gap_1().children(
                recent.into_iter().enumerate().map(|(i, color)| {
                    let color_u32 = color_rgb_u32(color);
                    div()
                        .id(SharedString::from(format!("photo-recent-{i}")))
                        .size(px(22.))
                        .rounded_sm()
                        .bg(rgb(color_u32))
                        .border_1()
                        .border_color(rgb(0x64748b))
                        .cursor_pointer()
                        .hover(|s| s.border_color(rgb(0x94a3b8)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.pick_recent_color(color, cx);
                            }),
                        )
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .id("photo_palette_sb")
                            .relative()
                            .size(px(SB_SIZE))
                            .flex_shrink_0()
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(0x475569))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    {
                                        let entity = cx.entity().clone();
                                        move |bounds, _, cx| {
                                            entity.update(cx, |this, _| {
                                                this.sb_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = sb_img {
                                            let _ = window.paint_image(
                                                bounds,
                                                Corners::default(),
                                                img.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                        let mx = bounds.origin.x
                                            + px(picker_s * f32::from(bounds.size.width));
                                        let my = bounds.origin.y
                                            + px((1.0 - picker_v) * f32::from(bounds.size.height));
                                        window.paint_quad(quad(
                                            Bounds {
                                                origin: point(mx - px(5.), my - px(5.)),
                                                size: size(px(10.), px(10.)),
                                            },
                                            px(5.),
                                            rgb(0xffffff),
                                            px(1.5),
                                            rgb(0x0f172a),
                                            Default::default(),
                                        ));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.drag = Some(DragKind::PaletteSb);
                                    this.set_palette_sb_from_pos(
                                        f32::from(ev.position.x),
                                        f32::from(ev.position.y),
                                        cx,
                                    );
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("photo_palette_hue")
                            .relative()
                            .w(px(HUE_BAR_W))
                            .h(px(SB_SIZE))
                            .flex_shrink_0()
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(0x475569))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    {
                                        let entity = cx.entity().clone();
                                        move |bounds, _, cx| {
                                            entity.update(cx, |this, _| {
                                                this.hue_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = hue_img {
                                            let _ = window.paint_image(
                                                bounds,
                                                Corners::default(),
                                                img.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                        let hy = bounds.origin.y
                                            + px((picker_h / 360.0).clamp(0.0, 1.0)
                                                * f32::from(bounds.size.height));
                                        window.paint_quad(quad(
                                            Bounds {
                                                origin: point(bounds.origin.x, hy - px(2.)),
                                                size: size(bounds.size.width, px(4.)),
                                            },
                                            px(0.),
                                            rgb(0xffffff),
                                            px(1.),
                                            rgb(0x0f172a),
                                            Default::default(),
                                        ));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.drag = Some(DragKind::PaletteHue);
                                    this.set_palette_hue_from_y(f32::from(ev.position.y), cx);
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(div().text_xs().text_color(rgb(0xcbd5e1)).child("RGB"))
                            .child(
                                div()
                                    .id("photo_eyedropper_btn")
                                    .size(px(20.))
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(if drop_on {
                                        rgb(0x38bdf8)
                                    } else {
                                        rgb(0x475569)
                                    })
                                    .bg(if drop_on {
                                        rgb(0x0ea5e9)
                                    } else {
                                        rgb(0x334155)
                                    })
                                    .child(eyedropper_icon(drop_on))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.arm_eyedropper(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("R"),
                            )
                            .child(
                                div()
                                    .id("photo_rgb_r_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(r_in),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("G"),
                            )
                            .child(
                                div()
                                    .id("photo_rgb_g_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(g_in),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("B"),
                            )
                            .child(
                                div()
                                    .id("photo_rgb_b_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(b_in),
                            ),
                    ),
            )
    }

    pub(super) fn rebuild_hue_image(&mut self) {
        let w = 4u32;
        let h = HUE_TEX_H;
        let mut rgba: RgbaImage = ImageBuffer::new(w, h);
        for y in 0..h {
            let hue = 360.0 * y as f32 / (h - 1).max(1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, 1.0, 1.0);
            for x in 0..w {
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        let neu = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
        if let Some(old) = self.hue_image.replace(neu) {
            self.gpu_drop.push(old);
        }
    }

    pub(super) fn rebuild_sb_image(&mut self) {
        let size = SB_TEX_SIZE;
        let mut rgba: RgbaImage = ImageBuffer::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let s = x as f32 / (size - 1).max(1) as f32;
                let v = 1.0 - y as f32 / (size - 1).max(1) as f32;
                let [r, g, b] = hsv_to_rgb(self.picker_h, s, v);
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        let neu = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
        if let Some(old) = self.sb_image.replace(neu) {
            self.gpu_drop.push(old);
        }
    }

    pub(super) fn picker_rgb(&self) -> [u8; 3] {
        hsv_to_rgb(self.picker_h, self.picker_s, self.picker_v)
    }

    pub(super) fn sync_rgb_inputs_from_picker(&mut self, cx: &mut Context<Self>) {
        let [r, g, b] = self.picker_rgb();
        self.rgb_syncing = true;
        self.rgb_r_input
            .update(cx, |t, cx| t.set_text(r.to_string(), cx));
        self.rgb_g_input
            .update(cx, |t, cx| t.set_text(g.to_string(), cx));
        self.rgb_b_input
            .update(cx, |t, cx| t.set_text(b.to_string(), cx));
        self.rgb_syncing = false;
    }

    pub(super) fn apply_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        if self.rgb_syncing || !self.color_picker_open {
            return;
        }
        let blur = self.rgb_r_input.update(cx, |t, _| t.take_blur_commit())
            | self.rgb_g_input.update(cx, |t, _| t.take_blur_commit())
            | self.rgb_b_input.update(cx, |t, _| t.take_blur_commit());
        let parse = |s: String| -> Option<u8> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            t.parse::<u8>().ok()
        };
        let r = parse(self.rgb_r_input.read(cx).text());
        let g = parse(self.rgb_g_input.read(cx).text());
        let b = parse(self.rgb_b_input.read(cx).text());
        let (Some(r), Some(g), Some(b)) = (r, g, b) else {
            if blur {
                self.sync_rgb_inputs_from_picker(cx);
            }
            return;
        };
        if [r, g, b] == self.picker_rgb() {
            return;
        }
        self.set_picker_from_rgb([r, g, b], cx);
    }

    pub(super) fn set_picker_from_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.commit_picker_color(false);
        cx.notify();
    }

    pub(super) fn preview_eyedropper_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.paint_color = rgb;
        self.sync_rgb_inputs_from_picker(cx);
        self.notify_chrome(cx);
    }

    pub(super) fn arm_eyedropper(&mut self, cx: &mut Context<Self>) {
        if !self.color_picker_open {
            return;
        }
        if self.eyedropper_armed {
            self.cancel_eyedropper(cx);
            return;
        }
        let c = self.picker_rgb();
        self.eyedropper_backup = Some((self.picker_h, self.picker_s, self.picker_v, c));
        self.eyedropper_armed = true;
        self.status = "取色: 在画布上移动预览, 单击确认, Esc/右键取消".into();
        self.notify_chrome(cx);
    }

    pub(super) fn cancel_eyedropper(&mut self, cx: &mut Context<Self>) {
        if !self.eyedropper_armed {
            return;
        }
        if let Some((h, s, v, c)) = self.eyedropper_backup.take() {
            self.picker_h = h;
            self.picker_s = s;
            self.picker_v = v;
            self.rebuild_sb_image();
            self.paint_color = c;
            self.sync_rgb_inputs_from_picker(cx);
        }
        self.eyedropper_armed = false;
        self.status = "已取消取色".into();
        self.notify_chrome(cx);
    }

    pub(super) fn confirm_eyedropper_at(&mut self, ix: f32, iy: f32, cx: &mut Context<Self>) {
        self.sample_paint_color(ix.round() as i32, iy.round() as i32, cx);
        let c = self.paint_color;
        let (h, s, v) = rgb_to_hsv(c);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.commit_picker_color(true);
        self.sync_rgb_inputs_from_picker(cx);
        self.eyedropper_armed = false;
        self.eyedropper_backup = None;
        self.status = "已取色".into();
        self.notify_chrome(cx);
    }

    pub(super) fn commit_picker_color(&mut self, push_recent: bool) {
        let c = self.picker_rgb();
        self.paint_color = c;
        if push_recent {
            self.push_recent_color(c);
        }
    }

    pub(super) fn push_recent_color(&mut self, c: [u8; 3]) {
        self.recent_colors.retain(|x| *x != c);
        self.recent_colors.insert(0, c);
        self.recent_colors.truncate(RECENT_COLORS_MAX);
    }

    pub(super) fn set_palette_sb_from_pos(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.sb_bounds.origin.x);
        let top = f32::from(self.sb_bounds.origin.y);
        let w = f32::from(self.sb_bounds.size.width).max(1.0);
        let h = f32::from(self.sb_bounds.size.height).max(1.0);
        self.picker_s = ((x - left) / w).clamp(0.0, 1.0);
        self.picker_v = (1.0 - (y - top) / h).clamp(0.0, 1.0);
        self.commit_picker_color(false);
        self.sync_rgb_inputs_from_picker(cx);
        self.notify_chrome(cx);
    }

    pub(super) fn set_palette_hue_from_y(&mut self, y: f32, cx: &mut Context<Self>) {
        let top = f32::from(self.hue_bounds.origin.y);
        let h = f32::from(self.hue_bounds.size.height).max(1.0);
        self.picker_h = ((y - top) / h).clamp(0.0, 1.0) * 360.0;
        self.rebuild_sb_image();
        self.commit_picker_color(false);
        self.sync_rgb_inputs_from_picker(cx);
        self.notify_chrome(cx);
    }

    pub(super) fn pick_recent_color(&mut self, color: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(color);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.commit_picker_color(true);
        self.sync_rgb_inputs_from_picker(cx);
        self.notify_chrome(cx);
    }
}
