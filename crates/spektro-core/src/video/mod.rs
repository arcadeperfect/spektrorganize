//! Video: what the catalog needs to show a clip, and what the print stage needs to render one.
//!
//! macOS only, and deliberately so. Everything goes through AVFoundation, which means the system
//! decodes and encodes: no bundled codec, no HEVC patent problem, and hardware acceleration where
//! the machine has it. On other platforms the functions here return [`VideoError::Unsupported`]
//! and the app carries on without video features rather than failing to build.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[cfg(target_os = "macos")]
mod avf;
#[cfg(target_os = "macos")]
mod avf_render;
pub mod lut;
pub mod render;

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("video needs macOS (AVFoundation); this build has no video support")]
    Unsupported,
    #[error("{0}")]
    Failed(String),
}

/// What a clip is, as far as the catalog is concerned.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoMeta {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    /// Four-character code of the video track's codec, e.g. `hvc1`, `avc1`, `apcn`.
    pub codec: String,
    /// When the camera recorded it, when the file says so.
    pub created: Option<String>,
}

impl VideoMeta {
    /// HEVC, which is the expensive one to decode and the one behind the thumbnail setting.
    pub fn is_hevc(&self) -> bool {
        matches!(self.codec.as_str(), "hvc1" | "hev1" | "hvcC" | "dvh1" | "dvhe")
    }

    /// `1:02:03` / `4:07`, for a caption.
    pub fn duration_text(&self) -> String {
        let total = self.duration.round().max(0.0) as u64;
        let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
        if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
    }
}

/// Read a clip's duration, size, frame rate, codec and creation date.
pub fn probe(path: &Path) -> Result<VideoMeta, VideoError> {
    #[cfg(target_os = "macos")]
    {
        avf::probe(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err(VideoError::Unsupported)
    }
}

/// A frame from `at` seconds in, as RGB8 at most `max_px` on the long edge.
///
/// `at` is clamped into the clip; asking past the end gives the last frame rather than an error,
/// which matters for the very short clips phones produce.
pub fn frame_rgb8(path: &Path, at: f64, max_px: u32) -> Result<(Vec<u8>, u32, u32), VideoError> {
    #[cfg(target_os = "macos")]
    {
        avf::frame_rgb8(path, at, max_px)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (_, _, _) = (path, at, max_px);
        Err(VideoError::Unsupported)
    }
}

/// A poster frame as JPEG bytes, from a little way in so it is not the black first frame.
pub fn poster_jpeg(path: &Path, max_px: u32, quality: u8) -> Result<Vec<u8>, VideoError> {
    let meta = probe(path)?;
    // A tenth of the way in, capped at two seconds: past the fade-up, before anything cuts.
    let at = (meta.duration * 0.1).min(2.0);
    let (rgb, w, h) = frame_rgb8(path, at, max_px)?;
    let img = image::RgbImage::from_raw(w, h, rgb).ok_or_else(|| VideoError::Failed("frame size mismatch".into()))?;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut std::io::Cursor::new(&mut out), quality)
        .encode_image(&image::DynamicImage::ImageRgb8(img))
        .map_err(|e| VideoError::Failed(e.to_string()))?;
    Ok(out)
}

/// Whether this build can do anything with video at all.
pub const fn supported() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_reads_as_minutes_and_seconds() {
        let m = |d: f64| VideoMeta { duration: d, ..Default::default() };
        assert_eq!(m(20.02).duration_text(), "0:20");
        assert_eq!(m(95.0).duration_text(), "1:35");
        assert_eq!(m(3725.0).duration_text(), "1:02:05");
    }

    #[test]
    fn hevc_is_recognised_by_its_four_character_code() {
        let c = |c: &str| VideoMeta { codec: c.into(), ..Default::default() };
        assert!(c("hvc1").is_hevc() && c("hev1").is_hevc());
        assert!(!c("avc1").is_hevc() && !c("apcn").is_hevc());
    }

    /// Render a real clip, with no look, to check the read/write path end to end:
    /// `SPEKTRO_TEST_VIDEO=/path/clip.mov cargo test -p spektro-core renders_a_real_clip -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a video file"]
    fn renders_a_real_clip() {
        let Ok(path) = std::env::var("SPEKTRO_TEST_VIDEO") else { return };
        let src = std::path::PathBuf::from(path);
        let dst = std::env::temp_dir().join("spektro-render-test.mp4");
        let req = render::RenderRequest {
            src: src.clone(),
            dst: dst.clone(),
            codec: render::OutCodec::H264,
            look: match std::env::var("SPEKTRO_TEST_LOOK").as_deref() {
                Ok("full") => render::LookMode::Full,
                Ok(_) => render::LookMode::Lut,
                Err(_) => render::LookMode::None,
            },
            max_px: 640,
            mbps: 4.0,
            audio: std::env::var("SPEKTRO_TEST_AUDIO").is_ok(),
        };
        let preset = std::env::var("SPEKTRO_TEST_PRESET").ok().map(|p| crate::film::Preset::load(std::path::Path::new(&p)).expect("preset"));
        let data_dir = std::env::var("SPEKTRO_DATA_DIR").unwrap_or_else(|_| "vendor/spektrafilm-data".into());
        let report = render::render(&req, preset.as_ref(), std::path::Path::new(&data_dir), &|d, t| {
            if d % 30 == 0 {
                println!("  {d}/{t}");
            }
        }, &|| false)
        .expect("render");
        println!("{} frames, {}x{}, {:.1}s", report.frames, report.width, report.height, report.seconds);
        let out = probe(&dst).expect("probe the result");
        println!("out: {} {}x{} {}", out.duration_text(), out.width, out.height, out.codec);
        assert!(report.frames > 0 && out.width == report.width);
        let _ = std::fs::remove_file(&dst);
    }

    /// Point this at a real clip to check the AVFoundation path:
    /// `SPEKTRO_TEST_VIDEO=/path/to/clip.mov cargo test -p spektro-core video -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a video file"]
    fn reads_a_real_clip() {
        let Ok(path) = std::env::var("SPEKTRO_TEST_VIDEO") else { return };
        let path = std::path::Path::new(&path);
        let meta = probe(path).expect("probe");
        println!("{} {}x{} {:.2}fps {} {:?}", meta.duration_text(), meta.width, meta.height, meta.fps, meta.codec, meta.created);
        assert!(meta.duration > 0.0 && meta.width > 0 && meta.height > 0);
        let jpg = poster_jpeg(path, 512, 85).expect("poster");
        println!("poster: {} bytes", jpg.len());
        assert!(jpg.len() > 1000);
    }
}
