# Arch Linux

The `dav1d-fullgrain` package replaces Arch's `dav1d` package
(`provides=dav1d`, `conflicts=dav1d`). Everything that uses the system's dav1d
then shows full-frame grain: ffmpeg, mpv, and players built on them.
The package also installs [av1-grain-seed](../../tools/av1-grain-seed/README.md)
and the [mpv helpers](../mpv/README.md) (in `/usr/share/dav1d-fullgrain/mpv`).

## Install

```sh
git clone https://github.com/divergentnn/av1-fullgrain.git
cd av1-fullgrain/install/arch
makepkg -si
```

The PKGBUILD fetches the decoder itself, at the version this repository
pins, so the clone doesn't need `--recursive`. `makepkg` runs the tests in
`check()` before it installs anything.

Then add the one line from the [mpv guide](../mpv/README.md) if mpv decodes
AV1 in hardware on your machine.

**Another default mode:** `DAV1D_FULLGRAIN_MODE=dual makepkg -si`. Any mode
can also be picked at runtime with `DAV1D_GRAIN_MODE`.

**Update:** `git pull` in the clone, then `makepkg -si` again.

**Back to stock:** `sudo pacman -S dav1d`.

## Check that it's active

With any AV1 file that has film grain:

```sh
ffmpeg -v error -i FILE -frames:v 5 -f md5 -
DAV1D_GRAIN_MODE=standard ffmpeg -v error -i FILE -frames:v 5 -f md5 -
```

Two different checksums mean the decoder is active (the first is full-frame
grain, the second standard grain). The same checksum twice means the
system's stock dav1d is still in use, or the file has no film grain.

## Tested

- The package builds and passes its `check()` (the decoder's grain tests,
  `av1grain` and `av1-grain-seed`) with `makepkg` on CachyOS (Arch-based),
  x86-64.
- Not tested: installing the package on a test system (the library itself
  was tested with mpv 0.41 and ffmpeg 9.0 without installing it, see
  [Linux](../linux/README.md#tested)); aarch64.
