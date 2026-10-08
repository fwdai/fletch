//! The secure channel: a Noise `XX` handshake over a plain WebSocket, then
//! nothing but encrypted binary frames.
//!
//! The transport is a carrier only. Confidentiality, integrity and both ends'
//! identities come from here, which is what lets the same frames travel
//! unchanged through the relay — the relay sees ciphertext, exactly as a
//! passive observer of the LAN does.
//!
//! Everything in this module is fixed by `docs/remote-protocol.md` → "Secure
//! channel": the pattern string, the prologue, the capability byte the
//! handshake payloads carry, the client as initiator, and the chunked and
//! fragmented frame formats. Both roles live here:
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

/// Capability bit: this end reassembles fragmented frames. In force on a
/// connection only when both ends advertised it (protocol doc, "Handshake
/// payloads carry capabilities").
pub const CAP_FRAGMENTS: u8 = 0x01;
/// What this build advertises in its handshake payloads.
pub const CAPABILITIES: u8 = CAP_FRAGMENTS;

/// Frame bytes per fragment message. Small enough that one device's large
/// answer interleaves with every other device's traffic on the shared relay
/// link, large enough that the per-message overhead is noise.
pub const FRAGMENT_PLAINTEXT: usize = 256 * 1024;
/// Largest frame a run of fragments may reassemble to, and so the largest a
/// fragmenting end will send. It is also what ends a run that never ends: the
/// receiver gives up once the run passes it, with no timer needed. A channel
/// starts at this limit and may be held to less (see
/// [`Channel::set_frame_limit`]), never more.
pub const MAX_FRAME_PLAINTEXT: usize = 64 * 1024 * 1024;
/// Largest frame that still fits one message under the 4 MiB message cap the
/// host and the relay enforce. The margin covers the chunk overhead (18 bytes
/// per 64 KiB) and the relay's mux header, with room to spare.
pub const MAX_MESSAGE_PLAINTEXT: usize = 4 * 1024 * 1024 - 64 * 1024;

/// Opens a fragment message: a zero-length chunk, which can never start an
/// ordinary message (a chunk shorter than its tag is malformed), so the two
/// forms cannot be mistaken for each other.
const FRAGMENT_MARKER: [u8; 2] = [0, 0];
/// A fragment's flag, the first byte of its plaintext. Inside the ciphertext
/// on purpose: anyone in the path who could flip it could cut a frame short.
const MORE: u8 = 0x01;
const LAST: u8 = 0x00;

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
    /// What this end advertises, and what the peer did. The connection gets
    /// the intersection.
    capabilities: u8,
    peer_capabilities: u8,
    /// Handshake messages written or read so far. Message 1 is the one sent
    /// before any key exists, which is why it carries no capabilities.
    messages: u8,
}

impl Handshake {
    /// One side of the handshake, advertising everything this build supports.
    pub fn new(key: &StaticKey, initiator: bool) -> Result<Self> {
        Self::with_capabilities(key, initiator, CAPABILITIES)
    }

    /// [`Handshake::new`] advertising `capabilities` instead. `0` sends the
    /// empty payloads of an end from before capabilities existed, which is
    /// how a test plays an old peer.
    pub fn with_capabilities(key: &StaticKey, initiator: bool, capabilities: u8) -> Result<Self> {
        let builder = builder()?
            .local_private_key(key.private_bytes())
            .map_err(|e| format!("cannot start the handshake: {e}"))?;
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|e| format!("cannot start the handshake: {e}"))?;
        Ok(Self::from_state(state, capabilities))
    }

    fn from_state(state: HandshakeState, capabilities: u8) -> Self {
        Self {
            state,
            buf: vec![0u8; MAX_NOISE_MESSAGE],
            capabilities,
            peer_capabilities: 0,
            messages: 0,
        }
    }

    /// The next outgoing handshake message. Messages 2 and 3 carry this end's
    /// capability byte; message 1 would carry it in the clear and
    /// unauthenticated, so it stays empty.
    pub fn write(&mut self) -> Result<Vec<u8>> {
        let capabilities = [self.capabilities];
        let payload: &[u8] = if self.messages == 0 || self.capabilities == 0 {
            &[]
        } else {
            &capabilities
        };
        let n = self
            .state
            .write_message(payload, &mut self.buf)
            .map_err(|e| format!("handshake failed: {e}"))?;
        self.messages += 1;
        Ok(self.buf[..n].to_vec())
    }

    /// Read the peer's next handshake message. Only the first payload byte of
    /// messages 2 and 3 means anything: an empty payload is an old peer with no
    /// capabilities, and trailing bytes are left for a later revision.
    pub fn read(&mut self, message: &[u8]) -> Result<()> {
        let mut out = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self
            .state
            .read_message(message, &mut out)
            .map_err(|e| format!("handshake failed: {e}"))?;
        if self.messages > 0 {
            self.peer_capabilities = out[..n].first().copied().unwrap_or(0);
        }
        self.messages += 1;
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
        let handshake_hash: [u8; 32] = self
            .state
            .get_handshake_hash()
            .try_into()
            .map_err(|_| "handshake failed: unexpected hash length".to_string())?;
        let state = self
            .state
            .into_transport_mode()
            .map_err(|e| format!("handshake failed: {e}"))?;
        Ok(Channel {
            state,
            buf: self.buf,
            fragments: self.capabilities & self.peer_capabilities & CAP_FRAGMENTS != 0,
            partial: None,
            frame_limit: MAX_FRAME_PLAINTEXT,
            handshake_hash,
        })
    }
}

/// A live connection: one JSON document per frame, encrypted as repeated
/// `u16 big-endian ciphertext length || ciphertext` chunks — one WebSocket
/// message per frame, or a run of fragment messages for a large frame when both
/// ends negotiated it.
///
/// Both directions live in one object because `snow` keeps them in one object.
/// Encryption and decryption must each happen in exactly one place per
/// connection, which is what keeps the nonce counters in step with the bytes on
/// the socket.
pub struct Channel {
    state: TransportState,
    buf: Vec<u8>,
    /// Both ends advertised [`CAP_FRAGMENTS`]. Decides both directions: this
    /// end fragments only to a peer that reassembles, and accepts fragments
    /// only from a peer it told it would.
    fragments: bool,
    /// The frame a fragment run has delivered so far, `None` between frames.
    partial: Option<Vec<u8>>,
    /// The most a fragment run may reassemble to on this channel. Ordinary
    /// messages are not held to it: the transport's 4 MiB message cap already
    /// bounds each one, and nothing accumulates across them.
    frame_limit: usize,
    /// Noise's `h` at the end of the handshake: a digest of the whole
    /// transcript, equal on both ends only if nobody sat between them. The
    /// pairing confirmation code is derived from it (`pairing::code`).
    handshake_hash: [u8; 32],
}

impl Channel {
    /// The peer's static public key, base64url — the host's identity, seen from
    /// a client.
    pub fn remote_static_base64(&self) -> Result<String> {
        remote_static_base64(self.state.get_remote_static())
    }

    /// The handshake transcript hash (see the field). Public: it binds the
    /// session, it does not unlock it.
    pub fn handshake_hash(&self) -> &[u8; 32] {
        &self.handshake_hash
    }

    /// Whether large frames travel fragmented on this connection, in either
    /// direction: both ends advertised [`CAP_FRAGMENTS`].
    pub fn fragments(&self) -> bool {
        self.fragments
    }

    /// Hold fragment runs on this channel to `bytes`, at most
    /// [`MAX_FRAME_PLAINTEXT`], which is where every channel starts.
    ///
    /// Reassembly begins as soon as the handshake is done, but the handshake
    /// only proves the peer holds *a* key, not one its owner trusts. The host
    /// keeps the limit small until the first frame has authenticated the peer,
    /// so a stranger cannot make it buffer 64 MiB per connection; a client
    /// whose peer is the pinned host has no reason to lower it.
    pub fn set_frame_limit(&mut self, bytes: usize) {
        self.frame_limit = bytes.min(MAX_FRAME_PLAINTEXT);
    }

    /// One protocol frame as the WebSocket binary messages that carry it, in
    /// order: one ordinary message, unless fragmentation was negotiated and the
    /// frame is over [`FRAGMENT_PLAINTEXT`], in which case a run of fragment
    /// messages. A frame over [`MAX_FRAME_PLAINTEXT`] is refused before
    /// anything is encrypted, since the peer would refuse the run.
    ///
    /// The whole run is encrypted in this one call, under whatever lock the
    /// caller holds on the channel, so no other frame's encryption can land
    /// between two of its fragments. The caller still has to write the
    /// messages back to back, which a connection's single writer does.
    pub fn encrypt_messages(&mut self, frame: &[u8]) -> Result<Vec<Vec<u8>>> {
        if !self.fragments || frame.len() <= FRAGMENT_PLAINTEXT {
            return Ok(vec![self.encrypt_frame(frame)?]);
        }
        if frame.len() > MAX_FRAME_PLAINTEXT {
            return Err(format!(
                "a {} byte frame is over the {MAX_FRAME_PLAINTEXT} byte cap",
                frame.len()
            ));
        }
        // Every piece but the last is exactly `FRAGMENT_PLAINTEXT`, and the
        // last is not empty since the frame is over it: the only shape
        // `decrypt_message` accepts.
        let mut pieces = frame.chunks(FRAGMENT_PLAINTEXT).peekable();
        let mut messages = Vec::with_capacity(frame.len().div_ceil(FRAGMENT_PLAINTEXT));
        let mut plaintext = Vec::with_capacity(1 + FRAGMENT_PLAINTEXT);
        while let Some(piece) = pieces.next() {
            plaintext.clear();
            plaintext.push(if pieces.peek().is_some() { MORE } else { LAST });
            plaintext.extend_from_slice(piece);
            let mut message = FRAGMENT_MARKER.to_vec();
            self.encrypt_chunks(&plaintext, &mut message)?;
            messages.push(message);
        }
        Ok(messages)
    }

    /// One WebSocket binary message in; a whole frame out once its last
    /// message has arrived, `None` while a fragment run is still open.
    ///
    /// Anything that breaks the fragment rules — a fragment on a connection
    /// that did not negotiate them, an ordinary message inside a run, a bad
    /// flag, a piece of the wrong size, a run past this channel's frame
    /// limit — is an error exactly like
    /// a frame that does not decrypt, and the caller closes the connection.
    pub fn decrypt_message(&mut self, message: &[u8]) -> Result<Option<Vec<u8>>> {
        // Without the capability a marker is just the malformed zero-length
        // chunk it always was, and `decrypt_frame` says so.
        let fragment = message
            .strip_prefix(&FRAGMENT_MARKER)
            .filter(|_| self.fragments);
        let Some(chunks) = fragment else {
            if self.partial.is_some() {
                return Err("malformed frame: an ordinary message inside a fragment run".into());
            }
            return self.decrypt_frame(message).map(Some);
        };
        let plaintext = self.decrypt_frame(chunks)?;
        let (&flag, piece) = plaintext
            .split_first()
            .ok_or_else(|| "malformed fragment: no flag".to_string())?;
        // Only the shape the encoder produces. Without it the byte limit alone
        // would not end a run: empty or tiny pieces could keep one open for
        // ever. With it a run is at most `frame_limit / FRAGMENT_PLAINTEXT`
        // messages, and no counter or timer is needed.
        let sized = match flag {
            MORE => piece.len() == FRAGMENT_PLAINTEXT,
            LAST => (1..=FRAGMENT_PLAINTEXT).contains(&piece.len()),
            other => return Err(format!("malformed fragment: flag {other:#04x}")),
        };
        if !sized {
            let which = if flag == MORE { "middle" } else { "last" };
            return Err(format!(
                "malformed fragment: a {} byte {which} piece",
                piece.len()
            ));
        }
        let mut frame = self.partial.take().unwrap_or_default();
        if frame.len() + piece.len() > self.frame_limit {
            return Err(format!(
                "malformed frame: fragments past the {} byte cap",
                self.frame_limit
            ));
        }
        frame.extend_from_slice(piece);
        if flag == MORE {
            self.partial = Some(frame);
            return Ok(None);
        }
        Ok(Some(frame))
    }

    /// One protocol frame as the bytes of one ordinary WebSocket binary
    /// message: repeated `u16` big-endian ciphertext length followed by that
    /// ciphertext, each chunk covering at most [`MAX_CHUNK_PLAINTEXT`] bytes of
    /// `plaintext`. What [`Channel::encrypt_messages`] sends for a frame it
    /// does not fragment, and the whole format toward a peer without the
    /// capability.
    pub fn encrypt_frame(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(plaintext.len() + TAG_LEN + 2);
        self.encrypt_chunks(plaintext, &mut out)?;
        Ok(out)
    }

    /// Append `plaintext` to `out` as chunks.
    ///
    /// Empty plaintext is one chunk holding just the tag, so a message is never
    /// zero bytes and the two ends' nonces advance together — hence `loop`, not
    /// `chunks()`, which yields nothing for empty input.
    fn encrypt_chunks(&mut self, plaintext: &[u8], out: &mut Vec<u8>) -> Result<()> {
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
                return Ok(());
            }
        }
    }

    /// The inverse of [`Channel::encrypt_frame`]: decrypt one ordinary
    /// message's chunks in order and concatenate the plaintext. A fragment
    /// message is not one; a caller on a connection that may fragment uses
    /// [`Channel::decrypt_message`] instead. Anything malformed — a truncated header or body, a chunk too short to
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
/// `<- e, ee, s, es`, read `-> s, se`. Three WebSocket **binary** messages, the
/// last two carrying each end's capability byte.
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
        pair_advertising(CAPABILITIES, CAPABILITIES)
    }

    /// [`pair`] with each end advertising what it is given: (initiator,
    /// responder).
    fn pair_advertising(phone_caps: u8, host_caps: u8) -> (Channel, Channel) {
        let phone = fresh_key();
        let host = fresh_key();
        let a = Handshake::with_capabilities(&phone, true, phone_caps).unwrap();
        let b = Handshake::with_capabilities(&host, false, host_caps).unwrap();
        complete(a, b, &phone)
    }

    /// [`pair_advertising`] with every key fixed, statics and ephemerals both.
    /// Two pairs built from the same keys derive the same transport keys
    /// whatever they advertise — the payloads feed the handshake hash, never
    /// the keys — so their output can be compared byte for byte.
    fn fixed_pair(phone_caps: u8, host_caps: u8) -> (Channel, Channel) {
        let phone = StaticKey::from_private([1; KEY_LEN]).unwrap();
        let host = StaticKey::from_private([2; KEY_LEN]).unwrap();
        let start = |key: &StaticKey, ephemeral: &[u8], initiator: bool, caps: u8| {
            let builder = builder()
                .unwrap()
                .local_private_key(key.private_bytes())
                .unwrap()
                .fixed_ephemeral_key_for_testing_only(ephemeral);
            let state = if initiator {
                builder.build_initiator()
            } else {
                builder.build_responder()
            };
            Handshake::from_state(state.unwrap(), caps)
        };
        let a = start(&phone, &[3; KEY_LEN], true, phone_caps);
        let b = start(&host, &[4; KEY_LEN], false, host_caps);
        complete(a, b, &phone)
    }

    /// Run the three messages between `a` (initiator) and `b`.
    fn complete(mut a: Handshake, mut b: Handshake, phone: &StaticKey) -> (Channel, Channel) {
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

    // --- fragmentation -------------------------------------------------------

    /// A fragment message as a sender that breaks the rules would build it:
    /// the marker, then `flag || piece` as chunks.
    fn raw_fragment(channel: &mut Channel, flag: u8, piece: &[u8]) -> Vec<u8> {
        let mut plaintext = vec![flag];
        plaintext.extend_from_slice(piece);
        let mut message = FRAGMENT_MARKER.to_vec();
        channel.encrypt_chunks(&plaintext, &mut message).unwrap();
        message
    }

    #[test]
    fn both_ends_must_advertise_for_fragmentation_to_be_on() {
        for (phone_caps, host_caps, on) in [
            (CAPABILITIES, CAPABILITIES, true),
            (CAPABILITIES, 0, false),
            (0, CAPABILITIES, false),
            (0, 0, false),
            // Reserved bits are ignored, not refused.
            (0xff, CAPABILITIES, true),
        ] {
            let (phone, host) = pair_advertising(phone_caps, host_caps);
            assert_eq!(phone.fragments(), on, "{phone_caps:#x} / {host_caps:#x}");
            assert_eq!(host.fragments(), on, "{phone_caps:#x} / {host_caps:#x}");
        }
    }

    #[test]
    fn a_small_frame_is_byte_identical_to_the_old_format() {
        let (mut new, _) = fixed_pair(CAPABILITIES, CAPABILITIES);
        let (mut old, _) = fixed_pair(0, 0);
        assert!(new.fragments() && !old.fragments());
        for len in [0, 1, MAX_CHUNK_PLAINTEXT + 1, FRAGMENT_PLAINTEXT] {
            let plaintext = body(len);
            assert_eq!(
                new.encrypt_messages(&plaintext).unwrap(),
                vec![old.encrypt_frame(&plaintext).unwrap()],
                "a {len} byte frame is one message, exactly as before"
            );
        }
    }

    #[test]
    fn a_large_frame_splits_into_fragments_and_reassembles() {
        for (len, fragments) in [
            (FRAGMENT_PLAINTEXT + 1, 2),
            (2 * FRAGMENT_PLAINTEXT, 2),
            (3 * FRAGMENT_PLAINTEXT + 5, 4),
        ] {
            let (mut phone, mut host) = pair();
            let frame = body(len);
            let messages = host.encrypt_messages(&frame).unwrap();
            assert_eq!(messages.len(), fragments, "{len} bytes");
            let chunks = (1 + FRAGMENT_PLAINTEXT).div_ceil(MAX_CHUNK_PLAINTEXT);
            for message in &messages {
                assert_eq!(message[..2], FRAGMENT_MARKER);
                assert!(message.len() <= 2 + 1 + FRAGMENT_PLAINTEXT + chunks * (2 + TAG_LEN));
            }
            let (last, run) = messages.split_last().unwrap();
            for message in run {
                assert_eq!(phone.decrypt_message(message).unwrap(), None);
            }
            assert_eq!(phone.decrypt_message(last).unwrap(), Some(frame));

            // The run left the channel in step: the next frame reads as usual.
            let next = host.encrypt_messages(b"{}").unwrap();
            assert_eq!(phone.decrypt_message(&next[0]).unwrap().unwrap(), b"{}");
        }
    }

    #[test]
    fn the_total_bound_is_enforced_on_both_ends() {
        let (mut phone, mut host) = pair();
        let err = host
            .encrypt_messages(&vec![0; MAX_FRAME_PLAINTEXT + 1])
            .unwrap_err();
        assert!(err.contains("cap"), "{err}");
        // Nothing was encrypted, so the channel is still usable.
        let ok = host.encrypt_messages(b"{}").unwrap();
        assert_eq!(phone.decrypt_message(&ok[0]).unwrap().unwrap(), b"{}");

        // A run that never ends is cut off at the bound rather than buffered.
        // Seeded directly: pushing 64 MiB through the cipher proves nothing
        // more about the check.
        phone.partial = Some(vec![0; MAX_FRAME_PLAINTEXT]);
        let last = raw_fragment(&mut host, LAST, b"x");
        let err = phone.decrypt_message(&last).unwrap_err();
        assert!(err.contains("cap"), "{err}");
    }

    /// Every middle piece is exactly `FRAGMENT_PLAINTEXT` and the last one 1
    /// to `FRAGMENT_PLAINTEXT`, which is what keeps a run to a bounded number
    /// of messages: no empty or tiny pieces to hold it open with.
    #[test]
    fn a_piece_of_the_wrong_size_is_malformed() {
        for (flag, len) in [
            (MORE, FRAGMENT_PLAINTEXT - 1),
            (MORE, 0),
            (MORE, FRAGMENT_PLAINTEXT + 1),
            (LAST, 0),
            (LAST, FRAGMENT_PLAINTEXT + 1),
        ] {
            let (mut phone, mut host) = pair();
            let fragment = raw_fragment(&mut host, flag, &body(len));
            let err = phone.decrypt_message(&fragment).unwrap_err();
            assert!(err.contains("piece"), "{flag:#04x} / {len}: {err}");
        }

        // The sizes at the edges of what is allowed, as one run.
        let (mut phone, mut host) = pair();
        let middle = raw_fragment(&mut host, MORE, &body(FRAGMENT_PLAINTEXT));
        let last = raw_fragment(&mut host, LAST, b"x");
        assert_eq!(phone.decrypt_message(&middle).unwrap(), None);
        let frame = phone.decrypt_message(&last).unwrap().unwrap();
        assert_eq!(frame.len(), FRAGMENT_PLAINTEXT + 1);
    }

    #[test]
    fn the_frame_limit_can_be_lowered_and_raised_but_not_past_the_cap() {
        let frame = body(2 * FRAGMENT_PLAINTEXT);
        let (mut phone, mut host) = pair();
        assert_eq!(host.frame_limit, MAX_FRAME_PLAINTEXT, "the default");

        host.set_frame_limit(64 * 1024);
        // An ordinary message is not a run, and the message cap bounds it.
        let ordinary = phone.encrypt_frame(&body(FRAGMENT_PLAINTEXT)).unwrap();
        assert_eq!(
            host.decrypt_message(&ordinary).unwrap().unwrap().len(),
            FRAGMENT_PLAINTEXT
        );
        // A run is refused on its first fragment, before anything piles up.
        let messages = phone.encrypt_messages(&frame).unwrap();
        let err = host.decrypt_message(&messages[0]).unwrap_err();
        assert!(err.contains("65536 byte cap"), "{err}");

        let (mut phone, mut host) = pair();
        host.set_frame_limit(64 * 1024);
        host.set_frame_limit(usize::MAX);
        assert_eq!(host.frame_limit, MAX_FRAME_PLAINTEXT, "the ceiling");
        let messages = phone.encrypt_messages(&frame).unwrap();
        let (last, run) = messages.split_last().unwrap();
        for message in run {
            assert_eq!(host.decrypt_message(message).unwrap(), None);
        }
        assert_eq!(host.decrypt_message(last).unwrap(), Some(frame));
    }

    #[test]
    fn a_peer_without_the_capability_never_receives_fragments() {
        for (phone_caps, host_caps) in [(0, CAPABILITIES), (CAPABILITIES, 0)] {
            let (mut phone, mut host) = pair_advertising(phone_caps, host_caps);
            let frame = body(3 * FRAGMENT_PLAINTEXT);
            let messages = host.encrypt_messages(&frame).unwrap();
            assert_eq!(messages.len(), 1, "one ordinary message, as before");
            // Today's decoder, which knows nothing of fragments, reads it.
            assert_eq!(phone.decrypt_frame(&messages[0]).unwrap(), frame);
        }
    }

    #[test]
    fn a_fragment_on_a_connection_without_the_capability_is_malformed() {
        let (mut phone, mut host) = pair_advertising(0, CAPABILITIES);
        let fragment = raw_fragment(&mut host, LAST, b"{}");
        assert!(phone.decrypt_message(&fragment).is_err());
    }

    #[test]
    fn an_ordinary_message_inside_a_run_is_malformed() {
        let (mut phone, mut host) = pair();
        let first = raw_fragment(&mut host, MORE, &body(FRAGMENT_PLAINTEXT));
        let ordinary = host.encrypt_frame(b"{}").unwrap();
        assert_eq!(phone.decrypt_message(&first).unwrap(), None);
        let err = phone.decrypt_message(&ordinary).unwrap_err();
        assert!(err.contains("inside a fragment run"), "{err}");
    }

    #[test]
    fn a_bad_flag_or_a_bare_marker_is_malformed() {
        let (mut phone, mut host) = pair();
        let bad = raw_fragment(&mut host, 0x02, b"{}");
        assert!(phone.decrypt_message(&bad).unwrap_err().contains("flag"));

        let (mut phone, _) = pair();
        assert!(phone.decrypt_message(&FRAGMENT_MARKER).is_err());
    }
}
