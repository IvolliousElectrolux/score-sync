//! 缩放图层.

use image::{imageops, RgbaImage};

pub use crate::geom::{rotate_rgba, rotated_aabb, trim_rgba};

pub fn scale_rgba(img: &RgbaImage, scale: f32) -> RgbaImage {
    let scale = scale.clamp(0.05, 8.0);
    let w = ((img.width() as f32 * scale).round() as u32).max(1);
    let h = ((img.height() as f32 * scale).round() as u32).max(1);
    resize_rgba(img, w, h)
}

pub fn resize_rgba(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    imageops::resize(img, w.max(1), h.max(1), imageops::FilterType::Triangle)
}

/// 拖动预览用的缩小图; 已经够小则原样 clone.
pub fn preview_rgba(img: &RgbaImage, max_side: u32) -> RgbaImage {
    let w = img.width();
    let h = img.height();
    let m = w.max(h).max(1);
    if m <= max_side.max(1) {
        return img.clone();
    }
    let s = max_side as f32 / m as f32;
    resize_rgba(
        img,
        (w as f32 * s).round().max(1.0) as u32,
        (h as f32 * s).round().max(1.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn scale_changes_size() {
        let img = RgbaImage::from_pixel(10, 8, Rgba([1, 2, 3, 255]));
        let out = scale_rgba(&img, 0.5);
        assert_eq!(out.dimensions(), (5, 4));
    }

    #[test]
    fn preview_caps_long_side() {
        let img = RgbaImage::from_pixel(800, 200, Rgba([1, 2, 3, 255]));
        let out = preview_rgba(&img, 400);
        assert_eq!(out.dimensions(), (400, 100));
    }
}
