// The Swift half of the default dictation engine: Apple's `SpeechAnalyzer`
// (macOS 26+). It has no Objective-C surface — Swift concurrency and
// `AsyncSequence` throughout — so it can't be reached from `objc2` the way the
// rest of `dictation/apple.rs` reaches AVFoundation. `build.rs` compiles this
// file into a static library that is linked into the Rust binary; the surface
// is plain C (`@_cdecl`), declared on the Rust side in `dictation/speech.rs`.
// Keep the two in sync.
//
// Rust owns every session: it opens the microphone, decides when a session
// starts and stops, and holds the handle `fletch_speech_start` returns. What
// lives here is only what has to be Swift — the analyzer and transcriber, the
// audio-format conversion, the model-asset check, and the running-transcript
// bookkeeping.
//
// Every entry point is guarded with `#available(macOS 26, *)`: the binary
// still launches on older macOS, where dictation reports unsupported and the
// composer hides the mic button.

import AVFAudio
import Foundation
import Speech

// MARK: - C surface

/// `(ctx, ok, message)`. Exactly one reply per request; `message` is set when
/// `ok` is false and is fit to show the user.
public typealias FletchSpeechReady = @convention(c) (UnsafeMutableRawPointer?, Bool, UnsafePointer<CChar>?) -> Void
/// `(ctx, text, is_final)`. `text` is the whole running transcript so far —
/// finalized prefix plus the current volatile tail — never a delta.
public typealias FletchSpeechResult = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?, Bool) -> Void
/// `(ctx, message)`. The session failed; no result follows.
public typealias FletchSpeechError = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void
/// `(ctx)`. The session object is gone and no callback will follow, so Rust
/// may free `ctx`. Fired from `deinit`, which is after every task that could
/// call back has ended.
public typealias FletchSpeechRelease = @convention(c) (UnsafeMutableRawPointer?) -> Void

private let unsupportedOS = "Dictation needs macOS 26 or later."
private let unsupportedLocale = "Dictation doesn't support this Mac's language."

/// Can the default engine run here at all: macOS 26+, and a model for this
/// Mac's language. Doesn't touch the model assets, so it is safe to call on
/// every mount of the composer.
@_cdecl("fletch_speech_availability")
public func fletchSpeechAvailability(_ ctx: UnsafeMutableRawPointer?, _ done: FletchSpeechReady) {
    guard #available(macOS 26, *) else {
        reply(done, ctx, ok: false, unsupportedOS)
        return
    }
    Task {
        let ok = await Bridge.matchedLocale() != nil
        reply(done, ctx, ok: ok, ok ? nil : unsupportedLocale)
    }
}

/// Everything a session needs that is asynchronous: the locale, the model
/// assets (downloaded on demand — a missing model is reported as a readable
/// failure while the download runs in the background), and the audio format
/// the analyzer wants. Runs before the mic opens, so `fletch_speech_start`
/// itself can be synchronous.
@_cdecl("fletch_speech_prepare")
public func fletchSpeechPrepare(_ ctx: UnsafeMutableRawPointer?, _ done: FletchSpeechReady) {
    guard #available(macOS 26, *) else {
        reply(done, ctx, ok: false, unsupportedOS)
        return
    }
    Task {
        let failure = await Bridge.prepare()
        reply(done, ctx, ok: failure == nil, failure)
    }
}

/// Start a session that accepts microphone buffers in `format` (an
/// `AVAudioFormat`, unretained). Returns a retained handle for the other
/// entry points, or nil when `fletch_speech_prepare` hasn't succeeded yet.
/// On nil no callback — `onRelease` included — is ever made.
@_cdecl("fletch_speech_start")
public func fletchSpeechStart(
    _ format: UnsafeMutableRawPointer,
    _ ctx: UnsafeMutableRawPointer?,
    _ onResult: FletchSpeechResult,
    _ onError: FletchSpeechError,
    _ onRelease: FletchSpeechRelease
) -> UnsafeMutableRawPointer? {
    guard #available(macOS 26, *), let prepared = Bridge.prepared() else { return nil }
    let input = Unmanaged<AVAudioFormat>.fromOpaque(format).takeUnretainedValue()
    guard
        let session = Session(
            input: input, prepared: prepared, ctx: ctx,
            onResult: onResult, onError: onError, onRelease: onRelease)
    else { return nil }
    session.run()
    return Unmanaged.passRetained(session).toOpaque()
}

/// Hand the analyzer one tap buffer (an `AVAudioPCMBuffer`, unretained, valid
/// only for the duration of the call). Real-time render thread.
@_cdecl("fletch_speech_feed")
public func fletchSpeechFeed(_ handle: UnsafeMutableRawPointer, _ buffer: UnsafeMutableRawPointer) {
    guard #available(macOS 26, *) else { return }
    Unmanaged<Session>.fromOpaque(handle).takeUnretainedValue()
        .feed(Unmanaged<AVAudioPCMBuffer>.fromOpaque(buffer).takeUnretainedValue())
}

/// The user stopped: no more audio. The analyzer finalizes what it has and
/// the results sequence ends, which is what produces the final `onResult`.
@_cdecl("fletch_speech_finish")
public func fletchSpeechFinish(_ handle: UnsafeMutableRawPointer) {
    guard #available(macOS 26, *) else { return }
    Unmanaged<Session>.fromOpaque(handle).takeUnretainedValue().finish()
}

/// Drop the session without waiting for anything. Must be the handle's last
/// use: this balances the retain from `fletch_speech_start`. The session's own
/// tasks keep the object alive until they wind down, and `onRelease` fires
/// only then.
@_cdecl("fletch_speech_cancel")
public func fletchSpeechCancel(_ handle: UnsafeMutableRawPointer) {
    guard #available(macOS 26, *) else { return }
    Unmanaged<Session>.fromOpaque(handle).takeRetainedValue().cancel()
}

private func reply(
    _ done: FletchSpeechReady, _ ctx: UnsafeMutableRawPointer?, ok: Bool, _ message: String?
) {
    if let message {
        message.withCString { done(ctx, ok, $0) }
    } else {
        done(ctx, ok, nil)
    }
}

// MARK: - Process-wide state

/// What `prepare` settles once and every session then reads: which locale to
/// transcribe in and which audio format the analyzer wants.
@available(macOS 26, *)
private enum Bridge {
    struct Prepared {
        let locale: Locale
        let format: AVAudioFormat
    }

    private static let lock = NSLock()
    private static var current: Prepared?
    /// The running model download, if any. One at a time: a second `prepare`
    /// while it runs reports the same download rather than starting another.
    private static var download: Progress?

    static func prepared() -> Prepared? {
        lock.withLock { current }
    }

    static func transcriber(for locale: Locale) -> SpeechTranscriber {
        // Volatile results are what make the composer's text move while the
        // user is still speaking; nothing else (timing, confidence) is used.
        SpeechTranscriber(
            locale: locale, transcriptionOptions: [], reportingOptions: [.volatileResults],
            attributeOptions: [])
    }

    /// The supported locale to transcribe in: this Mac's locale exactly, else
    /// any variant of its language (a Mac set to English/Poland still dictates
    /// in English). Nil means no model speaks the language at all.
    static func matchedLocale() async -> Locale? {
        let wanted = Locale.current
        let supported = await SpeechTranscriber.supportedLocales
        let id = wanted.identifier(.bcp47)
        if let exact = supported.first(where: { $0.identifier(.bcp47) == id }) {
            return exact
        }
        guard let language = wanted.language.languageCode else { return nil }
        return supported.first { $0.language.languageCode == language }
    }

    /// Nil on success; otherwise a message for the user.
    static func prepare() async -> String? {
        guard let locale = await matchedLocale() else { return unsupportedLocale }
        let module = transcriber(for: locale)
        // Ask the OS to keep this locale's model resident. `false` means the
        // per-app reservation cap is full, which only makes a later eviction
        // possible — not a reason to refuse the session.
        _ = try? await AssetInventory.reserve(locale: locale)
        do {
            // Nil means every asset the transcriber needs is already installed.
            if let request = try await AssetInventory.assetInstallationRequest(supporting: [module]) {
                return downloading(request, for: locale)
            }
        } catch {
            return "Couldn't check the speech model: \(error.localizedDescription)"
        }
        guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [module]) else {
            return "Speech recognition has no usable audio format on this Mac."
        }
        lock.withLock { current = Prepared(locale: locale, format: format) }
        return nil
    }

    /// Start the download unless one is running, and describe it. The session
    /// is refused rather than made to wait: the model is hundreds of megabytes
    /// and a mic button that hangs reads as broken.
    private static func downloading(_ request: AssetInstallationRequest, for locale: Locale) -> String {
        let progress = lock.withLock { () -> Progress in
            if let running = download { return running }
            download = request.progress
            Task {
                do {
                    try await request.downloadAndInstall()
                } catch {
                    NSLog("dictation: speech model download failed: %@", error.localizedDescription)
                }
                lock.withLock { download = nil }
            }
            return request.progress
        }
        let code = locale.language.languageCode?.identifier ?? ""
        let language = Locale.current.localizedString(forLanguageCode: code) ?? "this Mac's language"
        let percent = Int(progress.fractionCompleted * 100)
        return "Downloading the speech model for \(language) (\(percent)%). Try again in a moment."
    }
}

// MARK: - One session

/// One analyzer run, alive from `fletch_speech_start` to the end of its tasks.
///
/// Threading: `feed` runs on the render thread and touches only the converter
/// and the stream continuation; `finalized` is touched only by the results
/// task; `finish`/`cancel` only finish the continuation and spawn a task.
/// Nothing here needs a lock, which is what `@unchecked Sendable` asserts.
@available(macOS 26, *)
private final class Session: @unchecked Sendable {
    private let ctx: UnsafeMutableRawPointer?
    private let onResult: FletchSpeechResult
    private let onError: FletchSpeechError
    private let onRelease: FletchSpeechRelease

    private let transcriber: SpeechTranscriber
    private let analyzer: SpeechAnalyzer
    private let format: AVAudioFormat
    private let converter: AVAudioConverter
    private let stream: AsyncStream<AnalyzerInput>
    private let input: AsyncStream<AnalyzerInput>.Continuation
    /// Text the transcriber has committed to, in order. Volatile results are
    /// appended to this for display but never stored.
    private var finalized = ""

    init?(
        input: AVAudioFormat, prepared: Bridge.Prepared, ctx: UnsafeMutableRawPointer?,
        onResult: FletchSpeechResult, onError: FletchSpeechError, onRelease: FletchSpeechRelease
    ) {
        guard let converter = AVAudioConverter(from: input, to: prepared.format) else { return nil }
        // No priming: the first buffer should come out as soon as it goes in,
        // not after the converter has buffered a filter's worth of latency.
        converter.primeMethod = .none
        self.converter = converter
        self.format = prepared.format
        self.ctx = ctx
        self.onResult = onResult
        self.onError = onError
        self.onRelease = onRelease
        transcriber = Bridge.transcriber(for: prepared.locale)
        analyzer = SpeechAnalyzer(modules: [transcriber])
        (stream, self.input) = AsyncStream<AnalyzerInput>.makeStream()
    }

    deinit {
        onRelease(ctx)
    }

    func run() {
        // Subscribe to results before starting the analyzer so nothing is
        // missed. The loop ends when the analyzer finishes — after `finish`'s
        // finalization or `cancel` — and its normal end is the final result.
        Task { [self] in
            do {
                for try await result in transcriber.results {
                    let text = String(result.text.characters)
                    if result.isFinal {
                        finalized += text
                        report(finalized, final: false)
                    } else {
                        report(finalized + text, final: false)
                    }
                }
                report(finalized, final: true)
            } catch {
                fail(error)
            }
        }
        Task { [self] in
            do {
                try await analyzer.start(inputSequence: stream)
            } catch {
                fail(error)
            }
        }
    }

    /// Convert one tap buffer to the analyzer's format and queue it. The
    /// conversion doubles as the copy: the engine reuses the tap's buffer
    /// once the tap returns, and the converted buffer is a fresh allocation.
    func feed(_ buffer: AVAudioPCMBuffer) {
        let ratio = format.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 1
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return }
        var consumed = false
        var error: NSError?
        let status = converter.convert(to: out, error: &error) { _, inputStatus in
            // One buffer in per call; `noDataNow` (not `endOfStream`) keeps
            // the converter's resampling state alive for the next one.
            if consumed {
                inputStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            inputStatus.pointee = .haveData
            return buffer
        }
        if status != .error, out.frameLength > 0 {
            input.yield(AnalyzerInput(buffer: out))
        }
    }

    func finish() {
        input.finish()
        Task { [self] in
            do {
                try await analyzer.finalizeAndFinishThroughEndOfInput()
            } catch {
                fail(error)
            }
        }
    }

    func cancel() {
        input.finish()
        Task { [self] in
            await analyzer.cancelAndFinishNow()
        }
    }

    private func report(_ text: String, final: Bool) {
        text.withCString { onResult(ctx, $0, final) }
    }

    private func fail(_ error: Error) {
        error.localizedDescription.withCString { onError(ctx, $0) }
    }
}
