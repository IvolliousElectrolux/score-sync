//! 画布扩缩、谱纸色、边沿裁切烤入.

use image::{Rgb, RgbImage, Rgba, RgbaImage};

/// 从边沿采样谱纸色 (众数, 排除过深像素).
pub fn sample_paper(img: &RgbImage, ink_threshold: i32) -> [u8; 3] {
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        return [245, 245, 245];
    }
    let mut hist = [[0u32; 256]; 3];
    let mut n = 0u32;
    let band = ((w.min(h) / 20).max(2)).min(24);
    for y in 0..h {
        for x in 0..w {
            let edge = x < band || y < band || x + band >= w || y + band >= h;
            if !edge {
                continue;
            }
            let p = img.get_pixel(x, y);
            let gray = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            if gray as i32 >= ink_threshold {
                for c in 0..3 {
                    hist[c][p[c] as usize] += 1;
                }
                n += 1;
            }
        }
    }
    if n < 8 {
        return [245, 245, 245];
    }
    let mut mode = [245u8; 3];
    for c in 0..3 {
        mode[c] = hist[c]
            .iter()
            .enumerate()
            .max_by_key(|(_, &v)| v)
            .map(|(i, _)| i as u8)
            .unwrap_or(245);
    }
    mode
}

/// 把 `extra_*` 烤进条带: 负值向内裁, 正值用谱纸色外扩. 不含 shift/gap.
pub fn apply_edge_adjust(
    img: &RgbImage,
    extra_top: i32,
    extra_bottom: i32,
    extra_left: i32,
    extra_right: i32,
    paper: [u8; 3],
) -> RgbImage {
    let ow = img.width() as i32;
    let oh = img.height() as i32;
    let max_trim_h = (oh - 1).max(0);
    let trim_top = (-extra_top).clamp(0, max_trim_h);
    let trim_bot = (-extra_bottom).clamp(0, max_trim_h - trim_top);
    let max_trim_w = (ow - 1).max(0);
    let trim_left = (-extra_left).clamp(0, max_trim_w);
    let trim_right = (-extra_right).clamp(0, max_trim_w - trim_left);
    let ext_top = extra_top.max(0);
    let ext_bot = extra_bottom.max(0);
    let ext_left = extra_left.max(0);
    let ext_right = extra_right.max(0);
    let cw = (ow - trim_left - trim_right).max(1);
    let ch = (oh - trim_top - trim_bot).max(1);
    let nw = (cw + ext_left + ext_right).max(1) as u32;
    let nh = (ch + ext_top + ext_bot).max(1) as u32;
    let mut out = RgbImage::from_pixel(nw, nh, Rgb(paper));
    let dx = ext_left;
    let dy = ext_top;
    for y in 0..ch {
        for x in 0..cw {
            let sx = (x + trim_left) as u32;
            let sy = (y + trim_top) as u32;
            if sx < img.width() && sy < img.height() {
                out.put_pixel((x + dx) as u32, (y + dy) as u32, *img.get_pixel(sx, sy));
            }
        }
    }
    out
}

#[allow(dead_code)]
pub fn erase_disk(img: &mut RgbaImage, cx: i32, cy: i32, radius: i32) {
    let r2 = radius.saturating_mul(radius);
    let w = img.width() as i32;
    let h = img.height() as i32;
    for y in (cy - radius).max(0)..(cy + radius + 1).min(h) {
        for x in (cx - radius).max(0)..(cx + radius + 1).min(w) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                img.put_pixel(x as u32, y as u32, Rgba([0, 0, 0, 0]));
            }
        }
    }
}

#[allow(dead_code)]
pub fn fill_disk_rgb(img: &mut RgbaImage, cx: i32, cy: i32, radius: i32, rgb: [u8; 3]) {
    let r2 = radius.saturating_mul(radius);
    let w = img.width() as i32;
    let h = img.height() as i32;
    for y in (cy - radius).max(0)..(cy + radius + 1).min(h) {
        for x in (cx - radius).max(0)..(cx + radius + 1).min(w) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                img.put_pixel(x as u32, y as u32, Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
    }
}

/// 在画布坐标下把选区抠成新图层像素 (相对选区原点).
pub fn extract_rect_layer(
    src: &RgbaImage,
    src_x: i32,
    src_y: i32,
    sel_x0: i32,
    sel_y0: i32,
    sel_x1: i32,
    sel_y1: i32,
) -> (RgbaImage, i32, i32) {
    let x0 = sel_x0.min(sel_x1);
    let y0 = sel_y0.min(sel_y1);
    let x1 = sel_x0.max(sel_x1);
    let y1 = sel_y0.max(sel_y1);
    let w = (x1 - x0 + 1).max(1) as u32;
    let h = (y1 - y0 + 1).max(1) as u32;
    let mut out = RgbaImage::new(w, h);
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let sx = x0 + x - src_x;
            let sy = y0 + y - src_y;
            if sx >= 0 && sy >= 0 && sx < sw && sy < sh {
                out.put_pixel(x as u32, y as u32, *src.get_pixel(sx as u32, sy as u32));
            }
        }
    }
    (out, x0, y0)
}

/// 从选区抠走: 原图层对应像素变透明 (或填谱纸色).
pub fn cut_rect_from_layer(
    img: &mut RgbaImage,
    src_x: i32,
    src_y: i32,
    sel_x0: i32,
    sel_y0: i32,
    sel_x1: i32,
    sel_y1: i32,
    fill_paper: Option<[u8; 3]>,
) {
    let x0 = sel_x0.min(sel_x1);
    let y0 = sel_y0.min(sel_y1);
    let x1 = sel_x0.max(sel_x1);
    let y1 = sel_y0.max(sel_y1);
    let sw = img.width() as i32;
    let sh = img.height() as i32;
    let fill = fill_paper.map(|c| Rgba([c[0], c[1], c[2], 255]));
    for cy in y0..=y1 {
        for cx in x0..=x1 {
            let sx = cx - src_x;
            let sy = cy - src_y;
            if sx >= 0 && sy >= 0 && sx < sw && sy < sh {
                img.put_pixel(sx as u32, sy as u32, fill.unwrap_or(Rgba([0, 0, 0, 0])));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_adjust_trims_and_expands() {
        let img = RgbImage::from_pixel(10, 8, Rgb([9, 9, 9]));
        let out = apply_edge_adjust(&img, -2, -1, -3, 4, [200, 200, 200]);
        assert_eq!(out.width(), 10 - 3 + 4);
        assert_eq!(out.height(), 8 - 2 - 1);
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9]);
        assert_eq!(out.get_pixel(out.width() - 1, 0).0, [200, 200, 200]);
    }
}
