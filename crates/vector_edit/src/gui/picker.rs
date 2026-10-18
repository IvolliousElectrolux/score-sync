//! 填充 / 描边. 调色盘用 apply_bg::palette, 填充和描边共用一块.

use std::sync::Arc;

use apply_bg::palette::{self, PaletteSlot, SB_SIZE, HUE_BAR_W};
use gpui::{
    canvas, div, point, prelude::*, px, Bounds, Context, Entity, MouseButton, MouseDownEvent,
    MouseUpEvent, PathBuilder, Pixels, RenderImage,
};

use super::*;
use crate::document::ObjKind;

const PRESET_COLORS: [[u8; 4]; 4] = [
    [0, 0, 0, 255],
    [255, 255, 255, 255],
    [227, 38, 54, 255],
    [0, 114, 206, 255],
];

impl VectorEditApp {
    pub(crate) fn set_fill(&mut self, rgba: Option<[u8; 4]>, cx: &mut Context<Self>) {
        self.style.fill = rgba;
        self.remember_paints();
        self.commit_style(cx);
    }

    pub(crate) fn set_stroke(&mut self, rgba: Option<[u8; 4]>, cx: &mut Context<Self>) {
        self.style.stroke = rgba;
        self.remember_paints();
        self.commit_style(cx);
    }

    pub(crate) fn remember_paints(&mut self) {
        if let Some(c) = self.style.fill {
            self.fill_memory = c;
        }
        if let Some(c) = self.style.stroke {
            self.stroke_memory = c;
        }
    }

    fn memory_rgb(&self) -> [u8; 3] {
        let c = if self.paint_on_fill {
            self.fill_memory
        } else {
            self.stroke_memory
        };
        [c[0], c[1], c[2]]
    }

    pub(crate) fn set_active_paint(&mut self, rgba: Option<[u8; 4]>, cx: &mut Context<Self>) {
        if self.paint_on_fill {
            self.set_fill(rgba, cx);
        } else {
            self.set_stroke(rgba, cx);
        }
    }

    pub(crate) fn swap_paint(&mut self, cx: &mut Context<Self>) {
        let fill = self.style.fill;
        self.style.fill = self.style.stroke;
        self.style.stroke = fill;
        self.remember_paints();
        self.commit_style(cx);
    }

    /// 左键管调色盘: 未激活的先换到前面并打开, 已激活的开关弹出菜单.
    pub(crate) fn on_paint_proxy(&mut self, fill: bool, cx: &mut Context<Self>) {
        if self.paint_on_fill != fill {
            self.paint_on_fill = fill;
            if self.palette_open {
                self.retarget_palette(cx);
            } else {
                self.open_palette(cx);
            }
            return;
        }
        if self.palette_open {
            self.close_palette(cx);
        } else {
            self.open_palette(cx);
        }
    }

    /// 右键管填色: 未激活的只换到前面, 已激活的在有颜色和无之间切换.
    pub(crate) fn on_paint_proxy_right(&mut self, fill: bool, cx: &mut Context<Self>) {
        if self.paint_on_fill != fill {
            self.paint_on_fill = fill;
            if self.palette_open {
                self.retarget_palette(cx);
            } else {
                self.notify_chrome(cx);
            }
            return;
        }
        self.toggle_active_paint(cx);
    }

    fn toggle_active_paint(&mut self, cx: &mut Context<Self>) {
        if self.paint_on_fill {
            self.style.fill = if self.style.fill.is_some() {
                None
            } else {
                Some(self.fill_memory)
            };
        } else {
            self.style.stroke = if self.style.stroke.is_some() {
                None
            } else {
                Some(self.stroke_memory)
            };
        }
        self.commit_style(cx);
        if self.palette_open {
            self.retarget_palette(cx);
        }
    }

    /// 点中对象后, 色块改成它现在的填充和描边, 包括无.
    pub(crate) fn adopt_object_style(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(obj) = self.doc.as_ref().and_then(|d| d.object(id)).cloned() else {
            return;
        };
        match &obj.kind {
            ObjKind::Path(p) => {
                self.style.fill = p.fill.map(|c| c.rgba);
                if let Some(st) = &p.stroke {
                    self.style.stroke = Some(st.paint.rgba);
                    self.style.width = st.width;
                    self.style.cap = st.cap;
                    self.style.join = st.join;
                } else {
                    self.style.stroke = None;
                }
            }
            ObjKind::Text(t) => {
                self.style.fill = Some(t.fill.rgba);
                self.style.stroke = None;
            }
            ObjKind::Image(_) => return,
        }
        self.remember_paints();
        self.palette_a = self.active_alpha();
        if self.palette_open {
            self.retarget_palette(cx);
        } else {
            self.notify_chrome(cx);
        }
    }

    pub(crate) fn retarget_palette(&mut self, cx: &mut Context<Self>) {
        self.palette_a = self.active_alpha();
        let rgb = self.active_rgb().unwrap_or_else(|| self.memory_rgb());
        if self.palette.rgb() != rgb || self.palette.sb.is_none() {
            self.adopt_palette_rgb(rgb);
        }
        self.sync_rgb_inputs(cx);
        self.notify_chrome(cx);
    }

    pub(crate) fn palette_block(&self, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let inputs = (
            self.rgb_r.clone(),
            self.rgb_g.clone(),
            self.rgb_b.clone(),
            self.rgb_a.clone(),
        );
        palette::palette_panel(
            cx,
            "vpal",
            &self.palette,
            &inputs,
            self.palette_a as f32 / 255.0,
            |this, slot, bounds| this.note_palette_bounds(slot, bounds),
            |this, bounds| this.alpha_track = bounds,
            |this, x, y, cx| this.palette_sb_down(x, y, cx),
            |this, y, cx| this.palette_hue_down(y, cx),
            |this, x, cx| this.palette_alpha_down(x, cx),
            |this, color, cx| this.pick_recent(color, cx),
            |this, rgba, cx| this.pick_preset(rgba, cx),
            &PRESET_COLORS,
            |this, x, y, cx| this.palette_pointer_move(x, y, cx),
            |this, cx| this.palette_pointer_up(cx),
        )
    }

    pub(crate) fn palette_pointer_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(DragKind::PaletteSb) => self.palette_sb_at(x, y, cx),
            Some(DragKind::PaletteHue) => self.palette_hue_at(y, cx),
            Some(DragKind::PaletteAlpha) => self.palette_alpha_at(x, cx),
            _ => {}
        }
    }

    pub(crate) fn palette_pointer_up(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.drag,
            Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha)
        ) {
            self.drag = None;
            self.sync_rgb_inputs(cx);
            if !self.selected.is_empty() {
                self.schedule_palette_raster(cx);
            }
            self.notify_chrome(cx);
        }
    }

    pub(crate) fn apply_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        if self.rgb_syncing || !self.palette_open {
            return;
        }
        let mut sample = |input: &Entity<apply_bg::text_input::TextInput>| {
            let blur = input.update(cx, |t, _| t.take_blur_commit());
            let n = input.read(cx).text().trim().parse::<u8>().ok();
            (blur, n)
        };
        let (br, r) = sample(&self.rgb_r);
        let (bg, g) = sample(&self.rgb_g);
        let (bb, b) = sample(&self.rgb_b);
        let (ba, a) = sample(&self.rgb_a);
        let (Some(r), Some(g), Some(b), Some(a)) = (r, g, b, a) else {
            if br || bg || bb || ba {
                self.sync_rgb_inputs(cx);
            }
            return;
        };
        let rgb_same = [r, g, b] == self.palette.rgb();
        if rgb_same && a == self.palette_a {
            return;
        }
        let active_some = self.active_rgb().is_some();
        if rgb_same && !active_some {
            self.palette_a = a;
            if self.paint_on_fill {
                self.fill_memory[3] = a;
            } else {
                self.stroke_memory[3] = a;
            }
            self.notify_chrome(cx);
            return;
        }
        self.palette_a = a;
        if !rgb_same {
            let old = self.palette.set_rgb([r, g, b]);
            self.drop_palette_images(old);
            self.palette_sb_hue = self.palette.h;
        }
        self.commit_palette_rgb([r, g, b], cx);
    }

    fn open_palette(&mut self, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_a = self.active_alpha();
        let rgb = self.active_rgb().unwrap_or_else(|| self.memory_rgb());
        self.adopt_palette_rgb(rgb);
        self.sync_rgb_inputs(cx);
        self.notify_chrome(cx);
    }

    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        if let Some(rgb) = self.active_rgb() {
            self.palette.push_recent(rgb);
        }
        self.palette_open = false;
        self.palette_undo_held = false;
        if matches!(
            self.drag,
            Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha)
        ) {
            self.drag = None;
        }
        self.notify_chrome(cx);
    }

    fn note_palette_bounds(&mut self, slot: PaletteSlot, bounds: Bounds<Pixels>) {
        match slot {
            PaletteSlot::Sb => self.palette.sb_bounds = bounds,
            PaletteSlot::Hue => self.palette.hue_bounds = bounds,
        }
    }

    fn palette_sb_down(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        self.drag = Some(DragKind::PaletteSb);
        self.palette_sb_at(x, y, cx);
    }

    fn palette_hue_down(&mut self, y: f32, cx: &mut Context<Self>) {
        self.drag = Some(DragKind::PaletteHue);
        self.palette_hue_at(y, cx);
    }

    fn palette_alpha_down(&mut self, x: f32, cx: &mut Context<Self>) {
        self.drag = Some(DragKind::PaletteAlpha);
        self.palette_alpha_at(x, cx);
    }

    fn palette_alpha_at(&mut self, x: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.alpha_track.origin.x);
        let w = f32::from(self.alpha_track.size.width).max(1.0);
        let a = ((((x - left) / w).clamp(0.0, 1.0) * 255.0).round() as i32).clamp(0, 255) as u8;
        if a == self.palette_a {
            return;
        }
        self.palette_a = a;
        if self.paint_on_fill {
            self.fill_memory[3] = a;
            if let Some(c) = self.style.fill.as_mut() {
                c[3] = a;
            }
        } else {
            self.stroke_memory[3] = a;
            if let Some(c) = self.style.stroke.as_mut() {
                c[3] = a;
            }
        }
        let has = if self.paint_on_fill {
            self.style.fill.is_some()
        } else {
            self.style.stroke.is_some()
        };
        if has && !self.selected.is_empty() {
            if !(self.palette_open && self.palette_undo_held) {
                self.push_undo();
                self.palette_undo_held = true;
            }
            tools::write_style(self);
            self.schedule_palette_raster(cx);
        }
        self.notify_chrome(cx);
    }

    fn palette_sb_at(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let before = (self.palette.s, self.palette.v);
        let rgb = self.palette.sb_at(x, y);
        if (self.palette.s - before.0).abs() < 1e-4 && (self.palette.v - before.1).abs() < 1e-4 {
            return;
        }
        self.preview_palette_rgb(rgb, cx);
    }

    fn palette_hue_at(&mut self, y: f32, cx: &mut Context<Self>) {
        let prev_h = self.palette.h;
        let rgb = self.palette.hue_at(y);
        if (self.palette.h - prev_h).abs() < 1e-3 {
            return;
        }
        let hue_for_map = self.palette_sb_hue;
        if self.palette.sb.is_none()
            || !hue_for_map.is_finite()
            || (self.palette.h - hue_for_map).abs() >= 1.0
        {
            if let Some(old) = self.palette.rebuild_sb() {
                self.gpu_drop.push(old);
            }
            self.palette_sb_hue = self.palette.h;
        }
        self.preview_palette_rgb(rgb, cx);
    }

    fn pick_recent(&mut self, color: [u8; 3], cx: &mut Context<Self>) {
        self.adopt_palette_rgb(color);
        self.palette.push_recent(color);
        self.preview_palette_rgb(color, cx);
        self.sync_rgb_inputs(cx);
    }

    fn pick_preset(&mut self, rgba: [u8; 4], cx: &mut Context<Self>) {
        self.set_active_paint(Some(rgba), cx);
        if self.palette_open {
            self.retarget_palette(cx);
        }
    }

    fn adopt_palette_rgb(&mut self, rgb: [u8; 3]) {
        let old = self.palette.set_rgb(rgb);
        self.drop_palette_images(old);
        self.palette_sb_hue = self.palette.h;
    }

    /// 拖动时只改颜色和指针, 整页光栅丢到后台, 松手再把 RGB 数字补上.
    fn preview_palette_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let rgba = Some([rgb[0], rgb[1], rgb[2], self.palette_a]);
        let changed = if self.paint_on_fill {
            self.style.fill != rgba
        } else {
            self.style.stroke != rgba
        };
        if !changed {
            self.notify_chrome(cx);
            return;
        }
        if self.paint_on_fill {
            self.style.fill = rgba;
        } else {
            self.style.stroke = rgba;
        }
        self.remember_paints();
        let dragging = matches!(
            self.drag,
            Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha)
        );
        if !self.selected.is_empty() {
            if !(self.palette_open && self.palette_undo_held) {
                self.push_undo();
                if self.palette_open {
                    self.palette_undo_held = true;
                }
            }
            tools::write_style(self);
            if dragging {
                self.schedule_palette_raster(cx);
            } else {
                self.rebuild_raster(cx);
            }
        }
        self.notify_chrome(cx);
    }

    fn schedule_palette_raster(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() || self.doc.is_none() || !self.view_ready() {
            return;
        }
        if self.palette_paint_job {
            self.palette_paint_again = true;
            return;
        }
        self.palette_paint_job = true;
        self.palette_paint_again = false;
        let Some(doc) = self.doc.clone() else {
            self.palette_paint_job = false;
            return;
        };
        let plan = self.raster_plan();
        let gen = self.raster_gen;
        cx.spawn(async move |this, cx| {
            let img = cx
                .background_spawn(async move {
                    let tex_w = (plan.w * plan.px).round().max(1.0) as u32;
                    let tex_h = (plan.h * plan.px).round().max(1.0) as u32;
                    crate::raster::rasterize_rect(&doc, plan.x, plan.y, plan.w, plan.h, tex_w, tex_h)
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                this.palette_paint_job = false;
                if this.raster_gen == gen {
                    if let Some(img) = img {
                        if let Some(t) = this.base_tex.take() {
                            this.gpu_drop.push(t.tex);
                        }
                        this.raster_px = plan.px;
                        this.base_tex = Some(super::TexSprite {
                            tex: super::upload_rgba(&img),
                            doc_x: plan.x,
                            doc_y: plan.y,
                            doc_w: plan.w,
                            doc_h: plan.h,
                        });
                    }
                }
                if this.palette_paint_again {
                    this.schedule_palette_raster(cx);
                }
                this.notify_chrome(cx);
            })
            .ok();
        })
        .detach();
    }

    fn commit_palette_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        self.preview_palette_rgb(rgb, cx);
        if !matches!(
            self.drag,
            Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha)
        ) {
            self.sync_rgb_inputs(cx);
        }
    }

    fn commit_style(&mut self, cx: &mut Context<Self>) {
        self.remember_paints();
        if !self.selected.is_empty() {
            if !(self.palette_open && self.palette_undo_held) {
                self.push_undo();
                if self.palette_open {
                    self.palette_undo_held = true;
                }
            }
            tools::write_style(self);
            self.rebuild_raster(cx);
        }
        self.notify_chrome(cx);
    }

    fn active_rgb(&self) -> Option<[u8; 3]> {
        let rgba = if self.paint_on_fill {
            self.style.fill
        } else {
            self.style.stroke
        }?;
        Some([rgba[0], rgba[1], rgba[2]])
    }

    pub(crate) fn active_alpha(&self) -> u8 {
        let rgba = if self.paint_on_fill {
            self.style.fill
        } else {
            self.style.stroke
        };
        rgba.map(|c| c[3]).unwrap_or(self.memory_rgb_full()[3])
    }

    fn memory_rgb_full(&self) -> [u8; 4] {
        if self.paint_on_fill {
            self.fill_memory
        } else {
            self.stroke_memory
        }
    }

    fn sync_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        let [r, g, b] = self.palette.rgb();
        self.rgb_syncing = true;
        self.rgb_r.update(cx, |t, cx| t.set_text(r.to_string(), cx));
        self.rgb_g.update(cx, |t, cx| t.set_text(g.to_string(), cx));
        self.rgb_b.update(cx, |t, cx| t.set_text(b.to_string(), cx));
        self.rgb_a
            .update(cx, |t, cx| t.set_text(self.palette_a.to_string(), cx));
        self.rgb_syncing = false;
    }

    fn drop_palette_images(&mut self, imgs: Vec<Arc<RenderImage>>) {
        self.gpu_drop.extend(imgs);
    }

    fn picker_pop_w() -> f32 {
        (SB_SIZE + HUE_BAR_W + 8.0 + 16.0 + 8.0).max(240.0)
    }

    fn picker_pop_h() -> f32 {
        16.0 + 18.0 + 8.0 + SB_SIZE + 8.0 + 22.0 + 8.0 + 22.0 + 4.0
    }

    /// 32x32 色块里点中的是填充还是描边. 叠在上面的那块优先.
    fn proxy_hit(&self, x: f32, y: f32) -> Option<bool> {
        let b = self.proxy_bounds;
        let lx = x - f32::from(b.origin.x);
        let ly = y - f32::from(b.origin.y);
        if lx < 0.0 || ly < 0.0 || lx > 32.0 || ly > 32.0 {
            return None;
        }
        let in_fill = lx <= 20.0 && ly <= 20.0;
        let in_stroke = lx >= 12.0 && ly >= 12.0 && lx <= 32.0 && ly <= 32.0;
        if self.paint_on_fill {
            if in_fill {
                Some(true)
            } else if in_stroke {
                Some(false)
            } else {
                None
            }
        } else if in_stroke {
            Some(false)
        } else if in_fill {
            Some(true)
        } else {
            None
        }
    }

    fn color_picker_placement(&self) -> Option<(f32, f32, bool, f32)> {
        let layer = self.picker_layer_bounds;
        let pw = f32::from(layer.size.width);
        let ph = f32::from(layer.size.height);
        if pw < 8.0 || ph < 8.0 {
            return None;
        }
        let anchor = self.proxy_bounds;
        let aw = f32::from(anchor.size.width);
        let ah = f32::from(anchor.size.height);
        if aw < 1.0 || ah < 1.0 {
            return None;
        }
        let pop_w = Self::picker_pop_w();
        let pop_h = Self::picker_pop_h();
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

    fn point_in_picker(&self, x: f32, y: f32) -> bool {
        let Some((left, top, _, _)) = self.color_picker_placement() else {
            return false;
        };
        let layer = self.picker_layer_bounds;
        let ox = f32::from(layer.origin.x) + left;
        let oy = f32::from(layer.origin.y) + top;
        let w = Self::picker_pop_w();
        let h = Self::picker_pop_h() + 8.0;
        x >= ox && x <= ox + w && y >= oy && y <= oy + h
    }

    fn picker_caret(place_below: bool, caret_x: f32) -> impl IntoElement {
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
                        window.paint_path(path, ui_kit::chrome(0x1e293b));
                    }
                }
            })
            .absolute()
            .size_full(),
        )
    }

    pub(crate) fn color_picker_floating(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let placed = self.color_picker_placement();
        let (left, top, place_below, caret_x) = placed.unwrap_or((0.0, 0.0, true, 0.0));
        let pop_w = Self::picker_pop_w();
        div()
            .id("vector_color_picker_layer")
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
                                    || (f32::from(prev.origin.x) - f32::from(bounds.origin.x)).abs()
                                        > 0.5
                                    || (f32::from(prev.origin.y) - f32::from(bounds.origin.y)).abs()
                                        > 0.5
                                    || (f32::from(prev.size.width) - f32::from(bounds.size.width))
                                        .abs()
                                        > 0.5
                                    || (f32::from(prev.size.height) - f32::from(bounds.size.height))
                                        .abs()
                                        > 0.5;
                                if changed && this.palette_open {
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
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    cx.stop_propagation();
                    if this.proxy_hit(x, y).is_some() || this.point_in_picker(x, y) {
                        return;
                    }
                    this.close_palette(cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    if matches!(
                        this.drag,
                        Some(DragKind::PaletteSb | DragKind::PaletteHue | DragKind::PaletteAlpha)
                    ) {
                        this.palette_pointer_up(cx);
                        return;
                    }
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    if let Some(fill) = this.proxy_hit(x, y) {
                        this.on_paint_proxy(fill, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    if let Some(fill) = this.proxy_hit(x, y) {
                        this.on_paint_proxy_right(fill, cx);
                        return;
                    }
                    if this.point_in_picker(x, y) {
                        return;
                    }
                    this.close_palette(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    if this.proxy_hit(x, y).is_none() {
                        return;
                    }
                    cx.stop_propagation();
                    this.swap_paint(cx);
                }),
            )
            .when(placed.is_some(), |d| {
                d.child(
                    div()
                        .id("vector_color_picker_float")
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
                        .child(self.palette_block(cx))
                        .when(!place_below, |d| d.child(Self::picker_caret(false, caret_x))),
                )
            })
    }
}
