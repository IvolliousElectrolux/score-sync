//! 导出时按本机显卡和所选编码 (AVC / HEVC / AV1) 选编码器.
//!
//! 裁剪版 ffmpeg 把硬件编码器编进同一个 sidecar, 运行时再探测.
//! 机器上没有对应驱动, 或者这一帧尺寸编码器拒收, 都退回该编码的软件编码器.
//! HEVC / AV1 没有探测通过的硬件编码器时, 界面上不出现这一项.
//! 环境变量 `SCORE_SYNC_VIDEO_ENCODER` 可指定编码器名, 例如 `h264_qsv` 或 `hevc_nvenc`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::export::{ffmpeg_path, VideoCodec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoEncoder {
    Software,
    Qsv,
    Nvenc,
    Amf,
    MediaFoundation,
    VideoToolbox,
}

impl VideoEncoder {
    pub(crate) fn ffmpeg_name(self, codec: VideoCodec) -> &'static str {
        encoder_name(codec, self).unwrap_or(match codec {
            VideoCodec::Avc => "libx264",
            VideoCodec::Hevc => "libx265",
            VideoCodec::Av1 => "libsvtav1",
        })
    }

    pub(crate) fn label(self, codec: VideoCodec) -> &'static str {
        match self {
            Self::Software => self.ffmpeg_name(codec),
            Self::Qsv => "Intel Quick Sync",
            Self::Nvenc => "NVIDIA NVENC",
            Self::Amf => "AMD AMF",
            Self::MediaFoundation => "Media Foundation",
            Self::VideoToolbox => "VideoToolbox",
        }
    }

    pub(crate) fn is_hardware(self) -> bool {
        self != Self::Software
    }

    /// 编码器直接吃的未压缩格式. 输入已经是这个格式时不要再挂 `format` 滤镜,
    /// 否则每一帧都在 CPU 上重做一遍 RGB→YUV.
    pub(crate) fn stored_yuv(self) -> crate::yuv::Yuv420 {
        match self {
            Self::Qsv | Self::MediaFoundation => crate::yuv::Yuv420::Nv12,
            _ => crate::yuv::Yuv420::I420,
        }
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

/// 这一对 (编码, 硬件) 在裁剪版 ffmpeg 7.1 里有没有对应编码器.
/// AV1 没有 Media Foundation, 也没有 VideoToolbox.
pub(crate) fn encoder_name(codec: VideoCodec, encoder: VideoEncoder) -> Option<&'static str> {
    Some(match (codec, encoder) {
        (VideoCodec::Avc, VideoEncoder::Software) => "libx264",
        (VideoCodec::Hevc, VideoEncoder::Software) => "libx265",
        (VideoCodec::Av1, VideoEncoder::Software) => "libsvtav1",
        (VideoCodec::Avc, VideoEncoder::Qsv) => "h264_qsv",
        (VideoCodec::Hevc, VideoEncoder::Qsv) => "hevc_qsv",
        (VideoCodec::Av1, VideoEncoder::Qsv) => "av1_qsv",
        (VideoCodec::Avc, VideoEncoder::Nvenc) => "h264_nvenc",
        (VideoCodec::Hevc, VideoEncoder::Nvenc) => "hevc_nvenc",
        (VideoCodec::Av1, VideoEncoder::Nvenc) => "av1_nvenc",
        (VideoCodec::Avc, VideoEncoder::Amf) => "h264_amf",
        (VideoCodec::Hevc, VideoEncoder::Amf) => "hevc_amf",
        (VideoCodec::Av1, VideoEncoder::Amf) => "av1_amf",
        (VideoCodec::Avc, VideoEncoder::MediaFoundation) => "h264_mf",
        (VideoCodec::Hevc, VideoEncoder::MediaFoundation) => "hevc_mf",
        (VideoCodec::Av1, VideoEncoder::MediaFoundation) => return None,
        (VideoCodec::Avc, VideoEncoder::VideoToolbox) => "h264_videotoolbox",
        (VideoCodec::Hevc, VideoEncoder::VideoToolbox) => "hevc_videotoolbox",
        (VideoCodec::Av1, VideoEncoder::VideoToolbox) => return None,
    })
}

/// `-vf` 起, 到视频编码器参数为止. 不含 `-an` 和输出路径.
pub(crate) fn video_codec_args(codec: VideoCodec, encoder: VideoEncoder, crf: u32) -> Vec<String> {
    let crf_n = crf.clamp(1, 51).to_string();
    let q = quality_percent(crf.clamp(1, 51)).to_string();
    let name = encoder.ffmpeg_name(codec);
    let numeric = match encoder {
        VideoEncoder::MediaFoundation | VideoEncoder::VideoToolbox => q.as_str(),
        _ => crf_n.as_str(),
    };
    let parts: &[&str] = match (codec, encoder) {
        (_, VideoEncoder::Software) => match codec {
            VideoCodec::Avc => &[
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
            VideoCodec::Hevc => &[
                "-vf",
                "format=yuv420p",
                "-c:v",
                "libx265",
                "-pix_fmt",
                "yuv420p",
                "-preset",
                "medium",
                "-tune",
                "stillimage",
                "-tag:v",
                "hvc1",
                "-x265-params",
                "bframes=0",
                "-crf",
                "",
            ],
            VideoCodec::Av1 => &[
                "-vf",
                "format=yuv420p",
                "-c:v",
                "libsvtav1",
                "-pix_fmt",
                "yuv420p",
                "-preset",
                "6",
                "-crf",
                "",
            ],
        },
        // ICQ 的刻度和 CRF 相同, 数字越小越清晰. 软件帧由编码器自己送进核显.
        (_, VideoEncoder::Qsv) => &[
            "-vf",
            "format=nv12",
            "-c:v",
            "",
            "-preset",
            "medium",
            "-global_quality",
            "",
            "-pix_fmt",
            "nv12",
        ],
        (VideoCodec::Avc, VideoEncoder::Nvenc) => &[
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
        (_, VideoEncoder::Nvenc) => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "",
            "-preset",
            "p5",
            "-tune",
            "hq",
            "-rc",
            "constqp",
            "-qp",
            "",
            "-pix_fmt",
            "yuv420p",
        ],
        (VideoCodec::Avc, VideoEncoder::Amf) => &[
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
        (_, VideoEncoder::Amf) => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "",
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
            "-pix_fmt",
            "yuv420p",
        ],
        (_, VideoEncoder::MediaFoundation) => &[
            "-vf",
            "format=nv12",
            "-c:v",
            "",
            "-hw_encoding",
            "1",
            "-rate_control",
            "quality",
            "-quality",
            "",
            "-pix_fmt",
            "nv12",
        ],
        (VideoCodec::Avc, VideoEncoder::VideoToolbox) => &[
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
        (_, VideoEncoder::VideoToolbox) => &[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "",
            "-profile:v",
            "main",
            "-q:v",
            "",
            "-pix_fmt",
            "yuv420p",
        ],
    };
    parts
        .iter()
        .map(|part| {
            if part.is_empty() {
                if parts
                    .windows(2)
                    .any(|w| w[0] == "-c:v" && w[1].is_empty() && *part == "")
                {
                    // filled below
                }
                numeric
            } else {
                *part
            }
        })
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            if i > 0 && parts[i - 1] == "-c:v" && parts[i].is_empty() {
                name.to_string()
            } else if parts[i].is_empty() {
                numeric.to_string()
            } else {
                part
            }
        })
        .collect()
}

fn candidate_order(host: HostKind, gpus: Gpus) -> Vec<VideoEncoder> {
    let mut out = Vec::new();
    let known = gpus.nvidia || gpus.amd || gpus.intel;
    let push_vendors = |out: &mut Vec<VideoEncoder>| {
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
    out.push(VideoEncoder::Software);
    out
}

/// `None` 编码表示这个名字只指定厂商, 三种编码都优先用它.
fn parse_encoder_name(name: &str) -> Option<(Option<VideoCodec>, VideoEncoder)> {
    let name = name.trim().to_ascii_lowercase();
    let vendor = match name.as_str() {
        "qsv" | "h264_qsv" | "hevc_qsv" | "av1_qsv" => VideoEncoder::Qsv,
        "nvenc" | "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => VideoEncoder::Nvenc,
        "amf" | "h264_amf" | "hevc_amf" | "av1_amf" => VideoEncoder::Amf,
        "mf" | "h264_mf" | "hevc_mf" => VideoEncoder::MediaFoundation,
        "videotoolbox" | "vt" | "h264_videotoolbox" | "hevc_videotoolbox" => {
            VideoEncoder::VideoToolbox
        }
        "libx264" | "x264" | "cpu" | "libx265" | "libsvtav1" => VideoEncoder::Software,
        _ => return None,
    };
    let codec = match name.as_str() {
        "h264_qsv" | "h264_nvenc" | "h264_amf" | "h264_mf" | "h264_videotoolbox" | "libx264"
        | "x264" => Some(VideoCodec::Avc),
        "hevc_qsv" | "hevc_nvenc" | "hevc_amf" | "hevc_mf" | "hevc_videotoolbox" | "libx265" => {
            Some(VideoCodec::Hevc)
        }
        "av1_qsv" | "av1_nvenc" | "av1_amf" | "libsvtav1" => Some(VideoCodec::Av1),
        _ => None,
    };
    Some((codec, vendor))
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

#[cfg(any(test, target_os = "linux"))]
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
        let text = text
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X");
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

fn probe_encoder(codec: VideoCodec, encoder: VideoEncoder) -> bool {
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
    args.extend(video_codec_args(codec, encoder, 18));
    args.extend([
        "-an".to_string(),
        "-f".to_string(),
        "null".to_string(),
        "-".to_string(),
    ]);

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

fn wait_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
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

struct EncoderCaps {
    avc: VideoEncoder,
    hevc: VideoEncoder,
    av1: VideoEncoder,
}

fn pick_for(codec: VideoCodec, listed: &[String], order: &[VideoEncoder]) -> VideoEncoder {
    let forced = std::env::var("SCORE_SYNC_VIDEO_ENCODER")
        .ok()
        .and_then(|raw| parse_encoder_name(&raw));
    if let Some((forced_codec, vendor)) = forced {
        if forced_codec == Some(codec) || forced_codec.is_none() {
            if vendor == VideoEncoder::Software {
                return VideoEncoder::Software;
            }
            if encoder_name(codec, vendor).is_some()
                && listed.iter().any(|name| name == vendor.ffmpeg_name(codec))
                && probe_encoder(codec, vendor)
            {
                return vendor;
            }
            if forced_codec == Some(codec) {
                return VideoEncoder::Software;
            }
        }
    }
    for vendor in order {
        if *vendor == VideoEncoder::Software {
            return VideoEncoder::Software;
        }
        let Some(name) = encoder_name(codec, *vendor) else {
            continue;
        };
        if listed.iter().any(|item| item == name) && probe_encoder(codec, *vendor) {
            return *vendor;
        }
    }
    VideoEncoder::Software
}

fn encoder_caps() -> EncoderCaps {
    let listed = listed_encoders();
    let order = candidate_order(host_kind(), detect_gpus());
    EncoderCaps {
        avc: pick_for(VideoCodec::Avc, &listed, &order),
        hevc: pick_for(VideoCodec::Hevc, &listed, &order),
        av1: pick_for(VideoCodec::Av1, &listed, &order),
    }
}

fn caps() -> &'static EncoderCaps {
    static CACHE: OnceLock<EncoderCaps> = OnceLock::new();
    CACHE.get_or_init(encoder_caps)
}

pub(crate) fn select_video_encoder(codec: VideoCodec) -> VideoEncoder {
    match codec {
        VideoCodec::Avc => caps().avc,
        VideoCodec::Hevc => caps().hevc,
        VideoCodec::Av1 => caps().av1,
    }
}

/// 导出界面要列出的编码. AVC 一定有. HEVC / AV1 要有探测通过的硬件编码器.
pub(crate) fn hardware_codecs() -> Vec<VideoCodec> {
    let caps = caps();
    let mut out = vec![VideoCodec::Avc];
    if caps.hevc.is_hardware() {
        out.push(VideoCodec::Hevc);
    }
    if caps.av1.is_hardware() {
        out.push(VideoCodec::Av1);
    }
    out
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
        let args = video_codec_args(VideoCodec::Avc, VideoEncoder::Software, 18);
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-crf", "18"]));
        assert!(args.iter().any(|a| a == "stillimage"));
        assert!(!args.iter().any(|a| a == "h264_qsv"));
    }

    #[test]
    fn qsv_uses_icq_not_crf() {
        let args = video_codec_args(VideoCodec::Avc, VideoEncoder::Qsv, 18);
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
                VideoEncoder::Software
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
                VideoEncoder::Software
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
            vec![VideoEncoder::VideoToolbox, VideoEncoder::Software]
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

    #[test]
    fn av1_skips_encoders_ffmpeg_71_does_not_have() {
        assert!(encoder_name(VideoCodec::Av1, VideoEncoder::MediaFoundation).is_none());
        assert!(encoder_name(VideoCodec::Av1, VideoEncoder::VideoToolbox).is_none());
        assert_eq!(
            encoder_name(VideoCodec::Hevc, VideoEncoder::Qsv),
            Some("hevc_qsv")
        );
        assert_eq!(
            encoder_name(VideoCodec::Av1, VideoEncoder::Nvenc),
            Some("av1_nvenc")
        );
    }

    #[test]
    fn hevc_qsv_keeps_the_same_quality_number() {
        let args = video_codec_args(VideoCodec::Hevc, VideoEncoder::Qsv, 18);
        assert!(args.windows(2).any(|w| w == ["-c:v", "hevc_qsv"]));
        assert!(args.windows(2).any(|w| w == ["-global_quality", "18"]));
    }
}
