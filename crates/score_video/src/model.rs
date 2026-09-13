//! 视频时间轴数据模型: 素材 (静态谱面图) 片段 / 转场 (淡入淡出 / 刷入) / 音频片段.
//!
//! 三条轨道共用同一条时间轴 (单位: 秒, f64):
//! - `video_clips`: 彼此首尾相接、按时间升序, 覆盖 `[0, video_end())`.
//! - `fades`: 互不重叠的转场区间. 淡入/淡出可落在任意时刻; 刷入 (`WipeLtr`)
//!   必须依附翻页边界 (下一页起点), 不能单独刷在页内.
//! - `audio_clips`: 顺序播放, 不单独存起点, 由前面片段时长累加得出.
//!
//! 时间轴总长取当前非空音/视频轨的较短末端; 删短一轨时会把较长轨裁齐.
//! 末段右边缘对齐音频末尾: 向右拖只缩短该块, 不会超出音频.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::SharedString;
use image::RgbaImage;
use uuid::Uuid;

/// 素材池条目: 对应「输出组合」列表中的一张最终合成图 (全分辨率落盘, 内存按 LRU 热加载).
#[derive(Clone)]
pub struct MaterialItem {
    pub group_id: String,
    pub label: SharedString,
    /// 工程旁 `.staffcrop.cache/pool/<id>.png` (或会话临时池目录)
    pub cache_path: PathBuf,
    pub width: u32,
    pub height: u32,
}

impl MaterialItem {
    pub fn load_rgba(&self) -> Result<RgbaImage, String> {
        image::open(&self.cache_path)
            .map_err(|e| format!("读取素材缓存失败 ({}): {e}", self.cache_path.display()))
            .map(|i| i.to_rgba8())
    }

    /// 预览窗/素材池用: 优先读合成时写下的 `{gid}.prev.jpg`, 没有再缩全图.
    /// 导出仍走 [`Self::load_rgba`] 全分辨率 PNG.
    pub fn preview_path(&self) -> PathBuf {
        let stem = self
            .cache_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pool");
        self.cache_path.with_file_name(format!("{stem}.prev.jpg"))
    }

    pub fn load_preview_rgba(&self, max_side: u32) -> Result<RgbaImage, String> {
        let prev = self.preview_path();
        if prev.is_file() {
            match image::open(&prev) {
                Ok(im) => return Ok(im.to_rgba8()),
                Err(_) => {}
            }
        }
        let rgb = image::open(&self.cache_path)
            .map_err(|e| format!("读取素材缓存失败 ({}): {e}", self.cache_path.display()))?
            .to_rgb8();
        let (w, h) = rgb.dimensions();
        let m = w.max(h).max(1);
        let small = if m > max_side {
            let tw = ((w as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
            let th = ((h as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
            image::imageops::thumbnail(&rgb, tw, th)
        } else {
            rgb
        };
        Ok(image::DynamicImage::ImageRgb8(small).to_rgba8())
    }
}

/// 视频轨道上的一段.
#[derive(Clone)]
pub struct VideoClip {
    pub id: Uuid,
    pub group_id: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FadeKind {
    In,
    Out,
    /// 下一页从左到右盖住上一页 (硬切擦除, 不是整页滑入).
    WipeLtr,
}

impl FadeKind {
    pub fn is_wipe(self) -> bool {
        matches!(self, Self::WipeLtr)
    }
}

/// 转场轴上的一段: 淡入/淡出按时长线性叠黑 (或底色); 刷入按水平进度擦除.
#[derive(Clone)]
pub struct FadeSpan {
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    pub kind: FadeKind,
    /// true: 淡向工程底色 (只淡乐谱内容); false: 淡向纯黑. 刷入忽略此字段.
    pub keep_bg: bool,
}

/// 音频轨道上的一段.
#[derive(Clone)]
pub struct AudioClip {
    pub id: Uuid,
    pub path: PathBuf,
    pub label: SharedString,
    pub duration: f64,
    /// 该段在源文件里的起始时刻 (秒): 播放/导出时从源文件跳到这里开始读
    /// `duration` 长的内容. 整段导入时是 0; 用「分割音频」从中间切开时,
    /// 后半段的 `offset` = 前半段的 `offset + 分割点相对本段的偏移`, 这样
    /// 两段拼起来仍精确复现原始文件, 不用真的切分/重新编码文件本身.
    pub offset: f64,
}

/// 片段最小时长钳制, 避免拖出零宽/负宽片段.
pub const MIN_CLIP_DUR: f64 = 0.1;
/// 刷入转场默认时长 (秒): 快捷键切到下一页时从翻页边界起刷这么久.
pub const DEFAULT_WIPE_DUR: f64 = 1.0;
/// 普通播放预览刷新间隔 (毫秒).
pub const PLAY_TICK_MS: u64 = 33;
/// 刷入是硬边, 需要更高预览刷新, 否则边缘一格格跳.
pub const WIPE_TICK_MS: u64 = 8;
/// 淡入淡出预览刷新间隔 (毫秒).
pub const FADE_TICK_MS: u64 = 16;
/// 进入刷入前就开始用高刷新, 避免第一帧还停在 33ms.
pub const WIPE_TICK_LOOKAHEAD: f64 = 0.2;
/// 时间轴为空时的默认最短长度.
pub const DEFAULT_TIMELINE_MIN: f64 = 10.0;

#[derive(Default, Clone)]
pub struct Timeline {
    pub video_clips: Vec<VideoClip>,
    pub fades: Vec<FadeSpan>,
    pub audio_clips: Vec<AudioClip>,
    pub playhead: f64,
    pub selected_clip: Option<Uuid>,
    pub selected_fade: Option<Uuid>,
    /// Ctrl 多选的淡入淡出; 与 `selected_fade` 同步 (主选中必在集合内).
    pub selected_fades: HashSet<Uuid>,
    pub selected_audio: Option<Uuid>,
    /// 按键标记淡入淡出起点后, 等待第二次按键给出终点.
    pub pending_fade_anchor: Option<f64>,
    /// 鼠标在淡入淡出轨道拖选出的区间, 优先于 `pending_fade_anchor`.
    pub fade_selection: Option<(f64, f64)>,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn audio_cum_start(&self, i: usize) -> f64 {
        self.audio_clips[..i].iter().map(|c| c.duration).sum()
    }

    pub fn audio_total(&self) -> f64 {
        self.audio_clips.iter().map(|c| c.duration).sum()
    }

    pub fn video_end(&self) -> f64 {
        self.video_clips.last().map(|c| c.end).unwrap_or(0.0)
    }

    fn fades_end(&self) -> f64 {
        self.fades.iter().map(|f| f.end).fold(0.0, f64::max)
    }

    /// 非空音/视频轨的末端时刻; 两轨都有内容时取较短者 (删短一轨时把另一轨裁齐).
    pub fn shortest_av_end(&self) -> Option<f64> {
        let v = (!self.video_clips.is_empty()).then(|| self.video_end());
        let a = (!self.audio_clips.is_empty()).then(|| self.audio_total());
        match (v, a) {
            (None, None) => None,
            (Some(x), None) | (None, Some(x)) => Some(x.max(0.0)),
            (Some(v), Some(a)) => Some(v.min(a).max(0.0)),
        }
    }

    /// 时间轴总长: 以当前最短的非空音/视频轨末端为准; 都空时用淡入淡出末端或默认下限.
    pub fn timeline_end(&self) -> f64 {
        self.shortest_av_end()
            .unwrap_or_else(|| self.fades_end().max(DEFAULT_TIMELINE_MIN))
    }

    /// 把视频轨裁到 `end` (删掉完全落在之后的片段, 缩短末段).
    pub fn trim_video_to(&mut self, end: f64) {
        let end = end.max(0.0);
        while let Some(last) = self.video_clips.last() {
            if last.start >= end - 1e-9 {
                self.video_clips.pop();
                continue;
            }
            if last.end > end {
                let idx = self.video_clips.len() - 1;
                if end - self.video_clips[idx].start < MIN_CLIP_DUR {
                    self.video_clips.pop();
                } else {
                    self.video_clips[idx].end = end;
                }
            }
            break;
        }
    }

    /// 把末段视频延伸到 `end` (仅当已有视频且末段终点偏短时).
    pub fn extend_video_to(&mut self, end: f64) {
        let end = end.max(0.0);
        if let Some(last) = self.video_clips.last_mut() {
            if last.end < end {
                last.end = end;
            }
        }
    }

    /// 把音频轨总长裁到 `end` (从末段减时长 / 丢掉超出的片段).
    pub fn trim_audio_to(&mut self, end: f64) {
        let end = end.max(0.0);
        let mut total = self.audio_total();
        while total > end + 1e-9 && !self.audio_clips.is_empty() {
            let excess = total - end;
            let Some(last) = self.audio_clips.last_mut() else {
                break;
            };
            if last.duration <= excess + MIN_CLIP_DUR {
                total -= last.duration;
                self.audio_clips.pop();
            } else {
                last.duration -= excess;
                break;
            }
        }
    }

    fn trim_fades_to(&mut self, end: f64) {
        let end = end.max(0.0);
        self.fades.retain(|f| f.start < end - 1e-9);
        for f in &mut self.fades {
            if f.end > end {
                f.end = end.max(f.start + MIN_CLIP_DUR);
            }
        }
        self.fades.retain(|f| f.end - f.start >= MIN_CLIP_DUR - 1e-9);
    }

    /// 两轨都有内容时, 把较长轨裁到较短轨末端, 淡入淡出与播放头一并钳制.
    pub fn sync_tracks_to_shortest(&mut self) {
        let Some(target) = self.shortest_av_end() else {
            let end = self.timeline_end();
            self.trim_fades_to(end);
            self.playhead = self.playhead.clamp(0.0, end);
            self.clamp_wipe_spans();
            return;
        };
        self.trim_video_to(target);
        self.trim_audio_to(target);
        self.trim_fades_to(target);
        self.playhead = self.playhead.clamp(0.0, target);
        self.clamp_wipe_spans();
    }

    /// 导入/追加音频后: 音频更长则延伸视频对齐; 音频更短则裁视频对齐.
    pub fn fit_after_audio_change(&mut self) {
        if self.audio_clips.is_empty() || self.video_clips.is_empty() {
            self.sync_tracks_to_shortest();
            return;
        }
        let a = self.audio_total();
        let v = self.video_end();
        if a > v + 1e-9 {
            self.extend_video_to(a);
            self.trim_fades_to(a);
            self.playhead = self.playhead.clamp(0.0, a);
            self.clamp_wipe_spans();
        } else {
            self.sync_tracks_to_shortest();
        }
    }

    pub fn covering_clip(&self, t: f64) -> Option<&VideoClip> {
        self.video_clips
            .iter()
            .find(|c| t >= c.start && t < c.end)
            .or_else(|| {
                self.video_clips
                    .last()
                    .filter(|c| (t - c.end).abs() < 1e-6)
            })
    }

    pub fn covering_clip_index(&self, t: f64) -> Option<usize> {
        self.video_clips.iter().position(|c| t >= c.start && t < c.end)
    }

    pub fn covering_fade(&self, t: f64) -> Option<&FadeSpan> {
        self.fades.iter().find(|f| t >= f.start && t < f.end)
    }

    /// 刷入转场当前画面: `(上一页 gid, 下一页 gid, 0..=1 进度)`.
    /// 进度 0 仍是整张上一页, 1 已完全盖住.
    pub fn wipe_at(&self, t: f64) -> Option<(&str, &str, f64)> {
        let fade = self.covering_fade(t).filter(|f| f.kind.is_wipe())?;
        let idx = self.covering_clip_index(t)?;
        if idx == 0 {
            return None;
        }
        let span = (fade.end - fade.start).max(1e-6);
        let p = ((t - fade.start) / span).clamp(0.0, 1.0);
        Some((
            self.video_clips[idx - 1].group_id.as_str(),
            self.video_clips[idx].group_id.as_str(),
            p,
        ))
    }

    /// 播放预览刷新间隔: 刷入硬边用 ~120Hz, 淡入淡出用 ~60Hz, 静止页保持 33ms.
    pub fn playback_tick_ms(&self, t: f64) -> u64 {
        let mut tick = PLAY_TICK_MS;
        for f in &self.fades {
            let in_span = t >= f.start && t < f.end;
            if f.kind.is_wipe() {
                let soon = t < f.start && f.start - t <= WIPE_TICK_LOOKAHEAD;
                if in_span || soon {
                    return WIPE_TICK_MS;
                }
            } else if in_span {
                tick = tick.min(FADE_TICK_MS);
            }
        }
        tick
    }

    /// 「一键在当前时刻插入下一张组合」: 素材池按顺序推进的核心逻辑.
    ///
    /// - 时间轴为空: 用素材池第 1 张, 新建 `[playhead, timeline_end)`.
    /// - 播放头落在末段之后: 先把末段延伸到新的 `timeline_end`, 再按下条规则处理.
    /// - 否则: 找到覆盖播放头的片段, 截断其终点为播放头, 在其后插入
    ///   「素材池中该片段素材的下一个」, 时长顺延到旧终点.
    pub fn insert_next(&mut self, pool: &[MaterialItem]) -> Result<(), String> {
        if pool.is_empty() {
            return Err("素材池为空, 请先在右侧生成输出组合".to_string());
        }
        let t = self.playhead.max(0.0);
        if self.video_clips.is_empty() {
            let end = self.timeline_end().max(t + MIN_CLIP_DUR);
            self.video_clips.push(VideoClip {
                id: Uuid::new_v4(),
                group_id: pool[0].group_id.clone(),
                start: t,
                end,
            });
            return Ok(());
        }
        let last_idx = self.video_clips.len() - 1;
        let last_end = self.video_clips[last_idx].end;
        if t >= last_end {
            let new_end = self.timeline_end().max(t + MIN_CLIP_DUR);
            self.video_clips[last_idx].end = new_end;
        }
        let idx = self
            .covering_clip_index(t)
            .ok_or_else(|| "播放头不在任何素材片段内".to_string())?;
        if t - self.video_clips[idx].start < MIN_CLIP_DUR {
            return Err("距该片段起点太近, 无法在此插入".to_string());
        }
        let cur_group = self.video_clips[idx].group_id.clone();
        let pool_idx = pool
            .iter()
            .position(|m| m.group_id == cur_group)
            .ok_or_else(|| "当前片段素材已不在素材池中".to_string())?;
        let next_item = pool
            .get(pool_idx + 1)
            .ok_or_else(|| "素材池已用完, 没有下一张组合了".to_string())?;
        let old_end = self.video_clips[idx].end;
        self.video_clips[idx].end = t;
        self.video_clips.insert(
            idx + 1,
            VideoClip {
                id: Uuid::new_v4(),
                group_id: next_item.group_id.clone(),
                start: t,
                end: old_end,
            },
        );
        self.clamp_wipe_spans();
        Ok(())
    }

    /// 在播放头切到下一张组合, 并在翻页边界挂上默认 1 秒的左→右刷入转场.
    /// 刷入不能落在第一页之前, 也不能和已有淡入淡出叠置.
    pub fn insert_next_with_wipe(&mut self, pool: &[MaterialItem]) -> Result<(), String> {
        if pool.is_empty() {
            return Err("素材池为空, 请先在右侧生成输出组合".to_string());
        }
        if self.video_clips.is_empty() {
            return Err("还没有上一页, 请先插入第一张组合".to_string());
        }
        let t = self.playhead.max(0.0);
        if self.covering_fade(t).is_some_and(|f| !f.kind.is_wipe()) {
            return Err("此处已有淡入淡出, 不能叠置刷入转场".to_string());
        }
        let last_idx = self.video_clips.len() - 1;
        let last_end = self.video_clips[last_idx].end;
        let page_end = if t >= last_end {
            self.timeline_end().max(t + MIN_CLIP_DUR)
        } else {
            let idx = self
                .covering_clip_index(t)
                .ok_or_else(|| "播放头不在任何素材片段内".to_string())?;
            if t - self.video_clips[idx].start < MIN_CLIP_DUR {
                return Err("距该片段起点太近, 无法在此插入".to_string());
            }
            self.video_clips[idx].end
        };
        let covering_wipe = self
            .covering_fade(t)
            .filter(|f| f.kind.is_wipe())
            .map(|f| f.id);
        let wipe_end = self
            .wipe_end_for(t, page_end, covering_wipe)
            .ok_or_else(|| "翻页后剩余时间太短, 或与已有转场冲突, 无法放下刷入".to_string())?;
        self.insert_next(pool)?;
        self.truncate_wipe_before(t);
        self.push_wipe_span(t, wipe_end);
        self.clamp_wipe_spans();
        Ok(())
    }

    /// 素材池拖拽落到轨道上时使用: 直接指定素材, 截断逻辑与 `insert_next` 共用.
    pub fn insert_at(&mut self, t: f64, group_id: String) {
        let t = t.max(0.0);
        if self.video_clips.is_empty() {
            let end = self.timeline_end().max(t + MIN_CLIP_DUR);
            self.video_clips.push(VideoClip {
                id: Uuid::new_v4(),
                group_id,
                start: t,
                end,
            });
            return;
        }
        let last_idx = self.video_clips.len() - 1;
        let last_end = self.video_clips[last_idx].end;
        if t >= last_end {
            let end = self.timeline_end().max(t + MIN_CLIP_DUR);
            self.video_clips.push(VideoClip {
                id: Uuid::new_v4(),
                group_id,
                start: last_end,
                end,
            });
            return;
        }
        if let Some(idx) = self.covering_clip_index(t) {
            if t - self.video_clips[idx].start < MIN_CLIP_DUR {
                // 落点太靠近该片段起点: 直接替换素材, 不产生零宽片段.
                self.video_clips[idx].group_id = group_id;
                return;
            }
            let old_end = self.video_clips[idx].end;
            self.video_clips[idx].end = t;
            self.video_clips.insert(
                idx + 1,
                VideoClip {
                    id: Uuid::new_v4(),
                    group_id,
                    start: t,
                    end: old_end,
                },
            );
        }
        self.clamp_wipe_spans();
    }

    fn clip_idx(&self, id: Uuid) -> Option<usize> {
        self.video_clips.iter().position(|c| c.id == id)
    }

    /// 拖动片段左边界 (同步上一片段的右边界).
    pub fn trim_left(&mut self, id: Uuid, new_start: f64) {
        let Some(idx) = self.clip_idx(id) else { return };
        let old_start = self.video_clips[idx].start;
        let min = if idx == 0 {
            0.0
        } else {
            self.video_clips[idx - 1].start + MIN_CLIP_DUR
        };
        let max = self.video_clips[idx].end - MIN_CLIP_DUR;
        let ns = new_start.clamp(min, max.max(min));
        self.video_clips[idx].start = ns;
        if idx > 0 {
            self.video_clips[idx - 1].end = ns;
        }
        self.retarget_wipe_anchor(old_start, ns);
        self.sync_tracks_to_shortest();
    }

    /// 拖动片段右边界 (同步下一片段的左边界).
    /// 末段有音频时右边缘钉在音频末尾: 往右拖等于把左边界右移, 块变短.
    pub fn trim_right(&mut self, id: Uuid, new_end: f64) {
        let Some(idx) = self.clip_idx(id) else { return };
        let is_last = idx + 1 >= self.video_clips.len();
        if is_last && !self.audio_clips.is_empty() {
            let delta = new_end - self.video_clips[idx].end;
            self.slide_last_pinned_to_audio(idx, self.video_clips[idx].start + delta);
            self.sync_tracks_to_shortest();
            return;
        }
        let old_end = self.video_clips[idx].end;
        let min = self.video_clips[idx].start + MIN_CLIP_DUR;
        let max = if is_last {
            f64::MAX
        } else {
            self.video_clips[idx + 1].end - MIN_CLIP_DUR
        };
        let ne = new_end.clamp(min, max.max(min));
        self.video_clips[idx].end = ne;
        if !is_last {
            self.video_clips[idx + 1].start = ne;
        }
        self.retarget_wipe_anchor(old_end, ne);
        self.sync_tracks_to_shortest();
    }

    /// 整体拖动片段 (两侧边界同步偏移相同的量, 不产生缝隙/重叠).
    /// 末段有音频时右边缘钉在音频末尾, 只移动左边界 (往右拖则块变短).
    pub fn drag_body(&mut self, id: Uuid, delta: f64) {
        let Some(idx) = self.clip_idx(id) else { return };
        let min_start = if idx == 0 {
            0.0
        } else {
            self.video_clips[idx - 1].start + MIN_CLIP_DUR
        };
        let is_last = idx + 1 >= self.video_clips.len();
        if is_last && !self.audio_clips.is_empty() {
            self.slide_last_pinned_to_audio(idx, self.video_clips[idx].start + delta);
            self.sync_tracks_to_shortest();
            return;
        }
        let old_start = self.video_clips[idx].start;
        let old_end = self.video_clips[idx].end;
        let max_end = if is_last {
            f64::MAX
        } else {
            self.video_clips[idx + 1].end - MIN_CLIP_DUR
        };
        let dur = old_end - old_start;
        let mut new_start = old_start + delta;
        new_start = new_start.clamp(min_start, (max_end - dur).max(min_start));
        let new_end = new_start + dur;
        self.video_clips[idx].start = new_start;
        self.video_clips[idx].end = new_end;
        if idx > 0 {
            self.video_clips[idx - 1].end = new_start;
        }
        if !is_last {
            self.video_clips[idx + 1].start = new_end;
        }
        self.retarget_wipe_anchor(old_start, new_start);
        self.retarget_wipe_anchor(old_end, new_end);
        self.sync_tracks_to_shortest();
    }

    /// 末段右边缘钉在音频末尾, 只改起点 (并同步上一段终点).
    fn slide_last_pinned_to_audio(&mut self, idx: usize, new_start: f64) {
        let end = self.audio_total();
        let min_start = if idx == 0 {
            0.0
        } else {
            self.video_clips[idx - 1].start + MIN_CLIP_DUR
        };
        let max_start = (end - MIN_CLIP_DUR).max(min_start);
        let old_start = self.video_clips[idx].start;
        let ns = new_start.clamp(min_start, max_start);
        self.video_clips[idx].start = ns;
        self.video_clips[idx].end = end.max(ns + MIN_CLIP_DUR);
        if idx > 0 {
            self.video_clips[idx - 1].end = ns;
        }
        self.retarget_wipe_anchor(old_start, ns);
    }

    /// 标记淡入/淡出: 若已有鼠标拖选区间, 直接生成; 否则两次按键各标一端.
    pub fn mark_fade(&mut self, kind: FadeKind, t: f64) {
        if let Some((a, b)) = self.fade_selection.take() {
            self.push_fade_span(a, b, kind);
            return;
        }
        match self.pending_fade_anchor.take() {
            None => self.pending_fade_anchor = Some(t),
            Some(anchor) => self.push_fade_span(anchor, t, kind),
        }
    }

    fn push_fade_span(&mut self, a: f64, b: f64, kind: FadeKind) {
        if kind.is_wipe() {
            return;
        }
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if end - start < MIN_CLIP_DUR {
            return;
        }
        if self.interval_overlaps_fades(start, end, None) {
            return;
        }
        self.fades.push(FadeSpan {
            id: Uuid::new_v4(),
            start,
            end,
            kind,
            keep_bg: false,
        });
        self.sort_fades();
    }

    /// 删除当前选中的片段/淡入淡出区间 (轨道上的删除快捷键).
    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selected_clip.take() {
            if let Some(idx) = self.clip_idx(id) {
                if idx == 0 {
                    self.video_clips.remove(0);
                    if let Some(first) = self.video_clips.first_mut() {
                        first.start = 0.0;
                    }
                } else {
                    let end = self.video_clips[idx].end;
                    self.video_clips.remove(idx);
                    if idx - 1 < self.video_clips.len() {
                        self.video_clips[idx - 1].end = end;
                    }
                }
            }
        }
        let fade_ids = self.selected_fade_ids();
        if !fade_ids.is_empty() {
            self.fades.retain(|f| !fade_ids.contains(&f.id));
            self.clear_fade_selection();
        }
        if let Some(id) = self.selected_audio.take() {
            self.audio_clips.retain(|c| c.id != id);
        }
        // 删短任一轨后, 边界与较长轨一起对齐到最短轨末端.
        self.sync_tracks_to_shortest();
    }

    pub fn select_clip_at(&mut self, t: f64) {
        self.selected_clip = self.covering_clip(t).map(|c| c.id);
        self.clear_fade_selection();
        self.selected_audio = None;
    }

    pub fn select_fade_at(&mut self, t: f64) {
        self.selected_clip = None;
        self.selected_audio = None;
        match self.covering_fade(t).map(|f| f.id) {
            Some(id) => self.select_fade(id, false),
            None => self.clear_fade_selection(),
        }
    }

    pub fn fade_is_selected(&self, id: Uuid) -> bool {
        self.selected_fades.contains(&id)
    }

    pub fn clear_fade_selection(&mut self) {
        self.selected_fade = None;
        self.selected_fades.clear();
    }

    /// `additive` 为 Ctrl 多选: 已在集合里则去掉, 否则加入.
    pub fn select_fade(&mut self, id: Uuid, additive: bool) {
        self.selected_clip = None;
        self.selected_audio = None;
        self.fade_selection = None;
        if additive {
            if self.selected_fades.contains(&id) {
                self.selected_fades.remove(&id);
                if self.selected_fade == Some(id) {
                    self.selected_fade = self.selected_fades.iter().copied().next();
                }
            } else {
                self.selected_fades.insert(id);
                self.selected_fade = Some(id);
            }
        } else {
            self.selected_fades.clear();
            self.selected_fades.insert(id);
            self.selected_fade = Some(id);
        }
    }

    pub fn selected_fade_ids(&self) -> HashSet<Uuid> {
        let mut ids = self.selected_fades.clone();
        if let Some(id) = self.selected_fade {
            ids.insert(id);
        }
        ids
    }

    /// 选中项全部已开则关, 否则全开 (含混合状态).
    pub fn toggle_keep_bg_on_selected(&mut self) -> bool {
        let ids = self.selected_fade_ids();
        if ids.is_empty() {
            return false;
        }
        let all_on = self
            .fades
            .iter()
            .filter(|f| ids.contains(&f.id) && !f.kind.is_wipe())
            .all(|f| f.keep_bg);
        let keep = !all_on;
        let mut any = false;
        for f in &mut self.fades {
            if ids.contains(&f.id) && !f.kind.is_wipe() {
                f.keep_bg = keep;
                any = true;
            }
        }
        any && keep
    }

    pub fn selected_keep_bg(&self) -> bool {
        let ids = self.selected_fade_ids();
        !ids.is_empty()
            && self
                .fades
                .iter()
                .filter(|f| ids.contains(&f.id) && !f.kind.is_wipe())
                .all(|f| f.keep_bg)
            && self
                .fades
                .iter()
                .any(|f| ids.contains(&f.id) && !f.kind.is_wipe())
    }

    pub fn selected_fades_are_all_wipes(&self) -> bool {
        let ids = self.selected_fade_ids();
        !ids.is_empty()
            && self
                .fades
                .iter()
                .filter(|f| ids.contains(&f.id))
                .all(|f| f.kind.is_wipe())
    }

    pub fn remove_audio(&mut self, id: Uuid) {
        self.audio_clips.retain(|c| c.id != id);
        self.sync_tracks_to_shortest();
    }

    pub fn move_audio(&mut self, from: usize, to: usize) {
        if from >= self.audio_clips.len() || to >= self.audio_clips.len() || from == to {
            return;
        }
        let item = self.audio_clips.remove(from);
        self.audio_clips.insert(to.min(self.audio_clips.len()), item);
    }

    /// 按鼠标当前落点时刻重新排序音频片段 (轨道内直接拖动排序, 而非上下移按钮):
    /// 先取出被拖拽的片段, 再按剩余片段的累计时长比较落点属于哪个槽位并插回.
    pub fn reorder_audio_by_time(&mut self, id: Uuid, t: f64) {
        let Some(from) = self.audio_clips.iter().position(|c| c.id == id) else {
            return;
        };
        let dragged = self.audio_clips.remove(from);
        let mut cum = 0.0f64;
        let mut to = self.audio_clips.len();
        for (i, c) in self.audio_clips.iter().enumerate() {
            let mid = cum + c.duration * 0.5;
            if t < mid {
                to = i;
                break;
            }
            cum += c.duration;
        }
        self.audio_clips.insert(to, dragged);
    }

    /// 「分割音频」: 在时间轴时刻 `t` 处把覆盖该时刻的音频片段切成两段
    /// (同一源文件, 后段 `offset` 顺延), 离两端太近 (< `MIN_CLIP_DUR`) 时
    /// 视为无效分割点, 不做任何改动. 返回是否真的切开了.
    /// 切开后两段标签自动改为 `{原名}-1` / `{原名}-2` 以便区分.
    pub fn split_audio_at(&mut self, t: f64) -> bool {
        let mut cum = 0.0f64;
        for i in 0..self.audio_clips.len() {
            let dur = self.audio_clips[i].duration;
            let local = t - cum;
            if local > MIN_CLIP_DUR && local < dur - MIN_CLIP_DUR {
                let orig = self.audio_clips[i].clone();
                let base = orig.label.to_string();
                self.audio_clips[i].duration = local;
                self.audio_clips[i].label = format!("{base}-1").into();
                let second = AudioClip {
                    id: Uuid::new_v4(),
                    path: orig.path,
                    label: format!("{base}-2").into(),
                    duration: dur - local,
                    offset: orig.offset + local,
                };
                self.audio_clips.insert(i + 1, second);
                return true;
            }
            cum += dur;
        }
        false
    }

    fn fade_idx(&self, id: Uuid) -> Option<usize> {
        self.fades.iter().position(|f| f.id == id)
    }

    fn sort_fades(&mut self) {
        self.fades
            .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    }

    fn interval_overlaps_fades(&self, start: f64, end: f64, except: Option<Uuid>) -> bool {
        self.fades.iter().any(|f| {
            Some(f.id) != except && f.start < end - 1e-9 && start < f.end - 1e-9
        })
    }

    /// 刷入终点: 默认 1 秒, 不超过本页终点, 也不越过后面的转场.
    fn wipe_end_for(&self, start: f64, page_end: f64, ignore: Option<Uuid>) -> Option<f64> {
        let mut end = (start + DEFAULT_WIPE_DUR).min(page_end);
        for f in &self.fades {
            if Some(f.id) == ignore {
                continue;
            }
            if f.start >= start - 1e-9 {
                end = end.min(f.start);
            } else if f.end > start + 1e-9 {
                return None;
            }
        }
        if end - start < MIN_CLIP_DUR {
            None
        } else {
            Some(end)
        }
    }

    fn push_wipe_span(&mut self, start: f64, end: f64) {
        if end - start < MIN_CLIP_DUR {
            return;
        }
        if self.interval_overlaps_fades(start, end, None) {
            return;
        }
        self.fades.push(FadeSpan {
            id: Uuid::new_v4(),
            start,
            end,
            kind: FadeKind::WipeLtr,
            keep_bg: false,
        });
        self.sort_fades();
    }

    /// 新翻页落在既有刷入中间时, 把旧刷入截到翻页点 (相邻可以, 叠置不行).
    fn truncate_wipe_before(&mut self, t: f64) {
        let mut drop_ids = Vec::new();
        for f in &mut self.fades {
            if !f.kind.is_wipe() {
                continue;
            }
            if f.start < t - 1e-9 && f.end > t + 1e-9 {
                f.end = t;
                if f.end - f.start < MIN_CLIP_DUR {
                    drop_ids.push(f.id);
                }
            }
        }
        if !drop_ids.is_empty() {
            self.fades.retain(|f| !drop_ids.contains(&f.id));
        }
    }

    fn retarget_wipe_anchor(&mut self, old_t: f64, new_t: f64) {
        if (old_t - new_t).abs() < 1e-12 {
            return;
        }
        for f in &mut self.fades {
            if f.kind.is_wipe() && (f.start - old_t).abs() < 1e-6 {
                let dur = f.end - f.start;
                f.start = new_t;
                f.end = new_t + dur;
            }
        }
    }

    /// 刷入必须钉在某一页 (非首页) 的起点; 超出本页或叠上别的转场则缩短/丢掉.
    fn clamp_wipe_spans(&mut self) {
        let anchors: Vec<(f64, f64)> = self
            .video_clips
            .iter()
            .skip(1)
            .map(|c| (c.start, c.end))
            .collect();
        let others: Vec<(Uuid, f64, f64)> = self
            .fades
            .iter()
            .map(|f| (f.id, f.start, f.end))
            .collect();
        let mut keep = Vec::with_capacity(self.fades.len());
        for mut f in self.fades.drain(..) {
            if !f.kind.is_wipe() {
                keep.push(f);
                continue;
            }
            let Some(&(start, page_end)) = anchors
                .iter()
                .find(|(s, _)| (f.start - *s).abs() < 1e-4)
            else {
                continue;
            };
            f.start = start;
            let mut end = f.end.min(page_end);
            let mut blocked = false;
            for (id, os, oe) in &others {
                if *id == f.id {
                    continue;
                }
                if *os >= start - 1e-9 {
                    end = end.min(*os);
                } else if *oe > start + 1e-9 {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                continue;
            }
            f.end = end;
            if f.end - f.start >= MIN_CLIP_DUR - 1e-9 {
                keep.push(f);
            }
        }
        self.fades = keep;
        self.sort_fades();
    }

    /// 拖动淡入淡出左边界 (不越过前一个淡入淡出的终点).
    /// 刷入的左边界就是翻页点: 拖它等于拖下一页的左边缘.
    pub fn trim_fade_left(&mut self, id: Uuid, new_start: f64) {
        let Some(idx) = self.fade_idx(id) else { return };
        if self.fades[idx].kind.is_wipe() {
            let start = self.fades[idx].start;
            if let Some(clip) = self
                .video_clips
                .iter()
                .find(|c| (c.start - start).abs() < 1e-6)
            {
                let cid = clip.id;
                self.trim_left(cid, new_start);
            }
            return;
        }
        let min = if idx == 0 { 0.0 } else { self.fades[idx - 1].end };
        let max = self.fades[idx].end - MIN_CLIP_DUR;
        self.fades[idx].start = new_start.clamp(min, max.max(min));
    }

    /// 拖动淡入淡出右边界 (不越过下一个淡入淡出的起点).
    /// 刷入右边界只改时长, 起点钉在翻页点, 也不能刷出本页.
    pub fn trim_fade_right(&mut self, id: Uuid, new_end: f64) {
        let Some(idx) = self.fade_idx(id) else { return };
        let min = self.fades[idx].start + MIN_CLIP_DUR;
        let mut max = if idx + 1 < self.fades.len() {
            self.fades[idx + 1].start
        } else {
            f64::MAX
        };
        if self.fades[idx].kind.is_wipe() {
            let start = self.fades[idx].start;
            if let Some(clip) = self
                .video_clips
                .iter()
                .find(|c| (c.start - start).abs() < 1e-6)
            {
                max = max.min(clip.end);
            }
        }
        self.fades[idx].end = new_end.clamp(min, max.max(min));
    }

    /// 整体拖动淡入淡出区间 (保持时长, 不越过相邻淡入淡出).
    /// 刷入不能离开翻页边界, 中间拖动无效.
    pub fn drag_fade_body(&mut self, id: Uuid, delta: f64) {
        let Some(idx) = self.fade_idx(id) else { return };
        if self.fades[idx].kind.is_wipe() {
            return;
        }
        let min_start = if idx == 0 { 0.0 } else { self.fades[idx - 1].end };
        let max_end = if idx + 1 < self.fades.len() {
            self.fades[idx + 1].start
        } else {
            f64::MAX
        };
        let dur = self.fades[idx].end - self.fades[idx].start;
        let mut new_start = self.fades[idx].start + delta;
        new_start = new_start.clamp(min_start, (max_end - dur).max(min_start));
        self.fades[idx].start = new_start;
        self.fades[idx].end = new_start + dur;
    }

    /// 导出为不含 UI 交互态 (选中/待定锚点等) 的纯数据快照, 供宿主写入工程
    /// 文件; `AudioClip.path` 按原样保留 (工程内不重新打包音频, 与工程文件
    /// 放在一起才能正常回放/导出).
    pub fn snapshot(&self) -> TimelineSnapshot {
        TimelineSnapshot {
            video_clips: self
                .video_clips
                .iter()
                .map(|c| (c.group_id.clone(), c.start, c.end))
                .collect(),
            fades: self
                .fades
                .iter()
                .map(|f| (f.start, f.end, f.kind, f.keep_bg))
                .collect(),
            audio_clips: self
                .audio_clips
                .iter()
                .map(|c| (c.path.clone(), c.label.to_string(), c.duration, c.offset))
                .collect(),
            playhead: self.playhead,
        }
    }

    /// 从快照恢复时间轴 (载入工程时用); 重新生成各条目的 id, 并清空选中态.
    pub fn load_snapshot(&mut self, snap: TimelineSnapshot) {
        self.video_clips = snap
            .video_clips
            .into_iter()
            .map(|(group_id, start, end)| VideoClip {
                id: Uuid::new_v4(),
                group_id,
                start,
                end,
            })
            .collect();
        self.fades = snap
            .fades
            .into_iter()
            .map(|(start, end, kind, keep_bg)| FadeSpan {
                id: Uuid::new_v4(),
                start,
                end,
                kind,
                keep_bg,
            })
            .collect();
        self.audio_clips = snap
            .audio_clips
            .into_iter()
            .map(|(path, label, duration, offset)| AudioClip {
                id: Uuid::new_v4(),
                path,
                label: label.into(),
                duration,
                offset,
            })
            .collect();
        self.playhead = snap.playhead;
        self.selected_clip = None;
        self.clear_fade_selection();
        self.selected_audio = None;
        self.pending_fade_anchor = None;
        self.fade_selection = None;
    }
}

/// 时间轴的纯数据快照 (无 `Uuid`/`SharedString`/UI 交互态), 供宿主
/// (score_sync) 序列化进工程文件, 不需要给这个 crate 引入 `serde`.
#[derive(Clone, Default, PartialEq)]
pub struct TimelineSnapshot {
    /// (group_id, start, end)
    pub video_clips: Vec<(String, f64, f64)>,
    /// (start, end, 转场种类, 是否保持底色)
    pub fades: Vec<(f64, f64, FadeKind, bool)>,
    /// (音频文件路径, 显示名, 时长秒, 在源文件里的起始偏移秒)
    pub audio_clips: Vec<(PathBuf, String, f64, f64)>,
    pub playhead: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> MaterialItem {
        MaterialItem {
            group_id: id.to_string(),
            label: id.to_string().into(),
            cache_path: PathBuf::from("missing.png"),
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn preview_path_sits_beside_png() {
        let item = MaterialItem {
            group_id: "g".into(),
            label: "g".into(),
            cache_path: PathBuf::from("pool/abc.png"),
            width: 1,
            height: 1,
        };
        assert_eq!(item.preview_path(), PathBuf::from("pool/abc.prev.jpg"));
    }

    #[test]
    fn insert_next_first_spans_whole_timeline() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        assert_eq!(tl.video_clips.len(), 1);
        assert_eq!(tl.video_clips[0].group_id, "a");
        assert_eq!(tl.video_clips[0].start, 0.0);
        assert!(tl.video_clips[0].end >= DEFAULT_TIMELINE_MIN);
    }

    #[test]
    fn insert_next_truncates_and_advances() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b"), item("c")];
        tl.insert_next(&pool).unwrap();
        let old_end = tl.video_clips[0].end;
        // 播放头落在首段内部 (首段默认 [0, DEFAULT_TIMELINE_MIN)): 截断+顺延.
        tl.playhead = 5.0;
        tl.insert_next(&pool).unwrap();
        assert_eq!(tl.video_clips.len(), 2);
        assert_eq!(tl.video_clips[0].group_id, "a");
        assert_eq!(tl.video_clips[0].end, 5.0);
        assert_eq!(tl.video_clips[1].group_id, "b");
        assert_eq!(tl.video_clips[1].start, 5.0);
        assert_eq!(tl.video_clips[1].end, old_end);
    }

    #[test]
    fn insert_next_errors_when_pool_exhausted() {
        let mut tl = Timeline::new();
        let pool = vec![item("a")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        let err = tl.insert_next(&pool);
        assert!(err.is_err());
    }

    #[test]
    fn insert_next_extends_past_last_end() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = tl.timeline_end() + 5.0;
        tl.insert_next(&pool).unwrap();
        assert_eq!(tl.video_clips.len(), 2);
        assert_eq!(tl.video_clips[1].group_id, "b");
        assert!(tl.video_clips[1].end > tl.video_clips[1].start);
    }

    #[test]
    fn trim_left_clamps_and_syncs_previous() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 20.0;
        tl.insert_next(&pool).unwrap();
        let second_id = tl.video_clips[1].id;
        tl.trim_left(second_id, 15.0);
        assert_eq!(tl.video_clips[0].end, 15.0);
        assert_eq!(tl.video_clips[1].start, 15.0);
        // 越过上一片段起点应被钳制
        tl.trim_left(second_id, -5.0);
        assert!(tl.video_clips[1].start >= tl.video_clips[0].start);
    }

    #[test]
    fn mark_fade_two_presses_creates_span() {
        let mut tl = Timeline::new();
        tl.mark_fade(FadeKind::Out, 5.0);
        assert_eq!(tl.pending_fade_anchor, Some(5.0));
        tl.mark_fade(FadeKind::Out, 8.0);
        assert!(tl.pending_fade_anchor.is_none());
        assert_eq!(tl.fades.len(), 1);
        assert_eq!(tl.fades[0].start, 5.0);
        assert_eq!(tl.fades[0].end, 8.0);
        assert_eq!(tl.fades[0].kind, FadeKind::Out);
    }

    #[test]
    fn mark_fade_drag_selection_creates_span_directly() {
        let mut tl = Timeline::new();
        tl.fade_selection = Some((10.0, 7.0));
        tl.mark_fade(FadeKind::In, 999.0);
        assert_eq!(tl.fades.len(), 1);
        assert_eq!(tl.fades[0].start, 7.0);
        assert_eq!(tl.fades[0].end, 10.0);
        assert_eq!(tl.fades[0].kind, FadeKind::In);
    }

    #[test]
    fn fade_trim_and_drag_respect_neighbors() {
        let mut tl = Timeline::new();
        tl.push_fade_span(2.0, 5.0, FadeKind::In);
        tl.push_fade_span(8.0, 10.0, FadeKind::Out);
        let id0 = tl.fades[0].id;
        let id1 = tl.fades[1].id;
        // 右边界不能越过下一个淡入淡出的起点.
        tl.trim_fade_right(id0, 20.0);
        assert_eq!(tl.fades[0].end, tl.fades[1].start);
        // 左边界不能越过上一个淡入淡出的终点.
        tl.trim_fade_left(id1, -5.0);
        assert_eq!(tl.fades[1].start, tl.fades[0].end);
        // 整体拖动保持时长, 且不越过相邻区间.
        let dur = tl.fades[1].end - tl.fades[1].start;
        tl.drag_fade_body(id1, 100.0);
        assert_eq!(tl.fades[1].end - tl.fades[1].start, dur);
        assert!(tl.fades[1].start >= tl.fades[0].end);
    }

    #[test]
    fn reorder_audio_by_time_moves_clip() {
        let mut tl = Timeline::new();
        let mk = |label: &str, dur: f64| AudioClip {
            id: Uuid::new_v4(),
            path: PathBuf::new(),
            label: label.to_string().into(),
            duration: dur,
            offset: 0.0,
        };
        let a = mk("a", 5.0);
        let b = mk("b", 5.0);
        let c = mk("c", 5.0);
        let a_id = a.id;
        tl.audio_clips = vec![a, b, c];
        // 把 a (原第 0 个) 拖到末尾 (落点时刻超过 b+c 的中点).
        tl.reorder_audio_by_time(a_id, 100.0);
        assert_eq!(tl.audio_clips.last().unwrap().id, a_id);
        assert_eq!(tl.audio_clips[0].label.to_string(), "b");
    }

    #[test]
    fn split_audio_at_creates_two_clips_with_offset() {
        let mut tl = Timeline::new();
        let a_id = Uuid::new_v4();
        tl.audio_clips = vec![
            AudioClip {
                id: a_id,
                path: PathBuf::from("a.wav"),
                label: "a".to_string().into(),
                duration: 10.0,
                offset: 0.0,
            },
            AudioClip {
                id: Uuid::new_v4(),
                path: PathBuf::from("b.wav"),
                label: "b".to_string().into(),
                duration: 5.0,
                offset: 0.0,
            },
        ];
        assert!(tl.split_audio_at(4.0));
        assert_eq!(tl.audio_clips.len(), 3);
        assert_eq!(tl.audio_clips[0].path, PathBuf::from("a.wav"));
        assert_eq!(tl.audio_clips[0].duration, 4.0);
        assert_eq!(tl.audio_clips[0].offset, 0.0);
        assert_eq!(tl.audio_clips[0].label.to_string(), "a-1");
        assert_eq!(tl.audio_clips[1].path, PathBuf::from("a.wav"));
        assert_eq!(tl.audio_clips[1].duration, 6.0);
        assert_eq!(tl.audio_clips[1].offset, 4.0);
        assert_eq!(tl.audio_clips[1].label.to_string(), "a-2");
        assert_eq!(tl.audio_clips[2].path, PathBuf::from("b.wav"));
        assert_eq!(tl.audio_clips[2].label.to_string(), "b");
        // 落点太靠近某段边界时不应该分割.
        assert!(!tl.split_audio_at(0.01));
        assert!(!tl.split_audio_at(1000.0));
        assert_eq!(tl.audio_clips.len(), 3);
    }

    #[test]
    fn snapshot_round_trip_preserves_data() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 10.0;
        tl.insert_next(&pool).unwrap();
        tl.fade_selection = Some((0.0, 3.0));
        tl.mark_fade(FadeKind::In, 3.0);
        tl.audio_clips.push(AudioClip {
            id: Uuid::new_v4(),
            path: PathBuf::from("a.wav"),
            label: "第一乐章".to_string().into(),
            duration: 12.5,
            offset: 0.0,
        });
        tl.playhead = 4.0;

        let snap = tl.snapshot();
        assert_eq!(snap.video_clips.len(), 2);
        assert_eq!(snap.fades.len(), 1);
        assert_eq!(snap.audio_clips.len(), 1);
        assert_eq!(snap.playhead, 4.0);

        let mut tl2 = Timeline::new();
        tl2.load_snapshot(snap);
        assert_eq!(tl2.video_clips.len(), 2);
        assert_eq!(tl2.video_clips[0].group_id, "a");
        assert_eq!(tl2.fades.len(), 1);
        assert!(!tl2.fades[0].keep_bg);
        assert_eq!(tl2.audio_clips[0].label.to_string(), "第一乐章");
        assert_eq!(tl2.audio_clips[0].path, PathBuf::from("a.wav"));
        assert_eq!(tl2.playhead, 4.0);
        assert!(tl2.selected_clip.is_none());
    }

    #[test]
    fn delete_selected_closes_gap() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b"), item("c")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 10.0;
        tl.insert_next(&pool).unwrap();
        tl.playhead = 20.0;
        tl.insert_next(&pool).unwrap();
        let mid_id = tl.video_clips[1].id;
        tl.selected_clip = Some(mid_id);
        tl.delete_selected();
        assert_eq!(tl.video_clips.len(), 2);
        assert_eq!(tl.video_clips[0].end, 20.0);
        assert_eq!(tl.video_clips[1].start, 20.0);
    }

    #[test]
    fn timeline_end_follows_shortest_track_and_sync_trims() {
        let mut tl = Timeline::new();
        let pool = vec![item("a")];
        tl.insert_next(&pool).unwrap();
        assert!((tl.timeline_end() - DEFAULT_TIMELINE_MIN).abs() < 1e-9);

        tl.audio_clips.push(AudioClip {
            id: Uuid::new_v4(),
            path: PathBuf::from("a.wav"),
            label: "a".into(),
            duration: 30.0,
            offset: 0.0,
        });
        tl.fit_after_audio_change();
        assert!((tl.video_end() - 30.0).abs() < 1e-9);
        assert!((tl.timeline_end() - 30.0).abs() < 1e-9);

        tl.audio_clips.push(AudioClip {
            id: Uuid::new_v4(),
            path: PathBuf::from("b.wav"),
            label: "b".into(),
            duration: 10.0,
            offset: 0.0,
        });
        // 总音频 40 > 视频 30: 导入侧会 extend; 这里模拟追加后 fit
        tl.fit_after_audio_change();
        assert!((tl.video_end() - 40.0).abs() < 1e-9);

        let drop_id = tl.audio_clips[1].id;
        tl.selected_audio = Some(drop_id);
        tl.delete_selected();
        assert!((tl.audio_total() - 30.0).abs() < 1e-9);
        assert!((tl.video_end() - 30.0).abs() < 1e-9);
        assert!((tl.timeline_end() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn last_clip_drag_right_shortens_and_stays_on_audio_end() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        tl.insert_next(&pool).unwrap();
        tl.audio_clips.push(AudioClip {
            id: Uuid::new_v4(),
            path: PathBuf::from("a.wav"),
            label: "a".into(),
            duration: 10.0,
            offset: 0.0,
        });
        tl.fit_after_audio_change();
        let last_id = tl.video_clips.last().unwrap().id;
        let audio_end = tl.audio_total();
        let old_start = tl.video_clips.last().unwrap().start;
        assert!((tl.video_end() - audio_end).abs() < 1e-9);
        tl.drag_body(last_id, 2.0);
        let last = tl.video_clips.last().unwrap();
        assert!((last.end - audio_end).abs() < 1e-6);
        assert!((last.start - (old_start + 2.0)).abs() < 1e-6);
        assert!((tl.audio_total() - audio_end).abs() < 1e-9);
        tl.trim_right(last_id, audio_end + 1.0);
        let last = tl.video_clips.last().unwrap();
        assert!((last.end - audio_end).abs() < 1e-6);
        assert!(last.start > old_start + 2.0);
    }

    #[test]
    fn keep_bg_toggles_all_selected_fades() {
        let mut tl = Timeline::new();
        tl.push_fade_span(0.0, 1.0, FadeKind::Out);
        tl.push_fade_span(2.0, 3.0, FadeKind::In);
        tl.push_fade_span(4.0, 5.0, FadeKind::Out);
        let a = tl.fades[0].id;
        let b = tl.fades[1].id;
        tl.select_fade(a, false);
        tl.select_fade(b, true);
        assert!(tl.toggle_keep_bg_on_selected());
        assert!(tl.fades[0].keep_bg);
        assert!(tl.fades[1].keep_bg);
        assert!(!tl.fades[2].keep_bg);
        assert!(!tl.toggle_keep_bg_on_selected());
        assert!(!tl.fades[0].keep_bg);
        assert!(!tl.fades[1].keep_bg);

        let snap = tl.snapshot();
        tl.fades[0].keep_bg = true;
        tl.load_snapshot(snap.clone());
        assert!(!tl.fades[0].keep_bg);

        tl.fades[0].keep_bg = true;
        let snap2 = tl.snapshot();
        assert!(snap2.fades[0].3);
        let mut tl3 = Timeline::new();
        tl3.load_snapshot(snap2);
        assert!(tl3.fades[0].keep_bg);
    }

    #[test]
    fn insert_next_with_wipe_pins_to_page_cut() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b"), item("c")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        tl.insert_next_with_wipe(&pool).unwrap();
        assert_eq!(tl.video_clips.len(), 2);
        assert_eq!(tl.video_clips[0].end, 5.0);
        assert_eq!(tl.video_clips[1].start, 5.0);
        assert_eq!(tl.video_clips[1].group_id, "b");
        assert_eq!(tl.fades.len(), 1);
        assert_eq!(tl.fades[0].kind, FadeKind::WipeLtr);
        assert!((tl.fades[0].start - 5.0).abs() < 1e-9);
        assert!((tl.fades[0].end - 6.0).abs() < 1e-9);
        let (prev, next, p) = tl.wipe_at(5.5).unwrap();
        assert_eq!(prev, "a");
        assert_eq!(next, "b");
        assert!((p - 0.5).abs() < 1e-9);
    }

    #[test]
    fn wipe_rejected_on_empty_or_fade_overlap() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        assert!(tl.insert_next_with_wipe(&pool).is_err());
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        tl.fade_selection = Some((4.5, 6.5));
        tl.mark_fade(FadeKind::Out, 0.0);
        assert_eq!(tl.fades.len(), 1);
        assert!(tl.insert_next_with_wipe(&pool).is_err());
        assert_eq!(tl.video_clips.len(), 1);
    }

    #[test]
    fn wipe_follows_page_boundary_and_right_trim() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        tl.insert_next_with_wipe(&pool).unwrap();
        let next_id = tl.video_clips[1].id;
        let wipe_id = tl.fades[0].id;
        tl.trim_left(next_id, 4.0);
        assert!((tl.video_clips[1].start - 4.0).abs() < 1e-9);
        assert!((tl.fades[0].start - 4.0).abs() < 1e-9);
        assert!((tl.fades[0].end - 5.0).abs() < 1e-9);
        tl.trim_fade_right(wipe_id, 7.0);
        assert!((tl.fades[0].start - 4.0).abs() < 1e-9);
        assert!((tl.fades[0].end - 7.0).abs() < 1e-9);
        let old_end = tl.fades[0].end;
        tl.drag_fade_body(wipe_id, 2.0);
        assert!((tl.fades[0].start - 4.0).abs() < 1e-9);
        assert_eq!(tl.fades[0].end, old_end);
    }

    #[test]
    fn fade_cannot_overlap_wipe() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        tl.insert_next_with_wipe(&pool).unwrap();
        tl.fade_selection = Some((5.2, 6.5));
        tl.mark_fade(FadeKind::In, 0.0);
        assert_eq!(tl.fades.len(), 1);
        assert_eq!(tl.fades[0].kind, FadeKind::WipeLtr);
    }

    #[test]
    fn wipe_snapshot_round_trip() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 3.0;
        tl.insert_next_with_wipe(&pool).unwrap();
        let snap = tl.snapshot();
        assert_eq!(snap.fades[0].2, FadeKind::WipeLtr);
        let mut tl2 = Timeline::new();
        tl2.load_snapshot(snap);
        assert_eq!(tl2.fades[0].kind, FadeKind::WipeLtr);
        assert!((tl2.fades[0].start - 3.0).abs() < 1e-9);
    }

    #[test]
    fn playback_tick_speeds_up_around_wipe() {
        let mut tl = Timeline::new();
        let pool = vec![item("a"), item("b")];
        tl.insert_next(&pool).unwrap();
        tl.playhead = 5.0;
        tl.insert_next_with_wipe(&pool).unwrap();
        assert_eq!(tl.playback_tick_ms(1.0), PLAY_TICK_MS);
        assert_eq!(tl.playback_tick_ms(4.9), WIPE_TICK_MS);
        assert_eq!(tl.playback_tick_ms(5.5), WIPE_TICK_MS);
        assert_eq!(tl.playback_tick_ms(7.0), PLAY_TICK_MS);
        tl.fade_selection = Some((8.0, 9.0));
        tl.mark_fade(FadeKind::Out, 0.0);
        assert_eq!(tl.playback_tick_ms(8.5), FADE_TICK_MS);
    }
}
