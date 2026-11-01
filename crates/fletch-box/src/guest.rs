//! The host side of the guest contract: the vsock ports, console markers, env
//! names, guest paths and `fletch-guest` subcommands both ends agree on.
//! `fletch-guest` and the image's init implement the other side from this same
//! table; change both or neither.

/// The guest's SSH server listens here; libkrun exposes it as the box's
/// `ssh.sock` (listen mode: the host connects).
pub const SSH_PORT: u32 = 22;
/// The guest connects here for HTTP `CONNECT` egress; the host serves
/// `egress.sock`.
pub const EGRESS_PORT: u32 = 6000;
/// Init's `fletch-guest wait-stop` listens here; the host connects through
/// `ctl.sock` and writes [`STOP_LINE`].
pub const CONTROL_PORT: u32 = 5000;
/// What the host writes on the control port to ask for a clean shutdown.
pub const STOP_LINE: &str = "stop\n";

/// Console markers printed by init, each on its own line.
pub const MARK_HOSTKEY: &str = "FLETCH_HOSTKEY";
pub const MARK_READY: &str = "FLETCH_READY";
pub const MARK_INIT_ERROR: &str = "FLETCH_INIT_ERROR";

/// Kernel-side environment names init reads (set through `krun_set_env`, which
/// is safe for `KEY=VALUE`; only argv is re-split by libkrun's init).
pub const ENV_WRITABLE_ROOT: &str = "FLETCH_BOX_WRITABLE_ROOT";
pub const ENV_RPC_DIR: &str = "FLETCH_BOX_RPC_DIR";
pub const ENV_HOME: &str = "FLETCH_BOX_HOME";
/// One OpenSSH public key line; init writes it to the agent's authorized_keys.
pub const ENV_AUTHORIZED_KEY: &str = "FLETCH_BOX_AUTHORIZED_KEY";
pub const ENV_BOX_ID: &str = "FLETCH_BOX_ID";
/// How many folders the host shares with the guest (absent = none). Share `n`
/// is the virtio-fs device tagged [`share_tag`]`(n)`, described by
/// `FLETCH_BOX_SHARE_<n>` = `ro:<path>` or `rw:<path>`: init mounts it at that
/// path, in index order (the host lists parents before the folders inside
/// them), after the identity binds.
pub const ENV_SHARES: &str = "FLETCH_BOX_SHARES";
pub const ENV_SHARE_PREFIX: &str = "FLETCH_BOX_SHARE_";
/// The roots init can mount a share under: the image ships both as empty
/// mountpoints.
pub const SHARE_ROOTS: &[&str] = &["/Users", "/Volumes"];

/// The virtio-fs tag of share `n`.
pub fn share_tag(n: usize) -> String {
    format!("fletch-share-{n}")
}

/// Guest paths.
pub const INIT: &str = "/opt/fletch/init";
pub const GUEST_AGENT: &str = "/opt/fletch/bin/fletch-guest";
pub const TOOLS_MOUNT: &str = "/opt/fletch/tools";
pub const DATA_MOUNT: &str = "/data";
/// Where per-launch env files land (`fletch-guest exec --env-file`).
pub const ENV_FILE_DIR: &str = "/data/run";
pub const USER: &str = "agent";
pub const UID: u32 = 1000;
pub const GID: u32 = 1000;

/// The guest's loopback proxy, for processes that only honour `HTTPS_PROXY`.
pub const LOOPBACK_PROXY: &str = "http://127.0.0.1:8899";

/// Block device order: the base image, the data disk, then the optional tools
/// disk and the layer images. Init tells the disks after the data disk apart
/// by content (ext4 is the tools disk, an erofs with [`LAYER_MARKER`] is a
/// layer), not by letter.
pub const DISK_BASE: &str = "/dev/vda";
pub const DISK_DATA: &str = "/dev/vdb";
pub const DISK_TOOLS: &str = "/dev/vdc";
/// A layer image's marker file (`{name, version, dirs}`); init overlays each
/// of the directories it names on the root, read-only.
pub const LAYER_MARKER: &str = "/.fletch-layer";
/// Where init mounts the layer images before overlaying them.
pub const LAYERS_MOUNT: &str = "/run/layers";

/// `fletch-guest` subcommands the host invokes over the channel.
pub const EXEC: &str = "exec";
/// `exec --read-only <dir>`, repeatable: the command sees `<dir>` read-only,
/// mounts below it included, enforced by the guest kernel (namespaces of the
/// command's own: a recursive read-only bind of `<dir>`, `/data` hidden, a
/// private `/proc`, no capabilities). A `fletch-guest` older than the option
/// (image 0.3.0) refuses it as an unknown option, before running anything.
pub const EXEC_READ_ONLY: &str = "--read-only";
/// `exec --writable <path>`, repeatable: an existing directory or file inside
/// an [`EXEC_READ_ONLY`] directory that the command may write after all (a
/// bind of its own, writable). Every path on the way must be a real one, not
/// a symbolic link. New with image 0.3.1, like `--read-only` in its shipped
/// form.
pub const EXEC_WRITABLE: &str = "--writable";
/// `exec --cow <dir> <store>`, repeatable, two arguments: an existing
/// directory inside an [`EXEC_READ_ONLY`] directory that the command sees as
/// a private copy-on-write view (an overlay mounted on `<dir>`, `<dir>` its
/// lower layer): it reads what is there, and whatever it writes, new files
/// included, lands in `<store>/`[`EXEC_COW_UPPER`], never in `<dir>`.
/// `<store>` lies where the command may write (inside an [`EXEC_WRITABLE`]
/// path, the launch's own root) and need not exist; the guest makes it inside
/// the confinement. No `--writable` path may lie in `<dir>` (the overlay
/// would hide its mount). New with image 0.3.1.
pub const EXEC_COW: &str = "--cow";
/// `exec --cow-file <file> <copy>`, repeatable, two arguments: an existing
/// regular file inside an [`EXEC_READ_ONLY`] directory, covered for the
/// command by a bind of `<copy>`, a path where the command may write (inside
/// an [`EXEC_WRITABLE`] one) that the guest copies `<file>` to (owner-only)
/// when it does not exist yet, and keeps after. New with image 0.3.1.
pub const EXEC_COW_FILE: &str = "--cow-file";
/// The directory under an [`EXEC_COW`] store that holds what the command
/// wrote under `<dir>`, at the same relative paths (the overlay's upper
/// layer; `work` beside it is the overlay's own).
pub const EXEC_COW_UPPER: &str = "upper";
/// What `exec` exits with when it cannot set up `--read-only`: the command
/// never ran. Apart from 126 (could not run) and 127 (not found).
pub const EXEC_CONFINE_FAILED: i32 = 125;
pub const RELAY: &str = "relay";
pub const TAIL: &str = "tail";

/// The ssh `Host` alias in every generated `ssh_config`, and the name in
/// `known_hosts`.
pub const SSH_HOST_ALIAS: &str = "box";
