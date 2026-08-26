//! 框选/折线/画笔/橡皮, 选择与撤重.

use super::*;

impl MaskToolApp {
    pub fn cancel_poly_draft(&mut self, cx: &mut Context<Self>) {
        if self.eyedropper_armed {
            self.cancel_eyedropper(cx);
            return;
        }
        if self.poly_draft.is_none() {
            return;
        }
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = "已取消折线.".into();
        cx.notify();
    }

    /// 折线光标: 点数≥3 且靠近首点时吸附到首点.
    pub(super) fn poly_maybe_snap(&self, ix: f32, iy: f32) -> (f32, f32, bool) {
        let Some(draft) = &self.poly_draft else {
            return (ix, iy, false);
        };
        if draft.len() < 3 {
            return (ix, iy, false);
        }
        let (fx, fy) = draft[0];
        let xform = self.xform();
        let (sx, sy) = xform.image_to_screen(ix, iy);
        let (fsx, fsy) = xform.image_to_screen(fx, fy);
        let dist = (sx - fsx).hypot(sy - fsy);
        if dist <= POLY_SNAP_SCREEN_PX {
            (fx, fy, true)
        } else {
            (ix, iy, false)
        }
    }

    pub(super) fn finalize_poly(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.poly_draft.take() else {
            return;
        };
        self.poly_cursor = None;
        if draft.len() < 3 || self.img_w < 2 || self.img_h < 2 {
            self.status = "折线至少需要 3 个点.".into();
            cx.notify();
            return;
        }
        let iw = self.img_w as i32;
        let ih = self.img_h as i32;
        let clamp = |v: f32, hi: i32| -> i32 {
            v.round().clamp(0.0, (hi - 1).max(0) as f32) as i32
        };
        let pts: Vec<(i32, i32)> = draft
            .iter()
            .map(|(x, y)| (clamp(*x, iw), clamp(*y, ih)))
            .collect();
        // 去掉连续重复点
        let mut dedup: Vec<(i32, i32)> = Vec::new();
        for p in pts {
            if dedup.last().copied() != Some(p) {
                dedup.push(p);
            }
        }
        if dedup.len() < 3 {
            self.status = "折线顶点过少, 已取消.".into();
            cx.notify();
            return;
        }
        let mid = new_id();
        let bound_block = self.resolve_bound_block(draft[0].1, draft.last().unwrap().1);
        let mut m = MaskRect {
            id: mid.clone(),
            x0: 0,
            y0: 0,
            x1: 0,
            y1: 0,
            brush_points: Vec::new(),
            brush_radius: 0,
            color: self.mask_color,
            poly_points: dedup,
            opacity: self.mask_opacity,
            bound_block,
        };
        m.refresh_poly_bounds();
        self.push_undo();
        self.masks.push(m);
        self.selected.clear();
        self.selected.insert(mid);
        self.status = format!("蒙版 {} 个 (折线闭环)", self.masks.len()).into();
        cx.notify();
    }
    pub fn fit_to_view(&mut self, cx: &mut Context<Self>) {
        self.user_zoomed = false;
        self.zoom = 1.0;
        self.pan = point(0.0, 0.0);
        cx.notify();
    }

    pub fn toggle_draw_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Draw {
            ToolMode::Select
        } else {
            ToolMode::Draw
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_pan_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Pan {
            ToolMode::Select
        } else {
            ToolMode::Pan
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_brush_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Brush {
            ToolMode::Select
        } else {
            ToolMode::Brush
        };
        self.drag = None;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.brush_cursor = None;
        if self.mode != ToolMode::Brush {
            self.color_picker_open = false;
        }
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_eraser_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Eraser {
            ToolMode::Select
        } else {
            ToolMode::Eraser
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub fn toggle_poly_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = if self.mode == ToolMode::Poly {
            ToolMode::Select
        } else {
            ToolMode::Poly
        };
        self.drag = None;
        self.color_picker_open = false;
        self.poly_draft = None;
        self.poly_cursor = None;
        self.status = Self::mode_status(self.mode);
        cx.notify();
    }

    pub(super) fn mode_status(mode: ToolMode) -> SharedString {
        match mode {
            ToolMode::Draw => "框选".into(),
            ToolMode::Poly => "折线: 逐点连线, 靠近首点吸附闭环 (右键取消)".into(),
            ToolMode::Brush => "画笔 (拖动画布涂抹; 可改颜色/粗细)".into(),
            ToolMode::Eraser => "橡皮: 单击擦最上层 · 拖动擦光".into(),
            ToolMode::Select => format!(
                "选择 (可 {}多选 / Shift 拖选); 未选中任何蒙版时可直接拖动/拉伸分块",
                apply_bg::primary_mod()
            )
            .into(),
            ToolMode::Pan => "平移".into(),
        }
    }

    pub(super) fn brush_radius_px(&self) -> i32 {
        ((self.brush_size * 0.5).round() as i32).max(1)
    }

    pub(super) fn brush_size_max(&self) -> f32 {
        brush_size_max_for_image(self.img_w)
    }

    pub(super) fn clamp_brush_size(&mut self) {
        let max = self.brush_size_max();
        self.brush_size = self.brush_size.clamp(BRUSH_SIZE_MIN, max);
    }

    pub(super) fn set_brush_size_from_x(&mut self, x: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.brush_size_track.origin.x);
        let width = f32::from(self.brush_size_track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        self.brush_size = brush_size_from_t(t, BRUSH_SIZE_MIN, self.brush_size_max());
        cx.notify();
    }

    pub(super) fn append_brush_point(&mut self, id: &str, ix: f32, iy: f32) -> bool {
        let Some(m) = self.masks.iter_mut().find(|m| m.id == id) else {
            return false;
        };
        let px = ix.round() as i32;
        let py = iy.round() as i32;
        if let Some(&(lx, ly)) = m.brush_points.last() {
            let dx = (px - lx) as f32;
            let dy = (py - ly) as f32;
            // 过密采样没必要, 也减轻撤销快照体积.
            if dx * dx + dy * dy < 1.5 {
                return false;
            }
        }
        m.brush_points.push((px, py));
        m.refresh_brush_bounds();
        true
    }

    pub(super) fn hit_mask(&self, ix: f32, iy: f32) -> Option<String> {
        self.masks
            .iter()
            .rev()
            .find(|m| m.contains(ix, iy))
            .map(|m| m.id.clone())
    }

    /// 点擦: 只删最上层 (列表末尾优先).
    pub(super) fn erase_topmost_at(&mut self, ix: f32, iy: f32) -> bool {
        let Some(id) = self.hit_mask(ix, iy) else {
            return false;
        };
        self.masks.retain(|m| m.id != id);
        self.selected.remove(&id);
        true
    }

    /// 拖擦: 删掉该点碰到的全部蒙版.
    pub(super) fn erase_all_at(&mut self, ix: f32, iy: f32) -> bool {
        let ids: Vec<String> = self
            .masks
            .iter()
            .filter(|m| m.contains(ix, iy))
            .map(|m| m.id.clone())
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.masks.retain(|m| !ids.iter().any(|id| id == &m.id));
        for id in &ids {
            self.selected.remove(id);
        }
        true
    }

    pub(super) fn apply_selection_click(&mut self, id: Option<String>, control: bool) {
        match id {
            Some(id) if control => {
                if self.selected.contains(&id) {
                    self.selected.remove(&id);
                } else {
                    self.selected.insert(id);
                }
            }
            Some(id) => {
                self.selected.clear();
                self.selected.insert(id);
            }
            None if !control => {
                self.selected.clear();
            }
            None => {}
        }
    }

    /// 在图像边界内整体平移选中蒙版; 返回实际位移.
    pub(super) fn translate_selected(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        if dx == 0 && dy == 0 {
            return (0, 0);
        }
        let iw = self.img_w as i32;
        let ih = self.img_h as i32;
        if iw < 1 || ih < 1 {
            return (0, 0);
        }
        let mut min_dx = i32::MIN / 4;
        let mut max_dx = i32::MAX / 4;
        let mut min_dy = i32::MIN / 4;
        let mut max_dy = i32::MAX / 4;
        let mut any = false;
        for m in &self.masks {
            if !self.selected.contains(&m.id) {
                continue;
            }
            any = true;
            let r = m.normalized();
            min_dx = min_dx.max(-r.x0);
            max_dx = max_dx.min((iw - 1) - r.x1);
            min_dy = min_dy.max(-r.y0);
            max_dy = max_dy.min((ih - 1) - r.y1);
        }
        if !any {
            return (0, 0);
        }
        let dx = dx.clamp(min_dx, max_dx);
        let dy = dy.clamp(min_dy, max_dy);
        if dx == 0 && dy == 0 {
            return (0, 0);
        }
        for m in &mut self.masks {
            if self.selected.contains(&m.id) {
                m.translate(dx, dy);
            }
        }
        (dx, dy)
    }

    fn snapshot(&self) -> UndoSnapshot {
        UndoSnapshot {
            masks: self.masks.clone(),
            block_layout: self.block_layout.clone(),
            voff_target: self.voff_target,
            guides: self.guides.clone(),
            host_guide_token: None,
        }
    }

    /// 蒙版与「组合分块」调整共用同一条撤重时间线, 调哪个改哪个都先
    /// `push_undo` 存一份两者的快照.
    pub(super) fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// 宿主全局辅助线操作: 把当前页状态压入撤重, 并带上宿主令牌, 使
    /// Ctrl+Z/Y 能连同全部组合一起回滚.
    pub fn push_undo_with_host_token(&mut self, token: u64) {
        let mut snap = self.snapshot();
        snap.host_guide_token = Some(token);
        self.undo_stack.push(snap);
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// 从其它页撤回全局操作时, 丢掉本会话里对应令牌及其后的快照, 避免
    /// 再撤一次把已经回滚的全局状态冲乱.
    pub fn purge_host_token(&mut self, token: u64) {
        fn strip(stack: &mut Vec<UndoSnapshot>, token: u64) {
            if let Some(i) = stack
                .iter()
                .position(|s| s.host_guide_token == Some(token))
            {
                stack.truncate(i);
            }
        }
        strip(&mut self.undo_stack, token);
        strip(&mut self.redo_stack, token);
        for h in self.histories.values_mut() {
            strip(&mut h.undo, token);
            strip(&mut h.redo, token);
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            self.guide_host_cmd = Some(GuideHostCmd::UndoGlobalFallback);
            self.status = "没有可撤回的操作.".into();
            cx.notify();
            return;
        };
        let token = prev.host_guide_token;
        let mut now = self.snapshot();
        now.host_guide_token = token;
        self.redo_stack.push(now);
        let layout_changed =
            prev.block_layout != self.block_layout || prev.voff_target != self.voff_target;
        self.masks = prev.masks;
        self.block_layout = prev.block_layout;
        self.voff_target = prev.voff_target;
        self.guides = prev.guides;
        self.guide_selected.clear();
        self.selected.clear();
        if layout_changed {
            self.restore_preview_geom_from_layout();
            self.brush_cursor = None;
            self.poly_cursor = None;
        }
        self.status = format!("已撤回. 蒙版 {} 个", self.masks.len()).into();
        if let Some(t) = token {
            self.guide_host_cmd = Some(GuideHostCmd::UndoGlobal(t));
        }
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            self.guide_host_cmd = Some(GuideHostCmd::RedoGlobalFallback);
            self.status = "没有可重做的操作.".into();
            cx.notify();
            return;
        };
        let token = next.host_guide_token;
        let mut now = self.snapshot();
        now.host_guide_token = token;
        self.undo_stack.push(now);
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        let layout_changed =
            next.block_layout != self.block_layout || next.voff_target != self.voff_target;
        self.masks = next.masks;
        self.block_layout = next.block_layout;
        self.voff_target = next.voff_target;
        self.guides = next.guides;
        self.guide_selected.clear();
        self.selected.clear();
        if layout_changed {
            self.restore_preview_geom_from_layout();
            self.brush_cursor = None;
            self.poly_cursor = None;
        }
        self.status = format!("已重做. 蒙版 {} 个", self.masks.len()).into();
        if let Some(t) = token {
            self.guide_host_cmd = Some(GuideHostCmd::RedoGlobal(t));
        }
        cx.notify();
    }

    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        if !self.masks.iter().any(|m| self.selected.contains(&m.id)) {
            return;
        }
        self.push_undo();
        self.masks.retain(|m| !self.selected.contains(&m.id));
        self.selected.clear();
        self.status = "已删除选中蒙版.".into();
        cx.notify();
    }

    pub fn select_all_masks(&mut self, cx: &mut Context<Self>) {
        if self.masks.is_empty() {
            self.status = "没有蒙版可全选.".into();
            cx.notify();
            return;
        }
        self.selected = self.masks.iter().map(|m| m.id.clone()).collect();
        self.status = format!("已全选 {} 个蒙版 (Delete 删除)", self.selected.len()).into();
        cx.notify();
    }

    pub fn clear_masks(&mut self, cx: &mut Context<Self>) {
        if self.masks.is_empty() {
            return;
        }
        self.push_undo();
        self.masks.clear();
        self.selected.clear();
        self.status = "已清空蒙版.".into();
        cx.notify();
    }
}
