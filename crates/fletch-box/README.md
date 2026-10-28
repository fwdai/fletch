# fletch-box

The box sandbox runtime: libkrun micro-VMs with no shared folder and no network
device, reachable from the host only over SSH on vsock and an egress proxy. It
depends on nothing else in Fletch; `fletch-core`'s box engine
(`sandbox::boxes`) composes it per box, and the `fletch-box` binary drives it
by hand.

Two invariants hold throughout. The guest holds the agent's tree at the same
absolute paths the host uses, so nothing here translates a path. Nothing of the
host's enters the guest except the env file written for each launch; everything
else the host does to a box goes over SSH on vsock or through the egress proxy,
and a step that cannot establish that fails closed rather than falling back to
the host.

| Module | What |
| --- | --- |
| `guest` | The guest contract as constants: vsock ports, console markers, env names, paths, `fletch-guest` subcommands. `fletch-guest` and the image's init implement the other side from the same table. |
| `runtime` | `RuntimePaths` (where the helper and dylibs are, `probe()`), `VmSpec` → helper argv (`launch.rs`, shared with the helper by `#[path]`), `Vm` (start, `wait_ready` on the console markers, the stop ladder, kill). |
| `disks` | Sparse data disk, APFS `clone`, script disks for tests, the `dirty` marker. |
| `image_store` | Pinned base image download with size and sha256 verification, versions, prune. |
| `egress` | `HostPattern`, `EgressPolicy` with the default allow-list, the HTTP `CONNECT` proxy on a unix socket emitting `EgressEvent`s. |
| `channel` | `Channel`: the system OpenSSH client over a generated `ssh_config` whose `ProxyCommand` is `fletch-vm pipe <ssh.sock>`; exec, PTY command, put/get over stdin, `GIT_SSH_COMMAND`, `-L` forwards, relays; key generation and `known_hosts`. |
| `tools` | Tools disks: one read-only ext4 disk per `<provider>-<version>` holding that provider's CLI, built by booting a throwaway box and running the embedded install script through the egress proxy. |
| `bin/fletch-vm` | The helper: the only binary signed with the hypervisor entitlement. Loads libkrun at runtime, exits when its stdin closes, also the stdio-to-socket bridge (`pipe`). |
| `bin/fletch-box` | Manual-test CLI: `probe`, `boot`, `ssh`, `stop`, `image`, `tools build`. |

## Development

```sh
eval "$(scripts/box-dev.sh)"        # stages the runtime (packaging/box-bundle/ensure-dev.sh), builds the CLI, exports
                                    # FLETCH_BOX_HELPER, FLETCH_BOX_LIB_DIR and puts fletch-box on PATH
fletch-box probe
```

The guest is the Debian image `packaging/box-image` builds (`build-mac.sh` →
`out/base.erofs`). The runtime does not care which Linux is inside, only that
init prints the markers and binds the vsock ports; `dev/calina-sshd.sh` is a
stand-in init script for a dev image without `fletch-guest`, following the same
contract with a dropbear on the data disk.

```sh
export FLETCH_BOX_RUN_ROOT=/tmp/fletch-box-dev   # box dirs; default ~/.fletch/box-dev (short paths: sockets)
fletch-box boot --id dev1 --image packaging/box-image/out/base.erofs \
  --data $FLETCH_BOX_RUN_ROOT/dev1/data.img \
  --env FLETCH_BOX_HOME=$HOME --env FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/dev1 \
  --env FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/dev1
fletch-box ssh dev1 -- uname -r                # as `agent`; -l root for images without that user
fletch-box stop dev1                           # or Ctrl-C in the boot terminal
```

`boot` creates the data disk sparse when it is missing, generates the key pair
and passes `FLETCH_BOX_AUTHORIZED_KEY` itself. The box dir then holds `key`,
`key.pub`, `known_hosts`, `ssh_config`, `ssh.sock`, `ctl.sock`, `egress.sock`,
`console.log`, `dirty` (while running) and `vm.pid`. `ssh -F <box>/ssh_config
box …`, `GIT_SSH_COMMAND="ssh -F …"` and `ssh -F … -N -L` all work from any
shell.

## Tests

```sh
cargo test                                      # unit tests; compile and pass on Linux too
cargo clippy --all-targets -- -D warnings
eval "$(scripts/box-dev.sh)"
FLETCH_BOX_TEST_IMAGE=<base.erofs> FLETCH_BOX_TEST_INIT=<init> \
  cargo test --test vm -- --ignored --nocapture --test-threads 1
```

The VM tests boot the image with a script disk that prints the markers and
waits for `stop` on the control port: clean stop clears `dirty`, kill leaves it
and the next start removes the stale listen socket, closing the lifeline ends
the helper. They print boot and stop timings. They need an image whose init
runs the command libkrun hands it (`/bin/sh /dev/vdc`, the script disk) and
ships `socat`; the product init takes no argv, so this is a dev image, its init
named by `FLETCH_BOX_TEST_INIT`. The product image is exercised end to end by
`crates/fletch-core/src/sandbox/boxes/tests.rs`.

Measured on an M-series Mac (macOS 27, from inside a Fletch seatbelt sandbox),
1 vCPU, 1 GiB: first boot including formatting the data disk ready in 1.0 s,
later boots 0.16–0.24 s, control stop 27 ms. With dropbear on the data disk:
cold `ssh` 80 ms, 51 ms through the ControlMaster, `ssh -L` forward HTTP 200 in
2.7 ms, a small `git fetch` 1.1 s, egress `CONNECT registry.npmjs.org` allowed
and `example.com` denied with events.
