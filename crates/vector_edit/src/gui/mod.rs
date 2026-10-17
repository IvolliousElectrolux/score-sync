//! GPUI 矢量修图. 嵌入宿主时只提供画布, 工具条和侧栏由宿主拆开挂.

mod canvas;
mod chrome;
mod picker;
mod tools;
mod types;

pub use types::{Commit, FitHint, HostCmd, ImportOption, ToolMode};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, div, point, prelude::*, px, rgb, size, App, Application, Bounds, Context, Entity,
    ExternalPaths, FocusHandle, Focusable, KeyBinding, Pixels, Point, Render, RenderImage,
    SharedString, Window, WindowBounds, WindowOptions,
};

use crate::document::{is_scene_dir, VecDocument};
use crate::ids::is_pdf_path;

use types::*;

actions!(
    vector_edit,
    [
        OpenFile,
        ExportImage,
        ExportSvg,
        FitView,
        Undo,
        Redo,
        DeleteSelection,
        CopyLayer,
        PasteLayer,
        NewLayerFromSel,
        GroupSelection,
        UngroupSelection,
        RegroupSelection,
        RenameNode,
        Cancel,
        ApplyEdit,
        RestoreOriginal,
        ToolSelect,
        ToolNode,
        ToolCanvas,
        ToolHand,
        ToolPen,
        ToolPencil,
        ToolEraser,
        ToolLasso,
        SelectAll,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
    ]
);

pub struct VectorEditApp {
    focus_handle: FocusHandle,
    pub(crate) doc: Option<VecDocument>,
    pub(crate) mode: ToolMode,
    pub(crate) selected: Vec<String>,
    /// 已经选中组合时, 点成员松手且没有拖动, 就改选这个对象.
    pending_drill: Option<String>,
    pub(crate) zoom: f32,
    pub(crate) pan: Point<f32>,
    pub(crate) user_zoomed: bool,
    pub(crate) view_bounds: Bounds<Pixels>,
    pub(crate) drag: Option<DragKind>,
    pub(crate) status: SharedString,
    undo_stack: Vec<HistorySnap>,
    redo_stack: Vec<HistorySnap>,
    dirty: bool,
    pub(crate) brush: f32,
    width_track: Bounds<Pixels>,
    width_undo_held: bool,
    pub(crate) fit_hint: FitHint,
    pub(crate) import_options: Vec<ImportOption>,
    host_cmd: Option<HostCmd>,
    standalone: bool,
    pub(crate) nav_only: bool,
    gpu_drop: Vec<Arc<RenderImage>>,
    pub(crate) base_tex: Option<TexSprite>,
    pub(crate) sel_sprite: Option<SelSprite>,
    pub(crate) space_down: bool,
    pub(crate) alt_down: bool,
    pub(crate) shift_down: bool,
    pub(crate) ctrl_down: bool,
    pub(crate) style: StyleBuf,
    /// 色块切到无之后, 再点一次要恢复的填充和描边.
    pub(crate) fill_memory: [u8; 4],
    pub(crate) stroke_memory: [u8; 4],
    /// Illustrator 色块: true 时填充在上层, 色板改填充.
    pub(crate) paint_on_fill: bool,
    palette_open: bool,
    palette_undo_held: bool,
    palette: apply_bg::palette::PaletteModel,
    /// 饱和度图是按这个色相生成的. 拖色相条时隔一度才重画, 避免每格都上传贴图.
    palette_sb_hue: f32,
    palette_paint_job: bool,
    palette_paint_again: bool,
    /// 当前调色盘的不透明度, 色块为无时先记在这里.
    palette_a: u8,
    proxy_bounds: Bounds<Pixels>,
    picker_layer_bounds: Bounds<Pixels>,
    /// 图层行的窗口坐标, 键是显示下标 (0 在最上面).
    layer_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 展开的图层和组.
    expanded: std::collections::HashSet<String>,
    thumbs: HashMap<String, ThumbSlot>,
    /// 取消组合后还能重新组合的记录, 新的在后面.
    regroup_mem: Vec<crate::outline::DissolvedGroup>,
    rename_input: Entity<apply_bg::text_input::TextInput>,
    rename_target: Option<String>,
    /// 回车或失焦之后, 下一帧把焦点交回画布.
    rename_release: bool,
    /// 名称框要等这一帧画出来再聚焦, 否则第一次按键接不住焦点.
    rename_focus: bool,
    rename_bounds: Bounds<Pixels>,
    outline_scroll: gpui::UniformListScrollHandle,
    /// 选择窗格里 Shift 顺序多选的起点.
    outline_anchor: Option<String>,
    outline_menu: Option<OutlineMenu>,
    /// 当前帧看得见的行, 窗口坐标.
    outline_row_hits: Vec<OutlineRowHit>,
    /// 菜单卡片的窗口范围, 右键落在这里就不要改选中.
    menu_bounds: Bounds<Pixels>,
    /// 点了菜单项之后, 紧接着的 mouse_up 不要再改选区.
    suppress_outline_click: bool,
    /// Ctrl+C 记下的图层. 焦点离开这个编辑器就清掉.
    layer_clip: Option<crate::document::LayerRuntime>,
    /// 画布上复制的对象. 和图层剪贴板分开.
    obj_clip: Option<Vec<crate::document::Obj>>,
    /// 最近一次点在画布上. 否则 Ctrl+C/V 作用于图层.
    clip_on_canvas: bool,
    layer_clip_watch: Option<gpui::Subscription>,
    alpha_track: Bounds<Pixels>,
    rgb_r: Entity<apply_bg::text_input::TextInput>,
    rgb_g: Entity<apply_bg::text_input::TextInput>,
    rgb_b: Entity<apply_bg::text_input::TextInput>,
    rgb_a: Entity<apply_bg::text_input::TextInput>,
    rgb_syncing: bool,
    pub(crate) pen: Option<PenDraft>,
    pub(crate) lasso: Vec<(f32, f32)>,
    pub(crate) node_target: Option<String>,
    /// 节点工具下最近一次点中的锚点, Delete 删这个点.
    pub(crate) node_hit: Option<(usize, usize, NodeRole)>,
    image_path: Option<PathBuf>,
    pub(crate) hover_edge: Option<CanvasEdge>,
    pub(crate) hover_doc: Option<(f32, f32)>,
    /// 节点工具下, 指针压住的锚点或曲率手柄.
    hover_node: Option<NodeRole>,
    /// 节点工具下, 指针压住某个对象, 但没压住节点.
    hover_hit: bool,
    raster_px: f32,
    /// 窗口物理像素 / 逻辑像素. 光栅按这个密度出图, 避免高分屏把贴图再放大一档.
    device_scale: f32,
    raster_gen: u64,
    /// 已经排了一次空闲后再光栅, 避免滚轮每格都重画.
    raster_wait: bool,
    /// 后台正在光栅. 这时界面继续用旧图缩放, 不在主线程上堵.
    raster_running: bool,
    /// 画布底色, 跟多数可见对象的墨色反着来.
    pub(crate) backdrop: u32,
}

struct ThumbSlot {
    key: u64,
    tex: Option<Arc<RenderImage>>,
}

pub(crate) struct TexSprite {
    pub tex: Arc<RenderImage>,
    /// 这张贴图覆盖的文档矩形 (point). 放大后只画视口, 不再把整页低分辨率图拉满窗口.
    pub doc_x: f32,
    pub doc_y: f32,
    pub doc_w: f32,
    pub doc_h: f32,
}

pub(crate) struct SelSprite {
    pub tex: Arc<RenderImage>,
    pub doc_x: f32,
    pub doc_y: f32,
    pub doc_w: f32,
    pub doc_h: f32,
    pub dx: f32,
    pub dy: f32,
}

struct HistorySnap {
    doc: VecDocument,
    selected: Vec<String>,
    pan: Point<f32>,
    zoom: f32,
    user_zoomed: bool,
    regroup: Vec<crate::outline::DissolvedGroup>,
}

fn bounds_contains(b: Bounds<Pixels>, x: f32, y: f32) -> bool {
    let left = f32::from(b.origin.x);
    let top = f32::from(b.origin.y);
    x >= left
        && y >= top
        && x < left + f32::from(b.size.width)
        && y < top + f32::from(b.size.height)
}

fn thumb_key(doc: &crate::document::VecDocument, row: &crate::outline::OutlineRow) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let leaves = crate::outline::thumb_leaves(doc, row);
    leaves.len().hash(&mut hasher);
    for id in &leaves {
        let ptr = doc
            .object(id)
            .map(|obj| obj as *const crate::document::Obj as usize)
            .unwrap_or(0);
        ptr.hash(&mut hasher);
    }
    hasher.finish()
}

impl VectorEditApp {
    pub fn new(cx: &mut Context<Self>, standalone: bool) -> Self {
        let rgb_r = cx.new(|cx| {
            apply_bg::text_input::TextInput::new(cx, "0", "R").with_compact(true)
        });
        let rgb_g = cx.new(|cx| {
            apply_bg::text_input::TextInput::new(cx, "0", "G").with_compact(true)
        });
        let rgb_b = cx.new(|cx| {
            apply_bg::text_input::TextInput::new(cx, "0", "B").with_compact(true)
        });
        let rgb_a = cx.new(|cx| {
            apply_bg::text_input::TextInput::new(cx, "255", "A").with_compact(true)
        });
        cx.observe(&rgb_r, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_g, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_b, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        cx.observe(&rgb_a, |this, _, cx| this.apply_rgb_inputs(cx))
            .detach();
        let rename_input = cx.new(|cx| {
            apply_bg::text_input::TextInput::new(cx, "", "名称").with_compact(true)
        });
        cx.observe(&rename_input, |this, _, cx| this.finish_rename_if_blurred(cx))
            .detach();
        Self {
            focus_handle: cx.focus_handle(),
            doc: None,
            mode: ToolMode::Select,
            selected: Vec::new(),
            pending_drill: None,
            zoom: 1.0,
            pan: point(0.0, 0.0),
            user_zoomed: false,
            view_bounds: Bounds::default(),
            drag: None,
            status: "矢量修图".into(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
            brush: 2.0,
            width_track: Bounds::default(),
            width_undo_held: false,
            fit_hint: FitHint::default(),
            import_options: Vec::new(),
            host_cmd: None,
            standalone,
            nav_only: false,
            gpu_drop: Vec::new(),
            base_tex: None,
            sel_sprite: None,
            space_down: false,
            alt_down: false,
            shift_down: false,
            ctrl_down: false,
            style: StyleBuf::default(),
            fill_memory: [255, 255, 255, 255],
            stroke_memory: [0, 0, 0, 255],
            paint_on_fill: true,
            palette_open: false,
            palette_undo_held: false,
            palette: apply_bg::palette::PaletteModel::default(),
            palette_sb_hue: f32::NAN,
            palette_paint_job: false,
            palette_paint_again: false,
            palette_a: 255,
            proxy_bounds: Bounds::default(),
            picker_layer_bounds: Bounds::default(),
            layer_bounds: HashMap::new(),
            expanded: std::collections::HashSet::new(),
            thumbs: HashMap::new(),
            regroup_mem: Vec::new(),
            rename_input,
            rename_target: None,
            rename_release: false,
            rename_focus: false,
            rename_bounds: Bounds::default(),
            outline_scroll: gpui::UniformListScrollHandle::new(),
            outline_anchor: None,
            outline_menu: None,
            outline_row_hits: Vec::new(),
            menu_bounds: Bounds::default(),
            suppress_outline_click: false,
            layer_clip: None,
            obj_clip: None,
            clip_on_canvas: true,
            layer_clip_watch: None,
            alpha_track: Bounds::default(),
            rgb_r,
            rgb_g,
            rgb_b,
            rgb_a,
            rgb_syncing: false,
            pen: None,
            lasso: Vec::new(),
            node_target: None,
            node_hit: None,
            image_path: None,
            hover_edge: None,
            hover_doc: None,
            hover_node: None,
            hover_hit: false,
            raster_px: 1.0,
            device_scale: 1.0,
            raster_gen: 0,
            raster_wait: false,
            raster_running: false,
            backdrop: 0xe7e7e7,
        }
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn open_document(&mut self, doc: VecDocument, cx: &mut Context<Self>) {
        self.doc = Some(doc);
        self.selected.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.regroup_mem.clear();
        self.expanded.clear();
        self.rename_target = None;
        self.rename_release = false;
        self.rename_focus = false;
        if let Some(doc) = self.doc.as_ref() {
            for layer in &doc.layers {
                if layer.objects.len() <= 48 {
                    self.expanded.insert(layer.id.clone());
                }
            }
        }
        self.dirty = false;
        self.pen = None;
        self.lasso.clear();
        self.node_target = None;
        self.node_hit = None;
        self.user_zoomed = false;
        self.fit_to_view(cx);
        // 视口还没量到时按整页 1:1 光栅会卡住. 等布局后由 consider_raster 再画.
        if self.view_ready() {
            self.rebuild_raster(cx);
        }
        self.status = "矢量修图".into();
        cx.notify();
    }

    pub fn set_source(&mut self, source: crate::document::SourceFingerprint) {
        if let Some(d) = self.doc.as_mut() {
            d.source = Some(source);
        }
    }

    pub fn set_fit_hint(&mut self, hint: FitHint, cx: &mut Context<Self>) {
        self.fit_hint = hint;
        cx.notify();
    }

    pub fn set_import_options(&mut self, opts: Vec<ImportOption>, cx: &mut Context<Self>) {
        self.import_options = opts;
        cx.notify();
    }

    pub fn close_session(&mut self, cx: &mut Context<Self>) {
        self.drop_tex();
        self.clear_thumbs();
        self.doc = None;
        self.selected.clear();
        self.expanded.clear();
        self.regroup_mem.clear();
        self.rename_target = None;
        self.rename_focus = false;
        self.host_cmd = None;
        self.dirty = false;
        self.pen = None;
        self.backdrop = 0xe7e7e7;
        cx.notify();
    }

    pub fn has_host_cmd(&self) -> bool {
        self.host_cmd.is_some()
    }

    pub fn take_host_cmd(&mut self) -> Option<HostCmd> {
        self.host_cmd.take()
    }

    pub fn take_commit(&self) -> Option<Commit> {
        let doc = self.doc.as_ref()?.clone();
        let preview = crate::raster::rasterize_rgba(&doc, 1.0).unwrap_or_else(|_| {
            image::RgbaImage::new(doc.rounded_size().0, doc.rounded_size().1)
        });
        Some(Commit {
            document: doc,
            preview,
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_nav_only(&self) -> bool {
        self.nav_only
    }

    pub fn take_gpu_drops(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.gpu_drop)
    }

    pub fn request_apply(&mut self, cx: &mut Context<Self>) {
        if self.standalone {
            self.export_scene(cx);
            return;
        }
        self.host_cmd = Some(HostCmd::Apply);
        cx.notify();
    }

    pub fn request_restore(&mut self, cx: &mut Context<Self>) {
        self.host_cmd = Some(HostCmd::Restore);
        cx.notify();
    }

    pub(crate) fn notify_nav(&mut self, cx: &mut Context<Self>) {
        self.nav_only = true;
        cx.notify();
    }

    pub(crate) fn notify_chrome(&mut self, cx: &mut Context<Self>) {
        self.nav_only = false;
        cx.notify();
    }

    fn selected_leaves(&self) -> Vec<String> {
        self.doc
            .as_ref()
            .map(|d| d.leaf_ids(&self.selected))
            .unwrap_or_default()
    }

    pub(crate) fn text_focused(&self, window: &Window, cx: &App) -> bool {
        self.rename_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    pub fn set_mode(&mut self, mode: ToolMode, window: &Window, cx: &mut Context<Self>) {
        if self.text_focused(window, cx) {
            return;
        }
        self.mode = mode;
        if mode != ToolMode::Pen {
            self.commit_pen(false, cx);
        }
        if mode != ToolMode::Lasso {
            self.lasso.clear();
        }
        if mode != ToolMode::Node {
            self.node_target = None;
            self.node_hit = None;
            self.hover_node = None;
            self.hover_hit = false;
        } else if let Some((x, y)) = self.hover_doc {
            self.refresh_node_hover(x, y);
        }
        self.status = mode.hint().into();
        self.notify_chrome(cx);
    }

    pub fn nudge(&mut self, dx: f32, dy: f32, window: &Window, cx: &mut Context<Self>) {
        if self.text_focused(window, cx) {
            return;
        }
        let step = if self.shift_down { 10.0 } else { 1.0 };
        if self.selected.is_empty() {
            return;
        }
        self.push_undo();
        if let Some(doc) = self.doc.as_mut() {
            doc.translate_ids(&self.selected, dx * step, dy * step);
        }
        self.dirty = true;
        self.rebuild_raster(cx);
        self.notify_chrome(cx);
    }

    /// 选中所有看得见的元素. 组合按一组算, 藏起来的图层和对象不选.
    pub fn select_all(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.text_focused(window, cx) {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let mut ids = Vec::new();
        for layer in &doc.layers {
            if !layer.visible {
                continue;
            }
            let skip = layer.suppressed_ids();
            for obj in &layer.objects {
                if skip.contains(&obj.id) {
                    continue;
                }
                let id = doc.pick_target(&obj.id);
                if !ids.iter().any(|s| s == &id) {
                    ids.push(id);
                }
            }
        }
        self.selected = ids;
        self.node_target = None;
        self.node_hit = None;
        self.outline_menu = None;
        self.reveal_outline_selection();
        self.notify_chrome(cx);
    }

    pub fn delete_selection(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.text_focused(window, cx) {
            return;
        }
        if self.mode == ToolMode::Node {
            if let Some(id) = self.node_target.clone() {
                if let Some((sub, seg, role)) = self.node_hit {
                    if matches!(role, NodeRole::Head | NodeRole::Anchor) {
                        self.push_undo();
                        tools::delete_anchor(self, &id, sub, seg, role);
                        self.node_hit = None;
                        self.dirty = true;
                        self.rebuild_raster(cx);
                        self.notify_chrome(cx);
                        return;
                    }
                }
            }
        }
        if self.selected.is_empty() {
            self.delete_layer(cx);
            return;
        }
        self.push_undo();
        if let Some(doc) = self.doc.as_mut() {
            doc.delete_ids(&self.selected);
        }
        self.selected.clear();
        self.node_target = None;
        self.node_hit = None;
        self.dirty = true;
        self.rebuild_raster(cx);
        self.notify_chrome(cx);
    }

    pub fn extract_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.push_undo();
        let ids = self.selected.clone();
        if self.doc.as_mut().is_some_and(|d| d.extract_to_new_layer(&ids)) {
            self.selected.clear();
            self.dirty = true;
            self.rebuild_raster(cx);
        } else {
            self.undo_stack.pop();
        }
        self.notify_chrome(cx);
    }

    pub fn group_selected(&mut self, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        if self.selected.len() < 2 {
            self.status = "至少选中两个对象才能组合".into();
            self.notify_chrome(cx);
            return;
        }
        self.push_undo();
        let ids = self.selected.clone();
        let result = self
            .doc
            .as_mut()
            .map(|doc| crate::outline::group_selection(doc, &ids));
        match result {
            Some(Ok(gid)) => self.finish_group(gid, "已组合", cx),
            Some(Err(msg)) => {
                self.undo_stack.pop();
                self.status = msg.into();
                self.notify_chrome(cx);
            }
            None => {
                self.undo_stack.pop();
            }
        }
    }

    pub fn ungroup_selected(&mut self, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        self.push_undo();
        let ids = self.selected.clone();
        let result = self
            .doc
            .as_mut()
            .map(|doc| crate::outline::ungroup_selection(doc, &ids));
        match result {
            Some(Ok(dissolved)) => {
                self.selected = dissolved.iter().flat_map(|g| g.children.clone()).collect();
                self.regroup_mem.extend(dissolved);
                self.dirty = true;
                self.status = "已取消组合".into();
                self.notify_chrome(cx);
            }
            Some(Err(msg)) => {
                self.undo_stack.pop();
                self.status = msg.into();
                self.notify_chrome(cx);
            }
            None => {
                self.undo_stack.pop();
            }
        }
    }

    pub fn regroup_selected(&mut self, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        self.push_undo();
        let ids = self.selected.clone();
        let mut memory = std::mem::take(&mut self.regroup_mem);
        let result = self
            .doc
            .as_mut()
            .map(|doc| crate::outline::regroup(doc, &mut memory, &ids));
        self.regroup_mem = memory;
        match result {
            Some(Ok(gid)) => self.finish_group(gid, "已重新组合", cx),
            Some(Err(msg)) => {
                self.undo_stack.pop();
                self.status = msg.into();
                self.notify_chrome(cx);
            }
            None => {
                self.undo_stack.pop();
            }
        }
    }

    fn finish_group(&mut self, gid: String, status: &str, cx: &mut Context<Self>) {
        if let Some(doc) = self.doc.as_ref() {
            if let Some((li, _)) = doc.find_group(&gid) {
                let layer_id = doc.layers[li].id.clone();
                self.expanded.insert(layer_id);
            }
        }
        self.expanded.insert(gid.clone());
        self.selected = vec![gid];
        self.dirty = true;
        self.status = status.to_string().into();
        self.notify_chrome(cx);
    }

    pub(crate) fn toggle_expanded(&mut self, id: String, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        if !self.expanded.insert(id.clone()) {
            self.expanded.remove(&id);
        }
        cx.notify();
    }

    pub(crate) fn toggle_row_visible(&mut self, id: &str, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        let id = id.to_string();
        self.push_undo();
        let changed = self
            .doc
            .as_mut()
            .is_some_and(|doc| crate::outline::toggle_visible(doc, &id));
        if !changed {
            self.undo_stack.pop();
            return;
        }
        self.dirty = true;
        self.rebuild_raster(cx);
        self.notify_chrome(cx);
    }

    /// `(追加, 减去, 切换)`. Alt 减去, Ctrl 切换, Shift 追加.
    pub(crate) fn sel_mods(&self) -> (bool, bool, bool) {
        let sub = self.alt_down;
        let toggle = self.ctrl_down && !sub;
        let add = self.shift_down && !sub && !toggle;
        (add, sub, toggle)
    }

    pub(crate) fn pick_outline(
        &mut self,
        id: String,
        layer_index: usize,
        shift: bool,
        ctrl: bool,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        self.outline_menu = None;
        self.activate_layer(layer_index, cx);
        if shift {
            let rows = self
                .doc
                .as_ref()
                .map(|doc| crate::outline::flatten(doc, &self.expanded))
                .unwrap_or_default();
            let order: Vec<String> = rows
                .into_iter()
                .filter(|row| row.kind != crate::outline::RowKind::Layer)
                .map(|row| row.id)
                .collect();
            let cur = order.iter().position(|s| s == &id);
            let anchor = self
                .outline_anchor
                .as_ref()
                .and_then(|a| order.iter().position(|s| s == a));
            if let (Some(a), Some(b)) = (anchor, cur) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let range = order[lo..=hi].to_vec();
                if ctrl {
                    for rid in range {
                        if !self.selected.iter().any(|s| s == &rid) {
                            self.selected.push(rid);
                        }
                    }
                } else {
                    self.selected = range;
                }
            } else if ctrl {
                self.toggle_outline_id(id.clone());
                self.outline_anchor = Some(id);
            } else {
                self.selected = vec![id.clone()];
                self.outline_anchor = Some(id);
            }
        } else if ctrl {
            self.toggle_outline_id(id.clone());
            self.outline_anchor = Some(id);
        } else {
            self.selected = vec![id.clone()];
            self.outline_anchor = Some(id);
        }
        self.notify_chrome(cx);
    }

    fn toggle_outline_id(&mut self, id: String) {
        if let Some(i) = self.selected.iter().position(|s| s == &id) {
            self.selected.remove(i);
        } else {
            self.selected.push(id);
        }
    }

    /// `wx`/`wy` 是窗口坐标. 先按测到的窗格换成局部坐标, 窗格宽度明显不对时改贴右缘.
    pub(crate) fn open_outline_menu(
        &mut self,
        id: String,
        kind: crate::outline::RowKind,
        layer_index: usize,
        wx: f32,
        wy: f32,
        cx: &mut Context<Self>,
    ) {
        let (x, y, pin_right) = self.menu_point(wx, wy);
        self.commit_rename(cx);
        if kind == crate::outline::RowKind::Layer {
            self.select_layer(layer_index, cx);
        } else if !self.selected.iter().any(|s| s == &id) {
            self.pick_outline(id.clone(), layer_index, false, false, cx);
        } else {
            self.activate_layer(layer_index, cx);
        }
        self.outline_menu = Some(OutlineMenu {
            x,
            y,
            pin_right,
            on_canvas: false,
            target: id,
        });
        cx.notify();
    }

    fn menu_point(&self, wx: f32, wy: f32) -> (f32, f32, bool) {
        let origin = self.picker_layer_bounds.origin;
        let size = self.picker_layer_bounds.size;
        let pw = f32::from(size.width);
        let ph = f32::from(size.height);
        let x = wx - f32::from(origin.x);
        let y = wy - f32::from(origin.y);
        let menu_w = 208.0;
        let menu_h = 300.0;
        let pin_right = pw > 560.0 || x > pw + 1.0 || x < -1.0;
        let x = if pw > 8.0 {
            x.clamp(0.0, (pw - menu_w).max(0.0))
        } else {
            4.0
        };
        let y = if ph > 8.0 {
            y.clamp(0.0, (ph - menu_h).max(0.0))
        } else {
            y.max(0.0)
        };
        (x, y, pin_right)
    }

    /// 右键松开. 命中一行就打开菜单. 返回 true 表示这次点击已经由选择窗格处理.
    pub(crate) fn handle_outline_right_click(
        &mut self,
        x: f32,
        y: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.outline_menu.is_some() && bounds_contains(self.menu_bounds, x, y) {
            return true;
        }
        if let Some(hit) = self
            .outline_row_hits
            .iter()
            .rev()
            .find(|hit| bounds_contains(hit.bounds, x, y))
            .cloned()
        {
            self.open_outline_menu(hit.id, hit.kind, hit.layer_index, x, y, cx);
            return true;
        }
        if self.outline_menu.is_some() && bounds_contains(self.picker_layer_bounds, x, y) {
            self.outline_menu = None;
            cx.notify();
            return true;
        }
        false
    }

    /// 点图层行: 这一层成为当前节点, 清掉里面的对象选区.
    pub(crate) fn select_layer(&mut self, index: usize, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        self.outline_menu = None;
        self.selected.clear();
        self.activate_layer(index, cx);
    }

    pub(crate) fn begin_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.rename_target.is_some() {
            self.commit_rename(cx);
        }
        let Some((id, name)) = self.rename_source() else {
            return;
        };
        self.reveal_outline_id(&id);
        self.rename_release = false;
        self.rename_focus = true;
        self.rename_target = Some(id);
        self.rename_input.update(cx, |input, cx| {
            input.set_text(name, cx);
            input.select_all_text(cx);
        });
        cx.notify();
    }

    pub(crate) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename_target.is_none() {
            return;
        }
        self.rename_target = None;
        self.rename_focus = false;
        self.rename_release = false;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn finish_rename_if_blurred(&mut self, cx: &mut Context<Self>) {
        let commit = self.rename_input.update(cx, |input, _| input.take_blur_commit());
        // 名称框还没接上焦点时, 忽略这一次失焦, 否则第一次按键会马上关掉.
        if commit && !self.rename_focus {
            self.commit_rename(cx);
        }
    }

    /// 空文本不改名. 有文字且和现在不同才写入.
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.rename_target.clone() else {
            return;
        };
        let text = self.rename_input.read(cx).text();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let same = self
                .doc
                .as_ref()
                .is_some_and(|doc| crate::outline::display_name(doc, &id) == trimmed);
            if !same {
                self.push_undo();
                let changed = self
                    .doc
                    .as_mut()
                    .is_some_and(|doc| crate::outline::set_name(doc, &id, trimmed));
                if changed {
                    self.dirty = true;
                }
            }
        }
        self.rename_target = None;
        self.rename_focus = false;
        self.rename_release = true;
        self.notify_chrome(cx);
    }

    /// 展开父级, 并把这一行滚进右侧列表.
    pub(crate) fn reveal_outline_id(&mut self, id: &str) {
        let ancestors = self
            .doc
            .as_ref()
            .map(|doc| ancestor_chain(doc, id))
            .unwrap_or_default();
        if ancestors.is_empty() && self.doc.is_none() {
            return;
        }
        for anc in ancestors {
            self.expanded.insert(anc);
        }
        let pins = self.pin_ids();
        let expanded = self.expanded.clone();
        let rows = self
            .doc
            .as_ref()
            .map(|doc| crate::outline::flatten(doc, &expanded))
            .unwrap_or_default();
        let ix = rows
            .iter()
            .filter(|row| !pins.iter().any(|pin| pin == &row.id))
            .position(|row| row.id == id);
        if let Some(ix) = ix {
            self.outline_scroll
                .scroll_to_item(ix, gpui::ScrollStrategy::Center);
        }
    }

    /// 画布上的选区变了: 右侧跳到对应行. 已经能看见则不动.
    pub(crate) fn reveal_outline_selection(&mut self) {
        let Some(id) = self.selected.first().cloned() else {
            return;
        };
        let ancestors = {
            let Some(doc) = self.doc.as_ref() else {
                return;
            };
            let mut ancestors = Vec::new();
            for sel in &self.selected {
                ancestors.extend(ancestor_chain(doc, sel));
            }
            ancestors
        };
        for anc in ancestors {
            self.expanded.insert(anc);
        }
        let selected = self.selected.clone();
        let pins = self.pin_ids();
        let expanded = self.expanded.clone();
        let rows = self
            .doc
            .as_ref()
            .map(|doc| crate::outline::flatten(doc, &expanded))
            .unwrap_or_default();
        let ix = rows
            .iter()
            .filter(|row| !pins.iter().any(|pin| pin == &row.id))
            .position(|row| selected.iter().any(|id| id == &row.id));
        if let Some(ix) = ix {
            self.outline_scroll
                .scroll_to_item(ix, gpui::ScrollStrategy::Center);
        } else {
            self.reveal_outline_id(&id);
        }
    }

    fn rename_source(&self) -> Option<(String, String)> {
        let doc = self.doc.as_ref()?;
        if self.selected.len() == 1 {
            let id = self.selected[0].clone();
            let name = crate::outline::display_name(doc, &id);
            if !name.is_empty() {
                return Some((id, name));
            }
        }
        if self.selected.is_empty() {
            let layer = doc.layers.get(doc.active)?;
            return Some((layer.id.clone(), layer.name.clone()));
        }
        None
    }

    /// 当前选中节点的父链, 图层在前, 由外到内的组在后. 选中图层本身时为空.
    pub(crate) fn pin_ids(&self) -> Vec<String> {
        let Some(doc) = self.doc.as_ref() else {
            return Vec::new();
        };
        if self.selected.is_empty() {
            return Vec::new();
        }
        let mut common: Option<Vec<String>> = None;
        for id in &self.selected {
            let chain = ancestor_chain(doc, id);
            common = Some(match common {
                None => chain,
                Some(prev) => {
                    let n = prev.iter().zip(chain.iter()).take_while(|(a, b)| a == b).count();
                    prev.into_iter().take(n).collect()
                }
            });
        }
        common.unwrap_or_default()
    }

    pub(crate) fn pump_thumbs(&mut self, rows: &[crate::outline::OutlineRow], range: std::ops::Range<usize>) -> bool {
        let mut budget = 6u32;
        let mut made = false;
        let mut pending = false;
        for i in range.clone() {
            let Some(row) = rows.get(i) else {
                continue;
            };
            let key = {
                let Some(doc) = self.doc.as_ref() else {
                    break;
                };
                thumb_key(doc, row)
            };
            if self.thumbs.get(&row.id).is_some_and(|slot| slot.key == key) {
                continue;
            }
            if budget == 0 {
                pending = true;
                break;
            }
            let leaves = {
                let Some(doc) = self.doc.as_ref() else {
                    break;
                };
                crate::outline::thumb_leaves(doc, row)
            };
            let cost = if leaves.len() > 40 { 6 } else { 1 };
            let img = self
                .doc
                .as_ref()
                .and_then(|doc| crate::raster::rasterize_thumb(doc, &leaves, 36));
            if let Some(old) = self.thumbs.remove(&row.id) {
                if let Some(tex) = old.tex {
                    self.gpu_drop.push(tex);
                }
            }
            match img {
                Some(img) => {
                    self.thumbs.insert(
                        row.id.clone(),
                        ThumbSlot {
                            key,
                            tex: Some(upload_rgba(&img)),
                        },
                    );
                    made = true;
                    budget = budget.saturating_sub(cost);
                }
                None => {
                    self.thumbs.insert(row.id.clone(), ThumbSlot { key, tex: None });
                }
            }
        }
        if !pending {
            pending = range.into_iter().any(|i| {
                let Some(row) = rows.get(i) else {
                    return false;
                };
                let Some(doc) = self.doc.as_ref() else {
                    return false;
                };
                let key = thumb_key(doc, row);
                !self.thumbs.get(&row.id).is_some_and(|slot| slot.key == key)
            });
        }
        made && pending
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        if let Some(cur) = self.capture_history() {
            self.redo_stack.push(cur);
        }
        self.restore_history(prev);
        self.dirty = true;
        self.rebuild_raster(cx);
        self.notify_chrome(cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        if let Some(cur) = self.capture_history() {
            self.undo_stack.push(cur);
        }
        self.restore_history(next);
        self.dirty = true;
        self.rebuild_raster(cx);
        self.notify_chrome(cx);
    }

    pub fn fit_to_view(&mut self, cx: &mut Context<Self>) {
        self.user_zoomed = false;
        self.pan = point(0.0, 0.0);
        self.zoom = 1.0;
        self.raster_wait = false;
        self.notify_chrome(cx);
    }

    pub fn on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.outline_menu.take().is_some() {
            cx.notify();
            return true;
        }
        if self.rename_target.is_some() {
            self.rename_target = None;
            self.rename_focus = false;
            self.rename_release = true;
            cx.notify();
            return true;
        }
        if self.palette_open {
            self.close_palette(cx);
            return true;
        }
        if self.pen.is_some() {
            self.pen = None;
            cx.notify();
            return true;
        }
        if !self.lasso.is_empty() {
            self.lasso.clear();
            cx.notify();
            return true;
        }
        if !self.selected.is_empty() {
            self.selected.clear();
            self.node_target = None;
            self.node_hit = None;
            cx.notify();
            return true;
        }
        self.host_cmd = Some(HostCmd::Cancel);
        cx.notify();
        false
    }

    pub fn import_document_as_layer(&mut self, other: VecDocument, name: &str, cx: &mut Context<Self>) {
        self.push_undo();
        if let Some(doc) = self.doc.as_mut() {
            doc.append_layer_from(&other, name);
            self.dirty = true;
            self.rebuild_raster(cx);
        }
        cx.notify();
    }

    fn capture_history(&self) -> Option<HistorySnap> {
        Some(HistorySnap {
            doc: self.doc.clone()?,
            selected: self.selected.clone(),
            pan: self.pan,
            zoom: self.zoom,
            user_zoomed: self.user_zoomed,
            regroup: self.regroup_mem.clone(),
        })
    }

    fn restore_history(&mut self, snap: HistorySnap) {
        self.selected = snap.selected;
        self.doc = Some(snap.doc);
        self.pan = snap.pan;
        self.zoom = snap.zoom;
        self.user_zoomed = snap.user_zoomed;
        self.regroup_mem = snap.regroup;
        self.rename_target = None;
        self.rename_focus = false;
        self.rename_release = true;
    }

    pub(crate) fn push_undo(&mut self) {
        let Some(snap) = self.capture_history() else {
            return;
        };
        self.undo_stack.push(snap);
        if self.undo_stack.len() > 48 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(crate) fn preview_from_undo(&mut self) {
        if let Some(snap) = self.undo_stack.last() {
            self.doc = Some(snap.doc.clone());
            self.selected = snap.selected.clone();
        }
    }

    pub(crate) fn drop_tex(&mut self) {
        if let Some(t) = self.base_tex.take() {
            self.gpu_drop.push(t.tex);
        }
        if let Some(t) = self.sel_sprite.take() {
            self.gpu_drop.push(t.tex);
        }
    }

    pub(crate) fn rebuild_raster(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.sel_sprite.take() {
            self.gpu_drop.push(t.tex);
        }
        self.sweep_thumbs();
        self.rebuild_base(None);
        let _ = cx;
    }

    fn clear_thumbs(&mut self) {
        for (_, slot) in self.thumbs.drain() {
            if let Some(img) = slot.tex {
                self.gpu_drop.push(img);
            }
        }
    }

    /// 删掉已经不在文档里的缩略图. 内容没变的留着, 避免线宽和撤销把整列都闪掉.
    fn sweep_thumbs(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            self.clear_thumbs();
            return;
        };
        let mut live = std::collections::HashSet::new();
        for layer in &doc.layers {
            live.insert(layer.id.clone());
            for group in &layer.groups {
                live.insert(group.id.clone());
            }
            for obj in &layer.objects {
                live.insert(obj.id.clone());
            }
        }
        let stale: Vec<String> = self
            .thumbs
            .keys()
            .filter(|id| !live.contains(*id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(slot) = self.thumbs.remove(&id) {
                if let Some(img) = slot.tex {
                    self.gpu_drop.push(img);
                }
            }
        }
    }

    /// 节点拖动时把对象从底图抠掉, 形状改由矢量预览画, 松手再光栅一次.
    pub(crate) fn hide_selection_from_base(&mut self) {
        if let Some(old) = self.sel_sprite.take() {
            self.gpu_drop.push(old.tex);
        }
        let ids = self.selected_leaves();
        if ids.is_empty() {
            return;
        }
        let skip: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        self.rebuild_base(Some(&skip));
    }

    /// 拖动前把选区从底图里抠掉, 再单独光栅成可移动的精灵.
    pub(crate) fn preview_selection(&mut self, hide: bool) {
        let ids = self.selected_leaves();
        if hide {
            let skip: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
            self.rebuild_base(Some(&skip));
        }
        self.rebuild_sprite(&ids);
    }

    pub(crate) fn rebuild_sprite(&mut self, ids: &[String]) {
        if let Some(old) = self.sel_sprite.take() {
            self.gpu_drop.push(old.tex);
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let mut px = self.raster_px.max(0.35);
        if let Some(b) = doc.selection_bounds(ids) {
            let side = b.w().max(b.h()) * px;
            if side > 4096.0 {
                px *= 4096.0 / side;
            }
        }
        if let Some((img, x, y, w, h)) = crate::raster::rasterize_ids(doc, ids, px) {
            self.sel_sprite = Some(SelSprite {
                tex: upload_rgba(&img),
                doc_x: x,
                doc_y: y,
                doc_w: w,
                doc_h: h,
                dx: 0.0,
                dy: 0.0,
            });
        }
    }

    fn rebuild_base(&mut self, skip: Option<&std::collections::HashSet<&str>>) {
        // 作废正在排队或已经在后台跑的那次, 避免旧图盖住这次编辑.
        self.raster_gen = self.raster_gen.wrapping_add(1);
        self.raster_wait = false;
        let Some(doc) = self.doc.as_ref() else {
            self.backdrop = 0xe7e7e7;
            return;
        };
        self.backdrop = crate::document::contrast_backdrop(doc);
        let plan = self.raster_plan();
        let tex_w = (plan.w * plan.px).round().max(1.0) as u32;
        let tex_h = (plan.h * plan.px).round().max(1.0) as u32;
        let img = if let Some(skip) = skip.filter(|s| !s.is_empty()) {
            let mut filtered = doc.clone();
            for layer in &mut filtered.layers {
                layer.objects.retain(|o| !skip.contains(o.id.as_str()));
            }
            crate::raster::rasterize_rect(&filtered, plan.x, plan.y, plan.w, plan.h, tex_w, tex_h)
        } else {
            crate::raster::rasterize_rect(doc, plan.x, plan.y, plan.w, plan.h, tex_w, tex_h)
        };
        if let Ok(img) = img {
            if let Some(t) = self.base_tex.take() {
                self.gpu_drop.push(t.tex);
            }
            self.raster_px = plan.px;
            self.base_tex = Some(TexSprite {
                tex: upload_rgba(&img),
                doc_x: plan.x,
                doc_y: plan.y,
                doc_w: plan.w,
                doc_h: plan.h,
            });
        }
    }

    pub(crate) fn view_ready(&self) -> bool {
        f32::from(self.view_bounds.size.width) > 8.0 && f32::from(self.view_bounds.size.height) > 8.0
    }

    pub(crate) fn consider_raster(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.is_none() || self.drag.is_some() || !self.view_ready() {
            return;
        }
        let plan = self.raster_plan();
        if self.raster_current(plan.px) {
            self.raster_wait = false;
            return;
        }
        // 后台那次画完会再进来. 缩放过程中不叠第二次.
        if self.raster_wait || self.raster_running {
            return;
        }
        self.raster_wait = true;
        self.raster_gen = self.raster_gen.wrapping_add(1);
        let gen = self.raster_gen;
        cx.spawn(async move |this, cx| {
            // 滚轮连续转时每次都会把 gen 顶掉, 这里就空等, 停稳了才画.
            cx.background_executor()
                .timer(Duration::from_millis(160))
                .await;
            let job = this
                .update(cx, |this, _cx| {
                    if this.raster_gen != gen {
                        return None;
                    }
                    if this.drag.is_some() || !this.view_ready() {
                        this.raster_wait = false;
                        return None;
                    }
                    let plan = this.raster_plan();
                    if this.raster_current(plan.px) {
                        this.raster_wait = false;
                        return None;
                    }
                    let doc = this.doc.clone()?;
                    this.raster_wait = false;
                    this.raster_running = true;
                    Some((plan, doc))
                })
                .ok()
                .flatten();
            let Some((plan, doc)) = job else {
                return;
            };
            let img = cx
                .background_spawn(async move {
                    let tex_w = (plan.w * plan.px).round().max(1.0) as u32;
                    let tex_h = (plan.h * plan.px).round().max(1.0) as u32;
                    crate::raster::rasterize_rect(&doc, plan.x, plan.y, plan.w, plan.h, tex_w, tex_h)
                        .ok()
                        .map(|img| (img, plan))
                })
                .await;
            this.update(cx, |this, cx| {
                this.raster_running = false;
                if this.raster_gen != gen || this.drag.is_some() {
                    this.notify_nav(cx);
                    return;
                }
                if let Some((img, plan)) = img {
                    if let Some(t) = this.base_tex.take() {
                        this.gpu_drop.push(t.tex);
                    }
                    this.raster_px = plan.px;
                    this.base_tex = Some(TexSprite {
                        tex: upload_rgba(&img),
                        doc_x: plan.x,
                        doc_y: plan.y,
                        doc_w: plan.w,
                        doc_h: plan.h,
                    });
                    this.notify_chrome(cx);
                } else {
                    this.notify_nav(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn invalidate_raster_wait(&mut self) {
        self.raster_gen = self.raster_gen.wrapping_add(1);
        self.raster_wait = false;
    }

    fn commit_pen(&mut self, closed: bool, cx: &mut Context<Self>) {
        let Some(pen) = self.pen.take() else {
            return;
        };
        if pen.nodes.len() < 2 {
            return;
        }
        tools::commit_pen(self, &pen, closed, cx);
    }

    fn export_scene(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else {
            return;
        };
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        if let Err(e) = doc.save_to_dir(&dir) {
            self.status = format!("导出失败: {e}").into();
        } else {
            self.status = "已保存场景".into();
            self.dirty = false;
        }
        cx.notify();
    }

    pub(crate) fn export_png(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return;
        };
        match crate::raster::rasterize_rgba(doc, 4.0) {
            Ok(img) => {
                if let Err(e) = img.save(&path) {
                    self.status = format!("导出 PNG 失败: {e}").into();
                } else {
                    self.status = "已导出 PNG".into();
                }
            }
            Err(e) => self.status = e.into(),
        }
        cx.notify();
    }

    pub(crate) fn export_svg(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SVG", &["svg"])
            .save_file()
        else {
            return;
        };
        let svg = crate::svg::to_svg(doc);
        if let Err(e) = std::fs::write(&path, svg) {
            self.status = format!("导出 SVG 失败: {e}").into();
        } else {
            self.status = "已导出 SVG".into();
        }
        cx.notify();
    }

    fn load_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let result = if is_pdf_path(&path) {
            crate::import::import_pdf_page(&path, 0).map(|r| r.doc)
        } else if is_scene_dir(&path) {
            VecDocument::load_from_dir(&path)
        } else {
            Err("请打开 PDF 或矢量场景目录".into())
        };
        match result {
            Ok(doc) => {
                self.image_path = Some(path);
                self.open_document(doc, cx);
            }
            Err(e) => {
                self.status = e.into();
                cx.notify();
            }
        }
    }

    pub(crate) fn activate_layer(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.doc.as_mut() {
            if index < doc.layers.len() {
                doc.active = index;
            }
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn resolve_layer_drop(&self, from: usize, y: f32) -> (usize, Option<usize>, bool) {
        let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.layer_bounds.get(&i) else {
                continue;
            };
            let top = f32::from(b.origin.y);
            let bottom = top + f32::from(b.size.height);
            if y < top || y > bottom {
                continue;
            }
            if i == from {
                return (from, None, false);
            }
            let mid = (top + bottom) * 0.5;
            let after = y >= mid;
            let to = reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    pub(crate) fn update_layer_reorder(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(DragKind::LayerReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            mut armed,
            ..
        }) = self.drag.take()
        else {
            return;
        };
        if !armed && reorder_slop_exceeded(x - start_x, y - start_y) {
            armed = true;
        }
        let (to, line_at, line_after) = if armed {
            self.resolve_layer_drop(from, y)
        } else {
            (from, None, false)
        };
        self.drag = Some(DragKind::LayerReorder {
            from,
            to,
            line_at,
            line_after,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed,
        });
        self.notify_chrome(cx);
    }

    pub(crate) fn finish_layer_reorder(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.drag, Some(DragKind::LayerReorder { .. })) {
            return;
        }
        if let Some(DragKind::LayerReorder {
            from, to, armed, ..
        }) = self.drag.take()
        {
            self.commit_layer_reorder(from, to, armed, cx);
        }
    }

    pub(crate) fn commit_layer_reorder(
        &mut self,
        from: usize,
        to: usize,
        armed: bool,
        cx: &mut Context<Self>,
    ) {
        if armed && from != to {
            self.push_undo();
            let Some(doc) = self.doc.as_mut() else {
                self.notify_chrome(cx);
                return;
            };
            let aid = doc.layers.get(doc.active).map(|l| l.id.clone());
            apply_display_reorder(&mut doc.layers, from, to);
            if let Some(id) = aid {
                if let Some(i) = doc.layers.iter().position(|l| l.id == id) {
                    doc.active = i;
                }
            }
            self.dirty = true;
            self.rebuild_raster(cx);
        } else if !armed {
            let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
            if n > 0 && from < n {
                self.select_layer(n - 1 - from, cx);
                return;
            }
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn delete_layer(&mut self, cx: &mut Context<Self>) {
        self.push_undo();
        let removed = self.doc.as_mut().is_some_and(|d| d.delete_active_layer());
        if !removed {
            self.undo_stack.pop();
            self.status = "至少保留一个图层".into();
            self.notify_chrome(cx);
            return;
        }
        self.selected.clear();
        self.dirty = true;
        self.rebuild_raster(cx);
        self.status = "已删除图层".into();
        self.notify_chrome(cx);
    }

    pub fn note_canvas_target(&mut self) {
        self.clip_on_canvas = true;
    }

    pub fn note_side_target(&mut self) {
        self.clip_on_canvas = false;
    }

    pub fn copy_layer(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.text_focused(window, cx) {
            return;
        }
        if self.clip_on_canvas {
            self.copy_selection(cx);
            return;
        }
        let Some(src) = self
            .doc
            .as_ref()
            .and_then(|d| d.layers.get(d.active).cloned())
        else {
            return;
        };
        self.layer_clip = Some(src);
        self.status = "已复制图层".into();
        self.notify_chrome(cx);
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let leaves = doc.leaf_ids(&self.selected);
        let objs: Vec<crate::document::Obj> = leaves
            .iter()
            .filter_map(|id| doc.object(id).cloned())
            .collect();
        if objs.is_empty() {
            self.status = "没有选中的对象".into();
            self.notify_chrome(cx);
            return;
        }
        self.obj_clip = Some(objs);
        self.status = "已复制选中对象".into();
        self.notify_chrome(cx);
    }

    fn paste_selection(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(src) = self.obj_clip.clone() else {
            self.status = "没有可粘贴的对象".into();
            self.notify_chrome(cx);
            return;
        };
        let base = match self.canvas_pointer(window) {
            Some((px, py)) => match clip_center(&src) {
                Some((cx, cy)) => (px - cx, py - cy),
                None => (8.0, 8.0),
            },
            None => (8.0, 8.0),
        };
        let (dx, dy) = self
            .doc
            .as_ref()
            .map(|doc| clear_shift(doc, &src, base.0, base.1))
            .unwrap_or(base);
        self.push_undo();
        let Some(doc) = self.doc.as_mut() else {
            self.undo_stack.pop();
            return;
        };
        let Some(layer) = doc.active_layer_mut() else {
            self.undo_stack.pop();
            return;
        };
        let mut ids = Vec::with_capacity(src.len());
        for mut obj in src {
            crate::geom::translate_obj(&mut obj, dx, dy);
            obj.id = crate::ids::new_id();
            ids.push(obj.id.clone());
            layer.objects.push(Arc::new(obj));
        }
        self.selected = ids;
        self.dirty = true;
        self.rebuild_raster(cx);
        self.status = "已粘贴选中对象".into();
        self.notify_chrome(cx);
    }

    pub fn paste_layer(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.text_focused(window, cx) {
            return;
        }
        if self.clip_on_canvas {
            self.paste_selection(window, cx);
            return;
        }
        let Some(src) = self.layer_clip.clone() else {
            return;
        };
        self.push_undo();
        let Some(doc) = self.doc.as_mut() else {
            self.undo_stack.pop();
            return;
        };
        let mut copy = src;
        copy.id = crate::ids::new_id();
        copy.name = format!("{} 副本", copy.name);
        copy.reassign_ids();
        let at = (doc.active + 1).min(doc.layers.len());
        doc.layers.insert(at, copy);
        doc.active = at;
        self.dirty = true;
        self.rebuild_raster(cx);
        self.status = "已粘贴图层".into();
        self.notify_chrome(cx);
    }

    fn watch_layer_clip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layer_clip_watch.is_some() {
            return;
        }
        let handle = self.focus_handle.clone();
        self.layer_clip_watch = Some(cx.on_focus_out(&handle, window, |this, _, _, _| {
            this.layer_clip = None;
            this.obj_clip = None;
        }));
    }

    fn session_root(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .track_focus(&self.focus_handle)
            .key_context("VectorEdit")
            .on_action(cx.listener(|this, _: &OpenFile, _, cx| {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .pick_file()
                {
                    this.load_path(p, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExportImage, _, cx| this.export_png(cx)))
            .on_action(cx.listener(|this, _: &ExportSvg, _, cx| this.export_svg(cx)))
            .on_action(cx.listener(|this, _: &FitView, _, cx| this.fit_to_view(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelection, window, cx| this.delete_selection(window, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, window, cx| this.select_all(window, cx)))
            .on_action(cx.listener(|this, _: &CopyLayer, window, cx| this.copy_layer(window, cx)))
            .on_action(cx.listener(|this, _: &PasteLayer, window, cx| this.paste_layer(window, cx)))
            .on_action(cx.listener(|this, _: &NewLayerFromSel, _, cx| this.extract_selection(cx)))
            .on_action(cx.listener(|this, _: &GroupSelection, window, cx| {
                if this.text_focused(window, cx) {
                    return;
                }
                this.group_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &UngroupSelection, window, cx| {
                if this.text_focused(window, cx) {
                    return;
                }
                this.ungroup_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &RegroupSelection, window, cx| {
                if this.text_focused(window, cx) {
                    return;
                }
                this.regroup_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &RenameNode, window, cx| {
                this.begin_rename(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.on_escape(cx);
            }))
            .on_action(cx.listener(|this, _: &ApplyEdit, _, cx| this.request_apply(cx)))
            .on_action(cx.listener(|this, _: &RestoreOriginal, _, cx| this.request_restore(cx)))
            .on_action(cx.listener(|this, _: &ToolSelect, window, cx| this.set_mode(ToolMode::Select, window, cx)))
            .on_action(cx.listener(|this, _: &ToolNode, window, cx| this.set_mode(ToolMode::Node, window, cx)))
            .on_action(cx.listener(|this, _: &ToolCanvas, window, cx| this.set_mode(ToolMode::Canvas, window, cx)))
            .on_action(cx.listener(|this, _: &ToolHand, window, cx| this.set_mode(ToolMode::Hand, window, cx)))
            .on_action(cx.listener(|this, _: &ToolPen, window, cx| this.set_mode(ToolMode::Pen, window, cx)))
            .on_action(cx.listener(|this, _: &ToolPencil, window, cx| this.set_mode(ToolMode::Pencil, window, cx)))
            .on_action(cx.listener(|this, _: &ToolEraser, window, cx| this.set_mode(ToolMode::Eraser, window, cx)))
            .on_action(cx.listener(|this, _: &ToolLasso, window, cx| this.set_mode(ToolMode::Lasso, window, cx)))
            .on_action(cx.listener(|this, _: &NudgeLeft, window, cx| this.nudge(-1.0, 0.0, window, cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, window, cx| this.nudge(1.0, 0.0, window, cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, window, cx| this.nudge(0.0, -1.0, window, cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, window, cx| this.nudge(0.0, 1.0, window, cx)))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if this.text_focused(window, cx) {
                    return;
                }
                if ev.keystroke.key == "space" && !ev.is_held {
                    this.space_down = true;
                    this.notify_nav(cx);
                } else if ev.keystroke.key == "enter" && this.pen.is_some() {
                    this.commit_pen(false, cx);
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|this, ev: &gpui::KeyUpEvent, _, cx| {
                if ev.keystroke.key == "space" {
                    this.space_down = false;
                    this.notify_nav(cx);
                }
            }))
            .on_modifiers_changed(cx.listener(|this, ev: &gpui::ModifiersChangedEvent, _, cx| {
                this.note_pointer_modifiers(
                    ev.modifiers.shift,
                    ev.modifiers.alt,
                    apply_bg::is_primary_mod(&ev.modifiers),
                    cx,
                );
            }))
    }
}

impl Focusable for VectorEditApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for VectorEditApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.rename_release {
            self.rename_release = false;
            self.focus_handle.focus(window);
        }
        self.watch_layer_clip(window, cx);
        let sf = window.scale_factor();
        if sf.is_finite() && sf > 0.05 {
            self.device_scale = sf;
        }
        self.consider_raster(window, cx);
        for img in self.take_gpu_drops() {
            let _ = window.drop_image(img);
        }
        if !self.standalone {
            return self
                .session_root(cx)
                .id("vector_edit_embed")
                .size_full()
                .overflow_hidden()
                .child(self.image_view(cx))
                .into_any_element();
        }
        let title: SharedString = "矢量修图".into();
        self.session_root(cx)
            .id("vector_edit_root")
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(p) = paths.paths().first() {
                    this.load_path(p.clone(), cx);
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
                    .child(div().text_lg().child(title))
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
            .into_any_element()
    }
}

fn clip_center(objs: &[crate::document::Obj]) -> Option<(f32, f32)> {
    let mut acc: Option<crate::geom::Rect> = None;
    for obj in objs {
        let Some(b) = obj.bounds() else {
            continue;
        };
        acc = Some(match acc {
            None => b,
            Some(a) => a.union(b),
        });
    }
    acc.map(|b| (b.cx(), b.cy()))
}

/// 目标位置上如果已经有一份同样的粘贴, 再往右下错 8, 直到空位. 撤销掉那一份后空位回来.
fn clear_shift(doc: &crate::document::VecDocument, objs: &[crate::document::Obj], mut dx: f32, mut dy: f32) -> (f32, f32) {
    for _ in 0..64 {
        if !paste_occupied(doc, objs, dx, dy) {
            return (dx, dy);
        }
        dx += 8.0;
        dy += 8.0;
    }
    (dx, dy)
}

fn paste_occupied(doc: &crate::document::VecDocument, objs: &[crate::document::Obj], dx: f32, dy: f32) -> bool {
    if objs.is_empty() {
        return false;
    }
    objs.iter().all(|src| {
        let mut probe = src.clone();
        crate::geom::translate_obj(&mut probe, dx, dy);
        doc.layers.iter().any(|layer| {
            layer.objects.iter().any(|existing| {
                probe.id = existing.id.clone();
                probe == **existing
            })
        })
    })
}

fn ancestor_chain(doc: &crate::document::VecDocument, id: &str) -> Vec<String> {
    if doc.layers.iter().any(|layer| layer.id == id) {
        return Vec::new();
    }
    let Some(li) = doc
        .find_loc(id)
        .map(|(i, _)| i)
        .or_else(|| doc.find_group(id).map(|(i, _)| i))
    else {
        return Vec::new();
    };
    let Some(layer) = doc.layers.get(li) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    let mut cur = id.to_string();
    while let Some((pli, parent)) = doc.parent_of(&cur) {
        if pli != li {
            break;
        }
        groups.push(parent.clone());
        cur = parent;
    }
    groups.reverse();
    let mut chain = vec![layer.id.clone()];
    chain.extend(groups);
    chain
}

pub fn bind_embedded_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("v", ToolSelect, Some("VectorEdit")),
        KeyBinding::new("a", ToolNode, Some("VectorEdit")),
        KeyBinding::new("k", ToolCanvas, Some("VectorEdit")),
        KeyBinding::new("h", ToolHand, Some("VectorEdit")),
        KeyBinding::new("p", ToolPen, Some("VectorEdit")),
        KeyBinding::new("b", ToolPencil, Some("VectorEdit")),
        KeyBinding::new("e", ToolEraser, Some("VectorEdit")),
        KeyBinding::new("l", ToolLasso, Some("VectorEdit")),
        KeyBinding::new("f", FitView, Some("VectorEdit")),
        KeyBinding::new("j", NewLayerFromSel, Some("VectorEdit")),
        KeyBinding::new("escape", Cancel, Some("VectorEdit")),
        KeyBinding::new("delete", DeleteSelection, Some("VectorEdit")),
        KeyBinding::new("backspace", DeleteSelection, Some("VectorEdit")),
        KeyBinding::new("left", NudgeLeft, Some("VectorEdit")),
        KeyBinding::new("right", NudgeRight, Some("VectorEdit")),
        KeyBinding::new("up", NudgeUp, Some("VectorEdit")),
        KeyBinding::new("down", NudgeDown, Some("VectorEdit")),
    ]);
    cx.bind_keys(apply_bg::bind_primary("z", Undo, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary("y", Redo, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary("shift-z", Redo, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary("o", OpenFile, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary("e", ExportImage, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary(
        "shift-e",
        ExportSvg,
        Some("VectorEdit"),
    ));
    cx.bind_keys(apply_bg::bind_primary("g", GroupSelection, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary(
        "shift-g",
        UngroupSelection,
        Some("VectorEdit"),
    ));
    cx.bind_keys(apply_bg::bind_primary(
        "alt-g",
        RegroupSelection,
        Some("VectorEdit"),
    ));
    cx.bind_keys([KeyBinding::new("f2", RenameNode, Some("VectorEdit"))]);
    cx.bind_keys(apply_bg::bind_primary("a", SelectAll, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary("c", CopyLayer, Some("VectorEdit")));
    cx.bind_keys(apply_bg::bind_primary("v", PasteLayer, Some("VectorEdit")));
}

pub fn bind_keys(cx: &mut App) {
    bind_embedded_keys(cx);
    cx.bind_keys(apply_bg::bind_primary("s", ApplyEdit, Some("VectorEdit")));
}

pub fn run_gui(initial: Option<PathBuf>) {
    Application::new().run(move |cx: &mut App| {
        apply_bg::text_input::bind_keys(cx);
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("矢量修图".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                let app = cx.new(|cx| {
                    let mut app = VectorEditApp::new(cx, true);
                    if let Some(p) = initial.clone() {
                        app.load_path(p, cx);
                    }
                    app
                });
                cx.activate(true);
                app
            },
        )
        .unwrap();
    });
}

pub(crate) fn upload_rgba(img: &image::RgbaImage) -> Arc<RenderImage> {
    let w = img.width();
    let h = img.height();
    let raw = img.as_raw();
    let mut buf = vec![0u8; raw.len()];
    for (s, d) in raw.chunks_exact(4).zip(buf.chunks_exact_mut(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
    let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, buf).unwrap());
    Arc::new(RenderImage::new(smallvec::smallvec![frame]))
}
