use gpui::{
    canvas, div, prelude::*, px, relative, rgb, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, SharedString,
};

use super::*;

impl PhotoEditApp {
    pub fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .when(!self.standalone, |d| d.flex_nowrap().overflow_hidden())
            .w_full()
            .child(self.tool_btn(
                "sel",
                "框选 (V)",
                self.mode == ToolMode::Select,
                ToolMode::Select,
                cx,
            ))
            .child(self.tool_btn(
                "mov",
                "移动 (M)",
                self.mode == ToolMode::Move,
                ToolMode::Move,
                cx,
            ))
            .child(self.tool_btn(
                "can",
                "画布 (K)",
                self.mode == ToolMode::Canvas,
                ToolMode::Canvas,
                cx,
            ))
            .child(self.tool_btn(
                "hel",
                "修复 (H)",
                self.mode == ToolMode::Heal,
                ToolMode::Heal,
                cx,
            ))
            .child(self.tool_btn(
                "cln",
                "图章 (S)",
                self.mode == ToolMode::Clone,
                ToolMode::Clone,
                cx,
            ))
            .child(self.tool_btn(
                "pnt",
                "画笔 (B)",
                self.mode == ToolMode::Paint,
                ToolMode::Paint,
                cx,
            ))
            .child(self.tool_btn(
                "ers",
                "橡皮 (E)",
                self.mode == ToolMode::Eraser,
                ToolMode::Eraser,
                cx,
            ))
            .child(self.tool_btn(
                "wnd",
                "魔棒 (W)",
                self.mode == ToolMode::Wand,
                ToolMode::Wand,
                cx,
            ))
            .child(self.tool_btn(
                "las",
                "套索 (L)",
                self.mode == ToolMode::Lasso,
                ToolMode::Lasso,
                cx,
            ))
            .child(self.action_btn(
                "newl",
                "提出图层 (J)",
                |t, _, cx| t.new_layer_from_selection(cx),
                cx,
            ))
            .child(self.action_btn("fit", "适应 (F)", |t, _, cx| t.fit_to_view(cx), cx))
            .when(!self.standalone, |d| {
                d.child(self.action_btn("apply", "应用并退出", |t, _, cx| t.request_apply(cx), cx))
                    .child(self.action_btn(
                        "cancel",
                        "取消 (Esc)",
                        |t, _, cx| t.request_cancel(cx),
                        cx,
                    ))
                    .child(self.action_btn("rest", "还原", |t, _, cx| t.request_restore(cx), cx))
            })
            .when(!self.status.is_empty(), |d| {
                d.child(div().flex_1()).child(
                    div()
                        .px_2()
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(0x64748b))
                        .child(self.status.clone()),
                )
            })
    }

    pub fn toolbar_embedded(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.toolbar(cx)
    }

    fn tool_btn(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        mode: ToolMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.chip(
            id,
            label,
            on,
            move |this, _, cx| this.set_mode(mode, cx),
            cx,
        )
    }

    fn action_btn(
        &self,
        id: &'static str,
        label: &'static str,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.chip(id, label, false, f, cx)
    }

    fn chip(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id.into())
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .text_xs()
            .cursor_pointer()
            .when(on, |d| {
                d.bg(rgb(0xd8e0ea)).font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .hover(|s| s.bg(rgb(0xd8e0ea)))
            .child(label.into())
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.focus_handle.focus(window);
                    f(this, window, cx);
                }),
            )
    }

    pub fn side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fit = self.fit_label();
        div()
            .id("photo_side")
            .when(self.standalone, |d| {
                d.w(px(260.))
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(rgb(0xcbd5e1))
            })
            .when(!self.standalone, |d| d.w_full())
            .h_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(0xf1f5f9))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::LayerReorder { .. })) {
                    this.update_layer_reorder(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                } else if matches!(this.drag, Some(DragKind::PaletteSb)) {
                    this.set_palette_sb_from_pos(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                } else if matches!(this.drag, Some(DragKind::PaletteHue)) {
                    this.set_palette_hue_from_y(f32::from(ev.position.y), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if matches!(this.drag, Some(DragKind::PaletteSb | DragKind::PaletteHue)) {
                        this.drag = None;
                        this.notify_chrome(cx);
                        return;
                    }
                    this.finish_layer_reorder(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if matches!(this.drag, Some(DragKind::PaletteSb | DragKind::PaletteHue)) {
                        this.drag = None;
                        this.notify_chrome(cx);
                        return;
                    }
                    this.finish_layer_reorder(cx);
                }),
            )
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.side_bounds = bounds;
                                this.side_origin =
                                    (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("图层"),
            )
            .child(self.layer_list(cx))
            .child(self.fit_box(fit))
            .child(self.tool_opts(cx))
            .child(self.layer_drag_ghost())
            .when(self.color_picker_open, |d| {
                d.child(self.color_picker_floating(cx))
            })
    }

    fn layer_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("photo_layers").flex().flex_col().px_1().gap_1();
        let drag_from = match &self.drag {
            Some(DragKind::LayerReorder {
                from, armed: true, ..
            }) => Some(*from),
            _ => None,
        };
        let (line_at, line_after) = match &self.drag {
            Some(DragKind::LayerReorder {
                line_at,
                line_after,
                armed: true,
                ..
            }) => (*line_at, *line_after),
            _ => (None, false),
        };
        if let Some(doc) = self.doc.as_ref() {
            let n = doc.layers.len();
            for (storage, layer) in doc.layers.iter().enumerate().rev() {
                let display = n - 1 - storage;
                let active = storage == doc.active;
                let vis = layer.visible;
                let name = layer.name.clone();
                let dragging = drag_from == Some(display);
                let show_line = line_at == Some(display);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("ly-{storage}")))
                        .relative()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) })
                        .text_color(if active { rgb(0xffffff) } else { rgb(0x0f172a) })
                        .text_xs()
                        .cursor_pointer()
                        .flex_shrink_0()
                        .when(dragging, |d| d.opacity(0.35))
                        .when(show_line && !line_after, |d| {
                            d.border_t_2().border_color(rgb(0xf59e0b))
                        })
                        .when(show_line && line_after, |d| {
                            d.border_b_2().border_color(rgb(0xf59e0b))
                        })
                        .child(Self::measure_layer_bounds(cx.entity(), display))
                        .child(
                            div()
                                .id(SharedString::from(format!("lyv-{storage}")))
                                .w(px(18.))
                                .child(if vis { "●" } else { "○" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.focus_handle.focus(window);
                                        this.toggle_layer_vis(storage, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .child(name)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        this.focus_handle.focus(window);
                                        let mx = f32::from(ev.position.x);
                                        let my = f32::from(ev.position.y);
                                        let (ox, oy) = this
                                            .layer_bounds
                                            .get(&display)
                                            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
                                            .unwrap_or((mx, my));
                                        this.drag = Some(DragKind::LayerReorder {
                                            from: display,
                                            to: display,
                                            line_at: None,
                                            line_after: false,
                                            start_x: mx,
                                            start_y: my,
                                            origin_x: ox,
                                            origin_y: oy,
                                            x: mx,
                                            y: my,
                                            armed: false,
                                        });
                                        cx.notify();
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    move |this, ev: &MouseMoveEvent, _, cx| {
                                        if matches!(this.drag, Some(DragKind::LayerReorder { .. }))
                                        {
                                            this.update_layer_reorder(
                                                f32::from(ev.position.x),
                                                f32::from(ev.position.y),
                                                cx,
                                            );
                                        }
                                    },
                                )),
                        ),
                );
            }
        }
        list.child(
            div()
                .flex()
                .flex_row()
                .px_1()
                .gap_1()
                .child(self.action_btn("dup", "复制层", |t, _, cx| t.duplicate_layer(cx), cx))
                .child(self.action_btn(
                    "dell",
                    "删层 (Del)",
                    |t, _, cx| t.delete_active_layer(cx),
                    cx,
                )),
        )
    }

    fn measure_layer_bounds(entity: gpui::Entity<Self>, key: usize) -> impl IntoElement {
        canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, _| {
                    this.layer_bounds.insert(key, bounds);
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    fn layer_drag_ghost(&self) -> impl IntoElement {
        let Some(DragKind::LayerReorder {
            from,
            start_x,
            start_y,
            origin_x,
            origin_y,
            x,
            y,
            armed: true,
            ..
        }) = &self.drag
        else {
            return div().into_any_element();
        };
        let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
        let label = if n > 0 && *from < n {
            let storage = n - 1 - from;
            self.doc
                .as_ref()
                .and_then(|d| d.layers.get(storage))
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "...".into())
        } else {
            "...".into()
        };
        let gx = *origin_x + (*x - *start_x) - self.side_origin.0;
        let gy = *origin_y + (*y - *start_y) - self.side_origin.1;
        div()
            .id("layer-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x0f172a))
            .text_xs()
            .border_1()
            .border_color(rgb(0x94a3b8))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    fn fit_label(&self) -> String {
        let Some(doc) = self.doc.as_ref() else {
            return "未打开".into();
        };
        if self.fit_hint.aspect_w == 0 || !self.fit_hint.bg_enabled {
            return format!("画布 {}×{}", doc.canvas_w, doc.canvas_h);
        }
        let scale = self.fit_hint.scale_for(doc.canvas_w, doc.canvas_h);
        let sheet_h = self.fit_hint.other_h.saturating_add(doc.canvas_h);
        let page_h = self.fit_hint.page_h(doc.canvas_w);
        if scale < 0.999 {
            format!(
                "装页偏高: 组高 {sheet_h} / 页高 {page_h} (缩尺 {:.0}%)",
                scale * 100.0
            )
        } else {
            format!("装得进 16:9 (组高 {sheet_h} ≤ 页高 {page_h})")
        }
    }

    fn fit_box(&self, text: String) -> impl IntoElement {
        let ok = text.contains("装得进");
        div()
            .mx_2()
            .my_1()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(if ok { rgb(0xdcfce7) } else { rgb(0xfef3c7) })
            .text_xs()
            .child(text)
    }

    fn tool_opts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut box_ = div().flex().flex_col().px_2().py_1().gap_1().text_xs();
        match self.mode {
            ToolMode::Heal | ToolMode::Clone => {
                box_ = box_
                    .child(self.brush_size_slider(cx))
                    .child(self.hardness_slider(cx));
            }
            ToolMode::Paint => {
                box_ = box_
                    .child(self.paint_color_row(cx))
                    .child(self.brush_size_slider(cx))
                    .child(self.hardness_slider(cx));
            }
            ToolMode::Eraser => {
                let hint = match (self.erase_target, self.trace_erase) {
                    (EraseTarget::Layer, _) => "单击删最上层, 拖动删碰到的图层; Alt+滚轮调大小",
                    (EraseTarget::Trace, TraceErase::Dab) => {
                        "单击擦最上层笔尖, 拖动擦碰到的盖章点; 从中间擦断会拆成两笔"
                    }
                    (EraseTarget::Trace, TraceErase::Stroke) => {
                        "单击删最上面一整笔, 拖动删碰到的每一笔"
                    }
                };
                box_ = box_.child(self.brush_size_slider(cx)).child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .child(self.chip(
                            "erase_trace",
                            "痕迹",
                            self.erase_target == EraseTarget::Trace,
                            |this, _, cx| {
                                this.erase_target = EraseTarget::Trace;
                                this.notify_chrome(cx);
                            },
                            cx,
                        ))
                        .child(self.chip(
                            "erase_layer",
                            "图层",
                            self.erase_target == EraseTarget::Layer,
                            |this, _, cx| {
                                this.erase_target = EraseTarget::Layer;
                                this.notify_chrome(cx);
                            },
                            cx,
                        )),
                );
                if self.erase_target == EraseTarget::Trace {
                    box_ = box_.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .child(self.chip(
                                "erase_dab",
                                "笔尖",
                                self.trace_erase == TraceErase::Dab,
                                |this, _, cx| {
                                    this.trace_erase = TraceErase::Dab;
                                    this.notify_chrome(cx);
                                },
                                cx,
                            ))
                            .child(self.chip(
                                "erase_stroke",
                                "整笔",
                                self.trace_erase == TraceErase::Stroke,
                                |this, _, cx| {
                                    this.trace_erase = TraceErase::Stroke;
                                    this.notify_chrome(cx);
                                },
                                cx,
                            )),
                    );
                }
                box_ = box_.child(div().text_color(rgb(0x64748b)).child(hint));
            }
            ToolMode::Wand => {
                box_ = box_
                    .child(self.wand_tol_slider(cx))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .child(self.chip(
                                "wand_contig",
                                "连续",
                                self.wand_contiguous,
                                |this, _, cx| {
                                    this.wand_contiguous = !this.wand_contiguous;
                                    this.notify_chrome(cx);
                                },
                                cx,
                            ))
                            .child(self.chip(
                                "wand_aa",
                                "抗锯齿",
                                self.wand_antialias,
                                |this, _, cx| {
                                    this.wand_antialias = !this.wand_antialias;
                                    this.notify_chrome(cx);
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_color(rgb(0x64748b))
                            .child("Shift 加选, Alt 减选, Shift+Alt 交选"),
                    );
            }
            ToolMode::Lasso => {
                box_ = box_.child(
                    div()
                        .text_color(rgb(0x64748b))
                        .child("单击折线, 按住拖轨迹, 靠近首点或松手闭环"),
                );
            }
            _ => {}
        }
        if !self.import_options.is_empty() {
            box_ = box_.child(
                div()
                    .mt_1()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("从其它块导入"),
            );
            for opt in &self.import_options {
                let id = opt.id.clone();
                let label = opt.label.clone();
                box_ = box_.child(
                    div()
                        .id(SharedString::from(format!("imp-{id}")))
                        .px_1()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(0xe2e8f0))
                        .cursor_pointer()
                        .child(label)
                        .on_mouse_up(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.host_cmd = Some(HostCmd::RequestImport(id.clone()));
                                this.notify_chrome(cx);
                            }),
                        ),
                );
            }
        }
        box_
    }

    fn paint_color_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.paint_color;
        let paper = self
            .doc
            .as_ref()
            .map(|d| d.paper_rgb)
            .unwrap_or([245, 245, 240]);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("photo_paint_swatch")
                    .relative()
                    .w(px(22.))
                    .h(px(22.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .bg(rgb(((c[0] as u32) << 16)
                        | ((c[1] as u32) << 8)
                        | c[2] as u32))
                    .cursor_pointer()
                    .hover(|s| s.border_color(rgb(0x334155)))
                    .child(
                        canvas(
                            {
                                let entity = cx.entity().clone();
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, _| {
                                        this.paint_swatch_bounds = bounds;
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_color_picker(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(0x64748b))
                    .child("点色块开调色盘 · Alt 取样"),
            )
            .child(self.chip(
                "paper_c",
                "谱纸",
                false,
                move |this, _, cx| {
                    this.set_paint_color(paper, cx);
                },
                cx,
            ))
            .child(self.chip(
                "ink_c",
                "墨",
                false,
                |this, _, cx| {
                    this.set_paint_color([32, 32, 32], cx);
                },
                cx,
            ))
    }

    fn brush_size_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size_frac = brush_size_to_t(self.brush, BRUSH_MIN, self.brush_size_max());
        let brush_px = self.brush.round() as i32;
        self.frac_slider(
            "photo_brush_size_track",
            size_frac,
            format!("{brush_px}px"),
            SliderKind::BrushSize,
            cx,
        )
    }

    fn hardness_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.frac_slider(
            "photo_hardness_track",
            self.brush_hardness.clamp(0.0, 1.0),
            format!("硬度 {:.0}%", self.brush_hardness * 100.0),
            SliderKind::Hardness,
            cx,
        )
    }

    fn wand_tol_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = (self.wand_tol as f32 / WAND_TOL_MAX as f32).clamp(0.0, 1.0);
        self.frac_slider(
            "photo_wand_tol_track",
            t,
            format!("容差 {}", self.wand_tol),
            SliderKind::WandTol,
            cx,
        )
    }

    fn frac_slider(
        &self,
        id: &'static str,
        frac: f32,
        caption: String,
        kind: SliderKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let frac = frac.clamp(0.0, 1.0);
        div()
            .relative()
            .w_full()
            .h(px(28.))
            .child(
                div()
                    .absolute()
                    .left(relative(frac))
                    .bottom(px(16.))
                    .ml(px(-14.))
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(rgb(0x64748b))
                    .child(caption),
            )
            .child(
                div()
                    .id(id)
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(14.))
                    .rounded_full()
                    .bg(rgb(0xe2e8f0))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(
                        canvas(
                            {
                                let entity = cx.entity().clone();
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, _| {
                                        *this.slider_track_mut(kind) = bounds;
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .size_full()
                        .absolute(),
                    )
                    .child(
                        div()
                            .h_full()
                            .w(relative(frac))
                            .bg(rgb(0x2563eb))
                            .rounded_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.drag = Some(DragKind::Slider(kind));
                            this.set_slider_from_x(f32::from(ev.position.x), kind, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &gpui::MouseMoveEvent, _, cx| {
                        if matches!(this.drag, Some(DragKind::Slider(k)) if k == kind) {
                            this.set_slider_from_x(f32::from(ev.position.x), kind, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if matches!(this.drag, Some(DragKind::Slider(k)) if k == kind) {
                                this.drag = None;
                                this.notify_chrome(cx);
                            }
                        }),
                    ),
            )
    }
}
