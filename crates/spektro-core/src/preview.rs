//! Thumbnails: from a sidecar/lone JPEG (or other still) or the camera preview embedded in the RAW.
//!
//! Stills go through macOS `sips` (ImageIO) first: it decodes and downsizes camera JPEGs far
//! faster than the in-process decoder and keeps the EXIF orientation. The `image` crate is the
//! fallback (and the only path on other platforms). Embedded RAW previews are pulled out with
//! LibRaw and decoded in-process; they are small.

use crate::decode::RawFile;
use crate::scan::{Group, PreviewSource, Scan};
use image::imageops::FilterType;
use image::DynamicImage;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Produce (or reuse) a downscaled JPEG thumbnail for a scanned group in `cache_dir`. Returns
/// `None` when the group has nothing viewable (videos, sidecars).
pub fn thumbnail(scan: &Scan, group: &Group, cache_dir: &Path, max_px: u32) -> anyhow::Result<Option<PathBuf>> {
    let Some(fid) = group.preview_file else { return Ok(None) };
    let file = scan.file(fid);
    let key = {
        let mut h = blake3::Hasher::new();
        h.update(file.path.to_string_lossy().as_bytes());
        h.update(&file.size.to_le_bytes());
        if let Some(t) = file.mtime.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
            h.update(&t.as_secs().to_le_bytes());
        }
        h.update(&max_px.to_le_bytes());
        h.finalize().to_hex()[..24].to_string()
    };
    let out = cache_dir.join("thumbs").join(format!("{key}.jpg"));
    if out.exists() {
        return Ok(Some(out));
    }
    match make_thumbnails(&file.path, group.preview, &[(max_px, &out)])? {
        Some(_) => Ok(Some(out)),
        None => Ok(None),
    }
}

/// Write one JPEG per `(max_px, path)` from a single decode of `src`. Sizes may come in any
/// order. Returns the pixel size of the largest thumbnail written, or `None` when the source
/// has no preview (a RAW without an embedded JPEG, a video).
pub fn make_thumbnails(src: &Path, source: PreviewSource, outs: &[(u32, &Path)]) -> anyhow::Result<Option<(u32, u32)>> {
    if outs.is_empty() {
        return Ok(None);
    }
    let mut sorted: Vec<(u32, &Path)> = outs.to_vec();
    sorted.sort_by_key(|o| std::cmp::Reverse(o.0));
    let (largest_px, largest_out) = sorted[0];
    for (_, p) in &sorted {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
    }

    let img: DynamicImage = match source {
        PreviewSource::SidecarJpeg | PreviewSource::Itself => {
            // Fast path: ImageIO writes the largest size; smaller ones come from that file.
            let tmp = largest_out.with_extension("sips.jpg");
            if sips_resize(src, &tmp, largest_px) {
                let bytes = std::fs::read(&tmp)?;
                if exif_orientation(&bytes).unwrap_or(1) == 1 && sorted.len() == 1 {
                    std::fs::rename(&tmp, largest_out)?;
                    return Ok(image::image_dimensions(largest_out).ok().or(Some((largest_px, largest_px))));
                }
                // sips keeps the EXIF rotation flag; store upright pixels instead, so every
                // consumer (including the AI labeller) sees the picture the right way up.
                let _ = std::fs::remove_file(&tmp);
                decode_jpeg_oriented(&bytes)?
            } else {
                let _ = std::fs::remove_file(&tmp);
                decode_file_oriented(src)?
            }
        }
        PreviewSource::EmbeddedPreview => {
            let mut raw = RawFile::open(src)?;
            match raw.largest_jpeg_preview()? {
                Some(b) => decode_jpeg_oriented(&b)?,
                None => return Ok(None),
            }
        }
        PreviewSource::VideoFrame => {
            let (rgb, w, h) = crate::video::frame_rgb8(src, video_poster_time(src), largest_px).map_err(|e| anyhow::anyhow!("{e}"))?;
            let buf = image::RgbImage::from_raw(w, h, rgb).ok_or_else(|| anyhow::anyhow!("frame size mismatch"))?;
            DynamicImage::ImageRgb8(buf)
        }
        PreviewSource::None => return Ok(None),
    };

    // One large decode: shrink fast first, then filter properly at the exact size.
    let mut current = shrink(img, largest_px);
    let dims = (current.width(), current.height());
    for (i, (px, out)) in sorted.iter().enumerate() {
        if i > 0 {
            current = current.resize(*px, *px, FilterType::Triangle);
        }
        write_jpeg(&current, out)?;
    }
    Ok(Some(dims))
}

/// A little way into the clip, so the poster is not the black frame at the head.
fn video_poster_time(src: &Path) -> f64 {
    crate::video::probe(src).map(|m| (m.duration * 0.1).min(2.0)).unwrap_or(0.0)
}

fn shrink(img: DynamicImage, max_px: u32) -> DynamicImage {
    if img.width().max(img.height()) <= max_px {
        return img;
    }
    // `thumbnail` is a fast box filter; stop at 2x and finish with a triangle filter.
    let (w, h) = (img.width(), img.height());
    let scale = (2 * max_px) as f32 / w.max(h) as f32;
    let img = if scale < 1.0 { img.thumbnail((w as f32 * scale) as u32, (h as f32 * scale) as u32) } else { img };
    img.resize(max_px, max_px, FilterType::Triangle)
}

fn write_jpeg(img: &DynamicImage, out: &Path) -> anyhow::Result<()> {
    let tmp = out.with_extension("jpg.part");
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    enc.encode_image(&img.to_rgb8())?;
    std::fs::write(&tmp, &buf)?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

fn decode_file_oriented(path: &Path) -> anyhow::Result<DynamicImage> {
    let bytes = std::fs::read(path)?;
    let orientation = exif_orientation(&bytes).unwrap_or(1);
    let img = image::load_from_memory(&bytes)?;
    Ok(orient(img, orientation))
}

fn decode_jpeg_oriented(jpeg: &[u8]) -> anyhow::Result<DynamicImage> {
    let orientation = exif_orientation(jpeg).unwrap_or(1);
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)?;
    Ok(orient(img, orientation))
}

fn orient(img: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        3 => img.rotate180(),
        6 => img.rotate90(),
        8 => img.rotate270(),
        2 => img.fliph(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        7 => img.rotate270().fliph(),
        _ => img,
    }
}

fn exif_orientation(jpeg: &[u8]) -> Option<u16> {
    let mut cur = Cursor::new(jpeg);
    let exif = exif::Reader::new().read_from_container(&mut cur).ok()?;
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(|v| v as u16)
}

/// `sips -Z max -s format jpeg src --out dst`, killed after 15 s. False when sips is missing
/// (not macOS), fails, or produces nothing.
#[cfg(target_os = "macos")]
fn sips_resize(src: &Path, dst: &Path, max_px: u32) -> bool {
    let mut cmd = std::process::Command::new("/usr/bin/sips");
    cmd.arg("-Z")
        .arg(max_px.to_string())
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "85"])
        .arg(src)
        .arg("--out")
        .arg(dst)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    run_with_timeout(cmd, Duration::from_secs(15)) && std::fs::metadata(dst).map(|m| m.len() > 0).unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn sips_resize(_src: &Path, _dst: &Path, _max_px: u32) -> bool {
    false
}

/// Run a child to completion, killing it when it overruns. True on exit status 0.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn run_with_timeout(mut cmd: std::process::Command, timeout: Duration) -> bool {
    let Ok(mut child) = cmd.spawn() else { return false };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_two_sizes_from_one_still() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a.jpg");
        let img = image::RgbImage::from_fn(1200, 800, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        img.save(&src).unwrap();
        let big = tmp.path().join("t/1024/a.jpg");
        let small = tmp.path().join("t/256/a.jpg");
        let dims = make_thumbnails(&src, PreviewSource::Itself, &[(256, &small), (1024, &big)]).unwrap().unwrap();
        assert_eq!(dims.0.max(dims.1), 1024);
        let (w, h) = image::image_dimensions(&small).unwrap();
        assert_eq!(w.max(h), 256);
        assert!(w > h, "landscape stays landscape");
        assert!(!big.with_extension("jpg.part").exists());
    }
}
