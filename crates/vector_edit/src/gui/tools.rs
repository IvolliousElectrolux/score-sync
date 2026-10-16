//! 工具对文档的改动. 指针状态机在 canvas.

use super::*;
use crate::document::{FillRule, Obj, ObjKind, Paint, PathObj, Seg, Stroke, Subpath};
use crate::geom::{self, Rect};

pub(crate) fn commit_pen(
    app: &mut VectorEditApp,
    pen: &PenDraft,
    closed: bool,
    cx: &mut gpui::Context<VectorEditApp>,
) {
    if pen.nodes.len() < 2 {
        return;
    }
    let nodes = closing_nodes(pen, closed);
    let segs = nodes_to_segs(&nodes);
    if segs.is_empty() {
        return;
    }
    app.push_undo();
    if let Some(id) = &pen.continue_id {
        if let Some(doc) = app.doc.as_mut() {
            doc.delete_ids(std::slice::from_ref(id));
        }
    }
    let style = app.style.clone();
    if let Some(doc) = app.doc.as_mut() {
        let obj = styled_path(vec![Subpath { closed, segs }], &style);
        let id = obj.id.clone();
        if let Some(layer) = doc.active_layer_mut() {
            layer.objects.push(std::sync::Arc::new(obj));
        }
        app.selected = vec![id];
        app.dirty = true;
    }
    app.rebuild_raster(cx);
}

pub(crate) fn finish_pencil(app: &mut VectorEditApp, pts: &[(f32, f32)], cx: &mut gpui::Context<VectorEditApp>) {
    if pts.len() < 2 {
        return;
    }
    let segs = geom::fit_cubics(pts);
    if segs.is_empty() {
        return;
    }
    app.push_undo();
    let mut style = app.style.clone();
    style.width = app.brush.max(0.2);
    if let Some(doc) = app.doc.as_mut() {
        let obj = styled_path(vec![Subpath { closed: false, segs }], &style);
        let id = obj.id.clone();
        if let Some(layer) = doc.active_layer_mut() {
            layer.objects.push(std::sync::Arc::new(obj));
        }
        app.selected = vec![id];
        app.dirty = true;
    }
    app.rebuild_raster(cx);
}

pub(crate) fn erase_at(app: &mut VectorEditApp, x: f32, y: f32, cx: &mut gpui::Context<VectorEditApp>) {
    let tol = app.hit_tol();
    let Some(target) = app.doc.as_ref().and_then(|d| d.hit_test(x, y, tol)) else {
        return;
    };
    app.push_undo();
    let removed = app.doc.as_mut().is_some_and(|doc| {
        let before = doc.layers.iter().map(|l| l.objects.len()).sum::<usize>();
        doc.delete_ids(std::slice::from_ref(&target));
        doc.layers.iter().map(|l| l.objects.len()).sum::<usize>() != before
    });
    if !removed {
        app.undo_stack.pop();
        return;
    }
    app.selected.retain(|s| s != &target);
    if app.node_target.as_ref() == Some(&target) {
        app.node_target = None;
        app.node_hit = None;
    }
    app.dirty = true;
    app.rebuild_raster(cx);
    app.notify_chrome(cx);
}

pub(crate) fn apply_marquee(app: &mut VectorEditApp, rect: Rect, add: bool, sub: bool, toggle: bool) {
    let Some(doc) = app.doc.as_ref() else {
        return;
    };
    let hit = promote_hits(app, doc.objects_in_rect(rect), sub);
    combine_sel(app, hit, add, sub, toggle);
}

pub(crate) fn apply_lasso(app: &mut VectorEditApp, poly: &[(f32, f32)]) {
    let Some(doc) = app.doc.as_ref() else {
        return;
    };
    let (add, sub, toggle) = app.sel_mods();
    let hit = promote_hits(app, doc.objects_in_poly(poly), sub);
    combine_sel(app, hit, add, sub, toggle);
}

pub(crate) fn click_select(app: &mut VectorEditApp, x: f32, y: f32) {
    let tol = app.hit_tol();
    let hit = app.doc.as_ref().and_then(|d| d.hit_test(x, y, tol));
    let (add, sub, toggle) = app.sel_mods();
    match hit {
        Some(leaf) => {
            let id = app
                .doc
                .as_ref()
                .map(|d| {
                    if sub {
                        d.selection_anchor(&app.selected, &leaf)
                            .unwrap_or_else(|| d.pick_target(&leaf))
                    } else if !add && !toggle {
                        d.plain_click_target(&app.selected, &leaf)
                    } else {
                        d.pick_target(&leaf)
                    }
                })
                .unwrap_or(leaf);
            if sub {
                app.selected.retain(|s| s != &id);
            } else if toggle {
                if let Some(i) = app.selected.iter().position(|s| s == &id) {
                    app.selected.remove(i);
                } else {
                    app.selected.push(id);
                }
            } else if add {
                if !app.selected.iter().any(|s| s == &id) {
                    app.selected.push(id);
                }
            } else if !app.selected.iter().any(|s| s == &id) {
                app.selected = vec![id];
            }
        }
        None if !add && !sub && !toggle => app.selected.clear(),
        None => {}
    }
}

fn promote_hits(app: &VectorEditApp, leaves: Vec<String>, sub: bool) -> Vec<String> {
    let Some(doc) = app.doc.as_ref() else {
        return leaves;
    };
    let mut hit = Vec::new();
    for leaf in leaves {
        let id = if sub {
            doc.selection_anchor(&app.selected, &leaf)
                .unwrap_or_else(|| doc.pick_target(&leaf))
        } else {
            doc.pick_target(&leaf)
        };
        if !hit.iter().any(|h| h == &id) {
            hit.push(id);
        }
    }
    hit
}

fn combine_sel(app: &mut VectorEditApp, hit: Vec<String>, add: bool, sub: bool, toggle: bool) {
    if sub {
        app.selected.retain(|id| !hit.iter().any(|h| h == id));
    } else if toggle {
        for id in hit {
            if let Some(i) = app.selected.iter().position(|s| s == &id) {
                app.selected.remove(i);
            } else {
                app.selected.push(id);
            }
        }
    } else if add {
        for id in hit {
            if !app.selected.iter().any(|s| s == &id) {
                app.selected.push(id);
            }
        }
    } else {
        app.selected = hit;
    }
}

pub(crate) fn sample_style(app: &mut VectorEditApp, x: f32, y: f32) -> bool {
    let tol = app.hit_tol();
    let Some(id) = app.doc.as_ref().and_then(|d| d.hit_test(x, y, tol)) else {
        return false;
    };
    let Some(obj) = app.doc.as_ref().and_then(|d| d.object(&id)) else {
        return false;
    };
    match &obj.kind {
        ObjKind::Path(p) => {
            app.style.fill = p.fill.map(|c| c.rgba);
            if let Some(st) = &p.stroke {
                app.style.stroke = Some(st.paint.rgba);
                app.style.width = st.width;
                app.style.cap = st.cap;
                app.style.join = st.join;
            } else {
                app.style.stroke = None;
            }
        }
        ObjKind::Text(t) => {
            app.style.fill = Some(t.fill.rgba);
            app.style.stroke = None;
        }
        ObjKind::Image(_) => return false,
    }
    app.remember_paints();
    true
}

pub(crate) fn write_style(app: &mut VectorEditApp) {
    let style = app.style.clone();
    let ids = app.selected.clone();
    if let Some(doc) = app.doc.as_mut() {
        for id in &ids {
            doc.mutate(id, |o| apply_style(o, &style));
        }
    }
    app.dirty = true;
}

pub(crate) fn move_node(
    app: &mut VectorEditApp,
    id: &str,
    sub_i: usize,
    seg_i: usize,
    role: NodeRole,
    dx: f32,
    dy: f32,
) {
    let Some(doc) = app.doc.as_mut() else {
        return;
    };
    doc.mutate(id, |o| {
        let ObjKind::Path(path) = &mut o.kind else {
            return;
        };
        let Some(sub) = path.subpaths.get_mut(sub_i) else {
            return;
        };
        if sub.segs.get(seg_i).is_none() {
            return;
        }
        match role {
            NodeRole::Head | NodeRole::Anchor => {
                let at = if role == NodeRole::Head {
                    sub.segs.first().map(|s| s.start())
                } else {
                    sub.segs.get(seg_i).map(|s| s.end())
                };
                if let Some(at) = at {
                    shift_anchor(sub, at, dx, dy);
                }
            }
            NodeRole::In => {
                if let Some(Seg::Cubic { p1, .. }) = sub.segs.get_mut(seg_i) {
                    p1.0 += dx;
                    p1.1 += dy;
                }
            }
            NodeRole::Out => {
                if let Some(Seg::Cubic { p2, .. }) = sub.segs.get_mut(seg_i) {
                    p2.0 += dx;
                    p2.1 += dy;
                }
            }
        }
    });
}

fn shift_anchor(sub: &mut Subpath, at: (f32, f32), dx: f32, dy: f32) {
    let near = |p: (f32, f32)| (p.0 - at.0).hypot(p.1 - at.1) < 0.8;
    for seg in &mut sub.segs {
        match seg {
            Seg::Line { a, b } => {
                if near(*a) {
                    a.0 += dx;
                    a.1 += dy;
                }
                if near(*b) {
                    b.0 += dx;
                    b.1 += dy;
                }
            }
            Seg::Cubic { p0, p1, p2, p3 } => {
                let move0 = near(*p0);
                let move1 = near(*p1);
                let move2 = near(*p2);
                let move3 = near(*p3);
                if move0 {
                    p0.0 += dx;
                    p0.1 += dy;
                }
                if move3 {
                    p3.0 += dx;
                    p3.1 += dy;
                }
                if move1 {
                    p1.0 += dx;
                    p1.1 += dy;
                }
                if move2 {
                    p2.0 += dx;
                    p2.1 += dy;
                }
            }
        }
    }
}

pub(crate) fn toggle_anchor_smooth(
    app: &mut VectorEditApp,
    id: &str,
    sub_i: usize,
    seg_i: usize,
    role: NodeRole,
) {
    if !matches!(role, NodeRole::Head | NodeRole::Anchor) {
        return;
    }
    let Some(doc) = app.doc.as_mut() else {
        return;
    };
    doc.mutate(id, |o| {
        let ObjKind::Path(path) = &mut o.kind else {
            return;
        };
        let Some(sub) = path.subpaths.get_mut(sub_i) else {
            return;
        };
        let at = if role == NodeRole::Head {
            sub.segs.first().map(|s| s.start())
        } else {
            sub.segs.get(seg_i).map(|s| s.end())
        };
        let Some(at) = at else {
            return;
        };
        let near = |p: (f32, f32)| (p.0 - at.0).hypot(p.1 - at.1) < 0.8;
        let mut smooth = false;
        for seg in &sub.segs {
            if let Seg::Cubic { p0, p1, p2, p3 } = seg {
                if near(*p0) && !near(*p1) {
                    smooth = true;
                }
                if near(*p3) && !near(*p2) {
                    smooth = true;
                }
            }
        }
        if smooth {
            for seg in &mut sub.segs {
                if let Seg::Cubic { p0, p1, p2, p3 } = seg {
                    if near(*p0) {
                        *p1 = *p0;
                    }
                    if near(*p3) {
                        *p2 = *p3;
                    }
                }
            }
            return;
        }
        let mut prev = None;
        let mut next = None;
        for seg in sub.segs.iter() {
            if near(seg.end()) {
                prev = Some(seg.start());
            }
            if near(seg.start()) {
                next = Some(seg.end());
            }
        }
        let prev = prev.unwrap_or(at);
        let next = next.unwrap_or(at);
        let hin = (at.0 + (prev.0 - at.0) * 0.3, at.1 + (prev.1 - at.1) * 0.3);
        let hout = (at.0 + (next.0 - at.0) * 0.3, at.1 + (next.1 - at.1) * 0.3);
        for seg in &mut sub.segs {
            let touches = near(seg.start()) || near(seg.end());
            if touches {
                if let Seg::Line { a, b } = *seg {
                    *seg = Seg::Cubic {
                        p0: a,
                        p1: a,
                        p2: b,
                        p3: b,
                    };
                }
            }
            if let Seg::Cubic { p0, p1, p2, p3 } = seg {
                if near(*p0) {
                    *p1 = hout;
                }
                if near(*p3) {
                    *p2 = hin;
                }
            }
        }
    });
}

pub(crate) fn delete_anchor(app: &mut VectorEditApp, id: &str, sub_i: usize, seg_i: usize, role: NodeRole) {
    let mut empty = false;
    if let Some(doc) = app.doc.as_mut() {
    doc.mutate(id, |o| {
        let ObjKind::Path(path) = &mut o.kind else {
            return;
        };
        let Some(sub) = path.subpaths.get_mut(sub_i) else {
            return;
        };
        match role {
            NodeRole::Head => {
                if !sub.segs.is_empty() {
                    sub.segs.remove(0);
                }
            }
            NodeRole::Anchor if seg_i + 1 < sub.segs.len() => {
                let far = sub.segs[seg_i + 1].end();
                let far_in = match sub.segs[seg_i + 1] {
                    Seg::Cubic { p2, .. } => Some(p2),
                    Seg::Line { .. } => None,
                };
                sub.segs[seg_i].set_end(far);
                if let (Some(h), Seg::Cubic { p2, .. }) = (far_in, &mut sub.segs[seg_i]) {
                    *p2 = h;
                }
                sub.segs.remove(seg_i + 1);
            }
            NodeRole::Anchor => {
                if seg_i < sub.segs.len() {
                    sub.segs.remove(seg_i);
                }
            }
            NodeRole::In | NodeRole::Out => return,
        }
        if sub.segs.is_empty() {
            path.subpaths.remove(sub_i);
        }
        empty = path.subpaths.iter().all(|s| s.segs.is_empty());
    });
    }
    if empty {
        let owned = id.to_string();
        if let Some(doc) = app.doc.as_mut() {
            doc.delete_ids(std::slice::from_ref(&owned));
        }
        app.selected.retain(|s| s != id);
        app.node_target = None;
        app.node_hit = None;
    }
}

pub(crate) fn node_at(obj: &Obj, x: f32, y: f32, tol: f32) -> Option<(usize, usize, NodeRole)> {
    let ObjKind::Path(path) = &obj.kind else {
        return None;
    };
    let mut best: Option<(f32, usize, usize, NodeRole)> = None;
    let consider = |best: &mut Option<(f32, usize, usize, NodeRole)>, d: f32, s, g, role| {
        if d <= tol && best.map(|(bd, ..)| d < bd).unwrap_or(true) {
            *best = Some((d, s, g, role));
        }
    };
    for (si, sub) in path.subpaths.iter().enumerate() {
        for (gi, seg) in sub.segs.iter().enumerate() {
            let end = seg.end();
            consider(&mut best, (end.0 - x).hypot(end.1 - y), si, gi, NodeRole::Anchor);
            if gi == 0 {
                let st = seg.start();
                let closed_on_end = sub.closed
                    && sub.segs.last().is_some_and(|s| {
                        let e = s.end();
                        (e.0 - st.0).hypot(e.1 - st.1) < 0.5
                    });
                if !closed_on_end {
                    consider(
                        &mut best,
                        (st.0 - x).hypot(st.1 - y),
                        si,
                        gi,
                        NodeRole::Head,
                    );
                }
            }
            if let Seg::Cubic { p0, p1, p2, p3 } = seg {
                if (p1.0 - p0.0).hypot(p1.1 - p0.1) > tol * 0.35 {
                    consider(&mut best, (p1.0 - x).hypot(p1.1 - y), si, gi, NodeRole::In);
                }
                if (p2.0 - p3.0).hypot(p2.1 - p3.1) > tol * 0.35 {
                    consider(&mut best, (p2.0 - x).hypot(p2.1 - y), si, gi, NodeRole::Out);
                }
            }
        }
    }
    best.map(|(_, s, g, r)| (s, g, r))
}

pub(crate) fn resize_canvas(app: &mut VectorEditApp, edge: CanvasEdge, span: f32) {
    let Some(doc) = app.doc.as_mut() else {
        return;
    };
    let span = span.max(1.0);
    let gedge = match edge {
        CanvasEdge::Left => geom::Edge::Left,
        CanvasEdge::Right => geom::Edge::Right,
        CanvasEdge::Top => geom::Edge::Top,
        CanvasEdge::Bottom => geom::Edge::Bottom,
    };
    doc.resize_canvas(gedge, span);
}

fn styled_path(subs: Vec<Subpath>, style: &StyleBuf) -> Obj {
    Obj::path(PathObj {
        subpaths: subs,
        fill: style.fill.map(|rgba| Paint { rgba }),
        stroke: style.stroke.map(|rgba| Stroke {
            paint: Paint { rgba },
            width: style.width.max(0.15),
            cap: style.cap,
            join: style.join,
            dash: Vec::new(),
            dash_phase: 0.0,
        }),
        fill_rule: FillRule::Winding,
        clip: Vec::new(),
    })
}

fn closing_nodes(pen: &PenDraft, closed: bool) -> Vec<PenNode> {
    let mut nodes = pen.nodes.clone();
    if closed {
        if let Some(first) = pen.nodes.first() {
            let open = nodes
                .last()
                .is_none_or(|n| (n.p.0 - first.p.0).hypot(n.p.1 - first.p.1) > 0.4);
            if open {
                nodes.push(first.clone());
            }
        }
    }
    nodes
}

pub(crate) fn nodes_to_segs(nodes: &[PenNode]) -> Vec<Seg> {
    let mut segs = Vec::new();
    for w in nodes.windows(2) {
        let a = w[0].p;
        let b = w[1].p;
        let c1 = w[0].hout.unwrap_or(a);
        let c2 = w[1].hin.unwrap_or(b);
        if w[0].hout.is_none() && w[1].hin.is_none() {
            segs.push(Seg::Line { a, b });
        } else {
            segs.push(Seg::Cubic {
                p0: a,
                p1: c1,
                p2: c2,
                p3: b,
            });
        }
    }
    segs
}

fn apply_style(obj: &mut Obj, style: &StyleBuf) {
    match &mut obj.kind {
        ObjKind::Path(p) => {
            p.fill = style.fill.map(|rgba| Paint { rgba });
            p.stroke = style.stroke.map(|rgba| Stroke {
                paint: Paint { rgba },
                width: style.width.max(0.15),
                cap: style.cap,
                join: style.join,
                dash: p.stroke.as_ref().map(|s| s.dash.clone()).unwrap_or_default(),
                dash_phase: p.stroke.as_ref().map(|s| s.dash_phase).unwrap_or(0.0),
            });
        }
        ObjKind::Text(t) => {
            if let Some(rgba) = style.fill.or(style.stroke) {
                t.fill = Paint { rgba };
                t.edited = true;
                t.baked_id = None;
            }
        }
        ObjKind::Image(_) => {}
    }
}

pub(crate) fn selection_rect(app: &VectorEditApp) -> Option<Rect> {
    app.doc.as_ref()?.selection_bounds(&app.selected)
}

/// 选中的是组里的单个对象时, 包络仍用最外层组合的范围.
pub(crate) fn drilled_frame(app: &VectorEditApp) -> Option<Rect> {
    let doc = app.doc.as_ref()?;
    if app.selected.len() != 1 {
        return None;
    }
    let id = &app.selected[0];
    if doc.object(id).is_none() {
        return None;
    }
    let top = doc.pick_target(id);
    if top == *id {
        return None;
    }
    doc.selection_bounds(std::slice::from_ref(&top))
}

pub(crate) fn handle_points(b: Rect) -> [(f32, f32); 8] {
    let mx = b.cx();
    let my = b.cy();
    [
        (b.x0, b.y0),
        (mx, b.y0),
        (b.x1, b.y0),
        (b.x1, my),
        (b.x1, b.y1),
        (mx, b.y1),
        (b.x0, b.y1),
        (b.x0, my),
    ]
}
