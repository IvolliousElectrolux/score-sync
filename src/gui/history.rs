//! 分块撤重.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn capture_crop_snap(&self, page_ids: &[String]) -> CropSnap {
        let mut page_regions = HashMap::new();
        for pid in page_ids {
            if let Some(p) = self.doc.pages.iter().find(|p| p.id == *pid) {
                page_regions.insert(pid.clone(), p.regions.clone());
            }
        }
        CropSnap {
            page_regions,
            pages: None,
            current_page_index: None,
            group_masks: None,
            groups: self.doc.groups.clone(),
            selected_region_ids: self.doc.selected_region_ids.clone(),
            active_group_id: self.doc.active_group_id.clone(),
            groups_manual_order: self.doc.groups_manual_order,
            staff_grouping: self.doc.staff_grouping,
            group_guides: self.doc.group_guides.clone(),
            group_guide_defaults: self.doc.group_guide_defaults.clone(),
            guides_global: self.doc.guides_global,
            guides_sync_positions: self.doc.guides_sync_positions,
        }
    }

    pub(super) fn capture_crop_snap_pages(&self) -> CropSnap {
        // 结构撤重只保留路径 + 元数据, 不克隆整幅位图
        let pages = self
            .doc
            .pages
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p.image = None;
                p
            })
            .collect();
        CropSnap {
            page_regions: HashMap::new(),
            pages: Some(pages),
            current_page_index: Some(self.doc.current_page_index),
            group_masks: Some(self.doc.group_masks.clone()),
            groups: self.doc.groups.clone(),
            selected_region_ids: self.doc.selected_region_ids.clone(),
            active_group_id: self.doc.active_group_id.clone(),
            groups_manual_order: self.doc.groups_manual_order,
            staff_grouping: self.doc.staff_grouping,
            group_guides: self.doc.group_guides.clone(),
            group_guide_defaults: self.doc.group_guide_defaults.clone(),
            guides_global: self.doc.guides_global,
            guides_sync_positions: self.doc.guides_sync_positions,
        }
    }

    pub(super) fn push_crop_undo_for(&mut self, page_ids: &[String]) {
        let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) else {
            return;
        };
        if page_ids.is_empty() {
            return;
        }
        let snap = self.capture_crop_snap(page_ids);
        let h = self.crop_histories.entry(cur).or_default();
        h.undo.push(snap);
        if h.undo.len() > CROP_HISTORY_LIMIT {
            h.undo.remove(0);
        }
        h.redo.clear();
    }

    pub(super) fn push_crop_undo_current(&mut self) {
        let Some(id) = self.doc.current_page().map(|p| p.id.clone()) else {
            return;
        };
        self.push_crop_undo_for(&[id]);
    }

    pub(super) fn push_crop_undo_all_pages(&mut self) {
        let ids: Vec<String> = self.doc.pages.iter().map(|p| p.id.clone()).collect();
        self.push_crop_undo_for(&ids);
    }

    pub(super) fn push_crop_undo_page_structure(&mut self) {
        let snap = self.capture_crop_snap_pages();
        let h = &mut self.page_struct_history;
        h.undo.push(snap);
        if h.undo.len() > CROP_HISTORY_LIMIT {
            h.undo.remove(0);
        }
        h.redo.clear();
    }

    pub(super) fn apply_crop_snap(&mut self, snap: CropSnap) {
        if let Some(pages) = snap.pages {
            self.doc.pages = pages;
            if let Some(idx) = snap.current_page_index {
                self.doc.current_page_index = idx.min(self.doc.pages.len().saturating_sub(1));
            }
            if let Some(masks) = snap.group_masks {
                self.doc.group_masks = masks;
            }
            self.doc.retain_window(
                self.doc.current_page_index,
                crate::page_cache::WINDOW_RADIUS,
            );
        } else {
            for (pid, regions) in snap.page_regions {
                if let Some(p) = self.doc.pages.iter_mut().find(|p| p.id == pid) {
                    p.regions = regions;
                }
            }
        }
        self.doc.groups = snap.groups;
        self.doc.selected_region_ids = snap.selected_region_ids;
        self.doc.active_group_id = snap.active_group_id;
        self.doc.groups_manual_order = snap.groups_manual_order;
        self.doc.staff_grouping = snap.staff_grouping;
        self.doc.group_guides = snap.group_guides;
        self.doc.group_guide_defaults = snap.group_guide_defaults;
        self.doc.guides_global = snap.guides_global;
        self.doc.guides_sync_positions = snap.guides_sync_positions;
        self.doc.ensure_active_group();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
    }

    pub(super) fn undo_crop(&mut self, cx: &mut Context<Self>) {
        // 1) 当前页的 regions 撤重
        if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
            if let Some(h) = self.crop_histories.get_mut(&cur) {
                if let Some(prev) = h.undo.pop() {
                    let ids: Vec<String> = prev.page_regions.keys().cloned().collect();
                    let now = self.capture_crop_snap(&ids);
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.redo.push(now);
                    }
                    self.apply_crop_snap(prev);
                    self.status = "已撤回.".into();
                    self.hint = self.status.clone();
                    self.after_doc_change(cx);
                    return;
                }
            }
        }
        // 2) 删页 / 导入等结构撤重
        if let Some(prev) = self.page_struct_history.undo.pop() {
            if self.pdf_importing {
                self.abandon_pdf_import();
            }
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.redo.push(now);
            self.apply_crop_snap(prev);
            self.status = "已撤回页操作.".into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
            return;
        }
        self.status = "没有可撤回的操作.".into();
        cx.notify();
    }

    pub(super) fn redo_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
            if let Some(h) = self.crop_histories.get_mut(&cur) {
                if let Some(next) = h.redo.pop() {
                    let ids: Vec<String> = next.page_regions.keys().cloned().collect();
                    let now = self.capture_crop_snap(&ids);
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.push(now);
                        if h.undo.len() > CROP_HISTORY_LIMIT {
                            h.undo.remove(0);
                        }
                    }
                    self.apply_crop_snap(next);
                    self.status = "已重做.".into();
                    self.hint = self.status.clone();
                    self.after_doc_change(cx);
                    return;
                }
            }
        }
        if let Some(next) = self.page_struct_history.redo.pop() {
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.undo.push(now);
            if self.page_struct_history.undo.len() > CROP_HISTORY_LIMIT {
                self.page_struct_history.undo.remove(0);
            }
            self.apply_crop_snap(next);
            self.status = "已重做页操作.".into();
            self.hint = self.status.clone();
            self.refresh_render(cx);
            return;
        }
        self.status = "没有可重做的操作.".into();
        cx.notify();
    }

    pub(super) fn undo_action(&mut self, cx: &mut Context<Self>) {
        match self.side_tool {
            SideTool::Crop => self.undo_crop(cx),
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.undo(cx));
            }
            SideTool::Video => {
                self.score_video.update(cx, |v, cx| v.undo(cx));
            }
            SideTool::Project => {}
        }
    }

    pub(super) fn redo_action(&mut self, cx: &mut Context<Self>) {
        match self.side_tool {
            SideTool::Crop => self.redo_crop(cx),
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.redo(cx));
            }
            SideTool::Video => {
                self.score_video.update(cx, |v, cx| v.redo(cx));
            }
            SideTool::Project => {}
        }
    }
}
