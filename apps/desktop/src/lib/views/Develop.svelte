<script lang="ts">
  // The develop stage: a photo's own decode settings (white balance, exposure,
  // highlights), with nothing filmic on top. Saved with the photo, so every
  // print of it starts here.
  import { onMount } from "svelte";
  import { api, catalog, DEVELOPED_ONLY } from "../api";
  import { develop as D } from "../photo.svelte";
  import { library as lib } from "../library.svelte";
  import { store } from "../state.svelte";
  import JobPanel from "./JobPanel.svelte";
  import { panzoom } from "../panzoom";

  let zoom = $state(1);

  const candidates = $derived.by(() => {
    const ids = lib.selected.size ? [...lib.selected] : lib.focus !== null ? [lib.focus] : [];
    return ids.slice(0, 40);
  });

  onMount(async () => {
    const first = D.id ?? lib.focus ?? candidates[0] ?? null;
    if (first !== null) await D.setPhoto(first);
    const missing = candidates.filter((id) => !lib.thumbs.has(id));
    if (missing.length) for (const t of await catalog.requestThumbs(missing).catch(() => [])) lib.thumbs.set(t.id, t.path);
  });

  function exportDeveloped() {
    const ids = lib.selected.size ? [...lib.selected] : D.id !== null ? [D.id] : [];
    if (!ids.length) {
      store.say("Select photos in the Library first.");
      return;
    }
    store.startPrint({ ids, preset: DEVELOPED_ONLY, jpeg: store.config?.outputs.jpeg ?? true, exr: store.config?.outputs.exr ?? false });
  }
</script>

<div class="develop">
  <section class="center">
    <div class="bar row spread">
      <div class="row">
        <h1>Develop</h1>
        <span class="muted small">{D.input?.file ?? ""}</span>
      </div>
      <div class="row">
        {#if D.rendering}<span class="muted small">rendering…</span>{/if}
        <button disabled={D.isDefault} onclick={() => D.reset()}>Reset</button>
        {#if lib.selected.size > 1}
          <button onclick={() => D.applyTo([...lib.selected])}>Apply to {lib.selected.size} selected</button>
        {/if}
        <button onclick={() => (store.view = "print")}>Print look…</button>
        <button class="primary" disabled={store.busy} onclick={exportDeveloped}>
          Export {lib.selected.size > 1 ? `${lib.selected.size} developed` : "developed"}
        </button>
      </div>
    </div>
    <div class="stage" use:panzoom={{
        key: D.id,
        onzoom: (z) => {
          zoom = z;
          D.setZoom(z, () => D.refresh());
        },
      }} role="img" aria-label="Preview">
      <div class="view">
      {#if D.id === null}
        <div class="muted empty">Select photos in the Library; the first one opens here.</div>
      {:else if D.preview}
        <img src={D.preview} alt="Developed" />
      {:else if D.error}
        <div class="bad empty">Can't develop this photo: {D.error}</div>
      {:else}
        <div class="muted empty">Decoding…</div>
      {/if}
      </div>
      <div class="tag zoom right">
        {#if zoom !== 1}<span>{Math.round(zoom * 100)}%</span>{/if}
        <button class="full" class:on={D.full} onclick={() => D.setFull(!D.full, () => D.refresh())} title="Render every pixel ({D.native ? `${D.native} px` : 'full size'}) instead of a fitted preview — slower">
          {D.full ? "full res" : `${D.detail} px`}
        </button>
      </div>
    </div>
    {#if candidates.length > 1}
      <div class="strip">
        {#each candidates as id (id)}
          {@const t = lib.thumbs.get(id)}
          <button class="thumb" class:active={id === D.id} onclick={() => D.setPhoto(id)}>
            {#if t}<img src={api.fileUrl(t)} alt="" />{:else}<span class="muted small">#{id}</span>{/if}
          </button>
        {/each}
      </div>
    {/if}
    <JobPanel />
  </section>

  <aside class="controls">
    <p class="muted small note">
      {#if D.isRaw}
        An honest decode: camera white balance and no tone curve. What you set here is saved with the photo and used everywhere — exports and prints alike.
      {:else}
        A camera JPEG: nothing to decode, its colour is already baked in. Only exposure applies. Saved with the photo.
      {/if}
    </p>
    {#if D.isRaw}
      <label class="stack tight"><span class="small">White balance</span>
        <select value={D.raw.white_balance} onchange={(e) => D.setRaw({ white_balance: (e.target as HTMLSelectElement).value as any })}>
          <option value="as_shot">As shot</option>
          <option value="daylight">Daylight</option>
          <option value="tungsten">Tungsten</option>
          <option value="custom">Custom</option>
        </select>
      </label>
      {#if D.raw.white_balance === "custom"}
        <label class="stack tight"><span class="small row spread">Temperature <span class="mono">{Math.round(D.raw.temperature)} K</span></span>
          <input type="range" min="2000" max="12000" step="50" value={D.raw.temperature} oninput={(e) => D.setRaw({ temperature: Number((e.target as HTMLInputElement).value) })} />
        </label>
        <label class="stack tight"><span class="small row spread">Tint <span class="mono">{D.raw.tint.toFixed(2)}</span></span>
          <input type="range" min="0.8" max="1.2" step="0.005" value={D.raw.tint} oninput={(e) => D.setRaw({ tint: Number((e.target as HTMLInputElement).value) })} />
        </label>
      {/if}
    {/if}
    <label class="stack tight"><span class="small row spread">Exposure <span class="mono">{D.raw.exposure_ev.toFixed(2)} EV</span></span>
      <input type="range" min="-3" max="3" step="0.05" value={D.raw.exposure_ev} oninput={(e) => D.setRaw({ exposure_ev: Number((e.target as HTMLInputElement).value) })} />
    </label>
    {#if D.isRaw}
      <label class="stack tight"><span class="small">Highlights</span>
        <select value={D.raw.highlight} onchange={(e) => D.setRaw({ highlight: Number((e.target as HTMLSelectElement).value) })}>
          <option value={0}>Clip</option>
          <option value={1}>Unclip</option>
          <option value={2}>Blend</option>
          <option value={5}>Rebuild</option>
        </select>
      </label>
      <label class="stack tight"><span class="small">Demosaic</span>
        <select value={D.raw.demosaic ?? 3} onchange={(e) => D.setRaw({ demosaic: Number((e.target as HTMLSelectElement).value) })}>
          <option value={3}>Best (AHD / X-Trans 3-pass)</option>
          <option value={2}>Faster (PPG)</option>
          <option value={1}>VNG</option>
          <option value={0}>None (bilinear)</option>
        </select>
      </label>
    {/if}
    <p class="muted small note">
      Export writes the developed photo with no film look, into the render folder. A print look is optional, in the Print tab.
    </p>
  </aside>
</div>

<style>
  .develop {
    display: grid;
    grid-template-columns: 1fr 300px;
    height: 100%;
    min-height: 0;
  }
  .center {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    padding: 10px 14px;
    gap: 8px;
  }
  .bar {
    flex-wrap: wrap;
    row-gap: 6px;
  }
  h1 {
    margin: 0;
  }
  .controls {
    overflow-y: auto;
    padding: 12px;
    background: var(--bg-2);
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .stage {
    flex: 1;
    min-height: 0;
    position: relative;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #0e0d0c;
    border-radius: var(--radius);
  }
  .tag {
    position: absolute;
    top: 10px;
    left: 10px;
    background: rgba(0, 0, 0, 0.65);
    padding: 3px 9px;
    border-radius: 10px;
    font-size: 11.5px;
  }
  .tag.right {
    left: auto;
    right: 10px;
  }
  .view {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .stage img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .strip {
    display: flex;
    gap: 6px;
    overflow-x: auto;
  }
  .thumb {
    padding: 0;
    width: 64px;
    height: 48px;
    flex: none;
    overflow: hidden;
  }
  .thumb.active {
    border-color: var(--accent);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .tight {
    gap: 3px;
  }
  .small {
    font-size: 11.5px;
  }
  .note {
    margin: 0;
  }
  .empty {
    padding: 40px;
  }
  .tag.zoom {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .full {
    background: transparent;
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0 5px;
    font-size: 10.5px;
    color: var(--muted);
    line-height: 16px;
  }
  .full.on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
</style>
