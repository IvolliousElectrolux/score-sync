//! GPUI P 图界面.

mod canvas;
mod chrome;
mod io;
mod picker;
mod tools;
mod types;

pub use types::{Commit, FitHint, HostCmd, ImportOption, ToolMode};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    actions, div, point, prelude::*, px, rgb, size, App, Application, Bounds, Context, CursorStyle,
    Div, Entity, ExternalPaths, FocusHandle, Focusable, KeyBinding, ModifiersChangedEvent, Pixels,
    Point, Render, RenderImage, SharedString, Window, WindowBounds, WindowOptions,
};
use image::{RgbImage, RgbaImage};

use crate::document::EditDocument;
use crate::process::Selection;

use types::*;

actions!(
    photo_edit,
    [
        OpenFile,
        ExportImage,
        FitView,
        Undo,
        Redo,
        DeleteLayer,
        NewLayerFromSel,
        Cancel,
        ApplyEdit,
        RestoreOriginal,
        ToolSelect,
        ToolMove,
        ToolCanvas,
        ToolHeal,
        ToolClone,
        ToolPaint,
        ToolEraser,
        ToolWand,
        ToolLasso,
        ToolPan,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
    ]
);

struct LayerGpu {
    id: String,
    pixels_ptr: usize,
    stroke_rev: u64,
    cols: u32,
    rows: u32,
    tiles: Vec<Option<TileSprite>>,
}

pub struct PhotoEditApp {
    focus_handle: FocusHandle,
    pub(crate) doc: Option<EditDocument>,
    pub(crate) selection: Selection,
    pub(crate) mode: ToolMode,
    pub(crate) erase_target: EraseTarget,
    pub(crate) trace_erase: TraceErase,
    layer_tex: Vec<LayerGpu>,
    gpu_drop: Vec<Arc<RenderImage>>,
    zoom: f32,
    pan: Point<f32>,
    user_zoomed: bool,
    view_bounds: Bounds<Pixels>,
    drag: Option<DragKind>,
    status: SharedString,
    undo_stack: Vec<HistorySnap>,
    redo_stack: Vec<HistorySnap>,
    dirty: bool,
    brush: f32,
    brush_hardness: f32,
    /// 本笔已盖章位置 (图像坐标), 用于间距插值.
    brush_last: Option<(f32, f32)>,
    wand_tol: i32,
    wand_contiguous: bool,
    wand_antialias: bool,
    clone_src: Option<(i32, i32)>,
    clone_offset: Option<(i32, i32)>,
    clone_snap: Option<Arc<RgbaImage>>,
    clone_alt: bool,
    /// 本笔污点修复锁定的源偏移; 拖开后不再重搜.
    heal_offset: Option<(i32, i32)>,
    fit_hint: FitHint,
    import_options: Vec<ImportOption>,
    host_cmd: Option<HostCmd>,
    standalone: bool,
    image_path: Option<PathBuf>,
    hover_cursor: CursorStyle,
    /// 修复 / 图章圆形光标中心 (图像坐标).
    brush_cursor: Option<(f32, f32)>,
    last_pointer_view: Option<(f32, f32)>,
    hover_rotate: bool,
    brush_size_track: Bounds<Pixels>,
    hardness_track: Bounds<Pixels>,
    wand_tol_track: Bounds<Pixels>,
    last_shift: bool,
    /// 最近一次 notify 只是缩放/平移: 宿主不要整窗重绘.
    nav_only: bool,
    canvas_hover: Option<CanvasEdge>,
    paint_color: [u8; 3],
    layer_bounds: HashMap<usize, Bounds<Pixels>>,
    side_origin: (f32, f32),
    side_bounds: Bounds<Pixels>,
    picker_layer_bounds: Bounds<Pixels>,
    paint_swatch_bounds: Bounds<Pixels>,
    sb_bounds: Bounds<Pixels>,
    hue_bounds: Bounds<Pixels>,
    color_picker_open: bool,
    picker_h: f32,
    picker_s: f32,
    picker_v: f32,
    sb_image: Option<Arc<RenderImage>>,
    hue_image: Option<Arc<RenderImage>>,
    rgb_r_input: Entity<apply_bg::text_input::TextInput>,
    rgb_g_input: Entity<apply_bg::text_input::TextInput>,
    rgb_b_input: Entity<apply_bg::text_input::TextInput>,
    rgb_syncing: bool,
    eyedropper_armed: bool,
    eyedropper_backup: Option<(f32, f32, f32, [u8; 3])>,
    recent_colors: Vec<[u8; 3]>,
    lasso_draft: Option<Vec<(f32, f32)>>,
    lasso_cursor: Option<(f32, f32)>,
}

impl PhotoEditApp {
    pub fn new(cx: &mut Context<Self>, standalone: bool) -> Self {
        let rgb_r_input =
            cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "32", "R").with_compact(true));
        let rgb_g_input =
            cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "32", "G").with_compact(true));
        let rgb_b_input =
            cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "32", "B").with_compact(true));
        cx.observe(&rgb_r_input, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_g_input, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_b_input, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        let (picker_h, picker_s, picker_v) = rgb_to_hsv(PAINT_DEFAULT);
        Self {
            focus_handle: cx.focus_handle(),
            doc: None,
            selection: Selection::None,
            mode: ToolMode::Select,
            erase_target: EraseTarget::Trace,
            trace_erase: TraceErase::Dab,
            layer_tex: Vec::new(),
            gpu_drop: Vec::new(),
            zoom: 1.0,
            pan: point(0.0, 0.0),
            user_zoomed: false,
            view_bounds: Bounds::default(),
            drag: None,
            status: SharedString::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
            brush: BRUSH_DEFAULT,
            brush_hardness: HARD_DEFAULT,
            brush_last: None,
            wand_tol: 32,
            wand_contiguous: true,
            wand_antialias: true,
            clone_src: None,
            clone_offset: None,
            clone_snap: None,
            clone_alt: false,
            heal_offset: None,
            fit_hint: FitHint::default(),
            import_options: Vec::new(),
            host_cmd: None,
            standalone,
            image_path: None,
            hover_cursor: CursorStyle::Arrow,
            brush_cursor: None,
            last_pointer_view: None,
            hover_rotate: false,
            brush_size_track: Bounds::default(),
            hardness_track: Bounds::default(),
            wand_tol_track: Bounds::default(),
            last_shift: false,
            nav_only: false,
            canvas_hover: None,
            paint_color: PAINT_DEFAULT,
            layer_bounds: HashMap::new(),
            side_origin: (0.0, 0.0),
            side_bounds: Bounds::default(),
            picker_layer_bounds: Bounds::default(),
            paint_swatch_bounds: Bounds::default(),
            sb_bounds: Bounds::default(),
            hue_bounds: Bounds::default(),
            color_picker_open: false,
            picker_h,
            picker_s,
            picker_v,
            sb_image: None,
            hue_image: None,
            rgb_r_input,
            rgb_g_input,
            rgb_b_input,
            rgb_syncing: false,
            eyedropper_armed: false,
            eyedropper_backup: None,
            recent_colors: default_recent_colors(),
            lasso_draft: None,
            lasso_cursor: None,
        }
    }

    pub fn is_nav_only(&self) -> bool {
        self.nav_only
    }

    pub fn has_host_cmd(&self) -> bool {
        self.host_cmd.is_some()
    }

    pub(crate) fn notify_nav(&mut self, cx: &mut Context<Self>) {
        self.nav_only = true;
        cx.notify();
    }

    pub(crate) fn notify_chrome(&mut self, cx: &mut Context<Self>) {
        self.nav_only = false;
        cx.notify();
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn has_document(&self) -> bool {
        self.doc.is_some()
    }

    pub fn set_fit_hint(&mut self, hint: FitHint, cx: &mut Context<Self>) {
        self.fit_hint = hint;
        self.notify_chrome(cx);
    }

    pub fn set_import_options(&mut self, opts: Vec<ImportOption>, cx: &mut Context<Self>) {
        self.import_options = opts;
        self.notify_chrome(cx);
    }

    pub fn set_source(&mut self, source: crate::document::SourceFingerprint) {
        if let Some(d) = self.doc.as_mut() {
            d.source = Some(source);
        }
    }

    pub fn take_host_cmd(&mut self) -> Option<HostCmd> {
        self.host_cmd.take()
    }

    pub fn take_commit(&self) -> Option<Commit> {
        let doc = self.doc.as_ref()?;
        Some(Commit {
            flat: doc.flatten_rgb(),
            document: doc.clone(),
        })
    }

    pub fn open_rgb(&mut self, img: RgbImage, paper: Option<[u8; 3]>, cx: &mut Context<Self>) {
        let paper = paper.unwrap_or_else(|| crate::process::sample_paper(&img, 200));
        self.doc = Some(EditDocument::from_rgb(&img, paper));
        self.selection = Selection::None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.dirty = false;
        self.clone_src = None;
        self.heal_offset = None;
        self.clamp_brush_size();
        self.fit_to_view(cx);
        self.rebuild_preview();
        self.status = format!("画布 {}×{}", img.width(), img.height()).into();
        self.notify_chrome(cx);
    }

    pub fn open_document(&mut self, doc: EditDocument, cx: &mut Context<Self>) {
        self.doc = Some(doc);
        self.selection = Selection::None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.dirty = false;
        self.clamp_brush_size();
        self.fit_to_view(cx);
        self.rebuild_preview();
        self.status = "已载入上次修图".into();
        self.notify_chrome(cx);
    }

    pub fn import_rgba(&mut self, img: RgbaImage, name: impl Into<String>, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        self.push_undo();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let mut layer = crate::document::Layer::new(name, img);
        layer.x = 8;
        layer.y = 8;
        doc.layers.push(layer);
        doc.active = doc.layers.len() - 1;
        self.dirty = true;
        self.rebuild_preview();
        self.status = "已导入图层".into();
        self.notify_chrome(cx);
    }

    pub fn close_session(&mut self, cx: &mut Context<Self>) {
        self.doc = None;
        self.selection = Selection::None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.clone_src = None;
        self.clone_offset = None;
        self.clone_snap = None;
        self.heal_offset = None;
        self.brush_last = None;
        self.lasso_draft = None;
        self.lasso_cursor = None;
        self.eyedropper_armed = false;
        self.color_picker_open = false;
        self.dirty = false;
        self.drop_all_layer_tex();
        self.notify_chrome(cx);
    }

    pub fn take_gpu_drops(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.gpu_drop)
    }

    fn drop_all_layer_tex(&mut self) {
        let old = std::mem::take(&mut self.layer_tex);
        for g in old {
            self.drop_layer_gpu(g);
        }
    }

    fn drop_layer_gpu(&mut self, g: LayerGpu) {
        for tile in g.tiles.into_iter().flatten() {
            self.gpu_drop.push(tile.tex);
        }
    }

    pub(crate) fn layer_paints(&self) -> Vec<LayerPaint> {
        let Some(doc) = self.doc.as_ref() else {
            return Vec::new();
        };
        doc.layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.visible)
            .filter_map(|(i, l)| {
                let g = self.layer_tex.iter().find(|g| g.id == l.id)?;
                let (x, y, w, h) = if i == doc.active {
                    if let Some(DragKind::LayerScale {
                        cur_x,
                        cur_y,
                        cur_w,
                        cur_h,
                        ..
                    }) = &self.drag
                    {
                        (*cur_x, *cur_y, *cur_w, *cur_h)
                    } else {
                        (l.x, l.y, l.width(), l.height())
                    }
                } else {
                    (l.x, l.y, l.width(), l.height())
                };
                let extra = if i == doc.active {
                    match &self.drag {
                        Some(DragKind::LayerRotate { last_deg, .. }) => *last_deg,
                        _ => 0.0,
                    }
                } else {
                    0.0
                };
                let rotate_deg = l.rotation + extra;
                let tiles = g.tiles.iter().flatten().cloned().collect();
                Some(LayerPaint {
                    tiles,
                    x,
                    y,
                    w,
                    h,
                    active: i == doc.active,
                    rotate_deg,
                })
            })
            .collect()
    }

    pub(crate) fn refresh_layer_dirty(&mut self, index: usize, x0: i32, y0: i32, x1: i32, y1: i32) {
        let Some(layer) = self.doc.as_ref().and_then(|d| d.layers.get(index)) else {
            return;
        };
        let id = layer.id.clone();
        let pixels = layer.pixels.clone();
        let ink = layer.ink.clone();
        let rev = layer.stroke_rev;
        let lw = layer.width();
        let lh = layer.height();
        if self.layer_tex.iter().all(|g| g.id != id) {
            self.push_fresh_gpu(&id, pixels.as_ref(), ink.as_deref(), rev, lw, lh);
        }
        self.upload_tiles(
            &id,
            pixels.as_ref(),
            ink.as_deref(),
            rev,
            lw,
            lh,
            x0,
            y0,
            x1,
            y1,
            true,
            None,
        );
    }

    pub(crate) fn refresh_layer_tiles(&mut self, index: usize, tiles: &[(u32, u32)]) {
        if tiles.is_empty() {
            return;
        }
        let Some(layer) = self.doc.as_ref().and_then(|d| d.layers.get(index)) else {
            return;
        };
        let id = layer.id.clone();
        let pixels = layer.pixels.clone();
        let ink = layer.ink.clone();
        let rev = layer.stroke_rev;
        let lw = layer.width();
        let lh = layer.height();
        if self.layer_tex.iter().all(|g| g.id != id) {
            self.push_fresh_gpu(&id, pixels.as_ref(), ink.as_deref(), rev, lw, lh);
        }
        self.upload_tiles(
            &id,
            pixels.as_ref(),
            ink.as_deref(),
            rev,
            lw,
            lh,
            0,
            0,
            0,
            0,
            true,
            Some(tiles),
        );
    }

    fn upload_tiles(
        &mut self,
        id: &str,
        pixels: &RgbaImage,
        ink: Option<&RgbaImage>,
        stroke_rev: u64,
        lw: u32,
        lh: u32,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        mark_dirty: bool,
        only: Option<&[(u32, u32)]>,
    ) {
        let Some(g) = self.layer_tex.iter_mut().find(|g| g.id == id) else {
            return;
        };
        let cols = tile_count(lw);
        let rows = tile_count(lh);
        if g.cols != cols || g.rows != rows || g.tiles.len() != (cols * rows) as usize {
            for tile in g.tiles.drain(..).flatten() {
                self.gpu_drop.push(tile.tex);
            }
            g.cols = cols;
            g.rows = rows;
            g.tiles = vec![None; (cols * rows) as usize];
        }
        g.pixels_ptr = pixels as *const RgbaImage as usize;
        g.stroke_rev = stroke_rev;
        if cols == 0 || rows == 0 {
            return;
        }
        let mut coords: Vec<(u32, u32)> = Vec::new();
        if let Some(only) = only {
            coords.extend(
                only.iter()
                    .copied()
                    .filter(|&(tx, ty)| tx < cols && ty < rows),
            );
            coords.sort_unstable();
            coords.dedup();
        } else {
            let x0 = x0.clamp(0, lw as i32);
            let y0 = y0.clamp(0, lh as i32);
            let x1 = x1.clamp(0, lw as i32);
            let y1 = y1.clamp(0, lh as i32);
            if x1 <= x0 || y1 <= y0 {
                return;
            }
            let tx0 = (x0 / TILE as i32).clamp(0, cols as i32 - 1) as u32;
            let ty0 = (y0 / TILE as i32).clamp(0, rows as i32 - 1) as u32;
            let tx1 = ((x1 - 1) / TILE as i32).clamp(0, cols as i32 - 1) as u32;
            let ty1 = ((y1 - 1) / TILE as i32).clamp(0, rows as i32 - 1) as u32;
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    coords.push((tx, ty));
                }
            }
        }
        if coords.is_empty() {
            return;
        }
        let mut uploads: Vec<(usize, TileSprite)> = Vec::new();
        for (tx, ty) in coords {
            let idx = (ty * cols + tx) as usize;
            if !mark_dirty && g.tiles.get(idx).and_then(|t| t.as_ref()).is_some() {
                continue;
            }
            let Some(sample) = tile_sample(tx, ty, lw, lh) else {
                continue;
            };
            uploads.push((
                idx,
                TileSprite {
                    tex: upload_layer_rect(pixels, ink, sample.sx, sample.sy, sample.sw, sample.sh),
                    x: sample.x,
                    y: sample.y,
                    w: sample.w,
                    h: sample.h,
                    pad_l: sample.pad_l,
                    pad_t: sample.pad_t,
                    pad_r: sample.pad_r,
                    pad_b: sample.pad_b,
                },
            ));
        }
        for (idx, sprite) in uploads {
            if let Some(slot) = g.tiles.get_mut(idx) {
                if let Some(old) = slot.replace(sprite) {
                    self.gpu_drop.push(old.tex);
                }
            }
        }
    }

    fn push_fresh_gpu(
        &mut self,
        id: &str,
        pixels: &RgbaImage,
        ink: Option<&RgbaImage>,
        stroke_rev: u64,
        w: u32,
        h: u32,
    ) {
        let cols = tile_count(w);
        let rows = tile_count(h);
        self.layer_tex.push(LayerGpu {
            id: id.to_string(),
            pixels_ptr: pixels as *const RgbaImage as usize,
            stroke_rev,
            cols,
            rows,
            tiles: vec![None; (cols * rows) as usize],
        });
        self.upload_tiles(
            id, pixels, ink, stroke_rev, w, h, 0, 0, w as i32, h as i32, false, None,
        );
    }

    pub(crate) fn rebuild_preview(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            self.drop_all_layer_tex();
            return;
        };
        let mut next = Vec::with_capacity(doc.layers.len());
        let mut fresh: Vec<(
            String,
            Arc<RgbaImage>,
            Option<Arc<RgbaImage>>,
            u64,
            u32,
            u32,
        )> = Vec::new();
        let mut old = std::mem::take(&mut self.layer_tex);
        for layer in &doc.layers {
            let ptr = Arc::as_ptr(&layer.pixels) as usize;
            if let Some(idx) = old.iter().position(|g| {
                g.id == layer.id && g.pixels_ptr == ptr && g.stroke_rev == layer.stroke_rev
            }) {
                next.push(old.swap_remove(idx));
            } else {
                let cols = tile_count(layer.width());
                let rows = tile_count(layer.height());
                fresh.push((
                    layer.id.clone(),
                    layer.pixels.clone(),
                    layer.ink.clone(),
                    layer.stroke_rev,
                    layer.width(),
                    layer.height(),
                ));
                next.push(LayerGpu {
                    id: layer.id.clone(),
                    pixels_ptr: ptr,
                    stroke_rev: layer.stroke_rev,
                    cols,
                    rows,
                    tiles: vec![None; (cols * rows) as usize],
                });
            }
        }
        for g in old {
            self.drop_layer_gpu(g);
        }
        self.layer_tex = next;
        for (id, pixels, ink, rev, w, h) in fresh {
            self.upload_tiles(
                &id,
                pixels.as_ref(),
                ink.as_deref(),
                rev,
                w,
                h,
                0,
                0,
                w as i32,
                h as i32,
                false,
                None,
            );
        }
    }

    pub(crate) fn refresh_active_layer_tex(&mut self) {
        let Some(layer) = self.doc.as_ref().and_then(|d| d.active_layer()) else {
            return;
        };
        let id = layer.id.clone();
        let pixels = layer.pixels.clone();
        let ink = layer.ink.clone();
        let rev = layer.stroke_rev;
        let w = layer.width();
        let h = layer.height();
        let cols = tile_count(w);
        let rows = tile_count(h);
        let neu = LayerGpu {
            id: id.clone(),
            pixels_ptr: Arc::as_ptr(&pixels) as usize,
            stroke_rev: rev,
            cols,
            rows,
            tiles: vec![None; (cols * rows) as usize],
        };
        if let Some(g) = self.layer_tex.iter_mut().find(|g| g.id == id) {
            let old = std::mem::replace(g, neu);
            self.drop_layer_gpu(old);
        } else {
            self.layer_tex.push(neu);
        }
        self.upload_tiles(
            &id,
            pixels.as_ref(),
            ink.as_deref(),
            rev,
            w,
            h,
            0,
            0,
            w as i32,
            h as i32,
            false,
            None,
        );
    }

    fn history_snap(&self) -> Option<HistorySnap> {
        Some(HistorySnap {
            doc: self.doc.clone()?,
            pan: self.pan,
            zoom: self.zoom,
            user_zoomed: self.user_zoomed,
        })
    }

    fn restore_snap(&mut self, snap: HistorySnap) {
        self.doc = Some(snap.doc);
        self.pan = snap.pan;
        self.zoom = snap.zoom;
        self.user_zoomed = snap.user_zoomed;
        self.brush_last = None;
        self.clone_snap = None;
        self.heal_offset = None;
    }

    pub(crate) fn push_undo(&mut self) {
        if let Some(snap) = self.history_snap() {
            self.undo_stack.push(snap);
            if self.undo_stack.len() > HISTORY_LIMIT {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.history_snap() else {
            return;
        };
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(cur);
        self.restore_snap(prev);
        self.dirty = true;
        self.rebuild_preview();
        self.status = "已撤销".into();
        self.notify_chrome(cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.history_snap() else {
            return;
        };
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(cur);
        self.restore_snap(next);
        self.dirty = true;
        self.rebuild_preview();
        self.status = "已重做".into();
        self.notify_chrome(cx);
    }

    pub fn fit_to_view(&mut self, cx: &mut Context<Self>) {
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.notify_nav(cx);
    }

    pub fn request_apply(&mut self, cx: &mut Context<Self>) {
        if self.standalone {
            self.export_image_standalone(cx);
        } else {
            self.host_cmd = Some(HostCmd::Apply);
            self.notify_chrome(cx);
        }
    }

    pub fn request_cancel(&mut self, cx: &mut Context<Self>) {
        if self.standalone {
            self.status = "独立模式: 可直接关窗".into();
            self.notify_chrome(cx);
        } else {
            self.host_cmd = Some(HostCmd::Cancel);
            self.notify_chrome(cx);
        }
    }

    pub fn request_restore(&mut self, cx: &mut Context<Self>) {
        self.host_cmd = Some(HostCmd::Restore);
        self.notify_chrome(cx);
    }

    fn session_root(&self, cx: &mut Context<Self>) -> Div {
        div()
            .key_context("PhotoEdit")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| this.open_file(window, cx)))
            .on_action(cx.listener(|this, _: &ExportImage, _, cx| this.export_image_standalone(cx)))
            .on_action(cx.listener(|this, _: &FitView, _, cx| this.fit_to_view(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &DeleteLayer, _, cx| this.delete_active_layer(cx)))
            .on_action(
                cx.listener(|this, _: &NewLayerFromSel, _, cx| this.new_layer_from_selection(cx)),
            )
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.on_escape(cx)))
            .on_action(cx.listener(|this, _: &ApplyEdit, _, cx| this.request_apply(cx)))
            .on_action(cx.listener(|this, _: &RestoreOriginal, _, cx| this.request_restore(cx)))
            .on_action(
                cx.listener(|this, _: &ToolSelect, _, cx| this.set_mode(ToolMode::Select, cx)),
            )
            .on_action(cx.listener(|this, _: &ToolMove, _, cx| this.set_mode(ToolMode::Move, cx)))
            .on_action(
                cx.listener(|this, _: &ToolCanvas, _, cx| this.set_mode(ToolMode::Canvas, cx)),
            )
            .on_action(cx.listener(|this, _: &ToolHeal, _, cx| this.set_mode(ToolMode::Heal, cx)))
            .on_action(cx.listener(|this, _: &ToolClone, _, cx| this.set_mode(ToolMode::Clone, cx)))
            .on_action(cx.listener(|this, _: &ToolPaint, _, cx| this.set_mode(ToolMode::Paint, cx)))
            .on_action(
                cx.listener(|this, _: &ToolEraser, _, cx| this.set_mode(ToolMode::Eraser, cx)),
            )
            .on_action(cx.listener(|this, _: &ToolWand, _, cx| this.set_mode(ToolMode::Wand, cx)))
            .on_action(cx.listener(|this, _: &ToolLasso, _, cx| this.set_mode(ToolMode::Lasso, cx)))
            .on_action(cx.listener(|this, _: &ToolPan, _, cx| this.set_mode(ToolMode::Pan, cx)))
            .on_action(cx.listener(|this, _: &NudgeLeft, _, cx| this.nudge(-1, 0, cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, _, cx| this.nudge(1, 0, cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, _, cx| this.nudge(0, -1, cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, _, cx| this.nudge(0, 1, cx)))
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _, cx| {
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
    }

    pub fn set_mode(&mut self, mode: ToolMode, cx: &mut Context<Self>) {
        if self.mode == ToolMode::Lasso && mode != ToolMode::Lasso {
            self.lasso_draft = None;
            self.lasso_cursor = None;
        }
        self.mode = mode;
        self.drag = None;
        if !mode.uses_brush() {
            self.brush_cursor = None;
        }
        if mode != ToolMode::Paint {
            self.eyedropper_armed = false;
        }
        self.notify_chrome(cx);
    }
}

impl Focusable for PhotoEditApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PhotoEditApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for img in self.take_gpu_drops() {
            let _ = window.drop_image(img);
        }
        if !self.standalone {
            return self
                .session_root(cx)
                .id("photo_edit_embed")
                .size_full()
                .child(self.image_view(cx))
                .into_any_element();
        }
        let title: SharedString = match &self.image_path {
            Some(p) => format!(
                "P 图 — {}",
                p.file_name().and_then(|s| s.to_str()).unwrap_or("image")
            )
            .into(),
            None => "P 图 / Photo Edit".into(),
        };
        self.session_root(cx)
            .id("photo_edit_root")
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(p) = paths.paths().iter().find(|p| crate::ids::is_image_path(p)) {
                    this.load_image(p.clone(), cx);
                }
            }))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf8fafc))
            .text_color(rgb(0x0f172a))
            .font_family(apply_bg::ui_font())
            .when(self.standalone, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(0xcbd5e1))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(self.toolbar(cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.image_view(cx))
                    .child(self.side_panel(cx)),
            )
            .into_any_element()
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("v", ToolSelect, Some("PhotoEdit")),
        KeyBinding::new("m", ToolMove, Some("PhotoEdit")),
        KeyBinding::new("k", ToolCanvas, Some("PhotoEdit")),
        KeyBinding::new("h", ToolHeal, Some("PhotoEdit")),
        KeyBinding::new("s", ToolClone, Some("PhotoEdit")),
        KeyBinding::new("b", ToolPaint, Some("PhotoEdit")),
        KeyBinding::new("e", ToolEraser, Some("PhotoEdit")),
        KeyBinding::new("w", ToolWand, Some("PhotoEdit")),
        KeyBinding::new("l", ToolLasso, Some("PhotoEdit")),
        KeyBinding::new("p", ToolPan, Some("PhotoEdit")),
        KeyBinding::new("f", FitView, Some("PhotoEdit")),
        KeyBinding::new("j", NewLayerFromSel, Some("PhotoEdit")),
        KeyBinding::new("escape", Cancel, Some("PhotoEdit")),
        KeyBinding::new("delete", DeleteLayer, Some("PhotoEdit")),
        KeyBinding::new("backspace", DeleteLayer, Some("PhotoEdit")),
        KeyBinding::new("left", NudgeLeft, Some("PhotoEdit")),
        KeyBinding::new("right", NudgeRight, Some("PhotoEdit")),
        KeyBinding::new("up", NudgeUp, Some("PhotoEdit")),
        KeyBinding::new("down", NudgeDown, Some("PhotoEdit")),
    ]);
    cx.bind_keys(apply_bg::bind_primary("o", OpenFile, Some("PhotoEdit")));
    cx.bind_keys(apply_bg::bind_primary("s", ApplyEdit, Some("PhotoEdit")));
    cx.bind_keys(apply_bg::bind_primary("e", ExportImage, Some("PhotoEdit")));
    cx.bind_keys(apply_bg::bind_primary("z", Undo, Some("PhotoEdit")));
    cx.bind_keys(apply_bg::bind_primary("y", Redo, Some("PhotoEdit")));
    cx.bind_keys(apply_bg::bind_primary("shift-z", Redo, Some("PhotoEdit")));
}

pub fn run_gui(initial: Option<PathBuf>) {
    Application::new().run(move |cx: &mut App| {
        apply_bg::text_input::bind_keys(cx);
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(860.)), cx);
        let initial = initial.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("P 图 / Photo Edit".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let mut app = PhotoEditApp::new(cx, true);
                    if let Some(p) = initial.clone() {
                        app.load_image(p, cx);
                    }
                    app.focus_handle.focus(window);
                    app
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

fn tile_count(len: u32) -> u32 {
    len.max(1).div_ceil(TILE)
}

struct TileSample {
    sx: u32,
    sy: u32,
    sw: u32,
    sh: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    pad_l: u32,
    pad_t: u32,
    pad_r: u32,
    pad_b: u32,
}

/// 逻辑块 `[tx, ty)` 以及四周 [`TILE_BLEED`] 像素的取样矩形.
fn tile_sample(tx: u32, ty: u32, img_w: u32, img_h: u32) -> Option<TileSample> {
    let x = tx.saturating_mul(TILE);
    let y = ty.saturating_mul(TILE);
    if x >= img_w || y >= img_h {
        return None;
    }
    let w = ((tx + 1) * TILE).min(img_w).saturating_sub(x);
    let h = ((ty + 1) * TILE).min(img_h).saturating_sub(y);
    if w == 0 || h == 0 {
        return None;
    }
    let x0 = x.saturating_sub(TILE_BLEED);
    let y0 = y.saturating_sub(TILE_BLEED);
    let x1 = (x + w + TILE_BLEED).min(img_w);
    let y1 = (y + h + TILE_BLEED).min(img_h);
    Some(TileSample {
        sx: x0,
        sy: y0,
        sw: x1.saturating_sub(x0),
        sh: y1.saturating_sub(y0),
        x: x as i32,
        y: y as i32,
        w,
        h,
        pad_l: x - x0,
        pad_t: y - y0,
        pad_r: x1.saturating_sub(x + w),
        pad_b: y1.saturating_sub(y + h),
    })
}

fn upload_layer_rect(
    base: &RgbaImage,
    ink: Option<&RgbaImage>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Arc<RenderImage> {
    let Some(ink) = ink else {
        return upload_bgra_rect(base, x, y, w, h);
    };
    if w == 0 || h == 0 {
        return upload_bgra_rect(base, x, y, w, h);
    }
    let covered = x.saturating_add(w) <= base.width()
        && y.saturating_add(h) <= base.height()
        && x.saturating_add(w) <= ink.width()
        && y.saturating_add(h) <= ink.height();
    if !covered {
        return upload_bgra_rect(base, x, y, w, h);
    }
    let bw = base.width() as usize;
    let iw = ink.width() as usize;
    let base_raw = base.as_raw();
    let ink_raw = ink.as_raw();
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    let stride = w as usize * 4;
    for row in 0..h as usize {
        let sy = y as usize + row;
        let b0 = (sy * bw + x as usize) * 4;
        let i0 = (sy * iw + x as usize) * 4;
        let drow = &mut buf[row * stride..row * stride + stride];
        let brow = &base_raw[b0..b0 + stride];
        let irow = &ink_raw[i0..i0 + stride];
        for col in 0..w as usize {
            let s = &irow[col * 4..col * 4 + 4];
            let b = &brow[col * 4..col * 4 + 4];
            let d = &mut drow[col * 4..col * 4 + 4];
            if s[3] == 0 {
                d[0] = b[2];
                d[1] = b[1];
                d[2] = b[0];
                d[3] = b[3];
            } else if s[3] == 255 {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = 255;
            } else {
                let p = over_rgba(
                    image::Rgba([b[0], b[1], b[2], b[3]]),
                    image::Rgba([s[0], s[1], s[2], s[3]]),
                );
                d[0] = p[2];
                d[1] = p[1];
                d[2] = p[0];
                d[3] = p[3];
            }
        }
    }
    let img = RgbaImage::from_raw(w, h, buf).expect("tile buffer");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        img
    )]))
}

fn over_rgba(dst: image::Rgba<u8>, src: image::Rgba<u8>) -> image::Rgba<u8> {
    let sa = src[3] as f32 / 255.0;
    if sa >= 0.999 {
        return src;
    }
    let inv = 1.0 - sa;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * inv;
    if out_a <= 1e-6 {
        return image::Rgba([0, 0, 0, 0]);
    }
    image::Rgba([
        ((src[0] as f32 * sa + dst[0] as f32 * da * inv) / out_a).round() as u8,
        ((src[1] as f32 * sa + dst[1] as f32 * da * inv) / out_a).round() as u8,
        ((src[2] as f32 * sa + dst[2] as f32 * da * inv) / out_a).round() as u8,
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

fn upload_bgra_rect(src: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> Arc<RenderImage> {
    let sw = src.width() as usize;
    let raw = src.as_raw();
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..h as usize {
        let s0 = ((y as usize + row) * sw + x as usize) * 4;
        let d0 = row * w as usize * 4;
        let src_row = &raw[s0..s0 + w as usize * 4];
        let dst_row = &mut buf[d0..d0 + w as usize * 4];
        for (s, d) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
    let img = RgbaImage::from_raw(w, h, buf).expect("tile buffer");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        img
    )]))
}

fn cap_tex_size(w: u32, h: u32) -> (u32, u32) {
    let w = w.max(1);
    let h = h.max(1);
    if w.max(h) > GPU_TEX_MAX_SIDE {
        let m = w.max(h);
        (
            ((w as u64 * GPU_TEX_MAX_SIDE as u64) / m as u64).max(1) as u32,
            ((h as u64 * GPU_TEX_MAX_SIDE as u64) / m as u64).max(1) as u32,
        )
    } else {
        (w, h)
    }
}

#[allow(dead_code)]
pub fn rgb_to_render_image(rgb: &RgbImage) -> Arc<RenderImage> {
    let (w, h) = rgb.dimensions();
    let (tw, th) = cap_tex_size(w, h);
    let src = if (tw, th) != (w, h) {
        image::imageops::resize(rgb, tw, th, image::imageops::FilterType::Triangle)
    } else {
        rgb.clone()
    };
    let frame = image::Frame::new(rgba_from_rgb(&src));
    Arc::new(RenderImage::new(smallvec::smallvec![frame]))
}

#[cfg(test)]
mod tile_sample_tests {
    use super::{tile_sample, TILE_BLEED};

    #[test]
    fn adjacent_tiles_share_an_edge_and_keep_bleed_outside() {
        let a = tile_sample(0, 0, 1200, 800).expect("first tile");
        let b = tile_sample(1, 0, 1200, 800).expect("second tile");
        assert_eq!(a.x + a.w as i32, b.x);
        assert_eq!(a.pad_l, 0);
        assert_eq!(a.pad_r, TILE_BLEED);
        assert_eq!(b.pad_l, TILE_BLEED);
        assert_eq!(a.sx + a.pad_l, a.x as u32);
        assert_eq!(b.sx + b.pad_l, b.x as u32);
        assert!(a.sx + a.sw > a.x as u32 + a.w);
    }
}

fn rgba_from_rgb(rgb: &RgbImage) -> image::RgbaImage {
    let (w, h) = rgb.dimensions();
    let mut out = image::RgbaImage::new(w, h);
    for (x, y, p) in rgb.enumerate_pixels() {
        out.put_pixel(x, y, image::Rgba([p[0], p[1], p[2], 255]));
    }
    out
}
