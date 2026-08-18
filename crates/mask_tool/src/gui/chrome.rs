//! 工具栏与侧栏.

use super::*;

impl MaskToolApp {
    pub fn toolbar_embedded(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.btn("export", "导出本块", false, false, Self::export_image, cx))
            .child(self.btn(
                "fit",
                "适应",
                false,
                false,
                |this, _, cx| this.fit_to_view(cx),
                cx,
            ))
            .child(self.btn(
                "del",
                "删除",
                false,
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x64748b))
                    .child(self.status.clone()),
            )
    }
    pub(super) fn btn(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        grow: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
        let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
        let hover = if active { rgb(0x1d4ed8) } else { rgb(0xcbd5e1) };
        let mut el = div()
            .id(id.into())
            .px_3()
            .py_1()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(rgb(0x94a3b8))
            .text_color(fg)
            .cursor_pointer()
            .hover(move |s| s.bg(hover));
        if grow {
            el = el.flex_1().flex().justify_center().min_w(px(0.));
        }
        el.child(label.into()).on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
        )
    }

    pub fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.btn("open", "打开", false, false, Self::open_file, cx))
            .child(self.btn("export", "导出", false, false, Self::export_image, cx))
            .child(self.btn(
                "fit",
                "适应窗口",
                false,
                false,
                |this, _, cx| this.fit_to_view(cx),
                cx,
            ))
            .child(self.btn(
                "del",
                "删除",
                false,
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x64748b))
                    .child(self.status.clone()),
            )
    }

    pub(super) fn color_picker_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let slider_op = self.slider_opacity_value();
        let opacity_pct = (slider_op * 100.0).round() as i32;
        let opacity_label = if self.selected.is_empty() {
            format!("不透明度  {opacity_pct}%")
        } else {
            format!("选中项不透明度  {opacity_pct}%")
        };
        let frac = ((slider_op - 0.05) / 0.95).clamp(0.0, 1.0);
        let sb_img = self.sb_image.clone();
        let hue_img = self.hue_image.clone();
        let picker_s = self.picker_s;
        let picker_v = self.picker_v;
        let picker_h = self.picker_h;
        let recent: Vec<[u8; 3]> = self
            .recent_colors
            .iter()
            .copied()
            .take(RECENT_COLORS_MAX)
            .collect();

        div()
            .id("color_picker_popover")
            .w_full()
            .p_2()
            .rounded_md()
            .bg(rgb(0x1e293b))
            .border_1()
            .border_color(rgb(0x334155))
            .flex()
            .flex_col()
            .gap_2()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0xcbd5e1))
                    .child(opacity_label),
            )
            .child({
                div()
                    .id("palette_opacity_track")
                    .relative()
                    .w_full()
                    .h(px(14.))
                    .rounded_full()
                    .bg(rgb(0x334155))
                    .border_1()
                    .border_color(rgb(0x475569))
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(
                        canvas(
                            {
                                let entity = cx.entity().clone();
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, _| {
                                        this.opacity_track = bounds;
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
                            .bg(rgb(0x38bdf8))
                            .rounded_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.opacity_undid = false;
                            this.drag = Some(DragKind::PaletteOpacity);
                            this.set_palette_opacity_from_x(f32::from(ev.position.x), cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if matches!(this.drag, Some(DragKind::PaletteOpacity)) {
                                this.drag = None;
                                this.opacity_undid = false;
                                cx.notify();
                            }
                        }),
                    )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .children(recent.into_iter().enumerate().map(|(i, color)| {
                        let color_u32 = color_rgb_u32(color);
                        div()
                            .id(SharedString::from(format!("recent-{i}")))
                            .size(px(22.))
                            .rounded_sm()
                            .bg(rgb(color_u32))
                            .border_1()
                            .border_color(rgb(0x64748b))
                            .cursor_pointer()
                            .hover(|s| s.border_color(rgb(0x94a3b8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pick_recent_color(color, cx);
                                }),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .id("palette_sb")
                            .relative()
                            .size(px(SB_SIZE))
                            .flex_shrink_0()
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(0x475569))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    {
                                        let entity = cx.entity().clone();
                                        move |bounds, _, cx| {
                                            entity.update(cx, |this, _| {
                                                this.sb_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = sb_img {
                                            let b = Bounds {
                                                origin: bounds.origin,
                                                size: bounds.size,
                                            };
                                            let _ = window.paint_image(
                                                b,
                                                Corners::default(),
                                                img.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                        let mx = bounds.origin.x
                                            + px(picker_s * f32::from(bounds.size.width));
                                        let my = bounds.origin.y
                                            + px((1.0 - picker_v) * f32::from(bounds.size.height));
                                        let mark = Bounds {
                                            origin: point(mx - px(5.), my - px(5.)),
                                            size: size(px(10.), px(10.)),
                                        };
                                        window.paint_quad(quad(
                                            mark,
                                            px(5.),
                                            rgb(0xffffff),
                                            px(1.5),
                                            rgb(0x0f172a),
                                            Default::default(),
                                        ));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.opacity_undid = false;
                                    this.drag = Some(DragKind::PaletteSb);
                                    this.set_palette_sb_from_pos(
                                        f32::from(ev.position.x),
                                        f32::from(ev.position.y),
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::PaletteSb)) {
                                        this.drag = None;
                                        this.opacity_undid = false;
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("palette_hue")
                            .relative()
                            .w(px(HUE_BAR_W))
                            .h(px(SB_SIZE))
                            .flex_shrink_0()
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(0x475569))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    {
                                        let entity = cx.entity().clone();
                                        move |bounds, _, cx| {
                                            entity.update(cx, |this, _| {
                                                this.hue_bounds = bounds;
                                            });
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        if let Some(ref img) = hue_img {
                                            let b = Bounds {
                                                origin: bounds.origin,
                                                size: bounds.size,
                                            };
                                            let _ = window.paint_image(
                                                b,
                                                Corners::default(),
                                                img.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                        let hy = bounds.origin.y
                                            + px((picker_h / 360.0).clamp(0.0, 1.0)
                                                * f32::from(bounds.size.height));
                                        let mark = Bounds {
                                            origin: point(
                                                bounds.origin.x,
                                                hy - px(2.),
                                            ),
                                            size: size(bounds.size.width, px(4.)),
                                        };
                                        window.paint_quad(quad(
                                            mark,
                                            px(0.),
                                            rgb(0xffffff),
                                            px(1.),
                                            rgb(0x0f172a),
                                            Default::default(),
                                        ));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.opacity_undid = false;
                                    this.drag = Some(DragKind::PaletteHue);
                                    this.set_palette_hue_from_y(f32::from(ev.position.y), cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if matches!(this.drag, Some(DragKind::PaletteHue)) {
                                        this.drag = None;
                                        this.opacity_undid = false;
                                        cx.notify();
                                    }
                                }),
                            ),
                    ),
            )
            .child({
                let r_in = self.rgb_r_input.clone();
                let g_in = self.rgb_g_input.clone();
                let b_in = self.rgb_b_input.clone();
                let drop_on = self.eyedropper_armed;
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xcbd5e1))
                                    .child("RGB"),
                            )
                            .child(
                                div()
                                    .id("eyedropper_btn")
                                    .size(px(20.))
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(if drop_on {
                                        rgb(0x38bdf8)
                                    } else {
                                        rgb(0x475569)
                                    })
                                    .bg(if drop_on {
                                        rgb(0x0ea5e9)
                                    } else {
                                        rgb(0x334155)
                                    })
                                    .child(eyedropper_icon(drop_on))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.arm_eyedropper(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("R"),
                            )
                            .child(
                                div()
                                    .id("rgb_r_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(r_in),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("G"),
                            )
                            .child(
                                div()
                                    .id("rgb_g_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(g_in),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94a3b8))
                                    .flex_shrink_0()
                                    .child("B"),
                            )
                            .child(
                                div()
                                    .id("rgb_b_box")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(20.))
                                    .child(b_in),
                            ),
                    )
            })
    }

    pub fn side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mask_color_u32 = color_rgb_u32(self.mask_color);
        let list_items: Vec<_> = self
            .masks
            .iter()
            .map(|m| {
                let id = m.id.clone();
                let label = m.label();
                let selected = self.selected.contains(&m.id);
                (id, label, selected)
            })
            .collect();
        let side_w = if self.embed_side_width > 1.0 {
            self.embed_side_width
        } else {
            280.0
        };
        let embedded = self.embed_side_width > 1.0;

        let mut panel = div()
            .id("mask_side")
            .relative()
            .h_full()
            .flex()
            .flex_col()
            // padding 放内层, 避免绝对定位悬浮窗与侧栏 bounds 坐标系差出 padding 偏移
            .bg(rgb(0xf1f5f9));
        if embedded {
            panel = panel.w_full();
        } else {
            panel = panel
                .w(px(side_w))
                .border_l_1()
                .border_color(rgb(0xcbd5e1));
        }
        panel
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.side_bounds = bounds;
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("mask_side_inner")
                    .flex_1()
                    .w_full()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0x334155))
                            .child(if embedded {
                                "蒙版列表 (Ctrl+A 全选 · Delete 删除)"
                            } else {
                                "蒙版列表 (选中后 Delete 删除)"
                            }),
                    )
            .child(
                div()
                    .id("mask_list")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xcbd5e1))
                    .rounded_md()
                    .p_1()
                    .children(list_items.into_iter().map(|(id, label, selected)| {
                        let id_click = id.clone();
                        let bg = if selected {
                            rgb(0xdbeafe)
                        } else {
                            rgb(0xffffff)
                        };
                        div()
                            .id(SharedString::from(format!("mask-{id}")))
                            .w_full()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(bg)
                            .text_sm()
                            .text_color(rgb(0x0f172a))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xe2e8f0)))
                            .child(label)
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                                    if ev.modifiers.control {
                                        if this.selected.contains(&id_click) {
                                            this.selected.remove(&id_click);
                                        } else {
                                            this.selected.insert(id_click.clone());
                                        }
                                    } else {
                                        this.selected.clear();
                                        this.selected.insert(id_click.clone());
                                    }
                                    cx.notify();
                                }),
                            )
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child({
                        let brush_on = self.mode == ToolMode::Brush;
                        let eraser_on = self.mode == ToolMode::Eraser;
                        let size_frac = ((self.brush_size - BRUSH_SIZE_MIN)
                            / (BRUSH_SIZE_MAX - BRUSH_SIZE_MIN))
                            .clamp(0.0, 1.0);
                        let brush_px = self.brush_size.round() as i32;
                        let brush_color_u32 = color_rgb_u32(self.brush_color);
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .child(self.btn(
                                "mode_brush",
                                "画笔",
                                brush_on,
                                false,
                                |this, _, cx| this.toggle_brush_mode(cx),
                                cx,
                            ))
                            .child(
                                div()
                                    .id("brush_color_swatch")
                                    .relative()
                                    .size(px(28.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(brush_color_u32))
                                    .border_2()
                                    .border_color(rgb(0x000000))
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(rgb(0x334155)))
                                    .child(
                                        canvas(
                                            {
                                                let entity = cx.entity().clone();
                                                move |bounds, _, cx| {
                                                    entity.update(cx, |this, _| {
                                                        this.brush_swatch_bounds = bounds;
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
                                            this.open_color_picker(ColorPickerTarget::Brush, cx);
                                            if this.mode != ToolMode::Brush {
                                                this.mode = ToolMode::Brush;
                                                this.status = Self::mode_status(ToolMode::Brush);
                                            }
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(28.))
                                    .child(
                                        div()
                                            .absolute()
                                            .left(relative(size_frac))
                                            .bottom(px(16.))
                                            .ml(px(-14.))
                                            .whitespace_nowrap()
                                            .text_xs()
                                            .text_color(rgb(0x64748b))
                                            .child(format!("{brush_px}px")),
                                    )
                                    .child(
                                        div()
                                            .id("brush_size_track")
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
                                                                this.brush_size_track = bounds;
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
                                                    .w(relative(size_frac))
                                                    .bg(rgb(0x2563eb))
                                                    .rounded_full(),
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                                    this.drag = Some(DragKind::BrushSize);
                                                    this.set_brush_size_from_x(
                                                        f32::from(ev.position.x),
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    if matches!(this.drag, Some(DragKind::BrushSize))
                                                    {
                                                        this.drag = None;
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                    ),
                            )
                            .child(self.btn(
                                "mode_eraser",
                                "橡皮",
                                eraser_on,
                                false,
                                |this, _, cx| this.toggle_eraser_mode(cx),
                                cx,
                            ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .child(self.btn(
                                "mode_draw",
                                "框选 (B)",
                                self.mode == ToolMode::Draw,
                                true,
                                |this, _, cx| this.toggle_draw_mode(cx),
                                cx,
                            ))
                            .child(self.btn(
                                "mode_poly",
                                "折线 (L)",
                                self.mode == ToolMode::Poly,
                                true,
                                |this, _, cx| this.toggle_poly_mode(cx),
                                cx,
                            ))
                            .child(
                                div()
                                    .id("mask_color_swatch")
                                    .relative()
                                    .size(px(28.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(mask_color_u32))
                                    .border_2()
                                    .border_color(rgb(0x000000))
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(rgb(0x334155)))
                                    .child(
                                        canvas(
                                            {
                                                let entity = cx.entity().clone();
                                                move |bounds, _, cx| {
                                                    entity.update(cx, |this, _| {
                                                        this.mask_swatch_bounds = bounds;
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
                                            this.open_color_picker(ColorPickerTarget::Mask, cx);
                                        }),
                                    ),
                            )
                            .child(self.btn(
                                "mode_pan",
                                "平移 (P)",
                                self.mode == ToolMode::Pan,
                                true,
                                |this, _, cx| this.toggle_pan_mode(cx),
                                cx,
                            )),
                    )
                    .when(!embedded, |d| {
                        d.child(self.btn(
                            "btn_del",
                            "删除选中蒙版",
                            false,
                            false,
                            |this, _, cx| this.delete_selected(cx),
                            cx,
                        ))
                        .child(self.btn(
                            "btn_clear",
                            "清空全部蒙版",
                            false,
                            false,
                            |this, _, cx| this.clear_masks(cx),
                            cx,
                        ))
                    })
                    .child(self.btn(
                        "btn_export",
                        if embedded {
                            "导出本页图片 (E)…"
                        } else {
                            "导出已遮盖图片 (E)…"
                        },
                        false,
                        true,
                        Self::export_image,
                        cx,
                    )),
            ) // tools column
            ) // mask_side_inner
            .when(self.color_picker_open, |d| d.child(self.color_picker_floating(cx)))
    }
}
