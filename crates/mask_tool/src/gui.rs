//! GPUI 图形界面: 框选半透明白蒙版.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    actions, canvas, div, point, prelude::*, px, quad, relative, rgb, size, App, Application,
    Bounds, Context, Corners, CursorStyle, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
use image::{Frame, ImageBuffer, Rgb, RgbaImage};
use smallvec::smallvec;

use crate::mask::{
    default_export_path, export_masked, first_image_in_paths, is_image_path, new_id, MaskRect,
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
        Undo,
        Redo
    ]
);

const HISTORY_LIMIT: usize = 64;

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
    Opacity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolMode {
    /// 两模式都关: 只能选中 (含 Ctrl 多选 / Shift 拖选), 不能拖动画布
    Select,
    /// 框选新蒙版
    Draw,
    /// 空白拖动画布; 点在已选蒙版上则拖动蒙版
    Pan,
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
    opacity: f32,
    mode: ToolMode,
    zoom: f32,
    pan: Point<f32>,
    user_zoomed: bool,
    view_bounds: Bounds<Pixels>,
    opacity_track: Bounds<Pixels>,
    drag: Option<DragKind>,
    status: SharedString,
    hint: SharedString,
    /// 嵌入宿主时侧栏宽度 (0 = 用默认 280)
    embed_side_width: f32,
    /// 嵌入会话键 (组内成员); 与 path 二选一标识当前图
    session_key: Option<String>,
}

impl MaskToolApp {
    pub fn new(cx: &mut Context<Self>, initial: Option<PathBuf>) -> Self {
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
            opacity: 0.72,
            mode: ToolMode::Draw,
            zoom: 1.0,
            pan: point(0.0, 0.0),
            user_zoomed: false,
            view_bounds: Bounds::default(),
            opacity_track: Bounds::default(),
            drag: None,
            status: "就绪".into(),
            hint: "框选: 拖出蒙版. 平移: 空白拖动画布 / 拖已选蒙版.\n再点一次当前模式按钮可退出, 此时仅选择 (Ctrl 多选, Shift 拖选).\nCtrl+滚轮缩放 · Ctrl+Z/Y 撤重."
                .into(),
            embed_side_width: 0.0,
            session_key: None,
        };
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
        self.opacity
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.05, 1.0);
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
        self.session_key = Some(session_key);
        self.rgb_image = Some(rgb);
        self.render_image = Some(render);
        self.img_w = w;
        self.img_h = h;
        self.masks = masks;
        self.selected.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.user_zoomed = false;
        self.drag = None;
        self.status = format!("{label} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
        self.hint = format!(
            "编辑: {label}\n蒙版坐标相对本组合拼合图; 各组合独立 (共享脚注可在不同组画不同遮盖)."
        )
        .into();
        cx.notify();
    }

    pub fn clear_view(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
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
                self.image_path = Some(path.clone());
                self.session_key = None;
                self.rgb_image = Some(rgb);
                self.render_image = Some(render);
                self.img_w = w;
                self.img_h = h;
                self.masks = restored;
                self.selected.clear();
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.zoom = 1.0;
                self.pan = point(0.0, 0.0);
                self.user_zoomed = false;
                self.drag = None;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image");
                self.status = format!("已载入 {name} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
                self.hint = format!(
                    "已载入 {name}. 框选画蒙版; 平移拖动画布或已选框; 再点模式退出后 Shift 拖选."
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
        self.status = match self.mode {
            ToolMode::Draw => "框选".into(),
            ToolMode::Select => "选择 (可 Ctrl 多选 / Shift 拖选)".into(),
            ToolMode::Pan => "平移".into(),
        };
        cx.notify();
    }

    pub fn toggle_pan_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Pan {
            ToolMode::Select
        } else {
            ToolMode::Pan
        };
        self.drag = None;
        self.status = match self.mode {
            ToolMode::Draw => "框选".into(),
            ToolMode::Select => "选择 (可 Ctrl 多选 / Shift 拖选)".into(),
            ToolMode::Pan => "平移".into(),
        };
        cx.notify();
    }

    fn hit_mask(&self, ix: f32, iy: f32) -> Option<String> {
        self.masks
            .iter()
            .rev()
            .find(|m| m.contains(ix, iy))
            .map(|m| m.id.clone())
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
        match export_masked(base, &self.masks, self.opacity, &path) {
            Ok(()) => {
                self.status = format!("已保存: {}", path.display()).into();
            }
            Err(e) => {
                self.status = e.into();
            }
        }
        cx.notify();
    }

    fn set_opacity_from_x(&mut self, x: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.opacity_track.origin.x);
        let width = f32::from(self.opacity_track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        // slider 20%..100%
        self.opacity = 0.20 + t * 0.80;
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
            return;
        }
        let (sx, sy) = self.screen_in_view(ev.position);
        let xform = self.xform();
        let (ix, iy) = xform.screen_to_image(sx, sy);
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

    fn on_view_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (sx, sy) = self.screen_in_view(ev.position);
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
            Some(DragKind::PagePan { last }) => {
                let dx = f32::from(ev.position.x) - f32::from(last.x);
                let dy = f32::from(ev.position.y) - f32::from(last.y);
                self.pan.x += dx;
                self.pan.y += dy;
                self.user_zoomed = true;
                self.drag = Some(DragKind::PagePan {
                    last: ev.position,
                });
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
            Some(DragKind::Opacity) => {
                self.drag = Some(DragKind::Opacity);
                self.set_opacity_from_x(f32::from(ev.position.x), cx);
            }
            None => {}
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
                    });
                    self.selected.clear();
                    self.selected.insert(mid);
                    self.status = format!("蒙版 {} 个", self.masks.len()).into();
                }
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
            | Some(DragKind::Opacity)
            | None => {
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

    pub fn side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let opacity_pct = (self.opacity * 100.0).round() as i32;
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
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x334155))
                            .child(format!("白色不透明度  {opacity_pct}%")),
                    )
                    .child({
                        let frac = ((self.opacity - 0.20) / 0.80).clamp(0.0, 1.0);
                        div()
                            .id("opacity_track")
                            .relative()
                            .w_full()
                            .h(px(16.))
                            .flex_shrink_0()
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
                                    .bg(rgb(0x2563eb))
                                    .rounded_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    this.drag = Some(DragKind::Opacity);
                                    this.set_opacity_from_x(f32::from(ev.position.x), cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::Opacity)) {
                                        this.drag = None;
                                        cx.notify();
                                    }
                                }),
                            )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
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
            )
    }

    pub fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let render = self.render_image.clone();
        let masks = self.masks.clone();
        let selected = self.selected.clone();
        let opacity = self.opacity;
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
        let cursor = match self.mode {
            ToolMode::Draw => CursorStyle::Crosshair,
            ToolMode::Pan => CursorStyle::OpenHand,
            ToolMode::Select => CursorStyle::Arrow,
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
            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
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
                        } else {
                            // 占位提示由外层 status/hint 承担
                        }

                        for m in &masks {
                            let r = m.normalized();
                            let mut b = xform.image_rect_to_screen(r.x0, r.y0, r.x1, r.y1);
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill_c = rgb(0xffffff);
                            fill_c.a = opacity.clamp(0.05, 1.0);
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
                                rgb(0xffffff)
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
            KeyBinding::new("p", TogglePanMode, Some("MaskTool")),
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
