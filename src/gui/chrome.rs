//! 工具栏、工作区、工程面板、对话框.

use super::ScoreSyncApp;
use super::*;

impl ScoreSyncApp {
    pub(super) fn show_help(&mut self, cx: &mut Context<Self>) {
        if self.page_organize.is_some() {
            self.close_page_organize(cx);
        }
        if self.settings_open {
            self.close_settings(cx);
        }
        self.drag = None;
        self.dialog = Some(DialogKind::Help);
        cx.notify();
    }

    pub(super) fn show_error(
        &mut self,
        title: impl Into<String>,
        err: impl std::fmt::Display,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(DialogKind::Info {
            title: title.into(),
            body: err.to_string(),
        });
        cx.notify();
    }

    pub(super) fn has_modal_overlay(&self, cx: &App) -> bool {
        self.pdf_import.is_some()
            || self.page_organize.is_some()
            ||         self.dialog.is_some()
            || self.settings_open
            || self.bg.pick_open
            || self.bg.batch_open
            || self.apply_bg.read(cx).is_error_open()
            || self.score_video.read(cx).is_error_open()
            || self.score_video.read(cx).is_export_open()
    }

    pub(super) fn header_file_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("header_file_bar")
            .flex_shrink_0()
            .h(px(28.))
            .flex()
            .flex_row()
            .child(self.header_icon_btn(
                "hdr-new",
                "header/new.svg",
                ui_kit::always_hint(ui_kit::t("chrome.new"), "app.new").into(),
                |this, window, cx| this.request_new_project(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-open",
                "header/open-project.svg",
                ui_kit::always_hint(ui_kit::t("chrome.open_project"), "app.open_project").into(),
                |this, window, cx| this.open_project(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-save",
                "header/save.svg",
                ui_kit::always_hint(ui_kit::t("chrome.save"), "app.save").into(),
                |this, window, cx| this.save_project(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-saveas",
                "header/save-as.svg",
                ui_kit::always_hint(ui_kit::t("chrome.save_as"), "app.save_as").into(),
                |this, window, cx| this.save_project_as(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-help",
                "header/help.svg",
                ui_kit::always_hint(ui_kit::t("chrome.help"), "app.help").into(),
                |this, _, cx| this.show_help(cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-settings",
                "header/setting.svg",
                ui_kit::always_hint(ui_kit::t("chrome.settings"), "app.settings").into(),
                |this, _, cx| this.open_settings(cx),
                cx,
            ))
    }

    fn header_icon_btn(
        &self,
        id: &'static str,
        icon: &'static str,
        tip: SharedString,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let tip_hover = tip.clone();
        div()
            .id(id)
            .relative()
            .flex_shrink_0()
            .w(px(28.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(ui_kit::chrome(0xd8e0ea)))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| {
                            this.header_btn_bounds.insert(id.to_string(), bounds);
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(
                gpui::svg()
                    .path(icon)
                    .w(px(16.))
                    .h(px(16.))
                    .text_color(ui_kit::chrome(0x334155)),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.note_header_hover(id, tip_hover.clone(), cx);
                } else if this.header_hover_id == Some(id) {
                    this.release_tab_hover(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.clear_tab_hover(cx);
                    on_click(this, window, cx);
                }),
            )
    }

    fn note_header_hover(&mut self, id: &'static str, text: SharedString, cx: &mut Context<Self>) {
        if self.header_hover_id == Some(id) {
            return;
        }
        let warm = self.tooltip_warm;
        self.tab_hover_idx = None;
        self.header_hover_id = Some(id);
        self.spread_title_hover = None;
        self.tab_hover_gen = self.tab_hover_gen.wrapping_add(1);
        let gen = self.tab_hover_gen;
        if warm {
            self.show_header_tooltip(id, text, cx);
            return;
        }
        self.tab_tooltip = None;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            this.update(cx, |view, cx| {
                if view.tab_hover_gen != gen || view.header_hover_id != Some(id) {
                    return;
                }
                view.show_header_tooltip(id, text, cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_header_tooltip(&mut self, id: &'static str, text: SharedString, cx: &mut Context<Self>) {
        let (ax, ay, aw, ah) = self
            .header_btn_bounds
            .get(id)
            .map(|b| {
                (
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                )
            })
            .unwrap_or((0.0, 0.0, 0.0, 0.0));
        self.publish_hover_tooltip(
            TabTooltip {
                id: id.into(),
                anchor_x: ax,
                anchor_y: ay,
                anchor_w: aw,
                anchor_h: ah,
                text,
                measured_w: 0.0,
                measured_h: 0.0,
            },
            cx,
        );
    }

    pub(super) fn btn(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active { ui_kit::chrome(0x2563eb) } else { ui_kit::chrome(0xe2e8f0) };
        let fg = if active { rgb(0xffffff) } else { ui_kit::chrome(0x0f172a) };
        let hover = if active { ui_kit::chrome(0x1d4ed8) } else { ui_kit::chrome(0xcbd5e1) };
        let id: SharedString = id.into();
        let id_down = id.clone();
        let id_up = id.clone();
        let id_out = id.clone();
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(ui_kit::chrome(0x94a3b8))
            .text_color(fg)
            .text_sm()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(label.into())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.btn_press = Some(id_down.clone());
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    let press = this.btn_press.take();
                    if press.as_ref() != Some(&id_up) {
                        return;
                    }
                    on_click(this, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    if this.btn_press.as_ref() == Some(&id_out) {
                        this.btn_press = None;
                    }
                }),
            )
    }

    pub(super) fn menu_item(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let fg = if active { ui_kit::chrome(0x0f172a) } else { ui_kit::chrome(0x334155) };
        div()
            .id(id.into())
            .flex_shrink_0()
            .h_full()
            .px_2()
            .flex()
            .items_center()
            .when(active, |d| {
                d.bg(ui_kit::chrome(0xd8e0ea)).font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .text_color(fg)
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |s| if active { s } else { s.bg(ui_kit::chrome(0xd8e0ea)) })
            .child(label.into())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn spread_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("crop_toolbar")
            .flex_shrink_0()
            .flex()
            .flex_row()
            .h(px(28.))
            .w_full()
            .min_w(px(0.))
            .overflow_x_scroll()
            .bg(ui_kit::chrome(0xe2e8f0))
            .border_b_1()
            .border_color(ui_kit::chrome(0xcbd5e1))
            .child(self.menu_item(
                "open",
                ui_kit::menu_label(ui_kit::t("crop.open_file"), "crop.open_file"),
                false,
                Self::open_file,
                cx,
            ))
            .child(self.menu_item(
                "spread_dup",
                ui_kit::t("spread.dup"),
                false,
                |this, _, cx| this.duplicate_spread_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "spread_paste",
                ui_kit::menu_label(ui_kit::t("spread.paste"), "spread.paste"),
                false,
                |this, _, cx| this.paste_spread_clipboard(cx),
                cx,
            ))
            .child(self.menu_item(
                "spread_del",
                ui_kit::menu_label(ui_kit::t("spread.delete"), "crop.delete"),
                false,
                |this, _, cx| this.delete_spread_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "spread_blank",
                ui_kit::t("spread.blank"),
                false,
                |this, _, cx| {
                    let idx = this.doc.current_page_index.min(this.doc.pages.len());
                    this.insert_spread_blank(idx, cx);
                },
                cx,
            ))
    }

    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.doc.is_spread() {
            return self.spread_toolbar(cx).into_any_element();
        }
        // 分块菜单: 放在左栏顶上一行, 内嵌菜单项对齐视频轨运输条.
        div()
            .id("crop_toolbar")
            .flex_shrink_0()
            .flex()
            .flex_row()
            .h(px(28.))
            .w_full()
            .min_w(px(0.))
            .overflow_x_scroll()
            .bg(ui_kit::chrome(0xe2e8f0))
            .border_b_1()
            .border_color(ui_kit::chrome(0xcbd5e1))
            .child(self.menu_item(
                "open",
                ui_kit::menu_label(ui_kit::t("crop.open_file"), "crop.open_file"),
                false,
                Self::open_file,
                cx,
            ))
            .child(self.menu_item(
                "detect",
                ui_kit::menu_label(ui_kit::t("crop.detect"), "crop.detect"),
                false,
                |this, _, cx| this.run_detect(cx),
                cx,
            ))
            .child(self.menu_item(
                "detect_all",
                ui_kit::menu_label(ui_kit::t("crop.detect_all"), "crop.detect_all"),
                false,
                |this, _, cx| this.run_detect_all(cx),
                cx,
            ))
            .child(self.menu_item(
                "add_block",
                ui_kit::menu_label(ui_kit::t("crop.add"), "crop.add"),
                self.canvas_tool == CanvasTool::AddBlock,
                |this, _, cx| this.toggle_add_block(cx),
                cx,
            ))
            .child(self.menu_item(
                "split_block",
                ui_kit::menu_label(ui_kit::t("crop.split"), "crop.split"),
                self.canvas_tool == CanvasTool::SplitBlock,
                |this, _, cx| this.toggle_split_block(cx),
                cx,
            ))
            .child(self.menu_item(
                "merge",
                ui_kit::menu_label(ui_kit::t("crop.merge"), "crop.merge"),
                false,
                |this, _, cx| this.merge_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "ungroup",
                ui_kit::menu_label(ui_kit::t("crop.ungroup"), "crop.ungroup"),
                false,
                |this, _, cx| this.ungroup_active(cx),
                cx,
            ))
            .child(self.menu_item(
                "share",
                ui_kit::menu_label(ui_kit::t("crop.share"), "crop.share"),
                false,
                |this, _, cx| this.share_into_group(cx),
                cx,
            ))
            .child(self.menu_item(
                "del",
                ui_kit::menu_label(ui_kit::t("crop.delete"), "crop.delete"),
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "export",
                ui_kit::menu_label(ui_kit::t("crop.export"), "crop.export"),
                false,
                Self::export_groups_ui,
                cx,
            ))
            .child(self.menu_item(
                "reset",
                ui_kit::menu_label(ui_kit::t("crop.reset"), "crop.reset"),
                false,
                |this, _, cx| this.reset_groups(cx),
                cx,
            ))
            .child(self.menu_item(
                "organize_pages",
                ui_kit::menu_label(ui_kit::t("organize.toggle"), "organize.toggle"),
                false,
                |this, window, cx| this.toggle_page_organize(window, cx),
                cx,
            ))
            .into_any_element()
    }

    fn mask_photo_menu_btn(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("mask_photo_btn")
            .flex_shrink_0()
            .h(px(22.))
            .px_2()
            .flex()
            .items_center()
            .rounded_sm()
            .text_color(ui_kit::chrome(0x334155))
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.bg(ui_kit::chrome(0xd8e0ea)))
            .child(ui_kit::menu_label(ui_kit::t("mask.photo"), "mask.photo"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.try_open_photo_edit(window, cx);
                }),
            )
    }

    pub(super) fn tool_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let crop_on = self.side_tool == SideTool::Crop;
        let mask_on = self.side_tool == SideTool::Mask;
        let proj_on = self.side_tool == SideTool::Project;
        let video_on = self.side_tool == SideTool::Video;
        div()
            .id("tool_switcher")
            .flex()
            .flex_row()
            .h(px(28.))
            .w_full()
            .bg(ui_kit::chrome(0xe2e8f0))
            .border_b_1()
            .border_color(ui_kit::chrome(0xcbd5e1))
            .child(self.tool_tab("tool_crop", ui_kit::t_src("分块"), crop_on, SideTool::Crop, cx))
            .child(self.tool_tab("tool_proj", ui_kit::t_src("底色"), proj_on, SideTool::Project, cx))
            .child(self.tool_tab("tool_mask", ui_kit::t_src("蒙版"), mask_on, SideTool::Mask, cx))
            .child(self.tool_tab(
                "tool_video",
                ui_kit::t_src("视频"),
                video_on,
                SideTool::Video,
                cx,
            ))
    }

    pub(super) fn tool_tab(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        tool: SideTool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = label.into();
        let fg = if active { ui_kit::chrome(0x0f172a) } else { ui_kit::chrome(0x475569) };
        div()
            .id(id)
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .when(active, |d| {
                d.bg(ui_kit::chrome(0xf8fafc)).font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .text_color(fg)
            .text_sm()
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |s| if active { s } else { s.bg(ui_kit::chrome(0xd8e0ea)) })
            .child(label)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.set_side_tool(tool, window, cx);
                }),
            )
    }

    pub(super) fn left_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.photo_open() {
            let vector = self.vector_session();
            let tb = if vector {
                self.vector_edit
                    .update(cx, |p, cx| p.toolbar_embedded(cx).into_any_element())
            } else {
                self.photo_edit
                    .update(cx, |p, cx| p.toolbar_embedded(cx).into_any_element())
            };
            let canvas = if vector {
                self.vector_edit.clone().into_any_element()
            } else {
                self.photo_edit.clone().into_any_element()
            };
            return div()
                .id("left_workspace")
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .overflow_hidden()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.focus_photo_editor(window, cx);
                    }),
                )
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .h(px(28.))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .border_b_1()
                        .border_color(ui_kit::chrome(0xcbd5e1))
                        .bg(ui_kit::chrome(0xe2e8f0))
                        .child(tb),
                )
                .child(
                    div()
                        .id("photo_canvas_host")
                        .flex_1()
                        .min_h(px(0.))
                        .min_w(px(0.))
                        .overflow_hidden()
                        .child(canvas),
                )
                .into_any_element();
        }
        if self.side_tool == SideTool::Video {
            // 视频栏不用页签, 而是预览窗 + 轨道, 占满整个左侧工作区.
            let canvas = self
                .score_video
                .update(cx, |v, cx| v.left_panel(cx))
                .into_any_element();
            return div()
                .id("left_workspace")
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(canvas)
                .into_any_element();
        }
        if self.side_tool == SideTool::Project {
            // 底色: 左侧用蒙版同款预览 (只读), 设置在右侧面板.
            let canvas = self
                .mask_tool
                .update(cx, |m, cx| m.image_view(cx))
                .into_any_element();
            return div()
                .id("left_workspace")
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(ui_kit::chrome(0xcbd5e1))
                        .bg(ui_kit::chrome(0xf8fafc))
                        .child(self.tab_bar(cx)),
                )
                .child(div().flex_1().min_h(px(0.)).min_w(px(0.)).child(canvas))
                .into_any_element();
        }
        let spread = self.side_tool == SideTool::Crop && self.doc.is_spread();
        let canvas = if spread {
            self.spread_workspace(cx).into_any_element()
        } else {
            match self.side_tool {
                SideTool::Crop => self.image_view(cx).into_any_element(),
                SideTool::Mask => self
                    .mask_tool
                    .update(cx, |m, cx| m.image_view(cx))
                    .into_any_element(),
                SideTool::Project | SideTool::Video => unreachable!(),
            }
        };
        div()
            .id("left_workspace")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .when(self.side_tool == SideTool::Crop, |d| {
                d.child(self.toolbar(cx))
            })
            .when(self.side_tool == SideTool::Mask, |d| {
                // 蒙版面板顶部菜单栏 (导出本块/适应/删除 + 辅助线/对齐).
                let tb = self
                    .mask_tool
                    .update(cx, |m, cx| m.toolbar_embedded(cx).into_any_element());
                d.child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .h(px(28.))
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .border_b_1()
                        .border_color(ui_kit::chrome(0xcbd5e1))
                        .bg(ui_kit::chrome(0xe2e8f0))
                        .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(tb)),
                )
            })
            .when(!spread, |d| {
                d.child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(ui_kit::chrome(0xcbd5e1))
                        .bg(ui_kit::chrome(0xf8fafc))
                        .child(self.tab_bar(cx)),
                )
            })
            .child(
                // 定高的列: 子元素 flex_1 / h_full 才能落到视口, 而不是按内容把整块撑高.
                // 对开网格若跟着内容长高, 滚轮和右侧滑块都没有可滚距离.
                div()
                    .id("left_canvas_host")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(canvas),
            )
            .into_any_element()
    }

    /// 「组合分块」面板: 显示当前蒙版编辑目标 (组合拼合图) 内自上而下的
    /// 各成员分块, 供选中/后续拖动调整. 与顶部组合页签栏不同, 这里列的是
    /// 「块」而非「组合」, 不再重复.
    pub(super) fn mask_block_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.mask_block_rows();
        let mut list = div()
            .id("mask_block_list")
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .bg(ui_kit::card())
            .border_1()
            .border_color(ui_kit::chrome(0xcbd5e1))
            .rounded_md()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        if rows.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(ui_kit::chrome(0x94a3b8))
                    .child(if self.doc.is_spread() {
                        ui_kit::t_src("当前对开没有页面")
                    } else {
                        ui_kit::t_src("当前组合内没有分块")
                    }),
            );
        }
        for row in &rows {
            let rid = row.id.clone();
            let active = row.selected;
            let bg = if active { ui_kit::chrome(0x2563eb) } else { ui_kit::chrome(0xe2e8f0) };
            let fg = if active { rgb(0xffffff) } else { ui_kit::chrome(0x0f172a) };
            list = list.child(
                div()
                    .id(SharedString::from(format!("mask-blk-{rid}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(bg)
                    .text_color(fg)
                    .text_xs()
                    .cursor_pointer()
                    .w_full()
                    .child(row.label.clone())
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.mask_active_block_id = Some(rid.clone());
                            this.mask_tool
                                .update(cx, |m, cx| m.select_block(Some(rid.clone()), cx));
                            cx.notify();
                        }),
                    ),
            );
        }

        div()
            .id("mask_block_panel")
            .flex_shrink_0()
            .h(px(168.))
            .max_h(px(168.))
            .px_2()
            .pt_2()
            .pb_1()
            .border_b_1()
            .border_color(ui_kit::chrome(0xcbd5e1))
            .bg(ui_kit::chrome(0xf8fafc))
            .flex()
            .flex_col()
            .min_h(px(0.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .mb_1()
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(ui_kit::chrome(0x334155))
                            .flex_1()
                            .min_w(px(0.))
                            .child(if self.doc.is_spread() {
                                ui_kit::t_src("对开页面 (左页 / 右页)")
                            } else {
                                ui_kit::t_src("组合分块 (自上而下)")
                            }),
                    )
                    .child(self.mask_photo_menu_btn(cx)),
            )
            .child(
                self.attach_scrollbars(
                    "mask_block_scroll_wrap".into(),
                    ScrollList::MaskBlock,
                    &self.mask_block_scroll,
                    list,
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            )
    }

    pub(super) fn right_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.photo_open() {
            if self.vector_session() {
                self.vector_edit
                    .update(cx, |p, cx| p.side_panel(cx).into_any_element())
            } else {
                self.photo_edit
                    .update(cx, |p, cx| p.side_panel(cx).into_any_element())
            }
        } else {
            match self.side_tool {
                SideTool::Crop => self.side_panel(cx).into_any_element(),
                SideTool::Mask => {
                    let picker = self.mask_block_panel(cx).into_any_element();
                    let side_w = self.side_width;
                    let mask_body = self.mask_tool.update(cx, |m, cx| {
                        m.set_embed_side_width(side_w);
                        div()
                            .id("mask_right_body")
                            .w_full()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(m.side_panel(cx))
                            .into_any_element()
                    });
                    div()
                        .id("mask_right")
                        .w_full()
                        .h_full()
                        .flex()
                        .flex_col()
                        .min_h(px(0.))
                        .child(picker)
                        .child(mask_body)
                        .into_any_element()
                }
                SideTool::Project => self.bg_side_panel(cx).into_any_element(),
                SideTool::Video => self
                    .score_video
                    .update(cx, |v, cx| v.right_panel(cx))
                    .into_any_element(),
            }
        };
        div()
            .id("right_workspace")
            .w(px(self.side_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(ui_kit::chrome(0xf1f5f9))
            .child(div().flex_shrink_0().child(self.tool_switcher(cx)))
            .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(body))
    }

    pub(super) fn clear_project_bg(&mut self, cx: &mut Context<Self>) {
        if !self.doc.bg_enabled && self.doc.bg_image.is_none() && self.doc.bg_solid.is_none() {
            self.status = ui_kit::t_src("当前未启用工程底色层.");
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.doc.clear_project_bg();
        self.bg.applied_is_solid = false;
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        self.status = ui_kit::t_src("已取消工程底色层.");
        self.hint = self.status.clone();
        self.refresh_bg_preview_layer(cx);
        self.sync_video_pool(cx);
        cx.notify();
    }

    /// 强制重新拼合并加载蒙版预览图 (绕过 `load_rgb` 的 session_key 缓存),
    /// 用于底色启用/取消后需要刷新预览的场景. 会先落盘当前蒙版编辑, 再清空
    /// 内嵌工具视图, 避免清空动作把待落盘的蒙版一并清没.
    #[allow(dead_code)]
    pub(super) fn force_refresh_mask_preview(&mut self, cx: &mut Context<Self>) {
        self.flush_mask_to_doc(cx);
        self.mask_tool.update(cx, |m, cx| m.clear_view("", cx));
        self.mask_target = None;
        self.sync_mask_image(cx);
    }
    pub(super) fn dialog_overlay(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 关窗/新建确认必须盖过子面板错误, 否则「确定」被挡住也退不出.
        if matches!(self.dialog, Some(DialogKind::UnsavedExit)) {
            return self.unsaved_exit_dialog(cx).into_any_element();
        }
        if matches!(self.dialog, Some(DialogKind::UnsavedNew)) {
            return self.unsaved_new_dialog(cx).into_any_element();
        }
        if matches!(self.dialog, Some(DialogKind::UnsavedPhoto { .. })) {
            return self.unsaved_photo_dialog(cx).into_any_element();
        }
        if matches!(self.dialog, Some(DialogKind::SwitchLayout { .. })) {
            return self.switch_layout_dialog(cx).into_any_element();
        }
        if matches!(self.dialog, Some(DialogKind::UpdateAvailable { .. })) {
            return self.update_available_dialog(cx).into_any_element();
        }
        if self.settings_open {
            return self.settings_overlay(cx).into_any_element();
        }
        if self.bg.pick_open {
            return self.bg_pick_overlay(cx).into_any_element();
        }
        if self.bg.batch_open {
            return self.apply_bg_batch_overlay(cx).into_any_element();
        }
        if self.pdf_import.is_some() {
            return self.pdf_import_overlay(cx).into_any_element();
        }
        if self.page_organize.is_some() {
            return self.page_organize_overlay(window, cx).into_any_element();
        }
        if self.score_video.read(cx).is_export_open() {
            return self
                .score_video
                .update(cx, |v, cx| v.export_dialog(cx).into_any_element());
        }
        if self.score_video.read(cx).is_error_open() {
            return self
                .score_video
                .update(cx, |v, cx| v.error_dialog(cx).into_any_element());
        }
        if self.apply_bg.read(cx).is_error_open() {
            return self
                .apply_bg
                .update(cx, |v, cx| v.error_dialog(cx).into_any_element());
        }
        let Some(ref dlg) = self.dialog else {
            return div().into_any_element();
        };
        let (title, body) = match dlg {
            DialogKind::Help => (ui_kit::t("dialog.help_title").to_string(), help_text()),
            DialogKind::Info { title, body } => (title.clone(), body.clone()),
            DialogKind::UnsavedExit
            | DialogKind::UnsavedNew
            | DialogKind::UnsavedPhoto { .. }
            | DialogKind::SwitchLayout { .. }
            | DialogKind::UpdateAvailable { .. } => unreachable!(),
        };
        let body_el = div()
            .id("dlg_body")
            .text_sm()
            .text_color(ui_kit::chrome(0x334155))
            .whitespace_normal()
            .child(body);

        div()
            .id("dialog_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            // 阻断背后命中; move/up 留给本层处理 Help 滚动条拖动
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                    this.apply_scrollbar_drag(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                }
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                        this.drag = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("dialog_card")
                    .w(px(520.))
                    .max_h(px(520.))
                    .p_4()
                    .rounded_lg()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        self.attach_scrollbars(
                            "help_scroll_wrap".into(),
                            ScrollList::Help,
                            &self.help_scroll,
                            body_el,
                            cx,
                        )
                        .flex_1()
                        .min_h(px(0.)),
                    )
                    .child(div().flex_shrink_0().child(self.btn(
                        "dlg_ok",
                        ui_kit::t("dialog.ok"),
                        true,
                        |this, _, cx| {
                            this.dismiss_dialog(cx);
                        },
                        cx,
                    ))),
            )
            .into_any_element()
    }

    pub(super) fn unsaved_exit_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("dialog_backdrop_unsaved")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .id("dialog_card_unsaved")
                    .w(px(420.))
                    .p_4()
                    .rounded_lg()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(ui_kit::t_src("未保存的改动")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui_kit::chrome(0x334155))
                            .child(ui_kit::t_src("当前工程有未保存改动. 要在退出前保存吗?")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "exit_save",
                                ui_kit::t_src("保存并退出"),
                                true,
                                |this, window, cx| {
                                    // 保持 UnsavedExit 标记, 供保存成功后 quit
                                    if this.project_path.is_some() {
                                        this.save_project(window, cx);
                                    } else {
                                        this.save_project_as(window, cx);
                                    }
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "exit_discard",
                                ui_kit::t_src("不保存退出"),
                                false,
                                |this, _, cx| {
                                    this.dialog = None;
                                    this.dirty = false;
                                    this.allow_close = true;
                                    cx.quit();
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "exit_cancel",
                                ui_kit::t("dialog.cancel"),
                                false,
                                |this, _, cx| {
                                    this.dismiss_dialog(cx);
                                },
                                cx,
                            )),
                    ),
            )
    }

    fn switch_layout_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let to_spread = matches!(
            self.dialog,
            Some(DialogKind::SwitchLayout { to_spread: true })
        );
        let body = if to_spread {
            ui_kit::t_src("会按页序把每页收成一整页, 两两重配对开. 旧的蒙版, 分块微调和视频里对不上的片段会丢掉.")
        } else {
            ui_kit::t_src("会清掉对开组合, 页面上的分块留空, 需要重新识别. 旧的蒙版, 分块微调和视频里对不上的片段会丢掉.")
        };
        div()
            .id("dialog_backdrop_switch_layout")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .w(px(440.))
                    .p_4()
                    .rounded_lg()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(if to_spread {
                                ui_kit::t_src("切换为双页对开")
                            } else {
                                ui_kit::t_src("切换为谱表分块")
                            }),
                    )
                    .child(div().text_sm().text_color(ui_kit::chrome(0x334155)).child(body))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "layout_ok",
                                ui_kit::t_src("重建组合"),
                                true,
                                move |this, _, cx| this.apply_sheet_layout(to_spread, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "layout_cancel",
                                ui_kit::t("dialog.cancel"),
                                false,
                                |this, _, cx| this.dismiss_dialog(cx),
                                cx,
                            )),
                    ),
            )
    }

    pub(super) fn unsaved_new_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("dialog_backdrop_unsaved_new")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .id("dialog_card_unsaved_new")
                    .w(px(420.))
                    .p_4()
                    .rounded_lg()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(ui_kit::t_src("未保存的改动")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui_kit::chrome(0x334155))
                            .child(ui_kit::t_src("当前工程有未保存改动. 新建前要先保存吗?")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "new_save",
                                ui_kit::t_src("保存后新建"),
                                true,
                                |this, window, cx| {
                                    // 保持 UnsavedNew, 供保存成功后清空
                                    if this.project_path.is_some() {
                                        this.save_project(window, cx);
                                    } else {
                                        this.save_project_as(window, cx);
                                    }
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "new_discard",
                                ui_kit::t_src("不保存新建"),
                                false,
                                |this, _, cx| {
                                    this.dirty = false;
                                    this.do_new_project(cx);
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "new_cancel",
                                ui_kit::t("dialog.cancel"),
                                false,
                                |this, _, cx| {
                                    this.dismiss_dialog(cx);
                                },
                                cx,
                            )),
                    ),
            )
    }

    pub(super) fn unsaved_photo_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("dialog_backdrop_unsaved_photo")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .id("dialog_card_unsaved_photo")
                    .w(px(420.))
                    .p_4()
                    .rounded_lg()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(ui_kit::t_src("未应用的修图")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui_kit::chrome(0x334155))
                            .child(ui_kit::t_src("离开前要应用这次修图吗?")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "photo_apply",
                                ui_kit::t_src("应用并离开"),
                                true,
                                |this, window, cx| this.apply_photo_and_leave(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "photo_discard",
                                ui_kit::t_src("放弃"),
                                false,
                                |this, window, cx| this.discard_photo_and_leave(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "photo_stay",
                                ui_kit::t("dialog.cancel"),
                                false,
                                |this, _, cx| this.dismiss_dialog(cx),
                                cx,
                            )),
                    ),
            )
    }

    pub(super) fn update_available_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (current, latest, url, changes) = match &self.dialog {
            Some(DialogKind::UpdateAvailable {
                current,
                latest,
                url,
                changes,
            }) => (
                current.clone(),
                latest.clone(),
                url.clone(),
                changes.clone(),
            ),
            _ => return div().into_any_element(),
        };
        let url_open = url.clone();
        let mut notes = div()
            .id("update_notes")
            .flex()
            .flex_col()
            .gap_3()
            .text_sm()
            .text_color(ui_kit::chrome(0x334155));
        if changes.is_empty() {
            notes = notes.child(ui_kit::t_src("可前往发布页查看版本说明."));
        } else {
            for (ver, bullets) in &changes {
                let mut block = div().flex().flex_col().gap_1().child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(ui_kit::chrome(0x0f172a))
                        .child(ver.clone()),
                );
                if bullets.is_empty() {
                    block = block.child(div().text_color(ui_kit::chrome(0x64748b)).child(ui_kit::t_src("见发布页说明.")));
                } else {
                    for b in bullets {
                        block = block.child(div().whitespace_normal().child(format!("· {b}")));
                    }
                }
                notes = notes.child(block);
            }
        }
        div()
            .id("dialog_backdrop_update")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                    this.apply_scrollbar_drag(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                }
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if matches!(this.drag, Some(DragKind::Scrollbar { .. })) {
                        this.drag = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("dialog_card_update")
                    .w(px(480.))
                    .h(px(440.))
                    .max_h(px(440.))
                    .p_4()
                    .rounded_lg()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(ui_kit::chrome(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(ui_kit::t_src("发现新版本")),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .text_color(ui_kit::chrome(0x334155))
                            .child(match ui_kit::language() {
                                ui_kit::Language::En => {
                                    format!("Installed {current}, latest on GitHub {latest}.")
                                }
                                ui_kit::Language::Zh => format!("当前 {current}, GitHub 最新 {latest}."),
                            }),
                    )
                    .child(
                        self.attach_scrollbars(
                            "update_scroll_wrap".into(),
                            ScrollList::Update,
                            &self.update_scroll,
                            notes,
                            cx,
                        )
                        .flex_1()
                        .min_h(px(0.)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "update_open",
                                ui_kit::t_src("打开下载页"),
                                true,
                                move |this, _, cx| {
                                    crate::update::open_in_browser(&url_open);
                                    this.dismiss_dialog(cx);
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "update_later",
                                ui_kit::t_src("以后再说"),
                                false,
                                |this, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                },
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }
}
