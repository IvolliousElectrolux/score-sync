//! GPUI 主界面: 曲谱同步 (分块 / 蒙版 / 加底色).
//!
//! 按职责拆开, `ScoreSyncApp` 仍是唯一状态机:
//! - `types` 常量/枚举/`DragKind`
//! - `canvas` 坐标变换与谱面交互
//! - `crop` 识别与加减块
//! - `io` 打开/保存/导出
//! - `tabs` / `lists` 页签与侧栏列表
//! - `page_organize` 组织页面弹窗
//! - `chrome` 工具栏、工作区、对话框
//! - `sync` 页图窗口与蒙版/视频同步
//! - `mem` 像素缓冲分项 dump
//! - `history` 分块撤重
//! - `host` 窗口外拖拽与分隔条

mod bg;
mod canvas;
mod chrome;
mod crop;
mod guides;
mod history;
mod host;
mod io;
mod keymap;
mod lists;
mod mem;
mod page_organize;
mod pdf_import;
mod photo;
mod settings;
mod spread_view;
mod sync;
mod tabs;
mod types;

pub(crate) use types::*;

use gpui::actions;

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
        OrganizePages,
        ShowHelp,
        DumpMemory,
        ShareIntoGroup,
        UngroupActive,
        ConfirmParamEdit,
        CancelParamEdit,
        Undo,
        Redo,
        SelectAllPageRegions,
        OpenPhotoEdit,
        PrevPage,
        NextPage,
        CopySpreadPages,
        PasteSpreadPages,
        CopyOrganizePages,
        PasteOrganizePages,
        OpenSettings,
    ]
);

pub(crate) use crate::config;
pub(crate) use crate::model::{
    is_image_path, is_open_path, is_pdf_path, parse_color_hex, DocState,
};
pub(crate) use crate::pdf;
pub(crate) use crate::project::{self, is_project_path};
pub(crate) use apply_bg::gui::ApplyBgApp;
pub(crate) use apply_bg::is_primary_mod;
pub(crate) use apply_bg::text_input::TextInput;
pub(crate) use gpui::prelude::*;
pub(crate) use gpui::{
    canvas, div, point, px, quad, rgb, size, App, Application, Bounds, Context, CursorStyle,
    DispatchPhase, DisplayId, Entity, ExternalPaths, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    RenderImage, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
pub(crate) use mask_tool::gui::MaskToolApp;
pub(crate) use mask_tool::mask::MaskRect;
pub(crate) use photo_edit::PhotoEditApp;
pub(crate) use score_video::gui::ScoreVideoApp;
pub(crate) use score_video::model::MaterialItem;

pub(crate) struct ScoreSyncApp {
    focus_handle: FocusHandle,
    doc: DocState,
    render_image: Option<Arc<RenderImage>>,
    /// 适配视口时的整页贴图; 放大后作为底层, 视口裁切叠在上面.
    fit_render_image: Option<Arc<RenderImage>>,
    /// 当前 `render_image` 对应的原图矩形. `lod_is_fit` 时就是整页.
    view_lod: Option<canvas::ViewLod>,
    lod_is_fit: bool,
    /// 放大超过预览密度时暂存当前页原图, 回到适配即丢.
    lod_full: Option<Arc<image::RgbImage>>,
    lod_gen: u64,
    /// 正在后台生成的视口贴图; 未完成前不要重复开线程.
    lod_pending: Option<canvas::ViewLod>,
    /// 待从 GPUI 图集卸掉的旧贴图 (下一帧 `render` 里 `drop_image`).
    gpu_drop: Vec<Arc<RenderImage>>,
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
    /// 右侧工具: 分块 | 底色 | 蒙版 | 视频
    side_tool: SideTool,
    /// 画布工具: 普通 / 添加新块 / 分割块
    canvas_tool: CanvasTool,
    mask_tool: Entity<MaskToolApp>,
    photo_edit: Entity<PhotoEditApp>,
    vector_edit: Entity<vector_edit::VectorEditApp>,
    apply_bg: Entity<ApplyBgApp>,
    score_video: Entity<ScoreVideoApp>,
    photo_session: Option<photo::PhotoSession>,
    /// 打开修图的代数. 离开或再次打开后, 还在后台的那次结果作废.
    photo_open_gen: u64,
    photo_cmd_pending: bool,
    bg: bg::BgUi,
    /// 当前蒙版编辑目标: group_id (拼合图)
    mask_target: Option<String>,
    /// 当前蒙版预览图相对拼合图的横向/纵向偏移 (叠加工程底色补边时非零)
    mask_preview_hoff: i64,
    mask_preview_voff: i64,
    /// 当前蒙版编辑目标各分块的原始宽高 (region_id, w, h), 只给
    /// `natural_voff` / `voff_shift` 反算用, 不保留整段 RGB.
    block_piece_sizes: Vec<(String, u32, u32)>,
    /// 底色 GPU 贴图缓存 (`bg_gen` + 谱面宽 + 纵横比 → 已按目标页裁切的
    /// `BlockBgTile`). 完整底色只留 `DocState::bg_image` 一份备份; 贴图
    /// 用 [`apply_bg::process::crop_bg_to_page`] 裁出的恰好大小画布.
    /// `bg_gen` 变了 (换了底色图) 或目标页尺寸变了才需要重算.
    bg_tile_cache: Option<(u64, u32, mask_tool::gui::BlockBgTile)>,
    /// 上一次从蒙版工具读到并同步过的分块布局 (拖动分块时逐帧比较用).
    last_synced_block_layout: Vec<mask_tool::layout::BlockAdjust>,
    /// 上一次从蒙版工具读到并同步过的组合"目标纵向位置"
    /// (`mask_tool::gui::MaskToolApp::voff_target`, 逐帧比较用). 每次
    /// 检测到变化 (拖动或撤销/重做导致), 都用
    /// `apply_bg::process::natural_voff` 结合真实宽高比精确反算应该写入
    /// `DocState::group_voff_shift` 的量, 不在这里做任何线性近似.
    last_synced_voff_target: i64,
    /// 上一帧蒙版工具是否正在拖动分块. 松手那一帧 layout 可能没再变,
    /// 靠这个边沿把布局写回文档 (画面已是三层贴图, 不必再合成整图).
    block_drag_was_active: bool,
    dialog: Option<DialogKind>,
    settings_open: bool,
    settings_page: settings::SettingsPage,
    settings_capture: Option<(String, usize)>,
    settings_conflict: SharedString,
    /// 快捷键页的搜索框. 父视图观察它, 文字一变就重滤列表.
    settings_key_query: Entity<TextInput>,
    /// 必须留着, 否则观察会立刻取消.
    #[allow(dead_code)]
    settings_key_query_sub: gpui::Subscription,
    /// 与输入框内容同步, 避免在渲染里再读一遍.
    settings_key_filter: String,
    /// 录键搜索: 下一击写入搜索框, 而不是改绑定.
    settings_key_record: bool,
    capture_sub: Option<gpui::Subscription>,
    /// 窗口移动/缩放观察, 必须留着, 否则订阅会立刻取消.
    window_place_sub: Option<gpui::Subscription>,
    /// 位置保存的代数. 拖动过程中只让最后一次定时写入落地.
    window_place_gen: u64,
    update_probe: settings::UpdateProbe,
    /// 标签右键菜单
    tab_menu: Option<TabContextMenu>,
    /// 页签悬停 1s 后的完整文件名提示
    tab_hover_idx: Option<usize>,
    /// 对开缩略图下被截断的文件名, 悬停 1s 后共用 `tab_tooltip`
    spread_title_hover: Option<usize>,
    /// 标题栏图标悬停 (与页签共用 `tab_tooltip` / `tab_hover_gen`)
    header_hover_id: Option<&'static str>,
    tab_hover_gen: u64,
    tab_tooltip: Option<TabTooltip>,
    /// 提示已经显示过. 离开目标后还热一小段, 移到下一个该出提示的元素上不再等 1 秒.
    tooltip_warm: bool,
    tooltip_warm_gen: u64,
    /// 标题栏图标屏幕 bounds, 供悬浮提示定位
    header_btn_bounds: HashMap<String, Bounds<Pixels>>,
    /// 最近一帧窗口客户区, 悬浮提示夹紧用
    viewport_w: f32,
    viewport_h: f32,
    /// 原子块 y0-y1 行内编辑
    edit_y_input: Entity<TextInput>,
    /// 正在编辑 y 的 region id
    region_y_edit: Option<String>,
    /// 边距 / 墨迹阈值 点按编辑
    param_input: Entity<TextInput>,
    param_edit: Option<ParamEdit>,
    /// PDF 导入弹窗
    pdf_import: Option<pdf_import::PdfImportState>,
    /// 「组织页面」弹窗
    page_organize: Option<page_organize::PageOrganizeState>,
    /// 组织页面缩略图 (关弹窗也保留, 避免每次打开都重新解全页 PNG).
    org_thumbs: HashMap<String, Arc<RenderImage>>,
    /// 组织缩略图后台代次; 换工程时自增, 丢掉旧任务.
    org_thumb_gen: u64,
    org_thumb_busy: bool,
    org_thumb_miss: HashSet<String>,
    spread_thumb_queued: bool,
    pdf_w_input: Entity<TextInput>,
    pdf_h_input: Entity<TextInput>,
    pdf_scale_input: Entity<TextInput>,
    pdf_preview_page_input: Entity<TextInput>,
    /// 画布悬停光标 (边缘/分割)
    hover_cursor: CursorStyle,
    region_scroll: ScrollHandle,
    group_scroll: ScrollHandle,
    member_scroll: ScrollHandle,
    /// 「组合分块」列表滚动状态 (当前蒙版编辑目标内, 自上而下的成员分块).
    mask_block_scroll: ScrollHandle,
    /// 当前在「组合分块」列表/蒙版画布中选中的分块 (region_id).
    mask_active_block_id: Option<String>,
    help_scroll: ScrollHandle,
    update_scroll: ScrollHandle,
    tab_scroll: ScrollHandle,
    /// 「组织页面」网格滚动; 对开网格共用.
    page_organize_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    spread_cells: HashMap<usize, Bounds<Pixels>>,
    /// 对开文件名那一行的屏幕范围, 悬浮提示锚在这里
    spread_caption_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 点选顺序. 复制时按这个顺序收成一串, 不按页序.
    spread_selected: Vec<String>,
    spread_anchor: Option<String>,
    /// Ctrl+C 记下的页 id, 仍是点选顺序. Ctrl+V 再贴.
    spread_clip: Vec<String>,
    /// 组织页面里 Ctrl+C 记下的页 id, 点选顺序.
    organize_clip: Vec<String>,
    spread_drag: Option<spread_view::SpreadDrag>,
    spread_menu: Option<spread_view::SpreadMenu>,
    /// 蒙版组合页签独立滚动状态; 与分块页签的 `tab_scroll` 分开, 避免切换
    /// 面板时共用同一 handle 导致上一帧 max_offset 对不上, 滚动条抽搐一下.
    mask_tab_scroll: ScrollHandle,
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
    /// 分块撤重和页序撤重共用的序号, 保证 Ctrl+Z 先撤最近的一次.
    crop_undo_seq: u64,
    /// 底色应用/取消/切换/改比例撤重.
    bg_history: BgHistory,
    /// 蒙版「全局开启 / 全局对齐」等跨组合操作, 与当前页 `host_guide_token` 配对.
    guide_undo: Vec<GuideHistEntry>,
    guide_redo: Vec<GuideHistEntry>,
    next_guide_token: u64,
    /// 后台全局对齐代次; 文档改动或再次点击会对掉旧结果.
    align_all_gen: u64,
    align_all_running: bool,
    /// 有未保存改动
    dirty: bool,
    /// 切页异步加载代数, 防止连切时旧结果覆盖
    page_load_gen: u64,
    /// 当前页 GPU 贴图 (缩放 + BGRA 转换) 后台生成代数; 高清页这一步单独
    /// 就要上百毫秒, 见 `refresh_render`/`mask_tool::gui::rgb_to_render_image`
    /// 文档, 绝不能堵在界面线程上, 否则切页/撤重等"任何操作"都会卡顿.
    render_gen: u64,
    /// 蒙版预览后台生成代数, 见 `sync_mask_image` 文档
    mask_sync_gen: u64,
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
    /// 按下发生在哪个页签的「×」上; 仅在同一叉上松开才关页 (拖到其他页签叉上松开不关).
    tab_close_press: Option<usize>,
    /// 普通按钮按下 id; 松开时必须仍是同一按钮才算点击.
    btn_press: Option<SharedString>,
    /// 启动检查到的更新; 等当前对话框关掉后再弹出.
    pending_update: Option<crate::update::UpdateInfo>,
    /// 页图未就绪, 等加载后再识别.
    pending_redetect: bool,
    /// 「输出组合」列表按页分桶+排序的缓存 (`group_list_rows_in` 用), 避免
    /// 页签拖拽等纯 UI 交互每一帧都要把全部组合重新分桶排序一遍 (大工程
    /// 卡顿的根因之一, 与蒙版面板"组合分块"只列当前组合成员、天然轻量
    /// 不同). 用当前各组合 `(页序,y0,y1)` 的折叠校验值判断是否需要重建,
    /// 而不是去逐一追踪"文档结构改了"的时机 (后台 hydrate/redetect 等
    /// 异步流程也会改动组合, 手动追踪容易漏, 校验值能保证任何情况下都
    /// 不会读到脏数据).
    group_by_page_cache: std::cell::RefCell<Option<(u64, HashMap<usize, Vec<(usize, i32, i32)>>)>>,
    mem_last_accounted: u64,
    mem_last_ws: u64,
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
        let pdf_preview_page_input = cx.new(|cx| TextInput::new(cx, "1", "页").with_compact(true));
        let settings_key_query = cx.new(|cx| {
            TextInput::new(cx, "", ui_kit::t("settings.search_keys"))
        });
        let settings_key_query_sub = cx.observe(&settings_key_query, |this, input, cx| {
            let (focused, text) = input.update(cx, |input, _| (input.take_focus_commit(), input.text()));
            if focused {
                this.end_key_search(cx);
            }
            if this.settings_key_filter != text {
                this.settings_key_filter = text;
                this.settings_scroll.set_offset(point(px(0.), px(0.)));
            }
            cx.notify();
        });
        let mask_tool = cx.new(|cx| {
            let mut m = MaskToolApp::new(cx, None);
            m.apply_color_prefs(mask_prefs.clone());
            m
        });
        cx.observe(&mask_tool, |this, mt, cx| {
            if let Some(path) = mt.update(cx, |m, _| m.take_export_path()) {
                this.export_mask_group_to(path, cx);
            }
            if this.side_tool == SideTool::Project {
                let wheel = mt.update(cx, |m, _| m.take_preview_wheel());
                if wheel != 0 {
                    this.cycle_bg_preview_group(wheel, cx);
                }
                if let Some(rgb) = mt.update(cx, |m, _| m.take_host_pick_click()) {
                    this.apply_bg_eyedropper(rgb, cx);
                } else if this.bg.eyedropper_armed {
                    if let Some(hover) = mt.read(cx).host_pick_hover() {
                        this.preview_bg_eyedropper(hover, cx);
                    }
                }
                cx.notify();
                return;
            }
            let dragging = mt.read(cx).is_block_dragging();
            this.sync_block_layout_from_mask_tool(&mt, cx);
            this.handle_guide_host_cmd(cx);
            let step = mt.update(cx, |m, _| m.take_page_step_wheel());
            if step != 0 && !dragging {
                this.step_tab_direct(step as isize, cx);
            }
            if !dragging {
                cx.notify();
            }
        })
        .detach();
        let photo_edit = cx.new(|cx| PhotoEditApp::new(cx, false));
        cx.observe(&photo_edit, |this, photo, cx| {
            let p = photo.read(cx);
            if p.has_host_cmd() {
                this.photo_cmd_pending = true;
                cx.notify();
            } else if !p.is_nav_only() {
                cx.notify();
            }
        })
        .detach();
        let vector_edit = cx.new(|cx| vector_edit::VectorEditApp::new(cx, false));
        cx.observe(&vector_edit, |this, edit, cx| {
            let p = edit.read(cx);
            if p.has_host_cmd() {
                this.photo_cmd_pending = true;
                cx.notify();
            } else if !p.is_nav_only() {
                cx.notify();
            }
        })
        .detach();
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
            fit_render_image: None,
            view_lod: None,
            lod_is_fit: true,
            lod_full: None,
            lod_gen: 0,
            lod_pending: None,
            gpu_drop: Vec::new(),
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
            photo_edit,
            vector_edit,
            photo_session: None,
            photo_open_gen: 0,
            photo_cmd_pending: false,
            apply_bg,
            score_video,
            bg: bg::BgUi::new(cx),
            mask_target: None,
            mask_preview_hoff: 0,
            mask_preview_voff: 0,
            block_piece_sizes: Vec::new(),
            bg_tile_cache: None,
            last_synced_block_layout: Vec::new(),
            last_synced_voff_target: 0,
            block_drag_was_active: false,
            dialog: None,
            settings_open: false,
            settings_page: settings::SettingsPage::Project,
            settings_capture: None,
            settings_conflict: SharedString::default(),
            settings_key_query,
            settings_key_query_sub,
            settings_key_filter: String::new(),
            settings_key_record: false,
            capture_sub: None,
            window_place_sub: None,
            window_place_gen: 0,
            update_probe: settings::UpdateProbe::Idle,
            tab_menu: None,
            tab_hover_idx: None,
            spread_title_hover: None,
            header_hover_id: None,
            tab_hover_gen: 0,
            tab_tooltip: None,
            tooltip_warm: false,
            tooltip_warm_gen: 0,
            header_btn_bounds: HashMap::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
            edit_y_input,
            region_y_edit: None,
            param_input,
            param_edit: None,
            pdf_import: None,
            page_organize: None,
            org_thumbs: HashMap::new(),
            org_thumb_gen: 0,
            org_thumb_busy: false,
            org_thumb_miss: HashSet::new(),
            spread_thumb_queued: false,
            pdf_w_input,
            pdf_h_input,
            pdf_scale_input,
            pdf_preview_page_input,
            hover_cursor: CursorStyle::Arrow,
            region_scroll: ScrollHandle::new(),
            group_scroll: ScrollHandle::new(),
            member_scroll: ScrollHandle::new(),
            mask_block_scroll: ScrollHandle::new(),
            mask_active_block_id: None,
            help_scroll: ScrollHandle::new(),
            update_scroll: ScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            page_organize_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            spread_cells: HashMap::new(),
            spread_caption_bounds: HashMap::new(),
            spread_selected: Vec::new(),
            spread_anchor: None,
            spread_clip: Vec::new(),
            organize_clip: Vec::new(),
            spread_drag: None,
            spread_menu: None,
            mask_tab_scroll: ScrollHandle::new(),
            tab_bounds: HashMap::new(),
            member_bounds: HashMap::new(),
            group_bounds: HashMap::new(),
            project_path: None,
            saving: false,
            opening: false,
            video_sync_gen: 0,
            crop_histories: HashMap::new(),
            page_struct_history: CropHistory::default(),
            crop_undo_seq: 0,
            bg_history: BgHistory::default(),
            guide_undo: Vec::new(),
            guide_redo: Vec::new(),
            next_guide_token: 1,
            align_all_gen: 0,
            align_all_running: false,
            dirty: false,
            page_load_gen: 0,
            render_gen: 0,
            mask_sync_gen: 0,
            hydrate_gen: 0,
            pdf_load_gen: Arc::new(AtomicU64::new(0)),
            pdf_importing: false,
            video_pool_dirty: HashSet::new(),
            video_pool_all_dirty: true,
            allow_close: false,
            save_spin_phase: 0.0,
            tab_add_press: false,
            tab_close_press: None,
            btn_press: None,
            pending_update: None,
            pending_redetect: false,
            group_by_page_cache: std::cell::RefCell::new(None),
            mem_last_accounted: 0,
            mem_last_ws: 0,
        };
        app.observe_bg_rgb_inputs(cx);
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
        } else if ui_kit::reopen_last_project() {
            // 命令行没带任何文件时, 按偏好恢复上次打开的工程.
            let last = config::load().last_project;
            if !last.is_empty() {
                let path = PathBuf::from(last);
                if is_project_path(&path) && path.is_file() {
                    app.open_project_path(path, cx);
                }
            }
        }
        app.start_update_check(cx);
        app.start_memory_trace(cx);
        app
    }

    fn watch_window_placement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_place_sub = Some(cx.observe_window_bounds(window, |this, window, cx| {
            this.schedule_window_placement_save(window, cx);
        }));
    }

    fn schedule_window_placement_save(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (x, y, w, h, maximized) = window_placement_of(window);
        self.window_place_gen = self.window_place_gen.wrapping_add(1);
        let gen = self.window_place_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            this.update(cx, |app, _| {
                if app.window_place_gen != gen {
                    return;
                }
                config::remember_window(x, y, w, h, maximized);
            })
            .ok();
        })
        .detach();
    }

    fn store_window_placement(&mut self, window: &Window) {
        // 关掉尚未落地的定时保存, 避免退出后用旧坐标覆盖这次的位置.
        self.window_place_gen = self.window_place_gen.wrapping_add(1);
        let (x, y, w, h, maximized) = window_placement_of(window);
        config::remember_window(x, y, w, h, maximized);
    }

    /// 分块面板专属快捷键 (识别/开图/加块等). 蒙版、底色、视频和组织页面 overlay 都不走这套.
    fn crop_keys_live(&self) -> bool {
        self.side_tool == SideTool::Crop && self.page_organize.is_none() && !self.settings_open
    }

    fn staff_keys_live(&self) -> bool {
        self.crop_keys_live() && !self.doc.is_spread()
    }
}

impl Focusable for ScoreSyncApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ScoreSyncApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.flush_gpu_drops(window, cx);
        self.viewport_w = f32::from(window.viewport_size().width);
        self.viewport_h = f32::from(window.viewport_size().height);
        let canvas_side =
            f32::from(self.view_bounds.size.width).max(f32::from(self.view_bounds.size.height));
        let side = if canvas_side >= 64.0 {
            canvas_side
        } else {
            self.viewport_w.max(self.viewport_h)
        };
        if side >= 64.0 {
            self.doc.set_display_max_side(side.round() as u32);
        }
        self.sync_view_lod(cx);
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
        if self.photo_cmd_pending {
            self.photo_cmd_pending = false;
            self.handle_photo_host_cmd(window, cx);
        }
        let mask_mode = self.side_tool == SideTool::Mask;
        let video_mode = self.side_tool == SideTool::Video;
        let organize_open = self.page_organize.is_some();
        let photo_open = self.photo_session.is_some();
        let focus = if organize_open {
            self.focus_handle.clone()
        } else if photo_open {
            if self.vector_session() {
                self.vector_edit.read(cx).focus_handle_ref().clone()
            } else {
                self.photo_edit.read(cx).focus_handle_ref().clone()
            }
        } else if mask_mode {
            self.mask_tool.read(cx).focus_handle_ref().clone()
        } else if video_mode {
            self.score_video.read(cx).focus_handle_ref().clone()
        } else {
            self.focus_handle.clone()
        };
        let key_ctx = if self.settings_open {
            "Settings"
        } else if organize_open {
            "PageOrganize"
        } else if photo_open {
            if self.vector_session() {
                "VectorEdit"
            } else {
                "PhotoEdit"
            }
        } else if mask_mode {
            "MaskTool"
        } else if video_mode {
            "ScoreVideo"
        } else if self.side_tool == SideTool::Project {
            "ScoreProject"
        } else {
            "ScoreSync"
        };

        div()
            .id("root")
            .key_context(key_ctx)
            .track_focus(&focus)
            .on_modifiers_changed(
                cx.listener(|this, ev: &gpui::ModifiersChangedEvent, _, cx| {
                    if this.side_tool == SideTool::Mask && !this.photo_open() {
                        let shift = ev.modifiers.shift;
                        this.mask_tool.update(cx, |m, cx| m.note_shift(shift, cx));
                    }
                }),
            )
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
                if this.photo_open() {
                    this.apply_host_drag_at(x, y, cx);
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
                if this.drag.is_none() && this.side_tool == SideTool::Project {
                    this.mask_tool.update(cx, |m, cx| {
                        if m.needs_root_move_forward() {
                            m.root_mouse_move(x, y, cx);
                        }
                    });
                    this.apply_bg_palette_drag(x, y, cx);
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
                    if this.photo_open() {
                        this.finish_host_drag_at(
                            f32::from(ev.position.x),
                            f32::from(ev.position.y),
                            cx,
                        );
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
                        this.mask_tool.update(cx, |m, cx| m.root_mouse_up(x, y, cx));
                    }
                    if this.side_tool == SideTool::Project {
                        let x = f32::from(ev.position.x);
                        let y = f32::from(ev.position.y);
                        this.mask_tool.update(cx, |m, cx| m.root_mouse_up(x, y, cx));
                    }
                    this.finish_host_drag_at(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
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
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                if !this.crop_keys_live() {
                    return;
                }
                this.open_file(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenProject, window, cx| this.open_project(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewProject, window, cx| {
                    this.request_new_project(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SaveProject, window, cx| this.save_project(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SaveProjectAs, window, cx| this.save_project_as(window, cx)),
            )
            .on_action(cx.listener(|this, _: &DetectPage, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.run_detect(cx)
            }))
            .on_action(cx.listener(|this, _: &DetectAll, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.run_detect_all(cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleAddBlock, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.toggle_add_block(cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSplitBlock, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.toggle_split_block(cx)
            }))
            .on_action(cx.listener(|this, _: &MergeSelected, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.merge_selected(cx)
            }))
            .on_action(cx.listener(|this, _: &PairUngrouped, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.pair_ungrouped(cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| {
                if this.page_organize.is_some() {
                    this.delete_organize_selected(cx);
                    return;
                }
                if this.crop_keys_live() && this.doc.is_spread() {
                    this.delete_spread_selected(cx);
                    return;
                }
                if !this.crop_keys_live() {
                    return;
                }
                this.delete_selected(cx)
            }))
            .on_action(cx.listener(|this, _: &ExportGroups, window, cx| {
                if !this.crop_keys_live() {
                    return;
                }
                this.export_groups_ui(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetGroups, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.reset_groups(cx)
            }))
            .on_action(cx.listener(|this, _: &FitView, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.fit_to_view(cx)
            }))
            .on_action(cx.listener(|this, _: &OrganizePages, window, cx| {
                if this.doc.is_spread() && this.side_tool == SideTool::Crop {
                    return;
                }
                if this.page_organize.is_none() && this.side_tool != SideTool::Crop {
                    return;
                }
                this.toggle_page_organize(window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenPhotoEdit, window, cx| {
                this.try_open_photo_edit(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowHelp, _, cx| this.show_help(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(cx.listener(|this, _: &DumpMemory, _, cx| this.dump_memory_now(cx)))
            .on_action(cx.listener(|this, _: &ShareIntoGroup, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.share_into_group(cx)
            }))
            .on_action(cx.listener(|this, _: &UngroupActive, _, cx| {
                if !this.staff_keys_live() {
                    return;
                }
                this.ungroup_active(cx)
            }))
            .on_action(cx.listener(|this, _: &ConfirmParamEdit, window, cx| {
                if this.pdf_import.is_some() {
                    this.on_pdf_import_enter(window, cx);
                } else if this.param_edit.is_some() {
                    this.apply_param_edit(window, cx);
                } else if this.region_y_edit.is_some() {
                    this.apply_edit_y(window, cx);
                } else {
                    // 没有浮层时把 Enter 交回去, 文本框自己的确认才能接到.
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &CancelParamEdit, window, cx| {
                // 无上下文的 Esc 和 PhotoEdit 的 Esc 深度相同, 后注册的这条会先接到按键.
                // 没有要关掉的浮层时, 交给修图: 取消选区, 或离开本次修图.
                if this.param_edit.is_some() {
                    this.cancel_param_edit(window, cx);
                } else if this.region_y_edit.is_some() {
                    this.cancel_edit_y(window, cx);
                } else if this.has_modal_overlay(cx) {
                    this.dismiss_blocking_overlays(cx);
                } else if this.vector_session() {
                    this.vector_edit.update(cx, |p, cx| {
                        p.on_escape(cx);
                    });
                } else if this.photo_open() {
                    this.photo_edit.update(cx, |p, cx| p.on_escape(cx));
                } else {
                    this.dismiss_blocking_overlays(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::ExportImage, window, cx| {
                    this.mask_tool
                        .update(cx, |m, cx| m.export_image(window, cx));
                }),
            )
            .on_action(cx.listener(|this, _: &mask_tool::gui::FitView, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.fit_to_view(cx));
            }))
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::DeleteSelected, _, cx| {
                    this.mask_tool.update(cx, |m, cx| m.delete_selected(cx));
                }),
            )
            .on_action(cx.listener(|this, _: &mask_tool::gui::ClearMasks, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.clear_masks(cx));
            }))
            .on_action(cx.listener(|this, _: &mask_tool::gui::SelectAll, _, cx| {
                this.mask_tool.update(cx, |m, cx| m.select_all_masks(cx));
            }))
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::ToggleDrawMode, _, cx| {
                    this.mask_tool.update(cx, |m, cx| m.toggle_draw_mode(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::TogglePanMode, _, cx| {
                    this.mask_tool.update(cx, |m, cx| m.toggle_pan_mode(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::ToggleBrushMode, _, cx| {
                    this.mask_tool.update(cx, |m, cx| m.toggle_brush_mode(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::TogglePolyMode, _, cx| {
                    this.mask_tool.update(cx, |m, cx| m.toggle_poly_mode(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &mask_tool::gui::CancelPolyDraft, _, cx| {
                    this.mask_tool.update(cx, |m, cx| m.cancel_poly_draft(cx));
                }),
            )
            .on_action(cx.listener(|this, _: &photo_edit::gui::Undo, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.undo(cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::Redo, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.redo(cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::FitView, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.fit_to_view(cx));
            }))
            .on_action(
                cx.listener(|this, _: &photo_edit::gui::DeleteLayer, _, cx| {
                    this.photo_edit
                        .update(cx, |p, cx| p.delete_active_layer(cx));
                }),
            )
            .on_action(cx.listener(|this, _: &photo_edit::gui::CopyLayer, _, cx| {
                if this.photo_session.is_none() || this.vector_session() {
                    return;
                }
                this.photo_edit.update(cx, |p, cx| p.copy_layer(cx));
            }))
            .on_action(
                cx.listener(|this, _: &photo_edit::gui::PasteLayer, window, cx| {
                    if this.photo_session.is_none() || this.vector_session() {
                        return;
                    }
                    this.photo_edit
                        .update(cx, |p, cx| p.paste_layer(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &vector_edit::gui::CopyLayer, window, cx| {
                    if !this.vector_session() {
                        return;
                    }
                    this.vector_edit
                        .update(cx, |p, cx| p.copy_layer(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &vector_edit::gui::PasteLayer, window, cx| {
                    if !this.vector_session() {
                        return;
                    }
                    this.vector_edit
                        .update(cx, |p, cx| p.paste_layer(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &photo_edit::gui::NewLayerFromSel, _, cx| {
                    this.photo_edit
                        .update(cx, |p, cx| p.new_layer_from_selection(cx));
                }),
            )
            .on_action(cx.listener(|this, _: &photo_edit::gui::Cancel, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.on_escape(cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ApplyEdit, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.request_apply(cx));
            }))
            .on_action(
                cx.listener(|this, _: &photo_edit::gui::RestoreOriginal, _, cx| {
                    this.photo_edit.update(cx, |p, cx| p.request_restore(cx));
                }),
            )
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolSelect, _, cx| {
                this.photo_edit.update(cx, |p, cx| {
                    p.set_mode(photo_edit::gui::ToolMode::Select, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolMove, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Move, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolCanvas, _, cx| {
                this.photo_edit.update(cx, |p, cx| {
                    p.set_mode(photo_edit::gui::ToolMode::Canvas, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolHeal, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Heal, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolClone, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Clone, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolPaint, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Paint, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolEraser, _, cx| {
                this.photo_edit.update(cx, |p, cx| {
                    p.set_mode(photo_edit::gui::ToolMode::Eraser, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolWand, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Wand, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolLasso, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Lasso, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::ToolPan, _, cx| {
                this.photo_edit
                    .update(cx, |p, cx| p.set_mode(photo_edit::gui::ToolMode::Pan, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::NudgeLeft, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.nudge(-1, 0, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::NudgeRight, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.nudge(1, 0, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::NudgeUp, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.nudge(0, -1, cx));
            }))
            .on_action(cx.listener(|this, _: &photo_edit::gui::NudgeDown, _, cx| {
                this.photo_edit.update(cx, |p, cx| p.nudge(0, 1, cx));
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if this.vector_session() {
                    this.vector_edit.update(cx, |p, cx| p.undo(cx));
                } else if this.photo_session.is_some() {
                    this.photo_edit.update(cx, |p, cx| p.undo(cx));
                } else if this.page_organize.is_some() {
                    this.undo_organize(cx);
                } else {
                    this.undo_action(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if this.vector_session() {
                    this.vector_edit.update(cx, |p, cx| p.redo(cx));
                } else if this.photo_session.is_some() {
                    this.photo_edit.update(cx, |p, cx| p.redo(cx));
                } else if this.page_organize.is_some() {
                    this.redo_organize(cx);
                } else {
                    this.redo_action(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopySpreadPages, _, cx| {
                this.copy_spread_clipboard(cx);
            }))
            .on_action(cx.listener(|this, _: &PasteSpreadPages, _, cx| {
                this.paste_spread_clipboard(cx);
            }))
            .on_action(cx.listener(|this, _: &CopyOrganizePages, _, cx| {
                this.copy_organize_clipboard(cx);
            }))
            .on_action(cx.listener(|this, _: &PasteOrganizePages, _, cx| {
                this.paste_organize_clipboard(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllPageRegions, _, cx| {
                if this.page_organize.is_some() {
                    this.select_all_organize_pages(cx);
                    return;
                }
                if this.side_tool == SideTool::Crop && this.doc.is_spread() {
                    this.select_all_spread_pages(cx);
                    return;
                }
                if this.side_tool != SideTool::Crop {
                    return;
                }
                this.doc.select_all_current_page_regions();
                this.scroll_group_list_to_active();
                this.after_doc_change(cx);
            }))
            .on_action(cx.listener(|this, _: &PrevPage, window, cx| {
                this.step_tab(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextPage, window, cx| {
                this.step_tab(1, window, cx);
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
            .on_action(
                cx.listener(|this, _: &score_video::gui::SeekForward, _, cx| {
                    this.score_video.update(cx, |v, cx| v.seek_by(1.0, cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::SeekBackBig, _, cx| {
                    this.score_video.update(cx, |v, cx| v.seek_by(-5.0, cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::SeekForwardBig, _, cx| {
                    this.score_video.update(cx, |v, cx| v.seek_by(5.0, cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::InsertNext, _, cx| {
                    this.score_video.update(cx, |v, cx| v.insert_next(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::InsertNextWipe, _, cx| {
                    this.score_video.update(cx, |v, cx| v.insert_next_wipe(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::MarkFadeIn, _, cx| {
                    this.score_video.update(cx, |v, cx| v.mark_fade_in(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::MarkFadeOut, _, cx| {
                    this.score_video.update(cx, |v, cx| v.mark_fade_out(cx));
                }),
            )
            .on_action(
                cx.listener(|this, _: &score_video::gui::DeleteSelected, _, cx| {
                    this.score_video.update(cx, |v, cx| v.delete_selected(cx));
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths.paths().iter().cloned().collect();
                if this.bg.pick_open {
                    this.apply_drop_as_bg(&list, cx);
                    return;
                }
                let list: Vec<PathBuf> = list
                    .into_iter()
                    .filter(|p| is_open_path(p) || is_project_path(p))
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
            .bg(ui_kit::chrome(0xf8fafc))
            .text_color(ui_kit::chrome(0x0f172a))
            .font_family(apply_bg::ui_font())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_b_1()
                    .border_color(ui_kit::chrome(0xcbd5e1))
                    .bg(ui_kit::chrome(0xf1f5f9))
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
                                    .text_color(ui_kit::chrome(0x0f172a))
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(title_core),
                            )
                            .when(saving, |d| {
                                d.child(
                                    div().w(px(18.)).h(px(18.)).flex_shrink_0().child(
                                        canvas(
                                            |_, _, _| {},
                                            move |bounds, _, window, _| {
                                                paint_save_spinner(window, bounds, spin_phase);
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
                                        .text_color(ui_kit::chrome(0xdc2626))
                                        .child("*"),
                                )
                            })
                            .child(div().flex_1().min_w(px(8.)))
                            .child(self.header_file_bar(cx)),
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
                            .w(px(SIDE_SPLIT_W))
                            .h_full()
                            .flex_shrink_0()
                            .cursor(CursorStyle::ResizeColumn)
                            .bg(ui_kit::chrome(0xcbd5e1))
                            .hover(|s| s.bg(ui_kit::chrome(0x94a3b8)))
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
            .child(self.dialog_overlay(window, cx))
            .child(self.tab_context_menu_overlay(cx))
            .child(self.spread_menu_overlay(cx))
            .child(self.guide_context_menu_overlay(cx))
            .child(self.tab_tooltip_overlay(cx))
            .child(self.tab_drop_line_overlay())
            .child(self.spread_drag_ghost())
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

struct HeaderIcons;

impl gpui::AssetSource for HeaderIcons {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "header/new.svg" => Some(include_bytes!("../../assets/全局选项图标/new.svg")),
            "header/open-project.svg" => {
                Some(include_bytes!("../../assets/全局选项图标/open-project.svg"))
            }
            "header/save.svg" => Some(include_bytes!("../../assets/全局选项图标/save.svg")),
            "header/save-as.svg" => Some(include_bytes!("../../assets/全局选项图标/save as.svg")),
            "header/help.svg" => Some(include_bytes!("../../assets/全局选项图标/help.svg")),
            "header/setting.svg" => Some(include_bytes!("../../assets/全局选项图标/setting.svg")),
            _ => None,
        };
        Ok(bytes.map(std::borrow::Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

pub fn run_gui(initial: Vec<PathBuf>) {
    Application::new()
        .with_assets(HeaderIcons)
        .run(move |cx: &mut App| {
            ui_kit::load_prefs_from_disk();
            keymap::install(cx);
            let (bounds, display_id) = remembered_window_bounds(cx);
            let initial = initial.clone();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(bounds),
                    display_id,
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
                    entity.update(cx, |app, cx| app.watch_window_placement(window, cx));
                    let weak = entity.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        let Some(entity) = weak.upgrade() else {
                            return true;
                        };
                        entity.update(cx, |app, cx| {
                            app.store_window_placement(window);
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
    let radius = f32::from(bounds.size.width).min(f32::from(bounds.size.height)) * 0.36;
    const N: i32 = 14;
    for i in 0..N {
        let t = i as f32 / N as f32;
        // 头部在 phase, 尾迹向后拖
        let ang = (phase - t * 0.72) * std::f32::consts::TAU;
        let alpha = ((1.0 - t).powf(1.55)).clamp(0.08, 1.0);
        let dot = 1.6 + (1.0 - t) * 2.8;
        let x = cx + ang.cos() * radius;
        let y = cy + ang.sin() * radius;
        let mut fill = ui_kit::chrome(0x2563eb);
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

/// 最大化时记下的是还原后的矩形, 不是铺满屏幕的那一帧.
fn window_placement_of(window: &Window) -> (f32, f32, f32, f32, bool) {
    let state = window.window_bounds();
    let maximized = matches!(
        state,
        WindowBounds::Maximized(_) | WindowBounds::Fullscreen(_)
    );
    let bounds = state.get_bounds();
    (
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        maximized,
    )
}

/// 有上次记录就按那个位置和大小打开 (最大化也恢复); 没有, 或屏幕对不上, 则居中.
fn remembered_window_bounds(cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let fallback = WindowBounds::Windowed(default_window_bounds(cx));
    let Some(saved) = config::load().window else {
        return (fallback, None);
    };
    let displays = cx.displays();
    if displays.is_empty() {
        return (fallback, None);
    }
    let rects: Vec<(f32, f32, f32, f32)> = displays
        .iter()
        .map(|d| {
            let b = d.bounds();
            (
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )
        })
        .collect();
    let primary = cx
        .primary_display()
        .and_then(|p| displays.iter().position(|d| d.id() == p.id()))
        .unwrap_or(0);
    let Some((x, y, w, h, ix)) = config::place_window(&saved, &rects, primary) else {
        return (fallback, None);
    };
    let bounds = Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    };
    let state = if saved.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    };
    (state, displays.get(ix).map(|d| d.id()))
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

#[cfg(test)]
mod esc_key_tests {
    use gpui::{KeyBinding, KeyContext, Keymap, Keystroke};

    use super::CancelParamEdit;

    #[test]
    fn global_escape_is_dispatched_before_photo_edit() {
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("escape", photo_edit::gui::Cancel, Some("PhotoEdit")),
            KeyBinding::new("escape", CancelParamEdit, None),
        ]);
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("escape").unwrap()],
            &[KeyContext::parse("PhotoEdit").unwrap()],
        );
        assert!(!pending);
        assert!(
            result[0].action().partial_eq(&CancelParamEdit),
            "无上下文的 Esc 后注册, 深度与 PhotoEdit 相同, 会先于修图的 Cancel"
        );
    }
}
