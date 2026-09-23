<script lang="ts">
  // Full-screen viewer over the grid: one photo at a time, arrow keys to walk
  // the current listing, Blender navigation for zoom and pan, and the two
  // stages a step away. Shows the largest preview the catalog has; the develop
  // and print stages render their own.
  import { catalog, api, duration as durationText, looks as looksApi, type FrameRef, type RenderInfo } from "../../api";
  import { library as lib } from "../../library.svelte";
  import VideoExport from "./VideoExport.svelte";
  import { develop } from "../../photo.svelte";
  import { store } from "../../state.svelte";
  import Viewport from "../Viewport.svelte";
  import { quality, QUALITIES, type Quality } from "../../quality.svelte";

  const index = $derived(lib.loupe ?? 0);
  const asset = $derived(lib.item(index));
  let src = $state<string | null>(null);
  let zoom = $state(1);
  const cache = new Map<number, string | null>();
  function onZoom(z: number) {
    zoom = z;
  }

  async function load(id: number): Promise<string | null> {
    if (cache.has(id)) return cache.get(id)!;
    const p = await catalog.preview(id).catch(() => null);
    cache.set(id, p);
    return p;
  }

  // Show this photo, then quietly fetch its neighbours so paging is instant.
  $effect(() => {
    const a = asset;
    if (!a) {
      lib.ensure(Math.max(0, index - 5), Math.min(lib.total, index + 5));
      return;
    }
    src = null;
    fullFrame = null;
    if (a.renders) loadPrints(a.id);
    else ((prints = []), (shown = -1));
    if (a.kind === "video") loadClip(a.id);
    else clip = null;
    // At the chosen quality, so high or native carries from photo to photo.
    applyQuality(a.id);
    for (const j of [index - 1, index + 1]) {
      const n = lib.item(j);
      if (n) load(n.id);
    }
  });

  /**
   * The previews come from the camera JPEG or the RAW's embedded preview, which
   * runs out before a deep zoom does. This decodes the file itself at its own
   * size — a second or so for a RAW, so it is on request.
   */
  let qualBusy = $state(false);
  /** The photo decoded at its own size, for native quality. */
  let fullFrame = $state<FrameRef | null>(null);

  /**
   * The photo's prints, so the viewer can flick between the camera rendition and
   * each look. `shown` is an index into `prints`, or -1 for the original.
   */
  let prints = $state<RenderInfo[]>([]);
  let shown = $state(-1);

  /** The video file behind a clip, for the player. */
  let clip = $state<number | null>(null);
  let rendering = $state(false);
  let player = $state<HTMLVideoElement | undefined>();

  async function loadClip(id: number) {
    clip = null;
    try {
      const d = await catalog.asset(id);
      const f = d.files.find((x) => x.kind === "video") ?? d.files[0];
      if (f && lib.item(index)?.id === id) clip = f.id;
    } catch {
      // Without the file id there is nothing to play; the poster frame still shows.
    }
  }

  async function loadPrints(id: number) {
    prints = [];
    shown = -1;
    try {
      const d = await catalog.asset(id);
      if (lib.item(index)?.id === id) prints = d.renders.filter((r) => r.kind === "jpeg" && r.exists);
    } catch {
      // No prints to offer is not an error worth showing here.
    }
  }

  async function show(i: number) {
    const a = asset;
    if (!a) return;
    if (i < 0) {
      shown = -1;
      await applyQuality(a.id);
      return;
    }
    const r = prints[i];
    if (!r) return;
    shown = i;
    src = null;
    const p = await catalog.renderPreview(r.id, 2048).catch(() => null);
    if (lib.item(index)?.id === a.id && shown === i) src = p ? api.fileUrl(p) : null;
  }

  /** Step through original → print 1 → print 2 → … with [ and ]. */
  function cycle(d: number) {
    if (!prints.length) return;
    const n = prints.length + 1;
    const next = ((shown + 1 + d + n) % n) - 1;
    show(next);
  }
  /**
   * Show the photo at the chosen quality. Low is the catalog's 1024 px preview,
   * high a 4096 px one made from the same source, native a decode of the file
   * itself — a second or so for a RAW, which is why it is a choice.
   */
  async function applyQuality(id: number) {
    const q = quality.value;
    fullFrame = null;
    src = null;
    if (q === "native") {
      qualBusy = true;
      try {
        const raw = await looksApi.rawGet(id);
        const info = await looksApi.input(id);
        const px = Math.max(info.width ?? 0, info.height ?? 0) || 16384;
        const f = await looksApi.developFrame(id, raw, px);
        if (lib.item(index)?.id === id && quality.value === "native") fullFrame = f;
      } catch (e) {
        lib.error = String(e);
      } finally {
        qualBusy = false;
      }
      return;
    }
    const p = q === "high" ? await catalog.previewPx(id, 4096).catch(() => null) : await load(id);
    if (lib.item(index)?.id === id && quality.value === q) src = p ? api.fileUrl(p) : null;
  }

  function setQuality(q: Quality) {
    quality.set(q);
    if (asset && shown === -1) applyQuality(asset.id);
  }

  function go(delta: number) {
    const to = Math.min(lib.total - 1, Math.max(0, index + delta));
    if (to === index) return;
    lib.ensure(Math.max(0, to - 5), Math.min(lib.total, to + 5));
    lib.loupe = to;
    lib.cursor = to;
    const a = lib.item(to);
    if (a) lib.click(to, a.id, { shiftKey: false, metaKey: false, ctrlKey: false });
  }

  function close() {
    lib.scrollTo = index;
    lib.loupe = null;
  }

  function openStage(view: "develop" | "print") {
    const a = asset;
    if (!a) return;
    develop.setPhoto(a.id);
    lib.loupe = null;
    store.view = view;
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
    if (asset && lib.ratingKey(e, [asset.id])) {
      e.preventDefault();
      return;
    }
    switch (e.key) {
      case " ":
        // On a clip, space is play/pause — the usual meaning — and Esc still closes.
        if (player) {
          e.preventDefault();
          if (player.paused) player.play();
          else player.pause();
          break;
        }
        e.preventDefault();
        close();
        break;
      case "Escape":
      case "Enter":
        e.preventDefault();
        close();
        break;
      case "ArrowLeft":
        e.preventDefault();
        go(-1);
        break;
      case "ArrowRight":
        e.preventDefault();
        go(1);
        break;
      case "d":
        openStage("develop");
        break;
      case "q":
        if (asset) lib.queueAdd([asset.id]);
        break;
      case "e":
        if (asset) lib.askExport([asset.id]);
        break;
      case "f": {
        // F cycles the quality: low → high → native → low.
        const order: Quality[] = ["low", "high", "native"];
        setQuality(order[(order.indexOf(quality.value) + 1) % order.length]);
        break;
      }
      case "[":
        e.preventDefault();
        cycle(-1);
        break;
      case "]":
        e.preventDefault();
        cycle(1);
        break;
      case "p":
        openStage("print");
        break;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if rendering && asset}
  <VideoExport {asset} onclose={() => (rendering = false)} />
{/if}

<div class="loupe">
  <div class="bar row spread">
    <div class="row">
      <button class="ghost" onclick={close} title="Back to the grid (space or Esc)">← Grid</button>
      <span class="name">{asset?.name ?? ""}</span>
      <span class="muted small">{index + 1} / {lib.total}</span>
      {#if asset?.duration}<span class="muted small mono">{durationText(asset.duration)}</span>{/if}
      {#if prints.length}
        <span class="versions" title="[ and ] step through the prints">
          <button class:on={shown === -1} onclick={() => show(-1)}>camera</button>
          {#each prints as r, i (r.id)}
            <button class:on={shown === i} onclick={() => show(i)} title={r.preset_name ?? "print"}>
              {r.preset_name ?? `print ${i + 1}`}
            </button>
          {/each}
        </span>
      {/if}
      {#if asset}
        <span class="hearts" title="1–5 rate, 0 clears">
          {#each [1, 2, 3, 4, 5] as n (n)}
            <button class:on={asset.rating >= n} onclick={() => lib.rate([asset.id], asset.rating === n ? 0 : n)} aria-label="{n} hearts">♥</button>
          {/each}
        </span>
        {#if asset.flag === "select"}<span class="mk ok" title="Selected (S)">✓</span>{/if}
        {#if asset.flag === "reject"}<span class="mk bad" title="Rejected (R)">✕</span>{/if}
      {/if}
      {#if zoom !== 1}<span class="muted small mono">{Math.round(zoom * 100)}%</span>{/if}
    </div>
    <div class="row">
      <button class="nav" disabled={index === 0} onclick={() => go(-1)} title="Previous (←)">‹</button>
      <button class="nav" disabled={index >= lib.total - 1} onclick={() => go(1)} title="Next (→)">›</button>
      <button onclick={() => openStage("develop")} title="Develop this photo (D)">Develop…</button>
      <button onclick={() => openStage("print")} title="Print look for this photo (P)">Print look…</button>
      <button onclick={() => asset && lib.queueAdd([asset.id])} title="Add to the export queue (Q)">Queue</button>
      {#if asset?.kind === "video"}
        <button onclick={() => (rendering = true)} title="Render this clip through a look">Render…</button>
      {/if}
      <button onclick={() => asset && lib.askExport([asset.id])} title="Export this photo (E)">Export…</button>
      <span class="qual" role="group" aria-label="Quality (F cycles)">
        {#each QUALITIES as q (q.id)}
          <button class:on={quality.value === q.id} disabled={qualBusy} onclick={() => setQuality(q.id)} title="{q.hint} (F cycles)">{q.label}</button>
        {/each}
        {#if qualBusy}<span class="muted small">decoding…</span>{/if}
      </span>
    </div>
  </div>
  <div
    class="stage"
    role="img"
    aria-label="Photo"
    oncontextmenu={(e) => {
      if (!asset) return;
      e.preventDefault();
      lib.menu = { x: e.clientX, y: e.clientY, ids: [asset.id] };
    }}
  >
    <div class="view">
      {#if asset?.kind === "video"}
        {#if clip !== null}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video bind:this={player} src={catalog.clipUrl(clip)} poster={src ?? undefined} controls autoplay playsinline></video>
        {:else}
          <span class="muted">Opening the clip…</span>
        {/if}
      {:else if src || fullFrame}
        <Viewport frame={fullFrame} src={fullFrame ? null : src} key={asset?.id} onzoom={(z) => onZoom(z)} />
      {:else}
        <span class="muted">Loading…</span>
      {/if}
    </div>
  </div>
</div>

<style>
  .loupe {
    position: absolute;
    inset: 0;
    z-index: 15;
    background: var(--bg);
    display: grid;
    grid-template-rows: 44px 1fr;
  }
  .bar {
    padding: 0 12px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .name {
    font-weight: 600;
  }
  .hearts button {
    background: transparent;
    border: 0;
    padding: 0 1px;
    color: #4a433e;
    font-size: 14px;
  }
  .hearts button.on {
    color: var(--accent-2);
  }
  .mk {
    font-size: 12px;
  }
  .mk.ok {
    color: var(--ok);
  }
  .mk.bad {
    color: var(--bad);
  }
  .versions {
    display: flex;
    gap: 2px;
  }
  .versions button {
    font-size: 11px;
    padding: 1px 7px;
  }
  button.on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
  .nav {
    width: 34px;
    font-size: 16px;
    line-height: 1;
  }
  .stage {
    position: relative;
    overflow: hidden;
    background: #0e0d0c;
  }
  .view {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .view video {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .small {
    font-size: 11.5px;
  }
  .qual {
    display: inline-flex;
    gap: 2px;
    align-items: center;
  }
</style>
