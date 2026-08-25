//! 辅助线: 每个组合独立的一组水平参考线, 仅在蒙版画布内可见, 不参与
//! 导出/合成. 用于手动对齐各组合内的分块, 使视频里对应位置的分块尽量
//! 保持一致的竖直位置 (理想情况下每页谱行数量可控制一致).

use serde::{Deserialize, Serialize};

/// 两根时距顶: 页面高度的 5/17 (与旧版相同).
const TOP_MARGIN_NUM: f32 = 5.0;
const TOP_MARGIN_DEN: f32 = 17.0;
/// 两根时距底: 页面高度的 4/15. 比相对中线镜像 (5/17) 更靠下.
const BOT_MARGIN_NUM: f32 = 4.0;
const BOT_MARGIN_DEN: f32 = 15.0;

/// 单个组合的辅助线集合 (随组合持久化, key = group_id, 见
/// `DocState::group_guides`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GuideState {
    /// 各辅助线的纵坐标 (拼合图坐标系, 不含 `block_voff` 居中偏移); 渲染
    /// /命中测试前按需排序, 存储顺序不保证有序.
    #[serde(default)]
    pub lines: Vec<i32>,
    /// 锁定字段保留以兼容旧工程; 当前 UI 不再使用 (开启即可拖).
    #[serde(default)]
    pub locked: bool,
}

impl GuideState {
    pub fn is_default(&self) -> bool {
        self.lines.is_empty() && !self.locked
    }

    pub fn sorted_lines(&self) -> Vec<i32> {
        let mut v = self.lines.clone();
        v.sort_unstable();
        v
    }

    /// 按块数放置辅助线. 两根时距顶 5/17、距底 4/15 (下端比镜像对称更
    /// 下沉); 其它根数在这两端之间均分 (一根取两端中点).
    pub fn set_staff_slots(&mut self, n: u32, img_h: i32) {
        self.lines.clear();
        if n == 0 || img_h <= 0 {
            return;
        }
        let h = img_h as f32;
        let y_max = (img_h - 1).max(0) as f32;
        let push = |y: f32, lines: &mut Vec<i32>| {
            lines.push(y.round().clamp(0.0, y_max) as i32);
        };
        let y0 = h * TOP_MARGIN_NUM / TOP_MARGIN_DEN;
        let y1 = h - h * BOT_MARGIN_NUM / BOT_MARGIN_DEN;
        if n == 1 {
            push((y0 + y1) * 0.5, &mut self.lines);
            return;
        }
        let span = y1 - y0;
        let denom = (n - 1) as f32;
        for i in 0..n {
            push(y0 + span * (i as f32) / denom, &mut self.lines);
        }
    }

    /// 新增一条辅助线 (放在拼合图竖向中点; 若已存在极接近的线则不重复
    /// 添加). 返回是否真的新增了.
    pub fn add_at(&mut self, y: i32) -> bool {
        if self.lines.iter().any(|&v| (v - y).abs() < 2) {
            return false;
        }
        self.lines.push(y);
        true
    }

    /// 删除若干条辅助线 (按纵坐标匹配, 容差 1px).
    pub fn remove_near(&mut self, ys: &[i32]) {
        self.lines
            .retain(|&v| !ys.iter().any(|&y| (v - y).abs() <= 1));
    }

    /// 把本页辅助线按画布高度比例映到另一页. 同高则原样拷贝; 高度非法时
    /// 原样返回. 用于「同样根数的页面同步位置」.
    pub fn scaled_to(&self, src_h: i32, dst_h: i32) -> Self {
        if src_h <= 0 || dst_h <= 0 || src_h == dst_h {
            return self.clone();
        }
        let y_max = (dst_h - 1).max(0) as f32;
        let lines = self
            .lines
            .iter()
            .map(|&y| {
                ((y as f32) * (dst_h as f32) / (src_h as f32))
                    .round()
                    .clamp(0.0, y_max) as i32
            })
            .collect();
        Self {
            lines,
            locked: self.locked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_staff_slots_two_lines_decouple_top_and_bottom_margins() {
        let mut g = GuideState::default();
        g.set_staff_slots(2, 1440);
        // 顶: 1440 × 5/17 = 423.529 → 424; 底: 1440 − 1440×4/15 = 1056.
        assert_eq!(g.sorted_lines(), vec![424, 1056]);
    }

    #[test]
    fn set_staff_slots_three_lines_split_between_asymmetric_outers() {
        let mut g = GuideState::default();
        g.set_staff_slots(3, 1440);
        // 两端仍是两根时的位置, 中间均分 (424+1056)/2 = 740, 不是页心 720.
        assert_eq!(g.sorted_lines(), vec![424, 740, 1056]);
    }

    #[test]
    fn set_staff_slots_one_line_is_midpoint_of_two_line_span() {
        let mut g = GuideState::default();
        g.set_staff_slots(1, 1440);
        assert_eq!(g.sorted_lines(), vec![740]);
    }

    #[test]
    fn set_staff_slots_zero_clears() {
        let mut g = GuideState { lines: vec![10, 20], locked: false };
        g.set_staff_slots(0, 1000);
        assert!(g.lines.is_empty());
    }

    #[test]
    fn add_at_dedups_close_lines() {
        let mut g = GuideState::default();
        assert!(g.add_at(100));
        assert!(!g.add_at(101));
        assert_eq!(g.lines, vec![100]);
    }

    #[test]
    fn is_default_false_once_locked_even_without_lines() {
        let g = GuideState { lines: vec![], locked: true };
        assert!(!g.is_default());
    }

    #[test]
    fn scaled_to_same_height_is_clone() {
        let g = GuideState { lines: vec![100, 400], locked: false };
        assert_eq!(g.scaled_to(1000, 1000), g);
    }

    #[test]
    fn scaled_to_maps_proportionally() {
        let g = GuideState { lines: vec![100, 400], locked: false };
        let s = g.scaled_to(1000, 2000);
        assert_eq!(s.sorted_lines(), vec![200, 800]);
    }
}
