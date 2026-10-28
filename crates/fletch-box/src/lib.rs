//! The box sandbox runtime (docs/boxes-spec.md): libkrun micro-VMs with no
//! shared folder and no network device, reachable from the host only over SSH
//! on vsock ([`Channel`]) and an egress proxy ([`EgressProxy`]).
//!
//! This crate depends on nothing else in Fletch. The engine in `fletch-core`
//! composes these pieces per box; the `fletch-box` binary drives them by hand.

pub mod channel;
pub mod disks;
pub mod egress;
mod error;
pub mod guest;
pub mod image_store;
pub mod runtime;
pub mod tools;

pub use channel::{Channel, Forward};
pub use egress::{EgressEvent, EgressPolicy, EgressProxy, HostPattern};
pub use error::{Error, Result};
pub use image_store::{ImageStore, InstalledImage, PinnedImage};
pub use runtime::{HostKey, Probe, RuntimePaths, StopOutcome, Vm, VmSpec};
