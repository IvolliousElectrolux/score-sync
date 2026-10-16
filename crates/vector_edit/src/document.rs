//! 矢量场景: 画布 (PDF point, 左上为原点, y 向下) + 图层 + 对象.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::geom::{self, Rect};
use crate::ids::new_id;

pub const KIND_VECTOR: &str = "vector";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceFingerprint {
    #[serde(default)]
    pub page_id: String,
    #[serde(default)]
    pub y0: i32,
    #[serde(default)]
    pub y1: i32,
    #[serde(default)]
    pub w: u32,
    #[serde(default)]
    pub h: u32,
    #[serde(default)]
    pub extra_top: i32,
    #[serde(default)]
    pub extra_bottom: i32,
    #[serde(default)]
    pub extra_left: i32,
    #[serde(default)]
    pub extra_right: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    Winding,
    EvenOdd,
}

impl Default for FillRule {
    fn default() -> Self {
        Self::Winding
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

impl Default for LineCap {
    fn default() -> Self {
        Self::Butt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

impl Default for LineJoin {
    fn default() -> Self {
        Self::Miter
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paint {
    pub rgba: [u8; 4],
}

impl Paint {
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            rgba: [r, g, b, 255],
        }
    }

    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            rgba: [r, g, b, a],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f32,
    #[serde(default)]
    pub cap: LineCap,
    #[serde(default)]
    pub join: LineJoin,
    #[serde(default)]
    pub dash: Vec<f32>,
    #[serde(default)]
    pub dash_phase: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Seg {
    Line {
        a: (f32, f32),
        b: (f32, f32),
    },
    Cubic {
        p0: (f32, f32),
        p1: (f32, f32),
        p2: (f32, f32),
        p3: (f32, f32),
    },
}

impl Seg {
    pub fn start(&self) -> (f32, f32) {
        match *self {
            Seg::Line { a, .. } => a,
            Seg::Cubic { p0, .. } => p0,
        }
    }

    pub fn end(&self) -> (f32, f32) {
        match *self {
            Seg::Line { b, .. } => b,
            Seg::Cubic { p3, .. } => p3,
        }
    }

    pub fn set_start(&mut self, p: (f32, f32)) {
        match self {
            Seg::Line { a, .. } => *a = p,
            Seg::Cubic { p0, .. } => *p0 = p,
        }
    }

    pub fn set_end(&mut self, p: (f32, f32)) {
        match self {
            Seg::Line { b, .. } => *b = p,
            Seg::Cubic { p3, .. } => *p3 = p,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subpath {
    pub closed: bool,
    pub segs: Vec<Seg>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathObj {
    pub subpaths: Vec<Subpath>,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
    #[serde(default)]
    pub fill_rule: FillRule,
    /// 光栅时裁切, 第一版不单独编辑.
    #[serde(default)]
    pub clip: Vec<Subpath>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextObj {
    pub text: String,
    pub font_name: String,
    pub size: f32,
    pub fill: Paint,
    pub origin: (f32, f32),
    /// 未改字前用烘焙图; 改过之后改用系统字体.
    #[serde(default)]
    pub baked_id: Option<String>,
    #[serde(default)]
    pub edited: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageObj {
    pub png_id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ObjKind {
    Path(PathObj),
    Text(TextObj),
    Image(ImageObj),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Obj {
    pub id: String,
    /// 空字符串表示按类型自动起名.
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub kind: ObjKind,
}

/// 图层里的组合. 子级可以是对象, 也可以是嵌套的组, 顺序自下而上.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub children: Vec<String>,
}

impl Obj {
    pub fn path(path: PathObj) -> Self {
        Self::bare(new_id(), ObjKind::Path(path))
    }

    pub fn bare(id: impl Into<String>, kind: ObjKind) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            visible: true,
            kind,
        }
    }

    pub fn bounds(&self) -> Option<Rect> {
        geom::obj_bounds(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub visible: bool,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    pub objects: Vec<Obj>,
    #[serde(default)]
    pub groups: Vec<Group>,
}

fn default_opacity() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

impl Layer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            visible: true,
            opacity: 1.0,
            objects: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// 运行时对象用 Arc, 撤销只克隆向量.
#[derive(Clone, Debug)]
pub struct VecDocument {
    pub canvas_w: f32,
    pub canvas_h: f32,
    pub layers: Vec<LayerRuntime>,
    pub active: usize,
    pub source: Option<SourceFingerprint>,
    pub images: HashMap<String, Arc<RgbaImage>>,
}

#[derive(Clone, Debug)]
pub struct LayerRuntime {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub objects: Vec<Arc<Obj>>,
    pub groups: Vec<Group>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    kind: String,
    canvas_w: f32,
    canvas_h: f32,
    active: usize,
    #[serde(default)]
    source: Option<SourceFingerprint>,
    layers: Vec<Layer>,
}

impl VecDocument {
    pub fn empty(w: f32, h: f32) -> Self {
        let mut layer = LayerRuntime::from_layer(Layer::new("谱面"));
        layer.objects.push(Arc::new(Obj::path(PathObj {
            subpaths: Vec::new(),
            fill: None,
            stroke: None,
            fill_rule: FillRule::Winding,
            clip: Vec::new(),
        })));
        // 空图层不放占位路径.
        layer.objects.clear();
        Self {
            canvas_w: w.max(1.0),
            canvas_h: h.max(1.0),
            layers: vec![layer],
            active: 0,
            source: None,
            images: HashMap::new(),
        }
    }

    pub fn from_objects(w: f32, h: f32, objects: Vec<Obj>) -> Self {
        let mut doc = Self::empty(w, h);
        doc.layers[0].objects = objects.into_iter().map(Arc::new).collect();
        doc
    }

    pub fn rounded_size(&self) -> (u32, u32) {
        (
            self.canvas_w.round().max(1.0) as u32,
            self.canvas_h.round().max(1.0) as u32,
        )
    }

    pub fn active_layer(&self) -> Option<&LayerRuntime> {
        self.layers.get(self.active)
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut LayerRuntime> {
        self.layers.get_mut(self.active)
    }

    pub fn object(&self, id: &str) -> Option<&Obj> {
        for layer in &self.layers {
            for obj in &layer.objects {
                if obj.id == id {
                    return Some(obj);
                }
            }
        }
        None
    }

    pub fn mutate<F: FnOnce(&mut Obj)>(&mut self, id: &str, f: F) -> bool {
        for layer in &mut self.layers {
            if let Some(slot) = layer.objects.iter_mut().find(|o| o.id == id) {
                f(Arc::make_mut(slot));
                return true;
            }
        }
        false
    }

    pub fn find_loc(&self, id: &str) -> Option<(usize, usize)> {
        for (li, layer) in self.layers.iter().enumerate() {
            if let Some(oi) = layer.objects.iter().position(|o| o.id == id) {
                return Some((li, oi));
            }
        }
        None
    }

    pub fn find_group(&self, id: &str) -> Option<(usize, usize)> {
        for (li, layer) in self.layers.iter().enumerate() {
            if let Some(gi) = layer.groups.iter().position(|g| g.id == id) {
                return Some((li, gi));
            }
        }
        None
    }

    /// 直接父组. 图层根上的对象没有父组.
    pub fn parent_of(&self, id: &str) -> Option<(usize, String)> {
        for (li, layer) in self.layers.iter().enumerate() {
            for group in &layer.groups {
                if group.children.iter().any(|c| c == id) {
                    return Some((li, group.id.clone()));
                }
            }
        }
        None
    }

    /// 把选中的组展开成叶子对象. 已经是对象的 id 原样留下.
    pub fn leaf_ids(&self, ids: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for id in ids {
            self.collect_leaves(id, &mut out, &mut seen, 0);
        }
        out
    }

    fn collect_leaves(&self, id: &str, out: &mut Vec<String>, seen: &mut HashSet<String>, depth: u8) {
        if depth > 32 || !seen.insert(id.to_string()) {
            return;
        }
        if let Some((li, gi)) = self.find_group(id) {
            let children = self.layers[li].groups[gi].children.clone();
            for child in children {
                self.collect_leaves(&child, out, seen, depth + 1);
            }
            return;
        }
        if self.object(id).is_some() {
            out.push(id.to_string());
        }
    }

    /// 普通点选. 框选和套索仍走 `pick_target`.
    /// 组还没选中时选最外层的组. 组已经选中, 或这组里已经有一个对象被选中, 再点就选这个对象.
    pub fn plain_click_target(&self, selected: &[String], leaf: &str) -> String {
        let top = self.pick_target(leaf);
        if top == leaf || selected.len() != 1 {
            return top;
        }
        let cur = &selected[0];
        if cur == &top || (self.object(cur).is_some() && self.pick_target(cur) == top) {
            return leaf.to_string();
        }
        top
    }

    /// 画布点选时, 点到组里的对象就选中最外层的组.
    pub fn pick_target(&self, leaf: &str) -> String {
        let mut id = leaf.to_string();
        let mut top = leaf.to_string();
        let mut guard = 0;
        while let Some((_, parent)) = self.parent_of(&id) {
            guard += 1;
            if guard > 32 {
                break;
            }
            top = parent.clone();
            id = parent;
        }
        top
    }

    /// 这个叶子或它的某个上级已经在选区里时, 返回那个选区 id.
    pub fn selection_anchor(&self, selected: &[String], leaf: &str) -> Option<String> {
        if selected.iter().any(|s| s == leaf) {
            return Some(leaf.to_string());
        }
        let mut id = leaf.to_string();
        let mut found = None;
        let mut guard = 0;
        while let Some((_, parent)) = self.parent_of(&id) {
            guard += 1;
            if guard > 32 {
                break;
            }
            if selected.iter().any(|s| s == &parent) {
                found = Some(parent.clone());
            }
            id = parent;
        }
        found
    }

    /// 自上而下命中: 后绘制的在上面. 隐藏的组和对象跳过.
    pub fn hit_test(&self, x: f32, y: f32, tol: f32) -> Option<String> {
        for layer in self.layers.iter().rev() {
            if !layer.visible {
                continue;
            }
            let skip = layer.suppressed_ids();
            for obj in layer.objects.iter().rev() {
                if skip.contains(&obj.id) {
                    continue;
                }
                if geom::hit_obj(obj, x, y, tol) {
                    return Some(obj.id.clone());
                }
            }
        }
        None
    }

    pub fn hits(&self, x: f32, y: f32, tol: f32) -> bool {
        self.hit_test(x, y, tol).is_some()
    }

    pub fn objects_in_rect(&self, rect: Rect) -> Vec<String> {
        let mut ids = Vec::new();
        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            let skip = layer.suppressed_ids();
            for obj in &layer.objects {
                if skip.contains(&obj.id) {
                    continue;
                }
                if geom::obj_bounds(obj).is_some_and(|b| rect.contains_rect(b)) {
                    ids.push(obj.id.clone());
                }
            }
        }
        ids
    }

    pub fn objects_in_poly(&self, poly: &[(f32, f32)]) -> Vec<String> {
        let mut ids = Vec::new();
        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            let skip = layer.suppressed_ids();
            for obj in &layer.objects {
                if skip.contains(&obj.id) {
                    continue;
                }
                if let Some(b) = geom::obj_bounds(obj) {
                    if geom::rect_inside_poly(b, poly) {
                        ids.push(obj.id.clone());
                    }
                }
            }
        }
        ids
    }

    pub fn translate_ids(&mut self, ids: &[String], dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let leaves = self.leaf_ids(ids);
        for id in leaves {
            self.mutate(&id, |o| geom::translate_obj(o, dx, dy));
        }
    }

    pub fn scale_ids(&mut self, ids: &[String], cx: f32, cy: f32, sx: f32, sy: f32) {
        let leaves = self.leaf_ids(ids);
        for id in leaves {
            self.mutate(&id, |o| geom::scale_obj(o, cx, cy, sx, sy));
        }
    }

    pub fn rotate_ids(&mut self, ids: &[String], cx: f32, cy: f32, deg: f32) {
        if deg.abs() < 0.01 {
            return;
        }
        let leaves = self.leaf_ids(ids);
        for id in leaves {
            self.mutate(&id, |o| geom::rotate_obj(o, cx, cy, deg));
        }
    }

    pub fn delete_ids(&mut self, ids: &[String]) {
        let leaves = self.leaf_ids(ids);
        let drop_objs: HashSet<&str> = leaves.iter().map(|s| s.as_str()).collect();
        let mut drop_groups = HashSet::new();
        for id in ids {
            if let Some((li, _)) = self.find_group(id) {
                collect_group_tree(&self.layers[li], id, &mut drop_groups);
            }
        }
        for layer in &mut self.layers {
            layer.objects.retain(|o| !drop_objs.contains(o.id.as_str()));
            layer.groups.retain(|g| !drop_groups.contains(&g.id));
            for group in &mut layer.groups {
                group.children.retain(|c| !drop_objs.contains(c.as_str()) && !drop_groups.contains(c));
            }
            crate::outline::sanitize_layer(layer);
        }
    }

    pub fn duplicate_ids(&mut self, ids: &[String], dx: f32, dy: f32) -> Vec<String> {
        crate::outline::duplicate_items(self, ids, dx, dy)
    }

    pub fn content_bounds(&self) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            for obj in &layer.objects {
                let Some(b) = obj.bounds() else {
                    continue;
                };
                acc = Some(match acc {
                    None => b,
                    Some(a) => a.union(b),
                });
            }
        }
        acc
    }

    pub fn selection_bounds(&self, ids: &[String]) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        for id in self.leaf_ids(ids) {
            if let Some(b) = self.object(&id).and_then(|o| o.bounds()) {
                acc = Some(match acc {
                    None => b,
                    Some(a) => a.union(b),
                });
            }
        }
        acc
    }

    pub fn extract_to_new_layer(&mut self, ids: &[String]) -> bool {
        if ids.is_empty() || self.layers.is_empty() {
            return false;
        }
        let leaves = self.leaf_ids(ids);
        if leaves.is_empty() {
            return false;
        }
        let leaf_set: HashSet<&str> = leaves.iter().map(|s| s.as_str()).collect();
        let mut move_groups = HashSet::new();
        for id in ids {
            if let Some((li, _)) = self.find_group(id) {
                collect_group_tree(&self.layers[li], id, &mut move_groups);
            }
        }
        let mut moved_objs = Vec::new();
        let mut moved_groups = Vec::new();
        for layer in &mut self.layers {
            let mut keep = Vec::new();
            for obj in layer.objects.drain(..) {
                if leaf_set.contains(obj.id.as_str()) {
                    moved_objs.push(obj);
                } else {
                    keep.push(obj);
                }
            }
            layer.objects = keep;
            let mut keep_groups = Vec::new();
            for group in layer.groups.drain(..) {
                if move_groups.contains(&group.id) {
                    moved_groups.push(group);
                } else {
                    keep_groups.push(group);
                }
            }
            layer.groups = keep_groups;
            crate::outline::sanitize_layer(layer);
        }
        if moved_objs.is_empty() {
            return false;
        }
        let mut layer = LayerRuntime::from_layer(Layer::new("取出"));
        layer.objects = moved_objs;
        layer.groups = moved_groups;
        crate::outline::sanitize_layer(&mut layer);
        self.layers.push(layer);
        self.active = self.layers.len() - 1;
        true
    }

    pub fn delete_active_layer(&mut self) -> bool {
        if self.layers.len() <= 1 {
            return false;
        }
        let idx = self.active.min(self.layers.len() - 1);
        self.layers.remove(idx);
        self.active = idx.saturating_sub(1).min(self.layers.len() - 1);
        true
    }

    pub fn duplicate_active_layer(&mut self) {
        let Some(src) = self.layers.get(self.active).cloned() else {
            return;
        };
        let mut copy = src;
        copy.id = new_id();
        copy.name = format!("{} 副本", copy.name);
        copy.reassign_ids();
        self.layers.insert(self.active + 1, copy);
        self.active += 1;
    }

    pub fn move_active_layer(&mut self, dir: i32) {
        let n = self.layers.len();
        if n < 2 {
            return;
        }
        let next = self.active as i32 + dir;
        if next < 0 || next >= n as i32 {
            return;
        }
        self.layers.swap(self.active, next as usize);
        self.active = next as usize;
    }

    pub fn append_layer_from(&mut self, other: &VecDocument, name: &str) {
        let mut layer = LayerRuntime::from_layer(Layer::new(name));
        for src in &other.layers {
            if !src.visible {
                continue;
            }
            let mut piece = src.clone();
            piece.reassign_ids();
            for obj in &piece.objects {
                absorb_images(&mut self.images, &other.images, obj);
            }
            layer.objects.extend(piece.objects);
            layer.groups.extend(piece.groups);
        }
        self.layers.push(layer);
        self.active = self.layers.len() - 1;
    }

    pub fn resize_canvas(&mut self, edge: geom::Edge, new_span: f32) {
        let new_span = new_span.max(1.0);
        match edge {
            geom::Edge::Left => {
                let dx = self.canvas_w - new_span;
                self.canvas_w = new_span;
                for layer in &mut self.layers {
                    for obj in &mut layer.objects {
                        geom::translate_obj(Arc::make_mut(obj), -dx, 0.0);
                    }
                }
            }
            geom::Edge::Right => self.canvas_w = new_span,
            geom::Edge::Top => {
                let dy = self.canvas_h - new_span;
                self.canvas_h = new_span;
                for layer in &mut self.layers {
                    for obj in &mut layer.objects {
                        geom::translate_obj(Arc::make_mut(obj), 0.0, -dy);
                    }
                }
            }
            geom::Edge::Bottom => self.canvas_h = new_span,
        }
    }

    pub fn save_to_dir(&self, dir: &Path) -> Result<(), String> {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| format!("清掉旧修图失败: {e}"))?;
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("创建修图目录失败: {e}"))?;
        let img_dir = dir.join("images");
        if !self.images.is_empty() {
            std::fs::create_dir_all(&img_dir).map_err(|e| format!("创建 images 失败: {e}"))?;
            for (id, img) in &self.images {
                let path = img_dir.join(format!("{id}.png"));
                img.save(&path)
                    .map_err(|e| format!("写图像 {id} 失败: {e}"))?;
            }
        }
        let manifest = Manifest {
            kind: KIND_VECTOR.into(),
            canvas_w: self.canvas_w,
            canvas_h: self.canvas_h,
            active: self.active,
            source: self.source.clone(),
            layers: self.layers.iter().map(LayerRuntime::to_layer).collect(),
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| format!("序列化场景失败: {e}"))?;
        std::fs::write(dir.join("manifest.json"), json).map_err(|e| format!("写 manifest 失败: {e}"))?;
        if let Ok(preview) = crate::raster::rasterize_rgba(self, 1.5) {
            let _ = preview.save(dir.join("preview.png"));
        }
        Ok(())
    }

    pub fn load_from_dir(dir: &Path) -> Result<Self, String> {
        let raw = std::fs::read(dir.join("manifest.json")).map_err(|e| format!("读 manifest 失败: {e}"))?;
        let manifest: Manifest =
            serde_json::from_slice(&raw).map_err(|e| format!("解析场景失败: {e}"))?;
        if manifest.kind != KIND_VECTOR {
            return Err("不是矢量修图".into());
        }
        let mut images = HashMap::new();
        let img_dir = dir.join("images");
        if img_dir.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&img_dir) {
                for ent in rd.flatten() {
                    let path = ent.path();
                    let id = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if id.is_empty() {
                        continue;
                    }
                    if let Ok(img) = image::open(&path) {
                        images.insert(id, Arc::new(img.to_rgba8()));
                    }
                }
            }
        }
        let layers = manifest
            .layers
            .into_iter()
            .map(LayerRuntime::from_layer)
            .collect::<Vec<_>>();
        let active = manifest.active.min(layers.len().saturating_sub(1));
        Ok(Self {
            canvas_w: manifest.canvas_w.max(1.0),
            canvas_h: manifest.canvas_h.max(1.0),
            layers,
            active,
            source: manifest.source,
            images,
        })
    }
}

impl LayerRuntime {
    fn from_layer(layer: Layer) -> Self {
        Self {
            id: layer.id,
            name: layer.name,
            visible: layer.visible,
            opacity: layer.opacity.clamp(0.0, 1.0),
            objects: layer.objects.into_iter().map(Arc::new).collect(),
            groups: layer.groups,
        }
    }

    fn to_layer(&self) -> Layer {
        Layer {
            id: self.id.clone(),
            name: self.name.clone(),
            visible: self.visible,
            opacity: self.opacity,
            objects: self.objects.iter().map(|o| (**o).clone()).collect(),
            groups: self.groups.clone(),
        }
    }

    /// 对象和组都换成新 id, 组里的引用一起改.
    pub(crate) fn reassign_ids(&mut self) {
        let mut map: HashMap<String, String> = HashMap::new();
        for slot in &mut self.objects {
            let mut obj = (**slot).clone();
            let nid = new_id();
            map.insert(obj.id.clone(), nid.clone());
            obj.id = nid;
            *slot = Arc::new(obj);
        }
        for group in &self.groups {
            map.insert(group.id.clone(), new_id());
        }
        for group in &mut self.groups {
            if let Some(nid) = map.get(&group.id).cloned() {
                group.id = nid;
            }
            for child in &mut group.children {
                if let Some(nid) = map.get(child) {
                    *child = nid.clone();
                }
            }
        }
    }

    /// 不参与绘制和命中的对象: 自己隐藏, 或某个上级组隐藏.
    pub fn suppressed_ids(&self) -> HashSet<String> {
        let mut parent: HashMap<&str, &str> = HashMap::new();
        for group in &self.groups {
            for child in &group.children {
                parent.insert(child.as_str(), group.id.as_str());
            }
        }
        let mut hidden_group = HashSet::new();
        for group in &self.groups {
            let mut cur = group.id.as_str();
            let mut hidden = false;
            let mut guard = 0;
            loop {
                guard += 1;
                if guard > 64 {
                    break;
                }
                let vis = self
                    .groups
                    .iter()
                    .find(|g| g.id == cur)
                    .map(|g| g.visible)
                    .unwrap_or(true);
                if !vis {
                    hidden = true;
                    break;
                }
                match parent.get(cur) {
                    Some(p) => cur = p,
                    None => break,
                }
            }
            if hidden {
                hidden_group.insert(group.id.as_str());
            }
        }
        let mut out = HashSet::new();
        for obj in &self.objects {
            if !obj.visible {
                out.insert(obj.id.clone());
                continue;
            }
            if parent.get(obj.id.as_str()).is_some_and(|p| hidden_group.contains(p)) {
                out.insert(obj.id.clone());
            }
        }
        out
    }
}

fn absorb_images(
    dst: &mut HashMap<String, Arc<RgbaImage>>,
    src: &HashMap<String, Arc<RgbaImage>>,
    obj: &Obj,
) {
    match &obj.kind {
        ObjKind::Image(img) => {
            if let Some(bytes) = src.get(&img.png_id) {
                dst.insert(img.png_id.clone(), bytes.clone());
            }
        }
        ObjKind::Text(text) => {
            if let Some(id) = &text.baked_id {
                if let Some(bytes) = src.get(id) {
                    dst.insert(id.clone(), bytes.clone());
                }
            }
        }
        ObjKind::Path(_) => {}
    }
}

fn collect_group_tree(layer: &LayerRuntime, id: &str, out: &mut HashSet<String>) {
    if !out.insert(id.to_string()) {
        return;
    }
    let Some(group) = layer.groups.iter().find(|g| g.id == id) else {
        return;
    };
    for child in &group.children {
        if layer.groups.iter().any(|g| g.id == *child) {
            collect_group_tree(layer, child, out);
        }
    }
}

/// 目录里 manifest 的 kind. 没有 kind 的是位图修图.
pub fn sniff_edit_kind(dir: &Path) -> Option<&'static str> {
    let raw = std::fs::read(dir.join("manifest.json")).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    match v.get("kind").and_then(|k| k.as_str()) {
        Some(KIND_VECTOR) => Some(KIND_VECTOR),
        _ => None,
    }
}

pub fn is_scene_dir(path: &Path) -> bool {
    path.is_dir() && sniff_edit_kind(path) == Some(KIND_VECTOR)
}

pub fn scene_dir_or_none(path: &PathBuf) -> bool {
    is_scene_dir(path)
}

/// 画布底色跟多数可见对象的墨色反着来: 浅色笔画用深底, 深色笔画用浅底.
pub fn contrast_backdrop(doc: &VecDocument) -> u32 {
    let mut light = 0u32;
    let mut dark = 0u32;
    for layer in &doc.layers {
        if !layer.visible || layer.opacity < 0.05 {
            continue;
        }
        let skip = layer.suppressed_ids();
        for obj in &layer.objects {
            if skip.contains(&obj.id) {
                continue;
            }
            let Some(rgba) = obj_ink(obj) else {
                continue;
            };
            if rgba[3] < 16 {
                continue;
            }
            let y = 0.2126 * rgba[0] as f32 + 0.7152 * rgba[1] as f32 + 0.0722 * rgba[2] as f32;
            if y >= 0.55 * 255.0 {
                light += 1;
            } else {
                dark += 1;
            }
        }
    }
    if light > dark {
        0x242428
    } else {
        0xe7e7e7
    }
}

pub fn obj_ink(obj: &Obj) -> Option<[u8; 4]> {
    match &obj.kind {
        ObjKind::Path(p) => p
            .stroke
            .as_ref()
            .map(|s| s.paint.rgba)
            .filter(|c| c[3] >= 16)
            .or_else(|| p.fill.map(|f| f.rgba)),
        ObjKind::Text(t) => Some(t.fill.rgba),
        ObjKind::Image(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_path_and_image() {
        let mut doc = VecDocument::empty(120.0, 40.0);
        doc.layers[0].objects.push(Arc::new(Obj::path(PathObj {
            subpaths: vec![Subpath {
                closed: true,
                segs: vec![
                    Seg::Line {
                        a: (1.0, 1.0),
                        b: (20.0, 1.0),
                    },
                    Seg::Line {
                        a: (20.0, 1.0),
                        b: (20.0, 10.0),
                    },
                    Seg::Line {
                        a: (20.0, 10.0),
                        b: (1.0, 10.0),
                    },
                    Seg::Line {
                        a: (1.0, 10.0),
                        b: (1.0, 1.0),
                    },
                ],
            }],
            fill: Some(Paint::rgb(0, 0, 0)),
            stroke: Some(Stroke {
                paint: Paint::rgb(10, 10, 10),
                width: 0.4,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
                dash: Vec::new(),
                dash_phase: 0.0,
            }),
            fill_rule: FillRule::EvenOdd,
            clip: Vec::new(),
        })));
        let mut img = RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        doc.images.insert("im1".into(), Arc::new(img));
        doc.layers[0].objects.push(Arc::new(Obj::bare(
            "imgobj",
            ObjKind::Image(ImageObj {
                png_id: "im1".into(),
                x: 4.0,
                y: 4.0,
                w: 8.0,
                h: 8.0,
            }),
        )));
        let dir = std::env::temp_dir().join(format!("vector_edit_rt_{}", new_id()));
        doc.save_to_dir(&dir).unwrap();
        let loaded = VecDocument::load_from_dir(&dir).unwrap();
        assert!((loaded.canvas_w - 120.0).abs() < 0.01);
        assert_eq!(loaded.layers[0].objects.len(), 2);
        assert!(loaded.images.contains_key("im1"));
        assert_eq!(sniff_edit_kind(&dir), Some(KIND_VECTOR));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_sharing_copies_on_write() {
        let mut doc = VecDocument::from_objects(
            10.0,
            10.0,
            vec![Obj::path(PathObj {
                subpaths: vec![Subpath {
                    closed: false,
                    segs: vec![Seg::Line {
                        a: (0.0, 0.0),
                        b: (1.0, 0.0),
                    }],
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
            })],
        );
        let snap = doc.clone();
        let id = doc.layers[0].objects[0].id.clone();
        doc.translate_ids(&[id], 3.0, 0.0);
        let moved = doc.layers[0].objects[0].bounds().unwrap().x0;
        let old = snap.layers[0].objects[0].bounds().unwrap().x0;
        assert!((moved - old - 3.0).abs() < 0.01);
        assert!(old.abs() < 0.6);
        assert!(!std::sync::Arc::ptr_eq(
            &doc.layers[0].objects[0],
            &snap.layers[0].objects[0]
        ));
    }

    fn ink_path(rgb: [u8; 3]) -> Obj {
        Obj::path(PathObj {
            subpaths: vec![Subpath {
                closed: false,
                segs: vec![Seg::Line {
                    a: (0.0, 0.0),
                    b: (1.0, 0.0),
                }],
            }],
            fill: None,
            stroke: Some(Stroke {
                paint: Paint::rgb(rgb[0], rgb[1], rgb[2]),
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
    fn backdrop_follows_majority_ink() {
        let light = VecDocument::from_objects(
            10.0,
            10.0,
            vec![ink_path([250, 250, 250]), ink_path([255, 255, 255]), ink_path([0, 0, 0])],
        );
        assert_eq!(contrast_backdrop(&light), 0x242428);
        let dark = VecDocument::from_objects(
            10.0,
            10.0,
            vec![ink_path([0, 0, 0]), ink_path([12, 12, 12]), ink_path([255, 255, 255])],
        );
        assert_eq!(contrast_backdrop(&dark), 0xe7e7e7);
        assert_eq!(contrast_backdrop(&VecDocument::empty(10.0, 10.0)), 0xe7e7e7);
    }

    #[test]
    fn legacy_manifest_without_names_or_groups() {
        let raw = r#"{
            "kind": "vector",
            "canvas_w": 10.0,
            "canvas_h": 8.0,
            "active": 0,
            "layers": [{
                "id": "L",
                "name": "谱面",
                "visible": true,
                "opacity": 1.0,
                "objects": [{
                    "id": "o1",
                    "kind": {"Path": {
                        "subpaths": [{"closed": false, "segs": [{"Line": {"a": [0.0, 0.0], "b": [4.0, 0.0]}}]}],
                        "fill": null,
                        "stroke": {"paint": {"rgba": [0, 0, 0, 255]}, "width": 1.0}
                    }}
                }]
            }]
        }"#;
        let manifest: Manifest = serde_json::from_str(raw).unwrap();
        assert!(manifest.layers[0].groups.is_empty());
        assert!(manifest.layers[0].objects[0].name.is_empty());
        assert!(manifest.layers[0].objects[0].visible);
        let layer = LayerRuntime::from_layer(manifest.layers.into_iter().next().unwrap());
        assert_eq!(layer.objects.len(), 1);
        assert!(layer.groups.is_empty());
    }
}
