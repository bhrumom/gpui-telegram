//! Genuine End-to-End Encrypted (E2EE) Team Bot Sharing using X25519,
//! HKDF-SHA256, ChaCha20-Poly1305 AEAD, and Ed25519 `'share-send'` device proofs.
//!
//! Zero `rand` dependency: all randomness comes directly from `getrandom::getrandom`.
//! Secret key material is zeroized on drop via `zeroize`.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STD};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{BotError, BotErrorCode};

// ---------------------------------------------------------------------------
// Constants & Domain Separators
// ---------------------------------------------------------------------------

pub const TEAM_SHARE_ENVELOPE_VERSION: u8 = 1;
pub const TEAM_SHARE_OFFER_HKDF_INFO: &[u8] = b"orca-team-share-offer-v1";
pub const TEAM_SHARE_OFFER_SIG_DOMAIN: &[u8] = b"orca-team-share-offer-sig-v1\0";
pub const TEAM_SHARE_HELD_MSG_HKDF_INFO: &[u8] = b"orca-team-share-held-message-v1";
pub const TEAM_SHARE_SEND_PROOF_PURPOSE: &[u8] = b"share-send\0";

// ---------------------------------------------------------------------------
// Identity Keypairs & Low-Level Crypto Primitives
// ---------------------------------------------------------------------------

/// Public identity bundle shared between paired Orca hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSharePublicIdentity {
    pub host_id: String,
    pub x25519_public_b64: String,
    pub ed25519_public_b64: String,
}

impl TeamSharePublicIdentity {
    pub fn decode_x25519_public(&self) -> Result<[u8; 32], BotError> {
        decode_b64_fixed::<32>(&self.x25519_public_b64, "x25519_public_b64")
    }

    pub fn decode_ed25519_public(&self) -> Result<[u8; 32], BotError> {
        decode_b64_fixed::<32>(&self.ed25519_public_b64, "ed25519_public_b64")
    }
}

/// Host E2EE identity keypair (X25519 static encryption key + Ed25519 signing key).
///
/// Secret key bytes are automatically zeroized on drop and omitted from `Debug`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct TeamShareIdentityKeypair {
    #[zeroize(skip)]
    host_id: String,
    x25519_secret: [u8; 32],
    #[zeroize(skip)]
    x25519_public: [u8; 32],
    ed25519_secret: [u8; 32],
    #[zeroize(skip)]
    ed25519_public: [u8; 32],
}

impl fmt::Debug for TeamShareIdentityKeypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TeamShareIdentityKeypair")
            .field("host_id", &self.host_id)
            .field("x25519_public_b64", &BASE64_STD.encode(self.x25519_public))
            .field("ed25519_public_b64", &BASE64_STD.encode(self.ed25519_public))
            .finish()
    }
}

impl TeamShareIdentityKeypair {
    /// Generates a fresh keypair from the OS CSPRNG via `getrandom::getrandom`.
    pub fn generate(host_id: impl Into<String>) -> Result<Self, BotError> {
        let mut x25519_secret = [0u8; 32];
        let mut ed25519_secret = [0u8; 32];
        random_bytes(&mut x25519_secret)?;
        random_bytes(&mut ed25519_secret)?;
        let res = Self::from_secret_bytes(host_id, x25519_secret, ed25519_secret);
        x25519_secret.zeroize();
        ed25519_secret.zeroize();
        res
    }

    /// Deterministic constructor from explicit 32-byte secret seeds (ideal for tests and persistence).
    pub fn from_secret_bytes(
        host_id: impl Into<String>,
        x25519_secret: [u8; 32],
        ed25519_secret: [u8; 32],
    ) -> Result<Self, BotError> {
        let host_id = host_id.into();
        if host_id.trim().is_empty() || host_id.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "TeamShareIdentityKeypair host_id must be non-empty without control characters.",
            ));
        }
        if x25519_secret.iter().all(|&b| b == 0) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "X25519 secret key cannot be all-zero.",
            ));
        }
        let static_sec = StaticSecret::from(x25519_secret);
        let x25519_public = X25519PublicKey::from(&static_sec).to_bytes();
        let signing = SigningKey::from_bytes(&ed25519_secret);
        let ed25519_public = signing.verifying_key().to_bytes();
        Ok(Self {
            host_id,
            x25519_secret,
            x25519_public,
            ed25519_secret,
            ed25519_public,
        })
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn x25519_public_bytes(&self) -> [u8; 32] {
        self.x25519_public
    }

    pub fn ed25519_public_bytes(&self) -> [u8; 32] {
        self.ed25519_public
    }

    pub fn public_identity(&self) -> TeamSharePublicIdentity {
        TeamSharePublicIdentity {
            host_id: self.host_id.clone(),
            x25519_public_b64: BASE64_STD.encode(self.x25519_public),
            ed25519_public_b64: BASE64_STD.encode(self.ed25519_public),
        }
    }

    fn sign_bytes(&self, msg: &[u8]) -> [u8; 64] {
        let signing = SigningKey::from_bytes(&self.ed25519_secret);
        signing.sign(msg).to_bytes()
    }
}

fn random_bytes(buf: &mut [u8]) -> Result<(), BotError> {
    getrandom::getrandom(buf).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("OS CSPRNG getrandom failed: {e}"),
        )
    })
}

fn decode_b64_vec(raw: &str, field: &str) -> Result<Vec<u8>, BotError> {
    BASE64_STD.decode(raw.trim()).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid base64 in {field}: {e}"),
        )
    })
}

fn decode_b64_fixed<const N: usize>(raw: &str, field: &str) -> Result<[u8; N], BotError> {
    let bytes = decode_b64_vec(raw, field)?;
    if bytes.len() != N {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Expected {N} bytes in {field}, got {}.", bytes.len()),
        ));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn push_len_prefixed(buf: &mut Vec<u8>, slice: &[u8]) {
    buf.extend_from_slice(&(slice.len() as u64).to_be_bytes());
    buf.extend_from_slice(slice);
}

fn x25519_dh_checked(secret: &[u8; 32], peer_pub: &[u8; 32]) -> Result<[u8; 32], BotError> {
    let sec = StaticSecret::from(*secret);
    let pk = X25519PublicKey::from(*peer_pub);
    let shared = sec.diffie_hellman(&pk).to_bytes();
    if shared.iter().all(|&b| b == 0) {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "X25519 Diffie-Hellman produced an all-zero low-order shared secret.",
        ));
    }
    Ok(shared)
}

fn derive_aead_key(
    shared_secret: &[u8; 32],
    ephemeral_pub: &[u8; 32],
    recipient_pub: &[u8; 32],
    info: &[u8],
) -> Result<[u8; 32], BotError> {
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(ephemeral_pub);
    salt_hasher.update(recipient_pub);
    let salt: [u8; 32] = salt_hasher.finalize().into();

    let hk = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm).map_err(|_| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            "HKDF-SHA256 key derivation failed.",
        )
    })?;
    Ok(okm)
}

fn verify_ed25519_bytes(pub_bytes: &[u8; 32], msg: &[u8], sig_bytes: &[u8; 64]) -> Result<(), BotError> {
    let verifying = VerifyingKey::from_bytes(pub_bytes).map_err(|_| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            "Invalid Ed25519 verifying key bytes.",
        )
    })?;
    let sig = Signature::from_bytes(sig_bytes);
    verifying.verify(msg, &sig).map_err(|_| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            "Ed25519 signature verification failed.",
        )
    })
}

// ---------------------------------------------------------------------------
// E2EE Team Share Offer (Seal & Open)
// ---------------------------------------------------------------------------

/// Plaintext offer payload sealed inside [`SealedTeamShareOffer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamShareOfferPayload {
    pub share_id: String,
    pub bot_id: String,
    pub bot_name: String,
    pub relay_url: String,
    pub pinned_host_ed25519_b64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_chat_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
}

/// Wire envelope for an E2EE Team Bot Share Offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedTeamShareOffer {
    pub version: u8,
    pub offer_id: String,
    pub sender_host_id: String,
    pub recipient_host_id: String,
    pub expires_at_ms: i64,
    pub sender_ed25519_public_b64: String,
    pub ephemeral_x25519_public_b64: String,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
    pub signature_b64: String,
}

fn offer_aad(
    version: u8,
    offer_id: &str,
    sender_host_id: &str,
    recipient_host_id: &str,
    expires_at_ms: i64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + offer_id.len() + sender_host_id.len() + recipient_host_id.len());
    buf.push(version);
    push_len_prefixed(&mut buf, offer_id.as_bytes());
    push_len_prefixed(&mut buf, sender_host_id.as_bytes());
    push_len_prefixed(&mut buf, recipient_host_id.as_bytes());
    buf.extend_from_slice(&expires_at_ms.to_be_bytes());
    buf
}

fn offer_signature_digest(
    aad: &[u8],
    ephemeral_pub: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(TEAM_SHARE_OFFER_SIG_DOMAIN);
    hasher.update((aad.len() as u64).to_be_bytes());
    hasher.update(aad);
    hasher.update(ephemeral_pub);
    hasher.update(nonce);
    hasher.update((ciphertext.len() as u64).to_be_bytes());
    hasher.update(ciphertext);
    hasher.finalize().into()
}

/// Seals a `TeamShareOfferPayload` to `recipient` using ephemeral X25519 DH,
/// HKDF-SHA256 (`orca-team-share-offer-v1`), ChaCha20-Poly1305 AEAD, and an Ed25519
/// signature over the canonical envelope digest.
pub fn seal_team_share_offer(
    sender: &TeamShareIdentityKeypair,
    recipient: &TeamSharePublicIdentity,
    offer_id: &str,
    expires_at_ms: i64,
    payload: &TeamShareOfferPayload,
) -> Result<SealedTeamShareOffer, BotError> {
    if offer_id.trim().is_empty() || recipient.host_id.trim().is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Offer id and recipient host_id must be non-empty.",
        ));
    }
    let recipient_x25519 = recipient.decode_x25519_public()?;

    let mut eph_secret = [0u8; 32];
    random_bytes(&mut eph_secret)?;
    let eph_static = StaticSecret::from(eph_secret);
    let eph_pub = X25519PublicKey::from(&eph_static).to_bytes();

    let mut shared = x25519_dh_checked(&eph_secret, &recipient_x25519)?;
    eph_secret.zeroize();

    let mut aead_key = derive_aead_key(
        &shared,
        &eph_pub,
        &recipient_x25519,
        TEAM_SHARE_OFFER_HKDF_INFO,
    )?;
    shared.zeroize();

    let mut nonce_bytes = [0u8; 12];
    random_bytes(&mut nonce_bytes)?;
    let aad = offer_aad(
        TEAM_SHARE_ENVELOPE_VERSION,
        offer_id,
        sender.host_id(),
        &recipient.host_id,
        expires_at_ms,
    );
    let plaintext = serde_json::to_vec(payload).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Failed to serialize TeamShareOfferPayload: {e}"),
        )
    })?;

    let cipher = ChaCha20Poly1305::new((&aead_key).into());
    aead_key.zeroize();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                "ChaCha20-Poly1305 encryption of team share offer failed.",
            )
        })?;

    let digest = offer_signature_digest(&aad, &eph_pub, &nonce_bytes, &ciphertext);
    let sig = sender.sign_bytes(&digest);

    Ok(SealedTeamShareOffer {
        version: TEAM_SHARE_ENVELOPE_VERSION,
        offer_id: offer_id.to_string(),
        sender_host_id: sender.host_id().to_string(),
        recipient_host_id: recipient.host_id.clone(),
        expires_at_ms,
        sender_ed25519_public_b64: BASE64_STD.encode(sender.ed25519_public_bytes()),
        ephemeral_x25519_public_b64: BASE64_STD.encode(eph_pub),
        nonce_b64: BASE64_STD.encode(nonce_bytes),
        ciphertext_b64: BASE64_STD.encode(ciphertext),
        signature_b64: BASE64_STD.encode(sig),
    })
}

/// Verifies and decrypts a [`SealedTeamShareOffer`] for `recipient` at `now_ms`.
///
/// Rejects wrong recipient (`NotThisHost`), expired offers (`Expired`), and any
/// tampered signature/AAD/ciphertext (`InvalidArgument`).
pub fn open_team_share_offer(
    recipient: &TeamShareIdentityKeypair,
    expected_sender_ed25519_b64: Option<&str>,
    offer: &SealedTeamShareOffer,
    now_ms: i64,
) -> Result<TeamShareOfferPayload, BotError> {
    if offer.version != TEAM_SHARE_ENVELOPE_VERSION {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Unsupported sealed offer version {}.", offer.version),
        ));
    }
    if offer.recipient_host_id != recipient.host_id() {
        return Err(BotError::new(
            BotErrorCode::NotThisHost,
            format!(
                "Team share offer is addressed to host '{}', not '{}'.",
                offer.recipient_host_id,
                recipient.host_id()
            ),
        ));
    }
    if now_ms > offer.expires_at_ms {
        return Err(BotError::new(
            BotErrorCode::Expired,
            "Team share offer has expired.",
        ));
    }
    if let Some(expected) = expected_sender_ed25519_b64
        && offer.sender_ed25519_public_b64.trim() != expected.trim()
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Team share offer sender Ed25519 key does not match expected sender.",
        ));
    }

    let sender_ed_pub = decode_b64_fixed::<32>(&offer.sender_ed25519_public_b64, "sender_ed25519_public_b64")?;
    let eph_pub = decode_b64_fixed::<32>(&offer.ephemeral_x25519_public_b64, "ephemeral_x25519_public_b64")?;
    let nonce_bytes = decode_b64_fixed::<12>(&offer.nonce_b64, "nonce_b64")?;
    let ciphertext = decode_b64_vec(&offer.ciphertext_b64, "ciphertext_b64")?;
    let sig_bytes = decode_b64_fixed::<64>(&offer.signature_b64, "signature_b64")?;

    let aad = offer_aad(
        offer.version,
        &offer.offer_id,
        &offer.sender_host_id,
        &offer.recipient_host_id,
        offer.expires_at_ms,
    );
    let digest = offer_signature_digest(&aad, &eph_pub, &nonce_bytes, &ciphertext);
    verify_ed25519_bytes(&sender_ed_pub, &digest, &sig_bytes)?;

    let mut shared = x25519_dh_checked(&recipient.x25519_secret, &eph_pub)?;
    let mut aead_key = derive_aead_key(
        &shared,
        &eph_pub,
        &recipient.x25519_public_bytes(),
        TEAM_SHARE_OFFER_HKDF_INFO,
    )?;
    shared.zeroize();

    let cipher = ChaCha20Poly1305::new((&aead_key).into());
    aead_key.zeroize();
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                "Failed to authenticate/decrypt team share offer ciphertext.",
            )
        })?;

    serde_json::from_slice(&plaintext).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid decrypted TeamShareOfferPayload JSON: {e}"),
        )
    })
}

// ---------------------------------------------------------------------------
// E2EE Team Share Held Message & Mandatory 'share-send' Proof
// ---------------------------------------------------------------------------

/// Computes the canonical `'share-send'` device-proof digest:
/// `SHA-256(b"share-send\0" || host_key_bytes || body_bytes)`.
pub fn encode_share_send_proof_digest(host_key_bytes: &[u8], body_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(TEAM_SHARE_SEND_PROOF_PURPOSE);
    hasher.update(host_key_bytes);
    hasher.update(body_bytes);
    hasher.finalize().into()
}

/// Signs the `'share-send'` proof digest with `sender`'s Ed25519 identity key.
pub fn sign_share_send_proof(
    sender: &TeamShareIdentityKeypair,
    recipient_host_key_bytes: &[u8],
    body_bytes: &[u8],
) -> [u8; 64] {
    let digest = encode_share_send_proof_digest(recipient_host_key_bytes, body_bytes);
    sender.sign_bytes(&digest)
}

/// Verifies a `'share-send'` Ed25519 proof strictly binding `recipient_host_key_bytes` and `body_bytes`.
pub fn verify_share_send_proof(
    sender_ed25519_pub: &[u8; 32],
    recipient_host_key_bytes: &[u8],
    body_bytes: &[u8],
    proof_sig: &[u8; 64],
) -> Result<(), BotError> {
    let digest = encode_share_send_proof_digest(recipient_host_key_bytes, body_bytes);
    verify_ed25519_bytes(sender_ed25519_pub, &digest, proof_sig)
}

/// Plaintext payload sealed inside [`SealedTeamShareHeldMessage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamShareHeldMessagePayload {
    pub share_id: String,
    pub bot_id: String,
    pub client_message_id: String,
    pub sender_name: String,
    pub text: String,
    pub sent_at_ms: i64,
}

/// E2EE held-message envelope forwarded via relay while recipient host is offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedTeamShareHeldMessage {
    pub version: u8,
    pub share_id: String,
    pub bot_id: String,
    pub client_message_id: String,
    pub sender_host_id: String,
    pub recipient_host_id: String,
    pub sender_ed25519_public_b64: String,
    pub recipient_ed25519_public_b64: String,
    pub ephemeral_x25519_public_b64: String,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
    pub share_send_proof_b64: String,
}

fn held_message_aad(
    version: u8,
    share_id: &str,
    bot_id: &str,
    client_message_id: &str,
    sender_host_id: &str,
    recipient_host_id: &str,
    recipient_ed25519_pub: &[u8; 32],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        96 + share_id.len()
            + bot_id.len()
            + client_message_id.len()
            + sender_host_id.len()
            + recipient_host_id.len(),
    );
    buf.push(version);
    push_len_prefixed(&mut buf, share_id.as_bytes());
    push_len_prefixed(&mut buf, bot_id.as_bytes());
    push_len_prefixed(&mut buf, client_message_id.as_bytes());
    push_len_prefixed(&mut buf, sender_host_id.as_bytes());
    push_len_prefixed(&mut buf, recipient_host_id.as_bytes());
    buf.extend_from_slice(recipient_ed25519_pub);
    buf
}

fn held_message_proof_body(
    aad: &[u8],
    ephemeral_pub: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(aad.len() + 32 + 12 + 8 + ciphertext.len());
    push_len_prefixed(&mut body, aad);
    body.extend_from_slice(ephemeral_pub);
    body.extend_from_slice(nonce);
    push_len_prefixed(&mut body, ciphertext);
    body
}

/// Seals a teammate held message with ephemeral X25519 + HKDF-SHA256 + ChaCha20-Poly1305
/// and attaches the mandatory `'share-send'` Ed25519 proof binding `sha256("share-send\0" || host_key || body)`.
pub fn seal_team_share_held_message(
    sender: &TeamShareIdentityKeypair,
    recipient: &TeamSharePublicIdentity,
    payload: &TeamShareHeldMessagePayload,
) -> Result<SealedTeamShareHeldMessage, BotError> {
    if payload.share_id.trim().is_empty()
        || payload.bot_id.trim().is_empty()
        || payload.client_message_id.trim().is_empty()
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Held message share_id, bot_id, and client_message_id must be non-empty.",
        ));
    }
    let recipient_x25519 = recipient.decode_x25519_public()?;
    let recipient_ed25519 = recipient.decode_ed25519_public()?;

    let mut eph_secret = [0u8; 32];
    random_bytes(&mut eph_secret)?;
    let eph_static = StaticSecret::from(eph_secret);
    let eph_pub = X25519PublicKey::from(&eph_static).to_bytes();

    let mut shared = x25519_dh_checked(&eph_secret, &recipient_x25519)?;
    eph_secret.zeroize();

    let mut aead_key = derive_aead_key(
        &shared,
        &eph_pub,
        &recipient_x25519,
        TEAM_SHARE_HELD_MSG_HKDF_INFO,
    )?;
    shared.zeroize();

    let mut nonce_bytes = [0u8; 12];
    random_bytes(&mut nonce_bytes)?;
    let aad = held_message_aad(
        TEAM_SHARE_ENVELOPE_VERSION,
        &payload.share_id,
        &payload.bot_id,
        &payload.client_message_id,
        sender.host_id(),
        &recipient.host_id,
        &recipient_ed25519,
    );
    let plaintext = serde_json::to_vec(payload).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Failed to serialize TeamShareHeldMessagePayload: {e}"),
        )
    })?;

    let cipher = ChaCha20Poly1305::new((&aead_key).into());
    aead_key.zeroize();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                "ChaCha20-Poly1305 encryption of held message failed.",
            )
        })?;

    let body = held_message_proof_body(&aad, &eph_pub, &nonce_bytes, &ciphertext);
    let proof_sig = sign_share_send_proof(sender, &recipient_ed25519, &body);

    Ok(SealedTeamShareHeldMessage {
        version: TEAM_SHARE_ENVELOPE_VERSION,
        share_id: payload.share_id.clone(),
        bot_id: payload.bot_id.clone(),
        client_message_id: payload.client_message_id.clone(),
        sender_host_id: sender.host_id().to_string(),
        recipient_host_id: recipient.host_id.clone(),
        sender_ed25519_public_b64: BASE64_STD.encode(sender.ed25519_public_bytes()),
        recipient_ed25519_public_b64: BASE64_STD.encode(recipient_ed25519),
        ephemeral_x25519_public_b64: BASE64_STD.encode(eph_pub),
        nonce_b64: BASE64_STD.encode(nonce_bytes),
        ciphertext_b64: BASE64_STD.encode(ciphertext),
        share_send_proof_b64: BASE64_STD.encode(proof_sig),
    })
}

/// Verifies the `'share-send'` Ed25519 proof strictly against `recipient`'s own
/// Ed25519 public key and decrypts the held message envelope.
///
/// Rejects cross-host replays (`NotThisHost` / `InvalidArgument`), mismatched
/// `share_id`/`bot_id`/`client_message_id`, and any tampered ciphertext or proof.
pub fn open_team_share_held_message(
    recipient: &TeamShareIdentityKeypair,
    expected_sender_ed25519_b64: Option<&str>,
    expected_share_id: Option<&str>,
    expected_bot_id: Option<&str>,
    msg: &SealedTeamShareHeldMessage,
) -> Result<TeamShareHeldMessagePayload, BotError> {
    if msg.version != TEAM_SHARE_ENVELOPE_VERSION {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Unsupported held message version {}.", msg.version),
        ));
    }
    if msg.recipient_host_id != recipient.host_id() {
        return Err(BotError::new(
            BotErrorCode::NotThisHost,
            format!(
                "Held message is addressed to host '{}', not '{}'.",
                msg.recipient_host_id,
                recipient.host_id()
            ),
        ));
    }
    let my_ed_pub = recipient.ed25519_public_bytes();
    let wire_recipient_ed_pub =
        decode_b64_fixed::<32>(&msg.recipient_ed25519_public_b64, "recipient_ed25519_public_b64")?;
    if wire_recipient_ed_pub != my_ed_pub {
        return Err(BotError::new(
            BotErrorCode::NotThisHost,
            "Held message recipient Ed25519 host key does not match this host.",
        ));
    }
    if let Some(expected) = expected_sender_ed25519_b64
        && msg.sender_ed25519_public_b64.trim() != expected.trim()
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Held message sender Ed25519 key does not match expected sender.",
        ));
    }
    if let Some(expected_share) = expected_share_id
        && msg.share_id != expected_share
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Held message share_id does not match expected share.",
        ));
    }
    if let Some(expected_bot) = expected_bot_id
        && msg.bot_id != expected_bot
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Held message bot_id does not match expected bot.",
        ));
    }

    let sender_ed_pub =
        decode_b64_fixed::<32>(&msg.sender_ed25519_public_b64, "sender_ed25519_public_b64")?;
    let eph_pub =
        decode_b64_fixed::<32>(&msg.ephemeral_x25519_public_b64, "ephemeral_x25519_public_b64")?;
    let nonce_bytes = decode_b64_fixed::<12>(&msg.nonce_b64, "nonce_b64")?;
    let ciphertext = decode_b64_vec(&msg.ciphertext_b64, "ciphertext_b64")?;
    let proof_sig = decode_b64_fixed::<64>(&msg.share_send_proof_b64, "share_send_proof_b64")?;

    // Compute AAD & body strictly using THIS host's own verified Ed25519 key bytes
    let aad = held_message_aad(
        msg.version,
        &msg.share_id,
        &msg.bot_id,
        &msg.client_message_id,
        &msg.sender_host_id,
        recipient.host_id(),
        &my_ed_pub,
    );
    let body = held_message_proof_body(&aad, &eph_pub, &nonce_bytes, &ciphertext);
    verify_share_send_proof(&sender_ed_pub, &my_ed_pub, &body, &proof_sig)?;

    let mut shared = x25519_dh_checked(&recipient.x25519_secret, &eph_pub)?;
    let mut aead_key = derive_aead_key(
        &shared,
        &eph_pub,
        &recipient.x25519_public_bytes(),
        TEAM_SHARE_HELD_MSG_HKDF_INFO,
    )?;
    shared.zeroize();

    let cipher = ChaCha20Poly1305::new((&aead_key).into());
    aead_key.zeroize();
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                "Failed to authenticate/decrypt held message ciphertext.",
            )
        })?;

    let payload: TeamShareHeldMessagePayload = serde_json::from_slice(&plaintext).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid decrypted TeamShareHeldMessagePayload JSON: {e}"),
        )
    })?;

    if payload.share_id != msg.share_id
        || payload.bot_id != msg.bot_id
        || payload.client_message_id != msg.client_message_id
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Decrypted held message identifiers do not match signed envelope headers.",
        ));
    }

    Ok(payload)
}

// ---------------------------------------------------------------------------
// Team Share Offer State Machine & CAS Transition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamShareOfferState {
    Pending,
    Accepted,
    Declined,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamShareOfferAction {
    Accept,
    Decline,
    Revoke,
    Expire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamShareOfferRecord {
    pub offer_id: String,
    pub state: TeamShareOfferState,
    pub expires_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at_ms: Option<i64>,
}

/// Compare-and-swap state transition for a team share offer.
///
/// Prevents double-answering (`BotOfferAlreadyAnswered`) and accepting/declining
/// expired offers (`Expired`).
pub fn transition_team_share_offer(
    record: &TeamShareOfferRecord,
    action: TeamShareOfferAction,
    now_ms: i64,
) -> Result<TeamShareOfferRecord, BotError> {
    match record.state {
        TeamShareOfferState::Pending => match action {
            TeamShareOfferAction::Accept => {
                if now_ms > record.expires_at_ms {
                    return Err(BotError::new(
                        BotErrorCode::Expired,
                        "Cannot accept an expired team share offer.",
                    ));
                }
                Ok(TeamShareOfferRecord {
                    state: TeamShareOfferState::Accepted,
                    answered_at_ms: Some(now_ms),
                    ..record.clone()
                })
            }
            TeamShareOfferAction::Decline => {
                if now_ms > record.expires_at_ms {
                    return Err(BotError::new(
                        BotErrorCode::Expired,
                        "Cannot decline an expired team share offer.",
                    ));
                }
                Ok(TeamShareOfferRecord {
                    state: TeamShareOfferState::Declined,
                    answered_at_ms: Some(now_ms),
                    ..record.clone()
                })
            }
            TeamShareOfferAction::Revoke => Ok(TeamShareOfferRecord {
                state: TeamShareOfferState::Revoked,
                answered_at_ms: Some(now_ms),
                ..record.clone()
            }),
            TeamShareOfferAction::Expire => {
                if now_ms < record.expires_at_ms {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        "Offer has not yet reached expires_at_ms.",
                    ));
                }
                Ok(TeamShareOfferRecord {
                    state: TeamShareOfferState::Expired,
                    answered_at_ms: Some(now_ms),
                    ..record.clone()
                })
            }
        },
        TeamShareOfferState::Expired => match action {
            TeamShareOfferAction::Accept | TeamShareOfferAction::Decline => Err(BotError::new(
                BotErrorCode::Expired,
                "Team share offer is already expired.",
            )),
            TeamShareOfferAction::Revoke | TeamShareOfferAction::Expire => Err(BotError::new(
                BotErrorCode::BotOfferAlreadyAnswered,
                "Team share offer is already in terminal expired state.",
            )),
        },
        TeamShareOfferState::Accepted
        | TeamShareOfferState::Declined
        | TeamShareOfferState::Revoked => Err(BotError::new(
            BotErrorCode::BotOfferAlreadyAnswered,
            format!(
                "Team share offer '{}' was already answered (state: {:?}).",
                record.offer_id, record.state
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identities() -> (
        TeamShareIdentityKeypair,
        TeamShareIdentityKeypair,
        TeamShareIdentityKeypair,
    ) {
        let alice = TeamShareIdentityKeypair::from_secret_bytes(
            "host-alice",
            [0x11; 32],
            [0x22; 32],
        )
        .expect("alice keys");
        let bob = TeamShareIdentityKeypair::from_secret_bytes(
            "host-bob",
            [0x33; 32],
            [0x44; 32],
        )
        .expect("bob keys");
        let eve = TeamShareIdentityKeypair::from_secret_bytes(
            "host-eve",
            [0x55; 32],
            [0x66; 32],
        )
        .expect("eve keys");
        (alice, bob, eve)
    }

    #[test]
    fn identity_generation_and_debug_hides_secrets() {
        let kp = TeamShareIdentityKeypair::generate("host-local").expect("generate keypair");
        assert_eq!(kp.host_id(), "host-local");
        let pub_id = kp.public_identity();
        assert_eq!(pub_id.decode_x25519_public().expect("x25519"), kp.x25519_public_bytes());
        assert_eq!(pub_id.decode_ed25519_public().expect("ed25519"), kp.ed25519_public_bytes());
        let dbg = format!("{kp:?}");
        assert!(!dbg.contains("secret"));

        assert_eq!(
            TeamShareIdentityKeypair::from_secret_bytes("host-zero", [0u8; 32], [1u8; 32])
                .expect_err("all-zero x25519")
                .code,
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn seal_and_open_team_share_offer_roundtrip_and_attack_rejections() {
        let (alice, bob, eve) = test_identities();
        let payload = TeamShareOfferPayload {
            share_id: "share-42".to_string(),
            bot_id: "bot-scout".to_string(),
            bot_name: "Scout".to_string(),
            relay_url: "wss://c1.relay.onorca.dev".to_string(),
            pinned_host_ed25519_b64: alice.public_identity().ed25519_public_b64.clone(),
            pinned_chat_session_id: Some("chat-home-1".to_string()),
            sender_name: Some("Alice".to_string()),
        };

        let sealed = seal_team_share_offer(
            &alice,
            &bob.public_identity(),
            "offer-99",
            10_000,
            &payload,
        )
        .expect("seal offer");

        // Ciphertext must not contain plaintext fields
        assert!(!sealed.ciphertext_b64.contains("Scout"));

        let opened = open_team_share_offer(
            &bob,
            Some(&alice.public_identity().ed25519_public_b64),
            &sealed,
            5_000,
        )
        .expect("open offer");
        assert_eq!(opened, payload);

        // Wrong recipient -> NotThisHost
        assert_eq!(
            open_team_share_offer(&eve, None, &sealed, 5_000)
                .expect_err("wrong host")
                .code,
            BotErrorCode::NotThisHost
        );

        // Expired -> Expired
        assert_eq!(
            open_team_share_offer(&bob, None, &sealed, 10_001)
                .expect_err("expired")
                .code,
            BotErrorCode::Expired
        );

        // Tampered AAD (expires_at_ms) -> InvalidArgument
        let mut tampered_aad = sealed.clone();
        tampered_aad.expires_at_ms = 99_999;
        assert_eq!(
            open_team_share_offer(&bob, None, &tampered_aad, 5_000)
                .expect_err("tampered aad")
                .code,
            BotErrorCode::InvalidArgument
        );

        // Tampered ciphertext -> InvalidArgument
        let mut tampered_ct = sealed.clone();
        let mut raw_ct = decode_b64_vec(&tampered_ct.ciphertext_b64, "ct").expect("ct");
        raw_ct[0] ^= 0xff;
        tampered_ct.ciphertext_b64 = BASE64_STD.encode(raw_ct);
        assert_eq!(
            open_team_share_offer(&bob, None, &tampered_ct, 5_000)
                .expect_err("tampered ct")
                .code,
            BotErrorCode::InvalidArgument
        );

        // Wrong expected sender -> InvalidArgument
        assert_eq!(
            open_team_share_offer(
                &bob,
                Some(&eve.public_identity().ed25519_public_b64),
                &sealed,
                5_000
            )
            .expect_err("wrong expected sender")
            .code,
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn seal_and_open_held_message_enforces_share_send_proof_and_blocks_cross_host_replay() {
        let (alice, bob, eve) = test_identities();
        let payload = TeamShareHeldMessagePayload {
            share_id: "share-42".to_string(),
            bot_id: "bot-scout".to_string(),
            client_message_id: "msg-001".to_string(),
            sender_name: "Alice".to_string(),
            text: "Please check PR #418 before merge.".to_string(),
            sent_at_ms: 1_764_000_000_000,
        };

        let sealed = seal_team_share_held_message(&alice, &bob.public_identity(), &payload)
            .expect("seal held message");

        // Verify direct share-send digest formula: sha256(b"share-send\0" || host_key || body)
        let direct_sig = sign_share_send_proof(&alice, &bob.ed25519_public_bytes(), b"body-bytes");
        assert!(
            verify_share_send_proof(
                &alice.ed25519_public_bytes(),
                &bob.ed25519_public_bytes(),
                b"body-bytes",
                &direct_sig
            )
            .is_ok()
        );
        // Replaying same proof to Eve's host key MUST fail
        assert_eq!(
            verify_share_send_proof(
                &alice.ed25519_public_bytes(),
                &eve.ed25519_public_bytes(),
                b"body-bytes",
                &direct_sig
            )
            .expect_err("cross-host proof replay")
            .code,
            BotErrorCode::InvalidArgument
        );

        let opened = open_team_share_held_message(
            &bob,
            Some(&alice.public_identity().ed25519_public_b64),
            Some("share-42"),
            Some("bot-scout"),
            &sealed,
        )
        .expect("open held message");
        assert_eq!(opened, payload);

        // Relay forwards sealed message untouched to Eve -> NotThisHost
        assert_eq!(
            open_team_share_held_message(&eve, None, None, None, &sealed)
                .expect_err("wrong host replay")
                .code,
            BotErrorCode::NotThisHost
        );

        // Relay rewrites recipient headers to Eve's identity -> share-send proof fails!
        let mut forged_for_eve = sealed.clone();
        forged_for_eve.recipient_host_id = eve.host_id().to_string();
        forged_for_eve.recipient_ed25519_public_b64 = eve.public_identity().ed25519_public_b64;
        assert_eq!(
            open_team_share_held_message(&eve, None, None, None, &forged_for_eve)
                .expect_err("forged cross-host replay")
                .code,
            BotErrorCode::InvalidArgument
        );

        // Tampered bot_id -> InvalidArgument
        let mut tampered_bot = sealed.clone();
        tampered_bot.bot_id = "bot-other".to_string();
        assert_eq!(
            open_team_share_held_message(&bob, None, None, None, &tampered_bot)
                .expect_err("tampered bot_id")
                .code,
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn offer_cas_state_machine_blocks_double_answer_and_expired_accept() {
        let pending = TeamShareOfferRecord {
            offer_id: "offer-1".to_string(),
            state: TeamShareOfferState::Pending,
            expires_at_ms: 10_000,
            answered_at_ms: None,
        };

        // Expired accept rejected -> Expired
        assert_eq!(
            transition_team_share_offer(&pending, TeamShareOfferAction::Accept, 10_001)
                .expect_err("expired accept")
                .code,
            BotErrorCode::Expired
        );

        // Valid accept -> Accepted
        let accepted =
            transition_team_share_offer(&pending, TeamShareOfferAction::Accept, 5_000)
                .expect("accept offer");
        assert_eq!(accepted.state, TeamShareOfferState::Accepted);
        assert_eq!(accepted.answered_at_ms, Some(5_000));

        // Double answer -> BotOfferAlreadyAnswered
        assert_eq!(
            transition_team_share_offer(&accepted, TeamShareOfferAction::Decline, 6_000)
                .expect_err("double answer")
                .code,
            BotErrorCode::BotOfferAlreadyAnswered
        );
        assert_eq!(
            transition_team_share_offer(&accepted, TeamShareOfferAction::Accept, 6_000)
                .expect_err("repeat accept")
                .code,
            BotErrorCode::BotOfferAlreadyAnswered
        );
    }
}
