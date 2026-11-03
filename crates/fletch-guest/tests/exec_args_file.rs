//! `fletch-guest exec --args-file` end to end: the built binary execs a real
//! command, which prints back the arguments it was given.

use std::path::Path;
use std::process::Command;

/// Each argument followed by a NUL, the args file's format and what
/// `printf '%s\0' "$@"` prints.
fn nul_ended<'a>(args: impl IntoIterator<Item = &'a str>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for arg in args {
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0);
    }
    bytes
}

fn exec(env_file: &Path, args_file: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fletch-guest"))
        .arg("exec")
        .arg("--env-file")
        .arg(env_file)
        .arg("--args-file")
        .arg(args_file)
        .args(["--", "/bin/sh", "-c", "printf '%s\\0' \"$@\"", "sh", "own"])
        .output()
        .unwrap()
}

/// Three arguments of 100 KiB each: every one fits Linux's 128 KiB cap on a
/// single argument, while together they are past what the one ssh command
/// string sshd hands the guest shell could carry. Each reaches the command
/// whole, after the command's own, quotes and newlines untouched.
#[test]
fn the_files_arguments_reach_the_command_after_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let env_file = dir.path().join("t.env");
    std::fs::write(&env_file, "PATH=/usr/bin:/bin\n").unwrap();
    let big: Vec<String> = ['a', 'b', 'c']
        .iter()
        .map(|c| c.to_string().repeat(100 * 1024))
        .collect();
    let tricky = ["it's", "a \"quote\"", "$HOME", "", "new\nline", "*"];
    let args: Vec<&str> = big.iter().map(String::as_str).chain(tricky).collect();
    let args_file = dir.path().join("1.42.args");
    std::fs::write(&args_file, nul_ended(args.iter().copied())).unwrap();

    let out = exec(&env_file, &args_file);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = nul_ended(std::iter::once("own").chain(args.iter().copied()));
    assert!(out.stdout == expected, "the arguments came back changed");
    assert!(!args_file.exists(), "the args file is removed once read");
}

/// An empty file adds nothing; a missing one stops the launch before the
/// command runs.
#[test]
fn an_empty_args_file_adds_nothing_and_a_missing_one_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let env_file = dir.path().join("t.env");
    std::fs::write(&env_file, "").unwrap();
    let args_file = dir.path().join("1.42.args");
    std::fs::write(&args_file, b"").unwrap();

    let out = exec(&env_file, &args_file);
    assert!(out.status.success());
    assert_eq!(out.stdout, nul_ended(["own"]));

    let out = exec(&env_file, &args_file);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "the command never ran");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot read"), "{stderr}");
}
