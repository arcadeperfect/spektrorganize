//! "Print" catalog assets: render their RAWs through a spektrafilm preset with the same
//! decode -> film -> write pipeline imports use (`job::render_tasks`), write into the configured
//! render root with the render templates, and record each output as a `renders` row.
//!
//! Output names come from `templates.render_jpeg` / `render_exr` (the `{preset}` token is
//! available). When that path already holds a render of the same asset with another preset (or
//! anything that is not this asset's), the preset name is appended (`DSCF0001-portra-400.jpg`),
//! so printing with a second look never overwrites the first.

use super::{Catalog, RootKind, now};
use crate::config::{Config, Outputs};
use crate::film::{Preset, resolve_preset};
use crate::job::{Cancel, EventSink, JobEvent, RenderTask, render_tasks};
use crate::manifest::OutputKind;
use crate::template::{Context, Template};
use chrono::NaiveDateTime;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize)]
pub struct PrintReport {
    pub requested: usize,
    pub rendered: usize,
    /// Camera JPEGs copied alongside (or instead of) the renders.
    pub copied: usize,
    pub failed: usize,
    /// Assets that could not be printed at all (not a RAW, file missing, drive offline).
    pub skipped: Vec<(i64, String)>,
    pub outputs: Vec<PathBuf>,
}

struct Source {
    asset: i64,
    /// A clip rather than a photo: printed by rendering frames, not by developing a RAW.
    is_video: bool,
    raw: PathBuf,
    captured: Option<NaiveDateTime>,
    camera: Option<String>,
    make: Option<String>,
    model: Option<String>,
    lens: Option<String>,
    iso: Option<u32>,
    rel: String,
    root_label: String,
    added_at: String,
    raw_settings: crate::decode::RawSettings,
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() { "preset".into() } else { s }
}

fn with_suffix(rel: &str, suffix: &str) -> String {
    let p = Path::new(rel);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("render");
    let name = match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{suffix}.{ext}"),
        None => format!("{stem}-{suffix}"),
    };
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => format!("{}/{name}", super::rel_string(d)),
        None => name,
    }
}

/// How clips in a print run are rendered. Photos ignore this entirely.
#[derive(Debug, Clone, Copy)]
pub struct VideoPrint {
    pub look: crate::video::render::LookMode,
    pub codec: crate::video::render::OutCodec,
    /// Longest edge; 0 keeps the clip's own size.
    pub max_px: u32,
    pub mbps: f32,
}

impl Default for VideoPrint {
    fn default() -> Self {
        // A baked cube at posting size: the choice that finishes, for a batch.
        VideoPrint {
            look: crate::video::render::LookMode::Lut,
            codec: crate::video::render::OutCodec::H264,
            max_px: 1920,
            mbps: 12.0,
        }
    }
}

/// What one export run should produce.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// The look to print with; `None` renders nothing (camera JPEGs only).
    /// Use [`crate::film::DEVELOPED_ONLY`] for the develop stage's own output.
    pub look: Option<PathBuf>,
    pub jpeg: bool,
    pub exr: bool,
    /// Also copy the photo's camera JPEG, when it has one.
    pub camera_jpeg: bool,
    /// Where to write; the configured render root when `None`.
    pub destination: Option<PathBuf>,
    /// How any clips in the run are rendered.
    pub video: VideoPrint,
}

/// Render and/or copy the camera JPEGs of `assets` in one run.
pub fn export_assets(
    cat: &Catalog,
    cfg: &Config,
    assets: &[i64],
    opts: &ExportOptions,
    events: EventSink,
    cancel: &Cancel,
) -> anyhow::Result<PrintReport> {
    let renders = opts.look.is_some() && (opts.jpeg || opts.exr);
    if !renders && !opts.camera_jpeg {
        anyhow::bail!("choose what to export");
    }
    let mut cfg = cfg.clone();
    if let Some(dest) = &opts.destination {
        cfg.render_root = dest.clone();
    }
    let mut report = if renders {
        let outputs = Outputs { jpeg: opts.jpeg, exr: opts.exr, ..cfg.outputs.clone() };
        print_assets(cat, &cfg, assets, opts.look.as_deref().unwrap(), &outputs, opts.video, events.clone(), cancel)?
    } else {
        PrintReport { requested: assets.len(), ..Default::default() }
    };
    if opts.camera_jpeg {
        copy_camera_jpegs(cat, &cfg, assets, renders, &mut report, &events, cancel)?;
    }
    Ok(report)
}

/// Copy each photo's camera JPEG into the export root, named by the JPEG
/// render template. With a render in the same run the copy gets a `-camera`
/// suffix so the two never collide.
fn copy_camera_jpegs(
    cat: &Catalog,
    cfg: &Config,
    assets: &[i64],
    suffix: bool,
    report: &mut PrintReport,
    events: &EventSink,
    cancel: &Cancel,
) -> anyhow::Result<()> {
    let render_root_id = cat.ensure_root(&cfg.render_root, RootKind::Render)?;
    let render_root = cat.root(render_root_id)?.path;
    let stamp = now();
    for &asset in assets {
        if cancel.is_cancelled() {
            break;
        }
        let (src, source) = match camera_jpeg(cat, asset)? {
            Some(v) => v,
            None => {
                events(JobEvent::Log(format!("#{asset}: no camera JPEG")));
                report.skipped.push((asset, "no camera JPEG".into()));
                continue;
            }
        };
        let ctx = context(&source, &Preset { name: "camera".into(), film: String::new(), print: String::new(), params: serde_json::Value::Null });
        let rel = cfg.templates.render_jpeg.render(&ctx)?;
        let mut path = render_root.join(&rel);
        let ext = src.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "jpg".into());
        let stem = path.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        path.set_file_name(format!("{stem}{}.{ext}", if suffix { "-camera" } else { "" }));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        match crate::copy::copy_file(&src, &path, cfg.verify, |_| {}) {
            Ok(_) => {
                report.copied += 1;
                let rel = super::rel_string(path.strip_prefix(&render_root).unwrap_or(&path));
                cat.conn().execute(
                    "INSERT INTO renders (asset_id, kind, root_id, rel, preset_name, preset_hash, created_at)
                     VALUES (?1, 'camera', ?2, ?3, 'camera JPEG', 'camera', ?4)
                     ON CONFLICT (root_id, rel) DO UPDATE SET asset_id = excluded.asset_id, kind = excluded.kind,
                        preset_name = excluded.preset_name, created_at = excluded.created_at",
                    params![asset, render_root_id, rel, stamp],
                )?;
                events(JobEvent::Log(format!("#{asset}: copied {}", path.display())));
                report.outputs.push(path);
            }
            Err(e) => {
                report.failed += 1;
                events(JobEvent::Log(format!("#{asset}: copy failed: {e}")));
            }
        }
    }
    Ok(())
}

/// The photo's camera JPEG (or other image file) and its source row.
fn camera_jpeg(cat: &Catalog, asset: i64) -> anyhow::Result<Option<(PathBuf, Source)>> {
    let Ok(source) = load_source(cat, asset)? else { return Ok(None) };
    let row: Option<(String, String, i64, Option<i64>)> = cat
        .conn()
        .query_row(
            "SELECT f.rel, r.path, f.missing, r.online
             FROM asset_files af JOIN files f ON f.id = af.file_id JOIN roots r ON r.id = f.root_id
             WHERE af.asset_id = ?1 AND af.role IN ('jpeg', 'image')
             ORDER BY CASE af.role WHEN 'jpeg' THEN 0 ELSE 1 END LIMIT 1",
            [asset],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((rel, root, missing, online)) = row else { return Ok(None) };
    if missing == 1 || online == Some(0) {
        return Ok(None);
    }
    Ok(Some((Path::new(&root).join(rel), source)))
}

/// The file to render for a catalog photo (its RAW, else its image), or why it cannot be rendered.
pub fn raw_path(cat: &Catalog, asset: i64) -> anyhow::Result<Result<PathBuf, String>> {
    Ok(load_source(cat, asset)?.map(|s| s.raw))
}

fn load_source(cat: &Catalog, asset: i64) -> anyhow::Result<Result<Source, String>> {
    let row = cat
        .conn()
        .query_row(
            "SELECT a.kind, a.captured_at, a.camera, a.make, a.model, a.lens, a.iso, a.added_at,
                    f.rel, f.missing, r.path, r.online, r.label
             FROM assets a
             LEFT JOIN asset_files af ON af.asset_id = a.id AND af.file_id = (
                 SELECT file_id FROM asset_files
                 WHERE asset_id = a.id AND role IN ('raw', 'jpeg', 'image', 'video')
                 ORDER BY CASE role WHEN 'raw' THEN 0 WHEN 'video' THEN 2 ELSE 1 END LIMIT 1)
             LEFT JOIN files f ON f.id = af.file_id
             LEFT JOIN roots r ON r.id = f.root_id
             WHERE a.id = ?1",
            [asset],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<i64>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, captured, camera, make, model, lens, iso, added_at, rel, missing, root, online, label)) = row else {
        return Ok(Err("not in the catalog".into()));
    };
    let (Some(rel), Some(root)) = (rel, root) else {
        return Ok(Err(format!("nothing printable here ({kind})")));
    };
    if online == Some(0) {
        return Ok(Err(format!("drive offline ({root})")));
    }
    if missing == Some(1) {
        return Ok(Err(format!("file missing ({rel})")));
    }
    Ok(Ok(Source {
        asset,
        is_video: kind == "video",
        raw: Path::new(&root).join(&rel),
        captured: captured.and_then(|s| NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S").ok()),
        camera,
        make,
        model,
        lens,
        iso: iso.map(|i| i as u32),
        rel,
        root_label: label.unwrap_or_default(),
        added_at,
        raw_settings: cat.raw_settings(asset)?,
    }))
}

/// Swap a path's extension, keeping the rest of what the template produced.
fn with_extension(rel: &str, ext: &str) -> String {
    let p = Path::new(rel);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("render");
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => format!("{}/{stem}.{ext}", super::rel_string(d)),
        None => format!("{stem}.{ext}"),
    }
}

/// Render each clip through the look and record it as a print of that photo.
#[allow(clippy::too_many_arguments)]
fn print_clips(
    cat: &Catalog,
    cfg: &Config,
    clips: &[(Source, String)],
    preset: &Preset,
    video: VideoPrint,
    render_root_id: i64,
    render_root: &Path,
    report: &mut PrintReport,
    events: &EventSink,
    cancel: &Cancel,
) -> anyhow::Result<()> {
    if clips.is_empty() {
        return Ok(());
    }
    if !crate::video::supported() {
        for (src, _) in clips {
            report.skipped.push((src.asset, "video needs macOS".into()));
        }
        return Ok(());
    }
    let data_dir = crate::film::find_data_dir(cfg.data_dir.as_deref())?;
    let stamp = now();
    for (src, rel) in clips {
        if cancel.is_cancelled() {
            break;
        }
        let dst = render_root.join(rel);
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let req = crate::video::render::RenderRequest {
            src: src.raw.clone(),
            dst: dst.clone(),
            codec: video.codec,
            look: video.look,
            max_px: video.max_px,
            mbps: video.mbps,
            audio: false,
        };
        events(JobEvent::Log(format!("rendering {}", src.rel)));
        let name = src.rel.clone();
        match crate::video::render::render(&req, Some(preset), &data_dir, &|_, _| {}, &|| cancel.is_cancelled()) {
            Ok(out) => {
                report.rendered += 1;
                report.outputs.push(dst.clone());
                cat.conn().execute(
                    "INSERT INTO renders (asset_id, kind, root_id, rel, preset_name, preset_hash, created_at)
                     VALUES (?1, 'video', ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (root_id, rel) DO UPDATE SET asset_id = excluded.asset_id, preset_name = excluded.preset_name,
                                                             preset_hash = excluded.preset_hash, created_at = excluded.created_at",
                    params![src.asset, render_root_id, rel, preset.name, preset.hash(), stamp],
                )?;
                events(JobEvent::Log(format!("{name}: {} frames in {:.1}s", out.frames, out.seconds)));
            }
            Err(e) => {
                report.failed += 1;
                events(JobEvent::Log(format!("{name}: {e}")));
                report.skipped.push((src.asset, e.to_string()));
            }
        }
    }
    Ok(())
}

fn context(s: &Source, preset: &Preset) -> Context {
    let p = Path::new(&s.rel);
    Context {
        captured: s.captured,
        imported: chrono::DateTime::parse_from_rfc3339(&s.added_at)
            .map(|t| t.with_timezone(&chrono::Local).naive_local())
            .unwrap_or_else(|_| chrono::Local::now().naive_local()),
        camera: s.camera.clone(),
        make: s.make.clone(),
        model: s.model.clone(),
        stem: p.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default(),
        ext: p.extension().map(|x| x.to_string_lossy().to_string()).unwrap_or_default(),
        kind: "raw",
        volume: s.root_label.clone(),
        rel_dir: p.parent().map(super::rel_string).filter(|d| !d.is_empty()).unwrap_or_else(|| ".".into()),
        seq: 0,
        iso: s.iso,
        lens: s.lens.clone(),
        preset: Some(preset.name.clone()),
    }
}

/// Where to write one output: the template path unless something else lives there, then the
/// same name with the preset appended (and a counter if even that is taken).
fn choose_rel(cat: &Catalog, root_id: i64, root: &Path, want: String, asset: i64, preset: &Preset, taken: &mut HashSet<String>) -> anyhow::Result<String> {
    let hash = preset.hash();
    let ours = |rel: &str| -> anyhow::Result<bool> {
        let owner: Option<(i64, String)> = cat
            .conn()
            .query_row("SELECT asset_id, preset_hash FROM renders WHERE root_id = ?1 AND rel = ?2", params![root_id, rel], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(match owner {
            Some((a, h)) => a == asset && h == hash,
            None => !root.join(rel).exists(),
        })
    };
    let mut candidates = vec![want.clone(), with_suffix(&want, &slug(&preset.name))];
    for n in 2..100 {
        candidates.push(with_suffix(&want, &format!("{}-{n}", slug(&preset.name))));
    }
    for c in candidates {
        if !taken.contains(&c) && ours(&c)? {
            taken.insert(c.clone());
            return Ok(c);
        }
    }
    anyhow::bail!("no free output name for {want}")
}

/// Render `assets` with the preset at/named `preset` into `cfg.render_root`. Progress goes to
/// `events` as `RenderStarted`/`RenderDone{entry: asset id}`/`RenderFailed`/`RenderFinished`.
pub fn print_assets(
    cat: &Catalog,
    cfg: &Config,
    assets: &[i64],
    preset: &Path,
    outputs: &Outputs,
    video: VideoPrint,
    events: EventSink,
    cancel: &Cancel,
) -> anyhow::Result<PrintReport> {
    if !outputs.jpeg && !outputs.exr {
        anyhow::bail!("choose JPEG and/or EXR output");
    }
    // "Developed only" renders through the passthrough route: no film look.
    let developed = preset == Path::new(crate::film::DEVELOPED_ONLY);
    let preset = if developed {
        let film = Preset::load(&resolve_preset(&cfg.preset)?).map(|p| p.film).unwrap_or_else(|_| "kodak_portra_400".into());
        crate::film::developed_preset(&film)
    } else {
        Preset::load(&resolve_preset(preset)?)?
    };
    std::fs::create_dir_all(&cfg.render_root)?;
    let render_root_id = cat.ensure_root(&cfg.render_root, RootKind::Render)?;
    let render_root = cat.root(render_root_id)?.path;
    let mut report = PrintReport { requested: assets.len(), ..Default::default() };

    let mut tasks = Vec::new();
    let mut clips: Vec<(Source, String)> = Vec::new();
    let mut taken = HashSet::new();
    let mut planned: Vec<(i64, Option<String>, Option<String>)> = Vec::new();
    for &asset in assets {
        let src = match load_source(cat, asset)? {
            Ok(s) => s,
            Err(why) => {
                events(JobEvent::Log(format!("skipped #{asset}: {why}")));
                report.skipped.push((asset, why));
                continue;
            }
        };
        let ctx = context(&src, &preset);
        let mut rel_for = |t: &Template| -> anyhow::Result<String> { choose_rel(cat, render_root_id, &render_root, t.render(&ctx)?, asset, &preset, &mut taken) };
        if src.is_video {
            // A clip is rendered frame by frame; the template names it, with the codec's own
            // extension in place of the stills one.
            let rel = with_extension(&rel_for(&cfg.templates.render_jpeg)?, video.codec.extension());
            clips.push((src, rel));
            continue;
        }
        let jpeg = outputs.jpeg.then(|| rel_for(&cfg.templates.render_jpeg)).transpose()?;
        let exr = outputs.exr.then(|| rel_for(&cfg.templates.render_exr)).transpose()?;
        tasks.push(RenderTask {
            id: src.asset as u32,
            src: src.raw.clone(),
            jpeg: jpeg.as_ref().map(|r| render_root.join(r)),
            exr: exr.as_ref().map(|r| render_root.join(r)),
            raw: src.raw_settings.clone(),
        });
        planned.push((asset, jpeg, exr));
    }

    let results = render_tasks(tasks, &preset, cfg, outputs, events.clone(), cancel)?;
    print_clips(cat, cfg, &clips, &preset, video, render_root_id, &render_root, &mut report, &events, cancel)?;
    let record = preset.record();
    cat.conn().execute(
        "INSERT OR IGNORE INTO presets (hash, name, record, added_at) VALUES (?1, ?2, ?3, ?4)",
        params![record.hash, record.name, serde_json::to_string(&record)?, now()],
    )?;
    let stamp = now();
    for r in results {
        match r.outputs {
            Ok(written) => {
                report.rendered += 1;
                let asset = r.id as i64;
                for (kind, path) in written {
                    let rel = super::rel_string(path.strip_prefix(&render_root).unwrap_or(&path));
                    cat.conn().execute(
                        "INSERT INTO renders (asset_id, kind, root_id, rel, preset_name, preset_hash, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT (root_id, rel) DO UPDATE SET asset_id = excluded.asset_id, kind = excluded.kind,
                            preset_name = excluded.preset_name, preset_hash = excluded.preset_hash, created_at = excluded.created_at",
                        params![asset, if kind == OutputKind::Jpeg { "jpeg" } else { "exr" }, render_root_id, rel, preset.name, record.hash, stamp],
                    )?;
                    report.outputs.push(path);
                }
            }
            Err(_) => report.failed += 1,
        }
    }
    events(JobEvent::RenderFinished { done: report.rendered, failed: report.failed });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_get_the_preset_when_taken() {
        assert_eq!(slug("Portra 400 on Portra Endura"), "portra-400-on-portra-endura");
        assert_eq!(with_suffix("2024/2024-10-04/X-T10/DSCF0001.jpg", "portra"), "2024/2024-10-04/X-T10/DSCF0001-portra.jpg");
        assert_eq!(with_suffix("a.exr", "x-2"), "a-x-2.exr");

        let tmp = tempfile::tempdir().unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let root = tmp.path().join("renders");
        std::fs::create_dir_all(&root).unwrap();
        let root_id = cat.ensure_root(&root, RootKind::Render).unwrap();
        let root = cat.root(root_id).unwrap().path;
        cat.conn().execute("INSERT INTO assets (kind, added_at) VALUES ('raw', 'now'), ('raw', 'now')", []).unwrap();
        let preset = Preset { name: "Velvia 50".into(), film: "f".into(), print: "p".into(), params: Default::default() };
        let mut taken = HashSet::new();
        // Free: the template path.
        assert_eq!(choose_rel(&cat, root_id, &root, "d/A.jpg".into(), 1, &preset, &mut taken).unwrap(), "d/A.jpg");
        // Same batch, same path -> suffixed.
        assert_eq!(choose_rel(&cat, root_id, &root, "d/A.jpg".into(), 2, &preset, &mut taken).unwrap(), "d/A-velvia-50.jpg");
        // A render of this asset with this preset is overwritten in place; another preset's is not.
        cat.conn()
            .execute("INSERT INTO renders (asset_id, kind, root_id, rel, preset_hash, created_at) VALUES (1, 'jpeg', ?1, 'd/B.jpg', ?2, 'now')", params![root_id, preset.hash()])
            .unwrap();
        cat.conn()
            .execute("INSERT INTO renders (asset_id, kind, root_id, rel, preset_hash, created_at) VALUES (1, 'jpeg', ?1, 'd/C.jpg', 'other', 'now')", params![root_id])
            .unwrap();
        let mut taken = HashSet::new();
        assert_eq!(choose_rel(&cat, root_id, &root, "d/B.jpg".into(), 1, &preset, &mut taken).unwrap(), "d/B.jpg");
        assert_eq!(choose_rel(&cat, root_id, &root, "d/C.jpg".into(), 1, &preset, &mut taken).unwrap(), "d/C-velvia-50.jpg");
        // An unrelated file on disk is never overwritten.
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d/D.jpg"), b"someone else's").unwrap();
        assert_eq!(choose_rel(&cat, root_id, &root, "d/D.jpg".into(), 1, &preset, &mut taken).unwrap(), "d/D-velvia-50.jpg");
    }
}
