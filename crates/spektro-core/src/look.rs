//! Looks: the editor side of presets. Metadata for the controls, the full
//! parameters a look resolves to, fast previews, and print-filter balancing.
//!
//! A look is a [`Preset`]: film + paper + a partial parameter JSON layered on
//! the film's stock defaults. Previews decode the RAW at half size once per
//! (file, decode options) and keep the most recent few decodes; RAW
//! white-balance / exposure changes are applied to the cached decode, and the
//! film pipeline is rebuilt only when a construction-time parameter changes.

use crate::decode::{DevelopSettings, LinearImage, RawFile, RawSettings, WhiteBalance, apply_raw_adjustments};
use crate::film::Preset;
use serde::Serialize;
use spektrafilm_core::pipeline::Pipeline;
use spektrafilm_core::profile;
use spektrafilm_gpu::{ComputeBackend, select_backend};
use spektrafilm_math::image::ImageBuf;
use spektrafilm_math::precision::from_f32;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct StockInfo {
    pub name: String,
    /// Human name from the profile, when it has one.
    pub label: String,
    pub positive: bool,
    pub bw: bool,
    pub usage: String,
    pub target_print: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

/// Everything the look editor's pickers need.
#[derive(Debug, Clone, Serialize)]
pub struct LookMeta {
    pub films: Vec<StockInfo>,
    pub papers: Vec<StockInfo>,
    pub color_filters: Vec<Choice>,
    pub color_spaces: Vec<String>,
    pub upsamplers: Vec<String>,
    pub routes: Vec<String>,
    pub gamut_algorithms: Vec<String>,
}

pub fn meta(data_dir: &Path) -> anyhow::Result<LookMeta> {
    let mut films = Vec::new();
    let mut papers = Vec::new();
    for entry in std::fs::read_dir(data_dir.join("profiles"))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let Ok(p) = profile::load_profile(&path) else { continue };
        let info = StockInfo {
            label: p.info.name.clone().unwrap_or_else(|| name.replace('_', " ")),
            name,
            positive: p.is_positive(),
            bw: p.is_bw(),
            usage: p.info.usage.clone(),
            target_print: p.info.target_print.clone(),
        };
        if p.is_paper() { papers.push(info) } else { films.push(info) }
    }
    films.sort_by(|a, b| a.name.cmp(&b.name));
    papers.sort_by(|a, b| a.name.cmp(&b.name));
    let mut color_filters = vec![Choice { value: "none".into(), label: "none".into() }];
    color_filters.extend(
        spektrafilm_core::color_filters::color_filters().iter().map(|f| Choice { value: f.key.clone(), label: f.label.clone() }),
    );
    let mut upsamplers = vec!["hanatos2025".to_string()];
    if let Ok(reg) = spektrafilm_core::spectral_service::lut_registry(data_dir) {
        let mut r: Vec<String> = reg.values().filter(|d| d.kind == "reflectance").map(|d| d.identifier.clone()).collect();
        r.sort();
        upsamplers.extend(r);
    }
    Ok(LookMeta {
        films,
        papers,
        color_filters,
        color_spaces: spektrafilm_math::colourspaces::names().into_iter().map(String::from).collect(),
        upsamplers,
        routes: spektrafilm_core::params::ROUTES.iter().map(|s| s.to_string()).collect(),
        gamut_algorithms: ["off", "oklch", "oklrab", "cam16ucs", "jzazbz", "aces_rgc"].iter().map(|s| s.to_string()).collect(),
    })
}

/// The full parameters a look renders with (stock defaults + its overrides).
pub fn resolve(preset: &Preset, data_dir: &Path) -> anyhow::Result<serde_json::Value> {
    let (_, _, params) = preset.resolve(data_dir)?;
    Ok(serde_json::to_value(params)?)
}

/// Solve the enlarger M/Y filter shifts that print a midgray neutral with
/// this look (upstream "Neutralize print filters").
pub fn neutralize_filters(preset: &Preset, data_dir: &Path) -> anyhow::Result<(f32, f32)> {
    let (film, print, params) = preset.resolve(data_dir)?;
    spektrafilm_core::print_balance::solve_neutral_filter_shifts(&film, &print, &params, data_dir, 0.184).map_err(|e| anyhow::anyhow!(e))
}

/// Decode options LibRaw itself sees (the rest is applied afterwards).
#[derive(Clone, PartialEq)]
struct DecodeKey {
    path: PathBuf,
    /// Which frame of a clip, in hundredths of a second; `None` for a photo.
    frame: Option<i64>,
    as_shot: bool,
    highlight: i32,
    demosaic: Option<i32>,
    max_px: u32,
}

/// A rendered picture as the viewport wants it: sRGB-encoded 8-bit RGBA, row-major, no padding.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// From a pipeline output that is already sRGB-encoded.
    pub fn from_encoded(img: &ImageBuf) -> Frame {
        let mut rgba = Vec::with_capacity(img.data.len() / 3 * 4);
        for px in img.data.chunks_exact(3) {
            for v in px {
                rgba.push((spektrafilm_math::precision::to_f32(*v).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push(255);
        }
        Frame { width: img.width, height: img.height, rgba }
    }

    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// Preview renderer with small decode and pipeline caches. Not `Sync`; keep
/// one behind a mutex.
pub struct PreviewEngine {
    backend: Box<dyn ComputeBackend>,
    decodes: Vec<(DecodeKey, LinearImage)>,
    pipeline: Option<(String, Pipeline)>,
}

impl Default for PreviewEngine {
    fn default() -> Self {
        PreviewEngine { backend: select_backend(), decodes: Vec::new(), pipeline: None }
    }
}

impl PreviewEngine {
    fn decode(&mut self, raw_path: &Path, raw: &RawSettings, max_px: u32, at: Option<f64>) -> anyhow::Result<LinearImage> {
        let key = DecodeKey {
            path: raw_path.to_path_buf(),
            frame: at.map(|t| (t * 100.0) as i64),
            as_shot: raw.white_balance == WhiteBalance::AsShot,
            highlight: raw.highlight,
            demosaic: raw.demosaic,
            max_px,
        };
        if let Some(pos) = self.decodes.iter().position(|(k, _)| k == &key) {
            let entry = self.decodes.remove(pos);
            let img = entry.1.clone();
            self.decodes.push(entry);
            return Ok(img);
        }
        // A clip previews on one of its frames: the look is the same, and seeing it on the
        // picture beats seeing it on nothing.
        if crate::video::is_video_path(raw_path) {
            let meta = crate::video::probe(raw_path)?;
            let at = at.unwrap_or_else(|| (meta.duration * 0.1).min(2.0));
            let (rgb, w, h) = crate::video::frame_rgb8(raw_path, at, max_px)?;
            let srgb = spektrafilm_math::colourspaces::lookup("sRGB").map_err(|e| anyhow::anyhow!(e))?;
            let lut: Vec<f32> = (0..256).map(|v| srgb.cctf.decode(v as f64 / 255.0) as f32).collect();
            let img = LinearImage {
                width: w,
                height: h,
                data: rgb.iter().map(|v| lut[*v as usize]).collect(),
                color_space: "sRGB",
            };
            self.decodes.push((key, img.clone()));
            if self.decodes.len() > 4 {
                self.decodes.remove(0);
            }
            return Ok(img);
        }
        if !crate::decode::is_raw_path(raw_path) {
            // Camera JPEGs: decoded straight to linear Rec. 2020; the white
            // balance / exposure are applied per render like for RAWs.
            let img = crate::decode::decode_image(raw_path, &RawSettings::default(), max_px)?;
            self.decodes.push((key, img.clone()));
            if self.decodes.len() > 4 {
                self.decodes.remove(0);
            }
            return Ok(img);
        }
        // LibRaw options only: white balance mode, highlights, demosaic. The
        // colour-science adjustments are applied per render.
        let lib_only = RawSettings {
            white_balance: if key.as_shot { WhiteBalance::AsShot } else { WhiteBalance::Daylight },
            exposure_ev: 0.0,
            ..raw.clone()
        };
        let mut settings = DevelopSettings::with_raw(&lib_only);
        let mut file = RawFile::open(raw_path)?;
        // A half-size decode is four times quicker and all a fitted preview can use — but
        // "full resolution" has to mean the sensor's pixels, not half of them magnified.
        let long = file.width().max(file.height());
        settings.half_size = max_px != 0 && max_px * 2 <= long;
        let img = file.develop(&settings)?;
        let img = downscale(img, max_px);
        self.decodes.push((key, img.clone()));
        if self.decodes.len() > 4 {
            self.decodes.remove(0);
        }
        Ok(img)
    }

    /// The look's pipeline for input in `color_space`, reusing the cached one
    /// while only render-time parameters change.
    fn pipeline(&mut self, preset: &Preset, data_dir: &Path, color_space: &str) -> anyhow::Result<Pipeline> {
        let (film, print, mut params) = preset.resolve(data_dir)?;
        params.io.input_color_space = color_space.to_string();
        params.io.input_cctf_decoding = false;
        params.io.output_color_space = "sRGB".into();
        params.io.output_cctf_encoding = true;
        params.io.upscale_factor = 1.0;
        let key = spektrafilm_core::construction_key::construction_key(&preset.film, &print_name(&print), &params);
        Ok(match &self.pipeline {
            Some((k, p)) if *k == key => p.clone().with_params(params),
            _ => {
                let p = Pipeline::new_with_spectral(film, print, params, data_dir).map_err(|e| anyhow::anyhow!(e))?;
                self.pipeline = Some((key, p.clone()));
                p
            }
        })
    }

    /// Render `raw_path` through `preset` at most `max_px` on the long edge,
    /// as sRGB JPEG bytes.
    pub fn render(
        &mut self,
        raw_path: &Path,
        raw: &RawSettings,
        preset: &Preset,
        data_dir: &Path,
        max_px: u32,
        at: Option<f64>,
    ) -> anyhow::Result<Vec<u8>> {
        let mut img = self.decode(raw_path, raw, max_px, at)?;
        // The decode above used LibRaw's own balance; now the adaptation,
        // tint and exposure.
        apply_raw_adjustments(&mut img, raw);
        let img = crate::geometry::apply(img, raw);
        let pipeline = self.pipeline(preset, data_dir, img.color_space)?;
        let buf = ImageBuf::from_data(img.width, img.height, img.data.iter().map(|v| from_f32(*v)).collect());
        let out = pipeline.process(buf, self.backend.as_ref());
        crate::export::encode_jpeg(&out, 88)
    }

    /// The look on the photo, as pixels for the viewport: sRGB-encoded 8-bit RGBA, no JPEG
    /// in between. This is what Develop and Print draw.
    pub fn frame(
        &mut self,
        raw_path: &Path,
        raw: &RawSettings,
        preset: &Preset,
        data_dir: &Path,
        max_px: u32,
        at: Option<f64>,
    ) -> anyhow::Result<Frame> {
        let mut img = self.decode(raw_path, raw, max_px, at)?;
        apply_raw_adjustments(&mut img, raw);
        let img = crate::geometry::apply(img, raw);
        let pipeline = self.pipeline(preset, data_dir, img.color_space)?;
        let buf = ImageBuf::from_data(img.width, img.height, img.data.iter().map(|v| from_f32(*v)).collect());
        let out = pipeline.process(buf, self.backend.as_ref());
        Ok(Frame::from_encoded(&out))
    }

    /// The developed photo as viewport pixels (see [`Self::frame`]).
    pub fn frame_developed(&mut self, raw_path: &Path, raw: &RawSettings, max_px: u32, at: Option<f64>) -> anyhow::Result<Frame> {
        let mut img = self.decode(raw_path, raw, max_px, at)?;
        apply_raw_adjustments(&mut img, raw);
        self.to_srgb_frame(crate::geometry::apply(img, raw))
    }

    /// The print stage's "before" as viewport pixels (see [`Self::render_before`]).
    pub fn frame_before(
        &mut self,
        raw_path: &Path,
        raw: &RawSettings,
        preset: &Preset,
        data_dir: &Path,
        max_px: u32,
        at: Option<f64>,
    ) -> anyhow::Result<Frame> {
        let mut img = self.decode(raw_path, raw, max_px, at)?;
        apply_raw_adjustments(&mut img, raw);
        let mut img = crate::geometry::apply(img, raw);
        let pipeline = self.pipeline(preset, data_dir, img.color_space)?;
        let buf = ImageBuf::from_data(img.width, img.height, img.data.iter().map(|v| from_f32(*v)).collect());
        let ev = pipeline.autoexposure_ev(&buf) + pipeline.params.camera.exposure_compensation_ev as f64;
        let gain = 2f32.powf(ev as f32);
        img.data.iter_mut().for_each(|v| *v *= gain);
        self.to_srgb_frame(img)
    }

    /// Linear, in its own primaries -> sRGB-encoded 8-bit RGBA.
    fn to_srgb_frame(&self, img: LinearImage) -> anyhow::Result<Frame> {
        let src = spektrafilm_math::colourspaces::lookup(img.color_space).map_err(|e| anyhow::anyhow!(e))?;
        let dst = spektrafilm_math::colourspaces::lookup("sRGB").map_err(|e| anyhow::anyhow!(e))?;
        let mut m = [[0.0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = (0..3).map(|k| dst.xyz_to_rgb[i][k] * src.rgb_to_xyz[k][j]).sum::<f64>() as f32;
            }
        }
        // The transfer curve through a table: one pow per code value, not per pixel.
        let lut: Vec<u8> = (0..4096).map(|i| (dst.cctf.encode(i as f64 / 4095.0) * 255.0).round() as u8).collect();
        let mut rgba = Vec::with_capacity(img.data.len() / 3 * 4);
        for px in img.data.chunks_exact(3) {
            for row in &m {
                let v = (row[0] * px[0] + row[1] * px[1] + row[2] * px[2]).clamp(0.0, 1.0);
                rgba.push(lut[(v * 4095.0) as usize]);
            }
            rgba.push(255);
        }
        Ok(Frame { width: img.width, height: img.height, rgba })
    }

    /// The developed photo: decode plus its develop settings, colour-managed
    /// to sRGB. This is the develop stage's own output — no film look.
    pub fn render_developed(&mut self, raw_path: &Path, raw: &RawSettings, max_px: u32, at: Option<f64>) -> anyhow::Result<Vec<u8>> {
        let mut img = self.decode(raw_path, raw, max_px, at)?;
        apply_raw_adjustments(&mut img, raw);
        self.to_srgb_jpeg(crate::geometry::apply(img, raw))
    }

    /// The print stage's "before": the developed photo at the exposure the
    /// look itself applies (autoexposure + compensation), so the toggle shows
    /// only what the film does.
    pub fn render_before(
        &mut self,
        raw_path: &Path,
        raw: &RawSettings,
        preset: &Preset,
        data_dir: &Path,
        max_px: u32,
        at: Option<f64>,
    ) -> anyhow::Result<Vec<u8>> {
        let mut img = self.decode(raw_path, raw, max_px, at)?;
        apply_raw_adjustments(&mut img, raw);
        let mut img = crate::geometry::apply(img, raw);
        let pipeline = self.pipeline(preset, data_dir, img.color_space)?;
        let buf = ImageBuf::from_data(img.width, img.height, img.data.iter().map(|v| from_f32(*v)).collect());
        let ev = pipeline.autoexposure_ev(&buf) + pipeline.params.camera.exposure_compensation_ev as f64;
        let gain = 2f32.powf(ev as f32);
        img.data.iter_mut().for_each(|v| *v *= gain);
        self.to_srgb_jpeg(img)
    }

    fn to_srgb_jpeg(&self, img: LinearImage) -> anyhow::Result<Vec<u8>> {
        let src = spektrafilm_math::colourspaces::lookup(img.color_space).map_err(|e| anyhow::anyhow!(e))?;
        let dst = spektrafilm_math::colourspaces::lookup("sRGB").map_err(|e| anyhow::anyhow!(e))?;
        let mut m = [[0.0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = (0..3).map(|k| dst.xyz_to_rgb[i][k] * src.rgb_to_xyz[k][j]).sum::<f64>() as f32;
            }
        }
        let mut data = Vec::with_capacity(img.data.len());
        for px in img.data.chunks_exact(3) {
            for row in &m {
                let v = (row[0] * px[0] + row[1] * px[1] + row[2] * px[2]).clamp(0.0, 1.0);
                data.push(from_f32(dst.cctf.encode(v as f64) as f32));
            }
        }
        crate::export::encode_jpeg(&ImageBuf::from_data(img.width, img.height, data), 88)
    }

    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }
}

fn print_name(p: &profile::Profile) -> String {
    p.info.stock.clone().unwrap_or_default()
}

/// Box-filter downscale so the long edge is at most `max_px`.
fn downscale(img: LinearImage, max_px: u32) -> LinearImage {
    let long = img.width.max(img.height);
    if max_px == 0 || long <= max_px {
        return img;
    }
    let f = (long as f32 / max_px as f32).ceil() as u32;
    let (w, h) = (img.width / f, img.height / f);
    let mut data = vec![0.0f32; (w * h * 3) as usize];
    let n = (f * f) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for dy in 0..f {
                let row = ((y * f + dy) * img.width + x * f) as usize * 3;
                for dx in 0..f as usize {
                    for c in 0..3 {
                        acc[c] += img.data[row + dx * 3 + c];
                    }
                }
            }
            let o = ((y * w + x) * 3) as usize;
            for c in 0..3 {
                data[o + c] = acc[c] / n;
            }
        }
    }
    LinearImage { width: w, height: h, data, color_space: img.color_space }
}
