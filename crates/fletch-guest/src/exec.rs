//! `exec --env-file <path> [--args-file <file>] [--cwd <dir>] [--read-only
//! <dir>]... [--writable <path>]... [--cow <dir> <store>]... [--cow-file
//! <file> <copy>]... -- <cmd...>`: a clean environment from a file, then exec.
//! Every ssh session into a box starts here, so a process sees exactly what
//! the host wrote for this launch and nothing from sshd, a shell profile or
//! the box itself. With `--args-file`, the command takes the file's arguments
//! after its own, and the file is removed. With `--read-only`, the command
//! sees each such directory read-only, whoever owns it, bar each `--writable`
//! path inside one, and each `--cow` directory or `--cow-file` file inside
//! one as a private copy it may write ([`crate::confine`]).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{confine, split_command, Result};

pub fn run(args: &[String]) -> Result<()> {
    let (opts, cmd) = split_command(args)?;
    let opts = parse_options(opts)?;
    let env_file = &opts.env_file;
    let text =
        fs::read_to_string(env_file).map_err(|e| format!("exec: cannot read {env_file}: {e}"))?;
    let env = parse_env(&text).map_err(|e| format!("exec: {env_file}: {e}"))?;
    let extra = match &opts.args_file {
        Some(file) => take_args(file)?,
        None => Vec::new(),
    };

    if !opts.view.is_empty() {
        // After the env and args files are read: confinement hides `/data`,
        // where they lie.
        let view = &opts.view;
        let confined = confine::check_paths(view).and_then(|()| confine::confine(view));
        if let Err(err) = confined {
            eprintln!("fletch-guest: exec: {err}");
            std::process::exit(confine::CONFINE_FAILED);
        }
    }

    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]).args(&extra).env_clear().envs(env);
    if let Some(cwd) = &opts.cwd {
        command.current_dir(cwd);
    }
    // exec only returns on failure. Shell exit codes so the host can tell a
    // missing binary (127) from anything else (126).
    let err = command.exec();
    eprintln!("fletch-guest: exec {}: {err}", cmd[0]);
    std::process::exit(if err.kind() == io::ErrorKind::NotFound {
        127
    } else {
        126
    });
}

/// What precedes `--`.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    env_file: String,
    args_file: Option<PathBuf>,
    cwd: Option<String>,
    /// Every `--read-only`, `--writable`, `--cow` and `--cow-file`, in order;
    /// absolute. How they lie in each other is checked against the
    /// filesystem ([`confine::check_paths`]).
    view: confine::View,
}

fn parse_options(opts: &[String]) -> std::result::Result<Options, String> {
    let (mut env_file, mut args_file, mut cwd) = (None, None, None);
    let (mut read_only, mut writable) = (Vec::new(), Vec::new());
    let (mut cow, mut cow_files) = (Vec::new(), Vec::new());
    let mut it = opts.iter();
    while let Some(opt) = it.next() {
        match opt.as_str() {
            "--env-file" => env_file = it.next(),
            "--args-file" => args_file = Some(absolute(opt, it.next())?),
            "--cwd" => cwd = it.next(),
            "--read-only" => read_only.push(absolute(opt, it.next())?),
            "--writable" => writable.push(absolute(opt, it.next())?),
            "--cow" => cow.push(absolute_pair(opt, "<dir> <store>", &mut it)?),
            "--cow-file" => cow_files.push(absolute_pair(opt, "<file> <copy>", &mut it)?),
            other => return Err(format!("exec: unknown option {other}")),
        }
    }
    Ok(Options {
        env_file: env_file.ok_or("exec: --env-file is required")?.clone(),
        args_file,
        cwd: cwd.cloned(),
        view: confine::View {
            read_only,
            writable,
            cow,
            cow_files,
        },
    })
}

/// The two absolute paths `option` takes, the next two of `args`; `what`
/// names them for the error when there are fewer.
fn absolute_pair<'a>(
    option: &str,
    what: &str,
    args: &mut impl Iterator<Item = &'a String>,
) -> std::result::Result<(PathBuf, PathBuf), String> {
    let (Some(first), Some(second)) = (args.next(), args.next()) else {
        return Err(format!("exec: {option} needs {what}"));
    };
    let first = absolute(option, Some(first))?;
    Ok((first, absolute(option, Some(second))?))
}

/// The absolute path `option` takes.
fn absolute(option: &str, value: Option<&String>) -> std::result::Result<PathBuf, String> {
    let path = value.ok_or_else(|| format!("exec: {option} needs a path"))?;
    if !Path::new(path).is_absolute() {
        return Err(format!("exec: {option} {path}: not an absolute path"));
    }
    Ok(PathBuf::from(path))
}

/// The arguments in `file`, which is then removed: one launch's, read once,
/// so a later launch can never run with them.
fn take_args(file: &Path) -> std::result::Result<Vec<OsString>, String> {
    let shown = file.display();
    let bytes = fs::read(file).map_err(|e| format!("exec: cannot read {shown}: {e}"))?;
    fs::remove_file(file).map_err(|e| format!("exec: cannot remove {shown}: {e}"))?;
    parse_args(&bytes).map_err(|e| format!("exec: {shown}: {e}"))
}

/// Arguments each ended by a NUL byte, the one byte no argument can hold; any
/// other bytes are kept as they are, UTF-8 or not. Empty is no arguments. A
/// last argument with no NUL after it is refused: the file was cut short.
fn parse_args(bytes: &[u8]) -> std::result::Result<Vec<OsString>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let body = bytes
        .strip_suffix(b"\0")
        .ok_or("the last argument has no NUL after it; the file is incomplete")?;
    Ok(body
        .split(|b| *b == 0)
        .map(|arg| OsStr::from_bytes(arg).to_os_string())
        .collect())
}

/// `KEY=VALUE` per line; values may contain `=`; blank lines and `#` comments
/// are skipped. A later line wins over an earlier one with the same key.
pub fn parse_env(text: &str) -> std::result::Result<Vec<(String, String)>, String> {
    let mut vars = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected KEY=VALUE", n + 1))?;
        if key.is_empty() || key.contains('\0') || value.contains('\0') {
            return Err(format!("line {}: invalid variable", n + 1));
        }
        vars.push((key.to_string(), value.to_string()));
    }
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use super::{parse_args, parse_env, parse_options, take_args, Options};
    use crate::confine::View;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn args_are_each_ended_by_a_nul_and_kept_verbatim() {
        assert_eq!(parse_args(b"").unwrap(), os(&[]));
        assert_eq!(parse_args(b"\0").unwrap(), os(&[""]));
        assert_eq!(parse_args(b"one\0").unwrap(), os(&["one"]));
        assert_eq!(
            parse_args(b"--append-system-prompt\0it's \"two\"\nlines $HOME\0\0last\0").unwrap(),
            os(&[
                "--append-system-prompt",
                "it's \"two\"\nlines $HOME",
                "",
                "last"
            ])
        );
        // Bytes that are not UTF-8 reach the command as they are.
        assert_eq!(
            parse_args(b"\xff\xfe\0").unwrap(),
            vec![OsString::from_vec(vec![0xff, 0xfe])]
        );
    }

    #[test]
    fn an_args_file_cut_short_is_refused() {
        for bytes in [&b"one"[..], b"one\0tw"] {
            let err = parse_args(bytes).unwrap_err();
            assert!(err.contains("incomplete"), "{err}");
        }
    }

    #[test]
    fn an_args_file_is_read_once_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("1.42.args");
        std::fs::write(&file, b"a\0b c\0").unwrap();
        assert_eq!(take_args(&file).unwrap(), os(&["a", "b c"]));
        assert!(!file.exists(), "the file is removed once read");
        let err = take_args(&file).unwrap_err();
        assert!(err.contains("cannot read"), "{err}");

        // One cut short is removed all the same: it is never read again.
        std::fs::write(&file, b"a\0b").unwrap();
        assert!(take_args(&file).unwrap_err().contains("incomplete"));
        assert!(!file.exists());
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn options_take_repeated_read_only_and_writable_paths() {
        let mut args = words("--env-file /e --read-only /a --writable /a/own --cwd /a/ws");
        args.extend(words("--read-only /b --writable /a/.state.json"));
        args.extend(words("--cow /a/.cursor /a/own/s"));
        args.extend(words("--cow-file /a/.claude.json /a/own/c"));
        args.extend(words("--args-file /data/run/1.42.args"));
        let opts = parse_options(&args).unwrap();
        let pair = |a: &str, b: &str| (PathBuf::from(a), PathBuf::from(b));
        assert_eq!(
            opts,
            Options {
                env_file: "/e".into(),
                args_file: Some("/data/run/1.42.args".into()),
                cwd: Some("/a/ws".into()),
                view: View {
                    read_only: vec![PathBuf::from("/a"), PathBuf::from("/b")],
                    writable: vec![PathBuf::from("/a/own"), PathBuf::from("/a/.state.json")],
                    cow: vec![pair("/a/.cursor", "/a/own/s")],
                    cow_files: vec![pair("/a/.claude.json", "/a/own/c")],
                },
            }
        );
        let plain = parse_options(&words("--env-file /dev/null")).unwrap();
        assert!(plain.view.is_empty() && plain.cwd.is_none() && plain.args_file.is_none());
    }

    #[test]
    fn options_refuse_what_they_cannot_honour() {
        for (args, why) in [
            ("--env-file /e --read-only", "--read-only needs a path"),
            ("--env-file /e --read-only repo", "not an absolute path"),
            ("--env-file /e --writable", "--writable needs a path"),
            ("--env-file /e --writable own", "not an absolute path"),
            ("--env-file /e --cow /a", "--cow needs <dir> <store>"),
            ("--env-file /e --cow", "--cow needs <dir> <store>"),
            ("--env-file /e --cow /a store", "not an absolute path"),
            ("--env-file /e --cow a /store", "not an absolute path"),
            (
                "--env-file /e --cow-file /a",
                "--cow-file needs <file> <copy>",
            ),
            ("--env-file /e --cow-file /a copy", "not an absolute path"),
            ("--env-file /e --args-file", "--args-file needs a path"),
            (
                "--env-file /e --args-file run/1.args",
                "not an absolute path",
            ),
            ("--env-file /e --rw /x", "unknown option --rw"),
            ("--read-only /x", "--env-file is required"),
        ] {
            let err = parse_options(&words(args)).unwrap_err();
            assert!(err.contains(why), "{args}: {err}");
        }
    }

    #[test]
    fn parses_values_with_equals_and_skips_noise() {
        let text = "# comment\n\nHOME=/Users/alex\r\nHTTPS_PROXY=http://127.0.0.1:8899\nTOKEN=a=b==c\n   \n  # indented comment\nEMPTY=\n";
        let vars = parse_env(text).unwrap();
        assert_eq!(
            vars,
            vec![
                ("HOME".into(), "/Users/alex".into()),
                ("HTTPS_PROXY".into(), "http://127.0.0.1:8899".into()),
                ("TOKEN".into(), "a=b==c".into()),
                ("EMPTY".into(), String::new()),
            ]
        );
    }

    #[test]
    fn rejects_lines_without_equals_or_key() {
        assert!(parse_env("PATH\n").unwrap_err().contains("line 1"));
        assert!(parse_env("A=1\n=x\n").unwrap_err().contains("line 2"));
    }

    #[test]
    fn keeps_values_verbatim() {
        let vars = parse_env("X= spaced \"quoted\" \\n\n").unwrap();
        assert_eq!(vars[0].1, " spaced \"quoted\" \\n");
    }
}
