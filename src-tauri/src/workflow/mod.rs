//! Workflows v1 — a robust, observable, budget-bounded pipeline of sandboxed
//! agents. See `docs/workflows/TECH_SPEC.md`.
//!
//! This slice (S2) lands the definition layer: the spec types + validation
//! (`spec`), the portable YAML format (`yaml`), and the `wf_def_*` storage
//! commands (`definition`). The scheduler, journal, and run types arrive in
//! sibling modules with later slices; when S1 lands its `mod.rs` this file
//! merges to also declare `types`/`journal` and the `WorkflowService`.

pub mod definition;
pub mod spec;
pub mod yaml;
