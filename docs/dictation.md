# Voice dictation

The mic button in the agent composer (beside the paperclip) streams a speech
recognizer into the prompt box. This document is the part that isn't obvious
from the code: which platforms have it, what the OS demands before it works,
and where your audio goes.

## Platform support

**Apple only.** The default implementation is Apple's Speech framework
(`SFSpeechRecognizer` fed by an `AVAudioEngine` mic tap), reached through the
`objc2` bindings — the same code compiles for macOS and iOS, with no Swift
toolchain step. Linux and Windows get a stub that reports `supported: false`,
and the composer hides the button entirely rather than offer one that can only
fail.

macOS additionally has a local whisper.cpp engine the user can opt into; see
[The local engine](#the-local-engine) below.

The flow, end to end:

| Layer | File |
| --- | --- |
| Button | `src/components/Composer/dictation/DictationButton.tsx` |
| Session state | `src/components/Composer/dictation/useDictation.ts` |
| IPC | `src/api/domains/dictation.ts` → `dictation_availability` / `dictation_start` / `dictation_stop` |
| Events | `src/api/events.ts` — `dictation:transcript`, `dictation:state` |
| Commands + contract | `src-tauri/src/dictation/mod.rs` |
| Microphone (both engines) | `src-tauri/src/dictation/apple.rs` |
| Local engine | `src-tauri/src/dictation/capture.rs`, `src-tauri/src/dictation/whisper/` |

## Permissions

Apple's recognizer needs **two** separate TCC grants, prompted on the first
`dictation_start` (speech first, then the microphone, one dialog at a time):

- `NSSpeechRecognitionUsageDescription`
- `NSMicrophoneUsageDescription`

Both strings live in `src-tauri/Info.plist`, which the Tauri CLI picks up by
filename and `tauri-codegen` also embeds into the dev binary — so the prompts
work under `tauri dev`, not just in a bundle. The speech string is load-bearing
rather than cosmetic: `requestAuthorization:` crashes the process outright if
it is missing.

The local engine asks for the microphone only. Opting out of Apple's speech
service and then being made to authorize it would be a contradiction, so that
path never calls `requestAuthorization:` — and `dictation_availability`'s
`speech` field is meaningless when `engine` is `whisper`, which is why the
button ignores it there.

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

## Local Whisper engine (opt-in)

Settings › General › Dictation offers a second engine: whisper.cpp running
locally, for people who want transcription that never depends on Apple's
locale support or its servers. Apple's recognizer stays the default — the
local engine costs a one-time model download, so it can only be a choice the
user makes.

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
C++ tree it will never run. iOS therefore keeps Apple's recognizer whatever the
setting says — `dictation/capture.rs` and `dictation/whisper/engine.rs` don't
exist there. No link flags of our own are needed; `whisper-rs-sys` emits the
`c++`/Accelerate/Foundation/Metal/MetalKit lines itself.

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
gone quiet, so dictating is one click rather than two. Apple's recognizer is
untouched — it streams results and manages its own end-of-utterance, and there
is no PCM buffer on that path to measure.

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

Apple's recognizer streams revisions while the user speaks; whisper.cpp has
nothing to say until the whole clip is in. A stop on the local engine therefore
closes the mic, emits a new `dictation:state` of **`transcribing`**, runs the
model, and only then emits the one final transcript and `stopped` (or `error`
with a readable message). `transcribing` is not terminal: `useDictation` treats
it as "still stopping, not listening" and keeps the control held, and the button
swaps the mic for a spinner and says "Transcribing…".

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
