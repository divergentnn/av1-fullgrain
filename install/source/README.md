# Building from source

```sh
git clone --recursive https://github.com/divergentnn/av1-fullgrain.git
cd av1-fullgrain
meson setup build && ninja -C build
```

You need `git`, a C compiler (GCC or Clang), `meson`, `ninja` and, on x86,
`nasm`. Python 3 runs the tests.

| built | what it is |
|---|---|
| `build/decoder/dav1d/src/libdav1d.so.7` (`.7.dylib` on macOS) | the decoder library |
| `build/decoder/dav1d/tools/dav1d` | dav1d's command line decoder |
| `build/tools/av1-grain-seed/av1-grain-seed` | [the seed fixer](../../tools/av1-grain-seed/README.md) |
| `build/tools/av1grain/av1grain` | [grain-free copies](../../tools/av1grain/README.md) for the browser player |

`decoder/dav1d` is a git submodule ([dav1d-fullgrain](https://github.com/divergentnn/dav1d-fullgrain)).
In a clone made without `--recursive`, `meson setup` fetches it, or run
`git submodule update --init`. Without it (in a ZIP download, say), only
`av1-grain-seed` is built.

## Options

The decoder's options take its subproject name:

```sh
meson setup build -Ddav1d:grain_mode_default=dual -Ddav1d:enable_asm=false
```

`meson configure build` lists them all. The decoder can also be built on its
own, as plain dav1d: `meson setup build-decoder decoder/dav1d`, with options
without the prefix.

`meson install -C build` installs the decoder library, its headers, `dav1d`
and `av1-grain-seed`. To replace the system's dav1d, follow the guide for your
system in [Install](../../README.md#install).

## Tests

```sh
sh decoder/dav1d/tests/fullgrain/run.sh build/decoder/dav1d/tools/dav1d   # grain modes vs committed hashes, SIMD vs C
meson test -C build --suite av1fg --suite av1fg-dav1d --suite av1grain    # av1-grain-seed, av1grain
```

With an upstream dav1d 1.5.4 build as a second argument, `run.sh` also
checks that `standard` grain is identical to upstream's.

## Browser kit

```sh
browser/build.sh      # needs emsdk (installed into browser/emsdk unless EMSDK is set), meson, ninja
```

See the [browser player](../../browser/README.md).

## Windows

- `av1-grain-seed` builds with Visual Studio (MSVC): see
  [its guide](../../tools/av1-grain-seed/README.md#install).
- The decoder builds with MSYS2's GCC (MSVC not tried): see
  [Windows](../windows/README.md) (untested outside CI).

## Tested

CI ([workflow](../../.github/workflows/ci.yml)) builds and tests everything
on Linux x86-64 and macOS arm64, `av1-grain-seed` on Windows with MSVC, the
decoder on Windows with MSYS2's GCC, and the browser kit's WebAssembly build
against the decoder's hashes.
