<script lang="ts">
  // Render a clip through a look. Two ways, because they answer different
  // needs: a baked cube is fast enough for a whole clip, the full pipeline is
  // the only way to get grain.
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, catalog, duration as durationText, looks as looksApi, type AssetSummary, type Look } from "../../api";
  import { library as lib } from "../../library.svelte";
  import { lookEditor } from "../../looks.svelte";

  interface Props {
    asset: AssetSummary;
    onclose: () => void;
  }
  let { asset, onclose }: Props = $props();

  type Mode = "none" | "lut" | "full";
  let mode = $state<Mode>("lut");
  let codec = $state<"h264" | "hevc" | "pro_res422" | "pro_res4444">("h264");
  let maxPx = $state(1920);
  let mbps = $state(12);
  let busy = $state(false);
  let progress = $state<{ done: number; total: number } | null>(null);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);

  const look = $derived(lookEditor.look as Look | null);
  // A rough guess, to set expectations before a long render starts.
  const frames = $derived(Math.round((asset.duration ?? 0) * 30));
  const estimate = $derived(mode === "full" ? frames * 0.17 : frames * 0.03);

  $effect(() => {
    let off: (() => void) | undefined;
    api.on<{ done: number; total: number }>("video-progress", (p) => (progress = p)).then((f) => (off = f));
    return () => off?.();
  });

  async function run() {
    const ext = codec === "h264" || codec === "hevc" ? "mp4" : "mov";
    const suggested = asset.name.replace(/\.[^.]+$/, "") + (mode === "none" ? "" : "-look") + "." + ext;
    const dst = await save({ defaultPath: suggested, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] });
    if (!dst) return;
    busy = true;
    error = note = null;
    progress = null;
    try {
      const report = await looksApi.videoRender(asset.id, {
        src: "",
        dst,
        codec,
        look: mode,
        max_px: maxPx,
        mbps,
        audio: false,
      }, mode === "none" ? null : look);
      note = `${report.frames} frames · ${report.width}×${report.height} · ${report.seconds.toFixed(1)}s${
        report.silent ? " · no sound yet" : ""
      }`;
      lib.note = `Rendered ${asset.name}.`;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
      progress = null;
    }
  }

  async function exportCube() {
    if (!look) return;
    const dst = await save({ defaultPath: `${look.name || "look"}.cube`, filters: [{ name: "Cube LUT", extensions: ["cube"] }] });
    if (!dst) return;
    busy = true;
    try {
      await looksApi.exportCube(look, dst, 33);
      note = `Saved ${dst.split("/").pop()} — 33³, for Resolve and anything else that reads a cube.`;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="back" role="presentation" onclick={onclose}></div>
<div class="dlg card">
  <h2>Render {asset.name}</h2>
  <p class="muted small">{durationText(asset.duration ?? 0)} · {asset.width}×{asset.height}</p>

  <div class="field">
    <span class="small">Look</span>
    <div class="row">
      <button class="mini" class:on={mode === "none"} onclick={() => (mode = "none")}>none</button>
      <button class="mini" class:on={mode === "lut"} onclick={() => (mode = "lut")} disabled={!look}>baked LUT</button>
      <button class="mini" class:on={mode === "full"} onclick={() => (mode = "full")} disabled={!look}>full pipeline</button>
    </div>
    <p class="muted small hint">
      {#if mode === "none"}
        A straight re-encode, no look.
      {:else if mode === "lut"}
        {look?.name ?? "The look"} baked into a colour cube: fast, and the same cube can go to Resolve. No grain or halation — those are spatial, and a
        cube cannot carry them.
      {:else}
        The whole film pipeline on every frame, grain and all. Exposure is metered once so the clip does not pump.
      {/if}
      {#if mode !== "none" && frames}
        Roughly {estimate < 60 ? `${Math.max(1, Math.round(estimate))}s` : `${Math.round(estimate / 60)} min`} for {frames} frames.
      {/if}
    </p>
  </div>

  <div class="field">
    <span class="small">Format</span>
    <div class="row">
      <button class="mini" class:on={codec === "h264"} onclick={() => (codec = "h264")} title="Plays everywhere">H.264</button>
      <button class="mini" class:on={codec === "hevc"} onclick={() => (codec = "hevc")} title="Smaller, fussier">HEVC</button>
      <button class="mini" class:on={codec === "pro_res422"} onclick={() => (codec = "pro_res422")}>ProRes 422</button>
      <button class="mini" class:on={codec === "pro_res4444"} onclick={() => (codec = "pro_res4444")}>ProRes 4444</button>
    </div>
  </div>

  <div class="field row spread">
    <label class="small">Longest edge
      <select bind:value={maxPx}>
        <option value={0}>as shot</option>
        <option value={1080}>1080</option>
        <option value={1920}>1920</option>
        <option value={2560}>2560</option>
        <option value={3840}>3840</option>
      </select>
    </label>
    {#if codec === "h264" || codec === "hevc"}
      <label class="small">Mbps
        <input type="number" min="1" max="200" step="1" bind:value={mbps} />
      </label>
    {/if}
  </div>

  {#if progress}
    <div class="progress"><div style="width: {progress.total ? (progress.done / progress.total) * 100 : 0}%"></div></div>
    <p class="muted small">{progress.done} / {progress.total} frames</p>
  {/if}
  {#if note}<p class="small ok">{note}</p>{/if}
  {#if error}<p class="small bad">{error}</p>{/if}

  <div class="row spread foot">
    <button class="mini" onclick={exportCube} disabled={busy || !look}>Save the look as .cube</button>
    <div class="row">
      <button onclick={onclose} disabled={busy}>Close</button>
      <button class="primary" onclick={run} disabled={busy}>{busy ? "Rendering…" : "Render…"}</button>
    </div>
  </div>
</div>

<style>
  .back {
    position: absolute;
    inset: 0;
    background: rgba(10, 9, 8, 0.55);
    z-index: 19;
  }
  .dlg {
    position: absolute;
    z-index: 20;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 460px;
    max-width: calc(100vw - 40px);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .on {
    color: var(--accent-2);
    border-color: var(--accent-2);
  }
  .hint {
    line-height: 1.4;
    margin: 0;
  }
  .foot {
    margin-top: 4px;
  }
  .small {
    font-size: 11.5px;
  }
  .progress {
    height: 4px;
    background: var(--bg-3);
    border-radius: 2px;
    overflow: hidden;
  }
  .progress div {
    height: 100%;
    background: var(--accent);
  }
</style>
