//! 谱面加底色并按指定比例裁切 (宽=谱面宽) — 共享处理逻辑.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use image::imageops;
use image::{DynamicImage, RgbImage};
use rayon::prelude::*;

/// 默认裁切比例 (16:9).
pub const DEFAULT_ASPECT_W: u32 = 2560;
pub const DEFAULT_ASPECT_H: u32 = 1440;
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"];

/// 解析 "2560:1440" / "2560x1440" / "16/9" 等.
pub fn parse_aspect(s: &str) -> Result<(u32, u32), String> {
    let s = s
        .trim()
        .replace('：', ":")
        .replace('×', "x")
        .replace('Ｘ', "x");
    let parts: Vec<&str> = s
        .split(|c: char| matches!(c, ':' | 'x' | 'X' | '/' | ' '))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 2 {
        return Err(format!("比例格式无效: {s} (应为 宽:高, 如 {DEFAULT_ASPECT_W}:{DEFAULT_ASPECT_H})"));
    }
    let w: u32 = parts[0]
        .parse()
        .map_err(|_| format!("比例宽度无效: {}", parts[0]))?;
    let h: u32 = parts[1]
        .parse()
        .map_err(|_| format!("比例高度无效: {}", parts[1]))?;
    if w == 0 || h == 0 {
        return Err("比例宽高必须为正整数".into());
    }
    Ok((w, h))
}

pub fn format_aspect(w: u32, h: u32) -> String {
    format!("{w}:{h}")
}

#[derive(Debug, Clone)]
pub struct ProcessError {
    pub name: String,
    pub message: String,
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub ok: usize,
    pub errors: Vec<ProcessError>,
    pub elapsed_secs: f64,
    pub out_dir: PathBuf,
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

pub fn list_images(folder: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(folder)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();

    files.sort_by(|a, b| {
        let key = |p: &Path| {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if let Ok(n) = stem.parse::<u64>() {
                (0u8, n, p.file_name().map(|x| x.to_os_string()))
            } else {
                (1u8, 0, p.file_name().map(|x| x.to_os_string()))
            }
        };
        key(a).cmp(&key(b))
    });
    Ok(files)
}

/// 谱面居中叠在底色上, 再按宽=谱面宽、高=宽×aspect_h/aspect_w 居中裁切.
/// 只构造裁切区域大小的画布, 不复制整幅底色.
pub fn composite_and_crop(
    sheet: &RgbImage,
    bg: &RgbImage,
    aspect_w: u32,
    aspect_h: u32,
) -> Result<RgbImage, String> {
    if aspect_w == 0 || aspect_h == 0 {
        return Err("比例宽高必须为正整数".into());
    }
    let (sw, sh) = sheet.dimensions();
    let (bw, bh) = bg.dimensions();

    if bw < sw || bh < sh {
        return Err(format!("底色 ({bw}x{bh}) 无法完全盖住谱面 ({sw}x{sh})"));
    }

    let ox = ((bw - sw) / 2) as i64;
    let oy = ((bh - sh) / 2) as i64;

    let crop_w = sw;
    let crop_h = ((sw as f64) * (aspect_h as f64) / (aspect_w as f64)).round() as u32;
    if crop_h < 1 {
        return Err("计算出的裁切高度无效".into());
    }
    if crop_h > bh || crop_w > bw {
        return Err(format!("裁切区域 {crop_w}x{crop_h} 超出底色 {bw}x{bh}"));
    }

    let cx = (bw / 2) as i64;
    let cy = (bh / 2) as i64;
    let mut left = cx - (crop_w / 2) as i64;
    let mut top = cy - (crop_h / 2) as i64;
    let mut right = left + crop_w as i64;
    let mut bottom = top + crop_h as i64;

    if left < 0 {
        right -= left;
        left = 0;
    }
    if top < 0 {
        bottom -= top;
        top = 0;
    }
    if right > bw as i64 {
        left -= right - bw as i64;
        right = bw as i64;
    }
    if bottom > bh as i64 {
        top -= bottom - bh as i64;
        bottom = bh as i64;
    }

    let left_u = left as u32;
    let top_u = top as u32;
    let mut canvas =
        imageops::crop_imm(bg, left_u, top_u, (right - left) as u32, (bottom - top) as u32)
            .to_image();
    imageops::overlay(&mut canvas, sheet, ox - left, oy - top);
    Ok(canvas)
}

/// 谱面居中叠底色的"预览"版本 (蒙版/视频面板用): 不做最终裁切, 只在目标比例
/// 需要比谱面更高的画布时, 于上下补出底色, 让预览与最终导出的底色效果趋近一致;
/// 若目标比例会裁边 (裁切高度 <= 谱面高度) 则直接返回原图, 不改变画布尺寸,
/// 从而保证蒙版坐标系与 `compose_group` 输出保持一致 (宽度始终等于谱面宽度).
/// 返回 (预览图, 谱面在预览图中的纵向偏移量).
pub fn composite_preview(
    sheet: &RgbImage,
    bg: &RgbImage,
    aspect_w: u32,
    aspect_h: u32,
) -> Result<(RgbImage, i64), String> {
    if aspect_w == 0 || aspect_h == 0 {
        return Err("比例宽高必须为正整数".into());
    }
    let (sw, sh) = sheet.dimensions();
    let (bw, bh) = bg.dimensions();
    if bw < sw || bh < sh {
        return Err(format!("底色 ({bw}x{bh}) 无法完全盖住谱面 ({sw}x{sh})"));
    }

    let crop_h = ((sw as f64) * (aspect_h as f64) / (aspect_w as f64)).round() as u32;
    if crop_h <= sh || crop_h > bh {
        // 会裁边 (或底色不够高无法补边预览): 预览不改变画布, 与拼合图一致.
        return Ok((sheet.clone(), 0));
    }

    let ox = ((bw - sw) / 2) as i64;
    let oy = ((bh - sh) / 2) as i64;
    let cy = (bh / 2) as i64;
    let mut top = cy - (crop_h / 2) as i64;
    if top < 0 {
        top = 0;
    }
    if top + crop_h as i64 > bh as i64 {
        top = bh as i64 - crop_h as i64;
    }
    let voff = oy - top;

    let mut canvas = imageops::crop_imm(bg, ox as u32, top as u32, sw, crop_h).to_image();
    imageops::overlay(&mut canvas, sheet, 0, voff);
    Ok((canvas, voff))
}

fn process_one(
    path: &Path,
    bg: &RgbImage,
    out_dir: &Path,
    aspect_w: u32,
    aspect_h: u32,
) -> Result<(), ProcessError> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let sheet = image::open(path)
        .map_err(|e| ProcessError {
            name: name.clone(),
            message: e.to_string(),
        })?
        .to_rgb8();

    let out = composite_and_crop(&sheet, bg, aspect_w, aspect_h).map_err(|message| ProcessError {
        name: name.clone(),
        message,
    })?;

    let dest = out_dir.join(&name);
    DynamicImage::ImageRgb8(out)
        .save(&dest)
        .map_err(|e| ProcessError {
            name,
            message: e.to_string(),
        })?;
    Ok(())
}

/// `progress(done, total, name)` 在每张完成后回调 (可并行乱序).
pub fn process_folder(
    in_dir: &Path,
    bg_path: &Path,
    out_dir: &Path,
    aspect_w: u32,
    aspect_h: u32,
    jobs: Option<usize>,
    progress: impl Fn(usize, usize, &str) + Send + Sync + 'static,
) -> Result<ProcessResult, String> {
    if aspect_w == 0 || aspect_h == 0 {
        return Err("比例宽高必须为正整数".into());
    }
    if let Some(j) = jobs {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(j.max(1))
            .build_global();
    }

    if !in_dir.is_dir() {
        return Err(format!("输入目录无效: {}", in_dir.display()));
    }
    if !bg_path.is_file() {
        return Err(format!("底色不存在: {}", bg_path.display()));
    }

    let files = list_images(in_dir).map_err(|e| format!("无法读取目录: {e}"))?;
    if files.is_empty() {
        return Err("输入目录没有图片.".into());
    }

    fs::create_dir_all(out_dir).map_err(|e| format!("无法创建输出目录: {e}"))?;

    let t0 = Instant::now();
    let bg = Arc::new(
        image::open(bg_path)
            .map_err(|e| format!("无法打开底色: {e}"))?
            .to_rgb8(),
    );

    let done = AtomicUsize::new(0);
    let total = files.len();
    let out_dir_arc = Arc::new(out_dir.to_path_buf());
    let progress = Arc::new(progress);

    let results: Vec<Result<(), ProcessError>> = files
        .par_iter()
        .map(|path| {
            let r = process_one(path, &bg, &out_dir_arc, aspect_w, aspect_h);
            let i = done.fetch_add(1, Ordering::Relaxed) + 1;
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            progress(i, total, &name);
            r
        })
        .collect();

    let mut ok = 0usize;
    let mut errors: Vec<ProcessError> = Vec::new();
    for r in results {
        match r {
            Ok(()) => ok += 1,
            Err(e) => errors.push(e),
        }
    }

    Ok(ProcessResult {
        ok,
        errors,
        elapsed_secs: t0.elapsed().as_secs_f64(),
        out_dir: out_dir.to_path_buf(),
    })
}
