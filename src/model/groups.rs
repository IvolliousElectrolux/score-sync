//! 输出组合的排序、合并、拆分与选中.

use super::*;
use std::collections::HashSet;

impl DocState {
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
        let insert_in_remaining = raw_insert - moving.iter().filter(|&&i| i < raw_insert).count();

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
            .chain(
                self.groups
                    .iter()
                    .flat_map(|g| g.region_ids.iter().cloned()),
            )
            .collect();
        self.region_staff_anchors
            .retain(|k, _| valid_rids.contains(k));
        self.prune_orphan_edits();
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

    pub(super) fn group_min_page_idx(&self, g: &Group) -> usize {
        g.region_ids
            .iter()
            .filter_map(|rid| self.find_region(rid).map(|(pi, _)| pi))
            .min()
            .unwrap_or(usize::MAX)
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
        for rid in &ids {
            self.remove_region_edit(rid);
        }
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
