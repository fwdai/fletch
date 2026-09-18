# fletch-host

Fletch's engine with no window. `fletch-host serve` runs agents on a machine
you are not sitting at — a Mac mini in a closet, a cloud box — and the phone (or
another Fletch desktop) drives them over the LAN or through a relay. It is the
same engine the desktop app runs, booted by the same function
(`fletch_core::host::boot`); what it does not have is a webview, a tray, a
microphone or a window.

Unix only, and this first release targets **macOS**. It builds and runs on Linux
by construction — nothing here is macOS-specific — but Linux packaging (a
systemd unit, Docker UID mapping, prebuilt binaries in the release job) lands in
the next change. Until then, on Linux you build it yourself and you need Docker
or Podman, because there is no `sandbox-exec` to fall back to and the host
refuses to start rather than run an agent outside the boundary it promised.

## Install

No published binary yet. From a checkout:

```sh
cargo build --release --manifest-path crates/fletch-host/Cargo.toml
cp src-tauri/target/release/fletch-host ~/.local/bin/   # or anywhere on PATH
```

(The target directory is shared with the desktop build; `--target-dir` moves it.)

## Run it

```sh
fletch-host serve
```

Options, all of which apply to that run only and are never written to the
database:

| Flag | Default |
| --- | --- |
| `--data-dir PATH` | `~/Library/Application Support/fletch-host`, or `$XDG_DATA_HOME/fletch-host` |
| `--port N` | the stored `remote.port`, else `47285` |
| `--relay URL` / `--no-relay` | the stored `remote.relay_url` |
| `--name NAME` | the machine's name |

Remote access is always on: serving paired devices is the only reason the
process exists, so the desktop's `remote.enabled` setting is not consulted.
Logs go to stderr at `info`; `RUST_LOG` overrides that (`RUST_LOG=debug`,
`RUST_LOG=info,fletch_core::remote=debug`). Run it under whatever supervises
services on the machine — `launchd`, systemd, `tmux` — and let that capture
stderr.

`SIGINT` and `SIGTERM` kill the agents' processes and exit 0, so a restart is
never mistaken for a crash.

## Pair a phone

With the host running, on the host:

```sh
fletch-host pair
```

That prints a `fletch://pair?…` link, the 8-character code inside it, and when
both expire (5 minutes). On the phone, add a host and paste the link. The link
carries this host's public key, which the phone pins — the code only authorizes
one pairing.

> The terminal QR code is not in this release; see "Known gaps" below.

Then:

```sh
fletch-host status                 # what the host is doing, as JSON
fletch-host devices list
fletch-host devices revoke <id>    # also hangs up on the device immediately
```

Everything except `serve` talks to the running host over its admin socket, and
takes the same `--data-dir` to find it. With no host running they print
`fletch-host is not running; start it with 'fletch-host serve'` and exit 1.

## Approvals

If publish confirmation is on (`publish_confirmation`), an agent that wants to
push or open a PR asks first. On the desktop that is a dialog. Here it is:

```sh
fletch-host approvals list
fletch-host approve <id>           # or: fletch-host approve <id> --deny
```

An unanswered question is refused when `publish_approval_wait` elapses (120s by
default; `0` waits forever), so a host nobody is watching never publishes
without being asked. Today this CLI is the only way to answer one: the op that
lets a paired phone do it is Phase 0 of the multi-host plan and is not on the
wire yet, so on a host with a short wait, answer promptly or set
`publish_approval_wait` to `0`.

## GitHub

```sh
QUORUM_GITHUB_CLIENT_ID=<your oauth app's client id> fletch-host serve
fletch-host github login
```

`github login` prints a code and a URL; enter the code in a browser on any
machine, and the command waits until it is authorized. The token is stored in
the host's database (see below) and is what git pushes and the GitHub API use
from then on. The client id has to come from the environment because unlike the
desktop — which bakes its own in at build time — a host may well be signing in
as a different OAuth app; the app must have the device flow enabled.

## Projects

```sh
fletch-host project add /srv/code/my-repo
fletch-host project clone owner/repo --into /srv/code
```

Same code path as the desktop's New Project flow: a folder that is not a git
repo yet is initialized, and every connected client is told the project list
changed.

## Log in to the provider CLIs first, over SSH

Fletch runs `claude`, `codex`, `gemini` and friends; their credentials are
theirs, not Fletch's, and several of them sign in by opening a browser. So
before the first agent runs, do it interactively on the host:

```sh
ssh you@host
claude login        # and whichever others you use
```

Those logins persist in each tool's own config, so this is once per host (per
provider), not once per run.

## The data dir, and who may read it

Everything the host knows lives in one directory: the SQLite database, the
paired devices (`remote/devices.json`), this host's Noise private key
(`remote/host_key`), the admin socket (`host.sock`) and — unlike the desktop on
release macOS — **the GitHub token in plaintext**, in the database's `settings`
table.

That is deliberate. The desktop keeps secrets in the login keychain, which works
because a signed app reads its own items silently. A service started by launchd
or over SSH has no unlocked login keychain, so the same code would fail every
read (or block on a dialog nobody can see). The host's protection is the
filesystem instead:

- the data dir is created `0700` and re-restricted to `0700` on every start;
- the admin socket is `0600`, and that permission **is** its authentication —
  it takes no credential, because anyone who can open it can already read the
  database beside it.

So: **run the host as its own user, and let nothing else read that directory.**
Anyone who can read it can push as you.

Deliberately not the desktop's data directory. A Mac running both must not have
two engines on one SQLite database — each would sweep the other's live agents as
orphans.

## Known gaps

- **No terminal QR code** for the pairing link yet (the `qrcode` crate is not
  vendored in this change); paste the link or type the code instead.
- **Linux packaging** — systemd unit, Docker UID mapping, release artifacts —
  is the next change.
- **No PTY streaming**: a remote client sees agent status and events, not a live
  terminal. That is Phase 5 of the multi-host plan.
