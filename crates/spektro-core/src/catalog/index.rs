//! Indexing into the catalog:
//!
//! - [`add_folder`] / [`index_root`]: walk a root in place, incrementally. Files whose size and
//!   mtime are unchanged are skipped; only directories with new, changed, moved or vanished files
//!   are regrouped. Nothing on disk is ever written.
//! - [`index_manifest`]: record what an import copied (and rendered), using the manifest's own
//!   grouping and metadata. Runs after an import's copy phase and again after its renders.
//! - [`adopt_archive`]: `index_manifest` for every manifest of an existing archive. Read-only:
//!   it reads the manifests and stats files, nothing else.
//!
//! Grouping is the scanner's (`scan::build_groups`): RAW + camera JPEG with the same stem in the
//! same directory are one asset. Existing assets are kept whenever any of their files are still
//! part of a group, so keywords and ratings survive rescans, moves and late-arriving pairs.

use super::{Catalog, RootKind, Role, is_asset_kind, kind_from_str, mtime_ns, now, primary_rank, rel_string};
use crate::decode::RawFile;
use crate::job::Cancel;
use crate::manifest::{CopyStatus, Manifest, list_manifests};
use crate::meta::{CaptureMeta, MetaSource};
use crate::plan::Root as PlanRoot;
use crate::scan::{FileId, Kind, ScannedFile, build_groups, classify, is_hidden};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum IndexProgress {
    Walking { files: usize },
    Metadata { done: usize, total: usize },
    Writing { done: usize, total: usize },
    Manifest { done: usize, total: usize, name: String },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexReport {
    pub root: i64,
    /// Files seen on disk.
    pub files: usize,
    pub unchanged: usize,
    pub added: usize,
    pub changed: usize,
    pub moved: usize,
    /// Files that vanished since the last scan (kept, flagged missing).
    pub missing: usize,
    pub assets_created: usize,
    pub assets_merged: usize,
    /// Photos and clips this root holds after indexing — 0 means the folder had nothing
    /// importable in it (someone else's library bundle, documents, an empty tree).
    pub assets: usize,
    /// The root's folder was not reachable; nothing was changed.
    pub offline: bool,
    pub errors: Vec<(String, String)>,
    /// Assets created or re-linked; the caller may want thumbnails for them.
    #[serde(skip)]
    pub touched_assets: Vec<i64>,
}

/// Capture metadata plus displayed pixel size.
#[derive(Debug, Clone)]
pub struct FileMeta {
    pub meta: CaptureMeta,
    pub dims: Option<(u32, u32)>,
    /// Videos: how long it runs, and what it is encoded with.
    pub duration: Option<f64>,
    pub codec: Option<String>,
}

/// A video container's creation date, which comes in a few shapes.
fn parse_video_date(s: &str) -> Option<chrono::NaiveDateTime> {
    use chrono::{DateTime, NaiveDateTime};
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.naive_local());
    }
    for f in ["%Y-%m-%dT%H:%M:%S%z", "%Y-%m-%d %H:%M:%S", "%Y:%m:%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(s, f) {
            return Some(t);
        }
    }
    None
}

/// Read what the catalog stores about a photo/video: LibRaw header parse for RAWs, EXIF for
/// images, the mtime for everything else.
pub fn read_file_meta(path: &Path, kind: Kind) -> FileMeta {
    let mut duration = None;
    let mut codec = None;
    let (meta, dims) = match kind {
        Kind::Raw => match RawFile::open(path) {
            Ok(r) => {
                let d = (r.width(), r.height());
                (r.meta(), (d.0 > 0 && d.1 > 0).then_some(d))
            }
            Err(_) => (CaptureMeta::from_mtime(path), None),
        },
        Kind::Image => {
            let dims = image::ImageReader::open(path)
                .ok()
                .and_then(|r| r.with_guessed_format().ok())
                .and_then(|r| r.into_dimensions().ok());
            (CaptureMeta::from_exif(path), dims)
        }
        Kind::Video => match crate::video::probe(path) {
            // The container's own creation date beats the file's mtime, which is whenever it
            // was last copied about.
            Ok(v) => {
                duration = Some(v.duration);
                codec = Some(v.codec.clone());
                let mut meta = CaptureMeta::from_mtime(path);
                if let Some(t) = v.created.as_deref().and_then(parse_video_date) {
                    meta.captured_at = Some(t);
                    meta.source = MetaSource::Container;
                }
                (meta, (v.width > 0 && v.height > 0).then_some((v.width, v.height)))
            }
            Err(_) => (CaptureMeta::from_mtime(path), None),
        },
        _ => (CaptureMeta::from_mtime(path), None),
    };
    // Quarter-turn orientations display with width and height swapped.
    let dims = match meta.orientation {
        Some(5..=8) => dims.map(|(w, h)| (h, w)),
        _ => dims,
    };
    FileMeta { meta, dims, duration, codec }
}

// ---------- folders ----------

/// Index `path` in place as a folder root (or rescan it if it is already a root).
pub fn add_folder(cat: &mut Catalog, path: &Path, progress: &mut dyn FnMut(IndexProgress), cancel: &Cancel) -> anyhow::Result<IndexReport> {
    if !path.is_dir() {
        anyhow::bail!("{} is not a folder", path.display());
    }
    let id = match cat.root_by_path(path)? {
        Some((_, RootKind::Render)) => {
            anyhow::bail!("{} is a render output folder; its renders are shown with their photos", path.display())
        }
        Some((id, _)) => id,
        None => cat.ensure_root(path, RootKind::Folder)?,
    };
    index_root(cat, id, progress, cancel)
}

struct DiskFile {
    rel: String,
    abs: PathBuf,
    name: String,
    size: u64,
    mtime: Option<i64>,
    kind: Kind,
}

struct DbFile {
    id: i64,
    rel: String,
    name: String,
    size: i64,
    mtime: Option<i64>,
    missing: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unchanged(i64),
    /// Content changed, or the file was missing and is back.
    Changed(i64),
    New,
    /// Same name, size and mtime as a file that vanished elsewhere in the root.
    Moved(i64),
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn walk(root: &Path, skip: &[PathBuf], errors: &mut Vec<(String, String)>, progress: &mut dyn FnMut(IndexProgress), cancel: &Cancel) -> anyhow::Result<Vec<DiskFile>> {
    let mut out = Vec::new();
    let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
        e.depth() == 0 || (!is_hidden(&e.file_name().to_string_lossy()) && !skip.iter().any(|s| s == e.path()))
    });
    for entry in walker {
        if out.len() % 256 == 0 && cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push((e.path().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(), e.to_string()));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                errors.push((entry.path().to_string_lossy().to_string(), e.to_string()));
                continue;
            }
        };
        let abs = entry.path().to_path_buf();
        let rel = rel_string(abs.strip_prefix(root).unwrap_or(&abs));
        out.push(DiskFile {
            name: entry.file_name().to_string_lossy().to_string(),
            kind: classify(&abs),
            size: md.len(),
            mtime: mtime_ns(md.modified().ok()),
            rel,
            abs,
        });
        if out.len() % 200 == 0 {
            progress(IndexProgress::Walking { files: out.len() });
        }
    }
    progress(IndexProgress::Walking { files: out.len() });
    Ok(out)
}

/// Incrementally (re)index an archive or folder root.
pub fn index_root(cat: &mut Catalog, root_id: i64, progress: &mut dyn FnMut(IndexProgress), cancel: &Cancel) -> anyhow::Result<IndexReport> {
    let root = cat.root(root_id)?;
    if root.kind == RootKind::Render {
        anyhow::bail!("render roots are not indexed for photos");
    }
    let mut report = IndexReport { root: root_id, ..Default::default() };
    if !root.path.is_dir() {
        cat.conn().execute("UPDATE roots SET online = 0 WHERE id = ?1", [root_id])?;
        report.offline = true;
        return Ok(report);
    }
    // Roots nested inside this one are indexed on their own.
    let skip: Vec<PathBuf> =
        cat.roots()?.into_iter().filter(|r| r.id != root_id && r.path.starts_with(&root.path)).map(|r| r.path).collect();
    let disk = walk(&root.path, &skip, &mut report.errors, progress, cancel)?;
    report.files = disk.len();

    let db: Vec<DbFile> = {
        let mut stmt = cat.conn().prepare("SELECT id, rel, name, size, mtime_ns, missing FROM files WHERE root_id = ?1")?;
        stmt.query_map([root_id], |r| {
            Ok(DbFile { id: r.get(0)?, rel: r.get(1)?, name: r.get(2)?, size: r.get(3)?, mtime: r.get(4)?, missing: r.get::<_, i64>(5)? != 0 })
        })?
        .collect::<Result<_, _>>()?
    };
    let by_rel: HashMap<&str, &DbFile> = db.iter().map(|f| (f.rel.as_str(), f)).collect();
    let on_disk: HashSet<&str> = disk.iter().map(|d| d.rel.as_str()).collect();

    let mut states: Vec<State> = disk
        .iter()
        .map(|d| match by_rel.get(d.rel.as_str()) {
            Some(f) if !f.missing && f.size == d.size as i64 && f.mtime == d.mtime => State::Unchanged(f.id),
            Some(f) => State::Changed(f.id),
            None => State::New,
        })
        .collect();

    // Vanished rows (newly or previously missing) are candidates for a move.
    let vanished: Vec<&DbFile> = db.iter().filter(|f| !on_disk.contains(f.rel.as_str())).collect();
    let mut by_key: HashMap<(&str, i64, Option<i64>), Vec<&DbFile>> = HashMap::new();
    for f in &vanished {
        by_key.entry((f.name.as_str(), f.size, f.mtime)).or_default().push(f);
    }
    let mut moved_from: HashMap<i64, String> = HashMap::new();
    for (i, d) in disk.iter().enumerate() {
        if states[i] != State::New {
            continue;
        }
        if let Some(c) = by_key.get_mut(&(d.name.as_str(), d.size as i64, d.mtime))
            && c.len() == 1
        {
            let f = c.pop().unwrap();
            states[i] = State::Moved(f.id);
            moved_from.insert(f.id, f.rel.clone());
        }
    }
    let gone: Vec<&DbFile> = vanished.iter().filter(|f| !f.missing && !moved_from.contains_key(&f.id)).copied().collect();

    // Directories whose grouping may have changed.
    let mut dirty: HashSet<String> = HashSet::new();
    for (i, d) in disk.iter().enumerate() {
        match states[i] {
            State::Unchanged(_) => report.unchanged += 1,
            State::Changed(_) => {
                report.changed += 1;
                dirty.insert(dir_of(&d.rel).to_string());
            }
            State::New => {
                report.added += 1;
                dirty.insert(dir_of(&d.rel).to_string());
            }
            State::Moved(id) => {
                report.moved += 1;
                dirty.insert(dir_of(&d.rel).to_string());
                dirty.insert(dir_of(&moved_from[&id]).to_string());
            }
        }
    }
    for f in &gone {
        dirty.insert(dir_of(&f.rel).to_string());
    }
    report.missing = gone.len();

    // Metadata for new and changed photos/videos, in parallel, outside any transaction.
    let need_meta: Vec<usize> =
        (0..disk.len()).filter(|&i| matches!(states[i], State::New | State::Changed(_)) && is_asset_kind(disk[i].kind)).collect();
    let total = need_meta.len();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    let mut metas: HashMap<usize, FileMeta> = HashMap::with_capacity(total);
    for chunk in need_meta.chunks(64) {
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        let got: Vec<(usize, FileMeta)> = pool.install(|| chunk.par_iter().map(|&i| (i, read_file_meta(&disk[i].abs, disk[i].kind))).collect());
        metas.extend(got);
        progress(IndexProgress::Metadata { done: metas.len(), total });
    }

    // Write, a batch of directories per transaction so readers and other writers get turns.
    let mut dirs: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, d) in disk.iter().enumerate() {
        let dir = dir_of(&d.rel);
        if dirty.contains(dir) {
            dirs.entry(dir).or_default().push(i);
        }
    }
    let gone_by_dir: HashMap<&str, Vec<i64>> = gone.iter().fold(HashMap::new(), |mut m, f| {
        m.entry(dir_of(&f.rel)).or_default().push(f.id);
        m
    });
    // Directories that only lost files still need their gone files flagged.
    let empty_dirs: Vec<&str> = gone_by_dir.keys().filter(|d| !dirs.contains_key(*d)).copied().collect();
    for d in empty_dirs {
        dirs.insert(d, Vec::new());
    }

    let stamp = now();
    let dir_list: Vec<(&str, Vec<usize>)> = dirs.into_iter().collect();
    let total_dirs = dir_list.len();
    let mut touched: HashSet<i64> = HashSet::new();
    let mut counters = Counters::default();
    let mut done_dirs = 0;
    for batch in dir_list.chunks(100) {
        let tx = cat.conn_mut().transaction()?;
        for (dir, idxs) in batch {
            if let Some(ids) = gone_by_dir.get(dir) {
                let mut stmt = tx.prepare_cached("UPDATE files SET missing = 1, indexed_at = ?2 WHERE id = ?1")?;
                for id in ids {
                    stmt.execute(params![id, stamp])?;
                }
            }
            let mut present: Vec<DirFile> = Vec::with_capacity(idxs.len());
            let mut content_changed: HashSet<i64> = HashSet::new();
            for &i in idxs {
                let d = &disk[i];
                let id = match states[i] {
                    State::Unchanged(id) => id,
                    State::Changed(id) => {
                        tx.prepare_cached(
                            "UPDATE files SET size = ?2, mtime_ns = ?3, kind = ?4, blake3 = NULL, missing = 0, indexed_at = ?5 WHERE id = ?1",
                        )?
                        .execute(params![id, d.size as i64, d.mtime, d.kind.as_str(), stamp])?;
                        content_changed.insert(id);
                        id
                    }
                    State::Moved(id) => {
                        tx.prepare_cached("UPDATE files SET rel = ?2, name = ?3, missing = 0, indexed_at = ?4 WHERE id = ?1")?
                            .execute(params![id, d.rel, d.name, stamp])?;
                        id
                    }
                    State::New => {
                        let id = insert_file(&tx, root_id, &d.rel, &d.name, d.kind, d.size, d.mtime, None, false, &stamp)?;
                        content_changed.insert(id);
                        id
                    }
                };
                present.push(DirFile { file_id: id, rel: d.rel.clone(), abs: d.abs.clone(), name: d.name.clone(), kind: d.kind, size: d.size, meta: metas.remove(&i) });
            }
            regroup_dir(&tx, present, &content_changed, &mut counters, &mut touched)?;
        }
        tx.commit()?;
        done_dirs += batch.len();
        progress(IndexProgress::Writing { done: done_dirs, total: total_dirs });
    }

    // Assets whose primary vanished fall back to their best remaining file.
    let orphaned: Vec<i64> = {
        let mut stmt = cat.conn().prepare(
            "SELECT a.id FROM assets a JOIN files f ON f.id = a.primary_file_id WHERE f.root_id = ?1 AND f.missing = 1",
        )?;
        stmt.query_map([root_id], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    if !orphaned.is_empty() {
        let tx = cat.conn_mut().transaction()?;
        for asset in orphaned {
            let best: Option<(i64, String, String, String)> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT f.id, f.kind, r.path, f.rel FROM asset_files af JOIN files f ON f.id = af.file_id JOIN roots r ON r.id = f.root_id
                     WHERE af.asset_id = ?1 AND f.missing = 0 AND f.kind IN ('raw', 'image', 'video')",
                )?;
                let rows: Vec<(i64, String, String, String)> =
                    stmt.query_map([asset], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
                rows.into_iter().min_by_key(|(_, k, _, _)| primary_rank(kind_from_str(k)))
            };
            if let Some((fid, kind, root_path, rel)) = best {
                let fm = read_file_meta(&Path::new(&root_path).join(&rel), kind_from_str(&kind));
                set_primary(&tx, asset, fid, kind_from_str(&kind), &fm)?;
                touched.insert(asset);
            }
        }
        tx.commit()?;
    }

    cat.conn().execute("UPDATE roots SET online = 1, last_indexed_at = ?2 WHERE id = ?1", params![root_id, now()])?;
    report.assets_created = counters.created;
    report.assets_merged = counters.merged;
    report.assets = cat.conn().query_row(
        "SELECT COUNT(DISTINCT af.asset_id) FROM asset_files af JOIN files f ON f.id = af.file_id WHERE f.root_id = ?1",
        params![root_id],
        |r| r.get::<_, i64>(0),
    )? as usize;
    report.touched_assets = touched.into_iter().collect();
    Ok(report)
}

struct DirFile {
    file_id: i64,
    rel: String,
    abs: PathBuf,
    name: String,
    kind: Kind,
    size: u64,
    meta: Option<FileMeta>,
}

#[derive(Default)]
struct Counters {
    created: usize,
    merged: usize,
}

/// Group the present files of one directory and link each photo/video group to an asset.
fn regroup_dir(tx: &Connection, mut files: Vec<DirFile>, content_changed: &HashSet<i64>, counters: &mut Counters, touched: &mut HashSet<i64>) -> anyhow::Result<()> {
    let scanned: Vec<ScannedFile> = files
        .iter()
        .enumerate()
        .map(|(i, f)| ScannedFile { id: FileId(i as u32), path: f.abs.clone(), rel: PathBuf::from(&f.rel), kind: f.kind, size: f.size, mtime: None })
        .collect();
    for g in build_groups(&scanned) {
        if !is_asset_kind(g.kind) {
            continue;
        }
        let members: Vec<Member> = std::iter::once((g.primary, true))
            .chain(g.attachments.iter().map(|a| (*a, false)))
            .map(|(fid, is_primary)| {
                let f = &files[fid.0 as usize];
                Member { file_id: f.file_id, role: Role::for_file(&f.name, f.kind, is_primary), kind: f.kind, is_primary }
            })
            .collect();
        let p = g.primary.0 as usize;
        let primary_changed = content_changed.contains(&files[p].file_id);
        let pf = &mut files[p];
        let (abs, kind) = (pf.abs.clone(), pf.kind);
        let meta_slot = &mut pf.meta;
        let asset = link_group(tx, &members, primary_changed, None, || meta_slot.take().unwrap_or_else(|| read_file_meta(&abs, kind)), counters)?;
        touched.insert(asset);
    }
    Ok(())
}

struct Member {
    file_id: i64,
    role: Role,
    kind: Kind,
    is_primary: bool,
}

#[allow(clippy::too_many_arguments)]
fn insert_file(
    tx: &Connection,
    root_id: i64,
    rel: &str,
    name: &str,
    kind: Kind,
    size: u64,
    mtime: Option<i64>,
    blake3: Option<&str>,
    missing: bool,
    stamp: &str,
) -> anyhow::Result<i64> {
    let id = tx
        .prepare_cached(
            "INSERT INTO files (root_id, rel, name, kind, size, mtime_ns, blake3, missing, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (root_id, rel) DO UPDATE SET
                name = excluded.name, kind = excluded.kind, size = excluded.size, mtime_ns = excluded.mtime_ns,
                blake3 = COALESCE(excluded.blake3,
                                  CASE WHEN files.size = excluded.size AND files.mtime_ns IS excluded.mtime_ns THEN files.blake3 END),
                missing = excluded.missing, indexed_at = excluded.indexed_at
             RETURNING id",
        )?
        .query_row(params![root_id, rel, name, kind.as_str(), size as i64, mtime, blake3, missing as i64, stamp], |r| r.get(0))?;
    Ok(id)
}

fn set_primary(tx: &Connection, asset: i64, file_id: i64, kind: Kind, fm: &FileMeta) -> anyhow::Result<()> {
    let m = &fm.meta;
    tx.prepare_cached(
        "UPDATE assets SET kind = ?2, primary_file_id = ?3, captured_at = ?4, make = ?5, model = ?6, camera = ?7, lens = ?8,
                iso = ?9, width = ?10, height = ?11, orientation = ?12, meta_source = ?13,
                duration = ?14, codec = ?15 WHERE id = ?1",
    )?
    .execute(params![
        asset,
        kind.as_str(),
        file_id,
        m.captured_at.map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string()),
        m.make,
        m.model,
        m.camera,
        m.lens,
        m.iso,
        fm.dims.map(|d| d.0),
        fm.dims.map(|d| d.1),
        m.orientation,
        meta_source_str(m.source),
        fm.duration,
        fm.codec.as_deref(),
    ])?;
    Ok(())
}

fn meta_source_str(s: MetaSource) -> &'static str {
    match s {
        MetaSource::Raw => "raw",
        MetaSource::Exif => "exif",
        MetaSource::Container => "container",
        MetaSource::Mtime => "mtime",
    }
}

/// Attach a group's files to one asset: the primary's existing asset, else any member's, else a
/// new one. Assets left without files are merged into it (keywords, rating, renders move over).
fn link_group(
    tx: &Connection,
    members: &[Member],
    primary_changed: bool,
    import_id: Option<i64>,
    meta: impl FnOnce() -> FileMeta,
    counters: &mut Counters,
) -> anyhow::Result<i64> {
    let primary = members.iter().find(|m| m.is_primary).expect("group has a primary");
    let mut existing: Vec<(i64, i64)> = Vec::new(); // (file, asset)
    {
        let mut stmt = tx.prepare_cached("SELECT asset_id FROM asset_files WHERE file_id = ?1")?;
        for m in members {
            if let Some(a) = stmt.query_row([m.file_id], |r| r.get::<_, i64>(0)).optional()? {
                existing.push((m.file_id, a));
            }
        }
    }
    let target = existing.iter().find(|(f, _)| *f == primary.file_id).map(|(_, a)| *a).or_else(|| existing.iter().map(|(_, a)| *a).min());

    let mut meta = Some(meta);
    let asset = match target {
        Some(asset) => {
            let (cur, cur_kind, cur_missing): (Option<i64>, Option<String>, Option<i64>) = tx
                .prepare_cached("SELECT a.primary_file_id, f.kind, f.missing FROM assets a LEFT JOIN files f ON f.id = a.primary_file_id WHERE a.id = ?1")?
                .query_row([asset], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let cur_in_group = cur.is_some_and(|c| members.iter().any(|m| m.file_id == c));
            let promote = cur != Some(primary.file_id)
                && (cur.is_none()
                    || cur_missing == Some(1)
                    || cur_in_group
                    || primary_rank(primary.kind) < primary_rank(cur_kind.as_deref().map(kind_from_str).unwrap_or(Kind::Other)));
            if promote || (cur == Some(primary.file_id) && primary_changed) {
                set_primary(tx, asset, primary.file_id, primary.kind, &(meta.take().unwrap())())?;
            }
            if let Some(imp) = import_id {
                tx.prepare_cached("UPDATE assets SET import_id = COALESCE(import_id, ?2) WHERE id = ?1")?.execute(params![asset, imp])?;
            }
            asset
        }
        None => {
            tx.prepare_cached("INSERT INTO assets (kind, primary_file_id, import_id, added_at) VALUES (?1, ?2, ?3, ?4)")?
                .execute(params![primary.kind.as_str(), primary.file_id, import_id, now()])?;
            let asset = tx.last_insert_rowid();
            set_primary(tx, asset, primary.file_id, primary.kind, &(meta.take().unwrap())())?;
            counters.created += 1;
            asset
        }
    };

    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO asset_files (file_id, asset_id, role) VALUES (?1, ?2, ?3)
             ON CONFLICT (file_id) DO UPDATE SET asset_id = excluded.asset_id, role = excluded.role",
        )?;
        for m in members {
            stmt.execute(params![m.file_id, asset, m.role.as_str()])?;
        }
    }
    let mut others: Vec<i64> = existing.iter().map(|(_, a)| *a).filter(|a| *a != asset).collect();
    others.sort_unstable();
    others.dedup();
    for other in others {
        let left: i64 = tx.prepare_cached("SELECT COUNT(*) FROM asset_files WHERE asset_id = ?1")?.query_row([other], |r| r.get(0))?;
        if left == 0 {
            merge_asset(tx, other, asset)?;
            counters.merged += 1;
        }
    }
    Ok(asset)
}

/// Move everything that belongs to `from` onto `to`, then delete `from`.
fn merge_asset(tx: &Connection, from: i64, to: i64) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO asset_keywords (asset_id, keyword_id, source, confidence, added_at)
         SELECT ?2, keyword_id, source, confidence, added_at FROM asset_keywords WHERE asset_id = ?1
         ON CONFLICT (asset_id, keyword_id) DO UPDATE SET
            source = CASE WHEN asset_keywords.source = 'user' OR excluded.source = 'user' THEN 'user' ELSE asset_keywords.source END",
        params![from, to],
    )?;
    tx.execute("UPDATE renders SET asset_id = ?2 WHERE asset_id = ?1", params![from, to])?;
    tx.execute(
        "UPDATE assets SET rating = MAX(rating, (SELECT rating FROM assets WHERE id = ?1)),
                           import_id = COALESCE(import_id, (SELECT import_id FROM assets WHERE id = ?1))
         WHERE id = ?2",
        params![from, to],
    )?;
    tx.execute("DELETE FROM assets WHERE id = ?1", [from])?;
    Ok(())
}

// ---------- manifests ----------

/// Manifests written before the EXIF text fix can carry `lens", "", ...` junk.
fn tidy_meta(m: &CaptureMeta) -> CaptureMeta {
    let t = |s: &Option<String>| s.as_deref().and_then(crate::meta::tidy_text);
    CaptureMeta { make: t(&m.make), model: t(&m.model), camera: t(&m.camera), lens: t(&m.lens), ..m.clone() }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ManifestReport {
    pub import_id: i64,
    pub files: usize,
    pub missing_files: usize,
    pub assets: usize,
    pub assets_created: usize,
    pub renders: usize,
    #[serde(skip)]
    pub touched_assets: Vec<i64>,
}

/// Record an import (or an existing manifest) in the catalog. Idempotent: running it again after
/// the renders finished adds the render rows.
pub fn index_manifest(cat: &mut Catalog, manifest_path: &Path, m: &Manifest) -> anyhow::Result<ManifestReport> {
    let archive = m.archive_root_from(manifest_path);
    let archive_id = cat.ensure_root(&archive, RootKind::Archive)?;
    let mut root_ids: HashMap<PlanRoot, (i64, PathBuf)> = HashMap::new();
    root_ids.insert(PlanRoot::Archive, (archive_id, archive.clone()));
    for (which, recorded) in [(PlanRoot::Video, &m.video_root), (PlanRoot::Other, &m.other_root)] {
        let used = m.entries.iter().any(|e| e.root == which);
        if recorded == &m.archive_root || !used {
            root_ids.insert(which, (archive_id, archive.clone()));
        } else {
            let id = cat.ensure_root(recorded, RootKind::Archive)?;
            root_ids.insert(which, (id, recorded.clone()));
        }
    }
    let mut render_roots: HashMap<PathBuf, i64> = HashMap::new();
    for o in m.entries.iter().flat_map(|e| e.outputs.iter()) {
        if !render_roots.contains_key(&o.root) {
            let id = cat.ensure_root(&o.root, RootKind::Render)?;
            render_roots.insert(o.root.clone(), id);
        }
    }
    let online: HashMap<i64, bool> = cat.roots()?.into_iter().map(|r| (r.id, r.online)).collect();

    let manifest_rel = manifest_path.strip_prefix(&archive).map(rel_string).unwrap_or_else(|_| manifest_path.to_string_lossy().to_string());
    let stamp = now();
    let mut report = ManifestReport::default();
    let mut counters = Counters::default();
    let tx = cat.conn_mut().transaction()?;

    let preset_hash = m.preset.as_ref().map(|p| p.hash.clone());
    if let Some(p) = &m.preset {
        tx.execute(
            "INSERT OR IGNORE INTO presets (hash, name, record, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![p.hash, p.name, serde_json::to_string(p)?, stamp],
        )?;
    }
    let import_id: i64 = tx.query_row(
        "INSERT INTO imports (root_id, manifest_rel, source_label, created_at, indexed_at, preset_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (root_id, manifest_rel) DO UPDATE SET indexed_at = excluded.indexed_at, preset_hash = excluded.preset_hash
         RETURNING id",
        params![archive_id, manifest_rel, m.source_label, m.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(), stamp, preset_hash],
        |r| r.get(0),
    )?;
    report.import_id = import_id;

    let mut groups: BTreeMap<u32, Vec<&crate::manifest::Entry>> = BTreeMap::new();
    for e in &m.entries {
        if !matches!(e.copy, CopyStatus::Failed(_)) {
            groups.entry(e.group).or_default().push(e);
        }
    }
    for entries in groups.values() {
        let mut members = Vec::new();
        let mut primary_meta: Option<FileMeta> = None;
        for e in entries {
            let (root_id, root_path) = root_ids.get(&e.root).cloned().unwrap_or((archive_id, archive.clone()));
            let abs = root_path.join(&e.rel);
            let md = std::fs::metadata(&abs).ok();
            let root_online = online.get(&root_id).copied().unwrap_or(true);
            let (size, mtime, blake3, missing) = match &md {
                Some(md) => {
                    let hash = if md.len() == e.size { e.blake3.as_deref() } else { None };
                    (md.len(), mtime_ns(md.modified().ok()), hash, false)
                }
                None => (e.size, None, e.blake3.as_deref(), root_online),
            };
            let name = Path::new(&e.rel).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let file_id = insert_file(&tx, root_id, &e.rel, &name, e.kind, size, mtime, blake3, missing, &stamp)?;
            report.files += 1;
            if missing {
                report.missing_files += 1;
            }
            if e.is_primary {
                primary_meta = Some(FileMeta { meta: tidy_meta(&e.meta), dims: None, duration: None, codec: None });
            }
            members.push(Member { file_id, role: Role::for_file(&name, e.kind, e.is_primary), kind: e.kind, is_primary: e.is_primary });
        }
        let Some(primary) = members.iter().find(|m| m.is_primary) else { continue };
        if !is_asset_kind(primary.kind) {
            continue;
        }
        let meta = primary_meta.unwrap_or_else(|| FileMeta { meta: tidy_meta(&entries[0].meta), dims: None, duration: None, codec: None });
        let asset = link_group(&tx, &members, false, Some(import_id), || meta, &mut counters)?;
        report.assets += 1;
        report.touched_assets.push(asset);

        for e in entries {
            for o in &e.outputs {
                let root_id = render_roots[&o.root];
                let preset_name = m.preset.as_ref().filter(|p| p.hash == o.preset_hash).map(|p| p.name.clone());
                tx.prepare_cached(
                    "INSERT INTO renders (asset_id, kind, root_id, rel, preset_name, preset_hash, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT (root_id, rel) DO UPDATE SET asset_id = excluded.asset_id, kind = excluded.kind,
                        preset_name = COALESCE(excluded.preset_name, renders.preset_name), preset_hash = excluded.preset_hash,
                        created_at = excluded.created_at",
                )?
                .execute(params![
                    asset,
                    match o.kind {
                        crate::manifest::OutputKind::Jpeg => "jpeg",
                        crate::manifest::OutputKind::Exr => "exr",
                    },
                    root_id,
                    o.rel,
                    preset_name,
                    o.preset_hash,
                    o.rendered_at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
                ])?;
                report.renders += 1;
            }
        }
    }
    tx.commit()?;
    report.assets_created = counters.created;
    Ok(report)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AdoptReport {
    pub root: i64,
    pub manifests: usize,
    pub files: usize,
    pub missing_files: usize,
    pub assets: usize,
    pub assets_created: usize,
    pub renders: usize,
    pub errors: Vec<(String, String)>,
    #[serde(skip)]
    pub touched_assets: Vec<i64>,
}

/// Bring an existing archive into the catalog from its import manifests (oldest first). Only
/// reads: manifests are parsed and files are stat'ed; nothing under the archive is written.
pub fn adopt_archive(cat: &mut Catalog, archive_root: &Path, progress: &mut dyn FnMut(IndexProgress)) -> anyhow::Result<AdoptReport> {
    let mut manifests = list_manifests(archive_root);
    if manifests.is_empty() {
        anyhow::bail!("no import manifests in {}", archive_root.join(crate::manifest::DIR).display());
    }
    manifests.reverse();
    let mut report = AdoptReport::default();
    let total = manifests.len();
    for (i, path) in manifests.iter().enumerate() {
        progress(IndexProgress::Manifest { done: i, total, name: path.file_name().unwrap_or_default().to_string_lossy().to_string() });
        let result = Manifest::read(path).and_then(|m| index_manifest(cat, path, &m));
        match result {
            Ok(r) => {
                report.manifests += 1;
                report.files += r.files;
                report.missing_files += r.missing_files;
                report.assets += r.assets;
                report.assets_created += r.assets_created;
                report.renders += r.renders;
                report.touched_assets.extend(r.touched_assets);
            }
            Err(e) => report.errors.push((path.to_string_lossy().to_string(), e.to_string())),
        }
    }
    progress(IndexProgress::Manifest { done: total, total, name: String::new() });
    report.root = cat.root_by_path(archive_root)?.map(|(id, _)| id).unwrap_or(0);
    if report.manifests == 0 {
        anyhow::bail!("no manifest could be read: {}", report.errors.first().map(|e| e.1.as_str()).unwrap_or("?"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::testutil::write;
    use crate::catalog::keywords;

    fn silent() -> impl FnMut(IndexProgress) {
        |_| {}
    }

    fn assets(cat: &Catalog) -> Vec<(i64, String, String, Vec<(String, String)>)> {
        // (asset id, kind, primary name, [(file name, role)])
        let mut stmt = cat
            .conn()
            .prepare(
                "SELECT a.id, a.kind, p.name, f.name, af.role FROM assets a
                 JOIN files p ON p.id = a.primary_file_id
                 JOIN asset_files af ON af.asset_id = a.id JOIN files f ON f.id = af.file_id
                 ORDER BY a.id, f.name",
            )
            .unwrap();
        let rows: Vec<(i64, String, String, String, String)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap().collect::<Result<_, _>>().unwrap();
        let mut out: Vec<(i64, String, String, Vec<(String, String)>)> = Vec::new();
        for (id, kind, primary, name, role) in rows {
            match out.last_mut() {
                Some(last) if last.0 == id => last.3.push((name, role)),
                _ => out.push((id, kind, primary, vec![(name, role)])),
            }
        }
        out
    }

    fn by_primary<'a>(a: &'a [(i64, String, String, Vec<(String, String)>)], name: &str) -> &'a (i64, String, String, Vec<(String, String)>) {
        a.iter().find(|x| x.2 == name).unwrap_or_else(|| panic!("no asset with primary {name}: {a:?}"))
    }

    #[test]
    fn groups_raf_and_jpg_into_one_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("photos");
        write(&root, "2024/DSCF0001.RAF", 100, 1_700_000_000);
        write(&root, "2024/DSCF0001.JPG", 40, 1_700_000_000);
        write(&root, "2024/DSCF0002.RAF", 100, 1_700_000_100);
        write(&root, "2024/lonely.jpg", 30, 1_700_000_200);
        write(&root, "2024/C0001.MP4", 500, 1_700_000_300);
        write(&root, "2024/C0001M01.XML", 5, 1_700_000_300);
        write(&root, "notes.txt", 5, 1_700_000_300);
        write(&root, ".hidden/secret.jpg", 5, 1_700_000_300);

        let mut cat = Catalog::open_in_memory().unwrap();
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!(r.files, 7, "hidden folders are skipped");
        assert_eq!(r.added, 7);
        assert_eq!(r.assets_created, 4);

        let a = assets(&cat);
        assert_eq!(a.len(), 4, "{a:?}");
        let pair = by_primary(&a, "DSCF0001.RAF");
        assert_eq!(pair.1, "raw");
        assert_eq!(pair.3, vec![("DSCF0001.JPG".into(), "jpeg".into()), ("DSCF0001.RAF".into(), "raw".into())]);
        assert_eq!(by_primary(&a, "DSCF0002.RAF").3.len(), 1);
        assert_eq!(by_primary(&a, "lonely.jpg").1, "image");
        let vid = by_primary(&a, "C0001.MP4");
        assert_eq!(vid.3, vec![("C0001.MP4".into(), "video".into()), ("C0001M01.XML".into(), "sidecar".into())]);
        // The loose text file is indexed as a file but is not an asset.
        let n: i64 = cat.conn().query_row("SELECT COUNT(*) FROM files WHERE name = 'notes.txt'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // Relative paths.
        let rel: String = cat.conn().query_row("SELECT rel FROM files WHERE name = 'DSCF0001.JPG'", [], |r| r.get(0)).unwrap();
        assert_eq!(rel, "2024/DSCF0001.JPG");
    }

    #[test]
    fn incremental_reindex_skips_unchanged_and_keeps_keywords() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("photos");
        write(&root, "a/IMG_1.JPG", 10, 1_700_000_000);
        write(&root, "a/IMG_2.JPG", 10, 1_700_000_000);
        write(&root, "b/DSCF0009.RAF", 50, 1_700_000_000);
        let mut cat = Catalog::open_in_memory().unwrap();
        add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        let before = assets(&cat);
        let img1 = by_primary(&before, "IMG_1.JPG").0;
        let raw = by_primary(&before, "DSCF0009.RAF").0;
        keywords::add(cat.conn(), &[img1, raw], &["beach".into()], keywords::USER).unwrap();

        // Nothing changed: everything is skipped and no asset moves.
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!((r.unchanged, r.added, r.changed, r.moved, r.missing), (3, 0, 0, 0, 0));
        assert_eq!(assets(&cat), before);

        // A camera JPEG appears next to the RAW: it joins the RAW's asset, keywords stay.
        write(&root, "b/DSCF0009.JPG", 5, 1_700_000_000);
        // IMG_2 changes content.
        write(&root, "a/IMG_2.JPG", 12, 1_700_000_500);
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!((r.unchanged, r.added, r.changed), (2, 1, 1));
        let after = assets(&cat);
        let pair = by_primary(&after, "DSCF0009.RAF");
        assert_eq!(pair.0, raw, "asset id is stable");
        assert_eq!(pair.3.len(), 2);
        assert_eq!(keywords::of_asset(cat.conn(), raw).unwrap().len(), 1);

        // A file vanishes: kept as missing, keywords intact.
        std::fs::remove_file(root.join("a/IMG_1.JPG")).unwrap();
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!(r.missing, 1);
        let missing: i64 = cat.conn().query_row("SELECT missing FROM files WHERE name = 'IMG_1.JPG'", [], |r| r.get(0)).unwrap();
        assert_eq!(missing, 1);
        assert_eq!(keywords::of_asset(cat.conn(), img1).unwrap().len(), 1);

        // ... and reappears in another folder (moved): same file row, same asset, keywords follow.
        write(&root, "c/IMG_1.JPG", 10, 1_700_000_000);
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!((r.moved, r.added), (1, 0));
        let (rel, missing): (String, i64) =
            cat.conn().query_row("SELECT rel, missing FROM files WHERE name = 'IMG_1.JPG'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((rel.as_str(), missing), ("c/IMG_1.JPG", 0));
        assert_eq!(by_primary(&assets(&cat), "IMG_1.JPG").0, img1);
        assert_eq!(keywords::of_asset(cat.conn(), img1).unwrap()[0].name, "beach");
    }

    #[test]
    fn lone_jpeg_asset_is_kept_when_its_raw_arrives() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        write(&root, "DSCF0100.JPG", 10, 1_700_000_000);
        let mut cat = Catalog::open_in_memory().unwrap();
        add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        let jpg_asset = assets(&cat)[0].0;
        keywords::add(cat.conn(), &[jpg_asset], &["dog".into()], keywords::USER).unwrap();

        write(&root, "DSCF0100.RAF", 60, 1_700_000_000);
        add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        let a = assets(&cat);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, jpg_asset, "the existing asset (and its keywords) is reused");
        assert_eq!(a[0].1, "raw", "the RAW becomes the primary");
        assert_eq!(a[0].2, "DSCF0100.RAF");
        assert_eq!(keywords::of_asset(cat.conn(), jpg_asset).unwrap().len(), 1);
    }

    #[test]
    fn offline_root_is_left_alone_and_relocate_repoints_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("drive/photos");
        write(&root, "x/DSCF0001.RAF", 10, 1_700_000_000);
        write(&root, "x/DSCF0001.JPG", 10, 1_700_000_000);
        let mut cat = Catalog::open_in_memory().unwrap();
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        let root_id = r.root;

        // The drive goes away: rescanning marks the root offline and flags no files missing.
        let moved = tmp.path().join("other-drive/photos");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::rename(&root, &moved).unwrap();
        let r = index_root(&mut cat, root_id, &mut silent(), &Cancel::new()).unwrap();
        assert!(r.offline);
        let missing: i64 = cat.conn().query_row("SELECT COUNT(*) FROM files WHERE missing = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(missing, 0);
        assert!(!cat.root(root_id).unwrap().online);

        // Relocating to an unrelated folder is refused; to the real new place it works.
        let wrong = tmp.path().join("empty");
        std::fs::create_dir_all(&wrong).unwrap();
        assert!(cat.relocate_root(root_id, &wrong, false).is_err());
        let rep = cat.relocate_root(root_id, &moved, false).unwrap();
        assert_eq!((rep.checked, rep.found), (2, 2));
        let r = index_root(&mut cat, root_id, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!((r.unchanged, r.added, r.missing), (2, 0, 0));
        assert!(cat.root(root_id).unwrap().online);
        assert_eq!(assets(&cat).len(), 1);
    }

    #[test]
    fn nested_folders_are_refused_and_removing_a_root_forgets_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        write(&root, "a/one.jpg", 10, 1_700_000_000);
        let mut cat = Catalog::open_in_memory().unwrap();
        let r = add_folder(&mut cat, &root, &mut silent(), &Cancel::new()).unwrap();
        assert!(add_folder(&mut cat, &root.join("a"), &mut silent(), &Cancel::new()).is_err());
        let id = assets(&cat)[0].0;
        keywords::add(cat.conn(), &[id], &["x".into()], keywords::USER).unwrap();
        cat.remove_root(r.root).unwrap();
        assert!(assets(&cat).is_empty());
        let n: i64 = cat.conn().query_row("SELECT COUNT(*) FROM keywords", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        assert!(root.join("a/one.jpg").exists(), "nothing on disk is touched");
    }

    #[test]
    fn adopts_manifest_with_renders_idempotently() {
        use crate::config::Config;
        use crate::manifest::{Entry, OutputKind, OutputRecord};
        use crate::scan::PreviewSource;
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.archive_root = tmp.path().join("archive");
        cfg.render_root = tmp.path().join("renders");
        write(&cfg.archive_root, "2024/X-T10/DSCF0001.RAF", 30, 1_700_000_000);
        write(&cfg.archive_root, "2024/X-T10/DSCF0001.JPG", 10, 1_700_000_000);
        write(&cfg.render_root, "2024/X-T10/DSCF0001.jpg", 7, 1_700_000_000);
        let meta = CaptureMeta {
            captured_at: chrono::NaiveDate::from_ymd_opt(2024, 10, 4).unwrap().and_hms_opt(13, 55, 10),
            make: Some("Fujifilm".into()),
            model: Some("X-T10".into()),
            camera: Some("X-T10".into()),
            iso: Some(1000),
            lens: None,
            orientation: Some(1),
            source: MetaSource::Raw,
        };
        let preset = crate::manifest::PresetRecord {
            name: "Portra".into(),
            film: "kodak_portra_400".into(),
            print: "kodak_portra_endura".into(),
            params: serde_json::json!({}),
            hash: "abc123".into(),
            spektrafilm_rev: "x".into(),
        };
        let mut m = Manifest::new(&cfg, "CARD", Path::new("/Volumes/CARD"), Some(preset));
        let raw = Entry {
            id: 0,
            group: 0,
            seq: 1,
            kind: Kind::Raw,
            is_primary: true,
            source_rel: "DCIM/DSCF0001.RAF".into(),
            root: PlanRoot::Archive,
            rel: "2024/X-T10/DSCF0001.RAF".into(),
            size: 30,
            blake3: Some("h1".into()),
            copy: CopyStatus::Copied,
            meta: meta.clone(),
            preview: PreviewSource::SidecarJpeg,
            outputs: vec![OutputRecord {
                kind: OutputKind::Jpeg,
                root: cfg.render_root.clone(),
                rel: "2024/X-T10/DSCF0001.jpg".into(),
                rendered_at: chrono::Utc::now(),
                preset_hash: "abc123".into(),
            }],
        };
        let jpg = Entry { id: 1, kind: Kind::Image, is_primary: false, rel: "2024/X-T10/DSCF0001.JPG".into(), size: 10, outputs: vec![], ..raw.clone() };
        let failed = Entry { id: 2, group: 1, rel: "2024/X-T10/DSCF0002.RAF".into(), copy: CopyStatus::Failed("io".into()), outputs: vec![], ..raw.clone() };
        m.entries = vec![raw, jpg, failed];
        let path = m.default_path();
        m.write(&path).unwrap();

        let mut cat = Catalog::open_in_memory().unwrap();
        let r = adopt_archive(&mut cat, &cfg.archive_root, &mut silent()).unwrap();
        assert_eq!((r.manifests, r.files, r.assets, r.renders, r.assets_created), (1, 2, 1, 1, 1));
        let a = assets(&cat);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].3, vec![("DSCF0001.JPG".into(), "jpeg".into()), ("DSCF0001.RAF".into(), "raw".into())]);
        let (camera, captured): (String, String) =
            cat.conn().query_row("SELECT camera, captured_at FROM assets", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((camera.as_str(), captured.as_str()), ("X-T10", "2024-10-04T13:55:10"));
        let (kind, preset_name): (String, String) =
            cat.conn().query_row("SELECT kind, preset_name FROM renders", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((kind.as_str(), preset_name.as_str()), ("jpeg", "Portra"));
        let blake: String = cat.conn().query_row("SELECT blake3 FROM files WHERE name = 'DSCF0001.RAF'", [], |r| r.get(0)).unwrap();
        assert_eq!(blake, "h1");

        // Adopting again changes nothing; a folder rescan of the archive sees every file unchanged.
        let r = adopt_archive(&mut cat, &cfg.archive_root, &mut silent()).unwrap();
        assert_eq!(r.assets_created, 0);
        assert_eq!(assets(&cat), a);
        let root = cat.root_by_path(&cfg.archive_root).unwrap().unwrap().0;
        let r = index_root(&mut cat, root, &mut silent(), &Cancel::new()).unwrap();
        assert_eq!((r.unchanged, r.added, r.changed), (2, 0, 0));
        assert_eq!(assets(&cat), a);
        // The render root is registered but never walked for photos.
        assert!(cat.roots().unwrap().iter().any(|r| r.kind == RootKind::Render));
        let render_root = cat.root_by_path(&cfg.render_root).unwrap().unwrap().0;
        assert!(index_root(&mut cat, render_root, &mut silent(), &Cancel::new()).is_err());
    }
}
