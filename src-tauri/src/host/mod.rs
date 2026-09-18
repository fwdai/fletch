//! Host-side plumbing: the seam between the engine and whatever is hosting it
//! (today the Tauri desktop, later a headless `fletch-host`).

pub mod sink;

pub use sink::{emit, EventSink, Sink};
