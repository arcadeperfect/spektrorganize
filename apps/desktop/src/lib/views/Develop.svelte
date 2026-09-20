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
  import CropOverlay from "./CropOverlay.svelte";

  let zoom = $state(1);
  /** Crop mode: the overlay only appears while you are cropping. */
  let cropping = $state(false);
  let ratio = $state("free");

  const RATIOS: { id: string; label: string; value: number | null; hint: string }[] = [
    { id: "free", label: "free", value: null, hint: "Any shape" },
    { id: "orig", label: "orig", value: null, hint: "The photo's own shape" },
    { id: "1:1", label: "1×1", value: 1, hint: "Square" },
    { id: "3:2", label: "3×2", value: 3 / 2, hint: "35mm" },
    { id: "4:3", label: "4×3", value: 4 / 3, hint: "Four thirds" },
    { id: "5:4", label: "5×4", value: 5 / 4, hint: "Large format" },
    { id: "16:9", label: "16×9", value: 16 / 9, hint: "Widescreen" },
  ];

  /** Turn the chosen shape on its side, so one entry covers both orientations. */
  function flip() {
    const r = RATIOS.find((x) => x.id === ratio);
    if (!r?.value) return;
    portrait = !portrait;
    pickRatio(ratio, portrait);
  }
  let portrait = $state(false);

  /**
   * The ratio to hold, expressed in frame fractions: a 1:1 crop of a 3:2 photo
   * is a rectangle 2/3 as wide as it is tall in those fractions.
   */
  const frameAspect = $derived.by(() => {
    const el = imgEl;
    const w = el?.naturalWidth ?? 0;
    const h = el?.naturalHeight ?? 0;
    return w && h ? w / h : 1.5;
  });
  /** Lock holds whatever shape the rectangle is now, whatever the preset says. */
  let locked = $state(false);
  let lockedAspect = $state<number | null>(null);

  const holdAspect = $derived.by(() => {
    if (locked) return lockedAspect;
    if (ratio === "free") return null;
    let want = ratio === "orig" ? frameAspect : (RATIOS.find((r) => r.id === ratio)?.value ?? null);
    if (want === null) return null;
    if (portrait && ratio !== "orig") want = 1 / want;
    return want / frameAspect;
  });

  function toggleLock() {
    locked = !locked;
    if (!locked) {
      lockedAspect = null;
      return;
    }
    // Freeze the rectangle as it stands; with no rectangle yet, the frame's own shape.
    const c = D.raw.crop;
    lockedAspect = c && c.h > 0 ? c.w / c.h : 1;
  }

  let imgEl = $state<HTMLImageElement | undefined>();

  function setCrop(c: { x: number; y: number; w: number; h: number }) {
    D.setRaw({ crop: c });
  }

  /** Picking a shape reshapes the rectangle that is already there. */
  function pickRatio(id: string, keepPortrait = false) {
    ratio = id;
    if (!keepPortrait) portrait = false;
    locked = false;
    lockedAspect = null;
    const a = holdAspect;
    const c = D.raw.crop;
    if (!a || !c) return;
    // Keep the centre, fit the new shape inside the old rectangle.
    const w = Math.min(c.w, c.h * a);
    const h = w / a;
    setCrop({ x: Math.min(Math.max(c.x + c.w / 2 - w / 2, 0), 1 - w), y: Math.min(Math.max(c.y + c.h / 2 - h / 2, 0), 1 - h), w, h });
  }

  function turn(dir: -1 | 1) {
    D.setRaw({ rotate: (D.raw.rotate + dir + 4) % 4, crop: null });
  }

  const candidates = $derived.by(() => {
    const ids = lib.selected.size ? [...lib.selected] : lib.focus !== null ? [lib.focus] : [];
    return ids.slice(0, 40);
  });

  // Leaving Develop puts the preview back to the cropped frame.
  $effect(() => () => D.setWholeFrame(false));

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
        <div class="frame">
          <img src={D.preview} alt="Developed" bind:this={imgEl} />
          {#if cropping}
            <CropOverlay crop={D.raw.crop} aspect={holdAspect} {zoom} onchange={setCrop} />
          {/if}
        </div>
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
    <div class="geo stack tight">
      <div class="row spread">
        <span class="small">Geometry</span>
        {#if D.raw.rotate || D.raw.straighten || D.raw.crop}
          <button class="mini" onclick={() => D.setRaw({ rotate: 0, straighten: 0, crop: null })}>reset</button>
        {/if}
      </div>
      <div class="row">
        <button class="mini" onclick={() => turn(-1)} title="Rotate 90° anticlockwise">⟲ 90°</button>
        <button class="mini" onclick={() => turn(1)} title="Rotate 90° clockwise">⟳ 90°</button>
        <button
          class="mini"
          class:on={cropping}
          onclick={() => {
            cropping = !cropping;
            // While cropping, the preview shows the whole frame to draw on.
            D.setWholeFrame(cropping);
          }}
          title="Drag a rectangle on the photo"
        >
          {cropping ? "done" : "crop"}
        </button>
      </div>
      {#if cropping}
        <!-- The shapes, one click each, rather than buried in a menu. -->
        <div class="ratios">
          {#each RATIOS as r (r.id)}
            <button class="mini" class:on={!locked && ratio === r.id} onclick={() => pickRatio(r.id)} title={r.hint}>
              {portrait && r.value ? r.label.split("×").reverse().join("×") : r.label}
            </button>
          {/each}
          <button
            class="mini"
            onclick={flip}
            disabled={locked || ratio === "free" || ratio === "1:1"}
            title="Turn the shape on its side (3:2 ↔ 2:3)">⇄</button
          >
          <button
            class="mini"
            class:on={locked}
            onclick={toggleLock}
            title={locked ? "Shape is locked — click to resize freely" : "Lock the shape the rectangle has now"}
          >
            {locked ? "🔒" : "🔓"}
          </button>
        </div>
        {#if D.raw.crop}
          <div class="row">
            <button class="mini" onclick={() => D.setRaw({ crop: null })}>clear crop</button>
          </div>
        {/if}
      {/if}
      <label class="stack tight"><span class="small row spread">Straighten <span class="mono">{D.raw.straighten.toFixed(1)}°</span></span>
        <input
          type="range"
          min="-15"
          max="15"
          step="0.1"
          value={D.raw.straighten}
          oninput={(e) => D.setRaw({ straighten: Number((e.target as HTMLInputElement).value) }, true)}
          ondblclick={() => D.setRaw({ straighten: 0 })}
          title="Double-click to level"
        />
      </label>
    </div>

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
          <input type="range" min="2000" max="12000" step="50" value={D.raw.temperature} oninput={(e) => D.setRaw({ temperature: Number((e.target as HTMLInputElement).value) }, true)} />
        </label>
        <label class="stack tight"><span class="small row spread">Tint <span class="mono">{D.raw.tint.toFixed(2)}</span></span>
          <input type="range" min="0.8" max="1.2" step="0.005" value={D.raw.tint} oninput={(e) => D.setRaw({ tint: Number((e.target as HTMLInputElement).value) }, true)} />
        </label>
      {/if}
    {/if}
    <label class="stack tight"><span class="small row spread">Exposure <span class="mono">{D.raw.exposure_ev.toFixed(2)} EV</span></span>
      <input type="range" min="-3" max="3" step="0.05" value={D.raw.exposure_ev} oninput={(e) => D.setRaw({ exposure_ev: Number((e.target as HTMLInputElement).value) }, true)} />
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
  .frame {
    position: relative;
    display: inline-block;
    max-width: 100%;
    max-height: 100%;
    line-height: 0;
  }
  .geo {
    border-bottom: 1px solid var(--line);
    padding-bottom: 8px;
    margin-bottom: 4px;
  }
  .ratios {
    display: flex;
    flex-wrap: wrap;
    gap: 3px;
  }
  .geo .on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
</style>
