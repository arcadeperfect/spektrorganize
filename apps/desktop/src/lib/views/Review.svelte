<script lang="ts">
  import { human, type GroupView } from "../api";
  import { store } from "../state.svelte";
  import Thumb from "./Thumb.svelte";

  let filter = $state<"photos" | "videos" | "other">("photos");
  let size = $state(180);

  const photos = $derived(store.scan?.groups.filter((g) => g.kind === "raw" || g.kind === "image") ?? []);
  const videos = $derived(store.scan?.groups.filter((g) => g.kind === "video") ?? []);
  const others = $derived(store.scan?.groups.filter((g) => g.kind === "sidecar" || g.kind === "other") ?? []);
  const shown = $derived(filter === "photos" ? photos : filter === "videos" ? videos : others);
  const embedded = $derived(photos.filter((g) => g.preview === "embedded_preview").length);
  const excludedCount = $derived(store.scan?.groups.filter((g) => g.excluded).length ?? 0);

  function toggle(g: GroupView) {
    store.setExcluded([g.id], !g.excluded);
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

  {#if filter === "photos"}
    <div class="grid" style="--w: {size}px">
      {#each shown as g (g.id)}
        <div class="tile" class:excluded={g.excluded} onclick={() => toggle(g)} role="button" tabindex="0" onkeydown={(e) => e.key === " " && toggle(g)}>
          <Thumb group={g.id} />
          <div class="meta">
            <div class="row spread">
              <span class="name">{g.name}</span>
              <span class="badge">{g.kind === "raw" ? "RAW" : "JPG"}{g.attachments.length ? "+" + g.attachments.length : ""}</span>
            </div>
            <div class="muted small">{when(g)}{g.camera ? " · " + g.camera : ""}{g.iso ? " · ISO " + g.iso : ""}</div>
            <div class="muted small">{previewLabel(g.preview)}</div>
          </div>
          {#if g.excluded}
            <div class="x">excluded</div>
          {/if}
        </div>
      {/each}
    </div>
  {:else}
    <div class="list">
      {#each shown as g (g.id)}
        <div class="line" class:excluded={g.excluded} onclick={() => toggle(g)} role="button" tabindex="0" onkeydown={(e) => e.key === " " && toggle(g)}>
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
    margin-bottom: 10px;
    align-items: flex-start;
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
