//! The Podman sandbox engine's primitives. A skeleton: this module can say
//! whether Podman is usable, and nothing more.
//!
//! [`EngineKind::Podman`](crate::sandbox::EngineKind::Podman) exists and probes,
//! but `sandbox::engine_for` refuses it unconditionally — there is no
//! `SandboxEngine` implementation here yet, and no image build or cleanup.
//! Shipping the kind ahead of the launch path is only safe because that refusal
//! is unconditional rather than probe-gated: a Podman-stamped agent cannot
//! launch even on a machine where the probe reports the runtime healthy, so
//! there is no path on which a launch silently escapes the container boundary
//! the user picked.
//!
//! Everything here must work when Podman is absent or its machine is down:
//! probing reports that state instead of erroring.
//!
//! Layout mirrors [`docker`](crate::sandbox::docker) — the Podman *runtime* on
//! top of the runtime-neutral policy in [`crate::sandbox::container`], which
//! Podman will reuse unchanged (identical-path binds, the same labels, the same
//! image content):
//! - [`cli`] — podman binary resolution + bounded-invocation wrappers. Every
//!   podman call in this module goes through it, so no invocation can hang the
//!   app on a wedged machine connection.
//! - [`probe`] — cached availability for UI polling.

mod cli;
mod probe;

pub use probe::{availability, PodmanAvailability};
