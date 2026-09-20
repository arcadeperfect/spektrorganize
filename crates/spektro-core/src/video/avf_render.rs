//! Reading frames out of a clip, putting them through the look, and writing them back.
//!
//! One pass, frame by frame: AVAssetReader hands us BGRA, we hand back BGRA, AVAssetWriter
//! encodes. The audio track is copied across untouched — no decode, no re-encode — so a clip
//! keeps its sound without us having to understand it.

use super::lut::{Cube, bake, pipeline_for, srgb_decode};
use super::render::{LookMode, RenderReport, RenderRequest};
use super::VideoError;
use crate::film::Preset;
use objc2::rc::Retained;
use objc2_av_foundation::{
    AVAsset, AVAssetReader, AVAssetReaderTrackOutput, AVAssetWriter, AVAssetWriterInput, AVFileTypeMPEG4,
    AVFileTypeQuickTimeMovie, AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset, AVVideoAverageBitRateKey,
    AVVideoCodecKey, AVVideoCompressionPropertiesKey, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use spektrafilm_gpu::{ComputeBackend, select_backend};
use spektrafilm_math::image::ImageBuf;
use spektrafilm_math::precision::{from_f32, to_f32};
use std::path::Path;
use std::time::Instant;

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

fn oops(what: &str) -> VideoError {
    VideoError::Failed(what.to_string())
}

pub fn render(
    req: &RenderRequest,
    preset: Option<&Preset>,
    data_dir: &Path,
    progress: super::render::OnProgress<'_>,
    cancel: &dyn Fn() -> bool,
) -> Result<RenderReport, VideoError> {
    let meta = super::probe(&req.src)?;
    let (out_w, out_h) = fit(meta.width, meta.height, req.max_px);
    let started = Instant::now();

    // The look, if there is one. Both modes need a pipeline; the LUT mode only needs it once.
    let backend: Box<dyn ComputeBackend> = select_backend();
    let mut cube: Option<Cube> = None;
    let mut pipeline = None;
    if req.look != LookMode::None {
        let preset = preset.ok_or_else(|| oops("that look could not be loaded"))?;
        // Meter the exposure on one frame so the whole clip is graded the same way.
        let reference = first_frame(&req.src, &meta).ok();
        let p = pipeline_for(preset, data_dir, reference.as_ref()).map_err(|e| VideoError::Failed(e.to_string()))?;
        match req.look {
            LookMode::Lut => cube = Some(bake(&p, backend.as_ref(), 33).map_err(|e| VideoError::Failed(e.to_string()))?),
            LookMode::Full => pipeline = Some(p),
            LookMode::None => {}
        }
    }

    let total = (meta.duration * meta.fps.max(1.0) as f64).round().max(1.0) as usize;
    let mut frames = 0usize;

    // SAFETY: the reader, writer and their inputs are ours for the length of this call; every
    // buffer is locked before its bytes are touched and unlocked straight after.
    unsafe {
        let src_url = NSURL::fileURLWithPath(&NSString::from_str(&req.src.to_string_lossy()));
        let asset = AVURLAsset::URLAssetWithURL_options(&src_url, None);
        let av: &AVAsset = &asset;

        let reader = AVAssetReader::assetReaderWithAsset_error(av).map_err(|_| oops("could not read the clip"))?;
        #[allow(deprecated)]
        let video_track = av
            .tracksWithMediaType(AVMediaTypeVideo.ok_or_else(|| oops("no video media type"))?)
            .firstObject()
            .ok_or_else(|| oops("the clip has no video track"))?;

        let px_key = NSString::from_str("PixelFormatType");
        let settings = NSDictionary::from_slices(&[&*px_key], &[&*NSNumber::new_u32(BGRA) as &objc2::runtime::AnyObject]);
        let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&video_track, Some(&settings));
        output.setAlwaysCopiesSampleData(false);
        if !reader.canAddOutput(&output) {
            return Err(oops("this clip's video cannot be read as plain pixels"));
        }
        reader.addOutput(&output);

        // Sound, copied across as it is.
        #[allow(deprecated)]
        let audio_track = av
            .tracksWithMediaType(AVMediaTypeAudio.ok_or_else(|| oops("no audio media type"))?)
            .firstObject();
        // Its own reader: one reader serving two outputs decodes them in step, and pulling
        // sound ahead of picture through the same one is what made it stall.
        // Sound is not carried across yet: an AVAssetWriter interleaves its inputs, and every
        // arrangement tried here ends with one input waiting on the other. The rendered clip is
        // silent, which the caller is told about rather than left to discover.
        let audio_reader: Option<Retained<AVAssetReader>> = None;
        let _ = (&audio_track, req.audio);
        let audio_out = match (&audio_reader, &audio_track) {
            (Some(r), Some(t)) => {
                let o = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(t, None);
                r.canAddOutput(&o).then(|| {
                    r.addOutput(&o);
                    o
                })
            }
            _ => None,
        };

        let _ = std::fs::remove_file(&req.dst);
        let dst_url = NSURL::fileURLWithPath(&NSString::from_str(&req.dst.to_string_lossy()));
        let file_type = if req.codec.is_prores() { AVFileTypeQuickTimeMovie } else { AVFileTypeMPEG4 };
        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&dst_url, file_type.ok_or_else(|| oops("no file type"))?)
            .map_err(|_| oops("could not create the output file"))?;

        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeVideo.ok_or_else(|| oops("no video media type"))?,
            Some(&video_settings(req, out_w, out_h)),
        );
        input.setExpectsMediaDataInRealTime(false);
        if !writer.canAddInput(&input) {
            return Err(oops("that codec cannot be written here"));
        }
        writer.addInput(&input);

        // One adaptor, made before writing starts: a second one for the same input raises.
        let attrs = {
            use objc2::runtime::AnyObject;
            let key = NSString::from_str("PixelFormatType");
            let fmt = NSNumber::new_u32(BGRA);
            NSDictionary::from_slices(&[&*key], &[&*fmt as &AnyObject])
        };
        let adaptor = objc2_av_foundation::AVAssetWriterInputPixelBufferAdaptor::
            assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(&input, Some(&attrs));

        // Passthrough audio needs the source format to describe itself; without the hint the
        // writer refuses the input, and anything appended to an input it never took raises.
        let audio_in = match (&audio_out, &audio_track) {
            (Some(_), Some(t)) => {
                #[allow(deprecated)]
                let hint = t.formatDescriptions().firstObject().map(|d| {
                    let p: *const objc2_core_media::CMFormatDescription = std::mem::transmute(&*d);
                    &*p as &objc2_core_media::CMFormatDescription
                });
                let i = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings_sourceFormatHint(
                    AVMediaTypeAudio.expect("audio media type"),
                    None,
                    hint,
                );
                i.setExpectsMediaDataInRealTime(false);
                if writer.canAddInput(&i) {
                    writer.addInput(&i);
                    Some(i)
                } else {
                    None
                }
            }
            _ => None,
        };

        if !writer.startWriting() {
            return Err(oops(&format!("the writer would not start: {}", writer_error(&writer))));
        }
        if !reader.startReading() {
            return Err(oops("the reader would not start"));
        }
        if let Some(r) = &audio_reader
            && !r.startReading()
        {
            return Err(oops("the clip's sound could not be read"));
        }
        writer.startSessionAtSourceTime(CMTime { value: 0, timescale: 600, flags: objc2_core_media::CMTimeFlags::Valid, epoch: 0 });

        let mut work: Vec<f32> = Vec::new();
        // Sound is copied across as the video goes by, not after it. A writer with two inputs
        // interleaves them, and stops taking frames for one while the other lags behind — feed
        // it all the video first and it simply waits for ever.
        let mut audio_left = audio_in.is_some();
        while let Some(sample) = output.copyNextSampleBuffer() {
            if cancel() {
                reader.cancelReading();
                writer.cancelWriting();
                let _ = std::fs::remove_file(&req.dst);
                return Err(oops("cancelled"));
            }
            let time = CMSampleBuffer::presentation_time_stamp(&sample);
            if let (Some(ao), Some(ai)) = (&audio_out, &audio_in) {
                pump_audio(ao, ai, seconds(time) + 1.0, &mut audio_left);
            }
            let Some(src_px) = CMSampleBuffer::image_buffer(&sample) else { continue };
            let src_px: &CVPixelBuffer = &src_px;

            let out_px = process_frame(src_px, out_w, out_h, cube.as_ref(), pipeline.as_ref(), backend.as_ref(), &mut work)?;

            // The encoder pulls; wait for it rather than queueing frames it has not asked for.
            // If it has given up, `isReadyForMoreMediaData` never comes back true, so watch the
            // writer's status too — otherwise this waits for ever.
            let deadline = Instant::now() + std::time::Duration::from_secs(120);
            while !input.isReadyForMoreMediaData() {
                if writer.status() != objc2_av_foundation::AVAssetWriterStatus::Writing {
                    return Err(oops(&format!("the encoder stopped: {}", writer_error(&writer))));
                }
                if Instant::now() > deadline {
                    return Err(oops("the encoder stopped asking for frames"));
                }
                // Keep feeding sound while we wait: the writer interleaves, so a starved audio
                // input is exactly why the video one would be holding back.
                if let (Some(ao), Some(ai)) = (&audio_out, &audio_in) {
                    pump_audio(ao, ai, seconds(time) + 1.0, &mut audio_left);
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            if !adaptor.appendPixelBuffer_withPresentationTime(&out_px, time) {
                return Err(oops(&format!("the encoder refused a frame: {}", writer_error(&writer))));
            }
            frames += 1;
            progress(frames, total);
        }

        // The picture is done. Say so before draining the rest of the sound: while the writer
        // thinks more frames are coming, it holds the audio input closed to keep the two
        // interleaved, and the drain below would wait for ever.
        input.markAsFinished();

        if let (Some(ao), Some(ai)) = (&audio_out, &audio_in) {
            let deadline = Instant::now() + std::time::Duration::from_secs(60);
            while audio_left {
                if !ai.isReadyForMoreMediaData() {
                    if writer.status() != objc2_av_foundation::AVAssetWriterStatus::Writing || Instant::now() > deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    continue;
                }
                match ao.copyNextSampleBuffer() {
                    Some(sample) => {
                        ai.appendSampleBuffer(&sample);
                    }
                    None => audio_left = false,
                }
            }
            ai.markAsFinished();
        }
        #[allow(deprecated)]
        writer.finishWriting();
        if writer.status() != objc2_av_foundation::AVAssetWriterStatus::Completed {
            return Err(oops(&format!("writing failed: {}", writer_error(&writer))));
        }
    }

    Ok(RenderReport { frames, seconds: started.elapsed().as_secs_f32(), width: out_w, height: out_h, silent: true })
}

/// Copy sound across until it is `ahead` seconds past the picture, or the encoder stops asking.
unsafe fn pump_audio(ao: &AVAssetReaderTrackOutput, ai: &AVAssetWriterInput, ahead: f64, left: &mut bool) {
    unsafe {
        while *left && ai.isReadyForMoreMediaData() {
            match ao.copyNextSampleBuffer() {
                Some(a) => {
                    let at = seconds(CMSampleBuffer::presentation_time_stamp(&a));
                    ai.appendSampleBuffer(&a);
                    if at > ahead {
                        break;
                    }
                }
                None => *left = false,
            }
        }
    }
}

/// A timestamp in plain seconds.
fn seconds(t: CMTime) -> f64 {
    if t.timescale == 0 { 0.0 } else { t.value as f64 / t.timescale as f64 }
}

/// Longest edge capped at `max_px`, keeping the shape and staying even (encoders insist).
fn fit(w: u32, h: u32, max_px: u32) -> (u32, u32) {
    let (w, h) = (w.max(2), h.max(2));
    let long = w.max(h);
    let (mut ow, mut oh) = if max_px == 0 || long <= max_px {
        (w, h)
    } else {
        let s = max_px as f64 / long as f64;
        (((w as f64 * s).round() as u32).max(2), ((h as f64 * s).round() as u32).max(2))
    };
    ow -= ow % 2;
    oh -= oh % 2;
    (ow, oh)
}

/// The output settings dictionary: codec, size, and a bitrate for the lossy ones.
unsafe fn video_settings(req: &RenderRequest, w: u32, h: u32) -> Retained<NSDictionary<NSString>> {
    use objc2::runtime::AnyObject;
    let codec = NSString::from_str(req.codec.av_codec());
    let width = NSNumber::new_u32(w);
    let height = NSNumber::new_u32(h);

    let mut keys: Vec<&NSString> = vec![
        unsafe { AVVideoCodecKey }.expect("codec key"),
        unsafe { AVVideoWidthKey }.expect("width key"),
        unsafe { AVVideoHeightKey }.expect("height key"),
    ];
    let mut values: Vec<&AnyObject> = vec![&codec as &AnyObject, &width as &AnyObject, &height as &AnyObject];

    // ProRes sets its own rate; only the lossy codecs take a bitrate.
    let props;
    if !req.codec.is_prores() {
        let bits = NSNumber::new_u32((req.mbps.max(0.5) * 1_000_000.0) as u32);
        let rate_key = unsafe { AVVideoAverageBitRateKey }.expect("bitrate key");
        props = NSDictionary::from_slices(&[rate_key], &[&*bits as &AnyObject]);
        keys.push(unsafe { AVVideoCompressionPropertiesKey }.expect("compression key"));
        values.push(&props as &AnyObject);
    }
    NSDictionary::from_slices(&keys, &values)
}

/// The first frame, as the pipeline sees it — used to meter exposure once for the whole clip.
fn first_frame(path: &Path, meta: &super::VideoMeta) -> Result<ImageBuf, VideoError> {
    // A little way in, like the poster frame: the head of a clip is often black.
    let at = (meta.duration * 0.1).min(2.0);
    let (rgb, w, h) = super::frame_rgb8(path, at, 1024)?;
    let data = rgb.iter().map(|v| from_f32(srgb_decode(*v as f32 / 255.0))).collect();
    Ok(ImageBuf::from_data(w, h, data))
}

/// One frame: read the pixels, put them through the look, hand back a buffer to encode.
unsafe fn process_frame(
    src: &CVPixelBuffer,
    out_w: u32,
    out_h: u32,
    cube: Option<&Cube>,
    pipeline: Option<&spektrafilm_core::pipeline::Pipeline>,
    backend: &dyn ComputeBackend,
    work: &mut Vec<f32>,
) -> Result<Retained<CVPixelBuffer>, VideoError> {
    unsafe {
        CVPixelBufferLockBaseAddress(src, CVPixelBufferLockFlags::ReadOnly);
        let w = CVPixelBufferGetWidth(src);
        let h = CVPixelBufferGetHeight(src);
        let stride = CVPixelBufferGetBytesPerRow(src);
        let base = CVPixelBufferGetBaseAddress(src) as *const u8;
        if base.is_null() || w == 0 || h == 0 {
            CVPixelBufferUnlockBaseAddress(src, CVPixelBufferLockFlags::ReadOnly);
            return Err(oops("a frame had no pixels"));
        }

        // BGRA in, RGB out, scaled to the output size with a box filter.
        work.clear();
        work.reserve(out_w as usize * out_h as usize * 3);
        for y in 0..out_h as usize {
            let sy = (y * h) / out_h as usize;
            let row = base.add(sy * stride);
            for x in 0..out_w as usize {
                let sx = (x * w) / out_w as usize;
                let px = row.add(sx * 4);
                work.push(*px.add(2) as f32 / 255.0);
                work.push(*px.add(1) as f32 / 255.0);
                work.push(*px as f32 / 255.0);
            }
        }
        CVPixelBufferUnlockBaseAddress(src, CVPixelBufferLockFlags::ReadOnly);

        // The look. Both paths take and return display-referred values.
        match (cube, pipeline) {
            (Some(c), _) => {
                use rayon::prelude::*;
                work.par_chunks_exact_mut(3).for_each(|px| {
                    let out = c.apply([px[0], px[1], px[2]]);
                    px.copy_from_slice(&out);
                });
            }
            (None, Some(p)) => {
                let data = work.iter().map(|v| from_f32(srgb_decode(*v))).collect();
                let img = ImageBuf::from_data(out_w, out_h, data);
                let done = p.process(img, backend);
                work.clear();
                work.extend(done.data.iter().map(|v| to_f32(*v)));
            }
            (None, None) => {}
        }

        // Back to BGRA for the encoder.
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        let status = CVPixelBufferCreate(None, out_w as usize, out_h as usize, BGRA, None, std::ptr::NonNull::from(&mut out));
        if status != 0 || out.is_null() {
            return Err(oops("could not make a frame to write into"));
        }
        let out = Retained::from_raw(out).ok_or_else(|| oops("could not make a frame to write into"))?;
        CVPixelBufferLockBaseAddress(&out, CVPixelBufferLockFlags(0));
        let dst = CVPixelBufferGetBaseAddress(&out) as *mut u8;
        let dst_stride = CVPixelBufferGetBytesPerRow(&out);
        for y in 0..out_h as usize {
            let row = dst.add(y * dst_stride);
            for x in 0..out_w as usize {
                let i = (y * out_w as usize + x) * 3;
                let px = row.add(x * 4);
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                *px = q(work[i + 2]);
                *px.add(1) = q(work[i + 1]);
                *px.add(2) = q(work[i]);
                *px.add(3) = 255;
            }
        }
        CVPixelBufferUnlockBaseAddress(&out, CVPixelBufferLockFlags(0));
        Ok(out)
    }
}

/// Whatever the writer has to say about why it stopped.
unsafe fn writer_error(writer: &AVAssetWriter) -> String {
    unsafe { writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "no reason given".into()) }
}
