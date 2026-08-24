//! 「组合分块」拖动调整: 载入原始片段, 几何/命中测试, 快速重拼.

use super::*;
use crate::layout;

/// 命中「组合分块」的哪个区域.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockHitZone {
    Top,
    Bottom,
    Body,
}

impl MaskToolApp {
    /// 载入组合内各块的原始裁切片段 (未应用调整) 与已有的位置/尺寸微调,
    /// 供「移动分块」模式使用. 应在 `load_rgb` 之后调用 (不影响当前显示
    /// 的、可能含底色合成的预览图, 只有真正拖动块时才会切到本地重拼图).
    pub fn set_block_pieces(
        &mut self,
        pieces: Vec<(String, image::RgbImage)>,
        layout: Vec<BlockAdjust>,
        ink_threshold: i32,
    ) {
        self.block_pieces = pieces;
        self.block_layout = layout;
        self.block_ink_threshold = ink_threshold;
        if let Some(sel) = self.block_selected.clone() {
            if !self.block_pieces.iter().any(|(id, _)| *id == sel) {
                self.block_selected = None;
            }
        }
    }

    pub fn block_layout_clone(&self) -> Vec<BlockAdjust> {
        self.block_layout.clone()
    }

    pub fn has_block_pieces(&self) -> bool {
        !self.block_pieces.is_empty()
    }

    pub fn selected_block_id(&self) -> Option<&str> {
        self.block_selected.as_deref()
    }

    pub fn select_block(&mut self, region_id: Option<String>, cx: &mut Context<Self>) {
        self.block_selected = region_id;
        cx.notify();
    }

    /// 组内各块在当前 `block_layout` 下的拼合图纵向范围.
    pub(super) fn block_spans(&self) -> Vec<(String, i64, i64)> {
        let heights: Vec<(String, u32)> = self
            .block_pieces
            .iter()
            .map(|(id, img)| (id.clone(), img.height()))
            .collect();
        layout::compute_spans(&heights, &self.block_layout)
    }

    fn block_orig_height(&self, region_id: &str) -> Option<u32> {
        self.block_pieces
            .iter()
            .find(|(id, _)| id == region_id)
            .map(|(_, img)| img.height())
    }

    fn block_index(&self, region_id: &str) -> Option<usize> {
        self.block_pieces.iter().position(|(id, _)| id == region_id)
    }

    fn ensure_layout_entry(&mut self, region_id: &str) -> usize {
        if let Some(i) = self.block_layout.iter().position(|a| a.region_id == region_id) {
            return i;
        }
        self.block_layout.push(BlockAdjust {
            region_id: region_id.to_string(),
            ..Default::default()
        });
        self.block_layout.len() - 1
    }

    /// 「移动分块」模式命中测试: 在拼合图 y=`iy` (图像坐标) 处, 找最靠近的
    /// 块上/下边界 (容差 `tol`, 图像像素) 或落在哪个块本体内.
    pub(super) fn hit_block_at(&self, iy: f32, tol: f32) -> Option<(String, BlockHitZone)> {
        let spans = self.block_spans();
        if spans.is_empty() {
            return None;
        }
        let mut best: Option<(String, BlockHitZone, f32)> = None;
        for (rid, y0, y1) in &spans {
            let d_top = (iy - *y0 as f32).abs();
            let d_bot = (iy - *y1 as f32).abs();
            if d_top <= tol && best.as_ref().map(|(_, _, d)| d_top < *d).unwrap_or(true) {
                best = Some((rid.clone(), BlockHitZone::Top, d_top));
            }
            if d_bot <= tol && best.as_ref().map(|(_, _, d)| d_bot < *d).unwrap_or(true) {
                best = Some((rid.clone(), BlockHitZone::Bottom, d_bot));
            }
        }
        if let Some((rid, zone, _)) = best {
            return Some((rid, zone));
        }
        spans
            .iter()
            .find(|(_, y0, y1)| (*y0 as f32) <= iy && iy <= (*y1 as f32))
            .map(|(rid, ..)| (rid.clone(), BlockHitZone::Body))
    }

    /// 开始拖动块本体 (整体上下移动).
    pub(super) fn begin_block_move(&mut self, region_id: String, iy: f32) {
        let idx = self.ensure_layout_entry(&region_id);
        let start_gap_before = self.block_layout[idx].gap_before;
        self.block_selected = Some(region_id.clone());
        self.drag = Some(DragKind::BlockMove {
            region_id,
            start_iy: iy,
            start_gap_before,
        });
    }

    pub(super) fn begin_block_resize_top(&mut self, region_id: String, iy: f32) {
        let Some(orig_h) = self.block_orig_height(&region_id) else {
            return;
        };
        let idx = self.ensure_layout_entry(&region_id);
        let start_extra_top = self.block_layout[idx].extra_top;
        self.block_selected = Some(region_id.clone());
        self.drag = Some(DragKind::BlockResizeTop {
            region_id,
            start_iy: iy,
            start_extra_top,
            max_trim: (orig_h as i32 - 1).max(0),
        });
    }

    pub(super) fn begin_block_resize_bottom(&mut self, region_id: String, iy: f32) {
        let Some(orig_h) = self.block_orig_height(&region_id) else {
            return;
        };
        let idx = self.ensure_layout_entry(&region_id);
        let start_extra_bottom = self.block_layout[idx].extra_bottom;
        self.block_selected = Some(region_id.clone());
        self.drag = Some(DragKind::BlockResizeBottom {
            region_id,
            start_iy: iy,
            start_extra_bottom,
            max_trim: (orig_h as i32 - 1).max(0),
        });
    }

    pub(super) fn apply_block_move(&mut self, region_id: &str, start_iy: f32, start_gap_before: i32, iy: f32) {
        let Some(idx) = self.block_index(region_id) else {
            return;
        };
        let li = self.ensure_layout_entry(region_id);
        let delta = (iy - start_iy).round() as i32;
        self.block_layout[li].gap_before = (start_gap_before + delta).max(0);
        let _ = idx;
        self.rebuild_block_composite();
    }

    pub(super) fn apply_block_resize_top(
        &mut self,
        region_id: &str,
        start_iy: f32,
        start_extra_top: i32,
        max_trim: i32,
        iy: f32,
    ) {
        let li = self.ensure_layout_entry(region_id);
        // 往上拖 (iy 变小) 扩展 (extra_top 变大); 往下拖裁剪 (变负, 最多裁到剩 1px).
        let delta = (start_iy - iy).round() as i32;
        let new_val = (start_extra_top + delta).max(-max_trim);
        self.block_layout[li].extra_top = new_val;
        self.rebuild_block_composite();
    }

    pub(super) fn apply_block_resize_bottom(
        &mut self,
        region_id: &str,
        start_iy: f32,
        start_extra_bottom: i32,
        max_trim: i32,
        iy: f32,
    ) {
        let li = self.ensure_layout_entry(region_id);
        let delta = (iy - start_iy).round() as i32;
        let new_val = (start_extra_bottom + delta).max(-max_trim);
        self.block_layout[li].extra_bottom = new_val;
        self.rebuild_block_composite();
    }

    /// 用当前 `block_layout` 重新拼接 (0 卡顿目标: 只是像素搬运 + 边缘背景
    /// 合成, 不涉及磁盘/网络 IO), 更新画布显示; 不动历史/选中/缩放状态.
    pub(super) fn rebuild_block_composite(&mut self) {
        if self.block_pieces.is_empty() {
            return;
        }
        let combined = layout::stitch_with_layout(
            &self.block_pieces,
            &self.block_layout,
            self.block_ink_threshold,
        );
        let (w, h) = combined.dimensions();
        let mut rgba: RgbaImage = ImageBuffer::from_fn(w, h, |x, y| {
            let p = combined.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        self.render_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
        self.rgb_image = Some(combined);
        self.img_w = w;
        self.img_h = h;
    }
}
