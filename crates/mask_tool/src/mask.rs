//! 蒙版矩形 / 折线多边形 / 画笔描边与导出合成.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::{ImageBuffer, ImageFormat, Rgb};

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"];
pub const DEFAULT_MASK_OPACITY: f32 = 0.72;

pub fn is_image_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
            .unwrap_or(false)
}

pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:08x}", (nanos as u32).wrapping_mul(2654435761))
}

fn default_brush_color() -> [u8; 3] {
    [255, 255, 255]
}

fn default_mask_opacity() -> f32 {
    DEFAULT_MASK_OPACITY
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

/// 轴对齐矩形 / 折线多边形 / 画笔描边.
///
/// 旧工程只有 `id/x0/y0/x1/y1`, 新字段走默认值, 行为与原来一致.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MaskRect {
    pub id: String,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// 非空时为本条画笔描边的中心点折线 (图像坐标).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brush_points: Vec<(i32, i32)>,
    /// 画笔半径 (图像像素); 仅画笔有效.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub brush_radius: i32,
    /// 画笔 RGB; 矩形/多边形忽略, 始终按白色 + 本项不透明度合成.
    #[serde(default = "default_brush_color")]
    pub color: [u8; 3],
    /// 折线多边形顶点 (图像坐标, 按点击顺序, 已闭环无需重复首点).
    /// 非空且非画笔时按多边形填充.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub poly_points: Vec<(i32, i32)>,
    /// 本项不透明度 (0.05..=1). 旧工程缺省为 [`DEFAULT_MASK_OPACITY`].
    #[serde(default = "default_mask_opacity")]
    pub opacity: f32,
    /// 落笔时绑定的「组合分块」成员 (region_id); 拖动该块时这条画迹/蒙版
    /// 框整体跟着平移, 保证蒙版始终盖在同一块内容上. 落笔起点/终点所在块
    /// 冲突、都不在任何块内、或是旧工程 (无此字段) 时留空, 改为按当前
    /// 几何中心高度动态归属所在块 (随布局变化重新判定), 见
    /// `MaskToolApp::sync_masks_to_block_shift`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_block: Option<String>,
}

impl MaskRect {
    pub fn is_brush(&self) -> bool {
        !self.brush_points.is_empty()
    }

    pub fn is_poly(&self) -> bool {
        !self.is_brush() && self.poly_points.len() >= 3
    }

    pub fn effective_opacity(&self) -> f32 {
        self.opacity.clamp(0.05, 1.0)
    }

    pub fn refresh_brush_bounds(&mut self) {
        if self.brush_points.is_empty() {
            return;
        }
        let r = self.brush_radius.max(1);
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for &(x, y) in &self.brush_points {
            min_x = min_x.min(x - r);
            min_y = min_y.min(y - r);
            max_x = max_x.max(x + r);
            max_y = max_y.max(y + r);
        }
        self.x0 = min_x;
        self.y0 = min_y;
        self.x1 = max_x;
        self.y1 = max_y;
    }

    pub fn refresh_poly_bounds(&mut self) {
        if self.poly_points.is_empty() {
            return;
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for &(x, y) in &self.poly_points {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        self.x0 = min_x;
        self.y0 = min_y;
        self.x1 = max_x;
        self.y1 = max_y;
    }

    pub fn normalized(&self) -> MaskRect {
        if self.is_brush() {
            let mut m = self.clone();
            m.refresh_brush_bounds();
            return m;
        }
        if self.is_poly() {
            let mut m = self.clone();
            m.refresh_poly_bounds();
            return m;
        }
        MaskRect {
            id: self.id.clone(),
            x0: self.x0.min(self.x1),
            y0: self.y0.min(self.y1),
            x1: self.x0.max(self.x1),
            y1: self.y0.max(self.y1),
            brush_points: Vec::new(),
            brush_radius: 0,
            color: self.color,
            poly_points: Vec::new(),
            opacity: self.opacity,
            bound_block: self.bound_block.clone(),
        }
    }

    pub fn label(&self) -> String {
        if self.is_brush() {
            let r = self.brush_radius.max(1);
            return format!(
                "画笔 r={r}  {}点  α={:.0}%",
                self.brush_points.len(),
                self.effective_opacity() * 100.0
            );
        }
        if self.is_poly() {
            return format!(
                "折线 {}边  α={:.0}%",
                self.poly_points.len(),
                self.effective_opacity() * 100.0
            );
        }
        let r = self.normalized();
        format!(
            "({},{})–({},{})  {}×{}  α={:.0}%",
            r.x0,
            r.y0,
            r.x1,
            r.y1,
            r.x1 - r.x0 + 1,
            r.y1 - r.y0 + 1,
            self.effective_opacity() * 100.0
        )
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        if self.is_brush() {
            let r = self.brush_radius.max(1) as f32;
            let r2 = r * r;
            let pts = &self.brush_points;
            if pts.is_empty() {
                return false;
            }
            for &(px, py) in pts {
                let dx = x - px as f32;
                let dy = y - py as f32;
                if dx * dx + dy * dy <= r2 {
                    return true;
                }
            }
            for w in pts.windows(2) {
                let (x0, y0) = (w[0].0 as f32, w[0].1 as f32);
                let (x1, y1) = (w[1].0 as f32, w[1].1 as f32);
                if dist2_point_segment(x, y, x0, y0, x1, y1) <= r2 {
                    return true;
                }
            }
            return false;
        }
        if self.is_poly() {
            return point_in_poly(x, y, &self.poly_points);
        }
        let r = self.normalized();
        x >= r.x0 as f32
            && x <= (r.x1 as f32) + 1.0
            && y >= r.y0 as f32
            && y <= (r.y1 as f32) + 1.0
    }

    pub fn intersects_rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let r = self.normalized();
        let ax0 = x0.min(x1);
        let ax1 = x0.max(x1);
        let ay0 = y0.min(y1);
        let ay1 = y0.max(y1);
        !(r.x1 as f32 + 1.0 <= ax0
            || r.x0 as f32 >= ax1
            || r.y1 as f32 + 1.0 <= ay0
            || r.y0 as f32 >= ay1)
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        self.x0 += dx;
        self.x1 += dx;
        self.y0 += dy;
        self.y1 += dy;
        for p in &mut self.brush_points {
            p.0 += dx;
            p.1 += dy;
        }
        for p in &mut self.poly_points {
            p.0 += dx;
            p.1 += dy;
        }
    }

    pub fn offset_y(&mut self, dy: i32) {
        self.y0 += dy;
        self.y1 += dy;
        for p in &mut self.brush_points {
            p.1 += dy;
        }
        for p in &mut self.poly_points {
            p.1 += dy;
        }
    }

    /// 把所有坐标按 `f` 映射 (画布缩放/平移时用来整组换算蒙版).
    pub fn map_xy(&mut self, mut f: impl FnMut(i32, i32) -> (i32, i32)) {
        let (x0, y0) = f(self.x0, self.y0);
        let (x1, y1) = f(self.x1, self.y1);
        self.x0 = x0;
        self.y0 = y0;
        self.x1 = x1;
        self.y1 = y1;
        for p in &mut self.brush_points {
            *p = f(p.0, p.1);
        }
        for p in &mut self.poly_points {
            *p = f(p.0, p.1);
        }
        if self.is_brush() {
            self.refresh_brush_bounds();
        } else if self.is_poly() {
            self.refresh_poly_bounds();
        }
    }
}

/// 射线法判断点是否在多边形内.
pub fn point_in_poly(x: f32, y: f32, pts: &[(i32, i32)]) -> bool {
    if pts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = (pts[i].0 as f32, pts[i].1 as f32);
        let (xj, yj) = (pts[j].0 as f32, pts[j].1 as f32);
        let intersect = ((yi > y) != (yj > y))
            && (x < (xj - xi) * (y - yi) / (yj - yi + f32::EPSILON) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist2_point_segment(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let vx = x1 - x0;
    let vy = y1 - y0;
    let len2 = vx * vx + vy * vy;
    if len2 < 1e-6 {
        let dx = px - x0;
        let dy = py - y0;
        return dx * dx + dy * dy;
    }
    let t = ((px - x0) * vx + (py - y0) * vy) / len2;
    let t = t.clamp(0.0, 1.0);
    let qx = x0 + t * vx;
    let qy = y0 + t * vy;
    let dx = px - qx;
    let dy = py - qy;
    dx * dx + dy * dy
}

fn blend_color(p: &mut Rgb<u8>, color: [u8; 3], a: f32) {
    let inv = 1.0 - a;
    p[0] = (p[0] as f32 * inv + color[0] as f32 * a).round() as u8;
    p[1] = (p[1] as f32 * inv + color[1] as f32 * a).round() as u8;
    p[2] = (p[2] as f32 * inv + color[2] as f32 * a).round() as u8;
}

fn stamp_disk(
    rgb: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    cx: i32,
    cy: i32,
    radius: i32,
    color: [u8; 3],
    a: f32,
) {
    let (w, h) = rgb.dimensions();
    let r = radius.max(1);
    let r2 = (r as i64) * (r as i64);
    let x0 = (cx - r).max(0) as u32;
    let y0 = (cy - r).max(0) as u32;
    let x1 = ((cx + r).max(0) as u32).min(w.saturating_sub(1));
    let y1 = ((cy + r).max(0) as u32).min(h.saturating_sub(1));
    if x0 > x1 || y0 > y1 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as i64 - cx as i64;
            let dy = y as i64 - cy as i64;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            blend_color(rgb.get_pixel_mut(x, y), color, a);
        }
    }
}

fn stamp_polyline(
    rgb: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    points: &[(i32, i32)],
    radius: i32,
    color: [u8; 3],
    a: f32,
) {
    if points.is_empty() {
        return;
    }
    let r = radius.max(1);
    let step = (r as f32 * 0.5).max(1.0);
    stamp_disk(rgb, points[0].0, points[0].1, r, color, a);
    for w in points.windows(2) {
        let (x0, y0) = (w[0].0 as f32, w[0].1 as f32);
        let (x1, y1) = (w[1].0 as f32, w[1].1 as f32);
        let dist = ((x1 - x0).hypot(y1 - y0)).max(0.001);
        let n = (dist / step).ceil() as i32;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let x = (x0 + (x1 - x0) * t).round() as i32;
            let y = (y0 + (y1 - y0) * t).round() as i32;
            stamp_disk(rgb, x, y, r, color, a);
        }
    }
}

fn fill_poly_color(
    rgb: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    pts: &[(i32, i32)],
    color: [u8; 3],
    a: f32,
) {
    if pts.len() < 3 {
        return;
    }
    let (w, h) = rgb.dimensions();
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for &(x, y) in pts {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let x0 = min_x.max(0) as u32;
    let y0 = min_y.max(0) as u32;
    let x1 = (max_x.max(0) as u32).min(w.saturating_sub(1));
    let y1 = (max_y.max(0) as u32).min(h.saturating_sub(1));
    if x0 > x1 || y0 > y1 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            if point_in_poly(x as f32 + 0.5, y as f32 + 0.5, pts) {
                blend_color(rgb.get_pixel_mut(x, y), color, a);
            }
        }
    }
}

/// 在 RGB 图上叠蒙版. `default_opacity` 仅作兼容保留 (每项用自己的 `opacity`).
pub fn apply_masks_rgb(
    rgb: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    masks: &[MaskRect],
    _default_opacity: f32,
) {
    let (w, h) = rgb.dimensions();
    for m in masks {
        let a = m.effective_opacity();
        if m.is_brush() {
            stamp_polyline(rgb, &m.brush_points, m.brush_radius, m.color, a);
            continue;
        }
        if m.is_poly() {
            fill_poly_color(rgb, &m.poly_points, m.color, a);
            continue;
        }
        let r = m.normalized();
        let x0 = r.x0.max(0) as u32;
        let y0 = r.y0.max(0) as u32;
        let x1 = (r.x1.max(0) as u32).min(w.saturating_sub(1));
        let y1 = (r.y1.max(0) as u32).min(h.saturating_sub(1));
        if x0 > x1 || y0 > y1 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                blend_color(rgb.get_pixel_mut(x, y), m.color, a);
            }
        }
    }
}

pub fn export_masked(
    base: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    masks: &[MaskRect],
    opacity: f32,
    path: &Path,
) -> Result<(), String> {
    let mut rgb = base.clone();
    apply_masks_rgb(&mut rgb, masks, opacity);
    let format = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
        _ => ImageFormat::Png,
    };
    rgb.save_with_format(path, format)
        .map_err(|e| format!("保存失败: {e}"))
}

pub fn default_export_path(image_path: Option<&Path>) -> PathBuf {
    match image_path {
        Some(p) => p.with_file_name(format!(
            "{}_masked.png",
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("masked")
        )),
        None => PathBuf::from("masked.png"),
    }
}

pub fn first_image_in_paths(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| is_image_path(p)).cloned()
}
