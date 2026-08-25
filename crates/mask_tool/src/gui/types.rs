//! 常量, 坐标变换, 拖拽/工具枚举.

use super::*;

pub(crate) const HISTORY_LIMIT: usize = 64;
/// 画笔粗细 (直径, 图像像素) 的可调范围.
pub(crate) const BRUSH_SIZE_MIN: f32 = 2.0;
pub(crate) const BRUSH_SIZE_MAX: f32 = 80.0;
pub(crate) const BRUSH_SIZE_DEFAULT: f32 = 16.0;
/// 折线闭环: 距首点多少屏幕像素内吸附.
pub(crate) const POLY_SNAP_SCREEN_PX: f32 = 12.0;
/// 橡皮: 超过此图像像素位移才视为拖擦 (否则为单击擦顶层).
pub(crate) const ERASE_DRAG_SLOP_IMG: f32 = 3.0;
/// 拖动「组合分块」时, 命中块上下边界线的容差 (屏幕像素).
pub(crate) const BLOCK_EDGE_HIT_PX: f32 = 8.0;
/// 拖动「组合分块」边界/间距时, 靠近 0 的吸附容差 (图像像素).
/// 只吸附正在拖的那一侧, 其它块按守恒保持不动.
pub(crate) const BLOCK_SNAP_ZERO_IMG: i32 = 6;
/// 选色盘 SB 区边长 (屏幕像素).
pub(crate) const SB_SIZE: f32 = 168.0;
pub(crate) const HUE_BAR_W: f32 = 18.0;
pub(crate) const SB_TEX_SIZE: u32 = 128;
pub(crate) const HUE_TEX_H: u32 = 256;

pub(crate) fn color_rgb_u32(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
}

/// 画笔光标边框: 取 RGB 反色; 反色太接近时改用黑/白, 保证白笔也能看清.
pub(crate) fn opposite_rgb(c: [u8; 3]) -> [u8; 3] {
    let inv = [255 - c[0], 255 - c[1], 255 - c[2]];
    let dist = (inv[0] as i16 - c[0] as i16).unsigned_abs()
        + (inv[1] as i16 - c[1] as i16).unsigned_abs()
        + (inv[2] as i16 - c[2] as i16).unsigned_abs();
    if dist < 180 {
        let y = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
        if y >= 128.0 {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        }
    } else {
        inv
    }
}

/// 滴管 / 取色图标 (约 14×14 视口内绘制).
pub(crate) fn eyedropper_icon(active: bool) -> impl IntoElement {
    let stroke = if active {
        rgb(0xf8fafc)
    } else {
        rgb(0xe2e8f0)
    };
    div()
        .size(px(14.))
        .flex_shrink_0()
        .child(
            canvas(|_, _, _| {}, {
                move |bounds, _, window, _| {
                    let ox = f32::from(bounds.origin.x);
                    let oy = f32::from(bounds.origin.y);
                    let s = f32::from(bounds.size.width)
                        .min(f32::from(bounds.size.height))
                        .max(1.0);
                    let p = |x: f32, y: f32| {
                        point(px(ox + x / 16.0 * s), px(oy + y / 16.0 * s))
                    };
                    let thick = px((1.4_f32 * s / 14.0).max(1.0));
                    // 笔杆
                    let mut shaft = PathBuilder::stroke(thick);
                    shaft.move_to(p(3.2, 12.8));
                    shaft.line_to(p(10.2, 5.8));
                    if let Ok(path) = shaft.build() {
                        window.paint_path(path, stroke);
                    }
                    // 笔尖 V
                    let mut tip = PathBuilder::stroke(thick);
                    tip.move_to(p(2.0, 11.2));
                    tip.line_to(p(3.2, 12.8));
                    tip.line_to(p(4.8, 11.4));
                    if let Ok(path) = tip.build() {
                        window.paint_path(path, stroke);
                    }
                    // 顶部笔头 / 储液
                    let mut bulb = PathBuilder::stroke(thick);
                    bulb.move_to(p(9.0, 4.6));
                    bulb.line_to(p(11.0, 2.6));
                    bulb.line_to(p(13.2, 4.8));
                    bulb.line_to(p(11.2, 6.8));
                    bulb.close();
                    if let Ok(path) = bulb.build() {
                        window.paint_path(path, stroke);
                    }
                    // 一小滴
                    let drop = Bounds {
                        origin: p(2.4, 13.0),
                        size: size(px(2.2 / 16.0 * s), px(2.2 / 16.0 * s)),
                    };
                    window.paint_quad(quad(
                        drop,
                        px(1.2 / 16.0 * s),
                        stroke,
                        px(0.),
                        stroke,
                        Default::default(),
                    ));
                }
            })
            .size_full(),
        )
}

/// 预览用画笔: 沿折线叠圆形章 (与导出 `stamp_polyline` 同模型).
/// 避免 PathBuilder::stroke 在折返/自交时因 miter 尖角撕出畸形大块.
pub(crate) fn paint_brush_stamps(
    window: &mut Window,
    points: &[(i32, i32)],
    radius_img: f32,
    scale: f32,
    origin_x: f32,
    origin_y: f32,
    view_origin: Point<Pixels>,
    diam_screen: f32,
    fill: gpui::Rgba,
) {
    if points.is_empty() || diam_screen < 0.5 {
        return;
    }
    let to_screen = |ix: f32, iy: f32| -> (f32, f32) {
        (
            f32::from(view_origin.x) + origin_x + ix * scale,
            f32::from(view_origin.y) + origin_y + iy * scale,
        )
    };
    let paint_disk = |window: &mut Window, cx: f32, cy: f32| {
        let b = Bounds {
            origin: point(px(cx - diam_screen * 0.5), px(cy - diam_screen * 0.5)),
            size: size(px(diam_screen), px(diam_screen)),
        };
        window.paint_quad(quad(
            b,
            px(diam_screen * 0.5),
            fill,
            px(0.),
            fill,
            Default::default(),
        ));
    };
    let step_img = (radius_img * 0.5).max(1.0);
    let (sx, sy) = to_screen(points[0].0 as f32, points[0].1 as f32);
    paint_disk(window, sx, sy);
    for w in points.windows(2) {
        let (x0, y0) = (w[0].0 as f32, w[0].1 as f32);
        let (x1, y1) = (w[1].0 as f32, w[1].1 as f32);
        let dist = (x1 - x0).hypot(y1 - y0).max(0.001);
        let n = (dist / step_img).ceil() as i32;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let (sx, sy) = to_screen(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            paint_disk(window, sx, sy);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorPickerTarget {
    Mask,
    Brush,
}

#[derive(Clone, Copy)]
pub(crate) struct ViewXform {
    pub(crate) scale: f32,
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
}

impl ViewXform {
    pub(crate) fn compute(
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
        let drawn_w = img_w * scale;
        let drawn_h = img_h * scale;
        Self {
            scale,
            origin_x: (view_w - drawn_w) * 0.5 + pan.x,
            origin_y: (view_h - drawn_h) * 0.5 + pan.y,
        }
    }

    pub(crate) fn screen_to_image(&self, sx: f32, sy: f32) -> (f32, f32) {
        ((sx - self.origin_x) / self.scale, (sy - self.origin_y) / self.scale)
    }

    pub(crate) fn image_to_screen(&self, ix: f32, iy: f32) -> (f32, f32) {
        (
            self.origin_x + ix * self.scale,
            self.origin_y + iy * self.scale,
        )
    }

    pub(crate) fn edge_tol(&self) -> f32 {
        (BLOCK_EDGE_HIT_PX / self.scale).max(1.0)
    }

    pub(crate) fn image_rect_to_screen(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds<Pixels> {
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
    Draw {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    /// 画笔描边: 正在编辑的蒙版 id; `undid` 表示本笔是否已压入撤销栈.
    /// `start_iy`: 落笔起点的画布纵坐标, 松开时与终点一起判定绑定哪个
    /// 「组合分块」成员 (见 `MaskToolApp::resolve_bound_block`).
    Brush {
        id: String,
        start_iy: f32,
        undid: bool,
    },
    /// 平移模式: 空白处拖动画布
    PagePan {
        last: Point<Pixels>,
    },
    /// 平移模式: 拖动已选蒙版
    MoveMasks {
        last_ix: f32,
        last_iy: f32,
        undid: bool,
    },
    /// 无模式: Shift 拖选
    Marquee {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        additive: bool,
    },
    BrushSize,
    /// 选色盘内: 透明度 / SB / 色相
    PaletteOpacity,
    PaletteSb,
    PaletteHue,
    /// 橡皮: `wiping` 为 true 表示已进入拖擦 (擦光); 否则 mouse up 时点擦顶层.
    Erase {
        start_ix: f32,
        start_iy: f32,
        undid: bool,
        wiping: bool,
    },
    /// 「移动分块」: 整体拖动一个块上下移动, 优先消耗被拖动块与相邻块
    /// 之间*已有*的间距, 只有真的撞上了才会继续波及下一个/上一个块, 见
    /// `crate::layout::redistribute_for_block_move` 文档.
    ///
    /// `start_layout` 是拖动起点时的完整快照 (每帧都从这份快照重新分配,
    /// 不做增量累加, 避免多帧误差累积). `start_voff` 是拖动起点时的
    /// `block_voff` (画面当前的底色居中纵向偏移); 每帧先折进第一块
    /// `gap_before` 变成页面绝对坐标 (y=0 = 页顶), 再按绝对坐标分配.
    /// `undid`: 真正发生位移的第一帧才 push 一次撤销快照 (单击不动不占
    /// 撤销栈).
    BlockMove {
        region_id: String,
        start_iy: f32,
        start_layout: Vec<BlockAdjust>,
        start_voff: i32,
        undid: bool,
    },
    /// 「移动分块」: 拖动块的上边界 (裁剪/扩展). 与下边界不同, 上边界要
    /// 让"边线跟手"(边线本身跟着鼠标移动), 同时块自己的底边及往后所有
    /// 块的位置分毫不动 (与下边界"顶边及往前所有块不动"完全镜像) —— 这
    /// 需要在改 `extra_top` 的同时, 用 `gap_before` 反向同步调整.
    /// 每帧从 `start_layout` / `start_voff` 折成页面绝对坐标后再算, 最上方
    /// 块的上边界可以一直拖到页顶 (Y=0), 到顶即停.
    BlockResizeTop {
        region_id: String,
        start_iy: f32,
        start_layout: Vec<BlockAdjust>,
        start_voff: i32,
        max_trim: i32,
        undid: bool,
    },
    /// 「移动分块」: 拖动块的下边界. 先消耗与下一块之间的空白 (最后一块
    /// 则消耗末端留白), 其它块绝对位置不动; 贴住之后才挤开下一块.
    BlockResizeBottom {
        region_id: String,
        start_iy: f32,
        start_layout: Vec<BlockAdjust>,
        start_voff: i32,
        max_trim: i32,
        undid: bool,
    },
    /// 拖动辅助线. 始终按「全局按比例联动」: `orig_lines` 是拖动开始时
    /// 全部辅助线的原始位置快照, 每帧从原始值重新算比例, 避免逐帧复合
    /// 缩放导致越拖越飘.
    GuideMove {
        idx: usize,
        start_y: i32,
        orig_lines: Vec<i32>,
        undid: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolMode {
    /// 两模式都关: 只能选中 (含 Ctrl 多选 / Shift 拖选), 不能拖动画布
    Select,
    /// 框选新蒙版
    Draw,
    /// 折线多边形: 逐点连直线, 吸附首点闭环 (类似 PS 钢笔勾形)
    Poly,
    /// 画笔描边 (自由绘制, 可调颜色/粗细)
    Brush,
    /// 橡皮: 单击擦最上层, 拖动擦光碰到的全部
    Eraser,
    /// 空白拖动画布; 点在已选蒙版上则拖动蒙版
    Pan,
}

/// 一次撤销/重做快照: 同时包含蒙版、「组合分块」位置/尺寸调整与该组合的
/// 纵向目标位置 (`voff_target`, 见 `MaskToolApp::voff_target` 字段文档),
/// 三者共用同一条时间线 (哪个先改就先进撤销栈, `Ctrl+Z`/`Ctrl+Y` 统一
/// 处理). 把 `voff_target` 也纳入快照是为了让撤销/重做能精确恢复到当时
/// 的底色居中位置, 不依赖任何"重新推算"——尤其是拼合图高度跨越
/// `apply_bg::process::frame_size` 的宽高比切换分界点前后, 若不整体
/// 回滚这个值就可能在撤销后停在与原来不同的居中位置上.
#[derive(Clone, Default)]
pub(crate) struct UndoSnapshot {
    pub(crate) masks: Vec<MaskRect>,
    pub(crate) block_layout: Vec<BlockAdjust>,
    pub(crate) voff_target: i64,
    /// 本组合的辅助线 (位置 + 锁定态), 与蒙版/分块调整共用同一条撤重
    /// 时间线, 见 `MaskToolApp::snapshot`.
    pub(crate) guides: GuideState,
    /// 宿主全局辅助线操作的令牌: 撤/重这条快照时请宿主同步全部组合的
    /// 线/开关 (以及对齐带来的布局). `None` 表示纯本页编辑.
    pub(crate) host_guide_token: Option<u64>,
}

#[derive(Clone, Default)]
pub(crate) struct MaskHistory {
    pub(crate) undo: Vec<UndoSnapshot>,
    pub(crate) redo: Vec<UndoSnapshot>,
}
