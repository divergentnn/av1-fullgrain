# macOS (Homebrew)

For mpv and ffmpeg installed with [Homebrew](https://brew.sh). The installer
swaps the dav1d library inside Homebrew's `dav1d` package, so Homebrew's
ffmpeg and mpv (`brew install mpv`) use it.

Apps that bring their own copy of dav1d are not affected: IINA (see
[IINA](../iina/README.md)), mpv.app bundles, VLC and web browsers.

## Install

```sh
git clone --recursive https://github.com/divergentnn/av1-fullgrain.git
cd av1-fullgrain
install/macos/install.sh
```

The script:
- installs `meson` and `ninja` with Homebrew (`nasm` on Intel Macs);
- builds the decoder and runs its tests;
- swaps the `libdav1d.7.dylib` inside Homebrew's `dav1d` (keeping the
  original as `libdav1d.7.dylib.stock`) and re-signs it.

Then add the [mpv settings](../mpv/README.md): on Macs with M3 or later, mpv
decodes AV1 in hardware, which bypasses dav1d.

**Another default mode:** `GRAIN_MODE=dual install/macos/install.sh`. Any mode
can also be picked at runtime with `DAV1D_GRAIN_MODE`.

**Undo:** `install/macos/uninstall.sh`, or `brew reinstall dav1d`.

**After `brew upgrade`:** an upgrade of `dav1d` puts the stock library back.
Run `install.sh` again afterwards, or hold the version with `brew pin dav1d`.
The script refuses to replace a Homebrew dav1d other than 1.5.x (the
decoder is dav1d 1.5.4), unless you run it with `FORCE=1`.

## Try it without touching Homebrew

```sh
meson setup build && ninja -C build
install/mpv/mpv-fullgrain FILE.mkv
```

`mpv-fullgrain` starts Homebrew's mpv with this decoder (through
`DYLD_LIBRARY_PATH`) and the [live A/B keys](../mpv/README.md#live-ab).

## Check that it's active

With any AV1 file that has film grain:

```sh
ffmpeg -v error -i FILE -frames:v 5 -f md5 -
DAV1D_GRAIN_MODE=standard ffmpeg -v error -i FILE -frames:v 5 -f md5 -
```

Two different checksums mean the decoder is active. The same checksum twice
means Homebrew's stock dav1d is in use, or the file has no film grain.

## Tested

- In daily use on an Apple Silicon MacBook (M1-class) with Homebrew's mpv,
  installed with this script.
- CI runs the script on GitHub's macOS arm64 runner (Homebrew's current
  dav1d), checks that Homebrew's own `dav1d` then draws full-frame grain,
  and runs `uninstall.sh`.
- Not tested: Intel Macs.
