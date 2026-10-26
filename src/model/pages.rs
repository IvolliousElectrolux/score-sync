//! 页图内存窗口与增删复制移动页.

use super::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use image::RgbImage;

impl DocState {
    /// 确保显示用代理图在内存中 (按 [`Self::display_max_side`] 缩小).
    pub fn ensure_image(&mut self, page_idx: usize) -> Result<(), String> {
        let Some(page) = self.pages.get(page_idx) else {
            return Err("页不存在".into());
        };
        if page.blank {
            return Ok(());
        }
        if page.image.is_some() {
            if let Some(src) = page.vector.as_ref() {
                let (lw, lh) = src.logical_size();
                if let Some(page) = self.pages.get_mut(page_idx) {
                    page.img_w = lw;
                    page.img_h = lh;
                }
            }
            return Ok(());
        }
        let path = page.disk_path.clone();
        let logical = page.vector.as_ref().map(|s| s.logical_size());
        let max_side = self.display_max_side();
        let (w, h, img) = crate::page_cache::load_rgb_display(&path, max_side)?;
        if let Some(page) = self.pages.get_mut(page_idx) {
            if let Some((lw, lh)) = logical {
                page.img_w = lw;
                page.img_h = lh;
            } else {
                page.img_w = w;
                page.img_h = h;
            }
            page.image = Some(Arc::new(img));
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn ensure_images(&mut self, indices: &[usize]) -> Result<(), String> {
        for &i in indices {
            self.ensure_image(i)?;
        }
        Ok(())
    }

    pub fn unload_page_image(&mut self, page_idx: usize) {
        if let Some(page) = self.pages.get_mut(page_idx) {
            let _ = page.image.take();
        }
    }

    /// 按当前页体积决定内存窗口半径 (高清页小于默认 ±4).
    pub fn memory_window_radius(&self) -> usize {
        let b = self
            .current_page()
            .map(|p| p.estimated_bytes())
            .unwrap_or(0);
        crate::page_cache::window_radius_for_bytes(b)
    }

    /// 只留当前页附近的解码像素, 半径见 [`Self::memory_window_radius`].
    /// 缺的页在这里同步解码. 拖完页序不要走这条, 用
    /// [`Self::release_outside_memory_window`], 再让界面后台补图.
    pub fn retain_memory_window(&mut self) {
        self.retain_window(self.current_page_index, self.memory_window_radius());
    }

    /// 卸掉当前页窗口以外的像素, 不解码. 松手调序时同步解码旁边几页原图
    /// 会把界面卡住; 缺的页由 `request_page_window` 在后台补.
    pub fn release_outside_memory_window(&mut self) {
        let n = self.pages.len();
        if n == 0 {
            return;
        }
        let center = self.current_page_index.min(n - 1);
        let radius = self.memory_window_radius();
        let lo = center.saturating_sub(radius);
        let hi = (center + radius).min(n - 1);
        for i in 0..n {
            if i < lo || i > hi {
                self.unload_page_image(i);
            }
        }
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn ensure_group_pages(&mut self, group_id: &str) -> Result<(), String> {
        let idxs = self.page_indices_for_group(group_id);
        self.ensure_images(&idxs)
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
            vector: None,
            blank: false,
        };
        self.pages.push(page);
        let idx = self.pages.len() - 1;
        if switch_to {
            self.current_page_index = idx;
        }
        if self.is_spread() {
            self.install_full_page_region(idx, true);
            self.rebuild_spreads();
        } else {
            self.detect_page(idx, true);
        }
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
            vector: None,
            blank: false,
        };
        self.pages.push(page);
        let idx = self.pages.len() - 1;
        if switch_to {
            self.current_page_index = idx;
        }
        if self.is_spread() {
            self.install_full_page_region(idx, true);
            self.rebuild_spreads();
            self.retain_memory_window();
        } else if run_detect {
            crate::trace::log(&format!("doc: detect_page idx={idx} 开始"));
            self.detect_page(idx, true);
            crate::trace::log(&format!("doc: detect_page idx={idx} 结束"));
            self.retain_memory_window();
        }
        Ok(idx)
    }

    /// 登记一页矢量. `preview` 只是会话里的显示缓存, 不作为文档像素.
    /// 区域坐标是 point. 返回是否认出了谱行.
    pub fn add_vector_page(
        &mut self,
        display: PathBuf,
        preview_png: PathBuf,
        source: crate::vector_page::VectorSource,
        switch_to: bool,
    ) -> Result<(usize, bool), String> {
        let (w, h) = source.logical_size();
        let page = Page {
            id: new_id(),
            path: display,
            disk_path: preview_png,
            image: None,
            img_w: w,
            img_h: h,
            regions: HashMap::new(),
            vector: Some(source),
            blank: false,
        };
        self.pages.push(page);
        let idx = self.pages.len() - 1;
        if switch_to {
            self.current_page_index = idx;
        }
        let found = if self.is_spread() {
            self.install_full_page_region(idx, true);
            self.rebuild_spreads();
            false
        } else {
            self.detect_vector_page(idx, true)
        };
        self.retain_memory_window();
        Ok((idx, found))
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
        // 对开要拿删除前的左右成员和重配结果比, 右页没了就清蒙版.
        // 这里先留着原组合, 交给 rebuild_spreads.
        if !self.is_spread() {
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
            self.sort_groups();
            self.ensure_active_group();
        }
        self.selected_region_ids = self
            .selected_region_ids
            .difference(&dead_rids)
            .cloned()
            .collect();
        if self.pages.is_empty() {
            self.current_page_index = 0;
        } else if let Some(id) = keep_id {
            self.current_page_index = self.pages.iter().position(|p| p.id == id).unwrap_or(0);
        } else if let Some(old) = fallback_old {
            let new_idx = (0..old).filter(|i| !drop.contains(i)).count();
            self.current_page_index = new_idx.min(self.pages.len() - 1);
        } else {
            self.current_page_index = 0;
        }
        self.retain_memory_window();
        self.rebuild_rid_index();
        if self.is_spread() {
            self.rebuild_spreads();
        }
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
        // 先按新页序重建区域索引, 再排序. 索引还指着旧位置时,
        // 每个组合都会把全部页扫一遍.
        self.rebuild_rid_index();
        self.sort_groups();
        self.release_outside_memory_window();
        if self.is_spread() {
            self.rebuild_spreads();
        }
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
        let cur_id = self
            .pages
            .get(self.current_page_index)
            .map(|p| p.id.clone());
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
        self.rebuild_rid_index();
        self.sort_groups();
        self.release_outside_memory_window();
        if self.is_spread() {
            self.rebuild_spreads();
        }
    }

    /// 复制一页的文件与区域, 不插入. 位图会另写一份会话 PNG.
    /// 返回新页, 以及 `(旧区域 id, 新区域 id)`, 按 y 排序.
    fn duplicate_page(src: &Page) -> Option<(Page, Vec<(String, String)>)> {
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
        if src.blank {
            return Some((
                Page {
                    id: new_page_id,
                    path: new_path,
                    disk_path: std::path::PathBuf::new(),
                    image: None,
                    img_w: src.img_w,
                    img_h: src.img_h,
                    regions: HashMap::new(),
                    vector: None,
                    blank: true,
                },
                Vec::new(),
            ));
        }
        let disk_path = if src.vector.is_some() {
            src.disk_path.clone()
        } else {
            crate::page_cache::duplicate_disk_png(&src.disk_path).ok()?
        };
        let (img_w, img_h) = (src.width(), src.height());
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
            vector: src.vector.clone(),
            blank: false,
        };
        let mut region_map = Vec::with_capacity(ordered.len());
        for r in ordered {
            let old_rid = r.id.clone();
            let rid = new_id();
            region_map.push((old_rid, rid.clone()));
            page.regions.insert(
                rid.clone(),
                Region {
                    id: rid,
                    page_id: new_page_id.clone(),
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind,
                    color: r.color,
                },
            );
        }
        Some((page, region_map))
    }

    pub fn copy_page_at(&mut self, index: usize) -> Option<usize> {
        let ids = self.copy_pages_beside(&[index], index, true)?;
        let id = ids.into_iter().next()?;
        self.pages.iter().position(|p| p.id == id)
    }

    /// 按 `order` 的顺序复制这些页, 紧贴插在 `anchor` 左侧 (`after == false`) 或右侧.
    /// 返回新页 id, 顺序与 `order` 相同.
    pub fn copy_pages_beside(
        &mut self,
        order: &[usize],
        anchor: usize,
        after: bool,
    ) -> Option<Vec<String>> {
        let n = self.pages.len();
        if order.is_empty() || anchor >= n || order.iter().any(|&i| i >= n) {
            return None;
        }
        let anchor_id = self.pages[anchor].id.clone();
        let mut copies = Vec::with_capacity(order.len());
        let mut dups = Vec::with_capacity(order.len());
        for &i in order {
            let src_id = self.pages[i].id.clone();
            let (page, regions) = Self::duplicate_page(&self.pages[i])?;
            dups.push(PageDup {
                src_id,
                new_id: page.id.clone(),
                regions,
            });
            copies.push(page);
        }
        let new_ids: Vec<String> = copies.iter().map(|p| p.id.clone()).collect();
        let mut insert_at = self.pages.iter().position(|p| p.id == anchor_id)?;
        if after {
            insert_at += 1;
        }
        for (j, page) in copies.into_iter().enumerate() {
            self.pages.insert(insert_at + j, page);
        }
        self.current_page_index = insert_at.min(self.pages.len().saturating_sub(1));
        self.retain_memory_window();
        self.rebuild_rid_index();
        if self.is_spread() {
            self.rebuild_spreads();
            let pair = self.current_page_index / 2;
            if let Some(g) = self.groups.get(pair) {
                self.active_group_id = Some(g.id.clone());
            }
        }
        self.adopt_copied_pages(&dups, &anchor_id, after);
        Some(new_ids)
    }

    /// 新页带走原块上的修图, 谱表锚点和识别行. 谱表模式再按组合复制分块操作.
    fn adopt_copied_pages(&mut self, copies: &[PageDup], anchor_id: &str, after: bool) {
        for copy in copies {
            if let Some(atoms) = self.staff_line_atoms.get(&copy.src_id).cloned() {
                self.staff_line_atoms.insert(copy.new_id.clone(), atoms);
            }
            for (old_rid, new_rid) in &copy.regions {
                if let Some(anchor) = self.region_staff_anchors.get(old_rid).cloned() {
                    self.region_staff_anchors.insert(new_rid.clone(), anchor);
                }
                self.clone_region_edit_to(old_rid, new_rid, &copy.new_id);
            }
        }
        if !self.is_spread() {
            self.clone_staff_copy_groups(copies, anchor_id, after);
        }
    }

    /// 整组都在被复制的页里时, 组合连同蒙版, 微调, 辅助线和名称一起复制.
    /// 组里还有没被复制的页上的块时, 被复制的那些块各自单独成组, 只带走这块自己的微调和绑定蒙版.
    fn clone_staff_copy_groups(&mut self, copies: &[PageDup], anchor_id: &str, after: bool) {
        let mut rid_map: HashMap<String, String> = HashMap::new();
        for copy in copies {
            for (old_rid, new_rid) in &copy.regions {
                rid_map.insert(old_rid.clone(), new_rid.clone());
            }
        }
        let src_groups: Vec<(String, String, Vec<String>)> = self
            .groups
            .iter()
            .map(|g| (g.id.clone(), g.name.clone(), g.region_ids.clone()))
            .collect();
        let mut placed: HashSet<String> = HashSet::new();
        let mut pending: Vec<PendingGroup> = Vec::new();
        for (gid, name, rids) in &src_groups {
            let inside: Vec<String> = rids
                .iter()
                .filter(|rid| rid_map.contains_key(*rid))
                .cloned()
                .collect();
            if inside.is_empty() {
                continue;
            }
            let complete = rids.iter().all(|rid| rid_map.contains_key(rid));
            if complete && rids.iter().all(|rid| !placed.contains(rid)) {
                let masks = self.group_masks.get(gid).cloned().unwrap_or_default();
                let layout = self
                    .group_block_layout
                    .get(gid)
                    .cloned()
                    .unwrap_or_default();
                pending.push(PendingGroup {
                    name: name.clone(),
                    region_ids: rids
                        .iter()
                        .filter_map(|rid| rid_map.get(rid).cloned())
                        .collect(),
                    masks: mapped_masks(&masks, &rid_map, None),
                    layout: mapped_layout(&layout, rids, &rid_map),
                    voff: self.group_voff_shift.get(gid).copied().unwrap_or(0),
                    guides: self.group_guides.get(gid).cloned(),
                    guide_defaults: self.group_guide_defaults.get(gid).cloned(),
                });
                placed.extend(rids.iter().cloned());
                continue;
            }
            for old_rid in inside {
                if !placed.insert(old_rid.clone()) {
                    continue;
                }
                let masks = self.group_masks.get(gid).cloned().unwrap_or_default();
                let layout = self
                    .group_block_layout
                    .get(gid)
                    .cloned()
                    .unwrap_or_default();
                let old_rids = vec![old_rid.clone()];
                pending.push(PendingGroup {
                    name: String::new(),
                    region_ids: rid_map.get(&old_rid).cloned().into_iter().collect(),
                    masks: mapped_masks(&masks, &rid_map, Some(&old_rid)),
                    layout: mapped_layout(&layout, &old_rids, &rid_map),
                    voff: 0,
                    guides: None,
                    guide_defaults: None,
                });
            }
        }
        for copy in copies {
            for (old_rid, new_rid) in &copy.regions {
                if !placed.insert(old_rid.clone()) {
                    continue;
                }
                pending.push(PendingGroup {
                    name: String::new(),
                    region_ids: vec![new_rid.clone()],
                    masks: Vec::new(),
                    layout: Vec::new(),
                    voff: 0,
                    guides: None,
                    guide_defaults: None,
                });
            }
        }
        if pending.is_empty() {
            return;
        }
        let insert_at = self.staff_copy_insert_at(anchor_id, after);
        let mut cursor = insert_at;
        for group in pending {
            let gid = new_id();
            self.groups.insert(
                cursor,
                Group {
                    id: gid.clone(),
                    region_ids: group.region_ids,
                    name: group.name,
                },
            );
            cursor += 1;
            self.set_group_masks(&gid, group.masks);
            self.set_block_layout(&gid, group.layout);
            if group.voff != 0 {
                self.set_group_voff_shift(&gid, group.voff);
            }
            if let Some(guides) = group.guides {
                self.set_group_guides(&gid, guides);
            }
            if let Some(defaults) = group.guide_defaults {
                self.group_guide_defaults.insert(gid, defaults);
            }
        }
        if !self.groups_manual_order {
            self.sort_groups();
        }
    }

    fn staff_copy_insert_at(&self, anchor_id: &str, after: bool) -> usize {
        let belongs = |g: &Group| {
            g.region_ids
                .iter()
                .any(|rid| self.get_region(rid).is_some_and(|r| r.page_id == anchor_id))
        };
        let anchor_index = self
            .pages
            .iter()
            .position(|p| p.id == anchor_id)
            .unwrap_or(0);
        if after {
            let mut at = None;
            for (i, g) in self.groups.iter().enumerate() {
                if belongs(g) {
                    at = Some(i + 1);
                }
            }
            at.unwrap_or_else(|| {
                self.groups
                    .iter()
                    .enumerate()
                    .find(|(_, g)| self.group_sort_key(g).0 > anchor_index)
                    .map(|(i, _)| i)
                    .unwrap_or(self.groups.len())
            })
        } else {
            self.groups.iter().position(belongs).unwrap_or_else(|| {
                self.groups
                    .iter()
                    .enumerate()
                    .find(|(_, g)| self.group_sort_key(g).0 >= anchor_index)
                    .map(|(i, _)| i)
                    .unwrap_or(self.groups.len())
            })
        }
    }
}

struct PageDup {
    src_id: String,
    new_id: String,
    regions: Vec<(String, String)>,
}

struct PendingGroup {
    name: String,
    region_ids: Vec<String>,
    masks: Vec<MaskRect>,
    layout: Vec<BlockAdjust>,
    voff: i64,
    guides: Option<GuideState>,
    guide_defaults: Option<GuideState>,
}

fn mapped_masks(
    masks: &[MaskRect],
    rid_map: &HashMap<String, String>,
    only_bound: Option<&str>,
) -> Vec<MaskRect> {
    let mut out = Vec::new();
    for src in masks {
        if let Some(old) = only_bound {
            if src.bound_block.as_deref() != Some(old) {
                continue;
            }
        }
        let mut mask = src.clone();
        mask.id = new_id();
        if let Some(bound) = mask.bound_block.as_ref() {
            match rid_map.get(bound) {
                Some(next) => mask.bound_block = Some(next.clone()),
                None => continue,
            }
        }
        out.push(mask);
    }
    out
}

fn mapped_layout(
    layout: &[BlockAdjust],
    old_rids: &[String],
    rid_map: &HashMap<String, String>,
) -> Vec<BlockAdjust> {
    let mut out = Vec::new();
    for old in old_rids {
        let Some(next) = rid_map.get(old) else {
            continue;
        };
        let mut adj = layout
            .iter()
            .find(|a| a.region_id == *old)
            .cloned()
            .unwrap_or_default();
        adj.region_id = next.clone();
        out.push(adj);
    }
    out
}
