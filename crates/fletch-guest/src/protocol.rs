//! The JSON lines `relay` and `tail` exchange with the host over an ssh session.
//!
//! The mailbox half is acknowledged in both directions: a `request` or
//! `response` stays on disk at its sender until the peer answers it with an
//! `ack` for the same name, so a session that drops mid-flight loses nothing —
//! the next session re-emits whatever is still there, and the receiver dedupes
//! by name (the names are UUIDs the agent chose) with a receipt marker on disk.
//! Each sender says `synced` once it has re-emitted everything it still held
//! at session start; the receiver then drops every marker it was not shown,
//! since nothing can replay those names any more.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Event {
    /// A mailbox request file, guest → host. `body` is the file's text.
    Request { name: String, body: String },
    /// A mailbox response, host → guest, written to `responses/<name>`.
    Response { name: String, body: String },
    /// The peer has durably written `name` (a request on the host, a response
    /// in the guest); the sender may now delete its copy.
    Ack { name: String },
    /// The sender has re-emitted every file it still held when the session
    /// began (requests from the guest, responses from the host); the receiver
    /// may drop receipt markers for every other name.
    Synced,
    /// `tail` saw a file for the first time (or saw it shrink and start over).
    Create { path: String },
    /// `tail` read new bytes; `data` is base64.
    Append { path: String, data: String },
}

pub fn write_line(out: &mut impl Write, event: &Event) -> io::Result<()> {
    serde_json::to_writer(&mut *out, event)?;
    out.write_all(b"\n")?;
    out.flush()
}
