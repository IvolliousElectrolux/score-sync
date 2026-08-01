//! PDF 渲染为临时 PNG (pdfium), 对齐 app.py 的 pdf_pages_to_tmp_images.
//!
//! Windows: `pdfium.dll` 已通过 `include_bytes!` 打进可执行文件,
//! 首次使用时解到本地缓存目录再动态加载. 仍可用环境变量覆盖.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pdfium_render::prelude::*;

const PDF_RENDER_SCALE: f32 = 3.0;

#[cfg(windows)]
const EMBEDDED_PDFIUM: &[u8] = include_bytes!("../assets/pdfium.dll");

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

/// 把内嵌的 pdfium 解到可写缓存 (大小或版本戳不一致才重写).
#[cfg(windows)]
fn ensure_embedded_pdfium() -> Result<PathBuf, String> {
    // 与 assets/pdfium.dll 同源: pypdfium2_raw / pdfium-binaries build 6462
    const STAMP: &str = "6462";
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TEMP").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("score_sync").join("pdfium");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 pdfium 缓存目录失败: {e}"))?;
    let path = dir.join(lib_name());
    let stamp_path = dir.join(".stamp");
    let stamp_ok = std::fs::read_to_string(&stamp_path)
        .map(|s| s.trim() == STAMP)
        .unwrap_or(false);
    let size_ok = std::fs::metadata(&path)
        .map(|meta| meta.len() as usize == EMBEDDED_PDFIUM.len())
        .unwrap_or(false);
    if !(stamp_ok && size_ok) {
        let tmp = dir.join(format!("{}.tmp", lib_name()));
        std::fs::write(&tmp, EMBEDDED_PDFIUM)
            .map_err(|e| format!("写出内嵌 pdfium 失败: {e}"))?;
        std::fs::rename(&tmp, &path)
            .or_else(|_| {
                std::fs::copy(&tmp, &path)?;
                std::fs::remove_file(&tmp)
            })
            .map_err(|e| format!("安装内嵌 pdfium 失败: {e}"))?;
        let _ = std::fs::write(&stamp_path, STAMP);
    }
    Ok(path)
}

#[cfg(not(windows))]
fn ensure_embedded_pdfium() -> Result<PathBuf, String> {
    Err("当前平台未内嵌 pdfium, 请自行安装动态库.".into())
}

fn find_pdfium_path() -> Option<PathBuf> {
    // 1) 环境变量可覆盖
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
    // 2) 优先内嵌 (避免 target/release 旁旧 dll 版本不匹配)
    if let Ok(p) = ensure_embedded_pdfium() {
        return Some(p);
    }
    // 3) 可执行文件旁兜底
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join(lib_name()),
                dir.join("pdfium").join(lib_name()),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn bind_pdfium() -> Result<Pdfium, String> {
    let path = find_pdfium_path().ok_or_else(|| {
        format!(
            "无法准备 pdfium 动态库 ({}). 可设置 PDFIUM_DYNAMIC_LIB_PATH 覆盖.",
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
pub fn pdf_pages_to_tmp_images_streaming(
    pdf_path: &Path,
    mut on_page: impl FnMut(usize, usize, PathBuf),
) -> Result<usize, String> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| format!("打开 PDF 失败: {e}"))?;

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
    if n == 0 {
        return Err(format!("{} 没有页面.", pdf_path.display()));
    }

    for i in 0..n {
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
        let out_path = tmp_dir.join(format!("{stem}_p{:03}.png", i + 1));
        image
            .save(&out_path)
            .map_err(|e| format!("写临时 PNG 失败: {e}"))?;
        on_page(i, n, out_path);
    }
    Ok(n)
}

/// PDF 每页渲染到临时 PNG, 返回按页序的路径列表.
#[allow(dead_code)]
pub fn pdf_pages_to_tmp_images(pdf_path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    pdf_pages_to_tmp_images_streaming(pdf_path, |_, _, p| {
        out.push(p);
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_embedded_pdfium_ok() {
        bind_pdfium().expect("应能加载内嵌 pdfium (build 6462 + pdfium_6406 bindings)");
    }
}
