//! `wait-stop <vsock port>`: returns once the host connects and sends `stop`.
//! Init's last step before it unmounts `/data` and exits (spec §6.3). Anything
//! else on the socket is logged and ignored, so a stray connection cannot stop
//! a box.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use crate::{parse_port, ready, vsock, Result};

pub fn run(args: &[String]) -> Result<()> {
    let port = parse_port(args.first(), "vsock port")?;
    let listener = vsock::listen(port)?;
    ready::announce(&format!("vsock {port} (control)"));

    loop {
        let mut conn = listener.accept()?;
        let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        let _ = BufReader::new(&conn).read_line(&mut line);
        if line.trim() == "stop" {
            let _ = conn.write_all(b"stopping\n");
            return Ok(());
        }
        eprintln!("fletch-guest: wait-stop: ignored {:?}", line.trim());
    }
}
