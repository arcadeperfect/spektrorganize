//! Schema and migrations. `PRAGMA user_version` holds the number of migrations applied; each
//! migration runs in its own transaction. Append new migrations, never edit shipped ones.

use rusqlite::Connection;

const V1: &str = r#"
CREATE TABLE roots (
    id              INTEGER PRIMARY KEY,
    path            TEXT NOT NULL UNIQUE,          -- absolute
    kind            TEXT NOT NULL CHECK (kind IN ('archive', 'folder', 'render')),
    label           TEXT NOT NULL,
    added_at        TEXT NOT NULL,
    online          INTEGER NOT NULL DEFAULT 1,
    last_indexed_at TEXT
);

-- One row per import manifest indexed from an archive root.
CREATE TABLE imports (
    id           INTEGER PRIMARY KEY,
    root_id      INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    manifest_rel TEXT NOT NULL,                     -- relative to the root
    source_label TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    indexed_at   TEXT NOT NULL,
    preset_hash  TEXT,
    UNIQUE (root_id, manifest_rel)
);

CREATE TABLE files (
    id         INTEGER PRIMARY KEY,
    root_id    INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    rel        TEXT NOT NULL,                       -- '/'-separated, relative to the root
    name       TEXT NOT NULL,                       -- file name, for search and display
    kind       TEXT NOT NULL,                       -- raw | image | video | sidecar | other
    size       INTEGER NOT NULL,
    mtime_ns   INTEGER,
    blake3     TEXT,
    missing    INTEGER NOT NULL DEFAULT 0,          -- vanished on the last scan of an online root
    indexed_at TEXT NOT NULL,
    UNIQUE (root_id, rel)
);
CREATE INDEX files_name ON files(name COLLATE NOCASE);

CREATE TABLE assets (
    id              INTEGER PRIMARY KEY,
    kind            TEXT NOT NULL,                  -- kind of the primary file
    primary_file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
    captured_at     TEXT,                           -- camera wall clock, 'YYYY-MM-DDTHH:MM:SS'
    make            TEXT,
    model           TEXT,
    camera          TEXT,                           -- normalised model
    lens            TEXT,
    iso             INTEGER,
    width           INTEGER,                        -- as displayed (orientation applied)
    height          INTEGER,
    orientation     INTEGER,                        -- EXIF value
    meta_source     TEXT,                           -- raw | exif | mtime
    rating          INTEGER NOT NULL DEFAULT 0,
    import_id       INTEGER REFERENCES imports(id) ON DELETE SET NULL,
    added_at        TEXT NOT NULL
);
CREATE INDEX assets_captured ON assets(captured_at);
CREATE INDEX assets_camera ON assets(camera);
CREATE INDEX assets_primary ON assets(primary_file_id);

-- A file belongs to at most one asset.
CREATE TABLE asset_files (
    file_id  INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    role     TEXT NOT NULL                          -- raw | jpeg | image | video | xmp | sidecar | other
);
CREATE INDEX asset_files_asset ON asset_files(asset_id);

CREATE TABLE keywords (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE
);

-- One row per (asset, keyword). `source` is 'user' or 'ai:<labeller>'; a user keyword wins.
CREATE TABLE asset_keywords (
    asset_id   INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    keyword_id INTEGER NOT NULL REFERENCES keywords(id) ON DELETE CASCADE,
    source     TEXT NOT NULL DEFAULT 'user',
    confidence REAL,
    added_at   TEXT NOT NULL,
    PRIMARY KEY (asset_id, keyword_id)
) WITHOUT ROWID;
CREATE INDEX asset_keywords_keyword ON asset_keywords(keyword_id);

-- path NULL = generation failed for this src_key (negative cache; retried when the source changes).
CREATE TABLE thumbnails (
    asset_id   INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    size       INTEGER NOT NULL,                    -- long edge in px
    path       TEXT,
    source     TEXT NOT NULL,                       -- sidecar_jpeg | embedded_preview | itself | render | none
    src_key    TEXT NOT NULL,                       -- '<file id>:<size>:<mtime_ns>' of the source file
    width      INTEGER,
    height     INTEGER,
    created_at TEXT NOT NULL,
    PRIMARY KEY (asset_id, size)
) WITHOUT ROWID;

-- Every preset a render was made with, by content hash (see film::Preset::hash).
CREATE TABLE presets (
    hash     TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    record   TEXT NOT NULL,                         -- manifest::PresetRecord JSON
    added_at TEXT NOT NULL
);

CREATE TABLE renders (
    id          INTEGER PRIMARY KEY,
    asset_id    INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,                      -- jpeg | exr
    root_id     INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    rel         TEXT NOT NULL,
    preset_name TEXT,
    preset_hash TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE (root_id, rel)
);
CREATE INDEX renders_asset ON renders(asset_id);
"#;

/// v2: per-photo RAW settings (`decode::RawSettings` JSON; NULL = the honest decode).
const V2: &str = r#"
ALTER TABLE assets ADD COLUMN raw_settings TEXT;
"#;

/// v3: the export queue — photos lined up for one export run.
const V3: &str = r#"
CREATE TABLE export_queue (
    asset_id INTEGER PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL
);
"#;

/// v4: the pick / reject flag, alongside the star (heart) rating.
const V4: &str = r#"
ALTER TABLE assets ADD COLUMN flag TEXT;   -- 'select' | 'reject' | NULL
CREATE INDEX assets_flag ON assets(flag);
"#;

/// v5: catalog-wide preferences, e.g. whether thumbnails come from prints.
const V5: &str = r#"
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// v6: dynamic catalogs — a saved filter that re-runs every time it is opened.
const V6: &str = r#"
CREATE TABLE collections (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    filter     TEXT NOT NULL,          -- query::Filter as JSON
    sort       TEXT,                   -- query::SortKey, NULL = the library's current sort
    created_at TEXT NOT NULL
);
"#;

/// Every migration, in order. `user_version` = how many have been applied.
pub(crate) const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6];

pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn migrate(conn: &mut Connection) -> anyhow::Result<u32> {
    apply(conn, MIGRATIONS)
}

pub(crate) fn user_version(conn: &Connection) -> anyhow::Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as u32)
}

/// Apply the migrations `conn` has not seen yet. Refuses a database from a newer build.
pub(crate) fn apply(conn: &mut Connection, migrations: &[&str]) -> anyhow::Result<u32> {
    let current = user_version(conn)?;
    if current as usize > migrations.len() {
        anyhow::bail!(
            "catalog schema v{current} is newer than this build understands (v{}); update the app",
            migrations.len()
        );
    }
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    user_version(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn fresh_database_reaches_latest_and_reopen_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.sqlite");
        {
            let cat = crate::catalog::Catalog::open(&path).unwrap();
            assert_eq!(user_version(cat.conn()).unwrap(), SCHEMA_VERSION);
            let t = tables(cat.conn());
            for want in ["roots", "imports", "files", "assets", "asset_files", "keywords", "asset_keywords", "thumbnails", "presets", "renders"] {
                assert!(t.iter().any(|n| n == want), "missing table {want}: {t:?}");
            }
            let mode: String = cat.conn().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
            assert_eq!(mode, "wal");
            cat.conn().execute("INSERT INTO roots (path, kind, label, added_at) VALUES ('/x', 'folder', 'x', 'now')", []).unwrap();
        }
        // Reopening applies nothing and keeps the data.
        let cat = crate::catalog::Catalog::open(&path).unwrap();
        assert_eq!(user_version(cat.conn()).unwrap(), SCHEMA_VERSION);
        let n: i64 = cat.conn().query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn migrations_apply_stepwise_and_keep_data() {
        let mut conn = Connection::open_in_memory().unwrap();
        let steps = ["CREATE TABLE t (a INTEGER);", "ALTER TABLE t ADD COLUMN b TEXT DEFAULT 'x';"];
        assert_eq!(apply(&mut conn, &steps[..1]).unwrap(), 1);
        conn.execute("INSERT INTO t (a) VALUES (7)", []).unwrap();
        assert_eq!(apply(&mut conn, &steps).unwrap(), 2);
        let (a, b): (i64, String) = conn.query_row("SELECT a, b FROM t", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((a, b.as_str()), (7, "x"));
        // Running again is a no-op.
        assert_eq!(apply(&mut conn, &steps).unwrap(), 2);
    }

    #[test]
    fn failed_migration_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        let steps = ["CREATE TABLE t (a INTEGER);", "CREATE TABLE u (a); CREATE TABLE t (oops);"];
        assert!(apply(&mut conn, &steps).is_err());
        assert_eq!(user_version(&conn).unwrap(), 1);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'u'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "partial migration must not leave tables behind");
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        let err = apply(&mut conn, MIGRATIONS).unwrap_err().to_string();
        assert!(err.contains("newer"), "{err}");
    }
}
