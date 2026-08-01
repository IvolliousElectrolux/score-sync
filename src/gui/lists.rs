//! 列表行与标签栏辅助渲染数据.

use gpui::SharedString;

#[derive(Clone)]
pub struct ListRow {
    pub id: String,
    pub label: SharedString,
    pub color: u32,
    pub selected: bool,
}

#[derive(Clone)]
pub struct TabInfo {
    pub index: usize,
    pub label: SharedString,
    pub active: bool,
}
