# Windows

> **Untested.** Nobody has run these steps on a Windows PC yet. They follow
> from how the decoder and these players are built, and CI only checks the
> parts listed under [Tested](#tested). Expect rough edges.

## Which players can use it

The decoder can only reach players that load dav1d as a separate file
(a DLL with `dav1d` in its name), which you then replace:

- **mpv and ffmpeg from [MSYS2](https://www.msys2.org)** (the UCRT64
  environment) load `libdav1d-7.dll`. This is the path described below.
- **Most other Windows builds** of mpv (including the ones that mpv.net and
  most download pages use), VLC and Kodi have dav1d built into the program.
  They can't be changed. To check a player, look for a file with `dav1d` in
  its name next to its `.exe`: if there is none, dav1d is built in.

For players you can't change, [av1-grain-seed](../../tools/av1-grain-seed/README.md)
(which is tested on Windows) improves the standard grain in the files
themselves.

## mpv and ffmpeg from MSYS2

1. Install MSYS2 and open the **MSYS2 UCRT64** terminal.
2. Install the tools and mpv:

   ```sh
   pacman -S --needed git mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-meson \
       mingw-w64-ucrt-x86_64-ninja mingw-w64-ucrt-x86_64-nasm mingw-w64-ucrt-x86_64-mpv
   ```

3. Check MSYS2's dav1d version. The decoder is dav1d 1.5.4; with a newer
   dav1d, MSYS2's ffmpeg may need functions that 1.5.4 doesn't have.

   ```sh
   pacman -Q mingw-w64-ucrt-x86_64-dav1d       # should be 1.5.x
   ls /ucrt64/bin | grep dav1d                 # libdav1d-7.dll
   ```

4. Build the decoder:

   ```sh
   git clone --recursive https://github.com/divergentnn/av1-fullgrain.git
   cd av1-fullgrain
   meson setup build decoder/dav1d --buildtype=release
   ninja -C build
   ```

5. Replace MSYS2's library, keeping the original:

   ```sh
   cp /ucrt64/bin/libdav1d-7.dll /ucrt64/bin/libdav1d-7.dll.stock
   cp build/src/libdav1d-7.dll /ucrt64/bin/
   ```

6. Keep AV1 decoding in software: add the line from the
   [mpv guide](../mpv/README.md) to `%APPDATA%\mpv\mpv.conf`.

**Check** with any AV1 file that has film grain:

```sh
ffmpeg -v error -i FILE -frames:v 5 -f md5 -
DAV1D_GRAIN_MODE=standard ffmpeg -v error -i FILE -frames:v 5 -f md5 -
```

Two different checksums mean the decoder is active.

**Undo:** `cp /ucrt64/bin/libdav1d-7.dll.stock /ucrt64/bin/libdav1d-7.dll`, or
reinstall MSYS2's package with `pacman -S mingw-w64-ucrt-x86_64-dav1d`. An
MSYS2 update of dav1d also puts the stock library back.

**Visual Studio (MSVC)** has not been tried for the decoder. The full-frame
grain's AVX2 code uses GCC/Clang target attributes, so it probably needs changes
first. Use MSYS2's GCC as above.

## Tested

- Nothing in this guide has been tried on a Windows PC.
- CI builds the decoder with MSYS2's GCC (UCRT64, x86-64) on Windows and runs
  its grain tests, and runs `av1-grain-seed` with MSVC.
