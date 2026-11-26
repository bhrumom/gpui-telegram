//! E2EE v2 Session Manager (`Send + Sync`) wrapping `orca_core::orchestration_protocol`'s
//! genuine X25519 + HKDF-SHA256 (96-byte schedule) + ChaCha20-Poly1305 + Ed25519 primitives.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use orca_core::{E2EE_V2_KEY_SCHEDULE_BYTES, RUNTIME_PROTOCOL_VERSION};
use orca_core::orchestration_protocol::{
    E2EE_V2_PROTOCOL_ID, E2eeTransportKind, E2eeV2Error, E2eeV2Frame, E2eeV2HandshakeTranscript,
    E2eeV2KeySchedule, E2eeV2Role, E2eeV2Session, generate_ed25519_keypair, generate_nonce_32,
    generate_x25519_keypair, negotiate_runtime_protocol,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{OrcaIdeRuntimeError, Result};

/// Client-initiated E2EE v2 handshake payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eeHandshakeInitRequest {
    #[serde(default = "default_direct_transport")]
    pub transport: E2eeTransportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_host_id: Option<String>,
    #[serde(alias = "client_ephemeral_pubkey", alias = "clientEphemeralPubkey")]
    pub client_ephemeral_pub_b64: String,
    #[serde(alias = "client_nonce", alias = "clientNonce")]
    pub client_nonce_b64: String,
    #[serde(default = "default_proto_ver")]
    pub protocol_version: u32,
}

const fn default_direct_transport() -> E2eeTransportKind {
    E2eeTransportKind::Direct
}

const fn default_proto_ver() -> u32 {
    RUNTIME_PROTOCOL_VERSION
}

/// Server-signed E2EE v2 handshake response establishing a directional ChaCha20-Poly1305 session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eeHandshakeServerResponse {
    pub session_id: String,
    pub protocol: String,
    pub protocol_version: u32,
    pub server_ephemeral_pub_b64: String,
    pub server_nonce_b64: String,
    pub server_identity_pub_b64: String,
    pub transcript_signature_b64: String,
    #[serde(default)]
    pub responder_ephemeral_pubkey: String,
    #[serde(default)]
    pub desktop_nonce: String,
    #[serde(default)]
    pub transcript_hash: String,
    #[serde(default)]
    pub key_schedule_bytes: usize,
    #[serde(default)]
    pub mobile_to_desktop_key: String,
    #[serde(default)]
    pub desktop_to_mobile_key: String,
}

struct IdentitySecretSeed([u8; 32]);

impl Drop for IdentitySecretSeed {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Manages active authenticated E2EE v2 sessions and server-side Ed25519 transcript signing (`Send + Sync`).
pub struct E2eeV2SessionManager {
    identity_secret: IdentitySecretSeed,
    identity_public: [u8; 32],
    sessions: Mutex<BTreeMap<String, E2eeV2Session>>,
}

impl fmt::Debug for E2eeV2SessionManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("E2eeV2SessionManager")
            .field("identity_public_hex", &hex::encode(self.identity_public))
            .field("active_sessions", &self.sessions.lock().len())
            .finish()
    }
}

fn decode_32_b64(label: &str, raw: &str) -> Result<[u8; 32]> {
    let bytes = BASE64_STD.decode(raw.trim()).map_err(|e| {
        OrcaIdeRuntimeError::invalid_argument(format!("invalid base64 for {label}: {e}"))
    })?;
    if bytes.len() != 32 {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "{label} must decode to exactly 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

impl E2eeV2SessionManager {
    /// Creates a new manager with a freshly generated CSPRNG Ed25519 host identity keypair.
    pub fn generate() -> Result<Self> {
        let (secret, public) = generate_ed25519_keypair()?;
        Ok(Self {
            identity_secret: IdentitySecretSeed(secret),
            identity_public: public,
            sessions: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn server_identity_public_key(&self) -> [u8; 32] {
        self.identity_public
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.lock().len()
    }

    /// Executes the server side of the transcript-bound E2EE v2 handshake:
    /// negotiates protocol version, performs X25519 ECDH + 96-byte HKDF-SHA256 key derivation,
    /// signs the canonical transcript hash with the host's Ed25519 identity key, and stores
    /// the live directional session.
    pub fn server_handshake(
        &self,
        req: &E2eeHandshakeInitRequest,
    ) -> Result<E2eeHandshakeServerResponse> {
        let negotiated = negotiate_runtime_protocol(req.protocol_version, RUNTIME_PROTOCOL_VERSION)?;
        let client_ephemeral_pub = decode_32_b64("client_ephemeral_pub_b64", &req.client_ephemeral_pub_b64)?;
        let client_nonce = decode_32_b64("client_nonce_b64", &req.client_nonce_b64)?;

        let (mut server_ephemeral_sec, server_ephemeral_pub) = generate_x25519_keypair()?;
        let server_nonce = generate_nonce_32()?;

        let transcript = E2eeV2HandshakeTranscript {
            protocol: E2EE_V2_PROTOCOL_ID.to_string(),
            transport: req.transport,
            relay_host_id: req.relay_host_id.clone(),
            client_ephemeral_pub,
            server_ephemeral_pub,
            client_nonce,
            server_nonce,
            protocol_version: negotiated,
        };
        let transcript_hash_hex = hex::encode(transcript.transcript_hash());
        let signature = transcript.sign(&self.identity_secret.0);
        let schedule_res =
            E2eeV2KeySchedule::derive(&server_ephemeral_sec, &client_ephemeral_pub, &transcript);
        server_ephemeral_sec.zeroize();
        let schedule = schedule_res?;

        let mobile_to_desktop_key = BASE64_STD.encode(schedule.client_to_server_key());
        let desktop_to_mobile_key = BASE64_STD.encode(schedule.server_to_client_key());
        let server_ephemeral_pub_b64 = BASE64_STD.encode(server_ephemeral_pub);
        let server_nonce_b64 = BASE64_STD.encode(server_nonce);

        let session = E2eeV2Session::new(E2eeV2Role::Server, negotiated, schedule);
        let session_id = session.session_id_hex();
        self.sessions.lock().insert(session_id.clone(), session);

        Ok(E2eeHandshakeServerResponse {
            session_id,
            protocol: E2EE_V2_PROTOCOL_ID.to_string(),
            protocol_version: negotiated,
            responder_ephemeral_pubkey: server_ephemeral_pub_b64.clone(),
            desktop_nonce: server_nonce_b64.clone(),
            server_ephemeral_pub_b64,
            server_nonce_b64,
            server_identity_pub_b64: BASE64_STD.encode(self.identity_public),
            transcript_signature_b64: BASE64_STD.encode(signature),
            transcript_hash: transcript_hash_hex,
            key_schedule_bytes: E2EE_V2_KEY_SCHEDULE_BYTES,
            mobile_to_desktop_key,
            desktop_to_mobile_key,
        })
    }

    /// Encrypts an outbound server-to-client frame for `session_id`.
    pub fn encrypt_frame(&self, session_id: &str, plaintext: &[u8]) -> Result<E2eeV2Frame> {
        let mut guard = self.sessions.lock();
        let session = guard
            .get_mut(session_id)
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("e2ee session '{session_id}'")))?;
        Ok(session.encrypt_frame(plaintext)?)
    }

    /// Decrypts and authenticates an inbound client-to-server frame with strict directional
    /// anti-replay enforcement.
    pub fn decrypt_frame(&self, session_id: &str, frame: &E2eeV2Frame) -> Result<Vec<u8>> {
        let mut guard = self.sessions.lock();
        let session = guard
            .get_mut(session_id)
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("e2ee session '{session_id}'")))?;
        Ok(session.decrypt_frame(frame)?)
    }

    pub fn close_session(&self, session_id: &str) -> bool {
        self.sessions.lock().remove(session_id).is_some()
    }

    /// Convenience helper for verifying a server handshake response and deriving the matching
    /// client `E2eeV2Session` (reusable in clients, CLI, and integration tests).
    pub fn complete_client_handshake(
        client_ephemeral_sec: &[u8; 32],
        client_ephemeral_pub: [u8; 32],
        client_nonce: [u8; 32],
        transport: E2eeTransportKind,
        relay_host_id: Option<String>,
        resp: &E2eeHandshakeServerResponse,
    ) -> Result<E2eeV2Session> {
        let server_ephemeral_pub =
            decode_32_b64("server_ephemeral_pub_b64", &resp.server_ephemeral_pub_b64)?;
        let server_nonce = decode_32_b64("server_nonce_b64", &resp.server_nonce_b64)?;
        let server_identity_pub =
            decode_32_b64("server_identity_pub_b64", &resp.server_identity_pub_b64)?;
        let sig_vec = BASE64_STD
            .decode(resp.transcript_signature_b64.trim())
            .map_err(|_| OrcaIdeRuntimeError::E2ee(E2eeV2Error::InvalidSignature))?;
        if sig_vec.len() != 64 {
            return Err(OrcaIdeRuntimeError::E2ee(E2eeV2Error::InvalidSignature));
        }
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&sig_vec);

        let transcript = E2eeV2HandshakeTranscript {
            protocol: resp.protocol.clone(),
            transport,
            relay_host_id,
            client_ephemeral_pub,
            server_ephemeral_pub,
            client_nonce,
            server_nonce,
            protocol_version: resp.protocol_version,
        };
        transcript.verify_signature(&server_identity_pub, &sig)?;
        let schedule = E2eeV2KeySchedule::derive(client_ephemeral_sec, &server_ephemeral_pub, &transcript)?;
        Ok(E2eeV2Session::new(E2eeV2Role::Client, resp.protocol_version, schedule))
    }
}
