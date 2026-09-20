//! LibRaw wrapper: metadata, embedded previews, and an "honest" linear decode.
//!
//! One `RawFile` per file, per thread. `open` only parses headers, so it is cheap enough for
//! scanning a card; `develop` does the full unpack + demosaic.

use serde::{Deserialize, Serialize};
use crate::meta::{CaptureMeta, MetaSource, normalise_model};
use chrono::{DateTime, Local};
use rsraw_sys as sys;
use std::ffi::{CStr, CString};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("{op}: {msg} ({code})")]
    LibRaw { op: &'static str, code: i32, msg: String },
    #[error("path is not valid UTF-8/C string")]
    BadPath,
    #[error("LibRaw returned an unexpected image layout: {0}")]
    Layout(String),
}

fn check(op: &'static str, code: i32) -> Result<(), DecodeError> {
    if code == sys::LibRaw_errors_LIBRAW_SUCCESS as i32 {
        return Ok(());
    }
    let msg = unsafe {
        let p = sys::libraw_strerror(code);
        if p.is_null() { String::from("unknown error") } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    Err(DecodeError::LibRaw { op, code, msg })
}

fn cstr(buf: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    let s = String::from_utf8_lossy(&bytes).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Linear RGB, interleaved, values in `[0, 1]`, primaries as requested at develop time.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
    /// spektrafilm colour space name of the primaries.
    pub color_space: &'static str,
}

/// Decode settings. The defaults are the "honest decode": camera white balance, camera matrix,
/// no brightness scaling, no highlight recovery, no denoise.
#[derive(Debug, Clone)]
pub struct DevelopSettings {
    /// LibRaw `output_color`: 1 sRGB, 2 Adobe, 3 Wide, 4 ProPhoto, 5 XYZ, 6 ACES, 7 DCI-P3, 8 Rec.2020.
    pub output_color: i32,
    /// LibRaw `user_qual` demosaic: 0 linear, 1 VNG, 2 PPG, 3 AHD, 4 DCB, 11 DHT, 12 AAHD.
    /// X-Trans sensors use Markesteijn; quality > 2 selects the 3-pass variant.
    pub demosaic: i32,
    /// Half-size decode (2x2 binning, no demosaic). For previews only.
    pub half_size: bool,
    /// User-facing RAW adjustments (white balance, exposure, highlights).
    pub raw: RawSettings,
}

impl Default for DevelopSettings {
    fn default() -> Self {
        DevelopSettings { output_color: 8, demosaic: 3, half_size: false, raw: RawSettings::default() }
    }
}

impl DevelopSettings {
    /// The honest decode with a photo's RAW settings applied.
    pub fn with_raw(raw: &RawSettings) -> Self {
        DevelopSettings { demosaic: raw.demosaic.unwrap_or(3), raw: raw.clone(), ..Default::default() }
    }
}

/// White balance for the decode, mirroring upstream spektrafilm's RAW loader
/// (`utils/raw_file_processor.py`): as shot uses the camera's multipliers;
/// the others start from LibRaw's daylight balance and adapt (CAT02 von
/// Kries) from the scene temperature to the 6504 K reference.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhiteBalance {
    #[default]
    AsShot,
    Daylight,
    Tungsten,
    Custom,
}

/// Basic per-photo RAW settings. All default to the honest decode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RawSettings {
    pub white_balance: WhiteBalance,
    /// Scene colour temperature in kelvin (custom white balance).
    pub temperature: f64,
    /// Green–magenta multiplier on G after the adaptation (1 = none).
    pub tint: f64,
    /// Linear exposure change before the film stage, in stops.
    pub exposure_ev: f64,
    /// LibRaw highlight mode: 0 clip, 1 unclip, 2 blend, 3–9 rebuild.
    pub highlight: i32,
    /// LibRaw demosaic (`user_qual`); `None` keeps the default (AHD /
    /// Markesteijn 3-pass).
    pub demosaic: Option<i32>,
    /// Quarter turns clockwise on top of the camera's own orientation.
    pub rotate: i32,
    /// Straighten angle in degrees, positive clockwise. The frame is trimmed to fit.
    pub straighten: f64,
    /// Crop rectangle, as fractions of the straightened frame. `None` = the whole frame.
    pub crop: Option<Crop>,
}

/// A crop rectangle in fractions of the frame, from the top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Default for Crop {
    fn default() -> Self {
        Crop { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }
    }
}

impl Crop {
    /// The whole frame, in which case there is nothing to cut.
    pub fn is_whole(&self) -> bool {
        self.x <= 0.0 && self.y <= 0.0 && self.w >= 1.0 && self.h >= 1.0
    }
}

impl Default for RawSettings {
    fn default() -> Self {
        RawSettings {
            white_balance: WhiteBalance::AsShot,
            temperature: 5500.0,
            tint: 1.0,
            exposure_ev: 0.0,
            highlight: 0,
            demosaic: None,
            rotate: 0,
            straighten: 0.0,
            crop: None,
        }
    }
}

impl RawSettings {
    pub fn is_default(&self) -> bool {
        self == &RawSettings::default()
    }

    /// Whether anything in the geometry stage would change the frame.
    pub fn has_geometry(&self) -> bool {
        self.rotate.rem_euclid(4) != 0 || self.straighten.abs() >= 0.001 || self.crop.is_some_and(|c| !c.is_whole())
    }

    /// Scene temperature to adapt from, if the white balance adapts at all.
    fn adapt_from(&self) -> Option<f64> {
        match self.white_balance {
            WhiteBalance::AsShot | WhiteBalance::Daylight => None,
            WhiteBalance::Tungsten => Some(2850.0),
            WhiteBalance::Custom => Some(self.temperature.clamp(1667.0, 25000.0)),
        }
    }

    /// Short stable key for caches.
    pub fn key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

impl DevelopSettings {
    pub fn color_space_name(&self) -> &'static str {
        match self.output_color {
            1 => "sRGB",
            4 => "ProPhoto RGB",
            6 => "ACES2065-1",
            8 => "Rec. 2020",
            _ => "unknown",
        }
    }
}

pub struct RawFile {
    ptr: *mut sys::libraw_data_t,
}

unsafe impl Send for RawFile {}

impl Drop for RawFile {
    fn drop(&mut self) {
        unsafe { sys::libraw_close(self.ptr) }
    }
}

impl RawFile {
    /// Parse headers only.
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| DecodeError::BadPath)?;
        let ptr = unsafe { sys::libraw_init(0) };
        if ptr.is_null() {
            return Err(DecodeError::LibRaw { op: "libraw_init", code: -1, msg: "null handle".into() });
        }
        let me = RawFile { ptr };
        check("open_file", unsafe { sys::libraw_open_file(ptr, c.as_ptr()) })?;
        Ok(me)
    }

    fn data(&self) -> &sys::libraw_data_t {
        unsafe { &*self.ptr }
    }

    fn data_mut(&mut self) -> &mut sys::libraw_data_t {
        unsafe { &mut *self.ptr }
    }

    pub fn meta(&self) -> CaptureMeta {
        let d = self.data();
        let captured_at = if d.other.timestamp > 0 {
            // LibRaw parses the EXIF wall clock through mktime(); converting back through the
            // same local zone returns the wall clock the camera wrote.
            DateTime::<chrono::Utc>::from_timestamp(d.other.timestamp as i64, 0)
                .map(|t| t.with_timezone(&Local).naive_local())
        } else {
            None
        };
        let make = cstr(&d.idata.make);
        let model = cstr(&d.idata.model);
        let camera = cstr(&d.idata.normalized_model).or_else(|| model.as_deref().map(normalise_model));
        let iso = if d.other.iso_speed > 0.0 { Some(d.other.iso_speed.round() as u32) } else { None };
        let lens = cstr(&d.lens.Lens);
        let orientation = Some(flip_to_exif_orientation(d.sizes.flip));
        CaptureMeta { captured_at, make, model, camera, iso, lens, orientation, source: MetaSource::Raw }
    }

    pub fn width(&self) -> u32 {
        self.data().sizes.width as u32
    }
    pub fn height(&self) -> u32 {
        self.data().sizes.height as u32
    }

    /// Largest embedded JPEG preview, as raw JPEG bytes.
    pub fn largest_jpeg_preview(&mut self) -> Result<Option<Vec<u8>>, DecodeError> {
        let list = self.data().thumbs_list;
        let mut best: Option<(usize, u64)> = None;
        for i in 0..(list.thumbcount.max(0) as usize).min(list.thumblist.len()) {
            let t = &list.thumblist[i];
            if t.tformat == sys::LibRaw_internal_thumbnail_formats_LIBRAW_INTERNAL_THUMBNAIL_JPEG {
                let px = t.twidth as u64 * t.theight as u64;
                if best.is_none_or(|(_, b)| px > b) {
                    best = Some((i, px));
                }
            }
        }
        let Some((idx, _)) = best else { return Ok(None) };
        check("unpack_thumb_ex", unsafe { sys::libraw_unpack_thumb_ex(self.ptr, idx as i32) })?;
        let t = &self.data().thumbnail;
        if t.tformat != sys::LibRaw_thumbnail_formats_LIBRAW_THUMBNAIL_JPEG || t.thumb.is_null() {
            return Ok(None);
        }
        let bytes = unsafe { std::slice::from_raw_parts(t.thumb as *const u8, t.tlength as usize) }.to_vec();
        Ok(Some(bytes))
    }

    /// Full decode to linear RGB in the requested primaries. Camera orientation is applied.
    pub fn develop(&mut self, s: &DevelopSettings) -> Result<LinearImage, DecodeError> {
        {
            let d = self.data_mut();
            d.rawparams.use_rawspeed = 0;
            d.rawparams.max_raw_memory_mb = 4096;
            let p = &mut d.params;
            p.use_camera_wb = if s.raw.white_balance == WhiteBalance::AsShot { 1 } else { 0 };
            p.use_auto_wb = 0;
            p.use_camera_matrix = 1;
            p.output_color = s.output_color;
            p.gamm[0] = 1.0;
            p.gamm[1] = 1.0;
            p.no_auto_bright = 1;
            p.bright = 1.0;
            p.output_bps = 16;
            p.highlight = s.raw.highlight.clamp(0, 9);
            p.user_qual = s.demosaic;
            p.user_flip = -1;
            p.half_size = if s.half_size { 1 } else { 0 };
            p.fbdd_noiserd = 0;
            p.threshold = 0.0;
            p.med_passes = 0;
            p.use_fuji_rotate = 1;
        }
        check("unpack", unsafe { sys::libraw_unpack(self.ptr) })?;
        check("dcraw_process", unsafe { sys::libraw_dcraw_process(self.ptr) })?;
        let mut err: i32 = 0;
        let img = unsafe { sys::libraw_dcraw_make_mem_image(self.ptr, &mut err) };
        check("dcraw_make_mem_image", err)?;
        if img.is_null() {
            return Err(DecodeError::Layout("null processed image".into()));
        }
        let out = unsafe {
            let h = &*img;
            let result = if h.type_ != sys::LibRaw_image_formats_LIBRAW_IMAGE_BITMAP || h.colors != 3 || h.bits != 16 {
                Err(DecodeError::Layout(format!("type={} colors={} bits={}", h.type_, h.colors, h.bits)))
            } else {
                let n = h.width as usize * h.height as usize * 3;
                let bytes = std::slice::from_raw_parts(h.data.as_ptr(), h.data_size as usize);
                if bytes.len() < n * 2 {
                    Err(DecodeError::Layout(format!("data_size {} < {}", bytes.len(), n * 2)))
                } else {
                    let mut data = Vec::with_capacity(n);
                    // LibRaw writes 16-bit samples in host byte order.
                    for c in bytes[..n * 2].chunks_exact(2) {
                        data.push(u16::from_ne_bytes([c[0], c[1]]) as f32 / 65535.0);
                    }
                    let mut img = LinearImage { width: h.width as u32, height: h.height as u32, data, color_space: s.color_space_name() };
                    apply_raw_adjustments(&mut img, &s.raw);
                    Ok(img)
                }
            };
            sys::libraw_dcraw_clear_mem(img);
            result
        };
        // Free the unpacked raw buffers; the handle stays usable for metadata.
        unsafe { sys::libraw_recycle(self.ptr) };
        out
    }
}

/// Whether `path` is something LibRaw should decode (vs an ordinary image).
pub fn is_raw_path(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff" | "webp")
}

/// Decode an ordinary image (camera JPEG etc.) to linear light in the same
/// primaries RAW decodes use (Rec. 2020): orientation applied, sRGB transfer
/// curve removed, then the photo's white balance / exposure. Highlight and
/// demosaic settings do not apply. `max_px` > 0 downsizes on load.
pub fn decode_image(path: &Path, raw: &RawSettings, max_px: u32) -> Result<LinearImage, DecodeError> {
    use image::ImageDecoder;
    let io = |e: std::io::Error| DecodeError::Layout(format!("{}: {e}", path.display()));
    let img_err = |e: image::ImageError| DecodeError::Layout(format!("{}: {e}", path.display()));
    let reader = image::ImageReader::open(path).map_err(io)?.with_guessed_format().map_err(io)?;
    let mut decoder = reader.into_decoder().map_err(img_err)?;
    let orientation = decoder.orientation().map_err(img_err)?;
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(img_err)?;
    img.apply_orientation(orientation);
    if max_px > 0 && img.width().max(img.height()) > max_px {
        img = img.resize(max_px, max_px, image::imageops::FilterType::Triangle);
    }
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let srgb = spektrafilm_math::colourspaces::lookup("sRGB").map_err(DecodeError::Layout)?;
    let rec2020 = spektrafilm_math::colourspaces::lookup("ITU-R BT.2020").map_err(DecodeError::Layout)?;
    // sRGB -> Rec. 2020 (both D65, no adaptation).
    let mut m = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = (0..3).map(|k| rec2020.xyz_to_rgb[i][k] * srgb.rgb_to_xyz[k][j]).sum::<f64>() as f32;
        }
    }
    let lut: Vec<f32> = (0..256).map(|v| srgb.cctf.decode(v as f64 / 255.0) as f32).collect();
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for px in rgb.pixels() {
        let lin = [lut[px[0] as usize], lut[px[1] as usize], lut[px[2] as usize]];
        for row in &m {
            data.push(row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]);
        }
    }
    let mut out = LinearImage { width: w, height: h, data, color_space: "Rec. 2020" };
    apply_raw_adjustments(&mut out, raw);
    Ok(out)
}

/// Decode a photo for rendering: LibRaw for RAWs, [`decode_image`] otherwise.
pub fn develop_any(path: &Path, settings: &DevelopSettings) -> Result<LinearImage, DecodeError> {
    let img = if is_raw_path(path) { RawFile::open(path)?.develop(settings)? } else { decode_image(path, &settings.raw, 0)? };
    Ok(crate::geometry::apply(img, &settings.raw))
}

/// White-balance adaptation, tint and exposure on the decoded linear image
/// (in its own primaries). No-op for the defaults.
pub fn apply_raw_adjustments(img: &mut LinearImage, raw: &RawSettings) {
    let gain = 2f64.powf(raw.exposure_ev);
    let adapt = raw.adapt_from();
    let tint = if raw.white_balance == WhiteBalance::Custom { raw.tint } else { 1.0 };
    if adapt.is_none() && gain == 1.0 && tint == 1.0 {
        return;
    }
    let Ok(cs) = spektrafilm_math::colourspaces::lookup(img.color_space) else {
        return;
    };
    let mut m = match adapt {
        Some(t) => spektrafilm_math::white_balance::rgb_white_balance_matrix(&cs.rgb_to_xyz, &cs.xyz_to_rgb, t, 6504.0, tint),
        None => {
            let mut id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            for v in &mut id[1] {
                *v *= tint;
            }
            id
        }
    };
    for row in &mut m {
        for v in row.iter_mut() {
            *v *= gain;
        }
    }
    let m = m.map(|r| r.map(|v| v as f32));
    use rayon::prelude::*;
    img.data.par_chunks_exact_mut(3).for_each(|px| {
        let (r, g, b) = (px[0], px[1], px[2]);
        for i in 0..3 {
            px[i] = m[i][0] * r + m[i][1] * g + m[i][2] * b;
        }
    });
}

/// LibRaw `sizes.flip` (dcraw convention) -> EXIF orientation value.
fn flip_to_exif_orientation(flip: i32) -> u16 {
    match flip {
        0 => 1,
        3 => 3,
        5 => 8,
        6 => 6,
        _ => 1,
    }
}

/// Convenience for the scanner: metadata for a RAW path, or a message.
pub fn read_meta(path: &Path) -> Result<CaptureMeta, String> {
    RawFile::open(path).map(|r| r.meta()).map_err(|e| e.to_string())
}

pub fn libraw_version() -> String {
    unsafe { CStr::from_ptr(sys::libraw_version()).to_string_lossy().into_owned() }
}
