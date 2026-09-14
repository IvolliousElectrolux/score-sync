//! 辅助线默认值与谱表锚点.

use super::*;
use std::collections::HashSet;

impl DocState {
    pub fn get_group_guides(&self, group_id: &str) -> GuideState {
        self.group_guides.get(group_id).cloned().unwrap_or_default()
    }

    pub fn set_group_guides(&mut self, group_id: &str, guides: GuideState) {
        if guides.is_default() {
            self.group_guides.remove(group_id);
        } else {
            self.group_guides.insert(group_id.to_string(), guides);
        }
    }
    /// 按组内五线谱块数 (有预计算锚点则只数认得出谱表的; 否则退回总块数)
    /// 和预览画布高生成默认辅助线, 不读像素. 一根时落在两端中点 (比页心
    /// 略偏下). 文字/脚注默认不占线, 需手动加根数才纳入对齐.
    pub fn compute_default_guides(&self, group_id: &str) -> GuideState {
        let n = self.group_staff_block_count(group_id);
        let h = self
            .group_preview_frame(group_id)
            .map(|f| f.canvas_h as i32)
            .unwrap_or(0);
        let mut g = GuideState::default();
        g.set_staff_slots(n, h);
        g
    }

    fn group_staff_block_count(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 0;
        };
        let known = g
            .region_ids
            .iter()
            .filter(|id| self.region_staff_anchors.contains_key(*id))
            .count();
        if known == 0 {
            return g.region_ids.len() as u32;
        }
        g.region_ids
            .iter()
            .filter(|id| matches!(self.region_staff_anchors.get(*id), Some(Some(_))))
            .count() as u32
    }

    /// 为指定组合写入默认辅助线. `guides_global` 时若该组还没有显示用的
    /// 线, 一并拷过去.
    pub fn seed_guide_defaults_for(&mut self, gids: &[String]) {
        for gid in gids {
            let d = self.compute_default_guides(gid);
            if d.lines.is_empty() {
                self.group_guide_defaults.remove(gid);
            } else {
                self.group_guide_defaults.insert(gid.clone(), d.clone());
            }
            if self.guides_global
                && self.get_group_guides(gid).lines.is_empty()
                && !d.lines.is_empty()
            {
                self.set_group_guides(gid, d);
            }
        }
    }

    /// 刷新全部组合的默认辅助线 (建组/改底色后).
    pub fn seed_guide_defaults(&mut self) {
        let valid: HashSet<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        self.group_guide_defaults.retain(|k, _| valid.contains(k));
        let gids: Vec<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        self.seed_guide_defaults_for(&gids);
    }

    pub fn ingest_region_staff_anchors(&mut self, items: impl IntoIterator<Item = (String, Option<i32>)>) {
        for (id, y) in items {
            self.region_staff_anchors.insert(id, y);
        }
    }

    /// 当前页图已在内存时, 给尚未预算的分块补谱表锚点 (不读磁盘).
    pub fn seed_region_anchors_for_page(&mut self, page_idx: usize) {
        let Some(page) = self.pages.get(page_idx) else {
            return;
        };
        let Some(img) = page.image.as_ref() else {
            return;
        };
        let orig_h = page.img_h.max(1);
        let proxy_h = img.height().max(1);
        let thr = self.ink_threshold;
        let bands: Vec<(String, i32, i32)> = page
            .regions
            .values()
            .filter(|r| !self.region_staff_anchors.contains_key(&r.id))
            .map(|r| (r.id.clone(), r.y0, r.y1))
            .collect();
        if bands.is_empty() {
            return;
        }
        let computed: Vec<(String, Option<i32>)> = bands
            .into_iter()
            .map(|(id, y0, y1)| {
                let orig_band = (y1 - y0 + 1).max(1) as u32;
                let (py0, ph) = crate::page_cache::map_band_to_proxy(
                    y0.max(0) as u32,
                    orig_band,
                    orig_h,
                    proxy_h,
                );
                let py1 = py0.saturating_add(ph.saturating_sub(1)) as i32;
                let a = mask_tool::staff::band_staff_anchor(img, py0 as i32, py1, thr);
                let a = a.map(|v| {
                    if ph <= 1 || orig_h == proxy_h {
                        v
                    } else {
                        (v as i64 * orig_band as i64 / ph as i64) as i32
                    }
                });
                (id, a)
            })
            .collect();
        self.ingest_region_staff_anchors(computed);
    }

    /// 全局开启: 缺线的组合用预计算默认值填上. 不读页图.
    pub fn apply_guides_global_on(&mut self) {
        self.guides_global = true;
        self.seed_guide_defaults();
    }

    /// 全局关闭: 清掉显示用的线, 默认值保留以便再开.
    pub fn apply_guides_global_off(&mut self) {
        self.guides_global = false;
        self.group_guides.clear();
    }
}
