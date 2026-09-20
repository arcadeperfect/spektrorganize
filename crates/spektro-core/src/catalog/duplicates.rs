//! Find byte-identical files in the catalog, and purge the copies you do not want.
//!
//! Only exact duplicates are reported: same size, same BLAKE3. That is the one definition where
//! deleting a file loses nothing, which matters because this is the only part of the app that
//! touches a photo on disk. Files of the same scene in different formats (a RAW and its camera
//! JPEG) are not duplicates and never appear here.
//!
//! Hashes are computed only for files whose size collides with another file's, so a library of
//! unique photos costs one query and no reading.
//!
//! Purging keeps one file per group and puts the rest in the trash (or deletes them outright when
//! the caller insists). Before a copy goes, whatever the catalog knows about it — keywords,
//! rating, flag, prints — is merged onto the asset that is being kept, so choosing which copy
//! survives never costs you metadata.

use super::purge::{PurgeReport, Removal, remove_file};
use super::{Catalog, now};
use rusqlite::params;
use serde::Serialize;
use std::path::PathBuf;

/// One file in a duplicate group.
#[derive(Debug, Clone, Serialize)]
pub struct DupFile {
    pub file_id: i64,
    pub asset_id: Option<i64>,
    pub root_id: i64,
    pub root: String,
    pub rel: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    /// Role in its asset: a `raw` primary, its `jpeg`, a sidecar…
    pub role: Option<String>,
    pub indexed_at: String,
    /// What would be merged onto the keeper if this copy went.
    pub rating: i64,
    pub flag: String,
    pub keywords: i64,
    pub renders: i64,
    /// The file is on an online root and still there.
    pub present: bool,
}

/// Files that are byte-for-byte the same.
#[derive(Debug, Clone, Serialize)]
pub struct DupGroup {
    pub blake3: String,
    pub size: i64,
    pub files: Vec<DupFile>,
    /// Bytes that would come back if every copy but one went.
    pub wasted: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DupScan {
    pub groups: Vec<DupGroup>,
    /// Files read to settle a size collision.
    pub hashed: usize,
    pub wasted: i64,
}

/// Which folders to look in.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct DupScope {
    /// Roots to search; empty means the whole catalog.
    #[serde(default)]
    pub roots: Vec<i64>,
    /// With `roots` set: also read files outside them, and report a group when any of its copies
    /// is inside. Answers "is this folder's content already somewhere else?".
    #[serde(default)]
    pub whole_library: bool,
}

impl Catalog {
    /// Every group of byte-identical files. `progress(done, total)` reports hashing, which is the
    /// only slow part; files on offline roots, and files recorded as missing, are left out.
    pub fn find_duplicates(&self, scope: &DupScope, mut progress: impl FnMut(usize, usize)) -> anyhow::Result<DupScan> {
        // Scoped to some folders, and not comparing against the rest of the library, means both
        // the candidates and the "which sizes repeat" question stay inside those folders.
        let narrow = !scope.roots.is_empty() && !scope.whole_library;
        let in_roots = |alias: &str| format!(" AND {alias}.root_id IN ({})", vec!["?"; scope.roots.len()].join(", "));
        let sql = format!(
            "SELECT f.id, f.size, f.blake3, r.path, f.rel, f.root_id FROM files f JOIN roots r ON r.id = f.root_id
             WHERE f.missing = 0 AND r.online = 1 AND f.size > 0{}
               AND f.size IN (SELECT size FROM files WHERE missing = 0 AND size > 0{} GROUP BY size HAVING COUNT(*) > 1)",
            if narrow { in_roots("f") } else { String::new() },
            if narrow { in_roots("files") } else { String::new() },
        );
        let mut params: Vec<rusqlite::types::Value> = Vec::new();
        if narrow {
            for _ in 0..2 {
                params.extend(scope.roots.iter().map(|r| rusqlite::types::Value::Integer(*r)));
            }
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let candidates: Vec<(i64, i64, Option<String>, PathBuf, i64)> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                let root: String = r.get(3)?;
                let rel: String = r.get(4)?;
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, std::path::Path::new(&root).join(rel), r.get(5)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);

        let todo: Vec<(i64, PathBuf)> =
            candidates.iter().filter(|(_, _, h, _, _)| h.is_none()).map(|(id, _, _, p, _)| (*id, p.clone())).collect();
        let total = todo.len();
        let mut hashes: std::collections::HashMap<i64, String> = candidates
            .iter()
            .filter_map(|(id, _, h, _, _)| h.as_ref().map(|h| (*id, h.clone())))
            .collect();

        // Hash what we have to, a few at a time so progress is honest and the
        // caller can watch it happen.
        use rayon::prelude::*;
        let mut done = 0usize;
        for chunk in todo.chunks(32) {
            let got: Vec<(i64, String)> =
                chunk.par_iter().filter_map(|(id, p)| crate::copy::hash_file(p).ok().map(|h| (*id, h))).collect();
            {
                let tx = self.conn.unchecked_transaction()?;
                for (id, h) in &got {
                    tx.execute("UPDATE files SET blake3 = ?2 WHERE id = ?1", params![id, h])?;
                }
                tx.commit()?;
            }
            hashes.extend(got);
            done += chunk.len();
            progress(done, total);
        }

        // Group by (size, hash).
        let mut by_hash: std::collections::HashMap<(i64, String), Vec<i64>> = std::collections::HashMap::new();
        for (id, size, _, _, _) in &candidates {
            if let Some(h) = hashes.get(id) {
                by_hash.entry((*size, h.clone())).or_default().push(*id);
            }
        }

        // With `whole_library`, a group only matters when one of its copies is in the chosen
        // folders — the rest of the library is there for comparison, not for purging.
        let wanted: std::collections::HashSet<i64> = scope.roots.iter().copied().collect();
        let root_of: std::collections::HashMap<i64, i64> = candidates.iter().map(|(id, _, _, _, root)| (*id, *root)).collect();

        let mut groups = Vec::new();
        for ((size, blake3), ids) in by_hash {
            if ids.len() < 2 {
                continue;
            }
            if !wanted.is_empty() && !ids.iter().any(|id| root_of.get(id).is_some_and(|r| wanted.contains(r))) {
                continue;
            }
            let mut files = Vec::with_capacity(ids.len());
            for id in ids {
                files.push(self.dup_file(id)?);
            }
            // Oldest first: the copy that has been in the catalog longest is
            // usually the one to keep, and it is offered as the default.
            files.sort_by(|a, b| a.indexed_at.cmp(&b.indexed_at).then(a.path.cmp(&b.path)));
            let wasted = size * (files.len() as i64 - 1);
            groups.push(DupGroup { blake3, size, files, wasted });
        }
        groups.sort_by(|a, b| b.wasted.cmp(&a.wasted));
        let wasted = groups.iter().map(|g| g.wasted).sum();
        Ok(DupScan { groups, hashed: total, wasted })
    }

    fn dup_file(&self, file_id: i64) -> anyhow::Result<DupFile> {
        let f = self.conn.query_row(
            "SELECT f.id, f.root_id, r.path, f.rel, f.name, f.kind, f.indexed_at, af.asset_id, af.role
             FROM files f JOIN roots r ON r.id = f.root_id LEFT JOIN asset_files af ON af.file_id = f.id
             WHERE f.id = ?1",
            [file_id],
            |r| {
                let root: String = r.get(2)?;
                let rel: String = r.get(3)?;
                Ok(DupFile {
                    file_id: r.get(0)?,
                    root_id: r.get(1)?,
                    path: std::path::Path::new(&root).join(&rel).to_string_lossy().to_string(),
                    root,
                    rel,
                    name: r.get(4)?,
                    kind: r.get(5)?,
                    indexed_at: r.get(6)?,
                    asset_id: r.get(7)?,
                    role: r.get(8)?,
                    rating: 0,
                    flag: String::new(),
                    keywords: 0,
                    renders: 0,
                    present: false,
                })
            },
        )?;
        let mut f = f;
        f.present = std::path::Path::new(&f.path).exists();
        if let Some(asset) = f.asset_id {
            let (rating, flag): (i64, String) = self.conn.query_row(
                "SELECT rating, COALESCE(flag, '') FROM assets WHERE id = ?1",
                [asset],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            f.rating = rating;
            f.flag = flag;
            f.keywords = self.conn.query_row("SELECT COUNT(*) FROM asset_keywords WHERE asset_id = ?1", [asset], |r| r.get(0))?;
            f.renders = self.conn.query_row("SELECT COUNT(*) FROM renders WHERE asset_id = ?1", [asset], |r| r.get(0))?;
        }
        Ok(f)
    }

    /// Keep `keep`, remove `drop_ids`. Every id must be in the same duplicate group as `keep`,
    /// which must still be on disk — so this can never take the last copy of a photo.
    pub fn purge_duplicates(&self, keep: i64, drop_ids: &[i64], how: Removal) -> anyhow::Result<PurgeReport> {
        let keeper = self.dup_file(keep)?;
        anyhow::ensure!(keeper.present, "the copy you chose to keep is not on disk: {}", keeper.path);
        let (keep_hash, keep_size): (Option<String>, i64) =
            self.conn.query_row("SELECT blake3, size FROM files WHERE id = ?1", [keep], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let keep_hash = keep_hash.ok_or_else(|| anyhow::anyhow!("the file to keep has not been hashed; run the scan again"))?;

        let mut report = PurgeReport::default();
        for id in drop_ids {
            if *id == keep {
                continue;
            }
            let victim = match self.dup_file(*id) {
                Ok(v) => v,
                Err(e) => {
                    report.failed.push((id.to_string(), e.to_string()));
                    continue;
                }
            };
            let (hash, size): (Option<String>, i64) =
                self.conn.query_row("SELECT blake3, size FROM files WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if hash.as_deref() != Some(keep_hash.as_str()) || size != keep_size {
                report.failed.push((victim.path.clone(), "not a copy of the file being kept".into()));
                continue;
            }
            if victim.path == keeper.path {
                continue;
            }
            if let Err(e) = remove_file(&victim.path, how) {
                report.failed.push((victim.path.clone(), e.to_string()));
                continue;
            }
            self.merge_and_forget(&victim, &keeper)?;
            report.removed += 1;
            report.bytes += size;
        }
        // Assets whose every file has gone are no longer about anything.
        report.assets_removed = self
            .conn
            .execute("DELETE FROM assets WHERE NOT EXISTS (SELECT 1 FROM asset_files af WHERE af.asset_id = assets.id)", [])?;
        Ok(report)
    }

    /// Move what the catalog knew about the removed copy onto the keeper, then drop its row.
    fn merge_and_forget(&self, victim: &DupFile, keeper: &DupFile) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        if let (Some(from), Some(to)) = (victim.asset_id, keeper.asset_id)
            && from != to
        {
            // Keywords: a union, keeping the source that is already recorded.
            tx.execute(
                "INSERT OR IGNORE INTO asset_keywords (asset_id, keyword_id, source, confidence, added_at)
                 SELECT ?2, keyword_id, source, confidence, added_at FROM asset_keywords WHERE asset_id = ?1",
                params![from, to],
            )?;
            // The higher rating and any flag the keeper does not have.
            tx.execute(
                "UPDATE assets SET rating = MAX(rating, (SELECT rating FROM assets WHERE id = ?1)),
                                   flag = COALESCE(flag, (SELECT flag FROM assets WHERE id = ?1))
                 WHERE id = ?2",
                params![from, to],
            )?;
            // Prints of the copy are prints of this photo.
            tx.execute("UPDATE renders SET asset_id = ?2 WHERE asset_id = ?1", params![from, to])?;
        }
        tx.execute("DELETE FROM asset_files WHERE file_id = ?1", [victim.file_id])?;
        tx.execute("UPDATE assets SET primary_file_id = NULL WHERE primary_file_id = ?1", [victim.file_id])?;
        tx.execute("DELETE FROM files WHERE id = ?1", [victim.file_id])?;
        tx.execute("UPDATE roots SET last_indexed_at = ?2 WHERE id = ?1", params![victim.root_id, now()])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    /// Two roots, each with the same bytes, plus a file that is merely the same size.
    fn setup(dir: &std::path::Path) -> (Catalog, i64, i64, i64) {
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("one.jpg"), b"same bytes here!").unwrap();
        std::fs::write(b.join("one copy.jpg"), b"same bytes here!").unwrap();
        std::fs::write(b.join("other.jpg"), b"other bytes ....").unwrap(); // same length

        let cat = Catalog::open_in_memory().unwrap();
        let c = cat.conn();
        let root = |p: &std::path::Path| -> i64 {
            c.execute("INSERT INTO roots (path, label, kind, online, added_at) VALUES (?1, 'r', 'folder', 1, 'now')", [p.to_string_lossy()])
                .unwrap();
            c.last_insert_rowid()
        };
        let (ra, rb) = (root(&a), root(&b));
        let file = |root_id: i64, rel: &str, size: i64| -> i64 {
            c.execute(
                "INSERT INTO files (root_id, rel, name, kind, size, missing, indexed_at) VALUES (?1, ?2, ?2, 'image', ?3, 0, 'now')",
                params![root_id, rel, size],
            )
            .unwrap();
            let fid = c.last_insert_rowid();
            c.execute("INSERT INTO assets (kind, added_at) VALUES ('image', 'now')", []).unwrap();
            let aid = c.last_insert_rowid();
            c.execute("INSERT INTO asset_files (file_id, asset_id, role) VALUES (?1, ?2, 'image')", params![fid, aid]).unwrap();
            c.execute("UPDATE assets SET primary_file_id = ?1 WHERE id = ?2", params![fid, aid]).unwrap();
            fid
        };
        let f1 = file(ra, "one.jpg", 16);
        let f2 = file(rb, "one copy.jpg", 16);
        let f3 = file(rb, "other.jpg", 16);
        (cat, f1, f2, f3)
    }

    #[test]
    fn finds_only_byte_identical_files() {
        let dir = tempfile::tempdir().unwrap();
        let (cat, f1, f2, f3) = setup(dir.path());
        let scan = cat.find_duplicates(&DupScope::default(), |_, _| {}).unwrap();
        assert_eq!(scan.groups.len(), 1, "the same-size-but-different file is not a duplicate");
        let ids: Vec<i64> = scan.groups[0].files.iter().map(|f| f.file_id).collect();
        assert!(ids.contains(&f1) && ids.contains(&f2) && !ids.contains(&f3));
        assert_eq!(scan.groups[0].wasted, 16);
        assert_eq!(scan.hashed, 3, "all three collided on size, so all three were read");
    }

    #[test]
    fn purge_keeps_one_copy_and_moves_the_metadata_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let (cat, f1, f2, _) = setup(dir.path());
        cat.find_duplicates(&DupScope::default(), |_, _| {}).unwrap();
        let keeper_asset: i64 = cat.conn().query_row("SELECT asset_id FROM asset_files WHERE file_id = ?1", [f1], |r| r.get(0)).unwrap();
        let victim_asset: i64 = cat.conn().query_row("SELECT asset_id FROM asset_files WHERE file_id = ?1", [f2], |r| r.get(0)).unwrap();
        cat.set_rating(&[victim_asset], 4).unwrap();
        crate::catalog::keywords::add(cat.conn(), &[victim_asset], &["holiday".to_string()], "user").unwrap();

        let victim_path: String = {
            let f = cat.dup_file(f2).unwrap();
            f.path
        };
        let report = cat.purge_duplicates(f1, &[f2], Removal::Delete).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.bytes, 16);
        assert!(report.failed.is_empty());
        assert!(!std::path::Path::new(&victim_path).exists(), "the file is gone from disk");
        assert!(std::path::Path::new(&cat.dup_file(f1).unwrap().path).exists(), "the kept copy is untouched");

        // The rating and keyword moved to the surviving photo.
        let rating: i64 = cat.conn().query_row("SELECT rating FROM assets WHERE id = ?1", [keeper_asset], |r| r.get(0)).unwrap();
        assert_eq!(rating, 4);
        let kw: i64 =
            cat.conn().query_row("SELECT COUNT(*) FROM asset_keywords WHERE asset_id = ?1", [keeper_asset], |r| r.get(0)).unwrap();
        assert_eq!(kw, 1);
        assert_eq!(report.assets_removed, 1, "the emptied photo left the catalog");
    }

    #[test]
    fn scope_limits_the_search_to_the_chosen_folders() {
        let dir = tempfile::tempdir().unwrap();
        let (cat, f1, f2, _) = setup(dir.path());
        let root_a: i64 = cat.conn().query_row("SELECT root_id FROM files WHERE id = ?1", [f1], |r| r.get(0)).unwrap();
        let root_b: i64 = cat.conn().query_row("SELECT root_id FROM files WHERE id = ?1", [f2], |r| r.get(0)).unwrap();

        // Inside one folder alone there is nothing to find: the copies are in two.
        let only_a = DupScope { roots: vec![root_a], whole_library: false };
        assert!(cat.find_duplicates(&only_a, |_, _| {}).unwrap().groups.is_empty());

        // Comparing that folder against the rest of the library finds the pair.
        let a_vs_all = DupScope { roots: vec![root_a], whole_library: true };
        let scan = cat.find_duplicates(&a_vs_all, |_, _| {}).unwrap();
        assert_eq!(scan.groups.len(), 1);
        let ids: Vec<i64> = scan.groups[0].files.iter().map(|f| f.file_id).collect();
        assert!(ids.contains(&f1) && ids.contains(&f2));

        // Both folders in scope: found without needing the wider comparison.
        let both = DupScope { roots: vec![root_a, root_b], whole_library: false };
        assert_eq!(cat.find_duplicates(&both, |_, _| {}).unwrap().groups.len(), 1);
    }

    #[test]
    fn refuses_to_remove_a_file_that_is_not_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (cat, f1, _, f3) = setup(dir.path());
        cat.find_duplicates(&DupScope::default(), |_, _| {}).unwrap();
        let other = cat.dup_file(f3).unwrap().path;
        let report = cat.purge_duplicates(f1, &[f3], Removal::Delete).unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(report.failed.len(), 1);
        assert!(std::path::Path::new(&other).exists(), "a file that only shares a size is never touched");
    }
}
