//! 上次打开工程路径 + 蒙版选色偏好: 存于用户配置目录 (Windows: %APPDATA%/score_sync).
//!
//! `config.json` 为新格式; 若仅有旧版 `config.txt` 则读取 `last_project=` 并迁移.

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            last_project: String::new(),
            mask_prefs: MaskColorPrefs::default(),
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
}

/// 打开/保存工程成功后调用: 把这次的路径记为"上次打开的工程" (保留选色偏好).
pub fn remember_last_project(path: &std::path::Path) {
    let mut cfg = load();
    cfg.last_project = path.display().to_string();
    save(&cfg);
}

/// 把当前蒙版选色偏好写入 appdata (新工程默认用).
pub fn remember_mask_prefs(prefs: &MaskColorPrefs) {
    let mut cfg = load();
    cfg.mask_prefs = prefs.clone().clamp();
    save(&cfg);
}
