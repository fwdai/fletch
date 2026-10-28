//! `exec --env-file <path> [--cwd <dir>] -- <cmd...>`: a clean environment from
//! a file, then exec. Every ssh session into a box starts here, so a process
//! sees exactly what the host wrote for this launch and nothing from sshd, a
//! shell profile or the box itself.

use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::{split_command, Result};

pub fn run(args: &[String]) -> Result<()> {
    let (opts, cmd) = split_command(args)?;
    let (mut env_file, mut cwd) = (None, None);
    let mut it = opts.iter();
    while let Some(opt) = it.next() {
        match opt.as_str() {
            "--env-file" => env_file = it.next(),
            "--cwd" => cwd = it.next(),
            other => return Err(format!("exec: unknown option {other}").into()),
        }
    }
    let env_file = env_file.ok_or("exec: --env-file is required")?;
    let text =
        fs::read_to_string(env_file).map_err(|e| format!("exec: cannot read {env_file}: {e}"))?;
    let env = parse_env(&text).map_err(|e| format!("exec: {env_file}: {e}"))?;

    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]).env_clear().envs(env);
    if let Some(cwd) = cwd {
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
    use super::parse_env;

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
