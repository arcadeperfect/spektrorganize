<script lang="ts">
  // Byte-identical files in the catalog, and what to do about them. Only exact
  // copies are listed, so removing one loses nothing — a RAW and its camera
  // JPEG are never duplicates. Whatever the catalog knew about a removed copy
  // (keywords, rating, prints) moves onto the copy you keep.
  import { api, catalog, human, type DupGroup, type DupScan } from "../../api";
  import { library as lib } from "../../library.svelte";
  import ConfirmPurge from "./ConfirmPurge.svelte";

  let scan = $state<DupScan | null>(null);
  let scanning = $state(false);
  let progress = $state<[number, number] | null>(null);
  let busy = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);
  /** file_id to keep, per group (keyed by hash). */
  let keep = $state<Record<string, number>>({});
  /** Groups the user has dealt with or dismissed. */
  let done = $state<Record<string, true>>({});
  let hardDelete = $state(false);
  /** Folders to search; empty is the whole catalog. */
  let roots = $state<number[]>([]);
  let wholeLibrary = $state(true);
  const allRoots = $derived(lib.facets?.roots ?? []);

  const groups = $derived((scan?.groups ?? []).filter((g) => !done[g.blake3]));

  /** Thumbnails by asset, so a copy can be recognised rather than read as a path. */
  let thumbs = $state<Record<number, string | null>>({});
  async function loadThumbs(s: DupScan) {
    const ids = [...new Set(s.groups.flatMap((g) => g.files.map((f) => f.asset_id).filter((i): i is number => i !== null)))];
    for (let i = 0; i < ids.length; i += 200) {
      const batch = await catalog.requestThumbs(ids.slice(i, i + 200)).catch(() => []);
      for (const t of batch) thumbs[t.id] = t.path;
    }
  }
  const freeable = $derived(groups.reduce((n, g) => n + g.wasted, 0));

  /**
   * The folders the copies actually live in, with how many of the groups on
   * screen have a copy there. Duplicates usually split across two consistent
   * places, so choosing a side once beats choosing per group.
   */
  const sides = $derived.by(() => {
    const by = new Map<number, { root: string; groups: number }>();
    for (const g of groups) {
      const seen = new Set<number>();
      for (const f of g.files) {
        if (!f.present || seen.has(f.root_id)) continue;
        seen.add(f.root_id);
        const e = by.get(f.root_id) ?? { root: f.root, groups: 0 };
        e.groups++;
        by.set(f.root_id, e);
      }
    }
    return [...by.entries()]
      .map(([id, e]) => ({ id, ...e, label: e.root.split("/").pop() || e.root }))
      .sort((a, b) => b.groups - a.groups);
  });

  /** Keep the copy that lives in `rootId`, in every group that has one. */
  function preferRoot(rootId: number) {
    let set = 0;
    for (const g of groups) {
      const f = g.files.find((x) => x.root_id === rootId && x.present);
      if (!f) continue;
      keep[g.blake3] = f.file_id;
      set++;
    }
    const side = sides.find((s) => s.id === rootId);
    note = `Keeping the copy in ${side?.label ?? "that folder"} for ${plural(set, "group")}${
      set < groups.length ? `; ${groups.length - set} have no copy there and are unchanged` : ""
    }.`;
  }

  $effect(() => {
    let off: (() => void) | undefined;
    api.on<[number, number]>("duplicates-progress", (p) => (progress = p)).then((f) => (off = f));
    return () => off?.();
  });

  async function run() {
    scanning = true;
    error = note = null;
    progress = null;
    try {
      const s = await catalog.findDuplicates(roots.length ? { roots: [...roots], whole_library: wholeLibrary } : undefined);
      scan = s;
      keep = Object.fromEntries(s.groups.map((g) => [g.blake3, (g.files.find((f) => f.present) ?? g.files[0]).file_id]));
      done = {};
      loadThumbs(s);
      note = s.groups.length
        ? `${plural(s.groups.length, "group")} of copies · ${human(s.wasted)} to reclaim${s.hashed ? ` · read ${plural(s.hashed, "file")}` : ""}.`
        : `No duplicates${s.hashed ? ` (read ${plural(s.hashed, "file")})` : ""}.`;
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
      progress = null;
    }
  }

  function plural(n: number, word: string) {
    return `${n} ${word}${n === 1 ? "" : "s"}`;
  }

  /** Which copies would go for a group, given the chosen keeper. */
  function victims(g: DupGroup): number[] {
    return g.files.filter((f) => f.file_id !== keep[g.blake3]).map((f) => f.file_id);
  }

  /** What the confirm screen is about to do. */
  let pending = $state<{ jobs: { keeper: number; drop: number[]; hash: string }[]; files: { path: string; size: number }[]; keeping: string[] } | null>(
    null,
  );

  function stage(list: DupGroup[]) {
    const jobs: { keeper: number; drop: number[]; hash: string }[] = [];
    const files: { path: string; size: number }[] = [];
    const keeping: string[] = [];
    for (const g of list) {
      const keeper = g.files.find((f) => f.file_id === keep[g.blake3]);
      const drop = g.files.filter((f) => f.file_id !== keep[g.blake3]);
      if (!keeper || !drop.length) continue;
      jobs.push({ keeper: keeper.file_id, drop: drop.map((f) => f.file_id), hash: g.blake3 });
      for (const f of drop) files.push({ path: f.path, size: g.size });
      keeping.push(keeper.path);
    }
    if (!jobs.length) return;
    pending = { jobs, files, keeping };
  }

  /** Run the staged jobs, one group at a time. */
  async function execute() {
    if (!pending) return;
    busy = true;
    let removed = 0;
    let bytes = 0;
    const failures: string[] = [];
    try {
      for (const j of pending.jobs) {
        const r = await catalog.purgeDuplicates(j.keeper, j.drop, hardDelete);
        done[j.hash] = true;
        removed += r.removed;
        bytes += r.bytes;
        failures.push(...r.failed.map(([p, why]) => `${p}: ${why}`));
      }
      note = `${hardDelete ? "Deleted" : "Trashed"} ${plural(removed, "copy")}, ${human(bytes)} freed.`;
      if (failures.length) error = failures.join("\n");
      lib.scheduleRefresh();
      pending = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function shortPath(p: string) {
    return p.replace(/^\/Users\/[^/]+/, "~");
  }
</script>

{#if pending}
  <ConfirmPurge
    title="{hardDelete ? 'Delete' : 'Trash'} {pending.files.length} duplicate {pending.files.length === 1 ? 'copy' : 'copies'}"
    files={pending.files}
    keeping={pending.keeping}
    {hardDelete}
    {busy}
    note="Ratings, keywords and prints of the removed copies move to the copy that stays."
    onexecute={execute}
    oncancel={() => (pending = null)}
  />
{/if}

<div class="dupes">
  <div class="bar row spread">
    <div class="row">
      <h2>Duplicates</h2>
      <span class="muted small">byte-for-byte copies only</span>
    </div>
    <div class="row">
      {#if scan}
        <label class="row small danger" title="Skip the Trash. There is no getting these back.">
          <input type="checkbox" bind:checked={hardDelete} /> delete for good
        </label>
        <button disabled={busy || !groups.length} onclick={() => stage(groups)}>
          Review {hardDelete ? "deletion" : "trash"} ({human(freeable)})
        </button>
      {/if}
      <button disabled={scanning} onclick={run}>{scanning ? "Scanning…" : scan ? "Scan again" : "Scan"}</button>
      <button onclick={() => (lib.duplicates = false)}>Close</button>
    </div>
  </div>

  <div class="scope row">
    <span class="muted small">Look in</span>
    <button class="mini" class:on={!roots.length} onclick={() => (roots = [])}>everything</button>
    {#each allRoots as r (r.id)}
      <button
        class="mini"
        class:on={roots.includes(r.id)}
        disabled={!r.online}
        title={r.online ? r.path : `${r.path} (offline)`}
        onclick={() => (roots = roots.includes(r.id) ? roots.filter((x) => x !== r.id) : [...roots, r.id])}
      >
        {r.label ?? r.path.split("/").pop()}
      </button>
    {/each}
    {#if roots.length}
      <label class="row small" title="Read the rest of the library too, and report a group when any copy is in the chosen folders">
        <input type="checkbox" bind:checked={wholeLibrary} /> compare with the rest of the library
      </label>
    {/if}
  </div>

  {#if sides.length > 1}
    <div class="prefer row">
      <span class="muted small">Keep the copy in</span>
      {#each sides as s (s.id)}
        <button class="mini" onclick={() => preferRoot(s.id)} title={s.root}>
          {s.label} <span class="muted">({s.groups})</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if scanning}
    <p class="muted small">
      Reading files whose size matches another's{progress ? ` — ${progress[0]} / ${progress[1]}` : ""}. Only those are hashed, so a library of unique
      photos finishes at once.
    </p>
  {/if}
  {#if note}<p class="small">{note}</p>{/if}
  {#if error}<p class="bad small pre">{error}</p>{/if}

  {#if !scan && !scanning}
    <p class="muted small intro">
      Finds files that are identical byte for byte — the same photo copied twice, or imported from the same card twice. Pick which copy to keep; the
      rest go to the Trash. A RAW and its camera JPEG are different files and are never listed here.
    </p>
  {/if}

  <div class="list">
    {#each groups as g (g.blake3)}
      <div class="group card">
        <div class="row spread head">
          <span class="small">{g.files.length} copies · {human(g.size)} each · <b>{human(g.wasted)}</b> to reclaim</span>
          <div class="row">
            <button class="mini" disabled={busy} onclick={() => stage([g])}>{hardDelete ? "Delete" : "Trash"} the rest</button>
            <button class="mini" onclick={() => (done[g.blake3] = true)} title="Leave these alone">skip</button>
          </div>
        </div>
        {#each g.files as f (f.file_id)}
          <label class="copy row" class:keeping={keep[g.blake3] === f.file_id}>
            <input type="radio" name={g.blake3} checked={keep[g.blake3] === f.file_id} onchange={() => (keep[g.blake3] = f.file_id)} />
            <span class="shot">
              {#if f.asset_id !== null && thumbs[f.asset_id]}
                <img src={api.fileUrl(thumbs[f.asset_id]!)} alt="" />
              {:else}
                <span class="ph"></span>
              {/if}
            </span>
            <span class="path mono" title={f.path}>{shortPath(f.path)}</span>
            <span class="tags">
              {#if !f.present}<span class="bad">missing</span>{/if}
              {#if f.rating}<span class="accent">{"♥".repeat(f.rating)}</span>{/if}
              {#if f.flag === "select"}<span class="ok">✓</span>{/if}
              {#if f.flag === "reject"}<span class="bad">✕</span>{/if}
              {#if f.keywords}<span class="muted">{f.keywords} kw</span>{/if}
              {#if f.renders}<span class="muted">▣ {f.renders}</span>{/if}
              <span class="muted">{keep[g.blake3] === f.file_id ? "keep" : hardDelete ? "delete" : "trash"}</span>
            </span>
          </label>
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .dupes {
    position: absolute;
    inset: 0;
    z-index: 14;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    padding: 0 14px 12px;
    overflow: auto;
  }
  .bar {
    position: sticky;
    top: 0;
    background: var(--bg);
    padding: 10px 0 8px;
    border-bottom: 1px solid var(--line);
    z-index: 1;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .prefer {
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px 0 0;
  }
  .scope {
    flex-wrap: wrap;
    gap: 4px;
    padding: 8px 0 2px;
  }
  .scope .on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
  .intro,
  .pre {
    max-width: 62ch;
    line-height: 1.45;
  }
  .pre {
    white-space: pre-wrap;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }
  .group {
    padding: 8px 10px;
  }
  .head {
    margin-bottom: 4px;
  }
  .copy {
    gap: 8px;
    padding: 2px 0;
    min-width: 0;
    cursor: pointer;
  }
  .copy.keeping .path {
    color: var(--text);
  }
  .shot {
    flex: none;
    width: 48px;
    height: 36px;
    display: grid;
    place-items: center;
    background: var(--bg-2);
    border-radius: 3px;
    overflow: hidden;
  }
  .shot img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .ph {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 1px solid var(--line);
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
    font-size: 11.5px;
    color: var(--muted);
  }
  .tags {
    display: flex;
    gap: 6px;
    font-size: 10.5px;
    flex: none;
  }
  .accent {
    color: var(--accent-2);
  }
  .danger input {
    accent-color: var(--bad);
  }
  .small {
    font-size: 11.5px;
  }
</style>
