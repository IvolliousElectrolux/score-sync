//! 组合拼合、预览与终稿渲染.

use super::*;
use image::RgbImage;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// `DocState::compose_group_impl` 的 `&self` 无关核心: 按 `layout` 竖向拼合
/// 已裁切好的成员片段. 拆出来是为了让 [`GroupRenderJob::render`] 能在不
/// 持有 `&DocState` 的情况下 (例如后台线程) 复用同一套拼接逻辑.
///
/// 统一走 [`mask_tool::layout::stitch_with_stats`] 的整行 `copy_from_slice`
/// 快路径, 不再对"无布局微调"单独维护一份 `image::imageops::replace`
/// 慢路径 (内部逐像素 get_pixel/put_pixel, 高清页多块组合这样拼一次
/// 开销很可观——这正是切到蒙版/底色面板时卡顿的根因之一, 见
/// `score_sync::gui::sync::sync_mask_image` 文档). `layout` 为空时各块
/// 间距/扩展均为 0, `PieceStats` 不会被实际读取, 用零成本的占位统计即可,
/// 不必为此扫描像素.
pub(crate) fn compose_parts_impl(
    parts: &[(String, image::RgbImage)],
    layout: &[BlockAdjust],
    ink_threshold: i32,
    stats: Option<&std::collections::HashMap<String, mask_tool::layout::PieceStats>>,
) -> Option<image::RgbImage> {
    if parts.is_empty() {
        return None;
    }
    if layout.is_empty() && parts.len() == 1 {
        let max_w = parts.iter().map(|(_, p)| p.width()).max().unwrap_or(1);
        if parts[0].1.width() == max_w {
            return Some(parts[0].1.clone());
        }
    }
    let piece_stats: Vec<mask_tool::layout::PieceStats> = parts
        .iter()
        .map(|(rid, img)| {
            stats
                .and_then(|cache| cache.get(rid).copied())
                .unwrap_or_else(|| {
                    if layout.is_empty() {
                        mask_tool::layout::PieceStats::default()
                    } else {
                        mask_tool::layout::compute_piece_stats(img, ink_threshold)
                    }
                })
        })
        .collect();
    Some(mask_tool::layout::stitch_with_stats(parts, &piece_stats, layout))
}

/// 底色合成所需的快照 (见 [`GroupRenderJob`]).
struct GroupRenderBg {
    image: Option<Arc<RgbImage>>,
    solid: Option<[u8; 3]>,
    src_w: u32,
    src_h: u32,
    aspect_w: u32,
    aspect_h: u32,
    voff_shift: i64,
    leading_gap: u32,
    trailing_gap: u32,
}

/// 终稿一条成员: 优先在 [`GroupRenderJob::render`] 里按磁盘原图裁切,
/// 避免界面线程常驻全分辨率页. 单测没有可用 PNG 时带 `inline`.
struct GroupRenderPart {
    rid: String,
    disk_path: PathBuf,
    y0: u32,
    height: u32,
    inline: Option<RgbImage>,
}

/// `DocState::render_group_final` 所需只读数据的快照, 由
/// [`DocState::prepare_group_render_job`] 在主线程收集路径/条带范围
/// (不解码原图); [`Self::render`] 放到后台按磁盘全分辨率裁切 + 拼合 +
/// 蒙版 + 底色. 视频素材池批量重渲染见
/// `score_sync::gui::sync::sync_video_pool`.
pub struct GroupRenderJob {
    members: Vec<GroupRenderPart>,
    block_layout: Vec<BlockAdjust>,
    ink_threshold: i32,
    masks: Vec<MaskRect>,
    mask_opacity: f32,
    content_scale: f32,
    sheet_w: u32,
    bg_enabled: bool,
    bg: Option<GroupRenderBg>,
}

impl GroupRenderJob {
    fn collect_full_parts(&self) -> Result<Vec<(String, RgbImage)>, String> {
        let mut cache: HashMap<PathBuf, RgbImage> = HashMap::new();
        let mut parts = Vec::with_capacity(self.members.len());
        for m in &self.members {
            if let Some(img) = &m.inline {
                parts.push((m.rid.clone(), img.clone()));
                continue;
            }
            if !m.disk_path.is_file() {
                return Err(format!("缺页图 {}", m.disk_path.display()));
            }
            if !cache.contains_key(&m.disk_path) {
                cache.insert(
                    m.disk_path.clone(),
                    crate::page_cache::load_rgb(&m.disk_path)?,
                );
            }
            let full = cache.get(&m.disk_path).unwrap();
            let y0 = m.y0.min(full.height().saturating_sub(1));
            let y1 = (m.y0 + m.height).min(full.height());
            if y1 <= y0 {
                continue;
            }
            parts.push((m.rid.clone(), crop_band_fast(full, y0, y1 - y0)));
        }
        if parts.is_empty() {
            return Err("无成员片段".into());
        }
        Ok(parts)
    }

    /// 纯计算, 不接触 `DocState`, 可安全放到非主线程跑.
    pub fn render(&self) -> Result<RgbImage, String> {
        let parts = self.collect_full_parts()?;
        let mut combined =
            compose_parts_impl(&parts, &self.block_layout, self.ink_threshold, None)
                .ok_or_else(|| "无成员片段".to_string())?;
        drop(parts);
        if !self.masks.is_empty() {
            mask_tool::mask::apply_masks_to_sheet(
                &mut combined,
                &self.masks,
                self.content_scale,
                self.mask_opacity,
            );
        }
        if self.bg_enabled {
            if let Some(bg) = &self.bg {
                let composed = if let Some(color) = bg.solid {
                    apply_bg::process::composite_solid(
                        &combined,
                        color,
                        bg.src_w,
                        bg.src_h,
                        bg.aspect_w,
                        bg.aspect_h,
                        bg.voff_shift,
                        bg.leading_gap,
                        bg.trailing_gap,
                    )
                } else if let Some(img) = bg.image.as_ref() {
                    apply_bg::process::composite_and_crop(
                        &combined,
                        img,
                        bg.aspect_w,
                        bg.aspect_h,
                        bg.voff_shift,
                        bg.leading_gap,
                        bg.trailing_gap,
                    )
                } else {
                    Ok(combined.clone())
                };
                match composed {
                    Ok(c) => combined = c,
                    Err(e) => {
                        crate::trace::log(&format!(
                            "GroupRenderJob: 底色合成失败, 用纯谱面: {e}"
                        ));
                    }
                }
            }
        }
        Ok(combined)
    }

    /// 把完整底色换成已按最大谱面宽裁好的一页, 避免每个组合都对超宽扫描图做跨行拷贝.
    /// 纯色底不改 (没有扫描图可裁).
    pub(crate) fn attach_cropped_bg(&mut self, page: Arc<RgbImage>) {
        if let Some(bg) = &mut self.bg {
            if bg.image.is_some() {
                bg.src_w = page.width();
                bg.src_h = page.height();
                bg.image = Some(page);
            }
        }
    }

    pub(crate) fn bg_full_image(&self) -> Option<Arc<RgbImage>> {
        self.bg.as_ref().and_then(|b| b.image.clone())
    }

    pub(crate) fn bg_aspect(&self) -> Option<(u32, u32)> {
        self.bg.as_ref().map(|b| (b.aspect_w, b.aspect_h))
    }

    pub(crate) fn sheet_w(&self) -> u32 {
        self.sheet_w
    }
}

impl DocState {
    /// 组内各成员原始高度 (region y0/y1, 不需要像素).
    pub fn group_member_heights(&self, group_id: &str) -> Vec<(String, u32)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let (_, r) = self.find_region(rid)?;
                Some((rid.clone(), (r.y1 - r.y0 + 1).max(0) as u32))
            })
            .collect()
    }

    /// 拼合图宽: 取成员所在页宽的最大值.
    pub fn group_sheet_width(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 1;
        };
        g.region_ids
            .iter()
            .filter_map(|rid| {
                let (pi, _) = self.find_region(rid)?;
                Some(self.pages.get(pi)?.width().max(1))
            })
            .max()
            .unwrap_or(1)
    }

    /// 预览画布几何 (不含像素). 无底色时画布就是拼合图.
    pub fn group_preview_frame(&self, group_id: &str) -> Option<apply_bg::process::PreviewFrame> {
        let heights = self.group_member_heights(group_id);
        if heights.is_empty() {
            return None;
        }
        let sw = self.group_sheet_width(group_id);
        let sh = mask_tool::layout::sheet_height(&heights, self.get_block_layout(group_id));
        if !self.bg_enabled {
            return Some(apply_bg::process::PreviewFrame {
                canvas_w: sw,
                canvas_h: sh,
                hoff: 0,
                voff: 0,
                bg_left: 0,
                bg_top: 0,
                shows_bg: false,
                content_scale: 1.0,
            });
        }
        let (bw, bh) = self.bg_src_size()?;
        Some(apply_bg::process::preview_frame(
            sw,
            sh,
            bw,
            bh,
            self.bg_aspect_w,
            self.bg_aspect_h,
            self.get_group_voff_shift(group_id),
        ))
    }
    /// 裁切某页上的区域条带 (整宽). 走内存里的显示代理, 坐标按原图映射.
    /// 终稿请用 [`Self::prepare_group_render_job`] (磁盘原图).
    pub fn crop_region(&self, region_id: &str) -> Option<image::RgbImage> {
        let (pi, r) = self.find_region(region_id)?;
        let page = self.pages.get(pi)?;
        let img = page.image.as_ref()?;
        let y0 = r.y0.max(0) as u32;
        let y1 = (r.y1 as u32).min(page.height().saturating_sub(1));
        if y1 < y0 {
            return None;
        }
        let (py0, ph) = crate::page_cache::map_band_to_proxy(
            y0,
            y1 - y0 + 1,
            page.img_h.max(1),
            img.height().max(1),
        );
        Some(crop_band_fast(img, py0, ph))
    }

    /// 组内各成员的原始裁切片段 (未应用 `group_block_layout` 微调), 与
    /// `region_ids` 顺序一致; 调用前须 `ensure_group_pages`. 供蒙版编辑
    /// 里拖动「组合分块」使用: 已在内存中, 不必每帧回读磁盘, 只是重新
    /// 拼接 (含底色合成) 交回蒙版画布显示.
    pub fn group_member_pieces(&self, group_id: &str) -> Vec<(String, image::RgbImage)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        g.region_ids
            .iter()
            .filter_map(|rid| self.crop_region(rid).map(|img| (rid.clone(), img)))
            .collect()
    }

    /// 按组内成员顺序竖向拼合 (与导出一致, 不含蒙版). 若该组存在蒙版编辑时
    /// 的分块位置/尺寸微调 (`group_block_layout`), 在此一并应用; 否则走原
    /// 有的纯拼接快速路径 (性能/结果与旧版本完全一致). 导出终稿走带底色/
    /// 蒙版合成的 `render_group_final`, 这个不缓存统计的简单版本目前只在
    /// 测试里直接练到 `compose_parts_impl`, 保留作为公开的轻量入口.
    #[allow(dead_code)]
    pub fn compose_group(&self, group_id: &str) -> Option<image::RgbImage> {
        let parts = self.group_member_pieces(group_id);
        self.compose_group_impl(group_id, &parts, None)
    }

    /// 同 `compose_group`, 但各块裁切片段与背景色统计都由调用方预先准备.
    /// 预览已改为三层贴图, 这条路径留给测试 / 需要整图像素的调用方.
    #[allow(dead_code)]
    pub fn compose_group_with_parts_and_stats(
        &self,
        group_id: &str,
        parts: &[(String, image::RgbImage)],
        stats: &std::collections::HashMap<String, mask_tool::layout::PieceStats>,
    ) -> Option<image::RgbImage> {
        self.compose_group_impl(group_id, parts, Some(stats))
    }

    fn compose_group_impl(
        &self,
        group_id: &str,
        parts: &[(String, image::RgbImage)],
        stats: Option<&std::collections::HashMap<String, mask_tool::layout::PieceStats>>,
    ) -> Option<image::RgbImage> {
        compose_parts_impl(parts, self.get_block_layout(group_id), self.ink_threshold, stats)
    }

    /// 组合最前面那个块自己的 `gap_before` (人为拖动第一块腾出的、没有
    /// 真实内容的顶端留白). 启用底色层时合成阶段要跳过贴这一段, 让底色
    /// 直接透出来, 见 [`Self::compose_group_preview_from`] 与
    /// [`Self::render_group_final`].
    pub fn group_leading_gap(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 0;
        };
        let Some(first_rid) = g.region_ids.first() else {
            return 0;
        };
        mask_tool::layout::BlockAdjust::find(self.get_block_layout(group_id), first_rid)
            .map(|a| a.gap_before.max(0) as u32)
            .unwrap_or(0)
    }

    /// 组合最后一块后面的末端留白 (旧版拖过页边后缩小内部块用;
    /// 现已改为碰到页顶/页底即停, 此字段多为工程兼容).
    pub fn group_trailing_gap(&self, group_id: &str) -> u32 {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return 0;
        };
        let Some(last_rid) = g.region_ids.last() else {
            return 0;
        };
        mask_tool::layout::BlockAdjust::find(self.get_block_layout(group_id), last_rid)
            .map(|a| a.gap_after.max(0) as u32)
            .unwrap_or(0)
    }

    /// 组合内各块在拼合图中的纵向范围 (`(region_id, comp_y0, comp_y1)`),
    /// 已应用 `group_block_layout` 微调; 供「组合分块」列表/蒙版画布使用.
    pub fn group_member_spans(&self, group_id: &str) -> Vec<(String, i64, i64)> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        let heights: Vec<(String, u32)> = g
            .region_ids
            .iter()
            .filter_map(|rid| {
                let (_, r) = self.find_region(rid)?;
                Some((rid.clone(), (r.y1 - r.y0 + 1).max(0) as u32))
            })
            .collect();
        mask_tool::layout::compute_spans(&heights, self.get_block_layout(group_id))
    }

    /// 拼合图预览 (供蒙版/视频面板显示): 若已启用工程底色, 叠加底色预览
    /// (contain: 上下或左右补边, 不烧入蒙版). 返回 (预览图, 谱面在预览图
    /// 中的横向/纵向偏移, 供调用方换算蒙版坐标). GUI 侧统一走下面
    /// 复用裁切片段/统计缓存的 `..._with_parts_and_stats`, 这个简单版本
    /// 保留作为公开的轻量入口 (含测试覆盖).
    #[allow(dead_code)]
    pub fn compose_group_preview(&self, group_id: &str) -> Option<(RgbImage, i64, i64)> {
        self.compose_group_preview_from(group_id, self.compose_group(group_id))
    }

    /// 同 `compose_group_preview`, 但拼合图用调用方预先缓存好的裁切片段.
    #[allow(dead_code)]
    pub fn compose_group_preview_with_parts_and_stats(
        &self,
        group_id: &str,
        parts: &[(String, image::RgbImage)],
        stats: &std::collections::HashMap<String, mask_tool::layout::PieceStats>,
    ) -> Option<(RgbImage, i64, i64)> {
        self.compose_group_preview_from(
            group_id,
            self.compose_group_with_parts_and_stats(group_id, parts, stats),
        )
    }

    fn compose_group_preview_from(
        &self,
        group_id: &str,
        sheet: Option<RgbImage>,
    ) -> Option<(RgbImage, i64, i64)> {
        let sheet = sheet?;
        if !self.bg_enabled {
            return Some((sheet, 0, 0));
        }
        let voff_shift = self.get_group_voff_shift(group_id);
        let top_transparent = self.group_leading_gap(group_id);
        let bottom_transparent = self.group_trailing_gap(group_id);
        if let Some(color) = self.bg_solid {
            let Some((bw, bh)) = self.bg_src_size() else {
                return Some((sheet, 0, 0));
            };
            return match apply_bg::process::composite_preview_solid(
                &sheet,
                color,
                bw,
                bh,
                self.bg_aspect_w,
                self.bg_aspect_h,
                voff_shift,
                top_transparent,
                bottom_transparent,
            ) {
                Ok((canvas, hoff, voff)) => Some((canvas, hoff, voff)),
                Err(_) => Some((sheet, 0, 0)),
            };
        }
        let Some(bg) = self.bg_image.as_ref() else {
            return Some((sheet, 0, 0));
        };
        match apply_bg::process::composite_preview(
            &sheet,
            bg,
            self.bg_aspect_w,
            self.bg_aspect_h,
            voff_shift,
            top_transparent,
            bottom_transparent,
        ) {
            Ok((canvas, hoff, voff)) => Some((canvas, hoff, voff)),
            Err(_) => Some((sheet, 0, 0)),
        }
    }

    /// 终稿合成: 拼合 → 蒙版 → (可选) 底色底层裁切.
    ///
    /// 蒙版存在「预览画布 − hoff/voff」坐标系. 谱面高于页面被缩小装进画布时,
    /// 先除以 `content_scale` 映回未缩放拼合图再盖上, 然后按原图等比合成,
    /// 避免视频里遮盖偏移, 也不把谱面拉进画布坐标系里变形.
    pub fn render_group_final(&self, group_id: &str) -> Result<Option<RgbImage>, String> {
        match self.prepare_group_render_job(group_id) {
            Some(job) => job.render().map(Some),
            None => Ok(None),
        }
    }

    /// 见 [`GroupRenderJob`] 文档: 主线程只收集磁盘路径与条带范围, 不解码
    /// 原图. 单测若页图只在内存且已是全分辨率, 会把裁切条带放进 `inline`.
    pub fn prepare_group_render_job(&self, group_id: &str) -> Option<GroupRenderJob> {
        let g = self.groups.iter().find(|g| g.id == group_id)?;
        let mut members = Vec::new();
        for rid in &g.region_ids {
            let (pi, r) = self.find_region(rid)?;
            let page = self.pages.get(pi)?;
            let y0 = r.y0.max(0) as u32;
            let y1 = (r.y1 as u32).min(page.height().saturating_sub(1));
            if y1 < y0 {
                continue;
            }
            let height = y1 - y0 + 1;
            let inline = if page.is_full_res() {
                page.image
                    .as_ref()
                    .map(|img| crop_band_fast(img, y0, height))
            } else {
                None
            };
            if inline.is_none() && !page.disk_path.is_file() {
                continue;
            }
            members.push(GroupRenderPart {
                rid: rid.clone(),
                disk_path: page.disk_path.clone(),
                y0,
                height,
                inline,
            });
        }
        if members.is_empty() {
            return None;
        }
        let block_layout = self.get_block_layout(group_id).to_vec();
        let masks = self.get_group_masks(group_id).to_vec();
        let content_scale = self
            .group_preview_frame(group_id)
            .map(|f| f.content_scale)
            .unwrap_or(1.0);
        let bg = self.bg_src_size().map(|(src_w, src_h)| GroupRenderBg {
            image: self.bg_image.clone(),
            solid: self.bg_solid,
            src_w,
            src_h,
            aspect_w: self.bg_aspect_w,
            aspect_h: self.bg_aspect_h,
            voff_shift: self.get_group_voff_shift(group_id),
            leading_gap: self.group_leading_gap(group_id),
            trailing_gap: self.group_trailing_gap(group_id),
        });
        Some(GroupRenderJob {
            members,
            block_layout,
            ink_threshold: self.ink_threshold,
            masks,
            mask_opacity: self.mask_prefs.mask_opacity,
            content_scale,
            sheet_w: self.group_sheet_width(group_id),
            bg_enabled: self.bg_enabled,
            bg,
        })
    }
}
