//! Duplicate detection for an import, in two directions:
//!
//! - **Within the scan.** A folder whose photos were copied into sub-folders scans as two of
//!   everything. The copies collapse to one: the first by path is imported, the rest are marked
//!   as its copies and excluded.
//! - **Against the catalog.** A photo whose bytes are already in the library is marked, so
//!   re-importing the same card costs nothing.
//!
//! Only files whose *name and size* already match something are read, so a card of new photos is
//! checked for the price of one query. That is enough for the cases that actually happen — the
//! same file copied twice, or imported twice — and never hashes a card needlessly.

use super::Catalog;
use crate::scan::{GroupId, Scan};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// What a scan's group turned out to be.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportDupes {
    /// Group -> the group in this same scan it is a copy of (the one that will be imported).
    pub copies: HashMap<u32, u32>,
    /// Groups whose bytes are already in the catalog, with where they are.
    pub known: HashMap<u32, String>,
    /// Files read to settle a name-and-size match.
    pub hashed: usize,
}

impl ImportDupes {
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty() && self.known.is_empty()
    }

    /// Groups to leave out of the import: every copy, and everything already held.
    pub fn to_exclude(&self) -> Vec<u32> {
        self.copies.keys().chain(self.known.keys()).copied().collect()
    }
}

/// Look for copies within `scan`, and for photos the catalog already holds.
///
/// `progress(done, total)` reports hashing. The catalog is optional: without one, only the
/// within-scan pass runs.
pub fn find(scan: &Scan, catalog: Option<&Catalog>, mut progress: impl FnMut(usize, usize)) -> anyhow::Result<ImportDupes> {
    // The primary file of each group — the photo itself, not its sidecars.
    let primaries: Vec<(GroupId, PathBuf, u64, String)> = scan
        .groups
        .iter()
        .map(|g| {
            let f = scan.file(g.primary);
            let name = f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (g.id, f.path.clone(), f.size, name)
        })
        .collect();

    // Who shares a name and size with something else here, or with a file in the catalog?
    let mut by_name_size: HashMap<(&str, u64), Vec<usize>> = HashMap::new();
    for (i, (_, _, size, name)) in primaries.iter().enumerate() {
        by_name_size.entry((name.as_str(), *size)).or_default().push(i);
    }
    let mut suspect: Vec<usize> = by_name_size.values().filter(|v| v.len() > 1).flatten().copied().collect();

    // Catalog files with the same name and size, by (name, size).
    let mut known_by_hash: HashMap<String, String> = HashMap::new();
    if let Some(cat) = catalog {
        let mut stmt = cat.conn().prepare(
            "SELECT f.name, f.size, f.blake3, r.path, f.rel FROM files f JOIN roots r ON r.id = f.root_id WHERE f.missing = 0",
        )?;
        let rows: Vec<(String, i64, Option<String>, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        let mut cat_by_name_size: HashMap<(String, i64), Vec<(Option<String>, PathBuf)>> = HashMap::new();
        for (name, size, hash, root, rel) in rows {
            cat_by_name_size.entry((name, size)).or_default().push((hash, std::path::Path::new(&root).join(rel)));
        }
        for (i, (_, _, size, name)) in primaries.iter().enumerate() {
            let Some(matches) = cat_by_name_size.get(&(name.clone(), *size as i64)) else { continue };
            suspect.push(i);
            // Hash the catalog side too, when it was never hashed (a folder indexed in place).
            for (hash, path) in matches {
                let h = match hash {
                    Some(h) => h.clone(),
                    None => match crate::copy::hash_file(path) {
                        Ok(h) => {
                            let _ = cat.conn().execute(
                                "UPDATE files SET blake3 = ?2 WHERE rel = ?1 AND blake3 IS NULL",
                                rusqlite::params![
                                    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                                    h
                                ],
                            );
                            h
                        }
                        Err(_) => continue,
                    },
                };
                known_by_hash.entry(h).or_insert_with(|| path.to_string_lossy().to_string());
            }
        }
    }

    suspect.sort_unstable();
    suspect.dedup();
    let total = suspect.len();
    let mut out = ImportDupes { hashed: total, ..Default::default() };
    if total == 0 {
        return Ok(out);
    }

    // Hash the suspects on the card.
    use rayon::prelude::*;
    let mut hashes: HashMap<usize, String> = HashMap::new();
    let mut done = 0usize;
    for chunk in suspect.chunks(16) {
        let got: Vec<(usize, String)> =
            chunk.par_iter().filter_map(|i| crate::copy::hash_file(&primaries[*i].1).ok().map(|h| (*i, h))).collect();
        hashes.extend(got);
        done += chunk.len();
        progress(done, total);
    }

    // Within the scan: the first by path wins, the rest are its copies.
    let mut first_for_hash: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<usize> = hashes.keys().copied().collect();
    order.sort_by(|a, b| primaries[*a].1.cmp(&primaries[*b].1));
    for i in order {
        let h = hashes[&i].as_str();
        match first_for_hash.get(h) {
            Some(first) => {
                out.copies.insert(primaries[i].0.0, primaries[*first].0.0);
            }
            None => {
                first_for_hash.insert(h, i);
            }
        }
        if let Some(where_) = known_by_hash.get(h) {
            out.known.insert(primaries[i].0.0, where_.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::CaptureMeta;

    fn scan_dir(root: &std::path::Path) -> Scan {
        crate::scan::scan(root, &|p| Ok(CaptureMeta::from_mtime(p)), |_| {}).unwrap()
    }

    #[test]
    fn copies_inside_the_scan_collapse_to_one() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("all");
        let b = dir.path().join("all/backup");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("DSCF0001.JPG"), b"the same photo..").unwrap();
        std::fs::write(b.join("DSCF0001.JPG"), b"the same photo..").unwrap();
        std::fs::write(a.join("DSCF0002.JPG"), b"a different one!").unwrap();

        let scan = scan_dir(dir.path());
        let dupes = find(&scan, None, |_, _| {}).unwrap();
        assert_eq!(dupes.copies.len(), 1, "one of the two copies is marked");
        assert_eq!(dupes.to_exclude().len(), 1);
        // The one that is kept is the one nearer the top of the tree. (The scan
        // canonicalises its root, so compare against canonical paths.)
        let kept = *dupes.copies.values().next().unwrap();
        let kept_path = scan.groups.iter().find(|g| g.id.0 == kept).map(|g| scan.file(g.primary).path.clone()).unwrap();
        let backup = b.canonicalize().unwrap();
        assert!(!kept_path.starts_with(&backup), "kept {kept_path:?}, which is inside the backup folder");
        assert_eq!(dupes.hashed, 2, "only the pair that shares a name and size is read");
    }

    #[test]
    fn photos_already_in_the_catalog_are_marked() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("card");
        let archive = dir.path().join("archive");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(card.join("DSCF0003.JPG"), b"already imported").unwrap();
        std::fs::write(archive.join("DSCF0003.JPG"), b"already imported").unwrap();

        let cat = Catalog::open_in_memory().unwrap();
        cat.conn()
            .execute("INSERT INTO roots (path, label, kind, online, added_at) VALUES (?1, 'a', 'folder', 1, 'now')", [archive.to_string_lossy()])
            .unwrap();
        let root = cat.conn().last_insert_rowid();
        cat.conn()
            .execute(
                "INSERT INTO files (root_id, rel, name, kind, size, missing, indexed_at) VALUES (?1, 'DSCF0003.JPG', 'DSCF0003.JPG', 'image', 16, 0, 'now')",
                [root],
            )
            .unwrap();

        let scan = scan_dir(&card);
        let dupes = find(&scan, Some(&cat), |_, _| {}).unwrap();
        assert_eq!(dupes.known.len(), 1, "the catalog already holds these bytes");
        assert!(dupes.known.values().next().unwrap().ends_with("archive/DSCF0003.JPG"));
    }

    #[test]
    fn a_card_of_new_photos_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"one").unwrap();
        std::fs::write(dir.path().join("b.jpg"), b"twotwo").unwrap();
        let scan = scan_dir(dir.path());
        let dupes = find(&scan, None, |_, _| {}).unwrap();
        assert!(dupes.is_empty());
        assert_eq!(dupes.hashed, 0, "nothing shares a name and size, so nothing is hashed");
    }
}
