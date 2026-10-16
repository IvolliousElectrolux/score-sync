//! 包围盒, 命中, 变换, 条带裁切.

use crate::document::{Obj, ObjKind, Seg, Subpath};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }

    pub fn from_points(pts: impl IntoIterator<Item = (f32, f32)>) -> Option<Self> {
        let mut it = pts.into_iter();
        let (x, y) = it.next()?;
        let mut r = Self {
            x0: x,
            y0: y,
            x1: x,
            y1: y,
        };
        for (x, y) in it {
            r.include(x, y);
        }
        Some(r)
    }

    pub fn include(&mut self, x: f32, y: f32) {
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
    }

    pub fn pad(self, p: f32) -> Self {
        Self {
            x0: self.x0 - p,
            y0: self.y0 - p,
            x1: self.x1 + p,
            y1: self.y1 + p,
        }
    }

    pub fn intersects(self, o: Self) -> bool {
        self.x0 <= o.x1 && self.x1 >= o.x0 && self.y0 <= o.y1 && self.y1 >= o.y0
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn contains_rect(self, o: Self) -> bool {
        o.x0 >= self.x0 && o.x1 <= self.x1 && o.y0 >= self.y0 && o.y1 <= self.y1
    }

    pub fn union(self, o: Self) -> Self {
        Self {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    pub fn cx(self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }

    pub fn cy(self) -> f32 {
        (self.y0 + self.y1) * 0.5
    }

    pub fn w(self) -> f32 {
        self.x1 - self.x0
    }

    pub fn h(self) -> f32 {
        self.y1 - self.y0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

pub fn obj_bounds(obj: &Obj) -> Option<Rect> {
    match &obj.kind {
        ObjKind::Path(p) => {
            let r = subpaths_bounds(&p.subpaths)?;
            let pad = p.stroke.as_ref().map(|s| s.width.max(0.0) * 0.5).unwrap_or(0.0);
            Some(r.pad(pad))
        }
        ObjKind::Text(t) => {
            let w = (t.text.chars().count().max(1) as f32) * t.size * 0.55;
            let h = t.size.max(1.0);
            Some(Rect::new(t.origin.0, t.origin.1 - h, t.origin.0 + w, t.origin.1))
        }
        ObjKind::Image(im) => Some(Rect::new(im.x, im.y, im.x + im.w, im.y + im.h)),
    }
}

fn subpaths_bounds(subs: &[Subpath]) -> Option<Rect> {
    let mut pts = Vec::new();
    for sub in subs {
        for seg in &sub.segs {
            match seg {
                Seg::Line { a, b } => {
                    pts.push(*a);
                    pts.push(*b);
                }
                Seg::Cubic { p0, p1, p2, p3 } => {
                    pts.push(*p0);
                    pts.push(*p1);
                    pts.push(*p2);
                    pts.push(*p3);
                }
            }
        }
    }
    Rect::from_points(pts)
}

pub fn hit_obj(obj: &Obj, x: f32, y: f32, tol: f32) -> bool {
    let Some(b) = obj_bounds(obj) else {
        return false;
    };
    if !b.pad(tol).contains(x, y) {
        return false;
    }
    match &obj.kind {
        ObjKind::Image(_) | ObjKind::Text(_) => b.pad(tol).contains(x, y),
        ObjKind::Path(p) => {
            if p.fill.is_some() && point_in_subpaths(x, y, &p.subpaths, p.fill_rule) {
                return true;
            }
            let reach = p
                .stroke
                .as_ref()
                .map(|s| s.width.max(0.4) * 0.5)
                .unwrap_or(0.0)
                .max(tol);
            dist_to_subpaths(x, y, &p.subpaths) <= reach
        }
    }
}

/// 矩形四角都在多边形里, 并且没有边切进矩形, 才算完全包住.
pub fn rect_inside_poly(rect: Rect, poly: &[(f32, f32)]) -> bool {
    if poly.len() < 3 || rect.w() <= 0.0 || rect.h() <= 0.0 {
        return false;
    }
    let corners = [
        (rect.x0, rect.y0),
        (rect.x1, rect.y0),
        (rect.x1, rect.y1),
        (rect.x0, rect.y1),
    ];
    if !corners.iter().all(|(x, y)| point_in_poly(*x, *y, poly)) {
        return false;
    }
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if strict_inside(a, rect) || strict_inside(b, rect) || segment_crosses_rect(a, b, rect) {
            return false;
        }
    }
    true
}

fn strict_inside(p: (f32, f32), rect: Rect) -> bool {
    p.0 > rect.x0 && p.0 < rect.x1 && p.1 > rect.y0 && p.1 < rect.y1
}

fn segment_crosses_rect(a: (f32, f32), b: (f32, f32), rect: Rect) -> bool {
    let edges = [
        ((rect.x0, rect.y0), (rect.x1, rect.y0)),
        ((rect.x1, rect.y0), (rect.x1, rect.y1)),
        ((rect.x1, rect.y1), (rect.x0, rect.y1)),
        ((rect.x0, rect.y1), (rect.x0, rect.y0)),
    ];
    edges.iter().any(|(c, d)| segments_cross(a, b, *c, *d))
}

fn segments_cross(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> bool {
    fn side(p: (f32, f32), q: (f32, f32), r: (f32, f32)) -> f32 {
        (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
    }
    side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0
}

pub fn point_in_poly(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi + 1e-12) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn point_in_subpaths(x: f32, y: f32, subs: &[Subpath], rule: crate::document::FillRule) -> bool {
    let mut wind = 0i32;
    let mut crosses = 0i32;
    for sub in subs {
        let flat = flatten_sub(sub, 0.6);
        if flat.len() < 3 {
            continue;
        }
        let mut j = flat.len() - 1;
        for i in 0..flat.len() {
            let (xi, yi) = flat[i];
            let (xj, yj) = flat[j];
            if (yi > y) != (yj > y) {
                let xint = (xj - xi) * (y - yi) / (yj - yi + 1e-12) + xi;
                if x < xint {
                    crosses += 1;
                    wind += if yj > yi { 1 } else { -1 };
                }
            }
            j = i;
        }
    }
    match rule {
        crate::document::FillRule::EvenOdd => crosses % 2 == 1,
        crate::document::FillRule::Winding => wind != 0,
    }
}

fn dist_to_subpaths(x: f32, y: f32, subs: &[Subpath]) -> f32 {
    let mut best = f32::MAX;
    for sub in subs {
        let flat = flatten_sub(sub, 0.5);
        for w in flat.windows(2) {
            best = best.min(dist_to_seg(x, y, w[0], w[1]));
        }
    }
    best
}

fn dist_to_seg(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-8 {
        return (px - a.0).hypot(py - a.1);
    }
    let t = ((px - a.0) * dx + (py - a.1) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    (px - (a.0 + dx * t)).hypot(py - (a.1 + dy * t))
}

pub fn flatten_sub(sub: &Subpath, tol: f32) -> Vec<(f32, f32)> {
    let mut pts = Vec::new();
    for seg in &sub.segs {
        match *seg {
            Seg::Line { a, b } => {
                if pts.last().copied() != Some(a) {
                    pts.push(a);
                }
                pts.push(b);
            }
            Seg::Cubic { p0, p1, p2, p3 } => {
                if pts.last().copied() != Some(p0) {
                    pts.push(p0);
                }
                flatten_cubic(p0, p1, p2, p3, tol, &mut pts, 0);
            }
        }
    }
    if sub.closed {
        if let (Some(first), Some(last)) = (pts.first().copied(), pts.last().copied()) {
            if (first.0 - last.0).abs() > 0.05 || (first.1 - last.1).abs() > 0.05 {
                pts.push(first);
            }
        }
    }
    pts
}

fn flatten_cubic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    tol: f32,
    out: &mut Vec<(f32, f32)>,
    depth: u8,
) {
    if depth > 8 || cubic_flat(p0, p1, p2, p3, tol) {
        out.push(p3);
        return;
    }
    let (a, b) = split_cubic(p0, p1, p2, p3, 0.5);
    flatten_cubic(a[0], a[1], a[2], a[3], tol, out, depth + 1);
    flatten_cubic(b[0], b[1], b[2], b[3], tol, out, depth + 1);
}

fn cubic_flat(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), tol: f32) -> bool {
    let d1 = dist_to_seg(p1.0, p1.1, p0, p3);
    let d2 = dist_to_seg(p2.0, p2.1, p0, p3);
    d1 <= tol && d2 <= tol
}

pub fn split_cubic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> ([(f32, f32); 4], [(f32, f32); 4]) {
    let lerp = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let a = lerp(p0, p1, t);
    let b = lerp(p1, p2, t);
    let c = lerp(p2, p3, t);
    let d = lerp(a, b, t);
    let e = lerp(b, c, t);
    let f = lerp(d, e, t);
    ([p0, a, d, f], [f, e, c, p3])
}

pub fn translate_obj(obj: &mut Obj, dx: f32, dy: f32) {
    map_obj_points(obj, |x, y| (x + dx, y + dy), 1.0);
}

pub fn scale_obj(obj: &mut Obj, cx: f32, cy: f32, sx: f32, sy: f32) {
    let s = (sx.abs() + sy.abs()) * 0.5;
    map_obj_points(
        obj,
        |x, y| (cx + (x - cx) * sx, cy + (y - cy) * sy),
        s.max(0.01),
    );
}

pub fn rotate_obj(obj: &mut Obj, cx: f32, cy: f32, deg: f32) {
    let r = deg.to_radians();
    let (sn, cs) = r.sin_cos();
    map_obj_points(
        obj,
        |x, y| {
            let dx = x - cx;
            let dy = y - cy;
            (cx + dx * cs - dy * sn, cy + dx * sn + dy * cs)
        },
        1.0,
    );
}

fn map_obj_points(obj: &mut Obj, mut f: impl FnMut(f32, f32) -> (f32, f32), stroke_scale: f32) {
    match &mut obj.kind {
        ObjKind::Path(p) => {
            for sub in &mut p.subpaths {
                map_sub(sub, &mut f);
            }
            for sub in &mut p.clip {
                map_sub(sub, &mut f);
            }
            if let Some(st) = p.stroke.as_mut() {
                if (stroke_scale - 1.0).abs() > 0.001 {
                    st.width = (st.width * stroke_scale).max(0.05);
                    for d in &mut st.dash {
                        *d *= stroke_scale;
                    }
                }
            }
        }
        ObjKind::Text(t) => {
            t.origin = f(t.origin.0, t.origin.1);
            if (stroke_scale - 1.0).abs() > 0.001 {
                t.size = (t.size * stroke_scale).max(0.5);
            }
        }
        ObjKind::Image(im) => {
            let (x0, y0) = f(im.x, im.y);
            let (x1, y1) = f(im.x + im.w, im.y + im.h);
            im.x = x0.min(x1);
            im.y = y0.min(y1);
            im.w = (x1 - x0).abs().max(0.5);
            im.h = (y1 - y0).abs().max(0.5);
        }
    }
}

fn map_sub(sub: &mut Subpath, f: &mut impl FnMut(f32, f32) -> (f32, f32)) {
    for seg in &mut sub.segs {
        match seg {
            Seg::Line { a, b } => {
                *a = f(a.0, a.1);
                *b = f(b.0, b.1);
            }
            Seg::Cubic { p0, p1, p2, p3 } => {
                *p0 = f(p0.0, p0.1);
                *p1 = f(p1.0, p1.1);
                *p2 = f(p2.0, p2.1);
                *p3 = f(p3.0, p3.1);
            }
        }
    }
}

/// 与矩形相交的路径留下; 完全在外的丢掉; 跨边的折线裁切.
pub fn clip_subpaths(subs: &[Subpath], rect: Rect) -> Vec<Subpath> {
    let mut out = Vec::new();
    for sub in subs {
        let Some(b) = subpaths_bounds(std::slice::from_ref(sub)) else {
            continue;
        };
        if !b.intersects(rect) {
            continue;
        }
        if rect.contains_rect(b) {
            out.push(sub.clone());
            continue;
        }
        let flat = flatten_sub(sub, 0.35);
        if flat.len() < 2 {
            continue;
        }
        if sub.closed {
            let clipped = sutherland_hodgman(&flat, rect);
            if clipped.len() >= 3 {
                out.push(polyline_sub(&clipped, true));
            }
        } else if let Some(parts) = clip_open_polyline(&flat, rect) {
            out.extend(parts);
        }
    }
    out
}

fn polyline_sub(pts: &[(f32, f32)], closed: bool) -> Subpath {
    let mut segs = Vec::new();
    for w in pts.windows(2) {
        segs.push(Seg::Line { a: w[0], b: w[1] });
    }
    Subpath { closed, segs }
}

fn clip_open_polyline(pts: &[(f32, f32)], rect: Rect) -> Option<Vec<Subpath>> {
    let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur = Vec::new();
    for w in pts.windows(2) {
        if let Some((a, b)) = clip_line(w[0], w[1], rect) {
            if cur.last().copied() != Some(a) {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
                cur.push(a);
            }
            cur.push(b);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if cur.len() >= 2 {
        runs.push(cur);
    }
    let subs: Vec<_> = runs
        .into_iter()
        .filter(|r| r.len() >= 2)
        .map(|r| polyline_sub(&r, false))
        .collect();
    if subs.is_empty() {
        None
    } else {
        Some(subs)
    }
}

fn clip_line(a: (f32, f32), b: (f32, f32), r: Rect) -> Option<((f32, f32), (f32, f32))> {
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let inside = |p: f32, q: f32, t0: &mut f32, t1: &mut f32| -> bool {
        if p.abs() < 1e-8 {
            return q >= 0.0;
        }
        let t = q / p;
        if p < 0.0 {
            if t > *t1 {
                return false;
            }
            if t > *t0 {
                *t0 = t;
            }
        } else {
            if t < *t0 {
                return false;
            }
            if t < *t1 {
                *t1 = t;
            }
        }
        true
    };
    if !inside(-dx, a.0 - r.x0, &mut t0, &mut t1)
        || !inside(dx, r.x1 - a.0, &mut t0, &mut t1)
        || !inside(-dy, a.1 - r.y0, &mut t0, &mut t1)
        || !inside(dy, r.y1 - a.1, &mut t0, &mut t1)
    {
        return None;
    }
    Some((
        (a.0 + dx * t0, a.1 + dy * t0),
        (a.0 + dx * t1, a.1 + dy * t1),
    ))
}

fn sutherland_hodgman(poly: &[(f32, f32)], rect: Rect) -> Vec<(f32, f32)> {
    let mut output = poly.to_vec();
    if output.len() >= 2 {
        let a = output[0];
        let b = *output.last().unwrap();
        if (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05 {
            output.pop();
        }
    }
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        if output.is_empty() {
            break;
        }
        let input = std::mem::take(&mut output);
        let mut prev = *input.last().unwrap();
        for &cur in &input {
            let pin = inside_edge(prev, rect, edge);
            let cin = inside_edge(cur, rect, edge);
            if cin {
                if !pin {
                    if let Some(p) = intersect_edge(prev, cur, rect, edge) {
                        output.push(p);
                    }
                }
                output.push(cur);
            } else if pin {
                if let Some(p) = intersect_edge(prev, cur, rect, edge) {
                    output.push(p);
                }
            }
            prev = cur;
        }
    }
    output
}

fn inside_edge(p: (f32, f32), r: Rect, edge: Edge) -> bool {
    match edge {
        Edge::Left => p.0 >= r.x0,
        Edge::Right => p.0 <= r.x1,
        Edge::Top => p.1 >= r.y0,
        Edge::Bottom => p.1 <= r.y1,
    }
}

fn intersect_edge(a: (f32, f32), b: (f32, f32), r: Rect, edge: Edge) -> Option<(f32, f32)> {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let t = match edge {
        Edge::Left => {
            if dx.abs() < 1e-8 {
                return None;
            }
            (r.x0 - a.0) / dx
        }
        Edge::Right => {
            if dx.abs() < 1e-8 {
                return None;
            }
            (r.x1 - a.0) / dx
        }
        Edge::Top => {
            if dy.abs() < 1e-8 {
                return None;
            }
            (r.y0 - a.1) / dy
        }
        Edge::Bottom => {
            if dy.abs() < 1e-8 {
                return None;
            }
            (r.y1 - a.1) / dy
        }
    };
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some((a.0 + dx * t, a.1 + dy * t))
}

/// 在路径上离点最近的位置, 用于剪刀.
pub fn closest_on_path(subs: &[Subpath], x: f32, y: f32) -> Option<(usize, usize, f32)> {
    let mut best: Option<(f32, usize, usize, f32)> = None;
    for (si, sub) in subs.iter().enumerate() {
        for (gi, seg) in sub.segs.iter().enumerate() {
            let (d, t) = match *seg {
                Seg::Line { a, b } => {
                    let dx = b.0 - a.0;
                    let dy = b.1 - a.1;
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 < 1e-8 {
                        0.0
                    } else {
                        (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0)
                    };
                    let px = a.0 + dx * t;
                    let py = a.1 + dy * t;
                    ((x - px).hypot(y - py), t)
                }
                Seg::Cubic { p0, p1, p2, p3 } => {
                    let mut bd = f32::MAX;
                    let mut bt = 0.0;
                    for i in 0..=16 {
                        let t = i as f32 / 16.0;
                        let p = cubic_at(p0, p1, p2, p3, t);
                        let d = (x - p.0).hypot(y - p.1);
                        if d < bd {
                            bd = d;
                            bt = t;
                        }
                    }
                    (bd, bt)
                }
            };
            if best.map(|(bd, ..)| d < bd).unwrap_or(true) {
                best = Some((d, si, gi, t));
            }
        }
    }
    best.map(|(_, si, gi, t)| (si, gi, t))
}

pub fn cubic_at(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let b0 = u * u * u;
    let b1 = 3.0 * u * u * t;
    let b2 = 3.0 * u * t * t;
    let b3 = t * t * t;
    (
        b0 * p0.0 + b1 * p1.0 + b2 * p2.0 + b3 * p3.0,
        b0 * p0.1 + b1 * p1.1 + b2 * p2.1 + b3 * p3.1,
    )
}

/// Catmull-Rom 折线收成三次贝塞尔.
pub fn fit_cubics(pts: &[(f32, f32)]) -> Vec<Seg> {
    if pts.len() < 2 {
        return Vec::new();
    }
    let simp = rdp(pts, 0.8);
    if simp.len() < 2 {
        return Vec::new();
    }
    if simp.len() == 2 {
        return vec![Seg::Line {
            a: simp[0],
            b: simp[1],
        }];
    }
    let mut segs = Vec::new();
    for i in 0..simp.len() - 1 {
        let p0 = if i == 0 { simp[0] } else { simp[i - 1] };
        let p1 = simp[i];
        let p2 = simp[i + 1];
        let p3 = if i + 2 < simp.len() {
            simp[i + 2]
        } else {
            simp[i + 1]
        };
        let c1 = (
            p1.0 + (p2.0 - p0.0) / 6.0,
            p1.1 + (p2.1 - p0.1) / 6.0,
        );
        let c2 = (
            p2.0 - (p3.0 - p1.0) / 6.0,
            p2.1 - (p3.1 - p1.1) / 6.0,
        );
        segs.push(Seg::Cubic {
            p0: p1,
            p1: c1,
            p2: c2,
            p3: p2,
        });
    }
    segs
}

fn rdp(pts: &[(f32, f32)], eps: f32) -> Vec<(f32, f32)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut max_d = 0.0;
    let mut idx = 0;
    let a = pts[0];
    let b = *pts.last().unwrap();
    for (i, p) in pts.iter().enumerate().skip(1).take(pts.len() - 2) {
        let d = dist_to_seg(p.0, p.1, a, b);
        if d > max_d {
            max_d = d;
            idx = i;
        }
    }
    if max_d > eps {
        let mut left = rdp(&pts[..=idx], eps);
        let right = rdp(&pts[idx..], eps);
        left.pop();
        left.extend(right);
        left
    } else {
        vec![a, b]
    }
}

/// 用笔画把路径擦出缺口. 没擦到返回 None, 擦到则返回新的子路径 (被擦断的曲线改成折线).
pub fn carve_subpaths(subs: &[Subpath], stroke: &[(f32, f32)], radius: f32) -> Option<Vec<Subpath>> {
    if stroke.is_empty() || radius <= 0.0 {
        return None;
    }
    let mut changed = false;
    let mut out = Vec::new();
    for sub in subs {
        if !subpath_hits_stroke(sub, stroke, radius) {
            out.push(sub.clone());
            continue;
        }
        changed = true;
        let flat = densify(&flatten_sub(sub, 0.45), 0.8);
        if flat.len() < 2 {
            continue;
        }
        let mut run = Vec::new();
        for w in flat.windows(2) {
            if segment_hits_stroke(w[0], w[1], stroke, radius) {
                if run.len() >= 2 {
                    out.push(polyline_sub(&run, false));
                }
                run.clear();
            } else {
                if run.last().copied() != Some(w[0]) {
                    run.push(w[0]);
                }
                run.push(w[1]);
            }
        }
        if run.len() >= 2 {
            out.push(polyline_sub(&run, false));
        }
    }
    if changed {
        Some(out)
    } else {
        None
    }
}

fn densify(pts: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let Some(&first) = pts.first() else {
        return out;
    };
    out.push(first);
    let step = step.max(0.2);
    for w in pts.windows(2) {
        let len = (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
        let n = (len / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            out.push((
                w[0].0 + (w[1].0 - w[0].0) * t,
                w[0].1 + (w[1].1 - w[0].1) * t,
            ));
        }
    }
    out
}

fn subpath_hits_stroke(sub: &Subpath, stroke: &[(f32, f32)], radius: f32) -> bool {
    let flat = flatten_sub(sub, 0.6);
    flat.windows(2)
        .any(|w| segment_hits_stroke(w[0], w[1], stroke, radius))
}

fn segment_hits_stroke(a: (f32, f32), b: (f32, f32), stroke: &[(f32, f32)], radius: f32) -> bool {
    let len = (b.0 - a.0).hypot(b.1 - a.1);
    let n = ((len / 0.75).ceil() as usize).clamp(1, 48);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        if dist_to_stroke(p, stroke) <= radius {
            return true;
        }
    }
    false
}

fn dist_to_stroke(p: (f32, f32), stroke: &[(f32, f32)]) -> f32 {
    if stroke.len() == 1 {
        return (p.0 - stroke[0].0).hypot(p.1 - stroke[0].1);
    }
    let mut best = f32::MAX;
    for w in stroke.windows(2) {
        best = best.min(dist_to_seg(p.0, p.1, w[0], w[1]));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Obj, PathObj, Seg, Stroke, Paint, FillRule, LineCap, LineJoin, Subpath};

    fn line_obj(a: (f32, f32), b: (f32, f32)) -> Obj {
        Obj::path(PathObj {
            subpaths: vec![Subpath {
                closed: false,
                segs: vec![Seg::Line { a, b }],
            }],
            fill: None,
            stroke: Some(Stroke {
                paint: Paint::rgb(0, 0, 0),
                width: 1.0,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
                dash: Vec::new(),
                dash_phase: 0.0,
            }),
            fill_rule: FillRule::Winding,
            clip: Vec::new(),
        })
    }

    #[test]
    fn clip_drops_outside_and_cuts_crossing_line() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let outside = Subpath {
            closed: false,
            segs: vec![Seg::Line {
                a: (20.0, 20.0),
                b: (30.0, 20.0),
            }],
        };
        assert!(clip_subpaths(&[outside], rect).is_empty());
        let cross = Subpath {
            closed: false,
            segs: vec![Seg::Line {
                a: (-5.0, 5.0),
                b: (15.0, 5.0),
            }],
        };
        let clipped = clip_subpaths(&[cross], rect);
        assert_eq!(clipped.len(), 1);
        let Seg::Line { a, b } = clipped[0].segs[0] else {
            panic!("line");
        };
        assert!((a.0 - 0.0).abs() < 0.2);
        assert!((b.0 - 10.0).abs() < 0.2);
    }

    #[test]
    fn carve_splits_long_line() {
        let sub = Subpath {
            closed: false,
            segs: vec![Seg::Line {
                a: (0.0, 0.0),
                b: (100.0, 0.0),
            }],
        };
        let parts = carve_subpaths(&[sub], &[(50.0, 0.0)], 4.0).unwrap();
        assert_eq!(parts.len(), 2);
        let Seg::Line { b, .. } = parts[0].segs.last().unwrap() else {
            panic!("line");
        };
        assert!(b.0 < 48.0, "left end {}", b.0);
        let Seg::Line { a, .. } = parts[1].segs[0] else {
            panic!("line");
        };
        assert!(a.0 > 52.0, "right start {}", a.0);
    }

    #[test]
    fn hit_stroke_not_far_away() {
        let o = line_obj((0.0, 0.0), (10.0, 0.0));
        assert!(hit_obj(&o, 5.0, 0.2, 1.0));
        assert!(!hit_obj(&o, 5.0, 8.0, 1.0));
    }
}
