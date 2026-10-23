//! 上次打开工程路径、主窗口位置、蒙版选色偏好: 存于用户配置目录 (Windows: %APPDATA%/score_sync).
//!
//! `config.json` 为新格式; 若仅有旧版 `config.txt` 则读取 `last_project=` 并迁移.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use mask_tool::color_prefs::MaskColorPrefs;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// 最近一次成功打开/保存的工程文件路径 (`.staffcrop`).
    #[serde(default)]
    pub last_project: String,
    /// 蒙版/画笔颜色与透明度偏好 (新工程默认从此读取).
    #[serde(default)]
    pub mask_prefs: MaskColorPrefs,
    /// 上次 PDF 导入倍率 (标记 pt × 此值 = 像素).
    #[serde(default = "default_pdf_import_scale")]
    pub pdf_import_scale: f32,
    /// 上次 PDF 导入是否锁定宽高比.
    #[serde(default = "default_true")]
    pub pdf_import_lock_aspect: bool,
    /// 界面配色方案 id. 空或未知时用「默认」.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// `zh` 或 `en`.
    #[serde(default = "default_language")]
    pub language: String,
    /// 启动时是否重新打开上次工程.
    #[serde(default = "default_true")]
    pub reopen_last_project: bool,
    /// 启动时是否检查 GitHub 更新.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// 菜单文字后面是否带快捷键.
    #[serde(default = "default_true")]
    pub show_menu_shortcuts: bool,
    /// 和默认方案不同的键位. 键是命令 id, 值与默认槽位一一对应, 空串表示解开.
    #[serde(default)]
    pub shortcuts: HashMap<String, Vec<String>>,
    /// 上次主窗口的位置和大小. 没有则居中打开.
    #[serde(default)]
    pub window: Option<WindowPlacement>,
}

/// 主窗口上次停留的客户区位置 (逻辑像素) 和是否最大化.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

fn default_theme() -> String {
    "default".into()
}

fn default_language() -> String {
    "zh".into()
}

fn default_pdf_import_scale() -> f32 {
    3.0
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            last_project: String::new(),
            mask_prefs: MaskColorPrefs::default(),
            pdf_import_scale: default_pdf_import_scale(),
            pdf_import_lock_aspect: true,
            theme: default_theme(),
            language: default_language(),
            reopen_last_project: true,
            check_updates: true,
            show_menu_shortcuts: true,
            shortcuts: HashMap::new(),
            window: None,
        }
    }
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

fn config_json_path() -> PathBuf {
    config_dir().join("config.json")
}

fn config_txt_path() -> PathBuf {
    config_dir().join("config.txt")
}

fn load_legacy_txt() -> Option<Config> {
    let text = fs::read_to_string(config_txt_path()).ok()?;
    let mut cfg = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("last_project=") {
            cfg.last_project = v.to_string();
        }
    }
    Some(cfg)
}

pub fn load() -> Config {
    if let Ok(text) = fs::read_to_string(config_json_path()) {
        if let Ok(mut cfg) = serde_json::from_str::<Config>(&text) {
            cfg.mask_prefs = cfg.mask_prefs.clamp();
            return cfg;
        }
    }
    load_legacy_txt().unwrap_or_default()
}

pub fn save(cfg: &Config) {
    let dir = config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut cfg = cfg.clone();
    cfg.mask_prefs = cfg.mask_prefs.clamp();
    if let Ok(body) = serde_json::to_string_pretty(&cfg) {
        let _ = fs::write(config_json_path(), body);
    }
    ui_kit::apply_prefs(
        &cfg.theme,
        &cfg.language,
        cfg.show_menu_shortcuts,
        cfg.reopen_last_project,
        cfg.check_updates,
        cfg.shortcuts,
    );
}

/// 打开/保存工程成功后调用: 把这次的路径记为"上次打开的工程" (保留选色偏好).
pub fn remember_last_project(path: &std::path::Path) {
    let mut cfg = load();
    cfg.last_project = path.display().to_string();
    save(&cfg);
}

pub fn forget_last_project() {
    let mut cfg = load();
    if cfg.last_project.is_empty() {
        return;
    }
    cfg.last_project.clear();
    save(&cfg);
}

/// 记住本次 PDF 导入倍率与是否锁定宽高比.
pub fn remember_pdf_import(scale: f32, lock_aspect: bool) {
    let mut cfg = load();
    cfg.pdf_import_scale = crate::pdf::clamp_pdf_scale(scale);
    cfg.pdf_import_lock_aspect = lock_aspect;
    save(&cfg);
}

/// 把当前蒙版选色偏好写入 appdata (新工程默认用).
pub fn remember_mask_prefs(prefs: &MaskColorPrefs) {
    let mut cfg = load();
    cfg.mask_prefs = prefs.clone().clamp();
    save(&cfg);
}

/// 记住主窗口位置. 尺寸过小或坐标非法时不写, 避免最小化或未布局完成的瞬时值盖掉上次的有效位置.
pub fn remember_window(x: f32, y: f32, width: f32, height: f32, maximized: bool) {
    if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() {
        return;
    }
    if width < 80. || height < 60. {
        return;
    }
    let next = WindowPlacement {
        x,
        y,
        width,
        height,
        maximized,
    };
    let mut cfg = load();
    if cfg
        .window
        .as_ref()
        .is_some_and(|old| same_placement(old, &next))
    {
        return;
    }
    cfg.window = Some(next);
    save(&cfg);
}

fn same_placement(a: &WindowPlacement, b: &WindowPlacement) -> bool {
    a.maximized == b.maximized
        && (a.x - b.x).abs() < 0.5
        && (a.y - b.y).abs() < 0.5
        && (a.width - b.width).abs() < 0.5
        && (a.height - b.height).abs() < 0.5
}

/// 把上次窗口摆到仍有重叠的那块屏幕上, 并保证至少有一条边留在屏幕内.
/// 屏幕坐标是 `(x, y, width, height)`. 对不上任何屏幕时居中放到 `primary`.
/// 返回 `(x, y, width, height, screen_index)`.
pub fn place_window(
    saved: &WindowPlacement,
    screens: &[(f32, f32, f32, f32)],
    primary: usize,
) -> Option<(f32, f32, f32, f32, usize)> {
    const MIN_W: f32 = 200.;
    const MIN_H: f32 = 160.;
    const VISIBLE: f32 = 48.;
    if screens.is_empty()
        || !saved.x.is_finite()
        || !saved.y.is_finite()
        || !saved.width.is_finite()
        || !saved.height.is_finite()
        || saved.width < 80.
        || saved.height < 60.
    {
        return None;
    }
    let primary = primary.min(screens.len() - 1);
    let host = screens
        .iter()
        .enumerate()
        .map(|(i, screen)| (i, overlap(saved, *screen)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, area)| *area > 0.)
        .map(|(i, _)| i);
    let Some(host) = host else {
        let (sx, sy, sw, sh) = screens[primary];
        if sw < 1. || sh < 1. {
            return None;
        }
        let w = saved.width.clamp(MIN_W.min(sw), sw);
        let h = saved.height.clamp(MIN_H.min(sh), sh);
        return Some((sx + (sw - w) * 0.5, sy + (sh - h) * 0.5, w, h, primary));
    };
    let (sx, sy, sw, sh) = screens[host];
    if sw < 1. || sh < 1. {
        return None;
    }
    let w = saved.width.clamp(MIN_W.min(sw), sw);
    let h = saved.height.clamp(MIN_H.min(sh), sh);
    let visible_x = VISIBLE.min(w);
    let visible_y = VISIBLE.min(h);
    // 尺寸被屏幕裁过时贴齐屏幕原点, 避免裁完还向屏幕外偏出一截.
    let x = if saved.width > sw + 0.5 {
        sx
    } else {
        saved.x.clamp(sx + visible_x - w, sx + sw - visible_x)
    };
    let y = if saved.height > sh + 0.5 {
        sy
    } else {
        saved.y.clamp(sy, sy + sh - visible_y)
    };
    Some((x, y, w, h, host))
}

fn overlap(saved: &WindowPlacement, screen: (f32, f32, f32, f32)) -> f32 {
    let (sx, sy, sw, sh) = screen;
    let x1 = saved.x.max(sx);
    let y1 = saved.y.max(sy);
    let x2 = (saved.x + saved.width).min(sx + sw);
    let y2 = (saved.y + saved.height).min(sy + sh);
    (x2 - x1).max(0.) * (y2 - y1).max(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(x: f32, y: f32, width: f32, height: f32) -> WindowPlacement {
        WindowPlacement {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    #[test]
    fn keeps_a_window_on_the_screen_it_overlaps() {
        let screens = [(0., 0., 1920., 1080.), (1920., 0., 1920., 1080.)];
        let (x, y, w, h, screen) =
            place_window(&placement(2100., 120., 900., 700.), &screens, 0).unwrap();
        assert_eq!(screen, 1);
        assert_eq!((x, y, w, h), (2100., 120., 900., 700.));
    }

    #[test]
    fn centers_when_the_old_monitor_is_gone() {
        let screens = [(0., 0., 1600., 900.)];
        let (x, y, w, h, screen) =
            place_window(&placement(4000., 200., 800., 600.), &screens, 0).unwrap();
        assert_eq!(screen, 0);
        assert_eq!((x, y, w, h), (400., 150., 800., 600.));
    }

    #[test]
    fn shrinks_a_window_that_is_larger_than_the_screen() {
        let screens = [(0., 0., 1280., 800.)];
        let (x, y, w, h, _) = place_window(&placement(40., 30., 2000., 900.), &screens, 0).unwrap();
        assert_eq!((x, y, w, h), (0., 0., 1280., 800.));
    }

    #[test]
    fn pulls_a_partly_offscreen_window_back_into_view() {
        let screens = [(0., 0., 1280., 800.)];
        // 只有 20px 还留在屏幕里, 应被拉回至少 48px.
        let (x, y, w, h, _) =
            place_window(&placement(-580., 40., 600., 400.), &screens, 0).unwrap();
        assert_eq!((x, y, w, h), (-552., 40., 600., 400.));
        assert!((x + w) >= 48.);
    }
}
