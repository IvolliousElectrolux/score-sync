//! 常量, 拖拽种类, 波形采样.

use super::*;

pub(crate) const VIDEO_HISTORY_LIMIT: usize = 64;

pub(crate) const PREVIEW_H: f32 = 300.0;
pub(crate) const BAR_H: f32 = 10.0;
pub(crate) const TRACK_H: f32 = 40.0;
/// 音频轨道比视频/淡入淡出轨道稍高一些, 好放下波形预览.
pub(crate) const AUDIO_TRACK_H: f32 = 64.0;
/// 底部横向缩放/滚动条高度.
pub(crate) const TRACK_BAR_H: f32 = 18.0;
/// 音频排序拖拽: 超过此像素位移才进入"已拖起" (与分块标签页一致).
pub(crate) const AUDIO_REORDER_SLOP: f32 = 5.0;
pub(crate) const EDGE_ZONE: f32 = 8.0;
/// 波形基础采样密度 (每秒峰值点数). 只是源文件缓存的上限密度; 真正画到轨道
/// 上时按**当前可视窗口的像素列**再切一次, 粒度跟着 `px_per_sec` 走.
pub(crate) const WAVEFORM_BUCKETS_PER_SEC: f64 = 300.0;
pub(crate) const WAVEFORM_MIN_BUCKETS: usize = 64;
/// 约 2 小时 × 300/s, 长 CD 也不要把密度夹成固定点数.
pub(crate) const WAVEFORM_MAX_BUCKETS: usize = 2_500_000;
/// 单次绘制最多列数 (防布局算出离谱宽度时卡死); 正常可视宽度远小于此.
pub(crate) const WAVEFORM_MAX_PAINT_COLS: usize = 4096;

/// 源文件波形缓存: 峰值覆盖整段源时长, 绘制时再按片段 `offset` + 可视窗口切.
#[derive(Clone)]
pub(crate) struct CachedWaveform {
    pub peaks: Arc<Vec<f32>>,
    pub duration: f64,
}

/// 三条轨道紧贴在一起的总高度 (视频轨 + 淡入淡出轨 + 稍高一些的音频轨).
pub(crate) const TRACKS_TOTAL_H: f32 = TRACK_H * 2.0 + AUDIO_TRACK_H;
/// 拖动底部缩放条圆点缩放时, 可视时间窗口的最小时长 (秒).
pub(crate) const MIN_VISIBLE_SECS: f64 = 0.2;
/// 预览倍速 (导出始终 1x). 按钮显示当前值, 点击列出全部档位.
pub(crate) const PLAYBACK_SPEEDS: &[f32] = &[1.0, 1.25, 1.5, 2.0, 3.0];

pub(crate) fn fmt_speed(speed: f32) -> String {
    if (speed - speed.round()).abs() < 1e-3 {
        format!("x{}", speed.round() as i32)
    } else {
        format!("x{speed}")
    }
}

#[derive(Clone)]
pub(crate) enum VideoDrag {
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
/// 桶数按时长换算, 只作为源文件缓存; 画到轨道上时再按当前可视像素列
/// (随缩放变化) 切, 并只取该片段 `offset..offset+duration` 对应的一段.
pub(crate) fn compute_waveform_peaks(path: &std::path::Path) -> Option<CachedWaveform> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::audio::waveform_peaks(
            path,
            WAVEFORM_BUCKETS_PER_SEC,
            WAVEFORM_MIN_BUCKETS,
            WAVEFORM_MAX_BUCKETS,
        )
    }))
    .ok()
    .flatten()
    .map(|(peaks, duration)| CachedWaveform {
        peaks: Arc::new(peaks),
        duration,
    })
}

/// 只画片段与轨道可视区的交集. 列对齐到时间轴像素格 (`1/px_per_sec`),
/// 滚动/播放时格子钉在音频上, 不跟着窗口左缘重新切导致波形晃.
pub(crate) fn paint_waveform_in_view(
    wave: &CachedWaveform,
    clip_offset: f64,
    clip_duration: f64,
    clip_start: f64,
    px_per_sec: f32,
    bounds: Bounds<Pixels>,
    view_left: f32,
    view_right: f32,
    window: &mut Window,
) {
    let clip_x = f32::from(bounds.origin.x);
    let clip_w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if wave.peaks.is_empty() || clip_w < 1.0 || h < 1.0 || clip_duration <= 0.0 {
        return;
    }
    let vis_left = view_left.max(clip_x);
    let vis_right = view_right.min(clip_x + clip_w);
    if vis_right - vis_left < 1.0 {
        return;
    }
    let pps = px_per_sec.max(0.01) as f64;
    let col_dur = 1.0 / pps;
    let t_vis0 = clip_start + ((vis_left - clip_x) as f64 / clip_w as f64) * clip_duration;
    let t_vis1 = clip_start + ((vis_right - clip_x) as f64 / clip_w as f64) * clip_duration;
    let first = (t_vis0 / col_dur).floor() as i64;
    let last = (t_vis1 / col_dur).ceil() as i64;
    let n_cols = ((last - first).max(1) as usize).min(WAVEFORM_MAX_PAINT_COLS);
    let mid_y = f32::from(bounds.origin.y) + h * 0.5;
    let mut path = PathBuilder::fill();
    let mut bottoms = Vec::with_capacity(n_cols);
    for i in 0..n_cols {
        let t0 = first as f64 * col_dur + i as f64 * col_dur;
        let t1 = t0 + col_dur;
        let src0 = clip_offset + (t0 - clip_start);
        let src1 = clip_offset + (t1 - clip_start);
        let p = crate::audio::waveform_peak_in_range(&wave.peaks, wave.duration, src0, src1);
        let x = clip_x + (((t0 - clip_start) / clip_duration) as f32) * clip_w;
        let bh = (h * 0.5 * p).max(1.0);
        if i == 0 {
            path.move_to(point(px(x), px(mid_y - bh)));
        } else {
            path.line_to(point(px(x), px(mid_y - bh)));
        }
        bottoms.push((x, mid_y + bh));
    }
    for (x, y) in bottoms.iter().rev() {
        path.line_to(point(px(*x), px(*y)));
    }
    path.close();
    if let Ok(built) = path.build() {
        window.paint_path(built, rgba(0x5eead488));
    }
}

/// 时间轴边界吸附阈值 (像素): 视频/淡入淡出/音频边界彼此靠近时对齐.
pub(crate) const SNAP_PX: f32 = 8.0;

/// 吸附时排除自身边界, 避免拖拽边缘粘在自己身上.
#[derive(Clone, Copy)]
pub(crate) enum SnapExclude {
    None,
    Fade(Uuid),
    Video(Uuid),
}

pub(crate) struct FadeContextMenu {
    pub x: f32,
    pub y: f32,
}

/// 无工程底色时预览/导出淡向此米色 (近似谱纸).
pub(crate) const DEFAULT_FADE_BG_RGB: [u8; 3] = [0xE8, 0xD4, 0xB0];

pub(crate) fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    let h = (t / 3600.0).floor() as u64;
    let m = ((t - (h as f64) * 3600.0) / 60.0).floor() as u64;
    let s = t - (h as f64) * 3600.0 - (m as f64) * 60.0;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:05.2}")
    } else {
        format!("{m:02}:{s:05.2}")
    }
}
