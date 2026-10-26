//! 默认键位与用户覆盖. 安装时先清空, 再按原来的先后次序绑上, 后写的优先.

use std::collections::HashMap;

use gpui::{Action, KeyBinding, Keystroke};
use ui_kit::t;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    Global,
    Crop,
    Spread,
    Organize,
    Mask,
    Photo,
    Vector,
    Video,
}

pub(crate) struct Command {
    pub id: &'static str,
    pub group: Group,
    pub defaults: &'static [&'static str],
}

const HOST_FILE: &[Option<&str>] = &[
    None,
    Some("ScoreSync"),
    Some("ScoreVideo"),
    Some("ScoreProject"),
    Some("MaskTool"),
    Some("PhotoEdit"),
    Some("VectorEdit"),
];

const UNDO_CTX: &[Option<&str>] = &[
    Some("ScoreSync"),
    Some("ScoreProject"),
    Some("PageOrganize"),
    Some("MaskTool"),
    Some("PhotoEdit"),
    Some("VectorEdit"),
    Some("ScoreVideo"),
];

const HELP_CTX: &[Option<&str>] = &[None, Some("ScoreSync")];

const CROP: &[Option<&str>] = &[Some("ScoreSync")];
const ORGANIZE: &[Option<&str>] = &[Some("PageOrganize")];
const ORGANIZE_TOGGLE: &[Option<&str>] = &[Some("ScoreSync"), Some("PageOrganize")];
const MASK: &[Option<&str>] = &[Some("MaskTool")];
const PHOTO: &[Option<&str>] = &[Some("PhotoEdit")];
const VECTOR: &[Option<&str>] = &[Some("VectorEdit")];
const VIDEO: &[Option<&str>] = &[Some("ScoreVideo")];
const NONE_ONLY: &[Option<&str>] = &[None];

const TEXT_KEYS: &[&str] = &[
    "backspace",
    "delete",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "primary-a",
    "primary-c",
    "primary-v",
    "primary-x",
];

pub(crate) const COMMANDS: &[Command] = &[
    cmd("app.new", Group::Global, &["primary-shift-n"]),
    cmd("app.open_project", Group::Global, &["primary-shift-o"]),
    cmd("app.save", Group::Global, &["primary-s"]),
    cmd("app.save_as", Group::Global, &["primary-shift-s"]),
    cmd("app.undo", Group::Global, &["primary-z"]),
    cmd("app.redo", Group::Global, &["primary-y", "primary-shift-z"]),
    cmd("app.help", Group::Global, &["h", "f1"]),
    cmd("app.settings", Group::Global, &["primary-,"]),
    cmd("app.dump", Group::Global, &["primary-shift-m"]),
    cmd("app.confirm", Group::Global, &["enter"]),
    cmd("app.cancel", Group::Global, &["escape"]),
    cmd("crop.open_file", Group::Crop, &["primary-o"]),
    cmd("crop.detect", Group::Crop, &["d"]),
    cmd("crop.detect_all", Group::Crop, &["a"]),
    cmd("crop.add", Group::Crop, &["n"]),
    cmd("crop.split", Group::Crop, &["s"]),
    cmd("crop.merge", Group::Crop, &["m"]),
    cmd("crop.pair", Group::Crop, &["primary-m"]),
    cmd("crop.ungroup", Group::Crop, &["u"]),
    cmd("crop.share", Group::Crop, &["g"]),
    cmd("crop.export", Group::Crop, &["e"]),
    cmd("crop.reset", Group::Crop, &["r"]),
    cmd("crop.fit", Group::Crop, &["f"]),
    cmd("crop.delete", Group::Crop, &["delete", "backspace"]),
    cmd("crop.select_all", Group::Crop, &["primary-a"]),
    cmd("crop.prev", Group::Crop, &["primary-left", "primary-up"]),
    cmd("crop.next", Group::Crop, &["primary-right", "primary-down"]),
    cmd("spread.copy", Group::Spread, &["primary-c"]),
    cmd("spread.paste", Group::Spread, &["primary-v"]),
    cmd("organize.toggle", Group::Organize, &["p"]),
    cmd("organize.delete", Group::Organize, &["delete", "backspace"]),
    cmd("organize.select_all", Group::Organize, &["primary-a"]),
    cmd("organize.copy", Group::Organize, &["primary-c"]),
    cmd("organize.paste", Group::Organize, &["primary-v"]),
    cmd("mask.export", Group::Mask, &["e"]),
    cmd("mask.fit", Group::Mask, &["f"]),
    cmd("mask.delete", Group::Mask, &["delete", "backspace"]),
    cmd("mask.box", Group::Mask, &["b"]),
    cmd("mask.lasso", Group::Mask, &["l"]),
    cmd("mask.pan", Group::Mask, &["p"]),
    cmd("mask.escape", Group::Mask, &["escape"]),
    cmd("mask.photo", Group::Mask, &["t"]),
    cmd("mask.select_all", Group::Mask, &["primary-a"]),
    cmd("mask.prev", Group::Mask, &["primary-left", "primary-up"]),
    cmd("mask.next", Group::Mask, &["primary-right", "primary-down"]),
    cmd("photo.select", Group::Photo, &["v"]),
    cmd("photo.move", Group::Photo, &["m"]),
    cmd("photo.canvas", Group::Photo, &["k"]),
    cmd("photo.heal", Group::Photo, &["h"]),
    cmd("photo.clone", Group::Photo, &["s"]),
    cmd("photo.paint", Group::Photo, &["b"]),
    cmd("photo.eraser", Group::Photo, &["e"]),
    cmd("photo.wand", Group::Photo, &["w"]),
    cmd("photo.lasso", Group::Photo, &["l"]),
    cmd("photo.pan", Group::Photo, &["p"]),
    cmd("photo.fit", Group::Photo, &["f"]),
    cmd("photo.extract", Group::Photo, &["j"]),
    cmd("photo.cancel", Group::Photo, &["escape"]),
    cmd("photo.delete", Group::Photo, &["delete", "backspace"]),
    cmd("photo.nudge_left", Group::Photo, &["left"]),
    cmd("photo.nudge_right", Group::Photo, &["right"]),
    cmd("photo.nudge_up", Group::Photo, &["up"]),
    cmd("photo.nudge_down", Group::Photo, &["down"]),
    cmd("photo.open", Group::Photo, &["primary-o"]),
    cmd("photo.export", Group::Photo, &["primary-e"]),
    cmd("photo.copy", Group::Photo, &["primary-c"]),
    cmd("photo.paste", Group::Photo, &["primary-v"]),
    cmd("vector.select", Group::Vector, &["v"]),
    cmd("vector.node", Group::Vector, &["a"]),
    cmd("vector.canvas", Group::Vector, &["k"]),
    cmd("vector.hand", Group::Vector, &["h"]),
    cmd("vector.pen", Group::Vector, &["p"]),
    cmd("vector.pencil", Group::Vector, &["b"]),
    cmd("vector.eraser", Group::Vector, &["e"]),
    cmd("vector.lasso", Group::Vector, &["l"]),
    cmd("vector.fit", Group::Vector, &["f"]),
    cmd("vector.extract", Group::Vector, &["j"]),
    cmd("vector.cancel", Group::Vector, &["escape"]),
    cmd("vector.delete", Group::Vector, &["delete", "backspace"]),
    cmd("vector.nudge_left", Group::Vector, &["left"]),
    cmd("vector.nudge_right", Group::Vector, &["right"]),
    cmd("vector.nudge_up", Group::Vector, &["up"]),
    cmd("vector.nudge_down", Group::Vector, &["down"]),
    cmd("vector.open", Group::Vector, &["primary-o"]),
    cmd("vector.export", Group::Vector, &["primary-e"]),
    cmd("vector.export_svg", Group::Vector, &["primary-shift-e"]),
    cmd("vector.group", Group::Vector, &["primary-g"]),
    cmd("vector.ungroup", Group::Vector, &["primary-shift-g"]),
    cmd("vector.regroup", Group::Vector, &["primary-alt-g"]),
    cmd("vector.rename", Group::Vector, &["f2"]),
    cmd("vector.select_all", Group::Vector, &["primary-a"]),
    cmd("vector.copy", Group::Vector, &["primary-c"]),
    cmd("vector.paste", Group::Vector, &["primary-v"]),
    cmd("video.play", Group::Video, &["space"]),
    cmd("video.seek_back", Group::Video, &["left"]),
    cmd("video.seek_forward", Group::Video, &["right"]),
    cmd("video.seek_back_big", Group::Video, &["shift-left"]),
    cmd("video.seek_forward_big", Group::Video, &["shift-right"]),
    cmd("video.insert", Group::Video, &["n"]),
    cmd("video.wipe", Group::Video, &["w"]),
    cmd("video.fade_in", Group::Video, &["i"]),
    cmd("video.fade_out", Group::Video, &["o"]),
    cmd("video.delete", Group::Video, &["delete", "backspace"]),
];

/// 绑定次序: 视频, 修图, 再宿主. 同一深度时后绑定的优先, 与改设置之前一致.
const INSTALL_ORDER: &[&str] = &[
    "video.play",
    "video.seek_back",
    "video.seek_forward",
    "video.seek_back_big",
    "video.seek_forward_big",
    "video.insert",
    "video.wipe",
    "video.fade_in",
    "video.fade_out",
    "video.delete",
    "photo.select",
    "photo.move",
    "photo.canvas",
    "photo.heal",
    "photo.clone",
    "photo.paint",
    "photo.eraser",
    "photo.wand",
    "photo.lasso",
    "photo.pan",
    "photo.fit",
    "photo.extract",
    "photo.cancel",
    "photo.delete",
    "photo.nudge_left",
    "photo.nudge_right",
    "photo.nudge_up",
    "photo.nudge_down",
    "photo.open",
    "photo.export",
    "photo.copy",
    "photo.paste",
    "vector.select",
    "vector.node",
    "vector.canvas",
    "vector.hand",
    "vector.pen",
    "vector.pencil",
    "vector.eraser",
    "vector.lasso",
    "vector.fit",
    "vector.extract",
    "vector.cancel",
    "vector.delete",
    "vector.nudge_left",
    "vector.nudge_right",
    "vector.nudge_up",
    "vector.nudge_down",
    "vector.open",
    "vector.export",
    "vector.export_svg",
    "vector.group",
    "vector.ungroup",
    "vector.regroup",
    "vector.rename",
    "vector.select_all",
    "vector.copy",
    "vector.paste",
    "crop.open_file",
    "crop.detect",
    "crop.detect_all",
    "crop.add",
    "crop.split",
    "crop.merge",
    "crop.ungroup",
    "crop.share",
    "crop.export",
    "crop.reset",
    "crop.fit",
    "organize.toggle",
    "app.help",
    "crop.delete",
    "organize.delete",
    "app.confirm",
    "app.cancel",
    "mask.export",
    "mask.fit",
    "mask.delete",
    "mask.box",
    "mask.lasso",
    "mask.pan",
    "mask.escape",
    "mask.photo",
    "app.new",
    "app.open_project",
    "app.save",
    "app.save_as",
    "crop.pair",
    "app.undo",
    "app.redo",
    "crop.select_all",
    "organize.select_all",
    "organize.copy",
    "organize.paste",
    "mask.select_all",
    "crop.prev",
    "crop.next",
    "mask.prev",
    "mask.next",
    "spread.copy",
    "spread.paste",
    "app.dump",
    "app.settings",
];

const fn cmd(id: &'static str, group: Group, defaults: &'static [&'static str]) -> Command {
    Command {
        id,
        group,
        defaults,
    }
}

pub(crate) fn group_label(group: Group) -> &'static str {
    match group {
        Group::Global => "group.global",
        Group::Crop => "group.crop",
        Group::Spread => "group.spread",
        Group::Organize => "group.organize",
        Group::Mask => "group.mask",
        Group::Photo => "group.photo",
        Group::Vector => "group.vector",
        Group::Video => "group.video",
    }
}

pub(crate) fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|cmd| cmd.id == id)
}

pub(crate) fn install(cx: &mut gpui::App) {
    cx.clear_key_bindings();
    apply_bg::text_input::bind_keys(cx);
    ui_kit::clear_shortcut_hints();
    let mut keys = Vec::new();
    for id in INSTALL_ORDER {
        let effective = effective_keys(id);
        bind_id(id, &effective, &mut keys);
        ui_kit::set_shortcut_hint(id, &display_keys(&effective));
    }
    cx.bind_keys(keys);
}

pub(crate) fn effective_keys(id: &str) -> Vec<String> {
    let Some(cmd) = command(id) else {
        return Vec::new();
    };
    effective_in(&ui_kit::shortcut_overrides(), cmd)
}

fn effective_in(overrides: &HashMap<String, Vec<String>>, cmd: &Command) -> Vec<String> {
    let Some(over) = overrides.get(cmd.id) else {
        return cmd.defaults.iter().map(|key| (*key).to_string()).collect();
    };
    cmd.defaults
        .iter()
        .enumerate()
        .map(|(index, default)| match over.get(index) {
            Some(key) if key.is_empty() => String::new(),
            Some(key) if valid_logical(key) => key.clone(),
            _ => (*default).to_string(),
        })
        .collect()
}

pub(crate) fn valid_logical(key: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    if key.chars().any(char::is_whitespace) {
        return false;
    }
    let expanded = if let Some(rest) = key.strip_prefix("primary-") {
        format!("ctrl-{rest}")
    } else {
        key.to_string()
    };
    match Keystroke::parse(&expanded) {
        Ok(stroke) => known_key(&stroke.key),
        Err(_) => false,
    }
}

fn known_key(key: &str) -> bool {
    if key.chars().count() == 1 {
        return !key.chars().next().is_some_and(char::is_whitespace);
    }
    matches!(
        key,
        "escape"
            | "enter"
            | "tab"
            | "space"
            | "backspace"
            | "delete"
            | "left"
            | "right"
            | "up"
            | "down"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "insert"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    )
}

pub(crate) enum Conflict {
    Command(&'static str),
    TextInput,
}

pub(crate) fn find_conflict(id: &str, new_key: &str) -> Option<Conflict> {
    find_conflict_in(&ui_kit::shortcut_overrides(), id, new_key)
}

fn find_conflict_in(
    overrides: &HashMap<String, Vec<String>>,
    id: &str,
    new_key: &str,
) -> Option<Conflict> {
    if new_key.is_empty() {
        return None;
    }
    let norm = normalize(new_key);
    if contexts(id).contains(&None) && TEXT_KEYS.iter().any(|key| normalize(key) == norm) {
        return Some(Conflict::TextInput);
    }
    let mine = contexts(id);
    for cmd in COMMANDS {
        if cmd.id == id {
            continue;
        }
        if !contexts_overlap(mine, contexts(cmd.id)) {
            continue;
        }
        let keys = effective_in(overrides, cmd);
        if keys
            .iter()
            .any(|key| !key.is_empty() && normalize(key) == norm)
        {
            return Some(Conflict::Command(cmd.id));
        }
    }
    None
}

pub(crate) fn set_slot(id: &str, slot: usize, key: String) {
    let Some(cmd) = command(id) else {
        return;
    };
    let mut keys = effective_keys(id);
    if slot >= keys.len() {
        return;
    }
    keys[slot] = key;
    let mut map = ui_kit::shortcut_overrides();
    let same = keys
        .iter()
        .zip(cmd.defaults.iter())
        .all(|(key, default)| key == default);
    if same {
        map.remove(id);
    } else {
        map.insert(id.to_string(), keys);
    }
    ui_kit::set_shortcut_overrides(map);
}

pub(crate) fn clear_overrides() {
    ui_kit::set_shortcut_overrides(HashMap::new());
}

pub(crate) fn display_keys(keys: &[String]) -> String {
    keys.iter()
        .filter(|key| !key.is_empty())
        .map(|key| display_key(key))
        .collect::<Vec<_>>()
        .join(" / ")
}

pub(crate) fn display_key(logical: &str) -> String {
    if logical.is_empty() {
        return String::new();
    }
    let (mods, key) = split_mods(logical);
    let body = display_body(key);
    match mods {
        "" => body,
        "primary" => format!("{}{body}", apply_bg::primary_mod()),
        "primary-shift" => format!("{}{body}", apply_bg::primary_shift()),
        "primary-alt" => format!("{}{body}", apply_bg::primary_alt()),
        "shift" => format!("Shift+{body}"),
        "alt" => format!("{}{body}", apply_bg::alt_mod()),
        "ctrl" => format!("Ctrl+{body}"),
        other => format!("{other}+{body}"),
    }
}

pub(crate) fn logical_from(ks: &Keystroke) -> Option<String> {
    let key = ks.key.to_ascii_lowercase();
    if key.is_empty()
        || matches!(
            key.as_str(),
            "control"
                | "ctrl"
                | "alt"
                | "shift"
                | "cmd"
                | "command"
                | "super"
                | "win"
                | "fn"
                | "meta"
                | "escape"
        ) && !ks.modifiers.modified()
            && key != "escape"
    {
        return None;
    }
    if matches!(
        key.as_str(),
        "control" | "ctrl" | "alt" | "shift" | "cmd" | "command" | "super" | "win" | "fn" | "meta"
    ) {
        return None;
    }
    #[cfg(not(target_os = "macos"))]
    if ks.modifiers.platform {
        return None;
    }
    let mut parts = Vec::new();
    if ks.modifiers.secondary() {
        parts.push("primary");
    } else if ks.modifiers.control {
        parts.push("ctrl");
    }
    if ks.modifiers.alt {
        parts.push("alt");
    }
    if ks.modifiers.shift {
        parts.push("shift");
    }
    if ks.modifiers.function {
        parts.push("fn");
    }
    parts.push(key.as_str());
    let logical = parts.join("-");
    valid_logical(&logical).then_some(logical)
}

fn split_mods(logical: &str) -> (&str, &str) {
    for prefix in [
        "primary-alt-",
        "primary-shift-",
        "primary-",
        "shift-",
        "alt-",
        "ctrl-",
        "fn-",
    ] {
        if let Some(rest) = logical.strip_prefix(prefix) {
            let mods = prefix.trim_end_matches('-');
            return (mods, rest);
        }
    }
    ("", logical)
}

fn display_body(key: &str) -> String {
    match key {
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "space" => t("key.space").to_string(),
        "delete" => t("key.delete").to_string(),
        "backspace" => t("key.backspace").to_string(),
        "escape" => t("key.escape").to_string(),
        "enter" => t("key.enter").to_string(),
        "," => ",".into(),
        other if other.chars().count() == 1 => other.to_ascii_uppercase(),
        other => other.to_ascii_uppercase(),
    }
}

fn normalize(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

/// 快捷键页的搜索. 名称, 命令 id, 分组名, 以及键位都能对上.
/// 键位忽略大小写和空格, `+` / `-` 等价; Ctrl / Cmd / ⌘ 都认主修饰键.
pub(crate) fn command_matches(
    label: &str,
    id: &str,
    group: &str,
    keys: &[String],
    query: &str,
) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    if text_hit(label, query) || text_hit(id, query) || text_hit(group, query) || keys_hit(keys, query)
    {
        return true;
    }
    let words: Vec<&str> = query.split_whitespace().filter(|word| !word.is_empty()).collect();
    words.len() > 1
        && words.iter().all(|word| {
            text_hit(label, word) || text_hit(id, word) || text_hit(group, word) || keys_hit(keys, word)
        })
}

fn text_hit(hay: &str, query: &str) -> bool {
    let query = query.trim();
    !query.is_empty() && hay.to_lowercase().contains(&query.to_lowercase())
}

fn keys_hit(keys: &[String], query: &str) -> bool {
    let query = fold_key_query(query);
    if query.is_empty() {
        return false;
    }
    keys.iter().any(|key| {
        !key.is_empty()
            && key_haystacks(key)
                .iter()
                .any(|hay| chord_contains(hay, &query))
    })
}

/// 查询串收成 `ctrl+shift+s` 这种片段. 空格去掉, 符号换成单词.
fn fold_key_query(raw: &str) -> String {
    let mut text = raw.trim().to_lowercase();
    text = text.replace('＋', "+").replace('－', "+").replace('-', "+");
    text = text
        .replace('⌘', "cmd+")
        .replace('⇧', "shift+")
        .replace('⌥', "alt+")
        .replace('←', "left")
        .replace('→', "right")
        .replace('↑', "up")
        .replace('↓', "down");
    text.retain(|ch| !ch.is_whitespace());
    while text.contains("++") {
        text = text.replace("++", "+");
    }
    text.split('+')
        .filter(|part| !part.is_empty())
        .map(canon_token)
        .collect::<Vec<_>>()
        .join("+")
}

fn canon_token(token: &str) -> String {
    match token {
        "control" => "ctrl".into(),
        "command" | "meta" | "win" | "super" => "cmd".into(),
        "option" | "opt" => "alt".into(),
        "primary" => {
            if cfg!(target_os = "macos") {
                "cmd"
            } else {
                "ctrl"
            }
        }
        .into(),
        "escape" => "esc".into(),
        "delete" => "del".into(),
        "return" => "enter".into(),
        other => other.to_string(),
    }
}

fn key_haystacks(logical: &str) -> Vec<String> {
    let (mods, body) = split_mods(logical);
    let bodies = body_canons(body);
    let mut out = Vec::new();
    for mods in modifier_lists(mods) {
        for body in &bodies {
            if mods.is_empty() {
                out.push(body.clone());
            } else {
                let mut chord = mods.join("+");
                chord.push('+');
                chord.push_str(body);
                out.push(chord);
            }
        }
    }
    out
}

fn body_canons(body: &str) -> Vec<String> {
    let lower = body.to_ascii_lowercase();
    match lower.as_str() {
        "escape" => vec!["esc".into()],
        "delete" => vec!["del".into()],
        "return" => vec!["enter".into()],
        "space" => vec!["space".into(), "空格".into()],
        _ => vec![lower],
    }
}

/// 主修饰键同时给出 ctrl 和 cmd, 因为绑定本身两套都生效.
fn modifier_lists(mods: &str) -> Vec<Vec<&'static str>> {
    match mods {
        "" => vec![vec![]],
        "shift" => vec![vec!["shift"]],
        "alt" => vec![vec!["alt"]],
        "ctrl" => vec![vec!["ctrl"]],
        "fn" => vec![vec!["fn"]],
        "primary" => vec![vec!["ctrl"], vec!["cmd"]],
        "primary-shift" => vec![vec!["ctrl", "shift"], vec!["cmd", "shift"]],
        "primary-alt" => vec![vec!["ctrl", "alt"], vec!["cmd", "alt"]],
        _ => Vec::new(),
    }
}

/// `query` 按 `+` 对齐到 `hay` 的一段. 单个字母必须整段相等, 避免 `s` 命中 `shift`.
fn chord_contains(hay: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    let hay: Vec<&str> = hay.split('+').filter(|part| !part.is_empty()).collect();
    let query: Vec<&str> = query.split('+').filter(|part| !part.is_empty()).collect();
    if query.is_empty() || query.len() > hay.len() {
        return false;
    }
    hay.windows(query.len()).any(|window| {
        window.iter().zip(query.iter()).enumerate().all(|(index, (left, right))| {
            token_eq(left, right, index + 1 == query.len())
        })
    })
}

fn token_eq(hay: &str, query: &str, prefix_ok: bool) -> bool {
    if hay == query {
        return true;
    }
    if !prefix_ok || query.chars().count() <= 1 {
        return false;
    }
    let function_key = |token: &str| {
        token.starts_with('f') && token[1..].chars().all(|ch| ch.is_ascii_digit()) && token.len() > 1
    };
    if function_key(hay) && function_key(query) {
        return false;
    }
    hay.starts_with(query)
}

fn contexts(id: &str) -> &'static [Option<&'static str>] {
    match id {
        "app.new" | "app.open_project" | "app.save" | "app.save_as" => HOST_FILE,
        "app.undo" | "app.redo" => UNDO_CTX,
        "app.help" | "app.confirm" | "app.cancel" => HELP_CTX,
        "app.settings" | "app.dump" => NONE_ONLY,
        "organize.toggle" => ORGANIZE_TOGGLE,
        "organize.delete" | "organize.select_all" | "organize.copy" | "organize.paste" => ORGANIZE,
        id if id.starts_with("crop.") || id.starts_with("spread.") => CROP,
        id if id.starts_with("mask.") => MASK,
        id if id.starts_with("photo.") => PHOTO,
        id if id.starts_with("vector.") => VECTOR,
        id if id.starts_with("video.") => VIDEO,
        _ => NONE_ONLY,
    }
}

fn contexts_overlap(a: &[Option<&str>], b: &[Option<&str>]) -> bool {
    a.iter().any(|left| b.iter().any(|right| left == right))
}

fn bind_id(id: &str, keys: &[String], out: &mut Vec<KeyBinding>) {
    match id {
        "app.new" => bind_same(out, id, keys, NewProject),
        "app.open_project" => bind_same(out, id, keys, OpenProject),
        "app.save" => bind_same(out, id, keys, SaveProject),
        "app.save_as" => bind_same(out, id, keys, SaveProjectAs),
        "app.help" => bind_same(out, id, keys, ShowHelp),
        "app.settings" => bind_same(out, id, keys, OpenSettings),
        "app.dump" => bind_same(out, id, keys, DumpMemory),
        "app.confirm" => bind_same(out, id, keys, ConfirmParamEdit),
        "app.cancel" => bind_same(out, id, keys, CancelParamEdit),
        "app.undo" => {
            for key in keys {
                push(out, key, Undo, Some("ScoreSync"));
                push(out, key, Undo, Some("ScoreProject"));
                push(out, key, Undo, Some("PageOrganize"));
                push(out, key, mask_tool::gui::Undo, Some("MaskTool"));
                push(out, key, photo_edit::gui::Undo, Some("PhotoEdit"));
                push(out, key, vector_edit::gui::Undo, Some("VectorEdit"));
                push(out, key, score_video::gui::Undo, Some("ScoreVideo"));
            }
        }
        "app.redo" => {
            for key in keys {
                push(out, key, Redo, Some("ScoreSync"));
                push(out, key, Redo, Some("ScoreProject"));
                push(out, key, Redo, Some("PageOrganize"));
                push(out, key, mask_tool::gui::Redo, Some("MaskTool"));
                push(out, key, photo_edit::gui::Redo, Some("PhotoEdit"));
                push(out, key, vector_edit::gui::Redo, Some("VectorEdit"));
                push(out, key, score_video::gui::Redo, Some("ScoreVideo"));
            }
        }
        "crop.open_file" => bind_same(out, id, keys, OpenFile),
        "crop.detect" => bind_same(out, id, keys, DetectPage),
        "crop.detect_all" => bind_same(out, id, keys, DetectAll),
        "crop.add" => bind_same(out, id, keys, ToggleAddBlock),
        "crop.split" => bind_same(out, id, keys, ToggleSplitBlock),
        "crop.merge" => bind_same(out, id, keys, MergeSelected),
        "crop.pair" => bind_same(out, id, keys, PairUngrouped),
        "crop.ungroup" => bind_same(out, id, keys, UngroupActive),
        "crop.share" => bind_same(out, id, keys, ShareIntoGroup),
        "crop.export" => bind_same(out, id, keys, ExportGroups),
        "crop.reset" => bind_same(out, id, keys, ResetGroups),
        "crop.fit" => bind_same(out, id, keys, FitView),
        "crop.delete" | "organize.delete" => bind_same(out, id, keys, DeleteSelected),
        "crop.select_all" | "organize.select_all" => bind_same(out, id, keys, SelectAllPageRegions),
        "crop.prev" | "mask.prev" => bind_same(out, id, keys, PrevPage),
        "crop.next" | "mask.next" => bind_same(out, id, keys, NextPage),
        "spread.copy" => bind_same(out, id, keys, CopySpreadPages),
        "spread.paste" => bind_same(out, id, keys, PasteSpreadPages),
        "organize.copy" => bind_same(out, id, keys, CopyOrganizePages),
        "organize.paste" => bind_same(out, id, keys, PasteOrganizePages),
        "organize.toggle" => bind_same(out, id, keys, OrganizePages),
        "mask.export" => bind_same(out, id, keys, mask_tool::gui::ExportImage),
        "mask.fit" => bind_same(out, id, keys, mask_tool::gui::FitView),
        "mask.delete" => bind_same(out, id, keys, mask_tool::gui::DeleteSelected),
        "mask.box" => bind_same(out, id, keys, mask_tool::gui::ToggleDrawMode),
        "mask.lasso" => bind_same(out, id, keys, mask_tool::gui::TogglePolyMode),
        "mask.pan" => bind_same(out, id, keys, mask_tool::gui::TogglePanMode),
        "mask.escape" => bind_same(out, id, keys, mask_tool::gui::CancelPolyDraft),
        "mask.photo" => bind_same(out, id, keys, OpenPhotoEdit),
        "mask.select_all" => bind_same(out, id, keys, mask_tool::gui::SelectAll),
        "photo.select" => bind_same(out, id, keys, photo_edit::gui::ToolSelect),
        "photo.move" => bind_same(out, id, keys, photo_edit::gui::ToolMove),
        "photo.canvas" => bind_same(out, id, keys, photo_edit::gui::ToolCanvas),
        "photo.heal" => bind_same(out, id, keys, photo_edit::gui::ToolHeal),
        "photo.clone" => bind_same(out, id, keys, photo_edit::gui::ToolClone),
        "photo.paint" => bind_same(out, id, keys, photo_edit::gui::ToolPaint),
        "photo.eraser" => bind_same(out, id, keys, photo_edit::gui::ToolEraser),
        "photo.wand" => bind_same(out, id, keys, photo_edit::gui::ToolWand),
        "photo.lasso" => bind_same(out, id, keys, photo_edit::gui::ToolLasso),
        "photo.pan" => bind_same(out, id, keys, photo_edit::gui::ToolPan),
        "photo.fit" => bind_same(out, id, keys, photo_edit::gui::FitView),
        "photo.extract" => bind_same(out, id, keys, photo_edit::gui::NewLayerFromSel),
        "photo.cancel" => bind_same(out, id, keys, photo_edit::gui::Cancel),
        "photo.delete" => bind_same(out, id, keys, photo_edit::gui::DeleteLayer),
        "photo.nudge_left" => bind_same(out, id, keys, photo_edit::gui::NudgeLeft),
        "photo.nudge_right" => bind_same(out, id, keys, photo_edit::gui::NudgeRight),
        "photo.nudge_up" => bind_same(out, id, keys, photo_edit::gui::NudgeUp),
        "photo.nudge_down" => bind_same(out, id, keys, photo_edit::gui::NudgeDown),
        "photo.open" => bind_same(out, id, keys, photo_edit::gui::OpenFile),
        "photo.export" => bind_same(out, id, keys, photo_edit::gui::ExportImage),
        "photo.copy" => bind_same(out, id, keys, photo_edit::gui::CopyLayer),
        "photo.paste" => bind_same(out, id, keys, photo_edit::gui::PasteLayer),
        "vector.select" => bind_same(out, id, keys, vector_edit::gui::ToolSelect),
        "vector.node" => bind_same(out, id, keys, vector_edit::gui::ToolNode),
        "vector.canvas" => bind_same(out, id, keys, vector_edit::gui::ToolCanvas),
        "vector.hand" => bind_same(out, id, keys, vector_edit::gui::ToolHand),
        "vector.pen" => bind_same(out, id, keys, vector_edit::gui::ToolPen),
        "vector.pencil" => bind_same(out, id, keys, vector_edit::gui::ToolPencil),
        "vector.eraser" => bind_same(out, id, keys, vector_edit::gui::ToolEraser),
        "vector.lasso" => bind_same(out, id, keys, vector_edit::gui::ToolLasso),
        "vector.fit" => bind_same(out, id, keys, vector_edit::gui::FitView),
        "vector.extract" => bind_same(out, id, keys, vector_edit::gui::NewLayerFromSel),
        "vector.cancel" => bind_same(out, id, keys, vector_edit::gui::Cancel),
        "vector.delete" => bind_same(out, id, keys, vector_edit::gui::DeleteSelection),
        "vector.nudge_left" => bind_same(out, id, keys, vector_edit::gui::NudgeLeft),
        "vector.nudge_right" => bind_same(out, id, keys, vector_edit::gui::NudgeRight),
        "vector.nudge_up" => bind_same(out, id, keys, vector_edit::gui::NudgeUp),
        "vector.nudge_down" => bind_same(out, id, keys, vector_edit::gui::NudgeDown),
        "vector.open" => bind_same(out, id, keys, vector_edit::gui::OpenFile),
        "vector.export" => bind_same(out, id, keys, vector_edit::gui::ExportImage),
        "vector.export_svg" => bind_same(out, id, keys, vector_edit::gui::ExportSvg),
        "vector.group" => bind_same(out, id, keys, vector_edit::gui::GroupSelection),
        "vector.ungroup" => bind_same(out, id, keys, vector_edit::gui::UngroupSelection),
        "vector.regroup" => bind_same(out, id, keys, vector_edit::gui::RegroupSelection),
        "vector.rename" => bind_same(out, id, keys, vector_edit::gui::RenameNode),
        "vector.select_all" => bind_same(out, id, keys, vector_edit::gui::SelectAll),
        "vector.copy" => bind_same(out, id, keys, vector_edit::gui::CopyLayer),
        "vector.paste" => bind_same(out, id, keys, vector_edit::gui::PasteLayer),
        "video.play" => bind_same(out, id, keys, score_video::gui::PlayPause),
        "video.seek_back" => bind_same(out, id, keys, score_video::gui::SeekBack),
        "video.seek_forward" => bind_same(out, id, keys, score_video::gui::SeekForward),
        "video.seek_back_big" => bind_same(out, id, keys, score_video::gui::SeekBackBig),
        "video.seek_forward_big" => bind_same(out, id, keys, score_video::gui::SeekForwardBig),
        "video.insert" => bind_same(out, id, keys, score_video::gui::InsertNext),
        "video.wipe" => bind_same(out, id, keys, score_video::gui::InsertNextWipe),
        "video.fade_in" => bind_same(out, id, keys, score_video::gui::MarkFadeIn),
        "video.fade_out" => bind_same(out, id, keys, score_video::gui::MarkFadeOut),
        "video.delete" => bind_same(out, id, keys, score_video::gui::DeleteSelected),
        _ => {}
    }
}

fn bind_same<A: Action + Clone>(out: &mut Vec<KeyBinding>, id: &str, keys: &[String], action: A) {
    for ctx in contexts(id) {
        for key in keys {
            push(out, key, action.clone(), *ctx);
        }
    }
}

fn push<A: Action + Clone>(out: &mut Vec<KeyBinding>, logical: &str, action: A, ctx: Option<&str>) {
    if logical.is_empty() || !valid_logical(logical) {
        return;
    }
    if let Some(rest) = logical.strip_prefix("primary-") {
        out.extend(apply_bg::bind_primary(rest, action, ctx));
    } else {
        out.push(KeyBinding::new(logical, action, ctx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_order_covers_every_command_once() {
        assert_eq!(INSTALL_ORDER.len(), COMMANDS.len());
        for cmd in COMMANDS {
            assert_eq!(
                INSTALL_ORDER.iter().filter(|id| **id == cmd.id).count(),
                1,
                "{}",
                cmd.id
            );
        }
    }

    #[test]
    fn same_letter_in_different_panels_is_allowed() {
        let map = HashMap::new();
        assert!(find_conflict_in(&map, "photo.heal", "h").is_none());
        assert!(matches!(
            find_conflict_in(&map, "crop.detect", "d"),
            Some(Conflict::Command("crop.detect")) | None
        ));
        assert!(matches!(
            find_conflict_in(&map, "crop.add", "d"),
            Some(Conflict::Command("crop.detect"))
        ));
    }

    #[test]
    fn global_command_cannot_take_text_keys() {
        let map = HashMap::new();
        assert!(matches!(
            find_conflict_in(&map, "app.save", "primary-a"),
            Some(Conflict::TextInput)
        ));
    }

    #[test]
    fn override_replaces_only_that_slot() {
        let mut map = HashMap::new();
        map.insert("crop.delete".into(), vec!["x".into(), String::new()]);
        let cmd = command("crop.delete").unwrap();
        let keys = effective_in(&map, cmd);
        assert_eq!(keys, vec!["x".to_string(), String::new()]);
    }

    #[test]
    fn invalid_override_falls_back() {
        let mut map = HashMap::new();
        map.insert("crop.detect".into(), vec!["not a key".into()]);
        let cmd = command("crop.detect").unwrap();
        assert_eq!(effective_in(&map, cmd), vec!["d".to_string()]);
    }

    #[test]
    fn search_matches_name_id_and_chord() {
        let save = ["primary-s".to_string()];
        assert!(command_matches("保存工程", "app.save", "全局", &save, ""));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "保存"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "save"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "全局"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "ctrl+s"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "Ctrl + S"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "cmd+s"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "primary-s"));
        assert!(command_matches("保存工程", "app.save", "全局", &save, "⌘s"));
        assert!(!command_matches("保存工程", "app.save", "全局", &save, "识别"));
        assert!(!command_matches("识别", "crop.detect", "分块", &["d".into()], "ctrl+s"));
    }

    #[test]
    fn search_chord_does_not_swallow_extra_modifiers() {
        let shifted = ["primary-shift-s".to_string()];
        assert!(command_matches("另存", "app.save_as", "全局", &shifted, "ctrl+shift+s"));
        assert!(command_matches("另存", "app.save_as", "全局", &shifted, "shift+s"));
        assert!(command_matches("另存", "app.save_as", "全局", &shifted, "ctrl shift"));
        assert!(!command_matches("另存", "app.save_as", "全局", &shifted, "ctrl+s"));
        assert!(!command_matches("向左", "crop.prev", "分块", &["left".into()], "s"));
        assert!(command_matches(
            "向左",
            "crop.prev",
            "分块",
            &["shift-left".into()],
            "shift+left"
        ));
        assert!(command_matches("向左", "crop.prev", "分块", &["shift-left".into()], "←"));
        assert!(!command_matches("向左", "crop.prev", "分块", &["shift-left".into()], "s"));
        assert!(command_matches("播放", "video.play", "视频", &["space".into()], "空格"));
        assert!(command_matches("播放", "video.play", "视频", &["space".into()], "space"));
        assert!(!command_matches("帮助", "app.help", "全局", &["f1".into()], "f12"));
        assert!(command_matches("保存工程", "app.save", "全局", &["primary-s".into()], "保存 工程"));
    }
}
