# Mic capture spike

A single page that reports whether microphone capture works inside the Tauri
iOS webview: secure context, `getUserMedia`, `AudioContext` rates, an
`AudioWorklet` tap (frames, RMS, non-zero count) and `MediaRecorder` mime
types. Everything is printed into the page, so a screenshot is the whole log.

Not shipped. It lives here rather than in `public/` so it never lands in a
build.

## Run it on a device or simulator

```sh
cd mobile
mkdir -p public && cp spike/spike.html spike/spike.js spike/spike-worklet.js public/
# Point the dev webview at the page instead of the app:
#   tauri.conf.json → "build.devUrl": "http://localhost:1430/spike.html"
bun run tauri ios dev
```

`NSMicrophoneUsageDescription` comes from `src-tauri/Info.ios.plist`, which
the Tauri CLI merges into the generated project during `ios dev` / `ios build`.
Revert `devUrl` and delete `public/` afterwards.

Expected on a working device: `secure true`, `gUM ok`, a worklet line with a
non-zero `rms` while you talk, and `recorder mime=audio/mp4`. A worklet line
with `rms=0.00000 nonzero=0` means the mic opened but WebKit delivered silence,
which is the failure mode that would send the feature to a native capture
plugin instead.
