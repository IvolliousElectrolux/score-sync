//! 工程底色层 (图片 / 纯色).

use super::*;
use std::path::PathBuf;
use std::sync::Arc;

use image::RgbImage;

impl DocState {
    /// 启用工程底色层 (底层). 不修改页图 / 蒙版. `image` 与 UI 缓存可共享同一 `Arc`.
    pub fn set_project_bg_arc(
        &mut self,
        image: Arc<RgbImage>,
        source: Option<PathBuf>,
        aspect_w: u32,
        aspect_h: u32,
    ) -> Result<(), String> {
        if aspect_w == 0 || aspect_h == 0 {
            return Err("比例宽高必须为正整数".into());
        }
        self.bg_image = Some(image);
        self.bg_solid = None;
        self.bg_source_path = source;
        self.bg_aspect_w = aspect_w;
        self.bg_aspect_h = aspect_h;
        self.bg_enabled = true;
        self.bg_gen = self.bg_gen.wrapping_add(1);
        self.seed_guide_defaults();
        Ok(())
    }

    /// 启用纯色底色层. 不分配整张底色图.
    pub fn set_project_bg_solid(
        &mut self,
        color: [u8; 3],
        aspect_w: u32,
        aspect_h: u32,
    ) -> Result<(), String> {
        if aspect_w == 0 || aspect_h == 0 {
            return Err("比例宽高必须为正整数".into());
        }
        self.bg_image = None;
        self.bg_solid = Some(color);
        self.bg_source_path = None;
        self.bg_aspect_w = aspect_w;
        self.bg_aspect_h = aspect_h;
        self.bg_enabled = true;
        self.bg_gen = self.bg_gen.wrapping_add(1);
        self.seed_guide_defaults();
        Ok(())
    }

    /// 已启用纯色时只改颜色 (几何不变, 不重算辅助线).
    pub fn update_bg_solid_color(&mut self, color: [u8; 3]) {
        self.bg_solid = Some(color);
        self.bg_gen = self.bg_gen.wrapping_add(1);
    }

    /// 预览/合成用的底色源尺寸. 纯色没有像素备份, 用能盖住各页的虚拟画布.
    pub fn bg_src_size(&self) -> Option<(u32, u32)> {
        if !self.bg_enabled {
            return None;
        }
        if let Some(img) = self.bg_image.as_ref() {
            return Some((img.width(), img.height()));
        }
        if self.bg_solid.is_some() {
            let aw = self.bg_aspect_w.max(1);
            let ah = self.bg_aspect_h.max(1);
            let max_w = self
                .pages
                .iter()
                .map(|p| p.width())
                .max()
                .unwrap_or(aw)
                .max(aw)
                .max(1);
            return Some(apply_bg::process::page_size(max_w, aw, ah));
        }
        None
    }

    /// 取消工程底色层.
    pub fn clear_project_bg(&mut self) {
        self.bg_enabled = false;
        self.bg_image = None;
        self.bg_solid = None;
        self.bg_source_path = None;
        self.bg_gen = self.bg_gen.wrapping_add(1);
        self.seed_guide_defaults();
    }
}
