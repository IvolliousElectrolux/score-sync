use gpui::{point, px, size, Bounds, CursorStyle, Pixels, Point};

pub(crate) const HISTORY_LIMIT: usize = 48;

#[derive(Clone)]
pub(crate) struct HistorySnap {
    pub doc: crate::document::EditDocument,
    pub pan: Point<f32>,
    pub zoom: f32,
    pub user_zoomed: bool,
}
pub(crate) const GPU_TEX_MAX_SIDE: u32 = 2048;
pub(crate) const BRUSH_MIN: f32 = 2.0;
pub(crate) const BRUSH_SIZE_FALLBACK_MAX: f32 = 80.0;
pub(crate) const BRUSH_DEFAULT: f32 = 18.0;
pub(crate) const HARD_DEFAULT: f32 = 0.7;
pub(crate) const PAINT_DEFAULT: [u8; 3] = [32, 32, 32];
/// 图层列表拖排序: 超过此像素才进入拖拽态.
pub(crate) const REORDER_DRAG_SLOP: f32 = 5.0;
pub(crate) const POLY_SNAP_SCREEN_PX: f32 = 12.0;
/// 自由拖动时贴到起点的屏幕半径. 比点击闭环更小, 减少中途误吸.
pub(crate) const LASSO_DRAG_SNAP_SCREEN_PX: f32 = 6.0;
pub(crate) const LASSO_FREEHAND_SCREEN: f32 = 2.5;
pub(crate) const SB_SIZE: f32 = 168.0;
pub(crate) const HUE_BAR_W: f32 = 18.0;
pub(crate) const SB_TEX_SIZE: u32 = 128;
pub(crate) const HUE_TEX_H: u32 = 256;
pub(crate) const RECENT_COLORS_MAX: usize = 8;
/// 选区/画布拖边吸附的屏幕像素容差.
pub(crate) const EDGE_SNAP_SCREEN_PX: f32 = 8.0;
/// 涂抹间距: 直径的 1/4, 避免每帧重叠盖章.
pub(crate) const BRUSH_SPACING_FRAC: f32 = 0.25;
/// 橡皮: 超过此图像像素位移才视为拖擦 (否则单击只擦最上层).
pub(crate) const ERASE_DRAG_SLOP_IMG: f32 = 3.0;
/// 笔划预览按块上传; 单块边长. 单张 GPU 贴图有边长上限, 整层拆开贴.
pub(crate) const TILE: u32 = 512;
/// 每块贴图四周多取的源像素. 外圈会被图集渗色污染, 内圈仍是真实像素,
/// 绘制时只露出逻辑块, 放大后块与块之间不再出现细线.
pub(crate) const TILE_BLEED: u32 = 2;

/// 当前画布宽对应的笔刷直径上限 (宽的十分之一).
pub(crate) fn brush_size_max_for_image(img_w: u32) -> f32 {
    if img_w < 10 {
        BRUSH_SIZE_FALLBACK_MAX
    } else {
        (img_w as f32 / 10.0).max(BRUSH_MIN)
    }
}

/// 对数滑条: `t=0` → min, `t=1` → max, 越往右增长越快 (与蒙版画笔一致).
pub(crate) fn brush_size_from_t(t: f32, min: f32, max: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let min = min.max(1.0);
    let max = max.max(min);
    min * (max / min).powf(t)
}

pub(crate) fn luma(c: [u8; 3]) -> f32 {
    0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32
}

/// 画笔光标外圈: RGB 反色; 反色太接近时改用黑/白.
#[allow(dead_code)]
pub(crate) fn opposite_rgb(c: [u8; 3]) -> [u8; 3] {
    let inv = [255 - c[0], 255 - c[1], 255 - c[2]];
    let dist = (inv[0] as i16 - c[0] as i16).unsigned_abs()
        + (inv[1] as i16 - c[1] as i16).unsigned_abs()
        + (inv[2] as i16 - c[2] as i16).unsigned_abs();
    if dist < 180 {
        if luma(c) >= 128.0 {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        }
    } else {
        inv
    }
}

/// 外圈描边: `ring` 为主色; `halo` 为反色内外描边 (背景混杂或对比不够时).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RingStyle {
    pub ring: [u8; 3],
    pub halo: Option<[u8; 3]>,
}

/// 按外圈下方采样决定描边. `fill` 是圆内填充色 (没有则忽略).
pub(crate) fn ring_style_from_under(samples: &[[u8; 3]], fill: Option<[u8; 3]>) -> RingStyle {
    const MIN_CONTRAST: f32 = 48.0;
    const MIXED_STD: f32 = 38.0;
    if samples.is_empty() {
        return RingStyle {
            ring: [255, 255, 255],
            halo: Some([0, 0, 0]),
        };
    }
    let n = samples.len() as f32;
    let mean_y = samples.iter().map(|s| luma(*s)).sum::<f32>() / n;
    let var = samples
        .iter()
        .map(|s| {
            let d = luma(*s) - mean_y;
            d * d
        })
        .sum::<f32>()
        / n;
    let std = var.sqrt();
    let ring = if mean_y >= 128.0 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    };
    let min_vs = samples
        .iter()
        .map(|s| (luma(ring) - luma(*s)).abs())
        .fold(f32::MAX, f32::min);
    let fill_close = fill
        .map(|f| (luma(ring) - luma(f)).abs() < MIN_CONTRAST)
        .unwrap_or(false);
    let need_halo = std > MIXED_STD || min_vs < MIN_CONTRAST || fill_close;
    let halo = if need_halo {
        Some(if luma(ring) >= 128.0 {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        })
    } else {
        None
    };
    RingStyle { ring, halo }
}

pub(crate) fn sample_ring_points(cx: f32, cy: f32, r: f32) -> impl Iterator<Item = (f32, f32)> {
    let r = r.max(0.5);
    let n = if r < 4.0 {
        8
    } else if r < 16.0 {
        12
    } else {
        16
    };
    (0..n).map(move |i| {
        let a = (i as f32) * std::f32::consts::TAU / n as f32;
        (cx + r * a.cos(), cy + r * a.sin())
    })
}

pub(crate) fn rgb8(c: [u8; 3]) -> gpui::Rgba {
    gpui::rgb(((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
}

pub(crate) fn brush_size_to_t(size: f32, min: f32, max: f32) -> f32 {
    let min = min.max(1.0);
    let max = max.max(min);
    let size = size.clamp(min, max);
    let span = (max / min).ln();
    if span.abs() < 1e-6 {
        0.0
    } else {
        ((size / min).ln() / span).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolMode {
    #[default]
    Select,
    Move,
    Canvas,
    Heal,
    Clone,
    Paint,
    Eraser,
    Wand,
    Lasso,
    Pan,
}

impl ToolMode {
    pub(crate) fn uses_brush(self) -> bool {
        matches!(self, Self::Heal | Self::Clone | Self::Paint | Self::Eraser)
    }

    pub(crate) fn uses_selection(self) -> bool {
        matches!(self, Self::Select | Self::Wand | Self::Lasso)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ViewXform {
    pub scale: f32,
    pub origin_x: f32,
    pub origin_y: f32,
}

impl ViewXform {
    pub fn compute(
        img_w: f32,
        img_h: f32,
        view_w: f32,
        view_h: f32,
        zoom: f32,
        pan: Point<f32>,
        user_zoomed: bool,
    ) -> Self {
        if img_w < 1.0 || img_h < 1.0 || view_w < 1.0 || view_h < 1.0 {
            return Self {
                scale: 1.0,
                origin_x: 0.0,
                origin_y: 0.0,
            };
        }
        let fit = (view_w / img_w).min(view_h / img_h).max(0.0001);
        let scale = if user_zoomed {
            (fit * zoom).max(0.0001)
        } else {
            fit
        };
        Self {
            scale,
            origin_x: (view_w - img_w * scale) * 0.5 + pan.x,
            origin_y: (view_h - img_h * scale) * 0.5 + pan.y,
        }
    }

    pub fn screen_to_image(&self, sx: f32, sy: f32) -> (f32, f32) {
        (
            (sx - self.origin_x) / self.scale,
            (sy - self.origin_y) / self.scale,
        )
    }

    pub fn image_to_screen(&self, ix: f32, iy: f32) -> (f32, f32) {
        (
            self.origin_x + ix * self.scale,
            self.origin_y + iy * self.scale,
        )
    }

    #[allow(dead_code)]
    pub fn image_rect_to_screen(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds<Pixels> {
        let left = self.origin_x + x0 as f32 * self.scale;
        let top = self.origin_y + y0 as f32 * self.scale;
        let right = self.origin_x + (x1 as f32 + 1.0) * self.scale;
        let bottom = self.origin_y + (y1 as f32 + 1.0) * self.scale;
        Bounds {
            origin: point(px(left), px(top)),
            size: size(px((right - left).max(1.0)), px((bottom - top).max(1.0))),
        }
    }
}

pub(crate) enum DragKind {
    Select {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    SelResize {
        handle: BoxHandle,
        start: (f32, f32),
        orig: (i32, i32, i32, i32),
        orig_mask: Option<std::sync::Arc<Vec<u8>>>,
        cur: (i32, i32, i32, i32),
    },
    Move {
        start_x: i32,
        start_y: i32,
        orig_x: i32,
        orig_y: i32,
    },
    LayerScale {
        handle: BoxHandle,
        start: (f32, f32),
        orig_x: i32,
        orig_y: i32,
        orig_w: u32,
        orig_h: u32,
        cur_x: i32,
        cur_y: i32,
        cur_w: u32,
        cur_h: u32,
    },
    LayerRotate {
        last_ang: f32,
        accum_deg: f32,
        cx: f32,
        cy: f32,
        last_deg: f32,
    },
    Canvas {
        edge: CanvasEdge,
        start: (f32, f32),
        orig_w: u32,
        orig_h: u32,
        orig_pos: Vec<(i32, i32)>,
        orig_pan: Point<f32>,
    },
    Brush,
    /// 橡皮: `wiping` 为 true 表示已进入拖擦; 否则松开时只动最上层.
    Erase {
        start_ix: f32,
        start_iy: f32,
        undid: bool,
        wiping: bool,
        hit: bool,
    },
    Slider(SliderKind),
    LassoStroke {
        start_x: f32,
        start_y: f32,
        freehand: bool,
    },
    PaletteSb,
    PaletteHue,
    LayerReorder {
        from: usize,
        to: usize,
        line_at: Option<usize>,
        line_after: bool,
        start_x: f32,
        start_y: f32,
        origin_x: f32,
        origin_y: f32,
        x: f32,
        y: f32,
        armed: bool,
    },
    Pan {
        last_x: f32,
        last_y: f32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EraseTarget {
    Trace,
    Layer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceErase {
    /// 笔尖盖掉碰到的盖章点.
    Dab,
    /// 碰到的那一整笔都去掉.
    Stroke,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SliderKind {
    BrushSize,
    Hardness,
    WandTol,
}

pub(crate) const WAND_TOL_MAX: i32 = 255;

/// 按住 Shift 拖图层: 只留横或竖 (绝对值大的轴).
pub(crate) fn axis_lock_delta(dx: i32, dy: i32) -> (i32, i32) {
    if dx.abs() >= dy.abs() {
        (dx, 0)
    } else {
        (0, dy)
    }
}

pub(crate) fn reorder_slop_exceeded(dx: f32, dy: f32) -> bool {
    dx * dx + dy * dy >= REORDER_DRAG_SLOP * REORDER_DRAG_SLOP
}

pub(crate) fn reorder_to_index(from: usize, anchor: usize, after: bool) -> usize {
    if after {
        if from <= anchor {
            anchor
        } else {
            anchor + 1
        }
    } else if from < anchor {
        anchor - 1
    } else {
        anchor
    }
}

/// 图层列表自上而下是栈顶→栈底; `from`/`to` 为显示下标.
pub(crate) fn apply_display_reorder<T>(items: &mut Vec<T>, from_d: usize, to_d: usize) {
    let n = items.len();
    if n == 0 || from_d >= n || from_d == to_d {
        return;
    }
    let mut vis: Vec<T> = std::mem::take(items).into_iter().rev().collect();
    if from_d >= vis.len() {
        *items = vis.into_iter().rev().collect();
        return;
    }
    let item = vis.remove(from_d);
    vis.insert(to_d.min(vis.len()), item);
    *items = vis.into_iter().rev().collect();
}

pub(crate) fn color_rgb_u32(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
}

pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

pub(crate) fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

pub(crate) fn default_recent_colors() -> Vec<[u8; 3]> {
    vec![
        [32, 32, 32],
        [255, 255, 255],
        [250, 204, 21],
        [56, 189, 248],
        [251, 146, 60],
        [74, 222, 128],
        [248, 113, 113],
        [148, 163, 184],
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxHandle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl BoxHandle {
    pub fn cursor(self) -> CursorStyle {
        match self {
            Self::E | Self::W => CursorStyle::ResizeLeftRight,
            Self::N | Self::S => CursorStyle::ResizeUpDown,
            Self::NE | Self::SW => CursorStyle::ResizeUpRightDownLeft,
            Self::NW | Self::SE => CursorStyle::ResizeUpLeftDownRight,
        }
    }
}

/// 拖边/角得到新矩形.
/// `lock` 时锁宽高比: 角以对角为锚, 边以对边中点为锚.
/// 比例按按下位置归一, 避免点在容差内时第一帧跳变.
pub(crate) fn resize_rect(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    ix: f32,
    iy: f32,
    start_x: f32,
    start_y: f32,
    handle: BoxHandle,
    lock: bool,
) -> (f32, f32, f32, f32) {
    let mut a = x0.min(x1);
    let mut b = y0.min(y1);
    let mut c = x0.max(x1);
    let mut d = y0.max(y1);
    let ow = (c - a).max(1.0);
    let oh = (d - b).max(1.0);
    if !lock {
        match handle {
            BoxHandle::E => c = ix,
            BoxHandle::W => a = ix,
            BoxHandle::N => b = iy,
            BoxHandle::S => d = iy,
            BoxHandle::NE => {
                c = ix;
                b = iy;
            }
            BoxHandle::NW => {
                a = ix;
                b = iy;
            }
            BoxHandle::SE => {
                c = ix;
                d = iy;
            }
            BoxHandle::SW => {
                a = ix;
                d = iy;
            }
        }
    } else {
        let (fx, fy) = match handle {
            BoxHandle::SE => (a, b),
            BoxHandle::NW => (c, d),
            BoxHandle::NE => (a, d),
            BoxHandle::SW => (c, b),
            BoxHandle::E => (a, (b + d) * 0.5),
            BoxHandle::W => (c, (b + d) * 0.5),
            BoxHandle::N => ((a + c) * 0.5, d),
            BoxHandle::S => ((a + c) * 0.5, b),
        };
        let axis = |cur: f32, start: f32, fixed: f32| {
            let den = start - fixed;
            if den.abs() < 0.5 {
                1.0
            } else {
                (cur - fixed) / den
            }
        };
        let s = match handle {
            BoxHandle::E | BoxHandle::W => axis(ix, start_x, fx).abs(),
            BoxHandle::N | BoxHandle::S => axis(iy, start_y, fy).abs(),
            _ => axis(ix, start_x, fx).abs().max(axis(iy, start_y, fy).abs()),
        }
        .max(0.02);
        let nw = ow * s;
        let nh = oh * s;
        match handle {
            BoxHandle::E => {
                a = fx;
                c = fx + nw;
                b = fy - nh * 0.5;
                d = fy + nh * 0.5;
            }
            BoxHandle::W => {
                c = fx;
                a = fx - nw;
                b = fy - nh * 0.5;
                d = fy + nh * 0.5;
            }
            BoxHandle::N => {
                d = fy;
                b = fy - nh;
                a = fx - nw * 0.5;
                c = fx + nw * 0.5;
            }
            BoxHandle::S => {
                b = fy;
                d = fy + nh;
                a = fx - nw * 0.5;
                c = fx + nw * 0.5;
            }
            BoxHandle::SE => {
                a = fx;
                b = fy;
                c = fx + nw;
                d = fy + nh;
            }
            BoxHandle::NW => {
                c = fx;
                d = fy;
                a = fx - nw;
                b = fy - nh;
            }
            BoxHandle::NE => {
                a = fx;
                d = fy;
                c = fx + nw;
                b = fy - nh;
            }
            BoxHandle::SW => {
                c = fx;
                b = fy;
                a = fx - nw;
                d = fy + nh;
            }
        }
    }
    if c < a {
        std::mem::swap(&mut a, &mut c);
    }
    if d < b {
        std::mem::swap(&mut b, &mut d);
    }
    (a, b, (c - a).max(1.0) + a, (d - b).max(1.0) + b)
}

pub(crate) fn box_handle(
    ix: f32,
    iy: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    scale: f32,
) -> Option<BoxHandle> {
    let a = x0.min(x1);
    let b = y0.min(y1);
    let c = x0.max(x1);
    let d = y0.max(y1);
    let tol = (10.0 / scale.max(0.0001)).max(3.0);
    let in_x = ix >= a - tol && ix <= c + tol;
    let in_y = iy >= b - tol && iy <= d + tol;
    if !in_x || !in_y {
        return None;
    }
    let corner = (tol * 1.35).max(tol);
    let near_l = (ix - a).abs() <= tol;
    let near_r = (ix - c).abs() <= tol;
    let near_t = (iy - b).abs() <= tol;
    let near_b = (iy - d).abs() <= tol;
    let corner_l = (ix - a).abs() <= corner;
    let corner_r = (ix - c).abs() <= corner;
    let corner_t = (iy - b).abs() <= corner;
    let corner_b = (iy - d).abs() <= corner;
    match (near_t, near_b, near_l, near_r) {
        _ if corner_t && corner_l => Some(BoxHandle::NW),
        _ if corner_t && corner_r => Some(BoxHandle::NE),
        _ if corner_b && corner_l => Some(BoxHandle::SW),
        _ if corner_b && corner_r => Some(BoxHandle::SE),
        (true, _, true, _) => Some(BoxHandle::NW),
        (true, _, _, true) => Some(BoxHandle::NE),
        (_, true, true, _) => Some(BoxHandle::SW),
        (_, true, _, true) => Some(BoxHandle::SE),
        (true, _, _, _) => Some(BoxHandle::N),
        (_, true, _, _) => Some(BoxHandle::S),
        (_, _, true, _) => Some(BoxHandle::W),
        (_, _, _, true) => Some(BoxHandle::E),
        _ => None,
    }
}

pub(crate) fn shortest_arc_rad(from: f32, to: f32) -> f32 {
    let pi = std::f32::consts::PI;
    let tau = 2.0 * pi;
    (to - from + pi).rem_euclid(tau) - pi
}

pub(crate) fn snap_deg(deg: f32, step: f32) -> f32 {
    let step = step.max(0.01);
    (deg / step).round() * step
}

pub(crate) fn snap_to_targets(v: f32, targets: &[f32], tol: f32) -> f32 {
    let mut best = v;
    let mut best_d = tol;
    for &t in targets {
        let d = (v - t).abs();
        if d <= best_d {
            best_d = d;
            best = t;
        }
    }
    best
}

/// 画布拖边: 吸附到拖开始尺寸 (`orig_w/h`, 上一次位置) 和内容包围盒 (原来的边界).
pub(crate) fn snap_canvas_resize(
    edge: CanvasEdge,
    orig_w: u32,
    orig_h: u32,
    start: (f32, f32),
    ix: f32,
    iy: f32,
    content: Option<(f32, f32, f32, f32)>,
    tol: f32,
) -> (i32, i32, i32, i32) {
    let (sx, sy) = start;
    let mut w = orig_w as i32;
    let mut h = orig_h as i32;
    let mut ox = 0i32;
    let mut oy = 0i32;
    match edge {
        CanvasEdge::Right => {
            let mut right = orig_w as f32 + (ix - sx);
            let mut xs = [orig_w as f32, 0.0];
            let n = if let Some((_, _, mx, _)) = content {
                xs[1] = mx;
                2
            } else {
                1
            };
            right = snap_to_targets(right, &xs[..n], tol);
            w = right.round() as i32;
        }
        CanvasEdge::Left => {
            let mut left = ix - sx;
            let mut xs = [0.0, 0.0];
            let n = if let Some((mn, _, _, _)) = content {
                xs[1] = mn;
                2
            } else {
                1
            };
            left = snap_to_targets(left, &xs[..n], tol);
            ox = left.round() as i32;
            w = orig_w as i32 - ox;
        }
        CanvasEdge::Bottom => {
            let mut bottom = orig_h as f32 + (iy - sy);
            let mut ys = [orig_h as f32, 0.0];
            let n = if let Some((_, _, _, my)) = content {
                ys[1] = my;
                2
            } else {
                1
            };
            bottom = snap_to_targets(bottom, &ys[..n], tol);
            h = bottom.round() as i32;
        }
        CanvasEdge::Top => {
            let mut top = iy - sy;
            let mut ys = [0.0, 0.0];
            let n = if let Some((_, mn, _, _)) = content {
                ys[1] = mn;
                2
            } else {
                1
            };
            top = snap_to_targets(top, &ys[..n], tol);
            oy = top.round() as i32;
            h = orig_h as i32 - oy;
        }
    }
    (w.max(1), h.max(1), ox, oy)
}

pub(crate) fn opposite_handle_point(
    handle: BoxHandle,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) -> (f32, f32) {
    let a = x0.min(x1);
    let b = y0.min(y1);
    let c = x0.max(x1);
    let d = y0.max(y1);
    match handle {
        BoxHandle::SE => (a, b),
        BoxHandle::NW => (c, d),
        BoxHandle::NE => (a, d),
        BoxHandle::SW => (c, b),
        BoxHandle::E => (a, (b + d) * 0.5),
        BoxHandle::W => (c, (b + d) * 0.5),
        BoxHandle::N => ((a + c) * 0.5, d),
        BoxHandle::S => ((a + c) * 0.5, b),
    }
}

/// 缩放手柄再往外一点: 旋转热区. 命中缩放手柄时不算旋转.
/// 返回热区朝外的方位角 (y 向下, 0 为右, 顺时针), 用来转旋转光标.
pub(crate) fn box_rotate_hit(
    ix: f32,
    iy: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    scale: f32,
) -> Option<f32> {
    if box_handle(ix, iy, x0, y0, x1, y1, scale).is_some() {
        return None;
    }
    let a = x0.min(x1);
    let b = y0.min(y1);
    let c = x0.max(x1);
    let d = y0.max(y1);
    let mx = (a + c) * 0.5;
    let my = (b + d) * 0.5;
    let out = (14.0 / scale.max(0.0001)).max(5.0);
    let rad = (11.0 / scale.max(0.0001)).max(4.0);
    let spots = [
        (a, b),
        (c, b),
        (a, d),
        (c, d),
        (mx, b),
        (mx, d),
        (a, my),
        (c, my),
    ];
    let mut best: Option<(f32, f32)> = None;
    for (hx, hy) in spots {
        let dx = hx - mx;
        let dy = hy - my;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let rx = hx + dx / len * out;
        let ry = hy + dy / len * out;
        let dist = (ix - rx).hypot(iy - ry);
        if dist <= rad && best.map(|(d, _)| dist < d).unwrap_or(true) {
            best = Some((dist, dy.atan2(dx)));
        }
    }
    best.map(|(_, ang)| ang)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanvasEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// 宿主注入的装页数据.
#[derive(Clone, Debug, Default)]
pub struct FitHint {
    pub other_h: u32,
    pub sheet_w: u32,
    pub aspect_w: u32,
    pub aspect_h: u32,
    pub bg_enabled: bool,
}

impl FitHint {
    pub fn page_h(&self, canvas_w: u32) -> u32 {
        let w = if self.bg_enabled {
            self.sheet_w.max(canvas_w).max(1)
        } else {
            self.sheet_w.max(canvas_w).max(1)
        };
        let aw = self.aspect_w.max(1);
        let ah = self.aspect_h.max(1);
        ((w as u64 * ah as u64) / aw as u64).max(1) as u32
    }

    pub fn scale_for(&self, canvas_w: u32, canvas_h: u32) -> f32 {
        if !self.bg_enabled {
            return 1.0;
        }
        let sheet_h = self.other_h.saturating_add(canvas_h);
        let page_h = self.page_h(canvas_w);
        if sheet_h > page_h {
            page_h as f32 / sheet_h as f32
        } else {
            1.0
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImportOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub enum HostCmd {
    Apply,
    Cancel,
    Restore,
    RequestImport(String),
}

#[derive(Clone)]
pub(crate) struct TileSprite {
    pub tex: std::sync::Arc<gpui::RenderImage>,
    /// 逻辑块左上角 (图层像素), 不含渗色边.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// 贴图在逻辑块四周多出来的像素, 用来挡住图集线性过滤的边缘渗色.
    pub pad_l: u32,
    pub pad_t: u32,
    pub pad_r: u32,
    pub pad_b: u32,
}

pub(crate) struct LayerPaint {
    pub tiles: Vec<TileSprite>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub active: bool,
    pub rotate_deg: f32,
}

pub struct Commit {
    pub document: crate::document::EditDocument,
    pub flat: image::RgbImage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_edge_no_jump_at_press() {
        let (a, b, c, d) = resize_rect(
            0.0,
            0.0,
            100.0,
            50.0,
            96.0,
            22.0,
            96.0,
            22.0,
            BoxHandle::E,
            true,
        );
        assert!((a - 0.0).abs() < 0.2);
        assert!((c - 100.0).abs() < 0.2);
        assert!((b - 0.0).abs() < 0.2);
        assert!((d - 50.0).abs() < 0.2);
    }

    #[test]
    fn lock_east_anchors_left_mid() {
        let (a, b, c, d) = resize_rect(
            0.0,
            0.0,
            100.0,
            50.0,
            200.0,
            25.0,
            100.0,
            25.0,
            BoxHandle::E,
            true,
        );
        assert!((a - 0.0).abs() < 0.2);
        assert!((c - 200.0).abs() < 0.2);
        assert!((b - (-25.0)).abs() < 0.2);
        assert!((d - 75.0).abs() < 0.2);
    }

    #[test]
    fn lock_north_anchors_bottom_mid() {
        let (a, b, c, d) = resize_rect(
            0.0,
            0.0,
            100.0,
            50.0,
            50.0,
            -50.0,
            50.0,
            0.0,
            BoxHandle::N,
            true,
        );
        assert!((b - (-50.0)).abs() < 0.2);
        assert!((d - 50.0).abs() < 0.2);
        assert!((a - (-50.0)).abs() < 0.2);
        assert!((c - 150.0).abs() < 0.2);
    }

    #[test]
    fn log_slider_roundtrip() {
        let min = 2.0;
        let max = 400.0;
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let size = brush_size_from_t(t, min, max);
            let back = brush_size_to_t(size, min, max);
            assert!((back - t).abs() < 1e-5, "t={t} size={size} back={back}");
        }
        assert!((brush_size_from_t(0.0, min, max) - min).abs() < 1e-5);
        assert!((brush_size_from_t(1.0, min, max) - max).abs() < 1e-5);
    }

    #[test]
    fn log_slider_grows_faster_on_the_right() {
        let min = 2.0;
        let max = 400.0;
        let left = brush_size_from_t(0.4, min, max) - brush_size_from_t(0.2, min, max);
        let right = brush_size_from_t(0.8, min, max) - brush_size_from_t(0.6, min, max);
        assert!(right > left * 2.0, "left={left} right={right}");
        let mid = brush_size_from_t(0.5, min, max);
        assert!((mid - (min * max).sqrt()).abs() < 1e-3);
    }

    #[test]
    fn brush_max_is_tenth_of_image_width() {
        assert_eq!(brush_size_max_for_image(0), BRUSH_SIZE_FALLBACK_MAX);
        assert_eq!(brush_size_max_for_image(2000), 200.0);
        assert_eq!(brush_size_max_for_image(4000), 400.0);
    }

    #[test]
    fn axis_lock_picks_dominant_axis() {
        assert_eq!(axis_lock_delta(10, 3), (10, 0));
        assert_eq!(axis_lock_delta(-4, 9), (0, 9));
        assert_eq!(axis_lock_delta(5, -5), (5, 0));
        assert_eq!(axis_lock_delta(0, 0), (0, 0));
    }

    #[test]
    fn rotate_hotspot_is_outside_scale_handle() {
        let scale = 1.0;
        assert!(box_handle(0.0, 0.0, 0.0, 0.0, 100.0, 50.0, scale).is_some());
        assert!(box_rotate_hit(0.0, 0.0, 0.0, 0.0, 100.0, 50.0, scale).is_none());
        let nw = box_rotate_hit(-16.0, -16.0, 0.0, 0.0, 100.0, 50.0, scale);
        assert!(nw.is_some());
        let ang = nw.unwrap();
        assert!(
            ang < -std::f32::consts::FRAC_PI_2 && ang > -std::f32::consts::PI,
            "NW outward {ang}"
        );
        assert!(box_rotate_hit(50.0, 25.0, 0.0, 0.0, 100.0, 50.0, scale).is_none());
        let e = box_rotate_hit(114.0, 25.0, 0.0, 0.0, 100.0, 50.0, scale).unwrap();
        assert!(e.abs() < 0.2, "E outward {e}");
    }

    #[test]
    fn shortest_arc_crosses_180() {
        let pi = std::f32::consts::PI;
        let step = shortest_arc_rad(pi - 0.1, -pi + 0.1);
        assert!(
            step > 0.0 && step < 0.3,
            "should keep going past 180°, got {step}"
        );
        let back = shortest_arc_rad(-pi + 0.1, pi - 0.1);
        assert!(
            back < 0.0 && back > -0.3,
            "should keep going the other way, got {back}"
        );
    }

    #[test]
    fn shift_snaps_total_not_delta() {
        let base = 40.0;
        let extra = 8.0;
        let snapped = snap_deg(base + extra, 15.0);
        assert!((snapped - 45.0).abs() < 1e-4);
        assert!(((snapped - base) - 5.0).abs() < 1e-4);
    }

    #[test]
    fn snap_to_orig_and_canvas_edges() {
        let targets = [0.0, 99.0, 20.0, 80.0];
        assert!((snap_to_targets(2.0, &targets, 4.0) - 0.0).abs() < 1e-4);
        assert!((snap_to_targets(78.5, &targets, 4.0) - 80.0).abs() < 1e-4);
        assert!((snap_to_targets(50.0, &targets, 4.0) - 50.0).abs() < 1e-4);
    }

    #[test]
    fn canvas_snaps_to_orig_size_and_content() {
        let content = Some((10.0, 5.0, 90.0, 80.0));
        let (w, h, ox, oy) = snap_canvas_resize(
            CanvasEdge::Right,
            100,
            50,
            (100.0, 25.0),
            102.0,
            25.0,
            content,
            4.0,
        );
        assert_eq!((w, h, ox, oy), (100, 50, 0, 0));
        let (w, ..) = snap_canvas_resize(
            CanvasEdge::Right,
            100,
            50,
            (100.0, 25.0),
            88.0,
            25.0,
            content,
            4.0,
        );
        assert_eq!(w, 90);
        let (w, _, ox, _) = snap_canvas_resize(
            CanvasEdge::Left,
            100,
            50,
            (0.0, 25.0),
            2.0,
            25.0,
            content,
            4.0,
        );
        assert_eq!((ox, w), (0, 100));
        let (_, _, ox, _) = snap_canvas_resize(
            CanvasEdge::Left,
            100,
            50,
            (0.0, 25.0),
            11.0,
            25.0,
            content,
            4.0,
        );
        assert_eq!(ox, 10);
    }

    #[test]
    fn ring_uses_under_not_center_invert() {
        let white = [250u8, 250, 250];
        let black = [12u8, 12, 12];
        let samples = [white, white, white, white, white, white, white, white];
        let style = ring_style_from_under(&samples, Some(black));
        assert_eq!(
            style.ring,
            [0, 0, 0],
            "外圈下是白纸, 描边该是黑而不是反色成白"
        );
        let halo = style.halo.expect("黑填充贴近黑圈时加反色内外描边");
        assert_eq!(halo, [255, 255, 255]);
    }

    #[test]
    fn ring_uniform_paper_no_extra_halo() {
        let white = [250u8, 250, 250];
        let samples = [white; 8];
        let style = ring_style_from_under(&samples, None);
        assert_eq!(style.ring, [0, 0, 0]);
        assert_eq!(style.halo, None);
    }

    #[test]
    fn ring_mixed_staff_gets_halo() {
        let mut samples = [[240u8, 240, 240]; 12];
        samples[0] = [20, 20, 20];
        samples[1] = [18, 18, 18];
        samples[6] = [15, 15, 15];
        samples[7] = [22, 22, 22];
        let style = ring_style_from_under(&samples, None);
        assert!(style.halo.is_some(), "外圈下黑白混杂应加反色描边");
    }

    #[test]
    fn display_reorder_moves_top_layer_to_bottom() {
        let mut v = vec!['a', 'b', 'c'];
        apply_display_reorder(&mut v, 0, 2);
        assert_eq!(v, vec!['c', 'a', 'b']);
    }
}
