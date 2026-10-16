//! 跨平台主修饰键: Windows/Linux = Ctrl, macOS = Command.

use gpui::{Action, KeyBinding, Modifiers, ScrollDelta, ScrollWheelEvent};

/// 多选、滚轮缩放等: Mac 上 ⌘ (也认 Control), 其它平台 Ctrl.
#[inline]
pub fn is_primary_mod(m: &Modifiers) -> bool {
    m.secondary() || m.control
}

/// 界面文案里的修饰键: `"⌘"` 或 `"Ctrl+"`.
pub fn primary_mod() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}

/// `"⌘⇧"` 或 `"Ctrl+Shift+"`.
pub fn primary_shift() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘⇧"
    } else {
        "Ctrl+Shift+"
    }
}

/// Option 或 Alt. macOS 的 ⌥ 对应 Windows 的 Alt, 不是 Win 键.
pub fn alt_mod() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Alt"
    }
}

/// `"⌘⌥"` 或 `"Ctrl+Alt+"`. Control (macOS) 和 Win 键都不是这一组.
pub fn primary_alt() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘⌥"
    } else {
        "Ctrl+Alt+"
    }
}

pub fn with_mod(name: &str, key: &str) -> String {
    format!("{name} ({}{key})", primary_mod())
}

/// Ctrl+Shift+滚轮换页. 向上 (正) 为 `-1`, 向下为 `+1`.
/// 按住 Shift 时 Windows 把滚动量放在 x, 取绝对值更大的那一轴.
/// 一格只翻一页, 不按「每次滚动行数」连翻.
pub fn wheel_page_delta(ev: &ScrollWheelEvent) -> Option<i32> {
    let (x, y, min) = match ev.delta {
        ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y), 16.0),
        ScrollDelta::Lines(l) => (l.x, l.y, 0.5),
    };
    let v = if x.abs() > y.abs() { x } else { y };
    if v.abs() < min {
        None
    } else if v > 0.0 {
        Some(-1)
    } else {
        Some(1)
    }
}

/// `rest` 如 `"s"` / `"shift-s"` / `"shift-z"`.
/// `secondary-` 在 Mac 是 ⌘、在 Win/Linux 是 Ctrl; 另绑一份 `ctrl-` 让 Mac 上 Control 也能用.
pub fn bind_primary<A: Action + Clone>(
    rest: &str,
    action: A,
    context: Option<&str>,
) -> [KeyBinding; 2] {
    [
        KeyBinding::new(&format!("secondary-{rest}"), action.clone(), context),
        KeyBinding::new(&format!("ctrl-{rest}"), action, context),
    ]
}
