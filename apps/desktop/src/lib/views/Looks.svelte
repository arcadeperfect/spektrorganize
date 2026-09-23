<script lang="ts">
  // Look editor: pick or import a look, edit it against a live preview of a
  // catalog photo (with that photo's RAW settings), save it, print with it.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, catalog, duration as durationText, looks as lookApi } from "../api";
  import { lookEditor as L, getPath, ROUTE_PRINT, ROUTE_SCAN } from "../looks.svelte";
  import { develop as D } from "../photo.svelte";
  import { library as lib } from "../library.svelte";
  import { store } from "../state.svelte";
  import Control from "./looks/Control.svelte";
  import Rgb from "./looks/Rgb.svelte";
  import ProGroup from "./looks/ProGroup.svelte";
  /** Flow is the curated panel; Pro is every parameter the pipeline reads, discovered from the model. */
  let pro = $state(false);

  /** Frame sizes, as the grain scale needs them (long edge in mm). */
  const FORMATS = [
    { value: "24", label: "Half frame (24 mm)" },
    { value: "35", label: "Standard 35 (36 mm)" },
    { value: "56", label: "Medium 6×6 (56 mm)" },
    { value: "70", label: "Medium 6×7 (70 mm)" },
    { value: "120", label: "Large 4×5 (120 mm)" },
  ];
  /** Diffusion filter families this build knows. */
  const MIST = [{ value: "black_pro_mist", label: "Black Pro-Mist" }];
  import JobPanel from "./JobPanel.svelte";
  import Viewport from "./Viewport.svelte";

  let importing = $state(false);
  let url = $state("");
  let pasted = $state("");
  let paramsText = $state("");
  let paramsError = $state<string | null>(null);

  let zoom = $state(1);
  /** Screen pixels per picture pixel right now: 1 is true 1:1. */
  let printPx = $state(0);

  const candidates = $derived.by(() => {
    const ids = lib.selected.size ? [...lib.selected] : lib.focus !== null ? [lib.focus] : [];
    return ids.slice(0, 40);
  });

  onMount(async () => {
    await L.init();
    const first = D.id ?? lib.focus ?? candidates[0] ?? null;
    if (first !== null) await D.setPhoto(first);
    const missing = candidates.filter((id) => !lib.thumbs.has(id));
    if (missing.length) {
      for (const t of await catalog.requestThumbs(missing).catch(() => [])) lib.thumbs.set(t.id, t.path);
    }
  });

  $effect(() => {
    paramsText = JSON.stringify(L.look.params, null, 2);
  });

  // The print preview always renders the develop stage's current photo and settings.
  $effect(() => {
    D.version;
    L.syncPhoto();
  });

  const film = $derived(L.meta?.films.find((f) => f.name === L.look.film));
  const route = $derived(L.value("workflow.route") ?? ROUTE_PRINT);
  const scanning = $derived(route !== ROUTE_PRINT && !route.includes("print"));
  const filters = $derived(L.meta?.color_filters ?? []);
  const opt = (xs: string[] | undefined) => (xs ?? []).map((v) => ({ value: v, label: v }));

  async function importFile() {
    const path = await open({
      multiple: false,
      title: "Import a spektrafilm preset",
      filters: [{ name: "Presets", extensions: ["json", "dtpreset", "dtstyle", "pst", "cfg", "zip", "spkpreset"] }],
    });
    if (typeof path !== "string") return;
    await runImport(() => lookApi.importFile(path));
  }

  async function runImport(fn: () => Promise<{ preset: any; warnings: string[] }>) {
    try {
      const r = await fn();
      L.adopt(r.preset, r.warnings);
      importing = false;
      url = "";
      pasted = "";
    } catch (e) {
      L.error = String(e);
    }
  }

  async function print() {
    let path = L.path;
    if (!path || L.dirty) path = await L.save(!L.path);
    if (!path) return;
    const ids = lib.selected.size ? [...lib.selected] : D.id !== null ? [D.id] : [];
    if (!ids.length) {
      store.say("Select photos in the Library first.");
      return;
    }
    store.startPrint({ ids, preset: path, jpeg: store.config?.outputs.jpeg ?? true, exr: store.config?.outputs.exr ?? false });
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
    if ((e.key === "b" || e.key === "B") && !e.metaKey && !e.ctrlKey && !e.altKey && D.id !== null) {
      e.preventDefault();
      L.toggleBefore();
    }
  }

  function grainValue(): number {
    return getPath(L.effective, "film_render.grain.rms_granularity")?.[1] ?? 0;
  }
  function setGrain(v: number) {
    const stock: number[] = getPath(L.stock, "film_render.grain.rms_granularity") ?? [v, v, v];
    const g = stock[1] || 1;
    L.set("film_render.grain.rms_granularity", stock.map((s) => Math.round(((s * v) / g) * 100) / 100));
  }
  function usm(): [number, number] {
    return getPath(L.effective, "scanner.unsharp_mask") ?? [0.7, 0.7];
  }

  /**
   * ← and → walk the library's current listing; ↑ and ↓ walk the presets, so
   * a photo can be tried against every look without touching the mouse. A
   * field, slider or menu with focus keeps the keys for itself.
   */
  async function stepKey(e: KeyboardEvent) {
    const horizontal = e.key === "ArrowLeft" || e.key === "ArrowRight";
    const vertical = e.key === "ArrowUp" || e.key === "ArrowDown";
    if (!horizontal && !vertical) return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (vertical) {
      const list = L.list;
      if (!list.length) return;
      e.preventDefault();
      const i = list.findIndex((p) => p.path === L.path);
      // From no preset, ↓ starts at the top and ↑ at the bottom; otherwise no wrapping.
      const to = i < 0 ? (e.key === "ArrowDown" ? 0 : list.length - 1) : i + (e.key === "ArrowDown" ? 1 : -1);
      if (to < 0 || to >= list.length) return;
      await L.open(list[to].path);
      return;
    }
    if (D.id === null) return;
    e.preventDefault();
    const next = await lib.step(D.id, e.key === "ArrowRight" ? 1 : -1);
    if (next) await D.setPhoto(next.id);
  }
</script>

<svelte:window onkeydown={(e) => { onKey(e); stepKey(e); }} />

<div class="looks">
  <!-- look list -->
  <aside class="list">
    <div class="row spread">
      <h2>Looks</h2>
      <div class="row">
        <button class="ghost" onclick={() => L.newLook()} title="New look from the current film">＋</button>
        <button class="ghost" onclick={() => (importing = !importing)} title="Import a preset">Import</button>
      </div>
    </div>
    {#if importing}
      <div class="card stack imp">
        <button onclick={importFile}>From a file…</button>
        <div class="muted small">Python GUI states, darktable, vkdt, or a zip holding one.</div>
        <input type="text" placeholder="https://… (forum attachment)" bind:value={url} />
        <button disabled={!url} onclick={() => runImport(() => lookApi.importUrl(url))}>From URL</button>
        <textarea placeholder="…or paste preset text" bind:value={pasted} rows="3"></textarea>
        <button disabled={!pasted} onclick={() => runImport(() => lookApi.importText(pasted))}>From text</button>
      </div>
    {/if}
    <ul>
      {#each L.list as p (p.path)}
        <li>
          <button class="item" class:active={p.path === L.path} onclick={() => L.open(p.path)}>
            <span>{p.name}</span>
            <span class="muted small">{p.film.replaceAll("_", " ")}</span>
          </button>
        </li>
      {/each}
    </ul>
  </aside>

  <!-- preview -->
  <section class="center">
    <div class="bar row spread">
      <div class="row">
        <input class="name" type="text" value={L.look.name} oninput={(e) => L.setName((e.target as HTMLInputElement).value)} />
        {#if L.dirty}<span class="warn small">unsaved</span>{/if}
      </div>
      <div class="row">
        {#if L.rendering}<span class="muted small">rendering…</span>{:else if L.renderMs}<span class="muted small">{L.renderMs} ms</span>{/if}
        <button class="toggle" class:on={L.showBefore} disabled={D.id === null} onclick={() => L.toggleBefore()} title="Show the photo without the film look (key: B)">
          {L.showBefore ? "Before" : "After"}
        </button>
        <button onclick={() => L.save(false)} disabled={!L.dirty && !!L.path}>Save</button>
        <button onclick={() => L.save(true)}>Save as new</button>
        {#if L.path?.includes("/.config/spektrorganize/presets/")}
          <button class="ghost" onclick={() => L.remove()} title="Move this look to the Trash">Delete</button>
        {/if}
        <button class="primary" disabled={store.busy} onclick={print}>
          Print {lib.selected.size ? `${lib.selected.size} selected` : "this photo"}
        </button>
      </div>
    </div>
    <div class="stage pic">
      {#if D.id === null}
        <div class="muted empty">Select photos in the Library; the first one previews here.</div>
      {:else if L.preview || L.before || L.rendering}
        <Viewport
          frame={L.showBefore && L.before ? L.before : L.preview}
          key={`${D.id}:${D.raw.rotate}:${D.raw.straighten}:${D.raw.crop ? `${D.raw.crop.x},${D.raw.crop.y},${D.raw.crop.w},${D.raw.crop.h}` : ""}`}
          onzoom={(z, px) => {
            zoom = z;
            printPx = px;
            L.setZoom(z);
          }}
        />
        {#if L.showBefore && L.before}
          <div class="tag">Before</div>
        {:else if L.showBefore}
          <div class="tag">After (loading before…)</div>
        {/if}
        {#if L.error}<div class="bad err">{L.error}</div>{/if}
        {#if !L.preview && !L.before}<div class="muted empty">Decoding…</div>{/if}
      {:else if L.error}
        <div class="bad empty">Can't preview this {D.isVideo ? "clip" : "photo"}: {L.error}</div>
      {:else}
        <div class="muted empty">Decoding…</div>
      {/if}
      {#if L.busy}<div class="muted busy">{L.busy}</div>{/if}
      <div class="tag zoom right">
        {#if zoom !== 1}<span title="Of the picture's own pixels; 100% is one per screen pixel">{Math.round((printPx || zoom) * 100)}%</span>{/if}
        <button class="full" class:on={D.full} onclick={() => L.setFull(!D.full)} title="Render every pixel ({D.native ? `${D.native} px` : 'full size'}) instead of a fitted preview — slower">
          {D.full ? "full res" : `${D.detail} px`}
        </button>
      </div>
    </div>
      {#if D.isVideo && D.input?.duration}
        <div class="picker row">
          <span class="muted small">frame</span>
          <input
            type="range"
            min="0"
            max={D.input.duration}
            step="0.04"
            value={D.frameAt ?? Math.min(D.input.duration * 0.1, 2)}
            oninput={(e) => D.setFrame(Number((e.currentTarget as HTMLInputElement).value), () => L.refresh())}
          />
          <span class="muted small mono">{(D.frameAt ?? Math.min(D.input.duration * 0.1, 2)).toFixed(2)}s</span>
          <span class="muted small">of {durationText(D.input.duration)} — the look previews on one frame; Print renders them all</span>
        </div>
      {/if}
    {#if candidates.length > 1}
      <div class="strip">
        {#each candidates as id (id)}
          {@const t = lib.thumbs.get(id)}
          <button class="thumb" class:active={id === D.id} onclick={() => D.setPhoto(id)}>
            {#if t}<img src={api.fileUrl(t)} alt="" />{:else}<span class="muted small">#{id}</span>{/if}
          </button>
        {/each}
      </div>
    {/if}
    {#if L.warnings.length}
      <details class="card warnings">
        <summary class="warn">Imported with {L.warnings.length} note{L.warnings.length === 1 ? "" : "s"}</summary>
        <ul>{#each L.warnings as w}<li class="small">{w}</li>{/each}</ul>
      </details>
    {/if}
    <JobPanel />
  </section>

  <!-- controls -->
  <aside class="controls">
    {#if L.meta && L.effective}
      <div class="from-develop row spread">
        <span class="small">
          Developed: {D.isVideo ? "video frame" : D.isRaw ? D.raw.white_balance.replace("_", " ") : "camera JPEG"}{D.raw.exposure_ev !== 0 ? `, ${D.raw.exposure_ev > 0 ? "+" : ""}${D.raw.exposure_ev.toFixed(2)} EV` : ""}
        </span>
        <button class="ghost small" onclick={() => (store.view = "develop")}>Develop…</button>
      </div>
    {/if}
    {#if L.meta && L.effective}
      <details open>
        <summary>Film and paper</summary>
        <label class="stack tight"><span class="small">Film</span>
          <select value={L.look.film} onchange={(e) => L.setFilm((e.target as HTMLSelectElement).value)}>
            {#each L.meta.films as f (f.name)}<option value={f.name}>{f.label}{f.positive ? " (slide)" : ""}{f.bw ? " (B&W)" : ""}</option>{/each}
          </select>
        </label>
        <label class="stack tight"><span class="small">Process</span>
          <select value={route} onchange={(e) => L.set("workflow.route", (e.target as HTMLSelectElement).value)}>
            <option value={ROUTE_PRINT}>Print onto paper</option>
            <option value={ROUTE_SCAN}>Scan the film</option>
            {#each L.meta.routes.filter((r) => r !== ROUTE_PRINT && r !== ROUTE_SCAN) as r}<option value={r}>{r}</option>{/each}
          </select>
        </label>
        {#if !scanning}
          <label class="stack tight"><span class="small">Paper</span>
            <select value={L.look.print} onchange={(e) => L.setPrint((e.target as HTMLSelectElement).value)}>
              {#each L.meta.papers as p (p.name)}<option value={p.name}>{p.label}{p.name === film?.target_print ? " ★" : ""}</option>{/each}
            </select>
          </label>
        {/if}
      </details>

      <div class="row modes">
        <span class="muted small">Mode</span>
        <button class="mini" class:on={!pro} onclick={() => (pro = false)} title="The controls that matter day to day">Flow</button>
        <button class="mini" class:on={pro} onclick={() => (pro = true)} title="Every parameter the pipeline reads">Pro</button>
      </div>

      {#if pro}
        {#if L.effective}
          <ProGroup obj={L.effective} stock={L.stock} />
        {:else}
          <p class="muted small">Resolving the look…</p>
        {/if}
      {:else}
      <!-- The groups follow spektrafilm's Flow panel, so a look reads the same in both apps.
           Anything of ours that Flow does not show sits at the tail of its group. -->
      <details open>
        <summary>Film</summary>
        <Control label="Colour adaptation (CAT16)" path="settings.use_cat16" kind="check" hint="Chromatic adaptation feeding the spectral upsampling; off restores the older CAT02" />
        <Control label="Spectral upsampling" path="settings.rgb_to_raw_method" kind="select" options={opt(L.meta.upsamplers)} />
        <Control label="Film format" path="camera.film_format_mm" kind="select" options={FORMATS} hint="Sets how big the grain is on the frame" />
        <Control label="Exposure (EV)" path="camera.exposure_compensation_ev" min={-3} max={3} step={0.05} />
        <Control label="Auto exposure" path="camera.auto_exposure" kind="check" />
        <Control label="Taking filter" path="camera.color_filter" kind="select" options={filters} />
        <Control label="Contrast (film gamma)" path="film_render.chemistry.gamma_factor" min={0.25} max={4} step={0.01} />
        <Control label="Film base density" path="film_render.base.scale" min={0} max={3} step={0.05} />
      </details>

      {#if !scanning}
        <details open>
          <summary>Print</summary>
          <Control label="Exposure" path="enlarger.print_exposure" min={0.2} max={3} step={0.01} />
          <Control label="C filter" path="enlarger.c_filter_neutral" min={0} max={100} step={0.5} digits={1} hint="Cyan filtration (CC) on the enlarger" />
          <Control label="M filter shift" path="enlarger.m_filter_shift" min={-30} max={30} step={0.5} digits={1} hint="Enlarger magenta shift (CC). + = less green in the print." />
          <Control label="Y filter shift" path="enlarger.y_filter_shift" min={-30} max={30} step={0.5} digits={1} hint="Enlarger yellow shift (CC). + = less blue in the print." />
          <Control label="Preflash exposure" path="enlarger.preflash_exposure" min={0} max={1} step={0.005} digits={3} hint="A little even light on the paper before the print: lifts the toe, softens contrast" />
          <Control label="Preflash M filter shift" path="enlarger.preflash_m_filter_shift" min={-30} max={30} step={0.5} digits={1} />
          <Control label="Preflash Y filter shift" path="enlarger.preflash_y_filter_shift" min={-30} max={30} step={0.5} digits={1} />
          <button disabled={!!L.busy} onclick={() => L.neutralize()} title="Solve the filters so a midgray negative prints neutral">Neutralize filters</button>
          <Control label="Paper contrast" path="print_render.chemistry.gamma_factor" min={0.25} max={4} step={0.01} />
          <Control label="Paper base density" path="print_render.base.scale" min={0} max={5} step={0.05} />
          <Control label="Glare" path="print_render.glare.active" kind="check" />
        </details>
      {/if}

      <details>
        <summary>DIR couplers</summary>
        <Control label="Enabled" path="film_render.dir_couplers.active" kind="check" />
        <Control label="Amount" path="film_render.dir_couplers.amount" min={0} max={2} step={0.05} />
        <Control label="Diffusion (µm)" path="film_render.dir_couplers.diffusion_size_um" min={0} max={100} step={0.5} digits={1} />
        <Control label="Same-layer inhibition" path="film_render.dir_couplers.inhibition_samelayer" min={0} max={2} step={0.05} />
        <Control label="Interlayer inhibition" path="film_render.dir_couplers.inhibition_interlayer" min={0} max={2} step={0.05} />
      </details>

      <details>
        <summary>Grain</summary>
        <Control label="Enabled" path="film_render.grain.active" kind="check" />
        <div class="stack tight">
          <span class="small row spread">Amount (RMS) <span class="mono">{grainValue().toFixed(1)}</span></span>
          <input type="range" min="0" max="40" step="0.25" value={grainValue()} oninput={(e) => setGrain(Number((e.target as HTMLInputElement).value))} />
        </div>
        <Control label="Particle area (µm²)" path="film_render.grain.agx_particle_area_um2" min={0.02} max={2} step={0.01} />
        <Control label="Softness (px)" path="film_render.grain.blur" min={0} max={3} step={0.05} />
      </details>

      <details>
        <summary>Halation</summary>
        <Control label="Enabled" path="film_render.halation.active" kind="check" />
        <Control label="Amount" path="film_render.halation.halation_amount" min={0} max={3} step={0.05} />
        <Control label="Scale" path="film_render.halation.halation_spatial_scale" min={0.1} max={4} step={0.05} hint="How far the glow spreads" />
        <Control label="Boost (EV)" path="film_render.halation.boost_ev" min={0} max={4} step={0.05} hint="Extra light in the highlights that seed the glow" />
        <Rgb label="Strength RGB" path="film_render.halation.halation_strength" max={0.5} step={0.005} digits={3} />
        <Control label="Scatter amount" path="film_render.halation.scatter_amount" min={0} max={3} step={0.05} />
      </details>

      <details>
        <summary>Diffusion</summary>
        <Control label="Camera enabled" path="camera.diffusion_filter.active" kind="check" />
        <Control label="Camera family" path="camera.diffusion_filter.filter_family" kind="select" options={MIST} />
        <Control label="Camera strength" path="camera.diffusion_filter.strength" min={0} max={2} step={0.05} />
        <Control label="Camera halo warmth" path="camera.diffusion_filter.halo_warmth" min={-1} max={1} step={0.05} />
        {#if !scanning}
          <Control label="Print enabled" path="enlarger.diffusion_filter.active" kind="check" />
          <Control label="Print family" path="enlarger.diffusion_filter.filter_family" kind="select" options={MIST} />
          <Control label="Print strength" path="enlarger.diffusion_filter.strength" min={0} max={2} step={0.05} />
          <Control label="Print halo warmth" path="enlarger.diffusion_filter.halo_warmth" min={-1} max={1} step={0.05} />
        {/if}
      </details>

      <details>
        <summary>Scanner</summary>
        <Control label="White correction" path="scanner.white_correction" kind="check" />
        <Control label="Black correction" path="scanner.black_correction" kind="check" />
        <Control label="White level" path="scanner.white_level" min={0.5} max={1} step={0.005} digits={3} />
        <Control label="Black level" path="scanner.black_level" min={0} max={0.2} step={0.001} digits={3} />
        <div class="stack tight">
          <span class="small row spread">Sharpening <span class="mono">{usm()[1].toFixed(2)}</span></span>
          <input type="range" min="0" max="2" step="0.05" value={usm()[1]} oninput={(e) => L.set("scanner.unsharp_mask", [usm()[0], Number((e.target as HTMLInputElement).value)])} />
        </div>
        <Control label="Gamut compression" path="io.output_gamut_compress.algorithm" kind="select" options={opt(L.meta.gamut_algorithms)} />
      </details>

      {/if}

      <details>
        <summary>All changed parameters</summary>
        <textarea class="mono json" rows="14" value={paramsText} onchange={(e) => (paramsError = L.setParamsJson((e.target as HTMLTextAreaElement).value))}></textarea>
        {#if paramsError}<div class="bad small">{paramsError}</div>{/if}
        <div class="muted small">Only these fields differ from the film's stock defaults. Edit freely; any engine parameter works.</div>
      </details>
    {:else}
      <div class="muted">loading…</div>
    {/if}
  </aside>
</div>

<style>
  .looks {
    display: grid;
    grid-template-columns: 220px 1fr 320px;
    height: 100%;
    min-height: 0;
  }
  .list,
  .controls {
    overflow-y: auto;
    padding: 12px;
    background: var(--bg-2);
  }
  .list {
    border-right: 1px solid var(--line);
  }
  .controls {
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  ul {
    list-style: none;
    padding: 0;
    margin: 8px 0 0;
  }
  .item {
    width: 100%;
    text-align: left;
    display: flex;
    flex-direction: column;
    background: transparent;
    border-color: transparent;
    padding: 6px 8px;
  }
  .item.active {
    background: var(--bg-3);
    border-color: var(--line);
  }
  .imp textarea,
  .json {
    width: 100%;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 12px var(--mono);
    padding: 6px;
    user-select: text;
  }
  .center {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    padding: 10px 14px;
    gap: 8px;
  }
  .name {
    width: 240px;
    font-weight: 600;
  }
  .bar {
    flex-wrap: wrap;
    row-gap: 6px;
  }
  .bar > :global(.row) {
    flex-wrap: wrap;
  }
  .tag.right {
    left: auto;
    right: 10px;
  }
  .err,
  .busy {
    position: absolute;
    bottom: 10px;
    left: 10px;
    right: 10px;
    background: rgba(0, 0, 0, 0.7);
    padding: 6px 10px;
    border-radius: 6px;
    font-size: 12px;
  }
  .tag {
    position: absolute;
    top: 10px;
    left: 10px;
    background: rgba(0, 0, 0, 0.65);
    padding: 3px 9px;
    border-radius: 10px;
    font-size: 11.5px;
  }
  .toggle {
    min-width: 64px;
  }
  .toggle.on {
    border-color: var(--accent);
    color: var(--accent-2);
  }
  .strip {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    padding-bottom: 2px;
  }
  .thumb {
    padding: 0;
    width: 64px;
    height: 48px;
    flex: none;
    overflow: hidden;
  }
  .thumb.active {
    border-color: var(--accent);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  details {
    border-bottom: 1px solid var(--line);
    padding-bottom: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  details > :global(*:not(summary)) {
    margin-top: 8px;
  }
  summary {
    font-weight: 600;
    font-size: 12.5px;
    cursor: default;
  }
  .tight {
    gap: 3px;
  }
  .from-develop {
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 5px 8px;
  }
  .stage {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 5px 9px;
    font-size: 12px;
  }
  .small {
    font-size: 11.5px;
  }
  .empty {
    padding: 40px;
  }
  .warnings ul {
    list-style: disc;
    padding-left: 18px;
  }
  .tag.zoom {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .full {
    background: transparent;
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0 5px;
    font-size: 10.5px;
    color: var(--muted);
    line-height: 16px;
  }
  .full.on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
  .picker {
    padding: 6px 10px 0;
    gap: 8px;
  }
  .picker input[type="range"] {
    flex: 1;
    min-width: 0;
  }
  /* The picture's box: the viewport fills it, and only it. Without a positioned
     ancestor an absolute viewport pins to the page and hides everything. */
  .stage.pic {
    position: relative;
    overflow: hidden;
    padding: 0;
    min-height: 0;
    background: #0e0d0c;
    border-radius: var(--radius);
  }
  .modes {
    gap: 4px;
    margin-bottom: 4px;
  }
  .modes .on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
</style>
