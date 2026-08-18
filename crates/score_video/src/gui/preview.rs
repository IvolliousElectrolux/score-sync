//! 预览窗, 播放条, 左侧工作区.

use super::*;

impl ScoreVideoApp {
    pub(super) fn btn(
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

    pub(super) fn transport_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn preview(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

}
