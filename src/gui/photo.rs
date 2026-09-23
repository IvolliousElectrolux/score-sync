//! 蒙版工作区接管: 选中块按 T 进入 P 图.

use super::*;
use photo_edit::{
    apply_edge_adjust, sample_paper, FitHint, HostCmd, ImportOption, SourceFingerprint,
};

#[derive(Clone)]
pub(super) struct PhotoSession {
    pub group_id: String,
    pub region_id: String,
}

impl ScoreSyncApp {
    pub(super) fn photo_open(&self) -> bool {
        self.photo_session.is_some()
    }

    pub(super) fn try_open_photo_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.side_tool != SideTool::Mask {
            return;
        }
        if self.photo_open() {
            return;
        }
        let Some(gid) = self
            .mask_target
            .clone()
            .or_else(|| self.doc.active_group_id.clone())
        else {
            self.status = "没有当前组合".into();
            cx.notify();
            return;
        };
        let Some(rid) = self.mask_active_block_id.clone().or_else(|| {
            self.mask_tool
                .read(cx)
                .selected_block_id()
                .map(|s| s.to_string())
        }) else {
            self.status = "先在蒙版里选中一个分块再按 T".into();
            cx.notify();
            return;
        };
        if !self.mask_tool.read(cx).selected_masks_empty() {
            self.status = "请先取消蒙版选中再修图".into();
            cx.notify();
            return;
        }
        match self.load_block_for_photo(&gid, &rid) {
            Ok((img, paper, source, existing)) => {
                self.flush_mask_to_doc(cx);
                let hint = self.photo_fit_hint(&gid, &rid, img.width());
                let imports = self.photo_import_options(&gid, &rid);
                self.photo_edit.update(cx, |p, cx| {
                    if let Some(doc) = existing {
                        p.open_document(doc, cx);
                    } else {
                        p.open_rgb(img, Some(paper), cx);
                    }
                    p.set_source(source);
                    p.set_fit_hint(hint, cx);
                    p.set_import_options(imports, cx);
                });
                let mask_warn = self.block_has_intersecting_masks(&gid, &rid);
                self.photo_session = Some(PhotoSession {
                    group_id: gid,
                    region_id: rid,
                });
                self.photo_edit
                    .read(cx)
                    .focus_handle_ref()
                    .clone()
                    .focus(window);
                self.status = if mask_warn {
                    "P 图 (这块上的遮盖可能错位, 提交后只平移块下方蒙版)".into()
                } else {
                    "P 图".into()
                };
                self.hint = self.status.clone();
            }
            Err(e) => self.show_error("打开修图失败", e, cx),
        }
        cx.notify();
    }

    fn load_block_for_photo(
        &self,
        gid: &str,
        rid: &str,
    ) -> Result<
        (
            image::RgbImage,
            [u8; 3],
            SourceFingerprint,
            Option<photo_edit::EditDocument>,
        ),
        String,
    > {
        if let Some(doc) = self.doc.load_region_document(rid) {
            let flat = doc.flatten_rgb();
            let paper = doc.paper_rgb;
            let src = doc.source.clone().unwrap_or_default();
            return Ok((flat, paper, src, Some(doc)));
        }
        let (pi, r) = self.doc.find_region(rid).ok_or("找不到该分块")?;
        let page = self.doc.pages.get(pi).ok_or("找不到页")?;
        if !page.disk_path.is_file() {
            return Err("页图不在磁盘上".into());
        }
        let full = crate::page_cache::load_rgb(&page.disk_path)?;
        let y0 = r.y0.max(0) as u32;
        let y1 = (r.y1 as u32).min(full.height().saturating_sub(1));
        if y1 < y0 {
            return Err("条带为空".into());
        }
        let band = crate::model::crop_band_fast(&full, y0, y1 - y0 + 1);
        let adj = mask_tool::layout::BlockAdjust::find(self.doc.get_block_layout(gid), rid)
            .cloned()
            .unwrap_or_default();
        let paper = sample_paper(&band, self.doc.ink_threshold);
        let baked = apply_edge_adjust(
            &band,
            adj.extra_top,
            adj.extra_bottom,
            adj.extra_left,
            adj.extra_right,
            paper,
        );
        let source = SourceFingerprint {
            page_id: r.page_id.clone(),
            y0: r.y0,
            y1: r.y1,
            w: band.width(),
            h: band.height(),
            extra_top: adj.extra_top,
            extra_bottom: adj.extra_bottom,
            extra_left: adj.extra_left,
            extra_right: adj.extra_right,
        };
        Ok((baked, paper, source, None))
    }

    fn photo_fit_hint(&self, gid: &str, rid: &str, _block_w: u32) -> FitHint {
        let heights = self.doc.group_member_heights(gid);
        let other: u32 = heights
            .iter()
            .filter(|(id, _)| id != rid)
            .map(|(_, h)| *h)
            .sum();
        let layout = self.doc.get_block_layout(gid);
        let extra_gaps: u32 = layout
            .iter()
            .map(|a| a.gap_before.max(0) as u32 + a.gap_after.max(0) as u32)
            .sum();
        FitHint {
            other_h: other.saturating_add(extra_gaps),
            sheet_w: self.doc.group_sheet_width(gid),
            aspect_w: self.doc.bg_aspect_w.max(1),
            aspect_h: self.doc.bg_aspect_h.max(1),
            bg_enabled: self.doc.bg_enabled,
        }
    }

    fn photo_import_options(&self, gid: &str, current: &str) -> Vec<ImportOption> {
        let Some(g) = self.doc.groups.iter().find(|g| g.id == gid) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter(|id| id.as_str() != current)
            .filter_map(|rid| {
                let (pi, r) = self.doc.find_region(rid)?;
                Some(ImportOption {
                    id: rid.clone(),
                    label: format!("P{} {}", pi + 1, r.kind),
                })
            })
            .collect()
    }

    fn block_has_intersecting_masks(&self, gid: &str, rid: &str) -> bool {
        let Some((_, y0, y1)) = self
            .doc
            .group_member_spans(gid)
            .into_iter()
            .find(|(id, ..)| id == rid)
        else {
            return false;
        };
        self.doc
            .group_masks
            .get(gid)
            .map(|masks| {
                masks.iter().any(|m| {
                    let my0 = m.y0 as i64;
                    let my1 = m.y1 as i64;
                    my1 >= y0 && my0 <= y1
                })
            })
            .unwrap_or(false)
    }

    pub(super) fn close_photo_session(&mut self, cx: &mut Context<Self>) {
        self.photo_session = None;
        self.photo_edit.update(cx, |p, cx| p.close_session(cx));
        if self.side_tool == SideTool::Mask {
            self.sync_mask_image(cx);
        }
        cx.notify();
    }

    pub(super) fn apply_photo_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sess) = self.photo_session.clone() else {
            return;
        };
        let Some(commit) = self.photo_edit.read(cx).take_commit() else {
            self.close_photo_session(cx);
            return;
        };
        let using = crate::model::groups_using_region(&self.doc.groups, &sess.region_id);
        let old_spans: Vec<(String, i64, i64)> = using
            .iter()
            .filter_map(|gid| {
                let spans = self.doc.group_member_spans(gid);
                spans
                    .iter()
                    .find(|(id, ..)| id == &sess.region_id)
                    .map(|(_, y0, y1)| (gid.clone(), *y0, *y1 - *y0 + 1))
            })
            .collect();
        if let Err(e) = self
            .doc
            .write_region_edit(&sess.region_id, &commit.document)
        {
            self.show_error("保存修图失败", e, cx);
            return;
        }
        for gid in &using {
            self.clear_block_edge_adjust(gid, &sess.region_id);
        }
        let new_h = commit.flat.height() as i64;
        for (gid, y0, old_h) in old_spans {
            if let Some(masks) = self.doc.group_masks.get_mut(&gid) {
                crate::model::remap_masks_after_height_change(masks, y0, old_h, new_h);
            }
        }
        for gid in using {
            self.mark_video_pool_dirty_group(&gid);
        }
        self.dirty = true;
        self.close_photo_session(cx);
        self.mask_tool
            .read(cx)
            .focus_handle_ref()
            .clone()
            .focus(window);
        self.status = "已应用修图".into();
        cx.notify();
    }

    pub(super) fn restore_photo_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sess) = self.photo_session.clone() else {
            return;
        };
        let using = crate::model::groups_using_region(&self.doc.groups, &sess.region_id);
        let old_spans: Vec<(String, i64, i64)> = using
            .iter()
            .filter_map(|gid| {
                let spans = self.doc.group_member_spans(gid);
                spans
                    .iter()
                    .find(|(id, ..)| id == &sess.region_id)
                    .map(|(_, y0, y1)| (gid.clone(), *y0, *y1 - *y0 + 1))
            })
            .collect();
        let orig_h = self
            .doc
            .find_region(&sess.region_id)
            .map(|(_, r)| (r.y1 - r.y0 + 1).max(0) as i64)
            .unwrap_or(0);
        self.doc.remove_region_edit(&sess.region_id);
        for (gid, y0, old_h) in old_spans {
            if let Some(masks) = self.doc.group_masks.get_mut(&gid) {
                crate::model::remap_masks_after_height_change(masks, y0, old_h, orig_h);
            }
        }
        for gid in using {
            self.mark_video_pool_dirty_group(&gid);
        }
        self.dirty = true;
        match self.load_block_for_photo(&sess.group_id, &sess.region_id) {
            Ok((img, paper, source, existing)) => {
                let hint = self.photo_fit_hint(&sess.group_id, &sess.region_id, img.width());
                let imports = self.photo_import_options(&sess.group_id, &sess.region_id);
                self.photo_edit.update(cx, |p, cx| {
                    if let Some(doc) = existing {
                        p.open_document(doc, cx);
                    } else {
                        p.open_rgb(img, Some(paper), cx);
                    }
                    p.set_source(source);
                    p.set_fit_hint(hint, cx);
                    p.set_import_options(imports, cx);
                });
                self.photo_edit
                    .read(cx)
                    .focus_handle_ref()
                    .clone()
                    .focus(window);
                self.status = "已还原到这块的初始状态".into();
                self.hint = self.status.clone();
            }
            Err(e) => {
                self.show_error("还原失败", e, cx);
                self.close_photo_session(cx);
                self.mask_tool
                    .read(cx)
                    .focus_handle_ref()
                    .clone()
                    .focus(window);
            }
        }
        cx.notify();
    }

    fn clear_block_edge_adjust(&mut self, gid: &str, rid: &str) {
        if let Some(layout) = self.doc.group_block_layout.get_mut(gid) {
            if let Some(adj) = layout.iter_mut().find(|a| a.region_id == rid) {
                adj.extra_top = 0;
                adj.extra_bottom = 0;
                adj.extra_left = 0;
                adj.extra_right = 0;
            }
        }
    }

    pub(super) fn handle_photo_host_cmd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = self.photo_edit.update(cx, |p, _| p.take_host_cmd());
        match cmd {
            Some(HostCmd::Apply) => self.apply_photo_edit(window, cx),
            Some(HostCmd::Cancel) => self.request_leave_photo(None, window, cx),
            Some(HostCmd::Restore) => self.restore_photo_edit(window, cx),
            Some(HostCmd::RequestImport(rid)) => self.import_block_into_photo(&rid, cx),
            None => {}
        }
    }

    fn import_block_into_photo(&mut self, rid: &str, cx: &mut Context<Self>) {
        let gid = self
            .photo_session
            .as_ref()
            .map(|s| s.group_id.clone())
            .or_else(|| self.doc.active_group_id.clone());
        let Some(gid) = gid else {
            return;
        };
        match self.load_block_for_photo(&gid, rid) {
            Ok((img, _, _, _)) => {
                let rgba = photo_edit::document::rgb_to_rgba(&img);
                self.photo_edit
                    .update(cx, |p, cx| p.import_rgba(rgba, format!("导入 {rid}"), cx));
            }
            Err(e) => self.show_error("导入分块失败", e, cx),
        }
    }

    pub(super) fn request_leave_photo(
        &mut self,
        next: Option<SideTool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.photo_open() {
            if let Some(t) = next {
                self.set_side_tool(t, window, cx);
            }
            return;
        }
        let dirty = self.photo_edit.read(cx).is_dirty();
        if dirty {
            self.dialog = Some(DialogKind::UnsavedPhoto { next });
            cx.notify();
            return;
        }
        self.close_photo_session(cx);
        if let Some(t) = next {
            self.set_side_tool(t, window, cx);
        }
    }

    pub(super) fn discard_photo_and_leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = match self.dialog.take() {
            Some(DialogKind::UnsavedPhoto { next }) => next,
            _ => None,
        };
        self.close_photo_session(cx);
        if let Some(t) = next {
            self.set_side_tool(t, window, cx);
        }
    }

    pub(super) fn apply_photo_and_leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = match self.dialog.take() {
            Some(DialogKind::UnsavedPhoto { next }) => next,
            _ => None,
        };
        self.apply_photo_edit(window, cx);
        if let Some(t) = next {
            self.set_side_tool(t, window, cx);
        }
    }
}
