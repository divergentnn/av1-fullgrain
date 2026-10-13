# Full-frame grain without replacing dav1d

The [decoder](../../decoder/README.md) draws full-frame grain inside dav1d, so every app that uses the system's dav1d
gets it, but only when AV1 is decoded in software by that dav1d. The [GPU renderer](../../gpu/README.md) does the same
drawing on the GPU, after any decoder: it needs only the decoded picture without grain and the grain parameters that
came with it. Both give the same pictures, bit for bit.

This page lists where each one fits, what works today, and what could be built.

> [!IMPORTANT]
> **Judge AV1 film grain only with full-frame grain.** An AV1 encode with film grain synthesis carries a description of
> the grain, not the grain; the player makes it. Standard players cover the picture with copies of one small grain
> patch (32x32-pixel squares cut from an 82x73 patch). That repeating pattern is easy to see once noticed, and with
> some seeds the patch has clumps or holes that repeat across the whole frame. It is the player's grain, not the
> encode's: comparisons and quality verdicts made with it blame the encode for the decoder.
>
> **Check before you judge.** Pause on a flat, evenly lit area (sky, a wall, a dark background) and look at the grain
> at 1:1. Full-frame grain looks like film grain: random everywhere. Tiled grain shows a texture that repeats: a faint
> grid, diagonal streaks, or the same speckle in many places. If you see that, the player is not using full-frame
> grain (hardware decoding is the usual reason).

## What works today

| Where | How | Hardware decoding | Status |
|---|---|---|---|
| mpv, ffmpeg, ffplay, VLC and Kodi on Linux, Homebrew's mpv and ffmpeg on macOS | the decoder in place of the system's dav1d ([install guides](../../README.md#install)) | no: AV1 must be decoded in software | works |
| any player that reads y4m from a pipe, e.g. mpv | `av1-fullgrain-gpu decode FILE \| mpv --audio-file=FILE -` | yes (`--hwaccel cuda`, `vaapi`, `videotoolbox`, `d3d11va`) | works; no seeking (it's a pipe) |
| ffmpeg: encoding, filtering, comparisons | `av1-fullgrain-gpu decode FILE \| ffmpeg -i - ...` (ffmpeg exports the grain, the renderer draws it) | yes | works; video only, take audio from the file |
| your own program in C or C++ | the [C library](../../gpu/README.md#library): libdav1d with `apply_grain = 0`, then `av1fg_render` with dav1d's `Dav1dFilmGrainData` | yes, with any decoder that leaves the grain out | works ([example](../../gpu/examples/dav1d_gpu_grain.c)) |
| your own program in Rust | the [Rust library](../../gpu/README.md#library) | yes | works |
| web pages | the [browser player](../../browser/README.md): WebCodecs + WebGPU, same shaders | yes (the browser's decoder) | works |
| TVs, phones, streaming boxes | none: their decoders draw standard grain; [av1-grain-seed](../../tools/av1-grain-seed/README.md) replaces unlucky seeds | | standard grain only |

For `decode`, ffmpeg needs libdav1d (any version) for software decoding, or a hardware AV1 decoder. With an
av1-fullgrain decoder installed as the system's dav1d, `av1-fullgrain-gpu` sets `DAV1D_GRAIN_FORCE=0` for its ffmpeg,
so the decoder leaves the grain to the renderer.

## What could be built

| Where | How | Effort | Notes |
|---|---|---|---|
| **mpv with hardware decoding** (and anything else on libplacebo) | a full-frame mode in libplacebo's film grain shader, which mpv's `gpu-next` already uses for AV1 grain (`vd-lavc-film-grain=gpu`) | 3-5 days for the shader port (the WGSL is integer-only and maps to GLSL compute), plus a patched libplacebo to maintain | the most useful next step: full-frame grain with hardware decoding in an unmodified mpv build linked to the patched libplacebo. Upstream is unlikely to take a grain mode the AV1 spec doesn't define |
| **ffmpeg filter** (`-vf fullgrain`) | a libavfilter filter that reads the AV1 film grain side data (`-export_side_data film_grain`) and draws the grain, through the C library (GPU) or the decoder's C grain code (CPU) | 1-2 days, plus building ffmpeg | ffmpeg has no out-of-tree filters, so users would need a patched ffmpeg; the `decode` pipe above does the same today |
| **VapourSynth plugin** | `fullgrain.Render(clip, params="clip.json")`: grain from a PARAMS.json (`av1-fullgrain-gpu decode --params-out`, `av1grain strip`), applied per frame number | 1-2 days | VapourSynth's source filters don't pass AV1 grain parameters as frame properties, and some apply standard grain unless told not to; the plugin would rely on the frame numbers matching |
| **GStreamer** | an element after `dav1ddec apply-grain=false` | 2-3 days | GStreamer has no standard film grain metadata to read the parameters from; it would need one |
| **mpv with the library, no patches** | not possible: mpv has no plugin interface for video filters other than libavfilter and VapourSynth, and its VapourSynth filter doesn't pass the grain parameters | | use the pipe, the decoder, or (later) libplacebo |

## Tested

- `av1-fullgrain-gpu decode` into mpv 0.41 and ffmpeg 9.0 on Linux (NVIDIA and AMD GPUs), with software decoding
  (libdav1d) and with NVDEC (`--hwaccel cuda`): every picture identical to the decoder's
  ([exactness](../../gpu/README.md#exactness)).
- The C example with libdav1d 1.5.4 on Linux.
- macOS (Metal) and Windows (DX12): not tested yet.
