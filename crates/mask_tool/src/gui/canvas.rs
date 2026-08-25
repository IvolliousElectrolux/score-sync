//! 画布交互, 坐标, 窗口外鼠标转发.

use super::*;

impl MaskToolApp {
    pub(super) fn xform(&self) -> ViewXform {
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        // 拖动分块时用锁定的尺寸算缩放比例, 避免拼合图总高实时变化导致
        // 画面跟着缩放抖动, 也保证鼠标坐标换算全程用同一套比例 (见字段
        // 注释); 图像本身仍按当前实际尺寸绘制 (`img_bounds` 用的是活的
        // `self.img_w/h`), 只是缩放比例暂时不跟着重算.
        let (fit_w, fit_h) = self
            .block_drag_freeze
            .unwrap_or((self.img_w as f32, self.img_h as f32));
        ViewXform::compute(fit_w, fit_h, vw, vh, self.zoom, self.pan, self.user_zoomed)
    }

    pub(super) fn screen_in_view(&self, pos: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(pos.x) - f32::from(self.view_bounds.origin.x),
            f32::from(pos.y) - f32::from(self.view_bounds.origin.y),
        )
    }

    /// 限制平移: 画布边界就是目标分辨率 (拼合图, 含底色合成时为底色画布)
    /// 边界, 填不满视口的方向强制居中, 填得满的方向最多平移到边缘贴视口
    /// 边缘, 不允许露出画布之外的空白.
    pub(super) fn clamp_pan(&mut self) {
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        if vw < 1.0 || vh < 1.0 || self.img_w == 0 || self.img_h == 0 {
            return;
        }
        let fit = (vw / self.img_w as f32)
            .min(vh / self.img_h as f32)
            .max(0.0001);
        let scale = if self.user_zoomed {
            (fit * self.zoom).max(0.0001)
        } else {
            fit
        };
        let drawn_w = self.img_w as f32 * scale;
        let drawn_h = self.img_h as f32 * scale;
        let centered_x = (vw - drawn_w) * 0.5;
        let centered_y = (vh - drawn_h) * 0.5;
        let (pan_x_min, pan_x_max) = if drawn_w <= vw {
            (0.0, 0.0)
        } else {
            (vw - drawn_w - centered_x, -centered_x)
        };
        let (pan_y_min, pan_y_max) = if drawn_h <= vh {
            (0.0, 0.0)
        } else {
            (vh - drawn_h - centered_y, -centered_y)
        };
        self.pan.x = self.pan.x.clamp(pan_x_min, pan_x_max);
        self.pan.y = self.pan.y.clamp(pan_y_min, pan_y_max);
    }
    pub(super) fn on_view_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.render_image.is_none() {
            return;
        }
        if ev.click_count >= 2 && ev.button == MouseButton::Left {
            self.fit_to_view(cx);
            return;
        }
        if ev.button != MouseButton::Left {
            if ev.button == MouseButton::Right && self.eyedropper_armed {
                self.cancel_eyedropper(cx);
            }
            return;
        }
        let (sx, sy) = self.screen_in_view(ev.position);
        let xform = self.xform();
        let (ix, iy) = xform.screen_to_image(sx, sy);
        if self.eyedropper_armed {
            self.confirm_eyedropper_at(ix, iy, cx);
            return;
        }
        let control = apply_bg::is_primary_mod(&ev.modifiers);
        let shift = ev.modifiers.shift;

        match self.mode {
            ToolMode::Draw => {
                self.drag = Some(DragKind::Draw {
                    x0: ix,
                    y0: iy,
                    x1: ix,
                    y1: iy,
                });
            }
            ToolMode::Poly => {
                let (ix, iy, snap) = self.poly_maybe_snap(ix, iy);
                if snap {
                    self.finalize_poly(cx);
                    return;
                }
                match &mut self.poly_draft {
                    Some(draft) => {
                        if let Some(&(lx, ly)) = draft.last() {
                            if (lx - ix).abs() < 0.5 && (ly - iy).abs() < 0.5 {
                                // 忽略重复点
                            } else {
                                draft.push((ix, iy));
                            }
                        } else {
                            draft.push((ix, iy));
                        }
                        self.poly_cursor = Some((ix, iy));
                        self.status = format!(
                            "折线 {} 点 · 靠近首点吸附闭环 (右键/Esc 取消)",
                            draft.len()
                        )
                        .into();
                    }
                    None => {
                        self.poly_draft = Some(vec![(ix, iy)]);
                        self.poly_cursor = Some((ix, iy));
                        self.status = "折线起点已定 · 继续点击加点".into();
                    }
                }
            }
            ToolMode::Brush => {
                if self.img_w < 2 || self.img_h < 2 {
                    return;
                }
                let id = new_id();
                let r = self.brush_radius_px();
                let px = ix.round() as i32;
                let py = iy.round() as i32;
                self.push_undo();
                self.masks.push(MaskRect {
                    id: id.clone(),
                    x0: px - r,
                    y0: py - r,
                    x1: px + r,
                    y1: py + r,
                    brush_points: vec![(px, py)],
                    brush_radius: r,
                    color: self.brush_color,
                    poly_points: Vec::new(),
                    opacity: self.brush_opacity,
                    bound_block: None,
                });
                self.selected.clear();
                self.selected.insert(id.clone());
                self.drag = Some(DragKind::Brush { id, start_iy: iy, undid: true });
                self.status = format!("蒙版 {} 个", self.masks.len()).into();
            }
            ToolMode::Eraser => {
                self.drag = Some(DragKind::Erase {
                    start_ix: ix,
                    start_iy: iy,
                    undid: false,
                    wiping: false,
                });
            }
            ToolMode::Pan => {
                let hit = self.hit_mask(ix, iy);
                if let Some(ref id) = hit {
                    if self.selected.contains(id) && !control {
                        self.drag = Some(DragKind::MoveMasks {
                            last_ix: ix,
                            last_iy: iy,
                            undid: false,
                        });
                    } else {
                        self.apply_selection_click(hit, control);
                    }
                } else {
                    if !control {
                        self.selected.clear();
                    }
                    self.drag = Some(DragKind::PagePan {
                        last: ev.position,
                    });
                }
            }
            ToolMode::Select => {
                if shift {
                    self.drag = Some(DragKind::Marquee {
                        x0: ix,
                        y0: iy,
                        x1: ix,
                        y1: iy,
                        additive: control,
                    });
                } else {
                    let hit = self.hit_mask(ix, iy);
                    let tol = xform.edge_tol();
                    let guide_hit = if hit.is_none() && self.selected.is_empty() {
                        self.guide_hit_test(iy, tol)
                    } else {
                        None
                    };
                    // 未选中任何蒙版时: 先命中辅助线 (优先级最高, 细线容易被块
                    // 边界线盖住), 否则点在分块上就是拖动/拉伸分块; 一旦选中了
                    // 某个蒙版, 后续操作都按蒙版处理 (与蒙版列表/画布保持一致).
                    if let Some(idx) = guide_hit {
                        if control {
                            // Ctrl+点选: 只切换多选, 不触发拖动 (拖动只支持单条).
                            if self.guide_selected.contains(&idx) {
                                self.guide_selected.remove(&idx);
                            } else {
                                self.guide_selected.insert(idx);
                            }
                        } else {
                            self.begin_guide_drag(idx);
                        }
                    } else if hit.is_none() && self.selected.is_empty() && self.has_block_pieces() {
                        self.guide_selected.clear();
                        match self.hit_block_at(iy, tol) {
                            Some((rid, BlockHitZone::Top)) => self.begin_block_resize_top(rid, iy),
                            Some((rid, BlockHitZone::Bottom)) => {
                                self.begin_block_resize_bottom(rid, iy)
                            }
                            Some((rid, BlockHitZone::Body)) => self.begin_block_move(rid, iy),
                            None => {}
                        }
                    } else {
                        self.guide_selected.clear();
                        self.block_selected = None;
                        self.apply_selection_click(hit, control);
                    }
                }
            }
        }
        cx.notify();
    }

    pub fn has_active_drag(&self) -> bool {
        self.drag.is_some()
    }

    /// 滑条/色盘拖拽: 鼠标可离开控件但仍在窗口内, 需宿主根节点继续转发.
    pub fn needs_root_move_forward(&self) -> bool {
        matches!(
            self.drag,
            Some(
                DragKind::BrushSize
                    | DragKind::PaletteOpacity
                    | DragKind::PaletteSb
                    | DragKind::PaletteHue
            )
        )
    }

    /// 宿主在窗口外转发鼠标移动 (元素级 on_mouse_move 在窗口外不触发).
    pub fn root_mouse_move(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.apply_mouse_move_at(point(px(x), px(y)), cx);
    }

    /// 宿主在窗口外转发鼠标松开.
    pub fn root_mouse_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.apply_mouse_up_at(point(px(x), px(y)), cx);
    }

    pub(super) fn on_view_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_mouse_move_at(ev.position, cx);
    }

    pub(super) fn apply_mouse_move_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (sx, sy) = self.screen_in_view(position);
        if self.eyedropper_armed {
            let xform = self.xform();
            let (ix, iy) = xform.screen_to_image(sx, sy);
            if let Some(rgb) = self.sample_image_rgb(ix, iy) {
                self.preview_eyedropper_rgb(rgb, cx);
            }
            return;
        }
        match self.drag.take() {
            Some(DragKind::Draw { x0, y0, .. }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                self.drag = Some(DragKind::Draw {
                    x0,
                    y0,
                    x1: ix,
                    y1: iy,
                });
                cx.notify();
            }
            Some(DragKind::Brush { id, start_iy, undid }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                self.brush_cursor = Some((ix, iy));
                if self.append_brush_point(&id, ix, iy) {
                    cx.notify();
                } else {
                    cx.notify();
                }
                self.drag = Some(DragKind::Brush { id, start_iy, undid });
            }
            Some(DragKind::PagePan { last }) => {
                let dx = f32::from(position.x) - f32::from(last.x);
                let dy = f32::from(position.y) - f32::from(last.y);
                self.pan.x += dx;
                self.pan.y += dy;
                self.user_zoomed = true;
                self.clamp_pan();
                self.drag = Some(DragKind::PagePan { last: position });
                cx.notify();
            }
            Some(DragKind::MoveMasks {
                last_ix,
                last_iy,
                undid,
            }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                let raw_dx = ix - last_ix;
                let raw_dy = iy - last_iy;
                let step_x = raw_dx.round() as i32;
                let step_y = raw_dy.round() as i32;
                let mut undid = undid;
                if (step_x != 0 || step_y != 0) && !undid {
                    self.push_undo();
                    undid = true;
                }
                let (applied_x, applied_y) = self.translate_selected(step_x, step_y);
                self.drag = Some(DragKind::MoveMasks {
                    last_ix: last_ix + applied_x as f32,
                    last_iy: last_iy + applied_y as f32,
                    undid,
                });
                cx.notify();
            }
            Some(DragKind::Marquee {
                x0,
                y0,
                additive,
                ..
            }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                self.drag = Some(DragKind::Marquee {
                    x0,
                    y0,
                    x1: ix,
                    y1: iy,
                    additive,
                });
                cx.notify();
            }
            Some(DragKind::PaletteOpacity) => {
                self.drag = Some(DragKind::PaletteOpacity);
                self.set_palette_opacity_from_x(f32::from(position.x), cx);
            }
            Some(DragKind::PaletteSb) => {
                self.drag = Some(DragKind::PaletteSb);
                self.set_palette_sb_from_pos(
                    f32::from(position.x),
                    f32::from(position.y),
                    cx,
                );
            }
            Some(DragKind::PaletteHue) => {
                self.drag = Some(DragKind::PaletteHue);
                self.set_palette_hue_from_y(f32::from(position.y), cx);
            }
            Some(DragKind::BrushSize) => {
                self.drag = Some(DragKind::BrushSize);
                self.set_brush_size_from_x(f32::from(position.x), cx);
            }
            Some(DragKind::Erase {
                start_ix,
                start_iy,
                undid,
                wiping,
            }) => {
                let xform = self.xform();
                let (ix, iy) = xform.screen_to_image(sx, sy);
                let mut undid = undid;
                let mut wiping = wiping;
                if !wiping {
                    let dx = ix - start_ix;
                    let dy = iy - start_iy;
                    if dx * dx + dy * dy >= ERASE_DRAG_SLOP_IMG * ERASE_DRAG_SLOP_IMG {
                        wiping = true;
                        if !undid {
                            self.push_undo();
                            undid = true;
                        }
                        self.erase_all_at(start_ix, start_iy);
                        self.erase_all_at(ix, iy);
                        self.status = format!("拖擦中 · 蒙版 {} 个", self.masks.len()).into();
                        cx.notify();
                    }
                } else if self.erase_all_at(ix, iy) {
                    self.status = format!("拖擦中 · 蒙版 {} 个", self.masks.len()).into();
                    cx.notify();
                }
                self.drag = Some(DragKind::Erase {
                    start_ix,
                    start_iy,
                    undid,
                    wiping,
                });
            }
            Some(DragKind::BlockMove {
                region_id,
                start_iy,
                start_layout,
                start_voff,
                undid,
            }) => {
                let xform = self.xform();
                let (_, iy) = xform.screen_to_image(sx, sy);
                let (undid, changed) =
                    self.apply_block_move(&region_id, start_iy, &start_layout, start_voff, undid, iy);
                self.drag = Some(DragKind::BlockMove {
                    region_id,
                    start_iy,
                    start_layout,
                    start_voff,
                    undid,
                });
                if changed {
                    cx.notify();
                }
            }
            Some(DragKind::BlockResizeTop {
                region_id,
                start_iy,
                start_layout,
                start_voff,
                max_trim,
                undid,
            }) => {
                let xform = self.xform();
                let (_, iy) = xform.screen_to_image(sx, sy);
                let (undid, changed) = self.apply_block_resize_top(
                    &region_id,
                    start_iy,
                    &start_layout,
                    start_voff,
                    max_trim,
                    undid,
                    iy,
                );
                self.drag = Some(DragKind::BlockResizeTop {
                    region_id,
                    start_iy,
                    start_layout,
                    start_voff,
                    max_trim,
                    undid,
                });
                if changed {
                    cx.notify();
                }
            }
            Some(DragKind::BlockResizeBottom {
                region_id,
                start_iy,
                start_layout,
                start_voff,
                max_trim,
                undid,
            }) => {
                let xform = self.xform();
                let (_, iy) = xform.screen_to_image(sx, sy);
                let (undid, changed) = self.apply_block_resize_bottom(
                    &region_id,
                    start_iy,
                    &start_layout,
                    start_voff,
                    max_trim,
                    undid,
                    iy,
                );
                self.drag = Some(DragKind::BlockResizeBottom {
                    region_id,
                    start_iy,
                    start_layout,
                    start_voff,
                    max_trim,
                    undid,
                });
                if changed {
                    cx.notify();
                }
            }
            Some(DragKind::GuideMove {
                idx,
                start_y,
                orig_lines,
                undid,
            }) => {
                let xform = self.xform();
                let (_, iy) = xform.screen_to_image(sx, sy);
                let undid = self.apply_guide_move(idx, start_y, &orig_lines, undid, iy);
                self.drag = Some(DragKind::GuideMove {
                    idx,
                    start_y,
                    orig_lines,
                    undid,
                });
                cx.notify();
            }
            None => {
                if self.mode == ToolMode::Poly && self.poly_draft.is_some() {
                    let xform = self.xform();
                    let (ix, iy) = xform.screen_to_image(sx, sy);
                    let (ix, iy, _) = self.poly_maybe_snap(ix, iy);
                    self.poly_cursor = Some((ix, iy));
                    cx.notify();
                } else if self.mode == ToolMode::Brush {
                    let xform = self.xform();
                    let (ix, iy) = xform.screen_to_image(sx, sy);
                    self.brush_cursor = Some((ix, iy));
                    cx.notify();
                } else if self.mode == ToolMode::Select && self.selected.is_empty() && self.has_block_pieces() {
                    let xform = self.xform();
                    let (_, iy) = xform.screen_to_image(sx, sy);
                    let tol = xform.edge_tol();
                    let guide_hover = self.guide_hit_test(iy, tol);
                    if guide_hover != self.guide_hover {
                        self.guide_hover = guide_hover;
                        cx.notify();
                    }
                    let hover = guide_hover.is_none() && self.hit_block_at(iy, tol).is_some();
                    if hover != self.block_hover {
                        self.block_hover = hover;
                        cx.notify();
                    }
                } else if self.block_hover || self.guide_hover.is_some() {
                    self.block_hover = false;
                    self.guide_hover = None;
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn on_view_mouse_up(
        &mut self,
        ev: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.button != MouseButton::Left {
            return;
        }
        self.apply_mouse_up_at(ev.position, cx);
    }

    pub(super) fn apply_mouse_up_at(&mut self, _position: Point<Pixels>, cx: &mut Context<Self>) {
        self.block_drag_freeze = None;
        let finished = self.drag.take();
        match finished {
            Some(DragKind::Draw { x0, y0, x1, y1 }) => {
                if self.img_w < 2 || self.img_h < 2 {
                    cx.notify();
                    return;
                }
                let min_x = x0.min(x1);
                let max_x = x0.max(x1);
                let min_y = y0.min(y1);
                let max_y = y0.max(y1);
                let w = max_x - min_x;
                let h = max_y - min_y;
                if w >= 3.0 && h >= 3.0 {
                    let iw = self.img_w as i32;
                    let ih = self.img_h as i32;
                    let clamp = |v: f32, hi: i32| -> i32 {
                        v.round().clamp(0.0, (hi - 1).max(0) as f32) as i32
                    };
                    let mid = new_id();
                    let bound_block = self.resolve_bound_block(y0, y1);
                    self.push_undo();
                    self.masks.push(MaskRect {
                        id: mid.clone(),
                        x0: clamp(min_x, iw),
                        y0: clamp(min_y, ih),
                        x1: clamp(max_x, iw),
                        y1: clamp(max_y, ih),
                        brush_points: Vec::new(),
                        brush_radius: 0,
                        color: self.mask_color,
                        poly_points: Vec::new(),
                        opacity: self.mask_opacity,
                        bound_block,
                    });
                    self.selected.clear();
                    self.selected.insert(mid);
                    self.status = format!("蒙版 {} 个", self.masks.len()).into();
                }
                cx.notify();
            }
            Some(DragKind::Brush { id, start_iy, .. }) => {
                if let Some(m) = self.masks.iter_mut().find(|m| m.id == id) {
                    m.refresh_brush_bounds();
                }
                let end_iy = self
                    .masks
                    .iter()
                    .find(|m| m.id == id)
                    .and_then(|m| m.brush_points.last())
                    .map(|&(_, y)| y as f32)
                    .unwrap_or(start_iy);
                let bound_block = self.resolve_bound_block(start_iy, end_iy);
                if let Some(m) = self.masks.iter_mut().find(|m| m.id == id) {
                    m.bound_block = bound_block;
                }
                self.status = format!("蒙版 {} 个", self.masks.len()).into();
                cx.notify();
            }
            Some(DragKind::Marquee {
                x0,
                y0,
                x1,
                y1,
                additive,
            }) => {
                let min_x = x0.min(x1);
                let max_x = x0.max(x1);
                let min_y = y0.min(y1);
                let max_y = y0.max(y1);
                if (max_x - min_x) >= 2.0 && (max_y - min_y) >= 2.0 {
                    if !additive {
                        self.selected.clear();
                    }
                    for m in &self.masks {
                        if m.intersects_rect(min_x, min_y, max_x, max_y) {
                            self.selected.insert(m.id.clone());
                        }
                    }
                    self.status = format!("已选中 {} 个", self.selected.len()).into();
                }
                cx.notify();
            }
            Some(DragKind::MoveMasks { .. })
            | Some(DragKind::PagePan { .. })
            | Some(DragKind::BrushSize)
            | Some(DragKind::PaletteOpacity)
            | None => {
                self.opacity_undid = false;
                cx.notify();
            }
            Some(DragKind::BlockMove { region_id, .. }) => {
                self.status = format!("已移动分块 {region_id}").into();
                cx.notify();
            }
            Some(DragKind::BlockResizeTop { region_id, .. })
            | Some(DragKind::BlockResizeBottom { region_id, .. }) => {
                self.status = format!("已调整分块 {region_id} 边界").into();
                cx.notify();
            }
            Some(DragKind::GuideMove { .. }) => {
                self.status = "已调整辅助线.".into();
                if self.guides_sync {
                    self.guide_host_cmd = Some(GuideHostCmd::SyncPositions);
                }
                cx.notify();
            }
            Some(DragKind::PaletteSb) | Some(DragKind::PaletteHue) => {
                let c = self.picker_rgb();
                let mut prefs = self.color_prefs();
                prefs.push_recent(c);
                self.recent_colors = prefs.recent_colors;
                self.mark_prefs_dirty();
                self.opacity_undid = false;
                cx.notify();
            }
            Some(DragKind::Erase {
                start_ix,
                start_iy,
                undid,
                wiping,
            }) => {
                if !wiping {
                    if !undid {
                        self.push_undo();
                    }
                    if self.erase_topmost_at(start_ix, start_iy) {
                        self.status = format!("已擦除顶层 · 蒙版 {} 个", self.masks.len()).into();
                    } else {
                        self.undo_stack.pop();
                        self.status = "未点到可擦除蒙版.".into();
                    }
                } else {
                    self.status = format!("蒙版 {} 个", self.masks.len()).into();
                }
                cx.notify();
            }
        }
    }

    pub(super) fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.render_image.is_none() {
            return;
        }
        // 无模式禁止移动页面; Ctrl+滚轮缩放始终可用
        if !apply_bg::is_primary_mod(&ev.modifiers) {
            if self.mode == ToolMode::Select {
                return;
            }
            let delta = match ev.delta {
                ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
                ScrollDelta::Lines(p) => (p.x * 40.0, p.y * 40.0),
            };
            self.pan.x += delta.0;
            self.pan.y += delta.1;
            self.user_zoomed = true;
            self.clamp_pan();
            cx.notify();
            return;
        }
        let factor = match ev.delta {
            ScrollDelta::Pixels(p) => {
                if f32::from(p.y) > 0.0 || f32::from(p.x) > 0.0 {
                    1.15
                } else {
                    1.0 / 1.15
                }
            }
            ScrollDelta::Lines(p) => {
                if p.y > 0.0 || p.x > 0.0 {
                    1.15
                } else {
                    1.0 / 1.15
                }
            }
        };
        let (sx, sy) = self.screen_in_view(ev.position);
        let old = self.xform();
        let (ix, iy) = old.screen_to_image(sx, sy);

        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        let fit = if self.img_w > 0 && self.img_h > 0 && vw > 1.0 && vh > 1.0 {
            (vw / self.img_w as f32).min(vh / self.img_h as f32).max(0.0001)
        } else {
            1.0
        };
        let current_zoom = if self.user_zoomed {
            self.zoom
        } else {
            1.0
        };
        self.user_zoomed = true;
        self.zoom = (current_zoom * factor).clamp(0.05, 40.0);
        let new_scale = fit * self.zoom;
        // 保持鼠标下图像点不变
        self.pan.x = sx - (vw - self.img_w as f32 * new_scale) * 0.5 - ix * new_scale;
        self.pan.y = sy - (vh - self.img_h as f32 * new_scale) * 0.5 - iy * new_scale;
        self.clamp_pan();
        cx.notify();
    }
    pub fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let render = self.render_image.clone();
        let masks = self.masks.clone();
        let selected = self.selected.clone();
        let img_w = self.img_w;
        let img_h = self.img_h;
        let zoom = self.zoom;
        let pan = self.pan;
        let user_zoomed = self.user_zoomed;
        let rubber = match &self.drag {
            Some(DragKind::Draw { x0, y0, x1, y1 }) => Some((*x0, *y0, *x1, *y1, false)),
            Some(DragKind::Marquee { x0, y0, x1, y1, .. }) => Some((*x0, *y0, *x1, *y1, true)),
            _ => None,
        };
        let poly_draft = self.poly_draft.clone();
        let poly_cursor = self.poly_cursor;
        let brush_cursor = self.brush_cursor;
        let brush_size = self.brush_size;
        let brush_color = self.brush_color;
        let brush_opacity = self.brush_opacity;
        let mask_color = self.mask_color;
        let show_blocks =
            self.mode == ToolMode::Select && self.selected.is_empty() && self.has_block_pieces();
        let block_spans = if show_blocks { self.block_spans() } else { Vec::new() };
        let block_selected = self.block_selected.clone();
        let guide_lines = self.guides.lines.clone();
        let guide_selected = self.guide_selected.clone();
        let guide_hover = self.guide_hover;
        let freeze = self.block_drag_freeze;
        let tile_preview = self.is_block_dragging() && !self.block_tiles.is_empty();
        let block_tiles = if tile_preview {
            self.block_tiles.clone()
        } else {
            Vec::new()
        };
        let block_layout_paint = if tile_preview {
            self.block_layout.clone()
        } else {
            Vec::new()
        };
        let block_bg = if tile_preview {
            self.block_bg.clone()
        } else {
            None
        };
        let block_hoff = self.block_hoff;
        let block_voff_paint = self.block_voff;
        let block_bg_left = self.block_bg_left;
        let block_bg_top = self.block_bg_top;
        let block_shows_bg = self.block_shows_bg;
        let content_scale = self.content_scale;
        let cursor = if self.eyedropper_armed {
            CursorStyle::Crosshair
        } else {
            match self.mode {
                ToolMode::Brush => CursorStyle::None,
                ToolMode::Draw | ToolMode::Poly | ToolMode::Eraser => CursorStyle::Crosshair,
                ToolMode::Pan => CursorStyle::OpenHand,
                ToolMode::Select => {
                    let dragging_block = matches!(
                        self.drag,
                        Some(DragKind::BlockMove { .. })
                            | Some(DragKind::BlockResizeTop { .. })
                            | Some(DragKind::BlockResizeBottom { .. })
                            | Some(DragKind::GuideMove { .. })
                    );
                    if dragging_block
                        || (self.guide_hover.is_some() && self.selected.is_empty())
                        || (self.block_hover && self.selected.is_empty())
                    {
                        CursorStyle::ResizeUpDown
                    } else {
                        CursorStyle::Arrow
                    }
                }
            }
        };

        div()
            .id("mask_image_view")
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(0x2b2b2b))
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_view_mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    if this.eyedropper_armed {
                        this.cancel_eyedropper(cx);
                    } else if this.mode == ToolMode::Poly {
                        this.cancel_poly_draft(cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(p) = first_image_in_paths(paths.paths()) {
                    this.load_image(p, cx);
                }
            }))
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, _| {
                                this.view_bounds = bounds;
                            });
                        }
                    },
                    move |bounds, _, window, _cx| {
                        let vw = f32::from(bounds.size.width);
                        let vh = f32::from(bounds.size.height);
                        // 拖动分块时用锁定尺寸算缩放, 与鼠标坐标换算同一套,
                        // 避免拼合图总高变化把整页重新 fit 造成画面缩放抖动.
                        let (fit_w, fit_h) = freeze.unwrap_or((img_w as f32, img_h as f32));
                        let xform = ViewXform::compute(
                            fit_w,
                            fit_h,
                            vw,
                            vh,
                            zoom,
                            pan,
                            user_zoomed,
                        );
                        let img_bounds = Bounds {
                            origin: point(
                                bounds.origin.x + px(xform.origin_x),
                                bounds.origin.y + px(xform.origin_y),
                            ),
                            size: size(
                                px(img_w as f32 * xform.scale),
                                px(img_h as f32 * xform.scale),
                            ),
                        };

                        if tile_preview {
                            paint_live_block_tiles(
                                window,
                                img_bounds,
                                xform,
                                &block_tiles,
                                &block_layout_paint,
                                block_hoff,
                                block_voff_paint,
                                block_bg.as_ref(),
                                block_bg_left,
                                block_bg_top,
                                block_shows_bg,
                                content_scale,
                            );
                        } else if let Some(ref img) = render {
                            let _ = window.paint_image(
                                img_bounds,
                                Corners::default(),
                                img.clone(),
                                0,
                                false,
                            );
                        }

                        if show_blocks {
                            for (rid, y0, y1) in &block_spans {
                                let is_sel = block_selected.as_deref() == Some(rid.as_str());
                                // 选中的块不再叠色块 (会遮住原图颜色, 不方便对色),
                                // 改成更粗的实线边框; 未选中的块只画细虚线示意.
                                let line_color = if is_sel {
                                    rgb(0xf97316)
                                } else {
                                    rgb(0x38bdf8)
                                };
                                for &y in &[*y0, *y1] {
                                    let sy = bounds.origin.y + px(xform.origin_y + y as f32 * xform.scale);
                                    let mut line = PathBuilder::stroke(if is_sel { px(2.5) } else { px(1.) });
                                    if !is_sel {
                                        line = line.dash_array(&[px(6.), px(4.)]);
                                    }
                                    line.move_to(point(bounds.origin.x, sy));
                                    line.line_to(point(
                                        bounds.origin.x + px(img_w as f32 * xform.scale),
                                        sy,
                                    ));
                                    if let Ok(path) = line.build() {
                                        window.paint_path(path, line_color);
                                    }
                                }
                            }
                        }

                        for (i, &y) in guide_lines.iter().enumerate() {
                            let is_sel = guide_selected.contains(&i);
                            let is_hover = guide_hover == Some(i);
                            let color = if is_sel {
                                rgb(0xf59e0b)
                            } else if is_hover {
                                rgb(0xfbbf24)
                            } else {
                                rgb(0xa855f7)
                            };
                            let sy = bounds.origin.y + px(xform.origin_y + y as f32 * xform.scale);
                            let mut line = PathBuilder::stroke(if is_sel { px(2.) } else { px(1.2) });
                            line = line.dash_array(&[px(9.), px(5.)]);
                            line.move_to(point(bounds.origin.x, sy));
                            line.line_to(point(
                                bounds.origin.x + px(img_w as f32 * xform.scale),
                                sy,
                            ));
                            if let Ok(path) = line.build() {
                                window.paint_path(path, color);
                            }
                            // 左侧小三角把手, 便于识别可拖动的辅助线.
                            let handle = Bounds {
                                origin: point(bounds.origin.x, sy - px(5.)),
                                size: size(px(10.), px(10.)),
                            };
                            window.paint_quad(quad(
                                handle,
                                px(2.),
                                color,
                                px(0.),
                                color,
                                Default::default(),
                            ));
                        }

                        for m in &masks {
                            if m.is_brush() {
                                let r = m.brush_radius.max(1) as f32;
                                let diam = (r * 2.0 * xform.scale).max(2.0);
                                let color_u32 = ((m.color[0] as u32) << 16)
                                    | ((m.color[1] as u32) << 8)
                                    | (m.color[2] as u32);
                                let mut fill_c = rgb(color_u32);
                                fill_c.a = m.effective_opacity();
                                let is_sel = selected.contains(&m.id);
                                let border = if is_sel {
                                    rgb(0xdc5050)
                                } else {
                                    rgb(0xb4b4b4)
                                };
                                // 圆章叠画 (与导出一致); 不用整条 stroke, 避免折返畸形
                                if is_sel {
                                    paint_brush_stamps(
                                        window,
                                        &m.brush_points,
                                        r,
                                        xform.scale,
                                        xform.origin_x,
                                        xform.origin_y,
                                        bounds.origin,
                                        diam + 4.0,
                                        border,
                                    );
                                }
                                paint_brush_stamps(
                                    window,
                                    &m.brush_points,
                                    r,
                                    xform.scale,
                                    xform.origin_x,
                                    xform.origin_y,
                                    bounds.origin,
                                    diam,
                                    fill_c,
                                );
                                continue;
                            }

                            if m.is_poly() {
                                let color_u32 = color_rgb_u32(m.color);
                                let mut fill_c = rgb(color_u32);
                                fill_c.a = m.effective_opacity();
                                let border = if selected.contains(&m.id) {
                                    rgb(0xdc5050)
                                } else {
                                    rgb(0xb4b4b4)
                                };
                                let mut fill_builder = PathBuilder::fill();
                                let mut stroke_builder = PathBuilder::stroke(if selected
                                    .contains(&m.id)
                                {
                                    px(2.)
                                } else {
                                    px(1.)
                                });
                                let mut first = true;
                                for &(px_i, py_i) in &m.poly_points {
                                    let sx = bounds.origin.x
                                        + px(xform.origin_x + px_i as f32 * xform.scale);
                                    let sy = bounds.origin.y
                                        + px(xform.origin_y + py_i as f32 * xform.scale);
                                    let pt = point(sx, sy);
                                    if first {
                                        fill_builder.move_to(pt);
                                        stroke_builder.move_to(pt);
                                        first = false;
                                    } else {
                                        fill_builder.line_to(pt);
                                        stroke_builder.line_to(pt);
                                    }
                                }
                                fill_builder.close();
                                stroke_builder.close();
                                if let Ok(path) = fill_builder.build() {
                                    window.paint_path(path, fill_c);
                                }
                                if let Ok(path) = stroke_builder.build() {
                                    window.paint_path(path, border);
                                }
                                continue;
                            }

                            let r = m.normalized();
                            let mut b = xform.image_rect_to_screen(r.x0, r.y0, r.x1, r.y1);
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let color_u32 = color_rgb_u32(m.color);
                            let mut fill_c = rgb(color_u32);
                            fill_c.a = m.effective_opacity();
                            let border = if selected.contains(&m.id) {
                                rgb(0xdc5050)
                            } else {
                                rgb(0xb4b4b4)
                            };
                            let border_w = if selected.contains(&m.id) {
                                px(2.)
                            } else {
                                px(1.)
                            };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill_c,
                                border_w,
                                border,
                                Default::default(),
                            ));
                        }

                        if let Some((x0, y0, x1, y1, is_marquee)) = rubber {
                            let min_x = x0.min(x1) as i32;
                            let min_y = y0.min(y1) as i32;
                            let max_x = x0.max(x1) as i32;
                            let max_y = y0.max(y1) as i32;
                            let mut b = xform.image_rect_to_screen(min_x, min_y, max_x, max_y);
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill_c = if is_marquee {
                                rgb(0x3b82f6)
                            } else {
                                rgb(color_rgb_u32(mask_color))
                            };
                            fill_c.a = if is_marquee { 0.18 } else { 0.24 };
                            let border = if is_marquee {
                                rgb(0x2563eb)
                            } else {
                                rgb(0x508cdc)
                            };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill_c,
                                px(1.),
                                border,
                                Default::default(),
                            ));
                            let mut builder = PathBuilder::stroke(px(1.));
                            builder = builder.dash_array(&[px(4.), px(3.)]);
                            builder.move_to(b.origin);
                            builder.line_to(point(b.origin.x + b.size.width, b.origin.y));
                            builder.line_to(point(
                                b.origin.x + b.size.width,
                                b.origin.y + b.size.height,
                            ));
                            builder.line_to(point(b.origin.x, b.origin.y + b.size.height));
                            builder.close();
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, border);
                            }
                        }

                        // 折线草稿 + 橡皮筋
                        if let Some(ref draft) = poly_draft {
                            if !draft.is_empty() {
                                let to_screen = |ix: f32, iy: f32| {
                                    point(
                                        bounds.origin.x + px(xform.origin_x + ix * xform.scale),
                                        bounds.origin.y + px(xform.origin_y + iy * xform.scale),
                                    )
                                };
                                let mut stroke = PathBuilder::stroke(px(1.5));
                                stroke = stroke.dash_array(&[px(5.), px(3.)]);
                                let first = to_screen(draft[0].0, draft[0].1);
                                stroke.move_to(first);
                                for &(ix, iy) in draft.iter().skip(1) {
                                    stroke.line_to(to_screen(ix, iy));
                                }
                                if let Some((cx_i, cy_i)) = poly_cursor {
                                    stroke.line_to(to_screen(cx_i, cy_i));
                                }
                                if let Ok(path) = stroke.build() {
                                    window.paint_path(path, rgb(0x38bdf8));
                                }
                                // 顶点小方块; 首点在可吸附时加大
                                let can_snap = draft.len() >= 3
                                    && poly_cursor
                                        .map(|(cx_i, cy_i)| {
                                            (cx_i - draft[0].0).abs() < 0.01
                                                && (cy_i - draft[0].1).abs() < 0.01
                                        })
                                        .unwrap_or(false);
                                for (i, &(ix, iy)) in draft.iter().enumerate() {
                                    let p = to_screen(ix, iy);
                                    let sz = if i == 0 && can_snap { 10.0 } else { 6.0 };
                                    let b = Bounds {
                                        origin: point(p.x - px(sz * 0.5), p.y - px(sz * 0.5)),
                                        size: size(px(sz), px(sz)),
                                    };
                                    let col = if i == 0 {
                                        rgb(0xf97316)
                                    } else {
                                        rgb(0x38bdf8)
                                    };
                                    window.paint_quad(quad(
                                        b,
                                        px(1.),
                                        col,
                                        px(1.),
                                        rgb(0xffffff),
                                        Default::default(),
                                    ));
                                }
                            }
                        }

                        // 画笔圆形光标: 填充跟画笔色, 边框用反色 (白笔黑框)
                        if let Some((bx, by)) = brush_cursor {
                            let screen_r = (brush_size * 0.5 * xform.scale).max(1.5);
                            let cx_s = f32::from(bounds.origin.x) + xform.origin_x + bx * xform.scale;
                            let cy_s = f32::from(bounds.origin.y) + xform.origin_y + by * xform.scale;
                            let [cr, cg, cb] = brush_color;
                            let mut fill = rgb(
                                ((cr as u32) << 16) | ((cg as u32) << 8) | (cb as u32),
                            );
                            fill.a = (brush_opacity * 0.35).clamp(0.12, 0.55);
                            let [rr, rg, rb] = opposite_rgb(brush_color);
                            let mut ring = rgb(
                                ((rr as u32) << 16) | ((rg as u32) << 8) | (rb as u32),
                            );
                            ring.a = 1.0;
                            let ring_w = (screen_r * 0.08).clamp(1.5, 2.5);
                            let b = Bounds {
                                origin: point(px(cx_s - screen_r), px(cy_s - screen_r)),
                                size: size(px(screen_r * 2.0), px(screen_r * 2.0)),
                            };
                            window.paint_quad(quad(
                                b,
                                px(screen_r),
                                fill,
                                px(ring_w),
                                ring,
                                Default::default(),
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }
}

/// 拖动分块时的实时预览: 用加载时上传好的分块/底色 GPU 贴图按当前
/// layout 摆放, 间隙与扩展区用纯色块填充. 不再每帧生成整张预览图、也不
/// 再往 GPUI 图集上传新贴图 (那才是卡顿的主因).
fn paint_live_block_tiles(
    window: &mut Window,
    img_bounds: Bounds<Pixels>,
    xform: ViewXform,
    tiles: &[BlockTile],
    layout: &[BlockAdjust],
    hoff: i64,
    voff: i64,
    bg: Option<&BlockBgTile>,
    bg_left: u32,
    bg_top: u32,
    shows_bg: bool,
    content_scale: f32,
) {
    if tiles.is_empty() {
        return;
    }
    let scale = xform.scale;
    let cs = if content_scale > 0.0001 { content_scale } else { 1.0 };
    let img_rect = |ix: f32, iy: f32, w: f32, h: f32| -> Bounds<Pixels> {
        Bounds {
            origin: point(
                img_bounds.origin.x + px(ix * scale),
                img_bounds.origin.y + px(iy * scale),
            ),
            size: size(px((w * scale).max(1.0)), px((h * scale).max(1.0))),
        }
    };
    let fill_rect = |window: &mut Window, ix: f32, iy: f32, w: f32, h: f32, color: [u8; 3]| {
        if w < 0.5 || h < 0.5 {
            return;
        }
        let c = rgb(color_rgb_u32(color));
        window.paint_quad(quad(
            img_rect(ix, iy, w, h),
            px(0.),
            c,
            px(0.),
            c,
            Default::default(),
        ));
    };
    let canvas_x = |sx: f32| hoff as f32 + sx * cs;
    let canvas_y = |sy: f32| voff as f32 + sy * cs;
    let canvas_s = |s: f32| s * cs;

    window.with_content_mask(Some(ContentMask { bounds: img_bounds }), |window| {
        if shows_bg {
            if let Some(bg) = bg {
                let bg_bounds = Bounds {
                    origin: point(
                        img_bounds.origin.x - px(bg_left as f32 * scale),
                        img_bounds.origin.y - px(bg_top as f32 * scale),
                    ),
                    size: size(px(bg.width as f32 * scale), px(bg.height as f32 * scale)),
                };
                let _ = window.paint_image(
                    bg_bounds,
                    Corners::default(),
                    bg.image.clone(),
                    0,
                    false,
                );
            }
        } else {
            let white = rgb(0xffffff);
            window.paint_quad(quad(
                img_bounds,
                px(0.),
                white,
                px(0.),
                white,
                Default::default(),
            ));
        }

        let sheet_w = tiles.iter().map(|t| t.width).max().unwrap_or(1) as f32;
        let hx = canvas_x(0.0);
        let dw = canvas_s(sheet_w);
        let mut yy: i64 = 0;
        let mut prev_bottom: Option<[u8; 3]> = None;
        for (i, tile) in tiles.iter().enumerate() {
            let adj = BlockAdjust::find(layout, &tile.region_id)
                .cloned()
                .unwrap_or_default();
            let (gap, ext_top, content_h, ext_bottom, _trim_top) =
                crate::layout::effective_metrics(tile.height as i32, &adj);
            if gap > 0 {
                if i > 0 {
                    if let Some(prev) = prev_bottom {
                        let top_half = gap / 2;
                        if top_half > 0 {
                            fill_rect(
                                window,
                                hx,
                                canvas_y(yy as f32),
                                dw,
                                canvas_s(top_half as f32),
                                prev,
                            );
                        }
                        let bottom_half = gap - top_half;
                        if bottom_half > 0 {
                            fill_rect(
                                window,
                                hx,
                                canvas_y((yy + top_half as i64) as f32),
                                dw,
                                canvas_s(bottom_half as f32),
                                tile.top_fill,
                            );
                        }
                    }
                }
                yy += gap as i64;
            }
            if ext_top > 0 {
                fill_rect(
                    window,
                    hx,
                    canvas_y(yy as f32),
                    dw,
                    canvas_s(ext_top as f32),
                    tile.top_fill,
                );
            }
            let content_y = yy + ext_top as i64;
            if content_h > 0 {
                let piece_origin_y = canvas_y((yy + adj.extra_top as i64) as f32);
                let piece_bounds = img_rect(
                    hx,
                    piece_origin_y,
                    canvas_s(tile.width as f32),
                    canvas_s(tile.height as f32),
                );
                let clip = img_rect(
                    hx,
                    canvas_y(content_y as f32),
                    canvas_s(tile.width as f32),
                    canvas_s(content_h as f32),
                );
                window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                    let _ = window.paint_image(
                        piece_bounds,
                        Corners::default(),
                        tile.image.clone(),
                        0,
                        false,
                    );
                });
            }
            yy += ext_top as i64 + content_h as i64;
            if ext_bottom > 0 {
                fill_rect(
                    window,
                    hx,
                    canvas_y(yy as f32),
                    dw,
                    canvas_s(ext_bottom as f32),
                    tile.bottom_fill,
                );
                yy += ext_bottom as i64;
            }
            prev_bottom = Some(tile.bottom_fill);
        }
    });
}
