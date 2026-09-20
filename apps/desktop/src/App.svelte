<script lang="ts">
  import { onMount } from "svelte";
  import { store, type View } from "./lib/state.svelte";
  import { theme } from "./lib/theme.svelte";
  import Library from "./lib/views/Library.svelte";
  import Develop from "./lib/views/Develop.svelte";
  import Looks from "./lib/views/Looks.svelte";
  import Source from "./lib/views/Source.svelte";
  import Review from "./lib/views/Review.svelte";
  import Layout from "./lib/views/Layout.svelte";
  import Commit from "./lib/views/Commit.svelte";
  import Settings from "./lib/views/Settings.svelte";
  import Rerender from "./lib/views/Rerender.svelte";

  const steps: { id: View; label: string; needsScan: boolean }[] = [
    { id: "source", label: "Source", needsScan: false },
    { id: "review", label: "Review", needsScan: true },
    { id: "layout", label: "Layout", needsScan: true },
    { id: "commit", label: "Commit", needsScan: true },
  ];
  const importing = $derived(steps.some((s) => s.id === store.view));

  onMount(() => {
    theme.load();
    store.init();
  });
</script>

<div class="shell">
  <header class="titlebar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>spektrorganize</div>
    <nav class="tabs">
      <button class="tab" class:active={store.view === "library"} onclick={() => (store.view = "library")}>Library</button>
      <button class="tab" class:active={store.view === "develop"} onclick={() => (store.view = "develop")}>Develop</button>
      <button class="tab" class:active={store.view === "print"} onclick={() => (store.view = "print")}>Print</button>
      <span class="group" class:active={importing}>
        <span class="glabel">Import</span>
        {#each steps as s, i (s.id)}
          <button class="tab step" class:active={store.view === s.id} disabled={s.needsScan && !store.scan} onclick={() => (store.view = s.id)}>
            <span class="num">{i + 1}</span>{s.label}
          </button>
        {/each}
      </span>
      <button class="tab" class:active={store.view === "rerender"} onclick={() => (store.view = "rerender")}>Render</button>
      <button class="tab" class:active={store.view === "settings"} onclick={() => (store.view = "settings")}>Settings</button>
    </nav>
    <div class="drag" data-tauri-drag-region></div>
    {#if store.busy}
      <div class="busy">working…</div>
    {/if}
  </header>
  <main class:flush={store.view === "library" || store.view === "develop" || store.view === "print"}>
    {#if !store.config}
      <div class="muted">loading…</div>
    {:else if store.view === "library"}
      <Library />
    {:else if store.view === "develop"}
      <Develop />
    {:else if store.view === "print"}
      <Looks />
    {:else if store.view === "source"}
      <Source />
    {:else if store.view === "review"}
      <Review />
    {:else if store.view === "layout"}
      <Layout />
    {:else if store.view === "commit"}
      <Commit />
    {:else if store.view === "settings"}
      <Settings />
    {:else if store.view === "rerender"}
      <Rerender />
    {/if}
  </main>
  {#if store.toast}
    <div class="toast">{store.toast}</div>
  {/if}
</div>

<style>
  .shell {
    display: grid;
    grid-template-rows: 40px 1fr;
    height: 100%;
  }
  .titlebar {
    display: flex;
    align-items: center;
    gap: 14px;
    /* Room for the macOS traffic lights (overlay title bar). */
    padding: 0 12px 0 84px;
    background: var(--bg-2);
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }
  .brand {
    font-weight: 600;
    color: var(--accent);
    flex: none;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
  }
  .tab {
    background: transparent;
    border-color: transparent;
    padding: 4px 10px;
    color: var(--muted);
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .tab:hover:not(:disabled) {
    background: var(--bg-3);
    color: var(--text);
  }
  .tab.active {
    background: var(--bg-3);
    color: var(--text);
  }
  .tab:disabled {
    opacity: 0.35;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 1px;
    margin: 0 6px;
    padding: 0 4px;
    border-left: 1px solid var(--line);
    border-right: 1px solid var(--line);
  }
  .glabel {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    padding: 0 6px;
  }
  .group.active .glabel {
    color: var(--accent);
  }
  .step {
    padding: 4px 8px;
  }
  .num {
    display: inline-block;
    width: 16px;
    height: 16px;
    border-radius: 8px;
    background: var(--bg-3);
    color: var(--muted);
    font-size: 10px;
    text-align: center;
    line-height: 16px;
  }
  .step.active .num {
    background: var(--accent);
    color: #1a120a;
  }
  .drag {
    flex: 1;
    align-self: stretch;
  }
  .busy {
    color: var(--accent);
    font-size: 12px;
  }
  main {
    overflow: auto;
    padding: 18px 22px;
    min-height: 0;
  }
  main.flush {
    padding: 0;
    overflow: hidden;
  }
  .toast {
    position: fixed;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--bg-3);
    border: 1px solid var(--line);
    padding: 8px 14px;
    border-radius: 6px;
  }
</style>
