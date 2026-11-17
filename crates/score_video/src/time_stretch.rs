//! 预览倍速. 把一小段原始采样重叠相加, 时长变成 1/speed, 音高不动.
//!
//! 不新开 ffmpeg. 窗长大约 20ms, 输出步进是窗的一半, 输入步进再乘上倍速.
//! 汉宁窗在 50% 重叠时加起来是 1, 所以恒定电平不会被窗函数改变.

use std::f32::consts::PI;

pub(crate) struct TimeStretch {
    channels: usize,
    speed: f32,
    n: usize,
    hs: usize,
    hann: Vec<f32>,
    /// 下一颗窗的输入起点, 单位是帧.
    pos: f64,
    /// `input[0][0]` 对应的帧号.
    origin: usize,
    input: Vec<Vec<f32>>,
    /// 下一颗窗写入的输出帧号.
    write_at: usize,
    out_origin: usize,
    acc: Vec<Vec<f32>>,
    norm: Vec<f32>,
}

impl TimeStretch {
    pub(crate) fn new(channels: usize, speed: f32, sample_rate: u32) -> Self {
        let channels = channels.max(1);
        let speed = speed.clamp(0.25, 4.0);
        let n = ((sample_rate.max(1) as usize) / 50).max(256) & !1;
        let hs = n / 2;
        let hann = (0..n)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * (i as f32) / (n as f32)).cos()))
            .collect();
        Self {
            channels,
            speed,
            n,
            hs,
            hann,
            pos: 0.0,
            origin: 0,
            input: vec![Vec::new(); channels],
            write_at: 0,
            out_origin: 0,
            acc: vec![Vec::new(); channels],
            norm: Vec::new(),
        }
    }

    /// 吃进交错的 i16, 吐出已经能确定的输出采样 (可能暂时为空).
    pub(crate) fn push(&mut self, interleaved: &[i16]) -> Vec<i16> {
        let ch = self.channels;
        for frame in interleaved.chunks(ch) {
            if frame.len() < ch {
                break;
            }
            for (c, sample) in frame.iter().enumerate() {
                self.input[c].push(*sample as f32);
            }
        }
        self.produce()
    }

    /// 输入结束. 把还没交出去的尾巴吐完.
    pub(crate) fn finish(&mut self) -> Vec<i16> {
        let frames = self.available_frames();
        let start = (self.pos - self.origin as f64).max(0.0);
        if start < frames as f64 {
            let need = (start + self.n as f64).ceil() as usize + 1;
            for row in &mut self.input {
                if row.len() < need {
                    row.resize(need, 0.0);
                }
            }
        }
        let mut out = self.produce();
        let rest = self.norm.len();
        self.emit_frames(&mut out, rest);
        out
    }

    fn available_frames(&self) -> usize {
        self.input.iter().map(|row| row.len()).min().unwrap_or(0)
    }

    fn produce(&mut self) -> Vec<i16> {
        let mut out = Vec::new();
        loop {
            let frames = self.available_frames();
            let have_end = self.origin as f64 + frames as f64;
            let radius = (self.hs / 2) as f64;
            if self.pos + self.n as f64 + radius > have_end {
                break;
            }
            let delta = self.align_delta();
            self.add_grain(delta);
            self.pos += self.hs as f64 * f64::from(self.speed);
            self.write_at += self.hs;
            self.discard_input();
            self.emit_ready(&mut out);
        }
        out
    }

    /// 在自然起点附近找和上一段重叠区最像的偏移, 避免两段相位对不上把音高拉偏.
    fn align_delta(&self) -> f64 {
        if self.write_at == 0 || self.norm.is_empty() {
            return 0.0;
        }
        let radius = (self.hs / 2) as isize;
        let min_frame = self.origin as f64;
        let max_frame = self.origin as f64 + self.available_frames() as f64;
        let mut best = 0isize;
        let mut best_score = f32::NEG_INFINITY;
        let mut delta = -radius;
        while delta <= radius {
            let start = self.pos + delta as f64;
            if start >= min_frame && start + self.hs as f64 <= max_frame {
                let score = self.correlation(delta);
                if score > best_score {
                    best_score = score;
                    best = delta;
                }
            }
            delta += 1;
        }
        best as f64
    }

    fn correlation(&self, delta: isize) -> f32 {
        let mut score = 0.0f32;
        let start = self.pos + delta as f64;
        for i in 0..self.hs {
            let Some(norm) = self.norm.get(i).copied() else {
                continue;
            };
            if norm < 1e-4 {
                continue;
            }
            let prev = self.acc[0].get(i).copied().unwrap_or(0.0) / norm;
            score += prev * self.sample(0, start + i as f64);
        }
        score
    }

    fn add_grain(&mut self, delta: f64) {
        let base = self.write_at - self.out_origin;
        let need = base + self.n;
        if self.norm.len() < need {
            self.norm.resize(need, 0.0);
            for row in &mut self.acc {
                row.resize(need, 0.0);
            }
        }
        for i in 0..self.n {
            let frame = self.pos + delta + i as f64;
            let weight = self.hann[i];
            let at = base + i;
            self.norm[at] += weight;
            for c in 0..self.channels {
                self.acc[c][at] += weight * self.sample(c, frame);
            }
        }
    }

    fn sample(&self, channel: usize, frame: f64) -> f32 {
        let idx = frame - self.origin as f64;
        if idx < 0.0 {
            return 0.0;
        }
        let i0 = idx.floor() as usize;
        let frac = (idx - i0 as f64) as f32;
        let row = &self.input[channel];
        let a = row.get(i0).copied().unwrap_or(0.0);
        let b = row.get(i0 + 1).copied().unwrap_or(a);
        a * (1.0 - frac) + b * frac
    }

    fn discard_input(&mut self) {
        let margin = (self.hs / 2) as isize + 2;
        let keep_from = (self.pos.floor() as isize - margin).max(0) as usize;
        if keep_from <= self.origin {
            return;
        }
        let drop_n = (keep_from - self.origin).min(self.available_frames());
        if drop_n == 0 {
            return;
        }
        for row in &mut self.input {
            let n = drop_n.min(row.len());
            row.drain(..n);
        }
        self.origin += drop_n;
    }

    fn emit_ready(&mut self, out: &mut Vec<i16>) {
        if self.write_at <= self.out_origin {
            return;
        }
        let nready = self.write_at - self.out_origin;
        self.emit_frames(out, nready);
    }

    fn emit_frames(&mut self, out: &mut Vec<i16>, count: usize) {
        let count = count.min(self.norm.len());
        for frame in 0..count {
            let norm = self.norm[frame];
            for c in 0..self.channels {
                let value = if norm > 1e-6 {
                    self.acc[c][frame] / norm
                } else {
                    0.0
                };
                out.push(value.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16);
            }
        }
        if count == 0 {
            return;
        }
        self.norm.drain(..count);
        for row in &mut self.acc {
            if row.len() >= count {
                row.drain(..count);
            } else {
                row.clear();
            }
        }
        self.out_origin += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_level_survives_double_speed() {
        let mut stretch = TimeStretch::new(1, 2.0, 44100);
        let input = vec![8000i16; 44100];
        let mut out = stretch.push(&input);
        out.extend(stretch.finish());
        assert!(!out.is_empty());
        let mid = &out[out.len() / 4..out.len() * 3 / 4];
        let mean = mid.iter().map(|s| i32::from(*s)).sum::<i32>() / mid.len() as i32;
        assert!((mean - 8000).abs() < 2, "mean {mean}");
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 0.5).abs() < 0.05, "ratio {ratio}");
    }

    #[test]
    fn stereo_channels_stay_apart() {
        let mut stretch = TimeStretch::new(2, 1.5, 44100);
        let mut input = Vec::with_capacity(44100 * 2);
        for _ in 0..44100 {
            input.push(4000);
            input.push(-4000);
        }
        let mut out = stretch.push(&input);
        out.extend(stretch.finish());
        let mid = out.len() / 2;
        let start = mid - (mid % 2);
        assert!((i32::from(out[start]) - 4000).abs() < 3);
        assert!((i32::from(out[start + 1]) + 4000).abs() < 3);
    }

    #[test]
    fn double_speed_keeps_the_sine_period() {
        let sr = 44100.0f32;
        let freq = 440.0f32;
        let input: Vec<i16> = (0..sr as usize)
            .map(|i| {
                let x = (2.0 * std::f32::consts::PI * freq * (i as f32) / sr).sin();
                (x * 10000.0) as i16
            })
            .collect();
        let mut stretch = TimeStretch::new(1, 2.0, sr as u32);
        let mut out = stretch.push(&input);
        out.extend(stretch.finish());
        let period = mean_upward_period(&out[out.len() / 3..out.len() * 2 / 3]);
        let expect = sr / freq;
        assert!(
            (period - expect).abs() / expect < 0.03,
            "period {period} expected {expect}"
        );
    }

    fn mean_upward_period(samples: &[i16]) -> f32 {
        let mut marks = Vec::new();
        for i in 1..samples.len() {
            if samples[i - 1] <= 0 && samples[i] > 0 {
                marks.push(i);
            }
        }
        assert!(marks.len() >= 4, "crossings {}", marks.len());
        let mut sum = 0.0;
        for pair in marks.windows(2) {
            sum += (pair[1] - pair[0]) as f32;
        }
        sum / (marks.len() - 1) as f32
    }
}
