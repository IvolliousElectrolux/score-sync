//! 页识别结果 sidecar: 与 PNG 同目录的 `*.png.detect.json`.
//!
//! 第一轮识别在读图线程 (PDF 渲染 / 窗口加载) 完成并落盘, UI 只读 JSON,
//! 不在界面线程解码整本 PDF.

use std::path::{Path, PathBuf};

use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::staff_detect::{detect_bands, merge_system_bands, Band, StaffGrouping, StaffPackMode};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedStaffAnchor {
    /// `Some` = 谱表锚点 (相对条带顶); `None` = 已判定不是五线谱.
    pub y: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedLineBand {
    pub y0: i32,
    pub y1: i32,
    pub kind: String,
    /// 相对条带顶. 旧缓存没有.
    #[serde(default)]
    pub staff_anchor: Option<CachedStaffAnchor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedRegion {
    pub id: String,
    pub y0: i32,
    pub y1: i32,
    pub kind: String,
    /// 旧 sidecar 没有该字段, 视为尚未计算.
    #[serde(default)]
    pub staff_anchor: Option<CachedStaffAnchor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageDetectFile {
    pub img_w: u32,
    pub img_h: u32,
    pub ink_threshold: i32,
    pub margin: i32,
    #[serde(default)]
    pub staff_grouping: StaffGrouping,
    /// 页内合并实际收进行数. 跨页组合不改几何, 这里仍是 1.
    #[serde(default)]
    pub staff_pack_lines: u32,
    #[serde(default)]
    pub staff_pack_mode: StaffPackMode,
    /// 合并前的一行一块. 旧缓存没有.
    #[serde(default)]
    pub line_bands: Vec<CachedLineBand>,
    pub regions: Vec<CachedRegion>,
}

fn baked_pack_lines(lines: u32, mode: StaffPackMode) -> u32 {
    if mode == StaffPackMode::WithinPage && lines >= 2 {
        lines
    } else {
        1
    }
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

pub fn detect_to_file(
    img: &RgbImage,
    ink_threshold: i32,
    margin: i32,
    pack_lines: u32,
    pack_mode: StaffPackMode,
) -> PageDetectFile {
    let mut bands = detect_bands(img, ink_threshold, margin);
    if bands.is_empty() {
        bands.push(Band {
            y0: 0,
            y1: img.height().saturating_sub(1) as i32,
            kind: "region".into(),
        });
    }
    let line_bands = bands
        .iter()
        .map(|b| {
            let y = mask_tool::staff::band_staff_anchor(img, b.y0, b.y1, ink_threshold);
            CachedLineBand {
                y0: b.y0,
                y1: b.y1,
                kind: b.kind.clone(),
                staff_anchor: Some(CachedStaffAnchor { y }),
            }
        })
        .collect();
    if pack_mode == StaffPackMode::WithinPage {
        bands = merge_system_bands(bands, pack_lines);
    }
    let regions = bands
        .into_iter()
        .map(|b| {
            let y = mask_tool::staff::band_staff_anchor(img, b.y0, b.y1, ink_threshold);
            CachedRegion {
                id: new_rid(),
                y0: b.y0,
                y1: b.y1,
                kind: b.kind,
                staff_anchor: Some(CachedStaffAnchor { y }),
            }
        })
        .collect();
    PageDetectFile {
        img_w: img.width(),
        img_h: img.height(),
        ink_threshold,
        margin,
        staff_grouping: StaffGrouping::default(),
        staff_pack_lines: pack_lines,
        staff_pack_mode: pack_mode,
        line_bands,
        regions,
    }
}

/// 识别并写入 sidecar. 失败仍返回内存结果.
pub fn detect_and_save(
    img: &RgbImage,
    png_path: &Path,
    ink_threshold: i32,
    margin: i32,
    pack_lines: u32,
    pack_mode: StaffPackMode,
) -> PageDetectFile {
    let file = detect_to_file(img, ink_threshold, margin, pack_lines, pack_mode);
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
    pack_lines: u32,
    pack_mode: StaffPackMode,
) -> PageDetectFile {
    if let Some(cached) = load(png_path) {
        if cached.ink_threshold == ink_threshold
            && cached.margin == margin
            && baked_pack_lines(cached.staff_pack_lines, cached.staff_pack_mode)
                == baked_pack_lines(pack_lines, pack_mode)
        {
            return cached;
        }
    }
    detect_and_save(img, png_path, ink_threshold, margin, pack_lines, pack_mode)
}
