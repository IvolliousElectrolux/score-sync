//! 工程文件: 单个 `*.staffcrop` (= zip), 内含 `project.json` + `pages/*.png`.
//!
//! 保存时把当前内存中的页图打进压缩包, PDF 临时页也能跨会话恢复.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::ImageFormat;
use mask_tool::mask::MaskRect;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::model::{BlockAdjust, DocState, Group, Page, Region};
use crate::staff_detect::StaffGrouping;
use score_video::model::FadeKind;

pub const PROJECT_EXT: &str = "staffcrop";
pub const PROJECT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    margin: i32,
    ink_threshold: i32,
    #[serde(default)]
    staff_grouping: StaffGrouping,
    mask_opacity: f32,
    #[serde(default)]
    mask_prefs: Option<mask_tool::color_prefs::MaskColorPrefs>,
    current_page_index: usize,
    active_group_id: Option<String>,
    selected_region_ids: Vec<String>,
    pages: Vec<ProjectPage>,
    groups: Vec<ProjectGroup>,
    /// group_id -> masks
    group_masks: HashMap<String, Vec<MaskRect>>,
    /// group_id -> 分块位置/尺寸微调 (蒙版编辑); 旧工程文件没有该字段.
    #[serde(default)]
    group_block_layout: HashMap<String, Vec<ProjectBlockAdjust>>,
    /// group_id -> 组合拼合图相对默认居中位置的纵向手动偏移 (蒙版编辑
    /// 向上拖动分块消耗底色居中留白产生); 旧工程文件没有该字段.
    #[serde(default)]
    group_voff_shift: HashMap<String, i64>,
    /// group_id -> 辅助线 (蒙版画布内固定参考线); 旧工程文件没有该字段.
    #[serde(default)]
    group_guides: HashMap<String, mask_tool::guide::GuideState>,
    /// 辅助线左键是否全局开启; 旧工程没有该字段.
    #[serde(default)]
    guides_global: bool,
    /// 同样根数辅助线的组合是否同步位置; 旧工程没有该字段.
    #[serde(default)]
    guides_sync_positions: bool,
    /// 用户手动调过输出组合顺序
    #[serde(default)]
    groups_manual_order: bool,
    /// 工程底色层 (可选)
    #[serde(default)]
    bg: Option<ProjectBg>,
    /// 视频面板时间轴 (可选, 旧工程文件没有这个字段)
    #[serde(default)]
    video: ProjectVideo,
    /// 分块 P 图覆盖元数据; 像素在 zip `edits/{region_id}/`.
    #[serde(default)]
    region_edits: HashMap<String, ProjectRegionEdit>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ProjectRegionEdit {
    canvas_w: u32,
    canvas_h: u32,
    paper_rgb: [u8; 3],
    #[serde(default)]
    source: Option<photo_edit::SourceFingerprint>,
}

/// 视频面板时间轴的纯数据快照.
#[derive(Serialize, Deserialize, Default)]
struct ProjectVideo {
    #[serde(default)]
    video_clips: Vec<ProjectVideoClip>,
    #[serde(default)]
    fades: Vec<ProjectFadeSpan>,
    #[serde(default)]
    audio_clips: Vec<ProjectAudioClip>,
    #[serde(default)]
    playhead: f64,
}

#[derive(Serialize, Deserialize)]
struct ProjectVideoClip {
    group_id: String,
    start: f64,
    end: f64,
}

#[derive(Serialize, Deserialize)]
struct ProjectFadeSpan {
    start: f64,
    end: f64,
    /// true = 淡入, false = 淡出; 刷入时保持 false 以兼容旧读取器.
    fade_in: bool,
    /// 淡向工程底色而不是纯黑; 旧工程没有该字段, 视为 false.
    #[serde(default)]
    keep_bg: bool,
    /// `"wipe_ltr"` = 左→右刷入; 缺省则按 `fade_in` 当淡入/淡出.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ProjectAudioClip {
    /// 原始音频文件路径 (工程内不重新打包音频, 需与该路径保持有效才能回放/导出)
    path: String,
    label: String,
    duration: f64,
    /// 该段在源文件里的起始偏移秒 (「分割音频」产生的后半段 > 0); 旧工程
    /// 文件没有这个字段, 按 0 处理 (整段从头播放, 和旧行为一致).
    #[serde(default)]
    offset: f64,
}

#[derive(Serialize, Deserialize)]
struct ProjectBg {
    enabled: bool,
    aspect_w: u32,
    aspect_h: u32,
    /// zip 内相对路径, 如 `bg.png`; 纯色可为空
    #[serde(default)]
    image: String,
    #[serde(default)]
    source_path: Option<String>,
    /// 纯色底色 RGB; 旧工程没有该字段.
    #[serde(default)]
    solid_rgb: Option<[u8; 3]>,
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

#[derive(Serialize, Deserialize, Clone)]
struct ProjectBlockAdjust {
    region_id: String,
    #[serde(default)]
    extra_top: i32,
    #[serde(default)]
    extra_bottom: i32,
    #[serde(default)]
    extra_left: i32,
    #[serde(default)]
    extra_right: i32,
    #[serde(default)]
    gap_before: i32,
    #[serde(default)]
    gap_after: i32,
    #[serde(default)]
    shift_x: i32,
}

impl From<&BlockAdjust> for ProjectBlockAdjust {
    fn from(a: &BlockAdjust) -> Self {
        Self {
            region_id: a.region_id.clone(),
            extra_top: a.extra_top,
            extra_bottom: a.extra_bottom,
            extra_left: a.extra_left,
            extra_right: a.extra_right,
            gap_before: a.gap_before,
            gap_after: a.gap_after,
            shift_x: a.shift_x,
        }
    }
}

impl From<ProjectBlockAdjust> for BlockAdjust {
    fn from(a: ProjectBlockAdjust) -> Self {
        Self {
            region_id: a.region_id,
            extra_top: a.extra_top,
            extra_bottom: a.extra_bottom,
            extra_left: a.extra_left,
            extra_right: a.extra_right,
            gap_before: a.gap_before,
            gap_after: a.gap_after,
            shift_x: a.shift_x,
        }
    }
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

/// 把已有 PNG 文件流式写入 zip 条目 (不整文件读进内存).
fn copy_file_into_zip(zip: &mut ZipWriter<File>, path: &Path) -> Result<(), String> {
    let mut src =
        File::open(path).map_err(|e| format!("读取页图失败 ({}): {e}", path.display()))?;
    std::io::copy(&mut src, zip).map_err(|e| format!("写入页图失败 ({}): {e}", path.display()))?;
    Ok(())
}

/// 保存工程为单个 zip 文件. `path` 可为无扩展名, 会自动补 `.staffcrop`.
///
/// 页图逐张流式写入, PNG 用 Stored (本身已压缩); 不再把全部页一次读进内存.
pub fn save_project(doc: &DocState, path: &Path) -> Result<PathBuf, String> {
    let project_path = ensure_staffcrop_ext(path.to_path_buf());
    // 先写临时文件再替换, 避免写到一半失败毁掉旧工程
    let tmp_path = project_path.with_extension("staffcrop.tmp");

    let mut pages = Vec::with_capacity(doc.pages.len());
    for page in &doc.pages {
        let rel = format!("pages/{}.png", page.id);
        if !page.disk_path.is_file() && page.image.is_none() {
            return Err(format!("页 {} 既无磁盘备份也无内存图", page.id));
        }
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

    let bg_meta = if doc.bg_enabled {
        if let Some(c) = doc.bg_solid {
            Some(ProjectBg {
                enabled: true,
                aspect_w: doc.bg_aspect_w,
                aspect_h: doc.bg_aspect_h,
                image: String::new(),
                source_path: None,
                solid_rgb: Some(c),
            })
        } else if doc.bg_image.is_some() {
            Some(ProjectBg {
                enabled: true,
                aspect_w: doc.bg_aspect_w,
                aspect_h: doc.bg_aspect_h,
                image: "bg.png".into(),
                source_path: doc.bg_source_path.as_ref().map(|p| p.display().to_string()),
                solid_rgb: None,
            })
        } else {
            None
        }
    } else {
        None
    };

    let video = ProjectVideo {
        video_clips: doc
            .video_state
            .video_clips
            .iter()
            .map(|(group_id, start, end)| ProjectVideoClip {
                group_id: group_id.clone(),
                start: *start,
                end: *end,
            })
            .collect(),
        fades: doc
            .video_state
            .fades
            .iter()
            .map(|(start, end, kind, keep_bg)| ProjectFadeSpan {
                start: *start,
                end: *end,
                fade_in: matches!(kind, FadeKind::In),
                keep_bg: *keep_bg,
                kind: match kind {
                    FadeKind::WipeLtr => Some("wipe_ltr".into()),
                    _ => None,
                },
            })
            .collect(),
        audio_clips: doc
            .video_state
            .audio_clips
            .iter()
            .map(|(path, label, duration, offset)| ProjectAudioClip {
                path: path.display().to_string(),
                label: label.clone(),
                duration: *duration,
                offset: *offset,
            })
            .collect(),
        playhead: doc.video_state.playhead,
    };

    let meta = ProjectFile {
        version: PROJECT_VERSION,
        margin: doc.margin,
        ink_threshold: doc.ink_threshold,
        staff_grouping: doc.staff_grouping,
        mask_opacity: doc.mask_prefs.mask_opacity,
        mask_prefs: Some(doc.mask_prefs.clone()),
        current_page_index: doc
            .current_page_index
            .min(doc.pages.len().saturating_sub(1)),
        active_group_id: doc.active_group_id.clone(),
        selected_region_ids: selected,
        pages,
        groups,
        group_masks: doc.group_masks.clone(),
        group_block_layout: doc
            .group_block_layout
            .iter()
            .map(|(gid, v)| {
                (
                    gid.clone(),
                    v.iter().map(ProjectBlockAdjust::from).collect(),
                )
            })
            .collect(),
        group_voff_shift: doc.group_voff_shift.clone(),
        group_guides: doc.group_guides.clone(),
        guides_global: doc.guides_global,
        guides_sync_positions: doc.guides_sync_positions,
        groups_manual_order: doc.groups_manual_order,
        bg: bg_meta,
        video,
        region_edits: doc
            .region_edits
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    ProjectRegionEdit {
                        canvas_w: v.canvas_w,
                        canvas_h: v.canvas_h,
                        paper_rgb: v.paper_rgb,
                        source: v.source.clone(),
                    },
                )
            })
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&meta).map_err(|e| format!("序列化工程失败: {e}"))?;

    {
        let file = File::create(&tmp_path).map_err(|e| format!("创建临时工程失败: {e}"))?;
        let mut zip = ZipWriter::new(file);
        // JSON 可压; PNG 已是压缩格式, Stored 更快且几乎不占额外内存
        let json_opts =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let png_opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        zip.start_file("project.json", json_opts)
            .map_err(|e| format!("写入 project.json 失败: {e}"))?;
        zip.write_all(&json)
            .map_err(|e| format!("写入 project.json 失败: {e}"))?;
        // json 缓冲可先释放
        drop(json);

        for page in &doc.pages {
            let rel = format!("pages/{}.png", page.id);
            zip.start_file(&rel, png_opts)
                .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
            if page.disk_path.is_file() {
                copy_file_into_zip(&mut zip, &page.disk_path)?;
            } else if let Some(img) = page.image.as_ref() {
                let png =
                    encode_png(img).map_err(|e| format!("保存页图失败 ({}): {e}", page.id))?;
                zip.write_all(&png)
                    .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
            } else {
                return Err(format!("页 {} 既无磁盘备份也无内存图", page.id));
            }
        }

        for (rid, _) in &doc.region_edits {
            let dir = crate::model::DocState::region_edit_dir(rid);
            if !dir.is_dir() {
                continue;
            }
            for name in ["manifest.json", "flat.png"] {
                let p = dir.join(name);
                if p.is_file() {
                    let rel = format!("edits/{rid}/{name}");
                    zip.start_file(&rel, png_opts)
                        .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
                    copy_file_into_zip(&mut zip, &p)?;
                }
            }
            let layer_dir = dir.join("layers");
            if let Ok(rd) = std::fs::read_dir(&layer_dir) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if !p.is_file() {
                        continue;
                    }
                    let fname = ent.file_name().to_string_lossy().into_owned();
                    let rel = format!("edits/{rid}/layers/{fname}");
                    zip.start_file(&rel, png_opts)
                        .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
                    copy_file_into_zip(&mut zip, &p)?;
                }
            }
        }

        if doc.bg_enabled {
            if let Some(src) = doc.bg_source_path.as_ref().filter(|p| p.is_file()) {
                zip.start_file("bg.png", png_opts)
                    .map_err(|e| format!("写入 bg.png 失败: {e}"))?;
                copy_file_into_zip(&mut zip, src)?;
            } else if let Some(img) = doc.bg_image.as_ref() {
                let png = encode_png(img).map_err(|e| format!("编码底色失败: {e}"))?;
                zip.start_file("bg.png", png_opts)
                    .map_err(|e| format!("写入 bg.png 失败: {e}"))?;
                zip.write_all(&png)
                    .map_err(|e| format!("写入 bg.png 失败: {e}"))?;
            }
        }

        zip.finish()
            .map_err(|e| format!("完成工程压缩包失败: {e}"))?;
    }

    if project_path.exists() {
        std::fs::remove_file(&project_path).map_err(|e| format!("覆盖旧工程失败: {e}"))?;
    }
    std::fs::rename(&tmp_path, &project_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("写出工程文件失败: {e}")
    })?;
    Ok(project_path)
}

fn extract_edit_entries(zip: &mut ZipArchive<File>, session: &Path) -> Result<(), String> {
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let e = zip.by_index(i).ok()?;
            let n = e.name().replace('\\', "/");
            if n.starts_with("edits/") && !n.ends_with('/') {
                Some(n)
            } else {
                None
            }
        })
        .collect();
    for name in names {
        let bytes = read_zip_entry(zip, &name)?;
        let dest = session.join(&name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建 {name} 目录失败: {e}"))?;
        }
        std::fs::write(&dest, bytes).map_err(|e| format!("写出 {name} 失败: {e}"))?;
    }
    Ok(())
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
    let mut zip = ZipArchive::new(file)
        .map_err(|e| format!("无法作为工程压缩包打开 (是否为旧版旁路目录工程?): {e}"))?;

    let json_bytes = read_zip_entry(&mut zip, "project.json")?;
    let meta: ProjectFile =
        serde_json::from_slice(&json_bytes).map_err(|e| format!("解析 project.json 失败: {e}"))?;
    if meta.version == 0 || meta.version > PROJECT_VERSION {
        return Err(format!(
            "不支持的工程版本 {} (当前支持 1–{PROJECT_VERSION})",
            meta.version
        ));
    }

    let mut pages = Vec::with_capacity(meta.pages.len());
    // 解压到会话 tmp, 不一次全解码进内存
    let session = crate::page_cache::session_dir();
    for p in &meta.pages {
        let png = read_zip_entry(&mut zip, &p.image)?;
        let disk_path = session.join(format!("proj_{}.png", p.id));
        std::fs::write(&disk_path, &png)
            .map_err(|e| format!("写出页图到会话目录失败 ({}): {e}", p.id))?;
        let (w, h) = image::image_dimensions(&disk_path)
            .map_err(|e| format!("读取页尺寸失败 ({}): {e}", p.id))?;
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
            disk_path,
            image: None,
            img_w: w,
            img_h: h,
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
        staff_grouping: meta.staff_grouping,
        group_masks: meta.group_masks,
        group_block_layout: meta
            .group_block_layout
            .into_iter()
            .map(|(gid, v)| (gid, v.into_iter().map(BlockAdjust::from).collect()))
            .collect(),
        group_voff_shift: meta.group_voff_shift,
        group_guides: meta.group_guides,
        guides_global: meta.guides_global,
        guides_sync_positions: meta.guides_sync_positions,
        group_guide_defaults: HashMap::new(),
        region_staff_anchors: HashMap::new(),
        mask_prefs: meta
            .mask_prefs
            .unwrap_or_else(|| mask_tool::color_prefs::MaskColorPrefs {
                mask_opacity: meta.mask_opacity,
                ..Default::default()
            })
            .clamp(),
        // 工程文件内的 groups 顺序即导出顺序
        groups_manual_order: true,
        bg_enabled: false,
        bg_image: None,
        bg_solid: None,
        bg_source_path: None,
        bg_aspect_w: 2560,
        bg_aspect_h: 1440,
        bg_gen: 0,
        video_state: score_video::model::TimelineSnapshot {
            video_clips: meta
                .video
                .video_clips
                .into_iter()
                .map(|c| (c.group_id, c.start, c.end))
                .collect(),
            fades: meta
                .video
                .fades
                .into_iter()
                .map(|f| {
                    let kind = match f.kind.as_deref() {
                        Some("wipe_ltr") => FadeKind::WipeLtr,
                        _ if f.fade_in => FadeKind::In,
                        _ => FadeKind::Out,
                    };
                    (f.start, f.end, kind, f.keep_bg)
                })
                .collect(),
            audio_clips: meta
                .video
                .audio_clips
                .into_iter()
                .map(|c| (PathBuf::from(c.path), c.label, c.duration, c.offset))
                .collect(),
            playhead: meta.video.playhead,
        },
        rid_page: HashMap::new(),
        display_max_side: 0,
        region_edits: HashMap::new(),
    };
    for (rid, e) in meta.region_edits {
        doc.region_edits.insert(
            rid,
            crate::model::RegionEditMeta {
                canvas_w: e.canvas_w,
                canvas_h: e.canvas_h,
                paper_rgb: e.paper_rgb,
                source: e.source,
            },
        );
    }
    if let Some(bg) = meta.bg {
        if bg.enabled {
            doc.bg_aspect_w = bg.aspect_w.max(1);
            doc.bg_aspect_h = bg.aspect_h.max(1);
            doc.bg_source_path = bg.source_path.map(PathBuf::from);
            if let Some(c) = bg.solid_rgb {
                doc.bg_solid = Some(c);
                doc.bg_enabled = true;
            } else if !bg.image.is_empty() {
                let png = read_zip_entry(&mut zip, &bg.image)?;
                let image = image::load_from_memory(&png)
                    .map_err(|e| format!("解码底色失败: {e}"))?
                    .to_rgb8();
                if doc.bg_source_path.is_none() && image.width() > 0 && image.height() > 0 {
                    let p = image.get_pixel(image.width() / 2, image.height() / 2);
                    doc.bg_solid = Some([p[0], p[1], p[2]]);
                    doc.bg_enabled = true;
                } else {
                    doc.bg_image = Some(Arc::new(image));
                    doc.bg_enabled = true;
                }
            }
        }
    }
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
    extract_edit_entries(&mut zip, &session)?;
    doc.retain_memory_window();
    doc.rebuild_rid_index();
    doc.seed_guide_defaults();
    Ok(doc)
}
