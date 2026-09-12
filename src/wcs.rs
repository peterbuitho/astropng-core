//! Sky coordinates from an image header: either the plate solution (WCS
//! keywords, i.e. where the frame really points) or the mount's target
//! coordinates (`RA`/`DEC`, `OBJCTRA`/`OBJCTDEC`). Used to cross-check the
//! object name and to identify unnamed frames.

/// Where the image is centred on the sky.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyCoords {
    pub ra_deg: f64,
    pub dec_deg: f64,
    /// Half the image diagonal in degrees, when the pixel scale is known.
    pub fov_radius_deg: Option<f64>,
    /// True when derived from a plate solution (WCS) rather than the mount's
    /// intended target.
    pub solved: bool,
}

impl SkyCoords {
    /// How far a named object may sit from the image centre before we doubt
    /// the name: generous, because large targets are often framed off-centre
    /// and mosaics point at panels, not at the catalogue position.
    pub fn tolerance_deg(&self) -> f64 {
        2.0 + self.fov_radius_deg.unwrap_or(0.0)
    }

    /// Radius for "what is at these coordinates?" searches.
    pub fn search_radius_deg(&self) -> f64 {
        self.fov_radius_deg.unwrap_or(1.0).clamp(0.25, 2.0)
    }

    pub fn separation_deg(&self, ra_deg: f64, dec_deg: f64) -> f64 {
        separation_deg(self.ra_deg, self.dec_deg, ra_deg, dec_deg)
    }
}

/// Build coordinates from header keywords. `get` returns the trimmed,
/// unquoted value of a keyword (FITS card or XISF FITSKeyword / property).
/// `width`/`height` are the image size in pixels.
pub fn from_keywords(get: &dyn Fn(&str) -> Option<String>, width: u32, height: u32) -> Option<SkyCoords> {
    let num = |k: &str| get(k).and_then(|v| parse_number(&v));

    // --- Plate solution -----------------------------------------------------
    if let (Some(crval1), Some(crval2)) = (num("CRVAL1"), num("CRVAL2")) {
        let ctype_ok = get("CTYPE1").is_none_or(|t| t.to_ascii_uppercase().starts_with("RA"));
        if ctype_ok && (0.0..=360.0).contains(&crval1) && (-90.0..=90.0).contains(&crval2) {
            // Linear part of the WCS, degrees per pixel.
            let cd = match (num("CD1_1"), num("CD1_2"), num("CD2_1"), num("CD2_2")) {
                (Some(a), b, c, Some(d)) => Some([a, b.unwrap_or(0.0), c.unwrap_or(0.0), d]),
                _ => match (num("CDELT1"), num("CDELT2")) {
                    (Some(dx), Some(dy)) => {
                        let rot = num("CROTA2").unwrap_or(0.0).to_radians();
                        Some([
                            dx * rot.cos(),
                            -dy * rot.sin(),
                            dx * rot.sin(),
                            dy * rot.cos(),
                        ])
                    }
                    _ => None,
                },
            };

            let (mut ra, mut dec) = (crval1, crval2);
            let mut fov = None;
            if let Some([a, b, c, d]) = cd {
                let scale = (a * d - b * c).abs().sqrt();
                if scale > 0.0 && scale < 1.0 {
                    fov = Some(0.5 * scale * (width as f64).hypot(height as f64));
                }
                // Move from the reference pixel to the image centre (FITS
                // pixels are 1-based). Tangent-plane linear approximation is
                // plenty for a cross-check.
                if let (Some(crpix1), Some(crpix2)) = (num("CRPIX1"), num("CRPIX2")) {
                    let dx = (width as f64 + 1.0) / 2.0 - crpix1;
                    let dy = (height as f64 + 1.0) / 2.0 - crpix2;
                    let xi = a * dx + b * dy;
                    let eta = c * dx + d * dy;
                    let cos_dec = crval2.to_radians().cos().max(1e-6);
                    ra = (crval1 + xi / cos_dec).rem_euclid(360.0);
                    dec = (crval2 + eta).clamp(-90.0, 90.0);
                }
            }
            return Some(SkyCoords {
                ra_deg: ra,
                dec_deg: dec,
                fov_radius_deg: fov,
                solved: true,
            });
        }
    }

    // --- Mount / sequence target ---------------------------------------------
    let ra = get("OBJCTRA")
        .and_then(|v| parse_angle(&v, true))
        .or_else(|| get("RA").and_then(|v| parse_angle(&v, false)))
        .or_else(|| get("OBJRA").and_then(|v| parse_angle(&v, false)));
    let dec = get("OBJCTDEC")
        .and_then(|v| parse_angle(&v, false))
        .or_else(|| get("DEC").and_then(|v| parse_angle(&v, false)))
        .or_else(|| get("OBJDEC").and_then(|v| parse_angle(&v, false)));

    let (ra, dec) = (ra?, dec?);
    if !(0.0..=360.0).contains(&ra) || !(-90.0..=90.0).contains(&dec) {
        return None;
    }

    // Field of view from pixel size (µm) and focal length (mm), if given.
    let fov = match (num("XPIXSZ"), num("FOCALLEN")) {
        (Some(pix), Some(fl)) if pix > 0.0 && fl > 0.0 => {
            let bin = num("XBINNING").filter(|b| *b >= 1.0).unwrap_or(1.0);
            let arcsec_per_px = 206.265 * pix * bin / fl;
            Some(0.5 * arcsec_per_px / 3600.0 * (width as f64).hypot(height as f64))
        }
        _ => None,
    };

    Some(SkyCoords {
        ra_deg: ra,
        dec_deg: dec,
        fov_radius_deg: fov,
        solved: false,
    })
}

/// Parse a plain number (FITS allows Fortran 'D' exponents).
fn parse_number(s: &str) -> Option<f64> {
    s.trim().replace(['D', 'd'], "E").parse().ok()
}

/// Parse an angle in degrees. Accepts decimal degrees, or sexagesimal
/// ("05 35 17.3", "05:35:17", "+41 16 08", "-05d23m28s"). `hours` says a
/// sexagesimal (or bare decimal) value is in hours and must be scaled by 15.
pub fn parse_angle(s: &str, hours: bool) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let sexagesimal = s.contains(' ') || s.contains(':') || s.contains('h') || s.contains('d') || s.contains('m');
    if !sexagesimal {
        let v = parse_number(s)?;
        return Some(if hours { v * 15.0 } else { v });
    }

    let negative = s.starts_with('-');
    let parts: Vec<f64> = s
        .trim_start_matches(['+', '-'])
        .split([' ', ':', 'h', 'd', 'm', 's', '\'', '"'])
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<f64>().ok())
        .collect::<Option<Vec<f64>>>()?;
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut v = parts[0];
    if let Some(m) = parts.get(1) {
        v += m / 60.0;
    }
    if let Some(sec) = parts.get(2) {
        v += sec / 3600.0;
    }
    if negative {
        v = -v;
    }
    Some(if hours { v * 15.0 } else { v })
}

/// Great-circle separation in degrees (haversine).
pub fn separation_deg(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (ra1.to_radians(), dec1.to_radians(), ra2.to_radians(), dec2.to_radians());
    let s1 = ((dec2 - dec1) / 2.0).sin();
    let s2 = ((ra2 - ra1) / 2.0).sin();
    let h = s1 * s1 + dec1.cos() * dec2.cos() * s2 * s2;
    2.0 * h.sqrt().clamp(0.0, 1.0).asin().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn kw(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn angles() {
        assert!((parse_angle("05 35 17.3", true).unwrap() - 83.822083).abs() < 1e-4);
        assert!((parse_angle("05:35:17", true).unwrap() - 83.820833).abs() < 1e-4);
        assert!((parse_angle("-05 23 28", false).unwrap() + 5.391111).abs() < 1e-4);
        assert!((parse_angle("+41 16 08", false).unwrap() - 41.268889).abs() < 1e-4);
        assert_eq!(parse_angle("83.82", false), Some(83.82));
        assert_eq!(parse_angle("5.5", true), Some(82.5));
        assert_eq!(parse_angle("", false), None);
    }

    #[test]
    fn wcs_centre_and_fov() {
        // 2"/px, reference pixel at the image centre.
        let m = kw(&[
            ("CTYPE1", "RA---TAN"), ("CRVAL1", "314.7"), ("CRVAL2", "44.33"),
            ("CRPIX1", "400.5"), ("CRPIX2", "250.5"),
            ("CD1_1", "-0.000555556"), ("CD1_2", "0"), ("CD2_1", "0"), ("CD2_2", "0.000555556"),
        ]);
        let c = from_keywords(&|k| m.get(k).cloned(), 800, 500).unwrap();
        assert!(c.solved);
        assert!((c.ra_deg - 314.7).abs() < 1e-6 && (c.dec_deg - 44.33).abs() < 1e-6);
        // half diagonal: 0.5 * 2"/px * hypot(800,500) px = 943.4" = 0.262 deg
        assert!((c.fov_radius_deg.unwrap() - 0.262).abs() < 0.002);

        // Reference pixel at the corner: centre shifts by half the field.
        let m2 = kw(&[
            ("CRVAL1", "100.0"), ("CRVAL2", "0.0"), ("CRPIX1", "0.5"), ("CRPIX2", "0.5"),
            ("CDELT1", "-0.001"), ("CDELT2", "0.001"),
        ]);
        let c2 = from_keywords(&|k| m2.get(k).cloned(), 1000, 1000).unwrap();
        assert!((c2.ra_deg - 99.5).abs() < 1e-6 && (c2.dec_deg - 0.5).abs() < 1e-6);
    }

    #[test]
    fn target_coords() {
        let m = kw(&[("OBJCTRA", "00 42 44"), ("OBJCTDEC", "+41 16 08"), ("XPIXSZ", "3.76"), ("FOCALLEN", "400"), ("RA", "999")]);
        let c = from_keywords(&|k| m.get(k).cloned(), 6248, 4176).unwrap();
        assert!(!c.solved);
        assert!((c.ra_deg - 10.6833).abs() < 1e-3);
        assert!(c.fov_radius_deg.unwrap() > 1.9 && c.fov_radius_deg.unwrap() < 2.1);

        let m = kw(&[("RA", "83.82"), ("DEC", "-5.39")]);
        let c = from_keywords(&|k| m.get(k).cloned(), 100, 100).unwrap();
        assert_eq!((c.ra_deg, c.dec_deg, c.fov_radius_deg), (83.82, -5.39, None));

        assert!(from_keywords(&|_| None, 100, 100).is_none());
    }

    #[test]
    fn separation() {
        assert!(separation_deg(10.0, 40.0, 10.0, 40.0).abs() < 1e-9);
        assert!((separation_deg(0.0, 0.0, 1.0, 0.0) - 1.0).abs() < 1e-9);
        assert!((separation_deg(0.0, 89.0, 180.0, 89.0) - 2.0).abs() < 1e-6);
    }
}
