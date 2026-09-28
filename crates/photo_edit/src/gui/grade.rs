//! 滤镜 / 调整: 右侧面板状态, 以及写回当前图层.
//!
//! 拖动条每帧只重算屏幕上看得见的那一块 (最多约一百万像素), 贴一张预览.
//! 松手再对原图做一次全分辨率写回. 这样拖的时候不用反复上传整张大图.

use std::sync::Arc;

use gpui::{Context, RenderImage, SharedString, Window};
use image::RgbaImage;

use super::*;
use crate::process::{
    apply_grade, apply_look, render_grade_preview, FilterAmt, Look, PixelGate, PixelSpace, Tone,
};

const PREVIEW_MAX_SIDE: f32 = 1600.0;
const PREVIEW_MAX_PIXELS: f32 = 1_000_000.0;

pub(crate) struct TonePreview {
    pub layer_id: String,
    pub tex: Arc<RenderImage>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    tone: Tone,
    filter: FilterAmt,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
}

impl PhotoEditApp {
    pub(crate) fn reset_grade_ui(&mut self) {
        self.grade_pane = GradePane::Off;
        self.abandon_tone();
    }

    /// 结束这一轮调整. 像素留在当前值, 拖动条回到 0, 下次再拖是新的基准.
    pub(crate) fn finish_tone(&mut self) {
        self.commit_live_tone();
        self.tone = Tone::default();
        self.filter_amt = FilterAmt::default();
        self.tone_base = None;
        self.tone_layer_id = None;
        self.tone_dirty = false;
    }

    fn abandon_tone(&mut self) {
        self.tone_preview_due = false;
        self.tone_dirty = false;
        self.drop_tone_preview();
        self.tone = Tone::default();
        self.filter_amt = FilterAmt::default();
        self.tone_base = None;
        self.tone_layer_id = None;
    }

    fn grade_is_neutral(&self) -> bool {
        self.tone.is_neutral() && self.filter_amt.is_neutral()
    }

    pub(crate) fn toggle_grade(&mut self, pane: GradePane, cx: &mut Context<Self>) {
        self.grade_pane = if self.grade_pane == pane {
            GradePane::Off
        } else {
            pane
        };
        self.notify_chrome(cx);
    }

    pub(crate) fn apply_look(&mut self, look: Look, cx: &mut Context<Self>) {
        if self.doc.as_ref().and_then(|d| d.active_layer()).is_none() {
            self.status = "没有图层".into();
            self.notify_chrome(cx);
            return;
        }
        if !self.selection.is_none() && !self.selection_hits_active() {
            self.status = "选区不在当前图层上".into();
            self.notify_chrome(cx);
            return;
        }
        self.push_undo();
        self.map_active(|img, space, gate| apply_look(img, look, space, gate));
        self.dirty = true;
        self.refresh_active_layer_tex();
        self.status = format!("已应用{}", look.label()).into();
        self.notify_chrome(cx);
    }

    /// 拖动过程中合并到下一帧, 同一帧里的多次移动只重算一次预览.
    pub(crate) fn schedule_tone_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tone_dirty = true;
        self.tone_preview_due = true;
        if self.tone_preview_queued {
            return;
        }
        self.tone_preview_queued = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.tone_preview_queued = false;
            if !this.tone_preview_due {
                return;
            }
            this.tone_preview_due = false;
            if !matches!(this.drag, Some(DragKind::Slider(kind)) if kind.previews_grade()) {
                return;
            }
            let scale = window.scale_factor().max(1.0);
            this.rebuild_tone_preview(scale);
            this.notify_chrome(cx);
        });
        // 鼠标移动本身不会画一帧. 这里请求一帧, 回调会在绘制前用最新的拖动值重算预览.
        self.notify_chrome(cx);
    }

    /// 松手或离开这一轮调整时, 按当前拖动条把基准图完整写回.
    pub(crate) fn commit_live_tone(&mut self) {
        self.tone_preview_due = false;
        if !self.tone_dirty {
            self.drop_tone_preview();
            return;
        }
        self.drop_tone_preview();
        if !self.ensure_tone_session() {
            self.tone_dirty = false;
            return;
        }
        if self.grade_is_neutral() && self.pixels_match_tone_base() {
            self.tone_dirty = false;
            return;
        }
        let Some(base) = self.tone_base.clone() else {
            self.tone_dirty = false;
            return;
        };
        let tone = self.tone;
        let filter = self.filter_amt;
        let owned = self.owned_gate();
        {
            let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) else {
                self.tone_dirty = false;
                return;
            };
            let space = space_of(layer);
            let gate = gate_of(&owned);
            let img = Arc::make_mut(&mut layer.pixels);
            apply_grade(img, base.as_ref(), tone, filter, space, gate);
        }
        self.tone_dirty = false;
        self.dirty = !self.grade_is_neutral() || self.undo_stack.len() > 1;
        self.refresh_active_layer_tex();
    }

    fn rebuild_tone_preview(&mut self, px_scale: f32) {
        if !self.ensure_tone_session() {
            self.drop_tone_preview();
            return;
        }
        if self.grade_is_neutral() && self.pixels_match_tone_base() {
            self.drop_tone_preview();
            return;
        }
        let Some(layer) = self.doc.as_ref().and_then(|d| d.active_layer()) else {
            return;
        };
        let Some(base) = self.tone_base.clone() else {
            return;
        };
        let layer_id = layer.id.clone();
        let (ox, oy, rot) = (layer.x, layer.y, layer.rotation);
        let (lw, lh) = (layer.width(), layer.height());
        let Some((src_x, src_y, src_w, src_h, out_w, out_h)) =
            self.tone_preview_crop(ox, oy, lw, lh, rot, px_scale)
        else {
            self.drop_tone_preview();
            return;
        };
        if let Some(prev) = &self.tone_preview {
            if prev.layer_id == layer_id
                && prev.tone == self.tone
                && prev.filter == self.filter_amt
                && prev.src_x == src_x
                && prev.src_y == src_y
                && prev.src_w == src_w
                && prev.src_h == src_h
                && prev.out_w == out_w
                && prev.out_h == out_h
            {
                return;
            }
        }
        let owned = self.owned_gate();
        let space = PixelSpace {
            ox,
            oy,
            rot,
            w: lw,
            h: lh,
        };
        let gate = gate_of(&owned);
        let filter = self.filter_amt;
        let image = render_grade_preview(
            base.as_ref(),
            src_x,
            src_y,
            src_w,
            src_h,
            out_w,
            out_h,
            self.tone,
            filter,
            space,
            gate,
        );
        let tex = upload_bgra_rect(&image, 0, 0, image.width(), image.height());
        self.drop_tone_preview();
        self.tone_preview = Some(TonePreview {
            layer_id,
            tex,
            x: src_x as i32,
            y: src_y as i32,
            w: src_w,
            h: src_h,
            tone: self.tone,
            filter,
            src_x,
            src_y,
            src_w,
            src_h,
            out_w,
            out_h,
        });
        self.dirty = !self.grade_is_neutral() || self.undo_stack.len() > 1;
    }

    fn tone_preview_crop(
        &self,
        origin_x: i32,
        origin_y: i32,
        lw: u32,
        lh: u32,
        rot: f32,
        px_scale: f32,
    ) -> Option<(u32, u32, u32, u32, u32, u32)> {
        if lw == 0 || lh == 0 {
            return None;
        }
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        let (src_x, src_y, src_w, src_h, scale) = if vw < 8.0 || vh < 8.0 {
            (0, 0, lw, lh, 1.0)
        } else {
            let xform = self.view_xform();
            let corners = [
                xform.screen_to_image(0.0, 0.0),
                xform.screen_to_image(vw, 0.0),
                xform.screen_to_image(0.0, vh),
                xform.screen_to_image(vw, vh),
            ];
            let cx = origin_x as f32 + lw as f32 * 0.5;
            let cy = origin_y as f32 + lh as f32 * 0.5;
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for (px, py) in corners {
                let (lx, ly) = if rot.abs() < 0.05 {
                    (px - origin_x as f32, py - origin_y as f32)
                } else {
                    let (ux, uy) = crate::geom::rotate_point(px, py, cx, cy, -rot);
                    (ux - origin_x as f32, uy - origin_y as f32)
                };
                min_x = min_x.min(lx);
                min_y = min_y.min(ly);
                max_x = max_x.max(lx);
                max_y = max_y.max(ly);
            }
            let pad = 4.0 / xform.scale.max(0.0001);
            min_x -= pad;
            min_y -= pad;
            max_x += pad;
            max_y += pad;
            let x0 = min_x.floor().clamp(0.0, lw as f32) as u32;
            let y0 = min_y.floor().clamp(0.0, lh as f32) as u32;
            let x1 = max_x.ceil().clamp(0.0, lw as f32) as u32;
            let y1 = max_y.ceil().clamp(0.0, lh as f32) as u32;
            let sw = x1.saturating_sub(x0);
            let sh = y1.saturating_sub(y0);
            if sw == 0 || sh == 0 {
                return None;
            }
            (x0, y0, sw, sh, xform.scale)
        };
        let dpi = px_scale.max(1.0);
        let mut ow = ((src_w as f32) * scale * dpi)
            .round()
            .max(1.0)
            .min(src_w as f32);
        let mut oh = ((src_h as f32) * scale * dpi)
            .round()
            .max(1.0)
            .min(src_h as f32);
        let side = ow.max(oh);
        if side > PREVIEW_MAX_SIDE {
            let k = PREVIEW_MAX_SIDE / side;
            ow *= k;
            oh *= k;
        }
        if ow * oh > PREVIEW_MAX_PIXELS {
            let k = (PREVIEW_MAX_PIXELS / (ow * oh)).sqrt();
            ow *= k;
            oh *= k;
        }
        Some((
            src_x,
            src_y,
            src_w,
            src_h,
            ow.round().max(1.0) as u32,
            oh.round().max(1.0) as u32,
        ))
    }

    fn ensure_tone_session(&mut self) -> bool {
        let Some(layer_id) = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer())
            .map(|l| l.id.clone())
        else {
            return false;
        };
        if self.tone_layer_id.as_ref() != Some(&layer_id) {
            self.drop_tone_preview();
            self.tone_base = None;
            self.tone_layer_id = None;
            self.tone_dirty = !self.grade_is_neutral();
        }
        if self.tone_base.is_none() {
            if self.grade_is_neutral() {
                return false;
            }
            if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
                layer.bake_strokes();
            }
            self.tone_applying = true;
            self.push_undo();
            self.tone_applying = false;
            self.tone_base = self
                .doc
                .as_ref()
                .and_then(|d| d.active_layer())
                .map(|l| l.pixels.clone());
            self.tone_layer_id = Some(layer_id);
        }
        self.tone_base.is_some()
    }

    fn pixels_match_tone_base(&self) -> bool {
        let (Some(base), Some(doc)) = (&self.tone_base, self.doc.as_ref()) else {
            return false;
        };
        let Some(layer) = doc.active_layer() else {
            return false;
        };
        Arc::ptr_eq(&layer.pixels, base)
    }

    fn drop_tone_preview(&mut self) {
        if let Some(prev) = self.tone_preview.take() {
            self.gpu_drop.push(prev.tex);
        }
    }

    fn selection_hits_active(&self) -> bool {
        let Some(doc) = self.doc.as_ref() else {
            return false;
        };
        let Some(layer) = doc.active_layer() else {
            return false;
        };
        let Some((sx0, sy0, sx1, sy1)) = self.selection.bounds(doc.canvas_w, doc.canvas_h) else {
            return false;
        };
        let (lx0, ly0, lx1, ly1) = if layer.rotation.abs() < 0.05 {
            (
                layer.x,
                layer.y,
                layer.x + layer.width() as i32 - 1,
                layer.y + layer.height() as i32 - 1,
            )
        } else {
            let (a, b, c, d) = crate::geom::rotated_aabb(
                layer.x as f32,
                layer.y as f32,
                layer.width() as f32,
                layer.height() as f32,
                layer.rotation,
            );
            (
                a.floor() as i32,
                b.floor() as i32,
                c.ceil() as i32,
                d.ceil() as i32,
            )
        };
        sx1 >= lx0 && sx0 <= lx1 && sy1 >= ly0 && sy0 <= ly1
    }

    fn owned_gate(&self) -> OwnedGate {
        let Some(doc) = self.doc.as_ref() else {
            return OwnedGate::All;
        };
        match &self.selection {
            Selection::None => OwnedGate::All,
            Selection::Rect { .. } => match self.selection.bounds(doc.canvas_w, doc.canvas_h) {
                Some((x0, y0, x1, y1)) => OwnedGate::Rect { x0, y0, x1, y1 },
                None => OwnedGate::All,
            },
            Selection::Mask { data, .. } => OwnedGate::Mask {
                data: data.clone(),
                canvas_w: doc.canvas_w,
            },
        }
    }

    fn map_active(&mut self, f: impl FnOnce(&mut RgbaImage, PixelSpace, PixelGate<'_>)) {
        let owned = self.owned_gate();
        let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) else {
            return;
        };
        layer.bake_strokes();
        let space = space_of(layer);
        let gate = gate_of(&owned);
        let img = Arc::make_mut(&mut layer.pixels);
        f(img, space, gate);
    }
}

enum OwnedGate {
    All,
    Rect { x0: i32, y0: i32, x1: i32, y1: i32 },
    Mask { data: Arc<Vec<u8>>, canvas_w: u32 },
}

fn space_of(layer: &crate::document::Layer) -> PixelSpace {
    PixelSpace {
        ox: layer.x,
        oy: layer.y,
        rot: layer.rotation,
        w: layer.width(),
        h: layer.height(),
    }
}

fn gate_of(owned: &OwnedGate) -> PixelGate<'_> {
    match owned {
        OwnedGate::All => PixelGate::All,
        OwnedGate::Rect { x0, y0, x1, y1 } => PixelGate::Rect {
            x0: *x0,
            y0: *y0,
            x1: *x1,
            y1: *y1,
        },
        OwnedGate::Mask { data, canvas_w } => PixelGate::Mask {
            data: data.as_slice(),
            canvas_w: *canvas_w,
        },
    }
}

pub(crate) fn tone_caption(index: usize, value: f32) -> SharedString {
    let name = crate::process::TONE_LABELS
        .get(index)
        .copied()
        .unwrap_or("调整");
    let n = (value * 100.0).round() as i32;
    format!("{name} {n:+}").into()
}

pub(crate) fn filter_caption(index: usize, value: f32) -> String {
    let name = crate::process::FILTER_LABELS
        .get(index)
        .copied()
        .unwrap_or("滤镜");
    let n = (value * 100.0).round() as i32;
    format!("{name} {n}")
}
