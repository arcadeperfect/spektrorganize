<script lang="ts">
  import { api, human } from "../api";
  import { store } from "../state.svelte";

  const j = $derived(store.job);
  const copyPct = $derived(j.copyBytes ? Math.min(100, ((j.doneBytes + j.currentFileDone) / j.copyBytes) * 100) : 0);
  const renderPct = $derived(j.renderTotal ? ((j.renderDone + j.renderFailed) / j.renderTotal) * 100 : 0);
</script>

{#if j.phase !== "idle"}
  <div class="card stack">
    {#if j.copyFiles > 0 || j.phase === "copy"}
      <div class="row spread">
        <span>Copy {j.copied + j.failed}/{j.copyFiles} · {human(j.doneBytes + j.currentFileDone)} / {human(j.copyBytes)}</span>
        <span class="muted">{j.skipped ? `${j.skipped} skipped` : ""} {j.failed ? `· ${j.failed} failed` : ""}</span>
      </div>
      <div class="progress"><div style="width: {copyPct}%"></div></div>
      {#if j.phase === "copy" && j.currentFile}
        <div class="muted mono ellipsis">{j.currentFile}</div>
      {/if}
    {/if}

    {#if j.renderTotal > 0}
      <div class="row spread">
        <span>Render {j.renderDone + j.renderFailed}/{j.renderTotal} <span class="muted">on {j.backend}</span></span>
        <span class="muted">{j.renderFailed ? `${j.renderFailed} failed` : ""}</span>
      </div>
      <div class="progress"><div style="width: {renderPct}%"></div></div>
    {/if}

    {#if j.renderSkipped}
      <div class="skipped"><b>Files were copied, but nothing was rendered:</b> {j.renderSkipped}. Fix it in Settings, then use Render on this import's manifest.</div>
    {/if}

    {#if j.phase === "done"}
      <div class="row spread">
        <span class="ok">Done.</span>
        <div class="row">
          {#if j.manifest}<button onclick={() => api.reveal(j.manifest!)}>Reveal manifest</button>{/if}
          {#if j.outputs.length}<button onclick={() => api.reveal(j.outputs[0])}>Reveal renders</button>{/if}
          <button class="primary" onclick={() => store.dismissJob()}>Close</button>
        </div>
      </div>
    {:else if j.phase === "error"}
      <div class="row spread">
        <span class="bad">{j.error}</span>
        <button onclick={() => store.dismissJob()}>Close</button>
      </div>
    {:else}
      <div class="row">
        <button onclick={() => api.cancelJob()}>Cancel</button>
      </div>
    {/if}

    {#if j.log.length}
      <div class="log mono">
        {#each j.log as line, i (i)}<div>{line}</div>{/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .ellipsis {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .skipped {
    background: #3a2a1a;
    border: 1px solid var(--warn);
    border-radius: 6px;
    padding: 8px 10px;
  }
  .log {
    max-height: 160px;
    overflow: auto;
    color: var(--muted);
    font-size: 11px;
  }
</style>
