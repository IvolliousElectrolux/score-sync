use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;

use super::*;
use crate::document::{EraserHits, Layer, PaintStroke};
use crate::process::{
    canvas_to_layer_xy, combine_masks, cut_mask_from_layer, cut_rect_from_layer,
    extract_mask_layer, extract_rect_layer, fill_poly_mask, heal_disk, lift_layer_mask_to_canvas,
    mask_loops, pick_heal_offset, raster_selection, remap_mask_bounds, resize_rgba, rotate_rgba,
    scale_outline, stamp_color, stamp_from_soft, wand_mask_rgba, SelCombine, Selection, WandOpts,
};

impl PhotoEditApp {
    pub fn on_escape(&mut self, cx: &mut Context<Self>) {
        if self.eyedropper_armed {
            self.cancel_eyedropper(cx);
            return;
        }
        if self.color_picker_open {
            self.close_color_picker(cx);
            return;
        }
        if self.lasso_draft.is_some() {
            self.cancel_lasso_draft(cx);
            return;
        }
        if !self.selection.is_none() {
            self.selection = Selection::None;
            self.notify_chrome(cx);
            return;
        }
        if self.mode != ToolMode::Select {
            self.set_mode(ToolMode::Select, cx);
            return;
        }
        self.request_cancel(cx);
    }

    pub fn delete_active_layer(&mut self, cx: &mut Context<Self>) {
        let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        if n <= 1 {
            self.status = "至少保留一个图层".into();
            self.notify_chrome(cx);
            return;
        }
        self.push_undo();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let i = doc.active.min(doc.layers.len() - 1);
        doc.layers.remove(i);
        doc.set_active(i.saturating_sub(1));
        self.dirty = true;
        self.rebuild_preview();
        self.status = "已删除图层".into();
        self.notify_chrome(cx);
    }

    pub fn new_layer_from_selection(&mut self, cx: &mut Context<Self>) {
        if matches!(self.selection, Selection::None) {
            self.status = "先框选、魔棒或套索".into();
            self.notify_chrome(cx);
            return;
        }
        self.push_undo();
        self.bake_active_rotation();
        if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
            layer.bake_strokes();
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(layer) = doc.layers.get(doc.active).cloned() else {
            return;
        };
        let cw = doc.canvas_w;
        let ch = doc.canvas_h;
        let paper = Some(doc.paper_rgb);
        let extracted = match &self.selection {
            Selection::None => None,
            Selection::Rect { x0, y0, x1, y1 } => Some(extract_rect_layer(
                &layer.pixels,
                layer.x,
                layer.y,
                *x0,
                *y0,
                *x1,
                *y1,
            )),
            Selection::Mask { data: m, .. } => {
                extract_mask_layer(&layer.pixels, layer.x, layer.y, m, cw, ch)
                    .map(|(img, x, y)| (img, x, y))
            }
        };
        let Some((pixels, x, y)) = extracted else {
            self.status = "选区是空的".into();
            self.notify_chrome(cx);
            return;
        };
        let doc = self.doc.as_mut().unwrap();
        let layer = doc.layers.get_mut(doc.active).unwrap();
        let pixels_mut = Arc::make_mut(&mut layer.pixels);
        match &self.selection {
            Selection::Rect { x0, y0, x1, y1 } => {
                cut_rect_from_layer(pixels_mut, layer.x, layer.y, *x0, *y0, *x1, *y1, paper);
            }
            Selection::Mask { data: m, .. } => {
                cut_mask_from_layer(pixels_mut, layer.x, layer.y, m, cw, ch, paper);
            }
            Selection::None => {}
        }
        let mut neu = Layer::new(format!("选区 {}", doc.layers.len()), pixels);
        neu.x = x;
        neu.y = y;
        doc.layers.push(neu);
        doc.active = doc.layers.len() - 1;
        self.selection = Selection::None;
        self.dirty = true;
        self.rebuild_preview();
        self.status = "已从选区新建图层".into();
        self.notify_chrome(cx);
    }

    pub fn nudge(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        if self.mode != ToolMode::Move && self.mode != ToolMode::Select {
            return;
        }
        self.push_undo();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        if let Some(layer) = doc.active_layer_mut() {
            layer.x += dx;
            layer.y += dy;
        }
        self.dirty = true;
        self.rebuild_preview();
        self.notify_chrome(cx);
    }

    pub(crate) fn bake_active_rotation(&mut self) {
        let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) else {
            return;
        };
        if layer.rotation.abs() < 0.05 {
            layer.rotation = 0.0;
            return;
        }
        layer.bake_strokes();
        let deg = layer.rotation;
        let (cx, cy) = layer.center();
        let img = rotate_rgba(&layer.pixels, deg);
        let nw = img.width() as f32;
        let nh = img.height() as f32;
        layer.pixels = Arc::new(img);
        layer.x = (cx - nw * 0.5).round() as i32;
        layer.y = (cy - nh * 0.5).round() as i32;
        layer.rotation = 0.0;
        self.dirty = true;
        self.refresh_active_layer_tex();
    }

    pub(crate) fn commit_layer_rotate(&mut self, deg: f32) {
        if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
            let mut r = layer.rotation + deg;
            r = (r + 180.0).rem_euclid(360.0) - 180.0;
            if r.abs() < 0.05 {
                r = 0.0;
            }
            layer.rotation = r;
        }
        self.dirty = true;
    }

    pub(crate) fn begin_brush_stroke(&mut self, ix: i32, iy: i32) {
        self.push_undo();
        self.bake_active_rotation();
        self.brush_last = None;
        if self.mode == ToolMode::Paint {
            self.begin_paint_stroke();
        }
        if matches!(self.mode, ToolMode::Clone | ToolMode::Heal) {
            if self.mode == ToolMode::Clone {
                if let Some((sx, sy)) = self.clone_src {
                    if self.clone_offset.is_none() {
                        self.clone_offset = Some((sx - ix, sy - iy));
                    }
                }
            }
            if self.mode == ToolMode::Heal {
                self.heal_offset = None;
            }
            if let Some(layer) = self.doc.as_ref().and_then(|d| d.active_layer()) {
                self.clone_snap = Some(layer.pixels.clone());
            }
        }
        self.paint_brush_segment(ix as f32, iy as f32, true);
    }

    pub(crate) fn paint_brush_segment(&mut self, ix: f32, iy: f32, force: bool) {
        let spacing = (self.brush * BRUSH_SPACING_FRAC).max(1.0);
        let r = (self.brush * 0.5).round() as i32;
        let layer_info = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer())
            .map(|l| (l.x, l.y, l.width(), l.height()));
        let mut tiles: Vec<(u32, u32)> = Vec::new();
        let mut mark = |x: i32, y: i32| {
            let Some((ox, oy, lw, lh)) = layer_info else {
                return;
            };
            let lx = x - ox;
            let ly = y - oy;
            // 盖章半径再加贴图渗色, 只上传真正碰到的块, 不把斜向拖动的包围盒整片重传.
            let pad = r.max(1) + 1 + TILE_BLEED as i32;
            push_upload_tiles(
                &mut tiles,
                lx - pad,
                ly - pad,
                lx + pad + 1,
                ly + pad + 1,
                lw,
                lh,
            );
        };
        if force || self.brush_last.is_none() {
            let x = ix.round() as i32;
            let y = iy.round() as i32;
            if self.paint_brush_at(x, y) {
                mark(x, y);
            }
            self.brush_last = Some((ix, iy));
        } else if let Some((x0, y0)) = self.brush_last {
            let dx = ix - x0;
            let dy = iy - y0;
            let dist = dx.hypot(dy);
            if dist < spacing {
                return;
            }
            let steps = (dist / spacing).floor() as i32;
            if steps < 1 {
                return;
            }
            let ux = dx / dist;
            let uy = dy / dist;
            let mut x = x0;
            let mut y = y0;
            for _ in 0..steps {
                x += ux * spacing;
                y += uy * spacing;
                let px = x.round() as i32;
                let py = y.round() as i32;
                if self.paint_brush_at(px, py) {
                    mark(px, py);
                }
            }
            self.brush_last = Some((x, y));
        }
        if tiles.is_empty() {
            return;
        }
        tiles.sort_unstable();
        tiles.dedup();
        if let Some(index) = self.doc.as_ref().map(|d| d.active) {
            self.refresh_layer_tiles(index, &tiles);
        }
    }

    pub(crate) fn paint_brush_at(&mut self, ix: i32, iy: i32) -> bool {
        let r = (self.brush * 0.5).round() as i32;
        let hard = self.brush_hardness;
        let clone_offset = self.clone_offset;
        let snap = self.clone_snap.clone();
        let mode = self.mode;
        if mode == ToolMode::Heal && self.heal_offset.is_none() {
            if let (Some(src), Some(layer)) = (
                snap.as_ref(),
                self.doc.as_ref().and_then(|d| d.active_layer()),
            ) {
                self.heal_offset = pick_heal_offset(src, ix - layer.x, iy - layer.y, r);
            }
        }
        let heal_offset = self.heal_offset;
        let paint_color = self.paint_color;
        if mode == ToolMode::Paint {
            return self.paint_trace_at(ix, iy, r, hard, paint_color);
        }
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        let Some(layer) = doc.active_layer_mut() else {
            return false;
        };
        let lx = ix - layer.x;
        let ly = iy - layer.y;
        let layer_x = layer.x;
        let layer_y = layer.y;
        let img = Arc::make_mut(&mut layer.pixels);
        match mode {
            ToolMode::Heal => {
                let Some(src) = snap.as_ref() else {
                    return false;
                };
                let Some(off) = heal_offset else {
                    return false;
                };
                heal_disk(img, src, lx, ly, r, hard, off);
            }
            ToolMode::Clone => {
                let Some((ox, oy)) = clone_offset else {
                    return false;
                };
                let Some(src) = snap.as_ref() else {
                    return false;
                };
                stamp_from_soft(
                    img,
                    src,
                    lx,
                    ly,
                    ix + ox - layer_x,
                    iy + oy - layer_y,
                    r,
                    hard,
                );
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    fn begin_paint_stroke(&mut self) {
        let radius = (self.brush * 0.5).round().max(1.0) as i32;
        let hardness = self.brush_hardness;
        let color = self.paint_color;
        let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) else {
            return;
        };
        layer.strokes.push(PaintStroke {
            points: Vec::new(),
            radius,
            hardness,
            color,
        });
    }

    pub(crate) fn drop_empty_paint_stroke(&mut self) {
        if self.mode != ToolMode::Paint {
            return;
        }
        let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) else {
            return;
        };
        if layer.strokes.last().is_some_and(|s| s.points.is_empty()) {
            layer.strokes.pop();
        }
    }

    fn paint_trace_at(&mut self, ix: i32, iy: i32, r: i32, hard: f32, color: [u8; 3]) -> bool {
        let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) else {
            return false;
        };
        let (lx, ly) = canvas_to_layer_xy(
            ix,
            iy,
            layer.x,
            layer.y,
            layer.width(),
            layer.height(),
            layer.rotation,
        );
        if layer.strokes.is_empty() {
            layer.strokes.push(PaintStroke {
                points: Vec::new(),
                radius: r.max(1),
                hardness: hard,
                color,
            });
        }
        let stroke = layer.strokes.last_mut().unwrap();
        let radius = stroke.radius.max(1);
        let hardness = stroke.hardness;
        let rgb = stroke.color;
        stroke.points.push((lx, ly));
        let w = layer.width();
        let h = layer.height();
        if layer
            .ink
            .as_ref()
            .is_none_or(|im| im.width() != w || im.height() != h)
        {
            layer.ink = Some(Arc::new(RgbaImage::new(w, h)));
        }
        let ink = Arc::make_mut(layer.ink.as_mut().unwrap());
        stamp_color(ink, lx, ly, radius, hardness, rgb);
        layer.stroke_rev = layer.stroke_rev.wrapping_add(1);
        self.dirty = true;
        true
    }

    fn eraser_radius(&self) -> i32 {
        (self.brush * 0.5).round().max(1.0) as i32
    }

    pub(crate) fn erase_hit_status(&self) -> String {
        match self.erase_target {
            EraseTarget::Layer => "已擦除图层".into(),
            EraseTarget::Trace => match self.trace_erase {
                TraceErase::Dab => "已擦除痕迹".into(),
                TraceErase::Stroke => "已擦除整笔".into(),
            },
        }
    }

    /// 单击: 只动最上层碰到的对象.
    pub(crate) fn erase_top_at(&mut self, ix: f32, iy: f32) -> bool {
        let target = self.erase_target;
        let trace = self.trace_erase;
        match target {
            EraseTarget::Trace => match trace {
                TraceErase::Dab => self.erase_trace_dabs(&[(ix, iy)], true),
                TraceErase::Stroke => self.erase_whole_strokes(&[(ix, iy)], true),
            },
            EraseTarget::Layer => self.erase_layers_at(&[(ix, iy)], true),
        }
    }

    /// 拖擦: 沿路径擦碰到的痕迹或图层. `force` 先落在起点.
    pub(crate) fn erase_all_segment(&mut self, ix: f32, iy: f32, force: bool) -> bool {
        let points = self.take_erase_points(ix, iy, force);
        if points.is_empty() {
            return false;
        }
        let target = self.erase_target;
        let trace = self.trace_erase;
        match target {
            EraseTarget::Trace => match trace {
                TraceErase::Dab => self.erase_trace_dabs(&points, false),
                TraceErase::Stroke => self.erase_whole_strokes(&points, false),
            },
            EraseTarget::Layer => self.erase_layers_at(&points, false),
        }
    }

    fn take_erase_points(&mut self, ix: f32, iy: f32, force: bool) -> Vec<(f32, f32)> {
        let spacing = (self.brush * BRUSH_SPACING_FRAC).max(1.0);
        if force || self.brush_last.is_none() {
            self.brush_last = Some((ix, iy));
            return vec![(ix, iy)];
        }
        let Some((x0, y0)) = self.brush_last else {
            return Vec::new();
        };
        let dx = ix - x0;
        let dy = iy - y0;
        let dist = dx.hypot(dy);
        if dist < spacing {
            return Vec::new();
        }
        let steps = (dist / spacing).floor() as i32;
        if steps < 1 {
            return Vec::new();
        }
        let ux = dx / dist;
        let uy = dy / dist;
        let mut x = x0;
        let mut y = y0;
        let mut points = Vec::with_capacity(steps as usize);
        for _ in 0..steps {
            x += ux * spacing;
            y += uy * spacing;
            points.push((x, y));
        }
        self.brush_last = Some((x, y));
        points
    }

    fn layer_local_points(layer: &Layer, points: &[(f32, f32)]) -> Vec<(i32, i32)> {
        points
            .iter()
            .map(|&(ix, iy)| {
                canvas_to_layer_xy(
                    ix.round() as i32,
                    iy.round() as i32,
                    layer.x,
                    layer.y,
                    layer.width(),
                    layer.height(),
                    layer.rotation,
                )
            })
            .collect()
    }

    fn erase_trace_dabs(&mut self, points: &[(f32, f32)], top_only: bool) -> bool {
        let er = self.eraser_radius();
        let mut uploads = Vec::new();
        let mut any = false;
        {
            let Some(doc) = self.doc.as_mut() else {
                return false;
            };
            let n = doc.layers.len();
            let targets: Vec<usize> = if top_only {
                let mut found = None;
                for i in (0..n).rev() {
                    let layer = &doc.layers[i];
                    if !layer.visible {
                        continue;
                    }
                    let local = Self::layer_local_points(layer, points);
                    let hits = EraserHits::new(&local, er);
                    if layer.strokes.iter().any(|s| hits.hits_stroke(s)) {
                        found = Some(i);
                        break;
                    }
                }
                found.into_iter().collect()
            } else {
                (0..n).filter(|&i| doc.layers[i].visible).collect()
            };
            for i in targets {
                let local = Self::layer_local_points(&doc.layers[i], points);
                if local.is_empty() {
                    continue;
                }
                let hits = EraserHits::new(&local, er);
                let lw = doc.layers[i].width();
                let lh = doc.layers[i].height();
                let layer = &mut doc.layers[i];
                let mut next = Vec::with_capacity(layer.strokes.len());
                let mut clips: HashMap<(u32, u32), (i32, i32, i32, i32)> = HashMap::new();
                let mut changed = false;
                for stroke in layer.strokes.drain(..) {
                    let radius = stroke.radius;
                    match stroke.split_erased_hits(&hits) {
                        None => next.push(stroke),
                        Some((parts, removed)) => {
                            changed = true;
                            for (px, py) in removed {
                                cover_disk(&mut clips, px, py, radius, lw, lh);
                            }
                            next.extend(parts);
                        }
                    }
                }
                layer.strokes = next;
                if !changed {
                    continue;
                }
                any = true;
                uploads.push((i, ink_after_erase(layer, &clips)));
            }
        }
        self.finish_ink_uploads(any, uploads);
        any
    }

    fn erase_whole_strokes(&mut self, points: &[(f32, f32)], top_only: bool) -> bool {
        let er = self.eraser_radius();
        let mut uploads = Vec::new();
        let mut any = false;
        {
            let Some(doc) = self.doc.as_mut() else {
                return false;
            };
            let n = doc.layers.len();
            if top_only {
                for i in (0..n).rev() {
                    if !doc.layers[i].visible {
                        continue;
                    }
                    let local = Self::layer_local_points(&doc.layers[i], points);
                    let hits = EraserHits::new(&local, er);
                    let Some(si) = doc.layers[i].strokes.iter().enumerate().rev().find_map(
                        |(si, s)| hits.hits_stroke(s).then_some(si),
                    ) else {
                        continue;
                    };
                    let removed = doc.layers[i].strokes.remove(si);
                    let layer = &mut doc.layers[i];
                    let mut clips = HashMap::new();
                    cover_stroke(&mut clips, &removed, layer.width(), layer.height());
                    uploads.push((i, ink_after_erase(layer, &clips)));
                    any = true;
                    break;
                }
            } else {
                for i in 0..n {
                    if !doc.layers[i].visible || doc.layers[i].strokes.is_empty() {
                        continue;
                    }
                    let local = Self::layer_local_points(&doc.layers[i], points);
                    let hits = EraserHits::new(&local, er);
                    let lw = doc.layers[i].width();
                    let lh = doc.layers[i].height();
                    let layer = &mut doc.layers[i];
                    let mut next = Vec::with_capacity(layer.strokes.len());
                    let mut clips = HashMap::new();
                    let mut changed = false;
                    for stroke in layer.strokes.drain(..) {
                        if hits.hits_stroke(&stroke) {
                            changed = true;
                            cover_stroke(&mut clips, &stroke, lw, lh);
                        } else {
                            next.push(stroke);
                        }
                    }
                    layer.strokes = next;
                    if !changed {
                        continue;
                    }
                    any = true;
                    uploads.push((i, ink_after_erase(layer, &clips)));
                }
            }
        }
        self.finish_ink_uploads(any, uploads);
        any
    }

    fn finish_ink_uploads(
        &mut self,
        any: bool,
        uploads: Vec<(usize, Option<Vec<(u32, u32)>>)>,
    ) {
        if any {
            self.dirty = true;
        }
        for (i, tiles) in uploads {
            match tiles {
                None => {
                    let (w, h) = self
                        .doc
                        .as_ref()
                        .and_then(|d| d.layers.get(i))
                        .map(|l| (l.width() as i32, l.height() as i32))
                        .unwrap_or((0, 0));
                    self.refresh_layer_dirty(i, 0, 0, w, h);
                }
                Some(tiles) if !tiles.is_empty() => self.refresh_layer_tiles(i, &tiles),
                Some(_) => {}
            }
        }
    }

    fn disk_hits_alpha(img: &RgbaImage, cx: i32, cy: i32, r: i32) -> bool {
        let r = r.max(0);
        let w = img.width() as i32;
        let h = img.height() as i32;
        let x0 = (cx - r).max(0);
        let y0 = (cy - r).max(0);
        let x1 = (cx + r + 1).min(w);
        let y1 = (cy + r + 1).min(h);
        let r2 = i64::from(r) * i64::from(r);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = i64::from(x - cx);
                let dy = i64::from(y - cy);
                if dx * dx + dy * dy <= r2 && img.get_pixel(x as u32, y as u32)[3] > 8 {
                    return true;
                }
            }
        }
        false
    }

    fn layer_eraser_hit(layer: &Layer, points: &[(f32, f32)], er: i32) -> bool {
        if !layer.visible {
            return false;
        }
        let local = Self::layer_local_points(layer, points);
        for (lx, ly) in local {
            if layer.strokes.iter().any(|s| s.overlaps(lx, ly, er)) {
                return true;
            }
            if Self::disk_hits_alpha(layer.pixels.as_ref(), lx, ly, er) {
                return true;
            }
            if let Some(ink) = layer.ink.as_ref() {
                if Self::disk_hits_alpha(ink, lx, ly, er) {
                    return true;
                }
            }
        }
        false
    }

    fn erase_layers_at(&mut self, points: &[(f32, f32)], top_only: bool) -> bool {
        let er = self.eraser_radius();
        let mut hits: Vec<usize> = {
            let Some(doc) = self.doc.as_ref() else {
                return false;
            };
            let mut hits: Vec<usize> = doc
                .layers
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, layer)| Self::layer_eraser_hit(layer, points, er))
                .map(|(i, _)| i)
                .collect();
            if top_only {
                hits.truncate(1);
            }
            if hits.is_empty() {
                return false;
            }
            let n = doc.layers.len();
            if hits.len() >= n {
                if let Some(&keep) = hits.iter().min() {
                    hits.retain(|&i| i != keep);
                }
            }
            hits
        };
        if hits.is_empty() {
            self.status = "至少保留一个图层".into();
            return false;
        }
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        hits.sort_unstable();
        for i in hits.into_iter().rev() {
            doc.layers.remove(i);
        }
        let active = doc.active;
        doc.set_active(active);
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    pub(crate) fn apply_sel_resize(
        &mut self,
        handle: BoxHandle,
        orig: (i32, i32, i32, i32),
        start: (f32, f32),
        ix: f32,
        iy: f32,
    ) -> (i32, i32, i32, i32) {
        let lock = self.last_shift;
        let (x0, y0, x1, y1) = orig;
        let (mut a, mut b, mut c, mut d) = resize_rect(
            x0 as f32, y0 as f32, x1 as f32, y1 as f32, ix, iy, start.0, start.1, handle, lock,
        );
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w as f32, d.canvas_h as f32))
            .unwrap_or((1.0, 1.0));
        let tol = (EDGE_SNAP_SCREEN_PX / self.view_xform().scale.max(0.0001)).max(1.0);
        let xs = [0.0, (cw - 1.0).max(0.0), x0 as f32, x1 as f32];
        let ys = [0.0, (ch - 1.0).max(0.0), y0 as f32, y1 as f32];
        match handle {
            BoxHandle::E | BoxHandle::NE | BoxHandle::SE => {
                c = snap_to_targets(c, &xs, tol);
            }
            BoxHandle::W | BoxHandle::NW | BoxHandle::SW => {
                a = snap_to_targets(a, &xs, tol);
            }
            _ => {}
        }
        match handle {
            BoxHandle::N | BoxHandle::NE | BoxHandle::NW => {
                b = snap_to_targets(b, &ys, tol);
            }
            BoxHandle::S | BoxHandle::SE | BoxHandle::SW => {
                d = snap_to_targets(d, &ys, tol);
            }
            _ => {}
        }
        let na = a.round() as i32;
        let nb = b.round() as i32;
        let nc = c.round() as i32;
        let nd = d.round() as i32;
        (na.min(nc), nb.min(nd), na.max(nc), nb.max(nd))
    }

    pub(crate) fn commit_sel_resize(
        &mut self,
        orig: (i32, i32, i32, i32),
        orig_mask: Option<Arc<Vec<u8>>>,
        cur: (i32, i32, i32, i32),
    ) {
        if orig == cur {
            return;
        }
        if let Some(mask) = orig_mask {
            let (cw, ch) = self
                .doc
                .as_ref()
                .map(|d| (d.canvas_w, d.canvas_h))
                .unwrap_or((1, 1));
            let neu = remap_mask_bounds(&mask, cw, ch, orig, cur);
            if self.selection.loops().is_some() {
                let loops = trace_wand_loops(&neu, cw, ch);
                self.selection = Selection::from_loops(neu, loops, cw, ch);
            } else {
                let outline = self
                    .selection
                    .outline()
                    .map(|pts| scale_outline(pts, orig, cur))
                    .unwrap_or_default();
                self.selection = if outline.len() >= 3 {
                    Selection::from_poly(neu, outline, cw, ch)
                } else {
                    Selection::from_mask(neu, cw, ch)
                };
            }
        } else {
            let (na, nb, nc, nd) = cur;
            self.selection = Selection::Rect {
                x0: na,
                y0: nb,
                x1: nc,
                y1: nd,
            };
        }
    }

    pub(crate) fn apply_layer_scale_live(
        &mut self,
        handle: BoxHandle,
        start: (f32, f32),
        orig_x: i32,
        orig_y: i32,
        orig_w: u32,
        orig_h: u32,
        ix: f32,
        iy: f32,
    ) -> (i32, i32, u32, u32) {
        let lock = self.last_shift;
        let deg = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer())
            .map(|l| l.rotation)
            .unwrap_or(0.0);
        let x0 = orig_x as f32;
        let y0 = orig_y as f32;
        let x1 = x0 + orig_w as f32;
        let y1 = y0 + orig_h as f32;
        let ocx = x0 + orig_w as f32 * 0.5;
        let ocy = y0 + orig_h as f32 * 0.5;
        let to_local = |px: f32, py: f32| {
            if deg.abs() > 0.05 {
                crate::geom::rotate_point(px, py, ocx, ocy, -deg)
            } else {
                (px, py)
            }
        };
        let (lx, ly) = to_local(ix, iy);
        let (lsx, lsy) = to_local(start.0, start.1);
        let (mut a, mut b, mut c, mut d) =
            resize_rect(x0, y0, x1, y1, lx, ly, lsx, lsy, handle, lock);
        if deg.abs() > 0.05 {
            let (fix_lx, fix_ly) = opposite_handle_point(handle, x0, y0, x1, y1);
            let (fix_wx, fix_wy) = crate::geom::rotate_point(fix_lx, fix_ly, ocx, ocy, deg);
            let ncx = (a + c) * 0.5;
            let ncy = (b + d) * 0.5;
            let (nfix_lx, nfix_ly) = opposite_handle_point(handle, a, b, c, d);
            let (nwx, nwy) = crate::geom::rotate_point(nfix_lx, nfix_ly, ncx, ncy, deg);
            let dx = fix_wx - nwx;
            let dy = fix_wy - nwy;
            a += dx;
            b += dy;
            c += dx;
            d += dy;
        }
        let nx = a.round() as i32;
        let ny = b.round() as i32;
        let nw = (c - a).round().max(1.0) as u32;
        let nh = (d - b).round().max(1.0) as u32;
        (nx, ny, nw, nh)
    }

    pub(crate) fn bake_layer_scale(
        &mut self,
        orig_w: u32,
        orig_h: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) {
        if w == orig_w && h == orig_h {
            if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
                layer.x = x;
                layer.y = y;
            }
            return;
        }
        if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
            layer.bake_strokes();
        }
        let Some(layer) = self.doc.as_ref().and_then(|d| d.active_layer()) else {
            return;
        };
        let img = resize_rgba(layer.pixels.as_ref(), w, h);
        if let Some(layer) = self.doc.as_mut().and_then(|d| d.active_layer_mut()) {
            layer.pixels = Arc::new(img);
            layer.x = x;
            layer.y = y;
        }
        self.dirty = true;
        self.rebuild_preview();
    }

    pub(crate) fn wand_at(&mut self, ix: i32, iy: i32, cx: &mut Context<Self>) {
        let op = match (self.last_shift, self.clone_alt) {
            (true, true) => SelCombine::Intersect,
            (true, false) => SelCombine::Add,
            (false, true) => SelCombine::Subtract,
            (false, false) => SelCombine::Replace,
        };
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(layer) = doc.active_layer() else {
            return;
        };
        let cw = doc.canvas_w;
        let ch = doc.canvas_h;
        let lw = layer.width();
        let lh = layer.height();
        let (lx, ly) = canvas_to_layer_xy(ix, iy, layer.x, layer.y, lw, lh, layer.rotation);
        let miss = lx < 0 || ly < 0 || lx >= lw as i32 || ly >= lh as i32;
        let local = if miss {
            Vec::new()
        } else {
            wand_mask_rgba(
                &layer.pixels,
                lx,
                ly,
                WandOpts {
                    tolerance: self.wand_tol,
                    contiguous: self.wand_contiguous,
                    antialias: self.wand_antialias,
                },
            )
        };
        if local.iter().all(|&v| v == 0) {
            if op == SelCombine::Replace {
                self.selection = Selection::None;
                self.status = if miss {
                    "魔棒: 点在当前图层上".into()
                } else {
                    "魔棒: 该点透明或容差内无像素".into()
                };
            } else {
                self.status = "魔棒: 该点没有可选像素".into();
            }
            self.notify_chrome(cx);
            return;
        }
        let mask =
            lift_layer_mask_to_canvas(&local, lw, lh, layer.x, layer.y, layer.rotation, cw, ch);
        let merged = if op == SelCombine::Replace {
            mask
        } else {
            let prev = raster_selection(&self.selection, cw, ch);
            combine_masks(&prev, &mask, op)
        };
        let n = merged.iter().filter(|&&v| v > 0).count();
        if n == 0 {
            self.selection = Selection::None;
            self.status = "魔棒: 选区已清空".into();
            self.notify_chrome(cx);
            return;
        }
        let loops = trace_wand_loops(&merged, cw, ch);
        self.selection = Selection::from_loops(merged, loops, cw, ch);
        let verb = match op {
            SelCombine::Replace => "已选",
            SelCombine::Add => "已加选",
            SelCombine::Subtract => "已减选",
            SelCombine::Intersect => "已交选",
        };
        self.status = format!("魔棒: {verb} {n} 像素").into();
        self.notify_chrome(cx);
    }

    pub(crate) fn sample_canvas_rgb(&self, ix: i32, iy: i32) -> Option<[u8; 3]> {
        let doc = self.doc.as_ref()?;
        if ix < 0 || iy < 0 || ix >= doc.canvas_w as i32 || iy >= doc.canvas_h as i32 {
            return None;
        }
        for layer in doc.layers.iter().rev() {
            if !layer.visible {
                continue;
            }
            let lw = layer.width();
            let lh = layer.height();
            let (lx, ly) = canvas_to_layer_xy(ix, iy, layer.x, layer.y, lw, lh, layer.rotation);
            if lx < 0 || ly < 0 || lx >= lw as i32 || ly >= lh as i32 {
                continue;
            }
            let p = layer.pixels.get_pixel(lx as u32, ly as u32).0;
            if p[3] > 0 {
                return Some([p[0], p[1], p[2]]);
            }
        }
        Some(doc.paper_rgb)
    }

    pub(crate) fn sample_paint_color(&mut self, ix: i32, iy: i32, cx: &mut Context<Self>) {
        let Some(c) = self.sample_canvas_rgb(ix, iy) else {
            return;
        };
        self.paint_color = c;
        self.push_recent_color(c);
        let (h, s, v) = rgb_to_hsv(c);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        if self.color_picker_open {
            self.rebuild_sb_image();
            self.sync_rgb_inputs_from_picker(cx);
        }
        self.status = format!("已取色 {} {} {}", c[0], c[1], c[2]).into();
        self.notify_chrome(cx);
    }

    pub(crate) fn set_paint_color(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        self.paint_color = rgb;
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        if self.color_picker_open {
            self.rebuild_sb_image();
            self.sync_rgb_inputs_from_picker(cx);
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn lasso_maybe_snap(&self, ix: f32, iy: f32) -> (f32, f32, bool) {
        self.lasso_snap_within(ix, iy, POLY_SNAP_SCREEN_PX)
    }

    pub(crate) fn lasso_snap_within(&self, ix: f32, iy: f32, radius: f32) -> (f32, f32, bool) {
        let Some(draft) = &self.lasso_draft else {
            return (ix, iy, false);
        };
        if draft.len() < 3 {
            return (ix, iy, false);
        }
        let (fx, fy) = draft[0];
        let xform = self.view_xform();
        let (sx, sy) = xform.image_to_screen(ix, iy);
        let (fsx, fsy) = xform.image_to_screen(fx, fy);
        if (sx - fsx).hypot(sy - fsy) <= radius {
            (fx, fy, true)
        } else {
            (ix, iy, false)
        }
    }

    pub(crate) fn begin_lasso_at(&mut self, ix: f32, iy: f32, cx: &mut Context<Self>) {
        let (ix, iy, snap) = self.lasso_maybe_snap(ix, iy);
        if snap {
            self.finalize_lasso(cx);
            return;
        }
        match &mut self.lasso_draft {
            Some(draft) => {
                if let Some(&(lx, ly)) = draft.last() {
                    if (lx - ix).abs() >= 0.5 || (ly - iy).abs() >= 0.5 {
                        draft.push((ix, iy));
                    }
                } else {
                    draft.push((ix, iy));
                }
                self.status = format!(
                    "套索 {} 点 · 靠近首点闭环, 或按住拖轨迹 (右键/Esc 取消)",
                    draft.len()
                )
                .into();
            }
            None => {
                self.lasso_draft = Some(vec![(ix, iy)]);
                self.status = "套索起点 · 继续点击折线, 或按住拖出轨迹".into();
            }
        }
        self.lasso_cursor = Some((ix, iy));
        self.drag = Some(DragKind::LassoStroke {
            start_x: ix,
            start_y: iy,
            freehand: false,
        });
        self.notify_chrome(cx);
    }

    pub(crate) fn update_lasso_stroke(
        &mut self,
        ix: f32,
        iy: f32,
        vx: f32,
        vy: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(DragKind::LassoStroke {
            start_x,
            start_y,
            mut freehand,
        }) = self.drag.take()
        else {
            return;
        };
        let xform = self.view_xform();
        let (psx, psy) = xform.image_to_screen(start_x, start_y);
        if !freehand && (vx - psx).hypot(vy - psy) >= 5.0 {
            freehand = true;
        }
        if freehand {
            let (sx, sy, snapped) = self.lasso_snap_within(ix, iy, LASSO_DRAG_SNAP_SCREEN_PX);
            if snapped {
                self.lasso_cursor = Some((sx, sy));
                self.status = "套索已贴到起点 · 松手闭环".into();
            } else {
                if let Some(draft) = &mut self.lasso_draft {
                    if let Some(&(lx, ly)) = draft.last() {
                        let (lsx, lsy) = xform.image_to_screen(lx, ly);
                        let (csx, csy) = xform.image_to_screen(ix, iy);
                        if (lsx - csx).hypot(lsy - csy) >= LASSO_FREEHAND_SCREEN {
                            draft.push((ix, iy));
                        }
                    } else {
                        draft.push((ix, iy));
                    }
                }
                self.lasso_cursor = Some((ix, iy));
                if let Some(n) = self.lasso_draft.as_ref().map(|d| d.len()) {
                    self.status = format!("套索轨迹 {n} 点 · 松手闭环").into();
                }
            }
        } else {
            let (sx, sy, _) = self.lasso_maybe_snap(ix, iy);
            self.lasso_cursor = Some((sx, sy));
        }
        self.drag = Some(DragKind::LassoStroke {
            start_x,
            start_y,
            freehand,
        });
        self.notify_nav(cx);
    }

    pub(crate) fn finalize_lasso(&mut self, cx: &mut Context<Self>) {
        let Some(mut draft) = self.lasso_draft.take() else {
            return;
        };
        if let Some((x, y)) = self.lasso_cursor.take() {
            let far = draft
                .last()
                .map(|&(lx, ly)| (lx - x).hypot(ly - y) > 0.4)
                .unwrap_or(true);
            if far {
                draft.push((x, y));
            }
        }
        if draft.len() < 3 {
            self.status = "套索至少需要 3 个点.".into();
            self.notify_chrome(cx);
            return;
        }
        let (cw, ch) = self
            .doc
            .as_ref()
            .map(|d| (d.canvas_w, d.canvas_h))
            .unwrap_or((1, 1));
        let mask = fill_poly_mask(&draft, cw, ch);
        self.selection = Selection::from_poly(mask, draft, cw, ch);
        if self.selection.is_none() {
            self.status = "套索未围出区域.".into();
        } else {
            self.status = "套索已选".into();
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn cancel_lasso_draft(&mut self, cx: &mut Context<Self>) {
        if self.lasso_draft.is_none() {
            return;
        }
        self.lasso_draft = None;
        self.lasso_cursor = None;
        if matches!(self.drag, Some(DragKind::LassoStroke { .. })) {
            self.drag = None;
        }
        self.status = "已取消套索.".into();
        self.notify_chrome(cx);
    }

    pub(crate) fn toggle_layer_vis(&mut self, idx: usize, cx: &mut Context<Self>) {
        let exists = self.doc.as_ref().and_then(|d| d.layers.get(idx)).is_some();
        if !exists {
            return;
        }
        self.push_undo();
        if let Some(l) = self.doc.as_mut().and_then(|d| d.layers.get_mut(idx)) {
            l.visible = !l.visible;
            self.dirty = true;
            self.rebuild_preview();
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn select_layer(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.doc.as_mut() {
            doc.set_active(idx);
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn duplicate_layer(&mut self, cx: &mut Context<Self>) {
        let Some(src) = self.doc.as_ref().and_then(|d| d.active_layer().cloned()) else {
            return;
        };
        self.push_undo();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let mut copy = src;
        copy.id = crate::ids::new_id();
        copy.name = format!("{} 副本", copy.name);
        copy.x += 8;
        copy.y += 8;
        doc.layers.push(copy);
        doc.active = doc.layers.len() - 1;
        self.dirty = true;
        self.rebuild_preview();
        self.notify_chrome(cx);
    }

    pub(crate) fn brush_size_max(&self) -> f32 {
        brush_size_max_for_image(self.doc.as_ref().map(|d| d.canvas_w).unwrap_or(0))
    }

    pub(crate) fn clamp_brush_size(&mut self) {
        let max = self.brush_size_max();
        self.brush = self.brush.clamp(BRUSH_MIN, max);
    }

    pub(crate) fn slider_track_mut(&mut self, kind: SliderKind) -> &mut Bounds<Pixels> {
        match kind {
            SliderKind::BrushSize => &mut self.brush_size_track,
            SliderKind::Hardness => &mut self.hardness_track,
            SliderKind::WandTol => &mut self.wand_tol_track,
        }
    }

    pub(crate) fn set_slider_from_x(&mut self, x: f32, kind: SliderKind, cx: &mut Context<Self>) {
        let track = match kind {
            SliderKind::BrushSize => self.brush_size_track,
            SliderKind::Hardness => self.hardness_track,
            SliderKind::WandTol => self.wand_tol_track,
        };
        let left = f32::from(track.origin.x);
        let width = f32::from(track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        match kind {
            SliderKind::BrushSize => {
                self.brush = brush_size_from_t(t, BRUSH_MIN, self.brush_size_max());
            }
            SliderKind::Hardness => {
                self.brush_hardness = t;
            }
            SliderKind::WandTol => {
                self.wand_tol = (t * WAND_TOL_MAX as f32).round() as i32;
            }
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn bump_brush_size(&mut self, bigger: bool) {
        let max = self.brush_size_max();
        let t = brush_size_to_t(self.brush, BRUSH_MIN, max);
        let step = 0.06;
        let nt = if bigger { t + step } else { t - step };
        self.brush = brush_size_from_t(nt.clamp(0.0, 1.0), BRUSH_MIN, max);
        self.clamp_brush_size();
    }

    pub(crate) fn resolve_layer_drop(&self, from: usize, y: f32) -> (usize, Option<usize>, bool) {
        let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.layer_bounds.get(&i) else {
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
            let to = reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    pub(crate) fn update_layer_reorder(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(DragKind::LayerReorder {
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
        if !armed && reorder_slop_exceeded(x - start_x, y - start_y) {
            armed = true;
        }
        let (to, line_at, line_after) = if armed {
            self.resolve_layer_drop(from, y)
        } else {
            (from, None, false)
        };
        self.drag = Some(DragKind::LayerReorder {
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
        self.notify_chrome(cx);
    }

    pub(crate) fn finish_layer_reorder(&mut self, cx: &mut Context<Self>) {
        // 侧栏 `on_mouse_up_out` 在画布上松手时也会进来. 只能拿走图层排序,
        // 否则 `drag.take()` 会把框选/旋转一并丢掉, 松手后选区消失, 角度弹回.
        if !matches!(self.drag, Some(DragKind::LayerReorder { .. })) {
            return;
        }
        if let Some(DragKind::LayerReorder {
            from, to, armed, ..
        }) = self.drag.take()
        {
            self.commit_layer_reorder(from, to, armed, cx);
        }
    }

    pub(crate) fn commit_layer_reorder(
        &mut self,
        from: usize,
        to: usize,
        armed: bool,
        cx: &mut Context<Self>,
    ) {
        if armed && from != to {
            self.push_undo();
            let Some(doc) = self.doc.as_mut() else {
                self.notify_chrome(cx);
                return;
            };
            let aid = doc.layers.get(doc.active).map(|l| l.id.clone());
            apply_display_reorder(&mut doc.layers, from, to);
            if let Some(id) = aid {
                if let Some(i) = doc.layers.iter().position(|l| l.id == id) {
                    doc.active = i;
                }
            }
            self.dirty = true;
            self.rebuild_preview();
        } else if !armed {
            let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
            if n > 0 && from < n {
                let storage = n - 1 - from;
                self.select_layer(storage, cx);
                return;
            }
        }
        self.notify_chrome(cx);
    }
}

/// 半开矩形 `[x0, x1) × [y0, y1)` 盖到的图块.
fn push_upload_tiles(
    out: &mut Vec<(u32, u32)>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    lw: u32,
    lh: u32,
) {
    if lw == 0 || lh == 0 {
        return;
    }
    let x0 = x0.max(0);
    let y0 = y0.max(0);
    let x1 = x1.min(lw as i32);
    let y1 = y1.min(lh as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let tile = TILE as i32;
    let tx0 = x0 / tile;
    let ty0 = y0 / tile;
    let tx1 = (x1 - 1) / tile;
    let ty1 = (y1 - 1) / tile;
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            out.push((tx as u32, ty as u32));
        }
    }
}

fn push_tile_clip(
    clips: &mut HashMap<(u32, u32), (i32, i32, i32, i32)>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    lw: u32,
    lh: u32,
) {
    if lw == 0 || lh == 0 {
        return;
    }
    let x0 = x0.max(0);
    let y0 = y0.max(0);
    let x1 = x1.min(lw as i32);
    let y1 = y1.min(lh as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let tile = TILE as i32;
    let tx0 = x0 / tile;
    let ty0 = y0 / tile;
    let tx1 = (x1 - 1) / tile;
    let ty1 = (y1 - 1) / tile;
    for ty in ty0..=ty1 {
        let top = ty * tile;
        let bot = (top + tile).min(lh as i32);
        for tx in tx0..=tx1 {
            let left = tx * tile;
            let right = (left + tile).min(lw as i32);
            let cx0 = x0.max(left);
            let cy0 = y0.max(top);
            let cx1 = x1.min(right);
            let cy1 = y1.min(bot);
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            clips
                .entry((tx as u32, ty as u32))
                .and_modify(|r| {
                    r.0 = r.0.min(cx0);
                    r.1 = r.1.min(cy0);
                    r.2 = r.2.max(cx1);
                    r.3 = r.3.max(cy1);
                })
                .or_insert((cx0, cy0, cx1, cy1));
        }
    }
}

fn cover_disk(
    clips: &mut HashMap<(u32, u32), (i32, i32, i32, i32)>,
    px: i32,
    py: i32,
    radius: i32,
    lw: u32,
    lh: u32,
) {
    let r = radius.max(1);
    push_tile_clip(
        clips,
        px.saturating_sub(r + 1),
        py.saturating_sub(r + 1),
        px.saturating_add(r + 2),
        py.saturating_add(r + 2),
        lw,
        lh,
    );
}

fn cover_stroke(
    clips: &mut HashMap<(u32, u32), (i32, i32, i32, i32)>,
    stroke: &PaintStroke,
    lw: u32,
    lh: u32,
) {
    let r = stroke.radius.max(1);
    for &(px, py) in &stroke.points {
        cover_disk(clips, px, py, r, lw, lh);
    }
}

/// `None` 表示整层墨水没了, 调用方整层重传. 否则只重画被擦到的块.
fn ink_after_erase(
    layer: &mut Layer,
    clips: &HashMap<(u32, u32), (i32, i32, i32, i32)>,
) -> Option<Vec<(u32, u32)>> {
    if layer.strokes.is_empty() {
        layer.ink = None;
        layer.stroke_rev = layer.stroke_rev.wrapping_add(1);
        return None;
    }
    if clips.is_empty() {
        return Some(Vec::new());
    }
    let lw = layer.width();
    let lh = layer.height();
    let fits = layer
        .ink
        .as_ref()
        .is_some_and(|im| im.width() == lw && im.height() == lh);
    if !fits {
        layer.rebuild_ink_full();
        let mut all = Vec::new();
        push_upload_tiles(&mut all, 0, 0, lw as i32, lh as i32, lw, lh);
        return Some(all);
    }
    let bleed = TILE_BLEED as i32;
    let mut upload = Vec::new();
    for &(x0, y0, x1, y1) in clips.values() {
        layer.rebuild_ink_rect(x0, y0, x1, y1);
        push_upload_tiles(
            &mut upload,
            x0 - bleed,
            y0 - bleed,
            x1 + bleed,
            y1 + bleed,
            lw,
            lh,
        );
    }
    upload.sort_unstable();
    upload.dedup();
    Some(upload)
}

fn trace_wand_loops(mask: &[u8], w: u32, h: u32) -> Vec<Vec<(f32, f32)>> {
    let loops = mask_loops(mask, w, h, 128);
    if !loops.is_empty() || mask.iter().all(|&v| v == 0) {
        loops
    } else {
        mask_loops(mask, w, h, 1)
    }
}
