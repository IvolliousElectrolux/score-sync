//! 启动时后台检查 GitHub 是否有更新的正式版. 只查一次, 失败/离线则静默.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

const RELEASES_URL: &str =
    "https://api.github.com/repos/IvolliousElectrolux/score-sync/releases?per_page=100";
const USER_AGENT: &str = concat!("score-sync/", env!("CARGO_PKG_VERSION"));

#[derive(Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub url: String,
    /// 比当前新的各正式版 (新的在前). 每项为 (版本号, 条目).
    pub changes: Vec<(String, Vec<String>)>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// 阻塞调用, 须在后台线程跑. 无更新、离线或解析失败都返回 `None`.
pub fn check_latest() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    crate::trace::log(&format!("update: 检查 GitHub, 当前 {current}"));
    let body = match fetch_json(RELEASES_URL) {
        Ok(s) => s,
        Err(e) => {
            crate::trace::log(&format!("update: 跳过 ({e})"));
            return None;
        }
    };
    let rels: Vec<GhRelease> = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            crate::trace::log(&format!("update: 解析失败 ({e})"));
            return None;
        }
    };

    let mut newer: Vec<(String, String, Vec<String>)> = Vec::new();
    for rel in rels {
        if rel.draft || rel.prerelease {
            continue;
        }
        let ver = normalize_tag(&rel.tag_name);
        if !is_newer(&ver, &current) {
            continue;
        }
        let bullets = extract_bullets(rel.body.as_deref().unwrap_or(""));
        newer.push((ver, rel.html_url, bullets));
    }
    if newer.is_empty() {
        crate::trace::log("update: 已是最新");
        return None;
    }
    newer.sort_by(|a, b| cmp_semver(&b.0, &a.0));
    let latest = newer[0].0.clone();
    let url = newer[0].1.clone();
    crate::trace::log(&format!(
        "update: GitHub 最新 {latest}, 间隔 {} 个版本",
        newer.len()
    ));
    let changes = newer.into_iter().map(|(v, _, b)| (v, b)).collect();
    Some(UpdateInfo {
        current,
        latest,
        url,
        changes,
    })
}

fn fetch_json(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_string().map_err(|e| e.to_string())
}

fn normalize_tag(s: &str) -> String {
    s.trim().trim_start_matches(['v', 'V']).to_string()
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = normalize_tag(s);
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

fn cmp_semver(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_semver(a), parse_semver(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

fn is_newer(latest: &str, current: &str) -> bool {
    cmp_semver(latest, current) == std::cmp::Ordering::Greater
}

/// 从 GitHub Release body 抽出「主要变化」条目, 去掉安装说明.
fn extract_bullets(body: &str) -> Vec<String> {
    let text = body.replace('\r', "");
    let after = if let Some((_, rest)) = split_heading(&text, "主要变化") {
        rest
    } else {
        skip_install_section(&text)
    };
    let section = cut_at_next_heading(after);
    let mut out = Vec::new();
    for line in section.lines() {
        let t = line.trim();
        let item = if let Some(s) = t.strip_prefix("- ") {
            s
        } else if let Some(s) = t.strip_prefix("* ") {
            s
        } else {
            continue;
        };
        let item = item.replace("**", "").replace('`', "");
        let item = item.trim();
        if !item.is_empty() {
            out.push(item.to_string());
        }
    }
    out
}

fn split_heading<'a>(text: &'a str, title: &str) -> Option<(&'a str, &'a str)> {
    for prefix in ["### ", "## "] {
        let needle = format!("{prefix}{title}");
        if let Some(idx) = text.find(&needle) {
            let rest = &text[idx + needle.len()..];
            let rest = rest.strip_prefix('\n').unwrap_or(rest);
            return Some((&text[..idx], rest));
        }
    }
    None
}

fn skip_install_section(text: &str) -> &str {
    let Some((_, rest)) = split_heading(text, "安装") else {
        return text;
    };
    for (i, line) in rest.lines().enumerate() {
        let t = line.trim();
        if i > 0 && (t.starts_with("### ") || t.starts_with("## ")) {
            let pos = rest.find(line).unwrap_or(0);
            return &rest[pos..];
        }
    }
    ""
}

fn cut_at_next_heading(text: &str) -> &str {
    let mut seen_content = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("### ") || t.starts_with("## ") {
            if seen_content {
                if let Some(pos) = text.find(line) {
                    return &text[..pos];
                }
            }
        } else if !t.is_empty() {
            seen_content = true;
        }
    }
    text
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_1_3_2: &str = r#"## Score Sync 1.3.2

### 安装
解压后把这些文件放在同一目录:
- `score_sync.exe`
- `ffmpeg.exe` (视频导出)
- `pdfium.dll` (PDF 打开)
- `底色.png` (随包装好的底色图, 工程面板可直接选用)

### 主要变化
- **加底色**: 谱面按工程比例完整装进画布; 装得下则宽=谱面宽、上下补边, 上下会裁则改按高度、左右补边
- **视频**: 导出分辨率取素材中最大的一张; 总谱不再因按宽对齐而被上下截断
"#;

    #[test]
    fn extract_skips_install_keeps_changes() {
        let bullets = extract_bullets(SAMPLE_1_3_2);
        assert_eq!(bullets.len(), 2);
        assert!(bullets[0].starts_with("加底色"));
        assert!(bullets[1].starts_with("视频"));
        assert!(!bullets.iter().any(|b| b.contains("score_sync.exe")));
    }

    #[test]
    fn extract_empty_body() {
        assert!(extract_bullets("").is_empty());
        assert!(extract_bullets("### 安装\n- foo\n").is_empty());
    }

    #[test]
    fn newer_compares_semver() {
        assert!(is_newer("1.3.2", "1.3.1"));
        assert!(is_newer("v1.4.0", "1.3.9"));
        assert!(!is_newer("1.3.1", "1.3.1"));
        assert!(!is_newer("1.2.9", "1.3.0"));
    }
}
