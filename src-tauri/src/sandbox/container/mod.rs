//! Runtime-neutral container launch policy: everything about running an agent
//! in a container that is not specific to one container runtime.
//!
//! `sandbox::docker` and `sandbox::podman` are the *runtimes* (binary
//! resolution, daemon probe, CLI invocations, `SandboxEngine` impls); the policy
//! those invocations carry out lives here as pure data and pure functions, so
//! both runtimes reuse it verbatim and no policy change can land on only one.

pub mod auth;
pub(crate) mod config_dir;
pub(crate) mod freshness;
pub(crate) mod image_gc;
pub(crate) mod images;
pub(crate) mod labels;
pub(crate) mod launch;
pub(crate) mod launch_auth;
pub(crate) mod proc;
pub mod progress;
pub(crate) mod provider;
pub(crate) mod run_args;
pub(crate) mod sweep;
pub(crate) mod util;
pub(crate) mod version_guard;

pub use provider::ContainerProvider;
