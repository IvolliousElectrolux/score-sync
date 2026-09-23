//! 蒙版 / 底色画布的视口分辨率.
//!
//! 缩略图够用时不另开贴图. 放大到一颗缩略图像素铺不满屏幕时, 从磁盘原图
//! (或独立打开时的内存原图) 裁出当前视口, 贴图像素对齐窗口, 不超过 1:1.

use super::*;

const LOD_PAD: f32 = 0.22;
const LOD_DENSITY_SLACK: f32 = 1.02;
const LOD_COVER_DENSITY: f32 = 0.92;
const LOD_CACHE_MAX: usize = 2;

#[derive(Clone)]
pub(crate) struct PieceDetail {
    pub region_id: String,
    pub tex: Arc<RenderImage>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// 贴图实际像素, 用来判断再放大后还够不够.
    pub tex_w: u32,
    pub tex_h: u32,
}

/// 一次取景请求. `x/y/w/h` 是分块局部的逻辑像素 (整图时即画布像素).
#[derive(Clone)]
pub(crate) struct LodNeed {
    pub region_id: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub tex_w: u32,
    pub tex_h: u32,
    path: PathBuf,
    flat: bool,
    page_w: u32,
    page_h: u32,
    band_y: u32,
    band_h: u32,
    logical_w: u32,
    logical_h: u32,
}

struct Placed {
    region_id: String,
    hx: f32,
    origin_y: f32,
    logical_w: u32,
    logical_h: u32,
    clip_x0: f32,
    clip_y0: f32,
    clip_x1: f32,
    clip_y1: f32,
}

impl MaskToolApp {
    pub(super) fn sync_view_lod(&mut self, cx: &mut Context<Self>) {
        let vw = f32::from(self.view_bounds.size.width);
        let vh = f32::from(self.view_bounds.size.height);
        if vw < 8.0 || vh < 8.0 || self.img_w == 0 || self.img_h == 0 {
            return;
        }
        if self.block_tiles.is_empty() {
            self.sync_single_image_lod(cx, vw, vh);
            return;
        }
        let xform = self.xform();
        let cs = if self.content_scale > 0.0001 {
            self.content_scale
        } else {
            1.0
        };
        let screen_per = xform.scale * cs;
        let (ax, ay) = xform.screen_to_image(0.0, 0.0);
        let (bx, by) = xform.screen_to_image(vw, vh);
        let placed = placed_pieces(
            &self.block_tiles,
            &self.block_layout,
            self.block_hoff,
            self.block_voff,
            cs,
        );
        self.finish_tile_lod(
            cx,
            screen_per,
            &placed,
            ax.min(bx),
            ay.min(by),
            ax.max(bx),
            ay.max(by),
            cs,
        );
    }

    fn finish_tile_lod(
        &mut self,
        cx: &mut Context<Self>,
        screen_per: f32,
        placed: &[Placed],
        vis_x0: f32,
        vis_y0: f32,
        vis_x1: f32,
        vis_y1: f32,
        cs: f32,
    ) {
        let mut jobs = Vec::new();
        let mut tight = Vec::new();
        for (tile, place) in self.block_tiles.iter().zip(placed.iter()) {
            let Some(src) = tile.source.as_ref() else {
                continue;
            };
            let density = tile.thumb.width().max(1) as f32 / tile.width.max(1) as f32;
            if screen_per <= density * LOD_DENSITY_SLACK {
                continue;
            }
            let Some(vis) = local_visible(place, vis_x0, vis_y0, vis_x1, vis_y1, cs, 0.0) else {
                continue;
            };
            let Some(mut job) = local_visible(place, vis_x0, vis_y0, vis_x1, vis_y1, cs, LOD_PAD)
            else {
                continue;
            };
            let src_w = source_span(job.w, tile.width, src.page_w.max(1));
            let src_span_h = if src.flat { src.page_h } else { src.band_h };
            let src_h = source_span(job.h, tile.height, src_span_h.max(1));
            job.tex_w = (((job.w as f32) * screen_per.min(1.0)).round() as u32)
                .min(src_w)
                .max(1);
            job.tex_h = (((job.h as f32) * screen_per.min(1.0)).round() as u32)
                .min(src_h)
                .max(1);
            job.region_id = tile.region_id.clone();
            job.path = src.path.clone();
            job.flat = src.flat;
            job.page_w = src.page_w.max(1);
            job.page_h = src.page_h.max(1);
            job.band_y = src.band_y;
            job.band_h = src.band_h.max(1);
            job.logical_w = tile.width.max(1);
            job.logical_h = tile.height.max(1);
            let mut vis_need = vis;
            vis_need.region_id = tile.region_id.clone();
            vis_need.tex_w = (((vis_need.w as f32) * screen_per.min(1.0)).round() as u32).max(1);
            vis_need.tex_h = (((vis_need.h as f32) * screen_per.min(1.0)).round() as u32).max(1);
            tight.push(vis_need);
            jobs.push(job);
        }
        if jobs.is_empty() {
            if !self.view_detail.is_empty() || self.detail_pending.is_some() {
                self.clear_view_detail();
            }
            return;
        }
        if tight.iter().all(|need| self.detail_covers(need)) {
            return;
        }
        if let Some(pending) = &self.detail_pending {
            if tight
                .iter()
                .all(|need| pending.iter().any(|have| have.covers(need)))
            {
                return;
            }
        }
        self.detail_gen = self.detail_gen.wrapping_add(1);
        let gen = self.detail_gen;
        let mut cache = HashMap::new();
        for job in &jobs {
            if let Some(img) = self.lod_cache.get(&job.path) {
                cache.insert(job.path.clone(), img.clone());
            }
        }
        self.detail_pending = Some(jobs.clone());
        let (tx, rx) =
            async_channel::bounded::<(Vec<PieceDetail>, Vec<(PathBuf, Arc<image::RgbImage>)>)>(1);
        std::thread::spawn(move || {
            let built = build_details(&jobs, &mut cache);
            let fresh: Vec<_> = cache.into_iter().collect();
            let _ = tx.send_blocking((built, fresh));
        });
        cx.spawn(async move |this, cx| {
            if let Ok((built, fresh)) = rx.recv().await {
                this.update(cx, |view, cx| {
                    if view.detail_gen != gen {
                        for d in built {
                            view.retire_gpu_image(Some(d.tex));
                        }
                        return;
                    }
                    view.detail_pending = None;
                    view.install_details(built);
                    for (path, img) in fresh {
                        view.remember_page(path, img);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn sync_single_image_lod(&mut self, cx: &mut Context<Self>, vw: f32, vh: f32) {
        let Some(rgb) = self.rgb_image.as_ref() else {
            if !self.view_detail.is_empty() || self.detail_pending.is_some() {
                self.clear_view_detail();
            }
            return;
        };
        let (rw, rh) = rgb.dimensions();
        if rw < 2 || rh < 2 {
            return;
        }
        let xform = self.xform();
        let render_w = self
            .render_image
            .as_ref()
            .map(|img| {
                let sz = img.size(0);
                (sz.width.0 as f32).max(1.0)
            })
            .unwrap_or(rw as f32);
        let density = render_w / self.img_w.max(1) as f32;
        if xform.scale <= density * LOD_DENSITY_SLACK {
            if !self.view_detail.is_empty() || self.detail_pending.is_some() {
                self.clear_view_detail();
            }
            return;
        }
        let (ax, ay) = xform.screen_to_image(0.0, 0.0);
        let (bx, by) = xform.screen_to_image(vw, vh);
        let place = Placed {
            region_id: String::new(),
            hx: 0.0,
            origin_y: 0.0,
            logical_w: self.img_w.max(1),
            logical_h: self.img_h.max(1),
            clip_x0: 0.0,
            clip_y0: 0.0,
            clip_x1: self.img_w as f32,
            clip_y1: self.img_h as f32,
        };
        let Some(vis) = local_visible(
            &place,
            ax.min(bx),
            ay.min(by),
            ax.max(bx),
            ay.max(by),
            1.0,
            0.0,
        ) else {
            return;
        };
        let Some(mut job) = local_visible(
            &place,
            ax.min(bx),
            ay.min(by),
            ax.max(bx),
            ay.max(by),
            1.0,
            LOD_PAD,
        ) else {
            return;
        };
        let src_w = source_span(job.w, self.img_w.max(1), rw);
        let src_h = source_span(job.h, self.img_h.max(1), rh);
        job.tex_w = (((job.w as f32) * xform.scale.min(1.0)).round() as u32)
            .min(src_w)
            .max(1);
        job.tex_h = (((job.h as f32) * xform.scale.min(1.0)).round() as u32)
            .min(src_h)
            .max(1);
        job.logical_w = self.img_w.max(1);
        job.logical_h = self.img_h.max(1);
        job.page_w = rw;
        job.page_h = rh;
        job.flat = true;
        let mut vis_need = vis;
        vis_need.tex_w = (((vis_need.w as f32) * xform.scale.min(1.0)).round() as u32).max(1);
        vis_need.tex_h = (((vis_need.h as f32) * xform.scale.min(1.0)).round() as u32).max(1);
        if self.detail_covers(&vis_need) {
            return;
        }
        if let Some(pending) = &self.detail_pending {
            if pending.iter().any(|have| have.covers(&vis_need)) {
                return;
            }
        }
        let x = map_pos(job.x, job.logical_w, 0, rw, rw);
        let y = map_pos(job.y, job.logical_h, 0, rh, rh);
        let x1 = map_pos(job.x + job.w, job.logical_w, 0, rw, rw).max(x + 1);
        let y1 = map_pos(job.y + job.h, job.logical_h, 0, rh, rh).max(y + 1);
        let crop = crop_rgb(rgb, x, y, x1 - x, y1 - y);
        let tex_w = job.tex_w;
        let tex_h = job.tex_h;
        let detail_x = job.x as i32;
        let detail_y = job.y as i32;
        let detail_w = job.w;
        let detail_h = job.h;
        self.detail_gen = self.detail_gen.wrapping_add(1);
        let gen = self.detail_gen;
        self.detail_pending = Some(vec![job]);
        let (tx, rx) = async_channel::bounded::<PieceDetail>(1);
        std::thread::spawn(move || {
            let scaled = scale_rgb(&crop, tex_w, tex_h);
            let tex = rgb_to_render_image_capped(&scaled, tex_w.max(tex_h));
            let _ = tx.send_blocking(PieceDetail {
                region_id: String::new(),
                tex,
                x: detail_x,
                y: detail_y,
                w: detail_w,
                h: detail_h,
                tex_w,
                tex_h,
            });
        });
        cx.spawn(async move |this, cx| {
            if let Ok(detail) = rx.recv().await {
                this.update(cx, |view, cx| {
                    if view.detail_gen != gen {
                        view.retire_gpu_image(Some(detail.tex));
                        return;
                    }
                    view.detail_pending = None;
                    view.install_details(vec![detail]);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn clear_view_detail(&mut self) {
        self.detail_pending = None;
        self.detail_gen = self.detail_gen.wrapping_add(1);
        let old = std::mem::take(&mut self.view_detail);
        for d in old {
            self.retire_gpu_image(Some(d.tex));
        }
        self.lod_cache.clear();
        self.lod_cache_order.clear();
    }

    fn install_details(&mut self, next: Vec<PieceDetail>) {
        let old = std::mem::replace(&mut self.view_detail, next);
        for d in old {
            self.retire_gpu_image(Some(d.tex));
        }
    }

    fn detail_covers(&self, need: &LodNeed) -> bool {
        self.view_detail.iter().any(|d| {
            if d.region_id != need.region_id {
                return false;
            }
            let nx1 = need.x.saturating_add(need.w);
            let ny1 = need.y.saturating_add(need.h);
            let dx = d.x.max(0) as u32;
            let dy = d.y.max(0) as u32;
            let dx1 = dx.saturating_add(d.w);
            let dy1 = dy.saturating_add(d.h);
            if dx > need.x || dy > need.y || dx1 < nx1 || dy1 < ny1 {
                return false;
            }
            let have_x = d.tex_w as f32 / d.w.max(1) as f32;
            let have_y = d.tex_h as f32 / d.h.max(1) as f32;
            let want_x = need.tex_w as f32 / need.w.max(1) as f32;
            let want_y = need.tex_h as f32 / need.h.max(1) as f32;
            have_x + 1e-6 >= want_x * LOD_COVER_DENSITY
                && have_y + 1e-6 >= want_y * LOD_COVER_DENSITY
        })
    }

    fn remember_page(&mut self, path: PathBuf, img: Arc<image::RgbImage>) {
        if self.lod_cache.contains_key(&path) {
            self.lod_cache_order.retain(|p| p != &path);
        }
        self.lod_cache.insert(path.clone(), img);
        self.lod_cache_order.push(path);
        while self.lod_cache_order.len() > LOD_CACHE_MAX {
            let old = self.lod_cache_order.remove(0);
            self.lod_cache.remove(&old);
        }
    }
}

impl LodNeed {
    fn covers(&self, need: &LodNeed) -> bool {
        if self.region_id != need.region_id {
            return false;
        }
        let nx1 = need.x.saturating_add(need.w);
        let ny1 = need.y.saturating_add(need.h);
        let cx1 = self.x.saturating_add(self.w);
        let cy1 = self.y.saturating_add(self.h);
        if self.x > need.x || self.y > need.y || cx1 < nx1 || cy1 < ny1 {
            return false;
        }
        let have = self.tex_w as f32 / self.w.max(1) as f32;
        let want = need.tex_w as f32 / need.w.max(1) as f32;
        have + 1e-6 >= want * LOD_COVER_DENSITY
    }
}

fn placed_pieces(
    tiles: &[BlockTile],
    layout: &[BlockAdjust],
    hoff: i64,
    voff: i64,
    cs: f32,
) -> Vec<Placed> {
    let canvas_x = |sx: f32| hoff as f32 + sx * cs;
    let canvas_y = |sy: f32| voff as f32 + sy * cs;
    let mut yy: i64 = 0;
    let mut out = Vec::with_capacity(tiles.len());
    for tile in tiles {
        let adj = BlockAdjust::find(layout, &tile.region_id)
            .cloned()
            .unwrap_or_default();
        let (gap, ext_top, content_h, ext_bottom, _trim_top) =
            crate::layout::effective_metrics(tile.height as i32, &adj);
        let (trim_l, _trim_r, _ext_l, _ext_r, content_w) =
            crate::layout::effective_h_metrics(tile.width as i32, &adj);
        if gap > 0 {
            yy += gap as i64;
        }
        let content_y = yy + ext_top as i64;
        let hx = canvas_x(adj.shift_x as f32);
        let origin_y = canvas_y((yy + adj.extra_top as i64) as f32);
        out.push(Placed {
            region_id: tile.region_id.clone(),
            hx,
            origin_y,
            logical_w: tile.width.max(1),
            logical_h: tile.height.max(1),
            clip_x0: canvas_x((adj.shift_x + trim_l as i32) as f32),
            clip_y0: canvas_y(content_y as f32),
            clip_x1: canvas_x((adj.shift_x + trim_l as i32) as f32) + content_w as f32 * cs,
            clip_y1: canvas_y(content_y as f32) + content_h as f32 * cs,
        });
        yy += ext_top as i64 + content_h as i64;
        if ext_bottom > 0 {
            yy += ext_bottom as i64;
        }
    }
    out
}

fn local_visible(
    place: &Placed,
    vis_x0: f32,
    vis_y0: f32,
    vis_x1: f32,
    vis_y1: f32,
    cs: f32,
    pad: f32,
) -> Option<LodNeed> {
    let cs = if cs > 0.0001 { cs } else { 1.0 };
    let x0 = vis_x0.max(place.clip_x0);
    let y0 = vis_y0.max(place.clip_y0);
    let x1 = vis_x1.min(place.clip_x1);
    let y1 = vis_y1.min(place.clip_y1);
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    let mut lx0 = (x0 - place.hx) / cs;
    let mut ly0 = (y0 - place.origin_y) / cs;
    let mut lx1 = (x1 - place.hx) / cs;
    let mut ly1 = (y1 - place.origin_y) / cs;
    let pw = (lx1 - lx0).abs() * pad.max(0.0);
    let ph = (ly1 - ly0).abs() * pad.max(0.0);
    let lw = place.logical_w as f32;
    let lh = place.logical_h as f32;
    lx0 = (lx0 - pw).clamp(0.0, lw);
    ly0 = (ly0 - ph).clamp(0.0, lh);
    lx1 = (lx1 + pw).clamp(0.0, lw);
    ly1 = (ly1 + ph).clamp(0.0, lh);
    if lx1 - lx0 < 1.0 || ly1 - ly0 < 1.0 {
        return None;
    }
    let x = lx0.floor() as u32;
    let y = ly0.floor() as u32;
    let w = ((lx1.ceil() as u32).saturating_sub(x))
        .max(1)
        .min(place.logical_w.saturating_sub(x));
    let h = ((ly1.ceil() as u32).saturating_sub(y))
        .max(1)
        .min(place.logical_h.saturating_sub(y));
    Some(LodNeed {
        region_id: place.region_id.clone(),
        x,
        y,
        w,
        h,
        tex_w: w,
        tex_h: h,
        path: PathBuf::new(),
        flat: true,
        page_w: 1,
        page_h: 1,
        band_y: 0,
        band_h: 1,
        logical_w: place.logical_w,
        logical_h: place.logical_h,
    })
}

fn source_span(span: u32, logical: u32, src_span: u32) -> u32 {
    if logical == 0 {
        return 1;
    }
    ((span as u64 * src_span as u64) / logical as u64).max(1) as u32
}

fn map_pos(local: u32, logical: u32, origin: u32, span: u32, limit: u32) -> u32 {
    let logical = logical.max(1) as f64;
    let v = origin as f64 + local as f64 / logical * span as f64;
    (v.round() as u32).min(limit)
}

fn build_details(
    jobs: &[LodNeed],
    cache: &mut HashMap<PathBuf, Arc<image::RgbImage>>,
) -> Vec<PieceDetail> {
    let mut out = Vec::with_capacity(jobs.len());
    for job in jobs {
        let src = if let Some(img) = cache.get(&job.path) {
            img.clone()
        } else if let Some(img) = load_rgb(&job.path) {
            let img = Arc::new(img);
            cache.insert(job.path.clone(), img.clone());
            img
        } else {
            continue;
        };
        let (fw, fh) = src.dimensions();
        if fw < 1 || fh < 1 {
            continue;
        }
        let y_origin = if job.flat { 0 } else { job.band_y };
        let y_span = if job.flat { job.page_h } else { job.band_h };
        let x0 = map_pos(job.x, job.logical_w, 0, job.page_w, fw);
        let y0 = map_pos(job.y, job.logical_h, y_origin, y_span, fh);
        let x1 = map_pos(job.x + job.w, job.logical_w, 0, job.page_w, fw)
            .max(x0.saturating_add(1))
            .min(fw);
        let y1 = map_pos(job.y + job.h, job.logical_h, y_origin, y_span, fh)
            .max(y0.saturating_add(1))
            .min(fh);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let crop = crop_rgb(&src, x0, y0, x1 - x0, y1 - y0);
        let scaled = scale_rgb(&crop, job.tex_w, job.tex_h);
        out.push(PieceDetail {
            region_id: job.region_id.clone(),
            tex: rgb_to_render_image_capped(&scaled, job.tex_w.max(job.tex_h)),
            x: job.x as i32,
            y: job.y as i32,
            w: job.w,
            h: job.h,
            tex_w: job.tex_w,
            tex_h: job.tex_h,
        });
    }
    out
}

fn load_rgb(path: &std::path::Path) -> Option<image::RgbImage> {
    image::open(path).ok().map(|img| img.to_rgb8())
}

fn crop_rgb(src: &image::RgbImage, x: u32, y: u32, w: u32, h: u32) -> image::RgbImage {
    let x = x.min(src.width().saturating_sub(1));
    let y = y.min(src.height().saturating_sub(1));
    let w = w.min(src.width().saturating_sub(x)).max(1);
    let h = h.min(src.height().saturating_sub(y)).max(1);
    image::imageops::crop_imm(src, x, y, w, h).to_image()
}

fn scale_rgb(src: &image::RgbImage, w: u32, h: u32) -> image::RgbImage {
    let w = w.max(1);
    let h = h.max(1);
    if src.width() == w && src.height() == h {
        return src.clone();
    }
    image::imageops::thumbnail(src, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_rect_covers_tighter_view() {
        let have = LodNeed {
            region_id: "a".into(),
            x: 10,
            y: 20,
            w: 100,
            h: 80,
            tex_w: 100,
            tex_h: 80,
            path: PathBuf::new(),
            flat: true,
            page_w: 1,
            page_h: 1,
            band_y: 0,
            band_h: 1,
            logical_w: 1,
            logical_h: 1,
        };
        let need = LodNeed {
            region_id: "a".into(),
            x: 20,
            y: 30,
            w: 40,
            h: 40,
            tex_w: 40,
            tex_h: 40,
            ..have.clone()
        };
        assert!(have.covers(&need));
        let other = LodNeed {
            region_id: "b".into(),
            ..need.clone()
        };
        assert!(!have.covers(&other));
    }

    #[test]
    fn band_maps_into_page() {
        let x0 = map_pos(0, 1000, 0, 2000, 2000);
        let x1 = map_pos(500, 1000, 0, 2000, 2000);
        let y0 = map_pos(0, 400, 1000, 400, 3000);
        let y1 = map_pos(400, 400, 1000, 400, 3000);
        assert_eq!(x0, 0);
        assert_eq!(x1, 1000);
        assert_eq!(y0, 1000);
        assert_eq!(y1, 1400);
    }
}
