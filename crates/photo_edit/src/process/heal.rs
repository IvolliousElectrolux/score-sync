//! 污点修复: 从附近自动选源的仿制图章.

use image::RgbaImage;

use super::brush::stamp_offset;

const RING: i32 = 2;
const DIRS: i32 = 16;
const DIST_MULT: [f32; 3] = [1.5, 2.5, 3.5];

fn luma(p: [u8; 4]) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

fn in_bounds(img: &RgbaImage, x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && x < img.width() as i32 && y < img.height() as i32
}

fn pixel(img: &RgbaImage, x: i32, y: i32) -> Option<[u8; 4]> {
    if !in_bounds(img, x, y) {
        return None;
    }
    Some(img.get_pixel(x as u32, y as u32).0)
}

fn for_ring(
    img: &RgbaImage,
    cx: i32,
    cy: i32,
    r_in: i32,
    r_out: i32,
    mut f: impl FnMut(i32, i32, [u8; 4]),
) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let r_in2 = r_in * r_in;
    let r_out2 = r_out * r_out;
    for y in (cy - r_out).max(0)..(cy + r_out + 1).min(h) {
        for x in (cx - r_out).max(0)..(cx + r_out + 1).min(w) {
            let d2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
            if d2 <= r_in2 || d2 > r_out2 {
                continue;
            }
            let p = img.get_pixel(x as u32, y as u32).0;
            if p[3] == 0 {
                continue;
            }
            f(x, y, p);
        }
    }
}

fn ring_sad(
    img: &RgbaImage,
    dx: i32,
    dy: i32,
    sx: i32,
    sy: i32,
    radius: i32,
) -> Option<(f32, u32)> {
    let r_in = radius;
    let r_out = radius + RING;
    let ox = sx - dx;
    let oy = sy - dy;
    let mut sad = 0f32;
    let mut n = 0u32;
    for_ring(img, dx, dy, r_in, r_out, |x, y, dp| {
        let Some(sp) = pixel(img, x + ox, y + oy) else {
            return;
        };
        if sp[3] == 0 {
            return;
        }
        sad += (dp[0] as f32 - sp[0] as f32).abs()
            + (dp[1] as f32 - sp[1] as f32).abs()
            + (dp[2] as f32 - sp[2] as f32).abs();
        n += 1;
    });
    if n < 8 {
        return None;
    }
    Some((sad, n))
}

fn ring_luma_stats(img: &RgbaImage, cx: i32, cy: i32, radius: i32) -> Option<(f32, f32)> {
    let mut acc = 0f32;
    let mut acc2 = 0f32;
    let mut n = 0f32;
    for_ring(img, cx, cy, radius, radius + RING, |_, _, p| {
        let l = luma(p);
        acc += l;
        acc2 += l * l;
        n += 1.0;
    });
    if n < 1.0 {
        return None;
    }
    let mean = acc / n;
    let var = (acc2 / n - mean * mean).max(0.0);
    Some((mean, var))
}

fn ring_edge_energy(img: &RgbaImage, cx: i32, cy: i32, radius: i32) -> f32 {
    let mut e = 0f32;
    let mut n = 0f32;
    for_ring(img, cx, cy, radius, radius + RING, |x, y, _| {
        let l1 = pixel(img, x + 1, y).map(luma).unwrap_or(0.0);
        let l0 = pixel(img, x - 1, y).map(luma).unwrap_or(0.0);
        let u1 = pixel(img, x, y + 1).map(luma).unwrap_or(0.0);
        let u0 = pixel(img, x, y - 1).map(luma).unwrap_or(0.0);
        e += (l1 - l0).abs() + (u1 - u0).abs();
        n += 1.0;
    });
    if n < 1.0 {
        0.0
    } else {
        e / n
    }
}

fn disk_overlap_too_close(ox: i32, oy: i32, radius: i32) -> bool {
    ox * ox + oy * oy < radius * radius
}

/// 在快照上为污点圆盘挑一个源偏移 (源中心 = 目标中心 + offset).
pub fn pick_heal_offset(snap: &RgbaImage, cx: i32, cy: i32, radius: i32) -> Option<(i32, i32)> {
    let radius = radius.max(1);
    let w = snap.width() as i32;
    let h = snap.height() as i32;
    if w < 2 || h < 2 {
        return None;
    }
    if !in_bounds(snap, cx, cy) {
        return None;
    }
    let dest_stats = ring_luma_stats(snap, cx, cy, radius);
    let dest_energy = ring_edge_energy(snap, cx, cy, radius);

    let mut best: Option<(i32, i32)> = None;
    let mut best_score = f32::MAX;

    for di in 0..DIRS {
        let ang = di as f32 * std::f32::consts::TAU / DIRS as f32;
        let ux = ang.cos();
        let uy = ang.sin();
        for &mult in &DIST_MULT {
            let dist = (mult * radius as f32).round().max(1.0);
            let ox = (ux * dist).round() as i32;
            let oy = (uy * dist).round() as i32;
            if ox == 0 && oy == 0 {
                continue;
            }
            if disk_overlap_too_close(ox, oy, radius) {
                continue;
            }
            let sx = cx + ox;
            let sy = cy + oy;
            let Some(sp) = pixel(snap, sx, sy) else {
                continue;
            };
            if sp[3] == 0 {
                continue;
            }
            let Some((sad, n)) = ring_sad(snap, cx, cy, sx, sy, radius) else {
                continue;
            };
            let mut score = sad / n as f32;
            if let Some((paper, var)) = dest_stats {
                if var < 400.0 {
                    score += (paper - luma(sp)).max(0.0) * 0.4;
                }
            }
            let src_energy = ring_edge_energy(snap, sx, sy, radius);
            score += (dest_energy - src_energy).abs() * 0.35;
            if score < best_score {
                best_score = score;
                best = Some((ox, oy));
            }
        }
    }
    best.or_else(|| fallback_offset(snap, cx, cy, radius))
}

fn fallback_offset(snap: &RgbaImage, cx: i32, cy: i32, radius: i32) -> Option<(i32, i32)> {
    let step = radius + RING + 1;
    const CAND: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    for (ux, uy) in CAND {
        let ox = ux * step;
        let oy = uy * step;
        let sx = cx + ox;
        let sy = cy + oy;
        if pixel(snap, sx, sy).is_some_and(|p| p[3] > 0) {
            return Some((ox, oy));
        }
    }
    None
}

/// 用锁定的源偏移在圆盘上盖章. 不接纸色: 环带若含油墨, 均值会把补丁拉灰.
pub fn heal_disk(
    dst: &mut RgbaImage,
    snap: &RgbaImage,
    cx: i32,
    cy: i32,
    radius: i32,
    hardness: f32,
    offset: (i32, i32),
) {
    let (ox, oy) = offset;
    let sx = cx + ox;
    let sy = cy + oy;
    if !in_bounds(snap, sx, sy) {
        return;
    }
    stamp_offset(dst, snap, cx, cy, sx, sy, radius, hardness, [0.0; 3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn paper(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([210, 210, 210, 255]))
    }

    #[test]
    fn heal_fills_hole_from_neighbors() {
        let mut img = paper(16, 16);
        img.put_pixel(8, 8, Rgba([0, 0, 0, 255]));
        let snap = img.clone();
        let off = pick_heal_offset(&snap, 8, 8, 2).expect("offset");
        heal_disk(&mut img, &snap, 8, 8, 2, 1.0, off);
        let p = img.get_pixel(8, 8);
        assert!(p[0] > 100, "healed {}", p[0]);
        assert_eq!(p[3], 255);
    }

    #[test]
    fn heal_keeps_staff_line() {
        let mut img = paper(48, 48);
        for x in 0..48 {
            img.put_pixel(x, 24, Rgba([25, 25, 25, 255]));
        }
        for y in 21..28 {
            for x in 21..28 {
                if y != 24 {
                    img.put_pixel(x as u32, y as u32, Rgba([40, 40, 40, 255]));
                }
            }
        }
        let snap = img.clone();
        let off = pick_heal_offset(&snap, 24, 24, 5).expect("offset");
        heal_disk(&mut img, &snap, 24, 24, 5, 0.85, off);
        let line = img.get_pixel(24, 24);
        assert!(line[0] < 90, "line stayed dark {}", line[0]);
        let paper_px = img.get_pixel(24, 21);
        assert!(paper_px[0] > 140, "stain became paper {}", paper_px[0]);
    }

    #[test]
    fn heal_reads_snap_not_dst() {
        let snap = paper(32, 32);
        let mut dst = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 255]));
        heal_disk(&mut dst, &snap, 16, 16, 4, 1.0, (8, 0));
        let p = dst.get_pixel(16, 16);
        assert!(p[0] > 180, "cloned from snap {}", p[0]);
    }

    #[test]
    fn heal_does_not_gray_when_ring_has_ink() {
        let mut img = RgbaImage::from_pixel(64, 64, Rgba([250, 250, 250, 255]));
        for y in 28..37 {
            for x in 28..37 {
                img.put_pixel(x, y, Rgba([12, 12, 12, 255]));
            }
        }
        for x in 16..48 {
            img.put_pixel(x, 42, Rgba([8, 8, 8, 255]));
        }
        let snap = img.clone();
        heal_disk(&mut img, &snap, 32, 32, 8, 1.0, (20, 0));
        let p = img.get_pixel(32, 32);
        assert!(p[0] > 230, "paper clone must stay white, got gray {}", p[0]);
    }

    #[test]
    fn heal_empty_source_does_not_panic() {
        let snap = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        let mut dst = snap.clone();
        assert!(pick_heal_offset(&snap, 4, 4, 2).is_none());
        heal_disk(&mut dst, &snap, 4, 4, 2, 1.0, (3, 0));
    }
}
