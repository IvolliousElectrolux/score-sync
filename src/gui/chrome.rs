//! 工具栏、工作区、工程面板、对话框.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn show_help(&mut self, cx: &mut Context<Self>) {
        if self.page_organize.is_some() {
            self.close_page_organize(cx);
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
            || self.dialog.is_some()
            || self.bg.pick_open
            || self.bg.batch_open
            || self.apply_bg.read(cx).is_error_open()
            || self.score_video.read(cx).is_error_open()
            || self.score_video.read(cx).is_export_open()
    }

    pub(super) fn header_file_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ms = apply_bg::primary_shift();
        div()
            .id("header_file_bar")
            .flex_shrink_0()
            .mr_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(self.header_icon_btn(
                "hdr-new",
                HeaderGlyph::NewDoc,
                format!("新建工程 ({ms}N)").into(),
                |this, window, cx| this.request_new_project(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-open",
                HeaderGlyph::OpenFolder,
                format!("打开工程 ({ms}O)").into(),
                |this, window, cx| this.open_project(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-save",
                HeaderGlyph::SaveDisk,
                apply_bg::with_mod("保存工程", "S").into(),
                |this, window, cx| this.save_project(window, cx),
                cx,
            ))
            .child(self.header_icon_btn(
                "hdr-saveas",
                HeaderGlyph::SaveAsDisk,
                format!("另存工程 ({ms}S)").into(),
                |this, window, cx| this.save_project_as(window, cx),
                cx,
            ))
            .child(self.header_help_btn(cx))
    }

    fn header_icon_btn(
        &self,
        id: &'static str,
        glyph: HeaderGlyph,
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
            .w(px(22.))
            .h(px(22.))
            .rounded_full()
            .border_1()
            .border_color(rgb(0x64748b))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0xe2e8f0)).border_color(rgb(0x334155)))
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
                canvas(|_, _, _| {}, {
                    move |bounds, _, window, _| {
                        paint_header_glyph(window, bounds, glyph, rgb(0x334155));
                    }
                })
                .w(px(13.))
                .h(px(13.)),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.note_header_hover(id, tip_hover.clone(), cx);
                } else if this.header_hover_id == Some(id) {
                    this.clear_tab_hover(cx);
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

    fn header_help_btn(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let tip: SharedString = "操作说明 (H / F1)".into();
        let tip_hover = tip.clone();
        div()
            .id("hdr-help")
            .relative()
            .flex_shrink_0()
            .ml_1()
            .w(px(22.))
            .h(px(22.))
            .rounded_full()
            .border_1()
            .border_color(rgb(0x64748b))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(0x334155))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0xe2e8f0)).border_color(rgb(0x334155)))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| {
                            this.header_btn_bounds
                                .insert("hdr-help".to_string(), bounds);
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child("?")
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.note_header_hover("hdr-help", tip_hover.clone(), cx);
                } else if this.header_hover_id == Some("hdr-help") {
                    this.clear_tab_hover(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.clear_tab_hover(cx);
                    this.show_help(cx);
                }),
            )
    }

    fn note_header_hover(
        &mut self,
        id: &'static str,
        text: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.header_hover_id == Some(id) {
            return;
        }
        self.tab_hover_idx = None;
        self.header_hover_id = Some(id);
        self.tab_tooltip = None;
        self.tab_hover_gen = self.tab_hover_gen.wrapping_add(1);
        let gen = self.tab_hover_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            this.update(cx, |view, cx| {
                if view.tab_hover_gen != gen || view.header_hover_id != Some(id) {
                    return;
                }
                let (ax, ay, aw, ah) = view
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
                view.tab_tooltip = Some(TabTooltip {
                    id: id.into(),
                    anchor_x: ax,
                    anchor_y: ay,
                    anchor_w: aw,
                    anchor_h: ah,
                    text,
                    measured_w: 0.0,
                    measured_h: 0.0,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn btn(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active { rgb(0x2563eb) } else { rgb(0xe2e8f0) };
        let fg = if active { rgb(0xffffff) } else { rgb(0x0f172a) };
        let hover = if active { rgb(0x1d4ed8) } else { rgb(0xcbd5e1) };
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
            .border_color(rgb(0x94a3b8))
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
        let bg = if active { rgb(0x2563eb) } else { rgb(0xf8fafc) };
        let fg = if active { rgb(0xffffff) } else { rgb(0x334155) };
        let hover = if active { rgb(0x1d4ed8) } else { rgb(0xe2e8f0) };
        div()
            .id(id.into())
            .flex_shrink_0()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(bg)
            .text_color(fg)
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(label.into())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 分块菜单: 放在左栏顶上一行, 按钮尺寸对齐视频轨运输条.
        div()
            .id("crop_toolbar")
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pl_2()
            .pr_1()
            .py_1()
            .w_full()
            .min_w(px(0.))
            .overflow_x_scroll()
            .bg(rgb(0xe2e8f0))
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .child(self.menu_item("open", apply_bg::with_mod("打开", "O"), false, Self::open_file, cx))
            .child(self.menu_item(
                "detect",
                "识别本页 (D)",
                false,
                |this, _, cx| this.run_detect(cx),
                cx,
            ))
            .child(self.menu_item(
                "detect_all",
                "识别全部页 (A)",
                false,
                |this, _, cx| this.run_detect_all(cx),
                cx,
            ))
            .child(self.menu_item(
                "add_block",
                "添加新块 (N)",
                self.canvas_tool == CanvasTool::AddBlock,
                |this, _, cx| this.toggle_add_block(cx),
                cx,
            ))
            .child(self.menu_item(
                "split_block",
                "分割块 (S)",
                self.canvas_tool == CanvasTool::SplitBlock,
                |this, _, cx| this.toggle_split_block(cx),
                cx,
            ))
            .child(self.menu_item(
                "merge",
                "合并组合 (M)",
                false,
                |this, _, cx| this.merge_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "ungroup",
                "拆开组合 (U)",
                false,
                |this, _, cx| this.ungroup_active(cx),
                cx,
            ))
            .child(self.menu_item(
                "share",
                "共享脚注 (G)",
                false,
                |this, _, cx| this.share_into_group(cx),
                cx,
            ))
            .child(self.menu_item(
                "del",
                "删除 (Del)",
                false,
                |this, _, cx| this.delete_selected(cx),
                cx,
            ))
            .child(self.menu_item(
                "export",
                "导出组合 (E)",
                false,
                Self::export_groups_ui,
                cx,
            ))
            .child(self.menu_item(
                "reset",
                "重置本页分组 (R)",
                false,
                |this, _, cx| this.reset_groups(cx),
                cx,
            ))
            .child(self.menu_item(
                "organize_pages",
                "组织页面 (P)",
                false,
                |this, window, cx| this.toggle_page_organize(window, cx),
                cx,
            ))
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
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .w_full()
            .bg(rgb(0xe2e8f0))
            .border_b_1()
            .border_color(rgb(0xcbd5e1))
            .child(self.tool_tab("tool_crop", "分块", crop_on, SideTool::Crop, cx))
            .child(self.tool_tab("tool_proj", "底色", proj_on, SideTool::Project, cx))
            .child(self.tool_tab("tool_mask", "蒙版", mask_on, SideTool::Mask, cx))
            .child(self.tool_tab("tool_video", "视频", video_on, SideTool::Video, cx))
    }

    pub(super) fn tool_tab(
        &self,
        id: &'static str,
        label: &'static str,
        active: bool,
        tool: SideTool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if active {
            rgb(0x2563eb)
        } else {
            rgb(0xf8fafc)
        };
        let fg = if active {
            rgb(0xffffff)
        } else {
            rgb(0x334155)
        };
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(move |s| {
                if active {
                    s
                } else {
                    s.bg(rgb(0xf1f5f9))
                }
            })
            .child(label)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.set_side_tool(tool, window, cx);
                }),
            )
    }

    pub(super) fn left_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        .border_color(rgb(0xcbd5e1))
                        .bg(rgb(0xf8fafc))
                        .child(self.tab_bar(cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .min_w(px(0.))
                        .child(canvas),
                )
                .into_any_element();
        }
        let canvas = match self.side_tool {
            SideTool::Crop => self.image_view(cx).into_any_element(),
            SideTool::Mask => self
                .mask_tool
                .update(cx, |m, cx| m.image_view(cx))
                .into_any_element(),
            SideTool::Project | SideTool::Video => unreachable!(),
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
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(0xcbd5e1))
                        .bg(rgb(0xe2e8f0))
                        .child(tb),
                )
            })
            .child(
                div()
                    .w_full()
                    .min_w(px(0.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(0xcbd5e1))
                    .bg(rgb(0xf8fafc))
                    .child(self.tab_bar(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
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
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        if rows.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x94a3b8))
                    .child("当前组合内没有分块"),
            );
        }
        for row in &rows {
            let rid = row.id.clone();
            let active = row.selected;
            let bg = if active {
                rgb(0x2563eb)
            } else {
                rgb(0xe2e8f0)
            };
            let fg = if active {
                rgb(0xffffff)
            } else {
                rgb(0x0f172a)
            };
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
            .border_color(rgb(0xcbd5e1))
            .bg(rgb(0xf8fafc))
            .flex()
            .flex_col()
            .min_h(px(0.))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0x334155))
                    .mb_1()
                    .flex_shrink_0()
                    .child("组合分块 (自上而下)"),
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
        let body = match self.side_tool {
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
        };
        div()
            .id("right_workspace")
            .w(px(self.side_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(rgb(0xf1f5f9))
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.tool_switcher(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(body),
            )
    }

    pub(super) fn clear_project_bg(&mut self, cx: &mut Context<Self>) {
        if !self.doc.bg_enabled && self.doc.bg_image.is_none() {
            self.status = "当前未启用工程底色层.".into();
            self.hint = self.status.clone();
            cx.notify();
            return;
        }
        self.doc.clear_project_bg();
        self.mark_dirty();
        self.mark_video_pool_dirty_all();
        self.status = "已取消工程底色层.".into();
        self.hint = self.status.clone();
        self.force_refresh_mask_preview(cx);
        self.sync_video_pool(cx);
        cx.notify();
    }

    /// 强制重新拼合并加载蒙版预览图 (绕过 `load_rgb` 的 session_key 缓存),
    /// 用于底色启用/取消后需要刷新预览的场景. 会先落盘当前蒙版编辑, 再清空
    /// 内嵌工具视图, 避免清空动作把待落盘的蒙版一并清没.
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
        if matches!(dlg, DialogKind::UpdateAvailable { .. }) {
            return self.update_available_dialog(cx).into_any_element();
        }
        let (title, body) = match dlg {
            DialogKind::Help => ("操作说明".to_string(), help_text()),
            DialogKind::Info { title, body } => (title.clone(), body.clone()),
            DialogKind::UnsavedExit
            | DialogKind::UnsavedNew
            | DialogKind::UpdateAvailable { .. } => unreachable!(),
        };
        let body_el = div()
            .id("dlg_body")
            .text_sm()
            .text_color(rgb(0x334155))
            .whitespace_normal()
            .child(body);

        div()
            .id("dialog_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
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
                    this.apply_scrollbar_drag(f32::from(ev.position.x), f32::from(ev.position.y), cx);
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
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
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
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(self.btn(
                                "dlg_ok",
                                "确定",
                                true,
                                |this, _, cx| {
                                    this.dismiss_dialog(cx);
                                },
                                cx,
                            )),
                    ),
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
            .bg(gpui::rgba(0x00000080))
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
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("未保存的改动"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x334155))
                            .child("当前工程有未保存改动. 要在退出前保存吗?"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "exit_save",
                                "保存并退出",
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
                                "不保存退出",
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
                                "取消",
                                false,
                                |this, _, cx| {
                                    this.dismiss_dialog(cx);
                                },
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
            .bg(gpui::rgba(0x00000080))
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
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("未保存的改动"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x334155))
                            .child("当前工程有未保存改动. 新建前要先保存吗?"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(self.btn(
                                "new_save",
                                "保存后新建",
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
                                "不保存新建",
                                false,
                                |this, _, cx| {
                                    this.dirty = false;
                                    this.do_new_project(cx);
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "new_cancel",
                                "取消",
                                false,
                                |this, _, cx| {
                                    this.dismiss_dialog(cx);
                                },
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
            .text_color(rgb(0x334155));
        if changes.is_empty() {
            notes = notes.child("可前往发布页查看版本说明.");
        } else {
            for (ver, bullets) in &changes {
                let mut block = div().flex().flex_col().gap_1().child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(0x0f172a))
                        .child(ver.clone()),
                );
                if bullets.is_empty() {
                    block = block.child(
                        div()
                            .text_color(rgb(0x64748b))
                            .child("见发布页说明."),
                    );
                } else {
                    for b in bullets {
                        block = block.child(
                            div()
                                .whitespace_normal()
                                .child(format!("· {b}")),
                        );
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
            .bg(gpui::rgba(0x00000080))
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
                    this.apply_scrollbar_drag(f32::from(ev.position.x), f32::from(ev.position.y), cx);
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
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x94a3b8))
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
                            .child("发现新版本"),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .text_color(rgb(0x334155))
                            .child(format!(
                                "当前 {current}, GitHub 最新 {latest}."
                            )),
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
                                "打开下载页",
                                true,
                                move |this, _, cx| {
                                    crate::update::open_in_browser(&url_open);
                                    this.dismiss_dialog(cx);
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "update_later",
                                "以后再说",
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

#[derive(Clone, Copy)]
enum HeaderGlyph {
    NewDoc,
    OpenFolder,
    SaveDisk,
    SaveAsDisk,
}

fn paint_header_glyph(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    glyph: HeaderGlyph,
    color: gpui::Rgba,
) {
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let s = f32::from(bounds.size.width)
        .min(f32::from(bounds.size.height))
        .max(1.0);
    let p = |x: f32, y: f32| point(px(ox + x / 16.0 * s), px(oy + y / 16.0 * s));
    // 与问号圆框 border_1 同级细线, 不再用加粗描边.
    let thick = px(1.);
    let mut stroke = |pts: &[(f32, f32)], close: bool| {
        if pts.is_empty() {
            return;
        }
        let mut b = PathBuilder::stroke(thick);
        b.move_to(p(pts[0].0, pts[0].1));
        for &(x, y) in &pts[1..] {
            b.line_to(p(x, y));
        }
        if close {
            b.close();
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };
    match glyph {
        HeaderGlyph::NewDoc => {
            // 折角纸, 开口轮廓, 两行字
            stroke(
                &[
                    (10.0, 2.4),
                    (4.6, 2.4),
                    (4.6, 13.6),
                    (11.6, 13.6),
                    (11.6, 4.2),
                    (10.0, 2.4),
                ],
                false,
            );
            stroke(&[(10.0, 2.4), (10.0, 4.2), (11.6, 4.2)], false);
            stroke(&[(6.4, 7.4), (10.0, 7.4)], false);
            stroke(&[(6.4, 10.2), (9.2, 10.2)], false);
        }
        HeaderGlyph::OpenFolder => {
            stroke(&[(3.4, 5.6), (3.4, 3.8), (7.2, 3.8), (8.1, 5.6)], false);
            stroke(
                &[
                    (3.4, 5.6),
                    (3.4, 12.8),
                    (12.6, 12.8),
                    (12.6, 5.6),
                    (3.4, 5.6),
                ],
                false,
            );
        }
        HeaderGlyph::SaveDisk => paint_floppy(&mut stroke, false),
        HeaderGlyph::SaveAsDisk => paint_floppy(&mut stroke, true),
    }
}

fn paint_floppy(stroke: &mut impl FnMut(&[(f32, f32)], bool), save_as: bool) {
    stroke(
        &[
            (4.4, 2.8),
            (11.6, 2.8),
            (11.6, 13.2),
            (4.4, 13.2),
            (4.4, 2.8),
        ],
        false,
    );
    stroke(&[(6.6, 2.8), (6.6, 5.4), (9.4, 5.4), (9.4, 2.8)], false);
    stroke(&[(6.0, 8.2), (10.0, 8.2)], false);
    stroke(&[(6.0, 10.4), (9.2, 10.4)], false);
    if save_as {
        stroke(&[(10.2, 6.8), (13.2, 3.8)], false);
        stroke(&[(11.4, 3.8), (13.2, 3.8), (13.2, 5.6)], false);
    }
}
