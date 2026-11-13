//! `fletch-guest socket-activate` end to end: nothing runs until something
//! connects, then the command runs in its place with the listening socket as
//! fd 3, named by systemd's socket activation variables.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn the_command_starts_on_the_first_connection_holding_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, started) = (dir.path().join("d.sock"), dir.path().join("started"));
    let script = r#"touch "$1"; [ -S /dev/fd/3 ] && echo "$LISTEN_FDS $LISTEN_PID $$""#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_fletch-guest"))
        .arg("socket-activate")
        .arg(&socket)
        .args(["--", "/bin/sh", "-c", script, "sh"])
        .arg(&started)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();

    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("listening on"), "{ready}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!started.exists(), "the command ran before any connection");

    let _client = UnixStream::connect(&socket).unwrap();
    let status = child.wait().unwrap();
    let mut said = String::new();
    child.stderr.unwrap().read_to_string(&mut said).unwrap();
    assert!(status.success(), "{said}");
    assert_eq!(said, format!("1 {pid} {pid}\n"));
}
