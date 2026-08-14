//! 列表行与标签栏辅助渲染数据.

use gpui::SharedString;

#[derive(Clone)]
pub struct ListRow {
    pub id: String,
    pub label: SharedString,
    pub color: u32,
    pub selected: bool,
    /// 在源列表中的下标 (输出组合为 `doc.groups` 下标).
    pub src_index: usize,
}

#[derive(Clone)]
pub struct TabInfo {
    pub index: usize,
    pub label: SharedString,
    pub active: bool,
}
