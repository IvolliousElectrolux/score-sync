//! 主题, 语言, 菜单键位提示. 主程序和各个嵌入面板共用.

mod i18n;
mod theme;

pub use i18n::{language_code, parse_language, t, t_src, Language};
pub use theme::{card, chrome, on_accent, overlay, parse_theme_id, preset, Theme, THEME_IDS};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use serde::Deserialize;

struct State {
    theme_id: &'static str,
    language: Language,
    show_menu_shortcuts: bool,
    reopen_last_project: bool,
    check_updates: bool,
    shortcuts: HashMap<String, Vec<String>>,
    hints: HashMap<String, String>,
}

fn state() -> std::sync::RwLockWriteGuard<'static, State> {
    STATE.write().unwrap_or_else(|err| err.into_inner())
}

fn read_state() -> std::sync::RwLockReadGuard<'static, State> {
    STATE.read().unwrap_or_else(|err| err.into_inner())
}

static STATE: LazyLock<RwLock<State>> = LazyLock::new(|| {
    RwLock::new(State {
        theme_id: "default",
        language: Language::Zh,
        show_menu_shortcuts: true,
        reopen_last_project: true,
        check_updates: true,
        shortcuts: HashMap::new(),
        hints: HashMap::new(),
    })
});

pub fn theme() -> Theme {
    preset(read_state().theme_id)
}

pub fn theme_id() -> &'static str {
    read_state().theme_id
}

pub fn language() -> Language {
    read_state().language
}

pub fn show_menu_shortcuts() -> bool {
    read_state().show_menu_shortcuts
}

pub fn reopen_last_project() -> bool {
    read_state().reopen_last_project
}

pub fn check_updates() -> bool {
    read_state().check_updates
}

pub fn shortcut_overrides() -> HashMap<String, Vec<String>> {
    read_state().shortcuts.clone()
}

pub fn set_theme_id(id: &str) {
    state().theme_id = parse_theme_id(id);
}

pub fn set_language(lang: Language) {
    state().language = lang;
}

pub fn set_show_menu_shortcuts(show: bool) {
    state().show_menu_shortcuts = show;
}

pub fn set_reopen_last_project(reopen: bool) {
    state().reopen_last_project = reopen;
}

pub fn set_check_updates(check: bool) {
    state().check_updates = check;
}

pub fn set_shortcut_overrides(map: HashMap<String, Vec<String>>) {
    state().shortcuts = map;
}

pub fn set_shortcut_hint(id: &str, display: &str) {
    if display.is_empty() {
        state().hints.remove(id);
    } else {
        state().hints.insert(id.to_string(), display.to_string());
    }
}

pub fn clear_shortcut_hints() {
    state().hints.clear();
}

pub fn shortcut_hint(id: &str) -> Option<String> {
    read_state().hints.get(id).cloned()
}

/// 纯图标按钮的悬停提示: 始终带上键位.
pub fn always_hint(name: impl AsRef<str>, command_id: &str) -> String {
    let name = name.as_ref();
    match shortcut_hint(command_id) {
        Some(hint) if !hint.is_empty() => format!("{name} ({hint})"),
        _ => name.to_string(),
    }
}

/// 菜单和带文字的按钮. 「在菜单中显示快捷键」关掉时只留名称.
pub fn menu_label(name: impl AsRef<str>, command_id: &str) -> String {
    if show_menu_shortcuts() {
        always_hint(name, command_id)
    } else {
        name.as_ref().to_string()
    }
}

#[derive(Deserialize)]
struct DiskPrefs {
    #[serde(default)]
    theme: String,
    #[serde(default)]
    language: String,
    #[serde(default = "default_true")]
    show_menu_shortcuts: bool,
    #[serde(default = "default_true")]
    reopen_last_project: bool,
    #[serde(default = "default_true")]
    check_updates: bool,
    #[serde(default)]
    shortcuts: HashMap<String, Vec<String>>,
}

fn default_true() -> bool {
    true
}

pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join("score_sync");
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("score_sync");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("score_sync");
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".config").join("score_sync");
        }
    }
    std::env::temp_dir().join("score_sync")
}

/// 独立小工具启动时读同一份 config.json 的外观字段.
pub fn load_prefs_from_disk() {
    let path = config_dir().join("config.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(prefs) = serde_json::from_str::<DiskPrefs>(&text) else {
        return;
    };
    apply_prefs(
        &prefs.theme,
        &prefs.language,
        prefs.show_menu_shortcuts,
        prefs.reopen_last_project,
        prefs.check_updates,
        prefs.shortcuts,
    );
}

pub fn apply_prefs(
    theme: &str,
    language: &str,
    show_menu_shortcuts: bool,
    reopen_last_project: bool,
    check_updates: bool,
    shortcuts: HashMap<String, Vec<String>>,
) {
    let mut guard = state();
    guard.theme_id = parse_theme_id(theme);
    guard.language = parse_language(language);
    guard.show_menu_shortcuts = show_menu_shortcuts;
    guard.reopen_last_project = reopen_last_project;
    guard.check_updates = check_updates;
    guard.shortcuts = shortcuts;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_theme_falls_back_to_default() {
        assert_eq!(parse_theme_id("nope"), "default");
        assert_eq!(preset("default").bg_app, 0xf8fafc);
        assert_eq!(preset("dracula").accent, 0xbd93f9);
    }

    #[test]
    fn missing_translation_falls_back_to_chinese() {
        set_language(Language::En);
        assert_eq!(t("settings.title").as_ref(), "Settings");
        assert_eq!(t("no.such.id").as_ref(), "no.such.id");
        set_language(Language::Zh);
        assert_eq!(t_src("没有英文的句子").as_ref(), "没有英文的句子");
        set_language(Language::En);
        assert_eq!(t_src("分块").as_ref(), "Blocks");
        set_language(Language::Zh);
    }
}
