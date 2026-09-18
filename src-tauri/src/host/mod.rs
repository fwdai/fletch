//! Host-side plumbing: the seam between the engine and whatever is hosting it
//! (today the Tauri desktop, later a headless `fletch-host`).

pub mod boot;
pub mod ctx;
pub mod runtime;
pub mod sink;

pub use boot::{boot, BootConfig, Engine, RemoteBoot};
pub use ctx::EngineCtx;
pub use runtime::spawn;
pub use sink::{emit, Event, EventSink, Sink};
