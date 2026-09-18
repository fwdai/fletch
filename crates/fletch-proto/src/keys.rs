//! The static X25519 identity every end of the protocol proves in the
//! handshake, and the file it is kept in.
//!
//! One type for both ends. A host's key and a device's key differ only in the
//! file name and in what the public half is called downstream (the **host ID**
//! the pairing link carries, or the device key the host records when pairing);
//! the bytes, the derivation and the file rule are the same, so they are one
//! type here rather than two near-identical ones in two crates.

use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use snow::params::DHChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};

use crate::Result;

/// X25519 key length, for both halves of the pair.
pub const KEY_LEN: usize = 32;

/// The file a host keeps its identity in, inside its remote dir.
pub const HOST_KEY_FILE: &str = "host_key";
/// The file a client device keeps its identity in.
pub const DEVICE_KEY_FILE: &str = "device_key";

/// A key as the protocol writes it: base64url, no padding — 43 characters for
/// the 32 bytes of an X25519 public key.
pub fn encode_key(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// One end's static identity. The private half never leaves the machine; the
/// public half is the identity the peer pins.
///
/// Generated on first use and kept for the life of the install. Deleting the
/// file regenerates it, after which every pairing has to be made again — which
/// is also the recovery path for a stolen key.
pub struct StaticKey {
    private: [u8; KEY_LEN],
    public: [u8; KEY_LEN],
}

impl StaticKey {
    /// Read `<dir>/<file>`, or generate and persist one on first use.
    ///
    /// Only a *missing* file means a new identity (protocol doc, "Secure
    /// channel"). A file of the wrong length, or one that cannot be read, is an
    /// error the user sees: regenerating would silently invalidate every
    /// pairing, and a transient read failure must not cost the identity. The
    /// write is atomic (temp file, 0600, rename), so a crash mid-write leaves
    /// no partial key behind to be mistaken for corruption next launch.
    pub fn load_or_create(dir: &Path, file: &str) -> Result<Self> {
        let path = dir.join(file);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let private: [u8; KEY_LEN] = bytes.try_into().map_err(|_| {
                    format!(
                        "the key at {} is not a {KEY_LEN}-byte key; delete it to generate a new \
                         one, after which every pairing has to be made again",
                        path.display()
                    )
                })?;
                Self::from_private(private)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::generate()?;
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                write_private(&path, &key.private)?;
                Ok(key)
            }
            Err(e) => Err(format!(
                "the key at {} could not be read: {e}",
                path.display()
            )),
        }
    }

    /// A fresh identity, nowhere on disk. Used on first launch and by the tests.
    pub fn generate() -> Result<Self> {
        let keypair = crate::noise::builder()?
            .generate_keypair()
            .map_err(|e| format!("cannot generate a key: {e}"))?;
        let private: [u8; KEY_LEN] = keypair
            .private
            .try_into()
            .map_err(|_| "a generated key is not 32 bytes".to_string())?;
        Self::from_private(private)
    }

    /// An identity from raw private bytes, without touching the disk. The relay
    /// vectors build both ends' keys this way.
    pub fn from_private(private: [u8; KEY_LEN]) -> Result<Self> {
        let public = public_from_private(&private)?;
        Ok(Self { private, public })
    }

    /// The raw public key. The relay's host-link challenge hashes these bytes,
    /// not the base64 string.
    pub fn public_bytes(&self) -> &[u8; KEY_LEN] {
        &self.public
    }

    /// The public key as the protocol writes it: base64url, unpadded. For a
    /// host this is the **host ID** — what the pairing link carries, what the
    /// phone pins, and what the relay routes on.
    pub fn public_base64(&self) -> String {
        encode_key(&self.public)
    }

    /// `X25519(this private, their_public)`: the shared secret the relay's
    /// host-link challenge is answered with (`docs/remote-protocol.md` →
    /// "Relay").
    ///
    /// Goes through the same DH implementation the handshake and
    /// [`StaticKey::from_private`] use, so the relay proof needs no second
    /// crypto dependency and cannot disagree with the identity it is proving.
    pub fn diffie_hellman(&self, their_public: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN]> {
        let mut dh = curve25519()?;
        dh.set(&self.private);
        let mut shared = [0u8; KEY_LEN];
        dh.dh(their_public, &mut shared)
            .map_err(|e| format!("cannot compute the shared secret: {e}"))?;
        Ok(shared)
    }

    /// The private half, for the handshake builder next door.
    pub(crate) fn private_bytes(&self) -> &[u8; KEY_LEN] {
        &self.private
    }
}

/// Temp file, owner-only, then rename over the target: the key is never
/// half-written and never briefly world-readable.
fn write_private(path: &Path, private: &[u8; KEY_LEN]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, private).map_err(|e| format!("{}: {e}", tmp.display()))?;
    restrict(&tmp);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

/// The X25519 public key for a stored private key. Only the private half is on
/// disk (the doc fixes the file at 32 bytes), so the public half is derived at
/// load through the same DH implementation the handshake uses.
fn public_from_private(private: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN]> {
    let mut dh = curve25519()?;
    dh.set(private);
    dh.pubkey()
        .try_into()
        .map_err(|_| "an X25519 public key is not 32 bytes".to_string())
}

/// The X25519 primitive `snow` would use inside the handshake, on its own — for
/// the two places that need a bare DH: deriving the public key from the stored
/// private one, and answering the relay's challenge.
fn curve25519() -> Result<Box<dyn snow::types::Dh>> {
    DefaultResolver
        .resolve_dh(&DHChoice::Curve25519)
        .ok_or_else(|| "no curve25519 implementation".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_file_is_stable_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let first = StaticKey::load_or_create(dir.path(), HOST_KEY_FILE).unwrap();
        let second = StaticKey::load_or_create(dir.path(), HOST_KEY_FILE).unwrap();
        assert_eq!(first.public_base64(), second.public_base64());
        // base64url of 32 bytes, unpadded.
        assert_eq!(first.public_base64().len(), 43);
        let encoded = first.public_base64();
        assert!(!encoded.contains('=') && !encoded.contains('+') && !encoded.contains('/'));
        assert_eq!(
            std::fs::read(dir.path().join(HOST_KEY_FILE)).unwrap().len(),
            KEY_LEN
        );
    }

    /// The two file names are separate identities in the same directory, which
    /// is what lets one machine be a host and a client at once.
    #[test]
    fn the_host_and_device_files_are_separate_identities() {
        let dir = tempfile::tempdir().unwrap();
        let host = StaticKey::load_or_create(dir.path(), HOST_KEY_FILE).unwrap();
        let device = StaticKey::load_or_create(dir.path(), DEVICE_KEY_FILE).unwrap();
        assert_ne!(host.public_base64(), device.public_base64());
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DEVICE_KEY_FILE), b"not a key").unwrap();

        // No `Debug` on a private key, so no `unwrap_err`.
        let err = match StaticKey::load_or_create(dir.path(), DEVICE_KEY_FILE) {
            Ok(_) => panic!("a corrupt key file must not load"),
            Err(e) => e,
        };
        assert!(err.contains("not a 32-byte key"), "{err}");
        assert_eq!(
            std::fs::read(dir.path().join(DEVICE_KEY_FILE)).unwrap(),
            b"not a key",
            "left in place for the user to delete deliberately"
        );
        assert!(!dir.path().join("device_key.tmp").exists());
    }

    /// Both ends must reach the same secret from opposite sides, which is what
    /// the relay's challenge relies on.
    #[test]
    fn x25519_agrees_from_either_side() {
        let host = StaticKey::from_private([0x01; KEY_LEN]).unwrap();
        let relay = StaticKey::from_private([0x02; KEY_LEN]).unwrap();
        assert_eq!(
            host.diffie_hellman(relay.public_bytes()).unwrap(),
            relay.diffie_hellman(host.public_bytes()).unwrap()
        );
    }
}
