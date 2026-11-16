//! FLAC 预览寻址.
//!
//! 自带的 SEEKTABLE 间隔不超过 4 秒时, 播放直接把原文件交给解码器, 按表跳到
//! 目标采样附近的那一帧, 再丢掉帧内多出来的采样. 这样不用另扫文件, 也不用
//! 为每次跳转新开 ffmpeg.
//!
//! 没有表, 表是空的, 或两点隔得太开时, 导入后在后台把每一帧的文件偏移记下来,
//! 做成一张只活在内存里的 SEEKTABLE (不改用户的文件). 播放头还停在按下的位置,
//! 界面上给出剩余时间; 表好了再出声, 进度条和声音一起走.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, Once, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// 相邻两个 SEEKTABLE 点超过这么久, 就改扫每一帧. 4 秒以内的间隔, 解码器
/// 从点上再往前解到目标采样, 仍然是几毫秒的事.
const MAX_SEEK_GAP: Duration = Duration::from_secs(4);

const CACHE_MAGIC: &[u8; 4] = b"SSFI";
const CACHE_VERSION: u8 = 1;

#[derive(Clone)]
struct FramePoint {
    sample: u64,
    /// 这一帧在原文件里的绝对偏移.
    offset: u64,
    block: u16,
}

struct FlacInfo {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    total_samples: u64,
    first_frame: u64,
    streaminfo: Vec<u8>,
}

struct FlacIndex {
    info: FlacInfo,
    points: Vec<FramePoint>,
}

enum Kind {
    Queued,
    Running,
    Native,
    Ready(std::sync::Arc<FlacIndex>),
    Failed,
}

struct Job {
    len: u64,
    mtime_ns: u128,
    state: Mutex<Kind>,
    done: AtomicU64,
    total: AtomicU64,
    started: Mutex<Option<Instant>>,
}

struct World {
    jobs: Mutex<HashMap<PathBuf, std::sync::Arc<Job>>>,
    queue: Mutex<VecDeque<PathBuf>>,
    queue_cv: Condvar,
}

static WORLD: OnceLock<World> = OnceLock::new();
static WORKER: Once = Once::new();

fn world() -> &'static World {
    WORLD.get_or_init(|| World {
        jobs: Mutex::new(HashMap::new()),
        queue: Mutex::new(VecDeque::new()),
        queue_cv: Condvar::new(),
    })
}

fn start_worker() {
    WORKER.call_once(|| {
        let _ = world();
        std::thread::Builder::new()
            .name("flac-index".into())
            .spawn(worker_loop)
            .ok();
    });
}

fn worker_loop() {
    let world = world();
    loop {
        let path = {
            let mut queue = world.queue.lock().unwrap_or_else(|err| err.into_inner());
            loop {
                if let Some(path) = queue.pop_front() {
                    break path;
                }
                queue = world
                    .queue_cv
                    .wait(queue)
                    .unwrap_or_else(|err| err.into_inner());
            }
        };
        let Some(job) = job_for(&path) else {
            continue;
        };
        {
            let mut state = job.state.lock().unwrap_or_else(|err| err.into_inner());
            if !matches!(*state, Kind::Queued) {
                continue;
            }
            *state = Kind::Running;
        }
        *job.started.lock().unwrap_or_else(|err| err.into_inner()) = Some(Instant::now());
        let scanned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_index(&path, &job)));
        let mut state = job.state.lock().unwrap_or_else(|err| err.into_inner());
        *state = match scanned {
            Ok(Some(index)) => {
                let _ = write_cache(&path, &job, &index);
                Kind::Ready(std::sync::Arc::new(index))
            }
            _ => Kind::Failed,
        };
    }
}

fn job_for(path: &Path) -> Option<std::sync::Arc<Job>> {
    world()
        .jobs
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(path)
        .cloned()
}

/// 导入或打开工程时调用. 只读文件头; 需要扫帧的排进后台队列.
pub fn prime<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    for path in paths {
        let _ = seek_ready(path);
    }
}

/// 把即将要出声的文件挪到扫描队列前面.
pub fn prioritize<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    start_worker();
    let world = world();
    let mut queue = world.queue.lock().unwrap_or_else(|err| err.into_inner());
    for path in paths {
        if let Some(index) = queue.iter().position(|queued| queued.as_path() == path) {
            let item = queue.remove(index).unwrap_or_else(|| path.to_path_buf());
            queue.push_front(item);
        }
    }
    drop(queue);
    world.queue_cv.notify_all();
}

/// 1x 预览现在就能按采样点打开这个文件. 不是 FLAC, 或者表已经可用, 都算就绪.
/// 扫描失败也算就绪: 调用方退回 ffmpeg, 进度条仍然等出声之后再走.
pub fn seek_ready(path: &Path) -> bool {
    if !is_flac(path) {
        return true;
    }
    let Some((len, mtime_ns)) = file_identity(path) else {
        return true;
    };
    let world = world();
    let existing = world
        .jobs
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(path)
        .cloned();
    if let Some(job) = existing {
        if job.len == len && job.mtime_ns == mtime_ns {
            let state = job.state.lock().unwrap_or_else(|err| err.into_inner());
            return !matches!(*state, Kind::Queued | Kind::Running);
        }
    }
    let job = match classify(path, len, mtime_ns) {
        Some(job) => job,
        None => return true,
    };
    let ready = {
        let state = job.state.lock().unwrap_or_else(|err| err.into_inner());
        !matches!(*state, Kind::Queued | Kind::Running)
    };
    if !ready {
        enqueue(path);
    }
    ready
}

fn enqueue(path: &Path) {
    start_worker();
    let world = world();
    let mut queue = world.queue.lock().unwrap_or_else(|err| err.into_inner());
    if !queue.iter().any(|queued| queued.as_path() == path) {
        queue.push_back(path.to_path_buf());
    }
    drop(queue);
    world.queue_cv.notify_all();
}

fn classify(path: &Path, len: u64, mtime_ns: u128) -> Option<std::sync::Arc<Job>> {
    let kind = if let Some(index) = read_cache(path, len, mtime_ns) {
        Kind::Ready(std::sync::Arc::new(index))
    } else {
        match read_plan(path) {
            Ok(Plan::Native) => Kind::Native,
            Ok(Plan::Scan) => Kind::Queued,
            Err(_) => Kind::Failed,
        }
    };
    let total = len;
    let job = std::sync::Arc::new(Job {
        len,
        mtime_ns,
        state: Mutex::new(kind),
        done: AtomicU64::new(0),
        total: AtomicU64::new(total),
        started: Mutex::new(None),
    });
    world()
        .jobs
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .insert(path.to_path_buf(), std::sync::Arc::clone(&job));
    Some(job)
}

/// 这些文件里还有没建好的寻址表时, 给出一行给播放条.
pub fn wait_message<'a>(paths: impl IntoIterator<Item = &'a Path>) -> String {
    let mut eta: Option<f64> = None;
    for path in paths {
        if !is_flac(path) || seek_ready(path) {
            continue;
        }
        let Some(job) = job_for(path) else {
            continue;
        };
        let done = job.done.load(Ordering::Relaxed);
        let total = job.total.load(Ordering::Relaxed);
        let started = job.started.lock().unwrap_or_else(|err| err.into_inner());
        let Some(started) = *started else {
            continue;
        };
        if let Some(seconds) = eta_seconds(started.elapsed().as_secs_f64(), done, total) {
            eta = Some(eta.map(|best| best.max(seconds)).unwrap_or(seconds));
        }
    }
    format_wait(eta)
}

fn eta_seconds(elapsed: f64, done: u64, total: u64) -> Option<f64> {
    if done == 0 || total <= done || !(elapsed > 0.0) {
        return None;
    }
    Some(elapsed * (total - done) as f64 / done as f64)
}

fn format_wait(eta: Option<f64>) -> String {
    let zh = ui_kit::language() == ui_kit::Language::Zh;
    let Some(seconds) = eta else {
        return if zh {
            "正在建立 FLAC 寻址表, 请稍候...".into()
        } else {
            "Building the FLAC seek index...".into()
        };
    };
    let whole = seconds.round().max(1.0) as u64;
    if zh {
        if whole < 60 {
            format!("正在建立 FLAC 寻址表, 请等待约 {whole} 秒")
        } else {
            let mins = whole / 60;
            let secs = whole % 60;
            if secs == 0 {
                format!("正在建立 FLAC 寻址表, 请等待约 {mins} 分")
            } else {
                format!("正在建立 FLAC 寻址表, 请等待约 {mins} 分 {secs} 秒")
            }
        }
    } else if whole < 60 {
        format!("Building the FLAC seek index, about {whole} s left")
    } else {
        let mins = whole / 60;
        let secs = whole % 60;
        format!("Building the FLAC seek index, about {mins} min {secs} s left")
    }
}

enum Plan {
    Native,
    Scan,
}

fn read_plan(path: &Path) -> Result<Plan, String> {
    let mut file = File::open(path).map_err(|err| err.to_string())?;
    let info = read_info(&mut file)?;
    if let Some(points) = read_seek_table(&mut file, &info)? {
        if table_is_usable(&points, info.total_samples, info.sample_rate) {
            return Ok(Plan::Native);
        }
    }
    Ok(Plan::Scan)
}

fn table_is_usable(points: &[FramePoint], total_samples: u64, sample_rate: u32) -> bool {
    if points.len() < 2 || sample_rate == 0 {
        return false;
    }
    let limit = MAX_SEEK_GAP.as_secs().saturating_mul(u64::from(sample_rate));
    if points[0].sample > limit {
        return false;
    }
    let mut prev = points[0].sample;
    for point in points.iter().skip(1) {
        if point.sample < prev || point.sample - prev > limit {
            return false;
        }
        prev = point.sample;
    }
    if total_samples > 0 && total_samples.saturating_sub(prev) > limit.saturating_add(u64::from(sample_rate))
    {
        return false;
    }
    true
}

/// 从 `start_sample` (每声道) 起读交错的 i16. 给预览音源和测试共用.
pub(crate) struct FlacDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    skip_frames: u64,
}

impl FlacDecoder {
    pub(crate) fn open(path: &Path, start_sample: u64) -> Option<Self> {
        let media = open_media(path)?;
        let mss = MediaSourceStream::new(media, Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .ok()?;
        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|track| track.codec_params.sample_rate.is_some())?
            .clone();
        let sample_rate = track.codec_params.sample_rate?;
        if sample_rate == 0 {
            return None;
        }
        let channels = track.codec_params.channels.map(|n| n.count()).unwrap_or(1).max(1) as u16;
        let seeked = format
            .seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts: start_sample,
                    track_id: track.id,
                },
            )
            .ok()?;
        if seeked.actual_ts > seeked.required_ts {
            return None;
        }
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .ok()?;
        Some(Self {
            format,
            decoder,
            track_id: track.id,
            sample_rate,
            channels,
            skip_frames: seeked.required_ts - seeked.actual_ts,
        })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.channels
    }

    /// 下一帧里, 跳过目标采样之前的部分之后剩下的交错采样.
    pub(crate) fn pull(&mut self) -> Option<Vec<i16>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphError::IoError(_)) | Err(SymphError::ResetRequired) => return None,
                Err(_) => return None,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = self.decoder.decode(&packet).ok()?;
            let spec = *decoded.spec();
            let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            let mut samples = buf.samples().to_vec();
            let channels = spec.channels.count().max(1);
            if self.skip_frames > 0 && channels > 0 {
                let frames = samples.len() / channels;
                let drop_frames = (self.skip_frames as usize).min(frames);
                let drop_samples = drop_frames * channels;
                if drop_samples > 0 {
                    samples.drain(..drop_samples);
                }
                self.skip_frames -= drop_frames as u64;
            }
            if samples.is_empty() {
                continue;
            }
            return Some(samples);
        }
    }
}

fn open_media(path: &Path) -> Option<Box<dyn MediaSource>> {
    match read_plan(path).ok()? {
        Plan::Native => Some(Box::new(PlainFile(File::open(path).ok()?))),
        Plan::Scan => {
            let index = ready_index(path)?;
            Some(Box::new(VirtualFlac::open(path, &index).ok()?))
        }
    }
}

fn ready_index(path: &Path) -> Option<std::sync::Arc<FlacIndex>> {
    let job = job_for(path)?;
    let state = job.state.lock().unwrap_or_else(|err| err.into_inner());
    match &*state {
        Kind::Ready(index) => Some(std::sync::Arc::clone(index)),
        _ => None,
    }
}

struct PlainFile(File);

impl Read for PlainFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for PlainFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl MediaSource for PlainFile {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.0.metadata().ok().map(|meta| meta.len())
    }
}

struct VirtualFlac {
    header: Vec<u8>,
    file: File,
    origin: u64,
    len: u64,
    pos: u64,
}

impl VirtualFlac {
    fn open(path: &Path, index: &FlacIndex) -> Result<Self, String> {
        let header = virtual_header(index)?;
        let file = File::open(path).map_err(|err| err.to_string())?;
        let file_len = file.metadata().map_err(|err| err.to_string())?.len();
        if index.info.first_frame > file_len {
            return Err("flac frame start past end".into());
        }
        let len = header.len() as u64 + file_len - index.info.first_frame;
        Ok(Self {
            header,
            file,
            origin: index.info.first_frame,
            len,
            pos: 0,
        })
    }
}

impl Read for VirtualFlac {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let mut written = 0;
        while written < buf.len() && self.pos < self.len {
            if self.pos < self.header.len() as u64 {
                let start = self.pos as usize;
                let n = (self.header.len() - start).min(buf.len() - written);
                buf[written..written + n].copy_from_slice(&self.header[start..start + n]);
                self.pos += n as u64;
                written += n;
            } else {
                let file_pos = self.origin + (self.pos - self.header.len() as u64);
                self.file.seek(SeekFrom::Start(file_pos))?;
                let want = (buf.len() - written).min((self.len - self.pos) as usize);
                let n = self.file.read(&mut buf[written..written + want])?;
                if n == 0 {
                    break;
                }
                self.pos += n as u64;
                written += n;
            }
        }
        Ok(written)
    }
}

impl Seek for VirtualFlac {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(value) => value as i64,
            SeekFrom::End(delta) => self.len as i64 + delta,
            SeekFrom::Current(delta) => self.pos as i64 + delta,
        };
        if next < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

impl MediaSource for VirtualFlac {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

fn virtual_header(index: &FlacIndex) -> Result<Vec<u8>, String> {
    if index.points.is_empty() || index.info.streaminfo.is_empty() {
        return Err("empty flac index".into());
    }
    let table_bytes = index
        .points
        .len()
        .checked_mul(18)
        .filter(|n| *n <= 0x00FF_FFFF)
        .ok_or_else(|| "seek table too large".to_string())?;
    let info_len = index.info.streaminfo.len();
    if info_len > 0x00FF_FFFF {
        return Err("streaminfo too large".into());
    }
    let mut out = Vec::with_capacity(4 + 4 + info_len + 4 + table_bytes);
    out.extend_from_slice(b"fLaC");
    out.push(0x00);
    out.extend_from_slice(&(info_len as u32).to_be_bytes()[1..]);
    out.extend_from_slice(&index.info.streaminfo);
    out.push(0x80 | 0x03);
    out.extend_from_slice(&(table_bytes as u32).to_be_bytes()[1..]);
    for point in &index.points {
        let relative = point.offset.saturating_sub(index.info.first_frame);
        out.extend_from_slice(&point.sample.to_be_bytes());
        out.extend_from_slice(&relative.to_be_bytes());
        out.extend_from_slice(&point.block.to_be_bytes());
    }
    Ok(out)
}

fn build_index(path: &Path, job: &Job) -> Option<FlacIndex> {
    let mut file = File::open(path).ok()?;
    let info = read_info(&mut file).ok()?;
    let file_len = file.metadata().ok()?.len();
    job.total.store(file_len.max(1), Ordering::Relaxed);
    let points = scan_frames(&mut file, &info, file_len, &job.done)?;
    job.done.store(file_len, Ordering::Relaxed);
    if points.is_empty() {
        return None;
    }
    Some(FlacIndex { info, points })
}

fn scan_frames(
    file: &mut File,
    info: &FlacInfo,
    file_len: u64,
    done: &AtomicU64,
) -> Option<Vec<FramePoint>> {
    if info.first_frame > file_len {
        return None;
    }
    file.seek(SeekFrom::Start(info.first_frame)).ok()?;
    let mut cursor = FrameCursor {
        reader: file,
        buf: Vec::new(),
        base: info.first_frame,
        eof: false,
    };
    let mut points = Vec::new();
    let mut at = info.first_frame;
    loop {
        if !cursor.fill_to(at.saturating_add(32), done) && cursor.eof {
            break;
        }
        let rel = offset_in(&cursor, at)?;
        let header = match parse_frame_header(&cursor.buf, rel) {
            HeaderParse::Ok(header) => header,
            _ => break,
        };
        let expect = header.sample.saturating_add(u64::from(header.block));
        let mut search = cursor.base + header.header_end;
        let limit = at.saturating_add(1024 * 1024);
        let mut found = None;
        while search < limit {
            cursor.fill_to(search.saturating_add(32), done);
            let buf_end = cursor.base + cursor.buf.len() as u64;
            if search >= buf_end {
                break;
            }
            let start_rel = (search - cursor.base) as usize;
            let Some(delta) = find_sync(&cursor.buf[start_rel..]) else {
                if cursor.eof {
                    break;
                }
                search = buf_end.saturating_sub(1).max(search.saturating_add(1));
                continue;
            };
            let cand = search + delta as u64;
            cursor.fill_to(cand.saturating_add(32), done);
            let cand_rel = match offset_in(&cursor, cand) {
                Some(value) => value,
                None => break,
            };
            if let HeaderParse::Ok(next) = parse_frame_header(&cursor.buf, cand_rel) {
                if next.sample == expect {
                    let frame_end = cand_rel;
                    if frame_crc_ok(&cursor.buf[rel..frame_end]) {
                        found = Some(cand);
                        break;
                    }
                }
            }
            search = cand.saturating_add(1);
        }
        if let Some(next) = found {
            points.push(FramePoint {
                sample: header.sample,
                offset: at,
                block: header.block,
            });
            at = next;
            cursor.compact(at);
            continue;
        }
        if cursor.eof {
            let rel = offset_in(&cursor, at)?;
            if frame_crc_ok(&cursor.buf[rel..]) {
                points.push(FramePoint {
                    sample: header.sample,
                    offset: at,
                    block: header.block,
                });
            }
        }
        break;
    }
    Some(points)
}

struct FrameCursor<'a> {
    reader: &'a mut File,
    buf: Vec<u8>,
    base: u64,
    eof: bool,
}

impl FrameCursor<'_> {
    fn fill_to(&mut self, abs_end: u64, done: &AtomicU64) -> bool {
        while (self.base + self.buf.len() as u64) < abs_end && !self.eof {
            let mut tmp = [0u8; 64 * 1024];
            match self.reader.read(&mut tmp) {
                Ok(0) => self.eof = true,
                Ok(n) => {
                    self.buf.extend_from_slice(&tmp[..n]);
                    done.store(self.base + self.buf.len() as u64, Ordering::Relaxed);
                }
                Err(_) => return false,
            }
        }
        self.base + self.buf.len() as u64 >= abs_end || self.eof
    }

    fn compact(&mut self, keep_from: u64) {
        if keep_from <= self.base {
            return;
        }
        let drop_n = (keep_from - self.base) as usize;
        if drop_n > self.buf.len() {
            return;
        }
        self.buf.drain(..drop_n);
        self.base = keep_from;
    }
}

fn offset_in(cursor: &FrameCursor<'_>, abs: u64) -> Option<usize> {
    if abs < cursor.base {
        return None;
    }
    let rel = (abs - cursor.base) as usize;
    (rel <= cursor.buf.len()).then_some(rel)
}

fn frame_crc_ok(frame: &[u8]) -> bool {
    if frame.len() < 6 {
        return false;
    }
    let body = &frame[..frame.len() - 2];
    let footer = u16::from_be_bytes([frame[frame.len() - 2], frame[frame.len() - 1]]);
    crc16(body) == footer
}

fn find_sync(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == 0xFF && (buf[i + 1] & 0xFE) == 0xF8 {
            return Some(i);
        }
        i += 1;
    }
    None
}

struct ParsedFrame {
    sample: u64,
    block: u16,
    header_end: u64,
}

enum HeaderParse {
    Ok(ParsedFrame),
    NeedMore,
    Invalid,
}

fn parse_frame_header(buf: &[u8], pos: usize) -> HeaderParse {
    if pos >= buf.len() {
        return HeaderParse::NeedMore;
    }
    if pos + 5 > buf.len() {
        return HeaderParse::NeedMore;
    }
    if buf[pos] != 0xFF || (buf[pos + 1] & 0xFE) != 0xF8 {
        return HeaderParse::Invalid;
    }
    let variable = buf[pos + 1] & 1 == 1;
    let block_code = (buf[pos + 2] >> 4) & 0x0F;
    let rate_code = buf[pos + 2] & 0x0F;
    let channel_code = (buf[pos + 3] >> 4) & 0x0F;
    let bps_code = (buf[pos + 3] >> 1) & 0x07;
    if block_code == 0 || rate_code == 0x0F || channel_code > 10 || bps_code == 0x07 {
        return HeaderParse::Invalid;
    }
    let (number, width) = match read_utf8(&buf[pos + 4..]) {
        Utf8::Ok(value, width) => (value, width),
        Utf8::NeedMore => return HeaderParse::NeedMore,
        Utf8::Invalid => return HeaderParse::Invalid,
    };
    let mut i = pos + 4 + width;
    let block = match block_size(block_code, buf, &mut i) {
        SizeField::Ok(value) => value,
        SizeField::NeedMore => return HeaderParse::NeedMore,
        SizeField::Invalid => return HeaderParse::Invalid,
    };
    let extra = match rate_code {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    if i + extra >= buf.len() {
        return HeaderParse::NeedMore;
    }
    i += extra;
    if i >= buf.len() {
        return HeaderParse::NeedMore;
    }
    if crc8(&buf[pos..i]) != buf[i] {
        return HeaderParse::Invalid;
    }
    let sample = if variable {
        number
    } else {
        number.saturating_mul(u64::from(block))
    };
    HeaderParse::Ok(ParsedFrame {
        sample,
        block,
        header_end: (i + 1) as u64,
    })
}

enum SizeField {
    Ok(u16),
    NeedMore,
    Invalid,
}

fn block_size(code: u8, buf: &[u8], i: &mut usize) -> SizeField {
    let fixed = match code {
        1 => 192,
        2 => 576,
        3 => 1152,
        4 => 2304,
        5 => 4608,
        8 => 256,
        9 => 512,
        10 => 1024,
        11 => 2048,
        12 => 4096,
        13 => 8192,
        14 => 16384,
        15 => 32768,
        6 | 7 => 0,
        _ => return SizeField::Invalid,
    };
    if fixed != 0 {
        return SizeField::Ok(fixed);
    }
    if code == 6 {
        if *i >= buf.len() {
            return SizeField::NeedMore;
        }
        let value = u16::from(buf[*i]) + 1;
        *i += 1;
        return SizeField::Ok(value);
    }
    if *i + 1 >= buf.len() {
        return SizeField::NeedMore;
    }
    let value = u16::from_be_bytes([buf[*i], buf[*i + 1]]).saturating_add(1);
    *i += 2;
    if value == 0 {
        SizeField::Invalid
    } else {
        SizeField::Ok(value)
    }
}

enum Utf8 {
    Ok(u64, usize),
    NeedMore,
    Invalid,
}

fn read_utf8(buf: &[u8]) -> Utf8 {
    if buf.is_empty() {
        return Utf8::NeedMore;
    }
    let first = buf[0];
    let width = if first < 0x80 {
        1
    } else if first & 0xE0 == 0xC0 {
        2
    } else if first & 0xF0 == 0xE0 {
        3
    } else if first & 0xF8 == 0xF0 {
        4
    } else if first & 0xFC == 0xF8 {
        5
    } else if first & 0xFE == 0xFC {
        6
    } else if first == 0xFE {
        7
    } else {
        return Utf8::Invalid;
    };
    if buf.len() < width {
        return Utf8::NeedMore;
    }
    let mut value = u64::from(first & (0xFFu8 >> (width + 1))) ;
    if width == 7 {
        value = 0;
    }
    for byte in &buf[1..width] {
        if byte & 0xC0 != 0x80 {
            return Utf8::Invalid;
        }
        value = (value << 6) | u64::from(byte & 0x3F);
    }
    Utf8::Ok(value, width)
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = ((crc << 1) ^ 0x8005) & 0xFFFF;
            } else {
                crc = (crc << 1) & 0xFFFF;
            }
        }
    }
    crc
}

fn read_info(file: &mut File) -> Result<FlacInfo, String> {
    skip_id3(file)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|err| err.to_string())?;
    if &magic != b"fLaC" {
        return Err("not flac".into());
    }
    let mut streaminfo = None;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut total_samples = 0u64;
    let mut saw_info = false;
    loop {
        let mut hdr = [0u8; 4];
        file.read_exact(&mut hdr).map_err(|err| err.to_string())?;
        let last = hdr[0] & 0x80 != 0;
        let kind = hdr[0] & 0x7F;
        let len = u32::from_be_bytes([0, hdr[1], hdr[2], hdr[3]]) as u64;
        if kind == 0 {
            if len < 34 || len > 1024 * 1024 {
                return Err("bad streaminfo".into());
            }
            let mut payload = vec![0u8; len as usize];
            file.read_exact(&mut payload).map_err(|err| err.to_string())?;
            let word = u64::from_be_bytes(payload[10..18].try_into().unwrap_or([0; 8]));
            sample_rate = ((word >> 44) & 0xF_FFFF) as u32;
            channels = (((word >> 41) & 0x7) + 1) as u16;
            bits = (((word >> 36) & 0x1F) + 1) as u16;
            total_samples = word & 0xF_FFFF_FFFF;
            streaminfo = Some(payload);
            saw_info = true;
        } else {
            file.seek(SeekFrom::Current(len as i64))
                .map_err(|err| err.to_string())?;
        }
        if last {
            break;
        }
    }
    if !saw_info || sample_rate == 0 {
        return Err("flac has no streaminfo".into());
    }
    let first_frame = file.stream_position().map_err(|err| err.to_string())?;
    Ok(FlacInfo {
        sample_rate,
        channels,
        bits_per_sample: bits,
        total_samples,
        first_frame,
        streaminfo: streaminfo.unwrap_or_default(),
    })
}

fn read_seek_table(file: &mut File, info: &FlacInfo) -> Result<Option<Vec<FramePoint>>, String> {
    file.seek(SeekFrom::Start(0)).map_err(|err| err.to_string())?;
    skip_id3(file)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|err| err.to_string())?;
    if &magic != b"fLaC" {
        return Ok(None);
    }
    let mut points = None;
    loop {
        let mut hdr = [0u8; 4];
        file.read_exact(&mut hdr).map_err(|err| err.to_string())?;
        let last = hdr[0] & 0x80 != 0;
        let kind = hdr[0] & 0x7F;
        let len = u32::from_be_bytes([0, hdr[1], hdr[2], hdr[3]]) as usize;
        if kind == 3 {
            if len > 8 * 1024 * 1024 {
                return Err("seek table too large".into());
            }
            let mut raw = vec![0u8; len];
            file.read_exact(&mut raw).map_err(|err| err.to_string())?;
            let mut parsed = Vec::new();
            let mut i = 0;
            while i + 18 <= raw.len() {
                let sample = u64::from_be_bytes(raw[i..i + 8].try_into().unwrap_or([0; 8]));
                let offset = u64::from_be_bytes(raw[i + 8..i + 16].try_into().unwrap_or([0; 8]));
                let block = u16::from_be_bytes(raw[i + 16..i + 18].try_into().unwrap_or([0; 2]));
                i += 18;
                if sample == u64::MAX {
                    continue;
                }
                parsed.push(FramePoint {
                    sample,
                    offset: info.first_frame.saturating_add(offset),
                    block,
                });
            }
            if points.is_none() {
                points = Some(parsed);
            }
        } else {
            file.seek(SeekFrom::Current(len as i64))
                .map_err(|err| err.to_string())?;
        }
        if last {
            break;
        }
    }
    Ok(points)
}

fn skip_id3(file: &mut File) -> Result<(), String> {
    let mut hdr = [0u8; 10];
    file.read_exact(&mut hdr).map_err(|err| err.to_string())?;
    if &hdr[..3] != b"ID3" {
        file.seek(SeekFrom::Start(0)).map_err(|err| err.to_string())?;
        return Ok(());
    }
    let size = synchsafe(&hdr[6..10]).ok_or_else(|| "bad id3 size".to_string())?;
    let footer = if hdr[5] & 0x10 != 0 { 10 } else { 0 };
    file.seek(SeekFrom::Start(10 + u64::from(size) + footer))
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn synchsafe(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 || bytes.iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    Some(
        (u32::from(bytes[0]) << 21)
            | (u32::from(bytes[1]) << 14)
            | (u32::from(bytes[2]) << 7)
            | u32::from(bytes[3]),
    )
}

fn is_flac(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("flac"))
        .unwrap_or(false)
}

fn file_identity(path: &Path) -> Option<(u64, u128)> {
    let meta = path.metadata().ok()?;
    let modified = meta.modified().unwrap_or(UNIX_EPOCH);
    let mtime_ns = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Some((meta.len(), mtime_ns))
}

fn cache_path(path: &Path, len: u64, mtime_ns: u128) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    path.hash(&mut hasher);
    len.hash(&mut hasher);
    mtime_ns.hash(&mut hasher);
    std::env::temp_dir()
        .join("score_sync_audio")
        .join(format!("{:016x}.flxidx", hasher.finish()))
}

fn read_cache(path: &Path, len: u64, mtime_ns: u128) -> Option<FlacIndex> {
    let bytes = std::fs::read(cache_path(path, len, mtime_ns)).ok()?;
    if bytes.len() < 4 + 1 + 8 + 16 + 8 + 8 + 4 || &bytes[..4] != CACHE_MAGIC || bytes[4] != CACHE_VERSION
    {
        return None;
    }
    let mut i = 5;
    let take8 = |i: &mut usize| -> Option<u64> {
        let value = u64::from_le_bytes(bytes.get(*i..*i + 8)?.try_into().ok()?);
        *i += 8;
        Some(value)
    };
    let stored_len = take8(&mut i)?;
    let stored_mtime = take8(&mut i)?;
    if stored_len != len || stored_mtime != u64::try_from(mtime_ns).unwrap_or(u64::MAX) {
        return None;
    }
    let sample_rate = u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?);
    i += 4;
    let channels = u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?);
    i += 2;
    let bits = u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?);
    i += 2;
    let total_samples = take8(&mut i)?;
    let first_frame = take8(&mut i)?;
    let info_len = u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?) as usize;
    i += 4;
    let streaminfo = bytes.get(i..i + info_len)?.to_vec();
    i += info_len;
    let n = u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?) as usize;
    i += 4;
    let mut points = Vec::with_capacity(n);
    for _ in 0..n {
        let sample = take8(&mut i)?;
        let offset = take8(&mut i)?;
        let block = u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?);
        i += 2;
        points.push(FramePoint {
            sample,
            offset,
            block,
        });
    }
    Some(FlacIndex {
        info: FlacInfo {
            sample_rate,
            channels,
            bits_per_sample: bits,
            total_samples,
            first_frame,
            streaminfo,
        },
        points,
    })
}

fn write_cache(path: &Path, job: &Job, index: &FlacIndex) -> std::io::Result<()> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.push(CACHE_VERSION);
    bytes.extend_from_slice(&job.len.to_le_bytes());
    let mtime = u64::try_from(job.mtime_ns).unwrap_or(u64::MAX);
    bytes.extend_from_slice(&mtime.to_le_bytes());
    bytes.extend_from_slice(&index.info.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&index.info.channels.to_le_bytes());
    bytes.extend_from_slice(&index.info.bits_per_sample.to_le_bytes());
    bytes.extend_from_slice(&index.info.total_samples.to_le_bytes());
    bytes.extend_from_slice(&index.info.first_frame.to_le_bytes());
    bytes.extend_from_slice(&(index.info.streaminfo.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&index.info.streaminfo);
    bytes.extend_from_slice(&(index.points.len() as u32).to_le_bytes());
    for point in &index.points {
        bytes.extend_from_slice(&point.sample.to_le_bytes());
        bytes.extend_from_slice(&point.offset.to_le_bytes());
        bytes.extend_from_slice(&point.block.to_le_bytes());
    }
    let out = cache_path(path, job.len, job.mtime_ns);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = out.with_extension("flxidx.part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &out)
}

pub(crate) fn probe_rate(path: &Path) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    read_info(&mut file).ok().map(|info| info.sample_rate)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::*;

    fn usable(samples: &[u64], total: u64, rate: u32) -> bool {
        let points: Vec<_> = samples
            .iter()
            .map(|sample| FramePoint {
                sample: *sample,
                offset: 0,
                block: 0,
            })
            .collect();
        table_is_usable(&points, total, rate)
    }

    #[test]
    fn native_table_within_four_seconds_does_not_need_a_scan() {
        let rate = 44100;
        let points = [0, rate as u64 * 2, rate as u64 * 4];
        assert!(usable(&points, rate as u64 * 5, rate));
    }

    #[test]
    fn wide_or_missing_tables_need_a_byte_index() {
        let rate = 44100;
        assert!(!usable(&[], 1000, rate));
        assert!(!usable(&[0], 1000, rate));
        assert!(!usable(&[0, rate as u64 * 10], rate as u64 * 12, rate));
    }

    #[test]
    fn wait_copy_includes_an_eta_once_bytes_are_moving() {
        let text = format_wait(Some(3.2));
        assert!(text.contains('3') || text.contains("3"));
        let pending = format_wait(None);
        assert!(pending.contains("FLAC"));
    }

    #[test]
    fn eta_scales_with_the_unread_tail() {
        let eta = eta_seconds(2.0, 25, 100).unwrap();
        assert!((eta - 6.0).abs() < 1e-9);
        assert!(eta_seconds(1.0, 0, 10).is_none());
    }

    fn write_utf8(out: &mut Vec<u8>, value: u64) {
        if value < 0x80 {
            out.push(value as u8);
        } else if value < 0x800 {
            out.push((0xC0 | (value >> 6)) as u8);
            out.push((0x80 | (value & 0x3F)) as u8);
        } else if value < 0x1_0000 {
            out.push((0xE0 | (value >> 12)) as u8);
            out.push((0x80 | ((value >> 6) & 0x3F)) as u8);
            out.push((0x80 | (value & 0x3F)) as u8);
        } else {
            out.push((0xF0 | (value >> 18)) as u8);
            out.push((0x80 | ((value >> 12) & 0x3F)) as u8);
            out.push((0x80 | ((value >> 6) & 0x3F)) as u8);
            out.push((0x80 | (value & 0x3F)) as u8);
        }
    }

    fn encode_frame(frame: u64, sample: i16) -> Vec<u8> {
        let mut header = vec![0xFF, 0xF8, 0x19, 0x08];
        write_utf8(&mut header, frame);
        header.push(crc8(&header));
        let mut frame_bytes = header;
        frame_bytes.push(0x00);
        frame_bytes.extend_from_slice(&sample.to_be_bytes());
        let sum = crc16(&frame_bytes);
        frame_bytes.extend_from_slice(&sum.to_be_bytes());
        frame_bytes
    }

    fn streaminfo(total_samples: u64) -> Vec<u8> {
        let mut info = vec![0u8; 34];
        info[0..2].copy_from_slice(&192u16.to_be_bytes());
        info[2..4].copy_from_slice(&192u16.to_be_bytes());
        let word = (44100u64 << 44) | (15u64 << 36) | (total_samples & 0xF_FFFF_FFFF);
        info[10..18].copy_from_slice(&word.to_be_bytes());
        info
    }

    fn encode_flac(frame_count: u64, with_table: bool) -> Vec<u8> {
        let frames: Vec<_> = (0..frame_count).map(|i| encode_frame(i, 1000 + i as i16)).collect();
        let info = streaminfo(frame_count * 192);
        let mut body = Vec::new();
        let mut points = Vec::new();
        for (i, frame) in frames.iter().enumerate() {
            points.push((i as u64 * 192, body.len() as u64, 192u16));
            body.extend_from_slice(frame);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"fLaC");
        let last_info = if with_table { 0x00 } else { 0x80 };
        out.push(last_info);
        out.extend_from_slice(&34u32.to_be_bytes()[1..]);
        out.extend_from_slice(&info);
        if with_table {
            let table_len = (points.len() * 18) as u32;
            out.push(0x80 | 0x03);
            out.extend_from_slice(&table_len.to_be_bytes()[1..]);
            for (sample, offset, block) in points {
                out.extend_from_slice(&sample.to_be_bytes());
                out.extend_from_slice(&offset.to_be_bytes());
                out.extend_from_slice(&block.to_be_bytes());
            }
        }
        out.extend_from_slice(&body);
        out
    }

    fn temp_flac(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join("score_sync_flac_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    pub(crate) fn fixture_native_flac() -> PathBuf {
        temp_flac(
            &format!("engine-native-{}.flac", std::process::id()),
            &encode_flac(8, true),
        )
    }

    fn pull_n(path: &Path, start: u64, n: usize) -> Vec<i16> {
        let mut dec = FlacDecoder::open(path, start).expect("open flac");
        let mut out = Vec::new();
        while out.len() < n {
            let Some(chunk) = dec.pull() else {
                break;
            };
            out.extend(chunk);
        }
        out.truncate(n);
        out
    }

    #[test]
    fn seek_table_file_decodes_the_sample_inside_the_frame() {
        let path = temp_flac("native.flac", &encode_flac(130, true));
        assert!(seek_ready(&path), "a dense table is ready without a scan");
        let got = pull_n(&path, 192 * 2 + 3, 4);
        assert_eq!(got, vec![1002, 1002, 1002, 1002]);
        let head = pull_n(&path, 0, 2);
        assert_eq!(head, vec![1000, 1000]);
        let late = pull_n(&path, 192 * 128 + 1, 1);
        assert_eq!(late, vec![1128]);
    }

    #[test]
    fn file_without_a_table_matches_after_the_byte_index_is_built() {
        let name = format!(
            "scan-{}-{}.flac",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let path = temp_flac(&name, &encode_flac(6, false));
        assert!(
            !seek_ready(&path),
            "a file with no seek table is not ready until the scan finishes"
        );
        let start = Instant::now();
        while !seek_ready(&path) {
            assert!(start.elapsed() < Duration::from_secs(5), "index build stalled");
            std::thread::sleep(Duration::from_millis(10));
        }
        let got = pull_n(&path, 192 + 10, 3);
        assert_eq!(got, vec![1001, 1001, 1001]);
        let (len, mtime) = file_identity(&path).unwrap();
        let cached = read_cache(&path, len, mtime).expect("index written beside the temp dir");
        assert_eq!(cached.points.len(), 6);
        assert_eq!(cached.points[1].sample, 192);
        let again = pull_n(&path, 192 + 10, 3);
        assert_eq!(again, got);
    }
}
