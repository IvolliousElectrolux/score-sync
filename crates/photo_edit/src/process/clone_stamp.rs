//! 仿制图章: 只在当前图层取样/涂抹 (aligned, 源点随笔刷平移, 硬度控制边缘衰减).

use image::RgbaImage;

use super::brush::stamp_offset;

/// 从源图按相对偏移盖到目标层. `(src_x, src_y)` 对应当前笔尖在源图上的位置.
pub fn stamp_from_soft(
    dst: &mut RgbaImage,
    src: &RgbaImage,
    dst_x: i32,
    dst_y: i32,
    src_x: i32,
    src_y: i32,
    radius: i32,
    hardness: f32,
) {
    stamp_offset(
        dst, src, dst_x, dst_y, src_x, src_y, radius, hardness, [0.0; 3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn aligned_offset_copies_neighbor() {
        let mut dst = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255]));
        let mut src = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255]));
        src.put_pixel(2, 2, Rgba([200, 10, 10, 255]));
        stamp_from_soft(&mut dst, &src, 5, 5, 2, 2, 0, 1.0);
        assert_eq!(dst.get_pixel(5, 5).0, [200, 10, 10, 255]);
    }

    #[test]
    fn soft_edge_is_partial() {
        let mut dst = RgbaImage::from_pixel(16, 16, Rgba([0, 0, 0, 255]));
        let src = RgbaImage::from_pixel(16, 16, Rgba([255, 255, 255, 255]));
        stamp_from_soft(&mut dst, &src, 8, 8, 8, 8, 6, 0.0);
        let edge = dst.get_pixel(8 + 5, 8).0[0];
        assert!(edge > 0 && edge < 255, "soft edge {edge}");
    }
}
