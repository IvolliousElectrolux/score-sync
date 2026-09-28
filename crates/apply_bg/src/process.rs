//! 谱面加底色并按指定比例裁切 — 谱面完整装进画布 (contain).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use image::{DynamicImage, Rgb, RgbImage};
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

/// 等比缩放到刚好盖住 `tw×th`. 较短的一边对齐目标, 较长的一边可以超出.
/// 相对目标更宽时锁高, 更高时锁宽.
/// 例: 底色 2×1, 目标 20×16 → 32×16 (高对齐, 宽超出).
pub fn cover_size(bw: u32, bh: u32, tw: u32, th: u32) -> (u32, u32) {
    let bw = u64::from(bw.max(1));
    let bh = u64::from(bh.max(1));
    let tw = u64::from(tw.max(1));
    let th = u64::from(th.max(1));
    let ceil_div = |n: u64, d: u64| n.div_ceil(d);
    if bw * th >= tw * bh {
        let new_w = ceil_div(bw * th, bh).max(tw);
        (new_w.min(u64::from(u32::MAX)) as u32, th as u32)
    } else {
        let new_h = ceil_div(bh * tw, bw).max(th);
        (tw as u32, new_h.min(u64::from(u32::MAX)) as u32)
    }
}

/// 把整张底色等比缩放到刚好盖住 `tw×th`. 短边对齐, 长边超出的部分保留.
pub fn cover_resize(src: &RgbImage, tw: u32, th: u32) -> RgbImage {
    let (nw, nh) = cover_size(src.width(), src.height(), tw, th);
    if src.width() == nw && src.height() == nh {
        return src.clone();
    }
    image::imageops::resize(src, nw, nh, image::imageops::FilterType::Lanczos3)
}

/// 长边超出不超过这么多像素, 就当成宽高比几乎一样, 直接拉齐.
pub const COVER_ALIGN_PX: u32 = 10;
/// 长边超出不超过目标边的这个比例, 也直接拉齐. 和 [`COVER_ALIGN_PX`] 取更宽的那个.
pub const COVER_ALIGN_RATIO: f64 = 0.005;

pub fn cover_align_limit(target_side: u32) -> u32 {
    let pct = (f64::from(target_side.max(1)) * COVER_ALIGN_RATIO).round() as u32;
    COVER_ALIGN_PX.max(pct)
}

/// 等比盖住 `tw×th`. 长边超出在 [`cover_align_limit`] 以内时改成精确的 `tw×th`,
/// 否则短边对齐, 长边保留超出.
pub fn cover_target(bw: u32, bh: u32, tw: u32, th: u32) -> (u32, u32) {
    let tw = tw.max(1);
    let th = th.max(1);
    let (nw, nh) = cover_size(bw, bh, tw, th);
    let over_w = nw.saturating_sub(tw);
    let over_h = nh.saturating_sub(th);
    if over_w <= cover_align_limit(tw) && over_h <= cover_align_limit(th) {
        (tw, th)
    } else {
        (nw, nh)
    }
}

pub fn cover_target_resize(src: &RgbImage, tw: u32, th: u32) -> RgbImage {
    let (nw, nh) = cover_target(src.width(), src.height(), tw, th);
    if src.width() == nw && src.height() == nh {
        return src.clone();
    }
    image::imageops::resize(src, nw, nh, image::imageops::FilterType::Lanczos3)
}

/// 底色页面就是目标分辨率本身 (`aspect_w×aspect_h`), 不是谱面像素.
/// 谱面再小也放大到这一页上对齐, 见 [`preview_frame`].
pub fn page_size(_sw: u32, aspect_w: u32, aspect_h: u32) -> (u32, u32) {
    if aspect_w == 0 || aspect_h == 0 {
        return (1, 1);
    }
    (aspect_w.max(1), aspect_h.max(1))
}

/// 完整底色上、按目标分辨率定下的页面矩形 `(left, top, w, h)`.
/// 画布就是目标分辨率, 跟谱面像素无关. 底色装不下该页时返回 `None`.
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

/// 从完整底色裁出当前谱面宽对应的一页. 盖不住则原样返回.
pub fn working_bg_copy(bg: RgbImage, aspect_w: u32, aspect_h: u32, sheet_w: u32) -> RgbImage {
    if sheet_w == 0 {
        return bg;
    }
    crop_bg_to_page(&bg, aspect_w, aspect_h, sheet_w).unwrap_or(bg)
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
fn overlay_fast(
    dst: &mut RgbImage,
    src: &RgbImage,
    dx: i64,
    dy: i64,
    skip_top_rows: u32,
    skip_bottom_rows: u32,
) {
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

/// 谱面盖住目标页时, 不含手动偏移的纵向起点 (像素).
/// 宽对齐且谱面更矮时是垂直居中的上沿; 更高时装进页高, 起点为 0.
pub fn natural_voff(sw: u32, sh: u32, bw: u32, bh: u32, aspect_w: u32, aspect_h: u32) -> i64 {
    if aspect_w == 0 || aspect_h == 0 {
        return 0;
    }
    let frame = preview_frame(sw, sh, bw, bh, aspect_w, aspect_h, 0);
    if !frame.shows_bg {
        return 0;
    }
    frame.voff
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
    /// 谱面相对页面的缩放. 宽对齐目标分辨率; 按宽放大后高出页面时改为装进高度.
    /// 整段谱都留在画面里. 谱面比目标窄时大于 1.
    pub content_scale: f32,
}

/// 与 [`composite_preview`] / [`composite_and_crop`] 同一套页面几何,
/// 只算数字, 不碰像素. 画布就是目标分辨率. 和蒙版一样按宽对齐:
/// 谱面宽拉到页面宽, 矮于页面时垂直居中; 更高时缩小装进页高, 不裁谱.
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
    let Some((bg_left, bg_top, canvas_w, canvas_h)) = bg_page_rect(bw, bh, aspect_w, aspect_h, sw)
    else {
        return fallback;
    };
    let scale_w = canvas_w as f32 / sw as f32;
    let content_scale = if (sh as f32) * scale_w <= canvas_h as f32 {
        scale_w
    } else {
        canvas_h as f32 / sh as f32
    };
    let disp_w = ((sw as f32) * content_scale).round() as i64;
    let disp_h = ((sh as f32) * content_scale).round() as i64;
    let hoff = (canvas_w as i64 - disp_w) / 2;
    let span = canvas_h as i64 - disp_h;
    let voff = (span / 2 + voff_shift).clamp(span.min(0), span.max(0));
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
        return Err(format!(
            "底色 ({bw}x{bh}) 无法完全盖住页面 ({page_w}x{page_h})"
        ));
    }

    let frame = preview_frame(sw, sh, bw, bh, aspect_w, aspect_h, voff_shift);
    if !frame.shows_bg {
        return Ok(sheet.clone());
    }
    let mut canvas = if bg.width() == frame.canvas_w
        && bg.height() == frame.canvas_h
        && frame.bg_left == 0
        && frame.bg_top == 0
    {
        bg.clone()
    } else {
        crop_fast(
            bg,
            frame.bg_left,
            frame.bg_top,
            frame.canvas_w,
            frame.canvas_h,
        )
    };
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

    let mut canvas = crop_fast(
        bg,
        frame.bg_left,
        frame.bg_top,
        frame.canvas_w,
        frame.canvas_h,
    );
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

/// 纯色底色终稿: 不造整张底色图, 按 [`preview_frame`] 的画布直接填色再叠谱面.
/// `src_w`/`src_h` 只参与几何 (与图片底色的完整备份尺寸同角色).
pub fn composite_solid(
    sheet: &RgbImage,
    color: [u8; 3],
    src_w: u32,
    src_h: u32,
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
    let frame = preview_frame(sw, sh, src_w, src_h, aspect_w, aspect_h, voff_shift);
    if !frame.shows_bg {
        return Ok(sheet.clone());
    }
    let mut canvas = RgbImage::from_pixel(frame.canvas_w, frame.canvas_h, Rgb(color));
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

/// 纯色底色预览: 几何与 [`composite_solid`] / [`preview_frame`] 一致.
pub fn composite_preview_solid(
    sheet: &RgbImage,
    color: [u8; 3],
    src_w: u32,
    src_h: u32,
    aspect_w: u32,
    aspect_h: u32,
    voff_shift: i64,
    top_transparent: u32,
    bottom_transparent: u32,
) -> Result<(RgbImage, i64, i64), String> {
    let canvas = composite_solid(
        sheet,
        color,
        src_w,
        src_h,
        aspect_w,
        aspect_h,
        voff_shift,
        top_transparent,
        bottom_transparent,
    )?;
    let (sw, sh) = sheet.dimensions();
    let frame = preview_frame(sw, sh, src_w, src_h, aspect_w, aspect_h, voff_shift);
    if !frame.shows_bg {
        return Ok((canvas, 0, 0));
    }
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

    let files =
        list_images(in_dir).map_err(|e| ProcessError::folder(format!("无法读取目录: {e}")))?;
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
    fn page_size_is_the_target_resolution() {
        assert_eq!(page_size(2000, 2560, 1440), (2560, 1440));
        assert_eq!(page_size(595, 2560, 1440), (2560, 1440));
        assert_eq!(page_size(1920, 1920, 1080), (1920, 1080));
    }

    #[test]
    fn short_sheet_width_aligns_to_the_target() {
        let bg = solid(2560, 1440, 10, 20, 30);
        let sheet = solid(1600, 400, 200, 200, 200);
        let frame = preview_frame(1600, 400, 2560, 1440, 2560, 1440, 0);
        assert_eq!((frame.canvas_w, frame.canvas_h), (2560, 1440));
        assert!((frame.content_scale - 2560.0 / 1600.0).abs() < 1e-4);
        assert_eq!(frame.hoff, 0);
        assert!(frame.voff > 0);
        let out = composite_and_crop(&sheet, &bg, 2560, 1440, 0, 0, 0).unwrap();
        assert_eq!(out.dimensions(), (2560, 1440));
        assert_eq!(*out.get_pixel(0, 0), Rgb([10, 20, 30]));
        assert_eq!(*out.get_pixel(0, frame.voff as u32), Rgb([200, 200, 200]));
        assert_eq!(*out.get_pixel(2559, frame.voff as u32), Rgb([200, 200, 200]));
    }

    #[test]
    fn composite_width_align_matches_page_size() {
        let bg = solid(8000, 8000, 10, 20, 30);
        let wide = solid(2000, 400, 200, 200, 200);
        let tall = solid(2000, 2500, 200, 200, 200);
        let out_w = composite_and_crop(&wide, &bg, 2560, 1440, 0, 0, 0).unwrap();
        let out_t = composite_and_crop(&tall, &bg, 2560, 1440, 0, 0, 0).unwrap();
        assert_eq!(out_w.dimensions(), (2560, 1440));
        assert_eq!(out_t.dimensions(), (2560, 1440));
        let (pw, hoff_w, voff_w) = composite_preview(&wide, &bg, 2560, 1440, 0, 0, 0).unwrap();
        let (pt, hoff_t, voff_t) = composite_preview(&tall, &bg, 2560, 1440, 0, 0, 0).unwrap();
        assert_eq!(pw.dimensions(), out_w.dimensions());
        assert_eq!(pt.dimensions(), out_t.dimensions());
        assert_eq!(hoff_w, 0);
        assert!(voff_w > 0);
        assert!(hoff_t > 0);
        assert_eq!(voff_t, 0);
        assert_eq!(*pw.get_pixel(0, 0), Rgb([10, 20, 30]));
        assert_eq!(*pw.get_pixel(0, voff_w as u32), Rgb([200, 200, 200]));
        assert_eq!(*pt.get_pixel(0, 0), Rgb([10, 20, 30]));
        assert_eq!(*pt.get_pixel(hoff_t as u32, 0), Rgb([200, 200, 200]));
        let frame_w = preview_frame(2000, 400, 8000, 8000, 2560, 1440, 0);
        assert!((frame_w.content_scale - 2560.0 / 2000.0).abs() < 1e-6);
        let frame_t = preview_frame(2000, 2500, 8000, 8000, 2560, 1440, 0);
        assert!((frame_t.content_scale - 1440.0 / 2500.0).abs() < 1e-6);
    }

    #[test]
    fn composite_solid_matches_image_fill() {
        let color = [10u8, 20, 30];
        let bg = solid(8000, 8000, color[0], color[1], color[2]);
        let wide = solid(2000, 400, 200, 200, 200);
        let from_img = composite_and_crop(&wide, &bg, 16, 9, 0, 0, 0).unwrap();
        let from_solid = composite_solid(&wide, color, 8000, 8000, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(from_img.dimensions(), from_solid.dimensions());
        assert_eq!(from_img.get_pixel(0, 0), from_solid.get_pixel(0, 0));
        let (pw, hoff, voff) =
            composite_preview_solid(&wide, color, 8000, 8000, 16, 9, 0, 0, 0).unwrap();
        assert_eq!(pw.dimensions(), from_solid.dimensions());
        assert_eq!(hoff, 0);
        assert!(voff > 0);
    }

    #[test]
    fn voff_shift_moves_sheet_up_within_the_page() {
        let bg = solid(8000, 8000, 10, 20, 30);
        let wide = solid(2000, 400, 200, 200, 200);
        let (_, _, voff0) = composite_preview(&wide, &bg, 2560, 1440, 0, 0, 0).unwrap();
        assert!(voff0 > 0);
        let (_, _, voff_up) = composite_preview(&wide, &bg, 2560, 1440, -10, 0, 0).unwrap();
        assert_eq!(voff_up, voff0 - 10);
        let (_, _, voff_past) =
            composite_preview(&wide, &bg, 2560, 1440, -(voff0 + 100), 0, 0).unwrap();
        assert_eq!(voff_past, 0);
        let out = composite_and_crop(&wide, &bg, 2560, 1440, -(voff0 + 100), 0, 0).unwrap();
        assert_eq!(out.dimensions(), (2560, 1440));
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
        let (with_skip, _, voff) = composite_preview(&wide, &bg, 2000, 400, 0, 20, 0).unwrap();
        assert_eq!(
            *with_skip.get_pixel(0, voff as u32),
            image::Rgb([10, 20, 30])
        );
        assert_eq!(
            *with_skip.get_pixel(0, voff as u32 + 19),
            image::Rgb([10, 20, 30])
        );
        // 跳过的行数之后, 谱面自己的内容 (200,200,200) 照常显示.
        assert_eq!(
            *with_skip.get_pixel(0, voff as u32 + 20),
            image::Rgb([200, 200, 200])
        );
        // 不跳过时该处仍是谱面自带的纯白.
        let (no_skip, _, voff2) = composite_preview(&wide, &bg, 2000, 400, 0, 0, 0).unwrap();
        assert_eq!(voff2, voff);
        assert_eq!(
            *no_skip.get_pixel(0, voff2 as u32),
            image::Rgb([255, 255, 255])
        );
    }

    #[test]
    fn natural_voff_handles_aspect_regime_crossing_exactly() {
        // 宽对齐后仍矮于页面时有留白; 刚好顶满或更高时留白归零, 改为缩小.
        let sw = 2000u32;
        let page_h = 1440u32;
        let fit_h = ((page_h as f64) * (sw as f64) / 2560.0).round() as u32;
        let just_before = natural_voff(sw, fit_h - 10, 8000, 8000, 2560, 1440);
        let at_boundary = natural_voff(sw, fit_h, 8000, 8000, 2560, 1440);
        let just_after = natural_voff(sw, fit_h + 10, 8000, 8000, 2560, 1440);
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
    fn cover_size_locks_the_shorter_side_so_the_image_covers() {
        assert_eq!(cover_size(2, 1, 20, 16), (32, 16));
        assert_eq!(cover_size(1, 2, 20, 16), (20, 40));
        assert_eq!(cover_size(20, 16, 20, 16), (20, 16));
        assert_eq!(cover_size(2000, 1000, 2560, 1440), (2880, 1440));
    }

    #[test]
    fn cover_aligns_the_short_side_and_lets_the_long_side_overflow() {
        // 月光下的德累斯顿.png 是 1422×800, 比 2560×1440 略高. 等比只高出 1 像素, 直接拉齐.
        assert_eq!(cover_size(1422, 800, 2560, 1440), (2560, 1441));
        assert_eq!(cover_target(1422, 800, 2560, 1440), (2560, 1440));
        let almost = cover_target_resize(&solid(1422, 800, 8, 16, 24), 2560, 1440);
        assert_eq!(almost.dimensions(), (2560, 1440));
        // 更宽的图: 高对齐 1440, 宽超出 320, 留下.
        assert_eq!(cover_target(2000, 1000, 2560, 1440), (2880, 1440));
        // 高出 10 像素仍拉齐; 高出 11 像素留下.
        assert_eq!(cover_target(2560, 1450, 2560, 1440), (2560, 1440));
        assert_eq!(cover_target(2560, 1451, 2560, 1440), (2560, 1451));
        // 大目标上 0.5% 比 10 像素宽: 超出 15 拉齐, 超出 25 留下.
        assert_eq!(cover_target(4015, 2000, 4000, 2000), (4000, 2000));
        assert_eq!(cover_target(4025, 2000, 4000, 2000), (4025, 2000));
        // 反色.pdf 的页面像素不再决定成片. 成片就是目标分辨率.
        assert_eq!(page_size(1785, 2560, 1440), (2560, 1440));
    }

    #[test]
    fn dresden_png_covers_default_target() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../月光下的德累斯顿.png");
        if !path.is_file() {
            return;
        }
        let file = std::fs::File::open(&path).unwrap();
        let img = image::ImageReader::new(std::io::BufReader::new(file))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
            .into_rgb8();
        assert_eq!(img.dimensions(), (1422, 800));
        assert_eq!(
            cover_target_resize(&img, 2560, 1440).dimensions(),
            (2560, 1440)
        );
        let (w, h) = cover_target(img.width(), img.height(), 2560, 1440);
        assert!(bg_page_rect(w, h, 2560, 1440, 595).is_some());
        assert!(bg_page_rect(img.width(), img.height(), 2560, 1440, 595).is_none());
    }

    #[test]
    fn small_widescreen_covers_the_page_instead_of_the_aspect_numbers() {
        let (nw, nh) = cover_target(1422, 800, 2560, 1440);
        assert_eq!((nw, nh), (2560, 1440));
        let scaled = cover_target_resize(&solid(1422, 800, 8, 16, 24), 2560, 1440);
        assert_eq!(scaled.dimensions(), (nw, nh));
    }

    #[test]
    fn crop_bg_to_page_rejects_undersized_bg() {
        let bg = solid(100, 50, 10, 20, 30);
        assert!(crop_bg_to_page(&bg, 2560, 1440, 200).is_none());
        assert!(bg_page_rect(100, 50, 2560, 1440, 200).is_none());
    }
}
