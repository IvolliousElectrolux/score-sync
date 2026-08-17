//! 页识别结果 sidecar: 与 PNG 同目录的 `*.png.detect.json`.
//!
//! 第一轮识别在读图线程 (PDF 渲染 / 窗口加载) 完成并落盘, UI 只读 JSON,
//! 不在界面线程解码整本 PDF.

use std::path::{Path, PathBuf};

use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::staff_detect::{detect_bands, Band, StaffGrouping};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedRegion {
    pub id: String,
    pub y0: i32,
    pub y1: i32,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageDetectFile {
    pub img_w: u32,
    pub img_h: u32,
    pub ink_threshold: i32,
    pub margin: i32,
    #[serde(default)]
    pub staff_grouping: StaffGrouping,
    pub regions: Vec<CachedRegion>,
}

pub fn sidecar_path(png: &Path) -> PathBuf {
    let mut s = png.as_os_str().to_os_string();
    s.push(".detect.json");
    PathBuf::from(s)
}

pub fn load(png: &Path) -> Option<PageDetectFile> {
    let path = sidecar_path(png);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save(png: &Path, file: &PageDetectFile) -> Result<(), String> {
    let path = sidecar_path(png);
    let json = serde_json::to_vec_pretty(file).map_err(|e| format!("序列化识别缓存失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写识别缓存失败: {e}"))
}

fn new_rid() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub fn detect_to_file(img: &RgbImage, ink_threshold: i32, margin: i32) -> PageDetectFile {
    let mut bands = detect_bands(img, ink_threshold, margin);
    if bands.is_empty() {
        bands.push(Band {
            y0: 0,
            y1: img.height().saturating_sub(1) as i32,
            kind: "region".into(),
        });
    }
    let regions = bands
        .into_iter()
        .map(|b| CachedRegion {
            id: new_rid(),
            y0: b.y0,
            y1: b.y1,
            kind: b.kind,
        })
        .collect();
    PageDetectFile {
        img_w: img.width(),
        img_h: img.height(),
        ink_threshold,
        margin,
        staff_grouping: StaffGrouping::default(),
        regions,
    }
}

/// 识别并写入 sidecar. 失败仍返回内存结果.
pub fn detect_and_save(
    img: &RgbImage,
    png_path: &Path,
    ink_threshold: i32,
    margin: i32,
) -> PageDetectFile {
    let file = detect_to_file(img, ink_threshold, margin);
    if let Err(e) = save(png_path, &file) {
        crate::trace::log(&format!("detect sidecar 写入失败: {e}"));
    }
    file
}

/// 已有 sidecar 则读出; 否则用当前像素识别并落盘.
pub fn load_or_detect(
    img: &RgbImage,
    png_path: &Path,
    ink_threshold: i32,
    margin: i32,
) -> PageDetectFile {
    if let Some(cached) = load(png_path) {
        if cached.ink_threshold == ink_threshold && cached.margin == margin {
            return cached;
        }
    }
    detect_and_save(img, png_path, ink_threshold, margin)
}
