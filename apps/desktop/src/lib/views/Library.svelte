<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { catalog, type SortKey } from "../api";
  import { library as lib } from "../library.svelte";
  import { store } from "../state.svelte";
  import Grid from "./library/Grid.svelte";
  import Sidebar from "./library/Sidebar.svelte";
  import Detail from "./library/Detail.svelte";
  import Loupe from "./library/Loupe.svelte";
  import ExportDialog from "./library/ExportDialog.svelte";
  import ContextMenu from "./library/ContextMenu.svelte";
  import PrintDialog from "./library/PrintDialog.svelte";
  import JobPanel from "./JobPanel.svelte";

  let text = $state(lib.filter.text ?? "");
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  function onSearch() {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => lib.setFilter({ text: text.trim() || null }), 220);
  }

  onMount(() => {
    lib.init();
  });

  async function addFolder() {
    const dir = await open({ directory: true, multiple: false, title: "Add a folder to the catalog (indexed in place, never moved)" });
    if (typeof dir === "string") lib.addFolder(dir);
  }

  // Active filters as removable chips.
  const chips = $derived.by(() => {
    const f = lib.filter;
    const out: { label: string; clear: () => void }[] = [];
    const roots = lib.facets?.roots ?? [];
    for (const r of f.roots ?? []) out.push({ label: roots.find((x) => x.id === r)?.label ?? `root ${r}`, clear: () => lib.toggle("roots", r) });
    if (f.date_from || f.date_to) out.push({ label: `${f.date_from ?? "…"} – ${f.date_to ?? "…"}`, clear: () => lib.setFilter({ date_from: null, date_to: null }) });
    if (f.undated) out.push({ label: "undated", clear: () => lib.setFilter({ undated: false }) });
    for (const c of f.cameras ?? []) out.push({ label: c || "unknown camera", clear: () => lib.toggle("cameras", c) });
    for (const k of f.keywords ?? []) out.push({ label: `#${k}`, clear: () => lib.toggle("keywords", k) });
    if (f.has_render != null) out.push({ label: f.has_render ? "printed" : "not printed", clear: () => lib.setFilter({ has_render: null }) });
    if (f.ai_labelled != null) out.push({ label: f.ai_labelled ? "✦ labelled" : "not labelled", clear: () => lib.setFilter({ ai_labelled: null }) });
    if (f.missing != null) out.push({ label: "missing files", clear: () => lib.setFilter({ missing: null }) });
    return out;
  });

  const empty = $derived(lib.loaded && lib.facets?.total === 0 && !lib.indexing);
  const showJob = $derived(store.job.phase !== "idle" && store.job.kind === "print");
</script>

<div class="lib">
  {#if lib.loupe !== null}
    <Loupe />
  {/if}
  {#if lib.exportAsk}
    <ExportDialog ids={lib.exportAsk.ids} fromQueue={lib.exportAsk.fromQueue} onclose={() => (lib.exportAsk = null)} />
  {/if}
  <ContextMenu />
  <header class="bar">
    <span class="count">
      {lib.total.toLocaleString()} photo{lib.total === 1 ? "" : "s"}{#if lib.selected.size}<span class="muted">{` · ${lib.selected.size} selected`}</span>{/if}
    </span>
    {#each chips as c (c.label)}
      <span class="fchip">{c.label}<button class="x" onclick={c.clear} title="Remove filter">×</button></span>
    {/each}
    {#if chips.length > 1}<button class="mini" onclick={() => lib.clearFilter()}>clear</button>{/if}
    <span class="spacer"></span>
    <input class="search" type="search" placeholder="search names, folders, keywords" bind:value={text} oninput={onSearch} spellcheck="false" />
    <select bind:value={lib.sort} onchange={() => lib.reload()} title="Sort" class="sort">
      {#each [["captured_desc", "Newest"], ["captured_asc", "Oldest"], ["added_desc", "Recently added"], ["name", "Name"]] as [v, l] (v)}
        <option value={v as SortKey}>{l}</option>
      {/each}
    </select>
    <input type="range" min="110" max="340" bind:value={lib.tile} title="Thumbnail size" class="size" />
    <button onclick={addFolder} disabled={!!lib.indexing} title="Index an existing folder in place">+ Add folder…</button>
    <button onclick={() => (store.view = "source")} title="Copy from a card or folder into the archive, organised by your templates">Import…</button>
    <button class="ai" onclick={() => lib.askLabel()} disabled={!!lib.labelRun || lib.total === 0} title={lib.selected.size ? "AI keywords for the selection (replaces their AI keywords)" : "AI keywords for everything unlabelled in this view"}>
      ✦ Label{lib.selected.size ? ` ${lib.selected.size}` : ""}
    </button>
    <button onclick={() => lib.askExport()} disabled={store.busy} title="Export the queued photos">
      Queue{lib.queue.size ? ` (${lib.queue.size})` : ""}
    </button>
    <button class="primary" onclick={() => lib.askPrint()} disabled={lib.total === 0 || store.busy} title="Render through a film preset">Print…</button>
  </header>

  {#if lib.indexing}
    <div class="strip">
      <span>Indexing <b>{lib.indexing.label}</b> — {lib.indexing.text}</span>
      <span class="spacer"></span>
      <button class="mini" onclick={() => catalog.cancelIndex()}>Stop</button>
    </div>
  {/if}
  {#if lib.labelAsk}
    <div class="strip">
      <span>Label {lib.labelAsk.what} with AI keywords?{lib.labelAsk.replace ? " Their current AI keywords are replaced." : ""}</span>
      <span class="muted">Sends each 256 px thumbnail with its name, camera, date and folder to Claude with your API key. Your own keywords are never changed.</span>
      <span class="spacer"></span>
      <button class="primary" onclick={() => lib.runLabel()}>Label</button>
      <button onclick={() => (lib.labelAsk = null)}>Cancel</button>
    </div>
  {:else if lib.labelRun}
    <div class="strip">
      <span>✦ Labelling… batch {lib.labelRun.done}/{lib.labelRun.total} · {lib.labelRun.tagged} labelled</span>
      <progress max={lib.labelRun.total} value={lib.labelRun.done}></progress>
      <span class="spacer"></span>
      <button onclick={() => lib.stopLabel()}>Stop</button>
    </div>
  {/if}
  {#if lib.error}
    <div class="strip err"><span>{lib.error}</span><span class="spacer"></span><button class="mini" onclick={() => (lib.error = null)}>dismiss</button></div>
  {:else if lib.note}
    <div class="strip ok"><span>{lib.note}</span><span class="spacer"></span><button class="mini" onclick={() => (lib.note = null)}>ok</button></div>
  {/if}

  <div class="body">
    <Sidebar />
    <div class="center">
      {#if empty}
        <div class="welcome">
          <h1>Your catalog is empty</h1>
          <p class="muted">Photos stay where they are; the catalog only indexes them. RAW + camera JPEG pairs become one photo.</p>
          <div class="row">
            <button class="primary" onclick={addFolder}>Add a folder…</button>
            <button onclick={() => lib.adopt(store.config?.archive_root ?? null)} title={store.config?.archive_root}>Adopt my archive</button>
            <button onclick={() => (store.view = "source")}>Import from a card…</button>
          </div>
          <p class="muted small">“Adopt my archive” reads the import manifests in <span class="mono">{store.config?.archive_root}</span>, prints included. Nothing is written there.</p>
        </div>
      {:else}
        <Grid />
      {/if}
      {#if showJob}
        <div class="job">
          <div class="row spread">
            <b>Print</b>
          </div>
          <JobPanel />
        </div>
      {/if}
    </div>
    <Detail />
  </div>
</div>

{#if lib.printIds}
  <PrintDialog ids={lib.printIds} onclose={() => (lib.printIds = null)} />
{/if}

<style>
  .lib {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    position: relative;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .count {
    font-weight: 600;
    margin-right: 6px;
  }
  .spacer {
    flex: 1;
  }
  .search {
    flex: 0 1 220px;
    min-width: 110px;
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 5px 8px;
  }
  .search:focus {
    outline: none;
    border-color: var(--accent);
  }
  .sort {
    width: auto;
  }
  .size {
    width: 80px;
  }
  .fchip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: 11px;
    padding: 1px 2px 1px 8px;
    border-radius: 999px;
    background: #3a2a1c;
    color: var(--accent-2);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 210px minmax(0, 1fr) 300px;
  }
  .center {
    position: relative;
    min-width: 0;
    min-height: 0;
  }
  .welcome {
    max-width: 560px;
    margin: 80px auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .job {
    position: absolute;
    right: 12px;
    bottom: 12px;
    width: min(460px, calc(100% - 24px));
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 8px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
    gap: 6px;
    z-index: 5;
  }
  .small {
    font-size: 11.5px;
  }
</style>
