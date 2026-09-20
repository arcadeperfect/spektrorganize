<script lang="ts">
  import { untrack } from "svelte";
  import { human, type TreeNode } from "../api";
  import Tree from "./Tree.svelte";

  let { node, depth = 0 }: { node: TreeNode; depth?: number } = $props();
  // Initial expansion only; the user toggles from there.
  let open = $state(untrack(() => depth < 3));

  function flag(n: TreeNode) {
    switch (n.status?.status) {
      case "exists_same_size":
        return { text: "exists, skip", cls: "muted" };
      case "exists_different":
        return { text: "exists, differs → suffixed", cls: "warn" };
      case "collision":
        return { text: "collision → suffixed", cls: "bad" };
      default:
        return null;
    }
  }
</script>

<div class="node" style="padding-left: {depth * 14}px">
  {#if node.is_dir}
    <button class="ghost dir" onclick={() => (open = !open)}>
      <span class="caret">{open ? "▾" : "▸"}</span>
      <!-- Token-derived folders take the colour of their depth; literal ones stay white. -->
      <span class="name" style={node.dynamic ? `color: var(--tok-${((depth - 1) % 6) + 1})` : ""}>{node.name}/</span>
      <span class="muted count">{node.files} · {human(node.bytes)}</span>
    </button>
  {:else}
    {@const f = flag(node)}
    <div class="file">
      <span class="name mono">{node.name}</span>
      {#if f}<span class={f.cls + " count"}>{f.text}</span>{/if}
    </div>
  {/if}
</div>
{#if node.is_dir && open}
  {#each node.children as c (c.name)}
    <Tree node={c} depth={depth + 1} />
  {/each}
{/if}

<style>
  .node {
    white-space: nowrap;
  }
  .dir {
    padding: 2px 4px;
    display: inline-flex;
    gap: 6px;
    align-items: center;
    color: var(--text);
  }
  .caret {
    width: 10px;
    color: var(--muted);
  }
  .file {
    padding: 2px 4px 2px 20px;
    display: inline-flex;
    gap: 10px;
  }
  .count {
    font-size: 11px;
  }
</style>
