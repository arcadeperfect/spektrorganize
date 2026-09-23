//! Tauri commands: thin wrappers over spektro-core, plus app state and progress events.
//! Catalog (library) commands live in `catalog.rs`.

mod catalog;
mod looks;

use serde::{Deserialize, Serialize};
use spektro_core::catalog::import_dupes::ImportDupes;
use spektro_core::config::{Config, Outputs};
use spektro_core::job::{Cancel, JobEvent};
use spektro_core::manifest::Manifest;
use spektro_core::plan::{DestStatus, Plan, TreeNode};
use spektro_core::scan::{GroupId, Kind, PreviewSource, Scan, ScanProgress, SourceInfo};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    config_path: PathBuf,
    cache_dir: PathBuf,
    config: Mutex<Config>,
    scan: Mutex<Option<Arc<Scan>>>,
    excluded: Mutex<HashSet<u32>>,
    dupes: Mutex<ImportDupes>,
    /// What this import is about, typed on the Layout screen.
    description: Mutex<Option<String>>,
    plan: Mutex<Option<Arc<Plan>>>,
    cancel: Mutex<Option<Cancel>>,
    busy: AtomicBool,
}

type Result<T> = std::result::Result<T, String>;

pub(crate) fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn default_config_path() -> PathBuf {
    spektro_core::config::dirs_home().join(".config/spektrorganize/config.toml")
}

// ---------- views sent to the frontend ----------

#[derive(Serialize, Clone)]
pub struct GroupView {
    pub id: u32,
    pub kind: Kind,
    pub name: String,
    pub rel: String,
    pub bytes: u64,
    pub attachments: Vec<String>,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub iso: Option<u32>,
    pub lens: Option<String>,
    pub preview: PreviewSource,
    pub excluded: bool,
    /// This group is a copy of another group in the same scan (its id).
    pub copy_of: Option<u32>,
    /// The catalog already holds these bytes, at this path.
    pub known_at: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct ScanView {
    pub source: SourceInfo,
    pub groups: Vec<GroupView>,
    pub files: usize,
    pub bytes: u64,
    pub errors: Vec<(String, String)>,
    /// Copies found inside the scan, and photos the catalog already holds.
    pub copies: usize,
    pub already_held: usize,
    /// Numbered runs of images, for collapsing the review.
    pub sequences: Vec<spektro_core::sequence::SeqGroup>,
}

fn scan_view(scan: &Scan, excluded: &HashSet<u32>, dupes: &ImportDupes) -> ScanView {
    let groups = scan
        .groups
        .iter()
        .map(|g| {
            let p = scan.file(g.primary);
            GroupView {
                id: g.id.0,
                kind: g.kind,
                name: p.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                rel: p.rel.to_string_lossy().to_string(),
                bytes: scan.group_files(g).map(|f| f.size).sum(),
                attachments: g.attachments.iter().map(|id| scan.file(*id).path.file_name().unwrap_or_default().to_string_lossy().to_string()).collect(),
                captured_at: g.meta.captured_at.map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string()),
                camera: g.meta.camera.clone(),
                iso: g.meta.iso,
                lens: g.meta.lens.clone(),
                preview: g.preview,
                excluded: excluded.contains(&g.id.0),
                copy_of: dupes.copies.get(&g.id.0).copied(),
                known_at: dupes.known.get(&g.id.0).cloned(),
            }
        })
        .collect();
    ScanView {
        source: scan.source.clone(),
        groups,
        files: scan.files.len(),
        bytes: scan.total_bytes(),
        errors: scan.errors.iter().map(|(p, e)| (p.to_string_lossy().to_string(), e.clone())).collect(),
        copies: dupes.copies.len(),
        already_held: dupes.known.len(),
        // Four numbered files in a row is a sequence; three holiday snaps are not.
        sequences: spektro_core::sequence::detect(scan, 4),
    }
}

#[derive(Serialize)]
pub struct PlanView {
    pub tree: Vec<TreeNode>,
    pub files: usize,
    pub bytes: u64,
    pub bytes_to_copy: u64,
    pub renders: usize,
    pub collisions: usize,
    pub existing: usize,
}

#[derive(Serialize)]
pub struct ManifestSummary {
    pub path: PathBuf,
    pub created_at: String,
    pub source_label: String,
    pub entries: usize,
    pub raws: usize,
    pub rendered: usize,
    pub preset: Option<String>,
}

#[derive(Serialize)]
pub struct Info {
    pub config_path: PathBuf,
    pub cache_dir: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub libraw_version: String,
    pub spektrafilm_rev: String,
}

// ---------- commands ----------

#[tauri::command]
fn info(state: State<AppState>) -> Info {
    let cfg = state.config.lock().unwrap();
    Info {
        config_path: state.config_path.clone(),
        cache_dir: state.cache_dir.clone(),
        data_dir: spektro_core::film::find_data_dir(cfg.data_dir.as_deref()).ok(),
        libraw_version: spektro_core::decode::libraw_version(),
        spektrafilm_rev: spektro_core::film::SPEKTRAFILM_REV.to_string(),
    }
}

#[tauri::command]
fn list_sources() -> Vec<SourceInfo> {
    spektro_core::scan::list_sources()
}

#[tauri::command]
fn get_config(state: State<AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

/// The stock templates, for the "reset" button in Layout.
#[tauri::command]
fn default_templates() -> spektro_core::config::Templates {
    spektro_core::config::Templates::default()
}

#[tauri::command]
fn set_config(state: State<AppState>, config: Config) -> Result<()> {
    config.save(&state.config_path).map_err(err)?;
    *state.config.lock().unwrap() = config;
    *state.plan.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
fn validate_template(template: String) -> Result<Vec<String>> {
    spektro_core::template::Template::parse(&template)
        .map(|t| t.tokens().into_iter().map(String::from).collect())
        .map_err(err)
}

#[tauri::command]
fn list_presets() -> Vec<spektro_core::film::PresetSummary> {
    spektro_core::film::list_presets()
}

#[tauri::command]
fn start_scan(app: AppHandle, state: State<AppState>, path: String) -> Result<()> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("busy".into());
    }
    *state.scan.lock().unwrap() = None;
    *state.plan.lock().unwrap() = None;
    state.excluded.lock().unwrap().clear();
    *state.dupes.lock().unwrap() = ImportDupes::default();
    let root = PathBuf::from(path);
    std::thread::spawn(move || {
        let h = app.clone();
        let result = spektro_core::scan::scan(&root, &spektro_core::decode::read_meta, |p| {
            let _ = match p {
                ScanProgress::Walking { files } => h.emit("scan-progress", serde_json::json!({ "phase": "walking", "files": files })),
                ScanProgress::ReadingMetadata { done, total } => h.emit("scan-progress", serde_json::json!({ "phase": "metadata", "done": done, "total": total })),
            };
        });
        let st = app.state::<AppState>();
        match result {
            Ok(scan) => {
                // Copies within the card, and photos the library already holds. Only files
                // that share a name and size with something are read, so this is usually free.
                let _ = app.emit("scan-progress", serde_json::json!({ "phase": "duplicates", "done": 0, "total": 0 }));
                let dupes = {
                    let cat = app.state::<catalog::CatalogState>();
                    let db = cat.db.lock().unwrap();
                    spektro_core::catalog::import_dupes::find(&scan, Some(&db), |done, total| {
                        let _ = app.emit("scan-progress", serde_json::json!({ "phase": "duplicates", "done": done, "total": total }));
                    })
                    .unwrap_or_default()
                };
                {
                    let mut ex = st.excluded.lock().unwrap();
                    for id in dupes.to_exclude() {
                        ex.insert(id);
                    }
                }
                let view = {
                    let ex = st.excluded.lock().unwrap();
                    scan_view(&scan, &ex, &dupes)
                };
                *st.scan.lock().unwrap() = Some(Arc::new(scan));
                *st.dupes.lock().unwrap() = dupes;
                st.busy.store(false, Ordering::SeqCst);
                let _ = app.emit("scan-done", view);
            }
            Err(e) => {
                st.busy.store(false, Ordering::SeqCst);
                let _ = app.emit("scan-error", e.to_string());
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn get_scan(state: State<AppState>) -> Option<ScanView> {
    let scan = state.scan.lock().unwrap().clone()?;
    Some(scan_view(&scan, &state.excluded.lock().unwrap(), &state.dupes.lock().unwrap()))
}

/// What this import is, in the photographer's words. Kept on every photo it brings in.
#[tauri::command]
fn set_import_description(state: State<AppState>, text: String) {
    let text = text.trim().to_string();
    *state.description.lock().unwrap() = (!text.is_empty()).then_some(text);
}

#[tauri::command]
fn get_import_description(state: State<AppState>) -> Option<String> {
    state.description.lock().unwrap().clone()
}

#[tauri::command]
fn set_excluded(state: State<AppState>, ids: Vec<u32>, excluded: bool) {
    let mut ex = state.excluded.lock().unwrap();
    for id in ids {
        if excluded {
            ex.insert(id);
        } else {
            ex.remove(&id);
        }
    }
    *state.plan.lock().unwrap() = None;
}

/// Path of a thumbnail JPEG for a group (generated on demand, cached).
#[tauri::command]
async fn thumbnail(state: State<'_, AppState>, group: u32) -> Result<Option<String>> {
    let scan = state.scan.lock().unwrap().clone().ok_or("no scan")?;
    let cache = state.cache_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let g = scan.groups.get(group as usize).ok_or("no such group")?;
        spektro_core::preview::thumbnail(&scan, g, &cache, 512).map(|p| p.map(|p| p.to_string_lossy().to_string())).map_err(err)
    })
    .await
    .map_err(err)?
}

fn build_plan(state: &AppState) -> Result<Arc<Plan>> {
    let scan = state.scan.lock().unwrap().clone().ok_or("no scan")?;
    let excluded = state.excluded.lock().unwrap().clone();
    let cfg = state.config.lock().unwrap().clone();
    let included: Vec<GroupId> = scan.groups.iter().map(|g| g.id).filter(|g| !excluded.contains(&g.0)).collect();
    let plan = spektro_core::plan::plan(&scan, &included, &cfg, chrono::Local::now().naive_local()).map_err(err)?;
    let plan = Arc::new(plan);
    *state.plan.lock().unwrap() = Some(plan.clone());
    Ok(plan)
}

#[tauri::command]
fn make_plan(state: State<AppState>) -> Result<PlanView> {
    let plan = build_plan(&state)?;
    let cfg = state.config.lock().unwrap().clone();
    let tree = spektro_core::plan::tree(&plan, &cfg);
    Ok(PlanView {
        files: plan.files.len(),
        bytes: plan.total_bytes(),
        bytes_to_copy: plan.bytes_to_copy(cfg.skip_existing),
        renders: plan.renders.len(),
        collisions: plan.files.iter().filter(|f| f.status == DestStatus::Collision).count(),
        existing: plan.files.iter().filter(|f| matches!(f.status, DestStatus::ExistsSameSize | DestStatus::ExistsDifferent)).count(),
        tree,
    })
}

#[tauri::command]
fn start_import(app: AppHandle, state: State<AppState>, render: bool) -> Result<()> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("busy".into());
    }
    let scan = match state.scan.lock().unwrap().clone() {
        Some(s) => s,
        None => {
            state.busy.store(false, Ordering::SeqCst);
            return Err("no scan".into());
        }
    };
    let plan = match build_plan(&state) {
        Ok(p) => p,
        Err(e) => {
            state.busy.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };
    let cfg = state.config.lock().unwrap().clone();
    let cancel = Cancel::new();
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    std::thread::spawn(move || {
        let h = app.clone();
        let sink: spektro_core::job::EventSink = Arc::new(move |e: JobEvent| {
            let _ = h.emit("job-event", &e);
        });
        let hook_app = app.clone();
        let after_copy = move |path: &Path, m: &Manifest| catalog::index_manifest(&hook_app, path, m);
        let description = app.state::<AppState>().description.lock().unwrap().clone();
        let result =
            spektro_core::job::run_import(&scan, &plan, &cfg, render, sink, &cancel, Some(&after_copy), description.as_deref());
        if let Ok(r) = &result
            && r.rendered > 0
        {
            catalog::index_manifest(&app, &r.manifest_path, &r.manifest);
        }
        let st = app.state::<AppState>();
        st.busy.store(false, Ordering::SeqCst);
        *st.cancel.lock().unwrap() = None;
        match result {
            Ok(r) => {
                let _ = app.emit(
                    "job-done",
                    serde_json::json!({
                        "manifest": r.manifest_path,
                        "copied": r.copied, "skipped": r.skipped, "failed": r.failed,
                        "rendered": r.rendered, "render_failed": r.render_failed,
                    }),
                );
            }
            Err(e) => {
                let _ = app.emit("job-error", e.to_string());
            }
        }
    });
    Ok(())
}

#[derive(Deserialize)]
pub struct RenderRequest {
    pub manifest: PathBuf,
    pub exr: bool,
    pub jpeg: bool,
    pub render_root: Option<PathBuf>,
    pub only_missing: bool,
}

#[tauri::command]
fn start_render_manifest(app: AppHandle, state: State<AppState>, req: RenderRequest) -> Result<()> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("busy".into());
    }
    let cancel = Cancel::new();
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    std::thread::spawn(move || {
        let h = app.clone();
        let sink: spektro_core::job::EventSink = Arc::new(move |e: JobEvent| {
            let _ = h.emit("job-event", &e);
        });
        let outputs = Manifest::read(&req.manifest).ok().map(|m| {
            let mut o: Outputs = m.config.outputs.clone();
            o.exr = req.exr;
            o.jpeg = req.jpeg;
            o
        });
        let result = spektro_core::job::render_manifest(&req.manifest, outputs, req.render_root.clone(), req.only_missing, sink, &cancel);
        let st = app.state::<AppState>();
        st.busy.store(false, Ordering::SeqCst);
        *st.cancel.lock().unwrap() = None;
        if let Ok(m) = &result {
            catalog::index_manifest(&app, &req.manifest, m);
        }
        match result {
            Ok(_) => {
                let _ = app.emit("job-done", serde_json::json!({ "manifest": req.manifest }));
            }
            Err(e) => {
                let _ = app.emit("job-error", e.to_string());
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn cancel_job(state: State<AppState>) {
    if let Some(c) = state.cancel.lock().unwrap().as_ref() {
        c.cancel();
    }
}

#[tauri::command]
fn is_busy(state: State<AppState>) -> bool {
    state.busy.load(Ordering::SeqCst)
}

#[tauri::command]
fn list_manifests(state: State<AppState>) -> Vec<ManifestSummary> {
    let root = state.config.lock().unwrap().archive_root.clone();
    spektro_core::manifest::list_manifests(&root)
        .into_iter()
        .filter_map(|path| {
            let m = Manifest::read(&path).ok()?;
            Some(ManifestSummary {
                created_at: m.created_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
                source_label: m.source_label.clone(),
                entries: m.entries.len(),
                raws: m.entries.iter().filter(|e| e.kind == Kind::Raw && e.is_primary).count(),
                rendered: m.entries.iter().filter(|e| !e.outputs.is_empty()).count(),
                preset: m.preset.as_ref().map(|p| p.name.clone()),
                path,
            })
        })
        .collect()
}

#[tauri::command]
fn reveal(path: String) -> Result<()> {
    let p = Path::new(&path);
    let target = if p.exists() { p.to_path_buf() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() };
    std::process::Command::new("open").arg("-R").arg(&target).spawn().map_err(err)?;
    Ok(())
}

/// Open a file with its default app (a render, a RAW in the photo editor).
#[tauri::command]
fn open_path(path: String) -> Result<()> {
    std::process::Command::new("open").arg(&path).spawn().map_err(err)?;
    Ok(())
}

/// One chunk of a clip, by catalog file id. Ranges are served as 206 so the player can seek
/// without pulling the whole file.
fn serve_clip(app: &AppHandle, path: &str, range: Option<&str>) -> tauri::http::Response<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    use tauri::http::{Response, StatusCode, header};

    let fail = |code: StatusCode| Response::builder().status(code).body(Vec::new()).unwrap();
    let Ok(id) = path.trim_matches('/').parse::<i64>() else { return fail(StatusCode::BAD_REQUEST) };

    let file = {
        let st = app.state::<catalog::CatalogState>();
        let db = st.db.lock().unwrap();
        match db.file_path(id) {
            Ok(Some(p)) => p,
            _ => return fail(StatusCode::NOT_FOUND),
        }
    };
    let Ok(mut f) = std::fs::File::open(&file) else { return fail(StatusCode::NOT_FOUND) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mime = match file.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mov") => "video/quicktime",
        Some("m4v") => "video/x-m4v",
        Some("avi") => "video/x-msvideo",
        Some("mts") | Some("m2ts") => "video/mp2t",
        _ => "video/mp4",
    };

    // "bytes=start-end", either end optional. Anything else is treated as the whole file.
    let (start, end) = match range.and_then(|r| r.strip_prefix("bytes=")).map(|r| {
        let (a, b) = r.split_once('-').unwrap_or((r, ""));
        (a.parse::<u64>().unwrap_or(0), b.parse::<u64>().ok())
    }) {
        Some((s, e)) => {
            // A couple of megabytes at a time: enough to keep playing, small enough to seek.
            const CHUNK: u64 = 2 * 1024 * 1024;
            let end = e.unwrap_or_else(|| (s + CHUNK - 1).min(len.saturating_sub(1)));
            (s.min(len), end.min(len.saturating_sub(1)))
        }
        None => (0, len.saturating_sub(1)),
    };
    if len == 0 || start > end {
        return fail(StatusCode::RANGE_NOT_SATISFIABLE);
    }

    let mut buf = vec![0u8; (end - start + 1) as usize];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let partial = range.is_some();
    Response::builder()
        .status(if partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, buf.len().to_string())
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
        .header("Access-Control-Allow-Origin", "*")
        .body(buf)
        .unwrap()
}

pub fn run() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Clips are wherever the user keeps them, which the webview cannot read. This serves
        // them by catalog file id, with byte ranges so the player can seek, and it will only
        // ever open a path the catalog already knows about.
        // Rendered frames go to the viewport as bytes: width × height × RGBA, no JPEG, no
        // base64. The viewport uploads them straight into a texture.
        .register_uri_scheme_protocol("frame", |ctx, request| {
            use tauri::http::{Response, StatusCode, header};
            let fail = |code: StatusCode| Response::builder().status(code).body(Vec::new()).unwrap();
            let Ok(token) = request.uri().path().trim_matches('/').parse::<u64>() else { return fail(StatusCode::BAD_REQUEST) };
            let Some(frame) = ctx.app_handle().state::<looks::LookState>().take(token) else { return fail(StatusCode::NOT_FOUND) };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("X-Width", frame.width.to_string())
                .header("X-Height", frame.height.to_string())
                .header("Access-Control-Allow-Origin", "*")
                .header("Access-Control-Expose-Headers", "X-Width, X-Height")
                .body(frame.rgba.clone())
                .unwrap()
        })
        .register_asynchronous_uri_scheme_protocol("clip", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let uri = request.uri().clone();
            let range = request.headers().get(tauri::http::header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_owned);
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(serve_clip(&app, uri.path(), range.as_deref()));
            });
        })
        .setup(|app| {
            let config_path = default_config_path();
            let config = if config_path.exists() { Config::load(&config_path).unwrap_or_default() } else { Config::default() };
            let cache_dir = app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir().join("spektrorganize"));
            std::fs::create_dir_all(&cache_dir).ok();
            app.manage(AppState {
                config_path,
                cache_dir,
                config: Mutex::new(config),
                scan: Mutex::new(None),
                excluded: Mutex::new(HashSet::new()),
                dupes: Mutex::new(ImportDupes::default()),
                description: Mutex::new(None),
                plan: Mutex::new(None),
                cancel: Mutex::new(None),
                busy: AtomicBool::new(false),
            });
            let catalog = catalog::CatalogState::new(app.handle()).map_err(|e| format!("opening the catalog: {e:#}"))?;
            app.manage(catalog);
            app.manage(looks::LookState::default());
            catalog::warm_up(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            info,
            list_sources,
            get_config,
            set_import_description,
            get_import_description,
            default_templates,
            set_config,
            validate_template,
            list_presets,
            start_scan,
            get_scan,
            set_excluded,
            thumbnail,
            make_plan,
            start_import,
            start_render_manifest,
            cancel_job,
            is_busy,
            list_manifests,
            reveal,
            open_path,
            catalog::catalog_info,
            catalog::catalog_facets,
            catalog::catalog_list,
            catalog::catalog_ids,
            catalog::catalog_asset,
            catalog::catalog_add_folder,
            catalog::catalog_adopt,
            catalog::catalog_rescan,
            catalog::catalog_cancel_index,
            catalog::catalog_relocate_root,
            catalog::catalog_remove_root,
            catalog::catalog_request_thumbs,
            catalog::catalog_preview,
            catalog::catalog_preview_px,
            catalog::catalog_render_preview,
            catalog::catalog_find_duplicates,
            catalog::catalog_purge_duplicates,
            catalog::catalog_doomed_files,
            catalog::catalog_purge_assets,
            catalog::catalog_collections,
            catalog::catalog_save_collection,
            catalog::catalog_delete_collection,
            catalog::catalog_get_setting,
            catalog::catalog_set_setting,
            catalog::catalog_remake_rotated_thumbs,
            catalog::catalog_thumbs_from_prints,
            catalog::catalog_set_thumbs_from_prints,
            catalog::catalog_thumb_data,
            catalog::catalog_tag,
            catalog::catalog_set_rating,
            catalog::catalog_set_flag,
            catalog::catalog_label_info,
            catalog::catalog_apply_labels,
            catalog::catalog_clear_labels,
            catalog::catalog_print,
            catalog::catalog_queue,
            catalog::catalog_queue_add,
            catalog::catalog_queue_remove,
            catalog::catalog_queue_clear,
            catalog::catalog_export,
            catalog::ai_key,
            catalog::ai_key_save,
            looks::looks_meta,
            looks::look_load,
            looks::look_save,
            looks::look_delete,
            looks::look_import_file,
            looks::look_import_url,
            looks::look_import_text,
            looks::look_resolve,
            looks::look_neutralize,
            looks::look_preview,
            looks::look_preview_before,
            looks::develop_preview,
            looks::asset_input,
            looks::asset_raw_get,
            looks::asset_raw_set,
            looks::look_frame,
            looks::develop_frame,
            looks::look_frame_before,
            looks::video_render,
            looks::look_export_cube,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
