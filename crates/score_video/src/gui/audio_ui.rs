//! 导入音频, 分割, 开始拖片段.

use super::*;

impl ScoreVideoApp {
    pub(super) fn spawn_native_dialog<T, F, A>(cx: &mut Context<Self>, work: F, apply: A)
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        A: FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    {
        let (tx, rx) = async_channel::bounded::<T>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(work());
        });
        cx.spawn(async move |this, cx| {
            if let Ok(val) = rx.recv().await {
                this.update(cx, |view, cx| apply(view, val, cx)).ok();
            }
        })
        .detach();
    }

    pub fn import_audio(&mut self, cx: &mut Context<Self>) {
        Self::spawn_native_dialog(
            cx,
            || {
                rfd::FileDialog::new()
                    .add_filter(
                        "音频",
                        &["wav", "mp3", "flac", "ogg", "m4a", "aac", "m4b"],
                    )
                    .add_filter("M4A / AAC", &["m4a", "aac", "m4b"])
                    .pick_files()
            },
            |this, paths, cx| {
                let Some(paths) = paths else {
                    return;
                };
                this.add_audio_paths(paths, cx);
            },
        );
    }

    pub(super) fn add_audio_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut added = 0usize;
        let mut errors: Vec<String> = Vec::new();
        for p in paths {
            match crate::audio::probe_duration(&p) {
                Ok(dur) if dur > 0.001 => {
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
                Ok(_) => {
                    errors.push(crate::error::Error::audio_probe(&p, "时长为 0").to_string());
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        if added > 0 {
            self.timeline.fit_after_audio_change();
            self.audio.set_clips(self.timeline.audio_clips.clone());
            self.start_audio_preview_prep(cx);
        }
        if !errors.is_empty() {
            self.show_error("导入音频失败", errors.join("\n\n"), cx);
        }
        cx.notify();
    }

    /// m4a 等不能走 rodio 的格式, 后台转成临时 WAV 供预览/波形; 导入本身只读时长不解码.
    pub(super) fn start_audio_preview_prep(&mut self, cx: &mut Context<Self>) {
        let jobs: Vec<PathBuf> = self
            .timeline
            .audio_clips
            .iter()
            .map(|c| c.path.clone())
            .filter(|p| crate::audio::needs_ffmpeg_preview(p) && !crate::audio::preview_wav_ready(p))
            .collect();
        if jobs.is_empty() {
            return;
        }
        let n = jobs.len();
        let (tx, rx) = async_channel::unbounded::<(PathBuf, bool)>();
        std::thread::spawn(move || {
            for p in jobs {
                let ok = crate::audio::ensure_preview_wav(&p).is_some();
                let _ = tx.send_blocking((p, ok));
            }
        });
        cx.spawn(async move |this, cx| {
            let mut done = 0usize;
            let mut ok_n = 0usize;
            while let Ok((path, ok)) = rx.recv().await {
                done += 1;
                if ok {
                    ok_n += 1;
                }
                let d = done;
                let o = ok_n;
                this.update(cx, |view, cx| {
                    view.waveform_cache.remove(&path);
                    view.waveform_pending.remove(&path);
                    view.audio.set_clips(view.timeline.audio_clips.clone());
                    if d == n && o != n {
                        view.show_error(
                            "音频预览未全部就绪",
                            format!(
                                "已导入, 但 {o}/{n} 个文件转成预览波形失败.\n\
                                 导出仍使用原文件; 常见原因是 ffmpeg 不在程序目录, 或文件没有音轨."
                            ),
                            cx,
                        );
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// 「分割音频」按钮: 再次点击可取消待命; 否则进入待命, 等下一次鼠标
    /// 按下时在 `Render` 里加的全屏透明遮罩上判定落点 (见那边的说明).
    pub(super) fn toggle_split_audio_armed(&mut self, cx: &mut Context<Self>) {
        self.split_audio_armed = !self.split_audio_armed;
        cx.notify();
    }

    /// 待命状态下处理一次点击: 落在音频轨道内就从该时刻切开对应片段,
    /// 否则视为取消. 注意宿主走 `left_panel` 而不是 `Render`, 所以必须在
    /// 音频片段/音频轨道自身的 `on_mouse_down` 里直接调用, 不能依赖全屏遮罩.
    pub(super) fn handle_split_audio_click(&mut self, x: f32, cx: &mut Context<Self>) {
        self.split_audio_armed = false;
        let b = self.tracks_bounds;
        let left = f32::from(b.origin.x);
        let right = left + f32::from(b.size.width);
        // 不严格卡 y: 调用方已经保证点在音频轨道/片段上; 这里只校验 x 在
        // 轨道水平范围内, 避免缩放滚动后边界抖动导致误判取消.
        let in_track_x = x >= left - 2.0 && x <= right + 2.0;
        if !in_track_x {
            cx.notify();
            return;
        }
        let t = self.x_to_time(x);
        self.push_undo();
        if self.timeline.split_audio_at(t) {
            self.audio.set_clips(self.timeline.audio_clips.clone());
        } else {
            self.undo_stack.pop();
        }
        cx.notify();
    }

    /// 待命中点到非音频区域: 取消分割, 不开始别的拖拽.
    pub(super) fn cancel_split_audio_if_armed(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.split_audio_armed {
            return false;
        }
        self.split_audio_armed = false;
        cx.notify();
        true
    }

    pub(super) fn begin_clip_drag(&mut self, id: Uuid, mouse_x: f32, cx: &mut Context<Self>) {
        if self.cancel_split_audio_if_armed(cx) {
            return;
        }
        self.timeline.selected_clip = Some(id);
        self.timeline.clear_fade_selection();
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
    /// Ctrl 按下时只切换多选, 不开始拖动.
    pub(super) fn begin_fade_drag(
        &mut self,
        id: Uuid,
        mouse_x: f32,
        additive: bool,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_split_audio_if_armed(cx) {
            return;
        }
        self.timeline.select_fade(id, additive);
        self.drag_undo_pushed = false;
        if additive {
            cx.notify();
            return;
        }
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
    pub(super) fn begin_audio_drag(&mut self, id: Uuid, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.split_audio_armed {
            return;
        }
        let Some(from) = self.timeline.audio_clips.iter().position(|c| c.id == id) else {
            return;
        };
        self.timeline.selected_audio = Some(id);
        self.timeline.selected_clip = None;
        self.timeline.clear_fade_selection();
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

    pub(super) fn audio_reorder_slop_exceeded(dx: f32, dy: f32) -> bool {
        dx * dx + dy * dy >= AUDIO_REORDER_SLOP * AUDIO_REORDER_SLOP
    }

    /// 将「落在 anchor 之前/之后」换算成 remove 后再 insert 的下标.
    pub(super) fn reorder_to_index(from: usize, anchor: usize, after: bool) -> usize {
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
    pub(super) fn resolve_audio_drop(&self, from: usize, x: f32) -> (usize, Option<usize>, bool) {
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
}
