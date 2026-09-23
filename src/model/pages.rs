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
        if page.image.is_some() {
            return Ok(());
        }
        let path = page.disk_path.clone();
        let max_side = self.display_max_side();
        let (w, h, img) = crate::page_cache::load_rgb_display(&path, max_side)?;
        if let Some(page) = self.pages.get_mut(page_idx) {
            page.img_w = w;
            page.img_h = h;
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
            self.current_page_index = self.pages.iter().position(|p| p.id == id).unwrap_or(0);
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
}
