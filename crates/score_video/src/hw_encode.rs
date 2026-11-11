//! 导出时按本机显卡选择 H.264 编码器.
//!
//! 裁剪版 ffmpeg 把硬件编码器编进同一个 sidecar, 运行时再探测.
//! 机器上没有对应驱动, 或者这一帧尺寸编码器拒收, 都退回 libx264.
//! 环境变量 `SCORE_SYNC_VIDEO_ENCODER` 可指定编码器名, 例如 `h264_qsv`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::export::ffmpeg_path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoEncoder {
    Libx264,
    Qsv,
    Nvenc,
    Amf,
    MediaFoundation,
    VideoToolbox,
}

impl VideoEncoder {
    pub(crate) fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Libx264 => "libx264",
            Self::Qsv => "h264_qsv",
            Self::Nvenc => "h264_nvenc",
            Self::Amf => "h264_amf",
            Self::MediaFoundation => "h264_mf",
            Self::VideoToolbox => "h264_videotoolbox",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Libx264 => "libx264",
            Self::Qsv => "Intel Quick Sync",
            Self::Nvenc => "NVIDIA NVENC",
            Self::Amf => "AMD AMF",
            Self::MediaFoundation => "Media Foundation",
            Self::VideoToolbox => "VideoToolbox",
        }
    }

    pub(crate) fn is_hardware(self) -> bool {
        self != Self::Libx264
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostKind {
    Windows,
    Macos,
    Linux,
    Other,
}

fn host_kind() -> HostKind {
    if cfg!(windows) {
        HostKind::Windows
    } else if cfg!(target_os = "macos") {
        HostKind::Macos
    } else if cfg!(target_os = "linux") {
        HostKind::Linux
    } else {
        HostKind::Other
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Gpus {
    nvidia: bool,
    amd: bool,
    intel: bool,
}

/// CRF 14..28 映射到「越大越好」的 40..95, 给 VideoToolbox 和 Media Foundation 用.
fn quality_percent(crf: u32) -> u32 {
    let crf = crf.clamp(1, 51);
    116u32.saturating_sub(crf.saturating_mul(2)).clamp(40, 95)
}

/// `-vf` 起, 到视频编码器参数为止. 不含 `-an` 和输出路径.
pub(crate) fn video_codec_args(encoder: VideoEncoder, crf: u32) -> Vec<String> {
    let crf = crf.clamp(1, 51).to_string();
    let q = quality_percent(crf.parse().unwrap_or(18)).to_string();
    let parts: &[&str] = match encoder {
        VideoEncoder::Libx264 => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "medium",
            "-crf",
            "",
            "-tune",
            "stillimage",
        ],
        // ICQ 的刻度和 CRF 相同, 数字越小越清晰. 软件帧由编码器自己送进核显.
        VideoEncoder::Qsv => &[
            "-vf",
            "format=nv12",
            "-c:v",
            "h264_qsv",
            "-preset",
            "medium",
            "-global_quality",
            "",
            "-pix_fmt",
            "nv12",
        ],
        VideoEncoder::Nvenc => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p5",
            "-tune",
            "hq",
            "-rc",
            "constqp",
            "-qp",
            "",
            "-profile:v",
            "high",
            "-pix_fmt",
            "yuv420p",
        ],
        VideoEncoder::Amf => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "h264_amf",
            "-usage",
            "transcoding",
            "-quality",
            "quality",
            "-rc",
            "cqp",
            "-qp_i",
            "",
            "-qp_p",
            "",
            "-qp_b",
            "",
            "-profile:v",
            "high",
            "-pix_fmt",
            "yuv420p",
        ],
        VideoEncoder::MediaFoundation => &[
            "-vf",
            "format=nv12",
            "-c:v",
            "h264_mf",
            "-hw_encoding",
            "1",
            "-rate_control",
            "quality",
            "-quality",
            "",
            "-pix_fmt",
            "nv12",
        ],
        VideoEncoder::VideoToolbox => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "h264_videotoolbox",
            "-profile:v",
            "high",
            "-q:v",
            "",
            "-pix_fmt",
            "yuv420p",
        ],
    };
    let numeric = match encoder {
        VideoEncoder::MediaFoundation | VideoEncoder::VideoToolbox => q.as_str(),
        _ => crf.as_str(),
    };
    parts
        .iter()
        .map(|part| if part.is_empty() { numeric } else { *part })
        .map(str::to_string)
        .collect()
}

fn candidate_order(host: HostKind, gpus: Gpus) -> Vec<VideoEncoder> {
    let mut out = Vec::new();
    let known = gpus.nvidia || gpus.amd || gpus.intel;
    let mut push_vendors = |out: &mut Vec<VideoEncoder>| {
        if !known || gpus.nvidia {
            out.push(VideoEncoder::Nvenc);
        }
        if !known || gpus.amd {
            out.push(VideoEncoder::Amf);
        }
        if !known || gpus.intel {
            out.push(VideoEncoder::Qsv);
        }
    };
    match host {
        HostKind::Windows => {
            push_vendors(&mut out);
            out.push(VideoEncoder::MediaFoundation);
        }
        HostKind::Linux => push_vendors(&mut out),
        HostKind::Macos => out.push(VideoEncoder::VideoToolbox),
        HostKind::Other => {}
    }
    out.push(VideoEncoder::Libx264);
    out
}

fn parse_encoder_name(name: &str) -> Option<VideoEncoder> {
    match name.trim().to_ascii_lowercase().as_str() {
        "libx264" | "x264" | "cpu" => Some(VideoEncoder::Libx264),
        "h264_qsv" | "qsv" => Some(VideoEncoder::Qsv),
        "h264_nvenc" | "nvenc" => Some(VideoEncoder::Nvenc),
        "h264_amf" | "amf" => Some(VideoEncoder::Amf),
        "h264_mf" | "mf" => Some(VideoEncoder::MediaFoundation),
        "h264_videotoolbox" | "videotoolbox" | "vt" => Some(VideoEncoder::VideoToolbox),
        _ => None,
    }
}

/// 只看显卡描述行, 避免注册表里别的字段碰到 Intel / AMD 这几个词.
fn gpus_from_driver_text(text: &str) -> Gpus {
    let mut gpus = Gpus::default();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("driverdesc") && !lower.contains("providername") {
            continue;
        }
        if lower.contains("nvidia") || lower.contains("geforce") {
            gpus.nvidia = true;
        }
        if lower.contains("radeon")
            || lower.contains("advanced micro devices")
            || lower.contains("amd")
        {
            gpus.amd = true;
        }
        if lower.contains("intel") {
            gpus.intel = true;
        }
    }
    gpus
}

fn gpus_from_pci_vendors(ids: impl IntoIterator<Item = u32>) -> Gpus {
    let mut gpus = Gpus::default();
    for id in ids {
        match id {
            0x10de => gpus.nvidia = true,
            0x1002 => gpus.amd = true,
            0x8086 => gpus.intel = true,
            _ => {}
        }
    }
    gpus
}

fn detect_gpus() -> Gpus {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("reg");
        cmd.args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
            "/s",
        ]);
        let output = hidden(&mut cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            return gpus_from_driver_text(&text);
        }
    }
    #[cfg(target_os = "linux")]
    {
        return gpus_from_pci_vendors(linux_pci_vendors());
    }
    #[cfg(not(target_os = "linux"))]
    {
        Gpus::default()
    }
}

#[cfg(target_os = "linux")]
fn linux_pci_vendors() -> Vec<u32> {
    let mut ids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return ids;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix("card") else {
            continue;
        };
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let path = entry.path().join("device/vendor");
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let text = text.trim().trim_start_matches("0x").trim_start_matches("0X");
        if let Ok(id) = u32::from_str_radix(text, 16) {
            ids.push(id);
        }
    }
    ids
}

fn listed_encoders() -> Vec<String> {
    let mut cmd = Command::new(ffmpeg_path());
    cmd.args(["-hide_banner", "-encoders"]);
    let output = hidden(&mut cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn probe_encoder(encoder: VideoEncoder) -> bool {
    const W: usize = 256;
    const H: usize = 256;
    let frame = vec![0u8; W * H * 4];
    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "rgba".to_string(),
        "-s".to_string(),
        format!("{W}x{H}"),
        "-framerate".to_string(),
        "30".to_string(),
        "-i".to_string(),
        "-".to_string(),
        "-frames:v".to_string(),
        "2".to_string(),
    ];
    args.extend(video_codec_args(encoder, 18));
    args.extend(["-an".to_string(), "-f".to_string(), "null".to_string(), "-".to_string()]);

    let mut cmd = Command::new(ffmpeg_path());
    hidden(&mut cmd);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        for _ in 0..2 {
            if stdin.write_all(&frame).is_err() {
                break;
            }
        }
        let _ = stdin.flush();
    }
    wait_timeout(&mut child, Duration::from_secs(12))
        .map(|status| status.success())
        .unwrap_or(false)
}

fn wait_timeout(child: &mut std::process::Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                return child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => return None,
        }
    }
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn choose_video_encoder() -> VideoEncoder {
    if let Ok(raw) = std::env::var("SCORE_SYNC_VIDEO_ENCODER") {
        if let Some(encoder) = parse_encoder_name(&raw) {
            if encoder == VideoEncoder::Libx264 {
                return encoder;
            }
            let listed = listed_encoders();
            if listed.iter().any(|name| name == encoder.ffmpeg_name()) && probe_encoder(encoder) {
                return encoder;
            }
            return VideoEncoder::Libx264;
        }
    }
    let listed = listed_encoders();
    let gpus = detect_gpus();
    for encoder in candidate_order(host_kind(), gpus) {
        if encoder == VideoEncoder::Libx264 {
            return encoder;
        }
        if listed.iter().any(|name| name == encoder.ffmpeg_name()) && probe_encoder(encoder) {
            return encoder;
        }
    }
    VideoEncoder::Libx264
}

pub(crate) fn select_video_encoder() -> VideoEncoder {
    static CACHE: OnceLock<VideoEncoder> = OnceLock::new();
    *CACHE.get_or_init(choose_video_encoder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_tracks_crf() {
        assert_eq!(quality_percent(14), 88);
        assert_eq!(quality_percent(18), 80);
        assert_eq!(quality_percent(23), 70);
        assert_eq!(quality_percent(28), 60);
    }

    #[test]
    fn x264_keeps_stillimage_crf() {
        let args = video_codec_args(VideoEncoder::Libx264, 18);
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-crf", "18"]));
        assert!(args.iter().any(|a| a == "stillimage"));
        assert!(!args.iter().any(|a| a == "h264_qsv"));
    }

    #[test]
    fn qsv_uses_icq_not_crf() {
        let args = video_codec_args(VideoEncoder::Qsv, 18);
        assert!(args.windows(2).any(|w| w == ["-c:v", "h264_qsv"]));
        assert!(args.windows(2).any(|w| w == ["-global_quality", "18"]));
        assert!(args.windows(2).any(|w| w == ["-vf", "format=nv12"]));
        assert!(!args.iter().any(|a| a == "-crf" || a == "stillimage"));
    }

    #[test]
    fn intel_only_prefers_qsv_before_media_foundation() {
        let order = candidate_order(
            HostKind::Windows,
            Gpus {
                intel: true,
                ..Gpus::default()
            },
        );
        assert_eq!(
            order,
            vec![
                VideoEncoder::Qsv,
                VideoEncoder::MediaFoundation,
                VideoEncoder::Libx264
            ]
        );
    }

    #[test]
    fn nvidia_and_intel_tries_nvenc_first() {
        let order = candidate_order(
            HostKind::Windows,
            Gpus {
                nvidia: true,
                intel: true,
                ..Gpus::default()
            },
        );
        assert_eq!(order.first().copied(), Some(VideoEncoder::Nvenc));
        assert!(order.contains(&VideoEncoder::Qsv));
        assert!(!order.contains(&VideoEncoder::Amf));
    }

    #[test]
    fn unknown_gpu_tries_every_windows_encoder() {
        let order = candidate_order(HostKind::Windows, Gpus::default());
        assert_eq!(
            order,
            vec![
                VideoEncoder::Nvenc,
                VideoEncoder::Amf,
                VideoEncoder::Qsv,
                VideoEncoder::MediaFoundation,
                VideoEncoder::Libx264
            ]
        );
    }

    #[test]
    fn macos_is_videotoolbox_only() {
        let order = candidate_order(
            HostKind::Macos,
            Gpus {
                intel: true,
                ..Gpus::default()
            },
        );
        assert_eq!(
            order,
            vec![VideoEncoder::VideoToolbox, VideoEncoder::Libx264]
        );
    }

    #[test]
    fn driver_text_ignores_unrelated_lines() {
        let text = "\
            DriverDesc    REG_SZ    Intel(R) UHD Graphics 770\n\
            MatchingDeviceId    REG_SZ    PCI\\VEN_10DE\n\
            ProviderName    REG_SZ    Advanced Micro Devices, Inc.\n";
        let gpus = gpus_from_driver_text(text);
        assert!(gpus.intel);
        assert!(gpus.amd);
        assert!(!gpus.nvidia);
    }

    #[test]
    fn pci_vendors_map_to_gpus() {
        let gpus = gpus_from_pci_vendors([0x8086, 0x10de]);
        assert!(gpus.intel && gpus.nvidia);
        assert!(!gpus.amd);
    }
}
