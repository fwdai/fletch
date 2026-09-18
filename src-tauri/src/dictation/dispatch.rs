//! The desktop's answer to the five `dictation_*` remote ops.
//!
//! The engine's dispatcher (`fletch_core::remote::dispatch`) owns every other
//! op, but not these: transcription needs whisper.cpp, which is built on macOS
//! only and lives in this crate beside the microphone code it shares. So the
//! engine keeps the names on the wire and consults this extension for them; a
//! host without one answers "unavailable" instead (see
//! `SupervisorDispatch::with_dictation`).
//!
//! The arms are the ones that used to sit in the engine's own `match`, moved
//! whole — same argument keys, same functions, same replies.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use fletch_core::host::EngineCtx;
use fletch_core::remote::{Dispatch, DispatchFuture, DispatchResult, UNKNOWN_OP};

/// Names a phone's open dictation session. See `dictation::remote`.
#[derive(Deserialize)]
struct SessionArgs {
    session: String,
}

/// One chunk of a phone's dictation: 16-bit little-endian mono PCM, base64,
/// at the rate the phone captured it. See `dictation::remote`.
#[derive(Deserialize)]
struct AudioArgs {
    session: String,
    rate: f64,
    pcm: String,
}

/// Remote-only: the phone captures, this Mac transcribes with the local
/// whisper engine (`dictation::remote`). The transcript is the reply to
/// `dictation_end`; no event is involved. The engine settings these read come
/// off the ctx's DB handle — the same connection the Tauri commands reach
/// through `State`, so a phone sees exactly what Settings › Dictation shows.
pub struct DictationDispatch {
    ctx: Arc<EngineCtx>,
}

impl DictationDispatch {
    pub fn new(ctx: Arc<EngineCtx>) -> Self {
        Self { ctx }
    }
}

impl Dispatch for DictationDispatch {
    fn dispatch<'a>(&'a self, op: &'a str, args: Value) -> DispatchFuture<'a> {
        Box::pin(async move {
            let db = &self.ctx.db;
            match op {
                "dictation_status" => ok(super::remote::status(db)),
                "dictation_begin" => res(super::remote::begin(db)),
                "dictation_audio" => {
                    let a: AudioArgs = parse(args)?;
                    res(super::remote::append(&a.session, a.rate, &a.pcm))
                }
                "dictation_end" => {
                    let a: SessionArgs = parse(args)?;
                    res(super::remote::end(db, &a.session).await)
                }
                "dictation_cancel" => {
                    let a: SessionArgs = parse(args)?;
                    super::remote::cancel(&a.session);
                    Ok(Value::Null)
                }
                // Unreachable in production — the engine only routes the five
                // above here — but the trait is the whole op surface, so this
                // fails closed like any other unknown name.
                _ => Err(UNKNOWN_OP.to_string()),
            }
        })
    }
}

/// Serialize a command's value reply, exactly as Tauri's IPC would.
fn ok<T: serde::Serialize>(value: T) -> DispatchResult {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Commands answer with `crate::error::Result`; the wire carries the `Display`
/// of the error, per the protocol doc.
fn res<T: serde::Serialize>(result: crate::error::Result<T>) -> DispatchResult {
    ok(result.map_err(|e| e.to_string())?)
}

/// Deserialize an op's arguments, reporting a bad payload the way a rejected
/// `invoke` does.
fn parse<T: serde::de::DeserializeOwned>(args: Value) -> std::result::Result<T, String> {
    serde_json::from_value(args).map_err(|e| e.to_string())
}
