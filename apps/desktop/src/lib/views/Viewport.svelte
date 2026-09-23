<script lang="ts">
  // The picture, drawn by the GPU rather than laid out by the DOM.
  //
  // A rendered frame arrives as raw RGBA bytes over `frame://` and goes
  // straight into a texture; zoom and pan are two numbers the shader reads.
  // Past one texel per device pixel the sampler is nearest, so pixels are
  // square; below it, linear over mipmaps, so a 7000-pixel frame fitted into
  // 1400 does not shimmer. Nothing here is resampled by a compositor.
  //
  // WebGPU, with a WebGL2 fallback that does the same with older plumbing.
  import { panzoom } from "../panzoom";
  import { looks as looksApi, type FrameRef } from "../api";

  interface Rect {
    x: number;
    y: number;
    w: number;
    h: number;
  }

  interface Props {
    frame: FrameRef | null;
    /** Refit when this changes (the photo, its shape). */
    key?: unknown;
    /** Zoom relative to fit, and screen pixels per picture pixel. */
    onzoom?: (zoom: number, pxPerTexel: number) => void;
    /** Where the picture sits in the viewport, in CSS pixels; null when nothing is shown. */
    onlayout?: (rect: Rect | null) => void;
  }
  let { frame, key, onzoom, onlayout }: Props = $props();

  let host = $state<HTMLDivElement | undefined>();
  let canvas = $state<HTMLCanvasElement | undefined>();

  // The view state, as panzoom hands it over.
  let zoom = 1;
  let panX = 0;
  let panY = 0;
  let texW = 0;
  let texH = 0;

  // ---- GPU plumbing: one of these two is live ----
  type Gpu = {
    kind: "webgpu";
    device: GPUDevice;
    ctx: GPUCanvasContext;
    format: GPUTextureFormat;
    pipeline: GPURenderPipeline;
    mipPipeline: GPURenderPipeline;
    uniforms: GPUBuffer;
    nearest: GPUSampler;
    linear: GPUSampler;
    mipSampler: GPUSampler;
    texture: GPUTexture | null;
    groups: { nearest: GPUBindGroup; linear: GPUBindGroup } | null;
  };
  type Gl = {
    kind: "webgl2";
    gl: WebGL2RenderingContext;
    program: WebGLProgram;
    rectLoc: WebGLUniformLocation;
    texture: WebGLTexture | null;
  };
  let gpu: Gpu | Gl | null = null;
  let ready = false;

  const WGSL = `
    struct U { rect: vec4<f32> };
    @group(0) @binding(0) var<uniform> u: U;
    @group(0) @binding(1) var t: texture_2d<f32>;
    @group(0) @binding(2) var s: sampler;
    struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
    @vertex fn vs(@builtin(vertex_index) i: u32) -> V {
      var q = array<vec2<f32>, 6>(vec2(0.,0.), vec2(1.,0.), vec2(0.,1.), vec2(0.,1.), vec2(1.,0.), vec2(1.,1.));
      let p = q[i];
      var o: V;
      o.pos = vec4(u.rect.x + p.x * u.rect.z, u.rect.y + p.y * u.rect.w, 0., 1.);
      o.uv = vec2(p.x, 1. - p.y);
      return o;
    }
    @fragment fn fs(v: V) -> @location(0) vec4<f32> { return textureSample(t, s, v.uv); }
  `;

  async function initWebGpu(c: HTMLCanvasElement): Promise<Gpu | null> {
    const nav = navigator as Navigator & { gpu?: GPU };
    if (!nav.gpu) return null;
    const adapter = await nav.gpu.requestAdapter();
    if (!adapter) return null;
    // Ask for the adapter's real texture ceiling: a full-resolution frame is over 8192 wide.
    const device = await adapter.requestDevice({ requiredLimits: { maxTextureDimension2D: adapter.limits.maxTextureDimension2D } });
    const ctx = c.getContext("webgpu") as GPUCanvasContext | null;
    if (!ctx) return null;
    const format = nav.gpu.getPreferredCanvasFormat();
    ctx.configure({ device, format, alphaMode: "opaque" });
    const module = device.createShaderModule({ code: WGSL });
    const pipeline = device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "vs" },
      fragment: { module, entryPoint: "fs", targets: [{ format }] },
      primitive: { topology: "triangle-list" },
    });
    // Mip levels are made by drawing each level from the one above, into rgba8.
    const mipPipeline = device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "vs" },
      fragment: { module, entryPoint: "fs", targets: [{ format: "rgba8unorm" }] },
      primitive: { topology: "triangle-list" },
    });
    const uniforms = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
    return {
      kind: "webgpu",
      device,
      ctx,
      format,
      pipeline,
      mipPipeline,
      uniforms,
      nearest: device.createSampler({ magFilter: "nearest", minFilter: "nearest", mipmapFilter: "nearest" }),
      linear: device.createSampler({ magFilter: "linear", minFilter: "linear", mipmapFilter: "linear" }),
      mipSampler: device.createSampler({ magFilter: "linear", minFilter: "linear" }),
      texture: null,
      groups: null,
    };
  }

  function initWebGl(c: HTMLCanvasElement): Gl | null {
    const gl = c.getContext("webgl2", { alpha: false, antialias: false, premultipliedAlpha: false });
    if (!gl) return null;
    const vsSrc = `#version 300 es
      uniform vec4 rect; out vec2 uv;
      void main() {
        vec2 q[6] = vec2[6](vec2(0,0), vec2(1,0), vec2(0,1), vec2(0,1), vec2(1,0), vec2(1,1));
        vec2 p = q[gl_VertexID];
        gl_Position = vec4(rect.x + p.x * rect.z, rect.y + p.y * rect.w, 0., 1.);
        uv = vec2(p.x, 1. - p.y);
      }`;
    const fsSrc = `#version 300 es
      precision highp float; uniform sampler2D t; in vec2 uv; out vec4 o;
      void main() { o = texture(t, uv); }`;
    const sh = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return s;
    };
    const program = gl.createProgram()!;
    gl.attachShader(program, sh(gl.VERTEX_SHADER, vsSrc));
    gl.attachShader(program, sh(gl.FRAGMENT_SHADER, fsSrc));
    gl.linkProgram(program);
    gl.useProgram(program);
    return { kind: "webgl2", gl, program, rectLoc: gl.getUniformLocation(program, "rect")!, texture: null };
  }

  // ---- a frame arrives ----
  let loading = 0;
  async function load(f: FrameRef | null) {
    const seq = ++loading;
    if (!f || !gpu) {
      texW = texH = 0;
      draw();
      return;
    }
    const res = await fetch(looksApi.frameUrl(f.token));
    if (!res.ok) return;
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (seq !== loading) return; // a newer frame overtook this one
    if (bytes.length < f.width * f.height * 4) return;
    upload(bytes, f.width, f.height);
    texW = f.width;
    texH = f.height;
    draw();
  }

  function upload(bytes: Uint8Array<ArrayBuffer>, w: number, h: number) {
    if (!gpu) return;
    if (gpu.kind === "webgpu") {
      const g = gpu;
      const { device } = g;
      g.texture?.destroy();
      const levels = 1 + Math.floor(Math.log2(Math.max(w, h)));
      const texture = device.createTexture({
        size: { width: w, height: h },
        format: "rgba8unorm",
        mipLevelCount: levels,
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
      });
      device.queue.writeTexture({ texture }, bytes, { bytesPerRow: w * 4, rowsPerImage: h }, { width: w, height: h });
      // Each mip level from the one above, drawn with a linear sampler: a box-ish reduce.
      const encoder = device.createCommandEncoder();
      const full = new Float32Array([-1, -1, 2, 2]);
      const mipUniforms = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
      device.queue.writeBuffer(mipUniforms, 0, full);
      for (let level = 1; level < levels; level++) {
        const group = device.createBindGroup({
          layout: g.mipPipeline.getBindGroupLayout(0),
          entries: [
            { binding: 0, resource: { buffer: mipUniforms } },
            { binding: 1, resource: texture.createView({ baseMipLevel: level - 1, mipLevelCount: 1 }) },
            { binding: 2, resource: g.mipSampler },
          ],
        });
        const pass = encoder.beginRenderPass({
          colorAttachments: [{ view: texture.createView({ baseMipLevel: level, mipLevelCount: 1 }), loadOp: "clear", storeOp: "store" }],
        });
        pass.setPipeline(g.mipPipeline);
        pass.setBindGroup(0, group);
        pass.draw(6);
        pass.end();
      }
      device.queue.submit([encoder.finish()]);
      const view = texture.createView();
      const mk = (s: GPUSampler) =>
        device.createBindGroup({
          layout: g.pipeline.getBindGroupLayout(0),
          entries: [
            { binding: 0, resource: { buffer: g.uniforms } },
            { binding: 1, resource: view },
            { binding: 2, resource: s },
          ],
        });
      g.texture = texture;
      g.groups = { nearest: mk(g.nearest), linear: mk(g.linear) };
    } else {
      const { gl } = gpu;
      if (gpu.texture) gl.deleteTexture(gpu.texture);
      const t = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, bytes);
      gl.generateMipmap(gl.TEXTURE_2D);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gpu.texture = t;
    }
  }

  // ---- where the picture goes ----
  /** The picture's rectangle in CSS pixels of the host, for the current view state. */
  function placement(): Rect | null {
    if (!host || !texW || !texH) return null;
    const W = host.clientWidth;
    const H = host.clientHeight;
    if (!W || !H) return null;
    const fit = Math.min(W / texW, H / texH);
    const w = texW * fit * zoom;
    const h = texH * fit * zoom;
    return { x: (W - w) / 2 + panX, y: (H - h) / 2 + panY, w, h };
  }

  function draw() {
    if (!gpu || !canvas || !host) return;
    const dpr = window.devicePixelRatio || 1;
    const W = host.clientWidth;
    const H = host.clientHeight;
    const pw = Math.max(1, Math.round(W * dpr));
    const ph = Math.max(1, Math.round(H * dpr));
    if (canvas.width !== pw || canvas.height !== ph) {
      canvas.width = pw;
      canvas.height = ph;
    }
    const r = placement();
    onlayout?.(r);
    const pxPerTexel = r ? (r.w / texW) * dpr : 0;
    onzoom?.(zoom, pxPerTexel);

    // Snap the picture to whole device pixels so a pan never lands it between two.
    const rect = r
      ? (() => {
          const x0 = Math.round(r.x * dpr) / pw;
          const y0 = Math.round(r.y * dpr) / ph;
          const w = Math.round(r.w * dpr) / pw;
          const h = Math.round(r.h * dpr) / ph;
          // NDC: x right, y up; the quad's origin is its bottom-left.
          return new Float32Array([x0 * 2 - 1, 1 - (y0 + h) * 2, w * 2, h * 2]);
        })()
      : null;
    const nearest = pxPerTexel >= 1;

    if (gpu.kind === "webgpu") {
      const { device, ctx, pipeline, uniforms, groups } = gpu;
      const encoder = device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [{ view: ctx.getCurrentTexture().createView(), clearValue: { r: 0.055, g: 0.051, b: 0.047, a: 1 }, loadOp: "clear", storeOp: "store" }],
      });
      if (rect && groups) {
        device.queue.writeBuffer(uniforms, 0, rect);
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, nearest ? groups.nearest : groups.linear);
        pass.draw(6);
      }
      pass.end();
      device.queue.submit([encoder.finish()]);
    } else {
      const { gl, rectLoc, texture } = gpu;
      gl.viewport(0, 0, pw, ph);
      gl.clearColor(0.055, 0.051, 0.047, 1);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (rect && texture) {
        gl.bindTexture(gl.TEXTURE_2D, texture);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, nearest ? gl.NEAREST : gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, nearest ? gl.NEAREST : gl.LINEAR_MIPMAP_LINEAR);
        gl.uniform4fv(rectLoc, rect);
        gl.drawArrays(gl.TRIANGLES, 0, 6);
      }
    }
  }

  /** The zoom at which one picture pixel is one screen pixel. */
  function onePixel(): number | null {
    if (!host || !texW || !texH) return null;
    const fit = Math.min(host.clientWidth / texW, host.clientHeight / texH);
    return 1 / (fit * (window.devicePixelRatio || 1));
  }

  // ---- lifecycle ----
  $effect(() => {
    const c = canvas;
    if (!c) return;
    let alive = true;
    (async () => {
      gpu = (await initWebGpu(c)) ?? initWebGl(c);
      if (!alive) return;
      ready = true;
      load(frame);
    })();
    return () => {
      alive = false;
    };
  });

  // A new frame; loaded once the GPU is up.
  $effect(() => {
    const f = frame;
    if (ready) load(f);
  });

  // The host changes size with the window and the panels.
  $effect(() => {
    const h = host;
    if (!h) return;
    const ro = new ResizeObserver(() => draw());
    ro.observe(h);
    return () => ro.disconnect();
  });
</script>

<div
  class="vp"
  bind:this={host}
  use:panzoom={{
    key,
    apply: (z, x, y) => {
      zoom = z;
      panX = x;
      panY = y;
      draw();
    },
    pixel: onePixel,
  }}
  role="img"
  aria-label="Picture"
>
  <canvas bind:this={canvas}></canvas>
</div>

<style>
  .vp {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
