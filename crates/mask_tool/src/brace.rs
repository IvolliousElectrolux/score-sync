//! 谱行组左侧大括号识别: 扫左侧窄条墨迹, 返回纵向范围. 钢琴括号是 `{`
//! (尖朝页边, 开口朝谱表). 对齐用的「尖尖」优先取左包络上三个向左凸点
//! 的中间那个; 峰不清楚则退回墨迹范围中点. 是否采用括号、是否实为通页
//! 边框, 由 `crate::staff::staff_align_anchor` 判断.
//!
//! 与 `score_sync::staff_detect::has_brace` 采用同样的"左侧窄条扫墨迹"
//! 思路 (mask_tool 是独立 crate, 不能直接依赖宿主的 staff_detect, 故
//! 在此自成一份轻量实现).

use image::RgbImage;

/// 大括号候选窄条: 页面/拼合图左侧多宽比例内扫描 (大括号通常紧贴谱行
/// 组左侧, 页边距本身也计入这个比例内).
const LEFT_BAND_MIN_FRAC: f32 = 0.004;
const LEFT_BAND_MAX_FRAC: f32 = 0.09;
/// 大括号纵向覆盖至少占扫描带高度的这个比例才采信 (太短更可能是杂色噪点).
/// 是否够到顶/底谱表、是否实为通页边框, 由 `staff_align_anchor` 再判.
const MIN_COVERAGE_FRAC: f32 = 0.10;
/// 左包络局部极小 (向左凸) 至少比两侧肩低这么多像素才算峰.
const CUSP_MIN_PROMINENCE: i32 = 2;
/// 三个凸点之间至少隔括号高度的这么多, 避免把同一坨噪点拆成三峰.
const CUSP_MIN_SEP_FRAC: f32 = 0.12;

fn is_ink(p: &image::Rgb<u8>, threshold: i32) -> bool {
    let lum = (p[0] as i32 * 30 + p[1] as i32 * 59 + p[2] as i32 * 11) / 100;
    lum <= threshold
}

fn left_band_hits(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    ink_threshold: i32,
) -> Option<(u32, Vec<bool>)> {
    let (w, h) = rgb.dimensions();
    if w < 8 || h < 8 || y1 <= y0 || sheet_x1 <= sheet_x0 {
        return None;
    }
    let y0 = y0.clamp(0, h as i32 - 1) as u32;
    let y1 = y1.clamp(0, h as i32 - 1) as u32;
    if y1 <= y0 {
        return None;
    }
    let sx0 = sheet_x0.clamp(0, w as i32 - 1) as u32;
    let sx1 = sheet_x1.clamp(0, w as i32 - 1) as u32;
    if sx1 <= sx0 {
        return None;
    }
    let sheet_w = (sx1 - sx0 + 1) as f32;
    let x0 = (sx0 as f32 + sheet_w * LEFT_BAND_MIN_FRAC).round() as u32;
    let x0 = x0.clamp(sx0, sx1);
    let x1 = (sx0 as f32 + sheet_w * LEFT_BAND_MAX_FRAC).round() as u32;
    let x1 = x1.clamp(x0 + 1, sx1);
    let mut hits = vec![false; (y1 - y0 + 1) as usize];
    for y in y0..=y1 {
        let mut hit = false;
        for x in x0..=x1 {
            if is_ink(rgb.get_pixel(x, y), ink_threshold) {
                hit = true;
                break;
            }
        }
        hits[(y - y0) as usize] = hit;
    }
    Some((y0, hits))
}

/// 在 `rgb` 图像 `[y0, y1]` × `[sheet_x0, sheet_x1]` 内寻找左侧大括号
/// 墨迹的纵向范围 `(min_y, max_y)`. `sheet_x*` 是谱面自身的横向范围
/// (有底色 letterbox 时不要用整张画布宽, 否则窄条会扫到空白边). 找不到
/// 满足覆盖率条件的候选时返回 `None`.
pub fn detect_brace_extent(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    ink_threshold: i32,
) -> Option<(i32, i32)> {
    let (origin, hits) = left_band_hits(rgb, y0, y1, sheet_x0, sheet_x1, ink_threshold)?;
    let band_h = hits.len() as f32;
    let mut min_ink_row: Option<u32> = None;
    let mut max_ink_row: Option<u32> = None;
    let mut ink_rows = 0u32;
    for (i, &hit) in hits.iter().enumerate() {
        if hit {
            let y = origin + i as u32;
            ink_rows += 1;
            min_ink_row = Some(min_ink_row.map_or(y, |v| v.min(y)));
            max_ink_row = Some(max_ink_row.map_or(y, |v| v.max(y)));
        }
    }
    let (Some(a), Some(b)) = (min_ink_row, max_ink_row) else {
        return None;
    };
    let coverage = ink_rows as f32 / band_h;
    if coverage < MIN_COVERAGE_FRAC {
        return None;
    }
    Some((a as i32, b as i32))
}

/// 左侧窄条里, 覆盖 `seed_y0..=seed_y1` 的那一截连续墨迹 (中间空白不超过
/// `max_gap` 行). 用来拿钢琴大括号: 不要 min/max 整段扫描带, 否则脚注
/// 栏的竖线会把范围拉到谱表下面; 也不要只扫已认出的那一行谱表, 否则
/// 只认出高音时尖尖会落在高音谱表里.
pub fn detect_brace_cluster_near(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    seed_y0: i32,
    seed_y1: i32,
    max_gap: i32,
    ink_threshold: i32,
) -> Option<(i32, i32)> {
    let (origin, hits) = left_band_hits(rgb, y0, y1, sheet_x0, sheet_x1, ink_threshold)?;
    if hits.is_empty() {
        return None;
    }
    let n = hits.len() as i32;
    let origin_i = origin as i32;
    let seed_lo = (seed_y0.min(seed_y1) - origin_i).clamp(0, n - 1);
    let seed_hi = (seed_y0.max(seed_y1) - origin_i).clamp(0, n - 1);
    let seed_mid = (seed_lo + seed_hi) / 2;
    let mut seed = None;
    for dist in 0..=(seed_hi - seed_lo).max(0) {
        for i in [seed_mid - dist, seed_mid + dist] {
            if i >= seed_lo && i <= seed_hi && hits[i as usize] {
                seed = Some(i);
                break;
            }
        }
        if seed.is_some() {
            break;
        }
    }
    let seed = seed?;
    let max_gap = max_gap.max(0);
    let expand = |from: i32, step: i32| -> i32 {
        let mut last_ink = from;
        let mut i = from + step;
        while i >= 0 && i < n {
            if hits[i as usize] {
                last_ink = i;
            } else if (i - last_ink).abs() > max_gap {
                break;
            }
            i += step;
        }
        last_ink
    };
    let a = origin_i + expand(seed, -1);
    let b = origin_i + expand(seed, 1);
    if b - a < 8 {
        return None;
    }
    Some((a, b))
}

/// 左侧窄条里所有连续墨迹簇 (中间空白不超过 `max_gap`). 一块里两行
/// 钢琴大谱表会得到两截括号, 不要合成一段.
pub fn detect_left_ink_clusters(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    max_gap: i32,
    min_height: i32,
    ink_threshold: i32,
) -> Vec<(i32, i32)> {
    let Some((origin, hits)) = left_band_hits(rgb, y0, y1, sheet_x0, sheet_x1, ink_threshold)
    else {
        return Vec::new();
    };
    let n = hits.len() as i32;
    if n == 0 {
        return Vec::new();
    }
    let origin_i = origin as i32;
    let max_gap = max_gap.max(0);
    let min_height = min_height.max(1);
    let mut out = Vec::new();
    let mut i = 0i32;
    while i < n {
        if !hits[i as usize] {
            i += 1;
            continue;
        }
        let start = i;
        let mut last_ink = i;
        i += 1;
        while i < n {
            if hits[i as usize] {
                last_ink = i;
            } else if i - last_ink > max_gap {
                break;
            }
            i += 1;
        }
        if last_ink - start + 1 >= min_height {
            out.push((origin_i + start, origin_i + last_ink));
        }
    }
    out
}

fn left_band_x_range(sheet_x0: i32, sheet_x1: i32) -> Option<(u32, u32)> {
    if sheet_x1 <= sheet_x0 {
        return None;
    }
    let sheet_w = (sheet_x1 - sheet_x0 + 1) as f32;
    let x0 = (sheet_x0 as f32 + sheet_w * LEFT_BAND_MIN_FRAC).round() as i32;
    let x1 = (sheet_x0 as f32 + sheet_w * LEFT_BAND_MAX_FRAC).round() as i32;
    let x0 = x0.clamp(sheet_x0, sheet_x1) as u32;
    let x1 = x1.clamp(x0 as i32 + 1, sheet_x1) as u32;
    Some((x0, x1))
}

/// 每一行在左侧窄条里最靠左的墨点 `x` (`None` = 该行无墨).
fn left_envelope_xs(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    ink_threshold: i32,
) -> Option<Vec<Option<i32>>> {
    let (w, h) = rgb.dimensions();
    if w < 8 || h < 8 || y1 <= y0 {
        return None;
    }
    let y0 = y0.clamp(0, h as i32 - 1) as u32;
    let y1 = y1.clamp(0, h as i32 - 1) as u32;
    if y1 <= y0 {
        return None;
    }
    let (bx0, bx1) = left_band_x_range(sheet_x0, sheet_x1)?;
    let bx0 = bx0.min(w - 1);
    let bx1 = bx1.min(w - 1);
    if bx1 <= bx0 {
        return None;
    }
    let mut xs = Vec::with_capacity((y1 - y0 + 1) as usize);
    for y in y0..=y1 {
        let mut found = None;
        for x in bx0..=bx1 {
            if is_ink(rgb.get_pixel(x, y), ink_threshold) {
                found = Some(x as i32);
                break;
            }
        }
        xs.push(found);
    }
    Some(xs)
}

fn fill_envelope_gaps(xs: &[Option<i32>]) -> Option<Vec<i32>> {
    if xs.iter().all(|v| v.is_none()) {
        return None;
    }
    let n = xs.len();
    let mut out = vec![0i32; n];
    let mut last = xs.iter().copied().flatten().next()?;
    for i in 0..n {
        if let Some(v) = xs[i] {
            last = v;
        }
        out[i] = last;
    }
    Some(out)
}

fn smooth_avg(xs: &[i32], radius: usize) -> Vec<i32> {
    if radius == 0 || xs.len() < 3 {
        return xs.to_vec();
    }
    let n = xs.len();
    let mut out = vec![0i32; n];
    for i in 0..n {
        let lo = i.saturating_sub(radius);
        let hi = (i + radius).min(n - 1);
        let span = (hi - lo + 1) as i32;
        let sum: i32 = xs[lo..=hi].iter().sum();
        out[i] = sum / span;
    }
    out
}

/// 左包络上向左凸的局部极小. 返回下标 (相对包络数组).
fn leftward_peaks(xs: &[i32], min_sep: usize, prominence: i32) -> Vec<usize> {
    let n = xs.len();
    if n < 5 {
        return Vec::new();
    }
    let mut raw = Vec::new();
    for i in 1..n - 1 {
        if xs[i] > xs[i - 1] || xs[i] > xs[i + 1] {
            continue;
        }
        if xs[i] == xs[i - 1] && xs[i] == xs[i + 1] {
            continue;
        }
        let lo = i.saturating_sub(min_sep.max(2));
        let hi = (i + min_sep.max(2)).min(n - 1);
        let left_max = xs[lo..=i].iter().copied().max().unwrap_or(xs[i]);
        let right_max = xs[i..=hi].iter().copied().max().unwrap_or(xs[i]);
        let prom = (left_max - xs[i]).min(right_max - xs[i]);
        if prom >= prominence {
            raw.push((i, prom));
        }
    }
    raw.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut kept: Vec<usize> = Vec::new();
    for (i, _) in raw {
        if kept.iter().any(|&k| k.abs_diff(i) < min_sep) {
            continue;
        }
        kept.push(i);
    }
    kept.sort_unstable();
    kept
}

/// `{` 括号的尖: 左包络三个向左凸点的中间那个 `y`.
/// 峰数不够 / 中间峰不在括号中段时返回 `None` (调用方退回墨迹中点).
/// 不要取全局最左像素: 上下两瓣常常比尖更靠左.
pub fn detect_brace_cusp_y(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    ink_threshold: i32,
) -> Option<i32> {
    if y1 - y0 < 16 {
        return None;
    }
    let xs = left_envelope_xs(rgb, y0, y1, sheet_x0, sheet_x1, ink_threshold)?;
    let filled = fill_envelope_gaps(&xs)?;
    let smooth = smooth_avg(&filled, 2);
    let h = (y1 - y0).max(1) as usize;
    let min_sep = ((h as f32 * CUSP_MIN_SEP_FRAC).round() as usize).max(8);
    let peaks = leftward_peaks(&smooth, min_sep, CUSP_MIN_PROMINENCE);
    let origin = y0;
    let pick = if peaks.len() == 3 {
        peaks[1]
    } else if peaks.len() == 1 {
        let i = peaks[0];
        let frac = i as f32 / (smooth.len().saturating_sub(1).max(1) as f32);
        if (0.28..=0.72).contains(&frac) {
            i
        } else {
            return None;
        }
    } else {
        return None;
    };
    Some(origin + pick as i32)
}

/// 对齐用括号纵坐标: 左包络尖清楚则用尖, 否则墨迹范围中点.
pub fn brace_anchor_y(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    sheet_x0: i32,
    sheet_x1: i32,
    ink_threshold: i32,
) -> i32 {
    detect_brace_cusp_y(rgb, y0, y1, sheet_x0, sheet_x1, ink_threshold)
        .unwrap_or((y0 + y1) / 2)
}

/// 大括号尖尖. 先试左包络中间凸点; 竖条/峰不清楚时退回墨迹中点.
/// 对齐时请走 `crate::staff::staff_align_anchor` (会再判断是否包住谱行组).
pub fn detect_brace_tip_y(rgb: &RgbImage, y0: i32, y1: i32, ink_threshold: i32) -> Option<i32> {
    let w = rgb.width() as i32;
    let (a, b) = detect_brace_extent(rgb, y0, y1, 0, w.saturating_sub(1), ink_threshold)?;
    Some(brace_anchor_y(rgb, a, b, 0, w.saturating_sub(1), ink_threshold))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn blank(w: u32, h: u32) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb([255, 255, 255]))
    }

    #[test]
    fn finds_midpoint_of_left_margin_ink_bar() {
        // 大括号占据 [40, 160] 这段纵向范围 (band 是 [0, 199], 高度 200,
        // 覆盖率 121/200 = 60.5%, 落在 [25%, 92%] 内).
        let mut img = blank(400, 200);
        for y in 40..=160u32 {
            img.put_pixel(10, y, Rgb([0, 0, 0]));
        }
        let tip = detect_brace_tip_y(&img, 0, 199, 128);
        assert_eq!(tip, Some(100));
    }

    #[test]
    fn no_left_margin_ink_returns_none() {
        let img = blank(400, 200);
        assert_eq!(detect_brace_tip_y(&img, 0, 199, 128), None);
    }

    #[test]
    fn short_noise_bar_is_rejected() {
        let mut img = blank(400, 200);
        for y in 90..=100u32 {
            img.put_pixel(10, y, Rgb([0, 0, 0]));
        }
        assert_eq!(detect_brace_tip_y(&img, 0, 199, 128), None);
    }

    #[test]
    fn cluster_near_staff_ignores_disconnected_footnote_rule() {
        let mut img = blank(400, 420);
        for y in 40..=192u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        for y in 280..=400u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        let cluster = detect_brace_cluster_near(&img, 0, 419, 0, 399, 40, 72, 16, 128);
        assert_eq!(cluster, Some((40, 192)));
        let full = detect_brace_extent(&img, 0, 419, 0, 399, 128);
        assert_eq!(full, Some((40, 400)));
    }

    fn paint_curly_brace(img: &mut RgbImage, y0: u32, y1: u32, tip_t: f32) {
        // `{` 左缘: 上下两瓣比尖更靠左, 尖在 tip_t (0=顶, 1=底).
        let h = (y1 - y0) as f32;
        for y in y0..=y1 {
            let t = (y - y0) as f32 / h.max(1.0);
            let outer_top = (-((t - 0.18) / 0.06).powi(2)).exp();
            let tip = (-((t - tip_t) / 0.05).powi(2)).exp();
            let outer_bot = (-((t - 0.82) / 0.06).powi(2)).exp();
            let x = (26.0 - 16.0 * outer_top - 10.0 * tip - 16.0 * outer_bot).round() as i32;
            let x = x.clamp(2, 35) as u32;
            for dx in 0..5u32 {
                if x + dx < img.width() {
                    img.put_pixel(x + dx, y, Rgb([0, 0, 0]));
                }
            }
        }
    }

    #[test]
    fn straight_bar_falls_back_to_extent_midpoint() {
        let mut img = blank(400, 200);
        for y in 40..=160u32 {
            img.put_pixel(10, y, Rgb([0, 0, 0]));
        }
        let cusp = detect_brace_cusp_y(&img, 40, 160, 0, 399, 128);
        assert_eq!(cusp, None);
        assert_eq!(detect_brace_tip_y(&img, 0, 199, 128), Some(100));
    }

    #[test]
    fn curly_brace_uses_middle_leftward_peak_not_outer_lobes() {
        let mut img = blank(400, 240);
        paint_curly_brace(&mut img, 40, 200, 0.50);
        let cusp = detect_brace_cusp_y(&img, 40, 200, 0, 399, 128);
        assert!(cusp.is_some(), "expected a cusp, got None");
        let y = cusp.unwrap();
        assert!((y - 120).abs() <= 8, "cusp y={y}, want ~120");
    }

    #[test]
    fn curly_brace_shifted_tip_is_not_extent_midpoint() {
        let mut img = blank(400, 240);
        paint_curly_brace(&mut img, 40, 200, 0.38);
        let cusp = detect_brace_cusp_y(&img, 40, 200, 0, 399, 128);
        let mid = (40 + 200) / 2;
        assert!(cusp.is_some(), "expected a cusp, got None");
        let y = cusp.unwrap();
        let expect = 40 + ((200 - 40) as f32 * 0.38).round() as i32;
        assert!((y - expect).abs() <= 10, "cusp y={y}, want ~{expect}");
        assert!((y - mid).abs() > 8, "should not fall back to midpoint {mid}");
    }
}
