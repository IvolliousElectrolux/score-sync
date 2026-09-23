//! 谱面分块 P 图库 (可独立运行, 也可嵌入 score_sync).

pub mod document;
pub mod geom;
pub mod gui;
pub mod ids;
pub mod process;

pub use document::{EditDocument, Layer, SourceFingerprint};
pub use gui::{Commit, FitHint, HostCmd, ImportOption, PhotoEditApp};
pub use ids::{is_image_path, new_id};
pub use process::{apply_edge_adjust, sample_paper};
