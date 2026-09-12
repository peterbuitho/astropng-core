//! Identify the object in an image and fetch its catalogue information from
//! the CDS Sesame name resolver (backed by SIMBAD), to stamp a proper name
//! like "Andromeda Galaxy (M 31)" instead of the bare file name.
//!
//! Sources of identity, in order of trust:
//! 1. the `OBJECT` keyword from the FITS / XISF header (what the capture
//!    software was told the target was);
//! 2. a catalogue designation found in the file name (`M31`, `NGC_7000`,
//!    `Sh2-155`, ...).
//!
//! When both exist they are cross-checked against the resolved object's alias
//! list; on disagreement the file name wins (it is what the user named the
//! image) and a note is attached. If nothing resolves, the file name is used
//! verbatim.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::post::Label;
use crate::wcs::SkyCoords;

const SESAME_URL: &str = "https://cds.unistra.fr/cgi-bin/nph-sesame/-oxI/S?";
const TAP_URL: &str = "https://simbad.cds.unistra.fr/simbad/sim-tap/sync";

/// SIMBAD object types worth stamping when identifying a frame by its
/// coordinates: deep-sky objects, not the thousands of stars in any field.
const DSO_TYPES: &[&str] = &[
    "G", "AGN", "GiG", "GiP", "GiC", "IG", "PaG", "GrG", "ClG", "SBG", "EmG", "LIN", "SyG", "Sy1",
    "Sy2", "HII", "PN", "SNR", "RNe", "DNe", "GNe", "MoC", "Cld", "ISM", "EmO", "bub", "OpC",
    "GlC", "Cl*", "As*", "SFR", "glb",
];

/// Nebula types: what astrophotographers actually mean when they point at a
/// cluster embedded in one (NGC 7380 = the Wizard Nebula, NGC 2244 = the
/// Rosette). SIMBAD files the cluster and the nebula as separate objects.
const NEBULA_TYPES: &[&str] = &[
    "HII", "RNe", "DNe", "GNe", "MoC", "Cld", "ISM", "EmO", "bub", "SNR", "SFR", "PN",
];

/// Objects whose SIMBAD entry may be "the cluster" while the picture is of the
/// surrounding nebula: worth looking for a named companion nebula.
const COMPANION_HOST_TYPES: &[&str] = &[
    "OpC", "Cl*", "As*", "SFR", "HII", "GNe", "ISM", "Cld", "MoC", "EmO", "RNe", "DNe",
];

/// How far a companion nebula's catalogue position may sit from the cluster's.
const COMPANION_RADIUS_DEG: f64 = 0.5;

/// Catalogues we recognise in file names and prefer when presenting aliases.
/// Order = display priority.
struct Catalog {
    /// Upper-case prefixes accepted in file names.
    keys: &'static [&'static str],
    /// How we print it: prefix followed by the number.
    pretty: &'static str,
    /// Prefixes SIMBAD uses in its alias list (case-insensitive, whitespace
    /// collapsed), followed by the number.
    simbad: &'static [&'static str],
    /// Highest valid number (sanity filter against exposure times etc.).
    max: u32,
    /// Single-letter catalogues: the number must follow the letter directly
    /// ("M31", not "M_31") so that filter letters like `_B_` are not mistaken
    /// for Barnard designations.
    adjacent_only: bool,
}

const CATALOGS: &[Catalog] = &[
    Catalog { keys: &["M", "MESSIER"], pretty: "M ", simbad: &["M "], max: 110, adjacent_only: true },
    // Caldwell is not in SIMBAD: no alias prefixes; numbers come from the
    // built-in table in `catalog.rs` and queries are translated there.
    Catalog { keys: &["C", "CALDWELL"], pretty: "C ", simbad: &[], max: 109, adjacent_only: true },
    Catalog { keys: &["NGC"], pretty: "NGC ", simbad: &["NGC "], max: 7840, adjacent_only: false },
    Catalog { keys: &["IC"], pretty: "IC ", simbad: &["IC "], max: 5386, adjacent_only: false },
    Catalog { keys: &["SH2", "SH"], pretty: "Sh2-", simbad: &["SH 2-", "SH2-"], max: 313, adjacent_only: false },
    Catalog { keys: &["B", "BARNARD"], pretty: "Barnard ", simbad: &["Barnard "], max: 370, adjacent_only: true },
    Catalog { keys: &["LBN"], pretty: "LBN ", simbad: &["LBN "], max: 1125, adjacent_only: false },
    Catalog { keys: &["LDN"], pretty: "LDN ", simbad: &["LDN "], max: 1802, adjacent_only: false },
    Catalog { keys: &["VDB"], pretty: "vdB ", simbad: &["VdB ", "vdB "], max: 158, adjacent_only: false },
    Catalog { keys: &["CR", "COLLINDER"], pretty: "Cr ", simbad: &["Cr ", "Cl Collinder "], max: 471, adjacent_only: false },
    Catalog { keys: &["MEL", "MELOTTE"], pretty: "Mel ", simbad: &["Cl Melotte ", "Mel "], max: 245, adjacent_only: false },
    Catalog { keys: &["CED", "CEDERBLAD"], pretty: "Ced ", simbad: &["Ced "], max: 215, adjacent_only: false },
    Catalog { keys: &["ARP"], pretty: "Arp ", simbad: &["APG ", "Arp "], max: 338, adjacent_only: false },
    Catalog { keys: &["UGC"], pretty: "UGC ", simbad: &["UGC "], max: 12921, adjacent_only: false },
    Catalog { keys: &["PGC"], pretty: "PGC ", simbad: &["LEDA ", "PGC "], max: 9_999_999, adjacent_only: false },
    Catalog { keys: &["HD"], pretty: "HD ", simbad: &["HD "], max: 359083, adjacent_only: false },
    Catalog { keys: &["HIP"], pretty: "HIP ", simbad: &["HIP "], max: 120404, adjacent_only: false },
];

/// What Sesame told us about one object.
#[derive(Debug, Clone)]
pub struct ObjectInfo {
    /// SIMBAD main identifier, whitespace collapsed (e.g. "M 31").
    pub main_id: String,
    /// Best common name, if any (e.g. "Andromeda Galaxy").
    pub common_name: Option<String>,
    /// Recognised catalogue designations, pretty-printed, in display priority.
    pub designations: Vec<String>,
    /// Every alias, normalised for comparison.
    aliases_norm: HashSet<String>,
    /// SIMBAD object type code (e.g. "G", "HII", "OpC").
    pub otype: String,
    /// Hubble morphological type for galaxies (e.g. "SAB(s)cd"), if known.
    pub morph_type: Option<String>,
    pub ra_deg: Option<f64>,
    pub dec_deg: Option<f64>,
}

impl ObjectInfo {
    /// Does `designation` (any spacing/case) refer to this object? Checks
    /// genuine SIMBAD aliases, plus the built-in Caldwell number (kept out of
    /// `aliases_norm` itself: a Caldwell entry can list *several* physical
    /// objects, e.g. C14 = NGC 869 & NGC 884, and folding it into the alias
    /// set would make [`Self::same_object`] treat those two as one object).
    pub fn matches(&self, designation: &str) -> bool {
        let norm = normalize(designation);
        self.aliases_norm.contains(&norm)
            || crate::catalog::caldwell_number(&self.designations)
                .is_some_and(|n| norm == normalize(&format!("C {n}")))
    }

    /// Do the two records describe the same object (share any genuine SIMBAD
    /// identifier)? Deliberately ignores the Caldwell number for the reason
    /// given on [`Self::matches`]: two different members of one multi-object
    /// Caldwell entry must not compare equal here.
    pub fn same_object(&self, other: &ObjectInfo) -> bool {
        self.aliases_norm.intersection(&other.aliases_norm).next().is_some()
    }

    /// Notable enough to override a name the user gave: Messier, Caldwell,
    /// NGC, IC, Sharpless or Barnard, or anything with a common name. An
    /// LBN/LDN entry near the pointing position is not evidence the user
    /// mislabelled the image.
    fn is_notable(&self) -> bool {
        self.prominence() <= 5 || self.common_name.is_some()
    }

    /// Prominence tier for ranking cone-search hits: 0 = Messier ... n =
    /// lesser catalogues, then "has a common name", then obscure.
    fn prominence(&self) -> usize {
        for (tier, cat) in CATALOGS.iter().enumerate() {
            if self.designations.iter().any(|d| d.starts_with(cat.pretty)) {
                return tier;
            }
        }
        if self.common_name.is_some() {
            return CATALOGS.len();
        }
        usize::MAX
    }

    /// Build from a SIMBAD TAP row: main_id, '|'-separated ids, otype,
    /// morphological type, ra, dec.
    fn from_tap(
        main_id: &str,
        ids: &str,
        otype: &str,
        morph: Option<&str>,
        ra: Option<f64>,
        dec: Option<f64>,
    ) -> ObjectInfo {
        let main_id = collapse_ws(main_id);
        let main_id = main_id.strip_prefix("NAME ").map(str::to_string).unwrap_or(main_id);
        let aliases: Vec<String> = ids
            .split('|')
            .map(collapse_ws)
            .filter(|s| !s.is_empty())
            .chain(std::iter::once(main_id.clone()))
            .collect();
        ObjectInfo::from_aliases(main_id, aliases, otype.trim().to_string(), clean_morph(morph), ra, dec)
    }

    fn from_aliases(
        main_id: String,
        aliases: Vec<String>,
        otype: String,
        morph_type: Option<String>,
        ra_deg: Option<f64>,
        dec_deg: Option<f64>,
    ) -> ObjectInfo {
        let mut designations = Vec::new();
        for cat in CATALOGS {
            for alias in &aliases {
                if let Some(d) = catalog_designation(cat, alias) {
                    if !designations.contains(&d) {
                        designations.push(d);
                    }
                }
            }
        }
        let aliases_norm: HashSet<String> = aliases.iter().map(|a| normalize(a)).collect();

        // Caldwell number from the built-in table, slotted in right after
        // any Messier id so it shows up early in the second line. Not added
        // to `aliases_norm` - see the comment on `matches`/`same_object`.
        if let Some(n) = crate::catalog::caldwell_number(&designations) {
            let c = format!("C {n}");
            if !designations.contains(&c) {
                let pos = designations.iter().take_while(|d| d.starts_with("M ")).count();
                designations.insert(pos, c);
            }
        }

        // Curated / user names beat SIMBAD's "NAME" aliases, which are often
        // missing or the less common variant.
        let common_name =
            crate::catalog::popular_name(&designations).or_else(|| pick_common_name(&aliases));

        ObjectInfo {
            main_id,
            common_name,
            designations,
            aliases_norm,
            otype,
            morph_type,
            ra_deg,
            dec_deg,
        }
    }

    /// Plain-words type. For galaxies the Hubble morphology wins ("Spiral
    /// galaxy") over SIMBAD's activity class ("Galaxy (active nucleus)"),
    /// which is what a picture of NGC 2403 is about.
    pub fn type_description(&self) -> String {
        if is_galaxy_type(&self.otype) {
            if let Some(m) = self.morph_type.as_deref().and_then(morphology_description) {
                return m.to_string();
            }
        }
        otype_description(&self.otype).to_string()
    }

    /// "RA 00h 42m 44s  Dec +41° 16′ 08″", or empty if unknown.
    pub fn coordinates(&self) -> String {
        match (self.ra_deg, self.dec_deg) {
            (Some(ra), Some(dec)) => format!("RA {}  Dec {}", fmt_ra(ra), fmt_dec(dec)),
            _ => String::new(),
        }
    }
}

/// Resolves names online, caching every answer for the duration of a run so
/// a folder of 300 subs of the same target costs one request.
pub struct Resolver {
    enabled: bool,
    agent: Option<ureq::Agent>,
    cache: HashMap<String, Option<ObjectInfo>>,
    /// Cone-search results keyed by rounded position + radius.
    nearby_cache: HashMap<String, Option<ObjectInfo>>,
    /// Set (and lookups disabled) after the first network failure.
    pub failure: Option<String>,
}

impl Resolver {
    pub fn new(enabled: bool) -> Self {
        let agent = enabled.then(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(12)))
                .user_agent(format!(
                    "xisf2png/{} (+https://github.com/peterbuitho/xisf2png)",
                    env!("CARGO_PKG_VERSION")
                ))
                .build()
                .new_agent()
        });
        Resolver {
            enabled,
            agent,
            cache: HashMap::new(),
            nearby_cache: HashMap::new(),
            failure: None,
        }
    }

    /// The most prominent deep-sky object within `radius_deg` of a position,
    /// or `None` if there is nothing with a recognised catalogue id or a
    /// common name there.
    pub fn nearby(&mut self, ra_deg: f64, dec_deg: f64, radius_deg: f64) -> Option<&ObjectInfo> {
        self.cone(ra_deg, dec_deg, radius_deg, ConeKind::AnyDso)
    }

    /// The most prominent *named* nebula within `radius_deg` of a position.
    pub fn nearby_named_nebula(&mut self, ra_deg: f64, dec_deg: f64, radius_deg: f64) -> Option<&ObjectInfo> {
        self.cone(ra_deg, dec_deg, radius_deg, ConeKind::NamedNebula)
    }

    fn cone(&mut self, ra_deg: f64, dec_deg: f64, radius_deg: f64, kind: ConeKind) -> Option<&ObjectInfo> {
        if !self.enabled() {
            return None;
        }
        // 0.01 deg ~ 36" buckets: frames of one target share a lookup.
        let key = format!("{kind:?}|{ra_deg:.2}|{dec_deg:.2}|{radius_deg:.2}");
        if !self.nearby_cache.contains_key(&key) {
            let agent = self.agent.as_ref()?;
            match cone_search(agent, ra_deg, dec_deg, radius_deg, kind) {
                Ok(info) => {
                    self.nearby_cache.insert(key.clone(), info);
                }
                Err(e) => {
                    self.failure = Some(e);
                    return None;
                }
            }
        }
        self.nearby_cache.get(&key).and_then(|o| o.as_ref())
    }

    /// If `info` is a cluster/nebula without a common name, borrow the name
    /// (and ids, and type) of the named nebula it sits in, if SIMBAD has one
    /// at the same position. "NGC 7380" becomes "Wizard Nebula (NGC 7380)".
    fn adopt_companion_nebula(&mut self, info: &mut ObjectInfo) {
        if info.common_name.is_some() || !COMPANION_HOST_TYPES.contains(&info.otype.trim_end_matches('?')) {
            return;
        }
        let (Some(ra), Some(dec)) = (info.ra_deg, info.dec_deg) else {
            return;
        };
        let Some(neb) = self.nearby_named_nebula(ra, dec, COMPANION_RADIUS_DEG).cloned() else {
            return;
        };
        if neb.same_object(info) {
            return;
        }
        info.common_name = neb.common_name;
        for d in neb.designations {
            if !info.designations.contains(&d) {
                info.designations.push(d);
            }
        }
        info.aliases_norm.extend(neb.aliases_norm);
        info.otype = neb.otype;
    }

    pub fn enabled(&self) -> bool {
        self.enabled && self.failure.is_none()
    }

    /// Look a name up. `None` when disabled, not found, or after a network
    /// failure (which is recorded in `failure`).
    pub fn resolve(&mut self, query: &str) -> Option<&ObjectInfo> {
        if !self.enabled() {
            return None;
        }
        let key = normalize(query);
        if key.is_empty() {
            return None;
        }
        // "C 7" means Caldwell 7 to an astrophotographer; SIMBAD would not
        // know. Look up the underlying NGC/IC object instead.
        let query = crate::catalog::caldwell_target(query).unwrap_or(query);
        if !self.cache.contains_key(&key) {
            let agent = self.agent.as_ref()?;
            let mut result = fetch(agent, query);
            // Sesame does not take every abbreviation we use ("Cr 399" is
            // "Collinder 399" to it); retry with the spelled-out catalogue.
            if matches!(result, Ok(None)) {
                if let Some(alt) = spelled_out(query) {
                    result = fetch(agent, &alt);
                }
            }
            match result {
                Ok(info) => {
                    self.cache.insert(key.clone(), info);
                }
                Err(e) => {
                    self.failure = Some(e);
                    return None;
                }
            }
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }
}

/// Alternative spelling of a designation for the name resolver, if the
/// abbreviated form we print is not one it accepts.
fn spelled_out(query: &str) -> Option<String> {
    let q = collapse_ws(query);
    let (prefix, rest) = q.split_once(' ')?;
    let long = match prefix.to_ascii_uppercase().as_str() {
        "CR" => "Collinder",
        "MEL" => "Melotte",
        "CED" => "Cederblad",
        "B" => "Barnard",
        _ => return None,
    };
    Some(format!("{long} {rest}"))
}

fn fetch(agent: &ureq::Agent, query: &str) -> Result<Option<ObjectInfo>, String> {
    let url = format!("{SESAME_URL}{}", percent_encode(query.trim()));
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| format!("Sesame request failed: {e}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Sesame response unreadable: {e}"))?;
    parse_sesame(&body)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConeKind {
    /// Any deep-sky object with a recognised catalogue id or a common name.
    AnyDso,
    /// Nebulae that have a common ("NAME ...") alias.
    NamedNebula,
}

/// SIMBAD TAP cone search, ranked by prominence.
fn cone_search(agent: &ureq::Agent, ra: f64, dec: f64, radius: f64, kind: ConeKind) -> Result<Option<ObjectInfo>, String> {
    let type_list = match kind {
        ConeKind::AnyDso => DSO_TYPES,
        ConeKind::NamedNebula => NEBULA_TYPES,
    };
    let types = type_list
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",");
    let name_filter = match kind {
        ConeKind::AnyDso => "",
        ConeKind::NamedNebula => " AND i.ids LIKE '%NAME %'",
    };
    let adql = format!(
        "SELECT TOP 400 b.main_id, b.otype, b.ra, b.dec, \
         DISTANCE(POINT('ICRS', b.ra, b.dec), POINT('ICRS', {ra:.6}, {dec:.6})) AS d, i.ids, b.morph_type \
         FROM basic AS b JOIN ids AS i ON i.oidref = b.oid \
         WHERE CONTAINS(POINT('ICRS', b.ra, b.dec), CIRCLE('ICRS', {ra:.6}, {dec:.6}, {radius:.4})) = 1 \
         AND b.otype IN ({types}){name_filter} ORDER BY d ASC"
    );
    let url = format!(
        "{TAP_URL}?request=doQuery&lang=adql&format=tsv&query={}",
        percent_encode(&adql)
    );
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| format!("SIMBAD request failed: {e}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("SIMBAD response unreadable: {e}"))?;
    Ok(pick_from_tap_tsv(&body, kind == ConeKind::NamedNebula))
}

/// Pick the best hit from a TAP TSV result (columns: main_id, otype, ra, dec,
/// d, ids). Rows are already distance-sorted; we take the most prominent
/// tier and, within it, the closest.
///
/// With `require_name` (companion-nebula search) only objects with a clean
/// common name qualify, and a name ending in a type word ("... Nebula")
/// beats catalogue prominence: for a cluster inside the Rosette we want
/// "Rosette Nebula", not the NGC-numbered fragment that happens to be
/// closest.
pub fn pick_from_tap_tsv(tsv: &str, require_name: bool) -> Option<ObjectInfo> {
    let unquote = |s: &str| s.trim().trim_matches('"').to_string();
    // rank = (0 if name ends in a type word else 1 [named mode only], tier)
    let mut best: Option<((u8, usize), ObjectInfo)> = None;
    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 6 {
            continue;
        }
        let morph = cols.get(6).map(|c| unquote(c));
        let info = ObjectInfo::from_tap(
            &unquote(cols[0]),
            &unquote(cols[5]),
            &unquote(cols[1]),
            morph.as_deref(),
            cols[2].trim().parse().ok(),
            cols[3].trim().parse().ok(),
        );
        let tier = info.prominence();
        if tier == usize::MAX {
            continue;
        }
        let rank = if require_name {
            let Some(name) = info.common_name.as_deref().filter(|n| is_clean_name(n)) else {
                continue;
            };
            (u8::from(!ends_with_type_word(name)), tier)
        } else {
            (0, tier)
        };
        // Rows come closest-first, so only a strictly better rank replaces.
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            let done = rank == (0, 0);
            best = Some((rank, info));
            if done {
                break;
            }
        }
    }
    best.map(|(_, info)| info)
}

/// Parse Sesame's XML (`-ox` output). `Ok(None)` = nothing found.
pub fn parse_sesame(xml: &str) -> Result<Option<ObjectInfo>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("Sesame XML invalid: {e}"))?;

    let Some(resolver) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Resolver" && child_text(n, "oname").is_some())
    else {
        return Ok(None);
    };

    // SIMBAD prefixes common names with "NAME " in identifiers; drop it for
    // display ("NAME Horsehead Nebula" -> "Horsehead Nebula").
    let main_id = collapse_ws(&child_text(&resolver, "oname").unwrap_or_default());
    let main_id = main_id
        .strip_prefix("NAME ")
        .map(str::to_string)
        .unwrap_or(main_id);
    let otype = child_text(&resolver, "otype").unwrap_or_default().trim().to_string();
    let morph = clean_morph(child_text(&resolver, "MType").as_deref());
    let ra_deg = child_text(&resolver, "jradeg").and_then(|s| s.trim().parse().ok());
    let dec_deg = child_text(&resolver, "jdedeg").and_then(|s| s.trim().parse().ok());

    let aliases: Vec<String> = resolver
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "alias")
        .filter_map(|n| n.text())
        .map(collapse_ws)
        .chain(std::iter::once(main_id.clone()))
        .collect();

    Ok(Some(ObjectInfo::from_aliases(main_id, aliases, otype, morph, ra_deg, dec_deg)))
}

fn clean_morph(m: Option<&str>) -> Option<String> {
    m.map(str::trim).filter(|s| !s.is_empty() && *s != "~").map(str::to_string)
}

/// SIMBAD object types that are galaxies (where morphology is meaningful),
/// including the active-nucleus classes whose host galaxy is what the
/// picture shows (Centaurus A is filed as a blazar).
fn is_galaxy_type(otype: &str) -> bool {
    matches!(
        otype.trim_end_matches('?'),
        "G" | "AGN" | "GiG" | "GiP" | "GiC" | "BiC" | "SBG" | "EmG" | "H2G" | "LSB" | "rG"
            | "SyG" | "Sy1" | "Sy2" | "LIN" | "IG" | "PaG" | "BLL" | "Bla" | "QSO"
    )
}

/// Hubble / de Vaucouleurs morphology code in plain words:
/// "SAB(s)cd" -> Spiral galaxy, "SB(r)b" -> Barred spiral galaxy,
/// "E+0-1 pec" -> Elliptical galaxy, "S0" -> Lenticular galaxy,
/// "IB(s)m" -> Irregular galaxy, "dE" / "dSph" -> Dwarf ... galaxy.
pub fn morphology_description(code: &str) -> Option<&'static str> {
    let c: String = code
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if c.is_empty() {
        return None;
    }
    let (dwarf, body) = match c.strip_prefix('d') {
        Some(rest) if rest.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) => (true, rest),
        _ => (false, c.as_str()),
    };
    // Ring/spiral qualifiers like "(s)", "(r)", "(rs)" and the a–d stage
    // follow the class letters, so a prefix test on the upper-cased code is
    // enough to find the class.
    let up = body.to_ascii_uppercase();

    let class = if up.starts_with("SPH") || up.starts_with("DSPH") {
        "Spheroidal"
    } else if up.starts_with("CD") {
        "Giant elliptical"
    } else if up.starts_with('E') {
        "Elliptical"
    } else if up.starts_with("S0") || up.starts_with("SA0") || up.starts_with("SB0") || up.starts_with("SAB0") {
        "Lenticular"
    } else if up.starts_with("SB") {
        "Barred spiral"
    } else if up.starts_with("SA") || up.starts_with('S') {
        "Spiral"
    } else if up.starts_with('I') {
        "Irregular"
    } else if up.starts_with("RING") {
        "Ring"
    } else {
        return None;
    };
    Some(match (dwarf, class) {
        (true, "Elliptical") => "Dwarf elliptical galaxy",
        (true, "Spheroidal") => "Dwarf spheroidal galaxy",
        (true, "Irregular") => "Dwarf irregular galaxy",
        (true, "Spiral") | (true, "Barred spiral") => "Dwarf spiral galaxy",
        (true, _) => "Dwarf galaxy",
        (false, "Spheroidal") => "Spheroidal galaxy",
        (false, "Giant elliptical") => "Giant elliptical galaxy",
        (false, "Elliptical") => "Elliptical galaxy",
        (false, "Lenticular") => "Lenticular galaxy",
        (false, "Barred spiral") => "Barred spiral galaxy",
        (false, "Spiral") => "Spiral galaxy",
        (false, "Irregular") => "Irregular galaxy",
        (false, "Ring") => "Ring galaxy",
        _ => return None,
    })
}

/// If `alias` is "<simbad prefix><number>" for this catalogue, return the
/// pretty form.
fn catalog_designation(cat: &Catalog, alias: &str) -> Option<String> {
    for prefix in cat.simbad {
        if alias.len() > prefix.len() && alias[..prefix.len()].eq_ignore_ascii_case(prefix) {
            let rest = alias[prefix.len()..].trim();
            if let Ok(n) = rest.parse::<u32>() {
                if n >= 1 && n <= cat.max {
                    return Some(format!("{}{}", cat.pretty, n));
                }
            }
        }
    }
    None
}

/// Choose the most presentable "NAME ..." alias. SIMBAD lists several
/// variants ("Andromeda", "Andromeda Galaxy", "And Nebula", "NORTH AMER NEB");
/// prefer a mixed-case one ending in a type word, skip abbreviations and
/// shouting.
fn pick_common_name(aliases: &[String]) -> Option<String> {
    const CONSTELLATION_ABBR: &[&str] = &[
        "And", "Ant", "Aps", "Aqr", "Aql", "Ara", "Ari", "Aur", "Boo", "Cae", "Cam", "Cnc", "CVn",
        "CMa", "CMi", "Cap", "Car", "Cas", "Cen", "Cep", "Cet", "Cha", "Cir", "Col", "Com", "CrA",
        "CrB", "Crv", "Crt", "Cru", "Cyg", "Del", "Dor", "Dra", "Equ", "Eri", "For", "Gem", "Gru",
        "Her", "Hor", "Hya", "Hyi", "Ind", "Lac", "Leo", "LMi", "Lep", "Lib", "Lup", "Lyn", "Lyr",
        "Men", "Mic", "Mon", "Mus", "Nor", "Oct", "Oph", "Ori", "Pav", "Peg", "Per", "Phe", "Pic",
        "Psc", "PsA", "Pup", "Pyx", "Ret", "Sge", "Sgr", "Sco", "Scl", "Sct", "Ser", "Sex", "Tau",
        "Tel", "Tri", "TrA", "Tuc", "UMa", "UMi", "Vel", "Vir", "Vol", "Vul",
    ];
    const OTHER_ABBR: &[&str] = &["Neb", "Gal", "Cl", "Nebul", "Amer"];

    let names: Vec<&str> = aliases
        .iter()
        .filter_map(|a| a.strip_prefix("NAME "))
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return None;
    }

    let is_shouting = |n: &str| {
        let letters: Vec<char> = n.chars().filter(|c| c.is_alphabetic()).collect();
        letters.len() >= 4 && letters.iter().all(|c| c.is_uppercase())
    };
    let has_abbreviation = |n: &str| {
        n.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric());
            CONSTELLATION_ABBR.contains(&w) || OTHER_ABBR.contains(&w)
        })
    };

    // Catalogue-ish "names" (digits, lone capitals) are never used: better no
    // common name, so a companion nebula's can be adopted, than
    // "AFGL 333 Cloud (IC 1805)".
    let candidates: Vec<&str> = names.iter().copied().filter(|n| is_clean_name(n)).collect();
    let nice: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|n| !is_shouting(n) && !has_abbreviation(n))
        .collect();

    nice.iter()
        .copied()
        .find(|n| ends_with_type_word(n))
        .or_else(|| nice.first().copied())
        .or_else(|| candidates.first().copied())
        .map(|s| s.to_string())
}

/// Words a real common name tends to end with.
const TYPE_WORDS: &[&str] = &[
    "nebula", "galaxy", "cluster", "cloud", "remnant", "loop", "complex", "star", "group",
    "association", "region", "filament", "chain", "triplet", "quintet", "sextet", "arc",
    "wall", "bubble", "shell", "ring", "pair", "stream", "dwarf",
];

pub(crate) fn ends_with_type_word(name: &str) -> bool {
    name.split_whitespace()
        .last()
        .map(|w| TYPE_WORDS.contains(&w.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// A "NAME ..." alias that reads like a proper name rather than a catalogue
/// entry in disguise: "Wizard Nebula" yes; "AFGL 333 Cloud", "Rosette B",
/// "Lo 2", "NIPSS 1548C27 IRS 1" no (digits, or a lone capital letter).
pub(crate) fn is_clean_name(name: &str) -> bool {
    let mut words = 0;
    for w in name.split_whitespace() {
        words += 1;
        if w.chars().any(|c| c.is_ascii_digit()) {
            return false;
        }
        let alnum: Vec<char> = w.chars().filter(|c| c.is_alphanumeric()).collect();
        if alnum.len() == 1 && alnum[0].is_uppercase() {
            return false;
        }
    }
    words > 0
}

/// Find the first catalogue designation in a file name stem, e.g.
/// "2026-09-05_NGC7000_Ha_300s" -> "NGC 7000".
pub fn designation_in_name(name: &str) -> Option<String> {
    let upper: Vec<char> = name.to_ascii_uppercase().chars().collect();
    let n = upper.len();
    let is_sep = |c: char| c == ' ' || c == '_' || c == '-' || c == '.';

    let mut i = 0;
    while i < n {
        // Start of a letter run, not glued to a preceding letter/digit.
        if !upper[i].is_ascii_alphabetic() || (i > 0 && upper[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && upper[i].is_ascii_alphabetic() {
            i += 1;
        }
        let word: String = upper[start..i].iter().collect();

        for cat in CATALOGS {
            if !cat.keys.contains(&word.as_str()) {
                continue;
            }
            let mut j = i;
            // "SH" must be followed by "2" (optionally separated) to be Sharpless.
            if word == "SH" {
                if j < n && is_sep(upper[j]) {
                    j += 1;
                }
                if j < n && upper[j] == '2' {
                    j += 1;
                } else {
                    continue;
                }
            }
            let sep_here = j < n && is_sep(upper[j]);
            if sep_here {
                if cat.adjacent_only {
                    continue;
                }
                j += 1;
            }
            let dstart = j;
            while j < n && upper[j].is_ascii_digit() && j - dstart < 8 {
                j += 1;
            }
            if j == dstart || (j < n && upper[j].is_ascii_alphabetic()) {
                continue;
            }
            let digits: String = upper[dstart..j].iter().collect();
            if let Ok(num) = digits.parse::<u32>() {
                if num >= 1 && num <= cat.max {
                    return Some(format!("{}{}", cat.pretty, num));
                }
            }
        }
    }
    None
}

/// The finished identification for one file.
pub struct Identification {
    pub label: Label,
    /// Something worth telling the user (cross-check disagreement etc.).
    pub note: Option<String>,
}

/// A candidate found by name, before the coordinate check.
struct Named {
    info: ObjectInfo,
    /// The designation the user wrote (file name / header), for the title.
    preferred: Option<String>,
    note: Option<String>,
}

/// Work out what to stamp on an image, given the header's OBJECT keyword (if
/// any), the header coordinates (plate solution or mount target, if any) and
/// the file name stem.
pub fn identify(
    resolver: &mut Resolver,
    header_object: Option<&str>,
    coords: Option<SkyCoords>,
    stem: &str,
) -> Identification {
    let fallback = || Identification {
        label: Label::plain(stem),
        note: None,
    };
    if !resolver.enabled() {
        return fallback();
    }

    let file_desig = designation_in_name(stem);
    let header = header_object
        .map(str::trim)
        .map(|s| s.trim_matches('\'').trim())
        .filter(|s| !s.is_empty());

    let mut named = identify_by_name(resolver, header, file_desig.as_deref());
    if let Some(n) = named.as_mut() {
        resolver.adopt_companion_nebula(&mut n.info);
    }

    // --- Coordinates: validate the name, or identify an unnamed frame ------
    if let Some(c) = coords {
        match named {
            Some(named) => {
                let (Some(ra), Some(dec)) = (named.info.ra_deg, named.info.dec_deg) else {
                    return Identification {
                        label: compose(&named.info, named.preferred.as_deref()),
                        note: named.note,
                    };
                };
                let sep = c.separation_deg(ra, dec);
                if sep <= c.tolerance_deg() {
                    // The name fits. If the named object is inside the frame
                    // but a *different* notable object sits at the centre,
                    // both are in the picture: stamp both ("Heart Nebula
                    // (IC 1805) & Fish Head Nebula (NGC 896)").
                    if sep <= c.search_radius_deg() {
                        if let Some(mut centre) =
                            resolver.nearby(c.ra_deg, c.dec_deg, c.search_radius_deg()).cloned()
                        {
                            resolver.adopt_companion_nebula(&mut centre);
                            if !centre.same_object(&named.info)
                                && centre.is_notable()
                                && !same_region(&centre, &named.info)
                            {
                                let what = named
                                    .preferred
                                    .clone()
                                    .unwrap_or_else(|| named.info.main_id.clone());
                                let mut note = format!(
                                    "frame is centred on {}; {what} is {sep:.1}° off-centre, both in the field",
                                    actual_title(&centre)
                                );
                                if let Some(n) = named.note {
                                    note = format!("{n}; {note}");
                                }
                                return Identification {
                                    label: compose_pair(
                                        &named.info,
                                        named.preferred.as_deref(),
                                        &centre,
                                        &c,
                                    ),
                                    note: Some(note),
                                };
                            }
                        }
                    }
                    return Identification {
                        label: compose(&named.info, named.preferred.as_deref()),
                        note: named.note,
                    };
                }

                // The name does not fit where the frame points. Ask SIMBAD
                // what is actually there; a prominent object wins.
                let where_from = if c.solved { "plate solution" } else { "header coordinates" };
                let what = named
                    .preferred
                    .clone()
                    .unwrap_or_else(|| named.info.main_id.clone());
                if let Some(mut actual) = resolver.nearby(c.ra_deg, c.dec_deg, c.search_radius_deg()).cloned() {
                    resolver.adopt_companion_nebula(&mut actual);
                    if !actual.same_object(&named.info) && actual.is_notable() {
                        return Identification {
                            label: compose(&actual, None),
                            note: Some(format!(
                                "{what} is {sep:.1}° from the {where_from}; the frame is centred on {}, used that",
                                actual_title(&actual)
                            )),
                        };
                    }
                }
                let mut note = format!(
                    "{what} is {sep:.1}° from the {where_from} (tolerance {:.1}°)",
                    c.tolerance_deg()
                );
                if let Some(n) = named.note {
                    note = format!("{n}; {note}");
                }
                return Identification {
                    label: compose(&named.info, named.preferred.as_deref()),
                    note: Some(note),
                };
            }
            None => {
                if let Some(mut actual) = resolver.nearby(c.ra_deg, c.dec_deg, c.search_radius_deg()).cloned() {
                    resolver.adopt_companion_nebula(&mut actual);
                    let where_from = if c.solved { "plate solution" } else { "header coordinates" };
                    return Identification {
                        label: compose(&actual, None),
                        note: Some(format!("identified from the {where_from}")),
                    };
                }
            }
        }
    } else if let Some(named) = named {
        return Identification {
            label: compose(&named.info, named.preferred.as_deref()),
            note: named.note,
        };
    }

    // Nothing resolved (or the network went away).
    let mut id = fallback();
    if resolver.enabled() && (header.is_some() || file_desig.is_some()) {
        id.note = Some("object not found in SIMBAD; used file name".into());
    }
    id
}

/// Header OBJECT first, cross-checked against the file name; then the file
/// name alone.
fn identify_by_name(resolver: &mut Resolver, header: Option<&str>, file_desig: Option<&str>) -> Option<Named> {
    if let Some(h) = header {
        if let Some(info) = resolver.resolve(h).cloned() {
            return Some(match file_desig {
                Some(fd) if !info.matches(fd) => {
                    // Disagreement. Trust the file name if it resolves.
                    if let Some(info2) = resolver.resolve(fd).cloned() {
                        // "'NGC 7000' is NGC 7000" reads silly; only name the
                        // resolved object when it adds information.
                        let resolved_as = if normalize(h) == normalize(&info.main_id) {
                            String::new()
                        } else {
                            format!(" ({})", info.main_id)
                        };
                        Named {
                            info: info2,
                            preferred: Some(fd.to_string()),
                            note: Some(format!(
                                "header OBJECT is '{h}'{resolved_as} but file name says {fd}; used file name"
                            )),
                        }
                    } else {
                        Named {
                            preferred: designation_in_name(h),
                            note: Some(format!(
                                "header OBJECT '{h}' ({}) does not match file name designation {fd}",
                                info.main_id
                            )),
                            info,
                        }
                    }
                }
                Some(fd) => Named {
                    info,
                    preferred: Some(fd.to_string()),
                    note: None,
                },
                None => Named {
                    preferred: designation_in_name(h),
                    info,
                    note: None,
                },
            });
        }
    }

    if let Some(fd) = file_desig {
        if let Some(info) = resolver.resolve(fd).cloned() {
            return Some(Named {
                info,
                preferred: Some(fd.to_string()),
                note: None,
            });
        }
    }
    None
}

/// Short human name for notes: "Great Orion Nebula (M 42)" or "NGC 7000".
fn actual_title(info: &ObjectInfo) -> String {
    compose(info, None).title
}

/// Build the two-line label: "Common Name (Designation)" over
/// "other ids · type · coordinates".
fn compose(info: &ObjectInfo, preferred: Option<&str>) -> Label {
    let designation = title_designation(info, preferred);
    let title = title_of(info, &designation);
    let mut parts = object_parts(info, &designation, 3);
    let coords = info.coordinates();
    if !coords.is_empty() {
        parts.push(coords);
    }
    Label {
        title,
        subtitle: (!parts.is_empty()).then(|| parts.join("  ·  ")),
    }
}

/// Two objects sharing one frame: "Heart Nebula (IC 1805) & Fish Head Nebula
/// (NGC 896)", or, when they share a name, "Leo Triplet (M 65 & M 66)". The
/// second line carries both objects' ids and types, then the image centre.
fn compose_pair(first: &ObjectInfo, first_pref: Option<&str>, second: &ObjectInfo, centre: &SkyCoords) -> Label {
    let d1 = title_designation(first, first_pref);
    let d2 = title_designation(second, None);
    let title = match (&first.common_name, &second.common_name) {
        (Some(a), Some(b)) if normalize(a) == normalize(b) => format!("{a} ({d1} & {d2})"),
        _ => format!("{} & {}", title_of(first, &d1), title_of(second, &d2)),
    };
    let mut parts = vec![
        object_parts(first, &d1, 2).join("  ·  "),
        object_parts(second, &d2, 2).join("  ·  "),
    ];
    parts.retain(|p| !p.is_empty());
    parts.push(format!(
        "RA {}  Dec {}",
        fmt_ra(centre.ra_deg),
        fmt_dec(centre.dec_deg)
    ));
    Label {
        title,
        subtitle: Some(parts.join("   +   ")),
    }
}

/// Title designation: what the user wrote, else the best-known catalogue id.
/// Caldwell numbers are less recognisable than NGC/IC, so they only lead the
/// title when the user used them; otherwise they go to line two.
fn title_designation(info: &ObjectInfo, preferred: Option<&str>) -> String {
    preferred
        .map(str::to_string)
        .or_else(|| {
            info.designations
                .iter()
                .find(|d| !d.starts_with("C "))
                .or_else(|| info.designations.first())
                .cloned()
        })
        .unwrap_or_else(|| info.main_id.clone())
}

/// "Common Name (Designation)", or just the designation.
fn title_of(info: &ObjectInfo, designation: &str) -> String {
    match &info.common_name {
        Some(name) if normalize(name) != normalize(designation) => format!("{name} ({designation})"),
        _ => designation.to_string(),
    }
}

/// Up to `max_ids` other catalogue ids plus the type, for the second line.
fn object_parts(info: &ObjectInfo, designation: &str, max_ids: usize) -> Vec<String> {
    let key = normalize(designation);
    let mut parts: Vec<String> = info
        .designations
        .iter()
        .filter(|d| normalize(d) != key)
        .take(max_ids)
        .cloned()
        .collect();
    let ty = info.type_description();
    if !ty.is_empty() {
        parts.push(ty);
    }
    parts
}

/// Do two records describe the same region under slightly different names,
/// e.g. IC 1396 "Elephant's Trunk Nebula" vs SIMBAD's separate "Elephant
/// Trunk Nebula" entry? Compares name word sets (possessives stripped): a
/// shared subset counts as the same region. Identical names do *not* count
/// (they are paired as "Name (A & B)" instead).
fn same_region(a: &ObjectInfo, b: &ObjectInfo) -> bool {
    let words = |s: &str| -> HashSet<String> {
        s.split_whitespace()
            .map(|w| {
                w.trim_end_matches("'s")
                    .trim_end_matches("’s")
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    match (&a.common_name, &b.common_name) {
        (Some(x), Some(y)) => {
            if normalize(x) == normalize(y) {
                return false;
            }
            let (wx, wy) = (words(x), words(y));
            let shared = wx.intersection(&wy).count();
            // "Heart Nebula" vs "Fish Head Nebula" share only "nebula".
            shared >= 2 || wx.is_subset(&wy) || wy.is_subset(&wx)
        }
        _ => false,
    }
}

/// Canonical form for comparing identifiers: upper-case, no whitespace,
/// long catalogue names shortened to the abbreviations SIMBAD also uses.
pub fn normalize(s: &str) -> String {
    let mut u: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    for (long, short) in [
        ("MESSIER", "M"),
        ("CALDWELL", "C"),
        ("CLMELOTTE", "MEL"),
        ("MELOTTE", "MEL"),
        ("CLCOLLINDER", "CR"),
        ("COLLINDER", "CR"),
        ("CEDERBLAD", "CED"),
        ("LEDA", "PGC"),
        ("APG", "ARP"),
        ("SH2-", "SH2-"),
    ] {
        if u.starts_with(long) {
            u = format!("{short}{}", &u[long.len()..]);
        }
    }
    u
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn child_text<'a>(node: &roxmltree::Node<'a, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|c| c.text())
        .map(|t| t.to_string())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn fmt_ra(deg: f64) -> String {
    let hours = deg.rem_euclid(360.0) / 15.0;
    let total = (hours * 3600.0).round() as i64;
    let (h, m, s) = (total / 3600 % 24, total / 60 % 60, total % 60);
    format!("{h:02}h {m:02}m {s:02}s")
}

fn fmt_dec(deg: f64) -> String {
    let sign = if deg < 0.0 { '−' } else { '+' };
    let total = (deg.abs() * 3600.0).round() as i64;
    let (d, m, s) = (total / 3600, total / 60 % 60, total % 60);
    format!("{sign}{d:02}° {m:02}′ {s:02}″")
}

/// Human-readable SIMBAD object types (the common ones for astrophotography;
/// unknown codes are shown as-is).
fn otype_description(code: &str) -> &str {
    match code.trim_end_matches('?') {
        // Galaxies
        "G" => "Galaxy",
        "AGN" => "Galaxy (active nucleus)",
        "GiG" => "Galaxy in a group",
        "GiP" => "Galaxy in a pair",
        "GiC" => "Galaxy in a cluster",
        "BiC" => "Brightest cluster galaxy",
        "IG" => "Interacting galaxies",
        "PaG" => "Pair of galaxies",
        "GrG" => "Group of galaxies",
        "CGG" => "Compact group of galaxies",
        "ClG" => "Cluster of galaxies",
        "SCG" => "Supercluster of galaxies",
        "SBG" => "Starburst galaxy",
        "EmG" => "Emission-line galaxy",
        "H2G" => "HII galaxy",
        "LSB" => "Low surface brightness galaxy",
        "rG" => "Radio galaxy",
        "SyG" => "Seyfert galaxy",
        "Sy1" => "Seyfert 1 galaxy",
        "Sy2" => "Seyfert 2 galaxy",
        "LIN" => "LINER galaxy",
        "QSO" => "Quasar",
        "BLL" | "Bla" => "Blazar",
        "PoG" => "Part of a galaxy",
        // Nebulae and interstellar medium
        "HII" => "HII region (emission nebula)",
        "PN" => "Planetary nebula",
        "SNR" => "Supernova remnant",
        "RNe" => "Reflection nebula",
        "DNe" => "Dark nebula",
        "GNe" | "Neb" => "Nebula",
        "EmO" => "Emission object",
        "MoC" => "Molecular cloud",
        "Cld" => "Cloud",
        "ISM" => "Interstellar medium",
        "bub" => "Bubble",
        "HH" => "Herbig-Haro object",
        "SFR" => "Star-forming region",
        "PoC" => "Part of a cloud",
        "glb" => "Globule",
        "cor" => "Dense core",
        "out" => "Outflow",
        "sh" => "Interstellar shell",
        "reg" => "Region",
        // Clusters and associations
        "OpC" => "Open cluster",
        "GlC" => "Globular cluster",
        "Cl*" => "Star cluster",
        "As*" => "Stellar association",
        "MGr" => "Moving group",
        "St*" => "Stellar stream",
        // Stars
        "*" => "Star",
        "**" => "Double or multiple star",
        "V*" => "Variable star",
        "Pe*" => "Peculiar star",
        "Em*" => "Emission-line star",
        "Be*" => "Be star",
        "WR*" => "Wolf-Rayet star",
        "Ce*" => "Cepheid variable",
        "Mi*" => "Mira variable",
        "LP*" => "Long-period variable",
        "RR*" => "RR Lyrae variable",
        "EB*" => "Eclipsing binary",
        "SB*" => "Spectroscopic binary",
        "Or*" => "Orion variable",
        "TT*" => "T Tauri star",
        "Y*O" => "Young stellar object",
        "sg*" => "Supergiant",
        "s*b" => "Blue supergiant",
        "s*r" => "Red supergiant",
        "s*y" => "Yellow supergiant",
        "RG*" => "Red giant",
        "AB*" => "AGB star",
        "pA*" => "Post-AGB star",
        "C*" => "Carbon star",
        "WD*" => "White dwarf",
        "N*" => "Neutron star",
        "Psr" => "Pulsar",
        "BH" => "Black hole",
        "XB*" => "X-ray binary",
        "SN*" => "Supernova",
        "No*" => "Nova",
        "Sy*" => "Symbiotic star",
        "PM*" => "High proper-motion star",
        "HS*" => "Hot subdwarf",
        "BD*" => "Brown dwarf",
        "LM*" => "Low-mass star",
        "Pl" => "Exoplanet",
        // Misc
        "gLe" => "Gravitational lens",
        "X" => "X-ray source",
        "Rad" => "Radio source",
        "IR" => "Infrared source",
        "UV" => "UV source",
        "gam" => "Gamma-ray source",
        "err" | "?" | "" => "",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn designations_from_names() {
        assert_eq!(designation_in_name("M31_Andromeda_2026-09-05"), Some("M 31".into()));
        assert_eq!(designation_in_name("2026-09-05_NGC7000_Ha_300s_0001"), Some("NGC 7000".into()));
        assert_eq!(designation_in_name("ngc_7000 stack"), Some("NGC 7000".into()));
        assert_eq!(designation_in_name("Sh2-155_RGB"), Some("Sh2-155".into()));
        assert_eq!(designation_in_name("SH2_101"), Some("Sh2-155".replace("155", "101")));
        assert_eq!(designation_in_name("IC1396_Elephant"), Some("IC 1396".into()));
        assert_eq!(designation_in_name("B33_horsehead"), Some("Barnard 33".into()));
        assert_eq!(designation_in_name("Messier 42"), None); // Messier requires adjacency? no: multi-letter allows sep
        assert_eq!(designation_in_name("Messier42"), Some("M 42".into()));
        // Filter letters and exposure times must not be mistaken for catalogues.
        assert_eq!(designation_in_name("Light_B_120s_0001"), None);
        assert_eq!(designation_in_name("M_31_L"), None);
        assert_eq!(designation_in_name("NGC7000A"), None);
        assert_eq!(designation_in_name("M999"), None);
        assert_eq!(designation_in_name("flat_2026"), None);
        // Caldwell
        assert_eq!(designation_in_name("C7_L_300s"), Some("C 7".into()));
        assert_eq!(designation_in_name("Caldwell14_RGB"), Some("C 14".into()));
        assert_eq!(designation_in_name("C 7"), None); // lone letter needs adjacency
        assert_eq!(designation_in_name("C200"), None);
    }

    #[test]
    fn normalisation() {
        assert_eq!(normalize("M  31"), "M31");
        assert_eq!(normalize("SH 2-155"), "SH2-155");
        assert_eq!(normalize("Cl Melotte 22"), "MEL22");
        assert_eq!(normalize("Mel 22"), "MEL22");
        assert_eq!(normalize("Messier 31"), "M31");
    }

    #[test]
    fn sesame_parse() {
        let xml = r#"<?xml version="1.0"?><Sesame><Target option="S"><name>M31</name>
          <Resolver name="Sc=Simbad"><otype>AGN</otype><jradeg>10.68470833</jradeg><jdedeg>41.26875</jdedeg>
          <oname>M  31</oname><alias>M 31</alias><alias>NAME Andromeda</alias><alias>NAME Andromeda Galaxy</alias>
          <alias>NAME And Nebula</alias><alias>NGC 224</alias><alias>UGC 454</alias><alias>LEDA 2557</alias>
          </Resolver></Target></Sesame>"#;
        let info = parse_sesame(xml).unwrap().unwrap();
        assert_eq!(info.main_id, "M 31");
        assert_eq!(info.common_name.as_deref(), Some("Andromeda Galaxy"));
        assert_eq!(info.designations, vec!["M 31", "NGC 224", "UGC 454", "PGC 2557"]);

        // Caldwell number and curated name are added from the built-in table.
        let xml = r#"<Sesame><Target><name>IC342</name><Resolver name="S"><otype>G</otype>
          <jradeg>56.70</jradeg><jdedeg>68.096</jdedeg><oname>IC  342</oname>
          <alias>IC 342</alias><alias>UGC 2847</alias><alias>LEDA 13826</alias></Resolver></Target></Sesame>"#;
        let ic342 = parse_sesame(xml).unwrap().unwrap();
        assert_eq!(ic342.designations, vec!["C 5", "IC 342", "UGC 2847", "PGC 13826"]);
        assert_eq!(ic342.common_name.as_deref(), Some("Hidden Galaxy"));
        assert!(ic342.matches("C5"));
        assert!(ic342.matches("Caldwell 5"));
        assert_eq!(compose(&ic342, Some("IC 342")).title, "Hidden Galaxy (IC 342)");
        assert_eq!(compose(&ic342, Some("C 5")).title, "Hidden Galaxy (C 5)");
        assert!(info.matches("m31"));
        assert!(info.matches("NGC224"));
        assert!(!info.matches("NGC 7000"));
        assert_eq!(fmt_ra(10.68470833), "00h 42m 44s");
        assert_eq!(fmt_dec(41.26875), "+41° 16′ 08″");
        let label = compose(&info, Some("M 31"));
        assert_eq!(label.title, "Andromeda Galaxy (M 31)");
        assert_eq!(
            label.subtitle.as_deref(),
            Some("NGC 224  ·  UGC 454  ·  PGC 2557  ·  Galaxy (active nucleus)  ·  RA 00h 42m 44s  Dec +41° 16′ 08″")
        );

        let none = parse_sesame(r#"<Sesame><Target><name>ZZZ</name><INFO> *** Nothing found *** </INFO></Target></Sesame>"#).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn morphology_words() {
        assert_eq!(morphology_description("SAB(s)cd"), Some("Spiral galaxy"));
        assert_eq!(morphology_description("SA(s)b"), Some("Spiral galaxy"));
        assert_eq!(morphology_description("Sc"), Some("Spiral galaxy"));
        assert_eq!(morphology_description("SB(r)b"), Some("Barred spiral galaxy"));
        assert_eq!(morphology_description("SB(s)m"), Some("Barred spiral galaxy"));
        assert_eq!(morphology_description("E+0-1 pec"), Some("Elliptical galaxy"));
        assert_eq!(morphology_description("E3"), Some("Elliptical galaxy"));
        assert_eq!(morphology_description("S0 pec"), Some("Lenticular galaxy"));
        assert_eq!(morphology_description("SAB0^0"), Some("Lenticular galaxy"));
        assert_eq!(morphology_description("I0"), Some("Irregular galaxy"));
        assert_eq!(morphology_description("IB(s)m"), Some("Irregular galaxy"));
        assert_eq!(morphology_description("dE"), Some("Dwarf elliptical galaxy"));
        assert_eq!(morphology_description("dSph"), Some("Dwarf spheroidal galaxy"));
        assert_eq!(morphology_description("cD"), Some("Giant elliptical galaxy"));
        assert_eq!(morphology_description("~"), None);
        assert_eq!(morphology_description(""), None);

        // NGC 2403 is "AGN" to SIMBAD but SAB(s)cd morphologically.
        let xml = r#"<Sesame><Target><name>NGC2403</name><Resolver name="S"><otype>AGN</otype>
          <MType>SAB(s)cd</MType><jradeg>114.214</jradeg><jdedeg>65.6025</jdedeg>
          <oname>NGC  2403</oname><alias>NGC 2403</alias><alias>UGC 3918</alias></Resolver></Target></Sesame>"#;
        let g = parse_sesame(xml).unwrap().unwrap();
        assert_eq!(g.type_description(), "Spiral galaxy");
        assert_eq!(g.designations, vec!["C 7", "NGC 2403", "UGC 3918"]);
        let label = compose(&g, None);
        assert_eq!(label.title, "NGC 2403");
        assert!(label.subtitle.as_deref().unwrap().starts_with("C 7  ·  UGC 3918  ·  Spiral galaxy"));
    }

    #[test]
    fn pairs_and_same_region() {
        let mk = |main: &str, aliases: &[&str], otype: &str, ra: f64, dec: f64| {
            ObjectInfo::from_aliases(
                main.to_string(),
                aliases.iter().map(|s| s.to_string()).collect(),
                otype.to_string(),
                None,
                Some(ra),
                Some(dec),
            )
        };
        let centre = SkyCoords { ra_deg: 170.0, dec_deg: 13.0, fov_radius_deg: Some(0.5), solved: true };

        // Same name -> merged designations.
        let m65 = mk("M 65", &["M 65", "NGC 3623"], "GiP", 169.73, 13.09);
        let m66 = mk("M 66", &["M 66", "NGC 3627"], "AGN", 170.06, 12.99);
        assert!(!same_region(&m65, &m66));
        let l = compose_pair(&m65, Some("M 65"), &m66, &centre);
        assert_eq!(l.title, "Leo Triplet (M 65 & M 66)");
        assert!(l.subtitle.as_deref().unwrap().starts_with("NGC 3623  ·  Galaxy in a pair   +   NGC 3627"));

        // Different names -> "A & B".
        let heart = mk("IC 1805", &["IC 1805"], "OpC", 38.21, 61.47);
        let fish = mk("NGC 896", &["NGC 896"], "HII", 36.4, 62.0);
        assert!(!same_region(&heart, &fish));
        assert_eq!(
            compose_pair(&heart, Some("IC 1805"), &fish, &centre).title,
            "Heart Nebula (IC 1805) & Fish Head Nebula (NGC 896)"
        );

        // Same region under a variant name -> no pairing.
        let ic1396 = mk("IC 1396", &["IC 1396"], "OpC", 324.7, 57.5);
        let trunk = mk("Elephant Trunk Nebula", &["NAME Elephant Trunk Nebula"], "HII", 324.0, 57.5);
        assert!(same_region(&ic1396, &trunk));
        let a = mk("A", &["NAME North America Nebula"], "HII", 0.0, 0.0);
        let b = mk("B", &["NAME North America"], "HII", 0.0, 0.0);
        assert!(same_region(&a, &b));
    }

    #[test]
    fn tap_ranking_prefers_prominent_objects() {
        // Closest-first rows as SIMBAD returns them: obscure PNe inside M31
        // come before M31 itself; the Messier object must still win.
        let tsv = "main_id\totype\tra\tdec\td\tids\n\
            \"[PSC2013] 9\"\t\"PN\"\t10.6835\t41.2690\t0.0009\t\"[PSC2013] 9\"\n\
            \"Ford M 31 574\"\t\"PN\"\t10.6873\t41.2678\t0.0022\t\"Ford M 31 574|[B2015] M31 B127-33\"\n\
            \"NGC  206\"\t\"Cl*\"\t10.10\t40.73\t0.7\t\"NGC   206|OB 78\"\n\
            \"M  31\"\t\"AGN\"\t10.6847\t41.2687\t0.9\t\"NAME Andromeda Galaxy|M  31|NGC   224|UGC   454\"\n";
        let best = pick_from_tap_tsv(tsv, false).unwrap();
        assert_eq!(best.main_id, "M 31");
        assert_eq!(best.designations[0], "M 31");
        assert_eq!(best.common_name.as_deref(), Some("Andromeda Galaxy"));

        // Only obscure objects -> nothing worth stamping.
        let tsv = "main_id\totype\tra\tdec\td\tids\n\"[PSC2013] 9\"\t\"PN\"\t1\t2\t0.1\t\"[PSC2013] 9\"\n";
        assert!(pick_from_tap_tsv(tsv, false).is_none());

        // Named-nebula mode skips unnamed objects even if they are prominent,
        // rejects catalogue-ish "names", and prefers "... Nebula" over a
        // closer NGC-numbered fragment.
        let tsv = "main_id\totype\tra\tdec\td\tids\n\
            \"LBN 511\"\t\"HII\"\t1\t2\t0.01\t\"LBN 511\"\n\
            \"AFGL 333\"\t\"MoC\"\t1\t2\t0.015\t\"NAME AFGL 333 Cloud|AFGL 333\"\n\
            \"NGC  2238\"\t\"HII\"\t1\t2\t0.02\t\"NAME Rosette B|NGC  2238\"\n\
            \"SH  2-142\"\t\"HII\"\t1\t2\t0.03\t\"NAME Wizard Nebula|LBN 511|SH 2-142\"\n";
        let neb = pick_from_tap_tsv(tsv, true).unwrap();
        assert_eq!(neb.common_name.as_deref(), Some("Wizard Nebula"));
        assert_eq!(neb.designations, vec!["Sh2-142", "LBN 511"]);

        assert!(is_clean_name("Wizard Nebula"));
        assert!(is_clean_name("h Persei Cluster"));
        assert!(is_clean_name("Barnard's Loop"));
        assert!(!is_clean_name("AFGL 333 Cloud"));
        assert!(!is_clean_name("Rosette B"));
        assert!(!is_clean_name("Lo 2"));
        assert!(!is_clean_name("NIPSS 1548C27 IRS 1"));
    }

    /// Talks to SIMBAD. Run with: cargo test --lib live_simbad -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_simbad() {
        let mut r = Resolver::new(true);
        for name in ["IC 1805", "NGC 7380", "NGC 2244", "M 31", "NGC 2403", "M 82", "M 87", "NGC 5128"] {
            let mut info = r.resolve(name).cloned().expect("resolves");
            println!(
                "{name}: main_id={} otype={} morph={:?} type='{}' pos=({:?},{:?}) name={:?}",
                info.main_id,
                info.otype,
                info.morph_type,
                info.type_description(),
                info.ra_deg,
                info.dec_deg,
                info.common_name
            );
            if let (Some(ra), Some(dec)) = (info.ra_deg, info.dec_deg) {
                let neb = r.nearby_named_nebula(ra, dec, COMPANION_RADIUS_DEG).cloned();
                println!("   companion: {:?}", neb.as_ref().map(|n| (&n.main_id, &n.common_name, &n.otype)));
            }
            r.adopt_companion_nebula(&mut info);
            println!("   -> title: {}", compose(&info, Some(name)).title);
            assert!(r.failure.is_none(), "network: {:?}", r.failure);
        }
    }
}
