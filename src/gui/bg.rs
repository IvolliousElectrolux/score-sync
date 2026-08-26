//! 底色面板: 右侧栏选图、纯色取色、应用到工程组合.

use super::*;
use super::ScoreSyncApp;
use apply_bg::process::is_image;
use image::{Frame, ImageBuffer, Rgb, RgbaImage};
use mask_tool::color_prefs::{hsv_to_rgb, rgb_to_hsv};
use smallvec::smallvec;
use std::path::Path;

const BG_HUE_W: f32 = 16.0;
const BG_RECENT_W: f32 = 22.0;
const BG_RECENT_SWATCH: f32 = 20.0;
const BG_RECENT_GAP: f32 = 4.0;
const BG_RECENT_MIN: usize = 8;
const BG_RECENT_MAX: usize = 24;
const BG_SB_TEX: u32 = 128;
const BG_HUE_TEX: u32 = 256;
const BG_THUMB_MAX: u32 = 512;

pub(super) struct BgUi {
    pub pick_open: bool,
    pub batch_open: bool,
    pub pending_path: Option<PathBuf>,
    pub pending_preview: Option<Arc<RenderImage>>,
    /// 最近一次选中/应用的底色图 (取消、换纯色、再换文件都不丢).
    pub cached_image: Option<image::RgbImage>,
    pub cached_source_path: Option<PathBuf>,
    pub cached_session_path: Option<PathBuf>,
    /// 当前启用层是纯色 (仅 `doc.bg_enabled` 时有意义).
    pub applied_is_solid: bool,
    pub color: [u8; 3],
    pub picker_h: f32,
    pub picker_s: f32,
    pub picker_v: f32,
    pub sb_image: Option<Arc<RenderImage>>,
    pub hue_image: Option<Arc<RenderImage>>,
    pub sb_bounds: Bounds<Pixels>,
    pub hue_bounds: Bounds<Pixels>,
    pub rgb_r: Entity<TextInput>,
    pub rgb_g: Entity<TextInput>,
    pub rgb_b: Entity<TextInput>,
    pub rgb_syncing: bool,
    pub aspect_w: Entity<TextInput>,
    pub aspect_h: Entity<TextInput>,
    pub aspect_syncing: bool,
    pub eyedropper_armed: bool,
    pub recent: Vec<[u8; 3]>,
}

impl BgUi {
    pub fn new(cx: &mut Context<ScoreSyncApp>) -> Self {
        let rgb_r = cx.new(|cx| TextInput::new(cx, "253", "R").with_compact(true));
        let rgb_g = cx.new(|cx| TextInput::new(cx, "253", "G").with_compact(true));
        let rgb_b = cx.new(|cx| TextInput::new(cx, "253", "B").with_compact(true));
        let aspect_w = cx.new(|cx| TextInput::new(cx, "2560", "宽").with_compact(true));
        let aspect_h = cx.new(|cx| TextInput::new(cx, "1440", "高").with_compact(true));
        let color = [253, 253, 253];
        let (h, s, v) = rgb_to_hsv(color);
        let mut ui = Self {
            pick_open: false,
            batch_open: false,
            pending_path: None,
            pending_preview: None,
            cached_image: None,
            cached_source_path: None,
            cached_session_path: None,
            applied_is_solid: false,
            color,
            picker_h: h,
            picker_s: s,
            picker_v: v,
            sb_image: None,
            hue_image: None,
            sb_bounds: Bounds::default(),
            hue_bounds: Bounds::default(),
            rgb_r,
            rgb_g,
            rgb_b,
            rgb_syncing: false,
            aspect_w,
            aspect_h,
            aspect_syncing: false,
            eyedropper_armed: false,
            recent: vec![
                [253, 253, 253],
                [255, 255, 255],
                [250, 204, 21],
                [0, 0, 0],
                [148, 163, 184],
                [56, 189, 248],
                [251, 146, 60],
                [74, 222, 128],
            ],
        };
        ui.rebuild_hue_image();
        ui.rebuild_sb_image();
        ui
    }

    fn picker_rgb(&self) -> [u8; 3] {
        hsv_to_rgb(self.picker_h, self.picker_s, self.picker_v)
    }

    fn rebuild_hue_image(&mut self) {
        let w = 4u32;
        let h = BG_HUE_TEX;
        let mut rgba: RgbaImage = ImageBuffer::new(w, h);
        for y in 0..h {
            let hue = 360.0 * y as f32 / (h - 1).max(1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, 1.0, 1.0);
            for x in 0..w {
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        self.hue_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
    }

    fn rebuild_sb_image(&mut self) {
        let size = BG_SB_TEX;
        let mut rgba: RgbaImage = ImageBuffer::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let s = x as f32 / (size - 1).max(1) as f32;
                let v = 1.0 - y as f32 / (size - 1).max(1) as f32;
                let [r, g, b] = hsv_to_rgb(self.picker_h, s, v);
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        self.sb_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
    }
}

fn color_u32(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
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

fn thumbnail_rgb(rgb: &image::RgbImage, max_side: u32) -> image::RgbImage {
    let (w, h) = rgb.dimensions();
    let m = w.max(h);
    if m <= max_side {
        return rgb.clone();
    }
    let tw = ((w as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    let th = ((h as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    image::imageops::resize(rgb, tw, th, image::imageops::FilterType::Triangle)
}

impl ScoreSyncApp {
    pub(super) fn uses_mask_canvas(&self) -> bool {
        matches!(self.side_tool, SideTool::Mask | SideTool::Project)
    }

    pub(super) fn observe_bg_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        cx.observe(&self.bg.rgb_r, |this, _, cx| this.apply_bg_rgb_inputs(cx))
            .detach();
        cx.observe(&self.bg.rgb_g, |this, _, cx| this.apply_bg_rgb_inputs(cx))
            .detach();
        cx.observe(&self.bg.rgb_b, |this, _, cx| this.apply_bg_rgb_inputs(cx))
            .detach();
        cx.observe(&self.bg.aspect_w, |this, _, cx| this.apply_bg_aspect_inputs(cx))
            .detach();
        cx.observe(&self.bg.aspect_h, |this, _, cx| this.apply_bg_aspect_inputs(cx))
            .detach();
    }

    pub(super) fn sync_bg_ui_from_doc(&mut self, cx: &mut Context<Self>) {
        if let Some(img) = self.doc.bg_image.as_ref() {
            let thumb = thumbnail_rgb(img, BG_THUMB_MAX);
            self.bg.pending_preview = Some(rgb_to_render_image(&thumb));
            self.bg.applied_is_solid =
                self.doc.bg_enabled && self.doc.bg_source_path.is_none();
            if !self.bg.applied_is_solid {
                self.bg.cached_image = Some((**img).clone());
                self.bg.cached_source_path = self.doc.bg_source_path.clone();
                if !self
                    .bg
                    .cached_session_path
                    .as_ref()
                    .map(|p| p.is_file())
                    .unwrap_or(false)
                {
                    if let Ok(p) = crate::page_cache::write_rgb_png(img, "bg_cache") {
                        self.bg.cached_session_path = Some(p);
                    }
                }
            }
            let p = img.get_pixel(img.width() / 2, img.height() / 2);
            self.set_bg_picker_rgb([p[0], p[1], p[2]], true, cx);
        } else {
            self.bg.pending_preview = None;
            self.bg.cached_image = None;
            self.bg.cached_source_path = None;
            self.bg.cached_session_path = None;
            self.bg.applied_is_solid = false;
        }
        self.bg.pending_path = self.doc.bg_source_path.clone();
        self.sync_bg_aspect_inputs(cx);
        cx.notify();
    }

    fn set_bg_picker_rgb(&mut self, rgb: [u8; 3], sync_inputs: bool, cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.bg.picker_h = h;
        self.bg.picker_s = s;
        self.bg.picker_v = v;
        self.bg.color = rgb;
        self.bg.rebuild_sb_image();
        if sync_inputs {
            self.sync_bg_rgb_inputs(cx);
        }
    }

    fn sync_bg_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        let [r, g, b] = self.bg.color;
        self.bg.rgb_syncing = true;
        self.bg
            .rgb_r
            .update(cx, |t, cx| t.set_text(r.to_string(), cx));
        self.bg
            .rgb_g
            .update(cx, |t, cx| t.set_text(g.to_string(), cx));
        self.bg
            .rgb_b
            .update(cx, |t, cx| t.set_text(b.to_string(), cx));
        self.bg.rgb_syncing = false;
    }

    fn apply_bg_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        if self.bg.rgb_syncing {
            return;
        }
        let parse = |s: String| -> Option<u8> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            t.parse::<u8>().ok()
        };
        let r = parse(self.bg.rgb_r.read(cx).text());
        let g = parse(self.bg.rgb_g.read(cx).text());
        let b = parse(self.bg.rgb_b.read(cx).text());
        let (Some(r), Some(g), Some(b)) = (r, g, b) else {
            return;
        };
        if [r, g, b] == self.bg.color {
            return;
        }
        self.set_bg_picker_rgb([r, g, b], false, cx);
        cx.notify();
    }

    fn sync_bg_aspect_inputs(&mut self, cx: &mut Context<Self>) {
        self.bg.aspect_syncing = true;
        self.bg.aspect_w.update(cx, |t, cx| {
            t.set_text(self.doc.bg_aspect_w.max(1).to_string(), cx)
        });
        self.bg.aspect_h.update(cx, |t, cx| {
            t.set_text(self.doc.bg_aspect_h.max(1).to_string(), cx)
        });
        self.bg.aspect_syncing = false;
    }

    fn apply_bg_aspect_inputs(&mut self, cx: &mut Context<Self>) {
        if self.bg.aspect_syncing {
            return;
        }
        let parse = |s: String| -> Option<u32> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            t.parse::<u32>().ok().filter(|v| *v > 0)
        };
        let w = parse(self.bg.aspect_w.read(cx).text());
        let h = parse(self.bg.aspect_h.read(cx).text());
        let (Some(w), Some(h)) = (w, h) else {
            return;
        };
        if w == self.doc.bg_aspect_w && h == self.doc.bg_aspect_h {
            return;
        }
        if self.doc.bg_enabled {
            self.push_bg_undo();
            self.doc.bg_aspect_w = w;
            self.doc.bg_aspect_h = h;
            if self.bg.applied_is_solid {
                self.apply_solid_inner(cx);
            } else {
                self.apply_image_inner(cx);
            }
        } else {
            self.doc.bg_aspect_w = w;
            self.doc.bg_aspect_h = h;
        }
        cx.notify();
    }

    pub(super) fn apply_bg_eyedropper(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        self.set_bg_picker_rgb(rgb, true, cx);
        self.push_bg_recent(rgb);
        self.bg.eyedropper_armed = false;
        self.mask_tool
            .update(cx, |m, cx| m.set_host_pick_armed(false, cx));
        self.status = format!("已取色 RGB {},{},{}", rgb[0], rgb[1], rgb[2]).into();
        self.hint = self.status.clone();
        cx.notify();
    }

    pub(super) fn preview_bg_eyedropper(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        if rgb == self.bg.color {
            return;
        }
        self.set_bg_picker_rgb(rgb, true, cx);
        cx.notify();
    }

    fn push_bg_recent(&mut self, color: [u8; 3]) {
        self.bg.recent.retain(|c| *c != color);
        self.bg.recent.insert(0, color);
        while self.bg.recent.len() > BG_RECENT_MAX {
            self.bg.recent.pop();
        }
    }

    fn arm_bg_eyedropper(&mut self, cx: &mut Context<Self>) {
        if self.bg.eyedropper_armed {
            self.bg.eyedropper_armed = false;
            self.mask_tool
                .update(cx, |m, cx| m.set_host_pick_armed(false, cx));
            self.status = "已取消取色".into();
        } else {
            self.bg.eyedropper_armed = true;
            self.mask_tool
                .update(cx, |m, cx| m.set_host_pick_armed(true, cx));
            self.status = "取色: 在左侧预览上移动预览, 单击确认, 右键取消".into();
        }
        self.hint = self.status.clone();
        cx.notify();
    }

    pub(super) fn cycle_bg_preview_group(&mut self, dir: i32, cx: &mut Context<Self>) {
        let n = self.doc.groups.len();
        if n == 0 || dir == 0 {
            return;
        }
        let cur = self
            .mask_target
            .as_ref()
            .and_then(|id| self.doc.groups.iter().position(|g| &g.id == id))
            .unwrap_or(0);
        let next = (cur as i32 + dir).rem_euclid(n as i32) as usize;
        let gid = self.doc.groups[next].id.clone();
        self.set_mask_target(gid, true, cx);
    }

    fn open_bg_pick(&mut self, cx: &mut Context<Self>) {
        self.bg.pick_open = true;
        cx.notify();
    }

    fn close_bg_pick(&mut self, cx: &mut Context<Self>) {
        self.bg.pick_open = false;
        cx.notify();
    }

    fn confirm_bg_pick(&mut self, cx: &mut Context<Self>) {
        if self.bg.pending_path.is_none() && self.bg.pending_preview.is_none() {
            return;
        }
        self.bg.pick_open = false;
        if self.doc.bg_enabled && !self.bg.applied_is_solid {
            self.push_bg_undo();
            self.apply_image_inner(cx);
        } else {
            self.status = "已选择底色图, 点「应用底色」叠到工程组合.".into();
            self.hint = self.status.clone();
            cx.notify();
        }
    }

    fn set_pending_bg_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !is_image(&path) {
            self.show_error(
                "无法作为底色",
                crate::error::Error::msg(format!(
                    "只支持一张图片文件: {}",
                    path.display()
                )),
                cx,
            );
            return;
        }
        match image::open(&path) {
            Ok(im) => {
                let rgb = im.to_rgb8();
                let thumb = thumbnail_rgb(&rgb, BG_THUMB_MAX);
                self.cache_bg_file(path, rgb);
                self.bg.pending_preview = Some(rgb_to_render_image(&thumb));
                cx.notify();
            }
            Err(e) => {
                self.show_error(
                    "无法打开底色",
                    crate::error::Error::image_open(path.clone(), e),
                    cx,
                );
            }
        }
    }

    fn cache_bg_file(&mut self, path: PathBuf, rgb: image::RgbImage) {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("bg.png");
        let session = crate::page_cache::ingest_file(&path, name).or_else(|_| {
            crate::page_cache::write_rgb_png(&rgb, "bg_cache")
        });
        self.bg.cached_image = Some(rgb);
        self.bg.cached_source_path = Some(path.clone());
        self.bg.cached_session_path = session.ok();
        self.bg.pending_path = Some(path);
    }

    fn ensure_bg_session_cache(&mut self) {
        if self
            .bg
            .cached_session_path
            .as_ref()
            .map(|p| p.is_file())
            .unwrap_or(false)
        {
            return;
        }
        if let Some(img) = self.bg.cached_image.as_ref() {
            if let Ok(p) = crate::page_cache::write_rgb_png(img, "bg_cache") {
                self.bg.cached_session_path = Some(p);
            }
        }
    }

    fn load_cached_bg_image(&mut self) -> Option<image::RgbImage> {
        if let Some(img) = self.bg.cached_image.clone() {
            return Some(img);
        }
        if let Some(p) = self.bg.cached_session_path.clone() {
            if let Ok(img) = crate::page_cache::load_rgb(&p) {
                self.bg.cached_image = Some(img.clone());
                return Some(img);
            }
        }
        let path = self
            .bg
            .pending_path
            .clone()
            .or_else(|| self.bg.cached_source_path.clone())?;
        match image::open(&path) {
            Ok(im) => {
                let rgb = im.to_rgb8();
                self.bg.cached_image = Some(rgb.clone());
                Some(rgb)
            }
            Err(_) => None,
        }
    }

    fn pick_bg_file_dialog(&mut self, cx: &mut Context<Self>) {
        let start = self
            .bg
            .pending_path
            .clone()
            .or_else(|| self.doc.bg_source_path.clone())
            .unwrap_or_default();
        Self::spawn_native_dialog(
            cx,
            move || {
                let mut dialog = rfd::FileDialog::new().set_title("选择底色").add_filter(
                    "Images",
                    &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"],
                );
                if start.is_file() {
                    dialog = dialog
                        .set_file_name(
                            start
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("底色.png"),
                        )
                        .set_directory(start.parent().unwrap_or(Path::new(".")));
                }
                dialog.pick_file()
            },
            |this, picked, cx| {
                if let Some(p) = picked {
                    this.set_pending_bg_path(p, cx);
                }
            },
        );
    }

    pub(super) fn apply_drop_as_bg(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let Some(p) = paths.iter().find(|p| is_image(p)).cloned() else {
            return;
        };
        self.set_pending_bg_path(p, cx);
    }

    fn toggle_project_bg(&mut self, cx: &mut Context<Self>) {
        if self.doc.bg_enabled && !self.bg.applied_is_solid {
            self.push_bg_undo();
            self.clear_project_bg(cx);
            return;
        }
        if self.load_cached_bg_image().is_none() {
            self.show_error(
                "无法应用底色",
                crate::error::Error::msg("请先点「选择底色」导入一张底色图."),
                cx,
            );
            return;
        }
        self.push_bg_undo();
        self.apply_image_inner(cx);
    }

    fn toggle_solid_project_bg(&mut self, cx: &mut Context<Self>) {
        if self.doc.bg_enabled && self.bg.applied_is_solid {
            self.push_bg_undo();
            self.clear_project_bg(cx);
            return;
        }
        self.push_bg_undo();
        self.apply_solid_inner(cx);
    }

    fn apply_image_inner(&mut self, cx: &mut Context<Self>) {
        let Some(img) = self.load_cached_bg_image() else {
            self.show_error(
                "无法应用底色",
                crate::error::Error::msg("请先点「选择底色」导入一张底色图."),
                cx,
            );
            return;
        };
        self.apply_project_bg_image(img, self.bg.cached_source_path.clone(), cx);
    }

    fn apply_solid_inner(&mut self, cx: &mut Context<Self>) {
        let color = self.bg.picker_rgb();
        self.bg.color = color;
        self.push_bg_recent(color);
        let (w, h) = self.solid_bg_size();
        let img = image::RgbImage::from_pixel(w, h, Rgb(color));
        self.apply_project_bg_image(img, None, cx);
    }

    fn solid_bg_size(&self) -> (u32, u32) {
        let aw = self.doc.bg_aspect_w.max(1);
        let ah = self.doc.bg_aspect_h.max(1);
        let max_w = self
            .doc
            .pages
            .iter()
            .map(|p| p.width())
            .max()
            .unwrap_or(aw)
            .max(aw);
        let w = max_w.saturating_mul(2).max(2560).min(8192);
        let h = ((w as u64 * ah as u64) / aw as u64).max(1).min(8192) as u32;
        (w, h)
    }

    fn apply_project_bg_image(
        &mut self,
        rgb: image::RgbImage,
        source: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.doc.groups.is_empty() {
            self.show_error(
                "提示",
                crate::error::Error::msg("当前没有输出组合. 请先分块/合并后再应用底色层."),
                cx,
            );
            return;
        }
        let aw = self.doc.bg_aspect_w.max(1);
        let ah = self.doc.bg_aspect_h.max(1);
        match self.doc.set_project_bg(rgb, source.clone(), aw, ah) {
            Ok(()) => {
                if let Some(gid) = self.doc.groups.first().map(|g| g.id.clone()) {
                    let _ = self.doc.ensure_group_pages(&gid);
                    if let Err(e) = self.doc.render_group_final(&gid) {
                        self.doc.clear_project_bg();
                        self.doc.retain_memory_window();
                        self.show_error(
                            "底色不适用",
                            crate::error::Error::msg(format!(
                                "{e}\n已取消启用. 请换更大底色 (总谱按高度定画布时左右也要盖住) 或检查谱面尺寸."
                            )),
                            cx,
                        );
                        return;
                    }
                    self.doc.retain_memory_window();
                }
                self.bg.applied_is_solid = source.is_none();
                self.mark_dirty();
                self.mark_video_pool_dirty_all();
                let name = source
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("纯色");
                self.status = format!(
                    "已为 {} 个组合启用底色层 {} ({}:{})",
                    self.doc.groups.len(),
                    name,
                    aw,
                    ah
                )
                .into();
                self.hint = self.status.clone();
                if let Some(img) = self.bg.cached_image.as_ref() {
                    let thumb = thumbnail_rgb(img, BG_THUMB_MAX);
                    self.bg.pending_preview = Some(rgb_to_render_image(&thumb));
                } else if let Some(img) = self.doc.bg_image.as_ref() {
                    let thumb = thumbnail_rgb(img, BG_THUMB_MAX);
                    self.bg.pending_preview = Some(rgb_to_render_image(&thumb));
                }
                self.force_refresh_mask_preview(cx);
                self.sync_video_pool(cx);
                cx.notify();
            }
            Err(e) => {
                self.show_error("无法应用底色", crate::error::Error::msg(e), cx);
            }
        }
    }

    fn set_bg_palette_sb(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.bg.sb_bounds.origin.x);
        let top = f32::from(self.bg.sb_bounds.origin.y);
        let w = f32::from(self.bg.sb_bounds.size.width).max(1.0);
        let h = f32::from(self.bg.sb_bounds.size.height).max(1.0);
        self.bg.picker_s = ((x - left) / w).clamp(0.0, 1.0);
        self.bg.picker_v = (1.0 - (y - top) / h).clamp(0.0, 1.0);
        self.bg.color = self.bg.picker_rgb();
        self.sync_bg_rgb_inputs(cx);
        cx.notify();
    }

    fn set_bg_palette_hue(&mut self, y: f32, cx: &mut Context<Self>) {
        let top = f32::from(self.bg.hue_bounds.origin.y);
        let h = f32::from(self.bg.hue_bounds.size.height).max(1.0);
        self.bg.picker_h = ((y - top) / h).clamp(0.0, 1.0) * 360.0;
        self.bg.rebuild_sb_image();
        self.bg.color = self.bg.picker_rgb();
        self.sync_bg_rgb_inputs(cx);
        cx.notify();
    }

    pub(super) fn apply_bg_palette_drag(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(DragKind::BgPaletteSb) => self.set_bg_palette_sb(x, y, cx),
            Some(DragKind::BgPaletteHue) => self.set_bg_palette_hue(y, cx),
            _ => {}
        }
    }

    fn capture_bg_snap(&self) -> BgSnap {
        BgSnap {
            enabled: self.doc.bg_enabled,
            is_solid: self.bg.applied_is_solid && self.doc.bg_enabled,
            source_path: self
                .bg
                .cached_source_path
                .clone()
                .or_else(|| self.doc.bg_source_path.clone()),
            session_path: self.bg.cached_session_path.clone(),
            aspect_w: self.doc.bg_aspect_w,
            aspect_h: self.doc.bg_aspect_h,
            color: self.bg.color,
        }
    }

    fn push_bg_undo(&mut self) {
        self.ensure_bg_session_cache();
        let snap = self.capture_bg_snap();
        self.bg_history.undo.push(snap);
        if self.bg_history.undo.len() > CROP_HISTORY_LIMIT {
            self.bg_history.undo.remove(0);
        }
        self.bg_history.redo.clear();
    }

    fn restore_bg_snap(&mut self, snap: BgSnap, cx: &mut Context<Self>) {
        self.bg.aspect_syncing = true;
        self.bg.cached_source_path = snap.source_path.clone();
        self.bg.cached_session_path = snap.session_path.clone();
        self.bg.pending_path = snap.source_path.clone();
        self.doc.bg_aspect_w = snap.aspect_w.max(1);
        self.doc.bg_aspect_h = snap.aspect_h.max(1);
        self.set_bg_picker_rgb(snap.color, true, cx);
        self.sync_bg_aspect_inputs(cx);
        if let Some(p) = snap.session_path.as_ref() {
            if let Ok(img) = crate::page_cache::load_rgb(p) {
                self.bg.cached_image = Some(img.clone());
                let thumb = thumbnail_rgb(&img, BG_THUMB_MAX);
                self.bg.pending_preview = Some(rgb_to_render_image(&thumb));
            }
        }
        if snap.enabled {
            if snap.is_solid {
                self.apply_solid_inner(cx);
            } else {
                self.apply_image_inner(cx);
            }
        } else {
            self.bg.applied_is_solid = false;
            if self.doc.bg_enabled || self.doc.bg_image.is_some() {
                self.doc.clear_project_bg();
                self.mark_dirty();
                self.mark_video_pool_dirty_all();
                self.force_refresh_mask_preview(cx);
                self.sync_video_pool(cx);
            }
        }
        self.bg.aspect_syncing = false;
        cx.notify();
    }

    pub(super) fn undo_bg(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.bg_history.undo.pop() else {
            self.status = "没有可撤回的底色操作.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        };
        self.ensure_bg_session_cache();
        let now = self.capture_bg_snap();
        self.bg_history.redo.push(now);
        self.restore_bg_snap(prev, cx);
        self.status = "已撤回底色操作.".into();
        self.hint = self.status.clone();
    }

    pub(super) fn redo_bg(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.bg_history.redo.pop() else {
            self.status = "没有可重做的底色操作.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        };
        self.ensure_bg_session_cache();
        let now = self.capture_bg_snap();
        self.bg_history.undo.push(now);
        if self.bg_history.undo.len() > CROP_HISTORY_LIMIT {
            self.bg_history.undo.remove(0);
        }
        self.restore_bg_snap(next, cx);
        self.status = "已重做底色操作.".into();
        self.hint = self.status.clone();
    }

    pub(super) fn bg_side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.doc.bg_enabled;
        let image_on = enabled && !self.bg.applied_is_solid;
        let solid_on = enabled && self.bg.applied_is_solid;
        let can_image = image_on
            || self.bg.cached_image.is_some()
            || self.bg.pending_path.is_some()
            || self
                .bg
                .cached_session_path
                .as_ref()
                .map(|p| p.is_file())
                .unwrap_or(false);
        let image_label: SharedString = if image_on {
            "取消底色".into()
        } else {
            "应用底色".into()
        };
        let solid_label: SharedString = if solid_on {
            "取消纯色底色".into()
        } else {
            "使用纯色底色".into()
        };
        let aw = self.bg.aspect_w.clone();
        let ah = self.bg.aspect_h.clone();

        div()
            .id("bg_side_panel")
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(rgb(0xf8fafc))
            .child(
                div()
                    .id("bg_side_scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_1()
                                    .w_full()
                                    .child(self.bg_flex_btn(
                                        "bg_pick_open",
                                        "选择底色".into(),
                                        false,
                                        true,
                                        |this, _, cx| this.open_bg_pick(cx),
                                        cx,
                                    ))
                                    .child(self.bg_flex_btn(
                                        "bg_toggle",
                                        image_label,
                                        image_on,
                                        can_image,
                                        |this, _, cx| this.toggle_project_bg(cx),
                                        cx,
                                    ))
                                    .child(self.bg_flex_btn(
                                        "bg_batch_open",
                                        "批量加底色".into(),
                                        false,
                                        true,
                                        |this, _, cx| {
                                            this.bg.batch_open = true;
                                            cx.notify();
                                        },
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .w_full()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x334155))
                                            .flex_shrink_0()
                                            .child("目标分辨率"),
                                    )
                                    .child(
                                        div()
                                            .id("bg_aspect_w")
                                            .flex_1()
                                            .min_w(px(0.))
                                            .h(px(22.))
                                            .child(aw),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x64748b))
                                            .flex_shrink_0()
                                            .child(":"),
                                    )
                                    .child(
                                        div()
                                            .id("bg_aspect_h")
                                            .flex_1()
                                            .min_w(px(0.))
                                            .h(px(22.))
                                            .child(ah),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .h(px(1.))
                            .w_full()
                            .bg(rgb(0xcbd5e1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .child(
                                div()
                                    .w_full()
                                    .flex_shrink_0()
                                    .flex()
                                    .child(self.bg_flex_btn(
                                        "bg_toggle_solid",
                                        solid_label,
                                        solid_on,
                                        true,
                                        |this, _, cx| this.toggle_solid_project_bg(cx),
                                        cx,
                                    )),
                            )
                            .child(self.bg_color_picker(cx)),
                    ),
            )
    }

    fn bg_flex_btn(
        &self,
        id: &'static str,
        label: SharedString,
        active: bool,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if !enabled {
            rgb(0xf1f5f9)
        } else if active {
            rgb(0x2563eb)
        } else {
            rgb(0xe2e8f0)
        };
        let fg = if !enabled {
            rgb(0x94a3b8)
        } else if active {
            rgb(0xffffff)
        } else {
            rgb(0x0f172a)
        };
        div()
            .id(id)
            .flex_1()
            .min_w(px(0.))
            .px_1()
            .py_1()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(rgb(0x94a3b8))
            .text_color(fg)
            .text_xs()
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |d| {
                let id_down = id;
                let id_up = id;
                let id_out = id;
                d.cursor_pointer()
                    .hover(move |s| {
                        s.bg(if active {
                            rgb(0x1d4ed8)
                        } else {
                            rgb(0xcbd5e1)
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.btn_press = Some(id_down.into());
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            let press = this.btn_press.take();
                            if press != Some(SharedString::from(id_up)) {
                                return;
                            }
                            on_click(this, window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, _| {
                            if this.btn_press.as_ref() == Some(&SharedString::from(id_out)) {
                                this.btn_press = None;
                            }
                        }),
                    )
            })
            .child(label)
    }

    fn bg_palette_side(&self) -> f32 {
        // 面板 p_3 + 色盘 p_2 + 两处 gap_2 + 常用色条 + 色相条 + 边框
        let chrome = 24.0 + 16.0 + 16.0 + BG_RECENT_W + BG_HUE_W + 6.0;
        (self.side_width - chrome).max(112.0)
    }

    fn bg_recent_min_h() -> f32 {
        BG_RECENT_MIN as f32 * BG_RECENT_SWATCH
            + (BG_RECENT_MIN.saturating_sub(1) as f32) * BG_RECENT_GAP
    }

    fn bg_recent_slots(side: f32) -> usize {
        let min_h = Self::bg_recent_min_h();
        if side + 0.5 < min_h {
            BG_RECENT_MIN
        } else {
            let n = ((side + BG_RECENT_GAP) / (BG_RECENT_SWATCH + BG_RECENT_GAP)).floor() as usize;
            n.clamp(BG_RECENT_MIN, BG_RECENT_MAX)
        }
    }

    fn bg_recent_swatch(
        &self,
        i: usize,
        color: Option<[u8; 3]>,
        grow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut el = div()
            .id(SharedString::from(format!("bg-recent-{i}")))
            .w_full()
            .rounded_sm()
            .border_1()
            .border_color(rgb(0x64748b));
        el = if grow {
            el.flex_1().min_h(px(0.))
        } else {
            el.h(px(BG_RECENT_SWATCH)).flex_shrink_0()
        };
        if let Some(color) = color {
            let color_u = color_u32(color);
            el.bg(rgb(color_u))
                .cursor_pointer()
                .hover(|s| s.border_color(rgb(0x94a3b8)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_bg_picker_rgb(color, true, cx);
                        this.push_bg_recent(color);
                        cx.notify();
                    }),
                )
        } else {
            el.bg(rgb(0x0f172a))
        }
    }

    fn bg_color_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sb_img = self.bg.sb_image.clone();
        let hue_img = self.bg.hue_image.clone();
        let picker_s = self.bg.picker_s;
        let picker_v = self.bg.picker_v;
        let picker_h = self.bg.picker_h;
        let recent = self.bg.recent.clone();
        let drop_on = self.bg.eyedropper_armed;
        let r_in = self.bg.rgb_r.clone();
        let g_in = self.bg.rgb_g.clone();
        let b_in = self.bg.rgb_b.clone();
        let side = self.bg_palette_side();

        div()
            .id("bg_color_picker")
            .w_full()
            .flex_shrink_0()
            .p_2()
            .rounded_md()
            .bg(rgb(0x1e293b))
            .border_1()
            .border_color(rgb(0x334155))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .w_full()
                    .h(px(side))
                    .flex_shrink_0()
                    .child({
                        let slots = Self::bg_recent_slots(side);
                        let compressed = side + 0.5 < Self::bg_recent_min_h();
                        let mut cells: Vec<Option<[u8; 3]>> =
                            recent.into_iter().map(Some).collect();
                        while cells.len() < slots {
                            cells.push(None);
                        }
                        cells.truncate(slots);
                        let mut col = div()
                            .w(px(BG_RECENT_W))
                            .h(px(side))
                            .flex_shrink_0()
                            .flex()
                            .flex_col();
                        col = if compressed {
                            col.gap_1()
                        } else {
                            col.justify_between()
                        };
                        col.children(cells.into_iter().enumerate().map(|(i, color)| {
                            self.bg_recent_swatch(i, color, compressed, cx)
                        }))
                    })
                    .child(
                        div()
                            .id("bg_palette_sb")
                            .relative()
                            .size(px(side))
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
                                                this.bg.sb_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = sb_img {
                                            let _ = window.paint_image(
                                                Bounds {
                                                    origin: bounds.origin,
                                                    size: bounds.size,
                                                },
                                                gpui::Corners::default(),
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
                                    this.drag = Some(DragKind::BgPaletteSb);
                                    this.set_bg_palette_sb(
                                        f32::from(ev.position.x),
                                        f32::from(ev.position.y),
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                if matches!(this.drag, Some(DragKind::BgPaletteSb)) {
                                    this.set_bg_palette_sb(
                                        f32::from(ev.position.x),
                                        f32::from(ev.position.y),
                                        cx,
                                    );
                                }
                            }))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::BgPaletteSb)) {
                                        this.drag = None;
                                        this.push_bg_recent(this.bg.color);
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("bg_palette_hue")
                            .relative()
                            .w(px(BG_HUE_W))
                            .h(px(side))
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
                                                this.bg.hue_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = hue_img {
                                            let _ = window.paint_image(
                                                Bounds {
                                                    origin: bounds.origin,
                                                    size: bounds.size,
                                                },
                                                gpui::Corners::default(),
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
                                    this.drag = Some(DragKind::BgPaletteHue);
                                    this.set_bg_palette_hue(f32::from(ev.position.y), cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                if matches!(this.drag, Some(DragKind::BgPaletteHue)) {
                                    this.set_bg_palette_hue(f32::from(ev.position.y), cx);
                                }
                            }))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::BgPaletteHue)) {
                                        this.drag = None;
                                        this.push_bg_recent(this.bg.color);
                                        cx.notify();
                                    }
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
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xcbd5e1))
                            .flex_shrink_0()
                            .child("RGB"),
                    )
                    .child(
                        div()
                            .id("bg_eyedropper")
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
                            .text_xs()
                            .text_color(rgb(0xf8fafc))
                            .child("取")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.arm_bg_eyedropper(cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child("R"),
                    )
                    .child(div().id("bg_rgb_r").flex_1().min_w(px(0.)).h(px(20.)).child(r_in))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child("G"),
                    )
                    .child(div().id("bg_rgb_g").flex_1().min_w(px(0.)).h(px(20.)).child(g_in))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child("B"),
                    )
                    .child(div().id("bg_rgb_b").flex_1().min_w(px(0.)).h(px(20.)).child(b_in)),
            )
    }

    pub(super) fn bg_pick_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.bg.pick_open {
            return div().into_any_element();
        }
        let preview = self.bg.pending_preview.clone();
        let has_img = preview.is_some();
        let hint: SharedString = if has_img {
            "点击或拖入以替换".into()
        } else {
            "请拖入一张图片, 或点击打开文件".into()
        };
        let paint = preview.clone();
        let preview_canvas = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let Some(img) = &paint else {
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
                let _ = window.paint_image(
                    Bounds {
                        origin: point(ox, oy),
                        size: size(px(dw), px(dh)),
                    },
                    gpui::Corners::default(),
                    img.clone(),
                    0,
                    false,
                );
            },
        );

        div()
            .id("bg_pick_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_bg_pick(cx);
                    cx.stop_propagation();
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths.paths().to_vec();
                this.apply_drop_as_bg(&list, cx);
            }))
            .child(
                div()
                    .id("bg_pick_card")
                    .w(px(420.))
                    .h(px(360.))
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
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("选择底色"),
                    )
                    .child(
                        div()
                            .id("bg_pick_drop")
                            .w_full()
                            .flex_1()
                            .min_h(px(0.))
                            .rounded_lg()
                            .border_2()
                            .border_dashed()
                            .border_color(rgb(0x94a3b8))
                            .bg(rgb(0xf8fafc))
                            .relative()
                            .overflow_hidden()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xf1f5f9)).border_color(rgb(0x64748b)))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.pick_bg_file_dialog(cx)),
                            )
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                let list: Vec<PathBuf> = paths.paths().to_vec();
                                this.apply_drop_as_bg(&list, cx);
                            }))
                            .child(preview_canvas.absolute().inset_0().size_full())
                            .when(!has_img, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_3xl()
                                                .text_color(rgb(0x64748b))
                                                .child("🖼"),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0x475569))
                                                .child(hint.clone()),
                                        ),
                                )
                            })
                            .when(has_img, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .left_0()
                                        .right_0()
                                        .py_1()
                                        .bg(gpui::rgba(0x0f172acc))
                                        .flex()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0xf8fafc))
                                                .child(hint),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(self.btn(
                                "bg_pick_cancel",
                                "取消",
                                false,
                                |this, _, cx| this.close_bg_pick(cx),
                                cx,
                            ))
                            .child(self.btn(
                                "bg_pick_ok",
                                "确定",
                                has_img,
                                |this, _, cx| this.confirm_bg_pick(cx),
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn apply_bg_batch_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !self.bg.batch_open {
            return div().into_any_element();
        }
        let panel = self
            .apply_bg
            .update(cx, |m, cx| m.panel(cx).into_any_element());
        div()
            .id("bg_batch_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.bg.batch_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("bg_batch_card")
                    .w(px(640.))
                    .max_w(px(720.))
                    .max_h(px(520.))
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .shadow_lg()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(0xcbd5e1))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("谱面加底色 (批量)"),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("bg_batch_close")
                                    .px_2()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(0xe2e8f0)))
                                    .child("×")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.btn_press = Some("bg_batch_close".into());
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            let press = this.btn_press.take();
                                            if press != Some("bg_batch_close".into()) {
                                                return;
                                            }
                                            this.bg.batch_open = false;
                                            cx.notify();
                                        }),
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, _| {
                                            if this.btn_press.as_ref()
                                                == Some(&SharedString::from("bg_batch_close"))
                                            {
                                                this.btn_press = None;
                                            }
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("bg_batch_scroll")
                            .flex_1()
                            .min_h(px(0.))
                            .w_full()
                            .overflow_y_scroll()
                            .child(div().w_full().child(panel)),
                    ),
            )
            .into_any_element()
    }
}
