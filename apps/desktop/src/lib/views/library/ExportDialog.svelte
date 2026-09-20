<script lang="ts">
  // One export run: what to write (a printed look, the developed photo, the
  // camera JPEG, or a combination), and where.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, DEVELOPED_ONLY, type PresetSummary } from "../../api";
  import { library as lib } from "../../library.svelte";
  import { store } from "../../state.svelte";

  let { ids, fromQueue, onclose }: { ids: number[]; fromQueue: boolean; onclose: () => void } = $props();

  let presets = $state<PresetSummary[]>([]);
  let look = $state<string>(DEVELOPED_ONLY);
  let render = $state(true);
  let jpeg = $state(true);
  let exr = $state(false);
  let cameraJpeg = $state(false);
  let destination = $state<string | null>(null);

  const dest = $derived(destination ?? store.config?.render_root ?? "");
  const canRun = $derived((render && (jpeg || exr)) || cameraJpeg);

  onMount(async () => {
    presets = await api.listPresets();
    const configured = store.config?.preset ?? "";
    look = presets.find((p) => p.path === configured)?.path ?? DEVELOPED_ONLY;
    jpeg = store.config?.outputs.jpeg ?? true;
    exr = store.config?.outputs.exr ?? false;
    if (!jpeg && !exr) jpeg = true;
  });

  async function pickFolder() {
    const dir = await open({ directory: true, multiple: false, title: "Export into" });
    if (typeof dir === "string") destination = dir;
  }

  async function run() {
    if (!canRun) return;
    store.startExport({
      ids,
      look: render ? look : null,
      jpeg: render && jpeg,
      exr: render && exr,
      camera_jpeg: cameraJpeg,
      destination,
      clear_queue: fromQueue,
    });
    onclose();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="dialog card stack" role="dialog" aria-modal="true" aria-label="Export">
  <h1>Export {ids.length} photo{ids.length === 1 ? "" : "s"}{fromQueue ? " from the queue" : ""}</h1>

  {#if ids.length}
    <div class="strip">
      {#each ids.slice(0, 24) as id (id)}
        {@const t = lib.thumbs.get(id)}
        <span class="thumb">
          {#if t}<img src={api.fileUrl(t)} alt="" />{/if}
          {#if fromQueue}
            <button class="x" title="Remove from the queue" onclick={() => lib.queueRemove([id])}>×</button>
          {/if}
        </span>
      {/each}
      {#if ids.length > 24}<span class="muted small more">+{ids.length - 24}</span>{/if}
    </div>
  {/if}

  <label class="row"><input type="checkbox" bind:checked={render} /> <span>Render each photo</span></label>
  {#if render}
    <label class="stack tight indent">
      <span class="muted small">Look</span>
      <select bind:value={look}>
        <option value={DEVELOPED_ONLY}>No film look — developed only</option>
        {#each presets as p (p.path)}
          <option value={p.path}>{p.name} — {p.film.replaceAll("_", " ")}</option>
        {/each}
      </select>
    </label>
    <div class="row indent">
      <label class="row"><input type="checkbox" bind:checked={jpeg} /> JPEG</label>
      <label class="row"><input type="checkbox" bind:checked={exr} /> EXR ({store.config?.outputs.exr_color_space})</label>
    </div>
  {/if}

  <label class="row">
    <input type="checkbox" bind:checked={cameraJpeg} />
    <span>Copy the camera JPEG when the photo has one</span>
  </label>
  {#if cameraJpeg && render}
    <div class="muted small indent">Copies land beside the render with a <span class="mono">-camera</span> suffix, so neither overwrites the other.</div>
  {/if}

  <div class="stack tight">
    <span class="muted small">Into</span>
    <div class="row">
      <span class="mono path">{dest}</span>
      <button onclick={pickFolder}>Choose…</button>
      {#if destination}<button class="ghost" onclick={() => (destination = null)}>Render folder</button>{/if}
    </div>
    <span class="muted small">Named by the render templates in Settings; an existing render of another look is never overwritten.</span>
  </div>

  {#if store.busy}<div class="warn small">Another job is running; wait for it to finish.</div>{/if}
  <div class="row end">
    {#if fromQueue}
      <button class="ghost" onclick={() => lib.queueClear()}>Empty queue</button>
    {/if}
    <span class="spacer"></span>
    <button onclick={onclose}>Cancel</button>
    <button class="primary" disabled={!canRun || store.busy} onclick={run}>Export</button>
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
    top: 12%;
    left: 50%;
    transform: translateX(-50%);
    width: min(560px, 92vw);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }
  .strip {
    display: flex;
    gap: 4px;
    overflow-x: auto;
    padding-bottom: 2px;
  }
  .thumb {
    position: relative;
    width: 56px;
    height: 42px;
    flex: none;
    background: var(--bg-3);
    border-radius: 4px;
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .thumb .x {
    position: absolute;
    top: 0;
    right: 0;
    padding: 0 4px;
    background: rgba(0, 0, 0, 0.6);
    border: none;
    border-radius: 0 0 0 4px;
    line-height: 1.2;
  }
  .more {
    align-self: center;
  }
  .indent {
    padding-left: 22px;
  }
  .path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tight {
    gap: 3px;
  }
  .small {
    font-size: 11.5px;
  }
  .end {
    justify-content: flex-end;
  }
  .spacer {
    flex: 1;
  }
</style>
