<script lang="ts">
  // One path template as chips: drag tokens in from the palette, drag chips to
  // reorder, type the separators in between. The value stays a plain template
  // string, so nothing downstream changes.
  import { untrack } from "svelte";
  import { parse, serialize, normalise, insertToken, removeToken, TOKENS, type Part } from "../template";

  let {
    value,
    onchange,
    invalid = false,
  }: { value: string; onchange: (next: string) => void; invalid?: boolean } = $props();

  let parts = $state<Part[]>(untrack(() => parse(value)));
  let dragOver = $state<number | null>(null);
  let editingArg = $state<number | null>(null);
  let last = untrack(() => value);

  // Follow the value when it changes elsewhere (loading config, presets).
  $effect(() => {
    if (value !== last) {
      last = value;
      parts = parse(value);
    }
  });

  function commit() {
    parts = normalise(parts);
    last = serialize(parts);
    onchange(last);
  }

  function argsFor(name: string): string[] {
    return TOKENS.find((t) => t.name === name)?.args ?? [];
  }

  function onDrop(e: DragEvent, at: number) {
    e.preventDefault();
    e.stopPropagation();
    dragOver = null;
    const data = e.dataTransfer?.getData("text/plain") ?? "";
    if (data.startsWith("token:")) {
      const [, name, arg] = data.split(":");
      parts = insertToken(parts, at, { kind: "token", name, arg: arg || undefined });
    } else if (data.startsWith("move:")) {
      const from = Number(data.slice(5));
      const moved = parts[from];
      if (!moved || moved.kind !== "token") return;
      parts = removeToken(parts, from);
      parts = insertToken(parts, from < at ? at - 1 : at, moved);
    } else {
      return;
    }
    commit();
  }

  function remove(i: number) {
    parts = removeToken(parts, i);
    commit();
  }
</script>

<!-- The whole field takes a drop (appending at the end); the gaps and chips take
     one at their own position. -->
<div
  class="tpl"
  class:err={invalid}
  class:over={dragOver === -1}
  role="presentation"
  ondragover={(e) => {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    dragOver = -1;
  }}
  ondragleave={() => (dragOver === -1 ? (dragOver = null) : null)}
  ondrop={(e) => onDrop(e, parts.length)}
>
  {#each parts as p, i (i)}
    {#if p.kind === "text"}
      <span
        class="gap"
        class:over={dragOver === i}
        role="presentation"
        ondragover={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
          dragOver = i;
        }}
        ondragleave={() => (dragOver === i ? (dragOver = null) : null)}
        ondrop={(e) => onDrop(e, i)}
      >
        <input
          class="lit mono"
          size={Math.max(1, p.value.length)}
          value={p.value}
          placeholder=""
          spellcheck="false"
          oninput={(e) => {
            parts[i] = { kind: "text", value: (e.target as HTMLInputElement).value };
            commit();
          }}
        />
      </span>
    {:else}
      <span
        class="chip"
        class:over={dragOver === i}
        draggable="true"
        role="button"
        tabindex="0"
        ondragstart={(e) => {
          e.dataTransfer?.setData("text/plain", `move:${i}`);
          if (e.dataTransfer) e.dataTransfer.effectAllowed = "copyMove";
        }}
        ondragover={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
          dragOver = i;
        }}
        ondragleave={() => (dragOver === i ? (dragOver = null) : null)}
        ondrop={(e) => onDrop(e, i)}
        title="{`{${p.arg ? `${p.name}:${p.arg}` : p.name}}`} — {TOKENS.find((t) => t.name === p.name)?.hint ?? p.name}"
      >
        <!-- Only tokens that take an argument get the caret and the menu. -->
        <span
          class="nm"
          class:menu={argsFor(p.name).length > 0}
          onclick={() => argsFor(p.name).length > 0 && (editingArg = editingArg === i ? null : i)}
          onkeydown={() => {}}
          role="presentation">{p.name}{#if argsFor(p.name).length}<span class="caret">▾</span>{/if}</span
        >
        {#if p.arg}<span class="arg mono">{p.arg}</span>{/if}
        <button class="x" title="Remove" onclick={() => remove(i)}>×</button>
        {#if editingArg === i}
          <span class="args card">
            {#each argsFor(p.name) as a (a)}
              <button
                class="mini"
                onclick={() => {
                  parts[i] = { kind: "token", name: p.name, arg: a };
                  editingArg = null;
                  commit();
                }}>{a}</button
              >
            {/each}
            <input
              class="mono"
              placeholder="custom"
              value={p.arg ?? ""}
              onchange={(e) => {
                const v = (e.target as HTMLInputElement).value;
                parts[i] = { kind: "token", name: p.name, arg: v || undefined };
                editingArg = null;
                commit();
              }}
            />
            {#if p.arg}
              <button
                class="mini"
                onclick={() => {
                  parts[i] = { kind: "token", name: p.name };
                  editingArg = null;
                  commit();
                }}>plain</button
              >
            {/if}
          </span>
        {/if}
      </span>
    {/if}
  {/each}
</div>

<style>
  .tpl {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 1px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 6px;
    min-height: 32px;
  }
  .tpl.err {
    border-color: var(--bad);
  }
  .tpl.over {
    border-color: var(--accent);
  }
  .chip.over {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .gap {
    display: inline-flex;
    border-radius: 4px;
  }
  .gap.over {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .lit {
    border: none;
    background: transparent;
    padding: 2px 1px;
    width: auto;
    min-width: 12px;
    color: var(--muted);
  }
  .lit:focus {
    outline: none;
    color: var(--text);
  }
  .chip {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    background: var(--bg-3);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 1px 4px 1px 8px;
    font-size: 12px;
    cursor: grab;
  }
  .chip:active {
    cursor: grabbing;
  }
  .nm {
    color: var(--accent-2);
  }
  .nm.menu {
    cursor: pointer;
  }
  .caret {
    font-size: 8px;
    margin-left: 2px;
    color: var(--muted);
    vertical-align: middle;
  }
  .nm.menu:hover .caret {
    color: var(--accent-2);
  }
  .arg {
    color: var(--muted);
    font-size: 11px;
  }
  .x {
    border: none;
    background: transparent;
    color: var(--muted);
    padding: 0 2px;
    line-height: 1;
  }
  .x:hover {
    color: var(--text);
    background: transparent;
  }
  .args {
    position: absolute;
    top: 110%;
    left: 0;
    z-index: 5;
    display: flex;
    gap: 4px;
    padding: 5px;
    white-space: nowrap;
  }
  .args input {
    width: 90px;
  }
</style>
