//! 分块 P 图覆盖: 元数据 + 会话目录 + 蒙版 y 平移.

use std::collections::HashMap;
use std::path::PathBuf;

use image::RgbImage;
use photo_edit::{EditDocument, SourceFingerprint};

use super::*;

#[derive(Clone, Debug)]
pub struct RegionEditMeta {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub paper_rgb: [u8; 3],
    pub source: Option<SourceFingerprint>,
}

impl DocState {
    pub fn region_edit_dir(region_id: &str) -> PathBuf {
        crate::page_cache::session_dir()
            .join("edits")
            .join(region_id)
    }

    pub fn has_region_edit(&self, region_id: &str) -> bool {
        self.region_edits.contains_key(region_id)
            && Self::region_edit_dir(region_id).join("flat.png").is_file()
    }

    pub fn region_edit_size(&self, region_id: &str) -> Option<(u32, u32)> {
        self.region_edits
            .get(region_id)
            .map(|e| (e.canvas_w.max(1), e.canvas_h.max(1)))
    }

    pub fn write_region_edit(
        &mut self,
        region_id: &str,
        doc: &EditDocument,
    ) -> Result<RegionEditMeta, String> {
        let dir = Self::region_edit_dir(region_id);
        doc.save_to_dir(&dir)?;
        let meta = RegionEditMeta {
            canvas_w: doc.canvas_w.max(1),
            canvas_h: doc.canvas_h.max(1),
            paper_rgb: doc.paper_rgb,
            source: doc.source.clone(),
        };
        self.region_edits
            .insert(region_id.to_string(), meta.clone());
        Ok(meta)
    }

    pub fn remove_region_edit(&mut self, region_id: &str) {
        self.region_edits.remove(region_id);
        let dir = Self::region_edit_dir(region_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    pub fn load_region_flat(&self, region_id: &str) -> Option<RgbImage> {
        if !self.has_region_edit(region_id) {
            return None;
        }
        image::open(Self::region_edit_dir(region_id).join("flat.png"))
            .ok()
            .map(|i| i.to_rgb8())
    }

    pub fn load_region_document(&self, region_id: &str) -> Option<EditDocument> {
        if !self.has_region_edit(region_id) {
            return None;
        }
        EditDocument::load_from_dir(&Self::region_edit_dir(region_id)).ok()
    }

    pub fn prune_orphan_edits(&mut self) {
        let valid: std::collections::HashSet<String> = self
            .pages
            .iter()
            .flat_map(|p| p.regions.keys().cloned())
            .collect();
        let stale: Vec<String> = self
            .region_edits
            .keys()
            .filter(|k| !valid.contains(*k))
            .cloned()
            .collect();
        for rid in stale {
            self.remove_region_edit(&rid);
        }
    }
}

/// 块高变化后: 完全位于旧块下方的蒙版整体平移 `Δh`.
pub fn remap_masks_after_height_change(
    masks: &mut [MaskRect],
    block_y0: i64,
    old_h: i64,
    new_h: i64,
) {
    let dh = new_h - old_h;
    if dh == 0 {
        return;
    }
    let old_bottom = block_y0 + old_h;
    for m in masks {
        if (m.y0 as i64) >= old_bottom {
            m.translate(0, dh as i32);
        }
    }
}

pub fn groups_using_region(groups: &[Group], region_id: &str) -> Vec<String> {
    groups
        .iter()
        .filter(|g| g.region_ids.iter().any(|id| id == region_id))
        .map(|g| g.id.clone())
        .collect()
}

#[allow(dead_code)]
fn _keep_hashmap(_: HashMap<String, RegionEditMeta>) {}
