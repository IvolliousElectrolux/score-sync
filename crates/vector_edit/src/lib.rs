//! 谱面分块矢量修图 (可独立运行, 也可嵌入 score_sync).

pub mod document;
pub mod geom;
pub mod gui;
pub mod ids;
pub mod import;
mod outline;
pub mod raster;
pub mod svg;

pub use document::{
    sniff_edit_kind, FillRule, ImageObj, Layer, LineCap, LineJoin, Obj, ObjKind, Paint, PathObj,
    Seg, SourceFingerprint, Stroke, Subpath, TextObj, VecDocument, KIND_VECTOR,
};
pub use gui::{bind_embedded_keys, run_gui, Commit, FitHint, HostCmd, ImportOption, VectorEditApp};
pub use ids::new_id;
pub use import::{import_band, import_pdf_page, ImportReport, ImportRequest};
pub use raster::{rasterize_rect, rasterize_rgba, rasterize_rgba_size};
