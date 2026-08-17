//! 谱面加底色并按指定比例裁切 (谱面完整装进画布).
//!
//! - 无参数 → GPUI 图形界面
//! - 带目录参数 → 命令行批处理 (rayon 多线程)

// release 构建用 windows 子系统, 双击/启动 GUI 时不弹出控制台;
// debug 构建仍保留 console, 方便看日志.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use apply_bg::config;
use apply_bg::gui;
use apply_bg::process::{
    format_aspect, parse_aspect, process_folder, DEFAULT_ASPECT_H, DEFAULT_ASPECT_W,
};

#[derive(Parser, Debug)]
#[command(
    name = "apply_bg",
    about = "谱面加底色并按比例裁切 (谱面完整装进画布, 默认 2560:1440)"
)]
struct Args {
    /// 谱面图片所在目录 (省略则打开 GUI)
    folder: Option<PathBuf>,
    /// 底色图路径 (省略则用 GUI 曾保存的路径)
    #[arg(long)]
    bg: Option<PathBuf>,
    /// 输出目录 (默认: <输入目录>/加底色)
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// 裁切比例 宽:高 (省略则用已保存比例, 再否则 2560:1440)
    #[arg(long, value_name = "W:H")]
    aspect: Option<String>,
    /// 并行线程数 (默认: 逻辑 CPU 数)
    #[arg(short = 'j', long)]
    jobs: Option<usize>,
    /// 强制打开 GUI (即使给了 folder)
    #[arg(long)]
    gui: bool,
}

fn run_cli(args: Args) -> ExitCode {
    let Some(in_dir) = args.folder else {
        gui::run_gui();
        return ExitCode::SUCCESS;
    };
    let cfg = config::load();
    let bg = match args.bg.or_else(|| {
        let s = cfg.bg.trim();
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }) {
        Some(p) => p,
        None => {
            eprintln!(
                "未指定底色: 请传 --bg <路径>, 或先在 GUI 里选择底色 (会写入 {})",
                config::config_dir().display()
            );
            return ExitCode::FAILURE;
        }
    };
    let out_dir = args.out.unwrap_or_else(|| in_dir.join("加底色"));

    let (aspect_w, aspect_h) = if let Some(ref s) = args.aspect {
        match parse_aspect(s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        cfg.aspect_or_default()
    };

    println!("输入: {}", in_dir.display());
    println!("底色: {}", bg.display());
    println!("输出: {}", out_dir.display());
    println!("比例: {}", format_aspect(aspect_w, aspect_h));
    println!("并行: {} 线程", rayon::current_num_threads());
    let _ = io::stdout().flush();

    match process_folder(
        &in_dir,
        &bg,
        &out_dir,
        aspect_w,
        aspect_h,
        args.jobs,
        |i, n, name| {
            println!("PROGRESS {i}/{n} {name}");
            let _ = io::stdout().flush();
        },
    ) {
        Ok(res) => {
            let mut saved = config::load();
            saved.bg = bg.display().to_string();
            saved.in_dir = in_dir.display().to_string();
            saved.out_dir = out_dir.display().to_string();
            if args.aspect.is_some()
                || aspect_w != DEFAULT_ASPECT_W
                || aspect_h != DEFAULT_ASPECT_H
                || !saved.aspect.trim().is_empty()
            {
                saved.aspect = format_aspect(aspect_w, aspect_h);
            }
            config::save(&saved);
            println!(
                "完成: 成功 {} 张 → {} ({:.2}s)",
                res.ok,
                res.out_dir.display(),
                res.elapsed_secs
            );
            for e in &res.errors {
                eprintln!("  失败: {e}");
            }
            if res.errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("错误: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.gui || args.folder.is_none() {
        gui::run_gui();
        ExitCode::SUCCESS
    } else {
        run_cli(args)
    }
}
