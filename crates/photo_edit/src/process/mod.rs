//! 纯像素操作, 与 GUI 解耦.

mod brush;
mod canvas;
mod clone_stamp;
mod heal;
mod select;
mod transform;

pub use brush::{erase_stamp, stamp_color};
pub use canvas::*;
pub use clone_stamp::*;
pub use heal::*;
pub use select::*;
pub use transform::*;
