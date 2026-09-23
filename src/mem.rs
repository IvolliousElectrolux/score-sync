//! 应用侧内存分项: 像素缓冲按宽高×通道估算, 进程工作集走 OS.
//!
//! 这不是分配器级归因 (那种请用 dhat). 目的是回答「页图 / 底色 / 贴图 /
//! 素材池各占多少」, 好对照 `retain_memory_window` 是否真的把工作集按住.

use std::sync::Arc;

use image::RgbImage;

pub fn rgb_bytes(img: &RgbImage) -> u64 {
    img.width() as u64 * img.height() as u64 * 3
}

pub fn rgb_arc_unique_bytes(
    img: &Arc<RgbImage>,
    seen: &mut std::collections::HashSet<usize>,
) -> u64 {
    let p = Arc::as_ptr(img) as usize;
    if !seen.insert(p) {
        0
    } else {
        rgb_bytes(img)
    }
}

pub fn dims_rgb(w: u32, h: u32) -> u64 {
    w as u64 * h as u64 * 3
}

pub fn dims_bgra(w: u32, h: u32) -> u64 {
    w as u64 * h as u64 * 4
}

pub fn fmt_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let x = n as f64;
    if x >= GB {
        format!("{:.2}GB", x / GB)
    } else if x >= MB {
        format!("{:.1}MB", x / MB)
    } else if x >= KB {
        format!("{:.1}KB", x / KB)
    } else {
        format!("{n}B")
    }
}

/// 当前进程工作集 / 专用提交 (pagefile). 失败则两项都是 0.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessMem {
    pub working_set: u64,
    pub private: u64,
}

pub fn process_memory() -> ProcessMem {
    #[cfg(windows)]
    {
        windows_process_memory().unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        ProcessMem::default()
    }
}

#[cfg(windows)]
fn windows_process_memory() -> Option<ProcessMem> {
    use std::mem::MaybeUninit;
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
    let mut st = MaybeUninit::<ProcessMemoryCounters>::uninit();
    unsafe {
        let p = st.as_mut_ptr();
        (*p).cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), p, (*p).cb) == 0 {
            return None;
        }
        Some(ProcessMem {
            working_set: (*p).working_set_size as u64,
            private: (*p).pagefile_usage as u64,
        })
    }
}

/// 大块解码/合成之后请堆把空闲页交回 OS. 默认分配器经常占着工作集不放.
pub fn release_unused_to_os() {
    #[cfg(windows)]
    {
        windows_heap_compact();
    }
}

#[cfg(windows)]
fn windows_heap_compact() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetProcessHeap() -> *mut std::ffi::c_void;
        fn HeapCompact(heap: *mut std::ffi::c_void, flags: u32) -> usize;
    }
    unsafe {
        let _ = HeapCompact(GetProcessHeap(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn fmt_bytes_buckets() {
        assert_eq!(fmt_bytes(0), "0B");
        assert_eq!(fmt_bytes(800), "800B");
        assert_eq!(fmt_bytes(1536), "1.5KB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024), "3.0MB");
    }

    #[test]
    fn rgb_arc_unique_does_not_double_count() {
        let img = Arc::new(RgbImage::from_pixel(10, 20, image::Rgb([1, 2, 3])));
        let mut seen = std::collections::HashSet::new();
        let a = rgb_arc_unique_bytes(&img, &mut seen);
        let b = rgb_arc_unique_bytes(&img, &mut seen);
        assert_eq!(a, 10 * 20 * 3);
        assert_eq!(b, 0);
    }
}
