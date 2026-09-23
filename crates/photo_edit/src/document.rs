//! 图层文档: 画布 + RGBA 图层 + 拼合 / 磁盘读写.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{Rgb, RgbImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::geom::{rotate_rgba, rotated_aabb};
use crate::ids::new_id;
use crate::process::stamp_color;

/// 一笔涂抹. 身份是这条有序点列和它的笔属性, 不是像素是否连在一起.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaintStroke {
    pub points: Vec<(i32, i32)>,
    pub radius: i32,
    pub hardness: f32,
    pub color: [u8; 3],
}

impl PaintStroke {
    pub fn overlaps(&self, x: i32, y: i32, eraser_r: i32) -> bool {
        let pr = self.radius.max(1);
        let er = eraser_r.max(0);
        self.points
            .iter()
            .any(|&(px, py)| disks_touch(px, py, pr, x, y, er))
    }

    /// 点列的轴对齐包围盒 (含端点). 空笔为 None.
    pub fn point_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut it = self.points.iter().copied();
        let (mut x0, mut y0) = it.next()?;
        let (mut x1, mut y1) = (x0, y0);
        for (x, y) in it {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Some((x0, y0, x1, y1))
    }

    /// 橡皮盖住的盖章点从序列里拿掉.
    /// 只在拿掉的点把序列从中间断开时拆成多笔; 擦到头尾仍是同一笔.
    /// 点与点之间离得远, 只要中间没被擦掉, 就还是一笔.
    pub fn split_erased(&self, x: i32, y: i32, eraser_r: i32) -> Option<Vec<PaintStroke>> {
        let hits_at = [(x, y)];
        let hits = EraserHits::new(&hits_at, eraser_r);
        self.split_erased_hits(&hits).map(|(parts, _)| parts)
    }

    /// 一次走完所有橡皮采样. 返回拆开的笔, 以及被拿掉的盖章点.
    /// 拖擦时不要对每个采样点各拆一次, 否则笔被从中间切断后会反复拷贝整条点列.
    pub(crate) fn split_erased_hits(
        &self,
        hits: &EraserHits<'_>,
    ) -> Option<(Vec<PaintStroke>, Vec<(i32, i32)>)> {
        if self.points.is_empty() || hits.is_empty() {
            return None;
        }
        let pr = self.radius.max(1);
        if !hits.may_hit(self.point_bounds(), pr) {
            return None;
        }
        let mut runs: Vec<Vec<(i32, i32)>> = Vec::new();
        let mut cur = Vec::new();
        let mut removed = Vec::new();
        for &(px, py) in &self.points {
            if hits.covers(px, py, pr) {
                removed.push((px, py));
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push((px, py));
            }
        }
        if removed.is_empty() {
            return None;
        }
        if !cur.is_empty() {
            runs.push(cur);
        }
        let parts = runs
            .into_iter()
            .map(|points| PaintStroke {
                points,
                radius: self.radius,
                hardness: self.hardness,
                color: self.color,
            })
            .collect();
        Some((parts, removed))
    }
}

/// 一次拖擦上的橡皮采样, 供多笔共用, 避免每笔重建.
pub(crate) struct EraserHits<'a> {
    centers: &'a [(i32, i32)],
    er: i32,
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    cell: i32,
    cols: i32,
    rows: i32,
    bins: Vec<Vec<u32>>,
}

impl<'a> EraserHits<'a> {
    pub(crate) fn new(centers: &'a [(i32, i32)], eraser_r: i32) -> Self {
        let er = eraser_r.max(0);
        let Some(&(fx, fy)) = centers.first() else {
            return Self {
                centers,
                er,
                min_x: 0,
                min_y: 0,
                max_x: 0,
                max_y: 0,
                cell: 64,
                cols: 0,
                rows: 0,
                bins: Vec::new(),
            };
        };
        let mut min_x = fx;
        let mut min_y = fy;
        let mut max_x = fx;
        let mut max_y = fy;
        for &(x, y) in centers.iter().skip(1) {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        let span = i64::from((max_x - min_x).max(max_y - min_y).max(0));
        let cell = 64i32.max((span / 48).min(1_000_000) as i32).max(1);
        let cols = (max_x - min_x) / cell + 1;
        let rows = (max_y - min_y) / cell + 1;
        let mut bins = Vec::new();
        if centers.len() > 12 {
            bins = vec![Vec::new(); (cols as usize) * (rows as usize)];
            for (i, &(x, y)) in centers.iter().enumerate() {
                let cx = (x - min_x) / cell;
                let cy = (y - min_y) / cell;
                bins[(cy * cols + cx) as usize].push(i as u32);
            }
        }
        Self {
            centers,
            er,
            min_x,
            min_y,
            max_x,
            max_y,
            cell,
            cols,
            rows,
            bins,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.centers.is_empty()
    }

    pub(crate) fn hits_stroke(&self, stroke: &PaintStroke) -> bool {
        let pr = stroke.radius.max(1);
        if !self.may_hit(stroke.point_bounds(), pr) {
            return false;
        }
        stroke.points.iter().any(|&(x, y)| self.covers(x, y, pr))
    }

    fn may_hit(&self, bounds: Option<(i32, i32, i32, i32)>, pr: i32) -> bool {
        let Some((x0, y0, x1, y1)) = bounds else {
            return false;
        };
        let reach = i64::from(pr.max(0).saturating_add(self.er));
        i64::from(x1) + reach >= i64::from(self.min_x)
            && i64::from(y1) + reach >= i64::from(self.min_y)
            && i64::from(x0) - reach <= i64::from(self.max_x)
            && i64::from(y0) - reach <= i64::from(self.max_y)
    }

    pub(crate) fn covers(&self, px: i32, py: i32, pr: i32) -> bool {
        let reach_i = pr.max(0).saturating_add(self.er);
        let reach = i64::from(reach_i);
        if i64::from(px) + reach < i64::from(self.min_x)
            || i64::from(py) + reach < i64::from(self.min_y)
            || i64::from(px) - reach > i64::from(self.max_x)
            || i64::from(py) - reach > i64::from(self.max_y)
        {
            return false;
        }
        let lim = reach * reach;
        if self.bins.is_empty() {
            return self.centers.iter().any(|&(x, y)| {
                let dx = i64::from(px - x);
                let dy = i64::from(py - y);
                dx * dx + dy * dy <= lim
            });
        }
        let cell = self.cell.max(1);
        let cx0 = (i64::from(px) - reach - i64::from(self.min_x)).div_euclid(i64::from(cell));
        let cy0 = (i64::from(py) - reach - i64::from(self.min_y)).div_euclid(i64::from(cell));
        let cx1 = (i64::from(px) + reach - i64::from(self.min_x)).div_euclid(i64::from(cell));
        let cy1 = (i64::from(py) + reach - i64::from(self.min_y)).div_euclid(i64::from(cell));
        for cy in cy0..=cy1 {
            if cy < 0 || cy >= i64::from(self.rows) {
                continue;
            }
            for cx in cx0..=cx1 {
                if cx < 0 || cx >= i64::from(self.cols) {
                    continue;
                }
                let bin = &self.bins[(cy as i32 * self.cols + cx as i32) as usize];
                for &i in bin {
                    let (x, y) = self.centers[i as usize];
                    let dx = i64::from(px - x);
                    let dy = i64::from(py - y);
                    if dx * dx + dy * dy <= lim {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn stamp_strokes_into(dst: &mut RgbaImage, strokes: &[PaintStroke], origin_x: i32, origin_y: i32) {
    let x1 = origin_x + dst.width() as i32;
    let y1 = origin_y + dst.height() as i32;
    for stroke in strokes {
        let radius = stroke.radius.max(1);
        let margin = radius + 2;
        let margin_i = i64::from(margin);
        if let Some((bx0, by0, bx1, by1)) = stroke.point_bounds() {
            if i64::from(bx1) + margin_i < i64::from(origin_x)
                || i64::from(by1) + margin_i < i64::from(origin_y)
                || i64::from(bx0) - margin_i >= i64::from(x1)
                || i64::from(by0) - margin_i >= i64::from(y1)
            {
                continue;
            }
        }
        for &(px, py) in &stroke.points {
            if px + margin < origin_x
                || py + margin < origin_y
                || px - margin >= x1
                || py - margin >= y1
            {
                continue;
            }
            stamp_color(
                dst,
                px - origin_x,
                py - origin_y,
                radius,
                stroke.hardness,
                stroke.color,
            );
        }
    }
}

fn disks_touch(ax: i32, ay: i32, ar: i32, bx: i32, by: i32, br: i32) -> bool {
    let dx = i64::from(ax - bx);
    let dy = i64::from(ay - by);
    let r = i64::from(ar.max(0)) + i64::from(br.max(0));
    dx * dx + dy * dy <= r * r
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub opacity: f32,
    /// 相对像素缓冲的顺时针角度; 绘制时 GPU 转, 不改像素.
    pub rotation: f32,
    pub pixels: Arc<RgbaImage>,
    /// 画笔痕迹, 叠在 `pixels` 上, 不写进底图像素.
    pub strokes: Vec<PaintStroke>,
    /// 痕迹合成图, 透明底. 没有笔迹时为 None.
    pub ink: Option<Arc<RgbaImage>>,
    /// 笔迹或墨水变了就加一, 用来判断贴图要不要重传.
    pub stroke_rev: u64,
}

impl Layer {
    pub fn new(name: impl Into<String>, pixels: RgbaImage) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            x: 0,
            y: 0,
            visible: true,
            opacity: 1.0,
            rotation: 0.0,
            pixels: Arc::new(pixels),
            strokes: Vec::new(),
            ink: None,
            stroke_rev: 0,
        }
    }

    pub fn bake_strokes(&mut self) {
        let Some(ink) = self.ink.take() else {
            self.strokes.clear();
            return;
        };
        if self.strokes.is_empty() {
            return;
        }
        blit_ink_over(Arc::make_mut(&mut self.pixels), &ink);
        self.strokes.clear();
        self.stroke_rev = self.stroke_rev.wrapping_add(1);
    }

    pub fn rebuild_ink_full(&mut self) {
        self.stroke_rev = self.stroke_rev.wrapping_add(1);
        if self.strokes.is_empty() {
            self.ink = None;
            return;
        }
        let mut ink = RgbaImage::new(self.width(), self.height());
        stamp_strokes_into(&mut ink, &self.strokes, 0, 0);
        self.ink = Some(Arc::new(ink));
    }

    /// 只重画矩形里的墨水, 用新合成替换旧像素, 避免在旧墨水上再叠一层.
    pub fn rebuild_ink_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if self.strokes.is_empty() {
            self.ink = None;
            self.stroke_rev = self.stroke_rev.wrapping_add(1);
            return;
        }
        let w = self.width();
        let h = self.height();
        let fits = self
            .ink
            .as_ref()
            .is_some_and(|im| im.width() == w && im.height() == h);
        if !fits {
            self.rebuild_ink_full();
            return;
        }
        let x0c = x0.clamp(0, w as i32) as u32;
        let y0c = y0.clamp(0, h as i32) as u32;
        let x1c = x1.clamp(0, w as i32) as u32;
        let y1c = y1.clamp(0, h as i32) as u32;
        if x0c >= x1c || y0c >= y1c {
            self.stroke_rev = self.stroke_rev.wrapping_add(1);
            return;
        }
        let tw = x1c - x0c;
        let th = y1c - y0c;
        let mut temp = RgbaImage::new(tw, th);
        stamp_strokes_into(&mut temp, &self.strokes, x0c as i32, y0c as i32);
        let ink_w = self.width() as usize;
        let temp_raw = temp.into_raw();
        let ink = Arc::make_mut(self.ink.as_mut().unwrap());
        let ink_raw: &mut [u8] = ink.as_mut();
        let tw = tw as usize;
        let x0c = x0c as usize;
        let y0c = y0c as usize;
        for row in 0..th as usize {
            let n = tw * 4;
            let s0 = row * n;
            let d0 = ((y0c + row) * ink_w + x0c) * 4;
            ink_raw[d0..d0 + n].copy_from_slice(&temp_raw[s0..s0 + n]);
        }
        self.stroke_rev = self.stroke_rev.wrapping_add(1);
    }

    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    pub fn center(&self) -> (f32, f32) {
        (
            self.x as f32 + self.width() as f32 * 0.5,
            self.y as f32 + self.height() as f32 * 0.5,
        )
    }

    /// 像素矩形在 `extra_deg` 叠加后的轴对齐包围盒 (min_x, min_y, max_x, max_y).
    #[allow(dead_code)]
    pub fn aabb(&self, extra_deg: f32) -> (f32, f32, f32, f32) {
        let deg = self.rotation + extra_deg;
        let w = self.width() as f32;
        let h = self.height() as f32;
        if deg.abs() < 0.05 {
            let x = self.x as f32;
            let y = self.y as f32;
            return (x, y, x + w, y + h);
        }
        rotated_aabb(self.x as f32, self.y as f32, w, h, deg)
    }
}

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

#[derive(Clone, Debug)]
pub struct EditDocument {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub paper_rgb: [u8; 3],
    pub layers: Vec<Layer>,
    pub active: usize,
    pub source: Option<SourceFingerprint>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    canvas_w: u32,
    canvas_h: u32,
    paper_rgb: [u8; 3],
    active: usize,
    #[serde(default)]
    source: Option<SourceFingerprint>,
    layers: Vec<ManifestLayer>,
}

#[derive(Serialize, Deserialize)]
struct ManifestLayer {
    id: String,
    name: String,
    x: i32,
    y: i32,
    visible: bool,
    opacity: f32,
    #[serde(default)]
    rotation: f32,
    #[serde(default)]
    strokes: Vec<PaintStroke>,
    png: String,
}

impl EditDocument {
    pub fn from_rgb(img: &RgbImage, paper: [u8; 3]) -> Self {
        let (w, h) = img.dimensions();
        let layer = Layer::new("背景", rgb_to_rgba(img));
        Self {
            canvas_w: w.max(1),
            canvas_h: h.max(1),
            paper_rgb: paper,
            layers: vec![layer],
            active: 0,
            source: None,
        }
    }

    pub fn active_layer(&self) -> Option<&Layer> {
        self.layers.get(self.active)
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut Layer> {
        self.layers.get_mut(self.active)
    }

    pub fn set_active(&mut self, idx: usize) {
        if !self.layers.is_empty() {
            self.active = idx.min(self.layers.len() - 1);
        }
    }

    pub fn flatten_rgb(&self) -> RgbImage {
        self.flatten_rgb_except(None)
    }

    /// 拼合时跳过指定图层 (拖动该层时底下只画一次).
    pub fn flatten_rgb_except(&self, skip: Option<usize>) -> RgbImage {
        let w = self.canvas_w.max(1);
        let h = self.canvas_h.max(1);
        let mut out = RgbImage::from_pixel(w, h, Rgb(self.paper_rgb));
        for (i, layer) in self.layers.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            if !layer.visible || layer.opacity <= 0.001 {
                continue;
            }
            if layer.rotation.abs() < 0.05 {
                blit_rgba(&mut out, &layer.pixels, layer.x, layer.y, layer.opacity);
                if let Some(ink) = &layer.ink {
                    blit_rgba(&mut out, ink, layer.x, layer.y, layer.opacity);
                }
            } else {
                let img = rotate_rgba(&layer.pixels, layer.rotation);
                let (cx, cy) = layer.center();
                let nx = (cx - img.width() as f32 * 0.5).round() as i32;
                let ny = (cy - img.height() as f32 * 0.5).round() as i32;
                blit_rgba(&mut out, &img, nx, ny, layer.opacity);
                if let Some(ink) = &layer.ink {
                    let ink = rotate_rgba(ink, layer.rotation);
                    blit_rgba(&mut out, &ink, nx, ny, layer.opacity);
                }
            }
        }
        out
    }

    pub fn resize_canvas(&mut self, new_w: u32, new_h: u32, origin_x: i32, origin_y: i32) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        for layer in &mut self.layers {
            layer.x -= origin_x;
            layer.y -= origin_y;
        }
        self.canvas_w = new_w;
        self.canvas_h = new_h;
    }

    /// 从拖开始时的图层坐标绝对套一次, 避免每帧叠位移.
    pub fn resize_canvas_from(
        &mut self,
        new_w: u32,
        new_h: u32,
        orig_pos: &[(i32, i32)],
        origin_x: i32,
        origin_y: i32,
    ) {
        self.canvas_w = new_w.max(1);
        self.canvas_h = new_h.max(1);
        for (layer, &(x, y)) in self.layers.iter_mut().zip(orig_pos.iter()) {
            layer.x = x - origin_x;
            layer.y = y - origin_y;
        }
    }

    pub fn crop_to(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let x0 = x0.min(x1);
        let y0 = y0.min(y1);
        let x1 = x0.max(x1);
        let y1 = y0.max(y1);
        let w = (x1 - x0 + 1).max(1) as u32;
        let h = (y1 - y0 + 1).max(1) as u32;
        self.resize_canvas(w, h, x0, y0);
    }

    pub fn save_to_dir(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建编辑目录失败: {e}"))?;
        let layer_dir = dir.join("layers");
        std::fs::create_dir_all(&layer_dir).map_err(|e| format!("创建图层目录失败: {e}"))?;
        let mut layers = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            let rel = format!("layers/{}.png", layer.id);
            let path = dir.join(&rel);
            layer
                .pixels
                .save(&path)
                .map_err(|e| format!("写图层 {} 失败: {e}", layer.id))?;
            layers.push(ManifestLayer {
                id: layer.id.clone(),
                name: layer.name.clone(),
                x: layer.x,
                y: layer.y,
                visible: layer.visible,
                opacity: layer.opacity,
                rotation: layer.rotation,
                strokes: layer.strokes.clone(),
                png: rel,
            });
        }
        let manifest = Manifest {
            canvas_w: self.canvas_w,
            canvas_h: self.canvas_h,
            paper_rgb: self.paper_rgb,
            active: self.active,
            source: self.source.clone(),
            layers,
        };
        let json =
            serde_json::to_vec_pretty(&manifest).map_err(|e| format!("序列化清单失败: {e}"))?;
        std::fs::write(dir.join("manifest.json"), json)
            .map_err(|e| format!("写 manifest 失败: {e}"))?;
        let flat = self.flatten_rgb();
        flat.save(dir.join("flat.png"))
            .map_err(|e| format!("写 flat.png 失败: {e}"))?;
        Ok(())
    }

    pub fn load_from_dir(dir: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(dir.join("manifest.json"))
            .map_err(|e| format!("读 manifest 失败: {e}"))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| format!("解析 manifest 失败: {e}"))?;
        let mut layers = Vec::with_capacity(manifest.layers.len());
        for m in manifest.layers {
            let path = dir.join(&m.png);
            let img = image::open(&path)
                .map_err(|e| format!("读图层 {} 失败: {e}", m.id))?
                .to_rgba8();
            let mut layer = Layer {
                id: m.id,
                name: m.name,
                x: m.x,
                y: m.y,
                visible: m.visible,
                opacity: m.opacity.clamp(0.0, 1.0),
                rotation: m.rotation,
                pixels: Arc::new(img),
                strokes: m.strokes,
                ink: None,
                stroke_rev: 0,
            };
            if !layer.strokes.is_empty() {
                layer.rebuild_ink_full();
            }
            layers.push(layer);
        }
        if layers.is_empty() {
            return Err("清单里没有图层".into());
        }
        let active = manifest.active.min(layers.len() - 1);
        Ok(Self {
            canvas_w: manifest.canvas_w.max(1),
            canvas_h: manifest.canvas_h.max(1),
            paper_rgb: manifest.paper_rgb,
            layers,
            active,
            source: manifest.source,
        })
    }

    pub fn flat_path(dir: &Path) -> PathBuf {
        dir.join("flat.png")
    }
}

pub fn rgb_to_rgba(img: &RgbImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels() {
        out.put_pixel(x, y, Rgba([p[0], p[1], p[2], 255]));
    }
    out
}

pub fn blit_rgba(dst: &mut RgbImage, src: &RgbaImage, dx: i32, dy: i32, opacity: f32) {
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let x0 = dx.max(0);
    let y0 = dy.max(0);
    let x1 = (dx + sw).min(dw);
    let y1 = (dy + sh).min(dh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let op = opacity.clamp(0.0, 1.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let sx = (x - dx) as u32;
            let sy = (y - dy) as u32;
            let s = src.get_pixel(sx, sy);
            let a = (s[3] as f32 / 255.0) * op;
            if a <= 0.001 {
                continue;
            }
            let d = dst.get_pixel_mut(x as u32, y as u32);
            for c in 0..3 {
                d[c] = ((1.0 - a) * d[c] as f32 + a * s[c] as f32).round() as u8;
            }
        }
    }
}

pub fn blit_ink_over(base: &mut RgbaImage, ink: &RgbaImage) {
    let w = base.width().min(ink.width());
    let h = base.height().min(ink.height());
    for y in 0..h {
        for x in 0..w {
            let s = *ink.get_pixel(x, y);
            if s[3] == 0 {
                continue;
            }
            let d = base.get_pixel_mut(x, y);
            *d = src_over(*d, s);
        }
    }
}

fn src_over(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
    let sa = src[3] as f32 / 255.0;
    if sa >= 0.999 {
        return src;
    }
    let inv = 1.0 - sa;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * inv;
    if out_a <= 1e-6 {
        return Rgba([0, 0, 0, 0]);
    }
    Rgba([
        ((src[0] as f32 * sa + dst[0] as f32 * da * inv) / out_a).round() as u8,
        ((src[1] as f32 * sa + dst[1] as f32 * da * inv) / out_a).round() as u8,
        ((src[2] as f32 * sa + dst[2] as f32 * da * inv) / out_a).round() as u8,
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

pub fn fill_rgba(img: &mut RgbaImage, color: [u8; 4]) {
    for p in img.pixels_mut() {
        *p = Rgba(color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_uses_paper_and_layer() {
        let mut rgb = RgbImage::from_pixel(4, 4, Rgb([10, 20, 30]));
        rgb.put_pixel(1, 1, Rgb([200, 0, 0]));
        let doc = EditDocument::from_rgb(&rgb, [250, 250, 250]);
        let flat = doc.flatten_rgb();
        assert_eq!(flat.get_pixel(1, 1).0, [200, 0, 0]);
        assert_eq!(flat.dimensions(), (4, 4));
        let rest = doc.flatten_rgb_except(Some(0));
        assert_eq!(rest.get_pixel(1, 1).0, [250, 250, 250]);
    }

    #[test]
    fn resize_canvas_from_top_keeps_layer_pixels() {
        let rgb = RgbImage::from_pixel(8, 6, Rgb([1, 2, 3]));
        let mut doc = EditDocument::from_rgb(&rgb, [255, 255, 255]);
        doc.layers[0].x = 1;
        doc.layers[0].y = 2;
        let orig = vec![(1, 2)];
        doc.resize_canvas_from(8, 9, &orig, 0, -3);
        assert_eq!((doc.canvas_w, doc.canvas_h), (8, 9));
        assert_eq!((doc.layers[0].x, doc.layers[0].y), (1, 5));
        doc.resize_canvas_from(8, 6, &orig, 0, 0);
        assert_eq!((doc.layers[0].x, doc.layers[0].y), (1, 2));
    }

    #[test]
    fn crop_shifts_layers() {
        let rgb = RgbImage::from_pixel(8, 6, Rgb([1, 2, 3]));
        let mut doc = EditDocument::from_rgb(&rgb, [255, 255, 255]);
        doc.crop_to(2, 1, 5, 4);
        assert_eq!((doc.canvas_w, doc.canvas_h), (4, 4));
        assert_eq!((doc.layers[0].x, doc.layers[0].y), (-2, -1));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("photo_edit_doc_{}", new_id()));
        let rgb = RgbImage::from_pixel(3, 2, Rgb([9, 8, 7]));
        let mut doc = EditDocument::from_rgb(&rgb, [1, 2, 3]);
        doc.source = Some(SourceFingerprint {
            page_id: "p".into(),
            y0: 1,
            y1: 2,
            w: 3,
            h: 2,
            extra_top: -1,
            extra_bottom: 0,
            extra_left: 0,
            extra_right: 0,
        });
        doc.save_to_dir(&dir).unwrap();
        let loaded = EditDocument::load_from_dir(&dir).unwrap();
        assert_eq!(loaded.canvas_w, 3);
        assert_eq!(loaded.paper_rgb, [1, 2, 3]);
        assert_eq!(loaded.layers.len(), 1);
        assert_eq!(loaded.source.as_ref().unwrap().page_id, "p");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn stroke(points: Vec<(i32, i32)>) -> PaintStroke {
        PaintStroke {
            points,
            radius: 2,
            hardness: 1.0,
            color: [1, 2, 3],
        }
    }

    #[test]
    fn gap_in_one_stroke_stays_one_trajectory() {
        let s = stroke(vec![(0, 0), (80, 0)]);
        assert!(s.split_erased(40, 0, 2).is_none());
    }

    #[test]
    fn erasing_the_middle_splits_the_trajectory() {
        let s = stroke(vec![(0, 0), (10, 0), (20, 0), (30, 0), (40, 0)]);
        let parts = s.split_erased(20, 0, 1).expect("middle stamp is covered");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].points, vec![(0, 0), (10, 0)]);
        assert_eq!(parts[1].points, vec![(30, 0), (40, 0)]);
        assert_eq!(parts[0].color, s.color);
        assert_eq!(parts[0].radius, s.radius);
    }

    #[test]
    fn erasing_an_end_does_not_split() {
        let s = stroke(vec![(0, 0), (10, 0), (20, 0)]);
        let parts = s.split_erased(0, 0, 1).expect("first stamp is covered");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].points, vec![(10, 0), (20, 0)]);
    }

    #[test]
    fn ink_rect_matches_full_rebuild() {
        let mut layer = Layer::new("t", RgbaImage::new(40, 40));
        layer.strokes.push(PaintStroke {
            points: vec![(20, 20)],
            radius: 8,
            hardness: 0.0,
            color: [10, 20, 30],
        });
        layer.rebuild_ink_full();
        let ink = layer.ink.as_ref().unwrap();
        let (x, y, before) = (0..40)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .find_map(|(x, y)| {
                let p = ink.get_pixel(x, y).0;
                (p[3] > 0 && p[3] < 255).then_some((x, y, p))
            })
            .expect("soft edge");
        layer.rebuild_ink_rect(0, 0, 40, 40);
        assert_eq!(layer.ink.as_ref().unwrap().get_pixel(x, y).0, before);
    }

    #[test]
    fn split_many_matches_one_by_one() {
        let s = stroke((0..30).map(|i| (i * 4, (i % 3) * 3)).collect());
        let centers: Vec<(i32, i32)> = (0..16).map(|i| (8 + i * 5, 2)).collect();
        let hits = EraserHits::new(&centers, 2);
        let once = s
            .split_erased_hits(&hits)
            .map(|(parts, _)| parts)
            .unwrap_or_else(|| vec![s.clone()]);
        let mut seq = vec![s];
        for &(x, y) in &centers {
            let mut next = Vec::new();
            for part in seq {
                match part.split_erased(x, y, 2) {
                    None => next.push(part),
                    Some(parts) => next.extend(parts),
                }
            }
            seq = next;
        }
        let flat = |v: &[PaintStroke]| v.iter().map(|p| p.points.clone()).collect::<Vec<_>>();
        assert_eq!(flat(&once), flat(&seq));
        assert!(once.len() > 1, "middle samples should cut the stroke");
    }

    #[test]
    fn partial_rebuild_after_middle_erase_matches_full() {
        let mut layer = Layer::new("t", RgbaImage::new(96, 48));
        layer.strokes.push(PaintStroke {
            points: (0..10).map(|i| (8 + i * 8, 24)).collect(),
            radius: 7,
            hardness: 0.25,
            color: [20, 40, 60],
        });
        layer.strokes.push(PaintStroke {
            points: vec![(40, 24), (52, 28)],
            radius: 5,
            hardness: 1.0,
            color: [200, 10, 10],
        });
        layer.rebuild_ink_full();
        let hits = EraserHits::new(&[(40, 24), (48, 24)], 3);
        let mut next = Vec::new();
        let mut clips = Vec::new();
        for stroke in layer.strokes.drain(..) {
            let radius = stroke.radius.max(1);
            match stroke.split_erased_hits(&hits) {
                None => next.push(stroke),
                Some((parts, removed)) => {
                    for (px, py) in removed {
                        clips.push((
                            px - radius - 1,
                            py - radius - 1,
                            px + radius + 2,
                            py + radius + 2,
                        ));
                    }
                    next.extend(parts);
                }
            }
        }
        layer.strokes = next;
        assert!(clips.len() >= 2, "middle dabs removed");
        for (x0, y0, x1, y1) in &clips {
            layer.rebuild_ink_rect(*x0, *y0, *x1, *y1);
        }
        let partial = layer.ink.as_ref().unwrap().clone();
        layer.rebuild_ink_full();
        assert_eq!(partial.as_raw(), layer.ink.as_ref().unwrap().as_raw());
    }
}
