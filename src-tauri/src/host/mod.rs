//! Host-side plumbing: the seam between the engine and whatever is hosting it
//! (today the Tauri desktop, later a headless `fletch-host`).

pub mod ctx;
pub mod sink;

pub use ctx::EngineCtx;
pub use sink::{emit, EventSink, Sink};
