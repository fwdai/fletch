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
pub const RELAY: &str = "relay";
pub const TAIL: &str = "tail";

/// The ssh `Host` alias in every generated `ssh_config`, and the name in
/// `known_hosts`.
pub const SSH_HOST_ALIAS: &str = "box";
