//! `exec --env-file <path> [--cwd <dir>] [--read-only <dir>]... [--writable
//! <path>]... -- <cmd...>`: a clean environment from a file, then exec. Every
//! ssh session into a box starts here, so a process sees exactly what the host
//! wrote for this launch and nothing from sshd, a shell profile or the box
//! itself. With `--read-only`, the command sees each such directory read-only,
//! whoever owns it, bar each `--writable` path inside one ([`crate::confine`]).

use std::fs;
use std::io;
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

    if !opts.read_only.is_empty() || !opts.writable.is_empty() {
        // After the env file is read: confinement hides `/data`, where it lies.
        let confined = confine::check_paths(&opts.read_only, &opts.writable)
            .and_then(|()| confine::confine(&opts.read_only, &opts.writable));
        if let Err(err) = confined {
            eprintln!("fletch-guest: exec: {err}");
            std::process::exit(confine::CONFINE_FAILED);
        }
    }

    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]).env_clear().envs(env);
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
    cwd: Option<String>,
    /// Every `--read-only`, in order; absolute.
    read_only: Vec<PathBuf>,
    /// Every `--writable`, in order; absolute. Whether each lies inside a
    /// `--read-only` directory is checked against the filesystem
    /// ([`confine::check_paths`]).
    writable: Vec<PathBuf>,
}

fn parse_options(opts: &[String]) -> std::result::Result<Options, String> {
    let (mut env_file, mut cwd) = (None, None);
    let (mut read_only, mut writable) = (Vec::new(), Vec::new());
    let mut it = opts.iter();
    while let Some(opt) = it.next() {
        match opt.as_str() {
            "--env-file" => env_file = it.next(),
            "--cwd" => cwd = it.next(),
            "--read-only" => read_only.push(absolute(opt, it.next())?),
            "--writable" => writable.push(absolute(opt, it.next())?),
            other => return Err(format!("exec: unknown option {other}")),
        }
    }
    Ok(Options {
        env_file: env_file.ok_or("exec: --env-file is required")?.clone(),
        cwd: cwd.cloned(),
        read_only,
        writable,
    })
}

/// The absolute path `option` takes.
fn absolute(option: &str, value: Option<&String>) -> std::result::Result<PathBuf, String> {
    let path = value.ok_or_else(|| format!("exec: {option} needs a path"))?;
    if !Path::new(path).is_absolute() {
        return Err(format!("exec: {option} {path}: not an absolute path"));
    }
    Ok(PathBuf::from(path))
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
    use std::path::PathBuf;

    use super::{parse_env, parse_options, Options};

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn options_take_repeated_read_only_and_writable_paths() {
        let mut args = words("--env-file /e --read-only /a --writable /a/own --cwd /a/ws");
        args.extend(words("--read-only /b --writable /a/.state.json"));
        let opts = parse_options(&args).unwrap();
        assert_eq!(
            opts,
            Options {
                env_file: "/e".into(),
                cwd: Some("/a/ws".into()),
                read_only: vec![PathBuf::from("/a"), PathBuf::from("/b")],
                writable: vec![PathBuf::from("/a/own"), PathBuf::from("/a/.state.json")],
            }
        );
        let plain = parse_options(&words("--env-file /dev/null")).unwrap();
        assert!(plain.read_only.is_empty() && plain.cwd.is_none());
        assert!(plain.writable.is_empty());
    }

    #[test]
    fn options_refuse_what_they_cannot_honour() {
        for (args, why) in [
            ("--env-file /e --read-only", "--read-only needs a path"),
            ("--env-file /e --read-only repo", "not an absolute path"),
            ("--env-file /e --writable", "--writable needs a path"),
            ("--env-file /e --writable own", "not an absolute path"),
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
