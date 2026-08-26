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
            cx.notify();
            return;
        };
        self.redo_stack.push(self.timeline.snapshot());
        self.timeline.load_snapshot(prev);
        self.audio.set_clips(self.timeline.audio_clips.clone());
        self.drag = None;
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
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
        cx.notify();
    }

    pub fn set_pool(&mut self, pool: Vec<MaterialItem>, cx: &mut Context<Self>) {
        // 素材内容可能已变化但 group_id 不变, 因此整体清空缓存; 正在后台
        // 解码的旧内容用 `pool_gen` 标记过期, 回来时会被丢弃 (见 `image_for`).
        self.pool_gen = self.pool_gen.wrapping_add(1);
        self.render_cache.clear();
        self.image_loading.clear();
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

    /// 素材缩略图 (预览窗当前帧 / 素材池展开预览用): 命中缓存直接返回;
    /// 否则后台线程做磁盘读取 + 解码 + 缩放 + 通道换序, 本帧先返回
    /// `None` (画布这一帧先空着), 解码完成后自行 `cx.notify()` 刷新.
    /// 落盘素材图可达四五千像素见方, `image::open`+`to_rgba8`+缩放单张
    /// 就要上百毫秒, 若在界面线程上做 (原实现如此), 播放头移到新素材、
    /// 切入视频面板等操作都会明显卡一拍.
    pub(super) fn image_for(&mut self, group_id: &str, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.render_cache.get(group_id) {
            return Some(img.clone());
        }
        if !self.image_loading.insert(group_id.to_string()) {
            return None; // 已经有一份后台任务在算这张, 别重复起线程
        }
        let Some(item) = self.pool.iter().find(|m| m.group_id == group_id).cloned() else {
            self.image_loading.remove(group_id);
            return None;
        };
        let gen = self.pool_gen;
        let gid = group_id.to_string();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let render = item.load_rgba().ok().map(|rgba| {
                // 谱面组合拼合 (+ 可能叠加的工程底色补边) 后经常是很高的整图; 预览限幅.
                const MAX_PREVIEW_DIM: u32 = 2048;
                let (w, h) = rgba.dimensions();
                let mut small = if w > MAX_PREVIEW_DIM || h > MAX_PREVIEW_DIM {
                    let scale = (MAX_PREVIEW_DIM as f32 / w.max(h) as f32).min(1.0);
                    let nw = ((w as f32 * scale).round() as u32).max(1);
                    let nh = ((h as f32 * scale).round() as u32).max(1);
                    image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle)
                } else {
                    rgba
                };
                // GPUI 的 `RenderImage` 内部按 BGRA 排布读取像素.
                for px in small.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                Arc::new(RenderImage::new(smallvec![Frame::new(small)]))
            });
            let _ = tx.send_blocking(render);
        });
        cx.spawn(async move |this, cx| {
            let render = rx.recv().await.ok().flatten();
            this.update(cx, |view, cx| {
                if view.pool_gen != gen {
                    view.image_loading.remove(&gid); // 素材池已整体刷新, 这份结果作废
                    return;
                }
                match render {
                    Some(render) => {
                        view.image_loading.remove(&gid);
                        view.render_cache.insert(gid, render);
                        cx.notify();
                    }
                    // 解码失败 (如缓存文件损坏): 不移出 `image_loading`, 避免
                    // 之后每帧都重新起一次注定失败的后台线程; 留到下次
                    // `set_pool` 整体刷新时才会重试.
                    None => {}
                }
            })
            .ok();
        })
        .detach();
        None
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

    pub(super) fn set_playback_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.audio.set_speed(speed);
        self.speed_menu_open = false;
        cx.notify();
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
            Ok(()) => {}
            Err(e) => {
                self.undo_stack.pop();
                self.show_error("无法插入下一张组合", e, cx);
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
