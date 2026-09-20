//! Orchestration: copy phase, manifest, render phase; and re-rendering from a manifest.

use crate::config::{Config, Outputs};
use crate::copy::copy_file;
use crate::decode::{DevelopSettings, RawFile};
use crate::export::{Chromaticities, write_exr, write_jpeg};
use crate::film::{Preset, Renderer, find_data_dir, resolve_preset};
use crate::manifest::{CopyStatus, Entry, Manifest, OutputKind, OutputRecord};
use crate::plan::{DestStatus, Plan};
use crate::scan::{FileId, Kind, Scan};
use crate::template::{Context, Template};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    CopyStarted { files: usize, bytes: u64 },
    FileStarted { file: FileId, dest: PathBuf, size: u64 },
    FileProgress { file: FileId, bytes_done: u64 },
    FileDone { file: FileId, status: CopyStatus },
    CopyFinished { copied: usize, skipped: usize, failed: usize, bytes: u64 },
    ManifestWritten { path: PathBuf },
    RenderStarted { total: usize, backend: String },
    RenderDone { entry: u32, outputs: Vec<PathBuf>, seconds: f32 },
    RenderFailed { entry: u32, error: String },
    RenderFinished { done: usize, failed: usize },
    /// Rendering was requested but could not start; the copy still happened.
    RenderSkipped { reason: String },
    Log(String),
}

pub type EventSink = Arc<dyn Fn(JobEvent) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct ImportReport {
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub rendered: usize,
    pub render_failed: usize,
}

/// Called once the copy phase is over and the manifest is on disk, before any rendering starts
/// (the catalog indexes the import here, so new photos show up while the renders run).
pub type AfterCopy<'a> = &'a (dyn Fn(&Path, &Manifest) + Sync);

/// Copy everything in the plan, write the manifest, then render the RAWs from their archived
/// copies. Rendering never starts before the last copy has finished.
pub fn run_import(
    scan: &Scan,
    plan: &Plan,
    cfg: &Config,
    render: bool,
    events: EventSink,
    cancel: &Cancel,
    after_copy: Option<AfterCopy>,
) -> anyhow::Result<ImportReport> {
    let preset = if render && (cfg.outputs.exr || cfg.outputs.jpeg) {
        match resolve_preset(&cfg.preset).and_then(|p| Preset::load(&p)) {
            Ok(p) => Some(p),
            Err(e) => {
                events(JobEvent::RenderSkipped { reason: format!("preset unavailable: {e}") });
                None
            }
        }
    } else if render {
        events(JobEvent::RenderSkipped { reason: "no outputs selected in settings".into() });
        None
    } else {
        None
    };

    let mut manifest = Manifest::new(cfg, &scan.source.label, &scan.source.root, preset.as_ref().map(|p| p.record()));

    // ---- copy phase ----
    let to_copy: Vec<&crate::plan::PlannedFile> =
        plan.files.iter().filter(|f| !(cfg.skip_existing && f.status == DestStatus::ExistsSameSize)).collect();
    events(JobEvent::CopyStarted { files: to_copy.len(), bytes: to_copy.iter().map(|f| f.size).sum() });

    let (mut copied, mut skipped, mut failed, mut bytes) = (0usize, 0usize, 0usize, 0u64);
    for (idx, pf) in plan.files.iter().enumerate() {
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled during copy");
        }
        let sf = scan.file(pf.file);
        let group = scan.group(pf.group);
        let (status, hash) = if cfg.skip_existing && pf.status == DestStatus::ExistsSameSize {
            skipped += 1;
            (CopyStatus::SkippedExisting, None)
        } else {
            events(JobEvent::FileStarted { file: pf.file, dest: pf.dest.clone(), size: pf.size });
            let ev = events.clone();
            let fid = pf.file;
            match copy_file(&sf.path, &pf.dest, cfg.verify, |b| ev(JobEvent::FileProgress { file: fid, bytes_done: b })) {
                Ok(out) => {
                    copied += 1;
                    bytes += out.bytes;
                    (CopyStatus::Copied, Some(out.blake3))
                }
                Err(e) => {
                    failed += 1;
                    (CopyStatus::Failed(e.to_string()), None)
                }
            }
        };
        events(JobEvent::FileDone { file: pf.file, status: status.clone() });
        manifest.entries.push(Entry {
            id: idx as u32,
            group: pf.group.0,
            seq: plan.included.iter().position(|g| *g == pf.group).map(|i| i as u32 + 1).unwrap_or(0),
            kind: sf.kind,
            is_primary: group.primary == pf.file,
            source_rel: sf.rel.to_string_lossy().replace('\\', "/"),
            root: pf.root,
            rel: pf.rel.to_string_lossy().replace('\\', "/"),
            size: pf.size,
            blake3: hash,
            copy: status,
            meta: group.meta.clone(),
            preview: group.preview,
            outputs: Vec::new(),
        });
    }
    events(JobEvent::CopyFinished { copied, skipped, failed, bytes });

    let manifest_path = manifest.default_path();
    manifest.write(&manifest_path)?;
    events(JobEvent::ManifestWritten { path: manifest_path.clone() });
    if let Some(hook) = after_copy {
        hook(&manifest_path, &manifest);
    }

    // ---- render phase ----
    let (rendered, render_failed) = match preset {
        Some(preset) => {
            let tasks: Vec<u32> = manifest
                .entries
                .iter()
                .filter(|e| e.kind == Kind::Raw && e.is_primary && !matches!(e.copy, CopyStatus::Failed(_)))
                .map(|e| e.id)
                .collect();
            let cfg_for_render = cfg.clone();
            render_entries(&mut manifest, &manifest_path, &tasks, &preset, &cfg_for_render, &cfg.outputs, &cfg.render_root, events.clone(), cancel)?
        }
        None => (0, 0),
    };

    Ok(ImportReport { manifest_path, manifest, copied, skipped, failed, rendered, render_failed })
}

/// Re-render from a manifest, optionally overriding outputs and the render root. Uses the preset
/// recorded in the manifest, so the result matches the original import.
pub fn render_manifest(
    manifest_path: &Path,
    outputs: Option<Outputs>,
    render_root: Option<PathBuf>,
    only_missing: bool,
    events: EventSink,
    cancel: &Cancel,
) -> anyhow::Result<Manifest> {
    let mut manifest = Manifest::read(manifest_path)?;
    let preset = match &manifest.preset {
        Some(r) => Preset::from_record(r)?,
        None => {
            // The import ran without a usable preset; adopt the configured one and record it.
            let p = Preset::load(&resolve_preset(&manifest.config.preset)?)?;
            manifest.preset = Some(p.record());
            p
        }
    };
    let outputs = outputs.unwrap_or_else(|| manifest.config.outputs.clone());
    let render_root = render_root.unwrap_or_else(|| manifest.render_root.clone());
    let cfg = manifest.config.clone();
    let mut tasks: Vec<u32> = manifest.entries.iter().filter(|e| e.kind == Kind::Raw && e.is_primary).map(|e| e.id).collect();
    if only_missing {
        tasks.retain(|id| {
            let e = manifest.entries.iter().find(|e| e.id == *id).unwrap();
            let has = |k: OutputKind| e.outputs.iter().any(|o| o.kind == k && o.path().exists());
            (outputs.exr && !has(OutputKind::Exr)) || (outputs.jpeg && !has(OutputKind::Jpeg))
        });
    }
    render_entries(&mut manifest, manifest_path, &tasks, &preset, &cfg, &outputs, &render_root, events, cancel)?;
    Ok(manifest)
}

fn output_path(manifest: &Manifest, entry: &Entry, template: &Template, render_root: &Path) -> anyhow::Result<PathBuf> {
    let src = Path::new(&entry.source_rel);
    let ctx = Context {
        captured: entry.meta.captured_at,
        imported: manifest.created_at.naive_local(),
        camera: entry.meta.camera.clone(),
        make: entry.meta.make.clone(),
        model: entry.meta.model.clone(),
        stem: src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        ext: src.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        kind: entry.kind.as_str(),
        volume: manifest.source_label.clone(),
        rel_dir: src.parent().map(|p| p.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| ".".into()),
        seq: entry.seq,
        iso: entry.meta.iso,
        lens: entry.meta.lens.clone(),
        preset: None,
    };
    Ok(render_root.join(template.render(&ctx)?))
}

/// One RAW to render, with the outputs to write. `id` is echoed in `RenderDone`/`RenderFailed`
/// events (a manifest entry id for imports, an asset id for catalog prints).
#[derive(Debug, Clone)]
pub struct RenderTask {
    pub id: u32,
    pub src: PathBuf,
    pub exr: Option<PathBuf>,
    pub jpeg: Option<PathBuf>,
    /// The photo's RAW settings (white balance, exposure, highlights).
    pub raw: crate::decode::RawSettings,
}

#[derive(Debug, Clone)]
pub struct RenderResult {
    pub id: u32,
    pub outputs: Result<Vec<(OutputKind, PathBuf)>, String>,
    pub seconds: f32,
}

/// Update `manifest` outputs from render results and rewrite it.
#[allow(clippy::too_many_arguments)]
fn render_entries(
    manifest: &mut Manifest,
    manifest_path: &Path,
    task_ids: &[u32],
    preset: &Preset,
    cfg: &Config,
    outputs: &Outputs,
    render_root: &Path,
    events: EventSink,
    cancel: &Cancel,
) -> anyhow::Result<(usize, usize)> {
    if task_ids.is_empty() {
        events(JobEvent::RenderFinished { done: 0, failed: 0 });
        return Ok((0, 0));
    }
    let mut tasks = Vec::new();
    for id in task_ids {
        let e = manifest.entries.iter().find(|e| e.id == *id).unwrap();
        let exr = outputs.exr.then(|| output_path(manifest, e, &cfg.templates.render_exr, render_root)).transpose()?;
        let jpeg = outputs.jpeg.then(|| output_path(manifest, e, &cfg.templates.render_jpeg, render_root)).transpose()?;
        tasks.push(RenderTask { id: *id, src: manifest.resolve(manifest_path, e), exr, jpeg, raw: Default::default() });
    }
    let preset_hash = preset.hash();
    let results = render_tasks(tasks, preset, cfg, outputs, events.clone(), cancel)?;

    let (mut done, mut failed) = (0usize, 0usize);
    let now = Utc::now();
    for r in results {
        match r.outputs {
            Ok(written) => {
                done += 1;
                if let Some(entry) = manifest.entry_mut(r.id) {
                    for (kind, path) in &written {
                        let rel = path.strip_prefix(render_root).unwrap_or(path).to_string_lossy().replace('\\', "/");
                        entry.outputs.retain(|o| o.kind != *kind);
                        entry.outputs.push(OutputRecord {
                            kind: *kind,
                            root: render_root.to_path_buf(),
                            rel,
                            rendered_at: now,
                            preset_hash: preset_hash.clone(),
                        });
                    }
                }
            }
            Err(_) => failed += 1,
        }
    }

    manifest.write(manifest_path)?;
    events(JobEvent::ManifestWritten { path: manifest_path.to_path_buf() });
    events(JobEvent::RenderFinished { done, failed });
    Ok((done, failed))
}

/// Decode on `cfg.decode_threads` threads, render on the calling thread (one GPU), write on
/// two writer threads. Emits `RenderStarted` and one `RenderDone`/`RenderFailed` per task (not
/// `RenderFinished`: callers do their bookkeeping first). Returns one result per task that ran;
/// tasks skipped by a cancel are absent.
pub fn render_tasks(
    tasks: Vec<RenderTask>,
    preset: &Preset,
    cfg: &Config,
    outputs: &Outputs,
    events: EventSink,
    cancel: &Cancel,
) -> anyhow::Result<Vec<RenderResult>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let data_dir = find_data_dir(cfg.data_dir.as_deref())?;
    let develop = DevelopSettings::default();
    let renderer = Renderer::new(preset, &data_dir, outputs, develop.color_space_name())?;
    events(JobEvent::RenderStarted { total: tasks.len(), backend: renderer.backend_name().to_string() });

    let chroma = Chromaticities::for_space(&outputs.exr_color_space);
    let exr_space = outputs.exr_color_space.clone();
    let jpeg_quality = outputs.jpeg_quality;

    let (task_tx, task_rx) = crossbeam_channel::unbounded::<RenderTask>();
    let (dec_tx, dec_rx) = crossbeam_channel::bounded::<(RenderTask, Result<crate::decode::LinearImage, String>, Instant)>(1);
    let (wr_tx, wr_rx) = crossbeam_channel::bounded::<(RenderTask, crate::film::Rendered, Instant)>(2);
    let (res_tx, res_rx) = crossbeam_channel::unbounded::<RenderResult>();
    for t in tasks {
        task_tx.send(t).unwrap();
    }
    drop(task_tx);

    let mut results = Vec::new();
    std::thread::scope(|s| {
        // Decoders.
        for _ in 0..cfg.decode_threads.max(1) {
            let task_rx = task_rx.clone();
            let dec_tx = dec_tx.clone();
            let develop = develop.clone();
            let cancel = cancel.clone();
            s.spawn(move || {
                for task in task_rx.iter() {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let started = Instant::now();
                    let settings = DevelopSettings { demosaic: task.raw.demosaic.unwrap_or(develop.demosaic), raw: task.raw.clone(), ..develop.clone() };
                    let result = crate::decode::develop_any(&task.src, &settings).map_err(|e| e.to_string());
                    if dec_tx.send((task, result, started)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(dec_tx);

        // Writers. They emit the per-image events so progress is live.
        for _ in 0..2 {
            let wr_rx = wr_rx.clone();
            let res_tx = res_tx.clone();
            let exr_space = exr_space.clone();
            let events = events.clone();
            s.spawn(move || {
                for (task, rendered, started) in wr_rx.iter() {
                    let mut written = Vec::new();
                    let mut err: Option<String> = None;
                    if let (Some(path), Some(img)) = (&task.exr, &rendered.exr) {
                        match write_exr(path, img, chroma, &exr_space) {
                            Ok(()) => written.push((OutputKind::Exr, path.clone())),
                            Err(e) => err = Some(format!("exr: {e}")),
                        }
                    }
                    if let (Some(path), Some(img)) = (&task.jpeg, &rendered.jpeg) {
                        match write_jpeg(path, img, jpeg_quality) {
                            Ok(()) => written.push((OutputKind::Jpeg, path.clone())),
                            Err(e) => err = Some(format!("jpeg: {e}")),
                        }
                    }
                    let seconds = started.elapsed().as_secs_f32();
                    match &err {
                        Some(e) => events(JobEvent::RenderFailed { entry: task.id, error: e.clone() }),
                        None => events(JobEvent::RenderDone {
                            entry: task.id,
                            outputs: written.iter().map(|(_, p)| p.clone()).collect(),
                            seconds,
                        }),
                    }
                    let outputs = match err {
                        Some(e) => Err(e),
                        None => Ok(written),
                    };
                    let _ = res_tx.send(RenderResult { id: task.id, outputs, seconds });
                }
            });
        }
        drop(wr_rx);

        // Renderer (this thread).
        for (task, decoded, started) in dec_rx.iter() {
            if cancel.is_cancelled() {
                break;
            }
            match decoded {
                Ok(img) => {
                    let rendered = renderer.render(&img);
                    drop(img);
                    if wr_tx.send((task, rendered, started)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let error = format!("decode: {e}");
                    events(JobEvent::RenderFailed { entry: task.id, error: error.clone() });
                    let _ = res_tx.send(RenderResult { id: task.id, outputs: Err(error), seconds: started.elapsed().as_secs_f32() });
                }
            }
        }
        drop(wr_tx);
        drop(res_tx);
        results.extend(res_rx.iter());
    });
    Ok(results)
}

/// Decode one RAW and render it with a preset, for trying out a look. Returns the render time.
pub fn render_single(
    raw: &Path,
    preset_path: &Path,
    outputs: &Outputs,
    jpeg_out: Option<&Path>,
    exr_out: Option<&Path>,
    data_dir: Option<&Path>,
) -> anyhow::Result<f32> {
    let preset = Preset::load(&resolve_preset(preset_path)?)?;
    let data_dir = find_data_dir(data_dir)?;
    let settings = DevelopSettings::default();
    let mut file = RawFile::open(raw)?;
    let img = file.develop(&settings)?;
    let mut outs = outputs.clone();
    outs.jpeg = jpeg_out.is_some();
    outs.exr = exr_out.is_some();
    let renderer = Renderer::new(&preset, &data_dir, &outs, settings.color_space_name())?;
    let started = Instant::now();
    let rendered = renderer.render(&img);
    let secs = started.elapsed().as_secs_f32();
    if let (Some(p), Some(buf)) = (jpeg_out, &rendered.jpeg) {
        write_jpeg(p, buf, outs.jpeg_quality)?;
    }
    if let (Some(p), Some(buf)) = (exr_out, &rendered.exr) {
        write_exr(p, buf, Chromaticities::for_space(&outs.exr_color_space), &outs.exr_color_space)?;
    }
    Ok(secs)
}

/// Decode a RAW and write it straight out, no film stage. For checking the decode in Resolve.
pub fn decode_only(src: &Path, exr_out: Option<&Path>, jpeg_out: Option<&Path>) -> anyhow::Result<(u32, u32)> {
    let settings = DevelopSettings::default();
    let mut raw = RawFile::open(src)?;
    let img = raw.develop(&settings)?;
    let buf = || spektrafilm_math::image::ImageBuf::from_data(img.width, img.height, img.data.iter().map(|v| spektrafilm_math::precision::from_f32(*v)).collect());
    if let Some(p) = exr_out {
        write_exr(p, &buf(), Chromaticities::for_space(img.color_space), img.color_space)?;
    }
    if let Some(p) = jpeg_out {
        // Plain sRGB curve so the JPEG is viewable; primaries are left as decoded.
        let mut b = buf();
        for v in b.data.iter_mut() {
            *v = spektrafilm_math::precision::from_f32(srgb_encode(spektrafilm_math::precision::to_f32(*v)));
        }
        write_jpeg(p, &b, 92)?;
    }
    Ok((img.width, img.height))
}

fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}
