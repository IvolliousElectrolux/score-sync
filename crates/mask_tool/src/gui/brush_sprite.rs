//! 画笔显示缓存.
//!
//! 点列仍是命中, 撤销和导出的来源. 画面上按 512 分块盖圆章,
//! 只把新点补进被碰到的块, 不再每帧重放整条折线.
//! 选中高亮单独落在 halo 层. 取消选中只是不再提交这层,
//! 填充贴图留着, 避免长轨迹在失活时整笔重栅.

use super::*;

const TILE: i32 = 512;
/// 选中时比填充半径多出的一圈, 写进贴图, 不再每帧另画一圈圆章.
const HALO: i32 = 2;
const HALO_COLOR: [u8; 3] = [0xdc, 0x50, 0x50];

pub(crate) struct BrushSprite {
    tiles: HashMap<(i32, i32), TileBuf>,
    baked: usize,
    first: (i32, i32),
    last: (i32, i32),
    /// 点列相对贴图像素的平移. 拖动时只改这个, 不重栅格.
    shift: (i32, i32),
    radius: i32,
    color: [u8; 3],
    opacity: f32,
    selected: bool,
    /// 当前几何的高亮是否已经盖进 halo. 失活后仍保留, 再次选中直接显示.
    halo_baked: bool,
}

struct TileBuf {
    fill: RgbaImage,
    halo: RgbaImage,
    fill_tex: Option<Arc<RenderImage>>,
    halo_tex: Option<Arc<RenderImage>>,
    fill_dirty: bool,
    halo_dirty: bool,
}

pub(crate) struct BrushBlit {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub tex: Arc<RenderImage>,
}

pub(crate) enum OverlayPaint {
    Brush(String),
    Shape(MaskRect),
}

impl BrushSprite {
    pub(crate) fn gpu_bytes(&self) -> u64 {
        self.tiles
            .values()
            .map(|t| {
                let mut n = 0;
                if let Some(tex) = &t.fill_tex {
                    n += gpu_tex_bytes(tex);
                }
                if let Some(tex) = &t.halo_tex {
                    n += gpu_tex_bytes(tex);
                }
                n
            })
            .sum()
    }

    fn cache(&self) -> BrushCache {
        BrushCache {
            radius: self.radius,
            color: self.color,
            opacity: self.opacity,
            baked: self.baked,
            first: self.first,
            last: self.last,
            selected: self.selected,
            halo_baked: self.halo_baked,
        }
    }
}

#[derive(Clone, Copy)]
struct BrushCache {
    radius: i32,
    color: [u8; 3],
    opacity: f32,
    baked: usize,
    first: (i32, i32),
    last: (i32, i32),
    selected: bool,
    halo_baked: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum BrushPlan {
    Drop,
    Keep,
    Selection {
        selected: bool,
        stamp_halo: bool,
    },
    Extend {
        from: usize,
        selected: bool,
        stamp_halo_prefix: bool,
    },
    Rebuild,
}

/// 选中与否不参与几何是否仍有效. 失活不能因此整笔重栅.
fn plan_brush(
    cache: Option<&BrushCache>,
    points: &[(i32, i32)],
    radius: i32,
    color: [u8; 3],
    opacity: f32,
    selected: bool,
) -> BrushPlan {
    if points.is_empty() {
        return BrushPlan::Drop;
    }
    let Some(s) = cache else {
        return BrushPlan::Rebuild;
    };
    let len = points.len();
    let geom = s.radius == radius
        && s.color == color
        && (s.opacity - opacity).abs() < 1e-4
        && s.baked > 0
        && s.baked <= len
        && s.first == points[0]
        && points.get(s.baked - 1) == Some(&s.last);
    if !geom {
        return BrushPlan::Rebuild;
    }
    let stamp_halo = selected && !s.halo_baked;
    if s.baked == len {
        if s.selected == selected && !stamp_halo {
            BrushPlan::Keep
        } else {
            BrushPlan::Selection {
                selected,
                stamp_halo,
            }
        }
    } else {
        BrushPlan::Extend {
            from: s.baked,
            selected,
            stamp_halo_prefix: stamp_halo,
        }
    }
}

impl MaskToolApp {
    /// 非画笔走原来的画布坐标; 画笔只交出 id, 点列留给贴图缓存.
    pub(super) fn overlay_paint_items(&self) -> Vec<OverlayPaint> {
        self.masks
            .iter()
            .map(|m| {
                if m.is_brush() {
                    OverlayPaint::Brush(m.id.clone())
                } else if self.masks_are_sheet {
                    OverlayPaint::Shape(self.mask_sheet_to_canvas(m.clone()))
                } else {
                    OverlayPaint::Shape(m.clone())
                }
            })
            .collect()
    }

    pub(super) fn sync_brush_sprites(&mut self) -> HashMap<String, Vec<BrushBlit>> {
        let live: Vec<String> = self
            .masks
            .iter()
            .filter(|m| m.is_brush())
            .map(|m| m.id.clone())
            .collect();
        let stale: Vec<String> = self
            .brush_sprites
            .keys()
            .filter(|id| !live.iter().any(|k| k == *id))
            .cloned()
            .collect();
        for id in stale {
            self.drop_brush_sprite(&id);
        }
        for id in &live {
            self.ensure_brush_sprite(id);
        }
        let mut out = HashMap::new();
        for id in &live {
            let Some(sprite) = self.brush_sprites.get(id) else {
                continue;
            };
            let mut blits = Vec::new();
            for ((tx, ty), tile) in &sprite.tiles {
                let x = tx * TILE + sprite.shift.0;
                let y = ty * TILE + sprite.shift.1;
                if sprite.selected {
                    if let Some(tex) = &tile.halo_tex {
                        blits.push(BrushBlit {
                            x,
                            y,
                            w: TILE as u32,
                            h: TILE as u32,
                            tex: tex.clone(),
                        });
                    }
                }
                if let Some(tex) = &tile.fill_tex {
                    blits.push(BrushBlit {
                        x,
                        y,
                        w: TILE as u32,
                        h: TILE as u32,
                        tex: tex.clone(),
                    });
                }
            }
            out.insert(id.clone(), blits);
        }
        out
    }

    /// 整笔平移. 贴图像素不动, 绘制原点跟着点列走.
    pub(super) fn nudge_brush_sprite(&mut self, id: &str, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let Some(sprite) = self.brush_sprites.get_mut(id) else {
            return;
        };
        sprite.shift.0 += dx;
        sprite.shift.1 += dy;
        sprite.first.0 += dx;
        sprite.first.1 += dy;
        sprite.last.0 += dx;
        sprite.last.1 += dy;
    }

    pub(super) fn nudge_brush_sprites(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let ids: Vec<String> = self.brush_sprites.keys().cloned().collect();
        for id in ids {
            self.nudge_brush_sprite(&id, dx, dy);
        }
    }

    fn ensure_brush_sprite(&mut self, id: &str) {
        enum Job {
            Drop,
            Keep,
            /// 几何未变. `halo_pts` 只在第一次点亮且高亮还没栅过时带上整条点列.
            Selection {
                selected: bool,
                halo_pts: Option<Vec<(i32, i32)>>,
            },
            Extend {
                seg: Vec<(i32, i32)>,
                selected: bool,
                /// 已栅格的前缀还没有高亮, 而这次要显示高亮.
                halo_prefix: Option<Vec<(i32, i32)>>,
            },
            Rebuild(Vec<(i32, i32)>, i32, [u8; 3], f32, bool),
        }
        let job = {
            let Some(mask) = self.masks.iter().find(|m| m.id == id) else {
                return;
            };
            let points = &mask.brush_points;
            let radius = mask.brush_radius.max(1);
            let color = mask.color;
            let opacity = mask.effective_opacity();
            let selected = self.selected.contains(id);
            let cache = self.brush_sprites.get(id).map(BrushSprite::cache);
            match plan_brush(cache.as_ref(), points, radius, color, opacity, selected) {
                BrushPlan::Drop => Job::Drop,
                BrushPlan::Keep => Job::Keep,
                BrushPlan::Selection {
                    selected,
                    stamp_halo,
                } => Job::Selection {
                    selected,
                    halo_pts: stamp_halo.then(|| points.to_vec()),
                },
                BrushPlan::Extend {
                    from,
                    selected,
                    stamp_halo_prefix,
                } => Job::Extend {
                    seg: points[from - 1..].to_vec(),
                    selected,
                    halo_prefix: stamp_halo_prefix.then(|| points[..from].to_vec()),
                },
                BrushPlan::Rebuild => {
                    Job::Rebuild(points.to_vec(), radius, color, opacity, selected)
                }
            }
        };
        match job {
            Job::Drop => self.drop_brush_sprite(id),
            Job::Keep => {}
            Job::Selection { selected, halo_pts } => {
                self.set_brush_sprite_selected(id, selected);
                if let Some(pts) = halo_pts {
                    self.stamp_brush_halo(id, &pts);
                }
            }
            Job::Extend {
                seg,
                selected,
                halo_prefix,
            } => {
                self.set_brush_sprite_selected(id, selected);
                if let Some(pts) = halo_prefix {
                    self.stamp_brush_halo(id, &pts);
                }
                self.extend_brush_sprite(id, &seg);
            }
            Job::Rebuild(points, radius, color, opacity, selected) => {
                self.rebuild_brush_sprite(id, &points, radius, color, opacity, selected);
            }
        }
    }

    fn set_brush_sprite_selected(&mut self, id: &str, selected: bool) {
        let Some(sprite) = self.brush_sprites.get_mut(id) else {
            return;
        };
        sprite.selected = selected;
    }

    fn stamp_brush_halo(&mut self, id: &str, points: &[(i32, i32)]) {
        {
            let Some(sprite) = self.brush_sprites.get_mut(id) else {
                return;
            };
            stamp_halo_polyline(sprite, points);
        }
        upload_dirty_in(self, id);
    }

    fn rebuild_brush_sprite(
        &mut self,
        id: &str,
        points: &[(i32, i32)],
        radius: i32,
        color: [u8; 3],
        opacity: f32,
        selected: bool,
    ) {
        self.drop_brush_sprite(id);
        let mut sprite = BrushSprite {
            tiles: HashMap::new(),
            baked: 0,
            first: points[0],
            last: points[0],
            shift: (0, 0),
            radius,
            color,
            opacity,
            selected,
            halo_baked: false,
        };
        stamp_disk(&mut sprite, points[0].0, points[0].1);
        for w in points.windows(2) {
            stamp_segment(&mut sprite, w[0], w[1]);
        }
        sprite.baked = points.len();
        sprite.last = *points.last().unwrap_or(&points[0]);
        sprite.halo_baked = selected;
        upload_dirty(self, &mut sprite);
        self.brush_sprites.insert(id.to_string(), sprite);
    }

    fn extend_brush_sprite(&mut self, id: &str, seg: &[(i32, i32)]) {
        {
            let Some(sprite) = self.brush_sprites.get_mut(id) else {
                return;
            };
            for w in seg.windows(2) {
                stamp_segment(sprite, w[0], w[1]);
            }
            if let Some(&last) = seg.last() {
                sprite.last = last;
            }
            sprite.baked += seg.len().saturating_sub(1);
            if !sprite.selected {
                sprite.halo_baked = false;
            }
        }
        upload_dirty_in(self, id);
    }

    fn drop_brush_sprite(&mut self, id: &str) {
        let Some(sprite) = self.brush_sprites.remove(id) else {
            return;
        };
        for tile in sprite.tiles.into_values() {
            self.retire_gpu_image(tile.fill_tex);
            self.retire_gpu_image(tile.halo_tex);
        }
    }
}

fn upload_dirty(app: &mut MaskToolApp, sprite: &mut BrushSprite) {
    for tile in sprite.tiles.values_mut() {
        if tile.fill_dirty {
            if let Some(old) = tile.fill_tex.replace(upload_rgba(&tile.fill)) {
                app.retire_gpu_image(Some(old));
            }
            tile.fill_dirty = false;
        }
        if tile.halo_dirty {
            if let Some(old) = tile.halo_tex.replace(upload_rgba(&tile.halo)) {
                app.retire_gpu_image(Some(old));
            }
            tile.halo_dirty = false;
        }
    }
}

fn upload_dirty_in(app: &mut MaskToolApp, id: &str) {
    let pending: Vec<((i32, i32), bool, Arc<RenderImage>)> = {
        let Some(sprite) = app.brush_sprites.get_mut(id) else {
            return;
        };
        let mut pending = Vec::new();
        for (key, tile) in sprite.tiles.iter_mut() {
            if tile.fill_dirty {
                pending.push((*key, false, upload_rgba(&tile.fill)));
                tile.fill_dirty = false;
            }
            if tile.halo_dirty {
                pending.push((*key, true, upload_rgba(&tile.halo)));
                tile.halo_dirty = false;
            }
        }
        pending
    };
    for (key, halo, tex) in pending {
        let Some(tile) = app
            .brush_sprites
            .get_mut(id)
            .and_then(|s| s.tiles.get_mut(&key))
        else {
            app.retire_gpu_image(Some(tex));
            continue;
        };
        let slot = if halo {
            &mut tile.halo_tex
        } else {
            &mut tile.fill_tex
        };
        if let Some(old) = slot.replace(tex) {
            app.retire_gpu_image(Some(old));
        }
    }
}

fn stamp_segment(sprite: &mut BrushSprite, a: (i32, i32), b: (i32, i32)) {
    let radius = sprite.radius;
    walk_segment(a, b, radius, |x, y| stamp_disk(sprite, x, y));
}

fn stamp_halo_polyline(sprite: &mut BrushSprite, points: &[(i32, i32)]) {
    if points.is_empty() {
        return;
    }
    stamp_halo_disk(sprite, points[0].0, points[0].1);
    let radius = sprite.radius;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        walk_segment(a, b, radius, |x, y| stamp_halo_disk(sprite, x, y));
    }
    sprite.halo_baked = true;
}

fn stamp_halo_disk(sprite: &mut BrushSprite, cx: i32, cy: i32) {
    let x = cx - sprite.shift.0;
    let y = cy - sprite.shift.1;
    let r = sprite.radius.max(1);
    paint_disk(sprite, x, y, r + HALO, HALO_COLOR, 1.0, true);
}

fn walk_segment(a: (i32, i32), b: (i32, i32), radius: i32, mut visit: impl FnMut(i32, i32)) {
    let r = radius.max(1) as f32;
    let step = (r * 0.5).max(1.0);
    let (x0, y0) = (a.0 as f32, a.1 as f32);
    let (x1, y1) = (b.0 as f32, b.1 as f32);
    let dist = (x1 - x0).hypot(y1 - y0).max(0.001);
    let n = (dist / step).ceil() as i32;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let x = (x0 + (x1 - x0) * t).round() as i32;
        let y = (y0 + (y1 - y0) * t).round() as i32;
        visit(x, y);
    }
}

fn stamp_disk(sprite: &mut BrushSprite, cx: i32, cy: i32) {
    let x = cx - sprite.shift.0;
    let y = cy - sprite.shift.1;
    let r = sprite.radius.max(1);
    if sprite.selected {
        paint_disk(sprite, x, y, r + HALO, HALO_COLOR, 1.0, true);
    }
    paint_disk(sprite, x, y, r, sprite.color, sprite.opacity, false);
}

fn paint_disk(
    sprite: &mut BrushSprite,
    cx: i32,
    cy: i32,
    r: i32,
    color: [u8; 3],
    alpha: f32,
    halo: bool,
) {
    let r = r.max(0);
    let tx0 = (cx - r).div_euclid(TILE);
    let tx1 = (cx + r).div_euclid(TILE);
    let ty0 = (cy - r).div_euclid(TILE);
    let ty1 = (cy + r).div_euclid(TILE);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            if !disk_hits_tile(cx, cy, r, origin_x, origin_y) {
                continue;
            }
            let tile = sprite.tiles.entry((tx, ty)).or_insert_with(TileBuf::blank);
            let hit = {
                let img = if halo { &mut tile.halo } else { &mut tile.fill };
                stamp_into(img, origin_x, origin_y, cx, cy, r, color, alpha)
            };
            if hit {
                if halo {
                    tile.halo_dirty = true;
                } else {
                    tile.fill_dirty = true;
                }
            }
        }
    }
}

fn disk_hits_tile(cx: i32, cy: i32, r: i32, origin_x: i32, origin_y: i32) -> bool {
    let x = cx.clamp(origin_x, origin_x + TILE - 1);
    let y = cy.clamp(origin_y, origin_y + TILE - 1);
    let dx = i64::from(cx - x);
    let dy = i64::from(cy - y);
    dx * dx + dy * dy <= i64::from(r) * i64::from(r)
}

fn stamp_into(
    img: &mut RgbaImage,
    origin_x: i32,
    origin_y: i32,
    cx: i32,
    cy: i32,
    r: i32,
    color: [u8; 3],
    alpha: f32,
) -> bool {
    let r2 = i64::from(r) * i64::from(r);
    let left = (cx - r).max(origin_x);
    let right = (cx + r).min(origin_x + TILE - 1);
    let top = (cy - r).max(origin_y);
    let bot = (cy + r).min(origin_y + TILE - 1);
    if left > right || top > bot {
        return false;
    }
    let mut hit = false;
    for y in top..=bot {
        for x in left..=right {
            let dx = i64::from(x - cx);
            let dy = i64::from(y - cy);
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let px = img.get_pixel_mut((x - origin_x) as u32, (y - origin_y) as u32);
            blend(px, color, alpha);
            hit = true;
        }
    }
    hit
}

fn blend(px: &mut image::Rgba<u8>, color: [u8; 3], alpha: f32) {
    let sa = alpha.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = px[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 1e-6 {
        return;
    }
    for i in 0..3 {
        let s = color[i] as f32;
        let d = px[i] as f32;
        px[i] = ((s * sa + d * da * (1.0 - sa)) / out_a).round() as u8;
    }
    px[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

impl TileBuf {
    fn blank() -> Self {
        Self {
            fill: RgbaImage::new(TILE as u32, TILE as u32),
            halo: RgbaImage::new(TILE as u32, TILE as u32),
            fill_tex: None,
            halo_tex: None,
            fill_dirty: false,
            halo_dirty: false,
        }
    }
}

fn upload_rgba(src: &RgbaImage) -> Arc<RenderImage> {
    let (w, h) = src.dimensions();
    let raw = src.as_raw();
    let mut buf = vec![0u8; raw.len()];
    for (s, d) in raw.chunks_exact(4).zip(buf.chunks_exact_mut(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
    let img = RgbaImage::from_raw(w, h, buf).expect("brush tile");
    Arc::new(RenderImage::new(smallvec![Frame::new(img)]))
}

#[cfg(test)]
mod tests {
    use super::{plan_brush, stamp_into, BrushCache, BrushPlan};
    use image::RgbaImage;

    fn cache(selected: bool, halo_baked: bool, baked: usize) -> BrushCache {
        BrushCache {
            radius: 8,
            color: [255, 255, 255],
            opacity: 0.5,
            baked,
            first: (0, 0),
            last: (baked as i32 - 1, 0),
            selected,
            halo_baked,
        }
    }

    fn line(n: usize) -> Vec<(i32, i32)> {
        (0..n as i32).map(|x| (x, 0)).collect()
    }

    #[test]
    fn deselect_keeps_fill_and_does_not_restamp() {
        let points = line(4000);
        let have = cache(true, true, points.len());
        assert_eq!(
            plan_brush(Some(&have), &points, 8, [255, 255, 255], 0.5, false),
            BrushPlan::Selection {
                selected: false,
                stamp_halo: false,
            }
        );
    }

    #[test]
    fn reselect_reuses_baked_halo() {
        let points = line(4000);
        let have = cache(false, true, points.len());
        assert_eq!(
            plan_brush(Some(&have), &points, 8, [255, 255, 255], 0.5, true),
            BrushPlan::Selection {
                selected: true,
                stamp_halo: false,
            }
        );
    }

    #[test]
    fn first_select_stamps_halo_only() {
        let points = line(12);
        let have = cache(false, false, points.len());
        assert_eq!(
            plan_brush(Some(&have), &points, 8, [255, 255, 255], 0.5, true),
            BrushPlan::Selection {
                selected: true,
                stamp_halo: true,
            }
        );
    }

    #[test]
    fn extend_while_selected_does_not_rebuild() {
        let points = line(20);
        let have = cache(true, true, 12);
        assert_eq!(
            plan_brush(Some(&have), &points, 8, [255, 255, 255], 0.5, true),
            BrushPlan::Extend {
                from: 12,
                selected: true,
                stamp_halo_prefix: false,
            }
        );
    }

    #[test]
    fn radius_change_still_rebuilds() {
        let points = line(4);
        let have = cache(true, true, points.len());
        assert_eq!(
            plan_brush(Some(&have), &points, 9, [255, 255, 255], 0.5, true),
            BrushPlan::Rebuild
        );
    }

    #[test]
    fn disk_covers_center_and_stops_at_radius() {
        let mut img = RgbaImage::new(64, 64);
        assert!(stamp_into(&mut img, 0, 0, 10, 12, 4, [255, 0, 0], 1.0));
        assert_eq!(img.get_pixel(10, 12)[3], 255);
        assert_eq!(img.get_pixel(10, 12)[0], 255);
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        assert_eq!(img.get_pixel(14, 12)[3], 255);
        assert_eq!(img.get_pixel(15, 12)[3], 0);
    }
}
