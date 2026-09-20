//! spektro-core: everything the app does, minus the UI.
//!
//! Flow: `scan` a source -> `plan` destinations from templates -> `copy` -> write `manifest`
//! -> `decode` each RAW with LibRaw -> `film` render with spektrafilm -> `export` EXR/JPEG.
//! `catalog` indexes imports and existing folders into a SQLite library.

pub mod catalog;
pub mod config;
pub mod copy;
pub mod decode;
pub mod export;
pub mod film;
pub mod job;
pub mod look;
pub mod manifest;
pub mod meta;
pub mod plan;
pub mod preview;
pub mod scan;
pub mod template;
