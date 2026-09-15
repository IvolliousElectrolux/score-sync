//! 手工核对用: 正弦波 + 音名图, 覆盖当前全部转场, 走 `export_async` 出 mp4.
//!
//! 输出: `<repo>/transition_review/review.mp4`
//! 时间轴 (每页 3s, 共 21s, 底色 keep_bg = 暖黄):
//!   C 0-3    淡入→黑 0.0-1.2
//!   D 3-6    硬切
//!   E 6-9    左→右刷入 6.0-7.2
//!   F 9-12   淡出→黑 10.5-12.0
//!   G 12-15  淡入→底色 12.0-13.5
//!   A 15-18  淡出→底色 16.5-18.0
//!   B 18-21  左→右刷入 18.0-19.2

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{Rgba, RgbaImage};
use score_video::export::{export_async, Container, ExportMsg, ExportOptions};
use score_video::model::{AudioClip, FadeKind, FadeSpan, MaterialItem, Timeline, VideoClip};
use uuid::Uuid;

const W: u32 = 1280;
const H: u32 = 720;
const PAGE: f64 = 3.0;
const SR: u32 = 44100;
const FADE_BG: [u8; 3] = [240, 200, 80];

const NOTES: [(&str, f64, [u8; 3], &str); 7] = [
    ("C", 261.626, [229, 57, 53], "fade-in to black"),
    ("D", 293.665, [251, 140, 0], "hard cut"),
    ("E", 329.628, [220, 180, 20], "wipe L->R"),
    ("F", 349.228, [67, 160, 71], "fade-out to black"),
    ("G", 392.000, [30, 136, 229], "fade-in keep-bg"),
    ("A", 440.000, [142, 36, 170], "fade-out keep-bg"),
    ("B", 493.883, [216, 27, 96], "wipe L->R"),
];

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../transition_review")
}

fn write_sine(path: &Path, freq: f64) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (PAGE * SR as f64).round() as u32;
    for i in 0..n {
        let s = ((i as f64) * freq * 2.0 * std::f64::consts::PI / SR as f64).sin() * 18000.0;
        w.write_sample(s as i16).unwrap();
    }
    w.finalize().unwrap();
}

/// 5x7 大写字模, 行是 5 bit (高位在左).
fn glyph(ch: char) -> [u8; 7] {
    match ch {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        ' ' => [0; 7],
        _ => [0b11111, 0b10001, 0b10001, 0b00100, 0b00100, 0b00000, 0b00100],
    }
}

fn blit(img: &mut RgbaImage, mut x: i32, y: i32, text: &str, scale: i32, rgba: [u8; 4]) {
    let color = Rgba(rgba);
    for ch in text.chars() {
        let g = glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (0b10000 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = x + col * scale + dx;
                            let py = y + row as i32 * scale + dy;
                            if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height()
                            {
                                img.put_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                }
            }
        }
        x += 6 * scale;
    }
}

fn prefer_slim_ffmpeg() {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/ffmpeg.exe");
    if !vendor.is_file() {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::fs::copy(&vendor, dir.join("ffmpeg.exe"));
            if let Some(up) = dir.parent() {
                let _ = std::fs::copy(&vendor, up.join("ffmpeg.exe"));
            }
        }
    }
}

fn try_magick_page(png: &Path, note: &str, rgb: [u8; 3], caption: &str) -> bool {
    let font = [
        "C:/Windows/Fonts/arial.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyhbd.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.is_file());
    let Some(font) = font else {
        return false;
    };
    let fill = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
    let status = Command::new("magick")
        .args([
            "-size",
            &format!("{W}x{H}"),
            &format!("xc:{fill}"),
            "-font",
        ])
        .arg(&font)
        .args([
            "-fill",
            "white",
            "-gravity",
            "center",
            "-pointsize",
            "280",
            "-annotate",
            "+0-40",
            note,
            "-pointsize",
            "42",
            "-annotate",
            "+0+240",
            caption,
        ])
        .arg(png)
        .status();
    status.map(|s| s.success() && png.is_file()).unwrap_or(false)
}

fn write_page_png(png: &Path, note: &str, rgb: [u8; 3], caption: &str) {
    if try_magick_page(png, note, rgb, caption) {
        return;
    }
    let mut img = RgbaImage::from_pixel(W, H, Rgba([rgb[0], rgb[1], rgb[2], 255]));
    for x in 0..W {
        for y in 0..48 {
            img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
        }
        for y in (H - 96)..H {
            img.put_pixel(x, y, Rgba([20, 20, 20, 255]));
        }
    }
    blit(&mut img, 80, 8, note, 5, [rgb[0], rgb[1], rgb[2], 255]);
    let letter_w = 5 * 28;
    let lx = ((W as i32) - letter_w) / 2;
    blit(&mut img, lx, 220, note, 28, [255, 255, 255, 255]);
    blit(&mut img, 40, (H as i32) - 72, caption, 4, [255, 220, 80, 255]);
    img.save(png).unwrap();
}

fn fade(kind: FadeKind, start: f64, end: f64, keep_bg: bool) -> FadeSpan {
    FadeSpan {
        id: Uuid::new_v4(),
        start,
        end,
        kind,
        keep_bg,
    }
}

fn build_review_project(dir: &Path) -> (Timeline, Vec<MaterialItem>) {
    std::fs::create_dir_all(dir).unwrap();
    let mut pool = Vec::new();
    let mut tl = Timeline::new();
    for (i, (note, hz, rgb, cap)) in NOTES.iter().enumerate() {
        let png = dir.join(format!("{note}.png"));
        let wav = dir.join(format!("{note}.wav"));
        if !png.is_file() {
            write_page_png(&png, note, *rgb, cap);
        }
        if !wav.is_file() {
            write_sine(&wav, *hz);
        }
        let (w, h) = image::image_dimensions(&png).unwrap();
        pool.push(MaterialItem {
            group_id: (*note).into(),
            label: (*note).into(),
            cache_path: png,
            width: w,
            height: h,
        });
        let start = i as f64 * PAGE;
        tl.video_clips.push(VideoClip {
            id: Uuid::new_v4(),
            group_id: (*note).into(),
            start,
            end: start + PAGE,
        });
        tl.audio_clips.push(AudioClip {
            id: Uuid::new_v4(),
            path: wav,
            label: (*note).into(),
            duration: PAGE,
            offset: 0.0,
        });
    }
    tl.fades = vec![
        fade(FadeKind::In, 0.0, 1.2, false),
        fade(FadeKind::WipeLtr, 6.0, 7.2, false),
        fade(FadeKind::Out, 10.5, 12.0, false),
        fade(FadeKind::In, 12.0, 13.5, true),
        fade(FadeKind::Out, 16.5, 18.0, true),
        fade(FadeKind::WipeLtr, 18.0, 19.2, false),
    ];
    (tl, pool)
}

fn export_once(tl: Timeline, pool: Vec<MaterialItem>, out: PathBuf, log: bool) -> std::time::Duration {
    let opts = ExportOptions {
        container: Container::Mp4,
        width: W,
        height: H,
        fps: 30,
        crf: 18,
        out_path: out,
        fade_bg_rgb: FADE_BG,
    };
    let t0 = std::time::Instant::now();
    let rx = export_async(tl, pool, opts);
    loop {
        match rx.recv_blocking() {
            Ok(ExportMsg::Progress(s)) => {
                if log {
                    eprintln!("  {s}");
                }
            }
            Ok(ExportMsg::Done(Ok(_))) => break,
            Ok(ExportMsg::Done(Err(e))) => panic!("export failed: {e}"),
            Err(_) => panic!("export channel closed"),
        }
    }
    t0.elapsed()
}

fn ffmpeg_sidecar_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            if let Some(up) = dir.parent() {
                dirs.push(up.to_path_buf());
            }
        }
    }
    dirs
}

fn install_ffmpeg_sidecar(src: &Path) {
    let name = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    for dir in ffmpeg_sidecar_dirs() {
        let _ = std::fs::copy(src, dir.join(&name));
    }
}

fn time_with_ffmpeg(label: &str, src: &Path, dir: &Path, repeats: usize) -> Vec<f64> {
    install_ffmpeg_sidecar(src);
    let out = dir.join(format!("bench_{label}.mp4"));
    let mut secs = Vec::new();
    for i in 0..=repeats {
        let (tl, pool) = build_review_project(dir);
        let dt = export_once(tl, pool, out.clone(), false);
        let s = dt.as_secs_f64();
        let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        if i == 0 {
            eprintln!("  {label} warmup {s:.2}s  {bytes} bytes");
        } else {
            eprintln!("  {label} run {i} {s:.2}s  {bytes} bytes");
            secs.push(s);
        }
    }
    secs
}

#[test]
fn export_all_current_transitions() {
    prefer_slim_ffmpeg();
    let dir = out_dir();
    let (tl, pool) = build_review_project(&dir);
    let out = dir.join("review.mp4");
    let dt = export_once(tl, pool, out.clone(), true);
    assert!(out.is_file());
    let bytes = std::fs::metadata(&out).unwrap().len();
    eprintln!(
        "review video: {} ({bytes} bytes, {:.2}s)",
        out.canonicalize().unwrap().display(),
        dt.as_secs_f64()
    );
}

/// 同一条 21s / 1280x720 / CRF18 时间轴, 裁剪 sidecar 对比本机完整 ffmpeg.
/// 完整版找不到就跳过 (CI 没有 gyan/choco 包).
#[test]
fn export_slim_vs_full_ffmpeg_timing() {
    let dir = out_dir();
    let slim = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/ffmpeg.exe");
    if !slim.is_file() {
        eprintln!("skip: vendor/ffmpeg.exe missing");
        return;
    }
    let mut fulls: Vec<(PathBuf, &str)> = Vec::new();
    if let Ok(home) = std::env::var("USERPROFILE") {
        let scoop = PathBuf::from(home).join(r"scoop\apps\ffmpeg\current\bin\ffmpeg.exe");
        if scoop.is_file() {
            fulls.push((scoop, "gyan-full-7.1.1"));
        }
    }
    let choco = PathBuf::from(r"C:\ProgramData\chocolatey\lib\ffmpeg\tools\ffmpeg\bin\ffmpeg.exe");
    if choco.is_file() {
        fulls.push((choco, "gyan-essentials-99mb"));
    }
    if fulls.is_empty() {
        eprintln!("skip: no full ffmpeg on this machine");
        return;
    }

    eprintln!("same export: 21s 1280x720 30fps crf18 stillimage/medium");
    let slim_secs = time_with_ffmpeg("slim", &slim, &dir, 2);
    for (path, label) in &fulls {
        let full_secs = time_with_ffmpeg(label, path, &dir, 2);
        let slim_mean = slim_secs.iter().sum::<f64>() / slim_secs.len() as f64;
        let full_mean = full_secs.iter().sum::<f64>() / full_secs.len() as f64;
        let pct = (slim_mean / full_mean - 1.0) * 100.0;
        eprintln!(
            "==> slim {:.2}s vs {label} {:.2}s  ({:+.0}% wall vs full)",
            slim_mean, full_mean, pct
        );
    }
    prefer_slim_ffmpeg();
}
