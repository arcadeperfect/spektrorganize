<script lang="ts">
  // Three sliders on one [r, g, b] parameter. The array is written whole: the
  // path helpers make objects on the way down, and a triple must stay a triple.
  import { lookEditor as L } from "../../looks.svelte";

  let { label, path, min = 0, max = 1, step = 0.01, digits = 3 }: { label: string; path: string; min?: number; max?: number; step?: number; digits?: number } =
    $props();

  const value = $derived((L.value(path) as number[] | undefined) ?? [0, 0, 0]);
  const overridden = $derived(L.isOverridden(path));

  function setIdx(i: number, v: number) {
    const next = [...value];
    next[i] = v;
    L.set(path, next);
  }
</script>

<div class="ctl">
  <div class="head">
    <button class="dot" class:on={overridden} disabled={!overridden} onclick={() => L.reset(path)} aria-label="Reset" title={overridden ? "Changed by this look — click to use the stock value" : "Stock value"}></button>
    <span class="label">{label}</span>
  </div>
  {#each ["R", "G", "B"] as ch, i (ch)}
    <div class="row">
      <span class="ch muted">{ch}</span>
      <input type="range" {min} {max} {step} value={value[i]} oninput={(e) => setIdx(i, Number((e.target as HTMLInputElement).value))} />
      <span class="mono small">{(value[i] ?? 0).toFixed(digits)}</span>
    </div>
  {/each}
</div>

<style>
  .ctl {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .label {
    flex: 1;
    font-size: 12px;
  }
  .ch {
    width: 10px;
    font-size: 11px;
  }
  .row input[type="range"] {
    flex: 1;
    min-width: 0;
  }
  .dot {
    width: 8px;
    height: 8px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--line);
    background: transparent;
  }
  .dot.on {
    background: var(--accent-2);
    border-color: var(--accent-2);
  }
  .small {
    font-size: 11px;
    width: 44px;
    text-align: right;
  }
</style>
