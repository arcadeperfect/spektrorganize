<script lang="ts">
  // Everything flagged as a reject, as thumbnails, so you look at the photos
  // before deciding. Nothing is removed from this screen: choosing leads to the
  // list of paths, and only that screen deletes.
  import { api, catalog, human, type AssetSummary, type DoomedFile } from "../../api";
  import { library as lib } from "../../library.svelte";
  import ConfirmPurge from "./ConfirmPurge.svelte";

  let items = $state<AssetSummary[]>([]);
  let thumbs = $state<Record<number, string | null>>({});
  let chosen = $state<Set<number>>(new Set());
  let loading = $state(true);
  let busy = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);
  let hardDelete = $state(false);
  let size = $state(150);
  let pending = $state<DoomedFile[] | null>(null);

  $effect(() => {
    load();
  });

  async function load() {
    loading = true;
    try {
      const page = await catalog.list({ flags: ["reject"] }, lib.sort, 0, 2000);
      items = page.items;
      chosen = new Set(items.map((a) => a.id));
      const known = items.filter((a) => a.thumb).map((a) => [a.id, a.thumb] as const);
      thumbs = Object.fromEntries(known);
      const need = items.filter((a) => !a.thumb).map((a) => a.id);
      for (let i = 0; i < need.length; i += 200) {
        const batch = await catalog.requestThumbs(need.slice(i, i + 200)).catch(() => []);
        for (const t of batch) thumbs[t.id] = t.path;
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  function toggle(id: number) {
    const next = new Set(chosen);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    chosen = next;
  }

  /** Look up every file of the chosen photos, then show them for a last look. */
  async function review() {
    if (!chosen.size) return;
    busy = true;
    error = null;
    try {
      pending = await catalog.doomedFiles([...chosen]);
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function execute() {
    busy = true;
    try {
      const r = await catalog.purgeAssets([...chosen], hardDelete);
      note = `${hardDelete ? "Deleted" : "Trashed"} ${r.removed} ${r.removed === 1 ? "file" : "files"} · ${human(r.bytes)} · ${r.assets_removed} ${
        r.assets_removed === 1 ? "photo" : "photos"
      } left the catalog.`;
      if (r.failed.length) error = r.failed.map(([p, why]) => `${p}: ${why}`).join("\n");
      pending = null;
      lib.scheduleRefresh();
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function when(a: AssetSummary) {
    return a.captured_at ? a.captured_at.slice(0, 10) : "undated";
  }
</script>

{#if pending}
  <ConfirmPurge
    title="{hardDelete ? 'Delete' : 'Trash'} {chosen.size} rejected {chosen.size === 1 ? 'photo' : 'photos'}"
    files={pending}
    {hardDelete}
    {busy}
    note="Every file of each photo goes — the RAW, its camera JPEG and any sidecars. Prints already made from them stay on disk."
    onexecute={execute}
    oncancel={() => (pending = null)}
  />
{/if}

<div class="rejects">
  <div class="bar row spread">
    <div class="row">
      <h2>Rejected</h2>
      <span class="muted small">{items.length} marked ✕ · {chosen.size} chosen</span>
    </div>
    <div class="row">
      <input type="range" min="90" max="260" step="10" bind:value={size} title="Thumbnail size" />
      <button class="mini" onclick={() => (chosen = new Set(items.map((a) => a.id)))}>all</button>
      <button class="mini" onclick={() => (chosen = new Set())}>none</button>
      <label class="row small danger" title="Skip the Trash. There is no getting these back.">
        <input type="checkbox" bind:checked={hardDelete} /> delete for good
      </label>
      <button class="primary" disabled={busy || !chosen.size} onclick={review}>Review {chosen.size}…</button>
      <button onclick={() => (lib.rejects = false)}>Close</button>
    </div>
  </div>

  {#if note}<p class="small">{note}</p>{/if}
  {#if error}<p class="bad small pre">{error}</p>{/if}

  {#if loading}
    <p class="muted small">Loading…</p>
  {:else if !items.length}
    <p class="muted small intro">Nothing is rejected. Press <b>R</b> on a photo in the library to mark it, then come back here to clear them out.</p>
  {:else}
    <p class="muted small intro">
      Click a photo to spare it. What stays chosen goes to the {hardDelete ? "bin, permanently" : "Trash"} — and you see every path first.
    </p>
    <div class="grid" style="--w: {size}px">
      {#each items as a (a.id)}
        <button class="tile" class:spared={!chosen.has(a.id)} onclick={() => toggle(a.id)} title={a.name}>
          <span class="img">
            {#if thumbs[a.id]}
              <img src={api.fileUrl(thumbs[a.id]!)} alt="" />
            {:else}
              <span class="muted small">no preview</span>
            {/if}
            {#if !chosen.has(a.id)}<span class="keep">keep</span>{/if}
          </span>
          <span class="cap">
            <span class="name">{a.name}</span>
            <span class="muted">{when(a)}{a.rating ? ` · ${"♥".repeat(a.rating)}` : ""}</span>
          </span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .rejects {
    position: absolute;
    inset: 0;
    z-index: 14;
    background: var(--bg);
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
    flex-wrap: wrap;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .intro,
  .pre {
    max-width: 70ch;
    line-height: 1.45;
  }
  .pre {
    white-space: pre-wrap;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--w), 1fr));
    gap: 8px;
    margin-top: 10px;
  }
  .tile {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 3px;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .tile:hover {
    background: var(--bg-2);
  }
  .img {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 3 / 2;
    background: var(--bg-2);
    border-radius: 4px;
    overflow: hidden;
    outline: 2px solid var(--bad);
    outline-offset: -2px;
  }
  .tile.spared .img {
    outline-color: var(--ok);
  }
  .tile:not(.spared) img {
    opacity: 0.55;
  }
  img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .keep {
    position: absolute;
    top: 4px;
    left: 4px;
    font-size: 9.5px;
    padding: 0 5px;
    border-radius: 3px;
    background: rgba(20, 18, 17, 0.85);
    color: var(--ok);
  }
  .cap {
    display: flex;
    flex-direction: column;
    font-size: 11px;
    min-width: 0;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .danger input {
    accent-color: var(--bad);
  }
  .small {
    font-size: 11.5px;
  }
</style>
