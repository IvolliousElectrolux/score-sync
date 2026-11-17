//! 工程文件: 单个 `*.staffcrop` (= zip), 内含 `project.json` + `pages/*.png`.
//!
//! 保存时把当前内存中的页图打进压缩包, PDF 临时页也能跨会话恢复.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use image::ImageFormat;
use mask_tool::mask::MaskRect;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::model::{BlockAdjust, DocState, Group, Page, Region, SheetLayout};
use crate::staff_detect::{StaffGrouping, StaffPackMode};
use score_video::model::FadeKind;

pub const PROJECT_EXT: &str = "staffcrop";
pub const PROJECT_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    /// 缺省为谱表分块. 对开工程写成 2, 旧程序会拒绝打开.
    #[serde(default)]
    sheet_layout: SheetLayout,
    margin: i32,
    ink_threshold: i32,
    #[serde(default)]
    staff_grouping: StaffGrouping,
    /// 一次收几行谱表. 缺省 / 小于 2 = 一行一块.
    #[serde(default)]
    staff_pack_lines: u32,
    #[serde(default)]
    staff_pack_mode: StaffPackMode,
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
    #[serde(default)]
    kind: crate::model::EditKind,
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
    /// `"wipe_ltr"` = 左→右刷入, `"dissolve"` = 交叠; 缺省则按 `fade_in` 当淡入/淡出.
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
    /// zip 内相对路径, 如 `pages/abcd.png`. 矢量页为空.
    #[serde(default)]
    image: String,
    regions: Vec<ProjectRegion>,
    /// `"pt"` 表示区域坐标是 point. 缺省为像素.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    coord: String,
    /// zip 内 PDF, 如 `pdfs/abcd.pdf`. 同一文件多页只存一份.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pdf: String,
    #[serde(default)]
    pdf_page: u32,
    #[serde(default)]
    w_pt: f32,
    #[serde(default)]
    h_pt: f32,
    /// 对开空位. 不带页图, 和奇数末页旁边的空页一样.
    #[serde(default)]
    blank: bool,
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
    #[serde(default)]
    shift_y: i32,
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
            shift_y: a.shift_y,
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
            shift_y: a.shift_y,
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

fn sanitize_zip_name(rel: &str) -> String {
    rel.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
    let mut pdf_rels: HashMap<PathBuf, String> = HashMap::new();
    for page in &doc.pages {
        let (image, coord, pdf, pdf_page, w_pt, h_pt) = if let Some(src) = page.vector.as_ref() {
            let rel = if let Some(rel) = pdf_rels.get(&src.pdf_path) {
                rel.clone()
            } else {
                let rel = format!("pdfs/{}.pdf", page.id);
                pdf_rels.insert(src.pdf_path.clone(), rel.clone());
                rel
            };
            (
                String::new(),
                "pt".to_string(),
                rel,
                src.page_index,
                src.w_pt,
                src.h_pt,
            )
        } else if page.blank {
            (String::new(), String::new(), String::new(), 0, 0.0, 0.0)
        } else {
            if !page.disk_path.is_file() && page.image.is_none() {
                return Err(format!("页 {} 既无磁盘备份也无内存图", page.id));
            }
            (
                format!("pages/{}.png", page.id),
                String::new(),
                String::new(),
                0,
                0.0,
                0.0,
            )
        };
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
            image,
            regions,
            coord,
            pdf,
            pdf_page,
            w_pt,
            h_pt,
            blank: page.blank,
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
                    FadeKind::Dissolve => Some("dissolve".into()),
                    FadeKind::In | FadeKind::Out => None,
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
        sheet_layout: doc.sheet_layout,
        margin: doc.margin,
        ink_threshold: doc.ink_threshold,
        staff_grouping: doc.staff_grouping,
        staff_pack_lines: doc.staff_pack_lines,
        staff_pack_mode: doc.staff_pack_mode,
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
                        kind: v.kind,
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

        let mut written_pdfs: HashSet<String> = HashSet::new();
        for page in &doc.pages {
            if page.blank {
                continue;
            }
            if let Some(src) = page.vector.as_ref() {
                let Some(rel) = pdf_rels.get(&src.pdf_path) else {
                    continue;
                };
                if !written_pdfs.insert(rel.clone()) {
                    continue;
                }
                if !src.pdf_path.is_file() {
                    return Err(format!("矢量页缺少 PDF ({})", src.pdf_path.display()));
                }
                zip.start_file(rel, png_opts)
                    .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
                copy_file_into_zip(&mut zip, &src.pdf_path)?;
                continue;
            }
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
            for name in ["manifest.json", "flat.png", "preview.png"] {
                let p = dir.join(name);
                if p.is_file() {
                    let rel = format!("edits/{rid}/{name}");
                    zip.start_file(&rel, png_opts)
                        .map_err(|e| format!("写入 {rel} 失败: {e}"))?;
                    copy_file_into_zip(&mut zip, &p)?;
                }
            }
            for sub in ["layers", "images"] {
                let sub_dir = dir.join(sub);
                let Ok(rd) = std::fs::read_dir(&sub_dir) else {
                    continue;
                };
                for ent in rd.flatten() {
                    let p = ent.path();
                    if !p.is_file() {
                        continue;
                    }
                    let fname = ent.file_name().to_string_lossy().into_owned();
                    let rel = format!("edits/{rid}/{sub}/{fname}");
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

fn page_is_blank(p: &ProjectPage) -> bool {
    let title_stem = std::path::Path::new(&p.title)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(p.title.as_str());
    p.blank
        || title_stem
            .split("_copy")
            .next()
            .unwrap_or(title_stem)
            .starts_with("blank_")
}

fn regions_of(p: &ProjectPage) -> HashMap<String, Region> {
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
    regions
}

fn display_path_of(p: &ProjectPage) -> PathBuf {
    if p.title.is_empty() {
        PathBuf::from(if p.image.is_empty() { "page" } else { &p.image })
    } else {
        PathBuf::from(&p.title)
    }
}

fn write_bytes(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let file = File::create(dest).map_err(|e| format!("写出 {} 失败: {e}", dest.display()))?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(bytes)
        .map_err(|e| format!("写出 {} 失败: {e}", dest.display()))?;
    writer
        .flush()
        .map_err(|e| format!("写出 {} 失败: {e}", dest.display()))?;
    Ok(())
}

fn read_zip_file_entry(zip_path: &Path, name: &str) -> Result<Vec<u8>, String> {
    let file = File::open(zip_path).map_err(|e| format!("打开工程失败: {e}"))?;
    let mut zip = ZipArchive::new(file).map_err(|e| format!("无法作为工程压缩包打开: {e}"))?;
    read_zip_entry(&mut zip, name)
}

fn edit_entry_names(zip: &mut ZipArchive<File>) -> Vec<String> {
    (0..zip.len())
        .filter_map(|i| {
            let entry = zip.by_index(i).ok()?;
            let name = entry.name().replace('\\', "/");
            if name.starts_with("edits/") && !name.ends_with('/') {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

fn parallel_map<T, R>(items: &[T], conc: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    if items.is_empty() {
        return Vec::new();
    }
    let n = items.len();
    let conc = conc.max(1).min(n);
    let slots: Vec<std::sync::Mutex<Option<R>>> =
        (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..conc {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let value = f(&items[i]);
                *slots[i].lock().unwrap_or_else(|err| err.into_inner()) = Some(value);
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|err| err.into_inner())
                .expect("parallel slot")
        })
        .collect()
}

fn assemble_pages(
    zip_path: &Path,
    meta_pages: &[ProjectPage],
    session: &Path,
    report: &(impl Fn(LoadProgress) + Sync),
) -> Result<Vec<Page>, String> {
    let total = meta_pages.iter().filter(|p| !page_is_blank(p)).count();
    report(LoadProgress {
        stage: "解压页面",
        done: 0,
        total,
    });
    let done = AtomicUsize::new(0);
    let conc = crate::page_cache::concurrency_for_peak(8 * 1024 * 1024);
    let mut pdf_order = Vec::new();
    let mut pdf_pages: HashMap<String, Vec<u32>> = HashMap::new();
    for page in meta_pages {
        if page_is_blank(page) || page.coord != "pt" || page.pdf.is_empty() {
            continue;
        }
        if !pdf_pages.contains_key(&page.pdf) {
            pdf_order.push(page.pdf.clone());
        }
        let pages = pdf_pages.entry(page.pdf.clone()).or_default();
        if !pages.contains(&page.pdf_page) {
            pages.push(page.pdf_page);
        }
    }
    let pdf_jobs: Vec<(String, PathBuf, Vec<u32>)> = pdf_order
        .into_iter()
        .map(|name| {
            let dest = session.join(format!("proj_{}.pdf", sanitize_zip_name(&name)));
            let pages = pdf_pages.remove(&name).unwrap_or_default();
            (name, dest, pages)
        })
        .collect();
    let pdf_loaded = parallel_map(&pdf_jobs, conc, |job| {
        let (name, dest, pages) = job;
        let bytes = read_zip_file_entry(zip_path, name)?;
        write_bytes(dest, &bytes)?;
        let sources = crate::pdf::load_vector_sources(dest, pages)
            .map_err(|e| format!("读取矢量页失败 ({name}): {e}"))?;
        let n = pages.len();
        let finished = done.fetch_add(n, Ordering::Relaxed) + n;
        report(LoadProgress {
            stage: "解压页面",
            done: finished.min(total),
            total,
        });
        let mut by_page = HashMap::new();
        for source in sources {
            by_page.insert(source.page_index, source);
        }
        Ok::<_, String>((name.clone(), by_page))
    });
    let mut pdf_sources = HashMap::new();
    for item in pdf_loaded {
        let (name, by_page) = item?;
        pdf_sources.insert(name, by_page);
    }

    struct BitmapJob {
        index: usize,
        image: String,
        dest: PathBuf,
    }
    let bitmap_jobs: Vec<BitmapJob> = meta_pages
        .iter()
        .enumerate()
        .filter_map(|(index, page)| {
            if page_is_blank(page) || (page.coord == "pt" && !page.pdf.is_empty()) {
                return None;
            }
            Some(BitmapJob {
                index,
                image: page.image.clone(),
                dest: session.join(format!("proj_{}.png", page.id)),
            })
        })
        .collect();
    let bitmap_loaded = parallel_map(&bitmap_jobs, conc, |job| {
        let bytes = read_zip_file_entry(zip_path, &job.image)?;
        let (w, h) = crate::page_cache::png_ihdr_dimensions(&bytes)
            .or_else(|| {
                image::load_from_memory(&bytes)
                    .ok()
                    .map(|image| (image.width(), image.height()))
            })
            .ok_or_else(|| format!("读取页尺寸失败 ({})", job.image))?;
        write_bytes(&job.dest, &bytes)?;
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        report(LoadProgress {
            stage: "解压页面",
            done: finished.min(total),
            total,
        });
        Ok::<_, String>((job.index, job.dest.clone(), w, h))
    });
    let mut bitmaps = HashMap::new();
    for item in bitmap_loaded {
        let (index, dest, w, h) = item?;
        bitmaps.insert(index, (dest, w, h));
    }

    let mut pages = Vec::with_capacity(meta_pages.len());
    for (index, page) in meta_pages.iter().enumerate() {
        if page_is_blank(page) {
            pages.push(Page {
                id: page.id.clone(),
                path: if page.title.is_empty() {
                    PathBuf::from(format!("blank_{}.png", page.id))
                } else {
                    PathBuf::from(&page.title)
                },
                disk_path: PathBuf::new(),
                image: None,
                img_w: 1,
                img_h: 1,
                regions: HashMap::new(),
                vector: None,
                blank: true,
            });
            continue;
        }
        let vector = if page.coord == "pt" && !page.pdf.is_empty() {
            Some(
                pdf_sources
                    .get(&page.pdf)
                    .and_then(|by_page| by_page.get(&page.pdf_page))
                    .cloned()
                    .ok_or_else(|| format!("读取矢量页失败 ({})", page.id))?,
            )
        } else {
            None
        };
        let (disk_path, w, h) = if let Some(source) = vector.as_ref() {
            let (w, h) = source.logical_size();
            (PathBuf::new(), w, h)
        } else {
            bitmaps
                .remove(&index)
                .ok_or_else(|| format!("缺少页图 {}", page.id))?
        };
        pages.push(Page {
            id: page.id.clone(),
            path: display_path_of(page),
            disk_path,
            image: None,
            img_w: w,
            img_h: h,
            regions: regions_of(page),
            vector,
            blank: false,
        });
    }
    Ok(pages)
}

fn extract_named_entries(
    zip_path: &Path,
    session: &Path,
    names: &[String],
    report: &(impl Fn(LoadProgress) + Sync),
) -> Result<(), String> {
    if names.is_empty() {
        return Ok(());
    }
    let total = names.len();
    report(LoadProgress {
        stage: "解压修图",
        done: 0,
        total,
    });
    let done = AtomicUsize::new(0);
    let conc = crate::page_cache::concurrency_for_peak(1024 * 1024);
    let results = parallel_map(names, conc, |name| {
        let bytes = read_zip_file_entry(zip_path, name)?;
        let dest = session.join(name);
        write_bytes(&dest, &bytes)?;
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        if finished == total || finished % 16 == 0 {
            report(LoadProgress {
                stage: "解压修图",
                done: finished,
                total,
            });
        }
        Ok::<_, String>(())
    });
    for result in results {
        result?;
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

/// 打开工程时回传给界面的阶段进度. `done` / `total` 为 0 表示这一阶段没有计数.
#[derive(Clone, Debug)]
pub struct LoadProgress {
    pub stage: &'static str,
    pub done: usize,
    pub total: usize,
}

/// 读取工程 zip, 返回完整 DocState (不重新识别).
pub fn load_project(path: &Path) -> Result<DocState, String> {
    load_project_reporting(path, |_| {})
}

/// 同 [`load_project`], 并在解压页、修图和底色时回调进度.
/// 回调可能来自工作线程, 必须自己同步.
pub fn load_project_reporting(
    path: &Path,
    report: impl Fn(LoadProgress) + Send + Sync,
) -> Result<DocState, String> {
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

    let started = Instant::now();
    let session = crate::page_cache::session_dir();
    report(LoadProgress {
        stage: "读取工程",
        done: 0,
        total: meta.pages.len(),
    });
    let edit_names = edit_entry_names(&mut zip);
    let bg_png = match meta.bg.as_ref() {
        Some(bg) if bg.enabled && bg.solid_rgb.is_none() && !bg.image.is_empty() => {
            Some(read_zip_entry(&mut zip, &bg.image)?)
        }
        _ => None,
    };
    drop(zip);
    crate::trace::log(&format!(
        "load_project: project.json {} 页, {}ms",
        meta.pages.len(),
        started.elapsed().as_millis()
    ));
    let pages = assemble_pages(path, &meta.pages, &session, &report)?;
    crate::trace::log(&format!(
        "load_project: 解压页面 {}ms",
        started.elapsed().as_millis()
    ));

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
        staff_pack_lines: meta.staff_pack_lines,
        staff_pack_mode: meta.staff_pack_mode,
        staff_line_atoms: HashMap::new(),
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
        sheet_layout: meta.sheet_layout,
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
                        Some("dissolve") => FadeKind::Dissolve,
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
                kind: e.kind,
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
                let png = bg_png.unwrap_or_default();
                let image = image::load_from_memory(&png)
                    .map_err(|e| format!("解码底色失败: {e}"))?
                    .to_rgb8();
                // 纯色只认 solid_rgb. 没有 source_path 的位图也保留,
                // 取中心像素会把纹理收成纯色.
                doc.bg_image = Some(Arc::new(image));
                doc.bg_enabled = true;
            }
        }
    }
    report(LoadProgress {
        stage: "整理工程",
        done: meta.pages.len(),
        total: meta.pages.len(),
    });
    crate::trace::log(&format!(
        "load_project: 底色与文档 {}ms",
        started.elapsed().as_millis()
    ));
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
    let edit_started = Instant::now();
    extract_named_entries(path, &session, &edit_names, &report)?;
    crate::trace::log(&format!(
        "load_project: 解压修图 {} 个, {}ms; 合计 {}ms (页图像素等打开后再按窗口解码)",
        edit_names.len(),
        edit_started.elapsed().as_millis(),
        started.elapsed().as_millis()
    ));
    doc.rebuild_rid_index();
    doc.seed_guide_defaults();
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::io::Write;

    fn write_version_zip(path: &Path, version: u32) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("project.json", opts).unwrap();
        let json = format!(
            r#"{{"version":{version},"margin":20,"ink_threshold":200,"mask_opacity":1.0,"current_page_index":0,"active_group_id":null,"selected_region_ids":[],"pages":[],"groups":[],"group_masks":{{}}}}"#
        );
        zip.write_all(json.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn rejects_unsupported_project_version() {
        let dir = std::env::temp_dir().join(format!(
            "score_sync_proj_ver_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.staffcrop");
        let future = dir.join("future.staffcrop");
        write_version_zip(&old, 0);
        write_version_zip(&future, PROJECT_VERSION + 1);
        match load_project(&old) {
            Err(e) => assert!(e.contains("不支持的工程版本"), "{e}"),
            Ok(_) => panic!("version 0 should be rejected"),
        }
        match load_project(&future) {
            Err(e) => assert!(e.contains("不支持的工程版本"), "{e}"),
            Ok(_) => panic!("future version should be rejected"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_keeps_edits_order_and_textured_bg_without_path() {
        crate::page_cache::init_session();
        let dir = std::env::temp_dir().join(format!(
            "score_sync_proj_rt_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let page_png = dir.join("page.png");
        RgbImage::from_pixel(8, 20, Rgb([255, 255, 255]))
            .save(&page_png)
            .unwrap();

        let page_id = "page-a".to_string();
        let mut regions = HashMap::new();
        regions.insert(
            "r-rt".into(),
            Region {
                id: "r-rt".into(),
                page_id: page_id.clone(),
                y0: 0,
                y1: 9,
                kind: "manual".into(),
                color: "#e74c3c".into(),
            },
        );
        let mut doc = DocState::new();
        doc.groups_manual_order = false;
        doc.pages.push(Page {
            id: page_id.clone(),
            path: PathBuf::from("page.png"),
            disk_path: page_png,
            image: None,
            img_w: 8,
            img_h: 20,
            regions,
            vector: None,
            blank: false,
        });
        doc.groups.push(Group {
            id: "g-late".into(),
            region_ids: vec!["r-rt".into()],
            name: String::new(),
        });
        doc.groups.insert(
            0,
            Group {
                id: "g-early".into(),
                region_ids: vec!["r-rt".into()],
                name: String::new(),
            },
        );
        doc.rebuild_rid_index();

        let mut edit = photo_edit::EditDocument::from_rgb(
            &RgbImage::from_pixel(8, 10, Rgb([240, 240, 230])),
            [240, 240, 230],
        );
        edit.source = Some(photo_edit::SourceFingerprint {
            page_id: page_id.clone(),
            y0: 0,
            y1: 9,
            w: 8,
            h: 10,
            ..Default::default()
        });
        doc.write_region_edit("r-rt", &edit).unwrap();

        let mut bg = RgbImage::new(4, 2);
        bg.put_pixel(0, 0, Rgb([10, 20, 30]));
        bg.put_pixel(3, 1, Rgb([200, 10, 10]));
        doc.set_project_bg_arc(Arc::new(bg), None, 16, 9).unwrap();

        let path = save_project(&doc, &dir.join("piece")).unwrap();
        let _ = std::fs::remove_dir_all(DocState::region_edit_dir("r-rt"));
        let loaded = load_project(&path).unwrap();

        assert!(loaded.groups_manual_order);
        assert_eq!(
            loaded
                .groups
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>(),
            vec!["g-early", "g-late"]
        );
        assert!(loaded.region_edit_applies("r-rt"));
        assert!(loaded.load_region_flat("r-rt").is_some());
        assert!(loaded.bg_solid.is_none());
        let img = loaded.bg_image.as_ref().unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30]);
        assert!(loaded.bg_source_path.is_none());

        let mut detached = loaded;
        detached.get_region_mut("r-rt").unwrap().y1 = 14;
        assert!(detached.region_edit_detached("r-rt"));
        assert!(!detached.region_edit_applies("r-rt"));
        assert!(detached.load_region_flat("r-rt").is_none());
        assert_eq!(detached.group_member_heights("g-early")[0].1, 15);
        detached.get_region_mut("r-rt").unwrap().y1 = 9;
        assert!(detached.region_edit_applies("r-rt"));
        assert!(detached.load_region_flat("r-rt").is_some());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(DocState::region_edit_dir("r-rt"));
    }

    #[test]
    fn version_1_opens_as_staff_and_spread_round_trips_shift_y() {
        let dir = std::env::temp_dir().join(format!(
            "score_sync_proj_spread_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let v1 = dir.join("v1.staffcrop");
        write_version_zip(&v1, 1);
        let loaded_v1 = load_project(&v1).unwrap();
        assert_eq!(loaded_v1.sheet_layout, SheetLayout::Staff);

        let page_png = dir.join("page.png");
        RgbImage::from_pixel(8, 20, Rgb([255, 255, 255]))
            .save(&page_png)
            .unwrap();
        let mut regions = HashMap::new();
        regions.insert(
            "r-spread".into(),
            Region {
                id: "r-spread".into(),
                page_id: "page-spread".into(),
                y0: 0,
                y1: 19,
                kind: "page".into(),
                color: "#e74c3c".into(),
            },
        );
        let mut doc = DocState::new();
        doc.sheet_layout = SheetLayout::Spread;
        doc.pages.push(Page {
            id: "page-spread".into(),
            path: PathBuf::from("page.png"),
            disk_path: page_png,
            image: None,
            img_w: 8,
            img_h: 20,
            regions,
            vector: None,
            blank: false,
        });
        doc.groups.push(Group {
            id: "g-spread".into(),
            region_ids: vec!["r-spread".into()],
            name: String::new(),
        });
        doc.group_block_layout.insert(
            "g-spread".into(),
            vec![BlockAdjust {
                region_id: "r-spread".into(),
                shift_y: 12,
                shift_x: -3,
                ..Default::default()
            }],
        );
        doc.rebuild_rid_index();
        let path = save_project(&doc, &dir.join("spread")).unwrap();

        let file = File::open(&path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut json = String::new();
        zip.by_name("project.json")
            .unwrap()
            .read_to_string(&mut json)
            .unwrap();
        let meta: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(meta["version"], 2);
        assert_eq!(meta["sheet_layout"], "spread");
        assert_eq!(meta["group_block_layout"]["g-spread"][0]["shift_y"], 12);

        let loaded = load_project(&path).unwrap();
        assert_eq!(loaded.sheet_layout, SheetLayout::Spread);
        let adj = &loaded.group_block_layout["g-spread"][0];
        assert_eq!(adj.shift_y, 12);
        assert_eq!(adj.shift_x, -3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
