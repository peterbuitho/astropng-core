//! astropng-core - shared core for the AstroPNG family of ports (Rust, Go,
//! Nim, Zig, Scala/JVM): XISF/FITS parsing, pixel stretch, resize/stamp, WCS,
//! and SIMBAD catalogue lookup, plus batch orchestration tying it together.
//!
//! Same-language (Rust) consumers use the safe API re-exported here directly.
//! Other-language consumers link against the C ABI in [`ffi`] instead.

pub mod batch;
pub mod catalog;
pub mod ffi;
pub mod fits;
pub mod lookup;
pub mod pixels;
pub mod post;
pub mod wcs;
pub mod xisf;

pub use batch::{collect_files, run, FileStatus, Options, Progress, Summary};
pub use post::{Label, Stamper};

/// Crate version, for `--version` and window titles.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
