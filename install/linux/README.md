# Linux (other distributions)

On Arch and Arch-based distributions, use the [package](../arch/README.md)
instead.

You need `git`, a C compiler, `meson`, `ninja` and, on x86, `nasm`. On Debian
and Ubuntu: `sudo apt install git build-essential meson ninja-build nasm`.
On Fedora: `sudo dnf install git gcc meson ninja-build nasm`.

## Try it without installing

```sh
git clone --recursive https://github.com/divergentnn/av1-fullgrain.git
cd av1-fullgrain
meson setup build && ninja -C build
install/mpv/mpv-fullgrain FILE.mkv        # or: install/mpv/mpv-fullgrain standard|dual|multi FILE.mkv
```

`mpv-fullgrain` starts your system's mpv with this decoder and the
[live A/B keys](../mpv/README.md#live-ab). The same works for ffmpeg:

```sh
LD_LIBRARY_PATH=$PWD/build/decoder/dav1d/src ffmpeg -i FILE.mkv ...
```

## Install system-wide

This puts the decoder library in `/usr/local`, where ffmpeg, mpv and the
other players that use the system's dav1d pick it up:

```sh
git clone --recursive https://github.com/divergentnn/av1-fullgrain.git
cd av1-fullgrain
meson setup build --prefix=/usr/local
ninja -C build
sudo ninja -C build install
sudo ldconfig
ldd "$(command -v ffmpeg)" | grep dav1d    # should now point into /usr/local
```

This also installs `dav1d` (the command line decoder) and `av1-grain-seed`
in `/usr/local/bin`.

- **Debian and Ubuntu** search `/usr/local/lib` before the system's library,
  so this is all.
- **Fedora** and others: if `ldd` still shows the system's `libdav1d`, add
  the directory the install printed for `libdav1d.so.7` to a file in
  `/etc/ld.so.conf.d/` and run `sudo ldconfig` again.
- **Undo:** `sudo ninja -C build uninstall && sudo ldconfig`.

Then see the [mpv guide](../mpv/README.md) for the one line that keeps AV1
decoding in software.

Players installed as **Flatpak** or **Snap** bring their own copy of dav1d
and are not affected. For other players, see [players](../players/README.md).

## Check that it's active

With any AV1 file that has film grain:

```sh
ffmpeg -v error -i FILE -frames:v 5 -f md5 -
DAV1D_GRAIN_MODE=standard ffmpeg -v error -i FILE -frames:v 5 -f md5 -
```

Two different checksums mean the decoder is active. The same checksum twice
means the system's stock dav1d is still in use, or the file has no film
grain.

## Tested

- Building, and both ways of trying it without installing: mpv 0.41 through
  `mpv-fullgrain` and ffmpeg 9.0 through `LD_LIBRARY_PATH` show full-frame
  grain, and `standard` matches the system's stock dav1d (CachyOS, x86-64).
  CI builds on Ubuntu.
- Not tested: the system-wide install on Debian, Ubuntu or Fedora.
