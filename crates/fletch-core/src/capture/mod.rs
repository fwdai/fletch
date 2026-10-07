//! How context is captured: the deterministic base read from the repository
//! once (`bootstrap`), the background extractor over a session's turns
//! (`extract`) and deterministic ingestion from merged PRs plus the checkout
//! lifecycle hooks that settle provisional records (`ingest`). All read the
//! store and write only through `context::ContextService`; the store's write
//! methods are `pub(super)` to `context`, so this module cannot reach them.

pub mod bootstrap;
pub mod extract;
pub mod ingest;
