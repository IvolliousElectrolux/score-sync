//! 浮动取色器 / 滴管 / RGB 手输.

use super::*;

impl MaskToolApp {
    pub(super) fn mark_prefs_dirty(&mut self) {
        self.prefs_dirty = true;
    }

    pub(super) fn current_target_color(&self) -> [u8; 3] {
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_color,
            ColorPickerTarget::Brush => self.brush_color,
        }
    }

    pub(super) fn current_target_opacity(&self) -> f32 {
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_opacity,
            ColorPickerTarget::Brush => self.brush_opacity,
        }
    }

    pub(super) fn sync_picker_hsv_from_target(&mut self) {
        let (h, s, v) = rgb_to_hsv(self.current_target_color());
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
    }

    pub(super) fn point_in_bounds(x: f32, y: f32, b: Bounds<Pixels>) -> bool {
        let bx = f32::from(b.origin.x);
        let by = f32::from(b.origin.y);
        let bw = f32::from(b.size.width);
        let bh = f32::from(b.size.height);
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    }

    pub(super) fn open_color_picker(&mut self, target: ColorPickerTarget, cx: &mut Context<Self>) {
        if self.color_picker_open && self.color_picker_target == target {
            self.close_color_picker(cx);
            return;
        }
        self.color_picker_target = target;
        self.color_picker_open = true;
        self.sync_picker_hsv_from_target();
        self.rebuild_sb_image();
        if self.hue_image.is_none() {
            self.rebuild_hue_image();
        }
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    pub(super) fn close_color_picker(&mut self, cx: &mut Context<Self>) {
        if !self.color_picker_open {
            return;
        }
        let c = self.current_target_color();
        let mut prefs = self.color_prefs();
        prefs.push_recent(c);
        self.recent_colors = prefs.recent_colors;
        self.mark_prefs_dirty();
        self.eyedropper_armed = false;
        self.eyedropper_backup = None;
        self.color_picker_open = false;
        self.opacity_undid = false;
        if matches!(
            self.drag,
            Some(DragKind::PaletteOpacity | DragKind::PaletteSb | DragKind::PaletteHue)
        ) {
            self.drag = None;
        }
        cx.notify();
    }

    /// 选色悬浮窗相对悬浮层的布局: (left, top, place_below, caret_x_in_popover).
    /// `top` 为含箭头在内的外框顶部. 锚点换算一律相对 `picker_layer_bounds`
    /// (与绝对定位 `left`/`top` 同一坐标系), 避免侧栏 padding 造成的水平偏移.
    pub(super) fn color_picker_placement(&self) -> Option<(f32, f32, bool, f32)> {
        let layer = if f32::from(self.picker_layer_bounds.size.width) >= 8.0 {
            self.picker_layer_bounds
        } else {
            self.side_bounds
        };
        let pw = f32::from(layer.size.width);
        let ph = f32::from(layer.size.height);
        if pw < 8.0 || ph < 8.0 {
            return None;
        }
        let anchor = match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_swatch_bounds,
            ColorPickerTarget::Brush => self.brush_swatch_bounds,
        };
        let aw = f32::from(anchor.size.width);
        let ah = f32::from(anchor.size.height);
        if aw < 1.0 || ah < 1.0 {
            return None;
        }
        // 最近色 8 格单行: 8×22 + 7×gap4 = 204; 再加 padding/border
        let pop_w = Self::picker_pop_w();
        let pop_h = 14.0 + 16.0 + 22.0 + 8.0 + SB_SIZE + 36.0 + 44.0;
        let caret = 8.0;
        let gap = 6.0;
        let layer_left = f32::from(layer.origin.x);
        let layer_top = f32::from(layer.origin.y);
        let ax = f32::from(anchor.origin.x);
        let ay = f32::from(anchor.origin.y);
        let anchor_cx = ax + aw * 0.5 - layer_left;
        let anchor_top = ay - layer_top;
        let anchor_bot = ay + ah - layer_top;
        let space_below = ph - anchor_bot;
        let space_above = anchor_top;
        let place_below = space_below >= pop_h + caret + gap || space_below >= space_above;
        let left = (anchor_cx - pop_w * 0.5).clamp(4.0, (pw - pop_w - 4.0).max(4.0));
        let stack_h = pop_h + caret;
        let top = if place_below {
            anchor_bot + gap
        } else {
            (anchor_top - gap - stack_h).max(4.0)
        };
        let caret_x = (anchor_cx - left).clamp(10.0, pop_w - 10.0);
        Some((left, top, place_below, caret_x))
    }

    pub(super) fn picker_pop_w() -> f32 {
        // SB+色相列约 214; 最近色单行需 ≥ 8×22 + 7×4 + 内边距16 + 边框 ≈ 222
        (SB_SIZE + HUE_BAR_W + 8.0 + 16.0 + 4.0).max(232.0)
    }

    pub(super) fn picker_caret(place_below: bool, caret_x: f32) -> impl IntoElement {
        let h = 8.0_f32;
        let half = 8.0_f32;
        div()
            .w_full()
            .h(px(h))
            .relative()
            .child(
                canvas(|_, _, _| {}, {
                    move |bounds, _, window, _| {
                        let ox = f32::from(bounds.origin.x);
                        let oy = f32::from(bounds.origin.y);
                        let cx = ox + caret_x;
                        let mut builder = PathBuilder::fill();
                        if place_below {
                            builder.move_to(point(px(cx), px(oy)));
                            builder.line_to(point(px(cx - half), px(oy + h)));
                            builder.line_to(point(px(cx + half), px(oy + h)));
                        } else {
                            builder.move_to(point(px(cx), px(oy + h)));
                            builder.line_to(point(px(cx - half), px(oy)));
                            builder.line_to(point(px(cx + half), px(oy)));
                        }
                        builder.close();
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, rgb(0x1e293b));
                        }
                    }
                })
                .absolute()
                .size_full(),
            )
    }

    pub(super) fn color_picker_floating(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (left, top, place_below, caret_x) = self
            .color_picker_placement()
            .unwrap_or((8.0, 120.0, true, 40.0));
        let pop_w = Self::picker_pop_w();
        div()
            .id("color_picker_layer")
            .absolute()
            .inset_0()
            .child(
                canvas(
                    {
                        let entity = cx.entity().clone();
                        move |bounds, _, cx| {
                            entity.update(cx, |this, cx| {
                                let prev = this.picker_layer_bounds;
                                this.picker_layer_bounds = bounds;
                                // 首帧写入真实图层 bounds 后重算锚点, 纠正 padding 偏移
                                let changed = f32::from(prev.size.width) < 1.0
                                    || (f32::from(prev.origin.x) - f32::from(bounds.origin.x))
                                        .abs()
                                        > 0.5
                                    || (f32::from(prev.origin.y) - f32::from(bounds.origin.y))
                                        .abs()
                                        > 0.5;
                                if changed && this.color_picker_open {
                                    cx.notify();
                                }
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let x = f32::from(ev.position.x);
                    let y = f32::from(ev.position.y);
                    // 再点同一色块 = 提交并收起; 点另一色块 = 切换目标.
                    // 必须在遮罩层处理, 否则会先 close 再被下层按钮 open.
                    if Self::point_in_bounds(x, y, this.brush_swatch_bounds) {
                        this.open_color_picker(ColorPickerTarget::Brush, cx);
                        return;
                    }
                    if Self::point_in_bounds(x, y, this.mask_swatch_bounds) {
                        this.open_color_picker(ColorPickerTarget::Mask, cx);
                        return;
                    }
                    this.close_color_picker(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_color_picker(cx);
                }),
            )
            .child(
                div()
                    .id("color_picker_float")
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(pop_w))
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .when(place_below, |d| d.child(Self::picker_caret(true, caret_x)))
                    .child(self.color_picker_popover(cx))
                    .when(!place_below, |d| d.child(Self::picker_caret(false, caret_x))),
            )
    }

    pub(super) fn rebuild_hue_image(&mut self) {
        let w = 4u32;
        let h = HUE_TEX_H;
        let mut rgba: RgbaImage = ImageBuffer::new(w, h);
        for y in 0..h {
            let hue = 360.0 * y as f32 / (h - 1).max(1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, 1.0, 1.0);
            for x in 0..w {
                // GPUI RenderImage 用 BGRA
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        self.hue_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
    }

    pub(super) fn rebuild_sb_image(&mut self) {
        let size = SB_TEX_SIZE;
        let mut rgba: RgbaImage = ImageBuffer::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let s = x as f32 / (size - 1).max(1) as f32;
                let v = 1.0 - y as f32 / (size - 1).max(1) as f32;
                let [r, g, b] = hsv_to_rgb(self.picker_h, s, v);
                rgba.put_pixel(x, y, image::Rgba([b, g, r, 255]));
            }
        }
        self.sb_image = Some(Arc::new(RenderImage::new(smallvec![Frame::new(rgba)])));
    }

    pub(super) fn picker_rgb(&self) -> [u8; 3] {
        hsv_to_rgb(self.picker_h, self.picker_s, self.picker_v)
    }

    pub(super) fn sync_rgb_inputs_from_picker(&mut self, cx: &mut Context<Self>) {
        let [r, g, b] = self.picker_rgb();
        self.rgb_syncing = true;
        self.rgb_r_input
            .update(cx, |t, cx| t.set_text(r.to_string(), cx));
        self.rgb_g_input
            .update(cx, |t, cx| t.set_text(g.to_string(), cx));
        self.rgb_b_input
            .update(cx, |t, cx| t.set_text(b.to_string(), cx));
        self.rgb_syncing = false;
    }

    pub(super) fn apply_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        if self.rgb_syncing || !self.color_picker_open {
            return;
        }
        let blur = self.rgb_r_input.update(cx, |t, _| t.take_blur_commit())
            | self.rgb_g_input.update(cx, |t, _| t.take_blur_commit())
            | self.rgb_b_input.update(cx, |t, _| t.take_blur_commit());
        let parse = |s: String| -> Option<u8> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            t.parse::<u8>().ok()
        };
        let r = parse(self.rgb_r_input.read(cx).text());
        let g = parse(self.rgb_g_input.read(cx).text());
        let b = parse(self.rgb_b_input.read(cx).text());
        let (Some(r), Some(g), Some(b)) = (r, g, b) else {
            if blur {
                // 失焦时输入不完整: 回写当前合法色并视为已提交
                self.sync_rgb_inputs_from_picker(cx);
            }
            return;
        };
        if [r, g, b] == self.picker_rgb() {
            return;
        }
        self.set_picker_from_rgb([r, g, b], cx);
    }

    pub(super) fn set_picker_from_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.commit_picker_color(false);
        // 不回写文本框: 用户正在输入时回写会打断编辑
        cx.notify();
    }

    pub(super) fn sample_image_rgb(&self, ix: f32, iy: f32) -> Option<[u8; 3]> {
        if self.img_w == 0 || self.img_h == 0 {
            return None;
        }
        if !self.block_tiles.is_empty() {
            return self.sample_layered_rgb(ix, iy);
        }
        let img = self.rgb_image.as_ref()?;
        let x = ix.round().clamp(0.0, (self.img_w - 1) as f32) as u32;
        let y = iy.round().clamp(0.0, (self.img_h - 1) as f32) as u32;
        let p = img.get_pixel(x, y);
        Some([p[0], p[1], p[2]])
    }

    /// 取色预览: 只改色盘/HSV/目标色与 RGB 文本, 不改已选蒙版项、不入最近色.
    pub(super) fn preview_eyedropper_rgb(&mut self, rgb: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_color = rgb,
            ColorPickerTarget::Brush => self.brush_color = rgb,
        }
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    pub(super) fn arm_eyedropper(&mut self, cx: &mut Context<Self>) {
        if !self.color_picker_open {
            return;
        }
        if self.eyedropper_armed {
            self.cancel_eyedropper(cx);
            return;
        }
        let c = self.picker_rgb();
        self.eyedropper_backup = Some((self.picker_h, self.picker_s, self.picker_v, c));
        self.eyedropper_armed = true;
        self.status = "取色: 在左侧图上移动预览, 单击确认, Esc/右键取消".into();
        cx.notify();
    }

    pub(super) fn cancel_eyedropper(&mut self, cx: &mut Context<Self>) {
        if !self.eyedropper_armed {
            return;
        }
        if let Some((h, s, v, c)) = self.eyedropper_backup.take() {
            self.picker_h = h;
            self.picker_s = s;
            self.picker_v = v;
            self.rebuild_sb_image();
            match self.color_picker_target {
                ColorPickerTarget::Mask => self.mask_color = c,
                ColorPickerTarget::Brush => self.brush_color = c,
            }
            self.sync_rgb_inputs_from_picker(cx);
        }
        self.eyedropper_armed = false;
        self.status = "已取消取色".into();
        cx.notify();
    }

    pub(super) fn confirm_eyedropper_at(&mut self, ix: f32, iy: f32, cx: &mut Context<Self>) {
        if let Some(rgb) = self.sample_image_rgb(ix, iy) {
            self.preview_eyedropper_rgb(rgb, cx);
            self.commit_picker_color(true);
        }
        self.eyedropper_armed = false;
        self.eyedropper_backup = None;
        self.status = "已取色".into();
        cx.notify();
    }

    pub(super) fn commit_picker_color(&mut self, push_recent: bool) {
        let c = self.picker_rgb();
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_color = c,
            ColorPickerTarget::Brush => self.brush_color = c,
        }
        if !self.selected.is_empty() {
            if !self.opacity_undid {
                self.push_undo();
                self.opacity_undid = true;
            }
            for m in &mut self.masks {
                if self.selected.contains(&m.id) {
                    m.color = c;
                }
            }
        }
        if push_recent {
            let mut prefs = self.color_prefs();
            prefs.push_recent(c);
            self.recent_colors = prefs.recent_colors;
        }
        self.mark_prefs_dirty();
    }

    pub(super) fn apply_target_opacity(&mut self, v: f32) {
        let v = v.clamp(0.05, 1.0);
        match self.color_picker_target {
            ColorPickerTarget::Mask => self.mask_opacity = v,
            ColorPickerTarget::Brush => self.brush_opacity = v,
        }
        if !self.selected.is_empty() {
            if !self.opacity_undid {
                self.push_undo();
                self.opacity_undid = true;
            }
            for m in &mut self.masks {
                if self.selected.contains(&m.id) {
                    m.opacity = v;
                }
            }
        }
        self.mark_prefs_dirty();
    }

    pub(super) fn set_palette_opacity_from_x(&mut self, x: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.opacity_track.origin.x);
        let width = f32::from(self.opacity_track.size.width).max(1.0);
        let t = ((x - left) / width).clamp(0.0, 1.0);
        self.apply_target_opacity(0.05 + t * 0.95);
        cx.notify();
    }

    pub(super) fn set_palette_sb_from_pos(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let left = f32::from(self.sb_bounds.origin.x);
        let top = f32::from(self.sb_bounds.origin.y);
        let w = f32::from(self.sb_bounds.size.width).max(1.0);
        let h = f32::from(self.sb_bounds.size.height).max(1.0);
        self.picker_s = ((x - left) / w).clamp(0.0, 1.0);
        self.picker_v = (1.0 - (y - top) / h).clamp(0.0, 1.0);
        self.commit_picker_color(false);
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    pub(super) fn set_palette_hue_from_y(&mut self, y: f32, cx: &mut Context<Self>) {
        let top = f32::from(self.hue_bounds.origin.y);
        let h = f32::from(self.hue_bounds.size.height).max(1.0);
        self.picker_h = ((y - top) / h).clamp(0.0, 1.0) * 360.0;
        self.rebuild_sb_image();
        self.commit_picker_color(false);
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    pub(super) fn pick_recent_color(&mut self, color: [u8; 3], cx: &mut Context<Self>) {
        let (h, s, v) = rgb_to_hsv(color);
        self.picker_h = h;
        self.picker_s = s;
        self.picker_v = v;
        self.rebuild_sb_image();
        self.opacity_undid = false;
        self.commit_picker_color(true);
        self.opacity_undid = false;
        self.sync_rgb_inputs_from_picker(cx);
        cx.notify();
    }

    /// 当前滑条应对应的显示值: 有选中则取选中项平均透明度, 否则当前目标默认透明度.
    pub(super) fn slider_opacity_value(&self) -> f32 {
        if self.selected.is_empty() {
            return self.current_target_opacity();
        }
        let mut sum = 0.0f32;
        let mut n = 0usize;
        for m in &self.masks {
            if self.selected.contains(&m.id) {
                sum += m.effective_opacity();
                n += 1;
            }
        }
        if n == 0 {
            self.current_target_opacity()
        } else {
            sum / n as f32
        }
    }
}
