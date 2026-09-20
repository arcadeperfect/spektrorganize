<script lang="ts">
  import { human, type GroupView } from "../api";
  import { store } from "../state.svelte";
  import Thumb from "./Thumb.svelte";

  let filter = $state<"photos" | "videos" | "other">("photos");
  let size = $state(180);

  const photos = $derived(store.scan?.groups.filter((g) => g.kind === "raw" || g.kind === "image") ?? []);
  const videos = $derived(store.scan?.groups.filter((g) => g.kind === "video") ?? []);
  const others = $derived(store.scan?.groups.filter((g) => g.kind === "sidecar" || g.kind === "other") ?? []);
  /** Declined copies stay on screen by default, so you can see what was skipped. */
  let showCopies = $state(true);
  /** Numbered runs shown as one row each, rather than a few hundred. */
  let collapse = $state(true);
  const sequences = $derived(store.scan?.sequences ?? []);
  /** Group ids that belong to a run, mapped to the run they are in. */
  const inSequence = $derived.by(() => {
    const m = new Map<number, number>();
    if (!collapse) return m;
    sequences.forEach((s, i) => s.members.forEach((id) => m.set(id, i)));
    return m;
  });
  const isCopy = (g: GroupView) => g.copy_of !== null || g.known_at !== null;
  const shown = $derived(
    (filter === "photos" ? photos : filter === "videos" ? videos : others)
      .filter((g) => showCopies || !isCopy(g))
      // Collapsed: only the first frame of a run stands for the whole run.
      .filter((g) => {
        const seq = inSequence.get(g.id);
        return seq === undefined || sequences[seq].members[0] === g.id;
      }),
  );
  const embedded = $derived(photos.filter((g) => g.preview === "embedded_preview").length);
  const excludedCount = $derived(store.scan?.groups.filter((g) => g.excluded).length ?? 0);
  const copies = $derived(store.scan?.copies ?? 0);
  const held = $derived(store.scan?.already_held ?? 0);

  /** Put the copies and the already-held photos back in, if you really want them. */
  function includeDupes() {
    const ids = (store.scan?.groups ?? []).filter((g) => g.copy_of !== null || g.known_at !== null).map((g) => g.id);
    store.setExcluded(ids, false);
  }

  /** The last photo clicked, and what that click did — shift-click repeats it over a range. */
  let anchor = $state<{ index: number; excluded: boolean } | null>(null);

  function click(g: GroupView, i: number, e: MouseEvent | KeyboardEvent) {
    if (e.shiftKey && anchor) {
      // Everything between the two clicks takes the state the first click set.
      const [lo, hi] = anchor.index < i ? [anchor.index, i] : [i, anchor.index];
      store.setExcluded(
        shown.slice(lo, hi + 1).map((x) => x.id),
        anchor.excluded,
      );
      anchor = { index: i, excluded: anchor.excluded };
      return;
    }
    const excluded = !g.excluded;
    const seq = inSequence.get(g.id);
    store.setExcluded(seq === undefined ? [g.id] : sequences[seq].members, excluded);
    anchor = { index: i, excluded };
  }
  function setAll(excluded: boolean) {
    store.setExcluded(
      shown.map((g) => g.id),
      excluded,
    );
  }
  function previewLabel(p: GroupView["preview"]) {
    switch (p) {
      case "sidecar_jpeg":
        return "camera JPEG";
      case "embedded_preview":
        return "camera preview";
      default:
        return "";
    }
  }
  function when(g: GroupView) {
    return g.captured_at ? g.captured_at.replace("T", " ") : "undated";
  }
</script>

{#if store.scan}
  <div class="row spread head">
    <div>
      <h1>{store.scan.source.label}</h1>
      <div class="muted">
        {photos.length} photos · {videos.length} videos · {others.length} other · {human(store.scan.bytes)}
        {#if excludedCount > 0}
          · <span class="warn">{excludedCount} excluded</span>
        {/if}
      </div>
    </div>
    <div class="row">
      <div class="seg">
        <button class:active={filter === "photos"} onclick={() => (filter = "photos")}>Photos</button>
        <button class:active={filter === "videos"} onclick={() => (filter = "videos")}>Videos</button>
        <button class:active={filter === "other"} onclick={() => (filter = "other")}>Other</button>
      </div>
      <input type="range" min="120" max="320" bind:value={size} title="Thumbnail size" />
      <button class="ghost" onclick={() => setAll(false)}>Include all</button>
      <button class="ghost" onclick={() => setAll(true)}>Exclude all</button>
      <button class="primary" onclick={() => (store.view = "layout")}>Layout →</button>
    </div>
  </div>

  {#if filter === "photos" && embedded > 0}
    <div class="note muted">
      Previews for {embedded} RAW{embedded === 1 ? "" : "s"} without a sidecar JPEG come from the camera's embedded preview, not from the decode this app will do.
    </div>
  {/if}

  {#if sequences.length}
    <div class="card seqs">
      <div class="row spread">
        <span class="small">
          <b>{sequences.length}</b>
          {sequences.length === 1 ? "image sequence" : "image sequences"} ·
          {sequences.reduce((n, s) => n + s.frames, 0)} frames
          {#if collapse}— shown as one row each; every frame is still imported{/if}
        </span>
        <button class="mini" onclick={() => (collapse = !collapse)}>{collapse ? "show every frame" : "collapse them"}</button>
      </div>
    </div>
  {/if}

  {#if copies || held}
    <div class="card dupes">
      <div class="row spread">
        <span class="small">
          {#if copies}<b>{copies}</b> {copies === 1 ? "photo is a copy" : "photos are copies"} of another on this card{/if}
          {#if copies && held}, and{/if}
          {#if held}<b>{held}</b> {held === 1 ? "is" : "are"} already in your library{/if}
          — excluded, so each photo is imported once.
        </span>
        <div class="row">
          <button class="mini" onclick={() => (showCopies = !showCopies)}>{showCopies ? "hide them" : "show them"}</button>
          <button class="mini" onclick={includeDupes}>include them anyway</button>
        </div>
      </div>
    </div>
  {/if}

  {#if filter === "photos"}
    <div class="grid" style="--w: {size}px">
      {#each shown as g, i (g.id)}
        <div
          class="tile"
          class:excluded={g.excluded}
          onclick={(e) => click(g, i, e)}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === " " && click(g, i, e)}
        >
          <Thumb group={g.id} />
          <div class="meta">
            <div class="row spread">
              <span class="name">{g.name}</span>
              <span class="badge">{g.kind === "raw" ? "RAW" : "JPG"}{g.attachments.length ? "+" + g.attachments.length : ""}</span>
            </div>
            <div class="muted small">{when(g)}{g.camera ? " · " + g.camera : ""}{g.iso ? " · ISO " + g.iso : ""}</div>
            {#if inSequence.get(g.id) !== undefined}
              {@const s = sequences[inSequence.get(g.id)!]}
              <div class="small seq">{s.pattern} · {s.frames} frames{s.missing.length ? ` · ${s.missing.length} missing` : ""}</div>
            {:else}
              <div class="muted small">{previewLabel(g.preview)}</div>
            {/if}
          </div>
          {#if g.copy_of !== null}
            <div class="x why">copy on this card</div>
          {:else if g.known_at !== null}
            <div class="x why" title={g.known_at}>already in the library</div>
          {:else if g.excluded}
            <div class="x">excluded</div>
          {/if}
        </div>
      {/each}
    </div>
  {:else}
    <div class="list">
      {#each shown as g, i (g.id)}
        <div
          class="line"
          class:excluded={g.excluded}
          onclick={(e) => click(g, i, e)}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === " " && click(g, i, e)}
        >
          <input type="checkbox" checked={!g.excluded} tabindex="-1" />
          <span class="mono">{g.rel}</span>
          {#if g.attachments.length}
            <span class="muted">+ {g.attachments.join(", ")}</span>
          {/if}
          <span class="muted right">{human(g.bytes)} · {when(g)}</span>
        </div>
      {/each}
      {#if shown.length === 0}
        <div class="muted">nothing here</div>
      {/if}
    </div>
  {/if}

  {#if store.scan.errors.length}
    <div class="card" style="margin-top: 14px">
      <h2>Skipped</h2>
      {#each store.scan.errors as [p, e] (p)}
        <div class="mono bad">{p}: {e}</div>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .head {
    /* Stays put while the photos scroll under it. The negative margins let its
       background cover the page gutter, so nothing shows through at the sides. */
    position: sticky;
    top: -18px;
    z-index: 5;
    align-items: flex-start;
    margin: -18px -22px 10px;
    padding: 14px 22px 8px;
    background: var(--bg);
    border-bottom: 1px solid var(--line);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
  }
  .seg button {
    border: 0;
    border-radius: 0;
    background: transparent;
  }
  .seg button.active {
    background: var(--bg-3);
    color: var(--accent);
  }
  .note {
    margin: 0 0 10px;
    font-size: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--w), 1fr));
    gap: 10px;
  }
  .tile {
    position: relative;
    background: var(--bg-2);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 6px;
    transition: opacity 0.1s;
  }
  .tile:hover {
    border-color: #4a443f;
  }
  .seqs {
    padding: 6px 10px;
    margin-bottom: 8px;
  }
  .seq {
    color: var(--accent-2);
  }
  .dupes {
    padding: 6px 10px;
    margin-bottom: 8px;
    border-color: var(--warn);
  }
  .x.why {
    background: rgba(20, 18, 17, 0.88);
    color: var(--warn);
  }
  .tile.excluded {
    opacity: 0.35;
  }
  .meta {
    padding: 6px 2px 0;
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .small {
    font-size: 11px;
  }
  .badge {
    font-size: 10px;
    background: var(--bg-3);
    border-radius: 4px;
    padding: 1px 5px;
    color: var(--muted);
  }
  .x {
    position: absolute;
    top: 10px;
    left: 10px;
    background: var(--bad);
    color: #fff;
    font-size: 10px;
    padding: 2px 6px;
    border-radius: 4px;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .line {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 6px 8px;
    border-radius: 4px;
  }
  .line:hover {
    background: var(--bg-2);
  }
  .line.excluded {
    opacity: 0.4;
  }
  .right {
    margin-left: auto;
  }
</style>
