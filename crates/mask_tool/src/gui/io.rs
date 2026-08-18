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

    pub fn session_key(&self) -> Option<&str> {
        self.session_key.as_deref()
    }

    /// 从内存 RGB 载入 (组内成员裁切图); 坐标相对该裁切图.
    pub fn load_rgb(
        &mut self,
        rgb: image::RgbImage,
        session_key: String,
        masks: Vec<MaskRect>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        if self.session_key.as_ref() == Some(&session_key) && self.rgb_image.is_some() {
            return;
        }
        self.stash_history();
        let (w, h) = rgb.dimensions();
        let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
            let p = rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let render = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
        self.image_path = None;
        self.session_key = Some(session_key.clone());
        self.rgb_image = Some(rgb);
        self.render_image = Some(render);
        self.img_w = w;
        self.img_h = h;
        self.masks = masks;
        self.selected.clear();
        self.restore_history_for(&session_key);
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        self.user_zoomed = false;
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = format!("{label} ({w}×{h}) · 蒙版 {} 个", self.masks.len()).into();
        self.hint = format!(
            "编辑: {label}\n蒙版坐标相对本组合拼合图; 各组合独立 (共享脚注可在不同组画不同遮盖)."
        )
        .into();
        cx.notify();
    }

    pub fn clear_view(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.stash_history();
        self.image_path = None;
        self.session_key = None;
        self.rgb_image = None;
        self.render_image = None;
        self.img_w = 0;
        self.img_h = 0;
        self.masks.clear();
        self.selected.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = message.into();
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
                let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
                    let p = rgb.get_pixel(x, y);
                    image::Rgba([p[0], p[1], p[2], 255])
                });
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                let render = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
                let restored = self
                    .page_masks
                    .get(&path)
                    .cloned()
                    .unwrap_or_default();
                // 先把旧页的撤重栈存好, 再切路径.
                self.stash_history();
                self.image_path = Some(path.clone());
                self.session_key = None;
                self.rgb_image = Some(rgb);
                self.render_image = Some(render);
                self.img_w = w;
                self.img_h = h;
                self.masks = restored;
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
