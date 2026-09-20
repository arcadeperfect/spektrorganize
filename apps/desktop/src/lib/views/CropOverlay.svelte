<script lang="ts">
  // The crop rectangle, drawn over the preview. Coordinates are fractions of
  // the frame, so they survive zooming, a re-render at another size, and the
  // trip to the catalog. Left-drag is free for us: panzoom pans on middle-drag
  // or Alt+left-drag, so the two never fight.
  import type { Crop } from "../api";

  interface Props {
    crop: Crop | null;
    /** Width / height to hold while dragging; null lets the rectangle be any shape. */
    aspect: number | null;
    /** The view's zoom, so handles stay the same size on screen. */
    zoom?: number;
    onchange: (crop: Crop) => void;
  }

  let { crop, aspect, zoom = 1, onchange }: Props = $props();

  const WHOLE: Crop = { x: 0, y: 0, w: 1, h: 1 };
  const rect = $derived(crop ?? WHOLE);
  const MIN = 0.02;

  let box: HTMLDivElement | undefined = $state();
  /** What the pointer is doing: a handle name, "move", or "new". */
  let drag = $state<string | null>(null);
  let start = { x: 0, y: 0, rect: WHOLE };

  const HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as const;

  /** Pointer position as a fraction of the frame. */
  function at(e: PointerEvent): { x: number; y: number } {
    const r = box?.getBoundingClientRect();
    if (!r || !r.width || !r.height) return { x: 0, y: 0 };
    return { x: (e.clientX - r.left) / r.width, y: (e.clientY - r.top) / r.height };
  }

  function begin(e: PointerEvent, what: string) {
    if (e.button !== 0 || e.altKey) return; // Alt+drag belongs to panning
    e.preventDefault();
    e.stopPropagation();
    drag = what;
    start = { ...at(e), rect: { ...rect } };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    if (what === "new") onchange({ x: start.x, y: start.y, w: MIN, h: MIN });
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    const p = at(e);
    if (drag === "move") {
      const w = start.rect.w;
      const h = start.rect.h;
      onchange({
        x: clamp(start.rect.x + (p.x - start.x), 0, 1 - w),
        y: clamp(start.rect.y + (p.y - start.y), 0, 1 - h),
        w,
        h,
      });
      return;
    }
    // Resizing: work out the new edges, then fix the aspect if one is held.
    let { x, y, w, h } = start.rect;
    let [x0, y0, x1, y1] = [x, y, x + w, y + h];
    const d = drag === "new" ? "se" : drag;
    if (drag === "new") {
      [x0, y0, x1, y1] = [Math.min(start.x, p.x), Math.min(start.y, p.y), Math.max(start.x, p.x), Math.max(start.y, p.y)];
    } else {
      if (d.includes("w")) x0 = Math.min(p.x, x1 - MIN);
      if (d.includes("e")) x1 = Math.max(p.x, x0 + MIN);
      if (d.includes("n")) y0 = Math.min(p.y, y1 - MIN);
      if (d.includes("s")) y1 = Math.max(p.y, y0 + MIN);
    }
    x0 = clamp(x0, 0, 1);
    y0 = clamp(y0, 0, 1);
    x1 = clamp(x1, 0, 1);
    y1 = clamp(y1, 0, 1);
    let next = { x: x0, y: y0, w: Math.max(MIN, x1 - x0), h: Math.max(MIN, y1 - y0) };
    if (aspect) next = fitAspect(next, d, aspect);
    onchange(next);
  }

  function end(e: PointerEvent) {
    if (!drag) return;
    drag = null;
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
  }

  function clamp(v: number, lo: number, hi: number) {
    return Math.min(hi, Math.max(lo, v));
  }

  /**
   * Hold the ratio while resizing. `aspect` is in frame fractions, so the
   * caller has already folded in the picture's own shape.
   */
  function fitAspect(c: Crop, from: string, aspect: number): Crop {
    let { x, y, w, h } = c;
    // Drive off whichever edge the pointer moved, then pull the other to match.
    if (from === "n" || from === "s") w = h * aspect;
    else h = w / aspect;
    if (w > 1) ((w = 1), (h = w / aspect));
    if (h > 1) ((h = 1), (w = h * aspect));
    // Grow from the anchored corner, not the origin.
    if (from.includes("w")) x = c.x + c.w - w;
    if (from.includes("n")) y = c.y + c.h - h;
    if (from === "n" || from === "s") x = c.x + c.w / 2 - w / 2;
    if (from === "e" || from === "w") y = c.y + c.h / 2 - h / 2;
    return { x: clamp(x, 0, 1 - w), y: clamp(y, 0, 1 - h), w, h };
  }

  const pct = (v: number) => `${v * 100}%`;
</script>

<div
  class="crop"
  bind:this={box}
  role="presentation"
  onpointerdown={(e) => begin(e, "new")}
  onpointermove={move}
  onpointerup={end}
  onpointercancel={end}
>
  <!-- Everything outside the rectangle, dimmed. -->
  <div class="shade" style="clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%, 0 0, {pct(rect.x)} {pct(rect.y)}, {pct(rect.x)} {pct(rect.y + rect.h)}, {pct(rect.x + rect.w)} {pct(rect.y + rect.h)}, {pct(rect.x + rect.w)} {pct(rect.y)}, {pct(rect.x)} {pct(rect.y)})"></div>

  <div
    class="rect"
    style="left: {pct(rect.x)}; top: {pct(rect.y)}; width: {pct(rect.w)}; height: {pct(rect.h)}"
    role="presentation"
    onpointerdown={(e) => begin(e, "move")}
    onpointermove={move}
    onpointerup={end}
    onpointercancel={end}
  >
    <!-- Thirds, the usual framing guide. -->
    <div class="thirds"></div>
    {#each HANDLES as h (h)}
      <button
        class="h {h}"
        style="transform: scale({1 / Math.max(zoom, 0.2)})"
        aria-label="Resize {h}"
        onpointerdown={(e) => begin(e, h)}
        onpointermove={move}
        onpointerup={end}
        onpointercancel={end}
      ></button>
    {/each}
  </div>
</div>

<style>
  .crop {
    position: absolute;
    inset: 0;
    cursor: crosshair;
    touch-action: none;
  }
  .shade {
    position: absolute;
    inset: 0;
    background: rgba(10, 9, 8, 0.62);
    pointer-events: none;
  }
  .rect {
    position: absolute;
    outline: 1px solid rgba(255, 255, 255, 0.9);
    cursor: move;
  }
  .thirds {
    position: absolute;
    inset: 0;
    pointer-events: none;
    background:
      linear-gradient(to right, transparent 33.33%, rgba(255, 255, 255, 0.28) 33.33%, rgba(255, 255, 255, 0.28) calc(33.33% + 1px), transparent calc(33.33% + 1px)),
      linear-gradient(to right, transparent 66.66%, rgba(255, 255, 255, 0.28) 66.66%, rgba(255, 255, 255, 0.28) calc(66.66% + 1px), transparent calc(66.66% + 1px)),
      linear-gradient(to bottom, transparent 33.33%, rgba(255, 255, 255, 0.28) 33.33%, rgba(255, 255, 255, 0.28) calc(33.33% + 1px), transparent calc(33.33% + 1px)),
      linear-gradient(to bottom, transparent 66.66%, rgba(255, 255, 255, 0.28) 66.66%, rgba(255, 255, 255, 0.28) calc(66.66% + 1px), transparent calc(66.66% + 1px));
  }
  .h {
    position: absolute;
    width: 12px;
    height: 12px;
    padding: 0;
    border: 1px solid rgba(20, 18, 17, 0.8);
    border-radius: 2px;
    background: #fff;
  }
  .h:hover {
    background: var(--accent-2);
  }
  .nw {
    left: -6px;
    top: -6px;
    cursor: nwse-resize;
  }
  .n {
    left: calc(50% - 6px);
    top: -6px;
    cursor: ns-resize;
  }
  .ne {
    right: -6px;
    top: -6px;
    cursor: nesw-resize;
  }
  .e {
    right: -6px;
    top: calc(50% - 6px);
    cursor: ew-resize;
  }
  .se {
    right: -6px;
    bottom: -6px;
    cursor: nwse-resize;
  }
  .s {
    left: calc(50% - 6px);
    bottom: -6px;
    cursor: ns-resize;
  }
  .sw {
    left: -6px;
    bottom: -6px;
    cursor: nesw-resize;
  }
  .w {
    left: -6px;
    top: calc(50% - 6px);
    cursor: ew-resize;
  }
</style>
