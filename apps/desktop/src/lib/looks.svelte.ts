// Look editor state: the look being edited (only its overrides are stored),
// the parameters it resolves to, the preview photo with its RAW settings, and
// a coalescing preview loop (one render in flight; the latest edit wins).

import { api, looks as backend, type Look, type LookMeta, type PresetSummary } from "./api";
import { develop } from "./photo.svelte";

export const ROUTE_PRINT = "input > film > print > scan";
export const ROUTE_SCAN = "input > film > scan";

function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v));
}

export function getPath(obj: any, path: string): any {
  let cur = obj;
  for (const k of path.split(".")) {
    if (cur == null) return undefined;
    cur = cur[k];
  }
  return cur;
}

function setPath(obj: any, path: string, value: any) {
  const keys = path.split(".");
  let cur = obj;
  for (const k of keys.slice(0, -1)) {
    if (typeof cur[k] !== "object" || cur[k] === null || Array.isArray(cur[k])) cur[k] = {};
    cur = cur[k];
  }
  cur[keys[keys.length - 1]] = value;
}

function deletePath(obj: any, path: string) {
  const keys = path.split(".");
  const stack: any[] = [obj];
  let cur = obj;
  for (const k of keys.slice(0, -1)) {
    if (typeof cur[k] !== "object" || cur[k] === null) return;
    cur = cur[k];
    stack.push(cur);
  }
  delete cur[keys[keys.length - 1]];
  // Drop emptied parents.
  for (let i = keys.length - 2; i >= 0; i--) {
    const parent = stack[i];
    const child = parent[keys[i]];
    if (child && typeof child === "object" && !Array.isArray(child) && Object.keys(child).length === 0) delete parent[keys[i]];
    else break;
  }
}

class Looks {
  meta = $state<LookMeta | null>(null);
  list = $state<PresetSummary[]>([]);
  /** File the current look was loaded from / saved to (null = new, unsaved). */
  path = $state<string | null>(null);
  look = $state<Look>({ name: "New look", film: "kodak_portra_400", print: "kodak_portra_endura", params: {} });
  dirty = $state(false);
  warnings = $state<string[]>([]);
  /** Full parameters the look renders with, and the film's plain stock defaults. */
  effective = $state<Record<string, any> | null>(null);
  stock = $state<Record<string, any> | null>(null);

  preview = $state<string | null>(null);
  /** Before/after: the photo with only its RAW settings, no film stage. */
  showBefore = $state(false);
  before = $state<string | null>(null);
  rendering = $state(false);
  renderMs = $state(0);
  error = $state<string | null>(null);
  busy = $state<string | null>(null);

  private pending = false;
  private stockFor = "";
  private rawVersion = -1;

  /** The photo the print preview renders (the develop stage's photo). */
  get photo(): number | null {
    return develop.id;
  }

  async init() {
    if (!this.meta) this.meta = await backend.meta();
    await this.refreshList();
    if (!this.path && this.list.length && !this.dirty) await this.open(this.list[0].path);
    else this.refresh();
  }

  async refreshList() {
    this.list = await api.listPresets();
  }

  async open(path: string) {
    this.look = await backend.load(path);
    this.path = path;
    this.dirty = false;
    this.warnings = [];
    this.refresh();
  }

  newLook() {
    this.look = { name: "New look", film: this.look.film, print: this.look.print, params: {} };
    this.path = null;
    this.dirty = true;
    this.warnings = [];
    this.refresh();
  }

  adopt(look: Look, warnings: string[]) {
    this.look = look;
    this.path = null;
    this.dirty = true;
    this.warnings = warnings;
    this.refresh();
  }

  /** Value the look renders with at `path`. */
  value(path: string): any {
    return getPath(this.effective, path);
  }

  isOverridden(path: string): boolean {
    return getPath(this.look.params, path) !== undefined;
  }

  set(path: string, value: any) {
    if (path.startsWith("camera.")) this.before = null;
    const params = clone(this.look.params);
    setPath(params, path, value);
    this.look.params = params;
    if (this.effective) setPath(this.effective, path, value);
    this.touch();
  }

  reset(path: string) {
    const params = clone(this.look.params);
    deletePath(params, path);
    this.look.params = params;
    this.touch();
  }

  setFilm(name: string) {
    const film = this.meta?.films.find((f) => f.name === name);
    this.look.film = name;
    // Slides are scanned; negatives print onto their paired paper.
    const params = clone(this.look.params);
    if (film?.positive) setPath(params, "workflow.route", ROUTE_SCAN);
    else if (getPath(params, "workflow.route") === ROUTE_SCAN) deletePath(params, "workflow.route");
    this.look.params = params;
    if (film?.target_print && !film.positive) this.look.print = film.target_print;
    this.touch();
  }

  setPrint(name: string) {
    this.look.print = name;
    this.touch();
  }

  setName(name: string) {
    this.look.name = name;
    this.dirty = true;
  }

  setParamsJson(text: string): string | null {
    try {
      const v = JSON.parse(text);
      if (typeof v !== "object" || v === null || Array.isArray(v)) return "must be a JSON object";
      this.look.params = v;
      this.touch();
      return null;
    } catch (e) {
      return String(e);
    }
  }

  private touch() {
    this.dirty = true;
    if (this.showBefore) this.before = null;
    this.refresh();
  }

  async save(asNew = false): Promise<string | null> {
    try {
      const path = await backend.save(clone(this.look), asNew ? null : this.path);
      this.path = path;
      this.dirty = false;
      await this.refreshList();
      return path;
    } catch (e) {
      this.error = String(e);
      return null;
    }
  }

  async remove() {
    if (!this.path) return;
    await backend.remove(this.path);
    this.path = null;
    await this.refreshList();
    if (this.list.length) await this.open(this.list[0].path);
  }

  async neutralize() {
    this.busy = "Balancing print filters…";
    try {
      const [m, y] = await backend.neutralize(clone(this.look));
      const params = clone(this.look.params);
      setPath(params, "enlarger.m_filter_shift", Math.round(m * 100) / 100);
      setPath(params, "enlarger.y_filter_shift", Math.round(y * 100) / 100);
      this.look.params = params;
      this.touch();
    } catch (e) {
      this.error = String(e);
    } finally {
      this.busy = null;
    }
  }

  /** Show the print stage's input (the developed photo) instead of the print. */
  toggleBefore() {
    this.showBefore = !this.showBefore;
    if (this.showBefore && !this.before) this.refresh();
  }

  /** Follow the develop stage: a new photo or new develop settings. */
  syncPhoto() {
    if (this.rawVersion === develop.version) return;
    this.rawVersion = develop.version;
    this.preview = null;
    this.before = null;
    this.refresh();
  }

  /** Re-resolve the parameters and re-render the preview (coalesced). */
  refresh() {
    if (this.rendering) {
      this.pending = true;
      return;
    }
    this.run();
  }

  /** Render every pixel, for judging grain and sharpness. */
  setFull(on: boolean) {
    develop.setFull(on, () => {
      this.before = null;
      this.refresh();
    });
  }

  /** Follow the viewer's zoom: bigger renders when zoomed in. */
  setZoom(z: number) {
    develop.setZoom(z, () => {
      this.before = null;
      this.refresh();
    });
  }

  private async run() {
    this.rendering = true;
    this.error = null;
    try {
      const look = clone(this.look);
      const raw = { ...develop.raw };
      this.rawVersion = develop.version;
      if (this.stockFor !== look.film) {
        this.stock = await backend.resolve({ ...look, params: {} });
        this.stockFor = look.film;
      }
      this.effective = await backend.resolve(look);
      if (develop.id !== null) {
        const t = performance.now();
        const px = develop.detail;
        if (this.showBefore && !this.before) this.before = await backend.previewBefore(develop.id, look, raw, px);
        this.preview = await backend.preview(develop.id, look, raw, px);
        this.renderMs = Math.round(performance.now() - t);
      }
    } catch (e) {
      this.error = String(e);
    } finally {
      this.rendering = false;
      if (this.pending) {
        this.pending = false;
        this.run();
      }
    }
  }
}

export const lookEditor = new Looks();
