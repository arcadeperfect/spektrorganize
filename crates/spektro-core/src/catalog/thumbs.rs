//! Catalog thumbnails: JPEGs in a cache directory keyed by asset id and size
//! (`<dir>/<size>/<id / 1000>/<id>.jpg`), made from the camera JPEG next to a RAW, the preview
//! embedded in the RAW, or the image itself.
//!
//! A `thumbnails` row remembers which source file version (`src_key`) a thumbnail was made from,
//! so a changed or newly paired file is picked up; a failed attempt is stored with no path so
//! the same file is not retried until it changes (negative cache).
//!
//! [`ThumbWorker`] generates in the background on a rayon pool with two queues: `priority` for
//! what is on screen right now, and a background queue for everything else. Results are written
//! by one collector thread in small batches and reported through a callback, so the UI never
//! waits on a thumbnail.

use super::{Catalog, now};
use crate::preview::make_thumbnails;
use crate::scan::PreviewSource;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SMALL: u32 = 256;
pub const LARGE: u32 = 1024;

/// A catalog-wide yes/no preference.
fn setting_on(conn: &Connection, key: &str, default: bool) -> bool {
    use rusqlite::OptionalExtension as _;
    match conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get::<_, String>(0)).optional() {
        Ok(Some(v)) => v == "1",
        _ => default,
    }
}

pub fn thumb_path(dir: &Path, asset: i64, size: u32) -> PathBuf {
    dir.join(size.to_string()).join((asset / 1000).to_string()).join(format!("{asset}.jpg"))
}

#[derive(Debug, Clone)]
pub struct ThumbJob {
    pub asset: i64,
    pub size: u32,
    pub src: PathBuf,
    pub source: PreviewSource,
    pub key: String,
    pub out: PathBuf,
}

fn source_str(s: PreviewSource) -> &'static str {
    match s {
        PreviewSource::SidecarJpeg => "sidecar_jpeg",
        PreviewSource::EmbeddedPreview => "embedded_preview",
        PreviewSource::Itself => "itself",
        PreviewSource::VideoFrame => "video_frame",
        PreviewSource::None => "none",
    }
}

struct Candidate {
    role: String,
    file: i64,
    abs: PathBuf,
    size: i64,
    mtime: Option<i64>,
    usable: bool,
}

/// Thumbnails that need making at `size`: assets (RAW or image) with no row, or whose preview
/// source changed since. `ids = None` means the whole catalog. With `force`, existing and failed
/// thumbnails are redone too (e.g. the cache was cleared). Files on offline roots are skipped.
pub fn jobs(conn: &Connection, dir: &Path, ids: Option<&[i64]>, size: u32, force: bool) -> anyhow::Result<Vec<ThumbJob>> {
    fn prefer_prints(conn: &Connection) -> bool {
        setting_on(conn, "thumbs_from_prints", false)
    }

    /// The most recent JPEG print of each asset, on an online root.
    fn newest_prints(conn: &Connection, ids: Option<&[i64]>) -> anyhow::Result<HashMap<i64, Candidate>> {
        let base = "SELECT d.asset_id, d.id, r.path, d.rel, r.online
                    FROM renders d JOIN roots r ON r.id = d.root_id
                    WHERE d.kind = 'jpeg'";
        let tail = " ORDER BY d.created_at DESC, d.id DESC";
        let mut out: HashMap<i64, Candidate> = HashMap::new();
        let mut take = |sql: &str, p: Vec<rusqlite::types::Value>| -> anyhow::Result<()> {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(p))?;
            while let Some(r) = rows.next()? {
                let asset: i64 = r.get(0)?;
                if out.contains_key(&asset) {
                    continue; // the first row per asset is the newest
                }
                let root: String = r.get(2)?;
                let rel: String = r.get(3)?;
                let abs = Path::new(&root).join(rel);
                let online: bool = r.get(4)?;
                // Size and mtime come from the file, so printing over the same path
                // gives a new key and the thumbnail is made again.
                let md = std::fs::metadata(&abs).ok();
                let usable = online && md.is_some();
                let size = md.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                let mtime = md.as_ref().and_then(|m| m.modified().ok()).and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_nanos() as i64)
                });
                out.insert(asset, Candidate { role: "print".into(), file: r.get(1)?, abs, size, mtime, usable });
            }
            Ok(())
        };
        match ids {
            None => take(&format!("{base}{tail}"), Vec::new())?,
            Some(ids) => {
                for chunk in ids.chunks(500) {
                    let sql = format!("{base} AND d.asset_id IN ({}){tail}", vec!["?"; chunk.len()].join(", "));
                    take(&sql, chunk.iter().map(|i| rusqlite::types::Value::Integer(*i)).collect())?;
                }
            }
        }
        Ok(out)
    }

    let mut members: HashMap<i64, Vec<Candidate>> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();
    let base = "SELECT a.id, af.role, f.id, r.path, f.rel, f.size, f.mtime_ns, f.missing = 0 AND r.online = 1
                FROM assets a JOIN asset_files af ON af.asset_id = a.id JOIN files f ON f.id = af.file_id JOIN roots r ON r.id = f.root_id
                WHERE a.kind IN ('raw', 'image', 'video') AND af.role IN ('raw', 'jpeg', 'image', 'video')";
    let mut collect = |sql: &str, p: Vec<rusqlite::types::Value>| -> anyhow::Result<()> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(p))?;
        while let Some(r) = rows.next()? {
            let asset: i64 = r.get(0)?;
            let root: String = r.get(3)?;
            let rel: String = r.get(4)?;
            let e = members.entry(asset).or_default();
            if e.is_empty() {
                order.push(asset);
            }
            e.push(Candidate {
                role: r.get(1)?,
                file: r.get(2)?,
                abs: Path::new(&root).join(rel),
                size: r.get(5)?,
                mtime: r.get(6)?,
                usable: r.get(7)?,
            });
        }
        Ok(())
    };
    match ids {
        None => collect(&format!("{base} ORDER BY a.captured_at IS NULL, a.captured_at DESC, a.id DESC"), Vec::new())?,
        Some(ids) => {
            for chunk in ids.chunks(500) {
                let sql = format!("{base} AND a.id IN ({})", vec!["?"; chunk.len()].join(", "));
                collect(&sql, chunk.iter().map(|i| rusqlite::types::Value::Integer(*i)).collect())?;
            }
            // Keep the caller's order (it is usually what is on screen, top to bottom).
            let rank: HashMap<i64, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            order.sort_by_key(|a| rank.get(a).copied().unwrap_or(usize::MAX));
        }
    }

    // Videos encoded with HEVC, and whether their posters are wanted at all.
    let hevc_ok = setting_on(conn, "video_thumbs_hevc", true);
    let hevc: std::collections::HashSet<i64> = if hevc_ok {
        std::collections::HashSet::new()
    } else {
        let mut stmt = conn.prepare("SELECT id FROM assets WHERE codec IN ('hvc1', 'hev1', 'dvh1', 'dvhe')")?;
        stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };

    // With `thumbs_from_prints`, the newest JPEG print stands in for the camera rendition.
    let prints: HashMap<i64, Candidate> = if prefer_prints(conn) { newest_prints(conn, ids)? } else { HashMap::new() };

    let existing: HashMap<i64, String> = {
        let mut stmt = conn.prepare("SELECT asset_id, src_key FROM thumbnails WHERE size = ?1")?;
        stmt.query_map([size], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };

    let mut out = Vec::new();
    for asset in order {
        let files = &members[&asset];
        let pick = prints
            .get(&asset)
            .filter(|p| p.usable)
            .map(|p| (p, PreviewSource::Itself))
            .or_else(|| files.iter().find(|f| f.role == "jpeg" && f.usable).map(|f| (f, PreviewSource::SidecarJpeg)))
            .or_else(|| files.iter().find(|f| f.role == "raw" && f.usable).map(|f| (f, PreviewSource::EmbeddedPreview)))
            .or_else(|| files.iter().find(|f| f.role == "image" && f.usable).map(|f| (f, PreviewSource::Itself)))
            .or_else(|| files.iter().find(|f| f.role == "video" && f.usable).map(|f| (f, PreviewSource::VideoFrame)));
        // HEVC decodes several times slower than H.264 and often without hardware help, so a
        // library full of it can be left alone.
        if let Some((_, PreviewSource::VideoFrame)) = pick
            && !hevc_ok
            && hevc.contains(&asset)
        {
            continue;
        }
        let Some((f, source)) = pick else { continue };
        let key = format!("{}:{}:{}:{}", f.role, f.file, f.size, f.mtime.unwrap_or(0));
        if !force && existing.get(&asset) == Some(&key) {
            continue;
        }
        out.push(ThumbJob { asset, size, src: f.abs.clone(), source, key, out: thumb_path(dir, asset, size) });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct ThumbReady {
    pub id: i64,
    pub size: u32,
    /// `None`: no preview could be made (shown as a placeholder; not retried until the file changes).
    pub path: Option<String>,
}

/// Make one thumbnail. Errors and "no preview" both come back as `None`.
pub fn generate(job: &ThumbJob) -> (Option<(u32, u32)>, Option<String>) {
    match make_thumbnails(&job.src, job.source, &[(job.size, &job.out)]) {
        Ok(Some(dims)) => (Some(dims), None),
        Ok(None) => (None, Some("no preview in file".into())),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// Store the outcome of a job.
pub fn record(conn: &Connection, job: &ThumbJob, dims: Option<(u32, u32)>) -> anyhow::Result<()> {
    let path = dims.map(|_| job.out.to_string_lossy().to_string());
    conn.prepare_cached(
        "INSERT INTO thumbnails (asset_id, size, path, source, src_key, width, height, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (asset_id, size) DO UPDATE SET path = excluded.path, source = excluded.source, src_key = excluded.src_key,
            width = excluded.width, height = excluded.height, created_at = excluded.created_at",
    )?
    .execute(params![
        job.asset,
        job.size,
        path,
        if dims.is_some() { source_str(job.source) } else { "none" },
        job.key,
        dims.map(|d| d.0),
        dims.map(|d| d.1),
        now()
    ])?;
    Ok(())
}

/// Generate thumbnails synchronously on the current thread's rayon pool (CLI, tests, and the
/// one-off large preview in the detail panel). Returns (made, failed).
pub fn generate_all(cat: &Catalog, jobs: &[ThumbJob], mut progress: impl FnMut(usize, usize)) -> anyhow::Result<(usize, usize)> {
    use rayon::prelude::*;
    let (mut made, mut failed) = (0, 0);
    for chunk in jobs.chunks(32) {
        let results: Vec<Option<(u32, u32)>> = chunk.par_iter().map(|j| generate(j).0).collect();
        let tx = cat.conn().unchecked_transaction()?;
        for (job, dims) in chunk.iter().zip(results) {
            record(&tx, job, dims)?;
            if dims.is_some() {
                made += 1
            } else {
                failed += 1
            }
        }
        tx.commit()?;
        progress(made + failed, jobs.len());
    }
    Ok((made, failed))
}

struct Queues {
    high: VecDeque<ThumbJob>,
    low: VecDeque<ThumbJob>,
    /// Queued and not started, by (asset, size).
    pending: HashSet<(i64, u32)>,
}

/// Background thumbnail generator. Cheap to clone.
#[derive(Clone)]
pub struct ThumbWorker {
    queues: Arc<Mutex<Queues>>,
    pool: Arc<rayon::ThreadPool>,
    done_tx: crossbeam_channel::Sender<(ThumbJob, Option<(u32, u32)>)>,
}

impl ThumbWorker {
    /// `catalog` is the database file results are written to (on a connection of its own);
    /// `on_ready` receives each written batch.
    pub fn new(catalog: &Path, threads: usize, on_ready: impl Fn(Vec<ThumbReady>) + Send + 'static) -> anyhow::Result<ThumbWorker> {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).thread_name(|i| format!("thumbs-{i}")).build()?;
        let (done_tx, done_rx) = crossbeam_channel::unbounded::<(ThumbJob, Option<(u32, u32)>)>();
        let cat = Catalog::open(catalog)?;
        std::thread::Builder::new().name("thumbs-writer".into()).spawn(move || {
            let mut batch: Vec<(ThumbJob, Option<(u32, u32)>)> = Vec::new();
            loop {
                match done_rx.recv_timeout(Duration::from_millis(120)) {
                    Ok(item) => {
                        batch.push(item);
                        if batch.len() < 48 {
                            continue;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        if batch.is_empty() {
                            return;
                        }
                    }
                }
                if batch.is_empty() {
                    continue;
                }
                let write = || -> anyhow::Result<()> {
                    let tx = cat.conn().unchecked_transaction()?;
                    for (job, dims) in &batch {
                        record(&tx, job, *dims)?;
                    }
                    tx.commit()?;
                    Ok(())
                };
                if let Err(e) = write() {
                    tracing::warn!("thumbnail rows: {e}");
                }
                on_ready(
                    batch
                        .drain(..)
                        .map(|(job, dims)| ThumbReady { id: job.asset, size: job.size, path: dims.map(|_| job.out.to_string_lossy().to_string()) })
                        .collect(),
                );
            }
        })?;
        Ok(ThumbWorker {
            queues: Arc::new(Mutex::new(Queues { high: VecDeque::new(), low: VecDeque::new(), pending: HashSet::new() })),
            pool: Arc::new(pool),
            done_tx,
        })
    }

    /// Queue jobs. `priority` jobs (on screen now) run before everything queued in the
    /// background; queuing an already-queued thumbnail with priority bumps it.
    pub fn enqueue(&self, jobs: Vec<ThumbJob>, priority: bool) {
        let mut spawn = 0;
        {
            let mut q = self.queues.lock().unwrap();
            for job in jobs {
                let key = (job.asset, job.size);
                let fresh = q.pending.insert(key);
                if priority {
                    q.high.push_back(job);
                } else if fresh {
                    q.low.push_back(job);
                } else {
                    continue;
                }
                spawn += 1;
            }
        }
        // Each task takes whatever is most urgent when it gets to run.
        for _ in 0..spawn {
            let queues = self.queues.clone();
            let done = self.done_tx.clone();
            self.pool.spawn(move || {
                let job = {
                    let mut q = queues.lock().unwrap();
                    let mut next = None;
                    while let Some(j) = q.high.pop_front().or_else(|| q.low.pop_front()) {
                        // A bumped job sits in both queues; whichever copy comes first runs.
                        if q.pending.remove(&(j.asset, j.size)) {
                            next = Some(j);
                            break;
                        }
                    }
                    next
                };
                if let Some(job) = job {
                    let (dims, err) = generate(&job);
                    if let Some(e) = err {
                        tracing::debug!("thumbnail #{} {}: {e}", job.asset, job.src.display());
                    }
                    let _ = done.send((job, dims));
                }
            });
        }
    }

    /// Thumbnails queued and not yet started.
    pub fn pending(&self) -> usize {
        self.queues.lock().unwrap().pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::index::add_folder;
    use crate::job::Cancel;

    fn jpeg(path: &Path, w: u32, h: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x % 255) as u8, 90, 160])).save(path).unwrap();
    }

    #[test]
    fn makes_thumbnails_once_and_redoes_changed_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        jpeg(&root.join("a/one.jpg"), 900, 600);
        jpeg(&root.join("a/two.jpg"), 300, 600);
        std::fs::write(root.join("a/broken.jpg"), b"not a jpeg").unwrap();
        std::fs::write(root.join("a/clip.mp4"), b"video").unwrap();
        let dbp = tmp.path().join("c.sqlite");
        let mut cat = Catalog::open(&dbp).unwrap();
        add_folder(&mut cat, &root, &mut |_| {}, &Cancel::new()).unwrap();
        let dir = tmp.path().join("thumbs");

        let todo = jobs(cat.conn(), &dir, None, SMALL, false).unwrap();
        assert_eq!(todo.len(), 4, "the two photos, the broken one, and the clip's poster frame");
        let (made, failed) = generate_all(&cat, &todo, |_, _| {}).unwrap();
        // The "video" here is four bytes of text, so its poster fails like the broken JPEG does.
        assert_eq!((made, failed), (2, 2));
        assert!(jobs(cat.conn(), &dir, None, SMALL, false).unwrap().is_empty(), "failures are negative-cached");
        let page = cat.list(&Default::default(), Default::default(), 0, 10).unwrap();
        let one = page.items.iter().find(|a| a.name == "one.jpg").unwrap();
        let p = one.thumb.as_ref().unwrap();
        assert!(p.ends_with(&format!("256/0/{}.jpg", one.id)));
        let (w, h) = image::image_dimensions(p).unwrap();
        assert_eq!(w.max(h), 256);

        // The source changes: only that asset is redone.
        jpeg(&root.join("a/one.jpg"), 1000, 500);
        filetime::set_file_mtime(root.join("a/one.jpg"), filetime::FileTime::from_unix_time(1_800_000_000, 0)).unwrap();
        add_folder(&mut cat, &root, &mut |_| {}, &Cancel::new()).unwrap();
        let todo = jobs(cat.conn(), &dir, None, SMALL, false).unwrap();
        assert_eq!(todo.iter().map(|j| j.asset).collect::<Vec<_>>(), vec![one.id]);
        assert_eq!(jobs(cat.conn(), &dir, Some(&[one.id]), SMALL, true).unwrap().len(), 1);
    }

    #[test]
    fn worker_runs_in_background_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        for i in 0..6 {
            jpeg(&root.join(format!("p{i}.jpg")), 400, 300);
        }
        let dbp = tmp.path().join("c.sqlite");
        let mut cat = Catalog::open(&dbp).unwrap();
        add_folder(&mut cat, &root, &mut |_| {}, &Cancel::new()).unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let worker = ThumbWorker::new(&dbp, 2, move |batch| {
            for r in batch {
                tx.send(r).unwrap();
            }
        })
        .unwrap();
        let all = jobs(cat.conn(), &tmp.path().join("t"), None, SMALL, false).unwrap();
        let first = all[0].asset;
        worker.enqueue(all.clone(), false);
        worker.enqueue(all[..1].to_vec(), true);
        let mut got = Vec::new();
        while got.len() < 6 {
            got.push(rx.recv_timeout(Duration::from_secs(20)).expect("thumbnail result"));
        }
        assert!(got.iter().all(|r| r.path.is_some()));
        assert!(got.iter().any(|r| r.id == first));
        assert_eq!(worker.pending(), 0);
        // Rows are written by the worker's own connection.
        std::thread::sleep(Duration::from_millis(50));
        let n: i64 = cat.conn().query_row("SELECT COUNT(*) FROM thumbnails WHERE path IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 6);
    }
}
