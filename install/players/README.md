# VLC, Kodi and other players

A player shows full-frame grain when both of these are true:

1. **It decodes AV1 with dav1d, in software.** Hardware decoding bypasses
   dav1d, so turn it off for AV1 (or entirely) in the player's settings.
2. **It uses the system's dav1d library**, the one you replace with this
   decoder ([Arch](../arch/README.md), [other Linux](../linux/README.md),
   [macOS](../macos/README.md)). Players that bring their own copy of dav1d
   are not affected.

> [!IMPORTANT]
> **Judge AV1 film grain only with full-frame grain.** Standard players tile one small grain patch over the picture;
> the repeating pattern is the player's, not the encode's. Before comparing or judging, pause on a flat area and check
> that the grain looks random everywhere, with no repeating texture. More in [install/gpu](../gpu/README.md), which
> also lists the ways to get full-frame grain with hardware decoding.

None of the players below has been tested with the decoder. Where a step
says a player uses the system's dav1d, that was read from how it is built or
checked with `ldd`, not by watching a film.

## Is a player using the system's dav1d?

On Linux, look for `libdav1d` in the libraries the player (or its plugin)
loads:

```sh
ldd /path/to/player | grep -E 'dav1d|avcodec'
```

A line with `libdav1d.so.7 => /usr/lib/...` (or `/usr/local/lib/...` after a
system-wide install) means it uses the system's copy. A line with only
`libavcodec` means it goes through the system's ffmpeg, which uses the
system's dav1d. No line at all usually means the player has its own copy
built in.

On macOS, `otool -L /path/to/binary | grep dav1d` does the same.

## ffmpeg-based players

Players built on the system's ffmpeg or libmpv use the decoder once it is
installed system-wide: `ffplay`, and on Linux players such as Celluloid,
Haruna and SMPlayer (with mpv) from the distribution's packages. For players
built on mpv, also see the [mpv guide](../mpv/README.md).

## VLC

- **Linux, distribution packages:** VLC loads dav1d through a plugin that
  uses the system's library, so it should pick up the decoder. Check the
  plugin (its path varies by distribution):

  ```sh
  ldd "$(find /usr/lib* -name 'libdav1d_plugin.so' | head -n 1)" | grep dav1d
  ```

  Turn off hardware decoding under Tools > Preferences > Input / Codecs >
  Hardware-accelerated decoding.
- **Windows, macOS, Flatpak and Snap:** VLC has its own dav1d built in. Not
  affected.

## Kodi

- **Linux, distribution packages** that use the system's ffmpeg and dav1d:
  Arch's `kodi` package does (`ldd /usr/lib/kodi/kodi.bin` lists
  `/usr/lib/libdav1d.so.7`). Turn off hardware acceleration under
  Settings > Player > Videos (shown at the Advanced or Expert settings
  level), so that AV1 is decoded in software.
- **Windows, macOS, Android, LibreELEC / CoreELEC and Flatpak:** Kodi has its
  own ffmpeg and dav1d. Not affected.

## Not affected

- **Web browsers** (Chrome, Edge, Firefox, Safari) have their own AV1
  decoders. Use the [browser player](../../browser/README.md) for web pages.
- **TVs, phones, streaming boxes and hardware decoders.** Use
  [av1-grain-seed](../../tools/av1-grain-seed/README.md) to improve the
  standard grain they draw.
