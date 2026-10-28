# fletch-box

The box sandbox runtime: libkrun micro-VMs with no shared folder and no network
device, reachable from the host only over SSH on vsock and an egress proxy.
Design in `docs/boxes.md`, build spec in `docs/boxes-spec.md`; this crate is
phase 1 (§11). It depends on nothing else in Fletch; `fletch-core`'s box engine
(phase 4) composes it.

| Module | What |
| --- | --- |
| `guest` | The guest contract as constants: vsock ports, console markers, env names, paths, `fletch-guest` subcommands. Phase 2 implements the other side from the same table. |
| `runtime` | `RuntimePaths` (where the helper and dylibs are, `probe()`), `VmSpec` → helper argv (`launch.rs`, shared with the helper by `#[path]`), `Vm` (start, `wait_ready` on the console markers, the stop ladder, kill). |
| `disks` | Sparse data disk, APFS `clone`, script disks for tests, the `dirty` marker. |
| `image_store` | Pinned base image download with size and sha256 verification, versions, prune (port of Calina's). |
| `egress` | `HostPattern`, `EgressPolicy` with the default allow-list, the HTTP `CONNECT` proxy on a unix socket emitting `EgressEvent`s. |
| `channel` | `Channel`: the system OpenSSH client over a generated `ssh_config` whose `ProxyCommand` is `fletch-vm pipe <ssh.sock>`; exec, PTY command, put/get over stdin, `GIT_SSH_COMMAND`, `-L` forwards, relays; key generation and `known_hosts`. |
| `bin/fletch-vm` | The helper: the only binary signed with the hypervisor entitlement. Loads libkrun at runtime, exits when its stdin closes, also the stdio-to-socket bridge. |
| `bin/fletch-box` | Manual-test CLI: `probe`, `boot`, `ssh`, `stop`, `image`. |

## Development

```sh
eval "$(scripts/box-dev.sh)"        # builds both binaries in release, ad-hoc signs fletch-vm, exports
                                    # FLETCH_BOX_HELPER, FLETCH_BOX_LIB_DIR and puts fletch-box on PATH
fletch-box probe
```

`FLETCH_BOX_LIB_DIR` is `packaging/libkrun/dist` once `packaging/libkrun/ensure.sh`
has built it, else Calina's identical dist. Until phase 2 publishes the Debian
image, Calina's Alpine image (`~/Code/calina/sandbox/image/out/dev/base.erofs`)
is the dev guest: the runtime does not care which Linux is inside, only that
init prints the markers. `dev/calina-sshd.sh` is a stand-in init for it that
follows the guest contract with the spike's dropbear data disk
(`docs/boxes.md`, phase 2a).

```sh
export FLETCH_BOX_RUN_ROOT=/tmp/fletch-box-dev   # box dirs; default ~/.fletch/box-dev (short paths: sockets)
cp -c /tmp/fletch-box-spike/ws1/data.img $FLETCH_BOX_RUN_ROOT/dev1/data.img
fletch-box boot --id dev1 --image ~/Code/calina/sandbox/image/out/dev/base.erofs \
  --data $FLETCH_BOX_RUN_ROOT/dev1/data.img --init /opt/calina/init \
  --script-disk crates/fletch-box/dev/calina-sshd.sh
fletch-box ssh dev1 -l root -- uname -r        # -l root: Calina's image has no `agent` user
fletch-box stop dev1                           # or Ctrl-C in the boot terminal
```

The box dir then holds `key`, `key.pub`, `known_hosts`, `ssh_config`,
`ssh.sock`, `ctl.sock`, `egress.sock`, `console.log`, `dirty` (while running)
and `vm.pid`. `ssh -F <box>/ssh_config box …`, `GIT_SSH_COMMAND="ssh -F …"`
and `ssh -F … -N -L` all work from any shell.

## Tests

```sh
cargo test                                      # unit tests; compile and pass on Linux too
cargo clippy --all-targets -- -D warnings
eval "$(scripts/box-dev.sh)"
FLETCH_BOX_TEST_IMAGE=~/Code/calina/sandbox/image/out/dev/base.erofs \
  cargo test --test vm -- --ignored --nocapture --test-threads 1
```

The VM tests boot the image with a script disk that prints the markers and
waits for `stop` on the control port: clean stop clears `dirty`, kill leaves it
and the next start removes the stale listen socket, closing the lifeline ends
the helper. They print boot and stop timings.

Measured on an M-series Mac (macOS 27, from inside a Fletch seatbelt sandbox),
Calina's image, 1 vCPU, 1 GiB: first boot including formatting the data disk
ready in 1.0 s, later boots 0.16–0.24 s, control stop 27 ms. With dropbear on
the data disk: cold `ssh` 80 ms, 51 ms through the ControlMaster, `ssh -L`
forward HTTP 200 in 2.7 ms, `git fetch` of the spike repo 1.1 s, egress
`CONNECT registry.npmjs.org` allowed and `example.com` denied with events.
