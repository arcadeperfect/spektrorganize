<script lang="ts">
  import { onMount } from "svelte";
  import { api, human, DEVELOPED_ONLY, type PresetSummary } from "../api";
  import { store } from "../state.svelte";
  import JobPanel from "./JobPanel.svelte";

  let render = $state(true);
  let presets = $state<PresetSummary[]>([]);
  onMount(async () => {
    presets = await api.listPresets();
  });

  $effect(() => {
    if (store.scan && !store.plan && !store.planError) store.refreshPlan();
  });

  const outputsLabel = $derived.by(() => {
    const o = store.config?.outputs;
    if (!o) return "";
    const parts = [];
    if (o.exr) parts.push("EXR");
    if (o.jpeg) parts.push("JPEG");
    return parts.length ? parts.join(" + ") : "nothing selected";
  });
  const canRun = $derived(!!store.plan && !store.busy && store.job.phase !== "copy" && store.job.phase !== "render");
</script>

<h1>Commit</h1>
<p class="muted">Copies everything first so the card is free as early as possible, writes the manifest, then renders from the archived copies.</p>

{#if store.plan && store.config}
  <div class="card stack">
    <div class="row spread">
      <span><b>{store.plan.files}</b> files · <b>{human(store.plan.bytes_to_copy)}</b> to copy{store.plan.existing ? ` · ${store.plan.existing} already in place` : ""}</span>
      <span class="muted">{store.included.length} of {store.scan?.groups.length} items included</span>
    </div>
    {#if store.plan.collisions}
      <div class="warn">{store.plan.collisions} destination collisions will be written with a numeric suffix. Check the Layout tab if that is not what you want.</div>
    {/if}
    <label class="row">
      <input type="checkbox" bind:checked={render} disabled={!store.config.outputs.exr && !store.config.outputs.jpeg} />
      <span>Also render {store.plan.renders} photo{store.plan.renders === 1 ? "" : "s"} to {outputsLabel}:</span>
      <select class="look" bind:value={store.config.preset} onchange={() => store.saveConfig()} disabled={!render}>
        <option value={DEVELOPED_ONLY}>No film look — developed only</option>
        {#each presets as p (p.path)}
          <option value={p.path}>{p.name}</option>
        {/each}
        {#if !presets.some((p) => p.path === store.config?.preset)}
          <option value={store.config.preset}>{store.config.preset.split("/").pop()}</option>
        {/if}
      </select>
    </label>
    <div class="row">
      <button class="primary" disabled={!canRun} onclick={() => store.startImport(render)}>Commit</button>
      {#if store.job.phase === "done"}
        <span class="muted">Finished — the photos are in the catalog.</span>
        <button onclick={() => (store.view = "library")}>Show in Library</button>
      {/if}
    </div>
  </div>
{:else if store.planError}
  <div class="card bad">{store.planError}</div>
{/if}

<div style="margin-top: 14px">
  <JobPanel />
</div>

<style>
  .look {
    width: 240px;
  }
</style>
