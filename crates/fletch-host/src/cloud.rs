//! Cloud providers, from a terminal: which are connected, and connecting or
//! disconnecting one.
//!
//! A host that cannot boot a box gives a new workspace a machine at a
//! connected provider (`sandbox::default_workspace_type`), so an operator with
//! no window needs what the desktop's Settings › Cloud pane gives: the same
//! three engine calls (`cloud::statuses`, `commands::cloud_connect_impl`,
//! `commands::cloud_disconnect_impl`: spares made after a connect, destroyed
//! before a disconnect), over the admin socket (`ops`). Every word about a
//! provider — its id, its name, where
//! to get a token, what it bills — is the host's answer, so nothing here names
//! one.
//!
//! The API token is read from this process's stdin: typed at a prompt that
//! does not echo it when stdin is a terminal, read as one line when it is
//! piped. Never from an argument or the environment, where `ps`, a shell's
//! history or a service manager would keep it, and never printed.

use std::io::{BufRead, IsTerminal, Write};
use std::os::fd::AsFd;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::admin;

/// One provider, as the `cloud_status` and `cloud_connect` ops answer it
/// (the engine's `cloud::CloudProviderStatus`).
#[derive(Debug, Clone, Deserialize)]
struct Row {
    id: String,
    label: String,
    connected: bool,
    account: Option<String>,
    account_label: String,
    token_help: String,
    billing: String,
}

/// `fletch-host cloud status`: one line per provider this host knows.
pub async fn print_status(data_dir: &Path) -> Result<(), String> {
    for row in fetch_rows(data_dir).await? {
        println!("{}", status_line(&row));
    }
    Ok(())
}

/// `fletch-host cloud connect <provider>`: read a token from stdin and have
/// the host validate it with the provider and store it.
pub async fn connect(data_dir: &Path, provider: &str, account: Option<&str>) -> Result<(), String> {
    // Asked first, so a provider the host does not know is refused before
    // anyone pastes a token, and the prompt can say where to get one.
    let rows = fetch_rows(data_dir).await?;
    let row = rows.iter().find(|row| row.id == provider).ok_or_else(|| {
        let known: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        format!(
            "unknown cloud provider {provider:?}: expected one of {}",
            known.join(", ")
        )
    })?;
    let token = read_token(row)?;
    let answer = admin::call(
        data_dir,
        "cloud_connect",
        json!({ "provider": provider, "token": token, "account": account }),
    )
    .await?;
    let row: Row = serde_json::from_value(answer)
        .map_err(|e| format!("the host's answer did not parse: {e}"))?;
    println!("{}", connected_line(&row));
    println!("{}", row.billing);
    Ok(())
}

/// `fletch-host cloud disconnect <provider>`.
pub async fn disconnect(data_dir: &Path, provider: &str) -> Result<(), String> {
    admin::call(
        data_dir,
        "cloud_disconnect",
        json!({ "provider": provider }),
    )
    .await?;
    println!("disconnected {provider}");
    Ok(())
}

async fn fetch_rows(data_dir: &Path) -> Result<Vec<Row>, String> {
    let answer = admin::call(data_dir, "cloud_status", json!({})).await?;
    serde_json::from_value(answer)
        .map_err(|e| format!("the host's cloud provider list did not parse: {e}"))
}

/// A provider's line in `cloud status`: connected, in which account, or the
/// command that connects it.
fn status_line(row: &Row) -> String {
    if row.connected {
        format!("{}  {}  {}", row.id, row.label, account_cell(row))
    } else {
        format!(
            "{}  {}  not connected  fletch-host cloud connect {}",
            row.id, row.label, row.id
        )
    }
}

/// What `cloud connect` prints once the host has stored the connection.
fn connected_line(row: &Row) -> String {
    format!("connected {} ({})", row.id, account_cell(row))
}

/// The account a connection acts in, under the provider's name for it.
fn account_cell(row: &Row) -> String {
    format!(
        "{}: {}",
        row.account_label,
        row.account.as_deref().unwrap_or_default()
    )
}

/// The token for `row`'s provider, from stdin: at a prompt that does not echo
/// it when stdin is a terminal, else the first line piped in.
fn read_token(row: &Row) -> Result<String, String> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return first_line(stdin.lock());
    }
    // The prompt goes to stderr, so a script reading stdout sees only the
    // outcome.
    eprintln!("{}", row.token_help);
    eprint!("{} API token (hidden as you type): ", row.label);
    let _ = std::io::stderr().flush();
    without_echo(|| first_line(stdin.lock()))?
}

/// The first line of `input`, without its line ending or the whitespace
/// around it; an error, which never holds what was read, when it is blank.
fn first_line(mut input: impl BufRead) -> Result<String, String> {
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|e| format!("cannot read the token from stdin: {e}"))?;
    let token = line.trim();
    if token.is_empty() {
        return Err("no token was given: type or paste it at the prompt, or pipe it in".into());
    }
    Ok(token.to_string())
}

/// Run `read` with the terminal on stdin not echoing what is typed, and put
/// its settings back afterwards. The newline that ends the line still echoes
/// (`ECHONL`), so the cursor leaves the prompt as it would after any input.
fn without_echo<T>(read: impl FnOnce() -> T) -> Result<T, String> {
    use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};

    let stdin = std::io::stdin();
    let saved = tcgetattr(stdin.as_fd())
        .map_err(|e| format!("cannot read the terminal's settings: {e}"))?;
    let mut quiet = saved.clone();
    quiet.local_flags.remove(LocalFlags::ECHO);
    quiet.local_flags.insert(LocalFlags::ECHONL);
    tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &quiet)
        .map_err(|e| format!("cannot turn off the terminal's echo: {e}"))?;
    let read = read();
    if let Err(e) = tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &saved) {
        eprintln!("cannot restore the terminal's echo ({e}); run `stty echo`");
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(connected: bool) -> Row {
        Row {
            id: "acme".into(),
            label: "Acme Cloud".into(),
            connected,
            account: connected.then(|| "team".into()),
            account_label: "Organization".into(),
            token_help: "Create a token in the dashboard.".into(),
            billing: "Acme bills by the minute.".into(),
        }
    }

    /// A connected provider says which account it acts in; one that is not
    /// says the command that connects it.
    #[test]
    fn every_provider_line_says_where_it_stands() {
        assert_eq!(
            status_line(&row(true)),
            "acme  Acme Cloud  Organization: team"
        );
        assert_eq!(
            status_line(&row(false)),
            "acme  Acme Cloud  not connected  fletch-host cloud connect acme"
        );
        assert_eq!(
            connected_line(&row(true)),
            "connected acme (Organization: team)"
        );
    }

    /// A row reads the engine's status as the host serializes it, so a field
    /// renamed on one side fails here rather than as a blank column.
    #[test]
    fn a_row_reads_the_engines_status() {
        let status = fletch_core::cloud::CloudProviderStatus {
            id: "acme",
            label: "Acme Cloud",
            connected: true,
            account: Some("team".into()),
            account_label: "Organization",
            account_placeholder: "personal",
            token_placeholder: "acme_…",
            token_help: "Create a token in the dashboard.",
            billing: "Acme bills by the minute.",
        };
        let read: Row = serde_json::from_value(serde_json::to_value(&status).unwrap()).unwrap();
        assert_eq!(status_line(&read), status_line(&row(true)));
        assert_eq!(read.token_help, status.token_help);
        assert_eq!(read.billing, status.billing);
    }

    /// One line, trimmed, whatever follows it; a blank one is refused.
    #[test]
    fn the_token_is_the_first_line_of_stdin() {
        assert_eq!(
            first_line("  tok-123 \nsecond line\n".as_bytes()),
            Ok("tok-123".to_string())
        );
        assert_eq!(first_line("tok-456".as_bytes()), Ok("tok-456".to_string()));
        for blank in ["", "\n", "   \n"] {
            assert!(first_line(blank.as_bytes()).is_err(), "{blank:?}");
        }
    }
}
