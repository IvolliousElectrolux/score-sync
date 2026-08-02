//! 预览音频播放引擎: 按 `AudioClip` 顺序拼接播放, 支持跳转到任意时刻.
//!
//! rodio 的 `Sink` 只能从头顺序播放已 append 的音源, 不支持中途寻址; 因此每次
//! 跳转 (播放/暂停/拖动播放头) 都重新创建 `Sink`, 定位到覆盖目标时刻的那条
//! `AudioClip`, 用 `skip_duration` 跳过其内部偏移, 再依次 append 该条剩余部分
//! 与后续所有片段.

use std::fs::File;
use std::io::BufReader;
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::model::AudioClip;

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
                if let Ok(file) = File::open(&clip.path) {
                    if let Ok(dec) = Decoder::new(BufReader::new(file)) {
                        // `clip.offset` 是该段在源文件里的起始时刻 (分割音频
                        // 产生的后半段 > 0), `local_offset` 是本次寻址点相对
                        // 这一段自己起点的偏移, 两者相加才是源文件里真正要
                        // 跳到的位置.
                        let src = dec
                            .skip_duration(Duration::from_secs_f64(clip.offset + local_offset));
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

/// 用 `symphonia` (随 rodio) 探测音频文件总时长; 导入音频时调用一次.
///
/// 多数容器格式能直接从元数据得到时长; 拿不到时退化为整段解码计数 (仅导入时
/// 跑一次, 可接受).
///
/// `.wav` 单独走 `hound` 直接读头部采样数计算时长: 一来避免大文件时兜底的
/// 整段解码计数非常慢, 二来 rodio/symphonia 对部分 WAV (如大文件/特定编码)
/// 探测 `total_duration` 会失败导致误判为"无法识别".
pub fn probe_duration(path: &std::path::Path) -> Option<f64> {
    let is_wav = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    if is_wav {
        if let Ok(reader) = hound::WavReader::open(path) {
            let sr = reader.spec().sample_rate as f64;
            let frames = reader.duration() as f64; // 每声道帧数, 除采样率即秒数
            if sr > 0.0 {
                return Some(frames / sr);
            }
        }
    }
    let file = File::open(path).ok()?;
    let dec = Decoder::new(BufReader::new(file)).ok()?;
    if let Some(d) = dec.total_duration() {
        return Some(d.as_secs_f64());
    }
    let sample_rate = dec.sample_rate() as f64;
    let channels = dec.channels() as f64;
    if sample_rate <= 0.0 || channels <= 0.0 {
        return None;
    }
    let n = dec.count() as f64;
    Some(n / sample_rate / channels)
}
