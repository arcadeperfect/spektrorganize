<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, human, type Templates } from "../api";
  import { store } from "../state.svelte";
  import Tree from "./Tree.svelte";
  import PathTemplate from "./PathTemplate.svelte";
  import Splitter from "./Splitter.svelte";
  import { TOKENS, copyPath } from "../template";
  import { SvelteSet } from "svelte/reactivity";
  import { paneWidth } from "../panes";

  const templateFields: { key: keyof Templates; label: string; hint: string }[] = [
    { key: "raw", label: "RAW", hint: "under archive root" },
    { key: "paired_image", label: "JPEG next to a RAW", hint: "under archive root" },
    { key: "image", label: "JPEG on its own", hint: "under archive root" },
    { key: "video", label: "Video (+ sidecars)", hint: "under video root" },
    { key: "other", label: "Everything else", hint: "under other root" },
    { key: "render_exr", label: "Rendered EXR", hint: "under render root" },
    { key: "render_jpeg", label: "Rendered JPEG", hint: "under render root" },
  ];
  /** Text mode for hand-editing a template instead of dragging chips. */
  let raw = $state(false);
  /** Which field's "copy to" popover is open, and what it will write. */
  let copyFrom = $state<keyof Templates | null>(null);
  let copyTo = new SvelteSet<keyof Templates>();
  let keepName = $state(true);

  function openCopy(key: keyof Templates) {
    copyFrom = copyFrom === key ? null : key;
    copyTo.clear();
  }

  function applyCopy() {
    if (!store.config || !copyFrom) return;
    const from = store.config.templates[copyFrom];
    for (const k of copyTo) store.config.templates[k] = copyPath(from, store.config.templates[k], keepName);
    copyFrom = null;
    copyTo.clear();
    changed();
  }

  async function resetTemplates() {
    if (!store.config) return;
    store.config.templates = await api.defaultTemplates();
    changed();
  }

  /** What this import is, kept on every photo it brings in. */
  let description = $state("");
  let descLoaded = false;
  $effect(() => {
    if (descLoaded) return;
    descLoaded = true;
    api.getImportDescription().then((d) => (description = d ?? ""));
  });

  const LEFT = 420;
  let leftW = $state(paneWidth("layout.left", LEFT));

  let errors = $state<Record<string, string>>({});
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function validateAll() {
    if (!store.config) return false;
    const errs: Record<string, string> = {};
    for (const f of templateFields) {
      try {
        await api.validateTemplate(store.config.templates[f.key]);
      } catch (e) {
        errs[f.key] = String(e);
      }
    }
    errors = errs;
    return Object.keys(errs).length === 0;
  }

  function changed() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(async () => {
      if (await validateAll()) {
        await store.saveConfig();
        await store.refreshPlan();
      }
    }, 350);
  }

  async function pickRoot(key: "archive_root" | "video_root" | "other_root" | "render_root") {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir === "string" && store.config) {
      store.config[key] = dir;
      changed();
    }
  }

  $effect(() => {
    if (store.scan && !store.plan && !store.planError) store.refreshPlan();
  });
</script>

{#if store.config}
  <div class="cols" style="grid-template-columns: {leftW}px auto minmax(0, 1fr)">
    <div class="left stack">
      <h1>Layout</h1>

      <div class="card stack">
        <h2>Roots</h2>
        {#each [["archive_root", "Archive"], ["video_root", "Video"], ["other_root", "Other"], ["render_root", "Renders"]] as [key, label] (key)}
          {@const k = key as "archive_root" | "video_root" | "other_root" | "render_root"}
          <div class="field">
            <label for={k}>{label}</label>
            <div class="row">
              <input
                id={k}
                type="text"
                class="mono"
                placeholder={k === "video_root" || k === "other_root" ? "same as archive" : ""}
                value={store.config[k] ?? ""}
                oninput={(e) => {
                  const v = (e.currentTarget as HTMLInputElement).value;
                  if (store.config) {
                    if (k === "video_root" || k === "other_root") store.config[k] = v || null;
                    else store.config[k] = v;
                  }
                  changed();
                }}
              />
              <button onclick={() => pickRoot(k)}>…</button>
            </div>
          </div>
        {/each}
      </div>

      <div class="card stack">
        <h2>This import</h2>
        <p class="muted small">
          A note about what these photos are — a shoot, a trip, a roll. It is written into the import's manifest and onto every photo it brings in,
          so it survives the card and the file names.
        </p>
        <input
          type="text"
          placeholder="e.g. Sam and Ana's wedding, second camera"
          value={description}
          oninput={(e) => {
            description = (e.currentTarget as HTMLInputElement).value;
            api.setImportDescription(description);
          }}
          spellcheck="true"
        />
      </div>

      <div class="card stack">
        <div class="row spread">
          <h2>Templates</h2>
          <div class="row">
            <button class="mini" onclick={resetTemplates} title="Put every path back to the stock template">Reset</button>
            <button class="mini" onclick={() => (raw = !raw)}>{raw ? "Chips" : "Edit as text"}</button>
          </div>
        </div>
        <div class="tpl">
          <!-- The token palette stands beside the paths: drag one into a field. -->
          <div class="tokens" class:hide={raw}>
            {#each TOKENS as t (t.name)}
              <span
                class="tok"
                draggable="true"
                role="button"
                tabindex="0"
                title={t.hint}
                ondragstart={(e) => {
                  e.dataTransfer?.setData("text/plain", `token:${t.name}:${t.args?.[0] && t.name !== "ext" ? t.args[0] : ""}`);
                  if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
                }}
              >
                {t.name}
              </span>
            {/each}
          </div>
          <div class="paths">
        {#each templateFields as f (f.key)}
          <div class="field">
            <div class="row spread lbl">
              <label for={f.key}>{f.label} <span class="muted">· {f.hint}</span></label>
              <div class="row">
                <button
                  class="mini copy"
                  onclick={() => {
                    store.config!.templates[f.key] = "";
                    changed();
                  }}
                  title="Empty this path and start again">clear</button
                >
                <button class="mini copy" onclick={() => openCopy(f.key)} title="Copy this path to other fields">copy to…</button>
              </div>
            </div>
            {#if copyFrom === f.key}
              <div class="card copybox">
                {#each templateFields.filter((o) => o.key !== f.key) as o (o.key)}
                  <label class="pick">
                    <input
                      type="checkbox"
                      checked={copyTo.has(o.key)}
                      onchange={() => (copyTo.has(o.key) ? copyTo.delete(o.key) : copyTo.add(o.key))}
                    />
                    {o.label}
                  </label>
                {/each}
                <label class="pick keep">
                  <input type="checkbox" bind:checked={keepName} />
                  keep each file name
                </label>
                <div class="row">
                  <button class="mini" disabled={copyTo.size === 0} onclick={applyCopy}>Copy to {copyTo.size || ""}</button>
                  <button
                    class="mini"
                    onclick={() => {
                      const others = templateFields.filter((o) => o.key !== f.key).map((o) => o.key);
                      if (copyTo.size === others.length) copyTo.clear();
                      else others.forEach((k) => copyTo.add(k));
                    }}>{copyTo.size === templateFields.length - 1 ? "None" : "All"}</button
                  >
                  <button class="mini" onclick={() => (copyFrom = null)}>Cancel</button>
                </div>
              </div>
            {/if}
            {#if raw}
              <input
                id={f.key}
                type="text"
                class="mono"
                class:err={!!errors[f.key]}
                bind:value={store.config.templates[f.key]}
                oninput={changed}
                spellcheck="false"
              />
            {:else}
              <PathTemplate
                value={store.config.templates[f.key]}
                invalid={!!errors[f.key]}
                onchange={(v) => {
                  store.config!.templates[f.key] = v;
                  changed();
                }}
              />
            {/if}
            {#if errors[f.key]}<div class="bad small">{errors[f.key]}</div>{/if}
          </div>
        {/each}
            <p class="muted small hint">Drag a token into a path, or drag a chip to move it. A chip with a ▾ takes an argument — click its name (date ▾ %Y is the year).</p>
          </div>
        </div>
      </div>
    </div>

    <Splitter bind:width={leftW} min={300} max={900} name="layout.left" reset={LEFT} />

    <div class="right">
      <div class="row spread" style="margin-bottom: 8px">
        <h1>Result</h1>
        <div class="row">
          {#if store.plan}
            <span class="muted">{store.plan.files} files · {human(store.plan.bytes_to_copy)} to copy · {store.plan.renders} renders</span>
            {#if store.plan.collisions}<span class="bad">{store.plan.collisions} collisions</span>{/if}
            {#if store.plan.existing}<span class="warn">{store.plan.existing} already exist</span>{/if}
          {/if}
          <button class="primary" disabled={!store.plan} onclick={() => (store.view = "commit")}>Commit →</button>
        </div>
      </div>
      {#if store.planError}
        <div class="card bad">{store.planError}</div>
      {:else if store.plan}
        <div class="card tree">
          {#each store.plan.tree as root (root.name)}
            <Tree node={root} />
          {/each}
        </div>
      {:else}
        <div class="muted">computing…</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .cols {
    display: grid;
    gap: 12px;
    align-items: stretch;
    /* Fill the view so the splitter has something to span. */
    min-height: calc(100vh - 120px);
  }
  .left {
    min-width: 0;
    overflow: auto;
  }
  .right {
    min-width: 0;
  }
  .field label {
    display: block;
    margin-bottom: 3px;
    font-size: 12px;
  }
  .field {
    margin-bottom: 6px;
  }
  input.err {
    border-color: var(--bad);
  }
  .small {
    font-size: 11px;
  }
  .tpl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 12px;
    align-items: start;
  }
  .tokens {
    display: flex;
    flex-direction: column;
    gap: 3px;
    position: sticky;
    top: 0;
    padding-right: 10px;
    border-right: 1px solid var(--line);
  }
  .tokens.hide {
    display: none;
  }
  .paths {
    min-width: 0;
  }
  .tok {
    background: var(--bg-3);
    border-radius: 4px;
    padding: 2px 7px;
    font-size: 11px;
    color: var(--muted);
    cursor: grab;
    text-align: left;
  }
  .tok:hover {
    color: var(--text);
    background: var(--line);
  }
  .tok:active {
    cursor: grabbing;
  }
  .hint {
    margin: 6px 0 0;
  }
  .lbl {
    align-items: baseline;
  }
  .copy {
    font-size: 10.5px;
    color: var(--muted);
  }
  .copy:hover {
    color: var(--text);
  }
  .copybox {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-bottom: 6px;
    padding: 8px;
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .pick.keep {
    margin-top: 4px;
    padding-top: 6px;
    border-top: 1px solid var(--line);
    color: var(--muted);
  }
  .tree {
    overflow: auto;
    max-height: calc(100vh - 140px);
  }
</style>
