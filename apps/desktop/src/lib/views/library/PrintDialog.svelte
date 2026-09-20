<script lang="ts">
  import { onMount } from "svelte";
  import { api, DEVELOPED_ONLY, type PresetSummary } from "../../api";
  import { library as lib } from "../../library.svelte";
  import { store } from "../../state.svelte";

  let { ids, onclose }: { ids: number[]; onclose: () => void } = $props();

  let presets = $state<PresetSummary[]>([]);
  let preset = $state("");
  let jpeg = $state(true);
  let exr = $state(false);
  let cameraJpeg = $state(false);
  /** Clips in the selection are rendered frame by frame; these say how. */
  const clips = $derived(ids.filter((id) => lib.itemById(id)?.kind === "video").length);
  let clipLook = $state<"lut" | "full">("lut");
  let clipCodec = $state<"h264" | "hevc" | "pro_res422" | "pro_res4444">("h264");
  let clipPx = $state(1920);

  onMount(async () => {
    presets = await api.listPresets();
    const configured = store.config?.preset ?? "";
    const name = configured.split("/").pop();
    preset = presets.find((p) => p.path === configured || p.path.split("/").pop() === name)?.path ?? presets[0]?.path ?? "";
    jpeg = store.config?.outputs.jpeg ?? true;
    exr = store.config?.outputs.exr ?? false;
    if (!jpeg && !exr) jpeg = true;
  });

  function run() {
    if (!preset || (!jpeg && !exr)) return;
    store.startPrint({
      ids,
      preset,
      jpeg,
      exr,
      camera_jpeg: cameraJpeg,
      video: clips ? { look: clipLook, codec: clipCodec, max_px: clipPx, mbps: 12 } : undefined,
    });
    lib.note = null;
    onclose();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="dialog card stack" role="dialog" aria-modal="true" aria-label="Print">
  <h1>Print {ids.length} {ids.length === 1 ? "item" : "items"}</h1>
  <p class="muted small">
    Each photo is developed with its own settings, then printed with the look you choose, into <span class="mono">{store.config?.render_root}</span>, named by the render templates in Settings. A second look never overwrites
    the first: the preset name is added to the file name. Photos without a RAW are printed from their camera JPEG.
  </p>
  <label class="stack tight">
    <span class="muted small">Preset</span>
    <select bind:value={preset}>
      <option value={DEVELOPED_ONLY}>No film look — developed only</option>
      {#each presets as p (p.path)}
        <option value={p.path}>{p.name} — {p.film}{p.print ? " → " + p.print : ""}</option>
      {/each}
    </select>
  </label>
  <div class="row">
    <label class="row"><input type="checkbox" bind:checked={jpeg} /> JPEG</label>
    <label class="row"><input type="checkbox" bind:checked={exr} /> EXR ({store.config?.outputs.exr_color_space})</label>
  </div>
  <label class="row">
    <input type="checkbox" bind:checked={cameraJpeg} />
    <span>Also copy the camera JPEG when the photo has one</span>
  </label>
  {#if clips}
    <div class="clips stack tight">
      <span class="small">{clips} {clips === 1 ? "clip" : "clips"} in this selection</span>
      <div class="row">
        <button class="mini" class:on={clipLook === "lut"} onclick={() => (clipLook = "lut")}>baked LUT</button>
        <button class="mini" class:on={clipLook === "full"} onclick={() => (clipLook = "full")}>full pipeline</button>
        <select bind:value={clipCodec}>
          <option value="h264">H.264</option>
          <option value="hevc">HEVC</option>
          <option value="pro_res422">ProRes 422</option>
          <option value="pro_res4444">ProRes 4444</option>
        </select>
        <select bind:value={clipPx}>
          <option value={0}>as shot</option>
          <option value={1080}>1080</option>
          <option value={1920}>1920</option>
          <option value={3840}>3840</option>
        </select>
      </div>
      <span class="muted small">
        {clipLook === "lut"
          ? "The look baked into a colour cube: fast, but no grain or halation — a cube cannot carry them."
          : "The whole pipeline on every frame, grain and all, at minutes per clip."} Rendered clips have no sound yet.
      </span>
    </div>
  {/if}
  {#if store.busy}<div class="warn small">Another job is running; wait for it to finish.</div>{/if}
  <div class="row end">
    <button onclick={onclose}>Cancel</button>
    <button class="primary" disabled={!preset || (!jpeg && !exr) || store.busy} onclick={run}>Print</button>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 20;
  }
  .dialog {
    position: fixed;
    z-index: 21;
    top: 18%;
    left: 50%;
    transform: translateX(-50%);
    width: min(520px, 90vw);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }
  .tight {
    gap: 3px;
  }
  .end {
    justify-content: flex-end;
  }
  .small {
    font-size: 11.5px;
  }
  .clips {
    border-top: 1px solid var(--line);
    padding-top: 8px;
  }
  .clips .on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
</style>
