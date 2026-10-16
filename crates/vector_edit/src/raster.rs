//! tiny-skia 光栅. 编辑器瓦片和宿主成片共用.

use std::sync::OnceLock;

use fontdue::Font;
use image::RgbaImage;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, PremultipliedColorU8,
    Stroke, StrokeDash, Transform,
};

use crate::document::{Obj, ObjKind, Seg, Subpath, VecDocument};
use crate::geom;

pub fn rasterize_rgba(doc: &VecDocument, px_per_pt: f32) -> Result<RgbaImage, String> {
    let w = (doc.canvas_w * px_per_pt).round().max(1.0) as u32;
    let h = (doc.canvas_h * px_per_pt).round().max(1.0) as u32;
    rasterize_rgba_size(doc, w, h)
}

pub fn rasterize_rgba_size(doc: &VecDocument, tex_w: u32, tex_h: u32) -> Result<RgbaImage, String> {
    rasterize_rect(doc, 0.0, 0.0, doc.canvas_w, doc.canvas_h, tex_w, tex_h)
}

/// 文档矩形 `(x, y, w, h)` (point) 光栅成 `tex_w x tex_h`.
pub fn rasterize_rect(
    doc: &VecDocument,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tex_w: u32,
    tex_h: u32,
) -> Result<RgbaImage, String> {
    let tex_w = tex_w.max(1);
    let tex_h = tex_h.max(1);
    let w = w.max(0.5);
    let h = h.max(0.5);
    let sx = tex_w as f32 / w;
    let sy = tex_h as f32 / h;
    let mut pixmap = Pixmap::new(tex_w, tex_h).ok_or("光栅缓冲失败")?;
    let transform = Transform::from_translate(-x, -y).post_scale(sx, sy);
    let view = geom::Rect::new(x, y, x + w, y + h);
    for layer in &doc.layers {
        if !layer.visible || layer.opacity <= 0.001 {
            continue;
        }
        if layer.opacity >= 0.999 {
            paint_layer(&mut pixmap, doc, layer, transform, view);
        } else {
            let mut tmp = Pixmap::new(tex_w, tex_h).ok_or("光栅缓冲失败")?;
            paint_layer(&mut tmp, doc, layer, transform, view);
            pixmap.draw_pixmap(
                0,
                0,
                tmp.as_ref(),
                &PixmapPaint {
                    opacity: layer.opacity,
                    ..PixmapPaint::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    Ok(pixmap_to_rgba(&pixmap))
}

/// 跳过这些 id 再光栅整页. 拖动时底图留白, 选区单独画.
pub fn rasterize_filtered(
    doc: &VecDocument,
    px_per_pt: f32,
    skip: &std::collections::HashSet<&str>,
) -> Result<RgbaImage, String> {
    if skip.is_empty() {
        return rasterize_rgba(doc, px_per_pt);
    }
    let mut filtered = doc.clone();
    for layer in &mut filtered.layers {
        layer.objects.retain(|o| !skip.contains(o.id.as_str()));
    }
    rasterize_rgba(&filtered, px_per_pt)
}

/// 只画这些 id, 返回图像和文档空间矩形 (含一点边).
pub fn rasterize_ids(
    doc: &VecDocument,
    ids: &[String],
    px_per_pt: f32,
) -> Option<(RgbaImage, f32, f32, f32, f32)> {
    let leaves = doc.leaf_ids(ids);
    let set: std::collections::HashSet<&str> = leaves.iter().map(|s| s.as_str()).collect();
    let mut bounds: Option<geom::Rect> = None;
    for layer in &doc.layers {
        for obj in &layer.objects {
            if set.contains(obj.id.as_str()) {
                if let Some(b) = obj.bounds() {
                    bounds = Some(match bounds {
                        None => b,
                        Some(a) => a.union(b),
                    });
                }
            }
        }
    }
    let b = bounds?.pad(2.0);
    let mut filtered = doc.clone();
    for layer in &mut filtered.layers {
        layer.objects.retain(|o| set.contains(o.id.as_str()));
    }
    let img = rasterize_rect(
        &filtered,
        b.x0,
        b.y0,
        b.w().max(1.0),
        b.h().max(1.0),
        (b.w() * px_per_pt).round().max(1.0) as u32,
        (b.h() * px_per_pt).round().max(1.0) as u32,
    )
    .ok()?;
    Some((img, b.x0, b.y0, b.w().max(1.0), b.h().max(1.0)))
}

/// 选择窗格用的小图. 透明底以外先铺一层和墨色相反的底, 白线也能看见.
pub fn rasterize_thumb(doc: &VecDocument, ids: &[String], max_px: u32) -> Option<image::RgbaImage> {
    let leaves = doc.leaf_ids(ids);
    if leaves.is_empty() {
        return None;
    }
    let set: std::collections::HashSet<&str> = leaves.iter().map(|s| s.as_str()).collect();
    let mut bounds: Option<geom::Rect> = None;
    let mut ink: Option<[u8; 4]> = None;
    for layer in &doc.layers {
        for obj in &layer.objects {
            if !set.contains(obj.id.as_str()) {
                continue;
            }
            if ink.is_none() {
                ink = crate::document::obj_ink(obj);
            }
            if let Some(b) = obj.bounds() {
                bounds = Some(match bounds {
                    None => b,
                    Some(a) => a.union(b),
                });
            }
        }
    }
    let raw = bounds?;
    let b = raw.pad(raw.w().max(raw.h()).max(1.0) * 0.06 + 0.35);
    let max_px = max_px.clamp(8, 64);
    let bw = b.w().max(0.5);
    let bh = b.h().max(0.5);
    let scale = max_px as f32 / bw.max(bh);
    let tex_w = (bw * scale).round().clamp(1.0, max_px as f32) as u32;
    let tex_h = (bh * scale).round().clamp(1.0, max_px as f32) as u32;
    let mut pixmap = Pixmap::new(tex_w, tex_h)?;
    let bg = thumb_bg(ink);
    pixmap.fill(tiny_skia::Color::from_rgba8(bg[0], bg[1], bg[2], 255));
    let transform = Transform::from_translate(-b.x0, -b.y0).post_scale(tex_w as f32 / bw, tex_h as f32 / bh);
    for layer in &doc.layers {
        for obj in &layer.objects {
            if set.contains(obj.id.as_str()) {
                paint_obj(&mut pixmap, doc, obj, transform);
            }
        }
    }
    Some(pixmap_to_rgba(&pixmap))
}

fn thumb_bg(ink: Option<[u8; 4]>) -> [u8; 3] {
    let Some(rgba) = ink else {
        return [244, 244, 245];
    };
    let y = 0.2126 * rgba[0] as f32 + 0.7152 * rgba[1] as f32 + 0.0722 * rgba[2] as f32;
    if y >= 170.0 {
        [36, 36, 40]
    } else {
        [244, 244, 245]
    }
}

fn paint_layer(
    pixmap: &mut Pixmap,
    doc: &VecDocument,
    layer: &crate::document::LayerRuntime,
    transform: Transform,
    view: geom::Rect,
) {
    let skip = layer.suppressed_ids();
    for obj in &layer.objects {
        if skip.contains(&obj.id) {
            continue;
        }
        if obj.bounds().is_some_and(|b| !b.pad(2.0).intersects(view)) {
            continue;
        }
        paint_obj(pixmap, doc, obj, transform);
    }
}

fn paint_obj(pixmap: &mut Pixmap, doc: &VecDocument, obj: &Obj, transform: Transform) {
    match &obj.kind {
        ObjKind::Path(path) => {
            let built = build_path(&path.subpaths);
            let Some(sk) = built else {
                return;
            };
            let mask = clip_mask(pixmap.width(), pixmap.height(), &path.clip, transform);
            let rule = match path.fill_rule {
                crate::document::FillRule::Winding => FillRule::Winding,
                crate::document::FillRule::EvenOdd => FillRule::EvenOdd,
            };
            if let Some(fill) = &path.fill {
                let mut paint = Paint::default();
                paint.anti_alias = true;
                paint.set_color_rgba8(fill.rgba[0], fill.rgba[1], fill.rgba[2], fill.rgba[3]);
                pixmap.fill_path(&sk, &paint, rule, transform, mask.as_ref());
            }
            if let Some(st) = &path.stroke {
                let mut paint = Paint::default();
                paint.anti_alias = true;
                paint.set_color_rgba8(st.paint.rgba[0], st.paint.rgba[1], st.paint.rgba[2], st.paint.rgba[3]);
                let mut stroke = Stroke {
                    width: st.width.max(0.15),
                    miter_limit: 4.0,
                    line_cap: match st.cap {
                        crate::document::LineCap::Butt => LineCap::Butt,
                        crate::document::LineCap::Round => LineCap::Round,
                        crate::document::LineCap::Square => LineCap::Square,
                    },
                    line_join: match st.join {
                        crate::document::LineJoin::Miter => LineJoin::Miter,
                        crate::document::LineJoin::Round => LineJoin::Round,
                        crate::document::LineJoin::Bevel => LineJoin::Bevel,
                    },
                    dash: None,
                };
                if !st.dash.is_empty() {
                    stroke.dash = StrokeDash::new(st.dash.clone(), st.dash_phase);
                }
                pixmap.stroke_path(&sk, &paint, &stroke, transform, mask.as_ref());
            }
        }
        ObjKind::Text(text) => {
            if !text.edited {
                if let Some(id) = &text.baked_id {
                    if let Some(img) = doc.images.get(id) {
                        blit_image(pixmap, img, text.origin.0, text.origin.1 - text.size, text.size * 0.6 * text.text.chars().count().max(1) as f32, text.size, transform);
                        return;
                    }
                }
            }
            draw_text(pixmap, &text.text, text.size, text.origin, text.fill.rgba, transform);
        }
        ObjKind::Image(im) => {
            if let Some(img) = doc.images.get(&im.png_id) {
                blit_image(pixmap, img, im.x, im.y, im.w, im.h, transform);
            }
        }
    }
}

fn build_path(subs: &[Subpath]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let mut any = false;
    for sub in subs {
        let mut started = false;
        for seg in &sub.segs {
            match *seg {
                Seg::Line { a, b } => {
                    if !started {
                        pb.move_to(a.0, a.1);
                        started = true;
                    }
                    pb.line_to(b.0, b.1);
                    any = true;
                }
                Seg::Cubic { p0, p1, p2, p3 } => {
                    if !started {
                        pb.move_to(p0.0, p0.1);
                        started = true;
                    }
                    pb.cubic_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
                    any = true;
                }
            }
        }
        if sub.closed && started {
            pb.close();
        }
    }
    if any {
        pb.finish()
    } else {
        None
    }
}

fn clip_mask(w: u32, h: u32, clip: &[Subpath], transform: Transform) -> Option<Mask> {
    if clip.is_empty() {
        return None;
    }
    let path = build_path(clip)?;
    let mut mask = Mask::new(w, h)?;
    mask.fill_path(&path, FillRule::Winding, true, transform);
    Some(mask)
}

fn blit_image(pixmap: &mut Pixmap, img: &RgbaImage, x: f32, y: f32, w: f32, h: f32, transform: Transform) {
    let (iw, ih) = img.dimensions();
    if iw == 0 || ih == 0 || w < 0.2 || h < 0.2 {
        return;
    }
    let Some(mut src) = Pixmap::new(iw, ih) else {
        return;
    };
    for (i, p) in img.pixels().enumerate() {
        let pm = premultiply(p[0], p[1], p[2], p[3]);
        src.pixels_mut()[i] = pm;
    }
    let (ox, oy) = transform_point(transform, x, y);
    let (ox2, oy2) = transform_point(transform, x + w, y + h);
    let dw = (ox2 - ox).abs().max(1.0);
    let dh = (oy2 - oy).abs().max(1.0);
    let sx = dw / iw as f32;
    let sy = dh / ih as f32;
    pixmap.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &PixmapPaint::default(),
        Transform::from_scale(sx, sy).post_translate(ox.min(ox2), oy.min(oy2)),
        None,
    );
}

fn transform_point(t: Transform, x: f32, y: f32) -> (f32, f32) {
    let sx = t.sx;
    let sy = t.sy;
    let tx = t.tx;
    let ty = t.ty;
    (x * sx + tx, y * sy + ty)
}

fn draw_text(
    pixmap: &mut Pixmap,
    text: &str,
    size: f32,
    origin: (f32, f32),
    rgba: [u8; 4],
    transform: Transform,
) {
    let Some(font) = ui_font() else {
        return;
    };
    let px = (size * transform.sx.abs()).clamp(4.0, 256.0);
    let (ox, oy) = transform_point(transform, origin.0, origin.1);
    let mut pen_x = ox;
    let baseline = oy;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, px);
        if metrics.width == 0 || metrics.height == 0 {
            pen_x += metrics.advance_width;
            continue;
        }
        let mut glyph = match Pixmap::new(metrics.width as u32, metrics.height as u32) {
            Some(p) => p,
            None => continue,
        };
        for (i, a) in bitmap.iter().enumerate() {
            let a = *a;
            if a == 0 {
                continue;
            }
            let r = (rgba[0] as u16 * a as u16 / 255) as u8;
            let g = (rgba[1] as u16 * a as u16 / 255) as u8;
            let b = (rgba[2] as u16 * a as u16 / 255) as u8;
            let al = (rgba[3] as u16 * a as u16 / 255) as u8;
            glyph.pixels_mut()[i] = premultiply(r, g, b, al);
        }
        let gx = (pen_x + metrics.xmin as f32).round() as i32;
        let gy = (baseline - metrics.ymin as f32 - metrics.height as f32).round() as i32;
        pixmap.draw_pixmap(gx, gy, glyph.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        pen_x += metrics.advance_width;
    }
}

fn premultiply(r: u8, g: u8, b: u8, a: u8) -> PremultipliedColorU8 {
    PremultipliedColorU8::from_rgba(
        ((r as u16 * a as u16) / 255) as u8,
        ((g as u16 * a as u16) / 255) as u8,
        ((b as u16 * a as u16) / 255) as u8,
        a,
    )
    .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

fn pixmap_to_rgba(pixmap: &Pixmap) -> RgbaImage {
    let w = pixmap.width();
    let h = pixmap.height();
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        buf.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    RgbaImage::from_raw(w, h, buf).unwrap_or_else(|| RgbaImage::new(w, h))
}

fn ui_font() -> Option<&'static Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        let candidates = [
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\simhei.ttf",
            r"C:\Windows\Fonts\arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        for p in candidates {
            if let Ok(bytes) = std::fs::read(p) {
                if let Ok(font) = Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                    return Some(font);
                }
            }
        }
        None
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{FillRule, LineCap, LineJoin, Paint, PathObj, Seg, Stroke, Subpath};

    #[test]
    fn filled_rect_is_dark_inside() {
        let doc = VecDocument::from_objects(
            40.0,
            20.0,
            vec![Obj::path(PathObj {
                subpaths: vec![Subpath {
                    closed: true,
                    segs: vec![
                        Seg::Line { a: (5.0, 4.0), b: (30.0, 4.0) },
                        Seg::Line { a: (30.0, 4.0), b: (30.0, 16.0) },
                        Seg::Line { a: (30.0, 16.0), b: (5.0, 16.0) },
                        Seg::Line { a: (5.0, 16.0), b: (5.0, 4.0) },
                    ],
                }],
                fill: Some(Paint::rgb(0, 0, 0)),
                stroke: Some(Stroke {
                    paint: Paint::rgb(0, 0, 0),
                    width: 0.6,
                    cap: LineCap::Butt,
                    join: LineJoin::Miter,
                    dash: Vec::new(),
                    dash_phase: 0.0,
                }),
                fill_rule: FillRule::Winding,
                clip: Vec::new(),
            })],
        );
        let img = rasterize_rgba(&doc, 2.0).unwrap();
        let p = img.get_pixel(20, 20);
        assert!(p[3] > 200, "alpha {}", p[3]);
        assert!(p[0] < 40 && p[1] < 40 && p[2] < 40);
        let outside = img.get_pixel(2, 2);
        assert_eq!(outside[3], 0);
    }
}
