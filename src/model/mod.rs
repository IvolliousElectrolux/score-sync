//! 数据模型与纯业务操作.
//!
//! 按主题拆开, [`DocState`] 仍是唯一文档状态:
//! - `types` 页 / 组合 / 区域
//! - `compose` 拼合预览与终稿
//! - `guides` 辅助线与谱表锚点
//! - `bg` 工程底色
//! - `pages` 页的增删复制与内存窗口
//! - `detect` 识别与 sidecar
//! - `groups` 组合排序、合并与选中

mod bg;
mod compose;
mod detect;
mod groups;
mod guides;
mod pages;
mod types;

#[cfg(test)]
mod tests;

pub use mask_tool::guide::GuideState;
pub use mask_tool::layout::BlockAdjust;
pub use compose::GroupRenderJob;
pub use types::{
    is_image_path, is_open_path, is_pdf_path, new_id, parse_color_hex, Group, Page, Region, COLORS,
    DEFAULT_INK_THRESHOLD, DEFAULT_MARGIN,
};
#[cfg(test)]
pub(crate) use compose::compose_parts_impl;
pub(crate) use types::crop_band_fast;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use image::RgbImage;

use crate::staff_detect::StaffGrouping;
use mask_tool::color_prefs::MaskColorPrefs;
use mask_tool::mask::MaskRect;
use score_video::model::TimelineSnapshot;

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
    /// `bg_image` 每次被替换 (`set_project_bg_arc`/`clear_project_bg`) 时自增,
    /// 供 GUI 侧给「底色 GPU 贴图」做缓存判重: 完整底色只备份这一份,
    /// 贴图按目标页裁切后再缩放, 见 [`mask_tool::gui::BlockBgTile::from_full`].
    pub bg_gen: u64,
    /// 视频面板时间轴的纯数据快照 (实际编辑态在 `score_video::ScoreVideoApp`
    /// 里, 这里只是保存/载入工程时的中转载体).
    pub video_state: TimelineSnapshot,
    /// region_id → page index, 避免 find_region 每次扫全部页.
    pub(crate) rid_page: HashMap<String, usize>,
    /// 交互预览图最长边 (像素). `0` 表示用 [`crate::page_cache::DEFAULT_DISPLAY_MAX_SIDE`].
    pub display_max_side: u32,
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
            display_max_side: self.display_max_side,
        }
    }

    /// 已加载显示代理的唯一像素字节 (同一 `Arc` 不计两次).
    pub fn loaded_image_bytes_unique(
        &self,
        seen: &mut std::collections::HashSet<usize>,
    ) -> (usize, u64) {
        let mut n = 0usize;
        let mut bytes = 0u64;
        for p in &self.pages {
            if let Some(img) = p.image.as_ref() {
                n += 1;
                bytes += crate::mem::rgb_arc_unique_bytes(img, seen);
            }
        }
        (n, bytes)
    }

    pub fn display_max_side(&self) -> u32 {
        if self.display_max_side == 0 {
            crate::page_cache::DEFAULT_DISPLAY_MAX_SIDE
        } else {
            self.display_max_side
        }
    }

    /// 按画布视口更新预览最长边. 抖动小于 48px 忽略, 避免每帧重载.
    pub fn set_display_max_side(&mut self, side: u32) {
        let side = side.clamp(960, 3840);
        let cur = self.display_max_side();
        if (side as i32 - cur as i32).abs() < 48 {
            return;
        }
        self.display_max_side = side;
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
}
