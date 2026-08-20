//! 导出弹窗与 ffmpeg 进度.

use super::*;

impl ScoreVideoApp {
    pub fn is_export_open(&self) -> bool {
        self.export_open
    }

    /// 读取帧率输入框当前文本, 解析失败或超范围时回退到合理值.
    pub(super) fn export_fps(&self, cx: &App) -> u32 {
        self.export_fps_input
            .read(cx)
            .text()
            .trim()
            .parse::<u32>()
            .unwrap_or(30)
            .clamp(1, 240)
    }

    pub(super) fn start_export(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let ext = self.export_container.ext();
        let label = self.export_container.label();
        Self::spawn_native_dialog(
            cx,
            move || {
                rfd::FileDialog::new()
                    .add_filter(label, &[ext])
                    .set_file_name(format!("output.{ext}"))
                    .save_file()
            },
            |this, out_path, cx| {
                let Some(out_path) = out_path else {
                    return;
                };
                this.begin_export(out_path, cx);
            },
        );
    }

    pub(super) fn begin_export(&mut self, out_path: PathBuf, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let (w, h) = ExportOptions::size_from_pool(&self.pool);
        let opts = ExportOptions {
            container: self.export_container,
            width: w,
            height: h,
            fps: self.export_fps(cx),
            crf: self.export_crf,
            out_path: out_path.clone(),
            fade_bg_rgb: self.fade_bg_rgb,
        };
        self.export_out_path = Some(out_path);
        self.exporting = true;
        self.export_progress = "准备中...".into();
        self.export_log.clear();
        cx.notify();
        let rx = crate::export::export_async(self.timeline.clone(), self.pool.clone(), opts);
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let stop = matches!(msg, ExportMsg::Done(_));
                let _ = this.update(cx, |view, cx| {
                    match msg {
                        ExportMsg::Progress(s) => {
                            let line: SharedString = s.into();
                            view.export_progress = line.clone();
                            view.export_log.push(line);
                            const MAX_LOG: usize = 400;
                            if view.export_log.len() > MAX_LOG {
                                let drop_n = view.export_log.len() - MAX_LOG;
                                view.export_log.drain(0..drop_n);
                            }
                        }
                        ExportMsg::Done(Ok(path)) => {
                            view.exporting = false;
                            view.export_open = false;
                            view.status = format!("导出完成: {}", path.display()).into();
                        }
                        ExportMsg::Done(Err(e)) => {
                            view.exporting = false;
                            view.export_open = false;
                            view.export_progress = "导出失败".into();
                            for line in e.lines() {
                                view.export_log.push(line.to_string().into());
                            }
                            view.show_error("导出失败", e, cx);
                        }
                    }
                    cx.notify();
                });
                if stop {
                    break;
                }
            }
        })
        .detach();
    }
    pub fn export_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = ExportOptions::size_from_pool(&self.pool);
        let out_label: SharedString = self
            .export_out_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(点击「开始导出」时选择保存路径)".to_string())
            .into();
        let mp4_on = self.export_container == Container::Mp4;

        div()
            .id("sv_export_overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            // 与宿主 Help 弹窗一致: 挡住背后命中, 背景静态不接收事件.
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
            .on_mouse_move(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
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
                    .w(px(440.))
                    .bg(rgb(0x1e293b))
                    .rounded_lg()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_color(rgb(0xe2e8f0))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("导出视频"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("容器格式"))
                            .child(self.btn(
                                "sv_fmt_mp4",
                                "MP4",
                                mp4_on,
                                |this, _, cx| {
                                    this.export_container = Container::Mp4;
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "sv_fmt_mkv",
                                "MKV",
                                !mp4_on,
                                |this, _, cx| {
                                    this.export_container = Container::Mkv;
                                    cx.notify();
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child(SharedString::from(self.export_container.audio_hint())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("分辨率"))
                            .child(
                                div()
                                    .text_sm()
                                    .child(format!("{w} x {h}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x64748b))
                                    .child("(与素材图片一致, 不可更改)"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("帧率"))
                            .child(self.btn(
                                "sv_fps_minus",
                                "-",
                                false,
                                |this, _, cx| {
                                    let v = this.export_fps(cx).saturating_sub(1).max(1);
                                    this.export_fps_input
                                        .update(cx, |t: &mut apply_bg::text_input::TextInput, cx| t.set_text(v.to_string(), cx));
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(div().w(px(64.)).child(self.export_fps_input.clone()))
                            .child(div().text_sm().child("fps"))
                            .child(self.btn(
                                "sv_fps_plus",
                                "+",
                                false,
                                |this, _, cx| {
                                    let v = (this.export_fps(cx) + 1).min(240);
                                    this.export_fps_input
                                        .update(cx, |t: &mut apply_bg::text_input::TextInput, cx| t.set_text(v.to_string(), cx));
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x64748b))
                                    .child("(可直接点击输入框改数字)"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().min_w(px(80.)).child("质量 (CRF)"))
                            .child(self.stepper(
                                "sv_crf_minus",
                                "sv_crf_plus",
                                format!("CRF {}", self.export_crf).into(),
                                |this, _, cx| {
                                    this.export_crf = this.export_crf.saturating_sub(1).max(14);
                                    cx.notify();
                                },
                                |this, _, cx| {
                                    this.export_crf = (this.export_crf + 1).min(28);
                                    cx.notify();
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x64748b))
                            .child("CRF (Constant Rate Factor) 是 x264 编码的质量参数: 数值越小画质越好、文件越大, 越大则画质越差、文件越小; 0 近乎无损, 18~23 常见于\"肉眼无差\"的高质量, 28 起画质明显下降. 与分辨率/码率无关, 是恒定质量而非恒定码率的编码方式."),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child(out_label),
                    )
                    .child(if self.exporting {
                        div()
                            .text_xs()
                            .text_color(rgb(0xfbbf24))
                            .child(self.export_progress.clone())
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(if self.export_log.is_empty() {
                        div().into_any_element()
                    } else {
                        // ffmpeg 不再弹终端窗口, 它的原始输出 (进度/报错) 就
                        // 直接滚动显示在这里; 只展示最近若干行, 足够看清当前
                        // 在干什么或者失败原因.
                        const SHOW_LAST: usize = 10;
                        let start = self.export_log.len().saturating_sub(SHOW_LAST);
                        div()
                            .id("sv_export_log")
                            .w_full()
                            .max_h(px(150.))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(0x0b1220))
                            .border_1()
                            .border_color(rgb(0x1e293b))
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .children(
                                self.export_log[start..]
                                    .iter()
                                    .map(|l| div().child(l.clone())),
                            )
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .mt_2()
                            .child(self.btn(
                                "sv_export_cancel",
                                "关闭",
                                false,
                                |this, _, cx| {
                                    this.export_open = false;
                                    cx.notify();
                                },
                                cx,
                            ))
                            .child(self.btn(
                                "sv_export_go",
                                if self.exporting { "导出中..." } else { "开始导出" },
                                true,
                                |this, window, cx| this.start_export(window, cx),
                                cx,
                            )),
                    ),
            )
    }
}
