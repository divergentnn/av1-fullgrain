// av1-fullgrain on the GPU: AV1 film grain drawn with WebGPU compute shaders
// onto pictures decoded without grain (by the browser's own decoder, see
// native-clip.mjs), bit-identical to the dav1d fork's C code.
//
//   full      full-frame grain (src/fg_variant_tmpl.c, fgv_full_row): hashed
//             white noise, the signalled AR filter over the whole frame,
//             restarted for each group of 4 x 32 rows 16 rows above it.
//   standard  AV1 grain as the specification defines it (src/filmgrain_tmpl.c):
//             an 82 x 73 template per plane (LFSR noise + AR filter, run as a
//             wavefront in workgroup memory), 32 x 32 blocks copied from it at
//             per-block LFSR offsets, overlap blending at the block seams.
//
// The shaders and how they work: gpu/src/grain.wgsl (through grain-wgsl.mjs).
//
// Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

import { gaussianTable, generateScaling, fullKeys } from './fg-params.mjs';
import { WGSL } from './grain-wgsl.mjs';

// The shaders (grain-wgsl.mjs, generated from gpu/src/grain.wgsl, shared with the native renderer in gpu/) and
// their constants: every "const NAME = number;" line of the WGSL. P_* are the parameter block indices.
const C = Object.fromEntries([...WGSL.matchAll(/^const (\w+) = (\d+);/gm)].map(m => [m[1], +m[2]]));
const I = Object.fromEntries(Object.entries(C).filter(([k]) => k.startsWith('P_')).map(([k, v]) => [k.slice(2), v]));
export const ROWS = C.ROWS;        // rows of one band group: 16 warm-up + 4 x 32
const S = C.S, K = C.K;            // columns per wavefront step; steps of skew per row
const TPL = C.TPL;                 // a standard-mode template (GRAIN_HEIGHT + 1) x GRAIN_WIDTH, as dav1d allocates it
const GSTRIDE = C.GSTRIDE;
const NPARAM = C.NPARAM;

const adapters = new Map();
/** the WebGPU adapter (cached; the first request can return null while the GPU process starts, so ask twice) */
export async function gpuAdapter(powerPreference = 'high-performance') {
  if (!adapters.has(powerPreference)) {
    adapters.set(powerPreference, (async () => {
      for (let i = 0; i < 2; i++) {
        const a = await navigator.gpu.requestAdapter({ powerPreference });
        if (a) return a;
      }
      return null;
    })());
  }
  return adapters.get(powerPreference);
}

/** [] when WebGPU is usable here, otherwise the reasons */
export async function gpuSupport(powerPreference = 'high-performance') {
  if (!self.isSecureContext) return ['not a secure context'];
  if (!navigator.gpu) return ['no WebGPU'];
  try {
    if (!await gpuAdapter(powerPreference)) return ['no WebGPU adapter'];
  } catch (e) { return ['WebGPU adapter: ' + e.message]; }
  return [];
}

const devices = new Map();
/** the page's WebGPU device for this power preference (one for every GpuGrain): {adapter, device} */
export function gpuDevice(powerPreference = 'high-performance') {
  if (!devices.has(powerPreference)) {
    devices.set(powerPreference, (async () => {
      let adapter = null;                       // a fresh adapter: an adapter makes one device only
      for (let i = 0; i < 2 && !adapter; i++) adapter = await navigator.gpu.requestAdapter({ powerPreference });
      if (!adapter) throw new Error('no WebGPU adapter');
      const features = adapter.features.has('timestamp-query') ? ['timestamp-query'] : [];
      const device = await adapter.requestDevice({
        requiredFeatures: features,
        requiredLimits: {
          maxStorageBufferBindingSize: adapter.limits.maxStorageBufferBindingSize,
          maxBufferSize: adapter.limits.maxBufferSize,
          maxComputeInvocationsPerWorkgroup: Math.max(256, ROWS),
          maxComputeWorkgroupSizeX: Math.max(256, ROWS),
        },
      });
      device.lost.then(() => devices.delete(powerPreference));
      return { adapter, device };
    })());
    devices.get(powerPreference).catch(() => devices.delete(powerPreference));
  }
  return devices.get(powerPreference);
}

/** a grain renderer with its own buffers on the shared device (or opts.device + opts.adapter) */
export async function createGpuGrain(opts = {}) {
  const { adapter, device } = opts.device ? opts : await gpuDevice(opts.powerPreference || 'high-performance');
  return new GpuGrain(adapter, device, device.features.has('timestamp-query'));
}

const align = (x, a) => Math.ceil(x / a) * a;
const pipeCache = new WeakMap();

// AR coefficients (AV1 order: rows -lag..-1 x columns -lag..lag, then -lag..-1 in the row) as a fixed 3 x 7 grid
// of previous-row taps (row -3 first) and the in-row taps c1, c2, c3 (columns -1, -2, -3)
function grid(P, at, co, lag) {
  P.fill(0, at, at + GSTRIDE);
  let k = 0;
  for (let dy = -lag; dy < 0; dy++)
    for (let dx = -lag; dx <= lag; dx++) P[at + (dy + 3) * 7 + dx + 3] = co[k++];
  for (let dx = -lag; dx < 0; dx++) P[at + 20 - dx] = co[k++];
}

export class GpuGrain {
  constructor(adapter, device, timestamps) {
    this.I = I;
    this.adapter = adapter;
    this.device = device;
    this.info = adapter.info || {};
    this.timestamps = timestamps;
    if (!pipeCache.has(device)) {               // shader + pipelines once per device
      const module = device.createShaderModule({ code: WGSL });
      const B = (binding, type) => ({ binding, visibility: GPUShaderStage.COMPUTE, buffer: { type } });
      const bgl = device.createBindGroupLayout({ entries: [
        B(0, 'uniform'), B(1, 'read-only-storage'), B(2, 'storage'), B(3, 'storage'),
        B(4, 'storage'), B(5, 'read-only-storage'), B(6, 'storage'), B(7, 'storage'),
      ] });
      const layout = device.createPipelineLayout({ bindGroupLayouts: [bgl] });
      const pipe = entryPoint => device.createComputePipeline({ layout, compute: { module, entryPoint } });
      pipeCache.set(device, { bgl, pipes: {
        ar_luma: pipe('ar_luma'), ar_chroma: pipe('ar_chroma'), lumaterm: pipe('lumaterm'),
        std_tpl_luma: pipe('std_tpl_luma'), std_tpl_chroma: pipe('std_tpl_chroma'), std_offsets: pipe('std_offsets'),
        apply_y: pipe('apply_y'), apply_c: pipe('apply_c') } });
    }
    ({ bgl: this.bgl, pipes: this.pipes } = pipeCache.get(device));
    this.params = new Int32Array(NPARAM);
    this.uni = device.createBuffer({ size: NPARAM * 4, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
    this.tabs = device.createBuffer({ size: (2048 + 3 * 4096) * 4, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
    this.fmt = null;
    this.lastSet = null;
    if (timestamps) {
      this.qs = device.createQuerySet({ type: 'timestamp', count: 8 });
      this.qbuf = device.createBuffer({ size: 64, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
      this.qread = device.createBuffer({ size: 64, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    }
  }

  /** picture format: {width, height, bitdepth, layout (dav1d: 0 I400, 1 I420, 2 I422, 3 I444), mtrx} */
  configure(f) {
    const k = JSON.stringify([f.width, f.height, f.bitdepth, f.layout, f.mtrx]);
    if (this.fmtKey === k) return;
    this.fmtKey = k;
    this.fmt = f;
    const P = this.params, W = f.width, H = f.height, mono = f.layout === 0;
    const ssx = f.layout === 1 || f.layout === 2 ? 1 : 0, ssy = f.layout === 1 ? 1 : 0;
    const CW = mono ? 0 : (W + ssx) >> ssx, CH = mono ? 0 : (H + ssy) >> ssy;
    const NG = Math.ceil(H / 128);
    const lbw = 16 + W + 8, nchl = Math.ceil(lbw / S), lpitch = align(lbw, 4);
    const rsc = ROWS >> ssy, cbw = (16 >> ssx) + CW + (8 >> ssx), nchc = Math.ceil(cbw / S), cpitch = align(cbw, 4);
    Object.assign(this, { W, H, CW, CH, NG, ssx, ssy, mono });
    P.fill(0);
    P[I.W] = W; P[I.H] = H; P[I.SSX] = ssx; P[I.SSY] = ssy; P[I.BD] = f.bitdepth; P[I.CW] = CW; P[I.CH] = CH; P[I.NG] = NG;
    P[I.RSL] = ROWS; P[I.LPITCH] = lpitch; P[I.LBW] = lbw; P[I.NCHL] = nchl;
    P[I.RSC] = rsc; P[I.CPITCH] = cpitch; P[I.CBW] = cbw; P[I.NCHC] = nchc;
    const ntc = nchc + K * (rsc - 1);
    P[I.NTC] = ntc;
    P[I.MONO] = mono ? 1 : 0; P[I.PMAX] = (1 << f.bitdepth) - 1;
    // output: 16-bit samples, rows padded to an even count
    const dsy = align(W, 2), dsc = align(CW, 2);
    P[I.DOY] = 0; P[I.DSY] = dsy; P[I.DOU] = dsy * H; P[I.DOV] = dsy * H + dsc * CH; P[I.DSC] = dsc;
    this.dstSamples = dsy * H + 2 * dsc * CH;
    const dev = this.device;
    if (this.srcBuf && !this.ownSrc) this.srcBuf = null;      // not ours to destroy
    for (const b of ['fl', 'fc', 'lt', 'sb', 'dst', 'srcBuf', 'readBuf']) if (this[b]) { this[b].destroy(); this[b] = null; }
    P[I.NBR] = Math.ceil(H / 32); P[I.NBC] = Math.ceil(W / 32);
    this.sb = dev.createBuffer({ size: (3 * TPL + P[I.NBR] * P[I.NBC]) * 4, usage: GPUBufferUsage.STORAGE });
    this.fl = dev.createBuffer({ size: NG * ROWS * lpitch * 2, usage: GPUBufferUsage.STORAGE });
    this.fc = dev.createBuffer({ size: Math.max(16, mono ? 16 : 2 * NG * rsc * cpitch * 2), usage: GPUBufferUsage.STORAGE });
    this.lt = dev.createBuffer({ size: Math.max(16, mono ? 16 : NG * ntc * S * rsc * 4), usage: GPUBufferUsage.STORAGE });
    this.dst = dev.createBuffer({ size: this.dstSamples * 2, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
    this.srcSize = 0;
    this.bind = null;
    this.lastSet = null;
    this.fieldBytes = this.fl.size + this.fc.size + this.lt.size + this.sb.size;
  }

  /** a GPU buffer (STORAGE usage) that already holds the input picture, e.g. filled by VideoFrame.copyTo() into
   *  its mapped range (sourceBuffer()); planes / bps / semi as for upload() */
  setSource(buffer, planes, bps, semi = false) {
    if (this.srcBuf !== buffer) { this.srcBuf = buffer; this.ownSrc = false; this.bind = null; }
    this.layoutPlanes(planes, bps, semi);
  }

  /** a decoded VideoFrame -> a GPU buffer holding its planes (for setSource()), through a pool of mappable staging
   *  buffers and a GPU copy: a new mappedAtCreation buffer per frame is 3x slower in Chrome (fresh shared memory).
   *  Give the buffer back with releaseFrame() when it is no longer drawn. */
  async frameBuffer(frame, layout) {
    const dev = this.device, size = align(layout.size, 4);
    if (this.poolSize !== size) {                  // a new frame size: start new pools
      for (const b of this.stageFree || []) b.destroy();
      for (const b of this.frameFree || []) b.destroy();
      this.stageFree = []; this.frameFree = []; this.poolSize = size;
    }
    // staging buffers are mapped again as soon as their copy is done, so one is usually ready (no mapAsync wait
    // while the decoder holds the frame)
    const st = this.stageFree.pop() ||
               dev.createBuffer({ size, usage: GPUBufferUsage.MAP_WRITE | GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
    try { await frame.copyTo(st.getMappedRange(), { layout: layout.planes }); } finally { st.unmap(); }
    const fb = this.frameFree.pop() || dev.createBuffer({ size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
    const enc = dev.createCommandEncoder();
    enc.copyBufferToBuffer(st, 0, fb, 0, size);
    dev.queue.submit([enc.finish()]);
    const pools = this.stageFree;
    st.mapAsync(GPUMapMode.WRITE).then(() => {
      if (this.stageFree === pools && pools.length < 6) pools.push(st); else st.destroy();
    }, () => st.destroy());
    return fb;
  }

  /** a frameBuffer() that is no longer needed: reused once the GPU is done with it */
  releaseFrame(fb) {
    const pools = this.frameFree;
    this.device.queue.onSubmittedWorkDone().then(() => {
      if (this.frameFree === pools && fb.size === this.poolSize && pools.length < 8) pools.push(fb); else fb.destroy();
    });
  }

  layoutPlanes(planes, bps, semi) {
    const P = this.params;
    for (let i = 0; i < 3; i++) {
      const pl = planes[Math.min(i, planes.length - 1)];
      P[I.SOY + 2 * i] = pl.offset; P[I.SSY_ + 2 * i] = pl.stride;
    }
    P[I.BPS] = bps; P[I.STEPY] = bps; P[I.STEPC] = semi ? 2 * bps : bps;
    if (semi) { P[I.SOV] = P[I.SOU] + bps; P[I.SSV] = P[I.SSU]; }
  }

  /** the input picture: bytes (ArrayBuffer / view) with per-plane {offset, stride} in bytes; bps 1 or 2;
   *  semi = true: planes[1] is interleaved U V (NV12), planes[2] is ignored */
  upload(bytes, planes, bps, semi = false) {
    const P = this.params, dev = this.device;
    const u8 = bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const size = align(u8.byteLength, 4);
    if (!this.srcBuf || !this.ownSrc || this.srcSize < size) {
      if (this.srcBuf && this.ownSrc) this.srcBuf.destroy();
      this.ownSrc = true;
      this.srcBuf = dev.createBuffer({ size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
      this.srcSize = size;
      this.bind = null;
    }
    if (u8.byteLength % 4) {                  // writeBuffer wants a multiple of 4 bytes
      const t = new Uint8Array(size); t.set(u8); dev.queue.writeBuffer(this.srcBuf, 0, t);
    } else dev.queue.writeBuffer(this.srcBuf, 0, u8);
    this.layoutPlanes(planes, bps, semi);
  }

  /** the grain of this picture: null (none) or loadParams(...).picture(n); mode 'full' or 'standard' */
  setGrain(pic, mode = 'full') {
    const P = this.params, f = this.fmt, bd = f.bitdepth;
    if (mode !== 'full' && mode !== 'standard') throw new Error('grain mode: full or standard');
    this.mode = pic ? mode : 'none';
    P[I.MODE] = mode === 'standard' ? 1 : 0;
    if (!pic) { P[I.HASY] = P[I.HASU] = P[I.HASV] = 0; return; }
    const mono = this.mono, csfl = pic.chroma_scaling_from_luma;
    P[I.HASY] = pic.num_y_points ? 1 : 0;
    P[I.HASU] = !mono && (csfl || pic.num_uv_points[0]) ? 1 : 0;
    P[I.HASV] = !mono && (csfl || pic.num_uv_points[1]) ? 1 : 0;
    P[I.LAG] = pic.ar_coeff_lag; P[I.ARSH] = pic.ar_coeff_shift;
    P[I.GMIN] = -(128 << (bd - 8)); P[I.GMAX] = (128 << (bd - 8)) - 1;
    const keys = fullKeys(pic.seed);
    P[I.KEYY] = keys[0] | 0; P[I.KEYU] = keys[1] | 0; P[I.KEYV] = keys[2] | 0;
    P[I.SCSH] = pic.scaling_shift; P[I.CSFL] = csfl;
    P[I.SEED] = pic.seed; P[I.OVL] = pic.overlap_flag;
    const b8 = bd - 8;
    if (pic.clip_to_restricted_range) {
      P[I.YMIN] = 16 << b8; P[I.YMAX] = 235 << b8; P[I.CMIN] = 16 << b8; P[I.CMAX] = (f.mtrx === 0 ? 235 : 240) << b8;
    } else { P[I.YMIN] = P[I.CMIN] = 0; P[I.YMAX] = P[I.CMAX] = (1 << bd) - 1; }
    for (let pl = 0; pl < 2; pl++) {
      P[I.ULM + 3 * pl] = pic.uv_luma_mult[pl]; P[I.UM + 3 * pl] = pic.uv_mult[pl];
      P[I.UOFF + 3 * pl] = pic.uv_offset[pl] * (1 << b8);
      const npos = 2 * pic.ar_coeff_lag * (pic.ar_coeff_lag + 1);
      P[I.LCU + pl] = pic.num_y_points ? pic.ar_coeffs_uv[pl][npos] : 0;
      grid(P, I.GRID + GSTRIDE * (1 + pl), pic.ar_coeffs_uv[pl], pic.ar_coeff_lag);
    }
    grid(P, I.GRID, pic.ar_coeffs_y, pic.ar_coeff_lag);
    P[I.SCPU] = csfl ? 0 : 1; P[I.SCPV] = csfl ? 0 : 2;
    // tables: once per parameter set
    if (this.lastSet !== pic.table || this.lastBd !== bd) {
      this.lastSet = pic.table; this.lastBd = bd;
      const t = new Int32Array(2048 + 3 * 4096);
      t.set(gaussianTable(bd, pic.grain_scale_shift), 0);
      if (pic.num_y_points || csfl) t.set(generateScaling(bd, pic.y_points), 2048);
      for (let pl = 0; pl < 2; pl++) if (pic.num_uv_points[pl]) t.set(generateScaling(bd, pic.uv_points[pl]), 2048 + 4096 * (1 + pl));
      this.device.queue.writeBuffer(this.tabs, 0, t);
    }
  }

  bindGroup() {
    if (!this.bind) {
      this.bind = this.device.createBindGroup({ layout: this.bgl, entries: [
        { binding: 0, resource: { buffer: this.uni } }, { binding: 1, resource: { buffer: this.tabs } },
        { binding: 2, resource: { buffer: this.fl } }, { binding: 3, resource: { buffer: this.fc } },
        { binding: 4, resource: { buffer: this.sb } }, { binding: 5, resource: { buffer: this.srcBuf } },
        { binding: 6, resource: { buffer: this.dst } }, { binding: 7, resource: { buffer: this.lt } },
      ] });
    }
    return this.bind;
  }

  /** record the grain passes into enc (after upload + setGrain) */
  encode(enc) {
    const P = this.params, dev = this.device;
    P[I.CPLANE] = P[I.HASU] ? 1 : 2;           // ar_chroma: first chroma plane with grain
    dev.queue.writeBuffer(this.uni, 0, P);
    const bg = this.bindGroup(), ts = this.timestamps && this.timing;
    const pass = (pipe, x, y, z, q) => {
      const d = { };
      if (ts && q !== undefined) d.timestampWrites = { querySet: this.qs, beginningOfPassWriteIndex: q, endOfPassWriteIndex: q + 1 };
      const cp = enc.beginComputePass(d);
      cp.setPipeline(this.pipes[pipe]); cp.setBindGroup(0, bg); cp.dispatchWorkgroups(x, y, z); cp.end();
    };
    if (this.mode === 'full') {
      if (P[I.HASY]) pass('ar_luma', this.NG, 1, 1, 0);
      if (P[I.HASU] || P[I.HASV]) {
        if (P[I.LCU] || P[I.LCV]) pass('lumaterm', Math.ceil(P[I.CBW] / 64), P[I.RSC], this.NG);
        pass('ar_chroma', this.NG, P[I.HASU] && P[I.HASV] ? 2 : 1, 1, 2);   // both planes in one dispatch
      }
    }
    if (this.mode === 'standard') {
      if (P[I.HASY] || P[I.LCU] || P[I.LCV]) pass('std_tpl_luma', 1, 1, 1, 0);
      if (P[I.HASU] || P[I.HASV]) pass('std_tpl_chroma', P[I.HASU] && P[I.HASV] ? 2 : 1, 1, 1, 2);
      pass('std_offsets', Math.ceil(P[I.NBR] / 64), 1, 1);
    }
    pass('apply_y', Math.ceil(this.W / 32), Math.ceil(this.H / 8), 1, 4);
    if (!this.mono) pass('apply_c', Math.ceil(this.CW / 32), Math.ceil(this.CH / 8), 2, 6);
  }

  /** upload + grain + wait; -> GPU ms per pass when timestamps are available */
  async run() {
    const enc = this.device.createCommandEncoder();
    this.timing = true;
    this.encode(enc);
    if (this.timestamps) {
      enc.resolveQuerySet(this.qs, 0, 8, this.qbuf, 0);
      enc.copyBufferToBuffer(this.qbuf, 0, this.qread, 0, 64);
    }
    this.device.queue.submit([enc.finish()]);
    await this.device.queue.onSubmittedWorkDone();
    if (!this.timestamps) return null;
    await this.qread.mapAsync(GPUMapMode.READ);
    const t = new BigUint64Array(this.qread.getMappedRange().slice(0));
    this.qread.unmap();
    const ms = i => (t[i + 1] > t[i] ? Number(t[i + 1] - t[i]) / 1e6 : 0);
    return { arLuma: ms(0), arChroma: ms(2), applyY: ms(4), applyC: ms(6) };
  }

  /** the grained picture as packed planes (Y, U, V; 8-bit bytes or 16-bit little-endian), like av1grain decode */
  async readPacked() {
    const dev = this.device, bytes = this.dstSamples * 2;
    if (!this.readBuf || this.readBuf.size < bytes) {
      if (this.readBuf) this.readBuf.destroy();
      this.readBuf = dev.createBuffer({ size: bytes, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    }
    const enc = dev.createCommandEncoder();
    enc.copyBufferToBuffer(this.dst, 0, this.readBuf, 0, bytes);
    dev.queue.submit([enc.finish()]);
    await this.readBuf.mapAsync(GPUMapMode.READ);
    const s16 = new Uint16Array(this.readBuf.getMappedRange(0, bytes).slice(0));
    this.readBuf.unmap();
    const P = this.params, hbd = this.fmt.bitdepth > 8;
    const planes = [[I.DOY, I.DSY, this.W, this.H]];
    if (!this.mono) planes.push([I.DOU, I.DSC, this.CW, this.CH], [I.DOV, I.DSC, this.CW, this.CH]);
    const total = planes.reduce((a, [, , w, h]) => a + w * h, 0);
    const out = hbd ? new Uint16Array(total) : new Uint8Array(total);
    let o = 0;
    for (const [off, st, w, h] of planes)
      for (let y = 0; y < h; y++) { out.set(s16.subarray(P[off] + y * P[st], P[off] + y * P[st] + w), o); o += w; }
    return new Uint8Array(out.buffer);
  }
}
