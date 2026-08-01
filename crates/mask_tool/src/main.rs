//! 半透明白色蒙版工具 (GPUI).
//!
//! 打开图片后框选区域盖半透明白蒙版, 用于弱化反复段/非本行脚注等.
//! 可传图片路径作为初始文件.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use mask_tool::gui;
use mask_tool::mask::is_image_path;

#[derive(Parser, Debug)]
#[command(
    name = "mask_tool",
    about = "半透明白色蒙版工具: 框选区域盖住反复段/非本行脚注等"
)]
struct Args {
    /// 初始打开的图片 (省略则空白启动)
    image: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if let Some(ref p) = args.image {
        if !is_image_path(p) {
            eprintln!("不是支持的图片文件: {}", p.display());
            return ExitCode::FAILURE;
        }
    }
    gui::run_gui(args.image);
    ExitCode::SUCCESS
}
