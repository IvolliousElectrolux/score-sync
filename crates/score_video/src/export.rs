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
    // 每段都按自己的 `offset`/`duration` 在输入端 (`-ss`/`-t` 放在各自
    // `-i` 前面, 即"输入定位") 单独截取: 整段导入的片段 offset=0、duration=
    // 原文件全长, 截出来等于没截; 被「分割音频」切开的片段则精确截出对应
    // 子区间, 不然分割后导出还是会把整份原始文件放进去, 白切了.
    // 输出用 apad + `-t` 对齐到量化后的视频时长, 避免片尾差半帧导致 mux
    // `-shortest` 再截一刀.
    if clips.len() == 1 {
        let c = &clips[0];
        let args = vec![
            os("-y"),
            os("-ss"),
            os(format!("{:.9}", c.offset)),
            os("-t"),
            os(format!("{:.9}", c.duration)),
            os("-i"),
            os_path(&c.path),
            os("-af"),
            os("apad"),
            os("-t"),
            os(target),
            os("-c:a"),
            os(codec),
            os_path(out_audio),
        ];
        return run_ffmpeg(&args, "转码音频", tx);
    }
    let mut args = vec![os("-y")];
    for c in clips {
        args.push(os("-ss"));
        args.push(os(format!("{:.9}", c.offset)));
        args.push(os("-t"));
        args.push(os(format!("{:.9}", c.duration)));
        args.push(os("-i"));
        args.push(os_path(&c.path));
    }
    let mut filter = String::new();
    for i in 0..clips.len() {
        filter.push_str(&format!("[{i}:a]"));
    }
    filter.push_str(&format!(
        "concat=n={}:v=0:a=1,apad[aout]",
        clips.len()
    ));
    args.push(os("-filter_complex"));
    args.push(os(filter));
    args.push(os("-map"));
    args.push(os("[aout]"));
    args.push(os("-t"));
    args.push(os(target));
    args.push(os("-c:a"));
    args.push(os(codec));
    args.push(os_path(out_audio));
    run_ffmpeg(&args, "合并多段音频", tx)
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
}
