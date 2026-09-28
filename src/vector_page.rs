//! 矢量页: 几何分块, 坐标是「左上为原点、y 向下」的 PDF point.
//! 像素只在预览缓存和成片光栅时产生.

use std::path::PathBuf;

use image::{Rgb, RgbImage, Rgba, RgbaImage};
use mask_tool::layout::BlockAdjust;
use mask_tool::mask::MaskRect;

/// 整页扫描图覆盖率达到这个比例, 且路径/文字很少, 才走位图.
pub const SCAN_IMAGE_COVER: f32 = 0.80;
/// 少于此数的路径+文字, 配上一张铺满的图, 视为扫描件上的少量注释.
pub const SCAN_VECTOR_MAX: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageClass {
    Raster,
    Vector,
}

#[derive(Clone, Copy, Debug)]
pub struct HLine {
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxKind {
    Path,
    Text,
    Image,
}

#[derive(Clone, Copy, Debug)]
pub struct ContentBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    #[allow(dead_code)]
    pub kind: BoxKind,
}

/// 一条已套上页面矩阵的路径. 坐标左上为原点、y 向下.
/// 三次贝塞尔的两个控制点来自 PDFium 连续三个 `BezierTo`.
#[derive(Clone, Copy, Debug)]
pub enum PathCurve {
    Line {
        a: (f32, f32),
        b: (f32, f32),
    },
    Cubic {
        p0: (f32, f32),
        p1: (f32, f32),
        p2: (f32, f32),
        p3: (f32, f32),
    },
}

#[derive(Clone, Debug, Default)]
pub struct VectorScene {
    pub w: f32,
    pub h: f32,
    pub hlines: Vec<HLine>,
    pub boxes: Vec<ContentBox>,
    pub curves: Vec<PathCurve>,
    /// 最大内嵌图面积 / 页面积.
    pub image_cover: f32,
    pub path_count: u32,
    pub text_count: u32,
}

/// 一页矢量来源. 工程里嵌 PDF, 不嵌整页 PNG.
#[derive(Clone, Debug)]
pub struct VectorSource {
    pub pdf_path: PathBuf,
    pub page_index: u32,
    pub w_pt: f32,
    pub h_pt: f32,
    pub scene: VectorScene,
}

impl VectorSource {
    pub fn logical_size(&self) -> (u32, u32) {
        (
            self.w_pt.round().max(1.0) as u32,
            self.h_pt.round().max(1.0) as u32,
        )
    }
}

#[derive(Clone, Debug)]
pub struct StaffBand {
    pub y0: i32,
    pub y1: i32,
    /// 相对条带顶的对齐锚点.
    pub anchor: Option<i32>,
}

pub fn classify(scene: &VectorScene) -> PageClass {
    let vectors = scene.path_count.saturating_add(scene.text_count);
    if scene.image_cover >= SCAN_IMAGE_COVER && vectors < SCAN_VECTOR_MAX {
        return PageClass::Raster;
    }
    if vectors == 0 {
        return PageClass::Raster;
    }
    PageClass::Vector
}

/// 长水平线按 y 收成五线, 再按包围盒是否把相邻谱表连起来收成谱行.
/// 认不出谱线时返回空, 调用方收成整页一块.
pub fn detect_vector_bands(scene: &VectorScene, margin: i32) -> Vec<StaffBand> {
    let page_w = scene.w.max(1.0);
    let page_h = scene.h.max(1.0);
    let min_len = page_w * 0.30;
    let lines = coalesce_hlines(&scene.hlines, min_len);
    let clusters = cluster_ys(&lines, 1.2);
    let staves = group_staves(&clusters);
    if staves.is_empty() {
        return Vec::new();
    }
    let content = connectivity_boxes(scene);
    let mut systems: Vec<(f32, f32, Vec<StaffCore>)> = Vec::new();
    for staff in staves {
        let join = systems.last().is_some_and(|(_, bot, prev)| {
            let gap = staff.top - *bot;
            let spacing = prev.last().map(|s| s.spacing).unwrap_or(8.0);
            let staff_h = prev
                .last()
                .map(|s| s.bottom - s.top)
                .unwrap_or(spacing * 4.0);
            let limit = (spacing * 12.0).max(staff_h * 3.0);
            let close = gap >= -1.0 && gap < staff_h * 2.6;
            gap >= -1.0 && gap < limit && (close || gap_has_content(&content, *bot, staff.top))
        });
        if join {
            let last = systems.last_mut().unwrap();
            last.1 = staff.bottom;
            last.2.push(staff);
        } else {
            systems.push((staff.top, staff.bottom, vec![staff]));
        }
    }
    let margin = margin.max(0) as f32;
    let mut raw: Vec<(f32, f32, Vec<StaffCore>)> = systems
        .into_iter()
        .map(|(t, b, cores)| {
            let spacing = cores.first().map(|s| s.spacing).unwrap_or(6.0);
            let pad = margin.max(spacing * 5.0);
            ((t - pad).max(0.0), (b + pad).min(page_h - 1.0), cores)
        })
        .filter(|(t, b, _)| b > t)
        .collect();
    separate_bands(&mut raw);
    raw.into_iter()
        .map(|(t, b, cores)| {
            let y0 = t.round() as i32;
            let y1 = b.round().max(t) as i32;
            let anchor = band_anchor(scene, &cores, y0, y1);
            StaffBand { y0, y1, anchor }
        })
        .collect()
}

#[derive(Clone, Debug)]
struct StaffCore {
    top: f32,
    bottom: f32,
    spacing: f32,
    lines: Vec<f32>,
}

fn coalesce_hlines(lines: &[HLine], min_len: f32) -> Vec<HLine> {
    let mut buckets: Vec<(f32, Vec<(f32, f32)>)> = Vec::new();
    for line in lines {
        let (x0, x1) = if line.x0 <= line.x1 {
            (line.x0, line.x1)
        } else {
            (line.x1, line.x0)
        };
        if x1 - x0 < 4.0 {
            continue;
        }
        if let Some((y, spans)) = buckets.iter_mut().find(|(y, _)| (line.y - *y).abs() <= 0.6) {
            *y = (*y + line.y) * 0.5;
            spans.push((x0, x1));
        } else {
            buckets.push((line.y, vec![(x0, x1)]));
        }
    }
    let mut out = Vec::new();
    for (y, spans) in buckets {
        let covered = union_len(&spans);
        if covered >= min_len {
            let x0 = spans.iter().map(|s| s.0).fold(f32::MAX, f32::min);
            let x1 = spans.iter().map(|s| s.1).fold(f32::MIN, f32::max);
            out.push(HLine { x0, x1, y });
        }
    }
    out.sort_by(|a, b| a.y.total_cmp(&b.y));
    out
}

fn union_len(spans: &[(f32, f32)]) -> f32 {
    let mut s = spans.to_vec();
    s.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0f32;
    let mut cur: Option<(f32, f32)> = None;
    for (a, b) in s {
        if let Some((c0, c1)) = cur {
            if a <= c1 + 1.0 {
                cur = Some((c0, c1.max(b)));
            } else {
                total += c1 - c0;
                cur = Some((a, b));
            }
        } else {
            cur = Some((a, b));
        }
    }
    if let Some((c0, c1)) = cur {
        total += c1 - c0;
    }
    total
}

fn cluster_ys(lines: &[HLine], gap: f32) -> Vec<f32> {
    let mut ys: Vec<f32> = lines.iter().map(|l| l.y).collect();
    ys.sort_by(|a, b| a.total_cmp(b));
    let mut out = Vec::new();
    let mut acc: Vec<f32> = Vec::new();
    for y in ys {
        if acc.last().is_some_and(|p| y - *p > gap) {
            out.push(mean(&acc));
            acc.clear();
        }
        acc.push(y);
    }
    if !acc.is_empty() {
        out.push(mean(&acc));
    }
    out
}

fn mean(xs: &[f32]) -> f32 {
    xs.iter().sum::<f32>() / xs.len().max(1) as f32
}

fn group_staves(ys: &[f32]) -> Vec<StaffCore> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 < ys.len() {
        let window = &ys[i..i + 5];
        let gaps: Vec<f32> = window.windows(2).map(|w| w[1] - w[0]).collect();
        let gmin = gaps.iter().copied().fold(f32::MAX, f32::min);
        let gmax = gaps.iter().copied().fold(0.0f32, f32::max);
        if (3.0..=16.0).contains(&gmin) && gmax <= gmin * 1.45 + 0.8 {
            let spacing = mean(&gaps);
            let mut bottom = window[4];
            let mut lines = window.to_vec();
            let mut j = i + 5;
            while j < ys.len() && lines.len() < 9 {
                let step = ys[j] - *lines.last().unwrap();
                if step < spacing * 0.75 || step > spacing * 1.45 {
                    break;
                }
                lines.push(ys[j]);
                bottom = ys[j];
                j += 1;
            }
            out.push(StaffCore {
                top: window[0],
                bottom,
                spacing,
                lines,
            });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn connectivity_boxes(scene: &VectorScene) -> Vec<ContentBox> {
    scene
        .boxes
        .iter()
        .copied()
        .filter(|b| !is_page_border(scene, b))
        .collect()
}

fn is_page_border(scene: &VectorScene, b: &ContentBox) -> bool {
    let w = (b.x1 - b.x0).abs();
    let h = (b.y1 - b.y0).abs();
    let touch = b.x0 <= 1.5 || b.y0 <= 1.5 || b.x1 >= scene.w - 1.5 || b.y1 >= scene.h - 1.5;
    touch && (h > scene.h * 0.85 || w < 2.0 && h > scene.h * 0.5)
}

fn gap_has_content(boxes: &[ContentBox], bot: f32, top: f32) -> bool {
    let y0 = bot - 2.0;
    let y1 = top + 2.0;
    boxes
        .iter()
        .any(|b| b.y1 >= y0 && b.y0 <= y1 && b.y1 > b.y0)
}

fn separate_bands(bands: &mut Vec<(f32, f32, Vec<StaffCore>)>) {
    for i in 1..bands.len() {
        if bands[i].0 < bands[i - 1].1 {
            let mid = (bands[i - 1].1 + bands[i].0) * 0.5;
            bands[i - 1].1 = mid;
            bands[i].0 = mid + 0.5;
        }
    }
}

const LEFT_BAND_MIN_FRAC: f32 = 0.004;
const LEFT_BAND_MAX_FRAC: f32 = 0.14;
const CUSP_MIN_SEP_FRAC: f32 = 0.12;
const BRACE_REACH_SLACK: f32 = 0.45;
const BRACE_BORDER_FRAC: f32 = 0.92;
const BRACE_BORDER_MARGIN: f32 = 8.0;

fn band_anchor(scene: &VectorScene, cores: &[StaffCore], y0: i32, y1: i32) -> Option<i32> {
    let first = cores.first()?;
    let last = cores.last().unwrap();
    let rel = |y: f32| ((y - y0 as f32).round() as i32).clamp(0, (y1 - y0).max(0));
    let staff_top = first.lines[0];
    let staff_bot = *last.lines.last().unwrap();
    let centroid = (staff_top + staff_bot) * 0.5;
    // 一条带已经是一个谱行组 (钢琴的高低音在同一组里). 位图只在
    // 「一块里有多组」时改用联合重心; 这里始终按单组处理.
    if let Some(full) = left_span(scene, y0 as f32, y1 as f32) {
        let bh = (y1 - y0).max(1) as f32;
        let margin = staff_top - y0 as f32 >= BRACE_BORDER_MARGIN
            || y1 as f32 - staff_bot >= BRACE_BORDER_MARGIN;
        // 通页竖线没有向左的尖. 包住全部谱行的括号即使几乎撑满条带,
        // 仍然要对准尖, 不能改用谱表重心.
        if (full.1 - full.0) / bh >= BRACE_BORDER_FRAC
            && margin
            && brace_cusp_y(scene, full.0, full.1).is_none()
        {
            return Some(rel(centroid));
        }
    }
    let staff_h = (first.bottom - first.top).max(1.0);
    let pad = (staff_h * 0.20).max(4.0);
    let scan0 = (first.top - pad).max(y0 as f32);
    let scan1 = (last.bottom + pad)
        .max(first.top + staff_h * 8.0)
        .min(y1 as f32);
    let staves: Vec<(f32, f32)> = cores.iter().map(|c| (c.top, c.bottom)).collect();
    let max_gap = (staff_h * 0.5).max(4.0);
    if let Some(brace) = brace_cluster(scene, scan0, scan1, first.top, first.bottom, max_gap) {
        let slack = (staff_h * BRACE_REACH_SLACK).max(4.0);
        let brace_h = brace.1 - brace.0;
        if brace_h > staff_h * 2.0 {
            let system: Vec<(f32, f32)> = staves
                .iter()
                .copied()
                .filter(|s| s.0 <= brace.1 + slack)
                .collect();
            if !system.is_empty()
                && brace_reaches(brace, system[0])
                && (system.len() == 1 || brace_covers_ends(brace, &system))
            {
                return Some(rel(cusp_or_mid(scene, brace.0, brace.1)));
            }
            if let (Some(a), Some(b)) = (system.first(), system.last()) {
                return Some(rel((a.0 + b.1) * 0.5));
            }
        }
        if brace_covers_ends(brace, &staves) {
            return Some(rel(cusp_or_mid(scene, brace.0, brace.1)));
        }
    }
    Some(rel(centroid))
}

fn brace_reaches(brace: (f32, f32), staff: (f32, f32)) -> bool {
    let slack = ((staff.1 - staff.0).max(1.0) * BRACE_REACH_SLACK).max(4.0);
    brace.1 >= staff.0 - slack && brace.0 <= staff.1 + slack
}

fn brace_covers_ends(brace: (f32, f32), staves: &[(f32, f32)]) -> bool {
    let Some(&first) = staves.first() else {
        return false;
    };
    let Some(&last) = staves.last() else {
        return false;
    };
    brace_reaches(brace, first) && brace_reaches(brace, last)
}

fn left_x_lim(scene: &VectorScene) -> (f32, f32) {
    let w = scene.w.max(1.0);
    (w * LEFT_BAND_MIN_FRAC, w * LEFT_BAND_MAX_FRAC)
}

fn curve_in_left(scene: &VectorScene, c: &PathCurve, y0: f32, y1: f32) -> bool {
    let (x_min, x_max) = left_x_lim(scene);
    let (a, b) = curve_bbox(c);
    if b.1 < y0 || a.1 > y1 {
        return false;
    }
    if a.0 > x_max || b.0 < x_min {
        return false;
    }
    let h = b.1 - a.1;
    let w = b.0 - a.0;
    !(h > scene.h * 0.85 || (w < 1.5 && h > scene.h * 0.5))
}

fn curve_bbox(c: &PathCurve) -> ((f32, f32), (f32, f32)) {
    let pts: Vec<(f32, f32)> = match *c {
        PathCurve::Line { a, b } => vec![a, b],
        PathCurve::Cubic { p0, p1, p2, p3 } => vec![p0, p1, p2, p3],
    };
    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let x1 = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let y1 = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    ((x0, y0), (x1, y1))
}

fn left_span(scene: &VectorScene, y0: f32, y1: f32) -> Option<(f32, f32)> {
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for c in &scene.curves {
        if !curve_in_left(scene, c, y0, y1) {
            continue;
        }
        let (a, b) = curve_bbox(c);
        lo = lo.min(a.1.max(y0));
        hi = hi.max(b.1.min(y1));
    }
    (hi > lo).then_some((lo, hi))
}

/// 覆盖种子谱表的那一截左侧路径. 空隙超过 `max_gap` 的脚注竖线另成一簇.
fn brace_cluster(
    scene: &VectorScene,
    scan0: f32,
    scan1: f32,
    seed0: f32,
    seed1: f32,
    max_gap: f32,
) -> Option<(f32, f32)> {
    let mut spans = Vec::new();
    for c in &scene.curves {
        if !curve_in_left(scene, c, scan0, scan1) {
            continue;
        }
        let (a, b) = curve_bbox(c);
        let lo = a.1.max(scan0);
        let hi = b.1.min(scan1);
        if hi > lo {
            spans.push((lo, hi));
        }
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (lo, hi) in spans {
        if let Some(last) = merged.last_mut() {
            if lo <= last.1 + max_gap.max(0.0) {
                last.1 = last.1.max(hi);
                continue;
            }
        }
        merged.push((lo, hi));
    }
    let seed_mid = (seed0 + seed1) * 0.5;
    let run = merged.into_iter().find(|(lo, hi)| {
        seed_mid >= *lo - max_gap && seed_mid <= *hi + max_gap && *hi >= seed0 && *lo <= seed1
    })?;
    let h = run.1 - run.0;
    if h < (scan1 - scan0).max(1.0) * 0.10 {
        return None;
    }
    Some(run)
}

fn cusp_or_mid(scene: &VectorScene, y0: f32, y1: f32) -> f32 {
    brace_cusp_y(scene, y0, y1).unwrap_or((y0 + y1) * 0.5)
}

/// `{` 的尖: 左侧路径上向左的极值点, 三个取中间, 一个且在中段也用.
/// 上下两瓣常常比尖更靠左, 所以不取全局最左. y 是极值点自己的坐标.
fn brace_cusp_y(scene: &VectorScene, y0: f32, y1: f32) -> Option<f32> {
    if y1 - y0 < 8.0 {
        return None;
    }
    let (x_min, x_max) = left_x_lim(scene);
    let left: Vec<&PathCurve> = scene
        .curves
        .iter()
        .filter(|c| curve_in_left(scene, c, y0, y1))
        .collect();
    let mut mins: Vec<(f32, f32)> = Vec::new();
    for c in &left {
        for (x, y) in curve_x_minima(c) {
            if y < y0 || y > y1 || x < x_min - 1.0 || x > x_max + 1.0 {
                continue;
            }
            mins.push((y, x));
        }
    }
    // 尖常常是两段曲线相接的折点, 不是任何一段内部的 dx/dt = 0.
    // 上下两瓣是内部极值, 折点漏掉就只剩两峰, 只能退回括号中点.
    let owned: Vec<PathCurve> = left.into_iter().copied().collect();
    for (x, y) in join_x_minima(&owned) {
        if y < y0 || y > y1 || x < x_min - 1.0 || x > x_max + 1.0 {
            continue;
        }
        mins.push((y, x));
    }
    if mins.is_empty() {
        return None;
    }
    mins.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut compact: Vec<(f32, f32)> = Vec::new();
    for (y, x) in mins {
        if let Some(last) = compact.last_mut() {
            if (y - last.0).abs() < 0.8 {
                if x < last.1 {
                    *last = (y, x);
                }
                continue;
            }
        }
        compact.push((y, x));
    }
    let min_sep = ((y1 - y0) * CUSP_MIN_SEP_FRAC).max(4.0);
    let mut order: Vec<usize> = (0..compact.len()).collect();
    order.sort_by(|&i, &j| compact[i].1.total_cmp(&compact[j].1));
    let mut kept = Vec::new();
    for i in order {
        if kept
            .iter()
            .any(|&k: &usize| (compact[k].0 - compact[i].0).abs() < min_sep)
        {
            continue;
        }
        kept.push(i);
    }
    kept.sort_by(|&i, &j| compact[i].0.total_cmp(&compact[j].0));
    // 谱号、小节线也在左侧窄条里, 会多出离括号很远的峰. 只留靠近最左
    // 缘的那些, 否则峰数不是 3 就退回中点, 线和尖对不齐.
    let min_x = kept.iter().map(|&i| compact[i].1).fold(f32::MAX, f32::min);
    let x_slack = (scene.w * 0.045).max(14.0);
    kept.retain(|&i| compact[i].1 <= min_x + x_slack);
    kept.sort_by(|&i, &j| compact[i].0.total_cmp(&compact[j].0));
    let in_middle = |i: usize| {
        let frac = (compact[i].0 - y0) / (y1 - y0).max(1.0);
        (0.28..=0.72).contains(&frac)
    };
    let pick = if kept.len() == 3 {
        kept[1]
    } else if kept.len() == 1 {
        if in_middle(kept[0]) {
            kept[0]
        } else {
            return None;
        }
    } else {
        // 尖本身最靠左、又在中段 (上下瓣没有更靠左) 时直接用它.
        let leftmost = kept
            .iter()
            .copied()
            .min_by(|&i, &j| compact[i].1.total_cmp(&compact[j].1))?;
        if in_middle(leftmost) {
            leftmost
        } else {
            return None;
        }
    };
    Some(compact[pick].0)
}

fn curve_ends(c: &PathCurve) -> ((f32, f32), (f32, f32)) {
    match *c {
        PathCurve::Line { a, b } => (a, b),
        PathCurve::Cubic { p0, p3, .. } => (p0, p3),
    }
}

/// 沿路径离开起点 / 到达终点时的 dx. 正值表示往右走.
fn curve_dx_leave(c: &PathCurve) -> f32 {
    match *c {
        PathCurve::Line { a, b } => b.0 - a.0,
        PathCurve::Cubic { p0, p1, .. } => p1.0 - p0.0,
    }
}

fn curve_dx_arrive(c: &PathCurve) -> f32 {
    match *c {
        PathCurve::Line { a, b } => b.0 - a.0,
        PathCurve::Cubic { p2, p3, .. } => p3.0 - p2.0,
    }
}

/// 两段相接、并且都朝右离开的顶点是向左的尖. 大括号包住全部谱行时,
/// 尖通常就是这样一个折点, 两瓣才是曲线内部的极值.
fn join_x_minima(curves: &[PathCurve]) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    for (i, incoming) in curves.iter().enumerate() {
        let end = curve_ends(incoming).1;
        if curve_dx_arrive(incoming) >= -0.15 {
            continue;
        }
        for (j, outgoing) in curves.iter().enumerate() {
            if i == j {
                continue;
            }
            let start = curve_ends(outgoing).0;
            if (start.0 - end.0).abs() > 0.4 || (start.1 - end.1).abs() > 0.4 {
                continue;
            }
            if curve_dx_leave(outgoing) <= 0.15 {
                continue;
            }
            out.push(end);
        }
    }
    out
}

/// 一条路径上 x 的局部极小: 折线的左端点, 三次曲线 `dx/dt = 0` 且两侧更靠右的点.
fn curve_x_minima(c: &PathCurve) -> Vec<(f32, f32)> {
    match *c {
        PathCurve::Line { a, b } => {
            if (a.1 - b.1).abs() < 0.5 || (a.0 - b.0).abs() < 0.4 {
                return Vec::new();
            }
            let left = if a.0 <= b.0 { a } else { b };
            vec![left]
        }
        PathCurve::Cubic { p0, p1, p2, p3 } => {
            let mut out = Vec::new();
            for t in cubic_dx_roots(p0, p1, p2, p3) {
                let x = cubic_at(p0, p1, p2, p3, t).0;
                let xl = cubic_at(p0, p1, p2, p3, (t - 0.03).clamp(0.0, 1.0)).0;
                let xr = cubic_at(p0, p1, p2, p3, (t + 0.03).clamp(0.0, 1.0)).0;
                if x <= xl + 0.05 && x <= xr + 0.05 && (xl - x).max(xr - x) > 0.15 {
                    out.push(cubic_at(p0, p1, p2, p3, t));
                }
            }
            out
        }
    }
}

fn cubic_at(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let b0 = u * u * u;
    let b1 = 3.0 * u * u * t;
    let b2 = 3.0 * u * t * t;
    let b3 = t * t * t;
    (
        b0 * p0.0 + b1 * p1.0 + b2 * p2.0 + b3 * p3.0,
        b0 * p0.1 + b1 * p1.1 + b2 * p2.1 + b3 * p3.1,
    )
}

fn cubic_dx_roots(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) -> Vec<f32> {
    let ax = p1.0 - p0.0;
    let bx = p2.0 - p1.0;
    let cx = p3.0 - p2.0;
    let a = ax - 2.0 * bx + cx;
    let b = 2.0 * (bx - ax);
    let c = ax;
    let mut out = Vec::new();
    let push = |t: f32, out: &mut Vec<f32>| {
        if (0.0..=1.0).contains(&t) {
            out.push(t);
        }
    };
    if a.abs() < 1e-6 {
        if b.abs() > 1e-6 {
            push(-c / b, &mut out);
        }
        return out;
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return out;
    }
    let s = disc.sqrt();
    push((-b - s) / (2.0 * a), &mut out);
    push((-b + s) / (2.0 * a), &mut out);
    out
}

pub fn scale_i32(v: i32, scale: f32) -> i32 {
    (v as f32 * scale).round() as i32
}

pub fn scale_mask(mask: &MaskRect, scale: f32) -> MaskRect {
    let mut m = mask.clone();
    m.map_scale(scale, 0.0, 0.0);
    m
}

pub fn scale_adjust(adj: &BlockAdjust, scale: f32) -> BlockAdjust {
    BlockAdjust {
        region_id: adj.region_id.clone(),
        extra_top: scale_i32(adj.extra_top, scale),
        extra_bottom: scale_i32(adj.extra_bottom, scale),
        extra_left: scale_i32(adj.extra_left, scale),
        extra_right: scale_i32(adj.extra_right, scale),
        gap_before: scale_i32(adj.gap_before, scale),
        gap_after: scale_i32(adj.gap_after, scale),
        shift_x: scale_i32(adj.shift_x, scale),
    }
}

/// 成片里矢量谱面的像素 / point. 约 1600px 宽, 夹在 2 和 8 之间.
pub fn export_px_per_pt(page_w_pt: f32) -> f32 {
    (1600.0 / page_w_pt.max(1.0)).clamp(2.0, 8.0)
}

/// 不透明像素偏亮 (白墨) 时垫深灰, 否则垫白. 只用于没有底色的预览.
pub fn polarity_pad(src: &RgbaImage) -> [u8; 3] {
    let raw = src.as_raw();
    let n_px = raw.len() / 4;
    if n_px == 0 {
        return [255, 255, 255];
    }
    let step = (n_px / 4000).max(1);
    let mut sum = 0u64;
    let mut n = 0u64;
    let mut i = 0;
    while i < n_px {
        let o = i * 4;
        let a = raw[o + 3];
        if a > 200 {
            let r = raw[o] as u64;
            let g = raw[o + 1] as u64;
            let b = raw[o + 2] as u64;
            sum += (r * 30 + g * 59 + b * 11) / 100;
            n += 1;
        }
        i += step;
    }
    if n == 0 || sum / n > 180 {
        [0x2b, 0x2b, 0x2b]
    } else {
        [255, 255, 255]
    }
}

pub fn flatten_rgba(src: &RgbaImage, pad: [u8; 3]) -> RgbImage {
    let mut out = RgbImage::new(src.width(), src.height());
    for (d, s) in out.pixels_mut().zip(src.pixels()) {
        let a = s[3] as f32 / 255.0;
        *d = Rgb([
            (s[0] as f32 * a + pad[0] as f32 * (1.0 - a)).round() as u8,
            (s[1] as f32 * a + pad[1] as f32 * (1.0 - a)).round() as u8,
            (s[2] as f32 * a + pad[2] as f32 * (1.0 - a)).round() as u8,
        ]);
    }
    out
}

/// 把蒙版盖进 RGBA: 蒙版处提高不透明度并叠色, 其余 alpha 不动.
pub fn paint_masks_rgba(sheet: &mut RgbaImage, masks: &[MaskRect]) {
    if masks.is_empty() || sheet.width() == 0 {
        return;
    }
    let (w, h) = sheet.dimensions();
    let mut on_black = RgbImage::from_pixel(w, h, Rgb([0, 0, 0]));
    let mut on_white = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
    mask_tool::mask::apply_masks_rgb(&mut on_black, masks, 1.0);
    mask_tool::mask::apply_masks_rgb(&mut on_white, masks, 1.0);
    for y in 0..h {
        for x in 0..w {
            let b = on_black.get_pixel(x, y).0;
            let ww = on_white.get_pixel(x, y).0;
            let mut cover = 0.0f32;
            for i in 0..3 {
                let diff = ww[i] as i32 - b[i] as i32;
                if diff < 250 {
                    cover = cover.max((255 - diff) as f32 / 255.0);
                }
            }
            if cover < 0.004 {
                continue;
            }
            let px = sheet.get_pixel_mut(x, y);
            let src_a = px[3] as f32 / 255.0;
            let out_a = src_a + (1.0 - src_a) * cover;
            let inv = 1.0 - cover;
            let rgb = if cover > 0.001 {
                [
                    (b[0] as f32 / cover).clamp(0.0, 255.0),
                    (b[1] as f32 / cover).clamp(0.0, 255.0),
                    (b[2] as f32 / cover).clamp(0.0, 255.0),
                ]
            } else {
                [255.0, 255.0, 255.0]
            };
            *px = Rgba([
                (px[0] as f32 * inv + rgb[0] * cover).round() as u8,
                (px[1] as f32 * inv + rgb[1] * cover).round() as u8,
                (px[2] as f32 * inv + rgb[2] * cover).round() as u8,
                (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
            ]);
        }
    }
}

pub fn blend_rgba_into(dst: &mut RgbImage, src: &RgbaImage, ox: i64, oy: i64) {
    let (dw, dh) = dst.dimensions();
    let (sw, sh) = src.dimensions();
    for y in 0..sh {
        let dy = oy + y as i64;
        if dy < 0 || dy >= dh as i64 {
            continue;
        }
        for x in 0..sw {
            let dx = ox + x as i64;
            if dx < 0 || dx >= dw as i64 {
                continue;
            }
            let s = src.get_pixel(x, y).0;
            let a = s[3] as f32 / 255.0;
            if a < 0.004 {
                continue;
            }
            let d = dst.get_pixel_mut(dx as u32, dy as u32);
            let inv = 1.0 - a;
            d[0] = (s[0] as f32 * a + d[0] as f32 * inv).round() as u8;
            d[1] = (s[1] as f32 * a + d[1] as f32 * inv).round() as u8;
            d[2] = (s[2] as f32 * a + d[2] as f32 * inv).round() as u8;
        }
    }
}

/// 竖向拼 RGBA 条带. `layout` 已经是像素. 间隙保持透明.
pub fn stitch_rgba(parts: &[(String, RgbaImage)], layout: &[BlockAdjust]) -> Option<RgbaImage> {
    if parts.is_empty() {
        return None;
    }
    let width = parts
        .iter()
        .map(|(_, i)| i.width())
        .max()
        .unwrap_or(1)
        .max(1);
    let mut rows: Vec<(u32, u32, u32)> = Vec::new();
    let mut y = 0u32;
    for (i, (rid, img)) in parts.iter().enumerate() {
        let adj = BlockAdjust::find(layout, rid);
        let gap = adj.map(|a| a.gap_before.max(0) as u32).unwrap_or(0);
        let extra_top = adj.map(|a| a.extra_top).unwrap_or(0);
        let extra_bot = adj.map(|a| a.extra_bottom).unwrap_or(0);
        y = y.saturating_add(gap);
        let h = img.height() as i32 + extra_top + extra_bot;
        let h = h.max(1) as u32;
        rows.push((y, h, i as u32));
        y = y.saturating_add(h);
        if i + 1 == parts.len() {
            y = y.saturating_add(adj.map(|a| a.gap_after.max(0) as u32).unwrap_or(0));
        }
    }
    let mut out = RgbaImage::from_pixel(width, y.max(1), Rgba([0, 0, 0, 0]));
    for (top, _h, idx) in rows {
        let (rid, img) = &parts[idx as usize];
        let adj = BlockAdjust::find(layout, rid);
        let shift = adj.map(|a| a.shift_x).unwrap_or(0);
        let extra_top = adj.map(|a| a.extra_top).unwrap_or(0);
        let dest_y = top as i32 + extra_top.max(0);
        blit_rgba(&mut out, img, shift, dest_y);
    }
    Some(out)
}

fn blit_rgba(dst: &mut RgbaImage, src: &RgbaImage, ox: i32, oy: i32) {
    for y in 0..src.height() {
        let dy = oy + y as i32;
        if dy < 0 || dy >= dst.height() as i32 {
            continue;
        }
        for x in 0..src.width() {
            let dx = ox + x as i32;
            if dx < 0 || dx >= dst.width() as i32 {
                continue;
            }
            *dst.get_pixel_mut(dx as u32, dy as u32) = *src.get_pixel(x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(y: f32) -> HLine {
        HLine {
            x0: 30.0,
            x1: 370.0,
            y,
        }
    }

    fn staff(top: f32) -> Vec<HLine> {
        (0..5).map(|i| line(top + i as f32 * 8.0)).collect()
    }

    fn scene_two_systems() -> VectorScene {
        let mut hlines = staff(100.0);
        hlines.extend(staff(180.0));
        hlines.extend(staff(400.0));
        VectorScene {
            w: 400.0,
            h: 600.0,
            hlines,
            boxes: vec![ContentBox {
                x0: 40.0,
                y0: 130.0,
                x1: 80.0,
                y1: 185.0,
                kind: BoxKind::Text,
            }],
            curves: Vec::new(),
            image_cover: 0.0,
            path_count: 20,
            text_count: 4,
        }
    }

    fn staff_lines(top: f32) -> Vec<f32> {
        (0..5).map(|i| top + i as f32 * 8.0).collect()
    }

    fn one_staff_scene(curves: Vec<PathCurve>) -> (VectorScene, StaffCore) {
        let lines = staff_lines(100.0);
        let core = StaffCore {
            top: lines[0],
            bottom: *lines.last().unwrap(),
            spacing: 8.0,
            lines,
        };
        let scene = VectorScene {
            w: 400.0,
            h: 600.0,
            curves,
            path_count: 4,
            ..VectorScene::default()
        };
        (scene, core)
    }

    #[test]
    fn full_page_image_with_few_paths_is_raster() {
        let scene = VectorScene {
            w: 100.0,
            h: 100.0,
            image_cover: 0.92,
            path_count: 3,
            text_count: 0,
            ..VectorScene::default()
        };
        assert_eq!(classify(&scene), PageClass::Raster);
    }

    #[test]
    fn vector_with_corner_image_stays_vector() {
        let scene = VectorScene {
            w: 100.0,
            h: 100.0,
            image_cover: 0.12,
            path_count: 40,
            text_count: 2,
            ..VectorScene::default()
        };
        assert_eq!(classify(&scene), PageClass::Vector);
    }

    #[test]
    fn no_paths_or_text_is_raster() {
        let scene = VectorScene {
            w: 100.0,
            h: 100.0,
            image_cover: 0.0,
            ..VectorScene::default()
        };
        assert_eq!(classify(&scene), PageClass::Raster);
    }

    #[test]
    fn geometric_bands_split_unconnected_staves() {
        let bands = detect_vector_bands(&scene_two_systems(), 8);
        assert_eq!(bands.len(), 2, "{bands:?}");
        assert!(bands[0].y0 < 100 && bands[0].y1 > 210, "{:?}", bands[0]);
        assert!(
            bands[0].y1 < 300,
            "should not swallow the lower staff {:?}",
            bands[0]
        );
        assert!(bands[1].y0 > 350 && bands[1].y1 > 430, "{:?}", bands[1]);
    }

    #[test]
    fn mask_rect_scales_in_points() {
        let m = MaskRect {
            id: "m".into(),
            x0: 10,
            y0: 20,
            x1: 30,
            y1: 40,
            brush_points: vec![],
            brush_radius: 0.0,
            color: [255, 255, 255],
            poly_points: vec![],
            opacity: 1.0,
            bound_block: None,
        };
        let s = scale_mask(&m, 2.0);
        assert_eq!((s.x0, s.y0, s.x1, s.y1), (20, 40, 60, 80));
    }

    #[test]
    fn brace_with_outer_lobes_uses_middle_cusp() {
        // 上下瓣 x=14, 比尖 x=20 更靠左. 肩在 x=26, 所以尖仍是中间的向左凸点.
        let pts = [
            (30.0, 100.0),
            (14.0, 112.0),
            (26.0, 122.0),
            (20.0, 132.0),
            (26.0, 142.0),
            (14.0, 152.0),
            (30.0, 164.0),
        ];
        let curves: Vec<PathCurve> = pts
            .windows(2)
            .map(|w| PathCurve::Line { a: w[0], b: w[1] })
            .collect();
        let (scene, core) = one_staff_scene(curves);
        let y = band_anchor(&scene, &[core], 70, 230).unwrap();
        assert!((y - (132 - 70)).abs() <= 1, "cusp y rel {y}");
    }

    #[test]
    fn leftmost_vertex_is_the_cusp_when_it_sticks_out() {
        let curves = vec![
            PathCurve::Line {
                a: (30.0, 100.0),
                b: (18.0, 132.0),
            },
            PathCurve::Line {
                a: (18.0, 132.0),
                b: (30.0, 164.0),
            },
        ];
        let (scene, core) = one_staff_scene(curves);
        let y = band_anchor(&scene, &[core], 80, 190).unwrap();
        assert!((y - (132 - 80)).abs() <= 1, "tip y rel {y}");
    }

    #[test]
    fn page_border_uses_staff_centroid_not_the_rule() {
        let curves = vec![PathCurve::Line {
            a: (8.0, 40.0),
            b: (8.0, 560.0),
        }];
        let (scene, core) = one_staff_scene(curves);
        let y = band_anchor(&scene, &[core], 20, 580).unwrap();
        let mid = ((100.0_f32 + 132.0) * 0.5 - 20.0).round() as i32;
        assert_eq!(y, mid);
    }

    #[test]
    fn two_staves_without_brace_use_combined_centroid() {
        let hi = staff_lines(100.0);
        let lo = staff_lines(180.0);
        let cores = vec![
            StaffCore {
                top: hi[0],
                bottom: *hi.last().unwrap(),
                spacing: 8.0,
                lines: hi,
            },
            StaffCore {
                top: lo[0],
                bottom: *lo.last().unwrap(),
                spacing: 8.0,
                lines: lo,
            },
        ];
        let scene = VectorScene {
            w: 400.0,
            h: 600.0,
            path_count: 10,
            ..VectorScene::default()
        };
        let y = band_anchor(&scene, &cores, 60, 260).unwrap();
        let mid = ((100.0_f32 + 212.0) * 0.5 - 60.0).round() as i32;
        assert_eq!(y, mid);
    }

    fn piano_cores() -> Vec<StaffCore> {
        let hi = staff_lines(100.0);
        let lo = staff_lines(160.0);
        vec![
            StaffCore {
                top: hi[0],
                bottom: *hi.last().unwrap(),
                spacing: 8.0,
                lines: hi,
            },
            StaffCore {
                top: lo[0],
                bottom: *lo.last().unwrap(),
                spacing: 8.0,
                lines: lo,
            },
        ]
    }

    /// 包住高低音两行的括号: 上下瓣是三次曲线的内部极值, 尖是两段相接的折点,
    /// 而且不在括号包围盒中点上.
    fn brace_covering_both_staves() -> Vec<PathCurve> {
        vec![
            PathCurve::Cubic {
                p0: (30.0, 96.0),
                p1: (12.0, 102.0),
                p2: (12.0, 114.0),
                p3: (28.0, 120.0),
            },
            PathCurve::Cubic {
                p0: (28.0, 120.0),
                p1: (30.0, 123.0),
                p2: (26.0, 126.0),
                p3: (20.0, 128.0),
            },
            PathCurve::Cubic {
                p0: (20.0, 128.0),
                p1: (26.0, 132.0),
                p2: (30.0, 140.0),
                p3: (28.0, 148.0),
            },
            PathCurve::Cubic {
                p0: (28.0, 148.0),
                p1: (12.0, 158.0),
                p2: (12.0, 176.0),
                p3: (30.0, 186.0),
            },
        ]
    }

    #[test]
    fn brace_over_all_staves_uses_corner_cusp_not_extent_midpoint() {
        let scene = VectorScene {
            w: 400.0,
            h: 600.0,
            curves: brace_covering_both_staves(),
            path_count: 8,
            ..VectorScene::default()
        };
        let y = band_anchor(&scene, &piano_cores(), 70, 230).unwrap();
        let cusp = (128.0_f32 - 70.0).round() as i32;
        assert!(
            (y - cusp).abs() <= 1,
            "anchor {y}, want cusp {cusp} (not the brace midpoint)"
        );
    }

    #[test]
    fn tall_brace_over_all_staves_is_not_dropped_as_page_border() {
        let scene = VectorScene {
            w: 400.0,
            h: 600.0,
            curves: brace_covering_both_staves(),
            path_count: 8,
            ..VectorScene::default()
        };
        // 条带几乎被括号撑满, 上下仍有页边. 不能当成通页竖线改用谱表重心.
        let y = band_anchor(&scene, &piano_cores(), 88, 200).unwrap();
        let cusp = (128.0_f32 - 88.0).round() as i32;
        let centroid = ((100.0_f32 + 192.0) * 0.5 - 88.0).round() as i32;
        assert!((y - cusp).abs() <= 1, "anchor {y}, want cusp {cusp}");
        assert_ne!(y, centroid);
    }

    #[test]
    fn white_ink_picks_dark_pad() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 1, Rgba([250, 250, 250, 255]));
        assert_eq!(polarity_pad(&img), [0x2b, 0x2b, 0x2b]);
    }

    #[test]
    fn black_ink_picks_white_pad() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 1, Rgba([0, 0, 0, 255]));
        assert_eq!(polarity_pad(&img), [255, 255, 255]);
    }

    #[test]
    fn white_ink_blends_onto_paper_and_holes_stay_paper() {
        let mut src = RgbaImage::from_pixel(3, 1, Rgba([0, 0, 0, 0]));
        src.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        let mut paper = RgbImage::from_pixel(3, 1, Rgb([180, 140, 90]));
        blend_rgba_into(&mut paper, &src, 0, 0);
        assert_eq!(paper.get_pixel(0, 0).0, [180, 140, 90]);
        assert_eq!(paper.get_pixel(1, 0).0, [255, 255, 255]);
        assert_eq!(paper.get_pixel(2, 0).0, [180, 140, 90]);
    }
}
