//! 画布坐标变换与命中检测 (对照 SheetView).

use gpui::{point, px, size, Bounds, Pixels, Point};

pub const EDGE_HIT_PX: f32 = 8.0;

#[derive(Clone, Copy)]
pub struct ViewXform {
    pub scale: f32,
    pub origin_x: f32,
    pub origin_y: f32,
}

impl ViewXform {
    pub fn compute(
        img_w: f32,
        img_h: f32,
        view_w: f32,
        view_h: f32,
        zoom: f32,
        pan: Point<f32>,
        user_zoomed: bool,
    ) -> Self {
        if img_w < 1.0 || img_h < 1.0 || view_w < 1.0 || view_h < 1.0 {
            return Self {
                scale: 1.0,
                origin_x: 0.0,
                origin_y: 0.0,
            };
        }
        let fit = (view_w / img_w).min(view_h / img_h).max(0.0001);
        let scale = if user_zoomed {
            (fit * zoom).max(0.0001)
        } else {
            fit
        };
        let drawn_w = img_w * scale;
        let drawn_h = img_h * scale;
        Self {
            scale,
            origin_x: (view_w - drawn_w) * 0.5 + pan.x,
            origin_y: (view_h - drawn_h) * 0.5 + pan.y,
        }
    }

    pub fn screen_to_image(&self, sx: f32, sy: f32) -> (f32, f32) {
        (
            (sx - self.origin_x) / self.scale,
            (sy - self.origin_y) / self.scale,
        )
    }

    pub fn image_rect_to_screen(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds<Pixels> {
        let left = self.origin_x + x0 as f32 * self.scale;
        let top = self.origin_y + y0 as f32 * self.scale;
        let right = self.origin_x + (x1 as f32 + 1.0) * self.scale;
        let bottom = self.origin_y + (y1 as f32 + 1.0) * self.scale;
        Bounds {
            origin: point(px(left), px(top)),
            size: size(px((right - left).max(1.0)), px((bottom - top).max(1.0))),
        }
    }

    pub fn edge_tol(&self) -> f32 {
        (EDGE_HIT_PX / self.scale).max(1.0)
    }
}

/// 当前视口应对应的原图像素矩形, 以及贴图像素 (≤ 窗口, 且不超过原图 1:1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewLod {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub tex_w: u32,
    pub tex_h: u32,
}

impl ViewLod {
    /// `pad` 是相对可见宽高的外扩比例, 平移时少重建.
    pub fn compute(
        xform: &ViewXform,
        view_w: f32,
        view_h: f32,
        img_w: u32,
        img_h: u32,
        pad: f32,
    ) -> Self {
        let img_w = img_w.max(1);
        let img_h = img_h.max(1);
        let (ax, ay) = xform.screen_to_image(0.0, 0.0);
        let (bx, by) = xform.screen_to_image(view_w, view_h);
        let mut x0 = ax.min(bx);
        let mut x1 = ax.max(bx);
        let mut y0 = ay.min(by);
        let mut y1 = ay.max(by);
        let pw = (x1 - x0).abs() * pad.max(0.0);
        let ph = (y1 - y0).abs() * pad.max(0.0);
        x0 = (x0 - pw).clamp(0.0, img_w as f32);
        x1 = (x1 + pw).clamp(0.0, img_w as f32);
        y0 = (y0 - ph).clamp(0.0, img_h as f32);
        y1 = (y1 + ph).clamp(0.0, img_h as f32);
        if x1 - x0 < 1.0 {
            x1 = (x0 + 1.0).min(img_w as f32);
        }
        if y1 - y0 < 1.0 {
            y1 = (y0 + 1.0).min(img_h as f32);
        }
        let x = x0.floor() as u32;
        let y = y0.floor() as u32;
        let w = ((x1.ceil() as u32).saturating_sub(x))
            .max(1)
            .min(img_w.saturating_sub(x));
        let h = ((y1.ceil() as u32).saturating_sub(y))
            .max(1)
            .min(img_h.saturating_sub(y));
        // 贴图像素 = 该矩形的屏幕大小, 但不超过原图 1:1.
        let cap = xform.scale.min(1.0).max(0.0001);
        let tex_w = ((w as f32) * cap).round().max(1.0) as u32;
        let tex_h = ((h as f32) * cap).round().max(1.0) as u32;
        Self {
            x,
            y,
            w,
            h,
            tex_w: tex_w.min(w),
            tex_h: tex_h.min(h),
        }
    }

    #[cfg(test)]
    pub fn is_full_page(&self, img_w: u32, img_h: u32) -> bool {
        self.x == 0 && self.y == 0 && self.w >= img_w && self.h >= img_h
    }

    /// `need` 通常不带 padding 的可见矩形; 当前块只要盖住它且密度够用.
    pub fn covers(&self, need: &ViewLod) -> bool {
        let nx1 = need.x.saturating_add(need.w);
        let ny1 = need.y.saturating_add(need.h);
        let cx1 = self.x.saturating_add(self.w);
        let cy1 = self.y.saturating_add(self.h);
        if self.x > need.x || self.y > need.y || cx1 < nx1 || cy1 < ny1 {
            return false;
        }
        let have = self.tex_w as f32 / self.w.max(1) as f32;
        let want = need.tex_w as f32 / need.w.max(1) as f32;
        have + 1e-6 >= want * 0.92
    }
}

/// `scene_x`/`img_w`: 只在鼠标真正悬浮于页面宽度范围内 (含 `tol` 容差) 才允许
/// 命中边界线; 页面左右之外即便同一高度也不应选中/拖动.
pub fn hit_edge(
    regions: &[(String, i32, i32)],
    selected: &std::collections::HashSet<String>,
    scene_x: f32,
    scene_y: f32,
    img_w: f32,
    tol: f32,
) -> Option<(String, &'static str)> {
    if scene_x < -tol || scene_x > img_w + tol {
        return None;
    }
    let mut candidates: Vec<(String, &'static str, f32, bool)> = Vec::new();
    for (rid, y0, y1) in regions {
        let d_top = (scene_y - *y0 as f32).abs();
        let d_bot = (scene_y - *y1 as f32).abs();
        let sel = selected.contains(rid);
        if d_top <= tol {
            candidates.push((rid.clone(), "top", d_top, sel));
        }
        if d_bot <= tol {
            candidates.push((rid.clone(), "bottom", d_bot, sel));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| match (!a.3).cmp(&(!b.3)) {
        std::cmp::Ordering::Equal => a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal),
        other => other,
    });
    Some((candidates[0].0.clone(), candidates[0].1))
}

/// 同 `hit_edge`: 只在页面宽度范围内才算命中块本体.
pub fn region_at(
    regions: &[(String, i32, i32)],
    selected: &std::collections::HashSet<String>,
    scene_x: f32,
    scene_y: f32,
    img_w: f32,
) -> Option<String> {
    if scene_x < 0.0 || scene_x > img_w {
        return None;
    }
    let mut hits: Vec<&(String, i32, i32)> = regions
        .iter()
        .filter(|(_, y0, y1)| *y0 as f32 <= scene_y && scene_y <= *y1 as f32)
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|(rid, y0, y1)| (if selected.contains(rid) { 0 } else { 1 }, -(*y1 - *y0)));
    Some(hits[0].0.clone())
}

use super::ScoreSyncApp;
use super::*;

impl ScoreSyncApp {
    pub(super) fn xform(&self) -> ViewXform {
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        ViewXform::compute(
            self.img_w as f32,
            self.img_h as f32,
            vw,
            vh,
            self.zoom,
            self.pan,
            self.user_zoomed,
        )
    }

    /// 限制平移: 画布边界就是页面边界, 页面填不满视口的方向强制居中,
    /// 填得满的方向最多平移到页面边缘贴视口边缘, 不允许露出页面之外的
    /// 空白, 也不能无限拖走.
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

    pub(super) fn screen_in_view(&self, pos: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(pos.x) - f32::from(self.view_bounds.origin.x),
            f32::from(pos.y) - f32::from(self.view_bounds.origin.y),
        )
    }

    pub(super) fn current_regions_hitlist(&self) -> Vec<(String, i32, i32)> {
        let Some(page) = self.doc.current_page() else {
            return Vec::new();
        };
        page.regions
            .values()
            .map(|r| (r.id.clone(), r.y0, r.y1))
            .collect()
    }

    pub(super) fn on_view_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        if event.button != MouseButton::Left {
            if event.button == MouseButton::Right {
                // 空白处右键无操作; 标签右键在标签栏处理
            }
            return;
        }
        let (sx, sy) = self.screen_in_view(event.position);
        let xform = self.xform();
        let (ix, iy) = xform.screen_to_image(sx, sy);
        let ctrl = is_primary_mod(&event.modifiers);

        if self.canvas_tool == CanvasTool::SplitBlock {
            self.push_crop_undo_current();
            let msg = self.doc.split_block_at(iy);
            self.status = msg.clone().into();
            self.hint = self.status.clone();
            if msg.contains("已在") {
                self.canvas_tool = CanvasTool::Normal;
            }
            self.after_doc_change(cx);
            return;
        }

        if self.canvas_tool == CanvasTool::AddBlock {
            let y = iy.round() as i32;
            self.drag = Some(DragKind::AddBlock {
                anchor_y: y,
                role: None,
                cur_y: y,
            });
            self.status = format!("锚定线 y={y}; 上移→下边线, 下移→上边线").into();
            cx.notify();
            return;
        }

        let regions = self.current_regions_hitlist();
        let tol = xform.edge_tol();
        let img_w = self.img_w as f32;
        if let Some((rid, edge)) =
            hit_edge(&regions, &self.doc.selected_region_ids, ix, iy, img_w, tol)
        {
            self.doc.click_region(&rid, ctrl);
            self.scroll_group_list_to_active();
            self.drag = Some(DragKind::Edge {
                region_id: rid,
                edge,
                undid: false,
            });
            self.after_doc_change(cx);
            return;
        }
        if let Some(rid) = region_at(&regions, &self.doc.selected_region_ids, ix, iy, img_w) {
            self.doc.click_region(&rid, ctrl);
            self.scroll_group_list_to_active();
            self.after_doc_change(cx);
            // 仍可开始平移
            self.drag = Some(DragKind::PagePan {
                last: event.position,
            });
            return;
        }
        self.doc.click_blank(ctrl);
        self.drag = Some(DragKind::PagePan {
            last: event.position,
        });
        self.after_doc_change(cx);
    }

    pub(super) fn on_view_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        let (sx, sy) = self.screen_in_view(event.position);
        let xform = self.xform();
        let (ix, iy) = xform.screen_to_image(sx, sy);

        let drag = self.drag.take();
        match drag {
            Some(DragKind::Edge {
                region_id,
                edge,
                undid,
            }) => {
                let mut undid = undid;
                if !undid {
                    self.push_crop_undo_current();
                    undid = true;
                }
                self.doc
                    .apply_edge_drag(&region_id, edge, iy.round() as i32);
                self.drag = Some(DragKind::Edge {
                    region_id,
                    edge,
                    undid,
                });
                self.hover_cursor = CursorStyle::ResizeUpDown;
                self.after_doc_change(cx);
                return;
            }
            Some(DragKind::AddBlock {
                anchor_y, mut role, ..
            }) => {
                let cur = iy.round() as i32;
                const LOCK_PX: i32 = 2;
                if role.is_none() {
                    let dy = cur - anchor_y;
                    if dy <= -LOCK_PX {
                        role = Some(AddAnchorRole::Bottom);
                    } else if dy >= LOCK_PX {
                        role = Some(AddAnchorRole::Top);
                    }
                }
                let (y0, y1) = Self::add_block_preview_ys(anchor_y, role, cur);
                self.status = match role {
                    None => format!("锚定 y={anchor_y} (再上下移动以确定上下边)").into(),
                    Some(AddAnchorRole::Top) => {
                        format!("上边 y={y0} · 下边 y={y1} (首线=上边)").into()
                    }
                    Some(AddAnchorRole::Bottom) => {
                        format!("上边 y={y0} · 下边 y={y1} (首线=下边)").into()
                    }
                };
                self.drag = Some(DragKind::AddBlock {
                    anchor_y,
                    role,
                    cur_y: cur,
                });
                self.hover_cursor = CursorStyle::Crosshair;
                cx.notify();
                return;
            }
            Some(DragKind::PagePan { last }) => {
                let dx = f32::from(event.position.x) - f32::from(last.x);
                let dy = f32::from(event.position.y) - f32::from(last.y);
                self.pan.x += dx;
                self.pan.y += dy;
                self.user_zoomed = true;
                self.clamp_pan();
                self.drag = Some(DragKind::PagePan {
                    last: event.position,
                });
                cx.notify();
                return;
            }
            other => {
                self.drag = other;
                if self.forward_capture_drags(
                    f32::from(event.position.x),
                    f32::from(event.position.y),
                    cx,
                ) {
                    return;
                }
            }
        }

        if matches!(
            self.canvas_tool,
            CanvasTool::AddBlock | CanvasTool::SplitBlock
        ) {
            self.hover_cursor = CursorStyle::Crosshair;
        } else {
            let regions = self.current_regions_hitlist();
            let tol = xform.edge_tol();
            let img_w = self.img_w as f32;
            if hit_edge(&regions, &self.doc.selected_region_ids, ix, iy, img_w, tol).is_some() {
                self.hover_cursor = CursorStyle::ResizeUpDown;
            } else {
                self.hover_cursor = CursorStyle::Arrow;
            }
        }
        let _ = window;
        cx.notify();
    }

    pub(super) fn on_view_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        if matches!(self.drag, Some(DragKind::AddBlock { .. })) {
            if let Some(DragKind::AddBlock {
                anchor_y,
                role,
                cur_y,
            }) = self.drag.take()
            {
                match role {
                    None => {
                        self.status = "已取消添加新块 (未确定上下边方向)".into();
                        self.hint = self.status.clone();
                    }
                    Some(_) => {
                        let (y0, y1) = Self::add_block_preview_ys(anchor_y, role, cur_y);
                        if y1 < y0 {
                            self.status = "块高度无效, 已取消.".into();
                        } else {
                            self.push_crop_undo_current();
                            let msg = self.doc.add_manual_block(y0, y1);
                            self.status = msg.into();
                            self.hint = self.status.clone();
                            self.canvas_tool = CanvasTool::Normal;
                            self.after_doc_change(cx);
                            return;
                        }
                    }
                }
                cx.notify();
                return;
            }
        }
        if matches!(
            self.drag,
            Some(DragKind::Edge { .. }) | Some(DragKind::PagePan { .. })
        ) {
            let edged = matches!(self.drag, Some(DragKind::Edge { undid: true, .. }));
            self.drag = None;
            if edged {
                self.after_doc_change(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(super) fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() {
            return;
        }
        let (delta_x, delta_y) = match event.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(l) => (l.x * 30.0, l.y * 30.0),
        };
        if is_primary_mod(&event.modifiers) {
            let zoom_delta = if delta_y.abs() > 0.01 {
                delta_y
            } else {
                delta_x
            };
            let (sx, sy) = self.screen_in_view(event.position);
            let xform = self.xform();
            let (ix, iy) = xform.screen_to_image(sx, sy);
            let vw = f32::from(self.view_bounds.size.width);
            let vh = f32::from(self.view_bounds.size.height);
            let fit = if self.img_w > 0 && self.img_h > 0 {
                (vw / self.img_w as f32)
                    .min(vh / self.img_h as f32)
                    .max(0.0001)
            } else {
                1.0
            };
            let factor = if zoom_delta > 0.0 { 1.15 } else { 1.0 / 1.15 };
            let current_zoom = if self.user_zoomed { self.zoom } else { 1.0 };
            self.user_zoomed = true;
            self.zoom = (current_zoom * factor).clamp(0.05, 40.0);
            let new_scale = fit * self.zoom;
            self.pan.x = sx - (vw - self.img_w as f32 * new_scale) * 0.5 - ix * new_scale;
            self.pan.y = sy - (vh - self.img_h as f32 * new_scale) * 0.5 - iy * new_scale;
            self.clamp_pan();
            cx.notify();
        } else {
            // 与蒙版一致: 滚轮的 x/y 都平移画布. Shift+滚轮在 Windows 上常把
            // 滚动量放进 x; 其它平台仍在 y, 此时把 y 当作横向位移.
            if event.modifiers.shift {
                let dx = if delta_x.abs() > 0.01 {
                    delta_x
                } else {
                    delta_y
                };
                self.pan.x += dx;
            } else {
                self.pan.x += delta_x;
                self.pan.y += delta_y;
            }
            self.user_zoomed = true;
            self.clamp_pan();
            cx.notify();
        }
    }

    pub(super) fn on_view_double_click(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || event.click_count < 2 {
            return;
        }
        let (sx, sy) = self.screen_in_view(event.position);
        let xform = self.xform();
        let (ix, iy) = xform.screen_to_image(sx, sy);
        let regions = self.current_regions_hitlist();
        let tol = xform.edge_tol();
        let img_w = self.img_w as f32;
        if hit_edge(&regions, &self.doc.selected_region_ids, ix, iy, img_w, tol).is_some()
            || region_at(&regions, &self.doc.selected_region_ids, ix, iy, img_w).is_some()
        {
            return;
        }
        self.fit_to_view(cx);
    }
    pub(super) fn image_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let render_image = self.render_image.clone();
        let fit_render_image = self.fit_render_image.clone();
        let lod_is_fit = self.lod_is_fit;
        let view_lod = self.view_lod;
        let regions: Vec<(String, i32, i32, u32, bool)> = self
            .doc
            .current_page()
            .map(|page| {
                page.regions
                    .values()
                    .map(|r| {
                        (
                            r.id.clone(),
                            r.y0,
                            r.y1,
                            parse_color_hex(&r.color),
                            self.doc.selected_region_ids.contains(&r.id),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let img_w = self.img_w;
        let img_h = self.img_h;
        let zoom = self.zoom;
        let pan = self.pan;
        let user_zoomed = self.user_zoomed;
        let cursor = if matches!(
            self.canvas_tool,
            CanvasTool::AddBlock | CanvasTool::SplitBlock
        ) {
            CursorStyle::Crosshair
        } else {
            self.hover_cursor
        };
        let add_preview = match &self.drag {
            Some(DragKind::AddBlock {
                anchor_y,
                role,
                cur_y,
            }) => Some(Self::add_block_preview_ys(*anchor_y, *role, *cur_y)),
            _ => None,
        };

        let loading = render_image.is_none() && !self.doc.pages.is_empty();

        div()
            .id("image_view")
            .flex_1()
            .min_w(px(200.))
            .min_w_0()
            .h_full()
            .bg(rgb(0x2b2b2b))
            .overflow_hidden()
            .relative()
            .cursor(cursor)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if ev.click_count >= 2 {
                        this.on_view_double_click(ev, cx);
                    } else {
                        this.on_view_mouse_down(ev, window, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(Self::on_view_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_view_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let list: Vec<PathBuf> = paths
                    .paths()
                    .iter()
                    .filter(|p| is_open_path(p) || is_project_path(p))
                    .cloned()
                    .collect();
                if list.is_empty() {
                    return;
                }
                if this.pdf_import.is_some() {
                    this.import_dialog_add_paths(list, cx);
                } else {
                    this.load_paths(list, cx);
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
                        let xform = ViewXform::compute(
                            img_w as f32,
                            img_h as f32,
                            vw,
                            vh,
                            zoom,
                            pan,
                            user_zoomed,
                        );

                        if let Some(ref img) = fit_render_image {
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
                            let _ = window.paint_image(
                                img_bounds,
                                gpui::Corners::default(),
                                img.clone(),
                                0,
                                false,
                            );
                        }
                        if !lod_is_fit {
                            if let (Some(img), Some(lod)) = (render_image.as_ref(), view_lod) {
                                let x1 = lod.x.saturating_add(lod.w.saturating_sub(1)) as i32;
                                let y1 = lod.y.saturating_add(lod.h.saturating_sub(1)) as i32;
                                let mut b =
                                    xform.image_rect_to_screen(lod.x as i32, lod.y as i32, x1, y1);
                                b.origin.x = bounds.origin.x + b.origin.x;
                                b.origin.y = bounds.origin.y + b.origin.y;
                                let _ = window.paint_image(
                                    b,
                                    gpui::Corners::default(),
                                    Arc::clone(img),
                                    0,
                                    false,
                                );
                            }
                        } else if fit_render_image.is_none() {
                            if let Some(ref img) = render_image {
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
                                let _ = window.paint_image(
                                    img_bounds,
                                    gpui::Corners::default(),
                                    img.clone(),
                                    0,
                                    false,
                                );
                            }
                        }

                        let mut sorted = regions.clone();
                        sorted.sort_by_key(|(_, _, _, _, sel)| if *sel { 1 } else { 0 });
                        for (_id, y0, y1, color, selected) in &sorted {
                            let mut b = xform.image_rect_to_screen(
                                0,
                                *y0,
                                img_w.saturating_sub(1) as i32,
                                *y1,
                            );
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill = rgb(*color);
                            fill.a = if *selected { 0.38 } else { 0.18 };
                            // 与蒙版选中一致: 红色粗边框, 更醒目
                            let border = if *selected {
                                rgb(0xdc5050)
                            } else {
                                rgb(*color)
                            };
                            let bw = if *selected { px(2.) } else { px(1.) };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill,
                                bw,
                                border,
                                Default::default(),
                            ));
                        }

                        if let Some((py0, py1)) = add_preview {
                            let mut b = xform.image_rect_to_screen(
                                0,
                                py0,
                                img_w.saturating_sub(1) as i32,
                                py1,
                            );
                            b.origin.x = bounds.origin.x + b.origin.x;
                            b.origin.y = bounds.origin.y + b.origin.y;
                            let mut fill = rgb(0xf59e0b);
                            fill.a = 0.28;
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                fill,
                                px(2.),
                                rgb(0xf59e0b),
                                Default::default(),
                            ));
                            // 锚定/活动边细线
                            for ly in [py0, py1] {
                                let mut lb = xform.image_rect_to_screen(
                                    0,
                                    ly,
                                    img_w.saturating_sub(1) as i32,
                                    ly,
                                );
                                lb.origin.x = bounds.origin.x + lb.origin.x;
                                lb.origin.y = bounds.origin.y + lb.origin.y;
                                lb.size.height = px(2.).max(lb.size.height);
                                window.paint_quad(quad(
                                    lb,
                                    px(0.),
                                    rgb(0xea580c),
                                    px(0.),
                                    rgb(0xea580c),
                                    Default::default(),
                                ));
                            }
                        }
                    },
                )
                .size_full(),
            )
            .when(loading, |d| {
                d.child(
                    div()
                        .id("page_loading")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0xe2e8f0))
                        .text_sm()
                        .child("加载中…"),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn regions() -> Vec<(String, i32, i32)> {
        vec![("r0".into(), 0, 99), ("r1".into(), 100, 199)]
    }

    #[test]
    fn hit_edge_ignores_points_left_or_right_of_page() {
        let regs = regions();
        let sel = HashSet::new();
        let img_w = 800.0;
        // 同一高度 (边界线 y=100 附近), 但 x 在页面之外.
        assert!(hit_edge(&regs, &sel, -20.0, 100.0, img_w, 8.0).is_none());
        assert!(hit_edge(&regs, &sel, img_w + 20.0, 100.0, img_w, 8.0).is_none());
        // 页面内同一高度应正常命中.
        assert!(hit_edge(&regs, &sel, img_w * 0.5, 100.0, img_w, 8.0).is_some());
        // 容差范围内的页边刚好还能命中 (悬浮在边界线上, 只是稍微出界一点点).
        assert!(hit_edge(&regs, &sel, -4.0, 100.0, img_w, 8.0).is_some());
    }

    #[test]
    fn region_at_ignores_points_left_or_right_of_page() {
        let regs = regions();
        let sel = HashSet::new();
        let img_w = 800.0;
        assert!(region_at(&regs, &sel, -5.0, 50.0, img_w).is_none());
        assert!(region_at(&regs, &sel, img_w + 5.0, 50.0, img_w).is_none());
        assert!(region_at(&regs, &sel, img_w * 0.5, 50.0, img_w).is_some());
    }

    #[test]
    fn view_lod_at_fit_covers_full_page_at_window_pixels() {
        let xform = ViewXform::compute(4500.0, 6000.0, 1400.0, 900.0, 1.0, point(0.0, 0.0), false);
        let lod = ViewLod::compute(&xform, 1400.0, 900.0, 4500, 6000, 0.0);
        assert!(lod.is_full_page(4500, 6000));
        assert!(lod.tex_w as f32 <= 1400.0 * 1.05);
        assert!(lod.tex_h as f32 <= 900.0 * 1.05);
        assert!(lod.tex_w < 4500);
        assert!(lod.tex_h < 6000);
    }

    #[test]
    fn view_lod_zoom_past_1_to_1_caps_at_source_pixels() {
        let fit = (1400.0_f32 / 4500.0).min(900.0 / 6000.0);
        // 相对原图约 2× (scale=2)
        let zoom = 2.0 / fit;
        let xform = ViewXform::compute(4500.0, 6000.0, 1400.0, 900.0, zoom, point(0.0, 0.0), true);
        assert!((xform.scale - 2.0).abs() < 0.05);
        let vis = ViewLod::compute(&xform, 1400.0, 900.0, 4500, 6000, 0.0);
        assert!(vis.w < 4500 || vis.h < 6000);
        assert_eq!(vis.tex_w, vis.w);
        assert_eq!(vis.tex_h, vis.h);
        assert!(vis.tex_w as f32 <= 1400.0 / 2.0 + 4.0);
    }

    #[test]
    fn view_lod_covers_padded_block() {
        let xform = ViewXform::compute(2000.0, 2000.0, 400.0, 400.0, 4.0, point(0.0, 0.0), true);
        let padded = ViewLod::compute(&xform, 400.0, 400.0, 2000, 2000, 0.25);
        let vis = ViewLod::compute(&xform, 400.0, 400.0, 2000, 2000, 0.0);
        assert!(padded.covers(&vis));
        let shifted = ViewLod {
            x: padded.x.saturating_add(padded.w),
            ..vis
        };
        assert!(!padded.covers(&shifted));
    }
}
