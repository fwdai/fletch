//! The remote connection's secure channel: a Noise handshake over the plain
//! WebSocket, then nothing but encrypted binary frames.
//!
//! The transport is a carrier only. Confidentiality, integrity and both ends'
//! identities come from here, which is what lets the same frames travel
//! unchanged through the relay a later contract revision adds — the relay sees
//! ciphertext, exactly as a passive observer of the LAN does.
//!
//! Everything in this file is fixed by `docs/remote-protocol.md` → "Secure
//! channel": the pattern string, the prologue, empty handshake payloads, the
//! phone as initiator, and the chunked frame format.

use std::path::Path;

use base64::Engine;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use snow::{Builder, HandshakeState, TransportState};
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};

use crate::error::{Error, Result};

/// The one pattern this protocol speaks. `XX` because neither side knows the
/// other's static key before pairing, and both must prove one.
const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// Hashed into the handshake by both ends, so a handshake cannot be replayed
/// into a different version of this protocol.
const PROLOGUE: &[u8] = b"fletch-remote-v2";

/// X25519 key length, for both halves of the pair.
const KEY_LEN: usize = 32;
/// Noise's per-message cap. A chunk's ciphertext can never exceed it, which is
/// why a `u16` length prefix is enough for the frame format.
const MAX_NOISE_MESSAGE: usize = 65_535;
/// ChaChaPoly's authentication tag, the fixed overhead of every chunk.
const TAG_LEN: usize = 16;
/// Plaintext bytes per chunk: Noise's cap minus the tag (protocol doc).
pub(super) const MAX_CHUNK_PLAINTEXT: usize = MAX_NOISE_MESSAGE - TAG_LEN;

/// The file inside the remote dir holding the host's 32-byte private key.
const HOST_KEY_FILE: &str = "host_key";

/// A public key as the protocol writes it: base64url, no padding, 43 chars.
pub(super) fn encode_key(key: &[u8; KEY_LEN]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key)
}

/// This host's static identity. The public half is the **host ID**: what the
/// pairing link carries, what the phone pins, and what the relay will route on.
///
/// Generated on first use and kept for the life of the install. Deleting the
/// file regenerates it, after which every paired phone has to pair again —
/// which is also the recovery path for a stolen key.
pub struct HostKey {
    private: [u8; KEY_LEN],
    public: [u8; KEY_LEN],
}

impl HostKey {
    /// Load `<dir>/host_key`, creating it (0600, atomic write) on first use.
    ///
    /// A file that exists but is not a 32-byte key is an error rather than a
    /// silent regeneration: regenerating would invalidate every pairing, so the
    /// user should see the problem and delete the file deliberately.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(HOST_KEY_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let private: [u8; KEY_LEN] = bytes.try_into().map_err(|_| {
                    Error::Other(format!(
                        "The remote host key at {} is not a 32-byte key. Delete it to generate a \
                         new one; every paired device will then have to pair again.",
                        path.display()
                    ))
                })?;
                Self::from_private(private)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let keypair = generate_keypair()?;
                let private: [u8; KEY_LEN] = keypair.private.try_into().map_err(|_| {
                    Error::Other("remote: generated host key is not 32 bytes".to_string())
                })?;
                std::fs::create_dir_all(dir)?;
                write_private(&path, &private)?;
                Self::from_private(private)
            }
            Err(e) => Err(Error::Other(format!(
                "The remote host key at {} could not be read: {e}",
                path.display()
            ))),
        }
    }

    /// The host ID: the public key, base64url without padding (43 characters).
    pub fn host_id(&self) -> String {
        encode_key(&self.public)
    }

    /// The raw public key — the host ID before it is base64url'd. The relay's
    /// host-link challenge hashes these bytes, not the string.
    pub(super) fn public_bytes(&self) -> &[u8; KEY_LEN] {
        &self.public
    }

    /// `X25519(host private, their_public)`: the shared secret the relay's host
    /// link challenge is answered with (`docs/remote-protocol.md` → "Relay").
    ///
    /// Goes through the same DH implementation the handshake and
    /// `public_from_private` use, so the relay proof needs no second crypto
    /// dependency and cannot disagree with the identity it is proving.
    pub(super) fn diffie_hellman(&self, their_public: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN]> {
        let mut dh = curve25519()?;
        dh.set(&self.private);
        let mut shared = [0u8; KEY_LEN];
        dh.dh(their_public, &mut shared)
            .map_err(|e| noise_error("relay challenge dh", e))?;
        Ok(shared)
    }

    /// A responder handshake state carrying this identity.
    pub(super) fn responder(&self) -> Result<HandshakeState> {
        noise_builder()?
            .local_private_key(&self.private)
            .and_then(Builder::build_responder)
            .map_err(|e| noise_error("host handshake state", e))
    }

    /// An identity from raw private bytes, without touching the disk. The relay
    /// tests build both ends' keys this way.
    pub(super) fn from_private(private: [u8; KEY_LEN]) -> Result<Self> {
        let public = public_from_private(&private)?;
        Ok(Self { private, public })
    }
}

/// Write a private key the way `DeviceStore::persist` writes the registry: a
/// temp file chmod'ed 0600 and renamed over the target, so a crash mid-write
/// cannot leave a half key behind and the key is never briefly world-readable.
fn write_private(path: &Path, private: &[u8; KEY_LEN]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, private)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The X25519 public key for a stored private key. Only the private half is on
/// disk (the doc fixes the file at 32 bytes), so the public half is derived at
/// load through the same DH implementation the handshake uses.
fn public_from_private(private: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN]> {
    let mut dh = curve25519()?;
    dh.set(private);
    dh.pubkey()
        .try_into()
        .map_err(|_| Error::Other("remote: X25519 public key is not 32 bytes".to_string()))
}

/// The X25519 primitive `snow` would use inside the handshake, on its own — for
/// the two places that need a bare DH: deriving the public key from the stored
/// private one, and answering the relay's challenge.
fn curve25519() -> Result<Box<dyn snow::types::Dh>> {
    use snow::params::DHChoice;
    use snow::resolvers::{CryptoResolver, DefaultResolver};

    DefaultResolver
        .resolve_dh(&DHChoice::Curve25519)
        .ok_or_else(|| Error::Other("remote: no X25519 implementation".to_string()))
}

/// A fresh static keypair. Used for the host key, and by the tests for a device.
pub(super) fn generate_keypair() -> Result<snow::Keypair> {
    noise_builder()?
        .generate_keypair()
        .map_err(|e| noise_error("keypair generation", e))
}

/// Parameters and prologue in one place, so initiator and responder cannot
/// drift apart.
fn noise_builder() -> Result<Builder<'static>> {
    let params = NOISE_PARAMS
        .parse()
        .map_err(|e| Error::Other(format!("remote: bad noise params: {e}")))?;
    Builder::new(params)
        .prologue(PROLOGUE)
        .map_err(|e| noise_error("prologue", e))
}

/// An initiator handshake state — the phone's side. Only the tests need it on
/// this end; production initiates from `mobile/`.
#[cfg(test)]
pub(super) fn initiator(private: &[u8]) -> Result<HandshakeState> {
    noise_builder()?
        .local_private_key(private)
        .and_then(Builder::build_initiator)
        .map_err(|e| noise_error("client handshake state", e))
}

fn noise_error(context: &str, e: snow::Error) -> Error {
    Error::Other(format!("remote: {context}: {e}"))
}

/// A live connection's transport state: the keys the handshake produced, plus
/// the two nonce counters that make every frame unique.
///
/// Both directions live in one object because `snow` keeps them in one object.
/// Encryption and decryption each happen in exactly one place per connection
/// (the writer task and the reader loop), which is what keeps the counters in
/// step with the bytes on the socket.
pub(super) struct SecureChannel {
    transport: TransportState,
}

impl SecureChannel {
    pub(super) fn new(transport: TransportState) -> Self {
        Self { transport }
    }

    /// One protocol frame as the bytes of one WebSocket binary message:
    /// repeated `u16` big-endian ciphertext length followed by that ciphertext,
    /// each chunk covering at most `MAX_CHUNK_PLAINTEXT` bytes of `plaintext`.
    ///
    /// Empty plaintext is one chunk holding just the tag, so a frame is never
    /// zero bytes and the two ends' nonces advance together.
    pub(super) fn encrypt_frame(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(plaintext.len() + TAG_LEN + 2);
        let mut buf = [0u8; MAX_NOISE_MESSAGE];
        // `loop`, not `chunks()`: empty plaintext still owes one tag-only chunk.
        let mut rest = plaintext;
        loop {
            let (chunk, tail) = rest.split_at(rest.len().min(MAX_CHUNK_PLAINTEXT));
            let n = self
                .transport
                .write_message(chunk, &mut buf)
                .map_err(|e| noise_error("encrypt", e))?;
            let len = u16::try_from(n)
                .map_err(|_| Error::Other("remote: chunk over the noise cap".to_string()))?;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&buf[..n]);
            rest = tail;
            if rest.is_empty() {
                return Ok(out);
            }
        }
    }

    /// The inverse: decrypt the chunks in order and concatenate the plaintext.
    /// Anything malformed — a truncated header or body, a chunk too short to
    /// hold a tag, a tag that does not verify — is an error, and the caller
    /// closes the connection rather than trying to resynchronize.
    pub(super) fn decrypt_frame(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.is_empty() {
            return Err(Error::Other("remote: empty secure frame".to_string()));
        }
        let mut out = Vec::with_capacity(frame.len());
        let mut buf = [0u8; MAX_NOISE_MESSAGE];
        let mut rest = frame;
        while !rest.is_empty() {
            if rest.len() < 2 {
                return Err(malformed("truncated chunk header"));
            }
            let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
            if len < TAG_LEN {
                return Err(malformed("chunk shorter than the tag"));
            }
            if rest.len() < 2 + len {
                return Err(malformed("truncated chunk body"));
            }
            let n = self
                .transport
                .read_message(&rest[2..2 + len], &mut buf)
                .map_err(|e| noise_error("decrypt", e))?;
            out.extend_from_slice(&buf[..n]);
            rest = &rest[2 + len..];
        }
        Ok(out)
    }
}

fn malformed(what: &str) -> Error {
    Error::Other(format!("remote: malformed secure frame: {what}"))
}

/// Drive the responder's half of the handshake over `ws`: read `-> e`, write
/// `<- e, ee, s, es`, read `-> s, se`. Three WebSocket **binary** messages with
/// empty Noise payloads.
///
/// Returns the transport state and the initiator's static public key, which is
/// the only device identity the host ever trusts — the `pair`/`hello` frame
/// never carries one. Any non-binary message, or any Noise error, is a
/// handshake failure (close code 4001).
pub(super) async fn respond<S>(ws: &mut S, host: &HostKey) -> Result<(SecureChannel, [u8; KEY_LEN])>
where
    S: Stream<Item = std::result::Result<Message, WsError>>
        + Sink<Message, Error = WsError>
        + Unpin,
{
    let mut handshake = host.responder()?;
    let mut buf = [0u8; MAX_NOISE_MESSAGE];

    let first = next_binary(ws).await?;
    handshake
        .read_message(&first, &mut buf)
        .map_err(|e| noise_error("handshake message 1", e))?;

    let n = handshake
        .write_message(&[], &mut buf)
        .map_err(|e| noise_error("handshake message 2", e))?;
    ws.send(Message::Binary(Bytes::copy_from_slice(&buf[..n])))
        .await
        .map_err(|e| Error::Other(format!("remote: handshake write failed: {e}")))?;

    let third = next_binary(ws).await?;
    handshake
        .read_message(&third, &mut buf)
        .map_err(|e| noise_error("handshake message 3", e))?;

    let remote_static: [u8; KEY_LEN] = handshake
        .get_remote_static()
        .ok_or_else(|| Error::Other("remote: handshake proved no device key".to_string()))?
        .try_into()
        .map_err(|_| Error::Other("remote: device key is not 32 bytes".to_string()))?;
    let transport = handshake
        .into_transport_mode()
        .map_err(|e| noise_error("transport mode", e))?;
    Ok((SecureChannel::new(transport), remote_static))
}

/// The next WebSocket message, which must be binary. A text frame at any point
/// is a protocol violation by contract, and so is a socket that ends here.
async fn next_binary<S>(ws: &mut S) -> Result<Bytes>
where
    S: Stream<Item = std::result::Result<Message, WsError>> + Unpin,
{
    loop {
        let msg = ws
            .next()
            .await
            .ok_or_else(|| Error::Other("remote: socket closed during the handshake".to_string()))?
            .map_err(|e| Error::Other(format!("remote: handshake read failed: {e}")))?;
        match msg {
            Message::Binary(bytes) => return Ok(bytes),
            // Keepalives are the transport's business and are answered by
            // tungstenite; they can legitimately interleave the handshake.
            Message::Ping(_) | Message::Pong(_) => continue,
            other => {
                return Err(Error::Other(format!(
                    "remote: expected a binary handshake message, got {}",
                    describe(&other)
                )))
            }
        }
    }
}

fn describe(msg: &Message) -> &'static str {
    match msg {
        Message::Text(_) => "a text frame",
        Message::Close(_) => "a close frame",
        _ => "a control frame",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two channels that have completed a full XX handshake in memory: the
    /// codec cannot be exercised without real transport states, since the tag
    /// and the nonce counter are what it has to get right.
    fn pair() -> (SecureChannel, SecureChannel) {
        let client_key = generate_keypair().unwrap();
        let host_key = generate_keypair().unwrap();
        let mut client = initiator(&client_key.private).unwrap();
        let mut host = super::HostKey::from_private(host_key.private.try_into().unwrap())
            .unwrap()
            .responder()
            .unwrap();

        let mut buf = [0u8; MAX_NOISE_MESSAGE];
        let mut read = [0u8; MAX_NOISE_MESSAGE];
        let n = client.write_message(&[], &mut buf).unwrap();
        host.read_message(&buf[..n], &mut read).unwrap();
        let n = host.write_message(&[], &mut buf).unwrap();
        client.read_message(&buf[..n], &mut read).unwrap();
        let n = client.write_message(&[], &mut buf).unwrap();
        host.read_message(&buf[..n], &mut read).unwrap();

        assert_eq!(
            host.get_remote_static().unwrap(),
            &client_key.public[..],
            "the responder learns the initiator's static key from message 3"
        );
        (
            SecureChannel::new(client.into_transport_mode().unwrap()),
            SecureChannel::new(host.into_transport_mode().unwrap()),
        )
    }

    #[test]
    fn chunk_codec_roundtrips_every_boundary_size() {
        for len in [
            0usize,
            1,
            MAX_CHUNK_PLAINTEXT,
            MAX_CHUNK_PLAINTEXT + 1,
            200_000,
        ] {
            let (mut client, mut host) = pair();
            let plaintext: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let frame = client.encrypt_frame(&plaintext).unwrap();

            let chunks = len.div_ceil(MAX_CHUNK_PLAINTEXT).max(1);
            assert_eq!(
                frame.len(),
                plaintext.len() + chunks * (2 + TAG_LEN),
                "{len} bytes should travel as {chunks} chunk(s)"
            );
            assert_eq!(host.decrypt_frame(&frame).unwrap(), plaintext, "len {len}");
        }
    }

    #[test]
    fn a_stream_of_frames_stays_in_step() {
        let (mut client, mut host) = pair();
        for i in 0..4u8 {
            let frame = client.encrypt_frame(&[i; 3]).unwrap();
            assert_eq!(host.decrypt_frame(&frame).unwrap(), vec![i; 3]);
        }
    }

    #[test]
    fn truncated_frames_are_rejected() {
        let (mut client, mut host) = pair();
        let frame = client.encrypt_frame(b"{\"op\":\"hello\"}").unwrap();
        for cut in [0, 1, 2, 3, frame.len() - 1] {
            assert!(
                host.decrypt_frame(&frame[..cut]).is_err(),
                "a frame cut to {cut} bytes must not decrypt"
            );
        }
    }

    #[test]
    fn a_tampered_tag_is_rejected() {
        let (mut client, mut host) = pair();
        let mut frame = client.encrypt_frame(b"{\"op\":\"hello\"}").unwrap();
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        assert!(host.decrypt_frame(&frame).is_err());
    }

    #[test]
    fn a_tampered_ciphertext_is_rejected() {
        let (mut client, mut host) = pair();
        let mut frame = client.encrypt_frame(b"{\"op\":\"hello\"}").unwrap();
        frame[2] ^= 0x01;
        assert!(host.decrypt_frame(&frame).is_err());
    }

    #[test]
    fn a_chunk_too_short_for_a_tag_is_rejected() {
        let (_, mut host) = pair();
        assert!(host.decrypt_frame(&[0, 4, 1, 2, 3, 4]).is_err());
    }

    #[test]
    fn host_key_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = HostKey::load(dir.path()).unwrap();
        let second = HostKey::load(dir.path()).unwrap();
        assert_eq!(first.host_id(), second.host_id());
        assert_eq!(first.host_id().len(), 43, "43 base64url chars, no padding");
        assert_eq!(
            std::fs::read(dir.path().join(HOST_KEY_FILE)).unwrap().len(),
            KEY_LEN
        );
    }

    #[test]
    fn a_corrupt_host_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(HOST_KEY_FILE), b"nope").unwrap();
        assert!(HostKey::load(dir.path()).is_err());
    }
}
