# mpv

First install the decoder for your system: [Arch](../arch/README.md),
[other Linux](../linux/README.md), [macOS](../macos/README.md) or
[Windows](../windows/README.md) (untested). mpv then uses it whenever dav1d
decodes the AV1. Two settings decide whether it does.

> [!IMPORTANT]
> **Judge AV1 film grain only with full-frame grain.** Standard players tile one small grain patch over the picture;
> the repeating pattern is the player's, not the encode's. Before comparing or judging, pause on a flat area and check
> that the grain looks random everywhere, with no repeating texture. More in [install/gpu](../gpu/README.md), which
> also lists the ways to get full-frame grain with hardware decoding.

## Hardware decoding: one line

With hardware decoding, dav1d isn't used at all. That applies to Macs with
M3 or later and to most recent PC graphics cards, if mpv's `hwdec` is on.
This line in `~/.config/mpv/mpv.conf` keeps AV1 in software and every other
codec in hardware:

```ini
# AV1 in software (dav1d); every other codec keeps hardware decoding
hwdec-codecs=h264,vc1,hevc,vp8,vp9,prores,prores_raw,ffv1,dpx
```

That list is mpv 0.41's default without `av1`. The same file is
[mpv.conf](mpv.conf) here.

**Hardware decoding with full-frame grain:** the [GPU renderer](../../gpu/README.md) draws the grain after any
decoder; `av1-fullgrain-gpu decode --hwaccel cuda FILE | mpv --audio-file=FILE -` plays a clip that way (no seeking).

## Grain on the GPU: nothing to do

mpv's `gpu-next` renderer normally asks the decoder to leave grain out and
draws standard AV1 grain itself (`vd-lavc-film-grain=auto`). The decoder
applies full-frame grain anyway and hides the grain parameters from mpv, so
nothing is drawn twice. That's the decoder's `grain_force` option, on by
default; `DAV1D_GRAIN_FORCE=0` restores upstream dav1d's behaviour, and then
you'd need `vd-lavc-film-grain=cpu`.

## Live A/B

[grain-mode.lua](grain-mode.lua) switches the grain mode while a video plays:
- `g` cycles standard / dual / full / multi;
- `b` is a blind toggle between standard and full, shown only as A or B;
- `B` reveals which was which.

[mpv-fullgrain](mpv-fullgrain) sets everything up and uses the decoder from
a build in this checkout, without installing it (Linux and macOS):

```sh
meson setup build && ninja -C build                # from the top of the repository
install/mpv/mpv-fullgrain FILE.mkv                 # or: mpv-fullgrain standard|dual|multi FILE.mkv
```

With the decoder installed system-wide, set `DAV1D_GRAIN_MODE_FILE` to any
writable path before starting mpv, and load the script with
`--script=install/mpv/grain-mode.lua` or by copying it to
`~/.config/mpv/scripts/`. The Arch package installs it in
`/usr/share/dav1d-fullgrain/mpv/`.

## Tested

- Linux: mpv 0.41 with `mpv-fullgrain` (CachyOS, x86-64).
- macOS: Homebrew's mpv with the decoder installed by the
  [macOS script](../macos/README.md), on an Apple Silicon MacBook.
- Windows: not tested.
