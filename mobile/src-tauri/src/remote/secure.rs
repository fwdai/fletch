// The Noise half of the secure channel, with no WebSocket in it: the device
// key, the three-message handshake as a state machine, and the chunked frame
// codec. docs/remote-protocol.md, "Secure channel".

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use snow::params::DHChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::{Builder, HandshakeState, TransportState};
use std::path::{Path, PathBuf};

/// `Noise_XX_25519_ChaChaPoly_BLAKE2s` with the protocol's prologue and empty
/// handshake payloads. Both are part of the contract: a peer that disagrees on
/// either fails the handshake.
const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE: &[u8] = b"fletch-remote-v2";

/// Noise's message cap (65 535) minus the 16-byte ChaChaPoly tag.
pub const MAX_CHUNK_PLAINTEXT: usize = 65_519;
const NOISE_MAX_MESSAGE: usize = 65_535;

/// Marker the TS side matches on to tell a pinned-key mismatch (which no retry
/// can fix) from a transport failure (which one might).
pub const HOST_KEY_MISMATCH: &str = "host-key-mismatch";

const KEY_FILE: &str = "device_key";
const KEY_LEN: usize = 32;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// This device's static X25519 identity. The private half never leaves the
/// phone; the public half is what the host records when pairing.
pub struct DeviceKey {
    private: Vec<u8>,
    public: Vec<u8>,
}

impl DeviceKey {
    /// Read `<dir>/device_key`, or generate and persist one on first use.
    ///
    /// Same rule as the host's key file (protocol doc, "Secure channel"): only
    /// a *missing* file means a new identity. A file of the wrong length, or
    /// one that cannot be read, is an error the user sees — regenerating would
    /// silently invalidate every pairing this phone has, and a transient read
    /// failure must not cost the identity. The write is atomic (temp file,
    /// 0600, rename), so a crash mid-write leaves no partial key behind to be
    /// mistaken for corruption on the next launch.
    pub fn load_or_create(dir: &Path) -> Result<Self, String> {
        let path: PathBuf = dir.join(KEY_FILE);
        let private = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == KEY_LEN => bytes,
            Ok(_) => {
                return Err(format!(
                    "the device key at {} is not a {KEY_LEN}-byte key; delete it to start over \
                     (every host will need pairing again)",
                    path.display()
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let keys = Builder::new(PATTERN.parse().map_err(|e| format!("{e}"))?)
                    .generate_keypair()
                    .map_err(|e| format!("cannot generate a device key: {e}"))?;
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                write_atomically(&path, &keys.private)?;
                keys.private
            }
            Err(e) => {
                return Err(format!(
                    "the device key at {} could not be read: {e}",
                    path.display()
                ))
            }
        };
        let public = public_of(&private)?;
        Ok(Self { private, public })
    }

    pub fn public_base64(&self) -> String {
        b64(&self.public)
    }
}

/// Temp file, owner-only, then rename over the target: the key is never
/// half-written and never briefly world-readable.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
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

/// X25519 base-point multiplication through snow's own resolver, so the stored
/// 32 bytes stay the single source of truth for the identity.
fn public_of(private: &[u8]) -> Result<Vec<u8>, String> {
    let mut dh = DefaultResolver
        .resolve_dh(&DHChoice::Curve25519)
        .ok_or_else(|| "no curve25519 implementation".to_string())?;
    dh.set(private);
    Ok(dh.pubkey().to_vec())
}

/// One side of the XX handshake. The phone is the initiator: `write`, `read`,
/// `write`, then `finish`.
pub struct Handshake {
    state: HandshakeState,
    buf: Vec<u8>,
}

impl Handshake {
    pub fn new(key: &DeviceKey, initiator: bool) -> Result<Self, String> {
        let builder = Builder::new(PATTERN.parse().map_err(|e| format!("{e}"))?)
            .local_private_key(&key.private)
            .and_then(|b| b.prologue(PROLOGUE))
            .map_err(|e| format!("cannot start the handshake: {e}"))?;
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|e| format!("cannot start the handshake: {e}"))?;
        Ok(Self {
            state,
            buf: vec![0u8; NOISE_MAX_MESSAGE],
        })
    }

    /// The next outgoing handshake message. Payloads are empty by contract.
    pub fn write(&mut self) -> Result<Vec<u8>, String> {
        let n = self
            .state
            .write_message(&[], &mut self.buf)
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(self.buf[..n].to_vec())
    }

    pub fn read(&mut self, message: &[u8]) -> Result<(), String> {
        let mut out = vec![0u8; NOISE_MAX_MESSAGE];
        self.state
            .read_message(message, &mut out)
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(())
    }

    /// The peer's static public key, base64url, once a message carrying it has
    /// been read. For `XX` that is message 2 — before this side has revealed
    /// its own identity in message 3.
    pub fn remote_static_base64(&self) -> Result<String, String> {
        remote_static(self.state.get_remote_static())
    }

    pub fn finish(self) -> Result<Channel, String> {
        let state = self
            .state
            .into_transport_mode()
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(Channel {
            state,
            buf: vec![0u8; NOISE_MAX_MESSAGE],
        })
    }
}

/// A live channel: one JSON document per frame, encrypted as repeated
/// `u16 big-endian ciphertext length || ciphertext` chunks.
pub struct Channel {
    state: TransportState,
    buf: Vec<u8>,
}

fn remote_static(key: Option<&[u8]>) -> Result<String, String> {
    key.map(b64)
        .ok_or_else(|| "the host presented no identity key".to_string())
}

impl Channel {
    /// The peer's static public key, base64url — the host's identity.
    pub fn remote_static_base64(&self) -> Result<String, String> {
        remote_static(self.state.get_remote_static())
    }

    /// One protocol frame as one binary message: repeated `u16 BE ciphertext
    /// length || ciphertext`. Empty plaintext is one chunk holding only the tag
    /// (the doc's rule), so a frame is never zero bytes and both ends' nonces
    /// advance together — hence `loop`, not `chunks()`, which yields nothing
    /// for empty input.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(plaintext.len() + 32);
        let mut rest = plaintext;
        loop {
            let (chunk, tail) = rest.split_at(rest.len().min(MAX_CHUNK_PLAINTEXT));
            let n = self
                .state
                .write_message(chunk, &mut self.buf)
                .map_err(|e| format!("cannot encrypt: {e}"))?;
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(&self.buf[..n]);
            rest = tail;
            if rest.is_empty() {
                return Ok(out);
            }
        }
    }

    pub fn decrypt(&mut self, frame: &[u8]) -> Result<Vec<u8>, String> {
        if frame.is_empty() {
            return Err("malformed frame: zero bytes".into());
        }
        let mut out = Vec::with_capacity(frame.len());
        let mut at = 0usize;
        while at < frame.len() {
            let rest = frame.len() - at;
            if rest < 2 {
                return Err("truncated frame: no chunk length".into());
            }
            let len = u16::from_be_bytes([frame[at], frame[at + 1]]) as usize;
            at += 2;
            if len < 16 {
                return Err("malformed frame: chunk shorter than the tag".into());
            }
            if frame.len() - at < len {
                return Err("truncated frame: chunk shorter than its length".into());
            }
            let n = self
                .state
                .read_message(&frame[at..at + len], &mut self.buf)
                .map_err(|e| format!("cannot decrypt: {e}"))?;
            out.extend_from_slice(&self.buf[..n]);
            at += len;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key in a directory no other test shares, so the suite can run its
    /// cases in parallel and twice over.
    fn fresh_key() -> DeviceKey {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fletch-key-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        DeviceKey::load_or_create(&dir).unwrap()
    }

    /// Two channels that have completed a real XX handshake against each
    /// other, in memory: (initiator, responder).
    fn pair() -> (Channel, Channel) {
        let phone = fresh_key();
        let host = fresh_key();
        let mut a = Handshake::new(&phone, true).unwrap();
        let mut b = Handshake::new(&host, false).unwrap();
        let e = a.write().unwrap();
        b.read(&e).unwrap();
        let ees = b.write().unwrap();
        a.read(&ees).unwrap();
        let sse = a.write().unwrap();
        b.read(&sse).unwrap();
        (a.finish().unwrap(), b.finish().unwrap())
    }

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn handshake_authenticates_both_statics() {
        let (phone, host) = pair();
        // Each side ends up holding the other's public key: that, and nothing
        // else, is the authentication.
        assert!(!phone.remote_static_base64().unwrap().is_empty());
        assert!(!host.remote_static_base64().unwrap().is_empty());
    }

    #[test]
    fn the_host_static_is_known_before_the_third_message() {
        let phone = fresh_key();
        let host = fresh_key();
        let mut a = Handshake::new(&phone, true).unwrap();
        let mut b = Handshake::new(&host, false).unwrap();
        b.read(&a.write().unwrap()).unwrap();
        a.read(&b.write().unwrap()).unwrap();
        // The initiator can check the host's identity here, and abandon the
        // handshake without ever sending message 3.
        assert_eq!(a.remote_static_base64().unwrap(), host.public_base64());
        assert!(b.remote_static_base64().is_err());
    }

    #[test]
    fn key_file_is_stable_across_loads() {
        let dir = std::env::temp_dir().join(format!("fletch-stable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = DeviceKey::load_or_create(&dir).unwrap().public_base64();
        let second = DeviceKey::load_or_create(&dir).unwrap().public_base64();
        assert_eq!(first, second);
        // base64url of 32 bytes, unpadded.
        assert_eq!(first.len(), 43);
        assert!(!first.contains('=') && !first.contains('+') && !first.contains('/'));
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_not_a_new_identity() {
        let dir = std::env::temp_dir().join(format!("fletch-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(KEY_FILE), b"not a key").unwrap();

        // No `Debug` on a private key, so no `unwrap_err`.
        let err = match DeviceKey::load_or_create(&dir) {
            Ok(_) => panic!("a corrupt key file must not load"),
            Err(e) => e,
        };
        assert!(err.contains("not a 32-byte key"), "{err}");
        assert_eq!(
            std::fs::read(dir.join(KEY_FILE)).unwrap(),
            b"not a key",
            "left in place for the user to delete deliberately"
        );
        assert!(!dir.join("device_key.tmp").exists());
    }

    #[test]
    fn roundtrips_frames_at_and_across_the_chunk_boundary() {
        let (mut phone, mut host) = pair();
        for len in [
            0usize,
            1,
            MAX_CHUNK_PLAINTEXT,
            MAX_CHUNK_PLAINTEXT + 1,
            200_000,
        ] {
            let plaintext = body(len);
            let frame = phone.encrypt(&plaintext).unwrap();
            assert_eq!(host.decrypt(&frame).unwrap(), plaintext, "len {len}");
        }
    }

    #[test]
    fn an_empty_frame_is_one_tag_only_chunk_and_zero_bytes_is_malformed() {
        let (mut phone, mut host) = pair();
        let empty = phone.encrypt(b"").unwrap();
        assert_eq!(empty.len(), 2 + 16, "u16 length plus a bare tag");
        assert_eq!(host.decrypt(&empty).unwrap(), b"");
        assert!(host.decrypt(&[]).is_err(), "a zero-byte frame is malformed");
    }

    #[test]
    fn chunks_at_the_noise_cap_and_no_finer() {
        let (mut phone, _) = pair();
        let one = phone.encrypt(&body(MAX_CHUNK_PLAINTEXT)).unwrap();
        assert_eq!(one.len(), 2 + MAX_CHUNK_PLAINTEXT + 16);
        let two = phone.encrypt(&body(MAX_CHUNK_PLAINTEXT + 1)).unwrap();
        assert_eq!(two.len(), (2 + MAX_CHUNK_PLAINTEXT + 16) + (2 + 1 + 16));
        // The header is the ciphertext length, big-endian.
        let header = u16::from_be_bytes([two[0], two[1]]) as usize;
        assert_eq!(header, MAX_CHUNK_PLAINTEXT + 16);
    }

    #[test]
    fn rejects_a_truncated_frame() {
        let (mut phone, mut host) = pair();
        let frame = phone.encrypt(b"{\"id\":\"1\"}").unwrap();
        assert!(host.decrypt(&frame[..1]).unwrap_err().contains("truncated"));
        assert!(host
            .decrypt(&frame[..frame.len() - 1])
            .unwrap_err()
            .contains("truncated"));
    }

    #[test]
    fn rejects_a_tampered_frame() {
        let (mut phone, mut host) = pair();
        let mut frame = phone.encrypt(b"{\"id\":\"1\"}").unwrap();
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        assert!(host.decrypt(&frame).unwrap_err().contains("cannot decrypt"));
    }

    #[test]
    fn rejects_a_frame_from_a_stranger() {
        let (mut phone, _) = pair();
        let (_, mut other) = pair();
        let frame = phone.encrypt(b"{\"id\":\"1\"}").unwrap();
        assert!(other.decrypt(&frame).is_err());
    }
}
