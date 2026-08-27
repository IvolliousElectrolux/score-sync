//! GUI 内部类型、常量和帮助文案.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui::{Pixels, Point, SharedString};
use crate::model::{Group, Page, Region};
use mask_tool::guide::GuideState;
use mask_tool::layout::BlockAdjust;
use mask_tool::mask::MaskRect;

pub(crate) const CROP_HISTORY_LIMIT: usize = 64;

pub(crate) const A4_RATIO: f32 = 210.0 / 297.0;
pub(crate) const SIDE_PANEL_W: f32 = 340.0;
pub(crate) const SIDE_PANEL_MIN: f32 = 220.0;
/// 拖拽排序: 超过此像素位移才进入拖拽态 (防点击抖动出虚影)
pub(crate) const REORDER_DRAG_SLOP: f32 = 5.0;
pub(crate) const SIDE_PANEL_MAX: f32 = 720.0;
/// 页数超过此值时页签只渲染可视范围 (文案仍是页码:文件名).
pub(crate) const TAB_VIRTUAL_THRESHOLD: usize = 48;
/// 页签估宽 (页码:文件名 + 间距); 有实测后用平均值.
pub(crate) const TAB_SLOT_PX: f32 = 76.0;
/// 「组织页面」缩略图最长边 (像素).
pub(crate) const ORG_THUMB_MAX_SIDE: u32 = crate::page_cache::ORG_THUMB_SIDE;
pub(crate) const ORG_CELL_W: f32 = 128.0;
pub(crate) const ORG_THUMB_W: f32 = 112.0;
pub(crate) const ORG_THUMB_H: f32 = 158.0;
pub(crate) const ORG_GRID_GAP: f32 = 12.0;
pub(crate) const ORG_SCROLLBAR_W: f32 = 10.0;
/// 组织页面 overlay 相对窗口的边距 (原先 p_4 = 1rem).
pub(crate) const ORG_SLOT_PAD: f32 = 16.0;
/// 组织页面卡片水平 chrome: 左右 padding + border_1.
pub(crate) const ORG_CARD_PAD_X: f32 = 16.0;
pub(crate) const ORG_CARD_BORDER: f32 = 1.0;
pub(crate) const ORG_CARD_CHROME_X: f32 = ORG_CARD_PAD_X * 2.0 + ORG_CARD_BORDER * 2.0;
/// 页签中间 PDF 名的最大显示列宽 (半角=1, 全角=2). 超出则截断并加…….
pub(crate) const TAB_LABEL_NAME_COLS: usize = 11;
/// 输出组合 / 蒙版组合超过此值时只渲染可视范围 (数据仍是全部).
pub(crate) const GROUP_LIST_VIRTUAL_THRESHOLD: usize = 80;
pub(crate) const GROUP_ROW_PX: f32 = 30.0;
pub(crate) const MASK_TAB_SLOT_PX: f32 = 96.0;
/// 「组合分块」列表每行估高.
pub(crate) const MASK_BLOCK_ROW_PX: f32 = 28.0;
pub(crate) const HELP_TEMPLATE: &str = "\
【分块】快捷键:\n\
  {m}O 打开图片/PDF (仅本面板) | {ms}N 新建工程 | {ms}O 打开工程 | {m}S 保存工程 | {ms}S 另存工程\n\
  D 识别本页 | A 识别全部页\n\
  N 添加新块 | S 分割块 | M 合并组合 | {m}M 一键两两合并 | U 拆开组合 | G 共享脚注 | Delete 删除\n\
  E 导出组合 | R 重置本页分组 | P 组织页面 | H / F1 操作说明\n\
  {m}A 全选本页原子块 | 输出组合 {m}点击多选 (拖拽时整块一起调序)\n\
  {m}Z/Y 撤重 (按当前标签页独立记忆; 关闭页面亦可撤回)\n\
  滚轮上下平移画布, Shift+滚轮左右平移, {m}滚轮缩放\n\
\n\
【底色】 (右侧切到底色后):\n\
  左侧预览底色+组合 (不可编辑, 滚轮切换组合);\n\
  右侧「选择底色」导入图片, 「应用底色」/「取消底色」与「使用纯色底色」/「取消纯色底色」互斥;\n\
  调色盘 HSV / RGB / 最近色, 滴管可从左侧预览取谱纸色;\n\
  目标分辨率与批量加底色独立; {m}Z/Y 可撤回底色操作\n\
\n\
【蒙版】快捷键 (右侧切到蒙版后):\n\
   初始为选择态 (不激活框选); 未选中任何蒙版时可直接拖动/拉伸「组合分块」 (拖动中贴图跟手)\n\
  B 框选 | L 折线 (逐点连线, 吸附首点闭环) | P 平移 | 画笔/橡皮 (侧栏, 可调色/粗细)\n\
  E 导出本页图片 | F 适应 | Delete 删除选中\n\
  {m}A 全选蒙版 | {m}Z/Y 撤重 (蒙版与分块微调共用, 按组合独立记忆, 切走再回来仍可撤)\n\
  {m}S 保存工程 (各面板通用)\n\
  顶栏「辅助线」开关 (右键: 全局开启 / 同步同根数位置 / 当前页根数);\n\
  「对齐」把本组合锚到辅助线 (右键: 全局对齐 / 还原初始状态)\n\
  有选中时透明度滑条改选中项; 无选中时改后续新建默认透明度\n\
  点击色块打开浮动取色器: HSV / 最近色 / RGB 手输; 滴管可从左侧图取色\n\
  (悬浮实时预览色盘与 RGB, 单击确认, Esc/右键取消); 画笔光标为圆形预览\n\
\n\
【视频】快捷键 (右侧切到视频后, 先在轨道/预览区点一下获得焦点):\n\
  空格 播放/暂停 | ← / → 快退/快进 1 秒 | Shift+← / Shift+→ 快退/快进 5 秒\n\
  N 在播放头插入下一张组合 (按素材池顺序自动顺延) | I 标记淡入 | O 标记淡出\n\
  Delete / Backspace 删除当前选中的视频片段/淡入淡出/音频片段\n\
  {m}Z/Y 撤重 (时间轴操作)\n\
  鼠标: 拖动片段两端裁剪, 拖动片段整体移动; 淡入淡出轨道可直接拖选一段生成区间,\n\
  {m}点击可多选淡入淡出; 右键「保持背景为底色」只淡乐谱、不淡到黑 (多选则一起应用),\n\
  开启后色块变浅并标「·底」; 视频/淡入淡出/音频边界彼此靠近时自动吸附; 时间轴总长对齐最短的非空音/视频轨\n\
  (删短一轨时较长轨一并裁齐); 末段右边缘对齐音频末尾, 向右拖会缩短该块;\n\
  音频片段可左右拖动重新排序;\n\
  播放条可选预览倍速 (x1 / x1.25 / x1.5 / x2 / x3, ffmpeg atempo 保音调, 不影响导出);\n\
  轨道区 {m}滚轮缩放、普通滚轮左右平移,\n\
  底部横条可整体拖动平移, 拖两端圆点改变缩放.\n\
\n\
操作步骤:\n\
1. 打开/拖入图片或 PDF → 多标签页; 页图写入会话临时目录, 内存默认当前页±4, 高清页自动收到 ±2/±1 (输出组合/蒙版页签仍列出全部).\n\
2. {m}S 保存为单个 .staffcrop 工程包 (zip), 下次可用 {ms}O 继续; {ms}N 新建空白工程后再导入; 有未保存改动关窗会确认.\n\
3. 标签右键菜单「复制本页」可再放一页副本; 新页的输出组合插在原页组合之后、下一页之前.\n\
   「组织页面」(P) 用缩略图网格排序/删除, 比拖页签轻松; 无叉号, {m}点击多选 / Shift 连选, Delete 删选中, {m}Z/Y 撤重.\n\
4. 每页独立识别分块; 「识别全部页」按可用内存限并发异步处理.\n\
   识别先找五线谱表, 再看相邻谱表是否有墨迹像素直接 8 连通 (贴边分量、无内部孔洞或过窄的连通域不计).\n\
5. 「添加新块」(N): 按下定一条边; 先上移则该边为下边线, 先下移则该边为上边线, 拖出另一边后松开.\n\
   新块按上边线 y 插入「输出组合」(本页自上而下), 不会丢到列表末尾.\n\
6. 「分割块」(S): 在已有块内点击, 于指针 y 切成上下两块.\n\
7. {m}多选可跨页, 「合并组合」(M); 脚注可用「共享脚注」让同一块出现在多组导出中.\n\
   {m}M 一键两两合并: 把本页尚未组合的块按上下顺序两两配对; 若剩一块, 则与下一页\n\
   第一块未组合块配对 (例如两页各 5 块, 在第一页按一次得到 p1c1+p1c2, p1c3+p1c4, p1c5+p2c1;\n\
   再到第二页按一次得到 p2c2+p2c3, p2c4+p2c5). 工具栏无此按钮.\n\
8. 「输出组合」可 {m}点击多选并以整块拖拽调序 (导出按列表顺序); 标签为「排序号. p页c页内」\n\
   (p/c 按该组最上块所在页及该页内自上而下序号; 未手动调序时亦按最上块自动排).\n\
   左侧点选块或切回分块时, 列表会滚到对应组合.\n\
9. 「蒙版」编辑当前组合的竖向拼合图; 组合标签与分块一致为「排序号. 来源号」\n\
   (共享脚注可在不同组画不同遮盖). 标签栏/侧栏切换组合; 与分块互相切换时会定位并滚动到对应组合.\n\
   未选中蒙版时: 拖块本体上下移动, 拖上下边裁切/扩展 (先消耗块间空白; 拖动中用分块贴图跟手).\n\
   预览分三层 (底色 / 组合 / 画迹), 用缩略图贴图, 导出时才按终稿拼合.\n\
   「辅助线」按五线谱块数铺线 (两端距顶 5/17、距底 4/15, 拖动按比例联动, 非镜像);\n\
   默认识别为谱表的块才占线, 文字/脚注需在菜单里加根数才纳入对齐.\n\
   一块一个谱行组时用大括号尖 (不够到顶/底谱表则用该组重心); 一块多个谱行组时\n\
   用整块几何中心 (第一组顶线到末组底线的中点); 文字用上下边界中线.\n\
   「对齐」优先保持页宽; 对不齐则改按页高缩尺 (显示变窄). 「还原初始状态」清掉微调.\n\
   「全局开启」后左键开关作用于全部组合; 「同步同根数位置」把当前页线按高度比例映到同样根数的页;\n\
   「全局对齐」后台把已铺线的组合都对齐 (与当页「对齐」同一套几何, 按原始条带算锚点).\n\
10. 「导出组合」按「输出组合」列表顺序拼接并套用各组蒙版; 蒙版侧「导出本页图片」只导出当前组合.\n\
11. 「底色」右侧面板把底色作为可撤销的底层叠加到各输出组合 (不卡界面),\n\
    「取消底色」还原为两层状态; 也可指定纯色 (与图片底色互斥); 蒙版/视频里的预览会实时带上这层底色.\n\
    「批量加底色」弹窗仍可对目录里的谱面图做独立批处理 (比例与工程目标分辨率分开).\n\
12. 「视频」页: 上方预览窗 (悬浮显示可拖动的进度条; 素材按自身比例装进 16:9, 竖图不拉扁),\n\
    下方视频/淡入淡出/音频三条轨道;\n\
    右侧素材池按「输出组合」顺序显示, 点击展开该组合的预览, 拖到视频轨道指定位置即可插入;\n\
    「导入音频」可一次导入多段按顺序播放的音频 (wav/mp3/flac/ogg/m4a/aac, 如各乐章分轨); 「分割音频」按下后,\n\
    在音频轨道上点一下鼠标即可把该处的音频从此切开成两段 (命名为 原名-1 / 原名-2).\n\
    音频不打进工程 (文件往往很大), 只记原路径与切割点; 文件搬走后谱面切片和淡入淡出仍在,\n\
    预览没声、导出在合成音频时失败. 放回原路径即可恢复 (切开状态也还在); 路径变了则先删\n\
    音频轨上的旧片段再导入, 不要叠在旧片段后面 (否则总长变长, 末张谱会被拉齐).\n\
    淡入淡出可 {m}点击多选, 右键「保持背景为底色」则只淡乐谱内容、不淡到黑.\n\
13. 「导出视频」弹窗: 容器选 MP4 (音频有损 AAC, 兼容性好) 或 MKV (音频无损 FLAC);\n\
    帧率可直接点击数字修改; 画质 CRF 数值越小越清晰、文件越大; 分辨率跟随素材中最大的一张\n\
    (加底色后比例相同, 高矮谱面像素可能不同), 无需选择. 导出进度/日志直接显示在弹窗内, 不会另外弹出终端窗口.\n\
\n\
其他:\n\
  空白双击或 F 适应窗口; 拖动画布与侧栏之间的分隔条可调宽度.\n\
  P 组织页面: 缩略图后台并行加载 (关窗仍保留), 拖动排序, {m}点击多选 / Shift 连选, Delete 删除选中页.\n\
  右侧顶栏可切换「分块 / 底色 / 蒙版 / 视频」四个面板.\n\
  标题栏右侧图标: 新建 / 打开工程 / 保存 / 另存 ({ms}N / {ms}O / {m}S / {ms}S), 悬停显示名称; ? 为操作说明 (H / F1).\n\
  标题栏未保存改动显示 *; 异步保存中改为转圈提示.\n\
  视频终稿缓存在工程旁 `.staffcrop.cache/pool/` (不打进 zip); 切到视频面板时按当前组合重建, 已删除组合的旧文件会清掉.\n\
  启动时若已联网会检查 GitHub 更新, 有新版本则弹出当前到最新之间的版本摘要.\n\
  PDF 导入会先弹出分辨率框 (默认按标记尺寸×3 光栅化; 扫描件若页内图像更大则按图像像素预填). 导出组合使用导入后的像素, 不会再放大.\n\
  PDF 导入依赖 pdfium、视频导出依赖 ffmpeg, 需把对应文件放在程序所在目录 (或系统 PATH) 下.";

pub(crate) fn help_text() -> String {
    HELP_TEMPLATE
        .replace("{ms}", apply_bg::primary_shift())
        .replace("{m}", apply_bg::primary_mod())
}

/// 画布编辑工具 (互斥)
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CanvasTool {
    #[default]
    Normal,
    /// 拖出新块: 首按下为锚定边, 先上/下决定上下边
    AddBlock,
    /// 在已有块内切开
    SplitBlock,
}

/// 添加新块时, 首条线扮演的角色
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddAnchorRole {
    /// 首线为上边线, 向下拖出下边
    Top,
    /// 首线为下边线, 向上拖出上边
    Bottom,
}

/// 右侧工具栏模式 (类似 PS 面板切换)
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideTool {
    /// 谱表分块: 原子块 / 组合 / 成员
    Crop,
    /// 工程底色 (预览组合 + 右侧面板)
    Project,
    /// 蒙版遮盖 (mask_tool)
    Mask,
    /// 视频轨道编辑与导出 (score_video)
    Video,
}

/// 分块面板一次可撤操作的快照 (按「触发时所在页」入栈; 可含多页 regions).
/// 删除/复制页等结构变更另存完整 `pages`, 走 `page_struct_history`.
#[derive(Clone)]
pub(crate) struct CropSnap {
    pub(crate) page_regions: HashMap<String, HashMap<String, Region>>,
    /// 页级结构快照; `Some` 时 apply 整表替换 pages (含图), 忽略 page_regions.
    pub(crate) pages: Option<Vec<Page>>,
    pub(crate) current_page_index: Option<usize>,
    pub(crate) group_masks: Option<HashMap<String, Vec<MaskRect>>>,
    pub(crate) groups: Vec<Group>,
    pub(crate) selected_region_ids: HashSet<String>,
    pub(crate) active_group_id: Option<String>,
    pub(crate) groups_manual_order: bool,
    pub(crate) staff_grouping: crate::staff_detect::StaffGrouping,
    pub(crate) group_guides: HashMap<String, GuideState>,
    pub(crate) group_guide_defaults: HashMap<String, GuideState>,
    pub(crate) guides_global: bool,
    pub(crate) guides_sync_positions: bool,
}

/// 蒙版全局辅助线/对齐一次操作的文档快照, 与 `UndoSnapshot::host_guide_token`
/// 配对, 供 Ctrl+Z/Y 回滚全部组合.
#[derive(Clone)]
pub(crate) struct MaskGlobalSnap {
    pub(crate) group_guides: HashMap<String, GuideState>,
    pub(crate) guides_global: bool,
    pub(crate) guides_sync_positions: bool,
    pub(crate) group_block_layout: HashMap<String, Vec<BlockAdjust>>,
    pub(crate) group_voff_shift: HashMap<String, i64>,
    pub(crate) group_masks: HashMap<String, Vec<MaskRect>>,
}

#[derive(Clone)]
pub(crate) struct GuideHistEntry {
    pub(crate) token: u64,
    pub(crate) before: MaskGlobalSnap,
    pub(crate) after: MaskGlobalSnap,
    /// 对齐等改了拼合几何, 回滚后要重拼当前页预览, 但不能 `invalidate_session`.
    pub(crate) refresh_preview: bool,
}

#[derive(Clone, Default)]
pub(crate) struct CropHistory {
    pub(crate) undo: Vec<CropSnap>,
    pub(crate) redo: Vec<CropSnap>,
}

/// 底色面板一次可撤操作: 启用/种类/来源/会话缓存/比例/取色.
#[derive(Clone)]
pub(crate) struct BgSnap {
    pub(crate) enabled: bool,
    pub(crate) is_solid: bool,
    pub(crate) source_path: Option<PathBuf>,
    pub(crate) session_path: Option<PathBuf>,
    pub(crate) aspect_w: u32,
    pub(crate) aspect_h: u32,
    pub(crate) color: [u8; 3],
}

#[derive(Clone, Default)]
pub(crate) struct BgHistory {
    pub(crate) undo: Vec<BgSnap>,
    pub(crate) redo: Vec<BgSnap>,
}

pub(crate) enum DragKind {
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
    /// 底色侧栏内嵌调色盘
    BgPaletteSb,
    BgPaletteHue,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ScrollList {
    Region,
    Group,
    Member,
    /// 蒙版面板: 当前编辑目标内的「组合分块」列表
    MaskBlock,
    /// 操作说明对话框正文
    Help,
    /// 更新提示里的版本摘要
    Update,
    /// 「组织页面」缩略图网格
    PageOrganize,
}

#[derive(Clone)]
pub(crate) enum DialogKind {
    Help,
    Info {
        title: String,
        body: String,
    },
    /// 关窗时有未保存改动
    UnsavedExit,
    /// 新建工程前有未保存改动
    UnsavedNew,
    /// GitHub 上有更新的正式版
    UpdateAvailable {
        current: String,
        latest: String,
        url: String,
        /// 比当前新的各正式版 (新的在前): (版本号, 条目)
        changes: Vec<(String, Vec<String>)>,
    },
}

pub(crate) struct TabContextMenu {
    pub(crate) page_index: usize,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

pub(crate) struct TabTooltip {
    pub(crate) id: SharedString,
    pub(crate) anchor_x: f32,
    pub(crate) anchor_y: f32,
    pub(crate) anchor_w: f32,
    pub(crate) anchor_h: f32,
    pub(crate) text: SharedString,
    /// 实测宽高; 0 表示还没量到, 先用估算.
    pub(crate) measured_w: f32,
    pub(crate) measured_h: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamEdit {
    Margin,
    Threshold,
}

pub(crate) enum ImportJob {
    Pdf {
        path: PathBuf,
        scales: Vec<(f32, f32)>,
        /// 1-based 页码; 空则导入全部页.
        pages: Vec<u32>,
    },
    Image {
        path: PathBuf,
        /// `(宽, 高, 锁定宽高比)`; None 表示按原像素.
        target: Option<(u32, u32, bool)>,
    },
}

pub(crate) enum PdfLoadMsg {
    Page {
        path: PathBuf,
        /// 原 PDF 0-based 页码 (文件名 `_p012` 用).
        index: usize,
        /// 本次已导入页数 (1-based).
        done: usize,
        total: usize,
        pdf_name: String,
    },
    Image {
        path: PathBuf,
        target: Option<(u32, u32, bool)>,
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
