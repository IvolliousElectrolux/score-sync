//! 谱表识别、sidecar 灌入与按页重建组合.

use super::*;
use std::collections::{HashMap, HashSet};

use crate::staff_detect::{detect_bands, merge_system_bands, Band, LineBand, StaffPackMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaffPackApply {
    Unchanged,
    Applied,
    /// 设置已记下. 当前块是页内合并过的, 内存里没有合并前的谱行.
    NeedRedetect,
}

impl DocState {
    pub fn detect_page(&mut self, page_idx: usize, reset_groups: bool) {
        if self.pages.get(page_idx).is_some_and(|p| p.blank) {
            return;
        }
        if self
            .pages
            .get(page_idx)
            .map(|p| p.is_vector())
            .unwrap_or(false)
        {
            self.detect_vector_page(page_idx, reset_groups);
            return;
        }
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
        let atoms = bands
            .iter()
            .map(|b| LineBand {
                y0: b.y0,
                y1: b.y1,
                kind: b.kind.clone(),
                anchor: mask_tool::staff::band_staff_anchor(&full, b.y0, b.y1, self.ink_threshold),
            })
            .collect();
        self.staff_line_atoms.insert(page_id.clone(), atoms);
        let bands = if self.packs_within_page() {
            merge_system_bands(bands, self.staff_pack_lines)
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
            let page_regions: Vec<Region> =
                self.pages[page_idx].regions.values().cloned().collect();
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
            self.maybe_pack_across();
        }
    }

    /// 用已缓存的几何重做分块. 不改页的 point 尺寸, 也不写像素 sidecar.
    /// 返回是否认出了谱行.
    pub fn detect_vector_page(&mut self, page_idx: usize, reset_groups: bool) -> bool {
        let (scene, old_ids, page_id, page_h) = {
            let Some(page) = self.pages.get(page_idx) else {
                return false;
            };
            let Some(src) = page.vector.as_ref() else {
                return false;
            };
            (
                src.scene.clone(),
                page.regions.keys().cloned().collect::<HashSet<String>>(),
                page.id.clone(),
                page.img_h.max(1),
            )
        };
        let found = crate::vector_page::detect_vector_bands(&scene, self.margin);
        let staff_found = !found.is_empty();
        let atoms = if found.is_empty() {
            vec![LineBand {
                y0: 0,
                y1: page_h.saturating_sub(1) as i32,
                kind: "region".into(),
                anchor: None,
            }]
        } else {
            found
                .iter()
                .map(|b| LineBand {
                    y0: b.y0,
                    y1: b.y1,
                    kind: "system".into(),
                    anchor: b.anchor,
                })
                .collect()
        };
        self.staff_line_atoms.insert(page_id.clone(), atoms);
        let found = if staff_found && self.packs_within_page() {
            crate::vector_page::merge_staff_bands(found, self.staff_pack_lines)
        } else {
            found
        };
        let bands = if found.is_empty() {
            vec![crate::vector_page::StaffBand {
                y0: 0,
                y1: page_h.saturating_sub(1) as i32,
                anchor: None,
            }]
        } else {
            found
        };
        let mut regions = HashMap::new();
        let mut anchors = Vec::new();
        for (i, b) in bands.iter().enumerate() {
            let rid = new_id();
            let kind = if staff_found { "system" } else { "region" };
            regions.insert(
                rid.clone(),
                Region {
                    id: rid.clone(),
                    page_id: page_id.clone(),
                    y0: b.y0,
                    y1: b.y1,
                    kind: kind.into(),
                    color: COLORS[i % COLORS.len()].to_string(),
                },
            );
            anchors.push((rid, b.anchor));
        }
        if let Some(page) = self.pages.get_mut(page_idx) {
            if let Some(src) = page.vector.as_ref() {
                let (lw, lh) = src.logical_size();
                page.img_w = lw;
                page.img_h = lh;
            }
            page.regions = regions;
        }
        self.rebuild_rid_index();
        self.ingest_region_staff_anchors(anchors);
        if reset_groups {
            let page_regions: Vec<Region> =
                self.pages[page_idx].regions.values().cloned().collect();
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
            self.maybe_pack_across();
        }
        staff_found
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
            if page.is_vector() {
                if let Some(src) = page.vector.as_ref() {
                    let (lw, lh) = src.logical_size();
                    page.img_w = lw;
                    page.img_h = lh;
                }
            } else if file.img_w > 0 {
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
        if !file.line_bands.is_empty() {
            let atoms = file
                .line_bands
                .iter()
                .map(|b| LineBand {
                    y0: b.y0,
                    y1: b.y1,
                    kind: b.kind.clone(),
                    anchor: b.staff_anchor.as_ref().and_then(|a| a.y),
                })
                .collect();
            self.staff_line_atoms.insert(page_id, atoms);
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
            staff_pack_lines: self.staff_pack_lines,
            staff_pack_mode: self.staff_pack_mode,
            line_bands: self
                .staff_line_atoms
                .get(&page.id)
                .map(|bands| {
                    bands
                        .iter()
                        .map(|b| crate::detect_cache::CachedLineBand {
                            y0: b.y0,
                            y1: b.y1,
                            kind: b.kind.clone(),
                            staff_anchor: Some(crate::detect_cache::CachedStaffAnchor {
                                y: b.anchor,
                            }),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            regions: regions
                .into_iter()
                .map(|r| crate::detect_cache::CachedRegion {
                    id: r.id.clone(),
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind,
                    staff_anchor: self
                        .region_staff_anchors
                        .get(&r.id)
                        .map(|y| crate::detect_cache::CachedStaffAnchor { y: *y }),
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
        self.maybe_pack_across();
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
        let mut ordered: Vec<Region> = self.pages[page_idx].regions.values().cloned().collect();
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
        self.maybe_pack_across();
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
        self.maybe_pack_across();
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
        self.maybe_pack_across();
    }

    fn prune_missing_selection(&mut self) {
        let ids: Vec<String> = self.selected_region_ids.iter().cloned().collect();
        self.selected_region_ids = ids
            .into_iter()
            .filter(|id| self.get_region(id).is_some())
            .collect();
    }

    fn packs_within_page(&self) -> bool {
        !self.is_spread()
            && self.staff_pack_mode == StaffPackMode::WithinPage
            && self.staff_pack_lines >= 2
    }

    fn line_atoms_ready(&self) -> bool {
        self.pages
            .iter()
            .filter(|p| !p.blank && !p.regions.is_empty())
            .all(|p| {
                self.staff_line_atoms
                    .get(&p.id)
                    .is_some_and(|atoms| !atoms.is_empty())
            })
    }

    fn capture_line_atoms_from_regions(&mut self) {
        let mut next = Vec::new();
        for page in &self.pages {
            if page.blank {
                continue;
            }
            let mut atoms: Vec<LineBand> = page
                .regions
                .values()
                .map(|r| LineBand {
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind.clone(),
                    anchor: self.region_staff_anchors.get(&r.id).and_then(|y| *y),
                })
                .collect();
            atoms.sort_by_key(|a| (a.y0, a.y1));
            next.push((page.id.clone(), atoms));
        }
        for (id, atoms) in next {
            self.staff_line_atoms.insert(id, atoms);
        }
    }

    fn restore_pages_from_line_atoms(&mut self) {
        let page_ids: Vec<(usize, String)> = self
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.id.clone()))
            .collect();
        for (page_idx, page_id) in page_ids {
            let Some(atoms) = self.staff_line_atoms.get(&page_id).cloned() else {
                continue;
            };
            if atoms.is_empty() {
                continue;
            }
            let old_ids: HashSet<String> = self
                .pages
                .get(page_idx)
                .map(|p| p.regions.keys().cloned().collect())
                .unwrap_or_default();
            let mut regions = HashMap::new();
            let mut anchors = Vec::new();
            for (i, atom) in atoms.iter().enumerate() {
                let rid = new_id();
                regions.insert(
                    rid.clone(),
                    Region {
                        id: rid.clone(),
                        page_id: page_id.clone(),
                        y0: atom.y0,
                        y1: atom.y1,
                        kind: atom.kind.clone(),
                        color: COLORS[i % COLORS.len()].to_string(),
                    },
                );
                anchors.push((rid, atom.anchor));
            }
            if let Some(page) = self.pages.get_mut(page_idx) {
                page.regions = regions;
            }
            self.rebuild_rid_index();
            for id in &old_ids {
                self.region_staff_anchors.remove(id);
                self.region_edits.remove(id);
            }
            for (rid, anchor) in anchors {
                self.region_staff_anchors.insert(rid, anchor);
            }
            self.upsert_page_groups(page_idx, &old_ids);
        }
    }

    fn merge_systems_on_page(&mut self, page_idx: usize) {
        let count = self.staff_pack_lines as usize;
        if count < 2 {
            return;
        }
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let mut ordered: Vec<(String, i32, i32, String)> = page
            .regions
            .values()
            .map(|r| (r.id.clone(), r.y0, r.y1, r.kind.clone()))
            .collect();
        ordered.sort_by_key(|r| (r.1, r.2));
        let system_idxs: Vec<usize> = ordered
            .iter()
            .enumerate()
            .filter(|(_, r)| r.3 == "system")
            .map(|(i, _)| i)
            .collect();
        let mut dead = HashSet::new();
        let mut expands = Vec::new();
        let mut i = 0;
        while i + count <= system_idxs.len() {
            let chunk = &system_idxs[i..i + count];
            let y0 = ordered[chunk[0]].1;
            let y1 = ordered[*chunk.last().unwrap()].2;
            expands.push((ordered[chunk[0]].0.clone(), y0, y1));
            for &idx in &chunk[1..] {
                dead.insert(ordered[idx].0.clone());
            }
            for item in &ordered {
                if item.3 != "system" && item.1 >= y0 && item.2 <= y1 {
                    dead.insert(item.0.clone());
                }
            }
            i += count;
        }
        if expands.is_empty() {
            return;
        }
        if let Some(page) = self.pages.get_mut(page_idx) {
            for (id, y0, y1) in &expands {
                if let Some(region) = page.regions.get_mut(id) {
                    region.y0 = *y0;
                    region.y1 = *y1;
                }
            }
            for id in &dead {
                page.regions.remove(id);
            }
        }
        for (id, y0, y1) in &expands {
            self.region_staff_anchors
                .insert(id.clone(), Some((*y1 - *y0).max(0) / 2));
            if let Some(gid) = self
                .groups
                .iter()
                .find(|g| g.region_ids.iter().any(|rid| rid == id))
                .map(|g| g.id.clone())
            {
                self.group_masks.remove(&gid);
                self.group_block_layout.remove(&gid);
                self.group_voff_shift.remove(&gid);
                self.group_guides.remove(&gid);
                self.group_guide_defaults.remove(&gid);
            }
        }
        for id in &dead {
            self.region_staff_anchors.remove(id);
            self.region_edits.remove(id);
        }
        self.groups = std::mem::take(&mut self.groups)
            .into_iter()
            .filter_map(|mut g| {
                g.region_ids.retain(|id| !dead.contains(id));
                if g.region_ids.is_empty() {
                    None
                } else {
                    Some(g)
                }
            })
            .collect();
        self.rebuild_rid_index();
    }

    /// 记下行数和模式, 并按当前分块重放. 不重新扫像素.
    /// 页内合并过、又没有合并前谱行时, 只改设置, 返回 [`StaffPackApply::NeedRedetect`].
    pub fn apply_staff_pack(&mut self, lines: u32, mode: StaffPackMode) -> StaffPackApply {
        let lines = lines.clamp(1, 12);
        if self.staff_pack_lines.max(1) == lines && self.staff_pack_mode == mode {
            return StaffPackApply::Unchanged;
        }
        if self.is_spread() {
            self.staff_pack_lines = lines;
            self.staff_pack_mode = mode;
            return StaffPackApply::Applied;
        }
        let was_merged =
            self.staff_pack_mode == StaffPackMode::WithinPage && self.staff_pack_lines >= 2;
        let was_across =
            self.staff_pack_mode == StaffPackMode::AcrossPages && self.staff_pack_lines >= 2;
        if was_merged {
            if !self.line_atoms_ready() {
                self.staff_pack_lines = lines;
                self.staff_pack_mode = mode;
                return StaffPackApply::NeedRedetect;
            }
            self.restore_pages_from_line_atoms();
        } else if lines >= 2 || was_across {
            self.capture_line_atoms_from_regions();
        }
        self.staff_pack_lines = lines;
        self.staff_pack_mode = mode;
        if lines < 2 {
            if was_across {
                self.explode_pure_system_groups();
                self.seed_guide_defaults();
            }
            self.prune_missing_selection();
            self.ensure_active_group();
            return StaffPackApply::Applied;
        }
        self.explode_pure_system_groups();
        if mode == StaffPackMode::WithinPage {
            let n = self.pages.len();
            for i in 0..n {
                self.merge_systems_on_page(i);
            }
        } else {
            self.pack_ungrouped_systems(lines);
        }
        self.prune_missing_selection();
        self.ensure_active_group();
        self.seed_guide_defaults();
        StaffPackApply::Applied
    }
}
