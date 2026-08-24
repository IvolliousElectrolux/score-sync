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
}

impl BlockAdjust {
    pub fn is_noop(&self) -> bool {
        self.extra_top == 0 && self.extra_bottom == 0 && self.gap_before == 0
    }

    fn find<'a>(layout: &'a [BlockAdjust], region_id: &str) -> Option<&'a BlockAdjust> {
        layout.iter().find(|a| a.region_id == region_id)
    }
}

/// 单块调整后的有效高度 (裁剪/扩展合计) 与间距, 纯几何, 不涉及像素.
fn effective_metrics(orig_h: i32, adj: &BlockAdjust) -> (u32, u32, u32, u32, u32) {
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

/// 把组内各块 (已解码像素) 按顺序竖向拼接, 应用 `layout` 中的裁剪/扩展/
/// 间距, 新增区域用该块自身背景色统计合成填充. `layout` 为空时等价于
/// 单纯首尾相接 (不做任何裁剪/扩展/加间距).
pub fn stitch_with_layout(
    parts: &[(String, RgbImage)],
    layout: &[BlockAdjust],
    ink_threshold: i32,
) -> RgbImage {
    const SAMPLE_ROWS: u32 = 32;
    struct Piece {
        gap_before: u32,
        gap_seed: u64,
        gap_stats: ([f32; 3], [f32; 3]),
        ext_top: u32,
        ext_bottom: u32,
        top_seed: u64,
        top_stats: ([f32; 3], [f32; 3]),
        bottom_seed: u64,
        bottom_stats: ([f32; 3], [f32; 3]),
        content: RgbImage,
    }
    let mut pieces: Vec<Piece> = Vec::with_capacity(parts.len());
    for (rid, img) in parts {
        let adj = BlockAdjust::find(layout, rid).cloned().unwrap_or_default();
        let (gap_before, ext_top, content_h, ext_bottom, trim_top) =
            effective_metrics(img.height() as i32, &adj);
        let content = if trim_top > 0 || content_h != img.height() {
            image::imageops::crop_imm(img, 0, trim_top, img.width(), content_h).to_image()
        } else {
            img.clone()
        };
        let top_stats = bg_fill::sample_bg_stats(
            &bg_fill::edge_sample(&content, true, SAMPLE_ROWS),
            ink_threshold,
        );
        let bottom_stats = bg_fill::sample_bg_stats(
            &bg_fill::edge_sample(&content, false, SAMPLE_ROWS),
            ink_threshold,
        );
        pieces.push(Piece {
            gap_before,
            gap_seed: bg_fill::seed_from(&format!("{rid}:gap")),
            gap_stats: top_stats,
            ext_top,
            ext_bottom,
            top_seed: bg_fill::seed_from(&format!("{rid}:top")),
            top_stats,
            bottom_seed: bg_fill::seed_from(&format!("{rid}:bottom")),
            bottom_stats,
            content,
        });
    }
    let max_w = pieces.iter().map(|p| p.content.width()).max().unwrap_or(1);
    let total_h: u32 = pieces
        .iter()
        .map(|p| p.gap_before + p.ext_top + p.content.height() + p.ext_bottom)
        .sum();
    let mut combined = RgbImage::from_pixel(max_w, total_h.max(1), image::Rgb([255, 255, 255]));
    let mut yy: i64 = 0;
    for p in &pieces {
        if p.gap_before > 0 {
            let fill = bg_fill::synth_fill(max_w, p.gap_before, p.gap_stats.0, p.gap_stats.1, p.gap_seed);
            image::imageops::replace(&mut combined, &fill, 0, yy);
            yy += p.gap_before as i64;
        }
        if p.ext_top > 0 {
            let fill = bg_fill::synth_fill(max_w, p.ext_top, p.top_stats.0, p.top_stats.1, p.top_seed);
            image::imageops::replace(&mut combined, &fill, 0, yy);
            yy += p.ext_top as i64;
        }
        let src = if p.content.width() != max_w {
            let mut canvas =
                RgbImage::from_pixel(max_w, p.content.height(), image::Rgb([255, 255, 255]));
            image::imageops::replace(&mut canvas, &p.content, 0, 0);
            canvas
        } else {
            p.content.clone()
        };
        image::imageops::replace(&mut combined, &src, 0, yy);
        yy += p.content.height() as i64;
        if p.ext_bottom > 0 {
            let fill =
                bg_fill::synth_fill(max_w, p.ext_bottom, p.bottom_stats.0, p.bottom_stats.1, p.bottom_seed);
            image::imageops::replace(&mut combined, &fill, 0, yy);
            yy += p.ext_bottom as i64;
        }
    }
    combined
}

/// 整体移动一个块 (`idx`): 增大/减小它与上一块之间的间距. 向下移动没有
/// 上限 (自然把该块之后的内容一起顺移, 即"拉开间距"); 向上移动最多回到
/// 间距为 0 (不能反向撞进上一块). 返回实际生效的位移量 (可能因夹到 0
/// 而小于请求值).
pub fn move_block(layout: &mut [BlockAdjust], idx: usize, delta: i32) -> i32 {
    let Some(a) = layout.get_mut(idx) else {
        return 0;
    };
    let applied = delta.max(-a.gap_before);
    a.gap_before += applied;
    applied
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
            },
        ];
        let spans = compute_spans(&hs, &layout);
        assert_eq!(spans[0], ("a".into(), 0, 29));
        // b 前面多 10px 间距, 从 40 开始; 底边多扩 5px, 共 45px 高.
        assert_eq!(spans[1], ("b".into(), 40, 84));
    }

    #[test]
    fn move_block_down_cascades_following_blocks() {
        let mut layout = vec![
            BlockAdjust {
                region_id: "a".into(),
                ..Default::default()
            },
            BlockAdjust {
                region_id: "b".into(),
                ..Default::default()
            },
            BlockAdjust {
                region_id: "c".into(),
                ..Default::default()
            },
        ];
        let hs = heights(&[("a", 30), ("b", 30), ("c", 30)]);
        // 把 b 往下移 10px (即在 a/b 之间插入 10px 间距): b、c 都顺移 10px.
        let applied = move_block(&mut layout, 1, 10);
        assert_eq!(applied, 10);
        let spans = compute_spans(&hs, &layout);
        assert_eq!(spans[0], ("a".into(), 0, 29));
        assert_eq!(spans[1], ("b".into(), 40, 69));
        assert_eq!(spans[2], ("c".into(), 70, 99));

        // 再把 b 往上移回去, 最多回到间距为 0 (不能撞进 a 内部).
        let applied2 = move_block(&mut layout, 1, -20);
        assert_eq!(applied2, -10);
        let spans2 = compute_spans(&hs, &layout);
        assert_eq!(spans2[1], ("b".into(), 30, 59));
        assert_eq!(spans2[2], ("c".into(), 60, 89));
    }

    #[test]
    fn move_block_first_block_can_gain_top_padding() {
        let mut layout = vec![
            BlockAdjust {
                region_id: "a".into(),
                ..Default::default()
            },
            BlockAdjust {
                region_id: "b".into(),
                ..Default::default()
            },
        ];
        let hs = heights(&[("a", 20), ("b", 20)]);
        let applied = move_block(&mut layout, 0, 8);
        assert_eq!(applied, 8);
        let spans = compute_spans(&hs, &layout);
        assert_eq!(spans[0], ("a".into(), 8, 27));
        assert_eq!(spans[1], ("b".into(), 28, 47)); // b 随 a 一起顺移
    }
}
