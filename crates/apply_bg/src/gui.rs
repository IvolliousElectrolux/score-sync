//! GPUI 图形界面.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, relative, size, App, Application, Bounds, Context, Entity,
    InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};

use crate::config::{self, Config};
use crate::process::{
    self, format_aspect, parse_aspect, ProcessResult, DEFAULT_ASPECT_H, DEFAULT_ASPECT_W,
};
use crate::text_input::{self, TextInput};

#[derive(Clone)]
enum UiMsg {
    Progress {
        done: usize,
        total: usize,
        name: SharedString,
    },
    Finished(Result<ProcessResult, String>),
}

pub struct ApplyBgApp {
    bg_input: Entity<TextInput>,
    in_input: Entity<TextInput>,
    out_input: Entity<TextInput>,
    aspect_w_input: Entity<TextInput>,
    aspect_h_input: Entity<TextInput>,
    /// 已从配置读到比例, 或用户改过/点过恢复默认 → 持久化时写入 aspect.
    aspect_customized: bool,
    status: SharedString,
    progress_done: usize,
    progress_total: usize,
    running: bool,
    hint: SharedString,
}

impl ApplyBgApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cfg = config::load();
        let aspect_customized = !cfg.aspect.trim().is_empty();
        let (aw, ah) = cfg.aspect_or_default();
        let bg_s = cfg.bg;
        let in_s = cfg.in_dir;
        let out_s = if cfg.out_dir.is_empty() && !in_s.is_empty() {
            PathBuf::from(&in_s).join("加底色").display().to_string()
        } else {
            cfg.out_dir
        };

        let bg_input = cx.new(|cx| TextInput::new(cx, bg_s, "底色图片路径…"));
        let in_input = cx.new(|cx| TextInput::new(cx, in_s, "谱面输入目录…"));
        let out_input = cx.new(|cx| TextInput::new(cx, out_s, "输出目录…"));
        let aspect_w_input = cx.new(|cx| TextInput::new(cx, aw.to_string(), "宽"));
        let aspect_h_input = cx.new(|cx| TextInput::new(cx, ah.to_string(), "高"));

        Self {
            bg_input,
            in_input,
            out_input,
            aspect_w_input,
            aspect_h_input,
            aspect_customized,
            status: "就绪".into(),
            progress_done: 0,
            progress_total: 0,
            running: false,
            hint: format!(
                "裁切宽=谱面宽, 高=宽×比例高/比例宽. 路径记入 %APPDATA%\\apply_bg; 比例仅在修改或恢复默认后保存 (默认 {DEFAULT_ASPECT_W}:{DEFAULT_ASPECT_H})."
            )
            .into(),
        }
    }

    fn aspect_text(&self, cx: &Context<Self>) -> String {
        let w = self.aspect_w_input.read(cx).text().trim().to_string();
        let h = self.aspect_h_input.read(cx).text().trim().to_string();
        format!("{w}:{h}")
    }

    fn aspect_differs_from_default(&self, cx: &Context<Self>) -> bool {
        match parse_aspect(&self.aspect_text(cx)) {
            Ok((w, h)) => w != DEFAULT_ASPECT_W || h != DEFAULT_ASPECT_H,
            Err(_) => true,
        }
    }

    fn current_config(&self, cx: &Context<Self>) -> Config {
        let write_aspect = self.aspect_customized || self.aspect_differs_from_default(cx);
        Config {
            bg: self.bg_input.read(cx).text().trim().to_string(),
            in_dir: self.in_input.read(cx).text().trim().to_string(),
            out_dir: self.out_input.read(cx).text().trim().to_string(),
            aspect: if write_aspect {
                self.aspect_text(cx)
            } else {
                String::new()
            },
        }
    }

    fn persist(&self, cx: &Context<Self>) {
        config::save(&self.current_config(cx));
    }

    fn path_row(
        &self,
        label: &'static str,
        btn_id: &'static str,
        input: Entity<TextInput>,
        browse: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let browse = Arc::new(browse);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .child(
                div()
                    .w(px(48.))
                    .text_color(rgb(0x334155))
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(input))
            .child(self.btn(btn_id, "…", move |this, window, cx| browse(this, window, cx), cx))
    }

    fn aspect_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .child(
                div()
                    .w(px(48.))
                    .text_color(rgb(0x334155))
                    .child("比例"),
            )
            .child(
                div()
                    .w(px(96.))
                    .child(self.aspect_w_input.clone()),
            )
            .child(
                div()
                    .text_color(rgb(0x64748b))
                    .child(":"),
            )
            .child(
                div()
                    .w(px(96.))
                    .child(self.aspect_h_input.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x94a3b8))
                    .child("(宽:高)"),
            )
            .child(div().flex_1())
            .child(self.btn(
                "aspect_reset",
                "恢复默认",
                Self::reset_aspect,
                cx,
            ))
    }

    fn btn(
        &self,
        id: &'static str,
        label: &'static str,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgb(0xe2e8f0))
            .border_1()
            .border_color(rgb(0x94a3b8))
            .text_color(rgb(0x0f172a))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0xcbd5e1)))
            .active(|s| s.bg(rgb(0x94a3b8)))
            .child(label)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
            )
    }

    fn primary_btn(
        &self,
        label: SharedString,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bg = if enabled { rgb(0x2563eb) } else { rgb(0x94a3b8) };
        let hover = if enabled { rgb(0x1d4ed8) } else { rgb(0x94a3b8) };
        div()
            .id("run")
            .px_4()
            .py_2()
            .rounded_md()
            .bg(bg)
            .text_color(rgb(0xffffff))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
                    )
            })
            .child(label)
    }

    fn reset_aspect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.aspect_w_input.update(cx, |input, cx| {
            input.set_text(DEFAULT_ASPECT_W.to_string(), cx);
        });
        self.aspect_h_input.update(cx, |input, cx| {
            input.set_text(DEFAULT_ASPECT_H.to_string(), cx);
        });
        self.aspect_customized = true;
        self.persist(cx);
        self.status = format!(
            "已恢复默认比例 {}.",
            format_aspect(DEFAULT_ASPECT_W, DEFAULT_ASPECT_H)
        )
        .into();
        cx.notify();
    }

    fn pick_bg(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let start = PathBuf::from(self.bg_input.read(cx).text());
        let mut dialog = rfd::FileDialog::new()
            .set_title("选择底色")
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"]);
        if start.is_file() {
            dialog = dialog
                .set_file_name(
                    start
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("底色.png"),
                )
                .set_directory(start.parent().unwrap_or(std::path::Path::new(".")));
        }
        if let Some(p) = dialog.pick_file() {
            self.bg_input
                .update(cx, |input, cx| input.set_text(p.display().to_string(), cx));
            self.persist(cx);
            cx.notify();
        }
    }

    fn pick_in(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let start = PathBuf::from(self.in_input.read(cx).text());
        let picked = rfd::FileDialog::new()
            .set_title("选择谱面目录")
            .set_directory(if start.is_dir() {
                start
            } else {
                PathBuf::from(".")
            })
            .pick_folder();
        if let Some(p) = picked {
            let out = p.join("加底色").display().to_string();
            self.in_input
                .update(cx, |input, cx| input.set_text(p.display().to_string(), cx));
            self.out_input
                .update(cx, |input, cx| input.set_text(out, cx));
            self.persist(cx);
            cx.notify();
        }
    }

    fn pick_out(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let start = PathBuf::from(self.out_input.read(cx).text());
        let picked = rfd::FileDialog::new()
            .set_title("选择输出目录")
            .set_directory(if start.is_dir() {
                start
            } else if let Some(parent) = start.parent().filter(|p| p.is_dir()) {
                parent.to_path_buf()
            } else {
                PathBuf::from(".")
            })
            .pick_folder();
        if let Some(p) = picked {
            self.out_input
                .update(cx, |input, cx| input.set_text(p.display().to_string(), cx));
            self.persist(cx);
            cx.notify();
        }
    }

    fn start_run(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let bg = PathBuf::from(self.bg_input.read(cx).text().trim());
        let in_dir = PathBuf::from(self.in_input.read(cx).text().trim());
        let out_dir = PathBuf::from(self.out_input.read(cx).text().trim());

        let (aspect_w, aspect_h) = match parse_aspect(&self.aspect_text(cx)) {
            Ok(v) => v,
            Err(e) => {
                self.status = e.into();
                cx.notify();
                return;
            }
        };

        if bg.as_os_str().is_empty() {
            self.status = "请先选择底色图片.".into();
            cx.notify();
            return;
        }
        if in_dir.as_os_str().is_empty() {
            self.status = "请先选择谱面输入目录.".into();
            cx.notify();
            return;
        }
        if !bg.is_file() {
            self.status = format!("底色不存在: {}", bg.display()).into();
            cx.notify();
            return;
        }
        if !in_dir.is_dir() {
            self.status = format!("输入目录无效: {}", in_dir.display()).into();
            cx.notify();
            return;
        }
        match process::list_images(&in_dir) {
            Ok(files) if files.is_empty() => {
                self.status = "输入目录没有图片.".into();
                cx.notify();
                return;
            }
            Ok(files) => {
                self.progress_total = files.len();
                self.progress_done = 0;
            }
            Err(e) => {
                self.status = format!("无法读取目录: {e}").into();
                cx.notify();
                return;
            }
        }

        if self.aspect_differs_from_default(cx) {
            self.aspect_customized = true;
        }
        self.persist(cx);
        self.running = true;
        self.status = format!(
            "处理中… (比例 {})",
            format_aspect(aspect_w, aspect_h)
        )
        .into();
        cx.notify();

        let (tx, rx) = async_channel::unbounded::<UiMsg>();
        let tx_progress = tx.clone();

        std::thread::spawn(move || {
            let result = process::process_folder(
                &in_dir,
                &bg,
                &out_dir,
                aspect_w,
                aspect_h,
                None,
                move |done, total, name| {
                    let _ = tx_progress.send_blocking(UiMsg::Progress {
                        done,
                        total,
                        name: name.to_string().into(),
                    });
                },
            );
            let _ = tx.send_blocking(UiMsg::Finished(result));
        });

        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let stop = matches!(msg, UiMsg::Finished(_));
                this.update(cx, |view, cx| {
                    match msg {
                        UiMsg::Progress { done, total, name } => {
                            view.progress_done = done;
                            view.progress_total = total;
                            view.status = format!("{done}/{total}  {name}").into();
                        }
                        UiMsg::Finished(Ok(res)) => {
                            view.running = false;
                            view.progress_done = res.ok + res.errors.len();
                            view.progress_total = view.progress_done.max(view.progress_total);
                            let mut text = format!(
                                "成功 {} 张 → {} ({:.2}s)",
                                res.ok,
                                res.out_dir.display(),
                                res.elapsed_secs
                            );
                            if !res.errors.is_empty() {
                                text.push_str(&format!("; 失败 {} 条", res.errors.len()));
                                for e in res.errors.iter().take(5) {
                                    text.push_str(&format!(" | {e}"));
                                }
                            }
                            view.status = text.into();
                        }
                        UiMsg::Finished(Err(e)) => {
                            view.running = false;
                            view.status = format!("失败: {e}").into();
                        }
                    }
                    cx.notify();
                })
                .ok();
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    /// 嵌入宿主侧栏的表单面板 (与独立窗口内容一致, 无外层全屏壳).
    pub fn panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fraction = if self.progress_total == 0 {
            0.0
        } else {
            (self.progress_done as f32 / self.progress_total as f32).clamp(0.0, 1.0)
        };
        let run_label: SharedString = if self.running {
            "处理中…".into()
        } else {
            "一键处理".into()
        };
        let can_run = !self.running;
        let title: SharedString = match parse_aspect(&self.aspect_text(cx)) {
            Ok((w, h)) => format!("谱面加底色 / {w}:{h}").into(),
            Err(_) => "谱面加底色".into(),
        };

        div()
            .id("apply_bg_panel")
            .flex()
            .flex_col()
            .w_full()
            .gap_3()
            .p_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(self.path_row(
                "底色",
                "browse_bg",
                self.bg_input.clone(),
                Self::pick_bg,
                cx,
            ))
            .child(self.path_row(
                "输入",
                "browse_in",
                self.in_input.clone(),
                Self::pick_in,
                cx,
            ))
            .child(self.path_row(
                "输出",
                "browse_out",
                self.out_input.clone(),
                Self::pick_out,
                cx,
            ))
            .child(self.aspect_row(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x64748b))
                    .child(self.hint.clone()),
            )
            .child(
                div()
                    .w_full()
                    .h(px(10.))
                    .rounded_full()
                    .bg(rgb(0xe2e8f0))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .bg(rgb(0x2563eb))
                            .rounded_full(),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x334155))
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .child(self.primary_btn(run_label, can_run, Self::start_run, cx)),
            )
    }
}

impl Render for ApplyBgApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf8fafc))
            .text_color(rgb(0x0f172a))
            .font_family("Microsoft YaHei UI")
            .child(self.panel(cx))
    }
}

pub fn run_gui() {
    Application::new().run(|cx: &mut App| {
        text_input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(820.), px(430.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("谱面加底色".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(ApplyBgApp::new),
        )
        .unwrap();
        cx.activate(true);
    });
}
