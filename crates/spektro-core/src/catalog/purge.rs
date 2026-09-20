//! Removing photos from disk: the shared machinery behind purging duplicates and purging
//! rejects.
//!
//! Everything here moves files to the system trash by default, so a mistake is recoverable.
//! Deleting outright is possible but never the default, and both paths are driven from a screen
//! that lists every path first.

use super::Catalog;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Removal {
    /// Move to the system trash — recoverable, and the default.
    Trash,
    /// Delete outright. Only when the caller asks for it explicitly.
    Delete,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PurgeReport {
    pub removed: usize,
    pub bytes: i64,
    /// Files that could not be removed, with the reason.
    pub failed: Vec<(String, String)>,
    /// Photos that were left with no files and so were dropped from the catalog.
    pub assets_removed: usize,
}

/// One file of a photo, as offered for review before anything is deleted.
#[derive(Debug, Clone, Serialize)]
pub struct DoomedFile {
    pub file_id: i64,
    pub asset_id: i64,
    pub path: String,
    pub name: String,
    pub role: String,
    pub size: i64,
    pub present: bool,
}

/// Trash (preferred) or delete one file.
pub(crate) fn remove_file(path: &str, how: Removal) -> anyhow::Result<()> {
    match how {
        Removal::Trash => trash::delete(path).map_err(|e| anyhow::anyhow!("could not move to the trash: {e}")),
        Removal::Delete => std::fs::remove_file(path).map_err(|e| anyhow::anyhow!("{e}")),
    }
}

impl Catalog {
    /// Every file belonging to these photos — what a purge would remove, for the review screen.
    pub fn files_of_assets(&self, ids: &[i64]) -> anyhow::Result<Vec<DoomedFile>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(500) {
            let sql = format!(
                "SELECT f.id, af.asset_id, r.path, f.rel, f.name, af.role, f.size
                 FROM asset_files af JOIN files f ON f.id = af.file_id JOIN roots r ON r.id = f.root_id
                 WHERE af.asset_id IN ({}) ORDER BY af.asset_id, af.role",
                vec!["?"; chunk.len()].join(", ")
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                let root: String = r.get(2)?;
                let rel: String = r.get(3)?;
                let path = std::path::Path::new(&root).join(rel);
                Ok(DoomedFile {
                    file_id: r.get(0)?,
                    asset_id: r.get(1)?,
                    present: path.exists(),
                    path: path.to_string_lossy().to_string(),
                    name: r.get(4)?,
                    role: r.get(5)?,
                    size: r.get(6)?,
                })
            })?;
            for f in rows {
                out.push(f?);
            }
        }
        Ok(out)
    }

    /// Remove these photos: every file of each one goes, then the photo leaves the catalog.
    /// Prints made from them are left on disk — they are their own files, and are only unlinked.
    pub fn purge_assets(&self, ids: &[i64], how: Removal) -> anyhow::Result<PurgeReport> {
        let mut report = PurgeReport::default();
        for f in self.files_of_assets(ids)? {
            if !f.present {
                // Nothing on disk to remove; the row still goes.
                self.conn.execute("DELETE FROM files WHERE id = ?1", [f.file_id])?;
                continue;
            }
            if let Err(e) = remove_file(&f.path, how) {
                report.failed.push((f.path.clone(), e.to_string()));
                continue;
            }
            let tx = self.conn.unchecked_transaction()?;
            tx.execute("DELETE FROM asset_files WHERE file_id = ?1", [f.file_id])?;
            tx.execute("UPDATE assets SET primary_file_id = NULL WHERE primary_file_id = ?1", [f.file_id])?;
            tx.execute("DELETE FROM files WHERE id = ?1", [f.file_id])?;
            tx.commit()?;
            report.removed += 1;
            report.bytes += f.size;
        }
        // A photo with nothing left is not about anything.
        for chunk in ids.chunks(500) {
            let sql = format!(
                "DELETE FROM assets WHERE id IN ({}) AND NOT EXISTS (SELECT 1 FROM asset_files af WHERE af.asset_id = assets.id)",
                vec!["?"; chunk.len()].join(", ")
            );
            report.assets_removed += self.conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn cat_with_photo(dir: &std::path::Path, name: &str) -> (Catalog, i64, String) {
        let path = dir.join(name);
        std::fs::write(&path, b"a photo").unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let c = cat.conn();
        c.execute("INSERT INTO roots (path, label, kind, online, added_at) VALUES (?1, 'r', 'folder', 1, 'now')", [dir.to_string_lossy()])
            .unwrap();
        let root = c.last_insert_rowid();
        c.execute(
            "INSERT INTO files (root_id, rel, name, kind, size, missing, indexed_at) VALUES (?1, ?2, ?2, 'image', 7, 0, 'now')",
            params![root, name],
        )
        .unwrap();
        let fid = c.last_insert_rowid();
        c.execute("INSERT INTO assets (kind, added_at, flag) VALUES ('image', 'now', 'reject')", []).unwrap();
        let aid = c.last_insert_rowid();
        c.execute("INSERT INTO asset_files (file_id, asset_id, role) VALUES (?1, ?2, 'image')", params![fid, aid]).unwrap();
        c.execute("UPDATE assets SET primary_file_id = ?1 WHERE id = ?2", params![fid, aid]).unwrap();
        (cat, aid, path.to_string_lossy().to_string())
    }

    #[test]
    fn lists_then_removes_every_file_of_a_photo() {
        let dir = tempfile::tempdir().unwrap();
        let (cat, asset, path) = cat_with_photo(dir.path(), "DSCF1.JPG");

        let doomed = cat.files_of_assets(&[asset]).unwrap();
        assert_eq!(doomed.len(), 1, "the review screen sees the file first");
        assert_eq!(doomed[0].path, path);
        assert!(doomed[0].present);
        assert!(std::path::Path::new(&path).exists(), "listing removes nothing");

        let report = cat.purge_assets(&[asset], Removal::Delete).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.bytes, 7);
        assert_eq!(report.assets_removed, 1);
        assert!(!std::path::Path::new(&path).exists());
        let left: i64 = cat.conn().query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }
}
