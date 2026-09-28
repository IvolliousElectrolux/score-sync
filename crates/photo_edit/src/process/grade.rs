//! 底色 / 图层的滤镜和影调. 点运算相对一张基准图, 方便拖动条来回拉.

use image::RgbaImage;

/// 与界面拖动条下标一致: 亮度, 对比度, 饱和度, 色温, 色调, 高光, 阴影.
pub const TONE_LABELS: [&str; 7] = ["亮度", "对比度", "饱和度", "色温", "色调", "高光", "阴影"];

/// 与滤镜拖动条下标一致: 灰度, 棕褐, 锐化, 模糊, 褪色, 暗角.
/// 都是 0 为原图, 向右加深. 灰度和棕褐到 1 即满. 锐化, 模糊, 褪色, 暗角的右端
/// 比原先点一下更强, 相当于连续点了几次.
pub const FILTER_LABELS: [&str; 6] = ["灰度", "棕褐", "锐化", "模糊", "褪色", "暗角"];

/// 锐化拖到最右时的反差量. 原先点一下是 0.85.
const SHARPEN_MAX: f32 = 2.55;
/// 模糊拖到最右时的半径 (像素). 原先点一下是半径 2.
const BLUR_MAX: f32 = 16.0;
/// 褪色拖到最右时, 相当于连续套用旧公式的次数.
const FADE_MAX_STEPS: f32 = 3.0;
/// 暗角拖到最右时, 相对旧公式的倍数.
const VIGNETTE_MAX: f32 = 1.8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tone {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub highlights: f32,
    pub shadows: f32,
}

impl Tone {
    pub fn is_neutral(self) -> bool {
        self.brightness.abs() < 1.0e-4
            && self.contrast.abs() < 1.0e-4
            && self.saturation.abs() < 1.0e-4
            && self.temperature.abs() < 1.0e-4
            && self.tint.abs() < 1.0e-4
            && self.highlights.abs() < 1.0e-4
            && self.shadows.abs() < 1.0e-4
    }

    pub fn slot(self, index: usize) -> f32 {
        match index {
            0 => self.brightness,
            1 => self.contrast,
            2 => self.saturation,
            3 => self.temperature,
            4 => self.tint,
            5 => self.highlights,
            _ => self.shadows,
        }
    }

    pub fn set_slot(&mut self, index: usize, value: f32) {
        let value = value.clamp(-1.0, 1.0);
        match index {
            0 => self.brightness = value,
            1 => self.contrast = value,
            2 => self.saturation = value,
            3 => self.temperature = value,
            4 => self.tint = value,
            5 => self.highlights = value,
            _ => self.shadows = value,
        }
    }
}

/// 可叠加的滤镜强度. 0 为原图, 1 为这一项的最深.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FilterAmt {
    pub gray: f32,
    pub sepia: f32,
    pub sharpen: f32,
    pub blur: f32,
    pub fade: f32,
    pub vignette: f32,
}

impl FilterAmt {
    pub fn is_neutral(self) -> bool {
        self.point_neutral() && !self.has_spatial()
    }

    pub fn point_neutral(self) -> bool {
        self.gray.abs() < 1.0e-4
            && self.sepia.abs() < 1.0e-4
            && self.fade.abs() < 1.0e-4
            && self.vignette.abs() < 1.0e-4
    }

    pub fn has_spatial(self) -> bool {
        self.sharpen.abs() >= 1.0e-4 || self.blur.abs() >= 1.0e-4
    }

    pub fn slot(self, index: usize) -> f32 {
        match index {
            0 => self.gray,
            1 => self.sepia,
            2 => self.sharpen,
            3 => self.blur,
            4 => self.fade,
            _ => self.vignette,
        }
    }

    pub fn set_slot(&mut self, index: usize, value: f32) {
        let value = value.clamp(0.0, 1.0);
        match index {
            0 => self.gray = value,
            1 => self.sepia = value,
            2 => self.sharpen = value,
            3 => self.blur = value,
            4 => self.fade = value,
            _ => self.vignette = value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    AutoLevels,
    Grayscale,
    Invert,
    Sepia,
    Sharpen,
    Blur,
    Fade,
    Vignette,
    GrayWorld,
}

impl Look {
    /// 点一下就定型, 再点不会按比例加深.
    pub const ONCE: [Look; 2] = [Look::AutoLevels, Look::Invert];

    pub fn id(self) -> &'static str {
        match self {
            Look::AutoLevels => "look-levels",
            Look::Grayscale => "look-gray",
            Look::Invert => "look-invert",
            Look::Sepia => "look-sepia",
            Look::Sharpen => "look-sharp",
            Look::Blur => "look-blur",
            Look::Fade => "look-fade",
            Look::Vignette => "look-vignette",
            Look::GrayWorld => "look-wb",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Look::AutoLevels => "自动色阶",
            Look::Grayscale => "灰度",
            Look::Invert => "反相",
            Look::Sepia => "棕褐",
            Look::Sharpen => "锐化",
            Look::Blur => "模糊",
            Look::Fade => "褪色",
            Look::Vignette => "暗角",
            Look::GrayWorld => "自动白平衡",
        }
    }
}

/// 图层像素在画布上的位置. 旋转为 0 时只做平移.
#[derive(Clone, Copy, Debug)]
pub struct PixelSpace {
    pub ox: i32,
    pub oy: i32,
    pub rot: f32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy)]
pub enum PixelGate<'a> {
    All,
    /// 画布坐标, 含边界.
    Rect {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
    },
    Mask {
        data: &'a [u8],
        canvas_w: u32,
    },
}

/// 把 `base` 按影调写入 `img`. 两者尺寸必须相同, 且不要是同一块缓冲.
pub fn apply_tone(
    img: &mut RgbaImage,
    base: &RgbaImage,
    tone: Tone,
    space: PixelSpace,
    gate: PixelGate<'_>,
) {
    apply_grade(img, base, tone, FilterAmt::default(), space, gate);
}

/// 影调和滤镜强度一起相对 `base` 写入 `img`. 拖动条来回拉时都从这张基准算.
pub fn apply_grade(
    img: &mut RgbaImage,
    base: &RgbaImage,
    tone: Tone,
    filter: FilterAmt,
    space: PixelSpace,
    gate: PixelGate<'_>,
) {
    if img.dimensions() != base.dimensions() {
        return;
    }
    if std::ptr::eq(img.as_raw().as_ptr(), base.as_raw().as_ptr()) {
        if tone.is_neutral() && filter.is_neutral() {
            return;
        }
        let copy = base.clone();
        apply_grade(img, &copy, tone, filter, space, gate);
        return;
    }
    if tone.is_neutral() && filter.point_neutral() {
        let dst: &mut [u8] = img;
        dst.copy_from_slice(base.as_raw());
        if filter.has_spatial() {
            apply_spatial_scaled(img, filter, 1.0, &space, gate, false);
        }
        return;
    }
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        return;
    }
    let kernel = ToneKernel::from(tone);
    let fast = fast_gate(gate, &space, w, h);
    let src = base.as_raw();
    let dst: &mut [u8] = img;
    let rad = space.rot.to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());
    grade_rows_parallel(dst, w, h, |rows, y0, y1| {
        grade_span(
            rows, src, w, y0, y1, kernel, filter, space, gate, fast, cos, sin,
        );
    });
    if filter.has_spatial() {
        apply_spatial_scaled(img, filter, 1.0, &space, gate, false);
    }
}

/// 把图层矩形 `[src_x, src_y, src_w, src_h)` 采样成 `out_w × out_h` 并套上影调.
/// 拖动条用它生成屏幕大小的预览, 避免每动一下就改全图.
pub fn render_tone_preview(
    base: &RgbaImage,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
    tone: Tone,
    space: PixelSpace,
    gate: PixelGate<'_>,
) -> RgbaImage {
    render_grade_preview(
        base,
        src_x,
        src_y,
        src_w,
        src_h,
        out_w,
        out_h,
        tone,
        FilterAmt::default(),
        space,
        gate,
    )
}

/// 同 `render_tone_preview`, 并带上滤镜强度. 模糊和锐化按缩小比例换算半径.
pub fn render_grade_preview(
    base: &RgbaImage,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
    tone: Tone,
    filter: FilterAmt,
    space: PixelSpace,
    gate: PixelGate<'_>,
) -> RgbaImage {
    let bw = base.width();
    let bh = base.height();
    if bw == 0 || bh == 0 {
        return RgbaImage::new(1, 1);
    }
    let src_x = src_x.min(bw - 1);
    let src_y = src_y.min(bh - 1);
    let src_w = src_w.max(1).min(bw - src_x);
    let src_h = src_h.max(1).min(bh - src_y);
    let (out_w, out_h) = spatial_preview_size(out_w.max(1), out_h.max(1), filter);
    let mut out = RgbaImage::new(out_w, out_h);
    let kernel = ToneKernel::from(tone);
    let neutral = tone.is_neutral();
    let fast = fast_gate(gate, &space, bw, bh);
    let src = base.as_raw();
    let dst: &mut [u8] = &mut out;
    let rad = space.rot.to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());
    grade_rows_parallel(dst, out_w, out_h, |rows, y0, y1| {
        preview_span(
            rows, src, bw, src_x, src_y, src_w, src_h, out_w, out_h, y0, y1, kernel, filter,
            neutral, space, gate, fast, cos, sin,
        );
    });
    if filter.has_spatial() {
        let cover = fast_gate(gate, &space, space.w.max(1), space.h.max(1));
        if !matches!(cover, FastGate::None) {
            let scale =
                (out_w as f32 / src_w.max(1) as f32).min(out_h as f32 / src_h.max(1) as f32);
            let dummy = PixelSpace {
                ox: 0,
                oy: 0,
                rot: 0.0,
                w: out_w,
                h: out_h,
            };
            if matches!(cover, FastGate::All) {
                apply_spatial_scaled(&mut out, filter, scale, &dummy, PixelGate::All, true);
            } else {
                let before = out.clone();
                apply_spatial_scaled(&mut out, filter, scale, &dummy, PixelGate::All, true);
                mix_preview_gate(&mut out, &before, src_x, src_y, src_w, src_h, space, gate);
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
struct ToneKernel {
    temp_r: f32,
    temp_g: f32,
    temp_b: f32,
    bright: f32,
    hi_k: f32,
    sh_k: f32,
    scale: f32,
    sat: f32,
    /// 高光, 阴影, 饱和度都是 0: 每个通道只做一次加法和对比度.
    simple: bool,
}

impl ToneKernel {
    fn from(tone: Tone) -> Self {
        let scale = if tone.contrast >= 0.0 {
            1.0 + tone.contrast * 1.8
        } else {
            (1.0 + tone.contrast).max(0.0)
        };
        let sat = (1.0 + tone.saturation).max(0.0);
        let hi_k = tone.highlights * 90.0;
        let sh_k = tone.shadows * 90.0;
        let simple = hi_k.abs() < 1.0e-6 && sh_k.abs() < 1.0e-6 && (sat - 1.0).abs() < 1.0e-6;
        Self {
            temp_r: tone.temperature * 42.0 + tone.tint * 16.0,
            temp_g: tone.temperature * 6.0 - tone.tint * 32.0,
            temp_b: -tone.temperature * 46.0 + tone.tint * 20.0,
            bright: tone.brightness * 160.0,
            hi_k,
            sh_k,
            scale,
            sat,
            simple,
        }
    }
}

#[derive(Clone, Copy)]
enum FastGate {
    All,
    /// 图层坐标, 含边界. 没有相交时为 None, 调用方整图拷贝.
    Rect {
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    },
    None,
    Slow,
}

fn fast_gate(gate: PixelGate<'_>, space: &PixelSpace, w: u32, h: u32) -> FastGate {
    if w == 0 || h == 0 {
        return FastGate::None;
    }
    match gate {
        PixelGate::All => FastGate::All,
        PixelGate::Rect { x0, y0, x1, y1 } if space.rot.abs() < 0.05 => {
            match (
                axis_span(x0, x1, space.ox, w),
                axis_span(y0, y1, space.oy, h),
            ) {
                (Some((ax0, ax1)), Some((ay0, ay1))) => FastGate::Rect {
                    x0: ax0,
                    y0: ay0,
                    x1: ax1,
                    y1: ay1,
                },
                _ => FastGate::None,
            }
        }
        _ => FastGate::Slow,
    }
}

fn axis_span(a: i32, b: i32, origin: i32, len: u32) -> Option<(u32, u32)> {
    let lo = a.min(b) as i64 - origin as i64;
    let hi = a.max(b) as i64 - origin as i64;
    let last = len as i64 - 1;
    if hi < 0 || lo > last {
        return None;
    }
    Some((lo.max(0) as u32, hi.min(last) as u32))
}

fn grade_threads(pixels: usize) -> usize {
    if pixels < 65_536 {
        return 1;
    }
    let n = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
        .clamp(1, 8);
    (pixels / 65_536).clamp(1, n)
}

fn grade_rows_parallel(dst: &mut [u8], w: u32, h: u32, f: impl Fn(&mut [u8], u32, u32) + Sync) {
    let stride = w as usize * 4;
    let threads = grade_threads((w as usize).saturating_mul(h as usize));
    if threads == 1 || h < 2 {
        f(dst, 0, h);
        return;
    }
    let chunk = (h as usize).div_ceil(threads);
    let mut rest = dst;
    let mut parts = Vec::new();
    let mut y = 0u32;
    while y < h {
        let rows = ((h - y) as usize).min(chunk);
        let (head, tail) = rest.split_at_mut(rows * stride);
        parts.push((y, y + rows as u32, head));
        rest = tail;
        y += rows as u32;
    }
    let f = &f;
    std::thread::scope(|scope| {
        for (y0, y1, part) in parts {
            scope.spawn(move || f(part, y0, y1));
        }
    });
}

fn grade_span(
    dst: &mut [u8],
    src: &[u8],
    w: u32,
    y0: u32,
    y1: u32,
    kernel: ToneKernel,
    filter: FilterAmt,
    space: PixelSpace,
    gate: PixelGate<'_>,
    fast: FastGate,
    cos: f32,
    sin: f32,
) {
    if matches!(fast, FastGate::None) {
        let n = ((y1 - y0) as usize) * (w as usize) * 4;
        let s0 = (y0 as usize) * (w as usize) * 4;
        dst[..n].copy_from_slice(&src[s0..s0 + n]);
        return;
    }
    let row = w as usize * 4;
    for y in y0..y1 {
        let local = (y - y0) as usize;
        let s0 = y as usize * row;
        let srow = &src[s0..s0 + row];
        let drow = &mut dst[local * row..local * row + row];
        if let FastGate::Rect {
            y0: gy0, y1: gy1, ..
        } = fast
        {
            if y < gy0 || y > gy1 {
                drow.copy_from_slice(srow);
                continue;
            }
        }
        if matches!(fast, FastGate::All) && kernel.simple && filter.point_neutral() {
            affine_row(drow, srow, kernel);
            continue;
        }
        for x in 0..w {
            let i = x as usize * 4;
            let base = [srow[i], srow[i + 1], srow[i + 2]];
            drow[i + 3] = srow[i + 3];
            if srow[i + 3] == 0 {
                drow[i] = base[0];
                drow[i + 1] = base[1];
                drow[i + 2] = base[2];
                continue;
            }
            let m = gate_mix(fast, gate, &space, x, y, cos, sin);
            if m == 0 {
                drow[i] = base[0];
                drow[i + 1] = base[1];
                drow[i + 2] = base[2];
                continue;
            }
            let neu = map_rgb(base[0], base[1], base[2], kernel);
            let neu = paint_point(neu, x, y, space.w, space.h, filter);
            if m == 255 {
                drow[i] = neu[0];
                drow[i + 1] = neu[1];
                drow[i + 2] = neu[2];
            } else {
                write_mixed(&mut drow[i..i + 3], base, neu, m);
            }
        }
    }
}

fn affine_row(dst: &mut [u8], src: &[u8], k: ToneKernel) {
    let add_r = k.temp_r + k.bright;
    let add_g = k.temp_g + k.bright;
    let add_b = k.temp_b + k.bright;
    let scale = k.scale;
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        d[3] = s[3];
        if s[3] == 0 {
            d[0] = s[0];
            d[1] = s[1];
            d[2] = s[2];
            continue;
        }
        d[0] = affine_u8(s[0], add_r, scale);
        d[1] = affine_u8(s[1], add_g, scale);
        d[2] = affine_u8(s[2], add_b, scale);
    }
}

#[inline(always)]
fn affine_u8(c: u8, add: f32, scale: f32) -> u8 {
    u8f((c as f32 + add - 128.0) * scale + 128.0)
}

fn preview_span(
    dst: &mut [u8],
    src: &[u8],
    src_stride_w: u32,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
    y0: u32,
    y1: u32,
    kernel: ToneKernel,
    filter: FilterAmt,
    neutral: bool,
    space: PixelSpace,
    gate: PixelGate<'_>,
    fast: FastGate,
    cos: f32,
    sin: f32,
) {
    let sw = src_stride_w as usize;
    let row = out_w as usize * 4;
    for y in y0..y1 {
        let sy = src_y + map_axis(y, out_h, src_h);
        let sbase = sy as usize * sw * 4;
        let drow = &mut dst[((y - y0) as usize) * row..((y - y0) as usize) * row + row];
        for x in 0..out_w {
            let sx = src_x + map_axis(x, out_w, src_w);
            let i = sbase + sx as usize * 4;
            let o = x as usize * 4;
            let pix = [src[i], src[i + 1], src[i + 2], src[i + 3]];
            drow[o + 3] = pix[3];
            if pix[3] == 0 || (neutral && filter.point_neutral()) {
                drow[o] = pix[0];
                drow[o + 1] = pix[1];
                drow[o + 2] = pix[2];
                continue;
            }
            let m = gate_mix(fast, gate, &space, sx, sy, cos, sin);
            if m == 0 {
                drow[o] = pix[0];
                drow[o + 1] = pix[1];
                drow[o + 2] = pix[2];
                continue;
            }
            let neu = map_rgb(pix[0], pix[1], pix[2], kernel);
            let neu = paint_point(neu, sx, sy, space.w, space.h, filter);
            if m == 255 {
                drow[o] = neu[0];
                drow[o + 1] = neu[1];
                drow[o + 2] = neu[2];
            } else {
                write_mixed(&mut drow[o..o + 3], [pix[0], pix[1], pix[2]], neu, m);
            }
        }
    }
}

#[inline]
fn map_axis(i: u32, out_n: u32, src_n: u32) -> u32 {
    if out_n <= 1 || src_n <= 1 {
        return 0;
    }
    ((i as u64 * (src_n as u64 - 1)) / (out_n as u64 - 1)) as u32
}

#[inline]
fn gate_mix(
    fast: FastGate,
    gate: PixelGate<'_>,
    space: &PixelSpace,
    x: u32,
    y: u32,
    cos: f32,
    sin: f32,
) -> u8 {
    match fast {
        FastGate::All => 255,
        FastGate::None => 0,
        FastGate::Rect { x0, y0, x1, y1 } => {
            if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                255
            } else {
                0
            }
        }
        FastGate::Slow => {
            let (cx, cy) = if space.rot.abs() < 0.05 {
                (
                    space.ox.saturating_add(x as i32),
                    space.oy.saturating_add(y as i32),
                )
            } else {
                let px = space.ox as f32 + x as f32 + 0.5;
                let py = space.oy as f32 + y as f32 + 0.5;
                let pcx = space.ox as f32 + space.w as f32 * 0.5;
                let pcy = space.oy as f32 + space.h as f32 * 0.5;
                let dx = px - pcx;
                let dy = py - pcy;
                (
                    (pcx + dx * cos - dy * sin).floor() as i32,
                    (pcy + dx * sin + dy * cos).floor() as i32,
                )
            };
            match gate {
                PixelGate::All => 255,
                PixelGate::Rect { x0, y0, x1, y1 } => {
                    if cx >= x0 && cy >= y0 && cx <= x1 && cy <= y1 {
                        255
                    } else {
                        0
                    }
                }
                PixelGate::Mask { data, canvas_w } => {
                    if cx < 0 || cy < 0 || canvas_w == 0 {
                        return 0;
                    }
                    let i = cy as usize * canvas_w as usize + cx as usize;
                    data.get(i).copied().unwrap_or(0)
                }
            }
        }
    }
}

#[inline(always)]
fn map_rgb(r: u8, g: u8, b: u8, k: ToneKernel) -> [u8; 3] {
    if k.simple {
        return [
            affine_u8(r, k.temp_r + k.bright, k.scale),
            affine_u8(g, k.temp_g + k.bright, k.scale),
            affine_u8(b, k.temp_b + k.bright, k.scale),
        ];
    }
    let y = luma_f(r, g, b);
    let mut rgb = [
        r as f32 + k.temp_r,
        g as f32 + k.temp_g,
        b as f32 + k.temp_b,
    ];
    let hi = ((y - 140.0) / 100.0).clamp(0.0, 1.0);
    let sh = ((140.0 - y) / 110.0).clamp(0.0, 1.0);
    let lift = k.hi_k * hi + k.sh_k * sh + k.bright;
    for c in &mut rgb {
        *c += lift;
    }
    for c in &mut rgb {
        *c = (*c - 128.0) * k.scale + 128.0;
    }
    if (k.sat - 1.0).abs() < 1.0e-6 {
        return [u8f(rgb[0]), u8f(rgb[1]), u8f(rgb[2])];
    }
    let y2 = (0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]).clamp(0.0, 255.0);
    for c in &mut rgb {
        *c = u8f(y2 + (*c - y2) * k.sat) as f32;
    }
    [rgb[0] as u8, rgb[1] as u8, rgb[2] as u8]
}

pub fn apply_look(img: &mut RgbaImage, look: Look, space: PixelSpace, gate: PixelGate<'_>) {
    if img.width() == 0 || img.height() == 0 {
        return;
    }
    match look {
        Look::AutoLevels => auto_levels(img, &space, gate),
        Look::Grayscale => map_point(img, &space, gate, |r, g, b, _, _, _, _| {
            let y = luma_u8(r, g, b);
            [y, y, y]
        }),
        Look::Invert => map_point(img, &space, gate, |r, g, b, _, _, _, _| {
            [255 - r, 255 - g, 255 - b]
        }),
        Look::Sepia => map_point(img, &space, gate, |r, g, b, _, _, _, _| sepia(r, g, b)),
        Look::Fade => map_point(img, &space, gate, |r, g, b, _, _, _, _| {
            [fade(r), fade(g), fade(b)]
        }),
        Look::Vignette => map_point(img, &space, gate, |r, g, b, x, y, w, h| {
            vignette(r, g, b, x, y, w, h)
        }),
        Look::Sharpen => sharpen(img, &space, gate),
        Look::Blur => {
            let blurred = box_blur(img, 2);
            blend_toward(img, &blurred, &space, gate);
        }
        Look::GrayWorld => gray_world(img, &space, gate),
    }
}

fn map_point(
    img: &mut RgbaImage,
    space: &PixelSpace,
    gate: PixelGate<'_>,
    f: impl Fn(u8, u8, u8, u32, u32, u32, u32) -> [u8; 3],
) {
    let w = img.width();
    let h = img.height();
    let all = matches!(gate, PixelGate::All);
    let raw: &mut [u8] = img;
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if raw[i + 3] == 0 {
                continue;
            }
            let base = [raw[i], raw[i + 1], raw[i + 2]];
            let m = if all {
                255
            } else {
                gate_amount(gate, space, x, y)
            };
            if m == 0 {
                continue;
            }
            let neu = f(base[0], base[1], base[2], x, y, w, h);
            write_mixed(&mut raw[i..i + 3], base, neu, m);
        }
    }
}

fn blend_toward(img: &mut RgbaImage, src: &RgbaImage, space: &PixelSpace, gate: PixelGate<'_>) {
    if img.dimensions() != src.dimensions() {
        return;
    }
    let w = img.width();
    let h = img.height();
    let from = src.as_raw();
    let all = matches!(gate, PixelGate::All);
    let raw: &mut [u8] = img;
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if raw[i + 3] == 0 {
                continue;
            }
            let base = [raw[i], raw[i + 1], raw[i + 2]];
            let m = if all {
                255
            } else {
                gate_amount(gate, space, x, y)
            };
            if m == 0 {
                continue;
            }
            let neu = [from[i], from[i + 1], from[i + 2]];
            write_mixed(&mut raw[i..i + 3], base, neu, m);
        }
    }
}

fn sharpen(img: &mut RgbaImage, space: &PixelSpace, gate: PixelGate<'_>) {
    let blurred = box_blur(img, 1);
    let from = blurred.as_raw();
    let w = img.width();
    let h = img.height();
    let all = matches!(gate, PixelGate::All);
    let raw: &mut [u8] = img;
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if raw[i + 3] == 0 {
                continue;
            }
            let base = [raw[i], raw[i + 1], raw[i + 2]];
            let m = if all {
                255
            } else {
                gate_amount(gate, space, x, y)
            };
            if m == 0 {
                continue;
            }
            let neu = [
                unsharp(base[0], from[i]),
                unsharp(base[1], from[i + 1]),
                unsharp(base[2], from[i + 2]),
            ];
            write_mixed(&mut raw[i..i + 3], base, neu, m);
        }
    }
}

fn unsharp(src: u8, blur: u8) -> u8 {
    unsharp_k(src, blur, 0.85)
}

fn unsharp_k(src: u8, blur: u8, amount: f32) -> u8 {
    u8f(src as f32 + (src as f32 - blur as f32) * amount)
}

fn paint_point(rgb: [u8; 3], x: u32, y: u32, fw: u32, fh: u32, f: FilterAmt) -> [u8; 3] {
    if f.point_neutral() {
        return rgb;
    }
    let mut r = rgb[0];
    let mut g = rgb[1];
    let mut b = rgb[2];
    if f.fade > 1.0e-4 {
        let steps = f.fade * FADE_MAX_STEPS;
        r = fade_steps(r, steps);
        g = fade_steps(g, steps);
        b = fade_steps(b, steps);
    }
    if f.sepia > 1.0e-4 {
        let s = sepia(r, g, b);
        r = mix_u8(r, s[0], f.sepia);
        g = mix_u8(g, s[1], f.sepia);
        b = mix_u8(b, s[2], f.sepia);
    }
    if f.gray > 1.0e-4 {
        let yv = luma_u8(r, g, b);
        r = mix_u8(r, yv, f.gray);
        g = mix_u8(g, yv, f.gray);
        b = mix_u8(b, yv, f.gray);
    }
    if f.vignette > 1.0e-4 {
        let scale = vignette_scale(x, y, fw, fh, f.vignette);
        r = u8f(r as f32 * scale);
        g = u8f(g as f32 * scale);
        b = u8f(b as f32 * scale);
    }
    [r, g, b]
}

fn mix_u8(a: u8, b: u8, t: f32) -> u8 {
    u8f(a as f32 + (b as f32 - a as f32) * t)
}

fn fade_steps(c: u8, steps: f32) -> u8 {
    if steps <= 1.0e-4 {
        return c;
    }
    let n = steps.floor() as i32;
    let frac = steps - n as f32;
    let mut v = c as f32;
    for _ in 0..n {
        v = v * 0.78 + 32.0;
    }
    if frac > 1.0e-4 {
        let next = v * 0.78 + 32.0;
        v += (next - v) * frac;
    }
    u8f(v)
}

fn vignette_scale(x: u32, y: u32, w: u32, h: u32, amount: f32) -> f32 {
    let w = w.max(1) as f32;
    let h = h.max(1) as f32;
    let nx = (x as f32 + 0.5) / w * 2.0 - 1.0;
    let ny = (y as f32 + 0.5) / h * 2.0 - 1.0;
    let d = (nx * nx * 0.85 + ny * ny).sqrt();
    let k = ((d - 0.55) / 0.75).clamp(0.0, 1.0);
    let strength = amount.clamp(0.0, 1.0) * VIGNETTE_MAX;
    (1.0 - 0.62 * strength * k * k).max(0.0)
}

/// 锐化/模糊预览不必铺满 1600 边. 半径会按缩小比例一起缩, 拉伸回去观感仍接近.
const SPATIAL_PREVIEW_MAX: u32 = 900;

fn spatial_preview_size(out_w: u32, out_h: u32, filter: FilterAmt) -> (u32, u32) {
    if !filter.has_spatial() {
        return (out_w, out_h);
    }
    let side = out_w.max(out_h);
    if side <= SPATIAL_PREVIEW_MAX {
        return (out_w, out_h);
    }
    let s = SPATIAL_PREVIEW_MAX as f32 / side as f32;
    (
        ((out_w as f32) * s).round().max(1.0) as u32,
        ((out_h as f32) * s).round().max(1.0) as u32,
    )
}

/// `radius_scale` 为 1 时按源图像素. 预览图缩小后按同样比例缩小半径和锐化量.
/// `quantize_radius` 只给拖动预览: 半径取整, 少做一次整图模糊.
fn apply_spatial_scaled(
    img: &mut RgbaImage,
    filter: FilterAmt,
    radius_scale: f32,
    space: &PixelSpace,
    gate: PixelGate<'_>,
    quantize_radius: bool,
) {
    let scale = radius_scale.max(0.0);
    if scale < 1.0e-4 {
        return;
    }
    if filter.sharpen > 1.0e-4 {
        let amount = filter.sharpen * SHARPEN_MAX * scale.min(1.0);
        sharpen_amount(img, amount, space, gate);
    }
    if filter.blur > 1.0e-4 {
        let mut radius = filter.blur * BLUR_MAX * scale;
        if quantize_radius {
            radius = radius.round();
        }
        blur_frac(img, radius, space, gate);
    }
}

fn sharpen_amount(img: &mut RgbaImage, amount: f32, space: &PixelSpace, gate: PixelGate<'_>) {
    if amount < 1.0e-4 {
        return;
    }
    let blurred = box_blur(img, 1);
    let w = img.width();
    let h = img.height();
    let all = matches!(gate, PixelGate::All);
    let from = blurred.as_raw();
    let raw: &mut [u8] = img;
    grade_rows_parallel(raw, w, h, |rows, y0, y1| {
        let w = w as usize;
        for y in y0..y1 {
            let row_local = (y - y0) as usize * w * 4;
            let row_abs = y as usize * w * 4;
            for x in 0..w {
                let li = row_local + x * 4;
                let i = row_abs + x * 4;
                if rows[li + 3] == 0 {
                    continue;
                }
                let base = [rows[li], rows[li + 1], rows[li + 2]];
                let m = if all {
                    255
                } else {
                    gate_amount(gate, space, x as u32, y)
                };
                if m == 0 {
                    continue;
                }
                let neu = [
                    unsharp_k(base[0], from[i], amount),
                    unsharp_k(base[1], from[i + 1], amount),
                    unsharp_k(base[2], from[i + 2], amount),
                ];
                write_mixed(&mut rows[li..li + 3], base, neu, m);
            }
        }
    });
}

fn blur_frac(img: &mut RgbaImage, radius: f32, space: &PixelSpace, gate: PixelGate<'_>) {
    if radius < 0.04 {
        return;
    }
    let r0 = radius.floor() as i32;
    let frac = radius - r0 as f32;
    if r0 <= 0 {
        let blurred = box_blur(img, 1);
        blend_toward_weight(img, &blurred, frac, space, gate);
        return;
    }
    if frac < 0.02 {
        let blurred = box_blur(img, r0);
        blend_toward(img, &blurred, space, gate);
        return;
    }
    let a = box_blur(img, r0);
    let b = box_blur(img, r0 + 1);
    let mixed = lerp_images(&a, &b, frac);
    blend_toward(img, &mixed, space, gate);
}

fn blend_toward_weight(
    img: &mut RgbaImage,
    src: &RgbaImage,
    weight: f32,
    space: &PixelSpace,
    gate: PixelGate<'_>,
) {
    let weight = weight.clamp(0.0, 1.0);
    if weight < 1.0e-4 || img.dimensions() != src.dimensions() {
        return;
    }
    if weight > 0.999 {
        blend_toward(img, src, space, gate);
        return;
    }
    let w = img.width();
    let h = img.height();
    let from = src.as_raw();
    let all = matches!(gate, PixelGate::All);
    let raw: &mut [u8] = img;
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if raw[i + 3] == 0 {
                continue;
            }
            let base = [raw[i], raw[i + 1], raw[i + 2]];
            let m = if all {
                255
            } else {
                gate_amount(gate, space, x, y)
            };
            if m == 0 {
                continue;
            }
            let neu = [
                mix_u8(base[0], from[i], weight),
                mix_u8(base[1], from[i + 1], weight),
                mix_u8(base[2], from[i + 2], weight),
            ];
            write_mixed(&mut raw[i..i + 3], base, neu, m);
        }
    }
}

fn lerp_images(a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
    let mut out = a.clone();
    let br = b.as_raw();
    let dst: &mut [u8] = &mut out;
    for (i, px) in dst.chunks_exact_mut(4).enumerate() {
        let j = i * 4;
        px[0] = mix_u8(px[0], br[j], t);
        px[1] = mix_u8(px[1], br[j + 1], t);
        px[2] = mix_u8(px[2], br[j + 2], t);
    }
    out
}

fn mix_preview_gate(
    dst: &mut RgbaImage,
    before: &RgbaImage,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    space: PixelSpace,
    gate: PixelGate<'_>,
) {
    let fast = fast_gate(gate, &space, space.w.max(1), space.h.max(1));
    if matches!(fast, FastGate::All) {
        return;
    }
    if matches!(fast, FastGate::None) {
        let raw: &mut [u8] = dst;
        raw.copy_from_slice(before.as_raw());
        return;
    }
    let rad = space.rot.to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());
    let out_w = dst.width();
    let out_h = dst.height();
    let raw_before = before.as_raw();
    let pix: &mut [u8] = dst;
    for y in 0..out_h {
        let sy = src_y + map_axis(y, out_h, src_h);
        for x in 0..out_w {
            let sx = src_x + map_axis(x, out_w, src_w);
            let m = gate_mix(fast, gate, &space, sx, sy, cos, sin);
            let i = (y as usize * out_w as usize + x as usize) * 4;
            if m == 255 {
                continue;
            }
            if m == 0 {
                pix[i..i + 4].copy_from_slice(&raw_before[i..i + 4]);
                continue;
            }
            let neu = [pix[i], pix[i + 1], pix[i + 2]];
            let base = [raw_before[i], raw_before[i + 1], raw_before[i + 2]];
            write_mixed(&mut pix[i..i + 3], base, neu, m);
            pix[i + 3] = raw_before[i + 3];
        }
    }
}

fn auto_levels(img: &mut RgbaImage, space: &PixelSpace, gate: PixelGate<'_>) {
    let (lo, hi) = hist_ends(img, space, gate);
    map_point(img, space, gate, move |r, g, b, _, _, _, _| {
        [
            stretch(r, lo[0], hi[0]),
            stretch(g, lo[1], hi[1]),
            stretch(b, lo[2], hi[2]),
        ]
    });
}

fn hist_ends(img: &RgbaImage, space: &PixelSpace, gate: PixelGate<'_>) -> ([u8; 3], [u8; 3]) {
    let mut hist = [[0u32; 256]; 3];
    let mut total = 0u32;
    let w = img.width();
    let h = img.height();
    let raw = img.as_raw();
    let all = matches!(gate, PixelGate::All);
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if raw[i + 3] == 0 {
                continue;
            }
            if !all && gate_amount(gate, space, x, y) == 0 {
                continue;
            }
            hist[0][raw[i] as usize] += 1;
            hist[1][raw[i + 1] as usize] += 1;
            hist[2][raw[i + 2] as usize] += 1;
            total += 1;
        }
    }
    if total == 0 {
        return ([0; 3], [255; 3]);
    }
    let lo_n = ((u64::from(total) * 4) / 1000).max(1) as u32;
    let hi_n = ((u64::from(total) * 996) / 1000).max(lo_n as u64) as u32;
    let mut lo = [0u8; 3];
    let mut hi = [255u8; 3];
    for c in 0..3 {
        let mut acc = 0u32;
        for (i, bin) in hist[c].iter().enumerate() {
            acc += *bin;
            if acc >= lo_n {
                lo[c] = i as u8;
                break;
            }
        }
        acc = 0;
        for (i, bin) in hist[c].iter().enumerate() {
            acc += *bin;
            if acc >= hi_n {
                hi[c] = i as u8;
                break;
            }
        }
    }
    (lo, hi)
}

fn gray_world(img: &mut RgbaImage, space: &PixelSpace, gate: PixelGate<'_>) {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    let w = img.width();
    let h = img.height();
    let raw = img.as_raw();
    let all = matches!(gate, PixelGate::All);
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if raw[i + 3] == 0 {
                continue;
            }
            if !all && gate_amount(gate, space, x, y) == 0 {
                continue;
            }
            let yv = luma_f(raw[i], raw[i + 1], raw[i + 2]);
            if !(8.0..=248.0).contains(&yv) {
                continue;
            }
            sum[0] += u64::from(raw[i]);
            sum[1] += u64::from(raw[i + 1]);
            sum[2] += u64::from(raw[i + 2]);
            n += 1;
        }
    }
    if n == 0 {
        return;
    }
    let avg = [
        sum[0] as f32 / n as f32,
        sum[1] as f32 / n as f32,
        sum[2] as f32 / n as f32,
    ];
    let mean = (avg[0] + avg[1] + avg[2]) / 3.0;
    let mut gain = [1.0f32; 3];
    for c in 0..3 {
        if avg[c] > 1.0 {
            gain[c] = (mean / avg[c]).clamp(0.45, 2.2);
        }
    }
    map_point(img, space, gate, move |r, g, b, _, _, _, _| {
        [
            u8f(r as f32 * gain[0]),
            u8f(g as f32 * gain[1]),
            u8f(b as f32 * gain[2]),
        ]
    });
}

fn box_blur(src: &RgbaImage, radius: i32) -> RgbaImage {
    let radius = radius.max(0);
    if radius == 0 || src.width() == 0 || src.height() == 0 {
        return src.clone();
    }
    let w = src.width();
    let h = src.height();
    let horizontal = blur_rows(src.as_raw(), w, h, radius);
    let mut turned = vec![0u8; horizontal.len()];
    transpose_rgba(&horizontal, w as usize, h as usize, &mut turned);
    drop(horizontal);
    let vertical = blur_rows(&turned, h, w, radius);
    drop(turned);
    let mut out = RgbaImage::new(w, h);
    transpose_rgba(&vertical, h as usize, w as usize, &mut out);
    out
}

/// 分块转置, 让竖直模糊也走连续的行, 避免按列跳跃读内存.
fn transpose_rgba(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    const TILE: usize = 32;
    let mut y0 = 0;
    while y0 < h {
        let y1 = (y0 + TILE).min(h);
        let mut x0 = 0;
        while x0 < w {
            let x1 = (x0 + TILE).min(w);
            for y in y0..y1 {
                for x in x0..x1 {
                    let s = (y * w + x) * 4;
                    let d = (x * h + y) * 4;
                    dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
            x0 = x1;
        }
        y0 = y1;
    }
}

/// 边缘重复取样, 除数始终是整窗宽度. 与逐像素累加同一结果.
fn blur_rows(src: &[u8], w: u32, h: u32, radius: i32) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let radius = radius as usize;
    let mut dst = vec![0u8; src.len()];
    let threads = grade_threads(w.saturating_mul(h));
    if threads == 1 || h < 2 {
        blur_row_range(src, &mut dst, w, 0, h, radius);
        return dst;
    }
    let chunk = h.div_ceil(threads);
    let stride = w * 4;
    let mut rest = dst.as_mut_slice();
    let mut parts = Vec::new();
    let mut y = 0usize;
    while y < h {
        let rows = (h - y).min(chunk);
        let (head, tail) = rest.split_at_mut(rows * stride);
        parts.push((y, head));
        rest = tail;
        y += rows;
    }
    std::thread::scope(|scope| {
        for (y0, part) in parts {
            let rows = part.len() / stride;
            scope.spawn(move || blur_row_range(src, part, w, y0, y0 + rows, radius));
        }
    });
    dst
}

fn blur_row_range(src: &[u8], dst: &mut [u8], w: usize, y0: usize, y1: usize, radius: usize) {
    let span = (radius * 2 + 1) as u32;
    let mut prefix_r = vec![0u32; w + 1];
    let mut prefix_g = vec![0u32; w + 1];
    let mut prefix_b = vec![0u32; w + 1];
    for y in y0..y1 {
        let row = y * w * 4;
        let local = (y - y0) * w * 4;
        for x in 0..w {
            let i = row + x * 4;
            prefix_r[x + 1] = prefix_r[x] + u32::from(src[i]);
            prefix_g[x + 1] = prefix_g[x] + u32::from(src[i + 1]);
            prefix_b[x + 1] = prefix_b[x] + u32::from(src[i + 2]);
        }
        let left = row;
        let right = row + (w - 1) * 4;
        for x in 0..w {
            let si = row + x * 4;
            let di = local + x * 4;
            dst[di] = (window_sum(
                &prefix_r,
                x,
                radius,
                w,
                u32::from(src[left]),
                u32::from(src[right]),
            ) / span) as u8;
            dst[di + 1] = (window_sum(
                &prefix_g,
                x,
                radius,
                w,
                u32::from(src[left + 1]),
                u32::from(src[right + 1]),
            ) / span) as u8;
            dst[di + 2] = (window_sum(
                &prefix_b,
                x,
                radius,
                w,
                u32::from(src[left + 2]),
                u32::from(src[right + 2]),
            ) / span) as u8;
            dst[di + 3] = src[si + 3];
        }
    }
}

fn window_sum(
    prefix: &[u32],
    i: usize,
    radius: usize,
    len: usize,
    edge_lo: u32,
    edge_hi: u32,
) -> u32 {
    let i = i as i32;
    let radius = radius as i32;
    let len = len as i32;
    let lo = i - radius;
    let hi = i + radius;
    let x0 = lo.max(0) as usize;
    let x1 = hi.min(len - 1) as usize;
    let mut acc = prefix[x1 + 1] - prefix[x0];
    if lo < 0 {
        acc += edge_lo * (-lo) as u32;
    }
    if hi >= len {
        acc += edge_hi * (hi - (len - 1)) as u32;
    }
    acc
}

fn gate_amount(gate: PixelGate<'_>, space: &PixelSpace, x: u32, y: u32) -> u8 {
    let (cx, cy) = canvas_of(space, x, y);
    match gate {
        PixelGate::All => 255,
        PixelGate::Rect { x0, y0, x1, y1 } => {
            if cx >= x0 && cy >= y0 && cx <= x1 && cy <= y1 {
                255
            } else {
                0
            }
        }
        PixelGate::Mask { data, canvas_w } => {
            if cx < 0 || cy < 0 || canvas_w == 0 {
                return 0;
            }
            let i = cy as usize * canvas_w as usize + cx as usize;
            data.get(i).copied().unwrap_or(0)
        }
    }
}

fn canvas_of(space: &PixelSpace, x: u32, y: u32) -> (i32, i32) {
    if space.rot.abs() < 0.05 {
        return (
            space.ox.saturating_add(x as i32),
            space.oy.saturating_add(y as i32),
        );
    }
    let cx = space.ox as f32 + space.w as f32 * 0.5;
    let cy = space.oy as f32 + space.h as f32 * 0.5;
    let (sx, sy) = crate::geom::rotate_point(
        space.ox as f32 + x as f32 + 0.5,
        space.oy as f32 + y as f32 + 0.5,
        cx,
        cy,
        space.rot,
    );
    (sx.floor() as i32, sy.floor() as i32)
}

fn write_mixed(dst: &mut [u8], base: [u8; 3], neu: [u8; 3], m: u8) {
    if m == 0 {
        dst[..3].copy_from_slice(&base);
        return;
    }
    if m == 255 {
        dst[..3].copy_from_slice(&neu);
        return;
    }
    let m = u16::from(m);
    let inv = 255 - m;
    for c in 0..3 {
        dst[c] = ((u16::from(neu[c]) * m + u16::from(base[c]) * inv) / 255) as u8;
    }
}

fn luma_f(r: u8, g: u8, b: u8) -> f32 {
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

fn luma_u8(r: u8, g: u8, b: u8) -> u8 {
    luma_f(r, g, b).round() as u8
}

fn sepia(r: u8, g: u8, b: u8) -> [u8; 3] {
    let r = r as f32;
    let g = g as f32;
    let b = b as f32;
    [
        u8f(0.393 * r + 0.769 * g + 0.189 * b),
        u8f(0.349 * r + 0.686 * g + 0.168 * b),
        u8f(0.272 * r + 0.534 * g + 0.131 * b),
    ]
}

fn fade(c: u8) -> u8 {
    u8f(c as f32 * 0.78 + 32.0)
}

fn vignette(r: u8, g: u8, b: u8, x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let w = w.max(1) as f32;
    let h = h.max(1) as f32;
    let nx = (x as f32 + 0.5) / w * 2.0 - 1.0;
    let ny = (y as f32 + 0.5) / h * 2.0 - 1.0;
    let d = (nx * nx * 0.85 + ny * ny).sqrt();
    let k = ((d - 0.55) / 0.75).clamp(0.0, 1.0);
    let scale = 1.0 - 0.62 * k * k;
    [
        u8f(r as f32 * scale),
        u8f(g as f32 * scale),
        u8f(b as f32 * scale),
    ]
}

fn stretch(v: u8, lo: u8, hi: u8) -> u8 {
    if hi <= lo {
        return v;
    }
    let t = (i32::from(v) - i32::from(lo)) * 255 / (i32::from(hi) - i32::from(lo));
    t.clamp(0, 255) as u8
}

fn u8f(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn space() -> PixelSpace {
        PixelSpace {
            ox: 0,
            oy: 0,
            rot: 0.0,
            w: 2,
            h: 1,
        }
    }

    #[test]
    fn neutral_tone_keeps_pixels() {
        let base = RgbaImage::from_pixel(2, 1, Rgba([10, 80, 200, 255]));
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 0]));
        apply_tone(&mut img, &base, Tone::default(), space(), PixelGate::All);
        assert_eq!(img.get_pixel(1, 0).0, [10, 80, 200, 255]);
    }

    #[test]
    fn brightness_lifts_and_rect_gate_spares_the_rest() {
        let base = RgbaImage::from_pixel(2, 1, Rgba([10, 10, 10, 255]));
        let mut img = base.clone();
        let mut tone = Tone::default();
        tone.brightness = 1.0;
        apply_tone(
            &mut img,
            &base,
            tone,
            space(),
            PixelGate::Rect {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            },
        );
        assert!(img.get_pixel(0, 0).0[0] > 10);
        assert_eq!(img.get_pixel(1, 0).0, [10, 10, 10, 255]);
    }

    #[test]
    fn grayscale_and_gray_world() {
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([200, 40, 40, 255]));
        apply_look(&mut img, Look::Grayscale, space(), PixelGate::All);
        let g = img.get_pixel(0, 0).0;
        assert_eq!(g[0], g[1]);
        assert_eq!(g[1], g[2]);

        let mut cast = RgbaImage::from_pixel(2, 1, Rgba([220, 80, 80, 255]));
        apply_look(&mut cast, Look::GrayWorld, space(), PixelGate::All);
        let p = cast.get_pixel(0, 0).0;
        let spread = p[0].abs_diff(p[1]).max(p[1].abs_diff(p[2]));
        assert!(spread < 140);
    }

    #[test]
    fn parallel_tone_matches_per_pixel_reference() {
        let mut base = RgbaImage::new(512, 256);
        for (i, px) in base.pixels_mut().enumerate() {
            let r = (i * 17) as u8;
            let g = (i * 29) as u8;
            let b = (i * 13) as u8;
            let a = if i % 64 == 0 { 0 } else { 255 };
            *px = Rgba([r, g, b, a]);
        }
        let mut tone = Tone::default();
        tone.brightness = 0.35;
        tone.contrast = -0.2;
        tone.saturation = 0.4;
        tone.temperature = 0.15;
        tone.highlights = -0.25;
        let mut img = RgbaImage::new(512, 256);
        apply_tone(&mut img, &base, tone, space_wh(512, 256), PixelGate::All);
        let k = ToneKernel::from(tone);
        for (out, src) in img.pixels().zip(base.pixels()) {
            let expect = if src[3] == 0 {
                *src
            } else {
                let rgb = map_rgb(src[0], src[1], src[2], k);
                Rgba([rgb[0], rgb[1], rgb[2], src[3]])
            };
            assert_eq!(out, &expect);
        }
    }

    #[test]
    fn preview_one_to_one_matches_apply_tone() {
        let base = RgbaImage::from_fn(48, 32, |x, y| Rgba([(x * 5) as u8, (y * 7) as u8, 40, 255]));
        let mut tone = Tone::default();
        tone.contrast = 0.4;
        tone.tint = -0.3;
        tone.shadows = 0.5;
        let mut full = base.clone();
        let sp = space_wh(48, 32);
        apply_tone(&mut full, &base, tone, sp, PixelGate::All);
        let preview = render_tone_preview(&base, 0, 0, 48, 32, 48, 32, tone, sp, PixelGate::All);
        assert_eq!(preview.as_raw(), full.as_raw());
    }

    #[test]
    fn rect_gate_still_limits_parallel_grade() {
        let base = RgbaImage::from_pixel(80, 40, Rgba([20, 30, 40, 255]));
        let mut img = base.clone();
        let mut tone = Tone::default();
        tone.brightness = 1.0;
        apply_tone(
            &mut img,
            &base,
            tone,
            space_wh(80, 40),
            PixelGate::Rect {
                x0: 4,
                y0: 6,
                x1: 10,
                y1: 8,
            },
        );
        assert!(img.get_pixel(4, 6).0[0] > 20);
        assert_eq!(img.get_pixel(3, 6).0, [20, 30, 40, 255]);
        assert_eq!(img.get_pixel(4, 5).0, [20, 30, 40, 255]);
    }

    fn space_wh(w: u32, h: u32) -> PixelSpace {
        PixelSpace {
            ox: 0,
            oy: 0,
            rot: 0.0,
            w,
            h,
        }
    }

    #[test]
    fn filter_sliders_match_one_click_at_the_old_strength() {
        let src = RgbaImage::from_fn(5, 4, |x, y| {
            Rgba([
                (x * 40 + y * 3) as u8,
                (y * 50 + 20) as u8,
                (255 - x * 30) as u8,
                255,
            ])
        });
        let sp = space_wh(5, 4);

        let mut gray = src.clone();
        let mut amt = FilterAmt::default();
        amt.gray = 1.0;
        apply_grade(&mut gray, &src, Tone::default(), amt, sp, PixelGate::All);
        let mut once = src.clone();
        apply_look(&mut once, Look::Grayscale, sp, PixelGate::All);
        assert_eq!(gray.as_raw(), once.as_raw());

        let mut half = src.clone();
        amt.gray = 0.5;
        apply_grade(&mut half, &src, Tone::default(), amt, sp, PixelGate::All);
        let p = half.get_pixel(2, 1).0;
        let g = gray.get_pixel(2, 1).0;
        let s = src.get_pixel(2, 1).0;
        assert!(p[0].abs_diff(s[0]) > 0);
        assert!(p[0].abs_diff(g[0]) > 0);

        let mut sep = src.clone();
        amt = FilterAmt::default();
        amt.sepia = 1.0;
        apply_grade(&mut sep, &src, Tone::default(), amt, sp, PixelGate::All);
        let mut once = src.clone();
        apply_look(&mut once, Look::Sepia, sp, PixelGate::All);
        assert_eq!(sep.as_raw(), once.as_raw());

        let mut faded = src.clone();
        amt = FilterAmt::default();
        amt.fade = 1.0 / super::FADE_MAX_STEPS;
        apply_grade(&mut faded, &src, Tone::default(), amt, sp, PixelGate::All);
        let mut once = src.clone();
        apply_look(&mut once, Look::Fade, sp, PixelGate::All);
        assert_eq!(faded.as_raw(), once.as_raw());

        let mut sharp = src.clone();
        amt = FilterAmt::default();
        amt.sharpen = 0.85 / super::SHARPEN_MAX;
        apply_grade(&mut sharp, &src, Tone::default(), amt, sp, PixelGate::All);
        let mut once = src.clone();
        apply_look(&mut once, Look::Sharpen, sp, PixelGate::All);
        assert_eq!(sharp.as_raw(), once.as_raw());

        let mut blurred = src.clone();
        amt = FilterAmt::default();
        amt.blur = 2.0 / super::BLUR_MAX;
        apply_grade(&mut blurred, &src, Tone::default(), amt, sp, PixelGate::All);
        let mut once = src.clone();
        apply_look(&mut once, Look::Blur, sp, PixelGate::All);
        assert_eq!(blurred.as_raw(), once.as_raw());
    }

    #[test]
    fn deeper_blur_and_gray_stay_inside_the_rect() {
        let src = RgbaImage::from_fn(12, 8, |x, y| {
            Rgba([(x * 20) as u8, (y * 30) as u8, 80, 255])
        });
        let sp = space_wh(12, 8);
        let mut img = src.clone();
        let mut amt = FilterAmt::default();
        amt.gray = 1.0;
        amt.blur = 1.0;
        apply_grade(
            &mut img,
            &src,
            Tone::default(),
            amt,
            sp,
            PixelGate::Rect {
                x0: 2,
                y0: 2,
                x1: 6,
                y1: 5,
            },
        );
        assert_eq!(img.get_pixel(0, 0).0, src.get_pixel(0, 0).0);
        assert_ne!(img.get_pixel(4, 3).0, src.get_pixel(4, 3).0);
        let mut gray_only = src.clone();
        amt.blur = 0.0;
        apply_grade(
            &mut gray_only,
            &src,
            Tone::default(),
            amt,
            sp,
            PixelGate::Rect {
                x0: 2,
                y0: 2,
                x1: 6,
                y1: 5,
            },
        );
        assert_eq!(gray_only.get_pixel(0, 0).0, src.get_pixel(0, 0).0);
        let g = gray_only.get_pixel(4, 3).0;
        assert_eq!(g[0], g[1]);
        assert_eq!(g[1], g[2]);
    }

    #[test]
    fn grade_preview_matches_full_gray() {
        let src = RgbaImage::from_fn(9, 7, |x, y| Rgba([(x * 25) as u8, 40, (y * 20) as u8, 255]));
        let sp = space_wh(9, 7);
        let mut amt = FilterAmt::default();
        amt.gray = 0.6;
        amt.vignette = 0.4;
        let mut full = src.clone();
        apply_grade(&mut full, &src, Tone::default(), amt, sp, PixelGate::All);
        let preview = render_grade_preview(
            &src,
            0,
            0,
            9,
            7,
            9,
            7,
            Tone::default(),
            amt,
            sp,
            PixelGate::All,
        );
        assert_eq!(preview.as_raw(), full.as_raw());
    }

    #[test]
    fn prefix_blur_matches_naive_window() {
        let src = RgbaImage::from_fn(6, 5, |x, y| {
            Rgba([(x * 40) as u8, (y * 50) as u8, (x + y * 3) as u8, 200])
        });
        let fast = box_blur(&src, 2);
        let slow = naive_box_blur(&src, 2);
        assert_eq!(fast.as_raw(), slow.as_raw());
    }

    fn naive_box_blur(src: &RgbaImage, radius: i32) -> RgbaImage {
        let w = src.width() as i32;
        let h = src.height() as i32;
        let raw = src.as_raw();
        let span = (radius * 2 + 1) as u32;
        let mut tmp = vec![0u8; raw.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 3];
                for k in -radius..=radius {
                    let xx = (x + k).clamp(0, w - 1) as usize;
                    let i = (y as usize * w as usize + xx) * 4;
                    acc[0] += u32::from(raw[i]);
                    acc[1] += u32::from(raw[i + 1]);
                    acc[2] += u32::from(raw[i + 2]);
                }
                let i = (y as usize * w as usize + x as usize) * 4;
                tmp[i] = (acc[0] / span) as u8;
                tmp[i + 1] = (acc[1] / span) as u8;
                tmp[i + 2] = (acc[2] / span) as u8;
                tmp[i + 3] = raw[i + 3];
            }
        }
        let mut out = RgbaImage::new(src.width(), src.height());
        let dst: &mut [u8] = &mut out;
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 3];
                for k in -radius..=radius {
                    let yy = (y + k).clamp(0, h - 1) as usize;
                    let i = (yy * w as usize + x as usize) * 4;
                    acc[0] += u32::from(tmp[i]);
                    acc[1] += u32::from(tmp[i + 1]);
                    acc[2] += u32::from(tmp[i + 2]);
                }
                let i = (y as usize * w as usize + x as usize) * 4;
                dst[i] = (acc[0] / span) as u8;
                dst[i + 1] = (acc[1] / span) as u8;
                dst[i + 2] = (acc[2] / span) as u8;
                dst[i + 3] = tmp[i + 3];
            }
        }
        out
    }
}
