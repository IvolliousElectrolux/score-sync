//! 「组合分块」拖动调整用的背景色模式识别与快速填充.
//!
//! 设计目标: 扫描件背景往往不是纯白, 而是类似 250±3 的轻微噪点; 用统计
//! (均值/标准差, 排除疑似墨迹像素) 加简单伪随机噪声还原, 比纯色填充更
//! 不违和. 生成速度只与新增像素数量成正比 (不重算未变化部分), 供拖拽时
//! 每帧调用也不卡顿.

use image::RgbImage;

/// 从样本图中提取背景色统计 (均值/标准差), 排除灰度低于 `ink_threshold`
/// 的疑似墨迹像素. 若样本几乎全是"墨迹"(极端情况), 退化为整体统计.
pub fn sample_bg_stats(img: &RgbImage, ink_threshold: i32) -> ([f32; 3], [f32; 3]) {
    if let Some(stats) = stats_filtered(img, ink_threshold) {
        return stats;
    }
    stats_filtered(img, 0).unwrap_or(([245.0, 245.0, 245.0], [2.0, 2.0, 2.0]))
}

fn stats_filtered(img: &RgbImage, ink_threshold: i32) -> Option<([f32; 3], [f32; 3])> {
    let mut sum = [0f64; 3];
    let mut sum_sq = [0f64; 3];
    let mut n = 0u64;
    for p in img.pixels() {
        let gray = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
        if gray as i32 >= ink_threshold {
            for c in 0..3 {
                let v = p[c] as f64;
                sum[c] += v;
                sum_sq[c] += v * v;
            }
            n += 1;
        }
    }
    if n < 16 {
        return None;
    }
    let mut mean = [0f32; 3];
    let mut std = [0f32; 3];
    for c in 0..3 {
        let m = sum[c] / n as f64;
        let var = (sum_sq[c] / n as f64 - m * m).max(0.0);
        mean[c] = m as f32;
        // 限制噪声幅度, 避免个别异常样本 (残留墨点漏检等) 导致花斑.
        std[c] = (var.sqrt() as f32).min(18.0);
    }
    Some((mean, std))
}

/// 生成 `width x height` 的背景填充图块. `seed` 保证同一参数下结果稳定
/// (同一次拖拽/多次重算/预览与导出之间不会因为随机数不同而闪烁).
pub fn synth_fill(width: u32, height: u32, mean: [f32; 3], std: [f32; 3], seed: u64) -> RgbImage {
    let w = width.max(1);
    let h = height.max(1);
    let mut img = RgbImage::new(w, h);
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    for p in img.pixels_mut() {
        for c in 0..3 {
            // xorshift64*: 快速且分布足够均匀, 无需引入随机数 crate 依赖.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let r = ((state >> 40) as f32 / (1u64 << 24) as f32) - 0.5; // 约 [-0.5, 0.5)
            let v = mean[c] + r * 2.0 * std[c];
            p[c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    img
}

/// 从图像边缘 (顶部或底部) 取 `sample_rows` 行作为背景采样区域; 若图像
/// 本身矮于该值则取全图.
pub fn edge_sample(img: &RgbImage, from_top: bool, sample_rows: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let rows = sample_rows.min(h).max(1);
    let y0 = if from_top { 0 } else { h - rows };
    image::imageops::crop_imm(img, 0, y0, w, rows).to_image()
}

/// FNV-1a: 把字符串稳定映射到一个 u64 种子, 供背景填充的伪随机噪声使用
/// (同一块/同一条边多次重算都得到一样的噪点, 不会闪烁).
pub fn seed_from(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn sample_stats_ignore_dark_ink_pixels() {
        let mut img = RgbImage::from_pixel(20, 20, Rgb([250, 250, 250]));
        for x in 0..20 {
            img.put_pixel(x, 10, Rgb([10, 10, 10]));
        }
        let (mean, _std) = sample_bg_stats(&img, 128);
        assert!(mean[0] > 240.0, "mean should stay near background: {mean:?}");
    }

    #[test]
    fn synth_fill_is_deterministic_for_same_seed() {
        let a = synth_fill(30, 10, [250.0, 248.0, 245.0], [3.0, 3.0, 3.0], 42);
        let b = synth_fill(30, 10, [250.0, 248.0, 245.0], [3.0, 3.0, 3.0], 42);
        assert_eq!(a.into_raw(), b.into_raw());
    }

    #[test]
    fn synth_fill_stays_within_plausible_range() {
        let img = synth_fill(50, 50, [250.0, 250.0, 250.0], [3.0, 3.0, 3.0], 7);
        for p in img.pixels() {
            for c in 0..3 {
                assert!(p[c] as i32 >= 200, "unexpected dark pixel: {p:?}");
            }
        }
    }
}
