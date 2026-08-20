//! 视频/音频用户可见错误. 解码库内部 panic 由调用方 `catch_unwind` 接住后转成这些类型.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("找不到音频文件:\n{}", .0.display())]
    AudioMissing(PathBuf),
    #[error("无法读取音频时长 ({}):\n{detail}", .path.display())]
    AudioProbe { path: PathBuf, detail: String },
    #[error("无法播放音频 ({}):\n{detail}", .path.display())]
    AudioPlay { path: PathBuf, detail: String },
    #[error("启动 ffmpeg 失败 ({step}): {source}\n请确认程序同目录下有 ffmpeg, 或已加入系统 PATH.")]
    FfmpegSpawn {
        step: String,
        #[source]
        source: std::io::Error,
    },
    #[error("ffmpeg 执行失败 ({step}):\n{detail}")]
    Ffmpeg { step: String, detail: String },
    #[error("{0}")]
    Export(String),
}

impl Error {
    pub fn audio_probe(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self::AudioProbe {
            path: path.into(),
            detail: detail.into(),
        }
    }

    pub fn export(msg: impl Into<String>) -> Self {
        Self::Export(msg.into())
    }
}
