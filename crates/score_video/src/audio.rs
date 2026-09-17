//! 预览音频播放引擎: 按 `AudioClip` 顺序拼接播放, 支持跳转到任意时刻.
//!
//! rodio 的 `Sink` 只能从头顺序播放已 append 的音源, 不支持中途寻址; 因此每次
//! 跳转 (播放/暂停/拖动播放头) 都重新创建 `Sink`, 定位到覆盖目标时刻的那条
//! `AudioClip`. 1x 对 16-bit WAV (含 m4a 转出来的预览 WAV) 按采样点 `seek`,
//! 不要用 `skip_duration` 从头解码 (长文件暂停再播会卡, 墙钟却继续走, 出声时
//! 播放头已经往后漂). 其它格式和倍速预览走 ffmpeg, 输出 `-f wav` 再喂 rodio
//! (裁剪 sidecar 的 configure 名是 `pcm_s16le`, 旧的 `-f s16le` 编不进去).

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
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
    /// 预览倍速 (仅播放引擎, 不影响导出).
    speed: f32,
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
                speed: 1.0,
            },
            Err(_) => Self {
                _stream: None,
                handle: None,
                sink: None,
                clips: Vec::new(),
                playing: false,
                started_at: None,
                base_time: 0.0,
                speed: 1.0,
            },
        }
    }

    pub fn set_clips(&mut self, clips: Vec<AudioClip>) {
        self.clips = clips;
        if self.playing {
            self.begin_playback_at(self.current_time());
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// 播放中按墙钟×倍速估算当前时刻; 暂停, 或解码还没就绪时, 停在 `base_time`.
    pub fn current_time(&self) -> f64 {
        if self.playing {
            let elapsed = self
                .started_at
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            self.base_time + elapsed * (self.speed as f64)
        } else {
            self.base_time
        }
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// 预览倍速. 播放中途改速会按当前时刻重建解码 (ffmpeg atempo 保音调).
    pub fn set_speed(&mut self, speed: f32) {
        let speed = speed.clamp(0.25, 4.0);
        if (speed - self.speed).abs() < 1e-4 {
            return;
        }
        let t = self.current_time();
        self.speed = speed;
        if self.playing {
            self.begin_playback_at(t);
        }
    }

    pub fn play_from(&mut self, t: f64) {
        self.begin_playback_at(t);
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
        let t = t.max(0.0);
        if self.playing {
            self.begin_playback_at(t);
        } else {
            self.base_time = t;
            self.started_at = None;
        }
    }

    /// 先完成寻址再开墙钟, 避免拉起解码的时间被算进播放头.
    fn begin_playback_at(&mut self, t: f64) {
        self.base_time = t.max(0.0);
        self.playing = true;
        self.started_at = None;
        self.restart_from(self.base_time);
        self.started_at = Some(Instant::now());
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
        let stretch = (self.speed - 1.0).abs() > 1e-3;
        let mut cum = 0.0;
        for clip in &self.clips {
            let end = cum + clip.duration;
            if end > t {
                let local_offset = (t - cum).max(0.0);
                let remain = (clip.duration - local_offset).max(0.0);
                let src_t = clip.offset + local_offset;
                if remain >= 1e-4 {
                    // `clip.offset` 是该段在源文件里的起始时刻 (分割音频
                    // 产生的后半段 > 0), `local_offset` 是本次寻址点相对
                    // 这一段自己起点的偏移, 两者相加才是源文件里真正要
                    // 跳到的位置.
                    if stretch {
                        if let Some(src) =
                            open_atempo_source(&clip.path, src_t, remain, self.speed)
                        {
                            sink.append(src);
                        }
                    } else if let Some(src) = open_wav_slice(&clip.path, src_t, remain) {
                        sink.append(src);
                    } else if let Some(src) = open_atempo_source(&clip.path, src_t, remain, 1.0)
                    {
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

fn decode_path(path: &Path) -> Option<PathBuf> {
    if needs_ffmpeg_preview(path) {
        let wav = preview_wav_path(path);
        wav.is_file().then_some(wav)
    } else if path.is_file() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

/// 16-bit PCM WAV 按采样点寻址, 避免 rodio `skip_duration` 从头解码.
struct WavSliceSource {
    samples: hound::WavIntoSamples<BufReader<File>, i16>,
    channels: u16,
    sample_rate: u32,
    remaining: u64,
}

fn open_wav_slice(path: &Path, start: f64, duration: f64) -> Option<WavSliceSource> {
    let decode = decode_path(path)?;
    if duration < 1e-4 {
        return None;
    }
    let mut reader = hound::WavReader::new(BufReader::new(File::open(&decode).ok()?)).ok()?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return None;
    }
    let sr = spec.sample_rate.max(1);
    let ch = spec.channels.max(1);
    let total = reader.duration() as u64;
    let start_idx = ((start.max(0.0) * sr as f64).round() as u64).min(total);
    reader.seek(start_idx as u32).ok()?;
    let want = ((duration.max(0.0) * sr as f64).round() as u64).saturating_mul(ch as u64);
    let avail = total.saturating_sub(start_idx).saturating_mul(ch as u64);
    Some(WavSliceSource {
        samples: reader.into_samples::<i16>(),
        channels: ch,
        sample_rate: sr,
        remaining: want.min(avail),
    })
}

impl Iterator for WavSliceSource {
    type Item = i16;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.samples.next()?.ok()
    }
}

impl Source for WavSliceSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        let ch = self.channels.max(1) as u64;
        let sr = self.sample_rate.max(1) as f64;
        let secs = (self.remaining / ch) as f64 / sr;
        Some(Duration::from_secs_f64(secs.max(0.0)))
    }
}

/// ffmpeg `atempo` 单级只接受 0.5..=2.0, 3x 拆成 2×1.5.
pub(crate) fn atempo_filter(speed: f32) -> String {
    let mut s = speed as f64;
    let mut parts = Vec::new();
    while s > 2.0 + 1e-4 {
        parts.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s < 0.5 - 1e-4 {
        parts.push("atempo=0.5".to_string());
        s *= 2.0;
    }
    if (s - 1.0).abs() > 1e-4 {
        parts.push(format!("atempo={s:.5}"));
    }
    if parts.is_empty() {
        "anull".into()
    } else {
        parts.join(",")
    }
}

const ATEMPO_RATE: u32 = 44100;
const ATEMPO_CH: u16 = 2;

/// ffmpeg 往非 seekable 管道写 WAV 时, RIFF/`data` 长度经常是 `0xFFFFFFFF`.
/// hound 会因此报 "data chunk length is not a multiple of sample size".
/// 这里只定位到 `data` 载荷, 实际采样读到 EOF.
fn skip_to_wav_data<R: Read>(reader: &mut R) -> bool {
    let mut riff = [0u8; 12];
    if reader.read_exact(&mut riff).is_err() {
        return false;
    }
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return false;
    }
    loop {
        let mut hdr = [0u8; 8];
        if reader.read_exact(&mut hdr).is_err() {
            return false;
        }
        let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        if &hdr[0..4] == b"data" {
            return true;
        }
        let skip = (len as u64) + u64::from(len % 2);
        let mut buf = [0u8; 1024];
        let mut left = skip;
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            if reader.read_exact(&mut buf[..n]).is_err() {
                return false;
            }
            left -= n as u64;
        }
    }
}

/// 按需启动 ffmpeg atempo, 把 WAV 管道喂给 rodio (变速不变调).
/// 必须用 `-f wav`: 裁剪 sidecar 没有 `s16le` muxer (configure 名是 `pcm_s16le`).
struct AtempoSource {
    path: PathBuf,
    start: f64,
    duration: f64,
    speed: f32,
    child: Option<Child>,
    stdout: Option<BufReader<ChildStdout>>,
}

fn open_atempo_source(path: &Path, start: f64, duration: f64, speed: f32) -> Option<AtempoSource> {
    let decode = decode_path(path)?;
    if duration < 1e-3 {
        return None;
    }
    let mut src = AtempoSource {
        path: decode,
        start: start.max(0.0),
        duration,
        speed,
        child: None,
        stdout: None,
    };
    // 先拉起 ffmpeg 并越过 WAV 头, 失败就不要 append 空源 (否则播放头走、没声).
    src.ensure_started().then_some(src)
}

impl AtempoSource {
    fn ensure_started(&mut self) -> bool {
        if self.stdout.is_some() {
            return true;
        }
        let mut cmd = Command::new(crate::export::ffmpeg_path());
        cmd.arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-ss")
            .arg(format!("{:.6}", self.start))
            .arg("-t")
            .arg(format!("{:.6}", self.duration))
            .arg("-i")
            .arg(&self.path)
            .arg("-vn")
            .arg("-map")
            .arg("0:a:0");
        if (self.speed - 1.0).abs() > 1e-3 {
            cmd.arg("-af").arg(atempo_filter(self.speed));
        }
        cmd.arg("-f")
            .arg("wav")
            .arg("-acodec")
            .arg("pcm_s16le")
            .arg("-ac")
            .arg(ATEMPO_CH.to_string())
            .arg("-ar")
            .arg(ATEMPO_RATE.to_string())
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return false,
        };
        let mut stdout = match child.stdout.take() {
            Some(s) => BufReader::new(s),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        };
        if !skip_to_wav_data(&mut stdout) {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        self.child = Some(child);
        self.stdout = Some(stdout);
        true
    }
}

impl Iterator for AtempoSource {
    type Item = i16;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.ensure_started() {
            return None;
        }
        let stdout = self.stdout.as_mut()?;
        let mut buf = [0u8; 2];
        stdout.read_exact(&mut buf).ok()?;
        Some(i16::from_le_bytes(buf))
    }
}

impl Source for AtempoSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        ATEMPO_CH
    }

    fn sample_rate(&self) -> u32 {
        ATEMPO_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        let secs = self.duration / (self.speed as f64).max(0.01);
        Some(Duration::from_secs_f64(secs.max(0.0)))
    }
}

impl Drop for AtempoSource {
    fn drop(&mut self) {
        self.stdout.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 轨道波形峰值: 流式扫一遍采样, 不要 `collect` 整段 PCM.
/// 长 CD 轨 stereo 44.1kHz 整段解开会到数百 MB, 峰值算完后堆又不还给 OS.
pub fn waveform_peaks(
    path: &Path,
    buckets_per_sec: f64,
    min_buckets: usize,
    max_buckets: usize,
) -> Option<(Vec<f32>, f64)> {
    let wav = ensure_preview_wav(path)?;
    if let Some(peaks) = waveform_peaks_hound(&wav, buckets_per_sec, min_buckets, max_buckets) {
        return Some(peaks);
    }
    let dec = open_decoder(path)?;
    let channels = (dec.channels() as usize).max(1);
    let sr = dec.sample_rate().max(1) as f64;
    let frames = dec
        .total_duration()
        .map(|d| (d.as_secs_f64() * sr).round() as usize)
        .filter(|n| *n > 0)?;
    Some(peaks_from_i16(
        dec,
        channels,
        frames,
        sr,
        buckets_per_sec,
        min_buckets,
        max_buckets,
    ))
}

fn waveform_peaks_hound(
    path: &Path,
    buckets_per_sec: f64,
    min_buckets: usize,
    max_buckets: usize,
) -> Option<(Vec<f32>, f64)> {
    let reader = hound::WavReader::open(path).ok()?;
    if reader.spec().bits_per_sample != 16
        || reader.spec().sample_format != hound::SampleFormat::Int
    {
        return None;
    }
    let channels = (reader.spec().channels as usize).max(1);
    let sr = reader.spec().sample_rate.max(1) as f64;
    let frames = reader.duration() as usize;
    if frames == 0 {
        return None;
    }
    Some(peaks_from_i16(
        reader.into_samples::<i16>().filter_map(Result::ok),
        channels,
        frames,
        sr,
        buckets_per_sec,
        min_buckets,
        max_buckets,
    ))
}

fn peaks_from_i16<I: Iterator<Item = i16>>(
    samples: I,
    channels: usize,
    frames: usize,
    sample_rate: f64,
    buckets_per_sec: f64,
    min_buckets: usize,
    max_buckets: usize,
) -> (Vec<f32>, f64) {
    let duration_secs = frames as f64 / sample_rate.max(1.0);
    let buckets = ((duration_secs * buckets_per_sec).ceil() as usize)
        .clamp(min_buckets.max(1), max_buckets.max(1));
    let per_bucket = (frames as f64 / buckets as f64).max(1.0);
    let mut peaks = vec![0f32; buckets];
    let mut frame = 0usize;
    let mut ch = 0usize;
    let mut cur_b = 0usize;
    let mut m: i32 = 0;
    for s in samples {
        let b = ((frame as f64 / per_bucket) as usize).min(buckets - 1);
        if b != cur_b {
            peaks[cur_b] = (m as f32 / i16::MAX as f32).clamp(0.0, 1.0);
            m = 0;
            cur_b = b;
        }
        m = m.max((s as i32).abs());
        ch += 1;
        if ch < channels {
            continue;
        }
        ch = 0;
        frame += 1;
        if frame >= frames {
            break;
        }
    }
    peaks[cur_b] = (m as f32 / i16::MAX as f32).clamp(0.0, 1.0);
    (peaks, duration_secs)
}

/// 源时间区间 `[t0, t1)` 上的波形峰值. 一列覆盖多个桶时取 max (缩小时
/// 保峰值); 覆盖不到一个桶时线性插值 (放大时不出现台阶). 列宽由调用方
/// 按当前轨道缩放决定, 因此粒度始终相对可视像素, 不是固定秒数.
pub fn waveform_peak_in_range(peaks: &[f32], source_dur: f64, t0: f64, t1: f64) -> f32 {
    let n = peaks.len();
    if n == 0 || source_dur <= 0.0 {
        return 0.0;
    }
    let nf = n as f64;
    let start_f = (t0 / source_dur * nf).clamp(0.0, nf);
    let end_f = (t1 / source_dur * nf).clamp(start_f, nf);
    let span = end_f - start_f;
    if span >= 1.0 {
        let s = (start_f as usize).min(n - 1);
        let e = (end_f.ceil() as usize).clamp(s + 1, n);
        peaks[s..e].iter().copied().fold(0.0f32, f32::max)
    } else {
        let i0 = (start_f.floor() as usize).min(n - 1);
        let i1 = (i0 + 1).min(n - 1);
        let frac = (start_f - i0 as f64) as f32;
        peaks[i0] * (1.0 - frac) + peaks[i1] * frac
    }
}

/// 打开预览解码器. m4a 只在已转好 WAV 时打开, 绝不让 rodio 直接碰 MPEG-4
/// (会 unreachable panic). 未转好则返回 None, 调用方应先 `ensure_preview_wav`.
pub fn open_decoder(path: &Path) -> Option<Decoder<BufReader<File>>> {
    open_decoder_raw(&decode_path(path)?)
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
pub fn probe_duration(path: &Path) -> Result<f64, crate::error::Error> {
    if !path.is_file() {
        return Err(crate::error::Error::AudioMissing(path.to_path_buf()));
    }
    let ext = file_ext(path);
    if ext == "wav" {
        if let Ok(reader) = hound::WavReader::open(path) {
            let sr = reader.spec().sample_rate as f64;
            let frames = reader.duration() as f64;
            if sr > 0.0 {
                return Ok(frames / sr);
            }
        }
    }
    if needs_ffmpeg_preview(path) {
        return ffmpeg_probe_duration(path).ok_or_else(|| {
            crate::error::Error::audio_probe(path, "ffmpeg 无法读取时长 (文件损坏或没有音轨)")
        });
    }
    if let Some(dec) = open_decoder_raw(path) {
        if let Some(d) = std::panic::catch_unwind(AssertUnwindSafe(|| dec.total_duration()))
            .ok()
            .flatten()
        {
            let secs = d.as_secs_f64();
            if secs > 0.001 {
                return Ok(secs);
            }
        }
    }
    if let Some(secs) = ffmpeg_probe_duration(path) {
        return Ok(secs);
    }
    let Some(dec) = open_decoder_raw(path) else {
        return Err(crate::error::Error::audio_probe(
            path,
            "解码器无法打开此文件",
        ));
    };
    let sample_rate = dec.sample_rate() as f64;
    let channels = dec.channels() as f64;
    if sample_rate <= 0.0 || channels <= 0.0 {
        return Err(crate::error::Error::audio_probe(path, "采样率或声道无效"));
    }
    let n = match std::panic::catch_unwind(AssertUnwindSafe(|| dec.count())) {
        Ok(n) => n as f64,
        Err(_) => {
            return Err(crate::error::Error::audio_probe(
                path,
                "解码器在读取时长时崩溃, 已拦截",
            ));
        }
    };
    Ok(n / sample_rate / channels)
}

fn ffmpeg_probe_duration(path: &Path) -> Option<f64> {
    if let Some(secs) = ffmpeg_probe_decoded_duration(path) {
        return Some(secs);
    }
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

/// 解码第一路音轨到 null, 用 `-progress` 的 `out_time_us` 取微秒级时长.
/// banner 里的 `Duration: HH:MM:SS.xx` 只有百分之一秒, 多段导入会往后面累加.
fn ffmpeg_probe_decoded_duration(path: &Path) -> Option<f64> {
    let mut cmd = Command::new(crate::export::ffmpeg_path());
    cmd.args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-map",
            "0:a:0",
            "-f",
            "null",
            "-progress",
            "pipe:1",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    parse_ffmpeg_progress_seconds(&String::from_utf8_lossy(&output.stdout))
}

fn parse_ffmpeg_progress_seconds(progress: &str) -> Option<f64> {
    let mut last: Option<f64> = None;
    for line in progress.lines() {
        let Some(rest) = line.strip_prefix("out_time_us=") else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() || rest.eq_ignore_ascii_case("N/A") {
            continue;
        }
        let Ok(us) = rest.parse::<i64>() else {
            continue;
        };
        if us >= 0 {
            last = Some(us as f64 / 1_000_000.0);
        }
    }
    last.filter(|s| *s > 0.001)
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
    use super::{parse_ffmpeg_duration, parse_ffmpeg_progress_seconds};

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
    fn parse_ffmpeg_progress_uses_last_out_time_us() {
        let log = "out_time_us=1000000\nout_time_us=N/A\nout_time_us=754557007\nprogress=end\n";
        let d = parse_ffmpeg_progress_seconds(log).unwrap();
        assert!((d - 754.557007).abs() < 1e-9);
    }

    #[test]
    fn atempo_filter_chains_beyond_2x() {
        assert_eq!(super::atempo_filter(1.0), "anull");
        assert_eq!(super::atempo_filter(1.25), "atempo=1.25000");
        assert_eq!(super::atempo_filter(2.0), "atempo=2.00000");
        assert_eq!(super::atempo_filter(3.0), "atempo=2,atempo=1.50000");
    }

    #[test]
    fn wav_slice_seeks_by_sample_not_decode() {
        use rodio::Source;
        let dir = std::env::temp_dir().join(format!("sv_wav_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..8000 {
                w.write_sample(i as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let mut src = super::open_wav_slice(&path, 0.5, 0.25).expect("wav slice");
        assert_eq!(src.sample_rate(), 8000);
        assert_eq!(src.channels(), 1);
        let first = src.next().unwrap();
        assert_eq!(first, 4000);
        let n = 1 + src.count();
        assert_eq!(n, 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn waveform_peaks_streams_wav() {
        let dir = std::env::temp_dir().join("score_video_wave_peaks");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tone.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..8000 {
                let s = if i % 80 < 40 { 12_000i16 } else { 0 };
                w.write_sample(s).unwrap();
                w.write_sample(s).unwrap();
            }
            w.finalize().unwrap();
        }
        let (peaks, dur) = super::waveform_peaks(&path, 300.0, 64, 200_000).unwrap();
        assert!(peaks.len() >= 64);
        assert!(peaks.iter().any(|p| *p > 0.2));
        assert!((dur - 1.0).abs() < 0.05);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn waveform_peak_in_range_follows_column_width() {
        // 10 个等宽桶盖住 1 秒; 缩小时一列覆盖多桶取 max, 放大时插值.
        let peaks: Vec<f32> = (0..10).map(|i| i as f32 / 9.0).collect();
        let wide = super::waveform_peak_in_range(&peaks, 1.0, 0.0, 1.0);
        assert!((wide - 1.0).abs() < 1e-5);
        let mid = super::waveform_peak_in_range(&peaks, 1.0, 0.5, 0.6);
        let left = super::waveform_peak_in_range(&peaks, 1.0, 0.0, 0.1);
        assert!(mid > left);
        let zoomed = super::waveform_peak_in_range(&peaks, 1.0, 0.55, 0.56);
        assert!(zoomed > 0.4 && zoomed < 0.8);
    }

    #[test]
    fn skip_to_wav_data_ignores_list_and_unknown_size() {
        use std::io::Read;
        // RIFF + fmt + LIST + data(0xFFFFFFFF) + 2 stereo frames.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&u32::MAX.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // pcm
        wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"LIST");
        wav.extend_from_slice(&26u32.to_le_bytes());
        wav.extend_from_slice(b"INFOISFT");
        wav.extend_from_slice(&14u32.to_le_bytes());
        wav.extend_from_slice(b"Lavf61.7.100\0\0");
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&u32::MAX.to_le_bytes());
        wav.extend_from_slice(&1i16.to_le_bytes());
        wav.extend_from_slice(&2i16.to_le_bytes());
        wav.extend_from_slice(&3i16.to_le_bytes());
        wav.extend_from_slice(&4i16.to_le_bytes());
        let mut cur = std::io::Cursor::new(wav);
        assert!(super::skip_to_wav_data(&mut cur));
        let mut buf = [0u8; 2];
        cur.read_exact(&mut buf).unwrap();
        assert_eq!(i16::from_le_bytes(buf), 1);
    }

    #[test]
    fn wav_slice_rejects_non_pcm16() {
        assert!(super::open_wav_slice(std::path::Path::new("nope.mp3"), 0.0, 1.0).is_none());
    }

    #[test]
    fn atempo_source_reads_ffmpeg_wav_pipe() {
        use rodio::Source;
        // 优先用仓库里的裁剪 sidecar, 避免 PATH 上的完整 ffmpeg 把 `-f wav` 路径测假.
        let vendor = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor")
            .join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX));
        if vendor.is_file() {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let _ = std::fs::copy(&vendor, dir.join(vendor.file_name().unwrap()));
                }
            }
        }
        let ffmpeg_ok = std::process::Command::new(crate::export::ffmpeg_path())
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ffmpeg_ok {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("sv_atp_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..8000 {
                w.write_sample(((i % 64) * 200) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let mut src = super::open_atempo_source(&path, 0.0, 0.25, 1.0).expect("wav pipe");
        assert_eq!(src.sample_rate(), 44100);
        assert_eq!(src.channels(), 2);
        let n = src.by_ref().take(2000).count();
        assert!(n >= 1000, "got {n} samples from ffmpeg wav pipe");
        let mut fast = super::open_atempo_source(&path, 0.0, 0.25, 1.25).expect("atempo wav pipe");
        assert!(fast.by_ref().take(200).count() >= 50);

        // 必须真的合成 mp3 再走预览管道; 编不出来就失败, 不要 silently skip.
        let mp3 = dir.join("t.mp3");
        let full_ffmpeg = [
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/ffmpeg.full.bak"),
            std::path::PathBuf::from(r"C:\ProgramData\chocolatey\lib\ffmpeg\tools\ffmpeg\bin\ffmpeg.exe"),
        ];
        let encoder = full_ffmpeg.iter().find(|ff| ff.is_file()).expect("need full ffmpeg to encode mp3");
        let status = std::process::Command::new(encoder)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&path)
            .args(["-codec:a", "libmp3lame", "-q:a", "4"])
            .arg(&mp3)
            .status()
            .expect("spawn full ffmpeg");
        assert!(status.success(), "libmp3lame encode failed: {encoder:?}");
        assert!(mp3.is_file(), "mp3 not written");
        eprintln!("encoded mp3 {} bytes via {encoder:?}", mp3.metadata().unwrap().len());
        let mut src = super::open_atempo_source(&mp3, 0.0, 0.8, 1.0).expect("mp3 pipe");
        let n = src.by_ref().take(8000).count();
        eprintln!("slim ffmpeg decoded {n} pcm samples from synthesized mp3");
        assert!(n >= 2000, "slim ffmpeg must decode synthesized mp3, got {n} samples");
        let _ = std::fs::remove_dir_all(&dir);
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
