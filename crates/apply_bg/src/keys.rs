//! 跨平台主修饰键: Windows/Linux = Ctrl, macOS = Command.

use gpui::{Action, KeyBinding, Modifiers};

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

pub fn with_mod(name: &str, key: &str) -> String {
    format!("{name} ({}{key})", primary_mod())
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
