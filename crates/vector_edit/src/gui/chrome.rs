use gpui::{
    canvas, div, img, point, prelude::*, px, relative, rgb, uniform_list, AnyElement, Bounds,
    DispatchPhase, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, PathBuilder, Pixels, SharedString,
};

use super::tools;
use super::*;

fn menu_key(id: &str, fallback: &str) -> String {
    if !ui_kit::show_menu_shortcuts() {
        return String::new();
    }
    ui_kit::shortcut_hint(id)
        .filter(|hint| !hint.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

impl VectorEditApp {
    pub fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.toolbar_embedded(cx)
    }

    pub fn toolbar_embedded(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = [
            ToolMode::Select,
            ToolMode::Node,
            ToolMode::Canvas,
            ToolMode::Hand,
            ToolMode::Pen,
            ToolMode::Pencil,
            ToolMode::Eraser,
            ToolMode::Lasso,
        ];
        let mut row = div()
            .id("vector_toolbar")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .w_full()
            .min_w(px(0.))
            .h_full()
            .overflow_x_scroll();
        for mode in modes {
            row = row.child(self.tool_btn(mode, cx));
        }
        row = row
            .child(self.action_btn(
                ui_kit::menu_label(ui_kit::t("vector.extract"), "vector.extract"),
                "v-extract",
                cx,
                |this, cx| this.extract_selection(cx),
            ))
            .child(self.action_btn(
                ui_kit::menu_label(ui_kit::t("vector.fit"), "vector.fit"),
                "v-fit",
                cx,
                |this, cx| this.fit_to_view(cx),
            ));
        if !self.standalone {
            row = row
                .child(
                    self.action_btn(ui_kit::t_src("应用"), "v-apply", cx, |this, cx| {
                        this.request_apply(cx)
                    }),
                )
                .child(
                    self.action_btn(ui_kit::t_src("还原"), "v-restore", cx, |this, cx| {
                        this.request_restore(cx)
                    }),
                )
                .child(self.action_btn(
                    ui_kit::menu_label(ui_kit::t("vector.cancel"), "vector.cancel"),
                    "v-cancel",
                    cx,
                    |this, cx| {
                        this.host_cmd = Some(HostCmd::Cancel);
                        cx.notify();
                    },
                ));
        } else {
            row = row
                .child(
                    self.action_btn(ui_kit::t_src("保存"), "v-save", cx, |this, cx| {
                        this.request_apply(cx)
                    }),
                )
                .child(self.action_btn("PNG", "v-png", cx, |this, cx| this.export_png(cx)))
                .child(self.action_btn("SVG", "v-svg", cx, |this, cx| this.export_svg(cx)));
        }
        row
    }

    fn tool_btn(&self, mode: ToolMode, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let on = self.mode == mode;
        div()
            .id(SharedString::from(format!("vt-{}", mode.label())))
            .flex_shrink_0()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .rounded_sm()
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .when(on, |d| {
                d.bg(ui_kit::card()).font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .hover(|s| s.bg(ui_kit::chrome(0xd8e0ea)))
            .child(ui_kit::menu_label(
                ui_kit::t(mode.command_id()),
                mode.command_id(),
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.focus_handle.focus(window);
                    this.set_mode(mode, window, cx);
                }),
            )
    }

    fn action_btn(
        &self,
        label: impl Into<SharedString>,
        id: &'static str,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let label = label.into();
        div()
            .id(id)
            .flex_shrink_0()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.bg(ui_kit::chrome(0xd8e0ea)))
            .child(label)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.focus_handle.focus(window);
                    f(this, cx);
                }),
            )
    }

    pub fn side_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.outline_row_hits.clear();
        if self.outline_menu.is_none() {
            self.menu_bounds = Bounds::default();
        }
        let mut bottom = div()
            .id("vector_side_bottom")
            .flex_shrink_0()
            .max_h(px(280.))
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .overflow_y_scroll();
        bottom = bottom
            .child(div().text_sm().child(ui_kit::t_src("外观")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.paint_proxy(cx))
                    .child(div().flex_1().min_w(px(0.)).child(self.stroke_slider(cx))),
            );
        bottom = bottom.child(
            div()
                .text_xs()
                .text_color(ui_kit::chrome(0x94a3b8))
                .child(format!("{}+滚轮调线宽", apply_bg::alt_mod())),
        );
        if !self.import_options.is_empty() {
            bottom = bottom.child(div().text_sm().child(ui_kit::t_src("导入其它块")));
            for opt in self.import_options.clone() {
                let id = opt.id.clone();
                bottom = bottom.child(
                    div()
                        .id(SharedString::from(format!("vimp-{}", opt.id)))
                        .text_xs()
                        .cursor_pointer()
                        .child(opt.label.clone())
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.host_cmd = Some(HostCmd::RequestImport(id.clone()));
                                this.notify_chrome(cx);
                            }),
                        ),
                );
            }
        }
        div()
            .id("vector_side")
            .relative()
            .when(self.standalone, |d| {
                d.w(px(260.))
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(ui_kit::chrome(0xcbd5e1))
            })
            .when(!self.standalone, |d| d.w_full())
            .h_full()
            .flex()
            .flex_col()
            .bg(ui_kit::chrome(0xf1f5f9))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.note_side_target();
                    if this.outline_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &super::RenameNode, window, cx| {
                this.begin_rename(window, cx);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.rename_target.is_some() && ev.keystroke.key == "escape" {
                    this.cancel_rename(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::OutlineScroll { .. })) {
                    this.apply_outline_scroll(f32::from(ev.position.y), cx);
                    return;
                }
                if let Some(DragKind::Slider(kind)) = this.drag {
                    this.set_slider_from_x(f32::from(ev.position.x), kind, cx);
                    return;
                }
                if matches!(this.drag, Some(DragKind::LayerReorder { .. })) {
                    this.update_layer_reorder(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                    return;
                }
                this.palette_pointer_move(f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.suppress_outline_click = false;
                    if matches!(this.drag, Some(DragKind::OutlineScroll { .. })) {
                        this.drag = None;
                        cx.notify();
                        return;
                    }
                    if matches!(this.drag, Some(DragKind::Slider(_))) {
                        this.finish_slider(cx);
                        return;
                    }
                    if matches!(this.drag, Some(DragKind::LayerReorder { .. })) {
                        this.finish_layer_reorder(cx);
                        return;
                    }
                    this.palette_pointer_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.suppress_outline_click = false;
                    if matches!(this.drag, Some(DragKind::OutlineScroll { .. })) {
                        this.drag = None;
                        cx.notify();
                        return;
                    }
                    if matches!(this.drag, Some(DragKind::LayerReorder { .. })) {
                        this.finish_layer_reorder(cx);
                    }
                }),
            )
            .child({
                let entity = cx.entity().clone();
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| {
                            this.picker_layer_bounds = bounds;
                        });
                    },
                    {
                        let entity = cx.entity().clone();
                        move |_, _, window, cx| {
                            let open_menu = entity.clone();
                            window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                                if phase != DispatchPhase::Capture
                                    || ev.button != MouseButton::Right
                                {
                                    return;
                                }
                                let x = f32::from(ev.position.x);
                                let y = f32::from(ev.position.y);
                                let handled = open_menu.update(cx, |this, cx| {
                                    this.handle_outline_right_click(x, y, cx)
                                });
                                if handled {
                                    cx.stop_propagation();
                                }
                            });
                            let (scrolling, renaming) = entity.update(cx, |this, _| {
                                (
                                    matches!(this.drag, Some(DragKind::OutlineScroll { .. })),
                                    this.rename_target.is_some(),
                                )
                            });
                            if scrolling {
                                let follow = entity.clone();
                                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Capture {
                                        return;
                                    }
                                    let y = f32::from(ev.position.y);
                                    follow.update(cx, |this, cx| this.apply_outline_scroll(y, cx));
                                });
                                let up = entity.clone();
                                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Capture {
                                        return;
                                    }
                                    up.update(cx, |this, cx| {
                                        if matches!(this.drag, Some(DragKind::OutlineScroll { .. }))
                                        {
                                            this.drag = None;
                                            cx.notify();
                                        }
                                    });
                                });
                            }
                            if renaming {
                                let commit = entity.clone();
                                window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Capture {
                                        return;
                                    }
                                    let x = f32::from(ev.position.x);
                                    let y = f32::from(ev.position.y);
                                    commit.update(cx, |this, cx| {
                                        if this.rename_target.is_none() {
                                            return;
                                        }
                                        let b = this.rename_bounds;
                                        let left = f32::from(b.origin.x);
                                        let top = f32::from(b.origin.y);
                                        let inside = x >= left
                                            && y >= top
                                            && x < left + f32::from(b.size.width)
                                            && y < top + f32::from(b.size.height);
                                        if !inside {
                                            this.commit_rename(cx);
                                        }
                                    });
                                });
                            }
                        }
                    },
                )
                .absolute()
                .inset_0()
            })
            .child(self.outline_body(cx))
            .child(bottom)
            .child(self.layer_drag_ghost())
            .when(self.palette_open, |d| {
                d.child(self.color_picker_floating(cx))
            })
            .child(self.outline_context_menu(cx))
    }

    fn can_ungroup(&self) -> bool {
        let Some(doc) = self.doc.as_ref() else {
            return false;
        };
        self.selected
            .iter()
            .any(|id| doc.find_group(id).is_some() || doc.parent_of(id).is_some())
    }

    fn outline_shown(&self, id: &str) -> bool {
        let Some(doc) = self.doc.as_ref() else {
            return true;
        };
        if let Some(layer) = doc.layers.iter().find(|layer| layer.id == id) {
            return layer.visible;
        }
        if let Some((li, gi)) = doc.find_group(id) {
            return doc.layers[li].groups[gi].visible;
        }
        doc.object(id).is_none_or(|obj| obj.visible)
    }

    fn outline_context_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = self.outline_menu.clone().filter(|menu| !menu.on_canvas) else {
            return div().into_any_element();
        };
        let shown = self.outline_shown(&menu.target);
        let can_group = self.selected.len() >= 2;
        let can_ungroup = self.can_ungroup();
        let can_regroup = !self.regroup_mem.is_empty();
        let can_extract = !self.selected.is_empty();
        let can_paste_obj = self.obj_clip.is_some();
        let can_paste_layer = self.layer_clip.is_some();
        let group_hint = menu_key("vector.group", &format!("{}G", apply_bg::primary_mod()));
        let ungroup_hint = menu_key("vector.ungroup", &format!("{}G", apply_bg::primary_shift()));
        let regroup_hint = menu_key("vector.regroup", &format!("{}G", apply_bg::primary_alt()));
        let copy_hint = menu_key("vector.copy", &format!("{}C", apply_bg::primary_mod()));
        let paste_hint = menu_key("vector.paste", &format!("{}V", apply_bg::primary_mod()));
        let mut col = div()
            .id("vector-outline-menu")
            .absolute()
            .w(px(208.))
            .py_1()
            .flex()
            .flex_col()
            .rounded_md()
            .bg(ui_kit::card())
            .text_color(ui_kit::chrome(0x0f172a))
            .border_1()
            .border_color(ui_kit::chrome(0x94a3b8))
            .when(menu.pin_right, |d| d.right(px(8.)).top(px(menu.y)))
            .when(!menu.pin_right, |d| d.left(px(menu.x)).top(px(menu.y)))
            .child({
                let entity = cx.entity().clone();
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| this.menu_bounds = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.suppress_outline_click = true;
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .child(self.menu_row(
                "vmenu-rename",
                ui_kit::t_src("重命名"),
                menu_key("vector.rename", "F2"),
                cx,
                |this, window, cx| {
                    this.begin_rename(window, cx);
                },
            ))
            .child(self.menu_row(
                "vmenu-vis",
                if shown {
                    ui_kit::t_src("隐藏")
                } else {
                    ui_kit::t_src("显示")
                },
                "",
                cx,
                {
                    let target = menu.target.clone();
                    move |this, _, cx| this.toggle_row_visible(&target, cx)
                },
            ))
            .child(div().my(px(4.)).h(px(1.)).bg(ui_kit::chrome(0xe2e8f0)))
            .child(self.menu_row(
                "vmenu-all",
                ui_kit::t_src("全选"),
                menu_key(
                    "vector.select_all",
                    &format!("{}A", apply_bg::primary_mod()),
                ),
                cx,
                |this, window, cx| this.select_all(window, cx),
            ))
            .child(self.menu_row(
                "vmenu-del",
                ui_kit::t_src("删除"),
                menu_key("vector.delete", "Del"),
                cx,
                |this, window, cx| {
                    this.delete_selection(window, cx);
                },
            ));
        if can_group || can_ungroup || can_regroup {
            col = col.child(div().my(px(4.)).h(px(1.)).bg(ui_kit::chrome(0xe2e8f0)));
            if can_group {
                col = col.child(self.menu_row(
                    "vmenu-group",
                    ui_kit::t_src("组合"),
                    group_hint.clone(),
                    cx,
                    |this, _, cx| {
                        this.group_selected(cx);
                    },
                ));
            }
            if can_ungroup {
                col = col.child(self.menu_row(
                    "vmenu-ungroup",
                    ui_kit::t_src("取消组合"),
                    ungroup_hint,
                    cx,
                    |this, _, cx| this.ungroup_selected(cx),
                ));
            }
            if can_regroup {
                col = col.child(self.menu_row(
                    "vmenu-regroup",
                    ui_kit::t_src("重新组合"),
                    regroup_hint,
                    cx,
                    |this, _, cx| this.regroup_selected(cx),
                ));
            }
        }
        col = col
            .child(div().my(px(4.)).h(px(1.)).bg(ui_kit::chrome(0xe2e8f0)))
            .child(self.menu_row(
                "vmenu-copy",
                ui_kit::t_src("复制"),
                copy_hint,
                cx,
                |this, window, cx| {
                    if this.selected.is_empty() {
                        this.clip_on_canvas = false;
                        this.copy_layer(window, cx);
                    } else {
                        this.copy_selection(cx);
                    }
                },
            ));
        if can_paste_obj {
            col = col.child(self.menu_row(
                "vmenu-paste-obj",
                ui_kit::t_src("粘贴对象"),
                paste_hint.clone(),
                cx,
                |this, window, cx| {
                    this.clip_on_canvas = true;
                    this.paste_layer(window, cx);
                },
            ));
        }
        if can_paste_layer {
            col = col.child(self.menu_row(
                "vmenu-paste-layer",
                ui_kit::t_src("粘贴图层"),
                if can_paste_obj {
                    String::new()
                } else {
                    paste_hint
                },
                cx,
                |this, window, cx| {
                    this.clip_on_canvas = false;
                    this.paste_layer(window, cx);
                },
            ));
        }
        if can_extract {
            col = col
                .child(div().my(px(4.)).h(px(1.)).bg(ui_kit::chrome(0xe2e8f0)))
                .child(self.menu_row(
                    "vmenu-extract",
                    ui_kit::t_src("取出到新图层"),
                    menu_key("vector.extract", "J"),
                    cx,
                    |this, _, cx| {
                        this.extract_selection(cx);
                    },
                ));
        }
        div().absolute().inset_0().child(col).into_any_element()
    }

    pub(crate) fn canvas_context_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = self.outline_menu.clone().filter(|menu| menu.on_canvas) else {
            return div().into_any_element();
        };
        let can_group = self.selected.len() >= 2;
        let can_ungroup = self.can_ungroup();
        let can_regroup = !self.regroup_mem.is_empty();
        let has_sel = !self.selected.is_empty();
        let can_paste_obj = self.obj_clip.is_some();
        let group_hint = menu_key("vector.group", &format!("{}G", apply_bg::primary_mod()));
        let ungroup_hint = menu_key("vector.ungroup", &format!("{}G", apply_bg::primary_shift()));
        let regroup_hint = menu_key("vector.regroup", &format!("{}G", apply_bg::primary_alt()));
        let copy_hint = menu_key("vector.copy", &format!("{}C", apply_bg::primary_mod()));
        let paste_hint = menu_key("vector.paste", &format!("{}V", apply_bg::primary_mod()));
        let mut col = div()
            .id("vector-canvas-menu")
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .w(px(208.))
            .py_1()
            .flex()
            .flex_col()
            .rounded_md()
            .bg(ui_kit::card())
            .text_color(ui_kit::chrome(0x0f172a))
            .border_1()
            .border_color(ui_kit::chrome(0x94a3b8))
            .child({
                let entity = cx.entity().clone();
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| this.menu_bounds = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.suppress_outline_click = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, _: &MouseUpEvent, _, cx| cx.stop_propagation()),
            )
            .child(self.menu_row(
                "cmenu-all",
                ui_kit::t_src("全选"),
                menu_key(
                    "vector.select_all",
                    &format!("{}A", apply_bg::primary_mod()),
                ),
                cx,
                |this, window, cx| this.select_all(window, cx),
            ));
        if has_sel {
            col = col.child(self.menu_row(
                "cmenu-del",
                ui_kit::t_src("删除"),
                menu_key("vector.delete", "Del"),
                cx,
                |this, window, cx| {
                    this.delete_selection(window, cx);
                },
            ));
        }
        if can_group || can_ungroup || can_regroup {
            col = col.child(div().my(px(4.)).h(px(1.)).bg(ui_kit::chrome(0xe2e8f0)));
            if can_group {
                col = col.child(self.menu_row(
                    "cmenu-group",
                    ui_kit::t_src("组合"),
                    group_hint,
                    cx,
                    |this, _, cx| {
                        this.group_selected(cx);
                    },
                ));
            }
            if can_ungroup {
                col = col.child(self.menu_row(
                    "cmenu-ungroup",
                    ui_kit::t_src("取消组合"),
                    ungroup_hint,
                    cx,
                    |this, _, cx| this.ungroup_selected(cx),
                ));
            }
            if can_regroup {
                col = col.child(self.menu_row(
                    "cmenu-regroup",
                    ui_kit::t_src("重新组合"),
                    regroup_hint,
                    cx,
                    |this, _, cx| this.regroup_selected(cx),
                ));
            }
        }
        if has_sel || can_paste_obj {
            col = col.child(div().my(px(4.)).h(px(1.)).bg(ui_kit::chrome(0xe2e8f0)));
        }
        if has_sel {
            col = col.child(self.menu_row(
                "cmenu-copy",
                ui_kit::t_src("复制"),
                copy_hint,
                cx,
                |this, _, cx| {
                    this.copy_selection(cx);
                },
            ));
        }
        if can_paste_obj {
            col = col.child(self.menu_row(
                "cmenu-paste",
                ui_kit::t_src("粘贴对象"),
                paste_hint,
                cx,
                |this, window, cx| {
                    this.clip_on_canvas = true;
                    this.paste_layer(window, cx);
                },
            ));
        }
        if has_sel {
            col = col.child(self.menu_row(
                "cmenu-extract",
                ui_kit::t_src("取出到新图层"),
                menu_key("vector.extract", "J"),
                cx,
                |this, _, cx| {
                    this.extract_selection(cx);
                },
            ));
        }
        div().absolute().inset_0().child(col).into_any_element()
    }

    fn menu_row(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        hint: impl Into<SharedString>,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut gpui::Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let hint = hint.into();
        let show_hint = !hint.is_empty();
        div()
            .id(id)
            .h(px(26.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .cursor_pointer()
            .hover(|s| s.bg(ui_kit::chrome(0xdbeafe)))
            .child(div().text_xs().child(label.into()))
            .when(show_hint, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(ui_kit::chrome(0x94a3b8))
                        .child(hint),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.suppress_outline_click = true;
                    this.outline_menu = None;
                    f(this, window, cx);
                }),
            )
    }

    fn outline_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let all = self
            .doc
            .as_ref()
            .map(|doc| crate::outline::flatten(doc, &self.expanded))
            .unwrap_or_default();
        if all.is_empty() {
            return div()
                .flex_1()
                .p_2()
                .text_xs()
                .text_color(ui_kit::chrome(0x94a3b8))
                .child(ui_kit::t_src("没有图层"))
                .into_any_element();
        }
        let pins = self.pin_ids();
        let pinned: Vec<_> = pins
            .iter()
            .filter_map(|id| all.iter().find(|row| row.id == *id).cloned())
            .collect();
        let rows: Vec<_> = all
            .into_iter()
            .filter(|row| !pins.iter().any(|id| id == &row.id))
            .collect();
        let mut pin_col = div().flex_shrink_0().flex_col().px_1();
        for row in &pinned {
            pin_col = pin_col.child(self.outline_row(row, cx));
        }
        if !pinned.is_empty() {
            pin_col = pin_col
                .border_b_1()
                .border_color(ui_kit::chrome(0xe2e8f0))
                .pb_1();
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(pin_col)
            .child(self.outline_scroller(rows, cx))
            .into_any_element()
    }

    fn outline_scroller(
        &self,
        rows: Vec<crate::outline::OutlineRow>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity().clone();
        let scroll = gpui::UniformListScrollHandle(self.outline_scroll.0.clone());
        let list = if rows.is_empty() {
            div().flex_1().min_h(px(0.)).into_any_element()
        } else {
            uniform_list("vector-outline", rows.len(), move |range, window, app| {
                let notify = entity.update(app, |this, _| this.pump_thumbs(&rows, range.clone()));
                if notify {
                    let again = entity.clone();
                    window.defer(app, move |_, cx| {
                        again.update(cx, |_, cx| cx.notify());
                    });
                }
                entity.update(app, |this, cx| {
                    let mut out = Vec::new();
                    for i in range {
                        if let Some(row) = rows.get(i) {
                            out.push(this.outline_row(row, cx).into_any_element());
                        }
                    }
                    out
                })
            })
            .track_scroll(scroll)
            .size_full()
            .into_any_element()
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .pl_1()
            .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).child(list))
            .child(self.outline_scrollbar(cx))
    }

    fn apply_outline_scroll(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some(DragKind::OutlineScroll { grab }) = self.drag else {
            return;
        };
        let handle = self.outline_scroll.0.borrow().base_handle.clone();
        let max_y = f32::from(handle.max_offset().height);
        if max_y <= 0.5 {
            return;
        }
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        let track_top = f32::from(bounds.origin.y);
        let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
        let travel = (track_h - thumb_h).max(1.0);
        let thumb_top = (y - grab - track_top).clamp(0.0, travel);
        let frac = thumb_top / travel;
        let ox = handle.offset().x;
        handle.set_offset(point(ox, px(-frac * max_y)));
        cx.notify();
    }

    fn outline_scrollbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.outline_scroll.0.borrow().base_handle.clone();
        let max_y = f32::from(handle.max_offset().height);
        let bounds = handle.bounds();
        let track_h = f32::from(bounds.size.height).max(1.0);
        if max_y <= 1.0 || track_h <= 1.0 {
            return div().w(px(0.)).into_any_element();
        }
        let thumb_h = ((track_h * track_h) / (track_h + max_y)).clamp(24.0, track_h);
        let travel = (track_h - thumb_h).max(1.0);
        let off_y = -f32::from(handle.offset().y);
        let frac = (off_y / max_y).clamp(0.0, 1.0);
        let thumb_top = frac * travel;
        div()
            .id("vector-outline-vtrack")
            .w(px(10.))
            .h_full()
            .flex_shrink_0()
            .relative()
            .rounded_sm()
            .bg(ui_kit::chrome(0xe2e8f0))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let y = f32::from(ev.position.y);
                    let handle = this.outline_scroll.0.borrow().base_handle.clone();
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
                    let ox = handle.offset().x;
                    handle.set_offset(point(ox, px(-(target / travel) * max)));
                    this.drag = Some(DragKind::OutlineScroll { grab: thumb * 0.5 });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("vector-outline-vthumb")
                    .absolute()
                    .left_0()
                    .top(px(thumb_top))
                    .w_full()
                    .h(px(thumb_h))
                    .rounded_sm()
                    .bg(ui_kit::chrome(0x94a3b8))
                    .hover(|s| s.bg(ui_kit::chrome(0x64748b)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let y = f32::from(ev.position.y);
                            let handle = this.outline_scroll.0.borrow().base_handle.clone();
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
                            this.drag = Some(DragKind::OutlineScroll {
                                grab: (y - cur_top).clamp(0.0, thumb),
                            });
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn outline_row(
        &self,
        row: &crate::outline::OutlineRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::outline::RowKind;
        let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
        let display = n.saturating_sub(1).saturating_sub(row.layer_index);
        let active_layer = row.kind == RowKind::Layer
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.active == row.layer_index);
        let direct = self.selected.iter().any(|s| s == &row.id);
        let layer_is_selection = active_layer && self.selected.is_empty();
        let holds_selection = self.pin_ids().iter().any(|id| id == &row.id);
        let nested = !direct
            && !layer_is_selection
            && (holds_selection
                || row
                    .ancestors
                    .iter()
                    .any(|a| self.selected.iter().any(|s| s == a)));
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
        let dragging = row.kind == RowKind::Layer && drag_from == Some(display);
        let show_line = row.kind == RowKind::Layer && line_at == Some(display);
        let id = row.id.clone();
        let layer_index = row.layer_index;
        let expandable = row.expandable;
        let open = row.open;
        let visible = row.visible;
        let kind = row.kind;
        let menu_kind = kind;
        let menu_id = id.clone();
        div()
            .id(SharedString::from(format!("vrow-{}", row.id)))
            .relative()
            .h(px(32.))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pl(px(row.depth as f32 * 12.0))
            .pr_1()
            .rounded_sm()
            .when(direct || layer_is_selection, |d| {
                d.bg(ui_kit::chrome(0xdbeafe))
            })
            .when(nested, |d| d.bg(ui_kit::chrome(0xeff6ff)))
            .when(
                active_layer && !direct && !layer_is_selection && !holds_selection,
                |d| d.bg(ui_kit::chrome(0xe2e8f0)),
            )
            .when(dragging, |d| d.opacity(0.35))
            .when(show_line && !line_after, |d| {
                d.border_t_2().border_color(ui_kit::chrome(0xf59e0b))
            })
            .when(show_line && line_after, |d| {
                d.border_b_2().border_color(ui_kit::chrome(0xf59e0b))
            })
            .when(row.kind == RowKind::Layer, |d| {
                d.child(Self::measure_layer_bounds(cx.entity(), display))
            })
            .child({
                let entity = cx.entity().clone();
                let hit_id = menu_id.clone();
                let hit_kind = menu_kind;
                let hit_layer = layer_index;
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| {
                            this.outline_row_hits.push(OutlineRowHit {
                                id: hit_id,
                                kind: hit_kind,
                                layer_index: hit_layer,
                                bounds,
                            });
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full()
            })
            .child(self.outline_twist(id.clone(), expandable, open, cx))
            .child(self.outline_eye(&id, visible, cx))
            .child(self.outline_thumb(row).when(!row.shown, |d| d.opacity(0.4)))
            .child(
                self.outline_label(row, display, kind, cx)
                    .when(!row.shown, |d| d.opacity(0.45)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_outline_menu(
                        menu_id.clone(),
                        menu_kind,
                        layer_index,
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                }),
            )
            .when(kind != RowKind::Layer, |d| {
                let id = id.clone();
                d.on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                        if std::mem::take(&mut this.suppress_outline_click) {
                            return;
                        }
                        this.pick_outline(
                            id.clone(),
                            layer_index,
                            ev.modifiers.shift,
                            apply_bg::is_primary_mod(&ev.modifiers),
                            cx,
                        );
                    }),
                )
            })
            .when(kind == RowKind::Layer, |d| {
                d.on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if std::mem::take(&mut this.suppress_outline_click) {
                            return;
                        }
                        if this.drag.is_none() {
                            this.select_layer(layer_index, cx);
                        }
                    }),
                )
            })
    }

    fn outline_twist(
        &self,
        id: String,
        expandable: bool,
        open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(SharedString::from(format!("vtw-{id}")))
            .w(px(14.))
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .text_xs()
            .when(expandable, |d| {
                d.cursor_pointer().child(if open { "▾" } else { "▸" })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if expandable {
                        this.toggle_expanded(id.clone(), cx);
                    }
                }),
            )
    }

    fn outline_eye(&self, id: &str, open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let id = id.to_string();
        div()
            .id(SharedString::from(format!("veye-{id}")))
            .w(px(18.))
            .h(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .cursor_pointer()
            .child(apply_bg::palette::eye_icon(open, ui_kit::chrome(0x334155)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_row_visible(&id, cx);
                }),
            )
    }

    fn outline_thumb(&self, row: &crate::outline::OutlineRow) -> gpui::Div {
        let light = row.ink.is_some_and(|c| {
            0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32 >= 170.0
        });
        let mut slot = div()
            .w(px(28.))
            .h(px(22.))
            .flex_shrink_0()
            .rounded_sm()
            .overflow_hidden()
            .bg(rgb(if light { 0x242428 } else { 0xf4f4f5 }))
            .flex()
            .items_center()
            .justify_center();
        if let Some(tex) = self.thumbs.get(&row.id).and_then(|slot| slot.tex.clone()) {
            slot = slot.child(
                img(tex)
                    .w(px(28.))
                    .h(px(22.))
                    .object_fit(ObjectFit::Contain),
            );
        } else if let Some(ink) = row.ink {
            let color = (ink[0] as u32) << 16 | (ink[1] as u32) << 8 | ink[2] as u32;
            slot = slot.child(div().w(px(12.)).h(px(8.)).rounded_sm().bg(rgb(color)));
        }
        slot
    }

    fn outline_label(
        &self,
        row: &crate::outline::OutlineRow,
        display: usize,
        kind: crate::outline::RowKind,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        use crate::outline::RowKind;
        let renaming = self.rename_target.as_deref() == Some(row.id.as_str());
        let name = row.name.clone();
        let entity = cx.entity().clone();
        div()
            .id(SharedString::from(format!("vname-{}", row.id)))
            .relative()
            .flex_1()
            .min_w(px(0.))
            .text_xs()
            .truncate()
            .when(!renaming, |d| d.cursor_pointer())
            .when(renaming, |d| {
                d.child(
                    canvas(
                        move |bounds, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.rename_bounds = bounds;
                                if !this.rename_focus {
                                    return;
                                }
                                let handle = this.rename_input.read(cx).focus_handle(cx);
                                handle.focus(window);
                                this.rename_input.update(cx, |input, _| {
                                    let _ = input.take_blur_commit();
                                });
                                this.rename_focus = false;
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(self.rename_input.clone())
            })
            .when(!renaming, |d| d.child(name))
            .when(!renaming && kind == RowKind::Layer, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
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
            })
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
            return div().absolute().into_any_element();
        };
        let n = self.doc.as_ref().map(|d| d.layers.len()).unwrap_or(0);
        let label = if n > 0 && *from < n {
            let storage = n - 1 - from;
            self.doc
                .as_ref()
                .and_then(|d| d.layers.get(storage))
                .map(|l| format!("{} ({})", l.name, l.objects.len()))
                .unwrap_or_else(|| "...".into())
        } else {
            "...".into()
        };
        let layer = self.picker_layer_bounds;
        let gx = *origin_x + (*x - *start_x) - f32::from(layer.origin.x);
        let gy = *origin_y + (*y - *start_y) - f32::from(layer.origin.y);
        div()
            .id("vector-layer-drag-ghost")
            .absolute()
            .left(px(gx))
            .top(px(gy))
            .opacity(0.72)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(ui_kit::card())
            .text_color(ui_kit::chrome(0x0f172a))
            .text_xs()
            .border_1()
            .border_color(ui_kit::chrome(0x94a3b8))
            .whitespace_nowrap()
            .child(label)
            .into_any_element()
    }

    fn stroke_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.frac_slider(
            "v-stroke-w",
            stroke_to_t(self.style.width),
            format!("线宽 {:.2}", self.style.width),
            SliderKind::Width,
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
        let (cap_at, cap_ml) = caption_shift(frac);
        div()
            .relative()
            .w_full()
            .h(px(28.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.drag = Some(DragKind::Slider(kind));
                    this.set_slider_from_x(f32::from(ev.position.x), kind, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::Slider(k)) if k == kind) {
                    this.set_slider_from_x(f32::from(ev.position.x), kind, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if matches!(this.drag, Some(DragKind::Slider(k)) if k == kind) {
                        this.finish_slider(cx);
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(cap_at))
                    .bottom(px(16.))
                    .ml(px(cap_ml))
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(ui_kit::chrome(0x64748b))
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
                    .bg(ui_kit::chrome(0xe2e8f0))
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .overflow_hidden()
                    .child(
                        canvas(
                            {
                                let entity = cx.entity().clone();
                                move |bounds, _, cx| {
                                    entity
                                        .update(cx, |this, _| this.note_slider_track(kind, bounds));
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
                            .bg(ui_kit::chrome(0x2563eb))
                            .rounded_full(),
                    ),
            )
    }

    pub(crate) fn note_slider_track(&mut self, kind: SliderKind, bounds: Bounds<Pixels>) {
        match kind {
            SliderKind::Width => self.width_track = bounds,
        }
    }

    pub(crate) fn set_slider_from_x(&mut self, x: f32, kind: SliderKind, cx: &mut Context<Self>) {
        let track = match kind {
            SliderKind::Width => self.width_track,
        };
        let left = f32::from(track.origin.x);
        let width = f32::from(track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        match kind {
            SliderKind::Width => self.set_stroke_width(stroke_from_t(t), cx),
        }
    }

    pub(crate) fn finish_slider(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.width_undo_held = false;
        self.notify_chrome(cx);
    }

    pub(crate) fn set_stroke_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(STROKE_MIN, STROKE_MAX);
        if (self.style.width - width).abs() < 0.001 && (self.brush - width).abs() < 0.001 {
            return;
        }
        self.style.width = width;
        self.brush = width;
        if !self.selected.is_empty() {
            let dragging = matches!(self.drag, Some(DragKind::Slider(SliderKind::Width)));
            if dragging {
                if !self.width_undo_held {
                    self.push_undo();
                    self.width_undo_held = true;
                }
            } else {
                self.push_undo();
            }
            tools::write_style(self);
            self.rebuild_raster(cx);
        }
        self.notify_chrome(cx);
    }

    pub(crate) fn bump_stroke_width(&mut self, bigger: bool, cx: &mut Context<Self>) {
        let t = stroke_to_t(self.style.width);
        let next = stroke_from_t(if bigger { t + 0.06 } else { t - 0.06 });
        self.set_stroke_width(next, cx);
    }

    fn paint_proxy(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fill_on = self.paint_on_fill;
        let back = if fill_on {
            self.paint_square(false, cx)
        } else {
            self.paint_square(true, cx)
        };
        let front = if fill_on {
            self.paint_square(true, cx)
        } else {
            self.paint_square(false, cx)
        };
        let entity = cx.entity().clone();
        div()
            .relative()
            .w(px(32.))
            .h(px(32.))
            .flex_shrink_0()
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.swap_paint(cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, cx| {
                            let prev = this.proxy_bounds;
                            this.proxy_bounds = bounds;
                            let moved = (f32::from(prev.origin.x) - f32::from(bounds.origin.x))
                                .abs()
                                > 0.5
                                || (f32::from(prev.origin.y) - f32::from(bounds.origin.y)).abs()
                                    > 0.5;
                            if moved && this.palette_open {
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(back)
            .child(front)
    }

    fn paint_square(&self, fill: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let rgba = if fill {
            self.style.fill
        } else {
            self.style.stroke
        };
        let (left, top) = if fill { (0.0, 0.0) } else { (12.0, 12.0) };
        // 填充铺满内部. 描边只留一圈, 中间永远是格子, 白和透明也看得出是框.
        let mut square = div()
            .id(if fill {
                "v-proxy-fill"
            } else {
                "v-proxy-stroke"
            })
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(20.))
            .h(px(20.))
            .overflow_hidden()
            .border_1()
            .border_color(ui_kit::chrome(0x334155))
            .cursor_pointer()
            .child(apply_bg::palette::checkerboard(4.0));
        if let Some(c) = rgba {
            square = square.child(div().absolute().inset_0().bg(chip_rgba(c)));
        }
        if !fill {
            square = square.child(stroke_hole());
        }
        if rgba.is_none() {
            square = square.child(none_slash());
        }
        square
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    this.on_paint_proxy(fill, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.on_paint_proxy_right(fill, cx);
                }),
            )
    }
}

fn chip_rgba(c: [u8; 4]) -> gpui::Rgba {
    gpui::rgba(
        (u32::from(c[0]) << 24)
            | (u32::from(c[1]) << 16)
            | (u32::from(c[2]) << 8)
            | u32::from(c[3]),
    )
}

fn stroke_hole() -> impl IntoElement {
    div()
        .absolute()
        .top(px(4.))
        .left(px(4.))
        .right(px(4.))
        .bottom(px(4.))
        .border_1()
        .border_color(ui_kit::chrome(0x334155))
        .overflow_hidden()
        .child(apply_bg::palette::checkerboard(4.0))
}

fn caption_shift(frac: f32) -> (f32, f32) {
    let frac = frac.clamp(0.0, 1.0);
    if frac < 0.42 {
        (frac, 0.0)
    } else if frac > 0.7 {
        (frac, -52.0)
    } else {
        (frac, -20.0)
    }
}

fn none_slash() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |bounds, _, window, _| {
            let x0 = bounds.origin.x + px(2.);
            let y0 = bounds.origin.y + bounds.size.height - px(2.);
            let x1 = bounds.origin.x + bounds.size.width - px(2.);
            let y1 = bounds.origin.y + px(2.);
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(point(x0, y0));
            path.line_to(point(x1, y1));
            if let Ok(path) = path.build() {
                window.paint_path(path, ui_kit::chrome(0xef4444));
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}
