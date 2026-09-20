//! Token templates for destination paths.
//!
//! Syntax: `{token}` or `{token:arg}`; `{{` and `}}` are literal braces. Anything else is
//! copied verbatim. A template is a relative path; `/` separates directories.
//!
//! Tokens:
//! - `date[:strftime]`        capture time, default `%Y-%m-%d`
//! - `year`, `month`, `day`   capture time pieces, zero padded
//! - `time[:strftime]`        capture time, default `%H%M%S`
//! - `import_date[:strftime]` time of the import session, default `%Y-%m-%d`
//! - `camera`                 normalised camera model, e.g. `X-T5`
//! - `make`, `model`          raw EXIF strings
//! - `stem`, `ext[:lower]`    source file name pieces; `ext` has no leading dot
//! - `kind`                   `raw`, `image`, `video`, `sidecar`, `other`
//! - `volume`                 card label
//! - `rel_dir`                source directory relative to the card root, `.` at the root
//! - `seq[:width]`            1-based counter over the groups in an import, default width 4
//! - `iso`, `lens`            raw EXIF values; empty when unknown
//! - `preset`                 name of the film preset a render uses; empty outside renders

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::fmt;

const TOKENS: &[&str] = &[
    "date", "year", "month", "day", "time", "import_date", "camera", "make", "model", "stem",
    "ext", "kind", "volume", "rel_dir", "seq", "iso", "lens", "preset",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Token { name: String, arg: Option<String> },
}

/// A parsed template. Serialises as its source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    source: String,
    parts: Vec<Part>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("unclosed '{{' at byte {0}")]
    Unclosed(usize),
    #[error("unexpected '}}' at byte {0}")]
    StrayClose(usize),
    #[error("unknown token '{0}' (valid: {valid})", valid = TOKENS.join(", "))]
    UnknownToken(String),
    #[error("empty token at byte {0}")]
    EmptyToken(usize),
    #[error("template renders to an empty path")]
    EmptyResult,
    #[error("template must be a relative path without '..' components")]
    NotRelative,
}

/// Values available to a template.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub captured: Option<NaiveDateTime>,
    pub imported: NaiveDateTime,
    pub camera: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub stem: String,
    pub ext: String,
    pub kind: &'static str,
    pub volume: String,
    pub rel_dir: String,
    pub seq: u32,
    pub iso: Option<u32>,
    pub lens: Option<String>,
    /// Film preset name, for render templates.
    pub preset: Option<String>,
}

impl Template {
    pub fn parse(source: &str) -> Result<Self, TemplateError> {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let bytes: Vec<char> = source.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            match c {
                '{' if bytes.get(i + 1) == Some(&'{') => {
                    literal.push('{');
                    i += 2;
                }
                '}' if bytes.get(i + 1) == Some(&'}') => {
                    literal.push('}');
                    i += 2;
                }
                '{' => {
                    let start = i;
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j] != '}' {
                        j += 1;
                    }
                    if j >= bytes.len() {
                        return Err(TemplateError::Unclosed(start));
                    }
                    let inner: String = bytes[i + 1..j].iter().collect();
                    if inner.trim().is_empty() {
                        return Err(TemplateError::EmptyToken(start));
                    }
                    let (name, arg) = match inner.split_once(':') {
                        Some((n, a)) => (n.trim().to_string(), Some(a.to_string())),
                        None => (inner.trim().to_string(), None),
                    };
                    if !TOKENS.contains(&name.as_str()) {
                        return Err(TemplateError::UnknownToken(name));
                    }
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    parts.push(Part::Token { name, arg });
                    i = j + 1;
                }
                '}' => return Err(TemplateError::StrayClose(i)),
                _ => {
                    literal.push(c);
                    i += 1;
                }
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }
        let t = Template { source: source.to_string(), parts };
        t.check_relative()?;
        Ok(t)
    }

    fn check_relative(&self) -> Result<(), TemplateError> {
        if self.source.starts_with('/') {
            return Err(TemplateError::NotRelative);
        }
        for seg in self.source.split('/') {
            if seg == ".." {
                return Err(TemplateError::NotRelative);
            }
        }
        Ok(())
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Names of the tokens used, in order of first appearance.
    pub fn tokens(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in &self.parts {
            if let Part::Token { name, .. } = p
                && !out.contains(&name.as_str())
            {
                out.push(name);
            }
        }
        out
    }

    /// The directory segments this template produces: `Some(name)` for a segment written out
    /// literally, `None` for one any token contributes to. Token values are sanitised, so they
    /// never add separators and the segment count is fixed. The file name is not included.
    pub fn dir_segments(&self) -> Vec<Option<String>> {
        let mut segs: Vec<(String, bool)> = vec![(String::new(), false)];
        for p in &self.parts {
            match p {
                Part::Literal(text) => {
                    for (i, piece) in text.split('/').enumerate() {
                        if i > 0 {
                            segs.push((String::new(), false));
                        }
                        if let Some(last) = segs.last_mut() {
                            last.0.push_str(piece);
                        }
                    }
                }
                Part::Token { .. } => {
                    if let Some(last) = segs.last_mut() {
                        last.1 = true;
                    }
                }
            }
        }
        segs.pop();
        segs.into_iter().map(|(text, dynamic)| if dynamic { None } else { Some(text) }).collect()
    }

    /// Render to a relative path string. Each rendered token value is sanitised so it cannot
    /// introduce path separators; literals are trusted as written.
    pub fn render(&self, ctx: &Context) -> Result<String, TemplateError> {
        let mut out = String::new();
        for p in &self.parts {
            match p {
                Part::Literal(s) => out.push_str(s),
                Part::Token { name, arg } => out.push_str(&sanitise(&render_token(name, arg.as_deref(), ctx))),
            }
        }
        // Collapse accidental empty segments ("a//b", trailing "/") produced by empty values.
        let cleaned: Vec<&str> = out.split('/').filter(|s| !s.is_empty()).collect();
        if cleaned.is_empty() {
            return Err(TemplateError::EmptyResult);
        }
        Ok(cleaned.join("/"))
    }
}

fn render_token(name: &str, arg: Option<&str>, ctx: &Context) -> String {
    let date = |fmt: &str, fallback: &str| -> String {
        match ctx.captured {
            Some(t) => t.format(fmt).to_string(),
            None => fallback.to_string(),
        }
    };
    match name {
        "date" => date(arg.unwrap_or("%Y-%m-%d"), "undated"),
        "year" => date("%Y", "undated"),
        "month" => date("%m", "00"),
        "day" => date("%d", "00"),
        "time" => date(arg.unwrap_or("%H%M%S"), "000000"),
        "import_date" => ctx.imported.format(arg.unwrap_or("%Y-%m-%d")).to_string(),
        "camera" => ctx.camera.clone().unwrap_or_else(|| "unknown-camera".into()),
        "make" => ctx.make.clone().unwrap_or_default(),
        "model" => ctx.model.clone().unwrap_or_default(),
        "stem" => ctx.stem.clone(),
        "ext" => match arg {
            Some("lower") => ctx.ext.to_lowercase(),
            Some("upper") => ctx.ext.to_uppercase(),
            _ => ctx.ext.clone(),
        },
        "kind" => ctx.kind.to_string(),
        "volume" => ctx.volume.clone(),
        "rel_dir" => ctx.rel_dir.clone(),
        "seq" => {
            let width: usize = arg.and_then(|a| a.parse().ok()).unwrap_or(4);
            format!("{:0width$}", ctx.seq, width = width)
        }
        "iso" => ctx.iso.map(|v| v.to_string()).unwrap_or_default(),
        "lens" => ctx.lens.clone().unwrap_or_default(),
        "preset" => ctx.preset.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Make a token value safe as a path component. `rel_dir` is the one value that may legitimately
/// contain `/`, so it is only cleaned per segment.
fn sanitise(v: &str) -> String {
    v.split('/')
        .map(|seg| {
            let s: String = seg
                .chars()
                .map(|c| match c {
                    '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
                    c if c.is_control() => '-',
                    c => c,
                })
                .collect();
            let s = s.trim().to_string();
            if s == "." || s == ".." { String::from("_") } else { s }
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Serialize for Template {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Template::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn dir_segments_split_literal_from_token() {
        let t = Template::parse("Archive/{year}/{date}/{camera}/{stem}.{ext}").unwrap();
        assert_eq!(t.dir_segments(), vec![Some("Archive".into()), None, None, None]);
        let t = Template::parse("{camera}-raw/{stem}.{ext}").unwrap();
        assert_eq!(t.dir_segments(), vec![None]);
        let t = Template::parse("{stem}.{ext}").unwrap();
        assert!(t.dir_segments().is_empty());
    }
    use super::*;
    use chrono::NaiveDate;

    fn ctx() -> Context {
        Context {
            captured: Some(NaiveDate::from_ymd_opt(2026, 9, 15).unwrap().and_hms_opt(14, 3, 9).unwrap()),
            imported: NaiveDate::from_ymd_opt(2026, 9, 16).unwrap().and_hms_opt(8, 0, 0).unwrap(),
            camera: Some("X-T5".into()),
            make: Some("FUJIFILM".into()),
            model: Some("X-T5".into()),
            stem: "DSCF0001".into(),
            ext: "RAF".into(),
            kind: "raw",
            volume: "UNTITLED".into(),
            rel_dir: "DCIM/100_FUJI".into(),
            seq: 7,
            iso: Some(400),
            lens: Some("XF23mmF2 R WR".into()),
            preset: None,
        }
    }

    #[test]
    fn renders_default_layout() {
        let t = Template::parse("{date:%Y}/{date}/{camera}/{stem}.{ext:lower}").unwrap();
        assert_eq!(t.render(&ctx()).unwrap(), "2026/2026-09-15/X-T5/DSCF0001.raf");
    }

    #[test]
    fn pieces_and_seq() {
        let t = Template::parse("{year}-{month}-{day}_{time}_{seq:3}_{iso}").unwrap();
        assert_eq!(t.render(&ctx()).unwrap(), "2026-09-15_140309_007_400");
    }

    #[test]
    fn escaped_braces_and_literals() {
        let t = Template::parse("{{lit}}/{stem}").unwrap();
        assert_eq!(t.render(&ctx()).unwrap(), "{lit}/DSCF0001");
    }

    #[test]
    fn undated_fallback() {
        let mut c = ctx();
        c.captured = None;
        let t = Template::parse("{date}/{stem}").unwrap();
        assert_eq!(t.render(&c).unwrap(), "undated/DSCF0001");
    }

    #[test]
    fn rel_dir_keeps_slashes_but_values_are_sanitised() {
        let mut c = ctx();
        c.camera = Some("weird:name/with*stuff".into());
        let t = Template::parse("other/{rel_dir}/{camera}/{stem}.{ext}").unwrap();
        assert_eq!(t.render(&c).unwrap(), "other/DCIM/100_FUJI/weird-name/with-stuff/DSCF0001.RAF");
    }

    #[test]
    fn empty_segments_collapse() {
        let mut c = ctx();
        c.lens = None;
        let t = Template::parse("{lens}/{stem}").unwrap();
        assert_eq!(t.render(&c).unwrap(), "DSCF0001");
    }

    #[test]
    fn errors() {
        assert_eq!(Template::parse("{nope}").unwrap_err(), TemplateError::UnknownToken("nope".into()));
        assert_eq!(Template::parse("{stem").unwrap_err(), TemplateError::Unclosed(0));
        assert_eq!(Template::parse("a}b").unwrap_err(), TemplateError::StrayClose(1));
        assert_eq!(Template::parse("{}").unwrap_err(), TemplateError::EmptyToken(0));
        assert_eq!(Template::parse("/abs/{stem}").unwrap_err(), TemplateError::NotRelative);
        assert_eq!(Template::parse("../{stem}").unwrap_err(), TemplateError::NotRelative);
    }

    #[test]
    fn serde_roundtrip() {
        let t = Template::parse("{date}/{stem}.{ext}").unwrap();
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, "\"{date}/{stem}.{ext}\"");
        let back: Template = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
        assert!(serde_json::from_str::<Template>("\"{bad}\"").is_err());
    }
}
