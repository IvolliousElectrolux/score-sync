//! 视频轨道编辑与导出 (可嵌入 score_sync 等宿主).
//!
//! 素材是「已合成好的静态谱面图」, 视频轨道只是给每张图分配一段时间, 配合
//! 音频轨道与转场轨道 (淡入淡出 / 刷入). 预览直接按播放头选当前图 + 叠加转场,
//! 无需真正的视频解码; 导出时用 ffmpeg 把静止图拼接编码并套用淡入淡出或刷入.

pub mod audio;
pub mod error;
pub mod export;
pub mod gui;
pub mod model;
