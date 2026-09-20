//! Numbered image sequences in a scan, collapsed into one thing each.
//!
//! A folder of `render_0001.png … render_0480.png` is one shot, not 480 photos, and reviewing it
//! frame by frame is useless. This finds those runs (using the `sequitur` crate, which knows the
//! naming conventions) so the import screen can show one row per sequence. Nothing is skipped:
//! every frame is still copied — it is only the review that collapses.

use crate::scan::{Kind, Scan};
use serde::Serialize;
use std::path::PathBuf;

/// A run of numbered files that belong together.
#[derive(Debug, Clone, Serialize)]
pub struct SeqGroup {
    /// How the run reads, e.g. `render_####.png`.
    pub pattern: String,
    pub frames: usize,
    pub first: i64,
    pub last: i64,
    /// Numbers inside the range that are not on disk.
    pub missing: Vec<i64>,
    /// The scan groups this run covers, in frame order.
    pub members: Vec<u32>,
    pub bytes: u64,
}

/// Find the sequences among a scan's images. `min_frames` is how many numbered files it takes to
/// count as a sequence rather than a few photos that happen to be numbered — 4 or more is a
/// sensible floor for a camera roll, since `DSCF0001..0003` should stay three photos.
pub fn detect(scan: &Scan, min_frames: usize) -> Vec<SeqGroup> {
    // Only still images: RAWs off a camera are numbered too, and collapsing a day's shooting
    // into one row would be worse than useless.
    let candidates: Vec<(u32, PathBuf, u64)> = scan
        .groups
        .iter()
        .filter(|g| g.kind == Kind::Image)
        .map(|g| {
            let f = scan.file(g.primary);
            (g.id.0, f.path.clone(), scan.group_files(g).map(|f| f.size).sum())
        })
        .collect();
    if candidates.len() < min_frames {
        return Vec::new();
    }

    let paths: Vec<PathBuf> = candidates.iter().map(|(_, p, _)| p.clone()).collect();
    let parsed = sequitur::FileSequence::from_paths(&paths, min_frames.max(2));

    let mut out = Vec::new();
    for seq in parsed.sequences {
        let members: Vec<u32> = seq
            .items()
            .iter()
            .filter_map(|item| {
                let path = item.path();
                candidates.iter().find(|(_, p, _)| *p == path).map(|(id, _, _)| *id)
            })
            .collect();
        if members.len() < min_frames {
            continue;
        }
        let bytes = members
            .iter()
            .filter_map(|id| candidates.iter().find(|(gid, _, _)| gid == id).map(|(_, _, b)| *b))
            .sum();
        out.push(SeqGroup {
            pattern: pattern_of(&seq),
            frames: seq.len(),
            first: seq.first_frame() as i64,
            last: seq.last_frame() as i64,
            missing: seq.missing_frames().into_iter().map(|f| f as i64).collect(),
            members,
            bytes,
        });
    }
    out.sort_by(|a, b| b.frames.cmp(&a.frames));
    out
}

/// How a run reads at a glance: `render_####.png`.
fn pattern_of(seq: &sequitur::FileSequence) -> String {
    match (seq.prefix(), seq.extension()) {
        (Ok(p), Ok(e)) => format!("{p}####.{e}"),
        _ => seq.items().first().map(|i| i.filename()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::CaptureMeta;

    fn scan_of(dir: &std::path::Path) -> Scan {
        crate::scan::scan(dir, &|p| Ok(CaptureMeta::from_mtime(p)), |_| {}).unwrap()
    }

    #[test]
    fn a_numbered_run_collapses_and_loose_photos_do_not() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=12 {
            std::fs::write(dir.path().join(format!("render_{i:04}.png")), b"frame").unwrap();
        }
        // Three holiday snaps, numbered but not a sequence at this floor.
        for i in 1..=3 {
            std::fs::write(dir.path().join(format!("DSCF{i:04}.jpg")), b"photo").unwrap();
        }

        let scan = scan_of(dir.path());
        let seqs = detect(&scan, 4);
        assert_eq!(seqs.len(), 1, "one run, not two");
        let s = &seqs[0];
        assert_eq!(s.frames, 12);
        assert_eq!((s.first, s.last), (1, 12));
        assert_eq!(s.members.len(), 12);
        assert!(s.missing.is_empty());
        assert!(s.pattern.contains("render"), "{}", s.pattern);
    }

    #[test]
    fn gaps_in_a_run_are_reported_not_hidden() {
        let dir = tempfile::tempdir().unwrap();
        for i in (1..=10).filter(|i| *i != 4 && *i != 5) {
            std::fs::write(dir.path().join(format!("shot.{i:04}.png")), b"frame").unwrap();
        }
        let scan = scan_of(dir.path());
        let seqs = detect(&scan, 4);
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0].missing, vec![4, 5]);
        assert_eq!(seqs[0].frames, 8);
    }

    #[test]
    fn a_folder_of_plain_photos_has_no_sequences() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["one.jpg", "two.jpg", "three.jpg", "four.jpg"] {
            std::fs::write(dir.path().join(name), b"photo").unwrap();
        }
        let scan = scan_of(dir.path());
        assert!(detect(&scan, 4).is_empty());
    }
}
