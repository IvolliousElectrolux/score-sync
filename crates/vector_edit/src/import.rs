//! 从 PDF 页的一条 point 条带收成可编辑场景.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbaImage;
use pdfium_render::prelude::*;

use crate::document::{
    FillRule, ImageObj, LineCap, LineJoin, Obj, ObjKind, Paint, PathObj, Seg, SourceFingerprint,
    Stroke, Subpath, TextObj, VecDocument,
};
use crate::geom::{self, Rect};
use crate::ids::new_id;

const MAX_FORM_DEPTH: u8 = 8;

#[derive(Clone, Debug)]
pub struct ImportRequest {
    pub pdf_path: PathBuf,
    pub page_index: u32,
    pub y0: f32,
    pub y1: f32,
    pub extra_top: i32,
    pub extra_bottom: i32,
    pub extra_left: i32,
    pub extra_right: i32,
    pub page_id: String,
}

pub struct ImportReport {
    pub doc: VecDocument,
    pub truncated_forms: u32,
}

pub fn import_band(req: &ImportRequest) -> Result<ImportReport, String> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(&req.pdf_path, None)
        .map_err(|e| format!("打开矢量 PDF 失败: {e}"))?;
    let page = document
        .pages()
        .get(req.page_index as u16)
        .map_err(|e| format!("读取矢量页失败: {e}"))?;
    import_page(&page, req)
}

pub fn import_pdf_page(pdf_path: &Path, page_index: u32) -> Result<ImportReport, String> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| format!("打开 PDF 失败: {e}"))?;
    let page = document
        .pages()
        .get(page_index as u16)
        .map_err(|e| format!("读取页失败: {e}"))?;
    let (_page_w, h) = page_pt(&page);
    import_page(
        &page,
        &ImportRequest {
            pdf_path: pdf_path.to_path_buf(),
            page_index,
            y0: 0.0,
            y1: h,
            extra_top: 0,
            extra_bottom: 0,
            extra_left: 0,
            extra_right: 0,
            page_id: String::new(),
        },
    )
    .map(|report| report)
}

pub fn import_page(page: &PdfPage<'_>, req: &ImportRequest) -> Result<ImportReport, String> {
    let (page_w, page_h) = page_pt(page);
    let x_start = -(req.extra_left as f32);
    let x_end = page_w + req.extra_right as f32;
    let y_start = req.y0 - req.extra_top as f32;
    let y_end = req.y1 + req.extra_bottom as f32;
    let canvas_w = (x_end - x_start).max(1.0);
    let canvas_h = (y_end - y_start).max(1.0);
    let band = Rect::new(x_start, y_start, x_end, y_end);
    let mut acc = Collector {
        page_h,
        x_start,
        y_start,
        band,
        objects: Vec::new(),
        images: Default::default(),
        truncated: 0,
        text_slots: Vec::new(),
    };
    walk(page.objects().iter(), PdfMatrix::IDENTITY, &mut acc, 0);
    bake_isolated_text(page, &mut acc);
    let canvas = Rect::new(0.0, 0.0, canvas_w, canvas_h);
    let mut objects = Vec::new();
    for mut obj in acc.objects {
        if let ObjKind::Path(path) = &mut obj.kind {
            path.subpaths = geom::clip_subpaths(&path.subpaths, canvas);
            path.clip = geom::clip_subpaths(&path.clip, canvas);
            if path.subpaths.is_empty() {
                continue;
            }
        } else if let Some(b) = obj.bounds() {
            if !b.intersects(canvas) {
                continue;
            }
        }
        objects.push(obj);
    }
    let mut doc = VecDocument::from_objects(canvas_w, canvas_h, objects);
    doc.images = acc.images;
    doc.source = Some(SourceFingerprint {
        page_id: req.page_id.clone(),
        y0: req.y0.round() as i32,
        y1: req.y1.round() as i32,
        w: page_w.round().max(1.0) as u32,
        h: (req.y1 - req.y0).round().max(1.0) as u32,
        extra_top: req.extra_top,
        extra_bottom: req.extra_bottom,
        extra_left: req.extra_left,
        extra_right: req.extra_right,
    });
    Ok(ImportReport {
        doc,
        truncated_forms: acc.truncated,
    })
}

struct TextSlot {
    obj_index: usize,
    page_bounds: Rect,
}

struct Collector {
    page_h: f32,
    x_start: f32,
    y_start: f32,
    band: Rect,
    objects: Vec<Obj>,
    images: std::collections::HashMap<String, Arc<RgbaImage>>,
    truncated: u32,
    text_slots: Vec<TextSlot>,
}

fn walk<'a>(
    objects: impl Iterator<Item = PdfPageObject<'a>>,
    parent: PdfMatrix,
    acc: &mut Collector,
    depth: u8,
) {
    for obj in objects {
        if let Some(form) = obj.as_x_object_form_object() {
            if depth < MAX_FORM_DEPTH {
                let m = parent.multiply(form.matrix().unwrap_or(PdfMatrix::IDENTITY));
                walk(form.iter(), m, acc, depth + 1);
            } else {
                acc.truncated = acc.truncated.saturating_add(1);
            }
            continue;
        }
        if obj.as_path_object().is_some() {
            push_path(&obj, parent, acc);
        } else if obj.as_text_object().is_some() {
            push_text(&obj, parent, acc);
        } else if obj.as_image_object().is_some() {
            push_image(&obj, parent, acc);
        }
    }
}

fn push_path(obj: &PdfPageObject<'_>, parent: PdfMatrix, acc: &mut Collector) {
    let Some(path) = obj.as_path_object() else {
        return;
    };
    let acc_m = parent.multiply(path.matrix().unwrap_or(PdfMatrix::IDENTITY));
    let scale = matrix_scale(&acc_m);
    let subs = read_segments(path.segments().transform(acc_m).iter(), acc);
    if subs.is_empty() {
        return;
    }
    let Some(raw_bounds) = geom_of_subs(&subs) else {
        return;
    };
    let page_bounds = Rect::new(
        raw_bounds.x0 + acc.x_start,
        raw_bounds.y0 + acc.y_start,
        raw_bounds.x1 + acc.x_start,
        raw_bounds.y1 + acc.y_start,
    );
    if !page_bounds.intersects(acc.band) {
        return;
    }
    let fill_mode = path.fill_mode().unwrap_or(PdfPathFillMode::None);
    let stroked = path.is_stroked().unwrap_or(false);
    let fill = if fill_mode == PdfPathFillMode::None {
        None
    } else {
        paint_of(obj.fill_color().ok())
    };
    let stroke = if stroked { stroke_of(obj, scale) } else { None };
    let clip = read_clip(obj, &acc_m, acc);
    let rule = if fill_mode == PdfPathFillMode::EvenOdd {
        FillRule::EvenOdd
    } else {
        FillRule::Winding
    };
    if fill.is_none() && stroke.is_none() {
        return;
    }
    acc.objects.push(Obj::path(PathObj {
        subpaths: subs,
        fill,
        stroke,
        fill_rule: rule,
        clip,
    }));
}

fn push_text(obj: &PdfPageObject<'_>, parent: PdfMatrix, acc: &mut Collector) {
    let Some(text) = obj.as_text_object() else {
        return;
    };
    let Some(pb) = mapped_bounds(obj, &parent, acc.page_h) else {
        return;
    };
    let page_bounds = Rect::new(pb.0, pb.1, pb.2, pb.3);
    if !page_bounds.intersects(acc.band) {
        return;
    }
    let s = text.text();
    let local = to_local(page_bounds, acc);
    if s.trim().is_empty() {
        return;
    }
    let m = parent.multiply(text.matrix().unwrap_or(PdfMatrix::IDENTITY));
    let size = (text.unscaled_font_size().value * matrix_scale(&m)).max(0.5);
    let fill = paint_of(obj.fill_color().ok()).unwrap_or(Paint::rgb(0, 0, 0));
    let font_name = text.font().family();
    let obj = Obj::bare(
        new_id(),
        ObjKind::Text(TextObj {
            text: s,
            font_name,
            size,
            fill,
            origin: (local.x0, local.y1),
            baked_id: None,
            edited: false,
        }),
    );
    acc.text_slots.push(TextSlot {
        obj_index: acc.objects.len(),
        page_bounds,
    });
    acc.objects.push(obj);
}

fn push_image(obj: &PdfPageObject<'_>, parent: PdfMatrix, acc: &mut Collector) {
    let Some(image) = obj.as_image_object() else {
        return;
    };
    let Some(pb) = mapped_bounds(obj, &parent, acc.page_h) else {
        return;
    };
    let page_bounds = Rect::new(pb.0, pb.1, pb.2, pb.3);
    if !page_bounds.intersects(acc.band) {
        return;
    }
    let Ok(raw) = image.get_raw_image() else {
        return;
    };
    let id = new_id();
    acc.images.insert(id.clone(), Arc::new(raw.to_rgba8()));
    let local = to_local(page_bounds, acc);
    acc.objects.push(Obj::bare(
        new_id(),
        ObjKind::Image(ImageObj {
            png_id: id,
            x: local.x0,
            y: local.y0,
            w: local.w().max(0.5),
            h: local.h().max(0.5),
        }),
    ));
}

fn bake_isolated_text(page: &PdfPage<'_>, acc: &mut Collector) {
    let path_boxes: Vec<Rect> = acc
        .objects
        .iter()
        .filter_map(|o| match &o.kind {
            ObjKind::Path(_) => o.bounds().map(|b| {
                Rect::new(
                    b.x0 + acc.x_start,
                    b.y0 + acc.y_start,
                    b.x1 + acc.x_start,
                    b.y1 + acc.y_start,
                )
            }),
            _ => None,
        })
        .collect();
    let slots = std::mem::take(&mut acc.text_slots);
    for slot in slots {
        if path_boxes.iter().any(|b| b.intersects(slot.page_bounds)) {
            continue;
        }
        let w = slot.page_bounds.w().max(0.5);
        let h = slot.page_bounds.h().max(0.5);
        let tex_w = (w * 4.0).round().clamp(1.0, 512.0) as u32;
        let tex_h = (h * 4.0).round().clamp(1.0, 512.0) as u32;
        let Ok(img) = render_page_rect(
            page,
            slot.page_bounds.x0,
            slot.page_bounds.y0,
            w,
            h,
            tex_w,
            tex_h,
        ) else {
            continue;
        };
        let id = new_id();
        acc.images.insert(id.clone(), Arc::new(img));
        if let Some(ObjKind::Text(t)) = acc.objects.get_mut(slot.obj_index).map(|o| &mut o.kind) {
            t.baked_id = Some(id);
        }
    }
}

fn to_local(page_bounds: Rect, acc: &Collector) -> Rect {
    Rect::new(
        page_bounds.x0 - acc.x_start,
        page_bounds.y0 - acc.y_start,
        page_bounds.x1 - acc.x_start,
        page_bounds.y1 - acc.y_start,
    )
}

fn geom_of_subs(subs: &[Subpath]) -> Option<Rect> {
    let obj = Obj::path(PathObj {
        subpaths: subs.to_vec(),
        fill: None,
        stroke: None,
        fill_rule: FillRule::Winding,
        clip: Vec::new(),
    });
    obj.bounds()
}

fn read_segments<'a>(
    segments: impl Iterator<Item = PdfPathSegment<'a>>,
    acc: &Collector,
) -> Vec<Subpath> {
    let mut subs: Vec<Subpath> = Vec::new();
    let mut cur: Option<Subpath> = None;
    let mut prev: Option<(f32, f32)> = None;
    let mut bez: Vec<(f32, f32)> = Vec::new();
    let map = |x: PdfPoints, y: PdfPoints| {
        let page = (x.value, acc.page_h - y.value);
        (page.0 - acc.x_start, page.1 - acc.y_start)
    };
    let flush = |cur: &mut Option<Subpath>, subs: &mut Vec<Subpath>, closed: bool| {
        if let Some(mut sub) = cur.take() {
            if !sub.segs.is_empty() {
                sub.closed = closed || sub.closed;
                subs.push(sub);
            }
        }
    };
    for seg in segments {
        let (x, y_up) = seg.point();
        let p = map(x, y_up);
        match seg.segment_type() {
            PdfPathSegmentType::MoveTo | PdfPathSegmentType::Unknown => {
                flush(&mut cur, &mut subs, false);
                bez.clear();
                prev = Some(p);
                cur = Some(Subpath {
                    closed: false,
                    segs: Vec::new(),
                });
            }
            PdfPathSegmentType::LineTo => {
                bez.clear();
                if let (Some(a), Some(sub)) = (prev, cur.as_mut()) {
                    sub.segs.push(Seg::Line { a, b: p });
                    if seg.is_close() {
                        sub.closed = true;
                    }
                }
                prev = Some(p);
            }
            PdfPathSegmentType::BezierTo => {
                bez.push(p);
                if bez.len() == 3 {
                    if let (Some(a), Some(sub)) = (prev, cur.as_mut()) {
                        let end = bez[2];
                        sub.segs.push(Seg::Cubic {
                            p0: a,
                            p1: bez[0],
                            p2: bez[1],
                            p3: end,
                        });
                        if seg.is_close() {
                            sub.closed = true;
                        }
                        prev = Some(end);
                    }
                    bez.clear();
                }
            }
        }
    }
    flush(&mut cur, &mut subs, false);
    subs
}

fn read_clip(obj: &PdfPageObject<'_>, matrix: &PdfMatrix, acc: &Collector) -> Vec<Subpath> {
    let Some(clip) = obj.get_clip_path() else {
        return Vec::new();
    };
    if clip.is_empty() {
        return Vec::new();
    }
    let mut subs = Vec::new();
    for path in clip.iter() {
        let mut cur: Option<Subpath> = None;
        let mut prev = None;
        let mut bez = Vec::new();
        for seg in path.iter() {
            let (x, y_up) = seg.point();
            let (tx, ty) = apply_matrix(matrix, x.value, y_up.value);
            let p = (tx - acc.x_start, acc.page_h - ty - acc.y_start);
            match seg.segment_type() {
                PdfPathSegmentType::MoveTo | PdfPathSegmentType::Unknown => {
                    if let Some(sub) = cur.take() {
                        if !sub.segs.is_empty() {
                            subs.push(sub);
                        }
                    }
                    prev = Some(p);
                    bez.clear();
                    cur = Some(Subpath {
                        closed: true,
                        segs: Vec::new(),
                    });
                }
                PdfPathSegmentType::LineTo => {
                    if let (Some(a), Some(sub)) = (prev, cur.as_mut()) {
                        sub.segs.push(Seg::Line { a, b: p });
                    }
                    prev = Some(p);
                }
                PdfPathSegmentType::BezierTo => {
                    bez.push(p);
                    if bez.len() == 3 {
                        if let (Some(a), Some(sub)) = (prev, cur.as_mut()) {
                            sub.segs.push(Seg::Cubic {
                                p0: a,
                                p1: bez[0],
                                p2: bez[1],
                                p3: bez[2],
                            });
                            prev = Some(bez[2]);
                        }
                        bez.clear();
                    }
                }
            }
        }
        if let Some(sub) = cur.take() {
            if !sub.segs.is_empty() {
                subs.push(sub);
            }
        }
    }
    subs
}

fn paint_of(color: Option<PdfColor>) -> Option<Paint> {
    let c = color?;
    if c.alpha() == 0 {
        return None;
    }
    Some(Paint::rgba(c.red(), c.green(), c.blue(), c.alpha()))
}

fn stroke_of(obj: &PdfPageObject<'_>, scale: f32) -> Option<Stroke> {
    let paint = paint_of(obj.stroke_color().ok())?;
    let width = obj.stroke_width().map(|w| w.value).unwrap_or(1.0) * scale;
    let width = if width < 0.05 { 0.6 } else { width };
    let cap = match obj.line_cap().unwrap_or(PdfPageObjectLineCap::Butt) {
        PdfPageObjectLineCap::Butt => LineCap::Butt,
        PdfPageObjectLineCap::Round => LineCap::Round,
        PdfPageObjectLineCap::Square => LineCap::Square,
    };
    let join = match obj.line_join().unwrap_or(PdfPageObjectLineJoin::Miter) {
        PdfPageObjectLineJoin::Miter => LineJoin::Miter,
        PdfPageObjectLineJoin::Round => LineJoin::Round,
        PdfPageObjectLineJoin::Bevel => LineJoin::Bevel,
    };
    let dash = obj
        .dash_array()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.value * scale)
        .filter(|v| *v > 0.0)
        .collect::<Vec<_>>();
    let dash_phase = obj.dash_phase().map(|p| p.value * scale).unwrap_or(0.0);
    Some(Stroke {
        paint,
        width,
        cap,
        join,
        dash,
        dash_phase,
    })
}

fn matrix_scale(m: &PdfMatrix) -> f32 {
    let sx = (m.a() * m.a() + m.b() * m.b()).sqrt();
    let sy = (m.c() * m.c() + m.d() * m.d()).sqrt();
    ((sx + sy) * 0.5).max(0.01)
}

fn apply_matrix(m: &PdfMatrix, x: f32, y: f32) -> (f32, f32) {
    (m.a() * x + m.c() * y + m.e(), m.b() * x + m.d() * y + m.f())
}

fn mapped_bounds(
    obj: &PdfPageObject<'_>,
    parent: &PdfMatrix,
    page_h: f32,
) -> Option<(f32, f32, f32, f32)> {
    let q = obj.bounds().ok()?;
    let corners = [
        (q.left().value, q.bottom().value),
        (q.right().value, q.bottom().value),
        (q.right().value, q.top().value),
        (q.left().value, q.top().value),
    ];
    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);
    for (x, y) in corners {
        let (x, y) = parent.apply_to_points(PdfPoints::new(x), PdfPoints::new(y));
        xs.push(x.value);
        ys.push(page_h - y.value);
    }
    Some((
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    ))
}

fn page_pt(page: &PdfPage<'_>) -> (f32, f32) {
    (page.width().value.max(1.0), page.height().value.max(1.0))
}

pub fn render_page_rect(
    page: &PdfPage<'_>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tex_w: u32,
    tex_h: u32,
) -> Result<RgbaImage, String> {
    let rect_w = w.max(0.5);
    let rect_h = h.max(0.5);
    let tw = tex_w.max(1) as f32;
    let th = tex_h.max(1) as f32;
    let sx = tw / rect_w;
    let sy = th / rect_h;
    let cfg = PdfRenderConfig::new()
        .set_fixed_size(tw.round().max(1.0) as i32, th.round().max(1.0) as i32)
        .set_clear_color(PdfColor::new(0, 0, 0, 0))
        .transform(sx, 0.0, 0.0, sy, -x * sx, -y * sy)
        .map_err(|e| format!("光栅失败: {e}"))?;
    page.render_with_config(&cfg)
        .map(|bmp| bmp.as_image().into_rgba8())
        .map_err(|e| format!("光栅失败: {e}"))
}

fn bind_pdfium() -> Result<Pdfium, String> {
    let path = find_pdfium().ok_or_else(|| "找不到 pdfium.dll".to_string())?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
        .or_else(|_| Pdfium::bind_to_library(&path))
        .map_err(|e| format!("加载 pdfium 失败: {e}"))?;
    Ok(Pdfium::new(bindings))
}

fn find_pdfium() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let name = if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = dir.join(name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let c = dir.join(name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::rasterize_rgba;

    fn sample_pdf() -> Option<PathBuf> {
        let pdfium = bind_pdfium().ok()?;
        let mut doc = pdfium.create_new_pdf().ok()?;
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::Custom(
                PdfPoints::new(200.0),
                PdfPoints::new(80.0),
            ))
            .ok()?;
        let rect = PdfPagePathObject::new_rect(
            &doc,
            PdfRect::new(
                PdfPoints::new(20.0),
                PdfPoints::new(20.0),
                PdfPoints::new(80.0),
                PdfPoints::new(50.0),
            ),
            None,
            None,
            Some(PdfColor::new(0, 0, 0, 255)),
        )
        .ok()?;
        page.objects_mut().add_path_object(rect).ok()?;
        let line = PdfPagePathObject::new_line(
            &doc,
            PdfPoints::new(10.0),
            PdfPoints::new(10.0),
            PdfPoints::new(180.0),
            PdfPoints::new(10.0),
            PdfColor::new(0, 0, 0, 255),
            PdfPoints::new(2.0),
        )
        .ok()?;
        page.objects_mut().add_path_object(line).ok()?;
        let path = std::env::temp_dir().join(format!("vector_edit_sample_{}.pdf", new_id()));
        doc.save_to_file(&path).ok()?;
        drop(page);
        drop(doc);
        Some(path)
    }

    #[test]
    fn imported_scene_matches_pdfium_ink() {
        let Some(path) = sample_pdf() else {
            eprintln!("skip: pdfium dll 不在");
            return;
        };
        let report = import_band(&ImportRequest {
            pdf_path: path.clone(),
            page_index: 0,
            y0: 0.0,
            y1: 80.0,
            extra_top: 0,
            extra_bottom: 0,
            extra_left: 0,
            extra_right: 0,
            page_id: "p".into(),
        })
        .expect("import");
        assert!(
            report.doc.layers[0].objects.len() >= 2,
            "objects {}",
            report.doc.layers[0].objects.len()
        );
        let ours = rasterize_rgba(&report.doc, 2.0).unwrap();
        let pdfium = bind_pdfium().unwrap();
        let document = pdfium.load_pdf_from_file(&path, None).unwrap();
        let page = document.pages().get(0).unwrap();
        let theirs =
            render_page_rect(&page, 0.0, 0.0, 200.0, 80.0, ours.width(), ours.height()).unwrap();
        let mut ink_miss = 0u32;
        let mut ink_hit = 0u32;
        for (a, b) in ours.pixels().zip(theirs.pixels()) {
            let a_ink = a[3] > 180 && a[0] < 80;
            let b_ink = b[3] > 180 && b[0] < 80;
            if a_ink || b_ink {
                if a_ink && b_ink {
                    ink_hit += 1;
                } else {
                    ink_miss += 1;
                }
            }
        }
        let _ = std::fs::remove_file(&path);
        let total = ink_hit + ink_miss;
        assert!(total > 50, "no ink");
        let ratio = ink_miss as f32 / total as f32;
        assert!(
            ratio < 0.25,
            "ink mismatch {ratio:.2} miss {ink_miss} hit {ink_hit}"
        );
    }
}
