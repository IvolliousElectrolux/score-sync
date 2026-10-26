//! 分块撤重.

use super::ScoreSyncApp;
use super::*;

impl ScoreSyncApp {
    fn bump_undo_seq(&mut self) -> u64 {
        self.crop_undo_seq = self.crop_undo_seq.wrapping_add(1);
        self.crop_undo_seq
    }

    pub(super) fn capture_crop_snap(&mut self, page_ids: &[String]) -> CropSnap {
        let seq = self.bump_undo_seq();
        let mut page_regions = HashMap::new();
        for pid in page_ids {
            if let Some(p) = self.doc.pages.iter().find(|p| p.id == *pid) {
                page_regions.insert(pid.clone(), p.regions.clone());
            }
        }
        CropSnap {
            seq,
            page_regions,
            pages: None,
            current_page_index: None,
            group_masks: None,
            groups: self.doc.groups.clone(),
            selected_region_ids: self.doc.selected_region_ids.clone(),
            active_group_id: self.doc.active_group_id.clone(),
            groups_manual_order: self.doc.groups_manual_order,
            staff_grouping: self.doc.staff_grouping,
            staff_pack_lines: self.doc.staff_pack_lines,
            staff_pack_mode: self.doc.staff_pack_mode,
            group_guides: self.doc.group_guides.clone(),
            group_guide_defaults: self.doc.group_guide_defaults.clone(),
            guides_global: self.doc.guides_global,
            guides_sync_positions: self.doc.guides_sync_positions,
        }
    }

    pub(super) fn capture_crop_snap_pages(&mut self) -> CropSnap {
        let seq = self.bump_undo_seq();
        // 结构撤重只保留路径 + 元数据, 不克隆整幅位图
        let pages = self
            .doc
            .pages
            .iter()
            .map(|p| crate::model::Page {
                id: p.id.clone(),
                path: p.path.clone(),
                disk_path: p.disk_path.clone(),
                image: None,
                img_w: p.img_w,
                img_h: p.img_h,
                regions: p.regions.clone(),
                vector: p.vector.clone(),
                blank: p.blank,
            })
            .collect();
        CropSnap {
            seq,
            page_regions: HashMap::new(),
            pages: Some(pages),
            current_page_index: Some(self.doc.current_page_index),
            group_masks: Some(self.doc.group_masks.clone()),
            groups: self.doc.groups.clone(),
            selected_region_ids: self.doc.selected_region_ids.clone(),
            active_group_id: self.doc.active_group_id.clone(),
            groups_manual_order: self.doc.groups_manual_order,
            staff_grouping: self.doc.staff_grouping,
            staff_pack_lines: self.doc.staff_pack_lines,
            staff_pack_mode: self.doc.staff_pack_mode,
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
        self.clear_crop_redos();
        let h = self.crop_histories.entry(cur).or_default();
        h.undo.push(snap);
        if h.undo.len() > CROP_HISTORY_LIMIT {
            h.undo.remove(0);
        }
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
        self.clear_crop_redos();
        let h = &mut self.page_struct_history;
        h.undo.push(snap);
        if h.undo.len() > CROP_HISTORY_LIMIT {
            h.undo.remove(0);
        }
    }

    fn clear_crop_redos(&mut self) {
        self.page_struct_history.redo.clear();
        for history in self.crop_histories.values_mut() {
            history.redo.clear();
        }
    }

    pub(super) fn apply_crop_snap(&mut self, snap: CropSnap) {
        if let Some(mut pages) = snap.pages {
            let mut images: HashMap<String, std::sync::Arc<image::RgbImage>> = HashMap::new();
            for page in self.doc.pages.drain(..) {
                if let Some(image) = page.image {
                    images.insert(page.id, image);
                }
            }
            for page in &mut pages {
                if page.image.is_none() {
                    if let Some(image) = images.remove(&page.id) {
                        page.image = Some(image);
                    }
                }
            }
            self.doc.pages = pages;
            if let Some(idx) = snap.current_page_index {
                self.doc.current_page_index = idx.min(self.doc.pages.len().saturating_sub(1));
            }
            if let Some(masks) = snap.group_masks {
                self.doc.group_masks = masks;
            }
            // 快照里没有像素. 把还在窗口里的旧图接回去, 缺的页交给后台,
            // 不要在这一下把原图解码进界面线程.
            self.doc.release_outside_memory_window();
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
        self.doc.staff_pack_lines = snap.staff_pack_lines;
        self.doc.staff_pack_mode = snap.staff_pack_mode;
        self.doc.group_guides = snap.group_guides;
        self.doc.group_guide_defaults = snap.group_guide_defaults;
        self.doc.guides_global = snap.guides_global;
        self.doc.guides_sync_positions = snap.guides_sync_positions;
        self.doc.rebuild_rid_index();
        self.doc.ensure_active_group();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
    }

    /// 画布上正在显示的像素. 页 id 可以变 (粘贴出的新页和源页共用同一份图).
    pub(super) fn shown_page_token(&self) -> Option<(usize, u32, u32)> {
        let page = self.doc.current_page()?;
        let ptr = page.image.as_ref().map(|img| std::sync::Arc::as_ptr(img) as usize)?;
        if self.render_image.is_none() {
            return None;
        }
        let w = page.width();
        let h = page.height();
        if w != self.img_w || h != self.img_h {
            return None;
        }
        Some((ptr, w, h))
    }

    pub(super) fn finish_page_structure_edit(
        &mut self,
        before: Option<(usize, u32, u32)>,
        cx: &mut Context<Self>,
    ) {
        let after = self.shown_page_token();
        let same = before.is_some() && before == after;
        let size_changed = self.doc.current_page().is_some_and(|page| {
            page.width() != self.img_w || page.height() != self.img_h
        });
        self.request_page_window(cx);
        if self.page_organize.is_some() {
            self.sync_organize_after_doc(cx);
        }
        self.after_doc_change(cx);
        // 尺寸没变时 after_doc_change 不会重做贴图. 像素其实换了才补一次.
        if !same && !size_changed {
            self.refresh_render(cx);
        }
    }

    fn latest_region_undo_seq(&self) -> Option<u64> {
        let id = &self.doc.current_page()?.id;
        self.crop_histories
            .get(id)?
            .undo
            .last()
            .map(|snap| snap.seq)
    }

    fn latest_struct_undo_seq(&self) -> Option<u64> {
        self.page_struct_history.undo.last().map(|snap| snap.seq)
    }

    fn latest_region_redo_seq(&self) -> Option<u64> {
        let id = &self.doc.current_page()?.id;
        self.crop_histories
            .get(id)?
            .redo
            .last()
            .map(|snap| snap.seq)
    }

    fn latest_struct_redo_seq(&self) -> Option<u64> {
        self.page_struct_history.redo.last().map(|snap| snap.seq)
    }

    pub(super) fn undo_crop(&mut self, cx: &mut Context<Self>) {
        let struct_newer = match (self.latest_struct_undo_seq(), self.latest_region_undo_seq()) {
            (Some(page), Some(region)) => page > region,
            (Some(_), None) => true,
            _ => false,
        };
        if struct_newer {
            self.undo_page_structure(cx);
            return;
        }
        // 当前页的 regions 撤重
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
        self.undo_page_structure(cx);
    }

    fn undo_page_structure(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.page_struct_history.undo.pop() {
            if self.pdf_importing {
                self.abandon_pdf_import();
            }
            let shown = self.shown_page_token();
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.redo.push(now);
            self.apply_crop_snap(prev);
            self.status = "已撤回页操作.".into();
            self.hint = self.status.clone();
            self.finish_page_structure_edit(shown, cx);
            return;
        }
        self.status = "没有可撤回的操作.".into();
        cx.notify();
    }

    pub(super) fn redo_crop(&mut self, cx: &mut Context<Self>) {
        let struct_newer = match (self.latest_struct_redo_seq(), self.latest_region_redo_seq()) {
            (Some(page), Some(region)) => page > region,
            (Some(_), None) => true,
            _ => false,
        };
        if struct_newer {
            self.redo_page_structure(cx);
            return;
        }
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
        self.redo_page_structure(cx);
    }

    fn redo_page_structure(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.page_struct_history.redo.pop() {
            let shown = self.shown_page_token();
            let now = self.capture_crop_snap_pages();
            self.page_struct_history.undo.push(now);
            if self.page_struct_history.undo.len() > CROP_HISTORY_LIMIT {
                self.page_struct_history.undo.remove(0);
            }
            self.apply_crop_snap(next);
            self.status = "已重做页操作.".into();
            self.hint = self.status.clone();
            self.finish_page_structure_edit(shown, cx);
            return;
        }
        self.status = "没有可重做的操作.".into();
        cx.notify();
    }

    pub(super) fn undo_action(&mut self, cx: &mut Context<Self>) {
        match self.side_tool {
            SideTool::Crop => self.undo_crop(cx),
            SideTool::Mask if self.photo_open() => {
                self.photo_edit.update(cx, |p, cx| p.undo(cx));
            }
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.undo(cx));
            }
            SideTool::Video => {
                self.score_video.update(cx, |v, cx| v.undo(cx));
            }
            SideTool::Project => self.undo_bg(cx),
        }
    }

    pub(super) fn redo_action(&mut self, cx: &mut Context<Self>) {
        match self.side_tool {
            SideTool::Crop => self.redo_crop(cx),
            SideTool::Mask if self.photo_open() => {
                self.photo_edit.update(cx, |p, cx| p.redo(cx));
            }
            SideTool::Mask => {
                self.mask_tool.update(cx, |m, cx| m.redo(cx));
            }
            SideTool::Video => {
                self.score_video.update(cx, |v, cx| v.redo(cx));
            }
            SideTool::Project => self.redo_bg(cx),
        }
    }
}
