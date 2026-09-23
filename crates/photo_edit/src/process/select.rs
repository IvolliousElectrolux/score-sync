//! 矩形 / 魔棒 / 套索选区.
//!
//! 魔棒容差沿用 Photoshop 实测模型 (John Wheeler, 2015): 像素相对取样点的
//! 通道差 `dR, dG, dB` 同时满足六个不等式, 等价于
//! `max(dR, dG, dB, 0) - min(dR, dG, dB, 0) <= tolerance`.
//! 这比单纯的通道立方更紧, 会卡住色度方向的泄漏.
//! 抗锯齿沿用 GIMP 模糊选择与 PS 一致的做法: 容差内全选, 容差到 1.5 倍之间线性落到 0.

use std::collections::HashSet;
use std::sync::Arc;

use image::{RgbImage, RgbaImage};

#[derive(Clone, Debug, Default)]
pub enum Selection {
    #[default]
    None,
    Rect {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
    },
    /// 画布尺寸的选区蒙版, 0 = 未选, 255 = 全选, 中间值是抗锯齿边缘.
    /// `bounds` 缓存包围盒. `outline` 是套索多边形; `loops` 是魔棒轮廓 (含孔).
    Mask {
        data: Arc<Vec<u8>>,
        bounds: (i32, i32, i32, i32),
        outline: Arc<Vec<(f32, f32)>>,
        loops: Arc<Vec<Vec<(f32, f32)>>>,
    },
}

impl Selection {
    pub fn from_mask(data: Vec<u8>, canvas_w: u32, canvas_h: u32) -> Self {
        Self::from_parts(data, Vec::new(), Vec::new(), canvas_w, canvas_h)
    }

    pub fn from_poly(
        data: Vec<u8>,
        outline: Vec<(f32, f32)>,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Self {
        Self::from_parts(data, outline, Vec::new(), canvas_w, canvas_h)
    }

    pub fn from_loops(
        data: Vec<u8>,
        loops: Vec<Vec<(f32, f32)>>,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Self {
        Self::from_parts(data, Vec::new(), loops, canvas_w, canvas_h)
    }

    fn from_parts(
        data: Vec<u8>,
        outline: Vec<(f32, f32)>,
        loops: Vec<Vec<(f32, f32)>>,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Self {
        let data = Arc::new(data);
        let outline = Arc::new(outline);
        let loops = Arc::new(loops);
        match mask_bounds(&data, canvas_w, canvas_h) {
            Some(bounds) => Self::Mask {
                data,
                bounds,
                outline,
                loops,
            },
            None => Self::None,
        }
    }

    pub fn outline(&self) -> Option<&[(f32, f32)]> {
        match self {
            Self::Mask { outline, .. } if outline.len() >= 3 => Some(outline.as_slice()),
            _ => None,
        }
    }

    pub fn loops(&self) -> Option<&[Vec<(f32, f32)>]> {
        match self {
            Self::Mask { loops, .. } if !loops.is_empty() => Some(loops.as_slice()),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn bounds(&self, canvas_w: u32, canvas_h: u32) -> Option<(i32, i32, i32, i32)> {
        match self {
            Self::None => None,
            Self::Rect { x0, y0, x1, y1 } => {
                let a = (*x0).min(*x1).clamp(0, canvas_w.saturating_sub(1) as i32);
                let b = (*y0).min(*y1).clamp(0, canvas_h.saturating_sub(1) as i32);
                let c = (*x0).max(*x1).clamp(0, canvas_w.saturating_sub(1) as i32);
                let d = (*y0).max(*y1).clamp(0, canvas_h.saturating_sub(1) as i32);
                if c < a || d < b {
                    None
                } else {
                    Some((a, b, c, d))
                }
            }
            Self::Mask { bounds, .. } => {
                let (x0, y0, x1, y1) = *bounds;
                if x1 < x0 || y1 < y0 {
                    None
                } else {
                    Some((x0, y0, x1, y1))
                }
            }
        }
    }

    pub fn contains(&self, x: i32, y: i32, canvas_w: u32, canvas_h: u32) -> bool {
        match self {
            Self::None => false,
            Self::Rect { .. } => self
                .bounds(canvas_w, canvas_h)
                .map(|(x0, y0, x1, y1)| x >= x0 && x <= x1 && y >= y0 && y <= y1)
                .unwrap_or(false),
            Self::Mask { data: m, .. } => {
                if x < 0 || y < 0 {
                    return false;
                }
                let i = y as usize * canvas_w as usize + x as usize;
                m.get(i).copied().unwrap_or(0) > 0
            }
        }
    }
}

pub fn mask_bounds(m: &[u8], w: u32, h: u32) -> Option<(i32, i32, i32, i32)> {
    let mut x0 = w as i32;
    let mut y0 = h as i32;
    let mut x1 = -1;
    let mut y1 = -1;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if m[(y as usize * w as usize) + x as usize] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < 0 {
        None
    } else {
        Some((x0, y0, x1, y1))
    }
}

/// 把旧选区蒙版按包围盒缩放到新包围盒 (魔棒拖边).
pub fn remap_mask_bounds(
    old: &[u8],
    canvas_w: u32,
    canvas_h: u32,
    old_b: (i32, i32, i32, i32),
    new_b: (i32, i32, i32, i32),
) -> Vec<u8> {
    let (ox0, oy0, ox1, oy1) = old_b;
    let (nx0, ny0, nx1, ny1) = new_b;
    let ow = (ox1 - ox0 + 1).max(1) as f32;
    let oh = (oy1 - oy0 + 1).max(1) as f32;
    let nw = (nx1 - nx0 + 1).max(1) as f32;
    let nh = (ny1 - ny0 + 1).max(1) as f32;
    let mut out = vec![0u8; (canvas_w * canvas_h) as usize];
    let cw = canvas_w as i32;
    let ch = canvas_h as i32;
    for y in ny0.max(0)..=ny1.min(ch - 1) {
        for x in nx0.max(0)..=nx1.min(cw - 1) {
            let u = (x - nx0) as f32 / nw;
            let v = (y - ny0) as f32 / nh;
            let sx = ox0 + (u * ow).floor() as i32;
            let sy = oy0 + (v * oh).floor() as i32;
            if sx < 0 || sy < 0 || sx >= cw || sy >= ch {
                continue;
            }
            let i = (sy as usize) * canvas_w as usize + sx as usize;
            if old.get(i).copied().unwrap_or(0) > 0 {
                out[(y as usize) * canvas_w as usize + x as usize] = 255;
            }
        }
    }
    out
}

/// 魔棒参数. `tolerance` 为 0..=255, 与 Photoshop 选项栏一致.
#[derive(Clone, Copy, Debug)]
pub struct WandOpts {
    pub tolerance: i32,
    pub contiguous: bool,
    pub antialias: bool,
}

/// 与已有选区的合成方式, 对应 PS 的替换 / Shift 加选 / Alt 减选 / Shift+Alt 交选.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelCombine {
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// Photoshop 容差距离: 最小的 T, 使该像素落入取样点的容差体.
#[inline]
pub fn ps_tolerance_dist(seed: [u8; 3], px: [u8; 3]) -> i32 {
    let dr = px[0] as i32 - seed[0] as i32;
    let dg = px[1] as i32 - seed[1] as i32;
    let db = px[2] as i32 - seed[2] as i32;
    let hi = dr.max(dg).max(db).max(0);
    let lo = dr.min(dg).min(db).min(0);
    hi - lo
}

/// 容差内为 255; 抗锯齿时 (T, 1.5T) 线性下降, 与 GIMP 模糊选择同一条斜坡.
#[inline]
fn coverage_at(dist: i32, tol: i32, antialias: bool) -> u8 {
    if dist <= tol {
        return 255;
    }
    if !antialias || tol <= 0 || dist.saturating_mul(2) >= tol.saturating_mul(3) {
        return 0;
    }
    ((tol * 3 - dist * 2) * 255 / tol) as u8
}

fn wand_mask(
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    opts: WandOpts,
    sample: impl Fn(i32, i32) -> Option<[u8; 3]>,
) -> Vec<u8> {
    let n = (w.max(0) as usize).saturating_mul(h.max(0) as usize);
    let mut mask = vec![0u8; n];
    if w <= 0 || h <= 0 || x < 0 || y < 0 || x >= w || y >= h {
        return mask;
    }
    let Some(seed) = sample(x, y) else {
        return mask;
    };
    let tol = opts.tolerance.clamp(0, 255);
    let aa = opts.antialias;
    let idx = |x: i32, y: i32| (y as usize) * (w as usize) + (x as usize);
    let cov = |x: i32, y: i32| -> u8 {
        match sample(x, y) {
            Some(px) => coverage_at(ps_tolerance_dist(seed, px), tol, aa),
            None => 0,
        }
    };
    if cov(x, y) == 0 {
        return mask;
    }
    if !opts.contiguous {
        for yy in 0..h {
            for xx in 0..w {
                let c = cov(xx, yy);
                if c > 0 {
                    mask[idx(xx, yy)] = c;
                }
            }
        }
        return mask;
    }
    // 4 连通扫描线填充. 比较对象始终是取样色, 不是相邻像素.
    let mut stack = Vec::new();
    stack.push((x, y));
    while let Some((sx, sy)) = stack.pop() {
        if sx < 0 || sy < 0 || sx >= w || sy >= h || mask[idx(sx, sy)] != 0 || cov(sx, sy) == 0 {
            continue;
        }
        let mut left = sx;
        while left > 0 && mask[idx(left - 1, sy)] == 0 && cov(left - 1, sy) > 0 {
            left -= 1;
        }
        let mut right = sx;
        while right + 1 < w && mask[idx(right + 1, sy)] == 0 && cov(right + 1, sy) > 0 {
            right += 1;
        }
        for xx in left..=right {
            let c = cov(xx, sy).max(1);
            mask[idx(xx, sy)] = c;
        }
        for ny in [sy - 1, sy + 1] {
            if ny < 0 || ny >= h {
                continue;
            }
            let mut i = left;
            while i <= right {
                if mask[idx(i, ny)] == 0 && cov(i, ny) > 0 {
                    stack.push((i, ny));
                    i += 1;
                    while i <= right && mask[idx(i, ny)] == 0 && cov(i, ny) > 0 {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
        }
    }
    mask
}

pub fn wand_mask_rgba(img: &RgbaImage, x: i32, y: i32, opts: WandOpts) -> Vec<u8> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let raw = img.as_raw();
    wand_mask(w, h, x, y, opts, |x, y| {
        let i = ((y as usize) * (w as usize) + (x as usize)) * 4;
        if raw[i + 3] == 0 {
            None
        } else {
            Some([raw[i], raw[i + 1], raw[i + 2]])
        }
    })
}

pub fn flood_mask_rgba(img: &RgbaImage, x: i32, y: i32, tolerance: i32) -> Vec<u8> {
    wand_mask_rgba(
        img,
        x,
        y,
        WandOpts {
            tolerance,
            contiguous: true,
            antialias: false,
        },
    )
}

/// 把图层局部蒙版抬到画布坐标 (透明处不选; 旋转按像素中心映射).
pub fn lift_layer_mask_to_canvas(
    local: &[u8],
    layer_w: u32,
    layer_h: u32,
    layer_x: i32,
    layer_y: i32,
    rotation: f32,
    canvas_w: u32,
    canvas_h: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; (canvas_w * canvas_h) as usize];
    let cw = canvas_w as i32;
    let ch = canvas_h as i32;
    let cx = layer_x as f32 + layer_w as f32 * 0.5;
    let cy = layer_y as f32 + layer_h as f32 * 0.5;
    for y in 0..layer_h as i32 {
        for x in 0..layer_w as i32 {
            let v = local
                .get((y as usize) * layer_w as usize + x as usize)
                .copied()
                .unwrap_or(0);
            if v == 0 {
                continue;
            }
            let (gx, gy) = if rotation.abs() < 0.05 {
                (layer_x + x, layer_y + y)
            } else {
                let (rx, ry) = crate::geom::rotate_point(
                    layer_x as f32 + x as f32 + 0.5,
                    layer_y as f32 + y as f32 + 0.5,
                    cx,
                    cy,
                    rotation,
                );
                (rx.floor() as i32, ry.floor() as i32)
            };
            if gx >= 0 && gy >= 0 && gx < cw && gy < ch {
                let i = (gy as usize) * canvas_w as usize + gx as usize;
                if v > out[i] {
                    out[i] = v;
                }
            }
        }
    }
    out
}

pub fn canvas_to_layer_xy(
    ix: i32,
    iy: i32,
    layer_x: i32,
    layer_y: i32,
    layer_w: u32,
    layer_h: u32,
    rotation: f32,
) -> (i32, i32) {
    if rotation.abs() < 0.05 {
        return (ix - layer_x, iy - layer_y);
    }
    let cx = layer_x as f32 + layer_w as f32 * 0.5;
    let cy = layer_y as f32 + layer_h as f32 * 0.5;
    let (ux, uy) = crate::geom::rotate_point(ix as f32 + 0.5, iy as f32 + 0.5, cx, cy, -rotation);
    (
        (ux - layer_x as f32).floor() as i32,
        (uy - layer_y as f32).floor() as i32,
    )
}

pub fn flood_mask(img: &RgbImage, x: i32, y: i32, tolerance: i32) -> Vec<u8> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let raw = img.as_raw();
    wand_mask(
        w,
        h,
        x,
        y,
        WandOpts {
            tolerance,
            contiguous: true,
            antialias: false,
        },
        |x, y| {
            let i = ((y as usize) * (w as usize) + (x as usize)) * 3;
            Some([raw[i], raw[i + 1], raw[i + 2]])
        },
    )
}

pub fn raster_selection(sel: &Selection, canvas_w: u32, canvas_h: u32) -> Vec<u8> {
    let n = (canvas_w as usize).saturating_mul(canvas_h as usize);
    match sel {
        Selection::None => vec![0; n],
        Selection::Mask { data, .. } if data.len() == n => data.as_ref().clone(),
        Selection::Mask { .. } => vec![0; n],
        Selection::Rect { x0, y0, x1, y1 } => {
            let mut m = vec![0u8; n];
            let a = (*x0).min(*x1);
            let b = (*y0).min(*y1);
            let c = (*x0).max(*x1);
            let d = (*y0).max(*y1);
            let cw = canvas_w as i32;
            let ch = canvas_h as i32;
            for y in b.max(0)..=d.min(ch - 1) {
                for x in a.max(0)..=c.min(cw - 1) {
                    m[(y as usize) * canvas_w as usize + x as usize] = 255;
                }
            }
            m
        }
    }
}

pub fn combine_masks(prev: &[u8], neu: &[u8], op: SelCombine) -> Vec<u8> {
    match op {
        SelCombine::Replace => neu.to_vec(),
        SelCombine::Add => prev
            .iter()
            .zip(neu.iter())
            .map(|(a, b)| (*a).max(*b))
            .collect(),
        SelCombine::Subtract => prev
            .iter()
            .zip(neu.iter())
            .map(|(a, b)| ((*a as u16 * (255 - *b as u16)) / 255) as u8)
            .collect(),
        SelCombine::Intersect => prev
            .iter()
            .zip(neu.iter())
            .map(|(a, b)| ((*a as u16 * *b as u16) / 255) as u8)
            .collect(),
    }
}

/// 射线法: 点是否在多边形内.
pub fn point_in_poly(x: f32, y: f32, pts: &[(i32, i32)]) -> bool {
    if pts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = (pts[i].0 as f32, pts[i].1 as f32);
        let (xj, yj) = (pts[j].0 as f32, pts[j].1 as f32);
        let intersect =
            ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi + f32::EPSILON) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 拖选区边时, 把套索轮廓从旧包围盒线性映到新包围盒.
pub fn scale_outline(
    pts: &[(f32, f32)],
    old_b: (i32, i32, i32, i32),
    new_b: (i32, i32, i32, i32),
) -> Vec<(f32, f32)> {
    let (ox0, oy0, ox1, oy1) = old_b;
    let (nx0, ny0, nx1, ny1) = new_b;
    let ow = (ox1 - ox0).max(1) as f32;
    let oh = (oy1 - oy0).max(1) as f32;
    let nw = (nx1 - nx0) as f32;
    let nh = (ny1 - ny0) as f32;
    pts.iter()
        .map(|(x, y)| {
            let u = (*x - ox0 as f32) / ow;
            let v = (*y - oy0 as f32) / oh;
            (nx0 as f32 + u * nw, ny0 as f32 + v * nh)
        })
        .collect()
}

/// 扫描线填充多边形 (偶奇规则). 水平边跳过, 避免顶点被数两次.
pub fn fill_poly_mask(pts: &[(f32, f32)], canvas_w: u32, canvas_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (canvas_w as usize).saturating_mul(canvas_h as usize)];
    if pts.len() < 3 || canvas_w == 0 || canvas_h == 0 {
        return out;
    }
    let mut y_min = i32::MAX;
    let mut y_max = i32::MIN;
    for (_, y) in pts {
        y_min = y_min.min(y.floor() as i32);
        y_max = y_max.max(y.ceil() as i32);
    }
    let y_min = y_min.max(0);
    let y_max = y_max.min(canvas_h as i32 - 1);
    if y_max < y_min {
        return out;
    }
    let n = pts.len();
    let cw = canvas_w as i32;
    for y in y_min..=y_max {
        let scan = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            let (ymin, ymax, xa, xb) = if y0 <= y1 {
                (y0, y1, x0, x1)
            } else {
                (y1, y0, x1, x0)
            };
            if ymax - ymin < 1e-4 || scan < ymin || scan >= ymax {
                continue;
            }
            let t = (scan - ymin) / (ymax - ymin);
            xs.push(xa + t * (xb - xa));
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let row = (y as usize) * canvas_w as usize;
        let mut i = 0;
        while i + 1 < xs.len() {
            let mut a = xs[i].floor() as i32;
            let mut b = xs[i + 1].ceil() as i32;
            if b < a {
                std::mem::swap(&mut a, &mut b);
            }
            a = a.max(0);
            b = b.min(cw);
            for x in a..b {
                out[row + x as usize] = 255;
            }
            i += 2;
        }
    }
    out
}

pub fn extract_mask_layer(
    src: &RgbaImage,
    src_x: i32,
    src_y: i32,
    mask: &[u8],
    canvas_w: u32,
    canvas_h: u32,
) -> Option<(RgbaImage, i32, i32)> {
    let (x0, y0, x1, y1) = mask_bounds(mask, canvas_w, canvas_h)?;
    let w = (x1 - x0 + 1) as u32;
    let h = (y1 - y0 + 1) as u32;
    let mut out = RgbaImage::new(w, h);
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let cx = x0 + x;
            let cy = y0 + y;
            let mi = (cy as usize) * canvas_w as usize + cx as usize;
            let m = mask.get(mi).copied().unwrap_or(0);
            if m == 0 {
                continue;
            }
            let sx = cx - src_x;
            let sy = cy - src_y;
            if sx >= 0 && sy >= 0 && sx < sw && sy < sh {
                let mut p = *src.get_pixel(sx as u32, sy as u32);
                if m < 255 {
                    p.0[3] = ((p.0[3] as u16 * m as u16) / 255) as u8;
                }
                out.put_pixel(x as u32, y as u32, p);
            }
        }
    }
    Some((out, x0, y0))
}

pub fn cut_mask_from_layer(
    img: &mut RgbaImage,
    src_x: i32,
    src_y: i32,
    mask: &[u8],
    canvas_w: u32,
    canvas_h: u32,
    fill_paper: Option<[u8; 3]>,
) {
    let Some((x0, y0, x1, y1)) = mask_bounds(mask, canvas_w, canvas_h) else {
        return;
    };
    let sw = img.width() as i32;
    let sh = img.height() as i32;
    for cy in y0..=y1 {
        for cx in x0..=x1 {
            let mi = (cy as usize) * canvas_w as usize + cx as usize;
            let t = mask.get(mi).copied().unwrap_or(0) as u16;
            if t == 0 {
                continue;
            }
            let sx = cx - src_x;
            let sy = cy - src_y;
            if sx < 0 || sy < 0 || sx >= sw || sy >= sh {
                continue;
            }
            let p = img.get_pixel(sx as u32, sy as u32).0;
            let neu = if let Some(paper) = fill_paper {
                let inv = 255 - t;
                image::Rgba([
                    ((p[0] as u16 * inv + paper[0] as u16 * t) / 255) as u8,
                    ((p[1] as u16 * inv + paper[1] as u16 * t) / 255) as u8,
                    ((p[2] as u16 * inv + paper[2] as u16 * t) / 255) as u8,
                    ((p[3] as u16 * inv + 255 * t) / 255) as u8,
                ])
            } else {
                let a = ((p[3] as u16 * (255 - t)) / 255) as u8;
                if a == 0 {
                    image::Rgba([0, 0, 0, 0])
                } else {
                    image::Rgba([p[0], p[1], p[2], a])
                }
            };
            img.put_pixel(sx as u32, sy as u32, neu);
        }
    }
}

/// 沿选区 50% (或给定阈值) 边界走像素角点, 得到闭合轮廓, 含孔.
/// 直线段会收成端点; 过长的曲线再按约 1px 误差抽稀, 避免蚂蚁线每帧画几十万段.
pub fn mask_loops(mask: &[u8], w: u32, h: u32, thresh: u8) -> Vec<Vec<(f32, f32)>> {
    if w == 0 || h == 0 || mask.len() < (w as usize).saturating_mul(h as usize) {
        return Vec::new();
    }
    let Some((bx0, by0, bx1, by1)) = mask_bounds(mask, w, h) else {
        return Vec::new();
    };
    let wi = w as i32;
    let hi = h as i32;
    let on = |x: i32, y: i32| -> bool {
        if x < 0 || y < 0 || x >= wi || y >= hi {
            return false;
        }
        mask[(y as usize) * (w as usize) + x as usize] >= thresh
    };
    let mut visited: HashSet<(i32, i32, u8)> = HashSet::new();
    let mut loops = Vec::new();
    let y_end = (by1 + 1).min(hi);
    for y in by0..=y_end {
        for x in bx0..=bx1 {
            if on(x, y) && !on(x, y - 1) {
                trace_loop(&on, x, y, 0, &mut visited, &mut loops, wi, hi);
            }
            if on(x, y - 1) && !on(x, y) {
                trace_loop(&on, x + 1, y, 2, &mut visited, &mut loops, wi, hi);
            }
        }
    }
    budget_loops(&mut loops);
    loops
}

/// `facing`: 0 东, 1 南, 2 西, 3 北. 填充区保持在行进方向右侧 (4 连通).
fn trace_loop(
    on: &impl Fn(i32, i32) -> bool,
    sx: i32,
    sy: i32,
    sdir: u8,
    visited: &mut HashSet<(i32, i32, u8)>,
    loops: &mut Vec<Vec<(f32, f32)>>,
    w: i32,
    h: i32,
) {
    let start_key = (sx, sy, sdir);
    if visited.contains(&start_key) || !edge_is_boundary(on, sx, sy, sdir) {
        return;
    }
    let mut cx = sx;
    let mut cy = sy;
    let mut facing = sdir;
    let mut pts = Vec::new();
    let limit = ((w.max(0) as usize) + 1)
        .saturating_mul((h.max(0) as usize) + 1)
        .saturating_mul(4)
        .max(16);
    for _ in 0..limit {
        let Some(dir) = turn_priority(facing)
            .into_iter()
            .find(|&d| edge_is_boundary(on, cx, cy, d))
        else {
            break;
        };
        let key = (cx, cy, dir);
        if !visited.insert(key) {
            break;
        }
        if pts.is_empty() {
            pts.push((cx as f32, cy as f32));
        }
        let (nx, ny) = step_corner(cx, cy, dir);
        pts.push((nx as f32, ny as f32));
        cx = nx;
        cy = ny;
        facing = dir;
    }
    if pts.len() >= 2 && pts.first() == pts.last() {
        pts.pop();
    } else {
        return;
    }
    let pts = drop_colinear(&pts);
    if pts.len() >= 3 {
        loops.push(pts);
    }
}

fn turn_priority(facing: u8) -> [u8; 4] {
    match facing {
        0 => [1, 0, 3, 2],
        1 => [2, 1, 0, 3],
        2 => [3, 2, 1, 0],
        _ => [0, 3, 2, 1],
    }
}

fn step_corner(x: i32, y: i32, dir: u8) -> (i32, i32) {
    match dir {
        0 => (x + 1, y),
        1 => (x, y + 1),
        2 => (x - 1, y),
        _ => (x, y - 1),
    }
}

/// 该方向的出边是否夹在 "右为选中, 左为未选" 之间.
fn edge_is_boundary(on: &impl Fn(i32, i32) -> bool, cx: i32, cy: i32, dir: u8) -> bool {
    let (rx, ry, lx, ly) = match dir {
        0 => (cx, cy, cx, cy - 1),
        1 => (cx - 1, cy, cx, cy),
        2 => (cx - 1, cy - 1, cx - 1, cy),
        _ => (cx, cy - 1, cx - 1, cy - 1),
    };
    on(rx, ry) && !on(lx, ly)
}

fn drop_colinear(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = pts[(i + n - 1) % n];
        let c = pts[i];
        let q = pts[(i + 1) % n];
        let cross = (c.0 - p.0) * (q.1 - c.1) - (c.1 - p.1) * (q.0 - c.0);
        if cross.abs() > 0.01 {
            out.push(c);
        }
    }
    if out.len() < 3 {
        pts.to_vec()
    } else {
        out
    }
}

fn budget_loops(loops: &mut [Vec<(f32, f32)>]) {
    for pts in loops.iter_mut() {
        let mut eps = 1.15_f32;
        while pts.len() > 2000 && eps <= 4.0 {
            let next = dp_closed(pts, eps * eps);
            if next.len() >= pts.len() {
                break;
            }
            *pts = next;
            eps *= 1.6;
        }
    }
    let mut total: usize = loops.iter().map(|l| l.len()).sum();
    let mut eps = 1.5_f32;
    while total > 20000 && eps <= 6.0 {
        for pts in loops.iter_mut() {
            if pts.len() > 48 {
                *pts = dp_closed(pts, eps * eps);
            }
        }
        let neu: usize = loops.iter().map(|l| l.len()).sum();
        if neu >= total {
            break;
        }
        total = neu;
        eps *= 1.5;
    }
}

fn dp_closed(pts: &[(f32, f32)], eps2: f32) -> Vec<(f32, f32)> {
    if pts.len() < 4 {
        return pts.to_vec();
    }
    let mut far = 1usize;
    let mut best = 0.0_f32;
    for (i, p) in pts.iter().enumerate().skip(1) {
        let dx = p.0 - pts[0].0;
        let dy = p.1 - pts[0].1;
        let d = dx * dx + dy * dy;
        if d > best {
            best = d;
            far = i;
        }
    }
    if far == 0 || far + 1 >= pts.len() {
        return pts.to_vec();
    }
    let mut a = pts[..=far].to_vec();
    let mut b = Vec::with_capacity(pts.len() - far + 1);
    b.push(pts[far]);
    b.extend_from_slice(&pts[far + 1..]);
    b.push(pts[0]);
    a = dp_open(&a, eps2);
    b = dp_open(&b, eps2);
    if b.len() >= 2 {
        a.extend_from_slice(&b[1..b.len() - 1]);
    }
    if a.len() < 3 {
        pts.to_vec()
    } else {
        a
    }
}

fn dp_open(pts: &[(f32, f32)], eps2: f32) -> Vec<(f32, f32)> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let mut max_d = 0.0_f32;
        let mut idx = i;
        for k in i + 1..j {
            let d = seg_dist2(pts[k], pts[i], pts[j]);
            if d > max_d {
                max_d = d;
                idx = k;
            }
        }
        if max_d > eps2 {
            keep[idx] = true;
            stack.push((i, idx));
            stack.push((idx, j));
        }
    }
    pts.iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, p)| *p)
        .collect()
}

fn seg_dist2(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-8 {
        let ex = p.0 - a.0;
        let ey = p.1 - a.1;
        return ex * ex + ey * ey;
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    let ex = p.0 - (a.0 + t * dx);
    let ey = p.1 - (a.1 + t * dy);
    ex * ex + ey * ey
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn wand_fills_connected() {
        let mut img = RgbImage::from_pixel(8, 8, Rgb([255, 255, 255]));
        for y in 2..5 {
            for x in 2..5 {
                img.put_pixel(x, y, Rgb([10, 10, 10]));
            }
        }
        let m = flood_mask(&img, 3, 3, 20);
        let n = m.iter().filter(|&&v| v > 0).count();
        assert_eq!(n, 9);
    }

    #[test]
    fn remap_mask_scales_box() {
        let mut m = vec![0u8; 8 * 8];
        for y in 2..4 {
            for x in 2..4 {
                m[y * 8 + x] = 255;
            }
        }
        let out = remap_mask_bounds(&m, 8, 8, (2, 2, 3, 3), (1, 1, 4, 4));
        assert!(out[1 * 8 + 1] > 0);
        assert!(out[4 * 8 + 4] > 0);
    }

    #[test]
    fn mask_bounds_are_cached() {
        let mut m = vec![0u8; 8 * 8];
        for y in 2..5 {
            for x in 1..4 {
                m[y * 8 + x] = 255;
            }
        }
        let sel = Selection::from_mask(m, 8, 8);
        assert_eq!(sel.bounds(8, 8), Some((1, 2, 3, 4)));
    }

    #[test]
    fn rgba_wand_skips_transparent() {
        let mut img = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        for y in 2..5 {
            for x in 2..5 {
                img.put_pixel(x, y, image::Rgba([10, 10, 10, 255]));
            }
        }
        let empty = flood_mask_rgba(&img, 0, 0, 20);
        assert!(empty.iter().all(|&v| v == 0));
        let m = flood_mask_rgba(&img, 3, 3, 20);
        assert_eq!(m.iter().filter(|&&v| v > 0).count(), 9);
    }

    #[test]
    fn lift_mask_offsets_to_canvas() {
        let mut local = vec![0u8; 4 * 4];
        local[1 * 4 + 1] = 255;
        let out = lift_layer_mask_to_canvas(&local, 4, 4, 10, 20, 0.0, 32, 32);
        assert_eq!(out[(21) * 32 + 11], 255);
        assert_eq!(out.iter().filter(|&&v| v > 0).count(), 1);
    }

    #[test]
    fn fill_poly_covers_triangle() {
        let pts = [(1.0, 1.0), (6.0, 1.0), (1.0, 6.0)];
        let m = fill_poly_mask(&pts, 8, 8);
        assert!(m[2 * 8 + 2] > 0);
        assert_eq!(m[7 * 8 + 7], 0);
    }

    fn opts(tolerance: i32, contiguous: bool, antialias: bool) -> WandOpts {
        WandOpts {
            tolerance,
            contiguous,
            antialias,
        }
    }

    #[test]
    fn ps_tolerance_accepts_cube_and_rejects_chroma() {
        // 取样 255,120,0, 容差 10. (255,130,10) 的曼哈顿距离是 20, 旧魔棒会丢掉;
        // Photoshop 六个不等式都 <= 10, 应当选中. (245,130,0) 的通道差跨度是 20, 应当丢掉.
        let mut img = RgbaImage::from_pixel(4, 1, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(0, 0, image::Rgba([255, 120, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([255, 130, 10, 255]));
        img.put_pixel(2, 0, image::Rgba([245, 110, 0, 255]));
        img.put_pixel(3, 0, image::Rgba([245, 130, 0, 255]));
        let m = wand_mask_rgba(&img, 0, 0, opts(10, false, false));
        assert_eq!(m[0], 255);
        assert_eq!(m[1], 255);
        assert_eq!(m[2], 255);
        assert_eq!(m[3], 0);
        assert_eq!(ps_tolerance_dist([255, 120, 0], [255, 130, 10]), 10);
        assert_eq!(ps_tolerance_dist([255, 120, 0], [245, 130, 0]), 20);
    }

    #[test]
    fn antialias_ramps_past_tolerance() {
        let mut img = RgbaImage::from_pixel(3, 1, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([10, 0, 0, 255]));
        img.put_pixel(2, 0, image::Rgba([40, 0, 0, 255]));
        let hard = wand_mask_rgba(&img, 0, 0, opts(32, false, false));
        assert_eq!(hard[1], 255);
        assert_eq!(hard[2], 0);
        let soft = wand_mask_rgba(&img, 0, 0, opts(32, false, true));
        assert_eq!(soft[0], 255);
        assert_eq!(soft[1], 255);
        assert!(
            soft[2] > 0 && soft[2] < 255,
            "dist 40 应落在 (32, 48) 斜坡上, got {}",
            soft[2]
        );
    }

    #[test]
    fn contiguous_stops_at_gap_and_global_does_not() {
        let mut img = RgbaImage::from_pixel(5, 1, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(4, 0, image::Rgba([0, 0, 0, 255]));
        let local = wand_mask_rgba(&img, 0, 0, opts(8, true, false));
        assert_eq!(local.iter().filter(|&&v| v > 0).count(), 2);
        assert_eq!(local[4], 0);
        let global = wand_mask_rgba(&img, 0, 0, opts(8, false, false));
        assert_eq!(global[4], 255);
    }

    #[test]
    fn transparent_pixel_blocks_flood() {
        let mut img = RgbaImage::from_pixel(3, 1, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        let m = wand_mask_rgba(&img, 0, 0, opts(10, true, true));
        assert_eq!(m[0], 255);
        assert_eq!(m[1], 0);
        assert_eq!(m[2], 0);
    }

    #[test]
    fn l_shape_is_not_its_bounding_box() {
        let mut img = RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        for y in 0..3 {
            img.put_pixel(0, y, image::Rgba([0, 0, 0, 255]));
        }
        for x in 0..3 {
            img.put_pixel(x, 2, image::Rgba([0, 0, 0, 255]));
        }
        let m = wand_mask_rgba(&img, 0, 0, opts(12, true, false));
        assert_eq!(m.iter().filter(|&&v| v > 0).count(), 5);
        assert_eq!(m[1 * 4 + 1], 0);
        let loops = mask_loops(&m, 4, 4, 128);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 6);
    }

    #[test]
    fn wand_walks_around_a_notch() {
        let mut img = RgbaImage::from_pixel(3, 3, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let m = wand_mask_rgba(&img, 0, 0, opts(0, true, false));
        assert_eq!(m.iter().filter(|&&v| v > 0).count(), 8);
        assert_eq!(m[1 * 3 + 1], 0);
    }

    fn loop_box(pts: &[(f32, f32)]) -> (f32, f32, f32, f32) {
        let mut x0 = f32::MAX;
        let mut y0 = f32::MAX;
        let mut x1 = f32::MIN;
        let mut y1 = f32::MIN;
        for (x, y) in pts {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        (x0, y0, x1, y1)
    }

    #[test]
    fn contour_traces_border_hole_and_diagonal() {
        let full = vec![255u8; 4 * 4];
        let loops = mask_loops(&full, 4, 4, 128);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 4);
        assert_eq!(loop_box(&loops[0]), (0.0, 0.0, 4.0, 4.0));

        let mut ring = vec![255u8; 5 * 5];
        for y in 1..4 {
            for x in 1..4 {
                ring[y * 5 + x] = 0;
            }
        }
        let loops = mask_loops(&ring, 5, 5, 128);
        assert_eq!(loops.len(), 2, "外轮廓加一个孔");
        let mut boxes: Vec<_> = loops.iter().map(|p| loop_box(p)).collect();
        boxes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert!(boxes.iter().any(|b| *b == (0.0, 0.0, 5.0, 5.0)));
        assert!(boxes.iter().any(|b| *b == (1.0, 1.0, 4.0, 4.0)));

        let mut diag = vec![0u8; 2 * 2];
        diag[0] = 255;
        diag[1 * 2 + 1] = 255;
        let loops = mask_loops(&diag, 2, 2, 128);
        assert_eq!(loops.len(), 2, "只对角相接的像素应分成两圈");
    }

    #[test]
    fn combine_add_sub_and_intersect() {
        let a = vec![255u8, 255, 0, 0];
        let b = vec![0u8, 255, 255, 0];
        assert_eq!(
            combine_masks(&a, &b, SelCombine::Add),
            vec![255, 255, 255, 0]
        );
        assert_eq!(
            combine_masks(&a, &b, SelCombine::Intersect),
            vec![0, 255, 0, 0]
        );
        assert_eq!(
            combine_masks(&a, &b, SelCombine::Subtract),
            vec![255, 0, 0, 0]
        );
    }

    #[test]
    fn soft_extract_scales_alpha() {
        let src = RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 200]));
        let mask = vec![128u8];
        let (out, _, _) = extract_mask_layer(&src, 0, 0, &mask, 1, 1).unwrap();
        let p = out.get_pixel(0, 0).0;
        assert_eq!(p[0], 10);
        assert_eq!(p[3], ((200u16 * 128) / 255) as u8);
    }
}
