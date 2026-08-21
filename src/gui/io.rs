//! 打开 / 保存 / 导出 / 更新检查.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn start_update_check(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::bounded::<crate::update::UpdateInfo>(1);
        std::thread::spawn(move || {
            if let Some(info) = crate::update::check_latest() {
                let _ = tx.send_blocking(info);
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(info) = rx.recv().await {
                this.update(cx, |view, cx| {
                    view.pending_update = Some(info);
                    view.try_show_update_dialog(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn try_show_update_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            return;
        }
        let Some(info) = self.pending_update.take() else {
            return;
        };
        self.update_scroll.set_offset(point(px(0.), px(0.)));
        self.dialog = Some(DialogKind::UpdateAvailable {
            current: info.current,
            latest: info.latest,
            url: info.url,
            changes: info.changes,
        });
        cx.notify();
    }

    pub(super) fn dismiss_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        self.try_show_update_dialog(cx);
        cx.notify();
    }
    pub(super) fn load_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut images = Vec::new();
        let mut pdfs = Vec::new();
        let mut projects = Vec::new();
        for path in paths {
            if is_project_path(&path) {
                projects.push(path);
            } else if is_pdf_path(&path) {
                pdfs.push(path);
            } else if is_image_path(&path) {
                images.push(path);
            } else {
                self.show_error(
                    "不支持",
                    crate::error::Error::msg(format!("无法打开: {}", path.display())),
                    cx,
                );
            }
        }

        // 工程文件优先单独打开 (取最后一个)
        if let Some(proj) = projects.pop() {
            self.open_project_path(proj, cx);
            if projects.is_empty() && images.is_empty() && pdfs.is_empty() {
                return;
            }
        }

        let mut added = 0usize;
        for path in images {
            match image::open(&path) {
                Ok(im) => {
                    let rgb = im.to_rgb8();
                    match self.doc.add_page(path.clone(), rgb, true) {
                        Ok(_) => {
                            added += 1;
                            self.mark_dirty();
                            self.mark_video_pool_dirty_all();
                        }
                        Err(e) => {
                            self.show_error(
                                "打开失败",
                                crate::error::Error::msg(format!("{}: {e}", path.display())),
                                cx,
                            );
                        }
                    }
                }
                Err(e) => {
                    self.show_error(
                        "打开失败",
                        crate::error::Error::image_open(&path, e),
                        cx,
                    );
                }
            }
        }
        if added > 0 {
            self.refresh_render(cx);
            self.status = format!("已添加 {added} 页, 共 {} 页.", self.doc.pages.len()).into();
            self.hint = self.status.clone();
        }

        if !pdfs.is_empty() {
            self.start_pdf_load(pdfs, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn start_pdf_load(&mut self, pdfs: Vec<PathBuf>, cx: &mut Context<Self>) {
        crate::trace::log(&format!(
            "ui: 开始导入 {} 个 PDF: {}",
            pdfs.len(),
            pdfs.iter()
                .filter_map(|p| p.file_name()?.to_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let gen = self.pdf_load_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let token = Arc::clone(&self.pdf_load_gen);
        self.pdf_importing = true;
        self.status = format!(
            "PDF 后台渲染中… ({})",
            pdfs.first()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("pdf")
        )
        .into();
        self.hint = self.status.clone();
        cx.notify();

        let (tx, rx) = async_channel::unbounded::<PdfLoadMsg>();
        let ink = self.doc.ink_threshold;
        let margin = self.doc.margin;
        std::thread::spawn(move || {
            for pdf in pdfs {
                if token.load(Ordering::SeqCst) != gen {
                    break;
                }
                let name = pdf
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("pdf")
                    .to_string();
                let token_loop = Arc::clone(&token);
                let result = pdf::pdf_pages_to_tmp_images_streaming(
                    &pdf,
                    ink,
                    margin,
                    move || token_loop.load(Ordering::SeqCst) == gen,
                    |i, total, path| {
                        let _ = tx.send_blocking(PdfLoadMsg::Page {
                            path,
                            index: i,
                            total,
                            pdf_name: name.clone(),
                        });
                    },
                );
                if token.load(Ordering::SeqCst) != gen {
                    break;
                }
                match result {
                    Ok(n) => {
                        let _ = tx.send_blocking(PdfLoadMsg::Done {
                            pdf_name: name,
                            pages: n,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send_blocking(PdfLoadMsg::Err {
                            pdf_name: name,
                            message: e.to_string(),
                        });
                    }
                }
            }
            if token.load(Ordering::SeqCst) == gen {
                let _ = tx.send_blocking(PdfLoadMsg::AllFinished);
            }
        });

        cx.spawn(async move |this, cx| {
            let mut pages_since_yield = 0u32;
            while let Ok(msg) = rx.recv().await {
                let stop = matches!(msg, PdfLoadMsg::AllFinished);
                let is_page = matches!(msg, PdfLoadMsg::Page { .. });
                this.update(cx, |view, cx| {
                    if view.pdf_load_gen.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    match msg {
                        PdfLoadMsg::Page {
                            path,
                            index,
                            total,
                            pdf_name,
                        } => {
                            let was_empty = view.doc.pages.is_empty();
                            let display = PathBuf::from(format!(
                                "{pdf_name}_p{:03}.png",
                                index + 1
                            ));
                            // 导入只登记磁盘 PNG, 不在 UI 线程逐页识别 (几百页会卡死)
                            crate::trace::log(&format!(
                                "ui: 登记 PDF 页 {}/{total} (run_detect=false)",
                                index + 1
                            ));
                            match view.doc.add_page_from_disk(
                                display,
                                path.clone(),
                                was_empty,
                                false,
                            ) {
                                Ok(_) => {
                                    view.mark_dirty();
                                    let done = index + 1;
                                    let refresh = was_empty || done == total || done % 8 == 0;
                                    if refresh {
                                        if was_empty {
                                            view.refresh_render(cx);
                                        }
                                        view.status = format!(
                                            "PDF {pdf_name}: 已载入 {done}/{total} 页 (共 {} 页)",
                                            view.doc.pages.len()
                                        )
                                        .into();
                                        view.hint = view.status.clone();
                                        cx.notify();
                                    }
                                }
                                Err(e) => {
                                    view.show_error(
                                        "打开失败",
                                        crate::error::Error::image_open(&path, e),
                                        cx,
                                    );
                                }
                            }
                        }
                        PdfLoadMsg::Done { pdf_name, pages } => {
                            view.status =
                                format!("PDF {pdf_name} 完成: {pages} 页已载入.").into();
                            view.hint = view.status.clone();
                            cx.notify();
                        }
                        PdfLoadMsg::Err { pdf_name, message } => {
                            view.show_error(
                                "PDF 转换失败",
                                crate::error::Error::PdfOpen(format!("{pdf_name}\n{message}")),
                                cx,
                            );
                        }
                        PdfLoadMsg::AllFinished => {
                            crate::trace::log("ui: PDF 全部登记完成 (识别已写入 sidecar)");
                            view.pdf_importing = false;
                            view.refresh_render(cx);
                            view.start_hydrate_all(true, cx);
                        }
                    }
                })
                .ok();
                if stop {
                    break;
                }
                if is_page {
                    pages_since_yield += 1;
                    if pages_since_yield % 8 == 0 {
                        cx.background_executor()
                            .timer(Duration::from_millis(8))
                            .await;
                    }
                }
            }
        })
        .detach();
    }

    pub(super) fn spawn_native_dialog<T, F, A>(cx: &mut Context<Self>, work: F, apply: A)
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        A: FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    {
        let (tx, rx) = async_channel::bounded::<T>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(work());
        });
        cx.spawn(async move |this, cx| {
            if let Ok(val) = rx.recv().await {
                this.update(cx, |view, cx| apply(view, val, cx)).ok();
            }
        })
        .detach();
    }

    pub(super) fn open_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        Self::spawn_native_dialog(
            cx,
            || {
                rfd::FileDialog::new()
                    .set_title("打开图片 / PDF (可多选)")
                    .add_filter(
                        "Images / PDF",
                        &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "pdf"],
                    )
                    .add_filter("PDF", &["pdf"])
                    .add_filter("Images", &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"])
                    .pick_files()
            },
            |this, files, cx| {
                if let Some(paths) = files {
                    this.load_paths(paths, cx);
                }
            },
        );
    }

    pub(super) fn open_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        Self::spawn_native_dialog(
            cx,
            || {
                rfd::FileDialog::new()
                    .set_title("打开工程")
                    .add_filter("Score Sync 工程", &["staffcrop"])
                    .pick_file()
            },
            |this, file, cx| {
                if let Some(path) = file {
                    this.open_project_path(path, cx);
                }
            },
        );
    }

    pub(super) fn open_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.opening || self.saving {
            self.status = "工程读写进行中, 请稍候…".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.flush_mask_to_doc(cx);
        self.opening = true;
        self.abandon_pdf_import();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        self.status = format!("正在打开工程: {name}…").into();
        self.hint = self.status.clone();
        cx.notify();

        let path_bg = path.clone();
        let (tx, rx) = async_channel::bounded::<Result<DocState, String>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(project::load_project(&path_bg));
        });

        cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            this.update(cx, |view, cx| {
                view.opening = false;
                match result {
                    Ok(Ok(doc)) => {
                        let video_snap = doc.video_state.clone();
                        view.doc = doc;
                        view.project_path = Some(path.clone());
                        view.dirty = false;
                        view.video_pool_all_dirty = false;
                        view.video_pool_dirty.clear();
                        config::remember_last_project(&path);
                        view.drag = None;
                        view.dialog = None;
                        view.tab_menu = None;
                        view.param_edit = None;
                        view.region_y_edit = None;
                        view.crop_histories.clear();
                        view.page_struct_history = CropHistory::default();
                        view.side_tool = SideTool::Crop;
                        view.canvas_tool = CanvasTool::Normal;
                        view.mask_target = None;
                        view.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
                        view.score_video
                            .update(cx, |v, cx| v.load_timeline_snapshot(video_snap, cx));
                        view.user_zoomed = false;
                        view.zoom = 1.0;
                        view.pan = point(0.0, 0.0);
                        let mask_prefs = view.doc.mask_prefs.clone();
                        view.mask_tool.update(cx, |m, _| {
                            m.apply_color_prefs(mask_prefs);
                        });
                        view.refresh_render(cx);
                        view.start_hydrate_all(false, cx);
                        view.status = format!(
                            "已打开工程: {} ({} 页, {} 组)",
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("project"),
                            view.doc.pages.len(),
                            view.doc.groups.len()
                        )
                        .into();
                        view.hint = view.status.clone();
                        view.try_show_update_dialog(cx);
                    }
                    Ok(Err(e)) => {
                        view.show_error("打开工程失败", crate::error::Error::project(e), cx);
                    }
                    Err(_) => {
                        view.show_error(
                            "打开工程失败",
                            crate::error::Error::msg("后台打开通道已关闭."),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 新建空白工程: 有未保存改动时先确认.
    pub(super) fn request_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening || self.saving {
            self.status = "工程读写进行中, 请稍候…".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.refresh_dirty_from_panels(cx);
        if self.dirty {
            self.dialog = Some(DialogKind::UnsavedNew);
            cx.notify();
            return;
        }
        let _ = window;
        self.do_new_project(cx);
    }

    /// 丢掉进行中的 PDF 导入与全量 hydrate, 避免页回调写进新工程.
    pub(super) fn abandon_pdf_import(&mut self) {
        let _ = self.pdf_load_gen.fetch_add(1, Ordering::SeqCst);
        self.pdf_importing = false;
        self.hydrate_gen = self.hydrate_gen.wrapping_add(1);
        self.page_load_gen = self.page_load_gen.wrapping_add(1);
    }

    /// 清空当前文档/视频/蒙版状态, 回到可重新导入的空白工程.
    pub(super) fn do_new_project(&mut self, cx: &mut Context<Self>) {
        self.abandon_pdf_import();
        let mask_prefs = self.doc.mask_prefs.clone();
        self.flush_mask_to_doc(cx);
        self.doc = DocState::new();
        self.doc.mask_prefs = mask_prefs.clone();
        self.project_path = None;
        self.dirty = false;
        self.video_pool_all_dirty = true;
        self.video_pool_dirty.clear();
        self.drag = None;
        self.dialog = None;
        self.tab_menu = None;
        self.param_edit = None;
        self.region_y_edit = None;
        self.crop_histories.clear();
        self.page_struct_history = CropHistory::default();
        self.side_tool = SideTool::Crop;
        self.canvas_tool = CanvasTool::Normal;
        self.mask_target = None;
        self.mask_tool.update(cx, |m, cx| {
            m.clear_view("", cx);
            m.apply_color_prefs(mask_prefs);
        });
        self.score_video.update(cx, |v, cx| {
            v.load_timeline_snapshot(score_video::model::TimelineSnapshot::default(), cx);
            v.set_pool(Vec::new(), cx);
        });
        self.render_image = None;
        self.img_w = 0;
        self.img_h = 0;
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.status = format!(
            "已新建空白工程. 可用 {}O 导入图片/PDF.",
            apply_bg::primary_mod()
        )
        .into();
        self.hint = self.status.clone();
        self.try_show_update_dialog(cx);
        cx.notify();
    }

    pub(super) fn save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.project_path.clone() {
            self.save_project_to(path, cx);
        } else {
            self.save_project_as(window, cx);
        }
    }

    pub(super) fn save_project_as(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.pages.is_empty() {
            self.show_error(
                "提示",
                crate::error::Error::msg("当前没有可保存的页面."),
                cx,
            );
            return;
        }
        let start_dir = self
            .project_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let start_name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .or_else(|| {
                self.doc
                    .pages
                    .first()
                    .and_then(|page| page.path.file_stem().and_then(|s| s.to_str()))
                    .map(|stem| format!("{stem}.staffcrop"))
            });
        Self::spawn_native_dialog(
            cx,
            move || {
                let mut dlg = rfd::FileDialog::new()
                    .set_title("保存工程")
                    .add_filter("Score Sync 工程", &["staffcrop"]);
                if let Some(dir) = start_dir {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(name) = start_name {
                    dlg = dlg.set_file_name(name);
                }
                dlg.save_file()
            },
            |this, path, cx| {
                if let Some(path) = path {
                    this.save_project_to(path, cx);
                }
            },
        );
    }

    pub(super) fn save_project_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.saving || self.opening {
            self.status = "工程读写进行中, 请稍候…".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        if self.pdf_importing {
            self.show_error(
                "提示",
                crate::error::Error::msg(
                    "PDF 仍在载入, 请等全部页到齐后再保存; 若要放弃这份 PDF, 请先新建工程.",
                ),
                cx,
            );
            return;
        }
        if self.doc.pages.is_empty() {
            self.show_error(
                "提示",
                crate::error::Error::msg("当前没有可保存的页面."),
                cx,
            );
            return;
        }
        self.flush_mask_to_doc(cx);
        self.doc.video_state = self.score_video.read(cx).timeline_snapshot();
        // 把尚未灌入内存的 sidecar 写进 regions, 避免工程包只带上窗口内几页的分块
        let hydrated = self.doc.hydrate_detect_sidecars();
        if hydrated > 0 {
            self.doc.ensure_all_page_groups();
        }
        self.saving = true;
        self.save_spin_phase = 0.0;
        self.status = "正在保存工程…".into();
        self.hint = self.status.clone();
        cx.notify();
        self.start_save_spinner(cx);

        // 快照后放到后台流式打 zip; clone_for_save 不拷页图像素, 避免整首页一次进内存
        let doc = self.doc.clone_for_save();
        let (tx, rx) = async_channel::bounded::<Result<PathBuf, String>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(project::save_project(&doc, &path));
        });

        let quit_after = matches!(self.dialog, Some(DialogKind::UnsavedExit));
        let new_after = matches!(self.dialog, Some(DialogKind::UnsavedNew));
        cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(Ok(saved)) => {
                        view.project_path = Some(saved.clone());
                        view.dirty = false;
                        // 保存成功后对齐视频快照基准, 避免关窗误判仍脏
                        view.doc.video_state =
                            view.score_video.read(cx).timeline_snapshot();
                        config::remember_last_project(&saved);
                        view.status = format!("工程已保存: {}", saved.display()).into();
                        view.hint = view.status.clone();
                        if quit_after {
                            view.dialog = None;
                            view.allow_close = true;
                            cx.quit();
                        } else if new_after {
                            view.do_new_project(cx);
                        }
                    }
                    Ok(Err(e)) => {
                        view.show_error("保存工程失败", crate::error::Error::project(e), cx);
                    }
                    Err(_) => {
                        view.show_error(
                            "保存工程失败",
                            crate::error::Error::msg("后台保存通道已关闭."),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn export_groups_ui(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.flush_mask_to_doc(cx);
        }
        if self.doc.groups.is_empty() {
            self.show_error(
                "提示",
                crate::error::Error::export("没有可导出的内容."),
                cx,
            );
            return;
        }
        Self::spawn_native_dialog(
            cx,
            || {
                rfd::FileDialog::new()
                    .set_title("选择导出目录")
                    .pick_folder()
            },
            |this, out, cx| {
                let Some(out) = out else {
                    return;
                };
                this.start_export_groups(out, cx);
            },
        );
    }

    pub(super) fn start_export_groups(&mut self, out: PathBuf, cx: &mut Context<Self>) {
        let group_ids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let n = group_ids.len();
        let peak = self
            .doc
            .pages
            .iter()
            .map(|p| p.estimated_bytes())
            .max()
            .unwrap_or(64 * 1024 * 1024)
            .saturating_mul(2);
        let conc = crate::page_cache::concurrency_for_peak(peak);
        self.status = format!("正在导出 {n} 个组合…").into();
        self.hint = self.status.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let mut saved = 0usize;
            let mut err: Option<crate::error::Error> = None;
            let mut abs = 0usize;
            for (chunk_i, chunk) in group_ids.chunks(conc.max(1)).enumerate() {
                if chunk_i > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                let base = abs;
                abs += chunk.len();
                let batch = this
                    .update(cx, |view, _| {
                        match crate::export::export_groups_chunk(
                            &mut view.doc,
                            &out,
                            chunk,
                            base,
                        ) {
                            Ok(n) => {
                                saved += n;
                                view.doc.retain_window(
                                    view.doc.current_page_index,
                                    crate::page_cache::WINDOW_RADIUS,
                                );
                                view.status =
                                    format!("导出进度 {saved}/{}…", group_ids.len()).into();
                                view.hint = view.status.clone();
                                None
                            }
                            Err(e) => Some(e),
                        }
                    })
                    .unwrap_or(Some(crate::error::Error::export("导出任务中断")));
                if let Some(e) = batch {
                    err = Some(e);
                    break;
                }
            }
            this.update(cx, |view, cx| {
                view.doc.retain_window(
                    view.doc.current_page_index,
                    crate::page_cache::WINDOW_RADIUS,
                );
                match err {
                    Some(e) => {
                        view.show_error("导出失败", e, cx);
                    }
                    None => {
                        view.dialog = Some(DialogKind::Info {
                            title: "完成".into(),
                            body: format!(
                                "已导出 {saved} 个组合到:\n{}\n(已按输出组合列表顺序拼接并套用各组蒙版)",
                                out.display()
                            ),
                        });
                        view.status = format!("已导出 {saved} 个组合.").into();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
