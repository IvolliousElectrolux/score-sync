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
        self.mask_tool.update(cx, |m, cx| {
            m.set_embed_side_width(side_w);
            m.load_rgb(rgb, gid, masks, &label, cx);
            m.apply_color_prefs(mask_prefs);
        });
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
        let (masks, prefs) = self
            .mask_tool
            .update(cx, |m, _| (m.masks_clone(), m.color_prefs()));
        let (hoff, voff) = (self.mask_preview_hoff, self.mask_preview_voff);
        let masks: Vec<MaskRect> = masks
            .into_iter()
            .map(|mut m| {
                m.translate(-(hoff as i32), -(voff as i32));
                m
            })
            .collect();
        self.doc.set_group_masks(&gid, masks);
        self.doc.mask_prefs = prefs.clone();
        config::remember_mask_prefs(&prefs);
        self.mark_dirty();
        self.mark_video_pool_dirty_group(&gid);
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
            self.scroll_mask_picker_to_active();
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
                self.focus_handle.focus(window);
                self.status = "分块工具".into();
                self.hint = "拖入/打开图片、PDF 或工程. Ctrl+S 保存工程.".into();
            }
            SideTool::Mask => {
                self.mask_target = None;
                self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
                self.sync_mask_image(cx);
                self.scroll_mask_lists_to_active();
                self.mask_tool.read(cx).focus_handle_ref().focus(window);
                self.status = "蒙版工具".into();
                self.hint =
                    "蒙版编辑当前组合的拼合图. 标签切换组合; Ctrl+A 全选蒙版."
                        .into();
            }
            SideTool::Project => {
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
        self.score_video.update(cx, |v, _| v.set_aspect(aw, ah));
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
        self.doc.sync_group_colors();
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
