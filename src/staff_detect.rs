//! 钢琴谱 (大谱表) 行检测 — 移植自 staff_detect.py.

use image::{Rgb, RgbImage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Band {
    pub y0: i32,
    pub y1: i32,
    pub kind: String,
}

/// 灰度 ink: True = 墨迹 (像素 < threshold).
fn to_ink(img: &RgbImage, threshold: u8) -> Vec<Vec<bool>> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut ink = vec![vec![false; w]; h];
    for y in 0..h {
        for x in 0..w {
            let Rgb([r, g, b]) = *img.get_pixel(x as u32, y as u32);
            let gray = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;
            ink[y][x] = gray < threshold;
        }
    }
    ink
}

fn cluster_sorted(ys: &[i32], gap: i32) -> Vec<Vec<i32>> {
    if ys.is_empty() {
        return Vec::new();
    }
    let mut groups: Vec<Vec<i32>> = vec![vec![ys[0]]];
    for &y in &ys[1..] {
        let last = groups.last_mut().unwrap();
        if y - *last.last().unwrap() <= gap {
            last.push(y);
        } else {
            groups.push(vec![y]);
        }
    }
    groups
}

fn longest_black_run(row: &[bool]) -> usize {
    let mut best = 0usize;
    let mut cur = 0usize;
    for &v in row {
        if v {
            cur += 1;
            if cur > best {
                best = cur;
            }
        } else {
            cur = 0;
        }
    }
    best
}

fn n_transitions(row: &[bool]) -> usize {
    if row.len() < 2 {
        return 0;
    }
    row.windows(2).filter(|w| w[0] != w[1]).count()
}

fn bin_coverage(row: &[bool], n_bins: usize) -> usize {
    let w = row.len();
    if w == 0 {
        return 0;
    }
    let bw = (w / n_bins).max(1);
    let mut covered = 0;
    for i in 0..n_bins {
        let a = i * bw;
        let b = if i + 1 == n_bins { w } else { (i + 1) * bw };
        if row[a..b].iter().any(|&x| x) {
            covered += 1;
        }
    }
    covered
}

fn is_staff_line_row(row: &[bool]) -> bool {
    let fill_min = 0.42f32;
    let longest_min = 0.20f32;
    let bins_min = 14usize;
    let n_bins = 20usize;
    let max_transitions = 48usize;
    let w = row.len().max(1);
    let fill = row.iter().filter(|&&x| x).count() as f32 / w as f32;
    if fill < fill_min {
        return false;
    }
    if bin_coverage(row, n_bins) < bins_min {
        return false;
    }
    let longest_r = longest_black_run(row) as f32 / w as f32;
    if longest_r < longest_min {
        return false;
    }
    if n_transitions(row) > max_transitions {
        return false;
    }
    true
}

fn find_staff_line_ys(ink: &[Vec<bool>]) -> Vec<i32> {
    let h = ink.len();
    let raw: Vec<i32> = (0..h)
        .filter(|&y| is_staff_line_row(&ink[y]))
        .map(|y| y as i32)
        .collect();
    let thick_gap = (h as f32 / 700.0).round().max(3.0) as i32;
    let clusters = cluster_sorted(&raw, thick_gap);
    clusters
        .iter()
        .map(|c| {
            let sum: i32 = c.iter().sum();
            (sum as f32 / c.len() as f32).round() as i32
        })
        .collect()
}

fn group_staves(line_ys: &[i32]) -> Vec<(i32, i32)> {
    let n_lines = 5;
    let mut staves = Vec::new();
    let mut i = 0;
    let n = line_ys.len();
    while i < n {
        if i + n_lines - 1 < n {
            let chunk = &line_ys[i..i + n_lines];
            let gaps: Vec<i32> = (0..n_lines - 1).map(|j| chunk[j + 1] - chunk[j]).collect();
            let mut sorted = gaps.clone();
            sorted.sort();
            let med = sorted[sorted.len() / 2];
            let max_gap = (med as f32 * 1.8).round() as i32 + 2;
            let max_gap = max_gap.max(14);
            let gap_spread = (med as f32 * 0.9).round() as i32 + 2;
            let gap_spread = gap_spread.max(7);
            let gmax = *gaps.iter().max().unwrap();
            let gmin = *gaps.iter().min().unwrap();
            if med >= 4 && gmax <= max_gap && gmax - gmin <= gap_spread {
                staves.push((chunk[0], chunk[n_lines - 1]));
                i += n_lines;
                continue;
            }
        }
        i += 1;
    }
    staves
}

fn pair_grand_systems(staves: &[(i32, i32)], ink: &[Vec<bool>]) -> Vec<(i32, i32)> {
    if staves.is_empty() {
        return Vec::new();
    }
    let heights: Vec<i32> = staves.iter().map(|(t, b)| b - t + 1).collect();
    let mut sorted_h = heights.clone();
    sorted_h.sort();
    let med_h = sorted_h[sorted_h.len() / 2];
    let min_gap = ((med_h as f32 * 0.35).round() as i32).max(6);
    // 紧密版心内间距可能接近系统间距, 略收紧并用大括号裁决
    let max_gap = ((med_h as f32 * 2.4).round() as i32).max(36);

    let mut systems = Vec::new();
    let mut used = vec![false; staves.len()];
    for i in 0..staves.len() {
        if used[i] {
            continue;
        }
        let (t0, b0) = staves[i];
        let mut brace_partner: Option<usize> = None;
        let mut gap_partner: Option<usize> = None;
        for j in (i + 1)..staves.len() {
            if used[j] {
                continue;
            }
            let gap = staves[j].0 - b0;
            if gap < min_gap {
                continue;
            }
            if gap > max_gap {
                break;
            }
            if gap_partner.is_none() {
                gap_partner = Some(j);
            }
            if has_brace(ink, t0, staves[j].1) {
                brace_partner = Some(j);
                break;
            }
        }
        let partner = brace_partner.or(gap_partner);
        if let Some(j) = partner {
            used[i] = true;
            used[j] = true;
            systems.push((t0, staves[j].1));
        } else {
            used[i] = true;
            systems.push((t0, b0));
        }
    }
    systems
}

/// 左缘大括号: 在页面左侧窄条里找纵向连续墨迹 (钢琴大谱表前的花括号).
fn find_brace_spans(ink: &[Vec<bool>]) -> Vec<(i32, i32)> {
    let h = ink.len();
    if h == 0 {
        return Vec::new();
    }
    let w = ink[0].len();
    let x_lo = ((w as f32 * 0.004).round() as usize).min(w.saturating_sub(1));
    let x_hi = ((w as f32 * 0.09).round() as usize)
        .max(x_lo + 3)
        .min(w);
    // 每行左侧是否有足够墨迹 (括号笔画)
    let left_hit: Vec<bool> = (0..h)
        .map(|y| {
            let cnt = ink[y][x_lo..x_hi].iter().filter(|&&v| v).count();
            cnt >= 2
        })
        .collect();
    let mut runs = Vec::new();
    let mut start: Option<i32> = None;
    for y in 0..h {
        if left_hit[y] {
            if start.is_none() {
                start = Some(y as i32);
            }
        } else if let Some(s) = start.take() {
            runs.push((s, y as i32 - 1));
        }
    }
    if let Some(s) = start {
        runs.push((s, h as i32 - 1));
    }
    // 花括号中段可能镂空, 允许桥接细缝
    let bridge = ((h as f32 / 350.0).round() as i32).clamp(3, 12);
    let merged = merge_close_intervals(&runs, bridge);
    let min_h = ((h as f32 * 0.032).round() as i32).max(36);
    let max_h = ((h as f32 * 0.24).round() as i32).max(min_h + 10);
    merged
        .into_iter()
        .filter(|&(a, b)| {
            let hh = b - a + 1;
            hh >= min_h && hh <= max_h
        })
        .collect()
}

fn has_brace(ink: &[Vec<bool>], y0: i32, y1: i32) -> bool {
    let h = ink.len() as i32;
    if h <= 0 || y1 < y0 {
        return false;
    }
    let y0 = y0.max(0);
    let y1 = y1.min(h - 1);
    let span = y1 - y0 + 1;
    if span < 20 {
        return false;
    }
    let w = ink[0].len();
    let x_lo = ((w as f32 * 0.004).round() as usize).min(w.saturating_sub(1));
    let x_hi = ((w as f32 * 0.09).round() as usize)
        .max(x_lo + 3)
        .min(w);
    let mut hit_rows = 0i32;
    for y in y0..=y1 {
        let cnt = ink[y as usize][x_lo..x_hi]
            .iter()
            .filter(|&&v| v)
            .count();
        if cnt >= 2 {
            hit_rows += 1;
        }
    }
    hit_rows as f32 >= span as f32 * 0.42
}

/// 谱表核内竖直小节线数量 (用于确认真谱表行).
fn count_barlines(ink: &[Vec<bool>], y0: i32, y1: i32) -> i32 {
    let h = ink.len() as i32;
    if h <= 0 || y1 <= y0 {
        return 0;
    }
    let y0 = y0.max(0);
    let y1 = y1.min(h - 1);
    let span = (y1 - y0 + 1).max(1);
    let min_run = ((span as f32 * 0.55).round() as i32).max(12);
    let w = ink[0].len();
    // 避开最左括号区与最右页边
    let x_lo = ((w as f32 * 0.08).round() as usize).min(w.saturating_sub(1));
    let x_hi = ((w as f32 * 0.96).round() as usize).max(x_lo + 1).min(w);
    let mut counts = 0i32;
    let mut x = x_lo;
    while x < x_hi {
        let mut run = 0i32;
        let mut best = 0i32;
        for y in y0..=y1 {
            // 小节线窄: 本列有墨, 且左右邻列墨较少 (或本列连续)
            let here = ink[y as usize][x];
            if here {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        if best >= min_run {
            // 估算线宽: 向右看连续细柱
            let mut width = 1usize;
            while x + width < x_hi && width < 5 {
                let mut run2 = 0i32;
                let mut best2 = 0i32;
                for y in y0..=y1 {
                    if ink[y as usize][x + width] {
                        run2 += 1;
                        best2 = best2.max(run2);
                    } else {
                        run2 = 0;
                    }
                }
                if best2 >= min_run {
                    width += 1;
                } else {
                    break;
                }
            }
            if width <= 4 {
                counts += 1;
            }
            x += width.max(1) + 6; // 跳过这根线附近
        } else {
            x += 1;
        }
    }
    counts
}

fn looks_like_system(ink: &[Vec<bool>], y0: i32, y1: i32) -> bool {
    has_brace(ink, y0, y1) || count_barlines(ink, y0, y1) >= 2
}

/// 用五线核 + 大括号核合并出「谱表行」骨架; 绝不把多行粘成一行.
fn collect_system_cores(ink: &[Vec<bool>]) -> Vec<(i32, i32)> {
    let line_ys = find_staff_line_ys(ink);
    let staves = group_staves(&line_ys);
    let staff_systems = pair_grand_systems(&staves, ink);
    let braces = find_brace_spans(ink);

    let mut cores: Vec<(i32, i32)> = Vec::new();
    if braces.len() >= 2 {
        // 大括号优先: 每个括号对应一行大谱表, 用内部五线收紧上下
        for &(b0, b1) in &braces {
            let pad = ((b1 - b0 + 1) as f32 * 0.15).round() as i32;
            let search0 = (b0 - pad).max(0);
            let search1 = b1 + pad;
            let mut inner: Vec<(i32, i32)> = staff_systems
                .iter()
                .copied()
                .filter(|&(t, b)| t >= search0 - 4 && b <= search1 + 4 && overlaps(t, b, b0, b1))
                .collect();
            if inner.is_empty() {
                // 括号范围内的单行谱表
                let local_staves: Vec<(i32, i32)> = staves
                    .iter()
                    .copied()
                    .filter(|&(t, b)| overlaps(t, b, search0, search1))
                    .collect();
                if local_staves.len() >= 2 {
                    inner.push((local_staves[0].0, local_staves[local_staves.len() - 1].1));
                } else if let Some(&(t, b)) = local_staves.first() {
                    inner.push((t, b));
                }
            }
            if let Some(&(t, b)) = inner.first() {
                // 若括号内误收多块 staff_systems, 取与括号重叠最大的
                let best = inner
                    .iter()
                    .copied()
                    .max_by_key(|&(t, b)| {
                        let lo = t.max(b0);
                        let hi = b.min(b1);
                        (hi - lo + 1).max(0)
                    })
                    .unwrap_or((t, b));
                cores.push((best.0.min(b0), best.1.max(b1)));
            } else {
                cores.push((b0, b1));
            }
        }
    } else {
        cores = staff_systems;
    }

    // 丢掉既无括号又几乎无小节线的假核
    if cores.len() > 1 {
        let filtered: Vec<_> = cores
            .iter()
            .copied()
            .filter(|&(t, b)| looks_like_system(ink, t, b))
            .collect();
        if filtered.len() >= 2 {
            cores = filtered;
        }
    }

    cores.sort_by_key(|c| c.0);
    // 核若几乎重合只留一个
    let mut dedup: Vec<(i32, i32)> = Vec::new();
    for (t, b) in cores {
        if let Some(last) = dedup.last_mut() {
            let lo = t.max(last.0);
            let hi = b.min(last.1);
            let overlap = (hi - lo + 1).max(0);
            let min_h = (b - t + 1).min(last.1 - last.0 + 1).max(1);
            if overlap as f32 > min_h as f32 * 0.55 {
                last.0 = last.0.min(t);
                last.1 = last.1.max(b);
                continue;
            }
        }
        dedup.push((t, b));
    }
    dedup
}

/// 从谱表核扩到内容边界.
/// 宽松/紧密判定: 向邻行**五线核**方向搜时, 是否先遇到「整行完全无黑像素」.
/// - 宽松 (五线之前能见到空白): 扩到分隔空白为止 (含踏板/指法; 细白缝可桥接)
/// - 紧密 (空白前就顶到邻行五线): 与邻核平分间隙
fn system_extents(ink: &[Vec<bool>], cores: &[(i32, i32)], margin: i32) -> Vec<(i32, i32)> {
    let h = ink.len() as i32;
    if cores.is_empty() || h <= 0 {
        return Vec::new();
    }
    // 真正的「横向完全没有黑像素」
    let row_blank: Vec<bool> = ink
        .iter()
        .map(|row| !row.iter().any(|&x| x))
        .collect();
    // 谱表与 Ped. 之间细白缝不截断; 达到此长度才算行间分隔
    let sep_blank = 3i32;

    let mut extents = Vec::with_capacity(cores.len());
    for i in 0..cores.len() {
        let (ct, cb) = cores[i];
        let hard_top = if i > 0 { cores[i - 1].1 + 1 } else { 0 };
        let hard_bot = if i + 1 < cores.len() {
            cores[i + 1].0 - 1
        } else {
            h - 1
        };

        // 判定: 碰到下一行五线之前是否存在整行无墨
        let loose_down =
            has_blank_before(&row_blank, cb, hard_bot, 1) || i + 1 >= cores.len();
        let loose_up = has_blank_before(&row_blank, ct, hard_top, -1) || i == 0;

        let mut y1 = if loose_down {
            expand_to_separator(&row_blank, cb, hard_bot, 1, sep_blank)
        } else {
            let mid = (cb + cores[i + 1].0) / 2;
            mid.min(hard_bot).max(cb)
        };

        let mut y0 = if loose_up {
            expand_to_separator(&row_blank, ct, hard_top, -1, sep_blank)
        } else {
            let mid = (cores[i - 1].1 + ct) / 2;
            (mid + 1).max(hard_top).min(ct)
        };

        let room_up = (y0 - hard_top).max(0);
        let room_down = (hard_bot - y1).max(0);
        let up = if loose_up {
            if i > 0 {
                margin.min(room_up / 2)
            } else {
                margin.min(room_up)
            }
        } else {
            0
        };
        let down = if loose_down {
            if i + 1 < cores.len() {
                margin.min(room_down / 2)
            } else {
                margin.min(room_down)
            }
        } else {
            0
        };
        y0 = (y0 - up).max(hard_top);
        y1 = (y1 + down).min(hard_bot);
        if y1 < y0 {
            y0 = ct;
            y1 = cb;
        }
        extents.push((y0, y1));
    }
    extents
}

fn has_blank_before(row_blank: &[bool], from: i32, hard: i32, dir: i32) -> bool {
    first_blank_row(row_blank, from, hard, dir).is_some()
}

/// 从 `from` 沿 `dir` 走到 `hard`(含) 之前, 找第一个整行无墨的 y; 碰到 hard 仍没有则 None.
fn first_blank_row(row_blank: &[bool], from: i32, hard: i32, dir: i32) -> Option<i32> {
    let h = row_blank.len() as i32;
    let mut y = from;
    loop {
        let next = y + dir;
        if dir > 0 && next > hard {
            return None;
        }
        if dir < 0 && next < hard {
            return None;
        }
        if next < 0 || next >= h {
            return None;
        }
        if row_blank[next as usize] {
            return Some(next);
        }
        y = next;
    }
}

/// 沿 dir 扩展: 吃掉有墨行; 短于 `sep` 的空白桥接过去; 遇到 ≥sep 的空白分隔则停在最后有墨行.
fn expand_to_separator(
    row_blank: &[bool],
    from: i32,
    hard: i32,
    dir: i32,
    sep: i32,
) -> i32 {
    let h = row_blank.len() as i32;
    let mut y = from;
    let mut scan = from;
    let mut blank_run = 0i32;
    loop {
        let next = scan + dir;
        if dir > 0 && next > hard {
            break;
        }
        if dir < 0 && next < hard {
            break;
        }
        if next < 0 || next >= h {
            break;
        }
        if row_blank[next as usize] {
            blank_run += 1;
            if blank_run >= sep {
                break;
            }
            scan = next;
        } else {
            blank_run = 0;
            scan = next;
            y = next;
        }
    }
    y
}

/// 丢掉明显过矮的假大谱表 (ossia / 误检五线碎片).
fn filter_short_systems(systems: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    if systems.len() <= 1 {
        return systems;
    }
    let mut heights: Vec<i32> = systems.iter().map(|(t, b)| b - t + 1).collect();
    heights.sort();
    let med = heights[heights.len() / 2];
    let min_h = ((med as f32 * 0.40).round() as i32).max(48);
    systems
        .into_iter()
        .filter(|&(t, b)| b - t + 1 >= min_h)
        .collect()
}

/// 把紧贴大谱表的细碎墨迹并进邻近 system, 避免多出几条纸片带.
/// 注意: 绝不能把相邻 system 互相粘连.
fn absorb_fragments_into_systems(
    systems: &mut Vec<(i32, i32)>,
    fragments: &[(i32, i32)],
    max_gap: i32,
    max_frag_h: i32,
) -> Vec<(i32, i32)> {
    if systems.is_empty() {
        return fragments.to_vec();
    }
    systems.sort();
    let mut remain = Vec::new();
    for &(a, b) in fragments {
        let frag_h = b - a + 1;
        if frag_h > max_frag_h {
            remain.push((a, b));
            continue;
        }
        let mut best: Option<(usize, i32)> = None;
        for (i, &(s0, s1)) in systems.iter().enumerate() {
            if b < s0 {
                let gap = s0 - b - 1;
                if gap <= max_gap {
                    let better = best.map(|(_, g)| gap < g).unwrap_or(true);
                    if better {
                        best = Some((i, gap));
                    }
                }
            } else if a > s1 {
                let gap = a - s1 - 1;
                if gap <= max_gap {
                    let better = best.map(|(_, g)| gap < g).unwrap_or(true);
                    if better {
                        best = Some((i, gap));
                    }
                }
            } else {
                best = Some((i, 0));
                break;
            }
        }
        if let Some((i, _)) = best {
            let (s0, s1) = systems[i];
            // 扩展时不超过与邻谱表的中点, 防止粘连
            let prev_end = if i > 0 { systems[i - 1].1 } else { -1 };
            let next_start = if i + 1 < systems.len() {
                systems[i + 1].0
            } else {
                i32::MAX / 4
            };
            let mid_up = if prev_end >= 0 {
                (prev_end + s0) / 2
            } else {
                0
            };
            let mid_down = if next_start < i32::MAX / 4 {
                (s1 + next_start) / 2
            } else {
                i32::MAX / 4
            };
            let new0 = s0.min(a).max(mid_up);
            let new1 = s1.max(b).min(mid_down);
            systems[i] = (new0.min(new1), new0.max(new1));
        } else {
            remain.push((a, b));
        }
    }
    // 仅合并真正纵坐标重叠的; 相邻 (a == last.1+1) 绝不能粘
    systems.sort();
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (a, b) in systems.drain(..) {
        if let Some(last) = merged.last_mut() {
            if a <= last.1 {
                last.1 = last.1.max(b);
                continue;
            }
        }
        merged.push((a, b));
    }
    *systems = merged;
    remain
}

/// 页顶扫描黑边 / 装订线: 很薄且靠页顶 → 不当 header.
fn is_scan_edge_band(y0: i32, y1: i32, page_h: i32) -> bool {
    let hh = y1 - y0 + 1;
    let top_zone = ((page_h as f32 * 0.055).round() as i32).max(48);
    let max_h = ((page_h as f32 * 0.028).round() as i32).clamp(10, 48);
    // 贴顶的薄条几乎都是扫描黑边
    if y0 <= 3 && hh <= max_h.max(56) {
        return true;
    }
    y0 <= top_zone && hh <= max_h
}

/// 页底版号 / 页码 (如 "E. 4279 C."): 很薄且靠页底 → 不当 footer 分块.
fn is_page_number_band(y0: i32, y1: i32, page_h: i32) -> bool {
    if page_h <= 0 || y1 < y0 {
        return false;
    }
    let hh = y1 - y0 + 1;
    let bot = page_h - 1;
    let bottom_zone = ((page_h as f32 * 0.08).round() as i32).max(64);
    let max_h = ((page_h as f32 * 0.035).round() as i32).clamp(12, 56);
    // 贴底的薄条几乎都是版号/页码
    if y1 >= bot - 3 && hh <= max_h.max(56) {
        return true;
    }
    y1 >= bot - bottom_zone && hh <= max_h
}

/// content_bounds / absorb 后可能重叠: 在中点切开, 绝不粘成一片.
fn clamp_systems_apart(systems: &mut Vec<(i32, i32)>) {
    if systems.len() < 2 {
        return;
    }
    systems.sort_by_key(|s| s.0);
    for i in 0..systems.len() - 1 {
        if systems[i].1 >= systems[i + 1].0 {
            let lo = systems[i + 1].0;
            let hi = systems[i].1;
            let mid = (lo + hi) / 2;
            systems[i].1 = mid.max(systems[i].0);
            systems[i + 1].0 = (mid + 1).min(systems[i + 1].1);
        }
    }
}

fn merge_close_intervals(intervals: &[(i32, i32)], merge_gap: i32) -> Vec<(i32, i32)> {
    if intervals.is_empty() {
        return Vec::new();
    }
    let mut intervals: Vec<(i32, i32)> = intervals.to_vec();
    intervals.sort();
    let mut merged: Vec<[i32; 2]> = vec![[intervals[0].0, intervals[0].1]];
    for &(a, b) in &intervals[1..] {
        let last = merged.last_mut().unwrap();
        if a <= last[1] + merge_gap {
            last[1] = last[1].max(b);
        } else {
            merged.push([a, b]);
        }
    }
    merged.into_iter().map(|m| (m[0], m[1])).collect()
}

fn dense_intervals(ink: &[Vec<bool>], dense_ratio: f32, min_height: i32) -> Vec<(i32, i32)> {
    let h = ink.len();
    let w = if h > 0 { ink[0].len() } else { 0 };
    let dense: Vec<bool> = ink
        .iter()
        .map(|row| {
            let sum = row.iter().filter(|&&x| x).count();
            sum as f32 > w as f32 * dense_ratio
        })
        .collect();
    let mut out = Vec::new();
    let mut start: Option<i32> = None;
    for (y, &d) in dense.iter().enumerate() {
        let y = y as i32;
        if d && start.is_none() {
            start = Some(y);
        } else if !d {
            if let Some(s) = start.take() {
                if y - 1 - s + 1 >= min_height {
                    out.push((s, y - 1));
                }
            }
        }
    }
    if let Some(s) = start {
        if h as i32 - 1 - s + 1 >= min_height {
            out.push((s, h as i32 - 1));
        }
    }
    out
}

fn overlaps(a0: i32, a1: i32, b0: i32, b1: i32) -> bool {
    !(a1 < b0 || a0 > b1)
}

/// 正文大谱表 vs 脚注区小谱例: 从页眉往下连续收正文;
/// 进入下半页且明显变矮时停止, 其后一律归 footer 区.
fn pick_body_systems(systems: Vec<(i32, i32)>, page_h: i32) -> Vec<(i32, i32)> {
    if systems.is_empty() {
        return systems;
    }
    let upper_cut = ((page_h as f32 * 0.58).round() as i32).max(1);
    let mut upper_h: Vec<i32> = systems
        .iter()
        .filter(|(t, _)| *t < upper_cut)
        .map(|(t, b)| b - t + 1)
        .collect();
    if upper_h.is_empty() {
        upper_h = systems.iter().map(|(t, b)| b - t + 1).collect();
    }
    upper_h.sort();
    let med = upper_h[upper_h.len() / 2];
    let min_body = ((med as f32 * 0.55).round() as i32).max(56);

    let mut body = Vec::new();
    for &(t, b) in &systems {
        let hh = b - t + 1;
        let in_lower = t >= upper_cut;
        if in_lower && hh < min_body && !body.is_empty() {
            break;
        }
        // 下半页虽矮但若还没有任何正文, 仍可能是整页只有脚注谱例 — 收下
        if in_lower && hh < min_body && body.is_empty() {
            body.push((t, b));
            break;
        }
        if hh >= min_body || body.is_empty() {
            body.push((t, b));
        } else if !in_lower {
            // 上半页略矮但仍可能是真谱表
            if hh >= ((med as f32 * 0.42).round() as i32).max(40) {
                body.push((t, b));
            }
        } else {
            break;
        }
    }
    if body.is_empty() {
        // 兜底: 最高的一块
        let mut all = systems;
        all.sort_by_key(|&(t, b)| std::cmp::Reverse(b - t + 1));
        all.into_iter().take(1).collect()
    } else {
        body
    }
}

pub fn detect_bands(image: &RgbImage, ink_threshold: i32, margin: i32) -> Vec<Band> {
    let threshold = ink_threshold.clamp(1, 254) as u8;
    let ink = to_ink(image, threshold);
    let h = ink.len() as i32;
    if h <= 0 {
        return Vec::new();
    }

    // 细碎核 (五线配对 + 大括号) → 再按宽松/紧密扩边界, 硬停在邻核
    let cores = collect_system_cores(&ink);
    let systems_content = system_extents(&ink, &cores, margin);
    let mut systems = filter_short_systems(systems_content.clone());
    if systems.is_empty() && !systems_content.is_empty() {
        let mut fallback = systems_content;
        fallback.sort_by_key(|&(t, b)| std::cmp::Reverse(b - t + 1));
        systems = fallback.into_iter().take(1).collect();
    }
    let mut systems = pick_body_systems(systems, h);
    clamp_systems_apart(&mut systems);

    let merge_gap = ((h as f32 * 0.012).round() as i32).max(28);
    let min_height = ((h as f32 * 0.003).round() as i32).max(6);
    let dense = dense_intervals(&ink, 0.02, min_height);

    let mut uncovered: Vec<(i32, i32)> = Vec::new();
    for &(a0, a1) in &dense {
        let mut pieces = vec![(a0, a1)];
        for &(s0, s1) in &systems {
            let mut next_pieces = Vec::new();
            for (p0, p1) in pieces {
                if !overlaps(p0, p1, s0, s1) {
                    next_pieces.push((p0, p1));
                    continue;
                }
                if p0 < s0 {
                    next_pieces.push((p0, p1.min(s0 - 1)));
                }
                if p1 > s1 {
                    next_pieces.push((p0.max(s1 + 1), p1));
                }
            }
            pieces = next_pieces
                .into_iter()
                .filter(|&(x, y)| y >= x)
                .collect();
        }
        for (p0, p1) in pieces {
            if p1 - p0 + 1 >= 8 {
                uncovered.push((p0, p1));
            }
        }
    }
    let uncovered = merge_close_intervals(&uncovered, merge_gap);

    let med_sys_h = {
        let mut hs: Vec<i32> = systems.iter().map(|(t, b)| b - t + 1).collect();
        if hs.is_empty() {
            120
        } else {
            hs.sort();
            hs[hs.len() / 2]
        }
    };
    let max_frag_h = ((med_sys_h as f32 * 0.22).round() as i32).max(18);
    let absorb_gap = margin.max(16).min(36);
    // 只吸收紧贴正文谱表的碎片; 正文底边以下留给 footer
    let body_end = systems.last().map(|s| s.1).unwrap_or(-1);
    let (near_body, below_body): (Vec<_>, Vec<_>) = uncovered
        .into_iter()
        .partition(|&(a, _)| a <= body_end);
    let mut near_body =
        absorb_fragments_into_systems(&mut systems, &near_body, absorb_gap, max_frag_h);
    clamp_systems_apart(&mut systems);
    let body_end = systems.last().map(|s| s.1).unwrap_or(-1);
    let mut footer_parts: Vec<(i32, i32)> = below_body;
    let mut header_parts = Vec::new();
    let mut gap_parts = Vec::new();
    let first_sys = systems.first().map(|s| s.0).unwrap_or(h);
    for (a, b) in near_body.drain(..) {
        if b < first_sys {
            header_parts.push((a, b));
        } else if a > body_end {
            footer_parts.push((a, b));
        } else if a >= first_sys && b <= body_end {
            gap_parts.push((a, b));
        } else if a <= body_end && b > body_end {
            if a < body_end {
                let top = (a, body_end);
                if top.0 >= first_sys {
                    gap_parts.push(top);
                }
            }
            footer_parts.push((body_end + 1, b));
        } else {
            footer_parts.push((a, b));
        }
    }
    footer_parts = merge_close_intervals(&footer_parts, merge_gap);
    // 丢掉页底版号/页码薄条; 真脚注谱例通常更高或更靠上
    footer_parts = footer_parts
        .into_iter()
        .filter(|&(a, b)| !is_page_number_band(a, b, h))
        .collect();
    header_parts = merge_close_intervals(&header_parts, merge_gap);
    gap_parts = merge_close_intervals(&gap_parts, merge_gap);

    // 谱表之间的薄 gap: 丢掉即可, 切勿并回 system (会把多行粘成一片)
    let min_gap_band = ((med_sys_h as f32 * 0.18).round() as i32).max(20);
    let kept_gaps: Vec<_> = gap_parts
        .into_iter()
        .filter(|&(a, b)| b - a + 1 >= min_gap_band)
        .collect();

    // 丢掉扫描黑边伪页眉; 真指法页眉仍可并入第一谱表
    let mut header_parts: Vec<_> = header_parts
        .into_iter()
        .filter(|&(a, b)| !is_scan_edge_band(a, b, h))
        .collect();
    if !header_parts.is_empty() && !systems.is_empty() {
        let hy0 = header_parts[0].0;
        let hy1 = header_parts[header_parts.len() - 1].1;
        let header_h = hy1 - hy0 + 1;
        let gap = systems[0].0 - hy1 - 1;
        if gap <= absorb_gap && header_h <= max_frag_h.max(med_sys_h / 4) {
            let mid = if systems.len() > 1 {
                (systems[0].1 + systems[1].0) / 2
            } else {
                i32::MAX / 4
            };
            systems[0].0 = hy0.min(systems[0].0);
            systems[0].1 = systems[0].1.min(mid);
            header_parts.clear();
            clamp_systems_apart(&mut systems);
        }
    }
    if header_parts.len() == 1 && is_scan_edge_band(header_parts[0].0, header_parts[0].1, h) {
        header_parts.clear();
    }
    if !header_parts.is_empty() {
        let hy0 = header_parts[0].0;
        let hy1 = header_parts[header_parts.len() - 1].1;
        if is_scan_edge_band(hy0, hy1, h) {
            header_parts.clear();
        }
    }

    let mut bands = Vec::new();
    if !header_parts.is_empty() {
        bands.push(Band {
            y0: header_parts[0].0,
            y1: header_parts[header_parts.len() - 1].1,
            kind: "header".into(),
        });
    }
    for &(t, b) in &systems {
        bands.push(Band {
            y0: t,
            y1: b,
            kind: "system".into(),
        });
    }
    for &(a, b) in &kept_gaps {
        let inside = systems.iter().any(|&(s0, s1)| a >= s0 && b <= s1);
        if inside {
            continue;
        }
        // 落在正文底边以下的不算 gap
        if a > systems.last().map(|s| s.1).unwrap_or(-1) {
            continue;
        }
        bands.push(Band {
            y0: a,
            y1: b,
            kind: "gap".into(),
        });
    }
    // 正文以下整段作为一块 footer (分块以后再细化; 共享加入仍可用)
    if !footer_parts.is_empty() {
        let fy0 = footer_parts[0].0;
        let fy1 = footer_parts[footer_parts.len() - 1].1;
        // 整段仍只是贴底薄条时也丢掉 (合并后只剩版号)
        if !is_page_number_band(fy0, fy1, h) {
            bands.push(Band {
                y0: fy0,
                y1: fy1,
                kind: "footer".into(),
            });
        }
    }
    bands.sort_by_key(|b| (b.y0, b.y1));
    bands
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn empty_white_page_returns_empty_or_no_systems() {
        let img = RgbImage::from_pixel(200, 300, Rgb([255, 255, 255]));
        let bands = detect_bands(&img, 200, 20);
        assert!(bands.iter().all(|b| b.kind != "system") || bands.is_empty());
    }

    #[test]
    fn horizontal_staff_lines_detected_as_system() {
        // 合成: 两行五线 (大谱表), 白底黑线
        let w = 400u32;
        let h = 600u32;
        let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
        let draw_staff = |img: &mut RgbImage, top: u32| {
            for i in 0..5u32 {
                let y = top + i * 8;
                for x in 20..380 {
                    img.put_pixel(x, y, Rgb([0, 0, 0]));
                    if y + 1 < h {
                        img.put_pixel(x, y + 1, Rgb([0, 0, 0]));
                    }
                }
            }
        };
        draw_staff(&mut img, 100);
        draw_staff(&mut img, 180);
        let bands = detect_bands(&img, 200, 20);
        assert!(
            bands.iter().any(|b| b.kind == "system"),
            "expected system band, got {bands:?}"
        );
    }

    #[test]
    fn thin_bottom_plate_number_is_not_footer() {
        assert!(is_page_number_band(780, 798, 800));
        assert!(!is_page_number_band(500, 650, 800));

        let w = 400u32;
        let h = 800u32;
        let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
        // 大谱表
        for staff_top in [120u32, 220] {
            for i in 0..5u32 {
                let y = staff_top + i * 8;
                for x in 20..380 {
                    img.put_pixel(x, y, Rgb([0, 0, 0]));
                    img.put_pixel(x, y + 1, Rgb([0, 0, 0]));
                }
            }
        }
        // 页底薄版号墨迹
        for y in (h - 18)..(h - 4) {
            for x in 80..200 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        let bands = detect_bands(&img, 200, 20);
        assert!(
            bands.iter().all(|b| b.kind != "footer"),
            "page number must not become footer, got {bands:?}"
        );
    }
}
