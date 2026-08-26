//! 加载拼合图 / 打开导出 / 会话历史.

use super::*;

impl MaskToolApp {
    pub(super) fn history_key(&self) -> Option<String> {
        self.session_key
            .clone()
            .or_else(|| self.image_path.as_ref().map(|p| p.display().to_string()))
    }

    pub(super) fn stash_history(&mut self) {
        let Some(key) = self.history_key() else {
            return;
        };
        self.histories.insert(
            key,
            MaskHistory {
                undo: self.undo_stack.clone(),
                redo: self.redo_stack.clone(),
            },
        );
    }

    pub(super) fn restore_history_for(&mut self, key: &str) {
        if let Some(h) = self.histories.get(key) {
            self.undo_stack = h.undo.clone();
            self.redo_stack = h.redo.clone();
        } else {
            self.undo_stack.clear();
            self.redo_stack.clear();
        }
    }

    pub fn masks_clone(&self) -> Vec<MaskRect> {
        self.masks.clone()
    }

    pub fn guides_clone(&self) -> GuideState {
        self.guides.clone()
    }

    pub fn session_key(&self) -> Option<&str> {
        self.session_key.as_deref()
    }

    /// 让下一次 `load_rgb` 不再因 session_key 相同而跳过, 供宿主在改完
    /// 其它组合的辅助线/布局后强制重载当前页.
    pub fn invalidate_session(&mut self) {
        self.session_key = None;
    }

    /// 从内存 RGB 载入 (组内成员裁切图); 坐标相对该裁切图.
    pub fn load_rgb(
        &mut self,
        rgb: image::RgbImage,
        session_key: String,
        masks: Vec<MaskRect>,
        guides: GuideState,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let render = rgb_to_render_image(&rgb);
        self.load_rgb_with_render(rgb, render, session_key, masks, guides, label, cx);
    }

    /// 同 [`Self::load_rgb`], 但 GPU 贴图由调用方预先算好 (通常在后台线程,
    /// 见 `rgb_to_render_image` 文档: 高清拼合图这一步自己就要上百毫秒,
    /// 调用方若在界面线程上再转一遍就白白多卡一次). 宿主 (score_sync)
    /// 的 `sync_mask_image` 走这条路径, 界面线程只做一次贴图指针替换.
    pub fn load_rgb_with_render(
        &mut self,
        rgb: image::RgbImage,
        render: Arc<RenderImage>,
        session_key: String,
        masks: Vec<MaskRect>,
        guides: GuideState,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        if self.session_key.as_ref() == Some(&session_key) && self.rgb_image.is_some() {
            return;
        }
        self.stash_history();
        let (w, h) = rgb.dimensions();
        self.image_path = None;
        self.session_key = Some(session_key.clone());
        self.rgb_image = Some(rgb);
        self.replace_render_image(Some(render));
        self.img_w = w;
        self.img_h = h;
        self.clamp_brush_size();
        self.masks = masks;
        self.guides = guides;
        self.guide_selected.clear();
        self.selected.clear();
        self.restore_history_for(&session_key);
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.user_zoomed = false;
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.block_drag_freeze = None;
        self.status = format!("{label} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
        self.hint = format!(
            "编辑: {label}\n蒙版坐标相对本组合拼合图; 各组合独立 (共享脚注可在不同组画不同遮盖)."
        )
        .into();
        self.canvas_loading = false;
        cx.notify();
    }

    pub fn clear_view(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.stash_history();
        self.image_path = None;
        self.session_key = None;
        self.rgb_image = None;
        self.replace_render_image(None);
        self.canvas_loading = false;
        self.img_w = 0;
        self.img_h = 0;
        self.masks.clear();
        self.guides = GuideState::default();
        self.guide_selected.clear();
        self.selected.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.block_heights.clear();
        self.block_layout.clear();
        self.set_block_tiles(Vec::new(), None);
        self.piece_staff_ys.clear();
        self.block_hoff = 0;
        self.block_voff = 0;
        self.block_bg_left = 0;
        self.block_bg_top = 0;
        self.block_shows_bg = false;
        self.block_drag_freeze = None;
        self.status = message.into();
        cx.notify();
    }

    /// 立刻清空画布并挂「加载中」占位, 像素由宿主在后台生成完再
    /// [`Self::load_rgb_with_render`]. 界面线程只做这一步, 好让面板/组合
    /// 切换先跟手画出来.
    pub fn begin_preview_load(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.clear_view(message, cx);
        self.canvas_loading = true;
        cx.notify();
    }

    /// 更新当前会话预览图 (对齐/全局撤重后), 不碰撤重栈、缩放和平移.
    pub fn replace_session_image(
        &mut self,
        rgb: image::RgbImage,
        masks: Vec<MaskRect>,
        guides: GuideState,
        cx: &mut Context<Self>,
    ) {
        let render = rgb_to_render_image(&rgb);
        self.replace_session_image_with_render(rgb, render, masks, guides, cx);
    }

    /// 同 [`Self::replace_session_image`], 但 GPU 贴图由调用方预先算好
    /// (通常在后台线程, 见 `rgb_to_render_image` 文档), 界面线程只做一次
    /// 贴图指针替换; 供全局对齐/撤重后刷新预览时避免堵在界面线程上.
    pub fn replace_session_image_with_render(
        &mut self,
        rgb: image::RgbImage,
        render: Arc<RenderImage>,
        masks: Vec<MaskRect>,
        guides: GuideState,
        cx: &mut Context<Self>,
    ) {
        let (w, h) = rgb.dimensions();
        self.rgb_image = Some(rgb);
        self.replace_render_image(Some(render));
        self.img_w = w;
        self.img_h = h;
        self.clamp_brush_size();
        self.masks = masks;
        self.guides = guides;
        self.guide_selected.clear();
        self.canvas_loading = false;
        self.block_drag_freeze = None;
        cx.notify();
    }

    pub fn load_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !is_image_path(&path) {
            self.status = format!("不是支持的图片: {}", path.display()).into();
            cx.notify();
            return;
        }
        // 切页前缓存当前蒙版
        if let Some(old) = self.image_path.clone() {
            self.page_masks.insert(old, self.masks.clone());
        }
        match image::open(&path) {
            Ok(img) => {
                let rgb = img.to_rgb8();
                let (w, h) = rgb.dimensions();
                let restored = self
                    .page_masks
                    .get(&path)
                    .cloned()
                    .unwrap_or_default();
                let render = rgb_to_render_image(&rgb);
                // 先把旧页的撤重栈存好, 再切路径.
                self.stash_history();
                self.image_path = Some(path.clone());
                self.session_key = None;
                self.rgb_image = Some(rgb);
                self.replace_render_image(Some(render));
                self.img_w = w;
                self.img_h = h;
                self.clamp_brush_size();
                self.masks = restored;
                self.guides = GuideState::default();
                self.guide_selected.clear();
                self.selected.clear();
                self.restore_history_for(&path.display().to_string());
                self.zoom = 1.0;
                self.pan = point(0.0, 0.0);
                self.user_zoomed = false;
                self.drag = None;
                self.poly_draft = None;
                self.poly_cursor = None;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image");
                self.status = format!("已载入 {name} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
                self.hint = format!(
                    "已载入 {name}. 框选/折线/画笔画蒙版; 平移拖动画布或已选框."
                )
                .into();
                cx.notify();
            }
            Err(e) => {
                self.status = format!("打开失败: {e}").into();
                cx.notify();
            }
        }
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

    pub fn open_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // 嵌入宿主时由宿主管开图, 这里弹独立选图会盖掉当前组合.
        if self.embed_side_width > 1.0 {
            return;
        }
        let start = self
            .image_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        Self::spawn_native_dialog(
            cx,
            move || {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("打开图片")
                    .add_filter(
                        "Images",
                        &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"],
                    );
                if let Some(parent) = start {
                    dialog = dialog.set_directory(parent);
                }
                dialog.pick_file()
            },
            |this, path, cx| {
                if let Some(path) = path {
                    this.load_image(path, cx);
                }
            },
        );
    }

    pub fn export_image(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.rgb_image.is_none() {
            self.status = "请先打开图片.".into();
            cx.notify();
            return;
        }
        let suggested = default_export_path(self.image_path.as_deref());
        let file_name = suggested
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("masked.png")
            .to_string();
        let start_dir = suggested.parent().filter(|p| p.is_dir()).map(|p| p.to_path_buf());
        Self::spawn_native_dialog(
            cx,
            move || {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("导出已遮盖图片")
                    .add_filter("PNG", &["png"])
                    .add_filter("JPEG", &["jpg", "jpeg"])
                    .set_file_name(file_name);
                if let Some(parent) = start_dir {
                    dialog = dialog.set_directory(parent);
                }
                dialog.save_file()
            },
            |this, path, cx| {
                let Some(path) = path else {
                    return;
                };
                let Some(ref base) = this.rgb_image else {
                    this.status = "请先打开图片.".into();
                    cx.notify();
                    return;
                };
                match export_masked(base, &this.masks, this.mask_opacity, &path) {
                    Ok(()) => {
                        this.status = format!("已保存: {}", path.display()).into();
                    }
                    Err(e) => {
                        this.status = e.into();
                    }
                }
                cx.notify();
            },
        );
    }
}
