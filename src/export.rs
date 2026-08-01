//! 按组合裁切并竖向拼接导出 (含组合拼合图上的蒙版).

use std::path::{Path, PathBuf};

use mask_tool::mask::apply_masks_rgb;

use crate::model::DocState;

pub fn export_groups(doc: &DocState, out_dir: &Path) -> Result<(usize, PathBuf), String> {
    if doc.groups.is_empty() {
        return Err("没有可导出的内容.".into());
    }
    let stem = if doc.pages.is_empty() {
        "export".to_string()
    } else {
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
    };

    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let mut saved = 0usize;
    for (i, g) in doc.groups.iter().enumerate() {
        let Some(mut combined) = doc.compose_group(&g.id) else {
            continue;
        };
        // 蒙版坐标相对本组拼合图; 各组合独立 (共享脚注可在不同组画不同蒙版)
        let masks = doc.get_group_masks(&g.id);
        if !masks.is_empty() {
            apply_masks_rgb(&mut combined, masks, doc.mask_opacity);
        }
        let name = format!("{stem}_g{:02}.png", i + 1);
        let path = out_dir.join(&name);
        combined
            .save(&path)
            .map_err(|e| format!("保存失败 {}: {e}", path.display()))?;
        saved += 1;
    }
    Ok((saved, out_dir.to_path_buf()))
}
