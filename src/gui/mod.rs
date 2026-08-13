//! GPUI 主界面: 曲谱同步 (分块 / 蒙版 / 加底色).

mod canvas;
mod lists;
mod tabs;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, canvas, div, point, prelude::*, px, quad, rgb, size, App, Application, Bounds,
    Context, CursorStyle, DispatchPhase, Entity, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
use image::{Frame, ImageBuffer, RgbaImage};
use smallvec::smallvec;

use crate::config;
use crate::model::{
    is_image_path, is_open_path, is_pdf_path, parse_color_hex, DocState, Group, Page, Region,
};
use crate::pdf;
use crate::project::{self, is_project_path};
use crate::text_input::{self, TextInput};

use canvas::{hit_edge, region_at, ViewXform};
use lists::{ListRow, TabInfo};
use mask_tool::gui::MaskToolApp;
use mask_tool::mask::MaskRect;
use apply_bg::gui::ApplyBgApp;
use score_video::gui::ScoreVideoApp;
use score_video::model::MaterialItem;

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

const CROP_HISTORY_LIMIT: usize = 64;

const A4_RATIO: f32 = 210.0 / 297.0;
const SIDE_PANEL_W: f32 = 340.0;
const SIDE_PANEL_MIN: f32 = 220.0;
/// 拖拽排序: 超过此像素位移才进入拖拽态 (防点击抖动出虚影)
const REORDER_DRAG_SLOP: f32 = 5.0;
const SIDE_PANEL_MAX: f32 = 720.0;
const HELP_TEXT: &str = "\
【分块】快捷键:\n\
  Ctrl+O 打开图片/PDF | Ctrl+Shift+N 新建工程 | Ctrl+Shift+O 打开工程 | Ctrl+S 保存工程 | Ctrl+Shift+S 另存工程\n\
  D 识别本页 | A 识别全部页\n\
  N 添加新块 | S 分割块 | M 合并组合 | U 拆开组合 | G 共享脚注 | Delete 删除\n\
  E 导出组合 | R 重置本页分组 | F 适应窗口 | H / F1 操作说明\n\
  Ctrl+A 全选本页原子块 | 输出组合 Ctrl+点击多选 (拖拽时整块一起调序)\n\
  Ctrl+Z/Y 撤重 (按当前标签页独立记忆; 关闭页面亦可撤回)\n\
\n\
【蒙版】快捷键 (右侧切到蒙版后):\n\
  B 框选 | L 折线 (逐点连线, 吸附首点闭环) | P 平移 | 画笔/橡皮 (侧栏, 可调色/粗细)\n\
  E 导出本页图片 | F 适应 | Delete 删除选中\n\
  Ctrl+A 全选蒙版 | Ctrl+Z/Y 撤重 (按组合独立记忆, 切走再回来仍可撤)\n\
  Ctrl+S 保存工程 (各面板通用)\n\
  有选中时透明度滑条改选中项; 无选中时改后续新建默认透明度\n\
  点击色块打开浮动取色器: HSV / 最近色 / RGB 手输; 滴管可从左侧图取色\n\
  (悬浮实时预览色盘与 RGB, 单击确认, Esc/右键取消); 画笔光标为圆形预览\n\
\n\
【视频】快捷键 (右侧切到视频后, 先在轨道/预览区点一下获得焦点):\n\
  空格 播放/暂停 | ← / → 快退/快进 1 秒 | Shift+← / Shift+→ 快退/快进 5 秒\n\
  N 在播放头插入下一张组合 (按素材池顺序自动顺延) | I 标记淡入 | O 标记淡出\n\
  Delete / Backspace 删除当前选中的视频片段/淡入淡出/音频片段\n\
  Ctrl+Z/Y 撤重 (时间轴操作)\n\
  鼠标: 拖动片段两端裁剪, 拖动片段整体移动; 淡入淡出轨道可直接拖选一段生成区间,\n\
  视频/淡入淡出/音频边界彼此靠近时自动吸附; 时间轴总长对齐最短的非空音/视频轨\n\
  (删短一轨时较长轨一并裁齐); 音频片段可左右拖动重新排序;\n\
  轨道区 Ctrl+滚轮缩放、普通滚轮左右平移,\n\
  底部横条可整体拖动平移, 拖两端圆点改变缩放.\n\
\n\
操作步骤:\n\
1. 打开/拖入图片或 PDF → 多标签页; 页图写入会话临时目录, 内存只留当前页±4.\n\
2. Ctrl+S 保存为单个 .staffcrop 工程包 (zip), 下次可用 Ctrl+Shift+O 继续; Ctrl+Shift+N 新建空白工程后再导入; 有未保存改动关窗会确认.\n\
3. 标签右键菜单「复制本页」可再放一页副本; 新页的输出组合插在原页组合之后、下一页之前.\n\
4. 每页独立识别分块; 「识别全部页」按可用内存限并发异步处理.\n\
5. 「添加新块」(N): 按下定一条边; 先上移则该边为下边线, 先下移则该边为上边线, 拖出另一边后松开.\n\
6. 「分割块」(S): 在已有块内点击, 于指针 y 切成上下两块.\n\
7. Ctrl 多选可跨页, 「合并组合」; 脚注可用「共享脚注」让同一块出现在多组导出中.\n\
8. 「输出组合」可 Ctrl 多选并以整块拖拽调序 (导出按列表顺序); 标签为「排序号. p页c页内」\n\
   (p/c 按该组最上块所在页及该页内自上而下序号; 未手动调序时亦按最上块自动排).\n\
   左侧点选块或切回分块时, 列表会滚到对应组合.\n\
9. 「蒙版」编辑当前组合的竖向拼合图; 组合标签与分块一致为「排序号. 来源号」\n\
   (共享脚注可在不同组画不同遮盖). 标签栏/侧栏切换组合; 与分块互相切换时会定位并滚动到对应组合.\n\
10. 「导出组合」按「输出组合」列表顺序拼接并套用各组蒙版; 蒙版侧「导出本页图片」只导出当前组合.\n\
11. 「工程」页「应用到工程组合」把工程底色作为可撤销的底层异步叠加到各输出组合 (不卡界面),\n\
    「取消工程底色」还原为两层状态; 蒙版/视频里的预览也会实时带上这层底色.\n\
12. 「视频」页: 上方预览窗 (悬浮显示可拖动的进度条), 下方视频/淡入淡出/音频三条轨道;\n\
    右侧素材池按「输出组合」顺序显示, 点击展开该组合的预览, 拖到视频轨道指定位置即可插入;\n\
    「导入音频」可一次导入多段按顺序播放的音频 (如各乐章分轨); 「分割音频」按下后,\n\
    在音频轨道上点一下鼠标即可把该处的音频从此切开成两段 (命名为 原名-1 / 原名-2).\n\
13. 「导出视频」弹窗: 容器选 MP4 (音频有损 AAC, 兼容性好) 或 MKV (音频无损 FLAC);\n\
    帧率可直接点击数字修改; 画质 CRF 数值越小越清晰、文件越大; 分辨率固定跟随素材图片\n\
    (加底色后统一尺寸), 无需选择. 导出进度/日志直接显示在弹窗内, 不会另外弹出终端窗口.\n\
\n\
其他:\n\
  空白双击或 F 适应窗口; 拖动画布与侧栏之间的分隔条可调宽度.\n\
  右侧顶栏可切换「分块 / 蒙版 / 工程 / 视频」四个面板.\n\
  标题栏未保存改动显示 *; 异步保存中改为转圈提示.\n\
  「工程」面板可「清除视频缓存」删除旁路 `.staffcrop.cache`.\n\
  PDF 导入依赖 pdfium、视频导出依赖 ffmpeg, 需把对应文件放在程序所在目录 (或系统 PATH) 下.";

/// 画布编辑工具 (互斥)
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum CanvasTool {
    #[default]
    Normal,
    /// 拖出新块: 首按下为锚定边, 先上/下决定上下边
    AddBlock,
    /// 在已有块内切开
    SplitBlock,
}

/// 添加新块时, 首条线扮演的角色
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddAnchorRole {
    /// 首线为上边线, 向下拖出下边
    Top,
    /// 首线为下边线, 向上拖出上边
    Bottom,
}

/// 右侧工具栏模式 (类似 PS 面板切换)
#[derive(Clone, Copy, PartialEq, Eq)]
enum SideTool {
    /// 谱表分块: 原子块 / 组合 / 成员
    Crop,
    /// 蒙版遮盖 (mask_tool)
    Mask,
    /// 工程保存 / 加底色 (apply_bg)
    Project,
    /// 视频轨道编辑与导出 (score_video)
    Video,
}

/// 分块面板一次可撤操作的快照 (按「触发时所在页」入栈; 可含多页 regions).
/// 删除/复制页等结构变更另存完整 `pages`, 走 `page_struct_history`.
#[derive(Clone)]
struct CropSnap {
    page_regions: HashMap<String, HashMap<String, Region>>,
    /// 页级结构快照; `Some` 时 apply 整表替换 pages (含图), 忽略 page_regions.
    pages: Option<Vec<Page>>,
    current_page_index: Option<usize>,
    group_masks: Option<HashMap<String, Vec<MaskRect>>>,
    groups: Vec<Group>,
    selected_region_ids: HashSet<String>,
    active_group_id: Option<String>,
    groups_manual_order: bool,
}

#[derive(Clone, Default)]
struct CropHistory {
    undo: Vec<CropSnap>,
    redo: Vec<CropSnap>,
}

enum DragKind {
    PagePan { last: Point<Pixels> },
    Edge {
        region_id: String,
        edge: &'static str,
        /// 本轮拖边是否已压入撤销栈
        undid: bool,
    },
    /// 添加新块拖拽: 锚定边 + 活动边
    AddBlock {
        anchor_y: i32,
        role: Option<AddAnchorRole>,
        cur_y: i32,
    },
    MemberReorder {
        from: usize,
        /// move 目标下标 (remove 后再 insert 的下标)
        to: usize,
        /// 提示线画在哪一项; None = 原位无反应
        line_at: Option<usize>,
        /// true = 右边/下边, false = 左边/上边
        line_after: bool,
        start_x: f32,
        start_y: f32,
        origin_x: f32,
        origin_y: f32,
        x: f32,
        y: f32,
        armed: bool,
    },
    /// 输出组合列表竖直拖拽调序 (逻辑同 MemberReorder)
    GroupReorder {
        from: usize,
        line_at: Option<usize>,
        line_after: bool,
        start_x: f32,
        start_y: f32,
        origin_x: f32,
        origin_y: f32,
        x: f32,
        y: f32,
        armed: bool,
        /// 按下时是否按住 Ctrl (松开时用于多选, 不依赖 mouse_up 修饰键)
        ctrl: bool,
    },
    TabReorder {
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
    /// 侧栏列表滚动条拖拽
    Scrollbar {
        which: ScrollList,
        grab: f32,
        vertical: bool,
    },
    /// 左右分隔条
    SideResize {
        start_x: f32,
        start_w: f32,
    },
    /// 标签栏横向滚动条
    TabHScroll {
        grab: f32,
    },
}

#[derive(Clone, Copy, Debug)]
enum ScrollList {
    Region,
    Group,
    Member,
    /// 蒙版面板: 组合选择列表
    MaskGroup,
    /// 操作说明对话框正文
    Help,
}

#[derive(Clone)]
enum DialogKind {
    Help,
    Info {
        title: String,
        body: String,
    },
    /// 关窗时有未保存改动
    UnsavedExit,
    /// 新建工程前有未保存改动
    UnsavedNew,
}

struct TabContextMenu {
    page_index: usize,
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParamEdit {
    Margin,
    Threshold,
}

enum PdfLoadMsg {
    Page {
        path: PathBuf,
        index: usize,
        total: usize,
        pdf_name: String,
    },
    Done {
        pdf_name: String,
        pages: usize,
    },
    Err {
        pdf_name: String,
        message: String,
    },
    AllFinished,
}

struct ScoreSyncApp {
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
    /// 当前蒙版预览图相对拼合图的纵向偏移 (叠加工程底色补边时非零)
    mask_preview_voff: i64,
    dialog: Option<DialogKind>,
    /// 标签右键菜单
    tab_menu: Option<TabContextMenu>,
    /// 原子块 y0-y1 行内编辑
    edit_y_input: Entity<TextInput>,
    /// 正在编辑 y 的 region id
    region_y_edit: Option<String>,
    /// 边距 / 墨迹阈值 点按编辑
    param_input: Entity<TextInput>,
    param_edit: Option<ParamEdit>,
    /// 画布悬停光标 (边缘/分割)
    hover_cursor: CursorStyle,
    region_scroll: ScrollHandle,
    group_scroll: ScrollHandle,
    member_scroll: ScrollHandle,
    mask_group_scroll: ScrollHandle,
    help_scroll: ScrollHandle,
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
    /// 视频池组合脏标记 (分块/蒙版/底色变更后需重算缓存)
    video_pool_dirty: HashSet<String>,
    /// 全部视频池视为脏 (底色整体变更等)
    video_pool_all_dirty: bool,
    /// 用户确认退出后允许关窗
    allow_close: bool,
    /// 保存中转圈动画相位 (0..1)
    save_spin_phase: f32,
}

impl ScoreSyncApp {
    fn new(cx: &mut Context<Self>, initial: Vec<PathBuf>) -> Self {
        let cfg = config::load();
        let mask_prefs = cfg.mask_prefs.clone();
        let edit_y_input = cx.new(|cx| TextInput::new(cx, "", "例如 94-371"));
        let param_input = cx.new(|cx| TextInput::new(cx, "", "数字"));
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
            hint: "拖入/打开图片、PDF 或工程. Ctrl+S 保存工程. 标签右键可复制本页."
                .into(),
            region_panel_open: false,
            side_width: SIDE_PANEL_W,
            side_tool: SideTool::Crop,
            canvas_tool: CanvasTool::Normal,
            mask_tool,
            apply_bg,
            score_video,
            mask_target: None,
            mask_preview_voff: 0,
            dialog: None,
            tab_menu: None,
            edit_y_input,
            region_y_edit: None,
            param_input,
            param_edit: None,
            hover_cursor: CursorStyle::Arrow,
            region_scroll: ScrollHandle::new(),
            group_scroll: ScrollHandle::new(),
            member_scroll: ScrollHandle::new(),
            mask_group_scroll: ScrollHandle::new(),
            help_scroll: ScrollHandle::new(),
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
            video_pool_dirty: HashSet::new(),
            video_pool_all_dirty: true,
            allow_close: false,
            save_spin_phase: 0.0,
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
        app
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

    fn reorder_slop_exceeded(dx: f32, dy: f32) -> bool {
        dx * dx + dy * dy >= REORDER_DRAG_SLOP * REORDER_DRAG_SLOP
    }

    fn measure_item_bounds(
        entity: Entity<Self>,
        key: usize,
        kind: &'static str,
    ) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    match kind {
                        "tab" => {
                            this.tab_bounds.insert(key, bounds);
                        }
                        "group" => {
                            this.group_bounds.insert(key, bounds);
                        }
                        _ => {
                            this.member_bounds.insert(key, bounds);
                        }
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    fn item_origin(bounds: Option<&Bounds<Pixels>>, mouse_x: f32, mouse_y: f32) -> (f32, f32) {
        bounds
            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
            .unwrap_or((mouse_x, mouse_y))
    }

    /// 将「落在 anchor 之前/之后」换算成 remove 后再 insert 的下标.
    fn reorder_to_index(from: usize, anchor: usize, after: bool) -> usize {
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

    /// 水平列表 (标签): 原位无反应; 左半→该项左边, 右半→该项右边.
    /// 返回 (to, line_at, line_after).
    fn resolve_tab_drop(
        &self,
        from: usize,
        x: f32,
        _y: f32,
    ) -> (usize, Option<usize>, bool) {
        let n = self.doc.pages.len();
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.tab_bounds.get(&i) else {
                continue;
            };
            let left = f32::from(b.origin.x);
            let right = left + f32::from(b.size.width);
            if x < left || x > right {
                continue;
            }
            if i == from {
                return (from, None, false);
            }
            let mid = (left + right) * 0.5;
            let after = x >= mid;
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    /// 竖直列表 (成员): 原位无反应; 上半→该项上边, 下半→该项下边.
    fn resolve_member_drop(
        &self,
        from: usize,
        _x: f32,
        y: f32,
    ) -> (usize, Option<usize>, bool) {
        let n = self.member_list_rows().len();
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.member_bounds.get(&i) else {
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
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    /// 竖直列表 (输出组合): 同成员; 多选时落点忽略移动块内其它项.
    fn resolve_group_drop(
        &self,
        from: usize,
        _x: f32,
        y: f32,
    ) -> (usize, Option<usize>, bool) {
        let n = self.doc.groups.len();
        if n == 0 {
            return (from, None, false);
        }
        let moving: HashSet<usize> = self.doc.group_move_indices(from).into_iter().collect();
        for i in 0..n {
            let Some(b) = self.group_bounds.get(&i) else {
                continue;
            };
            let top = f32::from(b.origin.y);
            let bottom = top + f32::from(b.size.height);
            if y < top || y > bottom {
                continue;
            }
            if moving.contains(&i) {
                return (from, None, false);
            }
            let mid = (top + bottom) * 0.5;
            let after = y >= mid;
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    fn screen_in_view(&self, pos: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(pos.x) - f32::from(self.view_bounds.origin.x),
            f32::from(pos.y) - f32::from(self.view_bounds.origin.y),
        )
    }

    fn refresh_render(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.doc.current_page() else {
            self.render_image = None;
            self.img_w = 0;
            self.img_h = 0;
            cx.notify();
            return;
        };
        self.img_w = page.width();
        self.img_h = page.height();
        let Some(rgb) = page.image.as_ref() else {
            // 占位: 尺寸已知但像素未到, 触发异步窗口加载
            self.render_image = None;
            self.request_page_window(cx);
            cx.notify();
            return;
        };
        let (w, h) = (self.img_w, self.img_h);
        let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
            let p = rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
        // GPUI / Windows 纹理多为 BGRA
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let frame = Frame::new(rgba);
        self.render_image = Some(Arc::new(RenderImage::new(smallvec![frame])));
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.doc.sync_group_colors();
        self.sync_mask_image(cx);
        cx.notify();
    }

    /// 异步加载当前页 ±4 窗口并释放窗外页图.
    fn request_page_window(&mut self, cx: &mut Context<Self>) {
        self.page_load_gen = self.page_load_gen.wrapping_add(1);
        let gen = self.page_load_gen;
        let center = self.doc.current_page_index;
        let radius = crate::page_cache::WINDOW_RADIUS;
        let n = self.doc.pages.len();
        if n == 0 {
            return;
        }
        let lo = center.saturating_sub(radius);
        let hi = (center + radius).min(n - 1);
        let mut jobs: Vec<(usize, PathBuf)> = Vec::new();
        for i in lo..=hi {
            if self.doc.pages[i].image.is_none() {
                jobs.push((i, self.doc.pages[i].disk_path.clone()));
            }
        }
        // 窗外立刻卸掉
        for i in 0..n {
            if i < lo || i > hi {
                self.doc.unload_page_image(i);
            }
        }
        if jobs.is_empty() {
            // 当前页已在内存则刷新贴图
            if self.doc.pages.get(center).and_then(|p| p.image.as_ref()).is_some()
                && self.render_image.is_none()
            {
                self.refresh_render(cx);
            }
            return;
        }
        let (tx, rx) = async_channel::unbounded::<(usize, Result<image::RgbImage, String>)>();
        std::thread::spawn(move || {
            for (idx, path) in jobs {
                let r = crate::page_cache::load_rgb(&path);
                let _ = tx.send_blocking((idx, r));
            }
        });
        cx.spawn(async move |this, cx| {
            while let Ok((idx, result)) = rx.recv().await {
                this.update(cx, |view, cx| {
                    if view.page_load_gen != gen {
                        return;
                    }
                    if let Ok(img) = result {
                        if let Some(page) = view.doc.pages.get_mut(idx) {
                            page.img_w = img.width();
                            page.img_h = img.height();
                            page.image = Some(img);
                        }
                    }
                    if idx == view.doc.current_page_index {
                        view.refresh_render(cx);
                    } else {
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 保存进行中时驱动标题栏拖尾转圈.
    fn start_save_spinner(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(33))
                .await;
            let cont = this
                .update(cx, |view, cx| {
                    if !view.saving {
                        return false;
                    }
                    view.save_spin_phase = (view.save_spin_phase + 0.08) % 1.0;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !cont {
                break;
            }
        })
        .detach();
    }

    /// 落盘蒙版/同步视频时间轴后, 判定是否仍有未保存改动.
    fn refresh_dirty_from_panels(&mut self, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
        }
        let video_snap = self.score_video.read(cx).timeline_snapshot();
        let saved = &self.doc.video_state;
        if video_snap.video_clips != saved.video_clips
            || video_snap.fades != saved.fades
            || video_snap.audio_clips != saved.audio_clips
        {
            self.dirty = true;
        }
    }

    fn mark_video_pool_dirty_all(&mut self) {
        self.video_pool_all_dirty = true;
        self.video_pool_dirty.clear();
        self.mark_dirty();
    }

    fn mark_video_pool_dirty_group(&mut self, gid: &str) {
        if !self.video_pool_all_dirty {
            self.video_pool_dirty.insert(gid.to_string());
        }
        self.mark_dirty();
    }

    fn pool_cache_dir(&self) -> PathBuf {
        if let Some(ref p) = self.project_path {
            crate::page_cache::project_cache_dir(p)
        } else {
            crate::page_cache::session_dir().join("pool_cache")
        }
    }

    fn sync_mask_image(&mut self, cx: &mut Context<Self>) {
        if self.side_tool != SideTool::Mask {
            return;
        }
        self.flush_mask_to_doc(cx);
        let side_w = self.side_width;
        let target = self.resolve_mask_target();
        self.mask_target = target.clone();
        let Some(gid) = target else {
            self.mask_tool.update(cx, |m, cx| {
                m.set_embed_side_width(side_w);
                m.clear_view("请先有可编辑的组合", cx);
            });
            return;
        };
        if self.doc.ensure_group_pages(&gid).is_err() {
            self.mask_tool.update(cx, |m, cx| {
                m.set_embed_side_width(side_w);
                m.clear_view("无法加载该组合页图", cx);
            });
            return;
        }
        let Some((rgb, voff)) = self.doc.compose_group_preview(&gid) else {
            self.mask_tool.update(cx, |m, cx| {
                m.set_embed_side_width(side_w);
                m.clear_view("无法拼合该组合", cx);
            });
            return;
        };
        self.mask_preview_voff = voff;
        let masks: Vec<MaskRect> = self
            .doc
            .get_group_masks(&gid)
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.offset_y(voff as i32);
                m
            })
            .collect();
        let label = self
            .doc
            .groups
            .iter()
            .position(|g| g.id == gid)
            .map(|i| self.doc.group_crop_label(i))
            .unwrap_or_else(|| "组合".into());
        let mask_prefs = self.doc.mask_prefs.clone();
        self.mask_tool.update(cx, |m, cx| {
            m.set_embed_side_width(side_w);
            m.load_rgb(rgb, gid, masks, &label, cx);
            m.apply_color_prefs(mask_prefs);
        });
    }

    fn resolve_mask_target(&self) -> Option<String> {
        if let Some(ref id) = self.doc.active_group_id {
            if self.doc.groups.iter().any(|g| &g.id == id) {
                return Some(id.clone());
            }
        }
        if let Some(ref id) = self.mask_target {
            if self.doc.groups.iter().any(|g| &g.id == id) {
                return Some(id.clone());
            }
        }
        self.doc.groups.first().map(|g| g.id.clone())
    }

    fn flush_mask_to_doc(&mut self, cx: &mut Context<Self>) {
        let Some(gid) = self.mask_target.clone() else {
            return;
        };
        let (masks, prefs) = self
            .mask_tool
            .update(cx, |m, _| (m.masks_clone(), m.color_prefs()));
        let voff = self.mask_preview_voff;
        let masks: Vec<MaskRect> = masks
            .into_iter()
            .map(|mut m| {
                m.offset_y(-(voff as i32));
                m
            })
            .collect();
        self.doc.set_group_masks(&gid, masks);
        self.doc.mask_prefs = prefs.clone();
        config::remember_mask_prefs(&prefs);
        self.mark_dirty();
        self.mark_video_pool_dirty_group(&gid);
    }

    fn set_mask_target(&mut self, group_id: String, cx: &mut Context<Self>) {
        if self.mask_target.as_ref() == Some(&group_id) {
            return;
        }
        self.flush_mask_to_doc(cx);
        self.doc.active_group_id = Some(group_id);
        self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
        self.mask_target = None;
        self.sync_mask_image(cx);
        self.scroll_mask_lists_to_active();
        cx.notify();
    }

    fn set_side_tool(&mut self, tool: SideTool, window: &mut Window, cx: &mut Context<Self>) {
        if self.side_tool == tool {
            return;
        }
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
            self.doc.retain_window(
                self.doc.current_page_index,
                crate::page_cache::WINDOW_RADIUS,
            );
        }
        self.side_tool = tool;
        match tool {
            SideTool::Crop => {
                // 回到分块: 定位到当前蒙版组合所在页并选中该组
                self.restore_crop_from_mask_target(cx);
                self.focus_handle.focus(window);
                self.status = "分块工具".into();
                self.hint = "拖入/打开图片、PDF 或工程. Ctrl+S 保存工程.".into();
            }
            SideTool::Mask => {
                self.mask_target = None;
                self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
                self.sync_mask_image(cx);
                self.scroll_mask_lists_to_active();
                self.mask_tool.read(cx).focus_handle_ref().focus(window);
                self.status = "蒙版工具".into();
                self.hint =
                    "蒙版编辑当前组合的拼合图. 标签切换组合; Ctrl+A 全选蒙版."
                        .into();
            }
            SideTool::Project => {
                self.focus_handle.focus(window);
                self.status = "工程工具".into();
                self.hint =
                    "打开/保存工程; 下方加底色可「应用到工程组合」(双层, 可取消) 或批量导出目录."
                        .into();
            }
            SideTool::Video => {
                self.sync_video_pool(cx);
                self.score_video
                    .read(cx)
                    .focus_handle_ref()
                    .clone()
                    .focus(window);
                self.status = "视频工具".into();
                self.hint =
                    "N 插入下一张组合 | 空格播放/暂停 | ←→ 快退快进 | I/O 标记淡入淡出."
                        .into();
            }
        }
        cx.notify();
    }

    /// 把「输出组合」渲染为终稿写入工程旁持久缓存, 再同步给视频素材池 (LRU 热加载).
    fn sync_video_pool(&mut self, cx: &mut Context<Self>) {
        self.video_sync_gen = self.video_sync_gen.wrapping_add(1);
        let gen = self.video_sync_gen;
        let group_ids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let (aw, ah) = (self.doc.bg_aspect_w, self.doc.bg_aspect_h);
        self.score_video.update(cx, |v, _| v.set_aspect(aw, ah));
        if group_ids.is_empty() {
            self.score_video.update(cx, |v, cx| v.set_pool(Vec::new(), cx));
            return;
        }
        let cache_root = self.pool_cache_dir().join("pool");
        let _ = std::fs::create_dir_all(&cache_root);
        let all_dirty = self.video_pool_all_dirty;
        let dirty_set = self.video_pool_dirty.clone();
        // 估算并发: 取当前页峰值近似
        let peak = self
            .doc
            .pages
            .first()
            .map(|p| p.estimated_bytes().saturating_mul(3))
            .unwrap_or(64 * 1024 * 1024);
        let conc = crate::page_cache::concurrency_for_peak(peak.max(128 * 1024 * 1024));

        cx.spawn(async move |this, cx| {
            let mut items: Vec<MaterialItem> = Vec::with_capacity(group_ids.len());
            for (chunk_i, chunk) in group_ids.chunks(conc.max(1)).enumerate() {
                if chunk_i > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                let cancelled = this
                    .update(cx, |view, _| {
                        if view.video_sync_gen != gen {
                            return true;
                        }
                        for gid in chunk {
                            let Some(idx) =
                                view.doc.groups.iter().position(|g| &g.id == gid)
                            else {
                                continue;
                            };
                            let label = view.doc.groups[idx].display_name(idx);
                            let cache_path = cache_root.join(format!("{gid}.png"));
                            let need_rebuild = all_dirty
                                || dirty_set.contains(gid)
                                || !cache_path.is_file();
                            if need_rebuild {
                                let _ = view.doc.ensure_group_pages(gid);
                                match view.doc.render_group_final(gid) {
                                    Ok(Some(rgb)) => {
                                        if rgb.save(&cache_path).is_err() {
                                            continue;
                                        }
                                        items.push(MaterialItem {
                                            group_id: gid.clone(),
                                            label: label.into(),
                                            width: rgb.width(),
                                            height: rgb.height(),
                                            cache_path,
                                        });
                                    }
                                    _ => continue,
                                }
                                view.doc.retain_window(
                                    view.doc.current_page_index,
                                    crate::page_cache::WINDOW_RADIUS,
                                );
                            } else if let Ok((w, h)) = image::image_dimensions(&cache_path) {
                                items.push(MaterialItem {
                                    group_id: gid.clone(),
                                    label: label.into(),
                                    width: w,
                                    height: h,
                                    cache_path,
                                });
                            }
                        }
                        false
                    })
                    .unwrap_or(true);
                if cancelled {
                    return;
                }
            }
            this.update(cx, |view, cx| {
                if view.video_sync_gen == gen {
                    view.video_pool_all_dirty = false;
                    view.video_pool_dirty.clear();
                    view.score_video.update(cx, |v, cx| v.set_pool(items, cx));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// 从蒙版目标恢复分块页签与选中组合.
    fn restore_crop_from_mask_target(&mut self, cx: &mut Context<Self>) {
        let gid = self
            .mask_target
            .clone()
            .or_else(|| self.doc.active_group_id.clone());
        let Some(gid) = gid else {
            return;
        };
        self.doc.active_group_id = Some(gid.clone());
        let Some(g) = self.doc.groups.iter().find(|g| g.id == gid).cloned() else {
            return;
        };
        self.doc.selected_region_ids = g.region_ids.iter().cloned().collect();
        if let Some(rid) = g.region_ids.first() {
            if let Some((pi, _)) = self.doc.find_region(rid) {
                if pi != self.doc.current_page_index {
                    self.switch_page(pi, cx);
                    self.scroll_group_list_to_active();
                    return;
                }
            }
        }
        self.scroll_group_list_to_active();
        cx.notify();
    }

    fn after_doc_change(&mut self, cx: &mut Context<Self>) {
        self.doc.sync_group_colors();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        // 若当前页尺寸变了不必重渲整图, 但区域会重绘
        if let Some(page) = self.doc.current_page() {
            if page.width() != self.img_w || page.height() != self.img_h {
                self.refresh_render(cx);
                return;
            }
        } else {
            self.render_image = None;
            self.img_w = 0;
            self.img_h = 0;
        }
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
            self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
            self.mask_target = None;
            self.sync_mask_image(cx);
        }
        cx.notify();
    }

    /// 将「输出组合」列表滚到能显示 active_group 的位置.
    fn scroll_group_list_to_active(&self) {
        let Some(gid) = self.doc.active_group_id.as_ref() else {
            return;
        };
        let Some(ix) = self.doc.groups.iter().position(|g| &g.id == gid) else {
            return;
        };
        self.group_scroll.scroll_to_item(ix);
    }

    /// 将蒙版侧「编辑目标」列表与顶部组合标签滚到 active_group.
    fn scroll_mask_lists_to_active(&self) {
        let Some(gid) = self
            .mask_target
            .as_ref()
            .or(self.doc.active_group_id.as_ref())
        else {
            return;
        };
        let Some(ix) = self.doc.groups.iter().position(|g| &g.id == gid) else {
            return;
        };
        self.mask_group_scroll.scroll_to_item(ix);
        self.tab_scroll.scroll_to_item(ix);
    }

    fn capture_crop_snap(&self, page_ids: &[String]) -> CropSnap {
        let mut page_regions = HashMap::new();
        for pid in page_ids {
            if let Some(p) = self.doc.pages.iter().find(|p| p.id == *pid) {
                page_regions.insert(pid.clone(), p.regions.clone());
            }
        }
        CropSnap {
            page_regions,
            pages: None,
            current_page_index: None,
            group_masks: None,
            groups: self.doc.groups.clone(),
            selected_region_ids: self.doc.selected_region_ids.clone(),
            active_group_id: self.doc.active_group_id.clone(),
            groups_manual_order: self.doc.groups_manual_order,
        }
    }

    fn capture_crop_snap_pages(&self) -> CropSnap {
        // 结构撤重只保留路径 + 元数据, 不克隆整幅位图
        let pages = self
            .doc
            .pages
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p.image = None;
                p
            })
            .collect();
        CropSnap {
            page_regions: HashMap::new(),
            pages: Some(pages),
            current_page_index: Some(self.doc.current_page_index),
            group_masks: Some(self.doc.group_masks.clone()),
            groups: self.doc.groups.clone(),
            selected_region_ids: self.doc.selected_region_ids.clone(),
            active_group_id: self.doc.active_group_id.clone(),
            groups_manual_order: self.doc.groups_manual_order,
        }
    }

    fn push_crop_undo_for(&mut self, page_ids: &[String]) {
        let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) else {
            return;
        };
        if page_ids.is_empty() {
            return;
        }
        let snap = self.capture_crop_snap(page_ids);
        let h = self.crop_histories.entry(cur).or_default();
        h.undo.push(snap);
        if h.undo.len() > CROP_HISTORY_LIMIT {
            h.undo.remove(0);
        }
        h.redo.clear();
    }

    fn push_crop_undo_current(&mut self) {
        let Some(id) = self.doc.current_page().map(|p| p.id.clone()) else {
            return;
        };
        self.push_crop_undo_for(&[id]);
    }

    fn push_crop_undo_all_pages(&mut self) {
        let ids: Vec<String> = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        self.push_crop_undo_for(&ids);
    }

    fn push_crop_undo_page_structure(&mut self) {
        let snap = self.capture_crop_snap_pages();
        let h = &mut self.page_struct_history;
        h.undo.push(snap);
        if h.undo.len() > CROP_HISTORY_LIMIT {
            h.undo.remove(0);
        }
        h.redo.clear();
    }

    fn apply_crop_snap(&mut self, snap: CropSnap) {
        if let Some(pages) = snap.pages {
            self.doc.pages = pages;
            if let Some(idx) = snap.current_page_index {
                self.doc.current_page_index = idx.min(self.doc.pages.len().saturating_sub(1));
            }
            if let Some(masks) = snap.group_masks {
                self.doc.group_masks = masks;
            }
            self.doc.retain_window(
                self.doc.current_page_index,
                crate::page_cache::WINDOW_RADIUS,
            );
        } else {
            for (pid, regions) in snap.page_regions {
                if let Some(p) = self.doc.pages.iter_mut().find(|p| p.id == pid) {
                    p.regions = regions;
                }
            }
        }
        self.doc.groups = snap.groups;
        self.doc.selected_region_ids = snap.selected_region_ids;
        self.doc.active_group_id = snap.active_group_id;
        self.doc.groups_manual_order = snap.groups_manual_order;
        self.doc.ensure_active_group();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
    }

    fn undo_crop(&mut self, cx: &mut Context<Self>) {
        // 1) 当前页的 regions 撤重
        if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
            if let Some(h) = self.crop_histories.get_mut(&cur) {
                if let Some(prev) = h.undo.pop() {
                    let ids: Vec<String> = prev.page_regions.keys().cloned().collect();
                    let now = self.capture_crop_snap(&ids);
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.redo.push(now);
                    }
                    self.apply_crop_snap(prev);
                    self.status = "已撤回.".into();
                    self.hint = self.status.clone();
                    self.after_doc_change(cx);
                    return;
                }
            }
        }
        // 2) 删页等结构撤重
        if let Some(prev) = self.page_struct_history.undo.pop() {
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.redo.push(now);
            self.apply_crop_snap(prev);
            self.status = "已撤回页操作.".into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
            return;
        }
        self.status = "没有可撤回的操作.".into();
        cx.notify();
    }

    fn redo_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
            if let Some(h) = self.crop_histories.get_mut(&cur) {
                if let Some(next) = h.redo.pop() {
                    let ids: Vec<String> = next.page_regions.keys().cloned().collect();
                    let now = self.capture_crop_snap(&ids);
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.push(now);
                        if h.undo.len() > CROP_HISTORY_LIMIT {
                            h.undo.remove(0);
                        }
                    }
                    self.apply_crop_snap(next);
                    self.status = "已重做.".into();
                    self.hint = self.status.clone();
                    self.after_doc_change(cx);
                    return;
                }
            }
        }
        if let Some(next) = self.page_struct_history.redo.pop() {
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.undo.push(now);
            if self.page_struct_history.undo.len() > CROP_HISTORY_LIMIT {
                self.page_struct_history.undo.remove(0);
            }
            self.apply_crop_snap(next);
            self.status = "已重做页操作.".into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
            return;
        }
        self.status = "没有可重做的操作.".into();
        cx.notify();
    }

    fn undo_action(&mut self, cx: &mut Context<Self>) {
        match self.side_tool {
            SideTool::Crop => self.undo_crop(cx),
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.undo(cx));
            }
            SideTool::Video => {
                self.score_video.update(cx, |v, cx| v.undo(cx));
            }
            SideTool::Project => {}
        }
    }

    fn redo_action(&mut self, cx: &mut Context<Self>) {
        match self.side_tool {
            SideTool::Crop => self.redo_crop(cx),
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.redo(cx));
            }
            SideTool::Video => {
                self.score_video.update(cx, |v, cx| v.redo(cx));
            }
            SideTool::Project => {}
        }
    }

    fn load_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut images = Vec::new();
        let mut pdfs = Vec::new();
        let mut projects = Vec::new();
        for path in paths {
            if is_project_path(&path) {
                projects.push(path);
            } else if is_pdf_path(&path) {
                pdfs.push(path);
            } else if is_image_path(&path) {
                images.push(path);
            } else {
                self.dialog = Some(DialogKind::Info {
                    title: "不支持".into(),
                    body: format!("无法打开: {}", path.display()),
                });
            }
        }

        // 工程文件优先单独打开 (取最后一个)
        if let Some(proj) = projects.pop() {
            self.open_project_path(proj, cx);
            if projects.is_empty() && images.is_empty() && pdfs.is_empty() {
                return;
            }
        }

        let mut added = 0usize;
        for path in images {
            match image::open(&path) {
                Ok(im) => {
                    let rgb = im.to_rgb8();
                    match self.doc.add_page(path.clone(), rgb, true) {
                        Ok(_) => {
                            added += 1;
                            self.mark_dirty();
                            self.mark_video_pool_dirty_all();
                        }
                        Err(e) => {
                            self.dialog = Some(DialogKind::Info {
                                title: "打开失败".into(),
                                body: format!("{}: {e}", path.display()),
                            });
                        }
                    }
                }
                Err(e) => {
                    self.dialog = Some(DialogKind::Info {
                        title: "打开失败".into(),
                        body: format!("{}: {e}", path.display()),
                    });
                }
            }
        }
        if added > 0 {
            self.refresh_render(cx);
            self.status = format!("已添加 {added} 页, 共 {} 页.", self.doc.pages.len()).into();
            self.hint = self.status.clone();
        }

        if !pdfs.is_empty() {
            self.start_pdf_load(pdfs, cx);
        } else {
            cx.notify();
        }
    }

    fn start_pdf_load(&mut self, pdfs: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.status = format!(
            "PDF 后台渲染中… ({})",
            pdfs.first()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("pdf")
        )
        .into();
        self.hint = self.status.clone();
        cx.notify();

        let (tx, rx) = async_channel::unbounded::<PdfLoadMsg>();
        std::thread::spawn(move || {
            for pdf in pdfs {
                let name = pdf
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("pdf")
                    .to_string();
                let result = pdf::pdf_pages_to_tmp_images_streaming(&pdf, |i, total, path| {
                    let _ = tx.send_blocking(PdfLoadMsg::Page {
                        path,
                        index: i,
                        total,
                        pdf_name: name.clone(),
                    });
                });
                match result {
                    Ok(n) => {
                        let _ = tx.send_blocking(PdfLoadMsg::Done {
                            pdf_name: name,
                            pages: n,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send_blocking(PdfLoadMsg::Err {
                            pdf_name: name,
                            message: e,
                        });
                    }
                }
            }
            let _ = tx.send_blocking(PdfLoadMsg::AllFinished);
        });

        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let stop = matches!(msg, PdfLoadMsg::AllFinished);
                this.update(cx, |view, cx| {
                    match msg {
                        PdfLoadMsg::Page {
                            path,
                            index,
                            total,
                            pdf_name,
                        } => {
                            let was_empty = view.doc.pages.is_empty();
                            let display = PathBuf::from(format!(
                                "{pdf_name}_p{:03}.png",
                                index + 1
                            ));
                            // PDF 渲染输出已是磁盘 PNG, 直接登记; 识别后 retain 窗口
                            match view.doc.add_page_from_disk(display, path.clone(), was_empty, true)
                            {
                                Ok(_) => {
                                    view.mark_dirty();
                                    view.mark_video_pool_dirty_all();
                                    if was_empty {
                                        view.refresh_render(cx);
                                    }
                                    view.status = format!(
                                        "PDF {pdf_name}: 已载入 {}/{total} 页 (共 {} 页)",
                                        index + 1,
                                        view.doc.pages.len()
                                    )
                                    .into();
                                    view.hint = view.status.clone();
                                }
                                Err(e) => {
                                    view.dialog = Some(DialogKind::Info {
                                        title: "打开失败".into(),
                                        body: format!("{}: {e}", path.display()),
                                    });
                                }
                            }
                            cx.notify();
                        }
                        PdfLoadMsg::Done { pdf_name, pages } => {
                            view.status =
                                format!("PDF {pdf_name} 完成: {pages} 页已载入.").into();
                            view.hint = view.status.clone();
                            cx.notify();
                        }
                        PdfLoadMsg::Err { pdf_name, message } => {
                            view.dialog = Some(DialogKind::Info {
                                title: "PDF 转换失败".into(),
                                body: format!("{pdf_name}\n{message}"),
                            });
                            cx.notify();
                        }
                        PdfLoadMsg::AllFinished => {}
                    }
                })
                .ok();
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    fn open_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let files = rfd::FileDialog::new()
            .set_title("打开图片 / PDF (可多选)")
            .add_filter(
                "Images / PDF",
                &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "pdf"],
            )
            .add_filter("PDF", &["pdf"])
            .add_filter("Images", &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"])
            .pick_files();
        if let Some(paths) = files {
            self.load_paths(paths, cx);
        }
    }

    fn open_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let file = rfd::FileDialog::new()
            .set_title("打开工程")
            .add_filter("Score Sync 工程", &["staffcrop"])
            .pick_file();
        if let Some(path) = file {
            self.open_project_path(path, cx);
        }
    }

    fn open_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.opening || self.saving {
            self.status = "工程读写进行中, 请稍候…".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.flush_mask_to_doc(cx);
        self.opening = true;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        self.status = format!("正在打开工程: {name}…").into();
        self.hint = self.status.clone();
        cx.notify();

        let path_bg = path.clone();
        let (tx, rx) = async_channel::bounded::<Result<DocState, String>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(project::load_project(&path_bg));
        });

        cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            this.update(cx, |view, cx| {
                view.opening = false;
                match result {
                    Ok(Ok(doc)) => {
                        let video_snap = doc.video_state.clone();
                        view.doc = doc;
                        view.project_path = Some(path.clone());
                        view.dirty = false;
                        view.video_pool_all_dirty = false;
                        view.video_pool_dirty.clear();
                        config::remember_last_project(&path);
                        view.drag = None;
                        view.dialog = None;
                        view.tab_menu = None;
                        view.param_edit = None;
                        view.region_y_edit = None;
                        view.crop_histories.clear();
                        view.page_struct_history = CropHistory::default();
                        view.side_tool = SideTool::Crop;
                        view.canvas_tool = CanvasTool::Normal;
                        view.mask_target = None;
                        view.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
                        view.score_video
                            .update(cx, |v, cx| v.load_timeline_snapshot(video_snap, cx));
                        view.user_zoomed = false;
                        view.zoom = 1.0;
                        view.pan = point(0.0, 0.0);
                        let mask_prefs = view.doc.mask_prefs.clone();
                        view.mask_tool.update(cx, |m, _| {
                            m.apply_color_prefs(mask_prefs);
                        });
                        view.refresh_render(cx);
                        view.status = format!(
                            "已打开工程: {} ({} 页, {} 组)",
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("project"),
                            view.doc.pages.len(),
                            view.doc.groups.len()
                        )
                        .into();
                        view.hint = view.status.clone();
                    }
                    Ok(Err(e)) => {
                        view.dialog = Some(DialogKind::Info {
                            title: "打开工程失败".into(),
                            body: e,
                        });
                    }
                    Err(_) => {
                        view.dialog = Some(DialogKind::Info {
                            title: "打开工程失败".into(),
                            body: "后台打开通道已关闭.".into(),
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 新建空白工程: 有未保存改动时先确认.
    fn request_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening || self.saving {
            self.status = "工程读写进行中, 请稍候…".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.refresh_dirty_from_panels(cx);
        if self.dirty {
            self.dialog = Some(DialogKind::UnsavedNew);
            cx.notify();
            return;
        }
        let _ = window;
        self.do_new_project(cx);
    }

    /// 清空当前文档/视频/蒙版状态, 回到可重新导入的空白工程.
    fn do_new_project(&mut self, cx: &mut Context<Self>) {
        let mask_prefs = self.doc.mask_prefs.clone();
        self.flush_mask_to_doc(cx);
        self.doc = DocState::new();
        self.doc.mask_prefs = mask_prefs.clone();
        self.project_path = None;
        self.dirty = false;
        self.video_pool_all_dirty = true;
        self.video_pool_dirty.clear();
        self.drag = None;
        self.dialog = None;
        self.tab_menu = None;
        self.param_edit = None;
        self.region_y_edit = None;
        self.crop_histories.clear();
        self.page_struct_history = CropHistory::default();
        self.side_tool = SideTool::Crop;
        self.canvas_tool = CanvasTool::Normal;
        self.mask_target = None;
        self.mask_tool.update(cx, |m, cx| {
            m.clear_view("", cx);
            m.apply_color_prefs(mask_prefs);
        });
        self.score_video.update(cx, |v, cx| {
            v.load_timeline_snapshot(score_video::model::TimelineSnapshot::default(), cx);
            v.set_pool(Vec::new(), cx);
        });
        self.render_image = None;
        self.img_w = 0;
        self.img_h = 0;
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.status = "已新建空白工程. 可用 Ctrl+O 导入图片/PDF.".into();
        self.hint = self.status.clone();
        cx.notify();
    }

    fn save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.project_path.clone() {
            self.save_project_to(path, cx);
        } else {
            self.save_project_as(window, cx);
        }
    }

    fn save_project_as(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.pages.is_empty() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "当前没有可保存的页面.".into(),
            });
            cx.notify();
            return;
        }
        let mut dlg = rfd::FileDialog::new()
            .set_title("保存工程")
            .add_filter("Score Sync 工程", &["staffcrop"]);
        if let Some(ref p) = self.project_path {
            if let Some(parent) = p.parent() {
                dlg = dlg.set_directory(parent);
            }
            if let Some(name) = p.file_name() {
                dlg = dlg.set_file_name(name.to_string_lossy());
            }
        } else if let Some(page) = self.doc.pages.first() {
            if let Some(stem) = page.path.file_stem().and_then(|s| s.to_str()) {
                dlg = dlg.set_file_name(format!("{stem}.staffcrop"));
            }
        }
        let Some(path) = dlg.save_file() else {
            return;
        };
        self.save_project_to(path, cx);
    }

    fn save_project_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.saving || self.opening {
            self.status = "工程读写进行中, 请稍候…".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        if self.doc.pages.is_empty() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "当前没有可保存的页面.".into(),
            });
            cx.notify();
            return;
        }
        self.flush_mask_to_doc(cx);
        self.doc.video_state = self.score_video.read(cx).timeline_snapshot();
        self.saving = true;
        self.save_spin_phase = 0.0;
        self.status = "正在保存工程…".into();
        self.hint = self.status.clone();
        cx.notify();
        self.start_save_spinner(cx);

        // 快照后放到后台流式打 zip; clone_for_save 不拷页图像素, 避免整首页一次进内存
        let doc = self.doc.clone_for_save();
        let (tx, rx) = async_channel::bounded::<Result<PathBuf, String>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(project::save_project(&doc, &path));
        });

        let quit_after = matches!(self.dialog, Some(DialogKind::UnsavedExit));
        let new_after = matches!(self.dialog, Some(DialogKind::UnsavedNew));
        cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(Ok(saved)) => {
                        view.project_path = Some(saved.clone());
                        view.dirty = false;
                        // 保存成功后对齐视频快照基准, 避免关窗误判仍脏
                        view.doc.video_state =
                            view.score_video.read(cx).timeline_snapshot();
                        config::remember_last_project(&saved);
                        view.status = format!("工程已保存: {}", saved.display()).into();
                        view.hint = view.status.clone();
                        if quit_after {
                            view.dialog = None;
                            view.allow_close = true;
                            cx.quit();
                        } else if new_after {
                            view.do_new_project(cx);
                        }
                    }
                    Ok(Err(e)) => {
                        view.dialog = Some(DialogKind::Info {
                            title: "保存工程失败".into(),
                            body: e,
                        });
                    }
                    Err(_) => {
                        view.dialog = Some(DialogKind::Info {
                            title: "保存工程失败".into(),
                            body: "后台保存通道已关闭.".into(),
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fit_to_view(&mut self, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.mask_tool.update(cx, |m, cx| m.fit_to_view(cx));
            return;
        }
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        cx.notify();
    }

    fn run_detect(&mut self, cx: &mut Context<Self>) {
        if self.doc.current_page().is_none() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "请先打开图片.".into(),
            });
            cx.notify();
            return;
        }
        self.push_crop_undo_current();
        let idx = self.doc.current_page_index;
        self.doc.detect_page(idx, true);
        let n = self.doc.pages[idx].regions.len();
        let systems = self.doc.pages[idx]
            .regions
            .values()
            .filter(|r| r.kind == "system")
            .count();
        self.status = format!("本页识别到 {n} 块 (system={systems}).").into();
        self.hint = self.status.clone();
        self.after_doc_change(cx);
    }

    fn run_detect_all(&mut self, cx: &mut Context<Self>) {
        if self.doc.pages.is_empty() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "请先打开图片.".into(),
            });
            cx.notify();
            return;
        }
        self.push_crop_undo_all_pages();
        let n = self.doc.pages.len();
        let peak = self
            .doc
            .pages
            .iter()
            .map(|p| p.estimated_bytes())
            .max()
            .unwrap_or(64 * 1024 * 1024);
        let conc = crate::page_cache::concurrency_for_peak(peak);
        self.status = format!("正在识别全部 {n} 页…").into();
        self.hint = self.status.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            for start in (0..n).step_by(conc) {
                let end = (start + conc).min(n);
                this.update(cx, |view, _| {
                    for i in start..end {
                        view.doc.detect_page(i, true);
                    }
                    view.doc.retain_window(
                        view.doc.current_page_index,
                        crate::page_cache::WINDOW_RADIUS,
                    );
                    view.status = format!("识别进度 {}/{n}…", end).into();
                    view.hint = view.status.clone();
                })
                .ok();
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
            this.update(cx, |view, cx| {
                view.mark_dirty();
                view.mark_video_pool_dirty_all();
                view.status = format!("已识别全部 {n} 页.").into();
                view.hint = view.status.clone();
                view.after_doc_change(cx);
            })
            .ok();
        })
        .detach();
    }

    fn toggle_add_block(&mut self, cx: &mut Context<Self>) {
        self.canvas_tool = if self.canvas_tool == CanvasTool::AddBlock {
            CanvasTool::Normal
        } else {
            CanvasTool::AddBlock
        };
        self.drag = None;
        self.status = if self.canvas_tool == CanvasTool::AddBlock {
            "添加新块: 按下定一边, 先上移→该边为下边线, 先下移→为上边线, 拖出后松开".into()
        } else {
            "已退出添加新块".into()
        };
        self.hint = self.status.clone();
        cx.notify();
    }

    fn toggle_split_block(&mut self, cx: &mut Context<Self>) {
        self.canvas_tool = if self.canvas_tool == CanvasTool::SplitBlock {
            CanvasTool::Normal
        } else {
            CanvasTool::SplitBlock
        };
        self.drag = None;
        self.status = if self.canvas_tool == CanvasTool::SplitBlock {
            "分割块: 在已有块内点击, 于指针位置切成上下两块".into()
        } else {
            "已退出分割块".into()
        };
        self.hint = self.status.clone();
        cx.notify();
    }

    fn add_block_preview_ys(anchor_y: i32, role: Option<AddAnchorRole>, cur_y: i32) -> (i32, i32) {
        match role {
            None => (anchor_y, anchor_y),
            Some(AddAnchorRole::Top) => (anchor_y, cur_y.max(anchor_y)),
            Some(AddAnchorRole::Bottom) => (cur_y.min(anchor_y), anchor_y),
        }
    }

    fn merge_selected(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.merge_selected() {
            Ok(n) => {
                self.status = format!("已合并 {n} 块为组合.").into();
                self.hint = self.status.clone();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.dialog = Some(DialogKind::Info {
                    title: "提示".into(),
                    body: e.into(),
                });
                cx.notify();
            }
        }
    }

    fn share_into_group(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.share_selected_into_active() {
            Ok(0) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.status = "选中块已在当前组中.".into();
                cx.notify();
            }
            Ok(n) => {
                self.status =
                    format!("已共享加入 {n} 块到当前组 (仍保留在其他组中).").into();
                self.hint = self.status.clone();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.dialog = Some(DialogKind::Info {
                    title: "提示".into(),
                    body: e.into(),
                });
                cx.notify();
            }
        }
    }

    fn ungroup_active(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.ungroup_active() {
            Ok(()) => {
                self.status = "已拆开组合.".into();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.dialog = Some(DialogKind::Info {
                    title: "提示".into(),
                    body: e.into(),
                });
                cx.notify();
            }
        }
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.mask_tool.update(cx, |m, cx| m.delete_selected(cx));
            self.flush_mask_to_doc(cx);
            cx.notify();
            return;
        }
        self.push_crop_undo_all_pages();
        let n = self.doc.delete_selected();
        if n > 0 {
            self.status = format!("已删除 {n} 块.").into();
            self.after_doc_change(cx);
        } else if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
            if let Some(h) = self.crop_histories.get_mut(&cur) {
                h.undo.pop();
            }
        }
    }

    fn reset_groups(&mut self, cx: &mut Context<Self>) {
        if self.doc.current_page().is_none() {
            return;
        }
        self.push_crop_undo_all_pages();
        self.doc.reset_current_page_groups();
        self.status = "已重置本页分组.".into();
        self.hint = self.status.clone();
        self.after_doc_change(cx);
    }

    fn export_groups_ui(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
        }
        if self.doc.groups.is_empty() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "没有可导出的内容.".into(),
            });
            cx.notify();
            return;
        }
        let Some(out) = rfd::FileDialog::new()
            .set_title("选择导出目录")
            .pick_folder()
        else {
            return;
        };
        let group_ids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let n = group_ids.len();
        let peak = self
            .doc
            .pages
            .iter()
            .map(|p| p.estimated_bytes())
            .max()
            .unwrap_or(64 * 1024 * 1024)
            .saturating_mul(2);
        let conc = crate::page_cache::concurrency_for_peak(peak);
        self.status = format!("正在导出 {n} 个组合…").into();
        self.hint = self.status.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let mut saved = 0usize;
            let mut err: Option<String> = None;
            let mut abs = 0usize;
            for (chunk_i, chunk) in group_ids.chunks(conc.max(1)).enumerate() {
                if chunk_i > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                let base = abs;
                abs += chunk.len();
                let batch = this
                    .update(cx, |view, _| {
                        match crate::export::export_groups_chunk(
                            &mut view.doc,
                            &out,
                            chunk,
                            base,
                        ) {
                            Ok(n) => {
                                saved += n;
                                view.doc.retain_window(
                                    view.doc.current_page_index,
                                    crate::page_cache::WINDOW_RADIUS,
                                );
                                view.status =
                                    format!("导出进度 {saved}/{}…", group_ids.len()).into();
                                view.hint = view.status.clone();
                                None
                            }
                            Err(e) => Some(e),
                        }
                    })
                    .unwrap_or(Some("导出任务中断".into()));
                if let Some(e) = batch {
                    err = Some(e);
                    break;
                }
            }
            this.update(cx, |view, cx| {
                view.doc.retain_window(
                    view.doc.current_page_index,
                    crate::page_cache::WINDOW_RADIUS,
                );
                match err {
                    Some(e) => {
                        view.dialog = Some(DialogKind::Info {
                            title: "导出失败".into(),
                            body: e,
                        });
                    }
                    None => {
                        view.dialog = Some(DialogKind::Info {
                            title: "完成".into(),
                            body: format!(
                                "已导出 {saved} 个组合到:\n{}\n(已按输出组合列表顺序拼接并套用各组蒙版)",
                                out.display()
                            ),
                        });
                        view.status = format!("已导出 {saved} 个组合.").into();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_help(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.dialog = Some(DialogKind::Help);
        cx.notify();
    }

    fn switch_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.doc.pages.len() {
            return;
        }
        self.doc.current_page_index = index;
        if self.doc.pages[index].image.is_some() {
            self.request_page_window(cx);
            self.refresh_render(cx);
        } else {
            self.render_image = None;
            self.img_w = self.doc.pages[index].width();
            self.img_h = self.doc.pages[index].height();
            self.request_page_window(cx);
            cx.notify();
        }
    }

    fn close_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.doc.pages.len() {
            return;
        }
        self.push_crop_undo_page_structure();
        let pid = self.doc.pages.get(index).map(|p| p.id.clone());
        if self.doc.close_page_at(index) {
            if let Some(id) = pid {
                self.crop_histories.remove(&id);
            }
            self.status = "已关闭页面 (Ctrl+Z 可撤回).".into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
        } else {
            // close 失败则丢掉刚压的空操作
            self.page_struct_history.undo.pop();
        }
    }

    fn copy_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(at) = self.doc.copy_page_at(index) {
            self.status = format!(
                "已复制第 {} 页 → 新标签 {}",
                index + 1,
                at + 1
            )
            .into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
        }
    }

    fn region_list_rows(&self) -> Vec<ListRow> {
        let Some(page) = self.doc.current_page() else {
            return Vec::new();
        };
        let pno = self.doc.page_no(&page.id);
        let mut regions: Vec<_> = page.regions.values().cloned().collect();
        regions.sort_by_key(|r| (r.y0, r.y1));
        regions
            .into_iter()
            .map(|r| ListRow {
                selected: self.doc.selected_region_ids.contains(&r.id),
                color: parse_color_hex(&r.color),
                label: r.label(Some(pno)).into(),
                id: r.id,
            })
            .collect()
    }

    fn group_list_rows(&self) -> Vec<ListRow> {
        self.doc
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let mut labels = Vec::new();
                let mut pages_in = HashSet::new();
                for rid in &g.region_ids {
                    if let Some((_, r)) = self.doc.find_region(rid) {
                        let pno = self.doc.page_no(&r.page_id);
                        pages_in.insert(pno);
                        labels.push(format!("P{pno}:{}:{}-{}", r.kind, r.y0, r.y1));
                    }
                }
                let cross = if pages_in.len() > 1 { "跨页 " } else { "" };
                let text = format!(
                    "{cross}{} | [{}]",
                    self.doc.group_crop_label(i),
                    labels.join(", ")
                );
                ListRow {
                    id: g.id.clone(),
                    label: text.into(),
                    color: 0x0f172a,
                    selected: self.doc.group_has_selected_region(g),
                }
            })
            .collect()
    }

    fn member_list_rows(&self) -> Vec<ListRow> {
        let Some(g) = self.doc.active_group() else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let r = self.doc.get_region(rid)?;
                Some(ListRow {
                    id: rid.clone(),
                    label: r.label(Some(self.doc.page_no(&r.page_id))).into(),
                    color: parse_color_hex(&r.color),
                    selected: false,
                })
            })
            .collect()
    }

    fn tab_infos(&self) -> Vec<TabInfo> {
        self.doc
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let has_sel = p
                    .regions
                    .keys()
                    .any(|rid| self.doc.selected_region_ids.contains(rid));
                let mark = if has_sel { "●" } else { "" };
                TabInfo {
                    index: i,
                    label: format!("{mark}{}:{}", i + 1, p.title()).into(),
                    active: i == self.doc.current_page_index,
                }
            })
            .collect()
    }

    fn current_regions_hitlist(&self) -> Vec<(String, i32, i32)> {
        let Some(page) = self.doc.current_page() else {
            return Vec::new();
        };
        page.regions
            .values()
            .map(|r| (r.id.clone(), r.y0, r.y1))
            .collect()
    }

    fn on_view_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        if event.button != MouseButton::Left {
            if event.button == MouseButton::Right {
                // 空白处右键无操作; 标签右键在标签栏处理
            }
            return;
        }
        let (sx, sy) = self.screen_in_view(event.position);
        let xform = self.xform();
        let (_ix, iy) = xform.screen_to_image(sx, sy);
        let ctrl = event.modifiers.control;

        if self.canvas_tool == CanvasTool::SplitBlock {
            self.push_crop_undo_current();
            let msg = self.doc.split_block_at(iy);
            self.status = msg.clone().into();
            self.hint = self.status.clone();
            if msg.contains("已在") {
                self.canvas_tool = CanvasTool::Normal;
            }
            self.after_doc_change(cx);
            return;
        }

        if self.canvas_tool == CanvasTool::AddBlock {
            let y = iy.round() as i32;
            self.drag = Some(DragKind::AddBlock {
                anchor_y: y,
                role: None,
                cur_y: y,
            });
            self.status = format!("锚定线 y={y}; 上移→下边线, 下移→上边线").into();
            cx.notify();
            return;
        }

        let regions = self.current_regions_hitlist();
        let tol = xform.edge_tol();
        if let Some((rid, edge)) = hit_edge(&regions, &self.doc.selected_region_ids, iy, tol) {
            self.doc.click_region(&rid, ctrl);
            self.scroll_group_list_to_active();
            self.drag = Some(DragKind::Edge {
                region_id: rid,
                edge,
                undid: false,
            });
            self.after_doc_change(cx);
            return;
        }
        if let Some(rid) = region_at(&regions, &self.doc.selected_region_ids, iy) {
            self.doc.click_region(&rid, ctrl);
            self.scroll_group_list_to_active();
            self.after_doc_change(cx);
            // 仍可开始平移
            self.drag = Some(DragKind::PagePan {
                last: event.position,
            });
            return;
        }
        self.doc.click_blank(ctrl);
        self.drag = Some(DragKind::PagePan {
            last: event.position,
        });
        self.after_doc_change(cx);
    }

    fn on_view_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        let (sx, sy) = self.screen_in_view(event.position);
        let xform = self.xform();
        let (_ix, iy) = xform.screen_to_image(sx, sy);

        let drag = self.drag.take();
        match drag {
            Some(DragKind::Edge {
                region_id,
                edge,
                undid,
            }) => {
                let mut undid = undid;
                if !undid {
                    self.push_crop_undo_current();
                    undid = true;
                }
                self.doc.apply_edge_drag(&region_id, edge, iy.round() as i32);
                self.drag = Some(DragKind::Edge {
                    region_id,
                    edge,
                    undid,
                });
                self.hover_cursor = CursorStyle::ResizeUpDown;
                self.after_doc_change(cx);
                return;
            }
            Some(DragKind::AddBlock {
                anchor_y,
                mut role,
                ..
            }) => {
                let cur = iy.round() as i32;
                const LOCK_PX: i32 = 2;
                if role.is_none() {
                    let dy = cur - anchor_y;
                    if dy <= -LOCK_PX {
                        role = Some(AddAnchorRole::Bottom);
                    } else if dy >= LOCK_PX {
                        role = Some(AddAnchorRole::Top);
                    }
                }
                let (y0, y1) = Self::add_block_preview_ys(anchor_y, role, cur);
                self.status = match role {
                    None => format!("锚定 y={anchor_y} (再上下移动以确定上下边)").into(),
                    Some(AddAnchorRole::Top) => {
                        format!("上边 y={y0} · 下边 y={y1} (首线=上边)").into()
                    }
                    Some(AddAnchorRole::Bottom) => {
                        format!("上边 y={y0} · 下边 y={y1} (首线=下边)").into()
                    }
                };
                self.drag = Some(DragKind::AddBlock {
                    anchor_y,
                    role,
                    cur_y: cur,
                });
                self.hover_cursor = CursorStyle::Crosshair;
                cx.notify();
                return;
            }
            Some(DragKind::PagePan { last }) => {
                let dx = f32::from(event.position.x) - f32::from(last.x);
                let dy = f32::from(event.position.y) - f32::from(last.y);
                self.pan.x += dx;
                self.pan.y += dy;
                self.user_zoomed = true;
                self.drag = Some(DragKind::PagePan {
                    last: event.position,
                });
                cx.notify();
                return;
            }
            other => {
                self.drag = other;
            }
        }

        if matches!(
            self.canvas_tool,
            CanvasTool::AddBlock | CanvasTool::SplitBlock
        ) {
            self.hover_cursor = CursorStyle::Crosshair;
        } else {
            let regions = self.current_regions_hitlist();
            let tol = xform.edge_tol();
            if hit_edge(&regions, &self.doc.selected_region_ids, iy, tol).is_some() {
                self.hover_cursor = CursorStyle::ResizeUpDown;
            } else {
                self.hover_cursor = CursorStyle::Arrow;
            }
        }
        let _ = window;
        cx.notify();
    }

    fn on_view_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        if let Some(DragKind::AddBlock {
            anchor_y,
            role,
            cur_y,
        }) = self.drag.take()
        {
            match role {
                None => {
                    self.status = "已取消添加新块 (未确定上下边方向)".into();
                    self.hint = self.status.clone();
                }
                Some(_) => {
                    let (y0, y1) = Self::add_block_preview_ys(anchor_y, role, cur_y);
                    if y1 < y0 {
                        self.status = "块高度无效, 已取消.".into();
                    } else {
                        self.push_crop_undo_current();
                        let msg = self.doc.add_manual_block(y0, y1);
                        self.status = msg.into();
                        self.hint = self.status.clone();
                        self.canvas_tool = CanvasTool::Normal;
                        self.after_doc_change(cx);
                        return;
                    }
                }
            }
            cx.notify();
            return;
        }
        if matches!(
            self.drag,
            Some(DragKind::Edge { .. }) | Some(DragKind::PagePan { .. })
        ) {
            let edged = matches!(self.drag, Some(DragKind::Edge { undid: true, .. }));
            self.drag = None;
            if edged {
                self.after_doc_change(cx);
            } else {
                cx.notify();
            }
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            return;
        }
        let delta_y = match event.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * 30.0,
        };
        if event.modifiers.control {
            let (sx, sy) = self.screen_in_view(event.position);
            let xform = self.xform();
            let (ix, iy) = xform.screen_to_image(sx, sy);
            let vw = f32::from(self.view_bounds.size.width);
            let vh = f32::from(self.view_bounds.size.height);
            let fit = if self.img_w > 0 && self.img_h > 0 {
                (vw / self.img_w as f32)
                    .min(vh / self.img_h as f32)
                    .max(0.0001)
            } else {
                1.0
            };
            let factor = if delta_y > 0.0 { 1.15 } else { 1.0 / 1.15 };
            let current_zoom = if self.user_zoomed { self.zoom } else { 1.0 };
            self.user_zoomed = true;
            self.zoom = (current_zoom * factor).clamp(0.05, 40.0);
            let new_scale = fit * self.zoom;
            self.pan.x = sx - (vw - self.img_w as f32 * new_scale) * 0.5 - ix * new_scale;
            self.pan.y = sy - (vh - self.img_h as f32 * new_scale) * 0.5 - iy * new_scale;
            cx.notify();
        } else {
            self.pan.y += delta_y;
            self.user_zoomed = true;
            cx.notify();
        }
    }

    fn on_view_double_click(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || event.click_count < 2 {
            return;
        }
        let (sx, sy) = self.screen_in_view(event.position);
        let xform = self.xform();
        let (_ix, iy) = xform.screen_to_image(sx, sy);
        let regions = self.current_regions_hitlist();
        let tol = xform.edge_tol();
        if hit_edge(&regions, &self.doc.selected_region_ids, iy, tol).is_some()
            || region_at(&regions, &self.doc.selected_region_ids, iy).is_some()
        {
            return;
        }
        self.fit_to_view(cx);
    }

    fn begin_edit_y(&mut self, rid: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.find_region(&rid).is_none() {
            return;
        }
        if self.param_edit.is_some() {
            self.apply_param_edit(window, cx);
        }
        if self.region_y_edit.as_ref() == Some(&rid) {
            return;
        }
        if self.region_y_edit.is_some() {
            self.apply_edit_y(window, cx);
        }
        let (y0, y1) = {
            let Some((_, r)) = self.doc.find_region(&rid) else {
                return;
            };
            (r.y0, r.y1)
        };
        let text = format!("{y0}-{y1}");
        self.edit_y_input.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.select_all_text(cx);
        });
        self.region_y_edit = Some(rid);
        self.edit_y_input.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn begin_param_edit(
        &mut self,
        kind: ParamEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.region_y_edit.is_some() {
            self.apply_edit_y(window, cx);
        }
        // 切换编辑字段时先提交当前值
        if self.param_edit.is_some() && self.param_edit != Some(kind) {
            self.apply_param_edit(window, cx);
        }
        let text = match kind {
            ParamEdit::Margin => self.doc.margin.to_string(),
            ParamEdit::Threshold => self.doc.ink_threshold.to_string(),
        };
        self.param_input.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.select_all_text(cx);
        });
        self.param_edit = Some(kind);
        self.param_input.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn apply_param_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.param_edit else {
            return;
        };
        let text = self.param_input.read(cx).text();
        let text = text.trim();
        if let Ok(v) = text.parse::<i32>() {
            match kind {
                ParamEdit::Margin => self.doc.margin = v.clamp(0, 80),
                ParamEdit::Threshold => self.doc.ink_threshold = v.clamp(1, 254),
            }
        }
        self.param_edit = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn cancel_param_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.param_edit.is_none() {
            return;
        }
        self.param_edit = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn apply_edit_y(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(region_id) = self.region_y_edit.take() else {
            return;
        };
        let text = self.edit_y_input.read(cx).text();
        let text = text
            .trim()
            .replace(' ', "")
            .replace(',', "-")
            .replace('–', "-");
        self.focus_handle.focus(window);
        if !text.contains('-') {
            self.status = "y 范围需为 y0-y1, 例如 94-371".into();
            cx.notify();
            return;
        }
        let mut parts = text.splitn(2, '-');
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        let (Ok(y0), Ok(y1)) = (a.parse::<i32>(), b.parse::<i32>()) else {
            self.status = "y0 / y1 必须是整数".into();
            cx.notify();
            return;
        };
        if self.doc.find_region(&region_id).is_none() {
            self.status = "未能修改该块 y 范围".into();
            cx.notify();
            return;
        }
        self.push_crop_undo_current();
        if self.doc.set_region_y(&region_id, y0, y1) {
            self.status = format!("已改 → y={}-{}", y0.min(y1), y0.max(y1)).into();
            self.after_doc_change(cx);
        } else {
            if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                if let Some(h) = self.crop_histories.get_mut(&cur) {
                    h.undo.pop();
                }
            }
            self.status = "未能修改该块 y 范围".into();
            cx.notify();
        }
    }

    fn cancel_edit_y(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.region_y_edit.is_none() {
            return;
        }
        self.region_y_edit = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn btn(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
        let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
        let hover = if active { rgb(0x1d4ed8) } else { rgb(0xcbd5e1) };
        div()
            .id(id.into())
            .px_2()
            .py_1()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(rgb(0x94a3b8))
            .text_color(fg)
            .text_sm()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(label.into())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn menu_item(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let fg = if active { rgb(0x1d4ed8) } else { rgb(0x334155) };
        div()
            .id(id.into())
            .px_2()
            .py_1()
            .text_sm()
            .text_color(fg)
            .cursor_pointer()
            .rounded_sm()
            .hover(|s| s.bg(rgb(0xe2e8f0)))
            .when(active, |d| d.bg(rgb(0xdbeafe)))
            .child(label.into())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 顶部菜单栏: 文字项横排, 非独立按钮块
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x_1()
            .w_full()
            .child(self.menu_item("open", "打开 (Ctrl+O)", false, Self::open_file, cx))
            .child(self.menu_item(
                "detect",
                "识别本页 (D)",
                false,
                |this, _, cx| this.run_detect(cx),
                cx,
            ))
            .child(self.menu_item(
                "detect_all",
                "识别全部页 (A)",
                false,
                |this, _, cx| this.run_detect_all(cx),
                cx,
            ))
            .child(self.menu_item(
                "add_block",
                "添加新块 (N)",
                self.canvas_tool == CanvasTool::AddBlock,
                |this, _, cx| this.toggle_add_block(cx),
                cx,
            ))
            .child(self.menu_item(
                "split_block",
                "分割块 (S)",
                self.canvas_tool == CanvasTool::SplitBlock,
                |this, _, cx| this.toggle_split_block(cx),
                cx,
            ))
            .child(self.menu_item(
                "merge",
                "合并组合 (M)",
                false,
                |this, _, cx| this.merge_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "ungroup",
                "拆开组合 (U)",
                false,
                |this, _, cx| this.ungroup_active(cx),
                cx,
            ))
            .child(self.menu_item(
                "share",
                "共享脚注 (G)",
                false,
                |this, _, cx| this.share_into_group(cx),
                cx,
            ))
            .child(self.menu_item(
                "del",
                "删除 (Del)",
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "export",
                "导出组合 (E)",
                false,
                Self::export_groups_ui,
                cx,
            ))
            .child(self.menu_item(
                "reset",
                "重置本页分组 (R)",
                false,
                |this, _, cx| this.reset_groups(cx),
                cx,
            ))
            .child(self.menu_item(
                "fit",
                "适应窗口 (F)",
                false,
                |this, _, cx| this.fit_to_view(cx),
                cx,
            ))
            .child(self.menu_item(
                "help",
                "操作说明 (H)",
                false,
                |this, _, cx| this.show_help(cx),
                cx,
            ))
    }

    fn tool_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let crop_on = self.side_tool == SideTool::Crop;
        let mask_on = self.side_tool == SideTool::Mask;
        let proj_on = self.side_tool == SideTool::Project;
        let video_on = self.side_tool == SideTool::Video;
        div()
            .id("tool_switcher")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .w_full()
            .bg(rgb(0xe2e8f0))
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .child(self.tool_tab("tool_crop", "分块", crop_on, SideTool::Crop, cx))
            .child(self.tool_tab("tool_mask", "蒙版", mask_on, SideTool::Mask, cx))
            .child(self.tool_tab("tool_proj", "工程", proj_on, SideTool::Project, cx))
            .child(self.tool_tab("tool_video", "视频", video_on, SideTool::Video, cx))
    }

    fn tool_tab(
        &self,
        id: &'static str,
        label: &'static str,
        active: bool,
        tool: SideTool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active {
            rgb(0x2563eb)
        } else {
            rgb(0xf8fafc)
        };
        let fg = if active {
            rgb(0xffffff)
        } else {
            rgb(0x334155)
        };
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(move |s| {
                if active {
                    s
                } else {
                    s.bg(rgb(0xf1f5f9))
                }
            })
            .child(label)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.set_side_tool(tool, window, cx);
                }),
            )
    }

    fn left_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.side_tool == SideTool::Video {
            // 视频栏不用页签, 而是预览窗 + 轨道, 占满整个左侧工作区.
            let canvas = self
                .score_video
                .update(cx, |v, cx| v.left_panel(cx))
                .into_any_element();
            return div()
                .id("left_workspace")
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(canvas)
                .into_any_element();
        }
        let canvas = match self.side_tool {
            SideTool::Crop | SideTool::Project => self.image_view(cx).into_any_element(),
            SideTool::Mask => self
                .mask_tool
                .update(cx, |m, cx| m.image_view(cx))
                .into_any_element(),
            SideTool::Video => unreachable!(),
        };
        div()
            .id("left_workspace")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(0xcbd5e1))
                    .bg(rgb(0xf8fafc))
                    .child(self.tab_bar(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .child(canvas),
            )
            .into_any_element()
    }

    fn mask_target_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_gid = self.mask_target.clone().or_else(|| self.doc.active_group_id.clone());
        let mut list = div()
            .id("mask_group_list")
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .p_1()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md();
        for (i, g) in self.doc.groups.iter().enumerate() {
            let gid = g.id.clone();
            let active = active_gid.as_ref() == Some(&gid);
            let label = self.doc.group_crop_label(i);
            let bg = if active {
                rgb(0x2563eb)
            } else {
                rgb(0xe2e8f0)
            };
            let fg = if active {
                rgb(0xffffff)
            } else {
                rgb(0x0f172a)
            };
            list = list.child(
                div()
                    .id(SharedString::from(format!("mask-g-{gid}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(bg)
                    .text_color(fg)
                    .text_xs()
                    .cursor_pointer()
                    .flex_shrink_0()
                    .child(label)
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_mask_target(gid.clone(), cx);
                        }),
                    ),
            );
        }

        div()
            .id("mask_target_picker")
            .flex_shrink_0()
            .h(px(168.))
            .max_h(px(168.))
            .px_2()
            .pt_2()
            .pb_1()
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .bg(rgb(0xf8fafc))
            .flex()
            .flex_col()
            .min_h(px(0.))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0x334155))
                    .mb_1()
                    .flex_shrink_0()
                    .child("编辑目标 (组合拼合图)"),
            )
            .child(
                self.attach_scrollbars(
                    "mask_group_scroll_wrap".into(),
                    ScrollList::MaskGroup,
                    &self.mask_group_scroll,
                    list,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            )
    }

    fn right_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.side_tool {
            SideTool::Crop => self.side_panel(cx).into_any_element(),
            SideTool::Mask => {
                let picker = self.mask_target_picker(cx).into_any_element();
                let side_w = self.side_width;
                let mask_body = self.mask_tool.update(cx, |m, cx| {
                    m.set_embed_side_width(side_w);
                    div()
                        .id("mask_right_body")
                        .w_full()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(m.side_panel(cx))
                        .into_any_element()
                });
                div()
                    .id("mask_right")
                    .w_full()
                    .h_full()
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .child(picker)
                    .child(mask_body)
                    .into_any_element()
            }
            SideTool::Project => self.project_panel(cx).into_any_element(),
            SideTool::Video => self
                .score_video
                .update(cx, |v, cx| v.right_panel(cx))
                .into_any_element(),
        };
        div()
            .id("right_workspace")
            .w(px(self.side_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(rgb(0xf1f5f9))
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.tool_switcher(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(body),
            )
    }

    fn project_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let proj_name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("(未保存)")
            .to_string();
        let bg_status: SharedString = if self.doc.bg_enabled {
            let src = self
                .doc
                .bg_source_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("bg");
            format!(
                "底色层: 已启用 {} ({}:{}) — 导出时底层合成, 未改写页图",
                src, self.doc.bg_aspect_w, self.doc.bg_aspect_h
            )
            .into()
        } else {
            "底色层: 未启用".into()
        };
        let apply_panel = self.apply_bg.update(cx, |m, cx| m.panel(cx).into_any_element());
        div()
            .id("project_panel")
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(rgb(0xf1f5f9))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(0xcbd5e1))
                    .bg(rgb(0xffffff))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("工程文件"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .child(format!("当前: {proj_name}")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(self.btn(
                                "proj_new",
                                "新建工程",
                                false,
                                |this, window, cx| this.request_new_project(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_open",
                                "打开工程",
                                false,
                                |this, window, cx| this.open_project(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_save",
                                "保存 (Ctrl+S)",
                                true,
                                |this, window, cx| this.save_project(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_save_as",
                                "另存为",
                                false,
                                |this, window, cx| this.save_project_as(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_clear_video_cache",
                                "清除视频缓存",
                                false,
                                |this, _, cx| this.clear_video_pool_cache(cx),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .mt_2()
                            .child("工程底色层"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .child(bg_status),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(self.btn(
                                "proj_bg_apply",
                                "应用到工程组合",
                                true,
                                |this, _, cx| this.apply_project_bg(cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_bg_clear",
                                "取消工程底色",
                                false,
                                |this, _, cx| this.clear_project_bg(cx),
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .id("project_apply_scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_scroll()
                    .child(apply_panel),
            )
    }

    fn clear_video_pool_cache(&mut self, cx: &mut Context<Self>) {
        let dir = self.pool_cache_dir();
        let _ = std::fs::remove_dir_all(&dir);
        self.mark_video_pool_dirty_all();
        self.score_video
            .update(cx, |v, cx| v.set_pool(Vec::new(), cx));
        self.status = format!("已清除视频缓存: {}", dir.display()).into();
        self.hint = self.status.clone();
        if self.side_tool == SideTool::Video {
            self.sync_video_pool(cx);
        }
        cx.notify();
    }

    fn apply_project_bg(&mut self, cx: &mut Context<Self>) {
        if self.doc.groups.is_empty() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "当前没有输出组合. 请先分块/合并后再应用底色层.".into(),
            });
            cx.notify();
            return;
        }
        let params = self.apply_bg.read(cx).snapshot_params(cx);
        let (path, aw, ah) = match params {
            Ok(v) => v,
            Err(e) => {
                self.dialog = Some(DialogKind::Info {
                    title: "无法应用底色".into(),
                    body: e,
                });
                cx.notify();
                return;
            }
        };
        match image::open(&path) {
            Ok(im) => {
                let rgb = im.to_rgb8();
                match self
                    .doc
                    .set_project_bg(rgb, Some(path.clone()), aw, ah)
                {
                    Ok(()) => {
                        // 试合成第一组, 尽早发现底色太小等问题
                        if let Some(gid) = self.doc.groups.first().map(|g| g.id.clone()) {
                            let _ = self.doc.ensure_group_pages(&gid);
                            if let Err(e) = self.doc.render_group_final(&gid) {
                                self.doc.clear_project_bg();
                                self.doc.retain_window(
                                    self.doc.current_page_index,
                                    crate::page_cache::WINDOW_RADIUS,
                                );
                                self.dialog = Some(DialogKind::Info {
                                    title: "底色不适用".into(),
                                    body: format!(
                                        "{e}\n已取消启用. 请换更大底色或检查谱面尺寸."
                                    ),
                                });
                                cx.notify();
                                return;
                            }
                            self.doc.retain_window(
                                self.doc.current_page_index,
                                crate::page_cache::WINDOW_RADIUS,
                            );
                        }
                        self.mark_dirty();
                        self.mark_video_pool_dirty_all();
                        self.status = format!(
                            "已为 {} 个组合启用底色层 {} ({}:{})",
                            self.doc.groups.len(),
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("bg"),
                            aw,
                            ah
                        )
                        .into();
                        self.hint = self.status.clone();
                        self.force_refresh_mask_preview(cx);
                        self.sync_video_pool(cx);
                        cx.notify();
                    }
                    Err(e) => {
                        self.dialog = Some(DialogKind::Info {
                            title: "无法应用底色".into(),
                            body: e,
                        });
                        cx.notify();
                    }
                }
            }
            Err(e) => {
                self.dialog = Some(DialogKind::Info {
                    title: "无法打开底色".into(),
                    body: e.to_string(),
                });
                cx.notify();
            }
        }
    }

    fn clear_project_bg(&mut self, cx: &mut Context<Self>) {
        if !self.doc.bg_enabled && self.doc.bg_image.is_none() {
            self.status = "当前未启用工程底色层.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.doc.clear_project_bg();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        self.status = "已取消工程底色层.".into();
        self.hint = self.status.clone();
        self.force_refresh_mask_preview(cx);
        self.sync_video_pool(cx);
        cx.notify();
    }

    /// 强制重新拼合并加载蒙版预览图 (绕过 `load_rgb` 的 session_key 缓存),
    /// 用于底色启用/取消后需要刷新预览的场景. 会先落盘当前蒙版编辑, 再清空
    /// 内嵌工具视图, 避免清空动作把待落盘的蒙版一并清没.
    fn force_refresh_mask_preview(&mut self, cx: &mut Context<Self>) {
        self.flush_mask_to_doc(cx);
        self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
        self.mask_target = None;
        self.sync_mask_image(cx);
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.side_tool == SideTool::Mask {
            self.mask_group_tab_bar(cx).into_any_element()
        } else {
            self.page_tab_bar(cx).into_any_element()
        }
    }

    /// 蒙版模式标签: 各组合 (含所属页提示).
    fn mask_group_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_gid = self
            .mask_target
            .clone()
            .or_else(|| self.doc.active_group_id.clone());
        let handle = &self.tab_scroll;
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_h = max_x > 1.0 && track_w > 1.0;

        let mut row = div()
            .id("mask_tab_bar_row")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .overflow_x_scroll()
            .track_scroll(handle)
            .scrollbar_width(px(0.));
        for (i, g) in self.doc.groups.iter().enumerate() {
            let gid = g.id.clone();
            let active = active_gid.as_ref() == Some(&gid);
            let label = self.doc.group_crop_label(i);
            let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
            let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
            row = row.child(
                div()
                    .id(SharedString::from(format!("mask-tab-{gid}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_sm()
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .child(label)
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_mask_target(gid.clone(), cx);
                        }),
                    ),
            );
        }

        let mut wrap = div()
            .id("tab_bar")
            .flex()
            .flex_col()
            .w_full()
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .bg(rgb(0xf8fafc))
            .child(row);
        if show_h {
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let off_x = -f32::from(handle.offset().x);
            let frac = (off_x / max_x).clamp(0.0, 1.0);
            let thumb_left = frac * travel;
            wrap = wrap.child(
                div()
                    .id("mask_tab_htrack")
                    .h(px(8.))
                    .w_full()
                    .relative()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let handle = this.tab_scroll.clone();
                            let b = handle.bounds();
                            let tw = f32::from(b.size.width).max(1.0);
                            let max = f32::from(handle.max_offset().width);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((tw * tw) / (tw + max)).clamp(24.0, tw);
                            let travel = (tw - thumb).max(1.0);
                            let track_left = f32::from(b.origin.x);
                            let target = (x - track_left - thumb * 0.5).clamp(0.0, travel);
                            handle.set_offset(point(px(-(target / travel) * max), px(0.)));
                            this.drag = Some(DragKind::TabHScroll {
                                grab: thumb * 0.5,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id("mask_tab_hthumb")
                            .absolute()
                            .top_0()
                            .left(px(thumb_left))
                            .h_full()
                            .w(px(thumb_w))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let x = f32::from(ev.position.x);
                                    let handle = this.tab_scroll.clone();
                                    let b = handle.bounds();
                                    let tw = f32::from(b.size.width).max(1.0);
                                    let max = f32::from(handle.max_offset().width);
                                    let thumb = if max > 0.5 {
                                        ((tw * tw) / (tw + max)).clamp(24.0, tw)
                                    } else {
                                        tw
                                    };
                                    let travel = (tw - thumb).max(1.0);
                                    let track_left = f32::from(b.origin.x);
                                    let off = -f32::from(handle.offset().x);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_left = track_left + frac * travel;
                                    this.drag = Some(DragKind::TabHScroll {
                                        grab: (x - cur_left).clamp(0.0, thumb),
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }
        wrap
    }

    fn page_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tab_infos();
        let handle = &self.tab_scroll;
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_h = max_x > 1.0 && track_w > 1.0;
        let drag_from = match &self.drag {
            Some(DragKind::TabReorder {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };
        let (line_at, line_after) = match &self.drag {
            Some(DragKind::TabReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };

        let mut row = div()
            .id("tab_bar_row")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .overflow_x_scroll()
            .track_scroll(handle)
            .scrollbar_width(px(0.));
        for tab in tabs {
            let idx = tab.index;
            let active = tab.active;
            let dragging = drag_from == Some(idx);
            let show_line = line_at == Some(idx);
            let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
            let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
            row = row.child(
                div()
                    .id(SharedString::from(format!("tab-{idx}")))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_sm()
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !line_after, |d| {
                        d.border_l_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && line_after, |d| {
                        d.border_r_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "tab"))
                    .child(
                        div()
                            .child(tab.label.clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    if ev.click_count >= 1 {
                                        this.switch_page(idx, cx);
                                    }
                                    let mx = f32::from(ev.position.x);
                                    let my = f32::from(ev.position.y);
                                    let (ox, oy) = Self::item_origin(
                                        this.tab_bounds.get(&idx),
                                        mx,
                                        my,
                                    );
                                    this.drag = Some(DragKind::TabReorder {
                                        from: idx,
                                        to: idx,
                                        line_at: None,
                                        line_after: false,
                                        start_x: mx,
                                        start_y: my,
                                        origin_x: ox,
                                        origin_y: oy,
                                        x: mx,
                                        y: my,
                                        armed: false,
                                    });
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                                    this.tab_menu = Some(TabContextMenu {
                                        page_index: idx,
                                        x: f32::from(ev.position.x),
                                        y: f32::from(ev.position.y),
                                    });
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close-{idx}")))
                            .px_1()
                            .rounded_sm()
                            .hover(|s| s.bg(rgb(0x94a3b8)))
                            .child("×")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    // 拖拽排序松手落在叉上时不关页
                                    if matches!(this.drag, Some(DragKind::TabReorder { .. })) {
                                        return;
                                    }
                                    this.close_page(idx, cx);
                                }),
                            ),
                    ),
            );
        }
        row = row.child(
            div()
                .id("tab-add")
                .px_2()
                .py_1()
                .rounded_md()
                .bg(rgb(0xcbd5e1))
                .cursor_pointer()
                .flex_shrink_0()
                .child("+")
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.open_file(window, cx)),
                ),
        );
        row = row
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(DragKind::TabReorder {
                    from,
                    start_x,
                    start_y,
                    origin_x,
                    origin_y,
                    mut armed,
                    ..
                }) = this.drag.take()
                {
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                        armed = true;
                    }
                    let (to, line_at, line_after) = if armed {
                        this.resolve_tab_drop(from, x, y)
                    } else {
                        (from, None, false)
                    };
                    this.drag = Some(DragKind::TabReorder {
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
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(DragKind::TabReorder {
                        from, to, armed, ..
                    }) = this.drag.take()
                    {
                        if armed && from != to {
                            this.push_crop_undo_all_pages();
                            this.doc.move_page(from, to);
                            this.after_doc_change(cx);
                        } else {
                            cx.notify();
                        }
                    }
                }),
            );

        let mut wrap = div()
            .id("tab_bar")
            .flex()
            .flex_col()
            .w_full()
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .bg(rgb(0xf8fafc))
            .child(row);

        if show_h {
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let off_x = -f32::from(handle.offset().x);
            let frac = (off_x / max_x).clamp(0.0, 1.0);
            let thumb_left = frac * travel;
            wrap = wrap.child(
                div()
                    .id("tab_htrack")
                    .h(px(8.))
                    .w_full()
                    .relative()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let handle = this.tab_scroll.clone();
                            let b = handle.bounds();
                            let tw = f32::from(b.size.width).max(1.0);
                            let max = f32::from(handle.max_offset().width);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((tw * tw) / (tw + max)).clamp(24.0, tw);
                            let travel = (tw - thumb).max(1.0);
                            let track_left = f32::from(b.origin.x);
                            let target = (x - track_left - thumb * 0.5).clamp(0.0, travel);
                            handle.set_offset(point(px(-(target / travel) * max), px(0.)));
                            this.drag = Some(DragKind::TabHScroll {
                                grab: thumb * 0.5,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id("tab_hthumb")
                            .absolute()
                            .top_0()
                            .left(px(thumb_left))
                            .h_full()
                            .w(px(thumb_w))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let x = f32::from(ev.position.x);
                                    let handle = this.tab_scroll.clone();
                                    let b = handle.bounds();
                                    let tw = f32::from(b.size.width).max(1.0);
                                    let max = f32::from(handle.max_offset().width);
                                    let thumb = if max > 0.5 {
                                        ((tw * tw) / (tw + max)).clamp(24.0, tw)
                                    } else {
                                        tw
                                    };
                                    let travel = (tw - thumb).max(1.0);
                                    let track_left = f32::from(b.origin.x);
                                    let off = -f32::from(handle.offset().x);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_left = track_left + frac * travel;
                                    this.drag = Some(DragKind::TabHScroll {
                                        grab: (x - cur_left).clamp(0.0, thumb),
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }
        wrap
    }

    fn tab_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::TabReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let label = self
            .tab_infos()
            .get(*from)
            .map(|t| t.label.clone())
            .unwrap_or_else(|| "...".into());
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        div()
            .id("tab-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x2563eb))
            .text_color(rgb(0xffffff))
            .text_sm()
            .border_1()
            .border_color(rgb(0x1e40af))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    fn member_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::MemberReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let rows = self.member_list_rows();
        let (label, color) = rows
            .get(*from)
            .map(|r| (r.label.clone(), r.color))
            .unwrap_or_else(|| ("...".into(), 0x0f172a));
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        div()
            .id("member-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0xffffff))
            .text_color(rgb(color))
            .text_sm()
            .border_1()
            .border_color(rgb(0x94a3b8))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    fn group_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::GroupReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let rows = self.group_list_rows();
        let label = rows
            .get(*from)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "...".into());
        let gx = *origin_x + (*x - *start_x);
        let gy = *origin_y + (*y - *start_y);
        div()
            .id("group-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x0f172a))
            .text_xs()
            .border_1()
            .border_color(rgb(0x94a3b8))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let render_image = self.render_image.clone();
        let regions: Vec<(String, i32, i32, u32, bool)> = self
            .doc
            .current_page()
            .map(|page| {
                page.regions
                    .values()
                    .map(|r| {
                        (
                            r.id.clone(),
                            r.y0,
                            r.y1,
                            parse_color_hex(&r.color),
                            self.doc.selected_region_ids.contains(&r.id),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let img_w = self.img_w;
        let img_h = self.img_h;
        let zoom = self.zoom;
        let pan = self.pan;
        let user_zoomed = self.user_zoomed;
        let cursor = if matches!(
            self.canvas_tool,
            CanvasTool::AddBlock | CanvasTool::SplitBlock
        ) {
            CursorStyle::Crosshair
        } else {
            self.hover_cursor
        };
        let add_preview = match &self.drag {
            Some(DragKind::AddBlock {
                anchor_y,
                role,
                cur_y,
            }) => Some(Self::add_block_preview_ys(*anchor_y, *role, *cur_y)),
            _ => None,
        };

        let loading = render_image.is_none() && !self.doc.pages.is_empty();

        div()
            .id("image_view")
            .flex_1()
            .min_w(px(200.))
            .min_w_0()
            .h_full()
            .bg(rgb(0x2b2b2b))
            .overflow_hidden()
            .relative()
            .cursor(cursor)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if ev.click_count >= 2 {
                        this.on_view_double_click(ev, cx);
                    } else {
                        this.on_view_mouse_down(ev, window, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths
                    .paths()
                    .iter()
                    .filter(|p| is_open_path(p) || is_project_path(p))
                    .cloned()
                    .collect();
                if !list.is_empty() {
                    this.load_paths(list, cx);
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

                        if let Some(ref img) = render_image {
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
                                gpui::Corners::default(),
                                img.clone(),
                                0,
                                false,
                            );
                        }

                        let mut sorted = regions.clone();
                        sorted.sort_by_key(|(_, _, _, _, sel)| if *sel { 1 } else { 0 });
                        for (_id, y0, y1, color, selected) in &sorted {
                            let mut b = xform.image_rect_to_screen(
                                0,
                                *y0,
                                img_w.saturating_sub(1) as i32,
                                *y1,
                            );
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill = rgb(*color);
                            fill.a = if *selected { 0.38 } else { 0.18 };
                            // 与蒙版选中一致: 红色粗边框, 更醒目
                            let border = if *selected {
                                rgb(0xdc5050)
                            } else {
                                rgb(*color)
                            };
                            let bw = if *selected { px(2.) } else { px(1.) };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill,
                                bw,
                                border,
                                Default::default(),
                            ));
                        }

                        if let Some((py0, py1)) = add_preview {
                            let mut b = xform.image_rect_to_screen(
                                0,
                                py0,
                                img_w.saturating_sub(1) as i32,
                                py1,
                            );
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill = rgb(0xf59e0b);
                            fill.a = 0.28;
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill,
                                px(2.),
                                rgb(0xf59e0b),
                                Default::default(),
                            ));
                            // 锚定/活动边细线
                            for ly in [py0, py1] {
                                let mut lb = xform.image_rect_to_screen(
                                    0,
                                    ly,
                                    img_w.saturating_sub(1) as i32,
                                    ly,
                                );
                                lb.origin.x = bounds.origin.x + lb.origin.x;
                                lb.origin.y = bounds.origin.y + lb.origin.y;
                                lb.size.height = px(2.).max(lb.size.height);
                                window.paint_quad(quad(
                                    lb,
                                    px(0.),
                                    rgb(0xea580c),
                                    px(0.),
                                    rgb(0xea580c),
                                    Default::default(),
                                ));
                            }
                        }
                    },
                )
                .size_full(),
            )
            .when(loading, |d| {
                d.child(
                    div()
                        .id("page_loading")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0xe2e8f0))
                        .text_sm()
                        .child("加载中…"),
                )
            })
    }

    fn scroll_handle(&self, which: ScrollList) -> &ScrollHandle {
        match which {
            ScrollList::Region => &self.region_scroll,
            ScrollList::Group => &self.group_scroll,
            ScrollList::Member => &self.member_scroll,
            ScrollList::MaskGroup => &self.mask_group_scroll,
            ScrollList::Help => &self.help_scroll,
        }
    }

    fn apply_scrollbar_drag(&mut self, mouse_x: f32, mouse_y: f32, cx: &mut Context<Self>) {
        let Some(DragKind::Scrollbar {
            which,
            grab,
            vertical,
        }) = self.drag
        else {
            return;
        };
        let handle = self.scroll_handle(which).clone();
        let bounds = handle.bounds();
        if vertical {
            let max_y = f32::from(handle.max_offset().height);
            if max_y <= 0.5 {
                return;
            }
            let track_h = f32::from(bounds.size.height).max(1.0);
            let track_top = f32::from(bounds.origin.y);
            let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
            let travel = (track_h - thumb_h).max(1.0);
            let thumb_top = (mouse_y - grab - track_top).clamp(0.0, travel);
            let frac = thumb_top / travel;
            let ox = handle.offset().x;
            handle.set_offset(point(ox, px(-frac * max_y)));
        } else {
            let max_x = f32::from(handle.max_offset().width);
            if max_x <= 0.5 {
                return;
            }
            let track_w = f32::from(bounds.size.width).max(1.0);
            let track_left = f32::from(bounds.origin.x);
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let thumb_left = (mouse_x - grab - track_left).clamp(0.0, travel);
            let frac = thumb_left / travel;
            let oy = handle.offset().y;
            handle.set_offset(point(px(-frac * max_x), oy));
        }
        cx.notify();
    }

    fn apply_side_resize(&mut self, mouse_x: f32, cx: &mut Context<Self>) {
        let Some(DragKind::SideResize { start_x, start_w }) = self.drag else {
            return;
        };
        // 分隔条在侧栏左侧: 向左拖 → 侧栏变宽
        let new_w = (start_w + (start_x - mouse_x)).clamp(SIDE_PANEL_MIN, SIDE_PANEL_MAX);
        if (new_w - self.side_width).abs() > 0.5 {
            self.side_width = new_w;
            self.mask_tool.update(cx, |m, _| {
                m.set_embed_side_width(new_w);
            });
            cx.notify();
        }
    }

    /// 列表滚动: 仅内容溢出时显示滚动条; 支持纵向 + 横向.
    fn attach_scrollbars(
        &self,
        wrap_id: SharedString,
        which: ScrollList,
        handle: &ScrollHandle,
        mut list: Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let max_y = f32::from(handle.max_offset().height);
        let max_x = f32::from(handle.max_offset().width);
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        let track_w = f32::from(bounds.size.width).max(1.0);
        let show_v = max_y > 1.0 && track_h > 1.0;
        let show_h = max_x > 1.0 && track_w > 1.0;

        list = list
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_scroll()
            .track_scroll(handle)
            .scrollbar_width(px(0.));

        let mut row = div()
            .id(SharedString::from(format!("{wrap_id}-row")))
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(list);

        if show_v {
            let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
            let travel = (track_h - thumb_h).max(1.0);
            let off_y = -f32::from(handle.offset().y);
            let frac = (off_y / max_y).clamp(0.0, 1.0);
            let thumb_top = frac * travel;
            row = row.child(
                div()
                    .id(SharedString::from(format!("{wrap_id}-vtrack")))
                    .w(px(10.))
                    .h_full()
                    .flex_shrink_0()
                    .relative()
                    .rounded_sm()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let y = f32::from(ev.position.y);
                            let handle = this.scroll_handle(which).clone();
                            let b = handle.bounds();
                            let th = f32::from(b.size.height).max(1.0);
                            let max = f32::from(handle.max_offset().height);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((th * th) / (th + max)).clamp(24.0, th);
                            let travel = (th - thumb).max(1.0);
                            let track_top = f32::from(b.origin.y);
                            let target = (y - track_top - thumb * 0.5).clamp(0.0, travel);
                            let ox = handle.offset().x;
                            handle.set_offset(point(ox, px(-(target / travel) * max)));
                            this.drag = Some(DragKind::Scrollbar {
                                which,
                                grab: thumb * 0.5,
                                vertical: true,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{wrap_id}-vthumb")))
                            .absolute()
                            .left_0()
                            .top(px(thumb_top))
                            .w_full()
                            .h(px(thumb_h))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .hover(|s| s.bg(rgb(0x64748b)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let y = f32::from(ev.position.y);
                                    let handle = this.scroll_handle(which).clone();
                                    let b = handle.bounds();
                                    let th = f32::from(b.size.height).max(1.0);
                                    let max = f32::from(handle.max_offset().height);
                                    let thumb = if max > 0.5 {
                                        ((th * th) / (th + max)).clamp(24.0, th)
                                    } else {
                                        th
                                    };
                                    let travel = (th - thumb).max(1.0);
                                    let track_top = f32::from(b.origin.y);
                                    let off = -f32::from(handle.offset().y);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_top = track_top + frac * travel;
                                    this.drag = Some(DragKind::Scrollbar {
                                        which,
                                        grab: (y - cur_top).clamp(0.0, thumb),
                                        vertical: true,
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }

        let mut wrap = div()
            .id(wrap_id.clone())
            .relative()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(row);

        if show_h {
            let thumb_w = ((track_w * track_w) / (track_w + max_x)).clamp(24.0, track_w);
            let travel = (track_w - thumb_w).max(1.0);
            let off_x = -f32::from(handle.offset().x);
            let frac = (off_x / max_x).clamp(0.0, 1.0);
            let thumb_left = frac * travel;
            wrap = wrap.child(
                div()
                    .id(SharedString::from(format!("{wrap_id}-htrack")))
                    .h(px(10.))
                    .w_full()
                    .flex_shrink_0()
                    .relative()
                    .rounded_sm()
                    .bg(rgb(0xe2e8f0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let handle = this.scroll_handle(which).clone();
                            let b = handle.bounds();
                            let tw = f32::from(b.size.width).max(1.0);
                            let max = f32::from(handle.max_offset().width);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((tw * tw) / (tw + max)).clamp(24.0, tw);
                            let travel = (tw - thumb).max(1.0);
                            let track_left = f32::from(b.origin.x);
                            let target = (x - track_left - thumb * 0.5).clamp(0.0, travel);
                            let oy = handle.offset().y;
                            handle.set_offset(point(px(-(target / travel) * max), oy));
                            this.drag = Some(DragKind::Scrollbar {
                                which,
                                grab: thumb * 0.5,
                                vertical: false,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{wrap_id}-hthumb")))
                            .absolute()
                            .top_0()
                            .left(px(thumb_left))
                            .h_full()
                            .w(px(thumb_w))
                            .rounded_sm()
                            .bg(rgb(0x94a3b8))
                            .hover(|s| s.bg(rgb(0x64748b)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let x = f32::from(ev.position.x);
                                    let handle = this.scroll_handle(which).clone();
                                    let b = handle.bounds();
                                    let tw = f32::from(b.size.width).max(1.0);
                                    let max = f32::from(handle.max_offset().width);
                                    let thumb = if max > 0.5 {
                                        ((tw * tw) / (tw + max)).clamp(24.0, tw)
                                    } else {
                                        tw
                                    };
                                    let travel = (tw - thumb).max(1.0);
                                    let track_left = f32::from(b.origin.x);
                                    let off = -f32::from(handle.offset().x);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_left = track_left + frac * travel;
                                    this.drag = Some(DragKind::Scrollbar {
                                        which,
                                        grab: (x - cur_left).clamp(0.0, thumb),
                                        vertical: false,
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }

        wrap
    }

    fn side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let region_rows = self.region_list_rows();
        let group_rows = self.group_list_rows();
        let member_rows = self.member_list_rows();
        let region_open = self.region_panel_open;
        let margin = self.doc.margin;
        let thr = self.doc.ink_threshold;

        let mut panel = div()
            .id("side")
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(0xf1f5f9))
            .child(
                div()
                    .id("region_fold")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if region_open { "▼" } else { "▶" })
                    .child(if region_open {
                        "本页原子块 (点击 y 范围可编辑)"
                    } else {
                        "本页原子块 (折叠; 展开后可点 y 编辑)"
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            // 用 mouse_down: 别处拖拽后在标题上松开不应误触发展开/折叠
                            if this.drag.is_some() {
                                return;
                            }
                            this.region_panel_open = !this.region_panel_open;
                            cx.notify();
                        }),
                    ),
            );

        if region_open {
            let edit_y_input = self.edit_y_input.clone();
            let editing_rid = self.region_y_edit.clone();
            let mut list = div()
                .id("region_list")
                .flex()
                .flex_col()
                .gap_1()
                .border_1()
                .border_color(rgb(0xcbd5e1))
                .rounded_md()
                .p_1()
                .bg(rgb(0xffffff));
            for row in region_rows {
                let rid = row.id.clone();
                let rid_sel = row.id.clone();
                let rid_edit = row.id.clone();
                let editing = editing_rid.as_ref() == Some(&row.id);
                let bg = if row.selected {
                    rgb(0xdbeafe)
                } else {
                    rgb(0xffffff)
                };
                // 当前页原子块: 拆出可点编辑的 y 范围
                let pno = self
                    .doc
                    .current_page()
                    .map(|p| self.doc.page_no(&p.id))
                    .unwrap_or(1);
                let (y0, y1, kind) = self
                    .doc
                    .find_region(&row.id)
                    .map(|(_, r)| (r.y0, r.y1, r.kind.clone()))
                    .unwrap_or((0, 0, String::new()));
                let h = y1 - y0 + 1;
                let kind_pfx = format!("P{pno} {kind}  ");
                let y_label = format!("y={y0}-{y1}");
                let h_label = format!("  h={h}");
                list = list.child(
                    div()
                        .id(SharedString::from(format!("reg-{rid}")))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(bg)
                        .text_sm()
                        .text_color(rgb(row.color))
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .cursor_pointer()
                                .whitespace_nowrap()
                                .child(kind_pfx)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        if this.region_y_edit.is_some() {
                                            this.apply_edit_y(window, cx);
                                        }
                                        this.doc.click_region(&rid_sel, ev.modifiers.control);
                                        this.scroll_group_list_to_active();
                                        this.after_doc_change(cx);
                                    }),
                                ),
                        )
                        .child(if editing {
                            div()
                                .id(SharedString::from(format!("reg-y-edit-{rid}")))
                                .w(px(110.))
                                .h(px(24.))
                                .flex_shrink_0()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _, _, cx| {
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(edit_y_input.clone())
                                .into_any_element()
                        } else {
                            div()
                                .id(SharedString::from(format!("reg-y-{rid}")))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .px_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0xe2e8f0)).rounded_sm())
                                .child(y_label)
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.begin_edit_y(rid_edit.clone(), window, cx);
                                    }),
                                )
                                .into_any_element()
                        })
                        .child(
                            div()
                                .cursor_pointer()
                                .whitespace_nowrap()
                                .child(h_label)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        if this.region_y_edit.is_some() {
                                            this.apply_edit_y(window, cx);
                                        }
                                        this.doc.click_region(&rid, ev.modifiers.control);
                                        this.scroll_group_list_to_active();
                                        this.after_doc_change(cx);
                                    }),
                                ),
                        ),
                );
            }
            panel = panel.child(
                self.attach_scrollbars(
                    "region_scroll_wrap".into(),
                    ScrollList::Region,
                    &self.region_scroll,
                    list,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            );
        }

        panel = panel
            .child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("输出组合 (排序号. p页c页内; 拖拽调序; 导出按此顺序)"),
            );

        let mut glist = div()
            .id("group_list")
            .flex()
            .flex_col()
            .gap_1()
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .p_1()
            .bg(rgb(0xffffff));
        let group_moving: HashSet<usize> = match &self.drag {
            Some(DragKind::GroupReorder {
                from, armed: true, ..
            }) => self.doc.group_move_indices(*from).into_iter().collect(),
            _ => HashSet::new(),
        };
        let (group_line_at, group_line_after) = match &self.drag {
            Some(DragKind::GroupReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };
        for (i, row) in group_rows.iter().enumerate() {
            let idx = i;
            let gid = row.id.clone();
            let dragging = group_moving.contains(&idx);
            let show_line = group_line_at == Some(idx);
            let bg = if row.selected {
                rgb(0xdbeafe)
            } else {
                rgb(0xffffff)
            };
            glist = glist.child(
                div()
                    .id(SharedString::from(format!("grp-{gid}")))
                    .relative()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(bg)
                    .text_xs()
                    .text_color(rgb(0x0f172a))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !group_line_after, |d| {
                        d.border_t_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && group_line_after, |d| {
                        d.border_b_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "group"))
                    .child(row.label.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) =
                                Self::item_origin(this.group_bounds.get(&idx), mx, my);
                            this.drag = Some(DragKind::GroupReorder {
                                from: idx,
                                line_at: None,
                                line_after: false,
                                start_x: mx,
                                start_y: my,
                                origin_x: ox,
                                origin_y: oy,
                                x: mx,
                                y: my,
                                armed: false,
                                ctrl: ev.modifiers.control,
                            });
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if let Some(DragKind::GroupReorder {
                            from,
                            start_x,
                            start_y,
                            origin_x,
                            origin_y,
                            mut armed,
                            ctrl,
                            ..
                        }) = this.drag.take()
                        {
                            let x = f32::from(ev.position.x);
                            let y = f32::from(ev.position.y);
                            if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                                armed = true;
                            }
                            let (_to, line_at, line_after) = if armed {
                                this.resolve_group_drop(from, x, y)
                            } else {
                                (from, None, false)
                            };
                            this.drag = Some(DragKind::GroupReorder {
                                from,
                                line_at,
                                line_after,
                                start_x,
                                start_y,
                                origin_x,
                                origin_y,
                                x,
                                y,
                                armed,
                                ctrl,
                            });
                            cx.notify();
                        }
                    })),
            );
        }
        panel = panel
            .child(
                self.attach_scrollbars(
                    "group_scroll_wrap".into(),
                    ScrollList::Group,
                    &self.group_scroll,
                    glist,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            )
            .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("当前组合内成员 (拖拽调序; 可含多页)"),
        );

        let mut mlist = div()
            .id("member_list")
            .flex()
            .flex_col()
            .gap_1()
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .p_1()
            .bg(rgb(0xffffff));
        let drag_from = match &self.drag {
            Some(DragKind::MemberReorder {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };
        let (line_at, line_after) = match &self.drag {
            Some(DragKind::MemberReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };
        for (i, row) in member_rows.iter().enumerate() {
            let idx = i;
            let rid = row.id.clone();
            let dragging = drag_from == Some(idx);
            let show_line = line_at == Some(idx);
            mlist = mlist.child(
                div()
                    .id(SharedString::from(format!("mem-{rid}")))
                    .relative()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(0xffffff))
                    .text_sm()
                    .text_color(rgb(row.color))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when(dragging, |d| d.opacity(0.35))
                    .when(show_line && !line_after, |d| {
                        d.border_t_2().border_color(rgb(0xf59e0b))
                    })
                    .when(show_line && line_after, |d| {
                        d.border_b_2().border_color(rgb(0xf59e0b))
                    })
                    .child(Self::measure_item_bounds(cx.entity(), idx, "member"))
                    .child(row.label.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let mx = f32::from(ev.position.x);
                            let my = f32::from(ev.position.y);
                            let (ox, oy) =
                                Self::item_origin(this.member_bounds.get(&idx), mx, my);
                            this.drag = Some(DragKind::MemberReorder {
                                from: idx,
                                to: idx,
                                line_at: None,
                                line_after: false,
                                start_x: mx,
                                start_y: my,
                                origin_x: ox,
                                origin_y: oy,
                                x: mx,
                                y: my,
                                armed: false,
                            });
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if let Some(DragKind::MemberReorder {
                            from,
                            start_x,
                            start_y,
                            origin_x,
                            origin_y,
                            mut armed,
                            ..
                        }) = this.drag.take()
                        {
                            let x = f32::from(ev.position.x);
                            let y = f32::from(ev.position.y);
                            if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                                armed = true;
                            }
                            let (to, line_at, line_after) = if armed {
                                this.resolve_member_drop(from, x, y)
                            } else {
                                (from, None, false)
                            };
                            this.drag = Some(DragKind::MemberReorder {
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
                            cx.notify();
                        }
                    })),
            );
        }
        panel = panel.child(
            self.attach_scrollbars(
                "member_scroll_wrap".into(),
                ScrollList::Member,
                &self.member_scroll,
                mlist,
                cx,
            )
            .flex_1()
            .min_h(px(0.)),
        );

        // params (底部固定)
        let param_input = self.param_input.clone();
        let editing_margin = self.param_edit == Some(ParamEdit::Margin);
        let editing_thr = self.param_edit == Some(ParamEdit::Threshold);
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .text_sm()
                .child("边距px")
                .child(
                    div()
                        .id("margin_dec")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("-")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.margin = (this.doc.margin - 1).max(0);
                                cx.notify();
                            }),
                        ),
                )
                .child(if editing_margin {
                    div()
                        .id("margin_edit")
                        .w(px(56.))
                        .h(px(24.))
                        .flex_shrink_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(param_input.clone())
                        .into_any_element()
                } else {
                    div()
                        .id("margin_val")
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .px_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xe2e8f0)).rounded_sm())
                        .child(format!("{margin}"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.begin_param_edit(ParamEdit::Margin, window, cx);
                            }),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .id("margin_inc")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("+")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.margin = (this.doc.margin + 1).min(80);
                                cx.notify();
                            }),
                        ),
                )
                .child("墨迹阈值")
                .child(
                    div()
                        .id("thr_dec")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("-")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.ink_threshold = (this.doc.ink_threshold - 1).max(1);
                                cx.notify();
                            }),
                        ),
                )
                .child(if editing_thr {
                    div()
                        .id("thr_edit")
                        .w(px(56.))
                        .h(px(24.))
                        .flex_shrink_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(param_input)
                        .into_any_element()
                } else {
                    div()
                        .id("thr_val")
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .px_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xe2e8f0)).rounded_sm())
                        .child(format!("{thr}"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.begin_param_edit(ParamEdit::Threshold, window, cx);
                            }),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .id("thr_inc")
                        .px_2()
                        .bg(rgb(0xe2e8f0))
                        .rounded_sm()
                        .cursor_pointer()
                        .child("+")
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if this.param_edit.is_some() {
                                    this.apply_param_edit(window, cx);
                                }
                                this.doc.ink_threshold = (this.doc.ink_threshold + 1).min(254);
                                cx.notify();
                            }),
                        ),
                ),
        );
        panel
    }

    fn dialog_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.score_video.read(cx).is_export_open() {
            return self
                .score_video
                .update(cx, |v, cx| v.export_dialog(cx).into_any_element());
        }
        let Some(ref dlg) = self.dialog else {
            return div().into_any_element();
        };
        if matches!(dlg, DialogKind::UnsavedExit) {
            return self.unsaved_exit_dialog(cx).into_any_element();
        }
        if matches!(dlg, DialogKind::UnsavedNew) {
            return self.unsaved_new_dialog(cx).into_any_element();
        }
        let (title, body) = match dlg {
            DialogKind::Help => ("操作说明".to_string(), HELP_TEXT.to_string()),
            DialogKind::Info { title, body } => (title.clone(), body.clone()),
            DialogKind::UnsavedExit | DialogKind::UnsavedNew => unreachable!(),
        };
        let body_el = div()
            .id("dlg_body")
            .text_sm()
            .text_color(rgb(0x334155))
            .whitespace_normal()
            .child(body);

        div()
            .id("dialog_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            // 阻断背后命中; move/up 留给本层处理 Help 滚动条拖动
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                    this.apply_scrollbar_drag(f32::from(ev.position.x), f32::from(ev.position.y), cx);
                }
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                        this.drag = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("dialog_card")
                    .w(px(520.))
                    .h(px(520.))
                    .max_h(px(520.))
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        self.attach_scrollbars(
                            "help_scroll_wrap".into(),
                            ScrollList::Help,
                            &self.help_scroll,
                            body_el,
                            cx,
                        )
                        .flex_1()
                        .min_h(px(0.)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(self.btn(
                                "dlg_ok",
                                "确定",
                                true,
                                |this, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                },
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn unsaved_exit_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("dialog_backdrop_unsaved")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .id("dialog_card_unsaved")
                    .w(px(420.))
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("未保存的改动"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x334155))
                            .child("当前工程有未保存改动. 要在退出前保存吗?"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "exit_save",
                                "保存并退出",
                                true,
                                |this, window, cx| {
                                    // 保持 UnsavedExit 标记, 供保存成功后 quit
                                    if this.project_path.is_some() {
                                        this.save_project(window, cx);
                                    } else {
                                        this.save_project_as(window, cx);
                                    }
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "exit_discard",
                                "不保存退出",
                                false,
                                |this, _, cx| {
                                    this.dialog = None;
                                    this.dirty = false;
                                    this.allow_close = true;
                                    cx.quit();
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "exit_cancel",
                                "取消",
                                false,
                                |this, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                },
                                cx,
                            )),
                    ),
            )
    }

    fn unsaved_new_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("dialog_backdrop_unsaved_new")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .id("dialog_card_unsaved_new")
                    .w(px(420.))
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("未保存的改动"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x334155))
                            .child("当前工程有未保存改动. 新建前要先保存吗?"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "new_save",
                                "保存后新建",
                                true,
                                |this, window, cx| {
                                    // 保持 UnsavedNew, 供保存成功后清空
                                    if this.project_path.is_some() {
                                        this.save_project(window, cx);
                                    } else {
                                        this.save_project_as(window, cx);
                                    }
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "new_discard",
                                "不保存新建",
                                false,
                                |this, _, cx| {
                                    this.dirty = false;
                                    this.do_new_project(cx);
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "new_cancel",
                                "取消",
                                false,
                                |this, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                },
                                cx,
                            )),
                    ),
            )
    }

    fn tab_context_menu_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ref menu) = self.tab_menu else {
            return div().into_any_element();
        };
        let idx = menu.page_index;
        let x = menu.x;
        let y = menu.y;
        div()
            .id("tab-ctx-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.tab_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.tab_menu = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("tab-ctx-menu")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .min_w(px(148.))
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id("tab-ctx-copy")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xdbeafe)))
                            .child("复制本页")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.tab_menu = None;
                                    this.copy_page(idx, cx);
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for ScoreSyncApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ScoreSyncApp {
    /// 窗口外仍继续的拖拽: GPUI 的元素 on_mouse_move/up 要求 hovered,
    /// 鼠标离开窗口后需由 window.on_mouse_event 转发到此.
    fn handle_outside_window_mouse_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            if matches!(self.drag, Some(DragKind::Scrollbar { .. })) {
                self.apply_scrollbar_drag(x, y, cx);
            }
            return;
        }
        match self.side_tool {
            SideTool::Mask => {
                self.mask_tool
                    .update(cx, |m, cx| m.root_mouse_move(x, y, cx));
            }
            SideTool::Video => {
                self.score_video
                    .update(cx, |v, cx| v.root_mouse_move(x, y, cx));
            }
            _ => {}
        }
        self.apply_host_drag_at(x, y, cx);
    }

    fn handle_outside_window_mouse_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            if matches!(self.drag, Some(DragKind::Scrollbar { .. })) {
                self.drag = None;
                cx.notify();
            }
            return;
        }
        match self.side_tool {
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.root_mouse_up(x, y, cx));
            }
            SideTool::Video => {
                self.score_video
                    .update(cx, |v, cx| v.root_mouse_up(x, y, cx));
            }
            _ => {}
        }
        self.finish_host_drag_at(x, y, cx);
    }

    fn apply_host_drag_at(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(DragKind::Scrollbar { .. }) => {
                self.apply_scrollbar_drag(x, y, cx);
            }
            Some(DragKind::SideResize { .. }) => {
                self.apply_side_resize(x, cx);
            }
            Some(DragKind::TabHScroll { grab }) => {
                let handle = self.tab_scroll.clone();
                let b = handle.bounds();
                let max = f32::from(handle.max_offset().width);
                if max > 0.5 {
                    let tw = f32::from(b.size.width).max(1.0);
                    let thumb = ((tw * tw) / (tw + max)).clamp(24.0, tw);
                    let travel = (tw - thumb).max(1.0);
                    let track_left = f32::from(b.origin.x);
                    let thumb_left = (x - grab - track_left).clamp(0.0, travel);
                    handle.set_offset(point(px(-(thumb_left / travel) * max), px(0.)));
                    cx.notify();
                }
            }
            Some(DragKind::TabReorder {
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                mut armed,
                ..
            }) => {
                if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                    armed = true;
                }
                let (to, line_at, line_after) = if armed {
                    self.resolve_tab_drop(from, x, y)
                } else {
                    (from, None, false)
                };
                self.drag = Some(DragKind::TabReorder {
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
                cx.notify();
            }
            Some(DragKind::MemberReorder {
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                mut armed,
                ..
            }) => {
                if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                    armed = true;
                }
                let (to, line_at, line_after) = if armed {
                    self.resolve_member_drop(from, x, y)
                } else {
                    (from, None, false)
                };
                self.drag = Some(DragKind::MemberReorder {
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
                cx.notify();
            }
            Some(DragKind::GroupReorder {
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                mut armed,
                ctrl,
                ..
            }) => {
                if !armed && Self::reorder_slop_exceeded(x - start_x, y - start_y) {
                    armed = true;
                }
                let (_to, line_at, line_after) = if armed {
                    self.resolve_group_drop(from, x, y)
                } else {
                    (from, None, false)
                };
                self.drag = Some(DragKind::GroupReorder {
                    from,
                    line_at,
                    line_after,
                    start_x,
                    start_y,
                    origin_x,
                    origin_y,
                    x,
                    y,
                    armed,
                    ctrl,
                });
                cx.notify();
            }
            _ => {}
        }
    }

    fn finish_host_drag_at(&mut self, _x: f32, _y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(DragKind::TabReorder { .. })
            | Some(DragKind::MemberReorder { .. })
            | Some(DragKind::GroupReorder { .. })
            | Some(DragKind::Scrollbar { .. })
            | Some(DragKind::SideResize { .. })
            | Some(DragKind::TabHScroll { .. }) => {}
            _ => return,
        }
        match self.drag.take() {
            Some(DragKind::TabReorder {
                from, to, armed, ..
            }) => {
                if armed && from != to {
                    self.push_crop_undo_all_pages();
                    self.doc.move_page(from, to);
                    self.after_doc_change(cx);
                } else {
                    cx.notify();
                }
            }
            Some(DragKind::MemberReorder {
                from, to, armed, ..
            }) => {
                if armed && from != to {
                    let Some(g) = self.doc.active_group() else {
                        cx.notify();
                        return;
                    };
                    let mut ids = g.region_ids.clone();
                    if from < ids.len() && to < ids.len() {
                        self.push_crop_undo_all_pages();
                        let item = ids.remove(from);
                        ids.insert(to, item);
                        self.doc.reorder_active_members(ids);
                        self.after_doc_change(cx);
                    } else {
                        cx.notify();
                    }
                } else {
                    cx.notify();
                }
            }
            Some(DragKind::GroupReorder {
                from,
                armed,
                ctrl,
                line_at,
                line_after,
                ..
            }) => {
                if armed {
                    if let Some(anchor) = line_at {
                        self.push_crop_undo_all_pages();
                        self.doc.reorder_groups_block(from, anchor, line_after);
                        self.after_doc_change(cx);
                    } else {
                        cx.notify();
                    }
                } else if let Some(gid) = self.doc.groups.get(from).map(|g| g.id.clone()) {
                    self.doc.click_group(&gid, ctrl);
                    self.scroll_group_list_to_active();
                    self.refresh_render(cx);
                } else {
                    cx.notify();
                }
            }
            Some(
                DragKind::Scrollbar { .. }
                | DragKind::SideResize { .. }
                | DragKind::TabHScroll { .. },
            ) => {
                cx.notify();
            }
            _ => {}
        }
    }

    fn outside_window_drag_capture(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _cx| {
                let entity_m = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || window.is_window_hovered() {
                        return;
                    }
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    entity_m.update(cx, |this, cx| {
                        this.handle_outside_window_mouse_move(x, y, cx);
                    });
                });
                let entity_u = entity.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || window.is_window_hovered() {
                        return;
                    }
                    if ev.button != MouseButton::Left {
                        return;
                    }
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    entity_u.update(cx, |this, cx| {
                        this.handle_outside_window_mouse_up(x, y, cx);
                    });
                });
            },
        )
        .absolute()
        .size(px(0.))
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
                if this.dialog.is_some() {
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
                    if this.dialog.is_some() {
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
                if this.param_edit.is_some() {
                    this.apply_param_edit(window, cx);
                } else if this.region_y_edit.is_some() {
                    this.apply_edit_y(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CancelParamEdit, window, cx| {
                if this.param_edit.is_some() {
                    this.cancel_param_edit(window, cx);
                } else if this.region_y_edit.is_some() {
                    this.cancel_edit_y(window, cx);
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
                if !list.is_empty() {
                    this.load_paths(list, cx);
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
                                    .w(px(18.))
                                    .h(px(18.))
                                    .flex_shrink_0()
                                    .when(saving, |d| {
                                        d.child(
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
                                        )
                                    })
                                    .when(!saving && dirty, |d| {
                                        d.flex()
                                            .items_center()
                                            .justify_center()
                                            .text_lg()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xdc2626))
                                            .child("*")
                                    }),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x0f172a))
                                    .child(title_core),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .pb_1()
                            .child(self.toolbar(cx)),
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
        text_input::bind_keys(cx);
        apply_bg::text_input::bind_keys(cx);
        score_video::gui::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-o", OpenFile, Some("ScoreSync")),
            KeyBinding::new("ctrl-shift-n", NewProject, Some("ScoreSync")),
            KeyBinding::new("ctrl-shift-o", OpenProject, Some("ScoreSync")),
            KeyBinding::new("ctrl-s", SaveProject, Some("ScoreSync")),
            KeyBinding::new("ctrl-shift-s", SaveProjectAs, Some("ScoreSync")),
            // 任意面板 (含视频 / 输入框失焦前) 都能保存
            KeyBinding::new("ctrl-s", SaveProject, None),
            KeyBinding::new("ctrl-shift-s", SaveProjectAs, None),
            KeyBinding::new("ctrl-shift-o", OpenProject, None),
            KeyBinding::new("ctrl-shift-n", NewProject, None),
            KeyBinding::new("ctrl-s", SaveProject, Some("ScoreVideo")),
            KeyBinding::new("ctrl-shift-s", SaveProjectAs, Some("ScoreVideo")),
            KeyBinding::new("ctrl-shift-o", OpenProject, Some("ScoreVideo")),
            KeyBinding::new("ctrl-shift-n", NewProject, Some("ScoreVideo")),
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
            KeyBinding::new("delete", DeleteSelected, Some("ScoreSync")),
            KeyBinding::new("backspace", DeleteSelected, Some("ScoreSync")),
            KeyBinding::new("enter", ConfirmParamEdit, Some("ScoreSync")),
            KeyBinding::new("escape", CancelParamEdit, Some("ScoreSync")),
            KeyBinding::new("enter", ConfirmParamEdit, None),
            KeyBinding::new("escape", CancelParamEdit, None),
            KeyBinding::new("ctrl-z", Undo, Some("ScoreSync")),
            KeyBinding::new("ctrl-y", Redo, Some("ScoreSync")),
            KeyBinding::new("ctrl-shift-z", Redo, Some("ScoreSync")),
            KeyBinding::new("ctrl-a", SelectAllPageRegions, Some("ScoreSync")),
            // 蒙版工具 (右侧切换到蒙版时 key_context=MaskTool)
            KeyBinding::new("ctrl-o", mask_tool::gui::OpenFile, Some("MaskTool")),
            KeyBinding::new("ctrl-shift-o", OpenProject, Some("MaskTool")),
            KeyBinding::new("ctrl-shift-n", NewProject, Some("MaskTool")),
            KeyBinding::new("ctrl-s", SaveProject, Some("MaskTool")),
            KeyBinding::new("ctrl-shift-s", SaveProjectAs, Some("MaskTool")),
            KeyBinding::new("e", mask_tool::gui::ExportImage, Some("MaskTool")),
            KeyBinding::new("f", mask_tool::gui::FitView, Some("MaskTool")),
            KeyBinding::new("delete", mask_tool::gui::DeleteSelected, Some("MaskTool")),
            KeyBinding::new("backspace", mask_tool::gui::DeleteSelected, Some("MaskTool")),
            KeyBinding::new("b", mask_tool::gui::ToggleDrawMode, Some("MaskTool")),
            KeyBinding::new("l", mask_tool::gui::TogglePolyMode, Some("MaskTool")),
            KeyBinding::new("p", mask_tool::gui::TogglePanMode, Some("MaskTool")),
            KeyBinding::new("escape", mask_tool::gui::CancelPolyDraft, Some("MaskTool")),
            KeyBinding::new("ctrl-a", mask_tool::gui::SelectAll, Some("MaskTool")),
            KeyBinding::new("ctrl-z", mask_tool::gui::Undo, Some("MaskTool")),
            KeyBinding::new("ctrl-y", mask_tool::gui::Redo, Some("MaskTool")),
            KeyBinding::new("ctrl-shift-z", mask_tool::gui::Redo, Some("MaskTool")),
        ]);
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

