//! The Fletch remote protocol, with no Tauri and no engine in it.
//!
//! Everything here is fixed by `docs/remote-protocol.md`: the Noise pattern and
//! prologue, the chunked frame codec, the static key files, the relay's
//! host-link proof and its multiplexing framing. A host and a client built
//! against this crate speak the same wire by construction, which is the whole
//! reason it exists — the desktop (`src-tauri/`) and the phone
//! (`mobile/src-tauri/`) each used to carry their own copy.
//!
//! Layout:
//!
//! - [`keys`] — the static X25519 identity, its file, and the bare DH.
//! - [`noise`] — the `XX` handshake (both roles) and the encrypted frame codec.
//! - [`dial`] — opening a WebSocket with both address families in the race.
//! - [`relay`] — the host-link proof and the multiplexing frame codec.
//! - [`client`] — a client's live connections to many hosts, Tauri-free.
//!
//! What is *not* here: the host's state machine. Its listener, its op dispatch
//! and its relay link live in the desktop. That is policy; this is the wire and
//! the client end of it, which the phone and the desktop share.

pub mod client;
pub mod dial;
pub mod keys;
pub mod noise;
pub mod relay;

/// Every fallible call in this crate reports a message meant to be shown to a
/// person as-is.
///
/// A `String` rather than an error enum on purpose: nothing in either app
/// branches on the *kind* of protocol failure — a failed handshake closes the
/// socket and a bad key file is shown to the user — so an enum would only add a
/// conversion at every call site. The one failure a caller does distinguish
/// carries its own marker string ([`noise::HOST_KEY_MISMATCH`]).
pub type Result<T> = std::result::Result<T, String>;

pub use client::{ClientEvent, ConnectResult, ConnectionId, Dialer, Target};
pub use keys::{encode_key, StaticKey, DEVICE_KEY_FILE, HOST_KEY_FILE, KEY_LEN};
pub use noise::{Channel, Handshake, HOST_KEY_MISMATCH, MAX_CHUNK_PLAINTEXT};
