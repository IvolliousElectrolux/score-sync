//! 页图窗口、蒙版/视频同步、面板切换.

use super::*;
use super::ScoreSyncApp;

/// `sync_video_pool` 每个分片在主线程收集好、交给后台线程处理的一条素材.
/// `job` 为 `None` 表示该组合当前没有可用成员片段
/// (`prepare_group_render_job` 判定为空); 后台若渲染失败会尝试沿用已有缓存.
struct VideoPoolRebuildEntry {
    gid: String,
    label: String,
    cache_path: PathBuf,
    job: Option<crate::model::GroupRenderJob>,
}

/// 蒙版/底色预览后台任务的一条成员: 已在内存的页图只带 `Arc` (不拷像素),
/// 未加载的只带磁盘路径, 解码和裁切都在工作线程做.
struct MaskPreviewMemberSnap {
    rid: String,
    page_idx: usize,
    y0: u32,
    height: u32,
    image: Option<Arc<image::RgbImage>>,
    disk_path: PathBuf,
}

struct MaskPreviewBuilt {
    loaded_pages: Vec<(usize, Arc<image::RgbImage>)>,
    piece_sizes: Vec<(String, u32, u32)>,
    piece_ys: HashMap<String, Option<i32>>,
    tiles: Vec<mask_tool::gui::BlockTile>,
    bg_tile: Option<mask_tool::gui::BlockBgTile>,
    canvas_w: u32,
    canvas_h: u32,
    hoff: i64,
    voff: i64,
}

fn collect_mask_preview_members(
    doc: &crate::model::DocState,
    gid: &str,
) -> Vec<MaskPreviewMemberSnap> {
    let Some(g) = doc.groups.iter().find(|g| g.id == gid) else {
        return Vec::new();
    };
    g.region_ids
        .iter()
        .filter_map(|rid| {
            let (pi, r) = doc.find_region(rid)?;
            let page = doc.pages.get(pi)?;
            let y0 = r.y0.max(0) as u32;
            let y1 = (r.y1 as u32).min(page.height().saturating_sub(1));
            if y1 < y0 {
                return None;
            }
            Some(MaskPreviewMemberSnap {
                rid: rid.clone(),
                page_idx: pi,
                y0,
                height: y1 - y0 + 1,
                image: page.image.clone(),
                disk_path: page.disk_path.clone(),
            })
        })
        .collect()
}

/// 磁盘解码 + 条带裁切 + 分块/底色缩略图贴图, 全部在后台线程跑.
/// 预览不再把底色烧进整张拼合图: 画布用三层 GPU 贴图 (底色 / 组合 /
/// 画迹), 逻辑尺寸只来自 `preview_frame`. 终稿拼合仍走 `GroupRenderJob`.
fn build_mask_preview(
    members: Vec<MaskPreviewMemberSnap>,
    ink_threshold: i32,
    layout: Vec<mask_tool::layout::BlockAdjust>,
    bg_enabled: bool,
    bg_image: Option<Arc<image::RgbImage>>,
    bg_solid: Option<[u8; 3]>,
    bg_src_w: u32,
    bg_src_h: u32,
    bg_aspect_w: u32,
    bg_aspect_h: u32,
    voff_shift: i64,
    compute_bg_tile: bool,
) -> Result<MaskPreviewBuilt, String> {
    let mut loaded_pages = Vec::new();
    let mut pieces = Vec::new();
    for m in members {
        let img = if let Some(existing) = m.image {
            existing
        } else {
            let rgb = crate::page_cache::load_rgb(&m.disk_path)?;
            let a = Arc::new(rgb);
            loaded_pages.push((m.page_idx, a.clone()));
            a
        };
        let piece = crate::model::crop_band_fast(&img, m.y0, m.height);
        pieces.push((m.rid, piece));
    }
    if pieces.is_empty() {
        return Err("无法拼合该组合".into());
    }
    let mut stats = HashMap::new();
    for (rid, img) in &pieces {
        stats.insert(
            rid.clone(),
            mask_tool::layout::compute_piece_stats(img, ink_threshold),
        );
    }
    let heights: Vec<(String, u32)> = pieces
        .iter()
        .map(|(rid, img)| (rid.clone(), img.height()))
        .collect();
    let sheet_w = pieces.iter().map(|(_, img)| img.width()).max().unwrap_or(1);
    let sheet_h = mask_tool::layout::sheet_height(&heights, &layout);
    let (canvas_w, canvas_h, hoff, voff) = if bg_enabled && bg_src_w > 0 && bg_src_h > 0 {
        let frame = apply_bg::process::preview_frame(
            sheet_w,
            sheet_h,
            bg_src_w,
            bg_src_h,
            bg_aspect_w,
            bg_aspect_h,
            voff_shift,
        );
        (frame.canvas_w, frame.canvas_h, frame.hoff, frame.voff)
    } else {
        (sheet_w.max(1), sheet_h.max(1), 0, 0)
    };
    let piece_ys = mask_tool::staff::piece_staff_ys_from_parts(&pieces, ink_threshold);
    let tiles: Vec<mask_tool::gui::BlockTile> = pieces
        .iter()
        .map(|(rid, img)| {
            let st = stats.get(rid).copied().unwrap_or_default();
            mask_tool::gui::BlockTile::from_piece(rid.clone(), img, st)
        })
        .collect();
    let bg_tile = if compute_bg_tile {
        if let Some(color) = bg_solid {
            mask_tool::gui::BlockBgTile::from_solid(
                color,
                bg_aspect_w,
                bg_aspect_h,
                sheet_w,
                bg_src_w,
                bg_src_h,
            )
        } else {
            bg_image.and_then(|img| {
                mask_tool::gui::BlockBgTile::from_full(&img, bg_aspect_w, bg_aspect_h, sheet_w)
            })
        }
    } else {
        None
    };
    let piece_sizes: Vec<(String, u32, u32)> = pieces
        .iter()
        .map(|(rid, img)| (rid.clone(), img.width(), img.height()))
        .collect();
    drop(pieces);
    Ok(MaskPreviewBuilt {
        loaded_pages,
        piece_sizes,
        piece_ys,
        tiles,
        bg_tile,
        canvas_w,
        canvas_h,
        hoff,
        voff,
    })
}

impl ScoreSyncApp {
    /// 命中 `bg_tile_cache` (`bg_gen` / 纵横比 / 谱面宽都未变, 即目标页
    /// 裁切没换) 时直接复用, 免掉一次 `BlockBgTile::from_full` 里对裁切
    /// 画布的缩放.
    fn cached_bg_tile(
        &self,
        gen: u64,
        aspect_w: u32,
        aspect_h: u32,
        sheet_w: u32,
    ) -> Option<mask_tool::gui::BlockBgTile> {
        let (g, sw, tile) = self.bg_tile_cache.as_ref()?;
        if *g != gen || *sw != sheet_w || tile.aspect_w != aspect_w || tile.aspect_h != aspect_h {
            return None;
        }
        Some(tile.clone())
    }

    fn sync_mask_preview_offsets(&mut self) {
        if let Some(gid) = self.mask_target.as_ref() {
            if let Some(f) = self.doc.group_preview_frame(gid) {
                self.mask_preview_hoff = f.hoff;
                self.mask_preview_voff = f.voff;
                return;
            }
        }
        self.mask_preview_hoff = 0;
        self.mask_preview_voff = 0;
    }

    /// 只换底色层, 不 `clear_view` / 不重解码谱面. 纯色在界面线程即时完成;
    /// 图片底色缩略图仍丢到后台.
    pub(super) fn refresh_bg_preview_layer(&mut self, cx: &mut Context<Self>) {
        if !self.uses_mask_canvas() {
            return;
        }
        let loading = self.mask_tool.read(cx).is_canvas_loading();
        let has_tiles = self.mask_tool.read(cx).has_block_tiles();
        if loading || !has_tiles || self.mask_target.is_none() {
            self.sync_mask_image(cx);
            return;
        }
        let Some(gid) = self.mask_target.clone() else {
            self.sync_mask_image(cx);
            return;
        };
        let heights = self.doc.group_member_heights(&gid);
        let layout = self.doc.get_block_layout(&gid).to_vec();
        let intended = self.intended_voff_target(&gid, &heights, &layout);
        let bg_applied = self.doc.bg_enabled;
        if !bg_applied {
            self.bg_tile_cache = None;
            self.mask_tool.update(cx, |m, cx| {
                m.apply_host_bg_tile(None, intended, false);
                cx.notify();
            });
            self.sync_mask_preview_offsets();
            return;
        }
        let sheet_w = self.doc.group_sheet_width(&gid);
        let aw = self.doc.bg_aspect_w;
        let ah = self.doc.bg_aspect_h;
        let gen = self.doc.bg_gen;
        if let Some(color) = self.doc.bg_solid {
            let (src_w, src_h) = self.doc.bg_src_size().unwrap_or((sheet_w.max(1), 1));
            if let Some(tile) =
                mask_tool::gui::BlockBgTile::from_solid(color, aw, ah, sheet_w, src_w, src_h)
            {
                self.bg_tile_cache = Some((gen, sheet_w, tile.clone()));
                self.mask_tool.update(cx, |m, cx| {
                    m.apply_host_bg_tile(Some(tile), intended, true);
                    cx.notify();
                });
                self.sync_mask_preview_offsets();
                return;
            }
        }
        if let Some(cached) = self.cached_bg_tile(gen, aw, ah, sheet_w) {
            self.mask_tool.update(cx, |m, cx| {
                m.apply_host_bg_tile(Some(cached), intended, true);
                cx.notify();
            });
            self.sync_mask_preview_offsets();
            return;
        }
        let Some(img) = self.doc.bg_image.clone() else {
            self.bg_tile_cache = None;
            self.mask_tool.update(cx, |m, cx| {
                m.apply_host_bg_tile(None, intended, false);
                cx.notify();
            });
            self.sync_mask_preview_offsets();
            return;
        };
        self.mask_sync_gen = self.mask_sync_gen.wrapping_add(1);
        let mask_gen = self.mask_sync_gen;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let tile = mask_tool::gui::BlockBgTile::from_full(&img, aw, ah, sheet_w);
            let _ = tx.send_blocking(tile);
        });
        cx.spawn(async move |this, cx| {
            let Ok(tile) = rx.recv().await else {
                return;
            };
            this.update(cx, |view, cx| {
                if view.mask_sync_gen != mask_gen {
                    return;
                }
                if let Some(t) = tile.clone() {
                    view.bg_tile_cache = Some((gen, sheet_w, t));
                }
                let heights = view.doc.group_member_heights(&gid);
                let layout = view.doc.get_block_layout(&gid).to_vec();
                let intended = view.intended_voff_target(&gid, &heights, &layout);
                view.mask_tool.update(cx, |m, cx| {
                    m.apply_host_bg_tile(tile, intended, true);
                    cx.notify();
                });
                view.sync_mask_preview_offsets();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn retire_current_render_image(&mut self) {
        if let Some(img) = self.render_image.take() {
            self.gpu_drop.push(img);
        }
    }

    pub(super) fn flush_gpu_drops(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for img in std::mem::take(&mut self.gpu_drop) {
            let _ = window.drop_image(img);
        }
        let extra = self.mask_tool.update(cx, |m, _| m.take_gpu_drops());
        for img in extra {
            let _ = window.drop_image(img);
        }
    }

    /// 把当前页像素换成可显示的 GPU 贴图. `mask_tool::gui::rgb_to_render_image`
    /// 对高清页 (如 4500×6000 扫描页) 自己就要上百毫秒 (缩放到贴图上限
    /// + 逐像素 RGB→BGRA), 绝不能在界面线程同步做——那样切页/撤重/新建
    /// 分块等几乎任何触发它的操作都会卡一下 (高清多页工程尤其明显).
    /// 转换挪到后台线程, 界面线程只在结果送回来时换一次贴图指针;
    /// `render_gen` 保证连续切页时旧结果不会晚到覆盖新页面.
    pub(super) fn refresh_render(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.doc.current_page() else {
            self.retire_current_render_image();
            self.img_w = 0;
            self.img_h = 0;
            cx.notify();
            return;
        };
        let (w, h) = (page.width(), page.height());
        let Some(img) = page.image.clone() else {
            // 占位: 尺寸已知但像素未到, 触发异步窗口加载
            self.retire_current_render_image();
            self.img_w = w;
            self.img_h = h;
            self.request_page_window(cx);
            cx.notify();
            return;
        };
        // 先卸旧贴图 + 落定新尺寸 (区域叠加线等元数据立刻跟手), 图像素本身
        // 走"加载中"占位直到后台转换完成, 避免新旧页尺寸/像素暂时对不上.
        self.retire_current_render_image();
        self.img_w = w;
        self.img_h = h;
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.render_gen = self.render_gen.wrapping_add(1);
        let gen = self.render_gen;
        let (tx, rx) = async_channel::bounded::<Arc<RenderImage>>(1);
        std::thread::spawn(move || {
            let tex = mask_tool::gui::rgb_to_render_image(&img);
            let _ = tx.send_blocking(tex);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(tex) = rx.recv().await {
                this.update(cx, |view, cx| {
                    if view.render_gen != gen {
                        return; // 又切走了, 这份贴图已经过期
                    }
                    view.render_image = Some(tex);
                    view.sync_mask_image(cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// 异步加载当前页附近窗口并释放窗外页图.
    pub(super) fn request_page_window(&mut self, cx: &mut Context<Self>) {
        self.page_load_gen = self.page_load_gen.wrapping_add(1);
        let gen = self.page_load_gen;
        let center = self.doc.current_page_index;
        let radius = self.doc.memory_window_radius();
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
                            page.image = Some(Arc::new(img));
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
                view.doc.retain_memory_window();
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

    /// 切蒙版目标 (含侧栏面板首次切入). 磁盘解码、条带裁切、分块/底色
    /// 缩略图贴图全部在后台线程; 界面线程只先挂「加载中」占位.
    /// 预览分三层画, 不在这里烧底色. `mask_sync_gen` 保证连续切换时旧
    /// 结果不会晚到覆盖新目标.
    pub(super) fn sync_mask_image(&mut self, cx: &mut Context<Self>) {
        if !self.uses_mask_canvas() {
            return;
        }
        self.mask_sync_gen = self.mask_sync_gen.wrapping_add(1);
        let gen = self.mask_sync_gen;
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
        }
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
        let label = self
            .doc
            .groups
            .iter()
            .position(|g| g.id == gid)
            .map(|i| self.doc.group_crop_label(i))
            .unwrap_or_else(|| "组合".into());
        self.mask_tool.update(cx, |m, cx| {
            m.set_embed_side_width(side_w);
            m.begin_preview_load(format!("正在生成「{label}」预览…"), cx);
        });
        self.spawn_mask_preview(gen, gid, label, false, cx);
    }

    /// 对齐/全局撤重后刷新当前蒙版预览, 不 `invalidate_session`, 以免冲掉撤重栈.
    ///
    /// 与 `sync_mask_image` 同一套后台生成套路 (谱表锚点扫描 + 各分块/
    /// 底色缩略图贴图全部挪到后台线程), 复用同一个 `mask_sync_gen`
    /// 代次判重, 避免全局对齐/撤重这类批量操作后卡在界面线程上.
    pub(super) fn refresh_mask_preview_keep_history(&mut self, cx: &mut Context<Self>) {
        if self.side_tool != SideTool::Mask {
            return;
        }
        let Some(gid) = self.mask_target.clone() else {
            return;
        };
        let label = self
            .doc
            .groups
            .iter()
            .position(|g| g.id == gid)
            .map(|i| self.doc.group_crop_label(i))
            .unwrap_or_else(|| "组合".into());
        self.mask_sync_gen = self.mask_sync_gen.wrapping_add(1);
        let gen = self.mask_sync_gen;
        self.spawn_mask_preview(gen, gid, label, true, cx);
    }

    /// 收集廉价快照后把解码/裁切/缩略图贴图全部丢到后台.
    /// `keep_history`: 对齐/撤重后刷新, 不 `clear_view`, 用
    /// `replace_layered_preview` 保住撤重栈.
    fn spawn_mask_preview(
        &mut self,
        gen: u64,
        gid: String,
        label: String,
        keep_history: bool,
        cx: &mut Context<Self>,
    ) {
        let side_w = self.side_width;
        let members = collect_mask_preview_members(&self.doc, &gid);
        if members.is_empty() {
            if !keep_history {
                self.mask_tool.update(cx, |m, cx| {
                    m.set_embed_side_width(side_w);
                    m.clear_view("无法拼合该组合", cx);
                });
            }
            return;
        }
        let ink_threshold = self.doc.ink_threshold;
        let block_layout = self.doc.get_block_layout(&gid).to_vec();
        self.last_synced_block_layout = block_layout.clone();
        let heights_meta = self.doc.group_member_heights(&gid);
        let intended = self.intended_voff_target(&gid, &heights_meta, &block_layout);
        self.last_synced_voff_target = intended;
        let mask_prefs = self.doc.mask_prefs.clone();
        let guides = self.doc.get_group_guides(&gid);
        let bg_applied = self.doc.bg_enabled;
        let bg_gen = self.doc.bg_gen;
        let bg_aspect_w = self.doc.bg_aspect_w;
        let bg_aspect_h = self.doc.bg_aspect_h;
        let sheet_w = self.doc.group_sheet_width(&gid);
        let cached_bg_tile = if self.doc.bg_enabled {
            self.cached_bg_tile(bg_gen, bg_aspect_w, bg_aspect_h, sheet_w)
        } else {
            None
        };
        let (bg_src_w, bg_src_h) = if let Some(t) = cached_bg_tile.as_ref() {
            (t.src_width, t.src_height)
        } else {
            self.doc.bg_src_size().unwrap_or((0, 0))
        };
        let bg_solid = self.doc.bg_solid;
        let compute_bg_tile = bg_applied
            && cached_bg_tile.is_none()
            && (self.doc.bg_image.is_some() || bg_solid.is_some());
        let bg_image = if compute_bg_tile && bg_solid.is_none() {
            self.doc.bg_image.clone()
        } else {
            None
        };
        let voff_shift = self.doc.get_group_voff_shift(&gid);
        let doc_masks = self.doc.get_group_masks(&gid).to_vec();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = build_mask_preview(
                members,
                ink_threshold,
                block_layout.clone(),
                bg_applied,
                bg_image,
                bg_solid,
                bg_src_w,
                bg_src_h,
                bg_aspect_w,
                bg_aspect_h,
                voff_shift,
                compute_bg_tile,
            );
            let _ = tx.send_blocking((result, block_layout));
        });
        cx.spawn(async move |this, cx| {
            let Ok((result, block_layout)) = rx.recv().await else {
                return;
            };
            this.update(cx, |view, cx| {
                if view.mask_sync_gen != gen {
                    return;
                }
                let built = match result {
                    Ok(b) => b,
                    Err(e) => {
                        if !keep_history {
                            view.mask_tool.update(cx, |m, cx| {
                                m.set_embed_side_width(side_w);
                                m.clear_view(e, cx);
                            });
                        }
                        return;
                    }
                };
                for (idx, img) in built.loaded_pages {
                    if let Some(page) = view.doc.pages.get_mut(idx) {
                        page.img_w = img.width();
                        page.img_h = img.height();
                        if page.image.is_none() {
                            page.image = Some(img);
                        }
                    }
                    view.doc.seed_region_anchors_for_page(idx);
                }
                let heights: Vec<(String, u32)> = built
                    .piece_sizes
                    .iter()
                    .map(|(rid, _, h)| (rid.clone(), *h))
                    .collect();
                view.block_piece_sizes = built.piece_sizes;
                view.mask_preview_hoff = built.hoff;
                view.mask_preview_voff = built.voff;
                if let Some(t) = built.bg_tile.clone() {
                    let sw = t.width; // 裁切画布宽 == 谱面宽 (按宽定高)
                    view.bg_tile_cache = Some((bg_gen, sw, t));
                }
                let bg_tile = cached_bg_tile.or(built.bg_tile);
                let masks: Vec<MaskRect> = doc_masks
                    .into_iter()
                    .map(|mut m| {
                        m.translate(built.hoff as i32, built.voff as i32);
                        m
                    })
                    .collect();
                view.mask_tool.update(cx, |m, cx| {
                    m.set_embed_side_width(side_w);
                    if keep_history {
                        m.replace_layered_preview(
                            built.canvas_w,
                            built.canvas_h,
                            masks,
                            guides,
                            cx,
                        );
                    } else {
                        m.load_layered_preview(
                            built.canvas_w,
                            built.canvas_h,
                            gid,
                            masks,
                            guides,
                            &label,
                            cx,
                        );
                    }
                    m.apply_color_prefs(mask_prefs);
                    m.set_block_geometry(heights, block_layout, built.hoff, built.voff);
                    m.set_voff_target(intended);
                    m.set_piece_staff_ys(built.piece_ys);
                    m.set_block_tiles(built.tiles, bg_tile);
                    m.set_bg_applied(bg_applied);
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let Some((bw, bh)) = self.doc.bg_src_size() else {
            return 0;
        };
        if self.block_piece_sizes.is_empty() {
            return 0;
        }
        let heights: Vec<(String, u32)> = self
            .block_piece_sizes
            .iter()
            .map(|(rid, _, h)| (rid.clone(), *h))
            .collect();
        let sw = self.block_piece_sizes.iter().map(|(_, w, _)| *w).max().unwrap_or(1);
        let sh = mask_tool::layout::sheet_height(&heights, layout);
        let natural = apply_bg::process::natural_voff(
            sw,
            sh,
            bw,
            bh,
            self.doc.bg_aspect_w,
            self.doc.bg_aspect_h,
        );
        voff_target - natural
    }

    /// 文档里存的是 `voff_shift`; 蒙版工具要的是 `voff_target =
    /// natural + shift`. 对齐后不能再用合成预览的显示 voff 当目标,
    /// 否则会把已经折进布局的留白再算一遍, 全局对齐就会整体偏移.
    fn intended_voff_target(
        &self,
        gid: &str,
        heights: &[(String, u32)],
        layout: &[mask_tool::layout::BlockAdjust],
    ) -> i64 {
        if !self.doc.bg_enabled {
            return 0;
        }
        let Some((bw, bh)) = self.doc.bg_src_size() else {
            return 0;
        };
        if heights.is_empty() {
            return 0;
        }
        let sw = self.doc.group_sheet_width(gid);
        let sh = mask_tool::layout::sheet_height(heights, layout);
        let natural = apply_bg::process::natural_voff(
            sw,
            sh,
            bw,
            bh,
            self.doc.bg_aspect_w,
            self.doc.bg_aspect_h,
        );
        natural + self.doc.get_group_voff_shift(gid)
    }

    /// 蒙版拖动分块时 (`MaskToolApp::block_layout` 逐帧变化): 拖动过程中
    /// 只把布局写回文档, 画面由蒙版工具用已缓存的分块 GPU 贴图跟手绘制.
    /// 底色 / 组合 / 画迹始终分三层显示, 松手后不必再拼一张含底色的整图.
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
        let (cur_hoff, cur_voff) = mt.read(cx).preview_offsets();
        let drag_ended = self.block_drag_was_active && !dragging;
        self.block_drag_was_active = dragging;
        if layout == self.last_synced_block_layout
            && voff_target == self.last_synced_voff_target
            && !drag_ended
        {
            if dragging {
                self.mask_preview_hoff = cur_hoff;
                self.mask_preview_voff = cur_voff;
            }
            return;
        }
        self.last_synced_block_layout = layout.clone();
        self.last_synced_voff_target = voff_target;
        let voff_shift = self.resolve_group_voff_shift(&layout, voff_target);
        self.doc.set_block_layout(&gid, layout);
        self.doc.set_group_voff_shift(&gid, voff_shift);
        // 先跟蒙版工具当前画布对齐 (拖动中的 live geom, 或撤重刚还原的
        // geom). 预览始终用三层贴图, 不必再拼一张含底色的整图回填.
        self.mask_preview_hoff = cur_hoff;
        self.mask_preview_voff = cur_voff;
        if has_tiles {
            if !dragging {
                mt.update(cx, |m, _| m.release_block_tile_preview());
                self.mark_video_pool_dirty_group(&gid);
            }
            return;
        }
        mt.update(cx, |m, _| m.release_block_tile_preview());
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

    /// 蒙版侧「导出本页」: 预览是三层贴图, 这里才按终稿拼合 (蒙版 + 底色).
    pub(super) fn export_mask_group_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.flush_mask_to_doc(cx);
        let Some(gid) = self.mask_target.clone() else {
            self.mask_tool.update(cx, |m, cx| {
                m.set_status_text("没有可导出的组合", cx);
            });
            return;
        };
        let _ = self.doc.ensure_group_pages(&gid);
        let job = self.doc.prepare_group_render_job(&gid);
        self.doc.retain_memory_window();
        let Some(job) = job else {
            self.mask_tool.update(cx, |m, cx| {
                m.set_status_text("无法拼合该组合", cx);
            });
            return;
        };
        self.mask_tool.update(cx, |m, cx| {
            m.set_status_text("正在导出…", cx);
        });
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = job.render().and_then(|rgb| {
                let format = match path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .as_deref()
                {
                    Some("jpg") | Some("jpeg") => image::ImageFormat::Jpeg,
                    _ => image::ImageFormat::Png,
                };
                rgb.save_with_format(&path, format)
                    .map_err(|e| format!("保存失败: {e}"))?;
                Ok(path)
            });
            let _ = tx.send_blocking(result);
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else {
                return;
            };
            this.update(cx, |view, cx| {
                view.mask_tool.update(cx, |m, cx| {
                    match result {
                        Ok(p) => m.set_status_text(format!("已保存: {}", p.display()), cx),
                        Err(e) => m.set_status_text(e, cx),
                    }
                });
            })
            .ok();
        })
        .detach();
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
        }
        if self.side_tool == SideTool::Project {
            self.mask_tool.update(cx, |m, cx| {
                m.set_preview_only(false);
                m.set_host_pick_armed(false, cx);
            });
            self.bg.eyedropper_armed = false;
            self.bg.pick_open = false;
            self.bg.batch_open = false;
        }
        let was_mask_canvas = self.uses_mask_canvas();
        self.side_tool = tool;
        // 离开蒙版/底色时丢掉还在跑的预览线程, 避免和视频池终稿抢 CPU/内存.
        if was_mask_canvas && !self.uses_mask_canvas() {
            self.mask_sync_gen = self.mask_sync_gen.wrapping_add(1);
        }
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
                self.mask_tool.update(cx, |m, _| m.set_preview_only(false));
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
                self.mask_tool.update(cx, |m, _| m.set_preview_only(true));
                self.mask_target = None;
                self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
                self.sync_mask_image(cx);
                self.scroll_mask_lists_to_active();
                self.focus_handle.focus(window);
                self.status = "底色".into();
                self.hint =
                    "左侧预览组合 (滚轮切换). 右侧选择底色图或纯色, 再应用/取消.".into();
            }
            SideTool::Video => {
                self.score_video
                    .read(cx)
                    .focus_handle_ref()
                    .clone()
                    .focus(window);
                self.status = "视频工具".into();
                self.hint =
                    "N 插入下一张组合 | 空格播放/暂停 | ←→ 快退快进 | I/O 标记淡入淡出."
                        .into();
                self.sync_video_pool(cx);
            }
        }
        cx.notify();
    }

    /// 把「输出组合」渲染为终稿写入工程旁持久缓存, 再同步给视频素材池 (LRU 热加载).
    ///
    /// 先用磁盘缓存的 PNG 头立刻填池 (预览马上有画面), 再只对脏/缺失的组合
    /// 后台重渲, 每完成一个分片就 `upsert` 进池. 原先要等全部 chunk 结束才
    /// `set_pool`, 任一组合卡住或 `job.render` panic 就会一直黑屏、素材池 0 张.
    pub(super) fn sync_video_pool(&mut self, cx: &mut Context<Self>) {
        self.video_sync_gen = self.video_sync_gen.wrapping_add(1);
        let gen = self.video_sync_gen;
        let group_ids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let (aw, ah) = (self.doc.bg_aspect_w, self.doc.bg_aspect_h);
        let fade_bg = self.doc.bg_solid.unwrap_or_else(|| {
            sample_paper_rgb(self.doc.bg_image.as_deref())
        });
        self.score_video.update(cx, |v, _| {
            v.set_aspect(aw, ah);
            v.set_fade_bg_rgb(fade_bg);
        });
        if group_ids.is_empty() {
            self.score_video.update(cx, |v, cx| {
                v.set_pool(Vec::new(), cx);
                v.set_pool_status("", cx);
            });
            return;
        }
        let cache_root = self.pool_cache_dir().join("pool");
        let _ = std::fs::create_dir_all(&cache_root);
        crate::page_cache::prune_pool_cache(
            &cache_root,
            &group_ids.iter().cloned().collect(),
        );
        let all_dirty = self.video_pool_all_dirty;
        let dirty_set = self.video_pool_dirty.clone();

        let mut cached_items: Vec<MaterialItem> = Vec::new();
        let mut rebuild_ids: Vec<String> = Vec::new();
        for (idx, gid) in group_ids.iter().enumerate() {
            let cache_path = cache_root.join(format!("{gid}.png"));
            let need_rebuild = all_dirty || dirty_set.contains(gid) || !cache_path.is_file();
            if let Ok((w, h)) = image::image_dimensions(&cache_path) {
                let label = self
                    .doc
                    .groups
                    .get(idx)
                    .map(|g| g.display_name(idx))
                    .unwrap_or_else(|| gid.clone());
                cached_items.push(MaterialItem {
                    group_id: gid.clone(),
                    label: label.into(),
                    width: w,
                    height: h,
                    cache_path,
                });
            }
            if need_rebuild {
                rebuild_ids.push(gid.clone());
            }
        }
        let cached_n = cached_items.len();
        self.score_video.update(cx, |v, cx| {
            v.set_pool(cached_items, cx);
        });

        if rebuild_ids.is_empty() {
            self.video_pool_all_dirty = false;
            self.video_pool_dirty.clear();
            self.score_video
                .update(cx, |v, cx| v.set_pool_status("", cx));
            self.status = format!("视频工具 (素材 {cached_n} 张)").into();
            return;
        }

        let rebuild_n = rebuild_ids.len();
        self.score_video.update(cx, |v, cx| {
            v.set_pool_status(format!("正在更新 {rebuild_n} 张…"), cx);
        });
        self.status = if cached_n > 0 {
            format!("视频工具 (已显示缓存 {cached_n} 张, 后台更新 {rebuild_n} 张)").into()
        } else {
            format!("视频工具 (正在生成素材 0/{rebuild_n})").into()
        };

        let peak = self
            .doc
            .pages
            .first()
            .map(|p| p.estimated_bytes().saturating_mul(3))
            .unwrap_or(64 * 1024 * 1024);
        let conc = crate::page_cache::concurrency_for_peak(peak.max(128 * 1024 * 1024));

        cx.spawn(async move |this, cx| {
            let mut done = 0usize;
            let total = rebuild_ids.len();
            for (chunk_i, chunk) in rebuild_ids.chunks(conc.max(1)).enumerate() {
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
                let gathered = this.update(cx, |view, _| {
                    if view.video_sync_gen != gen {
                        return None;
                    }
                    let mut out = Vec::with_capacity(chunk.len());
                    for gid in chunk {
                        let Some(idx) = view.doc.groups.iter().position(|g| &g.id == gid) else {
                            continue;
                        };
                        let label = view.doc.groups[idx].display_name(idx);
                        let cache_path = cache_root.join(format!("{gid}.png"));
                        let _ = view.doc.ensure_group_pages(gid);
                        let job = view.doc.prepare_group_render_job(gid);
                        view.doc.retain_memory_window();
                        out.push(VideoPoolRebuildEntry {
                            gid: gid.clone(),
                            label,
                            cache_path,
                            job,
                        });
                    }
                    Some(out)
                });
                let Ok(Some(gathered)) = gathered else {
                    crate::trace::log(&format!(
                        "video_pool: chunk {} 结束 cancelled=true",
                        chunk_i + 1
                    ));
                    return;
                };
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let mut chunk_items = Vec::with_capacity(gathered.len());
                    for entry in gathered {
                        let VideoPoolRebuildEntry {
                            gid,
                            label,
                            cache_path,
                            job,
                        } = entry;
                        let mut item = None;
                        if let Some(job) = job {
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                job.render()
                            })) {
                                Ok(Ok(rgb)) => {
                                    if rgb.save(&cache_path).is_ok() {
                                        item = Some(MaterialItem {
                                            group_id: gid.clone(),
                                            label: label.clone().into(),
                                            width: rgb.width(),
                                            height: rgb.height(),
                                            cache_path: cache_path.clone(),
                                        });
                                    } else {
                                        crate::trace::log(&format!(
                                            "video_pool: {gid} 写入缓存失败"
                                        ));
                                    }
                                }
                                Ok(Err(e)) => {
                                    crate::trace::log(&format!(
                                        "video_pool: {gid} render 失败: {e}"
                                    ));
                                }
                                Err(_) => {
                                    crate::trace::log(&format!(
                                        "video_pool: {gid} render panic, 跳过"
                                    ));
                                }
                            }
                        } else {
                            crate::trace::log(&format!(
                                "video_pool: {gid} 无成员片段, 跳过渲染"
                            ));
                        }
                        if item.is_none() {
                            if let Ok((w, h)) = image::image_dimensions(&cache_path) {
                                item = Some(MaterialItem {
                                    group_id: gid.clone(),
                                    label: label.into(),
                                    width: w,
                                    height: h,
                                    cache_path,
                                });
                            } else {
                                crate::trace::log(&format!(
                                    "video_pool: {gid} 无缓存且渲染失败, 跳过"
                                ));
                            }
                        }
                        if let Some(it) = item {
                            chunk_items.push(it);
                        }
                    }
                    let _ = tx.send_blocking(chunk_items);
                });
                let Ok(chunk_items) = rx.recv().await else {
                    crate::trace::log(&format!(
                        "video_pool: chunk {} 工作线程通道关闭",
                        chunk_i + 1
                    ));
                    return;
                };
                done += chunk.len();
                let still_current = this
                    .update(cx, |view, cx| {
                        if view.video_sync_gen != gen {
                            return false;
                        }
                        view.score_video.update(cx, |v, cx| {
                            v.upsert_pool_items(chunk_items, &group_ids, cx);
                            if done < total {
                                v.set_pool_status(format!("正在更新 {done}/{total}…"), cx);
                            }
                        });
                        let n = view.score_video.read(cx).pool_len();
                        view.status = format!("视频工具 (素材 {n} 张, 更新 {done}/{total})").into();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                crate::trace::log(&format!(
                    "video_pool: chunk {} 结束 cancelled={}",
                    chunk_i + 1,
                    !still_current
                ));
                if !still_current {
                    return;
                }
            }
            crate::trace::log("video_pool: 全部 chunk 完成, 写回素材池");
            this.update(cx, |view, cx| {
                if view.video_sync_gen == gen {
                    view.video_pool_all_dirty = false;
                    view.video_pool_dirty.clear();
                    let n = view.score_video.read(cx).pool_len();
                    view.score_video
                        .update(cx, |v, cx| v.set_pool_status("", cx));
                    view.status = format!("视频工具 (素材 {n} 张)").into();
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
            self.retire_current_render_image();
            self.img_w = 0;
            self.img_h = 0;
        }
        if self.uses_mask_canvas() {
            if self.side_tool == SideTool::Mask {
                self.flush_mask_to_doc(cx);
            }
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

/// 诊断: 切蒙版/底色后画面要等一会. 对真实 Chopin 工程按
/// `build_mask_preview` 逐步计时 (发布模式). 跑法:
/// `cargo test -r -p score_sync --bin score_sync probe_chopin_mask_preview_wait -- --ignored --nocapture`
#[cfg(test)]
mod mask_preview_wait_probe {
    use super::*;
    use image::{Frame, ImageBuffer, RgbaImage};
    use smallvec::smallvec;
    use std::hint::black_box;
    use std::io::Read;
    use std::path::PathBuf;
    use std::time::Instant;

    const GPU_TEX_MAX_SIDE: u32 = 2048;

    fn chopin_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("score_sync crate parent")
            .join("Chopin - 4 Scherzos, Henle.pdf_p001.staffcrop")
    }

    fn zip_bytes(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Vec<u8> {
        let alt = name.replace('/', "\\");
        let idx = archive.index_for_name(name).or_else(|| archive.index_for_name(&alt));
        let idx = idx.unwrap_or_else(|| panic!("zip 里没有 {name}"));
        let mut zf = archive.by_index(idx).expect("zip by_index");
        let mut buf = Vec::new();
        zf.read_to_end(&mut buf).expect("read zip entry");
        buf
    }

    fn decode_rgb(bytes: &[u8]) -> image::RgbImage {
        image::load_from_memory(bytes)
            .expect("decode png")
            .to_rgb8()
    }

    fn ms(t0: Instant) -> f64 {
        t0.elapsed().as_secs_f64() * 1000.0
    }

    fn step(name: &str, t0: Instant) {
        eprintln!("  {name:<44} {:>8.1} ms", ms(t0));
    }

    fn scaled_dims(w: u32, h: u32) -> (u32, u32) {
        let m = w.max(h);
        if m <= GPU_TEX_MAX_SIDE {
            return (w, h);
        }
        let tw = ((w as u64).saturating_mul(GPU_TEX_MAX_SIDE as u64) / m as u64).max(1) as u32;
        let th = ((h as u64).saturating_mul(GPU_TEX_MAX_SIDE as u64) / m as u64).max(1) as u32;
        (tw, th)
    }

    fn triangle_downscale(rgb: &image::RgbImage) -> image::RgbImage {
        let (w, h) = rgb.dimensions();
        let (tw, th) = scaled_dims(w, h);
        if (tw, th) == (w, h) {
            return rgb.clone();
        }
        image::imageops::resize(rgb, tw, th, image::imageops::FilterType::Triangle)
    }

    fn nearest_downscale(rgb: &image::RgbImage) -> image::RgbImage {
        let (w, h) = rgb.dimensions();
        let (tw, th) = scaled_dims(w, h);
        if (tw, th) == (w, h) {
            return rgb.clone();
        }
        image::imageops::resize(rgb, tw, th, image::imageops::FilterType::Nearest)
    }

    fn rgb_to_bgra_buf(rgb: &image::RgbImage) -> RgbaImage {
        let (w, h) = rgb.dimensions();
        let src = rgb.as_raw();
        let n = src.len() / 3 * 4;
        let mut buf: Vec<u8> = Vec::with_capacity(n);
        #[allow(clippy::uninit_vec)]
        unsafe {
            buf.set_len(n);
        }
        for (dst, s) in buf.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
            dst[0] = s[2];
            dst[1] = s[1];
            dst[2] = s[0];
            dst[3] = 255;
        }
        ImageBuffer::from_raw(w, h, buf).expect("rgba size")
    }

    #[test]
    #[ignore]
    fn probe_chopin_mask_preview_wait() {
        let path = chopin_path();
        assert!(
            path.is_file(),
            "找不到样例工程: {} (把 Chopin .staffcrop 放在 crop_sheet/ 下)",
            path.display()
        );

        let t_open = Instant::now();
        let file = std::fs::File::open(&path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let project_bytes = zip_bytes(&mut zip, "project.json");
        let doc: serde_json::Value = serde_json::from_slice(&project_bytes).unwrap();
        let gid = doc["active_group_id"].as_str().unwrap();
        let group = doc["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"].as_str() == Some(gid))
            .expect("active group");
        let rid = group["region_ids"][0].as_str().unwrap();
        let mut page_image = None;
        let mut y0 = 0u32;
        let mut y1 = 0u32;
        for page in doc["pages"].as_array().unwrap() {
            for r in page["regions"].as_array().unwrap() {
                if r["id"].as_str() == Some(rid) {
                    page_image = page["image"].as_str().map(|s| s.to_string());
                    y0 = r["y0"].as_i64().unwrap().max(0) as u32;
                    y1 = r["y1"].as_i64().unwrap().max(0) as u32;
                }
            }
        }
        let page_entry = page_image.expect("region page");
        let ink = doc["ink_threshold"].as_i64().unwrap_or(200) as i32;
        let bg_on = doc["bg"]["enabled"].as_bool().unwrap_or(false);
        let aspect_w = doc["bg"]["aspect_w"].as_u64().unwrap_or(2560) as u32;
        let aspect_h = doc["bg"]["aspect_h"].as_u64().unwrap_or(1440) as u32;
        let voff_shift = doc["group_voff_shift"][gid].as_i64().unwrap_or(0);
        let height = y1.saturating_sub(y0) + 1;

        let page_png = zip_bytes(&mut zip, &page_entry.replace('\\', "/"));
        let bg_png = zip_bytes(&mut zip, "bg.png");
        drop(zip);
        eprintln!(
            "[probe] 打开 zip + 读 json/png 字节: {:.1} ms",
            ms(t_open)
        );
        eprintln!(
            "[probe] 组合 {gid} 成员 {rid}  {page_entry}  y0={y0} h={height}  bg={bg_on}  {aspect_w}x{aspect_h}  voff_shift={voff_shift}"
        );

        let t0 = Instant::now();
        let page = decode_rgb(&page_png);
        step("PNG 解码页图", t0);
        let (pw, ph) = page.dimensions();
        eprintln!("         页图 {pw}x{ph}  ({:.1} MB RGB)", (pw as u64 * ph as u64 * 3) as f64 / 1e6);

        let t0 = Instant::now();
        let bg = decode_rgb(&bg_png);
        step("PNG 解码底色", t0);
        let (bw, bh) = bg.dimensions();
        eprintln!("         底色 {bw}x{bh}  ({:.1} MB RGB)", (bw as u64 * bh as u64 * 3) as f64 / 1e6);

        let t0 = Instant::now();
        let piece = crate::model::crop_band_fast(&page, y0, height);
        step("crop_band_fast 条带", t0);
        let (sw, sh) = piece.dimensions();
        eprintln!("         条带 {sw}x{sh}");

        let t0 = Instant::now();
        let stats = mask_tool::layout::compute_piece_stats(&piece, ink);
        black_box(&stats);
        step("compute_piece_stats", t0);

        let t0 = Instant::now();
        let mut pieces = vec![(rid.to_string(), piece)];
        let sheet = crate::model::compose_parts_impl(&pieces, &[], ink, None)
            .expect("compose");
        step("compose_parts_impl (单块 clone)", t0);

        let frame = apply_bg::process::preview_frame(
            sheet.width(),
            sheet.height(),
            bw,
            bh,
            aspect_w,
            aspect_h,
            voff_shift,
        );
        eprintln!(
            "         preview_frame canvas={}x{} scale={:.4} shows_bg={}",
            frame.canvas_w, frame.canvas_h, frame.content_scale, frame.shows_bg
        );

        let t0 = Instant::now();
        let (canvas, hoff, voff) = apply_bg::process::composite_preview(
            &sheet,
            &bg,
            aspect_w,
            aspect_h,
            voff_shift,
            0,
            0,
        )
        .expect("composite");
        step("composite_preview (含 content_scale 缩放)", t0);
        eprintln!(
            "         画布 {}x{}  hoff={hoff} voff={voff}",
            canvas.width(),
            canvas.height()
        );

        let t0 = Instant::now();
        let piece_ys = mask_tool::staff::piece_staff_ys_from_parts(&pieces, ink);
        black_box(&piece_ys);
        step("piece_staff_ys_from_parts (谱表扫描)", t0);

        let t0 = Instant::now();
        let tile = mask_tool::gui::BlockTile::from_piece(rid.to_string(), &pieces[0].1, stats);
        black_box(&tile);
        step("BlockTile::from_piece (条带贴图)", t0);
        let (tw, th) = scaled_dims(sw, sh);
        eprintln!("         Triangle {sw}x{sh} -> {tw}x{th}");

        let t0 = Instant::now();
        let crop = apply_bg::process::crop_bg_to_page(&bg, aspect_w, aspect_h, sw)
            .expect("bg covers page");
        step("crop_bg_to_page", t0);
        eprintln!("         裁切 {}x{}", crop.width(), crop.height());

        let t0 = Instant::now();
        let bg_tile = mask_tool::gui::BlockBgTile::from_full(&bg, aspect_w, aspect_h, sw)
            .expect("from_full");
        black_box(&bg_tile);
        step("BlockBgTile::from_full (目标页裁切贴图)", t0);
        assert_eq!(
            (bg_tile.width, bg_tile.height, bg_tile.src_width, bg_tile.src_height),
            (crop.width(), crop.height(), bw, bh)
        );
        let (btw, bth) = scaled_dims(bg_tile.width, bg_tile.height);
        eprintln!(
            "         Triangle {}x{} -> {btw}x{bth}",
            bg_tile.width, bg_tile.height
        );

        let t0 = Instant::now();
        let render = mask_tool::gui::rgb_to_render_image(&canvas);
        black_box(&render);
        step("rgb_to_render_image (合成预览贴图)", t0);
        let (ctw, cth) = scaled_dims(canvas.width(), canvas.height());
        eprintln!(
            "         Triangle {}x{} -> {ctw}x{cth}",
            canvas.width(),
            canvas.height()
        );

        eprintln!("--- 把 rgb_to_render_image 拆开 ---");
        let t0 = Instant::now();
        let bg_scaled = triangle_downscale(&bg);
        step("  Triangle 缩底色", t0);
        let t0 = Instant::now();
        let _ = nearest_downscale(&bg);
        step("  Nearest 缩底色 (对照)", t0);
        let t0 = Instant::now();
        let bgra = rgb_to_bgra_buf(&bg_scaled);
        step("  RGB→BGRA 底色缩略", t0);
        let t0 = Instant::now();
        let _ = Arc::new(gpui::RenderImage::new(smallvec![Frame::new(bgra)]));
        step("  RenderImage::new 底色", t0);

        let t0 = Instant::now();
        let _ = triangle_downscale(&pieces[0].1);
        step("  Triangle 缩条带", t0);
        let t0 = Instant::now();
        let _ = triangle_downscale(&canvas);
        step("  Triangle 缩合成画布", t0);

        eprintln!("--- 切面板时页已在内存 (不解码) 的整条 build_mask_preview ---");
        let members = vec![MaskPreviewMemberSnap {
            rid: rid.to_string(),
            page_idx: 0,
            y0,
            height,
            image: Some(Arc::new(page)),
            disk_path: PathBuf::from("unused"),
        }];
        let bg_arc = Arc::new(bg);
        let t0 = Instant::now();
        let built = build_mask_preview(
            members,
            ink,
            Vec::new(),
            bg_on,
            Some(bg_arc.clone()),
            None,
            bg_arc.width(),
            bg_arc.height(),
            aspect_w,
            aspect_h,
            voff_shift,
            true,
        )
        .expect("build_mask_preview");
        step("build_mask_preview 全流水线 (页已在内存, 三层缩略图)", t0);
        black_box(&built);
        eprintln!(
            "         画布 {}x{}  hoff={} voff={}",
            built.canvas_w, built.canvas_h, built.hoff, built.voff
        );

        eprintln!("--- 旧路径对照: 整图合成 + Triangle 贴图 (已不再走) ---");
        let page2 = decode_rgb(&page_png);
        let piece2 = crate::model::crop_band_fast(&page2, y0, height);
        pieces[0].1 = piece2;
        let sheet2 = crate::model::compose_parts_impl(&pieces, &[], ink, None).unwrap();
        let t0 = Instant::now();
        let (canvas2, _, _) = apply_bg::process::composite_preview(
            &sheet2,
            bg_arc.as_ref(),
            aspect_w,
            aspect_h,
            voff_shift,
            0,
            0,
        )
        .unwrap();
        let preview = mask_tool::gui::rgb_to_render_image(&canvas2);
        black_box(&preview);
        step("第一帧必需 (composite + 预览贴图)", t0);
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
