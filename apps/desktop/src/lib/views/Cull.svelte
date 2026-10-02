<script lang="ts">
  // The rejecticator: go through what is on the card and delete the failed shots from
  // the card itself, without importing anything. Marking is free; nothing leaves the
  // card until the list of paths has been shown and its button pressed.
  import { api, human, type CullFile, type GroupView } from "../api";
  import { store } from "../state.svelte";
  import Thumb from "./Thumb.svelte";
  import ConfirmPurge from "./library/ConfirmPurge.svelte";

  let size = $state(130);
  let marked = $state<Set<number>>(new Set());
  let current = $state(0);
  let big = $state<string | null>(null);
  let pending = $state<CullFile[] | null>(null);
  let busy = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);
  let grid = $state<HTMLDivElement | null>(null);

  const all = $derived(store.scan?.groups.filter((g) => g.kind === "raw" || g.kind === "image" || g.kind === "video") ?? []);
  /** The last look: only what was marked when it began, so sparing one does not shuffle the rest. */
  let reviewing = $state<Set<number> | null>(null);
  const shots = $derived(reviewing ? all.filter((g) => reviewing!.has(g.id)) : all);
  const cur = $derived<GroupView | undefined>(shots[Math.min(current, shots.length - 1)]);
  const markedBytes = $derived(all.filter((g) => marked.has(g.id)).reduce((n, g) => n + g.bytes, 0));

  // The large picture for whichever shot is under the cursor. The old one stays up until
  // the new one is ready, so flicking through does not flash.
  $effect(() => {
    const g = cur;
    if (!g || g.kind === "video") {
      big = null;
      return;
    }
    let stale = false;
    api
      .thumbnail(g.id, 2048)
      .then((p) => {
        if (!stale) big = p ? api.fileUrl(p) : null;
      })
      .catch(() => {
        if (!stale) big = null;
      });
    return () => {
      stale = true;
    };
  });

  /** Proxies being built ahead of time, if asked for. */
  let proxies = $state<{ done: number; total: number } | null>(null);
  let proxied = $state(false);

  $effect(() => {
    const off = api.on<{ done: number; total: number; finished: boolean }>("cull-proxies", (p) => {
      if (!proxies) return;
      proxies = p.finished ? null : { done: p.done, total: p.total };
      if (p.finished) proxied = true;
    });
    return () => {
      off.then((f) => f());
      api.cullProxiesCancel();
    };
  });

  async function makeProxies() {
    const ids = all.filter((g) => g.kind !== "video").map((g) => g.id);
    if (!ids.length) return;
    proxies = { done: 0, total: ids.length };
    await api.cullProxies(ids).catch((e) => {
      proxies = null;
      error = String(e);
    });
  }
  function stopProxies() {
    api.cullProxiesCancel();
    proxies = null;
  }

  // Once proxies exist, have the browser hold the next few decoded, so a key press is instant.
  $effect(() => {
    if (!proxied) return;
    for (const g of shots.slice(current + 1, current + 4).concat(shots.slice(Math.max(0, current - 1), current))) {
      if (g.kind === "video") continue;
      api
        .thumbnail(g.id, 2048)
        .then((p) => {
          if (p) new Image().src = api.fileUrl(p);
        })
        .catch(() => {});
    }
  });

  /** The last tile marked by a click, so shift-click can repeat it over a range. */
  let anchor = $state<{ index: number; on: boolean } | null>(null);

  function setMarked(ids: number[], on: boolean) {
    const next = new Set(marked);
    for (const id of ids) on ? next.add(id) : next.delete(id);
    marked = next;
  }

  function toggle(i: number, e?: MouseEvent) {
    const g = shots[i];
    if (!g) return;
    if (e?.shiftKey && anchor) {
      const [lo, hi] = anchor.index < i ? [anchor.index, i] : [i, anchor.index];
      setMarked(
        shots.slice(lo, hi + 1).map((x) => x.id),
        anchor.on,
      );
      return;
    }
    const on = !marked.has(g.id);
    setMarked([g.id], on);
    anchor = { index: i, on };
  }

  function go(i: number) {
    current = Math.max(0, Math.min(shots.length - 1, i));
    grid?.querySelector(`[data-i="${current}"]`)?.scrollIntoView({ block: "nearest" });
  }

  function key(e: KeyboardEvent) {
    if (pending || e.metaKey || e.ctrlKey || (e.target as HTMLElement)?.tagName === "INPUT") return;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") go(current + 1);
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") go(current - 1);
    else if (e.key === "x" || e.key === "r" || e.key === "Backspace" || e.key === "Delete") {
      // Mark and move on, so a run of duds is a run of keypresses.
      const was = cur ? marked.has(cur.id) : false;
      toggle(current);
      if (!was) go(current + 1);
    } else if (e.key === "u" && cur) setMarked([cur.id], false);
    else return;
    e.preventDefault();
  }

  function startReview() {
    reviewing = new Set(marked);
    anchor = null;
    current = 0;
  }
  function stopReview() {
    reviewing = null;
    anchor = null;
    current = 0;
  }

  async function review() {
    busy = true;
    error = null;
    try {
      pending = await api.cullFiles([...marked]);
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function execute() {
    busy = true;
    try {
      const r = await api.cullDelete([...marked]);
      note = `Deleted ${r.removed} ${r.removed === 1 ? "file" : "files"} · ${human(r.bytes)} freed on ${store.scan?.source.label}.`;
      error = r.failed.length ? r.failed.map(([p, why]) => `${p}: ${why}`).join("\n") : null;
      const gone = new Set(r.groups);
      marked = new Set([...marked].filter((id) => !gone.has(id)));
      pending = null;
      reviewing = null;
      anchor = null;
      await store.reloadScan();
      go(current);
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function when(g: GroupView) {
    return g.captured_at ? g.captured_at.replace("T", " ") : "undated";
  }
</script>

<svelte:window onkeydown={key} />

{#if store.scan}
  <div class="cull">
    {#if pending}
      <ConfirmPurge
        title="Delete {marked.size} {marked.size === 1 ? 'shot' : 'shots'} from {store.scan.source.label}"
        files={pending}
        hardDelete={true}
        {busy}
        note="These are deleted from the card itself and were never imported. Every file of each shot goes — the RAW, its camera JPEG and any sidecars."
        onexecute={execute}
        oncancel={() => (pending = null)}
      />
    {/if}

    <div class="bar row spread">
      <div class="row">
        <h2>Rejecticator{reviewing ? " — last look" : ""}</h2>
        <span class="muted small">
          {store.scan.source.label} · {all.length} shots ·
          <span class:bad={marked.size > 0}>{marked.size} marked{marked.size ? ` · ${human(markedBytes)}` : ""}</span>
        </span>
      </div>
      <div class="row">
        <span class="muted small">←/→ move · X mark · U unmark · shift-click a range</span>
        <input type="range" min="90" max="240" step="10" bind:value={size} title="Thumbnail size" />
        {#if proxies}
          <button class="mini" onclick={stopProxies} title="Stop building proxies">proxies {proxies.done}/{proxies.total} ✕</button>
        {:else}
          <button class="mini" onclick={makeProxies} title="Build every preview now, so flicking through is instant">{proxied ? "proxies ready" : "make proxies"}</button>
        {/if}
        <button class="mini" disabled={!marked.size} onclick={() => (marked = new Set())}>clear marks</button>
        {#if reviewing}
          <button onclick={stopReview}>← Back to all</button>
          <button class="danger" disabled={busy || !marked.size} onclick={review}>Delete {marked.size}…</button>
        {:else}
          <button class="primary" disabled={!marked.size} onclick={startReview}>Review {marked.size}…</button>
        {/if}
      </div>
    </div>

    {#if reviewing}
      <p class="small msg muted">Only the shots you marked. Unmark any you want to keep, then Delete shows every file path before anything goes.</p>
    {:else if note}<p class="small msg">{note}</p>{/if}
    {#if error}<p class="bad small msg pre">{error}</p>{/if}

    <div class="panes">
      <div class="grid" style="--w: {size}px" bind:this={grid}>
        {#each shots as g, i (g.id)}
          <div
            class="tile"
            class:doomed={marked.has(g.id)}
            class:current={i === current}
            data-i={i}
            role="button"
            tabindex="-1"
            onclick={(e) => {
              current = i;
              if (e.shiftKey) toggle(i, e);
            }}
            ondblclick={() => toggle(i)}
            onkeydown={() => {}}
            title={g.rel}
          >
            <Thumb group={g.id} />
            <button
              class="mark"
              title={marked.has(g.id) ? "Keep" : "Mark for deletion"}
              onclick={(e) => {
                e.stopPropagation();
                current = i;
                toggle(i, e);
              }}>✕</button
            >
            <span class="name small">{g.name}</span>
          </div>
        {/each}
        {#if !shots.length}<p class="muted small">Nothing left on this source.</p>{/if}
      </div>

      <div class="stage" class:doomed={cur && marked.has(cur.id)}>
        {#if cur}
          {#if big}
            <img src={big} alt="" draggable="false" />
          {:else}
            <span class="muted small">{cur.kind === "video" ? "video — no preview here" : "loading…"}</span>
          {/if}
          <div class="info row spread">
            <span class="small">
              <b>{cur.name}</b>{cur.attachments.length ? ` + ${cur.attachments.join(", ")}` : ""}
              <span class="muted"> · {when(cur)}{cur.iso ? ` · ISO ${cur.iso}` : ""} · {human(cur.bytes)}</span>
            </span>
            <button class={marked.has(cur.id) ? "" : "danger"} onclick={() => toggle(current)}>
              {marked.has(cur.id) ? "Keep (U)" : "Mark ✕ (X)"}
            </button>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .cull {
    position: relative;
    height: 100%;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr);
  }
  .bar {
    grid-row: 1;
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .msg {
    margin: 6px 14px 0;
  }
  .pre {
    white-space: pre-wrap;
  }
  .panes {
    grid-row: 3;
    display: grid;
    grid-template-columns: minmax(260px, 38%) minmax(0, 1fr);
    min-height: 0;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--w), 1fr));
    gap: 6px;
    align-content: start;
    padding: 10px;
    overflow: auto;
    border-right: 1px solid var(--line);
  }
  .tile {
    position: relative;
    padding: 3px;
    border: 2px solid transparent;
    border-radius: 6px;
    background: var(--bg-2);
    min-width: 0;
  }
  .tile.current {
    border-color: var(--accent);
  }
  .tile.doomed {
    border-color: var(--bad);
  }
  .tile.doomed.current {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .tile.doomed :global(img) {
    opacity: 0.4;
  }
  .mark {
    position: absolute;
    top: 6px;
    right: 6px;
    padding: 0 6px;
    font-size: 11px;
    line-height: 18px;
    background: rgba(20, 18, 17, 0.8);
    border-color: transparent;
    color: var(--muted);
  }
  .tile.doomed .mark {
    background: var(--bad);
    color: #180e0b;
  }
  .name {
    display: block;
    padding-top: 2px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .stage {
    position: relative;
    display: grid;
    place-items: center;
    min-height: 0;
    min-width: 0;
    padding: 10px 10px 46px;
    box-shadow: inset 0 0 0 0 var(--bad);
  }
  .stage.doomed {
    box-shadow: inset 0 0 0 3px var(--bad);
  }
  .stage img {
    max-width: 100%;
    max-height: 100%;
    min-height: 0;
    object-fit: contain;
  }
  .info {
    position: absolute;
    left: 10px;
    right: 10px;
    bottom: 8px;
  }
  .danger {
    background: var(--bad);
    color: #180e0b;
    border-color: var(--bad);
  }
  .small {
    font-size: 11.5px;
  }
</style>
