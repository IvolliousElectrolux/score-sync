//! 蒙版/画笔颜色与透明度偏好 (最近使用色, HSV 换算).

use serde::{Deserialize, Serialize};

use crate::mask::DEFAULT_MASK_OPACITY;

pub const RECENT_COLORS_MAX: usize = 8;
pub const DEFAULT_BRUSH_OPACITY: f32 = 1.0;

fn default_white() -> [u8; 3] {
    [255, 255, 255]
}

fn default_mask_opacity() -> f32 {
    DEFAULT_MASK_OPACITY
}

fn default_brush_opacity() -> f32 {
    DEFAULT_BRUSH_OPACITY
}

fn default_recent() -> Vec<[u8; 3]> {
    vec![
        [255, 255, 255],
        [250, 204, 21],
        [56, 189, 248],
        [251, 146, 60],
        [74, 222, 128],
        [248, 113, 113],
        [0, 0, 0],
        [148, 163, 184],
    ]
}

/// 可序列化进工程 / appdata 的选色偏好.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskColorPrefs {
    #[serde(default = "default_recent")]
    pub recent_colors: Vec<[u8; 3]>,
    #[serde(default = "default_mask_opacity")]
    pub mask_opacity: f32,
    #[serde(default = "default_brush_opacity")]
    pub brush_opacity: f32,
    #[serde(default = "default_white")]
    pub mask_color: [u8; 3],
    #[serde(default = "default_white")]
    pub brush_color: [u8; 3],
}

impl Default for MaskColorPrefs {
    fn default() -> Self {
        Self {
            recent_colors: default_recent(),
            mask_opacity: DEFAULT_MASK_OPACITY,
            brush_opacity: DEFAULT_BRUSH_OPACITY,
            mask_color: [255, 255, 255],
            brush_color: [255, 255, 255],
        }
    }
}

impl MaskColorPrefs {
    pub fn clamp(mut self) -> Self {
        self.mask_opacity = self.mask_opacity.clamp(0.05, 1.0);
        self.brush_opacity = self.brush_opacity.clamp(0.05, 1.0);
        if self.recent_colors.is_empty() {
            self.recent_colors = default_recent();
        }
        while self.recent_colors.len() > RECENT_COLORS_MAX {
            self.recent_colors.pop();
        }
        self
    }

    pub fn push_recent(&mut self, color: [u8; 3]) {
        self.recent_colors.retain(|c| *c != color);
        self.recent_colors.insert(0, color);
        while self.recent_colors.len() > RECENT_COLORS_MAX {
            self.recent_colors.pop();
        }
    }
}

/// HSV → RGB. `h` 为度 (0..360), `s`/`v` 为 0..1.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// RGB → HSV. 返回 `(h_deg 0..360, s 0..1, v 0..1)`.
pub fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}
