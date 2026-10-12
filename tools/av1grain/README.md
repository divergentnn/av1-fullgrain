# av1grain

Splits an AV1 clip into a **grain-free copy** and a **grain file**
(`PARAMS.json`), for the [browser player](../../browser/README.md): the
browser's own decoder plays the copy, and the player draws the grain from the
file.

```sh
build/tools/av1grain/av1grain strip IN.ivf OUT.ivf PARAMS.json
python3 tools/av1grain/av1grain_mp4.py IN.mp4 OUT.mp4 PARAMS.json --verify   # the same for MP4
```

- `OUT` is the clip with every frame header's grain syntax replaced by
  `apply_grain = 0`. Tile data and everything else are untouched, so it
  decodes to exactly the original's pictures with grain off.
- `PARAMS.json` lists the grain dav1d applied to each output picture (sets of
  parameters, plus a seed per picture).
- The positions come from dav1d's own frame header parser, through a hook in
  the decoder (`fg_hook` in `decoder/dav1d/src/internal.h`). That's why
  `av1grain` is built against the decoder submodule.
- WebM, MKV, MOV: remux to MP4 first, e.g. with `ffmpeg -c copy`.
- `av1grain_mp4.py` needs only Python's standard library and the `av1grain`
  binary (`--av1grain PATH`, `$AV1GRAIN`, or `av1grain` on `$PATH`).

`av1grain decode` decodes with or without grain, optionally applying a
`PARAMS.json` to a grain-free copy; the tests use it.

## Build

From the top of this repository, with the decoder submodule present:

```sh
meson setup build && ninja -C build      # -> build/tools/av1grain/av1grain
```

## Tests

```sh
sh tools/av1grain/strip.sh build/tools/av1grain/av1grain    # or: meson test -C build --suite av1grain
```

On every test stream of the decoder: the copy decoded with grain off equals
the original with grain off; the copy has no grain left; the copy plus
`PARAMS.json` equals the original with grain, in full-frame and standard
grain.
