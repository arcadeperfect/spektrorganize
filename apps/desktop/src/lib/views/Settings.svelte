<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, catalog, looks, type CatalogInfo, type Info, type PresetSummary } from "../api";
  import { store } from "../state.svelte";
  import { theme, PRESETS } from "../theme.svelte";

  let presets = $state<PresetSummary[]>([]);
  let info = $state<Info | null>(null);
  let saved = $state(false);
  let catInfo = $state<CatalogInfo | null>(null);
  let fromPrints = $state(false);
  let hasKey = $state(false);
  let keyInput = $state("");
  let keyMsg = $state<string | null>(null);

  let colorSpaces = $state<string[]>(["ITU-R BT.2020", "ACES2065-1", "sRGB"]);
  onMount(async () => {
    looks.meta().then((m) => (colorSpaces = m.color_spaces)).catch(() => {});
    presets = await api.listPresets();
    info = await api.info();
    catInfo = await catalog.info().catch(() => null);
    fromPrints = await catalog.thumbsFromPrints().catch(() => false);
    hasKey = !!(await catalog.aiKey().catch(() => null));
  });

  async function saveKey(clear = false) {
    try {
      hasKey = await catalog.aiKeySave(clear ? "" : keyInput);
      keyInput = "";
      keyMsg = hasKey ? "saved" : "cleared";
      // An environment key still counts even when the file is cleared.
      hasKey = !!(await catalog.aiKey().catch(() => null));
    } catch (e) {
      keyMsg = String(e);
    }
  }

  async function save() {
    await store.saveConfig();
    saved = true;
    setTimeout(() => (saved = false), 1500);
  }

  async function pickPreset() {
    const f = await open({ multiple: false, filters: [{ name: "Preset", extensions: ["json"] }] });
    if (typeof f === "string" && store.config) {
      store.config.preset = f;
      save();
    }
  }
</script>

{#if store.config}
  <div class="row spread">
    <h1>Settings</h1>
    <div class="row">
      {#if saved}<span class="ok">saved</span>{/if}
      <button class="primary" onclick={save}>Save</button>
    </div>
  </div>

  <div class="grid">
    <div class="card stack">
      <h2>Colours</h2>
      <div class="row wrap">
        {#each PRESETS as p (p.name)}
          <button
            class="swatch"
            class:on={theme.current.accent === p.theme.accent}
            style="--sw: {p.theme.accent}"
            onclick={() => theme.set(p.theme)}
            title={p.name}
          >
            <span class="dot"></span>{p.name}
          </button>
        {/each}
      </div>
      <div class="row">
        <span class="w">Accent</span>
        <input type="color" value={theme.current.accent} oninput={(e) => theme.set({ accent: (e.target as HTMLInputElement).value })} />
        <span class="w">Highlight</span>
        <input type="color" value={theme.current.accent2} oninput={(e) => theme.set({ accent2: (e.target as HTMLInputElement).value })} />
        <span class="w">Background</span>
        <input type="color" value={theme.current.bg} oninput={(e) => theme.set({ bg: (e.target as HTMLInputElement).value })} />
        <button class="mini" onclick={() => theme.reset()}>Reset</button>
      </div>
      <div class="muted small">Colours are stored on this machine; panels and lines follow the background.</div>

      <h2>Film preset</h2>
      <select bind:value={store.config.preset} onchange={save}>
        <option value="__developed__">No film look — developed only</option>
        {#each presets as p (p.path)}
          <option value={p.path}>{p.name} — {p.film}{p.print ? " → " + p.print : ""}</option>
        {/each}
        {#if !presets.some((p) => p.path === store.config?.preset)}
          <option value={store.config.preset}>{store.config.preset}</option>
        {/if}
      </select>
      <div class="row">
        <button onclick={pickPreset}>Choose file…</button>
        <span class="muted">Looks are JSON: film, paper, and only the spektrafilm parameters they change. Edit and import them in the Looks tab; yours live in ~/.config/spektrorganize/presets.</span>
      </div>
    </div>

    <div class="card stack">
      <h2>Outputs</h2>
      <label class="row"><input type="checkbox" bind:checked={store.config.outputs.jpeg} onchange={save} /> JPEG (sRGB, viewable)</label>
      <div class="row">
        <span class="w">Quality</span>
        <input type="number" min="60" max="100" bind:value={store.config.outputs.jpeg_quality} onchange={save} style="width: 80px" />
      </div>
      <label class="row"><input type="checkbox" bind:checked={store.config.outputs.exr} onchange={save} /> EXR (half float, linear, for grading)</label>
      <div class="row">
        <span class="w">EXR colour space</span>
        <select bind:value={store.config.outputs.exr_color_space} onchange={save} style="width: 200px">
          {#each colorSpaces as cs (cs)}
            <option value={cs}>{cs} (linear)</option>
          {/each}
          {#if !colorSpaces.includes(store.config.outputs.exr_color_space)}
            <option value={store.config.outputs.exr_color_space}>{store.config.outputs.exr_color_space}</option>
          {/if}
        </select>
      </div>
      <div class="muted small">The film stage clamps to 0–1: the EXR is a print simulation, not scene-referred.</div>
    </div>

    <div class="card stack">
      <h2>Copy</h2>
      <div class="row">
        <span class="w">Verify</span>
        <select bind:value={store.config.verify} onchange={save} style="width: 200px">
          <option value="hash">Re-read and compare hash</option>
          <option value="size_only">Size only</option>
        </select>
      </div>
      <label class="row"><input type="checkbox" bind:checked={store.config.skip_existing} onchange={save} /> Skip files already at the destination with the same size</label>
    </div>

    <div class="card stack">
      <h2>Render</h2>
      <div class="row">
        <span class="w">Parallel decodes</span>
        <input type="number" min="1" max="8" bind:value={store.config.decode_threads} onchange={save} style="width: 80px" />
      </div>
      <div class="muted small">Each decode is single-threaded LibRaw; the film stage runs on the GPU one image at a time.</div>
    </div>

    <div class="card stack">
      <h2>AI labelling</h2>
      <div class="muted small">
        ✦ Label in the Library sends each photo's 256 px thumbnail, file name, camera, date and folder to Claude and stores the keywords it returns as ✦ AI keywords. It only
        runs when you press Label.
      </div>
      <div class="row">
        <span class="w">API key</span>
        {#if hasKey}<span class="ok">configured</span>{:else}<span class="warn">not set</span>{/if}
        {#if keyMsg}<span class="muted small">{keyMsg}</span>{/if}
      </div>
      <form
        class="row"
        onsubmit={(e) => {
          e.preventDefault();
          saveKey();
        }}
      >
        <input type="password" class="mono" bind:value={keyInput} placeholder="sk-ant-…" autocomplete="off" spellcheck="false" />
        <button type="submit" disabled={!keyInput.trim()}>Save</button>
        {#if hasKey}<button type="button" class="ghost" onclick={() => saveKey(true)}>Clear</button>{/if}
      </form>
      <div class="muted small">Stored owner-only in the app's data folder. The ANTHROPIC_API_KEY environment variable wins over it.</div>
    </div>

    {#if catInfo}
      <div class="card stack">
        <h2>Catalog</h2>
        <div class="mono small">database: {catInfo.path} (schema v{catInfo.schema})</div>
        <div class="mono small">thumbnails: {catInfo.thumbs_dir}</div>
        <div class="small">{catInfo.assets} photos indexed</div>
        <label class="row small">
          <input
            type="checkbox"
            checked={fromPrints}
            onchange={async (e) => {
              fromPrints = (e.currentTarget as HTMLInputElement).checked;
              await catalog.setThumbsFromPrints(fromPrints);
            }}
          />
          Show prints in the library — a photo's thumbnail comes from its newest print instead of the camera rendition. Affected thumbnails are made again as you scroll.
        </label>
      </div>
    {/if}

    {#if info}
      <div class="card stack">
        <h2>About</h2>
        <div class="mono small">config: {info.config_path}</div>
        <div class="mono small">cache: {info.cache_dir}</div>
        <div class="mono small">spektrafilm data: {info.data_dir ?? "not found"}</div>
        <div class="mono small">LibRaw {info.libraw_version} · spektrafilm-rs {info.spektrafilm_rev}</div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .w {
    width: 130px;
    color: var(--muted);
  }
  .small {
    font-size: 11px;
  }
  .swatch {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .swatch.on {
    border-color: var(--accent);
  }
  .swatch .dot {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--sw);
  }
  .wrap {
    flex-wrap: wrap;
  }
  input[type="color"] {
    width: 42px;
    height: 26px;
    padding: 0;
    background: transparent;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
</style>
