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
    store.startPrint({ ids, preset, jpeg, exr, camera_jpeg: cameraJpeg });
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
  <h1>Print {ids.length} photo{ids.length === 1 ? "" : "s"}</h1>
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
</style>
