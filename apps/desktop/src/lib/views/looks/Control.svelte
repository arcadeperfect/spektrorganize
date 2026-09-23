<script lang="ts">
  // One look parameter: a slider, checkbox or menu bound to a params path.
  // Shows whether the look overrides the stock value; the dot resets it.
  import { lookEditor as L } from "../../looks.svelte";

  let {
    label,
    path,
    kind = "slider",
    min = 0,
    max = 1,
    step = 0.01,
    options = [],
    hint = "",
    digits = 2,
  }: {
    label: string;
    path: string;
    kind?: "slider" | "check" | "select" | "text" | "int";
    min?: number;
    max?: number;
    step?: number;
    options?: { value: string; label: string }[];
    hint?: string;
    digits?: number;
  } = $props();

  const value = $derived(L.value(path));
  const overridden = $derived(L.isOverridden(path));

  function onSlider(e: Event) {
    L.set(path, Number((e.target as HTMLInputElement).value));
  }
  function onNumber(e: Event) {
    const v = Number((e.target as HTMLInputElement).value);
    if (Number.isFinite(v)) L.set(path, v);
  }
</script>

<div class="ctl" title={hint}>
  <div class="head">
    <button class="dot" class:on={overridden} disabled={!overridden} onclick={() => L.reset(path)} title={overridden ? "Changed by this look — click to use the stock value" : "Stock value"}
      aria-label="Reset"></button>
    <span class="label">{label}</span>
    {#if kind === "slider"}
      <input class="num mono" type="number" {step} value={typeof value === "number" ? Number(value.toFixed(digits)) : ""} onchange={onNumber} />
    {/if}
  </div>
  {#if kind === "slider"}
    <input type="range" {min} {max} {step} value={value ?? min} oninput={onSlider} ondblclick={() => L.reset(path)} />
  {:else if kind === "text"}
    <input class="mono" type="text" value={value ?? ""} spellcheck="false" onchange={(e) => L.set(path, (e.target as HTMLInputElement).value)} />
  {:else if kind === "int"}
    <input class="num mono" type="number" step="1" value={value ?? 0} onchange={(e) => {
      const v = Math.round(Number((e.target as HTMLInputElement).value));
      if (Number.isFinite(v)) L.set(path, v);
    }} />
  {:else if kind === "check"}
    <label class="row"><input type="checkbox" checked={!!value} onchange={(e) => L.set(path, (e.target as HTMLInputElement).checked)} /> <span class="muted">on</span></label>
  {:else}
    <select
      value={value == null ? "" : String(value)}
      onchange={(e) => {
        const v = (e.target as HTMLSelectElement).value;
        // A numeric parameter offered as a menu (film format) stays a number.
        L.set(path, typeof value === "number" ? Number(v) : v);
      }}
    >
      {#each options as o (o.value)}
        <option value={o.value}>{o.label}</option>
      {/each}
    </select>
  {/if}
</div>

<style>
  .ctl {
    display: flex;
    flex-direction: column;
    gap: 3px;
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
  .dot {
    width: 8px;
    height: 8px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--line);
    background: transparent;
  }
  .dot.on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .dot:disabled {
    opacity: 1;
  }
  .num {
    width: 72px;
    padding: 2px 6px;
    text-align: right;
  }
  input[type="range"] {
    width: 100%;
    accent-color: var(--accent);
  }
</style>
