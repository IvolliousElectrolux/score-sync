//! ffmpeg 导出流水线.
//!
//! 淡入淡出用 ffmpeg `fade` 滤镜 (黑场), 每个「淡入淡出区间」单独编码成一段
//! (`fade=t=in/out:st=0:d=区间长度`), 段与段之间再用 `-c:v copy` 无损拼接;
//! 这样避免了在同一路视频上链式叠加多个 `fade` 滤镜导致黑场互相污染的问题
//! (参考 `make_score_video.py` 已验证过的做法).

use std::collections::HashMap;
use std::io::Read;
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

struct Section {
    t0: f64,
    t1: f64,
    fade: Option<FadeKind>,
}

fn plan_sections(timeline: &Timeline) -> Vec<Section> {
    let end = timeline.timeline_end();
    let mut cuts: Vec<f64> = vec![0.0, end];
    for f in &timeline.fades {
        cuts.push(f.start.clamp(0.0, end));
        cuts.push(f.end.clamp(0.0, end));
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

    let mut sections = Vec::new();
    for i in 0..cuts.len().saturating_sub(1) {
        let t0 = cuts[i];
        let t1 = cuts[i + 1];
        if t1 - t0 < 1e-6 {
            continue;
        }
        let fade = timeline
            .fades
            .iter()
            .find(|f| (f.start - t0).abs() < 1e-6 && (f.end - t1).abs() < 1e-6)
            .map(|f| f.kind);
        sections.push(Section { t0, t1, fade });
    }
    sections
}

/// 某分段内, 各素材图片按时间顺序应显示的子时长.
fn section_segments(timeline: &Timeline, sec: &Section) -> Vec<(String, f64)> {
    let mut segs = Vec::new();
    for c in &timeline.video_clips {
        let s = c.start.max(sec.t0);
        let e = c.end.min(sec.t1);
        if e - s > 1e-6 {
            segs.push((c.group_id.clone(), e - s));
        }
    }
    segs
}

const BLACK_KEY: &str = "__black__";

fn escape_concat_path(p: &Path) -> String {
    p.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', r"'\''")
}

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

fn dump_pool_images(
    pool: &[MaterialItem],
    dir: &Path,
    target_w: u32,
    target_h: u32,
) -> Result<HashMap<String, PathBuf>, String> {
    let target_w = even_dim(target_w);
    let target_h = even_dim(target_h);
    let mut map = HashMap::new();
    for item in pool {
        let path = dir.join(format!("{}.png", item.group_id));
        let rgba = item.load_rgba()?;
        let (w, h) = rgba.dimensions();
        // 即便原图尺寸与目标「看起来」一样, 只要目标被 even_dim 抬过 (原图是
        // 奇数), 也必须走 fit_pad 补齐, 否则 libx264 会因奇数分辨率打不开.
        let result = if w == target_w && h == target_h {
            // 已是目标尺寸时可直接拷缓存文件, 省一次编解码
            if std::fs::copy(&item.cache_path, &path).is_ok() {
                Ok(())
            } else {
                rgba.save(&path).map(|_| ())
            }
        } else {
            fit_pad(&rgba, target_w, target_h).save(&path)
        };
        result.map_err(|e| format!("写入素材图 {} 失败: {e}", item.group_id))?;
        map.insert(item.group_id.clone(), path);
    }
    Ok(map)
}

fn build_concat_list(
    dir: &Path,
    name: &str,
    images: &HashMap<String, PathBuf>,
    segs: &[(String, f64)],
) -> Result<PathBuf, String> {
    let mut lines = Vec::new();
    let mut last_path: Option<&PathBuf> = None;
    for (gid, dur) in segs {
        let p = images
            .get(gid)
            .ok_or_else(|| format!("素材 {gid} 缺少图片, 无法导出"))?;
        lines.push(format!("file '{}'", escape_concat_path(p)));
        lines.push(format!("duration {dur:.6}"));
        last_path = Some(p);
    }
    // ffmpeg concat demuxer 对静帧素材的最后一条 duration 会再多播一次, 需要
    // 重复写最后一条 file 行 (无 duration) 并配合外层 `-t` 截断.
    if let Some(p) = last_path {
        lines.push(format!("file '{}'", escape_concat_path(p)));
    }
    let list_path = dir.join(format!("{name}.txt"));
    std::fs::write(&list_path, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(list_path)
}

/// 跑一次 ffmpeg 子进程.
///
/// - Windows 下加 `CREATE_NO_WINDOW`, 不再弹出黑色控制台窗口.
/// - ffmpeg 把人类可读日志和进度都写到 stderr, 而且刷新进度那行是用 `\r`
///   反复覆盖同一行 (不是 `\n` 分行), 这里按 `\r`/`\n` 都切一次, 每切出一段
///   非空文本就转发一条 `ExportMsg::Progress`, 这样进度/日志直接显示在应用
///   自己的导出弹窗里, 不用再弹一个终端窗口出来给用户看.
/// - 失败时把 stderr 尾部内容 (最近若干行) 一并放进错误信息里, 方便诊断.
fn run_ffmpeg(
    args: &[std::ffi::OsString],
    step: &str,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg_path());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "启动 ffmpeg 失败 ({step}): {e} — 请确认程序同目录下有 ffmpeg.exe, \
             或系统已安装 ffmpeg 并加入 PATH"
        )
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

    let status = child
        .wait()
        .map_err(|e| format!("等待 ffmpeg 结束失败 ({step}): {e}"))?;
    if let Some(h) = reader_handle {
        let _ = h.join();
    }
    if !status.success() {
        let detail = tail.lock().map(|t| t.trim().to_string()).unwrap_or_default();
        return Err(if detail.is_empty() {
            format!("ffmpeg 执行失败 ({step}), 退出码 {:?}", status.code())
        } else {
            format!("ffmpeg 执行失败 ({step}), 退出码 {:?}:\n{detail}", status.code())
        });
    }
    Ok(())
}

fn os(s: impl Into<String>) -> std::ffi::OsString {
    std::ffi::OsString::from(s.into())
}
fn os_path(p: &Path) -> std::ffi::OsString {
    p.as_os_str().to_owned()
}

#[allow(clippy::too_many_arguments)]
fn encode_section(
    list_path: &Path,
    out_path: &Path,
    duration: f64,
    fade: Option<(FadeKind, f64)>,
    opts: &ExportOptions,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    // 尺寸/像素格式已经在 `dump_pool_images`/`fit_pad` 那边统一处理好了 (每
    // 张图都精确等于偶数的 opts.width x opts.height 的不透明 RGBA), 这里只
    // 需要转帧率和转 yuv420p. 额外显式传 `-pix_fmt yuv420p`, 避免个别
    // ffmpeg 构建在 format 滤镜协商时漂移.
    let mut vf = format!("fps={fps},format=yuv420p", fps = opts.fps);
    if let Some((kind, d)) = fade {
        let t = match kind {
            FadeKind::In => "in",
            FadeKind::Out => "out",
        };
        vf.push_str(&format!(",fade=t={t}:st=0:d={d:.6}"));
    }
    let args = vec![
        os("-y"),
        os("-f"),
        os("concat"),
        os("-safe"),
        os("0"),
        os("-i"),
        os_path(list_path),
        os("-t"),
        os(format!("{duration:.6}")),
        os("-vf"),
        os(vf),
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
        os("-an"),
        os_path(out_path),
    ];
    run_ffmpeg(&args, "编码分段", tx)
}

fn concat_sections(
    dir: &Path,
    section_files: &[PathBuf],
    out_silent: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    let mut lines = Vec::new();
    for f in section_files {
        lines.push(format!("file '{}'", escape_concat_path(f)));
    }
    let list_path = dir.join("all_sections.txt");
    std::fs::write(&list_path, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    let args = vec![
        os("-y"),
        os("-f"),
        os("concat"),
        os("-safe"),
        os("0"),
        os("-i"),
        os_path(&list_path),
        os("-c:v"),
        os("copy"),
        os_path(out_silent),
    ];
    run_ffmpeg(&args, "拼接分段", tx)
}

fn build_audio(
    clips: &[AudioClip],
    target_duration: f64,
    out_audio: &Path,
    codec: &str,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    if clips.is_empty() {
        let args = vec![
            os("-y"),
            os("-f"),
            os("lavfi"),
            os("-i"),
            os("anullsrc=r=44100:cl=stereo"),
            os("-t"),
            os(format!("{target_duration:.6}")),
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
    if clips.len() == 1 {
        let c = &clips[0];
        let args = vec![
            os("-y"),
            os("-ss"),
            os(format!("{:.6}", c.offset)),
            os("-t"),
            os(format!("{:.6}", c.duration)),
            os("-i"),
            os_path(&c.path),
            os("-c:a"),
            os(codec),
            os_path(out_audio),
        ];
        return run_ffmpeg(&args, "转码音频", tx);
    }
    let mut args = vec![os("-y")];
    for c in clips {
        args.push(os("-ss"));
        args.push(os(format!("{:.6}", c.offset)));
        args.push(os("-t"));
        args.push(os(format!("{:.6}", c.duration)));
        args.push(os("-i"));
        args.push(os_path(&c.path));
    }
    let mut filter = String::new();
    for i in 0..clips.len() {
        filter.push_str(&format!("[{i}:a]"));
    }
    filter.push_str(&format!("concat=n={}:v=0:a=1[aout]", clips.len()));
    args.push(os("-filter_complex"));
    args.push(os(filter));
    args.push(os("-map"));
    args.push(os("[aout]"));
    args.push(os("-c:a"));
    args.push(os(codec));
    args.push(os_path(out_audio));
    run_ffmpeg(&args, "合并多段音频", tx)
}

fn mux_final(
    video: &Path,
    audio: &Path,
    out_path: &Path,
    tx: &async_channel::Sender<ExportMsg>,
) -> Result<(), String> {
    // 音频在 `build_audio` 里已经按容器要求编码成 aac/flac 了, 这里只是把
    // 两路已经编码好的流封装进同一个容器, 直接 copy 即可, 不必再转一次码
    // (否则 MKV 无损 flac 会在这一步又被强行转成有损 aac, 白做了).
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
        os("-shortest"),
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
    let work = WorkDir::new().map_err(|e| format!("创建临时目录失败: {e}"))?;
    let mut images = dump_pool_images(pool, work.path(), opts.width, opts.height)?;
    let black_path = work.path().join("__black__.png");
    // 用不透明黑 (alpha=255), 和 `fit_pad` 补边用的颜色/像素格式完全一致,
    // 保证喂给 ffmpeg 的每一帧尺寸、格式都一样 (见 `fit_pad` 上的说明).
    RgbaImage::from_pixel(
        even_dim(opts.width),
        even_dim(opts.height),
        image::Rgba([0, 0, 0, 255]),
    )
        .save(&black_path)
        .map_err(|e| format!("生成黑帧失败: {e}"))?;
    images.insert(BLACK_KEY.to_string(), black_path);

    let sections = plan_sections(timeline);
    if sections.is_empty() {
        return Err("时间轴为空, 无法导出".to_string());
    }

    let mut section_files = Vec::new();
    for (i, sec) in sections.iter().enumerate() {
        let _ = tx.send_blocking(ExportMsg::Progress(format!(
            "编码分段 {}/{}...",
            i + 1,
            sections.len()
        )));
        let mut segs = section_segments(timeline, sec);
        if segs.is_empty() {
            segs.push((BLACK_KEY.to_string(), sec.t1 - sec.t0));
        }
        let list_path = build_concat_list(work.path(), &format!("sec{i}"), &images, &segs)?;
        let out_mp4 = work.path().join(format!("sec{i}.mp4"));
        let fade = sec.fade.map(|k| (k, sec.t1 - sec.t0));
        encode_section(&list_path, &out_mp4, sec.t1 - sec.t0, fade, opts, tx)?;
        section_files.push(out_mp4);
    }

    let _ = tx.send_blocking(ExportMsg::Progress("拼接分段...".to_string()));
    let silent_video = work.path().join("silent.mp4");
    concat_sections(work.path(), &section_files, &silent_video, tx)?;

    let _ = tx.send_blocking(ExportMsg::Progress("合成音频...".to_string()));
    let audio_out = work
        .path()
        .join(format!("audio.{}", opts.container.audio_ext()));
    build_audio(
        &timeline.audio_clips,
        timeline.timeline_end(),
        &audio_out,
        opts.container.audio_codec(),
        tx,
    )?;

    let _ = tx.send_blocking(ExportMsg::Progress("合并音视频...".to_string()));
    if let Some(parent) = opts.out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    mux_final(&silent_video, &audio_out, &opts.out_path, tx)?;

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
