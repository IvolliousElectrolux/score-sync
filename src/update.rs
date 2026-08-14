//! 启动时后台检查 GitHub 是否有更新的正式版. 只查一次, 失败/离线则静默.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

const API_URL: &str = "https://api.github.com/repos/IvolliousElectrolux/score-sync/releases/latest";
const USER_AGENT: &str = concat!("score-sync/", env!("CARGO_PKG_VERSION"));

#[derive(Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
}

/// 阻塞调用, 须在后台线程跑. 无更新、离线或解析失败都返回 `None`.
pub fn check_latest() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    crate::trace::log(&format!("update: 检查 GitHub, 当前 {current}"));
    let body = match fetch_latest_json() {
        Ok(s) => s,
        Err(e) => {
            crate::trace::log(&format!("update: 跳过 ({e})"));
            return None;
        }
    };
    let rel: GhRelease = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            crate::trace::log(&format!("update: 解析失败 ({e})"));
            return None;
        }
    };
    let latest = rel.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
    crate::trace::log(&format!("update: GitHub 最新 {latest}"));
    if is_newer(&latest, &current) {
        Some(UpdateInfo {
            current,
            latest,
            url: rel.html_url,
        })
    } else {
        None
    }
}

fn fetch_latest_json() -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .build();
    let resp = agent
        .get(API_URL)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_string().map_err(|e| e.to_string())
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .and_then(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0);
    Some((major, minor, patch))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest != current,
    }
}

pub fn open_in_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
