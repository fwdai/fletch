//! The headless Fletch host.
//!
//! `fletch-host serve` boots the same engine the desktop boots
//! (`fletch_core::host::boot`) with no window and no webview, forces paired
//! remote access on, and then does one thing of its own: it serves a local
//! control socket so the machine's owner can pair a phone, answer an approval
//! or add a project from a terminal.
//!
//! Layout:
//!
//! - [`serve`] — the boot configuration and the process's main loop.
//! - [`admin`] — the control socket: where it lives, what a frame looks like,
//!   and both ends of it.
//! - [`ops`] — what the socket can ask the engine to do.
//! - [`login`] — the GitHub device flow, held across two admin calls.
//! - [`provider`] — `provider status|login`: the agent CLIs on this machine.
//! - [`cloud`] — `cloud status|connect|disconnect`: the cloud provider a
//!   workspace's machine is made at where this host cannot boot a box.
//! - [`service`] — `service install|uninstall`: the systemd unit and the
//!   launchd agent, rendered from the templates in `/packaging`.
//! - [`update`] — `update`: resolve a release, verify it, swap this binary.
//!
//! A library as well as a binary so the end-to-end test can boot a host in
//! process and talk to it exactly as the CLI does.

pub mod admin;
pub mod cloud;
pub mod login;
pub mod ops;
pub mod provider;
pub mod serve;
pub mod service;
pub mod update;
