//! 素材池图像, 播放/寻址, 插入与淡入淡出, 撤重.

use super::*;

impl ScoreVideoApp {
    pub(super) fn push_undo(&mut self) {
        self.undo_stack.push(self.timeline.snapshot());
        if self.undo_stack.len() > VIDEO_HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(super) fn ensure_drag_undo(&mut self) {
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
        // 素材内容可能已变化但 group_id 不变, 因此整体清空缓存.
        self.render_cache.clear();
        self.image_hot.clear();
        self.image_lru.clear();
        self.pool = pool;
        if let Some(gid) = &self.expanded_pool {
            if !self.pool.iter().any(|m| &m.group_id == gid) {
                self.expanded_pool = None;
            }
        }
        cx.notify();
    }

    /// 从磁盘缓存加载全分辨率图并放入 LRU 热集.
    pub(super) fn full_rgba(&mut self, group_id: &str) -> Option<Arc<image::RgbaImage>> {
        if let Some(img) = self.image_hot.get(group_id) {
            if let Some(pos) = self.image_lru.iter().position(|k| k == group_id) {
                if let Some(k) = self.image_lru.remove(pos) {
                    self.image_lru.push_back(k);
                }
            }
            return Some(img.clone());
        }
        let item = self.pool.iter().find(|m| m.group_id == group_id)?;
        let rgba = item.load_rgba().ok()?;
        let arc = Arc::new(rgba);
        self.image_hot.insert(group_id.to_string(), arc.clone());
        self.image_lru.push_back(group_id.to_string());
        while self.image_lru.len() > self.image_lru_cap {
            if let Some(old) = self.image_lru.pop_front() {
                self.image_hot.remove(&old);
            }
        }
        Some(arc)
    }

    /// 仅在播放期间才启动的进度 ticker (每次开始播放时新建一个).
    /// 之前的实现从 `new()` 起就无条件永久轮询, 会在应用生命周期内持续
    /// 尝试借用 entity 上下文; 一旦此时用户触发了 `rfd` 原生文件对话框
    /// (其模态消息循环会重入宿主窗口消息处理), 就可能与该 ticker 的
    /// `this.update` 产生重入借用冲突, 触发 `RefCell already borrowed` 崩溃.
    /// 改为只在真正播放时才存在这个任务, 播放停止后自动退出, 从根本上
    /// 避免了文件对话框场景下的重入.
    pub(super) fn start_ticker(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn image_for(&mut self, group_id: &str) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.render_cache.get(group_id) {
            return Some(img.clone());
        }
        let rgba_src = self.full_rgba(group_id)?;
        // 谱面组合拼合 (+ 可能叠加的工程底色补边) 后经常是很高的整图; 预览限幅.
        const MAX_PREVIEW_DIM: u32 = 2048;
        let (w, h) = rgba_src.dimensions();
        let mut rgba = if w > MAX_PREVIEW_DIM || h > MAX_PREVIEW_DIM {
            let scale = (MAX_PREVIEW_DIM as f32 / w.max(h) as f32).min(1.0);
            let nw = ((w as f32 * scale).round() as u32).max(1);
            let nh = ((h as f32 * scale).round() as u32).max(1);
            image::imageops::resize(&*rgba_src, nw, nh, image::imageops::FilterType::Triangle)
        } else {
            (*rgba_src).clone()
        };
        // GPUI 的 `RenderImage` 内部按 BGRA 排布读取像素.
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let render = Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]));
        self.render_cache
            .insert(group_id.to_string(), render.clone());
        Some(render)
    }

    pub(super) fn x_to_time(&self, x: f32) -> f64 {
        let rel = x - f32::from(self.tracks_bounds.origin.x);
        (rel.max(0.0) / self.px_per_sec.max(0.01)) as f64 + self.track_scroll
    }

    /// 时间点吸附: 就近对齐到视频/淡入淡出/音频边界、播放头、0 与时间轴末尾.
    /// `exclude` 排除当前正在拖拽的片段自身, 避免粘在自己的边上.
    pub(super) fn snap_time(&self, t: f64, exclude: SnapExclude) -> f64 {
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
    pub(super) fn snap_body_delta(
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
            let missing: Vec<String> = self
                .timeline
                .audio_clips
                .iter()
                .filter(|c| !c.path.is_file())
                .map(|c| {
                    crate::error::Error::AudioMissing(c.path.clone()).to_string()
                })
                .collect();
            if !missing.is_empty() {
                self.show_error("无法播放音频", missing.join("\n"), cx);
            }
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
    pub(super) fn seek_from_preview_x(&mut self, x: f32, cx: &mut Context<Self>) {
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
}
