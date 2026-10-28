//! Readiness handshake between init and the long-running services.
//!
//! Init must not print `FLETCH_READY` until the vsock ports are actually bound,
//! or the host's first ssh would be refused. Each service therefore prints one
//! line on stdout once it listens; init runs the service with stdout on a FIFO
//! and reads that line. After it, stdout is pointed at stderr (the console) so
//! the FIFO has no lingering writer and later output cannot block on it.

pub fn announce(what: &str) {
    println!("fletch-guest: listening on {what}");
    // SAFETY: plain dup2 of two inherited descriptors; no Rust object owns them.
    unsafe {
        libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO);
    }
}
