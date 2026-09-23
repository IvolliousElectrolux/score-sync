//! 独立 P 图工具.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use photo_edit::gui;
use photo_edit::is_image_path;

#[derive(Parser, Debug)]
#[command(name = "photo_edit", about = "谱面分块 P 图")]
struct Args {
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
