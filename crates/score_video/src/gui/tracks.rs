//! 三轨与底部缩放条.

use super::*;

impl ScoreVideoApp {
    pub(super) fn video_track_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn fade_track_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pps = self.px_per_sec;
        let scroll = self.track_scroll;
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
                    this.timeline.clear_fade_selection();
                    this.fade_menu = None;
                    this.timeline.fade_selection = Some((t, t));
                    this.drag = Some(VideoDrag::FadeSelect { anchor: t });
                    cx.notify();
                }),
            );
        for f in self.timeline.fades.clone() {
            let x = ((f.start - scroll) as f32) * pps;
            let w = ((f.end - f.start) as f32 * pps).max(2.0);
            let is_sel = self.timeline.fade_is_selected(f.id);
            let keep_bg = f.keep_bg;
            let label: SharedString = match (f.kind, keep_bg) {
                (FadeKind::In, false) => "淡入".into(),
                (FadeKind::Out, false) => "淡出".into(),
                (FadeKind::In, true) => "淡入·底".into(),
                (FadeKind::Out, true) => "淡出·底".into(),
            };
            // 保持底色: 更浅的填充 + 米色描边, 和淡到黑的块一眼能分开.
            let base_color = match (f.kind, keep_bg) {
                (FadeKind::In, false) => rgb(0x0d9488),
                (FadeKind::Out, false) => rgb(0xb45309),
                (FadeKind::In, true) => rgb(0x5eead4),
                (FadeKind::Out, true) => rgb(0xfbbf24),
            };
            let border = if is_sel {
                rgb(0xf8fafc)
            } else if keep_bg {
                rgb(0xfef3c7)
            } else {
                rgb(0x0f172a)
            };
            let text_color = if keep_bg {
                rgb(0x1e293b)
            } else {
                rgb(0xf1f5f9)
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
                    .border_color(border)
                    .rounded_sm()
                    .text_xs()
                    .text_color(text_color)
                    .px_1()
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.fade_menu = None;
                            let x = f32::from(ev.position.x);
                            this.begin_fade_drag(id, x, apply_bg::is_primary_mod(&ev.modifiers), cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_fade_menu(
                                id,
                                f32::from(ev.position.x),
                                f32::from(ev.position.y),
                                cx,
                            );
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
                                this.timeline.clear_fade_selection();
                                this.fade_menu = None;
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

    pub(super) fn open_fade_menu(&mut self, id: Uuid, x: f32, y: f32, cx: &mut Context<Self>) {
        if !self.timeline.fade_is_selected(id) {
            self.timeline.select_fade(id, false);
        }
        let ox = f32::from(self.left_bounds.origin.x);
        let oy = f32::from(self.left_bounds.origin.y);
        self.fade_menu = Some(FadeContextMenu {
            x: x - ox,
            y: y - oy,
        });
        cx.notify();
    }

    pub(super) fn fade_context_menu_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ref menu) = self.fade_menu else {
            return div().into_any_element();
        };
        let x = menu.x;
        let y = menu.y;
        let checked = self.timeline.selected_keep_bg();
        let label: SharedString = if checked {
            "✓  保持背景为底色".into()
        } else {
            "    保持背景为底色".into()
        };
        div()
            .id("sv-fade-ctx-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.fade_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.fade_menu = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("sv-fade-ctx-menu")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .min_w(px(180.))
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x1e293b))
                    .border_1()
                    .border_color(rgb(0x64748b))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id("sv-fade-keep-bg")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(0xf1f5f9))
                            .hover(|s| s.bg(rgb(0x334155)))
                            .child(label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.push_undo();
                                    let keep = this.timeline.toggle_keep_bg_on_selected();
                                    this.fade_menu = None;
                                    this.status = if keep {
                                        "已对选中淡入淡出保持底色 (只淡乐谱, 不淡到黑).".into()
                                    } else {
                                        "已恢复为淡到黑.".into()
                                    };
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn audio_track_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
    pub(super) fn min_px_per_sec(&self) -> f32 {
        let end = self.timeline.timeline_end().max(1.0) as f32;
        let width = f32::from(self.tracks_bounds.size.width).max(1.0);
        (width / end).max(0.01)
    }

    /// Ctrl+滚轮以鼠标所在时刻为锚点缩放轨道 (无上限, 下限为"全部轨道可见");
    /// 普通滚轮横向平移 (时间轴过长超出可视宽度时用).
    pub(super) fn on_tracks_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta_y = match event.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * 30.0,
        };
        if delta_y.abs() < 0.01 {
            return;
        }
        let min_pps = self.min_px_per_sec();
        if apply_bg::is_primary_mod(&event.modifiers) {
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
    pub(super) fn update_track_view(&mut self) {
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
    pub(super) fn visible_secs(&self) -> f64 {
        let width = f32::from(self.tracks_bounds.size.width).max(1.0);
        (width / self.px_per_sec.max(0.01)) as f64
    }

    /// 底部缩放条上某屏幕 x 坐标对应的时间轴时刻 (条上 0..宽度 线性映射到
    /// 0..时间轴总长, 与轨道区自身的 `x_to_time` 是两套不同的映射).
    pub(super) fn track_bar_x_to_time(&self, mouse_x: f32) -> f64 {
        let end = self.timeline.timeline_end().max(1.0);
        let origin_x = f32::from(self.track_bar_bounds.origin.x);
        let width = f32::from(self.track_bar_bounds.size.width).max(1.0);
        let frac = ((mouse_x - origin_x) / width).clamp(0.0, 1.0);
        frac as f64 * end
    }

    /// 拖动缩放条滑块本体: 平移可视窗口 (不改变缩放).
    pub(super) fn apply_track_bar_pan(&mut self, mouse_x: f32, grab: f32, cx: &mut Context<Self>) {
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
    pub(super) fn apply_track_bar_zoom_left(&mut self, mouse_x: f32, anchor_end_t: f64, cx: &mut Context<Self>) {
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
    pub(super) fn apply_track_bar_zoom_right(
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
    pub(super) fn waveform_for(&mut self, path: &PathBuf, cx: &mut Context<Self>) -> Option<Arc<Vec<f32>>> {
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

    pub(super) fn tracks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
    pub(super) fn track_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

}
