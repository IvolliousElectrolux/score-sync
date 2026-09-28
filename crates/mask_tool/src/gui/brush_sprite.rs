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
    paint_stroke(window, &screen, width, fill);
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
