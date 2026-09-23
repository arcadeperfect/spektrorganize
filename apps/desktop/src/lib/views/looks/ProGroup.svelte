<script lang="ts">
  // One level of the parameter model, discovered rather than listed: a
  // control per field, a nested group per object. This is how the Pro panel
  // shows everything the pipeline reads without anyone maintaining a list.
  import Self from "./ProGroup.svelte";
  import Control from "./Control.svelte";
  import Rgb from "./Rgb.svelte";
  import { HIDDEN, humanize, rangeFor } from "./ranges";

  let { obj, stock, path = "", depth = 0 }: { obj: Record<string, any>; stock: Record<string, any> | null; path?: string; depth?: number } = $props();

  const entries = $derived(
    Object.entries(obj)
      .filter(([k]) => !HIDDEN.has(path ? `${path}.${k}` : k))
      // Plain values first, then the nested groups, each alphabetical.
      .sort(([a, va], [b, vb]) => Number(isGroup(vb)) - Number(isGroup(va)) || a.localeCompare(b)),
  );

  function isGroup(v: unknown): boolean {
    return typeof v === "object" && v !== null && !Array.isArray(v);
  }
  function isNumbers(v: unknown): v is number[] {
    return Array.isArray(v) && v.length >= 2 && v.length <= 4 && v.every((x) => typeof x === "number");
  }
  const at = (k: string) => (path ? `${path}.${k}` : k);
</script>

{#each entries as [key, value] (key)}
  {@const p = at(key)}
  {#if isGroup(value)}
    <details open={depth === 0}>
      <summary>{humanize(key)}</summary>
      <Self obj={value} stock={stock?.[key] ?? null} path={p} depth={depth + 1} />
    </details>
  {:else if typeof value === "boolean"}
    <Control label={humanize(key)} path={p} kind="check" />
  {:else if typeof value === "number"}
    {@const r = rangeFor(p, stock?.[key])}
    {#if Number.isInteger(value) && Number.isInteger(stock?.[key] ?? value) && /^(n_|.*_(count|bounces|layers|resolution)$)/.test(key)}
      <Control label={humanize(key)} path={p} kind="int" />
    {:else}
      <Control label={humanize(key)} path={p} min={r.min} max={r.max} step={r.step} digits={r.step < 0.01 ? 3 : 2} />
    {/if}
  {:else if isNumbers(value)}
    {@const r = rangeFor(p, Math.max(...((stock?.[key] as number[] | undefined) ?? value)))}
    <Rgb label={humanize(key)} path={p} min={r.min} max={r.max} step={r.step} digits={r.step < 0.01 ? 3 : 2} />
  {:else if typeof value === "string"}
    <Control label={humanize(key)} path={p} kind="text" />
  {/if}
{/each}

<style>
  details {
    margin: 4px 0;
    padding-left: 6px;
    border-left: 1px solid var(--line);
  }
  summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 4px;
  }
</style>
