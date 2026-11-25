//! Pure-Rust Unified Runtime RPC Protocol v3, genuine transcript-bound E2EE v2
//! (X25519 + HKDF-SHA256 96-byte key schedule + ChaCha20-Poly1305 + Ed25519),
//! CAS `AtomicSnapshot<T>`, generation-fenced snapshot commits, and zeroized
//! `ProtectedSecret` authenticated sealing.
//!
//! Zero `rand` dependency: all randomness comes directly from `getrandom::getrandom`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STD};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    E2EE_V2_KEY_SCHEDULE_BYTES, MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION,
    MIN_COMPATIBLE_RUNTIME_SERVER_VERSION, RUNTIME_PROTOCOL_VERSION,
};

// ---------------------------------------------------------------------------
// Unified Runtime RPC Protocol v3
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerScope {
    Desktop,
    Cli,
    Bot,
    Relay,
    RemoteHost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequestEnvelope {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub protocol_version: u32,
    pub caller_scope: CallerScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponseEnvelope {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcErrorPayload>,
}

impl RpcResponseEnvelope {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(RpcErrorPayload {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum ProtocolNegotiationError {
    #[error("client protocol version {client} is below minimum supported {min_client}")]
    ClientTooOld { client: u32, min_client: u32 },
    #[error("server protocol version {server} is below minimum supported {min_server}")]
    ServerTooOld { server: u32, min_server: u32 },
    #[error("no compatible runtime protocol version between client {client} and server {server}")]
    Incompatible { client: u32, server: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolNegotiationResult {
    pub negotiated_version: u32,
    pub client_version: u32,
    pub server_version: u32,
    pub common_capabilities: BTreeSet<String>,
}

/// Negotiates the highest mutually compatible runtime protocol version enforcing
/// `RUNTIME_PROTOCOL_VERSION = 3`, `MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION = 2`,
/// and `MIN_COMPATIBLE_RUNTIME_SERVER_VERSION = 2`.
pub fn negotiate_runtime_protocol(
    client_v: u32,
    server_v: u32,
) -> Result<u32, ProtocolNegotiationError> {
    if client_v < MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION {
        return Err(ProtocolNegotiationError::ClientTooOld {
            client: client_v,
            min_client: MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION,
        });
    }
    if server_v < MIN_COMPATIBLE_RUNTIME_SERVER_VERSION {
        return Err(ProtocolNegotiationError::ServerTooOld {
            server: server_v,
            min_server: MIN_COMPATIBLE_RUNTIME_SERVER_VERSION,
        });
    }
    let negotiated = client_v.min(server_v).min(RUNTIME_PROTOCOL_VERSION);
    let floor = MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION.max(MIN_COMPATIBLE_RUNTIME_SERVER_VERSION);
    if negotiated < floor {
        return Err(ProtocolNegotiationError::Incompatible {
            client: client_v,
            server: server_v,
        });
    }
    Ok(negotiated)
}

/// Negotiates runtime protocol version together with the intersection of modular
/// domain capability tokens advertised by client and server.
pub fn negotiate_runtime_capabilities(
    client_v: u32,
    server_v: u32,
    client_caps: &BTreeSet<String>,
    server_caps: &BTreeSet<String>,
) -> Result<ProtocolNegotiationResult, ProtocolNegotiationError> {
    let negotiated_version = negotiate_runtime_protocol(client_v, server_v)?;
    let common_capabilities = client_caps.intersection(server_caps).cloned().collect();
    Ok(ProtocolNegotiationResult {
        negotiated_version,
        client_version: client_v,
        server_version: server_v,
        common_capabilities,
    })
}

/// Bounded FIFO idempotency ring caching completed RPC responses by `operation_id`.
#[derive(Debug, Clone)]
pub struct OperationIdDedupRing {
    capacity: usize,
    order: VecDeque<String>,
    entries: BTreeMap<String, RpcResponseEnvelope>,
}

impl OperationIdDedupRing {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            capacity: cap,
            order: VecDeque::with_capacity(cap),
            entries: BTreeMap::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, operation_id: &str) -> Option<&RpcResponseEnvelope> {
        self.entries.get(operation_id)
    }

    pub fn record(&mut self, operation_id: impl Into<String>, response: RpcResponseEnvelope) {
        let op_id = operation_id.into();
        if let Some(slot) = self.entries.get_mut(&op_id) {
            *slot = response;
            return;
        }
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(op_id.clone());
        self.entries.insert(op_id, response);
    }
}

// ---------------------------------------------------------------------------
// Genuine E2EE v2 Protocol (X25519 + HKDF-SHA256 96B + ChaCha20-Poly1305 + Ed25519)
// ---------------------------------------------------------------------------

pub const E2EE_V2_PROTOCOL_ID: &str = "orca-mobile-e2ee/v2";
pub const E2EE_V2_HKDF_INFO: &[u8] = b"orca-mobile-e2ee-v2-key-schedule";
pub const E2EE_V2_TRANSCRIPT_SIG_DOMAIN: &[u8] = b"orca-e2ee-v2-transcript-v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E2eeTransportKind {
    Direct,
    Relay,
}

impl E2eeTransportKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Relay => "relay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E2eeDirection {
    ClientToServer,
    ServerToClient,
}

impl E2eeDirection {
    const fn wire_byte(self) -> u8 {
        match self {
            Self::ClientToServer => 1,
            Self::ServerToClient => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E2eeV2Role {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum E2eeV2Error {
    #[error("OS CSPRNG failure: {0}")]
    Entropy(String),
    #[error("invalid X25519 key material: {0}")]
    InvalidKey(String),
    #[error("HKDF-SHA256 expansion failed")]
    HkdfExpandFailed,
    #[error("Ed25519 handshake transcript signature invalid")]
    InvalidSignature,
    #[error("sequence counter exhausted")]
    SequenceOverflow,
    #[error("unexpected frame direction (possible reflection attack)")]
    WrongDirection,
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    ProtocolVersionMismatch { expected: u32, got: u32 },
    #[error("replay detected: expected seq {expected}, got {got}")]
    ReplayDetected { expected: u64, got: u64 },
    #[error("out-of-order frame: expected seq {expected}, got {got}")]
    OutOfOrder { expected: u64, got: u64 },
    #[error("AEAD encryption or authentication failed")]
    AeadFailed,
}

fn fill_random(buf: &mut [u8]) -> Result<(), E2eeV2Error> {
    getrandom::getrandom(buf).map_err(|e| E2eeV2Error::Entropy(e.to_string()))
}

pub fn generate_nonce_32() -> Result<[u8; 32], E2eeV2Error> {
    let mut out = [0u8; 32];
    fill_random(&mut out)?;
    Ok(out)
}

pub fn generate_x25519_keypair() -> Result<([u8; 32], [u8; 32]), E2eeV2Error> {
    let mut secret = [0u8; 32];
    fill_random(&mut secret)?;
    let static_sec = StaticSecret::from(secret);
    let public = X25519PublicKey::from(&static_sec).to_bytes();
    Ok((secret, public))
}

pub fn generate_ed25519_keypair() -> Result<([u8; 32], [u8; 32]), E2eeV2Error> {
    let mut secret = [0u8; 32];
    fill_random(&mut secret)?;
    let signing = SigningKey::from_bytes(&secret);
    let public = signing.verifying_key().to_bytes();
    Ok((secret, public))
}

/// Canonical length-prefixed E2EE v2 handshake transcript bound into HKDF salt
/// and Ed25519 host authentication signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eeV2HandshakeTranscript {
    pub protocol: String,
    pub transport: E2eeTransportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_host_id: Option<String>,
    pub client_ephemeral_pub: [u8; 32],
    pub server_ephemeral_pub: [u8; 32],
    pub client_nonce: [u8; 32],
    pub server_nonce: [u8; 32],
    pub protocol_version: u32,
}

impl E2eeV2HandshakeTranscript {
    pub fn encode_canonical(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        push_lp(&mut buf, self.protocol.as_bytes());
        push_lp(&mut buf, self.transport.as_str().as_bytes());
        match &self.relay_host_id {
            Some(id) => {
                buf.push(1);
                push_lp(&mut buf, id.as_bytes());
            }
            None => buf.push(0),
        }
        push_lp(&mut buf, &self.client_ephemeral_pub);
        push_lp(&mut buf, &self.server_ephemeral_pub);
        push_lp(&mut buf, &self.client_nonce);
        push_lp(&mut buf, &self.server_nonce);
        buf.extend_from_slice(&self.protocol_version.to_be_bytes());
        buf
    }

    pub fn transcript_hash(&self) -> [u8; 32] {
        let digest = Sha256::digest(self.encode_canonical());
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }

    fn signature_payload(&self) -> Vec<u8> {
        let hash = self.transcript_hash();
        let mut msg = Vec::with_capacity(E2EE_V2_TRANSCRIPT_SIG_DOMAIN.len() + 32);
        msg.extend_from_slice(E2EE_V2_TRANSCRIPT_SIG_DOMAIN);
        msg.extend_from_slice(&hash);
        msg
    }

    /// Signs the canonical handshake transcript hash with an Ed25519 secret seed.
    pub fn sign(&self, ed25519_secret_seed: &[u8; 32]) -> [u8; 64] {
        let signing = SigningKey::from_bytes(ed25519_secret_seed);
        signing.sign(&self.signature_payload()).to_bytes()
    }

    /// Verifies an Ed25519 transcript signature against `ed25519_public_key`.
    pub fn verify_signature(
        &self,
        ed25519_public_key: &[u8; 32],
        signature_bytes: &[u8; 64],
    ) -> Result<(), E2eeV2Error> {
        let vk = VerifyingKey::from_bytes(ed25519_public_key)
            .map_err(|_| E2eeV2Error::InvalidSignature)?;
        let sig = Signature::from_bytes(signature_bytes);
        vk.verify(&self.signature_payload(), &sig)
            .map_err(|_| E2eeV2Error::InvalidSignature)
    }
}

fn push_lp(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
}

/// 96-byte HKDF-SHA256 key schedule for E2EE v2 (`E2EE_V2_KEY_SCHEDULE_BYTES = 96`),
/// strictly zeroized on drop and redacted in `Debug`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct E2eeV2KeySchedule {
    client_to_server_key: [u8; 32],
    server_to_client_key: [u8; 32],
    session_binding_key: [u8; 32],
}

impl fmt::Debug for E2eeV2KeySchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("E2eeV2KeySchedule")
            .field("client_to_server_key", &"[REDACTED]")
            .field("server_to_client_key", &"[REDACTED]")
            .field("session_binding_key", &"[REDACTED]")
            .finish()
    }
}

impl E2eeV2KeySchedule {
    /// Derives exact 96-byte key schedule (`32B c2s || 32B s2c || 32B session_binding`)
    /// from X25519 shared secret + SHA-256 canonical transcript salt via HKDF-SHA256.
    pub fn derive(
        local_x25519_secret: &[u8; 32],
        peer_x25519_public: &[u8; 32],
        transcript: &E2eeV2HandshakeTranscript,
    ) -> Result<Self, E2eeV2Error> {
        if local_x25519_secret.iter().all(|&b| b == 0) {
            return Err(E2eeV2Error::InvalidKey(
                "local X25519 secret cannot be all-zero".to_string(),
            ));
        }
        if peer_x25519_public.iter().all(|&b| b == 0) {
            return Err(E2eeV2Error::InvalidKey(
                "peer X25519 public key cannot be all-zero".to_string(),
            ));
        }

        let secret = StaticSecret::from(*local_x25519_secret);
        let peer_pub = X25519PublicKey::from(*peer_x25519_public);
        let shared = secret.diffie_hellman(&peer_pub);
        let mut shared_bytes = *shared.as_bytes();
        if shared_bytes.iter().all(|&b| b == 0) {
            shared_bytes.zeroize();
            return Err(E2eeV2Error::InvalidKey(
                "X25519 shared secret is low-order all-zero".to_string(),
            ));
        }

        let salt = transcript.transcript_hash();
        let hk = Hkdf::<Sha256>::new(Some(&salt), &shared_bytes);
        shared_bytes.zeroize();

        let mut okm = [0u8; E2EE_V2_KEY_SCHEDULE_BYTES];
        hk.expand(E2EE_V2_HKDF_INFO, &mut okm)
            .map_err(|_| E2eeV2Error::HkdfExpandFailed)?;

        let mut client_to_server_key = [0u8; 32];
        let mut server_to_client_key = [0u8; 32];
        let mut session_binding_key = [0u8; 32];
        client_to_server_key.copy_from_slice(&okm[0..32]);
        server_to_client_key.copy_from_slice(&okm[32..64]);
        session_binding_key.copy_from_slice(&okm[64..96]);
        okm.zeroize();

        Ok(Self {
            client_to_server_key,
            server_to_client_key,
            session_binding_key,
        })
    }

    pub fn client_to_server_key(&self) -> &[u8; 32] {
        &self.client_to_server_key
    }

    pub fn server_to_client_key(&self) -> &[u8; 32] {
        &self.server_to_client_key
    }

    pub fn session_binding_key(&self) -> &[u8; 32] {
        &self.session_binding_key
    }

    pub fn session_id_hex(&self) -> String {
        hex::encode(self.session_binding_key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eeV2Frame {
    pub direction: E2eeDirection,
    pub seq: u64,
    pub protocol_version: u32,
    pub ciphertext: Vec<u8>,
}

/// Stateful authenticated E2EE v2 session with directional monotonic sequence counters
/// and ChaCha20-Poly1305 AEAD framing bound to `(session_binding_key, direction, seq, protocol_version)`.
#[derive(Debug)]
pub struct E2eeV2Session {
    role: E2eeV2Role,
    protocol_version: u32,
    schedule: E2eeV2KeySchedule,
    c2s_seq: u64,
    s2c_seq: u64,
}

impl E2eeV2Session {
    pub fn new(role: E2eeV2Role, protocol_version: u32, schedule: E2eeV2KeySchedule) -> Self {
        Self {
            role,
            protocol_version,
            schedule,
            c2s_seq: 0,
            s2c_seq: 0,
        }
    }

    pub fn role(&self) -> E2eeV2Role {
        self.role
    }

    pub fn session_id_hex(&self) -> String {
        self.schedule.session_id_hex()
    }

    pub fn c2s_seq(&self) -> u64 {
        self.c2s_seq
    }

    pub fn s2c_seq(&self) -> u64 {
        self.s2c_seq
    }

    pub fn encrypt_frame(&mut self, plaintext: &[u8]) -> Result<E2eeV2Frame, E2eeV2Error> {
        let (direction, key, seq_ref) = match self.role {
            E2eeV2Role::Client => (
                E2eeDirection::ClientToServer,
                self.schedule.client_to_server_key(),
                &mut self.c2s_seq,
            ),
            E2eeV2Role::Server => (
                E2eeDirection::ServerToClient,
                self.schedule.server_to_client_key(),
                &mut self.s2c_seq,
            ),
        };
        if *seq_ref == u64::MAX {
            return Err(E2eeV2Error::SequenceOverflow);
        }
        let seq = *seq_ref;
        let nonce_bytes = build_frame_nonce(direction, seq);
        let aad = build_frame_aad(
            self.schedule.session_binding_key(),
            direction,
            seq,
            self.protocol_version,
        );
        let cipher = ChaCha20Poly1305::new(key.into());
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eeV2Error::AeadFailed)?;
        *seq_ref = seq + 1;
        Ok(E2eeV2Frame {
            direction,
            seq,
            protocol_version: self.protocol_version,
            ciphertext,
        })
    }

    pub fn decrypt_frame(&mut self, frame: &E2eeV2Frame) -> Result<Vec<u8>, E2eeV2Error> {
        let (expected_dir, key, seq_ref) = match self.role {
            E2eeV2Role::Client => (
                E2eeDirection::ServerToClient,
                self.schedule.server_to_client_key(),
                &mut self.s2c_seq,
            ),
            E2eeV2Role::Server => (
                E2eeDirection::ClientToServer,
                self.schedule.client_to_server_key(),
                &mut self.c2s_seq,
            ),
        };
        if frame.direction != expected_dir {
            return Err(E2eeV2Error::WrongDirection);
        }
        if frame.protocol_version != self.protocol_version {
            return Err(E2eeV2Error::ProtocolVersionMismatch {
                expected: self.protocol_version,
                got: frame.protocol_version,
            });
        }
        let expected_seq = *seq_ref;
        if frame.seq < expected_seq {
            return Err(E2eeV2Error::ReplayDetected {
                expected: expected_seq,
                got: frame.seq,
            });
        }
        if frame.seq > expected_seq {
            return Err(E2eeV2Error::OutOfOrder {
                expected: expected_seq,
                got: frame.seq,
            });
        }
        let nonce_bytes = build_frame_nonce(frame.direction, frame.seq);
        let aad = build_frame_aad(
            self.schedule.session_binding_key(),
            frame.direction,
            frame.seq,
            frame.protocol_version,
        );
        let cipher = ChaCha20Poly1305::new(key.into());
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: &frame.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eeV2Error::AeadFailed)?;
        *seq_ref = expected_seq.saturating_add(1);
        Ok(plaintext)
    }
}

fn build_frame_nonce(direction: E2eeDirection, seq: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[3] = direction.wire_byte();
    nonce[4..12].copy_from_slice(&seq.to_be_bytes());
    nonce
}

fn build_frame_aad(
    session_binding_key: &[u8; 32],
    direction: E2eeDirection,
    seq: u64,
    protocol_version: u32,
) -> [u8; 45] {
    let mut aad = [0u8; 45];
    aad[0..32].copy_from_slice(session_binding_key);
    aad[32] = direction.wire_byte();
    aad[33..41].copy_from_slice(&seq.to_be_bytes());
    aad[41..45].copy_from_slice(&protocol_version.to_be_bytes());
    aad
}

// ---------------------------------------------------------------------------
// Vault, CAS Atomic Snapshots & Authenticated Protected Secrets
// ---------------------------------------------------------------------------

pub const PROTECTED_SECRET_ENVELOPE_VERSION: u8 = 1;
const PROTECTED_SECRET_AAD_PREFIX: &[u8] = b"orca-protected-secret-v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum VaultError {
    #[error("snapshot serialization failed: {0}")]
    Serialization(String),
    #[error("snapshot SHA-256 integrity verification failed")]
    IntegrityMismatch,
    #[error("snapshot version conflict: expected {expected}, current {current}")]
    VersionConflict { expected: u64, current: u64 },
    #[error("stale async write generation {generation} fenced by committed generation {committed}")]
    StaleGeneration { generation: u64, committed: u64 },
    #[error("OS CSPRNG failure: {0}")]
    Entropy(String),
    #[error("unsupported sealed secret version {0}")]
    UnsupportedVersion(u8),
    #[error("invalid base64 or nonce length in sealed secret")]
    MalformedEnvelope,
    #[error("sealed secret AAD label mismatch")]
    AadLabelMismatch,
    #[error("sealed secret AEAD authentication failed")]
    DecryptionFailed,
}

/// Integrity-verified, CAS-versioned snapshot envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtomicSnapshot<T> {
    pub version: u64,
    pub updated_at_ms: i64,
    pub sha256: String,
    pub payload: T,
}

impl<T: Serialize> AtomicSnapshot<T> {
    pub fn new(payload: T, updated_at_ms: i64) -> Result<Self, VaultError> {
        let sha256 = compute_payload_sha256(&payload)?;
        Ok(Self {
            version: 1,
            updated_at_ms,
            sha256,
            payload,
        })
    }

    pub fn verify_integrity(&self) -> Result<(), VaultError> {
        let actual = compute_payload_sha256(&self.payload)?;
        if ct_eq_bytes(actual.as_bytes(), self.sha256.as_bytes()) {
            Ok(())
        } else {
            Err(VaultError::IntegrityMismatch)
        }
    }

    /// CAS version transition: succeeds ONLY when `self.version == expected_version`
    /// and `self.verify_integrity()` passes.
    pub fn next_version(
        &self,
        expected_version: u64,
        new_payload: T,
        updated_at_ms: i64,
    ) -> Result<Self, VaultError> {
        if self.version != expected_version {
            return Err(VaultError::VersionConflict {
                expected: expected_version,
                current: self.version,
            });
        }
        self.verify_integrity()?;
        let sha256 = compute_payload_sha256(&new_payload)?;
        Ok(Self {
            version: self.version.saturating_add(1),
            updated_at_ms,
            sha256,
            payload: new_payload,
        })
    }
}

fn compute_payload_sha256<T: Serialize>(payload: &T) -> Result<String, VaultError> {
    let json = serde_json::to_vec(payload).map_err(|e| VaultError::Serialization(e.to_string()))?;
    Ok(hex::encode(Sha256::digest(&json)))
}

fn ct_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Generation fence preventing stale async snapshot renames from overwriting a
/// newer synchronous quit flush (SM1-04).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtomicSnapshotGenerationFence {
    next_generation: u64,
    last_committed_generation: u64,
}

impl AtomicSnapshotGenerationFence {
    pub fn begin_async_write(&mut self) -> u64 {
        self.next_generation = self.next_generation.saturating_add(1);
        self.next_generation
    }

    pub fn can_commit_async(&self, generation: u64) -> bool {
        generation > self.last_committed_generation
    }

    pub fn commit_async(&mut self, generation: u64) -> Result<(), VaultError> {
        if generation <= self.last_committed_generation {
            return Err(VaultError::StaleGeneration {
                generation,
                committed: self.last_committed_generation,
            });
        }
        self.last_committed_generation = generation;
        Ok(())
    }

    pub fn commit_sync_flush(&mut self) -> u64 {
        self.next_generation = self.next_generation.saturating_add(1);
        self.last_committed_generation = self.next_generation;
        self.last_committed_generation
    }

    pub fn last_committed_generation(&self) -> u64 {
        self.last_committed_generation
    }
}

/// Zeroized secret bytes wrapper whose `Debug` and `Display` ALWAYS render `"[REDACTED]"`.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct ProtectedSecret {
    bytes: Vec<u8>,
}

impl ProtectedSecret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn from_string(secret: String) -> Self {
        Self {
            bytes: secret.into_bytes(),
        }
    }

    pub fn expose_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl fmt::Debug for ProtectedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for ProtectedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedProtectedSecret {
    pub version: u8,
    pub aad_label: String,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
}

/// Authenticates and encrypts `secret` using ChaCha20-Poly1305 with a fresh OS CSPRNG
/// 12-byte nonce and domain-separated AAD binding `aad_label`.
pub fn seal_protected_secret(
    key: &[u8; 32],
    aad_label: &str,
    secret: &ProtectedSecret,
) -> Result<SealedProtectedSecret, VaultError> {
    let mut nonce_bytes = [0u8; 12];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| VaultError::Entropy(e.to_string()))?;
    let mut aad = Vec::with_capacity(PROTECTED_SECRET_AAD_PREFIX.len() + aad_label.len());
    aad.extend_from_slice(PROTECTED_SECRET_AAD_PREFIX);
    aad.extend_from_slice(aad_label.as_bytes());

    let cipher = ChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: secret.expose_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::DecryptionFailed)?;

    Ok(SealedProtectedSecret {
        version: PROTECTED_SECRET_ENVELOPE_VERSION,
        aad_label: aad_label.to_string(),
        nonce_b64: BASE64_STD.encode(nonce_bytes),
        ciphertext_b64: BASE64_STD.encode(ciphertext),
    })
}

/// Verifies AAD label & Poly1305 tag and decrypts a [`SealedProtectedSecret`].
pub fn open_protected_secret(
    key: &[u8; 32],
    expected_aad_label: &str,
    sealed: &SealedProtectedSecret,
) -> Result<ProtectedSecret, VaultError> {
    if sealed.version != PROTECTED_SECRET_ENVELOPE_VERSION {
        return Err(VaultError::UnsupportedVersion(sealed.version));
    }
    if sealed.aad_label != expected_aad_label {
        return Err(VaultError::AadLabelMismatch);
    }
    let nonce_vec = BASE64_STD
        .decode(&sealed.nonce_b64)
        .map_err(|_| VaultError::MalformedEnvelope)?;
    if nonce_vec.len() != 12 {
        return Err(VaultError::MalformedEnvelope);
    }
    let ciphertext = BASE64_STD
        .decode(&sealed.ciphertext_b64)
        .map_err(|_| VaultError::MalformedEnvelope)?;
    let mut aad = Vec::with_capacity(PROTECTED_SECRET_AAD_PREFIX.len() + expected_aad_label.len());
    aad.extend_from_slice(PROTECTED_SECRET_AAD_PREFIX);
    aad.extend_from_slice(expected_aad_label.as_bytes());

    let cipher = ChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce_vec),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::DecryptionFailed)?;
    Ok(ProtectedSecret::new(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_v3_negotiation_envelopes_and_dedup_ring() {
        assert_eq!(negotiate_runtime_protocol(3, 3), Ok(3));
        assert_eq!(negotiate_runtime_protocol(2, 3), Ok(2));
        assert_eq!(negotiate_runtime_protocol(3, 2), Ok(2));
        assert_eq!(negotiate_runtime_protocol(4, 3), Ok(3));
        assert!(matches!(
            negotiate_runtime_protocol(1, 3),
            Err(ProtocolNegotiationError::ClientTooOld { .. })
        ));
        assert!(matches!(
            negotiate_runtime_protocol(3, 1),
            Err(ProtocolNegotiationError::ServerTooOld { .. })
        ));

        let c_caps = BTreeSet::from(["bots".to_string(), "e2ee_v2".to_string()]);
        let s_caps = BTreeSet::from(["e2ee_v2".to_string(), "plugins".to_string()]);
        let res = negotiate_runtime_capabilities(3, 3, &c_caps, &s_caps).expect("negotiate");
        assert_eq!(res.negotiated_version, 3);
        assert_eq!(res.common_capabilities, BTreeSet::from(["e2ee_v2".to_string()]));

        let ok_env = RpcResponseEnvelope::ok(serde_json::json!({"pong": true}));
        let ok_json = serde_json::to_value(&ok_env).expect("serialize ok");
        assert_eq!(ok_json["ok"], true);
        assert_eq!(ok_json["result"]["pong"], true);

        let err_env = RpcResponseEnvelope::err("unknown_task", "missing");
        let err_json = serde_json::to_value(&err_env).expect("serialize err");
        assert_eq!(err_json["ok"], false);
        assert_eq!(err_json["error"]["code"], "unknown_task");

        let mut ring = OperationIdDedupRing::new(2);
        ring.record("op1", ok_env.clone());
        ring.record("op2", err_env.clone());
        ring.record("op3", ok_env.clone());
        assert_eq!(ring.len(), 2);
        assert!(ring.get("op1").is_none());
        assert_eq!(ring.get("op2"), Some(&err_env));
        assert_eq!(ring.get("op3"), Some(&ok_env));
    }

    #[test]
    fn e2ee_v2_real_handshake_96b_hkdf_bidirectional_aead_and_replay_tamper_rejection() {
        let (c_x_sec, c_x_pub) = generate_x25519_keypair().expect("client x25519");
        let (s_x_sec, s_x_pub) = generate_x25519_keypair().expect("server x25519");
        let (s_ed_sec, s_ed_pub) = generate_ed25519_keypair().expect("server ed25519");

        let transcript = E2eeV2HandshakeTranscript {
            protocol: E2EE_V2_PROTOCOL_ID.to_string(),
            transport: E2eeTransportKind::Relay,
            relay_host_id: Some("host_0123456789ab".to_string()),
            client_ephemeral_pub: c_x_pub,
            server_ephemeral_pub: s_x_pub,
            client_nonce: generate_nonce_32().expect("c nonce"),
            server_nonce: generate_nonce_32().expect("s nonce"),
            protocol_version: RUNTIME_PROTOCOL_VERSION,
        };

        // Ed25519 transcript signature verification (and tamper detection)
        let sig = transcript.sign(&s_ed_sec);
        assert!(transcript.verify_signature(&s_ed_pub, &sig).is_ok());
        let mut tampered_transcript = transcript.clone();
        tampered_transcript.protocol_version = 2;
        assert_eq!(
            tampered_transcript.verify_signature(&s_ed_pub, &sig),
            Err(E2eeV2Error::InvalidSignature)
        );

        // Both sides derive identical 96-byte HKDF-SHA256 schedule
        let c_sched = E2eeV2KeySchedule::derive(&c_x_sec, &s_x_pub, &transcript).expect("c sched");
        let s_sched = E2eeV2KeySchedule::derive(&s_x_sec, &c_x_pub, &transcript).expect("s sched");
        assert_eq!(c_sched.client_to_server_key(), s_sched.client_to_server_key());
        assert_eq!(c_sched.server_to_client_key(), s_sched.server_to_client_key());
        assert_eq!(c_sched.session_binding_key(), s_sched.session_binding_key());
        assert_ne!(c_sched.client_to_server_key(), c_sched.server_to_client_key());
        assert!(format!("{c_sched:?}").contains("[REDACTED]"));

        let mut client = E2eeV2Session::new(E2eeV2Role::Client, RUNTIME_PROTOCOL_VERSION, c_sched);
        let mut server = E2eeV2Session::new(E2eeV2Role::Server, RUNTIME_PROTOCOL_VERSION, s_sched);

        // Client -> Server frame 0 & 1
        let f0 = client.encrypt_frame(b"hello server").expect("encrypt f0");
        let f1 = client.encrypt_frame(b"second msg").expect("encrypt f1");

        // Out-of-order frame 1 rejected before frame 0
        assert_eq!(
            server.decrypt_frame(&f1),
            Err(E2eeV2Error::OutOfOrder { expected: 0, got: 1 })
        );
        // Reflection attack (client trying to decrypt its own outgoing frame) rejected
        assert_eq!(client.decrypt_frame(&f0), Err(E2eeV2Error::WrongDirection));

        // Tampered ciphertext rejected without advancing seq
        let mut tampered = f0.clone();
        tampered.ciphertext[0] ^= 0xFF;
        assert_eq!(server.decrypt_frame(&tampered), Err(E2eeV2Error::AeadFailed));
        assert_eq!(server.c2s_seq(), 0);

        // Genuine decryption advances counter and blocks replay
        assert_eq!(server.decrypt_frame(&f0).expect("decrypt f0"), b"hello server");
        assert_eq!(
            server.decrypt_frame(&f0),
            Err(E2eeV2Error::ReplayDetected { expected: 1, got: 0 })
        );
        assert_eq!(server.decrypt_frame(&f1).expect("decrypt f1"), b"second msg");

        // Server -> Client reply
        let r0 = server.encrypt_frame(b"hello client").expect("encrypt r0");
        assert_eq!(client.decrypt_frame(&r0).expect("decrypt r0"), b"hello client");
    }

    #[test]
    fn atomic_snapshot_generation_fence_and_protected_secret_sealing() {
        let snap1 = AtomicSnapshot::new(serde_json::json!({"theme": "dark"}), 1_000)
            .expect("snap1");
        assert!(snap1.verify_integrity().is_ok());

        let snap2 = snap1
            .next_version(1, serde_json::json!({"theme": "light"}), 2_000)
            .expect("cas v1->v2");
        assert_eq!(snap2.version, 2);
        assert_eq!(
            snap2.next_version(1, serde_json::json!({}), 3_000),
            Err(VaultError::VersionConflict { expected: 1, current: 2 })
        );

        let mut corrupted = snap2.clone();
        corrupted.payload = serde_json::json!({"theme": "tampered"});
        assert_eq!(corrupted.verify_integrity(), Err(VaultError::IntegrityMismatch));

        // Generation fence blocks stale async write after sync quit flush
        let mut fence = AtomicSnapshotGenerationFence::default();
        let async_gen = fence.begin_async_write();
        let quit_gen = fence.commit_sync_flush();
        assert!(quit_gen > async_gen);
        assert!(!fence.can_commit_async(async_gen));
        assert!(matches!(
            fence.commit_async(async_gen),
            Err(VaultError::StaleGeneration { .. })
        ));

        // ProtectedSecret always redacts Debug & Display and seals with AEAD
        let secret = ProtectedSecret::from_string("super-secret-oauth-token".to_string());
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(format!("{secret}"), "[REDACTED]");

        let key = [0x42u8; 32];
        let sealed = seal_protected_secret(&key, "bot:scout:oauth", &secret).expect("seal");
        let opened = open_protected_secret(&key, "bot:scout:oauth", &sealed).expect("open");
        assert_eq!(opened.expose_bytes(), b"super-secret-oauth-token");

        assert_eq!(
            open_protected_secret(&key, "bot:other:oauth", &sealed),
            Err(VaultError::AadLabelMismatch)
        );
        let wrong_key = [0x99u8; 32];
        assert_eq!(
            open_protected_secret(&wrong_key, "bot:scout:oauth", &sealed),
            Err(VaultError::DecryptionFailed)
        );
    }
}
