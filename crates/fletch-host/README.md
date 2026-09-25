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

```sh
curl -fsSL https://raw.githubusercontent.com/fwdai/fletch/main/scripts/install-host.sh | bash
```

That downloads this machine's tarball, verifies it, installs the binary, runs
`fletch-host service install` and ends at the pairing QR — the manual steps
below, in one command. A fresh install needs **minisign** on `PATH`
(`brew install minisign`, `apt install minisign`, `dnf install minisign`): the
signature is what makes the download the project's binary rather than whoever
answered, so a missing minisign or a missing `.sig` stops the install instead of
warning and carrying on.

Run it again with the same `FLETCH_HOST_INSTALL_DIR` and it installs nothing: it
finds the binary already in that directory and hands the upgrade to
`fletch-host update`, which verifies the same two ways, copies the database
aside and restarts the service through the unit it already has — rather than
re-running `service install`, which would rewrite that unit and reset the
`--port`, `--name` or `--system` you installed it with. Only that directory
counts, never another `fletch-host` that happens to be on `PATH`: the service
unit names an absolute path, so upgrading a binary somewhere else would leave
the one the service actually runs behind. An installed service is found even
when no binary sits in that directory — `fletch-host service show` reports the
unit wherever it is — so pointing `FLETCH_HOST_INSTALL_DIR` somewhere new stops
the install with the two ways out rather than resetting the service you have.

Three env vars configure it: `FLETCH_HOST_VERSION=0.8.0` pins a release instead
of taking the latest (and is what a re-run passes to `update`),
`FLETCH_HOST_INSTALL_DIR` moves the binary off `~/.local/bin`, and
`FLETCH_HOST_SKIP_SIGNATURE=1` installs without checking the signature — the
insecure way past the minisign requirement, and it says so while it does it.

By hand instead. Each release publishes
`fletch-host-<version>-<target>.tar.gz`, a matching `.sha256`, and a `.sig`
(minisign, the same key the desktop's updater trusts — `fletch-host update`
checks both):

```sh
V=<the release version>                       # e.g. 0.7.32, without the leading v
T=x86_64-unknown-linux-gnu                    # or aarch64-unknown-linux-gnu, aarch64-apple-darwin
curl -fsSLO https://github.com/fwdai/fletch/releases/download/v$V/fletch-host-$V-$T.tar.gz
curl -fsSLO https://github.com/fwdai/fletch/releases/download/v$V/fletch-host-$V-$T.tar.gz.sha256
sha256sum -c fletch-host-$V-$T.tar.gz.sha256   # shasum -a 256 -c on macOS
tar xzf fletch-host-$V-$T.tar.gz
install -Dm755 fletch-host ~/.local/bin/fletch-host
```

`~/.local/bin` is a directory you own, which is what makes `fletch-host update`
able to replace the binary without `sudo`. Make sure it is on your `PATH`
(`export PATH="$HOME/.local/bin:$PATH"`). The tarball also carries this README
and both service templates, so the paths under "Run it as a service" work from
the extracted directory as well as from a checkout.

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

```sh
fletch-host service install            # systemd user unit, or a launchd user agent
fletch-host service uninstall
```

Either way the host runs **as your own user**, never as root: everything it
touches is yours, and on Linux the container sandbox maps the agent's files
back to that same user (see "The sandbox" below).

`install` writes the service definition, enables it and starts it, printing
every file it wrote and every command it ran. Running it again rewrites the
definition and restarts the host, so it is also how you change the flags:

| Flag | Meaning |
| --- | --- |
| `--data-dir PATH` | the data dir to serve (global flag; defaults to the same one every other subcommand uses). A relative path is taken from the directory you run this in and written absolute |
| `--port N` | port for paired devices |
| `--name NAME` | the name paired devices show |
| `--user NAME` | Linux, with `--system`: the user the unit runs as, by name or uid. Defaults to whoever ran `sudo`; root (by any spelling) is refused |
| `--system` | Linux: `/etc/systemd/system` instead of your own user unit. Needs `sudo`; not a thing on macOS |

The resolved data dir and this binary's absolute path are written into the
definition rather than left to be re-derived, because an init system's
environment is not your shell's — `$HOME` and `$XDG_DATA_HOME` may be unset or
different, and the service has to open the same data dir the CLI subcommands
talk to.

With `--system` the command itself runs as root, so its own default data dir
would be root's. Instead the unit gets the service user's:
`<their home>/.local/share/fletch-host`, from their passwd entry. If that user
keeps a custom `$XDG_DATA_HOME`, pass `--data-dir` explicitly; the unit cannot
see their environment.

**Linux.** The unit is `~/.config/systemd/user/fletch-host.service`
(`/etc/systemd/system/fletch-host.service` with `--system`). One step is left
for you, because it needs `sudo`:

```sh
loginctl enable-linger $USER          # or the host stops when you log out of SSH
journalctl --user -u fletch-host -f   # logs
```

`enable-linger` is the step people forget: without it systemd tears your user
session down when the last SSH connection closes, agents and all. `install`
prints the command.

**macOS.** The agent is `~/Library/LaunchAgents/com.fletch.host.plist`, loaded
with `launchctl bootstrap gui/$(id -u)` (and `launchctl load -w` on macOS old
enough to need it). Logs: `tail -f ~/Library/Logs/fletch-host.log`. A Mac that
sleeps is a host that answers nothing — `sudo pmset -a sleep 0`.

Set `QUORUM_GITHUB_CLIENT_ID` in the installed definition if you want
`fletch-host github login` to work (see "GitHub" below); both templates have
the line commented out and ready.

### Installing the definition by hand

`packaging/fletch-host.service` and `packaging/com.fletch.host.plist` are the
only copies of those two files: `service install` compiles them in with
`include_str!` and substitutes a handful of `{{PLACEHOLDER}}` tokens, so a
definition written by the CLI and one written by hand cannot drift. Each file's
header lists its tokens and what to put in them. That is why the copies in the
release tarball still have the tokens in them — they are the template, not a
rendered default.

## Update

```sh
fletch-host update --check     # current vs available, and nothing else
fletch-host update             # install the latest release
fletch-host update 0.7.32      # install (or reinstall) one version
```

A named version *older* than the running one is refused, and so is one that
cannot be ordered against it (a pre-release, say). An older host cannot open a
database this build has already migrated — it stops with "database schema is
newer than this app version" — so going back means restoring the pre-migration
copy from `<data-dir>/backups/` over the database as well. `--allow-downgrade`
says you mean to.

Run it **as the user the host runs as** — yourself for a user unit or a bare
`serve`, `sudo -u fletch fletch-host update` for a `--system --user fletch`
install. Not under `sudo`: it refuses, because everything below but the final
swap belongs in that user's data dir, and root has no business writing there.

What it does, in order — and it stops at the first thing that does not hold:

1. Resolves the release through the GitHub API and picks the asset for this
   build's target triple, `fletch-host-<version>-<target>.tar.gz`.
2. Downloads the tarball, its `.sha256` and its `.sig` into
   `<data-dir>/updates/<version>/`, and leaves them there.
3. Checks the sha256, **and** verifies the minisign signature against the key
   the desktop's updater trusts (`plugins.updater.pubkey` in
   `src-tauri/tauri.conf.json`; the release workflow signs the tarball with the
   matching private key). A missing or bad `.sig` aborts — a host that installs
   an unverified binary is a remote code execution path, so there is no
   `--force` for it.
4. Copies the SQLite database to
   `<data-dir>/backups/data.db.<old-version>.bak`. A running host's WAL is not
   in that copy, so stop the host first if you want a clean one.
5. If the running binary's directory is yours to write: extracts `fletch-host`
   next to it as `fletch-host.new`, chmods it 755 and `rename`s it over the
   current path — atomic, so the path never resolves to half a binary — and
   restarts the service from `service install` if there is one
   (`systemctl restart` / `launchctl kickstart -k`). If instead a bare `serve`
   is running — the admin socket answers — it says that it has to be restarted
   and does **not** kill it: a host with agents mid-run is not something an
   update gets to end.
6. If that directory needs root (`/usr/local/bin`, say), it stops with the
   download verified and the database copied, and prints the one command left:

   ```sh
   sudo '/usr/local/bin/fletch-host' update --from '<data-dir>/updates/<version>/fletch-host-<version>-<target>.tar.gz'
   ```

   The binary is named by its absolute path — root's `PATH` may find a
   different `fletch-host`, and it is this one, the one the service runs, that
   is being replaced. That step reads the tarball and its `.sig`, verifies the
   signature again (it trusts the signature, not the caller), replaces the
   binary, and restarts every installed unit whose `ExecStart` /
   `ProgramArguments` names that binary — a system unit directly, your user
   unit as you, a launchd agent in your session. A unit that runs some other
   `fletch-host` is left alone. It opens no data dir at all, and it refuses a binary
   directory that is not root's alone (owned by root, writable by nobody
   else), since root renaming a file someone else staged over its own binary
   would hand them root.

There is no rollback and no unattended upgrade path: that is deferred item 17
in `docs/multi-host-plan.md` §5.3. The database copy from step 4 is what a
manual rollback uses.

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
  the service user instead of by root. It also binds a generated `/etc/passwd`
  (and sets `no-new-privileges`) so the service user's uid resolves inside the
  container. Running as that user also means an agent cannot
  `apt-get install` inside its own container: bake what your project needs into
  a custom image (`docker_image` in Settings) instead.
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

`status` is one document: everything the desktop's Settings pane shows about
remote access (`enabled`, `listening`, the bound `port`, `name`, `hostId`,
reachable `addresses`, the paired `devices`, the `relay` link, any standing
`error`), plus what only this process knows —

- `dataDir`, `pid`, `version` — which host you are talking to.
- `agents: { total, running }` — live agents (archived ones are not counted)
  and how many are mid-turn right now.
- `sandboxEngine` — the selected engine, `sandbox-exec` / `docker` / `podman`.
  Worth a look on a Linux box, where seatbelt does not exist.
- `githubConnected` — whether a GitHub token is loaded, i.e. whether pushes and
  PRs will work. See "GitHub" below if it is `false`.
- `providers: { signedIn, signedOut }` — the installed agent CLIs, by whether
  they have a usable login. `fletch-host provider status` is the long form; see
  "Log in to the provider CLIs first" below.

Everything except `serve` talks to the running host over its admin socket, and
takes the same `--data-dir` to find it. With no host running they print
`fletch-host is not running; start it with 'fletch-host serve'` and exit 1.
A host that is up but still booting — a migration on a big data dir takes a
while — answers `the host is still starting` straight away, so these never hang
waiting on a start; and none of them is otherwise time-limited, because
`project clone` on a large repository is minutes of honest work.

## Approvals

If publish confirmation is on (`publish_confirmation`), an agent that wants to
push or open a PR asks first. On the desktop that is a dialog. Here it is:

```sh
fletch-host approvals list
fletch-host approve <id>           # or: fletch-host approve <id> --deny
```

An unanswered question is refused when `publish_approval_wait` elapses (120s by
default; `0` waits forever), so a host nobody is watching never publishes
without being asked. A paired phone or desktop can answer one too
(`answer_publish_approval`), and whichever side answers — or the wait lapsing —
takes the prompt down on all of them (`publish:approval-resolved`). So on a host
with a short wait, answer promptly or set `publish_approval_wait` to `0`.

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

Fletch runs `claude`, `codex`, `cursor-agent` and friends; their credentials are
theirs, not Fletch's, and several of them sign in by opening a browser. So
before the first agent runs, do it interactively on the host:

```sh
ssh you@host          # as the user the service runs as — this is required
fletch-host provider status
```

```
provider     installed  auth        fix
claude       2.1.4      signed out  fletch-host provider login claude
codex        0.48.0     signed in
cursor       —          not found   curl -fsSL https://cursor.com/install | bash
antigravity  —          not found   install Antigravity on this host
```

`fletch-host provider login claude` runs that CLI's own sign-in command — the
same one the desktop runs, with the same binary resolution (a custom binary path
set in settings wins, and the login shell's PATH is searched) — right in your
terminal, with its stdin and stdout. Whatever it asks for, a code to paste or a
browser to open, you answer it where you are sitting. When it exits the host
re-probes and prints what it now makes of that provider. antigravity and pi have
no CLI login: their credentials are made elsewhere and only land on this
machine, so `provider status` says so instead of offering a command.

Those logins persist in each tool's own config, so this is once per host (per
provider), not once per run — but they must be the logins of **the user the
service runs as**, since that is whose home directory the agent (and the
container sandbox, which mounts `~/.claude`) reads. That is a requirement, not
a preference: `provider login` refuses to run as anyone else (a login as root
would drop root-owned credential files into that user's home) and prints the
`sudo -u <user> fletch-host provider login <id>` that does it properly.

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

That includes the agents this host spawns. On a macOS host they run under
`sandbox-exec`, and the profile denies both reads and writes on this host's data
dir — so an agent (or anything a prompt injection talks it into) cannot read the
database and walk off with the GitHub token, cannot read `remote/host_key` to
impersonate the host, and cannot write `remote/devices.json` to pair a device of
its own. The two roots that hang off the same directory are re-allowed
individually, so an agent keeps its own checkout and its own RPC mailbox and
nothing else — a sibling agent's mailbox is not readable either. The one further
exception is `git-dist/`, the portable git Fletch downloads when the machine has
no usable system git: an agent may read and execute it (it is on the agent's
PATH, and is an unpacked upstream tarball, not a secret) but not write it. The
desktop app carries the same read-only exception against its own data dir, since
a shell's PATH search has to `stat` the binary before it will run it. A Run panel
command gets the same deny, with its checkout re-allowed. On Linux the question
does not arise: a container sees only the paths bound into it, and the data dir
is not one of them. None of this helps against anything *outside* the sandbox,
though — a `--data-dir` left world-readable is still world-readable, which is
what the `0700` above is for.

### Keeping the token out of the database

If a plaintext token in the database is not a trade you want to make, hand the
host the token instead and it never writes one:

```sh
FLETCH_GITHUB_TOKEN=ghp_... fletch-host serve
```

or, in the systemd unit, a credential — systemd reads the file as root and
exposes it to this service alone, so the token is neither in the unit nor in
anything else's environment:

```ini
LoadCredential=github_token:/etc/fletch/github_token
```

Either way the value is used for the life of the process and is never persisted;
`fletch-host status` reports which one it came from as `githubTokenSource`
(`env`, `credential` or `store`). The environment variable wins if both are set.

The trade-off: a supplied token is seeded *over* whatever the database holds, so
`fletch-host github login` still works and still stores its token, but the next
start is back on the supplied one (the command says so when it starts). Pick one
or the other — sign in, or supply the token and rotate it yourself.

Neither helps with the rest of the directory, which still holds this host's
private key and its paired devices. On a cloud box, put the data dir on an
encrypted volume.

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
- **No unattended upgrades and no rollback**: `fletch-host update` exists (see
  "Update"), but running it is yours to do, and undoing one is restoring the
  database copy it left behind by hand.
