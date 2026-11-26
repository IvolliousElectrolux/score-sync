//! 画笔显示.
//!
//! 点列仍是命中, 撤销和导出的来源. 界面上按当前缩放把中心折线描成
//! 圆头圆角的矢量笔画, 跟矢量页同一套屏幕分辨率, 放大不会再露出贴图像素.

use super::*;

/// 选中时比填充半径多出的屏幕像素 (每侧).
const HALO_PX: f32 = 2.0;
const HALO_COLOR: u32 = 0xdc5050;

pub(crate) struct BrushPaint {
    pub points: Vec<(f32, f32)>,
    pub radius: f32,
    pub color: [u8; 3],
    pub opacity: f32,
    pub selected: bool,
}

pub(crate) enum OverlayPaint {
    Brush(BrushPaint),
    Shape(MaskRect),
}

impl MaskToolApp {
    pub(super) fn overlay_paint_items(&self) -> Vec<OverlayPaint> {
        self.masks
            .iter()
            .map(|m| {
                if m.is_brush() {
                    OverlayPaint::Brush(BrushPaint {
                        points: m.brush_points.clone(),
                        radius: m.brush_radius.max(0.05),
                        color: m.color,
                        opacity: m.effective_opacity(),
                        selected: self.selected.contains(&m.id),
                    })
                } else if self.masks_are_sheet {
                    OverlayPaint::Shape(self.mask_sheet_to_canvas(m.clone()))
                } else {
                    OverlayPaint::Shape(m.clone())
                }
            })
            .collect()
    }
}

pub(crate) fn paint_brush_vector(
    window: &mut Window,
    brush: &BrushPaint,
    origin: Point<Pixels>,
    xform_origin_x: f32,
    xform_origin_y: f32,
    scale: f32,
    cs: f32,
    hoff: f32,
    voff: f32,
) {
    if brush.points.is_empty() || scale <= 0.0 {
        return;
    }
    let cs = if cs > 0.0001 { cs } else { 1.0 };
    let width = (brush.radius * 2.0 * cs * scale).max(1.0);
    let screen: Vec<Point<Pixels>> = brush
        .points
        .iter()
        .map(|&(x, y)| {
            let ix = hoff + x as f32 * cs;
            let iy = voff + y as f32 * cs;
            point(
                origin.x + px(xform_origin_x + ix * scale),
                origin.y + px(xform_origin_y + iy * scale),
            )
        })
        .collect();
    let [r, g, b] = brush.color;
    let mut fill = rgb(((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
    fill.a = brush.opacity.clamp(0.05, 1.0);
    if brush.selected {
        paint_stroke(window, &screen, width + HALO_PX * 2.0, rgb(HALO_COLOR));
    }
    // 不透明笔画用矢量. 半透明若直接描路径, 圆头和拐角的三角形会叠色, 笔迹里出现白纹.
    if fill.a < 0.999 && paint_uniform_stroke(window, &screen, width, [r, g, b], fill.a) {
        return;
    }
    paint_stroke(window, &screen, width, fill);
}

fn paint_uniform_stroke(
    window: &mut Window,
    pts: &[Point<Pixels>],
    width: f32,
    color: [u8; 3],
    opacity: f32,
) -> bool {
    if pts.is_empty() {
        return true;
    }
    let dpi = window.scale_factor().max(1.0);
    let radius = (width * 0.5 * dpi).max(0.5);
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for p in pts {
        let x = f32::from(p.x) * dpi;
        let y = f32::from(p.y) * dpi;
        min_x = min_x.min(x - radius);
        min_y = min_y.min(y - radius);
        max_x = max_x.max(x + radius);
        max_y = max_y.max(y + radius);
    }
    let origin_x = min_x.floor();
    let origin_y = min_y.floor();
    let w = (max_x.ceil() - origin_x).max(1.0) as u32;
    let h = (max_y.ceil() - origin_y).max(1.0) as u32;
    if w as u64 * h as u64 > 8_000_000 {
        return false;
    }
    let mut cover = vec![0f32; (w as usize) * (h as usize)];
    let stamp_at = |cover: &mut [f32], cx: f32, cy: f32| {
        let ix = cx.round() as i32;
        let iy = cy.round() as i32;
        let r = radius.ceil() as i32 + 1;
        let left = (ix - r).max(0);
        let top = (iy - r).max(0);
        let right = (ix + r).min(w as i32 - 1);
        let bottom = (iy + r).min(h as i32 - 1);
        if left > right || top > bottom {
            return;
        }
        for y in top..=bottom {
            let row = y as usize * w as usize;
            for x in left..=right {
                let d = ((x as f32 - cx).hypot(y as f32 - cy));
                let c = (radius + 0.5 - d).clamp(0.0, 1.0);
                if c > 0.0 {
                    let slot = &mut cover[row + x as usize];
                    if c > *slot {
                        *slot = c;
                    }
                }
            }
        }
    };
    let step = (radius * 0.45).max(1.0);
    let walk = |cover: &mut [f32], a: Point<Pixels>, b: Point<Pixels>| {
        let ax = f32::from(a.x) * dpi - origin_x;
        let ay = f32::from(a.y) * dpi - origin_y;
        let bx = f32::from(b.x) * dpi - origin_x;
        let by = f32::from(b.y) * dpi - origin_y;
        let dist = (bx - ax).hypot(by - ay).max(0.001);
        let n = (dist / step).ceil() as i32;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            stamp_at(cover, ax + (bx - ax) * t, ay + (by - ay) * t);
        }
    };
    if pts.len() == 1 {
        stamp_at(
            &mut cover,
            f32::from(pts[0].x) * dpi - origin_x,
            f32::from(pts[0].y) * dpi - origin_y,
        );
    } else {
        for pair in pts.windows(2) {
            walk(&mut cover, pair[0], pair[1]);
        }
    }
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    for (i, c) in cover.iter().copied().enumerate() {
        if c <= 0.0 {
            continue;
        }
        let a = (opacity * c * 255.0).round().clamp(0.0, 255.0) as u8;
        let p = i * 4;
        buf[p] = color[2];
        buf[p + 1] = color[1];
        buf[p + 2] = color[0];
        buf[p + 3] = a;
    }
    let img = RgbaImage::from_raw(w, h, buf).expect("brush coverage buffer");
    let tex = Arc::new(RenderImage::new(smallvec![Frame::new(img)]));
    let origin = point(px(origin_x / dpi), px(origin_y / dpi));
    let bounds = Bounds {
        origin,
        size: size(px(w as f32 / dpi), px(h as f32 / dpi)),
    };
    let _ = window.paint_image(bounds, Corners::default(), tex, 0, false);
    true
}

fn paint_stroke(window: &mut Window, pts: &[Point<Pixels>], width: f32, color: gpui::Rgba) {
    let width = width.max(1.0);
    if pts.is_empty() {
        return;
    }
    if collapsed(pts) {
        paint_disk(window, pts[0], width, color);
        return;
    }
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round);
    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
    let mut started = false;
    let mut last = pts[0];
    for &p in pts {
        if started && same_pt(last, p) {
            continue;
        }
        if started {
            builder.line_to(p);
        } else {
            builder.move_to(p);
            started = true;
        }
        last = p;
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn collapsed(pts: &[Point<Pixels>]) -> bool {
    let first = pts[0];
    pts.iter().all(|p| same_pt(first, *p))
}

fn same_pt(a: Point<Pixels>, b: Point<Pixels>) -> bool {
    (f32::from(a.x) - f32::from(b.x)).abs() < 0.05 && (f32::from(a.y) - f32::from(b.y)).abs() < 0.05
}

fn paint_disk(window: &mut Window, c: Point<Pixels>, diameter: f32, color: gpui::Rgba) {
    let d = diameter.max(1.0);
    let r = d * 0.5;
    window.paint_quad(quad(
        Bounds {
            origin: point(c.x - px(r), c.y - px(r)),
            size: size(px(d), px(d)),
        },
        px(r),
        color,
        px(0.),
        color,
        Default::default(),
    ));
}
