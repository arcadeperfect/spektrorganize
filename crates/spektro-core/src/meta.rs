//! Capture metadata: when and with what a file was shot.
//!
//! RAW files go through LibRaw (header parse only, see [`crate::decode`]). Lone JPEGs use
//! their EXIF block. Everything else falls back to the file's modification time.

use chrono::{DateTime, Local, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaSource {
    /// Parsed by LibRaw from the RAW container.
    Raw,
    /// EXIF from a JPEG/HEIF.
    Exif,
    /// File modification time only.
    Mtime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureMeta {
    /// Camera-local wall clock time, as written by the camera.
    pub captured_at: Option<NaiveDateTime>,
    pub make: Option<String>,
    pub model: Option<String>,
    /// Normalised model name suitable for folder names, e.g. `X-T5`, `ILCE-7RM5`.
    pub camera: Option<String>,
    pub iso: Option<u32>,
    pub lens: Option<String>,
    /// EXIF orientation tag value (1 = upright), when known.
    pub orientation: Option<u16>,
    pub source: MetaSource,
}

impl CaptureMeta {
    pub fn from_mtime(path: &Path) -> Self {
        let captured_at = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .map(|t| DateTime::<Local>::from(t).naive_local());
        CaptureMeta {
            captured_at,
            make: None,
            model: None,
            camera: None,
            iso: None,
            lens: None,
            orientation: None,
            source: MetaSource::Mtime,
        }
    }

    /// Read EXIF from a JPEG/HEIF/TIFF. Falls back to mtime for the date if the file has no
    /// usable EXIF; make/model stay `None` in that case.
    pub fn from_exif(path: &Path) -> Self {
        let fallback = Self::from_mtime(path);
        let Ok(file) = std::fs::File::open(path) else { return fallback };
        let mut reader = std::io::BufReader::new(file);
        let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else { return fallback };

        let text = |tag: exif::Tag| -> Option<String> {
            let f = exif.get_field(tag, exif::In::PRIMARY)?;
            match &f.value {
                // Several ASCII components (Fuji pads LensModel with empty ones): the first
                // non-empty one. `display_value` would join them as `a", "", "...`.
                exif::Value::Ascii(parts) => parts.iter().filter_map(|p| tidy_text(&String::from_utf8_lossy(p))).next(),
                _ => tidy_text(&f.display_value().to_string()),
            }
        };
        let captured_at = text(exif::Tag::DateTimeOriginal)
            .or_else(|| text(exif::Tag::DateTime))
            .and_then(|s| parse_exif_datetime(&s))
            .or(fallback.captured_at);
        let make = text(exif::Tag::Make).filter(|s| !s.is_empty());
        let model = text(exif::Tag::Model).filter(|s| !s.is_empty());
        let iso = exif
            .get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY)
            .and_then(|f| f.value.get_uint(0));
        let lens = text(exif::Tag::LensModel).filter(|s| !s.is_empty());
        let orientation = exif
            .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
            .map(|v| v as u16);
        let camera = model.as_deref().map(normalise_model);
        CaptureMeta {
            captured_at,
            make,
            model,
            camera,
            iso,
            lens,
            orientation,
            source: MetaSource::Exif,
        }
    }
}

/// Clean an EXIF text value: NUL padding, surrounding quotes and whitespace go; so does the
/// `", "", "...` tail older builds recorded for multi-part ASCII fields. `None` when empty.
pub fn tidy_text(s: &str) -> Option<String> {
    let s = s.trim().trim_start_matches('"');
    let s = s.split('"').next().unwrap_or("");
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!s.is_empty()).then(|| s.to_string())
}

/// EXIF stores `YYYY:MM:DD HH:MM:SS`.
pub fn parse_exif_datetime(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim(), "%Y:%m:%d %H:%M:%S").ok()
}

/// Strip the maker prefix some bodies repeat in the model string and tidy whitespace.
pub fn normalise_model(model: &str) -> String {
    let m = model.trim();
    let prefixes = ["FUJIFILM ", "Fujifilm ", "SONY ", "Sony ", "Canon ", "NIKON ", "Nikon "];
    let mut out = m;
    for p in prefixes {
        if let Some(rest) = out.strip_prefix(p) {
            out = rest;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exif_datetime() {
        let t = parse_exif_datetime("2026:09:15 14:03:09").unwrap();
        assert_eq!(t.format("%Y-%m-%d %H:%M:%S").to_string(), "2026-09-15 14:03:09");
        assert!(parse_exif_datetime("garbage").is_none());
    }

    #[test]
    fn tidies_exif_text() {
        assert_eq!(tidy_text("XF18-55mmF2.8-4 R LM OIS\", \"\", \"\","), Some("XF18-55mmF2.8-4 R LM OIS".into()));
        assert_eq!(tidy_text("\"X-T10\""), Some("X-T10".into()));
        assert_eq!(tidy_text("XF23mm\0\0"), Some("XF23mm".into()));
        assert_eq!(tidy_text("  "), None);
    }

    #[test]
    fn normalises_models() {
        assert_eq!(normalise_model("FUJIFILM X-T5"), "X-T5");
        assert_eq!(normalise_model("ILCE-7RM5"), "ILCE-7RM5");
        assert_eq!(normalise_model("  X100V  "), "X100V");
    }
}
