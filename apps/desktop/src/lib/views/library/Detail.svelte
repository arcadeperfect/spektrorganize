<script lang="ts">
  import { untrack } from "svelte";
  import { api, catalog, human, type LabelInfo } from "../../api";
  import { library as lib } from "../../library.svelte";
  import { develop } from "../../photo.svelte";
  import { panzoom } from "../../panzoom";
  import { store } from "../../state.svelte";

  /** Open a stage on this photo: develop (its own settings) or print (the film look). */
  function openStage(id: number, view: "develop" | "print") {
    develop.setPhoto(id);
    store.view = view;
  }

  const d = $derived(lib.detail);
  const multi = $derived(lib.selected.size > 1);

  // Large preview for the single asset: its 1024 px preview, or a print chosen below.
  let preview = $state<string | null>(null);
  let shownRender = $state<string | null>(null);
  /** The cached, viewable copy of `shownRender` (the render itself is outside the webview's reach). */
  let shownRenderSrc = $state<string | null>(null);
  const detailId = $derived(d?.id);
  // Only when another asset is shown (not on every refresh of the same one).
  $effect(() => {
    const id = detailId;
    const known = untrack(() => lib.detail?.preview ?? null);
    shownRender = shownRenderSrc = null;
    preview = known;
    if (id === undefined || known) return;
    let live = true;
    catalog
      .preview(id)
      .then((p) => {
        if (live && lib.detail?.id === id) preview = p;
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  });

  // Keyword coverage across a multi-selection (first 500).
  let multiInfo = $state<LabelInfo[]>([]);
  $effect(() => {
    void lib.kwVersion;
    const ids = [...lib.selected].slice(0, 500);
    if (ids.length < 2) {
      multiInfo = [];
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      catalog
        .labelInfo(ids)
        .then((r) => {
          if (live) multiInfo = r;
        })
        .catch(() => {});
    }, 120);
    return () => {
      live = false;
      clearTimeout(t);
    };
  });
  const coverage = $derived.by(() => {
    const m = new Map<string, { n: number; ai: boolean }>();
    for (const it of multiInfo) {
      for (const k of it.keywords) m.set(k, { n: (m.get(k)?.n ?? 0) + 1, ai: false });
      for (const k of it.ai_keywords) {
        const e = m.get(k);
        m.set(k, { n: (e?.n ?? 0) + 1, ai: e ? e.ai : true });
      }
    }
    return [...m].sort((a, b) => b[1].n - a[1].n || a[0].localeCompare(b[0]));
  });

  let newKw = $state("");
  function targets(): number[] {
    return multi ? [...lib.selected] : d ? [d.id] : [];
  }
  function onKwKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      addKw();
    }
  }
  async function addKw() {
    const words = newKw
      .split(",")
      .map((w) => w.trim())
      .filter(Boolean);
    if (!words.length) return;
    newKw = "";
    await lib.tag(targets(), words, []);
    if (multi) multiInfo = await catalog.labelInfo([...lib.selected].slice(0, 500));
  }
  async function dropKw(name: string) {
    await lib.tag(targets(), [], [name]);
    if (multi) multiInfo = await catalog.labelInfo([...lib.selected].slice(0, 500));
  }

  async function rate(n: number) {
    if (!d) return;
    await lib.rate(targets(), d.rating === n ? 0 : n);
  }

  async function flag(f: "select" | "reject") {
    await lib.flag(targets(), f);
  }

  const ROLE: Record<string, string> = {
    raw: "RAW",
    jpeg: "JPEG sidecar",
    image: "image",
    video: "video",
    xmp: "XMP",
    sidecar: "sidecar",
    other: "other",
  };
  function when(s: string | null) {
    return s ? s.replace("T", " ").replace("Z", " UTC") : "—";
  }
  const userKw = $derived(d?.keywords.filter((k) => k.source === "user") ?? []);
  const aiKw = $derived(d?.keywords.filter((k) => k.source !== "user") ?? []);
</script>

<aside class="detail">
  {#if multi}
    <h2>{lib.selected.size} selected</h2>
    <p class="muted small">Keywords, rating, labels and prints apply to all of them.</p>
    <div class="chips">
      {#each coverage as [name, c] (name)}
        <span class="chip" class:ai={c.ai} title={c.ai ? "AI label" : ""}>
          {name}{#if c.n < multiInfo.length}<span class="cov">{c.n}/{multiInfo.length}</span>{/if}
          <button class="x" title="Remove from all selected" onclick={() => dropKw(name)}>×</button>
        </span>
      {/each}
      <input class="kw-in" bind:value={newKw} onkeydown={onKwKey} placeholder="+ keyword" title="Enter adds; separate several with commas" spellcheck="false" />
    </div>
    <div class="row rate">
      <div class="hearts" role="group" aria-label="Rating">
        {#each [1, 2, 3, 4, 5] as n (n)}
          <button onclick={() => lib.rate([...lib.selected], n)} title="Give all {n} heart{n === 1 ? '' : 's'} ({n})">♥</button>
        {/each}
        <button class="mini clear" onclick={() => lib.rate([...lib.selected], 0)} title="Clear the rating (0)">clear</button>
      </div>
      <div class="flags" role="group" aria-label="Select or reject">
        <button class="mini pick" onclick={() => lib.flag([...lib.selected], "select")} title="Select all (S)">✓</button>
        <button class="mini drop" onclick={() => lib.flag([...lib.selected], "reject")} title="Reject all (R)">✕</button>
      </div>
    </div>
    <div class="acts">
      <button class="ai" disabled={!!lib.labelRun} onclick={() => lib.askLabel()}>✦ Label {lib.selected.size}</button>
      {#if coverage.some(([, c]) => c.ai)}
        <button class="mini" onclick={() => lib.clearAi([...lib.selected])} title="Remove the AI labels (keeps your own keywords)">clear AI labels</button>
      {/if}
      <button onclick={() => lib.askPrint()}>Print {lib.selected.size}…</button>
      <button onclick={() => openStage([...lib.selected][0], "develop")} title="White balance and exposure for these photos">Develop…</button>
      <button onclick={() => lib.queueAdd([...lib.selected])} title="Add to the export queue">Queue {lib.selected.size}</button>
      <button onclick={() => lib.askExport([...lib.selected])}>Export {lib.selected.size}…</button>
    </div>
  {:else if d}
    <div class="pv" use:panzoom={{ key: `${d.id}:${shownRender ?? ""}`, keys: "hover" }} role="img" aria-label="Preview">
      <div class="pv-view">
        {#if shownRender}
          {#if shownRenderSrc}<img src={api.fileUrl(shownRenderSrc)} alt="print" />{:else}<span class="muted small">loading print…</span>{/if}
        {:else if preview || d.thumb}
          <img src={api.fileUrl((preview ?? d.thumb)!)} alt="" />
        {:else}
          <span class="muted small">{d.kind === "video" ? "video" : "no preview"}</span>
        {/if}
      </div>
    </div>
    {#if shownRender}
      <div class="row spread small">
        <span class="muted">showing a print</span>
        <button class="mini" onclick={() => (shownRender = null)}>back to camera preview</button>
      </div>
    {/if}

    <h2 class="name">{d.files[0]?.name ?? `#${d.id}`}</h2>
    <div class="row rate">
      <div class="hearts" role="group" aria-label="Rating">
        {#each [1, 2, 3, 4, 5] as n (n)}
          <button class:on={d.rating >= n} onclick={() => rate(n)} title="{n} heart{n === 1 ? '' : 's'} ({n})">♥</button>
        {/each}
      </div>
      <div class="flags" role="group" aria-label="Select or reject">
        <button class="mini pick" class:on={d.flag === "select"} onclick={() => flag("select")} title="Select (S)">✓ select</button>
        <button class="mini drop" class:on={d.flag === "reject"} onclick={() => flag("reject")} title="Reject (R)">✕ reject</button>
      </div>
    </div>

    <dl>
      <dt>taken</dt>
      <dd>{when(d.captured_at)}{#if d.meta_source === "mtime"}<span class="muted"> (file date)</span>{/if}</dd>
      {#if d.camera || d.make}<dt>camera</dt><dd>{[d.make, d.camera].filter(Boolean).join(" ")}</dd>{/if}
      {#if d.lens}<dt>lens</dt><dd>{d.lens}</dd>{/if}
      {#if d.iso}<dt>ISO</dt><dd>{d.iso}</dd>{/if}
      {#if d.width && d.height}<dt>size</dt><dd>{d.width} × {d.height}</dd>{/if}
      {#if d.import_source}<dt>imported</dt><dd>from {d.import_source}, {when(d.imported_at)}</dd>{/if}
    </dl>

    <h4>Keywords</h4>
    <div class="chips">
      {#each userKw as k (k.name)}
        <span class="chip">{k.name}<button class="x" title="Remove" onclick={() => dropKw(k.name)}>×</button></span>
      {/each}
      {#each aiKw as k (k.name)}
        <span class="chip ai" title="AI label ({k.source})">{k.name}<button class="x" title="Remove" onclick={() => dropKw(k.name)}>×</button></span>
      {/each}
      <input class="kw-in" bind:value={newKw} onkeydown={onKwKey} placeholder="+ keyword" title="Enter adds; separate several with commas" spellcheck="false" />
    </div>
    <div class="acts">
      <button class="ai" disabled={!!lib.labelRun} onclick={() => lib.askLabel()}>✦ Label</button>
      <button onclick={() => openStage(d.id, "develop")} title="White balance, exposure and highlights for this photo">Develop…</button>
      <button onclick={() => openStage(d.id, "print")} title="Choose and tune the film look this photo prints with">Print look…</button>
      <button onclick={() => lib.queueAdd([d.id])} title="Add to the export queue">Queue</button>
      <button onclick={() => lib.askExport([d.id])}>Export…</button>
      {#if aiKw.length}
        <button class="mini" onclick={() => lib.clearAi([d.id])} title="Remove the AI labels (keeps your own keywords)">clear AI labels</button>
      {/if}
    </div>

    <h4>Files</h4>
    <ul class="files">
      {#each d.files as file (file.id)}
        <li>
          <span class="role" class:jpeg={file.role === "jpeg"}>{ROLE[file.role] ?? file.role}</span>
          <span class="fname" title={file.path}>{file.name}</span>
          {#if file.missing}<span class="bad small">missing</span>{:else if !file.online}<span class="warn small">offline</span>{/if}
          <span class="muted small">{human(file.size)}</span>
          <button class="mini" title="Reveal in Finder" onclick={() => api.reveal(file.path)}>↗</button>
        </li>
      {/each}
    </ul>

    <h4>Prints</h4>
    {#if d.renders.length}
      <ul class="files">
        {#each d.renders as r (r.id)}
          <li>
            <span class="role print">{r.kind.toUpperCase()}</span>
            <button
              class="link fname"
              title={r.path}
              disabled={!r.exists || r.kind !== "jpeg"}
              onclick={async () => {
                if (shownRender === r.path) {
                  shownRender = shownRenderSrc = null;
                  return;
                }
                shownRender = r.path;
                shownRenderSrc = null;
                shownRenderSrc = await catalog.renderPreview(r.id, 1024).catch(() => null);
              }}
            >
              {r.preset_name ?? r.preset_hash}
            </button>
            {#if !r.exists}<span class="bad small">missing</span>{/if}
            <span class="muted small">{r.created_at.slice(0, 10)}</span>
            <button class="mini" title="Open" disabled={!r.exists} onclick={() => api.openPath(r.path)}>open</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="muted small">Not printed yet.</p>
    {/if}
    <div class="acts">
      <button onclick={() => lib.askPrint()} disabled={d.kind !== "raw"} title={d.kind === "raw" ? "" : "Only RAWs can be printed"}>Print…</button>
    </div>
  {:else}
    <p class="muted small empty">Select a photo to see its files, keywords and prints. Shift-click or ⇧-arrows select ranges, ⌘A selects the whole view.</p>
  {/if}
</aside>

<style>
  .detail {
    overflow-y: auto;
    padding: 12px 14px 24px;
    border-left: 1px solid var(--line);
    background: var(--bg-2);
  }
  .pv {
    display: grid;
    place-items: center;
    background: var(--bg);
    border-radius: 6px;
    aspect-ratio: 1 / 1;
    overflow: hidden;
    margin-bottom: 8px;
      overflow: hidden;
    position: relative;
  }
  .pv-view {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .pv img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  h2 {
    font-size: 14px;
    margin: 4px 0 2px;
  }
  .name {
    word-break: break-all;
  }
  h4 {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    margin: 14px 0 6px;
  }
  dl {
    display: grid;
    grid-template-columns: 70px 1fr;
    gap: 3px 8px;
    margin: 8px 0 0;
    font-size: 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .rate {
    justify-content: space-between;
    gap: 8px;
  }
  .hearts {
    display: flex;
    gap: 0;
  }
  .hearts button {
    background: transparent;
    border: 0;
    padding: 0 2px;
    color: #4a433e;
    font-size: 15px;
  }
  .hearts button.on {
    color: var(--accent-2);
  }
  .flags {
    display: flex;
    gap: 4px;
  }
  .hearts .clear {
    margin-left: 6px;
    align-self: center;
  }
  .flags .pick.on {
    color: var(--ok);
    border-color: var(--ok);
  }
  .flags .drop.on {
    color: var(--bad);
    border-color: var(--bad);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
  }
  .kw-in {
    width: 110px;
    padding: 2px 6px;
    font-size: 11.5px;
  }
  .cov {
    opacity: 0.6;
    font-size: 9.5px;
    margin-left: 3px;
  }
  .acts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }
  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .files li {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    min-width: 0;
  }
  .role {
    flex: none;
    font-size: 9.5px;
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--bg-3);
    color: var(--muted);
  }
  .role.jpeg {
    background: #22301f;
    color: #a9cf98;
  }
  .role.print {
    background: #3a2a1c;
    color: var(--accent-2);
  }
  .fname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link {
    background: transparent;
    border: 0;
    padding: 0;
    text-align: left;
    color: var(--text);
    text-decoration: underline dotted;
  }
  .small {
    font-size: 11px;
  }
  .empty {
    margin-top: 40px;
    text-align: center;
  }
  .pv-view img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
</style>
