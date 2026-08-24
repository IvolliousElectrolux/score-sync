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
/// 「移动分块」模式下, 命中块上下边界线的容差 (屏幕像素).
pub(crate) const BLOCK_EDGE_HIT_PX: f32 = 8.0;
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
    Brush {
        id: String,
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
    /// 「移动分块」: 整体拖动一个块上下移动 (只改它自己的 gap_before).
    BlockMove {
        region_id: String,
        start_iy: f32,
        start_gap_before: i32,
    },
    /// 「移动分块」: 拖动块的上边界 (裁剪/扩展).
    BlockResizeTop {
        region_id: String,
        start_iy: f32,
        start_extra_top: i32,
        max_trim: i32,
    },
    /// 「移动分块」: 拖动块的下边界 (裁剪/扩展).
    BlockResizeBottom {
        region_id: String,
        start_iy: f32,
        start_extra_bottom: i32,
        max_trim: i32,
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
    /// 移动/拉伸「组合分块」: 拖动块本体上下移动, 拖动上下边界裁剪/扩展;
    /// 该模式下只能上下操作, 不能左右移动, 也不响应蒙版绘制/选中.
    MoveBlocks,
}

#[derive(Clone, Default)]
pub(crate) struct MaskHistory {
    pub(crate) undo: Vec<Vec<MaskRect>>,
    pub(crate) redo: Vec<Vec<MaskRect>>,
}
