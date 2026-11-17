//! ffmpeg 导出流水线.
//!
//! 预览跟音频时钟 (秒). 输出时长 = 帧数 / fps, 跟音频 `-t` 同一把尺子.
//! 每一帧该显示哪一页, 仍按时间轴的 `covering_*` 算, 不在接缝上丢帧.
//!
//! 内存: 同时只解码当前页, 以及刷入/交叠要用的另一页. 不再把整池 RGBA 留到导出结束.
//!
//! 速度有两条路, 看 sidecar 有没有 `concat` 解复用器:
//! - 有: 静止段只编码一帧, 这一帧的时长等于「帧数 / fps」; 转场段按帧画.
//!   各段再 `-c copy` 拼起来. 本机硬件编码器可用时, 静止和转场都交给它.
//!   硬件不接受 `fps/帧数`, 所以静止帧按导出帧率编一帧, 再把 MP4 里这一帧的
//!   时长改成这一页的帧数. 硬件失败, 或时长盒对不上, 整条时间轴改用同编码的
//!   软件编码器, 静止段直接 `-framerate fps/帧数`. 两套参数不混在同一次拼接里.
//! - 没有: 仍按帧写入 stdin, 但先转成编码器要的 nv12 / yuv420p, 不再每帧做 RGBA→YUV.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use image::RgbaImage;

use crate::hw_encode::{select_video_encoder, video_codec_args, VideoEncoder};
use crate::model::{AudioClip, FadeKind, MaterialItem, Timeline};
use crate::yuv::{rgba_to_yuv420, Yuv420};

/// ffmpeg 可执行文件路径: 优先找程序自身同目录下的 `ffmpeg(.exe)` (发行包
/// 自带, 用户不用单独装 ffmpeg), 找不到再退回 PATH 里的 `ffmpeg` (方便开发
/// 环境直接用系统装的那份).
///
/// 这是裁剪版 sidecar, 不是完整 ffmpeg. 能/不能做什么见仓库 README
/// 「裁剪版 ffmpeg」, 准绳是 `scripts/build_ffmpeg.sh` 的 configure 列表.
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

/// 导出画面用的编码. AVC 始终可选. HEVC / AV1 只在本机硬件编码器探测通过后出现.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    Avc,
    Hevc,
    Av1,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::Avc => "AVC",
            Self::Hevc => "HEVC",
            Self::Av1 => "AV1",
        }
    }
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
    pub codec: VideoCodec,
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
    if n % 2 == 0 {
        n
    } else {
        n + 1
    }
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
        let fade = timeline.covering_fade(t).map(|f| (f.kind, f.keep_bg));
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

/// 导出期间的页缓存: 最多留两张 RGBA (当前页 + 转场的另一页).
/// 预览贴图是缩小后的 JPEG, 和这里的全分辨率不是同一份, 不互相占用.
struct PageSource<'a> {
    pool: &'a [MaterialItem],
    w: u32,
    h: u32,
    slots: Vec<(String, Arc<Vec<u8>>)>,
    black: Arc<Vec<u8>>,
}

impl<'a> PageSource<'a> {
    fn new(pool: &'a [MaterialItem], w: u32, h: u32) -> Self {
        let w = even_dim(w);
        let h = even_dim(h);
        let black = RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255])).into_raw();
        Self {
            pool,
            w,
            h,
            slots: Vec::new(),
            black: Arc::new(black),
        }
    }

    fn rgba(&mut self, gid: &str) -> Result<Arc<Vec<u8>>, String> {
        if gid == BLACK_KEY {
            return Ok(Arc::clone(&self.black));
        }
        if let Some(i) = self.slots.iter().position(|(id, _)| id == gid) {
            let slot = self.slots.remove(i);
            let arc = Arc::clone(&slot.1);
            self.slots.insert(0, slot);
            return Ok(arc);
        }
        let item = self
            .pool
            .iter()
            .find(|m| m.group_id == gid)
            .ok_or_else(|| format!("素材 {gid} 缺少图片, 无法导出"))?;
        let rgba = item.load_rgba()?;
        let (iw, ih) = rgba.dimensions();
        let raw = if iw == self.w && ih == self.h {
            rgba.into_raw()
        } else {
            fit_pad(&rgba, self.w, self.h).into_raw()
        };
        let arc = Arc::new(raw);
        self.slots.insert(0, (gid.to_string(), Arc::clone(&arc)));
        if self.slots.len() > 2 {
            self.slots.pop();
        }
        Ok(arc)
    }
}

/// 静止段: 输入 1 张图, 帧率写成 `fps/帧数`, 这一帧的时长就是帧数/fps.
/// 转场段: 每帧都不同, 帧率就是导出帧率.
fn segment_rate(fps: u32, frames: u64, dynamic: bool) -> String {
    if dynamic {
        fps.max(1).to_string()
    } else {
        format!("{}/{}", fps.max(1), frames.max(1))
    }
}

fn segment_pictures(frames: u64, dynamic: bool) -> u64 {
    if dynamic {
        frames.max(1)
    } else {
        1
    }
}

fn without_format_filter(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg == "-vf" {
            skip = true;
            continue;
        }
        out.push(arg);
    }
    out
}

fn concat_demuxer_available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let mut cmd = Command::new(ffmpeg_path());
        cmd.args(["-hide_banner", "-demuxers"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let Ok(output) = cmd.output() else {
            return false;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .any(|line| line.split_whitespace().nth(1) == Some("concat"))
    })
}

fn ffconcat_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

/// 把一帧 (或一段转场的每一帧) 画进 `frame`. `layout = None` 时 `frame` 是 RGBA.
struct FramePainter<'a> {
    source: PageSource<'a>,
    w: u32,
    h: u32,
    layout: Option<Yuv420>,
    scratch: Vec<u8>,
    frame: Vec<u8>,
    fade_bg: [u8; 3],
}

impl<'a> FramePainter<'a> {
    fn new(
        pool: &'a [MaterialItem],
        w: u32,
        h: u32,
        layout: Option<Yuv420>,
        fade_bg: [u8; 3],
    ) -> Self {
        let w = even_dim(w);
        let h = even_dim(h);
        let rgba_len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
        Self {
            source: PageSource::new(pool, w, h),
            w,
            h,
            layout,
            scratch: vec![0u8; rgba_len],
            frame: Vec::new(),
            fade_bg,
        }
    }

    fn fill_still(&mut self, gid: &str) -> Result<(), String> {
        let rgba = self.source.rgba(gid)?;
        match self.layout {
            None => {
                self.frame.clear();
                self.frame.extend_from_slice(&rgba);
            }
            Some(layout) => rgba_to_yuv420(&rgba, self.w, self.h, layout, &mut self.frame),
        }
        Ok(())
    }

    /// 转场这一帧. 没有实际叠色时退回静止页.
    fn fill_at(&mut self, timeline: &Timeline, run: &FrameRun, t: f64) -> Result<(), String> {
        let kind = run.fade.map(|(k, _)| k);
        let blended = match kind {
            Some(FadeKind::WipeLtr) => {
                if let Some((prev, next, p, _)) = timeline.page_blend_at(t) {
                    let prev_img = self.source.rgba(prev)?;
                    let next_img = self.source.rgba(next)?;
                    apply_wipe(&prev_img, &next_img, &mut self.scratch, p as f32, self.w);
                    true
                } else {
                    false
                }
            }
            Some(FadeKind::Dissolve) => {
                if let Some((prev, next, p, _)) = timeline.page_blend_at(t) {
                    let prev_img = self.source.rgba(prev)?;
                    let next_img = self.source.rgba(next)?;
                    apply_dissolve(&prev_img, &next_img, &mut self.scratch, p as f32);
                    true
                } else {
                    false
                }
            }
            _ => match fade_overlay_at(timeline, t, self.fade_bg) {
                Some((alpha, bg)) if alpha > 0.004 => {
                    let pixels = self.source.rgba(&run.gid)?;
                    apply_fade_overlay(&pixels, &mut self.scratch, alpha, bg);
                    true
                }
                _ => false,
            },
        };
        if blended {
            match self.layout {
                None => {
                    self.frame.clear();
                    self.frame.extend_from_slice(&self.scratch);
                }
                Some(layout) => {
                    rgba_to_yuv420(&self.scratch, self.w, self.h, layout, &mut self.frame)
                }
            }
        } else {
            self.fill_still(&run.gid)?;
        }
        Ok(())
    }
}

fn fade_overlay_at(timeline: &Timeline, t: f64, fade_bg: [u8; 3]) -> Option<(f32, [u8; 3])> {
    let fade = timeline.covering_fade(t)?;
    if fade.kind.is_page_cut() {
        return None;
    }
    let span = (fade.end - fade.start).max(1e-6);
    let p = ((t - fade.start) / span).clamp(0.0, 1.0);
    let alpha = match fade.kind {
        FadeKind::In => 1.0 - p,
        FadeKind::Out => p,
        FadeKind::WipeLtr | FadeKind::Dissolve => return None,
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

/// 左→右硬切擦除: `progress` 为 0 时整帧上一页, 1 时整帧下一页.
/// 两页线性叠化: `progress` 为 0 时整帧上一页, 1 时整帧下一页.
fn apply_dissolve(prev: &[u8], next: &[u8], dst: &mut [u8], progress: f32) {
    let p = progress.clamp(0.0, 1.0);
    let ip = 1.0 - p;
    if dst.len() != prev.len() || dst.len() != next.len() || dst.len() % 4 != 0 {
        if dst.len() == next.len() {
            dst.copy_from_slice(next);
        }
        return;
    }
    for (out, (a, b)) in dst
        .chunks_exact_mut(4)
        .zip(prev.chunks_exact(4).zip(next.chunks_exact(4)))
    {
        out[0] = (a[0] as f32 * ip + b[0] as f32 * p + 0.5) as u8;
        out[1] = (a[1] as f32 * ip + b[1] as f32 * p + 0.5) as u8;
        out[2] = (a[2] as f32 * ip + b[2] as f32 * p + 0.5) as u8;
        out[3] = 255;
    }
}

fn apply_wipe(prev: &[u8], next: &[u8], dst: &mut [u8], progress: f32, width: u32) {
    let width = width.max(1) as usize;
    let split = ((progress.clamp(0.0, 1.0) * width as f32) as usize).min(width);
    let row_bytes = width * 4;
    if dst.len() != prev.len() || dst.len() != next.len() || row_bytes == 0 {
        dst.copy_from_slice(next);
        return;
    }
    for (out_row, (prev_row, next_row)) in dst.chunks_exact_mut(row_bytes).zip(
        prev.chunks_exact(row_bytes)
            .zip(next.chunks_exact(row_bytes)),
    ) {
        let left = split * 4;
        out_row[..left].copy_from_slice(&next_row[..left]);
        out_row[left..].copy_from_slice(&prev_row[left..]);
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
                    push_log_tail(&mut t, &text);
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
        let detail = tail
            .lock()
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
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

fn push_log_tail(tail: &mut String, text: &str) {
    tail.push_str(text);
    tail.push('\n');
    const MAX: usize = 6000;
    if tail.len() <= MAX {
        return;
    }
    let mut cut = tail.len() - MAX;
    while cut < tail.len() && !tail.is_char_boundary(cut) {
        cut += 1;
    }
    tail.drain(0..cut);
}

fn os(s: impl Into<String>) -> std::ffi::OsString {
    std::ffi::OsString::from(s.into())
}
fn os_path(p: &Path) -> std::ffi::OsString {
    p.as_os_str().to_owned()
}

struct EncodeFailure {
    message: String,
    /// 编码器拒绝了这条命令. 换 libx264 再试一次.
    retryable: bool,
}

impl From<String> for EncodeFailure {
    fn from(message: String) -> Self {
        let retryable = message.contains("退出码")
            || message.contains("写入帧失败")
            || message.contains("刷新帧数据失败")
            || message.contains("ffmpeg 执行失败");
        Self { message, retryable }
    }
}

fn encode_raw_video(
    pool: &[MaterialItem],
    timeline: &Timeline,
    runs: &[FrameRun],
    n_frames: u64,
    opts: &ExportOptions,
    encoder: VideoEncoder,
    out_path: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), EncodeFailure> {
    let fps = opts.fps.max(1);
    let w = even_dim(opts.width);
    let h = even_dim(opts.height);
    let layout = encoder.stored_yuv();
    let label = encoder.label(opts.codec);
    let mut args = vec![
        os("-y"),
        os("-f"),
        os("rawvideo"),
        os("-pix_fmt"),
        os(layout.ffmpeg_name()),
        os("-s"),
        os(format!("{w}x{h}")),
        os("-framerate"),
        os(fps.to_string()),
        os("-i"),
        os("-"),
        os("-frames:v"),
        os(n_frames.to_string()),
    ];
    for arg in without_format_filter(video_codec_args(opts.codec, encoder, opts.crf)) {
        args.push(os(arg));
    }
    args.extend([
        os("-video_track_timescale"),
        os(fps.to_string()),
        os("-an"),
        os_path(out_path),
    ]);

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
                    push_log_tail(&mut t, &text);
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
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin 不可写".to_string())?;
        let mut painter = FramePainter::new(pool, w, h, Some(layout), opts.fade_bg_rgb);
        let mut written = 0u64;
        let started = Instant::now();
        let mut last_progress = Instant::now() - Duration::from_secs(1);
        let mut report = |written: u64| {
            if last_progress.elapsed() < Duration::from_millis(200) {
                return;
            }
            last_progress = Instant::now();
            let head = format!("编码 {written}/{n_frames} 帧 ({label})");
            let _ = tx.send_blocking(ExportMsg::Progress(with_remaining(
                &head,
                started.elapsed().as_secs_f64(),
                written,
                n_frames,
            )));
        };
        for run in runs {
            if run.fade.is_none() {
                painter.fill_still(&run.gid)?;
                for _ in 0..run.frames {
                    stdin
                        .write_all(&painter.frame)
                        .map_err(|e| format!("写入帧失败: {e}"))?;
                    written += 1;
                    report(written);
                }
            } else {
                let base = written;
                for i in 0..run.frames {
                    painter.fill_at(timeline, run, frame_time(base + i, fps))?;
                    stdin
                        .write_all(&painter.frame)
                        .map_err(|e| format!("写入帧失败: {e}"))?;
                    written += 1;
                    report(written);
                }
            }
        }
        if written != n_frames {
            return Err(format!("写出 {written} 帧, 期望 {n_frames}"));
        }
        stdin.flush().map_err(|e| format!("刷新帧数据失败: {e}"))?;
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
        let detail = tail
            .lock()
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        if detail.is_empty() {
            return Err(e.into());
        }
        return Err(format!("{e}\n{detail}").into());
    }
    if !status.success() {
        let detail = tail
            .lock()
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        let detail = if detail.is_empty() {
            format!("退出码 {:?}", status.code())
        } else {
            format!("退出码 {:?}:\n{detail}", status.code())
        };
        return Err(crate::error::Error::Ffmpeg {
            step: "按帧编码".to_string(),
            detail,
        }
        .to_string()
        .into());
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

/// 电影时间刻度上的时长, 向上取整, 盖住媒体时间, 避免编辑列表把这一帧裁短.
/// 17 帧, 媒体 60 Hz, 电影 1000 Hz → 284. 600 帧 → 10000.
fn movie_duration_ticks(frames: u64, movie_ts: u64, media_ts: u64) -> Option<u32> {
    if movie_ts == 0 || media_ts == 0 {
        return None;
    }
    let ticks = frames.checked_mul(movie_ts)?;
    u32::try_from(ticks.div_ceil(media_ts)).ok()
}

fn read_u32(data: &[u8], off: usize) -> Result<u32, String> {
    let bytes = data.get(off..off + 4).ok_or("MP4 时长盒越界")?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn write_u32(data: &mut [u8], off: usize, value: u32) -> Result<(), String> {
    let bytes = data.get_mut(off..off + 4).ok_or("MP4 时长盒越界")?;
    bytes.copy_from_slice(&value.to_be_bytes());
    Ok(())
}

/// 只递归 moov 这一支. mdat 里的字节不是盒.
fn walk_timing(
    data: &[u8],
    start: usize,
    end: usize,
    slots: &mut [Option<usize>; 5],
) -> Result<(), String> {
    const NAMES: [&[u8]; 5] = [b"mvhd", b"mdhd", b"tkhd", b"elst", b"stts"];
    const CONTAINERS: [&[u8]; 6] = [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"edts"];
    let mut cursor = start;
    while cursor + 8 <= end {
        let size = read_u32(data, cursor)? as usize;
        if size < 8 || cursor + size > end {
            return Err("MP4 盒尺寸不对".to_string());
        }
        let kind = &data[cursor + 4..cursor + 8];
        let payload = cursor + 8;
        let next = cursor + size;
        if let Some(index) = NAMES.iter().position(|name| *name == kind) {
            if slots[index].is_some() {
                return Err("MP4 时长盒重复".to_string());
            }
            slots[index] = Some(payload);
        } else if CONTAINERS.iter().any(|name| *name == kind) {
            walk_timing(data, payload, next, slots)?;
        }
        cursor = next;
    }
    Ok(())
}

/// 硬件编码器拒收 `fps/帧数`, 静止段按导出帧率只编一帧, 包时长仍是 1/fps.
/// 这里把这一帧改成「帧数 / fps」, 和软件路径那一帧的时长对齐:
/// stts / mdhd 用媒体时间刻度, mvhd / tkhd / elst 用电影时间刻度并向上取整.
fn stretch_still_bytes(data: &mut [u8], frames: u64, fps: u32) -> Result<(), String> {
    if frames <= 1 {
        return Ok(());
    }
    let frames_u32 = u32::try_from(frames).map_err(|_| "这一页太长, 无法改时长".to_string())?;
    let mut slots = [None; 5];
    walk_timing(data, 0, data.len(), &mut slots)?;
    let [mvhd, mdhd, tkhd, elst, stts] = slots;
    let (Some(mvhd), Some(mdhd), Some(tkhd), Some(elst), Some(stts)) =
        (mvhd, mdhd, tkhd, elst, stts)
    else {
        return Err("MP4 缺少时长盒".to_string());
    };
    for (at, need) in [(mvhd, 20), (mdhd, 20), (tkhd, 24), (elst, 16), (stts, 16)] {
        if data.len() < at + need || data[at] != 0 {
            return Err("MP4 时长盒版本不受支持".to_string());
        }
    }
    if read_u32(data, mdhd + 12)? != fps {
        return Err("静止段的时间刻度不是导出帧率".to_string());
    }
    if read_u32(data, stts + 4)? != 1
        || read_u32(data, stts + 8)? != 1
        || read_u32(data, stts + 12)? != 1
    {
        return Err("静止段不是单帧, 不改时长".to_string());
    }
    if read_u32(data, elst + 4)? != 1 || read_u32(data, elst + 12)? != 0 {
        return Err("编辑列表不是单段, 不改时长".to_string());
    }
    let movie_ts = u64::from(read_u32(data, mvhd + 12)?);
    let movie_dur = movie_duration_ticks(frames, movie_ts, u64::from(fps))
        .ok_or("这一页的时长超出 MP4 能写下的范围")?;
    write_u32(data, stts + 12, frames_u32)?;
    write_u32(data, mdhd + 16, frames_u32)?;
    write_u32(data, mvhd + 16, movie_dur)?;
    write_u32(data, tkhd + 20, movie_dur)?;
    write_u32(data, elst + 8, movie_dur)?;
    Ok(())
}

fn stretch_still_mp4(path: &Path, frames: u64, fps: u32) -> Result<(), String> {
    if frames <= 1 {
        return Ok(());
    }
    let mut data = std::fs::read(path).map_err(|e| format!("读取静止段失败: {e}"))?;
    stretch_still_bytes(&mut data, frames, fps)?;
    std::fs::write(path, data).map_err(|e| format!("写回静止段时长失败: {e}"))?;
    Ok(())
}

fn held_hw_segment_args(
    codec: VideoCodec,
    encoder: VideoEncoder,
    fps: u32,
    w: u32,
    h: u32,
    frames: u64,
    dynamic: bool,
    crf: u32,
    out: &Path,
) -> Vec<std::ffi::OsString> {
    let pictures = if dynamic { frames.max(1) } else { 1 };
    let layout = encoder.stored_yuv();
    let mut args = vec![
        os("-y"),
        os("-hide_banner"),
        os("-loglevel"),
        os("error"),
        os("-progress"),
        os("pipe:2"),
        os("-framerate"),
        os(fps.max(1).to_string()),
        os("-f"),
        os("rawvideo"),
        os("-pix_fmt"),
        os(layout.ffmpeg_name()),
        os("-s"),
        os(format!("{w}x{h}")),
        os("-i"),
        os("-"),
        os("-frames:v"),
        os(pictures.to_string()),
        os("-fps_mode"),
        os("passthrough"),
    ];
    for arg in without_format_filter(video_codec_args(codec, encoder, crf)) {
        args.push(os(arg));
    }
    // Media Foundation 不认 -bf. 其余硬件关掉 B 帧, 短转场和后面的拼接才稳.
    if encoder != VideoEncoder::MediaFoundation {
        args.extend([os("-bf"), os("0")]);
    }
    args.extend([
        os("-video_track_timescale"),
        os(fps.max(1).to_string()),
        os("-an"),
        os_path(out),
    ]);
    args
}

fn held_segment_args(
    codec: VideoCodec,
    fps: u32,
    w: u32,
    h: u32,
    frames: u64,
    dynamic: bool,
    crf: u32,
    out: &Path,
) -> Vec<std::ffi::OsString> {
    let pictures = segment_pictures(frames, dynamic);
    let crf = crf.clamp(1, 51).to_string();
    let mut args = vec![
        os("-y"),
        os("-hide_banner"),
        os("-loglevel"),
        os("error"),
        os("-progress"),
        os("pipe:2"),
        os("-framerate"),
        os(segment_rate(fps, frames, dynamic)),
        os("-f"),
        os("rawvideo"),
        os("-pix_fmt"),
        os("yuv420p"),
        os("-s"),
        os(format!("{w}x{h}")),
        os("-i"),
        os("-"),
        os("-frames:v"),
        os(pictures.to_string()),
        os("-fps_mode"),
        os("passthrough"),
    ];
    // 硬件拒收「fps/帧数」时整条时间轴回到这里, 仍用同编码的软件编码器.
    match codec {
        VideoCodec::Avc => args.extend([
            os("-c:v"),
            os("libx264"),
            os("-preset"),
            os("medium"),
            os("-tune"),
            os("stillimage"),
            os("-bf"),
            os("0"),
            os("-profile:v"),
            os("high"),
            os("-crf"),
            os(crf),
        ]),
        VideoCodec::Hevc => args.extend([
            os("-c:v"),
            os("libx265"),
            os("-preset"),
            os("medium"),
            os("-tune"),
            os("stillimage"),
            os("-tag:v"),
            os("hvc1"),
            os("-x265-params"),
            os("bframes=0"),
            os("-crf"),
            os(crf),
        ]),
        VideoCodec::Av1 => args.extend([
            os("-c:v"),
            os("libsvtav1"),
            os("-preset"),
            os("6"),
            os("-crf"),
            os(crf),
            os("-svtav1-params"),
            os("keyint=256:scd=0"),
        ]),
    }
    args.extend([
        os("-pix_fmt"),
        os("yuv420p"),
        os("-video_track_timescale"),
        os(fps.max(1).to_string()),
        os("-an"),
        os_path(out),
    ]);
    args
}

/// 已完成 `done` 张画面, 用掉 `elapsed_secs` 秒时, 剩下的还要多久.
fn eta_seconds(elapsed_secs: f64, done: u64, total: u64) -> Option<f64> {
    if done == 0 || total <= done || !(elapsed_secs > 0.0) {
        return None;
    }
    Some(elapsed_secs * (total - done) as f64 / done as f64)
}

fn format_eta(seconds: f64) -> String {
    let whole = seconds.round().max(1.0) as u64;
    let hours = whole / 3600;
    let mins = (whole % 3600) / 60;
    let secs = whole % 60;
    if hours > 0 {
        if mins == 0 {
            format!("{hours} 小时")
        } else {
            format!("{hours} 小时 {mins} 分")
        }
    } else if mins > 0 {
        if secs == 0 {
            format!("{mins} 分")
        } else {
            format!("{mins} 分 {secs} 秒")
        }
    } else {
        format!("{secs} 秒")
    }
}

fn with_remaining(head: &str, elapsed_secs: f64, done: u64, total: u64) -> String {
    match eta_seconds(elapsed_secs, done, total) {
        Some(seconds) if seconds < 1.0 => format!("{head}, 剩余不到 1 秒..."),
        Some(seconds) => format!("{head}, 剩余 {}...", format_eta(seconds)),
        None => format!("{head}..."),
    }
}

fn progress_frame(text: &str) -> Option<u64> {
    let rest = text.trim().strip_prefix("frame=")?.trim();
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn is_progress_noise(text: &str) -> bool {
    let text = text.trim();
    progress_frame(text).is_some()
        || text.starts_with("fps=")
        || text.starts_with("stream_")
        || text.starts_with("bitrate=")
        || text.starts_with("total_size=")
        || text.starts_with("out_time")
        || text.starts_with("dup_frames=")
        || text.starts_with("drop_frames=")
        || text.starts_with("speed=")
        || text == "progress=continue"
        || text == "progress=end"
}

struct PictureClock {
    start: Instant,
    total: u64,
    done: u64,
    last_sent: Instant,
}

impl PictureClock {
    fn new(total: u64) -> Self {
        let now = Instant::now();
        Self {
            start: now,
            total,
            done: 0,
            last_sent: now - Duration::from_secs(1),
        }
    }
}

fn report_encoded(
    tx: &async_channel::Sender<ExportMsg>,
    clock: &Mutex<PictureClock>,
    base: u64,
    pictures: u64,
    frame: u64,
    segment_no: usize,
    segments: usize,
    label: Option<&str>,
    force: bool,
) {
    let Ok(mut clock) = clock.lock() else {
        return;
    };
    let done = base.saturating_add(frame.min(pictures));
    if done > clock.done {
        clock.done = done.min(clock.total);
    } else if !force {
        return;
    }
    if !force && clock.last_sent.elapsed() < Duration::from_millis(200) {
        return;
    }
    clock.last_sent = Instant::now();
    let head = match label {
        Some(label) => format!("编码画面 {segment_no}/{segments} ({label})"),
        None => format!("编码画面 {segment_no}/{segments}"),
    };
    let line = with_remaining(
        &head,
        clock.start.elapsed().as_secs_f64(),
        clock.done,
        clock.total,
    );
    let _ = tx.send_blocking(ExportMsg::Progress(line));
}

fn ffmpeg_with_stdin<W, F>(
    args: &[std::ffi::OsString],
    step: &str,
    on_frame: F,
    write: W,
) -> Result<(), String>
where
    W: FnOnce(&mut ChildStdin) -> Result<(), String>,
    F: Fn(u64) + Send + 'static,
{
    let mut cmd = Command::new(ffmpeg_path());
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| {
        crate::error::Error::FfmpegSpawn {
            step: step.to_string(),
            source: e,
        }
        .to_string()
    })?;
    let stderr = child.stderr.take();
    let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let reader_tail = Arc::clone(&tail);
    let reader = stderr.map(move |stderr| {
        let tail = reader_tail;
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stderr);
            let mut buf = Vec::new();
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
                if let Some(frame) = progress_frame(&text) {
                    on_frame(frame);
                    continue;
                }
                if is_progress_noise(&text) {
                    continue;
                }
                if let Ok(mut guard) = tail.lock() {
                    push_log_tail(&mut guard, &text);
                }
            }
        })
    });
    let write_result = (|| {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg stdin 不可写".to_string())?;
        write(&mut stdin)?;
        stdin.flush().map_err(|e| format!("刷新帧数据失败: {e}"))?;
        Ok(())
    })();
    drop(child.stdin.take());
    let status = child.wait().map_err(|e| format!("等待进程结束失败: {e}"))?;
    if let Some(handle) = reader {
        let _ = handle.join();
    }
    let detail = tail
        .lock()
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    if let Err(err) = write_result {
        if detail.is_empty() {
            return Err(err);
        }
        return Err(format!("{err}\n{detail}"));
    }
    if !status.success() {
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

fn concat_segments(
    list: &Path,
    out_path: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let args = vec![
        os("-y"),
        os("-f"),
        os("concat"),
        os("-safe"),
        os("0"),
        os("-i"),
        os_path(list),
        os("-c"),
        os("copy"),
        os_path(out_path),
    ];
    run_ffmpeg(&args, "拼接画面", tx)
}

fn write_run_frames(
    painter: &mut FramePainter<'_>,
    timeline: &Timeline,
    run: &FrameRun,
    base: u64,
    fps: u32,
    dynamic: bool,
    stdin: &mut ChildStdin,
) -> Result<(), String> {
    if dynamic {
        for index in 0..run.frames {
            painter.fill_at(timeline, run, frame_time(base + index, fps))?;
            stdin
                .write_all(&painter.frame)
                .map_err(|e| format!("写入帧失败: {e}"))?;
        }
    } else {
        painter.fill_still(&run.gid)?;
        stdin
            .write_all(&painter.frame)
            .map_err(|e| format!("写入帧失败: {e}"))?;
    }
    Ok(())
}

/// 静止段一帧, 转场段逐帧. 硬件整段失败时改回同编码的软件编码器.
/// 缺图不改路. 两条路都失败时, 调用方再改走按帧编码.
fn encode_held(
    pool: &[MaterialItem],
    timeline: &Timeline,
    runs: &[FrameRun],
    opts: &ExportOptions,
    out_path: &Path,
    work: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let encoder = select_video_encoder(opts.codec);
    if encoder.is_hardware() {
        match encode_held_hw(pool, timeline, runs, opts, encoder, out_path, work, tx) {
            Ok(()) => return Ok(()),
            Err(err) if err.contains("缺少图片") => return Err(err),
            Err(err) => {
                let brief = err
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(180)
                    .collect::<String>();
                let _ = tx.send_blocking(ExportMsg::Progress(format!(
                    "硬件编码失败, 改用 {}: {brief}",
                    VideoEncoder::Software.ffmpeg_name(opts.codec)
                )));
            }
        }
    }
    encode_held_software(pool, timeline, runs, opts, out_path, work, tx)
}

fn encode_held_hw(
    pool: &[MaterialItem],
    timeline: &Timeline,
    runs: &[FrameRun],
    opts: &ExportOptions,
    encoder: VideoEncoder,
    out_path: &Path,
    work: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let fps = opts.fps.max(1);
    let w = even_dim(opts.width);
    let h = even_dim(opts.height);
    let label = encoder.label(opts.codec);
    let total_pictures: u64 = runs
        .iter()
        .map(|run| segment_pictures(run.frames, run.fade.is_some()))
        .sum();
    let clock = Arc::new(Mutex::new(PictureClock::new(total_pictures)));
    let mut painter = FramePainter::new(pool, w, h, Some(encoder.stored_yuv()), opts.fade_bg_rgb);
    let mut script = String::from("ffconcat version 1.0\n");
    let mut cursor = 0u64;
    let mut encoded = 0u64;
    for (i, run) in runs.iter().enumerate() {
        let seg = work.join(format!("s{i:05}.mp4"));
        let dynamic = run.fade.is_some();
        let args = held_hw_segment_args(
            opts.codec, encoder, fps, w, h, run.frames, dynamic, opts.crf, &seg,
        );
        let base = cursor;
        let pictures = segment_pictures(run.frames, dynamic);
        let encoded_base = encoded;
        let segment_no = i + 1;
        let segments = runs.len();
        let watch = {
            let tx = tx.clone();
            let clock = Arc::clone(&clock);
            move |frame: u64| {
                report_encoded(
                    &tx,
                    &clock,
                    encoded_base,
                    pictures,
                    frame,
                    segment_no,
                    segments,
                    Some(label),
                    false,
                );
            }
        };
        ffmpeg_with_stdin(&args, "按页编码", watch, |stdin| {
            write_run_frames(&mut painter, timeline, run, base, fps, dynamic, stdin)
        })?;
        if !dynamic {
            stretch_still_mp4(&seg, run.frames, fps)?;
        }
        report_encoded(
            tx,
            &clock,
            encoded_base,
            pictures,
            pictures,
            segment_no,
            segments,
            Some(label),
            true,
        );
        script.push_str(&format!("file '{}'\n", ffconcat_path(&seg)));
        cursor += run.frames;
        encoded += pictures;
    }
    let list = work.join("segments.txt");
    std::fs::write(&list, script).map_err(|e| format!("写出拼接列表失败: {e}"))?;
    concat_segments(&list, out_path, tx)
}

fn encode_held_software(
    pool: &[MaterialItem],
    timeline: &Timeline,
    runs: &[FrameRun],
    opts: &ExportOptions,
    out_path: &Path,
    work: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let fps = opts.fps.max(1);
    let w = even_dim(opts.width);
    let h = even_dim(opts.height);
    let total_pictures: u64 = runs
        .iter()
        .map(|run| segment_pictures(run.frames, run.fade.is_some()))
        .sum();
    let clock = Arc::new(Mutex::new(PictureClock::new(total_pictures)));
    let mut painter = FramePainter::new(pool, w, h, Some(Yuv420::I420), opts.fade_bg_rgb);
    let mut script = String::from("ffconcat version 1.0\n");
    let mut cursor = 0u64;
    let mut encoded = 0u64;
    for (i, run) in runs.iter().enumerate() {
        let seg = work.join(format!("s{i:05}.mp4"));
        let dynamic = run.fade.is_some();
        let args = held_segment_args(opts.codec, fps, w, h, run.frames, dynamic, opts.crf, &seg);
        let base = cursor;
        let pictures = segment_pictures(run.frames, dynamic);
        let encoded_base = encoded;
        let segment_no = i + 1;
        let segments = runs.len();
        let watch = {
            let tx = tx.clone();
            let clock = Arc::clone(&clock);
            move |frame: u64| {
                report_encoded(
                    &tx,
                    &clock,
                    encoded_base,
                    pictures,
                    frame,
                    segment_no,
                    segments,
                    None,
                    false,
                );
            }
        };
        ffmpeg_with_stdin(&args, "按页编码", watch, |stdin| {
            write_run_frames(&mut painter, timeline, run, base, fps, dynamic, stdin)
        })?;
        report_encoded(
            tx,
            &clock,
            encoded_base,
            pictures,
            pictures,
            segment_no,
            segments,
            None,
            true,
        );
        script.push_str(&format!("file '{}'\n", ffconcat_path(&seg)));
        cursor += run.frames;
        encoded += pictures;
    }
    let list = work.join("segments.txt");
    std::fs::write(&list, script).map_err(|e| format!("写出拼接列表失败: {e}"))?;
    concat_segments(&list, out_path, tx)
}

fn encode_cfr(
    pool: &[MaterialItem],
    timeline: &Timeline,
    runs: &[FrameRun],
    total_frames: u64,
    opts: &ExportOptions,
    out_path: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let _ = tx.send_blocking(ExportMsg::Progress("检测本机视频编码器...".to_string()));
    let encoder = select_video_encoder(opts.codec);
    let _ = tx.send_blocking(ExportMsg::Progress(format!(
        "编码 {} 帧 ({}, {}), {} 页切点...",
        total_frames,
        opts.codec.label(),
        encoder.label(opts.codec),
        runs.len()
    )));
    match encode_raw_video(
        pool,
        timeline,
        runs,
        total_frames,
        opts,
        encoder,
        out_path,
        tx,
    ) {
        Ok(()) => Ok(()),
        Err(err) if err.retryable && encoder.is_hardware() => {
            let software = VideoEncoder::Software;
            let _ = tx.send_blocking(ExportMsg::Progress(format!(
                "硬件编码失败, 改用 {}...",
                software.ffmpeg_name(opts.codec)
            )));
            encode_raw_video(
                pool,
                timeline,
                runs,
                total_frames,
                opts,
                software,
                out_path,
                tx,
            )
            .map_err(|err| err.message)
        }
        Err(err) => Err(err.message),
    }
}

fn run_export(
    timeline: &Timeline,
    pool: &[MaterialItem],
    opts: &ExportOptions,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<PathBuf, String> {
    let work = WorkDir::new()
        .map_err(|e| crate::error::Error::export(format!("创建临时目录失败: {e}")).to_string())?;
    let fps = opts.fps.max(1);
    let _ = tx.send_blocking(ExportMsg::Progress("按帧对齐时间轴...".to_string()));
    let runs = build_frame_runs(timeline, fps);
    let total_frames: u64 = runs.iter().map(|r| r.frames).sum();
    if runs.is_empty() || total_frames == 0 {
        return Err(crate::error::Error::export("时间轴为空, 无法导出").to_string());
    }

    let silent_video = work.path().join("silent.mp4");
    if concat_demuxer_available() {
        let stills = runs.iter().filter(|r| r.fade.is_none()).count();
        let _ = tx.send_blocking(ExportMsg::Progress(format!(
            "按页编码 {} 段 (其中 {} 段静止页只编一帧), 共 {} 帧...",
            runs.len(),
            stills,
            total_frames
        )));
        if let Err(err) = encode_held(pool, timeline, &runs, opts, &silent_video, work.path(), tx) {
            let missing = err.contains("缺少图片");
            if missing {
                return Err(err);
            }
            let _ = tx.send_blocking(ExportMsg::Progress(format!(
                "按页编码失败, 改按帧编码: {err}"
            )));
            encode_cfr(pool, timeline, &runs, total_frames, opts, &silent_video, tx)?;
        }
    } else {
        encode_cfr(pool, timeline, &runs, total_frames, opts, &silent_video, tx)?;
    }

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
    fn held_static_rate_is_frames_over_fps() {
        assert_eq!(segment_rate(60, 17, false), "60/17");
        assert_eq!(segment_rate(60, 17, true), "60");
        assert_eq!(segment_pictures(17, false), 1);
        assert_eq!(segment_pictures(17, true), 17);
    }

    #[test]
    fn movie_duration_rounds_up_to_cover_the_frame() {
        assert_eq!(movie_duration_ticks(1, 1000, 60), Some(17));
        assert_eq!(movie_duration_ticks(17, 1000, 60), Some(284));
        assert_eq!(movie_duration_ticks(30, 1000, 60), Some(500));
        assert_eq!(movie_duration_ticks(600, 1000, 60), Some(10000));
        assert_eq!(movie_duration_ticks(17, 1000, 30), Some(567));
        assert_eq!(movie_duration_ticks(1, 0, 60), None);
    }

    #[test]
    fn encode_eta_scales_with_pictures_left() {
        assert_eq!(eta_seconds(20.0, 10, 40), Some(60.0));
        assert_eq!(eta_seconds(1.0, 0, 10), None);
        assert_eq!(eta_seconds(5.0, 10, 10), None);
        assert_eq!(format_eta(5.2), "5 秒");
        assert_eq!(format_eta(65.0), "1 分 5 秒");
        assert_eq!(format_eta(60.0), "1 分");
        assert_eq!(format_eta(90.0), "1 分 30 秒");
        assert_eq!(format_eta(3720.0), "1 小时 2 分");
        assert_eq!(format_eta(3600.0), "1 小时");
        assert_eq!(
            with_remaining("编码画面 1/4", 20.0, 10, 40),
            "编码画面 1/4, 剩余 1 分..."
        );
        assert_eq!(
            with_remaining("编码画面 1/4", 0.2, 1, 4),
            "编码画面 1/4, 剩余不到 1 秒..."
        );
        assert_eq!(with_remaining("编码画面 1/4", 0.0, 0, 4), "编码画面 1/4...");
        assert_eq!(progress_frame("frame=12"), Some(12));
        assert_eq!(progress_frame("frame=  3"), Some(3));
        assert_eq!(progress_frame("bitrate=1"), None);
    }

    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn timing_payload(len: usize, writes: &[(usize, u32)]) -> Vec<u8> {
        let mut payload = vec![0u8; len];
        for (at, value) in writes {
            payload[*at..*at + 4].copy_from_slice(&value.to_be_bytes());
        }
        payload
    }

    #[test]
    fn stretch_one_hardware_frame_to_the_page_length() {
        let stts = mp4_box(b"stts", &timing_payload(16, &[(4, 1), (8, 1), (12, 1)]));
        let stbl = mp4_box(b"stbl", &stts);
        let minf = mp4_box(b"minf", &stbl);
        let mdhd = mp4_box(b"mdhd", &timing_payload(24, &[(12, 60), (16, 1)]));
        let mdia = mp4_box(b"mdia", &[mdhd, minf].concat());
        let elst = mp4_box(b"elst", &timing_payload(20, &[(4, 1), (8, 17), (12, 0)]));
        let edts = mp4_box(b"edts", &elst);
        let tkhd = mp4_box(b"tkhd", &timing_payload(84, &[(20, 17)]));
        let trak = mp4_box(b"trak", &[tkhd, edts, mdia].concat());
        let mvhd = mp4_box(b"mvhd", &timing_payload(100, &[(12, 1000), (16, 17)]));
        let mut file = mp4_box(b"moov", &[mvhd, trak].concat());

        stretch_still_bytes(&mut file, 17, 60).unwrap();
        fn field(data: &[u8], kind: &[u8; 4], rel: usize) -> u32 {
            let at = data.windows(4).position(|w| w == kind.as_slice()).unwrap();
            u32::from_be_bytes(data[at + 4 + rel..at + 8 + rel].try_into().unwrap())
        }
        assert_eq!(field(&file, b"stts", 12), 17);
        assert_eq!(field(&file, b"mdhd", 16), 17);
        assert_eq!(field(&file, b"mvhd", 16), 284);
        assert_eq!(field(&file, b"tkhd", 20), 284);
        assert_eq!(field(&file, b"elst", 8), 284);

        stretch_still_bytes(&mut file, 1, 60).unwrap();
        assert_eq!(field(&file, b"stts", 12), 17);

        let stts_at = file
            .windows(4)
            .position(|w| w == b"stts".as_slice())
            .unwrap();
        file[stts_at + 4 + 15] = 2;
        assert!(stretch_still_bytes(&mut file, 30, 60).is_err());
    }

    #[test]
    fn held_hw_still_encodes_one_frame_at_the_export_rate() {
        let args = held_hw_segment_args(
            VideoCodec::Hevc,
            VideoEncoder::Qsv,
            60,
            1280,
            720,
            600,
            false,
            18,
            Path::new("a.mp4"),
        );
        let text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(text.windows(2).any(|pair| pair == ["-framerate", "60"]));
        assert!(text.windows(2).any(|pair| pair == ["-frames:v", "1"]));
        assert!(text.windows(2).any(|pair| pair == ["-c:v", "hevc_qsv"]));
        assert!(text.windows(2).any(|pair| pair == ["-pix_fmt", "nv12"]));
        assert!(text.windows(2).any(|pair| pair == ["-bf", "0"]));
        assert!(text
            .windows(2)
            .any(|pair| pair == ["-video_track_timescale", "60"]));
        assert!(!text.iter().any(|arg| arg == "-vf" || arg.contains('/')));
    }

    #[test]
    fn held_hw_transition_sends_every_frame_without_bf_on_mf() {
        let transition = held_hw_segment_args(
            VideoCodec::Avc,
            VideoEncoder::Qsv,
            60,
            640,
            360,
            30,
            true,
            18,
            Path::new("b.mp4"),
        );
        let text: Vec<String> = transition
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(text.windows(2).any(|pair| pair == ["-frames:v", "30"]));
        assert!(text.windows(2).any(|pair| pair == ["-c:v", "h264_qsv"]));

        let mf = held_hw_segment_args(
            VideoCodec::Avc,
            VideoEncoder::MediaFoundation,
            30,
            640,
            360,
            10,
            false,
            18,
            Path::new("c.mp4"),
        );
        let mf_text: Vec<String> = mf
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(mf_text.windows(2).any(|pair| pair == ["-c:v", "h264_mf"]));
        assert!(!mf_text.windows(2).any(|pair| pair == ["-bf", "0"]));
        assert!(!mf_text.iter().any(|arg| arg == "-vf"));
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

        let a_frames: u64 = runs.iter().filter(|r| r.gid == "a").map(|r| r.frames).sum();
        let b_frames: u64 = runs.iter().filter(|r| r.gid == "b").map(|r| r.frames).sum();
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
    fn wipe_overlay_is_not_a_color_fade() {
        let mut tl = Timeline::new();
        tl.video_clips = vec![clip("a", 0.0, 5.0), clip("b", 5.0, 10.0)];
        tl.fades.push(fade(FadeKind::WipeLtr, 5.0, 6.0));
        assert!(fade_overlay_at(&tl, 5.5, [1, 2, 3]).is_none());
        let (prev, next, p) = tl.wipe_at(5.5).unwrap();
        assert_eq!(prev, "a");
        assert_eq!(next, "b");
        assert!((p - 0.5).abs() < 1e-9);
    }

    #[test]
    fn apply_wipe_copies_left_from_next() {
        // 2x1 RGBA: prev = red, next = blue; p=0.5 → left blue, right red.
        let prev = vec![255, 0, 0, 255, 255, 0, 0, 255];
        let next = vec![0, 0, 255, 255, 0, 0, 255, 255];
        let mut dst = vec![0u8; 8];
        apply_wipe(&prev, &next, &mut dst, 0.5, 2);
        assert_eq!(&dst[..4], &[0, 0, 255, 255]);
        assert_eq!(&dst[4..], &[255, 0, 0, 255]);
        apply_wipe(&prev, &next, &mut dst, 0.0, 2);
        assert_eq!(dst, prev);
        apply_wipe(&prev, &next, &mut dst, 1.0, 2);
        assert_eq!(dst, next);
    }

    #[test]
    fn apply_dissolve_is_a_linear_mix() {
        let prev = vec![0, 0, 0, 255, 100, 0, 0, 255];
        let next = vec![100, 0, 0, 255, 0, 0, 0, 255];
        let mut dst = vec![0u8; 8];
        apply_dissolve(&prev, &next, &mut dst, 0.0);
        assert_eq!(dst, prev);
        apply_dissolve(&prev, &next, &mut dst, 1.0);
        assert_eq!(dst, next);
        apply_dissolve(&prev, &next, &mut dst, 0.5);
        assert_eq!(&dst[..4], &[50, 0, 0, 255]);
        assert_eq!(&dst[4..], &[50, 0, 0, 255]);

        let mut tl = Timeline::new();
        tl.video_clips = vec![clip("a", 0.0, 5.0), clip("b", 5.0, 10.0)];
        tl.fades.push(fade(FadeKind::Dissolve, 5.0, 6.0));
        assert!(fade_overlay_at(&tl, 5.5, [1, 2, 3]).is_none());
        let (prev_id, next_id, p, kind) = tl.page_blend_at(5.5).unwrap();
        assert_eq!((prev_id, next_id, kind), ("a", "b", FadeKind::Dissolve));
        assert!((p - 0.5).abs() < 1e-9);
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
        let a_frames: u64 = runs.iter().filter(|r| r.gid == "a").map(|r| r.frames).sum();
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
                let far_enough = last_onset
                    .map(|j| i.saturating_sub(j) >= min_gap)
                    .unwrap_or(true);
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
            assert!((t - exp).abs() < 0.005, "clip {i} onset {t} expected {exp}");
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
            assert!((t - exp).abs() < 0.005, "clip {i} onset {t} expected {exp}");
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
            codec: VideoCodec::Avc,
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
                notes[best_f], notes[i],
                "cut {:.3}s +0.2s audio is {} want {} (later clips drifted)",
                cut, notes[best_f], notes[i]
            );
        }
    }

    fn note_glyph(letter: u8) -> [u8; 7] {
        match letter {
            b'C' => [
                0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
            ],
            b'D' => [
                0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
            ],
            b'E' => [
                0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
            ],
            _ => [0; 7],
        }
    }

    fn draw_note_page(path: &Path, letter: u8, rgb: [u8; 3]) {
        let (w, h) = (160u32, 90u32);
        let mut img =
            image::RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        let glyph = note_glyph(letter);
        let scale = 8u32;
        let ox = (w - 5 * scale) / 2;
        let oy = (h - 7 * scale) / 2;
        for y in 0..7u32 {
            for x in 0..5u32 {
                if glyph[y as usize] & (1 << (4 - x)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        img.put_pixel(
                            ox + x * scale + dx,
                            oy + y * scale + dy,
                            image::Rgba([0, 0, 0, 255]),
                        );
                    }
                }
            }
        }
        img.save(path).unwrap();
    }

    fn write_edge_beep_wav(path: &Path, sr: u32, duration: f64, freq: f64) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let n = (duration * sr as f64).round() as u32;
        let beep_n = (0.08 * sr as f64).round() as u32;
        let end_beep = n.saturating_sub((0.38 * sr as f64).round() as u32);
        let end_beep_stop = n.saturating_sub((0.30 * sr as f64).round() as u32);
        for i in 0..n {
            let on = i < beep_n || (i >= end_beep && i < end_beep_stop);
            let s = if on {
                ((i as f64 * freq * 2.0 * std::f64::consts::PI / sr as f64).sin() * 20000.0) as i16
            } else {
                0
            };
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    /// 非整数秒的 flac: 开头和结尾各一声. 时长若被收成整数秒, 尾音会被裁掉,
    /// 下一段的开头音也会提前. 画面用音名字母, 核对翻页没有相对音频再偏一截.
    #[test]
    fn fractional_flac_tails_stay_with_the_page() {
        if !ffmpeg_available() {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("sv_frac_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let sr = 44100u32;
        let notes: [(&str, u8, f64, f64, [u8; 3]); 3] = [
            ("C", b'C', 261.626, 3.8, [229, 57, 53]),
            ("D", b'D', 293.665, 2.7, [251, 140, 0]),
            ("E", b'E', 329.628, 4.8, [253, 216, 53]),
        ];
        let mut pool = Vec::new();
        let mut tl = Timeline::new();
        let mut cursor = 0.0;
        let mut expect_onsets = Vec::new();
        let mut boundaries = Vec::new();
        for (name, letter, freq, dur, rgb) in notes {
            let wav = dir.join(format!("{name}.wav"));
            let flac = dir.join(format!("{name}.flac"));
            let png = dir.join(format!("{name}.png"));
            write_edge_beep_wav(&wav, sr, dur, freq);
            assert!(
                run_ff(&[
                    "-y",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-i",
                    wav.to_str().unwrap(),
                    "-c:a",
                    "flac",
                    flac.to_str().unwrap(),
                ]),
                "encode {name}.flac"
            );
            let probed = crate::audio::probe_duration(&flac).unwrap();
            assert!(
                (probed - dur).abs() < 0.002,
                "{name} probed {probed}, want {dur}"
            );
            draw_note_page(&png, letter, rgb);
            pool.push(MaterialItem {
                group_id: name.into(),
                label: name.into(),
                cache_path: png,
                width: 160,
                height: 90,
            });
            tl.video_clips.push(clip(name, cursor, cursor + probed));
            tl.audio_clips.push(audio_clip(flac, probed, 0.0));
            expect_onsets.push(cursor);
            expect_onsets.push(cursor + dur - 0.38);
            cursor += probed;
            boundaries.push(cursor);
        }

        let out = dir.join("CDE.mkv");
        let opts = ExportOptions {
            container: Container::Mkv,
            codec: VideoCodec::Avc,
            width: 160,
            height: 90,
            fps: 30,
            crf: 23,
            out_path: out.clone(),
            fade_bg_rgb: [255, 255, 255],
        };
        let rx = export_async(tl, pool, opts);
        loop {
            match rx.recv_blocking() {
                Ok(ExportMsg::Progress(_)) => {}
                Ok(ExportMsg::Done(Ok(_))) => break,
                Ok(ExportMsg::Done(Err(e))) => panic!("export failed: {e}"),
                Err(_) => panic!("export channel closed"),
            }
        }

        let wav = dir.join("CDE.wav");
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
        let onsets = beep_onsets(&wav, sr);
        assert_eq!(
            onsets.len(),
            expect_onsets.len(),
            "onsets={onsets:?} want={expect_onsets:?}"
        );
        for (got, exp) in onsets.iter().zip(expect_onsets.iter()) {
            assert!((got - exp).abs() < 0.03, "onset {got} expected {exp}");
        }

        let page = [notes[0].4, notes[1].4, notes[2].4];
        let checks = [
            (boundaries[0] - 0.05, 0usize),
            (boundaries[0] + 0.05, 1usize),
            (boundaries[1] - 0.05, 1usize),
            (boundaries[1] + 0.05, 2usize),
        ];
        for (i, (t, want)) in checks.iter().enumerate() {
            let frame = dir.join(format!("f{i}.png"));
            let ts = format!("{t:.6}");
            assert!(
                run_ff(&[
                    "-y",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-i",
                    out.to_str().unwrap(),
                    "-ss",
                    &ts,
                    "-frames:v",
                    "1",
                    frame.to_str().unwrap(),
                ]),
                "frame at {t}"
            );
            let img = image::open(&frame).unwrap().to_rgb8();
            let px = img.get_pixel(2, 2).0;
            let mut best = 0usize;
            let mut best_d = i32::MAX;
            for (j, c) in page.iter().enumerate() {
                let d = rgb_dist(px, *c);
                if d < best_d {
                    best_d = d;
                    best = j;
                }
            }
            assert_eq!(best, *want, "t={t:.3} corner {px:?} closest to page {best}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
