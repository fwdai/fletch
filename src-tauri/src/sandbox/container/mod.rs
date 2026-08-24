//! Shared **container launch policy** used by the Docker and Podman sandbox
//! engines.
//!
//! This module owns the product-level shape of a sandboxed agent container —
//! what gets mounted, what env is forwarded, how Claude's config is treated —
//! not the runtime plumbing that actually talks to a container CLI. Docker and
//! Podman each keep their own binary resolution, availability probe, image
//! lifecycle, cleanup, and kill semantics; they both build a [`RunSpec`] and
//! hand it to [`run_args`] so the mount/env argv stays identical.
//!
//! Layout:
//! - [`labels`] — `fletch.host-pid` / `fletch.agent-id` stamped on every
//!   container launch (cleanup keys on these)
//! - [`run_args`] — `ProviderMounts`, [`RunSpec`], and the pure argv builder
//! - [`config_dir`] — non-default config-dir detection and borrowed object stores

pub mod config_dir;
pub mod labels;
pub mod run_args;

pub use config_dir::borrowed_object_stores;
pub use labels::{agent_id_label, host_pid_label, AGENT_ID_LABEL, HOST_PID_LABEL};
pub use run_args::{
    prepare_config_mount_dir, run_args, ProviderMounts, RunSpec, CREDENTIALS_FILE,
};
