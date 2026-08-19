//! 谱面加底色并按比例裁切 (可嵌入 staff_crop 等宿主).

pub mod config;
pub mod gui;
pub mod process;
pub mod text_input;

/// 界面中文字体: Windows 用雅黑, macOS 用苹方.
pub fn ui_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "PingFang SC"
    } else if cfg!(windows) {
        "Microsoft YaHei UI"
    } else {
        "sans-serif"
    }
}
