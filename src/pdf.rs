//! PDF 渲染为临时 PNG (pdfium), 对齐 app.py 的 pdf_pages_to_tmp_images.
//!
//! `pdfium` 动态库 (以及 `ffmpeg`) 都不再打进可执行文件, 而是当作外部依赖:
//! 优先找程序自身同目录下的那份, 找不到再去系统 PATH 里找, 都找不到就报错
//! 提示用户放一份到 exe 旁边. 也可用环境变量 `PDFIUM_DYNAMIC_LIB_PATH` 强制
//! 指定路径.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pdfium_render::prelude::*;

/// 未指定时按 PDF 标记尺寸 (point, 1/72 inch) 的倍率光栅化.
/// 矢量/扫描一视同仁, 不读取页内图像的像素尺寸.
pub const DEFAULT_PDF_SCALE: f32 = 3.0;
/// 单边像素上限, 避免 pdfium / 内存炸掉.
pub const PDF_MAX_SIDE_PX: u32 = 8192;
pub const PDF_MIN_SCALE: f32 = 0.5;
pub const PDF_MAX_SCALE: f32 = 16.0;

/// 一份 PDF 里同尺寸页面的分组 (相邻页合并成范围).
#[derive(Clone, Debug)]
pub struct PdfSizeGroup {
    pub w_pt: f32,
    pub h_pt: f32,
    /// 1-based 页码.
    pub pages: Vec<u32>,
    /// 该尺寸代表页上最大嵌入图像的像素 (扫描件常远大于标记尺寸×3).
    pub image_px: Option<(u32, u32)>,
}

impl PdfSizeGroup {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}

#[derive(Clone, Debug)]
pub struct PdfInspect {
    pub path: PathBuf,
    pub name: String,
    pub page_count: usize,
    /// 按页数从多到少.
    pub groups: Vec<PdfSizeGroup>,
}

static PDF_TMP_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn cleanup_pdf_tmps() {
    if let Ok(mut dirs) = PDF_TMP_DIRS.lock() {
        for d in dirs.drain(..) {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}

fn lib_name() -> &'static str {
    #[cfg(windows)]
    {
        "pdfium.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libpdfium.dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "libpdfium.so"
    }
}

/// 在 PATH 环境变量列出的各目录里找 `lib_name()`, 找到就返回完整路径
/// (和 ffmpeg 那边 `ffmpeg_path()` 的 PATH 兜底是同一个思路, 只是 DLL 不能
/// 靠 `Command` 让系统自己解析, 得手动扫一遍 PATH).
fn find_in_path_env() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let name = lib_name();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_pdfium_path() -> Option<PathBuf> {
    // 1) 环境变量可强制指定 (文件或目录都行)
    if let Ok(p) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return Some(pb);
        }
        let dll = pb.join(lib_name());
        if dll.is_file() {
            return Some(dll);
        }
    }
    // 2) 程序自身同目录下 (发行包自带, 不用另外装)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [dir.join(lib_name()), dir.join("pdfium").join(lib_name())] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    // 3) 系统 PATH 里兜底 (兼容已经单独装了 pdfium 的开发环境)
    find_in_path_env()
}

pub(crate) fn bind_pdfium() -> Result<Pdfium, crate::error::Error> {
    let path = find_pdfium_path().ok_or_else(|| crate::error::Error::PdfiumMissing {
        lib: lib_name().to_string(),
    })?;
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
        .or_else(|_| Pdfium::bind_to_library(&path))
        .map_err(|e| crate::error::Error::PdfiumLoad {
            path: path.clone(),
            detail: e.to_string(),
        })?;
    Ok(Pdfium::new(bindings))
}

fn size_key(w: f32, h: f32) -> (i32, i32) {
    ((w * 2.0).round() as i32, (h * 2.0).round() as i32)
}

#[cfg(test)]
fn format_page_ranges(pages: &[u32]) -> String {
    if pages.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut start = pages[0];
    let mut prev = pages[0];
    for &p in &pages[1..] {
        if p == prev + 1 {
            prev = p;
            continue;
        }
        push_range(&mut out, start, prev);
        start = p;
        prev = p;
    }
    push_range(&mut out, start, prev);
    out
}

#[cfg(test)]
fn push_range(out: &mut String, start: u32, end: u32) {
    if !out.is_empty() {
        out.push_str(", ");
    }
    if start == end {
        out.push_str(&start.to_string());
    } else {
        out.push_str(&format!("{start}-{end}"));
    }
}

/// 解析形如 `1, 3-7, 9-13` 的页码 (1-based). 空输入表示全部页.
/// 容忍全/半角逗号、连字符/波浪线、全角数字与任意空格; 非法片段忽略,
/// 超出 `[1, total]` 的页码丢弃. 结果已排序去重.
pub fn parse_page_selection(input: &str, total: usize) -> Vec<u32> {
    if total == 0 {
        return Vec::new();
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return (1..=total as u32).collect();
    }
    let normalized: String = trimmed
        .chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => None,
            '，' => Some(','),
            '－' | '—' | '～' | '~' => Some('-'),
            '\u{FF10}'..='\u{FF19}' => char::from_u32(c as u32 - 0xFF10 + '0' as u32),
            c => Some(c),
        })
        .collect();
    let mut set = std::collections::BTreeSet::new();
    for part in normalized.split(',') {
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                for i in lo..=hi {
                    if i >= 1 && (i as usize) <= total {
                        set.insert(i);
                    }
                }
            }
        } else if let Ok(a) = part.parse::<u32>() {
            if a >= 1 && (a as usize) <= total {
                set.insert(a);
            }
        }
    }
    set.into_iter().collect()
}

fn probe_page_image_px(page: &PdfPage<'_>) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    for obj in page.objects().iter() {
        let Some(img) = obj.as_image_object() else {
            continue;
        };
        let Ok(bmp) = img.get_raw_bitmap() else {
            continue;
        };
        let w = bmp.width().max(0) as u32;
        let h = bmp.height().max(0) as u32;
        if w < 32 || h < 32 {
            continue;
        }
        let area = w.saturating_mul(h);
        let better = best.map(|(bw, bh)| area > bw.saturating_mul(bh)).unwrap_or(true);
        if better {
            best = Some((w, h));
        }
    }
    best
}

/// 只读页尺寸 (不渲染). 同尺寸页并成一组, 并抽样探测页内图像像素.
pub fn inspect_pdf(pdf_path: &Path) -> Result<PdfInspect, crate::error::Error> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| crate::error::Error::PdfOpen(e.to_string()))?;
    let n = document.pages().len() as usize;
    if n == 0 {
        return Err(crate::error::Error::PdfOpen(format!(
            "{} 没有页面.",
            pdf_path.display()
        )));
    }
    let sizes = document
        .pages()
        .page_sizes()
        .map_err(|e| crate::error::Error::PdfOpen(e.to_string()))?;

    let mut buckets: Vec<((i32, i32), PdfSizeGroup)> = Vec::new();
    for (i, rect) in sizes.iter().enumerate() {
        let w = rect.width().value.max(1.0);
        let h = rect.height().value.max(1.0);
        let key = size_key(w, h);
        let page_no = (i as u32) + 1;
        if let Some((_, g)) = buckets.iter_mut().find(|(k, _)| *k == key) {
            g.pages.push(page_no);
        } else {
            buckets.push((
                key,
                PdfSizeGroup {
                    w_pt: w,
                    h_pt: h,
                    pages: vec![page_no],
                    image_px: None,
                },
            ));
        }
    }
    for (_, g) in buckets.iter_mut() {
        let Some(&first) = g.pages.first() else {
            continue;
        };
        let idx = (first - 1) as u16;
        if let Ok(page) = document.pages().get(idx) {
            g.image_px = probe_page_image_px(&page);
        }
    }
    buckets.sort_by(|a, b| b.1.page_count().cmp(&a.1.page_count()));
    let name = pdf_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("pdf")
        .to_string();
    Ok(PdfInspect {
        path: pdf_path.to_path_buf(),
        name,
        page_count: n,
        groups: buckets.into_iter().map(|(_, g)| g).collect(),
    })
}

/// 导入弹窗预览: 按最长边限制光栅化单页, 不写临时文件.
pub fn render_pdf_page_preview(
    pdf_path: &Path,
    page_1based: u32,
    max_side: u32,
) -> Result<image::RgbImage, crate::error::Error> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| crate::error::Error::PdfOpen(e.to_string()))?;
    let n = document.pages().len() as u32;
    if page_1based == 0 || page_1based > n {
        return Err(crate::error::Error::PdfOpen(format!(
            "{} 没有第 {page_1based} 页 (共 {n} 页).",
            pdf_path.display()
        )));
    }
    let page = document
        .pages()
        .get((page_1based - 1) as u16)
        .map_err(|e| crate::error::Error::msg(format!("读取第 {page_1based} 页失败: {e}")))?;
    let w_pt = page.width().value.max(1.0);
    let h_pt = page.height().value.max(1.0);
    let cap = max_side.max(64) as f32;
    let scale = (cap / w_pt).min(cap / h_pt).clamp(0.05, PDF_MAX_SCALE);
    let cfg = PdfRenderConfig::new()
        .scale_page_width_by_factor(scale)
        .scale_page_height_by_factor(scale);
    let image = page
        .render_with_config(&cfg)
        .map_err(|e| crate::error::Error::msg(format!("渲染第 {page_1based} 页失败: {e}")))?
        .as_image()
        .into_rgb8();
    Ok(image)
}

pub fn clamp_pdf_scale(scale: f32) -> f32 {
    scale.clamp(PDF_MIN_SCALE, PDF_MAX_SCALE)
}

pub fn px_from_pt(pt: f32, scale: f32) -> u32 {
    let v = (pt * scale).round();
    (v as u32).clamp(1, PDF_MAX_SIDE_PX)
}

/// 由目标像素反推倍率 (锁定宽高比时用宽).
pub fn scale_from_target(pt: f32, px: u32) -> f32 {
    if pt < 0.5 {
        return DEFAULT_PDF_SCALE;
    }
    clamp_pdf_scale(px as f32 / pt)
}

/// 等比缩放到目标宽 (混入 PDF 时与谱面齐宽).
pub fn scale_rgb_to_width(rgb: image::RgbImage, target_w: u32) -> image::RgbImage {
    let w = rgb.width().max(1);
    let h = rgb.height().max(1);
    let tw = target_w.clamp(1, PDF_MAX_SIDE_PX);
    if w == tw {
        return rgb;
    }
    let th = ((h as u64)
        .saturating_mul(tw as u64)
        .saturating_div(w as u64))
    .clamp(1, PDF_MAX_SIDE_PX as u64) as u32;
    image::imageops::resize(&rgb, tw, th, image::imageops::FilterType::Lanczos3)
}

pub fn scale_rgb_to_size(rgb: image::RgbImage, target_w: u32, target_h: u32) -> image::RgbImage {
    let tw = target_w.clamp(1, PDF_MAX_SIDE_PX);
    let th = target_h.clamp(1, PDF_MAX_SIDE_PX);
    if rgb.width() == tw && rgb.height() == th {
        return rgb;
    }
    image::imageops::resize(&rgb, tw, th, image::imageops::FilterType::Lanczos3)
}

fn scale_for_page(scales: &[(f32, f32)], index: usize) -> (f32, f32) {
    if scales.is_empty() {
        return (DEFAULT_PDF_SCALE, DEFAULT_PDF_SCALE);
    }
    if let Some(&(sx, sy)) = scales.get(index) {
        return (sx.max(PDF_MIN_SCALE), sy.max(PDF_MIN_SCALE));
    }
    if scales.len() == 1 {
        let (sx, sy) = scales[0];
        return (sx.max(PDF_MIN_SCALE), sy.max(PDF_MIN_SCALE));
    }
    (DEFAULT_PDF_SCALE, DEFAULT_PDF_SCALE)
}

/// PDF 逐页渲染到临时 PNG; 每完成一页回调 `(index0, total, path)`.
/// `scales` 与页一一对应为 `(scale_x, scale_y)`; 长度为 1 时套用到每一页;
/// 空则用 [DEFAULT_PDF_SCALE].
/// `pages` 为 1-based 页码; 空则导入全部页.
/// 渲染后立刻在本线程识别并写 sidecar, 不占用 UI.
/// `should_continue` 返回 false 时停在当前页之前 (已写出的页保留), 返回已完成页数.
pub fn pdf_pages_to_tmp_images_streaming(
    pdf_path: &Path,
    ink_threshold: i32,
    margin: i32,
    scales: &[(f32, f32)],
    pages: &[u32],
    mut should_continue: impl FnMut() -> bool,
    mut on_page: impl FnMut(usize, usize, PathBuf),
) -> Result<usize, crate::error::Error> {
    crate::trace::log(&format!("pdf: 开始打开 {}", pdf_path.display()));
    let pdfium = bind_pdfium()?;
    crate::trace::log("pdf: pdfium 已加载");
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| crate::error::Error::PdfOpen(e.to_string()))?;
    crate::trace::log("pdf: 文档已打开");

    let tmp_dir = std::env::temp_dir().join(format!(
        "crop_sheet_pdf_{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| crate::error::Error::msg(format!("创建 PDF 临时目录失败: {e}")))?;
    if let Ok(mut dirs) = PDF_TMP_DIRS.lock() {
        dirs.push(tmp_dir.clone());
    }

    let stem = pdf_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pdf");
    let n = document.pages().len() as usize;
    let selected: Vec<usize> = if pages.is_empty() {
        (0..n).collect()
    } else {
        pages
            .iter()
            .filter_map(|&p| {
                let i = (p as usize).saturating_sub(1);
                (i < n).then_some(i)
            })
            .collect()
    };
    let total = selected.len();
    crate::trace::log(&format!(
        "pdf: 共 {n} 页, 导入 {total} 页, 渲染倍率 {scales:?} (空则 {DEFAULT_PDF_SCALE}), tmp={}",
        tmp_dir.display()
    ));
    if n == 0 || total == 0 {
        return Err(crate::error::Error::PdfOpen(format!(
            "{} 没有可导入的页面.",
            pdf_path.display()
        )));
    }

    for (done, &i) in selected.iter().enumerate() {
        if !should_continue() {
            crate::trace::log(&format!(
                "pdf: 在 {done}/{total} 处放弃 (已换工程或取消导入)"
            ));
            return Ok(done);
        }
        crate::trace::log(&format!("pdf: 渲染 {}/{total} (原第 {} 页) …", done + 1, i + 1));
        let page = document
            .pages()
            .get(i as u16)
            .map_err(|e| crate::error::Error::msg(format!("读取第 {} 页失败: {e}", i + 1)))?;
        let (sx, sy) = scale_for_page(scales, i);
        let cfg = PdfRenderConfig::new()
            .scale_page_width_by_factor(sx)
            .scale_page_height_by_factor(sy);
        let image = page
            .render_with_config(&cfg)
            .map_err(|e| crate::error::Error::msg(format!("渲染第 {} 页失败: {e}", i + 1)))?
            .as_image()
            .into_rgb8();
        crate::trace::log(&format!(
            "pdf: 渲染 {}/{total} 完成 {}×{}, 写 PNG …",
            done + 1,
            image.width(),
            image.height()
        ));
        let out_path = tmp_dir.join(format!("{stem}_p{:03}.png", i + 1));
        image
            .save(&out_path)
            .map_err(|e| crate::error::Error::msg(format!("写临时 PNG 失败: {e}")))?;
        crate::trace::log(&format!("pdf: 识别 {}/{total} …", done + 1));
        crate::detect_cache::detect_and_save(&image, &out_path, ink_threshold, margin);
        crate::trace::log(&format!("pdf: 已写+识别 {}/{total} → 回传 UI", done + 1));
        on_page(i, total, out_path);
    }
    crate::trace::log(&format!("pdf: 全部 {total} 页渲染结束"));
    Ok(total)
}

/// PDF 每页渲染到临时 PNG, 返回按页序的路径列表.
#[allow(dead_code)]
pub fn pdf_pages_to_tmp_images(pdf_path: &Path) -> Result<Vec<PathBuf>, crate::error::Error> {
    let mut out = Vec::new();
    pdf_pages_to_tmp_images_streaming(
        pdf_path,
        crate::model::DEFAULT_INK_THRESHOLD,
        crate::model::DEFAULT_MARGIN,
        &[],
        &[],
        || true,
        |_, _, p| {
            out.push(p);
        },
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pdfium 不再内嵌, 测试环境里不一定能找到; 本地在 `vendor/pdfium.dll`
    /// 放一份就能跑真实校验, 没放就跳过 (CI/新 clone 下这是预期情况, 不算
    /// 失败).
    #[test]
    fn bind_via_env_override_ok() {
        let dll = concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/pdfium.dll");
        if !std::path::Path::new(dll).is_file() {
            eprintln!("跳过: 未找到 {dll} (本地没放 pdfium.dll, 属预期情况)");
            return;
        }
        // SAFETY: 测试单线程内设置一次性环境变量, 供后续 find_pdfium_path 读取.
        unsafe {
            std::env::set_var("PDFIUM_DYNAMIC_LIB_PATH", dll);
        }
        bind_pdfium().expect("应能通过 PDFIUM_DYNAMIC_LIB_PATH 加载 pdfium");
    }

    #[test]
    fn page_ranges_collapse() {
        assert_eq!(format_page_ranges(&[1, 2, 3, 5, 6, 9]), "1-3, 5-6, 9");
        assert_eq!(format_page_ranges(&[4]), "4");
        assert_eq!(format_page_ranges(&[]), "");
    }

    #[test]
    fn parse_page_selection_empty_means_all() {
        assert_eq!(parse_page_selection("", 5), vec![1, 2, 3, 4, 5]);
        assert_eq!(parse_page_selection("  ", 3), vec![1, 2, 3]);
    }

    #[test]
    fn parse_page_selection_mixed_list_and_ranges() {
        assert_eq!(
            parse_page_selection("1, 3-7, 9-13", 20),
            vec![1, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13]
        );
    }

    #[test]
    fn parse_page_selection_tolerates_fullwidth() {
        assert_eq!(parse_page_selection("１，５～７", 20), vec![1, 5, 6, 7]);
    }

    #[test]
    fn parse_page_selection_drops_out_of_range() {
        assert_eq!(parse_page_selection("0, 5, 999", 6), vec![5]);
        assert_eq!(parse_page_selection("abc", 10), Vec::<u32>::new());
    }

    #[test]
    fn parse_page_selection_reversed_range() {
        assert_eq!(parse_page_selection("7-3", 10), vec![3, 4, 5, 6, 7]);
    }
}
