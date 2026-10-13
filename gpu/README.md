# av1-fullgrain-gpu

**AV1 film grain drawn on the GPU, over the whole frame.** You give it decoded pictures without grain and each
picture's film grain parameters; it gives back the pictures with **full-frame grain**, exactly as av1-fullgrain's
decoder draws it, bit for bit. It can also draw standard AV1 grain (the tiled kind), identical to stock dav1d.

It runs on Vulkan, Metal or DX12 through [wgpu](https://wgpu.rs), so on NVIDIA, AMD, Intel and Apple GPUs, on Linux,
Windows and macOS. Nothing has to be installed in place of the system's dav1d: any decoder that can leave the grain
out works, including hardware decoders.

It comes as:

- a **command line tool**, `av1-fullgrain-gpu`: decodes a video with ffmpeg and writes y4m with full-frame grain, or
  adds the grain to a y4m stream;
- a **Rust library** (`av1_fullgrain_gpu`);
- a **C library** with a small header, [include/av1_fullgrain_gpu.h](include/av1_fullgrain_gpu.h), that takes
  dav1d's own `Dav1dFilmGrainData` as it is.

The shaders are the [browser player](../browser/README.md)'s: both use [src/grain.wgsl](src/grain.wgsl).

## Build

Needs Rust 1.87 or newer.

```sh
cd gpu && cargo build --release
# -> target/release/av1-fullgrain-gpu                 the command line tool
#    target/release/libav1_fullgrain_gpu.{so,dylib,dll} and .a / .lib   the library
```

`decode` and `check` also need `ffmpeg` and `ffprobe` (with libdav1d) on the `PATH`, or `--ffmpeg PATH`.

## Command line

```sh
av1-fullgrain-gpu decode IN.mkv -o OUT.y4m              # full-frame grain; - for stdout
av1-fullgrain-gpu decode IN.mkv --ss 61.5 --frames 120 -o scene.y4m
av1-fullgrain-gpu decode IN.mkv --hwaccel cuda -o -     # hardware decoding (cuda, vaapi, videotoolbox, d3d11va)
av1-fullgrain-gpu decode IN.mkv --mode standard -o -    # standard AV1 grain, as stock dav1d
av1-fullgrain-gpu render --params PARAMS.json -i in.y4m -o out.y4m
av1-fullgrain-gpu adapters                              # the GPUs it can use; --adapter NAME picks one
```

**`decode`** runs ffmpeg with `-export_side_data film_grain`: ffmpeg's AV1 decoder then leaves the grain out and
passes its parameters along with each picture. `av1-fullgrain-gpu` reads them, draws the grain and writes the
pictures at their own bit depth (y4m, or raw planes with `--raw`). `--params-out FILE` also writes the grain of every
picture as a [PARAMS.json](../tools/av1grain/README.md). Video only: audio and subtitles stay in the original file.

Where the parameters come from (`--params-from`):
- `showinfo`: ffmpeg's side data, as its `showinfo` filter prints it. ffmpeg 7.0 and newer print all of it; older
  versions print only the seed.
- `bitstream`: the AV1 frame headers, read by the crate's own header parser (a second ffmpeg remuxes the video to
  IVF for it). Works with any ffmpeg; every picture's seed is checked against the decoder's, so the two can't drift
  apart. `--ss` needs `showinfo`.
- `auto` (default): `bitstream` with `--hwaccel` or with ffmpeg before 7.0, else `showinfo`. With `--hwaccel` and an
  explicit `showinfo`, the side data is also compared with the bitstream, picture by picture.

A picture whose parameters can't be read stops `decode` at that picture.

**`render`** adds grain to grain-free y4m. The grain comes from a PARAMS.json (`av1grain strip`, or `decode
--params-out`), or from an ffmpeg log of the `showinfo` filter on the grain-free decode:

```sh
ffmpeg -c:v libdav1d -export_side_data film_grain -i IN.mkv -vf showinfo -f yuv4mpegpipe -strict -1 nograin.y4m 2> grain.log
av1-fullgrain-gpu render --params grain.log -i nograin.y4m -o out.y4m
```

Some uses:

```sh
# watch a clip with full-frame grain in mpv, without installing the decoder (no seeking: it's a pipe)
av1-fullgrain-gpu decode clip.mkv | mpv --audio-file=clip.mkv -
# encode from the grain as the viewer sees it, e.g. a lossless reference for a comparison
av1-fullgrain-gpu decode clip.mkv | ffmpeg -i - -c:v ffv1 clip.fullgrain.mkv
```

> [!NOTE]
> **An av1-fullgrain decoder installed as the system's dav1d** draws its own grain even when ffmpeg asks for none,
> unless `DAV1D_GRAIN_FORCE=0` is set. `av1-fullgrain-gpu` sets it for the ffmpeg it starts; set it yourself when you
> make the grain-free decode for `render`.

## Library

Rust:

```rust
use av1_fullgrain_gpu::{GrainMode, Options, Plane, Renderer};
let mut r = Renderer::new(&Options::default())?;
// format: size, bit depth (8, 10, 12), layout (4:0:0, 4:2:0, 4:2:2, 4:4:4), matrix
// planes: the decoded grain-free picture; grain: Option<&FilmGrainParams> (dav1d's values)
r.render(&format, &planes, grain, GrainMode::Full, &mut out)?;      // or submit() / receive(): pictures in flight
```

`FilmGrainParams` has the layout of dav1d's `Dav1dFilmGrainData`. `ParamsFile` reads a PARAMS.json;
`parse_showinfo_line` reads ffmpeg's side data as `showinfo` prints it; `obu::Av1Parser` reads the grain of each shown
frame from the AV1 bitstream, one temporal unit at a time (`obu::IvfReader` for IVF).

C, with libdav1d decoding (`apply_grain = 0`):

```c
#include "av1_fullgrain_gpu.h"
AV1FGRenderer *r = av1fg_open(NULL, 0, err, sizeof err);
AV1FGPicture in = { p.p.w, p.p.h, p.p.bpc, p.p.layout, p.seq_hdr->mtrx,
                    { p.data[0], p.data[1], p.data[2] }, { p.stride[0], p.stride[1], p.stride[1] } };
const Dav1dFilmGrainData *g = p.frame_hdr->film_grain.present ? &p.frame_hdr->film_grain.data : NULL;
av1fg_render(r, &in, (const AV1FGFilmGrainData *) g, AV1FG_MODE_FULL, &out);
```

The whole program is [examples/dav1d_gpu_grain.c](examples/dav1d_gpu_grain.c). `av1fg_submit` / `av1fg_receive`
keep several pictures in flight. The header states the safety contract: one thread at a time per renderer, positive
strides, output planes that don't overlap, and what stays valid for how long.

**Where the parameters come from matters in one case:** ffmpeg's own AV1 decoder (the one `-hwaccel` uses) exports
the chroma multipliers and offsets as coded, without removing their offsets (`uv_mult` 128 means 0); libdav1d exports
dav1d's signed values. Seen in ffmpeg 9.0. `decode --hwaccel` therefore reads the bitstream (or, with an explicit
`--params-from showinfo`, corrects the offsets with `SideDataFrom::FfmpegAv1` and compares with the bitstream).

## Exactness

`av1-fullgrain-gpu check` decodes each stream through the `decode` path (ffmpeg, grain left out, grain parameters from
the side data), draws the grain on the GPU, and compares every picture (SHA-256) with av1-fullgrain's dav1d decoding
the same stream with full-frame grain, and with standard grain. It also checks that the grain-free decode equals
dav1d's with grain off, and (`--av1grain`) that the parameters equal those dav1d's own parser reports.

```sh
gpu/tests/make-vectors.sh /tmp/vec                     # synthetic streams (ffmpeg, aomenc; rav1e, SvtAv1EncApp if present)
av1-fullgrain-gpu check --dav1d build/decoder/dav1d/tools/dav1d --av1grain build/tools/av1grain/av1grain \
    decoder/dav1d/tests/fullgrain/*.ivf /tmp/vec/*.ivf
```

Every picture identical, full-frame and standard grain:

| streams | what they cover | NVIDIA RTX 3090 (Vulkan) | AMD Radeon 2-CU iGPU (Vulkan, RADV) |
|---|---|---|---|
| the decoder's 7 grain test streams | 4:2:0 / 4:2:2 / 4:4:4, 8 and 10 bit, lag 2 and 3, overlap on and off, chroma from luma, both clip ranges, grain_scale_shift, an odd size | 48 / 48 pictures | 48 / 48 |
| aomenc's 16 grain test vectors, 8 and 10 bit, 640x360 (+ an odd size) | update_grain = 0 (parameters taken from a reference frame: vectors 5, 9, 11), show_existing_frame, a new seed every picture | 330 / 330 | 330 / 330 |
| aomenc grain tables, 8 and 10 bit, SDR and HDR PQ | two parameter sets in one stream, lag 2 and 3, update_grain = 0 referencing a slot without grain (all-zero parameters: no grain, as in dav1d) | 30 / 30 | 30 / 30 |
| HDR PQ / BT.2020 10-bit, identity matrix 4:4:4 with restricted-range clipping, monochrome | | 30 / 30 | 30 / 30 |
| rav1e (photon noise, lag 0), SVT-AV1 | other encoders' parameters | 20 / 20 | 20 / 20 |
| libaom's conformance streams `av1-1-b8-23-film_grain-50`, `av1-1-b10-23-film_grain-50` | | 20 / 20 | 20 / 20 |
| films: 4K HDR PQ 10-bit (two), 1080p SDR 10-bit (two), 3584x2160 10-bit | SVT-AV1 film grain encodes, 48 pictures each | 240 / 240 | 48 / 48 (two films, 24 each) |

Counts are per grain mode, with the parameters from showinfo (ffmpeg 9.0) and again from the bitstream parser: identical
either way. The parser itself gives the same parameters as dav1d's own frame header parser (`av1grain strip`) on 55
streams, 1,774 pictures (`AV1FG_TEST_PARAMS=DIR cargo test --release -- --ignored`, DIR holding `name.ivf` +
`name.json` pairs), and mutated streams and random OBUs only ever give an error, never a crash (`cargo test
fuzz_headers`, in a debug build so that integer overflow is caught; `AV1FG_FUZZ_ITERS` for more runs). CI checks the
decoder's test streams and the synthetic ones on Mesa's CPU Vulkan driver (lavapipe), with Ubuntu 24.04's ffmpeg 6.1,
so through the bitstream parser. The same with **hardware decoding** (`--hwaccel cuda`, NVDEC): the grain-free pictures equal
dav1d's, and the GPU grain equals dav1d's full-frame grain, on 10 streams (the 4K HDR and 1080p films, update_grain = 0,
chroma from luma, monochrome, SVT-AV1, rav1e), with the parameters from the bitstream and, cross-checked, from the side
data. NVDEC doesn't decode 4:4:4, and it pads odd picture sizes (`decode` stops with an error there).
The browser kit, now on the same shader file, still matches dav1d on every picture in Chromium (WebGPU on Vulkan,
NVIDIA).

Only integer arithmetic runs on the GPU, so other GPUs should give the same pictures; Apple (Metal) and Windows (DX12)
are not tested yet.

## Speed

GPU time per picture (timestamp queries) and pictures per second including upload and readback (3 pictures in
flight), 10-bit 4:2:0:

| | full-frame, GPU time | full-frame, pictures/s | standard, GPU time | standard, pictures/s |
|---|---:|---:|---:|---:|
| RTX 3090, 1080p | 1.8 ms | 478 | 0.6 ms | 1113 |
| RTX 3090, 4K | 3.0 ms | 249 | 0.7 ms | 235 |
| AMD Radeon 2-CU iGPU (Ryzen 9600X), 1080p | 6.6 ms | 136 | 2.7 ms | 294 |
| AMD Radeon 2-CU iGPU (Ryzen 9600X), 4K | 23.7 ms | 37 | 9.1 ms | 82 |

At 4K, pictures/s is bound by moving 25 MB per picture to the GPU and back, not by the grain. Even the smallest
integrated GPU draws 4K full-frame grain faster than 24 pictures/s; software AV1 decoding of 4K is usually the slower
part. (RTX 3090 measured idle; the iGPU numbers come from a busy machine.)

## Licence

BSD-2-Clause, like the rest of av1-fullgrain and like dav1d. The grain algorithms are ports of dav1d's C code
(© VideoLAN and the dav1d authors, BSD-2-Clause) and of av1-fullgrain's full-frame grain. The Rust dependencies
(wgpu, naga, pollster, serde_json, clap, sha2) are MIT or Apache-2.0. All of this can be used from GPL-3.0 programs.
