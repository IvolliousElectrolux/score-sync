//! PDF 渲染为临时 PNG (pdfium), 对齐 app.py 的 pdf_pages_to_tmp_images.
//!
//! `pdfium` 动态库 (以及 `ffmpeg`) 都不再打进可执行文件, 而是当作外部依赖:
//! 优先找程序自身同目录下的那份, 找不到再去系统 PATH 里找, 都找不到就报错
//! 提示用户放一份到 exe 旁边. 也可用环境变量 `PDFIUM_DYNAMIC_LIB_PATH` 强制
//! 指定路径.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pdfium_render::prelude::*;

const PDF_RENDER_SCALE: f32 = 3.0;

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

pub(crate) fn bind_pdfium() -> Result<Pdfium, String> {
    let path = find_pdfium_path().ok_or_else(|| {
        format!(
            "找不到 {} — 请把它放在程序同目录下, 或安装后加入系统 PATH, \
             也可设置环境变量 PDFIUM_DYNAMIC_LIB_PATH 指定路径.",
            lib_name()
        )
    })?;
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
        .or_else(|_| Pdfium::bind_to_library(&path))
        .map_err(|e| {
            format!(
                "无法加载 pdfium ({}): {e}",
                path.display()
            )
        })?;
    Ok(Pdfium::new(bindings))
}

/// PDF 逐页渲染到临时 PNG; 每完成一页回调 `(index0, total, path)`.
/// 渲染后立刻在本线程识别并写 sidecar, 不占用 UI.
pub fn pdf_pages_to_tmp_images_streaming(
    pdf_path: &Path,
    ink_threshold: i32,
    margin: i32,
    mut on_page: impl FnMut(usize, usize, PathBuf),
) -> Result<usize, String> {
    crate::trace::log(&format!("pdf: 开始打开 {}", pdf_path.display()));
    let pdfium = bind_pdfium()?;
    crate::trace::log("pdf: pdfium 已加载");
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| format!("打开 PDF 失败: {e}"))?;
    crate::trace::log("pdf: 文档已打开");

    let tmp_dir = std::env::temp_dir().join(format!(
        "crop_sheet_pdf_{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    if let Ok(mut dirs) = PDF_TMP_DIRS.lock() {
        dirs.push(tmp_dir.clone());
    }

    let stem = pdf_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pdf");
    let n = document.pages().len() as usize;
    crate::trace::log(&format!(
        "pdf: 共 {n} 页, 渲染倍率 {PDF_RENDER_SCALE}, tmp={}",
        tmp_dir.display()
    ));
    if n == 0 {
        return Err(format!("{} 没有页面.", pdf_path.display()));
    }

    for i in 0..n {
        crate::trace::log(&format!("pdf: 渲染 {}/{n} …", i + 1));
        let page = document
            .pages()
            .get(i as u16)
            .map_err(|e| format!("读取第 {} 页失败: {e}", i + 1))?;
        let cfg = PdfRenderConfig::new().scale_page_by_factor(PDF_RENDER_SCALE);
        let image = page
            .render_with_config(&cfg)
            .map_err(|e| format!("渲染第 {} 页失败: {e}", i + 1))?
            .as_image()
            .into_rgb8();
        crate::trace::log(&format!(
            "pdf: 渲染 {}/{n} 完成 {}×{}, 写 PNG …",
            i + 1,
            image.width(),
            image.height()
        ));
        let out_path = tmp_dir.join(format!("{stem}_p{:03}.png", i + 1));
        image
            .save(&out_path)
            .map_err(|e| format!("写临时 PNG 失败: {e}"))?;
        crate::trace::log(&format!("pdf: 识别 {}/{n} …", i + 1));
        crate::detect_cache::detect_and_save(&image, &out_path, ink_threshold, margin);
        crate::trace::log(&format!("pdf: 已写+识别 {}/{n} → 回传 UI", i + 1));
        on_page(i, n, out_path);
    }
    crate::trace::log(&format!("pdf: 全部 {n} 页渲染结束"));
    Ok(n)
}

/// PDF 每页渲染到临时 PNG, 返回按页序的路径列表.
#[allow(dead_code)]
pub fn pdf_pages_to_tmp_images(pdf_path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    pdf_pages_to_tmp_images_streaming(
        pdf_path,
        crate::model::DEFAULT_INK_THRESHOLD,
        crate::model::DEFAULT_MARGIN,
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
}
