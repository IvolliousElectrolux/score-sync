//! 按组合裁切并竖向拼接导出 (含蒙版 + 可选工程底色层).
//!
//! 批量导出按组流水线: 加载成员页 → 合成写盘 → 由调用方 `retain_window` 释放.

use std::path::{Path, PathBuf};

use crate::model::DocState;

fn export_stem(doc: &DocState) -> String {
    if doc.pages.is_empty() {
        return "export".to_string();
    }
    let s = doc.pages[0]
        .path
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or("export")
        .to_string();
    if doc.pages.len() > 1 {
        format!("{s}_x{}", doc.pages.len())
    } else {
        s
    }
}

/// 同步导出全部组合 (供测试/脚本; GUI 走分块异步流水线).
#[allow(dead_code)]
pub fn export_groups(doc: &mut DocState, out_dir: &Path) -> Result<(usize, PathBuf), String> {
    if doc.groups.is_empty() {
        return Err("没有可导出的内容.".into());
    }
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let ids: Vec<String> = doc.groups.iter().map(|g| g.id.clone()).collect();
    let saved = export_groups_chunk(doc, out_dir, &ids, 0)?;
    Ok((saved, out_dir.to_path_buf()))
}

/// 导出一批组合. `start_index` 为已导出数量 (用于文件名序号延续).
/// 返回本批成功写出的数量.
pub fn export_groups_chunk(
    doc: &mut DocState,
    out_dir: &Path,
    group_ids: &[String],
    start_index: usize,
) -> Result<usize, String> {
    if group_ids.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let stem = export_stem(doc);
    let mut saved = 0usize;
    for (j, gid) in group_ids.iter().enumerate() {
        doc.ensure_group_pages(gid)?;
        let Some(combined) = doc.render_group_final(gid)? else {
            continue;
        };
        let name = format!("{stem}_g{:02}.png", start_index + j + 1);
        let path = out_dir.join(&name);
        combined
            .save(&path)
            .map_err(|e| format!("保存失败 {}: {e}", path.display()))?;
        saved += 1;
    }
    Ok(saved)
}
