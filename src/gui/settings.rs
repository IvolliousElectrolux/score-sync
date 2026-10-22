//! 全局设置: 项目属性, 偏好, 快捷键.

use gpui::{MouseButton, px, rgb};
use ui_kit::t;

use super::keymap::{self, Conflict, COMMANDS};
use super::ScoreSyncApp;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsPage {
    Project,
    Prefs,
    Keys,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum UpdateProbe {
    Idle,
    Checking,
    UpToDate,
    Failed(String),
}

impl ScoreSyncApp {
    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.close_settings(cx);
            return;
        }
        if matches!(
            self.dialog,
            Some(DialogKind::Help | DialogKind::Info { .. })
        ) {
            self.dialog = None;
        }
        self.settings_open = true;
        self.settings_conflict = SharedString::default();
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.end_shortcut_capture(cx);
        self.settings_open = false;
        self.settings_conflict = SharedString::default();
        cx.notify();
    }

    pub(super) fn settings_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        div()
            .id("settings_backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(ui_kit::overlay())
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                // 遮罩 occlude 挡住了根节点, 拖动条的移动只能在这里接着算.
                // 指针离开拖动条后, 只要还按着, 就继续跟着走.
                if matches!(
                    this.drag,
                    Some(DragKind::Scrollbar {
                        which: ScrollList::Settings,
                        ..
                    })
                ) {
                    this.apply_scrollbar_drag(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        cx,
                    );
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if matches!(
                        this.drag,
                        Some(DragKind::Scrollbar {
                            which: ScrollList::Settings,
                            ..
                        })
                    ) {
                        this.drag = None;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .id("settings_card")
                    .w(px(760.))
                    .h(px(540.))
                    .flex()
                    .flex_row()
                    .bg(ui_kit::card())
                    .border_1()
                    .border_color(rgb(theme.border_strong))
                    .rounded_lg()
                    .overflow_hidden()
                    .text_color(rgb(theme.text))
                    .child(self.settings_nav(cx))
                    .child(self.settings_body(cx)),
            )
    }

    fn settings_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        let page = self.settings_page;
        div()
            .w(px(168.))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(theme.bg_panel))
            .rounded_l(px(7.))
            .border_r_1()
            .border_color(rgb(theme.border))
            .py_2()
            .child(self.settings_nav_item(SettingsPage::Project, page, cx))
            .child(self.settings_nav_item(SettingsPage::Prefs, page, cx))
            .child(self.settings_nav_item(SettingsPage::Keys, page, cx))
    }

    fn settings_nav_item(
        &self,
        page: SettingsPage,
        current: SettingsPage,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = ui_kit::theme();
        let active = page == current;
        let label = match page {
            SettingsPage::Project => t("settings.project"),
            SettingsPage::Prefs => t("settings.prefs"),
            SettingsPage::Keys => t("settings.keys"),
        };
        div()
            .id(SharedString::from(format!("settings-nav-{page:?}")))
            .mx_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .when(active, |row| {
                row.bg(rgb(theme.bg_selected))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .hover(|row| row.bg(rgb(ui_kit::theme().bg_hover)))
            .child(label)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.end_shortcut_capture(cx);
                    this.settings_page = page;
                    this.settings_conflict = SharedString::default();
                    cx.notify();
                }),
            )
    }

    fn settings_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t("settings.title")),
                    )
                    .child(self.settings_text_btn("settings-close", &t("settings.close"), false, cx, |this, _, cx| {
                        this.close_settings(cx);
                    })),
            )
            .child(match self.settings_page {
                SettingsPage::Project => self.settings_project(cx).into_any_element(),
                SettingsPage::Prefs => self.settings_prefs(cx).into_any_element(),
                SettingsPage::Keys => self.settings_keys(cx).into_any_element(),
            })
    }

    fn settings_project(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        let spread = self.doc.is_spread();
        let name = self.settings_project_name();
        div()
            .id("settings_project")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .child(format!("{}: {name}", t("settings.project_name"))),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t("settings.layout")),
            )
            .child(self.settings_choice("layout-staff", &t("settings.layout_staff"), &t("settings.layout_staff_hint"), !spread, cx, |this, _, cx| {
                this.request_sheet_layout(false, cx);
            }))
            .child(self.settings_choice("layout-spread", &t("settings.layout_spread"), &t("settings.layout_spread_hint"), spread, cx, |this, _, cx| {
                this.request_sheet_layout(true, cx);
            }))
            .child(self.settings_heading(&t("settings.pack")))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child(t("settings.pack_hint")),
            )
            .child(self.settings_pack_lines(cx))
            .child(self.settings_choice(
                "pack-within",
                &t("settings.pack_within"),
                &t("settings.pack_within_hint"),
                self.doc.staff_pack_mode == crate::staff_detect::StaffPackMode::WithinPage,
                cx,
                |this, _, cx| {
                    let lines = this.doc.staff_pack_lines.max(1);
                    this.apply_staff_pack_choice(
                        lines,
                        crate::staff_detect::StaffPackMode::WithinPage,
                        cx,
                    );
                },
            ))
            .child(self.settings_choice(
                "pack-across",
                &t("settings.pack_across"),
                &t("settings.pack_across_hint"),
                self.doc.staff_pack_mode == crate::staff_detect::StaffPackMode::AcrossPages,
                cx,
                |this, _, cx| {
                    let lines = this.doc.staff_pack_lines.max(1);
                    this.apply_staff_pack_choice(
                        lines,
                        crate::staff_detect::StaffPackMode::AcrossPages,
                        cx,
                    );
                },
            ))
            .when(spread, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .child(t("settings.pack_spread_hint")),
                )
            })
    }

    fn settings_pack_lines(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        let lines = self.doc.staff_pack_lines.max(1);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(self.settings_text_btn(
                "pack-dec",
                &"-".into(),
                false,
                cx,
                |this, _, cx| {
                    let lines = this.doc.staff_pack_lines.max(1);
                    let mode = this.doc.staff_pack_mode;
                    this.apply_staff_pack_choice(lines.saturating_sub(1).max(1), mode, cx);
                },
            ))
            .child(
                div()
                    .min_w(px(72.))
                    .text_sm()
                    .child(format!("{lines} {}", t("settings.pack_lines"))),
            )
            .child(self.settings_text_btn(
                "pack-inc",
                &"+".into(),
                false,
                cx,
                |this, _, cx| {
                    let lines = this.doc.staff_pack_lines.max(1);
                    let mode = this.doc.staff_pack_mode;
                    this.apply_staff_pack_choice((lines + 1).min(12), mode, cx);
                },
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child("1-12"),
            )
    }

    fn apply_staff_pack_choice(
        &mut self,
        lines: u32,
        mode: crate::staff_detect::StaffPackMode,
        cx: &mut Context<Self>,
    ) {
        let lines = lines.clamp(1, 12);
        if self.doc.staff_pack_lines.max(1) == lines && self.doc.staff_pack_mode == mode {
            return;
        }
        self.push_crop_undo_all_pages();
        let spread = self.doc.is_spread();
        let result = self.doc.apply_staff_pack(lines, mode);
        let lines = self.doc.staff_pack_lines.max(1);
        let mode = self.doc.staff_pack_mode;
        self.status = match result {
            crate::model::StaffPackApply::Unchanged => return,
            crate::model::StaffPackApply::NeedRedetect => format!(
                "已改为 {lines} 行. 眼下这些块是按页合并过的, 请重新识别才会按新行数切开."
            )
            .into(),
            crate::model::StaffPackApply::Applied if spread => {
                "已记下谱行合并. 双页对开不识别谱表, 切回去之后重新识别才会用上.".into()
            }
            crate::model::StaffPackApply::Applied if lines < 2 => "已改回一行一块.".into(),
            crate::model::StaffPackApply::Applied
                if mode == crate::staff_detect::StaffPackMode::AcrossPages =>
            {
                format!("已按 {lines} 行跨页组成组合. 标题和脚注没有并进去.").into()
            }
            crate::model::StaffPackApply::Applied => {
                format!("已在每页把谱行按 {lines} 行收成一块.").into()
            }
        };
        self.hint = self.status.clone();
        self.mark_dirty();
        self.after_doc_change(cx);
    }

    fn settings_project_name(&self) -> String {
        if let Some(path) = &self.project_path {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| t("settings.unsaved").to_string())
        } else if self.doc.pages.is_empty() {
            t("settings.no_project").to_string()
        } else {
            t("settings.unsaved").to_string()
        }
    }

    fn settings_prefs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        let current = ui_kit::theme_id();
        let mut themes = div()
            .id("settings_themes")
            .flex()
            .flex_col()
            .gap_1();
        for id in ui_kit::THEME_IDS {
            let selected = *id == current;
            let preset = ui_kit::preset(id);
            let label = t(&format!("theme.{id}"));
            let id_owned = (*id).to_string();
            themes = themes.child(
                div()
                    .id(SharedString::from(format!("theme-{id}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |row| row.bg(rgb(theme.bg_selected)))
                    .hover(|row| row.bg(rgb(ui_kit::theme().bg_hover)))
                    .child(self.theme_swatch(preset))
                    .child(div().text_sm().child(label))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            ui_kit::set_theme_id(&id_owned);
                            this.persist_prefs(cx);
                        }),
                    ),
            );
        }
        let lang = ui_kit::language();
        div()
            .id("settings_prefs")
            .flex_1()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.settings_heading(&t("settings.theme")))
            .child(themes)
            .child(self.settings_heading(&t("settings.language")))
            .child(self.settings_choice(
                "lang-zh",
                &t("settings.lang_zh"),
                "",
                lang == ui_kit::Language::Zh,
                cx,
                |this, _, cx| {
                    ui_kit::set_language(ui_kit::Language::Zh);
                    this.persist_prefs(cx);
                },
            ))
            .child(self.settings_choice(
                "lang-en",
                &t("settings.lang_en"),
                "",
                lang == ui_kit::Language::En,
                cx,
                |this, _, cx| {
                    ui_kit::set_language(ui_kit::Language::En);
                    this.persist_prefs(cx);
                },
            ))
            .child(self.settings_heading(&t("settings.startup")))
            .child(self.settings_check(
                "reopen-last",
                &t("settings.reopen"),
                ui_kit::reopen_last_project(),
                cx,
                |this, _, cx| {
                    ui_kit::set_reopen_last_project(!ui_kit::reopen_last_project());
                    this.persist_prefs(cx);
                },
            ))
            .child(self.settings_heading(&t("settings.updates")))
            .child(self.settings_check(
                "check-updates",
                &t("settings.check_updates"),
                ui_kit::check_updates(),
                cx,
                |this, _, cx| {
                    ui_kit::set_check_updates(!ui_kit::check_updates());
                    this.persist_prefs(cx);
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.settings_text_btn(
                        "check-now",
                        &t(match self.update_probe {
                            UpdateProbe::Checking => "settings.checking",
                            _ => "settings.check_now",
                        }),
                        false,
                        cx,
                        |this, _, cx| this.check_updates_now(cx),
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(self.update_probe_label()),
                    ),
            )
    }

    fn update_probe_label(&self) -> SharedString {
        match &self.update_probe {
            UpdateProbe::Idle | UpdateProbe::Checking => SharedString::default(),
            UpdateProbe::UpToDate => t("settings.up_to_date"),
            UpdateProbe::Failed(err) => {
                format!("{} ({err})", t("settings.update_failed")).into()
            }
        }
    }

    fn theme_swatch(&self, preset: ui_kit::Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .rounded_sm()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(ui_kit::theme().border))
            .child(swatch(preset.bg_panel))
            .child(swatch(preset.bg_card))
            .child(swatch(preset.accent))
            .child(swatch(preset.warning))
    }

    fn settings_keys(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        let mut list = div()
            .id("settings_key_list")
            .flex()
            .flex_col()
            .gap(px(2.));
        let mut last = None;
        for cmd in COMMANDS {
            if last != Some(cmd.group) {
                last = Some(cmd.group);
                list = list.child(
                    div()
                        .mt_2()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.text_muted))
                        .child(t(keymap::group_label(cmd.group))),
                );
            }
            list = list.child(self.settings_key_row(cmd, cx));
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(self.settings_check(
                        "show-keys",
                        &t("settings.show_keys"),
                        ui_kit::show_menu_shortcuts(),
                        cx,
                        |this, _, cx| {
                            ui_kit::set_show_menu_shortcuts(!ui_kit::show_menu_shortcuts());
                            this.persist_prefs(cx);
                        },
                    ))
                    .child(self.settings_text_btn(
                        "restore-keys",
                        &t("settings.restore"),
                        false,
                        cx,
                        |this, _, cx| {
                            this.end_shortcut_capture(cx);
                            keymap::clear_overrides();
                            this.settings_conflict = SharedString::default();
                            this.persist_prefs(cx);
                        },
                    )),
            )
            .when(!self.settings_conflict.is_empty(), |col| {
                col.child(
                    div()
                        .px_4()
                        .py_1()
                        .text_xs()
                        .text_color(rgb(theme.danger))
                        .child(self.settings_conflict.clone()),
                )
            })
            .child(
                self.attach_scrollbars(
                    "settings_keys_scroll".into(),
                    ScrollList::Settings,
                    &self.settings_scroll,
                    list.px_4().py_2(),
                    cx,
                )
                .flex_1()
                .min_h(px(0.)),
            )
    }

    fn settings_key_row(&self, cmd: &keymap::Command, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ui_kit::theme();
        let keys = keymap::effective_keys(cmd.id);
        let mut row = div()
            .id(SharedString::from(format!("key-{}", cmd.id)))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .py(px(2.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_sm()
                    .child(t(cmd.id)),
            );
        for (slot, key) in keys.iter().enumerate() {
            let capturing = self.settings_capture.as_ref().is_some_and(|(id, index)| {
                id == cmd.id && *index == slot
            });
            let label = if capturing {
                t("settings.capture")
            } else if key.is_empty() {
                t("settings.unbound")
            } else {
                keymap::display_key(key).into()
            };
            let id = cmd.id.to_string();
            row = row.child(
                div()
                    .id(SharedString::from(format!("cap-{}-{slot}", cmd.id)))
                    .w(px(132.))
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if capturing {
                        theme.accent
                    } else {
                        theme.border_strong
                    }))
                    .bg(rgb(if capturing {
                        theme.bg_selected
                    } else {
                        theme.bg_input
                    }))
                    .text_xs()
                    .cursor_pointer()
                    .child(label)
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.begin_shortcut_capture(id.clone(), slot, cx);
                        }),
                    ),
            );
            if !key.is_empty() && !capturing {
                let id = cmd.id.to_string();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("clr-{}-{slot}", cmd.id)))
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .cursor_pointer()
                        .child(t("settings.clear"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.end_shortcut_capture(cx);
                                keymap::set_slot(&id, slot, String::new());
                                this.settings_conflict = SharedString::default();
                                this.persist_prefs(cx);
                            }),
                        ),
                );
            }
        }
        row
    }

    fn settings_heading(&self, text: &SharedString) -> impl IntoElement {
        div()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(text.clone())
    }

    fn settings_choice(
        &self,
        id: &'static str,
        title: &SharedString,
        hint: &str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let theme = ui_kit::theme();
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| row.bg(rgb(theme.bg_selected)))
            .hover(|row| row.bg(rgb(ui_kit::theme().bg_hover)))
            .child(choice_mark(selected))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().child(title.clone()))
                    .when(!hint.is_empty(), |col| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child(hint.to_string()),
                        )
                    }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn settings_check(
        &self,
        id: &'static str,
        title: &SharedString,
        checked: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let theme = ui_kit::theme();
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border_strong))
                    .bg(if checked {
                        rgb(theme.accent)
                    } else {
                        rgb(theme.bg_input)
                    }),
            )
            .child(div().text_sm().child(title.clone()))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn settings_text_btn(
        &self,
        id: &'static str,
        label: &SharedString,
        active: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let theme = ui_kit::theme();
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .border_1()
            .border_color(rgb(theme.border_strong))
            .bg(if active {
                rgb(theme.accent)
            } else {
                rgb(theme.bg_toolbar)
            })
            .text_color(if active {
                ui_kit::on_accent()
            } else {
                rgb(theme.text)
            })
            .hover(|btn| btn.bg(rgb(ui_kit::theme().bg_hover)))
            .child(label.clone())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    pub(super) fn begin_shortcut_capture(&mut self, id: String, slot: usize, cx: &mut Context<Self>) {
        self.end_shortcut_capture(cx);
        self.settings_capture = Some((id, slot));
        self.settings_conflict = SharedString::default();
        let entity = cx.entity().clone();
        self.capture_sub = Some(cx.intercept_keystrokes(move |event, _, app| {
            let keystroke = event.keystroke.clone();
            entity.update(app, |this, cx| this.on_shortcut_key(keystroke, cx));
            app.stop_propagation();
        }));
        cx.notify();
    }

    pub(super) fn end_shortcut_capture(&mut self, cx: &mut Context<Self>) {
        self.capture_sub = None;
        if self.settings_capture.take().is_some() {
            cx.notify();
        }
    }

    fn on_shortcut_key(&mut self, keystroke: gpui::Keystroke, cx: &mut Context<Self>) {
        let Some((id, slot)) = self.settings_capture.clone() else {
            return;
        };
        if keystroke.key.eq_ignore_ascii_case("escape") && !keystroke.modifiers.modified() {
            self.end_shortcut_capture(cx);
            return;
        }
        let Some(logical) = keymap::logical_from(&keystroke) else {
            return;
        };
        match keymap::find_conflict(&id, &logical) {
            Some(Conflict::Command(other)) => {
                self.settings_conflict = format!("{}: {}", t("settings.conflict"), t(other)).into();
                cx.notify();
            }
            Some(Conflict::TextInput) => {
                self.settings_conflict = t("settings.conflict_text");
                cx.notify();
            }
            None => {
                keymap::set_slot(&id, slot, logical);
                self.settings_conflict = SharedString::default();
                self.persist_prefs(cx);
                self.end_shortcut_capture(cx);
            }
        }
    }

    pub(super) fn persist_prefs(&mut self, cx: &mut Context<Self>) {
        let mut cfg = config::load();
        cfg.theme = ui_kit::theme_id().to_string();
        cfg.language = ui_kit::language_code(ui_kit::language()).to_string();
        cfg.show_menu_shortcuts = ui_kit::show_menu_shortcuts();
        cfg.reopen_last_project = ui_kit::reopen_last_project();
        cfg.check_updates = ui_kit::check_updates();
        cfg.shortcuts = ui_kit::shortcut_overrides();
        config::save(&cfg);
        keymap::install(cx);
        self.mask_tool.update(cx, |_, cx| cx.notify());
        self.photo_edit.update(cx, |_, cx| cx.notify());
        self.vector_edit.update(cx, |_, cx| cx.notify());
        self.apply_bg.update(cx, |_, cx| cx.notify());
        self.score_video.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub(super) fn check_updates_now(&mut self, cx: &mut Context<Self>) {
        if self.update_probe == UpdateProbe::Checking {
            return;
        }
        self.update_probe = UpdateProbe::Checking;
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::update::check_latest());
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                this.update(cx, |view, cx| view.finish_manual_update(result, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn finish_manual_update(&mut self, result: crate::update::UpdateCheck, cx: &mut Context<Self>) {
        match result {
            crate::update::UpdateCheck::UpToDate => self.update_probe = UpdateProbe::UpToDate,
            crate::update::UpdateCheck::Failed(err) => {
                self.update_probe = UpdateProbe::Failed(err);
            }
            crate::update::UpdateCheck::Available(info) => {
                self.update_probe = UpdateProbe::Idle;
                self.pending_update = Some(info);
                self.try_show_update_dialog(cx);
            }
        }
        cx.notify();
    }
}

fn swatch(hex: u32) -> impl IntoElement {
    div().w(px(14.)).h(px(14.)).bg(rgb(hex))
}

fn choice_mark(selected: bool) -> impl IntoElement {
    let theme = ui_kit::theme();
    div()
        .w(px(12.))
        .h(px(12.))
        .rounded_md()
        .border_1()
        .border_color(rgb(if selected {
            theme.accent
        } else {
            theme.border_strong
        }))
        .when(selected, |mark| mark.bg(rgb(theme.accent)))
}
