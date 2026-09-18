//! The relay hop: the host link's endpoint, its possession-of-key proof, and
//! the codec that multiplexes every off-LAN device onto one WebSocket.
//!
//! The contract is `docs/remote-protocol.md` → "Relay". The relay is a dumb
//! pipe that routes on the host ID and sees only the ciphertext the secure
//! channel already produces, so nothing here reaches above the transport.
//!
//! Only the pure parts live here: the frame codec, the endpoint and the proof.
//! The link's state machine — dial, authenticate, demultiplex, reconnect — is
//! the host's, and stays in the host.

use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::keys::{StaticKey, KEY_LEN};
use crate::Result;

const TYPE_OPEN: u8 = 0x01;
const TYPE_DATA: u8 = 0x02;
const TYPE_CLOSE: u8 = 0x03;
const TYPE_TEXT: u8 = 0x04;
const TYPE_NOTIFY: u8 = 0x05;

/// The `connId` a NOTIFY frame carries. It belongs to no virtual connection —
/// the doc fixes it at 0 so the header stays one shape for every frame type.
pub const NOTIFY_CONN: u32 = 0;

/// Length of the fixed header: `type (1) || connId (u32 big-endian)`.
const HEADER_LEN: usize = 5;

/// One frame on the host link. `OPEN` and `TEXT` only ever arrive at the host;
/// `DATA` and `CLOSE` travel both ways; `NOTIFY` is host → relay only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A device link attached; payload is empty.
    Open { conn: u32 },
    /// One device WebSocket binary message, verbatim.
    Data { conn: u32, payload: Bytes },
    /// The virtual connection is over: `code (u16 big-endian) || reason`.
    Close {
        conn: u32,
        code: u16,
        reason: String,
    },
    /// One device WebSocket *text* message, verbatim, so the host can apply its
    /// own 4001 rule to it rather than the relay guessing.
    Text { conn: u32, text: String },
    /// A push request for the relay itself to forward to APNs: UTF-8 JSON on
    /// `connId` 0. Host → relay only, and fire and forget — the relay sends no
    /// result frame, and one that does not know this type ignores it.
    Notify { payload: Bytes },
}

impl Frame {
    pub fn conn(&self) -> u32 {
        match self {
            Frame::Open { conn }
            | Frame::Data { conn, .. }
            | Frame::Close { conn, .. }
            | Frame::Text { conn, .. } => *conn,
            Frame::Notify { .. } => NOTIFY_CONN,
        }
    }

    pub fn encode(&self) -> Bytes {
        let mut out = Vec::with_capacity(HEADER_LEN + 32);
        out.push(match self {
            Frame::Open { .. } => TYPE_OPEN,
            Frame::Data { .. } => TYPE_DATA,
            Frame::Close { .. } => TYPE_CLOSE,
            Frame::Text { .. } => TYPE_TEXT,
            Frame::Notify { .. } => TYPE_NOTIFY,
        });
        out.extend_from_slice(&self.conn().to_be_bytes());
        match self {
            Frame::Open { .. } => {}
            Frame::Data { payload, .. } => out.extend_from_slice(payload),
            Frame::Close { code, reason, .. } => {
                out.extend_from_slice(&code.to_be_bytes());
                out.extend_from_slice(reason.as_bytes());
            }
            Frame::Text { text, .. } => out.extend_from_slice(text.as_bytes()),
            Frame::Notify { payload } => out.extend_from_slice(payload),
        }
        Bytes::from(out)
    }

    /// Decode one frame. `Err` carries what was wrong with it, for the log; a
    /// frame the host cannot parse is dropped, not fatal, since the relay is
    /// free to add frame types the host does not know yet.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(format!("frame of {} bytes has no header", bytes.len()));
        }
        let conn = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let body = &bytes[HEADER_LEN..];
        match bytes[0] {
            TYPE_OPEN => Ok(Frame::Open { conn }),
            TYPE_DATA => Ok(Frame::Data {
                conn,
                payload: Bytes::copy_from_slice(body),
            }),
            TYPE_CLOSE => {
                if body.len() < 2 {
                    return Err("close frame without a code".to_string());
                }
                Ok(Frame::Close {
                    conn,
                    code: u16::from_be_bytes([body[0], body[1]]),
                    reason: String::from_utf8_lossy(&body[2..]).into_owned(),
                })
            }
            TYPE_TEXT => Ok(Frame::Text {
                conn,
                text: String::from_utf8_lossy(body).into_owned(),
            }),
            // Decoded for the round-trip tests and for symmetry; the host never
            // receives one. `conn` is not checked against 0: a frame this side
            // only ever writes cannot arrive with another value unless the relay
            // invented it, and dropping it on arrival is already the answer.
            TYPE_NOTIFY => Ok(Frame::Notify {
                payload: Bytes::copy_from_slice(body),
            }),
            other => Err(format!("unknown frame type {other:#04x}")),
        }
    }

    pub fn into_message(self) -> Message {
        Message::Binary(self.encode())
    }
}

/// `<base>/v1/host/<hostId>`, per the doc's endpoint table.
pub fn host_endpoint(base: &str, host_id: &str) -> String {
    format!("{}/v1/host/{host_id}", base.trim_end_matches('/'))
}

/// `SHA-256( X25519(hostPrivate, relayKey) || nonce || hostKey )`, raw bytes
/// throughout (`docs/remote-protocol.md` → "Relay").
///
/// A relay that does not hold the private half of `relayKey` cannot verify the
/// proof, and one that does still learns nothing it could replay against
/// another relay — the host public key is hashed in, and it is the ID the relay
/// routes on anyway.
pub fn challenge_proof(
    host: &StaticKey,
    relay_key: &[u8; KEY_LEN],
    nonce: &[u8],
) -> Result<[u8; 32]> {
    let shared = host.diffie_hellman(relay_key)?;
    let mut digest = Sha256::new();
    digest.update(shared);
    digest.update(nonce);
    digest.update(host.public_bytes());
    Ok(digest.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use base64::Engine;

    fn roundtrip(frame: Frame) {
        let encoded = frame.encode();
        assert_eq!(Frame::decode(&encoded).unwrap(), frame, "{frame:?}");
    }

    #[test]
    fn every_frame_type_roundtrips() {
        roundtrip(Frame::Open { conn: 0 });
        roundtrip(Frame::Open { conn: u32::MAX });
        roundtrip(Frame::Data {
            conn: 7,
            payload: Bytes::from_static(&[0, 1, 2, 255]),
        });
        roundtrip(Frame::Data {
            conn: 7,
            payload: Bytes::new(),
        });
        roundtrip(Frame::Close {
            conn: 9,
            code: 4004,
            reason: "remote access disabled".to_string(),
        });
        roundtrip(Frame::Close {
            conn: 9,
            code: 1000,
            reason: String::new(),
        });
        roundtrip(Frame::Text {
            conn: 3,
            text: "{\"op\":\"hello\"}".to_string(),
        });
        roundtrip(Frame::Notify {
            payload: Bytes::from_static(br#"{"kind":"turn_complete"}"#),
        });
        roundtrip(Frame::Notify {
            payload: Bytes::new(),
        });
    }

    /// The header the doc fixes: `type (1) || connId (u32 big-endian)`, then the
    /// payload, and for CLOSE a `u16` big-endian code before the reason.
    #[test]
    fn the_wire_layout_is_the_documented_one() {
        assert_eq!(
            Frame::Data {
                conn: 0x01020304,
                payload: Bytes::from_static(b"hi"),
            }
            .encode()
            .as_ref(),
            &[0x02, 0x01, 0x02, 0x03, 0x04, b'h', b'i']
        );
        assert_eq!(
            Frame::Close {
                conn: 1,
                code: 4004,
                reason: "x".to_string(),
            }
            .encode()
            .as_ref(),
            &[0x03, 0, 0, 0, 1, 0x0f, 0xa4, b'x']
        );
        assert_eq!(
            Frame::Open { conn: 1 }.encode().as_ref(),
            &[0x01, 0, 0, 0, 1]
        );
        // NOTIFY is `0x05` on connId 0 — it belongs to no virtual connection.
        assert_eq!(
            Frame::Notify {
                payload: Bytes::from_static(b"{}"),
            }
            .encode()
            .as_ref(),
            &[0x05, 0, 0, 0, 0, b'{', b'}']
        );
    }

    #[test]
    fn malformed_frames_are_rejected_not_guessed() {
        for bytes in [
            vec![],
            vec![0x02],
            vec![0x02, 0, 0, 0],
            // CLOSE without room for a code.
            vec![0x03, 0, 0, 0, 1],
            vec![0x03, 0, 0, 0, 1, 0x0f],
            // A type the host does not know.
            vec![0x09, 0, 0, 0, 1],
        ] {
            assert!(
                Frame::decode(&bytes).is_err(),
                "{bytes:?} should not decode"
            );
        }
    }

    #[test]
    fn the_host_endpoint_is_the_documented_one() {
        assert_eq!(
            host_endpoint("wss://relay.fletch.sh", "abc"),
            "wss://relay.fletch.sh/v1/host/abc"
        );
        assert_eq!(
            host_endpoint("wss://relay.fletch.sh/", "abc"),
            "wss://relay.fletch.sh/v1/host/abc"
        );
    }

    /// The relay's own published vector (`relay/test-vector.json`, produced with
    /// Node's crypto and verified in workerd) must come out of this
    /// implementation byte for byte — the one place the two codebases are
    /// checked against the same numbers rather than each against itself. The
    /// fixed-key test below it is this crate's own vector, printed for
    /// comparison.
    #[test]
    fn the_challenge_proof_matches_the_relays_published_vector() {
        let raw = include_str!("../../../relay/test-vector.json");
        let vector: serde_json::Value = serde_json::from_str(raw).unwrap();
        let field = |name: &str| -> [u8; 32] {
            B64.decode(vector[name].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap_or_else(|_| panic!("{name} is not 32 bytes"))
        };

        let host = StaticKey::from_private(field("hostPrivate")).unwrap();
        assert_eq!(*host.public_bytes(), field("hostId"), "host public key");
        let relay_key = field("relayKey");
        assert_eq!(
            host.diffie_hellman(&relay_key).unwrap(),
            field("shared"),
            "X25519 shared secret"
        );
        let proof = challenge_proof(&host, &relay_key, &field("nonce")).unwrap();
        assert_eq!(proof, field("proof"), "proof");
    }

    /// The known-answer vector for the host link challenge, from fixed keys, so
    /// another implementation can be checked against exactly these bytes:
    ///
    /// - host private  `AQEB…` (32 × 0x01)
    /// - relay private `AgIC…` (32 × 0x02)
    /// - nonce         `AwMD…` (32 × 0x03)
    ///
    /// `proof = base64url( SHA-256( X25519(hostPrivate, relayPublic) || nonce || hostPublic ) )`.
    #[test]
    fn the_challenge_proof_matches_a_known_answer_vector() {
        let host = StaticKey::from_private([0x01; 32]).unwrap();
        let relay = StaticKey::from_private([0x02; 32]).unwrap();
        let nonce = [0x03u8; 32];

        // Both ends must reach the same secret from opposite sides, which is the
        // whole point of the challenge: the relay verifies with its own private
        // key against the host ID it is routing on.
        let from_host = host.diffie_hellman(relay.public_bytes()).unwrap();
        let from_relay = relay.diffie_hellman(host.public_bytes()).unwrap();
        assert_eq!(from_host, from_relay, "X25519 is not symmetric?");

        let proof = challenge_proof(&host, relay.public_bytes(), &nonce).unwrap();
        println!("host private:  {}", B64.encode([0x01u8; 32]));
        println!("host public:   {}", B64.encode(host.public_bytes()));
        println!("relay private: {}", B64.encode([0x02u8; 32]));
        println!("relay public:  {}", B64.encode(relay.public_bytes()));
        println!("nonce:         {}", B64.encode(nonce));
        println!("shared:        {}", B64.encode(from_host));
        println!("proof:         {}", B64.encode(proof));

        assert_eq!(
            B64.encode(host.public_bytes()),
            "pOCSkrZRwni5dyxWn1-puxPZBrRqtoyd-dwrRAn4ogk"
        );
        assert_eq!(
            B64.encode(relay.public_bytes()),
            "zo060cy2M-x7cMF4FKXHbs0CloUFDTRHRboFhw5YfVk"
        );
        assert_eq!(
            B64.encode(from_host),
            "LtdqtUmx5zwDHrSclEjweYrqgbaYJ5oMPcPkn7_EuVM"
        );
        assert_eq!(
            B64.encode(proof),
            "BzDo2Jh0uedvZ-WrLiP4wkMcCFhvU9L9REMa8_die2U"
        );
    }
}
