//! 路径/比例偏好: 存于 %APPDATA%/apply_bg (否则临时目录).

use std::fs;
use std::path::PathBuf;

use crate::process::{parse_aspect, DEFAULT_ASPECT_H, DEFAULT_ASPECT_W};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub bg: String,
    pub in_dir: String,
    pub out_dir: String,
    /// 空表示尚未写入过, UI/逻辑回退到默认 2560:1440.
    pub aspect: String,
}

impl Config {
    pub fn aspect_or_default(&self) -> (u32, u32) {
        if self.aspect.trim().is_empty() {
            (DEFAULT_ASPECT_W, DEFAULT_ASPECT_H)
        } else {
            parse_aspect(&self.aspect).unwrap_or((DEFAULT_ASPECT_W, DEFAULT_ASPECT_H))
        }
    }

}

pub fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join("apply_bg");
        }
    }
    std::env::temp_dir().join("apply_bg")
}

fn config_path() -> PathBuf {
    config_dir().join("config.txt")
}

pub fn load() -> Config {
    let path = config_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Config::default();
    };
    let mut cfg = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("bg=") {
            cfg.bg = v.to_string();
        } else if let Some(v) = line.strip_prefix("in=") {
            cfg.in_dir = v.to_string();
        } else if let Some(v) = line.strip_prefix("out=") {
            cfg.out_dir = v.to_string();
        } else if let Some(v) = line.strip_prefix("aspect=") {
            cfg.aspect = v.to_string();
        }
    }
    cfg
}

pub fn save(cfg: &Config) {
    let dir = config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut body = format!(
        "bg={}\nin={}\nout={}\n",
        cfg.bg, cfg.in_dir, cfg.out_dir
    );
    // 仅在用户改过/显式设过后写入比例, 保持"初次不写死"
    if !cfg.aspect.trim().is_empty() {
        body.push_str(&format!("aspect={}\n", cfg.aspect.trim()));
    }
    let _ = fs::write(config_path(), body);
}

