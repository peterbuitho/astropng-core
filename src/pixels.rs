//! Convert a decoded XISF image into 8-bit PNG pixel data, applying a linear
//! per-image min/max stretch to the full sample range.

use crate::xisf::{SampleFormat, XisfError, XisfImageData};

pub struct Image8 {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    /// Interleaved 8-bit samples, row-major.
    pub pixels: Vec<u8>,
}

type Result<T> = std::result::Result<T, XisfError>;

pub fn to_image(img: &XisfImageData) -> Result<Image8> {
    if img.channels != 1 && img.channels != 3 {
        return Err(XisfError(format!(
            "Unsupported channel count {} (only mono and RGB are handled).",
            img.channels
        )));
    }

    let w = img.width as usize;
    let h = img.height as usize;
    let c = img.channels as usize;
    let count = w * h * c;

    let mut samples = decode_samples(img, count)?;

    // Planar (channel-major) -> interleaved (pixel-major).
    if img.planar && c > 1 {
        let mut interleaved = vec![0.0f64; count];
        let plane = w * h;
        for ch in 0..c {
            for i in 0..plane {
                interleaved[i * c + ch] = samples[ch * plane + i];
            }
        }
        samples = interleaved;
    }

    // Global min/max linear scale to [0, 255].
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &v in &samples {
        if v.is_nan() {
            continue;
        }
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }

    let mut pixels = vec![0u8; count];
    if min.is_finite() && max.is_finite() && max > min {
        let scale = 255.0 / (max - min);
        for i in 0..count {
            let v = samples[i];
            if v.is_nan() {
                pixels[i] = 0;
                continue;
            }
            let s = (v - min) * scale;
            pixels[i] = if s <= 0.0 {
                0
            } else if s >= 255.0 {
                255
            } else {
                (s + 0.5) as u8
            };
        }
    }
    // else: flat / degenerate image -> all black (already zeroed).

    Ok(Image8 {
        width: img.width,
        height: img.height,
        channels: img.channels,
        pixels,
    })
}

fn decode_samples(img: &XisfImageData, count: usize) -> Result<Vec<f64>> {
    let raw = &img.raw_data;
    let bps = img.format.bytes_per_sample();
    let swap = img.big_endian;
    let mut out = vec![0.0f64; count];

    if raw.len() < count * bps {
        return Err(XisfError(format!(
            "Pixel data too small: got {} bytes, expected {}.",
            raw.len(),
            count * bps
        )));
    }

    for i in 0..count {
        let o = i * bps;
        let b = &raw[o..o + bps];
        out[i] = match img.format {
            SampleFormat::UInt8 => b[0] as f64,
            SampleFormat::UInt16 => {
                let v = to_arr::<2>(b);
                (if swap { u16::from_be_bytes(v) } else { u16::from_le_bytes(v) }) as f64
            }
            SampleFormat::UInt32 => {
                let v = to_arr::<4>(b);
                (if swap { u32::from_be_bytes(v) } else { u32::from_le_bytes(v) }) as f64
            }
            SampleFormat::UInt64 => {
                let v = to_arr::<8>(b);
                (if swap { u64::from_be_bytes(v) } else { u64::from_le_bytes(v) }) as f64
            }
            SampleFormat::Float32 => {
                let v = to_arr::<4>(b);
                (if swap { f32::from_be_bytes(v) } else { f32::from_le_bytes(v) }) as f64
            }
            SampleFormat::Float64 => {
                let v = to_arr::<8>(b);
                if swap { f64::from_be_bytes(v) } else { f64::from_le_bytes(v) }
            }
        };
    }

    Ok(out)
}

#[inline]
fn to_arr<const N: usize>(b: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&b[..N]);
    a
}
