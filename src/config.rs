//! 上次打开工程路径记忆: 存于 %APPDATA%/score_sync (否则临时目录), 与
//! `apply_bg` 记忆底色/输入输出路径是同一套逻辑, 方便下次启动时自动恢复.

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// 最近一次成功打开/保存的工程文件路径 (`.staffcrop`).
    pub last_project: String,
}

pub fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join("score_sync");
        }
    }
    std::env::temp_dir().join("score_sync")
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
        if let Some(v) = line.strip_prefix("last_project=") {
            cfg.last_project = v.to_string();
        }
    }
    cfg
}

pub fn save(cfg: &Config) {
    let dir = config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let body = format!("last_project={}\n", cfg.last_project);
    let _ = fs::write(config_path(), body);
}

/// 打开/保存工程成功后调用: 把这次的路径记为"上次打开的工程".
pub fn remember_last_project(path: &std::path::Path) {
    save(&Config {
        last_project: path.display().to_string(),
    });
}
