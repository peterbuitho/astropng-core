//! Built-in knowledge SIMBAD lacks: the Caldwell catalogue (not in SIMBAD at
//! all) and the popular nicknames astrophotographers use, many of which
//! SIMBAD does not carry ("Hidden Galaxy", "Soul Nebula", "Fireworks
//! Galaxy"). Plus an optional user names file for personal additions.
//!
//! All designations are compared in [`crate::lookup::normalize`] form.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::lookup::normalize;

/// (Caldwell number, designations (first = the one to look up in SIMBAD),
/// popular name). Source: the published Caldwell list; C99 (Coalsack) has no
/// catalogue designation.
pub const CALDWELL: &[(u32, &[&str], Option<&str>)] = &[
    (1, &["NGC 188"], Some("Polarissima Cluster")),
    (2, &["NGC 40"], Some("Bow-Tie Nebula")),
    (3, &["NGC 4236"], None),
    (4, &["NGC 7023"], Some("Iris Nebula")),
    (5, &["IC 342"], Some("Hidden Galaxy")),
    (6, &["NGC 6543"], Some("Cat's Eye Nebula")),
    (7, &["NGC 2403"], None),
    (8, &["NGC 559"], None),
    (9, &["Sh2-155"], Some("Cave Nebula")),
    (10, &["NGC 663"], None),
    (11, &["NGC 7635"], Some("Bubble Nebula")),
    (12, &["NGC 6946"], Some("Fireworks Galaxy")),
    (13, &["NGC 457"], Some("Owl Cluster")),
    (14, &["NGC 869", "NGC 884"], Some("Double Cluster")),
    (15, &["NGC 6826"], Some("Blinking Planetary")),
    (16, &["NGC 7243"], None),
    (17, &["NGC 147"], None),
    (18, &["NGC 185"], None),
    (19, &["IC 5146"], Some("Cocoon Nebula")),
    (20, &["NGC 7000"], Some("North America Nebula")),
    (21, &["NGC 4449"], None),
    (22, &["NGC 7662"], Some("Blue Snowball Nebula")),
    (23, &["NGC 891"], Some("Silver Sliver Galaxy")),
    (24, &["NGC 1275"], Some("Perseus A")),
    (25, &["NGC 2419"], Some("Intergalactic Wanderer")),
    (26, &["NGC 4244"], Some("Silver Needle Galaxy")),
    (27, &["NGC 6888"], Some("Crescent Nebula")),
    (28, &["NGC 752"], None),
    (29, &["NGC 5005"], None),
    (30, &["NGC 7331"], Some("Deer Lick Group")),
    (31, &["IC 405"], Some("Flaming Star Nebula")),
    (32, &["NGC 4631"], Some("Whale Galaxy")),
    (33, &["NGC 6992"], Some("Eastern Veil Nebula")),
    (34, &["NGC 6960"], Some("Western Veil Nebula")),
    (35, &["NGC 4889"], None),
    (36, &["NGC 4559"], None),
    (37, &["NGC 6885"], None),
    (38, &["NGC 4565"], Some("Needle Galaxy")),
    (39, &["NGC 2392"], Some("Eskimo Nebula")),
    (40, &["NGC 3626"], None),
    (41, &["Mel 25"], Some("Hyades")),
    (42, &["NGC 7006"], None),
    (43, &["NGC 7814"], Some("Little Sombrero Galaxy")),
    (44, &["NGC 7479"], Some("Superman Galaxy")),
    (45, &["NGC 5248"], None),
    (46, &["NGC 2261"], Some("Hubble's Variable Nebula")),
    (47, &["NGC 6934"], None),
    (48, &["NGC 2775"], None),
    (49, &["NGC 2237"], Some("Rosette Nebula")),
    // C50 is the cluster inside the Rosette; the companion-nebula logic names it.
    (50, &["NGC 2244"], None),
    (51, &["IC 1613"], None),
    (52, &["NGC 4697"], None),
    (53, &["NGC 3115"], Some("Spindle Galaxy")),
    (54, &["NGC 2506"], None),
    (55, &["NGC 7009"], Some("Saturn Nebula")),
    (56, &["NGC 246"], Some("Skull Nebula")),
    (57, &["NGC 6822"], Some("Barnard's Galaxy")),
    (58, &["NGC 2360"], Some("Caroline's Cluster")),
    (59, &["NGC 3242"], Some("Ghost of Jupiter")),
    (60, &["NGC 4038"], Some("Antennae Galaxies")),
    (61, &["NGC 4039"], Some("Antennae Galaxies")),
    (62, &["NGC 247"], None),
    (63, &["NGC 7293"], Some("Helix Nebula")),
    (64, &["NGC 2362"], Some("Tau Canis Majoris Cluster")),
    (65, &["NGC 253"], Some("Sculptor Galaxy")),
    (66, &["NGC 5694"], None),
    (67, &["NGC 1097"], None),
    (68, &["NGC 6729"], None),
    (69, &["NGC 6302"], Some("Butterfly Nebula")),
    (70, &["NGC 300"], Some("Sculptor Pinwheel Galaxy")),
    (71, &["NGC 2477"], None),
    (72, &["NGC 55"], Some("String of Pearls Galaxy")),
    (73, &["NGC 1851"], None),
    (74, &["NGC 3132"], Some("Eight-Burst Nebula")),
    (75, &["NGC 6124"], None),
    (76, &["NGC 6231"], None),
    (77, &["NGC 5128"], Some("Centaurus A")),
    (78, &["NGC 6541"], None),
    (79, &["NGC 3201"], None),
    (80, &["NGC 5139"], Some("Omega Centauri")),
    (81, &["NGC 6352"], None),
    (82, &["NGC 6193"], None),
    (83, &["NGC 4945"], None),
    (84, &["NGC 5286"], None),
    (85, &["IC 2391"], Some("Omicron Velorum Cluster")),
    (86, &["NGC 6397"], None),
    (87, &["NGC 1261"], None),
    (88, &["NGC 5823"], None),
    (89, &["NGC 6087"], Some("S Normae Cluster")),
    (90, &["NGC 2867"], None),
    (91, &["NGC 3532"], Some("Wishing Well Cluster")),
    (92, &["NGC 3372"], Some("Carina Nebula")),
    (93, &["NGC 6752"], Some("Great Peacock Globular")),
    (94, &["NGC 4755"], Some("Jewel Box Cluster")),
    (95, &["NGC 6025"], None),
    (96, &["NGC 2516"], Some("Southern Beehive Cluster")),
    (97, &["NGC 3766"], Some("Pearl Cluster")),
    (98, &["NGC 4609"], None),
    (99, &["Coalsack"], Some("Coalsack Nebula")),
    (100, &["IC 2944"], Some("Running Chicken Nebula")),
    (101, &["NGC 6744"], None),
    (102, &["IC 2602"], Some("Southern Pleiades")),
    (103, &["NGC 2070"], Some("Tarantula Nebula")),
    (104, &["NGC 362"], None),
    (105, &["NGC 4833"], None),
    (106, &["NGC 104"], Some("47 Tucanae")),
    (107, &["NGC 6101"], None),
    (108, &["NGC 4372"], None),
    (109, &["NGC 3195"], None),
];

/// Popular names for other frequently imaged objects that SIMBAD either
/// lacks or lists under a less common variant.
pub const POPULAR_NAMES: &[(&str, &str)] = &[
    // Messier
    ("M 1", "Crab Nebula"),
    ("M 6", "Butterfly Cluster"),
    ("M 7", "Ptolemy's Cluster"),
    ("M 8", "Lagoon Nebula"),
    ("M 11", "Wild Duck Cluster"),
    ("M 12", "Gumball Globular"),
    ("M 13", "Great Hercules Cluster"),
    ("M 15", "Great Pegasus Cluster"),
    ("M 16", "Eagle Nebula"),
    ("M 17", "Omega Nebula"),
    ("M 20", "Trifid Nebula"),
    ("M 22", "Great Sagittarius Cluster"),
    ("M 24", "Sagittarius Star Cloud"),
    ("M 27", "Dumbbell Nebula"),
    ("M 29", "Cooling Tower"),
    ("M 30", "Jellyfish Cluster"),
    ("M 31", "Andromeda Galaxy"),
    ("M 33", "Triangulum Galaxy"),
    ("M 34", "Spiral Cluster"),
    ("M 35", "Shoe-Buckle Cluster"),
    ("M 36", "Pinwheel Cluster"),
    ("M 38", "Starfish Cluster"),
    ("M 40", "Winnecke 4"),
    ("M 41", "Little Beehive Cluster"),
    ("M 42", "Orion Nebula"),
    ("M 43", "De Mairan's Nebula"),
    ("M 44", "Beehive Cluster"),
    ("M 45", "Pleiades"),
    ("M 50", "Heart-Shaped Cluster"),
    ("M 51", "Whirlpool Galaxy"),
    ("M 52", "Salt and Pepper Cluster"),
    ("M 55", "Specter Cluster"),
    ("M 57", "Ring Nebula"),
    ("M 61", "Swelling Spiral Galaxy"),
    ("M 62", "Flickering Globular Cluster"),
    ("M 63", "Sunflower Galaxy"),
    ("M 64", "Black Eye Galaxy"),
    ("M 65", "Leo Triplet"),
    ("M 66", "Leo Triplet"),
    ("M 67", "Golden Eye Cluster"), // also "King Cobra Cluster"
    ("M 71", "Angelfish Cluster"),
    ("M 74", "Phantom Galaxy"),
    ("M 76", "Little Dumbbell Nebula"),
    ("M 77", "Cetus A"),
    ("M 78", "Casper the Friendly Ghost Nebula"),
    ("M 81", "Bode's Galaxy"),
    ("M 82", "Cigar Galaxy"),
    ("M 83", "Southern Pinwheel Galaxy"),
    ("M 87", "Virgo A"),
    ("M 93", "Critter Cluster"),
    ("M 94", "Croc's Eye Galaxy"),
    ("M 97", "Owl Nebula"),
    ("M 99", "Coma Pinwheel Galaxy"),
    ("M 101", "Pinwheel Galaxy"),
    ("M 102", "Spindle Galaxy"),
    ("M 104", "Sombrero Galaxy"),
    ("M 107", "Crucifix Cluster"),
    ("M 108", "Surfboard Galaxy"),
    // Nebulae (NGC / IC / Sharpless / Barnard / others)
    ("NGC 281", "Pacman Nebula"),
    ("NGC 896", "Fish Head Nebula"), // brightest part of IC 1795
    ("NGC 1333", "Embryo Nebula"),
    ("NGC 1360", "Robin's Egg Nebula"),
    ("NGC 1435", "Merope Nebula"),
    ("NGC 1491", "Fossil Footprint Nebula"),
    ("NGC 1499", "California Nebula"),
    ("NGC 1514", "Crystal Ball Nebula"),
    ("NGC 1535", "Cleopatra's Eye"),
    ("NGC 1555", "Hind's Variable Nebula"),
    ("NGC 1579", "Northern Trifid Nebula"),
    ("NGC 1931", "Fly Nebula"),
    ("NGC 1977", "Running Man Nebula"),
    ("NGC 2024", "Flame Nebula"),
    ("NGC 2170", "Angel Nebula"),
    ("NGC 2174", "Monkey Head Nebula"),
    ("NGC 2264", "Christmas Tree Cluster"),
    ("NGC 2359", "Thor's Helmet"),
    ("NGC 2371", "Gemini Nebula"),
    ("NGC 2467", "Skull and Crossbones Nebula"),
    ("NGC 2736", "Pencil Nebula"),
    ("NGC 3324", "Gabriela Mistral Nebula"),
    ("NGC 3576", "Statue of Liberty Nebula"),
    ("NGC 3918", "Blue Planetary Nebula"),
    ("NGC 5189", "Spiral Planetary Nebula"),
    ("NGC 6164", "Dragon's Egg Nebula"),
    ("NGC 6188", "Rim Nebula"),
    ("NGC 6210", "Turtle Nebula"),
    ("NGC 6334", "Cat's Paw Nebula"),
    ("NGC 6357", "War and Peace Nebula"),
    ("NGC 6369", "Little Ghost Nebula"),
    ("NGC 6537", "Red Spider Nebula"),
    ("NGC 6572", "Blue Racquetball Nebula"),
    ("NGC 6751", "Glowing Eye Nebula"),
    ("NGC 6818", "Little Gem Nebula"),
    ("NGC 6905", "Blue Flash Nebula"),
    ("NGC 6979", "Pickering's Triangle"),
    ("NGC 6995", "Bat Nebula"),
    ("NGC 7008", "Fetus Nebula"),
    ("NGC 7027", "Jewel Bug Nebula"),
    ("NGC 7380", "Wizard Nebula"),
    ("NGC 7822", "Teddy Bear Nebula"),
    ("NGC 7538", "Northern Lagoon Nebula"),
    ("IC 63", "Ghost of Cassiopeia"),
    ("IC 410", "Tadpoles Nebula"),
    ("IC 417", "Spider Nebula"),
    ("IC 443", "Jellyfish Nebula"),
    ("IC 1318", "Sadr Region"),
    ("IC 1396", "Elephant's Trunk Nebula"),
    ("IC 1795", "Fish Head Nebula"),
    ("IC 1805", "Heart Nebula"),
    ("IC 1848", "Soul Nebula"),
    ("IC 2118", "Witch Head Nebula"),
    ("IC 2177", "Seagull Nebula"),
    ("IC 4406", "Retina Nebula"),
    ("IC 4592", "Blue Horsehead Nebula"),
    ("IC 4604", "Rho Ophiuchi Nebula"),
    ("IC 4628", "Prawn Nebula"),
    ("IC 5070", "Pelican Nebula"),
    ("Sh2-82", "Little Cocoon Nebula"),
    ("Sh2-101", "Tulip Nebula"),
    ("Sh2-106", "Celestial Snow Angel"),
    ("Sh2-114", "Flying Dragon Nebula"),
    ("Sh2-129", "Flying Bat Nebula"),
    ("Sh2-132", "Lion Nebula"),
    ("Sh2-142", "Wizard Nebula"),
    ("Sh2-157", "Lobster Claw Nebula"),
    ("Sh2-158", "Northern Lagoon Nebula"),
    ("Sh2-162", "Bubble Nebula"),
    ("Sh2-190", "Heart Nebula"),
    ("Sh2-199", "Soul Nebula"),
    ("Sh2-206", "Fossil Footprint Nebula"),
    ("Sh2-220", "California Nebula"),
    ("Sh2-229", "Flaming Star Nebula"),
    ("Sh2-236", "Tadpoles Nebula"),
    ("Sh2-240", "Spaghetti Nebula"),
    ("Sh2-248", "Jellyfish Nebula"),
    ("Sh2-252", "Monkey Head Nebula"),
    ("Sh2-261", "Lower's Nebula"),
    ("Sh2-264", "Angelfish Nebula"),
    ("Sh2-273", "Cone Nebula"),
    ("Sh2-274", "Medusa Nebula"),
    ("Sh2-275", "Rosette Nebula"),
    ("Sh2-276", "Barnard's Loop"),
    ("Sh2-279", "Running Man Nebula"),
    ("Sh2-296", "Seagull Nebula"),
    ("Sh2-308", "Dolphin Head Nebula"),
    ("Barnard 33", "Horsehead Nebula"),
    ("Barnard 72", "Snake Nebula"),
    ("Barnard 150", "Seahorse Nebula"),
    ("LDN 1235", "Dark Shark Nebula"),
    ("LDN 1622", "Boogeyman Nebula"),
    ("vdB 141", "Ghost Nebula"),
    // Galaxies
    ("NGC 1316", "Fornax A"),
    ("NGC 1317", "Fornax B"), // companion to NGC 1316 above
    ("NGC 1365", "Great Barred Spiral Galaxy"),
    ("NGC 1566", "Spanish Dancer Galaxy"),
    ("NGC 2442", "Meathook Galaxy"),
    ("NGC 2537", "Bear's Paw Galaxy"), // SIMBAD's own alias list also has
    // "Bear Claw Nebula", which our automatic pick prefers (it ends in a
    // type word) but is a mismatched, less-used name for a galaxy.
    ("NGC 2683", "UFO Galaxy"),
    ("NGC 2841", "Tiger's Eye Galaxy"),
    ("NGC 3184", "Little Pinwheel Galaxy"),
    ("NGC 3344", "Sliced Onion Galaxy"),
    ("NGC 3521", "Bubble Galaxy"),
    ("NGC 3628", "Hamburger Galaxy"),
    ("NGC 4435", "Eyes Galaxies"), // paired with NGC 4438 below (Markarian's Eyes)
    ("NGC 4438", "Eyes Galaxies"),
    ("NGC 4490", "Cocoon Galaxy"),
    ("NGC 4535", "Lost Galaxy"),
    ("NGC 4567", "Butterfly Galaxies"),
    ("NGC 4568", "Siamese Twins"),
    ("NGC 4656", "Hockey Stick Galaxy"),
    ("NGC 4676", "Mice Galaxies"),
    ("NGC 5907", "Splinter Galaxy"),
    ("NGC 6503", "Lost-in-Space Galaxy"),
    ("IC 2574", "Coddington's Nebula"),
    ("UGC 10214", "Tadpole Galaxy"),
    // Star clusters
    ("NGC 2169", "37 Cluster"),
    ("NGC 3293", "Gem Cluster"),
    ("NGC 6811", "Hole in a Cluster"),
    ("NGC 6819", "Foxhead Cluster"),
    ("NGC 6939", "Ghost Bush Cluster"),
    ("NGC 7789", "Caroline's Rose"),
    ("Mel 20", "Alpha Persei Cluster"),
    ("Mel 111", "Coma Star Cluster"),
    ("Cr 399", "Coathanger"),
];

/// Caldwell number for any of the given designations (normalised or not).
pub fn caldwell_number(designations: &[String]) -> Option<u32> {
    let norm: Vec<String> = designations.iter().map(|d| normalize(d)).collect();
    CALDWELL
        .iter()
        .find(|(_, ds, _)| ds.iter().any(|d| norm.contains(&normalize(d))))
        .map(|(n, _, _)| *n)
}

/// For a "C 7" / "Caldwell 7" style query, the designation to look up.
pub fn caldwell_target(query: &str) -> Option<&'static str> {
    let n = normalize(query);
    let digits = n.strip_prefix('C')?;
    let n: u32 = digits.parse().ok()?;
    CALDWELL
        .iter()
        .find(|(c, _, _)| *c == n)
        .map(|(_, ds, _)| ds[0])
}

/// Curated popular name for an object with these designations: the user's
/// names file first, then the built-in tables.
pub fn popular_name(designations: &[String]) -> Option<String> {
    let norm: Vec<String> = designations.iter().map(|d| normalize(d)).collect();
    if let Some(name) = norm.iter().find_map(|d| user_names().get(d).cloned()) {
        return Some(name);
    }
    if let Some(name) = CALDWELL.iter().find_map(|(_, ds, name)| {
        ds.iter()
            .any(|d| norm.contains(&normalize(d)))
            .then_some(*name)
            .flatten()
    }) {
        return Some(name.to_string());
    }
    POPULAR_NAMES
        .iter()
        .find(|(d, _)| norm.contains(&normalize(d)))
        .map(|(_, name)| name.to_string())
}

/// Locations of the optional user names file, first match wins:
/// `$XISF2PNG_NAMES`, `xisf2png-names.txt` next to the executable, then
/// `names.txt` in the per-user config folder (`%APPDATA%\xisf2png` on
/// Windows, `~/Library/Application Support/xisf2png` on macOS,
/// `$XDG_CONFIG_HOME/xisf2png` or `~/.config/xisf2png` elsewhere).
pub fn user_names_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(p) = std::env::var_os("XISF2PNG_NAMES") {
        paths.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("xisf2png-names.txt"));
        }
    }
    let config_dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    if let Some(dir) = config_dir {
        paths.push(dir.join("xisf2png").join("names.txt"));
    }
    paths
}

/// The user's names file, parsed once. Format: one `designation = Name` per
/// line, `#` starts a comment, e.g. `NGC 2403 = My Favourite Galaxy`.
pub fn user_names() -> &'static HashMap<String, String> {
    static NAMES: OnceLock<HashMap<String, String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut map = HashMap::new();
        for path in user_names_paths() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                let line = line.split('#').next().unwrap_or("").trim();
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let (key, value) = (normalize(key.trim()), value.trim());
                if !key.is_empty() && !value.is_empty() {
                    map.insert(key, value.to_string());
                }
            }
            break;
        }
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caldwell_lookups() {
        assert_eq!(caldwell_number(&["NGC 2403".into()]), Some(7));
        assert_eq!(caldwell_number(&["UGC 454".into(), "NGC  884".into()]), Some(14));
        assert_eq!(caldwell_number(&["NGC 224".into()]), None);
        assert_eq!(caldwell_target("C7"), Some("NGC 2403"));
        assert_eq!(caldwell_target("Caldwell 5"), Some("IC 342"));
        assert_eq!(caldwell_target("C 14"), Some("NGC 869"));
        assert_eq!(caldwell_target("C 110"), None);
        assert_eq!(caldwell_target("NGC 7"), None);
        assert_eq!(CALDWELL.len(), 109);
        for (i, (n, ds, _)) in CALDWELL.iter().enumerate() {
            assert_eq!(*n as usize, i + 1, "Caldwell table out of order");
            assert!(!ds.is_empty());
        }
    }

    #[test]
    fn names() {
        assert_eq!(popular_name(&["IC 342".into()]).as_deref(), Some("Hidden Galaxy"));
        assert_eq!(popular_name(&["NGC 2403".into()]), None);
        assert_eq!(popular_name(&["IC 1848".into()]).as_deref(), Some("Soul Nebula"));
        assert_eq!(popular_name(&["M  42".into()]).as_deref(), Some("Orion Nebula"));
        assert_eq!(popular_name(&["NGC 2682".into(), "M 67".into()]).as_deref(), Some("Golden Eye Cluster"));
        assert_eq!(popular_name(&["NGC 1491".into()]).as_deref(), Some("Fossil Footprint Nebula"));
        assert_eq!(popular_name(&["NGC 1333".into()]).as_deref(), Some("Embryo Nebula"));
        assert_eq!(popular_name(&["NGC 3521".into()]).as_deref(), Some("Bubble Galaxy"));
        assert_eq!(popular_name(&["NGC 6572".into()]).as_deref(), Some("Blue Racquetball Nebula"));
        assert_eq!(popular_name(&["NGC 7027".into()]).as_deref(), Some("Jewel Bug Nebula"));
        assert_eq!(popular_name(&["NGC 7822".into()]).as_deref(), Some("Teddy Bear Nebula"));
        assert_eq!(popular_name(&["NGC 7538".into()]).as_deref(), Some("Northern Lagoon Nebula"));
        assert_eq!(popular_name(&["SH 2-158".into()]).as_deref(), Some("Northern Lagoon Nebula"));
        assert_eq!(popular_name(&["NGC 6939".into()]).as_deref(), Some("Ghost Bush Cluster"));
        assert_eq!(popular_name(&["IC 4406".into()]).as_deref(), Some("Retina Nebula"));
        assert_eq!(popular_name(&["M 65".into()]).as_deref(), Some("Leo Triplet"));
        assert_eq!(popular_name(&["M 66".into()]).as_deref(), Some("Leo Triplet"));
        assert_eq!(popular_name(&["SH 2-206".into()]).as_deref(), Some("Fossil Footprint Nebula"));
        // Cross-checked against OpenNGC: a correction (was wrongly "Lobster
        // Nebula", which SIMBAD actually attaches to M 17) and two gaps.
        assert_eq!(popular_name(&["NGC 6357".into()]).as_deref(), Some("War and Peace Nebula"));
        assert_eq!(popular_name(&["NGC 1317".into()]).as_deref(), Some("Fornax B"));
        assert_eq!(popular_name(&["NGC 4435".into()]).as_deref(), Some("Eyes Galaxies"));
        assert_eq!(popular_name(&["NGC 2537".into()]).as_deref(), Some("Bear's Paw Galaxy"));
        // Every table entry must be in canonical pretty form so it matches.
        for (d, _) in POPULAR_NAMES {
            assert!(
                d.starts_with("M ") || d.starts_with("NGC ") || d.starts_with("IC ") || d.starts_with("Sh2-")
                    || d.starts_with("Barnard ") || d.starts_with("LDN ") || d.starts_with("vdB ")
                    || d.starts_with("UGC ") || d.starts_with("Mel ") || d.starts_with("Cr "),
                "unexpected designation form: {d}"
            );
        }
        assert_eq!(popular_name(&["SH 2-101".into()]).as_deref(), Some("Tulip Nebula"));
        assert_eq!(popular_name(&["NGC 9999".into()]), None);
    }
}
