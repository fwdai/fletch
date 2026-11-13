//! `reclaim`: gives the guest's file cache back to the host once work ends.
//! Run as root (the host runs `sudo -n /opt/fletch/bin/fletch-guest reclaim`):
//! dirty pages are written out, the clean page cache and slab caches are
//! dropped, and memory is compacted, which is what frees whole pages for the
//! VM to return.

use std::fs;

use crate::Result;

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err("reclaim takes no arguments".into());
    }
    // SAFETY: sync takes no arguments and cannot fail.
    unsafe { libc::sync() };
    for (knob, value) in [
        ("/proc/sys/vm/drop_caches", "3"),
        ("/proc/sys/vm/compact_memory", "1"),
    ] {
        fs::write(knob, value).map_err(|e| format!("reclaim: writing {knob}: {e}"))?;
    }
    Ok(())
}
