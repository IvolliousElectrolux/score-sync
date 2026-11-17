//! 进程内解码 mp3 / ogg / m4a / wav / aac, 按采样点跳转.
//!
//! FLAC 仍走 `flac_index`: 那里会用文件自带的 SEEKTABLE, 或等后台把帧偏移建好.
//! 这里打开失败时, 调用方再退回 ffmpeg 管道.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub(crate) struct SymphoniaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    skip_frames: u64,
}

impl SymphoniaDecoder {
    pub(crate) fn open(path: &Path, start_secs: f64) -> Option<Self> {
        let file = File::open(path).ok()?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .ok()?;
        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|track| track.codec_params.sample_rate.is_some())?
            .clone();
        let sample_rate = track.codec_params.sample_rate?;
        if sample_rate == 0 {
            return None;
        }
        let channels = track
            .codec_params
            .channels
            .map(|n| n.count())
            .unwrap_or(1)
            .max(1) as u16;
        let start_sample = (start_secs.max(0.0) * sample_rate as f64).round() as u64;
        let seeked = format
            .seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts: start_sample,
                    track_id: track.id,
                },
            )
            .ok()?;
        if seeked.actual_ts > seeked.required_ts {
            return None;
        }
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .ok()?;
        Some(Self {
            format,
            decoder,
            track_id: track.id,
            sample_rate,
            channels,
            skip_frames: seeked.required_ts - seeked.actual_ts,
        })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.channels
    }

    pub(crate) fn pull(&mut self) -> Option<Vec<i16>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(_) => return None,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphError::DecodeError(_)) => continue,
                Err(_) => return None,
            };
            let spec = *decoded.spec();
            let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            let mut samples = buf.samples().to_vec();
            let channels = spec.channels.count().max(1);
            if self.skip_frames > 0 && channels > 0 {
                let frames = samples.len() / channels;
                let drop_frames = (self.skip_frames as usize).min(frames);
                let drop_samples = drop_frames * channels;
                if drop_samples > 0 {
                    samples.drain(..drop_samples);
                }
                self.skip_frames -= drop_frames as u64;
            }
            if samples.is_empty() {
                continue;
            }
            return Some(samples);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn ramp_wav() -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("score-sync-ramp-{stamp}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..44100 {
            writer.write_sample((i / 100) as i16).unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    #[test]
    fn wav_seek_lands_on_the_requested_sample() {
        let path = ramp_wav();
        let mut decoder = SymphoniaDecoder::open(&path, 0.5).expect("open wav");
        let chunk = decoder.pull().expect("samples");
        let _ = std::fs::remove_file(&path);
        assert_eq!(decoder.sample_rate(), 44100);
        assert_eq!(decoder.channels(), 1);
        let got = i32::from(chunk[0]);
        assert!((got - 220).abs() <= 1, "got {got}");
    }
}
