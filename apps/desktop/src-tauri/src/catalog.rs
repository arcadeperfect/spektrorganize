//! Catalog commands: library queries, indexing jobs, keywords, thumbnails, prints, AI key.
//!
//! The UI reads through one connection (`CatalogState::db`); indexing and printing run on
//! threads with connections of their own; thumbnails come from a background `ThumbWorker`.
//!
//! Events: `catalog-index` (progress of an add/adopt/rescan), `catalog-index-done`,
//! `catalog-index-error`, `catalog-changed` (reload the view), `thumbs-ready` (a batch of
//! generated thumbnails). Prints report through the import job's `job-event` / `job-done`.

use crate::{AppState, err};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use spektro_core::catalog::index::{self, IndexProgress};
use spektro_core::catalog::keywords::{self, AssetLabels};
use spektro_core::catalog::query::{AssetDetail, Facets, Filter, LabelInfo, Page, Sort};
use spektro_core::catalog::thumbs::{self, ThumbReady, ThumbWorker};
use spektro_core::catalog::{Catalog, RelocateReport, RootKind};
use spektro_core::job::{Cancel, JobEvent};
use spektro_core::manifest::Manifest;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

type Result<T> = std::result::Result<T, String>;

pub struct CatalogState {
    pub path: PathBuf,
    pub thumbs_dir: PathBuf,
    pub db: Mutex<Catalog>,
    pub thumbs: ThumbWorker,
    indexing: AtomicBool,
    index_cancel: Mutex<Option<Cancel>>,
}

impl CatalogState {
    pub fn new(app: &AppHandle) -> anyhow::Result<CatalogState> {
        let data = app.path().app_data_dir()?;
        let cache = app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir().join("spektrorganize"));
        let path = data.join("catalog.sqlite");
        let thumbs_dir = cache.join("catalog-thumbs");
        let db = Catalog::open(&path)?;
        let h = app.clone();
        let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(2, 6)).unwrap_or(2);
        let thumbs = ThumbWorker::new(&path, threads, move |batch: Vec<ThumbReady>| {
            let _ = h.emit("thumbs-ready", batch);
        })?;
        Ok(CatalogState { path, thumbs_dir, db: Mutex::new(db), thumbs, indexing: AtomicBool::new(false), index_cancel: Mutex::new(None) })
    }

    /// Queue every missing 256 px thumbnail in the background (after startup or an index job).
    pub fn queue_missing_thumbs(&self) {
        let jobs = {
            let db = self.db.lock().unwrap();
            thumbs::jobs(db.conn(), &self.thumbs_dir, None, thumbs::SMALL, false)
        };
        match jobs {
            Ok(j) if !j.is_empty() => self.thumbs.enqueue(j, false),
            Ok(_) => {}
            Err(e) => tracing::warn!("thumbnail jobs: {e}"),
        }
    }
}

/// Render folders hold full-size prints the detail panel shows; let the webview load them.
pub fn allow_render_roots(app: &AppHandle) {
    let st = app.state::<CatalogState>();
    let roots = st.db.lock().unwrap().roots().unwrap_or_default();
    for r in roots.into_iter().filter(|r| r.kind == RootKind::Render) {
        if let Err(e) = app.asset_protocol_scope().allow_directory(&r.path, true) {
            tracing::warn!("asset scope {}: {e}", r.path.display());
        }
    }
}

/// Startup work that must not delay the window: online check, render scopes, thumbnails.
pub fn warm_up(app: AppHandle) {
    std::thread::spawn(move || {
        let st = app.state::<CatalogState>();
        if let Err(e) = st.db.lock().unwrap().refresh_online() {
            tracing::warn!("refresh roots: {e}");
        }
        allow_render_roots(&app);
        st.queue_missing_thumbs();
    });
}

/// Record an import manifest in the catalog (after the copy phase, and again after rendering).
pub fn index_manifest(app: &AppHandle, manifest_path: &Path, manifest: &Manifest) {
    let st = app.state::<CatalogState>();
    let result = Catalog::open(&st.path).and_then(|mut c| index::index_manifest(&mut c, manifest_path, manifest));
    match result {
        Ok(r) => {
            let _ = app.emit("catalog-changed", ());
            if !r.touched_assets.is_empty() {
                let jobs = thumbs::jobs(st.db.lock().unwrap().conn(), &st.thumbs_dir, Some(&r.touched_assets), thumbs::SMALL, false);
                if let Ok(j) = jobs {
                    st.thumbs.enqueue(j, false);
                }
            }
            if r.renders > 0 {
                allow_render_roots(app);
            }
        }
        Err(e) => {
            tracing::warn!("catalog: indexing {}: {e:#}", manifest_path.display());
            let _ = app.emit("job-event", JobEvent::Log(format!("catalog: could not index this import: {e:#}")));
        }
    }
}

// ---------- queries ----------

#[derive(Serialize)]
pub struct CatalogInfo {
    pub path: PathBuf,
    pub thumbs_dir: PathBuf,
    pub assets: i64,
    pub schema: u32,
}

#[tauri::command]
pub fn catalog_info(st: State<CatalogState>) -> Result<CatalogInfo> {
    let db = st.db.lock().unwrap();
    Ok(CatalogInfo { path: st.path.clone(), thumbs_dir: st.thumbs_dir.clone(), assets: db.asset_count().map_err(err)?, schema: spektro_core::catalog::SCHEMA_VERSION })
}

#[tauri::command]
pub fn catalog_facets(st: State<CatalogState>) -> Result<Facets> {
    st.db.lock().unwrap().facets().map_err(err)
}

#[tauri::command]
pub fn catalog_list(st: State<CatalogState>, filter: Filter, sort: Sort, offset: i64, limit: i64) -> Result<Page> {
    st.db.lock().unwrap().list(&filter, sort, offset, limit).map_err(err)
}

#[tauri::command]
pub fn catalog_ids(st: State<CatalogState>, filter: Filter, sort: Sort) -> Result<Vec<i64>> {
    st.db.lock().unwrap().ids(&filter, sort).map_err(err)
}

#[tauri::command]
pub fn catalog_asset(st: State<CatalogState>, id: i64) -> Result<AssetDetail> {
    st.db.lock().unwrap().detail(id).map_err(err)
}

// ---------- indexing ----------

fn start_index_job(
    app: AppHandle,
    st: &CatalogState,
    label: String,
    job: impl FnOnce(&mut Catalog, &mut dyn FnMut(IndexProgress), &Cancel) -> anyhow::Result<(serde_json::Value, Vec<i64>)> + Send + 'static,
) -> Result<()> {
    if st.indexing.swap(true, Ordering::SeqCst) {
        return Err("already indexing; wait for it to finish".into());
    }
    let cancel = Cancel::new();
    *st.index_cancel.lock().unwrap() = Some(cancel.clone());
    let path = st.path.clone();
    std::thread::spawn(move || {
        let h = app.clone();
        let lbl = label.clone();
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let mut progress = move |p: IndexProgress| {
            let end = matches!(&p, IndexProgress::Writing { done, total } if done == total);
            if end || last.elapsed().as_millis() >= 100 {
                last = std::time::Instant::now();
                let _ = h.emit("catalog-index", serde_json::json!({ "label": lbl, "progress": p }));
            }
        };
        let result = Catalog::open(&path).and_then(|mut c| job(&mut c, &mut progress, &cancel));
        let st = app.state::<CatalogState>();
        st.indexing.store(false, Ordering::SeqCst);
        *st.index_cancel.lock().unwrap() = None;
        match result {
            Ok((report, _touched)) => {
                let _ = app.emit("catalog-index-done", serde_json::json!({ "label": label, "report": report }));
                let _ = app.emit("catalog-changed", ());
                allow_render_roots(&app);
                st.queue_missing_thumbs();
            }
            Err(e) => {
                let _ = app.emit("catalog-index-error", serde_json::json!({ "label": label, "error": format!("{e:#}") }));
                let _ = app.emit("catalog-changed", ());
            }
        }
    });
    Ok(())
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
}

/// Index an existing folder in place (or rescan it if it is already in the catalog).
#[tauri::command]
pub fn catalog_add_folder(app: AppHandle, st: State<CatalogState>, path: String) -> Result<()> {
    let path = PathBuf::from(path);
    start_index_job(app, &st, folder_name(&path), move |c, progress, cancel| {
        let r = index::add_folder(c, &path, progress, cancel)?;
        Ok((serde_json::to_value(&r)?, r.touched_assets))
    })
}

/// Adopt an import archive from its manifests (default: the configured archive root).
#[tauri::command]
pub fn catalog_adopt(app: AppHandle, st: State<CatalogState>, state: State<AppState>, path: Option<String>) -> Result<()> {
    let path = path.map(PathBuf::from).unwrap_or_else(|| state.config.lock().unwrap().archive_root.clone());
    start_index_job(app, &st, folder_name(&path), move |c, progress, _cancel| {
        let r = index::adopt_archive(c, &path, progress)?;
        Ok((serde_json::to_value(&r)?, r.touched_assets))
    })
}

#[tauri::command]
pub fn catalog_rescan(app: AppHandle, st: State<CatalogState>, root: i64) -> Result<()> {
    let label = st.db.lock().unwrap().root(root).map(|r| r.label).map_err(err)?;
    start_index_job(app, &st, label, move |c, progress, cancel| {
        let r = index::index_root(c, root, progress, cancel)?;
        Ok((serde_json::to_value(&r)?, r.touched_assets))
    })
}

#[tauri::command]
pub fn catalog_cancel_index(st: State<CatalogState>) {
    if let Some(c) = st.index_cancel.lock().unwrap().as_ref() {
        c.cancel();
    }
}

#[tauri::command]
pub fn catalog_relocate_root(app: AppHandle, st: State<CatalogState>, root: i64, path: String, force: bool) -> Result<RelocateReport> {
    let r = st.db.lock().unwrap().relocate_root(root, Path::new(&path), force).map_err(err)?;
    allow_render_roots(&app);
    let _ = app.emit("catalog-changed", ());
    Ok(r)
}

#[tauri::command]
pub fn catalog_remove_root(app: AppHandle, st: State<CatalogState>, root: i64) -> Result<()> {
    st.db.lock().unwrap().remove_root(root).map_err(err)?;
    let _ = app.emit("catalog-changed", ());
    Ok(())
}

// ---------- thumbnails ----------

/// Thumbnails for what is on screen: missing ones are queued ahead of the background work (and
/// arrive as `thumbs-ready`); ones that exist already are returned right away.
#[tauri::command]
pub fn catalog_request_thumbs(st: State<CatalogState>, ids: Vec<i64>, force: bool) -> Result<Vec<ThumbReady>> {
    let db = st.db.lock().unwrap();
    let jobs = thumbs::jobs(db.conn(), &st.thumbs_dir, Some(&ids), thumbs::SMALL, force).map_err(err)?;
    let queued: std::collections::HashSet<i64> = jobs.iter().map(|j| j.asset).collect();
    let mut known = Vec::new();
    {
        let mut stmt = db.conn().prepare_cached("SELECT path FROM thumbnails WHERE asset_id = ?1 AND size = ?2").map_err(err)?;
        for id in ids.iter().filter(|id| !queued.contains(id)) {
            use rusqlite::OptionalExtension as _;
            let row: Option<Option<String>> = stmt.query_row(rusqlite::params![id, thumbs::SMALL], |r| r.get(0)).optional().map_err(err)?;
            // No row and no job: nothing to preview (video, offline drive).
            known.push(ThumbReady { id: *id, size: thumbs::SMALL, path: row.flatten() });
        }
    }
    drop(db);
    st.thumbs.enqueue(jobs, true);
    Ok(known)
}

fn thumb_now(st: &CatalogState, id: i64, size: u32) -> Result<Option<String>> {
    let job = {
        let db = st.db.lock().unwrap();
        let jobs = thumbs::jobs(db.conn(), &st.thumbs_dir, Some(&[id]), size, false).map_err(err)?;
        if jobs.is_empty() {
            use rusqlite::OptionalExtension as _;
            let existing: Option<Option<String>> = db
                .conn()
                .query_row("SELECT path FROM thumbnails WHERE asset_id = ?1 AND size = ?2", rusqlite::params![id, size], |r| r.get(0))
                .optional()
                .map_err(err)?;
            match existing.flatten() {
                Some(p) if Path::new(&p).exists() => return Ok(Some(p)),
                Some(_) => thumbs::jobs(db.conn(), &st.thumbs_dir, Some(&[id]), size, true).map_err(err)?.into_iter().next(),
                None => None,
            }
        } else {
            jobs.into_iter().next()
        }
    };
    let Some(job) = job else { return Ok(None) };
    let (dims, _) = thumbs::generate(&job);
    thumbs::record(st.db.lock().unwrap().conn(), &job, dims).map_err(err)?;
    Ok(dims.map(|_| job.out.to_string_lossy().to_string()))
}

/// The 1024 px preview for the detail panel (made on demand, then cached).
#[tauri::command]
pub async fn catalog_preview(app: AppHandle, id: i64) -> Result<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || thumb_now(&app.state::<CatalogState>(), id, thumbs::LARGE)).await.map_err(err)?
}

/// A viewable copy of one print, cached in the app's own cache directory. The render itself
/// lives wherever the user keeps their renders, which the webview cannot read; this copy is
/// somewhere it can.
#[tauri::command]
pub async fn catalog_render_preview(app: AppHandle, render: i64, max_px: u32) -> Result<Option<String>> {
    let size = (max_px.div_ceil(1024) * 1024).clamp(1024, 4096);
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<CatalogState>();
        let Some((src, online)) = st.db.lock().unwrap().render_path(render).map_err(err)? else { return Ok(None) };
        if !online || !src.exists() {
            return Ok(None);
        }
        let out = st.thumbs_dir.join("renders").join(size.to_string()).join(format!("{render}.jpg"));
        let fresh = match (std::fs::metadata(&out), std::fs::metadata(&src)) {
            (Ok(o), Ok(s)) => match (o.modified(), s.modified()) {
                (Ok(om), Ok(sm)) => om >= sm,
                _ => false,
            },
            _ => false,
        };
        if !fresh {
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir).map_err(err)?;
            }
            spektro_core::preview::make_thumbnails(&src, spektro_core::scan::PreviewSource::Itself, &[(size, &out)])
                .map_err(err)?;
        }
        Ok(out.exists().then(|| out.to_string_lossy().to_string()))
    })
    .await
    .map_err(err)?
}

// ---------- duplicates ----------

/// Every group of byte-identical files. Hashing runs on a worker thread and reports progress.
#[tauri::command]
pub async fn catalog_find_duplicates(
    app: AppHandle,
    scope: Option<spektro_core::catalog::duplicates::DupScope>,
) -> Result<spektro_core::catalog::duplicates::DupScan> {
    let scope = scope.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<CatalogState>();
        let db = st.db.lock().unwrap();
        db.find_duplicates(&scope, |done, total| {
            let _ = app.emit("duplicates-progress", (done, total));
        })
        .map_err(err)
    })
    .await
    .map_err(err)?
}

/// Keep one copy, remove the others. `delete` bypasses the trash; the default is recoverable.
#[tauri::command]
pub async fn catalog_purge_duplicates(
    app: AppHandle,
    keep: i64,
    drop: Vec<i64>,
    delete: bool,
) -> Result<spektro_core::catalog::purge::PurgeReport> {
    use spektro_core::catalog::purge::Removal;
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<CatalogState>();
        let out = {
            let db = st.db.lock().unwrap();
            db.purge_duplicates(keep, &drop, if delete { Removal::Delete } else { Removal::Trash }).map_err(err)?
        };
        let _ = app.emit("catalog-changed", ());
        Ok(out)
    })
    .await
    .map_err(err)?
}

/// The files these photos are made of — shown for review before anything is removed.
#[tauri::command]
pub fn catalog_doomed_files(st: State<CatalogState>, ids: Vec<i64>) -> Result<Vec<spektro_core::catalog::purge::DoomedFile>> {
    st.db.lock().unwrap().files_of_assets(&ids).map_err(err)
}

/// Remove whole photos: every file of each, then the photo itself.
#[tauri::command]
pub async fn catalog_purge_assets(
    app: AppHandle,
    ids: Vec<i64>,
    delete: bool,
) -> Result<spektro_core::catalog::purge::PurgeReport> {
    use spektro_core::catalog::purge::Removal;
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<CatalogState>();
        let out = {
            let db = st.db.lock().unwrap();
            db.purge_assets(&ids, if delete { Removal::Delete } else { Removal::Trash }).map_err(err)?
        };
        let _ = app.emit("catalog-changed", ());
        Ok(out)
    })
    .await
    .map_err(err)?
}

// ---------- dynamic catalogs ----------

#[tauri::command]
pub fn catalog_collections(st: State<CatalogState>) -> Result<Vec<spektro_core::catalog::Collection>> {
    st.db.lock().unwrap().collections().map_err(err)
}

#[tauri::command]
pub fn catalog_save_collection(
    st: State<CatalogState>,
    name: String,
    filter: serde_json::Value,
    sort: Option<String>,
) -> Result<i64> {
    st.db.lock().unwrap().save_collection(&name, &filter, sort.as_deref()).map_err(err)
}

#[tauri::command]
pub fn catalog_delete_collection(st: State<CatalogState>, id: i64) -> Result<()> {
    st.db.lock().unwrap().delete_collection(id).map_err(err)
}

/// A catalog-wide preference, by key.
#[tauri::command]
pub fn catalog_get_setting(st: State<CatalogState>, key: String) -> Result<Option<String>> {
    Ok(st.db.lock().unwrap().setting(&key))
}

#[tauri::command]
pub fn catalog_set_setting(app: AppHandle, st: State<CatalogState>, key: String, value: String) -> Result<()> {
    st.db.lock().unwrap().set_setting(&key, &value).map_err(err)?;
    let _ = app.emit("catalog-changed", ());
    Ok(())
}

/// Whether library thumbnails come from the newest print instead of the camera rendition.
#[tauri::command]
pub fn catalog_thumbs_from_prints(st: State<CatalogState>) -> Result<bool> {
    Ok(st.db.lock().unwrap().setting("thumbs_from_prints").as_deref() == Some("1"))
}

/// Switch that preference, and forget the thumbnails it changes so they are made again.
#[tauri::command]
pub fn catalog_set_thumbs_from_prints(app: AppHandle, st: State<CatalogState>, on: bool) -> Result<()> {
    {
        let db = st.db.lock().unwrap();
        db.set_setting("thumbs_from_prints", if on { "1" } else { "0" }).map_err(err)?;
        db.forget_print_thumbnails().map_err(err)?;
    }
    let _ = app.emit("catalog-changed", ());
    Ok(())
}

/// A preview at an arbitrary size, for a zoomed full-screen view. Sizes are rounded to 1024 px
/// steps so the cache holds a handful of variants rather than one per zoom level.
#[tauri::command]
pub async fn catalog_preview_px(app: AppHandle, id: i64, max_px: u32) -> Result<Option<String>> {
    let size = (max_px.div_ceil(1024) * 1024).clamp(thumbs::LARGE, 4096);
    tauri::async_runtime::spawn_blocking(move || thumb_now(&app.state::<CatalogState>(), id, size)).await.map_err(err)?
}

/// The 256 px thumbnail as a data URL, for the AI labeller.
#[tauri::command]
pub async fn catalog_thumb_data(app: AppHandle, id: i64) -> Result<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = thumb_now(&app.state::<CatalogState>(), id, thumbs::SMALL)? else { return Ok(None) };
        let bytes = std::fs::read(path).map_err(err)?;
        Ok(Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
    })
    .await
    .map_err(err)?
}

// ---------- keywords, rating, AI labels ----------

#[tauri::command]
pub fn catalog_tag(app: AppHandle, st: State<CatalogState>, ids: Vec<i64>, add: Vec<String>, remove: Vec<String>) -> Result<()> {
    let db = st.db.lock().unwrap();
    keywords::add(db.conn(), &ids, &add, keywords::USER).map_err(err)?;
    keywords::remove(db.conn(), &ids, &remove).map_err(err)?;
    let _ = app.emit("catalog-keywords", &ids);
    Ok(())
}

/// Pick / reject flag: "select", "reject", or null to clear.
#[tauri::command]
pub fn catalog_set_flag(st: State<CatalogState>, ids: Vec<i64>, flag: Option<String>) -> Result<()> {
    st.db.lock().unwrap().set_flag(&ids, flag.as_deref()).map_err(err)
}

#[tauri::command]
pub fn catalog_set_rating(st: State<CatalogState>, ids: Vec<i64>, rating: i64) -> Result<()> {
    st.db.lock().unwrap().set_rating(&ids, rating).map_err(err)
}

#[tauri::command]
pub fn catalog_label_info(st: State<CatalogState>, ids: Vec<i64>) -> Result<Vec<LabelInfo>> {
    st.db.lock().unwrap().label_info(&ids).map_err(err)
}

/// Store a batch of AI labels (`source` = `ai:<name>`); user keywords are never touched.
#[tauri::command]
pub fn catalog_apply_labels(app: AppHandle, st: State<CatalogState>, labels: Vec<AssetLabels>, source: String, replace: bool) -> Result<usize> {
    let n = keywords::apply_labels(st.db.lock().unwrap().conn(), &labels, &source, replace).map_err(err)?;
    let _ = app.emit("catalog-keywords", labels.iter().map(|l| l.id).collect::<Vec<_>>());
    Ok(n)
}

/// Remove AI labels (every source starting with `prefix`, e.g. `ai:`) from these assets.
#[tauri::command]
pub fn catalog_clear_labels(app: AppHandle, st: State<CatalogState>, ids: Vec<i64>, prefix: String) -> Result<usize> {
    let n = keywords::clear_source(st.db.lock().unwrap().conn(), &ids, &prefix).map_err(err)?;
    let _ = app.emit("catalog-keywords", &ids);
    Ok(n)
}

/// The Anthropic API key for labelling: `ANTHROPIC_API_KEY` wins, else the key file in the app
/// data dir (Finder launches never see shell env). Returned to this local, personal frontend,
/// which calls the API itself — only when the user presses Label.
#[tauri::command]
pub fn ai_key(app: AppHandle) -> Option<String> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY")
        && !k.trim().is_empty()
    {
        return Some(k.trim().to_string());
    }
    let path = app.path().app_data_dir().ok()?.join("anthropic_key");
    let k = std::fs::read_to_string(path).ok()?;
    let k = k.trim();
    (!k.is_empty()).then(|| k.to_string())
}

/// Save (or, with empty input, clear) the key file. Owner-readable only.
#[tauri::command]
pub fn ai_key_save(app: AppHandle, key: String) -> Result<bool> {
    let dir = app.path().app_data_dir().map_err(err)?;
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join("anthropic_key");
    let k = key.trim();
    if k.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(false);
    }
    std::fs::write(&path, k).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

// ---------- print ----------

#[derive(Deserialize)]
pub struct PrintRequest {
    pub ids: Vec<i64>,
    /// Preset file path (from `list_presets`) or name.
    pub preset: PathBuf,
    pub jpeg: bool,
    pub exr: bool,
    /// Also copy the camera JPEG of photos that have one.
    #[serde(default)]
    pub camera_jpeg: bool,
}

// ---------- export queue ----------

#[tauri::command]
pub fn catalog_queue(st: State<CatalogState>) -> Result<Vec<i64>> {
    st.db.lock().unwrap().export_queue().map_err(err)
}

#[tauri::command]
pub fn catalog_queue_add(app: AppHandle, st: State<CatalogState>, ids: Vec<i64>) -> Result<usize> {
    let n = st.db.lock().unwrap().export_queue_add(&ids).map_err(err)?;
    let _ = app.emit("queue-changed", ());
    Ok(n)
}

#[tauri::command]
pub fn catalog_queue_remove(app: AppHandle, st: State<CatalogState>, ids: Vec<i64>) -> Result<()> {
    st.db.lock().unwrap().export_queue_remove(&ids).map_err(err)?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
pub fn catalog_queue_clear(app: AppHandle, st: State<CatalogState>) -> Result<()> {
    st.db.lock().unwrap().export_queue_clear().map_err(err)?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[derive(Deserialize)]
pub struct ExportRequest {
    pub ids: Vec<i64>,
    /// Look to print with; `null` exports no render (camera JPEGs only).
    pub look: Option<PathBuf>,
    pub jpeg: bool,
    pub exr: bool,
    /// Also copy the camera JPEG of photos that have one.
    pub camera_jpeg: bool,
    /// Folder to export into; the configured render root when absent.
    pub destination: Option<PathBuf>,
    /// Clear the queue when the run finishes.
    pub clear_queue: bool,
}

/// Run one export: renders and/or camera-JPEG copies. Progress: `job-event`.
#[tauri::command]
pub fn catalog_export(app: AppHandle, state: State<AppState>, st: State<CatalogState>, req: ExportRequest) -> Result<()> {
    if req.ids.is_empty() {
        return Err("nothing to export".into());
    }
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("busy with another job".into());
    }
    let cfg = state.config.lock().unwrap().clone();
    let cancel = Cancel::new();
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    let path = st.path.clone();
    std::thread::spawn(move || {
        let h = app.clone();
        let sink: spektro_core::job::EventSink = Arc::new(move |e: JobEvent| {
            let _ = h.emit("job-event", &e);
        });
        let opts = spektro_core::catalog::print::ExportOptions {
            look: req.look.clone(),
            jpeg: req.jpeg,
            exr: req.exr,
            camera_jpeg: req.camera_jpeg,
            destination: req.destination.clone(),
        };
        let result = Catalog::open(&path).and_then(|c| {
            let r = spektro_core::catalog::print::export_assets(&c, &cfg, &req.ids, &opts, sink.clone(), &cancel);
            if r.is_ok() && req.clear_queue {
                let _ = c.export_queue_remove(&req.ids);
            }
            r
        });
        let st = app.state::<AppState>();
        st.busy.store(false, Ordering::SeqCst);
        *st.cancel.lock().unwrap() = None;
        match result {
            Ok(r) => {
                allow_render_roots(&app);
                let _ = app.emit("catalog-changed", ());
                let _ = app.emit("queue-changed", ());
                let _ = app.emit(
                    "job-done",
                    serde_json::json!({ "manifest": null, "rendered": r.rendered, "copied": r.copied, "render_failed": r.failed, "skipped": r.skipped.len() }),
                );
            }
            Err(e) => {
                let _ = app.emit("job-error", format!("{e:#}"));
            }
        }
    });
    Ok(())
}

/// Render assets through a preset into the render root. Progress: `job-event`; end: `job-done`.
#[tauri::command]
pub fn catalog_print(app: AppHandle, state: State<AppState>, st: State<CatalogState>, req: PrintRequest) -> Result<()> {
    if req.ids.is_empty() {
        return Err("nothing selected".into());
    }
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("busy with another job".into());
    }
    let cfg = state.config.lock().unwrap().clone();
    let cancel = Cancel::new();
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    let path = st.path.clone();
    std::thread::spawn(move || {
        let h = app.clone();
        let sink: spektro_core::job::EventSink = Arc::new(move |e: JobEvent| {
            let _ = h.emit("job-event", &e);
        });
        let mut outputs = cfg.outputs.clone();
        outputs.jpeg = req.jpeg;
        outputs.exr = req.exr;
        let opts = spektro_core::catalog::print::ExportOptions {
            look: Some(req.preset.clone()),
            jpeg: outputs.jpeg,
            exr: outputs.exr,
            camera_jpeg: req.camera_jpeg,
            destination: None,
        };
        let result = Catalog::open(&path).and_then(|c| spektro_core::catalog::print::export_assets(&c, &cfg, &req.ids, &opts, sink.clone(), &cancel));
        let st = app.state::<AppState>();
        st.busy.store(false, Ordering::SeqCst);
        *st.cancel.lock().unwrap() = None;
        match result {
            Ok(r) => {
                allow_render_roots(&app);
                let _ = app.emit("catalog-changed", ());
                let _ = app.emit(
                    "job-done",
                    serde_json::json!({ "manifest": null, "rendered": r.rendered, "copied": r.copied, "render_failed": r.failed, "skipped": r.skipped.len() }),
                );
            }
            Err(e) => {
                let _ = app.emit("job-error", format!("{e:#}"));
            }
        }
    });
    Ok(())
}
