//! Turn a scan plus config into concrete destination paths, and summarise them as a tree.

use crate::config::Config;
use crate::scan::{FileId, GroupId, Kind, Scan, ScannedFile};
use crate::template::{Context, Template};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Root {
    Archive,
    Video,
    Other,
    Render,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum DestStatus {
    New,
    /// Destination exists with the same size; will be skipped when `skip_existing` is on.
    ExistsSameSize,
    /// Destination exists with a different size; will be written under a suffixed name.
    ExistsDifferent,
    /// Another source file in this import mapped to the same path; suffixed.
    Collision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFile {
    pub file: FileId,
    pub group: GroupId,
    pub root: Root,
    /// Path relative to `root`.
    pub rel: PathBuf,
    /// Absolute destination.
    pub dest: PathBuf,
    pub size: u64,
    pub status: DestStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedRender {
    pub group: GroupId,
    pub raw: FileId,
    pub exr: Option<PathBuf>,
    pub jpeg: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub imported_at: NaiveDateTime,
    pub included: Vec<GroupId>,
    pub files: Vec<PlannedFile>,
    pub renders: Vec<PlannedRender>,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn bytes_to_copy(&self, skip_existing: bool) -> u64 {
        self.files
            .iter()
            .filter(|f| !(skip_existing && f.status == DestStatus::ExistsSameSize))
            .map(|f| f.size)
            .sum()
    }
}

fn context(scan: &Scan, group: &crate::scan::Group, file: &ScannedFile, seq: u32, imported_at: NaiveDateTime) -> Context {
    Context {
        captured: group.meta.captured_at,
        imported: imported_at,
        camera: group.meta.camera.clone(),
        make: group.meta.make.clone(),
        model: group.meta.model.clone(),
        stem: file.stem(),
        ext: file.ext(),
        kind: file.kind.as_str(),
        volume: scan.source.label.clone(),
        rel_dir: file.rel_dir(),
        seq,
        iso: group.meta.iso,
        lens: group.meta.lens.clone(),
        preset: None,
    }
}

fn root_path<'a>(cfg: &'a Config, root: Root) -> &'a Path {
    match root {
        Root::Archive => &cfg.archive_root,
        Root::Video => cfg.video_root(),
        Root::Other => cfg.other_root(),
        Root::Render => &cfg.render_root,
    }
}

/// Build the plan for the included groups, in scan (capture) order.
pub fn plan(scan: &Scan, included: &[GroupId], cfg: &Config, imported_at: NaiveDateTime) -> anyhow::Result<Plan> {
    let mut files: Vec<PlannedFile> = Vec::new();
    let mut renders = Vec::new();
    let mut seq: u32 = 0;

    let mut ordered: Vec<GroupId> = included.to_vec();
    ordered.sort();
    ordered.dedup();

    for gid in &ordered {
        let group = scan.group(*gid);
        seq += 1;
        let primary = scan.file(group.primary);

        // Primary destination.
        let (root, template): (Root, &Template) = match group.kind {
            Kind::Raw => (Root::Archive, &cfg.templates.raw),
            Kind::Image => (Root::Archive, &cfg.templates.image),
            Kind::Video => (Root::Video, &cfg.templates.video),
            Kind::Sidecar | Kind::Other => (Root::Other, &cfg.templates.other),
        };
        let rel = PathBuf::from(template.render(&context(scan, group, primary, seq, imported_at))?);
        let primary_dir = rel.parent().map(Path::to_path_buf).unwrap_or_default();
        files.push(planned(cfg, root, rel, primary, *gid));

        // Attachments.
        for aid in &group.attachments {
            let f = scan.file(*aid);
            let (aroot, arel) = match (group.kind, f.kind) {
                (Kind::Raw, Kind::Image) => {
                    (Root::Archive, PathBuf::from(cfg.templates.paired_image.render(&context(scan, group, f, seq, imported_at))?))
                }
                // Sidecars and anything else attached to a primary sit next to it.
                _ => (root, primary_dir.join(f.path.file_name().unwrap_or_default())),
            };
            files.push(planned(cfg, aroot, arel, f, *gid));
        }

        if group.kind == Kind::Raw && (cfg.outputs.exr || cfg.outputs.jpeg) {
            let ctx = context(scan, group, primary, seq, imported_at);
            let exr = cfg.outputs.exr.then(|| cfg.templates.render_exr.render(&ctx)).transpose()?.map(|r| cfg.render_root.join(r));
            let jpeg = cfg.outputs.jpeg.then(|| cfg.templates.render_jpeg.render(&ctx)).transpose()?.map(|r| cfg.render_root.join(r));
            renders.push(PlannedRender { group: *gid, raw: group.primary, exr, jpeg });
        }
    }

    resolve_collisions(&mut files);
    Ok(Plan { imported_at, included: ordered, files, renders })
}

fn planned(cfg: &Config, root: Root, rel: PathBuf, f: &ScannedFile, group: GroupId) -> PlannedFile {
    let dest = root_path(cfg, root).join(&rel);
    PlannedFile { file: f.id, group, root, rel, dest, size: f.size, status: DestStatus::New }
}

/// Assign final destinations: a path already claimed by another planned file, or occupied on
/// disk by a file of a different size, gets a numeric suffix. A file of the same size on disk is
/// reported as such so the copy can be skipped.
fn resolve_collisions(files: &mut [PlannedFile]) {
    let mut taken: HashSet<PathBuf> = HashSet::new();
    for f in files.iter_mut() {
        let mut n = 0;
        let mut candidate = f.dest.clone();
        let mut collided = false;
        let mut differs_on_disk = false;
        let same_size = loop {
            let by_plan = taken.contains(&candidate);
            let on_disk = std::fs::metadata(&candidate).ok().map(|md| md.len() == f.size);
            match (by_plan, on_disk) {
                (false, None) => break false,
                (false, Some(true)) => break true,
                (false, Some(false)) => differs_on_disk = true,
                (true, _) => collided = true,
            }
            n += 1;
            candidate = suffixed(&f.dest, n);
        };
        if n > 0 {
            f.rel = suffixed(&f.rel, n);
            f.dest = candidate.clone();
        }
        f.status = if collided {
            DestStatus::Collision
        } else if same_size {
            DestStatus::ExistsSameSize
        } else if differs_on_disk {
            DestStatus::ExistsDifferent
        } else {
            DestStatus::New
        };
        taken.insert(candidate);
    }
}

fn suffixed(p: &Path, n: usize) -> PathBuf {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let name = match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}_{n}.{ext}"),
        None => format!("{stem}_{n}"),
    };
    p.with_file_name(name)
}

/// A directory tree summary of one root, for the layout preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreeNode {
    pub name: String,
    pub is_dir: bool,
    pub files: usize,
    pub bytes: u64,
    pub status: Option<DestStatus>,
    /// A directory a token produced (as opposed to one written literally in the template),
    /// so the preview can colour it by depth. Always false for files and for roots.
    pub dynamic: bool,
    pub children: Vec<TreeNode>,
}

/// Folder names written literally by the templates that feed one root, by depth. A rendered
/// component that is not in its depth's set came from a token.
fn literal_dirs(templates: &[&Template]) -> Vec<HashSet<String>> {
    let mut out: Vec<HashSet<String>> = Vec::new();
    for t in templates {
        for (i, seg) in t.dir_segments().into_iter().enumerate() {
            if out.len() <= i {
                out.push(HashSet::new());
            }
            if let Some(name) = seg {
                out[i].insert(name);
            }
        }
    }
    out
}

/// One tree per distinct root directory (roots that share a path are merged).
pub fn tree(plan: &Plan, cfg: &Config) -> Vec<TreeNode> {
    let mut roots: BTreeMap<String, TreeNode> = BTreeMap::new();
    fn node_for<'a>(roots: &'a mut BTreeMap<String, TreeNode>, path: &Path) -> &'a mut TreeNode {
        roots
            .entry(path.to_string_lossy().to_string())
            .or_insert_with(|| TreeNode { name: path.to_string_lossy().to_string(), is_dir: true, ..Default::default() })
    }
    let t = &cfg.templates;
    let archive = literal_dirs(&[&t.raw, &t.paired_image, &t.image]);
    let video = literal_dirs(&[&t.video]);
    let other = literal_dirs(&[&t.other]);
    let render = literal_dirs(&[&t.render_exr, &t.render_jpeg]);
    for f in &plan.files {
        let lits = match f.root {
            Root::Archive => &archive,
            Root::Video => &video,
            Root::Other => &other,
            Root::Render => &render,
        };
        let node = node_for(&mut roots, root_path(cfg, f.root));
        insert(node, &f.rel, f.size, f.status.clone(), lits);
    }
    for r in &plan.renders {
        for p in [&r.exr, &r.jpeg].into_iter().flatten() {
            let node = node_for(&mut roots, &cfg.render_root);
            let rel = p.strip_prefix(&cfg.render_root).unwrap_or(p);
            let status = if p.exists() { DestStatus::ExistsDifferent } else { DestStatus::New };
            insert(node, rel, 0, status, &render);
        }
    }
    roots.into_values().collect()
}

fn insert(node: &mut TreeNode, rel: &Path, bytes: u64, status: DestStatus, lits: &[HashSet<String>]) {
    node.files += 1;
    node.bytes += bytes;
    let mut comps = rel.components().peekable();
    let mut cur = node;
    let mut depth = 0usize;
    while let Some(c) = comps.next() {
        let name = c.as_os_str().to_string_lossy().to_string();
        let is_last = comps.peek().is_none();
        let dynamic = !is_last && !lits.get(depth).is_some_and(|set| set.contains(&name));
        depth += 1;
        let idx = match cur.children.iter().position(|n| n.name == name) {
            Some(i) => i,
            None => {
                cur.children.push(TreeNode { name, is_dir: !is_last, dynamic, ..Default::default() });
                cur.children.len() - 1
            }
        };
        cur = &mut cur.children[idx];
        if is_last {
            cur.files = 1;
            cur.bytes = bytes;
            cur.status = Some(status.clone());
        } else {
            cur.files += 1;
            cur.bytes += bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{CaptureMeta, MetaSource};
    use crate::scan::{build_groups, Group, SourceInfo};
    use chrono::NaiveDate;

    fn scan_fixture(tmp: &Path) -> Scan {
        let mk = |id: u32, rel: &str, size: u64| {
            let path = tmp.join(rel);
            ScannedFile { id: FileId(id), rel: PathBuf::from(rel), kind: crate::scan::classify(&path), path, size, mtime: None }
        };
        let files = vec![
            mk(0, "DCIM/100_FUJI/DSCF0001.RAF", 30),
            mk(1, "DCIM/100_FUJI/DSCF0001.JPG", 5),
            mk(2, "PRIVATE/M4ROOT/CLIP/C0001.MP4", 900),
            mk(3, "PRIVATE/M4ROOT/CLIP/C0001M01.XML", 1),
            mk(4, "PRIVATE/M4ROOT/MEDIAPRO.XML", 1),
        ];
        let mut groups: Vec<Group> = build_groups(&files);
        groups.sort_by_key(|g| g.primary);
        for (i, g) in groups.iter_mut().enumerate() {
            g.id = GroupId(i as u32);
            g.meta = CaptureMeta {
                captured_at: Some(NaiveDate::from_ymd_opt(2026, 9, 15).unwrap().and_hms_opt(10, 0, 0).unwrap()),
                make: Some("FUJIFILM".into()),
                model: Some("X-T5".into()),
                camera: Some("X-T5".into()),
                iso: None,
                lens: None,
                orientation: None,
                source: MetaSource::Raw,
            };
        }
        Scan { source: SourceInfo { root: tmp.to_path_buf(), label: "CARD".into(), is_volume: false }, files, groups, errors: vec![] }
    }

    #[test]
    fn plans_all_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_fixture(tmp.path());
        let mut cfg = Config::default();
        cfg.archive_root = tmp.path().join("archive");
        cfg.video_root = Some(tmp.path().join("video"));
        cfg.render_root = tmp.path().join("render");
        cfg.outputs.exr = true;
        let included: Vec<GroupId> = scan.groups.iter().map(|g| g.id).collect();
        let now = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap().and_hms_opt(0, 0, 0).unwrap();
        let p = plan(&scan, &included, &cfg, now).unwrap();

        let by_file = |id: u32| p.files.iter().find(|f| f.file == FileId(id)).unwrap();
        assert_eq!(by_file(0).dest, tmp.path().join("archive/2026/2026-09-15/X-T5/DSCF0001.RAF"));
        assert_eq!(by_file(1).dest, tmp.path().join("archive/2026/2026-09-15/X-T5/DSCF0001.JPG"));
        assert_eq!(by_file(2).dest, tmp.path().join("video/2026/2026-09-15/video/C0001.MP4"));
        assert_eq!(by_file(3).dest, tmp.path().join("video/2026/2026-09-15/video/C0001M01.XML"));
        assert_eq!(by_file(4).dest, tmp.path().join("archive/2026-09-16/other/PRIVATE/M4ROOT/MEDIAPRO.XML"));
        assert_eq!(p.renders.len(), 1);
        assert_eq!(p.renders[0].exr.as_deref(), Some(tmp.path().join("render/2026/2026-09-15/X-T5/DSCF0001.exr").as_path()));
        assert_eq!(p.renders[0].jpeg.as_deref(), Some(tmp.path().join("render/2026/2026-09-15/X-T5/DSCF0001.jpg").as_path()));
        assert!(p.files.iter().all(|f| f.status == DestStatus::New));

        let t = tree(&p, &cfg);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].files, 3);
        assert_eq!(t[0].bytes, 36);
    }

    #[test]
    fn detects_existing_and_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_fixture(tmp.path());
        let mut cfg = Config::default();
        cfg.archive_root = tmp.path().join("archive");
        cfg.render_root = tmp.path().join("render");
        // Both the RAF and its JPEG map to the same name -> collision.
        cfg.templates.paired_image = Template::parse("{stem}.RAF").unwrap();
        cfg.templates.raw = Template::parse("{stem}.RAF").unwrap();
        // Pre-existing file with same size for the video.
        std::fs::create_dir_all(cfg.archive_root.join("2026/2026-09-15/video")).unwrap();
        std::fs::write(cfg.archive_root.join("2026/2026-09-15/video/C0001.MP4"), vec![0u8; 900]).unwrap();

        let included: Vec<GroupId> = scan.groups.iter().map(|g| g.id).collect();
        let now = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap().and_hms_opt(0, 0, 0).unwrap();
        let p = plan(&scan, &included, &cfg, now).unwrap();
        let by_file = |id: u32| p.files.iter().find(|f| f.file == FileId(id)).unwrap();
        assert_eq!(by_file(0).status, DestStatus::New);
        assert_eq!(by_file(1).status, DestStatus::Collision);
        assert_eq!(by_file(1).dest, cfg.archive_root.join("DSCF0001_1.RAF"));
        assert_eq!(by_file(2).status, DestStatus::ExistsSameSize);
    }
}
