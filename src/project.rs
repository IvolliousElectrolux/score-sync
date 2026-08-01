//! 工程文件: 单个 `*.staffcrop` (= zip), 内含 `project.json` + `pages/*.png`.
//!
//! 保存时把当前内存中的页图打进压缩包, PDF 临时页也能跨会话恢复.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use image::ImageFormat;
use mask_tool::mask::MaskRect;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::model::{DocState, Group, Page, Region};

pub const PROJECT_EXT: &str = "staffcrop";
pub const PROJECT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    margin: i32,
    ink_threshold: i32,
    mask_opacity: f32,
    current_page_index: usize,
    active_group_id: Option<String>,
    selected_region_ids: Vec<String>,
    pages: Vec<ProjectPage>,
    groups: Vec<ProjectGroup>,
    /// group_id -> masks
    group_masks: HashMap<String, Vec<MaskRect>>,
}

#[derive(Serialize, Deserialize)]
struct ProjectPage {
    id: String,
    /// 标签显示名
    title: String,
    /// zip 内相对路径, 如 `pages/abcd.png`
    image: String,
    regions: Vec<ProjectRegion>,
}

#[derive(Serialize, Deserialize)]
struct ProjectRegion {
    id: String,
    page_id: String,
    y0: i32,
    y1: i32,
    kind: String,
    color: String,
}

#[derive(Serialize, Deserialize)]
struct ProjectGroup {
    id: String,
    name: String,
    region_ids: Vec<String>,
}

pub fn is_project_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(PROJECT_EXT))
        .unwrap_or(false)
}

fn ensure_staffcrop_ext(path: PathBuf) -> PathBuf {
    if is_project_path(&path) {
        path
    } else {
        let mut p = path;
        p.set_extension(PROJECT_EXT);
        p
    }
}

fn encode_png(image: &image::RgbImage) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("编码 PNG 失败: {e}"))?;
    Ok(buf.into_inner())
}

/// 保存工程为单个 zip 文件. `path` 可为无扩展名, 会自动补 `.staffcrop`.
pub fn save_project(doc: &DocState, path: &Path) -> Result<PathBuf, String> {
    let project_path = ensure_staffcrop_ext(path.to_path_buf());
    // 先写临时文件再替换, 避免写到一半失败毁掉旧工程
    let tmp_path = project_path.with_extension("staffcrop.tmp");

    let mut pages = Vec::with_capacity(doc.pages.len());
    let mut page_pngs: Vec<(String, Vec<u8>)> = Vec::with_capacity(doc.pages.len());
    for page in &doc.pages {
        let rel = format!("pages/{}.png", page.id);
        let png = encode_png(&page.image)
            .map_err(|e| format!("保存页图失败 ({}): {e}", page.id))?;
        page_pngs.push((rel.clone(), png));
        let mut regions: Vec<ProjectRegion> = page
            .regions
            .values()
            .map(|r| ProjectRegion {
                id: r.id.clone(),
                page_id: r.page_id.clone(),
                y0: r.y0,
                y1: r.y1,
                kind: r.kind.clone(),
                color: r.color.clone(),
            })
            .collect();
        regions.sort_by_key(|r| (r.y0, r.y1, r.id.clone()));
        pages.push(ProjectPage {
            id: page.id.clone(),
            title: page.title(),
            image: rel,
            regions,
        });
    }

    let groups: Vec<ProjectGroup> = doc
        .groups
        .iter()
        .map(|g| ProjectGroup {
            id: g.id.clone(),
            name: g.name.clone(),
            region_ids: g.region_ids.clone(),
        })
        .collect();

    let mut selected: Vec<String> = doc.selected_region_ids.iter().cloned().collect();
    selected.sort();

    let meta = ProjectFile {
        version: PROJECT_VERSION,
        margin: doc.margin,
        ink_threshold: doc.ink_threshold,
        mask_opacity: doc.mask_opacity,
        current_page_index: doc.current_page_index.min(doc.pages.len().saturating_sub(1)),
        active_group_id: doc.active_group_id.clone(),
        selected_region_ids: selected,
        pages,
        groups,
        group_masks: doc.group_masks.clone(),
    };
    let json = serde_json::to_vec_pretty(&meta).map_err(|e| format!("序列化工程失败: {e}"))?;

    {
        let file = File::create(&tmp_path).map_err(|e| format!("创建临时工程失败: {e}"))?;
        let mut zip = ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("project.json", opts)
            .map_err(|e| format!("写入 project.json 失败: {e}"))?;
        zip.write_all(&json)
            .map_err(|e| format!("写入 project.json 失败: {e}"))?;

        for (rel, png) in &page_pngs {
            zip.start_file(rel, opts)
                .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
            zip.write_all(png)
                .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
        }

        zip.finish()
            .map_err(|e| format!("完成工程压缩包失败: {e}"))?;
    }

    if project_path.exists() {
        std::fs::remove_file(&project_path)
            .map_err(|e| format!("覆盖旧工程失败: {e}"))?;
    }
    std::fs::rename(&tmp_path, &project_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("写出工程文件失败: {e}")
    })?;
    Ok(project_path)
}

fn read_zip_entry(zip: &mut ZipArchive<File>, name: &str) -> Result<Vec<u8>, String> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| format!("工程内缺少 {name}: {e}"))?;
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取 {name} 失败: {e}"))?;
    Ok(buf)
}

/// 读取工程 zip, 返回完整 DocState (不重新识别).
pub fn load_project(path: &Path) -> Result<DocState, String> {
    if !is_project_path(path) {
        return Err("不是 .staffcrop 工程文件".into());
    }
    let file = File::open(path).map_err(|e| format!("打开工程失败: {e}"))?;
    let mut zip = ZipArchive::new(file).map_err(|e| {
        format!("无法作为工程压缩包打开 (是否为旧版旁路目录工程?): {e}")
    })?;

    let json_bytes = read_zip_entry(&mut zip, "project.json")?;
    let meta: ProjectFile = serde_json::from_slice(&json_bytes)
        .map_err(|e| format!("解析 project.json 失败: {e}"))?;
    if meta.version == 0 || meta.version > PROJECT_VERSION {
        return Err(format!(
            "不支持的工程版本 {} (当前支持 1–{PROJECT_VERSION})",
            meta.version
        ));
    }

    let mut pages = Vec::with_capacity(meta.pages.len());
    for p in &meta.pages {
        let png = read_zip_entry(&mut zip, &p.image)?;
        let image = image::load_from_memory(&png)
            .map_err(|e| format!("解码页图失败 ({}): {e}", p.image))?
            .to_rgb8();
        let mut regions = HashMap::new();
        for r in &p.regions {
            regions.insert(
                r.id.clone(),
                Region {
                    id: r.id.clone(),
                    page_id: r.page_id.clone(),
                    y0: r.y0,
                    y1: r.y1,
                    kind: r.kind.clone(),
                    color: r.color.clone(),
                },
            );
        }
        let display_path = if p.title.is_empty() {
            PathBuf::from(&p.image)
        } else {
            PathBuf::from(&p.title)
        };
        pages.push(Page {
            id: p.id.clone(),
            path: display_path,
            image,
            regions,
        });
    }

    let groups: Vec<Group> = meta
        .groups
        .into_iter()
        .map(|g| Group {
            id: g.id,
            name: g.name,
            region_ids: g.region_ids,
        })
        .collect();

    let mut doc = DocState {
        pages,
        groups,
        selected_region_ids: meta.selected_region_ids.into_iter().collect(),
        active_group_id: meta.active_group_id,
        current_page_index: meta.current_page_index,
        margin: meta.margin,
        ink_threshold: meta.ink_threshold,
        group_masks: meta.group_masks,
        mask_opacity: meta.mask_opacity,
    };
    if doc.current_page_index >= doc.pages.len() {
        doc.current_page_index = 0;
    }
    if let Some(ref gid) = doc.active_group_id {
        if !doc.groups.iter().any(|g| g.id == *gid) {
            doc.active_group_id = doc.groups.first().map(|g| g.id.clone());
        }
    } else {
        doc.ensure_active_group();
    }
    let valid_regions: HashSet<String> = doc
        .pages
        .iter()
        .flat_map(|p| p.regions.keys().cloned())
        .collect();
    doc.selected_region_ids
        .retain(|id| valid_regions.contains(id));
    Ok(doc)
}
