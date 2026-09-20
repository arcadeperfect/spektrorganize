//! Import manifest: what was copied where, with what settings, and what was rendered from it.
//!
//! Lives at `<archive_root>/.spektrorganize/imports/<utc-stamp>.json`. Archive-relative paths are
//! resolved against the manifest's own location, so a moved archive still resolves.

use crate::config::Config;
use crate::meta::CaptureMeta;
use crate::plan::Root;
use crate::scan::{Kind, PreviewSource};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MANIFEST_VERSION: u32 = 1;
pub const DIR: &str = ".spektrorganize/imports";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    Exr,
    Jpeg,
}

impl OutputKind {
    pub fn ext(self) -> &'static str {
        match self {
            OutputKind::Exr => "exr",
            OutputKind::Jpeg => "jpg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputRecord {
    pub kind: OutputKind,
    /// Render root this output was written under (absolute).
    pub root: PathBuf,
    /// Relative to `root`.
    pub rel: String,
    pub rendered_at: DateTime<Utc>,
    pub preset_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "detail")]
pub enum CopyStatus {
    Copied,
    SkippedExisting,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u32,
    pub group: u32,
    /// 1-based position of the group within the import, for `{seq}`.
    #[serde(default)]
    pub seq: u32,
    pub kind: Kind,
    pub is_primary: bool,
    /// Path on the source card, relative to its root.
    pub source_rel: String,
    pub root: Root,
    /// Relative to `root`.
    pub rel: String,
    pub size: u64,
    pub blake3: Option<String>,
    pub copy: CopyStatus,
    pub meta: CaptureMeta,
    pub preview: PreviewSource,
    #[serde(default)]
    pub outputs: Vec<OutputRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetRecord {
    pub name: String,
    pub film: String,
    pub print: String,
    pub params: serde_json::Value,
    pub hash: String,
    pub spektrafilm_rev: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub source_label: String,
    pub source_root: PathBuf,
    /// Roots as they were at import time (absolute).
    pub archive_root: PathBuf,
    pub video_root: PathBuf,
    pub other_root: PathBuf,
    pub render_root: PathBuf,
    pub config: Config,
    pub preset: Option<PresetRecord>,
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn new(cfg: &Config, source_label: &str, source_root: &Path, preset: Option<PresetRecord>) -> Self {
        Manifest {
            version: MANIFEST_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: Utc::now(),
            source_label: source_label.to_string(),
            source_root: source_root.to_path_buf(),
            archive_root: cfg.archive_root.clone(),
            video_root: cfg.video_root().to_path_buf(),
            other_root: cfg.other_root().to_path_buf(),
            render_root: cfg.render_root.clone(),
            config: cfg.clone(),
            preset,
            entries: Vec::new(),
        }
    }

    /// Default file name for this manifest inside `archive_root`. Never reuses an existing name.
    pub fn default_path(&self) -> PathBuf {
        let dir = self.archive_root.join(DIR);
        let stamp = self.created_at.format("%Y%m%dT%H%M%SZ").to_string();
        let mut path = dir.join(format!("{stamp}.json"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = dir.join(format!("{stamp}-{n}.json"));
        }
        path
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn read(path: &Path) -> anyhow::Result<Manifest> {
        let text = std::fs::read(path)?;
        let m: Manifest = serde_json::from_slice(&text)?;
        if m.version > MANIFEST_VERSION {
            anyhow::bail!("manifest version {} is newer than this app supports ({})", m.version, MANIFEST_VERSION);
        }
        Ok(m)
    }

    /// The archive root implied by where the manifest sits (`<root>/.spektrorganize/imports/x.json`),
    /// falling back to the recorded absolute root.
    pub fn archive_root_from(&self, manifest_path: &Path) -> PathBuf {
        manifest_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .filter(|_| manifest_path.parent().and_then(|d| d.file_name()) == Some("imports".as_ref()))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.archive_root.clone())
    }

    /// Absolute path of an entry, given where the manifest is now.
    pub fn resolve(&self, manifest_path: &Path, entry: &Entry) -> PathBuf {
        let archive = self.archive_root_from(manifest_path);
        // Roots that were the archive root at import time follow it when it moves.
        let root = match entry.root {
            Root::Archive => archive,
            Root::Video if self.video_root == self.archive_root => archive,
            Root::Video => self.video_root.clone(),
            Root::Other if self.other_root == self.archive_root => archive,
            Root::Other => self.other_root.clone(),
            Root::Render => self.render_root.clone(),
        };
        root.join(&entry.rel)
    }

    pub fn entry_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }
}

impl OutputRecord {
    pub fn path(&self) -> PathBuf {
        self.root.join(&self.rel)
    }
}

/// Small index of all manifests under an archive root, newest first.
pub fn list_manifests(archive_root: &Path) -> Vec<PathBuf> {
    let dir = archive_root.join(DIR);
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::MetaSource;

    #[test]
    fn roundtrip_and_relocation() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.archive_root = tmp.path().join("archive");
        cfg.video_root = Some(tmp.path().join("video"));
        cfg.render_root = tmp.path().join("render");
        let mut m = Manifest::new(&cfg, "CARD", Path::new("/Volumes/CARD"), None);
        m.entries.push(Entry {
            id: 0,
            group: 0,
            seq: 1,
            kind: Kind::Raw,
            is_primary: true,
            source_rel: "DCIM/100_FUJI/DSCF0001.RAF".into(),
            root: Root::Archive,
            rel: "2026/2026-09-15/X-T5/DSCF0001.RAF".into(),
            size: 30,
            blake3: Some("abc".into()),
            copy: CopyStatus::Copied,
            meta: CaptureMeta { captured_at: None, make: None, model: None, camera: None, iso: None, lens: None, orientation: None, source: MetaSource::Raw },
            preview: PreviewSource::EmbeddedPreview,
            outputs: vec![],
        });
        m.entries.push(Entry { id: 1, root: Root::Video, rel: "2026/v/C0001.MP4".into(), kind: Kind::Video, ..m.entries[0].clone() });
        let path = m.default_path();
        m.write(&path).unwrap();
        let back = Manifest::read(&path).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.resolve(&path, &back.entries[0]), tmp.path().join("archive/2026/2026-09-15/X-T5/DSCF0001.RAF"));

        // Move the archive: entries under the archive root follow, the separate video root does not.
        let moved = tmp.path().join("elsewhere");
        std::fs::create_dir_all(moved.join(DIR)).unwrap();
        let moved_manifest = moved.join(DIR).join(path.file_name().unwrap());
        std::fs::copy(&path, &moved_manifest).unwrap();
        let back = Manifest::read(&moved_manifest).unwrap();
        assert_eq!(back.resolve(&moved_manifest, &back.entries[0]), moved.join("2026/2026-09-15/X-T5/DSCF0001.RAF"));
        assert_eq!(back.resolve(&moved_manifest, &back.entries[1]), tmp.path().join("video/2026/v/C0001.MP4"));

        assert_eq!(list_manifests(&cfg.archive_root), vec![path]);
    }
}
