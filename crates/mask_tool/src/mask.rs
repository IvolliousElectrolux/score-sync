//! 蒙版矩形与导出合成.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::{ImageBuffer, ImageFormat, Rgb};

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"];

pub fn is_image_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
            .unwrap_or(false)
}

pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:08x}", (nanos as u32).wrapping_mul(2654435761))
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MaskRect {
    pub id: String,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl MaskRect {
    pub fn normalized(&self) -> MaskRect {
        MaskRect {
            id: self.id.clone(),
            x0: self.x0.min(self.x1),
            y0: self.y0.min(self.y1),
            x1: self.x0.max(self.x1),
            y1: self.y0.max(self.y1),
        }
    }

    pub fn label(&self) -> String {
        let r = self.normalized();
        format!(
            "({},{})–({},{})  {}×{}",
            r.x0,
            r.y0,
            r.x1,
            r.y1,
            r.x1 - r.x0 + 1,
            r.y1 - r.y0 + 1
        )
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let r = self.normalized();
        x >= r.x0 as f32
            && x <= (r.x1 as f32) + 1.0
            && y >= r.y0 as f32
            && y <= (r.y1 as f32) + 1.0
    }

    /// 与图像坐标轴对齐矩形是否相交 (含边界).
    pub fn intersects_rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let r = self.normalized();
        let ax0 = x0.min(x1);
        let ax1 = x0.max(x1);
        let ay0 = y0.min(y1);
        let ay1 = y0.max(y1);
        !(r.x1 as f32 + 1.0 <= ax0
            || r.x0 as f32 >= ax1
            || r.y1 as f32 + 1.0 <= ay0
            || r.y0 as f32 >= ay1)
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        self.x0 += dx;
        self.x1 += dx;
        self.y0 += dy;
        self.y1 += dy;
    }
}

/// 在 RGB 图上叠半透明白蒙版 (原地修改).
pub fn apply_masks_rgb(
    rgb: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    masks: &[MaskRect],
    opacity: f32,
) {
    let (w, h) = rgb.dimensions();
    let a = opacity.clamp(0.05, 1.0);
    let inv = 1.0 - a;
    for m in masks {
        let r = m.normalized();
        let x0 = r.x0.max(0) as u32;
        let y0 = r.y0.max(0) as u32;
        let x1 = (r.x1.max(0) as u32).min(w.saturating_sub(1));
        let y1 = (r.y1.max(0) as u32).min(h.saturating_sub(1));
        if x0 > x1 || y0 > y1 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = rgb.get_pixel_mut(x, y);
                p[0] = (p[0] as f32 * inv + 255.0 * a).round() as u8;
                p[1] = (p[1] as f32 * inv + 255.0 * a).round() as u8;
                p[2] = (p[2] as f32 * inv + 255.0 * a).round() as u8;
            }
        }
    }
}

/// 在 RGB 图上叠半透明白蒙版并保存 (等价于 alpha_composite 白层).
pub fn export_masked(
    base: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    masks: &[MaskRect],
    opacity: f32,
    path: &Path,
) -> Result<(), String> {
    let mut rgb = base.clone();
    apply_masks_rgb(&mut rgb, masks, opacity);
    let format = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
        _ => ImageFormat::Png,
    };
    rgb.save_with_format(path, format)
        .map_err(|e| format!("保存失败: {e}"))
}
pub fn default_export_path(image_path: Option<&Path>) -> PathBuf {
    match image_path {
        Some(p) => p.with_file_name(format!(
            "{}_masked.png",
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("masked")
        )),
        None => PathBuf::from("masked.png"),
    }
}

pub fn first_image_in_paths(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| is_image_path(p)).cloned()
}
