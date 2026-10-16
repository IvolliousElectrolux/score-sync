//! 独立矢量修图.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use vector_edit::gui;
use vector_edit::ids::is_pdf_path;
use vector_edit::document::is_scene_dir;

#[derive(Parser, Debug)]
#[command(name = "vector_edit", about = "谱面分块矢量修图")]
struct Args {
    file: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if let Some(ref p) = args.file {
        if !is_pdf_path(p) && !is_scene_dir(p) && p.exists() && !p.is_dir() {
            eprintln!("请打开 PDF 或矢量场景目录: {}", p.display());
            return ExitCode::FAILURE;
        }
    }
    gui::run_gui(args.file);
    ExitCode::SUCCESS
}
