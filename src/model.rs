//! 数据模型与纯业务操作 (对照 app.py 的 Region / Page / Group / MainWindow 逻辑).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use image::RgbImage;

use crate::staff_detect::{detect_bands, Band};
use mask_tool::mask::MaskRect;
use score_video::model::TimelineSnapshot;

pub const COLORS: &[&str] = &[
    "#e74c3c", "#3498db", "#2ecc71", "#f39c12", "#9b59b6", "#1abc9c", "#e67e22",
    "#2980b9", "#16a085", "#c0392b",
];

pub const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp", ".webp"];

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
    pub path: PathBuf,
    pub image: RgbImage,
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

    pub fn height(&self) -> u32 {
        self.image.height()
    }

    pub fn width(&self) -> u32 {
        self.image.width()
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

pub fn parse_color_hex(s: &str) -> u32 {
    let s = s.trim().trim_start_matches('#');
    u32::from_str_radix(s, 16).unwrap_or(0x3498db)
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
    /// 组合蒙版: key = group_id, 坐标相对该组竖向拼合图
    pub group_masks: HashMap<String, Vec<MaskRect>>,
    /// 蒙版白色不透明度
    pub mask_opacity: f32,
    /// 用户已手动拖拽调序「输出组合」; 为 true 时不再自动按页/y 排序
    pub groups_manual_order: bool,
    /// 工程级底色层 (底层); 不改写页图, 导出/终稿合成时才叠上
    pub bg_enabled: bool,
    pub bg_image: Option<RgbImage>,
    /// 仅用于 UI 显示来源路径
    pub bg_source_path: Option<PathBuf>,
    pub bg_aspect_w: u32,
    pub bg_aspect_h: u32,
    /// 视频面板时间轴的纯数据快照 (实际编辑态在 `score_video::ScoreVideoApp`
    /// 里, 这里只是保存/载入工程时的中转载体).
    pub video_state: TimelineSnapshot,
}

impl DocState {
    pub fn new() -> Self {
        Self {
            margin: 20,
            ink_threshold: 200,
            mask_opacity: 0.72,
            bg_aspect_w: 2560,
            bg_aspect_h: 1440,
            ..Default::default()
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

    /// 裁切某页上的区域条带 (整宽).
    pub fn crop_region(&self, region_id: &str) -> Option<image::RgbImage> {
        let (pi, r) = self.find_region(region_id)?;
        let page = self.pages.get(pi)?;
        let w = page.width();
        let y0 = r.y0.max(0) as u32;
        let y1 = (r.y1 as u32).min(page.height().saturating_sub(1));
        if y1 < y0 {
            return None;
        }
        Some(image::imageops::crop_imm(&page.image, 0, y0, w, y1 - y0 + 1).to_image())
    }

    /// 按组内成员顺序竖向拼合 (与导出一致, 不含蒙版).
    pub fn compose_group(&self, group_id: &str) -> Option<image::RgbImage> {
        let g = self.groups.iter().find(|g| g.id == group_id)?;
        let mut parts: Vec<image::RgbImage> = Vec::new();
        for rid in &g.region_ids {
            if let Some(crop) = self.crop_region(rid) {
                parts.push(crop);
            }
        }
        if parts.is_empty() {
            return None;
        }
        let max_w = parts.iter().map(|p| p.width()).max().unwrap();
        if parts.len() == 1 && parts[0].width() == max_w {
            return Some(parts.remove(0));
        }
        let total_h: u32 = parts.iter().map(|p| p.height()).sum();
        let mut combined =
            image::RgbImage::from_pixel(max_w, total_h, image::Rgb([255, 255, 255]));
        let mut yy = 0u32;
        for p in &parts {
            let src = if p.width() != max_w {
                let mut canvas =
                    image::RgbImage::from_pixel(max_w, p.height(), image::Rgb([255, 255, 255]));
                image::imageops::replace(&mut canvas, p, 0, 0);
                canvas
            } else {
                p.clone()
            };
            image::imageops::replace(&mut combined, &src, 0, yy as i64);
            yy += p.height();
        }
        Some(combined)
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
        self.bg_image = Some(image);
        self.bg_source_path = source;
        self.bg_aspect_w = aspect_w;
        self.bg_aspect_h = aspect_h;
        self.bg_enabled = true;
        Ok(())
    }

    /// 取消工程底色层.
    pub fn clear_project_bg(&mut self) {
        self.bg_enabled = false;
        self.bg_image = None;
        self.bg_source_path = None;
    }

    /// 拼合图预览 (供蒙版/视频面板显示): 若已启用工程底色, 叠加底色预览
    /// (仅在需要补边时改变画布高度, 不烧入蒙版). 返回 (预览图, 谱面在预览图
    /// 中的纵向偏移量, 供调用方换算蒙版坐标).
    pub fn compose_group_preview(&self, group_id: &str) -> Option<(RgbImage, i64)> {
        let sheet = self.compose_group(group_id)?;
        if !self.bg_enabled {
            return Some((sheet, 0));
        }
        let Some(bg) = self.bg_image.as_ref() else {
            return Some((sheet, 0));
        };
        match apply_bg::process::composite_preview(&sheet, bg, self.bg_aspect_w, self.bg_aspect_h)
        {
            Ok((canvas, voff)) => Some((canvas, voff)),
            Err(_) => Some((sheet, 0)),
        }
    }

    /// 终稿合成: 拼合 → 蒙版 → (可选) 底色底层裁切.
    pub fn render_group_final(&self, group_id: &str) -> Result<Option<RgbImage>, String> {
        let Some(mut combined) = self.compose_group(group_id) else {
            return Ok(None);
        };
        let masks = self.get_group_masks(group_id);
        if !masks.is_empty() {
            mask_tool::mask::apply_masks_rgb(&mut combined, masks, self.mask_opacity);
        }
        if self.bg_enabled {
            let Some(bg) = self.bg_image.as_ref() else {
                return Err("已启用底色但缺少底色图".into());
            };
            combined = apply_bg::process::composite_and_crop(
                &combined,
                bg,
                self.bg_aspect_w,
                self.bg_aspect_h,
            )?;
        }
        Ok(Some(combined))
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
        for page in &mut self.pages {
            if page.regions.contains_key(rid) {
                return page.regions.get_mut(rid);
            }
        }
        None
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
    pub fn reorder_groups(&mut self, from: usize, to: usize) {
        if from == to || from >= self.groups.len() || to >= self.groups.len() {
            return;
        }
        let g = self.groups.remove(from);
        self.groups.insert(to, g);
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
    }

    /// 加载一页 RGB 图并自动识别. `switch_to`: 是否切到新页.
    pub fn add_page(&mut self, path: PathBuf, image: RgbImage, switch_to: bool) -> usize {
        let page = Page {
            id: new_id(),
            path,
            image,
            regions: HashMap::new(),
        };
        self.pages.push(page);
        let idx = self.pages.len() - 1;
        self.detect_page(idx, true);
        if switch_to {
            self.current_page_index = idx;
        }
        idx
    }

    pub fn detect_page(&mut self, page_idx: usize, reset_groups: bool) {
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let old_ids: HashSet<String> = page.regions.keys().cloned().collect();
        let bands = detect_bands(&page.image, self.ink_threshold, self.margin);
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
            self.ensure_active_group();
        }
    }

    pub fn detect_all(&mut self) {
        let n = self.pages.len();
        for i in 0..n {
            self.detect_page(i, true);
        }
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
                "请至少选中 2 个原子块再合并.\n(可切换标签页后 Ctrl 继续多选以实现跨页组合)",
            );
        }
        ids.sort_by_key(|rid| self.region_sort_key(rid));
        let id_set: HashSet<String> = ids.iter().cloned().collect();
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
        new_groups.push(g_new);
        self.groups = new_groups;
        self.sort_groups();
        self.active_group_id = Some(gid);
        Ok(ids.len())
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
        let g = self.active_group_mut().unwrap();
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
        Ok(added)
    }

    pub fn ungroup_active(&mut self) -> Result<(), &'static str> {
        let Some(g) = self.active_group() else {
            return Err("请选择含多个成员的组合.");
        };
        if g.region_ids.len() <= 1 {
            return Err("请选择含多个成员的组合.");
        }
        let idx = self.groups.iter().position(|x| x.id == g.id).unwrap();
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
        self.groups.push(Group {
            id: new_id(),
            region_ids: vec![rid.clone()],
            name: String::new(),
        });
        self.sort_groups();
        self.selected_region_ids = HashSet::from([rid]);
        format!("P{page_no} 新建手动块 y={a}-{b} h={}.", b - a + 1)
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
        if index >= self.pages.len() {
            return false;
        }
        let page = self.pages.remove(index);
        let dead: HashSet<String> = page.regions.keys().cloned().collect();
        self.groups = self
            .groups
            .iter()
            .filter_map(|g| {
                let remain: Vec<String> = g
                    .region_ids
                    .iter()
                    .filter(|x| !dead.contains(*x))
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
            .difference(&dead)
            .cloned()
            .collect();
        self.sort_groups();
        self.ensure_active_group();
        if self.pages.is_empty() {
            self.current_page_index = 0;
        } else if self.current_page_index >= self.pages.len() {
            self.current_page_index = self.pages.len() - 1;
        } else if index < self.current_page_index {
            self.current_page_index -= 1;
        }
        true
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
        let image = src.image.clone();
        let mut ordered: Vec<Region> = src.regions.values().cloned().collect();
        ordered.sort_by_key(|r| (r.y0, r.y1));

        let mut page = Page {
            id: new_page_id.clone(),
            path: new_path,
            image,
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
        let insert_at = index + 1;
        self.pages.insert(insert_at, page);
        for rid in &new_region_ids {
            self.groups.push(Group {
                id: new_id(),
                region_ids: vec![rid.clone()],
                name: String::new(),
            });
        }
        self.sort_groups();
        self.current_page_index = insert_at;
        Some(insert_at)
    }

    pub fn select_group(&mut self, gid: &str) -> Option<usize> {
        self.active_group_id = Some(gid.to_string());
        let g = self.active_group()?;
        let rids = g.region_ids.clone();
        self.selected_region_ids = rids.iter().cloned().collect();
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
