//! Runtime-neutral container launch policy: everything about running an agent
//! in a container that is not specific to one container runtime.
//!
//! `sandbox::docker` is the Docker *runtime* — binary resolution, the daemon
//! probe, `docker build`/`run`/`rm` invocations, and the `SandboxEngine` impl.
//! What lives here is the policy those invocations carry out, expressed as pure
//! data and pure functions so a sibling runtime (podman) can reuse it verbatim:
//!
//! - [`provider`] — which providers can run in a container at all
//!   ([`ContainerProvider`]), their ids and in-image binaries.
//! - [`launch`] — per-launch policy: the container env, the mount sources it
//!   creates, and the per-provider config/data/auth preparation. What both
//!   runtimes' `launch_agent` calls before building any argv.
//! - [`config_dir`] — non-default config-dir detection and borrowed object stores.
//! - [`run_args`] — the `run` argv builder: mounts at identical host paths, the
//!   per-provider config/data mounts, and the bare `-e NAME` auth forwards.
//! - [`labels`] — the `fletch.host-pid` / `fletch.agent-id` labels every
//!   container carries so orphan and per-agent sweeps can attribute it, plus
//!   the pid-liveness parsing those sweeps share.
//! - [`sweep`] — the startup orphan sweep's probe-retry schedule.
//! - [`images`] — the embedded Dockerfiles/entrypoints and their
//!   content-addressed tags. Content only: nothing here builds or inspects.
//! - [`freshness`] — the image TTL and the host/in-image version-parity
//!   decision: when a built image has gone stale enough to rebuild.
//! - [`version_guard`] — the per-runtime loop guard that caps version-parity
//!   rebuild attempts at one per `host@tag` pairing.
//! - [`image_gc`] — which superseded agent images to reclaim: the listing
//!   format, its row parsing, and the selection rule.
//! - [`progress`] — the process-wide image-build progress sink the UI toast
//!   listens on.
//! - [`auth`] — the Anthropic credential chain for containerized agents.
//! - [`launch_auth`] — per-provider launch preparation: folding resolved
//!   credentials into the CLI process env and failing fast without one.
//! - [`proc`] — bounded subprocess execution (timeouts, line forwarding), the
//!   machinery every runtime CLI invocation is funneled through.
//! - [`util`] — container naming and the reserved-exit-code messages.

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
