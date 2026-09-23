//! spektrafilm film stage: presets, pipeline construction, rendering.

use crate::config::Outputs;
use crate::decode::LinearImage;
use crate::manifest::PresetRecord;
use serde::{Deserialize, Serialize};
use spektrafilm_core::params::RuntimeParams;
use spektrafilm_core::pipeline::Pipeline;
use spektrafilm_core::profile;
use spektrafilm_gpu::{ComputeBackend, select_backend};
use spektrafilm_math::image::ImageBuf;
use spektrafilm_math::precision::from_f32;
use std::path::{Path, PathBuf};

/// spektrafilm-rs engine this build links against: the local fork's branch
/// (port of upstream experimental 28bf883). Part of every preset hash, so
/// renders record which engine made them.
pub const SPEKTRAFILM_REV: &str = "spektro-experimental/28bf883";

/// Colourspace names the engine accepts (canonical names; aliases also resolve).
pub fn color_spaces() -> Vec<&'static str> {
    spektrafilm_math::colourspaces::names()
}

pub fn validate_color_space(name: &str) -> anyhow::Result<()> {
    spektrafilm_math::colourspaces::lookup(name).map(|_| ()).map_err(|e| anyhow::anyhow!(e))
}

/// A look: film + paper + a partial `RuntimeParams` JSON layered on the film's
/// stock defaults (upstream's per-stock halation / grain / coupler presets).
/// Only the fields a look changes need to be present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub film: String,
    /// Paper profile. Unused when the look's route scans the film directly.
    #[serde(default)]
    pub print: String,
    #[serde(default = "empty_object")]
    pub params: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

impl Preset {
    /// Load a native look file, or import any format the engine understands
    /// (Python GUI state, darktable, vkdt).
    pub fn load(path: &Path) -> anyhow::Result<Preset> {
        let text = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("reading preset {}: {e}", path.display()))?;
        if let Ok(p) = serde_json::from_str::<Preset>(&text)
            && !p.film.is_empty()
        {
            return Ok(p);
        }
        let (p, _warnings) = Preset::import(path, None)?;
        Ok(p)
    }

    /// Import a preset from another spektrafilm flavour, returning it with
    /// the importer's warnings (fields it could not map exactly).
    pub fn import(path: &Path, data_dir: Option<&Path>) -> anyhow::Result<(Preset, Vec<String>)> {
        let imported = spektrafilm_core::importers::import_file(path, data_dir).map_err(|e| anyhow::anyhow!(e))?;
        Preset::from_imported(imported, path)
    }

    /// Import from raw bytes (downloaded or pasted), `name_hint` for naming.
    pub fn import_bytes(name_hint: &str, bytes: &[u8], data_dir: Option<&Path>) -> anyhow::Result<(Preset, Vec<String>)> {
        let imported = spektrafilm_core::importers::import_bytes(name_hint, bytes, data_dir).map_err(|e| anyhow::anyhow!(e))?;
        Preset::from_imported(imported, Path::new(name_hint))
    }

    fn from_imported(i: spektrafilm_core::importers::ImportedPreset, source: &Path) -> anyhow::Result<(Preset, Vec<String>)> {
        let film = i.film.clone().ok_or_else(|| anyhow::anyhow!("preset names no film stock"))?;
        let name = i
            .name
            .clone()
            .unwrap_or_else(|| source.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| film.clone()));
        Ok((Preset { name, film, print: i.print.clone().unwrap_or_default(), params: i.params.clone() }, i.warnings))
    }

    pub fn from_record(r: &PresetRecord) -> anyhow::Result<Preset> {
        Ok(Preset { name: r.name.clone(), film: r.film.clone(), print: r.print.clone(), params: r.params.clone() })
    }

    /// Full engine parameters: the film's stock defaults with this look's
    /// overrides on top, plus the film and paper profiles to render with.
    pub fn resolve(&self, data_dir: &Path) -> anyhow::Result<(profile::Profile, profile::Profile, RuntimeParams)> {
        let load = |name: &str| profile::load_profile_by_name(data_dir, name).map_err(|e| anyhow::anyhow!("profile '{name}': {e}"));
        let film = load(&self.film)?;
        let params = spektrafilm_core::stock_presets::stock_defaults_with_overrides(&film, &self.params, data_dir)
            .map_err(|e| anyhow::anyhow!("look '{}': {e}", self.name))?;
        let print_name = if params.io.scan_film || self.print.is_empty() {
            film.info.target_print.clone().filter(|_| !params.io.scan_film).unwrap_or_else(|| self.film.clone())
        } else {
            self.print.clone()
        };
        let print = load(&print_name)?;
        Ok((film, print, params))
    }

    /// Stable hash over the preset content plus the engine revision.
    pub fn hash(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        let mut h = blake3::Hasher::new();
        h.update(&json);
        h.update(SPEKTRAFILM_REV.as_bytes());
        h.finalize().to_hex()[..16].to_string()
    }

    pub fn record(&self) -> PresetRecord {
        PresetRecord {
            name: self.name.clone(),
            film: self.film.clone(),
            print: self.print.clone(),
            params: self.params.clone(),
            hash: self.hash(),
            spektrafilm_rev: SPEKTRAFILM_REV.to_string(),
        }
    }

    /// Write as a native look file.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Preset reference meaning "no film look": develop the photo and colour-manage
/// it to the output space, nothing else (the engine's passthrough route).
pub const DEVELOPED_ONLY: &str = "__developed__";

/// The passthrough look behind [`DEVELOPED_ONLY`]. The film and paper are
/// irrelevant on that route, but a profile still has to load.
pub fn developed_preset(film: &str) -> Preset {
    Preset {
        name: "Developed".into(),
        film: film.to_string(),
        print: String::new(),
        params: serde_json::json!({
            "workflow": { "route": "input" },
            "camera": { "auto_exposure": false, "exposure_compensation_ev": 0.0 },
        }),
    }
}

/// Where new and imported looks are saved.
pub fn user_preset_dir() -> PathBuf {
    crate::config::dirs_home().join(".config/spektrorganize/presets")
}

/// A file name for a look: lowercase, alphanumerics and dashes.
pub fn preset_file_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    format!("{}.json", if out.is_empty() { "look".into() } else { out })
}

/// Locate the spektrafilm data directory: explicit config, `SPEKTRO_DATA_DIR`, next to the
/// executable (app bundle `Resources`), or the vendored copy in the source tree.
pub fn find_data_dir(configured: Option<&Path>) -> anyhow::Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = configured {
        candidates.push(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("SPEKTRO_DATA_DIR") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("../Resources/spektrafilm-data"));
        candidates.push(dir.join("spektrafilm-data"));
        candidates.push(dir.join("../../vendor/spektrafilm-data"));
        candidates.push(dir.join("../../../vendor/spektrafilm-data"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/spektrafilm-data"));
    for c in &candidates {
        if c.join("profiles").is_dir() && c.join("luts/spectral_upsampling/irradiance_xy_tc.npy").is_file() {
            return Ok(c.canonicalize().unwrap_or_else(|_| c.clone()));
        }
    }
    anyhow::bail!("spektrafilm data directory not found (tried {} locations); set data_dir in config", candidates.len())
}

/// Directories that may hold preset files, most user-specific first: the user config dir,
/// next to the executable (app bundle resources), and the source tree.
pub fn preset_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![crate::config::dirs_home().join(".config/spektrorganize/presets")];
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.join("../Resources/presets"));
        dirs.push(dir.join("presets"));
    }
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../presets"));
    dirs.into_iter().filter(|d| d.is_dir()).map(|d| d.canonicalize().unwrap_or(d)).collect()
}

/// Turn a configured preset reference into a file: absolute paths as-is, otherwise the file name
/// (or relative path) looked up in each of `preset_dirs()`.
pub fn resolve_preset(configured: &Path) -> anyhow::Result<PathBuf> {
    if configured.is_absolute() {
        if configured.is_file() {
            return Ok(configured.to_path_buf());
        }
        anyhow::bail!("preset file not found: {}", configured.display());
    }
    let name = configured.file_name().map(PathBuf::from).unwrap_or_else(|| configured.to_path_buf());
    let dirs = preset_dirs();
    for dir in &dirs {
        for candidate in [dir.join(configured), dir.join(&name)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir()
        && cwd.join(configured).is_file()
    {
        return Ok(cwd.join(configured));
    }
    anyhow::bail!(
        "preset '{}' not found in {}",
        configured.display(),
        dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetSummary {
    pub name: String,
    pub film: String,
    pub print: String,
    pub path: PathBuf,
}

/// All loadable presets across `preset_dirs()`, de-duplicated by file name (first dir wins).
pub fn list_presets() -> Vec<PresetSummary> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in preset_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json")).collect();
        paths.sort();
        for path in paths {
            let key = path.file_name().unwrap().to_os_string();
            if !seen.insert(key) {
                continue;
            }
            if let Ok(p) = Preset::load(&path) {
                out.push(PresetSummary { name: p.name, film: p.film, print: p.print, path });
            }
        }
    }
    out
}

/// Names of available profiles in a data dir, split into films and papers.
pub fn list_profiles(data_dir: &Path) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut films = Vec::new();
    let mut papers = Vec::new();
    for entry in std::fs::read_dir(data_dir.join("profiles"))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        match profile::load_profile(&path) {
            Ok(p) if p.is_paper() => papers.push(name),
            Ok(_) => films.push(name),
            Err(e) => tracing::warn!("profile {}: {e}", path.display()),
        }
    }
    films.sort();
    papers.sort();
    Ok((films, papers))
}

pub struct Rendered {
    pub exr: Option<ImageBuf>,
    pub jpeg: Option<ImageBuf>,
}

/// One preset compiled into up to two pipelines (EXR and JPEG output configs) sharing a backend.
pub struct Renderer {
    backend: Box<dyn ComputeBackend>,
    exr: Option<Pipeline>,
    jpeg: Option<Pipeline>,
    pub preset_hash: String,
}

impl Renderer {
    pub fn new(preset: &Preset, data_dir: &Path, outputs: &Outputs, input_color_space: &str) -> anyhow::Result<Renderer> {
        validate_color_space(input_color_space)?;
        if outputs.exr {
            validate_color_space(&outputs.exr_color_space)?;
        }
        if !outputs.exr && !outputs.jpeg {
            anyhow::bail!("no outputs selected");
        }
        let (film, print, mut base) = preset.resolve(data_dir)?;
        base.io.input_color_space = input_color_space.to_string();
        base.io.input_cctf_decoding = false;
        base.io.upscale_factor = 1.0;

        let mut jpeg_params = base.clone();
        jpeg_params.io.output_color_space = "sRGB".into();
        jpeg_params.io.output_cctf_encoding = true;

        let mut exr_params = base.clone();
        exr_params.io.output_color_space = outputs.exr_color_space.clone();
        exr_params.io.output_cctf_encoding = false;
        exr_params.io.output_gamut_compress.algorithm = "off".into();
        exr_params.scanner.white_correction = false;
        exr_params.scanner.black_correction = false;

        // Build once (spectral LUT + calibration), derive the second output config from it.
        let (first, second) = if outputs.jpeg { (jpeg_params, exr_params) } else { (exr_params, jpeg_params) };
        let first_pipe = Pipeline::new_with_spectral(film, print, first, data_dir).map_err(|e| anyhow::anyhow!("building pipeline: {e}"))?;
        let (jpeg, exr) = match (outputs.jpeg, outputs.exr) {
            (true, true) => {
                let e = first_pipe.clone().with_params(second);
                (Some(first_pipe), Some(e))
            }
            (true, false) => (Some(first_pipe), None),
            (false, true) => (None, Some(first_pipe)),
            (false, false) => unreachable!(),
        };
        Ok(Renderer { backend: select_backend(), exr, jpeg, preset_hash: preset.hash() })
    }

    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    pub fn render(&self, img: &LinearImage) -> Rendered {
        let to_buf = || ImageBuf::from_data(img.width, img.height, img.data.iter().map(|v| from_f32(*v)).collect());
        let exr = self.exr.as_ref().map(|p| p.process(to_buf(), self.backend.as_ref()));
        let jpeg = self.jpeg.as_ref().map(|p| p.process(to_buf(), self.backend.as_ref()));
        Rendered { exr, jpeg }
    }
}

#[cfg(test)]
mod flow_paths {
    use super::*;

    /// Every parameter path the Print panel binds must exist in the model, or a slider would
    /// write a field the pipeline never reads. Resolving a preset that overrides them all is
    /// the check: an unknown field is a deserialisation error.
    #[test]
    fn every_panel_path_is_a_real_parameter() {
        let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/spektrafilm-data");
        let params = serde_json::json!({
            "settings": { "rgb_to_raw_method": "hanatos2025", "use_cat16": true },
            "camera": { "film_format_mm": 56.0, "exposure_compensation_ev": 0.3, "auto_exposure": true, "color_filter": "none",
                        "diffusion_filter": { "active": true, "filter_family": "black_pro_mist", "strength": 0.5, "halo_warmth": 0.1 } },
            "enlarger": { "print_exposure": 1.1, "c_filter_neutral": 10.0, "m_filter_shift": 2.0, "y_filter_shift": -1.0,
                          "preflash_exposure": 0.02, "preflash_m_filter_shift": 1.0, "preflash_y_filter_shift": 1.0,
                          "diffusion_filter": { "active": true, "filter_family": "black_pro_mist", "strength": 0.5, "halo_warmth": 0.0 } },
            "film_render": { "chemistry": { "gamma_factor": 1.1 }, "base": { "scale": 1.0 },
                             "dir_couplers": { "active": true, "amount": 1.0, "diffusion_size_um": 20.0, "inhibition_samelayer": 1.0, "inhibition_interlayer": 1.0 },
                             "grain": { "active": true, "agx_particle_area_um2": 0.2, "blur": 0.5 },
                             "halation": { "active": true, "halation_amount": 1.0, "halation_spatial_scale": 1.0, "boost_ev": 0.5,
                                           "halation_strength": [0.05, 0.02, 0.0], "scatter_amount": 1.0 } },
            "print_render": { "chemistry": { "gamma_factor": 1.0 }, "base": { "scale": 1.0 }, "glare": { "active": false } },
            "scanner": { "white_correction": true, "black_correction": true, "white_level": 0.98, "black_level": 0.01 },
            "io": { "output_gamut_compress": { "algorithm": "cam16ucs" } }
        });
        let preset = Preset { name: "paths".into(), film: "kodak_portra_400".into(), print: "kodak_portra_endura".into(), params };
        let (_, _, p) = preset.resolve(&data_dir).expect("every path resolves");
        assert_eq!(p.camera.film_format_mm, 56.0);
        assert_eq!(p.enlarger.c_filter_neutral, 10.0);
        assert_eq!(p.film_render.halation.halation_strength, [0.05, 0.02, 0.0]);
        assert!(p.scanner.white_correction);
    }
}
