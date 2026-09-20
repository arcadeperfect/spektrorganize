use anyhow::Context as _;
use clap::{Parser, Subcommand};
use spektro_core::catalog::Catalog;
use spektro_core::catalog::index::IndexProgress;
use spektro_core::config::Config;
use spektro_core::job::{Cancel, JobEvent};
use spektro_core::plan::{DestStatus, TreeNode};
use spektro_core::scan::{GroupId, ScanProgress};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "spektro", version, about = "Import camera cards and batch-render them through spektrafilm")]
struct Cli {
    /// Config file (TOML). Defaults to ~/.config/spektrorganize/config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Catalog database. Defaults to the desktop app's (<data dir>/dev.alexharding.spektrorganize/catalog.sqlite).
    #[arg(long, global = true)]
    catalog: Option<PathBuf>,
    /// Thumbnail cache for the catalog. Defaults to the desktop app's (<cache dir>/dev.alexharding.spektrorganize/catalog-thumbs).
    #[arg(long, global = true)]
    thumbs_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List mounted cards (volumes with a DCIM folder).
    Sources,
    /// Scan a card or folder and list what was found.
    Scan {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show where everything would go, without copying.
    Plan {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Copy, write the manifest, then render.
    Import {
        path: PathBuf,
        /// Copy only, no film renders.
        #[arg(long)]
        no_render: bool,
        /// Group ids to leave out (from `scan`).
        #[arg(long, value_delimiter = ',')]
        exclude: Vec<u32>,
        /// Do not record the import in the catalog.
        #[arg(long)]
        no_catalog: bool,
        /// What this import is, kept on every photo it brings in.
        #[arg(long)]
        description: Option<String>,
    },
    /// Re-render the RAWs recorded in a manifest.
    Render {
        manifest: PathBuf,
        #[arg(long)]
        exr: bool,
        #[arg(long)]
        jpeg: bool,
        /// Write renders here instead of the recorded render root.
        #[arg(long)]
        render_root: Option<PathBuf>,
        /// Skip entries whose requested outputs already exist.
        #[arg(long)]
        only_missing: bool,
    },
    /// Render one RAW with a preset, for trying out a look.
    Try {
        raw: PathBuf,
        /// Preset file or name (looked up in the preset folders). Defaults to the configured one.
        #[arg(long)]
        preset: Option<PathBuf>,
        #[arg(long)]
        jpeg: Option<PathBuf>,
        #[arg(long)]
        exr: Option<PathBuf>,
    },
    /// Fast look preview of one RAW (half-size decode, GPU), as a JPEG. With
    /// `--wb`/`--temp`/`--ev` to try RAW settings.
    Preview {
        raw: PathBuf,
        #[arg(long)]
        preset: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Long edge in pixels.
        #[arg(long, default_value_t = 1600)]
        max: u32,
        /// White balance: as_shot | daylight | tungsten | custom.
        #[arg(long, default_value = "as_shot")]
        wb: String,
        #[arg(long, default_value_t = 5500.0)]
        temp: f64,
        #[arg(long, default_value_t = 0.0)]
        ev: f64,
        /// Render this many times (timing the cached path).
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        /// Render the "before" view (RAW settings only, no film stage).
        #[arg(long)]
        before: bool,
    },
    /// Decode one RAW with the honest decode and write it without the film stage.
    Decode {
        raw: PathBuf,
        #[arg(long)]
        exr: Option<PathBuf>,
        #[arg(long)]
        jpeg: Option<PathBuf>,
    },
    /// Write a default config file.
    Init {
        #[arg(default_value = "config.toml")]
        path: PathBuf,
    },
    /// List film and paper profiles available to the film stage.
    Profiles,
    /// The photo catalog (SQLite): index folders and archives, keywords, prints.
    Catalog {
        #[command(subcommand)]
        cmd: CatalogCmd,
    },
}

#[derive(Subcommand)]
enum CatalogCmd {
    /// Index an existing folder in place (never moved or renamed). Re-running rescans it.
    Add { folder: PathBuf },
    /// Adopt an import archive from its manifests, including recorded renders. Read-only.
    Adopt {
        /// Archive root. Defaults to the configured archive_root.
        archive: Option<PathBuf>,
    },
    /// Rescan a root (new, changed, moved and vanished files).
    Rescan { root: i64 },
    /// List roots.
    Roots,
    /// Point a root at its new location after moving it.
    Relocate {
        root: i64,
        path: PathBuf,
        /// Skip the check that its files are really there.
        #[arg(long)]
        force: bool,
    },
    /// Forget a root and its assets (files on disk are not touched).
    RemoveRoot { root: i64 },
    /// List assets, newest first.
    Ls {
        #[arg(long)]
        keyword: Vec<String>,
        #[arg(long)]
        camera: Vec<String>,
        #[arg(long)]
        root: Vec<i64>,
        /// YYYY-MM-DD, inclusive.
        #[arg(long)]
        from: Option<String>,
        /// YYYY-MM-DD, inclusive.
        #[arg(long)]
        to: Option<String>,
        /// Words to find in file paths, keywords, camera, lens.
        #[arg(long)]
        text: Option<String>,
        /// Only assets that have renders.
        #[arg(long)]
        rendered: bool,
        #[arg(long, default_value_t = 100)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
        #[arg(long)]
        json: bool,
    },
    /// Everything about one asset.
    Show {
        id: i64,
        #[arg(long)]
        json: bool,
    },
    /// Add and/or remove keywords on assets.
    Tag {
        #[arg(required = true)]
        ids: Vec<i64>,
        #[arg(long)]
        add: Vec<String>,
        #[arg(long)]
        remove: Vec<String>,
    },
    /// Render assets through a preset into the render root and record the prints.
    Print {
        #[arg(required = true)]
        ids: Vec<i64>,
        /// Preset file or name (looked up in the preset folders).
        #[arg(long)]
        preset: PathBuf,
        #[arg(long)]
        jpeg: bool,
        #[arg(long)]
        exr: bool,
        /// Also copy each photo's camera JPEG, when it has one.
        #[arg(long)]
        camera_jpeg: bool,
    },
    /// Generate missing thumbnails.
    Thumbs {
        #[arg(long, default_value_t = 256)]
        size: u32,
        /// Redo existing and failed ones too.
        #[arg(long)]
        force: bool,
    },
}

fn default_config_path() -> PathBuf {
    spektro_core::config::dirs_home().join(".config/spektrorganize/config.toml")
}

fn load_config(cli: &Cli) -> anyhow::Result<Config> {
    let path = cli.config.clone().unwrap_or_else(default_config_path);
    if path.exists() {
        Config::load(&path).with_context(|| format!("loading {}", path.display()))
    } else {
        eprintln!("no config at {}, using defaults (run `spektro init`)", path.display());
        Ok(Config::default())
    }
}

fn scan_path(path: &PathBuf) -> anyhow::Result<spektro_core::scan::Scan> {
    let mut last = 0;
    let scan = spektro_core::scan::scan(path, &spektro_core::decode::read_meta, |p| match p {
        ScanProgress::Walking { files } => {
            if files / 200 != last {
                last = files / 200;
                eprint!("\rwalking: {files} files");
            }
        }
        ScanProgress::ReadingMetadata { done, total } => eprint!("\rmetadata: {done}/{total}\n"),
    })?;
    eprintln!();
    Ok(scan)
}

fn print_tree(nodes: &[TreeNode]) {
    fn rec(n: &TreeNode, depth: usize) {
        let flag = match &n.status {
            Some(DestStatus::ExistsSameSize) => "  [exists, skip]",
            Some(DestStatus::ExistsDifferent) => "  [exists, differs -> suffixed]",
            Some(DestStatus::Collision) => "  [collision -> suffixed]",
            _ => "",
        };
        if n.is_dir {
            println!("{}{}/  ({} files, {})", "  ".repeat(depth), n.name, n.files, human(n.bytes));
        } else {
            println!("{}{}{}", "  ".repeat(depth), n.name, flag);
        }
        for c in &n.children {
            rec(c, depth + 1);
        }
    }
    for n in nodes {
        rec(n, 0);
    }
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

fn event_printer() -> spektro_core::job::EventSink {
    Arc::new(|e: JobEvent| {
        let mut out = std::io::stderr().lock();
        let _ = match e {
            JobEvent::CopyStarted { files, bytes } => writeln!(out, "copy: {files} files, {}", human(bytes)),
            JobEvent::FileStarted { dest, size, .. } => write!(out, "  -> {} ({})", dest.display(), human(size)),
            JobEvent::FileProgress { .. } => Ok(()),
            JobEvent::FileDone { status, .. } => writeln!(out, "  {status:?}"),
            JobEvent::CopyFinished { copied, skipped, failed, bytes } => {
                writeln!(out, "copy done: {copied} copied, {skipped} skipped, {failed} failed, {}", human(bytes))
            }
            JobEvent::ManifestWritten { path } => writeln!(out, "manifest: {}", path.display()),
            JobEvent::RenderStarted { total, backend } => writeln!(out, "render: {total} images on {backend}"),
            JobEvent::RenderDone { entry, outputs, seconds } => {
                writeln!(out, "  #{entry} {:.1}s -> {}", seconds, outputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))
            }
            JobEvent::RenderFailed { entry, error } => writeln!(out, "  #{entry} FAILED: {error}"),
            JobEvent::RenderFinished { done, failed } => writeln!(out, "render done: {done} ok, {failed} failed"),
            JobEvent::RenderSkipped { reason } => writeln!(out, "RENDER SKIPPED: {reason}"),
            JobEvent::Log(s) => writeln!(out, "{s}"),
        };
    })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).init();
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Sources => {
            for s in spektro_core::scan::list_sources() {
                println!("{}\t{}", s.label, s.root.display());
            }
        }
        Cmd::Scan { path, json } => {
            let scan = scan_path(path)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&scan)?);
            } else {
                for g in &scan.groups {
                    let p = scan.file(g.primary);
                    let date = g.meta.captured_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_else(|| "undated".into());
                    let attach: Vec<String> = g.attachments.iter().map(|id| scan.file(*id).rel.file_name().unwrap_or_default().to_string_lossy().to_string()).collect();
                    println!(
                        "{:>4}  {:<7} {}  {:<12} {}{}",
                        g.id.0,
                        g.kind.as_str(),
                        date,
                        g.meta.camera.as_deref().unwrap_or("-"),
                        p.rel.display(),
                        if attach.is_empty() { String::new() } else { format!("  +{}", attach.join(",")) }
                    );
                }
                println!("{} files, {} groups, {}", scan.files.len(), scan.groups.len(), human(scan.total_bytes()));
                for (p, e) in &scan.errors {
                    eprintln!("skipped {}: {e}", p.display());
                }
            }
        }
        Cmd::Plan { path, json } => {
            let cfg = load_config(&cli)?;
            let scan = scan_path(path)?;
            let included: Vec<GroupId> = scan.groups.iter().map(|g| g.id).collect();
            let plan = spektro_core::plan::plan(&scan, &included, &cfg, chrono_now())?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            } else {
                print_tree(&spektro_core::plan::tree(&plan, &cfg));
                println!("{} files, {} to copy", plan.files.len(), human(plan.bytes_to_copy(cfg.skip_existing)));
            }
        }
        Cmd::Import { path, no_render, exclude, no_catalog, description } => {
            let cfg = load_config(&cli)?;
            let scan = scan_path(path)?;
            let included: Vec<GroupId> = scan.groups.iter().map(|g| g.id).filter(|g| !exclude.contains(&g.0)).collect();
            let plan = spektro_core::plan::plan(&scan, &included, &cfg, chrono_now())?;
            let catalog_path = (!no_catalog).then(|| catalog_path(&cli));
            let index = |manifest_path: &std::path::Path, m: &spektro_core::manifest::Manifest| {
                let Some(db) = &catalog_path else { return };
                match Catalog::open(db).and_then(|mut c| spektro_core::catalog::index::index_manifest(&mut c, manifest_path, m)) {
                    Ok(r) => eprintln!("catalog: {} assets ({} new), {} renders -> {}", r.assets, r.assets_created, r.renders, db.display()),
                    Err(e) => eprintln!("catalog: indexing failed: {e:#}"),
                }
            };
            let report = spektro_core::job::run_import(&scan, &plan, &cfg, !no_render, event_printer(), &Cancel::new(), Some(&index), description.as_deref())?;
            if report.rendered > 0 {
                index(&report.manifest_path, &report.manifest);
            }
            println!(
                "copied {} skipped {} failed {}; rendered {} failed {}; manifest {}",
                report.copied,
                report.skipped,
                report.failed,
                report.rendered,
                report.render_failed,
                report.manifest_path.display()
            );
        }
        Cmd::Render { manifest, exr, jpeg, render_root, only_missing } => {
            let outputs = if *exr || *jpeg {
                let m = spektro_core::manifest::Manifest::read(manifest)?;
                let mut o = m.config.outputs.clone();
                o.exr = *exr;
                o.jpeg = *jpeg;
                Some(o)
            } else {
                None
            };
            spektro_core::job::render_manifest(manifest, outputs, render_root.clone(), *only_missing, event_printer(), &Cancel::new())?;
        }
        Cmd::Try { raw, preset, jpeg, exr } => {
            if exr.is_none() && jpeg.is_none() {
                anyhow::bail!("give --jpeg and/or --exr output paths");
            }
            let cfg = load_config(&cli)?;
            let preset = preset.clone().unwrap_or_else(|| cfg.preset.clone());
            let secs = spektro_core::job::render_single(raw, &preset, &cfg.outputs, jpeg.as_deref(), exr.as_deref(), cfg.data_dir.as_deref())?;
            println!("rendered in {secs:.1}s (plus decode) with {}", preset.display());
        }
        Cmd::Preview { raw, preset, out, max, wb, temp, ev, repeat, before } => {
            let cfg = load_config(&cli)?;
            let preset_path = spektro_core::film::resolve_preset(preset.as_ref().unwrap_or(&cfg.preset))?;
            let look = spektro_core::film::Preset::load(&preset_path)?;
            let data_dir = spektro_core::film::find_data_dir(cfg.data_dir.as_deref())?;
            let raw_settings = spektro_core::decode::RawSettings {
                white_balance: serde_json::from_value(serde_json::Value::String(wb.clone()))?,
                temperature: *temp,
                exposure_ev: *ev,
                ..Default::default()
            };
            let mut engine = spektro_core::look::PreviewEngine::default();
            for i in 0..(*repeat).max(1) {
                let t = std::time::Instant::now();
                let jpg = if *before {
                    engine.render_before(raw, &raw_settings, &look, &data_dir, *max)?
                } else {
                    engine.render(raw, &raw_settings, &look, &data_dir, *max)?
                };
                std::fs::write(out, &jpg)?;
                println!("preview {} in {:.0} ms ({})", i + 1, t.elapsed().as_secs_f64() * 1000.0, engine.backend_name());
            }
        }
        Cmd::Decode { raw, exr, jpeg } => {
            if exr.is_none() && jpeg.is_none() {
                anyhow::bail!("give --exr and/or --jpeg output paths");
            }
            let (w, h) = spektro_core::job::decode_only(raw, exr.as_deref(), jpeg.as_deref())?;
            println!("decoded {w}x{h} with LibRaw {}", spektro_core::decode::libraw_version());
        }
        Cmd::Init { path } => {
            let cfg = Config::default();
            cfg.save(path)?;
            println!("wrote {}", path.display());
        }
        Cmd::Catalog { cmd } => catalog_cmd(&cli, cmd)?,
        Cmd::Profiles => {
            let cfg = load_config(&cli)?;
            let dir = spektro_core::film::find_data_dir(cfg.data_dir.as_deref())?;
            let (films, papers) = spektro_core::film::list_profiles(&dir)?;
            println!("data: {}", dir.display());
            println!("films:  {}", films.join(", "));
            println!("papers: {}", papers.join(", "));
        }
    }
    Ok(())
}

fn catalog_path(cli: &Cli) -> PathBuf {
    cli.catalog.clone().unwrap_or_else(spektro_core::catalog::default_path)
}

fn open_catalog(cli: &Cli) -> anyhow::Result<Catalog> {
    let path = catalog_path(cli);
    Catalog::open(&path).with_context(|| format!("opening catalog {}", path.display()))
}

fn index_printer() -> impl FnMut(IndexProgress) {
    let mut last = std::time::Instant::now();
    move |p| {
        let now = std::time::Instant::now();
        let final_step = match &p {
            IndexProgress::Metadata { done, total } | IndexProgress::Writing { done, total } => done == total,
            IndexProgress::Manifest { done, total, .. } => done == total,
            IndexProgress::Walking { .. } => false,
        };
        if !final_step && now.duration_since(last).as_millis() < 250 {
            return;
        }
        last = now;
        match p {
            IndexProgress::Walking { files } => eprint!("\rwalking: {files} files      "),
            IndexProgress::Metadata { done, total } => eprint!("\rmetadata: {done}/{total}      "),
            IndexProgress::Writing { done, total } => eprint!("\rwriting: {done}/{total} folders      "),
            IndexProgress::Manifest { done, total, name } => eprint!("\rmanifest {done}/{total} {name}      "),
        }
    }
}

fn catalog_cmd(cli: &Cli, cmd: &CatalogCmd) -> anyhow::Result<()> {
    use spektro_core::catalog::{index, keywords, query, thumbs};
    let mut cat = open_catalog(cli)?;
    match cmd {
        CatalogCmd::Add { folder } => {
            let r = index::add_folder(&mut cat, folder, &mut index_printer(), &Cancel::new())?;
            eprintln!();
            println!(
                "root #{}: {} files ({} unchanged, {} new, {} changed, {} moved, {} missing); {} assets created, {} merged",
                r.root, r.files, r.unchanged, r.added, r.changed, r.moved, r.missing, r.assets_created, r.assets_merged
            );
            for (p, e) in &r.errors {
                eprintln!("  skipped {p}: {e}");
            }
        }
        CatalogCmd::Adopt { archive } => {
            let archive = match archive {
                Some(a) => a.clone(),
                None => load_config(cli)?.archive_root,
            };
            let r = index::adopt_archive(&mut cat, &archive, &mut index_printer())?;
            eprintln!();
            println!(
                "root #{}: {} manifests, {} files ({} missing), {} assets ({} new), {} renders",
                r.root, r.manifests, r.files, r.missing_files, r.assets, r.assets_created, r.renders
            );
            for (p, e) in &r.errors {
                eprintln!("  {p}: {e}");
            }
        }
        CatalogCmd::Rescan { root } => {
            let r = index::index_root(&mut cat, *root, &mut index_printer(), &Cancel::new())?;
            eprintln!();
            if r.offline {
                println!("root #{root} is offline; nothing changed");
            } else {
                println!("{} files: {} unchanged, {} new, {} changed, {} moved, {} missing", r.files, r.unchanged, r.added, r.changed, r.moved, r.missing);
            }
        }
        CatalogCmd::Roots => {
            cat.refresh_online()?;
            for r in cat.roots()? {
                let counts = if r.kind == spektro_core::catalog::RootKind::Render {
                    format!("{:>6} renders", r.renders)
                } else {
                    format!("{:>6} assets {:>6} files {:>4} missing", r.assets, r.files, r.missing)
                };
                println!("#{:<3} {:<8} {:<7} {counts}  {}", r.id, r.kind.as_str(), if r.online { "online" } else { "OFFLINE" }, r.path.display());
            }
        }
        CatalogCmd::Relocate { root, path, force } => {
            let r = cat.relocate_root(*root, path, *force)?;
            println!("root #{}: {} -> {} ({}/{} sampled files found)", r.root, r.from.display(), r.to.display(), r.found, r.checked);
        }
        CatalogCmd::RemoveRoot { root } => {
            cat.remove_root(*root)?;
            println!("root #{root} forgotten");
        }
        CatalogCmd::Ls { keyword, camera, root, from, to, text, rendered, limit, offset, json } => {
            let f = query::Filter {
                keywords: keyword.clone(),
                cameras: camera.clone(),
                roots: root.clone(),
                date_from: from.clone(),
                date_to: to.clone(),
                text: text.clone(),
                has_render: rendered.then_some(true),
                ..Default::default()
            };
            let page = cat.list(&f, query::Sort::CapturedDesc, *offset, *limit)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&page.items)?);
            } else {
                for a in &page.items {
                    let mut flags = Vec::new();
                    if a.has_jpeg {
                        flags.push("+jpg".to_string());
                    }
                    if a.renders > 0 {
                        flags.push(format!("{} render{}", a.renders, if a.renders == 1 { "" } else { "s" }));
                    }
                    if a.ai_labelled {
                        flags.push("ai".into());
                    }
                    if a.missing {
                        flags.push("MISSING".into());
                    }
                    println!(
                        "{:>6}  {:<5}  {:<19}  {:<10}  {:<22} {}",
                        a.id,
                        a.kind,
                        a.captured_at.as_deref().unwrap_or("undated").replace('T', " "),
                        a.camera.as_deref().unwrap_or("-"),
                        a.name,
                        flags.join(" ")
                    );
                }
                println!("{} of {} assets", page.items.len(), page.total);
            }
        }
        CatalogCmd::Show { id, json } => {
            let d = cat.detail(*id)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&d)?);
            } else {
                println!("asset #{} ({})", d.id, d.kind);
                println!("  captured  {}", d.captured_at.as_deref().unwrap_or("undated"));
                println!("  camera    {} {}", d.make.as_deref().unwrap_or(""), d.model.as_deref().unwrap_or("-"));
                if let Some(l) = &d.lens {
                    println!("  lens      {l}");
                }
                if let Some(i) = d.iso {
                    println!("  iso       {i}");
                }
                if let (Some(w), Some(h)) = (d.width, d.height) {
                    println!("  size      {w}x{h}");
                }
                if let Some(src) = &d.import_source {
                    println!("  imported  from {src} at {}", d.imported_at.as_deref().unwrap_or("?"));
                }
                for f in &d.files {
                    println!("  file      {:<7} {}{}", f.role, f.path.display(), if f.missing { "  [missing]" } else { "" });
                }
                if !d.keywords.is_empty() {
                    let k: Vec<String> =
                        d.keywords.iter().map(|k| if k.source == keywords::USER { k.name.clone() } else { format!("{} ({})", k.name, k.source) }).collect();
                    println!("  keywords  {}", k.join(", "));
                }
                for r in &d.renders {
                    println!(
                        "  render    {:<4} {} [{}]{}",
                        r.kind,
                        r.path.display(),
                        r.preset_name.as_deref().unwrap_or(&r.preset_hash),
                        if r.exists { "" } else { "  [missing]" }
                    );
                }
            }
        }
        CatalogCmd::Tag { ids, add, remove } => {
            if add.is_empty() && remove.is_empty() {
                anyhow::bail!("give --add and/or --remove");
            }
            let added = keywords::add(cat.conn(), ids, add, keywords::USER)?;
            let removed = keywords::remove(cat.conn(), ids, remove)?;
            println!("{added} added, {removed} removed");
        }
        CatalogCmd::Print { ids, preset, jpeg, exr, camera_jpeg } => {
            let cfg = load_config(cli)?;
            let mut outputs = cfg.outputs.clone();
            if *jpeg || *exr {
                outputs.jpeg = *jpeg;
                outputs.exr = *exr;
            }
            let opts = spektro_core::catalog::print::ExportOptions {
                look: Some(preset.clone()),
                jpeg: outputs.jpeg,
                exr: outputs.exr,
                camera_jpeg: *camera_jpeg,
                destination: None,
            };
            let r = spektro_core::catalog::print::export_assets(&cat, &cfg, ids, &opts, event_printer(), &Cancel::new())?;
            println!(
                "{} photo(s): {} rendered, {} camera JPEG(s) copied, {} failed, {} skipped -> {}",
                r.requested,
                r.rendered,
                r.copied,
                r.failed,
                r.skipped.len(),
                cfg.render_root.display()
            );
            for (id, why) in &r.skipped {
                eprintln!("  #{id}: {why}");
            }
        }
        CatalogCmd::Thumbs { size, force } => {
            let dir = cli.thumbs_dir.clone().unwrap_or_else(spektro_core::catalog::default_thumbs_dir);
            let jobs = thumbs::jobs(cat.conn(), &dir, None, *size, *force)?;
            let (made, failed) = thumbs::generate_all(&cat, &jobs, |d, t| eprint!("\rthumbnails: {d}/{t}   "))?;
            eprintln!();
            println!("{made} made, {failed} without a preview, in {}", dir.display());
        }
    }
    Ok(())
}

fn chrono_now() -> chrono::NaiveDateTime {
    chrono::Local::now().naive_local()
}
