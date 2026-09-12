//! Minimal reader for the monolithic XISF 1.0 file format, sufficient to pull
//! the first image out of a PixInsight / NINA generated `.xisf` file.

use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use base64::Engine;

const SIGNATURE: &[u8; 8] = b"XISF0100";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
}

impl SampleFormat {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::UInt8 => 1,
            SampleFormat::UInt16 => 2,
            SampleFormat::UInt32 => 4,
            SampleFormat::UInt64 => 8,
            SampleFormat::Float32 => 4,
            SampleFormat::Float64 => 8,
        }
    }
}

/// Decoded pixel payload of the first `<Image>` element in an XISF file,
/// plus the metadata needed to interpret the raw bytes.
pub struct XisfImageData {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub format: SampleFormat,
    /// Planar = channel-major storage; otherwise interleaved ("Normal").
    pub planar: bool,
    /// True when samples are stored big-endian.
    pub big_endian: bool,
    /// Raw, decompressed, un-shuffled sample bytes.
    pub raw_data: Vec<u8>,
    /// Target name from the header (FITS `OBJECT` keyword or the XISF
    /// `Observation:Object:Name` property), if present.
    pub object: Option<String>,
    /// Image centre from the plate solution or the mount target, if present.
    pub coords: Option<crate::wcs::SkyCoords>,
}

#[derive(Debug)]
pub struct XisfError(pub String);

impl fmt::Display for XisfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for XisfError {}

macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(XisfError(format!($($arg)*)))
    };
}

type Result<T> = std::result::Result<T, XisfError>;

pub fn read(path: &Path) -> Result<XisfImageData> {
    let file = fs::read(path).map_err(|e| XisfError(format!("cannot read file: {e}")))?;

    // --- Fixed 16-byte prologue ---
    if file.len() < 16 || &file[0..8] != SIGNATURE {
        bail!("Not an XISF 1.0 file (bad signature).");
    }
    let header_length = u32::from_le_bytes([file[8], file[9], file[10], file[11]]) as usize;
    // bytes 12..15 reserved

    let xml_start = 16;
    let xml_end = xml_start + header_length;
    if file.len() < xml_end {
        bail!("Truncated XISF header.");
    }
    let mut xml_bytes = &file[xml_start..xml_end];
    // Strip trailing NUL padding before parsing.
    while let Some((&0, rest)) = xml_bytes.split_last() {
        xml_bytes = rest;
    }
    let xml = std::str::from_utf8(xml_bytes)
        .map_err(|e| XisfError(format!("Invalid XISF XML header: {e}")))?;

    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| XisfError(format!("Invalid XISF XML header: {e}")))?;

    let image = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Image")
        .ok_or_else(|| XisfError("No <Image> element in XISF header.".into()))?;

    let attr = |name: &str| image.attribute(name);

    // --- Header metadata for the stamp: target name and coordinates ---
    // FITS keywords carried over by the capture software, then XISF's own
    // properties as a fallback.
    let fits_keyword = |key: &str| -> Option<String> {
        image
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "FITSKeyword")
            .find(|n| n.attribute("name").is_some_and(|k| k.trim().eq_ignore_ascii_case(key)))
            .and_then(|n| n.attribute("value"))
            .map(|v| v.trim().trim_matches('\'').trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let property = |id: &str| -> Option<String> {
        doc.descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "Property" && n.attribute("id") == Some(id))
            .and_then(|n| n.text().map(str::trim).map(str::to_string).or_else(|| n.attribute("value").map(str::to_string)))
            .filter(|s| !s.is_empty())
    };

    let object = fits_keyword("OBJECT").or_else(|| property("Observation:Object:Name"));

    // --- Geometry ---
    let geometry = attr("geometry")
        .ok_or_else(|| XisfError("<Image> missing geometry attribute.".into()))?;
    let geo: std::result::Result<Vec<i64>, _> =
        geometry.split(':').map(|s| s.parse::<i64>()).collect();
    let geo = geo.map_err(|_| XisfError(format!("Invalid geometry '{geometry}'.")))?;
    if geo.len() < 3 {
        bail!("Unsupported geometry '{geometry}' (expected w:h:channels).");
    }
    let width = geo[0];
    let height = geo[1];
    let channels = geo[geo.len() - 1];
    if width <= 0 || height <= 0 || channels <= 0 {
        bail!("Invalid geometry '{geometry}'.");
    }
    let width = width as u32;
    let height = height as u32;
    let channels = channels as u32;

    let coords = crate::wcs::from_keywords(
        &|key| {
            fits_keyword(key).or_else(|| match key {
                "RA" => property("Observation:Center:RA"),
                "DEC" => property("Observation:Center:Dec"),
                _ => None,
            })
        },
        width,
        height,
    );

    // --- Sample format ---
    let sample_format = attr("sampleFormat").unwrap_or("UInt16");
    let format = match sample_format {
        "UInt8" => SampleFormat::UInt8,
        "UInt16" => SampleFormat::UInt16,
        "UInt32" => SampleFormat::UInt32,
        "UInt64" => SampleFormat::UInt64,
        "Float32" => SampleFormat::Float32,
        "Float64" => SampleFormat::Float64,
        other => bail!("Unsupported sampleFormat '{other}'."),
    };

    let planar = !attr("pixelStorage").is_some_and(|v| v.eq_ignore_ascii_case("Normal"));
    let big_endian = attr("byteOrder").is_some_and(|v| v.eq_ignore_ascii_case("big"));

    let expected_samples = width as u64 * height as u64 * channels as u64;
    let expected_bytes = expected_samples * format.bytes_per_sample() as u64;

    // --- Locate + decode payload ---
    let location = attr("location")
        .ok_or_else(|| XisfError("<Image> missing location attribute.".into()))?;

    // compression may sit on the <Image> or on an embedded <Data> child.
    let mut compression = attr("compression").map(str::to_owned);

    let loc: Vec<&str> = location.split(':').collect();
    let mut payload: Vec<u8> = match loc[0] {
        "attachment" => {
            if loc.len() < 3 {
                bail!("Malformed attachment location '{location}'.");
            }
            let position: usize = loc[1]
                .parse()
                .map_err(|_| XisfError(format!("Malformed attachment location '{location}'.")))?;
            let size: usize = loc[2]
                .parse()
                .map_err(|_| XisfError(format!("Malformed attachment location '{location}'.")))?;
            let end = position
                .checked_add(size)
                .filter(|&e| e <= file.len())
                .ok_or_else(|| XisfError("Attachment extends past end of file.".into()))?;
            file[position..end].to_vec()
        }
        "embedded" => {
            let data = image
                .children()
                .find(|n| n.is_element() && n.tag_name().name() == "Data")
                .ok_or_else(|| XisfError("Embedded location but no <Data> child.".into()))?;
            if compression.is_none() {
                compression = data.attribute("compression").map(str::to_owned);
            }
            let text = data.text().unwrap_or("");
            decode_text(text, data.attribute("encoding").unwrap_or("base64"))?
        }
        "inline" => {
            let encoding = loc.get(1).copied().unwrap_or("base64");
            let text = image.text().unwrap_or("");
            decode_text(text, encoding)?
        }
        other => bail!("Unsupported location kind '{other}'."),
    };

    // --- Decompress + un-shuffle ---
    if let Some(spec) = compression.as_deref().filter(|s| !s.is_empty()) {
        payload = decompress(&payload, spec)?;
    }

    if (payload.len() as u64) < expected_bytes {
        bail!(
            "Pixel data too small: got {} bytes, expected {}.",
            payload.len(),
            expected_bytes
        );
    }

    Ok(XisfImageData {
        object,
        coords,
        width,
        height,
        channels,
        format,
        planar,
        big_endian,
        raw_data: payload,
    })
}

fn decode_text(text: &str, encoding: &str) -> Result<Vec<u8>> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    match encoding.to_ascii_lowercase().as_str() {
        "base64" => base64::engine::general_purpose::STANDARD
            .decode(cleaned.as_bytes())
            .map_err(|e| XisfError(format!("Invalid base64 data: {e}"))),
        "hex" => hex::decode(cleaned.as_bytes())
            .map_err(|e| XisfError(format!("Invalid hex data: {e}"))),
        other => bail!("Unsupported data encoding '{other}'."),
    }
}

fn decompress(input: &[u8], compression: &str) -> Result<Vec<u8>> {
    // grammar: codec[+sh]:uncompressedSize[:shuffleItemSize]
    let parts: Vec<&str> = compression.split(':').collect();
    if parts.len() < 2 {
        bail!("Malformed compression spec '{compression}'.");
    }
    let codec_spec = parts[0].to_ascii_lowercase();
    let uncompressed_size: usize = parts[1]
        .parse()
        .map_err(|_| XisfError(format!("Malformed compression spec '{compression}'.")))?;

    let shuffled = codec_spec.ends_with("+sh");
    let codec = if shuffled {
        &codec_spec[..codec_spec.len() - 3]
    } else {
        codec_spec.as_str()
    };
    let shuffle_item_size: usize = match parts.get(2) {
        Some(s) => s
            .parse()
            .map_err(|_| XisfError(format!("Malformed compression spec '{compression}'.")))?,
        None => 1,
    };

    let mut output: Vec<u8> = match codec {
        // Check lz4hc before lz4 — but both decode with the same block decoder.
        "lz4" | "lz4hc" => lz4_flex::block::decompress(input, uncompressed_size)
            .map_err(|e| XisfError(format!("LZ4 decode failed: {e}")))?,
        "zlib" => {
            let mut d = flate2::read::ZlibDecoder::new(input);
            let mut buf = Vec::with_capacity(uncompressed_size);
            d.read_to_end(&mut buf)
                .map_err(|e| XisfError(format!("zlib decode failed: {e}")))?;
            buf
        }
        "zstd" => {
            let mut d = ruzstd::StreamingDecoder::new(input)
                .map_err(|e| XisfError(format!("zstd decode failed: {e}")))?;
            let mut buf = Vec::with_capacity(uncompressed_size);
            d.read_to_end(&mut buf)
                .map_err(|e| XisfError(format!("zstd decode failed: {e}")))?;
            buf
        }
        other => bail!("Unsupported compression codec '{other}'."),
    };

    if output.len() != uncompressed_size {
        bail!(
            "{codec} decode produced {} bytes, expected {}.",
            output.len(),
            uncompressed_size
        );
    }

    if shuffled && shuffle_item_size > 1 {
        output = unshuffle(&output, shuffle_item_size);
    }

    Ok(output)
}

/// Reverse the XISF byte-shuffle: the compressed stream stores byte 0 of every
/// item, then byte 1 of every item, etc. Transpose it back.
fn unshuffle(input: &[u8], item_size: usize) -> Vec<u8> {
    let items = input.len() / item_size;
    let mut output = vec![0u8; input.len()];
    let mut p = 0usize;
    for b in 0..item_size {
        for i in 0..items {
            output[i * item_size + b] = input[p];
            p += 1;
        }
    }
    // trailing bytes that don't fill a whole item (shouldn't happen) copied as-is
    for k in (items * item_size)..input.len() {
        output[k] = input[k];
    }
    output
}
