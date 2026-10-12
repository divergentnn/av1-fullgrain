# The decoder

`decoder/dav1d` is [dav1d](https://code.videolan.org/videolan/dav1d) 1.5.4,
the AV1 decoder inside ffmpeg, mpv and most players, with full-frame film
grain. It is a git submodule: the fork lives in its own repository,
[dav1d-fullgrain](https://github.com/divergentnn/dav1d-fullgrain), which keeps
upstream's layout so it can follow upstream releases.

If `decoder/dav1d` is empty, fetch it:

```sh
git submodule update --init
```

(or clone with `git clone --recursive`).

## Build

With the tools, from the top of this repository:

```sh
meson setup build && ninja -C build
build/decoder/dav1d/tools/dav1d --version
```

The decoder on its own:

```sh
meson setup build-decoder decoder/dav1d --buildtype=release
ninja -C build-decoder
```

The library has the same name, version and API as dav1d 1.5.4
(`libdav1d.so.7`, `libdav1d.7.dylib`), so it can replace the system's copy.
See [Install](../README.md#install).

## What it changes

All of it is in `src/`; the rest of dav1d is untouched:
- the grain modes `full` (the default), `dual` and `multi`, in C with AVX2
  (x86-64) and NEON (arm64, also WebAssembly SIMD128) kernels;
- `grain_force`: full-frame grain even when the player turned dav1d's grain
  off to draw standard grain itself (mpv's `gpu-next`);
- a frame header parser hook for [av1grain](../tools/av1grain/README.md),
  not reachable through the public API.

How it works: [README](../README.md#how-it-works). Options:
[README](../README.md#options).

## Tests

```sh
sh decoder/dav1d/tests/fullgrain/run.sh build/decoder/dav1d/tools/dav1d [UPSTREAM_DAV1D]
```

checks every grain mode against committed hashes (the same on x86-64 and
arm64), the SIMD paths against the C paths and, given an upstream dav1d
1.5.4 build, that `standard` is md5-identical to it.

## Updating the submodule

```sh
git submodule update --remote decoder/dav1d     # dav1d-fullgrain's grain-variants branch
git add decoder/dav1d && git commit
```
