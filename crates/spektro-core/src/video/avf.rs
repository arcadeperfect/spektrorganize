//! The AVFoundation side of video support. Everything unsafe lives here.

use super::{VideoError, VideoMeta};
use objc2_av_foundation::{AVAsset, AVAssetImageGenerator, AVMediaTypeVideo, AVURLAsset};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{NSString, NSURL};
use std::path::Path;

fn fail(what: &str, path: &Path) -> VideoError {
    VideoError::Failed(format!("{what}: {}", path.display()))
}

/// An AVURLAsset for a file path.
fn asset(path: &Path) -> Result<objc2::rc::Retained<AVURLAsset>, VideoError> {
    let s = NSString::from_str(&path.to_string_lossy());
    // SAFETY: a file URL from a path string; AVURLAsset takes no ownership of it.
    unsafe {
        let url = NSURL::fileURLWithPath(&s);
        Ok(AVURLAsset::URLAssetWithURL_options(&url, None))
    }
}

pub fn probe(path: &Path) -> Result<VideoMeta, VideoError> {
    let a = asset(path)?;
    // SAFETY: reading properties of a live asset. `tracksWithMediaType` is the synchronous
    // accessor — deprecated in favour of the async one, but this runs on a worker thread where
    // blocking is what we want, and the async form needs a run loop we do not have.
    unsafe {
        let av: &AVAsset = &a;
        let dur = av.duration();
        let duration = if dur.timescale != 0 { dur.value as f64 / dur.timescale as f64 } else { 0.0 };

        #[allow(deprecated)]
        let tracks = av.tracksWithMediaType(AVMediaTypeVideo.ok_or_else(|| fail("no video media type", path))?);
        let Some(track) = tracks.firstObject() else {
            return Err(fail("no video track", path));
        };
        let size = track.naturalSize();
        let fps = track.nominalFrameRate();

        let created = av.creationDate().and_then(|item| {
            item.stringValue().map(|s| s.to_string())
        });

        Ok(VideoMeta {
            duration,
            width: size.width.abs() as u32,
            height: size.height.abs() as u32,
            fps,
            codec: codec_of(&track),
            created,
        })
    }
}

/// The video track's codec as its four-character code (`hvc1`, `avc1`, `apcn`…).
fn codec_of(track: &objc2_av_foundation::AVAssetTrack) -> String {
    // SAFETY: the format descriptions belong to the track and are read, not kept.
    unsafe {
        #[allow(deprecated)]
        let descs = track.formatDescriptions();
        let Some(first) = descs.firstObject() else { return String::new() };
        let desc: *const objc2_core_media::CMFormatDescription = std::mem::transmute(&*first);
        let code: u32 = objc2_core_media::CMFormatDescription::media_sub_type(&*desc);
        String::from_utf8_lossy(&code.to_be_bytes()).trim_end_matches('\0').to_string()
    }
}

pub fn frame_rgb8(path: &Path, at: f64, max_px: u32) -> Result<(Vec<u8>, u32, u32), VideoError> {
    let a = asset(path)?;
    // SAFETY: the generator is ours; the CGImage it returns is retained until we drop it.
    unsafe {
        let maker = AVAssetImageGenerator::assetImageGeneratorWithAsset(&a);
        // Orientation as the camera recorded it, and a tolerance so the decoder can use the
        // nearest keyframe instead of decoding forward to an exact time.
        maker.setAppliesPreferredTrackTransform(true);
        let slack = CMTime { value: 1, timescale: 2, flags: CMTimeFlags::Valid, epoch: 0 };
        maker.setRequestedTimeToleranceBefore(slack);
        maker.setRequestedTimeToleranceAfter(slack);
        if max_px > 0 {
            maker.setMaximumSize(CGSize { width: max_px as f64, height: max_px as f64 });
        }
        let time = CMTime { value: (at.max(0.0) * 600.0) as i64, timescale: 600, flags: CMTimeFlags::Valid, epoch: 0 };
        // The synchronous copy: deprecated in favour of the async one, which needs a run loop
        // this worker thread does not have.
        #[allow(deprecated)]
        let img = maker
            .copyCGImageAtTime_actualTime_error(time, std::ptr::null_mut())
            .map_err(|_| fail("could not read a frame", path))?;
        cg_to_rgb8(&img).ok_or_else(|| fail("could not convert the frame", path))
    }
}

/// Draw a CGImage into our own RGBA buffer, then drop the alpha. Going through a bitmap context
/// is what makes the result predictable: whatever the source pixel format, we get 8-bit RGB.
fn cg_to_rgb8(img: &CGImage) -> Option<(Vec<u8>, u32, u32)> {
    // SAFETY: the context writes into `rgba`, which outlives it, with a matching row stride.
    unsafe {
        let w = CGImage::width(Some(img));
        let h = CGImage::height(Some(img));
        if w == 0 || h == 0 {
            return None;
        }
        let mut rgba = vec![0u8; w * h * 4];
        let space = CGColorSpace::new_device_rgb()?;
        let ctx = CGBitmapContextCreate(
            rgba.as_mut_ptr() as *mut _,
            w,
            h,
            8,
            w * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )?;
        CGContext::draw_image(
            Some(&ctx),
            CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w as f64, height: h as f64 } },
            Some(img),
        );
        let mut rgb = Vec::with_capacity(w * h * 3);
        for px in rgba.chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
        }
        Some((rgb, w as u32, h as u32))
    }
}
