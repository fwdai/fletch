//! The secure channel: a Noise `XX` handshake over a plain WebSocket, then
//! nothing but encrypted binary frames.
//!
//! The transport is a carrier only. Confidentiality, integrity and both ends'
//! identities come from here, which is what lets the same frames travel
//! unchanged through the relay — the relay sees ciphertext, exactly as a
//! passive observer of the LAN does.
//!
//! Everything in this module is fixed by `docs/remote-protocol.md` → "Secure
//! channel": the pattern string, the prologue, empty handshake payloads, the
//! client as initiator, and the chunked frame format. Both roles live here:
//! [`respond`] is the host's half, [`initiate`] the client's, and both are the
//! same three messages driven by the same [`Handshake`].

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use snow::{Builder, HandshakeState, TransportState};
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};

use crate::keys::{encode_key, StaticKey, KEY_LEN};
use crate::Result;

/// The one pattern this protocol speaks. `XX` because neither side knows the
/// other's static key before pairing, and both must prove one.
const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// Hashed into the handshake by both ends, so a handshake cannot be replayed
/// into a different version of this protocol.
const PROLOGUE: &[u8] = b"fletch-remote-v2";

/// Noise's per-message cap. A chunk's ciphertext can never exceed it, which is
/// why a `u16` length prefix is enough for the frame format.
const MAX_NOISE_MESSAGE: usize = 65_535;
/// ChaChaPoly's authentication tag, the fixed overhead of every chunk.
const TAG_LEN: usize = 16;
/// Plaintext bytes per chunk: Noise's cap minus the tag (protocol doc).
pub const MAX_CHUNK_PLAINTEXT: usize = MAX_NOISE_MESSAGE - TAG_LEN;

/// Marker a client matches on to tell a pinned-key mismatch (which no retry can
/// fix) from a transport failure (which one might).
pub const HOST_KEY_MISMATCH: &str = "host-key-mismatch";

/// Parameters and prologue in one place, so initiator and responder cannot
/// drift apart.
pub(crate) fn builder() -> Result<Builder<'static>> {
    let params = PATTERN
        .parse()
        .map_err(|e| format!("bad noise params: {e}"))?;
    Builder::new(params)
        .prologue(PROLOGUE)
        .map_err(|e| format!("cannot start the handshake: {e}"))
}

/// One side of the `XX` handshake. The client is the initiator: `write`,
/// `read`, `write`, then `finish`. The host is the responder: `read`, `write`,
/// `read`, then `finish`.
pub struct Handshake {
    state: HandshakeState,
    buf: Vec<u8>,
}

impl Handshake {
    pub fn new(key: &StaticKey, initiator: bool) -> Result<Self> {
        let builder = builder()?
            .local_private_key(key.private_bytes())
            .map_err(|e| format!("cannot start the handshake: {e}"))?;
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|e| format!("cannot start the handshake: {e}"))?;
        Ok(Self {
            state,
            buf: vec![0u8; MAX_NOISE_MESSAGE],
        })
    }

    /// The next outgoing handshake message. Payloads are empty by contract.
    pub fn write(&mut self) -> Result<Vec<u8>> {
        let n = self
            .state
            .write_message(&[], &mut self.buf)
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(self.buf[..n].to_vec())
    }

    pub fn read(&mut self, message: &[u8]) -> Result<()> {
        let mut out = vec![0u8; MAX_NOISE_MESSAGE];
        self.state
            .read_message(message, &mut out)
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(())
    }

    /// The peer's static public key, base64url, once a message carrying it has
    /// been read. For `XX` the initiator has it after message 2 — before it has
    /// revealed its own identity in message 3.
    pub fn remote_static_base64(&self) -> Result<String> {
        remote_static_base64(self.state.get_remote_static())
    }

    /// The peer's static public key as raw bytes. The host's only notion of who
    /// is on the other end; a `pair`/`hello` frame never carries one.
    pub fn remote_static_bytes(&self) -> Result<[u8; KEY_LEN]> {
        remote_static_bytes(self.state.get_remote_static())
    }

    pub fn finish(self) -> Result<Channel> {
        let state = self
            .state
            .into_transport_mode()
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(Channel {
            state,
            buf: self.buf,
        })
    }
}

/// A live connection: one JSON document per frame, encrypted as repeated
/// `u16 big-endian ciphertext length || ciphertext` chunks.
///
/// Both directions live in one object because `snow` keeps them in one object.
/// Encryption and decryption must each happen in exactly one place per
/// connection, which is what keeps the nonce counters in step with the bytes on
/// the socket.
pub struct Channel {
    state: TransportState,
    buf: Vec<u8>,
}

impl Channel {
    /// The peer's static public key, base64url — the host's identity, seen from
    /// a client.
    pub fn remote_static_base64(&self) -> Result<String> {
        remote_static_base64(self.state.get_remote_static())
    }

    /// One protocol frame as the bytes of one WebSocket binary message:
    /// repeated `u16` big-endian ciphertext length followed by that ciphertext,
    /// each chunk covering at most [`MAX_CHUNK_PLAINTEXT`] bytes of `plaintext`.
    ///
    /// Empty plaintext is one chunk holding just the tag, so a frame is never
    /// zero bytes and the two ends' nonces advance together — hence `loop`, not
    /// `chunks()`, which yields nothing for empty input.
    pub fn encrypt_frame(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(plaintext.len() + TAG_LEN + 2);
        let mut rest = plaintext;
        loop {
            let (chunk, tail) = rest.split_at(rest.len().min(MAX_CHUNK_PLAINTEXT));
            let n = self
                .state
                .write_message(chunk, &mut self.buf)
                .map_err(|e| format!("cannot encrypt: {e}"))?;
            let len = u16::try_from(n).map_err(|_| "chunk over the noise cap".to_string())?;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&self.buf[..n]);
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
    pub fn decrypt_frame(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.is_empty() {
            return Err("malformed frame: zero bytes".to_string());
        }
        let mut out = Vec::with_capacity(frame.len());
        let mut rest = frame;
        while !rest.is_empty() {
            if rest.len() < 2 {
                return Err("truncated frame: no chunk length".to_string());
            }
            let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
            if len < TAG_LEN {
                return Err("malformed frame: chunk shorter than the tag".to_string());
            }
            if rest.len() - 2 < len {
                return Err("truncated frame: chunk shorter than its length".to_string());
            }
            let n = self
                .state
                .read_message(&rest[2..2 + len], &mut self.buf)
                .map_err(|e| format!("cannot decrypt: {e}"))?;
            out.extend_from_slice(&self.buf[..n]);
            rest = &rest[2 + len..];
        }
        Ok(out)
    }
}

fn remote_static_base64(key: Option<&[u8]>) -> Result<String> {
    key.map(encode_key)
        .ok_or_else(|| "the peer presented no identity key".to_string())
}

fn remote_static_bytes(key: Option<&[u8]>) -> Result<[u8; KEY_LEN]> {
    key.ok_or_else(|| "the peer presented no identity key".to_string())?
        .try_into()
        .map_err(|_| "the peer's identity key is not 32 bytes".to_string())
}

/// Drive the responder's half of the handshake over `ws`: read `-> e`, write
/// `<- e, ee, s, es`, read `-> s, se`. Three WebSocket **binary** messages with
/// empty Noise payloads.
///
/// Returns the channel and the initiator's static public key, which is the only
/// device identity a host ever trusts. Any non-binary message, or any Noise
/// error, is a handshake failure (close code 4001).
pub async fn respond<S>(ws: &mut S, host: &StaticKey) -> Result<(Channel, [u8; KEY_LEN])>
where
    S: Stream<Item = std::result::Result<Message, WsError>>
        + Sink<Message, Error = WsError>
        + Unpin,
{
    let mut handshake = Handshake::new(host, false)?;
    let first = next_binary(ws).await?;
    handshake.read(&first)?;
    send(ws, handshake.write()?).await?;
    let third = next_binary(ws).await?;
    handshake.read(&third)?;
    let remote_static = handshake.remote_static_bytes()?;
    Ok((handshake.finish()?, remote_static))
}

/// Drive the initiator's half over `ws`: write `-> e`, read `<- e, ee, s, es`,
/// write `-> s, se`.
///
/// `expected` is the host key the caller insists on. It is checked as soon as
/// message 2 delivers it, which is before message 3 reveals this device's own
/// identity — an impostor learns nothing. A mismatch is reported with the
/// [`HOST_KEY_MISMATCH`] marker, because no retry can fix it.
pub async fn initiate<S>(ws: &mut S, key: &StaticKey, expected: Option<&str>) -> Result<Channel>
where
    S: Stream<Item = std::result::Result<Message, WsError>>
        + Sink<Message, Error = WsError>
        + Unpin,
{
    let mut handshake = Handshake::new(key, true)?;
    send(ws, handshake.write()?).await?;
    let second = next_binary(ws).await?;
    handshake.read(&second)?;
    let seen = handshake.remote_static_base64()?;
    if let Some(expected) = expected {
        if expected != seen {
            return Err(format!(
                "{HOST_KEY_MISMATCH}: expected {expected}, host presented {seen}"
            ));
        }
    }
    send(ws, handshake.write()?).await?;
    handshake.finish()
}

async fn send<S>(ws: &mut S, bytes: Vec<u8>) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    ws.send(Message::Binary(Bytes::from(bytes)))
        .await
        .map_err(|e| format!("handshake failed: {e}"))
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
            .ok_or_else(|| "the socket closed during the handshake".to_string())?
            .map_err(|e| format!("handshake failed: {e}"))?;
        match msg {
            Message::Binary(bytes) => return Ok(bytes),
            // Keepalives are the transport's business and are answered by
            // tungstenite; they can legitimately interleave the handshake.
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Text(_) => return Err("the peer sent a cleartext frame".to_string()),
            Message::Close(_) => return Err("the peer closed during the handshake".to_string()),
            _ => return Err("expected a binary handshake message, got a control frame".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_key() -> StaticKey {
        StaticKey::generate().unwrap()
    }

    /// Two channels that have completed a real `XX` handshake against each
    /// other, in memory: (initiator, responder). The codec cannot be exercised
    /// without real transport states, since the tag and the nonce counter are
    /// what it has to get right.
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

        assert_eq!(
            b.remote_static_bytes().unwrap(),
            *phone.public_bytes(),
            "the responder learns the initiator's static key from message 3"
        );
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
    fn chunk_codec_roundtrips_every_boundary_size() {
        for len in [
            0usize,
            1,
            MAX_CHUNK_PLAINTEXT,
            MAX_CHUNK_PLAINTEXT + 1,
            200_000,
        ] {
            let (mut client, mut host) = pair();
            let plaintext = body(len);
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
    fn an_empty_frame_is_one_tag_only_chunk_and_zero_bytes_is_malformed() {
        let (mut phone, mut host) = pair();
        let empty = phone.encrypt_frame(b"").unwrap();
        assert_eq!(empty.len(), 2 + TAG_LEN, "u16 length plus a bare tag");
        assert_eq!(host.decrypt_frame(&empty).unwrap(), b"");
        assert!(
            host.decrypt_frame(&[]).is_err(),
            "a zero-byte frame is malformed"
        );
    }

    #[test]
    fn chunks_at_the_noise_cap_and_no_finer() {
        let (mut phone, _) = pair();
        let one = phone.encrypt_frame(&body(MAX_CHUNK_PLAINTEXT)).unwrap();
        assert_eq!(one.len(), 2 + MAX_CHUNK_PLAINTEXT + TAG_LEN);
        let two = phone.encrypt_frame(&body(MAX_CHUNK_PLAINTEXT + 1)).unwrap();
        assert_eq!(
            two.len(),
            (2 + MAX_CHUNK_PLAINTEXT + TAG_LEN) + (2 + 1 + TAG_LEN)
        );
        // The header is the ciphertext length, big-endian.
        let header = usize::from(u16::from_be_bytes([two[0], two[1]]));
        assert_eq!(header, MAX_CHUNK_PLAINTEXT + TAG_LEN);
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
        assert!(host
            .decrypt_frame(&frame[..1])
            .unwrap_err()
            .contains("truncated"));
        assert!(host
            .decrypt_frame(&frame[..frame.len() - 1])
            .unwrap_err()
            .contains("truncated"));
    }

    #[test]
    fn a_tampered_tag_is_rejected() {
        let (mut client, mut host) = pair();
        let mut frame = client.encrypt_frame(b"{\"op\":\"hello\"}").unwrap();
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        assert!(host
            .decrypt_frame(&frame)
            .unwrap_err()
            .contains("cannot decrypt"));
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
    fn rejects_a_frame_from_a_stranger() {
        let (mut phone, _) = pair();
        let (_, mut other) = pair();
        let frame = phone.encrypt_frame(b"{\"id\":\"1\"}").unwrap();
        assert!(other.decrypt_frame(&frame).is_err());
    }
}
