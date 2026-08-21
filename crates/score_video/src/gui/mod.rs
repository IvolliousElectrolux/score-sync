//! GPUI 图形界面: 视频轨道编辑 (预览窗 + 三轨 + 素材池 + 导出).
//!
//! 按职责拆开, `ScoreVideoApp` 仍是唯一状态机:
//! - `types` 常量/拖拽/波形
//! - `playback` 播放与时间轴编辑
//! - `audio_ui` 导入/分割音频
//! - `drag` 跨面板拖拽
//! - `preview` / `tracks` 左侧工作区
//! - `pool` 素材池
//! - `export_ui` 导出弹窗

mod audio_ui;
mod drag;
mod export_ui;
mod playback;
mod pool;
mod preview;
mod tracks;
mod types;

pub(crate) use types::*;

pub(crate) use std::collections::HashMap;
pub(crate) use std::path::PathBuf;
pub(crate) use std::sync::Arc;
pub(crate) use std::time::Duration;

pub(crate) use gpui::{
    actions, canvas, div, point, prelude::*, px, rgb, rgba, size, App, Application, Bounds,
    Context, Corners, CursorStyle, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Render,
    RenderImage, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
pub(crate) use image::Frame;
pub(crate) use rodio::Source;
pub(crate) use smallvec::smallvec;
pub(crate) use uuid::Uuid;

pub(crate) use crate::audio::AudioEngine;
pub(crate) use crate::export::{Container, ExportMsg, ExportOptions};
pub(crate) use crate::model::{AudioClip, FadeKind, MaterialItem, Timeline};

actions!(
    score_video,
    [
        PlayPause,
        SeekBack,
        SeekForward,
        SeekBackBig,
        SeekForwardBig,
        InsertNext,
        MarkFadeIn,
        MarkFadeOut,
        DeleteSelected,
        Undo,
        Redo,
    ]
);

/// 嵌入宿主 (score_sync) 启动时调用一次, 注册「视频」标签页下的快捷键.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", PlayPause, Some("ScoreVideo")),
        KeyBinding::new("left", SeekBack, Some("ScoreVideo")),
        KeyBinding::new("right", SeekForward, Some("ScoreVideo")),
        KeyBinding::new("shift-left", SeekBackBig, Some("ScoreVideo")),
        KeyBinding::new("shift-right", SeekForwardBig, Some("ScoreVideo")),
        KeyBinding::new("n", InsertNext, Some("ScoreVideo")),
        KeyBinding::new("i", MarkFadeIn, Some("ScoreVideo")),
        KeyBinding::new("o", MarkFadeOut, Some("ScoreVideo")),
        KeyBinding::new("delete", DeleteSelected, Some("ScoreVideo")),
        KeyBinding::new("backspace", DeleteSelected, Some("ScoreVideo")),
    ]);
    cx.bind_keys(apply_bg::bind_primary("z", Undo, Some("ScoreVideo")));
    cx.bind_keys(apply_bg::bind_primary("y", Redo, Some("ScoreVideo")));
    cx.bind_keys(apply_bg::bind_primary("shift-z", Redo, Some("ScoreVideo")));
}

pub struct ScoreVideoApp {
    focus_handle: FocusHandle,
    pool: Vec<MaterialItem>,
    render_cache: std::collections::HashMap<String, Arc<RenderImage>>,
    /// 全分辨率 RGBA 热集 (按 group_id), 超出容量时 LRU 淘汰.
    image_hot: std::collections::HashMap<String, Arc<image::RgbaImage>>,
    image_lru: std::collections::VecDeque<String>,
    image_lru_cap: usize,
    timeline: Timeline,
    audio: AudioEngine,
    aspect_w: u32,
    aspect_h: u32,
    tracks_bounds: Bounds<Pixels>,
    preview_bounds: Bounds<Pixels>,
    /// 左侧工作区屏幕 bounds, 供淡入淡出右键菜单定位.
    left_bounds: Bounds<Pixels>,
    /// 音频片段屏幕 bounds (按当前顺序下标), 供排序拖拽判定落点/幽灵原点.
    audio_clip_bounds: HashMap<usize, Bounds<Pixels>>,
    /// 底部横向缩放/滚动条自身的屏幕 bounds.
    track_bar_bounds: Bounds<Pixels>,
    px_per_sec: f32,
    /// 用户是否已通过 Ctrl+滚轮/底部缩放条手动缩放过轨道 (之后不再自动适应宽度).
    track_user_zoomed: bool,
    /// 轨道横向滚动偏移 (秒).
    track_scroll: f64,
    pool_scroll: ScrollHandle,
    /// 素材池中当前展开显示预览图的条目 (点击而非拖动时切换).
    expanded_pool: Option<String>,
    /// 音频波形峰值缓存: key = 源文件路径, 每个源按时长采样出足够密度的
    /// 基础峰值点, 绘制时再按当前片段的屏幕宽度 (随缩放变化) 重新降采样/
    /// 插值一次, 分辨率因此会跟着缩放丝滑改变.
    waveform_cache: std::collections::HashMap<PathBuf, Arc<Vec<f32>>>,
    /// 正在后台解码计算波形中的路径, 避免同一文件重复起线程.
    waveform_pending: std::collections::HashSet<PathBuf>,
    drag: Option<VideoDrag>,
    status: SharedString,
    fade_menu: Option<FadeContextMenu>,
    /// 淡向底色时用的 RGB (宿主从工程底色图采样).
    fade_bg_rgb: [u8; 3],
    /// 「分割音频」按钮按下后进入待命: 下一次鼠标按下时若落在音频轨道内就
    /// 从该处切开对应片段, 否则 (点在别处) 直接取消, 不作任何改动.
    split_audio_armed: bool,
    /// 用户可见错误弹窗 (标题, 正文). 导出弹窗打开时优先显示导出界面.
    error_dialog: Option<(SharedString, SharedString)>,
    export_open: bool,
    export_container: Container,
    /// 帧率: 可直接点击输入框改数字 (与 CRF 那种只能加减的 stepper 不同),
    /// 复用 `apply_bg` 里已经有的文本框组件, 无需重复注册快捷键 (宿主启动
    /// 时已经调用过 `apply_bg::text_input::bind_keys`).
    export_fps_input: Entity<apply_bg::text_input::TextInput>,
    export_crf: u32,
    export_out_path: Option<PathBuf>,
    exporting: bool,
    export_progress: SharedString,
    /// ffmpeg 的原始输出 (不再弹终端窗口, 直接在导出弹窗里滚动显示最近几
    /// 行), 超过上限就丢掉最旧的.
    export_log: Vec<SharedString>,
    /// 播放代数: 每次开始播放自增, 供 ticker 判断自身是否已过期而自行退出,
    /// 避免播放/暂停快速切换时残留多个 ticker 任务.
    play_gen: u64,
    undo_stack: Vec<crate::model::TimelineSnapshot>,
    redo_stack: Vec<crate::model::TimelineSnapshot>,
    /// 当前拖拽是否已为本次变更压过撤销栈.
    drag_undo_pushed: bool,
    /// 倍速下拉是否打开.
    speed_menu_open: bool,
    /// 倍速按钮屏幕 bounds, 供下拉定位.
    speed_btn_bounds: Bounds<Pixels>,
    /// 倍速菜单悬浮层 bounds (与绝对定位同一坐标系, 对齐蒙版取色器).
    speed_layer_bounds: Bounds<Pixels>,
}

impl ScoreVideoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let app = Self {
            focus_handle: cx.focus_handle(),
            pool: Vec::new(),
            render_cache: std::collections::HashMap::new(),
            image_hot: std::collections::HashMap::new(),
            image_lru: std::collections::VecDeque::new(),
            image_lru_cap: 12,
            timeline: Timeline::new(),
            audio: AudioEngine::new(),
            aspect_w: 16,
            aspect_h: 9,
            tracks_bounds: Bounds::default(),
            preview_bounds: Bounds::default(),
            left_bounds: Bounds::default(),
            audio_clip_bounds: HashMap::new(),
            track_bar_bounds: Bounds::default(),
            px_per_sec: 20.0,
            track_user_zoomed: false,
            track_scroll: 0.0,
            pool_scroll: ScrollHandle::new(),
            expanded_pool: None,
            waveform_cache: std::collections::HashMap::new(),
            waveform_pending: std::collections::HashSet::new(),
            drag: None,
            status: "就绪. N 插入下一张组合, 空格播放/暂停, I/O 标记淡入淡出.".into(),
            fade_menu: None,
            fade_bg_rgb: DEFAULT_FADE_BG_RGB,
            error_dialog: None,
            split_audio_armed: false,
            export_open: false,
            export_container: Container::Mp4,
            export_fps_input: cx.new(|cx| apply_bg::text_input::TextInput::new(cx, "30", "帧率")),
            export_crf: 18,
            export_out_path: None,
            exporting: false,
            export_progress: SharedString::default(),
            export_log: Vec::new(),
            play_gen: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            drag_undo_pushed: false,
            speed_menu_open: false,
            speed_btn_bounds: Bounds::default(),
            speed_layer_bounds: Bounds::default(),
        };
        app
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// 项目底色宽高比 (用于预览 letterbox 与导出默认分辨率).
    pub fn set_aspect(&mut self, w: u32, h: u32) {
        self.aspect_w = w.max(1);
        self.aspect_h = h.max(1);
    }

    /// 工程底色的代表色, 供「保持背景为底色」的淡入淡出叠色/导出.
    pub fn set_fade_bg_rgb(&mut self, rgb: [u8; 3]) {
        self.fade_bg_rgb = rgb;
    }

    /// 供宿主保存工程时读取当前时间轴 (纯数据快照, 不含选中态), 一并写入工程文件.
    pub fn timeline_snapshot(&self) -> crate::model::TimelineSnapshot {
        self.timeline.snapshot()
    }

    /// 供宿主载入工程后写回时间轴 (重新生成各条目 id, 并把音频重接到播放引擎).
    pub fn load_timeline_snapshot(
        &mut self,
        snap: crate::model::TimelineSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.timeline.load_snapshot(snap);
        self.audio.set_clips(self.timeline.audio_clips.clone());
        self.track_scroll = 0.0;
        self.track_user_zoomed = false;
        self.drag = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.drag_undo_pushed = false;
        let missing: Vec<PathBuf> = self
            .timeline
            .audio_clips
            .iter()
            .filter(|c| !c.path.is_file())
            .map(|c| c.path.clone())
            .collect();
        if !missing.is_empty() {
            let listed = missing
                .iter()
                .map(|p| crate::error::Error::AudioMissing(p.clone()).to_string())
                .collect::<Vec<_>>()
                .join("\n");
            self.show_error(
                "音频文件找不到",
                format!(
                    "工程里的音频路径已失效, 谱面切片和淡入淡出仍在.\n\
                     把文件放回原处即可; 路径变了则先删音频轨上的旧片段再导入.\n\n{listed}"
                ),
                cx,
            );
        }
        cx.notify();
    }

    pub fn show_error(
        &mut self,
        title: impl Into<String>,
        err: impl std::fmt::Display,
        cx: &mut Context<Self>,
    ) {
        let body = err.to_string();
        self.status = body.clone().into();
        self.error_dialog = Some((title.into().into(), body.into()));
        cx.notify();
    }

    pub fn is_error_open(&self) -> bool {
        self.error_dialog.is_some()
    }

    pub fn error_dialog(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, body) = self
            .error_dialog
            .clone()
            .unwrap_or_else(|| ("出错".into(), SharedString::default()));
        div()
            .id("sv_error_overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
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
                    .max_h(px(420.))
                    .bg(rgb(0x1e293b))
                    .rounded_lg()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_color(rgb(0xf8fafc))
                            .text_lg()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xcbd5e1))
                            .whitespace_normal()
                            .child(body),
                    )
                    .child(
                        div()
                            .id("sv_error_ok")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .text_color(rgb(0xffffff))
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0x1d4ed8)))
                            .child("确定")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.error_dialog = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }
}

impl Focusable for ScoreVideoApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ScoreVideoApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = if self.export_open {
            self.export_dialog(cx).into_any_element()
        } else if self.error_dialog.is_some() {
            self.error_dialog(cx).into_any_element()
        } else {
            div().into_any_element()
        };
        // 注意: 宿主 score_sync 实际走 left_panel/right_panel, 不调用本 Render;
        // 「分割音频」的点击逻辑已挂在音频轨道/片段的 on_mouse_down 上.
        div()
            .id("sv_root")
            .key_context("ScoreVideo")
            .track_focus(&self.focus_handle)
            .relative()
            .on_action(cx.listener(|this, _: &PlayPause, _, cx| this.play_pause(cx)))
            .on_action(cx.listener(|this, _: &SeekBack, _, cx| this.seek_by(-1.0, cx)))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| this.seek_by(1.0, cx)))
            .on_action(cx.listener(|this, _: &SeekBackBig, _, cx| this.seek_by(-5.0, cx)))
            .on_action(cx.listener(|this, _: &SeekForwardBig, _, cx| this.seek_by(5.0, cx)))
            .on_action(cx.listener(|this, _: &InsertNext, _, cx| this.insert_next(cx)))
            .on_action(cx.listener(|this, _: &MarkFadeIn, _, cx| this.mark_fade_in(cx)))
            .on_action(cx.listener(|this, _: &MarkFadeOut, _, cx| this.mark_fade_out(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| this.delete_selected(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo(cx)))
            .flex()
            .flex_row()
            .size_full()
            .font_family(apply_bg::ui_font())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .child(self.left_panel(cx)),
            )
            .child(
                div()
                    .w(px(300.))
                    .flex_shrink_0()
                    .h_full()
                    .child(self.right_panel(cx)),
            )
            .child(overlay)
    }
}

/// 独立运行入口 (仅供该 crate 单独调试): 传入若干图片作为素材池 + 可选音频.
pub fn run_gui(images: Vec<PathBuf>, audio: Option<PathBuf>) {
    Application::new().run(move |cx: &mut App| {
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("视频轨道编辑 (调试)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let images = images.clone();
                let audio = audio.clone();
                cx.new(|cx| {
                    let mut app = ScoreVideoApp::new(cx);
                    let mut pool = Vec::new();
                    for (i, p) in images.iter().enumerate() {
                        if let Ok(im) = image::open(p) {
                            let rgba = im.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            let cache = std::env::temp_dir().join(format!("sv_dbg_{i}.png"));
                            let _ = rgba.save(&cache);
                            pool.push(MaterialItem {
                                group_id: format!("g{i}"),
                                label: p
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("素材")
                                    .to_string()
                                    .into(),
                                cache_path: cache,
                                width: w,
                                height: h,
                            });
                        }
                    }
                    app.set_pool(pool, cx);
                    if let Some(a) = audio {
                        match crate::audio::probe_duration(&a) {
                            Ok(dur) => {
                                app.timeline.audio_clips.push(AudioClip {
                                    id: Uuid::new_v4(),
                                    path: a.clone(),
                                    label: a
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("audio")
                                        .to_string()
                                        .into(),
                                    duration: dur,
                                    offset: 0.0,
                                });
                                app.timeline.fit_after_audio_change();
                            }
                            Err(e) => app.show_error("导入音频失败", e, cx),
                        }
                    }
                    app.focus_handle.focus(window);
                    app
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
