# Voice dictation

The agent composer's primary control — the pill at the far right of its footer,
which is the microphone when the box is empty, send once there is a draft, and
stop while the agent runs — streams a speech recognizer into the prompt box.
This document is the part that isn't obvious from the code: which platforms
have it, what the OS demands before it works, and where your audio goes.

## The control

`src/components/Composer/PrimaryControl/` renders one pill whose state is
derived, never stored (`primaryState`): `error` outranks `listening`,
`transcribing`, `running`, `draft`, `unavailable`, `empty`, in that order. The
mic is a separate button on the same slot that slides left as a neutral
secondary once there is a draft, and collapses while listening or running.
Widths per state, tones, and timings follow the design spec; the CSS is the
source of those numbers.

Keys, handled on the textarea (`Composer/index.tsx`). ⌘⇧D also works with
the focus anywhere else in the window that isn't an editable field
(`useDictationHotkey`): it toggles dictation in the composer and brings the
caret there.

| Key | empty | draft | listening | agent running |
| --- | --- | --- | --- | --- |
| ↵ | — | send | stop & transcribe | — (the draft waits) |
| ⌘↵ | — | send | stop & transcribe | send mid-turn |
| ⇧↵ | newline | newline | newline | newline |
| ⌘⇧D | start dictation | start dictation | stop & transcribe | — |
| esc | — | — | cancel, discard interim | stop agent |

Nothing is ever sent by voice alone: speech lands as editable text and waits
for ↵.

While the mic is open the recognizer's running transcript is **not** written
into the textarea. It is shown by `InterimGhost`, a layer over the textarea
with identical metrics (the textarea's own text repeated invisibly, then the
interim words in italic with a pulsing dot), so undo history stays clean and
esc is a no-op on the draft. The final result is inserted at the caret of
whatever the box holds when it arrives (`insertTranscript`, space-normalised on
both sides; the ghost previews exactly that join), the caret moves to its end,
and the new span carries a short wash. A session that ends without a final result
(Apple's flush deadline, an error mid-utterance) commits what was last heard.

A failed transcription turns the pill danger-tinted with a retry glyph for
2.6 s (click retries; typing dismisses it sooner). A denied microphone grant
shows an outlined mic-off pill whose click opens System Settings › Privacy &
Security › Microphone — the shell plugin's open scope in `tauri.conf.json`
admits that URL scheme for this. Where no engine exists at all the mic is never
offered and the empty slot degrades to a plain, disabled send arrow — which is
also the form the workflow and roadmap composers use, having no dictation.

## Platform support

**macOS 26 or newer.** The default engine is Apple's on-device `SpeechAnalyzer`
(the model behind the system's own dictation since macOS 26), fed by an
`AVAudioEngine` mic tap. The analyzer is a Swift-only API, so it is reached
through a small Swift bridge compiled into the binary — see
[Building the Swift bridge](#building-the-swift-bridge). Linux and Windows get
a stub that reports `supported: false`, and the composer hides the button
entirely rather than offer one that can only fail. A Mac below 26 gets the same
answer from the real implementation, as does a Mac whose language Apple has no
model for. (`supported: false` is what degrades the control to a send arrow —
see [The control](#the-control).)

macOS additionally has a local whisper.cpp engine the user can opt into; see
[Local Whisper engine](#local-whisper-engine-opt-in) below. It has no macOS 26
requirement, so on an older Mac it is the only way to get the button back.

The flow, end to end:

| Layer | File |
| --- | --- |
| Control | `src/components/Composer/PrimaryControl/` (state derivation, pill, level bars, running trace) |
| Interim text | `src/components/Composer/InterimGhost.tsx` |
| Session state | `src/components/Composer/dictation/useDictation.ts` |
| IPC | `src/api/domains/dictation.ts` → `dictation_availability` / `dictation_start` / `dictation_stop` |
| Events | `src/api/events.ts` — `dictation:transcript`, `dictation:state`, `dictation:level` |
| Commands + contract | `src-tauri/src/dictation/mod.rs` |
| Microphone (both engines) | `src-tauri/src/dictation/apple.rs` |
| Level meter (both engines) | `src-tauri/src/dictation/level.rs` |
| Default engine | `src-tauri/src/dictation/speech.rs` (Rust side), `src-tauri/swift/SpeechBridge.swift` (Swift side) |
| Local engine | `src-tauri/src/dictation/capture.rs`, `src-tauri/src/dictation/whisper/` |

## Permissions

Dictation needs **one** TCC grant, the microphone, prompted on the first
`dictation_start`. The usage string, `NSMicrophoneUsageDescription`, lives in
`src-tauri/Info.plist`, which the Tauri CLI picks up by filename and
`tauri-codegen` also embeds into the dev binary — so the prompt works under
`tauri dev`, not just in a bundle.

Apple's analyzer does **not** need the Speech Recognition grant. This was
checked rather than assumed: a throwaway app bundle with a fresh bundle id and
no `NSSpeechRecognitionUsageDescription` transcribed a file with
`SFSpeechRecognizer.authorizationStatus()` reading "not determined" before and
after, and no prompt. So nothing calls `requestAuthorization:` any more, the
plist key is gone, and `dictation_availability`'s `speech` field is vestigial —
always `not_determined`, kept only so the wire shape didn't change. The button
ignores it.

A denied microphone grant can only be undone in System Settings › Privacy &
Security, so the control shows an outlined mic-off pill whose tooltip says so
and whose click opens that pane. To get the first-run prompt back while
testing:

```sh
tccutil reset Microphone com.fletch.desktop
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

## The default engine

`SpeechAnalyzer` runs entirely on this machine: **no audio leaves it**, there
is no server path and so no server-side session cap. `dictation_availability`
reports `on_device: true` for both engines, and the field exists only because
the wire shape predates this.

### Locale and the model

`SpeechTranscriber` takes its locale explicitly. The bridge uses
`Locale.current` and matches it against `SpeechTranscriber.supportedLocales` —
exactly first, then any variant of the same language, so a Mac set to
English/Poland still dictates in English. No match means
`dictation_availability.supported` is false and a start would say "Dictation
doesn't support this Mac's language." There is no setting for it.

The per-locale model is downloaded by the OS on demand, not bundled. The first
`dictation_start` (not launch) asks `AssetInventory` whether anything is
missing; if so it kicks off the download in the background and **rejects** the
start with "Downloading the speech model for English (12%). Try again in a
moment." — a readable failure rather than a mic button that hangs for a
multi-hundred-megabyte fetch. Once the model has landed the next start goes
through. `AssetInventory.reserve(locale:)` is called each time so the OS keeps
the model resident for us.

All of that is the bridge's `prepare` step, which `apple::start` awaits after
the permission check and before opening the mic. It also caches the audio
format the analyzer wants (`bestAvailableAudioFormat`, 16 kHz mono `Int16` in
practice), so the actual session start is synchronous.

### Volatile and final results

The transcriber is configured for `.volatileResults`: while the user speaks it
emits guesses that each replace the previous guess, and periodically a
`isFinal` result that commits a stretch of text. The bridge keeps the
concatenation of finalized results and sends `finalized + latest volatile` on
every result, so each `dictation:transcript` event is the whole running text —
the contract `useDictation` already relied on with the legacy recognizer.

A stop finishes the input stream and calls
`finalizeAndFinishThroughEndOfInput()`; the results sequence then ends, and
that end is reported as the one `is_final` transcript, which is what drives
`stopped` (the same shape `endAudio` had). A teardown calls
`cancelAndFinishNow()` instead. Should the analyzer never finish, the
two-second flush deadline in `apple.rs` forces the stop.

### Threading

The bridge's callbacks arrive on Swift's cooperative executors, never on the
main thread, so each one hops onto it with `run_on_main_thread` before touching
session state, stamped with the generation it belongs to. The tap's job is
unchanged: hand the buffer over and return. The conversion from the mic's
format to the analyzer's (an `AVAudioConverter`, which also serves as the copy
the tap's reused buffer needs) happens inside the bridge's `feed`, on the render
thread — the same thing Apple's own `SpeechAnalyzer` sample does.

## The level meter

While the mic is open the composer shows level bars, fed by `dictation:level`:
a value from 0 (silence) to 1 (loud, close speech), stamped with the session
id like every other dictation event, about every 90 ms (`level::LEVEL_POLL`).
It is display only — nothing about the session depends on it — and it stops
when the mic closes, so on Apple's engine the last few events precede the
flushed final transcript and `stopped`.

Both engines feed the same `level::Meter`, which is one atomic holding the most
recent tap buffer's RMS. The local engine already computes that RMS in the tap
for the [silence gate](#hands-free-auto-stop) and hands it over; Apple's path,
which otherwise passes the buffer to the bridge untouched, measures it in the
same callback. The tap does nothing else there — one pass over the samples and
one relaxed store — and a tokio task off the render thread samples the atomic
and emits. Should a microphone ever deliver something other than deinterleaved
float32, the meter reads nothing and reports silence rather than read the wrong
memory (the local engine refuses such a format outright; Apple's engine
converts it in the bridge and works regardless).

The mapping to 0–1 is linear in dBFS between −50 dB and −15 dB
(`level::normalize`): a quiet room on a laptop mic sits under the floor,
conversational speech lands mid-range, and only close, loud speech pins the
bars. That range is a display choice and is the one thing to tune if the bars
read too shy or too hot on common hardware.

## Building the Swift bridge

`SpeechAnalyzer`, `SpeechTranscriber`, `AnalyzerInput` and `AssetInventory`
have no Objective-C surface, so `objc2` can't reach them. `src-tauri/build.rs`
(`build_speech_bridge`) compiles `src-tauri/swift/SpeechBridge.swift` into a
static library and links it into the Rust binary; the Swift side exposes six
`@_cdecl` C functions (`availability`, `prepare`, `start`, `feed`, `finish`,
`cancel`) and takes C function pointers for its callbacks. All session state
stays in Rust; the Swift object holds only what has to be Swift.

The invocation, per target architecture:

```sh
xcrun --sdk macosx --find swiftc   # the compiler of the selected Xcode / CLT
swiftc -emit-library -static -parse-as-library -module-name FletchSpeech \
  -swift-version 5 -O \
  -target arm64-apple-macos13.0 \       # or x86_64-apple-macos13.0
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  swift/SpeechBridge.swift -o "$OUT_DIR/libfletch_speech.a"
```

The deployment target is `13.0`, matching `minimumSystemVersion`; the Swift
code guards every entry point with `#available(macOS 26, *)`, so the binary
still launches on older macOS and dictation reports unsupported there. Nothing
is bundled: the Swift objects carry autolink entries for `swiftCore`,
`Foundation`, `Speech` and friends, and the only thing the Rust link adds is a
search path to the SDK's `usr/lib/swift` stubs, which resolve to the runtime
that has shipped with macOS since 10.14.4. Both release slices (`arm64`,
`x86_64`) build from the same file.

The build needs the macOS 26 SDK — Xcode 26 or the matching Command Line Tools
— and stops early with a message saying so if `xcrun` is missing or the
selected SDK is older, rather than failing at the linker. The Linux CI build
compiles none of this (`CARGO_CFG_TARGET_OS` gate). A change to the Swift file
reruns the build script; so does a change of `SDKROOT` or
`MACOSX_DEPLOYMENT_TARGET`.

## Local Whisper engine (opt-in)

Settings › General › Dictation offers a second engine: whisper.cpp running
locally, for people who want transcription that doesn't depend on Apple's
locale support or on macOS 26. Apple's engine stays the default — the local
engine costs a one-time model download, so it can only be a choice the user
makes.

The opt-in is the `dictation_engine` settings row (`whisper` or `apple`),
written by the backend `set_dictation_engine` command. Enabling it starts the
download of the chosen model (see [Choosing a model](#choosing-a-model)) in a
background task and returns immediately; progress arrives as
`dictation:model_progress` events, and `dictation_model_status` is what a
freshly opened Settings screen reads instead. Weights only reach their final
path after their SHA-256 matched, so "the file is there, at the pinned size"
is the same statement as "it was verified" — that is what
`models::installed_path` checks.

### Where the weights live

`<app data>/whisper-models/`, seeded by `whisper::init` from `setup` — so
`~/Library/Application Support/com.fletch.desktop/whisper-models` in a bundle,
and `…/com.fletch.desktop/dev/whisper-models` in a debug build (`tauri dev`
gets its own data dir, weights included, so a dev run downloads its own copy).
One file per model the user has downloaded, so more than one can sit there at
once. Nothing else is written there, so deleting the directory is a clean
uninstall (as is Remove on every row in Settings). A download in flight is a
`download-<pid>.tmp` alongside; a run killed mid-download leaves one behind and
the next attempt sweeps it.

| Layer | File |
| --- | --- |
| Settings section | `src/components/SettingsScreen/DictationSection/` |
| IPC | `dictation_model_status` / `set_dictation_engine` / `set_dictation_model` / `dictation_model_download` / `dictation_model_remove` |
| Event | `src/api/events.ts` — `dictation:model_progress` |
| Catalog | `src-tauri/src/dictation/whisper/models.rs` |
| Download | `src-tauri/src/dictation/whisper/install.rs`, on the shared `download::download_verified` |

### Choosing a model

Settings lists every catalog entry with its size and a radio, and the choice
is the `dictation_model` setting (a `WhisperModel::id`), written by
`set_dictation_model`. Picking one while the engine is on starts its download
if it isn't already there; picking one while the engine is off just records the
choice.

Absent or unrecognized, the setting resolves to the **platform default** —
`small.en-q8_0` when `std::env::consts::ARCH` is `x86_64`, `DEFAULT_MODEL_ID`
otherwise. Intel (and Rosetta, which reports the same arch) has no
Metal-class GPU to hide the large model's decode behind, where a dictated
sentence takes longer than saying it again. An id that isn't in `MODELS` is
treated as absent rather than as an error, so dropping an entry in an update
can't leave dictation unable to name a model at all.

The model you switch away from **stays on disk**: it is already paid for, and
switching back shouldn't cost the download twice. Its row keeps saying
"Installed" with a Remove button, which is the only way to reclaim the space.

One download runs at a time process-wide, and changing the selection does not
cancel one in flight — `dictation_model_status` reports `downloading` plus
`downloading_id` so the bar stays on the row actually being fetched.

Everything reads the choice through one helper (`whisper::selected`, off a
`&Connection`), so the row's status, the download target and the weights a
session loads can't disagree. The engine itself has no database handle: `stop`
reads the choice when the mic closes and passes the model down through
`capture::transcribe` to `engine::transcribe`, so a session transcribes with
whatever Settings showed at the moment it ended. The engine's cached
`WhisperContext` is keyed on the path it loaded from, so a switch drops the old
weights before loading the new ones.

### Adding a model to the catalog

`src-tauri/src/dictation/whisper/models.rs` pins every candidate: file name,
URL, SHA-256, and exact byte size. Nothing is discovered at runtime — the
digest is the supply-chain boundary, and the size is both the progress total
and the cheap installed check. Settings offers whatever `MODELS` holds, so:

1. Open `https://huggingface.co/ggerganov/whisper.cpp/raw/main/<file>` — the
   LFS pointer, not the file itself. Copy `oid sha256` and `size` verbatim.
2. Add a `WhisperModel` entry to `MODELS` with those values, a `label`, and a
   one-line `note` (both are display copy in Settings).
3. To make it what a fresh opt-in gets, point `DEFAULT_MODEL_ID` (or
   `SMALL_MODEL_ID`, the Intel default) at the new `id`. Users who already
   chose a model keep it.

`size` is also where the "Downloads a 574 MB model once." copy comes from, so
the UI can't drift from the pinned file.

### How a session runs on it

Opting in to whisper.cpp in Settings replaces the recognizer, not the mic:
`apple.rs` still opens the `AVAudioEngine` and installs the tap, and the
commands, the events and the session-id contract are identical. What changes is
the tap's *sink*, and what a stop means.

### Dispatch

`dictation_start` and `dictation_availability` both go through
`dictation::engine`, which answers `whisper` only when **both** hold:

- the `dictation_engine` setting is exactly `"whisper"`, and
- the *selected* model's file is fully present
  (`whisper::models::installed_path` checks the exact byte size, and the
  download only moves a digest-verified file into place).

Dispatching on the setting alone would let an interrupted download leave the
mic button dead, so a half-finished install silently falls back to Apple's
recognizer. `dictation_availability` reports the answer as `engine`, so the UI
never has to guess which one a click will get.

whisper.cpp is compiled from source by `whisper-rs-sys`, which needs **cmake**
on the build host. The dependency lives under
`[target.'cfg(target_os = "macos")'.dependencies]` for two reasons: `metal`
puts inference on the GPU, and the Linux CI build has no business compiling a
C++ tree it will never run. No link flags of our own are needed;
`whisper-rs-sys` emits the `c++`/Accelerate/Foundation/Metal/MetalKit lines
itself.

ggml's CMake detects ARM CPU features by compiling probe programs with
`-mcpu=native+<feature>` and running them; the probes for features the CPU
lacks (SVE and SME on Apple Silicon) are meant to die with SIGILL. Under a
sandbox that stalls crash handling they can instead hang forever, and the whole
cargo build with them — silently, at the cmake configure step. The repo-root
`.cargo/config.toml` therefore pre-answers those two probes through the
`GGML_MACHINE_SUPPORTS_sve` / `GGML_MACHINE_SUPPORTS_sme` env vars, which
`whisper-rs-sys` forwards to cmake as defines. If a build ever sits in
`whisper-rs-sys` with no output, look for a `cmTC_*` process and check
`CMakeConfigureLog.yaml` under its `out/build` for which probe is running.

### Capture, and why resampling isn't decimation

whisper.cpp takes 16 kHz mono `f32`; the microphone is typically 48 kHz and may
be stereo. The two conversions are split by thread on purpose:

- **Channel averaging happens in the tap**, on Apple's real-time render thread,
  because it is a few adds per frame into a pre-sized buffer behind a
  `try_lock`. Nothing else happens there — no allocation, no session state, no
  events.
- **Resampling happens at stop**, in a `spawn_blocking` task, using
  `AVAudioConverter`. Taking every third sample would be cheap enough for the
  tap, but decimating without a low-pass folds everything above 8 kHz back into
  the speech band and the model hears the aliases.

A session is capped at five minutes of audio (`MAX_CAPTURE_SECS`); past that,
new buffers are dropped, which truncates the transcript rather than failing.

### The silence gate

Whisper does not return nothing for nothing. Given silence or a fraction of a
second of noise it invents a plausible sentence — "Thank you.", "[BLANK_AUDIO]",
a line of subtitle boilerplate — because that is what its training data has in
those places. So `whisper::engine::transcribe` refuses to run the model at all
when the clip is under `MIN_DURATION` (0.5 s, i.e. a tap on the button) or its
RMS is under `MIN_RMS` (0.002, i.e. a muted or unplugged input), and returns
empty text. `whisper.cpp`'s own `suppress_blank` handles the milder case of
silence at the head or tail of a real utterance.

Empty text emits **no** `dictation:transcript` — only the terminal `stopped` —
so a mistimed press leaves the composer exactly as it was.

### Hands-free: auto-stop

On the local engine a session ends itself once the user has spoken and then
gone quiet, so dictating is one click rather than two. Apple's engine is
untouched — it streams results while the mic is open, and there is no PCM
buffer on that path to measure.

The detector is two atomics on the capture buffer, updated by the tap on the
render thread. Each buffer's RMS is computed in the same pass that averages the
channels (the samples are already in registers), and counts as speech when it
clears `max(3 × noise floor, MIN_RMS)`. The noise floor is the running
*minimum* of buffer RMS, clamped to `NOISE_FLOOR_MIN`: a laptop's built-in mic
and a hot USB interface differ by more than an order of magnitude in what "a
quiet room" measures, so a fixed threshold would either stop mid-sentence on
one or never trigger on the other. A buffer is judged against the floor as it
stood *before* that buffer is folded in, so a loud first buffer isn't its own
floor. The lower bound is `engine::MIN_RMS`, the same threshold the
[silence gate](#the-silence-gate) uses — audio too quiet to transcribe isn't
worth holding a session open for.

Two consequences of "relative to the floor only", both deliberate. Someone who
is already talking when the first buffer arrives sets the floor to their own
voice, and is recognised at the first gap between words (tap buffers are
~20 ms, so within the first second); the alternative — an absolute "this loud is
always speech" level — was tried and dropped, because steady noise above it
(music, air conditioning) refreshed the speech clock forever and the session
could only end at the capture cap. And steady noise of any level never counts
as speech, so a session in a loud room ends on the no-speech timeout like a
silent one.

"When was speech last heard" is one atomic word (milliseconds plus one, zero
for never) rather than a flag beside a timestamp, so the monitor can never see
"spoken" paired with a timestamp that hasn't landed and mistake the whole
session so far for the pause.

One tokio task per whisper session polls those atomics every
`SILENCE_POLL` (100 ms) and stops on whichever comes first:

| Constant | Value | Ends the session when |
| --- | --- | --- |
| `SILENCE_STOP` | 2 s | Something was said, and nothing has been since |
| `NO_SPEECH_TIMEOUT` | 10 s | Nothing was ever said — clicked the mic, walked away |

The second case stops with no transcript at all: the clip has no speech in it,
so the gate answers empty and only `stopped` is emitted. The stop itself goes
through the same path a second click takes (`stop_session`), so
`transcribing` → transcript → `stopped` arrive in the usual order and the
frontend needs nothing new.

The monitor is scoped to its session's generation at both ends. It exits as
soon as its buffer is marked closed — `Sink::cancel`, which every teardown runs
— and the stop it issues names its own generation, so a monitor that wakes up
after the user already stopped can't cut the *next* session short.

There is no Settings toggle yet. The three constants are the whole policy, so
an opt-out would gate the monitor rather than change them.

### `transcribing`, and the model's lifetime

Apple's engine streams revisions while the user speaks; whisper.cpp has
nothing to say until the whole clip is in. A stop on the local engine therefore
closes the mic, emits a new `dictation:state` of **`transcribing`**, runs the
model, and only then emits the one final transcript and `stopped` (or `error`
with a readable message). `transcribing` is not terminal: `useDictation` treats
it as its `transcribing` phase and the pill shows the "Transcribing…" label —
the same state Apple's post-stop flush shows, just longer.

The weights are hundreds of megabytes and take long enough to load to be felt
between the stop and the text, so a loaded `WhisperContext` is cached and shared
by every session — then dropped after ten minutes (`IDLE_UNLOAD`) without a
transcription. One sleeper task at a time checks the last-use `Instant` rather
than trusting its own deadline, so a session that starts while it sleeps keeps
the model. The cache is keyed on the file it loaded, because a `WhisperContext`
says nothing about which weights it holds and the user can change the choice
between one session and the next.

### Testing it without the GUI

`whisper/engine.rs` has an end-to-end test that is `#[ignore]`d because it needs
real weights. Point it at a model and a **16 kHz mono 16-bit** WAV:

```sh
curl -L -o /tmp/ggml-small.en-q8_0.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q8_0.bin
say -o /tmp/hello.wav --data-format=LEI16@16000 "hello world, this is a dictation test"

FLETCH_WHISPER_TEST_MODEL=/tmp/ggml-small.en-q8_0.bin \
FLETCH_WHISPER_TEST_WAV=/tmp/hello.wav \
  cargo test --manifest-path src-tauri/Cargo.toml transcribes_a_wav -- --ignored --nocapture
```

The test asserts the transcript contains "hello world", case-insensitively.

The silence gate needs no model, so its test always runs; set
`FLETCH_WHISPER_TEST_SILENT_WAV` to try a real recording instead of synthetic
zeroes. The resampler is likewise covered without a microphone — `capture.rs`
puts a 440 Hz tone through it and checks the length and loudness that come out.

## Dictating from the phone

The mobile app has a mic button in its composer, but no speech model on the
phone: it captures the microphone in the webview and streams PCM to the Mac
over the remote protocol, and the Mac runs the local engine on it. The wire
contract is in [remote-protocol.md](remote-protocol.md), "Dictation"; the host
side is `src-tauri/src/dictation/remote.rs`.

What it reuses, and what it doesn't:

- The transcription is `whisper::engine::transcribe` — the same call the desktop
  stop makes, with the same silence gate, the same model cache and the same
  one-decode-at-a-time lock. The resampler is `capture::resample`, so the phone
  can send audio at whatever rate its audio session runs at.
- It requires the local engine: `dictation_engine` set to `whisper` and the
  selected model downloaded — the same two conditions [Dispatch](#dispatch)
  requires. Unlike the desktop there is **no fallback** to Apple's recognizer,
  which needs a microphone the Mac doesn't have; `dictation_status` tells the
  phone why, and it shows the reason instead of a mic.
- The microphone is the phone's, so `apple.rs` is not involved, and neither are
  the desktop's `dictation:*` events: whisper has no partials, so the transcript
  is simply the reply to `dictation_end`.
- Hands-free auto-stop is the same policy, ported to TypeScript in
  `mobile/src/dictation/silence.ts`: the same `SILENCE_STOP` (2 s),
  `NO_SPEECH_TIMEOUT` (10 s) and `SILENCE_POLL` (100 ms), and the same
  RMS-over-noise-floor rule, computed on the main thread from the Float32
  frames the worklet posts (the worklet stays a plain copy) and polled by
  `capture.ts`, which hands the pause to the session — so the stop takes the
  same path a tap on the button takes.

Audio is held in memory only, for the length of a session plus a 60 s idle
sweep, and never written to disk. The phone's `NSMicrophoneUsageDescription`
lives in `mobile/src-tauri/Info.ios.plist`.
