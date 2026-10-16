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

/// 图层面板用的小图. 盒子平均, 长边不超过 `max_side`.
pub fn box_thumb(img: &RgbaImage, max_side: u32) -> RgbaImage {
    let w = img.width();
    let h = img.height();
    let max_side = max_side.max(1);
    let m = w.max(h).max(1);
    if w == 0 || h == 0 || m <= max_side {
        return img.clone();
    }
    let tw = ((w as u64 * max_side as u64) / m as u64).max(1) as u32;
    let th = ((h as u64 * max_side as u64) / m as u64).max(1) as u32;
    let src = img.as_raw();
    let sw = w as usize;
    let sh = h as usize;
    let twu = tw as usize;
    let thu = th as usize;
    let mut out = vec![0u8; twu * thu * 4];
    for ty in 0..thu {
        let y0 = ty * sh / thu;
        let y1 = ((ty + 1) * sh / thu).max(y0 + 1).min(sh);
        for tx in 0..twu {
            let x0 = tx * sw / twu;
            let x1 = ((tx + 1) * sw / twu).max(x0 + 1).min(sw);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for y in y0..y1 {
                let row = &src[(y * sw + x0) * 4..(y * sw + x1) * 4];
                for px in row.chunks_exact(4) {
                    acc[0] += px[0] as u32;
                    acc[1] += px[1] as u32;
                    acc[2] += px[2] as u32;
                    acc[3] += px[3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = (ty * twu + tx) * 4;
            out[o] = (acc[0] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[2] / n) as u8;
            out[o + 3] = (acc[3] / n) as u8;
        }
    }
    RgbaImage::from_raw(tw, th, out).expect("thumb buffer")
}

/// 底图像素叠上笔迹, 再按图层角度转正, 缩到长边 `max_side`.
pub fn layer_thumb_rgba(
    pixels: &RgbaImage,
    ink: Option<&RgbaImage>,
    rotation_deg: f32,
    max_side: u32,
) -> RgbaImage {
    let mut base = box_thumb(pixels, max_side);
    if let Some(ink) = ink {
        let over = box_thumb(ink, max_side);
        if over.dimensions() == base.dimensions() {
            blit_thumb(&mut base, &over);
        }
    }
    if rotation_deg.abs() < 0.5 {
        return base;
    }
    box_thumb(&crate::geom::rotate_rgba(&base, rotation_deg), max_side)
}

fn blit_thumb(base: &mut RgbaImage, ink: &RgbaImage) {
    let w = base.width().min(ink.width());
    let h = base.height().min(ink.height());
    for y in 0..h {
        for x in 0..w {
            let s = *ink.get_pixel(x, y);
            if s[3] == 0 {
                continue;
            }
            let d = base.get_pixel_mut(x, y);
            let sa = s[3] as f32 / 255.0;
            if sa >= 0.999 {
                *d = s;
                continue;
            }
            let inv = 1.0 - sa;
            let da = d[3] as f32 / 255.0;
            let out_a = sa + da * inv;
            if out_a <= 1e-6 {
                *d = image::Rgba([0, 0, 0, 0]);
                continue;
            }
            *d = image::Rgba([
                ((s[0] as f32 * sa + d[0] as f32 * da * inv) / out_a).round() as u8,
                ((s[1] as f32 * sa + d[1] as f32 * da * inv) / out_a).round() as u8,
                ((s[2] as f32 * sa + d[2] as f32 * da * inv) / out_a).round() as u8,
                (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
            ]);
        }
    }
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

    #[test]
    fn box_thumb_keeps_solid_color_and_caps_side() {
        let img = RgbaImage::from_pixel(80, 40, Rgba([20, 40, 60, 255]));
        let out = box_thumb(&img, 20);
        assert_eq!(out.dimensions(), (20, 10));
        assert_eq!(out.get_pixel(3, 3).0, [20, 40, 60, 255]);
    }

    #[test]
    fn layer_thumb_paints_opaque_ink_over_paper() {
        let paper = RgbaImage::from_pixel(40, 20, Rgba([240, 240, 240, 255]));
        let mut ink = RgbaImage::new(40, 20);
        for x in 0..20 {
            for y in 0..20 {
                ink.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let out = layer_thumb_rgba(&paper, Some(&ink), 0.0, 20);
        assert_eq!(out.dimensions(), (20, 10));
        assert_eq!(out.get_pixel(2, 2).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(15, 2).0, [240, 240, 240, 255]);
    }
}
