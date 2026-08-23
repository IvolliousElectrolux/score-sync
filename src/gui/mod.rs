//! GPUI 主界面: 曲谱同步 (分块 / 蒙版 / 加底色).
//!
//! 按职责拆开, `ScoreSyncApp` 仍是唯一状态机:
//! - `types` 常量/枚举/`DragKind`
//! - `canvas` 坐标变换与谱面交互
//! - `crop` 识别与加减块
//! - `io` 打开/保存/导出
//! - `tabs` / `lists` 页签与侧栏列表
//! - `chrome` 工具栏、工作区、对话框
//! - `sync` 页图窗口与蒙版/视频同步
//! - `history` 分块撤重
//! - `host` 窗口外拖拽与分隔条

mod canvas;
mod chrome;
mod crop;
mod history;
mod host;
mod io;
mod pdf_import;
mod lists;
mod sync;
mod tabs;
mod types;

pub(crate) use types::*;

use gpui::actions;
use image::{Frame, ImageBuffer, RgbaImage};
use smallvec::smallvec;

pub(crate) use std::collections::{HashMap, HashSet};
pub(crate) use std::path::PathBuf;
pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
pub(crate) use std::sync::Arc;
pub(crate) use std::time::Duration;

actions!(
    score_sync,
    [
        OpenFile,
        OpenProject,
        NewProject,
        SaveProject,
        SaveProjectAs,
        DetectPage,
        DetectAll,
        ToggleAddBlock,
        ToggleSplitBlock,
        MergeSelected,
        PairUngrouped,
        DeleteSelected,
        ExportGroups,
        ResetGroups,
        FitView,
        ShowHelp,
        ShareIntoGroup,
        UngroupActive,
        ConfirmParamEdit,
        CancelParamEdit,
        Undo,
        Redo,
        SelectAllPageRegions,
    ]
);


pub(crate) use gpui::prelude::*;
pub(crate) use gpui::{
    canvas, div, point, px, quad, rgb, size, App, Application, Bounds, Context, CursorStyle,
    DispatchPhase, Entity, ExternalPaths, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    RenderImage, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
pub(crate) use crate::config;
pub(crate) use crate::model::{
    is_image_path, is_open_path, is_pdf_path, parse_color_hex, DocState,
};
pub(crate) use crate::pdf;
pub(crate) use crate::project::{self, is_project_path};
pub(crate) use apply_bg::text_input::TextInput;
pub(crate) use apply_bg::gui::ApplyBgApp;
pub(crate) use apply_bg::is_primary_mod;
pub(crate) use mask_tool::gui::MaskToolApp;
pub(crate) use mask_tool::mask::MaskRect;
pub(crate) use score_video::gui::ScoreVideoApp;
pub(crate) use score_video::model::MaterialItem;

pub(crate) struct ScoreSyncApp {
    focus_handle: FocusHandle,
    doc: DocState,
    render_image: Option<Arc<RenderImage>>,
    img_w: u32,
    img_h: u32,
    zoom: f32,
    pan: Point<f32>,
    user_zoomed: bool,
    view_bounds: Bounds<Pixels>,
    drag: Option<DragKind>,
    status: SharedString,
    hint: SharedString,
    region_panel_open: bool,
    side_width: f32,
    /// 右侧工具: 分块 | 蒙版 | 工程
    side_tool: SideTool,
    /// 画布工具: 普通 / 添加新块 / 分割块
    canvas_tool: CanvasTool,
    mask_tool: Entity<MaskToolApp>,
    apply_bg: Entity<ApplyBgApp>,
    score_video: Entity<ScoreVideoApp>,
    /// 当前蒙版编辑目标: group_id (拼合图)
    mask_target: Option<String>,
    /// 当前蒙版预览图相对拼合图的横向/纵向偏移 (叠加工程底色补边时非零)
    mask_preview_hoff: i64,
    mask_preview_voff: i64,
    dialog: Option<DialogKind>,
    /// 标签右键菜单
    tab_menu: Option<TabContextMenu>,
    /// 页签悬停 1s 后的完整文件名提示
    tab_hover_idx: Option<usize>,
    tab_hover_gen: u64,
    tab_tooltip: Option<TabTooltip>,
    /// 原子块 y0-y1 行内编辑
    edit_y_input: Entity<TextInput>,
    /// 正在编辑 y 的 region id
    region_y_edit: Option<String>,
    /// 边距 / 墨迹阈值 点按编辑
    param_input: Entity<TextInput>,
    param_edit: Option<ParamEdit>,
    /// PDF 导入弹窗
    pdf_import: Option<pdf_import::PdfImportState>,
    pdf_w_input: Entity<TextInput>,
    pdf_h_input: Entity<TextInput>,
    pdf_scale_input: Entity<TextInput>,
    /// 画布悬停光标 (边缘/分割)
    hover_cursor: CursorStyle,
    region_scroll: ScrollHandle,
    group_scroll: ScrollHandle,
    member_scroll: ScrollHandle,
    mask_group_scroll: ScrollHandle,
    help_scroll: ScrollHandle,
    update_scroll: ScrollHandle,
    tab_scroll: ScrollHandle,
    /// 标签页条目屏幕 bounds (供拖拽虚影锚点)
    tab_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 组合内成员条目屏幕 bounds
    member_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 输出组合条目屏幕 bounds
    group_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 当前工程文件路径 (Ctrl+S 覆盖保存)
    project_path: Option<PathBuf>,
    /// 后台保存进行中, 避免重复触发
    saving: bool,
    /// 后台打开工程进行中
    opening: bool,
    /// 视频素材池后台重算代次: 每次触发 `sync_video_pool` 自增, 供异步回调
    /// 判断自己是否已被更晚的一轮请求取代 (取代则丢弃结果, 避免旧结果
    /// 覆盖新状态; 例如快速连续应用/取消底色时).
    video_sync_gen: u64,
    /// 分块撤重: key = page_id, 各标签页互不影响.
    crop_histories: HashMap<String, CropHistory>,
    /// 删页/复制页等文档结构撤重 (与单页 regions 栈分开).
    page_struct_history: CropHistory,
    /// 有未保存改动
    dirty: bool,
    /// 切页异步加载代数, 防止连切时旧结果覆盖
    page_load_gen: u64,
    /// 全量灌入识别 sidecar 的代数, 防止重叠 hydrate 互相覆盖
    hydrate_gen: u64,
    /// PDF 导入代数: 新建/打开工程时自增, 后台渲染与 UI 登记都丢弃旧代
    pdf_load_gen: Arc<AtomicU64>,
    /// 仍有 PDF 页在后台渲染或登记 (保存会不完整)
    pdf_importing: bool,
    /// 视频池组合脏标记 (分块/蒙版/底色变更后需重算缓存)
    video_pool_dirty: HashSet<String>,
    /// 全部视频池视为脏 (底色整体变更等)
    video_pool_all_dirty: bool,
    /// 用户确认退出后允许关窗
    allow_close: bool,
    /// 保存中转圈动画相位 (0..1)
    save_spin_phase: f32,
    /// 按下发生在标签栏「+」上; 仅空点松开时才打开文件.
    tab_add_press: bool,
    /// 启动检查到的更新; 等当前对话框关掉后再弹出.
    pending_update: Option<crate::update::UpdateInfo>,
    /// 页图未就绪, 等加载后再识别.
    pending_redetect: bool,
}

impl ScoreSyncApp {
    fn new(cx: &mut Context<Self>, initial: Vec<PathBuf>) -> Self {
        let cfg = config::load();
        let mask_prefs = cfg.mask_prefs.clone();
        let edit_y_input = cx.new(|cx| TextInput::new(cx, "", "例如 94-371"));
        let param_input = cx.new(|cx| TextInput::new(cx, "", "数字"));
        let pdf_w_input = cx.new(|cx| TextInput::new(cx, "", "宽").with_compact(true));
        let pdf_h_input = cx.new(|cx| TextInput::new(cx, "", "高").with_compact(true));
        let pdf_scale_input = cx.new(|cx| TextInput::new(cx, "", "倍率").with_compact(true));
        let mask_tool = cx.new(|cx| {
            let mut m = MaskToolApp::new(cx, None);
            m.apply_color_prefs(mask_prefs.clone());
            m
        });
        cx.observe(&mask_tool, |_, _, cx| cx.notify()).detach();
        let apply_bg = cx.new(ApplyBgApp::new);
        cx.observe(&apply_bg, |_, _, cx| cx.notify()).detach();
        let score_video = cx.new(ScoreVideoApp::new);
        cx.observe(&score_video, |this, video, cx| {
            let snap = video.read(cx).timeline_snapshot();
            let saved = &this.doc.video_state;
            if snap.video_clips != saved.video_clips
                || snap.fades != saved.fades
                || snap.audio_clips != saved.audio_clips
            {
                this.dirty = true;
            }
            cx.notify();
        })
        .detach();
        let mut app = Self {
            focus_handle: cx.focus_handle(),
            doc: {
                let mut d = DocState::new();
                d.mask_prefs = mask_prefs;
                d
            },
            render_image: None,
            img_w: 0,
            img_h: 0,
            zoom: 1.0,
            pan: point(0.0, 0.0),
            user_zoomed: false,
            view_bounds: Bounds::default(),
            drag: None,
            status: "就绪".into(),
            hint: format!(
                "拖入/打开图片、PDF 或工程. {}S 保存工程. 标签右键可复制本页.",
                apply_bg::primary_mod()
            )
            .into(),
            region_panel_open: false,
            side_width: SIDE_PANEL_W,
            side_tool: SideTool::Crop,
            canvas_tool: CanvasTool::Normal,
            mask_tool,
            apply_bg,
            score_video,
            mask_target: None,
            mask_preview_hoff: 0,
            mask_preview_voff: 0,
            dialog: None,
            tab_menu: None,
            tab_hover_idx: None,
            tab_hover_gen: 0,
            tab_tooltip: None,
            edit_y_input,
            region_y_edit: None,
            param_input,
            param_edit: None,
            pdf_import: None,
            pdf_w_input,
            pdf_h_input,
            pdf_scale_input,
            hover_cursor: CursorStyle::Arrow,
            region_scroll: ScrollHandle::new(),
            group_scroll: ScrollHandle::new(),
            member_scroll: ScrollHandle::new(),
            mask_group_scroll: ScrollHandle::new(),
            help_scroll: ScrollHandle::new(),
            update_scroll: ScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            tab_bounds: HashMap::new(),
            member_bounds: HashMap::new(),
            group_bounds: HashMap::new(),
            project_path: None,
            saving: false,
            opening: false,
            video_sync_gen: 0,
            crop_histories: HashMap::new(),
            page_struct_history: CropHistory::default(),
            dirty: false,
            page_load_gen: 0,
            hydrate_gen: 0,
            pdf_load_gen: Arc::new(AtomicU64::new(0)),
            pdf_importing: false,
            video_pool_dirty: HashSet::new(),
            video_pool_all_dirty: true,
            allow_close: false,
            save_spin_phase: 0.0,
            tab_add_press: false,
            pending_update: None,
            pending_redetect: false,
        };
        if !initial.is_empty() {
            let projects: Vec<PathBuf> = initial
                .iter()
                .filter(|p| is_project_path(p))
                .cloned()
                .collect();
            let others: Vec<PathBuf> = initial
                .into_iter()
                .filter(|p| !is_project_path(p))
                .collect();
            if let Some(proj) = projects.last() {
                app.open_project_path(proj.clone(), cx);
            }
            if !others.is_empty() {
                app.load_paths(others, cx);
            }
        } else {
            // 命令行没带任何文件时, 尝试自动恢复上次打开的工程 (与 apply_bg
            // 记忆底色路径同一套逻辑, 存于 %APPDATA%\score_sync).
            let last = config::load().last_project;
            if !last.is_empty() {
                let path = PathBuf::from(last);
                if is_project_path(&path) && path.is_file() {
                    app.open_project_path(path, cx);
                }
            }
        }
        app.start_update_check(cx);
        app
    }
}

impl Focusable for ScoreSyncApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ScoreSyncApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_core: SharedString = if let Some(page) = self.doc.current_page() {
            format!(
                "曲谱同步 — [{}/{}] {}",
                self.doc.current_page_index + 1,
                self.doc.pages.len(),
                page.title()
            )
            .into()
        } else {
            "曲谱同步 / Score Sync".into()
        };
        let saving = self.saving;
        let dirty = self.dirty;
        let spin_phase = self.save_spin_phase;

        // A4-ish: side panel fixed; left takes rest (ratio used as min width hint)
        let _ = A4_RATIO;
        let mask_mode = self.side_tool == SideTool::Mask;
        let video_mode = self.side_tool == SideTool::Video;
        let focus = if mask_mode {
            self.mask_tool.read(cx).focus_handle_ref().clone()
        } else if video_mode {
            self.score_video.read(cx).focus_handle_ref().clone()
        } else {
            self.focus_handle.clone()
        };
        let key_ctx = if mask_mode {
            "MaskTool"
        } else if video_mode {
            "ScoreVideo"
        } else {
            "ScoreSync"
        };

        div()
            .id("root")
            .key_context(key_ctx)
            .track_focus(&focus)
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // 点输入框外自动保存边距/墨迹阈值/原子块 y
                    if this.param_edit.is_some() {
                        this.apply_param_edit(window, cx);
                    }
                    if this.region_y_edit.is_some() {
                        this.apply_edit_y(window, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let x = f32::from(ev.position.x);
                let y = f32::from(ev.position.y);
                // Help 打开时仍允许拖 Help 滚动条; 其它拖拽一律忽略
                if this.has_modal_overlay(cx) {
                    if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                        this.apply_scrollbar_drag(x, y, cx);
                    }
                    return;
                }
                // 视频栏: 素材池 → 轨道跨面板拖放, 由宿主根节点转发鼠标坐标
                // (轨道内部的裁剪/拖选等交互已在 score_video 自身处理).
                if this.drag.is_none() && this.side_tool == SideTool::Video {
                    this.score_video
                        .update(cx, |v, cx| v.root_mouse_move(x, y, cx));
                }
                if this.drag.is_none() && this.side_tool == SideTool::Mask {
                    this.mask_tool.update(cx, |m, cx| {
                        if m.needs_root_move_forward() {
                            m.root_mouse_move(x, y, cx);
                        }
                    });
                }
                this.apply_host_drag_at(x, y, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    if this.has_modal_overlay(cx) {
                        if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                            this.drag = None;
                            cx.notify();
                        }
                        return;
                    }
                    if this.side_tool == SideTool::Video {
                        let x = f32::from(ev.position.x);
                        let y = f32::from(ev.position.y);
                        this.score_video
                            .update(cx, |v, cx| v.root_mouse_up(x, y, cx));
                    }
                    if this.side_tool == SideTool::Mask {
                        let x = f32::from(ev.position.x);
                        let y = f32::from(ev.position.y);
                        this.mask_tool
                            .update(cx, |m, cx| m.root_mouse_up(x, y, cx));
                    }
                    this.finish_host_drag_at(f32::from(ev.position.x), f32::from(ev.position.y), cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    this.handle_outside_window_mouse_up(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                }),
            )
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| this.open_file(window, cx)))
            .on_action(cx.listener(|this, _: &OpenProject, window, cx| {
                this.open_project(window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewProject, window, cx| {
                this.request_new_project(window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                this.save_project(window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveProjectAs, window, cx| {
                this.save_project_as(window, cx)
            }))
            .on_action(cx.listener(|this, _: &DetectPage, _, cx| this.run_detect(cx)))
            .on_action(cx.listener(|this, _: &DetectAll, _, cx| this.run_detect_all(cx)))
            .on_action(cx.listener(|this, _: &ToggleAddBlock, _, cx| this.toggle_add_block(cx)))
            .on_action(cx.listener(|this, _: &ToggleSplitBlock, _, cx| {
                this.toggle_split_block(cx)
            }))
            .on_action(cx.listener(|this, _: &MergeSelected, _, cx| this.merge_selected(cx)))
            .on_action(cx.listener(|this, _: &PairUngrouped, _, cx| this.pair_ungrouped(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| {
                this.delete_selected(cx)
            }))
            .on_action(cx.listener(|this, _: &ExportGroups, window, cx| {
                this.export_groups_ui(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetGroups, _, cx| this.reset_groups(cx)))
            .on_action(cx.listener(|this, _: &FitView, _, cx| this.fit_to_view(cx)))
            .on_action(cx.listener(|this, _: &ShowHelp, _, cx| this.show_help(cx)))
            .on_action(cx.listener(|this, _: &ShareIntoGroup, _, cx| {
                this.share_into_group(cx)
            }))
            .on_action(cx.listener(|this, _: &UngroupActive, _, cx| {
                this.ungroup_active(cx)
            }))
            .on_action(cx.listener(|this, _: &ConfirmParamEdit, window, cx| {
                if this.pdf_import.is_some() {
                    this.on_pdf_import_enter(window, cx);
                } else if this.param_edit.is_some() {
                    this.apply_param_edit(window, cx);
                } else if this.region_y_edit.is_some() {
                    this.apply_edit_y(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CancelParamEdit, window, cx| {
                if this.pdf_import.is_some() {
                    this.close_import_dialog(cx);
                } else if this.param_edit.is_some() {
                    this.cancel_param_edit(window, cx);
                } else if this.region_y_edit.is_some() {
                    this.cancel_edit_y(window, cx);
                } else {
                    this.dismiss_blocking_overlays(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::OpenFile, window, cx| {
                this.mask_tool.update(cx, |m, cx| m.open_file(window, cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::ExportImage, window, cx| {
                this.mask_tool.update(cx, |m, cx| m.export_image(window, cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::FitView, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.fit_to_view(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::DeleteSelected, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.delete_selected(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::ClearMasks, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.clear_masks(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::SelectAll, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.select_all_masks(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::ToggleDrawMode, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.toggle_draw_mode(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::TogglePanMode, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.toggle_pan_mode(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::ToggleBrushMode, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.toggle_brush_mode(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::TogglePolyMode, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.toggle_poly_mode(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::CancelPolyDraft, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.cancel_poly_draft(cx));
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.undo_action(cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.redo_action(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllPageRegions, _, cx| {
                if this.side_tool != SideTool::Crop {
                    return;
                }
                this.doc.select_all_current_page_regions();
                this.scroll_group_list_to_active();
                this.after_doc_change(cx);
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::Undo, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.undo(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::Redo, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.redo(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::Undo, _, cx| {
                this.score_video.update(cx, |v, cx| v.undo(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::Redo, _, cx| {
                this.score_video.update(cx, |v, cx| v.redo(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::PlayPause, _, cx| {
                this.score_video.update(cx, |v, cx| v.play_pause(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::SeekBack, _, cx| {
                this.score_video.update(cx, |v, cx| v.seek_by(-1.0, cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::SeekForward, _, cx| {
                this.score_video.update(cx, |v, cx| v.seek_by(1.0, cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::SeekBackBig, _, cx| {
                this.score_video.update(cx, |v, cx| v.seek_by(-5.0, cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::SeekForwardBig, _, cx| {
                this.score_video.update(cx, |v, cx| v.seek_by(5.0, cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::InsertNext, _, cx| {
                this.score_video.update(cx, |v, cx| v.insert_next(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::MarkFadeIn, _, cx| {
                this.score_video.update(cx, |v, cx| v.mark_fade_in(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::MarkFadeOut, _, cx| {
                this.score_video.update(cx, |v, cx| v.mark_fade_out(cx));
            }))
            .on_action(cx.listener(|this, _: &score_video::gui::DeleteSelected, _, cx| {
                this.score_video.update(cx, |v, cx| v.delete_selected(cx));
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths
                    .paths()
                    .iter()
                    .filter(|p| is_open_path(p) || is_project_path(p))
                    .cloned()
                    .collect();
                if list.is_empty() {
                    return;
                }
                if this.pdf_import.is_some() {
                    this.import_dialog_add_paths(list, cx);
                } else {
                    this.load_paths(list, cx);
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
                    .flex()
                    .flex_col()
                    .border_b_1()
                    .border_color(rgb(0xcbd5e1))
                    .bg(rgb(0xf1f5f9))
                    .child(
                        div()
                            .px_3()
                            .pt_2()
                            .pb_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x0f172a))
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(title_core),
                            )
                            .when(saving, |d| {
                                d.child(
                                    div()
                                        .w(px(18.))
                                        .h(px(18.))
                                        .flex_shrink_0()
                                        .child(
                                            canvas(
                                                |_, _, _| {},
                                                move |bounds, _, window, _| {
                                                    paint_save_spinner(
                                                        window,
                                                        bounds,
                                                        spin_phase,
                                                    );
                                                },
                                            )
                                            .size_full(),
                                        ),
                                )
                            })
                            .when(!saving && dirty, |d| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xdc2626))
                                        .child("*"),
                                )
                            })
                            .child(div().flex_1().min_w(px(8.)))
                            .child(
                                div()
                                    .id("help_header")
                                    .flex_shrink_0()
                                    .mr_2()
                                    .w(px(22.))
                                    .h(px(22.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(rgb(0x64748b))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(rgb(0x334155))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(0xe2e8f0)).border_color(rgb(0x334155)))
                                    .child("?")
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.show_help(cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.left_workspace(cx))
                    .child(
                        div()
                            .id("side_split")
                            .w(px(5.))
                            .h_full()
                            .flex_shrink_0()
                            .cursor(CursorStyle::ResizeColumn)
                            .bg(rgb(0xcbd5e1))
                            .hover(|s| s.bg(rgb(0x94a3b8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    this.drag = Some(DragKind::SideResize {
                                        start_x: f32::from(ev.position.x),
                                        start_w: this.side_width,
                                    });
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(self.right_workspace(cx)),
            )
            .child(self.dialog_overlay(cx))
            .child(self.tab_context_menu_overlay(cx))
            .child(self.tab_tooltip_overlay())
            .child(self.tab_drag_ghost())
            .child(self.member_drag_ghost())
            .child(self.group_drag_ghost())
            .child(
                self.score_video
                    .read(cx)
                    .audio_drag_ghost()
                    .into_any_element(),
            )
            .child(self.outside_window_drag_capture(cx))
    }
}

pub fn run_gui(initial: Vec<PathBuf>) {
    Application::new().run(move |cx: &mut App| {
        apply_bg::text_input::bind_keys(cx);
        score_video::gui::bind_keys(cx);
        let mut keys = vec![
            KeyBinding::new("d", DetectPage, Some("ScoreSync")),
            KeyBinding::new("a", DetectAll, Some("ScoreSync")),
            KeyBinding::new("n", ToggleAddBlock, Some("ScoreSync")),
            KeyBinding::new("s", ToggleSplitBlock, Some("ScoreSync")),
            KeyBinding::new("m", MergeSelected, Some("ScoreSync")),
            KeyBinding::new("u", UngroupActive, Some("ScoreSync")),
            KeyBinding::new("g", ShareIntoGroup, Some("ScoreSync")),
            KeyBinding::new("e", ExportGroups, Some("ScoreSync")),
            KeyBinding::new("r", ResetGroups, Some("ScoreSync")),
            KeyBinding::new("f", FitView, Some("ScoreSync")),
            KeyBinding::new("h", ShowHelp, Some("ScoreSync")),
            KeyBinding::new("f1", ShowHelp, Some("ScoreSync")),
            KeyBinding::new("h", ShowHelp, None),
            KeyBinding::new("f1", ShowHelp, None),
            KeyBinding::new("delete", DeleteSelected, Some("ScoreSync")),
            KeyBinding::new("backspace", DeleteSelected, Some("ScoreSync")),
            KeyBinding::new("enter", ConfirmParamEdit, Some("ScoreSync")),
            KeyBinding::new("escape", CancelParamEdit, Some("ScoreSync")),
            KeyBinding::new("enter", ConfirmParamEdit, None),
            KeyBinding::new("escape", CancelParamEdit, None),
            KeyBinding::new("e", mask_tool::gui::ExportImage, Some("MaskTool")),
            KeyBinding::new("f", mask_tool::gui::FitView, Some("MaskTool")),
            KeyBinding::new("delete", mask_tool::gui::DeleteSelected, Some("MaskTool")),
            KeyBinding::new("backspace", mask_tool::gui::DeleteSelected, Some("MaskTool")),
            KeyBinding::new("b", mask_tool::gui::ToggleDrawMode, Some("MaskTool")),
            KeyBinding::new("l", mask_tool::gui::TogglePolyMode, Some("MaskTool")),
            KeyBinding::new("p", mask_tool::gui::TogglePanMode, Some("MaskTool")),
            KeyBinding::new("escape", mask_tool::gui::CancelPolyDraft, Some("MaskTool")),
        ];
        keys.extend(apply_bg::bind_primary("o", OpenFile, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("shift-n", NewProject, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("shift-o", OpenProject, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("s", SaveProject, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("shift-s", SaveProjectAs, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("s", SaveProject, None));
        keys.extend(apply_bg::bind_primary("shift-s", SaveProjectAs, None));
        keys.extend(apply_bg::bind_primary("shift-o", OpenProject, None));
        keys.extend(apply_bg::bind_primary("shift-n", NewProject, None));
        keys.extend(apply_bg::bind_primary("s", SaveProject, Some("ScoreVideo")));
        keys.extend(apply_bg::bind_primary("shift-s", SaveProjectAs, Some("ScoreVideo")));
        keys.extend(apply_bg::bind_primary("shift-o", OpenProject, Some("ScoreVideo")));
        keys.extend(apply_bg::bind_primary("shift-n", NewProject, Some("ScoreVideo")));
        keys.extend(apply_bg::bind_primary("m", PairUngrouped, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("z", Undo, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("y", Redo, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("shift-z", Redo, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("a", SelectAllPageRegions, Some("ScoreSync")));
        keys.extend(apply_bg::bind_primary("o", mask_tool::gui::OpenFile, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("shift-o", OpenProject, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("shift-n", NewProject, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("s", SaveProject, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("shift-s", SaveProjectAs, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("a", mask_tool::gui::SelectAll, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("z", mask_tool::gui::Undo, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("y", mask_tool::gui::Redo, Some("MaskTool")));
        keys.extend(apply_bg::bind_primary("shift-z", mask_tool::gui::Redo, Some("MaskTool")));
        cx.bind_keys(keys);
        let bounds = default_window_bounds(cx);
        let initial = initial.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("曲谱同步 / Score Sync".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let entity = cx.new(|cx| {
                    let app = ScoreSyncApp::new(cx, initial.clone());
                    app.focus_handle.focus(window);
                    app
                });
                let weak = entity.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    let Some(entity) = weak.upgrade() else {
                        return true;
                    };
                    entity.update(cx, |app, cx| {
                        if app.allow_close {
                            return true;
                        }
                        app.dismiss_error_overlays(cx);
                        app.refresh_dirty_from_panels(cx);
                        if !app.dirty {
                            return true;
                        }
                        app.dialog = Some(DialogKind::UnsavedExit);
                        cx.notify();
                        false
                    })
                });
                entity
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

/// 标题栏保存中指示: 圆圈拖尾转圈 (非盲文点阵).
fn paint_save_spinner(window: &mut Window, bounds: Bounds<Pixels>, phase: f32) {
    let cx = f32::from(bounds.origin.x) + f32::from(bounds.size.width) * 0.5;
    let cy = f32::from(bounds.origin.y) + f32::from(bounds.size.height) * 0.5;
    let radius = f32::from(bounds.size.width)
        .min(f32::from(bounds.size.height))
        * 0.36;
    const N: i32 = 14;
    for i in 0..N {
        let t = i as f32 / N as f32;
        // 头部在 phase, 尾迹向后拖
        let ang = (phase - t * 0.72) * std::f32::consts::TAU;
        let alpha = ((1.0 - t).powf(1.55)).clamp(0.08, 1.0);
        let dot = 1.6 + (1.0 - t) * 2.8;
        let x = cx + ang.cos() * radius;
        let y = cy + ang.sin() * radius;
        let mut fill = rgb(0x2563eb);
        fill.a = alpha;
        window.paint_quad(quad(
            Bounds {
                origin: point(px(x - dot * 0.5), px(y - dot * 0.5)),
                size: size(px(dot), px(dot)),
            },
            px(dot),
            fill,
            px(0.),
            fill,
            Default::default(),
        ));
    }
}

/// 首选尺寸夹紧到主屏内并留边距, 保证四边都在屏幕内.
fn default_window_bounds(cx: &App) -> Bounds<Pixels> {
    const PREF_W: f32 = 1400.;
    const PREF_H: f32 = 920.;
    const MARGIN: f32 = 56.;
    const MIN_W: f32 = 720.;
    const MIN_H: f32 = 480.;

    let (avail_w, avail_h) = cx
        .primary_display()
        .map(|d| {
            let b = d.bounds();
            (f32::from(b.size.width), f32::from(b.size.height))
        })
        .unwrap_or((PREF_W, PREF_H));

    let max_w = (avail_w - MARGIN * 2.).max(MIN_W.min(avail_w));
    let max_h = (avail_h - MARGIN * 2.).max(MIN_H.min(avail_h));
    let w = PREF_W.min(max_w).clamp(1., avail_w.max(1.));
    let h = PREF_H.min(max_h).clamp(1., avail_h.max(1.));
    Bounds::centered(None, size(px(w), px(h)), cx)
}
