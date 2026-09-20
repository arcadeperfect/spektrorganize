import { api, catalog, type Config, type JobEvent, type PlanView, type ScanView } from "./api";

export type View = "library" | "develop" | "print" | "source" | "review" | "layout" | "commit" | "settings" | "rerender";

export interface JobState {
  /** What started it: a card import, a manifest re-render, or a print from the library. */
  kind: "import" | "rerender" | "print";
  phase: "idle" | "copy" | "render" | "done" | "error";
  copyFiles: number;
  copyBytes: number;
  doneFiles: number;
  doneBytes: number;
  currentFile: string;
  currentFileBytes: number;
  currentFileDone: number;
  copied: number;
  skipped: number;
  failed: number;
  manifest: string | null;
  renderTotal: number;
  renderDone: number;
  renderFailed: number;
  backend: string;
  outputs: string[];
  log: string[];
  error: string | null;
  renderSkipped: string | null;
}

function freshJob(kind: JobState["kind"] = "import"): JobState {
  return {
    kind,
    phase: "idle",
    copyFiles: 0,
    copyBytes: 0,
    doneFiles: 0,
    doneBytes: 0,
    currentFile: "",
    currentFileBytes: 0,
    currentFileDone: 0,
    copied: 0,
    skipped: 0,
    failed: 0,
    manifest: null,
    renderTotal: 0,
    renderDone: 0,
    renderFailed: 0,
    backend: "",
    outputs: [],
    log: [],
    error: null,
    renderSkipped: null,
  };
}

class Store {
  view = $state<View>("library");
  config = $state<Config | null>(null);
  scan = $state<ScanView | null>(null);
  scanning = $state(false);
  scanProgress = $state("");
  scanError = $state<string | null>(null);
  plan = $state<PlanView | null>(null);
  planError = $state<string | null>(null);
  job = $state<JobState>(freshJob());
  busy = $state(false);
  toast = $state<string | null>(null);

  included = $derived(this.scan ? this.scan.groups.filter((g) => !g.excluded) : []);
  includedRaws = $derived(this.included.filter((g) => g.kind === "raw").length);

  private listening = false;

  async init() {
    this.config = await api.getConfig();
    this.busy = await api.isBusy();
    const existing = await api.getScan();
    if (existing) this.scan = existing;
    if (this.listening) return;
    this.listening = true;
    await api.on<{ phase: string; files?: number; done?: number; total?: number }>("scan-progress", (p) => {
      this.scanProgress =
        p.phase === "walking"
          ? `${p.files} files found`
          : p.phase === "duplicates"
            ? p.total
              ? `checking for copies ${p.done}/${p.total}`
              : "checking for copies"
            : `reading metadata ${p.done}/${p.total}`;
    });
    await api.on<ScanView>("scan-done", (s) => {
      this.scan = s;
      this.scanning = false;
      this.busy = false;
      this.plan = null;
      this.view = "review";
    });
    await api.on<string>("scan-error", (e) => {
      this.scanning = false;
      this.busy = false;
      this.scanError = e;
    });
    await api.on<JobEvent>("job-event", (e) => this.onJobEvent(e));
    await api.on<{ manifest: string; copied?: number; skipped?: number; failed?: number; rendered?: number; render_failed?: number }>(
      "job-done",
      (r) => {
        this.job.phase = "done";
        this.job.manifest = r.manifest;
        this.busy = false;
      },
    );
    await api.on<string>("job-error", (e) => {
      this.job.phase = "error";
      this.job.error = e;
      this.busy = false;
    });
  }

  async startScan(path: string) {
    this.scanError = null;
    this.scanning = true;
    this.busy = true;
    this.scanProgress = "starting";
    try {
      await api.startScan(path);
    } catch (e) {
      this.scanning = false;
      this.busy = false;
      this.scanError = String(e);
    }
  }

  async setExcluded(ids: number[], excluded: boolean) {
    if (!this.scan) return;
    await api.setExcluded(ids, excluded);
    const set = new Set(ids);
    for (const g of this.scan.groups) if (set.has(g.id)) g.excluded = excluded;
    this.plan = null;
  }

  async saveConfig() {
    if (!this.config) return;
    await api.setConfig(this.config);
    this.plan = null;
  }

  async refreshPlan() {
    if (!this.scan) return;
    this.planError = null;
    try {
      this.plan = await api.makePlan();
    } catch (e) {
      this.planError = String(e);
      this.plan = null;
    }
  }

  async startImport(render: boolean) {
    this.job = freshJob("import");
    this.job.phase = "copy";
    this.busy = true;
    try {
      await api.startImport(render);
    } catch (e) {
      this.job.phase = "error";
      this.job.error = String(e);
      this.busy = false;
    }
  }

  async startRender(req: { manifest: string; exr: boolean; jpeg: boolean; render_root: string | null; only_missing: boolean }) {
    this.job = freshJob("rerender");
    this.job.phase = "render";
    this.busy = true;
    try {
      await api.startRenderManifest(req);
    } catch (e) {
      this.job.phase = "error";
      this.job.error = String(e);
      this.busy = false;
    }
  }

  /** Run one export (renders and/or camera JPEG copies). */
  async startExport(req: {
    ids: number[];
    look: string | null;
    jpeg: boolean;
    exr: boolean;
    camera_jpeg: boolean;
    destination: string | null;
    clear_queue: boolean;
  }) {
    this.job = freshJob("print");
    this.job.phase = "render";
    this.busy = true;
    try {
      await catalog.export(req);
    } catch (e) {
      this.job.phase = "error";
      this.job.error = String(e);
      this.busy = false;
    }
  }

  /** Print catalog assets through a preset; progress shows in the JobPanel. */
  async startPrint(req: { ids: number[]; preset: string; jpeg: boolean; exr: boolean; camera_jpeg?: boolean }) {
    this.job = freshJob("print");
    this.job.phase = "render";
    this.busy = true;
    try {
      await catalog.print(req);
    } catch (e) {
      this.job.phase = "error";
      this.job.error = String(e);
      this.busy = false;
    }
  }

  dismissJob() {
    if (this.job.phase === "done" || this.job.phase === "error") this.job = freshJob();
  }

  private onJobEvent(e: JobEvent) {
    const j = this.job;
    switch (e.type) {
      case "copy_started":
        j.phase = "copy";
        j.copyFiles = e.files;
        j.copyBytes = e.bytes;
        break;
      case "file_started":
        j.currentFile = e.dest;
        j.currentFileBytes = e.size;
        j.currentFileDone = 0;
        break;
      case "file_progress":
        j.currentFileDone = e.bytes_done;
        break;
      case "file_done":
        if (e.status.status === "copied") {
          j.copied++;
          j.doneFiles++;
          j.doneBytes += j.currentFileBytes;
        } else if (e.status.status === "skipped_existing") {
          j.skipped++;
        } else {
          j.failed++;
          j.doneFiles++;
          j.log.push(`copy failed: ${e.status.detail}`);
        }
        j.currentFileDone = 0;
        break;
      case "copy_finished":
        j.copied = e.copied;
        j.skipped = e.skipped;
        j.failed = e.failed;
        j.doneBytes = e.bytes;
        break;
      case "manifest_written":
        j.manifest = e.path;
        break;
      case "render_started":
        j.phase = "render";
        j.renderTotal = e.total;
        j.backend = e.backend;
        break;
      case "render_done":
        j.renderDone++;
        j.outputs.push(...e.outputs);
        break;
      case "render_failed":
        j.renderFailed++;
        j.log.push(`render #${e.entry} failed: ${e.error}`);
        break;
      case "render_finished":
        j.renderDone = e.done;
        j.renderFailed = e.failed;
        break;
      case "render_skipped":
        j.renderSkipped = e.reason;
        break;
      case "log":
        j.log.push(e[0]);
        break;
    }
  }

  say(msg: string) {
    this.toast = msg;
    setTimeout(() => {
      if (this.toast === msg) this.toast = null;
    }, 2500);
  }
}

export const store = new Store();
