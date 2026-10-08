//! Presence-only macOS Keychain lookups.
//!
//! [`item_present`] answers "does an item exist for this service?" by running
//! `security find-generic-password` **without** `-w`. Omitting `-w` is the whole
//! point: `security` then reports metadata and an exit status only, so the
//! password is never read, never returned, and macOS never raises the "… wants
//! to use your confidential information stored in your keychain" prompt. That
//! matters because the providers settings pane polls the auth probe every few
//! seconds while it is open.
//!
//! A caller that genuinely needs the secret asks for it with [`read_password`]:
//! only `agent::claude_oauth`, once per agent launch or explicit user action
//! (an account's limits Refresh). Nothing on a polling path may do that;
//! [`item_stamp`] is how a polling path notices that an item changed.
//!
//! Off macOS there is no equally cheap check, so the answer is always `false`;
//! callers treat that as "no Keychain login to find here", not "signed out".

/// Whether a generic-password item exists for `service`, optionally narrowed to
/// `account`. Exit status 0 means the item exists; a missing item, a locked
/// keychain, or an unavailable `security` binary all read as `false`.
///
/// Never passes `-w`, so no secret is read and no Keychain prompt appears.
#[cfg(target_os = "macos")]
pub(crate) fn item_present(service: &str, account: Option<&str>) -> bool {
    let mut command = std::process::Command::new("security");
    command.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        command.args(["-a", account]);
    }
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn item_present(_service: &str, _account: Option<&str>) -> bool {
    false
}

/// The `acct` and `mdat` attributes of the item for `service`, as `security`
/// prints them without `-w`: whose item it is, and a value that changes
/// whenever its password is rewritten. `None` when there is no item. Reads no
/// secret, so it never prompts and may run on a polling path.
#[cfg(target_os = "macos")]
pub(crate) fn item_stamp(service: &str) -> Option<ItemStamp> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_item_stamp(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn item_stamp(_service: &str) -> Option<ItemStamp> {
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemStamp {
    pub account: Option<String>,
    pub modified: String,
}

#[cfg(any(target_os = "macos", test))]
fn parse_item_stamp(attributes: &str) -> Option<ItemStamp> {
    let value = |name: &str| {
        let prefix = format!("\"{name}\"<");
        attributes.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(prefix.as_str())?;
            let (_, value) = rest.split_once(">=")?;
            Some(value.trim().to_string())
        })
    };
    let account =
        value("acct").and_then(|v| Some(v.strip_prefix('"')?.strip_suffix('"')?.to_string()));
    Some(ItemStamp {
        account,
        modified: value("mdat")?,
    })
}

/// How long a `security` call that may raise a Keychain prompt gets before it
/// is abandoned: long enough for a person to answer an unlock or access
/// prompt, short enough that a dialog nobody sees can't hold a launch forever.
#[cfg(target_os = "macos")]
const PROMPTABLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Run `command` (stdout piped, stderr discarded), feeding `input` to its
/// stdin, and give up — killing it — after `timeout`.
#[cfg(target_os = "macos")]
fn run_bounded(
    mut command: std::process::Command,
    input: Option<&[u8]>,
    timeout: std::time::Duration,
) -> std::result::Result<std::process::Output, String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run `security`: {e}"))?;
    if let Some(input) = input {
        // Dropping stdin after the input ends `security -i`'s session.
        let mut stdin = child.stdin.take().ok_or("`security` has no stdin")?;
        stdin
            .write_all(input)
            .map_err(|e| format!("could not talk to `security`: {e}"))?;
    }
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the Keychain didn't answer in time (an unanswered prompt?)".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(e) => return Err(format!("`security` failed: {e}")),
        }
    };
    let mut stdout = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout);
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

/// The password of the item for `service`. Reading it can raise the macOS
/// "wants to use your confidential information" prompt, so only a launch or
/// an explicit user action may call this, never a polling path; and it gives
/// up after [`PROMPTABLE_TIMEOUT`].
#[cfg(target_os = "macos")]
pub(crate) fn read_password(service: &str) -> Option<String> {
    let mut command = std::process::Command::new("security");
    command.args(["find-generic-password", "-s", service, "-w"]);
    let out = run_bounded(command, None, PROMPTABLE_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim_end_matches('\n');
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read_password(_service: &str) -> Option<String> {
    None
}

/// The longest command line `security -i` takes whole, newline included.
/// Measured on macOS 27: a longer line is cut at this length and the
/// truncated password stored anyway, so a write that doesn't fit must not be
/// attempted at all.
const SECURITY_LINE_MAX: usize = 4096;

fn write_command(service: &str, account: &str, password: &str) -> String {
    let hex: String = password.bytes().map(|b| format!("{b:02x}")).collect();
    format!("add-generic-password -U -a \"{account}\" -s \"{service}\" -X {hex}\n")
}

/// Whether [`write_password`] can store a password of `len` bytes for
/// `service` under `account`.
pub(crate) fn password_fits(service: &str, account: &str, len: usize) -> bool {
    write_command(service, account, "").len() + 2 * len <= SECURITY_LINE_MAX
}

/// Create or update (`-U`) the item for `service` under `account`. Written
/// through `security`, like claude's own writes, so the item keeps the access
/// list claude gave it and later reads by either side stay prompt-free (the
/// in-process Keychain API is a different client and gets prompted). The
/// command goes over `security -i`'s stdin with the password hex-encoded
/// (`-X`): a `-w` argument would put the secret in argv, where any local
/// process can read it. A password too long for one `security -i` line is
/// refused rather than stored truncated, and failures carry fixed text only:
/// `security` echoes the command, secret included, in its own errors.
#[cfg(target_os = "macos")]
pub(crate) fn write_password(
    service: &str,
    account: &str,
    password: &str,
) -> std::result::Result<(), String> {
    if [service, account]
        .iter()
        .any(|v| v.contains(['"', '\\', '\n']))
    {
        return Err("the Keychain item's name can't be passed to `security`".into());
    }
    if !password_fits(service, account, password.len()) {
        return Err("the login is too large for `security` to store whole".into());
    }
    let mut command = std::process::Command::new("security");
    command.arg("-i");
    let input = write_command(service, account, password);
    let out = run_bounded(command, Some(input.as_bytes()), PROMPTABLE_TIMEOUT)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`security` couldn't write the Keychain item (exit {})",
            out.status.code().unwrap_or(-1)
        ))
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn write_password(
    _service: &str,
    _account: &str,
    _password: &str,
) -> std::result::Result<(), String> {
    Err("there is no Keychain on this platform".into())
}

/// `security`'s exit status for `errSecItemNotFound`: the one failure of a
/// delete that means "nothing to delete" rather than "could not delete".
#[cfg(target_os = "macos")]
const ITEM_NOT_FOUND: i32 = 44;

/// Delete the generic-password item for `service`. `Ok` when no item remains
/// afterwards — deleted, or absent to begin with — and `Err` with the reason
/// otherwise: a locked keychain, a denied request, or no `security` binary. The
/// two are kept apart on purpose: a caller about to discard the item's owner
/// must not read "could not check" as "already gone". Deleting reads no
/// secret, so it raises no Keychain prompt. Only for logins Fletch owns — a
/// managed provider account's — never the CLI's own default item.
#[cfg(target_os = "macos")]
pub(crate) fn delete_item(service: &str) -> std::result::Result<(), String> {
    let output = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", service])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("could not run `security`: {e}"))?;
    if output.status.success() || output.status.code() == Some(ITEM_NOT_FOUND) {
        return Ok(());
    }
    // `security` prints a one-line reason ("User interaction is not allowed",
    // "The specified keychain could not be found"), never the item's contents.
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if reason.is_empty() {
        format!("`security` exited with {}", output.status)
    } else {
        reason
    })
}

/// Off macOS nothing is in a keychain to delete.
#[cfg(not(target_os = "macos"))]
pub(crate) fn delete_item(_service: &str) -> std::result::Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Keychain *hit* isn't unit-testable — it would need an item
    /// planted in the developer's login keychain — so only the miss is covered.
    /// A service nobody has registered exits non-zero and, because no `-w` is
    /// passed, does so without touching or prompting for any secret.
    #[test]
    fn absent_item_reads_as_missing() {
        assert!(!item_present(
            "fletch-keychain-probe-service-that-does-not-exist",
            None
        ));
        assert!(!item_present(
            "fletch-keychain-probe-service-that-does-not-exist",
            Some("nobody")
        ));
    }

    /// Deleting an item nobody registered is a success, not a failure: the
    /// caller's question is "does one remain", and none does. (`security` exits
    /// 44 for it, which is the one code the delete must not report.)
    #[test]
    fn deleting_an_absent_item_is_not_an_error() {
        assert_eq!(
            delete_item("fletch-keychain-probe-service-that-does-not-exist"),
            Ok(())
        );
    }

    #[test]
    fn item_stamp_reads_the_account_and_modification_date() {
        let attributes = r#"keychain: "/Users/x/Library/Keychains/login.keychain-db"
attributes:
    "acct"<blob>="alex"
    "mdat"<timedate>=0x32303236313030383036313333325A00  "20261008061332Z\000"
    "svce"<blob>="Claude Code-credentials"
"#;
        let stamp = parse_item_stamp(attributes).unwrap();
        assert_eq!(stamp.account.as_deref(), Some("alex"));
        assert!(stamp.modified.contains("20261008061332Z"), "{stamp:?}");
    }

    #[test]
    fn a_password_fits_only_while_its_whole_line_does() {
        let (svc, acct) = ("Claude Code-credentials-e6d7ed77", "alex");
        let fixed = write_command(svc, acct, "").len();
        let largest = (SECURITY_LINE_MAX - fixed) / 2;
        assert!(password_fits(svc, acct, largest));
        assert!(!password_fits(svc, acct, largest + 1));
        assert!(!password_fits(svc, acct, 4500));
        assert_eq!(
            write_command(svc, acct, &"x".repeat(largest)).len(),
            fixed + 2 * largest
        );
    }

    /// A login over 4 KB is refused outright, with fixed text, and the item
    /// already stored is left exactly as it was — never replaced by a cut-off
    /// copy. Ignored like the round trip below.
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn an_oversized_password_is_refused_and_the_item_kept() {
        let service = format!("fletch-keychain-big-{}", std::process::id());
        let user = std::env::var("USER").unwrap();
        write_password(&service, &user, "{\"small\":true}").unwrap();
        let big = format!("{{\"blob\":\"{}\"}}", "s".repeat(4500));
        let err = write_password(&service, &user, &big).unwrap_err();
        assert!(!err.contains("sss"), "{err}");
        assert_eq!(read_password(&service).as_deref(), Some("{\"small\":true}"));
        delete_item(&service).unwrap();
    }

    #[test]
    fn item_stamp_needs_a_modification_date() {
        assert_eq!(parse_item_stamp("    \"acct\"<blob>=\"alex\"\n"), None);
    }

    /// Writes, rewrites, reads back and removes a throwaway item in the login
    /// keychain. Ignored because it touches the developer's real keychain; on
    /// a Mac run `cargo test --lib keychain_round_trip -- --ignored`.
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn keychain_round_trip_through_security() {
        let service = format!("fletch-keychain-test-{}", std::process::id());
        let user = std::env::var("USER").unwrap();
        write_password(&service, &user, "{\"a\":1}").unwrap();
        let first = item_stamp(&service).expect("item written");
        assert_eq!(first.account.as_deref(), Some(user.as_str()));
        assert_eq!(read_password(&service).as_deref(), Some("{\"a\":1}"));
        // `mdat` has one-second resolution.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_password(&service, &user, "{\"a\":2}").unwrap();
        assert_eq!(read_password(&service).as_deref(), Some("{\"a\":2}"));
        assert_ne!(item_stamp(&service).unwrap().modified, first.modified);
        delete_item(&service).unwrap();
        assert!(item_stamp(&service).is_none());
    }
}
