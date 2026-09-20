<script lang="ts">
  // Right-click menu for photos in the grid and the full-screen viewer.
  import { api, catalog } from "../../api";
  import { library as lib } from "../../library.svelte";
  import { develop } from "../../photo.svelte";
  import { store } from "../../state.svelte";

  const menu = $derived(lib.menu);
  const ids = $derived(menu?.ids ?? []);
  const one = $derived(ids.length === 1);

  function close() {
    lib.menu = null;
  }

  async function reveal() {
    const id = ids[0];
    close();
    const d = await catalog.asset(id).catch(() => null);
    const file = d?.files.find((f) => f.role === "raw") ?? d?.files[0];
    if (file) api.reveal(file.path);
  }

  /** Run an action on the photos the menu opened with; closing first would
   * empty the derived list before the action reads it. */
  function act(fn: (ids: number[]) => void) {
    const snapshot = [...ids];
    close();
    fn(snapshot);
  }
</script>

{#if menu}
  <div class="backdrop" role="presentation" oncontextmenu={(e) => { e.preventDefault(); close(); }} onclick={close}></div>
  <div class="menu card" style="left: {menu.x}px; top: {menu.y}px" role="menu" tabindex="-1">
    <button role="menuitem" onclick={() => act((sel) => lib.askExport(sel))}>Export{one ? "" : ` ${ids.length}`}…</button>
    <button role="menuitem" onclick={() => act((sel) => lib.queueAdd(sel))}>Add to export queue</button>
    {#if ids.some((id) => lib.queue.has(id))}
      <button role="menuitem" onclick={() => act((sel) => lib.queueRemove(sel))}>Remove from export queue</button>
    {/if}
    <div class="sep"></div>
    <button role="menuitem" onclick={() => act((sel) => { develop.setPhoto(sel[0]); store.view = "develop"; })}>Develop…</button>
    <button role="menuitem" onclick={() => act((sel) => { develop.setPhoto(sel[0]); store.view = "print"; })}>Print look…</button>
    <button role="menuitem" onclick={() => act(() => lib.askPrint())}>Print{one ? "" : ` ${ids.length}`}…</button>
    <div class="sep"></div>
    <button role="menuitem" onclick={reveal}>Reveal in Finder</button>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .menu {
    position: fixed;
    z-index: 31;
    padding: 4px;
    min-width: 190px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
  }
  .menu button {
    text-align: left;
    background: transparent;
    border-color: transparent;
    padding: 5px 9px;
  }
  .menu button:hover {
    background: var(--bg-3);
  }
  .sep {
    height: 1px;
    background: var(--line);
    margin: 3px 0;
  }
</style>
