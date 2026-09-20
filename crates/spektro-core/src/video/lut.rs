//! Baking a look into a colour cube, and applying that cube to frames.
//!
//! A cube is the practical way to put a film look on a whole clip: the pipeline runs once over
//! the grid, and every frame after that is three interpolations per pixel. What it cannot carry
//! is anything spatial — grain and halation depend on neighbouring pixels, so they are simply not
//! in a LUT. That is the trade, and it is the same trade a colourist makes.
//!
//! The cube is display-referred: sRGB code values in, sRGB code values out, which is what the
//! frames already are and what Resolve expects from a `.cube` file.

use crate::film::Preset;
use spektrafilm_core::pipeline::Pipeline;
use spektrafilm_math::image::ImageBuf;
use spektrafilm_math::precision::{from_f32, to_f32};
use std::path::Path;

/// A cube of `size`³ entries, indexed `r + g*size + b*size²`, each a display-referred RGB triple.
#[derive(Debug, Clone)]
pub struct Cube {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl Cube {
    /// Look up a display-referred colour, trilinearly interpolated.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let last = (n - 1) as f32;
        let at = |r: usize, g: usize, b: usize| -> [f32; 3] { self.data[r + g * n + b * n * n] };

        let pos = rgb.map(|v| v.clamp(0.0, 1.0) * last);
        let lo = pos.map(|v| v.floor() as usize);
        let hi = lo.map(|v| (v + 1).min(n - 1));
        let f = [pos[0] - lo[0] as f32, pos[1] - lo[1] as f32, pos[2] - lo[2] as f32];

        let mut out = [0.0f32; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let c00 = at(lo[0], lo[1], lo[2])[i] + (at(hi[0], lo[1], lo[2])[i] - at(lo[0], lo[1], lo[2])[i]) * f[0];
            let c01 = at(lo[0], lo[1], hi[2])[i] + (at(hi[0], lo[1], hi[2])[i] - at(lo[0], lo[1], hi[2])[i]) * f[0];
            let c10 = at(lo[0], hi[1], lo[2])[i] + (at(hi[0], hi[1], lo[2])[i] - at(lo[0], hi[1], lo[2])[i]) * f[0];
            let c11 = at(lo[0], hi[1], hi[2])[i] + (at(hi[0], hi[1], hi[2])[i] - at(lo[0], hi[1], hi[2])[i]) * f[0];
            let c0 = c00 + (c10 - c00) * f[1];
            let c1 = c01 + (c11 - c01) * f[1];
            *o = c0 + (c1 - c0) * f[2];
        }
        out
    }

    /// The cube as a Resolve-readable `.cube` file.
    pub fn to_cube_file(&self, title: &str) -> String {
        let mut out = format!("TITLE \"{title}\"\nLUT_3D_SIZE {}\nDOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n\n", self.size);
        // .cube runs red fastest, which is the order the data is already in.
        for px in &self.data {
            out.push_str(&format!("{:.6} {:.6} {:.6}\n", px[0], px[1], px[2]));
        }
        out
    }
}

/// sRGB transfer, the two directions.
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Build the pipeline a clip is rendered through: sRGB in and out, no upscaling, and a fixed
/// exposure. Auto-exposure is measured once on a reference frame and folded in — left on, it
/// would meter every frame separately and the clip would pump as the scene changed.
pub fn pipeline_for(
    preset: &Preset,
    data_dir: &Path,
    reference: Option<&ImageBuf>,
) -> anyhow::Result<Pipeline> {
    let (film, print, mut params) = preset.resolve(data_dir)?;
    params.io.input_color_space = "sRGB".into();
    params.io.input_cctf_decoding = false;
    params.io.output_color_space = "sRGB".into();
    params.io.output_cctf_encoding = true;
    params.io.upscale_factor = 1.0;
    let metering = params.camera.auto_exposure;
    let mut p = Pipeline::new_with_spectral(film, print, params, data_dir).map_err(|e| anyhow::anyhow!(e))?;
    if metering {
        let ev = reference.map(|img| p.autoexposure_ev(img)).unwrap_or(0.0);
        let mut params = p.params.clone();
        params.camera.auto_exposure = false;
        params.camera.exposure_compensation_ev += ev as f32;
        p = p.with_params(params);
    }
    Ok(p)
}

/// Run the grid through the pipeline to get a cube. 33 is the usual size; 64 is finer and takes
/// eight times as long to bake (still seconds, not minutes).
pub fn bake(pipeline: &Pipeline, backend: &dyn spektrafilm_gpu::ComputeBackend, size: usize) -> anyhow::Result<Cube> {
    anyhow::ensure!((2..=64).contains(&size), "a cube is between 2 and 64 a side");
    let last = (size - 1) as f32;
    let mut data = Vec::with_capacity(size * size * size * 3);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                // The grid is in code values; the pipeline wants light.
                data.push(from_f32(srgb_decode(r as f32 / last)));
                data.push(from_f32(srgb_decode(g as f32 / last)));
                data.push(from_f32(srgb_decode(b as f32 / last)));
            }
        }
    }
    let n = (size * size * size) as u32;
    let img = ImageBuf::from_data(n, 1, data);
    let out = pipeline.process(img, backend);
    let cube = out.data.chunks_exact(3).map(|p| [to_f32(p[0]), to_f32(p[1]), to_f32(p[2])]).collect();
    Ok(Cube { size, data: cube })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cube that does nothing, to check the interpolation itself.
    fn identity(size: usize) -> Cube {
        let last = (size - 1) as f32;
        let mut data = Vec::new();
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push([r as f32 / last, g as f32 / last, b as f32 / last]);
                }
            }
        }
        Cube { size, data }
    }

    #[test]
    fn an_identity_cube_returns_what_it_is_given() {
        let c = identity(17);
        for v in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.25, 0.5, 0.75], [0.13, 0.87, 0.42]] {
            let got = c.apply(v);
            for i in 0..3 {
                assert!((got[i] - v[i]).abs() < 1e-5, "{v:?} -> {got:?}");
            }
        }
    }

    #[test]
    fn values_outside_the_cube_are_clamped_not_wrapped() {
        let c = identity(9);
        assert_eq!(c.apply([-1.0, 2.0, 0.5])[0], 0.0);
        assert_eq!(c.apply([-1.0, 2.0, 0.5])[1], 1.0);
    }

    #[test]
    fn cube_files_carry_every_entry_red_fastest() {
        let c = identity(3);
        let text = c.to_cube_file("test");
        assert!(text.contains("LUT_3D_SIZE 3"));
        let rows: Vec<&str> = text.lines().filter(|l| l.starts_with(char::is_numeric)).collect();
        assert_eq!(rows.len(), 27);
        assert!(rows[1].starts_with("0.500000 0.000000 0.000000"), "red moves first: {}", rows[1]);
    }
}
