//! The batch job itself: find files, convert / post-process each one, report
//! progress. Used by both the CLI and the GUI.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};

use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};

use crate::fits;
use crate::lookup::{self, Resolver};
use crate::pixels;
use crate::post::{Label, Stamper};
use crate::xisf;

/// Extensions (lower-case) treated as source images in normal mode.
pub const IMAGE_EXTS: &[&str] = &["xisf", "fits", "fit", "fts"];
const FITS_EXTS: &[&str] = &["fits", "fit", "fts"];

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Folder to scan. Defaults to the current directory when the CLI gets no
    /// positional argument.
    pub input_dir: PathBuf,
    /// Where to write PNGs; `None` means next to the sources (or, with
    /// `png_only`, edit them in place).
    pub output_dir: Option<PathBuf>,
    pub recursive: bool,
    pub overwrite: bool,
    /// Scale to exactly 3840x2160 and stamp the file name.
    pub resize4k: bool,
    /// Skip XISF conversion: operate on existing `.png` files. Implies
    /// `resize4k`.
    pub png_only: bool,
    /// A TrueType / OpenType font file for the stamp; `None` = bundled font.
    pub font: Option<PathBuf>,
    /// Look the object up online (CDS Sesame / SIMBAD) and stamp its proper
    /// name and catalogue info instead of the bare file name. Only relevant
    /// when stamping; the file name is the fallback.
    pub lookup: bool,
    /// Explicit files to process (e.g. from a right-click selection or drag
    /// and drop). When non-empty, `input_dir`, `recursive` and `png_only` are
    /// not used for scanning: each file is handled by its own extension
    /// (`.png` = resize/stamp only) and written next to itself unless
    /// `output_dir` is set, in which case all outputs go flat into it.
    pub files: Vec<PathBuf>,
    /// Number of files to convert in parallel. `0` (the default) means
    /// `min(available_parallelism, 8)`.
    pub concurrency: usize,
}

/// Resolve `Options::concurrency` to an actual worker count for `job_count`
/// files.
fn worker_count(requested: usize, job_count: usize) -> usize {
    let n = if requested > 0 {
        requested
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8)
    };
    n.max(1).min(job_count.max(1))
}

impl Options {
    /// Effective output directory.
    pub fn output_dir(&self) -> &Path {
        self.output_dir.as_deref().unwrap_or(&self.input_dir)
    }

    /// File extensions that will be scanned for.
    pub fn input_exts(&self) -> &'static [&'static str] {
        if self.png_only {
            &["png"]
        } else {
            IMAGE_EXTS
        }
    }

    /// Human-readable description of the input file kind, for messages such
    /// as "No .xisf / .fits files found."
    pub fn input_kind(&self) -> &'static str {
        if self.png_only {
            ".png"
        } else {
            ".xisf / .fits"
        }
    }

    /// `png_only` is pointless without the 4K step, so it implies it.
    pub fn resize4k(&self) -> bool {
        self.resize4k || self.png_only
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Ok,
    Skipped,
    Failed(String),
}

/// One progress report, sent after each file has been handled.
#[derive(Debug, Clone)]
pub struct Progress {
    /// 1-based index of the file just handled.
    pub index: usize,
    pub total: usize,
    /// Path relative to the input directory.
    pub rel: PathBuf,
    pub status: FileStatus,
    /// Title that was stamped on the image, when it differs from the file
    /// name (i.e. the object was identified online).
    pub label: Option<String>,
    /// Something the user should know about this file (e.g. header and file
    /// name disagree about the object).
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub total: usize,
    pub converted: u32,
    pub skipped: u32,
    pub failed: u32,
    /// True when the run was stopped early via the cancel flag.
    pub cancelled: bool,
    /// Run-level warnings (e.g. the online lookup was unreachable).
    pub warnings: Vec<String>,
}

/// Result of handling one file.
struct Outcome {
    written: bool,
    label: Option<String>,
    note: Option<String>,
}

/// Run the whole batch. `report` is called once per file; set `cancel` from
/// another thread to stop after the current file. Errors returned here are
/// fatal setup problems (bad input dir, unreadable font); per-file problems are
/// reported through `FileStatus::Failed` and counted in the summary.
pub fn run(
    opts: &Options,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&Progress),
) -> Result<Summary, String> {
    let explicit = !opts.files.is_empty();
    if !explicit && !opts.input_dir.is_dir() {
        return Err(format!(
            "Input directory not found: {}",
            opts.input_dir.display()
        ));
    }

    let stamper = if opts.resize4k() {
        Some(match &opts.font {
            Some(path) => Stamper::from_file(path)?,
            None => Stamper::bundled(),
        })
    } else {
        None
    };

    let mut files = Vec::new();
    if explicit {
        files.extend(opts.files.iter().cloned());
    } else {
        collect_files(&opts.input_dir, opts.recursive, opts.input_exts(), &mut files);
        files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    }

    let mut summary = Summary {
        total: files.len(),
        ..Summary::default()
    };
    if files.is_empty() {
        return Ok(summary);
    }

    // Resolve each file's display name and destination up front. Scanned files
    // keep their position relative to the input folder; explicit files go flat.
    let jobs: Vec<Job> = files
        .iter()
        .map(|file| {
            let rel: PathBuf = if explicit {
                file.file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| file.clone())
            } else {
                file.strip_prefix(&opts.input_dir).unwrap_or(file).to_path_buf()
            };
            let dest = match (&opts.output_dir, explicit) {
                (Some(out), _) => out.join(&rel).with_extension("png"),
                (None, true) => file.with_extension("png"),
                (None, false) => opts.input_dir.join(&rel).with_extension("png"),
            };
            Job {
                src: file.clone(),
                dest,
                rel,
            }
        })
        .collect();

    // The online lookup runs behind a mutex: one network round-trip at a time,
    // shared cache across workers (a run of 300 subs of one target still costs
    // one or two SIMBAD requests). The heavy work - decode, stretch, resize,
    // stamp, encode - runs in parallel.
    let resolver = Mutex::new(Resolver::new(opts.lookup && stamper.is_some()));
    let stamper = stamper.as_ref();
    let workers = worker_count(opts.concurrency, jobs.len());
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, FileStatus, Option<String>, Option<String>)>();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (jobs, next, resolver) = (&jobs, &next, &resolver);
            scope.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let idx = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(idx) else { break };
                let msg = match process_one(&job.src, &job.dest, opts, stamper, resolver) {
                    Ok(Outcome {
                        written: true,
                        label,
                        note,
                    }) => (idx, FileStatus::Ok, label, note),
                    Ok(Outcome { note, .. }) => (idx, FileStatus::Skipped, None, note),
                    Err(e) => (idx, FileStatus::Failed(e), None, None),
                };
                if tx.send(msg).is_err() {
                    break;
                }
            });
        }
        drop(tx);

        // Collector: reports in completion order, on this (single) thread.
        for (idx, status, label, note) in rx {
            match &status {
                FileStatus::Ok => summary.converted += 1,
                FileStatus::Skipped => summary.skipped += 1,
                FileStatus::Failed(_) => summary.failed += 1,
            }
            report(&Progress {
                index: idx + 1,
                total: jobs.len(),
                rel: jobs[idx].rel.clone(),
                status,
                label,
                note,
            });
        }
    });

    if cancel.load(Ordering::Relaxed) {
        summary.cancelled = true;
    }

    if let Some(e) = resolver.into_inner().unwrap().failure {
        summary.warnings.push(format!(
            "Online object lookup unavailable ({e}); file names were stamped instead."
        ));
    }

    Ok(summary)
}

/// One file's source, destination and display path.
struct Job {
    src: PathBuf,
    dest: PathBuf,
    rel: PathBuf,
}

/// Handle one file.
fn process_one(
    src: &Path,
    dest: &Path,
    opts: &Options,
    stamper: Option<&Stamper>,
    resolver: &Mutex<Resolver>,
) -> Result<Outcome, String> {
    // A PNG source is only ever resized/stamped, never "converted".
    let is_png = has_ext(src, &["png"]);
    let in_place = is_png && is_same_file(src, dest);
    if !in_place && dest.exists() && !opts.overwrite {
        return Ok(Outcome {
            written: false,
            label: None,
            note: None,
        });
    }

    let mut header_object: Option<String> = None;
    let mut header_coords = None;
    let mut img = if is_png {
        image::ImageReader::open(src)
            .and_then(|r| r.decode().map_err(std::io::Error::other))
            .map_err(|e| format!("cannot read PNG: {e}"))?
    } else {
        let data = if has_ext(src, FITS_EXTS) {
            fits::read(src)
        } else {
            xisf::read(src)
        }
        .map_err(|e| e.to_string())?;
        header_object = data.object.clone();
        header_coords = data.coords;
        let image8 = pixels::to_image(&data).map_err(|e| e.to_string())?;
        image8_to_dynamic(image8)?
    };

    let mut label = None;
    let mut note = None;
    if let Some(stamper) = stamper {
        let stem = dest
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let id = {
            let mut resolver = resolver.lock().unwrap();
            lookup::identify(
                &mut resolver,
                header_object.as_deref(),
                header_coords,
                &stem,
            )
        };
        if id.label != Label::plain(&stem) {
            label = Some(id.label.title.clone());
        }
        note = id.note;
        img = stamper.resize_and_label(img, &id.label);
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create output folder: {e}"))?;
    }
    let file = File::create(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let mut w = BufWriter::new(file);
    img.write_to(&mut w, ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;

    Ok(Outcome {
        written: true,
        label,
        note,
    })
}

fn image8_to_dynamic(img: pixels::Image8) -> Result<DynamicImage, String> {
    let (w, h) = (img.width, img.height);
    match img.channels {
        1 => GrayImage::from_raw(w, h, img.pixels)
            .map(DynamicImage::ImageLuma8)
            .ok_or_else(|| "pixel buffer size mismatch".to_string()),
        3 => RgbImage::from_raw(w, h, img.pixels)
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(|| "pixel buffer size mismatch".to_string()),
        n => Err(format!("unsupported channel count {n}")),
    }
}

/// Recursively (or not) gather files whose extension (case-insensitive) is
/// one of `exts`, under `dir`.
pub fn collect_files(dir: &Path, recursive: bool, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if recursive {
                collect_files(&path, recursive, exts, out);
            }
        } else if file_type.is_file() && has_ext(&path, exts) {
            out.push(path);
        }
    }
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| exts.iter().any(|e| s.eq_ignore_ascii_case(e)))
}

/// True when both paths refer to the same existing file (handles `.` vs `./`,
/// trailing separators, drive-letter case, ...).
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
