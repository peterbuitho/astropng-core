//! Post-processing: resize to exactly 3840x2160 (cover + center crop) and
//! stamp the file name in the bottom-right corner. Pure Rust: the `image`
//! crate does the scaling and `ab_glyph` rasterises a bundled font, so no
//! external tools are needed on any platform.

use std::path::Path;

use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use image::{DynamicImage, ImageBuffer, Pixel};

pub const TARGET_WIDTH: u32 = 3840;
pub const TARGET_HEIGHT: u32 = 2160;

/// Title size in pixels (matches ImageMagick's former `-pointsize 48`).
const TITLE_PX: f32 = 48.0;
/// Size of the optional second line (catalogue ids, type, coordinates).
const SUBTITLE_PX: f32 = 30.0;
/// Vertical gap between the two lines.
const LINE_GAP: f32 = 14.0;
/// Distance of the label from the right edge and bottom edge, in pixels.
const MARGIN_RIGHT: f32 = 60.0;
const MARGIN_BOTTOM: f32 = 120.0;

/// What gets stamped on the image: a title (object name or file name) and an
/// optional smaller line underneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub title: String,
    pub subtitle: Option<String>,
}

impl Label {
    /// A single-line label.
    pub fn plain(title: impl Into<String>) -> Self {
        Label {
            title: title.into(),
            subtitle: None,
        }
    }
}
/// Soft dark shadow offset behind the white text, so the label stays readable
/// over bright nebulosity.
const SHADOW_OFFSET: f32 = 2.0;
const SHADOW_OPACITY: f32 = 0.7;

/// DejaVu Sans Condensed Bold - a close, freely redistributable stand-in for
/// the condensed gothic look originally intended. License: assets/fonts/LICENSE-DejaVu.txt
static BUNDLED_FONT: &[u8] = include_bytes!("../assets/fonts/DejaVuSansCondensed-Bold.ttf");

/// The embedded stamp font, also useful as a glyph fallback for the GUI
/// (arrows, degree signs, primes) since it covers far more of Unicode than
/// egui's default fonts.
pub fn bundled_font_bytes() -> &'static [u8] {
    BUNDLED_FONT
}

/// Holds the label font; create once and reuse for every image.
pub struct Stamper {
    font: FontArc,
}

impl Stamper {
    /// Use the bundled font.
    pub fn bundled() -> Self {
        let font = FontArc::try_from_slice(BUNDLED_FONT).expect("bundled font is valid");
        Stamper { font }
    }

    /// Use a TrueType / OpenType font file supplied by the user.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("cannot read font file {}: {e}", path.display()))?;
        let font = FontArc::try_from_vec(bytes)
            .map_err(|_| format!("{} is not a valid TrueType/OpenType font", path.display()))?;
        Ok(Stamper { font })
    }

    /// Scale `img` (up or down, aspect ratio kept) so it covers 3840x2160,
    /// center-crop the overflow to exactly 3840x2160, and draw `label` in the
    /// bottom-right corner. Mono images stay mono; RGB stays RGB; anything
    /// else is converted to RGB.
    pub fn resize_and_label(&self, img: DynamicImage, label: &Label) -> DynamicImage {
        let resized = img.resize_to_fill(
            TARGET_WIDTH,
            TARGET_HEIGHT,
            image::imageops::FilterType::Lanczos3,
        );

        match resized {
            DynamicImage::ImageLuma8(mut buf) => {
                self.draw_label(&mut buf, label);
                DynamicImage::ImageLuma8(buf)
            }
            DynamicImage::ImageRgb8(mut buf) => {
                self.draw_label(&mut buf, label);
                DynamicImage::ImageRgb8(buf)
            }
            other => {
                let mut buf = other.to_rgb8();
                self.draw_label(&mut buf, label);
                DynamicImage::ImageRgb8(buf)
            }
        }
    }

    /// Draw the label right-aligned in the bottom-right corner: the subtitle
    /// (if any) sits on the original single-line position and the title goes
    /// above it, so nothing moves closer to the bottom edge than before.
    fn draw_label<P>(&self, img: &mut ImageBuffer<P, Vec<u8>>, label: &Label)
    where
        P: Pixel<Subpixel = u8>,
    {
        let right = img.width() as f32 - MARGIN_RIGHT;
        let bottom = img.height() as f32 - MARGIN_BOTTOM;
        let max_width = img.width() as f32 - 2.0 * MARGIN_RIGHT;

        let title_scaled = self.font.as_scaled(PxScale::from(TITLE_PX));
        let mut title_baseline = bottom - title_scaled.descent().abs();

        if let Some(sub) = label.subtitle.as_deref().filter(|s| !s.trim().is_empty()) {
            let sub_scaled = self.font.as_scaled(PxScale::from(SUBTITLE_PX));
            let sub_baseline = bottom - sub_scaled.descent().abs();
            self.draw_line(img, sub, SUBTITLE_PX, right, sub_baseline, max_width);
            title_baseline = sub_baseline - sub_scaled.ascent() - LINE_GAP;
        }

        if !label.title.trim().is_empty() {
            self.draw_line(img, &label.title, TITLE_PX, right, title_baseline, max_width);
        }
    }

    /// Draw one line of text with its right edge at `right` and its baseline
    /// at `baseline`, shrinking the font if it would exceed `max_width`.
    fn draw_line<P>(
        &self,
        img: &mut ImageBuffer<P, Vec<u8>>,
        text: &str,
        px: f32,
        right: f32,
        baseline: f32,
        max_width: f32,
    ) where
        P: Pixel<Subpixel = u8>,
    {
        let (mut glyphs, mut width, mut scale) = self.layout(text, px);
        if width > max_width && width > 0.0 {
            let shrunk = px * max_width / width;
            (glyphs, width, scale) = self.layout(text, shrunk);
        }
        let x0 = right - width;

        // Shadow first, then the white text on top.
        let origin = point(x0, baseline);
        let shadow_origin = point(x0 + SHADOW_OFFSET, baseline + SHADOW_OFFSET);
        self.blit(img, &glyphs, scale, shadow_origin, 0, SHADOW_OPACITY);
        self.blit(img, &glyphs, scale, origin, 255, 1.0);
    }

    /// Lay out glyphs left-to-right from x = 0; returns (glyph id + x offset,
    /// total width, scale).
    fn layout(&self, text: &str, px: f32) -> (Vec<(ab_glyph::GlyphId, f32)>, f32, PxScale) {
        let scale = PxScale::from(px);
        let scaled = self.font.as_scaled(scale);
        let mut glyphs = Vec::with_capacity(text.len());
        let mut x = 0.0f32;
        let mut prev = None;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            glyphs.push((id, x));
            x += scaled.h_advance(id);
            prev = Some(id);
        }
        (glyphs, x, scale)
    }

    /// Rasterise `glyphs` (id + x offset from `origin`, whose y is the
    /// baseline) into `img`, blending every channel towards `value`.
    fn blit<P>(
        &self,
        img: &mut ImageBuffer<P, Vec<u8>>,
        glyphs: &[(ab_glyph::GlyphId, f32)],
        scale: PxScale,
        origin: ab_glyph::Point,
        value: u8,
        opacity: f32,
    ) where
        P: Pixel<Subpixel = u8>,
    {
        let (w, h) = (img.width() as i64, img.height() as i64);
        for &(id, dx) in glyphs {
            let glyph = id.with_scale_and_position(scale, point(origin.x + dx, origin.y));
            let Some(outlined) = self.font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i64 + gx as i64;
                let py = bounds.min.y as i64 + gy as i64;
                if px < 0 || py < 0 || px >= w || py >= h {
                    return;
                }
                let a = (coverage * opacity).clamp(0.0, 1.0);
                if a <= 0.0 {
                    return;
                }
                let pixel = img.get_pixel_mut(px as u32, py as u32);
                for c in pixel.channels_mut() {
                    let src = *c as f32;
                    *c = (src + (value as f32 - src) * a).round().clamp(0.0, 255.0) as u8;
                }
            });
        }
    }
}
