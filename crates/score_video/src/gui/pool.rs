//! 右侧素材池.

use super::*;

impl ScoreVideoApp {
    pub fn right_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("sv_pool_list")
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_scroll()
            .track_scroll(&self.pool_scroll)
            .scrollbar_width(px(0.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2();
        for item in self.pool.clone() {
            let gid = item.group_id.clone();
            let gid2 = gid.clone();
            let expanded = self.expanded_pool.as_deref() == Some(gid.as_str());
            let mut entry = div()
                .id(SharedString::from(format!("sv-pool-entry-{gid}")))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .id(SharedString::from(format!("sv-pool-{gid}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(expanded, |s| s.bg(rgb(0x334155)))
                        .when(!expanded, |s| s.bg(rgb(0x1e293b)))
                        .text_color(rgb(0xe2e8f0))
                        .text_xs()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0x334155)))
                        .child(item.label.clone())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                let x = f32::from(ev.position.x);
                                let y = f32::from(ev.position.y);
                                this.drag = Some(VideoDrag::PoolDrop {
                                    group_id: gid2.clone(),
                                    start_x: x,
                                    start_y: y,
                                    last_x: x,
                                    last_y: y,
                                });
                                cx.notify();
                            }),
                        ),
                );
            // 点击 (非拖动) 时向下展开该素材的图片预览; 手动加入时间轴请改为
            // 拖拽到左侧视频轨道上的具体位置.
            if expanded {
                let img = self.image_for(&gid, cx);
                entry = entry.child(
                    div()
                        .id(SharedString::from(format!("sv-pool-preview-{gid}")))
                        .w_full()
                        .h(px(160.))
                        .rounded_md()
                        .bg(rgb(0x020617))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    if let Some(img) = &img {
                                        let sz = img.size(0);
                                        let iw = (sz.width.0 as f32).max(1.0);
                                        let ih = (sz.height.0 as f32).max(1.0);
                                        let vw = f32::from(bounds.size.width);
                                        let vh = f32::from(bounds.size.height);
                                        let fit = (vw / iw).min(vh / ih).max(0.0001);
                                        let dw = iw * fit;
                                        let dh = ih * fit;
                                        let ox = bounds.origin.x + px((vw - dw) * 0.5);
                                        let oy = bounds.origin.y + px((vh - dh) * 0.5);
                                        let img_bounds = Bounds {
                                            origin: point(ox, oy),
                                            size: size(px(dw), px(dh)),
                                        };
                                        let _ = window.paint_image(
                                            img_bounds,
                                            Corners::default(),
                                            img.clone(),
                                            0,
                                            false,
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        ),
                );
            }
            list = list.child(entry);
        }

        // 竖直拖动条 (与宿主其它列表滚动条同款样式/交互, 仅内容溢出时显示).
        let handle = self.pool_scroll.clone();
        let max_y = f32::from(handle.max_offset().height);
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        let show_v = max_y > 1.0 && track_h > 1.0;
        let mut list_row = div()
            .id("sv_pool_row")
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .child(list);
        if show_v {
            let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
            let travel = (track_h - thumb_h).max(1.0);
            let off_y = -f32::from(handle.offset().y);
            let frac = (off_y / max_y).clamp(0.0, 1.0);
            let thumb_top = frac * travel;
            list_row = list_row.child(
                div()
                    .id("sv_pool_vtrack")
                    .w(px(10.))
                    .h_full()
                    .flex_shrink_0()
                    .relative()
                    .rounded_sm()
                    .bg(rgb(0x1e293b))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let y = f32::from(ev.position.y);
                            let handle = this.pool_scroll.clone();
                            let b = handle.bounds();
                            let th = f32::from(b.size.height).max(1.0);
                            let max = f32::from(handle.max_offset().height);
                            if max <= 0.5 {
                                return;
                            }
                            let thumb = ((th * th) / (th + max)).clamp(24.0, th);
                            let travel = (th - thumb).max(1.0);
                            let track_top = f32::from(b.origin.y);
                            let target = (y - track_top - thumb * 0.5).clamp(0.0, travel);
                            handle.set_offset(point(px(0.), px(-(target / travel) * max)));
                            this.drag = Some(VideoDrag::PoolScroll { grab: thumb * 0.5 });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id("sv_pool_vthumb")
                            .absolute()
                            .left_0()
                            .top(px(thumb_top))
                            .w_full()
                            .h(px(thumb_h))
                            .rounded_sm()
                            .bg(rgb(0x475569))
                            .hover(|s| s.bg(rgb(0x64748b)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    let y = f32::from(ev.position.y);
                                    let handle = this.pool_scroll.clone();
                                    let b = handle.bounds();
                                    let th = f32::from(b.size.height).max(1.0);
                                    let max = f32::from(handle.max_offset().height);
                                    let thumb = if max > 0.5 {
                                        ((th * th) / (th + max)).clamp(24.0, th)
                                    } else {
                                        th
                                    };
                                    let travel = (th - thumb).max(1.0);
                                    let track_top = f32::from(b.origin.y);
                                    let off = -f32::from(handle.offset().y);
                                    let frac = if max > 0.5 {
                                        (off / max).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let cur_top = track_top + frac * travel;
                                    this.drag = Some(VideoDrag::PoolScroll {
                                        grab: (y - cur_top).clamp(0.0, thumb),
                                    });
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }

        div()
            .id("sv_right")
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            .bg(rgb(0x0f172a))
            .text_color(rgb(0xe2e8f0))
            .child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0xcbd5e1))
                    .child(if self.pool_status.is_empty() {
                        format!("素材池 ({} 个输出组合, 可拖入视频轨道)", self.pool.len())
                    } else {
                        format!(
                            "素材池 ({} 个输出组合) — {}",
                            self.pool.len(),
                            self.pool_status
                        )
                    }),
            )
            .child(list_row)
            .child(
                div()
                    .flex_shrink_0()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(0x1e293b))
                    .child(self.btn(
                        "sv_export",
                        "导出视频...",
                        true,
                        |this, _, cx| {
                            this.export_open = true;
                            cx.notify();
                        },
                        cx,
                    )),
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn stepper(
        &self,
        minus_id: &'static str,
        plus_id: &'static str,
        label: SharedString,
        on_minus: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        on_plus: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.btn(minus_id, "-", false, on_minus, cx))
            .child(div().text_sm().min_w(px(90.)).text_center().child(label))
            .child(self.btn(plus_id, "+", false, on_plus, cx))
    }

}
