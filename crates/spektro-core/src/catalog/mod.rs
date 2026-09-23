//! The photo catalog: a SQLite index of everything the app knows about, so the UI can present
//! photos however it likes instead of by folder.
//!
//! - **roots**: an import archive (`archive`), a folder added in place (`folder`, never moved or
//!   renamed by us), or a render output folder (`render`). Every path below is stored relative
//!   to its root, so a moved root is fixed with one `relocate_root`.
//! - **files**: one row per file under a root (size, mtime, BLAKE3 when known). Files that
//!   vanish are kept with `missing = 1` so their asset and keywords survive an offline drive or
//!   a temporary rename; a file that reappears elsewhere in the same root with the same name,
//!   size and mtime is treated as moved and keeps its asset.
//! - **assets**: the logical photo (or video). A RAW and its camera JPEG are one asset with two
//!   files (`asset_files.role` = `raw` / `jpeg`), grouped exactly like the import scanner does.
//! - **keywords**: per asset, with a `source` of `user` or `ai:<name>`; a user keyword wins
//!   over an AI one, and AI labels can be cleared without touching the user's.
//! - **thumbnails**: cache files keyed by asset id + size, generated in the background.
//! - **renders**: "prints" of an asset through a spektrafilm preset (JPEG/EXR), plus the preset
//!   records they were made with.
//!
//! Connections: one per thread. SQLite runs in WAL mode, so the UI can read while an indexer or
//! the thumbnail writer holds a write transaction; `busy_timeout` covers writer overlap.

pub mod duplicates;
pub mod import_dupes;
pub mod index;
pub mod keywords;
pub mod print;
pub mod purge;
pub mod query;
mod schema;
pub mod thumbs;

use crate::scan::Kind;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use schema::SCHEMA_VERSION;

pub struct Catalog {
    conn: Connection,
    path: Option<PathBuf>,
}

impl Catalog {
    /// Open (creating if needed) and migrate the catalog at `path`.
    pub fn open(path: &Path) -> anyhow::Result<Catalog> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        configure(&conn, true)?;
        schema::migrate(&mut conn)?;
        Ok(Catalog { conn, path: Some(path.to_path_buf()) })
    }

    /// A private in-memory catalog (tests, dry runs).
    pub fn open_in_memory() -> anyhow::Result<Catalog> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn, false)?;
        schema::migrate(&mut conn)?;
        Ok(Catalog { conn, path: None })
    }

    /// Another connection to the same database, for a worker thread.
    pub fn reopen(&self) -> anyhow::Result<Catalog> {
        match &self.path {
            Some(p) => Catalog::open(p),
            None => anyhow::bail!("an in-memory catalog cannot be reopened"),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn configure(conn: &Connection, wal: bool) -> anyhow::Result<()> {
    if wal {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(())
}

/// The desktop app's bundle identifier; the CLI shares the app's catalog and
/// thumbnail cache (Tauri's `app_data_dir` / `app_cache_dir`).
pub const APP_ID: &str = "dev.alexharding.spektrorganize";

/// Default catalog: `<data dir>/dev.alexharding.spektrorganize/catalog.sqlite`
/// (`~/Library/Application Support` on macOS, `~/.local/share` on Linux) —
/// the same database the desktop app opens.
pub fn default_path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| crate::config::dirs_home().join(".local/share")).join(APP_ID).join("catalog.sqlite")
}

/// Default thumbnail cache, shared with the app: `<cache dir>/dev.alexharding.spektrorganize/catalog-thumbs`.
pub fn default_thumbs_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| crate::config::dirs_home().join(".cache")).join(APP_ID).join("catalog-thumbs")
}

/// A dynamic catalog: a named filter, re-run every time it is opened.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub filter: serde_json::Value,
    pub sort: Option<String>,
    pub created_at: String,
}

// ---------- small shared helpers ----------

pub(crate) fn now() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(crate) fn mtime_ns(t: Option<SystemTime>) -> Option<i64> {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64)
}

/// Forward-slash relative path string.
pub(crate) fn rel_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

pub(crate) fn kind_from_str(s: &str) -> Kind {
    match s {
        "raw" => Kind::Raw,
        "image" => Kind::Image,
        "video" => Kind::Video,
        "sidecar" => Kind::Sidecar,
        _ => Kind::Other,
    }
}

/// Rank for choosing an asset's primary file: RAW beats image beats video.
pub(crate) fn primary_rank(kind: Kind) -> u8 {
    match kind {
        Kind::Raw => 0,
        Kind::Image => 1,
        Kind::Video => 2,
        _ => 3,
    }
}

/// Only photos and videos become assets; loose sidecars/unknown files are indexed as files.
pub(crate) fn is_asset_kind(kind: Kind) -> bool {
    matches!(kind, Kind::Raw | Kind::Image | Kind::Video)
}

/// What a file is to its asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Raw,
    /// A camera JPEG next to a RAW (the "sidecar JPEG").
    Jpeg,
    /// The asset's own image (lone JPEG/HEIF), or a non-JPEG image next to a RAW.
    Image,
    Video,
    Xmp,
    Sidecar,
    Other,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Raw => "raw",
            Role::Jpeg => "jpeg",
            Role::Image => "image",
            Role::Video => "video",
            Role::Xmp => "xmp",
            Role::Sidecar => "sidecar",
            Role::Other => "other",
        }
    }

    pub fn for_file(name: &str, kind: Kind, is_primary: bool) -> Role {
        let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match (kind, is_primary) {
            (Kind::Raw, _) => Role::Raw,
            (Kind::Image, false) if ext == "jpg" || ext == "jpeg" => Role::Jpeg,
            (Kind::Image, _) => Role::Image,
            (Kind::Video, _) => Role::Video,
            _ if ext == "xmp" => Role::Xmp,
            (Kind::Sidecar, _) => Role::Sidecar,
            _ => Role::Other,
        }
    }
}

// ---------- roots ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    /// Written by imports (token-template layout); manifests live in `.spektrorganize/`.
    Archive,
    /// An existing folder indexed in place; never moved or renamed.
    Folder,
    /// Render ("print") outputs. Not walked for assets.
    Render,
}

impl RootKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RootKind::Archive => "archive",
            RootKind::Folder => "folder",
            RootKind::Render => "render",
        }
    }
    pub fn parse(s: &str) -> RootKind {
        match s {
            "archive" => RootKind::Archive,
            "render" => RootKind::Render,
            _ => RootKind::Folder,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Root {
    pub id: i64,
    pub path: PathBuf,
    pub kind: RootKind,
    pub label: String,
    pub added_at: String,
    pub online: bool,
    pub last_indexed_at: Option<String>,
    /// Assets whose primary file is under this root.
    pub assets: i64,
    pub files: i64,
    pub missing: i64,
    /// Render outputs recorded under this root (render roots).
    pub renders: i64,
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl Catalog {
    pub fn roots(&self) -> anyhow::Result<Vec<Root>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.path, r.kind, r.label, r.added_at, r.online, r.last_indexed_at,
                    (SELECT COUNT(*) FROM assets a JOIN files f ON f.id = a.primary_file_id WHERE f.root_id = r.id),
                    (SELECT COUNT(*) FROM files f WHERE f.root_id = r.id),
                    (SELECT COUNT(*) FROM files f WHERE f.root_id = r.id AND f.missing = 1),
                    (SELECT COUNT(*) FROM renders rr WHERE rr.root_id = r.id)
             FROM roots r ORDER BY r.kind = 'render', r.path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Root {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                kind: RootKind::parse(&r.get::<_, String>(2)?),
                label: r.get(3)?,
                added_at: r.get(4)?,
                online: r.get::<_, i64>(5)? != 0,
                last_indexed_at: r.get(6)?,
                assets: r.get(7)?,
                files: r.get(8)?,
                missing: r.get(9)?,
                renders: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn root(&self, id: i64) -> anyhow::Result<Root> {
        self.roots()?.into_iter().find(|r| r.id == id).ok_or_else(|| anyhow::anyhow!("no root #{id}"))
    }

    pub fn root_by_path(&self, path: &Path) -> anyhow::Result<Option<(i64, RootKind)>> {
        let p = canonical(path);
        Ok(self
            .conn
            .query_row("SELECT id, kind FROM roots WHERE path = ?1", [p.to_string_lossy()], |r| {
                Ok((r.get::<_, i64>(0)?, RootKind::parse(&r.get::<_, String>(1)?)))
            })
            .optional()?)
    }

    /// The root that contains `path` (itself or an ancestor), if any.
    pub fn root_containing(&self, path: &Path) -> anyhow::Result<Option<Root>> {
        let p = canonical(path);
        Ok(self.roots()?.into_iter().filter(|r| p.starts_with(&r.path)).max_by_key(|r| r.path.as_os_str().len()))
    }

    /// Get or create the root at `path`. A path inside an existing archive/folder root is
    /// refused (its files are already indexed under that root).
    pub fn ensure_root(&self, path: &Path, kind: RootKind) -> anyhow::Result<i64> {
        let p = canonical(path);
        if let Some((id, existing)) = self.root_by_path(&p)? {
            if existing != kind && (existing == RootKind::Render) != (kind == RootKind::Render) {
                anyhow::bail!("{} is already in the catalog as a {} root", p.display(), existing.as_str());
            }
            return Ok(id);
        }
        if kind != RootKind::Render
            && let Some(parent) = self.root_containing(&p)?
            && parent.kind != RootKind::Render
        {
            anyhow::bail!(
                "{} is inside {} ({} root #{}), which is already indexed; rescan that root instead",
                p.display(),
                parent.path.display(),
                parent.kind.as_str(),
                parent.id
            );
        }
        let label = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string());
        self.conn.execute(
            "INSERT INTO roots (path, kind, label, added_at, online) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![p.to_string_lossy(), kind.as_str(), label, now(), p.is_dir() as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Re-check which roots are reachable (a disconnected drive goes offline, not missing).
    /// Returns whether anything changed, so a caller polling this knows when to tell the UI.
    pub fn refresh_online(&self) -> anyhow::Result<bool> {
        let mut changed = false;
        for r in self.roots()? {
            let online = r.path.is_dir();
            if online != r.online {
                self.conn.execute("UPDATE roots SET online = ?2 WHERE id = ?1", params![r.id, online as i64])?;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Point a root at its new location (the whole tree moved, e.g. to another drive). Checks a
    /// sample of its files exists there first unless `force`.
    pub fn relocate_root(&self, id: i64, new_path: &Path, force: bool) -> anyhow::Result<RelocateReport> {
        let root = self.root(id)?;
        if !new_path.is_dir() {
            anyhow::bail!("{} is not a folder", new_path.display());
        }
        let p = canonical(new_path);
        if let Some((other, _)) = self.root_by_path(&p)?
            && other != id
        {
            anyhow::bail!("{} is already root #{other}", p.display());
        }
        let sample: Vec<String> = {
            let sql = if root.kind == RootKind::Render {
                "SELECT rel FROM renders WHERE root_id = ?1 ORDER BY id LIMIT 50"
            } else {
                "SELECT rel FROM files WHERE root_id = ?1 AND missing = 0 ORDER BY id LIMIT 50"
            };
            let mut stmt = self.conn.prepare(sql)?;
            stmt.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let found = sample.iter().filter(|rel| p.join(rel).exists()).count();
        if !force && !sample.is_empty() && found == 0 {
            anyhow::bail!("none of {} sampled files of {} exist under {}", sample.len(), root.path.display(), p.display());
        }
        self.conn.execute("UPDATE roots SET path = ?2, online = 1 WHERE id = ?1", params![id, p.to_string_lossy()])?;
        Ok(RelocateReport { root: id, from: root.path, to: p, checked: sample.len(), found })
    }

    /// Forget a root and every asset under it. Nothing on disk is touched.
    pub fn remove_root(&mut self, id: i64) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM assets WHERE id IN (SELECT af.asset_id FROM asset_files af JOIN files f ON f.id = af.file_id WHERE f.root_id = ?1)",
            [id],
        )?;
        tx.execute("DELETE FROM roots WHERE id = ?1", [id])?;
        tx.commit()?;
        keywords::prune_unused(&self.conn)?;
        Ok(())
    }

    pub fn asset_count(&self) -> anyhow::Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))?)
    }

    // ---------- dynamic catalogs ----------

    /// The saved filters, newest first.
    pub fn collections(&self) -> anyhow::Result<Vec<Collection>> {
        let mut stmt = self.conn.prepare("SELECT id, name, filter, sort, created_at FROM collections ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            let filter: String = r.get(2)?;
            Ok(Collection {
                id: r.get(0)?,
                name: r.get(1)?,
                filter: serde_json::from_str(&filter).unwrap_or(serde_json::Value::Null),
                sort: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Save a filter under a name, replacing one of the same name.
    pub fn save_collection(&self, name: &str, filter: &serde_json::Value, sort: Option<&str>) -> anyhow::Result<i64> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a dynamic catalog needs a name");
        self.conn.execute(
            "INSERT INTO collections (name, filter, sort, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET filter = excluded.filter, sort = excluded.sort",
            params![name, filter.to_string(), sort, now()],
        )?;
        Ok(self.conn.query_row("SELECT id FROM collections WHERE name = ?1", [name], |r| r.get(0))?)
    }

    pub fn delete_collection(&self, id: i64) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM collections WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Where one file lives, by its catalog id.
    pub fn file_path(&self, file_id: i64) -> anyhow::Result<Option<std::path::PathBuf>> {
        use rusqlite::OptionalExtension as _;
        Ok(self
            .conn
            .query_row(
                "SELECT r.path, f.rel FROM files f JOIN roots r ON r.id = f.root_id WHERE f.id = ?1",
                [file_id],
                |row| {
                    let root: String = row.get(0)?;
                    let rel: String = row.get(1)?;
                    Ok(std::path::Path::new(&root).join(rel))
                },
            )
            .optional()?)
    }

    /// Where a render lives, and whether its root is online.
    pub fn render_path(&self, render: i64) -> anyhow::Result<Option<(std::path::PathBuf, bool)>> {
        use rusqlite::OptionalExtension as _;
        Ok(self
            .conn
            .query_row(
                "SELECT r.path, d.rel, r.online FROM renders d JOIN roots r ON r.id = d.root_id WHERE d.id = ?1",
                [render],
                |row| {
                    let root: String = row.get(0)?;
                    let rel: String = row.get(1)?;
                    Ok((std::path::Path::new(&root).join(rel), row.get(2)?))
                },
            )
            .optional()?)
    }

    /// A catalog-wide preference, `None` when never set.
    pub fn setting(&self, key: &str) -> Option<String> {
        use rusqlite::OptionalExtension as _;
        self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Forget the thumbnails of every photo that has a print, so they are made again
    /// from the other source. Used when `thumbs_from_prints` is switched.
    pub fn forget_print_thumbnails(&self) -> anyhow::Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM thumbnails WHERE asset_id IN (SELECT DISTINCT asset_id FROM renders WHERE kind = 'jpeg')", [])?)
    }

    /// A photo's pixel size, when the indexer could read it.
    pub fn dimensions(&self, id: i64) -> anyhow::Result<(Option<i64>, Option<i64>)> {
        Ok(self.conn.query_row("SELECT width, height FROM assets WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// A photo's RAW settings (the honest decode when none are stored).
    pub fn raw_settings(&self, id: i64) -> anyhow::Result<crate::decode::RawSettings> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT raw_settings FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
    }

    /// Store RAW settings on photos; the defaults clear them.
    pub fn set_raw_settings(&self, ids: &[i64], raw: &crate::decode::RawSettings) -> anyhow::Result<()> {
        let json = if raw.is_default() { None } else { Some(serde_json::to_string(raw)?) };
        for id in ids {
            self.conn.execute("UPDATE assets SET raw_settings = ?2 WHERE id = ?1", params![id, json])?;
        }
        Ok(())
    }

    /// Photos queued for export, oldest first.
    pub fn export_queue(&self) -> anyhow::Result<Vec<i64>> {
        let mut stmt = self.conn.prepare("SELECT asset_id FROM export_queue ORDER BY added_at, asset_id")?;
        let ids = stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }

    pub fn export_queue_add(&self, ids: &[i64]) -> anyhow::Result<usize> {
        let now = now();
        let mut added = 0;
        for id in ids {
            added += self
                .conn
                .execute("INSERT OR IGNORE INTO export_queue (asset_id, added_at) VALUES (?1, ?2)", params![id, now])?;
        }
        Ok(added)
    }

    pub fn export_queue_remove(&self, ids: &[i64]) -> anyhow::Result<()> {
        for id in ids {
            self.conn.execute("DELETE FROM export_queue WHERE asset_id = ?1", [id])?;
        }
        Ok(())
    }

    pub fn export_queue_clear(&self) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM export_queue", [])?;
        Ok(())
    }

    /// Mark photos as picked ("select"), rejected ("reject"), or neither.
    pub fn set_flag(&self, ids: &[i64], flag: Option<&str>) -> anyhow::Result<()> {
        let flag = match flag {
            Some("select") => Some("select"),
            Some("reject") => Some("reject"),
            _ => None,
        };
        for id in ids {
            self.conn.execute("UPDATE assets SET flag = ?2 WHERE id = ?1", params![id, flag])?;
        }
        Ok(())
    }

    pub fn set_rating(&self, ids: &[i64], rating: i64) -> anyhow::Result<()> {
        let rating = rating.clamp(0, 5);
        for id in ids {
            self.conn.execute("UPDATE assets SET rating = ?2 WHERE id = ?1", params![id, rating])?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RelocateReport {
    pub root: i64,
    pub from: PathBuf,
    pub to: PathBuf,
    pub checked: usize,
    pub found: usize,
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::Path;

    /// Write a file with `len` bytes of deterministic content and a fixed mtime.
    pub fn write(root: &Path, rel: &str, len: usize, mtime_secs: i64) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![b'x'; len]).unwrap();
        filetime::set_file_mtime(&p, filetime::FileTime::from_unix_time(mtime_secs, 0)).unwrap();
    }
}

#[cfg(test)]
mod collection_tests {
    use super::*;

    #[test]
    fn save_open_and_delete_a_dynamic_catalog() {
        let cat = Catalog::open_in_memory().unwrap();
        let filter = serde_json::json!({ "cameras": ["X-T10"], "min_rating": 3 });
        let id = cat.save_collection("Keepers", &filter, Some("captured_desc")).unwrap();

        let all = cat.collections().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Keepers");
        assert_eq!(all[0].filter, filter);
        assert_eq!(all[0].sort.as_deref(), Some("captured_desc"));

        // Saving the same name replaces the filter rather than adding a second row.
        let again = cat.save_collection("Keepers", &serde_json::json!({ "min_rating": 5 }), None).unwrap();
        assert_eq!(again, id);
        let all = cat.collections().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].filter["min_rating"], 5);

        assert!(cat.save_collection("  ", &filter, None).is_err());

        cat.delete_collection(id).unwrap();
        assert!(cat.collections().unwrap().is_empty());
    }

    #[test]
    fn settings_round_trip() {
        let cat = Catalog::open_in_memory().unwrap();
        assert_eq!(cat.setting("thumbs_from_prints"), None);
        cat.set_setting("thumbs_from_prints", "1").unwrap();
        assert_eq!(cat.setting("thumbs_from_prints").as_deref(), Some("1"));
        cat.set_setting("thumbs_from_prints", "0").unwrap();
        assert_eq!(cat.setting("thumbs_from_prints").as_deref(), Some("0"));
    }
}
