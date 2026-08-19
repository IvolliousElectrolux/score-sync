//! GPUI 图形界面: 框选半透明白蒙版.
//!
//! 按职责拆开, `MaskToolApp` 仍是唯一状态机:
//! - `types` 常量/枚举/变换
//! - `picker` 浮动取色器
//! - `tools` 工具模式与撤重
//! - `io` 加载/导出
//! - `canvas` 画布交互
//! - `chrome` 工具栏与侧栏

mod canvas;
mod chrome;
mod io;
mod picker;
mod tools;
mod types;

pub(crate) use types::*;

pub(crate) use std::collections::HashSet;
pub(crate) use std::path::PathBuf;
pub(crate) use std::sync::Arc;

pub(crate) use gpui::{
    actions, canvas, div, point, prelude::*, px, quad, relative, rgb, size, App, Application,
    Bounds, Context, Corners, CursorStyle, Entity, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
pub(crate) use image::{Frame, ImageBuffer, Rgb, RgbaImage};
pub(crate) use smallvec::smallvec;

pub(crate) use crate::color_prefs::{
    hsv_to_rgb, rgb_to_hsv, MaskColorPrefs, DEFAULT_BRUSH_OPACITY, RECENT_COLORS_MAX,
};
pub(crate) use crate::mask::{
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
            .font_family(apply_bg::ui_font())
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
