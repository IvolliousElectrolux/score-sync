//! 旋转几何, 供图层拼合和光栅共用.

use image::{imageops, RgbaImage};

/// y 向下, 正角顺时针, 绕 (cx, cy).
pub fn rotate_point(px: f32, py: f32, cx: f32, cy: f32, degrees: f32) -> (f32, f32) {
    let rad = degrees.to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());
    let dx = px - cx;
    let dy = py - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

/// 原矩形绕中心旋转后的四个角 (y 向下, 正角顺时针): NW, NE, SE, SW.
pub fn rotated_corners(x: f32, y: f32, w: f32, h: f32, degrees: f32) -> [(f32, f32); 4] {
    let w = w.max(1.0);
    let h = h.max(1.0);
    let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    if degrees.abs() < 0.05 {
        return pts;
    }
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    pts.map(|(px, py)| rotate_point(px, py, cx, cy, degrees))
}

/// 原矩形绕中心旋转后的轴对齐包围盒 (y 向下, 正角顺时针).
/// 返回 (min_x, min_y, max_x, max_y), 四条边都经过某个角点.
pub fn rotated_aabb(x: f32, y: f32, w: f32, h: f32, degrees: f32) -> (f32, f32, f32, f32) {
    let corners = rotated_corners(x, y, w, h, degrees);
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for (rx, ry) in corners {
        min_x = min_x.min(rx);
        max_x = max_x.max(rx);
        min_y = min_y.min(ry);
        max_y = max_y.max(ry);
    }
    (min_x, min_y, max_x, max_y)
}

/// 裁掉四周全透明, 返回 (图, 相对原图的左上偏移).
pub fn trim_rgba(img: &RgbaImage) -> (RgbaImage, i32, i32) {
    let w = img.width();
    let h = img.height();
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 8 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x > max_x || min_y > max_y {
        return (img.clone(), 0, 0);
    }
    let nw = max_x - min_x + 1;
    let nh = max_y - min_y + 1;
    if nw == w && nh == h && min_x == 0 && min_y == 0 {
        return (img.clone(), 0, 0);
    }
    let cropped = imageops::crop_imm(img, min_x, min_y, nw, nh).to_image();
    (cropped, min_x as i32, min_y as i32)
}

pub fn rotate_rgba(img: &RgbaImage, degrees: f32) -> RgbaImage {
    let d = degrees.rem_euclid(360.0);
    if (d - 0.0).abs() < 0.01 {
        return img.clone();
    }
    if (d - 90.0).abs() < 0.01 {
        return imageops::rotate90(img);
    }
    if (d - 180.0).abs() < 0.01 {
        return imageops::rotate180(img);
    }
    if (d - 270.0).abs() < 0.01 {
        return imageops::rotate270(img);
    }
    rotate_bilinear(img, d)
}

fn rotate_bilinear(img: &RgbaImage, degrees: f32) -> RgbaImage {
    let rad = degrees.to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());
    let w = img.width() as f32;
    let h = img.height() as f32;
    let cx = w * 0.5;
    let cy = h * 0.5;
    let (min_x, min_y, max_x, max_y) = rotated_aabb(0.0, 0.0, w, h, degrees);
    let nw = (max_x - min_x).round().max(1.0) as u32;
    let nh = (max_y - min_y).round().max(1.0) as u32;
    let mut out = RgbaImage::new(nw, nh);
    let ncx = nw as f32 * 0.5;
    let ncy = nh as f32 * 0.5;
    let inv_cos = cos;
    let inv_sin = -sin;
    for y in 0..nh {
        for x in 0..nw {
            let dx = x as f32 + 0.5 - ncx;
            let dy = y as f32 + 0.5 - ncy;
            let sx = dx * inv_cos - dy * inv_sin + cx;
            let sy = dx * inv_sin + dy * inv_cos + cy;
            if let Some(p) = sample_bilinear(img, sx, sy) {
                out.put_pixel(x, y, p);
            }
        }
    }
    out
}

fn sample_bilinear(img: &RgbaImage, x: f32, y: f32) -> Option<image::Rgba<u8>> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return None;
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let mut acc = [0f32; 4];
    let mut wsum = 0f32;
    for (ix, iy, wt) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x0 + 1, y0, fx * (1.0 - fy)),
        (x0, y0 + 1, (1.0 - fx) * fy),
        (x0 + 1, y0 + 1, fx * fy),
    ] {
        if ix >= 0 && iy >= 0 && ix < w && iy < h {
            let p = img.get_pixel(ix as u32, iy as u32);
            for c in 0..4 {
                acc[c] += p[c] as f32 * wt;
            }
            wsum += wt;
        }
    }
    if wsum < 0.01 {
        return None;
    }
    let mut out = [0u8; 4];
    for c in 0..4 {
        out[c] = (acc[c] / wsum).round().clamp(0.0, 255.0) as u8;
    }
    Some(image::Rgba(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn aabb_edges_touch_corners() {
        let (x0, y0, x1, y1) = rotated_aabb(10.0, 20.0, 100.0, 40.0, 33.0);
        let cx = 10.0 + 50.0;
        let cy = 20.0 + 20.0;
        let corners = [
            rotate_point(10.0, 20.0, cx, cy, 33.0),
            rotate_point(110.0, 20.0, cx, cy, 33.0),
            rotate_point(10.0, 60.0, cx, cy, 33.0),
            rotate_point(110.0, 60.0, cx, cy, 33.0),
        ];
        let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
        let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
        assert!((min_x - x0).abs() < 1e-5);
        assert!((max_x - x1).abs() < 1e-5);
        assert!((min_y - y0).abs() < 1e-5);
        assert!((max_y - y1).abs() < 1e-5);
    }

    #[test]
    fn aabb_45_grows_then_90_swaps() {
        let (a, b, c, d) = rotated_aabb(0.0, 0.0, 100.0, 40.0, 0.0);
        assert!((c - a - 100.0).abs() < 0.01 && (d - b - 40.0).abs() < 0.01);
        let (a, b, c, d) = rotated_aabb(0.0, 0.0, 100.0, 40.0, 90.0);
        assert!((c - a - 40.0).abs() < 0.01 && (d - b - 100.0).abs() < 0.01);
        let (a, b, c, d) = rotated_aabb(0.0, 0.0, 100.0, 40.0, 45.0);
        assert!((d - b) > 70.0, "45° height {}", d - b);
        assert!((c - a) > 90.0, "45° width {}", c - a);
    }

    #[test]
    fn corners_follow_layer_not_aabb() {
        let c0 = rotated_corners(0.0, 0.0, 100.0, 40.0, 0.0);
        assert!((c0[1].0 - 100.0).abs() < 0.01 && (c0[2].1 - 40.0).abs() < 0.01);
        let c = rotated_corners(0.0, 0.0, 100.0, 40.0, 45.0);
        let (min_x, min_y, max_x, max_y) = rotated_aabb(0.0, 0.0, 100.0, 40.0, 45.0);
        let top = (c[0].0 - c[1].0).hypot(c[0].1 - c[1].1);
        let side = (c[1].0 - c[2].0).hypot(c[1].1 - c[2].1);
        assert!((top - 100.0).abs() < 0.05, "OBB top {top}");
        assert!((side - 40.0).abs() < 0.05, "OBB side {side}");
        let aabb_area = (max_x - min_x) * (max_y - min_y);
        assert!(
            aabb_area > 100.0 * 40.0 * 1.5,
            "AABB area {aabb_area} should exceed the layer"
        );
    }

    #[test]
    fn rotate90_swaps() {
        let img = RgbaImage::from_pixel(10, 4, Rgba([1, 2, 3, 255]));
        let out = rotate_rgba(&img, 90.0);
        assert_eq!(out.dimensions(), (4, 10));
    }

    #[test]
    fn trim_drops_transparent_margin() {
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0]));
        img.put_pixel(3, 2, Rgba([9, 9, 9, 255]));
        img.put_pixel(5, 6, Rgba([9, 9, 9, 255]));
        let (out, dx, dy) = trim_rgba(&img);
        assert_eq!((dx, dy), (3, 2));
        assert_eq!(out.dimensions(), (3, 5));
    }
}
