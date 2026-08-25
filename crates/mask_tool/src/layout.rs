//! 「组合分块」位置/尺寸微调: 数据模型 + 拼接/几何计算.
//!
//! 蒙版编辑时可以对组合内某个分块的上下边做裁剪/扩展, 或在块与块之间插入
//! 间距, 只影响该组合的拼合图 (蒙版预览/终稿导出/视频素材), 不改变分块
//! 面板中的原始 `Region.y0/y1`. 这里同时给出:
//! - 纯几何版本 [`compute_spans`] (不需要像素数据, 供列表/画布做位置显示
//!   与命中测试);
//! - 像素版本 [`stitch_with_layout`] (实际拼接输出图像, 新增区域用
//!   [`crate::bg_fill`] 识别背景色模式后填充).

use image::RgbImage;

use crate::bg_fill;

/// 对组合内某一分块的位置/尺寸微调.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockAdjust {
    pub region_id: String,
    /// 顶边调整: 负值向内裁掉该数值像素 (裁进图内容), 正值向外扩展该数值
    /// 像素 (背景色模式填充).
    pub extra_top: i32,
    /// 底边调整: 同上, 作用于底边.
    pub extra_bottom: i32,
    /// 与上一个块之间的额外间距 (像素, 背景色模式填充); 组合内第一块的
    /// 该值表示画布最顶端多出的留白 (同样有效, 并非被忽略).
    pub gap_before: i32,
    /// 整张拼合图最末端的留白 (只作用在最后一块上). 旧版「向上拖过页顶
    /// 后改用高度缩放宽度」会写入这里; 现在碰到页顶即停, 不再新增.
    /// 已有工程里的残留值仍会在向下拖时优先被吃掉.
    pub gap_after: i32,
}

impl BlockAdjust {
    pub fn is_noop(&self) -> bool {
        self.extra_top == 0 && self.extra_bottom == 0 && self.gap_before == 0 && self.gap_after == 0
    }

    pub fn find<'a>(layout: &'a [BlockAdjust], region_id: &str) -> Option<&'a BlockAdjust> {
        layout.iter().find(|a| a.region_id == region_id)
    }
}

/// 单块调整后的有效高度 (裁剪/扩展合计) 与间距, 纯几何, 不涉及像素.
/// 返回 `(gap_before, ext_top, content_h, ext_bottom, trim_top)`.
pub fn effective_metrics(orig_h: i32, adj: &BlockAdjust) -> (u32, u32, u32, u32, u32) {
    // 返回 (gap_before, ext_top, content_h, ext_bottom, trim_top) 均为像素数.
    let max_trim = (orig_h - 1).max(0);
    let trim_top = (-adj.extra_top).clamp(0, max_trim);
    let remaining = max_trim - trim_top;
    let trim_bottom = (-adj.extra_bottom).clamp(0, remaining);
    let ext_top = adj.extra_top.max(0) as u32;
    let ext_bottom = adj.extra_bottom.max(0) as u32;
    let content_h = (orig_h - trim_top - trim_bottom).max(0) as u32;
    let gap_before = adj.gap_before.max(0) as u32;
    (gap_before, ext_top, content_h, ext_bottom, trim_top as u32)
}

/// 计算组合内各块在最终拼合图中的纵向范围 (`comp_y0..=comp_y1`), 按
/// `heights` 给出的原始高度与传入的 `layout` 微调推算, 不需要像素数据.
pub fn compute_spans(heights: &[(String, u32)], layout: &[BlockAdjust]) -> Vec<(String, i64, i64)> {
    let mut yy: i64 = 0;
    heights
        .iter()
        .map(|(rid, h)| {
            let adj = BlockAdjust::find(layout, rid).cloned().unwrap_or_default();
            let (gap_before, ext_top, content_h, ext_bottom, _trim_top) =
                effective_metrics(*h as i32, &adj);
            yy += gap_before as i64;
            let y0 = yy;
            let total = ext_top as i64 + content_h as i64 + ext_bottom as i64;
            let y1 = yy + total - 1;
            yy += total;
            (rid.clone(), y0, y1.max(y0))
        })
        .collect()
}

/// 最后一块后面的末端留白 (只读最后一块的 `gap_after`).
pub fn trailing_gap(heights: &[(String, u32)], layout: &[BlockAdjust]) -> u32 {
    let Some((last_id, _)) = heights.last() else {
        return 0;
    };
    BlockAdjust::find(layout, last_id)
        .map(|a| a.gap_after.max(0) as u32)
        .unwrap_or(0)
}

/// 拼合图总高: 各块范围 + 末端留白.
pub fn sheet_height(heights: &[(String, u32)], layout: &[BlockAdjust]) -> u32 {
    let base = compute_spans(heights, layout)
        .last()
        .map(|(_, _, y1)| (*y1 + 1).max(1) as u32)
        .unwrap_or(1);
    base.saturating_add(trailing_gap(heights, layout))
}

/// 单块背景色统计 (顶边/底边样本各自的众数/标准差), 采样自该块原始
/// (未裁剪) 图像的边缘, 与当前裁剪/扩展量无关 —— 这样可以在加载时算一次
/// 并缓存, 拖动分块时每帧复用, 不必重新扫描像素 (这一步扫描才是真正
/// 耗时的部分, 拼接本身只是内存搬运).
#[derive(Clone, Copy, Debug)]
pub struct PieceStats {
    pub top: ([f32; 3], [f32; 3]),
    pub bottom: ([f32; 3], [f32; 3]),
}

impl Default for PieceStats {
    fn default() -> Self {
        let flat = ([245.0, 245.0, 245.0], [2.0, 2.0, 2.0]);
        Self { top: flat, bottom: flat }
    }
}

const STATS_SAMPLE_ROWS: u32 = 32;

/// 计算单块的背景色统计, 见 [`PieceStats`].
pub fn compute_piece_stats(img: &RgbImage, ink_threshold: i32) -> PieceStats {
    PieceStats {
        top: bg_fill::sample_bg_stats(&bg_fill::edge_sample(img, true, STATS_SAMPLE_ROWS), ink_threshold),
        bottom: bg_fill::sample_bg_stats(&bg_fill::edge_sample(img, false, STATS_SAMPLE_ROWS), ink_threshold),
    }
}

/// 把组内各块 (已解码像素) 按顺序竖向拼接, 应用 `layout` 中的裁剪/扩展/
/// 间距, 新增区域用该块自身背景色统计合成填充. `layout` 为空时等价于
/// 单纯首尾相接 (不做任何裁剪/扩展/加间距). 每次都会重新扫描各块边缘算
/// 统计量; 如果要反复重拼 (比如拖动分块时每帧都要), 用 [`stitch_with_stats`]
/// 搭配预先算好并缓存的 [`PieceStats`], 避免每帧都扫描像素.
pub fn stitch_with_layout(
    parts: &[(String, RgbImage)],
    layout: &[BlockAdjust],
    ink_threshold: i32,
) -> RgbImage {
    let stats: Vec<PieceStats> = parts
        .iter()
        .map(|(_, img)| compute_piece_stats(img, ink_threshold))
        .collect();
    stitch_with_stats(parts, &stats, layout)
}

/// 把 `src` 第 `src_y0..src_y0+count` 行整块拷到 `dst` 左上角对齐 (x=0)、
/// 纵向偏移 `dst_y` 的位置; 按行 `copy_from_slice`, 不用
/// `image::imageops::replace`/`crop_imm` (内部逐像素调用 get_pixel/
/// put_pixel, 拖动分块每帧都要重新拼接整张组合图, 大图这样调用的开销
/// 很可观).
fn blit_rows(dst: &mut RgbImage, src: &RgbImage, src_y0: u32, count: u32, dst_y: i64) {
    if dst_y < 0 || count == 0 {
        return;
    }
    let dw = dst.width() as usize;
    let dh = dst.height() as usize;
    let sw = src.width() as usize;
    let copy_w = sw.min(dw) * 3;
    let dst_y = dst_y as usize;
    // `ImageBuffer` 同时实现了 `Index<(u32,u32)>` 与 `Deref<Target=[u8]>`,
    // 直接用 range 下标会被解析成前者报类型不匹配, 需要先显式解引用成
    // 裸字节切片再按 range 切.
    let src_buf: &[u8] = src;
    let dst_buf: &mut [u8] = dst;
    for row in 0..count as usize {
        let dy = dst_y + row;
        if dy >= dh {
            break;
        }
        let sy = src_y0 as usize + row;
        let d0 = dy * dw * 3;
        let s0 = sy * sw * 3;
        dst_buf[d0..d0 + copy_w].copy_from_slice(&src_buf[s0..s0 + copy_w]);
    }
}

/// 同 [`stitch_with_layout`], 但背景色统计由调用方预先算好传入 (`stats`
/// 与 `parts` 一一对应; 长度不够时该块退化为一个浅灰兜底色). 新增区域用
/// 统计均值纯色填充 (不加噪声), 只做像素搬运 + 整块填色, 没有逐像素扫描
/// 统计也没有逐像素随机数, 可以每帧调用.
pub fn stitch_with_stats(
    parts: &[(String, RgbImage)],
    stats: &[PieceStats],
    layout: &[BlockAdjust],
) -> RgbImage {
    let default_stats = PieceStats::default();
    struct Piece<'a> {
        gap_before: u32,
        ext_top: u32,
        ext_bottom: u32,
        stats: &'a PieceStats,
        trim_top: u32,
        content_h: u32,
        img: &'a RgbImage,
    }
    let mut pieces: Vec<Piece> = Vec::with_capacity(parts.len());
    for (i, (rid, img)) in parts.iter().enumerate() {
        let adj = BlockAdjust::find(layout, rid).cloned().unwrap_or_default();
        let (gap_before, ext_top, content_h, ext_bottom, trim_top) =
            effective_metrics(img.height() as i32, &adj);
        pieces.push(Piece {
            gap_before,
            ext_top,
            ext_bottom,
            stats: stats.get(i).unwrap_or(&default_stats),
            trim_top,
            content_h,
            img,
        });
    }
    let max_w = pieces.iter().map(|p| p.img.width()).max().unwrap_or(1);
    let trailing = parts
        .last()
        .and_then(|(rid, _)| BlockAdjust::find(layout, rid))
        .map(|a| a.gap_after.max(0) as u32)
        .unwrap_or(0);
    let total_h: u32 = pieces
        .iter()
        .map(|p| p.gap_before + p.ext_top + p.content_h + p.ext_bottom)
        .sum::<u32>()
        .saturating_add(trailing);
    let mut combined = RgbImage::from_pixel(max_w, total_h.max(1), image::Rgb([255, 255, 255]));
    let mut yy: i64 = 0;
    for i in 0..pieces.len() {
        let p = &pieces[i];
        if p.gap_before > 0 {
            // 只有"块与块之间"(i > 0, 存在真正的上一块) 才需要智能识别
            // 背景色模式填充: 间距上半段贴上一块的底边色, 下半段贴本块的
            // 顶边色, 各自与真正相邻的那条边界颜色衔接.
            //
            // 画布最前端 (i == 0, 没有上一块) 这段留白根本不是"两块之间"
            // 的间隙, 不需要 (也不该) 计算任何填充色——有底色层时宿主会
            // 在合成阶段直接跳过贴图这一段, 让底色本身透出来 (见
            // `apply_bg::process::composite_preview` 的 `top_transparent`
            // 参数与 `Doc::group_leading_gap`); 没有底色层时这里保持画布
            // 初始化的纯白即可, 省一次采样/填色开销.
            if i > 0 {
                let prev_color = pieces[i - 1].stats.bottom.0;
                let top_half = p.gap_before / 2;
                if top_half > 0 {
                    let fill = bg_fill::flat_fill(max_w, top_half, prev_color);
                    blit_rows(&mut combined, &fill, 0, top_half, yy);
                }
                let bottom_half = p.gap_before - top_half;
                if bottom_half > 0 {
                    let fill = bg_fill::flat_fill(max_w, bottom_half, p.stats.top.0);
                    blit_rows(&mut combined, &fill, 0, bottom_half, yy + top_half as i64);
                }
            }
            yy += p.gap_before as i64;
        }
        if p.ext_top > 0 {
            let fill = bg_fill::flat_fill(max_w, p.ext_top, p.stats.top.0);
            blit_rows(&mut combined, &fill, 0, p.ext_top, yy);
            yy += p.ext_top as i64;
        }
        // 未裁剪且宽度已一致时直接整块搬原图 (拖动其它块时大多数块都是
        // 这种情况, 是每帧最大的一块拷贝, 必须走快路径).
        if p.img.width() == max_w {
            blit_rows(&mut combined, p.img, p.trim_top, p.content_h, yy);
        } else {
            let mut padded = RgbImage::from_pixel(max_w, p.content_h, image::Rgb([255, 255, 255]));
            blit_rows(&mut padded, p.img, p.trim_top, p.content_h, 0);
            blit_rows(&mut combined, &padded, 0, p.content_h, yy);
        }
        yy += p.content_h as i64;
        if p.ext_bottom > 0 {
            let fill = bg_fill::flat_fill(max_w, p.ext_bottom, p.stats.bottom.0);
            blit_rows(&mut combined, &fill, 0, p.ext_bottom, yy);
            yy += p.ext_bottom as i64;
        }
    }
    combined
}

/// 对比同一组 `heights` 在旧/新 `layout` 下, 算出每个块的内容整体挪动了
/// 多少像素 (含拖间距导致的整体顺移、裁剪/扩展导致的内容在块自身范围内
/// 错位两种来源的合计). 只返回*真的动过* (非 0) 的块. 供拖动分块时同步
/// 平移归属该块的蒙版/画迹, 见 `MaskToolApp::sync_masks_to_block_shift`.
///
/// 原理: 块 i 顶端 `y0` 只受它自己 `gap_before` 与前面所有块总高的影响,
/// 跟它自己的 `extra_top` 无关; 但 `extra_top` 变化会让"块内容真正开始
/// 的位置"相对 `y0` 错位 (裁掉顶部空白/内容则内容整体上移, 反之下移),
/// 这部分位移恰好等于 `extra_top` 本身的变化量 (可证明: 无论扩展还是
/// 裁剪, `ext_top - trim_top` 恒等于 `extra_top`). 两者相加即为块内容在
/// 画布坐标系下的总位移, 对块内任意"真正落在内容区"的点都是同一个常数
/// (与该点在块内的具体偏移无关), 因此可以整体作为一个平移量应用.
pub fn block_content_shifts(
    heights: &[(String, u32)],
    old_layout: &[BlockAdjust],
    new_layout: &[BlockAdjust],
) -> std::collections::HashMap<String, i32> {
    let old_spans = compute_spans(heights, old_layout);
    let new_spans = compute_spans(heights, new_layout);
    let extra_top_of = |layout: &[BlockAdjust], id: &str| {
        BlockAdjust::find(layout, id).map(|a| a.extra_top).unwrap_or(0)
    };
    let mut deltas = std::collections::HashMap::new();
    for (rid, _) in heights {
        let old_y0 = old_spans.iter().find(|(r, ..)| r == rid).map(|(_, y0, _)| *y0);
        let new_y0 = new_spans.iter().find(|(r, ..)| r == rid).map(|(_, y0, _)| *y0);
        let (Some(old_y0), Some(new_y0)) = (old_y0, new_y0) else {
            continue;
        };
        let d = (new_y0 - old_y0) as i32 + (extra_top_of(new_layout, rid) - extra_top_of(old_layout, rid));
        if d != 0 {
            deltas.insert(rid.clone(), d);
        }
    }
    deltas
}

/// 拖动块本体 (整体上下移动) 一帧的结果: 更新后的完整 `layout` (只有真的
/// 被"波及"的块的 `gap_before` 会变, 其余原样保留自传入的起点快照) 与
/// `voff_shift_delta` (需要*加到*拖动起点快照的组合纵向手动偏移上, 非直接
/// 赋值; 含义见 [`redistribute_for_block_move`] 关于 `extra_room` 的说明).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockMoveResult {
    pub layout: Vec<BlockAdjust>,
    pub voff_shift_delta: i32,
}

fn gap_of(layout: &[BlockAdjust], id: &str) -> i32 {
    BlockAdjust::find(layout, id).map(|a| a.gap_before).unwrap_or(0)
}

fn set_gap(layout: &mut Vec<BlockAdjust>, id: &str, v: i32) {
    if let Some(a) = layout.iter_mut().find(|a| a.region_id == id) {
        a.gap_before = v;
    } else {
        layout.push(BlockAdjust { region_id: id.to_string(), gap_before: v, ..Default::default() });
    }
}

fn gap_after_of(layout: &[BlockAdjust], id: &str) -> i32 {
    BlockAdjust::find(layout, id).map(|a| a.gap_after).unwrap_or(0)
}

fn set_gap_after(layout: &mut Vec<BlockAdjust>, id: &str, v: i32) {
    if let Some(a) = layout.iter_mut().find(|a| a.region_id == id) {
        a.gap_after = v;
    } else {
        layout.push(BlockAdjust { region_id: id.to_string(), gap_after: v, ..Default::default() });
    }
}

/// 把画布上的居中留白 (`voff`, 页面顶到拼合图顶, 像素) 折进第一块的
/// `gap_before`, 使拼合图 y=0 对齐页面顶端. 视觉位置不变: 各块 sheet y
/// 都加 `voff`, 调用方应把显示用的 `voff_target` 置 0 (顶对齐).
/// `voff <= 0` 时原样拷贝.
pub fn fold_voff_into_leading_gap(
    heights: &[(String, u32)],
    layout: &[BlockAdjust],
    voff: i32,
) -> Vec<BlockAdjust> {
    let mut layout = layout.to_vec();
    if voff <= 0 {
        return layout;
    }
    if let Some((first_id, _)) = heights.first() {
        let g = gap_of(&layout, first_id);
        set_gap(&mut layout, first_id, g + voff);
    }
    layout
}

/// 计算拖动块本体一帧的新间距分配: 优先消耗被拖动块与相邻块之间*已有*的
/// 间距, 只有真的把某处间距吃到 0 (撞上了那个块) 才会继续波及、推动
/// *再往后* (或再往前) 的下一个块, 如此逐块传递, 直到吸收完这次拖动的
/// 位移量或者到达链条尽头为止; 任何一个没有被"撞到"的块 (以及它自己的
/// `gap_before`) 完全不受影响, 绝对位置分毫不动. 两个方向完全对称,
/// 只是传递方向相反.
///
/// 向下 (`delta >= 0`): 被拖动块自己的 `gap_before` 直接加 `delta` (它是
/// 唯一被鼠标直接拖动的把手, 没有上限). 从它*后面*第一个块开始依次尝试用
/// 各自已有的 `gap_before` 吸收这次增量: 某块的间距足够吸收剩余量就地
/// 打住 (它和它后面所有块绝对位置都不变); 吸收不完就把它自己的间距榨干
/// (归零, 该块因此被"撞到", 自己也要跟着往下挪), 剩余量继续找它后面
/// 那一个块吸收, 直至链条尽头 (最后一块之后不再有人吸收, 直接体现为
/// 拼合图整体变高, 不设上限). 链条以外没有被波及的所有块, 包括拼合图
/// 自身坐标系下"更前面"的所有块, 位置分毫不动; 但拼合图整体变高后, 若
/// 启用了底色居中合成, 宿主重新居中会让画面在画布上整体上移 (留白自动
/// 收缩), 因此这些"更前面"的块的*绝对*位置反而会跟着变——用
/// `voff_shift_delta` 精确抵消掉这一部分 (向下拖恒为 0, 因为向下拖只会
/// 让拼合图变高, 不会碰到画布最前端或外部居中留白, 所以只需要让手动偏移
/// 保持不变即可精确抵消; 精确值由宿主结合真实宽高比 `frame_size` 重算,
/// 这里不做任何假设).
///
/// 向上 (`delta < 0`, 与上面镜像): 从被拖动块自己面向"上一个块"的间距
/// 开始尝试吸收, 吸收不完就依次往前传递, 直至到达画布最前端 (第一块的
/// `gap_before`) 也吸收不完时, 先消耗底色居中留白 (`extra_room`);
/// 居中留白也用尽即已顶到页面 Y=0, 剩余位移丢弃 (拖到页顶就停, 不再写成
/// `gap_after` 去缩小内部块 — 那套只留给向下拖过页底).
/// 被拖动块的*下一个*块通过把内部吸收量 + 居中留白消耗量加到它自己的
/// `gap_before` 上, 保持绝对位置不变.
///
/// 调用方若已用 [`fold_voff_into_leading_gap`] 把 `voff` 折进第一块
/// `gap_before` (页面绝对坐标, y=0 即页顶), 应传 `extra_room = 0`.
///
/// `heights`/`start_layout` 均取自拖动起点的快照 (不做增量累加, 每帧都从
/// 同一个起点重算, 避免多帧误差累积). `snap` 只作用在*被拖动的块*上
/// (它自己的 `gap_before` 回 0, 或它去贴住下一块); 其它块的间距按守恒
/// 精确补偿, 绝不再各自吸附 —— 否则没在拖的块会跟着晃.
pub fn redistribute_for_block_move(
    heights: &[(String, u32)],
    start_layout: &[BlockAdjust],
    region_id: &str,
    extra_room: i32,
    delta: i32,
    snap: impl Fn(i32) -> i32,
) -> BlockMoveResult {
    let mut layout = start_layout.to_vec();
    let Some(idx) = heights.iter().position(|(id, _)| id == region_id) else {
        return BlockMoveResult { layout, voff_shift_delta: 0 };
    };

    if delta >= 0 {
        let mut overflow = delta;
        if let Some((last_id, _)) = heights.last() {
            let ga = gap_after_of(&layout, last_id);
            if ga > 0 && overflow > 0 {
                let absorbed = ga.min(overflow);
                set_gap_after(&mut layout, last_id, ga - absorbed);
                overflow -= absorbed;
            }
        }
        if overflow > 0 {
            let own = gap_of(&layout, region_id);
            let snapped_own = snap(own + overflow);
            set_gap(&mut layout, region_id, snapped_own);
            let actual = snapped_own - own;
            if actual > 0 {
                let mut rest = actual;
                let mut last_absorbed: Option<String> = None;
                for (id, _) in &heights[idx + 1..] {
                    if rest <= 0 {
                        break;
                    }
                    let g = gap_of(&layout, id);
                    let absorbed = g.min(rest);
                    set_gap(&mut layout, id, g - absorbed);
                    rest -= absorbed;
                    last_absorbed = Some(id.clone());
                }
                // 贴住下一块: 被拖的块多走剩余间距, 下一块绝对位置不动.
                if let Some(id) = last_absorbed {
                    let rem = gap_of(&layout, &id);
                    if rem > 0 && snap(rem) == 0 {
                        let new_own = gap_of(&layout, region_id) + rem;
                        set_gap(&mut layout, region_id, new_own);
                        set_gap(&mut layout, &id, 0);
                    }
                }
            } else if actual < 0 {
                // 被拖的块吸附回上一块, 把让出来的空间补给下一块, 其它块不动.
                if let Some((nid, _)) = heights.get(idx + 1) {
                    let ng = gap_of(&layout, nid);
                    set_gap(&mut layout, nid, ng - actual);
                }
            }
        }
        BlockMoveResult { layout, voff_shift_delta: 0 }
    } else {
        let mut overflow = -delta;
        let mut total_absorbed = 0i32;
        let mut chain_ids: Vec<String> = Vec::with_capacity(idx + 1);
        chain_ids.push(region_id.to_string());
        for (id, _) in heights[..idx].iter().rev() {
            chain_ids.push(id.clone());
        }
        for id in &chain_ids {
            if overflow <= 0 {
                break;
            }
            let g = gap_of(&layout, id);
            let absorbed = g.min(overflow);
            let raw_new = g - absorbed;
            // 只吸附正在拖的块自己的剩余间距 (贴住上一块); 被撞开的
            // 更前面的块不吸附, 否则那些块会自己跳.
            let snapped = if id == region_id {
                snap(raw_new)
            } else {
                raw_new
            };
            let step_total = absorbed + (raw_new - snapped);
            set_gap(&mut layout, id, snapped);
            overflow -= step_total;
            total_absorbed += step_total;
        }
        overflow = overflow.max(0);
        let take_voff = overflow.min(extra_room.max(0));
        total_absorbed += take_voff;
        if let Some((nid, _)) = heights.get(idx + 1) {
            let ng = gap_of(&layout, nid);
            set_gap(&mut layout, nid, ng + total_absorbed);
        }
        BlockMoveResult { layout, voff_shift_delta: -take_voff }
    }
}

/// 拖上边界一帧: `extra_top += delta`, `gap_before -= delta` 以保持内容
/// 与其它块不动. 吸附只作用在被拖的那条边上 —— `extra_top → 0` (回到
/// 原内容顶) 或 `gap_before → 0` (贴住上一块/页顶); 两个目标同时落入
/// 容差时不吸 (跟着鼠标), 避免两边抢着吸导致边线来回跳.
pub fn resize_top_apply_delta(
    start_extra_top: i32,
    start_gap_before: i32,
    delta: i32,
    snap: impl Fn(i32) -> i32,
) -> (i32, i32) {
    let raw_extra = start_extra_top + delta;
    let raw_gap = start_gap_before - delta;
    let sum = start_extra_top + start_gap_before;
    let extra_hit = snap(raw_extra) != raw_extra;
    let gap_hit = snap(raw_gap) != raw_gap;
    match (extra_hit, gap_hit) {
        (true, false) => (0, sum.max(0)),
        (false, true) => (sum, 0),
        _ => (raw_extra, raw_gap),
    }
}

/// 拖下边界一帧: `extra_bottom += delta`. 有下一块时用它的 `gap_before`
/// (最后一块则用自己的 `gap_after`) 反向抵消, 先消耗空白, 其它块绝对
/// 位置不动; 间距吃完后才继续加高、挤开下一块. 吸附只作用在被拖的底边上.
pub fn resize_bottom_apply_delta(
    start_extra_bottom: i32,
    start_slack: i32,
    delta: i32,
    max_trim: i32,
    snap: impl Fn(i32) -> i32,
) -> (i32, i32) {
    let extra_min = -max_trim;
    let extra_clamped = (start_extra_bottom + delta).max(extra_min);
    let consumed = extra_clamped - start_extra_bottom;
    let raw_slack = start_slack - consumed;
    if raw_slack < 0 {
        if snap(extra_clamped) != extra_clamped {
            let extra = 0.max(extra_min);
            let slack = (start_slack + start_extra_bottom - extra).max(0);
            return (extra, slack);
        }
        return (extra_clamped, 0);
    }
    let sum = start_extra_bottom + start_slack;
    let extra_hit = snap(extra_clamped) != extra_clamped;
    let slack_hit = snap(raw_slack) != raw_slack;
    match (extra_hit, slack_hit) {
        (true, false) => (0.max(extra_min), sum.max(0)),
        (false, true) => (sum.max(extra_min), 0),
        _ => (extra_clamped, raw_slack),
    }
}

/// 一次「对齐到辅助线」任务: `(region_id, anchor_offset_sheet, target_canvas_y)`.
/// `anchor_offset_sheet` 是锚点相对该块 span 顶边的拼合图像素 (不含
/// `voff`); `target_canvas_y` 是辅助线的画布纵坐标. 调用方负责:
/// - 放入要对齐的块 (五线谱, 以及根数多于谱表时纳入的文字块);
/// - 块按从上到下 (span 顺序);
/// - 辅助线按当前纵坐标排序 (不是存储下标, 上下拖过之后仍按坐标配对).
pub type AlignAssignment = (String, i32, i32);

fn span_height(orig_h: i32, adj: &BlockAdjust) -> i32 {
    let (_, ext_top, content_h, ext_bottom, _) = effective_metrics(orig_h, adj);
    (ext_top + content_h + ext_bottom) as i32
}

struct AlignBlock {
    id: String,
    h: i32,
}

fn align_block_geoms(heights: &[(String, u32)], layout: &[BlockAdjust]) -> Vec<AlignBlock> {
    heights
        .iter()
        .map(|(id, h)| {
            let adj = BlockAdjust::find(layout, id).cloned().unwrap_or_default();
            AlignBlock {
                id: id.clone(),
                h: span_height(*h as i32, &adj),
            }
        })
        .collect()
}

fn assignment_map(assignments: &[AlignAssignment]) -> std::collections::HashMap<String, (i32, i32)> {
    assignments
        .iter()
        .cloned()
        .map(|(id, off, tgt)| (id, (off, tgt)))
        .collect()
}

fn orig_gaps(heights: &[(String, u32)], layout: &[BlockAdjust]) -> Vec<i32> {
    heights.iter().map(|(id, _)| gap_of(layout, id)).collect()
}

fn assigned_y0(off: i32, tgt: i32, page_h: i32, sh: Option<i32>) -> i32 {
    match sh {
        Some(sh) if page_h > 0 => {
            let t = clamp_guide_target(tgt, page_h);
            let anchor = ((t as f64) * (sh as f64) / (page_h as f64)).round() as i32;
            anchor - off
        }
        _ => tgt - off,
    }
}

/// 未配线的块保持原来的间距, 不要立刻贴到上一块后面: 否则脚注会把谱行
/// 从辅助线上挤开, 或把拼合图撑高后被迫缩尺.
fn try_place(
    items: &[AlignBlock],
    assign: &std::collections::HashMap<String, (i32, i32)>,
    orig_gaps: &[i32],
    page_h: i32,
    sh: Option<i32>,
) -> Option<Vec<i32>> {
    let mut y0s = vec![0i32; items.len()];
    let mut prev_end = 0i32;
    for (i, item) in items.iter().enumerate() {
        let y0 = if let Some(&(off, tgt)) = assign.get(&item.id) {
            assigned_y0(off, tgt, page_h, sh)
        } else {
            let g = if i == 0 {
                0
            } else {
                orig_gaps.get(i).copied().unwrap_or(0).max(0)
            };
            prev_end + g
        };
        if y0 < 0 || y0 < prev_end {
            return None;
        }
        if let Some(limit) = sh {
            if y0 + item.h > limit {
                return None;
            }
        }
        y0s[i] = y0;
        prev_end = y0 + item.h;
    }
    if sh.is_none() && page_h > 0 && prev_end > page_h {
        return None;
    }
    Some(y0s)
}

fn place_best_effort(
    items: &[AlignBlock],
    assign: &std::collections::HashMap<String, (i32, i32)>,
    orig_gaps: &[i32],
) -> Vec<i32> {
    let mut y0s = vec![0i32; items.len()];
    let mut prev_end = 0i32;
    for (i, item) in items.iter().enumerate() {
        let y0 = if let Some(&(off, tgt)) = assign.get(&item.id) {
            (tgt - off).max(prev_end).max(0)
        } else {
            let g = if i == 0 {
                0
            } else {
                orig_gaps.get(i).copied().unwrap_or(0).max(0)
            };
            prev_end + g
        };
        y0s[i] = y0.max(prev_end);
        prev_end = y0s[i] + item.h;
    }
    y0s
}

fn clamp_guide_target(target: i32, page_h: i32) -> i32 {
    target.clamp(1, (page_h - 1).max(1))
}

fn min_scale_k(
    items: &[AlignBlock],
    assign: &std::collections::HashMap<String, (i32, i32)>,
    orig_gaps: &[i32],
    page_h: i32,
) -> f32 {
    let mut k = 1.0f32;
    let mut i = 0usize;
    let mut prefix = 0i32;
    while i < items.len() && !assign.contains_key(&items[i].id) {
        if i > 0 {
            prefix += orig_gaps.get(i).copied().unwrap_or(0).max(0);
        }
        prefix += items[i].h;
        i += 1;
    }
    let mut prev: Option<(i32, i32, i32)> = None;
    let mut between = 0i32;
    while i < items.len() {
        if let Some(&(off, tgt)) = assign.get(&items[i].id) {
            let t = clamp_guide_target(tgt, page_h);
            k = k.max(off as f32 / t as f32);
            k = k.max((items[i].h - off) as f32 / (page_h - t).max(1) as f32);
            if let Some((pt, po, ph)) = prev {
                k = k.max((ph - po + off + between) as f32 / (t - pt).max(1) as f32);
            } else {
                k = k.max((prefix + off) as f32 / t as f32);
            }
            prev = Some((t, off, items[i].h));
            between = 0;
            i += 1;
        } else {
            between += orig_gaps.get(i).copied().unwrap_or(0).max(0);
            between += items[i].h;
            i += 1;
        }
    }
    if let Some((t, off, h)) = prev {
        k = k.max((h - off + between) as f32 / (page_h - t).max(1) as f32);
    }
    k.max(1.0)
}

fn layout_from_y0s(
    heights: &[(String, u32)],
    start_layout: &[BlockAdjust],
    y0s: &[i32],
    sheet_h: Option<i32>,
) -> Vec<BlockAdjust> {
    let mut layout = start_layout.to_vec();
    let mut prev_end = 0i32;
    for (i, (id, orig_h)) in heights.iter().enumerate() {
        let adj = BlockAdjust::find(&layout, id).cloned().unwrap_or_default();
        let h = span_height(*orig_h as i32, &adj);
        let y0 = y0s.get(i).copied().unwrap_or(prev_end);
        set_gap(&mut layout, id, (y0 - prev_end).max(0));
        prev_end = y0 + h;
    }
    if let Some((last_id, _)) = heights.last() {
        let ga = sheet_h.map(|s| (s - prev_end).max(0)).unwrap_or(0);
        set_gap_after(&mut layout, last_id, ga);
    }
    layout
}

/// 「对齐到辅助线」核心几何, 纯函数 (不依赖 GUI 状态). 先把 `voff` 折进
/// 第一块 `gap_before` (页面绝对坐标, y=0 即页顶), 再按 `assignments`
/// 一次算好各块 `y0`, 不走逐块碰撞级联 (避免先对齐的块被后一块挤偏).
///
/// `page_h > 0` 时优先保持页面按宽定高: 锚点画布坐标 = 拼合图坐标.
/// 若这样会顶出页顶、块重叠、或拼合图高过页面 (预览会缩小从而导致全体
/// 偏移), 则改用页高锁定: 加高拼合图使 `(y0+offset) * page_h / sh =
/// target`, 显示宽度随 `content_scale` 缩小. `page_h == 0` 时没有页面锁,
/// 只在拼合图坐标系落位, 顶不出则贴住.
/// 返回新布局与累计的 `voff_shift_delta` (加到起点的 `voff_target` 上;
/// 折进绝对坐标后为 `-voff`, 把显示顶对齐到页顶).
pub fn align_blocks_to_targets(
    heights: &[(String, u32)],
    start_layout: &[BlockAdjust],
    voff: i32,
    assignments: &[AlignAssignment],
    page_h: i32,
) -> (Vec<BlockAdjust>, i32) {
    let voff = voff.max(0);
    let folded = fold_voff_into_leading_gap(heights, start_layout, voff);
    let voff_shift = -voff;
    if heights.is_empty() {
        return (folded, voff_shift);
    }
    let items = align_block_geoms(heights, &folded);
    let assign = assignment_map(assignments);
    if assign.is_empty() {
        return (folded, voff_shift);
    }
    let gaps = orig_gaps(heights, &folded);
    let page_h = page_h.max(0);

    if let Some(y0s) = try_place(&items, &assign, &gaps, page_h, None) {
        return (layout_from_y0s(heights, &folded, &y0s, None), voff_shift);
    }
    if page_h > 0 {
        let k = min_scale_k(&items, &assign, &gaps, page_h);
        let mut sh = (k * page_h as f32).ceil() as i32;
        sh = sh.max(page_h + 1);
        let sh_max = page_h.saturating_mul(32).max(sh);
        while sh <= sh_max {
            if let Some(y0s) = try_place(&items, &assign, &gaps, page_h, Some(sh)) {
                return (layout_from_y0s(heights, &folded, &y0s, Some(sh)), voff_shift);
            }
            sh += 1;
        }
    }
    let y0s = place_best_effort(&items, &assign, &gaps);
    (layout_from_y0s(heights, &folded, &y0s, None), voff_shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heights(hs: &[(&str, u32)]) -> Vec<(String, u32)> {
        hs.iter().map(|(id, h)| (id.to_string(), *h)).collect()
    }

    #[test]
    fn compute_spans_plain_stack_matches_naive_sum() {
        let hs = heights(&[("a", 30), ("b", 40)]);
        let spans = compute_spans(&hs, &[]);
        assert_eq!(spans, vec![("a".into(), 0, 29), ("b".into(), 30, 69)]);
    }

    #[test]
    fn compute_spans_honors_gap_and_extend() {
        let hs = heights(&[("a", 30), ("b", 40)]);
        let layout = vec![
            BlockAdjust::default(),
            BlockAdjust {
                region_id: "b".into(),
                extra_top: 0,
                extra_bottom: 5,
                gap_before: 10,
                ..Default::default()
            },
        ];
        let spans = compute_spans(&hs, &layout);
        assert_eq!(spans[0], ("a".into(), 0, 29));
        // b 前面多 10px 间距, 从 40 开始; 底边多扩 5px, 共 45px 高.
        assert_eq!(spans[1], ("b".into(), 40, 84));
    }

    fn gap(layout: &[BlockAdjust], id: &str) -> i32 {
        BlockAdjust::find(layout, id).map(|a| a.gap_before).unwrap_or(0)
    }

    fn after(layout: &[BlockAdjust], id: &str) -> i32 {
        BlockAdjust::find(layout, id).map(|a| a.gap_after).unwrap_or(0)
    }

    #[test]
    fn block_move_down_grows_own_gap_and_leaves_others_alone_when_no_existing_slack() {
        // chain = [a, b, c], 都紧贴 (无间距). 拖 b 往下 10px: 只加到 b 自己
        // 的 gap_before, a 完全不受影响; c 后面没有既有间距可吸收, 只能
        // 跟着顺移 (相对 b 的距离不变). voff_shift_delta 恒为 0 (向下拖
        // 不涉及外部居中留白的借用, 具体如何抵消拼合图变高后的居中收缩
        // 交给宿主结合真实宽高比精确计算).
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let r = redistribute_for_block_move(&hs, &[], "b", 0, 10, |v| v);
        assert_eq!(r.voff_shift_delta, 0);
        assert_eq!(gap(&r.layout, "a"), 0);
        assert_eq!(gap(&r.layout, "b"), 10);
        assert_eq!(gap(&r.layout, "c"), 0);
        let spans = compute_spans(&hs, &r.layout);
        assert_eq!(spans[0], ("a".into(), 0, 29));
        assert_eq!(spans[1], ("b".into(), 40, 69));
        assert_eq!(spans[2], ("c".into(), 70, 99)); // 与 b 的距离不变, 跟着顺移
    }

    #[test]
    fn block_move_down_absorbs_existing_gap_further_down_the_chain() {
        // b 下方 (c 前面) 已经有 12px 间距. 拖 a 往下 5px (a 是链条最前一块,
        // 没有"上一块"可比较, 直接加到自己身上); a 下面 b 紧贴 a, 没有既有
        // 间距可吸收, 只能跟着顺移 5px; 但这 5px 推力会继续传递给 b-c 间的
        // 既有间距去吸收 (12 -> 7), 使得 c 的*绝对位置*保持不变 (不是它的
        // gap_before 不变).
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "c".into(), gap_before: 12, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "a", 0, 5, |v| v);
        assert_eq!(gap(&r.layout, "a"), 5);
        assert_eq!(gap(&r.layout, "b"), 0); // 自己没有间距可吸收, 跟着顺移
        assert_eq!(gap(&r.layout, "c"), 7); // 12 - 5, 吸收掉推力
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[2].1, spans_old[2].1); // c 绝对位置完全不变
    }

    #[test]
    fn block_move_down_consumes_adjacent_gap_before_pushing_further_blocks() {
        // 用户报告的场景: 2 下方 (3 前面) 已经空了一段 (12px). 拖 1 往下移
        // 20px: 1 自己的 gap_before 直接加 20; 2 紧贴 1, 没有自己的间距可
        // 吸收, 只能跟着顺移 20px (绝对位置改变); 2-3 之间已有的 12px 间距
        // 应该被优先吸收掉 —— 3 的绝对位置应该尽量保持不变, 而不是"3 与 2
        // 的相对位置不变、一起被拖走 20px".
        let hs = heights(&[("p1", 30), ("p2", 30), ("p3", 30)]);
        let layout = vec![BlockAdjust { region_id: "p3".into(), gap_before: 12, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "p1", 0, 20, |v| v);
        assert_eq!(gap(&r.layout, "p1"), 20);
        assert_eq!(gap(&r.layout, "p2"), 0); // 跟着 p1 顺移 (自己没有间距可吸收)
        // p2 顺移 20px 之后, p2-p3 之间原有的 12px 缺口只剩 12-20 = -8,
        // 已经不够, 所以 p3 自己的间距被榨干到 0, 还需要再往下挪 8px.
        assert_eq!(gap(&r.layout, "p3"), 0);
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[2].1 - spans_old[2].1, 8); // p3 只挪了溢出的 8px, 不是整 20px
    }

    #[test]
    fn block_move_down_leaves_untouched_blocks_completely_fixed_when_gap_absorbs_all() {
        // 同上场景但只拖 1 往下 5px (小于既有的 12px 缺口): 2 跟着顺移 5px,
        // 但这段位移被 2-3 间的既有间距完全吸收, 3 应该分毫不动.
        let hs = heights(&[("p1", 30), ("p2", 30), ("p3", 30)]);
        let layout = vec![BlockAdjust { region_id: "p3".into(), gap_before: 12, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "p1", 0, 5, |v| v);
        assert_eq!(gap(&r.layout, "p3"), 7); // 12 - 5, 部分吸收, 未榨干
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[2].1, spans_old[2].1); // p3 绝对位置完全不变
    }

    #[test]
    fn block_move_up_only_consumes_own_leading_gap_and_opens_gap_below() {
        // chain = [a, b, c] (c 被拖动), b-c 之间已有 20px 间距 (即 c 自己的
        // gap_before). 把 c 往上拖 30px: 只吃 c 自己面向 b 的这 20px, a-b
        // 间距碰都不碰; c 顶多上移 20px 就贴住 b 拖不动了; c 与它下一块
        // 之间 (若存在) 要新增同样 20px 的间距保持下一块绝对位置不变.
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30), ("d", 30)]);
        let layout = vec![BlockAdjust { region_id: "c".into(), gap_before: 20, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "c", 0, -30, |v| v);
        assert_eq!(gap(&r.layout, "a"), 0); // 完全没被波及
        assert_eq!(gap(&r.layout, "c"), 0);
        assert_eq!(gap(&r.layout, "d"), 20); // 只补偿内部吃掉的 20px
        assert_eq!(after(&r.layout, "d"), 0); // 越过页顶的 10px 丢掉, 不再写 gap_after
        assert_eq!(r.voff_shift_delta, 0);
    }

    #[test]
    fn block_move_up_cascades_backward_only_when_own_gap_insufficient() {
        // b-c 之间只有 5px (c 自己的 gap_before), 但 a-b 之间已经有 40px
        // (b 自己的 gap_before). 把 c 往上拖 30px: 先吃满 c 自己的 5px (c
        // 撞上 b, 跟着 b 一起继续往上, 不会裁进任何块的内容), 剩下 25px
        // 从 a-b 间距里扣, a 完全不受影响.
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30), ("d", 30)]);
        let layout = vec![
            BlockAdjust { region_id: "b".into(), gap_before: 40, ..Default::default() },
            BlockAdjust { region_id: "c".into(), gap_before: 5, ..Default::default() },
        ];
        let r = redistribute_for_block_move(&hs, &layout, "c", 0, -30, |v| v);
        assert_eq!(gap(&r.layout, "a"), 0);
        assert_eq!(gap(&r.layout, "b"), 15); // 40 - 25
        assert_eq!(gap(&r.layout, "c"), 0); // 5 全部吃掉
        assert_eq!(gap(&r.layout, "d"), 30); // 总吸收量守恒 (5+25)
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[0].1, spans_old[0].1); // a 绝对位置不变
        assert_eq!(spans_new[3].1, spans_old[3].1); // d 绝对位置不变
        assert_eq!(spans_old[2].1 - spans_new[2].1, 30); // c 精确上移 30px
    }

    #[test]
    fn block_move_up_stops_at_page_top_without_any_slack() {
        // 刚加载的原始紧贴状态、又没有居中留白: 往上拖到页顶就停, 不写
        // 底端留白去缩小内部块.
        let hs = heights(&[("a", 30), ("b", 30)]);
        let r = redistribute_for_block_move(&hs, &[], "b", 0, -30, |v| v);
        assert_eq!(gap(&r.layout, "a"), 0);
        assert_eq!(gap(&r.layout, "b"), 0);
        assert_eq!(after(&r.layout, "b"), 0);
        assert_eq!(r.voff_shift_delta, 0);
    }

    #[test]
    fn block_move_up_snap_bonus_is_folded_into_next_compensation() {
        // b 自己的留白有 50px, 往上拖 45px: 吸收后剩余 5px, 在吸附容差
        // (<=6px 归零) 内直接吸附成 0 —— 相当于"多吃了" 5px 的吸附奖励.
        // 补给下一块 c 的量必须是*吸附后实际吃掉的总量* (50px, 不是请求量
        // 45px), 否则 c 会因为吸附奖励多移动的部分而跟着错位.
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 50, ..Default::default() }];
        let snap = |v: i32| if v.abs() <= 6 { 0 } else { v };
        let r = redistribute_for_block_move(&hs, &layout, "b", 0, -45, snap);
        assert_eq!(gap(&r.layout, "a"), 0); // 完全没被波及
        assert_eq!(gap(&r.layout, "b"), 0); // 5px 残余被吸附掉
        assert_eq!(gap(&r.layout, "c"), 50); // 守恒: 补偿吸附后的真实总量
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[2].1, spans_old[2].1); // c 绝对位置不变
    }

    #[test]
    fn block_move_up_does_not_snap_next_block_compensation() {
        // 往上只拖 4px (在吸附容差内): 被拖的 b 自己的剩余间距 46 不吸,
        // 补给 c 的 4px 也不能被吸成 0, 否则 c 会跟着 b 晃.
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 50, ..Default::default() }];
        let snap = |v: i32| if v.abs() <= 6 { 0 } else { v };
        let r = redistribute_for_block_move(&hs, &layout, "b", 0, -4, snap);
        assert_eq!(gap(&r.layout, "b"), 46);
        assert_eq!(gap(&r.layout, "c"), 4);
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[2].1, spans_old[2].1);
    }

    #[test]
    fn block_move_down_snap_does_not_pull_following_block() {
        // c 前面有 12px. 把 b 往下拖 4px: b 自己的 gap 被吸回 0, 不能
        // 再从 c 的间距里扣 4px (那样 c 会往上跳来"对吸").
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "c".into(), gap_before: 12, ..Default::default() }];
        let snap = |v: i32| if v.abs() <= 6 { 0 } else { v };
        let r = redistribute_for_block_move(&hs, &layout, "b", 0, 4, snap);
        assert_eq!(gap(&r.layout, "b"), 0);
        assert_eq!(gap(&r.layout, "c"), 12);
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[0].1, spans_old[0].1);
        assert_eq!(spans_new[2].1, spans_old[2].1);
    }

    #[test]
    fn block_move_down_snaps_dragged_to_next_block() {
        // c 前面 20px, 把 b 往下拖 17px: 剩下 3px 由 b 多走去贴住 c,
        // c 绝对位置不变.
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "c".into(), gap_before: 20, ..Default::default() }];
        let snap = |v: i32| if v.abs() <= 6 { 0 } else { v };
        let r = redistribute_for_block_move(&hs, &layout, "b", 0, 17, snap);
        assert_eq!(gap(&r.layout, "b"), 20);
        assert_eq!(gap(&r.layout, "c"), 0);
        let spans_old = compute_spans(&hs, &layout);
        let spans_new = compute_spans(&hs, &r.layout);
        assert_eq!(spans_new[2].1, spans_old[2].1);
        assert_eq!(spans_new[1].1 - spans_old[1].1, 20);
    }

    #[test]
    fn resize_top_snaps_only_the_dragged_edge() {
        let snap = |v: i32| if v.abs() <= 6 { 0 } else { v };
        // 只靠近 extra_top=0: 边吸回去, gap 用守恒补上, 内容位置不变.
        let (e, g) = resize_top_apply_delta(0, 40, 4, snap);
        assert_eq!((e, g), (0, 40));
        // 只靠近 gap=0: 边去贴住上一块, extra 用守恒补上.
        let (e, g) = resize_top_apply_delta(0, 40, 37, snap);
        assert_eq!((e, g), (40, 0));
        // 两个目标都在容差里 (中间地带): 不吸, 跟着鼠标, 避免两边对吸.
        let (e, g) = resize_top_apply_delta(0, 8, 4, snap);
        assert_eq!((e, g), (4, 4));
        // 不在容差里: 原样.
        let (e, g) = resize_top_apply_delta(0, 40, 20, snap);
        assert_eq!((e, g), (20, 20));
    }

    #[test]
    fn resize_bottom_consumes_next_gap_so_next_stays() {
        let hs = heights(&[("a", 30), ("b", 40)]);
        let old_layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 20, ..Default::default() }];
        let old_spans = compute_spans(&hs, &old_layout);
        let (extra, slack) = resize_bottom_apply_delta(0, 20, 8, 29, |v| v);
        assert_eq!((extra, slack), (8, 12));
        let new_layout = vec![
            BlockAdjust { region_id: "a".into(), extra_bottom: extra, ..Default::default() },
            BlockAdjust { region_id: "b".into(), gap_before: slack, ..Default::default() },
        ];
        let new_spans = compute_spans(&hs, &new_layout);
        assert_eq!(new_spans[0].2 - old_spans[0].2, 8); // a 底边跟手
        assert_eq!(new_spans[1].1, old_spans[1].1); // b 绝对位置不变
        assert_eq!(block_content_shifts(&hs, &old_layout, &new_layout).get("b"), None);
    }

    #[test]
    fn resize_bottom_pushes_next_only_after_gap_exhausted() {
        let (extra, slack) = resize_bottom_apply_delta(0, 20, 25, 29, |v| v);
        assert_eq!((extra, slack), (25, 0));
        let hs = heights(&[("a", 30), ("b", 40)]);
        let old_layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 20, ..Default::default() }];
        let old_spans = compute_spans(&hs, &old_layout);
        let new_layout = vec![
            BlockAdjust { region_id: "a".into(), extra_bottom: extra, ..Default::default() },
            BlockAdjust { region_id: "b".into(), gap_before: slack, ..Default::default() },
        ];
        let new_spans = compute_spans(&hs, &new_layout);
        // 空白 20px 吃完后还多 5px, b 被挤下去 5px.
        assert_eq!(new_spans[1].1 - old_spans[1].1, 5);
    }

    #[test]
    fn resize_bottom_shrinking_opens_next_gap_next_stays() {
        let hs = heights(&[("a", 30), ("b", 40)]);
        let old_layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 20, ..Default::default() }];
        let old_spans = compute_spans(&hs, &old_layout);
        let (extra, slack) = resize_bottom_apply_delta(0, 20, -6, 29, |v| v);
        assert_eq!((extra, slack), (-6, 26));
        let new_layout = vec![
            BlockAdjust { region_id: "a".into(), extra_bottom: extra, ..Default::default() },
            BlockAdjust { region_id: "b".into(), gap_before: slack, ..Default::default() },
        ];
        let new_spans = compute_spans(&hs, &new_layout);
        assert_eq!(new_spans[1].1, old_spans[1].1);
    }

    #[test]
    fn resize_bottom_snaps_only_the_dragged_edge() {
        let snap = |v: i32| if v.abs() <= 6 { 0 } else { v };
        let (e, s) = resize_bottom_apply_delta(0, 40, 4, 29, snap);
        assert_eq!((e, s), (0, 40));
        let (e, s) = resize_bottom_apply_delta(0, 40, 37, 29, snap);
        assert_eq!((e, s), (40, 0));
        let (e, s) = resize_bottom_apply_delta(0, 8, 4, 29, snap);
        assert_eq!((e, s), (4, 4));
    }

    #[test]
    fn block_move_down_voff_shift_is_always_zero() {
        // 向下拖不受 extra_room (居中留白) 影响, voff_shift_delta 恒为 0
        // (交由宿主结合真实宽高比精确抵消居中收缩).
        let hs = heights(&[("a", 30), ("b", 30)]);
        let r1 = redistribute_for_block_move(&hs, &[], "b", 0, 21, |v| v);
        let r2 = redistribute_for_block_move(&hs, &[], "b", 999, 21, |v| v);
        assert_eq!(r1.voff_shift_delta, 0);
        assert_eq!(r2.voff_shift_delta, 0);
    }

    #[test]
    fn block_move_up_consumes_center_padding_after_internal_gap_exhausted() {
        // 内部留白只有 5px, 底色居中留白 (block_voff) 还有 40px: 往上
        // 拖 30px, 先吃满 5px 内部留白, 剩下 25px 从居中留白里扣, 一比一
        // (不再是 /2 近似, 精确值由宿主结合真实宽高比重算, 这里只需要
        // 精确报告"借了多少外部留白").
        let hs = heights(&[("a", 30), ("b", 30)]);
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 5, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "b", 40, -30, |v| v);
        assert_eq!(gap(&r.layout, "b"), 0);
        assert_eq!(r.voff_shift_delta, -25);
    }

    #[test]
    fn block_move_up_stops_after_center_padding() {
        // 两段留白加起来只有 5 + 10 = 15px, 想往上拖 100px: 内部 5px +
        // 居中 10px 吃完即已到页顶, 剩余位移丢掉, 不写 gap_after.
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 5, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "b", 10, -100, |v| v);
        assert_eq!(gap(&r.layout, "b"), 0);
        assert_eq!(gap(&r.layout, "c"), 15); // 内部 5 + 居中 10
        assert_eq!(after(&r.layout, "c"), 0);
        assert_eq!(r.voff_shift_delta, -10);
    }

    #[test]
    fn block_move_up_ignores_center_padding_when_internal_gap_already_enough() {
        // 内部留白本身就够用时 (20px 留白, 只要往上拖 10px), 不应该去动
        // 居中留白 (哪怕它还有很多).
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 20, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "b", 999, -10, |v| v);
        assert_eq!(gap(&r.layout, "b"), 10);
        assert_eq!(gap(&r.layout, "c"), 10);
        assert_eq!(r.voff_shift_delta, 0);
    }

    #[test]
    fn block_content_shifts_move_cascades_to_following_blocks_only() {
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        let old_layout: Vec<BlockAdjust> = vec![];
        // b 往下拖开 10px 间距 (中间空腔背景色填充): a 不动, b/c 各自的
        // 内容都跟着往下挪 10px (c 是被 b 的高度变化级联带动的, b 自己是
        // gap_before 本身产生的位移, 两种来源, 但增量公式对两者一视同仁).
        let new_layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 10, ..Default::default() }];
        let deltas = block_content_shifts(&hs, &old_layout, &new_layout);
        assert_eq!(deltas.get("a"), None);
        assert_eq!(deltas.get("b"), Some(&10));
        assert_eq!(deltas.get("c"), Some(&10));
    }

    #[test]
    fn resize_top_edge_tracks_drag_while_content_and_following_blocks_stay_fixed() {
        // 模拟 `MaskToolApp::apply_block_resize_top` 的公式: `extra_top`
        // 按 delta 增减的同时, 用同一个块自己的 `gap_before` 反向抵消,
        // 使得"边线跟手"而内容与它后面所有块的绝对位置分毫不动
        // (下边界同样先消耗与下一块之间的空白).
        let hs = heights(&[("a", 30), ("b", 30)]);
        let start_gap_before = 10i32; // a 前面已有的留白 (画布顶端)
        let start_extra_top = 0i32;
        let old_layout = vec![BlockAdjust { region_id: "a".into(), gap_before: start_gap_before, ..Default::default() }];
        let old_spans = compute_spans(&hs, &old_layout);

        // 往下拖裁剪 6px (delta = -6): 边线 (a 的 comp_y0) 应该跟着往下
        // 移动 6px, a 的内容 (裁剪后剩下的部分) 与 b 的位置分毫不动.
        let delta = -6i32;
        let new_layout_crop = vec![BlockAdjust {
            region_id: "a".into(),
            extra_top: start_extra_top + delta,
            gap_before: start_gap_before - delta,
            ..Default::default()
        }];
        let new_spans_crop = compute_spans(&hs, &new_layout_crop);
        // 边线跟手: 往下拖 (delta 为负) 边线 (a 的 comp_y0) 应该往下移动
        // 相应距离, 即 y0 的变化量是 `-delta` (正数, 往下).
        assert_eq!(new_spans_crop[0].1 - old_spans[0].1, -delta as i64);
        assert_eq!(new_spans_crop[0].2, old_spans[0].2); // a 自己的底边不动
        assert_eq!(new_spans_crop[1].1, old_spans[1].1); // b 的顶边不动
        let deltas_crop = block_content_shifts(&hs, &old_layout, &new_layout_crop);
        assert_eq!(deltas_crop.get("a"), None); // 内容本身不挪动 (只是被盖住/露出多少)
        assert_eq!(deltas_crop.get("b"), None);

        // 往上拖扩展 4px (delta = +4): 同理, 边线往上移动, 吃掉一部分
        // a 自己已有的顶端留白, b 的位置依然分毫不动.
        let delta = 4i32;
        let new_layout_extend = vec![BlockAdjust {
            region_id: "a".into(),
            extra_top: start_extra_top + delta,
            gap_before: start_gap_before - delta,
            ..Default::default()
        }];
        let new_spans_extend = compute_spans(&hs, &new_layout_extend);
        assert_eq!(new_spans_extend[0].1 - old_spans[0].1, -delta as i64);
        assert_eq!(new_spans_extend[0].2, old_spans[0].2);
        assert_eq!(new_spans_extend[1].1, old_spans[1].1);
        let deltas_extend = block_content_shifts(&hs, &old_layout, &new_layout_extend);
        assert_eq!(deltas_extend.get("a"), None);
        assert_eq!(deltas_extend.get("b"), None);
    }

    #[test]
    fn fold_voff_into_leading_gap_is_noop_when_voff_zero_or_negative() {
        let hs = heights(&[("a", 30), ("b", 30)]);
        let layout = vec![BlockAdjust { region_id: "a".into(), gap_before: 8, ..Default::default() }];
        assert_eq!(fold_voff_into_leading_gap(&hs, &layout, 0), layout);
        assert_eq!(fold_voff_into_leading_gap(&hs, &layout, -10), layout);
    }

    #[test]
    fn fold_voff_into_leading_gap_adds_to_first_block_only() {
        let hs = heights(&[("a", 30), ("b", 30)]);
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 12, ..Default::default() }];
        let folded = fold_voff_into_leading_gap(&hs, &layout, 500);
        assert_eq!(gap(&folded, "a"), 500);
        assert_eq!(gap(&folded, "b"), 12);
        let spans = compute_spans(&hs, &folded);
        // 折进后 sheet y=0 即页顶; 第一块顶边在 500, 与原先 canvas Y=voff 一致.
        assert_eq!(spans[0], ("a".into(), 500, 529));
        assert_eq!(spans[1], ("b".into(), 542, 571));
    }

    #[test]
    fn resize_top_first_block_uses_page_top_as_absolute_origin() {
        // 起始: 第一块顶边在页面 Y=500 (voff=500, gap_before=0). 折进绝对
        // 坐标后往上拖 100px: 边线到 Y=400, 底边与下一块绝对位置不动.
        let hs = heights(&[("a", 30), ("b", 30)]);
        let voff = 500i32;
        let folded = fold_voff_into_leading_gap(&hs, &[], voff);
        let delta = 100i32;
        let new_layout = vec![BlockAdjust {
            region_id: "a".into(),
            extra_top: delta,
            gap_before: gap(&folded, "a") - delta,
            ..Default::default()
        }];
        let new_spans = compute_spans(&hs, &new_layout);
        assert_eq!(new_spans[0].1, (voff - delta) as i64); // 顶边跟手, 以页顶为 0
        assert_eq!(new_spans[0].2, (voff + 30 - 1) as i64); // a 底边仍在原 canvas 位置
        assert_eq!(new_spans[1].1, (voff + 30) as i64); // b 不动
    }

    #[test]
    fn block_content_shifts_resize_top_moves_own_content_not_own_y0() {
        let hs = heights(&[("a", 30), ("b", 30)]);
        let old_layout: Vec<BlockAdjust> = vec![];
        // 裁掉 a 顶部 8px (extra_top = -8): a 的内容 (裁剪后剩下的部分)
        // 整体相对画布上移 8px (可见起点从 y0 提到更靠近 y0 本身, 即视觉
        // 上"往上移了 8px"); b 的 y0 也跟着往上挪 8px (a 变矮了).
        let new_layout = vec![BlockAdjust { region_id: "a".into(), extra_top: -8, ..Default::default() }];
        let deltas = block_content_shifts(&hs, &old_layout, &new_layout);
        assert_eq!(deltas.get("a"), Some(&-8));
        assert_eq!(deltas.get("b"), Some(&-8));
    }

    #[test]
    fn block_content_shifts_resize_bottom_does_not_move_own_content() {
        let hs = heights(&[("a", 30), ("b", 30)]);
        let old_layout: Vec<BlockAdjust> = vec![];
        // 扩展 a 底部 5px (extra_bottom = 5): a 自己的内容起点不受影响
        // (只是变高了), b 的 y0 跟着往下挪 5px.
        let new_layout = vec![BlockAdjust { region_id: "a".into(), extra_bottom: 5, ..Default::default() }];
        let deltas = block_content_shifts(&hs, &old_layout, &new_layout);
        assert_eq!(deltas.get("a"), None);
        assert_eq!(deltas.get("b"), Some(&5));
    }

    #[test]
    fn stitch_with_layout_dimensions_match_compute_spans() {
        use image::{Rgb, RgbImage};
        let a = RgbImage::from_pixel(10, 20, Rgb([10, 20, 30]));
        let b = RgbImage::from_pixel(10, 15, Rgb([40, 50, 60]));
        let parts = vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())];
        let layout = vec![BlockAdjust {
            region_id: "b".into(),
            extra_top: 3,
            extra_bottom: -2,
            gap_before: 5,
            ..Default::default()
        }];
        let combined = stitch_with_layout(&parts, &layout, 200);
        let heights = vec![("a".to_string(), 20u32), ("b".to_string(), 15u32)];
        let spans = compute_spans(&heights, &layout);
        let expected_h = spans.last().unwrap().2 + 1 + trailing_gap(&heights, &layout) as i64;
        assert_eq!(combined.height() as i64, expected_h);
        assert_eq!(combined.width(), 10);
        // 未调整的块 (a) 像素应原样保留 (走了跳过克隆的快路径).
        assert_eq!(*combined.get_pixel(0, 0), Rgb([10, 20, 30]));
        assert_eq!(*combined.get_pixel(9, 19), Rgb([10, 20, 30]));
    }

    #[test]
    fn stitch_with_stats_first_block_leading_gap_stays_plain_white() {
        use image::{Rgb, RgbImage};
        // 画布最前端 (a 前面, 没有"上一块") 的留白不是"两块之间"的间隙,
        // 不参与任何采样/填色计算, 保持画布初始化时的纯白 (有底色层时
        // 宿主合成阶段会跳过贴图这一段, 让底色透出来, 见 `apply_bg`).
        let a = RgbImage::from_pixel(10, 20, Rgb([10, 20, 30]));
        let parts = vec![("a".to_string(), a.clone())];
        let stats = vec![PieceStats { top: ([9.0, 9.0, 9.0], [1.0, 1.0, 1.0]), bottom: ([9.0, 9.0, 9.0], [1.0, 1.0, 1.0]) }];
        let layout = vec![BlockAdjust { region_id: "a".into(), gap_before: 6, ..Default::default() }];
        let combined = stitch_with_stats(&parts, &stats, &layout);
        assert_eq!(*combined.get_pixel(0, 0), Rgb([255, 255, 255]));
        assert_eq!(*combined.get_pixel(0, 5), Rgb([255, 255, 255]));
        // 从第 6 行起才是 a 自己的内容.
        assert_eq!(*combined.get_pixel(0, 6), Rgb([10, 20, 30]));
    }

    #[test]
    fn stitch_with_stats_gap_between_two_real_blocks_still_gets_smart_fill() {
        use image::{Rgb, RgbImage};
        // b 前面 (b 与 a 之间) 的间隙是真正"两块之间"的间隙, 依然要智能
        // 识别背景色填充 (不是纯白).
        let a = RgbImage::from_pixel(10, 20, Rgb([10, 20, 30]));
        let b = RgbImage::from_pixel(10, 15, Rgb([40, 50, 60]));
        let parts = vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())];
        let stats = vec![
            PieceStats { top: ([10.0, 20.0, 30.0], [1.0, 1.0, 1.0]), bottom: ([1.0, 2.0, 3.0], [1.0, 1.0, 1.0]) },
            PieceStats { top: ([40.0, 50.0, 60.0], [1.0, 1.0, 1.0]), bottom: ([4.0, 5.0, 6.0], [1.0, 1.0, 1.0]) },
        ];
        let layout = vec![BlockAdjust { region_id: "b".into(), gap_before: 6, ..Default::default() }];
        let combined = stitch_with_stats(&parts, &stats, &layout);
        // 间隙上半段贴 a 的底边色, 下半段贴 b 的顶边色, 均不是纯白.
        assert_ne!(*combined.get_pixel(0, 20), Rgb([255, 255, 255]));
        assert_ne!(*combined.get_pixel(0, 25), Rgb([255, 255, 255]));
    }

    #[test]
    fn block_move_first_block_down_gains_top_padding() {
        // 拖第一块 (a) 本身往下移: 没有"上一块", 直接加到自己 (即画布
        // 顶端留白) 上, 没有上限; a 后面没有既有间距可吸收, b 跟着顺移.
        let hs = heights(&[("a", 20), ("b", 20)]);
        let r = redistribute_for_block_move(&hs, &[], "a", 0, 8, |v| v);
        assert_eq!(gap(&r.layout, "a"), 8);
        assert_eq!(r.voff_shift_delta, 0);
        let spans = compute_spans(&hs, &r.layout);
        assert_eq!(spans[0], ("a".into(), 8, 27));
        assert_eq!(spans[1], ("b".into(), 28, 47)); // b 随 a 一起顺移
    }

    #[test]
    fn block_move_first_block_up_consumes_own_padding_directly() {
        // 拖第一块 (a) 本身往上移: 它自己的 gap_before 就是画布顶端留白,
        // 直接消耗; 补给下一块 b 的量与吃掉的量守恒.
        let hs = heights(&[("a", 20), ("b", 20)]);
        let layout = vec![BlockAdjust { region_id: "a".into(), gap_before: 15, ..Default::default() }];
        let r = redistribute_for_block_move(&hs, &layout, "a", 0, -20, |v| v);
        assert_eq!(gap(&r.layout, "a"), 0);
        assert_eq!(gap(&r.layout, "b"), 15);
        assert_eq!(after(&r.layout, "b"), 0); // 超出页顶的 5px 丢掉
        assert_eq!(r.voff_shift_delta, 0);
    }

    #[test]
    fn align_blocks_to_targets_moves_each_to_its_own_guide_by_geometric_center() {
        // 两块各 40px 高, 锚点取几何中线 (offset=19), 目标辅助线 100 / 300.
        let hs = heights(&[("a", 40), ("b", 40)]);
        let (layout, _voff_shift) = align_blocks_to_targets(
            &hs,
            &[],
            0,
            &[("a".into(), 19, 100), ("b".into(), 19, 300)],
            0,
        );
        let spans = compute_spans(&hs, &layout);
        let (_, a0, a1) = spans[0].clone();
        let (_, b0, b1) = spans[1].clone();
        assert_eq!((a0 + a1) / 2, 100);
        assert_eq!((b0 + b1) / 2, 300);
    }

    #[test]
    fn align_blocks_to_targets_skips_unassigned_blocks() {
        let hs = heights(&[("a", 40), ("b", 40), ("c", 40)]);
        // 只对齐 a/b (例如 c 不是五线谱): c 自己没有调整项; 仍可能因 a/b
        // 挪动而顺移, 那是堆叠布局的自然结果.
        let (layout, _) = align_blocks_to_targets(
            &hs,
            &[],
            0,
            &[("a".into(), 19, 50), ("b".into(), 19, 150)],
            0,
        );
        let c_adj = BlockAdjust::find(&layout, "c");
        assert!(c_adj.is_none() || c_adj.unwrap().is_noop());
    }

    #[test]
    fn align_blocks_to_targets_pairs_by_given_order_not_storage_index() {
        // 辅助线存储顺序是「下面那条, 上面那条」(模拟把上面拖到下面之后
        // 的 vec 顺序); 调用方按纵坐标排好后再传入, 顶块仍应对到 80 而
        // 不是 200.
        let hs = heights(&[("top", 40), ("bot", 40)]);
        let (layout, _) = align_blocks_to_targets(
            &hs,
            &[],
            0,
            &[("top".into(), 19, 80), ("bot".into(), 19, 200)],
            0,
        );
        let spans = compute_spans(&hs, &layout);
        assert_eq!((spans[0].1 + spans[0].2) / 2, 80);
        assert_eq!((spans[1].1 + spans[1].2) / 2, 200);
    }

    #[test]
    fn align_blocks_to_targets_scales_when_page_width_cannot_fit() {
        // 块比页顶到辅助线的距离更高: 保持页宽会顶出页顶. 应按页高缩尺,
        // 锚点画布坐标仍落在辅助线上.
        let hs = heights(&[("a", 250), ("b", 250)]);
        let page_h = 400i32;
        let (layout, _) = align_blocks_to_targets(
            &hs,
            &[],
            0,
            &[("a".into(), 125, 100), ("b".into(), 125, 300)],
            page_h,
        );
        let sh = sheet_height(&hs, &layout) as i32;
        assert!(sh > page_h);
        let spans = compute_spans(&hs, &layout);
        let canvas = |y0: i64, off: i32| {
            (((y0 as i32 + off) as f64) * (page_h as f64) / (sh as f64)).round() as i32
        };
        assert_eq!(canvas(spans[0].1, 125), 100);
        assert_eq!(canvas(spans[1].1, 125), 300);
    }

    #[test]
    fn align_blocks_to_targets_keeps_page_width_when_it_fits() {
        let hs = heights(&[("a", 40), ("b", 40)]);
        let (layout, _) = align_blocks_to_targets(
            &hs,
            &[],
            0,
            &[("a".into(), 19, 100), ("b".into(), 19, 300)],
            400,
        );
        let sh = sheet_height(&hs, &layout);
        assert!(sh <= 400);
        let spans = compute_spans(&hs, &layout);
        assert_eq!((spans[0].1 + spans[0].2) / 2, 100);
        assert_eq!((spans[1].1 + spans[1].2) / 2, 300);
        assert_eq!(after(&layout, "b"), 0);
    }

    #[test]
    fn align_unassigned_text_keeps_staff_on_guide_when_sheet_overflows() {
        // 只对齐谱表, 下面的文字块不配辅助线. 两块加起来高过页面时要缩尺,
        // 谱表锚点的画布坐标仍须落在线上.
        let hs = heights(&[("staff", 250), ("text", 200)]);
        let page_h = 400i32;
        let (layout, _) = align_blocks_to_targets(
            &hs,
            &[],
            0,
            &[("staff".into(), 125, 180)],
            page_h,
        );
        let sh = sheet_height(&hs, &layout) as i32;
        let spans = compute_spans(&hs, &layout);
        let hit = if sh > page_h {
            (((spans[0].1 as i32 + 125) as f64) * (page_h as f64) / (sh as f64)).round() as i32
        } else {
            spans[0].1 as i32 + 125
        };
        assert_eq!(hit, 180);
    }
}
