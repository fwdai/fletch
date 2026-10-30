# Debugging a native crash

Release builds ship stripped, so a crash report shows our frames as
`Fletch + 0x7f161c`. Each release has its symbols attached as
`Fletch-<version>-dSYMs.zip` (one `fletch.dSYM` per architecture), which turn
those addresses back into functions and lines.

## 1. Get the crash report

- On the Mac that crashed: `~/Library/Logs/DiagnosticReports/Fletch-*.ips`
  (older ones under `Retired/`).
- Or the minidump on the Sentry issue.

Note the app version, the architecture (`arm64` / `x86_64`), and the
`Fletch` image's base address and UUID. In an `.ips` they are in
`usedImages`, and each frame of the crashed thread (`faultingThread`) has an
`imageOffset`.

## 2. Get the matching symbols

```sh
gh release download v0.8.17 --pattern 'Fletch-*-dSYMs.zip'
unzip Fletch-0.8.17-dSYMs.zip
DWARF=$(ls Fletch-0.8.17-dSYMs/aarch64/fletch.dSYM/Contents/Resources/DWARF/*)
dwarfdump --uuid "$DWARF"   # must equal the Fletch image UUID in the report
```

Use `x86_64/` for an Intel crash. If the UUIDs differ, the symbols are for
another build and the names they give are wrong.

## 3. Symbolicate

`atos` takes the base address and absolute addresses (base + offset):

```sh
BASE=0x102c94000   # the Fletch image's base from the report
atos -o "$DWARF" -arch arm64 -l $BASE \
  $(printf '0x%x ' $((BASE + 0x807360)) $((BASE + 0x7f161c)))
```

## 4. Look around the crash

- `~/Library/Logs/com.fletch.desktop/fletch.<date>.log`: our log, including
  any Rust panic message (`ERROR fletch_lib: panic`).
- The system log, for what AppKit or WebKit said just before:

  ```sh
  /usr/bin/log show --style compact --predicate 'process == "Fletch"' \
    --start '2026-10-30 17:00:40' --end '2026-10-30 17:00:50'
  ```

  It keeps routine messages for only about a day, so read it soon after the
  crash. (`/usr/bin/log`, since zsh has a `log` builtin.)

`panic in a function that cannot unwind` with no earlier panic in our log
means an Objective-C exception unwound into a Rust `extern "C"` frame; the
system log usually names the exception (`*** Assertion failure in …`). See
`src-tauri/src/appkit_exceptions.rs`.
