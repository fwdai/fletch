//! The box sandbox runtime: libkrun micro-VMs with no network device and no
//! shared folder but the ones the user asks for, reachable from the host only
//! over SSH on vsock ([`Channel`]) and an egress proxy ([`EgressProxy`]). The
//! guest holds the agent's tree at the same absolute paths the host uses, so
//! nothing here translates a path; nothing of the host's enters the guest
//! except the env file written for each launch and the folders a spec shares
//! ([`VmSpec::shares`], each at its own path, read-only unless the user made
//! it writable); and a step that cannot establish what it promises fails
//! closed rather than falling back to the host.
//!
//! This crate depends on nothing else in Fletch. The engine in `fletch-core`
//! composes these pieces per box; the `fletch-box` binary drives them by hand.

pub mod channel;
pub mod disks;
pub mod egress;
mod error;
pub mod fsutil;
pub mod guest;
pub mod image_store;
pub mod runtime;
pub mod tools;
pub mod version;

pub use channel::{Channel, Forward};
pub use egress::{EgressEvent, EgressPolicy, EgressProxy, HostPattern};
pub use error::{Error, Result};
pub use image_store::{ImageStore, InstalledImage, InstalledLayer, PinnedImage};
pub use runtime::{HostKey, Probe, RuntimePaths, StopOutcome, Vm, VmSpec};
