use gpui::{Bounds, Pixels, Point};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolMode {
    Select,
    Node,
    Canvas,
    Hand,
    Pen,
    Pencil,
    Eraser,
    Lasso,
}

impl ToolMode {
    pub fn label(self) -> &'static str {
        match self {
            ToolMode::Select => "选择",
            ToolMode::Node => "节点",
            ToolMode::Canvas => "画布",
            ToolMode::Hand => "手形",
            ToolMode::Pen => "钢笔",
            ToolMode::Pencil => "铅笔",
            ToolMode::Eraser => "橡皮",
            ToolMode::Lasso => "套索",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ToolMode::Select => "V 点选或框选完全包住的元素. 点组合先选整组, 再点一次选里面的元素. Shift 追加, Ctrl 切换, Alt 拖动复制, Ctrl+A 全选",
            ToolMode::Node => "A 拖锚点, 双击平滑/尖角, Delete 删点, 点另一条路径可切换",
            ToolMode::Canvas => "K 拖四边改画布大小",
            ToolMode::Hand => "H 或按住空格拖动平移, 滚轮平移, Ctrl 滚轮缩放",
            ToolMode::Pen => "P 单击加点, 拖出手柄, 靠近起点会吸附并闭合, Enter 结束",
            ToolMode::Pencil => "B 按住描绘, Alt 滚轮改粗细, Alt 单击吸取",
            ToolMode::Eraser => "E 十字准星对准一个元素, 按下就删掉这一个",
            ToolMode::Lasso => "L 圈住完全包在里面的元素. Shift 追加, Ctrl 切换, Alt 去掉",
        }
    }
}

/// 右键菜单. `x`/`y` 相对它所在的那一块 (选择窗格或画布). `on_canvas` 为真时画在左边.
#[derive(Clone, Debug)]
pub(crate) struct OutlineMenu {
    pub x: f32,
    pub y: f32,
    pub pin_right: bool,
    pub on_canvas: bool,
    pub target: String,
}

/// 一行在窗口里的范围, 用来接右键. 命中不依赖这一行的 hitbox 是否 hovered.
#[derive(Clone, Debug)]
pub(crate) struct OutlineRowHit {
    pub id: String,
    pub kind: crate::outline::RowKind,
    pub layer_index: usize,
    pub bounds: Bounds<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Debug)]
pub(crate) struct PenNode {
    pub p: (f32, f32),
    pub hin: Option<(f32, f32)>,
    pub hout: Option<(f32, f32)>,
}

#[derive(Clone, Debug)]
pub(crate) struct PenDraft {
    pub nodes: Vec<PenNode>,
    pub continue_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum DragKind {
    Pan {
        origin: Point<f32>,
        pan: Point<f32>,
    },
    Marquee {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        add: bool,
        sub: bool,
        toggle: bool,
    },
    Move {
        origin: (f32, f32),
        copy: bool,
    },
    /// 钢笔按住拖出的手柄, index 是刚放下的锚点.
    PenPull {
        index: usize,
    },
    Scale {
        handle: usize,
        bounds: crate::geom::Rect,
        sx: f32,
        sy: f32,
    },
    Rotate {
        center: (f32, f32),
        start_ang: f32,
        #[allow(dead_code)]
        acc: f32,
    },
    Node {
        id: String,
        sub: usize,
        seg: usize,
        role: NodeRole,
        last: (f32, f32),
    },
    Pencil {
        pts: Vec<(f32, f32)>,
    },
    Canvas {
        edge: CanvasEdge,
        orig_w: f32,
        orig_h: f32,
        origin: (f32, f32),
        scale: f32,
        new_w: f32,
        new_h: f32,
        /// 新画布左上角, 用拖之前的文档坐标. 上边和左边跟着指针, 对边不动.
        shift_x: f32,
        shift_y: f32,
    },
    Lasso {
        pts: Vec<(f32, f32)>,
    },
    PaletteSb,
    PaletteHue,
    PaletteAlpha,
    Slider(SliderKind),
    /// `from`/`to` 是列表上的显示下标, 0 在最上面.
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
    /// 图层面板滚动条. `grab` 是指针在滑块内的纵向偏移.
    OutlineScroll {
        grab: f32,
    },
}

pub(crate) const REORDER_DRAG_SLOP: f32 = 5.0;

pub(crate) fn reorder_slop_exceeded(dx: f32, dy: f32) -> bool {
    dx * dx + dy * dy >= REORDER_DRAG_SLOP * REORDER_DRAG_SLOP
}

/// 显示列表里, 把 `from` 插到 `anchor` 的上沿或下沿之后的新下标.
pub(crate) fn reorder_to_index(from: usize, anchor: usize, after: bool) -> usize {
    if after {
        if from <= anchor { anchor } else { anchor + 1 }
    } else if from < anchor {
        anchor - 1
    } else {
        anchor
    }
}

/// 图层列表自上而下是栈顶到栈底. `from`/`to` 为显示下标.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SliderKind {
    Width,
}

pub(crate) const STROKE_MIN: f32 = 0.15;
pub(crate) const STROKE_MAX: f32 = 24.0;

pub(crate) fn stroke_from_t(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    STROKE_MIN * (STROKE_MAX / STROKE_MIN).powf(t)
}

pub(crate) fn stroke_to_t(width: f32) -> f32 {
    let width = width.clamp(STROKE_MIN, STROKE_MAX);
    let span = (STROKE_MAX / STROKE_MIN).ln();
    if span.abs() < 1e-6 {
        0.0
    } else {
        ((width / STROKE_MIN).ln() / span).clamp(0.0, 1.0)
    }
}

pub(crate) const EDGE_SNAP_SCREEN_PX: f32 = 8.0;

/// 拖边预览, 不吸附. 返回 `(新宽, 新高, 新画布左, 新画布上)`.
pub(crate) fn canvas_edge_drag(
    edge: CanvasEdge,
    orig_w: f32,
    orig_h: f32,
    dx: f32,
    dy: f32,
) -> (f32, f32, f32, f32) {
    match edge {
        CanvasEdge::Left => {
            let span = (orig_w - dx).max(1.0);
            (span, orig_h, orig_w - span, 0.0)
        }
        CanvasEdge::Right => ((orig_w + dx).max(1.0), orig_h, 0.0, 0.0),
        CanvasEdge::Top => {
            let span = (orig_h - dy).max(1.0);
            (orig_w, span, 0.0, orig_h - span)
        }
        CanvasEdge::Bottom => (orig_w, (orig_h + dy).max(1.0), 0.0, 0.0),
    }
}

/// `content` 是 `(x0, y0, x1, y1)`. `frame` 是居中目标宽高, 0 表示这一边不吸附.
pub(crate) fn canvas_edge_drag_snapped(
    edge: CanvasEdge,
    orig_w: f32,
    orig_h: f32,
    dx: f32,
    dy: f32,
    content: Option<(f32, f32, f32, f32)>,
    frame: (u32, u32),
    tol: f32,
) -> (f32, f32, f32, f32) {
    let (dx, dy) = match edge {
        CanvasEdge::Left => {
            let mut xs = [0.0, 0.0, 0.0, 0.0];
            let mut n = 1;
            if let Some((mn, _, _, _)) = content {
                xs[n] = mn;
                n += 1;
            }
            n = push_frame_edges(&mut xs, n, orig_w, frame.0);
            (snap_to_targets(dx, &xs[..n], tol), dy)
        }
        CanvasEdge::Right => {
            let mut xs = [orig_w, 0.0, 0.0, 0.0];
            let mut n = 1;
            if let Some((_, _, mx, _)) = content {
                xs[n] = mx;
                n += 1;
            }
            n = push_frame_edges(&mut xs, n, orig_w, frame.0);
            let right = snap_to_targets(orig_w + dx, &xs[..n], tol);
            (right - orig_w, dy)
        }
        CanvasEdge::Top => {
            let mut ys = [0.0, 0.0, 0.0, 0.0];
            let mut n = 1;
            if let Some((_, mn, _, _)) = content {
                ys[n] = mn;
                n += 1;
            }
            n = push_frame_edges(&mut ys, n, orig_h, frame.1);
            (dx, snap_to_targets(dy, &ys[..n], tol))
        }
        CanvasEdge::Bottom => {
            let mut ys = [orig_h, 0.0, 0.0, 0.0];
            let mut n = 1;
            if let Some((_, _, _, my)) = content {
                ys[n] = my;
                n += 1;
            }
            n = push_frame_edges(&mut ys, n, orig_h, frame.1);
            let bottom = snap_to_targets(orig_h + dy, &ys[..n], tol);
            (dx, bottom - orig_h)
        }
    };
    canvas_edge_drag(edge, orig_w, orig_h, dx, dy)
}

fn snap_to_targets(v: f32, targets: &[f32], tol: f32) -> f32 {
    if tol <= 0.0 {
        return v;
    }
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

fn push_frame_edges(xs: &mut [f32], n: usize, canvas: f32, target: u32) -> usize {
    let Some((a, b)) = target_frame_edges(canvas, target) else {
        return n;
    };
    let mut n = n;
    if n < xs.len() {
        xs[n] = a;
        n += 1;
    }
    if n < xs.len() {
        xs[n] = b;
        n += 1;
    }
    n
}

/// 画布里居中的目标宽 (或高) 的两条边. 目标不比画布小则没有内侧边.
pub(crate) fn target_frame_edges(canvas: f32, target: u32) -> Option<(f32, f32)> {
    if target == 0 || canvas <= target as f32 {
        return None;
    }
    let a = (canvas - target as f32) * 0.5;
    Some((a, a + target as f32))
}

/// 节点工具的光标. 空白是箭头, 压在对象上是手, 压在锚点或曲率手柄上是十字.
pub(crate) fn node_pointer_cursor(
    dragging: Option<NodeRole>,
    hover: Option<NodeRole>,
    over_object: bool,
) -> gpui::CursorStyle {
    if dragging.is_some() || hover.is_some() {
        gpui::CursorStyle::Crosshair
    } else if over_object {
        gpui::CursorStyle::PointingHand
    } else {
        gpui::CursorStyle::Arrow
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeRole {
    /// 路径起点 (第一段的起点). 闭合后和终点重合, 只保留终点.
    Head,
    /// 某一段的终点.
    Anchor,
    /// 本段起点伸出的手柄 (cubic p1).
    In,
    /// 本段终点收回的手柄 (cubic p2).
    Out,
}

#[derive(Clone, Debug)]
pub(crate) struct StyleBuf {
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<[u8; 4]>,
    pub width: f32,
    pub cap: crate::document::LineCap,
    pub join: crate::document::LineJoin,
}

impl Default for StyleBuf {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: Some([0, 0, 0, 255]),
            width: 1.0,
            cap: crate::document::LineCap::Butt,
            join: crate::document::LineJoin::Miter,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FitHint {
    pub other_h: u32,
    pub sheet_w: u32,
    pub aspect_w: u32,
    pub aspect_h: u32,
    pub bg_enabled: bool,
    pub snap_w: u32,
    pub snap_h: u32,
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

pub struct Commit {
    pub document: crate::document::VecDocument,
    pub preview: image::RgbaImage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_edge_follows_pointer_and_keeps_bottom() {
        let (w, h, sx, sy) = canvas_edge_drag(CanvasEdge::Top, 100.0, 80.0, 0.0, -15.0);
        assert!((w - 100.0).abs() < 0.01);
        assert!((h - 95.0).abs() < 0.01);
        assert!(sx.abs() < 0.01);
        assert!((sy + 15.0).abs() < 0.01);
        assert!((sy + h - 80.0).abs() < 0.01);

        let (w, h, sx, sy) = canvas_edge_drag(CanvasEdge::Top, 100.0, 80.0, 0.0, 12.0);
        assert!((h - 68.0).abs() < 0.01);
        assert!(sx.abs() < 0.01 && (w - 100.0).abs() < 0.01);
        assert!((sy - 12.0).abs() < 0.01);
        assert!((sy + h - 80.0).abs() < 0.01);
    }

    #[test]
    fn left_edge_follows_pointer_and_keeps_right() {
        let (w, h, sx, sy) = canvas_edge_drag(CanvasEdge::Left, 100.0, 80.0, -10.0, 0.0);
        assert!((w - 110.0).abs() < 0.01);
        assert!((h - 80.0).abs() < 0.01);
        assert!((sx + 10.0).abs() < 0.01);
        assert!(sy.abs() < 0.01);
        assert!((sx + w - 100.0).abs() < 0.01);
    }

    #[test]
    fn top_edge_snaps_to_content_and_keeps_bottom() {
        let content = Some((4.0, 10.0, 90.0, 70.0));
        let (_, h, _, sy) =
            canvas_edge_drag_snapped(CanvasEdge::Top, 100.0, 80.0, 0.0, 6.0, content, (0, 0), 4.0);
        assert!((sy - 10.0).abs() < 0.01);
        assert!((h - 70.0).abs() < 0.01);
        assert!((sy + h - 80.0).abs() < 0.01);
        let (_, h, _, sy) =
            canvas_edge_drag_snapped(CanvasEdge::Top, 100.0, 80.0, 0.0, 2.0, content, (0, 0), 4.0);
        assert!(sy.abs() < 0.01);
        assert!((h - 80.0).abs() < 0.01);
    }

    #[test]
    fn node_cursor_marks_empty_object_and_point() {
        assert_eq!(
            node_pointer_cursor(None, None, false),
            gpui::CursorStyle::Arrow
        );
        assert_eq!(
            node_pointer_cursor(None, None, true),
            gpui::CursorStyle::PointingHand
        );
        assert_eq!(
            node_pointer_cursor(None, Some(NodeRole::Anchor), false),
            gpui::CursorStyle::Crosshair
        );
        assert_eq!(
            node_pointer_cursor(None, Some(NodeRole::Out), true),
            gpui::CursorStyle::Crosshair
        );
        assert_eq!(
            node_pointer_cursor(Some(NodeRole::In), None, false),
            gpui::CursorStyle::Crosshair
        );
        assert_eq!(
            node_pointer_cursor(Some(NodeRole::Head), None, true),
            gpui::CursorStyle::Crosshair
        );
    }

    #[test]
    fn stroke_slider_roundtrip() {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let w = stroke_from_t(t);
            assert!((stroke_to_t(w) - t).abs() < 1e-4);
        }
        assert!((stroke_from_t(0.0) - STROKE_MIN).abs() < 1e-4);
        assert!((stroke_from_t(1.0) - STROKE_MAX).abs() < 1e-4);
    }
}
