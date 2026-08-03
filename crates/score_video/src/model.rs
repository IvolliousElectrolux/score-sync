//! 视频时间轴数据模型: 素材 (静态谱面图) 片段 / 黑场淡入淡出 / 音频片段.
//!
//! 三条轨道共用同一条时间轴 (单位: 秒, f64):
//! - `video_clips`: 彼此首尾相接、按时间升序, 覆盖 `[0, video_end())`.
//! - `fades`: 互不重叠的黑场淡入/淡出区间, 可落在任意时刻.
//! - `audio_clips`: 顺序播放, 不单独存起点, 由前面片段时长累加得出.
//!
//! 时间轴总长取当前非空音/视频轨的较短末端; 删短一轨时会把较长轨裁齐.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::SharedString;
use image::RgbaImage;
use uuid::Uuid;

/// 素材池条目: 对应「输出组合」列表 (已按导出顺序排好) 中的一张最终合成图.
#[derive(Clone)]
pub struct MaterialItem {
    pub group_id: String,
    pub label: SharedString,
    pub image: Arc<RgbaImage>,
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
}

/// 黑场淡入淡出轨道上的一段 (幅度按时长线性变化).
#[derive(Clone)]
pub struct FadeSpan {
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    pub kind: FadeKind,
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

    /// 非空音/视频轨的末端时刻; 两轨都有内容时取较短者 (时间轴边界对齐最短轨).
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
            let last = self.audio_clips.last_mut().unwrap();
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
            return;
        };
        self.trim_video_to(target);
        self.trim_audio_to(target);
        self.trim_fades_to(target);
        self.playhead = self.playhead.clamp(0.0, target);
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
    }

    fn clip_idx(&self, id: Uuid) -> Option<usize> {
        self.video_clips.iter().position(|c| c.id == id)
    }

    /// 拖动片段左边界 (同步上一片段的右边界).
    pub fn trim_left(&mut self, id: Uuid, new_start: f64) {
        let Some(idx) = self.clip_idx(id) else { return };
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
        self.sync_tracks_to_shortest();
    }

    /// 拖动片段右边界 (同步下一片段的左边界).
    /// 末段不得超出音频轨末端 (有音频时), 以保证边界对齐最短轨.
    pub fn trim_right(&mut self, id: Uuid, new_end: f64) {
        let Some(idx) = self.clip_idx(id) else { return };
        let min = self.video_clips[idx].start + MIN_CLIP_DUR;
        let max = if idx + 1 < self.video_clips.len() {
            self.video_clips[idx + 1].end - MIN_CLIP_DUR
        } else if !self.audio_clips.is_empty() {
            self.audio_total()
        } else {
            f64::MAX
        };
        let ne = new_end.clamp(min, max.max(min));
        self.video_clips[idx].end = ne;
        if idx + 1 < self.video_clips.len() {
            self.video_clips[idx + 1].start = ne;
        }
        self.sync_tracks_to_shortest();
    }

    /// 整体拖动片段 (两侧边界同步偏移相同的量, 不产生缝隙/重叠).
    pub fn drag_body(&mut self, id: Uuid, delta: f64) {
        let Some(idx) = self.clip_idx(id) else { return };
        let min_start = if idx == 0 {
            0.0
        } else {
            self.video_clips[idx - 1].start + MIN_CLIP_DUR
        };
        let max_end = if idx + 1 < self.video_clips.len() {
            self.video_clips[idx + 1].end - MIN_CLIP_DUR
        } else if !self.audio_clips.is_empty() {
            self.audio_total()
        } else {
            f64::MAX
        };
        let dur = self.video_clips[idx].end - self.video_clips[idx].start;
        let mut new_start = self.video_clips[idx].start + delta;
        new_start = new_start.clamp(min_start, (max_end - dur).max(min_start));
        let new_end = new_start + dur;
        self.video_clips[idx].start = new_start;
        self.video_clips[idx].end = new_end;
        if idx > 0 {
            self.video_clips[idx - 1].end = new_start;
        }
        if idx + 1 < self.video_clips.len() {
            self.video_clips[idx + 1].start = new_end;
        }
        self.sync_tracks_to_shortest();
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
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if end - start < MIN_CLIP_DUR {
            return;
        }
        self.fades.push(FadeSpan {
            id: Uuid::new_v4(),
            start,
            end,
            kind,
        });
        self.fades
            .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
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
        if let Some(id) = self.selected_fade.take() {
            self.fades.retain(|f| f.id != id);
        }
        if let Some(id) = self.selected_audio.take() {
            self.audio_clips.retain(|c| c.id != id);
        }
        // 删短任一轨后, 边界与较长轨一起对齐到最短轨末端.
        self.sync_tracks_to_shortest();
    }

    pub fn select_clip_at(&mut self, t: f64) {
        self.selected_clip = self.covering_clip(t).map(|c| c.id);
        self.selected_fade = None;
        self.selected_audio = None;
    }

    pub fn select_fade_at(&mut self, t: f64) {
        self.selected_fade = self.covering_fade(t).map(|f| f.id);
        self.selected_clip = None;
        self.selected_audio = None;
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

    /// 拖动淡入淡出左边界 (不越过前一个淡入淡出的终点).
    pub fn trim_fade_left(&mut self, id: Uuid, new_start: f64) {
        let Some(idx) = self.fade_idx(id) else { return };
        let min = if idx == 0 { 0.0 } else { self.fades[idx - 1].end };
        let max = self.fades[idx].end - MIN_CLIP_DUR;
        self.fades[idx].start = new_start.clamp(min, max.max(min));
    }

    /// 拖动淡入淡出右边界 (不越过下一个淡入淡出的起点).
    pub fn trim_fade_right(&mut self, id: Uuid, new_end: f64) {
        let Some(idx) = self.fade_idx(id) else { return };
        let min = self.fades[idx].start + MIN_CLIP_DUR;
        let max = if idx + 1 < self.fades.len() {
            self.fades[idx + 1].start
        } else {
            f64::MAX
        };
        self.fades[idx].end = new_end.clamp(min, max.max(min));
    }

    /// 整体拖动淡入淡出区间 (保持时长, 不越过相邻淡入淡出).
    pub fn drag_fade_body(&mut self, id: Uuid, delta: f64) {
        let Some(idx) = self.fade_idx(id) else { return };
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
                .map(|f| (f.start, f.end, f.kind == FadeKind::In))
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
            .map(|(start, end, is_in)| FadeSpan {
                id: Uuid::new_v4(),
                start,
                end,
                kind: if is_in { FadeKind::In } else { FadeKind::Out },
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
        self.selected_fade = None;
        self.selected_audio = None;
        self.pending_fade_anchor = None;
        self.fade_selection = None;
    }
}

/// 时间轴的纯数据快照 (无 `Uuid`/`SharedString`/UI 交互态), 供宿主
/// (score_sync) 序列化进工程文件, 不需要给这个 crate 引入 `serde`.
#[derive(Clone, Default)]
pub struct TimelineSnapshot {
    /// (group_id, start, end)
    pub video_clips: Vec<(String, f64, f64)>,
    /// (start, end, 是否为淡入)
    pub fades: Vec<(f64, f64, bool)>,
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
            image: Arc::new(RgbaImage::new(1, 1)),
        }
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
}
