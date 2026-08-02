//! 独立调试入口: 传入一个图片文件夹当素材池, 可选传一个音频文件.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clap::Parser;

use score_video::gui;

#[derive(Parser, Debug)]
#[command(
    name = "score_video",
    about = "视频轨道编辑与导出 (调试用: 传入图片文件夹 + 可选音频)"
)]
struct Args {
    /// 素材图片所在目录 (按文件名排序作为素材池顺序)
    folder: Option<PathBuf>,
    /// 音频文件路径
    #[arg(long)]
    audio: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let mut images = Vec::new();
    if let Some(dir) = args.folder {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| {
                            matches!(
                                e.to_ascii_lowercase().as_str(),
                                "png" | "jpg" | "jpeg" | "bmp" | "webp"
                            )
                        })
                        .unwrap_or(false)
                })
                .collect();
            paths.sort();
            images = paths;
        }
    }
    gui::run_gui(images, args.audio);
}
