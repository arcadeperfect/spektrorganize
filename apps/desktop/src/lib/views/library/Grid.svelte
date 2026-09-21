<script lang="ts">
  // Virtualized thumbnail grid: only the rows in view (plus a few) exist in the DOM; pages of the
  // listing are fetched as they scroll in. What is "visible" comes from the scroll box's own
  // geometry, updated on scroll and resize events (no IntersectionObserver / rAF, which stop
  // firing in a window that isn't compositing).
  import { onMount, untrack } from "svelte";
  import { api, duration as durationText, type AssetSummary } from "../../api";
  import { library as lib } from "../../library.svelte";

  const GAP = 6;
  const PAD = 10;
  const CAPTION = 34;
  const OVERSCAN = 3;

  let scroller: HTMLDivElement | undefined = $state();
  let width = $state(900);
  let height = $state(600);
  let scrollTop = $state(0);

  const cols = $derived(Math.max(1, Math.floor((width - 2 * PAD + GAP) / (lib.tile + GAP))));
  const cell = $derived(Math.floor((width - 2 * PAD - GAP * (cols - 1)) / cols));
  const rowH = $derived(cell + CAPTION + GAP);
  const rows = $derived(Math.ceil(lib.total / cols));
  const firstRow = $derived(Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN));
  const lastRow = $derived(Math.min(rows - 1, Math.ceil((scrollTop + height) / rowH) + OVERSCAN));
  const rowList = $derived.by(() => {
    const out: number[] = [];
    for (let r = firstRow; r <= lastRow; r++) out.push(r);
    return out;
  });

  function measure() {
    if (!scroller) return;
    width = scroller.clientWidth;
    height = scroller.clientHeight;
    scrollTop = scroller.scrollTop;
  }

  onMount(() => {
    // Come back to where we were (switching views unmounts the grid). Measure
    // first so the rows have their real height before the position is restored.
    measure();
    if (scroller && lib.scroll) {
      scroller.scrollTop = lib.scroll;
      scrollTop = scroller.scrollTop;
    }
    const ro = new ResizeObserver(measure);
    if (scroller) ro.observe(scroller);
    window.addEventListener("resize", measure);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", measure);
    };
  });

  // Load the pages in view.
  $effect(() => {
    lib.ensure(firstRow * cols, Math.min(lib.total, (lastRow + 1) * cols));
  });

  // Ask for thumbnails of what is in view (coalesced into one request per tick).
  $effect(() => {
    for (let i = firstRow * cols; i < Math.min(lib.total, (lastRow + 1) * cols); i++) {
      const a = lib.item(i);
      if (a && (a.kind === "raw" || a.kind === "image") && lib.thumbOf(a) === undefined) lib.want(a.id);
    }
  });

  /**
   * The first photo in view. Scroll is kept in pixels, but the row height
   * changes with the tile size (and the column count with the window), so the
   * same pixel position means a different photo afterwards. Re-derive the
   * position from this photo whenever the layout changes.
   */
  let anchorIndex = 0;
  let layout = untrack(() => `${cols}x${rowH}`);
  $effect(() => {
    const now = `${cols}x${rowH}`;
    if (now === layout) return;
    layout = now;
    if (!scroller || lib.scroll === 0) return;
    const top = PAD + Math.floor(anchorIndex / cols) * rowH;
    scroller.scrollTop = top;
    scrollTop = top;
    lib.scroll = top;
  });

  /** Remember which photo is at the top, under the geometry in force right now. */
  function noteAnchor() {
    anchorIndex = Math.max(0, Math.floor((scrollTop - PAD) / rowH)) * cols;
  }

  // A new filter starts at the top — but only a new one. `resets` is already
  // non-zero when the grid mounts, so compare against what we last saw,
  // otherwise every return to the Library would jump to the top.
  let seenResets = untrack(() => lib.resets);
  $effect(() => {
    if (lib.resets === seenResets) return;
    seenResets = lib.resets;
    if (!scroller) return;
    scroller.scrollTop = 0;
    scrollTop = 0;
    lib.scroll = 0;
  });

  // Bring one index into view (set when the full-screen viewer closes).
  $effect(() => {
    if (lib.scrollTo !== null) {
      scrollToIndex(lib.scrollTo);
      lib.scrollTo = null;
    }
  });

  function indices(r: number): number[] {
    const out: number[] = [];
    for (let c = 0; c < cols; c++) {
      const i = r * cols + c;
      if (i < lib.total) out.push(i);
    }
    return out;
  }

  function when(a: AssetSummary): string {
    return a.captured_at ? a.captured_at.slice(0, 16).replace("T", " ") : "undated";
  }

  function scrollToIndex(i: number) {
    if (!scroller) return;
    const top = PAD + Math.floor(i / cols) * rowH;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + rowH > scroller.scrollTop + height) scroller.scrollTop = top + rowH - height;
  }

  function onKey(e: KeyboardEvent) {
    // The full-screen viewer owns the keyboard while it is open.
    if (lib.loupe !== null) return;
    const mod = e.metaKey || e.ctrlKey;
    if ((e.key === "Enter" || e.key === " ") && !mod) {
      const i = lib.cursor ?? lib.anchor;
      if (i !== null) {
        e.preventDefault();
        lib.loupe = i;
      }
      return;
    }
    if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      lib.selectAll();
      return;
    }
    if (e.key === "Escape") {
      lib.clearSelection();
      return;
    }
    if (lib.ratingKey(e, lib.keyTargets())) {
      e.preventDefault();
      return;
    }
    const step: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols };
    if (!(e.key in step) || lib.total === 0) return;
    e.preventDefault();
    const from = lib.cursor ?? lib.anchor ?? -1;
    const to = Math.min(lib.total - 1, Math.max(0, from + step[e.key]));
    const a = lib.item(to);
    if (!a) return;
    lib.cursor = to;
    lib.click(to, a.id, { shiftKey: e.shiftKey, metaKey: false, ctrlKey: false });
    scrollToIndex(to);
  }
</script>

<div
  class="scroller"
  bind:this={scroller}
  onscroll={() => scroller && ((scrollTop = scroller.scrollTop), (lib.scroll = scrollTop), noteAnchor())}
  onkeydown={onKey}
  tabindex="0"
  role="listbox"
  aria-multiselectable="true"
  aria-label="Photos"
>
  <div class="spacer" style="height: {rows * rowH + 2 * PAD}px">
    {#each rowList as r (r)}
      <div
        class="row"
        style="top: {PAD + r * rowH}px; left: {PAD}px; grid-template-columns: repeat({cols}, {cell}px); gap: {GAP}px"
      >
        {#each indices(r) as i (i)}
          {@const a = lib.item(i)}
          {#if a}
            {@const t = lib.thumbOf(a)}
            <div
              class="tile"
              class:sel={lib.selected.has(a.id)}
              class:focus={lib.focus === a.id}
              class:rejected={a.flag === "reject"}
              role="option"
              aria-selected={lib.selected.has(a.id)}
              tabindex="-1"
              onclick={(e) => {
                lib.cursor = i;
                lib.click(i, a.id, e);
              }}
              ondblclick={() => (lib.loupe = i)}
              oncontextmenu={(e) => {
                e.preventDefault();
                lib.cursor = i;
                // Right-clicking outside the selection acts on that photo alone.
                if (!lib.selected.has(a.id)) lib.click(i, a.id, { shiftKey: false, metaKey: false, ctrlKey: false });
                lib.menu = { x: e.clientX, y: e.clientY, ids: lib.selected.has(a.id) ? [...lib.selected] : [a.id] };
              }}
              onkeydown={() => {}}
              title={a.name}
            >
              <div class="img" style="height: {cell}px">
                {#if t}
                  <img src={api.fileUrl(t)} alt="" draggable="false" onerror={() => lib.thumbBroken(a.id)} />
                {:else if t === null}
                  <span class="ph">{a.kind === "video" ? "▶ video" : "no preview"}</span>
                {:else}
                  <span class="ph pending"></span>
                {/if}
                <span class="badges">
                  {#if a.has_jpeg}
                    <span class="b" title="RAW with its camera JPEG linked">RAW+JPG</span>
                  {:else if a.kind === "raw"}
                    <span class="b">RAW</span>
                  {/if}
                  {#if a.renders}<span class="b print" title="{a.renders} print{a.renders === 1 ? '' : 's'}">▣ {a.renders}</span>{/if}
                  {#if a.ai_labelled}<span class="b ai" title="AI keywords">✦</span>{/if}
                </span>
                {#if a.missing}
                  <span class="flag bad">missing</span>
                {:else if !a.online}
                  <span class="flag warn">offline</span>
                {/if}
                {#if a.flag}<span class="mark {a.flag}" title={a.flag === "select" ? "Selected (S)" : "Rejected (R)"}>{a.flag === "select" ? "✓" : "✕"}</span>{/if}
                {#if a.kind === "video"}
                  <span class="clip">▶{a.duration ? ` ${durationText(a.duration)}` : ""}</span>
                {/if}
                {#if a.rating}<span class="hearts">{"♥".repeat(a.rating)}</span>{/if}
              </div>
              <div class="cap">
                <span class="name">{a.name}</span>
                <span class="date">{when(a)}{a.camera ? ` · ${a.camera}` : ""}</span>
              </div>
            </div>
          {:else}
            <div class="tile loading"><div class="img" style="height: {cell}px"></div></div>
          {/if}
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .scroller {
    position: relative;
    overflow-y: auto;
    overflow-x: hidden;
    height: 100%;
    outline: none;
  }
  .spacer {
    position: relative;
  }
  .row {
    position: absolute;
    display: grid;
  }
  .tile {
    border-radius: 6px;
    padding: 3px;
    border: 1px solid transparent;
    min-width: 0;
  }
  .tile:hover {
    background: var(--bg-2);
  }
  .tile.sel {
    background: #3a2a1c;
    border-color: var(--accent);
  }
  .tile.focus {
    box-shadow: 0 0 0 1px var(--accent-2) inset;
  }
  .img {
    position: relative;
    display: grid;
    place-items: center;
    background: var(--bg-2);
    border-radius: 4px;
    overflow: hidden;
  }
  .tile.loading .img {
    background: var(--bg-2);
    opacity: 0.5;
  }
  img {
    /* Fill the tile: with max-width alone a 256 px thumbnail sat small and
       centred in a large cell instead of scaling up to it. */
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
  .ph {
    font-size: 11px;
    color: var(--muted);
  }
  .ph.pending {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid var(--line);
    border-top-color: var(--muted);
    animation: spin 1s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .badges {
    position: absolute;
    left: 4px;
    bottom: 4px;
    display: flex;
    gap: 3px;
  }
  .b {
    font-size: 9px;
    line-height: 14px;
    padding: 0 4px;
    border-radius: 3px;
    background: rgba(20, 18, 17, 0.78);
    color: #d8d0c8;
    letter-spacing: 0.02em;
  }
  .b.print {
    color: var(--accent-2);
  }
  .b.ai {
    color: #cbb8f2;
  }
  .flag {
    position: absolute;
    top: 4px;
    right: 4px;
    font-size: 9px;
    padding: 0 5px;
    border-radius: 3px;
    background: rgba(20, 18, 17, 0.85);
  }
  .clip {
    position: absolute;
    left: 4px;
    top: 4px;
    font-size: 9.5px;
    padding: 0 5px;
    border-radius: 3px;
    background: rgba(20, 18, 17, 0.85);
    color: #d8d0c8;
  }
  .hearts {
    position: absolute;
    right: 4px;
    bottom: 3px;
    font-size: 10px;
    letter-spacing: -0.5px;
    color: var(--accent-2);
    text-shadow: 0 0 3px #000;
  }
  .mark {
    position: absolute;
    top: 4px;
    left: 4px;
    width: 15px;
    height: 15px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    font-size: 9px;
    background: rgba(20, 18, 17, 0.85);
  }
  .mark.select {
    color: var(--ok);
  }
  .mark.reject {
    color: var(--bad);
  }
  .tile.rejected .img {
    opacity: 0.45;
  }
  .cap {
    padding: 4px 2px 0;
    display: flex;
    flex-direction: column;
    line-height: 1.25;
    min-width: 0;
  }
  .name,
  .date {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name {
    font-size: 11.5px;
  }
  .date {
    font-size: 10.5px;
    color: var(--muted);
  }
</style>
