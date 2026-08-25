//! 「组合分块」拖动调整: 几何/命中测试. 实际重新拼接 (含底色合成) 由
//! 宿主负责 (`compose_group_preview` 之类既有逻辑), 这里只算位置/尺寸,
//! 换算成命中测试与叠加线用的坐标. 拖动中途只改 `block_layout` 并
//! `cx.notify()`, 画面用加载时上传好的分块 GPU 贴图按新位置绘制 (不再
//! 每帧整图重拼/重新上传); 松手后宿主 observe 到变化, 用
//! [`Self::update_base_image`] 换回含底色合成的整图, 蒙版编辑继续显示
//! 那份最终预览.

use super::*;
use crate::layout;

/// 命中「组合分块」的哪个区域.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockHitZone {
    Top,
    Bottom,
    Body,
}

/// 单个分块的 GPU 贴图 (原始裁切, 未应用 layout), 拖动时只改绘制位置.
#[derive(Clone)]
pub struct BlockTile {
    pub region_id: String,
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub top_fill: [u8; 3],
    pub bottom_fill: [u8; 3],
}

impl BlockTile {
    pub fn from_piece(region_id: String, img: &image::RgbImage, stats: crate::layout::PieceStats) -> Self {
        let (width, height) = img.dimensions();
        Self {
            region_id,
            image: rgb_to_render_image(img),
            width,
            height,
            top_fill: mean_to_u8(stats.top.0),
            bottom_fill: mean_to_u8(stats.bottom.0),
        }
    }
}

/// 工程底色的 GPU 贴图, 拖动时按 `preview_frame` 的裁切原点平移裁剪绘制.
#[derive(Clone)]
pub struct BlockBgTile {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub aspect_w: u32,
    pub aspect_h: u32,
}

impl BlockBgTile {
    pub fn from_rgb(img: &image::RgbImage, aspect_w: u32, aspect_h: u32) -> Self {
        let (width, height) = img.dimensions();
        Self {
            image: rgb_to_render_image(img),
            width,
            height,
            aspect_w,
            aspect_h,
        }
    }
}

fn mean_to_u8(m: [f32; 3]) -> [u8; 3] {
    [
        m[0].round().clamp(0.0, 255.0) as u8,
        m[1].round().clamp(0.0, 255.0) as u8,
        m[2].round().clamp(0.0, 255.0) as u8,
    ]
}

/// RGB → BGRA `RenderImage` (GPUI 贴图). 按行整块展开, 不走逐像素
/// `get_pixel`/`put_pixel`; 拖动分块的热路径会反复用到, 加载分块贴图
/// 与松手后回填整图都走这里.
pub(crate) fn rgb_to_render_image(rgb: &image::RgbImage) -> Arc<RenderImage> {
    let (w, h) = rgb.dimensions();
    let src = rgb.as_raw();
    let n = src.len() / 3 * 4;
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(n);
    }
    for (dst, s) in buf.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
        dst[0] = s[2];
        dst[1] = s[1];
        dst[2] = s[0];
        dst[3] = 255;
    }
    let rgba: RgbaImage = ImageBuffer::from_raw(w, h, buf).expect("rgba buffer size matches w*h*4");
    Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]))
}

impl MaskToolApp {
    /// 载入组合内各块的原始高度 (未应用调整) 与已有的位置/尺寸微调, 供
    /// 「组合分块」拖动使用. `voff`: 该组合拼合图在当前显示画布中的纵向
    /// 偏移 (底色合成把谱面居中时非 0), 换算命中测试/叠加线坐标要用,
    /// 同时也是"当前目标纵向位置" (`voff_target`, 见字段文档) 的初始值
    /// ——刚加载/尚未拖动过时, 目标位置就是当前显示的位置.
    pub fn set_block_geometry(
        &mut self,
        heights: Vec<(String, u32)>,
        layout: Vec<BlockAdjust>,
        hoff: i64,
        voff: i64,
    ) {
        self.block_heights = heights;
        self.block_layout = layout;
        self.block_hoff = hoff;
        self.block_voff = voff;
        self.voff_target = voff;
        if let Some(sel) = self.block_selected.clone() {
            if !self.block_heights.iter().any(|(id, _)| *id == sel) {
                self.block_selected = None;
            }
        }
    }

    pub fn set_piece_staff_ys(&mut self, ys: std::collections::HashMap<String, Option<i32>>) {
        self.piece_staff_ys = ys;
    }

    pub fn set_block_tiles(&mut self, tiles: Vec<BlockTile>, bg: Option<BlockBgTile>) {
        self.block_tiles = tiles;
        self.block_bg = bg;
        // 只填底色裁切原点, 不动 hoff/voff/img 尺寸——那些以宿主刚合成的
        // 预览图为准, 这里再算一遍并 `shift_masks` 会把已换算过的蒙版再
        // 平移一次.
        if let Some(frame) = self.compute_preview_frame() {
            self.block_bg_left = frame.bg_left;
            self.block_bg_top = frame.bg_top;
            self.block_shows_bg = frame.shows_bg;
            self.content_scale = frame.content_scale;
        }
    }

    pub fn is_block_dragging(&self) -> bool {
        matches!(
            self.drag,
            Some(DragKind::BlockMove { .. })
                | Some(DragKind::BlockResizeTop { .. })
                | Some(DragKind::BlockResizeBottom { .. })
        )
    }

    pub fn has_block_tiles(&self) -> bool {
        !self.block_tiles.is_empty()
    }

    pub fn preview_offsets(&self) -> (i64, i64) {
        (self.block_hoff, self.block_voff)
    }

    pub fn block_layout_clone(&self) -> Vec<BlockAdjust> {
        self.block_layout.clone()
    }

    pub fn voff_target(&self) -> i64 {
        self.voff_target
    }

    pub fn has_block_pieces(&self) -> bool {
        !self.block_heights.is_empty()
    }

    pub fn selected_block_id(&self) -> Option<&str> {
        self.block_selected.as_deref()
    }

    /// 从宿主一侧 (如右侧「组合分块」列表点选) 设置当前选中的分块, 与
    /// 画布内点选分块保持双向同步.
    pub fn select_block(&mut self, region_id: Option<String>, cx: &mut Context<Self>) {
        if self.block_selected == region_id {
            return;
        }
        self.block_selected = region_id;
        cx.notify();
    }

    /// 只替换当前显示的位图 (宿主重算含底色合成的拼合图后回填), 不动
    /// 历史/选中/缩放/会话状态; 拖动分块期间宿主每帧调用它保持画面同步.
    /// `voff`: 新拼合图在画布中的纵向偏移 (调整分块可能改变拼合图总高,
    /// 底色合成居中的偏移量也会跟着变, 必须同步更新, 否则下一帧命中
    /// 测试/叠加线的位置会跟画面错位).
    pub fn update_base_image(&mut self, rgb: image::RgbImage, hoff: i64, voff: i64, cx: &mut Context<Self>) {
        let (w, h) = rgb.dimensions();
        self.render_image = Some(rgb_to_render_image(&rgb));
        self.rgb_image = Some(rgb);
        self.img_w = w;
        self.img_h = h;
        self.block_hoff = hoff;
        self.block_voff = voff;
        if let Some(frame) = self.compute_preview_frame() {
            self.content_scale = frame.content_scale;
        }
        cx.notify();
    }

    fn compute_preview_frame(&self) -> Option<apply_bg::process::PreviewFrame> {
        if self.block_tiles.is_empty() {
            return None;
        }
        let sheet_w = self.block_tiles.iter().map(|t| t.width).max().unwrap_or(1);
        let sheet_h = layout::sheet_height(&self.block_heights, &self.block_layout);
        Some(if let Some(bg) = self.block_bg.as_ref() {
            let natural = apply_bg::process::natural_voff(
                sheet_w,
                sheet_h,
                bg.width,
                bg.height,
                bg.aspect_w,
                bg.aspect_h,
            );
            apply_bg::process::preview_frame(
                sheet_w,
                sheet_h,
                bg.width,
                bg.height,
                bg.aspect_w,
                bg.aspect_h,
                self.voff_target - natural,
            )
        } else {
            apply_bg::process::PreviewFrame {
                canvas_w: sheet_w,
                canvas_h: sheet_h,
                hoff: 0,
                voff: 0,
                bg_left: 0,
                bg_top: 0,
                shows_bg: false,
                content_scale: 1.0,
            }
        })
    }

    /// 按当前 `block_layout` / `voff_target` 重算预览画布尺寸与谱面偏移,
    /// 不碰像素. 拖动分块时每帧调用, 让叠加线/命中测试/贴图位置跟手.
    /// 底色页面尺寸锁在按宽定高; 谱面变高时只缩小内部块 (`content_scale`),
    /// 蒙版坐标按旧/新 (hoff, voff, scale) 整组换算.
    pub(super) fn refresh_preview_geom(&mut self) {
        let Some(frame) = self.compute_preview_frame() else {
            return;
        };
        self.remap_masks_canvas(
            self.block_hoff,
            self.block_voff,
            self.content_scale,
            frame.hoff,
            frame.voff,
            frame.content_scale,
        );
        self.img_w = frame.canvas_w;
        self.img_h = frame.canvas_h;
        self.block_hoff = frame.hoff;
        self.block_voff = frame.voff;
        self.block_bg_left = frame.bg_left;
        self.block_bg_top = frame.bg_top;
        self.block_shows_bg = frame.shows_bg;
        self.content_scale = frame.content_scale;
    }

    pub(super) fn content_scale_or_1(&self) -> f32 {
        if self.content_scale > 0.0001 {
            self.content_scale
        } else {
            1.0
        }
    }

    /// 谱面在画布上的横向范围 (已叠加 `block_hoff` / `content_scale`).
    /// 五线谱/大括号扫描必须用这个范围, 不能用整张画布宽 (底色 letterbox
    /// 会把谱线占宽稀释到判不成谱表).
    pub(super) fn sheet_x_range(&self) -> (i32, i32) {
        let w = self.img_w as i32;
        if w <= 1 {
            return (0, 0);
        }
        let cs = self.content_scale_or_1();
        let displayed_w = if self.block_tiles.is_empty() {
            self.img_w
        } else {
            let piece_w = self.block_tiles.iter().map(|t| t.width).max().unwrap_or(1);
            ((piece_w as f32) * cs).round().max(1.0) as u32
        };
        let x0 = self.block_hoff.clamp(0, (w - 1) as i64) as i32;
        let x1 = (x0 as i64 + displayed_w as i64 - 1).clamp(x0 as i64, (w - 1) as i64) as i32;
        (x0, x1.max(x0))
    }

    /// 把蒙版从旧的 (hoff, voff, content_scale) 映到新变换.
    /// 谱面点 `(sx, sy)` 对应画布 `hoff + sx * scale`, `voff + sy * scale`.
    fn remap_masks_canvas(
        &mut self,
        old_hoff: i64,
        old_voff: i64,
        old_scale: f32,
        new_hoff: i64,
        new_voff: i64,
        new_scale: f32,
    ) {
        let os = if old_scale > 0.0001 { old_scale } else { 1.0 };
        let ns = if new_scale > 0.0001 { new_scale } else { 1.0 };
        if old_hoff == new_hoff
            && old_voff == new_voff
            && (os - ns).abs() < 0.0001
        {
            return;
        }
        let oh = old_hoff as f32;
        let ov = old_voff as f32;
        let nh = new_hoff as f32;
        let nv = new_voff as f32;
        let radius_k = ns / os;
        for m in &mut self.masks {
            if m.is_brush() {
                m.brush_radius = ((m.brush_radius as f32) * radius_k).round().max(1.0) as i32;
            }
            m.map_xy(|x, y| {
                let sx = (x as f32 - oh) / os;
                let sy = (y as f32 - ov) / os;
                (
                    (nh + sx * ns).round() as i32,
                    (nv + sy * ns).round() as i32,
                )
            });
        }
    }

    /// 拖动分块导致拼合图总高 (罕见情况下总宽) 变化时, 底色合成居中的
    /// 偏移量也会跟着变; 已加载的蒙版矩形位置是按旧偏移换算的, 需要跟着
    /// 整体平移同样的量, 否则蒙版会跟画面错位.
    pub fn shift_masks(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        for m in &mut self.masks {
            m.translate(dx, dy);
        }
    }

    /// 组内各块在当前画布坐标系下 (已叠加 `block_voff` / `content_scale`)
    /// 的纵向范围.
    pub(super) fn block_spans(&self) -> Vec<(String, i64, i64)> {
        let cs = self.content_scale_or_1();
        let voff = self.block_voff as f32;
        layout::compute_spans(&self.block_heights, &self.block_layout)
            .into_iter()
            .map(|(rid, y0, y1)| {
                let cy0 = voff + (y0 as f32) * cs;
                let cy1 = voff + ((y1 + 1) as f32) * cs - 1.0;
                (rid, cy0.round() as i64, cy1.round() as i64)
            })
            .collect()
    }

    /// 画布坐标系下 (已叠加 `block_voff`) `iy` 落在哪个块的纵向范围内.
    pub(super) fn block_at_y(&self, iy: f32) -> Option<String> {
        self.block_spans()
            .into_iter()
            .find(|(_, y0, y1)| (*y0 as f32) <= iy && iy <= (*y1 as f32))
            .map(|(rid, ..)| rid)
    }

    /// 一条画迹/蒙版框落笔时应绑定的块: 起点/终点 (画布坐标, 已含
    /// `block_voff`) 其中一个不在任何块内, 或两者落在同一块内, 就绑定
    /// 那个块; 起点终点分别落在不同块 (冲突) 或都不在任何块内, 则不绑定
    /// (之后按几何中心动态归属, 见 [`Self::sync_masks_to_block_shift`]).
    pub(super) fn resolve_bound_block(&self, start_iy: f32, end_iy: f32) -> Option<String> {
        let start = self.block_at_y(start_iy);
        let end = self.block_at_y(end_iy);
        match (start, end) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            _ => None,
        }
    }

    /// 用旧/新 `block_layout` (同一份 `block_heights`) 对比出每个块的内容
    /// 整体挪动了多少 (含拖间距、裁剪/扩展导致的内容错位), 让归属该块的
    /// 画迹/蒙版框跟着同步平移, 保证蒙版拖动分块后仍然盖在同一块内容上.
    /// 每帧 (或每次布局变化) 传入*变化前*的 `old_layout` 调用一次即可,
    /// 内部按当前 `self.block_layout` 与之对比算出增量并直接平移
    /// `self.masks`.
    pub(super) fn sync_masks_to_block_shift(&mut self, old_layout: &[BlockAdjust]) {
        if self.block_heights.is_empty() || self.masks.is_empty() {
            return;
        }
        let deltas = layout::block_content_shifts(&self.block_heights, old_layout, &self.block_layout);
        if deltas.is_empty() {
            return;
        }
        let old_spans = layout::compute_spans(&self.block_heights, old_layout);
        let cs = self.content_scale_or_1();
        for m in &mut self.masks {
            let target = m.bound_block.as_ref().filter(|b| self.block_heights.iter().any(|(id, _)| id == *b)).cloned();
            let target = target.or_else(|| {
                let cy = ((m.y0 + m.y1) as f32 / 2.0 - self.block_voff as f32) / cs;
                old_spans
                    .iter()
                    .find(|(_, y0, y1)| (*y0 as f32) <= cy && cy <= (*y1 as f32))
                    .map(|(rid, ..)| rid.clone())
            });
            if let Some(rid) = target {
                if let Some(&d) = deltas.get(&rid) {
                    m.offset_y((d as f32 * cs).round() as i32);
                }
            }
        }
    }

    fn block_orig_height(&self, region_id: &str) -> Option<u32> {
        self.block_heights
            .iter()
            .find(|(id, _)| id == region_id)
            .map(|(_, h)| *h)
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

    /// 拖动命中测试: 在画布 y=`iy` (图像坐标, 已含 `block_voff`) 处, 找
    /// 最靠近的块上/下边界 (容差 `tol`, 图像像素) 或落在哪个块本体内.
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

    /// 开始拖动块本体 (整体上下移动). 记录完整的 `block_layout` 快照
    /// (供每帧重新分配用, 不做增量累加) 与当前 `block_voff` (折进第一块
    /// `gap_before` 后变成页面绝对坐标, y=0 即页顶).
    pub(super) fn begin_block_move(&mut self, region_id: String, iy: f32) {
        if !self.block_heights.iter().any(|(id, _)| *id == region_id) {
            return;
        }
        self.block_selected = Some(region_id.clone());
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
        self.drag = Some(DragKind::BlockMove {
            region_id,
            start_iy: iy,
            start_layout: self.block_layout.clone(),
            start_voff: self.block_voff.max(0).min(i32::MAX as i64) as i32,
            undid: false,
        });
    }

    pub(super) fn begin_block_resize_top(&mut self, region_id: String, iy: f32) {
        let Some(orig_h) = self.block_orig_height(&region_id) else {
            return;
        };
        self.ensure_layout_entry(&region_id);
        self.block_selected = Some(region_id.clone());
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
        self.drag = Some(DragKind::BlockResizeTop {
            region_id,
            start_iy: iy,
            start_layout: self.block_layout.clone(),
            start_voff: self.block_voff.max(0).min(i32::MAX as i64) as i32,
            max_trim: (orig_h as i32 - 1).max(0),
            undid: false,
        });
    }

    pub(super) fn begin_block_resize_bottom(&mut self, region_id: String, iy: f32) {
        let Some(orig_h) = self.block_orig_height(&region_id) else {
            return;
        };
        self.ensure_layout_entry(&region_id);
        self.block_selected = Some(region_id.clone());
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
        self.drag = Some(DragKind::BlockResizeBottom {
            region_id,
            start_iy: iy,
            start_layout: self.block_layout.clone(),
            start_voff: self.block_voff.max(0).min(i32::MAX as i64) as i32,
            max_trim: (orig_h as i32 - 1).max(0),
            undid: false,
        });
    }

    /// 每帧都从拖动起点的快照重新分配 (不做增量累加, 避免多帧误差), 见
    /// `layout::redistribute_for_block_move` 文档. 首次真正产生位移时
    /// push 一次撤销快照 (`undid` 之前是否已经 push 过), 返回
    /// `(undid, changed)`: `changed` 为 false 时调用方不必 `cx.notify()`.
    /// 完成后同步跟着这次布局变化平移归属受影响块的蒙版, 并刷新预览几何.
    pub(super) fn apply_block_move(
        &mut self,
        region_id: &str,
        start_iy: f32,
        start_layout: &[BlockAdjust],
        start_voff: i32,
        undid: bool,
        iy: f32,
    ) -> (bool, bool) {
        let cs = self.content_scale_or_1();
        let delta = ((iy - start_iy) / cs).round() as i32;
        if delta == 0 {
            return (undid, false);
        }
        let abs_start = layout::fold_voff_into_leading_gap(
            &self.block_heights,
            start_layout,
            start_voff,
        );
        let r = layout::redistribute_for_block_move(
            &self.block_heights,
            &abs_start,
            region_id,
            0,
            delta,
            snap_zero,
        );
        let new_voff_target = 0i64;
        if r.layout == self.block_layout && new_voff_target == self.voff_target {
            return (undid, false);
        }
        let mut undid = undid;
        if !undid {
            self.push_undo();
            undid = true;
        }
        let old_layout = self.block_layout.clone();
        self.block_layout = r.layout;
        self.voff_target = new_voff_target;
        self.sync_masks_to_block_shift(&old_layout);
        self.refresh_preview_geom();
        (undid, true)
    }

    /// 拖动上边界: 让"边线跟手" (边线本身跟着鼠标移动), 块自己的底边及
    /// 往后所有块的绝对位置分毫不动——与下边界"顶边及往前所有块不动"
    /// 完全镜像. 做法: 先把起点的 `voff` 折进第一块 `gap_before` (页面
    /// 绝对坐标, y=0 即页顶), 再让 `extra_top` 按 `delta` 增减, 同时把
    /// 这个块自己的 `gap_before` 按 `-delta` 反向抵消. 最上方块的上边界
    /// 因此可以一直拖到页顶; 到顶即停, 不改用高度缩放宽度. 吸附只作用在
    /// 被拖的那条边上, 另一侧用间距守恒反推, 其它块绝对位置不动.
    pub(super) fn apply_block_resize_top(
        &mut self,
        region_id: &str,
        start_iy: f32,
        start_layout: &[BlockAdjust],
        start_voff: i32,
        max_trim: i32,
        undid: bool,
        iy: f32,
    ) -> (bool, bool) {
        let cs = self.content_scale_or_1();
        let raw_delta = ((start_iy - iy) / cs).round() as i32;
        if raw_delta == 0 {
            return (undid, false);
        }
        let mut layout = layout::fold_voff_into_leading_gap(
            &self.block_heights,
            start_layout,
            start_voff,
        );
        let start_extra_top = BlockAdjust::find(&layout, region_id)
            .map(|a| a.extra_top)
            .unwrap_or(0);
        let start_gap_before = BlockAdjust::find(&layout, region_id)
            .map(|a| a.gap_before)
            .unwrap_or(0);
        let delta = raw_delta.clamp(-max_trim - start_extra_top, start_gap_before);
        if delta == 0 {
            return (undid, false);
        }
        let (new_extra_top, new_gap_before) = layout::resize_top_apply_delta(
            start_extra_top,
            start_gap_before,
            delta,
            snap_zero,
        );
        if let Some(a) = layout.iter_mut().find(|a| a.region_id == region_id) {
            a.extra_top = new_extra_top;
            a.gap_before = new_gap_before;
        } else {
            layout.push(BlockAdjust {
                region_id: region_id.to_string(),
                extra_top: new_extra_top,
                gap_before: new_gap_before,
                ..Default::default()
            });
        }
        if layout == self.block_layout && self.voff_target == 0 {
            return (undid, false);
        }
        let mut undid = undid;
        if !undid {
            self.push_undo();
            undid = true;
        }
        let old_layout = self.block_layout.clone();
        self.block_layout = layout;
        self.voff_target = 0;
        self.sync_masks_to_block_shift(&old_layout);
        self.refresh_preview_geom();
        (undid, true)
    }

    pub(super) fn apply_block_resize_bottom(
        &mut self,
        region_id: &str,
        start_iy: f32,
        start_layout: &[BlockAdjust],
        start_voff: i32,
        max_trim: i32,
        undid: bool,
        iy: f32,
    ) -> (bool, bool) {
        let cs = self.content_scale_or_1();
        let delta = ((iy - start_iy) / cs).round() as i32;
        if delta == 0 {
            return (undid, false);
        }
        let mut layout = layout::fold_voff_into_leading_gap(
            &self.block_heights,
            start_layout,
            start_voff,
        );
        let start_extra_bottom = BlockAdjust::find(&layout, region_id)
            .map(|a| a.extra_bottom)
            .unwrap_or(0);
        let next_id = self
            .block_heights
            .iter()
            .position(|(id, _)| id == region_id)
            .and_then(|i| self.block_heights.get(i + 1).map(|(id, _)| id.clone()));
        let start_slack = if let Some(ref nid) = next_id {
            BlockAdjust::find(&layout, nid)
                .map(|a| a.gap_before)
                .unwrap_or(0)
        } else {
            BlockAdjust::find(&layout, region_id)
                .map(|a| a.gap_after)
                .unwrap_or(0)
        };
        let (new_extra_bottom, new_slack) = layout::resize_bottom_apply_delta(
            start_extra_bottom,
            start_slack,
            delta,
            max_trim,
            snap_zero,
        );
        if let Some(a) = layout.iter_mut().find(|a| a.region_id == region_id) {
            a.extra_bottom = new_extra_bottom;
            if next_id.is_none() {
                a.gap_after = new_slack;
            }
        } else {
            layout.push(BlockAdjust {
                region_id: region_id.to_string(),
                extra_bottom: new_extra_bottom,
                gap_after: if next_id.is_none() { new_slack } else { 0 },
                ..Default::default()
            });
        }
        if let Some(ref nid) = next_id {
            if let Some(a) = layout.iter_mut().find(|a| a.region_id == *nid) {
                a.gap_before = new_slack;
            } else {
                layout.push(BlockAdjust {
                    region_id: nid.clone(),
                    gap_before: new_slack,
                    ..Default::default()
                });
            }
        }
        if layout == self.block_layout && self.voff_target == 0 {
            return (undid, false);
        }
        let mut undid = undid;
        if !undid {
            self.push_undo();
            undid = true;
        }
        let old_layout = self.block_layout.clone();
        self.block_layout = layout;
        self.voff_target = 0;
        self.sync_masks_to_block_shift(&old_layout);
        self.refresh_preview_geom();
        (undid, true)
    }
}

/// 靠近 0 (在 `BLOCK_SNAP_ZERO_IMG` 容差内) 时吸附为精确的 0, 方便手动
/// 拉伸/拖动跑偏后一下子调回"无调整"的原始状态.
fn snap_zero(v: i32) -> i32 {
    if v.abs() <= BLOCK_SNAP_ZERO_IMG {
        0
    } else {
        v
    }
}
