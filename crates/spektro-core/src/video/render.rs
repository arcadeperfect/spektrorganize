//! Rendering a clip: read frames, put each through the look, write them out again.
//!
//! Two ways to apply a look, because they answer different needs. A **LUT** is baked once from
//! the preset and applied per frame — fast enough to be practical on a whole clip, and the same
//! cube can go to Resolve. The **full** pipeline runs spektrafilm on every frame, which is what
//! the stills do: grain, halation and all, at seconds per frame rather than milliseconds.
//!
//! Both read and write through AVFoundation, so the codecs are the system's.

use super::VideoError;
use crate::film::Preset;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How the look reaches the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LookMode {
    /// Copy the picture through untouched (a format change, nothing more).
    None,
    /// Bake the preset into a colour cube and apply that to every frame.
    Lut,
    /// Run the whole film pipeline on every frame. Slow, and the only way to get grain.
    Full,
}

/// What comes out the other end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutCodec {
    /// H.264 in MP4 — the one that plays everywhere and posts anywhere.
    H264,
    /// HEVC in MP4: smaller at the same quality, fussier about where it plays.
    Hevc,
    /// ProRes 422 in a QuickTime movie, for editing rather than posting.
    ProRes422,
    /// ProRes 4444, when the extra weight is worth it.
    ProRes4444,
}

impl OutCodec {
    pub fn extension(self) -> &'static str {
        match self {
            OutCodec::H264 | OutCodec::Hevc => "mp4",
            OutCodec::ProRes422 | OutCodec::ProRes4444 => "mov",
        }
    }

    /// The AVFoundation codec key.
    pub(crate) fn av_codec(self) -> &'static str {
        match self {
            OutCodec::H264 => "avc1",
            OutCodec::Hevc => "hvc1",
            OutCodec::ProRes422 => "apcn",
            OutCodec::ProRes4444 => "ap4h",
        }
    }

    pub(crate) fn is_prores(self) -> bool {
        matches!(self, OutCodec::ProRes422 | OutCodec::ProRes4444)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderRequest {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub codec: OutCodec,
    pub look: LookMode,
    /// Longest edge; 0 keeps the clip's own size. 1920 is the usual "postable" answer.
    pub max_px: u32,
    /// Megabits per second for H.264/HEVC. ProRes ignores it — the codec sets its own rate.
    pub mbps: f32,
    /// Asks for the original sound to be carried across. Not honoured yet — see
    /// [`RenderReport::silent`].
    pub audio: bool,
}

impl Default for RenderRequest {
    fn default() -> Self {
        RenderRequest {
            src: PathBuf::new(),
            dst: PathBuf::new(),
            codec: OutCodec::H264,
            look: LookMode::Lut,
            max_px: 1920,
            mbps: 12.0,
            audio: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RenderReport {
    pub frames: usize,
    pub seconds: f32,
    pub width: u32,
    pub height: u32,
    /// The clip was rendered without its sound. True for now whenever the source had any.
    pub silent: bool,
}

/// Progress as frames go by. `total` is an estimate from duration × frame rate.
pub type OnProgress<'a> = &'a (dyn Fn(usize, usize) + Send + Sync);

/// Render `req`, calling `progress` as frames are written. `cancel` stops it between frames,
/// leaving a partial file that is then removed.
pub fn render(
    req: &RenderRequest,
    preset: Option<&Preset>,
    data_dir: &Path,
    progress: OnProgress<'_>,
    cancel: &dyn Fn() -> bool,
) -> Result<RenderReport, VideoError> {
    #[cfg(target_os = "macos")]
    {
        super::avf_render::render(req, preset, data_dir, progress, cancel)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (req, preset, data_dir, progress, cancel);
        Err(VideoError::Unsupported)
    }
}
