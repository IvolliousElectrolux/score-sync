//! 「组合分块」拖动调整: 几何/命中测试. 预览始终用分块/底色缩略图贴图
//! 分三层绘制 (底色 / 组合 / 画迹), 这里只算位置/尺寸, 换算成命中测试与
//! 叠加线用的坐标. 拖动中途只改 `block_layout` 并 `cx.notify()`, 不再每帧
//! 整图重拼/重新上传; 终稿拼合由宿主在导出时做.

use super::*;
use crate::layout;

/// 命中「组合分块」的哪个区域.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockHitZone {
    Top,
    Bottom,
    Left,
    Right,
    Body,
}

/// 单个分块的 GPU 贴图 (原始裁切, 未应用 layout), 拖动时只改绘制位置.
/// `thumb` 与 GPU 贴图同尺寸, 供滴管在三层预览上取样, 不必持有整图 RGB.
/// `source` 是磁盘上的原分辨率, 放大超过缩略图密度时按视口再裁一块.
#[derive(Clone)]
pub struct BlockTile {
    pub region_id: String,
    pub image: Arc<RenderImage>,
    pub thumb: Arc<image::RgbImage>,
    pub width: u32,
    pub height: u32,
    pub top_fill: [u8; 3],
    pub bottom_fill: [u8; 3],
    pub source: Option<PieceDisk>,
}

/// 分块原图像素在磁盘上的位置. `flat` 时文件本身就是这块;
/// 否则是整页, 条带为 `[band_y, band_y+band_h)`.
#[derive(Clone)]
pub struct PieceDisk {
    pub path: std::path::PathBuf,
    pub flat: bool,
    pub page_w: u32,
    pub page_h: u32,
    pub band_y: u32,
    pub band_h: u32,
}

impl BlockTile {
    pub fn from_piece(
        region_id: String,
        img: &image::RgbImage,
        stats: crate::layout::PieceStats,
    ) -> Self {
        let (width, height) = img.dimensions();
        Self::from_piece_sized(region_id, img, width, height, stats, None)
    }

    /// `logical_w`/`logical_h` 是原图像素尺寸 (布局/命中); GPU/thumb 可能更小.
    pub fn from_piece_sized(
        region_id: String,
        img: &image::RgbImage,
        logical_w: u32,
        logical_h: u32,
        stats: crate::layout::PieceStats,
        source: Option<PieceDisk>,
    ) -> Self {
        let (thumb, image) = rgb_to_thumb_and_render(img);
        Self {
            region_id,
            image,
            thumb,
            width: logical_w.max(1),
            height: logical_h.max(1),
            top_fill: mean_to_u8(stats.top.0),
            bottom_fill: mean_to_u8(stats.bottom.0),
            source,
        }
    }
}

/// 工程底色的 GPU 贴图: 完整底色只作备份, 这里上传的是按目标页
/// ([`apply_bg::process::page_size`]) 裁好后再缩到贴图上限的画布.
/// 预览铺满; 不再按完整扫描图原点平移裁剪, 也不把整页 RGB 送去 Triangle.
#[derive(Clone)]
pub struct BlockBgTile {
    pub image: Arc<RenderImage>,
    pub thumb: Arc<image::RgbImage>,
    /// 裁切后的目标画布宽高 (逻辑尺寸; GPU/thumb 可能更小).
    pub width: u32,
    pub height: u32,
    /// 完整底色备份的像素尺寸, 只给 `preview_frame` / `natural_voff`.
    pub src_width: u32,
    pub src_height: u32,
    pub aspect_w: u32,
    pub aspect_h: u32,
    /// 纯色底色: 预览画色块, 不上传大贴图. 改色只改这个字段.
    pub solid: Option<[u8; 3]>,
}

impl BlockBgTile {
    /// 从完整底色备份按目标页矩形直接缩到贴图上限, 不先拷一整页再 Triangle.
    /// 底色装不下该页时返回 `None`.
    pub fn from_full(
        full: &image::RgbImage,
        aspect_w: u32,
        aspect_h: u32,
        sheet_w: u32,
    ) -> Option<Self> {
        let (left, top, width, height) = apply_bg::process::bg_page_rect(
            full.width(),
            full.height(),
            aspect_w,
            aspect_h,
            sheet_w,
        )?;
        let thumb = Arc::new(crop_to_thumb(
            full,
            left,
            top,
            width,
            height,
            GPU_TEX_MAX_SIDE,
        ));
        Some(Self {
            image: rgb_to_render_image_raw(&thumb),
            thumb,
            width,
            height,
            src_width: full.width(),
            src_height: full.height(),
            aspect_w,
            aspect_h,
            solid: None,
        })
    }

    /// 纯色底色贴图: 1×1 缩略图 + 逻辑页尺寸, 预览画色块不采样大图.
    /// `src_w`/`src_h` 须能盖住 [`apply_bg::process::page_size`].
    pub fn from_solid(
        color: [u8; 3],
        aspect_w: u32,
        aspect_h: u32,
        sheet_w: u32,
        src_w: u32,
        src_h: u32,
    ) -> Option<Self> {
        let (_left, _top, width, height) =
            apply_bg::process::bg_page_rect(src_w, src_h, aspect_w, aspect_h, sheet_w)?;
        let thumb = Arc::new(image::RgbImage::from_pixel(1, 1, image::Rgb(color)));
        Some(Self {
            image: rgb_to_render_image_raw(&thumb),
            thumb,
            width,
            height,
            src_width: src_w,
            src_height: src_h,
            aspect_w,
            aspect_h,
            solid: Some(color),
        })
    }

    pub fn recolor_solid(&mut self, color: [u8; 3]) {
        self.solid = Some(color);
        self.thumb = Arc::new(image::RgbImage::from_pixel(1, 1, image::Rgb(color)));
    }
}

fn mean_to_u8(m: [f32; 3]) -> [u8; 3] {
    [
        m[0].round().clamp(0.0, 255.0) as u8,
        m[1].round().clamp(0.0, 255.0) as u8,
        m[2].round().clamp(0.0, 255.0) as u8,
    ]
}

fn sample_thumb(
    thumb: &image::RgbImage,
    logical_w: u32,
    logical_h: u32,
    lx: f32,
    ly: f32,
) -> Option<[u8; 3]> {
    let (tw, th) = thumb.dimensions();
    if tw == 0 || th == 0 || logical_w == 0 || logical_h == 0 {
        return None;
    }
    let x = (lx * tw as f32 / logical_w as f32)
        .clamp(0.0, (tw - 1) as f32)
        .round() as u32;
    let y = (ly * th as f32 / logical_h as f32)
        .clamp(0.0, (th - 1) as f32)
        .round() as u32;
    let p = thumb.get_pixel(x.min(tw - 1), y.min(th - 1));
    Some([p[0], p[1], p[2]])
}

/// GPUI 图集按整图上传; 高清页一次就是几十 MB, 切页还不 drop,
/// 按钮一按就要重绘整棵树, 卡半拍. 显示贴图限最长边, 命中/识别仍用原图像素.
pub const GPU_TEX_MAX_SIDE: u32 = 2048;

/// RGB → BGRA `RenderImage` (GPUI 贴图). 超过 [`GPU_TEX_MAX_SIDE`] 用
/// 面积平均缩成缩略图再上传 (大倍率下比 Triangle 快一个数量级, 预览
/// 本来也装不进 2048 以上的细节).
pub fn rgb_to_render_image(rgb: &image::RgbImage) -> Arc<RenderImage> {
    rgb_to_render_image_capped(rgb, GPU_TEX_MAX_SIDE)
}

/// 同 [`rgb_to_render_image`], 最长边上限由调用方指定 (页面预览跟屏幕走).
pub fn rgb_to_render_image_capped(rgb: &image::RgbImage, max_side: u32) -> Arc<RenderImage> {
    let (w, h) = rgb.dimensions();
    if max_side > 0 && w.max(h) > max_side {
        let scaled = downscale_to_max_side(rgb, max_side);
        return rgb_to_render_image_raw(&scaled);
    }
    rgb_to_render_image_raw(rgb)
}

fn rgb_to_thumb_and_render(rgb: &image::RgbImage) -> (Arc<image::RgbImage>, Arc<RenderImage>) {
    let (w, h) = rgb.dimensions();
    if w.max(h) > GPU_TEX_MAX_SIDE {
        let thumb = Arc::new(downscale_to_max_side(rgb, GPU_TEX_MAX_SIDE));
        let image = rgb_to_render_image_raw(&thumb);
        (thumb, image)
    } else {
        let thumb = Arc::new(rgb.clone());
        let image = rgb_to_render_image_raw(rgb);
        (thumb, image)
    }
}

fn gpu_scaled_dims(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let m = w.max(h);
    if m <= max_side || max_side == 0 {
        return (w.max(1), h.max(1));
    }
    let tw = ((w as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    let th = ((h as u64).saturating_mul(max_side as u64) / m as u64).max(1) as u32;
    (tw, th)
}

/// 把 `rgb` 缩到最长边 ≤ `max_side` (面积平均, 保持宽高比).
pub fn downscale_to_max_side(rgb: &image::RgbImage, max_side: u32) -> image::RgbImage {
    let (w, h) = rgb.dimensions();
    let (tw, th) = gpu_scaled_dims(w, h, max_side);
    if (tw, th) == (w, h) {
        return rgb.clone();
    }
    area_average(rgb, 0, 0, w, h, tw, th)
}

/// 从 `src` 的 `(left, top, cw × ch)` 直接缩到最长边 ≤ `max_side`,
/// 不先分配一整页裁切缓冲.
fn crop_to_thumb(
    src: &image::RgbImage,
    left: u32,
    top: u32,
    cw: u32,
    ch: u32,
    max_side: u32,
) -> image::RgbImage {
    let (tw, th) = gpu_scaled_dims(cw.max(1), ch.max(1), max_side);
    area_average(src, left, top, cw.max(1), ch.max(1), tw, th)
}

/// 把源矩形映射到 `tw × th`: 每个目标像素对覆盖的源像素做箱式平均.
/// 大倍率缩略比 `FilterType::Triangle` 便宜得多, 预览观感也够用.
fn area_average(
    src: &image::RgbImage,
    left: u32,
    top: u32,
    cw: u32,
    ch: u32,
    tw: u32,
    th: u32,
) -> image::RgbImage {
    let sw = src.width();
    let sh = src.height();
    let mut out = image::RgbImage::new(tw.max(1), th.max(1));
    if tw == 0 || th == 0 || cw == 0 || ch == 0 {
        return out;
    }
    let src_buf: &[u8] = src;
    let dst_buf: &mut [u8] = &mut out;
    let tw = tw.max(1);
    let th = th.max(1);
    for y in 0..th {
        let sy0 = top.saturating_add(((y as u64 * ch as u64) / th as u64) as u32);
        let sy1 = top
            .saturating_add((((y as u64 + 1) * ch as u64 + th as u64 - 1) / th as u64) as u32)
            .min(top.saturating_add(ch))
            .min(sh);
        let sy1 = sy1.max(sy0.saturating_add(1).min(sh));
        for x in 0..tw {
            let sx0 = left.saturating_add(((x as u64 * cw as u64) / tw as u64) as u32);
            let sx1 = left
                .saturating_add((((x as u64 + 1) * cw as u64 + tw as u64 - 1) / tw as u64) as u32)
                .min(left.saturating_add(cw))
                .min(sw);
            let sx1 = sx1.max(sx0.saturating_add(1).min(sw));
            let mut rs = 0u64;
            let mut gs = 0u64;
            let mut bs = 0u64;
            let mut n = 0u64;
            for sy in sy0..sy1 {
                if sy >= sh {
                    break;
                }
                let row = sy as usize * sw as usize * 3;
                for sx in sx0..sx1 {
                    if sx >= sw {
                        break;
                    }
                    let i = row + sx as usize * 3;
                    rs += src_buf[i] as u64;
                    gs += src_buf[i + 1] as u64;
                    bs += src_buf[i + 2] as u64;
                    n += 1;
                }
            }
            let o = (y as usize * tw as usize + x as usize) * 3;
            if n == 0 {
                dst_buf[o] = 0;
                dst_buf[o + 1] = 0;
                dst_buf[o + 2] = 0;
            } else {
                dst_buf[o] = (rs / n) as u8;
                dst_buf[o + 1] = (gs / n) as u8;
                dst_buf[o + 2] = (bs / n) as u8;
            }
        }
    }
    out
}

fn rgb_to_render_image_raw(rgb: &image::RgbImage) -> Arc<RenderImage> {
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
        self.clear_view_detail();
        let old_tiles = std::mem::take(&mut self.block_tiles);
        let old_bg = self.block_bg.take();
        for t in old_tiles {
            self.retire_gpu_image(Some(t.image));
        }
        if let Some(old) = old_bg {
            let reuse = bg
                .as_ref()
                .map(|n| Arc::ptr_eq(&n.image, &old.image))
                .unwrap_or(false);
            if !reuse {
                self.retire_gpu_image(Some(old.image));
            }
        }
        self.block_tiles = tiles;
        self.block_bg = bg;
        // 预览始终分三层画 (底色贴图 / 分块贴图 / 画迹), 画布尺寸跟
        // `preview_frame` 走, 不再等宿主合成一张整图再定宽高.
        if let Some(frame) = self.compute_preview_frame() {
            self.apply_preview_frame(frame);
        }
        self.ensure_sheet_masks();
    }

    /// 宿主只换底色层 (应用/取消/改纯色), 分块贴图不动, 避免整页重解码.
    pub fn apply_host_bg_tile(
        &mut self,
        bg: Option<BlockBgTile>,
        voff_target: i64,
        bg_applied: bool,
    ) {
        let old_bg = self.block_bg.take();
        if let Some(old) = old_bg {
            let reuse = bg
                .as_ref()
                .map(|n| Arc::ptr_eq(&n.image, &old.image))
                .unwrap_or(false);
            if !reuse {
                self.retire_gpu_image(Some(old.image));
            }
        }
        self.block_bg = bg;
        self.voff_target = voff_target;
        self.bg_applied = bg_applied;
        self.block_drag_freeze = None;
        self.refresh_preview_geom();
    }

    /// 纯色已在画时只改颜色, 不动几何 / GPU 贴图.
    pub fn recolor_host_bg_solid(&mut self, color: [u8; 3]) -> bool {
        let Some(bg) = self.block_bg.as_mut() else {
            return false;
        };
        if bg.solid.is_none() {
            return false;
        }
        bg.recolor_solid(color);
        true
    }

    pub fn is_block_dragging(&self) -> bool {
        matches!(
            self.drag,
            Some(DragKind::BlockMove { .. })
                | Some(DragKind::BlockResizeTop { .. })
                | Some(DragKind::BlockResizeBottom { .. })
                | Some(DragKind::BlockResizeLeft { .. })
                | Some(DragKind::BlockResizeRight { .. })
        )
    }

    /// 有分块贴图就按三层画 (底色 / 组合 / 画迹), 不再等一张烧好底色的整图.
    pub fn wants_block_tile_preview(&self) -> bool {
        !self.block_tiles.is_empty()
    }

    /// 在当前画布尺寸上锁住贴图预览 (撤重/对齐后整图尚未回填时).
    /// 拖动途中不要调用: 那边要保持拖动起点的尺寸, 避免缩放跟手跳.
    pub(super) fn hold_block_tile_preview(&mut self) {
        if self.block_tiles.is_empty() {
            return;
        }
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
    }

    /// 宿主合成失败或切页时解开贴图预览锁.
    pub fn release_block_tile_preview(&mut self) {
        self.block_drag_freeze = None;
    }

    pub fn has_block_tiles(&self) -> bool {
        !self.block_tiles.is_empty()
    }

    /// 按与 `paint_live_block_tiles` 相同的几何从缩略图层取样 (底色 → 组合).
    pub(super) fn sample_layered_rgb(&self, ix: f32, iy: f32) -> Option<[u8; 3]> {
        if self.img_w == 0 || self.img_h == 0 || self.block_tiles.is_empty() {
            return None;
        }
        let x = ix;
        let y = iy.clamp(0.0, (self.img_h - 1) as f32);
        let cs = self.content_scale_or_1();
        let hoff = self.block_hoff as f32;
        let voff = self.block_voff as f32;
        let canvas_x = |sx: f32| hoff + sx * cs;
        let canvas_y = |sy: f32| voff + sy * cs;
        let canvas_s = |s: f32| s * cs;
        let sheet_w = self.block_tiles.iter().map(|t| t.width).max().unwrap_or(1) as f32;
        let hx = canvas_x(0.0);
        let dw = canvas_s(sheet_w);
        let in_sheet_x = x >= hx && x < hx + dw;

        let mut yy: i64 = 0;
        let mut prev_bottom: Option<[u8; 3]> = None;
        let layout = &self.block_layout;
        for (i, tile) in self.block_tiles.iter().enumerate() {
            let adj = BlockAdjust::find(layout, &tile.region_id)
                .cloned()
                .unwrap_or_default();
            let (gap, ext_top, content_h, ext_bottom, _trim_top) =
                crate::layout::effective_metrics(tile.height as i32, &adj);
            if gap > 0 {
                if i > 0 && in_sheet_x {
                    if let Some(prev) = prev_bottom {
                        let top_half = gap / 2;
                        let y0 = canvas_y(yy as f32);
                        let y_mid = canvas_y((yy + top_half as i64) as f32);
                        let y1 = canvas_y((yy + gap as i64) as f32);
                        if top_half > 0 && y >= y0 && y < y_mid {
                            return Some(prev);
                        }
                        if y >= y_mid && y < y1 {
                            return Some(tile.top_fill);
                        }
                    }
                }
                yy += gap as i64;
            }
            let hx_block = canvas_x(adj.shift_x as f32);
            let (trim_l, trim_r, ext_l, ext_r, _content_w) =
                crate::layout::effective_h_metrics(tile.width as i32, &adj);
            let overlay_x = canvas_x((adj.shift_x - adj.extra_left) as f32);
            let overlay_w =
                canvas_s((tile.width as i32 + adj.extra_left + adj.extra_right).max(1) as f32);
            // 后续界面 (preview_only) 按原块左右裁, 溢出区取样落到后面的底色.
            let in_overlay_x =
                x >= overlay_x && x < overlay_x + overlay_w && (!self.preview_only || in_sheet_x);
            let paper_left_x1 = canvas_x((adj.shift_x + trim_l as i32) as f32);
            let paper_right_x0 = canvas_x((adj.shift_x + tile.width as i32 - trim_r as i32) as f32);
            let in_paper_x = (x >= canvas_x((adj.shift_x - ext_l as i32) as f32)
                && x < paper_left_x1)
                || (x >= paper_right_x0
                    && x < canvas_x((adj.shift_x + tile.width as i32 + ext_r as i32) as f32));
            let in_block_x = in_overlay_x || (in_paper_x && (!self.preview_only || in_sheet_x));
            if ext_top > 0 && in_block_x {
                let y0 = canvas_y(yy as f32);
                let y1 = canvas_y((yy + ext_top as i64) as f32);
                if y >= y0 && y < y1 {
                    return Some(tile.top_fill);
                }
            }
            let content_y = yy + ext_top as i64;
            if content_h > 0 && in_block_x {
                let piece_origin_y = canvas_y((yy + adj.extra_top as i64) as f32);
                let clip_y0 = canvas_y(content_y as f32);
                let clip_y1 = canvas_y((content_y + content_h as i64) as f32);
                if y >= clip_y0 && y < clip_y1 {
                    if in_paper_x {
                        return Some(tile.top_fill);
                    }
                    let local_x = (x - hx_block) / cs;
                    let local_y = (y - piece_origin_y) / cs;
                    if let Some(rgb) =
                        sample_thumb(&tile.thumb, tile.width, tile.height, local_x, local_y)
                    {
                        return Some(rgb);
                    }
                }
            }
            yy += ext_top as i64 + content_h as i64;
            if ext_bottom > 0 && in_block_x {
                let y0 = canvas_y(yy as f32);
                let y1 = canvas_y((yy + ext_bottom as i64) as f32);
                if y >= y0 && y < y1 {
                    return Some(tile.bottom_fill);
                }
                yy += ext_bottom as i64;
            }
            prev_bottom = Some(tile.bottom_fill);
        }

        if self.block_shows_bg {
            if let Some(bg) = self.block_bg.as_ref() {
                if let Some(c) = bg.solid {
                    return Some(c);
                }
                return sample_thumb(&bg.thumb, bg.width, bg.height, x, y);
            }
        }
        Some([255, 255, 255])
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

    pub fn set_voff_target(&mut self, v: i64) {
        self.voff_target = v;
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
    /// 历史/选中/缩放/会话状态. 拖动/撤重期间画面先用分块贴图撑着, 整图
    /// 到齐后走这里换回最终预览. `voff`: 新拼合图在画布中的纵向偏移
    /// (调整分块可能改变拼合图总高, 底色合成居中的偏移量也会跟着变,
    /// 必须同步更新, 否则下一帧命中测试/叠加线的位置会跟画面错位).
    pub fn update_base_image(
        &mut self,
        rgb: image::RgbImage,
        hoff: i64,
        voff: i64,
        cx: &mut Context<Self>,
    ) {
        let render = rgb_to_render_image(&rgb);
        self.update_base_image_with_render(rgb, render, hoff, voff, cx);
    }

    /// 同 [`Self::update_base_image`], 但 GPU 贴图由调用方 (通常在后台
    /// 线程) 预先转换好, 见 `rgb_to_render_image` 文档: 大图这一步自己
    /// 就要上百毫秒, 拖动分块松手那一刻若在界面线程上做就会卡一拍.
    pub fn update_base_image_with_render(
        &mut self,
        rgb: image::RgbImage,
        render: Arc<RenderImage>,
        hoff: i64,
        voff: i64,
        cx: &mut Context<Self>,
    ) {
        let (w, h) = rgb.dimensions();
        self.replace_render_image(Some(render));
        self.rgb_image = Some(rgb);
        self.img_w = w;
        self.img_h = h;
        self.clamp_brush_size();
        self.block_hoff = hoff;
        self.block_voff = voff;
        self.block_drag_freeze = None;
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
                bg.src_width,
                bg.src_height,
                bg.aspect_w,
                bg.aspect_h,
            );
            apply_bg::process::preview_frame(
                sheet_w,
                sheet_h,
                bg.src_width,
                bg.src_height,
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
    /// 底色页面尺寸锁在按宽定高; 谱面变高时只缩小内部块 (`content_scale`).
    /// 蒙版存在谱面坐标里, 缩放变化只改变换, 不再改数字.
    pub(super) fn refresh_preview_geom(&mut self) {
        let Some(frame) = self.compute_preview_frame() else {
            return;
        };
        self.ensure_sheet_masks();
        self.apply_preview_frame(frame);
    }

    /// 撤重后: 快照里的蒙版已经是谱面坐标, 只按还原后的 layout
    /// 重算 hoff/voff/尺寸, 不要再把坐标当画布点映一次.
    pub(super) fn restore_preview_geom_from_layout(&mut self) {
        let Some(frame) = self.compute_preview_frame() else {
            return;
        };
        self.apply_preview_frame(frame);
        self.hold_block_tile_preview();
    }

    fn apply_preview_frame(&mut self, frame: apply_bg::process::PreviewFrame) {
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

    /// 底色页在谱面坐标里的高度; 无底色 / 无比例时为 0 (没有页面锁).
    fn page_lock_height(&self) -> i32 {
        let Some(bg) = self.block_bg.as_ref() else {
            return 0;
        };
        if bg.aspect_w == 0 || bg.aspect_h == 0 {
            return 0;
        }
        let sw = self.block_tiles.iter().map(|t| t.width).max().unwrap_or(1);
        apply_bg::process::page_size(sw, bg.aspect_w, bg.aspect_h).1 as i32
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

    /// 宿主/落笔给的是画布坐标. 转成谱面坐标后, 缩放只改变换.
    pub(super) fn ensure_sheet_masks(&mut self) {
        if self.masks_are_sheet {
            return;
        }
        let cs = self.content_scale_or_1();
        let inv = 1.0 / cs;
        let dx = -(self.block_hoff as f32) * inv;
        let dy = -(self.block_voff as f32) * inv;
        for m in &mut self.masks {
            m.map_scale(inv, dx, dy);
        }
        self.masks_are_sheet = true;
    }

    /// 给宿主 / 绘制用: 谱面坐标映回当前画布 (hoff/voff/content_scale).
    pub(super) fn masks_for_canvas(&self) -> Vec<MaskRect> {
        if !self.masks_are_sheet {
            return self.masks.clone();
        }
        self.masks
            .iter()
            .cloned()
            .map(|m| self.mask_sheet_to_canvas(m))
            .collect()
    }

    pub(super) fn mask_sheet_to_canvas(&self, mut m: MaskRect) -> MaskRect {
        let cs = self.content_scale_or_1();
        m.map_scale(cs, self.block_hoff as f32, self.block_voff as f32);
        m
    }

    pub(super) fn canvas_to_sheet_xy(&self, x: f32, y: f32) -> (f32, f32) {
        if !self.masks_are_sheet {
            return (x, y);
        }
        let cs = self.content_scale_or_1();
        (
            (x - self.block_hoff as f32) / cs,
            (y - self.block_voff as f32) / cs,
        )
    }

    pub(super) fn sheet_to_canvas_xy(&self, x: f32, y: f32) -> (f32, f32) {
        if !self.masks_are_sheet {
            return (x, y);
        }
        let cs = self.content_scale_or_1();
        (
            self.block_hoff as f32 + x * cs,
            self.block_voff as f32 + y * cs,
        )
    }

    pub(super) fn canvas_radius_to_sheet(&self, r: i32) -> i32 {
        if !self.masks_are_sheet {
            return r.max(1);
        }
        let cs = self.content_scale_or_1();
        ((r as f32) / cs).round().max(1.0) as i32
    }

    /// 拖动分块导致拼合图总高 (罕见情况下总宽) 变化时, 底色合成居中的
    /// 偏移量也会跟着变; 已加载的蒙版矩形位置是按旧偏移换算的, 需要跟着
    /// 整体平移同样的量, 否则蒙版会跟画面错位.
    pub fn shift_masks(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        self.ensure_sheet_masks();
        let cs = self.content_scale_or_1();
        let sdx = (dx as f32 / cs).round() as i32;
        let sdy = (dy as f32 / cs).round() as i32;
        if sdx == 0 && sdy == 0 {
            return;
        }
        for m in &mut self.masks {
            m.translate(sdx, sdy);
        }
        self.nudge_brush_sprites(sdx, sdy);
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
        self.ensure_sheet_masks();
        let deltas =
            layout::block_content_shifts(&self.block_heights, old_layout, &self.block_layout);
        if deltas.is_empty() {
            return;
        }
        let old_spans = layout::compute_spans(&self.block_heights, old_layout);
        let mut nudges = Vec::new();
        for m in &mut self.masks {
            let target = m
                .bound_block
                .as_ref()
                .filter(|b| self.block_heights.iter().any(|(id, _)| id == *b))
                .cloned();
            let target = target.or_else(|| {
                let cy = (m.y0 + m.y1) as f32 / 2.0;
                old_spans
                    .iter()
                    .find(|(_, y0, y1)| (*y0 as f32) <= cy && cy <= (*y1 as f32))
                    .map(|(rid, ..)| rid.clone())
            });
            if let Some(rid) = target {
                if let Some(&(dx, dy)) = deltas.get(&rid) {
                    m.translate(dx, dy);
                    if m.is_brush() {
                        nudges.push((m.id.clone(), dx, dy));
                    }
                }
            }
        }
        for (id, dx, dy) in nudges {
            self.nudge_brush_sprite(&id, dx, dy);
        }
    }

    fn block_orig_height(&self, region_id: &str) -> Option<u32> {
        self.block_heights
            .iter()
            .find(|(id, _)| id == region_id)
            .map(|(_, h)| *h)
    }

    fn ensure_layout_entry(&mut self, region_id: &str) -> usize {
        if let Some(i) = self
            .block_layout
            .iter()
            .position(|a| a.region_id == region_id)
        {
            return i;
        }
        self.block_layout.push(BlockAdjust {
            region_id: region_id.to_string(),
            ..Default::default()
        });
        self.block_layout.len() - 1
    }

    fn block_piece_width(&self, region_id: &str) -> u32 {
        self.block_tiles
            .iter()
            .find(|t| t.region_id == region_id)
            .map(|t| t.width)
            .or_else(|| self.block_tiles.iter().map(|t| t.width).max())
            .unwrap_or(self.img_w.max(1))
    }

    /// 块内容在画布坐标系下的左右缘 (已叠加 `block_hoff` / `shift_x` /
    /// 左右裁扩 / `content_scale`). 可以超出页面, 供命中与叠加框使用.
    fn block_canvas_x_range(&self, region_id: &str) -> (f32, f32) {
        let cs = self.content_scale_or_1();
        let adj = BlockAdjust::find(&self.block_layout, region_id)
            .cloned()
            .unwrap_or_default();
        let w = self.block_piece_width(region_id) as i32;
        let (x0_sheet, w_sheet) = layout::overlay_x_w(w, &adj);
        let x0 = self.block_hoff as f32 + x0_sheet as f32 * cs;
        (x0, x0 + w_sheet as f32 * cs - 1.0)
    }

    /// 未横移时块的左右缘 (后续界面裁切线; 右缘为裁切边界, 不含最后一列
    /// 之后的像素).
    fn block_orig_canvas_x_range(&self, region_id: &str) -> (f32, f32) {
        let cs = self.content_scale_or_1();
        let w = self.block_piece_width(region_id) as f32;
        let x0 = self.block_hoff as f32;
        (x0, x0 + w * cs)
    }

    /// 有 `shift_x` 或左右向外扩展的块: 原左右边界 + 块纵向范围, 供蒙版
    /// 界面画裁切示意线 (导出仍按这里截).
    pub(super) fn block_shift_crop_marks(&self) -> Vec<(f32, f32, f32, f32)> {
        self.block_spans()
            .into_iter()
            .filter_map(|(rid, y0, y1)| {
                let adj = BlockAdjust::find(&self.block_layout, &rid)
                    .cloned()
                    .unwrap_or_default();
                if adj.shift_x == 0 && adj.extra_left <= 0 && adj.extra_right <= 0 {
                    return None;
                }
                let (ox0, ox1) = self.block_orig_canvas_x_range(&rid);
                Some((ox0, ox1, y0 as f32, y1 as f32))
            })
            .collect()
    }

    /// 各块在画布坐标系下的框 (`x0, y0, x1, y1`), 含横向偏移.
    pub(super) fn block_overlay_boxes(&self) -> Vec<(String, f32, f32, f32, f32)> {
        self.block_spans()
            .into_iter()
            .map(|(rid, y0, y1)| {
                let (x0, x1) = self.block_canvas_x_range(&rid);
                (rid, x0, y0 as f32, x1, y1 as f32)
            })
            .collect()
    }

    /// 拖动命中测试: 在画布 (`ix`, `iy`) (图像坐标, 已含 hoff/voff) 处,
    /// 找最靠近的块四边 (容差 `tol`) 或落在哪个块本体内. 横向按 `shift_x`
    /// 与左右裁扩后的实际位置 (含溢出页面的部分).
    pub(super) fn hit_block_at(
        &self,
        ix: f32,
        iy: f32,
        tol: f32,
    ) -> Option<(String, BlockHitZone)> {
        let spans = self.block_spans();
        if spans.is_empty() {
            return None;
        }
        let mut best: Option<(String, BlockHitZone, f32)> = None;
        let consider = |best: &mut Option<(String, BlockHitZone, f32)>,
                        rid: &str,
                        zone: BlockHitZone,
                        d: f32| {
            if d <= tol && best.as_ref().map(|(_, _, bd)| d < *bd).unwrap_or(true) {
                *best = Some((rid.to_string(), zone, d));
            }
        };
        for (rid, y0, y1) in &spans {
            let (x0, x1) = self.block_canvas_x_range(rid);
            let y0 = *y0 as f32;
            let y1 = *y1 as f32;
            if ix + tol < x0 || ix - tol > x1 || iy + tol < y0 || iy - tol > y1 {
                continue;
            }
            consider(&mut best, rid, BlockHitZone::Top, (iy - y0).abs());
            consider(&mut best, rid, BlockHitZone::Bottom, (iy - y1).abs());
            consider(&mut best, rid, BlockHitZone::Left, (ix - x0).abs());
            consider(&mut best, rid, BlockHitZone::Right, (ix - x1).abs());
        }
        if let Some((rid, zone, _)) = best {
            return Some((rid, zone));
        }
        spans
            .iter()
            .find(|(rid, y0, y1)| {
                let (x0, x1) = self.block_canvas_x_range(rid);
                ix >= x0 && ix <= x1 && (*y0 as f32) <= iy && iy <= (*y1 as f32)
            })
            .map(|(rid, ..)| (rid.clone(), BlockHitZone::Body))
    }

    /// 开始拖动块本体. `horizontal` 为 true 时锁定左右 (Shift 按下时);
    /// 否则只上下. 记录完整的 `block_layout` 快照 (供每帧重新分配用,
    /// 不做增量累加) 与当前 `block_voff` (折进第一块 `gap_before` 后变成
    /// 页面绝对坐标, y=0 即页顶).
    pub(super) fn begin_block_move(
        &mut self,
        region_id: String,
        ix: f32,
        iy: f32,
        horizontal: bool,
    ) {
        if !self.block_heights.iter().any(|(id, _)| *id == region_id) {
            return;
        }
        self.block_selected = Some(region_id.clone());
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
        self.drag = Some(DragKind::BlockMove {
            region_id,
            start_ix: ix,
            start_iy: iy,
            start_layout: self.block_layout.clone(),
            start_voff: self.block_voff.max(0).min(i32::MAX as i64) as i32,
            horizontal,
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

    pub(super) fn begin_block_resize_left(&mut self, region_id: String, ix: f32) {
        let orig_w = self.block_piece_width(&region_id) as i32;
        if orig_w <= 0 {
            return;
        }
        self.ensure_layout_entry(&region_id);
        self.block_selected = Some(region_id.clone());
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
        self.drag = Some(DragKind::BlockResizeLeft {
            region_id,
            start_ix: ix,
            start_layout: self.block_layout.clone(),
            orig_w,
            undid: false,
        });
    }

    pub(super) fn begin_block_resize_right(&mut self, region_id: String, ix: f32) {
        let orig_w = self.block_piece_width(&region_id) as i32;
        if orig_w <= 0 {
            return;
        }
        self.ensure_layout_entry(&region_id);
        self.block_selected = Some(region_id.clone());
        self.block_drag_freeze = Some((self.img_w as f32, self.img_h as f32));
        self.drag = Some(DragKind::BlockResizeRight {
            region_id,
            start_ix: ix,
            start_layout: self.block_layout.clone(),
            orig_w,
            undid: false,
        });
    }

    /// 每帧都从拖动起点的快照重新分配 (不做增量累加, 避免多帧误差), 见
    /// `layout::redistribute_for_block_move` 文档. 有页面锁时向下拖到页底
    /// 即停, 与向上到顶对称. 首次真正产生位移时 push 一次撤销快照
    /// (`undid` 之前是否已经 push 过), 返回 `(undid, changed)`:
    /// `changed` 为 false 时调用方不必 `cx.notify()`.
    /// 完成后同步跟着这次布局变化平移归属受影响块的蒙版, 并刷新预览几何.
    pub(super) fn apply_block_move(
        &mut self,
        region_id: &str,
        start_ix: f32,
        start_iy: f32,
        start_layout: &[BlockAdjust],
        start_voff: i32,
        horizontal: bool,
        undid: bool,
        ix: f32,
        iy: f32,
    ) -> (bool, bool) {
        if horizontal {
            return self.apply_block_shift_x_drag(region_id, start_ix, start_layout, undid, ix);
        }
        let cs = self.content_scale_or_1();
        let delta = ((iy - start_iy) / cs).round() as i32;
        if delta == 0 {
            return (undid, false);
        }
        let abs_start =
            layout::fold_voff_into_leading_gap(&self.block_heights, start_layout, start_voff);
        let r = layout::redistribute_for_block_move(
            &self.block_heights,
            &abs_start,
            region_id,
            0,
            delta,
            self.page_lock_height(),
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

    /// Shift 左右拖: 只改被拖块的 `shift_x`, 上下布局不动, 不夹原块左右.
    fn apply_block_shift_x_drag(
        &mut self,
        region_id: &str,
        start_ix: f32,
        start_layout: &[BlockAdjust],
        undid: bool,
        ix: f32,
    ) -> (bool, bool) {
        let cs = self.content_scale_or_1();
        let delta = ((ix - start_ix) / cs).round() as i32;
        if delta == 0 {
            return (undid, false);
        }
        let layout = layout::apply_block_shift_x(start_layout, region_id, delta, snap_zero);
        if layout == self.block_layout {
            return (undid, false);
        }
        let mut undid = undid;
        if !undid {
            self.push_undo();
            undid = true;
        }
        let old_layout = self.block_layout.clone();
        self.block_layout = layout;
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
        let mut layout =
            layout::fold_voff_into_leading_gap(&self.block_heights, start_layout, start_voff);
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
        let (new_extra_top, new_gap_before) =
            layout::resize_top_apply_delta(start_extra_top, start_gap_before, delta, snap_zero);
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

    /// 拖下边界: 先消耗与下一块之间的空白, 贴住之后才挤开. 有页面锁时
    /// 拼合图高到页底即停, 与上边界到顶对称.
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
        let mut layout =
            layout::fold_voff_into_leading_gap(&self.block_heights, start_layout, start_voff);
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
        let start_sh = layout::sheet_height(&self.block_heights, &layout) as i32;
        let page_h = self.page_lock_height();
        let max_grow = if page_h > 0 {
            (page_h - start_sh).max(0)
        } else {
            i32::MAX
        };
        let (new_extra_bottom, new_slack) = layout::resize_bottom_apply_delta(
            start_extra_bottom,
            start_slack,
            delta,
            max_trim,
            max_grow,
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

    /// 拖左右边界: 边线跟手, 内容与另一侧不动. 向内裁掉的空位用谱纸色
    /// 填; 向外扩不夹原块, 导出仍按原块左右截掉.
    pub(super) fn apply_block_resize_side(
        &mut self,
        region_id: &str,
        start_ix: f32,
        start_layout: &[BlockAdjust],
        orig_w: i32,
        left: bool,
        undid: bool,
        ix: f32,
    ) -> (bool, bool) {
        let cs = self.content_scale_or_1();
        let mouse = ((ix - start_ix) / cs).round() as i32;
        if mouse == 0 {
            return (undid, false);
        }
        let delta = if left { -mouse } else { mouse };
        let layout = layout::apply_block_resize_side(
            start_layout,
            region_id,
            left,
            delta,
            orig_w,
            snap_zero,
        );
        if layout == self.block_layout {
            return (undid, false);
        }
        let mut undid = undid;
        if !undid {
            self.push_undo();
            undid = true;
        }
        let old_layout = self.block_layout.clone();
        self.block_layout = layout;
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

#[cfg(test)]
mod gpu_tex_tests {
    use super::{rgb_to_render_image, BlockBgTile, GPU_TEX_MAX_SIDE};
    use image::RgbImage;

    #[test]
    fn downscale_keeps_aspect_and_caps_side() {
        let img = RgbImage::from_pixel(3000, 4000, image::Rgb([10, 20, 30]));
        let thumb = super::downscale_to_max_side(&img, GPU_TEX_MAX_SIDE);
        let (w, h) = thumb.dimensions();
        assert!(w.max(h) <= GPU_TEX_MAX_SIDE);
        assert_eq!(w * 4000, h * 3000);
        assert_eq!(*thumb.get_pixel(0, 0), image::Rgb([10, 20, 30]));
    }

    #[test]
    fn huge_rgb_is_capped_for_gpu() {
        let img = RgbImage::from_pixel(3000, 4000, image::Rgb([10, 20, 30]));
        let tex = rgb_to_render_image(&img);
        let sz = tex.size(0);
        let w = i32::from(sz.width) as u32;
        let h = i32::from(sz.height) as u32;
        assert!(w.max(h) <= GPU_TEX_MAX_SIDE);
        assert_eq!(w * 4000, h * 3000);
    }

    #[test]
    fn from_full_uploads_page_crop_not_full_bg() {
        let bg = RgbImage::from_pixel(800, 800, image::Rgb([10, 20, 30]));
        let sheet_w = 200u32;
        let tile = BlockBgTile::from_full(&bg, 16, 9, sheet_w).expect("covers page");
        let (cw, ch) = apply_bg::process::page_size(sheet_w, 16, 9);
        assert_eq!((tile.width, tile.height), (cw, ch));
        assert_eq!((tile.src_width, tile.src_height), (800, 800));
        assert_eq!((tile.aspect_w, tile.aspect_h), (16, 9));
        let sz = tile.image.size(0);
        let tw = i32::from(sz.width) as u32;
        let th = i32::from(sz.height) as u32;
        assert!(tw.max(th) <= GPU_TEX_MAX_SIDE);
        assert!(tile.width < bg.width() || tile.height < bg.height());
    }

    #[test]
    fn from_full_rejects_undersized_bg() {
        let bg = RgbImage::from_pixel(50, 50, image::Rgb([10, 20, 30]));
        assert!(BlockBgTile::from_full(&bg, 16, 9, 200).is_none());
    }

    #[test]
    fn from_solid_covers_page_without_full_image() {
        let sheet_w = 200u32;
        let (src_w, src_h) = apply_bg::process::page_size(400, 16, 9);
        let tile = BlockBgTile::from_solid([10, 20, 30], 16, 9, sheet_w, src_w, src_h)
            .expect("virtual src covers page");
        let (cw, ch) = apply_bg::process::page_size(sheet_w, 16, 9);
        assert_eq!((tile.width, tile.height), (cw, ch));
        assert_eq!(tile.solid, Some([10, 20, 30]));
        tile.thumb.get_pixel(0, 0);
        let mut tile = tile;
        tile.recolor_solid([1, 2, 3]);
        assert_eq!(tile.solid, Some([1, 2, 3]));
        assert_eq!(*tile.thumb.get_pixel(0, 0), image::Rgb([1, 2, 3]));
    }
}
