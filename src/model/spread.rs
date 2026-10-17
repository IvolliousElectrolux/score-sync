//! 总谱双页对开: 全页区域, 按页序两两成组.

use super::*;
use std::collections::HashSet;

pub const SPREAD_KIND: &str = "page";

fn only_region_id(page: &Page) -> Option<String> {
    if page.regions.len() != 1 {
        return None;
    }
    page.regions.keys().next().cloned()
}

fn is_full_page_region(page: &Page) -> bool {
    let h = page.img_h.max(1) as i32;
    let Some(r) = page.regions.values().next() else {
        return false;
    };
    page.regions.len() == 1 && r.y0 == 0 && r.y1 == h - 1 && r.page_id == page.id
}

impl DocState {
    pub fn is_spread(&self) -> bool {
        self.sheet_layout == SheetLayout::Spread
    }

    /// 切到对开: 每页收成一个全页区域, 按页序两两成组. 旧蒙版和微调丢掉.
    pub fn enter_spread(&mut self) {
        self.sheet_layout = SheetLayout::Spread;
        self.group_masks.clear();
        self.group_block_layout.clear();
        self.group_voff_shift.clear();
        self.group_guides.clear();
        self.group_guide_defaults.clear();
        self.region_staff_anchors.clear();
        self.guides_global = false;
        self.groups.clear();
        self.selected_region_ids.clear();
        let n = self.pages.len();
        for i in 0..n {
            self.install_full_page_region(i, true);
        }
        self.rebuild_spreads();
    }

    /// 切回谱表分块: 清掉区域和组合, 由用户重新识别.
    pub fn leave_spread(&mut self) {
        self.sheet_layout = SheetLayout::Staff;
        for page in &mut self.pages {
            page.regions.clear();
        }
        self.groups.clear();
        self.group_masks.clear();
        self.group_block_layout.clear();
        self.group_voff_shift.clear();
        self.group_guides.clear();
        self.group_guide_defaults.clear();
        self.region_staff_anchors.clear();
        self.selected_region_ids.clear();
        self.active_group_id = None;
        self.groups_manual_order = false;
        self.rebuild_rid_index();
    }

    /// `force` 时丢掉现有区域再建全页块. 已经是全页则保留区域 id.
    pub fn install_full_page_region(&mut self, page_idx: usize, force: bool) {
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        if !force && is_full_page_region(page) {
            return;
        }
        let page_id = page.id.clone();
        let h = page.img_h.max(1) as i32;
        let old: Vec<String> = page.regions.keys().cloned().collect();
        let rid = new_id();
        let color = COLORS[page_idx % COLORS.len()].to_string();
        if let Some(page) = self.pages.get_mut(page_idx) {
            page.regions.clear();
            page.regions.insert(
                rid.clone(),
                Region {
                    id: rid,
                    page_id,
                    y0: 0,
                    y1: h - 1,
                    kind: SPREAD_KIND.into(),
                    color,
                },
            );
        }
        for id in old {
            self.region_edits.remove(&id);
            self.region_staff_anchors.remove(&id);
        }
    }

    /// 按当前页序重配对. 组 id 跟左页走; 左右成员都没变时保留蒙版和微调.
    pub fn rebuild_spreads(&mut self) {
        if !self.is_spread() {
            return;
        }
        self.drop_empty_spread_books();
        let n = self.pages.len();
        for i in 0..n {
            if self.pages[i].blank {
                if let Some(page) = self.pages.get_mut(i) {
                    page.regions.clear();
                }
                continue;
            }
            self.install_full_page_region(i, false);
        }
        let mut old: Vec<(String, String, Vec<String>)> = Vec::new();
        for g in &self.groups {
            let Some(rid) = g.region_ids.first() else {
                continue;
            };
            let Some(pid) = self.get_region(rid).map(|r| r.page_id.clone()) else {
                continue;
            };
            old.push((pid, g.id.clone(), g.region_ids.clone()));
        }
        let mut pairs: Vec<(String, Vec<String>)> = Vec::new();
        let mut i = 0;
        while i < n {
            let left_pid = self.pages[i].id.clone();
            let mut members = Vec::new();
            if !self.pages[i].blank {
                if let Some(left_rid) = only_region_id(&self.pages[i]) {
                    members.push(left_rid);
                }
            }
            if i + 1 < n && !self.pages[i + 1].blank {
                if let Some(right_rid) = only_region_id(&self.pages[i + 1]) {
                    members.push(right_rid);
                }
            }
            // 组 id 跟第一张有内容的页走. 左页是空位时, 右页仍能对上原来的组.
            let key = members
                .first()
                .and_then(|rid| self.get_region(rid).map(|r| r.page_id.clone()))
                .unwrap_or(left_pid);
            pairs.push((key, members));
            i += 2;
        }
        let mut kept: HashSet<String> = HashSet::new();
        let mut groups = Vec::with_capacity(pairs.len());
        let mut clear_ids = Vec::new();
        for (left_pid, members) in pairs {
            if let Some((_, gid, old_members)) = old.iter().find(|(p, ..)| p == &left_pid) {
                let gid = gid.clone();
                if old_members != &members {
                    clear_ids.push(gid.clone());
                }
                kept.insert(gid.clone());
                groups.push(Group {
                    id: gid,
                    region_ids: members,
                    name: String::new(),
                });
            } else {
                let gid = new_id();
                kept.insert(gid.clone());
                groups.push(Group {
                    id: gid,
                    region_ids: members,
                    name: String::new(),
                });
            }
        }
        for id in clear_ids {
            self.drop_group_extras(&id);
        }
        self.group_masks.retain(|k, _| kept.contains(k));
        self.group_block_layout.retain(|k, _| kept.contains(k));
        self.group_voff_shift.retain(|k, _| kept.contains(k));
        self.group_guides.retain(|k, _| kept.contains(k));
        self.group_guide_defaults.retain(|k, _| kept.contains(k));
        let active = self.active_group_id.clone();
        self.groups = groups;
        self.groups_manual_order = true;
        if active.as_ref().is_some_and(|id| kept.contains(id)) {
            self.active_group_id = active;
        } else {
            self.active_group_id = self.groups.first().map(|g| g.id.clone());
        }
        self.rebuild_rid_index();
        self.selected_region_ids
            .retain(|id| self.rid_page.contains_key(id));
    }

    /// 一册左右都是空页时删掉这一册. 末页单独一张空页时, 右侧本来就是空位, 同样删掉.
    fn drop_empty_spread_books(&mut self) {
        let n = self.pages.len();
        if n == 0 {
            return;
        }
        let mut drop = vec![false; n];
        let mut i = 0;
        while i + 1 < n {
            if self.pages[i].blank && self.pages[i + 1].blank {
                drop[i] = true;
                drop[i + 1] = true;
            }
            i += 2;
        }
        if n % 2 == 1 && self.pages[n - 1].blank {
            drop[n - 1] = true;
        }
        if !drop.iter().any(|d| *d) {
            return;
        }
        let cur = self.current_page_index;
        let mut kept_before = 0usize;
        let mut kept = Vec::with_capacity(n);
        for (i, page) in self.pages.drain(..).enumerate() {
            if drop[i] {
                continue;
            }
            if i < cur {
                kept_before += 1;
            }
            kept.push(page);
        }
        self.pages = kept;
        if self.pages.is_empty() {
            self.current_page_index = 0;
        } else {
            self.current_page_index = kept_before.min(self.pages.len() - 1);
        }
    }

    fn drop_group_extras(&mut self, id: &str) {
        self.group_masks.remove(id);
        self.group_block_layout.remove(id);
        self.group_voff_shift.remove(id);
        self.group_guides.remove(id);
        self.group_guide_defaults.remove(id);
    }

    /// 成员 `(region_id, 宽, 高)`, 宽取页宽或修图画布宽.
    pub fn group_member_sizes(&self, group_id: &str) -> Vec<(String, u32, u32)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let (pi, r) = self.find_region(rid)?;
                let page = self.pages.get(pi)?;
                let (edit_w, edit_h) = self
                    .region_edit_size(rid)
                    .map(|(w, h)| (Some(w), Some(h)))
                    .unwrap_or((None, None));
                let w = edit_w.unwrap_or(page.width()).max(1);
                let h = edit_h.unwrap_or((r.y1 - r.y0 + 1).max(0) as u32).max(1);
                Some((rid.clone(), w, h))
            })
            .collect()
    }

    /// 对开拼合图宽高. 不是对开时退回单栏宽和竖向总高.
    pub fn spread_sheet_size(&self, group_id: &str) -> (u32, u32) {
        let sizes = self.group_member_sizes(group_id);
        if sizes.is_empty() {
            return (1, 1);
        }
        if self.is_spread() {
            let (_, w, h) =
                mask_tool::layout::spread_layout(&sizes, self.get_block_layout(group_id));
            return (w.max(1), h.max(1));
        }
        let w = sizes.iter().map(|(_, w, _)| *w).max().unwrap_or(1);
        let heights: Vec<(String, u32)> = sizes.iter().map(|(id, _, h)| (id.clone(), *h)).collect();
        let h = mask_tool::layout::sheet_height(&heights, self.get_block_layout(group_id));
        (w.max(1), h.max(1))
    }

    /// 在 `index` 插入一张空页. 只占页序, 不生成白图, 和奇数末页旁边的空位一样.
    /// 记下相邻页的尺寸, 只给界面和工程用, 成片不画这一页.
    /// 返回空页留下的位置. 这一对因此变成两页都空时, 整对被删掉, 返回 `None`.
    pub fn insert_blank_page(&mut self, index: usize) -> Result<Option<usize>, String> {
        let index = index.min(self.pages.len());
        let (w, h) = self
            .pages
            .get(index.saturating_sub(1))
            .or_else(|| self.pages.get(index))
            .map(|p| (p.img_w.max(1), p.img_h.max(1)))
            .unwrap_or((1, 1));
        let name = format!("blank_{}.png", new_id());
        let id = new_id();
        let page = Page {
            id: id.clone(),
            path: std::path::PathBuf::from(&name),
            disk_path: std::path::PathBuf::new(),
            image: None,
            img_w: w,
            img_h: h,
            regions: HashMap::new(),
            vector: None,
            blank: true,
        };
        self.pages.insert(index, page);
        self.current_page_index = index;
        if self.is_spread() {
            self.rebuild_spreads();
        }
        Ok(self.pages.iter().position(|p| p.id == id))
    }
}
