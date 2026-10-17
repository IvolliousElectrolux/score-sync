//! 蒙版工作区接管: 选中块按 T 进入 P 图.

use super::*;
use photo_edit::{
    apply_edge_adjust, sample_paper, FitHint, HostCmd, ImportOption, SourceFingerprint,
};

#[derive(Clone)]
pub(super) enum PhotoSession {
    Block {
        group_id: String,
        region_id: String,
        vector: bool,
    },
    /// `original` 是进入修图时的底色图, 还原用, 不写回工程.
    Background {
        original: std::sync::Arc<image::RgbImage>,
    },
}

/// 打开修图时要在后台做的那一步. 不含界面状态.
enum BlockPhotoJob {
    RasterExisting(PathBuf),
    RasterFresh {
        path: PathBuf,
        y0: u32,
        y1: u32,
        ink: i32,
        extra_top: i32,
        extra_bottom: i32,
        extra_left: i32,
        extra_right: i32,
        source: SourceFingerprint,
    },
    VectorExisting {
        dir: PathBuf,
        source: vector_edit::SourceFingerprint,
    },
    VectorFresh {
        req: vector_edit::ImportRequest,
        source: vector_edit::SourceFingerprint,
    },
}

enum BlockPhotoReady {
    Raster {
        paper: [u8; 3],
        source: SourceFingerprint,
        existing: Option<photo_edit::EditDocument>,
        fresh: Option<image::RgbaImage>,
        /// 大图的缩小预览, 后台做好. 界面先贴这一张, 不再一次性传完整页.
        preview: Option<image::RgbaImage>,
    },
    Vector {
        doc: vector_edit::VecDocument,
        source: vector_edit::SourceFingerprint,
        note: String,
    },
}

fn run_block_photo_job(job: BlockPhotoJob) -> Result<BlockPhotoReady, String> {
    match job {
        BlockPhotoJob::RasterExisting(dir) => {
            let doc = photo_edit::EditDocument::load_from_dir(&dir)?;
            let paper = doc.paper_rgb;
            let source = doc.source.clone().unwrap_or_default();
            let preview = doc
                .layers
                .first()
                .and_then(|layer| photo_edit::opening_preview(&layer.pixels));
            Ok(BlockPhotoReady::Raster {
                paper,
                source,
                existing: Some(doc),
                fresh: None,
                preview,
            })
        }
        BlockPhotoJob::RasterFresh {
            path,
            y0,
            y1,
            ink,
            extra_top,
            extra_bottom,
            extra_left,
            extra_right,
            mut source,
        } => {
            let full = crate::page_cache::load_rgb(&path)?;
            let y1 = y1.min(full.height().saturating_sub(1));
            if y1 < y0 {
                return Err("条带为空".into());
            }
            let band = crate::model::crop_band_fast(&full, y0, y1 - y0 + 1);
            let paper = sample_paper(&band, ink);
            let baked = apply_edge_adjust(
                &band,
                extra_top,
                extra_bottom,
                extra_left,
                extra_right,
                paper,
            );
            source.w = band.width();
            source.h = band.height();
            let rgba = photo_edit::document::rgb_to_rgba(&baked);
            let preview = photo_edit::opening_preview(&rgba);
            Ok(BlockPhotoReady::Raster {
                paper,
                source,
                existing: None,
                fresh: Some(rgba),
                preview,
            })
        }
        BlockPhotoJob::VectorExisting { dir, source } => {
            let mut doc = vector_edit::VecDocument::load_from_dir(&dir)?;
            doc.source = Some(source.clone());
            Ok(BlockPhotoReady::Vector {
                doc,
                source,
                note: String::new(),
            })
        }
        BlockPhotoJob::VectorFresh { req, mut source } => {
            let report = vector_edit::import_band(&req)?;
            let (w, h) = report.doc.rounded_size();
            source.w = w;
            source.h = h;
            let note = if report.truncated_forms > 0 {
                format!("有 {} 个过深的外观已跳过", report.truncated_forms)
            } else {
                String::new()
            };
            Ok(BlockPhotoReady::Vector {
                doc: report.doc,
                source,
                note,
            })
        }
    }
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
        let vector = self
            .doc
            .find_region(&rid)
            .and_then(|(pi, _)| self.doc.pages.get(pi))
            .map(|p| p.is_vector())
            .unwrap_or(false);
        let job = match self.prepare_block_photo_job(&gid, &rid, vector) {
            Ok(job) => job,
            Err(e) => {
                self.show_error("打开修图失败", e, cx);
                return;
            }
        };
        self.flush_mask_to_doc(cx);
        let mask_warn = self.block_has_intersecting_masks(&gid, &rid);
        let photo_hint = self.photo_fit_hint(&gid, &rid, 0);
        let photo_imports = if vector {
            Vec::new()
        } else {
            self.photo_import_options(&gid, &rid)
        };
        let vector_hint = if vector {
            Some(self.vector_fit_hint(&gid, &rid))
        } else {
            None
        };
        let vector_imports = if vector {
            self.vector_import_options(&gid, &rid)
        } else {
            Vec::new()
        };
        self.photo_edit.update(cx, |p, cx| p.close_session(cx));
        self.vector_edit.update(cx, |p, cx| p.close_session(cx));
        self.photo_open_gen = self.photo_open_gen.wrapping_add(1);
        let gen = self.photo_open_gen;
        self.photo_session = Some(PhotoSession::Block {
            group_id: gid,
            region_id: rid,
            vector,
        });
        let focus = if vector {
            self.vector_edit.read(cx).focus_handle_ref().clone()
        } else {
            self.photo_edit.read(cx).focus_handle_ref().clone()
        };
        focus.focus(window);
        self.status = "正在打开修图…".into();
        self.hint = self.status.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            // 矢量导入走 pdfium, 留在主线程; 先让修图界面画出来再读.
            // 位图解码不碰 pdfium, 放到后台, 界面不用等整页读完.
            let loaded = if matches!(job, BlockPhotoJob::VectorFresh { .. }) {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                run_block_photo_job(job)
            } else {
                cx.background_executor()
                    .spawn(async move { run_block_photo_job(job) })
                    .await
            };
            this.update(cx, |this, cx| {
                if this.photo_open_gen != gen {
                    return;
                }
                match loaded {
                    Ok(ready) => this.show_block_photo(
                        ready,
                        mask_warn,
                        photo_hint,
                        photo_imports,
                        vector_hint,
                        vector_imports,
                        cx,
                    ),
                    Err(e) => {
                        this.close_photo_session(cx);
                        this.show_error("打开修图失败", e, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn vector_session(&self) -> bool {
        matches!(
            self.photo_session,
            Some(PhotoSession::Block { vector: true, .. })
        )
    }

    /// 点左侧画布或工具条时, 接下来的 Ctrl+C/V 作用于选中内容.
    pub(super) fn focus_photo_editor(&self, window: &mut Window, cx: &mut App) {
        if self.photo_session.is_none() {
            return;
        }
        let handle = if self.vector_session() {
            self.vector_edit.update(cx, |p, _| {
                p.note_canvas_target();
                p.focus_handle_ref().clone()
            })
        } else {
            self.photo_edit.update(cx, |p, _| {
                p.note_canvas_target();
                p.focus_handle_ref().clone()
            })
        };
        handle.focus(window);
    }

    fn show_block_photo(
        &mut self,
        ready: BlockPhotoReady,
        mask_warn: bool,
        photo_hint: FitHint,
        photo_imports: Vec<ImportOption>,
        vector_hint: Option<vector_edit::FitHint>,
        vector_imports: Vec<vector_edit::ImportOption>,
        cx: &mut Context<Self>,
    ) {
        match ready {
            BlockPhotoReady::Raster {
                paper,
                source,
                existing,
                fresh,
                preview,
            } => {
                self.photo_edit.update(cx, |p, cx| {
                    if let Some(doc) = existing {
                        p.open_streaming(doc, preview, "已载入上次修图", cx);
                    } else if let Some(img) = fresh {
                        let (w, h) = img.dimensions();
                        let doc = photo_edit::EditDocument::from_rgba(img, paper);
                        p.open_streaming(doc, preview, format!("画布 {w}×{h}"), cx);
                    }
                    p.set_source(source);
                    p.set_fit_hint(photo_hint, cx);
                    p.set_import_options(photo_imports, cx);
                });
                self.status = if mask_warn {
                    "P 图 (这块上的遮盖可能错位, 提交后只平移块下方蒙版)".into()
                } else {
                    "P 图".into()
                };
            }
            BlockPhotoReady::Vector { doc, source, note } => {
                let hint = vector_hint.unwrap_or_default();
                self.vector_edit.update(cx, |p, cx| {
                    p.open_document(doc, cx);
                    p.set_source(source);
                    p.set_fit_hint(hint, cx);
                    p.set_import_options(vector_imports, cx);
                });
                self.status = if mask_warn {
                    "P 图 · 矢量 (这块上的遮盖可能错位, 提交后只平移块下方蒙版)".into()
                } else if note.is_empty() {
                    "P 图 · 矢量".into()
                } else {
                    format!("P 图 · 矢量 ({note})").into()
                };
            }
        }
        self.hint = self.status.clone();
        cx.notify();
    }

    fn prepare_block_photo_job(
        &self,
        gid: &str,
        rid: &str,
        vector: bool,
    ) -> Result<BlockPhotoJob, String> {
        if vector {
            let (pi, r) = self.doc.find_region(rid).ok_or("找不到该分块")?;
            let page = self.doc.pages.get(pi).ok_or("找不到页")?;
            let src = page.vector.as_ref().ok_or("这一页不是矢量页")?;
            let adj = mask_tool::layout::BlockAdjust::find(self.doc.get_block_layout(gid), rid)
                .cloned()
                .unwrap_or_default();
            let source = vector_edit::SourceFingerprint {
                page_id: r.page_id.clone(),
                y0: r.y0,
                y1: r.y1,
                w: src.w_pt.round().max(1.0) as u32,
                h: (r.y1 - r.y0 + 1).max(1) as u32,
                extra_top: adj.extra_top,
                extra_bottom: adj.extra_bottom,
                extra_left: adj.extra_left,
                extra_right: adj.extra_right,
            };
            if self.doc.region_edit_kind(rid) == crate::model::EditKind::Vector
                && self.doc.region_edit_applies(rid)
            {
                return Ok(BlockPhotoJob::VectorExisting {
                    dir: DocState::region_edit_dir(rid),
                    source,
                });
            }
            return Ok(BlockPhotoJob::VectorFresh {
                req: vector_edit::ImportRequest {
                    pdf_path: src.pdf_path.clone(),
                    page_index: src.page_index,
                    y0: r.y0 as f32,
                    y1: (r.y1 + 1) as f32,
                    extra_top: adj.extra_top,
                    extra_bottom: adj.extra_bottom,
                    extra_left: adj.extra_left,
                    extra_right: adj.extra_right,
                    page_id: r.page_id.clone(),
                },
                source,
            });
        }
        if self.doc.region_edit_kind(rid) == crate::model::EditKind::Raster
            && self.doc.has_region_edit(rid)
        {
            return Ok(BlockPhotoJob::RasterExisting(DocState::region_edit_dir(
                rid,
            )));
        }
        let (pi, r) = self.doc.find_region(rid).ok_or("找不到该分块")?;
        let page = self.doc.pages.get(pi).ok_or("找不到页")?;
        if !page.disk_path.is_file() {
            return Err("页图不在磁盘上".into());
        }
        let adj = mask_tool::layout::BlockAdjust::find(self.doc.get_block_layout(gid), rid)
            .cloned()
            .unwrap_or_default();
        Ok(BlockPhotoJob::RasterFresh {
            path: page.disk_path.clone(),
            y0: r.y0.max(0) as u32,
            y1: r.y1 as u32,
            ink: self.doc.ink_threshold,
            extra_top: adj.extra_top,
            extra_bottom: adj.extra_bottom,
            extra_left: adj.extra_left,
            extra_right: adj.extra_right,
            source: SourceFingerprint {
                page_id: r.page_id.clone(),
                y0: r.y0,
                y1: r.y1,
                w: 0,
                h: 0,
                extra_top: adj.extra_top,
                extra_bottom: adj.extra_bottom,
                extra_left: adj.extra_left,
                extra_right: adj.extra_right,
            },
        })
    }

    fn prepare_vector_document(
        &self,
        gid: &str,
        rid: &str,
    ) -> Result<
        (
            vector_edit::VecDocument,
            vector_edit::SourceFingerprint,
            String,
        ),
        String,
    > {
        let (pi, r) = self.doc.find_region(rid).ok_or("找不到该分块")?;
        let page = self.doc.pages.get(pi).ok_or("找不到页")?;
        let src = page.vector.as_ref().ok_or("这一页不是矢量页")?;
        let adj = mask_tool::layout::BlockAdjust::find(self.doc.get_block_layout(gid), rid)
            .cloned()
            .unwrap_or_default();
        let source = vector_edit::SourceFingerprint {
            page_id: r.page_id.clone(),
            y0: r.y0,
            y1: r.y1,
            w: src.w_pt.round().max(1.0) as u32,
            h: (r.y1 - r.y0 + 1).max(1) as u32,
            extra_top: adj.extra_top,
            extra_bottom: adj.extra_bottom,
            extra_left: adj.extra_left,
            extra_right: adj.extra_right,
        };
        if let Some(mut doc) = self.doc.load_vector_document(rid) {
            doc.source = Some(source.clone());
            return Ok((doc, source, String::new()));
        }
        let report = vector_edit::import_band(&vector_edit::ImportRequest {
            pdf_path: src.pdf_path.clone(),
            page_index: src.page_index,
            y0: r.y0 as f32,
            y1: (r.y1 + 1) as f32,
            extra_top: adj.extra_top,
            extra_bottom: adj.extra_bottom,
            extra_left: adj.extra_left,
            extra_right: adj.extra_right,
            page_id: r.page_id.clone(),
        })?;
        let (w, h) = report.doc.rounded_size();
        let source = vector_edit::SourceFingerprint { w, h, ..source };
        let note = if report.truncated_forms > 0 {
            format!("有 {} 个过深的外观已跳过", report.truncated_forms)
        } else {
            String::new()
        };
        Ok((report.doc, source, note))
    }

    fn vector_fit_hint(&self, gid: &str, rid: &str) -> vector_edit::FitHint {
        let photo = self.photo_fit_hint(gid, rid, 0);
        vector_edit::FitHint {
            other_h: photo.other_h,
            sheet_w: photo.sheet_w,
            aspect_w: photo.aspect_w,
            aspect_h: photo.aspect_h,
            bg_enabled: photo.bg_enabled,
            snap_w: photo.snap_w,
            snap_h: photo.snap_h,
        }
    }

    fn vector_import_options(&self, gid: &str, current: &str) -> Vec<vector_edit::ImportOption> {
        let Some(g) = self.doc.groups.iter().find(|g| g.id == gid) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter(|id| id.as_str() != current)
            .filter_map(|rid| {
                let (pi, r) = self.doc.find_region(rid)?;
                let page = self.doc.pages.get(pi)?;
                if !page.is_vector() {
                    return None;
                }
                Some(vector_edit::ImportOption {
                    id: rid.clone(),
                    label: format!("P{} {}", pi + 1, r.kind),
                })
            })
            .collect()
    }

    pub(super) fn try_open_bg_photo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.photo_open() {
            return;
        }
        let Some(img) = self.image_for_bg_edit() else {
            self.status = "先选择一张底色图再修图.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        };
        let (w, h) = img.dimensions();
        let tw = self.doc.bg_aspect_w.max(1);
        let th = self.doc.bg_aspect_h.max(1);
        let original = std::sync::Arc::new(img);
        self.photo_edit.update(cx, |p, cx| {
            p.open_rgb((*original).clone(), None, cx);
            p.set_fit_hint(
                FitHint {
                    snap_w: if w > tw { tw } else { 0 },
                    snap_h: if h > th { th } else { 0 },
                    ..FitHint::default()
                },
                cx,
            );
            p.set_import_options(Vec::new(), cx);
        });
        self.photo_session = Some(PhotoSession::Background { original });
        self.photo_edit
            .read(cx)
            .focus_handle_ref()
            .clone()
            .focus(window);
        self.status = format!("底色修图 {w}×{h}").into();
        self.hint = self.status.clone();
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
            snap_w: 0,
            snap_h: 0,
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
        self.photo_open_gen = self.photo_open_gen.wrapping_add(1);
        self.photo_session = None;
        self.photo_edit.update(cx, |p, cx| p.close_session(cx));
        self.vector_edit.update(cx, |p, cx| p.close_session(cx));
        if self.side_tool == SideTool::Mask {
            self.sync_mask_image(cx);
        }
        cx.notify();
    }

    pub(super) fn apply_photo_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sess) = self.photo_session.clone() else {
            return;
        };
        if self.vector_session() {
            self.apply_vector_edit(window, cx);
            return;
        }
        let Some(commit) = self.photo_edit.read(cx).take_commit() else {
            self.close_photo_session(cx);
            return;
        };
        let region_id = match sess {
            PhotoSession::Background { .. } => {
                self.commit_edited_bg(commit.flat, cx);
                self.close_photo_session(cx);
                cx.notify();
                return;
            }
            PhotoSession::Block { region_id, .. } => region_id,
        };
        let using = crate::model::groups_using_region(&self.doc.groups, &region_id);
        let old_spans: Vec<(String, i64, i64)> = using
            .iter()
            .filter_map(|gid| {
                let spans = self.doc.group_member_spans(gid);
                spans
                    .iter()
                    .find(|(id, ..)| id == &region_id)
                    .map(|(_, y0, y1)| (gid.clone(), *y0, *y1 - *y0 + 1))
            })
            .collect();
        if let Err(e) = self.doc.write_region_edit(&region_id, &commit.document) {
            self.show_error("保存修图失败", e, cx);
            return;
        }
        for gid in &using {
            self.clear_block_edge_adjust(gid, &region_id);
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
        let (group_id, region_id) = match sess {
            PhotoSession::Background { original } => {
                self.photo_edit.update(cx, |p, cx| {
                    p.open_rgb((*original).clone(), None, cx);
                });
                self.status = "已还原到进入修图时的底色.".into();
                self.hint = self.status.clone();
                cx.notify();
                return;
            }
            PhotoSession::Block {
                group_id,
                region_id,
                vector,
            } => {
                if vector {
                    self.restore_vector_edit(&group_id, &region_id, cx);
                    return;
                }
                (group_id, region_id)
            }
        };
        let using = crate::model::groups_using_region(&self.doc.groups, &region_id);
        let old_spans: Vec<(String, i64, i64)> = using
            .iter()
            .filter_map(|gid| {
                let spans = self.doc.group_member_spans(gid);
                spans
                    .iter()
                    .find(|(id, ..)| id == &region_id)
                    .map(|(_, y0, y1)| (gid.clone(), *y0, *y1 - *y0 + 1))
            })
            .collect();
        let orig_h = self
            .doc
            .find_region(&region_id)
            .map(|(_, r)| (r.y1 - r.y0 + 1).max(0) as i64)
            .unwrap_or(0);
        self.doc.remove_region_edit(&region_id);
        for (gid, y0, old_h) in old_spans {
            if let Some(masks) = self.doc.group_masks.get_mut(&gid) {
                crate::model::remap_masks_after_height_change(masks, y0, old_h, orig_h);
            }
        }
        for gid in using {
            self.mark_video_pool_dirty_group(&gid);
        }
        self.dirty = true;
        match self.load_block_for_photo(&group_id, &region_id) {
            Ok((img, paper, source, existing)) => {
                let hint = self.photo_fit_hint(&group_id, &region_id, img.width());
                let imports = self.photo_import_options(&group_id, &region_id);
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
        if self.vector_session() {
            self.handle_vector_host_cmd(window, cx);
            return;
        }
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
        let gid = match self.photo_session.as_ref() {
            Some(PhotoSession::Block { group_id, .. }) => Some(group_id.clone()),
            _ => None,
        }
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
        let dirty = if self.vector_session() {
            self.vector_edit.read(cx).is_dirty()
        } else {
            self.photo_edit.read(cx).is_dirty()
        };
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

    fn apply_vector_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PhotoSession::Block {
            region_id,
            vector: true,
            ..
        }) = self.photo_session.clone()
        else {
            return;
        };
        let Some(commit) = self.vector_edit.read(cx).take_commit() else {
            self.close_photo_session(cx);
            return;
        };
        let using = crate::model::groups_using_region(&self.doc.groups, &region_id);
        let old_spans: Vec<(String, i64, i64)> = using
            .iter()
            .filter_map(|gid| {
                let spans = self.doc.group_member_spans(gid);
                spans
                    .iter()
                    .find(|(id, ..)| id == &region_id)
                    .map(|(_, y0, y1)| (gid.clone(), *y0, *y1 - *y0 + 1))
            })
            .collect();
        if let Err(e) = self
            .doc
            .write_vector_region_edit(&region_id, &commit.document)
        {
            self.show_error("保存修图失败", e, cx);
            return;
        }
        for gid in &using {
            self.clear_block_edge_adjust(gid, &region_id);
        }
        let new_h = commit.document.rounded_size().1 as i64;
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

    fn restore_vector_edit(&mut self, gid: &str, rid: &str, cx: &mut Context<Self>) {
        let using = crate::model::groups_using_region(&self.doc.groups, rid);
        let old_spans: Vec<(String, i64, i64)> = using
            .iter()
            .filter_map(|g| {
                let spans = self.doc.group_member_spans(g);
                spans
                    .iter()
                    .find(|(id, ..)| id == rid)
                    .map(|(_, y0, y1)| (g.clone(), *y0, *y1 - *y0 + 1))
            })
            .collect();
        let orig_h = self
            .doc
            .find_region(rid)
            .map(|(_, r)| (r.y1 - r.y0 + 1).max(0) as i64)
            .unwrap_or(0);
        self.doc.remove_region_edit(rid);
        for (g, y0, old_h) in old_spans {
            if let Some(masks) = self.doc.group_masks.get_mut(&g) {
                crate::model::remap_masks_after_height_change(masks, y0, old_h, orig_h);
            }
        }
        for g in using {
            self.mark_video_pool_dirty_group(&g);
        }
        self.dirty = true;
        match self.prepare_vector_document(gid, rid) {
            Ok((doc, source, _)) => {
                let hint = self.vector_fit_hint(gid, rid);
                let imports = self.vector_import_options(gid, rid);
                self.vector_edit.update(cx, |p, cx| {
                    p.open_document(doc, cx);
                    p.set_source(source);
                    p.set_fit_hint(hint, cx);
                    p.set_import_options(imports, cx);
                });
                self.status = "已从 PDF 重新导入这条".into();
            }
            Err(e) => self.show_error("还原失败", e, cx),
        }
        self.hint = self.status.clone();
        cx.notify();
    }

    fn handle_vector_host_cmd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = self.vector_edit.update(cx, |p, _| p.take_host_cmd());
        match cmd {
            Some(vector_edit::HostCmd::Apply) => self.apply_vector_edit(window, cx),
            Some(vector_edit::HostCmd::Cancel) => self.request_leave_photo(None, window, cx),
            Some(vector_edit::HostCmd::Restore) => self.restore_photo_edit(window, cx),
            Some(vector_edit::HostCmd::RequestImport(rid)) => {
                self.import_block_into_vector(&rid, cx)
            }
            None => {}
        }
    }

    fn import_block_into_vector(&mut self, rid: &str, cx: &mut Context<Self>) {
        let gid = match self.photo_session.as_ref() {
            Some(PhotoSession::Block { group_id, .. }) => Some(group_id.clone()),
            _ => None,
        };
        let Some(gid) = gid else {
            return;
        };
        match self.prepare_vector_document(&gid, rid) {
            Ok((doc, _, _)) => {
                let name = format!("导入 {rid}");
                self.vector_edit
                    .update(cx, |p, cx| p.import_document_as_layer(doc, &name, cx));
            }
            Err(e) => self.show_error("导入分块失败", e, cx),
        }
    }
}
