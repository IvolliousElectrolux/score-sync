//! 把场景写成 SVG, 给独立导出对照. 宿主成片不读 SVG.

use std::fmt::Write as _;

use crate::document::{ObjKind, Seg, VecDocument};
use crate::outline;

pub fn to_svg(doc: &VecDocument) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#,
        w = doc.canvas_w,
        h = doc.canvas_h
    );
    for layer in &doc.layers {
        if !layer.visible {
            continue;
        }
        let _ = writeln!(s, r#"<g opacity="{}">"#, layer.opacity);
        let skip = layer.suppressed_ids();
        write_items(&mut s, layer, &outline::roots_bottom_to_top(layer), &skip);
        let _ = writeln!(s, "</g>");
    }
    s.push_str("</svg>\n");
    s
}

fn write_items(
    s: &mut String,
    layer: &crate::document::LayerRuntime,
    ids: &[String],
    skip: &std::collections::HashSet<String>,
) {
    for id in ids {
        if let Some(group) = layer.groups.iter().find(|g| g.id == *id) {
            if !group.visible {
                continue;
            }
            let _ = writeln!(s, "<g>");
            write_items(s, layer, &group.children, skip);
            let _ = writeln!(s, "</g>");
        } else if let Some(obj) = layer.objects.iter().find(|o| o.id == *id) {
            if skip.contains(&obj.id) {
                continue;
            }
            match &obj.kind {
                ObjKind::Path(p) => {
                    let d = path_d(&p.subpaths);
                    let fill = p
                        .fill
                        .as_ref()
                        .map(|c| hex(c.rgba))
                        .unwrap_or_else(|| "none".into());
                    let (stroke, sw) = if let Some(st) = &p.stroke {
                        (hex(st.paint.rgba), format!("{}", st.width))
                    } else {
                        ("none".into(), "0".into())
                    };
                    let _ = writeln!(
                        s,
                        r#"<path d="{d}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"/>"#
                    );
                }
                ObjKind::Text(t) => {
                    let _ = writeln!(
                        s,
                        r#"<text x="{x}" y="{y}" font-size="{sz}" fill="{fill}">{text}</text>"#,
                        x = t.origin.0,
                        y = t.origin.1,
                        sz = t.size,
                        fill = hex(t.fill.rgba),
                        text = xml_escape(&t.text)
                    );
                }
                ObjKind::Image(im) => {
                    let _ = writeln!(
                        s,
                        r#"<image href="images/{id}.png" x="{x}" y="{y}" width="{w}" height="{h}"/>"#,
                        id = im.png_id,
                        x = im.x,
                        y = im.y,
                        w = im.w,
                        h = im.h
                    );
                }
            }
        }
    }
}

fn path_d(subs: &[crate::document::Subpath]) -> String {
    let mut d = String::new();
    for sub in subs {
        for (i, seg) in sub.segs.iter().enumerate() {
            match seg {
                Seg::Line { a, b } => {
                    if i == 0 {
                        let _ = write!(d, "M {} {} ", a.0, a.1);
                    }
                    let _ = write!(d, "L {} {} ", b.0, b.1);
                }
                Seg::Cubic { p0, p1, p2, p3 } => {
                    if i == 0 {
                        let _ = write!(d, "M {} {} ", p0.0, p0.1);
                    }
                    let _ = write!(
                        d,
                        "C {} {} {} {} {} {} ",
                        p1.0, p1.1, p2.0, p2.1, p3.0, p3.1
                    );
                }
            }
        }
        if sub.closed {
            d.push_str("Z ");
        }
    }
    d
}

fn hex(rgba: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2])
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
