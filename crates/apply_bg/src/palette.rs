//! 可复用的 HSV 调色盘: 饱和度方块, 色相条, 最近色, RGB 输入.
//! 宿主自己保存 `PaletteModel`, 并把指针事件交回来.

use std::sync::Arc;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, rgba, size, Bounds, Context, Corners, Entity,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, RenderImage,
    SharedString,
};
use image::{Frame, ImageBuffer, RgbaImage};
use smallvec::smallvec;

use crate::text_input::TextInput;

pub const SB_SIZE: f32 = 160.0;
pub const HUE_BAR_W: f32 = 18.0;
const SB_TEX: u32 = 128;
const HUE_TEX_H: u32 = 256;
const RECENT_MAX: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteSlot {
    Sb,
    Hue,
}

pub struct PaletteModel {
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub sb: Option<Arc<RenderImage>>,
    pub hue: Option<Arc<RenderImage>>,
    pub sb_bounds: Bounds<Pixels>,
    pub hue_bounds: Bounds<Pixels>,
    pub recent: Vec<[u8; 3]>,
}

impl Default for PaletteModel {
    fn default() -> Self {
        Self {
            h: 0.0,
            s: 0.0,
            v: 0.0,
            sb: None,
            hue: None,
            sb_bounds: Bounds::default(),
            hue_bounds: Bounds::default(),
            recent: Vec::new(),
        }
    }
}

impl PaletteModel {
    pub fn rgb(&self) -> [u8; 3] {
        hsv_to_rgb(self.h, self.s, self.v)
    }

    /// 换成这个颜色, 并重建饱和度图. 返回被换下来的贴图, 交给宿主丢掉.
    pub fn set_rgb(&mut self, rgb: [u8; 3]) -> Vec<Arc<RenderImage>> {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.h = h;
        self.s = s;
        self.v = v;
        self.rebuild_maps()
    }

    pub fn rebuild_maps(&mut self) -> Vec<Arc<RenderImage>> {
        let mut old = Vec::new();
        if let Some(img) = self.hue.take() {
            old.push(img);
        }
        self.hue = Some(hue_image());
        if let Some(img) = self.sb.replace(sb_image(self.h)) {
            old.push(img);
        }
        old
    }

    pub fn rebuild_sb(&mut self) -> Option<Arc<RenderImage>> {
        self.sb.replace(sb_image(self.h))
    }

    pub fn sb_at(&mut self, x: f32, y: f32) -> [u8; 3] {
        let left = f32::from(self.sb_bounds.origin.x);
        let top = f32::from(self.sb_bounds.origin.y);
        let w = f32::from(self.sb_bounds.size.width).max(1.0);
        let h = f32::from(self.sb_bounds.size.height).max(1.0);
        self.s = ((x - left) / w).clamp(0.0, 1.0);
        self.v = (1.0 - (y - top) / h).clamp(0.0, 1.0);
        self.rgb()
    }

    pub fn hue_at(&mut self, y: f32) -> [u8; 3] {
        let top = f32::from(self.hue_bounds.origin.y);
        let h = f32::from(self.hue_bounds.size.height).max(1.0);
        self.h = ((y - top) / h).clamp(0.0, 1.0) * 360.0;
        self.rgb()
    }

    pub fn push_recent(&mut self, color: [u8; 3]) {
        self.recent.retain(|c| *c != color);
        self.recent.insert(0, color);
        self.recent.truncate(RECENT_MAX);
    }
}

pub fn palette_panel<T: 'static>(
    cx: &mut Context<T>,
    id_prefix: &'static str,
    model: &PaletteModel,
    rgb_inputs: &(
        Entity<TextInput>,
        Entity<TextInput>,
        Entity<TextInput>,
        Entity<TextInput>,
    ),
    alpha_t: f32,
    on_bounds: impl Fn(&mut T, PaletteSlot, Bounds<Pixels>) + Clone + 'static,
    on_alpha_bounds: impl Fn(&mut T, Bounds<Pixels>) + Clone + 'static,
    on_sb: impl Fn(&mut T, f32, f32, &mut Context<T>) + Clone + 'static,
    on_hue: impl Fn(&mut T, f32, &mut Context<T>) + Clone + 'static,
    on_alpha: impl Fn(&mut T, f32, &mut Context<T>) + Clone + 'static,
    on_recent: impl Fn(&mut T, [u8; 3], &mut Context<T>) + Clone + 'static,
    on_preset: impl Fn(&mut T, [u8; 4], &mut Context<T>) + Clone + 'static,
    presets: &[[u8; 4]],
    on_move: impl Fn(&mut T, f32, f32, &mut Context<T>) + Clone + 'static,
    on_up: impl Fn(&mut T, &mut Context<T>) + Clone + 'static,
) -> impl IntoElement {
    let sb_img = model.sb.clone();
    let hue_img = model.hue.clone();
    let picker_s = model.s;
    let picker_v = model.v;
    let picker_h = model.h;
    let swatch = model.rgb();
    let recent = model.recent.clone();
    let presets: Vec<[u8; 4]> = presets.to_vec();
    let (r_in, g_in, b_in, a_in) = rgb_inputs.clone();
    let entity = cx.entity().clone();

    div()
        .id(SharedString::from(format!("{id_prefix}-panel")))
        .w_full()
        .p_2()
        .rounded_md()
        .bg(ui_kit::chrome(0x1e293b))
        .border_1()
        .border_color(ui_kit::chrome(0x334155))
        .flex()
        .flex_col()
        .gap_2()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| cx.stop_propagation()),
        )
        .on_mouse_move(cx.listener({
            let on_move = on_move.clone();
            move |this, ev: &MouseMoveEvent, _, cx| {
                on_move(this, f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener({
                let on_up = on_up.clone();
                move |this, _: &MouseUpEvent, _, cx| on_up(this, cx)
            }),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .children(presets.into_iter().enumerate().map(|(i, color)| {
                    let on_preset = on_preset.clone();
                    preset_chip(id_prefix, i, color, cx, on_preset)
                }))
                .children(recent.into_iter().enumerate().map(|(i, color)| {
                    let color_u32 =
                        ((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32;
                    let on_recent = on_recent.clone();
                    div()
                        .id(SharedString::from(format!("{id_prefix}-recent-{i}")))
                        .size(px(18.))
                        .rounded_sm()
                        .bg(rgb(color_u32))
                        .border_1()
                        .border_color(ui_kit::chrome(0x64748b))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                on_recent(this, color, cx);
                            }),
                        )
                })),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .gap_2()
                .child(sb_square(
                    entity.clone(),
                    id_prefix,
                    sb_img,
                    picker_s,
                    picker_v,
                    on_bounds.clone(),
                    on_sb,
                    cx,
                ))
                .child(hue_bar(
                    entity.clone(),
                    id_prefix,
                    hue_img,
                    picker_h,
                    on_bounds,
                    on_hue,
                    cx,
                )),
        )
        .child(rgb_row(id_prefix, r_in, g_in, b_in))
        .child(alpha_row(
            entity,
            id_prefix,
            swatch,
            alpha_t,
            a_in,
            on_alpha_bounds,
            on_alpha,
            cx,
        ))
}

/// 灰白格子, 垫在半透明色下面, 白和透明才分得开.
pub fn checkerboard(cell: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let x0 = f32::from(bounds.origin.x);
            let y0 = f32::from(bounds.origin.y);
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let mut y = 0.0;
            let mut row = 0i32;
            while y < h - 0.1 {
                let mut x = 0.0;
                let mut col = 0i32;
                let ch = cell.min(h - y);
                while x < w - 0.1 {
                    let cw = cell.min(w - x);
                    let light = (row + col) % 2 == 0;
                    window.paint_quad(quad(
                        Bounds {
                            origin: point(px(x0 + x), px(y0 + y)),
                            size: size(px(cw), px(ch)),
                        },
                        px(0.),
                        if light { rgb(0xffffff) } else { rgb(0xd4d4d8) },
                        px(0.),
                        rgb(0x000000),
                        Default::default(),
                    ));
                    x += cell;
                    col += 1;
                }
                y += cell;
                row += 1;
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

/// 线条眼睛. 睁开是显示, 闭上是隐藏.
pub fn eye_icon(open: bool, color: gpui::Rgba) -> impl IntoElement {
    div().size(px(16.)).flex_shrink_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let ox = f32::from(bounds.origin.x);
                let oy = f32::from(bounds.origin.y);
                let s = f32::from(bounds.size.width)
                    .min(f32::from(bounds.size.height))
                    .max(1.0);
                let p = |x: f32, y: f32| point(px(ox + x / 16.0 * s), px(oy + y / 16.0 * s));
                let thick = px((1.35_f32 * s / 16.0).max(1.0));
                let mut lid = PathBuilder::stroke(thick);
                if open {
                    lid.move_to(p(1.5, 8.2));
                    lid.curve_to(p(14.5, 8.2), p(8.0, 2.2));
                    lid.move_to(p(1.5, 8.2));
                    lid.curve_to(p(14.5, 8.2), p(8.0, 14.2));
                } else {
                    lid.move_to(p(1.8, 8.0));
                    lid.curve_to(p(14.2, 8.0), p(8.0, 5.6));
                    lid.move_to(p(1.8, 8.0));
                    lid.curve_to(p(14.2, 8.0), p(8.0, 10.4));
                }
                if let Ok(path) = lid.build() {
                    window.paint_path(path, color);
                }
                if open {
                    let center = p(8.0, 8.2);
                    let r = 1.55 / 16.0 * s;
                    let mut pupil = PathBuilder::stroke(thick);
                    pupil.move_to(point(center.x + px(r), center.y));
                    pupil.arc_to(
                        point(px(r), px(r)),
                        px(0.),
                        false,
                        false,
                        point(center.x - px(r), center.y),
                    );
                    pupil.arc_to(
                        point(px(r), px(r)),
                        px(0.),
                        false,
                        false,
                        point(center.x + px(r), center.y),
                    );
                    pupil.close();
                    if let Ok(path) = pupil.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .size_full(),
    )
}

fn preset_chip<T: 'static>(
    id_prefix: &'static str,
    index: usize,
    color: [u8; 4],
    cx: &mut Context<T>,
    on_preset: impl Fn(&mut T, [u8; 4], &mut Context<T>) + 'static,
) -> impl IntoElement {
    let packed = (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2]);
    div()
        .id(SharedString::from(format!("{id_prefix}-preset-{index}")))
        .size(px(18.))
        .rounded_sm()
        .bg(rgb(packed))
        .border_1()
        .border_color(ui_kit::chrome(0x64748b))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_preset(this, color, cx);
            }),
        )
}

fn sb_square<T: 'static>(
    entity: Entity<T>,
    id_prefix: &'static str,
    img: Option<Arc<RenderImage>>,
    s: f32,
    v: f32,
    on_bounds: impl Fn(&mut T, PaletteSlot, Bounds<Pixels>) + 'static,
    on_sb: impl Fn(&mut T, f32, f32, &mut Context<T>) + 'static,
    cx: &mut Context<T>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{id_prefix}-sb")))
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(SB_SIZE))
        .rounded_sm()
        .overflow_hidden()
        .border_1()
        .border_color(ui_kit::chrome(0x475569))
        .cursor_pointer()
        .child(
            canvas(
                {
                    let entity = entity.clone();
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| on_bounds(this, PaletteSlot::Sb, bounds));
                    }
                },
                move |bounds, _, window, _| {
                    if let Some(ref img) = img {
                        let _ =
                            window.paint_image(bounds, Corners::default(), img.clone(), 0, false);
                    }
                    let mx = bounds.origin.x + px(s * f32::from(bounds.size.width));
                    let my = bounds.origin.y + px((1.0 - v) * f32::from(bounds.size.height));
                    window.paint_quad(quad(
                        Bounds {
                            origin: point(mx - px(5.), my - px(5.)),
                            size: size(px(10.), px(10.)),
                        },
                        px(5.),
                        rgb(0xffffff),
                        px(1.5),
                        ui_kit::chrome(0x0f172a),
                        Default::default(),
                    ));
                },
            )
            .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                on_sb(this, f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }),
        )
}

fn hue_bar<T: 'static>(
    entity: Entity<T>,
    id_prefix: &'static str,
    img: Option<Arc<RenderImage>>,
    hue: f32,
    on_bounds: impl Fn(&mut T, PaletteSlot, Bounds<Pixels>) + 'static,
    on_hue: impl Fn(&mut T, f32, &mut Context<T>) + 'static,
    cx: &mut Context<T>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{id_prefix}-hue")))
        .relative()
        .w(px(HUE_BAR_W))
        .h(px(SB_SIZE))
        .flex_shrink_0()
        .rounded_sm()
        .overflow_hidden()
        .border_1()
        .border_color(ui_kit::chrome(0x475569))
        .cursor_pointer()
        .child(
            canvas(
                move |bounds, _, cx| {
                    entity.update(cx, |this, _| on_bounds(this, PaletteSlot::Hue, bounds));
                },
                move |bounds, _, window, _| {
                    if let Some(ref img) = img {
                        let _ =
                            window.paint_image(bounds, Corners::default(), img.clone(), 0, false);
                    }
                    let hy = bounds.origin.y
                        + px((hue / 360.0).clamp(0.0, 1.0) * f32::from(bounds.size.height));
                    window.paint_quad(quad(
                        Bounds {
                            origin: point(bounds.origin.x, hy - px(2.)),
                            size: size(bounds.size.width, px(4.)),
                        },
                        px(0.),
                        rgb(0xffffff),
                        px(1.),
                        ui_kit::chrome(0x0f172a),
                        Default::default(),
                    ));
                },
            )
            .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                on_hue(this, f32::from(ev.position.y), cx);
            }),
        )
}

fn rgb_row(
    id_prefix: &'static str,
    r_in: Entity<TextInput>,
    g_in: Entity<TextInput>,
    b_in: Entity<TextInput>,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(channel(id_prefix, "R", r_in))
        .child(channel(id_prefix, "G", g_in))
        .child(channel(id_prefix, "B", b_in))
}

fn alpha_row<T: 'static>(
    entity: Entity<T>,
    id_prefix: &'static str,
    swatch: [u8; 3],
    alpha_t: f32,
    a_in: Entity<TextInput>,
    on_bounds: impl Fn(&mut T, Bounds<Pixels>) + 'static,
    on_alpha: impl Fn(&mut T, f32, &mut Context<T>) + 'static,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let t = alpha_t.clamp(0.0, 1.0);
    let fill = rgba(
        (u32::from(swatch[0]) << 24)
            | (u32::from(swatch[1]) << 16)
            | (u32::from(swatch[2]) << 8)
            | 255,
    );
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(ui_kit::chrome(0x94a3b8))
                .child("A"),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id_prefix}-alpha")))
                .relative()
                .flex_1()
                .h(px(12.))
                .rounded_sm()
                .overflow_hidden()
                .border_1()
                .border_color(ui_kit::chrome(0x475569))
                .cursor_pointer()
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| on_bounds(this, bounds));
                        },
                        move |bounds, _, window, _| {
                            window.paint_quad(quad(
                                bounds,
                                px(0.),
                                ui_kit::chrome(0x0f172a),
                                px(0.),
                                rgb(0x000000),
                                Default::default(),
                            ));
                            let w = px(f32::from(bounds.size.width) * t);
                            window.paint_quad(quad(
                                Bounds {
                                    origin: bounds.origin,
                                    size: size(w, bounds.size.height),
                                },
                                px(0.),
                                fill,
                                px(0.),
                                rgb(0x000000),
                                Default::default(),
                            ));
                        },
                    )
                    .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        on_alpha(this, f32::from(ev.position.x), cx);
                    }),
                ),
        )
        .child(div().w(px(40.)).h(px(20.)).flex_shrink_0().child(a_in))
}

fn channel(
    id_prefix: &'static str,
    label: &'static str,
    input: Entity<TextInput>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_1()
        .min_w(px(0.))
        .child(
            div()
                .text_xs()
                .text_color(ui_kit::chrome(0x94a3b8))
                .child(label),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id_prefix}-rgb-{label}")))
                .flex_1()
                .min_w(px(0.))
                .h(px(20.))
                .child(input),
        )
}

fn upload(img: RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(smallvec![Frame::new(img)]))
}

fn hue_image() -> Arc<RenderImage> {
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
    upload(rgba)
}

fn sb_image(hue: f32) -> Arc<RenderImage> {
    let size = SB_TEX;
    let mut rgba: RgbaImage = ImageBuffer::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let s = x as f32 / (size - 1).max(1) as f32;
            let v = 1.0 - y as f32 / (size - 1).max(1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, s, v);
            rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
        }
    }
    upload(rgba)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

pub fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_roundtrip() {
        let (h, s, v) = rgb_to_hsv([255, 0, 0]);
        assert!(s > 0.99 && v > 0.99);
        assert!(h < 1.0 || h > 359.0);
        assert_eq!(hsv_to_rgb(h, s, v), [255, 0, 0]);
    }
}
