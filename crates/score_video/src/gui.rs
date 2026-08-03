//! GPUI 图形界面: 视频轨道编辑 (预览窗 + 视频/淡入淡出/音频三轨 + 素材池 + 导出).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, canvas, div, point, prelude::*, px, rgb, rgba, size, App, Application, Bounds,
    Context, Corners, CursorStyle, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render,
    RenderImage, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
use image::Frame;
use rodio::Source;
use smallvec::smallvec;
use uuid::Uuid;

use crate::audio::AudioEngine;
use crate::export::{Container, ExportMsg, ExportOptions};
use crate::model::{AudioClip, FadeKind, MaterialItem, Timeline};

actions!(
    score_video,
    [
        PlayPause,
        SeekBack,
        SeekForward,
        SeekBackBig,
        SeekForwardBig,
        InsertNext,
        MarkFadeIn,
        MarkFadeOut,
        DeleteSelected,
        Undo,
        Redo,
    ]
);

const VIDEO_HISTORY_LIMIT: usize = 64;

/// 嵌入宿主 (score_sync) 启动时调用一次, 注册「视频」标签页下的快捷键.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", PlayPause, Some("ScoreVideo")),
        KeyBinding::new("left", SeekBack, Some("ScoreVideo")),
        KeyBinding::new("right", SeekForward, Some("ScoreVideo")),
        KeyBinding::new("shift-left", SeekBackBig, Some("ScoreVideo")),
        KeyBinding::new("shift-right", SeekForwardBig, Some("ScoreVideo")),
        KeyBinding::new("n", InsertNext, Some("ScoreVideo")),
        KeyBinding::new("i", MarkFadeIn, Some("ScoreVideo")),
        KeyBinding::new("o", MarkFadeOut, Some("ScoreVideo")),
        KeyBinding::new("delete", DeleteSelected, Some("ScoreVideo")),
        KeyBinding::new("backspace", DeleteSelected, Some("ScoreVideo")),
        KeyBinding::new("ctrl-z", Undo, Some("ScoreVideo")),
        KeyBinding::new("ctrl-y", Redo, Some("ScoreVideo")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("ScoreVideo")),
    ]);
}

const PREVIEW_H: f32 = 300.0;
const BAR_H: f32 = 10.0;
const TRACK_H: f32 = 40.0;
/// 音频轨道比视频/淡入淡出轨道稍高一些, 好放下波形预览.
const AUDIO_TRACK_H: f32 = 64.0;
/// 底部横向缩放/滚动条高度.
const TRACK_BAR_H: f32 = 18.0;
/// 音频排序拖拽: 超过此像素位移才进入"已拖起" (与分块标签页一致).
const AUDIO_REORDER_SLOP: f32 = 5.0;
const EDGE_ZONE: f32 = 8.0;
/// 波形基础采样密度 (每秒峰值点数). 基础数据按时长而非固定点数采样, 绘制时
/// 再按当前片段的屏幕宽度 (随缩放变化) 重新降采样/插值, 分辨率因此始终跟着
/// 缩放走, 而不是一批固定点数被硬拉伸/压缩成同一个"采样率"的样子.
const WAVEFORM_BUCKETS_PER_SEC: f64 = 300.0;
const WAVEFORM_MIN_BUCKETS: usize = 64;
const WAVEFORM_MAX_BUCKETS: usize = 200_000;
/// 三条轨道紧贴在一起的总高度 (视频轨 + 淡入淡出轨 + 稍高一些的音频轨).
const TRACKS_TOTAL_H: f32 = TRACK_H * 2.0 + AUDIO_TRACK_H;
/// 拖动底部缩放条圆点缩放时, 可视时间窗口的最小时长 (秒).
const MIN_VISIBLE_SECS: f64 = 0.2;

#[derive(Clone)]
enum VideoDrag {
    Seek,
    TrimLeft {
        id: Uuid,
    },
    TrimRight {
        id: Uuid,
    },
    Body {
        id: Uuid,
        last_t: f64,
    },
    FadeSelect {
        anchor: f64,
    },
    /// 拖动淡入淡出左/右边界 (与视频轨道片段的裁剪逻辑一致).
    FadeTrimLeft {
        id: Uuid,
    },
    FadeTrimRight {
        id: Uuid,
    },
    /// 整体拖动淡入淡出区间 (保持时长).
    FadeBody {
        id: Uuid,
        last_t: f64,
    },
    /// 拖动音频片段排序 (手感对齐分块标签页: 过阈值才 armed, 幽灵跟随,
    /// 原位半透明, 落点左右边指示线, 松开才真正换序).
    AudioBody {
        id: Uuid,
        from: usize,
        to: usize,
        line_at: Option<usize>,
        line_after: bool,
        start_x: f32,
        start_y: f32,
        origin_x: f32,
        origin_y: f32,
        x: f32,
        y: f32,
        label: SharedString,
        armed: bool,
    },
    /// 拖动素材池自定义竖直滚动条滑块.
    PoolScroll {
        grab: f32,
    },
    /// 素材池条目被拖拽中 (可能跨越素材池/轨道两个面板, 由宿主转发鼠标事件).
    PoolDrop {
        group_id: String,
        start_x: f32,
        start_y: f32,
        last_x: f32,
        last_y: f32,
    },
    /// 拖动底部横向缩放条滑块本体 = 平移 (对应 `track_scroll`).
    TrackBarPan { grab: f32 },
    /// 拖动底部横向缩放条滑块左端圆点 = 改变可视窗口左边界从而改变缩放,
    /// 锚定右边界时刻不动 (PR 时间轴缩放条手感).
    TrackBarZoomLeft { anchor_end_t: f64 },
    /// 拖动底部横向缩放条滑块右端圆点 = 改变可视窗口右边界从而改变缩放,
    /// 锚定左边界时刻不动.
    TrackBarZoomRight { anchor_start_t: f64 },
    /// 调整"待定淡入淡出预框选区"(拖选出来但尚未按 I/O 提交) 的左/右边界,
    /// 而不必重新拖选一次.
    FadeSelectTrimLeft,
    FadeSelectTrimRight,
}

/// 解码整个音频文件, 按响度绝对值取每个桶内的峰值 (0..1 归一化), 供音频
/// 轨道绘制波形预览用. 在后台线程调用, 较大文件也不会卡 UI.
///
/// 桶数按时长 (而非固定常数) 换算, 保证基础数据本身有足够密度; 实际绘制时
/// 再按当前片段的屏幕宽度 (随缩放实时变化) 重新降采样/插值一次, 分辨率因此
/// 会跟着缩放丝滑变化, 而不是固定一批点被硬拉伸/压缩.
fn compute_waveform_peaks(path: &std::path::Path) -> Option<Vec<f32>> {
    let file = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::new(std::io::BufReader::new(file)).ok()?;
    let channels = (dec.channels() as usize).max(1);
    let sample_rate = dec.sample_rate().max(1) as f64;
    let samples: Vec<i16> = dec.collect();
    let frames = samples.len() / channels;
    if frames == 0 {
        return None;
    }
    let duration_secs = frames as f64 / sample_rate;
    let buckets = ((duration_secs * WAVEFORM_BUCKETS_PER_SEC).ceil() as usize)
        .clamp(WAVEFORM_MIN_BUCKETS, WAVEFORM_MAX_BUCKETS);
    let mut peaks = vec![0f32; buckets];
    let per_bucket = (frames as f64 / buckets as f64).max(1.0);
    for (b, peak) in peaks.iter_mut().enumerate() {
        let start = ((b as f64) * per_bucket) as usize;
        let end = (((b + 1) as f64) * per_bucket).ceil() as usize;
        let end = end.clamp(start + 1, frames);
        let mut m: i32 = 0;
        for f in start..end {
            for c in 0..channels {
                if let Some(&s) = samples.get(f * channels + c) {
                    m = m.max((s as i32).abs());
                }
            }
        }
        *peak = (m as f32 / i16::MAX as f32).clamp(0.0, 1.0);
    }
    Some(peaks)
}

/// 时间轴边界吸附阈值 (像素): 视频/淡入淡出/音频边界彼此靠近时对齐.
const SNAP_PX: f32 = 8.0;

/// 吸附时排除自身边界, 避免拖拽边缘粘在自己身上.
#[derive(Clone, Copy)]
enum SnapExclude {
    None,
    Fade(Uuid),
    Video(Uuid),
}

pub struct ScoreVideoApp {
    focus_handle: FocusHandle,
    pool: Vec<MaterialItem>,
    render_cache: std::collections::HashMap<String, Arc<RenderImage>>,
    timeline: Timeline,
    audio: AudioEngine,
    aspect_w: u32,
    aspect_h: u32,
    tracks_bounds: Bounds<Pixels>,
    preview_bounds: Bounds<Pixels>,
    /// 音频片段屏幕 bounds (按当前顺序下标), 供排序拖拽判定落点/幽灵原点.
    audio_clip_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 底部横向缩放/滚动条自身的屏幕 bounds.
    track_bar_bounds: Bounds<Pixels>,
    px_per_sec: f32,
    /// 用户是否已通过 Ctrl+滚轮/底部缩放条手动缩放过轨道 (之后不再自动适应宽度).
    track_user_zoomed: bool,
    /// 轨道横向滚动偏移 (秒).
    track_scroll: f64,
    pool_scroll: ScrollHandle,
    /// 素材池中当前展开显示预览图的条目 (点击而非拖动时切换).
    expanded_pool: Option<String>,
    /// 音频波形峰值缓存: key = 源文件路径, 每个源按时长采样出足够密度的
    /// 基础峰值点, 绘制时再按当前片段的屏幕宽度 (随缩放变化) 重新降采样/
    /// 插值一次, 分辨率因此会跟着缩放丝滑改变.
    waveform_cache: std::collections::HashMap<PathBuf, Arc<Vec<f32>>>,
    /// 正在后台解码计算波形中的路径, 避免同一文件重复起线程.
    waveform_pending: std::collections::HashSet<PathBuf>,
    drag: Option<VideoDrag>,
    status: SharedString,
    /// 「分割音频」按钮按下后进入待命: 下一次鼠标按下时若落在音频轨道内就
    /// 从该处切开对应片段, 否则 (点在别处) 直接取消, 不作任何改动.
    split_audio_armed: bool,
    export_open: bool,
    export_container: Container,
    /// 帧率: 可直接点击输入框改数字 (与 CRF 那种只能加减的 stepper 不同),
    /// 复用 `apply_bg` 里已经有的文本框组件, 无需重复注册快捷键 (宿主启动
    /// 时已经调用过 `apply_bg::text_input::bind_keys`).
    export_fps_input: Entity<apply_bg::text_input::TextInput>,
    export_crf: u32,
    export_out_path: Option<PathBuf>,
    exporting: bool,
    export_progress: SharedString,
    /// ffmpeg 的原始输出 (不再弹终端窗口, 直接在导出弹窗里滚动显示最近几
    /// 行), 超过上限就丢掉最旧的.
    export_log: Vec<SharedString>,
    /// 播放代数: 每次开始播放自增, 供 ticker 判断自身是否已过期而自行退出,
    /// 避免播放/暂停快速切换时残留多个 ticker 任务.
    play_gen: u64,
    undo_stack: Vec<crate::model::TimelineSnapshot>,
    redo_stack: Vec<crate::model::TimelineSnapshot>,
    /// 当前拖拽是否已为本次变更压过撤销栈.
    drag_undo_pushed: bool,
}

impl ScoreVideoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let app = Self {
            focus_handle: cx.focus_handle(),
            pool: Vec::new(),
            render_cache: std::collections::HashMap::new(),
            timeline: Timeline::new(),
            audio: AudioEngine::new(),
            aspect_w: 16,
            aspect_h: 9,
            tracks_bounds: Bounds::default(),
            preview_bounds: Bounds::default(),
            audio_clip_bounds: HashMap::new(),
            track_bar_bounds: Bounds::default(),
            px_per_sec: 20.0,
            track_user_zoomed: false,
            track_scroll: 0.0,
            pool_scroll: ScrollHandle::new(),
            expanded_pool: None,
            waveform_cache: std::collections::HashMap::new(),
            waveform_pending: std::collections::HashSet::new(),
            drag: None,
            status: "就绪. N 插入下一张组合, 空格播放/暂停, I/O 标记淡入淡出.".into(),
            split_audio_armed: false,
            export_open: false,
            export_container: Container::Mp4,
            export_fps_input: cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "30", "帧率")),
            export_crf: 18,
            export_out_path: None,
            exporting: false,
            export_progress: SharedString::default(),
            export_log: Vec::new(),
            play_gen: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            drag_undo_pushed: false,
        };
        app
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// 项目底色宽高比 (用于预览 letterbox 与导出默认分辨率).
    pub fn set_aspect(&mut self, w: u32, h: u32) {
        self.aspect_w = w.max(1);
        self.aspect_h = h.max(1);
    }

    /// 供宿主保存工程时读取当前时间轴 (纯数据快照, 不含选中态), 一并写入工程文件.
    pub fn timeline_snapshot(&self) -> crate::model::TimelineSnapshot {
        self.timeline.snapshot()
    }

    /// 供宿主载入工程后写回时间轴 (重新生成各条目 id, 并把音频重接到播放引擎).
    pub fn load_timeline_snapshot(
        &mut self,
        snap: crate::model::TimelineSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.timeline.load_snapshot(snap);
        self.audio.set_clips(self.timeline.audio_clips.clone());
        self.track_scroll = 0.0;
        self.track_user_zoomed = false;
        self.drag = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.drag_undo_pushed = false;
        cx.notify();
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.timeline.snapshot());
        if self.undo_stack.len() > VIDEO_HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn ensure_drag_undo(&mut self) {
        if !self.drag_undo_pushed {
            self.push_undo();
            self.drag_undo_pushed = true;
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            self.status = "没有可撤回的操作.".into();
            cx.notify();
            return;
        };
        self.redo_stack.push(self.timeline.snapshot());
        self.timeline.load_snapshot(prev);
        self.audio.set_clips(self.timeline.audio_clips.clone());
        self.drag = None;
        self.status = "已撤回.".into();
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            self.status = "没有可重做的操作.".into();
            cx.notify();
            return;
        };
        self.undo_stack.push(self.timeline.snapshot());
        if self.undo_stack.len() > VIDEO_HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.timeline.load_snapshot(next);
        self.audio.set_clips(self.timeline.audio_clips.clone());
        self.drag = None;
        self.status = "已重做.".into();
        cx.notify();
    }

    pub fn set_pool(&mut self, pool: Vec<MaterialItem>, cx: &mut Context<Self>) {
        // 素材内容 (例如工程底色叠加状态) 可能已变化但 group_id 不变, 因此
        // 整体清空缓存而不是按 id 保留, 避免残留旧贴图.
        self.render_cache.clear();
        self.pool = pool;
        if let Some(gid) = &self.expanded_pool {
            if !self.pool.iter().any(|m| &m.group_id == gid) {
                self.expanded_pool = None;
            }
        }
        cx.notify();
    }

    /// 仅在播放期间才启动的进度 ticker (每次开始播放时新建一个).
    /// 之前的实现从 `new()` 起就无条件永久轮询, 会在应用生命周期内持续
    /// 尝试借用 entity 上下文; 一旦此时用户触发了 `rfd` 原生文件对话框
    /// (其模态消息循环会重入宿主窗口消息处理), 就可能与该 ticker 的
    /// `this.update` 产生重入借用冲突, 触发 `RefCell already borrowed` 崩溃.
    /// 改为只在真正播放时才存在这个任务, 播放停止后自动退出, 从根本上
    /// 避免了文件对话框场景下的重入.
    fn start_ticker(&mut self, cx: &mut Context<Self>) {
        self.play_gen = self.play_gen.wrapping_add(1);
        let gen = self.play_gen;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(33))
                .await;
            let result = this.update(cx, |view, cx| {
                if view.play_gen != gen || !view.audio.is_playing() {
                    return false;
                }
                let t = view.audio.current_time();
                let end = view.timeline.timeline_end();
                if t >= end {
                    view.audio.pause();
                    view.timeline.playhead = end;
                } else {
                    view.timeline.playhead = t;
                }
                cx.notify();
                true
            });
            match result {
                Ok(true) => continue,
                _ => break,
            }
        })
        .detach();
    }

    fn image_for(&mut self, group_id: &str) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.render_cache.get(group_id) {
            return Some(img.clone());
        }
        let item = self.pool.iter().find(|m| m.group_id == group_id)?;
        // 谱面组合拼合 (+ 可能叠加的工程底色补边) 后经常是很高的整图 (几千
        // 甚至上万像素), 若原样整张丢给 GPU 当贴图, 可能超出显卡/后端的纹理
        // 尺寸上限, 曾在切到「视频」面板/展开素材预览时触发底层渲染崩溃
        // (STATUS_STACK_BUFFER_OVERRUN, 无 Rust panic 输出, 是原生层面的问题).
        // 这里只按屏幕预览需要限幅缩小一份副本用于显示; 导出仍读取素材池里
        // 未缩放的原图 (见 `export::dump_pool_images`), 不受影响.
        const MAX_PREVIEW_DIM: u32 = 2048;
        let (w, h) = item.image.dimensions();
        let mut rgba = if w > MAX_PREVIEW_DIM || h > MAX_PREVIEW_DIM {
            let scale = (MAX_PREVIEW_DIM as f32 / w.max(h) as f32).min(1.0);
            let nw = ((w as f32 * scale).round() as u32).max(1);
            let nh = ((h as f32 * scale).round() as u32).max(1);
            image::imageops::resize(&*item.image, nw, nh, image::imageops::FilterType::Triangle)
        } else {
            (*item.image).clone()
        };
        // GPUI 的 `RenderImage` 内部按 BGRA 排布读取像素, 而素材池里的图是标准
        // RGBA (来自 `image` 库); 不交换 R/B 通道的话画面颜色会整体错位 (例如
        // 暖色调底色显示成冷色调), 这里与 `mask_tool::gui::load_rgb` 保持一致.
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let render = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
        self.render_cache
            .insert(group_id.to_string(), render.clone());
        Some(render)
    }

    fn x_to_time(&self, x: f32) -> f64 {
        let rel = x - f32::from(self.tracks_bounds.origin.x);
        (rel.max(0.0) / self.px_per_sec.max(0.01)) as f64 + self.track_scroll
    }

    /// 时间点吸附: 就近对齐到视频/淡入淡出/音频边界、播放头、0 与时间轴末尾.
    /// `exclude` 排除当前正在拖拽的片段自身, 避免粘在自己的边上.
    fn snap_time(&self, t: f64, exclude: SnapExclude) -> f64 {
        let threshold = (SNAP_PX / self.px_per_sec.max(0.01)) as f64;
        let mut best = t;
        let mut best_d = threshold;
        let mut consider = |c: f64| {
            let d = (c - t).abs();
            if d < best_d {
                best_d = d;
                best = c;
            }
        };
        consider(0.0);
        consider(self.timeline.timeline_end());
        consider(self.timeline.playhead);
        for c in &self.timeline.video_clips {
            if matches!(exclude, SnapExclude::Video(id) if id == c.id) {
                continue;
            }
            consider(c.start);
            consider(c.end);
        }
        for f in &self.timeline.fades {
            if matches!(exclude, SnapExclude::Fade(id) if id == f.id) {
                continue;
            }
            consider(f.start);
            consider(f.end);
        }
        let mut audio_t = 0.0;
        consider(audio_t);
        for a in &self.timeline.audio_clips {
            audio_t += a.duration;
            consider(audio_t);
        }
        best
    }

    /// 整体拖动时按左右边界就近吸附, 返回修正后的时间增量.
    fn snap_body_delta(
        &self,
        start: f64,
        end: f64,
        delta: f64,
        exclude: SnapExclude,
    ) -> (f64, f64) {
        let new_start = start + delta;
        let new_end = end + delta;
        let ss = self.snap_time(new_start, exclude);
        let se = self.snap_time(new_end, exclude);
        let adj = if (ss - new_start).abs() <= (se - new_end).abs() {
            ss - new_start
        } else {
            se - new_end
        };
        (delta + adj, adj)
    }

    pub fn play_pause(&mut self, cx: &mut Context<Self>) {
        if self.audio.is_playing() {
            self.audio.pause();
            // 使当前 ticker 在下一次 tick 时自行退出.
            self.play_gen = self.play_gen.wrapping_add(1);
            self.timeline.playhead = self.audio.current_time();
        } else {
            self.audio.set_clips(self.timeline.audio_clips.clone());
            self.audio.play_from(self.timeline.playhead);
            self.start_ticker(cx);
        }
        cx.notify();
    }

    pub fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        let t = t.clamp(0.0, self.timeline.timeline_end());
        self.timeline.playhead = t;
        self.audio.seek(t);
        cx.notify();
    }

    pub fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        let t = self.timeline.playhead + delta;
        self.seek(t, cx);
    }

    /// 与 `x_to_time` 逻辑一致 (同一套 `px_per_sec`/`track_scroll`), 只是坐标
    /// 原点换成预览窗自己的 bounds, 确保拖动顶部进度条寻址的落点跟下方轨道
    /// 播放头竖线严格对应.
    fn seek_from_preview_x(&mut self, x: f32, cx: &mut Context<Self>) {
        let rel = x - f32::from(self.preview_bounds.origin.x);
        let t = (rel.max(0.0) / self.px_per_sec.max(0.01)) as f64 + self.track_scroll;
        self.seek(t, cx);
    }

    pub fn insert_next(&mut self, cx: &mut Context<Self>) {
        self.push_undo();
        match self.timeline.insert_next(&self.pool) {
            Ok(()) => self.status = "已插入下一张组合".into(),
            Err(e) => {
                self.undo_stack.pop();
                self.status = e.into();
            }
        }
        cx.notify();
    }

    pub fn mark_fade_in(&mut self, cx: &mut Context<Self>) {
        self.push_undo();
        self.timeline.mark_fade(FadeKind::In, self.timeline.playhead);
        cx.notify();
    }

    pub fn mark_fade_out(&mut self, cx: &mut Context<Self>) {
        self.push_undo();
        self.timeline
            .mark_fade(FadeKind::Out, self.timeline.playhead);
        cx.notify();
    }

    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        self.push_undo();
        let before = self.timeline.snapshot();
        self.timeline.delete_selected();
        let after = self.timeline.snapshot();
        // 无选中可删时撤回刚压的快照
        if before.video_clips.len() == after.video_clips.len()
            && before.fades.len() == after.fades.len()
            && before.audio_clips.len() == after.audio_clips.len()
        {
            self.undo_stack.pop();
        } else {
            self.audio.set_clips(self.timeline.audio_clips.clone());
        }
        cx.notify();
    }

    pub fn import_audio(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("音频", &["wav", "mp3", "flac", "ogg", "m4a", "aac"])
            .pick_files()
        else {
            return;
        };
        let mut added = 0usize;
        for p in paths {
            let dur = crate::audio::probe_duration(&p).unwrap_or(0.0);
            if dur <= 0.001 {
                self.status = format!("无法识别音频时长, 已跳过: {}", p.display()).into();
                continue;
            }
            if added == 0 {
                self.push_undo();
            }
            let label = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("audio")
                .to_string();
            self.timeline.audio_clips.push(AudioClip {
                id: Uuid::new_v4(),
                path: p,
                label: label.into(),
                duration: dur,
                offset: 0.0,
            });
            added += 1;
        }
        if added > 0 {
            self.timeline.fit_after_audio_change();
            self.audio.set_clips(self.timeline.audio_clips.clone());
        }
        cx.notify();
    }

    /// 「分割音频」按钮: 再次点击可取消待命; 否则进入待命, 等下一次鼠标
    /// 按下时在 `Render` 里加的全屏透明遮罩上判定落点 (见那边的说明).
    fn toggle_split_audio_armed(&mut self, cx: &mut Context<Self>) {
        self.split_audio_armed = !self.split_audio_armed;
        self.status = if self.split_audio_armed {
            "分割音频: 在音频轨道上点击要切开的位置 (点其他地方取消)".into()
        } else {
            "已取消分割音频".into()
        };
        cx.notify();
    }

    /// 待命状态下处理一次点击: 落在音频轨道内就从该时刻切开对应片段,
    /// 否则视为取消. 注意宿主走 `left_panel` 而不是 `Render`, 所以必须在
    /// 音频片段/音频轨道自身的 `on_mouse_down` 里直接调用, 不能依赖全屏遮罩.
    fn handle_split_audio_click(&mut self, x: f32, cx: &mut Context<Self>) {
        self.split_audio_armed = false;
        let b = self.tracks_bounds;
        let left = f32::from(b.origin.x);
        let right = left + f32::from(b.size.width);
        // 不严格卡 y: 调用方已经保证点在音频轨道/片段上; 这里只校验 x 在
        // 轨道水平范围内, 避免缩放滚动后边界抖动导致误判取消.
        let in_track_x = x >= left - 2.0 && x <= right + 2.0;
        if !in_track_x {
            self.status = "已取消分割音频".into();
            cx.notify();
            return;
        }
        let t = self.x_to_time(x);
        self.push_undo();
        if self.timeline.split_audio_at(t) {
            self.audio.set_clips(self.timeline.audio_clips.clone());
            self.status = format!("已在 {} 处分割音频", fmt_time(t)).into();
        } else {
            self.undo_stack.pop();
            self.status = "该处没有可分割的音频片段 (太靠近边界或未落在片段上)".into();
        }
        cx.notify();
    }

    /// 待命中点到非音频区域: 取消分割, 不开始别的拖拽.
    fn cancel_split_audio_if_armed(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.split_audio_armed {
            return false;
        }
        self.split_audio_armed = false;
        self.status = "已取消分割音频".into();
        cx.notify();
        true
    }

    fn begin_clip_drag(&mut self, id: Uuid, mouse_x: f32, cx: &mut Context<Self>) {
        if self.cancel_split_audio_if_armed(cx) {
            return;
        }
        self.timeline.selected_clip = Some(id);
        self.timeline.selected_fade = None;
        self.timeline.selected_audio = None;
        self.drag_undo_pushed = false;
        if let Some(c) = self.timeline.video_clips.iter().find(|c| c.id == id) {
            let origin_x = f32::from(self.tracks_bounds.origin.x) - (self.track_scroll as f32) * self.px_per_sec;
            let start_x = origin_x + (c.start as f32) * self.px_per_sec;
            let end_x = origin_x + (c.end as f32) * self.px_per_sec;
            if (mouse_x - start_x).abs() <= EDGE_ZONE {
                self.drag = Some(VideoDrag::TrimLeft { id });
            } else if (mouse_x - end_x).abs() <= EDGE_ZONE {
                self.drag = Some(VideoDrag::TrimRight { id });
            } else {
                self.drag = Some(VideoDrag::Body {
                    id,
                    last_t: self.x_to_time(mouse_x),
                });
            }
        }
        cx.notify();
    }

    /// 淡入淡出条目上按下: 边缘裁剪或整体拖动 (与 `begin_clip_drag` 逻辑一致).
    fn begin_fade_drag(&mut self, id: Uuid, mouse_x: f32, cx: &mut Context<Self>) {
        if self.cancel_split_audio_if_armed(cx) {
            return;
        }
        self.timeline.selected_fade = Some(id);
        self.timeline.selected_clip = None;
        self.timeline.selected_audio = None;
        self.drag_undo_pushed = false;
        if let Some(f) = self.timeline.fades.iter().find(|f| f.id == id) {
            let origin_x = f32::from(self.tracks_bounds.origin.x) - (self.track_scroll as f32) * self.px_per_sec;
            let start_x = origin_x + (f.start as f32) * self.px_per_sec;
            let end_x = origin_x + (f.end as f32) * self.px_per_sec;
            if (mouse_x - start_x).abs() <= EDGE_ZONE {
                self.drag = Some(VideoDrag::FadeTrimLeft { id });
            } else if (mouse_x - end_x).abs() <= EDGE_ZONE {
                self.drag = Some(VideoDrag::FadeTrimRight { id });
            } else {
                self.drag = Some(VideoDrag::FadeBody {
                    id,
                    last_t: self.x_to_time(mouse_x),
                });
            }
        }
        cx.notify();
    }

    /// 音频条目上按下: 开始排序拖拽 (未过阈值前只是选中, 不换序).
    fn begin_audio_drag(&mut self, id: Uuid, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.split_audio_armed {
            return;
        }
        let Some(from) = self.timeline.audio_clips.iter().position(|c| c.id == id) else {
            return;
        };
        self.timeline.selected_audio = Some(id);
        self.timeline.selected_clip = None;
        self.timeline.selected_fade = None;
        self.drag_undo_pushed = false;
        let label = self.timeline.audio_clips[from].label.clone();
        let (origin_x, origin_y) = self
            .audio_clip_bounds
            .get(&from)
            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
            .unwrap_or((x, y));
        self.drag = Some(VideoDrag::AudioBody {
            id,
            from,
            to: from,
            line_at: None,
            line_after: false,
            start_x: x,
            start_y: y,
            origin_x,
            origin_y,
            x,
            y,
            label,
            armed: false,
        });
        cx.notify();
    }

    fn audio_reorder_slop_exceeded(dx: f32, dy: f32) -> bool {
        dx * dx + dy * dy >= AUDIO_REORDER_SLOP * AUDIO_REORDER_SLOP
    }

    /// 将「落在 anchor 之前/之后」换算成 remove 后再 insert 的下标.
    fn reorder_to_index(from: usize, anchor: usize, after: bool) -> usize {
        if after {
            if from <= anchor {
                anchor
            } else {
                anchor + 1
            }
        } else if from < anchor {
            anchor - 1
        } else {
            anchor
        }
    }

    /// 水平音轨: 原位无反应; 左半→该项左边, 右半→该项右边.
    /// 返回 (to, line_at, line_after).
    fn resolve_audio_drop(&self, from: usize, x: f32) -> (usize, Option<usize>, bool) {
        let n = self.timeline.audio_clips.len();
        if n == 0 {
            return (from, None, false);
        }
        for i in 0..n {
            let Some(b) = self.audio_clip_bounds.get(&i) else {
                continue;
            };
            let left = f32::from(b.origin.x);
            let right = left + f32::from(b.size.width);
            if x < left || x > right {
                continue;
            }
            if i == from {
                return (from, None, false);
            }
            let mid = (left + right) * 0.5;
            let after = x >= mid;
            let to = Self::reorder_to_index(from, i, after);
            return (to, Some(i), after);
        }
        (from, None, false)
    }

    pub fn has_active_drag(&self) -> bool {
        self.drag.is_some()
    }

    /// 由宿主在窗口外 / 跨面板时转发: 处理当前所有拖拽种类.
    pub fn root_mouse_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match &mut self.drag {
            Some(VideoDrag::PoolDrop {
                last_x, last_y, ..
            }) => {
                *last_x = x;
                *last_y = y;
                cx.notify();
            }
            Some(VideoDrag::PoolScroll { grab }) => {
                let grab = *grab;
                self.apply_pool_scroll_drag(y, grab, cx);
            }
            Some(_) => {
                // 鼠标已离开左面板 (或整个窗口) 时, 仍继续更新轨道内拖拽.
                if !self.point_in_left_panel(x, y) {
                    self.apply_left_drag_move(x, y, cx);
                }
            }
            None => {}
        }
    }

    pub fn root_mouse_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match self.drag.clone() {
            Some(VideoDrag::PoolDrop {
                group_id,
                start_x,
                start_y,
                ..
            }) => {
                self.drag = None;
                let moved = ((x - start_x).powi(2) + (y - start_y).powi(2)).sqrt();
                let b = self.tracks_bounds;
                let within = x >= f32::from(b.origin.x)
                    && x <= f32::from(b.origin.x) + f32::from(b.size.width)
                    && y >= f32::from(b.origin.y)
                    && y <= f32::from(b.origin.y) + f32::from(b.size.height);
                if within {
                    let t = self.x_to_time(x);
                    self.push_undo();
                    self.timeline.insert_at(t, group_id);
                    self.status = "已从素材池拖入片段".into();
                } else if moved < 4.0 {
                    self.expanded_pool = if self.expanded_pool.as_deref() == Some(group_id.as_str())
                    {
                        None
                    } else {
                        Some(group_id)
                    };
                }
                cx.notify();
            }
            Some(VideoDrag::PoolScroll { .. }) => {
                self.drag = None;
                cx.notify();
            }
            Some(_) => {
                // 窗口外或右栏松开: 结束左面板发起的拖拽.
                self.end_left_drag(x, cx);
            }
            None => {}
        }
    }

    fn point_in_left_panel(&self, x: f32, y: f32) -> bool {
        // tracks_bounds 覆盖三轨区域; 预览区也算左栏. 用 tracks + 一个宽松包络:
        // 若尚未 layout, 视为不在左栏以便根节点接管.
        let b = self.tracks_bounds;
        let w = f32::from(b.size.width);
        let h = f32::from(b.size.height);
        if w < 1.0 || h < 1.0 {
            return false;
        }
        // 左栏大致: 从窗口左边到侧栏分割线. tracks 的右缘即左栏右缘近似.
        let left = 0.0;
        let right = f32::from(b.origin.x) + w + 8.0;
        let top = 0.0;
        let bottom = f32::from(b.origin.y) + h + TRACK_BAR_H + 80.0;
        x >= left && x <= right && y >= top && y <= bottom
    }

    fn apply_left_drag_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match &self.drag {
            None
            | Some(VideoDrag::PoolDrop { .. })
            | Some(VideoDrag::PoolScroll { .. }) => return,
            _ => {}
        }
        match self.drag.clone() {
            Some(VideoDrag::Seek) => self.seek_from_preview_x(x, cx),
            Some(VideoDrag::TrimLeft { id }) => {
                self.ensure_drag_undo();
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::Video(id));
                self.timeline.trim_left(id, t);
                cx.notify();
            }
            Some(VideoDrag::TrimRight { id }) => {
                self.ensure_drag_undo();
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::Video(id));
                self.timeline.trim_right(id, t);
                cx.notify();
            }
            Some(VideoDrag::Body { id, last_t }) => {
                self.ensure_drag_undo();
                let t = self.x_to_time(x);
                let delta = t - last_t;
                let (final_delta, adj) = if let Some(c) =
                    self.timeline.video_clips.iter().find(|c| c.id == id)
                {
                    self.snap_body_delta(c.start, c.end, delta, SnapExclude::Video(id))
                } else {
                    (delta, 0.0)
                };
                self.timeline.drag_body(id, final_delta);
                self.drag = Some(VideoDrag::Body {
                    id,
                    last_t: t + adj,
                });
                cx.notify();
            }
            Some(VideoDrag::FadeSelect { anchor }) => {
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::None);
                self.timeline.fade_selection = Some((anchor, t));
                cx.notify();
            }
            Some(VideoDrag::FadeTrimLeft { id }) => {
                self.ensure_drag_undo();
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::Fade(id));
                self.timeline.trim_fade_left(id, t);
                cx.notify();
            }
            Some(VideoDrag::FadeTrimRight { id }) => {
                self.ensure_drag_undo();
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::Fade(id));
                self.timeline.trim_fade_right(id, t);
                cx.notify();
            }
            Some(VideoDrag::FadeBody { id, last_t }) => {
                self.ensure_drag_undo();
                let t = self.x_to_time(x);
                let delta = t - last_t;
                let (final_delta, adj) =
                    if let Some(f) = self.timeline.fades.iter().find(|f| f.id == id) {
                        self.snap_body_delta(f.start, f.end, delta, SnapExclude::Fade(id))
                    } else {
                        (delta, 0.0)
                    };
                self.timeline.drag_fade_body(id, final_delta);
                self.drag = Some(VideoDrag::FadeBody {
                    id,
                    last_t: t + adj,
                });
                cx.notify();
            }
            Some(VideoDrag::AudioBody {
                id,
                from,
                start_x,
                start_y,
                origin_x,
                origin_y,
                label,
                mut armed,
                ..
            }) => {
                if !armed && Self::audio_reorder_slop_exceeded(x - start_x, y - start_y) {
                    armed = true;
                }
                let (to, line_at, line_after) = if armed {
                    self.resolve_audio_drop(from, x)
                } else {
                    (from, None, false)
                };
                self.drag = Some(VideoDrag::AudioBody {
                    id,
                    from,
                    to,
                    line_at,
                    line_after,
                    start_x,
                    start_y,
                    origin_x,
                    origin_y,
                    x,
                    y,
                    label,
                    armed,
                });
                cx.notify();
            }
            Some(VideoDrag::TrackBarPan { grab }) => {
                self.apply_track_bar_pan(x, grab, cx);
            }
            Some(VideoDrag::TrackBarZoomLeft { anchor_end_t }) => {
                self.apply_track_bar_zoom_left(x, anchor_end_t, cx);
            }
            Some(VideoDrag::TrackBarZoomRight { anchor_start_t }) => {
                self.apply_track_bar_zoom_right(x, anchor_start_t, cx);
            }
            Some(VideoDrag::FadeSelectTrimLeft) => {
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::None);
                if let Some((_, b)) = self.timeline.fade_selection {
                    self.timeline.fade_selection = Some((t, b));
                }
                cx.notify();
            }
            Some(VideoDrag::FadeSelectTrimRight) => {
                let raw = self.x_to_time(x);
                let t = self.snap_time(raw, SnapExclude::None);
                if let Some((a, _)) = self.timeline.fade_selection {
                    self.timeline.fade_selection = Some((a, t));
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn end_left_drag(&mut self, x: f32, cx: &mut Context<Self>) {
        match &self.drag {
            None
            | Some(VideoDrag::PoolDrop { .. })
            | Some(VideoDrag::PoolScroll { .. }) => return,
            _ => {}
        }
        if let Some(VideoDrag::FadeSelect { anchor }) = self.drag {
            let t = self.x_to_time(x);
            if (t - anchor).abs() < 0.15 {
                self.timeline.fade_selection = None;
                self.timeline.select_fade_at(anchor);
            }
        }
        if let Some(VideoDrag::AudioBody {
            from, to, armed, ..
        }) = self.drag.take()
        {
            if armed && from != to {
                self.push_undo();
                self.timeline.move_audio(from, to);
                self.audio.set_clips(self.timeline.audio_clips.clone());
                self.status = "已调整音频顺序".into();
            }
            self.drag = None;
            cx.notify();
            return;
        }
        self.drag = None;
        cx.notify();
    }

    pub fn audio_drag_ghost(&self) -> impl IntoElement {
        let Some(VideoDrag::AudioBody {
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            label,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let gx = origin_x + (x - start_x);
        let gy = origin_y + (y - start_y);
        div()
            .id("sv-audio-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x0891b2))
            .text_color(rgb(0xffffff))
            .text_xs()
            .border_1()
            .border_color(rgb(0x0e7490))
            .whitespace_nowrap()
            .child(label.clone())
            .into_any_element()
    }

    /// 拖动素材池自定义滚动条滑块 (可能由宿主跨面板转发调用).
    fn apply_pool_scroll_drag(&mut self, mouse_y: f32, grab: f32, cx: &mut Context<Self>) {
        let handle = self.pool_scroll.clone();
        let max_y = f32::from(handle.max_offset().height);
        if max_y <= 0.5 {
            return;
        }
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        let track_top = f32::from(bounds.origin.y);
        let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
        let travel = (track_h - thumb_h).max(1.0);
        let thumb_top = (mouse_y - grab - track_top).clamp(0.0, travel);
        let frac = thumb_top / travel;
        handle.set_offset(point(px(0.), px(-frac * max_y)));
        cx.notify();
    }

    pub fn is_export_open(&self) -> bool {
        self.export_open
    }

    /// 读取帧率输入框当前文本, 解析失败或超范围时回退到合理值.
    fn export_fps(&self, cx: &App) -> u32 {
        self.export_fps_input
            .read(cx)
            .text()
            .trim()
            .parse::<u32>()
            .unwrap_or(30)
            .clamp(1, 240)
    }

    fn start_export(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let ext = self.export_container.ext();
        let Some(out_path) = rfd::FileDialog::new()
            .add_filter(self.export_container.label(), &[ext])
            .set_file_name(&format!("output.{ext}"))
            .save_file()
        else {
            return;
        };
        let (w, h) = ExportOptions::size_from_pool(&self.pool);
        let opts = ExportOptions {
            container: self.export_container,
            width: w,
            height: h,
            fps: self.export_fps(cx),
            crf: self.export_crf,
            out_path: out_path.clone(),
        };
        self.export_out_path = Some(out_path);
        self.exporting = true;
        self.export_progress = "准备中...".into();
        self.export_log.clear();
        cx.notify();
        let rx = crate::export::export_async(self.timeline.clone(), self.pool.clone(), opts);
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let stop = matches!(msg, ExportMsg::Done(_));
                let _ = this.update(cx, |view, cx| {
                    match msg {
                        ExportMsg::Progress(s) => {
                            let line: SharedString = s.into();
                            view.export_progress = line.clone();
                            view.export_log.push(line);
                            const MAX_LOG: usize = 400;
                            if view.export_log.len() > MAX_LOG {
                                let drop_n = view.export_log.len() - MAX_LOG;
                                view.export_log.drain(0..drop_n);
                            }
                        }
                        ExportMsg::Done(Ok(path)) => {
                            view.exporting = false;
                            view.export_open = false;
                            view.status = format!("导出完成: {}", path.display()).into();
                        }
                        ExportMsg::Done(Err(e)) => {
                            view.exporting = false;
                            view.export_progress = "导出失败, 详情见下方日志".into();
                            for line in e.lines() {
                                view.export_log.push(line.to_string().into());
                            }
                        }
                    }
                    cx.notify();
                });
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    fn btn(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if primary { rgb(0x2563eb) } else { rgb(0x334155) };
        let hover = if primary { rgb(0x1d4ed8) } else { rgb(0x475569) };
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_md()
            .bg(bg)
            .text_color(rgb(0xffffff))
            .text_xs()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(label.into())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn transport_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let playing = self.audio.is_playing();
        let time_label = format!(
            "{} / {}",
            fmt_time(self.timeline.playhead),
            fmt_time(self.timeline.timeline_end())
        );
        div()
            .id("sv_transport")
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .bg(rgb(0x1e293b))
            .border_b_1()
            .border_color(rgb(0x0f172a))
            .child(self.btn(
                "sv_play",
                if playing { "暂停" } else { "播放" },
                true,
                |this, _, cx| this.play_pause(cx),
                cx,
            ))
            .child(self.btn(
                "sv_insert_next",
                "插入下一张 (N)",
                false,
                |this, _, cx| this.insert_next(cx),
                cx,
            ))
            .child(self.btn(
                "sv_fade_in",
                "标记淡入 (I)",
                false,
                |this, _, cx| this.mark_fade_in(cx),
                cx,
            ))
            .child(self.btn(
                "sv_fade_out",
                "标记淡出 (O)",
                false,
                |this, _, cx| this.mark_fade_out(cx),
                cx,
            ))
            .child(self.btn(
                "sv_delete",
                "删除选中 (Del)",
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(self.btn(
                "sv_import_audio",
                "导入音频",
                false,
                |this, _, cx| this.import_audio(cx),
                cx,
            ))
            // 分割按钮必须用 mouse_down + stop_propagation: 若走普通 btn 的
            // mouse_up, 待命时点按钮取消会先被左侧 panel 的 mouse_down 当成
            // 「点别处取消」清掉 armed, 再被 mouse_up 的 toggle 重新打开.
            .child({
                let armed = self.split_audio_armed;
                let label: SharedString = if armed {
                    "分割音频 (点轨道选位置...)".into()
                } else {
                    "分割音频".into()
                };
                let bg = if armed { rgb(0x2563eb) } else { rgb(0x334155) };
                let hover = if armed { rgb(0x1d4ed8) } else { rgb(0x475569) };
                div()
                    .id("sv_split_audio")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(bg)
                    .text_color(rgb(0xffffff))
                    .text_xs()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_split_audio_armed(cx);
                        }),
                    )
            })
            .child(
                div()
                    .ml_auto()
                    .text_xs()
                    .text_color(rgb(0x94a3b8))
                    .child(time_label),
            )
    }

    fn preview(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.timeline.playhead;
        let cur_group = self.timeline.covering_clip(t).map(|c| c.group_id.clone());
        let img = cur_group.as_deref().and_then(|g| self.image_for(g));
        let fade_alpha = self
            .timeline
            .covering_fade(t)
            .map(|f| {
                let span = (f.end - f.start).max(1e-6);
                let p = ((t - f.start) / span).clamp(0.0, 1.0);
                match f.kind {
                    FadeKind::In => 1.0 - p,
                    FadeKind::Out => p,
                }
            })
            .unwrap_or(0.0) as f32;
        let aspect_w = self.aspect_w as f32;
        let aspect_h = self.aspect_h.max(1) as f32;
        // 与下方轨道的播放头竖线共用同一套缩放/滚动映射, 让这条进度条填充位置
        // 始终跟轨道上的红竖线严格对齐 (而不是单纯按"播放时刻 / 总时长"的
        // 比例来算, 那样在轨道缩放后位置就对不上了).
        let width = f32::from(self.tracks_bounds.size.width).max(1.0);
        let progress_x = ((t - self.track_scroll) as f32) * self.px_per_sec;
        let progress = (progress_x / width).clamp(0.0, 1.0);

        div()
            .id("sv_preview")
            .relative()
            .w_full()
            .flex_1()
            .min_h(px(PREVIEW_H * 0.4))
            .bg(rgb(0x020617))
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.preview_bounds = bounds;
                            });
                        }
                    },
                    move |bounds, _, window, _| {
                        let vw = f32::from(bounds.size.width);
                        let vh = f32::from(bounds.size.height).max(1.0);
                        let fit = (vw / aspect_w).min(vh / aspect_h).max(0.0001);
                        let dw = aspect_w * fit;
                        let dh = aspect_h * fit;
                        let ox = bounds.origin.x + px((vw - dw) * 0.5);
                        let oy = bounds.origin.y + px((vh - dh) * 0.5);
                        let img_bounds = Bounds {
                            origin: point(ox, oy),
                            size: size(px(dw), px(dh)),
                        };
                        if let Some(img) = &img {
                            let _ =
                                window.paint_image(img_bounds, Corners::default(), img.clone(), 0, false);
                        } else {
                            window.paint_quad(gpui::fill(img_bounds, rgb(0x111827)));
                        }
                        if fade_alpha > 0.004 {
                            let mut faded = rgba(0x000000ff);
                            faded.a = fade_alpha;
                            window.paint_quad(gpui::fill(img_bounds, faded));
                        }
                    },
                )
                .size_full(),
            )
            .child(
                // 进度条 (始终显示在底部, 可拖动寻址).
                div()
                    .id("sv_progress_bar")
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(BAR_H))
                    .bg(rgb(0x1e293b))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            this.drag = Some(VideoDrag::Seek);
                            let x = f32::from(ev.position.x);
                            this.seek_from_preview_x(x, cx);
                        }),
                    )
                    .child(
                        div()
                            .h_full()
                            .bg(rgb(0x3b82f6))
                            .w(gpui::relative(progress.clamp(0.0, 1.0))),
                    ),
            )
    }

    fn video_track_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pps = self.px_per_sec;
        let scroll = self.track_scroll;
        let selected = self.timeline.selected_clip;
        let mut row = div()
            .id("sv_video_row")
            .relative()
            .w_full()
            .h(px(TRACK_H))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(0x1e293b));
        for c in self.timeline.video_clips.clone() {
            let x = ((c.start - scroll) as f32) * pps;
            let w = ((c.end - c.start) as f32 * pps).max(2.0);
            let label: SharedString = self
                .pool
                .iter()
                .find(|m| m.group_id == c.group_id)
                .map(|m| m.label.clone())
                .unwrap_or_else(|| c.group_id.clone().into());
            let is_sel = selected == Some(c.id);
            let id = c.id;
            row = row.child(
                div()
                    .id(SharedString::from(format!("sv-clip-{id}")))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(x))
                    .w(px(w))
                    .bg(if is_sel { rgb(0x2563eb) } else { rgb(0x334155) })
                    .border_1()
                    .border_color(if is_sel {
                        rgb(0x93c5fd)
                    } else {
                        rgb(0x0f172a)
                    })
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(0xe2e8f0))
                    .px_1()
                    .child(label)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            this.begin_clip_drag(id, x, cx);
                        }),
                    ),
            );
        }
        row
    }

    fn fade_track_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pps = self.px_per_sec;
        let scroll = self.track_scroll;
        let selected = self.timeline.selected_fade;
        let sel_range = self.timeline.fade_selection;
        let mut row = div()
            .id("sv_fade_row")
            .relative()
            .w_full()
            .h(px(TRACK_H))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(0x1e293b))
            .bg(rgb(0x111827))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    // 命中已有淡入淡出条目的处理在条目自身的 on_mouse_down 里
                    // 通过 `stop_propagation` 拦截, 这里只处理空白区域拖选新建.
                    let x = f32::from(ev.position.x);
                    let t = this.snap_time(this.x_to_time(x), SnapExclude::None);
                    this.timeline.selected_fade = None;
                    this.timeline.fade_selection = Some((t, t));
                    this.drag = Some(VideoDrag::FadeSelect { anchor: t });
                    cx.notify();
                }),
            );
        for f in self.timeline.fades.clone() {
            let x = ((f.start - scroll) as f32) * pps;
            let w = ((f.end - f.start) as f32 * pps).max(2.0);
            let is_sel = selected == Some(f.id);
            let label = match f.kind {
                FadeKind::In => "淡入",
                FadeKind::Out => "淡出",
            };
            let base_color = match f.kind {
                FadeKind::In => rgb(0x0d9488),
                FadeKind::Out => rgb(0xb45309),
            };
            let id = f.id;
            row = row.child(
                div()
                    .id(SharedString::from(format!("sv-fade-{id}")))
                    .absolute()
                    .top_1()
                    .bottom_1()
                    .left(px(x))
                    .w(px(w))
                    .bg(base_color)
                    .border_1()
                    .border_color(if is_sel {
                        rgb(0xf8fafc)
                    } else {
                        rgb(0x0f172a)
                    })
                    .rounded_sm()
                    .text_xs()
                    .text_color(rgb(0xf1f5f9))
                    .px_1()
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let x = f32::from(ev.position.x);
                            this.begin_fade_drag(id, x, cx);
                        }),
                    ),
            );
        }
        if let Some((a, b)) = sel_range {
            let (s, e) = if a <= b { (a, b) } else { (b, a) };
            let x = ((s - scroll) as f32) * pps;
            let w = ((e - s) as f32 * pps).max(1.0);
            row = row.child(
                div()
                    .id("sv_fade_pending_sel")
                    .absolute()
                    .top_1()
                    .bottom_1()
                    .left(px(x))
                    .w(px(w))
                    .bg(rgba(0x3b82f655))
                    .border_1()
                    .border_color(rgb(0x93c5fd))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            // 预框选已存在: 靠近其两侧边缘则拖动改边界, 否则
                            // (点在中间) 视为放弃当前预框选, 从这里重新拖选.
                            cx.stop_propagation();
                            let mx = f32::from(ev.position.x);
                            let origin_x = f32::from(this.tracks_bounds.origin.x)
                                - (this.track_scroll as f32) * this.px_per_sec;
                            let start_x = origin_x + (s as f32) * this.px_per_sec;
                            let end_x = origin_x + (e as f32) * this.px_per_sec;
                            if (mx - start_x).abs() <= EDGE_ZONE {
                                this.drag = Some(VideoDrag::FadeSelectTrimLeft);
                            } else if (mx - end_x).abs() <= EDGE_ZONE {
                                this.drag = Some(VideoDrag::FadeSelectTrimRight);
                            } else {
                                let t = this.snap_time(this.x_to_time(mx), SnapExclude::None);
                                this.timeline.selected_fade = None;
                                this.timeline.fade_selection = Some((t, t));
                                this.drag = Some(VideoDrag::FadeSelect { anchor: t });
                            }
                            cx.notify();
                        }),
                    ),
            );
        }
        row
    }

    fn audio_track_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pps = self.px_per_sec;
        let scroll = self.track_scroll;
        let selected = self.timeline.selected_audio;
        let split_armed = self.split_audio_armed;
        let drag_from = match &self.drag {
            Some(VideoDrag::AudioBody {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };
        let (line_at, line_after) = match &self.drag {
            Some(VideoDrag::AudioBody {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };
        let mut row = div()
            .id("sv_audio_row")
            .relative()
            .w_full()
            .h(px(AUDIO_TRACK_H))
            .flex_shrink_0()
            .bg(if split_armed { rgb(0x0e7490) } else { rgb(0x082f2f) })
            .border_1()
            .border_color(if split_armed {
                rgb(0x22d3ee)
            } else {
                rgb(0x082f2f)
            })
            .cursor(if split_armed {
                CursorStyle::Crosshair
            } else {
                CursorStyle::Arrow
            })
            // 空白处也能接收分割点击 (片段没盖满整轨时).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if this.split_audio_armed {
                        cx.stop_propagation();
                        this.handle_split_audio_click(f32::from(ev.position.x), cx);
                    }
                }),
            );
        let mut cum = 0.0f64;
        for (idx, c) in self.timeline.audio_clips.clone().into_iter().enumerate() {
            let x = ((cum - scroll) as f32) * pps;
            let w = (c.duration as f32 * pps).max(2.0);
            let is_sel = selected == Some(c.id);
            let id = c.id;
            let dragging = drag_from == Some(idx);
            let show_line = line_at == Some(idx);
            let waveform = self.waveform_for(&c.path, cx);
            let mut clip = div()
                .id(SharedString::from(format!("sv-audio-{id}")))
                .relative()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(x))
                .w(px(w))
                .bg(if is_sel { rgb(0x0891b2) } else { rgb(0x155e63) })
                .border_1()
                .border_color(rgb(0x0f172a))
                .overflow_hidden()
                .cursor(if split_armed {
                    CursorStyle::Crosshair
                } else {
                    CursorStyle::PointingHand
                })
                .when(dragging, |d| d.opacity(0.35))
                .when(show_line && !line_after, |d| {
                    d.border_l_2().border_color(rgb(0xf59e0b))
                })
                .when(show_line && line_after, |d| {
                    d.border_r_2().border_color(rgb(0xf59e0b))
                })
                .child({
                    let entity = cx.entity().clone();
                    canvas(
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.audio_clip_bounds.insert(idx, bounds);
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                    .size_full()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        // 分割待命时优先切开, 绝不进入选中/拖拽排序.
                        if this.split_audio_armed {
                            cx.stop_propagation();
                            this.handle_split_audio_click(f32::from(ev.position.x), cx);
                            return;
                        }
                        this.begin_audio_drag(
                            id,
                            f32::from(ev.position.x),
                            f32::from(ev.position.y),
                            cx,
                        );
                    }),
                );
            if let Some(peaks) = waveform {
                clip = clip.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let w = f32::from(bounds.size.width);
                            let h = f32::from(bounds.size.height);
                            let mid_y = f32::from(bounds.origin.y) + h * 0.5;
                            let ox = f32::from(bounds.origin.x);
                            let n_peaks = peaks.len();
                            if n_peaks == 0 || w < 1.0 {
                                return;
                            }
                            // 按当前屏幕宽度重新采样 (每列一像素): 缩放越小
                            // 时一列覆盖多个原始峰值点, 取其中最大值 (标准
                            // 波形降采样手法); 缩放越大时一列覆盖不到一个
                            // 原始点, 则在相邻两点间线性插值. 分辨率因此
                            // 始终跟着当前缩放丝滑变化, 而不是固定一批点被
                            // 硬拉伸/压缩成同一个"采样率"的样子.
                            let n_cols = (w.round() as usize).max(1).min(4000);
                            let col_w = w / n_cols as f32;
                            let step = n_peaks as f32 / n_cols as f32;
                            for col in 0..n_cols {
                                let start_f = col as f32 * step;
                                let p = if step >= 1.0 {
                                    let s = (start_f as usize).min(n_peaks - 1);
                                    let e = ((start_f + step).ceil() as usize)
                                        .clamp(s + 1, n_peaks);
                                    peaks[s..e].iter().copied().fold(0.0f32, f32::max)
                                } else {
                                    let i0 = (start_f.floor() as usize).min(n_peaks - 1);
                                    let i1 = (i0 + 1).min(n_peaks - 1);
                                    let frac = start_f - i0 as f32;
                                    peaks[i0] * (1.0 - frac) + peaks[i1] * frac
                                };
                                let bh = (h * 0.5 * p).max(1.0);
                                let bx = ox + col as f32 * col_w;
                                let bar_bounds = Bounds {
                                    origin: point(px(bx), px(mid_y - bh)),
                                    size: size(px(col_w.max(1.0)), px(bh * 2.0)),
                                };
                                window.paint_quad(gpui::fill(bar_bounds, rgba(0x5eead488)));
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
            }
            clip = clip.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .text_xs()
                    .text_color(rgb(0xe2e8f0))
                    .bg(rgba(0x0f172ab0))
                    .px_1()
                    .child(c.label.clone()),
            );
            row = row.child(clip);
            cum += c.duration;
        }
        row
    }

    /// 缩到最小时正好能显示完整时间轴的 px/秒 (三条轨道共用同一个
    /// `px_per_sec`, 因此缩放天然是同步的); 缩放没有上限.
    fn min_px_per_sec(&self) -> f32 {
        let end = self.timeline.timeline_end().max(1.0) as f32;
        let width = f32::from(self.tracks_bounds.size.width).max(1.0);
        (width / end).max(0.01)
    }

    /// Ctrl+滚轮以鼠标所在时刻为锚点缩放轨道 (无上限, 下限为"全部轨道可见");
    /// 普通滚轮横向平移 (时间轴过长超出可视宽度时用).
    fn on_tracks_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta_y = match event.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * 30.0,
        };
        if delta_y.abs() < 0.01 {
            return;
        }
        let min_pps = self.min_px_per_sec();
        if event.modifiers.control {
            let mouse_x = f32::from(event.position.x);
            let anchor_t = self.x_to_time(mouse_x);
            let factor = if delta_y > 0.0 { 1.15 } else { 1.0 / 1.15 };
            self.track_user_zoomed = true;
            self.px_per_sec = (self.px_per_sec * factor).max(min_pps);
            let origin_x = f32::from(self.tracks_bounds.origin.x);
            let rel = (mouse_x - origin_x).max(0.0);
            self.track_scroll = (anchor_t - (rel / self.px_per_sec) as f64).max(0.0);
        } else {
            self.track_scroll = (self.track_scroll - (delta_y as f64) / self.px_per_sec.max(0.01) as f64)
                .max(0.0);
        }
        cx.notify();
    }

    /// 每帧渲染轨道区之前先钳定一次本帧的缩放/滚动 (预览窗顶部的进度条与轨道
    /// 播放头竖线共用这份 `px_per_sec`/`track_scroll`, 必须在两者渲染之前先
    /// 统一算好, 否则两处各自读到不同帧的值会不同步).
    /// 缩放没有上限, 缩到最小正好显示完整时间轴; 播放中若播放头贴近右边缘
    /// 会提前把轨道向前滚动跟随, 而不是让竖线本身移出可视区域外.
    fn update_track_view(&mut self) {
        let end = self.timeline.timeline_end().max(1.0);
        let width = f32::from(self.tracks_bounds.size.width).max(1.0);
        let min_pps = self.min_px_per_sec();
        if !self.track_user_zoomed {
            self.px_per_sec = min_pps;
        } else {
            self.px_per_sec = self.px_per_sec.max(min_pps);
        }
        let visible_secs = (width / self.px_per_sec.max(0.01)) as f64;
        let max_scroll = (end as f64 - visible_secs).max(0.0);
        self.track_scroll = self.track_scroll.clamp(0.0, max_scroll);

        if self.audio.is_playing() {
            let follow_margin = 24.0f32.min(width * 0.15);
            let raw_x = ((self.timeline.playhead - self.track_scroll) as f32) * self.px_per_sec;
            if raw_x > width - follow_margin {
                let target = self.timeline.playhead
                    - ((width - follow_margin) / self.px_per_sec) as f64;
                self.track_scroll = target.clamp(0.0, max_scroll);
            }
        }
    }

    /// 当前可视时间窗口长度 (秒), 由轨道区宽度与当前缩放算出.
    fn visible_secs(&self) -> f64 {
        let width = f32::from(self.tracks_bounds.size.width).max(1.0);
        (width / self.px_per_sec.max(0.01)) as f64
    }

    /// 底部缩放条上某屏幕 x 坐标对应的时间轴时刻 (条上 0..宽度 线性映射到
    /// 0..时间轴总长, 与轨道区自身的 `x_to_time` 是两套不同的映射).
    fn track_bar_x_to_time(&self, mouse_x: f32) -> f64 {
        let end = self.timeline.timeline_end().max(1.0);
        let origin_x = f32::from(self.track_bar_bounds.origin.x);
        let width = f32::from(self.track_bar_bounds.size.width).max(1.0);
        let frac = ((mouse_x - origin_x) / width).clamp(0.0, 1.0);
        frac as f64 * end
    }

    /// 拖动缩放条滑块本体: 平移可视窗口 (不改变缩放).
    fn apply_track_bar_pan(&mut self, mouse_x: f32, grab: f32, cx: &mut Context<Self>) {
        let end = self.timeline.timeline_end().max(1.0);
        let visible = self.visible_secs();
        let max_scroll = (end - visible).max(0.0);
        let width = f32::from(self.track_bar_bounds.size.width).max(1.0);
        let origin_x = f32::from(self.track_bar_bounds.origin.x);
        let thumb_w = ((visible / end) as f32 * width).clamp(24.0f32.min(width), width);
        let travel = (width - thumb_w).max(1.0);
        let target = (mouse_x - origin_x - grab).clamp(0.0, travel);
        self.track_scroll = (target / travel) as f64 * max_scroll;
        cx.notify();
    }

    /// 拖动缩放条滑块左端圆点: 改变可视窗口左边界 (=缩放), 锚定右边界时刻.
    fn apply_track_bar_zoom_left(&mut self, mouse_x: f32, anchor_end_t: f64, cx: &mut Context<Self>) {
        let width_px = f32::from(self.tracks_bounds.size.width).max(1.0);
        let t = self.track_bar_x_to_time(mouse_x);
        let max_start = (anchor_end_t - MIN_VISIBLE_SECS).max(0.0);
        let new_start = t.clamp(0.0, max_start);
        let visible = (anchor_end_t - new_start).max(MIN_VISIBLE_SECS);
        self.track_scroll = new_start;
        self.px_per_sec = (width_px / visible as f32).max(0.01);
        self.track_user_zoomed = true;
        cx.notify();
    }

    /// 拖动缩放条滑块右端圆点: 改变可视窗口右边界 (=缩放), 锚定左边界时刻.
    fn apply_track_bar_zoom_right(
        &mut self,
        mouse_x: f32,
        anchor_start_t: f64,
        cx: &mut Context<Self>,
    ) {
        let width_px = f32::from(self.tracks_bounds.size.width).max(1.0);
        let end = self.timeline.timeline_end().max(1.0);
        let t = self.track_bar_x_to_time(mouse_x);
        let min_end = anchor_start_t + MIN_VISIBLE_SECS;
        let new_end = t.clamp(min_end, end.max(min_end));
        let visible = (new_end - anchor_start_t).max(MIN_VISIBLE_SECS);
        self.track_scroll = anchor_start_t;
        self.px_per_sec = (width_px / visible as f32).max(0.01);
        self.track_user_zoomed = true;
        cx.notify();
    }

    /// 音频波形峰值 (命中缓存则直接返回; 否则后台解码一次并缓存, 本次先
    /// 返回 `None`, 解码完成后会自行 `cx.notify()` 刷新).
    fn waveform_for(&mut self, path: &PathBuf, cx: &mut Context<Self>) -> Option<Arc<Vec<f32>>> {
        if let Some(w) = self.waveform_cache.get(path) {
            return Some(w.clone());
        }
        if !self.waveform_pending.insert(path.clone()) {
            return None;
        }
        let p = path.clone();
        let (tx, rx) = async_channel::bounded::<Option<Vec<f32>>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(compute_waveform_peaks(&p));
        });
        let path_key = path.clone();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else {
                return;
            };
            this.update(cx, |view, cx| {
                view.waveform_pending.remove(&path_key);
                if let Some(peaks) = result {
                    view.waveform_cache.insert(path_key, Arc::new(peaks));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        None
    }

    fn tracks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let playhead_x = ((self.timeline.playhead - self.track_scroll) as f32) * self.px_per_sec;

        div()
            .id("sv_tracks")
            .relative()
            .w_full()
            .h(px(TRACKS_TOTAL_H))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::on_tracks_scroll))
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.tracks_bounds = bounds;
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.video_track_row(cx))
            .child(self.fade_track_row(cx))
            .child(self.audio_track_row(cx))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(playhead_x))
                    .w(px(2.))
                    .bg(rgb(0xf87171)),
            )
    }

    /// 底部横向缩放/滚动条: 主体逻辑与素材池的竖直滚动条一致 (点击空白处
    /// 跳转, 拖动滑块本体平移); 额外在滑块两端各加一个小圆点, 拖动圆点改变
    /// 该端边界时刻从而改变缩放 (剪辑软件常见的时间轴缩放条手感): 滑块
    /// (可视窗口) 越短缩放越大, 拖到撑满整条则回到最小缩放 (完整时间轴可见).
    fn track_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let end = self.timeline.timeline_end().max(1.0);
        let visible = self.visible_secs();
        let max_scroll = (end - visible).max(0.0);
        let width = f32::from(self.track_bar_bounds.size.width).max(1.0);
        let thumb_w = ((visible / end) as f32 * width).clamp(24.0f32.min(width), width);
        let travel = (width - thumb_w).max(1.0);
        let frac = if max_scroll > 0.0 {
            (self.track_scroll / max_scroll).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let thumb_left = frac * travel;
        let start_t = self.track_scroll;
        let end_t = (self.track_scroll + visible).min(end);

        div()
            .id("sv_track_bar")
            .relative()
            .w_full()
            .h(px(TRACK_BAR_H))
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(0x1e293b))
            .bg(rgb(0x0b1220))
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.track_bar_bounds = bounds;
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                // 点击滑块之外的空白处 = 以点击处为中心跳转可视窗口.
                div()
                    .id("sv_track_bar_track")
                    .absolute()
                    .inset_0()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let x = f32::from(ev.position.x);
                            let t = this.track_bar_x_to_time(x);
                            let half = this.visible_secs() * 0.5;
                            let max_scroll =
                                (this.timeline.timeline_end().max(1.0) - this.visible_secs())
                                    .max(0.0);
                            this.track_scroll = (t - half).clamp(0.0, max_scroll);
                            this.track_user_zoomed = true;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .id("sv_track_bar_thumb")
                    .absolute()
                    .top(px(2.))
                    .bottom(px(2.))
                    .left(px(thumb_left))
                    .w(px(thumb_w))
                    .rounded_sm()
                    .bg(rgb(0x334155))
                    .hover(|s| s.bg(rgb(0x475569)))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let x = f32::from(ev.position.x);
                            let origin_x = f32::from(this.track_bar_bounds.origin.x);
                            let grab = x - origin_x - thumb_left;
                            this.drag = Some(VideoDrag::TrackBarPan { grab });
                            cx.notify();
                        }),
                    )
                    .child(
                        // 左端圆点: 拖动改变左边界 (=缩放), 锚定右边界时刻.
                        div()
                            .id("sv_track_bar_grip_l")
                            .absolute()
                            .left(px(-5.))
                            .top(px(1.))
                            .w(px(11.))
                            .h(px(11.))
                            .rounded_full()
                            .bg(rgb(0x93c5fd))
                            .border_1()
                            .border_color(rgb(0x0f172a))
                            .cursor(CursorStyle::ResizeColumn)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.drag =
                                        Some(VideoDrag::TrackBarZoomLeft { anchor_end_t: end_t });
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        // 右端圆点: 拖动改变右边界 (=缩放), 锚定左边界时刻.
                        div()
                            .id("sv_track_bar_grip_r")
                            .absolute()
                            .right(px(-5.))
                            .top(px(1.))
                            .w(px(11.))
                            .h(px(11.))
                            .rounded_full()
                            .bg(rgb(0x93c5fd))
                            .border_1()
                            .border_color(rgb(0x0f172a))
                            .cursor(CursorStyle::ResizeColumn)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.drag = Some(VideoDrag::TrackBarZoomRight {
                                        anchor_start_t: start_t,
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }

    /// 画布区 (预览窗 + 三条轨道), 挂到宿主的「画布」区域.
    pub fn left_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // 预览窗顶部的进度条与下方轨道的播放头竖线共用同一份缩放/滚动状态,
        // 必须在渲染二者之前先统一算好这一帧的值, 否则前者会读到上一帧的
        // 陈旧数据 (因为 `preview()` 在 `tracks()` 之前渲染).
        self.update_track_view();
        div()
            .id("sv_left")
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            .bg(rgb(0x0b1220))
            .text_color(rgb(0xe2e8f0))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let x = f32::from(ev.position.x);
                let y = f32::from(ev.position.y);
                this.apply_left_drag_move(x, y, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    this.end_left_drag(f32::from(ev.position.x), cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    if this.drag.is_some() {
                        this.end_left_drag(f32::from(ev.position.x), cx);
                    }
                }),
            )
            .child(self.transport_bar(cx))
            .child(self.preview(cx))
            .child(self.tracks(cx))
            .child(self.track_bar(cx))
            .child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x64748b))
                    .bg(rgb(0x0f172a))
                    .child(self.status.clone()),
            )
    }

    /// 侧栏区 (素材池 + 导出按钮), 挂到宿主的「侧栏」区域.
    pub fn right_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("sv_pool_list")
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_scroll()
            .track_scroll(&self.pool_scroll)
            .scrollbar_width(px(0.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2();
        for item in self.pool.clone() {
            let gid = item.group_id.clone();
            let gid2 = gid.clone();
            let expanded = self.expanded_pool.as_deref() == Some(gid.as_str());
            let mut entry = div()
                .id(SharedString::from(format!("sv-pool-entry-{gid}")))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .id(SharedString::from(format!("sv-pool-{gid}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(expanded, |s| s.bg(rgb(0x334155)))
                        .when(!expanded, |s| s.bg(rgb(0x1e293b)))
                        .text_color(rgb(0xe2e8f0))
                        .text_xs()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0x334155)))
                        .child(item.label.clone())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                let x = f32::from(ev.position.x);
                                let y = f32::from(ev.position.y);
                                this.drag = Some(VideoDrag::PoolDrop {
                                    group_id: gid2.clone(),
                                    start_x: x,
                                    start_y: y,
                                    last_x: x,
                                    last_y: y,
                                });
                                cx.notify();
                            }),
                        ),
                );
            // 点击 (非拖动) 时向下展开该素材的图片预览; 手动加入时间轴请改为
            // 拖拽到左侧视频轨道上的具体位置.
            if expanded {
                let img = self.image_for(&gid);
                entry = entry.child(
                    div()
                        .id(SharedString::from(format!("sv-pool-preview-{gid}")))
                        .w_full()
                        .h(px(160.))
                        .rounded_md()
                        .bg(rgb(0x020617))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    if let Some(img) = &img {
                                        let sz = img.size(0);
                                        let iw = (sz.width.0 as f32).max(1.0);
                                        let ih = (sz.height.0 as f32).max(1.0);
                                        let vw = f32::from(bounds.size.width);
                                        let vh = f32::from(bounds.size.height);
                                        let fit = (vw / iw).min(vh / ih).max(0.0001);
                                        let dw = iw * fit;
                                        let dh = ih * fit;
                                        let ox = bounds.origin.x + px((vw - dw) * 0.5);
                                        let oy = bounds.origin.y + px((vh - dh) * 0.5);
                                        let img_bounds = Bounds {
                                            origin: point(ox, oy),
                                            size: size(px(dw), px(dh)),
                                        };
                                        let _ = window.paint_image(
                                            img_bounds,
                                            Corners::default(),
                                            img.clone(),
                                            0,
                                            false,
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        ),
                );
            }
            list = list.child(entry);
        }

        // 竖直拖动条 (与宿主其它列表滚动条同款样式/交互, 仅内容溢出时显示).
        let handle = self.pool_scroll.clone();
        let max_y = f32::from(handle.max_offset().height);
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        let show_v = max_y > 1.0 && track_h > 1.0;
        let mut list_row = div()
            .id("sv_pool_row")
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(list);
        if show_v {
            let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
            let travel = (track_h - thumb_h).max(1.0);
            let off_y = -f32::from(handle.offset().y);
            let frac = (off_y / max_y).clamp(0.0, 1.0);
            let thumb_top = frac * travel;
            list_row = list_row.child(
                div()
                    .id("sv_pool_vtrack")
                    .w(px(10.))
                    .h_full()
                    .flex_shrink_0()
                    .relative()
                    .rounded_sm()
                    .bg(rgb(0x1e293b))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let y = f32::from(ev.position.y);
                            let handle = this.pool_scroll.clone();
                            let b = handle.bounds();
                            let th = f32::from(b.size.height).max(1.0);
                            let max = f32::from(handle.max_offset().height);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((th * th) / (th + max)).clamp(24.0, th);
                            let travel = (th - thumb).max(1.0);
                            let track_top = f32::from(b.origin.y);
                            let target = (y - track_top - thumb * 0.5).clamp(0.0, travel);
                            handle.set_offset(point(px(0.), px(-(target / travel) * max)));
                            this.drag = Some(VideoDrag::PoolScroll { grab: thumb * 0.5 });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id("sv_pool_vthumb")
                            .absolute()
                            .left_0()
                            .top(px(thumb_top))
                            .w_full()
                            .h(px(thumb_h))
                            .rounded_sm()
                            .bg(rgb(0x475569))
                            .hover(|s| s.bg(rgb(0x64748b)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let y = f32::from(ev.position.y);
                                    let handle = this.pool_scroll.clone();
                                    let b = handle.bounds();
                                    let th = f32::from(b.size.height).max(1.0);
                                    let max = f32::from(handle.max_offset().height);
                                    let thumb = if max > 0.5 {
                                        ((th * th) / (th + max)).clamp(24.0, th)
                                    } else {
                                        th
                                    };
                                    let travel = (th - thumb).max(1.0);
                                    let track_top = f32::from(b.origin.y);
                                    let off = -f32::from(handle.offset().y);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_top = track_top + frac * travel;
                                    this.drag = Some(VideoDrag::PoolScroll {
                                        grab: (y - cur_top).clamp(0.0, thumb),
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }

        div()
            .id("sv_right")
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            .bg(rgb(0x0f172a))
            .text_color(rgb(0xe2e8f0))
            .child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0xcbd5e1))
                    .child(format!("素材池 ({} 个输出组合, 可拖入视频轨道)", self.pool.len())),
            )
            .child(list_row)
            .child(
                div()
                    .flex_shrink_0()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(0x1e293b))
                    .child(self.btn(
                        "sv_export",
                        "导出视频...",
                        true,
                        |this, _, cx| {
                            this.export_open = true;
                            cx.notify();
                        },
                        cx,
                    )),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn stepper(
        &self,
        minus_id: &'static str,
        plus_id: &'static str,
        label: SharedString,
        on_minus: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        on_plus: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.btn(minus_id, "-", false, on_minus, cx))
            .child(div().text_sm().min_w(px(90.)).text_center().child(label))
            .child(self.btn(plus_id, "+", false, on_plus, cx))
    }

    /// 导出参数弹窗内容; 由宿主在 `dialog_overlay` 里判断 `is_export_open()` 后调用.
    pub fn export_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = ExportOptions::size_from_pool(&self.pool);
        let out_label: SharedString = self
            .export_out_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(点击「开始导出」时选择保存路径)".to_string())
            .into();
        let mp4_on = self.export_container == Container::Mp4;

        div()
            .id("sv_export_overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            // 与宿主 Help 弹窗一致: 挡住背后命中, 背景静态不接收事件.
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .w(px(440.))
                    .bg(rgb(0x1e293b))
                    .rounded_lg()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_color(rgb(0xe2e8f0))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("导出视频"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("容器格式"))
                            .child(self.btn(
                                "sv_fmt_mp4",
                                "MP4",
                                mp4_on,
                                |this, _, cx| {
                                    this.export_container = Container::Mp4;
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "sv_fmt_mkv",
                                "MKV",
                                !mp4_on,
                                |this, _, cx| {
                                    this.export_container = Container::Mkv;
                                    cx.notify();
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child(SharedString::from(self.export_container.audio_hint())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("分辨率"))
                            .child(
                                div()
                                    .text_sm()
                                    .child(format!("{w} x {h}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x64748b))
                                    .child("(与素材图片一致, 不可更改)"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("帧率"))
                            .child(self.btn(
                                "sv_fps_minus",
                                "-",
                                false,
                                |this, _, cx| {
                                    let v = this.export_fps(cx).saturating_sub(1).max(1);
                                    this.export_fps_input
                                        .update(cx, |t: &mut apply_bg::text_input::TextInput, cx| t.set_text(v.to_string(), cx));
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(div().w(px(64.)).child(self.export_fps_input.clone()))
                            .child(div().text_sm().child("fps"))
                            .child(self.btn(
                                "sv_fps_plus",
                                "+",
                                false,
                                |this, _, cx| {
                                    let v = (this.export_fps(cx) + 1).min(240);
                                    this.export_fps_input
                                        .update(cx, |t: &mut apply_bg::text_input::TextInput, cx| t.set_text(v.to_string(), cx));
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x64748b))
                                    .child("(可直接点击输入框改数字)"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("质量 (CRF)"))
                            .child(self.stepper(
                                "sv_crf_minus",
                                "sv_crf_plus",
                                format!("CRF {}", self.export_crf).into(),
                                |this, _, cx| {
                                    this.export_crf = this.export_crf.saturating_sub(1).max(14);
                                    cx.notify();
                                },
                                |this, _, cx| {
                                    this.export_crf = (this.export_crf + 1).min(28);
                                    cx.notify();
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .child("CRF (Constant Rate Factor) 是 x264 编码的质量参数: 数值越小画质越好、文件越大, 越大则画质越差、文件越小; 0 近乎无损, 18~23 常见于\"肉眼无差\"的高质量, 28 起画质明显下降. 与分辨率/码率无关, 是恒定质量而非恒定码率的编码方式."),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child(out_label),
                    )
                    .child(if self.exporting {
                        div()
                            .text_xs()
                            .text_color(rgb(0xfbbf24))
                            .child(self.export_progress.clone())
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(if self.export_log.is_empty() {
                        div().into_any_element()
                    } else {
                        // ffmpeg 不再弹终端窗口, 它的原始输出 (进度/报错) 就
                        // 直接滚动显示在这里; 只展示最近若干行, 足够看清当前
                        // 在干什么或者失败原因.
                        const SHOW_LAST: usize = 10;
                        let start = self.export_log.len().saturating_sub(SHOW_LAST);
                        div()
                            .id("sv_export_log")
                            .w_full()
                            .max_h(px(150.))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(0x0b1220))
                            .border_1()
                            .border_color(rgb(0x1e293b))
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .children(
                                self.export_log[start..]
                                    .iter()
                                    .map(|l| div().child(l.clone())),
                            )
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .mt_2()
                            .child(self.btn(
                                "sv_export_cancel",
                                "关闭",
                                false,
                                |this, _, cx| {
                                    this.export_open = false;
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "sv_export_go",
                                if self.exporting { "导出中..." } else { "开始导出" },
                                true,
                                |this, window, cx| this.start_export(window, cx),
                                cx,
                            )),
                    ),
            )
    }
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - (m as f64) * 60.0;
    format!("{m:02}:{s:05.2}")
}

impl Focusable for ScoreVideoApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ScoreVideoApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = if self.export_open {
            self.export_dialog(cx).into_any_element()
        } else {
            div().into_any_element()
        };
        // 注意: 宿主 score_sync 实际走 left_panel/right_panel, 不调用本 Render;
        // 「分割音频」的点击逻辑已挂在音频轨道/片段的 on_mouse_down 上.
        div()
            .id("sv_root")
            .key_context("ScoreVideo")
            .track_focus(&self.focus_handle)
            .relative()
            .on_action(cx.listener(|this, _: &PlayPause, _, cx| this.play_pause(cx)))
            .on_action(cx.listener(|this, _: &SeekBack, _, cx| this.seek_by(-1.0, cx)))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| this.seek_by(1.0, cx)))
            .on_action(cx.listener(|this, _: &SeekBackBig, _, cx| this.seek_by(-5.0, cx)))
            .on_action(cx.listener(|this, _: &SeekForwardBig, _, cx| this.seek_by(5.0, cx)))
            .on_action(cx.listener(|this, _: &InsertNext, _, cx| this.insert_next(cx)))
            .on_action(cx.listener(|this, _: &MarkFadeIn, _, cx| this.mark_fade_in(cx)))
            .on_action(cx.listener(|this, _: &MarkFadeOut, _, cx| this.mark_fade_out(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| this.delete_selected(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo(cx)))
            .flex()
            .flex_row()
            .size_full()
            .font_family("Microsoft YaHei UI")
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .child(self.left_panel(cx)),
            )
            .child(
                div()
                    .w(px(300.))
                    .flex_shrink_0()
                    .h_full()
                    .child(self.right_panel(cx)),
            )
            .child(overlay)
    }
}

/// 独立运行入口 (仅供该 crate 单独调试): 传入若干图片作为素材池 + 可选音频.
pub fn run_gui(images: Vec<PathBuf>, audio: Option<PathBuf>) {
    Application::new().run(move |cx: &mut App| {
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("视频轨道编辑 (调试)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let images = images.clone();
                let audio = audio.clone();
                cx.new(|cx| {
                    let mut app = ScoreVideoApp::new(cx);
                    let mut pool = Vec::new();
                    for (i, p) in images.iter().enumerate() {
                        if let Ok(im) = image::open(p) {
                            pool.push(MaterialItem {
                                group_id: format!("g{i}"),
                                label: p
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("素材")
                                    .to_string()
                                    .into(),
                                image: Arc::new(im.to_rgba8()),
                            });
                        }
                    }
                    app.set_pool(pool, cx);
                    if let Some(a) = audio {
                        if let Some(dur) = crate::audio::probe_duration(&a) {
                            app.timeline.audio_clips.push(AudioClip {
                                id: Uuid::new_v4(),
                                path: a.clone(),
                                label: a
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("audio")
                                    .to_string()
                                    .into(),
                                duration: dur,
                                offset: 0.0,
                            });
                            app.timeline.fit_after_audio_change();
                        }
                    }
                    app.focus_handle.focus(window);
                    app
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
