// av1-fullgrain: AV1 film grain drawn on the GPU (WGSL compute shaders), bit-identical to the
// decoder's C code (decoder/dav1d). One source for the browser kit (browser/web/gpu-grain.mjs, through
// the generated browser/web/grain-wgsl.mjs) and the native renderer (gpu/, wgpu: Vulkan, Metal, DX12).
//
//   full      full-frame grain (src/fg_variant_tmpl.c, fgv_full_row): hashed white noise, the
//             signalled AR filter over the whole frame, restarted for each group of 4 x 32 rows
//             16 rows above it.
//   standard  AV1 grain as the specification defines it (src/filmgrain_tmpl.c): an 82 x 73 template
//             per plane (LFSR noise + AR filter, run as a wavefront in workgroup memory), 32 x 32
//             blocks copied from it at per-block LFSR offsets, overlap blending at the block seams.
//
// Full-frame AR: one workgroup per band group and plane, one thread per row. A row's AR filter
// depends on its own previous samples, so rows run as a wavefront: in step t, row r filters columns
// [(t - K r) S, (t - K r + 1) S). The rows above are read from a ring of the last RING steps' results
// in workgroup memory (the taps reach back at most 7 steps), into a 3 x 7 register window that slides
// along the row. Global memory only receives the finished field (row-major, two 16-bit samples per
// word) for the next passes: storageBarrier() did not make one thread's global writes visible to the
// others reliably (Chromium / Vulkan / NVIDIA), workgroupBarrier() on workgroup memory does.
//
// Only integer arithmetic, so the result is the same on every GPU.
//
// The host fills the parameter block U (P_* indices below) and the tables, then runs, per picture:
//   full:     ar_luma (NG groups), lumaterm (if a chroma AR has a luma tap), ar_chroma (NG x planes)
//   standard: std_tpl_luma, std_tpl_chroma, std_offsets
//   then:     apply_y, apply_c
// The constants below are read by the hosts (gpu-grain.mjs parses every "const NAME = number;" line;
// the Rust crate checks its copies against them in a test): keep that form.
//
// Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause
// Ported from dav1d's C code (© VideoLAN and the dav1d authors, BSD-2-Clause) and av1-fullgrain's full-frame grain.

const ROWS = 144;      // rows of one band group: 16 warm-up + 4 x 32
const S = 4;           // columns per wavefront step
const K = 2;           // steps of skew per row (K = 2 needs S >= 3)
const RING = 8;        // steps kept in workgroup memory (the oldest tap is 7 steps back)
const TPL = 6068;      // one standard-mode template, (73 + 1) x 82 samples, as dav1d allocates it
const GSTRIDE = 24;    // AR taps per plane in the parameter block
const NPARAM = 144;    // parameter block size (i32)

// parameter block indices
const P_W = 0;          // picture width
const P_H = 1;          // picture height
const P_SSX = 2;        // chroma subsampling x
const P_SSY = 3;        // chroma subsampling y
const P_BD = 4;         // bit depth
const P_CW = 5;         // chroma width
const P_CH = 6;         // chroma height
const P_NG = 7;         // band groups (128 rows each)
const P_LAG = 8;        // ar_coeff_lag
const P_ARSH = 9;       // ar_coeff_shift
const P_GMIN = 10;      // grain minimum
const P_GMAX = 11;      // grain maximum
const P_HASY = 12;      // luma grain on
const P_HASU = 13;      // Cb grain on
const P_HASV = 14;      // Cr grain on
const P_KEYY = 15;      // full-frame noise key, luma
const P_KEYU = 16;      // ... Cb
const P_KEYV = 17;      // ... Cr
const P_SCSH = 18;      // scaling_shift
const P_YMIN = 19;      // luma clip minimum
const P_YMAX = 20;      // luma clip maximum
const P_CMIN = 21;      // chroma clip minimum
const P_CMAX = 22;      // chroma clip maximum
const P_CSFL = 23;      // chroma_scaling_from_luma
const P_ULM = 24;       // Cb luma multiplier
const P_UM = 25;        // Cb multiplier
const P_UOFF = 26;      // Cb offset (scaled to the bit depth)
const P_VLM = 27;       // Cr luma multiplier
const P_VM = 28;        // Cr multiplier
const P_VOFF = 29;      // Cr offset
const P_LCU = 30;       // Cb AR luma coefficient
const P_LCV = 31;       // Cr AR luma coefficient
const P_RSL = 32;       // luma field rows per group
const P_LPITCH = 33;    // luma field pitch (samples)
const P_LBW = 34;       // luma field width
const P_NCHL = 35;      // luma wavefront steps per row
const P_RSC = 36;       // chroma field rows per group
const P_CPITCH = 37;    // chroma field pitch
const P_CBW = 38;       // chroma field width
const P_NCHC = 39;      // chroma wavefront steps per row
const P_SOY = 40;       // source luma offset (bytes)
const P_SSY_ = 41;      // source luma stride (bytes)
const P_SOU = 42;       // source Cb offset
const P_SSU = 43;       // source Cb stride
const P_SOV = 44;       // source Cr offset
const P_SSV = 45;       // source Cr stride
const P_BPS = 46;       // source bytes per sample
const P_DOY = 47;       // output luma offset (samples)
const P_DOU = 48;       // output Cb offset
const P_DOV = 49;       // output Cr offset
const P_DSY = 50;       // output luma stride (samples)
const P_DSC = 51;       // output chroma stride
const P_MONO = 52;      // monochrome
const P_SCPU = 53;      // Cb scaling LUT (0 luma, 1 Cb)
const P_SCPV = 54;      // Cr scaling LUT (0 luma, 2 Cr)
const P_PMAX = 55;      // largest sample value
const P_CPLANE = 56;    // first chroma plane with grain (ar_chroma)
const P_NTC = 57;       // luma-term steps per group
const P_MODE = 58;      // 0 full-frame, 1 standard
const P_SEED = 59;      // grain_seed
const P_OVL = 60;       // overlap_flag
const P_NBR = 61;       // standard: 32x32 block rows
const P_NBC = 62;       // standard: 32x32 block columns
const P_STEPY = 63;     // source luma sample step (bytes)
const P_GRID = 64;      // AR taps, 24 per plane: 3 x 7 previous-row taps (row -3 first, zero outside the lag), then columns -1, -2, -3
const P_STEPC = 136;    // source chroma sample step (bytes; 2x for NV12)


struct Params { v: array<vec4<i32>, NPARAM / 4> }
@group(0) @binding(0) var<uniform> U: Params;
@group(0) @binding(1) var<storage, read> tabs: array<i32>;        // Gaussian table (2048), then 3 scaling LUTs (4096 each)
@group(0) @binding(2) var<storage, read_write> FL: array<u32>;
@group(0) @binding(3) var<storage, read_write> FC: array<u32>;
@group(0) @binding(4) var<storage, read_write> SB: array<i32>;   // standard mode: 3 templates, then the block offsets
@group(0) @binding(5) var<storage, read> src: array<u32>;
@group(0) @binding(6) var<storage, read_write> dst: array<u32>;
@group(0) @binding(7) var<storage, read_write> LT: array<i32>;

fn p(i: i32) -> i32 { return U.v[i >> 2][i & 3]; }

fn hash(x0: u32) -> u32 {
  var x = x0;
  x = x ^ (x >> 16u); x = x * 0x7feb352du;
  x = x ^ (x >> 15u); x = x * 0x846ca68bu;
  return x ^ (x >> 16u);
}

fn round2(x: i32, sh: i32) -> i32 { return (x + ((1 << u32(sh)) >> 1u)) >> u32(sh); }

// the finished fields: row-major per band group, two 16-bit samples per word
fn lword(g: i32, r: i32, c: i32) -> i32 { return ((g * p(P_RSL) + r) * p(P_LPITCH) + c) >> 1u; }
fn cword(pl: i32, g: i32, r: i32, c: i32) -> i32 {
  let n = p(P_NG) * p(P_RSC) * p(P_CPITCH);
  return ((pl - 1) * n + (g * p(P_RSC) + r) * p(P_CPITCH) + c) >> 1u;
}
fn half(w: u32, c: i32) -> i32 {
  if ((c & 1) == 0) { return bitcast<i32>(w << 16u) >> 16u; }
  return bitcast<i32>(w) >> 16u;
}
fn lfield(g: i32, r: i32, c: i32) -> i32 { return half(FL[lword(g, r, c)], c); }
fn cfield(pl: i32, g: i32, r: i32, c: i32) -> i32 { return half(FC[cword(pl, g, r, c)], c); }

// the chroma AR's luma term, round2(sum of the 1, 2 or 4 luma grain samples, ssx + ssy), for chroma (r, c) of
// group g; stored in the order the AR reads it (step-major: the rows of one step are adjacent)
fn ltidx(g: i32, r: i32, c: i32) -> i32 {
  return ((g * p(P_NTC) + c / S + K * r) * S + c % S) * p(P_RSC) + r;
}

@compute @workgroup_size(64)
fn lumaterm(@builtin(global_invocation_id) id: vec3<u32>) {
  let c = i32(id.x); let r = i32(id.y); let g = i32(id.z);
  let ssx = p(P_SSX); let ssy = p(P_SSY);
  let bhc = (min(p(P_H) - g * 128, 128) + ssy) >> u32(ssy);
  if (c >= p(P_CBW) || r >= (16 >> u32(ssy)) + bhc) { return; }
  let rl = r << u32(ssy); let cl = c << u32(ssx);
  var l = lfield(g, rl, cl);
  if (ssx != 0) { l += lfield(g, rl, cl + 1); }
  if (ssy != 0) {
    l += lfield(g, rl + 1, cl);
    if (ssx != 0) { l += lfield(g, rl + 1, cl + 1); }
  }
  let ssum = u32(ssx + ssy);
  LT[ltidx(g, r, c)] = (l + ((1 << ssum) >> 1u)) >> ssum;
}

// the last RING steps' results of every row: [step % RING][column % S / 2][row], 2 samples per word
var<workgroup> ring: array<u32, RING * (S / 2) * ROWS>;

fn rread(r: i32, c: i32) -> i32 {
  let slot = (c / S + K * r) & (RING - 1);
  return half(ring[(slot * (S / 2) + ((c % S) >> 1u)) * ROWS + r], c);
}

// one band group of one plane: the AR recursion of fgv_full_row / ar_band
fn ar(pl: i32, g: i32, r: i32) {
  let H = p(P_H); let ssx = p(P_SSX); let ssy = p(P_SSY);
  let Y0 = g * 128;
  let bh = min(H - Y0, 128);
  let bhc = (bh + ssy) >> u32(ssy);
  let hasC = p(P_HASU) != 0 || p(P_HASV) != 0;
  var R: i32; var bw: i32; var y0: i32; var x0: i32; var nch: i32; var key: u32; var lc = 0;
  if (pl == 0) {
    R = 16 + max(bh, select(0, bhc << u32(ssy), hasC));
    bw = p(P_LBW); y0 = Y0 - 16; x0 = -16; nch = p(P_NCHL);
    key = bitcast<u32>(p(P_KEYY));
  } else {
    let Pc = 16 >> u32(ssy);
    R = Pc + bhc;
    bw = p(P_CBW); y0 = (Y0 >> u32(ssy)) - Pc; x0 = -(16 >> u32(ssx)); nch = p(P_NCHC);
    key = bitcast<u32>(p(P_KEYY + pl));
    lc = p(P_LCU + pl - 1);
  }
  let G = P_GRID + GSTRIDE * pl;
  let k00 = p(G + 0); let k01 = p(G + 1); let k02 = p(G + 2); let k03 = p(G + 3); let k04 = p(G + 4); let k05 = p(G + 5); let k06 = p(G + 6);
  let k10 = p(G + 7); let k11 = p(G + 8); let k12 = p(G + 9); let k13 = p(G + 10); let k14 = p(G + 11); let k15 = p(G + 12); let k16 = p(G + 13);
  let k20 = p(G + 14); let k21 = p(G + 15); let k22 = p(G + 16); let k23 = p(G + 17); let k24 = p(G + 18); let k25 = p(G + 19); let k26 = p(G + 20);
  let c1 = p(G + 21); let c2 = p(G + 22); let c3 = p(G + 23);
  let sh = u32(p(P_ARSH));
  let gmin = p(P_GMIN); let gmax = p(P_GMAX);
  let rnd = (1 << sh) >> 1u;
  let rk = hash(key ^ hash(bitcast<u32>(y0 + r) * 0x9E3779B9u + 0x7f4a7c15u));
  let ar_row = r >= 3 && r < R;
  // window: a = row r - 1, b = r - 2, d = r - 3; position j holds column c - 3 + j
  var a0 = 0; var a1 = 0; var a2 = 0; var a3 = 0; var a4 = 0; var a5 = 0; var a6 = 0;
  var b0 = 0; var b1 = 0; var b2 = 0; var b3 = 0; var b4 = 0; var b5 = 0; var b6 = 0;
  var d0 = 0; var d1 = 0; var d2 = 0; var d3 = 0; var d4 = 0; var d5 = 0; var d6 = 0;
  var p1 = 0; var p2 = 0; var p3 = 0;
  var pend = 0u;
  let steps = nch + K * (R - 1);
  for (var t = 0; t < steps; t++) {
    let q = t - K * r;
    if (r < R && q >= 0 && q < nch) {
      if (q == 0 && ar_row) {
        a4 = rread(r - 1, 0); a5 = rread(r - 1, 1); a6 = rread(r - 1, 2);
        b4 = rread(r - 2, 0); b5 = rread(r - 2, 1); b6 = rread(r - 2, 2);
        d4 = rread(r - 3, 0); d5 = rread(r - 3, 1); d6 = rread(r - 3, 2);
      }
      var n = 0;
      for (var i = 0; i < S; i++) {
        let c = q * S + i;
        if (c >= bw) { break; }
        a0 = a1; a1 = a2; a2 = a3; a3 = a4; a4 = a5; a5 = a6;
        b0 = b1; b1 = b2; b2 = b3; b3 = b4; b4 = b5; b5 = b6;
        d0 = d1; d1 = d2; d2 = d3; d3 = d4; d4 = d5; d5 = d6;
        if (ar_row && c + 3 < bw) {
          a6 = rread(r - 1, c + 3); b6 = rread(r - 2, c + 3); d6 = rread(r - 3, c + 3);
        }
        let x = x0 + c;
        let h = hash(rk + bitcast<u32>(x >> 1u) * 0x85ebca6bu);
        var v = tabs[select((h >> 5u) & 0x7ffu, h >> 21u, (x & 1) != 0)];
        if (ar_row && c >= 3 && c < bw - 3) {
          var sum = k20 * a0 + k21 * a1 + k22 * a2 + k23 * a3 + k24 * a4 + k25 * a5 + k26 * a6
            + k10 * b0 + k11 * b1 + k12 * b2 + k13 * b3 + k14 * b4 + k15 * b5 + k16 * b6
            + k00 * d0 + k01 * d1 + k02 * d2 + k03 * d3 + k04 * d4 + k05 * d5 + k06 * d6;
          if (lc != 0) { sum += LT[ltidx(g, r, c)] * lc; }
          sum += c3 * p3 + c2 * p2 + c1 * p1 + rnd;
          v = clamp(v + (sum >> sh), gmin, gmax);
        }
        p3 = p2; p2 = p1; p1 = v;
        if ((i & 1) == 0) {
          pend = u32(v) & 0xffffu;
        } else {
          let w = pend | (u32(v) << 16u);
          ring[((t & (RING - 1)) * (S / 2) + (i >> 1u)) * ROWS + r] = w;
          if (pl == 0) { FL[lword(g, r, c - 1)] = w; } else { FC[cword(pl, g, r, c - 1)] = w; }
        }
        n++;
      }
      if ((n & 1) != 0) {            // the row's last column, alone in its word
        let c = q * S + n - 1;
        ring[((t & (RING - 1)) * (S / 2) + ((n - 1) >> 1u)) * ROWS + r] = pend;
        if (pl == 0) { FL[lword(g, r, c)] = pend; } else { FC[cword(pl, g, r, c)] = pend; }
      }
    }
    workgroupBarrier();
  }
}

@compute @workgroup_size(ROWS)
fn ar_luma(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
  ar(0, i32(wg.x), i32(li));
}

@compute @workgroup_size(ROWS)
fn ar_chroma(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
  ar(p(P_CPLANE) + i32(wg.y), i32(wg.x), i32(li));
}

// a sample of the input picture (8 or 16 bits, any offsets / strides in bytes)
fn rsrc(pl: i32, x: i32, y: i32) -> i32 {
  let bps = p(P_BPS);
  let b = p(P_SOY + 2 * pl) + y * p(P_SSY_ + 2 * pl) + x * select(p(P_STEPC), p(P_STEPY), pl == 0);
  let w = src[b >> 2u];
  if (bps == 1) { return i32((w >> (u32(b & 3) * 8u)) & 0xffu); }
  return i32((w >> (u32(b & 2) * 8u)) & 0xffffu);
}

fn noise(sc: i32, lut: i32, gr: i32) -> i32 {
  return round2(tabs[2048 + lut * 4096 + sc] * gr, p(P_SCSH));
}

// ---- standard AV1 grain (src/filmgrain_tmpl.c)

fn lfsr(st: ptr<function, u32>, bits: u32) -> u32 {
  let r = *st;
  let bit = (r ^ (r >> 1u) ^ (r >> 3u) ^ (r >> 12u)) & 1u;
  *st = (r >> 1u) | (bit << 15u);
  return (*st >> (16u - bits)) & ((1u << bits) - 1u);
}

// the template of one plane, packed two samples per word (41 words per row), in workgroup memory
var<workgroup> tw: array<u32, 74 * 41>;

fn std_tpl(pl: i32, r: i32) {
  let ssx = select(0, p(P_SSX), pl > 0); let ssy = select(0, p(P_SSY), pl > 0);
  let TW = select(82, 44, ssx != 0); let TH = select(73, 38, ssy != 0);
  if (r == 0) {                                          // LFSR noise, raster order (one thread: it is serial)
    var st = u32(p(P_SEED)) ^ select(select(0u, 0xb524u, pl == 1), 0x49d8u, pl == 2);
    for (var y = 0; y < TH; y++) {
      for (var x = 0; x < TW; x += 2) {
        let v0 = tabs[lfsr(&st, 11u)]; let v1 = tabs[lfsr(&st, 11u)];
        tw[y * 41 + (x >> 1u)] = (u32(v0) & 0xffffu) | (u32(v1) << 16u);
      }
    }
  }
  workgroupBarrier();
  // AR filter: rows [3, TH) x columns [3, TW - 3); row r handles column t - 5 r in step t. A thread writes a word
  // when both its samples are done, so a row's sample is readable 1 step late: skew 5 (lag 3 needs 4) covers it.
  let G = P_GRID + GSTRIDE * pl;
  let k00 = p(G + 0); let k01 = p(G + 1); let k02 = p(G + 2); let k03 = p(G + 3); let k04 = p(G + 4); let k05 = p(G + 5); let k06 = p(G + 6);
  let k10 = p(G + 7); let k11 = p(G + 8); let k12 = p(G + 9); let k13 = p(G + 10); let k14 = p(G + 11); let k15 = p(G + 12); let k16 = p(G + 13);
  let k20 = p(G + 14); let k21 = p(G + 15); let k22 = p(G + 16); let k23 = p(G + 17); let k24 = p(G + 18); let k25 = p(G + 19); let k26 = p(G + 20);
  let c1 = p(G + 21); let c2 = p(G + 22); let c3 = p(G + 23);
  let lc = select(0, p(P_LCU + pl - 1), pl > 0);
  let sh = u32(p(P_ARSH)); let rnd = (1 << sh) >> 1u;
  let gmin = p(P_GMIN); let gmax = p(P_GMAX);
  var p1 = 0; var p2 = 0; var p3 = 0; var hold = 0u;
  let steps = TW + 5 * (TH - 1);
  for (var t = 0; t < steps; t++) {
    let c = t - 5 * r;
    if (r >= 3 && r < TH && c >= 0 && c < TW) {
      let w0 = tw[r * 41 + (c >> 1u)];
      var v = half(w0, c);
      if (c >= 3 && c < TW - 3) {
        var sum = 0;
        sum += k00 * half(tw[(r - 3) * 41 + ((c + -3) >> 1u)], c + -3);
        sum += k01 * half(tw[(r - 3) * 41 + ((c + -2) >> 1u)], c + -2);
        sum += k02 * half(tw[(r - 3) * 41 + ((c + -1) >> 1u)], c + -1);
        sum += k03 * half(tw[(r - 3) * 41 + ((c + 0) >> 1u)], c + 0);
        sum += k04 * half(tw[(r - 3) * 41 + ((c + 1) >> 1u)], c + 1);
        sum += k05 * half(tw[(r - 3) * 41 + ((c + 2) >> 1u)], c + 2);
        sum += k06 * half(tw[(r - 3) * 41 + ((c + 3) >> 1u)], c + 3);
        sum += k10 * half(tw[(r - 2) * 41 + ((c + -3) >> 1u)], c + -3);
        sum += k11 * half(tw[(r - 2) * 41 + ((c + -2) >> 1u)], c + -2);
        sum += k12 * half(tw[(r - 2) * 41 + ((c + -1) >> 1u)], c + -1);
        sum += k13 * half(tw[(r - 2) * 41 + ((c + 0) >> 1u)], c + 0);
        sum += k14 * half(tw[(r - 2) * 41 + ((c + 1) >> 1u)], c + 1);
        sum += k15 * half(tw[(r - 2) * 41 + ((c + 2) >> 1u)], c + 2);
        sum += k16 * half(tw[(r - 2) * 41 + ((c + 3) >> 1u)], c + 3);
        sum += k20 * half(tw[(r - 1) * 41 + ((c + -3) >> 1u)], c + -3);
        sum += k21 * half(tw[(r - 1) * 41 + ((c + -2) >> 1u)], c + -2);
        sum += k22 * half(tw[(r - 1) * 41 + ((c + -1) >> 1u)], c + -1);
        sum += k23 * half(tw[(r - 1) * 41 + ((c + 0) >> 1u)], c + 0);
        sum += k24 * half(tw[(r - 1) * 41 + ((c + 1) >> 1u)], c + 1);
        sum += k25 * half(tw[(r - 1) * 41 + ((c + 2) >> 1u)], c + 2);
        sum += k26 * half(tw[(r - 1) * 41 + ((c + 3) >> 1u)], c + 3);
        if (lc != 0) {
          let lx = ((c - 3) << u32(ssx)) + 3; let ly = ((r - 3) << u32(ssy)) + 3;
          var l = 0;
          for (var i = 0; i <= ssy; i++) { for (var j = 0; j <= ssx; j++) { l += SB[(ly + i) * 82 + lx + j]; } }
          sum += round2(l, ssx + ssy) * lc;
        }
        sum += c3 * p3 + c2 * p2 + c1 * p1 + rnd;
        v = clamp(v + (sum >> sh), gmin, gmax);
      }
      p3 = p2; p2 = p1; p1 = v;
      if ((c & 1) == 0) { hold = u32(v) & 0xffffu; } else { tw[r * 41 + (c >> 1u)] = hold | (u32(v) << 16u); }
    }
    workgroupBarrier();
  }
  for (var i = r; i < TH * TW; i += 128) {
    let y = i / TW; let x = i % TW;
    SB[pl * TPL + y * 82 + x] = half(tw[y * 41 + (x >> 1u)], x);
  }
}

@compute @workgroup_size(128)
fn std_tpl_luma(@builtin(local_invocation_index) li: u32) { std_tpl(0, i32(li)); }

@compute @workgroup_size(128)
fn std_tpl_chroma(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
  std_tpl(p(P_CPLANE) + i32(wg.x), i32(li));
}

// the 8-bit block offsets of every 32 x 32 block: one LFSR per block row, one number per block
@compute @workgroup_size(64)
fn std_offsets(@builtin(global_invocation_id) id: vec3<u32>) {
  let R = i32(id.x);
  if (R >= p(P_NBR)) { return; }
  let s = u32(p(P_SEED));
  var st = s ^ ((u32(R * 37 + 178) & 0xffu) << 8u) ^ (u32(R * 173 + 105) & 0xffu);
  let n = p(P_NBC);
  for (var c = 0; c < n; c++) { SB[(3 * TPL) + R * n + c] = i32(lfsr(&st, 8u)); }
}

// sample_lut(): block (C - bxo, R - byo)'s template at (x, y) of the current block, + its overlap extension
fn slut(pl: i32, ssx: i32, ssy: i32, R: i32, C: i32, bxo: i32, byo: i32, x: i32, y: i32) -> i32 {
  let off = SB[(3 * TPL) + (R - byo) * p(P_NBC) + C - bxo];
  let ox = 3 + (2 >> u32(ssx)) * (3 + (off >> 4u));
  let oy = 3 + (2 >> u32(ssy)) * (3 + (off & 15));
  return SB[pl * TPL + (oy + y + (32 >> u32(ssy)) * byo) * 82 + ox + x + (32 >> u32(ssx)) * bxo];
}

fn blend(old: i32, cur: i32, w0: i32, w1: i32) -> i32 {
  return clamp(round2(old * w0 + cur * w1, 5), p(P_GMIN), p(P_GMAX));
}

// overlap weights: w[sub][pos] -> (old, new)
fn wold(sub: i32, i: i32) -> i32 { return select(select(17, 27, i == 0), 23, sub != 0); }
fn wnew(sub: i32, i: i32) -> i32 { return select(select(27, 17, i == 0), 22, sub != 0); }

// the grain of plane pl at (x, y): full-frame field or standard blocks
fn grain_at(pl: i32, x: i32, y: i32) -> i32 {
  let ssx = select(0, p(P_SSX), pl > 0); let ssy = select(0, p(P_SSY), pl > 0);
  if (p(P_MODE) == 0) {
    if (pl == 0) { let g = y / 128; return lfield(g, 16 + y - g * 128, 16 + x); }
    let g = (y << u32(ssy)) / 128;
    return cfield(pl, g, (16 >> u32(ssy)) + y - ((g * 128) >> u32(ssy)), (16 >> u32(ssx)) + x);
  }
  let bw = 32 >> u32(ssx); let bh = 32 >> u32(ssy);
  let R = y / bh; let C = x / bw; let lx = x - C * bw; let ly = y - R * bh;
  let ov = p(P_OVL) != 0;
  let vx = ov && C > 0 && lx < (2 >> u32(ssx));
  let vy = ov && R > 0 && ly < (2 >> u32(ssy));
  var g = slut(pl, ssx, ssy, R, C, 0, 0, lx, ly);
  if (vx) { g = blend(slut(pl, ssx, ssy, R, C, 1, 0, lx, ly), g, wold(ssx, lx), wnew(ssx, lx)); }
  if (vy) {
    var top = slut(pl, ssx, ssy, R, C, 0, 1, lx, ly);
    if (vx) { top = blend(slut(pl, ssx, ssy, R, C, 1, 1, lx, ly), top, wold(ssx, lx), wnew(ssx, lx)); }
    g = blend(top, g, wold(ssy, ly), wnew(ssy, ly));
  }
  return g;
}

@compute @workgroup_size(16, 8)
fn apply_y(@builtin(global_invocation_id) id: vec3<u32>) {
  let W = p(P_W); let H = p(P_H);
  let x0 = i32(id.x) * 2; let y = i32(id.y);
  if (x0 >= W || y >= H) { return; }
  var word = 0u;
  for (var k = 0; k < 2; k++) {
    let x = x0 + k;
    if (x >= W) { break; }
    let s = rsrc(0, x, y);
    var o = s;
    if (p(P_HASY) != 0) {
      o = clamp(s + noise(s, 0, grain_at(0, x, y)), p(P_YMIN), p(P_YMAX));
    }
    word |= u32(o) << (16u * u32(k));
  }
  dst[(p(P_DOY) + y * p(P_DSY) + x0) >> 1u] = word;
}

@compute @workgroup_size(16, 8)
fn apply_c(@builtin(global_invocation_id) id: vec3<u32>) {
  let pl = 1 + i32(id.z);
  let CW = p(P_CW); let CH = p(P_CH); let W = p(P_W);
  let ssx = p(P_SSX); let ssy = p(P_SSY);
  let x0 = i32(id.x) * 2; let y = i32(id.y);
  if (x0 >= CW || y >= CH) { return; }
  let has = p(P_HASY + pl) != 0;
  let lm = p(P_ULM + 3 * (pl - 1)); let m = p(P_UM + 3 * (pl - 1)); let off = p(P_UOFF + 3 * (pl - 1));
  let lut = p(P_SCPU + pl - 1);
  var word = 0u;
  for (var k = 0; k < 2; k++) {
    let x = x0 + k;
    if (x >= CW) { break; }
    let s = rsrc(pl, x, y);
    var o = s;
    if (has) {
      let lx = x << u32(ssx); let ly = y << u32(ssy);
      var avg = rsrc(0, lx, ly);
      if (ssx != 0) { avg = (avg + rsrc(0, min(lx + 1, W - 1), ly) + 1) >> 1u; }
      var val = avg;
      if (p(P_CSFL) == 0) { val = clamp(((avg * lm + s * m) >> 6u) + off, 0, p(P_PMAX)); }
      let gr = grain_at(pl, x, y);
      o = clamp(s + noise(val, lut, gr), p(P_CMIN), p(P_CMAX));
    }
    word |= u32(o) << (16u * u32(k));
  }
  let dof = select(p(P_DOV), p(P_DOU), pl == 1);
  dst[(dof + y * p(P_DSC) + x0) >> 1u] = word;
}
