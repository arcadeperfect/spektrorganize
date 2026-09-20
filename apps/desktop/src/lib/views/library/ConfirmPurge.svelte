<script lang="ts">
  // The last screen before anything leaves the disk: every path that will go,
  // in full, with one button to do it. Nothing here is a summary — if a file is
  // not on this list it is not touched.
  import { human } from "../../api";

  interface Doomed {
    path: string;
    size?: number;
    present?: boolean;
  }

  interface Props {
    title: string;
    files: Doomed[];
    /** Paths that are being kept, when the choice was "one of these". */
    keeping?: string[];
    hardDelete: boolean;
    busy?: boolean;
    note?: string | null;
    onexecute: () => void;
    oncancel: () => void;
  }

  let { title, files, keeping = [], hardDelete, busy = false, note = null, onexecute, oncancel }: Props = $props();

  /** The copies that are staying: useful to check, and easy to put away. */
  let showKeeping = $state(true);

  const bytes = $derived(files.reduce((n, f) => n + (f.size ?? 0), 0));
  const missing = $derived(files.filter((f) => f.present === false).length);
  const verb = $derived(hardDelete ? "Delete for good" : "Move to the Trash");
</script>

<div class="confirm">
  <div class="head">
    <h2>{title}</h2>
    <p class="small" class:bad={hardDelete}>
      {files.length}
      {files.length === 1 ? "file" : "files"}{bytes ? ` · ${human(bytes)}` : ""} ·
      {#if hardDelete}
        <b>deleted outright — there is no getting these back</b>
      {:else}
        moved to the Trash, where you can put them back
      {/if}
      {#if missing}· <span class="warn">{missing} already gone from disk</span>{/if}
    </p>
    {#if note}<p class="muted small">{note}</p>{/if}
  </div>

  <div class="cols">
    <div class="pane">
      <h3 class="small">Going</h3>
      <ul class="paths mono">
        {#each files as f (f.path)}
          <li class:gone={f.present === false}>{f.path}</li>
        {/each}
      </ul>
    </div>
    {#if keeping.length && showKeeping}
      <div class="pane">
        <h3 class="small ok">Staying</h3>
        <ul class="paths mono">
          {#each keeping as p (p)}
            <li>{p}</li>
          {/each}
        </ul>
      </div>
    {/if}
  </div>

  <div class="row foot">
    {#if keeping.length}
      <label class="row small muted keep-toggle">
        <input type="checkbox" bind:checked={showKeeping} /> show what stays
      </label>
    {/if}
    <button onclick={oncancel} disabled={busy}>Back</button>
    <button class={hardDelete ? "danger" : "primary"} onclick={onexecute} disabled={busy || !files.length}>
      {busy ? "Working…" : `${verb} (${files.length})`}
    </button>
  </div>
</div>

<style>
  .confirm {
    position: absolute;
    inset: 0;
    z-index: 16;
    background: var(--bg);
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    padding: 12px 14px;
    gap: 8px;
  }
  h2 {
    margin: 0 0 2px;
    font-size: 15px;
  }
  h3 {
    margin: 0 0 4px;
    color: var(--muted);
    font-weight: 600;
  }
  p {
    margin: 0;
  }
  .cols {
    display: flex;
    gap: 12px;
    min-height: 0;
  }
  .pane {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .paths {
    flex: 1;
    overflow: auto;
    margin: 0;
    padding: 6px 8px;
    list-style: none;
    background: var(--bg-2);
    border: 1px solid var(--line);
    border-radius: 6px;
    font-size: 11px;
    line-height: 1.6;
  }
  .paths li {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    direction: rtl;
    text-align: left;
  }
  .paths li.gone {
    color: var(--muted);
    text-decoration: line-through;
  }
  .foot {
    justify-content: flex-end;
  }
  .keep-toggle {
    margin-right: auto;
  }
  .danger {
    background: var(--bad);
    color: #180e0b;
    border-color: var(--bad);
  }
  .small {
    font-size: 11.5px;
  }
</style>
