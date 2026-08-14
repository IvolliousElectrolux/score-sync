//! 逐步诊断日志: 写 stderr, 并追加到 `%TEMP%/score_sync_trace.log`.
//!
//! 设置环境变量 `SCORE_SYNC_TRACE=1` 时, Windows 会挂上控制台,
//! 这样 `cargo run -r` 也能在终端看到输出.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

static START: Mutex<Option<Instant>> = Mutex::new(None);
static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init() {
    if let Ok(mut g) = START.lock() {
        *g = Some(Instant::now());
    }
    let path = std::env::temp_dir().join("score_sync_trace.log");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    {
        let _ = writeln!(f, "=== score_sync trace {} ===", chrono_now());
        let _ = writeln!(f, "log: {}", path.display());
    }
    if let Ok(mut g) = LOG_PATH.lock() {
        *g = Some(path.clone());
    }
    maybe_attach_console();
    log(&format!("trace 已启动 → {}", path.display()));
}

fn chrono_now() -> String {
    // 不引入 chrono: 用系统本地时间粗格式
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix={secs}")
}

fn elapsed_ms() -> u128 {
    START
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0)
}

pub fn log(msg: &str) {
    if std::env::var_os("SCORE_SYNC_TRACE").is_none() {
        return;
    }
    let line = format!("[+{:>8}ms] {msg}", elapsed_ms());
    eprintln!("{line}");
    let path = LOG_PATH.lock().ok().and_then(|g| g.clone());
    if let Some(path) = path {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

fn maybe_attach_console() {
    #[cfg(windows)]
    {
        let on = std::env::var_os("SCORE_SYNC_TRACE").is_some();
        if !on {
            return;
        }
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                let _ = AllocConsole();
            }
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn AllocConsole() -> i32;
    fn AttachConsole(pid: u32) -> i32;
}
