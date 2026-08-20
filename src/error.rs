//! 宿主用户可见错误. 弹窗用 `Display`, 不再让后台线程里的失败变成进程崩溃.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error(
        "找不到 {lib} — 请把它放在程序同目录下, 或加入系统 PATH, \
         也可设置环境变量 PDFIUM_DYNAMIC_LIB_PATH 指定路径."
    )]
    PdfiumMissing { lib: String },
    #[error("无法加载 pdfium ({}): {detail}", .path.display())]
    PdfiumLoad { path: PathBuf, detail: String },
    #[error("打开 PDF 失败: {0}")]
    PdfOpen(String),
    #[error("{0}")]
    Project(String),
    #[error("无法打开图片 ({}): {detail}", .path.display())]
    ImageOpen { path: PathBuf, detail: String },
    #[error("{0}")]
    Export(String),
}

impl Error {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }

    pub fn project(s: impl Into<String>) -> Self {
        Self::Project(s.into())
    }

    pub fn image_open(path: impl Into<PathBuf>, detail: impl std::fmt::Display) -> Self {
        Self::ImageOpen {
            path: path.into(),
            detail: detail.to_string(),
        }
    }

    pub fn export(s: impl Into<String>) -> Self {
        Self::Export(s.into())
    }
}
