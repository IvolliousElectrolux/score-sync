//! 页 / 组合 / 区域类型与路径辅助.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbImage;

pub const COLORS: &[&str] = &[
    "#e74c3c", "#3498db", "#2ecc71", "#f39c12", "#9b59b6", "#1abc9c", "#e67e22",
    "#2980b9", "#16a085", "#c0392b",
];

pub const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp", ".webp"];

pub const DEFAULT_MARGIN: i32 = 20;
pub const DEFAULT_INK_THRESHOLD: i32 = 200;

#[derive(Clone, Debug)]
pub struct Region {
    pub id: String,
    pub page_id: String,
    pub y0: i32,
    pub y1: i32,
    pub kind: String,
    pub color: String,
}

impl Region {
    pub fn label(&self, page_no: Option<usize>) -> String {
        let prefix = page_no
            .map(|n| format!("P{n} "))
            .unwrap_or_default();
        format!(
            "{prefix}{}  y={}-{}  h={}",
            self.kind,
            self.y0,
            self.y1,
            self.y1 - self.y0 + 1
        )
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub id: String,
    /// 显示用路径 / 原始文件名
    pub path: PathBuf,
    /// 会话 tmp (或工程解压落盘) 上的 PNG 备份
    pub disk_path: PathBuf,
    /// 仅内存窗口内有值; 窗口外为 None. 交互预览按
    /// [`DocState::display_max_side`] 缩小, 坐标仍用下面的原图像素尺寸.
    /// `Arc` 让后台任务廉价共享同一份显示像素.
    pub image: Option<Arc<RgbImage>>,
    /// 磁盘原图尺寸 (识别 / 区域 y0y1 / 导出). 与 `image` 像素宽高可能不同.
    pub img_w: u32,
    pub img_h: u32,
    pub regions: HashMap<String, Region>,
}

impl Page {
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("page")
            .to_string()
    }

    /// 页签短标签用的原页码 (PDF `_p012`) 与「复制」标记.
    pub fn tab_badge(&self, fallback_index1: usize) -> String {
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let src = source_page_no_from_stem(stem)
            .map(|n| n.to_string())
            .unwrap_or_else(|| fallback_index1.to_string());
        let copy = copy_mark_from_stem(stem);
        format!("{src}{copy}")
    }

    pub fn width(&self) -> u32 {
        self.img_w
    }

    pub fn height(&self) -> u32 {
        self.img_h
    }

    /// 当前内存里那份图的字节数 (显示代理; 未加载则按原图估).
    pub fn estimated_bytes(&self) -> u64 {
        if let Some(img) = self.image.as_ref() {
            img.width() as u64 * img.height() as u64 * 3
        } else {
            self.estimated_full_bytes()
        }
    }

    /// 磁盘原图解码后的 RGB 字节数. 导出并发按这个估峰值.
    pub fn estimated_full_bytes(&self) -> u64 {
        (self.img_w as u64) * (self.img_h as u64) * 3
    }

    /// 内存图与原图同尺寸 (测试夹具 / 尚未缩小的小图).
    pub fn is_full_res(&self) -> bool {
        self.image
            .as_ref()
            .map(|img| img.width() == self.img_w && img.height() == self.img_h)
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub region_ids: Vec<String>,
    pub name: String,
}

impl Group {
    pub fn display_name(&self, index: usize) -> String {
        if self.name.is_empty() {
            format!("组合 {}", index + 1)
        } else {
            self.name.clone()
        }
    }
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = format!(".{}", e.to_ascii_lowercase());
            IMAGE_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

pub fn is_open_path(path: &Path) -> bool {
    is_image_path(path) || is_pdf_path(path)
}

fn source_page_no_from_stem(stem: &str) -> Option<u32> {
    let b = stem.as_bytes();
    let mut i = 0;
    let mut last = None;
    while i + 2 < b.len() {
        if b[i] == b'_' && b[i + 1] == b'p' && b[i + 2].is_ascii_digit() {
            let start = i + 2;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            if let Ok(n) = stem[start..end].parse::<u32>() {
                last = Some(n);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    last
}

fn copy_mark_from_stem(stem: &str) -> String {
    let Some(pos) = stem.rfind("_copy") else {
        return String::new();
    };
    let rest = &stem[pos + 5..];
    if rest.starts_with("_p") {
        return String::new();
    }
    if rest.is_empty() {
        "复制".into()
    } else if rest.chars().all(|c| c.is_ascii_digit()) {
        format!("复制{rest}")
    } else {
        "复制".into()
    }
}

pub fn parse_color_hex(s: &str) -> u32 {
    let s = s.trim().trim_start_matches('#');
    u32::from_str_radix(s, 16).unwrap_or(0x3498db)
}

/// 裁出 `img` 的 `[y0, y0+height)` 整行条带 (宽度不变), 按行整块
/// `copy_from_slice`, 不用 `image::imageops::crop_imm().to_image()`
/// (内部逐像素调用 get_pixel/put_pixel; 高清扫描页整页宽度的裁切这样
/// 调用开销很可观, 是切到蒙版/底色面板时卡顿的根因之一, 见
/// `apply_bg::process::crop_fast`/`mask_tool::layout::blit_rows` 同样的
/// 考量). 供 [`DocState::crop_region`] 与「全局对齐」后台任务复用.
pub(crate) fn crop_band_fast(img: &RgbImage, y0: u32, height: u32) -> RgbImage {
    let w = img.width();
    let h = img.height();
    let y0 = y0.min(h);
    let height = height.min(h.saturating_sub(y0));
    let mut out = RgbImage::new(w, height);
    let row_bytes = w as usize * 3;
    let src: &[u8] = img;
    let dst: &mut [u8] = &mut out;
    for row in 0..height as usize {
        let s0 = (y0 as usize + row) * row_bytes;
        let d0 = row * row_bytes;
        dst[d0..d0 + row_bytes].copy_from_slice(&src[s0..s0 + row_bytes]);
    }
    out
}
