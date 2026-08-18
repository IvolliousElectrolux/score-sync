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
/// 波形基础采样密度 (每秒峰值点数). 基础数据按时长而非固定点数采样, 绘制时
/// 再按当前片段的屏幕宽度 (随缩放变化) 重新降采样/插值, 分辨率因此始终跟着
/// 缩放走, 而不是一批固定点数被硬拉伸/压缩成同一个"采样率"的样子.
pub(crate) const WAVEFORM_BUCKETS_PER_SEC: f64 = 300.0;
pub(crate) const WAVEFORM_MIN_BUCKETS: usize = 64;
pub(crate) const WAVEFORM_MAX_BUCKETS: usize = 200_000;
/// 三条轨道紧贴在一起的总高度 (视频轨 + 淡入淡出轨 + 稍高一些的音频轨).
pub(crate) const TRACKS_TOTAL_H: f32 = TRACK_H * 2.0 + AUDIO_TRACK_H;
/// 拖动底部缩放条圆点缩放时, 可视时间窗口的最小时长 (秒).
pub(crate) const MIN_VISIBLE_SECS: f64 = 0.2;

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
/// 桶数按时长 (而非固定常数) 换算, 保证基础数据本身有足够密度; 实际绘制时
/// 再按当前片段的屏幕宽度 (随缩放实时变化) 重新降采样/插值一次, 分辨率因此
/// 会跟着缩放丝滑变化, 而不是固定一批点被硬拉伸/压缩.
pub(crate) fn compute_waveform_peaks(path: &std::path::Path) -> Option<Vec<f32>> {
    let _ = crate::audio::ensure_preview_wav(path)?;
    let dec = crate::audio::open_decoder(path)?;
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
pub(crate) const SNAP_PX: f32 = 8.0;

/// 吸附时排除自身边界, 避免拖拽边缘粘在自己身上.
#[derive(Clone, Copy)]
pub(crate) enum SnapExclude {
    None,
    Fade(Uuid),
    Video(Uuid),
}
pub(crate) fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - (m as f64) * 60.0;
    format!("{m:02}:{s:05.2}")
}
