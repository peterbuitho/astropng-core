//! Minimal FITS reader: the image in the primary HDU of a `.fits` / `.fit` /
//! `.fts` file, as written by PixInsight, N.I.N.A., Siril, APT, SharpCap and
//! most capture software.
//!
//! Supported: BITPIX 8, 16, 32, 64, -32, -64; NAXIS = 2 (mono) or 3 with
//! NAXIS3 = 1 or 3 (RGB planes); BZERO / BSCALE; ROWORDER. Not supported:
//! images stored in extensions, tile-compressed (`.fz`) files, BINTABLEs.
//!
//! The decoded image is handed over as [`XisfImageData`] so the rest of the
//! pipeline is shared with XISF.

use std::path::Path;

use crate::xisf::{SampleFormat, XisfError, XisfImageData};

const BLOCK: usize = 2880;
const CARD: usize = 80;

type Result<T> = std::result::Result<T, XisfError>;

macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(XisfError(format!($($arg)*)))
    };
}

pub fn read(path: &Path) -> Result<XisfImageData> {
    let bytes = std::fs::read(path).map_err(|e| XisfError(format!("cannot read file: {e}")))?;
    parse(&bytes)
}

/// Parse an in-memory FITS file.
pub fn parse(bytes: &[u8]) -> Result<XisfImageData> {
    if bytes.len() < BLOCK || !bytes.starts_with(b"SIMPLE  =") {
        bail!("Not a FITS file (missing SIMPLE card).");
    }

    let header = Header::parse(bytes)?;

    if header.bool("SIMPLE") != Some(true) {
        bail!("Non-standard FITS file (SIMPLE is not T).");
    }
    let bitpix = header.int("BITPIX").ok_or_else(|| XisfError("BITPIX missing".into()))?;
    let naxis = header.int("NAXIS").ok_or_else(|| XisfError("NAXIS missing".into()))?;

    let (width, height, channels) = match naxis {
        0 => bail!("Primary HDU has no image data (NAXIS = 0); images in extensions are not supported."),
        2 => (header.axis(1)?, header.axis(2)?, 1),
        3 => {
            let c = header.axis(3)?;
            if c != 1 && c != 3 {
                bail!("Unsupported NAXIS3 = {c} (only 1 or 3 channel images are handled).");
            }
            (header.axis(1)?, header.axis(2)?, c)
        }
        n => bail!("Unsupported NAXIS = {n} (only 2-D images and 3-plane RGB are handled)."),
    };
    if width == 0 || height == 0 {
        bail!("Image has zero size.");
    }

    let bytes_per_sample = match bitpix {
        8 => 1,
        16 => 2,
        32 => 4,
        64 => 8,
        -32 => 4,
        -64 => 8,
        other => bail!("Unsupported BITPIX = {other}."),
    };
    let count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(channels as usize))
        .ok_or_else(|| XisfError("Image dimensions overflow".into()))?;
    let data_len = count * bytes_per_sample;
    let data = bytes
        .get(header.data_offset..header.data_offset + data_len)
        .ok_or_else(|| {
            XisfError(format!(
                "Pixel data truncated: need {data_len} bytes after the header, file has {}.",
                bytes.len().saturating_sub(header.data_offset)
            ))
        })?;

    let bzero = header.float("BZERO").unwrap_or(0.0);
    let bscale = header.float("BSCALE").unwrap_or(1.0);
    let scaled = bzero != 0.0 || bscale != 1.0;

    // FITS stores the first row at the *bottom* of the image unless the
    // writer says otherwise via the (non-standard but widely used) ROWORDER
    // keyword.
    let bottom_up = !matches!(
        header.string("ROWORDER").as_deref(),
        Some(s) if s.eq_ignore_ascii_case("TOP-DOWN")
    );

    let (w, h, c) = (width as usize, height as usize, channels as usize);
    let plane = w * h;

    // Fast paths: sample formats the rest of the pipeline reads natively.
    // Everything else (signed integers, BZERO/BSCALE) is converted to f32.
    let (format, raw_data) = match (bitpix, scaled) {
        (8, false) => (SampleFormat::UInt8, flip_rows(data, w, h, c, 1, bottom_up)),
        (-32, false) => (SampleFormat::Float32, flip_rows(data, w, h, c, 4, bottom_up)),
        (-64, false) => (SampleFormat::Float64, flip_rows(data, w, h, c, 8, bottom_up)),
        _ => {
            let mut out = vec![0u8; count * 4];
            for ch in 0..c {
                for y in 0..h {
                    let dst_y = if bottom_up { h - 1 - y } else { y };
                    for x in 0..w {
                        let i = ch * plane + y * w + x;
                        let raw = read_sample(bitpix, &data[i * bytes_per_sample..]);
                        let v = (bzero + bscale * raw) as f32;
                        let o = (ch * plane + dst_y * w + x) * 4;
                        out[o..o + 4].copy_from_slice(&v.to_be_bytes());
                    }
                }
            }
            (SampleFormat::Float32, out)
        }
    };

    let object = header
        .string("OBJECT")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let coords = crate::wcs::from_keywords(&|k| header.value(k), width, height);

    Ok(XisfImageData {
        width,
        height,
        channels,
        format,
        planar: true,
        big_endian: true,
        raw_data,
        object,
        coords,
    })
}

/// Read one big-endian sample as f64.
#[inline]
fn read_sample(bitpix: i64, b: &[u8]) -> f64 {
    match bitpix {
        8 => b[0] as f64,
        16 => i16::from_be_bytes([b[0], b[1]]) as f64,
        32 => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64,
        64 => i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f64,
        -32 => f32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64,
        -64 => f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        _ => 0.0,
    }
}

/// Copy planar sample bytes, reversing the row order of every plane when
/// `flip` is set.
fn flip_rows(data: &[u8], w: usize, h: usize, c: usize, bps: usize, flip: bool) -> Vec<u8> {
    if !flip {
        return data.to_vec();
    }
    let row = w * bps;
    let plane = row * h;
    let mut out = vec![0u8; data.len()];
    for ch in 0..c {
        for y in 0..h {
            let src = ch * plane + y * row;
            let dst = ch * plane + (h - 1 - y) * row;
            out[dst..dst + row].copy_from_slice(&data[src..src + row]);
        }
    }
    out
}

/// The primary header: keyword -> raw value text (comment stripped).
struct Header {
    cards: Vec<(String, String)>,
    /// Byte offset of the data unit (header padded to a 2880-byte block).
    data_offset: usize,
}

impl Header {
    fn parse(bytes: &[u8]) -> Result<Header> {
        let mut cards = Vec::new();
        let mut pos = 0;
        loop {
            let Some(card) = bytes.get(pos..pos + CARD) else {
                bail!("Header has no END card.");
            };
            pos += CARD;
            let key = String::from_utf8_lossy(&card[..8]).trim_end().to_string();
            if key == "END" {
                break;
            }
            // Only "KEY     = value / comment" cards carry values; COMMENT,
            // HISTORY, CONTINUE and blank cards are ignored.
            if &card[8..10] == b"= " {
                let value = strip_comment(&String::from_utf8_lossy(&card[10..]));
                cards.push((key, value));
            }
        }
        let data_offset = pos.div_ceil(BLOCK) * BLOCK;
        Ok(Header { cards, data_offset })
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.cards
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            "T" => Some(true),
            "F" => Some(false),
            _ => None,
        }
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.get(key)?.parse().ok()
    }

    fn float(&self, key: &str) -> Option<f64> {
        // FITS allows Fortran-style 'D' exponents.
        self.get(key)?.replace(['D', 'd'], "E").parse().ok()
    }

    /// Any value as text: strings unquoted, numbers as written.
    fn value(&self, key: &str) -> Option<String> {
        let v = self.get(key)?;
        if v.starts_with('\'') {
            self.string(key)
        } else {
            Some(v.trim().to_string())
        }
        .filter(|s| !s.is_empty())
    }

    /// A quoted string value with the quotes removed and '' unescaped.
    fn string(&self, key: &str) -> Option<String> {
        let v = self.get(key)?;
        let inner = v.strip_prefix('\'')?.strip_suffix('\'').unwrap_or(v);
        Some(inner.replace("''", "'").trim_end().to_string())
    }

    fn axis(&self, n: u32) -> Result<u32> {
        let key = format!("NAXIS{n}");
        match self.int(&key) {
            Some(v) if v >= 0 => Ok(v as u32),
            Some(v) => bail!("{key} = {v} is negative."),
            None => bail!("{key} missing."),
        }
    }
}

/// Drop the "/ comment" part of a value field, respecting quoted strings.
fn strip_comment(field: &str) -> String {
    let field = field.trim();
    if let Some(rest) = field.strip_prefix('\'') {
        // Find the closing quote, skipping '' escapes.
        let b = rest.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\'' {
                if b.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                return field[..i + 2].to_string();
            }
            i += 1;
        }
        return field.to_string();
    }
    match field.find('/') {
        Some(idx) => field[..idx].trim().to_string(),
        None => field.to_string(),
    }
}
