//! Looks (film presets): list / load / save / import, the editor's live
//! preview of a catalog photo, and per-photo RAW settings.

use crate::catalog::CatalogState;
use crate::{AppState, Result, err};
use serde::Serialize;
use spektro_core::decode::RawSettings;
use spektro_core::film::{Preset, preset_file_name, user_preset_dir};
use spektro_core::look::{self, LookMeta, PreviewEngine};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub struct LookState {
    engine: Mutex<Option<PreviewEngine>>,
}

fn data_dir(state: &AppState) -> Result<PathBuf> {
    let cfg = state.config.lock().unwrap();
    spektro_core::film::find_data_dir(cfg.data_dir.as_deref()).map_err(err)
}

#[tauri::command]
pub fn looks_meta(state: State<AppState>) -> Result<LookMeta> {
    look::meta(&data_dir(&state)?).map_err(err)
}

#[tauri::command]
pub fn look_load(path: String) -> Result<Preset> {
    Preset::load(Path::new(&path)).map_err(err)
}

/// Save a look. Without `path` (or when `path` is outside the user folder,
/// e.g. a bundled look) it goes to the user preset folder under a name
/// derived from the look's name, never overwriting another file.
#[tauri::command]
pub fn look_save(preset: Preset, path: Option<String>) -> Result<String> {
    let dir = user_preset_dir();
    let target = match path.map(PathBuf::from) {
        Some(p) if p.starts_with(&dir) => p,
        _ => {
            let base = preset_file_name(&preset.name);
            let mut candidate = dir.join(&base);
            let stem = base.trim_end_matches(".json").to_string();
            let mut n = 2;
            while candidate.exists() {
                candidate = dir.join(format!("{stem}-{n}.json"));
                n += 1;
            }
            candidate
        }
    };
    preset.save(&target).map_err(err)?;
    Ok(target.display().to_string())
}

/// Move a user look to the Trash (bundled looks are read-only).
#[tauri::command]
pub fn look_delete(path: String) -> Result<()> {
    let p = PathBuf::from(&path);
    if !p.starts_with(user_preset_dir()) {
        return Err("only your own looks can be deleted".into());
    }
    trash::delete(&p).map_err(err)
}

#[derive(Serialize)]
pub struct Imported {
    preset: Preset,
    warnings: Vec<String>,
}

fn import_bytes(state: &AppState, name: &str, bytes: &[u8]) -> Result<Imported> {
    let data_dir = data_dir(state).ok();
    // Forum posts often attach a zip holding the preset file.
    if bytes.starts_with(b"PK\x03\x04") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(err)?;
        let mut last_err = String::from("the zip holds no preset file");
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(err)?;
            let fname = f.name().to_string();
            if f.is_dir() || fname.starts_with("__MACOSX") || fname.rsplit('/').next().is_some_and(|n| n.starts_with('.')) {
                continue;
            }
            let mut inner = Vec::new();
            f.read_to_end(&mut inner).map_err(err)?;
            match Preset::import_bytes(&fname, &inner, data_dir.as_deref()) {
                Ok((preset, warnings)) => return Ok(Imported { preset, warnings }),
                Err(e) => last_err = format!("{fname}: {e}"),
            }
        }
        return Err(last_err);
    }
    let (preset, warnings) = Preset::import_bytes(name, bytes, data_dir.as_deref()).map_err(err)?;
    Ok(Imported { preset, warnings })
}

/// Import any spektrafilm flavour's preset file (not saved until the user saves).
#[tauri::command]
pub fn look_import_file(state: State<AppState>, path: String) -> Result<Imported> {
    let bytes = std::fs::read(&path).map_err(err)?;
    let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    import_bytes(&state, &name, &bytes)
}

/// Import from a URL the user pasted (a forum attachment, a gist…).
#[tauri::command]
pub async fn look_import_url(app: AppHandle, url: String) -> Result<Imported> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("enter an http(s) address".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut resp = ureq::get(&url).call().map_err(err)?;
        let bytes = resp.body_mut().with_config().limit(20 * 1024 * 1024).read_to_vec().map_err(err)?;
        let name = url.rsplit('/').next().unwrap_or("preset").split('?').next().unwrap_or("preset").to_string();
        import_bytes(&app.state::<AppState>(), &name, &bytes)
    })
    .await
    .map_err(err)?
}

/// Import pasted text (a Python GUI state, a vkdt line…).
#[tauri::command]
pub fn look_import_text(state: State<AppState>, text: String) -> Result<Imported> {
    import_bytes(&state, "pasted", text.as_bytes())
}

/// The full parameters a look renders with (stock defaults + overrides).
#[tauri::command]
pub fn look_resolve(state: State<AppState>, preset: Preset) -> Result<serde_json::Value> {
    look::resolve(&preset, &data_dir(&state)?).map_err(err)
}

/// Enlarger M/Y shifts that print midgray neutral with this look.
#[tauri::command]
pub async fn look_neutralize(app: AppHandle, preset: Preset) -> Result<(f32, f32)> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = data_dir(&app.state::<AppState>())?;
        look::neutralize_filters(&preset, &dir).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Live preview of catalog photo `id` through `preset` with `raw` settings,
/// as a JPEG data URL. Renders are serialised; a newer request simply waits.
#[tauri::command]
pub async fn look_preview(app: AppHandle, id: i64, preset: Preset, raw: RawSettings, max_px: u32) -> Result<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let raw_path = {
            let st = app.state::<CatalogState>();
            let db = st.db.lock().unwrap();
            spektro_core::catalog::print::raw_path(&db, id).map_err(err)??
        };
        let dir = data_dir(&app.state::<AppState>())?;
        let looks = app.state::<LookState>();
        let mut engine = looks.engine.lock().unwrap();
        let engine = engine.get_or_insert_with(PreviewEngine::default);
        let jpg = engine.render(&raw_path, &raw, &preset, &dir, max_px.clamp(256, 16384)).map_err(|e| format!("{e:#}"))?;
        use base64::Engine;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
    })
    .await
    .map_err(err)?
}

/// The develop stage's own output for a photo: decode + develop settings,
/// no film look.
#[tauri::command]
pub async fn develop_preview(app: AppHandle, id: i64, raw: RawSettings, max_px: u32) -> Result<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let raw_path = {
            let st = app.state::<CatalogState>();
            let db = st.db.lock().unwrap();
            spektro_core::catalog::print::raw_path(&db, id).map_err(err)??
        };
        let looks = app.state::<LookState>();
        let mut engine = looks.engine.lock().unwrap();
        let engine = engine.get_or_insert_with(PreviewEngine::default);
        let jpg = engine.render_developed(&raw_path, &raw, max_px.clamp(256, 16384)).map_err(|e| format!("{e:#}"))?;
        use base64::Engine;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
    })
    .await
    .map_err(err)?
}

/// The "before" image for the print editor: the developed photo at the look's exposure.
#[tauri::command]
pub async fn look_preview_before(app: AppHandle, id: i64, preset: Preset, raw: RawSettings, max_px: u32) -> Result<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let raw_path = {
            let st = app.state::<CatalogState>();
            let db = st.db.lock().unwrap();
            spektro_core::catalog::print::raw_path(&db, id).map_err(err)??
        };
        let dir = data_dir(&app.state::<AppState>())?;
        let looks = app.state::<LookState>();
        let mut engine = looks.engine.lock().unwrap();
        let engine = engine.get_or_insert_with(PreviewEngine::default);
        let jpg = engine.render_before(&raw_path, &raw, &preset, &dir, max_px.clamp(256, 16384)).map_err(|e| format!("{e:#}"))?;
        use base64::Engine;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
    })
    .await
    .map_err(err)?
}

/// What the film stage will be fed for this photo: a RAW to decode, or an
/// image file (camera JPEG) that has no decode stage.
#[derive(Serialize)]
pub struct InputInfo {
    is_raw: bool,
    file: String,
    error: Option<String>,
    /// Pixel size from the catalog, so the viewer knows what "full resolution" means.
    width: Option<i64>,
    height: Option<i64>,
}

#[tauri::command]
pub fn asset_input(st: State<CatalogState>, id: i64) -> Result<InputInfo> {
    let db = st.db.lock().unwrap();
    let (width, height) = db.dimensions(id).unwrap_or((None, None));
    match spektro_core::catalog::print::raw_path(&db, id).map_err(err)? {
        Ok(p) => Ok(InputInfo {
            is_raw: spektro_core::decode::is_raw_path(&p),
            file: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            error: None,
            width,
            height,
        }),
        Err(why) => Ok(InputInfo { is_raw: false, file: String::new(), error: Some(why), width, height }),
    }
}

#[tauri::command]
pub fn asset_raw_get(st: State<CatalogState>, id: i64) -> Result<RawSettings> {
    st.db.lock().unwrap().raw_settings(id).map_err(err)
}

#[tauri::command]
pub fn asset_raw_set(st: State<CatalogState>, ids: Vec<i64>, raw: RawSettings) -> Result<()> {
    st.db.lock().unwrap().set_raw_settings(&ids, &raw).map_err(err)
}
