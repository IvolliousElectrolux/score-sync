//! 谱面加底色并按指定比例裁切 — 谱面完整装进画布 (contain).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use image::{DynamicImage, RgbImage};
use rayon::prelude::*;

/// 默认裁切比例 (16:9).
pub const DEFAULT_ASPECT_W: u32 = 2560;
pub const DEFAULT_ASPECT_H: u32 = 1440;
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"];

#[derive(Debug, Clone, thiserror::Error)]
pub enum AspectError {
    #[error("比例格式无效: {0} (应为 宽:高, 如 2560:1440)")]
    Format(String),
    #[error("比例宽度无效: {0}")]
    Width(String),
    #[error("比例高度无效: {0}")]
    Height(String),
    #[error("比例宽高必须为正整数")]
    Zero,
}

/// 解析 "2560:1440" / "2560x1440" / "16/9" 等.
pub fn parse_aspect(s: &str) -> Result<(u32, u32), AspectError> {
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
        return Err(AspectError::Format(s));
    }
    let w: u32 = parts[0]
        .parse()
        .map_err(|_| AspectError::Width(parts[0].to_string()))?;
    let h: u32 = parts[1]
        .parse()
        .map_err(|_| AspectError::Height(parts[1].to_string()))?;
    if w == 0 || h == 0 {
        return Err(AspectError::Zero);
    }
    Ok((w, h))
}

pub fn format_aspect(w: u32, h: u32) -> String {
    format!("{w}:{h}")
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{name}: {message}")]
pub struct ProcessError {
    pub name: String,
    pub message: String,
}

impl ProcessError {
    pub fn new(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message: message.into(),
        }
    }

    pub fn folder(message: impl Into<String>) -> Self {
        Self::new("处理", message)
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

/// 把谱面完整装进目标比例的画布.
/// 谱面相对更宽 (装得下高度) → 宽=谱面宽, 上下补边;
/// 谱面相对更高 (按宽会对上下裁切) → 高=谱面高, 左右补边.
///
/// 蒙版预览/终稿叠底色不再走这个"变高就放大页面"的分支, 见 [`page_size`].
pub fn frame_size(sw: u32, sh: u32, aspect_w: u32, aspect_h: u32) -> (u32, u32) {
    let sw = sw.max(1);
    let sh = sh.max(1);
    let h_from_w = ((sw as f64) * (aspect_h as f64) / (aspect_w as f64)).round() as u32;
    if h_from_w >= sh {
        (sw, h_from_w.max(1))
    } else {
        let w_from_h = ((sh as f64) * (aspect_w as f64) / (aspect_h as f64)).round() as u32;
        (w_from_h.max(1), sh)
    }
}

/// 底色页面尺寸: 始终按谱面宽度定高 (`宽=谱面宽, 高=宽×比例`).
/// 谱面再高也不放大页面, 只缩小内部块装进去, 见 [`preview_frame`].
pub fn page_size(sw: u32, aspect_w: u32, aspect_h: u32) -> (u32, u32) {
    let sw = sw.max(1);
    if aspect_w == 0 || aspect_h == 0 {
        return (sw, 1);
    }
    let h = ((sw as f64) * (aspect_h as f64) / (aspect_w as f64)).round() as u32;
    (sw, h.max(1))
}

/// 完整底色上、按谱面宽与纵横比定下的目标页矩形 `(left, top, w, h)`.
/// 画布大小只跟谱面宽有关, 跟谱面高无关. 底色装不下该页时返回 `None`.
pub fn bg_page_rect(
    bw: u32,
    bh: u32,
    aspect_w: u32,
    aspect_h: u32,
    sheet_w: u32,
) -> Option<(u32, u32, u32, u32)> {
    if aspect_w == 0 || aspect_h == 0 || sheet_w == 0 {
        return None;
    }
    let (page_w, page_h) = page_size(sheet_w, aspect_w, aspect_h);
    if bw < page_w || bh < page_h {
        return None;
    }
    let (left, top, right, bottom) = clamp_centered_rect(bw, bh, page_w, page_h);
    Some((
        left.max(0) as u32,
        top.max(0) as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// 从完整底色备份裁出目标页 (恰好 [`page_size`] 那一块).
/// 绘制/贴图用这一块, 不要把整张扫描图送去缩放.
pub fn crop_bg_to_page(
    bg: &RgbImage,
    aspect_w: u32,
    aspect_h: u32,
    sheet_w: u32,
) -> Option<RgbImage> {
    let (left, top, w, h) = bg_page_rect(bg.width(), bg.height(), aspect_w, aspect_h, sheet_w)?;
    Some(crop_fast(bg, left, top, w, h))
}

fn clamp_centered_rect(bw: u32, bh: u32, crop_w: u32, crop_h: u32) -> (i64, i64, i64, i64) {
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
    (left, top, right, bottom)
}

/// 从 `src` 裁出 `(left, top)` 起 `w x h` 区域, 按行整块 `copy_from_slice`
/// 拷贝, 不用 `image::imageops::crop_imm().to_image()` (内部逐像素调用
/// get_pixel/put_pixel, 大图每帧都裁一次这样调用的开销很可观, 蒙版拖动
/// 分块时这里是热路径).
fn crop_fast(src: &RgbImage, left: u32, top: u32, w: u32, h: u32) -> RgbImage {
    let sw = src.width() as usize;
    let sh = src.height() as usize;
    let mut out = RgbImage::new(w, h);
    let ow = w as usize;
    let avail_w = sw.saturating_sub(left as usize).min(ow);
    let copy_w = avail_w * 3;
    // `ImageBuffer` 同时实现了 `Index<(u32,u32)>` 与 `Deref<Target=[u8]>`,
    // 直接用 range 下标会被解析成前者报类型不匹配, 需要先显式解引用成
    // 裸字节切片再按 range 切.
    let src_buf: &[u8] = src;
    let out_buf: &mut [u8] = &mut out;
    for row in 0..h as usize {
        let sy = top as usize + row;
        if sy >= sh {
            break;
        }
        let s0 = (sy * sw + left as usize) * 3;
        let d0 = row * ow * 3;
        out_buf[d0..d0 + copy_w].copy_from_slice(&src_buf[s0..s0 + copy_w]);
    }
    out
}

/// 把不透明的 `src` 整块贴到 `dst` 的 `(dx, dy)` 位置 (超出 `dst` 边界的
/// 部分自动裁掉), 按行整块拷贝, 替代 `image::imageops::overlay` (内部
/// 逐像素调用 blend; 这里两幅图都是不透明谱面/底色, 不需要按像素混合,
/// 直接覆盖即可, 同样是每帧都要跑一次的热路径).
///
/// `skip_top_rows`: `src` 最上面这么多行不贴 (让 `dst` 本身在这段的像素
/// 保留可见), 用于"谱面最前端人为拖出来的留白, 没有真实内容可言, 直接
/// 露出底色而不是贴一块自己的颜色"的场景, 见 [`composite_preview`]。
fn overlay_fast(dst: &mut RgbImage, src: &RgbImage, dx: i64, dy: i64, skip_top_rows: u32, skip_bottom_rows: u32) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let skip = (skip_top_rows as i64).min(sh);
    let skip_bot = (skip_bottom_rows as i64).min((sh - skip).max(0));
    let x0 = dx.max(0);
    let y0 = (dy + skip).max(0);
    let x1 = (dx + sw).min(dw);
    let y1 = (dy + sh - skip_bot).min(dh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let copy_w = (x1 - x0) as usize * 3;
    let dw_u = dw as usize;
    let sw_u = sw as usize;
    let src_buf: &[u8] = src;
    let dst_buf: &mut [u8] = dst;
    for y in y0..y1 {
        let sy = (y - dy) as usize;
        let sx0 = (x0 - dx) as usize;
        let s0 = (sy * sw_u + sx0) * 3;
        let d0 = (y as usize * dw_u + x0 as usize) * 3;
        dst_buf[d0..d0 + copy_w].copy_from_slice(&src_buf[s0..s0 + copy_w]);
    }
}

fn overlay_sheet(
    canvas: &mut RgbImage,
    sheet: &RgbImage,
    hoff: i64,
    voff: i64,
    top_transparent: u32,
    bottom_transparent: u32,
    content_scale: f32,
) {
    let skip = top_transparent.min(sheet.height());
    let skip_bot = bottom_transparent.min(sheet.height().saturating_sub(skip));
    if (content_scale - 1.0).abs() < 0.0001 {
        overlay_fast(canvas, sheet, hoff, voff, skip, skip_bot);
        return;
    }
    let vis_h = sheet.height().saturating_sub(skip).saturating_sub(skip_bot);
    if vis_h == 0 || sheet.width() == 0 {
        return;
    }
    let vis = crop_fast(sheet, 0, skip, sheet.width(), vis_h);
    let dw = ((vis.width() as f32) * content_scale).round().max(1.0) as u32;
    let dh = ((vis.height() as f32) * content_scale).round().max(1.0) as u32;
    let scaled = image::imageops::resize(&vis, dw, dh, image::imageops::FilterType::Triangle);
    let dy = voff + ((skip as f32) * content_scale).round() as i64;
    overlay_fast(canvas, &scaled, hoff, dy, 0, 0);
}

/// 谱面居中叠底色时不含任何手动偏移的"自然"纵向留白 (像素), 即
/// `composite_and_crop`/`composite_preview` 在 `voff_shift=0` 时会得到的
/// `voff`. 只依赖谱面/底色尺寸, 不需要像素数据, 可以在每次「组合分块」
/// 布局 (谱面高度) 变化时重新精确计算, 用来推导应该写入的
/// `voff_shift`: 按 [`page_size`] 重算. 谱面高于页面时不再放大画布,
/// 自然留白归零, 内容改为缩小装进页面.
pub fn natural_voff(sw: u32, sh: u32, bw: u32, bh: u32, aspect_w: u32, aspect_h: u32) -> i64 {
    if aspect_w == 0 || aspect_h == 0 {
        return 0;
    }
    let sw = sw.max(1);
    let sh = sh.max(1);
    let (page_w, page_h) = page_size(sw, aspect_w, aspect_h);
    if bw < page_w || bh < page_h || sh >= page_h {
        return 0;
    }
    let oy = ((bh - sh) / 2) as i64;
    let (_, top, _, bottom) = clamp_centered_rect(bw, bh, page_w, page_h);
    (oy - top).clamp(0, (bottom - top - sh as i64).max(0))
}

/// 蒙版预览画布的几何 (不含任何像素合成).
/// 拖动分块时每帧只需要这些数字来摆放已上传的分块贴图, 不必重切底色/
/// 重贴谱面.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewFrame {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub hoff: i64,
    pub voff: i64,
    /// 底色上取预览画布的左上角; `shows_bg` 为 false 时为 0.
    pub bg_left: u32,
    pub bg_top: u32,
    /// 是否真的叠了底色 (false 时画布就是谱面本身, 不要画底色贴图).
    pub shows_bg: bool,
    /// 谱面相对页面的缩放. 谱面高于 [`page_size`] 时 < 1, 只缩小内部块,
    /// 不放大底色页面.
    pub content_scale: f32,
}

/// 与 [`composite_preview`] / [`composite_and_crop`] 同一套页面几何,
/// 只算数字, 不碰像素. 画布始终按 [`page_size`] 定形.
pub fn preview_frame(
    sw: u32,
    sh: u32,
    bw: u32,
    bh: u32,
    aspect_w: u32,
    aspect_h: u32,
    voff_shift: i64,
) -> PreviewFrame {
    let sw = sw.max(1);
    let sh = sh.max(1);
    let fallback = PreviewFrame {
        canvas_w: sw,
        canvas_h: sh,
        hoff: 0,
        voff: 0,
        bg_left: 0,
        bg_top: 0,
        shows_bg: false,
        content_scale: 1.0,
    };
    if aspect_w == 0 || aspect_h == 0 {
        return fallback;
    }
    let Some((bg_left, bg_top, canvas_w, canvas_h)) =
        bg_page_rect(bw, bh, aspect_w, aspect_h, sw)
    else {
        return fallback;
    };
    let content_scale = if sh > canvas_h {
        canvas_h as f32 / sh as f32
    } else {
        1.0
    };
    let disp_w = ((sw as f32) * content_scale).round() as i64;
    let disp_h = ((sh as f32) * content_scale).round() as i64;
    let hoff = ((canvas_w as i64 - disp_w) / 2).max(0);
    let vmax = (canvas_h as i64 - disp_h).max(0);
    let voff = if content_scale < 1.0 {
        // 缩小后的谱面刚好装满页面高度; 顶端/底端留白靠透明区露出底色,
        // 不再用负 voff 把内容推出页面裁掉.
        0
    } else {
        let oy = ((bh as i64) - sh as i64) / 2;
        (oy - bg_top as i64 + voff_shift).clamp(0, vmax)
    };
    PreviewFrame {
        canvas_w,
        canvas_h,
        hoff,
        voff,
        bg_left,
        bg_top,
        shows_bg: true,
        content_scale,
    }
}

/// 谱面居中叠在底色上, 再按目标比例取一块完整装得下谱面的画布 (contain).
/// 只构造该区域大小, 不复制整幅底色. `voff_shift`: 相对默认垂直居中位置
/// 的手动纵向偏移 (像素, 负值即比默认居中更靠上). 蒙版编辑把居中留白折进
/// 第一块 `gap_before` 后, 此值会是 `-natural_voff`, 让拼合图顶对齐到
/// 页顶. 只改谱面在已裁出的画布内的贴图位置, 不改裁切区域本身.
/// `top_transparent`: 谱面 (拼合图) 最上面这么多行不贴到画布上——这段是
/// 拖动第一块产生的人为留白, 没有真实内容, 直接露出底色本身即可, 不需要
/// (也不该) 贴任何颜色上去, 见 [`composite_preview`] 与
/// `mask_tool::layout::stitch_with_stats`.
pub fn composite_and_crop(
    sheet: &RgbImage,
    bg: &RgbImage,
    aspect_w: u32,
    aspect_h: u32,
    voff_shift: i64,
    top_transparent: u32,
    bottom_transparent: u32,
) -> Result<RgbImage, String> {
    if aspect_w == 0 || aspect_h == 0 {
        return Err("比例宽高必须为正整数".into());
    }
    let (sw, sh) = sheet.dimensions();
    let (bw, bh) = bg.dimensions();
    let (page_w, page_h) = page_size(sw, aspect_w, aspect_h);
    if bw < page_w || bh < page_h {
        return Err(format!("底色 ({bw}x{bh}) 无法完全盖住页面 ({page_w}x{page_h})"));
    }

    let frame = preview_frame(sw, sh, bw, bh, aspect_w, aspect_h, voff_shift);
    if !frame.shows_bg {
        return Ok(sheet.clone());
    }
    let mut canvas = crop_fast(bg, frame.bg_left, frame.bg_top, frame.canvas_w, frame.canvas_h);
    overlay_sheet(
        &mut canvas,
        sheet,
        frame.hoff,
        frame.voff,
        top_transparent,
        bottom_transparent,
        frame.content_scale,
    );
    Ok(canvas)
}

/// 谱面居中叠底色的预览 (蒙版用): 画布与终稿同一套 contain 比例,
/// 上下或左右补出底色. 返回 (预览图, 谱面在预览图中的横向/纵向偏移).
/// `voff_shift`/`top_transparent` 含义见 [`composite_and_crop`].
pub fn composite_preview(
    sheet: &RgbImage,
    bg: &RgbImage,
    aspect_w: u32,
    aspect_h: u32,
    voff_shift: i64,
    top_transparent: u32,
    bottom_transparent: u32,
) -> Result<(RgbImage, i64, i64), String> {
    if aspect_w == 0 || aspect_h == 0 {
        return Err("比例宽高必须为正整数".into());
    }
    let (sw, sh) = sheet.dimensions();
    let (bw, bh) = bg.dimensions();
    let frame = preview_frame(sw, sh, bw, bh, aspect_w, aspect_h, voff_shift);
    if !frame.shows_bg {
        return Ok((sheet.clone(), 0, 0));
    }

    let mut canvas = crop_fast(bg, frame.bg_left, frame.bg_top, frame.canvas_w, frame.canvas_h);
    overlay_sheet(
        &mut canvas,
        sheet,
        frame.hoff,
        frame.voff,
        top_transparent,
        bottom_transparent,
        frame.content_scale,
    );
    Ok((canvas, frame.hoff, frame.voff))
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
        .map_err(|e| ProcessError::new(name.clone(), e.to_string()))?
        .to_rgb8();

    let out = composite_and_crop(&sheet, bg, aspect_w, aspect_h, 0, 0, 0)
        .map_err(|message| ProcessError::new(name.clone(), message))?;

    let dest = out_dir.join(&name);
    DynamicImage::ImageRgb8(out)
        .save(&dest)
        .map_err(|e| ProcessError::new(name, e.to_string()))?;
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
) -> Result<ProcessResult, ProcessError> {
    if aspect_w == 0 || aspect_h == 0 {
        return Err(ProcessError::folder("比例宽高必须为正整数"));
    }
    if let Some(j) = jobs {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(j.max(1))
            .build_global();
    }

    if !in_dir.is_dir() {
        return Err(ProcessError::folder(format!(
            "输入目录无效: {}",
            in_dir.display()
        )));
    }
    if !bg_path.is_file() {
        return Err(ProcessError::folder(format!(
            "底色不存在: {}",
            bg_path.display()
        )));
    }

    let files = list_images(in_dir)
        .map_err(|e| ProcessError::folder(format!("无法读取目录: {e}")))?;
    if files.is_empty() {
        return Err(ProcessError::folder("输入目录没有图片."));
    }

    fs::create_dir_all(out_dir)
        .map_err(|e| ProcessError::folder(format!("无法创建输出目录: {e}")))?;

    let t0 = Instant::now();
    let bg = Arc::new(
        image::open(bg_path)
            .map_err(|e| ProcessError::folder(format!("无法打开底色: {e}")))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb([r, g, b]))
    }

    #[test]
    fn frame_size_wide_sheet_pads_vertically() {
        assert_eq!(frame_size(2000, 400, 16, 9), (2000, 1125));
        assert_eq!(frame_size(2000, 400, 2560, 1440), (2000, 1125));
    }

    #[test]
    fn frame_size_tall_sheet_pads_horizontally() {
        assert_eq!(frame_size(2000, 2500, 16, 9), (4444, 2500));
        assert_eq!(frame_size(2000, 2500, 2560, 1440), (4444, 2500));
    }

    #[test]
    fn frame_size_already_matching_stays() {
        assert_eq!(frame_size(1920, 1080, 16, 9), (1920, 1080));
    }

    #[test]
    fn page_size_is_width_based() {
        assert_eq!(page_size(2000, 16, 9), (2000, 1125));
        assert_eq!(page_size(2000, 2560, 1440), (2000, 1125));
        assert_eq!(page_size(1920, 16, 9), (1920, 1080));
    }

    #[test]
    fn composite_contain_matches_page_size() {
        let bg = solid(8000, 8000, 10, 20, 30);
        let wide = solid(2000, 400, 200, 200, 200);
        let tall = solid(2000, 2500, 200, 200, 200);
        let out_w = composite_and_crop(&wide, &bg, 16, 9, 0, 0, 0).unwrap();
        let out_t = composite_and_crop(&tall, &bg, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(out_w.dimensions(), (2000, 1125));
        // 高谱不再放大页面, 画布锁在按宽定高的尺寸, 内部块缩小装进去.
        assert_eq!(out_t.dimensions(), (2000, 1125));
        let (pw, hoff_w, voff_w) = composite_preview(&wide, &bg, 16, 9, 0, 0, 0).unwrap();
        let (pt, hoff_t, voff_t) = composite_preview(&tall, &bg, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(pw.dimensions(), out_w.dimensions());
        assert_eq!(pt.dimensions(), out_t.dimensions());
        assert_eq!(hoff_w, 0);
        assert!(voff_w > 0);
        assert!(hoff_t > 0);
        assert_eq!(voff_t, 0);
        // 页面四角仍是底色; 缩小后的谱面水平居中, 从顶端贴齐.
        assert_eq!(*pt.get_pixel(0, 0), Rgb([10, 20, 30]));
        assert_eq!(*pt.get_pixel(hoff_t as u32, 0), Rgb([200, 200, 200]));
        let frame_t = preview_frame(2000, 2500, 8000, 8000, 16, 9, 0);
        assert!(frame_t.content_scale < 1.0);
        assert!((frame_t.content_scale - 1125.0 / 2500.0).abs() < 1e-6);
        assert_eq!((frame_t.canvas_w, frame_t.canvas_h), (2000, 1125));
    }

    #[test]
    fn voff_shift_moves_sheet_up_past_padding() {
        let bg = solid(8000, 8000, 10, 20, 30);
        let wide = solid(2000, 400, 200, 200, 200);
        let (_, _, voff0) = composite_preview(&wide, &bg, 16, 9, 0, 0, 0).unwrap();
        assert!(voff0 > 0);
        // 负偏移把谱面往上挪, 在合理范围内应该精确生效.
        let (_, _, voff_up) = composite_preview(&wide, &bg, 16, 9, -10, 0, 0).unwrap();
        assert_eq!(voff_up, voff0 - 10);
        // 超过居中留白后钳制在 0, 不再把内容推出页面顶端.
        let (_, _, voff_past) = composite_preview(&wide, &bg, 16, 9, -(voff0 + 100), 0, 0).unwrap();
        assert_eq!(voff_past, 0);
        let out = composite_and_crop(&wide, &bg, 16, 9, -(voff0 + 100), 0, 0).unwrap();
        assert_eq!(out.dimensions(), composite_and_crop(&wide, &bg, 16, 9, 0, 0, 0).unwrap().dimensions());
    }

    #[test]
    fn preview_frame_matches_composite_preview() {
        let bg = solid(8000, 8000, 10, 20, 30);
        let wide = solid(2000, 400, 200, 200, 200);
        let (out, hoff, voff) = composite_preview(&wide, &bg, 16, 9, -12, 0, 0).unwrap();
        let frame = preview_frame(2000, 400, 8000, 8000, 16, 9, -12);
        assert!(frame.shows_bg);
        assert_eq!(frame.hoff, hoff);
        assert_eq!(frame.voff, voff);
        assert_eq!((frame.canvas_w, frame.canvas_h), out.dimensions());
    }

    #[test]
    fn natural_voff_matches_composite_at_zero_shift() {
        let bg = solid(8000, 8000, 10, 20, 30);
        let wide = solid(2000, 400, 200, 200, 200);
        let (_, _, voff0) = composite_preview(&wide, &bg, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(natural_voff(2000, 400, 8000, 8000, 16, 9), voff0);

        let tall = solid(2000, 2500, 200, 200, 200);
        let (_, _, voff_t) = composite_preview(&tall, &bg, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(natural_voff(2000, 2500, 8000, 8000, 16, 9), voff_t);
    }

    #[test]
    fn top_transparent_rows_let_background_show_through() {
        // 谱面顶端若干行是"人为拖出来的留白" (纯白, 255,255,255), 用
        // `top_transparent` 跳过这几行的贴图后, 画布对应位置应该露出
        // 底色本身的颜色 (10,20,30), 而不是谱面自带的白色.
        let bg = solid(8000, 8000, 10, 20, 30);
        let mut wide = solid(2000, 400, 200, 200, 200);
        for y in 0..20u32 {
            for x in 0..wide.width() {
                wide.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let (with_skip, _, voff) = composite_preview(&wide, &bg, 16, 9, 0, 20, 0).unwrap();
        assert_eq!(*with_skip.get_pixel(0, voff as u32), image::Rgb([10, 20, 30]));
        assert_eq!(*with_skip.get_pixel(0, voff as u32 + 19), image::Rgb([10, 20, 30]));
        // 跳过的行数之后, 谱面自己的内容 (200,200,200) 照常显示.
        assert_eq!(*with_skip.get_pixel(0, voff as u32 + 20), image::Rgb([200, 200, 200]));
        // 不跳过时该处仍是谱面自带的纯白.
        let (no_skip, _, voff2) = composite_preview(&wide, &bg, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(voff2, voff);
        assert_eq!(*no_skip.get_pixel(0, voff2 as u32), image::Rgb([255, 255, 255]));
    }

    #[test]
    fn natural_voff_handles_aspect_regime_crossing_exactly() {
        // sw 固定, sh 跨越 page_h 分界点前后: 越过分界点后不再放大页面,
        // 自然留白归零, 内容改为缩小装进页面.
        let sw = 2000u32;
        let h_from_w = ((sw as f64) * 9.0 / 16.0).round() as u32;
        let just_before = natural_voff(sw, h_from_w - 10, 8000, 8000, 16, 9);
        let at_boundary = natural_voff(sw, h_from_w, 8000, 8000, 16, 9);
        let just_after = natural_voff(sw, h_from_w + 10, 8000, 8000, 16, 9);
        assert!(just_before > 0);
        assert_eq!(at_boundary, 0);
        assert_eq!(just_after, 0);
    }

    #[test]
    fn crop_bg_to_page_matches_preview_canvas() {
        let mut bg = solid(800, 800, 10, 20, 30);
        bg.put_pixel(400, 400, Rgb([1, 2, 3]));
        let sheet_w = 200u32;
        let crop = crop_bg_to_page(&bg, 16, 9, sheet_w).expect("bg covers page");
        let frame = preview_frame(sheet_w, 400, 800, 800, 16, 9, 0);
        assert_eq!(crop.dimensions(), (frame.canvas_w, frame.canvas_h));
        let tall_frame = preview_frame(sheet_w, 2500, 800, 800, 16, 9, 0);
        assert_eq!(
            (crop.width(), crop.height()),
            (tall_frame.canvas_w, tall_frame.canvas_h)
        );
        let cx = 400u32 - frame.bg_left;
        let cy = 400u32 - frame.bg_top;
        assert_eq!(*crop.get_pixel(cx, cy), Rgb([1, 2, 3]));
    }

    #[test]
    fn crop_bg_to_page_rejects_undersized_bg() {
        let bg = solid(100, 50, 10, 20, 30);
        assert!(crop_bg_to_page(&bg, 16, 9, 200).is_none());
        assert!(bg_page_rect(100, 50, 16, 9, 200).is_none());
    }
}
