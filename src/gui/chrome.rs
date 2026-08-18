//! 工具栏、工作区、工程面板、对话框.

use super::*;
use super::ScoreSyncApp;

impl ScoreSyncApp {
    pub(super) fn show_help(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.dialog = Some(DialogKind::Help);
        cx.notify();
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
        div()
            .id(id.into())
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
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
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
        let fg = if active { rgb(0x1d4ed8) } else { rgb(0x334155) };
        div()
            .id(id.into())
            .px_2()
            .py_1()
            .text_sm()
            .text_color(fg)
            .cursor_pointer()
            .rounded_sm()
            .hover(|s| s.bg(rgb(0xe2e8f0)))
            .when(active, |d| d.bg(rgb(0xdbeafe)))
            .child(label.into())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 顶部菜单栏: 文字项横排, 非独立按钮块
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x_1()
            .w_full()
            .child(self.menu_item("open", "打开 (Ctrl+O)", false, Self::open_file, cx))
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
                "fit",
                "适应窗口 (F)",
                false,
                |this, _, cx| this.fit_to_view(cx),
                cx,
            ))
            .child(self.menu_item(
                "help",
                "操作说明 (H)",
                false,
                |this, _, cx| this.show_help(cx),
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
            .child(self.tool_tab("tool_mask", "蒙版", mask_on, SideTool::Mask, cx))
            .child(self.tool_tab("tool_proj", "工程", proj_on, SideTool::Project, cx))
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
        let canvas = match self.side_tool {
            SideTool::Crop | SideTool::Project => self.image_view(cx).into_any_element(),
            SideTool::Mask => self
                .mask_tool
                .update(cx, |m, cx| m.image_view(cx))
                .into_any_element(),
            SideTool::Video => unreachable!(),
        };
        div()
            .id("left_workspace")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(
                div()
                    .w_full()
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

    pub(super) fn mask_target_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_gid = self.mask_target.clone().or_else(|| self.doc.active_group_id.clone());
        let n = self.doc.groups.len();
        let virtualize = n > GROUP_LIST_VIRTUAL_THRESHOLD;
        let (start, end) = self.visible_mask_picker_range();
        let mut list = div()
            .id("mask_group_list")
            .flex()
            .when(virtualize, |d| d.flex_col())
            .when(!virtualize, |d| d.flex_row().flex_wrap())
            .gap_1()
            .p_1()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        if virtualize && start > 0 {
            list = list.child(
                div()
                    .h(px(start as f32 * MASK_PICKER_ROW_PX))
                    .w_full()
                    .flex_shrink_0(),
            );
        }
        for i in start..end {
            let Some(g) = self.doc.groups.get(i) else {
                continue;
            };
            let gid = g.id.clone();
            let active = active_gid.as_ref() == Some(&gid);
            let label = self.doc.group_crop_label(i);
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
                    .id(SharedString::from(format!("mask-g-{gid}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(bg)
                    .text_color(fg)
                    .text_xs()
                    .cursor_pointer()
                    .flex_shrink_0()
                    .child(label)
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_mask_target(gid.clone(), false, cx);
                        }),
                    ),
            );
        }
        if virtualize && end < n {
            list = list.child(
                div()
                    .h(px((n - end) as f32 * MASK_PICKER_ROW_PX))
                    .w_full()
                    .flex_shrink_0(),
            );
        }

        div()
            .id("mask_target_picker")
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
                    .child("编辑目标 (组合拼合图)"),
            )
            .child(
                self.attach_scrollbars(
                    "mask_group_scroll_wrap".into(),
                    ScrollList::MaskGroup,
                    &self.mask_group_scroll,
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
                let picker = self.mask_target_picker(cx).into_any_element();
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
            SideTool::Project => self.project_panel(cx).into_any_element(),
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

    pub(super) fn project_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let proj_name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("(未保存)")
            .to_string();
        let bg_status: SharedString = if self.doc.bg_enabled {
            let src = self
                .doc
                .bg_source_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("bg");
            format!(
                "底色层: 已启用 {} ({}:{}) — 导出时底层合成, 未改写页图",
                src, self.doc.bg_aspect_w, self.doc.bg_aspect_h
            )
            .into()
        } else {
            "底色层: 未启用".into()
        };
        let apply_panel = self.apply_bg.update(cx, |m, cx| m.panel(cx).into_any_element());
        div()
            .id("project_panel")
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(rgb(0xf1f5f9))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(0xcbd5e1))
                    .bg(rgb(0xffffff))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("工程文件"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .child(format!("当前: {proj_name}")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(self.btn(
                                "proj_new",
                                "新建工程",
                                false,
                                |this, window, cx| this.request_new_project(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_open",
                                "打开工程",
                                false,
                                |this, window, cx| this.open_project(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_save",
                                "保存 (Ctrl+S)",
                                true,
                                |this, window, cx| this.save_project(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_save_as",
                                "另存为",
                                false,
                                |this, window, cx| this.save_project_as(window, cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_clear_video_cache",
                                "清除视频缓存",
                                false,
                                |this, _, cx| this.clear_video_pool_cache(cx),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .mt_2()
                            .child("工程底色层"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .child(bg_status),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(self.btn(
                                "proj_bg_apply",
                                "应用到工程组合",
                                true,
                                |this, _, cx| this.apply_project_bg(cx),
                                cx,
                            ))
                            .child(self.btn(
                                "proj_bg_clear",
                                "取消工程底色",
                                false,
                                |this, _, cx| this.clear_project_bg(cx),
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .id("project_apply_scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_scroll()
                    .child(apply_panel),
            )
    }

    pub(super) fn clear_video_pool_cache(&mut self, cx: &mut Context<Self>) {
        let dir = self.pool_cache_dir();
        let _ = std::fs::remove_dir_all(&dir);
        self.mark_video_pool_dirty_all();
        self.score_video
            .update(cx, |v, cx| v.set_pool(Vec::new(), cx));
        self.status = format!("已清除视频缓存: {}", dir.display()).into();
        self.hint = self.status.clone();
        if self.side_tool == SideTool::Video {
            self.sync_video_pool(cx);
        }
        cx.notify();
    }

    pub(super) fn apply_project_bg(&mut self, cx: &mut Context<Self>) {
        crate::trace::log("apply_bg: 点击应用到工程组合");
        if self.doc.groups.is_empty() {
            self.dialog = Some(DialogKind::Info {
                title: "提示".into(),
                body: "当前没有输出组合. 请先分块/合并后再应用底色层.".into(),
            });
            cx.notify();
            return;
        }
        let params = self.apply_bg.read(cx).snapshot_params(cx);
        let (path, aw, ah) = match params {
            Ok(v) => v,
            Err(e) => {
                self.dialog = Some(DialogKind::Info {
                    title: "无法应用底色".into(),
                    body: e,
                });
                cx.notify();
                return;
            }
        };
        crate::trace::log(&format!(
            "apply_bg: 打开底色 {} 比例 {aw}:{ah}",
            path.display()
        ));
        match image::open(&path) {
            Ok(im) => {
                let rgb = im.to_rgb8();
                crate::trace::log(&format!(
                    "apply_bg: 底色已解码 {}x{}",
                    rgb.width(),
                    rgb.height()
                ));
                match self
                    .doc
                    .set_project_bg(rgb, Some(path.clone()), aw, ah)
                {
                    Ok(()) => {
                        // 试合成第一组, 尽早发现底色太小等问题
                        if let Some(gid) = self.doc.groups.first().map(|g| g.id.clone()) {
                            let _ = self.doc.ensure_group_pages(&gid);
                            if let Err(e) = self.doc.render_group_final(&gid) {
                                self.doc.clear_project_bg();
                                self.doc.retain_window(
                                    self.doc.current_page_index,
                                    crate::page_cache::WINDOW_RADIUS,
                                );
                                self.dialog = Some(DialogKind::Info {
                                    title: "底色不适用".into(),
                                    body: format!(
                                        "{e}\n已取消启用. 请换更大底色 (总谱按高度定画布时左右也要盖住) 或检查谱面尺寸."
                                    ),
                                });
                                cx.notify();
                                return;
                            }
                            self.doc.retain_window(
                                self.doc.current_page_index,
                                crate::page_cache::WINDOW_RADIUS,
                            );
                        }
                        self.mark_dirty();
                        self.mark_video_pool_dirty_all();
                        self.status = format!(
                            "已为 {} 个组合启用底色层 {} ({}:{})",
                            self.doc.groups.len(),
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("bg"),
                            aw,
                            ah
                        )
                        .into();
                        self.hint = self.status.clone();
                        crate::trace::log("apply_bg: 即将刷新蒙版预览");
                        self.force_refresh_mask_preview(cx);
                        crate::trace::log("apply_bg: 即将同步视频池");
                        self.sync_video_pool(cx);
                        crate::trace::log("apply_bg: 应用到工程完成");
                        cx.notify();
                    }
                    Err(e) => {
                        self.dialog = Some(DialogKind::Info {
                            title: "无法应用底色".into(),
                            body: e,
                        });
                        cx.notify();
                    }
                }
            }
            Err(e) => {
                self.dialog = Some(DialogKind::Info {
                    title: "无法打开底色".into(),
                    body: e.to_string(),
                });
                cx.notify();
            }
        }
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
    pub(super) fn dialog_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.score_video.read(cx).is_export_open() {
            return self
                .score_video
                .update(cx, |v, cx| v.export_dialog(cx).into_any_element());
        }
        let Some(ref dlg) = self.dialog else {
            return div().into_any_element();
        };
        if matches!(dlg, DialogKind::UnsavedExit) {
            return self.unsaved_exit_dialog(cx).into_any_element();
        }
        if matches!(dlg, DialogKind::UnsavedNew) {
            return self.unsaved_new_dialog(cx).into_any_element();
        }
        if matches!(dlg, DialogKind::UpdateAvailable { .. }) {
            return self.update_available_dialog(cx).into_any_element();
        }
        let (title, body) = match dlg {
            DialogKind::Help => ("操作说明".to_string(), HELP_TEXT.to_string()),
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
                    .h(px(520.))
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
