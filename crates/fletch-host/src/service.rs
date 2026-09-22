//! `fletch-host service install|uninstall`: the host as an init-system service.
//!
//! Two platforms, one shape. On Linux a systemd unit — a *user* unit by
//! default, `--system` for `/etc/systemd/system` with a `User=`; on macOS a
//! launchd user agent. Both are rendered from the templates in `/packaging`,
//! which this module compiles in with [`include_str!`] so that the file a
//! reader installs by hand and the file the CLI writes are the same file.
//!
//! What varies between an install and a hand-written unit is only the few
//! tokens each template documents: the absolute path of this executable, the
//! `serve` arguments, and (systemd) the `User=` line and the install target.
//! Rendering is a string substitution plus one guard — a rendered unit that
//! still contains `{{` is a template that grew a token the CLI does not know
//! about, and that is an error rather than a broken unit written to disk.
//!
//! Every path and command is computed portably, so both renderers and both
//! path sets can be unit-tested on either platform; only the dispatch in
//! [`install`] and [`uninstall`] looks at the host OS, through `cfg!` rather
//! than `#[cfg]` so that neither half can rot unnoticed.

use std::path::{Path, PathBuf};
use std::process::Command;

use nix::unistd::{Uid, User};

/// The systemd unit's file name, and the name `systemctl` takes.
pub const UNIT_NAME: &str = "fletch-host.service";

/// The launchd agent's label, and (with `.plist`) its file name.
pub const LAUNCHD_LABEL: &str = "com.fletch.host";

/// The templates, from `/packaging`. `include_str!` is what makes those files
/// the single source: there is no second copy to keep in step, and the copies
/// the release tarball ships are the same bytes with their placeholders still
/// in them (each template's header says what to substitute, for whoever
/// installs without the CLI).
pub const SYSTEMD_TEMPLATE: &str = include_str!("../../../packaging/fletch-host.service");
pub const LAUNCHD_TEMPLATE: &str = include_str!("../../../packaging/com.fletch.host.plist");

/// Everything a rendered unit needs. `data_dir` is always written into the
/// unit, resolved: an init system's environment is not the operator's shell,
/// and a service that picks a *different* default data dir than the CLI
/// subcommands is a host nobody can pair.
#[derive(Debug, Clone)]
pub struct Spec {
    /// Absolute path of the binary the service runs.
    pub exec: PathBuf,
    /// Absolute path of the data dir to serve from.
    pub data_dir: PathBuf,
    pub port: Option<u16>,
    pub name: Option<String>,
    /// `User=` for a system unit. Ignored by launchd (an agent is already the
    /// logged-in user's) and by a systemd user unit.
    pub user: Option<String>,
    /// Linux only: write to `/etc/systemd/system` instead of the user unit dir.
    pub system: bool,
    /// launchd only: where stdout and stderr go.
    pub log_path: PathBuf,
}

impl Spec {
    /// The command line after the binary: `serve`, then its flags.
    pub fn serve_args(&self) -> Vec<String> {
        let mut args = vec!["serve".to_string()];
        args.push("--data-dir".to_string());
        args.push(self.data_dir.to_string_lossy().to_string());
        if let Some(port) = self.port {
            args.push("--port".to_string());
            args.push(port.to_string());
        }
        if let Some(name) = &self.name {
            args.push("--name".to_string());
            args.push(name.clone());
        }
        args
    }
}

/// Where the rendered unit goes.
pub fn systemd_unit_path(spec: &Spec) -> Result<PathBuf, String> {
    if spec.system {
        return Ok(PathBuf::from("/etc/systemd/system").join(UNIT_NAME));
    }
    Ok(systemd_user_dir()?.join(UNIT_NAME))
}

/// `$XDG_CONFIG_HOME/systemd/user`, or `~/.config/systemd/user`. Resolved from
/// the environment rather than through `dirs`, whose `config_dir` means
/// `~/Library/Application Support` on macOS — this path is systemd's, so it is
/// the XDG one on every OS the test suite runs on.
pub fn systemd_user_dir() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home()?.join(".config"),
    };
    Ok(base.join("systemd").join("user"))
}

/// The same, for another user's home — whose `$XDG_CONFIG_HOME` this process
/// cannot see, so it is the XDG default.
pub fn systemd_user_dir_in(home: &Path) -> PathBuf {
    home.join(".config").join("systemd").join("user")
}

pub fn launchd_plist_path() -> Result<PathBuf, String> {
    Ok(launchd_plist_path_in(&home()?))
}

pub fn launchd_plist_path_in(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

pub fn launchd_log_path() -> Result<PathBuf, String> {
    Ok(home()?.join("Library").join("Logs").join("fletch-host.log"))
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "cannot find your home directory".to_string())
}

/// The user a `--system` unit runs as: the explicit `--user`, else whoever
/// invoked `sudo`. A system unit is installed as root, so "whoever runs this
/// command" is the one answer that must never be given — the host runs as the
/// user whose agents these are, never as root (the README's invariant; the
/// container UID mapping and every file it writes assume it).
pub fn system_user(flag: Option<String>, sudo_user: Option<String>) -> Result<String, String> {
    let spec = flag.or(sudo_user).ok_or_else(|| {
        "`--system` needs the user the host runs as: pass `--user NAME`, or run this \
         through sudo (which says who you are)."
            .to_string()
    })?;
    let user = lookup_user(&spec)?;
    if user.uid.is_root() {
        return Err(
            "the host does not run as root: pass `--user NAME` for the account whose agents \
             these are."
                .to_string(),
        );
    }
    Ok(user.name)
}

/// A user from passwd, by name or by numeric uid — systemd reads `User=0` as
/// root exactly like `User=root`, so a check on the *entry* is the only one
/// that holds, and the name it returns is the one written into the unit.
pub fn lookup_user(spec: &str) -> Result<User, String> {
    let lookup_err = |e| format!("cannot look up user {spec}: {e}");
    if let Some(user) = User::from_name(spec).map_err(lookup_err)? {
        return Ok(user);
    }
    if let Ok(uid) = spec.parse::<u32>() {
        if let Some(user) = User::from_uid(Uid::from_raw(uid)).map_err(lookup_err)? {
            return Ok(user);
        }
    }
    Err(format!("no such user: {spec}"))
}

/// Whoever ran `sudo`, when this process is root because of it: the person the
/// host belongs to, whose data dir, unit and socket every subcommand but
/// `serve` should mean. `None` outside sudo, and when sudo was run by root.
pub fn sudoer() -> Option<User> {
    if !Uid::effective().is_root() {
        return None;
    }
    let name = std::env::var("SUDO_USER").ok()?;
    lookup_user(&name).ok().filter(|user| !user.uid.is_root())
}

/// The data dir the CLI would default to if `user` ran it: what
/// [`crate::serve::default_data_dir`] picks, under the platform's data root
/// in *their* home from passwd rather than this process's (under sudo, this
/// process's home is root's). Their `$XDG_DATA_HOME`, if they set one, is not
/// visible from here; `--data-dir` is the way to honour it.
pub fn default_data_dir_of(user: &str) -> Result<PathBuf, String> {
    Ok(crate::serve::data_dir_under(platform_data_root(
        &lookup_user(user)?.dir,
    )))
}

/// `dirs::data_dir()`'s answer for a home this process does not live in.
fn platform_data_root(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("share")
    }
}

/// Render the systemd unit.
pub fn render_systemd(spec: &Spec) -> Result<String, String> {
    // `ExecStart=` already says `serve`; the template's `{{SERVE_ARGS}}` is
    // what follows it, leading space included, quoted the way systemd parses.
    let args: Vec<String> = spec
        .serve_args()
        .into_iter()
        .skip(1)
        .map(|arg| format!(" {}", systemd_quote(&arg)))
        .collect();
    let user = match (&spec.user, spec.system) {
        // A user unit runs as its owner by definition; `User=` there is a unit
        // systemd refuses to start.
        (Some(user), true) => format!("User={user}"),
        _ => String::new(),
    };
    let wanted_by = if spec.system {
        "multi-user.target"
    } else {
        "default.target"
    };
    finish(
        SYSTEMD_TEMPLATE
            .replace("{{EXEC}}", &systemd_quote(&spec.exec.to_string_lossy()))
            .replace("{{SERVE_ARGS}}", &args.concat())
            .replace("{{USER}}", &user)
            .replace("{{WANTED_BY}}", wanted_by),
    )
}

/// Render the launchd agent.
pub fn render_launchd(spec: &Spec) -> Result<String, String> {
    let args: Vec<String> = spec
        .serve_args()
        .iter()
        .map(|arg| format!("        <string>{}</string>", xml_escape(arg)))
        .collect();
    finish(
        LAUNCHD_TEMPLATE
            .replace("{{EXEC}}", &xml_escape(&spec.exec.to_string_lossy()))
            .replace("{{ARG_STRINGS}}", &args.join("\n"))
            .replace(
                "{{LOG_PATH}}",
                &xml_escape(&spec.log_path.to_string_lossy()),
            ),
    )
}

/// The guard: a rendered unit with a placeholder left in it never reaches
/// disk. Only a template edit can trigger this, which is exactly when it is
/// worth catching.
fn finish(rendered: String) -> Result<String, String> {
    match rendered.find("{{") {
        Some(at) => {
            let rest: String = rendered[at..].chars().take(40).collect();
            Err(format!(
                "the packaging template has a placeholder this build does not know how to fill: \
                 {rest}"
            ))
        }
        None => Ok(rendered),
    }
}

/// One `ExecStart=` word. systemd splits on whitespace and understands double
/// quotes, so anything with a space in it (a data dir under "Application
/// Support", say) has to be quoted.
/// Every word of a systemd unit's `ExecStart=` — the executable, then the
/// arguments — unquoted the way [`systemd_quote`] quoted them (or as a
/// hand-written unit would: bare words, or ones in double quotes with `\"` and
/// `\\`).
pub fn systemd_unit_words(unit: &str) -> Option<Vec<String>> {
    let line = unit
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("ExecStart="))?
        .trim_start();
    // systemd's `ExecStart=@path`, `-path`, `+path` etc. prefixes name the
    // same file; strip them so a hand-edited unit still matches.
    let mut rest = line.trim_start_matches(['@', '-', '+', '!', ':']);
    let mut words = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Some(words);
        }
        if let Some(quoted) = rest.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = quoted.chars();
            loop {
                match chars.next()? {
                    '\\' => out.push(chars.next()?),
                    '"' => break,
                    c => out.push(c),
                }
            }
            words.push(out);
            rest = chars.as_str();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            words.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
}

/// The executable a systemd unit runs: the first word of its `ExecStart=`.
/// What `update` matches against the binary it replaced.
pub fn systemd_unit_exec(unit: &str) -> Option<PathBuf> {
    first_word(systemd_unit_words(unit)?)
}

/// Every `<string>` of a launchd plist's `ProgramArguments` array — the
/// executable, then the arguments — XML-unescaped the way [`render_launchd`]
/// escaped them. Bounded at `</array>`: the `<string>`s after it are the log
/// paths and the environment, not the command line.
pub fn launchd_plist_words(plist: &str) -> Option<Vec<String>> {
    let after_key = plist.split("<key>ProgramArguments</key>").nth(1)?;
    let array = after_key.split("</array>").next().unwrap_or(after_key);
    Some(
        array
            .split("<string>")
            .skip(1)
            .filter_map(|chunk| chunk.split("</string>").next())
            .map(|word| xml_unescape(word.trim()))
            .collect(),
    )
}

/// The executable a launchd plist runs: the first `<string>` of its
/// `ProgramArguments`.
pub fn launchd_plist_exec(plist: &str) -> Option<PathBuf> {
    first_word(launchd_plist_words(plist)?)
}

fn first_word(words: Vec<String>) -> Option<PathBuf> {
    let word = words.into_iter().next()?;
    (!word.is_empty()).then(|| PathBuf::from(word))
}

/// One installed service definition: the file, and the command line in it.
#[derive(Debug, Clone)]
pub struct Installed {
    pub unit: PathBuf,
    pub exec: PathBuf,
    /// Everything after the executable — the `serve` arguments the unit was
    /// installed with.
    pub args: Vec<String>,
}

/// Reads a unit's command line out of its text.
type WordsOf = fn(&str) -> Option<Vec<String>>;

/// The service definition installed on this machine, if there is one: the
/// systemd system unit, then the systemd user unit, then the launchd agent —
/// the order `update::restart` probes in. Read-only, and it runs nothing.
pub fn find_installed() -> Option<Installed> {
    first_installed(candidate_units())
}

fn candidate_units() -> Vec<(PathBuf, WordsOf)> {
    let mut candidates: Vec<(PathBuf, WordsOf)> = vec![(
        Path::new("/etc/systemd/system").join(UNIT_NAME),
        systemd_unit_words as WordsOf,
    )];
    if let Ok(dir) = systemd_user_dir() {
        candidates.push((dir.join(UNIT_NAME), systemd_unit_words));
    }
    if let Ok(plist) = launchd_plist_path() {
        candidates.push((plist, launchd_plist_words));
    }
    candidates
}

fn first_installed(candidates: Vec<(PathBuf, WordsOf)>) -> Option<Installed> {
    candidates
        .into_iter()
        .find_map(|(path, words_of)| read_installed(&path, words_of))
}

fn read_installed(unit: &Path, words_of: WordsOf) -> Option<Installed> {
    let text = std::fs::read_to_string(unit).ok()?;
    let mut words = words_of(&text)?.into_iter();
    let exec = words.next().filter(|word| !word.is_empty())?;
    Some(Installed {
        unit: unit.to_path_buf(),
        exec: PathBuf::from(exec),
        args: words.collect(),
    })
}

/// `service show`: the installed unit as `key=value` lines a shell can read —
/// what it is, what it runs, and with which arguments. `default_args` is what
/// a flag-less [`install`] on this machine would write instead, which is what
/// lets a caller tell a rewrite that changes nothing from one that would reset
/// an operator's `--data-dir`, `--port` or `--name`.
///
/// No installed unit is an error, not an empty success: a caller tests the
/// exit status, and every line it reads is then about a unit that exists.
pub fn show() -> Result<(), String> {
    let Some(installed) = find_installed() else {
        return Err("no fletch-host service is installed".to_string());
    };
    let defaults = Spec {
        exec: PathBuf::new(),
        data_dir: crate::serve::default_data_dir(),
        port: None,
        name: None,
        user: None,
        system: false,
        log_path: PathBuf::new(),
    };
    println!("unit={}", installed.unit.display());
    println!("exec={}", installed.exec.display());
    println!("args={}", installed.args.join(" "));
    println!("default_args={}", defaults.serve_args().join(" "));
    Ok(())
}

fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn systemd_quote(word: &str) -> String {
    if word
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\' || c == '\'')
    {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        word.to_string()
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Write the unit and start it. Idempotent: an existing unit is overwritten
/// and the service restarted, so `install` twice with different flags leaves
/// the second one's flags running.
pub fn install(spec: &Spec) -> Result<(), String> {
    if cfg!(target_os = "linux") {
        install_systemd(spec)
    } else if cfg!(target_os = "macos") {
        if spec.system {
            return Err(system_is_systemds());
        }
        install_launchd(spec)
    } else {
        Err(unsupported())
    }
}

pub fn uninstall(system: bool) -> Result<(), String> {
    if cfg!(target_os = "linux") {
        uninstall_systemd(system)
    } else if cfg!(target_os = "macos") {
        if system {
            return Err(system_is_systemds());
        }
        uninstall_launchd()
    } else {
        Err(unsupported())
    }
}

/// A launchd *daemon* (`/Library/LaunchDaemons`, root, no login session) is a
/// different thing from the agent this installs, and not one the host wants:
/// the provider CLIs it drives are logged in as a user.
fn system_is_systemds() -> String {
    "`--system` is systemd's; on macOS the host installs as a launchd user agent \
     (~/Library/LaunchAgents). Drop the flag."
        .to_string()
}

fn unsupported() -> String {
    "`service install` knows systemd (Linux) and launchd (macOS); \
     on anything else, run `fletch-host serve` under your own supervisor"
        .to_string()
}

fn install_systemd(spec: &Spec) -> Result<(), String> {
    let unit = render_systemd(spec)?;
    let path = systemd_unit_path(spec)?;
    write_unit(&path, &unit)?;

    let scope: &[&str] = if spec.system { &[] } else { &["--user"] };
    systemctl(scope, &["daemon-reload"])?;
    systemctl(scope, &["enable", UNIT_NAME])?;
    // `restart` rather than `start`: it starts a stopped service and reloads a
    // running one, which is what makes a second `install` idempotent.
    systemctl(scope, &["restart", UNIT_NAME])?;

    println!();
    if spec.system {
        println!("Status: systemctl status {UNIT_NAME}");
        println!("Logs:   journalctl -u fletch-host -f");
    } else {
        println!(
            "A systemd user unit stops when you log out. To keep the host serving, enable \
             lingering once (it needs sudo, so this command does not run it for you):"
        );
        println!();
        println!(
            "    loginctl enable-linger {}",
            spec.user
                .clone()
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "$USER".to_string())
        );
        println!();
        println!("Status: systemctl --user status {UNIT_NAME}");
        println!("Logs:   journalctl --user -u fletch-host -f");
    }
    Ok(())
}

fn uninstall_systemd(system: bool) -> Result<(), String> {
    let scope: &[&str] = if system { &[] } else { &["--user"] };
    let path = systemd_unit_path(&uninstall_spec(system))?;
    if !path.exists() {
        return Err(format!("no unit at {}", path.display()));
    }
    // Best effort, in this order: a unit that is already stopped, or disabled,
    // must not stop the removal of the file.
    systemctl_ok(scope, &["disable", "--now", UNIT_NAME]);
    remove(&path)?;
    systemctl(scope, &["daemon-reload"])?;
    Ok(())
}

fn install_launchd(spec: &Spec) -> Result<(), String> {
    let plist = render_launchd(spec)?;
    let path = launchd_plist_path()?;
    if let Some(parent) = spec.log_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    // An agent already loaded from the old plist would keep running the old
    // command line, so unload before writing rather than after.
    let target = launchd_target()?;
    launchctl_ok(&["bootout", &format!("{target}/{LAUNCHD_LABEL}")]);
    write_unit(&path, &plist)?;

    let plist_arg = path.to_string_lossy().to_string();
    match launchctl(&["bootstrap", &target, &plist_arg]) {
        Ok(()) => {}
        // `bootstrap` is the modern verb; `load -w` is the one that exists on
        // macOS before 10.11 and still works after it.
        Err(e) => {
            println!("(bootstrap failed: {e}; falling back to `launchctl load -w`)");
            launchctl(&["load", "-w", &plist_arg])?;
        }
    }
    println!();
    println!("Status: launchctl print {target}/{LAUNCHD_LABEL}");
    println!("Logs:   tail -f {}", spec.log_path.display());
    println!(
        "A Mac that sleeps answers nothing: `sudo pmset -a sleep 0` if this host has to stay up."
    );
    Ok(())
}

fn uninstall_launchd() -> Result<(), String> {
    let path = launchd_plist_path()?;
    if !path.exists() {
        return Err(format!("no agent at {}", path.display()));
    }
    let target = launchd_target()?;
    if launchctl(&["bootout", &format!("{target}/{LAUNCHD_LABEL}")]).is_err() {
        launchctl_ok(&["unload", "-w", &path.to_string_lossy()]);
    }
    remove(&path)?;
    Ok(())
}

/// `gui/<uid>`: the per-user launchd domain, the one a user agent lives in.
fn launchd_target() -> Result<String, String> {
    Ok(format!("gui/{}", nix::unistd::getuid().as_raw()))
}

/// A spec with only the field `uninstall` needs. `uninstall` takes no flags
/// beyond `--system`, so there is nothing to render and nothing to resolve.
fn uninstall_spec(system: bool) -> Spec {
    Spec {
        exec: PathBuf::new(),
        data_dir: PathBuf::new(),
        port: None,
        name: None,
        user: None,
        system,
        log_path: PathBuf::new(),
    }
}

fn write_unit(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let existed = path.exists();
    std::fs::write(path, contents).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "cannot write {}: {e}\nA system unit is root's; re-run with sudo, or drop \
                 `--system` for a user unit under your home directory.",
                path.display()
            )
        } else {
            format!("cannot write {}: {e}", path.display())
        }
    })?;
    println!(
        "{} {}",
        if existed { "rewrote" } else { "wrote" },
        path.display()
    );
    Ok(())
}

fn remove(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
    println!("removed {}", path.display());
    Ok(())
}

fn systemctl(scope: &[&str], args: &[&str]) -> Result<(), String> {
    let all: Vec<&str> = scope.iter().chain(args.iter()).copied().collect();
    run("systemctl", &all)
}

fn systemctl_ok(scope: &[&str], args: &[&str]) {
    let all: Vec<&str> = scope.iter().chain(args.iter()).copied().collect();
    if let Err(e) = run("systemctl", &all) {
        println!("({e})");
    }
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    run("launchctl", args)
}

fn launchctl_ok(args: &[&str]) {
    let _ = run("launchctl", args);
}

/// Run one of the two init-system tools, printing the command first. Every
/// side effect this subcommand has is either a file it named or a line printed
/// here, which is the whole point: an operator can redo or undo it by hand.
pub fn run(program: &str, args: &[&str]) -> Result<(), String> {
    println!("$ {program} {}", args.join(" "));
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(format!(
        "{program} {} failed{}{}",
        args.join(" "),
        match out.status.code() {
            Some(code) => format!(" (exit {code})"),
            None => String::new(),
        },
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec {
            exec: PathBuf::from("/home/fletch/.local/bin/fletch-host"),
            data_dir: PathBuf::from("/home/fletch/.local/share/fletch-host"),
            port: Some(47285),
            name: Some("mac mini".to_string()),
            user: None,
            system: false,
            log_path: PathBuf::from("/home/fletch/Library/Logs/fletch-host.log"),
        }
    }

    #[test]
    fn serve_args_are_the_flags_the_cli_was_given() {
        assert_eq!(
            spec().serve_args(),
            vec![
                "serve",
                "--data-dir",
                "/home/fletch/.local/share/fletch-host",
                "--port",
                "47285",
                "--name",
                "mac mini",
            ]
        );
        let bare = Spec {
            port: None,
            name: None,
            ..spec()
        };
        // The data dir is never omitted: see `Spec`.
        assert_eq!(
            bare.serve_args(),
            vec![
                "serve",
                "--data-dir",
                "/home/fletch/.local/share/fletch-host"
            ]
        );
    }

    #[test]
    fn user_unit_renders_every_placeholder() {
        let unit = render_systemd(&spec()).unwrap();
        assert!(!unit.contains("{{"), "{unit}");
        assert_eq!(
            unit.lines()
                .find(|line| line.starts_with("ExecStart="))
                .unwrap(),
            "ExecStart=/home/fletch/.local/bin/fletch-host serve --data-dir \
             /home/fletch/.local/share/fletch-host --port 47285 --name \"mac mini\""
        );
        assert!(unit.contains("WantedBy=default.target"));
        // A user unit that says `User=` is one systemd refuses to start.
        assert!(!unit.contains("\nUser="));
    }

    #[test]
    fn system_unit_names_a_user_and_the_multi_user_target() {
        let unit = render_systemd(&Spec {
            system: true,
            user: Some("fletch".to_string()),
            ..spec()
        })
        .unwrap();
        assert!(!unit.contains("{{"), "{unit}");
        assert!(unit.contains("\nUser=fletch\n"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn launchd_agent_renders_every_placeholder() {
        let plist = render_launchd(&Spec {
            exec: PathBuf::from("/Users/fletch/.local/bin/fletch-host"),
            data_dir: PathBuf::from("/Users/fletch/Library/Application Support/fletch-host"),
            ..spec()
        })
        .unwrap();
        assert!(!plist.contains("{{"), "{plist}");
        assert!(plist.contains("<string>/Users/fletch/.local/bin/fletch-host</string>"));
        assert!(plist.contains("        <string>serve</string>\n"));
        // A space in the path is a plist problem, not a quoting one.
        assert!(plist
            .contains("<string>/Users/fletch/Library/Application Support/fletch-host</string>"));
        assert!(plist.contains("<string>--port</string>"));
        assert!(plist.contains("<string>47285</string>"));
        assert!(plist.contains("<string>/home/fletch/Library/Logs/fletch-host.log</string>"));
        assert!(plist.contains("<key>Label</key>"));
    }

    #[test]
    fn a_placeholder_the_cli_does_not_know_is_an_error() {
        let e = finish("ExecStart={{WHAT_IS_THIS}} serve\n".to_string()).unwrap_err();
        assert!(e.contains("{{WHAT_IS_THIS}}"), "{e}");
    }

    #[test]
    fn systemd_words_are_quoted_only_when_they_have_to_be() {
        assert_eq!(
            systemd_quote("/usr/bin/fletch-host"),
            "/usr/bin/fletch-host"
        );
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn systemd_paths_are_xdg_on_every_os() {
        // Whatever `dirs` would say on macOS, this is systemd's own path.
        let dir = systemd_user_dir().unwrap();
        assert!(dir.ends_with("systemd/user"), "{}", dir.display());
        assert_eq!(
            systemd_unit_path(&Spec {
                system: true,
                ..spec()
            })
            .unwrap(),
            PathBuf::from("/etc/systemd/system/fletch-host.service")
        );
    }

    /// `sudo fletch-host service install --system` runs as root; the unit must
    /// still run as the person, so root is never derived and never accepted.
    #[test]
    fn a_system_unit_runs_as_the_sudoer_or_the_named_user_never_root() {
        // Names are resolved through passwd, so the one user every machine
        // running this suite has is the one running it.
        let me = User::from_uid(nix::unistd::getuid())
            .unwrap()
            .expect("the current user has a passwd entry")
            .name;
        assert_eq!(
            system_user(Some(me.clone()), Some("root".into())).unwrap(),
            me,
            "an explicit --user wins over the sudoer"
        );
        assert_eq!(system_user(None, Some(me.clone())).unwrap(), me);
        let e = system_user(None, None).unwrap_err();
        assert!(e.contains("--user"), "{e}");
        let e = system_user(Some("root".into()), None).unwrap_err();
        assert!(e.contains("root"), "{e}");
        let e = system_user(None, Some("root".into())).unwrap_err();
        assert!(e.contains("root"), "{e}");
        // `User=0` is root to systemd; a string compare would let it through.
        let e = system_user(Some("0".into()), None).unwrap_err();
        assert!(e.contains("root"), "{e}");
        let e = system_user(None, Some("no-such-user-fletch-test".into())).unwrap_err();
        assert!(e.contains("no such user"), "{e}");
    }

    /// A numeric uid resolves to its passwd entry, so the unit carries the
    /// name and the root check sees the uid.
    #[test]
    fn users_resolve_by_name_or_uid_to_the_same_entry() {
        let me = User::from_uid(nix::unistd::getuid())
            .unwrap()
            .expect("the current user has a passwd entry");
        assert_eq!(lookup_user(&me.name).unwrap().uid, me.uid);
        assert_eq!(
            lookup_user(&me.uid.as_raw().to_string()).unwrap().name,
            me.name
        );
        assert!(lookup_user("0").unwrap().uid.is_root());
        assert_eq!(
            system_user(Some(me.uid.as_raw().to_string()), None).unwrap(),
            me.name
        );
    }

    /// The service user's default data dir comes from *their* passwd home, by
    /// the same rule `default_data_dir` applies to this process's.
    #[test]
    fn a_users_default_data_dir_comes_from_their_passwd_home_not_this_process() {
        let me = User::from_uid(nix::unistd::getuid())
            .unwrap()
            .expect("the current user has a passwd entry");
        assert_eq!(
            default_data_dir_of(&me.name).unwrap(),
            crate::serve::data_dir_under(platform_data_root(&me.dir))
        );
        // Same root `dirs` would pick for this process, when the homes agree.
        if dirs::home_dir().as_deref() == Some(me.dir.as_path()) {
            assert_eq!(
                default_data_dir_of(&me.name).unwrap(),
                crate::serve::default_data_dir()
            );
        }
        let e = default_data_dir_of("no-such-user-fletch-test").unwrap_err();
        assert!(e.contains("no such user"), "{e}");
    }
}

/// The executable a rendered unit names round-trips through the parser
/// `update` uses to decide which unit to restart — including the paths the
/// renderers have to quote or escape.
#[cfg(test)]
mod exec_tests {
    use super::*;

    fn spec_with_exec(exec: &str) -> Spec {
        Spec {
            exec: PathBuf::from(exec),
            data_dir: PathBuf::from("/srv/fletch-host"),
            port: None,
            name: None,
            user: None,
            system: false,
            log_path: PathBuf::from("/var/log/fletch-host.log"),
        }
    }

    #[test]
    fn a_rendered_systemd_unit_names_its_executable_back() {
        for exec in [
            "/usr/local/bin/fletch-host",
            "/opt/my tools/fletch-host",
            "/odd/\"quoted\"/fletch-host",
        ] {
            let unit = render_systemd(&spec_with_exec(exec)).unwrap();
            assert_eq!(
                systemd_unit_exec(&unit).as_deref(),
                Some(Path::new(exec)),
                "{exec}\n{unit}"
            );
        }
        // Hand-written variants systemd accepts.
        assert_eq!(
            systemd_unit_exec("[Service]\nExecStart=-/usr/bin/fletch-host serve\n").as_deref(),
            Some(Path::new("/usr/bin/fletch-host"))
        );
        assert_eq!(systemd_unit_exec("[Service]\nType=simple\n"), None);
    }

    #[test]
    fn a_rendered_launchd_plist_names_its_executable_back() {
        for exec in [
            "/Users/me/.local/bin/fletch-host",
            "/Users/me/Tools & Co/fletch-host",
            "/Users/me/<odd>/fletch-host",
        ] {
            let plist = render_launchd(&spec_with_exec(exec)).unwrap();
            assert_eq!(
                launchd_plist_exec(&plist).as_deref(),
                Some(Path::new(exec)),
                "{exec}\n{plist}"
            );
        }
        assert_eq!(launchd_plist_exec("<plist><dict></dict></plist>"), None);
    }

    /// What `service show` reports: the first unit that exists, in probing
    /// order, with the whole command line split back out of it.
    #[test]
    fn the_lookup_reports_the_first_unit_that_exists_and_its_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let exec = "/opt/my tools/fletch-host";
        let unit = dir.path().join(UNIT_NAME);
        let plist = dir.path().join("com.fletch.host.plist");
        let missing = dir.path().join("missing.service");

        let mut spec = spec_with_exec(exec);
        spec.port = Some(4242);
        std::fs::write(&unit, render_systemd(&spec).unwrap()).unwrap();
        std::fs::write(&plist, render_launchd(&spec_with_exec(exec)).unwrap()).unwrap();

        // Nothing installed at any candidate path is "nothing installed".
        assert!(first_installed(vec![(missing.clone(), systemd_unit_words)]).is_none());

        // The systemd unit wins over the plist when both are there, and its
        // arguments come back whole — quoted paths unquoted, flags kept.
        let found = first_installed(vec![
            (missing.clone(), systemd_unit_words),
            (unit.clone(), systemd_unit_words),
            (plist.clone(), launchd_plist_words),
        ])
        .unwrap();
        assert_eq!(found.unit, unit);
        assert_eq!(found.exec, PathBuf::from(exec));
        assert_eq!(
            found.args,
            ["serve", "--data-dir", "/srv/fletch-host", "--port", "4242"]
        );

        // With the systemd unit gone, the plist answers — and its
        // `ProgramArguments` stop at `</array>`, so the log paths below it are
        // not arguments.
        let found = first_installed(vec![
            (missing, systemd_unit_words),
            (plist.clone(), launchd_plist_words),
        ])
        .unwrap();
        assert_eq!(found.unit, plist);
        assert_eq!(found.exec, PathBuf::from(exec));
        assert_eq!(found.args, ["serve", "--data-dir", "/srv/fletch-host"]);
    }
}
