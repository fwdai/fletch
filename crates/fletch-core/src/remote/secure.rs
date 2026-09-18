//! The remote connection's secure channel, under the module path the rest of
//! `remote/` imports from.
//!
//! The code itself lives in `crates/fletch-proto`: the Noise pattern, the
//! prologue, the chunked frame codec and the static key file are the same on
//! both ends of the protocol, and the phone (`mobile/src-tauri`) builds against
//! the same crate — which is what makes "a host and a client agree on the wire"
//! a compile-time fact rather than two files kept in step by hand.
//!
//! The host is the responder ([`respond`]); the initiator half is what the
//! phone runs, and what the tests here drive through [`Handshake`].

pub use fletch_proto::keys::{encode_key, StaticKey as HostKey, HOST_KEY_FILE};
pub use fletch_proto::noise::{respond, Channel as SecureChannel};

/// The initiator half. Production never runs it on this end — the phone does —
/// so only the tests here reach for it.
#[cfg(test)]
pub use fletch_proto::noise::Handshake;
