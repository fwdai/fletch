# Voice dictation

The mic button in the agent composer (beside the paperclip) streams the
platform's native speech recognizer into the prompt box. This document is the
part that isn't obvious from the code: which platforms have it, what the OS
demands before it works, and where your audio goes.

## Platform support

**Apple only.** The implementation is Apple's Speech framework
(`SFSpeechRecognizer` fed by an `AVAudioEngine` mic tap), reached through the
`objc2` bindings — the same code compiles for macOS and iOS, with no Swift
toolchain step. Linux and Windows get a stub that reports `supported: false`,
and the composer hides the button entirely rather than offer one that can only
fail.

The flow, end to end:

| Layer | File |
| --- | --- |
| Button | `src/components/Composer/dictation/DictationButton.tsx` |
| Session state | `src/components/Composer/dictation/useDictation.ts` |
| IPC | `src/api/domains/dictation.ts` → `dictation_availability` / `dictation_start` / `dictation_stop` |
| Events | `src/api/events.ts` — `dictation:transcript`, `dictation:state` |
| Commands + contract | `src-tauri/src/dictation/mod.rs` |
| Recognizer | `src-tauri/src/dictation/apple.rs` |

## Permissions

Dictation needs **two** separate TCC grants, prompted on the first
`dictation_start` (speech first, then the microphone, one dialog at a time):

- `NSSpeechRecognitionUsageDescription`
- `NSMicrophoneUsageDescription`

Both strings live in `src-tauri/Info.plist`, which the Tauri CLI picks up by
filename and `tauri-codegen` also embeds into the dev binary — so the prompts
work under `tauri dev`, not just in a bundle. The speech string is load-bearing
rather than cosmetic: `requestAuthorization:` crashes the process outright if
it is missing.

A denied grant can only be undone in System Settings › Privacy & Security, so
the button shows a slashed mic and says so. To get the first-run prompts back
while testing:

```sh
tccutil reset Microphone com.fletch.desktop
tccutil reset SpeechRecognition com.fletch.desktop
```

## The audio-input entitlement

Release builds are signed with the hardened runtime, which denies microphone
access to the process regardless of the user's TCC answer unless
`com.apple.security.device.audio-input` is granted. It is set in
`src-tauri/Entitlements.plist` and wired through `bundle.macOS.entitlements` in
`tauri.conf.json` — entitlements, unlike `Info.plist`, are never discovered by
filename.

The failure mode when this doesn't reach `codesign` is quiet: dev builds work,
the notarized app lights the mic and transcribes silence. If dictation returns
an empty transcript only in a released build, check the entitlement first.

## On-device vs Apple's servers

The request sets `requiresOnDeviceRecognition` to whatever the recognizer
reports as `supportsOnDeviceRecognition()`. Where that is true (a supported
locale on Apple Silicon, with the assets downloaded) **no audio leaves the
machine**. Where it is false, recognition is server-backed and Apple caps a
session at roughly a minute, after which the recognizer ends it itself — the
composer sees the ordinary final transcript and `stopped`, so a long dictation
simply stops rather than breaking. `dictation_availability` reports which mode
this machine is in as `on_device`.
