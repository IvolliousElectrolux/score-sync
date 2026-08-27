//! 会话页图磁盘备份与内存滑动窗口 (默认 ±4, 高清页自动收窄).
//!
//! - 本会话打开的所有页 PNG 落在 `score_sync_session_<uuid>/`
//! - 内存最多保留当前页前后各 [`window_radius_for_bytes`] 页
//! - 退出时清理会话目录; 工程旁 `.staffcrop.cache` 不由此清理

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::RgbImage;
use uuid::Uuid;

/// 当前页前后各保留的页数 → 至多 2*R+1 张在内存 (小图).
pub const WINDOW_RADIUS: usize = 4;

/// 单页 RGB 超过此值则窗口收到 ±2 (约 2800×2800).
const HEAVY_PAGE_BYTES: u64 = 24 * 1024 * 1024;
/// 单页 RGB 超过此值则窗口收到 ±1 (约 4000×4000). 9 张高清页会把
/// 工作集顶到近 GB, 任意 UI 重绘都卡半拍.
const HUGE_PAGE_BYTES: u64 = 48 * 1024 * 1024;

/// 按单页解码体积收缩内存窗口; 高清谱不要再硬留 ±4.
pub fn window_radius_for_bytes(page_bytes: u64) -> usize {
    if page_bytes >= HUGE_PAGE_BYTES {
        1
    } else if page_bytes >= HEAVY_PAGE_BYTES {
        2
    } else {
        WINDOW_RADIUS
    }
}

static SESSION_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static EXTRA_TMP_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// 启动时清掉异常退出残留的旧会话目录, 并创建本次会话目录.
pub fn init_session() -> PathBuf {
    cleanup_stale_sessions();
    let dir = std::env::temp_dir().join(format!(
        "score_sync_session_{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ));
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut g) = SESSION_DIR.lock() {
        *g = Some(dir.clone());
    }
    dir
}

pub fn session_dir() -> PathBuf {
    if let Ok(g) = SESSION_DIR.lock() {
        if let Some(d) = g.as_ref() {
            return d.clone();
        }
    }
    init_session()
}

/// 登记额外临时目录 (如 pdfium 逐页输出目录), 退出时一并删除.
#[allow(dead_code)]
pub fn register_extra_tmp(dir: PathBuf) {
    if let Ok(mut dirs) = EXTRA_TMP_DIRS.lock() {
        dirs.push(dir);
    }
}

fn cleanup_stale_sessions() {
    let tmp = std::env::temp_dir();
    let Ok(rd) = std::fs::read_dir(&tmp) else {
        return;
    };
    for ent in rd.flatten() {
        let name = ent.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("score_sync_session_") || s.starts_with("crop_sheet_pdf_") {
            let _ = std::fs::remove_dir_all(ent.path());
        }
    }
}

/// 退出时清理本次会话与登记的额外 tmp (不清工程旁 `.staffcrop.cache`).
pub fn cleanup_session() {
    if let Ok(mut g) = SESSION_DIR.lock() {
        if let Some(d) = g.take() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    if let Ok(mut dirs) = EXTRA_TMP_DIRS.lock() {
        for d in dirs.drain(..) {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    // 兼容旧 pdf 清理入口
    crate::pdf::cleanup_pdf_tmps();
}

/// 把已有文件拷进会话目录, 返回会话内路径.
pub fn ingest_file(src: &Path, preferred_name: &str) -> Result<PathBuf, String> {
    let dir = session_dir();
    let safe = preferred_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let stem = Path::new(&safe)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    let ext = Path::new(&safe)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png");
    let mut dest = dir.join(format!("{stem}.{ext}"));
    let mut n = 0u32;
    while dest.exists() {
        n += 1;
        dest = dir.join(format!("{stem}_{n}.{ext}"));
    }
    std::fs::copy(src, &dest).map_err(|e| format!("拷贝页图到会话目录失败: {e}"))?;
    Ok(dest)
}

/// 将内存图写成会话 PNG, 返回路径.
pub fn write_rgb_png(image: &RgbImage, preferred_name: &str) -> Result<PathBuf, String> {
    let dir = session_dir();
    let safe: String = preferred_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = Path::new(&safe)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    let mut dest = dir.join(format!("{stem}.png"));
    let mut n = 0u32;
    while dest.exists() {
        n += 1;
        dest = dir.join(format!("{stem}_{n}.png"));
    }
    image
        .save(&dest)
        .map_err(|e| format!("写入会话页图失败: {e}"))?;
    Ok(dest)
}

/// 从磁盘解码 RGB.
pub fn load_rgb(path: &Path) -> Result<RgbImage, String> {
    image::open(path)
        .map_err(|e| format!("读取页图失败 ({}): {e}", path.display()))
        .map(|i| i.to_rgb8())
}

/// 「组织页面」缩略图最长边 (像素). 与 GUI `ORG_THUMB_MAX_SIDE` 相同.
pub const ORG_THUMB_SIDE: u32 = 192;

/// 会话页图旁的组织缩略图: `foo.png` → `foo.org.jpg`.
pub fn org_thumb_path(disk_png: &Path) -> PathBuf {
    let stem = disk_png
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    disk_png.with_file_name(format!("{stem}.org.jpg"))
}

/// 按最长边缩小 (已小于等于目标则 clone). 大图用 `thumbnail` 整数抽样, 比
/// Triangle 整页卷积快一个数量级, 192px 预览足够.
pub fn shrink_rgb_max(rgb: &RgbImage, max_side: u32) -> RgbImage {
    let w = rgb.width().max(1);
    let h = rgb.height().max(1);
    let m = w.max(h);
    if m <= max_side {
        return rgb.clone();
    }
    let tw = ((w as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    let th = ((h as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    image::imageops::thumbnail(rgb, tw, th)
}

/// 把已缩好的组织缩略图写成 JPEG sidecar.
pub fn save_org_thumb(thumb: &RgbImage, disk_png: &Path) -> Result<(), String> {
    let path = org_thumb_path(disk_png);
    thumb
        .save_with_format(&path, image::ImageFormat::Jpeg)
        .map_err(|e| format!("写组织缩略图失败: {e}"))
}

/// 从全分辨率页图写出组织缩略图 sidecar (导入/写 PNG 时顺手做, 打开弹窗就不用再解全图).
pub fn write_org_thumb(rgb: &RgbImage, disk_png: &Path) -> Result<(), String> {
    let thumb = shrink_rgb_max(rgb, ORG_THUMB_SIDE);
    save_org_thumb(&thumb, disk_png)
}

/// 复制一份会话内页图文件 (用于「复制本页」).
pub fn duplicate_disk_png(src: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("page.png");
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    let dest = ingest_file(src, &format!("{stem}_copy.png"))?;
    let src_thumb = org_thumb_path(src);
    if src_thumb.is_file() {
        let _ = std::fs::copy(&src_thumb, org_thumb_path(&dest));
    }
    Ok(dest)
}

/// 工程旁视频池缓存目录: `foo.staffcrop` → `foo.staffcrop.cache`.
pub fn project_cache_dir(project_path: &Path) -> PathBuf {
    let mut s = project_path.as_os_str().to_os_string();
    s.push(".cache");
    PathBuf::from(s)
}

#[allow(dead_code)]
pub fn pool_cache_png(project_path: &Path, group_id: &str) -> PathBuf {
    project_cache_dir(project_path)
        .join("pool")
        .join(format!("{group_id}.png"))
}

#[allow(dead_code)]
pub fn pool_thumb_png(project_path: &Path, group_id: &str) -> PathBuf {
    project_cache_dir(project_path)
        .join("pool")
        .join(format!("{group_id}_thumb.png"))
}

/// 删掉当前组合列表里已经没有的缓存 PNG, 避免删组后磁盘一直涨.
pub fn prune_pool_cache(cache_root: &Path, live_group_ids: &std::collections::HashSet<String>) {
    let Ok(rd) = std::fs::read_dir(cache_root) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".png") else {
            continue;
        };
        let gid = stem.strip_suffix("_thumb").unwrap_or(stem);
        if !live_group_ids.contains(gid) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 可用物理内存字节 (启发式). 失败时给一个保守默认.
pub fn available_memory_bytes() -> u64 {
    #[cfg(windows)]
    {
        windows_available_memory().unwrap_or(2 * 1024 * 1024 * 1024)
    }
    #[cfg(not(windows))]
    {
        2 * 1024 * 1024 * 1024
    }
}

/// 根据可用内存与单任务峰值估算并发度 (至少 1).
pub fn concurrency_for_peak(peak_bytes: u64) -> usize {
    let avail = available_memory_bytes();
    let budget = (avail as f64 * 0.45) as u64;
    let peak = peak_bytes.max(64 * 1024 * 1024);
    ((budget / peak).max(1) as usize).clamp(1, 4)
}

#[cfg(windows)]
fn windows_available_memory() -> Option<u64> {
    use std::mem::MaybeUninit;
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(lpBuffer: *mut MemoryStatusEx) -> i32;
    }
    let mut st = MaybeUninit::<MemoryStatusEx>::uninit();
    unsafe {
        let p = st.as_mut_ptr();
        (*p).dw_length = std::mem::size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(p) == 0 {
            return None;
        }
        Some((*p).ull_avail_phys)
    }
}

#[cfg(test)]
mod window_radius_tests {
    use super::window_radius_for_bytes;
    use super::org_thumb_path;
    use std::path::PathBuf;

    #[test]
    fn small_pages_keep_default_radius() {
        assert_eq!(window_radius_for_bytes(8 * 1024 * 1024), 4);
    }

    #[test]
    fn heavy_pages_shrink_to_two() {
        assert_eq!(window_radius_for_bytes(30 * 1024 * 1024), 2);
    }

    #[test]
    fn huge_pages_shrink_to_one() {
        assert_eq!(window_radius_for_bytes(80 * 1024 * 1024), 1);
    }

    #[test]
    fn org_thumb_sidecar_keeps_stem() {
        let p = PathBuf::from("tmp/Chopin_p012.png");
        assert_eq!(
            org_thumb_path(&p).file_name().and_then(|s| s.to_str()),
            Some("Chopin_p012.org.jpg")
        );
    }
}
