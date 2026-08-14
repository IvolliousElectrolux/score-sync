//! 曲谱同步 / Score Sync (GPUI).
//!
//! 打开图片或 PDF, 自动识别大谱表行, 跨页组合、蒙版与加底色, 导出竖向拼接切片.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod detect_cache;
mod export;
mod gui;
mod model;
mod page_cache;
mod pdf;
mod project;
mod staff_detect;
mod text_input;
mod trace;
mod update;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use model::is_open_path;
use project::is_project_path;

#[derive(Parser, Debug)]
#[command(
    name = "score_sync",
    about = "曲谱同步一条龙: 分块 / 蒙版 / 加底色 / 工程保存"
)]
struct Args {
    /// 初始打开的图片、PDF 或工程文件 (可多个; 工程文件会单独打开)
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    for p in &args.paths {
        if !p.is_file() {
            eprintln!("文件不存在: {}", p.display());
            return ExitCode::FAILURE;
        }
        if !(is_open_path(p) || is_project_path(p)) {
            eprintln!("不支持的文件类型: {}", p.display());
            return ExitCode::FAILURE;
        }
    }
    // 启动建会话目录; 退出清理会话 tmp (保留工程旁视频池缓存)
    page_cache::init_session();
    trace::init();
    crate::trace::log("main: 即将进入 GUI");
    let _guard = scopeguard_cleanup();
    gui::run_gui(args.paths);
    crate::trace::log("main: GUI 已退出");
    ExitCode::SUCCESS
}

fn scopeguard_cleanup() -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            page_cache::cleanup_session();
        }
    }
    Guard
}
