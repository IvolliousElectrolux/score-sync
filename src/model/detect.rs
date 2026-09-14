//! 谱表识别、sidecar 灌入与按页重建组合.

use super::*;
use std::collections::{HashMap, HashSet};

use crate::staff_detect::{detect_bands, Band};

impl DocState {
    pub fn detect_page(&mut self, page_idx: usize, reset_groups: bool) {
        let (path, mem, is_full, old_ids, page_id) = {
            let Some(page) = self.pages.get(page_idx) else {
                return;
            };
            (
                page.disk_path.clone(),
                page.image.clone(),
                page.is_full_res(),
                page.regions.keys().cloned().collect::<HashSet<String>>(),
                page.id.clone(),
            )
        };
        let max_side = self.display_max_side();
        let full: Arc<RgbImage> = if is_full {
            mem.unwrap()
        } else if path.is_file() {
            match crate::page_cache::load_rgb(&path) {
                Ok(img) => Arc::new(img),
                Err(_) => return,
            }
        } else if let Some(img) = mem {
            img
        } else {
            return;
        };
        let (w, h) = full.dimensions();
        let bands = detect_bands(&full, self.ink_threshold, self.margin);
        let bands = if bands.is_empty() {
            vec![Band {
                y0: 0,
                y1: h.saturating_sub(1) as i32,
                kind: "region".into(),
            }]
        } else {
            bands
        };
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
        let anchors: Vec<(String, Option<i32>)> = regions
            .values()
            .map(|r| {
                (
                    r.id.clone(),
                    mask_tool::staff::band_staff_anchor(&full, r.y0, r.y1, self.ink_threshold),
                )
            })
            .collect();
        let proxy = if w.max(h) > max_side {
            Arc::new(crate::page_cache::shrink_rgb_max(&full, max_side))
        } else {
            full.clone()
        };
        drop(full);
        if let Some(page) = self.pages.get_mut(page_idx) {
            page.img_w = w;
            page.img_h = h;
            page.image = Some(proxy);
            page.regions = regions;
        }
        self.rebuild_rid_index();
        self.ingest_region_staff_anchors(anchors);
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
}
