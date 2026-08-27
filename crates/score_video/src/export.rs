//! ffmpeg 导出流水线.
//!
//! 预览跟音频时钟 (秒). 导出不再用 concat/tpad/movie 拼静帧: 那些接缝会
//! 每页丢掉约 1 帧的时长, 一个小时下来就是好几秒, 而且方向和旧的 duration+fps
//! 滤镜相反 (旧的偏晚, concat 偏早).
//!
//! 现在按时间轴算出每一帧该显示哪一页, 把 RGBA 像素按 `-framerate fps` 写进
//! ffmpeg stdin. 输出时长 = 帧数 / fps, 跟音频 `-t` 同一把尺子.

use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use image::RgbaImage;

use crate::model::{AudioClip, FadeKind, MaterialItem, Timeline};

/// ffmpeg 可执行文件路径: 优先找程序自身同目录下的 `ffmpeg(.exe)` (发行包
/// 自带, 用户不用单独装 ffmpeg), 找不到再退回 PATH 里的 `ffmpeg` (方便开发
/// 环境直接用系统装的那份).
pub(crate) fn ffmpeg_path() -> PathBuf {
    let name = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from(name)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp4,
    Mkv,
}

impl Container {
    pub fn ext(&self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
        }
    }

    /// 音频编码: MP4 用有损 AAC (兼容性最好), MKV 用无损 FLAC (体积更大,
    /// 音质无损).
    pub fn audio_codec(&self) -> &'static str {
        match self {
            Container::Mp4 => "aac",
            Container::Mkv => "flac",
        }
    }

    fn audio_ext(&self) -> &'static str {
        match self {
            Container::Mp4 => "m4a",
            Container::Mkv => "flac",
        }
    }

    /// UI 提示文案: 说明两种容器对应的音频压缩方式.
    pub fn audio_hint(&self) -> &'static str {
        match self {
            Container::Mp4 => "MP4: 音频转码为有损 AAC, 兼容性最好, 体积较小.",
            Container::Mkv => "MKV: 音频保留为无损 FLAC, 音质无损, 体积较大.",
        }
    }
}

#[derive(Clone)]
pub struct ExportOptions {
    pub container: Container,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub out_path: PathBuf,
    /// 「保持背景为底色」时 ffmpeg fade 的目标色.
    pub fade_bg_rgb: [u8; 3],
}

impl ExportOptions {
    /// 分辨率不再让用户选: 加底色后各图比例相同, 但高矮谱面像素可能不同
    /// (contain: 宽条上下补边、高图左右补边). 取池中面积最大的一张, 其余
    /// dump 时 `fit_pad` 装进同一画布. 素材池为空时给一个保守的默认值
    /// (此时也导不出任何内容, 不会真正用到).
    ///
    /// libx264 + `yuv420p` 要求宽高都是偶数, 奇数分辨率会在打开编码器时直接
    /// 报 `Error while opening encoder - maybe incorrect parameters such as
    /// bit_rate, rate, width or height`. 这里向上取偶, 再在 dump 时用黑边补齐.
    pub fn size_from_pool(pool: &[MaterialItem]) -> (u32, u32) {
        let (w, h) = pool
            .iter()
            .map(|m| (m.width, m.height))
            .max_by_key(|&(w, h)| (w as u64).saturating_mul(h as u64))
            .unwrap_or((1920, 1080));
        (even_dim(w), even_dim(h))
    }
}

/// 向上取偶且至少为 2 (x264 / yuv420p 硬性要求).
fn even_dim(n: u32) -> u32 {
    let n = n.max(2);
    if n % 2 == 0 { n } else { n + 1 }
}

pub enum ExportMsg {
    Progress(String),
    Done(Result<PathBuf, String>),
}

/// 时间轴上连续相同画面 (同一页 + 同一淡入淡出状态) 的一段, 单位是整帧.
struct FrameRun {
    gid: String,
    frames: u64,
    fade: Option<(FadeKind, bool)>,
}

/// 第一个 PTS >= `t` 的帧号 (`ceil(t * fps)`).
///
/// 第 `F` 帧从 `F / fps` 开始显示. 若用 `round(t * fps)`, 翻页会发生在切点
/// *之前* 的那一帧起点上, 导出就会相对预览向前偏.
fn frame_at_or_after(t: f64, fps: u32) -> u64 {
    let fps = fps.max(1) as f64;
    if !t.is_finite() || t <= 0.0 {
        return 0;
    }
    let x = t * fps;
    let r = x.round();
    if (x - r).abs() <= 1e-9 {
        r.max(0.0) as u64
    } else {
        x.ceil().max(0.0) as u64
    }
}

fn frames_to_seconds(frames: u64, fps: u32) -> f64 {
    frames as f64 / fps.max(1) as f64
}

fn frame_time(f: u64, fps: u32) -> f64 {
    f as f64 / fps.max(1) as f64
}

/// 先冻结时间轴, 再按预览同一套 `covering_*` 把每一帧贴到 `t = F/fps`.
fn build_frame_runs(timeline: &Timeline, fps: u32) -> Vec<FrameRun> {
    let fps = fps.max(1);
    let end_f = frame_at_or_after(timeline.timeline_end(), fps);
    if end_f == 0 {
        return Vec::new();
    }

    let mut cuts: Vec<u64> = vec![0, end_f];
    for c in &timeline.video_clips {
        cuts.push(frame_at_or_after(c.start, fps).min(end_f));
        cuts.push(frame_at_or_after(c.end, fps).min(end_f));
    }
    for fade in &timeline.fades {
        cuts.push(frame_at_or_after(fade.start, fps).min(end_f));
        cuts.push(frame_at_or_after(fade.end, fps).min(end_f));
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs: Vec<FrameRun> = Vec::new();
    for w in cuts.windows(2) {
        let f0 = w[0];
        let f1 = w[1];
        if f1 <= f0 {
            continue;
        }
        let t = frame_time(f0, fps);
        let gid = timeline
            .covering_clip(t)
            .map(|c| c.group_id.clone())
            .unwrap_or_else(|| BLACK_KEY.to_string());
        let fade = timeline
            .covering_fade(t)
            .map(|f| (f.kind, f.keep_bg));
        if let Some(last) = runs.last_mut() {
            if last.gid == gid && last.fade == fade {
                last.frames += f1 - f0;
                continue;
            }
        }
        runs.push(FrameRun {
            gid,
            frames: f1 - f0,
            fade,
        });
    }
    runs
}

const BLACK_KEY: &str = "__black__";

struct WorkDir(PathBuf);

impl WorkDir {
    fn new() -> std::io::Result<Self> {
        let dir = std::env::temp_dir()
            .join("score_video")
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 把图片等比缩放到不超过 `target_w x target_h`, 再用不透明黑色居中补齐到
/// 目标尺寸 (等价于以前交给 ffmpeg `scale=...:force_original_aspect_ratio=
/// decrease,pad=...` 做的事, 挪到这边用 Rust 提前算好). 已经是目标尺寸就
/// 直接照抄, 不用再走一遍缩放.
///
/// 这样做是因为素材池里各图片理论上尺寸该完全一致 (加底色后统一画布), 但
/// 只要有一张不完全一致 (或黑场帧尺寸算法不完全对齐), ffmpeg 拿 concat
/// demuxer 顺序喂图给 `scale,pad,fps,format` 这条 `-vf` 链时, 中途切换到不同
/// 尺寸/像素格式的输入会触发它重新协商 filter graph, 这个"重新协商"在部分
/// ffmpeg 版本上并不稳定, 会报 `Error reinitializing filters!` /
/// `Invalid argument` 而直接失败. 提前在这里统一好尺寸和像素格式 (都是不
/// 带透明的 RGBA, alpha 全 255), 喂给 ffmpeg 的每一帧就完全一致, 不会再触发
/// 这个重协商路径.
fn fit_pad(img: &RgbaImage, target_w: u32, target_h: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let target_w = even_dim(target_w);
    let target_h = even_dim(target_h);
    if w == target_w && h == target_h {
        return img.clone();
    }
    let scale = (target_w as f64 / w.max(1) as f64).min(target_h as f64 / h.max(1) as f64);
    let nw = ((w as f64 * scale).round() as u32).clamp(1, target_w);
    let nh = ((h as f64 * scale).round() as u32).clamp(1, target_h);
    // 缩放后的宽高也取偶, 避免 overlay 到偶数画布时出现半像素偏差.
    let nw = even_dim(nw).min(target_w);
    let nh = even_dim(nh).min(target_h);
    let resized = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut canvas = RgbaImage::from_pixel(target_w, target_h, image::Rgba([0, 0, 0, 255]));
    let ox = ((target_w - nw) / 2) as i64;
    let oy = ((target_h - nh) / 2) as i64;
    image::imageops::overlay(&mut canvas, &resized, ox, oy);
    canvas
}

fn load_pool_rgba(
    pool: &[MaterialItem],
    target_w: u32,
    target_h: u32,
) -> Result<HashMap<String, Vec<u8>>, String> {
    let target_w = even_dim(target_w);
    let target_h = even_dim(target_h);
    let mut map = HashMap::new();
    for item in pool {
        let rgba = item.load_rgba()?;
        let (w, h) = rgba.dimensions();
        let img = if w == target_w && h == target_h {
            rgba
        } else {
            fit_pad(&rgba, target_w, target_h)
        };
        map.insert(item.group_id.clone(), img.into_raw());
    }
    Ok(map)
}

fn fade_overlay_at(timeline: &Timeline, t: f64, fade_bg: [u8; 3]) -> Option<(f32, [u8; 3])> {
    let fade = timeline.covering_fade(t)?;
    let span = (fade.end - fade.start).max(1e-6);
    let p = ((t - fade.start) / span).clamp(0.0, 1.0);
    let alpha = match fade.kind {
        FadeKind::In => 1.0 - p,
        FadeKind::Out => p,
    } as f32;
    let bg = if fade.keep_bg { fade_bg } else { [0, 0, 0] };
    Some((alpha, bg))
}

fn apply_fade_overlay(src: &[u8], dst: &mut [u8], alpha: f32, bg: [u8; 3]) {
    let a = alpha.clamp(0.0, 1.0);
    let ia = 1.0 - a;
    for (out, px) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        out[0] = (px[0] as f32 * ia + bg[0] as f32 * a + 0.5) as u8;
        out[1] = (px[1] as f32 * ia + bg[1] as f32 * a + 0.5) as u8;
        out[2] = (px[2] as f32 * ia + bg[2] as f32 * a + 0.5) as u8;
        out[3] = 255;
    }
}

fn run_ffmpeg(
    args: &[std::ffi::OsString],
    step: &str,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    run_ffmpeg_in(args, step, tx, None)
}

fn run_ffmpeg_in(
    args: &[std::ffi::OsString],
    step: &str,
    tx: &async_channel::Sender<ExportMsg>,
    cwd: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg_path());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        crate::error::Error::FfmpegSpawn {
            step: step.to_string(),
            source: e,
        }
        .to_string()
    })?;

    let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let reader_handle = child.stderr.take().map(|stderr| {
        let tx = tx.clone();
        let step = step.to_string();
        let tail = tail.clone();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stderr);
            let mut buf: Vec<u8> = Vec::new();
            let mut last_sent: Option<Instant> = None;
            for byte in reader.by_ref().bytes() {
                let Ok(b) = byte else { break };
                if b != b'\n' && b != b'\r' {
                    buf.push(b);
                    continue;
                }
                if buf.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(&buf).trim().to_string();
                buf.clear();
                if text.is_empty() {
                    continue;
                }
                if let Ok(mut t) = tail.lock() {
                    t.push_str(&text);
                    t.push('\n');
                    let len = t.len();
                    if len > 6000 {
                        let cut = len - 6000;
                        t.drain(0..cut);
                    }
                }
                let should_send = last_sent
                    .map(|t| t.elapsed() >= Duration::from_millis(150))
                    .unwrap_or(true);
                if should_send {
                    last_sent = Some(Instant::now());
                    let _ = tx.send_blocking(ExportMsg::Progress(format!("[{step}] {text}")));
                }
            }
        })
    });

    let status = child.wait().map_err(|e| {
        crate::error::Error::Ffmpeg {
            step: step.to_string(),
            detail: format!("等待进程结束失败: {e}"),
        }
        .to_string()
    })?;
    if let Some(h) = reader_handle {
        let _ = h.join();
    }
    if !status.success() {
        let detail = tail.lock().map(|t| t.trim().to_string()).unwrap_or_default();
        let detail = if detail.is_empty() {
            format!("退出码 {:?}", status.code())
        } else {
            format!("退出码 {:?}:\n{detail}", status.code())
        };
        return Err(crate::error::Error::Ffmpeg {
            step: step.to_string(),
            detail,
        }
        .to_string());
    }
    Ok(())
}

fn os(s: impl Into<String>) -> std::ffi::OsString {
    std::ffi::OsString::from(s.into())
}
fn os_path(p: &Path) -> std::ffi::OsString {
    p.as_os_str().to_owned()
}

fn encode_raw_video(
    images: &HashMap<String, Vec<u8>>,
    timeline: &Timeline,
    runs: &[FrameRun],
    n_frames: u64,
    opts: &ExportOptions,
    out_path: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let fps = opts.fps.max(1);
    let w = even_dim(opts.width);
    let h = even_dim(opts.height);
    let frame_len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    let args = vec![
        os("-y"),
        os("-f"),
        os("rawvideo"),
        os("-pix_fmt"),
        os("rgba"),
        os("-s"),
        os(format!("{w}x{h}")),
        os("-framerate"),
        os(fps.to_string()),
        os("-i"),
        os("-"),
        os("-frames:v"),
        os(n_frames.to_string()),
        os("-vf"),
        os("format=yuv420p"),
        os("-c:v"),
        os("libx264"),
        os("-pix_fmt"),
        os("yuv420p"),
        os("-preset"),
        os("medium"),
        os("-crf"),
        os(opts.crf.to_string()),
        os("-tune"),
        os("stillimage"),
        os("-video_track_timescale"),
        os(fps.to_string()),
        os("-an"),
        os_path(out_path),
    ];

    let mut cmd = Command::new(ffmpeg_path());
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        crate::error::Error::FfmpegSpawn {
            step: "按帧编码".to_string(),
            source: e,
        }
        .to_string()
    })?;

    let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let reader_handle = child.stderr.take().map(|stderr| {
        let tx = tx.clone();
        let tail = tail.clone();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stderr);
            let mut buf: Vec<u8> = Vec::new();
            let mut last_sent: Option<Instant> = None;
            for byte in reader.by_ref().bytes() {
                let Ok(b) = byte else { break };
                if b != b'\n' && b != b'\r' {
                    buf.push(b);
                    continue;
                }
                if buf.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(&buf).trim().to_string();
                buf.clear();
                if text.is_empty() {
                    continue;
                }
                if let Ok(mut t) = tail.lock() {
                    t.push_str(&text);
                    t.push('\n');
                    let len = t.len();
                    if len > 6000 {
                        let cut = len - 6000;
                        t.drain(0..cut);
                    }
                }
                let should_send = last_sent
                    .map(|t| t.elapsed() >= Duration::from_millis(150))
                    .unwrap_or(true);
                if should_send {
                    last_sent = Some(Instant::now());
                    let _ = tx.send_blocking(ExportMsg::Progress(format!("[按帧编码] {text}")));
                }
            }
        })
    });

    let write_result = (|| -> Result<(), String> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin 不可写".to_string())?;
        let mut stdin = BufWriter::with_capacity(frame_len.max(1) * 2, stdin);
        let mut scratch = vec![0u8; frame_len];
        let mut written = 0u64;
        let mut last_progress = Instant::now();
        for run in runs {
            let pixels = images
                .get(&run.gid)
                .ok_or_else(|| format!("素材 {} 缺少图片, 无法导出", run.gid))?;
            if pixels.len() != frame_len {
                return Err(format!(
                    "素材 {} 像素尺寸不匹配 ({} != {})",
                    run.gid,
                    pixels.len(),
                    frame_len
                ));
            }
            if run.fade.is_none() {
                for _ in 0..run.frames {
                    stdin
                        .write_all(pixels)
                        .map_err(|e| format!("写入帧失败: {e}"))?;
                    written += 1;
                }
            } else {
                let base = written;
                for i in 0..run.frames {
                    let t = frame_time(base + i, fps);
                    match fade_overlay_at(timeline, t, opts.fade_bg_rgb) {
                        Some((alpha, bg)) if alpha > 0.004 => {
                            apply_fade_overlay(pixels, &mut scratch, alpha, bg);
                            stdin
                                .write_all(&scratch)
                                .map_err(|e| format!("写入帧失败: {e}"))?;
                        }
                        _ => {
                            stdin
                                .write_all(pixels)
                                .map_err(|e| format!("写入帧失败: {e}"))?;
                        }
                    }
                    written += 1;
                }
            }
            if last_progress.elapsed() >= Duration::from_millis(200) {
                last_progress = Instant::now();
                let _ = tx.send_blocking(ExportMsg::Progress(format!(
                    "编码 {written}/{n_frames} 帧..."
                )));
            }
        }
        if written != n_frames {
            return Err(format!("写出 {written} 帧, 期望 {n_frames}"));
        }
        stdin
            .flush()
            .map_err(|e| format!("刷新帧数据失败: {e}"))?;
        Ok(())
    })();

    drop(child.stdin.take());
    let status = child.wait().map_err(|e| {
        crate::error::Error::Ffmpeg {
            step: "按帧编码".to_string(),
            detail: format!("等待进程结束失败: {e}"),
        }
        .to_string()
    })?;
    if let Some(h) = reader_handle {
        let _ = h.join();
    }
    if let Err(e) = write_result {
        let detail = tail.lock().map(|t| t.trim().to_string()).unwrap_or_default();
        if detail.is_empty() {
            return Err(e);
        }
        return Err(format!("{e}\n{detail}"));
    }
    if !status.success() {
        let detail = tail.lock().map(|t| t.trim().to_string()).unwrap_or_default();
        let detail = if detail.is_empty() {
            format!("退出码 {:?}", status.code())
        } else {
            format!("退出码 {:?}:\n{detail}", status.code())
        };
        return Err(crate::error::Error::Ffmpeg {
            step: "按帧编码".to_string(),
            detail,
        }
        .to_string());
    }
    Ok(())
}

fn build_audio(
    clips: &[AudioClip],
    target_duration: f64,
    out_audio: &Path,
    codec: &str,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    for c in clips {
        if !c.path.is_file() {
            return Err(crate::error::Error::AudioMissing(c.path.clone()).to_string());
        }
    }
    let target = format!("{target_duration:.9}");
    if clips.is_empty() {
        let args = vec![
            os("-y"),
            os("-f"),
            os("lavfi"),
            os("-i"),
            os("anullsrc=r=44100:cl=stereo"),
            os("-t"),
            os(target),
            os("-c:a"),
            os(codec),
            os_path(out_audio),
        ];
        return run_ffmpeg(&args, "生成静音音轨", tx);
    }
    // 每段用 atrim 按 `offset`/`duration` 裁切, 再 pad/trim 成时间轴上声明的
    // 整段时长, 最后 concat. 不要用输入侧 `-ss`/`-t`: 压缩格式按包对齐,
    // 多段接缝每段差几十毫秒, 后面的乐章会越偏越远; 单轨没有接缝所以从前
    // 看不出来. 输出再 apad + `-t` 对齐量化后的视频时长.
    let mut args = vec![os("-y")];
    for c in clips {
        args.push(os("-i"));
        args.push(os_path(&c.path));
    }
    args.push(os("-filter_complex"));
    args.push(os(audio_concat_filter(clips)));
    args.push(os("-map"));
    args.push(os("[aout]"));
    args.push(os("-t"));
    args.push(os(target));
    args.push(os("-c:a"));
    args.push(os(codec));
    args.push(os_path(out_audio));
    let step = if clips.len() == 1 {
        "转码音频"
    } else {
        "合并多段音频"
    };
    run_ffmpeg(&args, step, tx)
}

/// 每段先裁到 `[offset, offset+duration)`, 统一成立体声, 再强制输出恰好
/// `duration` 秒 (短则静音补齐, 长则截断), 这样 concat 接缝落在时间轴边界上.
fn audio_concat_filter(clips: &[AudioClip]) -> String {
    let mut parts = Vec::with_capacity(clips.len() + 1);
    for (i, c) in clips.iter().enumerate() {
        let start = c.offset.max(0.0);
        let dur = c.duration.max(0.0);
        parts.push(format!(
            "[{i}:a:0]atrim=start={start:.9}:duration={dur:.9},asetpts=PTS-STARTPTS,\
             aformat=sample_fmts=fltp:channel_layouts=stereo,\
             apad=whole_dur={dur:.9},atrim=end={dur:.9},asetpts=PTS-STARTPTS[a{i}]"
        ));
    }
    let mut labels = String::new();
    for i in 0..clips.len() {
        labels.push_str(&format!("[a{i}]"));
    }
    if clips.len() == 1 {
        parts.push(format!("{labels}apad[aout]"));
    } else {
        parts.push(format!(
            "{labels}concat=n={}:v=0:a=1,apad[aout]",
            clips.len()
        ));
    }
    parts.join(";")
}

fn mux_final(
    video: &Path,
    audio: &Path,
    out_path: &Path,
    duration: f64,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    // 音频在 `build_audio` 里已经按容器要求编码成 aac/flac 了, 这里只是把
    // 两路已经编码好的流封装进同一个容器, 直接 copy 即可, 不必再转一次码
    // (否则 MKV 无损 flac 会在这一步又被强行转成有损 aac, 白做了).
    // 时长锁在视频的整帧网格上, 不用 `-shortest` (AAC 帧对齐可能让音频略短,
    // 再截一刀会把片尾画面切掉).
    let args = vec![
        os("-y"),
        os("-i"),
        os_path(video),
        os("-i"),
        os_path(audio),
        os("-map"),
        os("0:v:0"),
        os("-map"),
        os("1:a:0"),
        os("-c:v"),
        os("copy"),
        os("-c:a"),
        os("copy"),
        os("-t"),
        os(format!("{duration:.9}")),
        os_path(out_path),
    ];
    run_ffmpeg(&args, "合并音视频", tx)
}

fn run_export(
    timeline: &Timeline,
    pool: &[MaterialItem],
    opts: &ExportOptions,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<PathBuf, String> {
    let _ = tx.send_blocking(ExportMsg::Progress("准备素材图片...".to_string()));
    let work = WorkDir::new()
        .map_err(|e| crate::error::Error::export(format!("创建临时目录失败: {e}")).to_string())?;
    let mut images = load_pool_rgba(pool, opts.width, opts.height)?;
    let w = even_dim(opts.width);
    let h = even_dim(opts.height);
    images.insert(
        BLACK_KEY.to_string(),
        RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255])).into_raw(),
    );

    let fps = opts.fps.max(1);
    let _ = tx.send_blocking(ExportMsg::Progress("按帧对齐时间轴...".to_string()));
    let runs = build_frame_runs(timeline, fps);
    let total_frames: u64 = runs.iter().map(|r| r.frames).sum();
    if runs.is_empty() || total_frames == 0 {
        return Err(crate::error::Error::export("时间轴为空, 无法导出").to_string());
    }

    let _ = tx.send_blocking(ExportMsg::Progress(format!(
        "编码 {} 帧 ({} 页切点)...",
        total_frames,
        runs.len()
    )));
    let silent_video = work.path().join("silent.mp4");
    encode_raw_video(
        &images,
        timeline,
        &runs,
        total_frames,
        opts,
        &silent_video,
        tx,
    )?;

    let video_dur = frames_to_seconds(total_frames, fps);
    let _ = tx.send_blocking(ExportMsg::Progress("合成音频...".to_string()));
    let audio_out = work
        .path()
        .join(format!("audio.{}", opts.container.audio_ext()));
    build_audio(
        &timeline.audio_clips,
        video_dur,
        &audio_out,
        opts.container.audio_codec(),
        tx,
    )?;

    let _ = tx.send_blocking(ExportMsg::Progress("合并音视频...".to_string()));
    if let Some(parent) = opts.out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    mux_final(&silent_video, &audio_out, &opts.out_path, video_dur, tx)?;

    Ok(opts.out_path.clone())
}

/// 后台线程跑导出, 通过 channel 回传进度/结果.
pub fn export_async(
    timeline: Timeline,
    pool: Vec<MaterialItem>,
    opts: ExportOptions,
) -> async_channel::Receiver<ExportMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let result = run_export(&timeline, &pool, &opts, &tx);
        let _ = tx.send_blocking(ExportMsg::Done(result));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::VideoClip;
    use uuid::Uuid;

    fn clip(gid: &str, start: f64, end: f64) -> VideoClip {
        VideoClip {
            id: Uuid::nil(),
            group_id: gid.into(),
            start,
            end,
        }
    }

    fn fade(kind: FadeKind, start: f64, end: f64) -> crate::model::FadeSpan {
        crate::model::FadeSpan {
            id: Uuid::new_v4(),
            start,
            end,
            kind,
            keep_bg: false,
        }
    }

    fn occupancy(runs: &[FrameRun]) -> Vec<(String, u64, bool)> {
        runs.iter()
            .map(|r| (r.gid.clone(), r.frames, r.fade.is_some()))
            .collect()
    }

    #[test]
    fn each_page_snaps_its_own_endpoints() {
        let mut tl = Timeline::new();
        tl.video_clips = vec![
            clip("a", 0.0, 1.016666),
            clip("b", 1.016666, 2.0),
            clip("c", 2.0, 3.7),
        ];
        let fps = 30;
        let runs = build_frame_runs(&tl, fps);
        let total: u64 = runs.iter().map(|r| r.frames).sum();
        assert_eq!(total, frame_at_or_after(3.7, fps));
        assert!(runs.iter().all(|r| r.frames > 0));

        let a_frames: u64 = runs
            .iter()
            .filter(|r| r.gid == "a")
            .map(|r| r.frames)
            .sum();
        let b_frames: u64 = runs
            .iter()
            .filter(|r| r.gid == "b")
            .map(|r| r.frames)
            .sum();
        assert_eq!(a_frames, frame_at_or_after(1.016666, fps));
        assert_eq!(
            b_frames,
            frame_at_or_after(2.0, fps) - frame_at_or_after(1.016666, fps)
        );
        assert_eq!(
            occupancy(&runs)
                .into_iter()
                .map(|(g, _, _)| g)
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn duration_rounding_is_not_used() {
        let mut tl = Timeline::new();
        let fps = 30u32;
        let n = 400usize;
        let mut t = 0.0;
        for i in 0..n {
            let dur = 3.0 + ((i % 7) as f64) * 0.017;
            tl.video_clips.push(clip(&format!("g{i}"), t, t + dur));
            t += dur;
        }
        let runs = build_frame_runs(&tl, fps);
        let total: u64 = runs.iter().map(|r| r.frames).sum();
        assert_eq!(total, frame_at_or_after(t, fps));
        let duration_sum: u64 = tl
            .video_clips
            .iter()
            .map(|c| ((c.end - c.start) * fps as f64).round() as u64)
            .sum();
        assert_ne!(duration_sum, total);
        for c in &tl.video_clips {
            let got: u64 = runs
                .iter()
                .filter(|r| r.gid == c.group_id)
                .map(|r| r.frames)
                .sum();
            assert_eq!(
                got,
                frame_at_or_after(c.end, fps).saturating_sub(frame_at_or_after(c.start, fps))
            );
        }
    }

    #[test]
    fn fade_overlay_matches_preview_formula() {
        let mut tl = Timeline::new();
        tl.video_clips = vec![clip("a", 0.0, 10.0)];
        tl.fades.push(fade(FadeKind::In, 0.0, 2.0));
        let (alpha0, bg0) = fade_overlay_at(&tl, 0.0, [1, 2, 3]).unwrap();
        assert!((alpha0 - 1.0).abs() < 1e-5);
        assert_eq!(bg0, [0, 0, 0]);
        let (alpha1, _) = fade_overlay_at(&tl, 1.0, [1, 2, 3]).unwrap();
        assert!((alpha1 - 0.5).abs() < 1e-5);
        assert!(fade_overlay_at(&tl, 2.0, [1, 2, 3]).is_none());
        tl.fades[0].keep_bg = true;
        let (_, bg) = fade_overlay_at(&tl, 0.5, [9, 8, 7]).unwrap();
        assert_eq!(bg, [9, 8, 7]);
    }

    #[test]
    fn fade_splits_a_page_without_shifting_neighbors() {
        let mut tl = Timeline::new();
        tl.video_clips = vec![clip("a", 0.0, 10.0)];
        tl.fades.push(fade(FadeKind::In, 0.0, 2.0));
        tl.fades.push(fade(FadeKind::Out, 8.0, 10.0));
        let fps = 30;
        let runs = build_frame_runs(&tl, fps);
        assert_eq!(runs.len(), 3);
        assert!(runs[0].fade.is_some());
        assert!(runs[1].fade.is_none());
        assert!(runs[2].fade.is_some());
        let total: u64 = runs.iter().map(|r| r.frames).sum();
        assert_eq!(total, frame_at_or_after(10.0, fps));
        assert_eq!(runs[0].frames, frame_at_or_after(2.0, fps));
        assert_eq!(
            runs[2].frames,
            frame_at_or_after(10.0, fps) - frame_at_or_after(8.0, fps)
        );
    }

    #[test]
    fn page_turn_is_not_before_preview_clock() {
        let cut = 1.016666;
        let fps = 30u32;
        let mut tl = Timeline::new();
        tl.video_clips = vec![clip("a", 0.0, cut), clip("b", cut, 2.0)];
        let runs = build_frame_runs(&tl, fps);
        let a_frames: u64 = runs
            .iter()
            .filter(|r| r.gid == "a")
            .map(|r| r.frames)
            .sum();
        let rounded = (cut * fps as f64).round() as u64;
        assert!(a_frames >= rounded);
        assert!(frame_time(a_frames, fps) + 1e-9 >= cut);
        assert!(frame_time(a_frames.saturating_sub(1), fps) < cut);
        assert_eq!(
            tl.covering_clip(frame_time(a_frames.saturating_sub(1), fps))
                .map(|c| c.group_id.as_str()),
            Some("a")
        );
        assert_eq!(
            tl.covering_clip(frame_time(a_frames, fps))
                .map(|c| c.group_id.as_str()),
            Some("b")
        );
    }

    fn audio_clip(path: PathBuf, duration: f64, offset: f64) -> AudioClip {
        AudioClip {
            id: Uuid::new_v4(),
            path,
            label: "a".into(),
            duration,
            offset,
        }
    }

    #[test]
    fn audio_concat_filter_forces_each_clip_duration() {
        let clips = vec![
            audio_clip(PathBuf::from("a.m4a"), 5.0, 0.0),
            audio_clip(PathBuf::from("b.m4a"), 4.25, 1.5),
        ];
        let f = audio_concat_filter(&clips);
        assert!(f.contains("[0:a:0]atrim=start=0.000000000:duration=5.000000000"));
        assert!(f.contains("[1:a:0]atrim=start=1.500000000:duration=4.250000000"));
        assert!(f.contains("apad=whole_dur=5.000000000"));
        assert!(f.contains("apad=whole_dur=4.250000000"));
        assert!(f.contains("concat=n=2:v=0:a=1,apad[aout]"));
        assert!(!f.contains("-ss"));
        let one = audio_concat_filter(&[audio_clip(PathBuf::from("a.wav"), 10.0, 0.0)]);
        assert!(one.ends_with("[a0]apad[aout]"));
        assert!(!one.contains("concat="));
    }

    fn ffmpeg_available() -> bool {
        std::process::Command::new(ffmpeg_path())
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn write_beep_wav(path: &Path, sr: u32, duration: f64, beep: f64) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let n = (duration * sr as f64).round() as u32;
        let beep_n = (beep * sr as f64).round() as u32;
        for i in 0..n {
            let s = if i < beep_n {
                (((i as f64) * 440.0 * 2.0 * std::f64::consts::PI / sr as f64).sin() * 20000.0)
                    as i16
            } else {
                0
            };
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    fn beep_onsets(path: &Path, sr: u32) -> Vec<f64> {
        let mut reader = hound::WavReader::open(path).unwrap();
        let ch = reader.spec().channels.max(1) as usize;
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        let mut onsets = Vec::new();
        let mut prev_loud = false;
        let min_gap = (sr as usize) / 5;
        let mut last_onset: Option<usize> = None;
        for (i, frame) in samples.chunks(ch).enumerate() {
            let loud = frame.iter().any(|s| s.abs() > 800);
            if loud && !prev_loud {
                let far_enough = last_onset.map(|j| i.saturating_sub(j) >= min_gap).unwrap_or(true);
                if far_enough {
                    onsets.push(i as f64 / sr as f64);
                    last_onset = Some(i);
                }
            }
            prev_loud = loud;
        }
        onsets
    }

    #[test]
    fn multi_clip_concat_keeps_timeline_boundaries() {
        if !ffmpeg_available() {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("sv_ac_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let sr = 8000u32;
        let clips_n = 4;
        let dur = 2.0;
        let mut clips = Vec::new();
        for i in 0..clips_n {
            let p = dir.join(format!("c{i}.wav"));
            write_beep_wav(&p, sr, dur, 0.08);
            clips.push(audio_clip(p, dur, 0.0));
        }
        let out = dir.join("out.wav");
        let (tx, _rx) = async_channel::unbounded();
        build_audio(&clips, dur * clips_n as f64, &out, "pcm_s16le", &tx).unwrap();
        let onsets = beep_onsets(&out, sr);
        assert_eq!(onsets.len(), clips_n, "onsets={onsets:?}");
        for (i, t) in onsets.iter().enumerate() {
            let exp = i as f64 * dur;
            assert!(
                (t - exp).abs() < 0.005,
                "clip {i} onset {t} expected {exp}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aac_multi_clip_concat_does_not_accumulate() {
        if !ffmpeg_available() {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("sv_aac_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let sr = 44100u32;
        let clips_n = 8;
        let dur = 1.0;
        let mut clips = Vec::new();
        for i in 0..clips_n {
            let wav = dir.join(format!("c{i}.wav"));
            let m4a = dir.join(format!("c{i}.m4a"));
            write_beep_wav(&wav, sr, dur, 0.08);
            let mut cmd = std::process::Command::new(ffmpeg_path());
            cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
                .arg(&wav)
                .args(["-c:a", "aac", "-b:a", "128k"])
                .arg(&m4a);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            let status = cmd.status().unwrap();
            assert!(status.success());
            clips.push(audio_clip(m4a, dur, 0.0));
        }
        let out = dir.join("out.wav");
        let (tx, _rx) = async_channel::unbounded();
        build_audio(&clips, dur * clips_n as f64, &out, "pcm_s16le", &tx).unwrap();
        let onsets = beep_onsets(&out, sr);
        assert_eq!(onsets.len(), clips_n, "onsets={onsets:?}");
        for (i, t) in onsets.iter().enumerate() {
            let exp = i as f64 * dur;
            assert!(
                (t - exp).abs() < 0.02,
                "clip {i} onset {t} expected {exp} (accumulated drift)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_clips_from_one_file_keep_offsets() {
        if !ffmpeg_available() {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("sv_as_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let sr = 8000u32;
        let src = dir.join("src.wav");
        // 8s, 蜂鸣出现在 0 / 2 / 4 / 6
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::create(&src, spec).unwrap();
            let n = 8 * sr;
            for i in 0..n {
                let t = i as f64 / sr as f64;
                let local = t % 2.0;
                let s = if local < 0.08 {
                    ((t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 20000.0) as i16
                } else {
                    0
                };
                w.write_sample(s).unwrap();
            }
            w.finalize().unwrap();
        }
        let clips: Vec<AudioClip> = (0..4)
            .map(|i| audio_clip(src.clone(), 2.0, i as f64 * 2.0))
            .collect();
        let out = dir.join("out.wav");
        let (tx, _rx) = async_channel::unbounded();
        build_audio(&clips, 8.0, &out, "pcm_s16le", &tx).unwrap();
        let onsets = beep_onsets(&out, sr);
        assert_eq!(onsets.len(), 4, "onsets={onsets:?}");
        for (i, t) in onsets.iter().enumerate() {
            let exp = i as f64 * 2.0;
            assert!(
                (t - exp).abs() < 0.005,
                "clip {i} onset {t} expected {exp}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn cdefgab_fixture() -> Option<PathBuf> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .parent()?
            .parent()?
            .join("sync_cdefgab_test");
        p.is_dir().then_some(p)
    }

    fn rgb_dist(a: [u8; 3], b: [u8; 3]) -> i32 {
        let dr = a[0] as i32 - b[0] as i32;
        let dg = a[1] as i32 - b[1] as i32;
        let db = a[2] as i32 - b[2] as i32;
        dr * dr + dg * dg + db * db
    }

    fn goertzel_power(samples: &[i16], sr: f64, freq: f64) -> f64 {
        let n = samples.len() as f64;
        if n < 16.0 {
            return 0.0;
        }
        let k = (n * freq / sr).round();
        let w = 2.0 * std::f64::consts::PI * k / n;
        let coeff = 2.0 * w.cos();
        let mut s1 = 0.0;
        let mut s2 = 0.0;
        for x in samples {
            let s0 = f64::from(*x) + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    fn run_ff(args: &[&str]) -> bool {
        let mut cmd = std::process::Command::new(ffmpeg_path());
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.status().map(|s| s.success()).unwrap_or(false)
    }

    #[test]
    fn cdefgab_fixture_export_stays_aligned() {
        if !ffmpeg_available() {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let Some(root) = cdefgab_fixture() else {
            eprintln!("skip: sync_cdefgab_test fixture missing");
            return;
        };
        let notes = ["C", "D", "E", "F", "G", "A", "B"];
        let hz = [261.626, 293.665, 329.628, 349.228, 392.0, 440.0, 493.883];
        let colors: [[u8; 3]; 7] = [
            [229, 57, 53],
            [251, 140, 0],
            [253, 216, 53],
            [67, 160, 71],
            [30, 136, 229],
            [142, 36, 170],
            [216, 27, 96],
        ];
        let dur = 4.017052;
        let mut pool = Vec::new();
        let mut tl = Timeline::new();
        for (i, n) in notes.iter().enumerate() {
            let png = root.join("pages").join(format!("{n}.png"));
            let m4a = root.join("audio").join(format!("{n}.m4a"));
            assert!(png.is_file(), "missing {}", png.display());
            assert!(m4a.is_file(), "missing {}", m4a.display());
            pool.push(MaterialItem {
                group_id: (*n).into(),
                label: (*n).into(),
                cache_path: png,
                width: 1280,
                height: 720,
            });
            let start = i as f64 * dur;
            tl.video_clips.push(clip(n, start, start + dur));
            tl.audio_clips.push(audio_clip(m4a, dur, 0.0));
        }
        let out = root.join("CDEFGAB.mp4");
        let opts = ExportOptions {
            container: Container::Mp4,
            width: 1280,
            height: 720,
            fps: 30,
            crf: 18,
            out_path: out.clone(),
            fade_bg_rgb: [255, 255, 255],
        };
        let rx = export_async(tl, pool, opts);
        loop {
            match rx.recv_blocking() {
                Ok(ExportMsg::Progress(s)) => eprintln!("  {s}"),
                Ok(ExportMsg::Done(Ok(p))) => {
                    eprintln!("exported {}", p.display());
                    break;
                }
                Ok(ExportMsg::Done(Err(e))) => panic!("export failed: {e}"),
                Err(_) => panic!("export channel closed"),
            }
        }
        assert!(out.is_file());

        let wav = root.join("CDEFGAB.wav");
        assert!(
            run_ff(&[
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                out.to_str().unwrap(),
                "-vn",
                "-ac",
                "1",
                "-ar",
                "44100",
                "-c:a",
                "pcm_s16le",
                wav.to_str().unwrap(),
            ]),
            "extract wav"
        );
        let mut reader = hound::WavReader::open(&wav).unwrap();
        let sr = reader.spec().sample_rate as f64;
        let pcm: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();

        for (i, n) in notes.iter().enumerate() {
            let mid = i as f64 * dur + dur * 0.5;
            let frame = root.join(format!("frame_{n}.png"));
            assert!(
                run_ff(&[
                    "-y",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-i",
                    out.to_str().unwrap(),
                    "-ss",
                    &format!("{mid:.6}"),
                    "-frames:v",
                    "1",
                    frame.to_str().unwrap(),
                ]),
                "extract frame {n}"
            );
            let img = image::open(&frame).unwrap().to_rgb8();
            let px = img.get_pixel(640, 36).0;
            let mut best = 0usize;
            let mut best_d = i32::MAX;
            for (j, c) in colors.iter().enumerate() {
                let d = rgb_dist(px, *c);
                if d < best_d {
                    best_d = d;
                    best = j;
                }
            }
            assert_eq!(
                notes[best], *n,
                "t={mid:.3}s frame color {:?} closest to {} want {n}",
                px, notes[best]
            );

            let a0 = ((mid - 0.4) * sr).max(0.0) as usize;
            let a1 = ((mid + 0.4) * sr) as usize;
            let slice = &pcm[a0.min(pcm.len())..a1.min(pcm.len())];
            let mut best_f = 0usize;
            let mut best_p = f64::NEG_INFINITY;
            for (j, f) in hz.iter().enumerate() {
                let p = goertzel_power(slice, sr, *f);
                if p > best_p {
                    best_p = p;
                    best_f = j;
                }
            }
            assert_eq!(
                notes[best_f], *n,
                "t={mid:.3}s audio peak {} want {n}",
                notes[best_f]
            );
        }

        for i in 1..notes.len() {
            let cut = i as f64 * dur;
            let after = cut + 0.2;
            let a0 = (after * sr) as usize;
            let a1 = ((after + 0.5) * sr) as usize;
            let slice = &pcm[a0.min(pcm.len())..a1.min(pcm.len())];
            let mut best_f = 0usize;
            let mut best_p = f64::NEG_INFINITY;
            for (j, f) in hz.iter().enumerate() {
                let p = goertzel_power(slice, sr, *f);
                if p > best_p {
                    best_p = p;
                    best_f = j;
                }
            }
            assert_eq!(
                notes[best_f],
                notes[i],
                "cut {:.3}s +0.2s audio is {} want {} (later clips drifted)",
                cut,
                notes[best_f],
                notes[i]
            );
        }
    }
}
