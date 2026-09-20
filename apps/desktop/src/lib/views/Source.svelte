<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, human, type SourceInfo } from "../api";
  import { store } from "../state.svelte";

  let sources = $state<SourceInfo[]>([]);
  let custom = $state<string | null>(null);

  async function refresh() {
    sources = await api.listSources();
  }

  async function pick() {
    const dir = await open({ directory: true, multiple: false, title: "Choose a folder to import" });
    if (typeof dir === "string") custom = dir;
  }

  onMount(() => {
    refresh();
    const t = setInterval(refresh, 3000);
    return () => clearInterval(t);
  });
</script>

<h1>Source</h1>
<p class="muted">Cards are detected automatically when they contain a DCIM folder. Scanning reads only file headers.</p>

<div class="grid">
  <div class="card stack">
    <h2>Cards</h2>
    {#if sources.length === 0}
      <div class="muted">No card mounted.</div>
    {/if}
    {#each sources as s (s.root)}
      <div class="row spread src">
        <div>
          <div>{s.label}</div>
          <div class="muted mono">{s.root}</div>
        </div>
        <button class="primary" disabled={store.busy} onclick={() => store.startScan(s.root)}>Scan</button>
      </div>
    {/each}
    <div class="row">
      <button class="ghost" onclick={refresh}>Refresh</button>
    </div>
  </div>

  <div class="card stack">
    <h2>Folder</h2>
    <div class="row">
      <button onclick={pick}>Choose folder…</button>
      {#if custom}
        <span class="mono muted">{custom}</span>
      {/if}
    </div>
    <div class="row">
      <button class="primary" disabled={!custom || store.busy} onclick={() => custom && store.startScan(custom)}>Scan folder</button>
    </div>
  </div>
</div>

{#if store.scanning}
  <div class="card" style="margin-top: 14px">
    <div class="row spread"><span>Scanning…</span><span class="muted">{store.scanProgress}</span></div>
  </div>
{/if}
{#if store.scanError}
  <div class="card bad" style="margin-top: 14px">{store.scanError}</div>
{/if}
{#if store.scan && !store.scanning}
  <div class="card" style="margin-top: 14px">
    <div class="row spread">
      <span>Last scan: <b>{store.scan.source.label}</b> · {store.scan.groups.length} items · {human(store.scan.bytes)}</span>
      <button onclick={() => (store.view = "review")}>Review →</button>
    </div>
  </div>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .src {
    padding: 8px 0;
    border-top: 1px solid var(--line);
  }
</style>
