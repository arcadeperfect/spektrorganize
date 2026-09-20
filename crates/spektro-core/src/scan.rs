//! Walk a source (SD card or folder), classify files, group them, read capture metadata.

use crate::meta::CaptureMeta;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Raw,
    Image,
    Video,
    Sidecar,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Raw => "raw",
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Sidecar => "sidecar",
            Kind::Other => "other",
        }
    }

    fn is_primary(self) -> bool {
        matches!(self, Kind::Raw | Kind::Image | Kind::Video)
    }
}

const RAW_EXT: &[&str] = &[
    "raf", "arw", "dng", "cr2", "cr3", "nef", "nrw", "orf", "rw2", "pef", "srw", "x3f", "3fr",
    "iiq", "srf", "sr2", "erf", "mrw", "rwl", "kdc", "dcr", "mos", "mef", "crw", "fff",
];
const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "heif", "heic", "hif", "png", "tif", "tiff", "webp", "avif"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v", "avi", "mts", "m2ts", "mxf", "mkv", "3gp"];
const SIDECAR_EXT: &[&str] = &["xml", "thm", "xmp", "bim", "srt", "lrv", "wav", "aae", "txt"];

pub fn classify(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    // Sony writes small JPEG thumbnails for clips under THMBNL; they are not photos.
    if path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("THMBNL") {
        return Kind::Other;
    }
    let e = ext.as_str();
    if RAW_EXT.contains(&e) {
        Kind::Raw
    } else if IMAGE_EXT.contains(&e) {
        Kind::Image
    } else if VIDEO_EXT.contains(&e) {
        Kind::Video
    } else if SIDECAR_EXT.contains(&e) {
        Kind::Sidecar
    } else {
        Kind::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupId(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannedFile {
    pub id: FileId,
    pub path: PathBuf,
    /// Path relative to the source root.
    pub rel: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

impl ScannedFile {
    pub fn stem(&self) -> String {
        self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
    }
    pub fn ext(&self) -> String {
        self.path.extension().and_then(|s| s.to_str()).unwrap_or("").to_string()
    }
    /// Directory of the file relative to the source root, `.` at the root.
    pub fn rel_dir(&self) -> String {
        match self.rel.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().replace('\\', "/"),
            _ => ".".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    /// A camera JPEG shot alongside the RAW.
    SidecarJpeg,
    /// The camera's preview embedded inside the RAW container.
    EmbeddedPreview,
    /// The file itself is a viewable image.
    Itself,
    /// A frame out of a video.
    VideoFrame,
    None,
}

/// A primary file with its attachments (paired JPEG, sidecars).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub primary: FileId,
    pub attachments: Vec<FileId>,
    pub kind: Kind,
    pub meta: CaptureMeta,
    pub preview: PreviewSource,
    /// The file to read the preview from (sidecar JPEG, or the RAW itself).
    pub preview_file: Option<FileId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub root: PathBuf,
    /// Volume label for cards, folder name otherwise.
    pub label: String,
    pub is_volume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scan {
    pub source: SourceInfo,
    pub files: Vec<ScannedFile>,
    /// Groups in capture order (undated last, then by path).
    pub groups: Vec<Group>,
    /// Files that were skipped because they could not be read.
    pub errors: Vec<(PathBuf, String)>,
}

impl Scan {
    pub fn file(&self, id: FileId) -> &ScannedFile {
        &self.files[id.0 as usize]
    }
    pub fn group(&self, id: GroupId) -> &Group {
        &self.groups[id.0 as usize]
    }
    pub fn group_files(&self, g: &Group) -> impl Iterator<Item = &ScannedFile> {
        std::iter::once(g.primary).chain(g.attachments.iter().copied()).map(|id| self.file(id))
    }
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Debug, Clone)]
pub enum ScanProgress {
    Walking { files: usize },
    ReadingMetadata { done: usize, total: usize },
}

/// Mounted volumes that look like camera cards (contain a `DCIM` folder).
pub fn list_sources() -> Vec<SourceInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/Volumes") else { return out };
    for entry in rd.flatten() {
        let root = entry.path();
        if root.join("DCIM").is_dir() {
            out.push(SourceInfo {
                label: entry.file_name().to_string_lossy().to_string(),
                root,
                is_volume: true,
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

pub fn source_info(root: &Path) -> SourceInfo {
    let is_volume = root.starts_with("/Volumes") && root.components().count() == 3;
    SourceInfo {
        label: root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "source".into()),
        root: root.to_path_buf(),
        is_volume,
    }
}

pub(crate) fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name == "Thumbs.db" || name == "desktop.ini" || is_library_package(name)
}

/// Another program's library bundle: Lightroom, Capture One, Photos, Aperture. These hold
/// thousands of preview and cache files and no originals worth indexing — the originals, when
/// they are managed, live inside as opaque blobs. Walking into one buries the catalog in junk.
pub(crate) fn is_library_package(name: &str) -> bool {
    const PACKAGES: &[&str] = &[
        ".lrdata",
        ".lrcat",
        ".lrcat-data",
        ".cocatalog",
        ".cosessiondb",
        ".photoslibrary",
        ".aplibrary",
        ".migratedaperturelibrary",
        ".photolibrary",
    ];
    let lower = name.to_ascii_lowercase();
    PACKAGES.iter().any(|p| lower.ends_with(p))
}

/// Walk `root`, classify and group. `meta_reader` reads capture metadata for a RAW primary; it is
/// injected so the scanner does not depend on LibRaw directly (see `crate::decode::read_meta`).
pub fn scan(
    root: &Path,
    meta_reader: &(dyn Fn(&Path) -> Result<CaptureMeta, String> + Sync),
    mut progress: impl FnMut(ScanProgress),
) -> anyhow::Result<Scan> {
    let root = root.canonicalize()?;
    let source = source_info(&root);
    let mut files = Vec::new();
    let mut errors = Vec::new();

    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(&e.file_name().to_string_lossy()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push((e.path().map(Path::to_path_buf).unwrap_or_default(), e.to_string()));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                errors.push((entry.path().to_path_buf(), e.to_string()));
                continue;
            }
        };
        let path = entry.path().to_path_buf();
        let rel = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
        files.push(ScannedFile {
            id: FileId(files.len() as u32),
            kind: classify(&path),
            path,
            rel,
            size: md.len(),
            mtime: md.modified().ok(),
        });
        if files.len() % 50 == 0 {
            progress(ScanProgress::Walking { files: files.len() });
        }
    }
    progress(ScanProgress::Walking { files: files.len() });

    let mut groups = build_groups(&files);

    // Metadata, in parallel but bounded: SD cards do not like many concurrent readers.
    let total = groups.len();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build()?;
    let metas: Vec<CaptureMeta> = pool.install(|| {
        groups
            .par_iter()
            .map(|g| {
                let f = &files[g.primary.0 as usize];
                match g.kind {
                    Kind::Raw => meta_reader(&f.path).unwrap_or_else(|_| CaptureMeta::from_mtime(&f.path)),
                    Kind::Image => CaptureMeta::from_exif(&f.path),
                    _ => CaptureMeta::from_mtime(&f.path),
                }
            })
            .collect()
    });
    for (g, m) in groups.iter_mut().zip(metas) {
        g.meta = m;
    }
    progress(ScanProgress::ReadingMetadata { done: total, total });

    // Capture order; undated groups sort last.
    groups.sort_by(|a, b| {
        let ka = (a.meta.captured_at.is_none(), a.meta.captured_at, &files[a.primary.0 as usize].rel);
        let kb = (b.meta.captured_at.is_none(), b.meta.captured_at, &files[b.primary.0 as usize].rel);
        ka.cmp(&kb)
    });
    for (i, g) in groups.iter_mut().enumerate() {
        g.id = GroupId(i as u32);
    }

    Ok(Scan { source, files, groups, errors })
}

/// Group files by directory and stem prefix. Every file ends up in exactly one group.
pub fn build_groups(files: &[ScannedFile]) -> Vec<Group> {
    // Directory -> primaries in that directory.
    let mut by_dir: HashMap<PathBuf, Vec<FileId>> = HashMap::new();
    for f in files {
        by_dir.entry(f.path.parent().map(Path::to_path_buf).unwrap_or_default()).or_default().push(f.id);
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut owner: Vec<Option<usize>> = vec![None; files.len()];

    for ids in by_dir.values() {
        // Primaries: raw beats image beats video when stems are identical.
        let rank = |k: Kind| match k {
            Kind::Raw => 0,
            Kind::Image => 1,
            Kind::Video => 2,
            _ => 3,
        };
        let mut prim: Vec<&ScannedFile> = ids.iter().map(|id| &files[id.0 as usize]).filter(|f| f.kind.is_primary()).collect();
        prim.sort_by_key(|f| (f.stem().to_ascii_lowercase(), rank(f.kind)));

        // A primary whose stem exactly matches an earlier, higher-ranked primary is an attachment
        // (the paired JPEG next to a RAW). Otherwise it starts a group.
        let mut stem_to_group: Vec<(String, usize)> = Vec::new();
        for f in &prim {
            let stem = f.stem().to_ascii_lowercase();
            if let Some((_, gi)) = stem_to_group.iter().find(|(s, _)| *s == stem) {
                groups[*gi].attachments.push(f.id);
                owner[f.id.0 as usize] = Some(*gi);
                continue;
            }
            let gi = groups.len();
            groups.push(Group {
                id: GroupId(gi as u32),
                primary: f.id,
                attachments: Vec::new(),
                kind: f.kind,
                meta: CaptureMeta::from_mtime(&f.path),
                preview: PreviewSource::None,
                preview_file: None,
            });
            owner[f.id.0 as usize] = Some(gi);
            stem_to_group.push((stem, gi));
        }

        // Non-primaries attach to the primary with the longest stem that prefixes theirs.
        for id in ids {
            let f = &files[id.0 as usize];
            if owner[f.id.0 as usize].is_some() {
                continue;
            }
            let stem = f.stem().to_ascii_lowercase();
            let best = stem_to_group
                .iter()
                .filter(|(s, _)| stem.starts_with(s.as_str()))
                .max_by_key(|(s, _)| s.len());
            match best {
                Some((_, gi)) => {
                    groups[*gi].attachments.push(f.id);
                    owner[f.id.0 as usize] = Some(*gi);
                }
                None => {
                    let gi = groups.len();
                    groups.push(Group {
                        id: GroupId(gi as u32),
                        primary: f.id,
                        attachments: Vec::new(),
                        kind: f.kind,
                        meta: CaptureMeta::from_mtime(&f.path),
                        preview: PreviewSource::None,
                        preview_file: None,
                    });
                    owner[f.id.0 as usize] = Some(gi);
                }
            }
        }
    }

    for g in &mut groups {
        g.attachments.sort();
        let (preview, file) = match g.kind {
            Kind::Raw => match g.attachments.iter().find(|id| {
                let f = &files[id.0 as usize];
                f.kind == Kind::Image && matches!(f.ext().to_ascii_lowercase().as_str(), "jpg" | "jpeg")
            }) {
                Some(id) => (PreviewSource::SidecarJpeg, Some(*id)),
                None => (PreviewSource::EmbeddedPreview, Some(g.primary)),
            },
            Kind::Image => (PreviewSource::Itself, Some(g.primary)),
            _ => (PreviewSource::None, None),
        };
        g.preview = preview;
        g.preview_file = file;
    }
    groups
}

#[cfg(test)]
mod tests {

    #[test]
    fn library_bundles_are_skipped() {
        assert!(is_hidden("second_life-2 Previews.lrdata"));
        assert!(is_hidden("Second Life.cocatalog"));
        assert!(is_hidden("second_life-2.lrcat"));
        assert!(is_hidden("Photos Library.photoslibrary"));
        assert!(!is_hidden("second life"));
        assert!(!is_hidden("DSCF0001.RAF"));
    }
    use super::*;

    fn f(id: u32, p: &str, size: u64) -> ScannedFile {
        let path = PathBuf::from(p);
        ScannedFile { id: FileId(id), rel: path.clone(), kind: classify(&path), path, size, mtime: None }
    }

    #[test]
    fn classifies() {
        assert_eq!(classify(Path::new("DCIM/100_FUJI/DSCF0001.RAF")), Kind::Raw);
        assert_eq!(classify(Path::new("DCIM/100MSDCF/DSC00001.ARW")), Kind::Raw);
        assert_eq!(classify(Path::new("DCIM/100MSDCF/DSC00001.JPG")), Kind::Image);
        assert_eq!(classify(Path::new("PRIVATE/M4ROOT/CLIP/C0001.MP4")), Kind::Video);
        assert_eq!(classify(Path::new("PRIVATE/M4ROOT/CLIP/C0001M01.XML")), Kind::Sidecar);
        assert_eq!(classify(Path::new("PRIVATE/M4ROOT/THMBNL/C0001T01.JPG")), Kind::Other);
        assert_eq!(classify(Path::new("MEDIAPRO.XML")), Kind::Sidecar);
        assert_eq!(classify(Path::new("weird.bin")), Kind::Other);
    }

    #[test]
    fn groups_raw_with_paired_jpeg_and_video_with_sidecar() {
        let files = vec![
            f(0, "DCIM/100_FUJI/DSCF0001.RAF", 30),
            f(1, "DCIM/100_FUJI/DSCF0001.JPG", 5),
            f(2, "DCIM/100_FUJI/DSCF0002.RAF", 30),
            f(3, "PRIVATE/M4ROOT/CLIP/C0001.MP4", 900),
            f(4, "PRIVATE/M4ROOT/CLIP/C0001M01.XML", 1),
            f(5, "PRIVATE/M4ROOT/MEDIAPRO.XML", 1),
            f(6, "DCIM/100_FUJI/lonely.JPG", 4),
        ];
        let groups = build_groups(&files);
        let find = |id: u32| groups.iter().find(|g| g.primary == FileId(id)).unwrap();

        let raw1 = find(0);
        assert_eq!(raw1.attachments, vec![FileId(1)]);
        assert_eq!(raw1.preview, PreviewSource::SidecarJpeg);
        assert_eq!(raw1.preview_file, Some(FileId(1)));

        let raw2 = find(2);
        assert!(raw2.attachments.is_empty());
        assert_eq!(raw2.preview, PreviewSource::EmbeddedPreview);

        let vid = find(3);
        assert_eq!(vid.attachments, vec![FileId(4)]);

        let mediapro = find(5);
        assert_eq!(mediapro.kind, Kind::Sidecar);

        let lonely = find(6);
        assert_eq!(lonely.kind, Kind::Image);
        assert_eq!(lonely.preview, PreviewSource::Itself);

        // Every file is in exactly one group.
        let mut seen: Vec<FileId> = groups.iter().flat_map(|g| std::iter::once(g.primary).chain(g.attachments.iter().copied())).collect();
        seen.sort();
        assert_eq!(seen, (0..7).map(FileId).collect::<Vec<_>>());
    }
}
