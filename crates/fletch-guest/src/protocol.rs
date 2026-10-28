//! The JSON lines `relay` and `tail` exchange with the host over an ssh session.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Event {
    /// A mailbox request file, guest → host. `body` is the file's text.
    Request { name: String, body: String },
    /// A mailbox response, host → guest, written to `responses/<name>`.
    Response { name: String, body: String },
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
