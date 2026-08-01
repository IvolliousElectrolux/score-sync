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

pub fn hit_edge(
    regions: &[(String, i32, i32)],
    selected: &std::collections::HashSet<String>,
    scene_y: f32,
    tol: f32,
) -> Option<(String, &'static str)> {
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

pub fn region_at(
    regions: &[(String, i32, i32)],
    selected: &std::collections::HashSet<String>,
    scene_y: f32,
) -> Option<String> {
    let mut hits: Vec<&(String, i32, i32)> = regions
        .iter()
        .filter(|(_, y0, y1)| *y0 as f32 <= scene_y && scene_y <= *y1 as f32)
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|(rid, y0, y1)| {
        (
            if selected.contains(rid) { 0 } else { 1 },
            -(*y1 - *y0),
        )
    });
    Some(hits[0].0.clone())
}
