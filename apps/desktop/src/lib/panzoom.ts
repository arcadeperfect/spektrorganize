// Blender-style navigation for image views, as a Svelte action.
//
// Mouse: middle-drag pans, Ctrl+middle-drag zooms, wheel zooms. Blender's
// "emulate 3 button mouse" is honoured, so Alt/Option+left-drag pans too.
// Trackpad: two-finger swipe pans (never zooms), pinch zooms.
// Keys: Home fits the view, 1 is 1:1, +/- zoom, 0 fits as well.
//
// The action transforms its element's first child, so the element itself is
// the viewport. Pass `key` (e.g. the photo id): the view refits when it
// changes, and holds still when only the image behind it is re-rendered.

export interface PanZoomOptions {
  /** Refit whenever this changes. */
  key?: unknown;
  /** Reports the zoom factor (1 = fit to view). */
  onzoom?: (zoom: number) => void;
  /** Whether the keys work anywhere ("always", for a full-screen view) or
   * only while the pointer is over this view ("hover"). */
  keys?: "always" | "hover";
}

const MIN = 0.05;
const MAX = 32;

export function panzoom(node: HTMLElement, options: PanZoomOptions = {}) {
  let opts = options;
  let key = options.key;
  let zoom = 1;
  let x = 0;
  let y = 0;
  let dragging: "pan" | "zoom" | null = null;
  let hovered = false;
  let lastX = 0;
  let lastY = 0;

  const target = () => node.firstElementChild as HTMLElement | null;

  function apply() {
    const el = target();
    if (el) {
      el.style.transform = `translate(${x}px, ${y}px) scale(${zoom})`;
      el.style.transformOrigin = "center center";
      el.style.willChange = "transform";
    }
    node.style.cursor = zoom > 1 ? "grab" : "default";
    opts.onzoom?.(zoom);
  }

  function fit() {
    zoom = 1;
    x = 0;
    y = 0;
    apply();
  }

  /** Zoom about a point in the element's box, so that point stays put. */
  function zoomAt(factor: number, clientX?: number, clientY?: number) {
    const next = Math.min(MAX, Math.max(MIN, zoom * factor));
    const r = node.getBoundingClientRect();
    const cx = (clientX ?? r.left + r.width / 2) - (r.left + r.width / 2);
    const cy = (clientY ?? r.top + r.height / 2) - (r.top + r.height / 2);
    const k = next / zoom;
    x = cx - (cx - x) * k;
    y = cy - (cy - y) * k;
    zoom = next;
    apply();
  }

  /**
   * Trackpad or mouse wheel? A two-finger swipe gives sideways movement, small
   * or fractional steps; a wheel gives one big vertical notch. Momentum can look
   * like either, so once a swipe is seen the view stays in trackpad mode for a
   * moment — otherwise a single run of the fingers would pan, then zoom.
   */
  let trackpadUntil = 0;
  function isTrackpad(e: WheelEvent): boolean {
    const now = performance.now();
    if (e.deltaX !== 0 || !Number.isInteger(e.deltaY) || Math.abs(e.deltaY) < 40) {
      trackpadUntil = now + 1200;
      return true;
    }
    return now < trackpadUntil;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    // Pinch on a trackpad arrives as a wheel event with ctrlKey set.
    if (e.ctrlKey) {
      trackpadUntil = performance.now() + 1200;
      zoomAt(Math.exp(-e.deltaY * 0.01), e.clientX, e.clientY);
      return;
    }
    // Two fingers pan, as everywhere else on macOS; the mouse wheel zooms, as in Blender.
    if (isTrackpad(e) || e.shiftKey) {
      x -= e.deltaX;
      y -= e.deltaY;
      apply();
      return;
    }
    zoomAt(e.deltaY < 0 ? 1.15 : 1 / 1.15, e.clientX, e.clientY);
  }

  function onPointerDown(e: PointerEvent) {
    const middle = e.button === 1;
    const emulated = e.button === 0 && e.altKey;
    if (!middle && !emulated) return;
    e.preventDefault();
    dragging = e.ctrlKey || e.metaKey ? "zoom" : "pan";
    lastX = e.clientX;
    lastY = e.clientY;
    node.setPointerCapture(e.pointerId);
    node.style.cursor = dragging === "pan" ? "grabbing" : "ns-resize";
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    lastX = e.clientX;
    lastY = e.clientY;
    if (dragging === "pan") {
      x += dx;
      y += dy;
      apply();
    } else {
      zoomAt(Math.exp(-dy * 0.01));
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = null;
    node.releasePointerCapture?.(e.pointerId);
    apply();
  }

  function onDoubleClick(e: MouseEvent) {
    e.preventDefault();
    fit();
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
    if (e.metaKey || e.ctrlKey) return;
    if ((opts.keys ?? "always") === "hover" && !hovered) return;
    // Zoom presets are on the numpad, as in Blender; the plain digits rate photos.
    switch (e.code === "Numpad0" || e.code === "Numpad1" ? e.code : e.key) {
      case "Home":
      case "Numpad0":
        fit();
        break;
      case "Numpad1":
        // 1:1 — the preview is rendered at screen scale, so this is fit × 1.
        zoom = 1;
        x = 0;
        y = 0;
        apply();
        break;
      case "+":
      case "=":
        zoomAt(1.25);
        break;
      case "-":
      case "_":
        zoomAt(1 / 1.25);
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  const onEnter = () => (hovered = true);
  const onLeave = () => (hovered = false);
  node.addEventListener("pointerenter", onEnter);
  node.addEventListener("pointerleave", onLeave);
  node.addEventListener("wheel", onWheel, { passive: false });
  node.addEventListener("pointerdown", onPointerDown);
  node.addEventListener("pointermove", onPointerMove);
  node.addEventListener("pointerup", onPointerUp);
  node.addEventListener("pointercancel", onPointerUp);
  node.addEventListener("dblclick", onDoubleClick);
  node.addEventListener("auxclick", (e) => e.preventDefault());
  window.addEventListener("keydown", onKey);
  apply();

  return {
    update(next: PanZoomOptions) {
      opts = next;
      if (next.key !== key) {
        key = next.key;
        fit();
      } else {
        apply();
      }
    },
    destroy() {
      node.removeEventListener("pointerenter", onEnter);
      node.removeEventListener("pointerleave", onLeave);
      node.removeEventListener("wheel", onWheel);
      node.removeEventListener("pointerdown", onPointerDown);
      node.removeEventListener("pointermove", onPointerMove);
      node.removeEventListener("pointerup", onPointerUp);
      node.removeEventListener("pointercancel", onPointerUp);
      node.removeEventListener("dblclick", onDoubleClick);
      window.removeEventListener("keydown", onKey);
    },
  };
}
