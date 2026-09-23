// Library state: the current filter, a page cache over the catalog listing (the grid is
// virtualized and asks for what it shows), thumbnails as they arrive, selection, the detail
// panel, indexing progress and AI labelling runs.

import { SvelteMap, SvelteSet } from "svelte/reactivity";
import {
  api,
  catalog,
  type AssetDetail,
  type AssetSummary,
  type Collection,
  type Facets,
  type Filter,
  type IndexProgress,
  type SortKey,
  type ThumbReady,
} from "./api";
import { LABEL_BATCH, LABEL_SOURCE, labelItems } from "./label";

export const PAGE = 200;

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

function progressText(p: IndexProgress): string {
  switch (p.phase) {
    case "walking":
      return `${p.files} files found`;
    case "metadata":
      return `reading metadata ${p.done}/${p.total}`;
    case "writing":
      return `writing ${p.done}/${p.total} folders`;
    case "manifest":
      return `manifest ${p.done}/${p.total}`;
  }
}

class Library {
  filter = $state<Filter>({});
  sort = $state<SortKey>("captured_desc");
  tile = $state(168);
  /** Tiles past this ask for the 1024 px thumbnails instead of the 256 px ones. */
  static readonly BIG_TILE = 220;
  facets = $state<Facets | null>(null);
  total = $state(0);
  loaded = $state(false);
  /** Bumped when the filter changes; the grid scrolls back to the top. */
  resets = $state(0);

  pages = new SvelteMap<number, AssetSummary[]>();
  /** Asset id -> 256 px thumbnail path; null = no preview could be made. */
  thumbs = new SvelteMap<number, string | null>();
  selected = new SvelteSet<number>();
  focus = $state<number | null>(null);
  anchor = $state<number | null>(null);
  detail = $state<AssetDetail | null>(null);

  indexing = $state<{ label: string; text: string } | null>(null);
  note = $state<string | null>(null);
  error = $state<string | null>(null);
  labelAsk = $state<{ ids: number[]; what: string; replace: boolean } | null>(null);
  labelRun = $state<{ done: number; total: number; tagged: number } | null>(null);
  printIds = $state<number[] | null>(null);
  /** Photos lined up for one export run (kept in the catalog). */
  queue = new SvelteSet<number>();
  /** Open export dialog: the photos to export, and whether they came from the queue. */
  exportAsk = $state<{ ids: number[]; fromQueue: boolean } | null>(null);
  /** Right-click menu position and the photos it acts on. */
  menu = $state<{ x: number; y: number; ids: number[] } | null>(null);
  /** Bumped whenever keywords change, so keyword views re-read. */
  kwVersion = $state(0);

  /** Grid scroll position and keyboard cursor, kept across view switches. */
  scroll = $state(0);
  cursor = $state<number | null>(null);
  /** Index the grid should scroll into view once (set when the viewer closes). */
  scrollTo = $state<number | null>(null);
  /** Full-screen viewer: the index being shown, or null. */
  loupe = $state<number | null>(null);

  /** Visible index range, reported by the grid. */
  visible = { start: 0, end: 0 };
  private generation = 0;
  private inflight = new Set<number>();
  private wanted = new Set<number>();
  private requested = new Set<number>();
  private retried = new Set<number>();
  private wantTimer: ReturnType<typeof setTimeout> | null = null;
  private refreshTimer: ReturnType<typeof setTimeout> | null = null;
  private labelCancel = false;
  private listening = false;

  async init() {
    if (!this.listening) {
      this.listening = true;
      await api.on<null>("catalog-changed", () => this.scheduleRefresh());
      await api.on<number[]>("catalog-keywords", (ids) => {
        this.kwVersion++;
        if (this.focus !== null && ids.includes(this.focus)) this.loadDetail(this.focus);
        this.scheduleRefresh();
      });
      await api.on<ThumbReady[]>("thumbs-ready", (batch) => this.onThumbs(batch));
      await api.on<null>("queue-changed", () => this.refreshQueue());
      await api.on<{ label: string; progress: IndexProgress }>("catalog-index", (e) => {
        this.indexing = { label: e.label, text: progressText(e.progress) };
      });
      await api.on<{ label: string; report: Record<string, number> }>("catalog-index-done", (e) => {
        this.indexing = null;
        const r = e.report;
        if ("manifests" in r) {
          this.note = `Adopted ${e.label}: ${plural(r.manifests, "import")}, ${plural(r.assets, "photo")} (${r.assets_created} new), ${plural(r.renders, "render")}.`;
        } else if (r.offline) {
          this.error = `${e.label} is offline — connect the drive, or relocate the folder.`;
        } else {
          const bits = [`${r.added} new`, `${r.changed} changed`, `${r.moved} moved`, `${r.missing} missing`].filter((b) => !b.startsWith("0 "));
          if (!r.assets) {
            // Plenty of files, no photos: someone else's library bundle, or a folder of documents.
            this.error = `${e.label}: ${plural(r.files, "file")}, but no photos or video the library can show. Folders of previews or another app's catalog (Lightroom, Capture One, Photos) hold no originals to index.`;
          } else {
            this.note = `Indexed ${e.label}: ${plural(r.assets, "photo")} from ${plural(r.files, "file")}${bits.length ? ` (${bits.join(", ")})` : ", nothing changed"}.`;
          }
        }
      });
      await api.on<{ label: string; error: string }>("catalog-index-error", (e) => {
        this.indexing = null;
        this.error = `${e.label}: ${e.error}`;
      });
    }
    if (this.loaded) {
      // Coming back to the Library from another view: keep the place, and just
      // catch up on anything that changed while we were away.
      this.scheduleRefresh();
      this.refreshQueue();
      return;
    }
    await this.reload();
    await this.refreshQueue();
    await this.loadCollections();
    this.loaded = true;
  }

  // ---------- listing ----------

  /** A loaded summary by id, when the page holding it is in the cache. */
  itemById(id: number): AssetSummary | undefined {
    for (const [, items] of this.pages) {
      const hit = items.find((a) => a.id === id);
      if (hit) return hit;
    }
    return undefined;
  }

  /**
   * The photo `delta` places along from `id` in the current listing — what
   * ← and → mean in Develop and Print. Loads the page it lands on if needed,
   * and moves the cursor and selection there so the rest of the app follows.
   */
  async step(id: number, delta: number): Promise<AssetSummary | null> {
    let i = this.cursor !== null && this.item(this.cursor)?.id === id ? this.cursor : -1;
    if (i < 0) {
      for (const [n, items] of this.pages) {
        const k = items.findIndex((a) => a.id === id);
        if (k >= 0) {
          i = n * PAGE + k;
          break;
        }
      }
    }
    if (i < 0) return null;
    const to = i + delta;
    if (to < 0 || to >= this.total) return null;
    // Fetch the page itself: `ensure` does not wait, and it would move the grid's own window.
    await this.fetchPage(Math.floor(to / PAGE));
    for (let waited = 0; !this.item(to) && waited < 40; waited++) {
      // Another request already has the page in flight; give it a moment.
      await new Promise((r) => setTimeout(r, 25));
    }
    const a = this.item(to);
    if (!a) return null;
    this.cursor = to;
    this.click(to, a.id, { shiftKey: false, metaKey: false, ctrlKey: false });
    return a;
  }

  /** The clip the render dialog is open for. */
  renderClip = $state<number | null>(null);

  item(index: number): AssetSummary | undefined {
    return this.pages.get(Math.floor(index / PAGE))?.[index % PAGE];
  }

  thumbOf(a: AssetSummary): string | null | undefined {
    return this.thumbs.has(a.id) ? this.thumbs.get(a.id) : (a.thumb ?? undefined);
  }

  async loadFacets() {
    try {
      this.facets = await catalog.facets();
    } catch (e) {
      this.error = String(e);
    }
  }

  private async fetchPage(n: number, force = false) {
    if (!force && (this.pages.has(n) || this.inflight.has(n))) return;
    const gen = this.generation;
    this.inflight.add(n);
    try {
      const page = await catalog.list($state.snapshot(this.filter), this.sort, n * PAGE, PAGE);
      if (gen !== this.generation) return;
      this.total = page.total;
      this.pages.set(n, page.items);
    } catch (e) {
      this.error = String(e);
    } finally {
      if (gen === this.generation) this.inflight.delete(n);
    }
  }

  /** Make sure the pages covering [start, end) are loaded. */
  ensure(start: number, end: number) {
    this.visible = { start, end };
    const last = Math.max(start, end - 1);
    for (let p = Math.floor(start / PAGE); p <= Math.floor(last / PAGE); p++) this.fetchPage(p);
  }

  /** The filter or sort changed: start over from the top. */
  async reload() {
    this.generation++;
    this.pages.clear();
    this.inflight.clear();
    this.anchor = null;
    this.scroll = 0;
    this.cursor = null;
    this.resets++;
    await Promise.all([this.loadFacets(), this.fetchPage(0)]);
  }

  /** The data changed under the same filter: refetch what is on screen without blanking it. */
  async refresh() {
    this.generation++;
    this.inflight.clear();
    const first = Math.floor(this.visible.start / PAGE);
    const last = Math.floor(Math.max(this.visible.start, this.visible.end - 1) / PAGE);
    for (const k of [...this.pages.keys()]) if (k < first || k > last) this.pages.delete(k);
    const want: number[] = [];
    for (let p = first; p <= last; p++) want.push(p);
    await Promise.all([this.loadFacets(), ...want.map((p) => this.fetchPage(p, true))]);
    if (this.focus !== null) this.loadDetail(this.focus);
  }

  scheduleRefresh() {
    if (this.refreshTimer) clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      this.refresh();
    }, 200);
  }

  // ---------- filter ----------

  /** Called when the tile size changes: past the threshold the grid wants other thumbnails. */
  retileThumbs(before: number) {
    const was = before > Library.BIG_TILE;
    const now = this.tile > Library.BIG_TILE;
    if (was === now) return;
    this.thumbs.clear();
    this.requested.clear();
  }

  setFilter(patch: Partial<Filter>) {
    this.filter = { ...this.filter, ...patch };
    this.openCollection = null;
    this.reload();
  }

  toggle(key: "cameras" | "keywords" | "roots" | "kinds", value: string | number) {
    const cur = (this.filter[key] ?? []) as (string | number)[];
    const next = cur.includes(value) ? cur.filter((v) => v !== value) : [...cur, value];
    this.setFilter({ [key]: next } as Partial<Filter>);
  }

  /** Only this one value (clicking a sidebar entry); clicking it again clears it. */
  only(key: "cameras" | "keywords" | "roots", value: string | number) {
    const cur = (this.filter[key] ?? []) as (string | number)[];
    this.setFilter({ [key]: cur.length === 1 && cur[0] === value ? [] : [value] } as Partial<Filter>);
  }

  setDates(from: string | null, to: string | null) {
    const same = this.filter.date_from === from && this.filter.date_to === to;
    this.setFilter(same ? { date_from: null, date_to: null, undated: false } : { date_from: from, date_to: to, undated: false });
  }

  clearFilter() {
    this.openCollection = null;
    const text = this.filter.text;
    this.filter = text ? { text } : {};
    this.reload();
  }

  get hasFilter(): boolean {
    const f = this.filter;
    return !!(
      f.roots?.length ||
      f.cameras?.length ||
      f.keywords?.length ||
      f.date_from ||
      f.date_to ||
      f.undated ||
      f.has_render != null ||
      f.ai_labelled != null ||
      f.missing != null ||
      f.kinds?.length ||
      f.flags?.length ||
      f.min_rating
    );
  }

  // ---------- thumbnails ----------

  /** A tile on screen needs its thumbnail; requests are coalesced into one call per tick. */
  want(id: number) {
    if (this.thumbs.has(id) || this.requested.has(id) || this.wanted.has(id)) return;
    this.wanted.add(id);
    if (!this.wantTimer) this.wantTimer = setTimeout(() => this.flushWants(), 40);
  }

  private async flushWants() {
    this.wantTimer = null;
    const ids = [...this.wanted];
    this.wanted.clear();
    if (!ids.length) return;
    ids.forEach((id) => this.requested.add(id));
    try {
      const size = this.tile > Library.BIG_TILE ? 1024 : 256;
      const known = await catalog.requestThumbs(ids, false, size);
      for (const t of known) {
        this.thumbs.set(t.id, t.path);
        this.requested.delete(t.id);
      }
    } catch (e) {
      ids.forEach((id) => this.requested.delete(id));
      console.warn("thumbnails", e);
    }
  }

  private onThumbs(batch: ThumbReady[]) {
    const want = this.tile > Library.BIG_TILE ? 1024 : 256;
    for (const t of batch) {
      if (t.size !== want) continue;
      this.thumbs.set(t.id, t.path);
      this.requested.delete(t.id);
    }
  }

  /** The cached file is gone (cache cleared): make it again, once per session. */
  thumbBroken(id: number) {
    if (this.retried.has(id)) {
      this.thumbs.set(id, null);
      return;
    }
    this.retried.add(id);
    this.thumbs.delete(id);
    this.requested.add(id);
    catalog.requestThumbs([id], true).catch(() => this.requested.delete(id));
  }

  // ---------- selection & detail ----------

  private idsInRange(lo: number, hi: number): number[] | null {
    const out: number[] = [];
    for (let i = lo; i <= hi; i++) {
      const a = this.item(i);
      if (!a) return null;
      out.push(a.id);
    }
    return out;
  }

  async click(index: number, id: number, ev: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) {
    if (ev.shiftKey && this.anchor !== null) {
      const lo = Math.min(this.anchor, index);
      const hi = Math.max(this.anchor, index);
      let ids = this.idsInRange(lo, hi);
      if (!ids) ids = (await catalog.ids($state.snapshot(this.filter), this.sort)).slice(lo, hi + 1);
      if (!(ev.metaKey || ev.ctrlKey)) this.selected.clear();
      ids.forEach((i) => this.selected.add(i));
    } else if (ev.metaKey || ev.ctrlKey) {
      if (this.selected.has(id)) this.selected.delete(id);
      else this.selected.add(id);
      this.anchor = index;
    } else {
      this.selected.clear();
      this.selected.add(id);
      this.anchor = index;
    }
    this.loadDetail(id);
  }

  async selectAll() {
    const ids = await catalog.ids($state.snapshot(this.filter), this.sort);
    this.selected.clear();
    ids.forEach((i) => this.selected.add(i));
  }

  clearSelection() {
    this.selected.clear();
    this.anchor = null;
  }

  async loadDetail(id: number) {
    this.focus = id;
    try {
      const d = await catalog.asset(id);
      if (this.focus === id) this.detail = d;
    } catch (e) {
      if (this.focus === id) this.detail = null;
      this.error = String(e);
    }
  }

  /** Ids an action applies to: the selection, else everything in the current view. */
  async targetIds(): Promise<number[]> {
    if (this.selected.size) return [...this.selected];
    return catalog.ids($state.snapshot(this.filter), this.sort);
  }

  /** The duplicates panel is open over the grid. */
  duplicates = $state(false);
  /** The rejected-photos panel is open over the grid. */
  rejects = $state(false);

  // ---------- dynamic catalogs ----------

  collections = $state<Collection[]>([]);
  /** The catalog now on screen, so the sidebar can mark it. */
  openCollection = $state<number | null>(null);

  async loadCollections() {
    this.collections = await catalog.collections().catch(() => []);
  }

  /** Save what is on screen — filter and sort — under a name. */
  async saveCollection(name: string) {
    try {
      const id = await catalog.saveCollection(name.trim(), $state.snapshot(this.filter), this.sort);
      await this.loadCollections();
      this.openCollection = id;
      this.note = `Saved "${name.trim()}".`;
    } catch (e) {
      this.error = String(e);
    }
  }

  async openCollectionById(id: number) {
    const c = this.collections.find((x) => x.id === id);
    if (!c) return;
    this.filter = { ...c.filter };
    if (c.sort) this.sort = c.sort;
    this.openCollection = id;
    await this.reload();
  }

  async deleteCollection(id: number) {
    try {
      await catalog.deleteCollection(id);
      if (this.openCollection === id) this.openCollection = null;
      await this.loadCollections();
    } catch (e) {
      this.error = String(e);
    }
  }

  // ---------- rating and flags ----------

  /** Rewrite cached summaries so the grid reacts before the write lands. */
  private patch(ids: number[], f: (a: AssetSummary) => AssetSummary) {
    const want = new Set(ids);
    for (const [n, items] of this.pages) {
      if (!items.some((a) => want.has(a.id))) continue;
      this.pages.set(
        n,
        items.map((a) => (want.has(a.id) ? f(a) : a)),
      );
    }
  }

  /** Photos a rating/flag key applies to: the selection, else the photo under the cursor. */
  keyTargets(): number[] {
    if (this.selected.size) return [...this.selected];
    const i = this.cursor ?? this.anchor;
    const a = i === null ? undefined : this.item(i);
    return a ? [a.id] : [];
  }

  async rate(ids: number[], rating: number) {
    if (!ids.length) return;
    const r = Math.max(0, Math.min(5, Math.round(rating)));
    this.patch(ids, (a) => ({ ...a, rating: r }));
    if (this.detail && ids.includes(this.detail.id)) this.detail = { ...this.detail, rating: r };
    try {
      await catalog.setRating(ids, r);
      // A rating filter can push the photo out of the view.
      if (this.filter.min_rating) this.scheduleRefresh();
      else await this.loadFacets();
    } catch (e) {
      this.error = String(e);
      await this.refresh();
    }
  }

  /** Set "select"/"reject", or clear it when every target already has it. */
  async flag(ids: number[], flag: "select" | "reject") {
    if (!ids.length) return;
    const has = new Set(ids);
    let all = true;
    for (const [, items] of this.pages) for (const a of items) if (has.has(a.id) && a.flag !== flag) all = false;
    const next = all ? "" : flag;
    this.patch(ids, (a) => ({ ...a, flag: next }));
    if (this.detail && ids.includes(this.detail.id)) this.detail = { ...this.detail, flag: next };
    try {
      await catalog.setFlag(ids, next || null);
      if (this.filter.flags?.length) this.scheduleRefresh();
      else await this.loadFacets();
    } catch (e) {
      this.error = String(e);
      await this.refresh();
    }
  }

  /**
   * Shared shortcut: 1–5 rate (0 clears), S selects, R rejects. Returns true
   * when the key was handled.
   */
  ratingKey(e: KeyboardEvent, ids: number[]): boolean {
    if (e.metaKey || e.ctrlKey || e.altKey || !ids.length) return false;
    if (e.key.length === 1 && e.key >= "0" && e.key <= "5") {
      void this.rate(ids, Number(e.key));
      return true;
    }
    const k = e.key.toLowerCase();
    if (k === "s" || k === "r") {
      void this.flag(ids, k === "s" ? "select" : "reject");
      return true;
    }
    return false;
  }

  // ---------- keywords ----------

  async tag(ids: number[], add: string[], remove: string[]) {
    try {
      await catalog.tag(ids, add, remove);
    } catch (e) {
      this.error = String(e);
    }
  }

  async clearAi(ids: number[]) {
    try {
      const n = await catalog.clearLabels(ids, "ai:");
      this.note = `Removed ${plural(n, "AI label")}; your own keywords are unchanged.`;
    } catch (e) {
      this.error = String(e);
    }
  }

  // ---------- indexing ----------

  async addFolder(path: string) {
    this.note = this.error = null;
    this.indexing = { label: path.split("/").pop() ?? path, text: "starting" };
    try {
      await catalog.addFolder(path);
    } catch (e) {
      this.indexing = null;
      this.error = String(e);
    }
  }

  async adopt(path: string | null) {
    this.note = this.error = null;
    this.indexing = { label: path?.split("/").pop() ?? "archive", text: "reading manifests" };
    try {
      await catalog.adopt(path);
    } catch (e) {
      this.indexing = null;
      this.error = String(e);
    }
  }

  async rescan(root: number, label: string) {
    this.note = this.error = null;
    this.indexing = { label, text: "starting" };
    try {
      await catalog.rescan(root);
    } catch (e) {
      this.indexing = null;
      this.error = String(e);
    }
  }

  // ---------- AI labels ----------

  /** ✦ Label: the selection (replacing its AI labels), else everything unlabelled in view. */
  async askLabel() {
    this.note = this.error = null;
    if (this.selected.size) {
      const n = this.selected.size;
      this.labelAsk = { ids: [...this.selected], what: `${plural(n, "selected photo")}`, replace: true };
      return;
    }
    try {
      const ids = await catalog.ids({ ...$state.snapshot(this.filter), ai_labelled: false, kinds: ["raw", "image"] }, this.sort);
      if (!ids.length) {
        this.note = "Everything in this view already has AI labels — select photos to redo them.";
        return;
      }
      this.labelAsk = { ids, what: `${plural(ids.length, "unlabelled photo")} in this view`, replace: false };
    } catch (e) {
      this.error = String(e);
    }
  }

  async runLabel() {
    const ask = this.labelAsk;
    this.labelAsk = null;
    if (!ask) return;
    const key = await catalog.aiKey().catch(() => null);
    if (!key) {
      this.error = "No Anthropic API key yet — add one in Settings (or set ANTHROPIC_API_KEY before launching).";
      return;
    }
    this.labelCancel = false;
    let tagged = 0;
    try {
      const infos = await catalog.labelInfo(ask.ids);
      this.labelRun = { done: 0, total: Math.ceil(infos.length / LABEL_BATCH), tagged: 0 };
      const { refused } = await labelItems(
        key,
        infos,
        (it) => catalog.thumbData(it.id),
        async (labels, done, total) => {
          if (labels.length) await catalog.applyLabels(labels, LABEL_SOURCE, ask.replace);
          tagged += labels.length;
          this.labelRun = { done, total, tagged };
        },
        () => this.labelCancel,
      );
      this.note =
        (this.labelCancel ? `Stopped — ${tagged} labelled.` : `Labelled ${plural(tagged, "photo")}.`) +
        (refused ? ` ${refused} were declined by the model and stay unlabelled.` : "");
    } catch (e) {
      this.error = `AI labelling: ${e}${tagged ? ` (${tagged} labelled before it stopped)` : ""}`;
    } finally {
      this.labelRun = null;
      this.refresh();
    }
  }

  stopLabel() {
    this.labelCancel = true;
  }

  // ---------- export queue ----------

  async refreshQueue() {
    try {
      const ids = await catalog.queue();
      this.queue.clear();
      ids.forEach((id) => this.queue.add(id));
    } catch (e) {
      console.warn("queue", e);
    }
  }

  /** Queue photos for the next export run. */
  async queueAdd(ids: number[]) {
    if (!ids.length) return;
    await catalog.queueAdd(ids);
    ids.forEach((id) => this.queue.add(id));
    this.note = `${plural(ids.length, "photo")} queued for export.`;
  }

  async queueRemove(ids: number[]) {
    await catalog.queueRemove(ids);
    ids.forEach((id) => this.queue.delete(id));
  }

  async queueClear() {
    await catalog.queueClear();
    this.queue.clear();
  }

  /** Open the export dialog for the queue, or for a specific set of photos. */
  askExport(ids?: number[]) {
    const list = ids ?? [...this.queue];
    if (!list.length) {
      this.note = ids ? "Nothing to export." : "The export queue is empty. Right-click photos to add them.";
      return;
    }
    this.exportAsk = { ids: list, fromQueue: !ids };
  }

  // ---------- print ----------

  async askPrint() {
    this.note = this.error = null;
    const ids = await this.targetIds();
    if (!ids.length) {
      this.note = "Nothing to print in this view.";
      return;
    }
    this.printIds = ids;
  }
}

export const library = new Library();
