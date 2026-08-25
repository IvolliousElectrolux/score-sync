//! 页图窗口、蒙版/视频同步、面板切换.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn refresh_render(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.doc.current_page() else {
            self.render_image = None;
            self.img_w = 0;
            self.img_h = 0;
            cx.notify();
            return;
        };
        self.img_w = page.width();
        self.img_h = page.height();
        let Some(rgb) = page.image.as_ref() else {
            // 占位: 尺寸已知但像素未到, 触发异步窗口加载
            self.render_image = None;
            self.request_page_window(cx);
            cx.notify();
            return;
        };
        let (w, h) = (self.img_w, self.img_h);
        let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
            let p = rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
        // GPUI / Windows 纹理多为 BGRA
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let frame = Frame::new(rgba);
        self.render_image = Some(Arc::new(RenderImage::new(smallvec![frame])));
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.sync_mask_image(cx);
        cx.notify();
    }

    /// 异步加载当前页 ±4 窗口并释放窗外页图.
    pub(super) fn request_page_window(&mut self, cx: &mut Context<Self>) {
        self.page_load_gen = self.page_load_gen.wrapping_add(1);
        let gen = self.page_load_gen;
        let center = self.doc.current_page_index;
        let radius = crate::page_cache::WINDOW_RADIUS;
        let n = self.doc.pages.len();
        if n == 0 {
            return;
        }
        let lo = center.saturating_sub(radius);
        let hi = (center + radius).min(n - 1);
        let mut jobs: Vec<(usize, PathBuf, bool)> = Vec::new();
        for i in lo..=hi {
            if self.doc.pages[i].regions.is_empty() {
                if self.doc.load_detect_sidecar(i) {
                    self.doc.ensure_page_groups(i);
                }
            }
            if self.doc.pages[i].image.is_none() {
                jobs.push((
                    i,
                    self.doc.pages[i].disk_path.clone(),
                    self.doc.pages[i].regions.is_empty(),
                ));
            }
        }
        // 窗外立刻卸掉
        for i in 0..n {
            if i < lo || i > hi {
                self.doc.unload_page_image(i);
            }
        }
        if jobs.is_empty() {
            // 当前页已在内存则刷新贴图
            if self.doc.pages.get(center).and_then(|p| p.image.as_ref()).is_some()
            {
                if self.pending_redetect {
                    self.flush_pending_redetect(cx);
                    return;
                }
                if self.render_image.is_none() {
                    self.refresh_render(cx);
                }
            }
            return;
        }
        let ink = self.doc.ink_threshold;
        let margin = self.doc.margin;
        let (tx, rx) = async_channel::unbounded::<(
            usize,
            Result<image::RgbImage, String>,
            Option<crate::detect_cache::PageDetectFile>,
        )>();
        std::thread::spawn(move || {
            for (idx, path, need_detect) in jobs {
                let r = crate::page_cache::load_rgb(&path);
                let detect = if need_detect {
                    match &r {
                        Ok(img) => Some(crate::detect_cache::load_or_detect(
                            img, &path, ink, margin,
                        )),
                        Err(_) => crate::detect_cache::load(&path),
                    }
                } else {
                    None
                };
                let _ = tx.send_blocking((idx, r, detect));
            }
        });
        cx.spawn(async move |this, cx| {
            while let Ok((idx, result, detect)) = rx.recv().await {
                this.update(cx, |view, cx| {
                    if view.page_load_gen != gen {
                        return;
                    }
                    if let Some(file) = detect {
                        if view
                            .doc
                            .pages
                            .get(idx)
                            .map(|p| p.regions.is_empty())
                            .unwrap_or(false)
                        {
                            view.doc.apply_detect_file(idx, &file);
                            view.doc.ensure_page_groups(idx);
                        }
                    }
                    if let Ok(img) = result {
                        if let Some(page) = view.doc.pages.get_mut(idx) {
                            page.img_w = img.width();
                            page.img_h = img.height();
                            page.image = Some(img);
                        }
                        view.doc.seed_region_anchors_for_page(idx);
                    }
                    if idx == view.doc.current_page_index {
                        if view.pending_redetect
                            && view
                                .doc
                                .pages
                                .get(idx)
                                .and_then(|p| p.image.as_ref())
                                .is_some()
                        {
                            view.flush_pending_redetect(cx);
                        } else {
                            view.refresh_render(cx);
                        }
                    } else {
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// 后台把各页 sidecar 灌进 regions, 再补齐全部输出组合.
    /// 页图像素仍只留当前 ±4; 列表/蒙版页签用的是组合数据, 不解码整本图.
    /// `detect_missing`: PDF 导入后若某页没有 sidecar, 再后台识别该页.
    pub(super) fn start_hydrate_all(&mut self, detect_missing: bool, cx: &mut Context<Self>) {
        self.hydrate_gen = self.hydrate_gen.wrapping_add(1);
        let gen = self.hydrate_gen;
        let jobs: Vec<(usize, PathBuf)> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.regions.is_empty())
            .map(|(i, p)| (i, p.disk_path.clone()))
            .collect();
        if jobs.is_empty() {
            self.doc.ensure_all_page_groups();
            self.start_seed_align_anchors(cx);
            self.request_page_window(cx);
            cx.notify();
            return;
        }
        let n_jobs = jobs.len();
        self.status = format!("正在载入全部分块结果 ({n_jobs} 页)…").into();
        self.hint = self.status.clone();
        cx.notify();
        let (tx, rx) =
            async_channel::unbounded::<(usize, Option<crate::detect_cache::PageDetectFile>)>();
        std::thread::spawn(move || {
            for (idx, path) in jobs {
                let file = crate::detect_cache::load(&path);
                let _ = tx.send_blocking((idx, file));
            }
        });
        cx.spawn(async move |this, cx| {
            let mut done = 0usize;
            while let Ok((idx, file)) = rx.recv().await {
                done += 1;
                let d = done;
                this.update(cx, |view, cx| {
                    if view.hydrate_gen != gen {
                        return;
                    }
                    if let Some(file) = file {
                        if view
                            .doc
                            .pages
                            .get(idx)
                            .map(|p| p.regions.is_empty())
                            .unwrap_or(false)
                        {
                            view.doc.apply_detect_file(idx, &file);
                            view.doc.ensure_page_groups(idx);
                        }
                    }
                    if d == n_jobs || d % 16 == 0 {
                        view.status = format!("正在载入全部分块结果 {d}/{n_jobs}…").into();
                        view.hint = view.status.clone();
                        cx.notify();
                    }
                })
                .ok();
                if d % 16 == 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(4))
                        .await;
                }
            }
            this.update(cx, |view, cx| {
                if view.hydrate_gen != gen {
                    return;
                }
                view.doc.ensure_all_page_groups();
                let n = view.doc.pages.len();
                let g = view.doc.groups.len();
                view.status = format!("已载入 {n} 页, {g} 个输出组合.").into();
                view.hint = view.status.clone();
                view.request_page_window(cx);
                if detect_missing {
                    view.start_detect_missing_pages(cx);
                } else {
                    view.start_seed_align_anchors(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 仍无 regions 的页在后台识别 (只补组合, 不重置已有合并/调序).
    pub(super) fn start_detect_missing_pages(&mut self, cx: &mut Context<Self>) {
        let jobs: Vec<(usize, PathBuf)> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.regions.is_empty())
            .map(|(i, p)| (i, p.disk_path.clone()))
            .collect();
        if jobs.is_empty() {
            return;
        }
        let n = jobs.len();
        let ink = self.doc.ink_threshold;
        let margin = self.doc.margin;
        let gen = self.hydrate_gen;
        self.status = format!("部分页无缓存, 正在后台识别 ({n} 页)…").into();
        self.hint = self.status.clone();
        cx.notify();
        let (tx, rx) = async_channel::unbounded::<(usize, crate::detect_cache::PageDetectFile)>();
        std::thread::spawn(move || {
            for (idx, path) in jobs {
                match crate::page_cache::load_rgb(&path) {
                    Ok(img) => {
                        let file = crate::detect_cache::detect_and_save(
                            &img, &path, ink, margin,
                        );
                        let _ = tx.send_blocking((idx, file));
                    }
                    Err(e) => {
                        crate::trace::log(&format!("hydrate 读页 {} 失败: {e}", idx + 1));
                    }
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let mut done = 0usize;
            while let Ok((idx, file)) = rx.recv().await {
                done += 1;
                let d = done;
                this.update(cx, |view, cx| {
                    if view.hydrate_gen != gen {
                        return;
                    }
                    if view
                        .doc
                        .pages
                        .get(idx)
                        .map(|p| p.regions.is_empty())
                        .unwrap_or(false)
                    {
                        view.doc.apply_detect_file(idx, &file);
                        view.doc.ensure_page_groups(idx);
                    }
                    if d == n || d % 8 == 0 {
                        view.status = format!("后台识别进度 {d}/{n}…").into();
                        view.hint = view.status.clone();
                        cx.notify();
                    }
                })
                .ok();
                if d % 8 == 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(8))
                        .await;
                }
            }
            this.update(cx, |view, cx| {
                if view.hydrate_gen != gen {
                    return;
                }
                view.doc.ensure_all_page_groups();
                view.doc.retain_window(
                    view.doc.current_page_index,
                    crate::page_cache::WINDOW_RADIUS,
                );
                let n_pages = view.doc.pages.len();
                let g = view.doc.groups.len();
                view.status = format!("已载入 {n_pages} 页, {g} 个输出组合.").into();
                view.hint = view.status.clone();
                view.after_doc_change(cx);
                view.start_seed_align_anchors(cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 保存进行中时驱动标题栏拖尾转圈.
    pub(super) fn start_save_spinner(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(33))
                .await;
            let cont = this
                .update(cx, |view, cx| {
                    if !view.saving {
                        return false;
                    }
                    view.save_spin_phase = (view.save_spin_phase + 0.08) % 1.0;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !cont {
                break;
            }
        })
        .detach();
    }

    /// 落盘蒙版/同步视频时间轴后, 判定是否仍有未保存改动.
    pub(super) fn refresh_dirty_from_panels(&mut self, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
        }
        let video_snap = self.score_video.read(cx).timeline_snapshot();
        let saved = &self.doc.video_state;
        if video_snap.video_clips != saved.video_clips
            || video_snap.fades != saved.fades
            || video_snap.audio_clips != saved.audio_clips
        {
            self.dirty = true;
        }
    }

    pub(super) fn mark_video_pool_dirty_all(&mut self) {
        self.video_pool_all_dirty = true;
        self.video_pool_dirty.clear();
        self.mark_dirty();
    }

    pub(super) fn mark_video_pool_dirty_group(&mut self, gid: &str) {
        if !self.video_pool_all_dirty {
            self.video_pool_dirty.insert(gid.to_string());
        }
        self.mark_dirty();
    }

    pub(super) fn pool_cache_dir(&self) -> PathBuf {
        if let Some(ref p) = self.project_path {
            crate::page_cache::project_cache_dir(p)
        } else {
            crate::page_cache::session_dir().join("pool_cache")
        }
    }

    pub(super) fn sync_mask_image(&mut self, cx: &mut Context<Self>) {
        if self.side_tool != SideTool::Mask {
            return;
        }
        self.flush_mask_to_doc(cx);
        let side_w = self.side_width;
        let target = self.resolve_mask_target();
        self.mask_target = target.clone();
        self.ensure_mask_active_block();
        let Some(gid) = target else {
            self.mask_tool.update(cx, |m, cx| {
                m.set_embed_side_width(side_w);
                m.clear_view("请先有可编辑的组合", cx);
            });
            return;
        };
        if self.doc.ensure_group_pages(&gid).is_err() {
            self.mask_tool.update(cx, |m, cx| {
                m.set_embed_side_width(side_w);
                m.clear_view("无法加载该组合页图", cx);
            });
            return;
        }
        let Some((rgb, hoff, voff)) = self.doc.compose_group_preview(&gid) else {
            self.mask_tool.update(cx, |m, cx| {
                m.set_embed_side_width(side_w);
                m.clear_view("无法拼合该组合", cx);
            });
            return;
        };
        self.mask_preview_hoff = hoff;
        self.mask_preview_voff = voff;
        let masks: Vec<MaskRect> = self
            .doc
            .get_group_masks(&gid)
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.translate(hoff as i32, voff as i32);
                m
            })
            .collect();
        let label = self
            .doc
            .groups
            .iter()
            .position(|g| g.id == gid)
            .map(|i| self.doc.group_crop_label(i))
            .unwrap_or_else(|| "组合".into());
        let mask_prefs = self.doc.mask_prefs.clone();
        let pieces = self.doc.group_member_pieces(&gid);
        let ink_threshold = self.doc.ink_threshold;
        let heights: Vec<(String, u32)> = pieces
            .iter()
            .map(|(rid, img)| (rid.clone(), img.height()))
            .collect();
        self.block_stats_cache.clear();
        for (rid, img) in &pieces {
            self.block_stats_cache
                .insert(rid.clone(), mask_tool::layout::compute_piece_stats(img, ink_threshold));
        }
        self.block_pieces_cache = pieces;
        let tiles: Vec<mask_tool::gui::BlockTile> = self
            .block_pieces_cache
            .iter()
            .map(|(rid, img)| {
                let stats = self
                    .block_stats_cache
                    .get(rid)
                    .copied()
                    .unwrap_or_default();
                mask_tool::gui::BlockTile::from_piece(rid.clone(), img, stats)
            })
            .collect();
        let bg_tile = if self.doc.bg_enabled {
            self.doc.bg_image.as_ref().map(|img| {
                mask_tool::gui::BlockBgTile::from_rgb(img, self.doc.bg_aspect_w, self.doc.bg_aspect_h)
            })
        } else {
            None
        };
        let block_layout = self.doc.get_block_layout(&gid).to_vec();
        self.last_synced_block_layout = block_layout.clone();
        // 刚加载/尚未拖动过时, 目标纵向位置就是当前显示的位置 (与
        // `MaskToolApp::set_block_geometry` 内部对 `voff_target` 的初始化
        // 保持一致).
        self.last_synced_voff_target = voff;
        let guides = self.doc.get_group_guides(&gid);
        let bg_applied = self.doc.bg_enabled;
        let piece_ys = piece_staff_ys_from_parts(&self.block_pieces_cache, ink_threshold);
        self.mask_tool.update(cx, |m, cx| {
            m.set_embed_side_width(side_w);
            m.load_rgb(rgb, gid, masks, guides, &label, cx);
            m.apply_color_prefs(mask_prefs);
            m.set_block_geometry(heights, block_layout, hoff, voff);
            m.set_piece_staff_ys(piece_ys);
            m.set_block_tiles(tiles, bg_tile);
            m.set_bg_applied(bg_applied);
        });
    }

    /// 对齐/全局撤重后刷新当前蒙版预览, 不 `invalidate_session`, 以免冲掉撤重栈.
    pub(super) fn refresh_mask_preview_keep_history(&mut self, cx: &mut Context<Self>) {
        if self.side_tool != SideTool::Mask {
            return;
        }
        let Some(gid) = self.mask_target.clone() else {
            return;
        };
        if self.doc.ensure_group_pages(&gid).is_err() {
            return;
        }
        let Some((rgb, hoff, voff)) = self.doc.compose_group_preview(&gid) else {
            return;
        };
        self.mask_preview_hoff = hoff;
        self.mask_preview_voff = voff;
        let masks: Vec<MaskRect> = self
            .doc
            .get_group_masks(&gid)
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.translate(hoff as i32, voff as i32);
                m
            })
            .collect();
        let mask_prefs = self.doc.mask_prefs.clone();
        let pieces = self.doc.group_member_pieces(&gid);
        let ink_threshold = self.doc.ink_threshold;
        let heights: Vec<(String, u32)> = pieces
            .iter()
            .map(|(rid, img)| (rid.clone(), img.height()))
            .collect();
        self.block_stats_cache.clear();
        for (rid, img) in &pieces {
            self.block_stats_cache
                .insert(rid.clone(), mask_tool::layout::compute_piece_stats(img, ink_threshold));
        }
        self.block_pieces_cache = pieces;
        let tiles: Vec<mask_tool::gui::BlockTile> = self
            .block_pieces_cache
            .iter()
            .map(|(rid, img)| {
                let stats = self
                    .block_stats_cache
                    .get(rid)
                    .copied()
                    .unwrap_or_default();
                mask_tool::gui::BlockTile::from_piece(rid.clone(), img, stats)
            })
            .collect();
        let bg_tile = if self.doc.bg_enabled {
            self.doc.bg_image.as_ref().map(|img| {
                mask_tool::gui::BlockBgTile::from_rgb(img, self.doc.bg_aspect_w, self.doc.bg_aspect_h)
            })
        } else {
            None
        };
        let block_layout = self.doc.get_block_layout(&gid).to_vec();
        self.last_synced_block_layout = block_layout.clone();
        self.last_synced_voff_target = voff;
        let guides = self.doc.get_group_guides(&gid);
        let bg_applied = self.doc.bg_enabled;
        let piece_ys = piece_staff_ys_from_parts(&self.block_pieces_cache, ink_threshold);
        self.mask_tool.update(cx, |m, cx| {
            m.replace_session_image(rgb, masks, guides, cx);
            m.apply_color_prefs(mask_prefs);
            m.set_block_geometry(heights, block_layout, hoff, voff);
            m.set_piece_staff_ys(piece_ys);
            m.set_block_tiles(tiles, bg_tile);
            m.set_bg_applied(bg_applied);
        });
    }

    /// 精确算出「组合分块」当前布局应该写入的 `DocState::group_voff_shift`,
    /// 使得 `apply_bg::process::natural_voff(拼合图高度) + group_voff_shift
    /// == voff_target` (`MaskToolApp::voff_target` 表达的"目标纵向位置",
    /// 见其字段文档). 不假设这个关系随拼合图高度变化是线性的——拼合图
    /// 高度跨越 `apply_bg::process::frame_size` 按宽/按高定形的切换分界点
    /// 前后并不成立, 这里直接用真实宽高比重算, 保证:
    /// - 拖动结束/撤销/重做后, "未被这次改动波及的块" 的绝对位置在底色
    ///   居中合成后依然精确不变 (无论拼合图高度是否跨越了切换分界点);
    /// - 撤销/重做直接回滚 `voff_target` (随 `UndoSnapshot` 一起, 不额外
    ///   处理), 这里对同一个 `voff_target` 结合*当时*的布局重新精确反算
    ///   出的 `group_voff_shift` 与原来完全一致, 不会残留误差.
    fn resolve_group_voff_shift(&self, layout: &[mask_tool::layout::BlockAdjust], voff_target: i64) -> i64 {
        if !self.doc.bg_enabled {
            return 0;
        }
        let Some(bg) = self.doc.bg_image.as_ref() else {
            return 0;
        };
        if self.block_pieces_cache.is_empty() {
            return 0;
        }
        let heights: Vec<(String, u32)> = self
            .block_pieces_cache
            .iter()
            .map(|(rid, img)| (rid.clone(), img.height()))
            .collect();
        let sw = self.block_pieces_cache.iter().map(|(_, img)| img.width()).max().unwrap_or(1);
        let sh = mask_tool::layout::sheet_height(&heights, layout);
        let natural = apply_bg::process::natural_voff(
            sw,
            sh,
            bg.width(),
            bg.height(),
            self.doc.bg_aspect_w,
            self.doc.bg_aspect_h,
        );
        voff_target - natural
    }

    /// 蒙版拖动分块时 (`MaskToolApp::block_layout` 逐帧变化): 拖动过程中
    /// 只把布局写回文档, 画面由蒙版工具用已缓存的分块 GPU 贴图跟手绘制,
    /// 避免每帧整图重拼 + 重新上传贴图. 松手 (或撤销/重做) 后再合成一次
    /// 含底色的最终预览图回填.
    pub(super) fn sync_block_layout_from_mask_tool(
        &mut self,
        mt: &Entity<MaskToolApp>,
        cx: &mut Context<Self>,
    ) {
        if self.side_tool != SideTool::Mask {
            return;
        }
        let Some(gid) = self.mask_target.clone() else {
            return;
        };
        let dragging = mt.read(cx).is_block_dragging();
        let has_tiles = mt.read(cx).has_block_tiles();
        let active = mt.read(cx).selected_block_id().map(|s| s.to_string());
        if active.is_some() && active != self.mask_active_block_id {
            self.mask_active_block_id = active;
            self.scroll_mask_block_list_to_active();
            cx.notify();
        }
        let layout = mt.read(cx).block_layout_clone();
        let voff_target = mt.read(cx).voff_target();
        let drag_ended = self.block_drag_was_active && !dragging;
        self.block_drag_was_active = dragging;
        if layout == self.last_synced_block_layout
            && voff_target == self.last_synced_voff_target
            && !drag_ended
        {
            if dragging {
                let (hoff, voff) = mt.read(cx).preview_offsets();
                self.mask_preview_hoff = hoff;
                self.mask_preview_voff = voff;
            }
            return;
        }
        self.last_synced_block_layout = layout.clone();
        self.last_synced_voff_target = voff_target;
        let voff_shift = self.resolve_group_voff_shift(&layout, voff_target);
        self.doc.set_block_layout(&gid, layout);
        self.doc.set_group_voff_shift(&gid, voff_shift);
        if dragging && has_tiles && !drag_ended {
            let (hoff, voff) = mt.read(cx).preview_offsets();
            self.mask_preview_hoff = hoff;
            self.mask_preview_voff = voff;
            return;
        }
        let Some((rgb, hoff, voff)) = self.doc.compose_group_preview_with_parts_and_stats(
            &gid,
            &self.block_pieces_cache,
            &self.block_stats_cache,
        ) else {
            return;
        };
        let dx = (hoff - self.mask_preview_hoff) as i32;
        let dy = (voff - self.mask_preview_voff) as i32;
        self.mask_preview_hoff = hoff;
        self.mask_preview_voff = voff;
        mt.update(cx, |m, cx| {
            m.shift_masks(dx, dy);
            m.update_base_image(rgb, hoff, voff, cx);
        });
        self.mark_video_pool_dirty_group(&gid);
    }

    pub(super) fn resolve_mask_target(&self) -> Option<String> {
        if let Some(ref id) = self.doc.active_group_id {
            if self.doc.groups.iter().any(|g| &g.id == id) {
                return Some(id.clone());
            }
        }
        if let Some(ref id) = self.mask_target {
            if self.doc.groups.iter().any(|g| &g.id == id) {
                return Some(id.clone());
            }
        }
        self.doc.groups.first().map(|g| g.id.clone())
    }

    pub(super) fn flush_mask_to_doc(&mut self, cx: &mut Context<Self>) {
        let Some(gid) = self.mask_target.clone() else {
            return;
        };
        let (masks, prefs, block_layout, voff_target, guides) = self.mask_tool.update(cx, |m, _| {
            (
                m.masks_clone(),
                m.color_prefs(),
                m.block_layout_clone(),
                m.voff_target(),
                m.guides_clone(),
            )
        });
        let (hoff, voff) = (self.mask_preview_hoff, self.mask_preview_voff);
        let masks: Vec<MaskRect> = masks
            .into_iter()
            .map(|mut m| {
                m.translate(-(hoff as i32), -(voff as i32));
                m
            })
            .collect();
        let voff_shift = self.resolve_group_voff_shift(&block_layout, voff_target);
        // 关窗 / 切页签也会 flush; 内容没变时不能把工程标脏, 否则刚保存完
        // 退出仍会弹出未保存.
        let masks_changed = masks.as_slice() != self.doc.get_group_masks(&gid);
        let guides_changed = guides != self.doc.get_group_guides(&gid);
        let layout_changed =
            block_layout_effectively_differs(&block_layout, self.doc.get_block_layout(&gid));
        let voff_changed = voff_shift != self.doc.get_group_voff_shift(&gid);
        let prefs_changed = prefs != self.doc.mask_prefs.clone().clamp();
        if masks_changed {
            self.doc.set_group_masks(&gid, masks);
        }
        if guides_changed {
            self.doc.set_group_guides(&gid, guides);
        }
        if layout_changed {
            self.doc.set_block_layout(&gid, block_layout.clone());
        }
        if voff_changed {
            self.doc.set_group_voff_shift(&gid, voff_shift);
        }
        if prefs_changed {
            self.doc.mask_prefs = prefs.clone();
            config::remember_mask_prefs(&prefs);
        }
        if masks_changed || layout_changed || voff_changed {
            self.mark_video_pool_dirty_group(&gid);
        } else if guides_changed || prefs_changed {
            self.mark_dirty();
        }
    }

    /// `scroll_other`: 点顶部页签时滚侧栏列表定位; 点侧栏自身则两边都不滚.
    pub(super) fn set_mask_target(&mut self, group_id: String, scroll_other: bool, cx: &mut Context<Self>) {
        if self.mask_target.as_ref() == Some(&group_id) {
            return;
        }
        self.flush_mask_to_doc(cx);
        self.doc.active_group_id = Some(group_id);
        self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
        self.mask_target = None;
        self.sync_mask_image(cx);
        if scroll_other {
            self.scroll_mask_block_list_to_active();
        }
        cx.notify();
    }

    pub(super) fn set_side_tool(&mut self, tool: SideTool, window: &mut Window, cx: &mut Context<Self>) {
        if self.side_tool == tool {
            return;
        }
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
            self.doc.retain_window(
                self.doc.current_page_index,
                crate::page_cache::WINDOW_RADIUS,
            );
        }
        self.side_tool = tool;
        match tool {
            SideTool::Crop => {
                // 回到分块: 定位到当前蒙版组合所在页并选中该组
                self.restore_crop_from_mask_target(cx);
                self.scroll_page_tabs_to_index(self.doc.current_page_index);
                self.focus_handle.focus(window);
                self.status = "分块工具".into();
                self.hint = format!(
                    "拖入/打开图片、PDF 或工程. {}S 保存工程.",
                    apply_bg::primary_mod()
                )
                .into();
            }
            SideTool::Mask => {
                self.mask_target = None;
                self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
                self.sync_mask_image(cx);
                self.scroll_mask_lists_to_active();
                self.mask_tool.read(cx).focus_handle_ref().focus(window);
                self.status = "蒙版工具".into();
                self.hint =
                    format!(
                        "蒙版编辑当前组合的拼合图. 标签切换组合; {}A 全选蒙版.",
                        apply_bg::primary_mod()
                    )
                    .into();
            }
            SideTool::Project => {
                self.scroll_page_tabs_to_index(self.doc.current_page_index);
                self.focus_handle.focus(window);
                self.status = "工程工具".into();
                self.hint =
                    "打开/保存工程; 下方加底色可「应用到工程组合」(双层, 可取消) 或批量导出目录."
                        .into();
            }
            SideTool::Video => {
                self.sync_video_pool(cx);
                self.score_video
                    .read(cx)
                    .focus_handle_ref()
                    .clone()
                    .focus(window);
                self.status = "视频工具".into();
                self.hint =
                    "N 插入下一张组合 | 空格播放/暂停 | ←→ 快退快进 | I/O 标记淡入淡出."
                        .into();
            }
        }
        cx.notify();
    }

    /// 把「输出组合」渲染为终稿写入工程旁持久缓存, 再同步给视频素材池 (LRU 热加载).
    pub(super) fn sync_video_pool(&mut self, cx: &mut Context<Self>) {
        self.video_sync_gen = self.video_sync_gen.wrapping_add(1);
        let gen = self.video_sync_gen;
        let group_ids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let (aw, ah) = (self.doc.bg_aspect_w, self.doc.bg_aspect_h);
        let fade_bg = sample_paper_rgb(self.doc.bg_image.as_ref());
        self.score_video.update(cx, |v, _| {
            v.set_aspect(aw, ah);
            v.set_fade_bg_rgb(fade_bg);
        });
        if group_ids.is_empty() {
            self.score_video.update(cx, |v, cx| v.set_pool(Vec::new(), cx));
            return;
        }
        let cache_root = self.pool_cache_dir().join("pool");
        let _ = std::fs::create_dir_all(&cache_root);
        let all_dirty = self.video_pool_all_dirty;
        let dirty_set = self.video_pool_dirty.clone();
        // 估算并发: 取当前页峰值近似
        let peak = self
            .doc
            .pages
            .first()
            .map(|p| p.estimated_bytes().saturating_mul(3))
            .unwrap_or(64 * 1024 * 1024);
        let conc = crate::page_cache::concurrency_for_peak(peak.max(128 * 1024 * 1024));

        cx.spawn(async move |this, cx| {
            let mut items: Vec<MaterialItem> = Vec::with_capacity(group_ids.len());
            for (chunk_i, chunk) in group_ids.chunks(conc.max(1)).enumerate() {
                if chunk_i > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                crate::trace::log(&format!(
                    "video_pool: 开始 chunk {} ({} 组)",
                    chunk_i + 1,
                    chunk.len()
                ));
                let cancelled = this
                    .update(cx, |view, _| {
                        if view.video_sync_gen != gen {
                            return true;
                        }
                        for gid in chunk {
                            let Some(idx) =
                                view.doc.groups.iter().position(|g| &g.id == gid)
                            else {
                                continue;
                            };
                            let label = view.doc.groups[idx].display_name(idx);
                            let cache_path = cache_root.join(format!("{gid}.png"));
                            let need_rebuild = all_dirty
                                || dirty_set.contains(gid)
                                || !cache_path.is_file();
                            if need_rebuild {
                                let _ = view.doc.ensure_group_pages(gid);
                                match view.doc.render_group_final(gid) {
                                    Ok(Some(rgb)) => {
                                        if rgb.save(&cache_path).is_err() {
                                            continue;
                                        }
                                        items.push(MaterialItem {
                                            group_id: gid.clone(),
                                            label: label.into(),
                                            width: rgb.width(),
                                            height: rgb.height(),
                                            cache_path,
                                        });
                                    }
                                    _ => continue,
                                }
                                view.doc.retain_window(
                                    view.doc.current_page_index,
                                    crate::page_cache::WINDOW_RADIUS,
                                );
                            } else if let Ok((w, h)) = image::image_dimensions(&cache_path) {
                                items.push(MaterialItem {
                                    group_id: gid.clone(),
                                    label: label.into(),
                                    width: w,
                                    height: h,
                                    cache_path,
                                });
                            }
                        }
                        false
                    })
                    .unwrap_or(true);
                crate::trace::log(&format!(
                    "video_pool: chunk {} 结束 cancelled={cancelled}",
                    chunk_i + 1
                ));
                if cancelled {
                    return;
                }
            }
            crate::trace::log("video_pool: 全部 chunk 完成, 写回素材池");
            this.update(cx, |view, cx| {
                if view.video_sync_gen == gen {
                    view.video_pool_all_dirty = false;
                    view.video_pool_dirty.clear();
                    view.score_video.update(cx, |v, cx| v.set_pool(items, cx));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// 从蒙版目标恢复分块页签与选中组合.
    pub(super) fn restore_crop_from_mask_target(&mut self, cx: &mut Context<Self>) {
        let gid = self
            .mask_target
            .clone()
            .or_else(|| self.doc.active_group_id.clone());
        let Some(gid) = gid else {
            return;
        };
        self.doc.active_group_id = Some(gid.clone());
        let Some(g) = self.doc.groups.iter().find(|g| g.id == gid).cloned() else {
            return;
        };
        self.doc.selected_region_ids = g.region_ids.iter().cloned().collect();
        if let Some(rid) = g.region_ids.first() {
            if let Some((pi, _)) = self.doc.find_region(rid) {
                if pi != self.doc.current_page_index {
                    self.switch_page(pi, cx);
                    self.scroll_group_list_to_active();
                    return;
                }
            }
        }
        self.scroll_group_list_to_active();
        cx.notify();
    }

    pub(super) fn after_doc_change(&mut self, cx: &mut Context<Self>) {
        self.cancel_align_all();
        self.doc.prune_dangling_groups_if_hydrated();
        self.doc.sync_group_colors();
        self.doc.seed_guide_defaults();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        // 若当前页尺寸变了不必重渲整图, 但区域会重绘
        if let Some(page) = self.doc.current_page() {
            if page.width() != self.img_w || page.height() != self.img_h {
                self.refresh_render(cx);
                return;
            }
        } else {
            self.render_image = None;
            self.img_w = 0;
            self.img_h = 0;
        }
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
            self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
            self.mask_target = None;
            self.sync_mask_image(cx);
        }
        cx.notify();
    }
}

/// 与 `DocState::set_block_layout` 一致: 全是空操作等价于未设置.
fn block_layout_effectively_differs(
    new: &[mask_tool::layout::BlockAdjust],
    stored: &[mask_tool::layout::BlockAdjust],
) -> bool {
    let new_live = new.iter().any(|a| !a.is_noop());
    let stored_live = stored.iter().any(|a| !a.is_noop());
    match (new_live, stored_live) {
        (false, false) => false,
        (true, true) => new != stored,
        _ => true,
    }
}

#[cfg(test)]
mod layout_diff_tests {
    use super::block_layout_effectively_differs;
    use mask_tool::layout::BlockAdjust;

    fn adj(id: &str, extra_top: i32) -> BlockAdjust {
        BlockAdjust {
            region_id: id.into(),
            extra_top,
            extra_bottom: 0,
            gap_before: 0,
            gap_after: 0,
        }
    }

    #[test]
    fn empty_and_all_noop_layouts_are_the_same() {
        assert!(!block_layout_effectively_differs(&[], &[]));
        assert!(!block_layout_effectively_differs(&[adj("a", 0)], &[]));
        assert!(!block_layout_effectively_differs(&[], &[adj("a", 0)]));
    }

    #[test]
    fn live_layout_differs_from_empty() {
        assert!(block_layout_effectively_differs(&[adj("a", 4)], &[]));
        assert!(block_layout_effectively_differs(&[], &[adj("a", 4)]));
        assert!(block_layout_effectively_differs(&[adj("a", 4)], &[adj("a", 5)]));
    }
}

fn sample_paper_rgb(img: Option<&image::RgbImage>) -> [u8; 3] {
    let Some(img) = img else {
        return [0xE8, 0xD4, 0xB0];
    };
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return [0xE8, 0xD4, 0xB0];
    };
    let mut rs = 0u64;
    let mut gs = 0u64;
    let mut bs = 0u64;
    let mut n = 0u64;
    for yi in 0..8u32 {
        for xi in 0..8u32 {
            let x = (w - 1) * xi / 7;
            let y = (h - 1) * yi / 7;
            let p = img.get_pixel(x, y);
            rs += p[0] as u64;
            gs += p[1] as u64;
            bs += p[2] as u64;
            n += 1;
        }
    }
    [
        (rs / n) as u8,
        (gs / n) as u8,
        (bs / n) as u8,
    ]
}

/// 在原始裁切条带上算锚点 (当前算法), 不读 sidecar 里可能过期的值,
/// 也不在缩放后的预览画布上重检.
fn piece_staff_ys_from_parts(
    parts: &[(String, image::RgbImage)],
    threshold: i32,
) -> HashMap<String, Option<i32>> {
    parts
        .iter()
        .map(|(id, img)| {
            let y1 = (img.height() as i32).saturating_sub(1);
            (
                id.clone(),
                mask_tool::staff::band_staff_anchor(img, 0, y1, threshold),
            )
        })
        .collect()
}
