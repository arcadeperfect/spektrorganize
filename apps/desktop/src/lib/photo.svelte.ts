// The develop stage: one photo's decode settings, its developed preview, and
// the catalog round-trip. The print stage (looks.svelte.ts) renders on top of
// this, so both stages read the same settings.

import { looks as backend, defaultRaw, type FrameRef, type InputInfo, type RawSettings } from "./api";
import { quality, type Quality } from "./quality.svelte";

/** Preview size when the photo is fitted to the stage. */
export const BASE_PX = 1600;

/** Preview size while a control is being dragged. */
const DRAG_PX = 900;

/** Pixels to render for a zoom level, in steps so small moves do not re-render. */
export function detailFor(zoom: number, native: number): number {
  const want = Math.ceil((BASE_PX * Math.max(1, zoom)) / 800) * 800;
  const cap = native > 0 ? Math.min(native, 16384) : 4096;
  return Math.min(cap, Math.max(BASE_PX, want));
}


class DevelopStage {
  id = $state<number | null>(null);
  /** Whether this photo is a RAW to decode or an image with nothing to decode. */
  input = $state<InputInfo | null>(null);
  raw = $state<RawSettings>(defaultRaw());
  /** The developed picture, as a frame the GPU viewport fetches. */
  preview = $state<FrameRef | null>(null);
  rendering = $state(false);
  error = $state<string | null>(null);
  /** Bumped whenever the settings change, so the print stage re-renders. */
  version = $state(0);

  private pending = false;
  /**
   * Preview resolution. The stage renders at screen scale, so zooming in means
   * re-rendering bigger — otherwise you are magnifying a 1600 px JPEG. Capped at
   * the backend's 4096; the engine never renders past the photo's own size.
   */
  detail = $state(BASE_PX);
  /**
   * While the crop tool is open the preview shows the whole frame, so the
   * rectangle has something to be drawn on. The crop itself is still saved.
   */
  showWholeFrame = $state(false);
  private zoomTimer: ReturnType<typeof setTimeout> | null = null;
  private settleTimer: ReturnType<typeof setTimeout> | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private frameTimer: ReturnType<typeof setTimeout> | null = null;
  /** A control is being dragged right now, so the preview stays small. */
  dragging = $state(false);

  /** A clip: previews show one frame of it. */
  get isVideo(): boolean {
    return this.input?.is_video ?? false;
  }

  /** Which frame of a clip the previews show, in seconds. Null is the poster frame. */
  frameAt = $state<number | null>(null);

  /** Move to another frame of the clip and re-render whatever stage is in view. */
  setFrame(at: number, then: () => void) {
    this.frameAt = at;
    if (this.frameTimer) clearTimeout(this.frameTimer);
    // Scrubbing asks for a decode per position; wait for the handle to settle.
    this.frameTimer = setTimeout(() => {
      this.frameTimer = null;
      then();
    }, 180);
  }

  /** The photo's own longest side, 0 when the catalog never recorded it. */
  get native(): number {
    return Math.max(this.input?.width ?? 0, this.input?.height ?? 0);
  }

  /** Choose low / high / native; `then` re-renders the stage in view. Remembered across views. */
  setQuality(q: Quality, then: () => void) {
    quality.set(q);
    this.detail = quality.px(this.native);
    then();
  }

  /**
   * Zoom no longer changes the render size: the quality setting does, so what
   * you see is what you chose. Kept so the viewers' wiring stays simple.
   */
  setZoom(_z: number, _then: () => void) {}

  get isRaw(): boolean {
    return this.input?.is_raw ?? true;
  }

  get isDefault(): boolean {
    const d = defaultRaw();
    return (Object.keys(d) as (keyof RawSettings)[]).every((k) => this.raw[k] === d[k]);
  }

  async setPhoto(id: number | null) {
    if (this.id === id) return;
    this.id = id;
    this.preview = null;
    this.error = null;
    this.input = id === null ? null : await backend.input(id).catch(() => null);
    this.frameAt = null;
    this.detail = quality.px(this.native);
    this.raw = id === null ? defaultRaw() : await backend.rawGet(id).catch(() => defaultRaw());
    this.version++;
    this.refresh();
  }

  /**
   * Change the develop settings. `live` is for a control being dragged: the
   * preview renders small while the pointer moves and full size once it
   * settles, and the catalog write waits for the same pause. Without it a
   * slider queues a decode, a JPEG encode and a database write per pixel.
   */
  setRaw(patch: Partial<RawSettings>, live = false) {
    this.raw = { ...this.raw, ...patch };
    this.version++;
    this.queueSave();
    // The crop tool renders the whole frame, so dragging the rectangle changes
    // nothing about the picture on screen: save it and skip the render.
    if (this.showWholeFrame && Object.keys(patch).length === 1 && "crop" in patch) return;
    if (!live) {
      this.dragging = false;
      this.refresh();
      return;
    }
    this.dragging = true;
    if (this.settleTimer) clearTimeout(this.settleTimer);
    this.settleTimer = setTimeout(() => {
      this.settleTimer = null;
      this.dragging = false;
      this.refresh();
    }, 220);
    this.refresh();
  }

  /** One write per pause, not one per pointer move. */
  private queueSave() {
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      if (this.id !== null) backend.rawSet([this.id], $state.snapshot(this.raw)).catch((e) => (this.error = String(e)));
    }, 250);
  }

  reset() {
    this.setRaw(defaultRaw());
  }

  /** Open or close the crop tool's view of the uncropped frame. */
  setWholeFrame(on: boolean) {
    if (this.showWholeFrame === on) return;
    this.showWholeFrame = on;
    this.refresh();
  }

  /** Copy this photo's develop settings onto others. */
  async applyTo(ids: number[]) {
    await backend.rawSet(ids, this.raw);
  }

  /** Re-render the developed preview (coalesced: one render in flight). */
  refresh() {
    if (this.rendering) {
      this.pending = true;
      return;
    }
    this.run();
  }

  private async run() {
    if (this.id === null) return;
    this.rendering = true;
    try {
      const raw = this.showWholeFrame ? { ...this.raw, crop: null } : { ...this.raw };
      // Dragging: a smaller render keeps up with the pointer. The settled one follows.
      const px = this.dragging ? Math.min(this.detail, DRAG_PX) : this.detail;
      this.preview = await backend.developFrame(this.id, raw, px, this.frameAt);
      this.error = null;
    } catch (e) {
      this.error = String(e);
    } finally {
      this.rendering = false;
      if (this.pending) {
        this.pending = false;
        this.run();
      }
    }
  }
}

export const develop = new DevelopStage();
