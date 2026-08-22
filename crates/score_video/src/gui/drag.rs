//! 窗口外/跨面板拖拽转发.

use super::*;

impl ScoreVideoApp {
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

    pub(super) fn point_in_left_panel(&self, x: f32, y: f32) -> bool {
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

    pub(super) fn apply_left_drag_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
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

    pub(super) fn end_left_drag(&mut self, x: f32, cx: &mut Context<Self>) {
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
    pub(super) fn apply_pool_scroll_drag(&mut self, mouse_y: f32, grab: f32, cx: &mut Context<Self>) {
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
}
