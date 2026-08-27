//! 数据模型与纯业务操作 (对照 app.py 的 Region / Page / Group / MainWindow 逻辑).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbImage;

use crate::staff_detect::{detect_bands, Band, StaffGrouping};
pub use mask_tool::layout::BlockAdjust;
use mask_tool::color_prefs::MaskColorPrefs;
pub use mask_tool::guide::GuideState;
use mask_tool::mask::MaskRect;
use score_video::model::TimelineSnapshot;

pub const COLORS: &[&str] = &[
    "#e74c3c", "#3498db", "#2ecc71", "#f39c12", "#9b59b6", "#1abc9c", "#e67e22",
    "#2980b9", "#16a085", "#c0392b",
];

pub const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp", ".webp"];

pub const DEFAULT_MARGIN: i32 = 20;
pub const DEFAULT_INK_THRESHOLD: i32 = 200;

#[derive(Clone, Debug)]
pub struct Region {
    pub id: String,
    pub page_id: String,
    pub y0: i32,
    pub y1: i32,
    pub kind: String,
    pub color: String,
}

impl Region {
    pub fn label(&self, page_no: Option<usize>) -> String {
        let prefix = page_no
            .map(|n| format!("P{n} "))
            .unwrap_or_default();
        format!(
            "{prefix}{}  y={}-{}  h={}",
            self.kind,
            self.y0,
            self.y1,
            self.y1 - self.y0 + 1
        )
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub id: String,
    /// 显示用路径 / 原始文件名
    pub path: PathBuf,
    /// 会话 tmp (或工程解压落盘) 上的 PNG 备份
    pub disk_path: PathBuf,
    /// 仅内存窗口内有值; 窗口外为 None. `Arc` 让后台任务廉价共享同一份
    /// 像素, 切蒙版/底色时不必在界面线程再 memcpy 一整页.
    pub image: Option<Arc<RgbImage>>,
    /// 卸载后仍可用的尺寸缓存
    pub img_w: u32,
    pub img_h: u32,
    pub regions: HashMap<String, Region>,
}

impl Page {
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("page")
            .to_string()
    }

    /// 页签短标签用的原页码 (PDF `_p012`) 与「复制」标记.
    pub fn tab_badge(&self, fallback_index1: usize) -> String {
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let src = source_page_no_from_stem(stem)
            .map(|n| n.to_string())
            .unwrap_or_else(|| fallback_index1.to_string());
        let copy = copy_mark_from_stem(stem);
        format!("{src}{copy}")
    }

    pub fn height(&self) -> u32 {
        if let Some(img) = self.image.as_ref() {
            img.height()
        } else {
            self.img_h
        }
    }

    pub fn width(&self) -> u32 {
        if let Some(img) = self.image.as_ref() {
            img.width()
        } else {
            self.img_w
        }
    }

    /// 估算本页解码后占用的字节数.
    pub fn estimated_bytes(&self) -> u64 {
        (self.width() as u64) * (self.height() as u64) * 3
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub region_ids: Vec<String>,
    pub name: String,
}

impl Group {
    pub fn display_name(&self, index: usize) -> String {
        if self.name.is_empty() {
            format!("组合 {}", index + 1)
        } else {
            self.name.clone()
        }
    }
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = format!(".{}", e.to_ascii_lowercase());
            IMAGE_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

pub fn is_open_path(path: &Path) -> bool {
    is_image_path(path) || is_pdf_path(path)
}

fn source_page_no_from_stem(stem: &str) -> Option<u32> {
    let b = stem.as_bytes();
    let mut i = 0;
    let mut last = None;
    while i + 2 < b.len() {
        if b[i] == b'_' && b[i + 1] == b'p' && b[i + 2].is_ascii_digit() {
            let start = i + 2;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            if let Ok(n) = stem[start..end].parse::<u32>() {
                last = Some(n);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    last
}

fn copy_mark_from_stem(stem: &str) -> String {
    let Some(pos) = stem.rfind("_copy") else {
        return String::new();
    };
    let rest = &stem[pos + 5..];
    if rest.starts_with("_p") {
        return String::new();
    }
    if rest.is_empty() {
        "复制".into()
    } else if rest.chars().all(|c| c.is_ascii_digit()) {
        format!("复制{rest}")
    } else {
        "复制".into()
    }
}

pub fn parse_color_hex(s: &str) -> u32 {
    let s = s.trim().trim_start_matches('#');
    u32::from_str_radix(s, 16).unwrap_or(0x3498db)
}

/// 裁出 `img` 的 `[y0, y0+height)` 整行条带 (宽度不变), 按行整块
/// `copy_from_slice`, 不用 `image::imageops::crop_imm().to_image()`
/// (内部逐像素调用 get_pixel/put_pixel; 高清扫描页整页宽度的裁切这样
/// 调用开销很可观, 是切到蒙版/底色面板时卡顿的根因之一, 见
/// `apply_bg::process::crop_fast`/`mask_tool::layout::blit_rows` 同样的
/// 考量). 供 [`DocState::crop_region`] 与「全局对齐」后台任务复用.
pub(crate) fn crop_band_fast(img: &RgbImage, y0: u32, height: u32) -> RgbImage {
    let w = img.width();
    let h = img.height();
    let y0 = y0.min(h);
    let height = height.min(h.saturating_sub(y0));
    let mut out = RgbImage::new(w, height);
    let row_bytes = w as usize * 3;
    let src: &[u8] = img;
    let dst: &mut [u8] = &mut out;
    for row in 0..height as usize {
        let s0 = (y0 as usize + row) * row_bytes;
        let d0 = row * row_bytes;
        dst[d0..d0 + row_bytes].copy_from_slice(&src[s0..s0 + row_bytes]);
    }
    out
}

/// `DocState::compose_group_impl` 的 `&self` 无关核心: 按 `layout` 竖向拼合
/// 已裁切好的成员片段. 拆出来是为了让 [`GroupRenderJob::render`] 能在不
/// 持有 `&DocState` 的情况下 (例如后台线程) 复用同一套拼接逻辑.
///
/// 统一走 [`mask_tool::layout::stitch_with_stats`] 的整行 `copy_from_slice`
/// 快路径, 不再对"无布局微调"单独维护一份 `image::imageops::replace`
/// 慢路径 (内部逐像素 get_pixel/put_pixel, 高清页多块组合这样拼一次
/// 开销很可观——这正是切到蒙版/底色面板时卡顿的根因之一, 见
/// `score_sync::gui::sync::sync_mask_image` 文档). `layout` 为空时各块
/// 间距/扩展均为 0, `PieceStats` 不会被实际读取, 用零成本的占位统计即可,
/// 不必为此扫描像素.
pub(crate) fn compose_parts_impl(
    parts: &[(String, image::RgbImage)],
    layout: &[BlockAdjust],
    ink_threshold: i32,
    stats: Option<&std::collections::HashMap<String, mask_tool::layout::PieceStats>>,
) -> Option<image::RgbImage> {
    if parts.is_empty() {
        return None;
    }
    if layout.is_empty() && parts.len() == 1 {
        let max_w = parts.iter().map(|(_, p)| p.width()).max().unwrap_or(1);
        if parts[0].1.width() == max_w {
            return Some(parts[0].1.clone());
        }
    }
    let piece_stats: Vec<mask_tool::layout::PieceStats> = parts
        .iter()
        .map(|(rid, img)| {
            stats
                .and_then(|cache| cache.get(rid).copied())
                .unwrap_or_else(|| {
                    if layout.is_empty() {
                        mask_tool::layout::PieceStats::default()
                    } else {
                        mask_tool::layout::compute_piece_stats(img, ink_threshold)
                    }
                })
        })
        .collect();
    Some(mask_tool::layout::stitch_with_stats(parts, &piece_stats, layout))
}

/// 底色合成所需的快照 (见 [`GroupRenderJob`]).
struct GroupRenderBg {
    image: Option<Arc<RgbImage>>,
    solid: Option<[u8; 3]>,
    src_w: u32,
    src_h: u32,
    aspect_w: u32,
    aspect_h: u32,
    voff_shift: i64,
    leading_gap: u32,
    trailing_gap: u32,
}

/// `DocState::render_group_final` 所需只读数据的快照, 由
/// [`DocState::prepare_group_render_job`] 在主线程一次性收集 (裁切片段的
/// 浅拷贝 + 一些小块元数据, 很快); [`Self::render`] 之后可以放到后台线程
/// 执行真正耗时的拼合 + 蒙版叠加 + 底色合成裁切, 不再堵在界面线程上
/// (视频素材池批量重渲染「输出组合」终稿正是这个场景, 见
/// `score_sync::gui::sync::sync_video_pool`).
pub struct GroupRenderJob {
    parts: Vec<(String, image::RgbImage)>,
    block_layout: Vec<BlockAdjust>,
    ink_threshold: i32,
    masks: Vec<MaskRect>,
    mask_opacity: f32,
    content_scale: f32,
    bg_enabled: bool,
    bg: Option<GroupRenderBg>,
}

impl GroupRenderJob {
    /// 纯计算, 不接触 `DocState`, 可安全放到非主线程跑.
    pub fn render(&self) -> Result<RgbImage, String> {
        let mut combined =
            compose_parts_impl(&self.parts, &self.block_layout, self.ink_threshold, None)
                .ok_or_else(|| "无成员片段".to_string())?;
        if !self.masks.is_empty() {
            mask_tool::mask::apply_masks_to_sheet(
                &mut combined,
                &self.masks,
                self.content_scale,
                self.mask_opacity,
            );
        }
        if self.bg_enabled {
            if let Some(bg) = &self.bg {
                let composed = if let Some(color) = bg.solid {
                    apply_bg::process::composite_solid(
                        &combined,
                        color,
                        bg.src_w,
                        bg.src_h,
                        bg.aspect_w,
                        bg.aspect_h,
                        bg.voff_shift,
                        bg.leading_gap,
                        bg.trailing_gap,
                    )
                } else if let Some(img) = bg.image.as_ref() {
                    apply_bg::process::composite_and_crop(
                        &combined,
                        img,
                        bg.aspect_w,
                        bg.aspect_h,
                        bg.voff_shift,
                        bg.leading_gap,
                        bg.trailing_gap,
                    )
                } else {
                    Ok(combined.clone())
                };
                match composed {
                    Ok(c) => combined = c,
                    Err(e) => {
                        crate::trace::log(&format!(
                            "GroupRenderJob: 底色合成失败, 用纯谱面: {e}"
                        ));
                    }
                }
            }
        }
        Ok(combined)
    }
}

/// 应用级文档状态 (页 / 组 / 选中).
#[derive(Clone, Default)]
pub struct DocState {
    pub pages: Vec<Page>,
    pub groups: Vec<Group>,
    pub selected_region_ids: HashSet<String>,
    pub active_group_id: Option<String>,
    pub current_page_index: usize,
    pub margin: i32,
    pub ink_threshold: i32,
    /// 旧工程字段, 识别已不再使用.
    pub staff_grouping: StaffGrouping,
    /// 组合蒙版: key = group_id, 坐标相对预览画布上的谱面原点
    /// (flush 时已减去 hoff/voff; 谱面高于页面缩小时为缩小后的显示像素)
    pub group_masks: HashMap<String, Vec<MaskRect>>,
    /// 组合内分块的位置/尺寸微调 (蒙版编辑用, 只影响拼合图): key = group_id,
    /// value 与该组 `region_ids` 一一对应 (缺省即视为无调整).
    pub group_block_layout: HashMap<String, Vec<BlockAdjust>>,
    /// 组合拼合图在底色画布中相对默认居中位置的纵向手动偏移 (像素, 负值
    /// 表示比默认居中更靠上): key = group_id, 缺省 (0) 即维持原有的自动
    /// 居中. 蒙版编辑把居中留白折进第一块 `gap_before` (页面绝对坐标)
    /// 后, 此偏移会写成 `-natural_voff`, 让拼合图顶对齐到页顶.
    pub group_voff_shift: HashMap<String, i64>,
    /// 组合内的辅助线 (蒙版画布内的固定参考线, 仅用于手动对齐, 不参与
    /// 导出/合成): key = group_id.
    pub group_guides: HashMap<String, GuideState>,
    /// 蒙版「辅助线」左键开关是否作用到全部组合.
    pub guides_global: bool,
    /// 同样根数辅助线的组合是否同步位置.
    pub guides_sync_positions: bool,
    /// 按块数 + 画布几何预计算的默认辅助线 (导入/建组时写入, 不含用户
    /// 拖动). 不进工程文件, 随时可从几何重算. 全局开启时拷到 `group_guides`.
    pub group_guide_defaults: HashMap<String, GuideState>,
    /// 各分块条带的谱表锚点 (相对条带顶, `None` = 已判定非谱表).
    /// 导入/识别时写入, 不进工程文件. 缺 key 表示还没算过.
    pub region_staff_anchors: HashMap<String, Option<i32>>,
    /// 蒙版/画笔默认色、透明度与最近使用色
    pub mask_prefs: MaskColorPrefs,
    /// 用户已手动拖拽调序「输出组合」; 为 true 时不再自动按页/y 排序
    pub groups_manual_order: bool,
    /// 工程级底色层 (底层); 不改写页图, 导出/终稿合成时才叠上
    pub bg_enabled: bool,
    pub bg_image: Option<Arc<RgbImage>>,
    /// 纯色底色: 有值时不持有整张 `bg_image`, 预览画色块, 终稿按页填色.
    pub bg_solid: Option<[u8; 3]>,
    /// 仅用于 UI 显示来源路径
    pub bg_source_path: Option<PathBuf>,
    pub bg_aspect_w: u32,
    pub bg_aspect_h: u32,
    /// `bg_image` 每次被替换 (`set_project_bg`/`clear_project_bg`) 时自增,
    /// 供 GUI 侧给「底色 GPU 贴图」做缓存判重: 完整底色只备份这一份,
    /// 贴图按目标页裁切后再缩放, 见 [`mask_tool::gui::BlockBgTile::from_full`].
    pub bg_gen: u64,
    /// 视频面板时间轴的纯数据快照 (实际编辑态在 `score_video::ScoreVideoApp`
    /// 里, 这里只是保存/载入工程时的中转载体).
    pub video_state: TimelineSnapshot,
    /// region_id → page index, 避免 find_region 每次扫全部页.
    pub(crate) rid_page: HashMap<String, usize>,
}

impl DocState {
    pub fn new() -> Self {
        Self {
            margin: DEFAULT_MARGIN,
            ink_threshold: DEFAULT_INK_THRESHOLD,
            mask_prefs: MaskColorPrefs::default(),
            bg_aspect_w: 2560,
            bg_aspect_h: 1440,
            ..Default::default()
        }
    }

    /// 后台保存用快照: 不拷贝页图像素 (走 disk_path), 避免与 UI 窗口图叠成双倍内存.
    /// 底色仍按需拷一份 (通常一张).
    pub fn clone_for_save(&self) -> Self {
        Self {
            pages: self
                .pages
                .iter()
                .map(|p| Page {
                    id: p.id.clone(),
                    path: p.path.clone(),
                    disk_path: p.disk_path.clone(),
                    image: None,
                    img_w: p.img_w,
                    img_h: p.img_h,
                    regions: p.regions.clone(),
                })
                .collect(),
            groups: self.groups.clone(),
            selected_region_ids: self.selected_region_ids.clone(),
            active_group_id: self.active_group_id.clone(),
            current_page_index: self.current_page_index,
            margin: self.margin,
            ink_threshold: self.ink_threshold,
            staff_grouping: self.staff_grouping,
            group_masks: self.group_masks.clone(),
            group_block_layout: self.group_block_layout.clone(),
            group_voff_shift: self.group_voff_shift.clone(),
            group_guides: self.group_guides.clone(),
            guides_global: self.guides_global,
            guides_sync_positions: self.guides_sync_positions,
            group_guide_defaults: self.group_guide_defaults.clone(),
            region_staff_anchors: self.region_staff_anchors.clone(),
            mask_prefs: self.mask_prefs.clone(),
            groups_manual_order: self.groups_manual_order,
            bg_enabled: self.bg_enabled,
            bg_image: self.bg_image.clone(),
            bg_solid: self.bg_solid,
            bg_source_path: self.bg_source_path.clone(),
            bg_aspect_w: self.bg_aspect_w,
            bg_aspect_h: self.bg_aspect_h,
            bg_gen: self.bg_gen,
            video_state: self.video_state.clone(),
            rid_page: HashMap::new(),
        }
    }

    pub fn get_group_masks(&self, group_id: &str) -> &[MaskRect] {
        self.group_masks
            .get(group_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn set_group_masks(&mut self, group_id: &str, masks: Vec<MaskRect>) {
        if masks.is_empty() {
            self.group_masks.remove(group_id);
        } else {
            self.group_masks.insert(group_id.to_string(), masks);
        }
    }

    pub fn get_block_layout(&self, group_id: &str) -> &[BlockAdjust] {
        self.group_block_layout
            .get(group_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 全为无操作的调整视为未设置, 及时清理避免工程文件里堆积空数据.
    pub fn set_block_layout(&mut self, group_id: &str, layout: Vec<BlockAdjust>) {
        if layout.iter().all(BlockAdjust::is_noop) {
            self.group_block_layout.remove(group_id);
        } else {
            self.group_block_layout.insert(group_id.to_string(), layout);
        }
    }

    pub fn get_group_voff_shift(&self, group_id: &str) -> i64 {
        self.group_voff_shift.get(group_id).copied().unwrap_or(0)
    }

    pub fn set_group_voff_shift(&mut self, group_id: &str, shift: i64) {
        if shift == 0 {
            self.group_voff_shift.remove(group_id);
        } else {
            self.group_voff_shift.insert(group_id.to_string(), shift);
        }
    }

    pub fn get_group_guides(&self, group_id: &str) -> GuideState {
        self.group_guides.get(group_id).cloned().unwrap_or_default()
    }

    pub fn set_group_guides(&mut self, group_id: &str, guides: GuideState) {
        if guides.is_default() {
            self.group_guides.remove(group_id);
        } else {
            self.group_guides.insert(group_id.to_string(), guides);
        }
    }

    /// 组内各成员原始高度 (region y0/y1, 不需要像素).
    pub fn group_member_heights(&self, group_id: &str) -> Vec<(String, u32)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let (_, r) = self.find_region(rid)?;
                Some((rid.clone(), (r.y1 - r.y0 + 1).max(0) as u32))
            })
            .collect()
    }

    /// 拼合图宽: 取成员所在页宽的最大值.
    pub fn group_sheet_width(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 1;
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let (pi, _) = self.find_region(rid)?;
                Some(self.pages.get(pi)?.width().max(1))
            })
            .max()
            .unwrap_or(1)
    }

    /// 预览画布几何 (不含像素). 无底色时画布就是拼合图.
    pub fn group_preview_frame(&self, group_id: &str) -> Option<apply_bg::process::PreviewFrame> {
        let heights = self.group_member_heights(group_id);
        if heights.is_empty() {
            return None;
        }
        let sw = self.group_sheet_width(group_id);
        let sh = mask_tool::layout::sheet_height(&heights, self.get_block_layout(group_id));
        if !self.bg_enabled {
            return Some(apply_bg::process::PreviewFrame {
                canvas_w: sw,
                canvas_h: sh,
                hoff: 0,
                voff: 0,
                bg_left: 0,
                bg_top: 0,
                shows_bg: false,
                content_scale: 1.0,
            });
        }
        let (bw, bh) = self.bg_src_size()?;
        Some(apply_bg::process::preview_frame(
            sw,
            sh,
            bw,
            bh,
            self.bg_aspect_w,
            self.bg_aspect_h,
            self.get_group_voff_shift(group_id),
        ))
    }

    /// 按组内五线谱块数 (有预计算锚点则只数认得出谱表的; 否则退回总块数)
    /// 和预览画布高生成默认辅助线, 不读像素. 一根时落在两端中点 (比页心
    /// 略偏下). 文字/脚注默认不占线, 需手动加根数才纳入对齐.
    pub fn compute_default_guides(&self, group_id: &str) -> GuideState {
        let n = self.group_staff_block_count(group_id);
        let h = self
            .group_preview_frame(group_id)
            .map(|f| f.canvas_h as i32)
            .unwrap_or(0);
        let mut g = GuideState::default();
        g.set_staff_slots(n, h);
        g
    }

    fn group_staff_block_count(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 0;
        };
        let known = g
            .region_ids
            .iter()
            .filter(|id| self.region_staff_anchors.contains_key(*id))
            .count();
        if known == 0 {
            return g.region_ids.len() as u32;
        }
        g.region_ids
            .iter()
            .filter(|id| matches!(self.region_staff_anchors.get(*id), Some(Some(_))))
            .count() as u32
    }

    /// 为指定组合写入默认辅助线. `guides_global` 时若该组还没有显示用的
    /// 线, 一并拷过去.
    pub fn seed_guide_defaults_for(&mut self, gids: &[String]) {
        for gid in gids {
            let d = self.compute_default_guides(gid);
            if d.lines.is_empty() {
                self.group_guide_defaults.remove(gid);
            } else {
                self.group_guide_defaults.insert(gid.clone(), d.clone());
            }
            if self.guides_global
                && self.get_group_guides(gid).lines.is_empty()
                && !d.lines.is_empty()
            {
                self.set_group_guides(gid, d);
            }
        }
    }

    /// 刷新全部组合的默认辅助线 (建组/改底色后).
    pub fn seed_guide_defaults(&mut self) {
        let valid: HashSet<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        self.group_guide_defaults.retain(|k, _| valid.contains(k));
        let gids: Vec<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        self.seed_guide_defaults_for(&gids);
    }

    pub fn ingest_region_staff_anchors(&mut self, items: impl IntoIterator<Item = (String, Option<i32>)>) {
        for (id, y) in items {
            self.region_staff_anchors.insert(id, y);
        }
    }

    /// 当前页图已在内存时, 给尚未预算的分块补谱表锚点 (不读磁盘).
    pub fn seed_region_anchors_for_page(&mut self, page_idx: usize) {
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let Some(img) = page.image.as_ref() else {
            return;
        };
        let thr = self.ink_threshold;
        let bands: Vec<(String, i32, i32)> = page
            .regions
            .values()
            .filter(|r| !self.region_staff_anchors.contains_key(&r.id))
            .map(|r| (r.id.clone(), r.y0, r.y1))
            .collect();
        if bands.is_empty() {
            return;
        }
        let computed: Vec<(String, Option<i32>)> = bands
            .into_iter()
            .map(|(id, y0, y1)| {
                (id, mask_tool::staff::band_staff_anchor(img, y0, y1, thr))
            })
            .collect();
        self.ingest_region_staff_anchors(computed);
    }

    /// 全局开启: 缺线的组合用预计算默认值填上. 不读页图.
    pub fn apply_guides_global_on(&mut self) {
        self.guides_global = true;
        self.seed_guide_defaults();
    }

    /// 全局关闭: 清掉显示用的线, 默认值保留以便再开.
    pub fn apply_guides_global_off(&mut self) {
        self.guides_global = false;
        self.group_guides.clear();
    }

    /// 同步确保某页像素在内存中.
    pub fn ensure_image(&mut self, page_idx: usize) -> Result<(), String> {
        let Some(page) = self.pages.get(page_idx) else {
            return Err("页不存在".into());
        };
        if page.image.is_some() {
            return Ok(());
        }
        let path = page.disk_path.clone();
        let img = crate::page_cache::load_rgb(&path)?;
        let (w, h) = (img.width(), img.height());
        if let Some(page) = self.pages.get_mut(page_idx) {
            page.img_w = w;
            page.img_h = h;
            page.image = Some(Arc::new(img));
        }
        Ok(())
    }

    pub fn ensure_images(&mut self, indices: &[usize]) -> Result<(), String> {
        for &i in indices {
            self.ensure_image(i)?;
        }
        Ok(())
    }

    pub fn unload_page_image(&mut self, page_idx: usize) {
        if let Some(page) = self.pages.get_mut(page_idx) {
            if let Some(img) = page.image.take() {
                page.img_w = img.width();
                page.img_h = img.height();
            }
        }
    }

    /// 按当前页体积决定内存窗口半径 (高清页小于默认 ±4).
    pub fn memory_window_radius(&self) -> usize {
        let b = self.current_page().map(|p| p.estimated_bytes()).unwrap_or(0);
        crate::page_cache::window_radius_for_bytes(b)
    }

    /// 只留当前页附近的解码像素, 半径见 [`Self::memory_window_radius`].
    pub fn retain_memory_window(&mut self) {
        self.retain_window(self.current_page_index, self.memory_window_radius());
    }

    /// 内存只保留 `center ± radius` 页的像素.
    pub fn retain_window(&mut self, center: usize, radius: usize) {
        let n = self.pages.len();
        if n == 0 {
            return;
        }
        let center = center.min(n - 1);
        let lo = center.saturating_sub(radius);
        let hi = (center + radius).min(n - 1);
        for i in 0..n {
            if i < lo || i > hi {
                self.unload_page_image(i);
            } else if self.pages[i].image.is_none() {
                let _ = self.ensure_image(i);
            }
        }
    }

    pub fn page_indices_for_group(&self, group_id: &str) -> Vec<usize> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        let mut idxs = Vec::new();
        for rid in &g.region_ids {
            if let Some((pi, _)) = self.find_region(rid) {
                if !idxs.contains(&pi) {
                    idxs.push(pi);
                }
            }
        }
        idxs
    }

    pub fn ensure_group_pages(&mut self, group_id: &str) -> Result<(), String> {
        let idxs = self.page_indices_for_group(group_id);
        self.ensure_images(&idxs)
    }

    /// 裁切某页上的区域条带 (整宽). 调用前须 `ensure` 相关页.
    pub fn crop_region(&self, region_id: &str) -> Option<image::RgbImage> {
        let (pi, r) = self.find_region(region_id)?;
        let page = self.pages.get(pi)?;
        let img = page.image.as_ref()?;
        let y0 = r.y0.max(0) as u32;
        let y1 = (r.y1 as u32).min(page.height().saturating_sub(1));
        if y1 < y0 {
            return None;
        }
        Some(crop_band_fast(img, y0, y1 - y0 + 1))
    }

    /// 组内各成员的原始裁切片段 (未应用 `group_block_layout` 微调), 与
    /// `region_ids` 顺序一致; 调用前须 `ensure_group_pages`. 供蒙版编辑
    /// 里拖动「组合分块」使用: 已在内存中, 不必每帧回读磁盘, 只是重新
    /// 拼接 (含底色合成) 交回蒙版画布显示.
    pub fn group_member_pieces(&self, group_id: &str) -> Vec<(String, image::RgbImage)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| self.crop_region(rid).map(|img| (rid.clone(), img)))
            .collect()
    }

    /// 按组内成员顺序竖向拼合 (与导出一致, 不含蒙版). 若该组存在蒙版编辑时
    /// 的分块位置/尺寸微调 (`group_block_layout`), 在此一并应用; 否则走原
    /// 有的纯拼接快速路径 (性能/结果与旧版本完全一致). 导出终稿走带底色/
    /// 蒙版合成的 `render_group_final`, 这个不缓存统计的简单版本目前只在
    /// 测试里直接练到 `compose_parts_impl`, 保留作为公开的轻量入口.
    #[allow(dead_code)]
    pub fn compose_group(&self, group_id: &str) -> Option<image::RgbImage> {
        let parts = self.group_member_pieces(group_id);
        self.compose_group_impl(group_id, &parts, None)
    }

    /// 同 `compose_group`, 但各块裁切片段与背景色统计都由调用方预先准备.
    /// 预览已改为三层贴图, 这条路径留给测试 / 需要整图像素的调用方.
    #[allow(dead_code)]
    pub fn compose_group_with_parts_and_stats(
        &self,
        group_id: &str,
        parts: &[(String, image::RgbImage)],
        stats: &std::collections::HashMap<String, mask_tool::layout::PieceStats>,
    ) -> Option<image::RgbImage> {
        self.compose_group_impl(group_id, parts, Some(stats))
    }

    fn compose_group_impl(
        &self,
        group_id: &str,
        parts: &[(String, image::RgbImage)],
        stats: Option<&std::collections::HashMap<String, mask_tool::layout::PieceStats>>,
    ) -> Option<image::RgbImage> {
        compose_parts_impl(parts, self.get_block_layout(group_id), self.ink_threshold, stats)
    }

    /// 组合最前面那个块自己的 `gap_before` (人为拖动第一块腾出的、没有
    /// 真实内容的顶端留白). 启用底色层时合成阶段要跳过贴这一段, 让底色
    /// 直接透出来, 见 [`Self::compose_group_preview_from`] 与
    /// [`Self::render_group_final`].
    pub fn group_leading_gap(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 0;
        };
        let Some(first_rid) = g.region_ids.first() else {
            return 0;
        };
        mask_tool::layout::BlockAdjust::find(self.get_block_layout(group_id), first_rid)
            .map(|a| a.gap_before.max(0) as u32)
            .unwrap_or(0)
    }

    /// 组合最后一块后面的末端留白 (旧版向上拖过页顶后缩小内部块用;
    /// 现已改为碰到页顶即停, 此字段多为工程兼容).
    pub fn group_trailing_gap(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 0;
        };
        let Some(last_rid) = g.region_ids.last() else {
            return 0;
        };
        mask_tool::layout::BlockAdjust::find(self.get_block_layout(group_id), last_rid)
            .map(|a| a.gap_after.max(0) as u32)
            .unwrap_or(0)
    }

    /// 组合内各块在拼合图中的纵向范围 (`(region_id, comp_y0, comp_y1)`),
    /// 已应用 `group_block_layout` 微调; 供「组合分块」列表/蒙版画布使用.
    pub fn group_member_spans(&self, group_id: &str) -> Vec<(String, i64, i64)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        let heights: Vec<(String, u32)> = g
            .region_ids
            .iter()
            .filter_map(|rid| {
                let (_, r) = self.find_region(rid)?;
                Some((rid.clone(), (r.y1 - r.y0 + 1).max(0) as u32))
            })
            .collect();
        mask_tool::layout::compute_spans(&heights, self.get_block_layout(group_id))
    }

    /// 启用工程底色层 (底层). 不修改页图 / 蒙版.
    pub fn set_project_bg(
        &mut self,
        image: RgbImage,
        source: Option<PathBuf>,
        aspect_w: u32,
        aspect_h: u32,
    ) -> Result<(), String> {
        if aspect_w == 0 || aspect_h == 0 {
            return Err("比例宽高必须为正整数".into());
        }
        self.bg_image = Some(Arc::new(image));
        self.bg_solid = None;
        self.bg_source_path = source;
        self.bg_aspect_w = aspect_w;
        self.bg_aspect_h = aspect_h;
        self.bg_enabled = true;
        self.bg_gen = self.bg_gen.wrapping_add(1);
        self.seed_guide_defaults();
        Ok(())
    }

    /// 启用纯色底色层. 不分配整张底色图.
    pub fn set_project_bg_solid(
        &mut self,
        color: [u8; 3],
        aspect_w: u32,
        aspect_h: u32,
    ) -> Result<(), String> {
        if aspect_w == 0 || aspect_h == 0 {
            return Err("比例宽高必须为正整数".into());
        }
        self.bg_image = None;
        self.bg_solid = Some(color);
        self.bg_source_path = None;
        self.bg_aspect_w = aspect_w;
        self.bg_aspect_h = aspect_h;
        self.bg_enabled = true;
        self.bg_gen = self.bg_gen.wrapping_add(1);
        self.seed_guide_defaults();
        Ok(())
    }

    /// 已启用纯色时只改颜色 (几何不变, 不重算辅助线).
    pub fn update_bg_solid_color(&mut self, color: [u8; 3]) {
        self.bg_solid = Some(color);
        self.bg_gen = self.bg_gen.wrapping_add(1);
    }

    /// 预览/合成用的底色源尺寸. 纯色没有像素备份, 用能盖住各页的虚拟画布.
    pub fn bg_src_size(&self) -> Option<(u32, u32)> {
        if !self.bg_enabled {
            return None;
        }
        if let Some(img) = self.bg_image.as_ref() {
            return Some((img.width(), img.height()));
        }
        if self.bg_solid.is_some() {
            let aw = self.bg_aspect_w.max(1);
            let ah = self.bg_aspect_h.max(1);
            let max_w = self
                .pages
                .iter()
                .map(|p| p.width())
                .max()
                .unwrap_or(aw)
                .max(aw)
                .max(1);
            return Some(apply_bg::process::page_size(max_w, aw, ah));
        }
        None
    }

    /// 取消工程底色层.
    pub fn clear_project_bg(&mut self) {
        self.bg_enabled = false;
        self.bg_image = None;
        self.bg_solid = None;
        self.bg_source_path = None;
        self.bg_gen = self.bg_gen.wrapping_add(1);
        self.seed_guide_defaults();
    }

    /// 拼合图预览 (供蒙版/视频面板显示): 若已启用工程底色, 叠加底色预览
    /// (contain: 上下或左右补边, 不烧入蒙版). 返回 (预览图, 谱面在预览图
    /// 中的横向/纵向偏移, 供调用方换算蒙版坐标). GUI 侧统一走下面
    /// 复用裁切片段/统计缓存的 `..._with_parts_and_stats`, 这个简单版本
    /// 保留作为公开的轻量入口 (含测试覆盖).
    #[allow(dead_code)]
    pub fn compose_group_preview(&self, group_id: &str) -> Option<(RgbImage, i64, i64)> {
        self.compose_group_preview_from(group_id, self.compose_group(group_id))
    }

    /// 同 `compose_group_preview`, 但拼合图用调用方预先缓存好的裁切片段.
    #[allow(dead_code)]
    pub fn compose_group_preview_with_parts_and_stats(
        &self,
        group_id: &str,
        parts: &[(String, image::RgbImage)],
        stats: &std::collections::HashMap<String, mask_tool::layout::PieceStats>,
    ) -> Option<(RgbImage, i64, i64)> {
        self.compose_group_preview_from(
            group_id,
            self.compose_group_with_parts_and_stats(group_id, parts, stats),
        )
    }

    fn compose_group_preview_from(
        &self,
        group_id: &str,
        sheet: Option<RgbImage>,
    ) -> Option<(RgbImage, i64, i64)> {
        let sheet = sheet?;
        if !self.bg_enabled {
            return Some((sheet, 0, 0));
        }
        let voff_shift = self.get_group_voff_shift(group_id);
        let top_transparent = self.group_leading_gap(group_id);
        let bottom_transparent = self.group_trailing_gap(group_id);
        if let Some(color) = self.bg_solid {
            let Some((bw, bh)) = self.bg_src_size() else {
                return Some((sheet, 0, 0));
            };
            return match apply_bg::process::composite_preview_solid(
                &sheet,
                color,
                bw,
                bh,
                self.bg_aspect_w,
                self.bg_aspect_h,
                voff_shift,
                top_transparent,
                bottom_transparent,
            ) {
                Ok((canvas, hoff, voff)) => Some((canvas, hoff, voff)),
                Err(_) => Some((sheet, 0, 0)),
            };
        }
        let Some(bg) = self.bg_image.as_ref() else {
            return Some((sheet, 0, 0));
        };
        match apply_bg::process::composite_preview(
            &sheet,
            bg,
            self.bg_aspect_w,
            self.bg_aspect_h,
            voff_shift,
            top_transparent,
            bottom_transparent,
        ) {
            Ok((canvas, hoff, voff)) => Some((canvas, hoff, voff)),
            Err(_) => Some((sheet, 0, 0)),
        }
    }

    /// 终稿合成: 拼合 → 蒙版 → (可选) 底色底层裁切.
    ///
    /// 蒙版存在「预览画布 − hoff/voff」坐标系. 谱面高于页面被缩小装进画布时,
    /// 先除以 `content_scale` 映回未缩放拼合图再盖上, 然后按原图等比合成,
    /// 避免视频里遮盖偏移, 也不把谱面拉进画布坐标系里变形.
    pub fn render_group_final(&self, group_id: &str) -> Result<Option<RgbImage>, String> {
        match self.prepare_group_render_job(group_id) {
            Some(job) => job.render().map(Some),
            None => Ok(None),
        }
    }

    /// 见 [`GroupRenderJob`] 文档: 收集渲染某组合终稿所需的只读快照 (裁切
    /// 片段的浅拷贝 + 蒙版/底色等小块元数据), 供调用方挪到后台线程调用
    /// [`GroupRenderJob::render`], 主线程这一步应该很快.
    pub fn prepare_group_render_job(&self, group_id: &str) -> Option<GroupRenderJob> {
        let parts = self.group_member_pieces(group_id);
        if parts.is_empty() {
            return None;
        }
        let block_layout = self.get_block_layout(group_id).to_vec();
        let masks = self.get_group_masks(group_id).to_vec();
        let content_scale = self
            .group_preview_frame(group_id)
            .map(|f| f.content_scale)
            .unwrap_or(1.0);
        let bg = self.bg_src_size().map(|(src_w, src_h)| GroupRenderBg {
            image: self.bg_image.clone(),
            solid: self.bg_solid,
            src_w,
            src_h,
            aspect_w: self.bg_aspect_w,
            aspect_h: self.bg_aspect_h,
            voff_shift: self.get_group_voff_shift(group_id),
            leading_gap: self.group_leading_gap(group_id),
            trailing_gap: self.group_trailing_gap(group_id),
        });
        Some(GroupRenderJob {
            parts,
            block_layout,
            ink_threshold: self.ink_threshold,
            masks,
            mask_opacity: self.mask_prefs.mask_opacity,
            content_scale,
            bg_enabled: self.bg_enabled,
            bg,
        })
    }

    pub fn current_page(&self) -> Option<&Page> {
        self.pages.get(self.current_page_index)
    }

    pub fn page_index(&self, page_id: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.id == page_id)
    }

    pub fn page_no(&self, page_id: &str) -> usize {
        self.page_index(page_id).map(|i| i + 1).unwrap_or(0)
    }

    pub fn find_region(&self, rid: &str) -> Option<(usize, &Region)> {
        if let Some(&pi) = self.rid_page.get(rid) {
            if let Some(r) = self.pages.get(pi).and_then(|p| p.regions.get(rid)) {
                return Some((pi, r));
            }
        }
        for (pi, page) in self.pages.iter().enumerate() {
            if let Some(r) = page.regions.get(rid) {
                return Some((pi, r));
            }
        }
        None
    }

    pub fn get_region(&self, rid: &str) -> Option<&Region> {
        self.find_region(rid).map(|(_, r)| r)
    }

    pub fn get_region_mut(&mut self, rid: &str) -> Option<&mut Region> {
        let pi = self.rid_page.get(rid).copied().filter(|&pi| {
            self.pages
                .get(pi)
                .map(|p| p.regions.contains_key(rid))
                .unwrap_or(false)
        });
        if let Some(pi) = pi {
            return self.pages.get_mut(pi).and_then(|p| p.regions.get_mut(rid));
        }
        for page in &mut self.pages {
            if page.regions.contains_key(rid) {
                return page.regions.get_mut(rid);
            }
        }
        None
    }

    pub fn rebuild_rid_index(&mut self) {
        self.rid_page.clear();
        self.rid_page.reserve(
            self.pages
                .iter()
                .map(|p| p.regions.len())
                .sum::<usize>(),
        );
        for (i, page) in self.pages.iter().enumerate() {
            for rid in page.regions.keys() {
                self.rid_page.insert(rid.clone(), i);
            }
        }
    }

    pub fn active_group(&self) -> Option<&Group> {
        let id = self.active_group_id.as_ref()?;
        self.groups.iter().find(|g| &g.id == id)
    }

    pub fn active_group_mut(&mut self) -> Option<&mut Group> {
        let id = self.active_group_id.clone()?;
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn region_sort_key(&self, rid: &str) -> (usize, i32, i32) {
        match self.find_region(rid) {
            Some((pi, r)) => (pi, r.y0, r.y1),
            None => (usize::MAX, i32::MAX, i32::MAX),
        }
    }

    pub fn group_sort_key(&self, g: &Group) -> (usize, i32, i32) {
        match g.region_ids.first() {
            Some(rid) => self.region_sort_key(rid),
            None => (usize::MAX, i32::MAX, i32::MAX),
        }
    }

    /// 组合内最上块的排序键 (页序, y0, y1); 空组给哨兵值.
    pub fn group_top_key(&self, g: &Group) -> (usize, i32, i32) {
        g.region_ids
            .iter()
            .map(|rid| self.region_sort_key(rid))
            .min()
            .unwrap_or((usize::MAX, i32::MAX, i32::MAX))
    }

    /// 来源号 `p<页码>c<该页内按最上块 y 的序号>` (1-based).
    /// 页码取组合最上块所在页; `c` 只在「最上块落在同一页」的组合之间计数.
    pub fn group_origin_code(&self, group_index: usize) -> String {
        let Some(g) = self.groups.get(group_index) else {
            return "p?c?".into();
        };
        let top = self.group_top_key(g);
        if top.0 == usize::MAX {
            return "p?c?".into();
        }
        let page_no = top.0 + 1;
        let mut same_page: Vec<(usize, (usize, i32, i32))> = self
            .groups
            .iter()
            .enumerate()
            .filter_map(|(i, og)| {
                let k = self.group_top_key(og);
                if k.0 == top.0 {
                    Some((i, k))
                } else {
                    None
                }
            })
            .collect();
        same_page.sort_by_key(|(_, k)| *k);
        let c = same_page
            .iter()
            .position(|(i, _)| *i == group_index)
            .map(|i| i + 1)
            .unwrap_or(1);
        format!("p{page_no}c{c}")
    }

    /// 分块「输出组合」列表用: `排序号. 来源号`, 如 `5. p2c2`.
    pub fn group_crop_label(&self, group_index: usize) -> String {
        format!(
            "{}. {}",
            group_index + 1,
            self.group_origin_code(group_index)
        )
    }

    pub fn sort_groups(&mut self) {
        if self.groups_manual_order {
            return;
        }
        // Need keys first to avoid borrow issues
        let mut keyed: Vec<(usize, (usize, i32, i32))> = self
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| (i, self.group_sort_key(g)))
            .collect();
        keyed.sort_by_key(|(_, k)| *k);
        let order: Vec<usize> = keyed.into_iter().map(|(i, _)| i).collect();
        let mut new_groups = Vec::with_capacity(self.groups.len());
        for i in order {
            new_groups.push(std::mem::replace(
                &mut self.groups[i],
                Group {
                    id: String::new(),
                    region_ids: Vec::new(),
                    name: String::new(),
                },
            ));
        }
        self.groups = new_groups;
    }

    /// 拖拽调序输出组合; 之后保留用户顺序直到重新识别重建分组.
    /// 若拖拽起点本身已在多选内, 则整块选中组合一起移动; 否则只动这一项.
    pub fn group_move_indices(&self, from: usize) -> Vec<usize> {
        if from >= self.groups.len() {
            return Vec::new();
        }
        if self.group_has_selected_region(&self.groups[from]) {
            let idxs: Vec<usize> = self
                .groups
                .iter()
                .enumerate()
                .filter(|(_, g)| self.group_has_selected_region(g))
                .map(|(i, _)| i)
                .collect();
            if !idxs.is_empty() {
                return idxs;
            }
        }
        vec![from]
    }

    /// 将 from 所属移动块 (多选整体或单项) 插到 anchor 之前/之后.
    pub fn reorder_groups_block(&mut self, from: usize, anchor: usize, after: bool) {
        let n = self.groups.len();
        if from >= n || anchor >= n {
            return;
        }
        let moving = self.group_move_indices(from);
        if moving.is_empty() {
            return;
        }
        let moving_set: HashSet<usize> = moving.iter().copied().collect();
        if moving_set.contains(&anchor) {
            return;
        }
        let raw_insert = if after { anchor + 1 } else { anchor };
        let insert_in_remaining =
            raw_insert - moving.iter().filter(|&&i| i < raw_insert).count();

        let mut remaining = Vec::with_capacity(n - moving.len());
        let mut block = Vec::with_capacity(moving.len());
        for (i, g) in self.groups.drain(..).enumerate() {
            if moving_set.contains(&i) {
                block.push(g);
            } else {
                remaining.push(g);
            }
        }
        let insert_at = insert_in_remaining.min(remaining.len());
        for (j, g) in block.into_iter().enumerate() {
            remaining.insert(insert_at + j, g);
        }
        self.groups = remaining;
        self.groups_manual_order = true;
    }

    pub fn sync_group_colors(&mut self) {
        self.sort_groups();
        let mut assigned: HashSet<String> = HashSet::new();
        let color_assigns: Vec<(String, String)> = {
            let mut out = Vec::new();
            for (i, g) in self.groups.iter().enumerate() {
                let color = COLORS[i % COLORS.len()].to_string();
                for rid in &g.region_ids {
                    if assigned.contains(rid) {
                        continue;
                    }
                    if self.get_region(rid).is_some() {
                        out.push((rid.clone(), color.clone()));
                        assigned.insert(rid.clone());
                    }
                }
            }
            out
        };
        for (rid, color) in color_assigns {
            if let Some(r) = self.get_region_mut(&rid) {
                r.color = color;
            }
        }
    }

    pub fn ensure_active_group(&mut self) {
        self.prune_orphan_masks();
        if let Some(ref id) = self.active_group_id {
            if self.groups.iter().any(|g| &g.id == id) {
                return;
            }
        }
        self.active_group_id = self.groups.first().map(|g| g.id.clone());
    }

    fn prune_orphan_masks(&mut self) {
        let valid: HashSet<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        self.group_masks.retain(|k, _| valid.contains(k));
        self.group_guides.retain(|k, _| valid.contains(k));
        self.group_guide_defaults.retain(|k, _| valid.contains(k));
        let valid_rids: HashSet<String> = self
            .pages
            .iter()
            .flat_map(|p| p.regions.keys().cloned())
            .chain(self.groups.iter().flat_map(|g| g.region_ids.iter().cloned()))
            .collect();
        self.region_staff_anchors.retain(|k, _| valid_rids.contains(k));
    }

    /// 去掉已经找不到 region 的组合和残留 rid.
    /// 有页尚未灌入 regions 时不要调用, 否则会误删那些页的组合.
    pub fn prune_dangling_groups(&mut self) {
        let valid: HashSet<String> = self
            .pages
            .iter()
            .flat_map(|p| p.regions.keys().cloned())
            .collect();
        for g in &mut self.groups {
            g.region_ids.retain(|id| valid.contains(id));
        }
        self.groups.retain(|g| !g.region_ids.is_empty());
        self.selected_region_ids.retain(|id| valid.contains(id));
        self.ensure_active_group();
    }

    /// 全部页都已有识别结果时才清幽灵组合 (避免 hydrate 中途误删).
    pub fn prune_dangling_groups_if_hydrated(&mut self) {
        if self.pages.is_empty() || self.pages.iter().any(|p| p.regions.is_empty()) {
            return;
        }
        self.prune_dangling_groups();
    }

    /// 加载一页 RGB 图并写入会话 tmp, 再自动识别. `switch_to`: 是否切到新页.
    pub fn add_page(
        &mut self,
        path: PathBuf,
        image: RgbImage,
        switch_to: bool,
    ) -> Result<usize, String> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("page.png");
        let disk_path = crate::page_cache::write_rgb_png(&image, name)?;
        let _ = crate::page_cache::write_org_thumb(&image, &disk_path);
        let (w, h) = (image.width(), image.height());
        let page = Page {
            id: new_id(),
            path,
            disk_path,
            image: Some(Arc::new(image)),
            img_w: w,
            img_h: h,
            regions: HashMap::new(),
        };
        self.pages.push(page);
        let idx = self.pages.len() - 1;
        if switch_to {
            self.current_page_index = idx;
        }
        self.detect_page(idx, true);
        self.retain_memory_window();
        Ok(idx)
    }

    /// 已有磁盘 PNG (会话 tmp / PDF 渲染输出 / 工程解压) 登记为新页.
    pub fn add_page_from_disk(
        &mut self,
        path: PathBuf,
        disk_path: PathBuf,
        switch_to: bool,
        run_detect: bool,
    ) -> Result<usize, String> {
        let (w, h) = image::image_dimensions(&disk_path)
            .map_err(|e| format!("读取页尺寸失败 ({}): {e}", disk_path.display()))?;
        let page = Page {
            id: new_id(),
            path,
            disk_path,
            image: None,
            img_w: w,
            img_h: h,
            regions: HashMap::new(),
        };
        self.pages.push(page);
        let idx = self.pages.len() - 1;
        if switch_to {
            self.current_page_index = idx;
        }
        if run_detect {
            crate::trace::log(&format!("doc: detect_page idx={idx} 开始"));
            self.detect_page(idx, true);
            crate::trace::log(&format!("doc: detect_page idx={idx} 结束"));
        }
        if run_detect {
            self.retain_memory_window();
        }
        Ok(idx)
    }

    pub fn detect_page(&mut self, page_idx: usize, reset_groups: bool) {
        if self.ensure_image(page_idx).is_err() {
            return;
        }
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let Some(img) = page.image.as_ref() else {
            return;
        };
        let old_ids: HashSet<String> = page.regions.keys().cloned().collect();
        let bands = detect_bands(img, self.ink_threshold, self.margin);
        let bands = if bands.is_empty() {
            vec![Band {
                y0: 0,
                y1: page.height().saturating_sub(1) as i32,
                kind: "region".into(),
            }]
        } else {
            bands
        };
        let page_id = page.id.clone();
        let mut regions = HashMap::new();
        for (i, b) in bands.iter().enumerate() {
            let rid = new_id();
            regions.insert(
                rid.clone(),
                Region {
                    id: rid,
                    page_id: page_id.clone(),
                    y0: b.y0,
                    y1: b.y1,
                    kind: b.kind.clone(),
                    color: COLORS[i % COLORS.len()].to_string(),
                },
            );
        }
        if let Some(page) = self.pages.get_mut(page_idx) {
            page.regions = regions;
        }
        self.rebuild_rid_index();
        self.seed_region_anchors_for_page(page_idx);
        self.save_detect_sidecar(page_idx);
        if reset_groups {
            let page_regions: Vec<Region> = self.pages[page_idx]
                .regions
                .values()
                .cloned()
                .collect();
            let mut new_groups: Vec<Group> = Vec::new();
            for g in &self.groups {
                let remain: Vec<String> = g
                    .region_ids
                    .iter()
                    .filter(|x| !old_ids.contains(*x))
                    .cloned()
                    .collect();
                if !remain.is_empty() {
                    new_groups.push(Group {
                        id: g.id.clone(),
                        region_ids: remain,
                        name: g.name.clone(),
                    });
                }
            }
            let mut ordered = page_regions;
            ordered.sort_by_key(|r| (r.y0, r.y1));
            for r in ordered {
                new_groups.push(Group {
                    id: new_id(),
                    region_ids: vec![r.id],
                    name: String::new(),
                });
            }
            self.groups = new_groups;
            self.selected_region_ids = self
                .selected_region_ids
                .difference(&old_ids)
                .cloned()
                .collect();
            self.groups_manual_order = false;
            self.sort_groups();
            self.prune_dangling_groups_if_hydrated();
            self.ensure_active_group();
            self.seed_guide_defaults();
        }
    }

    pub fn apply_detect_file(
        &mut self,
        page_idx: usize,
        file: &crate::detect_cache::PageDetectFile,
    ) {
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let page_id = page.id.clone();
        let mut regions = HashMap::new();
        for (i, r) in file.regions.iter().enumerate() {
            regions.insert(
                r.id.clone(),
                Region {
                    id: r.id.clone(),
                    page_id: page_id.clone(),
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind.clone(),
                    color: COLORS[i % COLORS.len()].to_string(),
                },
            );
        }
        if let Some(page) = self.pages.get_mut(page_idx) {
            if file.img_w > 0 {
                page.img_w = file.img_w;
                page.img_h = file.img_h;
            }
            page.regions = regions;
        }
        self.rebuild_rid_index();
        for r in &file.regions {
            if let Some(a) = &r.staff_anchor {
                self.region_staff_anchors.insert(r.id.clone(), a.y);
            }
        }
        self.seed_region_anchors_for_page(page_idx);
    }

    pub fn load_detect_sidecar(&mut self, page_idx: usize) -> bool {
        let Some(path) = self.pages.get(page_idx).map(|p| p.disk_path.clone()) else {
            return false;
        };
        let Some(file) = crate::detect_cache::load(&path) else {
            return false;
        };
        self.apply_detect_file(page_idx, &file);
        true
    }

    pub fn save_detect_sidecar(&self, page_idx: usize) {
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let mut regions: Vec<Region> = page.regions.values().cloned().collect();
        regions.sort_by_key(|r| (r.y0, r.y1));
        let file = crate::detect_cache::PageDetectFile {
            img_w: page.img_w,
            img_h: page.img_h,
            ink_threshold: self.ink_threshold,
            margin: self.margin,
            staff_grouping: self.staff_grouping,
            regions: regions
                .into_iter()
                .map(|r| crate::detect_cache::CachedRegion {
                    id: r.id.clone(),
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind,
                    staff_anchor: self.region_staff_anchors.get(&r.id).map(|y| {
                        crate::detect_cache::CachedStaffAnchor { y: *y }
                    }),
                })
                .collect(),
        };
        let _ = crate::detect_cache::save(&page.disk_path, &file);
    }

    fn group_min_page_idx(&self, g: &Group) -> usize {
        g.region_ids
            .iter()
            .filter_map(|rid| self.find_region(rid).map(|(pi, _)| pi))
            .min()
            .unwrap_or(usize::MAX)
    }

    /// 本页已有识别结果但还没有对应输出组合时, 按页序补上 1:1 组合.
    /// 不改已有合并/调序, 也不删其它页的组 (其它页 regions 可能尚未灌入).
    pub fn ensure_page_groups(&mut self, page_idx: usize) {
        let page_rids: HashSet<String> = self
            .pages
            .get(page_idx)
            .map(|p| p.regions.keys().cloned().collect())
            .unwrap_or_default();
        if page_rids.is_empty() {
            return;
        }
        let covered: HashSet<String> = self
            .groups
            .iter()
            .flat_map(|g| g.region_ids.iter().cloned())
            .collect();
        if page_rids.iter().all(|id| covered.contains(id)) {
            return;
        }
        let mut ordered: Vec<Region> = self.pages[page_idx]
            .regions
            .values()
            .filter(|r| !covered.contains(&r.id))
            .cloned()
            .collect();
        ordered.sort_by_key(|r| (r.y0, r.y1));
        let new_groups: Vec<Group> = ordered
            .iter()
            .map(|r| Group {
                id: new_id(),
                region_ids: vec![r.id.clone()],
                name: String::new(),
            })
            .collect();
        let new_ids: Vec<String> = new_groups.iter().map(|g| g.id.clone()).collect();
        let insert_at = {
            let mut at = self.groups.len();
            for (i, g) in self.groups.iter().enumerate().rev() {
                if self.group_min_page_idx(g) > page_idx {
                    at = i;
                } else {
                    break;
                }
            }
            at
        };
        for (i, g) in new_groups.into_iter().enumerate() {
            self.groups.insert(insert_at + i, g);
        }
        self.seed_guide_defaults_for(&new_ids);
        self.ensure_active_group();
    }

    pub fn ensure_all_page_groups(&mut self) {
        for i in 0..self.pages.len() {
            self.ensure_page_groups(i);
        }
        self.prune_dangling_groups_if_hydrated();
        self.ensure_active_group();
        self.seed_guide_defaults();
    }

    /// 把磁盘 sidecar 灌进尚未有 regions 的页. 返回灌入页数. 不改 groups.
    pub fn hydrate_detect_sidecars(&mut self) -> usize {
        let n = self.pages.len();
        let mut loaded = 0usize;
        for i in 0..n {
            if self.pages[i].regions.is_empty() && self.load_detect_sidecar(i) {
                loaded += 1;
            }
        }
        if loaded > 0 {
            self.rebuild_rid_index();
        }
        loaded
    }

    /// 用 sidecar 替换本页 regions, 并按新结果重建本页输出组合.
    /// 必须在替换前记下旧 rid: 重新识别会生成全新 id, 不能靠新 id 去匹配旧组合.
    pub fn replace_page_detect(
        &mut self,
        page_idx: usize,
        file: &crate::detect_cache::PageDetectFile,
    ) {
        let old_ids: HashSet<String> = self
            .pages
            .get(page_idx)
            .map(|p| p.regions.keys().cloned().collect())
            .unwrap_or_default();
        self.apply_detect_file(page_idx, file);
        self.upsert_page_groups(page_idx, &old_ids);
    }

    /// 按页序插入/替换本页 groups, 其它页的组块顺序不受影响.
    /// `old_region_ids` 是替换前本页的 rid, 用来丢掉已失效的旧组合.
    pub fn upsert_page_groups(&mut self, page_idx: usize, old_region_ids: &HashSet<String>) {
        let page_rids: HashSet<String> = self
            .pages
            .get(page_idx)
            .map(|p| p.regions.keys().cloned().collect())
            .unwrap_or_default();
        if page_rids.is_empty() {
            return;
        }
        let mut drop_ids = page_rids.clone();
        drop_ids.extend(old_region_ids.iter().cloned());
        self.groups = self
            .groups
            .drain(..)
            .filter_map(|mut g| {
                g.region_ids.retain(|id| !drop_ids.contains(id));
                if g.region_ids.is_empty() {
                    None
                } else {
                    Some(g)
                }
            })
            .collect();
        let mut ordered: Vec<Region> = self.pages[page_idx]
            .regions
            .values()
            .cloned()
            .collect();
        ordered.sort_by_key(|r| (r.y0, r.y1));
        let new_groups: Vec<Group> = ordered
            .iter()
            .map(|r| Group {
                id: new_id(),
                region_ids: vec![r.id.clone()],
                name: String::new(),
            })
            .collect();
        let insert_at = {
            let mut at = self.groups.len();
            for (i, g) in self.groups.iter().enumerate().rev() {
                if self.group_min_page_idx(g) > page_idx {
                    at = i;
                } else {
                    break;
                }
            }
            at
        };
        for (i, g) in new_groups.into_iter().enumerate() {
            self.groups.insert(insert_at + i, g);
        }
        self.seed_guide_defaults();
        self.ensure_active_group();
    }

    /// 按页序从当前各页 regions 重建全部 groups. 全量识别结束时调用一次,
    /// 避免 detect_page(reset_groups=true) 每页都拷贝已有 groups (O(n²)).
    pub fn rebuild_all_groups(&mut self) {
        let mut new_groups: Vec<Group> = Vec::new();
        for page in &self.pages {
            let mut ordered: Vec<Region> = page.regions.values().cloned().collect();
            ordered.sort_by_key(|r| (r.y0, r.y1));
            for r in ordered {
                new_groups.push(Group {
                    id: new_id(),
                    region_ids: vec![r.id],
                    name: String::new(),
                });
            }
        }
        self.groups = new_groups;
        self.selected_region_ids.clear();
        self.groups_manual_order = false;
        self.sort_groups();
        self.ensure_active_group();
        self.seed_guide_defaults();
    }

    #[allow(dead_code)]
    pub fn detect_all(&mut self) {
        let n = self.pages.len();
        for i in 0..n {
            self.detect_page(i, false);
            self.retain_memory_window();
        }
        self.rebuild_all_groups();
    }

    pub fn reset_current_page_groups(&mut self) {
        let Some(page) = self.current_page() else {
            return;
        };
        let page_ids: HashSet<String> = page.regions.keys().cloned().collect();
        let ordered: Vec<Region> = {
            let mut v: Vec<_> = page.regions.values().cloned().collect();
            v.sort_by_key(|r| (r.y0, r.y1));
            v
        };
        let mut new_groups: Vec<Group> = Vec::new();
        for g in &self.groups {
            let foreign: Vec<String> = g
                .region_ids
                .iter()
                .filter(|x| !page_ids.contains(*x))
                .cloned()
                .collect();
            if !foreign.is_empty() {
                new_groups.push(Group {
                    id: g.id.clone(),
                    region_ids: foreign,
                    name: g.name.clone(),
                });
            }
        }
        for r in ordered {
            new_groups.push(Group {
                id: new_id(),
                region_ids: vec![r.id],
                name: String::new(),
            });
        }
        self.groups = new_groups;
        self.groups_manual_order = false;
        self.sort_groups();
        self.ensure_active_group();
    }

    pub fn delete_selected(&mut self) -> usize {
        let ids: Vec<String> = self
            .selected_region_ids
            .iter()
            .filter(|rid| self.get_region(rid).is_some())
            .cloned()
            .collect();
        if ids.is_empty() {
            return 0;
        }
        let id_set: HashSet<String> = ids.iter().cloned().collect();
        for rid in &ids {
            for page in &mut self.pages {
                page.regions.remove(rid);
            }
        }
        self.groups = self
            .groups
            .iter()
            .filter_map(|g| {
                let remain: Vec<String> = g
                    .region_ids
                    .iter()
                    .filter(|x| !id_set.contains(*x))
                    .cloned()
                    .collect();
                if remain.is_empty() {
                    None
                } else {
                    Some(Group {
                        id: g.id.clone(),
                        region_ids: remain,
                        name: g.name.clone(),
                    })
                }
            })
            .collect();
        self.sort_groups();
        self.selected_region_ids.clear();
        self.ensure_active_group();
        ids.len()
    }

    pub fn merge_selected(&mut self) -> Result<usize, &'static str> {
        let mut ids: Vec<String> = self
            .selected_region_ids
            .iter()
            .filter(|rid| self.get_region(rid).is_some())
            .cloned()
            .collect();
        if ids.len() < 2 {
            return Err(
                "请至少选中 2 个原子块再合并.\n(可切换标签页后 ⌘/Ctrl 继续多选以实现跨页组合)",
            );
        }
        ids.sort_by_key(|rid| self.region_sort_key(rid));
        let id_set: HashSet<String> = ids.iter().cloned().collect();
        // 以排序后第一个块所在组合的原位置为准插入 (手动调序时 sort_groups 不会跑).
        let first_rid = &ids[0];
        let old_idx = self
            .groups
            .iter()
            .position(|g| g.region_ids.iter().any(|x| x == first_rid))
            .unwrap_or(self.groups.len());
        let mut insert_at = 0usize;
        for (i, g) in self.groups.iter().enumerate() {
            if i >= old_idx {
                break;
            }
            let keep = g.region_ids.iter().any(|x| !id_set.contains(x));
            if keep {
                insert_at += 1;
            }
        }

        let mut new_groups: Vec<Group> = Vec::new();
        for g in &self.groups {
            let remain: Vec<String> = g
                .region_ids
                .iter()
                .filter(|x| !id_set.contains(*x))
                .cloned()
                .collect();
            if !remain.is_empty() {
                new_groups.push(Group {
                    id: g.id.clone(),
                    region_ids: remain,
                    name: g.name.clone(),
                });
            }
        }
        let g_new = Group {
            id: new_id(),
            region_ids: ids.clone(),
            name: String::new(),
        };
        let gid = g_new.id.clone();
        let insert_at = insert_at.min(new_groups.len());
        new_groups.insert(insert_at, g_new);
        self.groups = new_groups;
        self.sort_groups();
        self.active_group_id = Some(gid);
        self.seed_guide_defaults();
        Ok(ids.len())
    }

    fn region_is_uncombined(&self, rid: &str) -> bool {
        !self
            .groups
            .iter()
            .any(|g| g.region_ids.len() > 1 && g.region_ids.iter().any(|x| x == rid))
    }

    fn ungrouped_rids_on_page(&self, page_idx: usize) -> Vec<String> {
        let Some(page) = self.pages.get(page_idx) else {
            return Vec::new();
        };
        let mut rids: Vec<String> = page
            .regions
            .keys()
            .filter(|rid| self.region_is_uncombined(rid))
            .cloned()
            .collect();
        rids.sort_by_key(|rid| self.region_sort_key(rid));
        rids
    }

    fn first_ungrouped_after(&self, page_idx: usize) -> Option<String> {
        for pi in (page_idx + 1)..self.pages.len() {
            let rids = self.ungrouped_rids_on_page(pi);
            if let Some(rid) = rids.into_iter().next() {
                return Some(rid);
            }
        }
        None
    }

    /// 当前页未组合块按顺序两两合并; 奇数剩一块则与后续页第一块未组合块配对.
    pub fn pair_ungrouped(&mut self) -> Result<usize, &'static str> {
        let page = self.current_page_index;
        let rids = self.ungrouped_rids_on_page(page);
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut i = 0;
        while i + 1 < rids.len() {
            pairs.push((rids[i].clone(), rids[i + 1].clone()));
            i += 2;
        }
        if i < rids.len() {
            if let Some(next) = self.first_ungrouped_after(page) {
                pairs.push((rids[i].clone(), next));
            }
        }
        if pairs.is_empty() {
            return Err("本页没有足够的未组合块可配对.");
        }
        for (a, b) in &pairs {
            self.selected_region_ids = HashSet::from([a.clone(), b.clone()]);
            self.merge_selected()?;
        }
        Ok(pairs.len())
    }

    pub fn share_selected_into_active(&mut self) -> Result<usize, &'static str> {
        if self.active_group().is_none() {
            return Err("请先在「输出组合」里选一个目标组.");
        }
        let mut ids: Vec<String> = self
            .selected_region_ids
            .iter()
            .filter(|rid| self.get_region(rid).is_some())
            .cloned()
            .collect();
        if ids.is_empty() {
            return Err("请先选中要共享加入的块 (如脚注).");
        }
        ids.sort_by_key(|rid| self.region_sort_key(rid));
        let Some(g) = self.active_group_mut() else {
            return Err("请先在「输出组合」里选一个目标组.");
        };
        let mut added = 0;
        for rid in ids {
            if !g.region_ids.contains(&rid) {
                g.region_ids.push(rid);
                added += 1;
            }
        }
        if added == 0 {
            return Ok(0);
        }
        self.sort_groups();
        self.seed_guide_defaults();
        Ok(added)
    }

    pub fn ungroup_active(&mut self) -> Result<(), &'static str> {
        let Some(g) = self.active_group() else {
            return Err("请选择含多个成员的组合.");
        };
        if g.region_ids.len() <= 1 {
            return Err("请选择含多个成员的组合.");
        }
        let Some(idx) = self.groups.iter().position(|x| x.id == g.id) else {
            return Err("请选择含多个成员的组合.");
        };
        let region_ids = g.region_ids.clone();
        let singles: Vec<Group> = region_ids
            .into_iter()
            .map(|rid| Group {
                id: new_id(),
                region_ids: vec![rid],
                name: String::new(),
            })
            .collect();
        let first_id = singles[0].id.clone();
        self.groups.splice(idx..=idx, singles);
        self.sort_groups();
        self.active_group_id = Some(first_id);
        self.seed_guide_defaults();
        Ok(())
    }

    pub fn apply_edge_drag(&mut self, region_id: &str, edge: &str, new_y: i32) {
        let Some((pi, _)) = self.find_region(region_id) else {
            return;
        };
        let h = self.pages[pi].height() as i32;
        let new_y = new_y.clamp(0, h - 1);
        let Some(r) = self.pages[pi].regions.get_mut(region_id) else {
            return;
        };
        if edge == "top" {
            if new_y <= r.y1 {
                r.y0 = new_y;
            } else {
                r.y0 = r.y1;
                r.y1 = new_y;
            }
        } else if new_y >= r.y0 {
            r.y1 = new_y;
        } else {
            r.y1 = r.y0;
            r.y0 = new_y;
        }
        self.sort_groups();
    }

    pub fn set_region_y(&mut self, rid: &str, y0: i32, y1: i32) -> bool {
        let Some((pi, _)) = self.find_region(rid) else {
            return false;
        };
        let h = self.pages[pi].height() as i32;
        let (mut y0, mut y1) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        y0 = y0.clamp(0, h - 1);
        y1 = y1.clamp(0, h - 1);
        if let Some(r) = self.pages[pi].regions.get_mut(rid) {
            r.y0 = y0;
            r.y1 = y1;
            self.selected_region_ids = HashSet::from([rid.to_string()]);
            true
        } else {
            false
        }
    }

    /// 在已有块内于 y 切开为上下两块; 点在空白处无效.
    pub fn split_block_at(&mut self, scene_y: f32) -> String {
        if self.current_page().is_none() {
            return "请先打开图片.".into();
        }
        let pi = self.current_page_index;
        let h = self.pages[pi].height() as i32;
        let y = (scene_y.round() as i32).clamp(0, h - 1);
        let page_id = self.pages[pi].id.clone();
        let page_no = pi + 1;
        let hit: Vec<Region> = self.pages[pi]
            .regions
            .values()
            .filter(|r| r.y0 <= y && y <= r.y1)
            .cloned()
            .collect();
        if hit.is_empty() {
            return format!("P{page_no} y={y}: 请点在已有块内部进行分割.");
        }
        let mut created: Vec<String> = Vec::new();
        let n = hit.len();
        for target in hit {
            self.split_one_region(&page_id, &target, y, &mut created);
        }
        if created.is_empty() {
            return format!("P{page_no} y={y}: 无法在此位置分割 (已在块边).");
        }
        self.selected_region_ids = created.into_iter().collect();
        self.rebuild_rid_index();
        self.seed_guide_defaults();
        format!("P{page_no} 已在 y={y} 切开 {n} 块.")
    }

    /// 在空白处新建手动块 [y0, y1] (含端点).
    pub fn add_manual_block(&mut self, y0: i32, y1: i32) -> String {
        if self.current_page().is_none() {
            return "请先打开图片.".into();
        }
        let pi = self.current_page_index;
        let h = self.pages[pi].height() as i32;
        let (mut a, mut b) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        a = a.clamp(0, h - 1);
        b = b.clamp(0, h - 1);
        if b < a {
            return "块高度无效.".into();
        }
        let page_id = self.pages[pi].id.clone();
        let page_no = pi + 1;
        let rid = new_id();
        let n_regions = self.pages[pi].regions.len();
        self.pages[pi].regions.insert(
            rid.clone(),
            Region {
                id: rid.clone(),
                page_id,
                y0: a,
                y1: b,
                kind: "manual".into(),
                color: COLORS[n_regions % COLORS.len()].to_string(),
            },
        );
        self.rebuild_rid_index();
        self.insert_group_by_top_y(Group {
            id: new_id(),
            region_ids: vec![rid.clone()],
            name: String::new(),
        });
        if !self.groups_manual_order {
            self.sort_groups();
        }
        self.selected_region_ids = HashSet::from([rid]);
        self.seed_guide_defaults();
        format!("P{page_no} 新建手动块 y={a}-{b} h={}.", b - a + 1)
    }

    /// 按最上块 (页序, y0, y1) 把新组合插进输出列表, 而不是追加到末尾.
    /// 已手动调序时也不全量重排: 插到本页里「上边线紧挨在下方」的那个组合前面.
    fn insert_group_by_top_y(&mut self, group: Group) {
        let key = self.group_top_key(&group);
        let page_idx = key.0;
        let mut successor: Option<(usize, (usize, i32, i32))> = None;
        let mut last_geo: Option<(usize, (usize, i32, i32))> = None;
        for (i, g) in self.groups.iter().enumerate() {
            let k = self.group_top_key(g);
            if k.0 != page_idx {
                continue;
            }
            if last_geo.is_none_or(|(_, lk)| k >= lk) {
                last_geo = Some((i, k));
            }
            if k > key && successor.is_none_or(|(_, sk)| k < sk) {
                successor = Some((i, k));
            }
        }
        let at = if let Some((i, _)) = successor {
            i
        } else if let Some((i, _)) = last_geo {
            i + 1
        } else {
            let mut at = self.groups.len();
            for (i, g) in self.groups.iter().enumerate().rev() {
                if self.group_min_page_idx(g) > page_idx {
                    at = i;
                } else {
                    break;
                }
            }
            at
        };
        self.groups.insert(at, group);
    }

    fn split_one_region(
        &mut self,
        page_id: &str,
        target: &Region,
        y: i32,
        created: &mut Vec<String>,
    ) {
        let Some(pi) = self.page_index(page_id) else {
            return;
        };
        if !self.pages[pi].regions.contains_key(&target.id) {
            return;
        }
        if y < target.y0 || y > target.y1 {
            return;
        }
        if target.y0 == target.y1 && y == target.y0 {
            return;
        }
        let mut parts = vec![Region {
            id: new_id(),
            page_id: page_id.to_string(),
            y0: target.y0,
            y1: y,
            kind: target.kind.clone(),
            color: target.color.clone(),
        }];
        if y < target.y1 {
            let n = self.pages[pi].regions.len();
            parts.push(Region {
                id: new_id(),
                page_id: page_id.to_string(),
                y0: y + 1,
                y1: target.y1,
                kind: target.kind.clone(),
                color: COLORS[n % COLORS.len()].to_string(),
            });
        } else if y == target.y1 && target.y0 < target.y1 {
            return;
        }
        if parts.len() == 1 && parts[0].y0 == target.y0 && parts[0].y1 == target.y1 {
            return;
        }
        let old_id = target.id.clone();
        self.pages[pi].regions.remove(&old_id);
        let mut new_ids = Vec::new();
        for p in parts {
            new_ids.push(p.id.clone());
            created.push(p.id.clone());
            self.pages[pi].regions.insert(p.id.clone(), p);
        }
        for g in &mut self.groups {
            if let Some(pos) = g.region_ids.iter().position(|x| x == &old_id) {
                g.region_ids.splice(pos..=pos, new_ids.clone());
            }
        }
    }

    pub fn close_page_at(&mut self, index: usize) -> bool {
        !self.close_pages_at(&[index]).is_empty()
    }

    /// 按原下标批量关页 (可乱序/重复). 返回被删页 id.
    pub fn close_pages_at(&mut self, indices: &[usize]) -> Vec<String> {
        let n = self.pages.len();
        if n == 0 {
            return Vec::new();
        }
        let drop: HashSet<usize> = indices.iter().copied().filter(|&i| i < n).collect();
        if drop.is_empty() {
            return Vec::new();
        }
        let cur = self.current_page_index.min(n - 1);
        let keep_id = if drop.contains(&cur) {
            None
        } else {
            Some(self.pages[cur].id.clone())
        };
        let fallback_old = (cur + 1..n)
            .find(|i| !drop.contains(i))
            .or_else(|| (0..cur).rev().find(|i| !drop.contains(i)));

        let mut kept = Vec::with_capacity(n - drop.len());
        let mut dead_rids = HashSet::new();
        let mut dead_pids = Vec::with_capacity(drop.len());
        for (i, page) in self.pages.drain(..).enumerate() {
            if drop.contains(&i) {
                dead_rids.extend(page.regions.keys().cloned());
                dead_pids.push(page.id);
            } else {
                kept.push(page);
            }
        }
        self.pages = kept;
        self.groups = self
            .groups
            .iter()
            .filter_map(|g| {
                let remain: Vec<String> = g
                    .region_ids
                    .iter()
                    .filter(|x| !dead_rids.contains(*x))
                    .cloned()
                    .collect();
                if remain.is_empty() {
                    None
                } else {
                    Some(Group {
                        id: g.id.clone(),
                        region_ids: remain,
                        name: g.name.clone(),
                    })
                }
            })
            .collect();
        self.selected_region_ids = self
            .selected_region_ids
            .difference(&dead_rids)
            .cloned()
            .collect();
        self.sort_groups();
        self.ensure_active_group();
        if self.pages.is_empty() {
            self.current_page_index = 0;
        } else if let Some(id) = keep_id {
            self.current_page_index = self
                .pages
                .iter()
                .position(|p| p.id == id)
                .unwrap_or(0);
        } else if let Some(old) = fallback_old {
            let new_idx = (0..old).filter(|i| !drop.contains(i)).count();
            self.current_page_index = new_idx.min(self.pages.len() - 1);
        } else {
            self.current_page_index = 0;
        }
        self.retain_memory_window();
        self.rebuild_rid_index();
        dead_pids
    }

    pub fn move_page(&mut self, from: usize, to: usize) {
        if from >= self.pages.len() || to >= self.pages.len() || from == to {
            return;
        }
        let page = self.pages.remove(from);
        self.pages.insert(to, page);
        if self.current_page_index == from {
            self.current_page_index = to;
        } else if from < self.current_page_index && to >= self.current_page_index {
            self.current_page_index -= 1;
        } else if from > self.current_page_index && to <= self.current_page_index {
            self.current_page_index += 1;
        }
        self.sort_groups();
        self.retain_memory_window();
        self.rebuild_rid_index();
    }

    /// 将 `moving` 整块 (保持相对顺序) 插到 `anchor` 之前/之后.
    pub fn move_pages_block(&mut self, moving: &[usize], anchor: usize, after: bool) {
        let n = self.pages.len();
        if n == 0 || moving.is_empty() || anchor >= n {
            return;
        }
        let moving_set: HashSet<usize> = moving.iter().copied().filter(|&i| i < n).collect();
        if moving_set.is_empty() || moving_set.contains(&anchor) {
            return;
        }
        let cur_id = self.pages.get(self.current_page_index).map(|p| p.id.clone());
        let raw_insert = if after { anchor + 1 } else { anchor };
        let insert_in_remaining =
            raw_insert - moving_set.iter().filter(|&&i| i < raw_insert).count();

        let mut remaining = Vec::with_capacity(n - moving_set.len());
        let mut block = Vec::with_capacity(moving_set.len());
        // 按原序抽出, 不按 moving 切片的乱序
        for (i, p) in self.pages.drain(..).enumerate() {
            if moving_set.contains(&i) {
                block.push(p);
            } else {
                remaining.push(p);
            }
        }
        let insert_at = insert_in_remaining.min(remaining.len());
        for (j, p) in block.into_iter().enumerate() {
            remaining.insert(insert_at + j, p);
        }
        self.pages = remaining;
        if let Some(id) = cur_id {
            if let Some(idx) = self.pages.iter().position(|p| p.id == id) {
                self.current_page_index = idx;
            } else if !self.pages.is_empty() {
                self.current_page_index = self.current_page_index.min(self.pages.len() - 1);
            } else {
                self.current_page_index = 0;
            }
        }
        self.sort_groups();
        self.retain_memory_window();
        self.rebuild_rid_index();
    }

    pub fn copy_page_at(&mut self, index: usize) -> Option<usize> {
        if index >= self.pages.len() {
            return None;
        }
        let src = &self.pages[index];
        let new_page_id = new_id();
        let stem = src
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("page");
        let suf = src
            .path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{s}"))
            .unwrap_or_default();
        let copy_name = if stem.ends_with("_copy") {
            format!("{stem}2{suf}")
        } else if stem.contains("_copy") {
            format!("{stem}_{}{suf}", &new_id()[..4])
        } else {
            format!("{stem}_copy{suf}")
        };
        let new_path = src.path.with_file_name(copy_name);
        let disk_path = match crate::page_cache::duplicate_disk_png(&src.disk_path) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let (img_w, img_h) = (src.width(), src.height());
        // 复制页不克隆像素; 若在窗口内再按需加载
        let mut ordered: Vec<Region> = src.regions.values().cloned().collect();
        ordered.sort_by_key(|r| (r.y0, r.y1));

        let mut page = Page {
            id: new_page_id.clone(),
            path: new_path,
            disk_path,
            image: None,
            img_w,
            img_h,
            regions: HashMap::new(),
        };
        let mut new_region_ids = Vec::new();
        for r in ordered {
            let rid = new_id();
            page.regions.insert(
                rid.clone(),
                Region {
                    id: rid.clone(),
                    page_id: new_page_id.clone(),
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind,
                    color: r.color,
                },
            );
            new_region_ids.push(rid);
        }
        let src_page_id = self.pages[index].id.clone();
        let insert_at = index + 1;
        self.pages.insert(insert_at, page);

        // 插到「原页最后一个组合」之后、「下一页组合」之前 (含手动调序时也不丢到末尾)
        let mut insert_group_at = None;
        for (i, g) in self.groups.iter().enumerate() {
            let belongs_src = g.region_ids.iter().any(|rid| {
                self.get_region(rid)
                    .is_some_and(|r| r.page_id == src_page_id)
            });
            if belongs_src {
                insert_group_at = Some(i + 1);
            }
        }
        let insert_group_at = insert_group_at.unwrap_or_else(|| {
            self.groups
                .iter()
                .enumerate()
                .find(|(_, g)| self.group_sort_key(g).0 > index)
                .map(|(i, _)| i)
                .unwrap_or(self.groups.len())
        });
        for (j, rid) in new_region_ids.iter().enumerate() {
            self.groups.insert(
                insert_group_at + j,
                Group {
                    id: new_id(),
                    region_ids: vec![rid.clone()],
                    name: String::new(),
                },
            );
        }
        if !self.groups_manual_order {
            self.sort_groups();
        }
        self.current_page_index = insert_at;
        self.retain_memory_window();
        self.rebuild_rid_index();
        Some(insert_at)
    }

    pub fn select_group(&mut self, gid: &str) -> Option<usize> {
        self.active_group_id = Some(gid.to_string());
        let g = self.active_group()?;
        let rids = g.region_ids.clone();
        self.selected_region_ids = rids.iter().cloned().collect();
        self.focus_page_for_regions(&rids)
    }

    /// Ctrl 点击输出组合: 将该组全部成员并入/移出多选; 普通点击等同 select_group.
    pub fn click_group(&mut self, gid: &str, ctrl: bool) -> Option<usize> {
        if !ctrl {
            return self.select_group(gid);
        }
        let Some(g) = self.groups.iter().find(|g| g.id == gid) else {
            return None;
        };
        let rids = g.region_ids.clone();
        if rids.is_empty() {
            self.active_group_id = Some(gid.to_string());
            return None;
        }
        let all_selected = rids
            .iter()
            .all(|rid| self.selected_region_ids.contains(rid));
        if all_selected {
            for rid in &rids {
                self.selected_region_ids.remove(rid);
            }
            if self.active_group_id.as_deref() == Some(gid) {
                self.active_group_id = self
                    .groups
                    .iter()
                    .find(|g| {
                        g.region_ids
                            .iter()
                            .any(|r| self.selected_region_ids.contains(r))
                    })
                    .map(|g| g.id.clone());
            }
        } else {
            for rid in &rids {
                self.selected_region_ids.insert(rid.clone());
            }
            self.active_group_id = Some(gid.to_string());
        }
        self.focus_page_for_regions(&rids)
    }

    fn focus_page_for_regions(&mut self, rids: &[String]) -> Option<usize> {
        let first = rids.first()?;
        let (pi, _) = self.find_region(first)?;
        let page = self.current_page()?;
        let on_page = rids.iter().any(|rid| page.regions.contains_key(rid));
        if !on_page {
            self.current_page_index = pi;
            Some(pi)
        } else {
            None
        }
    }

    pub fn click_region(&mut self, region_id: &str, ctrl: bool) {
        if ctrl {
            if self.selected_region_ids.contains(region_id) {
                self.selected_region_ids.remove(region_id);
            } else {
                self.selected_region_ids.insert(region_id.to_string());
            }
        } else {
            self.selected_region_ids = HashSet::from([region_id.to_string()]);
        }
        for g in &self.groups {
            if g.region_ids.iter().any(|x| x == region_id) {
                self.active_group_id = Some(g.id.clone());
                break;
            }
        }
    }

    /// 全选当前页全部原子块, 并尽量把 active_group 落到第一个块所属组合.
    pub fn select_all_current_page_regions(&mut self) {
        let Some(page) = self.current_page() else {
            return;
        };
        let ids: HashSet<String> = page.regions.keys().cloned().collect();
        let mut regs: Vec<_> = page.regions.values().cloned().collect();
        regs.sort_by_key(|r| (r.y0, r.y1));
        let first_rid = regs.first().map(|r| r.id.clone());
        self.selected_region_ids = ids;
        self.active_group_id = first_rid.and_then(|rid| {
            self.groups
                .iter()
                .find(|g| g.region_ids.iter().any(|id| id == &rid))
                .map(|g| g.id.clone())
        });
    }

    pub fn group_has_selected_region(&self, g: &Group) -> bool {
        g.region_ids
            .iter()
            .any(|rid| self.selected_region_ids.contains(rid))
    }

    pub fn click_blank(&mut self, ctrl: bool) {
        if !ctrl {
            self.selected_region_ids.clear();
        }
    }

    pub fn reorder_active_members(&mut self, new_ids: Vec<String>) {
        let Some(gid) = self.active_group_id.clone() else {
            return;
        };
        let existing: Vec<String> = self
            .groups
            .iter()
            .find(|g| g.id == gid)
            .map(|g| g.region_ids.clone())
            .unwrap_or_default();
        let mut final_ids = new_ids;
        for rid in existing {
            if !final_ids.contains(&rid) && self.get_region(&rid).is_some() {
                final_ids.push(rid);
            }
        }
        final_ids.retain(|rid| self.get_region(rid).is_some());
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
            g.region_ids = final_ids;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_page(h: u32) -> Page {
        Page {
            id: new_id(),
            path: PathBuf::from("p.png"),
            disk_path: PathBuf::from("p.png"),
            image: None,
            img_w: 80,
            img_h: h,
            regions: HashMap::new(),
        }
    }

    fn seed_bands(doc: &mut DocState, page_idx: usize, bands: &[(i32, i32)]) {
        let page_id = doc.pages[page_idx].id.clone();
        for (i, &(y0, y1)) in bands.iter().enumerate() {
            let rid = format!("r{page_idx}-{i}");
            doc.pages[page_idx].regions.insert(
                rid.clone(),
                Region {
                    id: rid.clone(),
                    page_id: page_id.clone(),
                    y0,
                    y1,
                    kind: "staff".into(),
                    color: COLORS[i % COLORS.len()].to_string(),
                },
            );
            doc.groups.push(Group {
                id: format!("g{page_idx}-{i}"),
                region_ids: vec![rid],
                name: String::new(),
            });
        }
        doc.rebuild_rid_index();
    }

    fn group_y0s(doc: &DocState) -> Vec<(usize, i32)> {
        doc.groups
            .iter()
            .map(|g| {
                let k = doc.group_top_key(g);
                (k.0, k.1)
            })
            .collect()
    }

    #[test]
    fn add_manual_block_inserts_by_top_y_when_manual_order() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(400));
        seed_bands(&mut doc, 0, &[(10, 20), (40, 50), (70, 80)]);
        doc.groups_manual_order = true;

        doc.add_manual_block(25, 30);
        assert_eq!(group_y0s(&doc), vec![(0, 10), (0, 25), (0, 40), (0, 70)]);

        doc.add_manual_block(1, 5);
        assert_eq!(
            group_y0s(&doc),
            vec![(0, 1), (0, 10), (0, 25), (0, 40), (0, 70)]
        );

        doc.add_manual_block(90, 95);
        assert_eq!(
            group_y0s(&doc),
            vec![(0, 1), (0, 10), (0, 25), (0, 40), (0, 70), (0, 90)]
        );
    }

    #[test]
    fn add_manual_block_stays_on_its_page_between_neighbors() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(400));
        doc.pages.push(stub_page(400));
        seed_bands(&mut doc, 0, &[(10, 20), (40, 50), (70, 80)]);
        seed_bands(&mut doc, 1, &[(10, 20), (40, 50)]);
        doc.groups_manual_order = true;
        doc.current_page_index = 0;

        doc.add_manual_block(55, 60);
        assert_eq!(
            group_y0s(&doc),
            vec![
                (0, 10),
                (0, 40),
                (0, 55),
                (0, 70),
                (1, 10),
                (1, 40)
            ]
        );
    }

    fn group_rids(doc: &DocState) -> Vec<Vec<String>> {
        let mut gs: Vec<( (usize, i32, i32), Vec<String> )> = doc
            .groups
            .iter()
            .map(|g| {
                let mut rids = g.region_ids.clone();
                rids.sort_by_key(|rid| doc.region_sort_key(rid));
                (doc.group_top_key(g), rids)
            })
            .collect();
        gs.sort_by_key(|(k, _)| *k);
        gs.into_iter().map(|(_, r)| r).collect()
    }

    #[test]
    fn pair_ungrouped_pairs_current_page_and_spills_odd_to_next() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(400));
        doc.pages.push(stub_page(400));
        seed_bands(
            &mut doc,
            0,
            &[(10, 20), (30, 40), (50, 60), (70, 80), (90, 100)],
        );
        seed_bands(
            &mut doc,
            1,
            &[(10, 20), (30, 40), (50, 60), (70, 80), (90, 100)],
        );
        doc.current_page_index = 0;
        assert_eq!(doc.pair_ungrouped().unwrap(), 3);
        assert_eq!(
            group_rids(&doc),
            vec![
                vec!["r0-0".to_string(), "r0-1".to_string()],
                vec!["r0-2".to_string(), "r0-3".to_string()],
                vec!["r0-4".to_string(), "r1-0".to_string()],
                vec!["r1-1".to_string()],
                vec!["r1-2".to_string()],
                vec!["r1-3".to_string()],
                vec!["r1-4".to_string()],
            ]
        );

        doc.current_page_index = 1;
        assert_eq!(doc.pair_ungrouped().unwrap(), 2);
        assert_eq!(
            group_rids(&doc),
            vec![
                vec!["r0-0".to_string(), "r0-1".to_string()],
                vec!["r0-2".to_string(), "r0-3".to_string()],
                vec!["r0-4".to_string(), "r1-0".to_string()],
                vec!["r1-1".to_string(), "r1-2".to_string()],
                vec!["r1-3".to_string(), "r1-4".to_string()],
            ]
        );
    }

    fn replace_page_regions(doc: &mut DocState, page_idx: usize, bands: &[(i32, i32)]) -> HashSet<String> {
        let old_ids: HashSet<String> = doc.pages[page_idx].regions.keys().cloned().collect();
        let page_id = doc.pages[page_idx].id.clone();
        doc.pages[page_idx].regions.clear();
        for (i, &(y0, y1)) in bands.iter().enumerate() {
            let rid = format!("n{page_idx}-{i}");
            doc.pages[page_idx].regions.insert(
                rid.clone(),
                Region {
                    id: rid,
                    page_id: page_id.clone(),
                    y0,
                    y1,
                    kind: "system".into(),
                    color: COLORS[i % COLORS.len()].to_string(),
                },
            );
        }
        doc.rebuild_rid_index();
        old_ids
    }

    #[test]
    fn upsert_page_groups_drops_stale_rids_from_redetect() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(400));
        seed_bands(&mut doc, 0, &[(10, 20), (40, 50), (70, 80)]);
        let old_ids = replace_page_regions(&mut doc, 0, &[(12, 22), (42, 52)]);
        doc.upsert_page_groups(0, &old_ids);
        let rids: Vec<String> = doc.groups.iter().flat_map(|g| g.region_ids.clone()).collect();
        assert_eq!(doc.groups.len(), 2);
        assert!(rids.iter().all(|id| id.starts_with("n0-")));
        assert!(doc.groups.iter().all(|g| doc.group_top_key(g).0 != usize::MAX));
    }

    #[test]
    fn upsert_page_groups_keeps_other_page_and_strips_cross_page_old_rids() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(400));
        doc.pages.push(stub_page(400));
        seed_bands(&mut doc, 0, &[(10, 20), (40, 50)]);
        seed_bands(&mut doc, 1, &[(10, 20), (40, 50)]);
        doc.groups.retain(|g| {
            g.region_ids != ["r0-1".to_string()] && g.region_ids != ["r1-0".to_string()]
        });
        doc.groups.push(Group {
            id: "cross".into(),
            region_ids: vec!["r0-1".into(), "r1-0".into()],
            name: String::new(),
        });
        let old_ids = replace_page_regions(&mut doc, 0, &[(12, 22), (42, 52)]);
        doc.upsert_page_groups(0, &old_ids);
        assert!(
            doc.groups
                .iter()
                .all(|g| g.region_ids.iter().all(|id| doc.find_region(id).is_some()))
        );
        assert!(doc.groups.iter().any(|g| g.region_ids == ["r1-0".to_string()]));
        assert!(doc.groups.iter().any(|g| g.region_ids == ["r1-1".to_string()]));
        assert_eq!(
            doc.groups
                .iter()
                .filter(|g| g.region_ids.iter().any(|id| id.starts_with("n0-")))
                .count(),
            2
        );
    }

    #[test]
    fn prune_dangling_skips_while_some_page_has_no_regions() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(400));
        doc.pages.push(stub_page(400));
        seed_bands(&mut doc, 0, &[(10, 20)]);
        doc.groups.push(Group {
            id: "pending".into(),
            region_ids: vec!["r1-future".into()],
            name: String::new(),
        });
        doc.prune_dangling_groups_if_hydrated();
        assert!(doc.groups.iter().any(|g| g.id == "pending"));
        seed_bands(&mut doc, 1, &[(10, 20)]);
        doc.prune_dangling_groups_if_hydrated();
        assert!(!doc.groups.iter().any(|g| g.id == "pending"));
    }

    #[test]
    fn compose_group_with_layout_applies_gap_and_extend() {
        let mut doc = DocState::new();
        let mut page = stub_page(100);
        page.image = Some(Arc::new(image::RgbImage::from_pixel(
            80,
            100,
            image::Rgb([250, 250, 250]),
        )));
        let page_id = page.id.clone();
        doc.pages.push(page);
        let r0 = Region {
            id: "r0".into(),
            page_id: page_id.clone(),
            y0: 0,
            y1: 29,
            kind: "system".into(),
            color: "#e74c3c".into(),
        };
        let r1 = Region {
            id: "r1".into(),
            page_id: page_id.clone(),
            y0: 30,
            y1: 59,
            kind: "system".into(),
            color: "#3498db".into(),
        };
        doc.pages[0].regions.insert(r0.id.clone(), r0);
        doc.pages[0].regions.insert(r1.id.clone(), r1);
        doc.rebuild_rid_index();
        doc.groups.push(Group {
            id: "g1".into(),
            region_ids: vec!["r0".into(), "r1".into()],
            name: String::new(),
        });

        // 无调整: 高度应等于两块之和 (各 30px).
        let plain = doc.compose_group("g1").unwrap();
        assert_eq!(plain.height(), 60);

        // r1 前插入 10px 间距, r1 底边再向外扩 5px.
        doc.set_block_layout(
            "g1",
            vec![
                BlockAdjust {
                    region_id: "r0".into(),
                    ..Default::default()
                },
                BlockAdjust {
                    region_id: "r1".into(),
                    extra_top: 0,
                    extra_bottom: 5,
                    gap_before: 10,
                    ..Default::default()
                },
            ],
        );
        let adjusted = doc.compose_group("g1").unwrap();
        assert_eq!(adjusted.height(), 60 + 10 + 5);
        assert_eq!(adjusted.width(), plain.width());

        // 顶边向内裁掉 8px 应减小总高.
        doc.set_block_layout(
            "g1",
            vec![
                BlockAdjust {
                    region_id: "r0".into(),
                    extra_top: -8,
                    ..Default::default()
                },
                BlockAdjust {
                    region_id: "r1".into(),
                    ..Default::default()
                },
            ],
        );
        let trimmed = doc.compose_group("g1").unwrap();
        assert_eq!(trimmed.height(), 60 - 8);

        // 全 0 调整应自动回退到「未设置」状态 (不占工程文件空间).
        doc.set_block_layout(
            "g1",
            vec![
                BlockAdjust {
                    region_id: "r0".into(),
                    ..Default::default()
                },
                BlockAdjust {
                    region_id: "r1".into(),
                    ..Default::default()
                },
            ],
        );
        assert!(doc.group_block_layout.get("g1").is_none());
    }

    fn named_stub(name: &str) -> Page {
        let mut p = stub_page(80);
        p.path = PathBuf::from(name);
        p
    }

    fn page_names(doc: &DocState) -> Vec<String> {
        doc.pages
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn move_pages_block_inserts_after_anchor() {
        let mut doc = DocState::new();
        for i in 0..6 {
            doc.pages.push(named_stub(&format!("{i}.png")));
        }
        doc.current_page_index = 2;
        let cur_id = doc.pages[2].id.clone();
        doc.move_pages_block(&[1, 2, 3], 5, true);
        assert_eq!(page_names(&doc), vec!["0.png", "4.png", "5.png", "1.png", "2.png", "3.png"]);
        assert_eq!(doc.pages.iter().position(|p| p.id == cur_id), Some(4));
    }

    #[test]
    fn move_pages_block_inserts_before_anchor() {
        let mut doc = DocState::new();
        for i in 0..5 {
            doc.pages.push(named_stub(&format!("{i}.png")));
        }
        doc.move_pages_block(&[3, 4], 0, false);
        assert_eq!(page_names(&doc), vec!["3.png", "4.png", "0.png", "1.png", "2.png"]);
        assert_eq!(doc.current_page_index, 2);
    }

    #[test]
    fn close_pages_at_prefers_next_then_prev() {
        let mut doc = DocState::new();
        for i in 0..6 {
            doc.pages.push(named_stub(&format!("{i}.png")));
        }
        seed_bands(&mut doc, 2, &[(10, 20)]);
        seed_bands(&mut doc, 4, &[(10, 20)]);
        doc.current_page_index = 2;
        let dead = doc.close_pages_at(&[2, 3]);
        assert_eq!(dead.len(), 2);
        assert_eq!(page_names(&doc), vec!["0.png", "1.png", "4.png", "5.png"]);
        assert_eq!(doc.pages[doc.current_page_index].path.file_name().unwrap(), "4.png");
        assert_eq!(doc.groups.len(), 1);

        doc.current_page_index = 3;
        let dead = doc.close_pages_at(&[3]);
        assert_eq!(dead.len(), 1);
        assert_eq!(page_names(&doc), vec!["0.png", "1.png", "4.png"]);
        assert_eq!(doc.pages[doc.current_page_index].path.file_name().unwrap(), "4.png");
    }

    #[test]
    fn close_page_at_matches_batch() {
        let mut doc = DocState::new();
        for i in 0..3 {
            doc.pages.push(named_stub(&format!("{i}.png")));
        }
        doc.current_page_index = 1;
        assert!(doc.close_page_at(1));
        assert_eq!(page_names(&doc), vec!["0.png", "2.png"]);
        assert_eq!(doc.current_page_index, 1);
    }

    #[test]
    fn global_guides_switch_uses_precomputed_defaults_and_can_turn_off() {
        let mut doc = DocState::new();
        doc.pages.push(stub_page(1440));
        seed_bands(&mut doc, 0, &[(10, 200), (300, 490)]);
        doc.seed_guide_defaults();
        assert_eq!(doc.group_guide_defaults.len(), 2);
        doc.apply_guides_global_on();
        assert!(doc.guides_global);
        let gid = doc.groups[0].id.clone();
        let g = doc.get_group_guides(&gid);
        assert_eq!(g.lines.len(), 1);
        let h = doc.group_preview_frame(&gid).unwrap().canvas_h as i32;
        let mid = h / 2;
        assert!(g.lines[0] > mid, "单根应略低于画布中线: y={} mid={}", g.lines[0], mid);
        doc.apply_guides_global_off();
        assert!(!doc.guides_global);
        assert!(doc.get_group_guides(&gid).lines.is_empty());
        assert!(!doc.group_guide_defaults.is_empty());
        doc.apply_guides_global_on();
        assert_eq!(doc.get_group_guides(&gid).lines.len(), 1);
    }

    #[test]
    fn render_final_mask_stays_on_scaled_stain() {
        // 高谱面会缩小装进 16:9 页面. 蒙版按编辑器习惯存在「预览画布 − 偏移」
        // 坐标系; 终稿先除以 content_scale 盖到未缩放拼合图, 再等比合成.
        let mut doc = DocState::new();
        doc.bg_aspect_w = 16;
        doc.bg_aspect_h = 9;
        let sw = 200u32;
        let sh = 250u32;
        let stain = (100u32, 200u32);
        let mut sheet = image::RgbImage::from_pixel(sw, sh, image::Rgb([180, 180, 180]));
        sheet.put_pixel(stain.0, stain.1, image::Rgb([255, 0, 0]));
        let mut page = stub_page(sh);
        page.img_w = sw;
        page.image = Some(Arc::new(sheet));
        let page_id = page.id.clone();
        doc.pages.push(page);
        doc.pages[0].regions.insert(
            "r0".into(),
            Region {
                id: "r0".into(),
                page_id,
                y0: 0,
                y1: (sh - 1) as i32,
                kind: "system".into(),
                color: "#e74c3c".into(),
            },
        );
        doc.rebuild_rid_index();
        doc.groups.push(Group {
            id: "g1".into(),
            region_ids: vec!["r0".into()],
            name: String::new(),
        });
        doc.bg_enabled = true;
        doc.bg_image = Some(Arc::new(image::RgbImage::from_pixel(800, 800, image::Rgb([10, 20, 30]))));

        let frame = doc.group_preview_frame("g1").unwrap();
        assert!(frame.content_scale < 1.0);
        let stored_x = ((stain.0 as f32) * frame.content_scale).round() as i32;
        let stored_y = ((stain.1 as f32) * frame.content_scale).round() as i32;
        doc.set_group_masks(
            "g1",
            vec![MaskRect {
                id: "cover".into(),
                x0: stored_x - 2,
                y0: stored_y - 2,
                x1: stored_x + 2,
                y1: stored_y + 2,
                brush_points: Vec::new(),
                brush_radius: 0,
                color: [255, 255, 255],
                poly_points: Vec::new(),
                opacity: 1.0,
                bound_block: None,
            }],
        );

        let out = doc.render_group_final("g1").unwrap().unwrap();
        let cx = (frame.hoff as i32 + stored_x).max(0) as u32;
        let cy = (frame.voff as i32 + stored_y).max(0) as u32;
        let covered = out.get_pixel(cx, cy);
        assert!(
            covered[0] > 200 && covered[1] > 200 && covered[2] > 200,
            "污点对应画布位置应为白蒙版, 得到 {covered:?} at ({cx},{cy})"
        );
        // 旧实现会把蒙版打在未缩放拼合图的 (stored_x, stored_y), 缩小后
        // 出现在更靠近原点处; 那里应仍是谱面灰, 不是白块.
        let ghost_x = (frame.hoff as f32 + stored_x as f32 * frame.content_scale).round() as u32;
        let ghost_y = (frame.voff as f32 + stored_y as f32 * frame.content_scale).round() as u32;
        if ghost_x != cx || ghost_y != cy {
            let ghost = out.get_pixel(ghost_x, ghost_y);
            assert!(
                ghost[0] < 220,
                "旧偏移位置不该被蒙上, 得到 {ghost:?} at ({ghost_x},{ghost_y})"
            );
        }
    }

}

