# IINA

**Not supported.** IINA brings its own copy of dav1d inside the app, so
installing this decoder, into Homebrew or anywhere else, doesn't change what
IINA shows. You can see the copy with:

```sh
ls /Applications/IINA.app/Contents/Frameworks | grep -i dav1d
```

What works instead:
- **mpv from Homebrew.** IINA is built on mpv, so the playback is much the
  same. Install the decoder with the [macOS guide](../macos/README.md) and
  add the [mpv settings](../mpv/README.md).
- **Fixing the seeds** with [av1-grain-seed](../../tools/av1-grain-seed/README.md)
  improves the standard grain that IINA draws, without changing IINA.

Replacing the library inside `IINA.app` by hand is not something we've
tried. It would break the app's code signature (it would have to be signed
again), and every IINA update would undo it.

On Macs with M3 or later, IINA may also decode AV1 in hardware, which
bypasses dav1d completely.
