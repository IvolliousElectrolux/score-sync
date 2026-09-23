//! 图章 / 污点共用的笔尖: 余弦硬度, 边缘 AA, 双线性取样.

use std::sync::{Arc, Mutex, OnceLock};

use image::{Rgba, RgbaImage};

pub fn brush_weight(dx: i32, dy: i32, radius: i32, hardness: f32) -> f32 {
    let r = radius.max(1) as f32;
    let d = ((dx * dx + dy * dy) as f32).sqrt();
    let cover = (r + 0.5 - d).clamp(0.0, 1.0);
    if cover <= 0.0 {
        return 0.0;
    }
    let hard = hardness.clamp(0.0, 1.0);
    let inner = hard * r;
    let profile = if inner >= r - 1e-3 {
        1.0
    } else if d <= inner {
        1.0
    } else {
        let span = (r - inner).max(1e-6);
        let u = ((d - inner) / span).clamp(0.0, 1.0);
        0.5 * (1.0 + (std::f32::consts::PI * u).cos())
    };
    profile * cover
}

pub fn sample_bilinear(img: &RgbaImage, x: f32, y: f32) -> Option<Rgba<u8>> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    if w < 1 || h < 1 {
        return None;
    }
    if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
        return None;
    }
    let x0 = (x.floor() as i32).clamp(0, w - 1);
    let y0 = (y.floor() as i32).clamp(0, h - 1);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = (x - x0 as f32).clamp(0.0, 1.0);
    let fy = (y - y0 as f32).clamp(0.0, 1.0);
    let p00 = img.get_pixel(x0 as u32, y0 as u32).0.map(|v| v as f32);
    let p10 = img.get_pixel(x1 as u32, y0 as u32).0.map(|v| v as f32);
    let p01 = img.get_pixel(x0 as u32, y1 as u32).0.map(|v| v as f32);
    let p11 = img.get_pixel(x1 as u32, y1 as u32).0.map(|v| v as f32);
    let mut out = [0u8; 4];
    for i in 0..4 {
        let a = p00[i] + (p10[i] - p00[i]) * fx;
        let b = p01[i] + (p11[i] - p01[i]) * fx;
        out[i] = (a + (b - a) * fy).round().clamp(0.0, 255.0) as u8;
    }
    Some(Rgba(out))
}

pub fn blend(dst: Rgba<u8>, src: Rgba<u8>, a: f32) -> Rgba<u8> {
    if a <= 0.001 || src[3] == 0 {
        return dst;
    }
    let sa = (src[3] as f32 / 255.0) * a.clamp(0.0, 1.0);
    if sa >= 0.999 {
        return src;
    }
    let inv = 1.0 - sa;
    Rgba([
        (dst[0] as f32 * inv + src[0] as f32 * sa).round() as u8,
        (dst[1] as f32 * inv + src[1] as f32 * sa).round() as u8,
        (dst[2] as f32 * inv + src[2] as f32 * sa).round() as u8,
        (dst[3] as f32 * inv + 255.0 * sa).round().min(255.0) as u8,
    ])
}

fn shift_rgb(src: Rgba<u8>, rgb_shift: [f32; 3]) -> Rgba<u8> {
    Rgba([
        (src[0] as f32 + rgb_shift[0]).round().clamp(0.0, 255.0) as u8,
        (src[1] as f32 + rgb_shift[1]).round().clamp(0.0, 255.0) as u8,
        (src[2] as f32 + rgb_shift[2]).round().clamp(0.0, 255.0) as u8,
        src[3],
    ])
}

/// 从源图按相对偏移盖到目标层. `(src_x, src_y)` 对应当前笔尖在源图上的位置.
pub fn stamp_offset(
    dst: &mut RgbaImage,
    src: &RgbaImage,
    dst_x: i32,
    dst_y: i32,
    src_x: i32,
    src_y: i32,
    radius: i32,
    hardness: f32,
    rgb_shift: [f32; 3],
) {
    let radius = radius.max(1);
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    let has_shift = rgb_shift.iter().any(|v| v.abs() > 0.05);
    for y in (dst_y - radius - 1).max(0)..(dst_y + radius + 2).min(dh) {
        for x in (dst_x - radius - 1).max(0)..(dst_x + radius + 2).min(dw) {
            let w = brush_weight(x - dst_x, y - dst_y, radius, hardness);
            if w <= 0.0 {
                continue;
            }
            let sx = src_x as f32 + (x - dst_x) as f32;
            let sy = src_y as f32 + (y - dst_y) as f32;
            let Some(mut s) = sample_bilinear(src, sx, sy) else {
                continue;
            };
            if has_shift {
                s = shift_rgb(s, rgb_shift);
            }
            let d = *dst.get_pixel(x as u32, y as u32);
            dst.put_pixel(x as u32, y as u32, blend(d, s, w));
        }
    }
}

/// 按笔尖硬度把透明度乘掉. 笔芯为 1 时该像素变透明, 软边按权重残留.
pub fn erase_stamp(
    dst: &mut RgbaImage,
    dst_x: i32,
    dst_y: i32,
    radius: i32,
    hardness: f32,
) -> bool {
    let radius = radius.max(1);
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    let mut hit = false;
    for y in (dst_y - radius - 1).max(0)..(dst_y + radius + 2).min(dh) {
        for x in (dst_x - radius - 1).max(0)..(dst_x + radius + 2).min(dw) {
            let w = brush_weight(x - dst_x, y - dst_y, radius, hardness);
            if w <= 0.0 {
                continue;
            }
            let d = *dst.get_pixel(x as u32, y as u32);
            if d[3] == 0 {
                continue;
            }
            let na = (d[3] as f32 * (1.0 - w)).round().clamp(0.0, 255.0) as u8;
            if na == d[3] {
                continue;
            }
            dst.put_pixel(x as u32, y as u32, Rgba([d[0], d[1], d[2], na]));
            hit = true;
        }
    }
    hit
}

struct BrushKernel {
    radius: i32,
    hardness_bits: u32,
    side: i32,
    weights: Vec<f32>,
}

impl BrushKernel {
    fn build(radius: i32, hardness: f32) -> Self {
        let radius = radius.max(1);
        let side = 2 * radius + 3;
        let mut weights = vec![0.0; (side * side) as usize];
        for dy in -(radius + 1)..=(radius + 1) {
            for dx in -(radius + 1)..=(radius + 1) {
                let w = brush_weight(dx, dy, radius, hardness);
                let ix = (dy + radius + 1) * side + (dx + radius + 1);
                weights[ix as usize] = w;
            }
        }
        Self {
            radius,
            hardness_bits: hardness.to_bits(),
            side,
            weights,
        }
    }

    fn weight(&self, dx: i32, dy: i32) -> f32 {
        let o = self.radius + 1;
        self.weights[(dy + o) as usize * self.side as usize + (dx + o) as usize]
    }
}

fn kernel_slot() -> &'static Mutex<Option<Arc<BrushKernel>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<BrushKernel>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn brush_kernel(radius: i32, hardness: f32) -> Arc<BrushKernel> {
    let mut slot = kernel_slot().lock().unwrap_or_else(|e| e.into_inner());
    let bits = hardness.to_bits();
    if let Some(k) = slot.as_ref() {
        if k.radius == radius.max(1) && k.hardness_bits == bits {
            return Arc::clone(k);
        }
    }
    let k = Arc::new(BrushKernel::build(radius, hardness));
    *slot = Some(Arc::clone(&k));
    k
}

/// 在当前层盖实心色章 (硬度 + 边缘 AA).
/// 笔尖权重按半径和硬度缓存, 已经盖实的像素直接跳过, 快速拖动时重叠部分不再重算.
pub fn stamp_color(
    dst: &mut RgbaImage,
    dst_x: i32,
    dst_y: i32,
    radius: i32,
    hardness: f32,
    rgb: [u8; 3],
) {
    let radius = radius.max(1);
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    if dw < 1 || dh < 1 {
        return;
    }
    let y0 = (dst_y - radius - 1).max(0);
    let y1 = (dst_y + radius + 2).min(dh);
    let x0 = (dst_x - radius - 1).max(0);
    let x1 = (dst_x + radius + 2).min(dw);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let kernel = brush_kernel(radius, hardness);
    let width = dw as usize;
    let raw: &mut [u8] = dst.as_mut();
    for y in y0..y1 {
        let dy = y - dst_y;
        let row = &mut raw[(y as usize) * width * 4..(y as usize + 1) * width * 4];
        for x in x0..x1 {
            let a = kernel.weight(x - dst_x, dy);
            if a <= 0.001 {
                continue;
            }
            let i = (x as usize) * 4;
            let px = &mut row[i..i + 4];
            if a >= 0.999 {
                if px[0] == rgb[0] && px[1] == rgb[1] && px[2] == rgb[2] && px[3] == 255 {
                    continue;
                }
                px[0] = rgb[0];
                px[1] = rgb[1];
                px[2] = rgb[2];
                px[3] = 255;
                continue;
            }
            let inv = 1.0 - a;
            px[0] = (px[0] as f32 * inv + rgb[0] as f32 * a).round() as u8;
            px[1] = (px[1] as f32 * inv + rgb[1] as f32 * a).round() as u8;
            px[2] = (px[2] as f32 * inv + rgb[2] as f32 * a).round() as u8;
            px[3] = (px[3] as f32 * inv + 255.0 * a).round().min(255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardness_core_is_solid() {
        let w = brush_weight(0, 0, 8, 0.5);
        assert!(w > 0.99, "center {w}");
    }

    #[test]
    fn soft_edge_is_partial() {
        let w = brush_weight(7, 0, 8, 0.0);
        assert!(w > 0.0 && w < 1.0, "soft edge {w}");
    }

    #[test]
    fn hard_brush_still_has_aa() {
        let inner = brush_weight(5, 0, 6, 1.0);
        let rim = brush_weight(6, 0, 6, 1.0);
        assert!(inner > 0.99, "inner {inner}");
        assert!(rim > 0.0 && rim < 1.0, "AA rim {rim}");
    }

    #[test]
    fn bilinear_integer_is_nearest() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 1, Rgba([10, 20, 30, 255]));
        let p = sample_bilinear(&img, 1.0, 1.0).unwrap();
        assert_eq!(p.0, [10, 20, 30, 255]);
    }

    #[test]
    fn stamp_color_paints_center() {
        let mut img = RgbaImage::from_pixel(8, 8, Rgba([255, 255, 255, 255]));
        stamp_color(&mut img, 4, 4, 2, 1.0, [10, 20, 30]);
        assert_eq!(img.get_pixel(4, 4).0, [10, 20, 30, 255]);
    }

    #[test]
    fn erase_clears_core_and_keeps_outside() {
        let mut img = RgbaImage::from_pixel(16, 16, Rgba([8, 9, 10, 255]));
        assert!(erase_stamp(&mut img, 8, 8, 3, 1.0));
        assert_eq!(img.get_pixel(8, 8).0, [8, 9, 10, 0]);
        assert_eq!(img.get_pixel(0, 0).0[3], 255);
    }

    #[test]
    fn erase_soft_edge_keeps_some_alpha() {
        let mut img = RgbaImage::from_pixel(16, 16, Rgba([1, 2, 3, 200]));
        assert!(erase_stamp(&mut img, 8, 8, 6, 0.0));
        let rim = img.get_pixel(13, 8).0[3];
        assert!(rim > 0 && rim < 200, "rim alpha {rim}");
        assert_eq!(img.get_pixel(8, 8).0[3], 0);
    }

    fn stamp_color_ref(
        dst: &mut RgbaImage,
        dst_x: i32,
        dst_y: i32,
        radius: i32,
        hardness: f32,
        rgb: [u8; 3],
    ) {
        let radius = radius.max(1);
        let dw = dst.width() as i32;
        let dh = dst.height() as i32;
        let src = Rgba([rgb[0], rgb[1], rgb[2], 255]);
        for y in (dst_y - radius - 1).max(0)..(dst_y + radius + 2).min(dh) {
            for x in (dst_x - radius - 1).max(0)..(dst_x + radius + 2).min(dw) {
                let w = brush_weight(x - dst_x, y - dst_y, radius, hardness);
                if w <= 0.0 {
                    continue;
                }
                let d = *dst.get_pixel(x as u32, y as u32);
                dst.put_pixel(x as u32, y as u32, blend(d, src, w));
            }
        }
    }

    #[test]
    fn stamp_color_matches_reference_blend() {
        let mut a = RgbaImage::from_pixel(24, 24, Rgba([8, 16, 24, 200]));
        let mut b = a.clone();
        stamp_color(&mut a, 11, 13, 6, 0.4, [180, 40, 20]);
        stamp_color_ref(&mut b, 11, 13, 6, 0.4, [180, 40, 20]);
        assert_eq!(a.as_raw(), b.as_raw());
        stamp_color(&mut a, 14, 13, 6, 0.4, [180, 40, 20]);
        stamp_color_ref(&mut b, 14, 13, 6, 0.4, [180, 40, 20]);
        assert_eq!(a.as_raw(), b.as_raw());
    }
}
