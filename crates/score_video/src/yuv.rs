//! RGB → YUV420, 系数对齐裁剪版 ffmpeg 的 `format=yuv420p`.
//!
//! 8x8 到 2560x1440 的纯色样本都落在 BT.601 有限范围 (全范围 RGB 进去,
//! Y 16..235). 平坦色块和现有 swscale 逐字节一致; 4:2:0 色度用 2x2 平均,
//! 只在刷入/交叠的色度高频边上和 swscale 的滤波差一两级.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Yuv420 {
    /// `yuv420p`: Y, 然后 U, 然后 V.
    I420,
    /// `nv12`: Y, 然后交错的 UV. Quick Sync / Media Foundation 吃这个.
    Nv12,
}

impl Yuv420 {
    pub(crate) fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::I420 => "yuv420p",
            Self::Nv12 => "nv12",
        }
    }

    pub(crate) fn byte_len(self, w: u32, h: u32) -> usize {
        (w as usize).saturating_mul(h as usize).saturating_mul(3) / 2
    }
}

struct Coeffs {
    kr: i64,
    kg: i64,
    kb: i64,
    denom: i64,
    u_den: i64,
    v_den: i64,
}

impl Coeffs {
    fn bt601() -> Self {
        const SCALE: f64 = 65536.0;
        Self {
            kr: (0.299 * SCALE).round() as i64,
            kg: (0.587 * SCALE).round() as i64,
            kb: (0.114 * SCALE).round() as i64,
            denom: 255 * (SCALE as i64),
            u_den: (255.0 * 2.0 * (1.0 - 0.114) * SCALE).round() as i64,
            v_den: (255.0 * 2.0 * (1.0 - 0.299) * SCALE).round() as i64,
        }
    }

    fn map(&self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let r = i64::from(r);
        let g = i64::from(g);
        let b = i64::from(b);
        let yf = self.kr * r + self.kg * g + self.kb * b;
        let y = (16 * self.denom + 219 * yf + self.denom / 2) / self.denom;
        let u = 128 + div_round(224 * (b * (self.denom / 255) - yf), self.u_den);
        let v = 128 + div_round(224 * (r * (self.denom / 255) - yf), self.v_den);
        (clamp_u8(y), clamp_u8(u), clamp_u8(v))
    }
}

fn div_round(n: i64, d: i64) -> i64 {
    if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    }
}

fn clamp_u8(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

/// `src` 是紧密 RGBA. `dst` 会被调成目标平面大小.
pub(crate) fn rgba_to_yuv420(src: &[u8], w: u32, h: u32, layout: Yuv420, dst: &mut Vec<u8>) {
    let w = w as usize;
    let h = h as usize;
    let y_size = w.saturating_mul(h);
    let cw = w / 2;
    let ch = h / 2;
    let len = layout.byte_len(w as u32, h as u32);
    dst.clear();
    dst.resize(len, 0);
    if w == 0 || h == 0 || cw == 0 || src.len() < y_size.saturating_mul(4) {
        return;
    }
    let coeff = Coeffs::bt601();
    let mut acc_u = vec![0i32; cw];
    let mut acc_v = vec![0i32; cw];
    for by in 0..ch {
        acc_u.fill(0);
        acc_v.fill(0);
        for row in 0..2 {
            let y = by * 2 + row;
            let row_off = y * w * 4;
            for x in 0..w {
                let i = row_off + x * 4;
                let (yy, uu, vv) = coeff.map(src[i], src[i + 1], src[i + 2]);
                dst[y * w + x] = yy;
                acc_u[x / 2] += i32::from(uu);
                acc_v[x / 2] += i32::from(vv);
            }
        }
        match layout {
            Yuv420::I420 => {
                let u_base = y_size + by * cw;
                let v_base = y_size + cw * ch + by * cw;
                for x in 0..cw {
                    dst[u_base + x] = ((acc_u[x] + 2) / 4) as u8;
                    dst[v_base + x] = ((acc_v[x] + 2) / 4) as u8;
                }
            }
            Yuv420::Nv12 => {
                let uv = y_size + by * cw * 2;
                for x in 0..cw {
                    dst[uv + x * 2] = ((acc_u[x] + 2) / 4) as u8;
                    dst[uv + x * 2 + 1] = ((acc_v[x] + 2) / 4) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pix(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        Coeffs::bt601().map(r, g, b)
    }

    #[test]
    fn solid_colors_match_ffmpeg_format_yuv420p() {
        assert_eq!(pix(0, 0, 0), (16, 128, 128));
        assert_eq!(pix(255, 255, 255), (235, 128, 128));
        assert_eq!(pix(255, 0, 0), (81, 90, 240));
        assert_eq!(pix(0, 255, 0), (145, 54, 34));
        assert_eq!(pix(0, 0, 255), (41, 240, 110));
        assert_eq!(pix(128, 128, 128), (126, 128, 128));
        assert_eq!(pix(250, 248, 240), (229, 124, 129));
    }

    #[test]
    fn flat_2x2_chroma_is_the_pixel_uv() {
        let mut rgba = vec![0u8; 2 * 2 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let mut dst = Vec::new();
        rgba_to_yuv420(&rgba, 2, 2, Yuv420::I420, &mut dst);
        assert_eq!(dst.len(), 6);
        assert_eq!(&dst[..4], &[81, 81, 81, 81]);
        assert_eq!(dst[4], 90);
        assert_eq!(dst[5], 240);

        rgba_to_yuv420(&rgba, 2, 2, Yuv420::Nv12, &mut dst);
        assert_eq!(dst.len(), 6);
        assert_eq!(&dst[4..], &[90, 240]);
    }
}
