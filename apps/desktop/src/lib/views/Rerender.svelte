<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type ManifestSummary } from "../api";
  import { store } from "../state.svelte";
  import JobPanel from "./JobPanel.svelte";

  let manifests = $state<ManifestSummary[]>([]);
  let selected = $state<string | null>(null);
  let exr = $state(false);
  let jpeg = $state(true);
  let renderRoot = $state<string | null>(null);
  let onlyMissing = $state(true);

  onMount(async () => {
    manifests = await api.listManifests();
    if (store.config) {
      exr = store.config.outputs.exr;
      jpeg = store.config.outputs.jpeg;
    }
  });

  async function pickManifest() {
    const f = await open({ multiple: false, filters: [{ name: "Manifest", extensions: ["json"] }] });
    if (typeof f === "string") selected = f;
  }
  async function pickRoot() {
    const d = await open({ directory: true, multiple: false });
    if (typeof d === "string") renderRoot = d;
  }
  function run() {
    if (!selected) return;
    store.startRender({ manifest: selected, exr, jpeg, render_root: renderRoot, only_missing: onlyMissing });
  }
</script>

<h1>Render</h1>
<p class="muted">Renders the RAWs recorded in an import manifest, using the preset stored in it. Works after the archive has moved; point it at the manifest inside the archive.</p>

<div class="cols">
  <div class="card stack">
    <h2>Manifests in {store.config?.archive_root}</h2>
    {#if manifests.length === 0}
      <div class="muted">none found</div>
    {/if}
    {#each manifests as m (m.path)}
      <button class="man" class:active={selected === m.path} onclick={() => (selected = m.path)}>
        <div class="row spread">
          <span>{m.created_at} · {m.source_label}</span>
          <span class="muted">{m.raws} RAW · {m.rendered} rendered</span>
        </div>
        <div class="muted small">{m.preset ?? "no preset"}</div>
      </button>
    {/each}
    <div class="row"><button onclick={pickManifest}>Choose a manifest file…</button></div>
  </div>

  <div class="stack">
    <div class="card stack">
      <h2>Outputs</h2>
      <label class="row"><input type="checkbox" bind:checked={exr} /> EXR</label>
      <label class="row"><input type="checkbox" bind:checked={jpeg} /> JPEG</label>
      <label class="row"><input type="checkbox" bind:checked={onlyMissing} /> Only entries missing these outputs</label>
      <div class="row">
        <span class="muted">Render root</span>
        <input type="text" class="mono" value={renderRoot ?? ""} placeholder="as recorded in the manifest" oninput={(e) => (renderRoot = (e.currentTarget as HTMLInputElement).value || null)} />
        <button onclick={pickRoot}>…</button>
      </div>
      {#if selected}<div class="mono small muted">{selected}</div>{/if}
      <div class="row">
        <button class="primary" disabled={!selected || store.busy || (!exr && !jpeg)} onclick={run}>Render</button>
      </div>
    </div>
    <JobPanel />
  </div>
</div>

<style>
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
    align-items: start;
  }
  .man {
    text-align: left;
    display: block;
    width: 100%;
    background: transparent;
  }
  .man.active {
    border-color: var(--accent);
  }
  .small {
    font-size: 11px;
  }
</style>
