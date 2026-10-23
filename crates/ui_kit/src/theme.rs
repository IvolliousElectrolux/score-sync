//! 界面语义色. 谱纸, 墨色, 蒙版用户色不在这里.

use gpui::{rgb, rgba, Rgba};

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg_app: u32,
    pub bg_panel: u32,
    pub bg_toolbar: u32,
    pub bg_hover: u32,
    pub bg_card: u32,
    pub bg_selected: u32,
    pub bg_input: u32,
    pub border: u32,
    pub border_strong: u32,
    pub text: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub text_on_accent: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub danger: u32,
    pub warning: u32,
    pub overlay: u32,
}

pub const THEME_IDS: &[&str] = &[
    "default",
    "modern_light",
    "one_dark",
    "one_dark_pro",
    "dracula",
    "catppuccin_latte",
    "catppuccin_frappe",
    "catppuccin_macchiato",
    "catppuccin_mocha",
];

pub fn parse_theme_id(id: &str) -> &'static str {
    THEME_IDS
        .iter()
        .copied()
        .find(|item| *item == id)
        .unwrap_or("default")
}

pub fn preset(id: &str) -> Theme {
    match parse_theme_id(id) {
        "modern_light" => MODERN_LIGHT,
        "one_dark" => ONE_DARK,
        "one_dark_pro" => ONE_DARK_PRO,
        "dracula" => DRACULA,
        "catppuccin_latte" => LATTE,
        "catppuccin_frappe" => FRAPPE,
        "catppuccin_macchiato" => MACCHIATO,
        "catppuccin_mocha" => MOCHA,
        _ => DEFAULT,
    }
}

/// 把「默认」方案里的界面色换成当前方案. 不认识的色值原样返回 (谱纸, 墨色).
pub fn chrome(hex: u32) -> Rgba {
    let t = super::theme();
    let mapped = match hex & 0x00ff_ffff {
        0xf8fafc => t.bg_app,
        0xf1f5f9 => t.bg_panel,
        0xe2e8f0 => t.bg_toolbar,
        0xd8e0ea => t.bg_hover,
        0xeef2ff | 0xeff6ff | 0xdbeafe => t.bg_selected,
        0xcbd5e1 | 0xe5e7eb => t.border,
        0x94a3b8 => t.border_strong,
        0x0f172a | 0x1e293b => t.text,
        0x334155 | 0x475569 => t.text_secondary,
        0x64748b => t.text_muted,
        0x2563eb | 0x3b82f6 => t.accent,
        0x1d4ed8 | 0x1e40af => t.accent_hover,
        0xdc2626 | 0xb91c1c | 0xef4444 => t.danger,
        0xf59e0b | 0xd97706 => t.warning,
        _ => hex & 0x00ff_ffff,
    };
    rgb(mapped)
}

pub fn card() -> Rgba {
    rgb(super::theme().bg_card)
}

pub fn on_accent() -> Rgba {
    rgb(super::theme().text_on_accent)
}

pub fn overlay() -> Rgba {
    rgba(super::theme().overlay)
}

const DEFAULT: Theme = Theme {
    bg_app: 0xf8fafc,
    bg_panel: 0xf1f5f9,
    bg_toolbar: 0xe2e8f0,
    bg_hover: 0xd8e0ea,
    bg_card: 0xffffff,
    bg_selected: 0xeff6ff,
    bg_input: 0xffffff,
    border: 0xcbd5e1,
    border_strong: 0x94a3b8,
    text: 0x0f172a,
    text_secondary: 0x334155,
    text_muted: 0x64748b,
    text_on_accent: 0xffffff,
    accent: 0x2563eb,
    accent_hover: 0x1d4ed8,
    danger: 0xdc2626,
    warning: 0xf59e0b,
    overlay: 0x0000_0080,
};

/// VS Code Light Modern.
const MODERN_LIGHT: Theme = Theme {
    bg_app: 0xf3f3f3,
    bg_panel: 0xf8f8f8,
    bg_toolbar: 0xf3f3f3,
    bg_hover: 0xe8e8e8,
    bg_card: 0xffffff,
    bg_selected: 0xe8f3fc,
    bg_input: 0xffffff,
    border: 0xe5e5e5,
    border_strong: 0xcecece,
    text: 0x3b3b3b,
    text_secondary: 0x616161,
    text_muted: 0x717171,
    text_on_accent: 0xffffff,
    accent: 0x0078d4,
    accent_hover: 0x006cbe,
    danger: 0xc42b1c,
    warning: 0xbf8803,
    overlay: 0x0000_0080,
};

/// Atom One Dark.
const ONE_DARK: Theme = Theme {
    bg_app: 0x282c34,
    bg_panel: 0x21252b,
    bg_toolbar: 0x21252b,
    bg_hover: 0x2c313a,
    bg_card: 0x282c34,
    bg_selected: 0x3e4451,
    bg_input: 0x1d1f23,
    border: 0x181a1f,
    border_strong: 0x3e4451,
    text: 0xabb2bf,
    text_secondary: 0x9da5b4,
    text_muted: 0x5c6370,
    text_on_accent: 0xffffff,
    accent: 0x61afef,
    accent_hover: 0x528bff,
    danger: 0xe06c75,
    warning: 0xe5c07b,
    overlay: 0x0000_0099,
};

/// One Dark Pro: 更深的侧栏, 紫色强调.
const ONE_DARK_PRO: Theme = Theme {
    bg_app: 0x21252b,
    bg_panel: 0x21252b,
    bg_toolbar: 0x282c34,
    bg_hover: 0x2c313a,
    bg_card: 0x282c34,
    bg_selected: 0x2c313a,
    bg_input: 0x1b1d23,
    border: 0x181a1f,
    border_strong: 0x3e4451,
    text: 0xabb2bf,
    text_secondary: 0x9da5b4,
    text_muted: 0x5c6370,
    text_on_accent: 0xffffff,
    accent: 0xc678dd,
    accent_hover: 0xd19aee,
    danger: 0xe06c75,
    warning: 0xe5c07b,
    overlay: 0x0000_0099,
};

const DRACULA: Theme = Theme {
    bg_app: 0x282a36,
    bg_panel: 0x21222c,
    bg_toolbar: 0x21222c,
    bg_hover: 0x44475a,
    bg_card: 0x282a36,
    bg_selected: 0x44475a,
    bg_input: 0x21222c,
    border: 0x191a21,
    border_strong: 0x6272a4,
    text: 0xf8f8f2,
    text_secondary: 0xf8f8f2,
    text_muted: 0x6272a4,
    text_on_accent: 0xffffff,
    accent: 0xbd93f9,
    accent_hover: 0xff79c6,
    danger: 0xff5555,
    warning: 0xffb86c,
    overlay: 0x0000_0099,
};

const LATTE: Theme = Theme {
    bg_app: 0xeff1f5,
    bg_panel: 0xe6e9ef,
    bg_toolbar: 0xdce0e8,
    bg_hover: 0xccd0da,
    bg_card: 0xeff1f5,
    bg_selected: 0xccd0da,
    bg_input: 0xe6e9ef,
    border: 0xbcc0cc,
    border_strong: 0xacb0be,
    text: 0x4c4f69,
    text_secondary: 0x5c5f77,
    text_muted: 0x6c6f85,
    text_on_accent: 0xffffff,
    accent: 0x1e66f5,
    accent_hover: 0x7287fd,
    danger: 0xd20f39,
    warning: 0xdf8e1d,
    overlay: 0x0000_0080,
};

const FRAPPE: Theme = Theme {
    bg_app: 0x303446,
    bg_panel: 0x292c3c,
    bg_toolbar: 0x232634,
    bg_hover: 0x414559,
    bg_card: 0x303446,
    bg_selected: 0x414559,
    bg_input: 0x292c3c,
    border: 0x414559,
    border_strong: 0x51576d,
    text: 0xc6d0f5,
    text_secondary: 0xb5bfe2,
    text_muted: 0x838ba7,
    text_on_accent: 0xffffff,
    accent: 0x8caaee,
    accent_hover: 0xbabbf1,
    danger: 0xe78284,
    warning: 0xe5c890,
    overlay: 0x0000_0099,
};

const MACCHIATO: Theme = Theme {
    bg_app: 0x24273a,
    bg_panel: 0x1e2030,
    bg_toolbar: 0x181926,
    bg_hover: 0x363a4f,
    bg_card: 0x24273a,
    bg_selected: 0x363a4f,
    bg_input: 0x1e2030,
    border: 0x363a4f,
    border_strong: 0x494d64,
    text: 0xcad3f5,
    text_secondary: 0xb8c0e0,
    text_muted: 0x8087a2,
    text_on_accent: 0xffffff,
    accent: 0x8aadf4,
    accent_hover: 0xb7bdf8,
    danger: 0xed8796,
    warning: 0xeed49f,
    overlay: 0x0000_0099,
};

const MOCHA: Theme = Theme {
    bg_app: 0x1e1e2e,
    bg_panel: 0x181825,
    bg_toolbar: 0x11111b,
    bg_hover: 0x313244,
    bg_card: 0x1e1e2e,
    bg_selected: 0x313244,
    bg_input: 0x181825,
    border: 0x313244,
    border_strong: 0x45475a,
    text: 0xcdd6f4,
    text_secondary: 0xbac2de,
    text_muted: 0x7f849c,
    text_on_accent: 0xffffff,
    accent: 0x89b4fa,
    accent_hover: 0xb4befe,
    danger: 0xf38ba8,
    warning: 0xf9e2af,
    overlay: 0x0000_0099,
};
