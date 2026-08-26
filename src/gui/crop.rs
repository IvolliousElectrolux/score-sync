//! 识别、加块、合并、删除、参数编辑.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn fit_to_view(&mut self, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.mask_tool.update(cx, |m, cx| m.fit_to_view(cx));
            return;
        }
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        cx.notify();
    }

    pub(super) fn run_detect(&mut self, cx: &mut Context<Self>) {
        if self.doc.current_page().is_none() {
            self.show_error(
                "提示",
                crate::error::Error::msg("请先打开图片."),
                cx,
            );
            return;
        }
        if !self.current_page_pixels_ready() {
            self.pending_redetect = true;
            self.status = "页图加载中, 到齐后重新识别…".into();
            self.hint = self.status.clone();
            self.request_page_window(cx);
            cx.notify();
            return;
        }
        self.pending_redetect = false;
        self.push_crop_undo_current();
        let idx = self.doc.current_page_index;
        self.doc.detect_page(idx, true);
        let n = self.doc.pages[idx].regions.len();
        let systems = self.doc.pages[idx]
            .regions
            .values()
            .filter(|r| r.kind == "system")
            .count();
        self.status = format!("本页识别到 {n} 块 (system={systems}).").into();
        self.hint = self.status.clone();
        self.after_doc_change(cx);
    }

    pub(super) fn current_page_pixels_ready(&self) -> bool {
        let idx = self.doc.current_page_index;
        self.doc
            .pages
            .get(idx)
            .and_then(|p| p.image.as_ref())
            .is_some()
    }

    pub(super) fn flush_pending_redetect(&mut self, cx: &mut Context<Self>) {
        if !self.pending_redetect {
            return;
        }
        if !self.current_page_pixels_ready() {
            return;
        }
        self.run_detect(cx);
    }

    pub(super) fn run_detect_all(&mut self, cx: &mut Context<Self>) {
        if self.doc.pages.is_empty() {
            self.show_error(
                "提示",
                crate::error::Error::msg("请先打开图片."),
                cx,
            );
            return;
        }
        self.push_crop_undo_all_pages();
        let n = self.doc.pages.len();
        let jobs: Vec<(usize, PathBuf)> = self
            .doc
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.disk_path.clone()))
            .collect();
        let ink = self.doc.ink_threshold;
        let margin = self.doc.margin;
        self.status = format!("正在后台重新识别全部 {n} 页…").into();
        self.hint = self.status.clone();
        cx.notify();

        let (tx, rx) = async_channel::unbounded::<(usize, crate::detect_cache::PageDetectFile)>();
        std::thread::spawn(move || {
            for (idx, path) in jobs {
                match crate::page_cache::load_rgb(&path) {
                    Ok(img) => {
                        let file =
                            crate::detect_cache::detect_and_save(
                                &img, &path, ink, margin,
                            );
                        let _ = tx.send_blocking((idx, file));
                    }
                    Err(e) => {
                        crate::trace::log(&format!("detect_all 读页 {} 失败: {e}", idx + 1));
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
                    view.doc.replace_page_detect(idx, &file);
                    if d == n || d % 8 == 0 {
                        view.status = format!("识别进度 {d}/{n}…").into();
                        view.hint = view.status.clone();
                        crate::trace::log(&format!("ui: detect_all 进度 {d}/{n}"));
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
                view.doc.retain_memory_window();
                view.status = format!("已识别全部 {n} 页.").into();
                view.hint = view.status.clone();
                view.after_doc_change(cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn toggle_add_block(&mut self, cx: &mut Context<Self>) {
        self.canvas_tool = if self.canvas_tool == CanvasTool::AddBlock {
            CanvasTool::Normal
        } else {
            CanvasTool::AddBlock
        };
        self.drag = None;
        self.status = if self.canvas_tool == CanvasTool::AddBlock {
            "添加新块: 按下定一边, 先上移→该边为下边线, 先下移→为上边线, 拖出后松开".into()
        } else {
            "已退出添加新块".into()
        };
        self.hint = self.status.clone();
        cx.notify();
    }

    pub(super) fn toggle_split_block(&mut self, cx: &mut Context<Self>) {
        self.canvas_tool = if self.canvas_tool == CanvasTool::SplitBlock {
            CanvasTool::Normal
        } else {
            CanvasTool::SplitBlock
        };
        self.drag = None;
        self.status = if self.canvas_tool == CanvasTool::SplitBlock {
            "分割块: 在已有块内点击, 于指针位置切成上下两块".into()
        } else {
            "已退出分割块".into()
        };
        self.hint = self.status.clone();
        cx.notify();
    }

    pub(super) fn add_block_preview_ys(anchor_y: i32, role: Option<AddAnchorRole>, cur_y: i32) -> (i32, i32) {
        match role {
            None => (anchor_y, anchor_y),
            Some(AddAnchorRole::Top) => (anchor_y, cur_y.max(anchor_y)),
            Some(AddAnchorRole::Bottom) => (cur_y.min(anchor_y), anchor_y),
        }
    }

    pub(super) fn merge_selected(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.merge_selected() {
            Ok(n) => {
                self.status = format!("已合并 {n} 块为组合.").into();
                self.hint = self.status.clone();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.show_error("提示", crate::error::Error::msg(e), cx);
            }
        }
    }

    pub(super) fn pair_ungrouped(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.pair_ungrouped() {
            Ok(n) => {
                self.status = format!("已顺序配对合并 {n} 组.").into();
                self.hint = self.status.clone();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.show_error("提示", crate::error::Error::msg(e), cx);
            }
        }
    }

    pub(super) fn share_into_group(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.share_selected_into_active() {
            Ok(0) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.status = "选中块已在当前组中.".into();
                cx.notify();
            }
            Ok(n) => {
                self.status =
                    format!("已共享加入 {n} 块到当前组 (仍保留在其他组中).").into();
                self.hint = self.status.clone();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.show_error("提示", crate::error::Error::msg(e), cx);
            }
        }
    }

    pub(super) fn ungroup_active(&mut self, cx: &mut Context<Self>) {
        self.push_crop_undo_all_pages();
        match self.doc.ungroup_active() {
            Ok(()) => {
                self.status = "已拆开组合.".into();
                self.after_doc_change(cx);
            }
            Err(e) => {
                if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                    if let Some(h) = self.crop_histories.get_mut(&cur) {
                        h.undo.pop();
                    }
                }
                self.show_error("提示", crate::error::Error::msg(e), cx);
            }
        }
    }

    pub(super) fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if self.side_tool == SideTool::Mask {
            self.mask_tool.update(cx, |m, cx| m.delete_selected(cx));
            self.flush_mask_to_doc(cx);
            cx.notify();
            return;
        }
        self.push_crop_undo_all_pages();
        let n = self.doc.delete_selected();
        if n > 0 {
            self.status = format!("已删除 {n} 块.").into();
            self.after_doc_change(cx);
        } else if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
            if let Some(h) = self.crop_histories.get_mut(&cur) {
                h.undo.pop();
            }
        }
    }

    pub(super) fn reset_groups(&mut self, cx: &mut Context<Self>) {
        if self.doc.current_page().is_none() {
            return;
        }
        self.push_crop_undo_all_pages();
        self.doc.reset_current_page_groups();
        self.status = "已重置本页分组.".into();
        self.hint = self.status.clone();
        self.after_doc_change(cx);
    }
    pub(super) fn begin_edit_y(&mut self, rid: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.find_region(&rid).is_none() {
            return;
        }
        if self.param_edit.is_some() {
            self.apply_param_edit(window, cx);
        }
        if self.region_y_edit.as_ref() == Some(&rid) {
            return;
        }
        if self.region_y_edit.is_some() {
            self.apply_edit_y(window, cx);
        }
        let (y0, y1) = {
            let Some((_, r)) = self.doc.find_region(&rid) else {
                return;
            };
            (r.y0, r.y1)
        };
        let text = format!("{y0}-{y1}");
        self.edit_y_input.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.select_all_text(cx);
        });
        self.region_y_edit = Some(rid);
        self.edit_y_input.focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn begin_param_edit(
        &mut self,
        kind: ParamEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.region_y_edit.is_some() {
            self.apply_edit_y(window, cx);
        }
        // 切换编辑字段时先提交当前值
        if self.param_edit.is_some() && self.param_edit != Some(kind) {
            self.apply_param_edit(window, cx);
        }
        let text = match kind {
            ParamEdit::Margin => self.doc.margin.to_string(),
            ParamEdit::Threshold => self.doc.ink_threshold.to_string(),
        };
        self.param_input.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.select_all_text(cx);
        });
        self.param_edit = Some(kind);
        self.param_input.focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn apply_param_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.param_edit else {
            return;
        };
        let text = self.param_input.read(cx).text();
        let text = text.trim();
        if let Ok(v) = text.parse::<i32>() {
            match kind {
                ParamEdit::Margin => self.doc.margin = v.clamp(0, 80),
                ParamEdit::Threshold => self.doc.ink_threshold = v.clamp(1, 254),
            }
        }
        self.param_edit = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn cancel_param_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.param_edit.is_none() {
            return;
        }
        self.param_edit = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn apply_edit_y(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(region_id) = self.region_y_edit.take() else {
            return;
        };
        let text = self.edit_y_input.read(cx).text();
        let text = text
            .trim()
            .replace(' ', "")
            .replace(',', "-")
            .replace('–', "-");
        self.focus_handle.focus(window);
        if !text.contains('-') {
            self.status = "y 范围需为 y0-y1, 例如 94-371".into();
            cx.notify();
            return;
        }
        let mut parts = text.splitn(2, '-');
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        let (Ok(y0), Ok(y1)) = (a.parse::<i32>(), b.parse::<i32>()) else {
            self.status = "y0 / y1 必须是整数".into();
            cx.notify();
            return;
        };
        if self.doc.find_region(&region_id).is_none() {
            self.status = "未能修改该块 y 范围".into();
            cx.notify();
            return;
        }
        self.push_crop_undo_current();
        if self.doc.set_region_y(&region_id, y0, y1) {
            self.status = format!("已改 → y={}-{}", y0.min(y1), y0.max(y1)).into();
            self.after_doc_change(cx);
        } else {
            if let Some(cur) = self.doc.current_page().map(|p| p.id.clone()) {
                if let Some(h) = self.crop_histories.get_mut(&cur) {
                    h.undo.pop();
                }
            }
            self.status = "未能修改该块 y 范围".into();
            cx.notify();
        }
    }

    pub(super) fn cancel_edit_y(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.region_y_edit.is_none() {
            return;
        }
        self.region_y_edit = None;
        self.focus_handle.focus(window);
        cx.notify();
    }
}
