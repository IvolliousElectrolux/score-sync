//! GPUI 图形界面: 框选半透明白蒙版.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    actions, canvas, div, point, prelude::*, px, quad, relative, rgb, size, App, Application,
    Bounds, Context, Corners, CursorStyle, Entity, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
use image::{Frame, ImageBuffer, Rgb, RgbaImage};
use smallvec::smallvec;

use crate::color_prefs::{
    hsv_to_rgb, rgb_to_hsv, MaskColorPrefs, DEFAULT_BRUSH_OPACITY, RECENT_COLORS_MAX,
};
use crate::mask::{
    default_export_path, export_masked, first_image_in_paths, is_image_path, new_id, MaskRect,
    DEFAULT_MASK_OPACITY,
};

actions!(
    mask_tool,
    [
        OpenFile,
        ExportImage,
        FitView,
        DeleteSelected,
        ClearMasks,
        SelectAll,
        ToggleDrawMode,
        TogglePanMode,
        ToggleBrushMode,
        TogglePolyMode,
        CancelPolyDraft,
        Undo,
        Redo
    ]
);

const HISTORY_LIMIT: usize = 64;
/// 画笔粗细 (直径, 图像像素) 的可调范围.
const BRUSH_SIZE_MIN: f32 = 2.0;
const BRUSH_SIZE_MAX: f32 = 80.0;
const BRUSH_SIZE_DEFAULT: f32 = 16.0;
/// 折线闭环: 距首点多少屏幕像素内吸附.
const POLY_SNAP_SCREEN_PX: f32 = 12.0;
/// 橡皮: 超过此图像像素位移才视为拖擦 (否则为单击擦顶层).
const ERASE_DRAG_SLOP_IMG: f32 = 3.0;
/// 选色盘 SB 区边长 (屏幕像素).
const SB_SIZE: f32 = 168.0;
const HUE_BAR_W: f32 = 18.0;
const SB_TEX_SIZE: u32 = 128;
const HUE_TEX_H: u32 = 256;

fn color_rgb_u32(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
}

/// 滴管 / 取色图标 (约 14×14 视口内绘制).
fn eyedropper_icon(active: bool) -> impl IntoElement {
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
fn paint_brush_stamps(
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
enum ColorPickerTarget {
    Mask,
    Brush,
}

#[derive(Clone, Copy)]
struct ViewXform {
    scale: f32,
    origin_x: f32,
    origin_y: f32,
}

impl ViewXform {
    fn compute(
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

    fn screen_to_image(&self, sx: f32, sy: f32) -> (f32, f32) {
        ((sx - self.origin_x) / self.scale, (sy - self.origin_y) / self.scale)
    }

    fn image_to_screen(&self, ix: f32, iy: f32) -> (f32, f32) {
        (
            self.origin_x + ix * self.scale,
            self.origin_y + iy * self.scale,
        )
    }

    fn image_rect_to_screen(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds<Pixels> {
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

enum DragKind {
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolMode {
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

#[derive(Clone, Default)]
struct MaskHistory {
    undo: Vec<Vec<MaskRect>>,
    redo: Vec<Vec<MaskRect>>,
}

pub struct MaskToolApp {
    focus_handle: FocusHandle,
    image_path: Option<PathBuf>,
    /// 按图片路径缓存蒙版, 切换页时恢复
    page_masks: std::collections::HashMap<PathBuf, Vec<MaskRect>>,
    rgb_image: Option<ImageBuffer<Rgb<u8>, Vec<u8>>>,
    render_image: Option<Arc<RenderImage>>,
    img_w: u32,
    img_h: u32,
    masks: Vec<MaskRect>,
    selected: HashSet<String>,
    /// 变更前快照栈 (Ctrl+Z)
    undo_stack: Vec<Vec<MaskRect>>,
    /// 撤销后快照栈 (Ctrl+Y)
    redo_stack: Vec<Vec<MaskRect>>,
    /// 按组合/页面会话持久化的撤重历史 (切走再回来仍可撤)
    histories: std::collections::HashMap<String, MaskHistory>,
    /// 框选/折线默认色与透明度
    mask_color: [u8; 3],
    mask_opacity: f32,
    /// 画笔默认色与透明度 (默认不透明)
    brush_color: [u8; 3],
    brush_opacity: f32,
    recent_colors: Vec<[u8; 3]>,
    mode: ToolMode,
    zoom: f32,
    pan: Point<f32>,
    user_zoomed: bool,
    view_bounds: Bounds<Pixels>,
    opacity_track: Bounds<Pixels>,
    brush_size_track: Bounds<Pixels>,
    sb_bounds: Bounds<Pixels>,
    hue_bounds: Bounds<Pixels>,
    /// 侧栏与色块锚点 (供选色悬浮窗定位)
    side_bounds: Bounds<Pixels>,
    /// 选色悬浮层自身 bounds (与 `left`/`top` 同一坐标系, 避免 padding 偏移)
    picker_layer_bounds: Bounds<Pixels>,
    mask_swatch_bounds: Bounds<Pixels>,
    brush_swatch_bounds: Bounds<Pixels>,
    /// 画笔直径 (图像像素), 半径 = size/2.
    brush_size: f32,
    color_picker_open: bool,
    color_picker_target: ColorPickerTarget,
    picker_h: f32,
    picker_s: f32,
    picker_v: f32,
    sb_image: Option<Arc<RenderImage>>,
    hue_image: Option<Arc<RenderImage>>,
    /// 调色盘 RGB 三通道文本框
    rgb_r_input: Entity<apply_bg::text_input::TextInput>,
    rgb_g_input: Entity<apply_bg::text_input::TextInput>,
    rgb_b_input: Entity<apply_bg::text_input::TextInput>,
    /// 正在从 HSV 回写 RGB 文本, 避免 observe 回环
    rgb_syncing: bool,
    /// 取色器: 已按下, 在左侧图上预览/单击确认
    eyedropper_armed: bool,
    /// 进入取色前的 HSV + RGB, Esc/右键取消时还原
    eyedropper_backup: Option<(f32, f32, f32, [u8; 3])>,
    /// 折线草稿顶点 (图像坐标); 非空表示正在勾形.
    poly_draft: Option<Vec<(f32, f32)>>,
    /// 折线橡皮筋终点 (当前鼠标图像坐标).
    poly_cursor: Option<(f32, f32)>,
    /// 画笔圆形光标中心 (图像坐标); 仅 Brush 模式跟踪.
    brush_cursor: Option<(f32, f32)>,
    /// 透明度拖动时是否已为「改选中项」压过撤销栈.
    opacity_undid: bool,
    drag: Option<DragKind>,
    status: SharedString,
    hint: SharedString,
    /// 嵌入宿主时侧栏宽度 (0 = 用默认 280)
    embed_side_width: f32,
    /// 嵌入会话键 (组内成员); 与 path 二选一标识当前图
    session_key: Option<String>,
    /// 偏好变更回调标记: 宿主可在 notify 后 flush 到 doc/appdata
    prefs_dirty: bool,
}

impl MaskToolApp {
    pub fn new(cx: &mut Context<Self>, initial: Option<PathBuf>) -> Self {
        let rgb_r_input =
            cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "255", "R").with_compact(true));
        let rgb_g_input =
            cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "255", "G").with_compact(true));
        let rgb_b_input =
            cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "255", "B").with_compact(true));
        cx.observe(&rgb_r_input, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_g_input, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_b_input, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        let mut app = Self {
            focus_handle: cx.focus_handle(),
            image_path: None,
            page_masks: std::collections::HashMap::new(),
            rgb_image: None,
            render_image: None,
            img_w: 0,
            img_h: 0,
            masks: Vec::new(),
            selected: HashSet::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            histories: std::collections::HashMap::new(),
            mask_color: [255, 255, 255],
            mask_opacity: DEFAULT_MASK_OPACITY,
            brush_color: [255, 255, 255],
            brush_opacity: DEFAULT_BRUSH_OPACITY,
            recent_colors: MaskColorPrefs::default().recent_colors,
            mode: ToolMode::Draw,
            zoom: 1.0,
            pan: point(0.0, 0.0),
            user_zoomed: false,
            view_bounds: Bounds::default(),
            opacity_track: Bounds::default(),
            brush_size_track: Bounds::default(),
            sb_bounds: Bounds::default(),
            hue_bounds: Bounds::default(),
            side_bounds: Bounds::default(),
            picker_layer_bounds: Bounds::default(),
            mask_swatch_bounds: Bounds::default(),
            brush_swatch_bounds: Bounds::default(),
            brush_size: BRUSH_SIZE_DEFAULT,
            color_picker_open: false,
            color_picker_target: ColorPickerTarget::Brush,
            picker_h: 0.0,
            picker_s: 0.0,
            picker_v: 1.0,
            sb_image: None,
            hue_image: None,
            rgb_r_input,
            rgb_g_input,
            rgb_b_input,
            rgb_syncing: false,
            eyedropper_armed: false,
            eyedropper_backup: None,
            poly_draft: None,
            poly_cursor: None,
            brush_cursor: None,
            opacity_undid: false,
            drag: None,
            status: "就绪".into(),
            hint: "框选/折线与画笔各有独立颜色与透明度 (点色块打开选色盘).\n橡皮单击擦顶层, 拖动擦光. Ctrl+Z/Y 撤重."
                .into(),
            embed_side_width: 0.0,
            session_key: None,
            prefs_dirty: false,
        };
        app.rebuild_hue_image();
        app.rebuild_sb_image();
        if let Some(path) = initial {
            app.load_image(path, cx);
        }
        app
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn set_embed_side_width(&mut self, w: f32) {
        self.embed_side_width = w;
    }

    pub fn image_path(&self) -> Option<&PathBuf> {
        self.image_path.as_ref()
    }

    /// 若路径与当前不同则载入 (嵌入宿主切页时调用).
    pub fn sync_image_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.image_path.as_ref() == Some(&path) {
            return;
        }
        self.load_image(path, cx);
    }

    pub fn opacity(&self) -> f32 {
        self.mask_opacity
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.mask_opacity = opacity.clamp(0.05, 1.0);
        self.prefs_dirty = true;
    }

    pub fn color_prefs(&self) -> MaskColorPrefs {
        MaskColorPrefs {
            recent_colors: self.recent_colors.clone(),
            mask_opacity: self.mask_opacity,
            brush_opacity: self.brush_opacity,
            mask_color: self.mask_color,
            brush_color: self.brush_color,
        }
        .clamp()
    }

    pub fn apply_color_prefs(&mut self, prefs: MaskColorPrefs) {
        let prefs = prefs.clamp();
        self.recent_colors = prefs.recent_colors;
        self.mask_opacity = prefs.mask_opacity;
        self.brush_opacity = prefs.brush_opacity;
        self.mask_color = prefs.mask_color;
        self.brush_color = prefs.brush_color;
        self.sync_picker_hsv_from_target();
        self.rebuild_sb_image();
        self.prefs_dirty = false;
    }

    pub fn take_prefs_dirty(&mut self) -> bool {
        let d = self.prefs_dirty;
        self.prefs_dirty = false;
        d
    }

    fn mark_prefs_dirty(&mut self) {
        self.prefs_dirty = true;
    }

    fn current_target_color(&self) -> [u8; 3] {
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_color,
            ColorPickerTarget::Brush => self.brush_color,
        }
    }

    fn current_target_opacity(&self) -> f32 {
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_opacity,
            ColorPickerTarget::Brush => self.brush_opacity,
        }
    }

    fn sync_picker_hsv_from_target(&mut self) {
        let (h, s, v) = rgb_to_hsv(self.current_target_color());
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
    }

    fn point_in_bounds(x: f32, y: f32, b: Bounds<Pixels>) -> bool {
        let bx = f32::from(b.origin.x);
        let by = f32::from(b.origin.y);
        let bw = f32::from(b.size.width);
        let bh = f32::from(b.size.height);
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    }

    fn open_color_picker(&mut self, target: ColorPickerTarget, cx: &mut Context<Self>) {
        if self.color_picker_open && self.color_picker_target == target {
            self.close_color_picker(cx);
            return;
        }
        self.color_picker_target = target;
        self.color_picker_open = true;
        self.sync_picker_hsv_from_target();
        self.rebuild_sb_image();
        if self.hue_image.is_none() {
            self.rebuild_hue_image();
        }
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    fn close_color_picker(&mut self, cx: &mut Context<Self>) {
        if !self.color_picker_open {
            return;
        }
        let c = self.current_target_color();
        let mut prefs = self.color_prefs();
        prefs.push_recent(c);
        self.recent_colors = prefs.recent_colors;
        self.mark_prefs_dirty();
        self.eyedropper_armed = false;
        self.eyedropper_backup = None;
        self.color_picker_open = false;
        self.opacity_undid = false;
        if matches!(
            self.drag,
            Some(DragKind::PaletteOpacity | DragKind::PaletteSb | DragKind::PaletteHue)
        ) {
            self.drag = None;
        }
        cx.notify();
    }

    /// 选色悬浮窗相对悬浮层的布局: (left, top, place_below, caret_x_in_popover).
    /// `top` 为含箭头在内的外框顶部. 锚点换算一律相对 `picker_layer_bounds`
    /// (与绝对定位 `left`/`top` 同一坐标系), 避免侧栏 padding 造成的水平偏移.
    fn color_picker_placement(&self) -> Option<(f32, f32, bool, f32)> {
        let layer = if f32::from(self.picker_layer_bounds.size.width) >= 8.0 {
            self.picker_layer_bounds
        } else {
            self.side_bounds
        };
        let pw = f32::from(layer.size.width);
        let ph = f32::from(layer.size.height);
        if pw < 8.0 || ph < 8.0 {
            return None;
        }
        let anchor = match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_swatch_bounds,
            ColorPickerTarget::Brush => self.brush_swatch_bounds,
        };
        let aw = f32::from(anchor.size.width);
        let ah = f32::from(anchor.size.height);
        if aw < 1.0 || ah < 1.0 {
            return None;
        }
        // 最近色 8 格单行: 8×22 + 7×gap4 = 204; 再加 padding/border
        let pop_w = Self::picker_pop_w();
        let pop_h = 14.0 + 16.0 + 22.0 + 8.0 + SB_SIZE + 36.0 + 44.0;
        let caret = 8.0;
        let gap = 6.0;
        let layer_left = f32::from(layer.origin.x);
        let layer_top = f32::from(layer.origin.y);
        let ax = f32::from(anchor.origin.x);
        let ay = f32::from(anchor.origin.y);
        let anchor_cx = ax + aw * 0.5 - layer_left;
        let anchor_top = ay - layer_top;
        let anchor_bot = ay + ah - layer_top;
        let space_below = ph - anchor_bot;
        let space_above = anchor_top;
        let place_below = space_below >= pop_h + caret + gap || space_below >= space_above;
        let left = (anchor_cx - pop_w * 0.5).clamp(4.0, (pw - pop_w - 4.0).max(4.0));
        let stack_h = pop_h + caret;
        let top = if place_below {
            anchor_bot + gap
        } else {
            (anchor_top - gap - stack_h).max(4.0)
        };
        let caret_x = (anchor_cx - left).clamp(10.0, pop_w - 10.0);
        Some((left, top, place_below, caret_x))
    }

    fn picker_pop_w() -> f32 {
        // SB+色相列约 214; 最近色单行需 ≥ 8×22 + 7×4 + 内边距16 + 边框 ≈ 222
        (SB_SIZE + HUE_BAR_W + 8.0 + 16.0 + 4.0).max(232.0)
    }

    fn picker_caret(place_below: bool, caret_x: f32) -> impl IntoElement {
        let h = 8.0_f32;
        let half = 8.0_f32;
        div()
            .w_full()
            .h(px(h))
            .relative()
            .child(
                canvas(|_, _, _| {}, {
                    move |bounds, _, window, _| {
                        let ox = f32::from(bounds.origin.x);
                        let oy = f32::from(bounds.origin.y);
                        let cx = ox + caret_x;
                        let mut builder = PathBuilder::fill();
                        if place_below {
                            builder.move_to(point(px(cx), px(oy)));
                            builder.line_to(point(px(cx - half), px(oy + h)));
                            builder.line_to(point(px(cx + half), px(oy + h)));
                        } else {
                            builder.move_to(point(px(cx), px(oy + h)));
                            builder.line_to(point(px(cx - half), px(oy)));
                            builder.line_to(point(px(cx + half), px(oy)));
                        }
                        builder.close();
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, rgb(0x1e293b));
                        }
                    }
                })
                .absolute()
                .size_full(),
            )
    }

    fn color_picker_floating(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (left, top, place_below, caret_x) = self
            .color_picker_placement()
            .unwrap_or((8.0, 120.0, true, 40.0));
        let pop_w = Self::picker_pop_w();
        div()
            .id("color_picker_layer")
            .absolute()
            .inset_0()
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, cx| {
                                let prev = this.picker_layer_bounds;
                                this.picker_layer_bounds = bounds;
                                // 首帧写入真实图层 bounds 后重算锚点, 纠正 padding 偏移
                                let changed = f32::from(prev.size.width) < 1.0
                                    || (f32::from(prev.origin.x) - f32::from(bounds.origin.x))
                                        .abs()
                                        > 0.5
                                    || (f32::from(prev.origin.y) - f32::from(bounds.origin.y))
                                        .abs()
                                        > 0.5;
                                if changed && this.color_picker_open {
                                    cx.notify();
                                }
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    // 再点同一色块 = 提交并收起; 点另一色块 = 切换目标.
                    // 必须在遮罩层处理, 否则会先 close 再被下层按钮 open.
                    if Self::point_in_bounds(x, y, this.brush_swatch_bounds) {
                        this.open_color_picker(ColorPickerTarget::Brush, cx);
                        return;
                    }
                    if Self::point_in_bounds(x, y, this.mask_swatch_bounds) {
                        this.open_color_picker(ColorPickerTarget::Mask, cx);
                        return;
                    }
                    this.close_color_picker(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_color_picker(cx);
                }),
            )
            .child(
                div()
                    .id("color_picker_float")
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(pop_w))
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .when(place_below, |d| d.child(Self::picker_caret(true, caret_x)))
                    .child(self.color_picker_popover(cx))
                    .when(!place_below, |d| d.child(Self::picker_caret(false, caret_x))),
            )
    }

    fn rebuild_hue_image(&mut self) {
        let w = 4u32;
        let h = HUE_TEX_H;
        let mut rgba: RgbaImage = ImageBuffer::new(w, h);
        for y in 0..h {
            let hue = 360.0 * y as f32 / (h - 1).max(1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, 1.0, 1.0);
            for x in 0..w {
                // GPUI RenderImage 用 BGRA
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        self.hue_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
    }

    fn rebuild_sb_image(&mut self) {
        let size = SB_TEX_SIZE;
        let mut rgba: RgbaImage = ImageBuffer::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let s = x as f32 / (size - 1).max(1) as f32;
                let v = 1.0 - y as f32 / (size - 1).max(1) as f32;
                let [r, g, b] = hsv_to_rgb(self.picker_h, s, v);
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        self.sb_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
    }

    fn picker_rgb(&self) -> [u8; 3] {
        hsv_to_rgb(self.picker_h, self.picker_s, self.picker_v)
    }

    fn sync_rgb_inputs_from_picker(&mut self, cx: &mut Context<Self>) {
        let [r, g, b] = self.picker_rgb();
        self.rgb_syncing = true;
        self.rgb_r_input
            .update(cx, |t, cx| t.set_text(r.to_string(), cx));
        self.rgb_g_input
            .update(cx, |t, cx| t.set_text(g.to_string(), cx));
        self.rgb_b_input
            .update(cx, |t, cx| t.set_text(b.to_string(), cx));
        self.rgb_syncing = false;
    }

    fn apply_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        if self.rgb_syncing || !self.color_picker_open {
            return;
        }
        let blur = self.rgb_r_input.update(cx, |t, _| t.take_blur_commit())
            | self.rgb_g_input.update(cx, |t, _| t.take_blur_commit())
            | self.rgb_b_input.update(cx, |t, _| t.take_blur_commit());
        let parse = |s: String| -> Option<u8> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            t.parse::<u8>().ok()
        };
        let r = parse(self.rgb_r_input.read(cx).text());
        let g = parse(self.rgb_g_input.read(cx).text());
        let b = parse(self.rgb_b_input.read(cx).text());
        let (Some(r), Some(g), Some(b)) = (r, g, b) else {
            if blur {
                // 失焦时输入不完整: 回写当前合法色并视为已提交
                self.sync_rgb_inputs_from_picker(cx);
            }
            return;
        };
        if [r, g, b] == self.picker_rgb() {
            return;
        }
        self.set_picker_from_rgb([r, g, b], cx);
    }

    fn set_picker_from_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.commit_picker_color(false);
        // 不回写文本框: 用户正在输入时回写会打断编辑
        cx.notify();
    }

    fn sample_image_rgb(&self, ix: f32, iy: f32) -> Option<[u8; 3]> {
        let img = self.rgb_image.as_ref()?;
        if self.img_w == 0 || self.img_h == 0 {
            return None;
        }
        let x = ix.round().clamp(0.0, (self.img_w - 1) as f32) as u32;
        let y = iy.round().clamp(0.0, (self.img_h - 1) as f32) as u32;
        let p = img.get_pixel(x, y);
        Some([p[0], p[1], p[2]])
    }

    /// 取色预览: 只改色盘/HSV/目标色与 RGB 文本, 不改已选蒙版项、不入最近色.
    fn preview_eyedropper_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_color = rgb,
            ColorPickerTarget::Brush => self.brush_color = rgb,
        }
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    fn arm_eyedropper(&mut self, cx: &mut Context<Self>) {
        if !self.color_picker_open {
            return;
        }
        if self.eyedropper_armed {
            self.cancel_eyedropper(cx);
            return;
        }
        let c = self.picker_rgb();
        self.eyedropper_backup = Some((self.picker_h, self.picker_s, self.picker_v, c));
        self.eyedropper_armed = true;
        self.status = "取色: 在左侧图上移动预览, 单击确认, Esc/右键取消".into();
        cx.notify();
    }

    fn cancel_eyedropper(&mut self, cx: &mut Context<Self>) {
        if !self.eyedropper_armed {
            return;
        }
        if let Some((h, s, v, c)) = self.eyedropper_backup.take() {
            self.picker_h = h;
            self.picker_s = s;
            self.picker_v = v;
            self.rebuild_sb_image();
            match self.color_picker_target {
                ColorPickerTarget::Mask => self.mask_color = c,
                ColorPickerTarget::Brush => self.brush_color = c,
            }
            self.sync_rgb_inputs_from_picker(cx);
        }
        self.eyedropper_armed = false;
        self.status = "已取消取色".into();
        cx.notify();
    }

    fn confirm_eyedropper_at(&mut self, ix: f32, iy: f32, cx: &mut Context<Self>) {
        if let Some(rgb) = self.sample_image_rgb(ix, iy) {
            self.preview_eyedropper_rgb(rgb, cx);
            self.commit_picker_color(true);
        }
        self.eyedropper_armed = false;
        self.eyedropper_backup = None;
        self.status = "已取色".into();
        cx.notify();
    }

    fn commit_picker_color(&mut self, push_recent: bool) {
        let c = self.picker_rgb();
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_color = c,
            ColorPickerTarget::Brush => self.brush_color = c,
        }
        if !self.selected.is_empty() {
            if !self.opacity_undid {
                self.push_undo();
                self.opacity_undid = true;
            }
            for m in &mut self.masks {
                if self.selected.contains(&m.id) {
                    m.color = c;
                }
            }
        }
        if push_recent {
            let mut prefs = self.color_prefs();
            prefs.push_recent(c);
            self.recent_colors = prefs.recent_colors;
        }
        self.mark_prefs_dirty();
    }

    fn apply_target_opacity(&mut self, v: f32) {
        let v = v.clamp(0.05, 1.0);
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_opacity = v,
            ColorPickerTarget::Brush => self.brush_opacity = v,
        }
        if !self.selected.is_empty() {
            if !self.opacity_undid {
                self.push_undo();
                self.opacity_undid = true;
            }
            for m in &mut self.masks {
                if self.selected.contains(&m.id) {
                    m.opacity = v;
                }
            }
        }
        self.mark_prefs_dirty();
    }

    fn set_palette_opacity_from_x(&mut self, x: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.opacity_track.origin.x);
        let width = f32::from(self.opacity_track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        self.apply_target_opacity(0.05 + t * 0.95);
        cx.notify();
    }

    fn set_palette_sb_from_pos(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.sb_bounds.origin.x);
        let top = f32::from(self.sb_bounds.origin.y);
        let w = f32::from(self.sb_bounds.size.width).max(1.0);
        let h = f32::from(self.sb_bounds.size.height).max(1.0);
        self.picker_s = ((x - left) / w).clamp(0.0, 1.0);
        self.picker_v = (1.0 - (y - top) / h).clamp(0.0, 1.0);
        self.commit_picker_color(false);
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    fn set_palette_hue_from_y(&mut self, y: f32, cx: &mut Context<Self>) {
        let top = f32::from(self.hue_bounds.origin.y);
        let h = f32::from(self.hue_bounds.size.height).max(1.0);
        self.picker_h = ((y - top) / h).clamp(0.0, 1.0) * 360.0;
        self.rebuild_sb_image();
        self.commit_picker_color(false);
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    fn pick_recent_color(&mut self, color: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(color);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.opacity_undid = false;
        self.commit_picker_color(true);
        self.opacity_undid = false;
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    /// 当前滑条应对应的显示值: 有选中则取选中项平均透明度, 否则当前目标默认透明度.
    fn slider_opacity_value(&self) -> f32 {
        if self.selected.is_empty() {
            return self.current_target_opacity();
        }
        let mut sum = 0.0f32;
        let mut n = 0usize;
        for m in &self.masks {
            if self.selected.contains(&m.id) {
                sum += m.effective_opacity();
                n += 1;
            }
        }
        if n == 0 {
            self.current_target_opacity()
        } else {
            sum / n as f32
        }
    }

    pub fn cancel_poly_draft(&mut self, cx: &mut Context<Self>) {
        if self.eyedropper_armed {
            self.cancel_eyedropper(cx);
            return;
        }
        if self.poly_draft.is_none() {
            return;
        }
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = "已取消折线.".into();
        cx.notify();
    }

    /// 折线光标: 点数≥3 且靠近首点时吸附到首点.
    fn poly_maybe_snap(&self, ix: f32, iy: f32) -> (f32, f32, bool) {
        let Some(draft) = &self.poly_draft else {
            return (ix, iy, false);
        };
        if draft.len() < 3 {
            return (ix, iy, false);
        }
        let (fx, fy) = draft[0];
        let xform = self.xform();
        let (sx, sy) = xform.image_to_screen(ix, iy);
        let (fsx, fsy) = xform.image_to_screen(fx, fy);
        let dist = (sx - fsx).hypot(sy - fsy);
        if dist <= POLY_SNAP_SCREEN_PX {
            (fx, fy, true)
        } else {
            (ix, iy, false)
        }
    }

    fn finalize_poly(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.poly_draft.take() else {
            return;
        };
        self.poly_cursor = None;
        if draft.len() < 3 || self.img_w < 2 || self.img_h < 2 {
            self.status = "折线至少需要 3 个点.".into();
            cx.notify();
            return;
        }
        let iw = self.img_w as i32;
        let ih = self.img_h as i32;
        let clamp = |v: f32, hi: i32| -> i32 {
            v.round().clamp(0.0, (hi - 1).max(0) as f32) as i32
        };
        let pts: Vec<(i32, i32)> = draft
            .iter()
            .map(|(x, y)| (clamp(*x, iw), clamp(*y, ih)))
            .collect();
        // 去掉连续重复点
        let mut dedup: Vec<(i32, i32)> = Vec::new();
        for p in pts {
            if dedup.last().copied() != Some(p) {
                dedup.push(p);
            }
        }
        if dedup.len() < 3 {
            self.status = "折线顶点过少, 已取消.".into();
            cx.notify();
            return;
        }
        let mid = new_id();
        let mut m = MaskRect {
            id: mid.clone(),
            x0: 0,
            y0: 0,
            x1: 0,
            y1: 0,
            brush_points: Vec::new(),
            brush_radius: 0,
            color: self.mask_color,
            poly_points: dedup,
            opacity: self.mask_opacity,
        };
        m.refresh_poly_bounds();
        self.push_undo();
        self.masks.push(m);
        self.selected.clear();
        self.selected.insert(mid);
        self.status = format!("蒙版 {} 个 (折线闭环)", self.masks.len()).into();
        cx.notify();
    }

    fn history_key(&self) -> Option<String> {
        self.session_key
            .clone()
            .or_else(|| self.image_path.as_ref().map(|p| p.display().to_string()))
    }

    fn stash_history(&mut self) {
        let Some(key) = self.history_key() else {
            return;
        };
        self.histories.insert(
            key,
            MaskHistory {
                undo: self.undo_stack.clone(),
                redo: self.redo_stack.clone(),
            },
        );
    }

    fn restore_history_for(&mut self, key: &str) {
        if let Some(h) = self.histories.get(key) {
            self.undo_stack = h.undo.clone();
            self.redo_stack = h.redo.clone();
        } else {
            self.undo_stack.clear();
            self.redo_stack.clear();
        }
    }

    pub fn masks_clone(&self) -> Vec<MaskRect> {
        self.masks.clone()
    }

    pub fn session_key(&self) -> Option<&str> {
        self.session_key.as_deref()
    }

    /// 从内存 RGB 载入 (组内成员裁切图); 坐标相对该裁切图.
    pub fn load_rgb(
        &mut self,
        rgb: image::RgbImage,
        session_key: String,
        masks: Vec<MaskRect>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        if self.session_key.as_ref() == Some(&session_key) && self.rgb_image.is_some() {
            return;
        }
        self.stash_history();
        let (w, h) = rgb.dimensions();
        let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
            let p = rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let render = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
        self.image_path = None;
        self.session_key = Some(session_key.clone());
        self.rgb_image = Some(rgb);
        self.render_image = Some(render);
        self.img_w = w;
        self.img_h = h;
        self.masks = masks;
        self.selected.clear();
        self.restore_history_for(&session_key);
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.user_zoomed = false;
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = format!("{label} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
        self.hint = format!(
            "编辑: {label}\n蒙版坐标相对本组合拼合图; 各组合独立 (共享脚注可在不同组画不同遮盖)."
        )
        .into();
        cx.notify();
    }

    pub fn clear_view(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.stash_history();
        self.image_path = None;
        self.session_key = None;
        self.rgb_image = None;
        self.render_image = None;
        self.img_w = 0;
        self.img_h = 0;
        self.masks.clear();
        self.selected.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = message.into();
        cx.notify();
    }

    /// 嵌入用工具条 (无「打开」, 图由宿主同步组内块).
    pub fn toolbar_embedded(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.btn("export", "导出本块", false, false, Self::export_image, cx))
            .child(self.btn(
                "fit",
                "适应",
                false,
                false,
                |this, _, cx| this.fit_to_view(cx),
                cx,
            ))
            .child(self.btn(
                "del",
                "删除",
                false,
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x64748b))
                    .child(self.status.clone()),
            )
    }

    fn xform(&self) -> ViewXform {
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        ViewXform::compute(
            self.img_w as f32,
            self.img_h as f32,
            vw,
            vh,
            self.zoom,
            self.pan,
            self.user_zoomed,
        )
    }

    fn screen_in_view(&self, pos: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(pos.x) - f32::from(self.view_bounds.origin.x),
            f32::from(pos.y) - f32::from(self.view_bounds.origin.y),
        )
    }

    pub fn load_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !is_image_path(&path) {
            self.status = format!("不是支持的图片: {}", path.display()).into();
            cx.notify();
            return;
        }
        // 切页前缓存当前蒙版
        if let Some(old) = self.image_path.clone() {
            self.page_masks.insert(old, self.masks.clone());
        }
        match image::open(&path) {
            Ok(img) => {
                let rgb = img.to_rgb8();
                let (w, h) = rgb.dimensions();
                let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
                    let p = rgb.get_pixel(x, y);
                    image::Rgba([p[0], p[1], p[2], 255])
                });
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                let render = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
                let restored = self
                    .page_masks
                    .get(&path)
                    .cloned()
                    .unwrap_or_default();
                // 先把旧页的撤重栈存好, 再切路径.
                self.stash_history();
                self.image_path = Some(path.clone());
                self.session_key = None;
                self.rgb_image = Some(rgb);
                self.render_image = Some(render);
                self.img_w = w;
                self.img_h = h;
                self.masks = restored;
                self.selected.clear();
                self.restore_history_for(&path.display().to_string());
                self.zoom = 1.0;
                self.pan = point(0.0, 0.0);
                self.user_zoomed = false;
                self.drag = None;
                self.poly_draft = None;
                self.poly_cursor = None;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image");
                self.status = format!("已载入 {name} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
                self.hint = format!(
                    "已载入 {name}. 框选/折线/画笔画蒙版; 平移拖动画布或已选框."
                )
                .into();
                cx.notify();
            }
            Err(e) => {
                self.status = format!("打开失败: {e}").into();
                cx.notify();
            }
        }
    }

    pub fn fit_to_view(&mut self, cx: &mut Context<Self>) {
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        cx.notify();
    }

    pub fn toggle_draw_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Draw {
            ToolMode::Select
        } else {
            ToolMode::Draw
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_pan_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Pan {
            ToolMode::Select
        } else {
            ToolMode::Pan
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_brush_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Brush {
            ToolMode::Select
        } else {
            ToolMode::Brush
        };
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.brush_cursor = None;
        if self.mode != ToolMode::Brush {
            self.color_picker_open = false;
        }
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_eraser_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Eraser {
            ToolMode::Select
        } else {
            ToolMode::Eraser
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_poly_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Poly {
            ToolMode::Select
        } else {
            ToolMode::Poly
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    fn mode_status(mode: ToolMode) -> SharedString {
        match mode {
            ToolMode::Draw => "框选".into(),
            ToolMode::Poly => "折线: 逐点连线, 靠近首点吸附闭环 (右键取消)".into(),
            ToolMode::Brush => "画笔 (拖动画布涂抹; 可改颜色/粗细)".into(),
            ToolMode::Eraser => "橡皮: 单击擦最上层 · 拖动擦光".into(),
            ToolMode::Select => "选择 (可 Ctrl 多选 / Shift 拖选)".into(),
            ToolMode::Pan => "平移".into(),
        }
    }

    fn brush_radius_px(&self) -> i32 {
        ((self.brush_size * 0.5).round() as i32).max(1)
    }

    fn set_brush_size_from_x(&mut self, x: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.brush_size_track.origin.x);
        let width = f32::from(self.brush_size_track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        self.brush_size = BRUSH_SIZE_MIN + t * (BRUSH_SIZE_MAX - BRUSH_SIZE_MIN);
        cx.notify();
    }

    fn append_brush_point(&mut self, id: &str, ix: f32, iy: f32) -> bool {
        let Some(m) = self.masks.iter_mut().find(|m| m.id == id) else {
            return false;
        };
        let px = ix.round() as i32;
        let py = iy.round() as i32;
        if let Some(&(lx, ly)) = m.brush_points.last() {
            let dx = (px - lx) as f32;
            let dy = (py - ly) as f32;
            // 过密采样没必要, 也减轻撤销快照体积.
            if dx * dx + dy * dy < 1.5 {
                return false;
            }
        }
        m.brush_points.push((px, py));
        m.refresh_brush_bounds();
        true
    }

    fn hit_mask(&self, ix: f32, iy: f32) -> Option<String> {
        self.masks
            .iter()
            .rev()
            .find(|m| m.contains(ix, iy))
            .map(|m| m.id.clone())
    }

    /// 点擦: 只删最上层 (列表末尾优先).
    fn erase_topmost_at(&mut self, ix: f32, iy: f32) -> bool {
        let Some(id) = self.hit_mask(ix, iy) else {
            return false;
        };
        self.masks.retain(|m| m.id != id);
        self.selected.remove(&id);
        true
    }

    /// 拖擦: 删掉该点碰到的全部蒙版.
    fn erase_all_at(&mut self, ix: f32, iy: f32) -> bool {
        let ids: Vec<String> = self
            .masks
            .iter()
            .filter(|m| m.contains(ix, iy))
            .map(|m| m.id.clone())
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.masks.retain(|m| !ids.iter().any(|id| id == &m.id));
        for id in &ids {
            self.selected.remove(id);
        }
        true
    }

    fn apply_selection_click(&mut self, id: Option<String>, control: bool) {
        match id {
            Some(id) if control => {
                if self.selected.contains(&id) {
                    self.selected.remove(&id);
                } else {
                    self.selected.insert(id);
                }
            }
            Some(id) => {
                self.selected.clear();
                self.selected.insert(id);
            }
            None if !control => {
                self.selected.clear();
            }
            None => {}
        }
    }

    /// 在图像边界内整体平移选中蒙版; 返回实际位移.
    fn translate_selected(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        if dx == 0 && dy == 0 {
            return (0, 0);
        }
        let iw = self.img_w as i32;
        let ih = self.img_h as i32;
        if iw < 1 || ih < 1 {
            return (0, 0);
        }
        let mut min_dx = i32::MIN / 4;
        let mut max_dx = i32::MAX / 4;
        let mut min_dy = i32::MIN / 4;
        let mut max_dy = i32::MAX / 4;
        let mut any = false;
        for m in &self.masks {
            if !self.selected.contains(&m.id) {
                continue;
            }
            any = true;
            let r = m.normalized();
            min_dx = min_dx.max(-r.x0);
            max_dx = max_dx.min((iw - 1) - r.x1);
            min_dy = min_dy.max(-r.y0);
            max_dy = max_dy.min((ih - 1) - r.y1);
        }
        if !any {
            return (0, 0);
        }
        let dx = dx.clamp(min_dx, max_dx);
        let dy = dy.clamp(min_dy, max_dy);
        if dx == 0 && dy == 0 {
            return (0, 0);
        }
        for m in &mut self.masks {
            if self.selected.contains(&m.id) {
                m.translate(dx, dy);
            }
        }
        (dx, dy)
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.masks.clone());
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            self.status = "没有可撤回的操作.".into();
            cx.notify();
            return;
        };
        self.redo_stack.push(self.masks.clone());
        self.masks = prev;
        self.selected.clear();
        self.status = format!("已撤回. 蒙版 {} 个", self.masks.len()).into();
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            self.status = "没有可重做的操作.".into();
            cx.notify();
            return;
        };
        self.undo_stack.push(self.masks.clone());
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.masks = next;
        self.selected.clear();
        self.status = format!("已重做. 蒙版 {} 个", self.masks.len()).into();
        cx.notify();
    }

    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        if !self.masks.iter().any(|m| self.selected.contains(&m.id)) {
            return;
        }
        self.push_undo();
        self.masks.retain(|m| !self.selected.contains(&m.id));
        self.selected.clear();
        self.status = "已删除选中蒙版.".into();
        cx.notify();
    }

    pub fn select_all_masks(&mut self, cx: &mut Context<Self>) {
        if self.masks.is_empty() {
            self.status = "没有蒙版可全选.".into();
            cx.notify();
            return;
        }
        self.selected = self.masks.iter().map(|m| m.id.clone()).collect();
        self.status = format!("已全选 {} 个蒙版 (Delete 删除)", self.selected.len()).into();
        cx.notify();
    }

    pub fn clear_masks(&mut self, cx: &mut Context<Self>) {
        if self.masks.is_empty() {
            return;
        }
        self.push_undo();
        self.masks.clear();
        self.selected.clear();
        self.status = "已清空蒙版.".into();
        cx.notify();
    }

    pub fn open_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("打开图片")
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"],
            );
        if let Some(ref p) = self.image_path {
            if let Some(parent) = p.parent() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(path) = dialog.pick_file() {
            self.load_image(path, cx);
        }
    }

    pub fn export_image(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(ref base) = self.rgb_image else {
            self.status = "请先打开图片.".into();
            cx.notify();
            return;
        };
        let suggested = default_export_path(self.image_path.as_deref());
        let mut dialog = rfd::FileDialog::new()
            .set_title("导出已遮盖图片")
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .set_file_name(
                suggested
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("masked.png"),
            );
        if let Some(parent) = suggested.parent().filter(|p| p.is_dir()) {
            dialog = dialog.set_directory(parent);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        match export_masked(base, &self.masks, self.mask_opacity, &path) {
            Ok(()) => {
                self.status = format!("已保存: {}", path.display()).into();
            }
            Err(e) => {
                self.status = e.into();
            }
        }
        cx.notify();
    }

    fn on_view_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.render_image.is_none() {
            return;
        }
        if ev.click_count >= 2 && ev.button == MouseButton::Left {
            self.fit_to_view(cx);
            return;
        }
        if ev.button != MouseButton::Left {
            if ev.button == MouseButton::Right && self.eyedropper_armed {
                self.cancel_eyedropper(cx);
            }
            return;
        }
        let (sx, sy) = self.screen_in_view(ev.position);
        let xform = self.xform();
        let (ix, iy) = xform.screen_to_image(sx, sy);
        if self.eyedropper_armed {
            self.confirm_eyedropper_at(ix, iy, cx);
            return;
        }
        let control = ev.modifiers.control;
        let shift = ev.modifiers.shift;

        match self.mode {
            ToolMode::Draw => {
                self.drag = Some(DragKind::Draw {
                    x0: ix,
                    y0: iy,
                    x1: ix,
                    y1: iy,
                });
            }
            ToolMode::Poly => {
                let (ix, iy, snap) = self.poly_maybe_snap(ix, iy);
                if snap {
                    self.finalize_poly(cx);
                    return;
                }
                match &mut self.poly_draft {
                    Some(draft) => {
                        if let Some(&(lx, ly)) = draft.last() {
                            if (lx - ix).abs() < 0.5 && (ly - iy).abs() < 0.5 {
                                // 忽略重复点
                            } else {
                                draft.push((ix, iy));
                            }
                        } else {
                            draft.push((ix, iy));
                        }
                        self.poly_cursor = Some((ix, iy));
                        self.status = format!(
                            "折线 {} 点 · 靠近首点吸附闭环 (右键/Esc 取消)",
                            draft.len()
                        )
                        .into();
                    }
                    None => {
                        self.poly_draft = Some(vec![(ix, iy)]);
                        self.poly_cursor = Some((ix, iy));
                        self.status = "折线起点已定 · 继续点击加点".into();
                    }
                }
            }
            ToolMode::Brush => {
                if self.img_w < 2 || self.img_h < 2 {
                    return;
                }
                let id = new_id();
                let r = self.brush_radius_px();
                let px = ix.round() as i32;
                let py = iy.round() as i32;
                self.push_undo();
                self.masks.push(MaskRect {
                    id: id.clone(),
                    x0: px - r,
                    y0: py - r,
                    x1: px + r,
                    y1: py + r,
                    brush_points: vec![(px, py)],
                    brush_radius: r,
                    color: self.brush_color,
                    poly_points: Vec::new(),
                    opacity: self.brush_opacity,
                });
                self.selected.clear();
                self.selected.insert(id.clone());
                self.drag = Some(DragKind::Brush { id, undid: true });
                self.status = format!("蒙版 {} 个", self.masks.len()).into();
            }
            ToolMode::Eraser => {
                self.drag = Some(DragKind::Erase {
                    start_ix: ix,
                    start_iy: iy,
                    undid: false,
                    wiping: false,
                });
            }
            ToolMode::Pan => {
                let hit = self.hit_mask(ix, iy);
                if let Some(ref id) = hit {
                    if self.selected.contains(id) && !control {
                        self.drag = Some(DragKind::MoveMasks {
                            last_ix: ix,
                            last_iy: iy,
                            undid: false,
                        });
                    } else {
                        self.apply_selection_click(hit, control);
                    }
                } else {
                    if !control {
                        self.selected.clear();
                    }
                    self.drag = Some(DragKind::PagePan {
                        last: ev.position,
                    });
                }
            }
            ToolMode::Select => {
                if shift {
                    self.drag = Some(DragKind::Marquee {
                        x0: ix,
                        y0: iy,
                        x1: ix,
                        y1: iy,
                        additive: control,
                    });
                } else {
                    let hit = self.hit_mask(ix, iy);
                    self.apply_selection_click(hit, control);
                }
            }
        }
        cx.notify();
    }

    pub fn has_active_drag(&self) -> bool {
        self.drag.is_some()
    }

    /// 滑条/色盘拖拽: 鼠标可离开控件但仍在窗口内, 需宿主根节点继续转发.
    pub fn needs_root_move_forward(&self) -> bool {
        matches!(
            self.drag,
            Some(
                DragKind::BrushSize
                    | DragKind::PaletteOpacity
                    | DragKind::PaletteSb
                    | DragKind::PaletteHue
            )
        )
    }

    /// 宿主在窗口外转发鼠标移动 (元素级 on_mouse_move 在窗口外不触发).
    pub fn root_mouse_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.apply_mouse_move_at(point(px(x), px(y)), cx);
    }

    /// 宿主在窗口外转发鼠标松开.
    pub fn root_mouse_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.apply_mouse_up_at(point(px(x), px(y)), cx);
    }

    fn on_view_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_mouse_move_at(ev.position, cx);
    }

    fn apply_mouse_move_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (sx, sy) = self.screen_in_view(position);
        if self.eyedropper_armed {
            let xform = self.xform();
            let (ix, iy) = xform.screen_to_image(sx, sy);
            if let Some(rgb) = self.sample_image_rgb(ix, iy) {
                self.preview_eyedropper_rgb(rgb, cx);
            }
            return;
        }
        match self.drag.take() {
            Some(DragKind::Draw { x0, y0, .. }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                self.drag = Some(DragKind::Draw {
                    x0,
                    y0,
                    x1: ix,
                    y1: iy,
                });
                cx.notify();
            }
            Some(DragKind::Brush { id, undid }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                self.brush_cursor = Some((ix, iy));
                if self.append_brush_point(&id, ix, iy) {
                    cx.notify();
                } else {
                    cx.notify();
                }
                self.drag = Some(DragKind::Brush { id, undid });
            }
            Some(DragKind::PagePan { last }) => {
                let dx = f32::from(position.x) - f32::from(last.x);
                let dy = f32::from(position.y) - f32::from(last.y);
                self.pan.x += dx;
                self.pan.y += dy;
                self.user_zoomed = true;
                self.drag = Some(DragKind::PagePan { last: position });
                cx.notify();
            }
            Some(DragKind::MoveMasks {
                last_ix,
                last_iy,
                undid,
            }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                let raw_dx = ix - last_ix;
                let raw_dy = iy - last_iy;
                let step_x = raw_dx.round() as i32;
                let step_y = raw_dy.round() as i32;
                let mut undid = undid;
                if (step_x != 0 || step_y != 0) && !undid {
                    self.push_undo();
                    undid = true;
                }
                let (applied_x, applied_y) = self.translate_selected(step_x, step_y);
                self.drag = Some(DragKind::MoveMasks {
                    last_ix: last_ix + applied_x as f32,
                    last_iy: last_iy + applied_y as f32,
                    undid,
                });
                cx.notify();
            }
            Some(DragKind::Marquee {
                x0,
                y0,
                additive,
                ..
            }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                self.drag = Some(DragKind::Marquee {
                    x0,
                    y0,
                    x1: ix,
                    y1: iy,
                    additive,
                });
                cx.notify();
            }
            Some(DragKind::PaletteOpacity) => {
                self.drag = Some(DragKind::PaletteOpacity);
                self.set_palette_opacity_from_x(f32::from(position.x), cx);
            }
            Some(DragKind::PaletteSb) => {
                self.drag = Some(DragKind::PaletteSb);
                self.set_palette_sb_from_pos(
                    f32::from(position.x),
                    f32::from(position.y),
                    cx,
                );
            }
            Some(DragKind::PaletteHue) => {
                self.drag = Some(DragKind::PaletteHue);
                self.set_palette_hue_from_y(f32::from(position.y), cx);
            }
            Some(DragKind::BrushSize) => {
                self.drag = Some(DragKind::BrushSize);
                self.set_brush_size_from_x(f32::from(position.x), cx);
            }
            Some(DragKind::Erase {
                start_ix,
                start_iy,
                undid,
                wiping,
            }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                let mut undid = undid;
                let mut wiping = wiping;
                if !wiping {
                    let dx = ix - start_ix;
                    let dy = iy - start_iy;
                    if dx * dx + dy * dy >= ERASE_DRAG_SLOP_IMG * ERASE_DRAG_SLOP_IMG {
                        wiping = true;
                        if !undid {
                            self.push_undo();
                            undid = true;
                        }
                        self.erase_all_at(start_ix, start_iy);
                        self.erase_all_at(ix, iy);
                        self.status = format!("拖擦中 · 蒙版 {} 个", self.masks.len()).into();
                        cx.notify();
                    }
                } else if self.erase_all_at(ix, iy) {
                    self.status = format!("拖擦中 · 蒙版 {} 个", self.masks.len()).into();
                    cx.notify();
                }
                self.drag = Some(DragKind::Erase {
                    start_ix,
                    start_iy,
                    undid,
                    wiping,
                });
            }
            None => {
                if self.mode == ToolMode::Poly && self.poly_draft.is_some() {
                    let xform = self.xform();
                    let (ix, iy) = xform.screen_to_image(sx, sy);
                    let (ix, iy, _) = self.poly_maybe_snap(ix, iy);
                    self.poly_cursor = Some((ix, iy));
                    cx.notify();
                } else if self.mode == ToolMode::Brush {
                    let xform = self.xform();
                    let (ix, iy) = xform.screen_to_image(sx, sy);
                    self.brush_cursor = Some((ix, iy));
                    cx.notify();
                }
            }
        }
    }

    fn on_view_mouse_up(
        &mut self,
        ev: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.button != MouseButton::Left {
            return;
        }
        self.apply_mouse_up_at(ev.position, cx);
    }

    fn apply_mouse_up_at(&mut self, _position: Point<Pixels>, cx: &mut Context<Self>) {
        let finished = self.drag.take();
        match finished {
            Some(DragKind::Draw { x0, y0, x1, y1 }) => {
                if self.img_w < 2 || self.img_h < 2 {
                    cx.notify();
                    return;
                }
                let min_x = x0.min(x1);
                let max_x = x0.max(x1);
                let min_y = y0.min(y1);
                let max_y = y0.max(y1);
                let w = max_x - min_x;
                let h = max_y - min_y;
                if w >= 3.0 && h >= 3.0 {
                    let iw = self.img_w as i32;
                    let ih = self.img_h as i32;
                    let clamp = |v: f32, hi: i32| -> i32 {
                        v.round().clamp(0.0, (hi - 1).max(0) as f32) as i32
                    };
                    let mid = new_id();
                    self.push_undo();
                    self.masks.push(MaskRect {
                        id: mid.clone(),
                        x0: clamp(min_x, iw),
                        y0: clamp(min_y, ih),
                        x1: clamp(max_x, iw),
                        y1: clamp(max_y, ih),
                        brush_points: Vec::new(),
                        brush_radius: 0,
                        color: self.mask_color,
                        poly_points: Vec::new(),
                        opacity: self.mask_opacity,
                    });
                    self.selected.clear();
                    self.selected.insert(mid);
                    self.status = format!("蒙版 {} 个", self.masks.len()).into();
                }
                cx.notify();
            }
            Some(DragKind::Brush { id, .. }) => {
                if let Some(m) = self.masks.iter_mut().find(|m| m.id == id) {
                    m.refresh_brush_bounds();
                }
                self.status = format!("蒙版 {} 个", self.masks.len()).into();
                cx.notify();
            }
            Some(DragKind::Marquee {
                x0,
                y0,
                x1,
                y1,
                additive,
            }) => {
                let min_x = x0.min(x1);
                let max_x = x0.max(x1);
                let min_y = y0.min(y1);
                let max_y = y0.max(y1);
                if (max_x - min_x) >= 2.0 && (max_y - min_y) >= 2.0 {
                    if !additive {
                        self.selected.clear();
                    }
                    for m in &self.masks {
                        if m.intersects_rect(min_x, min_y, max_x, max_y) {
                            self.selected.insert(m.id.clone());
                        }
                    }
                    self.status = format!("已选中 {} 个", self.selected.len()).into();
                }
                cx.notify();
            }
            Some(DragKind::MoveMasks { .. })
            | Some(DragKind::PagePan { .. })
            | Some(DragKind::BrushSize)
            | Some(DragKind::PaletteOpacity)
            | None => {
                self.opacity_undid = false;
                cx.notify();
            }
            Some(DragKind::PaletteSb) | Some(DragKind::PaletteHue) => {
                let c = self.picker_rgb();
                let mut prefs = self.color_prefs();
                prefs.push_recent(c);
                self.recent_colors = prefs.recent_colors;
                self.mark_prefs_dirty();
                self.opacity_undid = false;
                cx.notify();
            }
            Some(DragKind::Erase {
                start_ix,
                start_iy,
                undid,
                wiping,
            }) => {
                if !wiping {
                    if !undid {
                        self.push_undo();
                    }
                    if self.erase_topmost_at(start_ix, start_iy) {
                        self.status = format!("已擦除顶层 · 蒙版 {} 个", self.masks.len()).into();
                    } else {
                        self.undo_stack.pop();
                        self.status = "未点到可擦除蒙版.".into();
                    }
                } else {
                    self.status = format!("蒙版 {} 个", self.masks.len()).into();
                }
                cx.notify();
            }
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.render_image.is_none() {
            return;
        }
        // 无模式禁止移动页面; Ctrl+滚轮缩放始终可用
        if !ev.modifiers.control {
            if self.mode == ToolMode::Select {
                return;
            }
            let delta = match ev.delta {
                ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
                ScrollDelta::Lines(p) => (p.x * 40.0, p.y * 40.0),
            };
            self.pan.x += delta.0;
            self.pan.y += delta.1;
            self.user_zoomed = true;
            cx.notify();
            return;
        }
        let factor = match ev.delta {
            ScrollDelta::Pixels(p) => {
                if f32::from(p.y) > 0.0 || f32::from(p.x) > 0.0 {
                    1.15
                } else {
                    1.0 / 1.15
                }
            }
            ScrollDelta::Lines(p) => {
                if p.y > 0.0 || p.x > 0.0 {
                    1.15
                } else {
                    1.0 / 1.15
                }
            }
        };
        let (sx, sy) = self.screen_in_view(ev.position);
        let old = self.xform();
        let (ix, iy) = old.screen_to_image(sx, sy);

        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        let fit = if self.img_w > 0 && self.img_h > 0 && vw > 1.0 && vh > 1.0 {
            (vw / self.img_w as f32).min(vh / self.img_h as f32).max(0.0001)
        } else {
            1.0
        };
        let current_zoom = if self.user_zoomed {
            self.zoom
        } else {
            1.0
        };
        self.user_zoomed = true;
        self.zoom = (current_zoom * factor).clamp(0.05, 40.0);
        let new_scale = fit * self.zoom;
        // 保持鼠标下图像点不变
        self.pan.x = sx - (vw - self.img_w as f32 * new_scale) * 0.5 - ix * new_scale;
        self.pan.y = sy - (vh - self.img_h as f32 * new_scale) * 0.5 - iy * new_scale;
        cx.notify();
    }

    fn btn(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        grow: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
        let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
        let hover = if active { rgb(0x1d4ed8) } else { rgb(0xcbd5e1) };
        let mut el = div()
            .id(id.into())
            .px_3()
            .py_1()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(rgb(0x94a3b8))
            .text_color(fg)
            .cursor_pointer()
            .hover(move |s| s.bg(hover));
        if grow {
            el = el.flex_1().flex().justify_center().min_w(px(0.));
        }
        el.child(label.into()).on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
        )
    }

    pub fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.btn("open", "打开", false, false, Self::open_file, cx))
            .child(self.btn("export", "导出", false, false, Self::export_image, cx))
            .child(self.btn(
                "fit",
                "适应窗口",
                false,
                false,
                |this, _, cx| this.fit_to_view(cx),
                cx,
            ))
            .child(self.btn(
                "del",
                "删除",
                false,
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x64748b))
                    .child(self.status.clone()),
            )
    }

    fn color_picker_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let slider_op = self.slider_opacity_value();
        let opacity_pct = (slider_op * 100.0).round() as i32;
        let opacity_label = if self.selected.is_empty() {
            format!("不透明度  {opacity_pct}%")
        } else {
            format!("选中项不透明度  {opacity_pct}%")
        };
        let frac = ((slider_op - 0.05) / 0.95).clamp(0.0, 1.0);
        let sb_img = self.sb_image.clone();
        let hue_img = self.hue_image.clone();
        let picker_s = self.picker_s;
        let picker_v = self.picker_v;
        let picker_h = self.picker_h;
        let recent: Vec<[u8; 3]> = self
            .recent_colors
            .iter()
            .copied()
            .take(RECENT_COLORS_MAX)
            .collect();

        div()
            .id("color_picker_popover")
            .w_full()
            .p_2()
            .rounded_md()
            .bg(rgb(0x1e293b))
            .border_1()
            .border_color(rgb(0x334155))
            .flex()
            .flex_col()
            .gap_2()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0xcbd5e1))
                    .child(opacity_label),
            )
            .child({
                div()
                    .id("palette_opacity_track")
                    .relative()
                    .w_full()
                    .h(px(14.))
                    .rounded_full()
                    .bg(rgb(0x334155))
                    .border_1()
                    .border_color(rgb(0x475569))
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(
                        canvas(
                            {
                                let entity = cx.entity().clone();
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, _| {
                                        this.opacity_track = bounds;
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .size_full()
                        .absolute(),
                    )
                    .child(
                        div()
                            .h_full()
                            .w(relative(frac))
                            .bg(rgb(0x38bdf8))
                            .rounded_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.opacity_undid = false;
                            this.drag = Some(DragKind::PaletteOpacity);
                            this.set_palette_opacity_from_x(f32::from(ev.position.x), cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if matches!(this.drag, Some(DragKind::PaletteOpacity)) {
                                this.drag = None;
                                this.opacity_undid = false;
                                cx.notify();
                            }
                        }),
                    )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .children(recent.into_iter().enumerate().map(|(i, color)| {
                        let color_u32 = color_rgb_u32(color);
                        div()
                            .id(SharedString::from(format!("recent-{i}")))
                            .size(px(22.))
                            .rounded_sm()
                            .bg(rgb(color_u32))
                            .border_1()
                            .border_color(rgb(0x64748b))
                            .cursor_pointer()
                            .hover(|s| s.border_color(rgb(0x94a3b8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pick_recent_color(color, cx);
                                }),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .id("palette_sb")
                            .relative()
                            .size(px(SB_SIZE))
                            .flex_shrink_0()
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(0x475569))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    {
                                        let entity = cx.entity().clone();
                                        move |bounds, _, cx| {
                                            entity.update(cx, |this, _| {
                                                this.sb_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = sb_img {
                                            let b = Bounds {
                                                origin: bounds.origin,
                                                size: bounds.size,
                                            };
                                            let _ = window.paint_image(
                                                b,
                                                Corners::default(),
                                                img.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                        let mx = bounds.origin.x
                                            + px(picker_s * f32::from(bounds.size.width));
                                        let my = bounds.origin.y
                                            + px((1.0 - picker_v) * f32::from(bounds.size.height));
                                        let mark = Bounds {
                                            origin: point(mx - px(5.), my - px(5.)),
                                            size: size(px(10.), px(10.)),
                                        };
                                        window.paint_quad(quad(
                                            mark,
                                            px(5.),
                                            rgb(0xffffff),
                                            px(1.5),
                                            rgb(0x0f172a),
                                            Default::default(),
                                        ));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.opacity_undid = false;
                                    this.drag = Some(DragKind::PaletteSb);
                                    this.set_palette_sb_from_pos(
                                        f32::from(ev.position.x),
                                        f32::from(ev.position.y),
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::PaletteSb)) {
                                        this.drag = None;
                                        this.opacity_undid = false;
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("palette_hue")
                            .relative()
                            .w(px(HUE_BAR_W))
                            .h(px(SB_SIZE))
                            .flex_shrink_0()
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(0x475569))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    {
                                        let entity = cx.entity().clone();
                                        move |bounds, _, cx| {
                                            entity.update(cx, |this, _| {
                                                this.hue_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = hue_img {
                                            let b = Bounds {
                                                origin: bounds.origin,
                                                size: bounds.size,
                                            };
                                            let _ = window.paint_image(
                                                b,
                                                Corners::default(),
                                                img.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                        let hy = bounds.origin.y
                                            + px((picker_h / 360.0).clamp(0.0, 1.0)
                                                * f32::from(bounds.size.height));
                                        let mark = Bounds {
                                            origin: point(
                                                bounds.origin.x,
                                                hy - px(2.),
                                            ),
                                            size: size(bounds.size.width, px(4.)),
                                        };
                                        window.paint_quad(quad(
                                            mark,
                                            px(0.),
                                            rgb(0xffffff),
                                            px(1.),
                                            rgb(0x0f172a),
                                            Default::default(),
                                        ));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.opacity_undid = false;
                                    this.drag = Some(DragKind::PaletteHue);
                                    this.set_palette_hue_from_y(f32::from(ev.position.y), cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::PaletteHue)) {
                                        this.drag = None;
                                        this.opacity_undid = false;
                                        cx.notify();
                                    }
                                }),
                            ),
                    ),
            )
            .child({
                let r_in = self.rgb_r_input.clone();
                let g_in = self.rgb_g_input.clone();
                let b_in = self.rgb_b_input.clone();
                let drop_on = self.eyedropper_armed;
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xcbd5e1))
                                    .child("RGB"),
                            )
                            .child(
                                div()
                                    .id("eyedropper_btn")
                                    .size(px(20.))
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(if drop_on {
                                        rgb(0x38bdf8)
                                    } else {
                                        rgb(0x475569)
                                    })
                                    .bg(if drop_on {
                                        rgb(0x0ea5e9)
                                    } else {
                                        rgb(0x334155)
                                    })
                                    .child(eyedropper_icon(drop_on))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.arm_eyedropper(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("R"),
                            )
                            .child(
                                div()
                                    .id("rgb_r_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(r_in),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("G"),
                            )
                            .child(
                                div()
                                    .id("rgb_g_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(g_in),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("B"),
                            )
                            .child(
                                div()
                                    .id("rgb_b_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(b_in),
                            ),
                    )
            })
    }

    pub fn side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mask_color_u32 = color_rgb_u32(self.mask_color);
        let list_items: Vec<_> = self
            .masks
            .iter()
            .map(|m| {
                let id = m.id.clone();
                let label = m.label();
                let selected = self.selected.contains(&m.id);
                (id, label, selected)
            })
            .collect();
        let side_w = if self.embed_side_width > 1.0 {
            self.embed_side_width
        } else {
            280.0
        };
        let embedded = self.embed_side_width > 1.0;

        let mut panel = div()
            .id("mask_side")
            .relative()
            .h_full()
            .flex()
            .flex_col()
            // padding 放内层, 避免绝对定位悬浮窗与侧栏 bounds 坐标系差出 padding 偏移
            .bg(rgb(0xf1f5f9));
        if embedded {
            panel = panel.w_full();
        } else {
            panel = panel
                .w(px(side_w))
                .border_l_1()
                .border_color(rgb(0xcbd5e1));
        }
        panel
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.side_bounds = bounds;
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("mask_side_inner")
                    .flex_1()
                    .w_full()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0x334155))
                            .child(if embedded {
                                "蒙版列表 (Ctrl+A 全选 · Delete 删除)"
                            } else {
                                "蒙版列表 (选中后 Delete 删除)"
                            }),
                    )
            .child(
                div()
                    .id("mask_list")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xcbd5e1))
                    .rounded_md()
                    .p_1()
                    .children(list_items.into_iter().map(|(id, label, selected)| {
                        let id_click = id.clone();
                        let bg = if selected {
                            rgb(0xdbeafe)
                        } else {
                            rgb(0xffffff)
                        };
                        div()
                            .id(SharedString::from(format!("mask-{id}")))
                            .w_full()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(bg)
                            .text_sm()
                            .text_color(rgb(0x0f172a))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xe2e8f0)))
                            .child(label)
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                                    if ev.modifiers.control {
                                        if this.selected.contains(&id_click) {
                                            this.selected.remove(&id_click);
                                        } else {
                                            this.selected.insert(id_click.clone());
                                        }
                                    } else {
                                        this.selected.clear();
                                        this.selected.insert(id_click.clone());
                                    }
                                    cx.notify();
                                }),
                            )
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child({
                        let brush_on = self.mode == ToolMode::Brush;
                        let eraser_on = self.mode == ToolMode::Eraser;
                        let size_frac = ((self.brush_size - BRUSH_SIZE_MIN)
                            / (BRUSH_SIZE_MAX - BRUSH_SIZE_MIN))
                            .clamp(0.0, 1.0);
                        let brush_px = self.brush_size.round() as i32;
                        let brush_color_u32 = color_rgb_u32(self.brush_color);
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .child(self.btn(
                                "mode_brush",
                                "画笔",
                                brush_on,
                                false,
                                |this, _, cx| this.toggle_brush_mode(cx),
                                cx,
                            ))
                            .child(
                                div()
                                    .id("brush_color_swatch")
                                    .relative()
                                    .size(px(28.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(brush_color_u32))
                                    .border_2()
                                    .border_color(rgb(0x000000))
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(rgb(0x334155)))
                                    .child(
                                        canvas(
                                            {
                                                let entity = cx.entity().clone();
                                                move |bounds, _, cx| {
                                                    entity.update(cx, |this, _| {
                                                        this.brush_swatch_bounds = bounds;
                                                    });
                                                }
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_color_picker(ColorPickerTarget::Brush, cx);
                                            if this.mode != ToolMode::Brush {
                                                this.mode = ToolMode::Brush;
                                                this.status = Self::mode_status(ToolMode::Brush);
                                            }
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(28.))
                                    .child(
                                        div()
                                            .absolute()
                                            .left(relative(size_frac))
                                            .bottom(px(16.))
                                            .ml(px(-14.))
                                            .whitespace_nowrap()
                                            .text_xs()
                                            .text_color(rgb(0x64748b))
                                            .child(format!("{brush_px}px")),
                                    )
                                    .child(
                                        div()
                                            .id("brush_size_track")
                                            .absolute()
                                            .left_0()
                                            .right_0()
                                            .bottom_0()
                                            .h(px(14.))
                                            .rounded_full()
                                            .bg(rgb(0xe2e8f0))
                                            .border_1()
                                            .border_color(rgb(0x94a3b8))
                                            .overflow_hidden()
                                            .cursor_pointer()
                                            .child(
                                                canvas(
                                                    {
                                                        let entity = cx.entity().clone();
                                                        move |bounds, _, cx| {
                                                            entity.update(cx, |this, _| {
                                                                this.brush_size_track = bounds;
                                                            });
                                                        }
                                                    },
                                                    |_, _, _, _| {},
                                                )
                                                .size_full()
                                                .absolute(),
                                            )
                                            .child(
                                                div()
                                                    .h_full()
                                                    .w(relative(size_frac))
                                                    .bg(rgb(0x2563eb))
                                                    .rounded_full(),
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                                    this.drag = Some(DragKind::BrushSize);
                                                    this.set_brush_size_from_x(
                                                        f32::from(ev.position.x),
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    if matches!(this.drag, Some(DragKind::BrushSize))
                                                    {
                                                        this.drag = None;
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                    ),
                            )
                            .child(self.btn(
                                "mode_eraser",
                                "橡皮",
                                eraser_on,
                                false,
                                |this, _, cx| this.toggle_eraser_mode(cx),
                                cx,
                            ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .child(self.btn(
                                "mode_draw",
                                "框选 (B)",
                                self.mode == ToolMode::Draw,
                                true,
                                |this, _, cx| this.toggle_draw_mode(cx),
                                cx,
                            ))
                            .child(self.btn(
                                "mode_poly",
                                "折线 (L)",
                                self.mode == ToolMode::Poly,
                                true,
                                |this, _, cx| this.toggle_poly_mode(cx),
                                cx,
                            ))
                            .child(
                                div()
                                    .id("mask_color_swatch")
                                    .relative()
                                    .size(px(28.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(mask_color_u32))
                                    .border_2()
                                    .border_color(rgb(0x000000))
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(rgb(0x334155)))
                                    .child(
                                        canvas(
                                            {
                                                let entity = cx.entity().clone();
                                                move |bounds, _, cx| {
                                                    entity.update(cx, |this, _| {
                                                        this.mask_swatch_bounds = bounds;
                                                    });
                                                }
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_color_picker(ColorPickerTarget::Mask, cx);
                                        }),
                                    ),
                            )
                            .child(self.btn(
                                "mode_pan",
                                "平移 (P)",
                                self.mode == ToolMode::Pan,
                                true,
                                |this, _, cx| this.toggle_pan_mode(cx),
                                cx,
                            )),
                    )
                    .when(!embedded, |d| {
                        d.child(self.btn(
                            "btn_del",
                            "删除选中蒙版",
                            false,
                            false,
                            |this, _, cx| this.delete_selected(cx),
                            cx,
                        ))
                        .child(self.btn(
                            "btn_clear",
                            "清空全部蒙版",
                            false,
                            false,
                            |this, _, cx| this.clear_masks(cx),
                            cx,
                        ))
                    })
                    .child(self.btn(
                        "btn_export",
                        if embedded {
                            "导出本页图片 (E)…"
                        } else {
                            "导出已遮盖图片 (E)…"
                        },
                        false,
                        true,
                        Self::export_image,
                        cx,
                    )),
            ) // tools column
            ) // mask_side_inner
            .when(self.color_picker_open, |d| d.child(self.color_picker_floating(cx)))
    }

    pub fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let render = self.render_image.clone();
        let masks = self.masks.clone();
        let selected = self.selected.clone();
        let img_w = self.img_w;
        let img_h = self.img_h;
        let zoom = self.zoom;
        let pan = self.pan;
        let user_zoomed = self.user_zoomed;
        let rubber = match &self.drag {
            Some(DragKind::Draw { x0, y0, x1, y1 }) => Some((*x0, *y0, *x1, *y1, false)),
            Some(DragKind::Marquee { x0, y0, x1, y1, .. }) => Some((*x0, *y0, *x1, *y1, true)),
            _ => None,
        };
        let poly_draft = self.poly_draft.clone();
        let poly_cursor = self.poly_cursor;
        let brush_cursor = self.brush_cursor;
        let brush_size = self.brush_size;
        let brush_color = self.brush_color;
        let brush_opacity = self.brush_opacity;
        let mask_color = self.mask_color;
        let cursor = if self.eyedropper_armed {
            CursorStyle::Crosshair
        } else {
            match self.mode {
                ToolMode::Brush => CursorStyle::None,
                ToolMode::Draw | ToolMode::Poly | ToolMode::Eraser => CursorStyle::Crosshair,
                ToolMode::Pan => CursorStyle::OpenHand,
                ToolMode::Select => CursorStyle::Arrow,
            }
        };

        div()
            .id("mask_image_view")
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(0x2b2b2b))
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_view_mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    if this.eyedropper_armed {
                        this.cancel_eyedropper(cx);
                    } else if this.mode == ToolMode::Poly {
                        this.cancel_poly_draft(cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(p) = first_image_in_paths(paths.paths()) {
                    this.load_image(p, cx);
                }
            }))
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.view_bounds = bounds;
                            });
                        }
                    },
                    move |bounds, _, window, _cx| {
                        let vw = f32::from(bounds.size.width);
                        let vh = f32::from(bounds.size.height);
                        let xform = ViewXform::compute(
                            img_w as f32,
                            img_h as f32,
                            vw,
                            vh,
                            zoom,
                            pan,
                            user_zoomed,
                        );

                        if let Some(ref img) = render {
                            let img_bounds = Bounds {
                                origin: point(
                                    bounds.origin.x + px(xform.origin_x),
                                    bounds.origin.y + px(xform.origin_y),
                                ),
                                size: size(
                                    px(img_w as f32 * xform.scale),
                                    px(img_h as f32 * xform.scale),
                                ),
                            };
                            let _ = window.paint_image(
                                img_bounds,
                                Corners::default(),
                                img.clone(),
                                0,
                                false,
                            );
                        }

                        for m in &masks {
                            if m.is_brush() {
                                let r = m.brush_radius.max(1) as f32;
                                let diam = (r * 2.0 * xform.scale).max(2.0);
                                let color_u32 = ((m.color[0] as u32) << 16)
                                    | ((m.color[1] as u32) << 8)
                                    | (m.color[2] as u32);
                                let mut fill_c = rgb(color_u32);
                                fill_c.a = m.effective_opacity();
                                let is_sel = selected.contains(&m.id);
                                let border = if is_sel {
                                    rgb(0xdc5050)
                                } else {
                                    rgb(0xb4b4b4)
                                };
                                // 圆章叠画 (与导出一致); 不用整条 stroke, 避免折返畸形
                                if is_sel {
                                    paint_brush_stamps(
                                        window,
                                        &m.brush_points,
                                        r,
                                        xform.scale,
                                        xform.origin_x,
                                        xform.origin_y,
                                        bounds.origin,
                                        diam + 4.0,
                                        border,
                                    );
                                }
                                paint_brush_stamps(
                                    window,
                                    &m.brush_points,
                                    r,
                                    xform.scale,
                                    xform.origin_x,
                                    xform.origin_y,
                                    bounds.origin,
                                    diam,
                                    fill_c,
                                );
                                continue;
                            }

                            if m.is_poly() {
                                let color_u32 = color_rgb_u32(m.color);
                                let mut fill_c = rgb(color_u32);
                                fill_c.a = m.effective_opacity();
                                let border = if selected.contains(&m.id) {
                                    rgb(0xdc5050)
                                } else {
                                    rgb(0xb4b4b4)
                                };
                                let mut fill_builder = PathBuilder::fill();
                                let mut stroke_builder = PathBuilder::stroke(if selected
                                    .contains(&m.id)
                                {
                                    px(2.)
                                } else {
                                    px(1.)
                                });
                                let mut first = true;
                                for &(px_i, py_i) in &m.poly_points {
                                    let sx = bounds.origin.x
                                        + px(xform.origin_x + px_i as f32 * xform.scale);
                                    let sy = bounds.origin.y
                                        + px(xform.origin_y + py_i as f32 * xform.scale);
                                    let pt = point(sx, sy);
                                    if first {
                                        fill_builder.move_to(pt);
                                        stroke_builder.move_to(pt);
                                        first = false;
                                    } else {
                                        fill_builder.line_to(pt);
                                        stroke_builder.line_to(pt);
                                    }
                                }
                                fill_builder.close();
                                stroke_builder.close();
                                if let Ok(path) = fill_builder.build() {
                                    window.paint_path(path, fill_c);
                                }
                                if let Ok(path) = stroke_builder.build() {
                                    window.paint_path(path, border);
                                }
                                continue;
                            }

                            let r = m.normalized();
                            let mut b = xform.image_rect_to_screen(r.x0, r.y0, r.x1, r.y1);
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let color_u32 = color_rgb_u32(m.color);
                            let mut fill_c = rgb(color_u32);
                            fill_c.a = m.effective_opacity();
                            let border = if selected.contains(&m.id) {
                                rgb(0xdc5050)
                            } else {
                                rgb(0xb4b4b4)
                            };
                            let border_w = if selected.contains(&m.id) {
                                px(2.)
                            } else {
                                px(1.)
                            };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill_c,
                                border_w,
                                border,
                                Default::default(),
                            ));
                        }

                        if let Some((x0, y0, x1, y1, is_marquee)) = rubber {
                            let min_x = x0.min(x1) as i32;
                            let min_y = y0.min(y1) as i32;
                            let max_x = x0.max(x1) as i32;
                            let max_y = y0.max(y1) as i32;
                            let mut b = xform.image_rect_to_screen(min_x, min_y, max_x, max_y);
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill_c = if is_marquee {
                                rgb(0x3b82f6)
                            } else {
                                rgb(color_rgb_u32(mask_color))
                            };
                            fill_c.a = if is_marquee { 0.18 } else { 0.24 };
                            let border = if is_marquee {
                                rgb(0x2563eb)
                            } else {
                                rgb(0x508cdc)
                            };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill_c,
                                px(1.),
                                border,
                                Default::default(),
                            ));
                            let mut builder = PathBuilder::stroke(px(1.));
                            builder = builder.dash_array(&[px(4.), px(3.)]);
                            builder.move_to(b.origin);
                            builder.line_to(point(b.origin.x + b.size.width, b.origin.y));
                            builder.line_to(point(
                                b.origin.x + b.size.width,
                                b.origin.y + b.size.height,
                            ));
                            builder.line_to(point(b.origin.x, b.origin.y + b.size.height));
                            builder.close();
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, border);
                            }
                        }

                        // 折线草稿 + 橡皮筋
                        if let Some(ref draft) = poly_draft {
                            if !draft.is_empty() {
                                let to_screen = |ix: f32, iy: f32| {
                                    point(
                                        bounds.origin.x + px(xform.origin_x + ix * xform.scale),
                                        bounds.origin.y + px(xform.origin_y + iy * xform.scale),
                                    )
                                };
                                let mut stroke = PathBuilder::stroke(px(1.5));
                                stroke = stroke.dash_array(&[px(5.), px(3.)]);
                                let first = to_screen(draft[0].0, draft[0].1);
                                stroke.move_to(first);
                                for &(ix, iy) in draft.iter().skip(1) {
                                    stroke.line_to(to_screen(ix, iy));
                                }
                                if let Some((cx_i, cy_i)) = poly_cursor {
                                    stroke.line_to(to_screen(cx_i, cy_i));
                                }
                                if let Ok(path) = stroke.build() {
                                    window.paint_path(path, rgb(0x38bdf8));
                                }
                                // 顶点小方块; 首点在可吸附时加大
                                let can_snap = draft.len() >= 3
                                    && poly_cursor
                                        .map(|(cx_i, cy_i)| {
                                            (cx_i - draft[0].0).abs() < 0.01
                                                && (cy_i - draft[0].1).abs() < 0.01
                                        })
                                        .unwrap_or(false);
                                for (i, &(ix, iy)) in draft.iter().enumerate() {
                                    let p = to_screen(ix, iy);
                                    let sz = if i == 0 && can_snap { 10.0 } else { 6.0 };
                                    let b = Bounds {
                                        origin: point(p.x - px(sz * 0.5), p.y - px(sz * 0.5)),
                                        size: size(px(sz), px(sz)),
                                    };
                                    let col = if i == 0 {
                                        rgb(0xf97316)
                                    } else {
                                        rgb(0x38bdf8)
                                    };
                                    window.paint_quad(quad(
                                        b,
                                        px(1.),
                                        col,
                                        px(1.),
                                        rgb(0xffffff),
                                        Default::default(),
                                    ));
                                }
                            }
                        }

                        // 画笔圆形光标 (与粗细/颜色一致)
                        if let Some((bx, by)) = brush_cursor {
                            let screen_r = (brush_size * 0.5 * xform.scale).max(1.5);
                            let cx_s = f32::from(bounds.origin.x) + xform.origin_x + bx * xform.scale;
                            let cy_s = f32::from(bounds.origin.y) + xform.origin_y + by * xform.scale;
                            let [cr, cg, cb] = brush_color;
                            let mut fill = rgb(
                                ((cr as u32) << 16) | ((cg as u32) << 8) | (cb as u32),
                            );
                            fill.a = (brush_opacity * 0.35).clamp(0.12, 0.55);
                            let mut ring = rgb(
                                ((cr as u32) << 16) | ((cg as u32) << 8) | (cb as u32),
                            );
                            ring.a = brush_opacity.clamp(0.45, 1.0);
                            let b = Bounds {
                                origin: point(px(cx_s - screen_r), px(cy_s - screen_r)),
                                size: size(px(screen_r * 2.0), px(screen_r * 2.0)),
                            };
                            window.paint_quad(quad(
                                b,
                                px(screen_r),
                                fill,
                                px(1.5),
                                ring,
                                Default::default(),
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }
}

impl Focusable for MaskToolApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MaskToolApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = match &self.image_path {
            Some(p) => format!(
                "蒙版遮盖 — {}",
                p.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image")
            )
            .into(),
            None => "蒙版遮盖 / Mask Overlay".into(),
        };

        div()
            .id("root")
            .key_context("MaskTool")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| this.open_file(window, cx)))
            .on_action(cx.listener(|this, _: &ExportImage, window, cx| {
                this.export_image(window, cx)
            }))
            .on_action(cx.listener(|this, _: &FitView, _, cx| this.fit_to_view(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| {
                this.delete_selected(cx)
            }))
            .on_action(cx.listener(|this, _: &ClearMasks, _, cx| this.clear_masks(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all_masks(cx)))
            .on_action(cx.listener(|this, _: &ToggleDrawMode, _, cx| {
                this.toggle_draw_mode(cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePanMode, _, cx| this.toggle_pan_mode(cx)))
            .on_action(cx.listener(|this, _: &ToggleBrushMode, _, cx| {
                this.toggle_brush_mode(cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePolyMode, _, cx| {
                this.toggle_poly_mode(cx)
            }))
            .on_action(cx.listener(|this, _: &CancelPolyDraft, _, cx| {
                this.cancel_poly_draft(cx)
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo(cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(p) = first_image_in_paths(paths.paths()) {
                    this.load_image(p, cx);
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf8fafc))
            .text_color(rgb(0x0f172a))
            .font_family("Microsoft YaHei UI")
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0xcbd5e1))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(self.toolbar(cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.image_view(cx))
                    .child(self.side_panel(cx)),
            )
    }
}

pub fn run_gui(initial: Option<PathBuf>) {
    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-o", OpenFile, Some("MaskTool")),
            KeyBinding::new("e", ExportImage, Some("MaskTool")),
            KeyBinding::new("f", FitView, Some("MaskTool")),
            KeyBinding::new("delete", DeleteSelected, Some("MaskTool")),
            KeyBinding::new("backspace", DeleteSelected, Some("MaskTool")),
            KeyBinding::new("b", ToggleDrawMode, Some("MaskTool")),
            KeyBinding::new("l", TogglePolyMode, Some("MaskTool")),
            KeyBinding::new("p", TogglePanMode, Some("MaskTool")),
            KeyBinding::new("escape", CancelPolyDraft, Some("MaskTool")),
            KeyBinding::new("ctrl-a", SelectAll, Some("MaskTool")),
            KeyBinding::new("ctrl-z", Undo, Some("MaskTool")),
            KeyBinding::new("ctrl-y", Redo, Some("MaskTool")),
            KeyBinding::new("ctrl-shift-z", Redo, Some("MaskTool")),
        ]);
        let bounds = Bounds::centered(None, size(px(1200.), px(860.)), cx);
        let initial = initial.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("蒙版遮盖 / Mask Overlay".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let app = MaskToolApp::new(cx, initial.clone());
                    app.focus_handle.focus(window);
                    app
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
