use super::*;
use std::path::PathBuf;

impl PhotoEditApp {
    pub fn open_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "图片",
                &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"],
            )
            .pick_file()
        {
            self.load_image(path, cx);
        }
    }

    pub fn load_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match image::open(&path) {
            Ok(img) => {
                let rgb = img.to_rgb8();
                self.image_path = Some(path);
                self.open_rgb(rgb, None, cx);
            }
            Err(e) => {
                self.status = format!("打开失败: {e}").into();
                self.notify_chrome(cx);
            }
        }
    }

    pub fn export_image_standalone(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else {
            self.status = "没有可导出的图".into();
            self.notify_chrome(cx);
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name("edit.png")
            .save_file()
        else {
            return;
        };
        let flat = doc.flatten_rgb();
        match flat.save(&path) {
            Ok(()) => {
                self.status = format!("已导出 {}", path.display()).into();
                self.dirty = false;
            }
            Err(e) => self.status = format!("导出失败: {e}").into(),
        }
        self.notify_chrome(cx);
    }
}
