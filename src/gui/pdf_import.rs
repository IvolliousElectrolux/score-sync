//! PDF 导入弹窗: 拖入/点选后选择渲染像素.

use super::*;
use super::ScoreSyncApp;
use crate::pdf::{
    clamp_pdf_scale, inspect_pdf, parse_page_selection, px_from_pt, render_pdf_page_preview,
    scale_from_target, PdfInspect, PdfSizeGroup, DEFAULT_PDF_SCALE, PDF_MAX_SIDE_PX,
};
use image::{Frame, ImageBuffer, RgbaImage};
use smallvec::smallvec;
use std::path::Path;

pub(super) enum ImportItem {
    Pdf {
        info: PdfInspect,
        page_input: Entity<TextInput>,
    },
    PdfPending {
        path: PathBuf,
        name: String,
        page_input: Entity<TextInput>,
    },
    Image { path: PathBuf, name: String },
}

impl ImportItem {
    fn path(&self) -> &PathBuf {
        match self {
            Self::Pdf { info, .. } => &info.path,
            Self::PdfPending { path, .. } | Self::Image { path, .. } => path,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Pdf { info, .. } => info.name.as_str(),
            Self::PdfPending { name, .. } | Self::Image { name, .. } => name.as_str(),
        }
    }

    fn as_pdf(&self) -> Option<&PdfInspect> {
        match self {
            Self::Pdf { info, .. } => Some(info),
            _ => None,
        }
    }

    fn page_input(&self) -> Option<&Entity<TextInput>> {
        match self {
            Self::Pdf { page_input, .. } | Self::PdfPending { page_input, .. } => Some(page_input),
            Self::Image { .. } => None,
        }
    }

    fn is_pdf(&self) -> bool {
        !matches!(self, Self::Image { .. })
    }

    fn page_count(&self) -> u32 {
        match self {
            Self::Pdf { info, .. } => (info.page_count as u32).max(1),
            Self::PdfPending { .. } | Self::Image { .. } => 1,
        }
    }
}

struct ImportListDrag {
    from: usize,
    to: usize,
    line_at: Option<usize>,
    line_after: bool,
    start_x: f32,
    start_y: f32,
    armed: bool,
}

pub(super) struct PdfImportState {
    pub items: Vec<ImportItem>,
    pub loading: bool,
    pub error: Option<String>,
    pub lock_aspect: bool,
    pub target_w: u32,
    pub target_h: u32,
    pub scale: f32,
    pub inspect_gen: u64,
    pub inspect_inflight: u32,
    active: Option<usize>,
    preview_page: u32,
    preview_image: Option<Arc<RenderImage>>,
    preview_shown: Option<(PathBuf, u32)>,
    preview_gen: u64,
    preview_loading: bool,
    item_bounds: HashMap<usize, Bounds<Pixels>>,
    list_drag: Option<ImportListDrag>,
}

const PREVIEW_MAX_SIDE: u32 = 512;

impl PdfImportState {
    fn new(lock_aspect: bool, scale: f32) -> Self {
        Self {
            items: Vec::new(),
            loading: false,
            error: None,
            lock_aspect,
            target_w: 0,
            target_h: 0,
            scale: clamp_pdf_scale(scale),
            inspect_gen: 0,
            inspect_inflight: 0,
            active: None,
            preview_page: 1,
            preview_image: None,
            preview_shown: None,
            preview_gen: 0,
            preview_loading: false,
            item_bounds: HashMap::new(),
            list_drag: None,
        }
    }

    fn pdfs(&self) -> impl Iterator<Item = &PdfInspect> {
        self.items.iter().filter_map(ImportItem::as_pdf)
    }

    fn has_pdf(&self) -> bool {
        self.pdfs().next().is_some()
    }

    fn has_pending_pdf(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i, ImportItem::PdfPending { .. }))
    }

    fn contains_path(&self, p: &PathBuf) -> bool {
        self.items.iter().any(|i| i.path() == p)
    }

    fn mode(&self) -> Option<&PdfSizeGroup> {
        self.active
            .and_then(|i| self.items.get(i))
            .and_then(ImportItem::as_pdf)
            .and_then(|f| f.groups.first())
            .or_else(|| self.pdfs().next().and_then(|f| f.groups.first()))
    }

    /// 按目标像素宽给该文件每一页单独算倍率 (锁定时高度随该页比例).
    fn scales_for_file(&self, f: &PdfInspect) -> Vec<(f32, f32)> {
        let n = f.page_count.max(1);
        let mut out = vec![(self.scale, self.scale); n];
        let tw = self.target_w.max(1);
        let th = self.target_h.max(1);
        for g in &f.groups {
            let sx = scale_from_target(g.w_pt, tw);
            let sy = if self.lock_aspect {
                sx
            } else {
                scale_from_target(g.h_pt, th)
            };
            for &p in &g.pages {
                let i = (p as usize).saturating_sub(1);
                if i < out.len() {
                    out[i] = (sx, sy);
                }
            }
        }
        out
    }

    fn total_pages(&self) -> usize {
        let pdf_pages: usize = self.pdfs().map(|f| f.page_count).sum();
        let images = self
            .items
            .iter()
            .filter(|i| matches!(i, ImportItem::Image { .. }))
            .count();
        pdf_pages + images
    }

    fn resolve_drop(&self, from: usize, y: f32) -> (usize, Option<usize>, bool) {
        let n = self.items.len();
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.item_bounds.get(&i) else {
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
            let to = ScoreSyncApp::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    fn apply_reorder(&mut self, from: usize, to: usize) {
        if from == to || from >= self.items.len() {
            return;
        }
        let item = self.items.remove(from);
        let to = to.min(self.items.len());
        self.items.insert(to, item);
        self.item_bounds.clear();
        if let Some(a) = self.active {
            self.active = Some(Self::remap_index(a, from, to));
        }
    }

    fn remap_index(idx: usize, from: usize, to: usize) -> usize {
        if idx == from {
            to
        } else if from < to && idx > from && idx <= to {
            idx - 1
        } else if to < from && idx >= to && idx < from {
            idx + 1
        } else {
            idx
        }
    }

    fn clamp_active(&mut self) {
        if self.items.is_empty() {
            self.active = None;
            self.preview_page = 1;
            self.preview_image = None;
            self.preview_shown = None;
            self.preview_loading = false;
            return;
        }
        if let Some(a) = self.active {
            if a >= self.items.len() {
                self.active = Some(self.items.len() - 1);
                self.preview_page = 1;
            }
        }
    }

    fn active_page_count(&self) -> u32 {
        self.active
            .and_then(|i| self.items.get(i))
            .map(ImportItem::page_count)
            .unwrap_or(1)
    }
}

fn item_file_name(path: &PathBuf) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string()
}

fn fmt_pt(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i32)
    } else {
        format!("{v:.1}")
    }
}

fn trim_float(v: f32) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn thumbnail_rgb(rgb: image::RgbImage, max_side: u32) -> image::RgbImage {
    let w = rgb.width().max(1);
    let h = rgb.height().max(1);
    let m = w.max(h);
    if m <= max_side {
        return rgb;
    }
    let tw = ((w as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    let th = ((h as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    image::imageops::resize(&rgb, tw, th, image::imageops::FilterType::Triangle)
}

fn load_image_preview(path: &Path, max_side: u32) -> Result<image::RgbImage, String> {
    let rgb = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    Ok(thumbnail_rgb(rgb, max_side))
}

fn rgb_to_render_image(rgb: &image::RgbImage) -> Arc<RenderImage> {
    let (w, h) = rgb.dimensions();
    let src = rgb.as_raw();
    let n = src.len() / 3 * 4;
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(n);
    }
    for (dst, s) in buf.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
        dst[0] = s[2];
        dst[1] = s[1];
        dst[2] = s[0];
        dst[3] = 255;
    }
    let rgba: RgbaImage = ImageBuffer::from_raw(w, h, buf).expect("rgba buffer size matches w*h*4");
    Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]))
}

impl ScoreSyncApp {
    pub(super) fn open_import_dialog(&mut self, cx: &mut Context<Self>) {
        if self.pdf_import.is_some() {
            cx.notify();
            return;
        }
        let cfg = config::load();
        self.pdf_import = Some(PdfImportState::new(
            cfg.pdf_import_lock_aspect,
            cfg.pdf_import_scale,
        ));
        self.sync_pdf_import_inputs(cx);
        self.sync_preview_page_input(cx);
        cx.notify();
    }

    pub(super) fn close_import_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(st) = self.pdf_import.as_mut() {
            st.inspect_gen = st.inspect_gen.wrapping_add(1);
            st.preview_gen = st.preview_gen.wrapping_add(1);
        }
        self.pdf_import = None;
        cx.notify();
    }

    fn sync_preview_page_input(&mut self, cx: &mut Context<Self>) {
        let page = self
            .pdf_import
            .as_ref()
            .map(|st| st.preview_page)
            .unwrap_or(1);
        self.pdf_preview_page_input
            .update(cx, |i, cx| i.set_text(page.to_string(), cx));
    }

    fn activate_import_item(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        if idx >= st.items.len() {
            return;
        }
        let changed = st.active != Some(idx);
        st.active = Some(idx);
        if changed {
            st.preview_page = 1;
            self.sync_preview_page_input(cx);
        }
        self.request_import_preview(cx);
    }

    fn nudge_import_preview_page(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        let count = st.active_page_count().max(1);
        let cur = st.preview_page.max(1);
        let next = (cur as i32 + delta).clamp(1, count as i32) as u32;
        if next == cur {
            return;
        }
        st.preview_page = next;
        self.sync_preview_page_input(cx);
        self.request_import_preview(cx);
    }

    fn commit_preview_page_field(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_ref() else {
            return;
        };
        if st.active.is_none() {
            return;
        }
        let count = st.active_page_count().max(1);
        let txt = self.pdf_preview_page_input.read(cx).text();
        let parsed = txt.trim().parse::<u32>().ok().filter(|v| *v > 0);
        let page = parsed.unwrap_or(st.preview_page).clamp(1, count);
        let st = self.pdf_import.as_mut().unwrap();
        if st.preview_page != page {
            st.preview_page = page;
        }
        self.sync_preview_page_input(cx);
        self.request_import_preview(cx);
    }

    fn on_import_preview_scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y,
        };
        cx.stop_propagation();
        if dy.abs() < 0.01 {
            return;
        }
        let step = if dy < 0.0 { 1 } else { -1 };
        self.nudge_import_preview_page(step, cx);
    }

    fn request_import_preview(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        let Some(idx) = st.active else {
            st.preview_image = None;
            st.preview_shown = None;
            st.preview_loading = false;
            cx.notify();
            return;
        };
        let Some(item) = st.items.get(idx) else {
            return;
        };
        let path = item.path().clone();
        let is_pdf = item.is_pdf();
        let count = item.page_count().max(1);
        let page = st.preview_page.clamp(1, count);
        if st.preview_page != page {
            st.preview_page = page;
        }
        if st.preview_shown.as_ref() == Some(&(path.clone(), page)) && st.preview_image.is_some() {
            st.preview_loading = false;
            cx.notify();
            return;
        }
        let gen = st.preview_gen.wrapping_add(1);
        st.preview_gen = gen;
        st.preview_loading = true;
        if st.preview_shown.as_ref().map(|(p, _)| p != &path).unwrap_or(true) {
            st.preview_image = None;
        }
        cx.notify();
        let (tx, rx) = async_channel::bounded::<Result<(PathBuf, u32, Arc<RenderImage>), String>>(1);
        std::thread::spawn(move || {
            let rgb = if is_pdf {
                render_pdf_page_preview(&path, page, PREVIEW_MAX_SIDE).map_err(|e| e.to_string())
            } else {
                load_image_preview(&path, PREVIEW_MAX_SIDE)
            };
            let msg = rgb.map(|img| (path, page, rgb_to_render_image(&img)));
            let _ = tx.send_blocking(msg);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(res) = rx.recv().await {
                this.update(cx, |view, cx| {
                    let Some(st) = view.pdf_import.as_mut() else {
                        return;
                    };
                    if st.preview_gen != gen {
                        return;
                    }
                    st.preview_loading = false;
                    match res {
                        Ok((path, page, img)) => {
                            st.preview_shown = Some((path, page));
                            st.preview_image = Some(img);
                        }
                        Err(_) => {
                            st.preview_image = None;
                            st.preview_shown = None;
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn sync_pdf_import_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_ref() else {
            return;
        };
        let w = if st.target_w == 0 {
            String::new()
        } else {
            st.target_w.to_string()
        };
        let h = if st.target_h == 0 {
            String::new()
        } else {
            st.target_h.to_string()
        };
        let scale = if st.has_pdf() {
            trim_float(st.scale)
        } else {
            String::new()
        };
        self.pdf_w_input.update(cx, |i, cx| i.set_text(w, cx));
        self.pdf_h_input.update(cx, |i, cx| i.set_text(h, cx));
        self.pdf_scale_input
            .update(cx, |i, cx| i.set_text(scale, cx));
    }

    fn apply_mode_defaults(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        let Some(mode) = st.mode().cloned() else {
            return;
        };
        let mut scale = if st.scale < 0.5 {
            DEFAULT_PDF_SCALE
        } else {
            st.scale
        };
        let mut tw = px_from_pt(mode.w_pt, scale);
        let mut th = px_from_pt(mode.h_pt, scale);
        let mut img_scale = 0.0f32;
        for f in st.pdfs() {
            for g in &f.groups {
                if let Some((iw, ih)) = g.image_px {
                    let sx = iw as f32 / g.w_pt.max(1.0);
                    let sy = ih as f32 / g.h_pt.max(1.0);
                    img_scale = img_scale.max(sx.max(sy));
                }
            }
        }
        if img_scale > scale {
            scale = clamp_pdf_scale(img_scale);
            tw = px_from_pt(mode.w_pt, scale);
            th = px_from_pt(mode.h_pt, scale);
        }
        st.scale = scale;
        st.target_w = tw;
        st.target_h = th;
        self.sync_pdf_import_inputs(cx);
    }

    pub(super) fn import_dialog_add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if let Some(proj) = paths.iter().rev().find(|p| is_project_path(p)).cloned() {
            self.close_import_dialog(cx);
            self.open_project_path(proj, cx);
            return;
        }
        self.open_import_dialog(cx);
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        let mut pdfs = Vec::new();
        let start_len = st.items.len();
        for p in paths {
            if st.contains_path(&p) || pdfs.contains(&p) {
                continue;
            }
            if is_pdf_path(&p) {
                let name = item_file_name(&p);
                let page_input = cx.new(|cx| {
                    TextInput::new(cx, "", "如 1, 3-7").with_compact(true)
                });
                st.items.push(ImportItem::PdfPending {
                    path: p.clone(),
                    name,
                    page_input,
                });
                pdfs.push(p);
            } else if is_image_path(&p) {
                let name = item_file_name(&p);
                st.items.push(ImportItem::Image { path: p, name });
            }
        }
        if st.active.is_none() && st.items.len() > start_len {
            st.active = Some(start_len);
            st.preview_page = 1;
        }
        if pdfs.is_empty() {
            self.sync_preview_page_input(cx);
            self.request_import_preview(cx);
            cx.notify();
            return;
        }
        st.inspect_inflight = st.inspect_inflight.saturating_add(pdfs.len() as u32);
        st.loading = true;
        st.error = None;
        let gen = st.inspect_gen;
        cx.notify();
        self.sync_preview_page_input(cx);
        self.request_import_preview(cx);
        let (tx, rx) = async_channel::unbounded::<Result<PdfInspect, (PathBuf, String)>>();
        std::thread::spawn(move || {
            for p in pdfs {
                let msg = match inspect_pdf(&p) {
                    Ok(info) => Ok(info),
                    Err(e) => Err((p, e.to_string())),
                };
                if tx.send_blocking(msg).is_err() {
                    break;
                }
            }
        });
        cx.spawn(async move |this, cx| {
            while let Ok(res) = rx.recv().await {
                this.update(cx, |view, cx| {
                    let first = {
                        let Some(st) = view.pdf_import.as_mut() else {
                            return;
                        };
                        if st.inspect_gen != gen {
                            return;
                        }
                        st.inspect_inflight = st.inspect_inflight.saturating_sub(1);
                        st.loading = st.inspect_inflight > 0;
                        match res {
                            Ok(info) => {
                                let path = info.path.clone();
                                let first = !st.has_pdf();
                                if let Some(slot) = st.items.iter_mut().find(|i| {
                                    matches!(i, ImportItem::PdfPending { path: p, .. } if *p == path)
                                }) {
                                    let page_input = match slot {
                                        ImportItem::PdfPending { page_input, .. } => {
                                            page_input.clone()
                                        }
                                        ImportItem::Pdf { page_input, .. } => page_input.clone(),
                                        ImportItem::Image { .. } => unreachable!(),
                                    };
                                    *slot = ImportItem::Pdf { info, page_input };
                                }
                                st.error = None;
                                first
                            }
                            Err((path, msg)) => {
                                st.items.retain(|i| {
                                    !matches!(i, ImportItem::PdfPending { path: p, .. } if *p == path)
                                });
                                st.clamp_active();
                                st.error = Some(format!("{}: {msg}", path.display()));
                                false
                            }
                        }
                    };
                    if first {
                        view.apply_mode_defaults(cx);
                    } else {
                        view.sync_pdf_import_inputs(cx);
                    }
                    view.request_import_preview(cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn import_dialog_pick_files(&mut self, cx: &mut Context<Self>) {
        Self::spawn_native_dialog(
            cx,
            || {
                rfd::FileDialog::new()
                    .set_title("打开图片 / PDF (可多选)")
                    .add_filter(
                        "图片 / PDF",
                        &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "pdf"],
                    )
                    .add_filter("PDF", &["pdf"])
                    .add_filter("图片", &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"])
                    .pick_files()
            },
            |this, files, cx| {
                if let Some(paths) = files {
                    this.import_dialog_add_paths(paths, cx);
                }
            },
        );
    }

    fn pdf_import_field_focused(&self, window: &Window, cx: &App) -> bool {
        if self.pdf_w_input.focus_handle(cx).is_focused(window)
            || self.pdf_h_input.focus_handle(cx).is_focused(window)
            || self.pdf_scale_input.focus_handle(cx).is_focused(window)
            || self.pdf_preview_page_input.focus_handle(cx).is_focused(window)
        {
            return true;
        }
        self.pdf_import.as_ref().is_some_and(|st| {
            st.items.iter().any(|i| {
                i.page_input()
                    .is_some_and(|inp| inp.focus_handle(cx).is_focused(window))
            })
        })
    }

    /// 输入框有焦点时 Enter 只提交该框; 失焦后才确认导入.
    pub(super) fn on_pdf_import_enter(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.pdf_import_field_focused(window, cx) {
            self.commit_pdf_import_fields(cx);
            self.commit_preview_page_field(cx);
        } else {
            self.confirm_pdf_import(cx);
        }
    }

    fn blur_pdf_import_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pdf_import_field_focused(window, cx) {
            return;
        }
        self.focus_handle.focus(window);
        self.commit_pdf_import_fields(cx);
        self.commit_preview_page_field(cx);
    }

    fn commit_pdf_import_fields(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_ref() else {
            return;
        };
        if !st.has_pdf() {
            return;
        }
        let w_txt = self.pdf_w_input.read(cx).text();
        let h_txt = self.pdf_h_input.read(cx).text();
        let s_txt = self.pdf_scale_input.read(cx).text();
        let Some(mode) = st.mode().map(|g| (g.w_pt, g.h_pt)) else {
            return;
        };
        let st = self.pdf_import.as_mut().unwrap();
        let (sw, sh) = mode;
        let scale_shown = trim_float(st.scale);
        if s_txt.trim() != scale_shown {
            if let Ok(s) = s_txt.trim().parse::<f32>() {
                if (s - st.scale).abs() > 0.0005 {
                    st.scale = clamp_pdf_scale(s);
                    st.target_w = px_from_pt(sw, st.scale);
                    st.target_h = px_from_pt(sh, st.scale);
                    self.sync_pdf_import_inputs(cx);
                    cx.notify();
                    return;
                }
            }
        }
        let parsed_w = w_txt.trim().parse::<u32>().ok().filter(|v| *v > 0);
        let parsed_h = h_txt.trim().parse::<u32>().ok().filter(|v| *v > 0);
        if let Some(w) = parsed_w {
            if w != st.target_w {
                let w = w.min(PDF_MAX_SIDE_PX);
                if st.lock_aspect {
                    st.scale = scale_from_target(sw, w);
                    st.target_w = w;
                    let h = ((w as f32) * (sh / sw.max(0.5))).round() as u32;
                    st.target_h = h.clamp(1, PDF_MAX_SIDE_PX);
                } else {
                    st.target_w = w;
                    st.scale = scale_from_target(sw, w);
                }
                self.sync_pdf_import_inputs(cx);
                cx.notify();
                return;
            }
        }
        if let Some(h) = parsed_h {
            if h != st.target_h {
                let h = h.min(PDF_MAX_SIDE_PX);
                if st.lock_aspect {
                    st.scale = scale_from_target(sh, h);
                    st.target_h = h;
                    let w = ((h as f32) * (sw / sh.max(0.5))).round() as u32;
                    st.target_w = w.clamp(1, PDF_MAX_SIDE_PX);
                } else {
                    st.target_h = h;
                }
                self.sync_pdf_import_inputs(cx);
                cx.notify();
            }
        }
    }

    fn toggle_pdf_lock_aspect(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        st.lock_aspect = !st.lock_aspect;
        if st.lock_aspect {
            if let Some(mode) = st.mode().map(|g| (g.w_pt, g.h_pt)) {
                st.target_w = px_from_pt(mode.0, st.scale);
                st.target_h = px_from_pt(mode.1, st.scale);
            }
        }
        self.sync_pdf_import_inputs(cx);
        cx.notify();
    }

    pub(super) fn confirm_pdf_import(&mut self, cx: &mut Context<Self>) {
        self.commit_pdf_import_fields(cx);
        let Some(st) = self.pdf_import.as_ref() else {
            return;
        };
        if st.loading || st.has_pending_pdf() {
            return;
        }
        if st.items.is_empty() {
            return;
        }
        let any_pdf = st.has_pdf();
        let tw = st.target_w;
        let th = st.target_h.max(1);
        let lock = st.lock_aspect;
        let scale = st.scale;
        let mut page_err: Option<String> = None;
        let jobs: Vec<ImportJob> = st
            .items
            .iter()
            .filter_map(|item| match item {
                ImportItem::Pdf { info, page_input } => {
                    let txt = page_input.read(cx).text();
                    let pages = parse_page_selection(&txt, info.page_count);
                    if !txt.trim().is_empty() && pages.is_empty() {
                        page_err = Some(format!(
                            "{}: 页码无效, 请用如 1, 3-7 (共 {} 页)",
                            info.name, info.page_count
                        ));
                        return None;
                    }
                    Some(ImportJob::Pdf {
                        path: info.path.clone(),
                        scales: st.scales_for_file(info),
                        pages,
                    })
                }
                ImportItem::Image { path, .. } => Some(ImportJob::Image {
                    path: path.clone(),
                    target: if any_pdf && tw > 0 {
                        Some((tw, th, lock))
                    } else {
                        None
                    },
                }),
                ImportItem::PdfPending { .. } => None,
            })
            .collect();
        if let Some(msg) = page_err {
            if let Some(st) = self.pdf_import.as_mut() {
                st.error = Some(msg);
            }
            cx.notify();
            return;
        }
        config::remember_pdf_import(scale, lock);
        self.pdf_import = None;
        if jobs.is_empty() {
            cx.notify();
            return;
        }
        self.push_crop_undo_page_structure();
        self.start_import_jobs(jobs, false, cx);
        cx.notify();
    }

    fn remove_import_item(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        if idx >= st.items.len() {
            return;
        }
        let was_active = st.active == Some(idx);
        st.items.remove(idx);
        st.item_bounds.clear();
        st.list_drag = None;
        if let Some(a) = st.active {
            if a == idx {
                st.active = if st.items.is_empty() {
                    None
                } else {
                    Some(idx.min(st.items.len() - 1))
                };
                st.preview_page = 1;
                st.preview_image = None;
                st.preview_shown = None;
            } else if a > idx {
                st.active = Some(a - 1);
            }
        }
        if was_active {
            self.sync_preview_page_input(cx);
        }
        self.sync_pdf_import_inputs(cx);
        self.request_import_preview(cx);
        cx.notify();
    }

    fn import_list_drag_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        let Some(ImportListDrag {
            from,
            start_x,
            start_y,
            mut armed,
            ..
        }) = st.list_drag.take()
        else {
            return;
        };
        if !armed && ScoreSyncApp::reorder_slop_exceeded(x - start_x, y - start_y) {
            armed = true;
        }
        let (to, line_at, line_after) = if armed {
            st.resolve_drop(from, y)
        } else {
            (from, None, false)
        };
        st.list_drag = Some(ImportListDrag {
            from,
            to,
            line_at,
            line_after,
            start_x,
            start_y,
            armed,
        });
        cx.notify();
    }

    fn finish_import_list_drag(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.pdf_import.as_mut() else {
            return;
        };
        let Some(drag) = st.list_drag.take() else {
            return;
        };
        if drag.armed && drag.from != drag.to {
            st.apply_reorder(drag.from, drag.to);
        }
        cx.notify();
    }

    fn measure_import_row(entity: Entity<Self>, idx: usize) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    if let Some(st) = this.pdf_import.as_mut() {
                        st.item_bounds.insert(idx, bounds);
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    pub(super) fn pdf_import_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pdf_import.is_some() {
            let w_blur = self.pdf_w_input.update(cx, |i, _| i.take_blur_commit());
            let h_blur = self.pdf_h_input.update(cx, |i, _| i.take_blur_commit());
            let s_blur = self
                .pdf_scale_input
                .update(cx, |i, _| i.take_blur_commit());
            let p_blur = self
                .pdf_preview_page_input
                .update(cx, |i, _| i.take_blur_commit());
            if w_blur || h_blur || s_blur {
                self.commit_pdf_import_fields(cx);
            }
            if p_blur {
                self.commit_preview_page_field(cx);
            }
            let focus_idx = {
                let inputs: Vec<(usize, Entity<TextInput>)> = self
                    .pdf_import
                    .as_ref()
                    .map(|st| {
                        st.items
                            .iter()
                            .enumerate()
                            .filter_map(|(i, item)| {
                                item.page_input().cloned().map(|e| (i, e))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                inputs.into_iter().find_map(|(i, e)| {
                    e.update(cx, |t, _| t.take_focus_commit()).then_some(i)
                })
            };
            if let Some(idx) = focus_idx {
                self.activate_import_item(idx, cx);
            }
        }
        let Some(st) = self.pdf_import.as_ref() else {
            return div().into_any_element();
        };
        let has_pdf = st.has_pdf();
        let n_items = st.items.len();
        let loading = st.loading;
        let lock = st.lock_aspect;
        let can_import = !loading && !st.items.is_empty() && !st.has_pending_pdf();
        let w_input = self.pdf_w_input.clone();
        let h_input = self.pdf_h_input.clone();
        let scale_input = self.pdf_scale_input.clone();
        let pages = st.total_pages();
        let mode = st.mode().cloned();
        let target_w = st.target_w;
        let target_h = st.target_h;
        let drag_from = st.list_drag.as_ref().and_then(|d| d.armed.then_some(d.from));
        let (line_at, line_after) = match &st.list_drag {
            Some(d) if d.armed => (d.line_at, d.line_after),
            _ => (None, false),
        };
        let active_idx = st.active;
        let preview_img = st.preview_image.clone();
        let preview_loading = st.preview_loading;
        let preview_count = st.active_page_count();
        let preview_page_input = self.pdf_preview_page_input.clone();
        let item_rows: Vec<(usize, String, String, bool, bool, Option<Entity<TextInput>>)> = st
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let sub = match item {
                    ImportItem::Pdf { info, .. } => format!("PDF · {} 页", info.page_count),
                    ImportItem::PdfPending { .. } => "PDF · 读取中…".into(),
                    ImportItem::Image { .. } => {
                        if has_pdf && target_w > 0 {
                            if lock {
                                format!("图片 · 齐宽 {target_w} px")
                            } else {
                                format!("图片 · 拉伸到 {target_w}×{target_h}")
                            }
                        } else {
                            "图片 · 原像素".into()
                        }
                    }
                };
                (
                    i,
                    item.name().to_string(),
                    sub,
                    matches!(item, ImportItem::PdfPending { .. }),
                    active_idx == Some(i),
                    item.page_input().cloned(),
                )
            })
            .collect();
        let err = st.error.clone();
        let entity = cx.entity();

        let drop_hint: SharedString = if loading {
            "正在读取 PDF…".into()
        } else if n_items > 0 {
            "点击或拖入以添加更多".into()
        } else {
            "请拖入文件".into()
        };

        let mut list = div()
            .id("pdf_import_items")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .gap_1()
            .w_full()
            .overflow_y_scroll();
        for (idx, name, sub, pending, is_active, page_input) in item_rows {
            let dragging = drag_from == Some(idx);
            let show_line = line_at == Some(idx);
            list = list.child(
                div()
                    .id(SharedString::from(format!("pdf_import_item-{idx}")))
                    .relative()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if is_active { rgb(0xeff6ff) } else { rgb(0xffffff) })
                    .border_1()
                    .border_color(if is_active {
                        rgb(0x3b82f6)
                    } else {
                        rgb(0xe2e8f0)
                    })
                    .cursor_move()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !line_after, |d| {
                        d.border_t_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && line_after, |d| {
                        d.border_b_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_import_row(entity.clone(), idx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x0f172a))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .w_full()
                                    .child(
                                        div()
                                            .text_xs()
                                            .flex_shrink_0()
                                            .text_color(if pending {
                                                rgb(0xb45309)
                                            } else {
                                                rgb(0x64748b)
                                            })
                                            .child(sub),
                                    )
                                    .when_some(page_input, |d, inp| {
                                        d.child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "pdf_import_pages-{idx}"
                                                )))
                                                .flex_1()
                                                .min_w(px(72.))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.activate_import_item(idx, cx);
                                                        cx.stop_propagation();
                                                    }),
                                                )
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|_, _, _, cx| {
                                                        cx.stop_propagation()
                                                    }),
                                                )
                                                .child(inp),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("pdf_import_item_x-{idx}")))
                            .px_1()
                            .rounded_sm()
                            .text_color(rgb(0x64748b))
                            .hover(|s| s.bg(rgb(0xe2e8f0)).text_color(rgb(0x0f172a)))
                            .cursor_pointer()
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if this
                                        .pdf_import
                                        .as_ref()
                                        .and_then(|s| s.list_drag.as_ref())
                                        .is_some_and(|d| d.armed)
                                    {
                                        return;
                                    }
                                    this.remove_import_item(idx, cx);
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            if let Some(st) = this.pdf_import.as_mut() {
                                st.list_drag = Some(ImportListDrag {
                                    from: idx,
                                    to: idx,
                                    line_at: None,
                                    line_after: false,
                                    start_x: mx,
                                    start_y: my,
                                    armed: false,
                                });
                            }
                            this.activate_import_item(idx, cx);
                        }),
                    ),
            );
        }

        let mut drop = div()
            .id("pdf_import_drop")
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(rgb(0x94a3b8))
            .bg(rgb(0xf8fafc))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .p_2()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths
                    .paths()
                    .iter()
                    .filter(|p| is_open_path(p) || is_project_path(p))
                    .cloned()
                    .collect();
                if !list.is_empty() {
                    this.import_dialog_add_paths(list, cx);
                }
            }));
        if n_items == 0 {
            drop = drop
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0xf1f5f9)).border_color(rgb(0x64748b)))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.import_dialog_pick_files(cx)),
                )
                .child(div().text_3xl().text_color(rgb(0x64748b)).child("📎"))
                .child(div().text_sm().text_color(rgb(0x475569)).child(drop_hint));
        } else {
            drop = drop
                .overflow_hidden()
                .child(
                    div()
                        .id("pdf_import_add_more")
                        .w_full()
                        .flex_shrink_0()
                        .py_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0x475569))
                        .hover(|s| s.text_color(rgb(0x0f172a)))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.import_dialog_pick_files(cx)),
                        )
                        .child("📎")
                        .child(drop_hint),
                )
                .child(list);
        }

        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .gap_2()
            .child(drop);
        if let Some(err) = err {
            body = body.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(0xb91c1c))
                    .child(err),
            );
        }
        if let Some(mode) = mode.as_ref() {
            let header = format!("{n_items} 个文件 · 共 {pages} 页");
            body = body.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(0x334155))
                    .child(header),
            );
            body = body.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(0x64748b))
                    .child("源尺寸 (只读)")
                    .child(
                        div()
                            .w(px(72.))
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0xe2e8f0))
                            .bg(rgb(0xf1f5f9))
                            .flex()
                            .items_center()
                            .text_color(rgb(0x94a3b8))
                            .child(fmt_pt(mode.w_pt)),
                    )
                    .child("×")
                    .child(
                        div()
                            .w(px(72.))
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0xe2e8f0))
                            .bg(rgb(0xf1f5f9))
                            .flex()
                            .items_center()
                            .text_color(rgb(0x94a3b8))
                            .child(fmt_pt(mode.h_pt)),
                    )
                    .child("pt"),
            );
            body = body.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(0x334155))
                    .child("目标")
                    .child(
                        div()
                            .w(px(72.))
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0xcbd5e1))
                            .bg(rgb(0xffffff))
                            .child(w_input),
                    )
                    .child("×")
                    .child(
                        div()
                            .w(px(72.))
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0xcbd5e1))
                            .bg(rgb(0xffffff))
                            .child(h_input),
                    )
                    .child("px")
                    .child(
                        div()
                            .id("pdf_lock_aspect")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .child(if lock { "☑" } else { "☐" })
                            .child("锁定宽高比")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.toggle_pdf_lock_aspect(cx)),
                            ),
                    ),
            );
            body = body.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(0x334155))
                    .child("倍率")
                    .child(
                        div()
                            .w(px(64.))
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0xcbd5e1))
                            .bg(rgb(0xffffff))
                            .child(scale_input),
                    )
                    .child("相对参考页的 PDF 标记尺寸 (72 pt = 1 inch)"),
            );
        }

        let preview_hint: SharedString = if n_items == 0 {
            "选择文件后预览".into()
        } else if preview_loading && preview_img.is_none() {
            "载入预览…".into()
        } else if preview_img.is_none() {
            "无法预览".into()
        } else {
            "".into()
        };
        let paint_img = preview_img.clone();
        let preview_canvas = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let Some(img) = &paint_img else {
                    return;
                };
                let sz = img.size(0);
                let iw = (sz.width.0 as f32).max(1.0);
                let ih = (sz.height.0 as f32).max(1.0);
                let vw = f32::from(bounds.size.width);
                let vh = f32::from(bounds.size.height);
                let fit = (vw / iw).min(vh / ih).max(0.0001);
                let dw = iw * fit;
                let dh = ih * fit;
                let ox = bounds.origin.x + px((vw - dw) * 0.5);
                let oy = bounds.origin.y + px((vh - dh) * 0.5);
                let img_bounds = Bounds {
                    origin: point(ox, oy),
                    size: size(px(dw), px(dh)),
                };
                let _ = window.paint_image(
                    img_bounds,
                    gpui::Corners::default(),
                    img.clone(),
                    0,
                    false,
                );
            },
        )
        .size_full();
        let show_preview_hint = preview_img.is_none();
        let preview_pane = div()
            .id("pdf_import_preview")
            .w(px(226.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                this.on_import_preview_scroll(ev, cx);
            }))
            .child(
                div()
                    .id("pdf_import_preview_img")
                    .w_full()
                    .h(px(320.))
                    .flex_shrink_0()
                    .bg(rgb(0xf1f5f9))
                    .border_1()
                    .border_color(rgb(0xe2e8f0))
                    .overflow_hidden()
                    .relative()
                    .child(
                        preview_canvas
                            .absolute()
                            .inset_0()
                            .size_full(),
                    )
                    .when(show_preview_hint, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(0x64748b))
                                .child(preview_hint),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_xs()
                    .text_color(rgb(0x334155))
                    .child("页")
                    .child(
                        div()
                            .w(px(52.))
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0xcbd5e1))
                            .bg(rgb(0xffffff))
                            .child(preview_page_input),
                    )
                    .child(format!("/ {preview_count}")),
            );

        div()
            .id("pdf_import_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.import_list_drag_move(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_import_list_drag(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.blur_pdf_import_fields(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths
                    .paths()
                    .iter()
                    .filter(|p| is_open_path(p) || is_project_path(p))
                    .cloned()
                    .collect();
                if !list.is_empty() {
                    this.import_dialog_add_paths(list, cx);
                }
            }))
            .child(
                div()
                    .id("pdf_import_card")
                    .w(px(780.))
                    .h(px(480.))
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.blur_pdf_import_fields(window, cx);
                        }),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("打开文件"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.))
                            .gap_3()
                            .child(body)
                            .child(preview_pane),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "pdf_import_cancel",
                                "取消",
                                false,
                                |this, _, cx| this.close_import_dialog(cx),
                                cx,
                            ))
                            .child(self.btn(
                                "pdf_import_ok",
                                "导入",
                                can_import,
                                |this, _, cx| this.confirm_pdf_import(cx),
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }
}
