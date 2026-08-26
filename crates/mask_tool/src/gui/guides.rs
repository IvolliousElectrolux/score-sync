//! 辅助线: 按当前组合里「能判定为五线谱」的块数自动生成, 两端按距顶
//! 5/17、距底 4/15 放置, 拖动时按比例联动 (非镜像). 辅助线本身只是画布
//! 坐标系里的固定横线, 不参与导出合成.

use super::*;

/// 蒙版工具请宿主代办的辅助线/对齐操作 (全局开启、全局对齐、同步位置).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideHostCmd {
    /// 左键开关且「全局开启」已勾选: 给所有组合生成辅助线.
    EnableAll,
    /// 左键开关且「全局开启」已勾选: 清空所有组合的辅助线 (同时关掉开关).
    DisableAll,
    /// 菜单勾选「全局开启」: 开则铺到全部组合, 关则全部关掉. 可撤.
    SetGlobal(bool),
    /// 菜单勾选「同步同根数位置」.
    SetSync(bool),
    /// 对齐菜单「全局对齐」.
    AlignAll,
    /// 当前页辅助线位置或根数刚改完, 请同步到同样根数的其它页.
    SyncPositions,
    /// 本页撤重碰到带宿主令牌的快照, 请回滚对应的全局辅助线状态.
    UndoGlobal(u64),
    /// 本页重做碰到带宿主令牌的快照, 请重放对应的全局辅助线状态.
    RedoGlobal(u64),
    /// 本页没有可撤的蒙版操作, 请撤最近一次全局辅助线操作.
    UndoGlobalFallback,
    /// 本页没有可重做的蒙版操作, 请重做最近一次全局辅助线操作.
    RedoGlobalFallback,
}

impl MaskToolApp {
    pub fn guides_on(&self) -> bool {
        !self.guides.lines.is_empty()
    }

    pub fn is_guide_dragging(&self) -> bool {
        matches!(self.drag, Some(DragKind::GuideMove { .. }))
    }

    pub fn take_guide_host_cmd(&mut self) -> Option<GuideHostCmd> {
        self.guide_host_cmd.take()
    }

    pub fn set_guide_prefs(&mut self, global: bool, sync: bool) {
        self.guides_global = global;
        self.guides_sync = sync;
    }

    /// 宿主改完全部组合的线之后, 只更新当前页显示, 不重载会话 (以免冲掉撤重).
    pub fn apply_live_guides(&mut self, guides: GuideState) {
        self.guides = guides;
        self.guide_selected.clear();
        self.guide_hover = None;
    }

    pub fn guides_global(&self) -> bool {
        self.guides_global
    }

    pub fn guides_sync(&self) -> bool {
        self.guides_sync
    }

    pub fn guide_menu(&self) -> Option<(f32, f32)> {
        self.guide_menu
    }

    pub fn align_menu(&self) -> Option<(f32, f32)> {
        self.align_menu
    }

    pub fn close_guide_menus(&mut self) {
        self.guide_menu = None;
        self.align_menu = None;
    }

    pub fn staff_block_count(&self) -> usize {
        self.staff_block_ids().len()
    }

    pub fn block_count(&self) -> usize {
        self.block_heights.len()
    }

    pub fn guide_count(&self) -> usize {
        self.guides.lines.len()
    }

    /// 当前组合里重新判定为五线谱的块 (自上而下, 不看存盘 kind).
    /// 有预计算条带锚点时用那个, 缺的 id 当非谱表, 不要因为组合里多了
    /// 文字块就把已有谱行锚点整组丢掉、改去缩放后的预览上重检.
    pub(super) fn staff_block_ids(&self) -> Vec<String> {
        if !self.piece_staff_ys.is_empty() {
            return self
                .block_heights
                .iter()
                .filter(|(id, _)| matches!(self.piece_staff_ys.get(id), Some(Some(_))))
                .map(|(id, _)| id.clone())
                .collect();
        }
        let Some(rgb) = self.rgb_image.as_ref() else {
            return Vec::new();
        };
        let (x0, x1) = self.sheet_x_range();
        let thr = crate::staff::DEFAULT_INK_THRESHOLD;
        self.block_spans()
            .into_iter()
            .filter(|(_, y0, y1)| {
                crate::staff::looks_like_staff(rgb, *y0 as i32, *y1 as i32, x0, x1, thr)
            })
            .map(|(rid, ..)| rid)
            .collect()
    }

    fn emit_host(&mut self, cmd: GuideHostCmd, cx: &mut Context<Self>) {
        self.guide_host_cmd = Some(cmd);
        cx.notify();
    }

    pub(crate) fn open_guide_menu(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        self.align_menu = None;
        self.guide_menu = Some((x, y));
        cx.notify();
    }

    pub(crate) fn open_align_menu(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        self.guide_menu = None;
        self.align_menu = Some((x, y));
        cx.notify();
    }

    /// 开: 按五线谱块数放置辅助线 (两端 5/17 与 4/15); 文字/脚注默认不
    /// 占线, 需在菜单里加根数才纳入对齐. 「全局开启」勾选时改请宿主铺到
    /// 全部组合 (再点一次即关掉开关).
    pub(crate) fn guide_toggle(&mut self, cx: &mut Context<Self>) {
        self.close_guide_menus();
        if self.guides_global {
            if self.guides_on() {
                self.emit_host(GuideHostCmd::DisableAll, cx);
            } else {
                self.emit_host(GuideHostCmd::EnableAll, cx);
            }
            return;
        }
        if self.guides_on() {
            self.push_undo();
            self.guides.lines.clear();
            self.guide_selected.clear();
            self.status = "已关闭辅助线.".into();
            cx.notify();
            return;
        }
        if self.img_h < 2 || !self.has_block_pieces() {
            self.status = "没有分块, 无法开启辅助线.".into();
            cx.notify();
            return;
        }
        let n = {
            let staff_n = self.staff_block_ids().len() as u32;
            if staff_n > 0 {
                staff_n
            } else {
                self.block_heights.len() as u32
            }
        };
        if n == 0 {
            self.status = "没有分块, 无法开启辅助线.".into();
            cx.notify();
            return;
        }
        self.push_undo();
        self.guides.set_staff_slots(n, self.img_h as i32);
        self.guide_selected.clear();
        self.status = format!("已开启辅助线 ({n} 条).").into();
        if self.guides_sync {
            self.guide_host_cmd = Some(GuideHostCmd::SyncPositions);
        }
        cx.notify();
    }

    /// 手动改当前页根数. 只允许在「五线谱块数, 总块数」闭区间内增减,
    /// 用来给歌词/说明文字也留一条线 (文字用上下边界中线对齐).
    pub fn set_guide_count(&mut self, n: u32, cx: &mut Context<Self>) {
        if !self.guides_on() {
            return;
        }
        let staff_n = self.staff_block_ids().len() as u32;
        let block_n = self.block_heights.len() as u32;
        if staff_n == 0 || block_n == 0 || self.img_h < 2 {
            return;
        }
        let max_n = block_n.max(staff_n);
        let n = n.clamp(staff_n, max_n);
        if n == self.guides.lines.len() as u32 {
            return;
        }
        self.push_undo();
        self.guides.set_staff_slots(n, self.img_h as i32);
        self.guide_selected.clear();
        self.status = format!("本页辅助线 {n} 条.").into();
        if self.guides_sync {
            self.guide_host_cmd = Some(GuideHostCmd::SyncPositions);
        }
        cx.notify();
    }

    pub fn request_set_global(&mut self, on: bool, cx: &mut Context<Self>) {
        self.guides_global = on;
        self.emit_host(GuideHostCmd::SetGlobal(on), cx);
    }

    pub fn request_set_sync(&mut self, on: bool, cx: &mut Context<Self>) {
        self.guides_sync = on;
        self.emit_host(GuideHostCmd::SetSync(on), cx);
    }

    pub fn request_align_all(&mut self, cx: &mut Context<Self>) {
        self.close_guide_menus();
        self.emit_host(GuideHostCmd::AlignAll, cx);
    }

    pub(super) fn begin_guide_drag(&mut self, idx: usize) {
        let Some(&start_y) = self.guides.lines.get(idx) else {
            return;
        };
        let orig_lines = self.guides.lines.clone();
        self.guide_selected.clear();
        self.guide_selected.insert(idx);
        self.drag = Some(DragKind::GuideMove {
            idx,
            start_y,
            orig_lines,
            undid: false,
        });
    }

    /// 命中测试: 屏幕/图像坐标 `iy` (画布坐标系) 附近有没有辅助线, 容差
    /// `tol` (图像像素, 与分块拖动共用 `ViewXform::edge_tol`).
    pub(super) fn guide_hit_test(&self, iy: f32, tol: f32) -> Option<usize> {
        self.guides
            .lines
            .iter()
            .enumerate()
            .map(|(i, &y)| (i, (y as f32 - iy).abs()))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(i, _)| i)
    }

    /// 每帧按当前鼠标图像坐标重算辅助线位置 (夹在画布范围内), 并以画布
    /// 纵向中心为基准把其余线按比例联动 (默认上下边距不对称, 所以不是
    /// 镜像). 每次都从 `orig_lines` (拖动开始时的原始位置) 重新算, 不基于
    /// 上一帧已经缩放过的值累加. 拖的是正中那条 (分母过小) 时只动自己,
    /// 避免其余线被比例放大甩飞.
    pub(super) fn apply_guide_move(
        &mut self,
        idx: usize,
        start_y: i32,
        orig_lines: &[i32],
        undid: bool,
        iy: f32,
    ) -> bool {
        if idx >= self.guides.lines.len() {
            return undid;
        }
        let new_y = (iy.round() as i32).clamp(0, (self.img_h as i32 - 1).max(0));
        if new_y == self.guides.lines[idx] {
            return undid;
        }
        let mut undid = undid;
        if !undid {
            self.push_undo();
            undid = true;
        }
        self.guides.lines[idx] = new_y;
        let center = self.img_h as f32 / 2.0;
        let denom = center - start_y as f32;
        if denom.abs() < 1.0 {
            return undid;
        }
        let ratio = (center - new_y as f32) / denom;
        let n = self.guides.lines.len().min(orig_lines.len());
        for i in 0..n {
            if i == idx {
                continue;
            }
            let orig_y = orig_lines[i];
            let scaled = (center - (center - orig_y as f32) * ratio).round() as i32;
            self.guides.lines[i] = scaled.clamp(0, (self.img_h as i32 - 1).max(0));
        }
        undid
    }

    /// 优先用原始条带预计算锚点 (与全局对齐同一套); 缺锚点时才在预览
    /// 画布上重检. 脚注把拼合图撑高、页面缩小之后, 预览上重检会偏.
    /// 组合里多了尚未缓存的文字块时, 仍用已有谱行条带锚点, 文字当非谱表.
    fn current_block_align_anchors(&self) -> Vec<crate::staff::BlockAlignAnchor> {
        if !self.piece_staff_ys.is_empty() {
            return crate::staff::anchors_from_piece_ys(
                &self.block_heights,
                &self.block_layout,
                &self.piece_staff_ys,
            );
        }
        let Some(rgb) = self.rgb_image.as_ref() else {
            return Vec::new();
        };
        let (x0, x1) = self.sheet_x_range();
        let cs = self.content_scale_or_1();
        let thr = crate::staff::DEFAULT_INK_THRESHOLD;
        crate::staff::collect_block_align_anchors(rgb, &self.block_spans(), x0, x1, cs, thr)
    }

    /// 与左键「对齐」同一套输入, 供宿主全局对齐在后台逐页复用.
    pub fn current_align_input(&self) -> Option<(crate::staff::AlignGroupInput, i64)> {
        if !self.guides_on() || self.block_heights.is_empty() {
            return None;
        }
        Some((
            crate::staff::AlignGroupInput {
                heights: self.block_heights.clone(),
                layout: self.block_layout.clone(),
                voff: self.block_voff.max(0).min(i32::MAX as i64) as i32,
                page_h: if self.block_shows_bg {
                    self.img_h as i32
                } else {
                    0
                },
                anchors: self.current_block_align_anchors(),
                guide_lines: self.guides.lines.clone(),
            },
            self.voff_target,
        ))
    }

    /// 「对齐」: 按辅助线当前纵坐标从上到下配对 (不是存储下标).
    /// 一块一个谱行组时优先用大括号尖尖, 否则用该组重心; 一块多个谱行
    /// 组时用整块几何中心; 文字用上下边界中线.
    /// 根数等于五线谱块数时只动谱表; 根数更多时把文字块也纳入.
    /// 能保持页宽则保持; 否则按页高缩尺 (显示变窄) 也要把锚点落到线上.
    pub fn guide_align_current(&mut self, cx: &mut Context<Self>) {
        self.close_guide_menus();
        let Some((input, _)) = self.current_align_input() else {
            self.status = "没有辅助线或没有分块, 无法对齐.".into();
            cx.notify();
            return;
        };
        let aligned_n = crate::staff::assignments_for_guides(&input.anchors, &input.guide_lines).len();
        let Some((new_layout, voff_shift_delta)) = crate::staff::align_group(&input) else {
            self.status = "没有可对齐到辅助线的块.".into();
            cx.notify();
            return;
        };
        self.push_undo();
        let old_layout = self.block_layout.clone();
        self.block_layout = new_layout;
        self.voff_target += voff_shift_delta as i64;
        self.sync_masks_to_block_shift(&old_layout);
        self.refresh_preview_geom();
        self.hold_block_tile_preview();
        self.status = format!("已将 {aligned_n} 个块对齐到辅助线.").into();
        cx.notify();
    }

    /// 还原到没有任何拖动或对齐时该页应有的分块位置 (清空微调, 纵向居中
    /// 回到自然位置).
    pub fn reset_block_layout(&mut self, cx: &mut Context<Self>) {
        self.close_guide_menus();
        if self.block_heights.is_empty() {
            self.status = "没有分块, 无法还原.".into();
            cx.notify();
            return;
        }
        let natural = if let Some(bg) = self.block_bg.as_ref() {
            let sw = self.block_tiles.iter().map(|t| t.width).max().unwrap_or(1);
            let sh = crate::layout::sheet_height(&self.block_heights, &[]);
            apply_bg::process::natural_voff(
                sw,
                sh,
                bg.src_width,
                bg.src_height,
                bg.aspect_w,
                bg.aspect_h,
            )
        } else {
            0
        };
        let layout_noop = self.block_layout.iter().all(BlockAdjust::is_noop);
        if layout_noop && self.voff_target == natural {
            self.status = "本页已是初始状态.".into();
            cx.notify();
            return;
        }
        self.push_undo();
        let old_layout = self.block_layout.clone();
        self.block_layout.clear();
        self.voff_target = natural;
        self.sync_masks_to_block_shift(&old_layout);
        self.refresh_preview_geom();
        self.hold_block_tile_preview();
        self.status = "已还原本页分块到初始位置.".into();
        cx.notify();
    }

    /// 宿主同步「底色层」是否启用, 只用于导出按钮文案跟随.
    pub fn set_bg_applied(&mut self, enabled: bool) {
        self.bg_applied = enabled;
    }
}
