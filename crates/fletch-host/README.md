# fletch-host

Fletch's engine with no window. `fletch-host serve` runs agents on a machine
you are not sitting at — a Mac mini in a closet, a cloud box — and the phone (or
another Fletch desktop) drives them over the LAN or through a relay. It is the
same engine the desktop app runs, booted by the same function
(`fletch_core::host::boot`); what it does not have is a webview, a tray, a
microphone or a window.

Unix only: **Linux** (x86-64 and arm64) and **macOS** (Apple silicon). On Linux
you need Docker or Podman — there is no `sandbox-exec` to fall back to, and the
host refuses to start rather than run an agent outside the boundary it promised.

## Install

Each release publishes `fletch-host-<version>-<target>.tar.gz` and a matching
`.sha256`:

```sh
V=<the release version>                       # e.g. 0.7.32, without the leading v
T=x86_64-unknown-linux-gnu                    # or aarch64-unknown-linux-gnu, aarch64-apple-darwin
curl -fsSLO https://github.com/fwdai/fletch/releases/download/v$V/fletch-host-$V-$T.tar.gz
curl -fsSLO https://github.com/fwdai/fletch/releases/download/v$V/fletch-host-$V-$T.tar.gz.sha256
sha256sum -c fletch-host-$V-$T.tar.gz.sha256   # shasum -a 256 -c on macOS
tar xzf fletch-host-$V-$T.tar.gz
install -Dm755 fletch-host ~/.local/bin/fletch-host
```

`~/.local/bin` is where the service units below expect it. Make sure it is on
your `PATH` (`export PATH="$HOME/.local/bin:$PATH"`). The tarball also carries
this README and both service units, so the paths in "Run it as a service" work
from the extracted directory as well as from a checkout.

From a checkout instead:

```sh
cargo build --release --manifest-path crates/fletch-host/Cargo.toml
install -Dm755 crates/fletch-host/target/release/fletch-host ~/.local/bin/fletch-host
```

(This package is not in a Cargo workspace, so it builds into its own target
directory; `--target-dir` or `$CARGO_TARGET_DIR` shares one with the desktop.
`--no-default-features` drops the pairing QR, the only dependency outside the
desktop's own tree — for a registry you cannot reach.)

## Run it

```sh
fletch-host serve
```

Options, all of which apply to that run only and are never written to the
database:

| Flag | Default |
| --- | --- |
| `--data-dir PATH` | `~/Library/Application Support/fletch-host`, or `$XDG_DATA_HOME/fletch-host` (`…/fletch-host/dev` from a debug build) |
| `--port N` | the stored `remote.port`, else `47285` |
| `--relay URL` / `--no-relay` | the stored `remote.relay_url` |
| `--name NAME` | the machine's name |

Remote access is always on: serving paired devices is the only reason the
process exists, so the desktop's `remote.enabled` setting is not consulted.
Logs go to stderr at `info`; `RUST_LOG` overrides that (`RUST_LOG=debug`,
`RUST_LOG=info,fletch_core::remote=debug`).

`SIGINT` and `SIGTERM` kill the agents' processes and exit 0, so a restart is
never mistaken for a crash.

## Run it as a service

Both units in `packaging/` run the host **as your own user**, never as root:
everything it touches is yours, and on Linux the container sandbox maps the
agent's files back to that same user (see "The sandbox" below).

Linux, systemd:

```sh
install -Dm644 packaging/fletch-host.service ~/.config/systemd/user/fletch-host.service
systemctl --user daemon-reload
systemctl --user enable --now fletch-host
loginctl enable-linger $USER          # or it stops when you log out of SSH
journalctl --user -u fletch-host -f   # logs
```

`enable-linger` is the step people forget: without it systemd tears your user
session down when the last SSH connection closes, agents and all.

macOS, launchd (`launchd` does not expand `~`, so substitute your user name):

```sh
sed "s/YOU/$USER/g" packaging/com.fletch.host.plist \
  > ~/Library/LaunchAgents/com.fletch.host.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.fletch.host.plist
tail -f ~/Library/Logs/fletch-host.log
```

Set `QUORUM_GITHUB_CLIENT_ID` in whichever unit you use if you want
`fletch-host github login` to work (see "GitHub" below); both files have the
line commented out and ready.

## The sandbox

On macOS an agent runs under `sandbox-exec` by default, exactly as it does on
the desktop. On Linux there is no such thing, so the host needs **Docker or
Podman** and refuses to start without one — it will not silently run an agent
outside the boundary it promised. With no stored preference it picks Docker if
the daemon answers, else Podman.

Two consequences of running containers on Linux, where (unlike macOS) the
container's root really is the machine's root:

- With a **rootful Docker daemon** the launch adds `--user <uid>:<gid>` so the
  agent's checkout, its RPC replies and claude's transcripts come back owned by
  the service user instead of by root. That also means an agent cannot
  `apt-get install` inside its own container: bake what your project needs into
  a custom image (`docker_image` in Settings) instead. The **Cursor** image is
  the one provider this breaks — its CLI installs under `/root`, which only
  root can read — so use Cursor on a macOS host, or another provider here.
- **Rootless Docker and Podman** already map the container's root to your user,
  so they are left exactly as they are.

## Pair a phone

With the host running, on the host:

```sh
fletch-host pair
```

That prints a `fletch://pair?…` link, that link as a QR code, the 8-character
code inside it, and when both expire (5 minutes). On the phone, add a host and
scan the code (or paste the link). The link carries this host's public key,
which the phone pins — the code only authorizes one pairing.

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
provider), not once per run — but they must be the logins of **the user the
service runs as**, since that is whose home directory the agent (and the
container sandbox, which mounts `~/.claude`) reads. `ssh` in as that user.

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
  database beside it;
- RPC mailboxes are `0700` too, and agent checkouts are whatever your `umask`
  makes them — on Linux the container writes them as your uid, which is the
  whole point of the mapping above.

So: **run the host as its own user, and let nothing else read that directory.**
Anyone who can read it can push as you.

Deliberately not the desktop's data directory. A Mac running both must not have
two engines on one SQLite database — each would sweep the other's live agents as
orphans. A debug build takes a `dev` subfolder of it for the same reason: one
machine, two builds, two databases.

Agent checkouts and RPC mailboxes hang off the same directory, one level down:

```
<data-dir>/workspaces/<agent>/   # checkouts (dev/workspaces from a debug build)
<data-dir>/rpc/<agent>/          # RPC mailboxes, 0700
```

So `--data-dir` is the whole of a host's state, and two hosts with two data dirs
share none of it. That matters beyond tidiness: agent names come from a pool of
place names and are recycled, so two engines with two databases *will*
eventually both name an agent `fuji` — and a shared root would put both in one
checkout directory and make each one's startup sweep treat the other's live
mailboxes as garbage. The desktop keeps its own two roots under `~/.fletch`,
and a debug host takes the `dev/` subfolder of these for the same reason it
takes one of the data dir.

`$FLETCH_WORKSPACES_ROOT` and `$FLETCH_RPC_ROOT` override the base if you want
these elsewhere (on a disk with room for checkouts, say); the host only sets
them when they are unset, and then keeping them unique per host is yours to do.

## Known gaps

- **No PTY streaming**: a remote client sees agent status and events, not a live
  terminal. That is Phase 5 of the multi-host plan.
- **Cursor cannot run in a container on a Linux host** (see "The sandbox").
- **No `fletch-host update`**: upgrading is downloading the new tarball over
  the old binary and restarting the service.
