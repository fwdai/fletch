//! `fletch-host`: run the Fletch engine with no window, and manage it from a
//! terminal.
//!
//! `serve` is the host. Every other subcommand is a client of the socket that
//! `serve` opened (`admin`), which is why they all take the same `--data-dir`:
//! that is how they find it.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use fletch_core::host::HeadlessRelay;
use fletch_host::{admin, serve};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(
    name = "fletch-host",
    version,
    about = "Run Fletch's engine headless and control it from a terminal",
    long_about = "Runs the Fletch engine with no window. Agents spawn, run and are \
                  controlled here; a paired phone or desktop drives them over the LAN \
                  or through a relay. Every subcommand but `serve` talks to a running \
                  host over its admin socket."
)]
struct Cli {
    /// Where the host keeps its database, host key and paired devices.
    /// Defaults to the platform data dir (`~/Library/Application
    /// Support/fletch-host`, `$XDG_DATA_HOME/fletch-host`) — never the
    /// desktop's, so both can run on one machine.
    #[arg(long, global = true, value_name = "PATH")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the engine until SIGINT or SIGTERM.
    Serve {
        /// TCP port for paired devices. Defaults to the stored `remote.port`,
        /// or 47285. Not persisted: it applies to this run only.
        #[arg(long)]
        port: Option<u16>,
        /// Relay to reach this host from off the LAN, e.g.
        /// wss://relay.fletch.sh. Defaults to the stored `remote.relay_url`.
        #[arg(long, value_name = "URL", conflicts_with = "no_relay")]
        relay: Option<String>,
        /// Do not dial any relay this run, whatever is stored.
        #[arg(long)]
        no_relay: bool,
        /// The name paired devices show for this host. Defaults to the
        /// machine's name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Print what the host is doing, as JSON.
    Status,
    /// Mint a pairing link for a phone or a desktop.
    Pair,
    /// Paired devices.
    Devices {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Publish approvals waiting for an answer.
    Approvals {
        #[command(subcommand)]
        command: ApprovalCommand,
    },
    /// Answer a waiting publish approval.
    Approve {
        /// The approval's id, from `approvals list`.
        id: String,
        /// Refuse it instead of allowing it.
        #[arg(long)]
        deny: bool,
    },
    /// GitHub sign-in (the device flow: a code you enter on another machine).
    Github {
        #[command(subcommand)]
        command: GithubCommand,
    },
    /// Projects this host can run agents in.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
}

#[derive(Subcommand)]
enum DeviceCommand {
    /// List every paired device.
    List,
    /// Revoke a device's credential and hang up on it.
    Revoke {
        /// The device's id, from `devices list`.
        id: String,
    },
}

#[derive(Subcommand)]
enum ApprovalCommand {
    /// List every approval still waiting.
    List,
}

#[derive(Subcommand)]
enum GithubCommand {
    /// Sign in to GitHub and wait for the code to be entered.
    Login,
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Pin an existing folder as a project (it is initialized as a git repo if
    /// it is not one yet).
    Add {
        /// Path to the folder, on this host.
        path: PathBuf,
    },
    /// Clone a GitHub repo and pin it.
    Clone {
        /// `owner/repo`, or any URL `git clone` understands.
        spec: String,
        /// Where to put the clone. Defaults to the current directory.
        #[arg(long, value_name = "DIR")]
        into: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(serve::default_data_dir);

    // One runtime for both halves: the engine's background tasks belong to it
    // (`serve`), and the client subcommands use it for the socket.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => fail(&format!("cannot start a tokio runtime: {e}")),
    };

    let result = match cli.command {
        Command::Serve {
            port,
            relay,
            no_relay,
            name,
        } => {
            init_logging();
            runtime.block_on(serve::run(serve::Config {
                data_dir,
                port,
                relay: match (relay, no_relay) {
                    (Some(url), _) => HeadlessRelay::Url(url),
                    (None, true) => HeadlessRelay::Off,
                    (None, false) => HeadlessRelay::Stored,
                },
                name,
                handle_signals: true,
            }))
        }
        command => runtime.block_on(client(&data_dir, command)),
    };
    if let Err(e) = result {
        fail(&e);
    }
}

/// Everything that is not `serve`: one or two socket calls and some printing.
async fn client(data_dir: &std::path::Path, command: Command) -> Result<(), String> {
    match command {
        // Handled by the caller; listed so this match stays exhaustive.
        Command::Serve { .. } => unreachable!("serve does not go over the socket"),
        Command::Status => {
            print_json(&admin::call(data_dir, "status", json!({})).await?);
        }
        Command::Pair => {
            let invite = admin::call(data_dir, "begin_pairing", json!({})).await?;
            println!("{}", string(&invite, "url"));
            println!();
            println!("code:    {}", string(&invite, "token"));
            println!("expires: {}", string(&invite, "expiresAt"));
            println!();
            println!(
                "Open Fletch on the phone, choose \"Pair with a host\" and paste the link \
                 above (or type the code)."
            );
        }
        Command::Devices { command } => match command {
            DeviceCommand::List => {
                let devices = admin::call(data_dir, "devices_list", json!({})).await?;
                let devices = devices.as_array().cloned().unwrap_or_default();
                if devices.is_empty() {
                    println!("no paired devices; run `fletch-host pair` to add one");
                }
                for device in devices {
                    println!(
                        "{}  {} ({})  {}  last seen {}",
                        string(&device, "deviceId"),
                        string(&device, "name"),
                        string(&device, "platform"),
                        if device["connected"] == json!(true) {
                            "connected"
                        } else {
                            "offline"
                        },
                        match device["lastSeenAt"].as_str() {
                            Some(at) => at.to_string(),
                            None => "never".to_string(),
                        }
                    );
                }
            }
            DeviceCommand::Revoke { id } => {
                admin::call(data_dir, "revoke_device", json!({ "deviceId": id })).await?;
                println!("revoked {id}");
            }
        },
        Command::Approvals { command } => match command {
            ApprovalCommand::List => {
                let pending = admin::call(data_dir, "approvals_list", json!({})).await?;
                let pending = pending.as_array().cloned().unwrap_or_default();
                if pending.is_empty() {
                    println!("nothing is waiting for approval");
                }
                for approval in pending {
                    println!(
                        "{}  {}  {}  {}  asked {}",
                        string(&approval, "id"),
                        string(&approval, "agent_id"),
                        string(&approval, "op"),
                        string(&approval, "detail"),
                        string(&approval, "requested_at"),
                    );
                }
            }
        },
        Command::Approve { id, deny } => {
            admin::call(
                data_dir,
                "answer_publish_approval",
                json!({ "id": id, "approved": !deny }),
            )
            .await?;
            println!("{} {id}", if deny { "denied" } else { "approved" });
        }
        Command::Github { command } => match command {
            GithubCommand::Login => github_login(data_dir).await?,
        },
        Command::Project { command } => match command {
            ProjectCommand::Add { path } => {
                let path = absolute(&path)?;
                let project = admin::call(data_dir, "project_add", json!({ "path": path })).await?;
                print_json(&project);
            }
            ProjectCommand::Clone { spec, into } => {
                let parent = absolute(&into.unwrap_or_else(|| PathBuf::from(".")))?;
                let project = admin::call(
                    data_dir,
                    "project_clone",
                    json!({ "spec": spec, "destParent": parent }),
                )
                .await?;
                print_json(&project);
            }
        },
    }
    Ok(())
}

/// How often `github login` asks the host whether the code has been entered.
/// The host is already polling GitHub on the provider's own interval; this only
/// decides how quickly the terminal notices.
const LOGIN_POLL: Duration = Duration::from_secs(2);

async fn github_login(data_dir: &std::path::Path) -> Result<(), String> {
    let started = admin::call(data_dir, "github_login_start", json!({})).await?;
    let expires_in = started["expiresIn"].as_u64().unwrap_or(900);
    println!(
        "Open {} and enter this code: {}",
        string(&started, "verificationUrl"),
        string(&started, "userCode")
    );
    println!("Waiting for you to authorize it (the code is good for {expires_in}s)...");

    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);
    loop {
        tokio::time::sleep(LOGIN_POLL).await;
        let status = admin::call(data_dir, "github_login_status", json!({})).await?;
        match status["state"].as_str().unwrap_or("") {
            "complete" => {
                let who = status["name"]
                    .as_str()
                    .or_else(|| status["email"].as_str())
                    .unwrap_or("your GitHub account");
                println!("signed in as {who}");
                return Ok(());
            }
            "failed" => return Err(string(&status, "error")),
            _ => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err("the code expired before it was authorized; run the command again".into());
        }
    }
}

/// Paths are the *host's*, and a host may well have a different working
/// directory than the shell that is talking to it — so send an absolute one and
/// let a relative path mean "relative to where I typed this".
fn absolute(path: &std::path::Path) -> Result<String, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?
            .join(path)
    };
    Ok(path.to_string_lossy().to_string())
}

fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => println!("{e}"),
    }
}

/// A string field of an answer, or a blank. A missing field is the host and the
/// CLI disagreeing about a version, which is worth printing an empty column
/// over rather than refusing to show the rest.
fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// Log to stderr, so an init system's journal (and a terminal) get it without
/// any configuration. `RUST_LOG` overrides the default.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}
