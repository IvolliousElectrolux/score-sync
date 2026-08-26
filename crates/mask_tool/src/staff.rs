//! 蒙版辅助线用的「这一块还是不是五线谱」再判定: 不看分块存盘时的
//! `kind` (用户可能已手动改切), 只看当前图像里是否还能认出至少一组
//! 五线谱表. 一块一个谱行组时对齐锚点用 `{` 的尖; 一块多个谱行组时
//! 用整块所有谱行组的几何中心 (第一组顶线到末组底线的中点). 认不出
//! 谱表的块用上下边界中线.

use std::collections::HashMap;

use image::RgbImage;

use crate::layout::{self, BlockAdjust};

use crate::brace::{
    brace_anchor_y, detect_brace_cluster_near, detect_brace_extent, detect_left_ink_clusters,
};

/// 与宿主 `score_sync::model::DEFAULT_INK_THRESHOLD` 一致.
pub const DEFAULT_INK_THRESHOLD: i32 = 200;

/// 大括号「够到」某一行谱表: 允许比谱表两端各伸出/缩进这么多 (相对该
/// 谱表自身高度). 扫描/老书制板时常差几线, 过严会把钢琴大括号判成
/// 没包住、退回重心, 对齐就偏.
const BRACE_STAFF_REACH_SLACK: f32 = 0.45;
/// 左侧墨迹几乎撑满整块 (含谱表上下的页边) 时当通页边框, 不用尖尖.
const BRACE_BLOCK_BORDER_FRAC: f32 = 0.92;
/// 谱表上下页边都小于这个像素时视为切得很紧, 不再用「撑满整块」当边框.
const BRACE_TIGHT_CROP_MARGIN: i32 = 8;

fn is_ink(p: &image::Rgb<u8>, threshold: i32) -> bool {
    let gray = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as i32;
    gray < threshold
}

fn is_staff_line_row(rgb: &RgbImage, y: u32, x0: u32, x1: u32, threshold: i32) -> bool {
    let w = (x1.saturating_sub(x0) + 1) as usize;
    if w < 8 {
        return false;
    }
    let n_bins = 20usize;
    let bw = (w / n_bins).max(1);
    let mut ink_count = 0usize;
    let mut longest = 0usize;
    let mut cur = 0usize;
    let mut transitions = 0usize;
    let mut prev = false;
    let mut bin_hit = [false; 20];
    for i in 0..w {
        let x = x0 + i as u32;
        let ink = is_ink(rgb.get_pixel(x, y), threshold);
        if ink {
            ink_count += 1;
            cur += 1;
            if cur > longest {
                longest = cur;
            }
            let bin = (i / bw).min(n_bins - 1);
            bin_hit[bin] = true;
        } else {
            cur = 0;
        }
        if i > 0 && ink != prev {
            transitions += 1;
        }
        prev = ink;
    }
    let wf = w as f32;
    if ink_count as f32 / wf < 0.42 {
        return false;
    }
    if longest as f32 / wf < 0.20 {
        return false;
    }
    if transitions > 48 {
        return false;
    }
    let bins = bin_hit.iter().filter(|&&v| v).count();
    bins >= 14
}

fn cluster_sorted(ys: &[i32], gap: i32) -> Vec<Vec<i32>> {
    if ys.is_empty() {
        return Vec::new();
    }
    let mut groups: Vec<Vec<i32>> = vec![vec![ys[0]]];
    for &y in &ys[1..] {
        let Some(last) = groups.last_mut() else {
            groups.push(vec![y]);
            continue;
        };
        let Some(&last_y) = last.last() else {
            last.push(y);
            continue;
        };
        if y - last_y <= gap {
            last.push(y);
        } else {
            groups.push(vec![y]);
        }
    }
    groups
}

fn find_staff_line_ys(rgb: &RgbImage, y0: u32, y1: u32, x0: u32, x1: u32, threshold: i32) -> Vec<i32> {
    let raw: Vec<i32> = (y0..=y1)
        .filter(|&y| is_staff_line_row(rgb, y, x0, x1, threshold))
        .map(|y| y as i32)
        .collect();
    let h = (y1.saturating_sub(y0) + 1).max(1);
    let thick_gap = (h as f32 / 700.0).round().max(3.0) as i32;
    cluster_sorted(&raw, thick_gap)
        .iter()
        .map(|c| {
            let sum: i32 = c.iter().sum();
            (sum as f32 / c.len() as f32).round() as i32
        })
        .collect()
}

fn typical_line_gap(line_ys: &[i32]) -> i32 {
    if line_ys.len() < 2 {
        return 8;
    }
    let gaps: Vec<i32> = line_ys.windows(2).map(|w| w[1] - w[0]).collect();
    // 先用最上面那组「连续 4 个相近的五线间距」(4..=24): 脚注横线的
    // 间距常在 16..=40, 整段中位数会被拉大, 真谱表 8px 缝反而 < min_g.
    for w in gaps.windows(4) {
        if w.iter().any(|&g| !(4..=24).contains(&g)) {
            continue;
        }
        let lo = *w.iter().min().unwrap_or(&8);
        let hi = *w.iter().max().unwrap_or(&8);
        if hi <= (lo * 2).max(lo + 4) {
            return ((w[0] + w[1] + w[2] + w[3]) / 4).max(4);
        }
    }
    let mut small: Vec<i32> = gaps
        .into_iter()
        .filter(|&g| (4..=48).contains(&g))
        .collect();
    if small.is_empty() {
        return 8;
    }
    small.sort_unstable();
    small[small.len() / 4].max(4)
}

/// 同一系统的谱表 (钢琴大谱表等). 行距大过约 4 倍首行谱表高时视为
/// 下一系统或脚注, 不再并进对齐用的谱行组.
fn first_system_staves(staves: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let Some(&first) = staves.first() else {
        return Vec::new();
    };
    let h0 = (first.1 - first.0).max(1);
    let mut out = vec![first];
    for &s in &staves[1..] {
        let gap = s.0 - out[out.len() - 1].1;
        if gap > h0.saturating_mul(4) {
            break;
        }
        out.push(s);
    }
    out
}

/// 把水平线收成五线谱表 (每组恰好 5 条、间距接近). 返回 (顶线, 底线).
fn group_staves(line_ys: &[i32]) -> Vec<(i32, i32)> {
    let n_lines = 5;
    let n = line_ys.len();
    if n < n_lines {
        return Vec::new();
    }
    let med_gap = typical_line_gap(line_ys);
    let min_g = ((med_gap as f32 * 0.45).round() as i32).max(3);
    let max_g = ((med_gap as f32 * 1.85).round() as i32).max(med_gap + 3);

    let mut staves = Vec::new();
    let mut i = 0;
    while i < n {
        let mut picked = vec![line_ys[i]];
        let mut j = i + 1;
        while picked.len() < n_lines && j < n {
            let Some(&last_y) = picked.last() else {
                break;
            };
            let gap = line_ys[j] - last_y;
            if gap < min_g {
                j += 1;
                continue;
            }
            if gap > max_g {
                break;
            }
            picked.push(line_ys[j]);
            j += 1;
        }
        if picked.len() == n_lines {
            staves.push((picked[0], picked[n_lines - 1]));
            i = j;
            continue;
        }
        i += 1;
    }
    staves
}

fn clamp_rect(rgb: &RgbImage, y0: i32, y1: i32, x0: i32, x1: i32) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = rgb.dimensions();
    if w < 8 || h < 8 || y1 <= y0 || x1 <= x0 {
        return None;
    }
    let y0 = y0.clamp(0, h as i32 - 1) as u32;
    let y1 = y1.clamp(0, h as i32 - 1) as u32;
    let x0 = x0.clamp(0, w as i32 - 1) as u32;
    let x1 = x1.clamp(0, w as i32 - 1) as u32;
    if y1 <= y0 || x1 <= x0 {
        return None;
    }
    Some((x0, x1, y0, y1))
}

fn find_staves(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    x0: i32,
    x1: i32,
    threshold: i32,
) -> Vec<(i32, i32)> {
    let Some((x0, x1, y0, y1)) = clamp_rect(rgb, y0, y1, x0, x1) else {
        return Vec::new();
    };
    let lines = find_staff_line_ys(rgb, y0, y1, x0, x1, threshold);
    first_system_staves(&group_staves(&lines))
}

fn find_all_staves(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    x0: i32,
    x1: i32,
    threshold: i32,
) -> Vec<(i32, i32)> {
    let Some((x0, x1, y0, y1)) = clamp_rect(rgb, y0, y1, x0, x1) else {
        return Vec::new();
    };
    let lines = find_staff_line_ys(rgb, y0, y1, x0, x1, threshold);
    group_staves(&lines)
}

/// 一块里多组谱表时的锚点: 第一组顶线到末组底线的中点 (整块几何中心).
fn systems_combined_centroid(systems: &[Vec<(i32, i32)>]) -> Option<i32> {
    let first = systems.first()?.first()?;
    let last = systems.last()?.last()?;
    Some((first.0 + last.1) / 2)
}

/// 按左侧括号簇把谱表收成谱行组. 一块里两行钢琴会有两截括号.
fn split_staff_systems(
    rgb: &RgbImage,
    staves: &[(i32, i32)],
    y0: i32,
    y1: i32,
    x0: i32,
    x1: i32,
    threshold: i32,
) -> Vec<Vec<(i32, i32)>> {
    if staves.is_empty() {
        return Vec::new();
    }
    let h0 = (staves[0].1 - staves[0].0).max(1);
    let max_gap = (h0 / 2).max(8);
    let clusters = detect_left_ink_clusters(rgb, y0, y1, x0, x1, max_gap, 8, threshold);
    let mut used = vec![false; staves.len()];
    let mut systems: Vec<Vec<(i32, i32)>> = Vec::new();
    for brace in clusters {
        let mut sys = Vec::new();
        for (i, &s) in staves.iter().enumerate() {
            if !used[i] && brace_reaches_staff(brace, s) {
                sys.push(s);
                used[i] = true;
            }
        }
        if !sys.is_empty() {
            systems.push(sys);
        }
    }
    let leftover: Vec<(i32, i32)> = staves
        .iter()
        .enumerate()
        .filter(|(i, _)| !used[*i])
        .map(|(_, s)| *s)
        .collect();
    for extra in split_staff_systems_by_gap(&leftover) {
        if extra.is_empty() {
            continue;
        }
        if let Some(prev) = systems.last_mut() {
            if let (Some(&last), Some(&e0)) = (prev.last(), extra.first()) {
                let h = (prev[0].1 - prev[0].0).max(1);
                let gap = e0.0 - last.1;
                // 括号只够到高音时, 低音仍属同一钢琴谱行组, 不要拆成两组.
                if gap <= h.saturating_mul(4) {
                    prev.extend(extra);
                    continue;
                }
            }
        }
        systems.push(extra);
    }
    if !systems.is_empty() {
        return systems;
    }
    split_staff_systems_by_gap(staves)
}

fn split_staff_systems_by_gap(staves: &[(i32, i32)]) -> Vec<Vec<(i32, i32)>> {
    if staves.is_empty() {
        return Vec::new();
    }
    let mut systems: Vec<Vec<(i32, i32)>> = vec![vec![staves[0]]];
    for &s in &staves[1..] {
        let prev = systems.last().unwrap();
        let last = *prev.last().unwrap();
        let h = (prev[0].1 - prev[0].0).max(1);
        let gap = s.0 - last.1;
        let new_system = if prev.len() >= 2 {
            let intra = prev[1].0 - prev[0].1;
            gap > (intra * 5 / 4).max(h.saturating_mul(2))
        } else {
            gap > h.saturating_mul(4)
        };
        if new_system {
            systems.push(vec![s]);
        } else {
            systems.last_mut().unwrap().push(s);
        }
    }
    systems
}

fn staff_group_extent(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    x0: i32,
    x1: i32,
    threshold: i32,
) -> Option<(i32, i32)> {
    let staves = find_staves(rgb, y0, y1, x0, x1, threshold);
    let first = staves.first()?;
    let last = staves.last()?;
    Some((first.0, last.1))
}

/// 当前图像块里能否认出至少一组五线谱表. `x0..=x1` / `y0..=y1` 是扫描
/// 范围 (画布/拼合图坐标); 底色 letterbox 时应传入谱面自身的横向范围,
/// 不要用整张画布宽, 否则谱线占宽不够会被漏判.
pub fn looks_like_staff(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    x0: i32,
    x1: i32,
    threshold: i32,
) -> bool {
    staff_group_extent(rgb, y0, y1, x0, x1, threshold).is_some()
}

fn brace_reaches_staff(brace: (i32, i32), staff: (i32, i32)) -> bool {
    let (b0, b1) = brace;
    let (s0, s1) = staff;
    let slack = (((s1 - s0).max(1) as f32) * BRACE_STAFF_REACH_SLACK).round() as i32;
    b1 >= s0 - slack && b0 <= s1 + slack
}

/// 大括号只要够到最上一行谱表和最下一行谱表即算覆盖 (单行谱表时两者
/// 是同一组). 不要求严格包住谱行组全高.
fn brace_covers_end_staves(brace: (i32, i32), staves: &[(i32, i32)]) -> bool {
    let Some(&first) = staves.first() else {
        return false;
    };
    let Some(&last) = staves.last() else {
        return false;
    };
    brace_reaches_staff(brace, first) && brace_reaches_staff(brace, last)
}

/// 左侧通页竖线: 墨迹几乎撑满整块, 且块在谱表上下还留得下页边.
fn brace_is_block_border(brace: (i32, i32), block: (i32, i32), staff: (i32, i32)) -> bool {
    let (b0, b1) = brace;
    let (y0, y1) = block;
    let bh = (y1 - y0).max(1) as f32;
    if (b1 - b0) as f32 / bh < BRACE_BLOCK_BORDER_FRAC {
        return false;
    }
    let margin_top = staff.0 - y0;
    let margin_bot = y1 - staff.1;
    margin_top >= BRACE_TIGHT_CROP_MARGIN || margin_bot >= BRACE_TIGHT_CROP_MARGIN
}

/// 对齐锚点 (画布/图像纵坐标): 一块里只有一个谱行组时, 大括号够到
/// 顶/底谱表则用 `{` 的尖 (左包络三峰中间凸点; 不清楚则退回该组几何
/// 重心). 一块里有多个谱行组时不用第一组的尖/中心, 改用整块所有谱行
/// 组的几何中心 (第一组顶线到末组底线的中点). 一块只能对一条辅助线.
/// 认不出五线谱时返回 `None`.
pub fn staff_align_anchor(
    rgb: &RgbImage,
    y0: i32,
    y1: i32,
    x0: i32,
    x1: i32,
    threshold: i32,
) -> Option<i32> {
    let all = find_all_staves(rgb, y0, y1, x0, x1, threshold);
    let mut systems = split_staff_systems(rgb, &all, y0, y1, x0, x1, threshold);
    // 括号簇若把两行钢琴收成一组, 再按行距拆一次.
    if systems.len() < 2 {
        let by_gap = split_staff_systems_by_gap(&all);
        if by_gap.len() >= 2 {
            systems = by_gap;
        }
    }
    if systems.len() >= 2 {
        return systems_combined_centroid(&systems);
    }
    let staves = systems.into_iter().next().filter(|s| !s.is_empty()).or_else(|| {
        let s = first_system_staves(&all);
        (!s.is_empty()).then_some(s)
    })?;
    let first = *staves.first()?;
    let last = *staves.last()?;
    let staff = (first.0, last.1);
    let centroid = (staff.0 + staff.1) / 2;
    let staff_h = (first.1 - first.0).max(1);
    // 从第一行谱表起向下探一个钢琴系统的高度, 让括号簇能接到低音谱表;
    // 不要扫整块 (脚注竖线), 也不要只扫已认出的那一行 (只认出高音时
    // 中点会落在高音谱表里).
    let pad = ((staff_h as f32) * 0.20).round().max(4.0) as i32;
    let scan0 = (first.0 - pad).max(y0);
    let scan1 = (last.1 + pad)
        .max(first.0 + staff_h.saturating_mul(8))
        .min(y1);
    if let Some(full) = detect_brace_extent(rgb, y0, y1, x0, x1, threshold) {
        if brace_is_block_border(full, (y0, y1), staff) {
            return Some(centroid);
        }
    }
    let max_gap = (staff_h / 2).max(8);
    let brace = detect_brace_cluster_near(
        rgb, scan0, scan1, x0, x1, first.0, first.1, max_gap, threshold,
    );
    if let Some(brace) = brace {
        let slack = (((staff_h as f32) * BRACE_STAFF_REACH_SLACK).round() as i32).max(4);
        let brace_h = (brace.1 - brace.0).max(1);
        // 括号明显高过单行谱表: 按括号底裁掉脚注假谱表, 再决定用尖尖
        // 还是系统重心. 短括号 (只画在高音旁) 不走这条, 以免把低音丢掉.
        if brace_h > staff_h.saturating_mul(2) {
            let system: Vec<(i32, i32)> = staves
                .iter()
                .copied()
                .filter(|s| s.0 <= brace.1 + slack)
                .collect();
            if !system.is_empty()
                && brace_reaches_staff(brace, system[0])
                && (system.len() == 1 || brace_covers_end_staves(brace, &system))
            {
                return Some(brace_anchor_y(rgb, brace.0, brace.1, x0, x1, threshold));
            }
            if let (Some(a), Some(b)) = (system.first(), system.last()) {
                return Some((a.0 + b.1) / 2);
            }
        }
        if brace_covers_end_staves(brace, &staves) {
            return Some(brace_anchor_y(rgb, brace.0, brace.1, x0, x1, threshold));
        }
    }
    Some(centroid)
}

/// 页图上一条带 (原始分块 y0..=y1) 的谱表锚点, 换成该条带自身坐标
/// (相对 y0). `None` 表示认不出五线谱. 不裁切、不拼画布, 供导入/后台
/// 预计算, 全局对齐时不再读整页拼合图.
pub fn band_staff_anchor(rgb: &RgbImage, y0: i32, y1: i32, threshold: i32) -> Option<i32> {
    let x1 = (rgb.width() as i32).saturating_sub(1);
    let a = staff_align_anchor(rgb, y0, y1, 0, x1, threshold)?;
    Some(a - y0)
}

/// 一块在「对齐到辅助线」时用的锚点: `offset_sheet` 是锚点相对该块 span
/// 顶边的拼合图像素 (不含 `voff`). 五线谱走 [`staff_align_anchor`];
/// 认不出谱表时 (歌词/说明文字等) 用上下边界的中线.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockAlignAnchor {
    pub region_id: String,
    pub offset_sheet: i32,
    pub is_staff: bool,
}

/// 在原始裁切条带上算锚点 (相对条带顶). 当页对齐与全局对齐都走这个,
/// 不要在整页图上带邻行像素重检, 也不要在缩放后的预览画布上重检.
pub fn piece_staff_ys_from_parts(
    parts: &[(String, RgbImage)],
    threshold: i32,
) -> HashMap<String, Option<i32>> {
    parts
        .iter()
        .map(|(id, img)| {
            let y1 = (img.height() as i32).saturating_sub(1);
            (id.clone(), band_staff_anchor(img, 0, y1, threshold))
        })
        .collect()
}

/// 用预计算的条带锚点还原对齐用的 `offset_sheet`.
/// `extra_top` 来自分块微调 (正值=顶上留白, 负值=从顶裁进内容);
/// 非谱表则用 span 高度中线.
pub fn block_anchor_from_piece_y(
    region_id: String,
    piece_staff_y: Option<i32>,
    extra_top: i32,
    span_h: i32,
) -> BlockAlignAnchor {
    match piece_staff_y {
        Some(y) => BlockAlignAnchor {
            region_id,
            offset_sheet: extra_top + y,
            is_staff: true,
        },
        None => BlockAlignAnchor {
            region_id,
            offset_sheet: (span_h - 1).max(0) / 2,
            is_staff: false,
        },
    }
}

/// 自上而下收集各块锚点. `spans` 是画布坐标 (`voff` + sheet×scale).
pub fn collect_block_align_anchors(
    rgb: &RgbImage,
    spans: &[(String, i64, i64)],
    x0: i32,
    x1: i32,
    content_scale: f32,
    threshold: i32,
) -> Vec<BlockAlignAnchor> {
    let cs = if content_scale > 0.0001 {
        content_scale
    } else {
        1.0
    };
    spans
        .iter()
        .map(|(rid, cy0, cy1)| {
            let (anchor, is_staff) =
                match staff_align_anchor(rgb, *cy0 as i32, *cy1 as i32, x0, x1, threshold) {
                    Some(a) => (a, true),
                    None => (((*cy0 + *cy1) / 2) as i32, false),
                };
            let offset = ((anchor as f32 - *cy0 as f32) / cs).round() as i32;
            BlockAlignAnchor {
                region_id: rid.clone(),
                offset_sheet: offset,
                is_staff,
            }
        })
        .collect()
}

/// 按辅助线当前纵坐标从上到下配对. 根数多于五线谱块数时, 把非谱表块
/// (文字等, 用上下边界中线) 也纳入, 让谱表和文字一起对齐; 否则仍只动
/// 五线谱块 (与旧行为一致).
pub fn assignments_for_guides(
    anchors: &[BlockAlignAnchor],
    guide_ys: &[i32],
) -> Vec<(String, i32, i32)> {
    if guide_ys.is_empty() || anchors.is_empty() {
        return Vec::new();
    }
    let n_staff = anchors.iter().filter(|a| a.is_staff).count();
    let items: Vec<&BlockAlignAnchor> = if guide_ys.len() > n_staff {
        anchors.iter().collect()
    } else {
        anchors.iter().filter(|a| a.is_staff).collect()
    };
    let mut targets = guide_ys.to_vec();
    targets.sort_unstable();
    let n = items.len().min(targets.len());
    items
        .into_iter()
        .zip(targets)
        .take(n)
        .map(|(a, t)| (a.region_id.clone(), a.offset_sheet, t))
        .collect()
}

/// 与蒙版「对齐」同一套: 按块 span + 条带预计算锚点还原 `offset_sheet`.
pub fn anchors_from_piece_ys(
    heights: &[(String, u32)],
    layout: &[BlockAdjust],
    piece_staff_ys: &HashMap<String, Option<i32>>,
) -> Vec<BlockAlignAnchor> {
    let spans = layout::compute_spans(heights, layout);
    spans
        .into_iter()
        .map(|(rid, y0, y1)| {
            let extra = BlockAdjust::find(layout, &rid)
                .map(|a| a.extra_top)
                .unwrap_or(0);
            let span_h = (y1 - y0 + 1) as i32;
            let piece_y = piece_staff_ys.get(&rid).copied().flatten();
            block_anchor_from_piece_y(rid, piece_y, extra, span_h)
        })
        .collect()
}

/// 一次「对齐到辅助线」的纯数据输入. 当页按钮与全局后台线程共用, 避免
/// 两条路径用不同的高度/锚点/页高而导致偏移.
#[derive(Clone, Debug)]
pub struct AlignGroupInput {
    pub heights: Vec<(String, u32)>,
    pub layout: Vec<BlockAdjust>,
    pub voff: i32,
    pub page_h: i32,
    pub anchors: Vec<BlockAlignAnchor>,
    pub guide_lines: Vec<i32>,
}

/// 与蒙版左键「对齐」同一套几何. 没有可配对的块时返回 `None` (不折
/// `voff`, 与当页按钮一致).
pub fn align_group(input: &AlignGroupInput) -> Option<(Vec<BlockAdjust>, i32)> {
    if input.guide_lines.is_empty() || input.heights.is_empty() {
        return None;
    }
    let assignments = assignments_for_guides(&input.anchors, &input.guide_lines);
    if assignments.is_empty() {
        return None;
    }
    Some(layout::align_blocks_to_targets(
        &input.heights,
        &input.layout,
        input.voff,
        &assignments,
        input.page_h,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn blank(w: u32, h: u32) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb([255, 255, 255]))
    }

    fn paint_staff(img: &mut RgbImage, top: u32, x0: u32, x1: u32, gap: u32) {
        for i in 0..5u32 {
            let y = top + i * gap;
            if y >= img.height() {
                break;
            }
            for x in x0..=x1.min(img.width() - 1) {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
    }

    #[test]
    fn blank_region_is_not_staff() {
        let img = blank(400, 200);
        assert!(!looks_like_staff(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD));
        assert_eq!(
            staff_align_anchor(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD),
            None
        );
    }

    #[test]
    fn five_lines_count_as_staff() {
        let mut img = blank(400, 200);
        paint_staff(&mut img, 40, 20, 380, 8);
        assert!(looks_like_staff(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD));
        // 谱表 40..72, 几何重心 56.
        assert_eq!(
            staff_align_anchor(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD),
            Some(56)
        );
    }

    #[test]
    fn three_lines_are_not_a_staff() {
        let mut img = blank(400, 200);
        for i in 0..3u32 {
            let y = 40 + i * 8;
            for x in 20..=380 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        assert!(!looks_like_staff(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD));
    }

    #[test]
    fn full_brace_uses_tip_not_just_centroid() {
        // 两行谱表 (钢琴), 大括号包住整个谱行组; 重心与尖尖都在两行中间,
        // 这里用「大括号比谱表略偏上」让两者可区分: 尖尖 = 括号中点.
        let mut img = blank(400, 280);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        // 谱行组 40..192. 括号画在 40..176 (仍覆盖 70%+ 且两端够近).
        for y in 40..=176u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
            img.put_pixel(13, y, Rgb([0, 0, 0]));
        }
        let staff = staff_group_extent(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(staff, Some((40, 192)));
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        // 括号中点 108, 谱行组重心 (40+192)/2 = 116.
        assert_eq!(anchor, Some(108));
    }

    #[test]
    fn partial_brace_falls_back_to_staff_centroid() {
        let mut img = blank(400, 280);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        // 只包住上行谱表, 不能代表整个谱行组.
        for y in 40..=72u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
            img.put_pixel(13, y, Rgb([0, 0, 0]));
        }
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some((40 + 192) / 2));
    }

    #[test]
    fn slightly_short_piano_brace_still_uses_tip() {
        // 钢琴两行谱, 大括号因扫描略短, 只伸进上下谱表内部, 不够到谱行
        // 组两端. 旧的 70%/18% 会判没包住; 现在够到顶/底谱表即用尖尖.
        let mut img = blank(400, 280);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        for y in 70..=170u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
            img.put_pixel(13, y, Rgb([0, 0, 0]));
        }
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some(120));
        assert_ne!(anchor, Some((40 + 192) / 2));
    }

    #[test]
    fn full_height_staff_brace_is_not_dropped_as_border() {
        // 切得很紧的谱行组上, 大括号几乎等高, 不能当通页边框丢掉.
        let mut img = blank(400, 200);
        paint_staff(&mut img, 8, 40, 380, 8);
        paint_staff(&mut img, 120, 40, 380, 8);
        for y in 8..=152u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        let staff = staff_group_extent(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(staff, Some((8, 152)));
        let anchor = staff_align_anchor(&img, 0, 199, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some((8 + 152) / 2));
    }

    #[test]
    fn page_border_with_margins_falls_back_to_centroid() {
        let mut img = blank(400, 280);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        for y in 0..280u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some((40 + 192) / 2));
    }

    #[test]
    fn footnote_lines_below_piano_do_not_steal_the_anchor() {
        // 谱行块裁切里常带着脚注. 脚注的横线不能把五线间距中位数拉偏,
        // 否则只认出高音谱表, 辅助线会落到高音谱表里.
        let mut img = blank(400, 520);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        for y in 40..=176u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
            img.put_pixel(13, y, Rgb([0, 0, 0]));
        }
        for i in 0..12u32 {
            let y = 260 + i * 20;
            for x in 20..=380u32 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        let staff = staff_group_extent(&img, 0, 519, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(staff, Some((40, 192)));
        let anchor = staff_align_anchor(&img, 0, 519, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some((40 + 176) / 2));
        assert_ne!(anchor, Some((40 + 72) / 2));
    }

    #[test]
    fn piano_brace_still_used_when_only_treble_groups() {
        // 低音谱表因音符过密认不出时, 仍应顺着大括号接到系统底, 不要把
        // 尖尖收成高音谱表中线.
        let mut img = blank(400, 280);
        paint_staff(&mut img, 40, 40, 380, 8);
        for y in 40..=192u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
            img.put_pixel(13, y, Rgb([0, 0, 0]));
        }
        let staff = staff_group_extent(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(staff, Some((40, 72)));
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some((40 + 192) / 2));
        assert_ne!(anchor, Some((40 + 72) / 2));
    }

    #[test]
    fn curly_brace_anchor_follows_cusp_not_ink_midpoint() {
        let mut img = blank(400, 280);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        let y0 = 40u32;
        let y1 = 192u32;
        let h = (y1 - y0) as f32;
        let tip_t = 0.38f32;
        for y in y0..=y1 {
            let t = (y - y0) as f32 / h;
            let outer_top = (-((t - 0.18) / 0.06).powi(2)).exp();
            let tip = (-((t - tip_t) / 0.05).powi(2)).exp();
            let outer_bot = (-((t - 0.82) / 0.06).powi(2)).exp();
            let x = (26.0 - 16.0 * outer_top - 10.0 * tip - 16.0 * outer_bot).round() as i32;
            let x = x.clamp(2, 35) as u32;
            for dx in 0..5u32 {
                img.put_pixel(x + dx, y, Rgb([0, 0, 0]));
            }
        }
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        let expect = 40 + (152.0 * tip_t).round() as i32;
        let mid = (40 + 192) / 2;
        let y = anchor.expect("piano curly brace should yield an anchor");
        assert!((y - expect).abs() <= 10, "anchor {y}, want cusp ~{expect}");
        assert!((y - mid).abs() > 6, "should not be ink midpoint {mid}");
    }

    #[test]
    fn two_piano_systems_in_one_block_use_combined_centroid() {
        // 一块里两行大谱表: 锚点是整块几何中心, 不是第一组的尖/中心.
        let mut img = blank(400, 520);
        paint_staff(&mut img, 40, 40, 380, 8);
        paint_staff(&mut img, 160, 40, 380, 8);
        paint_staff(&mut img, 300, 40, 380, 8);
        paint_staff(&mut img, 420, 40, 380, 8);
        for y in 40..=192u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        for y in 300..=452u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        let anchor = staff_align_anchor(&img, 0, 519, 0, 399, DEFAULT_INK_THRESHOLD);
        let first = (40 + 192) / 2;
        let combined = (40 + 452) / 2;
        assert_eq!(anchor, Some(combined));
        assert_ne!(anchor, Some(first));
    }

    #[test]
    fn left_margin_marks_above_staff_do_not_pull_brace_tip() {
        // 块顶左侧的速度/小节标记不应算进大括号墨迹范围.
        let mut img = blank(400, 280);
        paint_staff(&mut img, 80, 40, 380, 8);
        paint_staff(&mut img, 200, 40, 380, 8);
        for y in 8..=24u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
        }
        for y in 80..=232u32 {
            img.put_pixel(12, y, Rgb([0, 0, 0]));
            img.put_pixel(13, y, Rgb([0, 0, 0]));
        }
        let staff = staff_group_extent(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(staff, Some((80, 232)));
        let anchor = staff_align_anchor(&img, 0, 279, 0, 399, DEFAULT_INK_THRESHOLD);
        assert_eq!(anchor, Some((80 + 232) / 2));
        assert_ne!(anchor, Some((8 + 232) / 2));
    }

    #[test]
    fn assignments_only_staffs_when_guide_count_matches_staffs() {
        let anchors = vec![
            BlockAlignAnchor {
                region_id: "text".into(),
                offset_sheet: 10,
                is_staff: false,
            },
            BlockAlignAnchor {
                region_id: "s1".into(),
                offset_sheet: 20,
                is_staff: true,
            },
            BlockAlignAnchor {
                region_id: "s2".into(),
                offset_sheet: 30,
                is_staff: true,
            },
        ];
        let a = assignments_for_guides(&anchors, &[80, 200]);
        assert_eq!(
            a,
            vec![
                ("s1".into(), 20, 80),
                ("s2".into(), 30, 200),
            ]
        );
    }

    #[test]
    fn assignments_include_text_when_more_guides_than_staffs() {
        let anchors = vec![
            BlockAlignAnchor {
                region_id: "text".into(),
                offset_sheet: 10,
                is_staff: false,
            },
            BlockAlignAnchor {
                region_id: "s1".into(),
                offset_sheet: 20,
                is_staff: true,
            },
        ];
        let a = assignments_for_guides(&anchors, &[50, 150]);
        assert_eq!(
            a,
            vec![
                ("text".into(), 10, 50),
                ("s1".into(), 20, 150),
            ]
        );
    }

    #[test]
    fn blank_block_anchor_uses_vertical_midline() {
        let img = blank(400, 80);
        let spans = vec![("t".into(), 0i64, 79i64)];
        let a = collect_block_align_anchors(&img, &spans, 0, 399, 1.0, DEFAULT_INK_THRESHOLD);
        assert_eq!(a.len(), 1);
        assert!(!a[0].is_staff);
        assert_eq!(a[0].offset_sheet, 39);
    }

    #[test]
    fn band_staff_anchor_is_relative_to_band_top() {
        let mut img = blank(400, 240);
        paint_staff(&mut img, 40, 10, 390, 8);
        let y = band_staff_anchor(&img, 20, 120, DEFAULT_INK_THRESHOLD).unwrap();
        let abs = staff_align_anchor(&img, 20, 120, 0, 399, DEFAULT_INK_THRESHOLD).unwrap();
        assert_eq!(y, abs - 20);
    }

    #[test]
    fn block_anchor_from_piece_y_adds_extra_top() {
        let a = block_anchor_from_piece_y("s".into(), Some(40), 10, 100);
        assert!(a.is_staff);
        assert_eq!(a.offset_sheet, 50);
        let t = block_anchor_from_piece_y("t".into(), None, 0, 80);
        assert!(!t.is_staff);
        assert_eq!(t.offset_sheet, 39);
    }

    #[test]
    fn piece_crop_anchor_matches_full_page_band() {
        let mut page = blank(400, 400);
        paint_staff(&mut page, 80, 40, 380, 8);
        let full = band_staff_anchor(&page, 40, 200, DEFAULT_INK_THRESHOLD);
        let crop = image::imageops::crop_imm(&page, 0, 40, 400, 161).to_image();
        let piece = band_staff_anchor(&crop, 0, 160, DEFAULT_INK_THRESHOLD);
        assert_eq!(full, piece);
    }

    #[test]
    fn align_group_matches_page_align_geometry() {
        let heights = vec![("a".into(), 40u32), ("b".into(), 40)];
        let mut piece_ys = HashMap::new();
        piece_ys.insert("a".into(), Some(19));
        piece_ys.insert("b".into(), Some(19));
        let anchors = anchors_from_piece_ys(&heights, &[], &piece_ys);
        let input = AlignGroupInput {
            heights: heights.clone(),
            layout: vec![],
            voff: 40,
            page_h: 400,
            anchors,
            guide_lines: vec![100, 300],
        };
        let (layout, voff_delta) = align_group(&input).unwrap();
        assert_eq!(voff_delta, -40);
        let spans = layout::compute_spans(&heights, &layout);
        assert_eq!(spans[0].1 + 19, 100);
        assert_eq!(spans[1].1 + 19, 300);
    }
}
