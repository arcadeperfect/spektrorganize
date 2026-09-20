<script lang="ts">
  // A drag handle between two panes. The view owns the width; this reports a new
  // one as the pointer moves. `side` says which pane is being sized: "left" for a
  // handle to the right of it, "right" for one to its left. Double-click resets;
  // arrow keys nudge, so it works without a mouse.
  import { savePaneWidth } from "../panes";

  interface Props {
    width: number;
    min?: number;
    max?: number;
    /** Which pane the width belongs to, relative to this handle. */
    side?: "left" | "right";
    /** Remembered under this name when given. */
    name?: string;
    /** Width a double-click goes back to. */
    reset?: number;
  }

  let { width = $bindable(), min = 180, max = 900, side = "left", name, reset }: Props = $props();

  let dragging = $state(false);
  let startX = 0;
  let startW = 0;

  function set(w: number) {
    width = Math.round(Math.min(max, Math.max(min, w)));
  }

  function remember() {
    if (name) savePaneWidth(name, width);
  }

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    dragging = true;
    startX = e.clientX;
    startW = width;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function move(e: PointerEvent) {
    if (!dragging) return;
    const d = e.clientX - startX;
    set(startW + (side === "left" ? d : -d));
  }

  function up(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    remember();
  }

  function key(e: KeyboardEvent) {
    const step = e.shiftKey ? 40 : 10;
    if (e.key === "ArrowLeft") set(width + (side === "left" ? -step : step));
    else if (e.key === "ArrowRight") set(width + (side === "left" ? step : -step));
    else return;
    e.preventDefault();
    remember();
  }
</script>

<!-- A button rather than a bare role="separator": it focuses and takes keys without
     fighting the a11y rules. -->
<button
  type="button"
  class="split"
  class:dragging
  aria-label="Resize pane ({width}px; arrow keys adjust)"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onkeydown={key}
  ondblclick={() => {
    if (reset === undefined) return;
    set(reset);
    remember();
  }}
  title="Drag to resize{reset === undefined ? '' : ' · double-click to reset'}"
></button>

<style>
  .split {
    position: relative;
    padding: 0;
    border: 0;
    border-radius: 0;
    width: 6px;
    cursor: col-resize;
    align-self: stretch;
    /* The line sits in the middle of a wider grab area. */
    background:
      linear-gradient(var(--line), var(--line)) center / 1px 100% no-repeat;
    touch-action: none;
  }
  .split:hover,
  .split:focus-visible,
  .split.dragging {
    background: linear-gradient(var(--accent), var(--accent)) center / 2px 100% no-repeat;
    outline: none;
  }
</style>
