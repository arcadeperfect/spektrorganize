<script lang="ts">
  import { open, confirm } from "@tauri-apps/plugin-dialog";
  import { SvelteSet } from "svelte/reactivity";
  import { catalog, type CatalogRoot, type Filter } from "../../api";
  import { library as lib } from "../../library.svelte";
  import { store } from "../../state.svelte";

  const f = $derived(lib.filter);

  /** What the catalog calls each kind, in words. */
  const KIND_NAMES: Record<string, string> = { raw: "RAW", image: "JPEG & images", video: "Video", other: "Other" };
  const facets = $derived(lib.facets);
  const folders = $derived(facets?.roots.filter((r) => r.kind !== "render") ?? []);

  // Years -> months, newest first.
  const years = $derived.by(() => {
    const out: { year: string; count: number; months: { key: string; count: number }[] }[] = [];
    for (const m of facets?.months ?? []) {
      const y = m.key.slice(0, 4);
      let e = out.find((x) => x.year === y);
      if (!e) out.push((e = { year: y, count: 0, months: [] }));
      e.count += m.count;
      e.months.push(m);
    }
    return out;
  });
  const openYears = new SvelteSet<string>();
  let yearsSeeded = false;
  $effect(() => {
    if (!yearsSeeded && years.length) {
      yearsSeeded = true;
      openYears.add(years[0].year);
    }
  });

  let kwFilter = $state("");
  const keywords = $derived.by(() => {
    const all = facets?.keywords ?? [];
    const q = kwFilter.trim().toLowerCase();
    return q ? all.filter((k) => k.name.toLowerCase().includes(q)) : all.slice(0, 60);
  });

  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  function monthRange(key: string): [string, string] {
    const [y, m] = key.split("-").map(Number);
    const last = new Date(y, m, 0).getDate();
    return [`${key}-01`, `${key}-${String(last).padStart(2, "0")}`];
  }
  function isRange(from: string, to: string) {
    return f.date_from === from && f.date_to === to;
  }

  async function addFolder() {
    const dir = await open({ directory: true, multiple: false, title: "Add a folder to the catalog (indexed in place, never moved)" });
    if (typeof dir === "string") lib.addFolder(dir);
  }

  async function adopt() {
    const dir = await open({
      directory: true,
      multiple: false,
      title: "Adopt an import archive (the folder with .spektrorganize/imports)",
      defaultPath: store.config?.archive_root,
    });
    if (typeof dir === "string") lib.adopt(dir);
  }

  async function relocate(r: CatalogRoot) {
    const dir = await open({ directory: true, multiple: false, title: `Where is ${r.label} now?` });
    if (typeof dir !== "string") return;
    try {
      const rep = await catalog.relocateRoot(r.id, dir, false);
      lib.note = `${r.label} now points to ${dir} (${rep.found}/${rep.checked} sampled files found).`;
    } catch (e) {
      lib.error = String(e);
    }
  }

  async function remove(r: CatalogRoot) {
    const ok = await confirm(
      `Forget “${r.label}” and its ${r.assets} photos (with their keywords) from the catalog? Nothing on disk is touched.`,
      { title: "Remove from catalog", kind: "warning", okLabel: "Remove" },
    );
    if (!ok) return;
    try {
      await catalog.removeRoot(r.id);
      if (f.roots?.includes(r.id)) lib.setFilter({ roots: f.roots.filter((x) => x !== r.id) });
    } catch (e) {
      lib.error = String(e);
    }
  }

  // Collapsible sections, remembered per machine.
  const SECTIONS_KEY = "spektrorganize.sections";
  let closed = $state(new SvelteSet<string>(loadClosed()));

  function loadClosed(): string[] {
    try {
      return JSON.parse(localStorage.getItem(SECTIONS_KEY) ?? "[]");
    } catch {
      return [];
    }
  }

  function toggleSection(name: string) {
    if (closed.has(name)) closed.delete(name);
    else closed.add(name);
    try {
      localStorage.setItem(SECTIONS_KEY, JSON.stringify([...closed]));
    } catch {
      // Remembering is a convenience; the section still toggles.
    }
  }
  /** Name being typed for a new catalog; null when the field is closed. */
  let naming = $state<string | null>(null);

  async function save() {
    if (!naming?.trim()) return;
    await lib.saveCollection(naming);
    naming = null;
  }

  // setFilter, not setDates: picking the same date again should not toggle the range off.
  function setFrom(v: string) {
    lib.setFilter({ date_from: v || null, undated: false });
  }
  function setTo(v: string) {
    lib.setFilter({ date_to: v || null, undated: false });
  }

  /** A one-line summary of a saved filter, for the tooltip. */
  function describe(x: Filter): string {
    const bits: string[] = [];
    if (x.text) bits.push(`"${x.text}"`);
    if (x.date_from || x.date_to) bits.push(`${x.date_from ?? "…"} → ${x.date_to ?? "…"}`);
    if (x.cameras?.length) bits.push(x.cameras.join(", "));
    if (x.keywords?.length) bits.push(x.keywords.join(", "));
    if (x.kinds?.length) bits.push(x.kinds.join(", "));
    if (x.flags?.length) bits.push(x.flags.join(", "));
    if (x.min_rating) bits.push(`${x.min_rating}+ hearts`);
    if (x.has_render === true) bits.push("printed");
    if (x.ai_labelled != null) bits.push(x.ai_labelled ? "AI labelled" : "not labelled");
    if (x.missing === true) bits.push("missing");
    if (x.undated) bits.push("undated");
    return bits.length ? bits.join(" · ") : "everything";
  }
</script>

<aside class="side">
  {#if facets}
    <section>
      <button class="item" class:active={!lib.hasFilter} onclick={() => lib.clearFilter()}>
        <span>All photos</span><span class="n">{facets.total}</span>
      </button>
      <button class="item" class:active={f.flags?.includes("select")} onclick={() => lib.setFilter({ flags: f.flags?.includes("select") ? [] : ["select"] })}>
        <span class="ok">✓ Selected</span><span class="n">{facets.selected}</span>
      </button>
      <button class="item" class:active={f.flags?.includes("reject")} onclick={() => lib.setFilter({ flags: f.flags?.includes("reject") ? [] : ["reject"] })}>
        <span class="bad">✕ Rejected</span><span class="n">{facets.rejected}</span>
      </button>
      <button class="item" class:active={!!f.min_rating} onclick={() => lib.setFilter({ min_rating: f.min_rating ? null : 1 })}>
        <span>♥ Rated{#if f.min_rating}<span class="stars"> {"♥".repeat(f.min_rating)}+</span>{/if}</span><span class="n">{facets.rated}</span>
      </button>
      {#if f.min_rating}
        <div class="rate-row" role="group" aria-label="Minimum rating">
          {#each [1, 2, 3, 4, 5] as n (n)}
            <button class="h" class:on={(f.min_rating ?? 0) >= n} onclick={() => lib.setFilter({ min_rating: n })} title="{n} heart{n === 1 ? '' : 's'} or more">♥</button>
          {/each}
        </div>
      {/if}
      <button class="item" class:active={f.has_render === true} onclick={() => lib.setFilter({ has_render: f.has_render === true ? null : true })}>
        <span>▣ Printed</span><span class="n">{facets.rendered}</span>
      </button>
      <button class="item" class:active={f.ai_labelled === true} onclick={() => lib.setFilter({ ai_labelled: f.ai_labelled === true ? null : true })}>
        <span>✦ AI labelled</span><span class="n">{facets.ai_labelled}</span>
      </button>
      <button class="item" class:active={f.ai_labelled === false} onclick={() => lib.setFilter({ ai_labelled: f.ai_labelled === false ? null : false })}>
        <span>Not labelled yet</span><span class="n">{facets.total - facets.ai_labelled}</span>
      </button>
      {#if facets.missing}
        <button class="item" class:active={f.missing === true} onclick={() => lib.setFilter({ missing: f.missing === true ? null : true })}>
          <span class="warn">Missing files</span><span class="n">{facets.missing}</span>
        </button>
      {/if}
      <button class="item" class:active={lib.duplicates} onclick={() => (lib.duplicates = true)} title="Find files that are byte-for-byte copies">
        <span class="muted">Duplicates…</span>
      </button>
      {#if facets.rejected}
        <button class="item" class:active={lib.rejects} onclick={() => (lib.rejects = true)} title="Review the rejects and clear them out">
          <span class="muted">Purge rejected…</span><span class="n">{facets.rejected}</span>
        </button>
      {/if}
    </section>

    <section>
      <h3>
        <button class="twist sect" onclick={() => toggleSection("Catalogs")} title="Show or hide">{closed.has("Catalogs") ? "▸" : "▾"}</button>
        <span>Catalogs</span>
        <button class="tiny" title="Save the current filter as a dynamic catalog" onclick={() => (naming = naming === null ? "" : null)}>+</button>
      </h3>
      {#if !closed.has("Catalogs")}
        {#if naming !== null}
          <div class="range">
            <input
              class="name"
              bind:value={naming}
              placeholder="name this catalog"
              spellcheck="false"
              onkeydown={(e) => {
                if (e.key === "Enter") save();
                if (e.key === "Escape") naming = null;
              }}
            />
            <button class="tiny" onclick={save} title="Save">✓</button>
          </div>
        {/if}
        {#each lib.collections as c (c.id)}
          <div class="root">
            <button class="item" class:active={lib.openCollection === c.id} onclick={() => lib.openCollectionById(c.id)} title={describe(c.filter)}>
              <span class="lbl">{c.name}</span>
            </button>
            <button class="tiny del" title="Delete this catalog" onclick={() => lib.deleteCollection(c.id)}>×</button>
          </div>
        {/each}
        {#if !lib.collections.length && naming === null}
          <p class="muted small empty">Filter the library, then press + to keep it as a catalog that re-runs itself.</p>
        {/if}
      {/if}
    </section>

    <section>
      <h3>
        <button class="twist sect" onclick={() => toggleSection("Folders")} title="Show or hide">{closed.has("Folders") ? "▸" : "▾"}</button>
        <span>Folders</span>
        <button class="tiny" title="Add a folder (indexed in place)" onclick={addFolder}>+</button>
      </h3>
      {#if !closed.has("Folders")}
      {#each folders as r (r.id)}
        <div class="root" class:active={f.roots?.includes(r.id)}>
          <button class="item" onclick={() => lib.only("roots", r.id)} title="{r.path}{r.kind === 'archive' ? ' — import archive' : ' — indexed in place'}">
            <span class="lbl">
              <span class="kind">{r.kind === "archive" ? "◆" : "▢"}</span>
              {r.label}
              {#if !r.online}<span class="off">offline</span>{/if}
            </span>
            <span class="n">{r.assets}</span>
          </button>
          <span class="tools">
            <button class="tiny" title="Rescan for new, changed and moved files" disabled={!!lib.indexing} onclick={() => lib.rescan(r.id, r.label)}>↻</button>
            <button class="tiny" title="It moved: point to its new location…" onclick={() => relocate(r)}>⇢</button>
            <button class="tiny" title="Remove from the catalog (files stay)" onclick={() => remove(r)}>×</button>
          </span>
        </div>
      {/each}
      <div class="adds">
        <button class="mini" disabled={!!lib.indexing} onclick={addFolder}>+ Add folder…</button>
        <button class="mini" disabled={!!lib.indexing} onclick={adopt} title="Bring in an existing import archive from its manifests, renders included">Adopt archive…</button>
      </div>{/if}
    </section>

    {#if years.length || facets.undated}
      <section>
      <h3>
        <button class="twist sect" onclick={() => toggleSection("Dates")} title="Show or hide">{closed.has("Dates") ? "▸" : "▾"}</button>
        <span>Dates</span>
      </h3>
      {#if !closed.has("Dates")}
        <!-- Any range, not just the year/month drilldown below. -->
        <div class="range">
          <input type="date" value={f.date_from ?? ""} max={f.date_to ?? undefined} onchange={(e) => setFrom(e.currentTarget.value)} title="From" />
          <span class="muted">→</span>
          <input type="date" value={f.date_to ?? ""} min={f.date_from ?? undefined} onchange={(e) => setTo(e.currentTarget.value)} title="To" />
          {#if f.date_from || f.date_to}
            <button class="tiny" title="Clear the range" onclick={() => lib.setFilter({ date_from: null, date_to: null })}>×</button>
          {/if}
        </div>
        {#each years as y (y.year)}
          <div class="yr">
            <button class="twist" onclick={() => (openYears.has(y.year) ? openYears.delete(y.year) : openYears.add(y.year))}>
              {openYears.has(y.year) ? "▾" : "▸"}
            </button>
            <button class="item" class:active={isRange(`${y.year}-01-01`, `${y.year}-12-31`)} onclick={() => lib.setDates(`${y.year}-01-01`, `${y.year}-12-31`)}>
              <span>{y.year}</span><span class="n">{y.count}</span>
            </button>
          </div>
          {#if openYears.has(y.year)}
            {#each y.months as m (m.key)}
              {@const [from, to] = monthRange(m.key)}
              <button class="item sub" class:active={isRange(from, to)} onclick={() => lib.setDates(from, to)}>
                <span>{MONTHS[Number(m.key.slice(5, 7)) - 1]}</span><span class="n">{m.count}</span>
              </button>
            {/each}
          {/if}
        {/each}
        {#if facets.undated}
          <button class="item" class:active={f.undated} onclick={() => lib.setFilter({ undated: !f.undated, date_from: null, date_to: null })}>
            <span class="muted">Undated</span><span class="n">{facets.undated}</span>
          </button>
        {/if}{/if}
    </section>
    {/if}

    {#if facets.kinds.length > 1}
      <section>
      <h3>
        <button class="twist sect" onclick={() => toggleSection("Kinds")} title="Show or hide">{closed.has("Kinds") ? "▸" : "▾"}</button>
        <span>Kinds</span>
      </h3>
      {#if !closed.has("Kinds")}
        {#each facets.kinds as k (k.key)}
          <button class="item" class:active={f.kinds?.includes(k.key)} onclick={() => lib.toggle("kinds", k.key)}>
            <span>{KIND_NAMES[k.key] ?? k.key}</span><span class="n">{k.count}</span>
          </button>
        {/each}{/if}
    </section>
    {/if}

    {#if facets.cameras.length}
      <section>
      <h3>
        <button class="twist sect" onclick={() => toggleSection("Cameras")} title="Show or hide">{closed.has("Cameras") ? "▸" : "▾"}</button>
        <span>Cameras</span>
      </h3>
      {#if !closed.has("Cameras")}
        {#each facets.cameras as c (c.key)}
          <button class="item" class:active={f.cameras?.includes(c.key)} onclick={() => lib.toggle("cameras", c.key)}>
            <span class:muted={!c.key}>{c.key || "unknown"}</span><span class="n">{c.count}</span>
          </button>
        {/each}{/if}
    </section>
    {/if}

    <section>
      <h3>
        <button class="twist sect" onclick={() => toggleSection("Keywords")} title="Show or hide">{closed.has("Keywords") ? "▸" : "▾"}</button>
        <span>Keywords</span>
      </h3>
      {#if !closed.has("Keywords")}
      {#if (facets.keywords.length ?? 0) > 12}
        <input class="kwf" type="text" placeholder="filter keywords" bind:value={kwFilter} spellcheck="false" />
      {/if}
      {#each keywords as k (k.name)}
        <button class="item" class:active={f.keywords?.includes(k.name)} onclick={() => lib.toggle("keywords", k.name)} title={k.ai ? `${k.ai} of ${k.count} from AI labels` : ""}>
          <span class="lbl">{#if k.ai === k.count}<span class="aimark">✦</span>{/if}{k.name}</span><span class="n">{k.count}</span>
        </button>
      {:else}
        <div class="muted small pad">Select photos and add keywords, or ✦ Label them.</div>
      {/each}{/if}
    </section>
  {/if}
</aside>

<style>
  .twist.sect {
    width: 16px;
  }
  .side {
    overflow-y: auto;
    padding: 10px 8px 24px;
    border-right: 1px solid var(--line);
    background: var(--bg-2);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  h3 {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 10.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    margin: 0 0 4px;
    padding: 0 6px;
  }
  .item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 6px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-radius: 5px;
    padding: 4px 6px;
    color: var(--text);
    min-width: 0;
  }
  .item:hover {
    background: var(--bg-3);
  }
  .item.active {
    background: #3a2a1c;
    color: var(--accent-2);
  }
  .item.sub {
    padding-left: 26px;
  }
  .lbl {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .n {
    color: var(--muted);
    font-size: 11px;
    flex: none;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--bad);
  }
  .stars {
    color: var(--accent-2);
    font-size: 10px;
  }
  .range {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px 6px;
  }
  .range input[type="date"] {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    padding: 2px 4px;
  }
  .range .name {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    padding: 2px 6px;
  }
  .del {
    opacity: 0.6;
  }
  .del:hover {
    opacity: 1;
    color: var(--bad);
  }
  .empty {
    padding: 0 8px 6px;
    line-height: 1.35;
  }
  .rate-row {
    display: flex;
    padding: 0 8px 4px 26px;
  }
  .rate-row .h {
    background: transparent;
    border: 0;
    padding: 0 2px;
    color: #4a433e;
    font-size: 13px;
  }
  .rate-row .h.on {
    color: var(--accent-2);
  }
  .kind {
    color: var(--muted);
    font-size: 10px;
    margin-right: 3px;
  }
  .off {
    font-size: 9.5px;
    color: var(--warn);
    margin-left: 4px;
  }
  .root {
    position: relative;
  }
  .root .tools {
    position: absolute;
    right: 2px;
    top: 1px;
    display: none;
    gap: 1px;
    background: var(--bg-3);
    border-radius: 4px;
  }
  .root:hover .tools {
    display: flex;
  }
  .tiny {
    border: 0;
    background: transparent;
    padding: 1px 6px;
    color: var(--muted);
    font-size: 12px;
  }
  .tiny:hover {
    color: var(--text);
    background: var(--bg-3);
  }
  .adds {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 6px 4px 0;
  }
  .yr {
    display: flex;
    align-items: center;
  }
  .twist {
    border: 0;
    background: transparent;
    color: var(--muted);
    padding: 0 2px 0 4px;
    width: 18px;
    font-size: 10px;
  }
  .kwf {
    margin: 0 4px 4px;
    width: auto;
    padding: 4px 6px;
    font-size: 12px;
  }
  .aimark {
    color: #cbb8f2;
    font-size: 9px;
    margin-right: 4px;
  }
  .small {
    font-size: 11px;
  }
  .pad {
    padding: 2px 6px;
  }
</style>
