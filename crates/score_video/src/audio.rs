//! 预览音频播放引擎: 按 `AudioClip` 顺序拼接播放, 支持跳转到任意时刻.
//!
//! rodio 的 `Sink` 只能从头顺序播放已 append 的音源, 不支持中途寻址; 因此每次
//! 跳转 (播放/暂停/拖动播放头) 都重新创建 `Sink`, 定位到覆盖目标时刻的那条
//! `AudioClip`, 用 `skip_duration` 跳过其内部偏移, 再依次 append 该条剩余部分
//! 与后续所有片段.

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufReader;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rodio::decoder::Mp4Type;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::model::AudioClip;

static PREVIEW_LOCK: Mutex<()> = Mutex::new(());

pub struct AudioEngine {
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    clips: Vec<AudioClip>,
    playing: bool,
    started_at: Option<Instant>,
    base_time: f64,
}

impl AudioEngine {
    pub fn new() -> Self {
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                _stream: Some(stream),
                handle: Some(handle),
                sink: None,
                clips: Vec::new(),
                playing: false,
                started_at: None,
                base_time: 0.0,
            },
            Err(_) => Self {
                _stream: None,
                handle: None,
                sink: None,
                clips: Vec::new(),
                playing: false,
                started_at: None,
                base_time: 0.0,
            },
        }
    }

    pub fn set_clips(&mut self, clips: Vec<AudioClip>) {
        self.clips = clips;
        if self.playing {
            self.restart_from(self.current_time());
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// 播放中按墙钟估算当前时刻; 暂停时返回记录的播放头.
    pub fn current_time(&self) -> f64 {
        if self.playing {
            let elapsed = self
                .started_at
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            self.base_time + elapsed
        } else {
            self.base_time
        }
    }

    pub fn play_from(&mut self, t: f64) {
        self.base_time = t.max(0.0);
        self.started_at = Some(Instant::now());
        self.playing = true;
        self.restart_from(self.base_time);
    }

    pub fn pause(&mut self) {
        self.base_time = self.current_time();
        self.playing = false;
        self.started_at = None;
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
    }

    /// 拖动播放头 (无论播放/暂停中都调用).
    pub fn seek(&mut self, t: f64) {
        self.base_time = t.max(0.0);
        self.started_at = Some(Instant::now());
        if self.playing {
            self.restart_from(self.base_time);
        }
    }

    fn restart_from(&mut self, t: f64) {
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        let Ok(sink) = Sink::try_new(handle) else {
            return;
        };
        let mut cum = 0.0;
        for clip in &self.clips {
            let end = cum + clip.duration;
            if end > t {
                let local_offset = (t - cum).max(0.0);
                if let Some(dec) = open_decoder(&clip.path) {
                    // `clip.offset` 是该段在源文件里的起始时刻 (分割音频
                    // 产生的后半段 > 0), `local_offset` 是本次寻址点相对
                    // 这一段自己起点的偏移, 两者相加才是源文件里真正要
                    // 跳到的位置.
                    let skip = Duration::from_secs_f64(clip.offset + local_offset);
                    if let Ok(src) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        dec.skip_duration(skip)
                    })) {
                        sink.append(src);
                    }
                }
            }
            cum = end;
        }
        sink.play();
        self.sink = Some(sink);
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// MPEG-4 / 裸 AAC: rodio 0.19 初始化时遇到 SeekError 会 `unreachable!` 直接崩
/// (常见于带封面图的 iTunes/商店 m4a). 预览改走 ffmpeg 转 WAV.
pub fn needs_ffmpeg_preview(path: &Path) -> bool {
    matches!(
        file_ext(path).as_str(),
        "m4a" | "m4b" | "m4r" | "mp4" | "m4v" | "mov" | "aac"
    )
}

fn file_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn preview_wav_path(src: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut hasher);
    if let Ok(meta) = src.metadata() {
        meta.len().hash(&mut hasher);
        if let Ok(mtime) = meta.modified() {
            mtime.hash(&mut hasher);
        }
    }
    std::env::temp_dir()
        .join("score_sync_audio")
        .join(format!("{:016x}.wav", hasher.finish()))
}

pub fn preview_wav_ready(src: &Path) -> bool {
    !needs_ffmpeg_preview(src) || preview_wav_path(src).is_file()
}

/// 把 m4a/aac 等转成临时 WAV 供预览/波形. 已是 wav 则原样返回.
/// 只取第一条音轨 (`-map 0:a:0`), 避开封面 MJPEG.
pub fn ensure_preview_wav(src: &Path) -> Option<PathBuf> {
    if !needs_ffmpeg_preview(src) {
        return Some(src.to_path_buf());
    }
    let _guard = PREVIEW_LOCK.lock().ok()?;
    let out = preview_wav_path(src);
    if out.is_file() {
        return Some(out);
    }
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension("wav.part");
    let mut cmd = Command::new(crate::export::ffmpeg_path());
    cmd.args([
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
    ])
    .arg(src)
    .args(["-vn", "-map", "0:a:0", "-acodec", "pcm_s16le", "-f", "wav"])
    .arg(&tmp)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = cmd.status().ok()?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// 打开预览解码器. m4a 只在已转好 WAV 时打开, 绝不让 rodio 直接碰 MPEG-4
/// (会 unreachable panic). 未转好则返回 None, 调用方应先 `ensure_preview_wav`.
pub fn open_decoder(path: &Path) -> Option<Decoder<BufReader<File>>> {
    let decode_path = if needs_ffmpeg_preview(path) {
        let wav = preview_wav_path(path);
        if !wav.is_file() {
            return None;
        }
        wav
    } else {
        path.to_path_buf()
    };
    open_decoder_raw(&decode_path)
}

fn open_decoder_raw(path: &Path) -> Option<Decoder<BufReader<File>>> {
    let ext = file_ext(path);
    let open = || File::open(path).ok().map(BufReader::new);
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| match ext.as_str() {
        "m4a" | "m4b" | "m4r" => {
            let hinted = open().and_then(|r| Decoder::new_mp4(r, Mp4Type::M4a).ok());
            hinted.or_else(|| open().and_then(|r| Decoder::new(r).ok()))
        }
        "mp4" | "m4v" | "mov" => {
            let hinted = open().and_then(|r| Decoder::new_mp4(r, Mp4Type::Mp4).ok());
            hinted.or_else(|| open().and_then(|r| Decoder::new(r).ok()))
        }
        "aac" => {
            let hinted = open().and_then(|r| Decoder::new_aac(r).ok());
            hinted.or_else(|| open().and_then(|r| Decoder::new(r).ok()))
        }
        _ => open().and_then(|r| Decoder::new(r).ok()),
    }));
    result.ok().flatten()
}

/// 探测音频文件总时长; 导入时调用.
///
/// `.wav` 走 `hound` 读头. `.m4a` 等 MPEG-4 只问 ffmpeg, 绝不走 rodio
/// (带封面的商店 m4a 会在 rodio 初始化时 panic).
/// 其它格式先 rodio 元数据, 失败再 ffmpeg, 最后整段计数.
pub fn probe_duration(path: &Path) -> Option<f64> {
    let ext = file_ext(path);
    if ext == "wav" {
        if let Ok(reader) = hound::WavReader::open(path) {
            let sr = reader.spec().sample_rate as f64;
            let frames = reader.duration() as f64;
            if sr > 0.0 {
                return Some(frames / sr);
            }
        }
    }
    if needs_ffmpeg_preview(path) {
        return ffmpeg_probe_duration(path);
    }
    if let Some(dec) = open_decoder_raw(path) {
        if let Some(d) = std::panic::catch_unwind(AssertUnwindSafe(|| dec.total_duration()))
            .ok()
            .flatten()
        {
            let secs = d.as_secs_f64();
            if secs > 0.001 {
                return Some(secs);
            }
        }
    }
    if let Some(secs) = ffmpeg_probe_duration(path) {
        return Some(secs);
    }
    let dec = open_decoder_raw(path)?;
    let sample_rate = dec.sample_rate() as f64;
    let channels = dec.channels() as f64;
    if sample_rate <= 0.0 || channels <= 0.0 {
        return None;
    }
    let n = std::panic::catch_unwind(AssertUnwindSafe(|| dec.count()))
        .ok()? as f64;
    Some(n / sample_rate / channels)
}

fn ffmpeg_probe_duration(path: &Path) -> Option<f64> {
    let mut cmd = Command::new(crate::export::ffmpeg_path());
    cmd.arg("-hide_banner")
        .arg("-i")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    parse_ffmpeg_duration(&String::from_utf8_lossy(&output.stderr))
}

fn parse_ffmpeg_duration(stderr: &str) -> Option<f64> {
    let idx = stderr.find("Duration:")?;
    let rest = stderr[idx + "Duration:".len()..].trim_start();
    let token = rest.split(',').next()?.trim();
    if token.is_empty() || token.eq_ignore_ascii_case("N/A") {
        return None;
    }
    let mut parts = token.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    let secs = h * 3600.0 + m * 60.0 + s;
    (secs > 0.001).then_some(secs)
}

#[cfg(test)]
mod tests {
    use super::parse_ffmpeg_duration;

    #[test]
    fn parse_ffmpeg_m4a_duration_line() {
        let log = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.m4a':\n  Duration: 00:03:24.56, start: 0.000000, bitrate: 256 kb/s\n";
        let d = parse_ffmpeg_duration(log).unwrap();
        assert!((d - 204.56).abs() < 0.01);
    }

    #[test]
    fn parse_ffmpeg_duration_na() {
        assert!(parse_ffmpeg_duration("Duration: N/A, start: 0.000000").is_none());
    }

    #[test]
    fn decode_user_m4a_steps() {
        let p = std::path::Path::new(
            r"D:\Tencent Save\QQ download\04 Piano Concerto No. 26 in D Major, K. 537 _Coronation__ 1. Allegro.m4a",
        );
        if !p.is_file() {
            eprintln!("skip: file missing");
            return;
        }
        let open = std::panic::catch_unwind(|| super::open_decoder(p));
        assert!(open.is_ok(), "open_decoder must not panic");
        let probe = std::panic::catch_unwind(|| super::probe_duration(p));
        assert!(probe.is_ok(), "probe_duration must not panic");
        let dur = probe.unwrap().expect("ffmpeg should read m4a duration");
        assert!((dur - 827.23).abs() < 0.5, "dur={dur}");
        let raw = std::panic::catch_unwind(|| super::open_decoder_raw(p));
        assert!(raw.is_ok(), "open_decoder_raw must catch rodio panic");
        assert!(raw.unwrap().is_none(), "rodio must not decode this m4a");
    }
}
