//! Encrypted OpenBot Vault, `0600` API Key Files, RFC 6238 TOTP, and One-Time Link Payments (`OB-F10` Part B).
//!
//! 1:1 pure-Rust port of upstream `src/main/vault-service.ts`, `src/main/vault-store.ts`,
//! `src/backend/agent-api-keys.ts`, `src/backend/password-vault.ts`,
//! `packages/contracts/src/two-step-code.ts`, and `src/backend/link-wallet.ts`.

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use chrono::{Duration, SecondsFormat, Utc};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_storage::{
    OpenBotDatabase, register_secret_value, register_secret_values, write_json_file_atomically,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use url::Url;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const VAULT_FILE_NAME: &str = "openbot-vault-v1.json";
pub const VAULT_MASTER_KEY_FILE_NAME: &str = "openbot-vault-master.key";
pub const VAULT_KEYS_DIR_NAME: &str = "vault-keys";
pub const LINK_CARD_ID_PREFIX: &str = "link:";

pub const VAULT_TEXT_LIMIT: usize = 512;
pub const VAULT_FILL_SITE_LIMIT: usize = 100;
pub const VAULT_NOTES_LIMIT: usize = 4_096;
pub const VAULT_PASSWORD_LIMIT: usize = 1_024;
pub const VAULT_API_KEY_LIMIT: usize = 8_192;
pub const VAULT_AGENT_SCOPE_LIMIT: usize = 1_000;
pub const TWO_STEP_KEY_LIMIT: usize = 1_024;
pub const LINK_PAYMENT_MIN_CONTEXT_CHARS: usize = 100;
pub const LINK_PAYMENT_MAX_CONTEXT_CHARS: usize = 2_000;
pub const LINK_PAYMENT_MAX_AMOUNT_MINOR: i64 = 100_000_000;
pub const MAX_VAULT_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;

const VAULT_AEAD_AAD: &[u8] = b"openbot-vault:envelope:v1";
const VAULT_HKDF_INFO: &[u8] = b"openbot-vault:aead-key:v1";
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const APPROVAL_TTL_MS: u64 = 5 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultItemKind {
    Login,
    Card,
    Contact,
    ApiKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultCardBrand {
    Visa,
    Mastercard,
    Amex,
    Discover,
}

impl VaultCardBrand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visa => "visa",
            Self::Mastercard => "mastercard",
            Self::Amex => "amex",
            Self::Discover => "discover",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultStatus {
    Ready,
    Unavailable,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretFillMethod {
    Password,
    Otp,
    Authenticator,
}

impl SecretFillMethod {
    pub fn parse(raw: &str) -> RuntimeResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "password" => Ok(Self::Password),
            "otp" => Ok(Self::Otp),
            "authenticator" | "totp" => Ok(Self::Authenticator),
            other => Err(OpenBotRuntimeError::Validation(format!(
                "invalid secret method '{other}': expected password, otp, or authenticator"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Otp => "otp",
            Self::Authenticator => "authenticator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoginProviderKind {
    #[serde(rename = "openbot_vault", alias = "vault")]
    OpenBotVault,
    #[serde(rename = "onepassword", alias = "1password")]
    OnePassword,
    Bitwarden,
    #[serde(rename = "protonpass", alias = "proton_pass")]
    ProtonPass,
}

impl LoginProviderKind {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::OpenBotVault => "OpenBot Vault",
            Self::OnePassword => "1Password",
            Self::Bitwarden => "Bitwarden",
            Self::ProtonPass => "Proton Pass",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultContactFields {
    #[serde(default)]
    pub label: String,
    #[serde(default, alias = "full_name")]
    pub full_name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub address2: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub region: String,
    #[serde(default, alias = "postal_code")]
    pub postal_code: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VaultSaveInput {
    #[serde(rename_all = "camelCase")]
    Login {
        #[serde(default)]
        id: Option<String>,
        website: String,
        #[serde(default)]
        username: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        password: Option<String>,
        #[serde(default, alias = "two_step_key")]
        two_step_key: Option<String>,
        #[serde(default)]
        agents: Option<Vec<String>>,
    },
    #[serde(rename_all = "camelCase")]
    Card {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: String,
        #[serde(default, alias = "name_on_card")]
        name_on_card: String,
        #[serde(alias = "exp_month")]
        exp_month: u8,
        #[serde(alias = "exp_year")]
        exp_year: u16,
        #[serde(default, alias = "billing_contact_id")]
        billing_contact_id: Option<String>,
        #[serde(default)]
        number: Option<String>,
        #[serde(default, alias = "security_code", alias = "cvv", alias = "cvc")]
        security_code: Option<String>,
        #[serde(default, alias = "fill_sites")]
        fill_sites: Option<Vec<String>>,
        #[serde(default)]
        agents: Option<Vec<String>>,
    },
    Contact {
        #[serde(default)]
        id: Option<String>,
        #[serde(flatten)]
        fields: VaultContactFields,
        #[serde(default)]
        agents: Option<Vec<String>>,
    },
    ApiKey {
        #[serde(default)]
        id: Option<String>,
        name: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        agents: Option<Vec<String>>,
    },
}

impl fmt::Debug for VaultSaveInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Login {
                id,
                website,
                username,
                ..
            } => f
                .debug_struct("VaultSaveInput::Login")
                .field("id", id)
                .field("website", website)
                .field("username", username)
                .field("password", &"[REDACTED]")
                .field("two_step_key", &"[REDACTED]")
                .finish(),
            Self::Card {
                id,
                label,
                name_on_card,
                exp_month,
                exp_year,
                ..
            } => f
                .debug_struct("VaultSaveInput::Card")
                .field("id", id)
                .field("label", label)
                .field("name_on_card", name_on_card)
                .field("exp_month", exp_month)
                .field("exp_year", exp_year)
                .field("number", &"[REDACTED]")
                .field("security_code", &"[REDACTED]")
                .finish(),
            Self::Contact { id, fields, .. } => f
                .debug_struct("VaultSaveInput::Contact")
                .field("id", id)
                .field("fields", fields)
                .finish(),
            Self::ApiKey { id, name, .. } => f
                .debug_struct("VaultSaveInput::ApiKey")
                .field("id", id)
                .field("name", name)
                .field("key", &"[REDACTED]")
                .finish(),
        }
    }
}

/// Strictly masked summary returned by `list_items` / `save_item` / `delete_item`.
/// NEVER contains plaintext passwords, TOTP seeds, card PANs, CVVs, or API keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VaultItemSummary {
    Login {
        id: String,
        website: String,
        username: String,
        notes: String,
        #[serde(rename = "hasPassword")]
        has_password: bool,
        #[serde(rename = "hasTwoStep")]
        has_two_step: bool,
        #[serde(rename = "addedBy", default, skip_serializing_if = "Option::is_none")]
        added_by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<String>>,
    },
    Card {
        id: String,
        label: String,
        #[serde(rename = "nameOnCard")]
        name_on_card: String,
        brand: Option<VaultCardBrand>,
        last4: String,
        #[serde(rename = "maskedNumber")]
        masked_number: String,
        #[serde(rename = "expMonth")]
        exp_month: u8,
        #[serde(rename = "expYear")]
        exp_year: u16,
        expiry: String,
        #[serde(rename = "hasSecurityCode")]
        has_security_code: bool,
        #[serde(rename = "billingContactId")]
        billing_contact_id: Option<String>,
        #[serde(rename = "fillSites")]
        fill_sites: Vec<String>,
        #[serde(rename = "addedBy", default, skip_serializing_if = "Option::is_none")]
        added_by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<String>>,
    },
    Contact {
        id: String,
        #[serde(flatten)]
        fields: VaultContactFields,
        #[serde(rename = "addedBy", default, skip_serializing_if = "Option::is_none")]
        added_by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<String>>,
    },
    ApiKey {
        id: String,
        name: String,
        notes: String,
        #[serde(rename = "hasKey")]
        has_key: bool,
        #[serde(rename = "addedBy", default, skip_serializing_if = "Option::is_none")]
        added_by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<String>>,
    },
}

impl VaultItemSummary {
    pub fn id(&self) -> &str {
        match self {
            Self::Login { id, .. }
            | Self::Card { id, .. }
            | Self::Contact { id, .. }
            | Self::ApiKey { id, .. } => id,
        }
    }

    pub const fn kind(&self) -> VaultItemKind {
        match self {
            Self::Login { .. } => VaultItemKind::Login,
            Self::Card { .. } => VaultItemKind::Card,
            Self::Contact { .. } => VaultItemKind::Contact,
            Self::ApiKey { .. } => VaultItemKind::ApiKey,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultState {
    pub status: VaultStatus,
    pub items: Vec<VaultItemSummary>,
    pub agent_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultLoginEntry {
    pub id: String,
    pub title: String,
    pub website: String,
    pub username: Option<String>,
    pub has_one_time_password: bool,
    pub provider: LoginProviderKind,
    pub provider_name: String,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLoginRef {
    pub id: String,
    #[zeroize(skip)]
    pub provider: LoginProviderKind,
    pub website: String,
    pub title: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub two_step_key: Option<String>,
}

impl fmt::Debug for ExternalLoginRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalLoginRef")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("website", &self.website)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("has_two_step", &self.two_step_key.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultTwoStepCode {
    pub id: String,
    pub code: String,
    pub period: u64,
    pub remaining_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedApiKeyFile {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPaymentRequest {
    pub amount: i64,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(alias = "merchant_name")]
    pub merchant_name: String,
    #[serde(alias = "merchant_url")]
    pub merchant_url: String,
    pub context: String,
    #[serde(default)]
    pub approved: bool,
    #[serde(default, alias = "agent_id")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPaymentResult {
    pub status: String,
    pub card_id: String,
    pub brand: Option<VaultCardBrand>,
    pub last4: String,
    pub masked_number: String,
    pub exp_month: u8,
    pub exp_year: u16,
    pub valid_until: String,
    pub amount: i64,
    pub currency: String,
    pub merchant_name: String,
    pub merchant_origin: String,
    pub one_time: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAuditEntry {
    pub timestamp: String,
    pub action: String,
    pub card_id: String,
    pub masked_number: String,
    pub last4: String,
    pub page_origin: String,
    pub agent_id: Option<String>,
    pub one_time: bool,
}

/// Controlled internal card payload returned ONLY by `VaultService::get_card_for_fill`
/// for authorized HTTPS `fill_card` execution. Intentionally does NOT implement `Serialize`
/// and redacts PAN/CVV in `Debug`, wiping sensitive memory on drop via `ZeroizeOnDrop`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct CardForFill {
    #[zeroize(skip)]
    pub card_id: String,
    #[zeroize(skip)]
    pub masked_number: String,
    #[zeroize(skip)]
    pub last4: String,
    #[zeroize(skip)]
    pub brand: Option<VaultCardBrand>,
    #[zeroize(skip)]
    pub name_on_card: String,
    #[zeroize(skip)]
    pub exp_month: u8,
    #[zeroize(skip)]
    pub exp_year: u16,
    #[zeroize(skip)]
    pub one_time: bool,
    number: Zeroizing<String>,
    security_code: Zeroizing<String>,
}

impl CardForFill {
    pub fn pan_for_browser_fill(&self) -> &str {
        self.number.as_str()
    }

    pub fn cvv_for_browser_fill(&self) -> &str {
        self.security_code.as_str()
    }

    pub fn expiry_mm_yy(&self) -> String {
        format!("{:02}/{:02}", self.exp_month, self.exp_year % 100)
    }
}

impl fmt::Debug for CardForFill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CardForFill")
            .field("card_id", &self.card_id)
            .field("masked_number", &self.masked_number)
            .field("last4", &self.last4)
            .field("brand", &self.brand)
            .field("exp_month", &self.exp_month)
            .field("exp_year", &self.exp_year)
            .field("one_time", &self.one_time)
            .field("number", &"[REDACTED]")
            .field("security_code", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct StoredLogin {
    #[zeroize(skip)]
    id: String,
    #[zeroize(skip)]
    website: String,
    #[zeroize(skip)]
    username: String,
    password: String,
    notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    two_step_key: Option<String>,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    added_by: Option<String>,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCardFillSite {
    page: String,
    frames: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct StoredCard {
    #[zeroize(skip)]
    id: String,
    #[zeroize(skip)]
    label: String,
    #[zeroize(skip)]
    name_on_card: String,
    number: String,
    #[zeroize(skip)]
    exp_month: u8,
    #[zeroize(skip)]
    exp_year: u16,
    security_code: String,
    #[zeroize(skip)]
    billing_contact_id: Option<String>,
    #[zeroize(skip)]
    #[serde(default)]
    fill_sites: Vec<StoredCardFillSite>,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    added_by: Option<String>,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredContact {
    id: String,
    #[serde(flatten)]
    fields: VaultContactFields,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    added_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct StoredApiKey {
    #[zeroize(skip)]
    id: String,
    #[zeroize(skip)]
    name: String,
    key: String,
    notes: String,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    added_by: Option<String>,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct StoredLinkCard {
    #[zeroize(skip)]
    card_id: String,
    number: String,
    security_code: String,
    #[zeroize(skip)]
    brand: Option<VaultCardBrand>,
    #[zeroize(skip)]
    last4: String,
    #[zeroize(skip)]
    name_on_card: String,
    #[zeroize(skip)]
    exp_month: u8,
    #[zeroize(skip)]
    exp_year: u16,
    #[zeroize(skip)]
    amount: i64,
    #[zeroize(skip)]
    currency: String,
    #[zeroize(skip)]
    merchant_name: String,
    #[zeroize(skip)]
    merchant_url: String,
    #[zeroize(skip)]
    context: String,
    #[zeroize(skip)]
    valid_until: String,
    #[zeroize(skip)]
    consumed: bool,
    #[zeroize(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    consumed_at: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct VaultRecord {
    #[serde(default)]
    logins: Vec<StoredLogin>,
    #[serde(default)]
    cards: Vec<StoredCard>,
    #[zeroize(skip)]
    #[serde(default)]
    contacts: Vec<StoredContact>,
    #[serde(default)]
    api_keys: Vec<StoredApiKey>,
    #[serde(default)]
    link_cards: Vec<StoredLinkCard>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultEnvelope {
    version: u32,
    kdf: String,
    salt_b64: String,
    nonce_b64: String,
    ciphertext_b64: String,
}

// ---------------------------------------------------------------------------
// RFC 4648 Base32 & RFC 6238 TOTP (`compute_totp`, `compute_totp_from_uri`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TotpAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

struct ParsedTwoStepKey {
    base32: Zeroizing<String>,
    algorithm: TotpAlgorithm,
    digits: u32,
    period_secs: u64,
}

fn decode_base32_rfc4648(raw: &str) -> RuntimeResult<Zeroizing<Vec<u8>>> {
    let mut cleaned = Zeroizing::new(String::with_capacity(raw.len()));
    for ch in raw.chars() {
        if ch.is_whitespace() || ch == '-' {
            continue;
        }
        cleaned.push(ch.to_ascii_uppercase());
    }
    let stripped = cleaned.trim_end_matches('=');
    if stripped.is_empty() {
        return Err(OpenBotRuntimeError::Validation(
            "TOTP Base32 secret cannot be empty".to_string(),
        ));
    }

    let mut out = Zeroizing::new(Vec::with_capacity((stripped.len() * 5) / 8));
    let mut bit_buf: u32 = 0;
    let mut bit_count: u32 = 0;

    for byte in stripped.bytes() {
        let val = match BASE32_ALPHABET.iter().position(|&c| c == byte) {
            Some(pos) => pos as u32,
            None => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "invalid Base32 character '{}' in TOTP secret",
                    byte as char
                )));
            }
        };
        bit_buf = (bit_buf << 5) | val;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(((bit_buf >> bit_count) & 0xff) as u8);
        }
    }

    if out.is_empty() {
        return Err(OpenBotRuntimeError::Validation(
            "decoded TOTP secret cannot be empty".to_string(),
        ));
    }
    Ok(out)
}

fn parse_two_step_key(raw: &str) -> RuntimeResult<ParsedTwoStepKey> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > TWO_STEP_KEY_LIMIT {
        return Err(OpenBotRuntimeError::Validation(
            "twoStepKey must be between 1 and 1024 characters".to_string(),
        ));
    }
    if trimmed.len() >= 10 && trimmed[..10].eq_ignore_ascii_case("otpauth://") {
        let parsed = Url::parse(trimmed).map_err(|err| {
            OpenBotRuntimeError::Validation(format!("invalid otpauth URI: {err}"))
        })?;
        let host = parsed.host_str().unwrap_or_default();
        if !host.eq_ignore_ascii_case("totp") {
            return Err(OpenBotRuntimeError::Validation(format!(
                "unsupported otpauth type '{host}' (expected 'totp')"
            )));
        }
        let mut secret: Option<String> = None;
        let mut algorithm = TotpAlgorithm::Sha1;
        let mut digits: u32 = 6;
        let mut period_secs: u64 = 30;
        for (k, v) in parsed.query_pairs() {
            if k.eq_ignore_ascii_case("secret") {
                secret = Some(v.into_owned());
            } else if k.eq_ignore_ascii_case("algorithm") {
                algorithm = match v.to_ascii_uppercase().as_str() {
                    "SHA1" | "SHA-1" => TotpAlgorithm::Sha1,
                    "SHA256" | "SHA-256" => TotpAlgorithm::Sha256,
                    "SHA512" | "SHA-512" => TotpAlgorithm::Sha512,
                    other => {
                        return Err(OpenBotRuntimeError::Validation(format!(
                            "unsupported TOTP algorithm '{other}'"
                        )));
                    }
                };
            } else if k.eq_ignore_ascii_case("digits") {
                let d: u32 = v.parse().map_err(|_| {
                    OpenBotRuntimeError::Validation(format!("invalid TOTP digits '{v}'"))
                })?;
                if !(6..=8).contains(&d) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "TOTP digits must be 6..=8, got {d}"
                    )));
                }
                digits = d;
            } else if k.eq_ignore_ascii_case("period") {
                let p: u64 = v.parse().map_err(|_| {
                    OpenBotRuntimeError::Validation(format!("invalid TOTP period '{v}'"))
                })?;
                if !(10..=300).contains(&p) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "TOTP period must be 10..=300 seconds, got {p}"
                    )));
                }
                period_secs = p;
            }
        }
        let secret_str = secret.ok_or_else(|| {
            OpenBotRuntimeError::Validation("otpauth URI missing 'secret' parameter".to_string())
        })?;
        let _ = decode_base32_rfc4648(&secret_str)?;
        return Ok(ParsedTwoStepKey {
            base32: Zeroizing::new(clean_base32_display(&secret_str)),
            algorithm,
            digits,
            period_secs,
        });
    }

    let _ = decode_base32_rfc4648(trimmed)?;
    Ok(ParsedTwoStepKey {
        base32: Zeroizing::new(clean_base32_display(trimmed)),
        algorithm: TotpAlgorithm::Sha1,
        digits: 6,
        period_secs: 30,
    })
}

fn clean_base32_display(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>()
        .trim_end_matches('=')
        .to_string()
}

fn normalize_two_step_key(raw: &str) -> RuntimeResult<String> {
    let parsed = parse_two_step_key(raw)?;
    if parsed.algorithm == TotpAlgorithm::Sha1 && parsed.digits == 6 && parsed.period_secs == 30 {
        return Ok(parsed.base32.as_str().to_string());
    }
    let alg = match parsed.algorithm {
        TotpAlgorithm::Sha1 => "SHA1",
        TotpAlgorithm::Sha256 => "SHA256",
        TotpAlgorithm::Sha512 => "SHA512",
    };
    Ok(format!(
        "otpauth://totp/OpenBot?secret={}&algorithm={alg}&digits={}&period={}",
        parsed.base32.as_str(),
        parsed.digits,
        parsed.period_secs
    ))
}

fn compute_totp_with_alg(
    secret_base32: &str,
    unix_secs: u64,
    period_secs: u64,
    digits: u32,
    algorithm: TotpAlgorithm,
) -> RuntimeResult<String> {
    if period_secs == 0 {
        return Err(OpenBotRuntimeError::Validation(
            "TOTP period_secs must be greater than 0".to_string(),
        ));
    }
    if !(1..=9).contains(&digits) {
        return Err(OpenBotRuntimeError::Validation(format!(
            "TOTP digits must be 1..=9, got {digits}"
        )));
    }
    let key = decode_base32_rfc4648(secret_base32)?;
    let counter = unix_secs / period_secs;
    let counter_bytes = counter.to_be_bytes();

    let digest: Zeroizing<Vec<u8>> = match algorithm {
        TotpAlgorithm::Sha1 => {
            let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&key).map_err(|err| {
                OpenBotRuntimeError::Execution(format!("HMAC-SHA1 init failed: {err}"))
            })?;
            mac.update(&counter_bytes);
            Zeroizing::new(mac.finalize().into_bytes().to_vec())
        }
        TotpAlgorithm::Sha256 => {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).map_err(|err| {
                OpenBotRuntimeError::Execution(format!("HMAC-SHA256 init failed: {err}"))
            })?;
            mac.update(&counter_bytes);
            Zeroizing::new(mac.finalize().into_bytes().to_vec())
        }
        TotpAlgorithm::Sha512 => {
            let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&key).map_err(|err| {
                OpenBotRuntimeError::Execution(format!("HMAC-SHA512 init failed: {err}"))
            })?;
            mac.update(&counter_bytes);
            Zeroizing::new(mac.finalize().into_bytes().to_vec())
        }
    };

    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let bin_code = (((digest[offset] & 0x7f) as u32) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    let modulus = 10u64.pow(digits);
    let otp = u64::from(bin_code) % modulus;
    Ok(format!("{otp:0width$}", width = digits as usize))
}

/// Genuine RFC 6238 TOTP generator using HMAC-SHA1 and RFC 4226 dynamic truncation.
pub fn compute_totp(
    secret_base32: &str,
    unix_secs: u64,
    period_secs: u64,
    digits: u32,
) -> RuntimeResult<String> {
    compute_totp_with_alg(secret_base32, unix_secs, period_secs, digits, TotpAlgorithm::Sha1)
}

/// Computes an RFC 6238 TOTP code from either a raw Base32 secret or an `otpauth://totp/...` URI.
pub fn compute_totp_from_uri(
    id: &str,
    uri_or_secret: &str,
    unix_ms: u64,
) -> RuntimeResult<VaultTwoStepCode> {
    let parsed = parse_two_step_key(uri_or_secret)?;
    let code = compute_totp_with_alg(
        &parsed.base32,
        unix_ms / 1_000,
        parsed.period_secs,
        parsed.digits,
        parsed.algorithm,
    )?;
    let period_ms = parsed.period_secs * 1_000;
    let remaining_ms = period_ms - (unix_ms % period_ms);
    Ok(VaultTwoStepCode {
        id: id.to_string(),
        code,
        period: parsed.period_secs,
        remaining_ms,
    })
}

// ---------------------------------------------------------------------------
// Crypto, Atomic `0600` File Helpers, Card & Origin Matching
// ---------------------------------------------------------------------------

fn now_rfc3339_millis() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn ensure_dir_mode_700(dir: &Path) -> RuntimeResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let existed = dir.exists();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        if !existed {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn set_file_mode_600(path: &Path) -> RuntimeResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn write_secret_file_atomically(path: &Path, bytes: &[u8]) -> RuntimeResult<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_dir_mode_700(parent)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("secret");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));

    let write_res: RuntimeResult<()> = (|| {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        set_file_mode_600(&tmp_path)?;
        fs::rename(&tmp_path, path)?;
        set_file_mode_600(path)?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

fn load_or_create_master_key(path: &Path) -> RuntimeResult<Zeroizing<[u8; 32]>> {
    if path.exists() {
        let mut file = OpenOptions::new().read(true).open(path)?;
        let mut raw = Zeroizing::new(Vec::new());
        file.read_to_end(&mut raw)?;
        set_file_mode_600(path)?;

        if raw.len() == 32 {
            let mut key = Zeroizing::new([0u8; 32]);
            key.copy_from_slice(&raw);
            return Ok(key);
        }
        if let Ok(text) = std::str::from_utf8(&raw) {
            let decoded = Zeroizing::new(hex::decode(text.trim()).map_err(|err| {
                OpenBotRuntimeError::Execution(format!("corrupt vault master key encoding: {err}"))
            })?);
            if decoded.len() == 32 {
                let mut key = Zeroizing::new([0u8; 32]);
                key.copy_from_slice(&decoded);
                return Ok(key);
            }
        }
        return Err(OpenBotRuntimeError::Execution(format!(
            "invalid vault master key size ({} bytes) at {}",
            raw.len(),
            path.display()
        )));
    }

    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *key).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("failed to generate vault master key entropy: {err}"))
    })?;
    let encoded = Zeroizing::new(hex::encode(&*key));
    write_secret_file_atomically(path, encoded.as_bytes())?;
    Ok(key)
}

fn derive_vault_aead_key(
    master_key: &[u8; 32],
    salt: &[u8; 16],
) -> RuntimeResult<Zeroizing<[u8; 32]>> {
    let params = Argon2Params::new(1_024, 1, 1, Some(32)).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("failed to initialize Argon2id parameters: {err}"))
    })?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut kek = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(master_key, salt, &mut *kek)
        .map_err(|err| {
            OpenBotRuntimeError::Execution(format!("Argon2id vault key derivation failed: {err}"))
        })?;

    let hk = Hkdf::<Sha256>::new(Some(salt), &*kek);
    let mut aead_key = Zeroizing::new([0u8; 32]);
    hk.expand(VAULT_HKDF_INFO, &mut *aead_key).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("HKDF-SHA256 vault subkey expansion failed: {err}"))
    })?;
    Ok(aead_key)
}

fn encrypt_vault_record(
    master_key: &[u8; 32],
    record: &VaultRecord,
) -> RuntimeResult<VaultEnvelope> {
    let plaintext = Zeroizing::new(serde_json::to_vec(record)?);
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("failed to generate vault KDF salt: {err}"))
    })?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::getrandom(&mut nonce_bytes).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("failed to generate vault AEAD nonce: {err}"))
    })?;

    let aead_key = derive_vault_aead_key(master_key, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&*aead_key));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
                aad: VAULT_AEAD_AAD,
            },
        )
        .map_err(|err| {
            OpenBotRuntimeError::Execution(format!("ChaCha20Poly1305 vault encryption failed: {err}"))
        })?;

    let envelope = VaultEnvelope {
        version: 1,
        kdf: "argon2id-hkdf-sha256".to_string(),
        salt_b64: BASE64_STANDARD.encode(salt),
        nonce_b64: BASE64_STANDARD.encode(nonce_bytes),
        ciphertext_b64: BASE64_STANDARD.encode(ciphertext),
    };
    let encoded_len = serde_json::to_vec(&envelope)?.len();
    if encoded_len > MAX_VAULT_ENVELOPE_BYTES {
        return Err(OpenBotRuntimeError::LimitExceeded(format!(
            "vault file exceeds maximum size of {MAX_VAULT_ENVELOPE_BYTES} bytes"
        )));
    }
    Ok(envelope)
}

fn decrypt_vault_envelope(
    master_key: &[u8; 32],
    envelope: &VaultEnvelope,
) -> RuntimeResult<VaultRecord> {
    if envelope.version != 1 {
        return Err(OpenBotRuntimeError::Validation(format!(
            "unsupported vault envelope version {}",
            envelope.version
        )));
    }
    if envelope.kdf != "argon2id-hkdf-sha256" {
        return Err(OpenBotRuntimeError::Validation(format!(
            "unsupported vault KDF '{}'",
            envelope.kdf
        )));
    }
    let salt_vec = BASE64_STANDARD.decode(envelope.salt_b64.trim()).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("invalid vault salt base64: {err}"))
    })?;
    if salt_vec.len() != 16 {
        return Err(OpenBotRuntimeError::Execution(format!(
            "invalid vault salt length: {}",
            salt_vec.len()
        )));
    }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&salt_vec);

    let nonce_vec = BASE64_STANDARD
        .decode(envelope.nonce_b64.trim())
        .map_err(|err| {
            OpenBotRuntimeError::Execution(format!("invalid vault nonce base64: {err}"))
        })?;
    if nonce_vec.len() != 12 {
        return Err(OpenBotRuntimeError::Execution(format!(
            "invalid vault nonce length: {}",
            nonce_vec.len()
        )));
    }
    let ciphertext = BASE64_STANDARD
        .decode(envelope.ciphertext_b64.trim())
        .map_err(|err| {
            OpenBotRuntimeError::Execution(format!("invalid vault ciphertext base64: {err}"))
        })?;

    let aead_key = derive_vault_aead_key(master_key, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&*aead_key));
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce_vec),
                Payload {
                    msg: &ciphertext,
                    aad: VAULT_AEAD_AAD,
                },
            )
            .map_err(|_| {
                OpenBotRuntimeError::Execution(
                    "vault AEAD authentication failed (wrong master key or tampered ciphertext)"
                        .to_string(),
                )
            })?,
    );
    Ok(serde_json::from_slice(&plaintext)?)
}

fn parse_website_url(value: &str) -> Option<Url> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    Url::parse(&candidate).ok()
}

fn is_web_scheme(url: &Url) -> bool {
    url.scheme() == "https" || url.scheme() == "http"
}

fn is_aws_region_label(label: &str) -> bool {
    let mut parts = label.split('-');
    let Some(geo) = parts.next() else {
        return false;
    };
    let rest: Vec<&str> = parts.collect();
    if rest.len() < 2 {
        return false;
    }
    let valid_geo = matches!(
        geo,
        "us" | "eu" | "ap" | "ca" | "sa" | "me" | "af" | "il" | "cn" | "mx"
    );
    let last_is_num = rest
        .last()
        .is_some_and(|seg| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()));
    valid_geo && last_is_num
}

fn public_suffix_label_count(labels: &[&str]) -> usize {
    let n = labels.len();

    // 5-label AWS dualstack regional service suffixes (e.g. s3.dualstack.us-east-1.amazonaws.com)
    if n >= 5
        && matches!(
            (
                labels[n - 5],
                labels[n - 4],
                labels[n - 2],
                labels[n - 1]
            ),
            (
                "s3" | "s3-accesspoint" | "s3-fips",
                "dualstack",
                "amazonaws",
                "com"
            )
        )
        && is_aws_region_label(labels[n - 3])
    {
        return 5;
    }

    // 4-label Azure Storage & AWS regional cloud private suffixes
    if n >= 4 {
        let quad = (labels[n - 4], labels[n - 3], labels[n - 2], labels[n - 1]);
        if matches!(
            quad,
            (
                "blob" | "file" | "queue" | "table" | "web",
                "core",
                "windows",
                "net"
            )
        ) {
            return 4;
        }
        if quad.2 == "amazonaws"
            && quad.3 == "com"
            && matches!(
                quad.0,
                "s3" | "s3-website"
                    | "s3-accesspoint"
                    | "s3-fips"
                    | "s3-control"
                    | "s3-control-fips"
                    | "s3-object-lambda"
                    | "execute-api"
                    | "lambda-url"
                    | "elb"
                    | "compute"
            )
            && is_aws_region_label(quad.1)
        {
            return 4;
        }
        if quad.0 == "lambda-url" && quad.2 == "on" && quad.3 == "aws" && is_aws_region_label(quad.1)
        {
            return 4;
        }
    }

    // 3-label cloud/private suffixes & regional hyphenated AWS suffixes
    if n >= 3 {
        let triple = (labels[n - 3], labels[n - 2], labels[n - 1]);
        if matches!(
            triple,
            ("s3", "amazonaws", "com")
                | ("s3-website", "amazonaws", "com")
                | ("storage", "googleapis", "com")
                | ("commondatastorage", "googleapis", "com")
                | ("pages", "github", "io")
                | ("a", "run", "app")
        ) {
            return 3;
        }
        if triple.1 == "amazonaws"
            && triple.2 == "com"
            && (triple.0.starts_with("s3-")
                || triple.0.starts_with("execute-api-")
                || triple.0.starts_with("lambda-url-")
                || triple.0.starts_with("elb-")
                || triple.0.starts_with("compute-"))
        {
            return 3;
        }
    }

    // 2-label ICANN multi-part ccTLDs & PSL private multi-tenant hosting suffixes
    // (`{ allowPrivateDomains: true }`, matching upstream `tldts.getDomain`).
    if n >= 2 {
        let pair = (labels[n - 2], labels[n - 1]);
        let is_two_label_suffix = matches!(
            pair,
            // ICANN multi-part ccTLDs
            ("co" | "org" | "ac" | "gov" | "me" | "net" | "ltd" | "plc" | "sch", "uk")
                | ("co" | "ne" | "or" | "ac" | "go" | "ed" | "gr" | "ad" | "lg", "jp")
                | ("com" | "net" | "org" | "edu" | "gov" | "asn" | "id", "au")
                | ("com" | "net" | "org" | "gov" | "edu", "br")
                | (
                    "co" | "org" | "net" | "govt" | "ac" | "gen" | "geek" | "maori" | "school",
                    "nz"
                )
                | (
                    "co" | "or" | "ne" | "re" | "go" | "ac" | "pe" | "hs" | "ms" | "es" | "sc"
                        | "kg" | "mil",
                    "kr"
                )
                | ("co" | "org" | "net" | "gov" | "ac" | "edu" | "web" | "nom", "za")
                | (
                    "co" | "net" | "org" | "gen" | "firm" | "ind" | "ac" | "edu" | "gov" | "nic"
                        | "res",
                    "in"
                )
                | ("com" | "org" | "gob" | "edu" | "net", "mx")
                | ("com" | "org" | "edu" | "gov" | "idv" | "net", "hk")
                | ("com" | "org" | "edu" | "gov" | "idv" | "net", "tw")
                | ("com" | "org" | "net" | "gov" | "edu" | "gen" | "biz" | "web", "tr")
                | ("com" | "org" | "net" | "gob" | "edu" | "tur", "ar")
                | ("co" | "or" | "ac" | "go" | "web" | "my" | "biz" | "sch", "id")
                | ("co" | "org" | "ac" | "gov" | "net" | "muni" | "k12", "il")
                | ("com" | "org" | "net" | "gov" | "edu", "my")
                | ("com" | "org" | "net" | "gov" | "edu", "ph")
                | ("com" | "org" | "net" | "gov" | "edu", "vn")
                | ("com" | "org" | "net" | "gov" | "edu", "sg")
                | ("com" | "net" | "org" | "gov" | "edu" | "ac", "cn")
                | ("com" | "org" | "net" | "gov" | "edu", "co")
                | ("com", "pe" | "ua" | "pk" | "ng" | "eg" | "sa" | "pt" | "gr" | "ru")
                | ("co" | "or" | "ac" | "go" | "in", "th")
                | ("co" | "or" | "ac", "ke")
                | ("com" | "net" | "org", "pl")
                | ("com" | "nom" | "org", "es")
                | ("eu" | "us" | "uk" | "de" | "jpn" | "cn" | "br" | "sa" | "za", "com")
                // PSL private multi-tenant hosting suffixes
                | ("github" | "gitlab" | "bitbucket" | "ngrok" | "pantheonsite" | "webflow" | "codepen" | "codesandbox" | "stackblitz", "io")
                | ("pages" | "workers" | "r2" | "fly" | "deno" | "cloudflarepages", "dev")
                | (
                    "vercel" | "netlify" | "web" | "railway" | "replit" | "ngrok-free" | "ngrok"
                        | "streamlit" | "run",
                    "app"
                )
                | ("vercel" | "now" | "surge", "sh")
                | (
                    "netlify" | "herokuapp" | "herokudns" | "appspot" | "firebaseapp"
                        | "amazonaws" | "onrender" | "myshopify" | "trycloudflare"
                        | "pythonanywhere" | "wpengine" | "blogspot" | "wordpress" | "tumblr"
                        | "auth0" | "okta" | "zendesk" | "force" | "salesforce-sites"
                        | "squarespace" | "wixsite" | "substack" | "medium" | "amplifyapp",
                    "com"
                )
                | (
                    "cloudfront" | "azurewebsites" | "azurestaticapps" | "azureedge" | "cloudapp"
                        | "fastly" | "ddns" | "serveo" | "cloudfunctions",
                    "net"
                )
                | ("repl" | "supabase" | "carrd", "co")
                | ("glitch" | "localtunnel", "me")
                | ("kinsta" | "convex", "cloud")
                | ("duckdns" | "hopto", "org")
                | ("supabase", "in")
                | ("framer", "website")
                | ("notion", "site")
                | ("hf", "space")
                | ("gradio", "live")
                | ("on", "aws")
        );
        if is_two_label_suffix {
            return 2;
        }
    }

    1
}

fn registrable_domain(host: &str) -> Option<String> {
    let trimmed = host.trim();
    let without_fqdn = trimmed.strip_suffix('.').unwrap_or(trimmed);
    let lower = without_fqdn.to_ascii_lowercase();
    if lower.is_empty() || lower.starts_with('[') || lower.ends_with(']') {
        return None;
    }
    if lower.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let labels: Vec<&str> = lower.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return None;
    }
    let last = labels[labels.len() - 1];
    if last.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let suffix_len = public_suffix_label_count(&labels);
    if labels.len() <= suffix_len {
        return None;
    }
    let start = labels.len() - suffix_len - 1;
    Some(labels[start..].join("."))
}

/// Checks whether a stored login website matches an active browser tab origin,
/// mirroring `src/backend/password-vault.ts:L65-L77`.
pub fn website_matches_origin(saved_website: &str, page_origin: &str) -> bool {
    let Some(page) = parse_website_url(page_origin) else {
        return false;
    };
    let Some(saved) = parse_website_url(saved_website) else {
        return false;
    };
    if !is_web_scheme(&saved) {
        return false;
    }
    let page_host = page.host_str().unwrap_or_default().to_ascii_lowercase();
    let saved_host = saved.host_str().unwrap_or_default().to_ascii_lowercase();
    if page_host.is_empty() || saved_host.is_empty() {
        return false;
    }
    if page.scheme() == "http" {
        return saved.scheme() == "http"
            && page_host == saved_host
            && page.port_or_known_default() == saved.port_or_known_default();
    }
    if page.scheme() != "https" {
        return false;
    }
    if saved.scheme() == "http" {
        return page_host == saved_host;
    }
    if page_host == saved_host {
        return true;
    }
    match (registrable_domain(&page_host), registrable_domain(&saved_host)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn canonical_https_origin(raw_url: &str, context_label: &str) -> RuntimeResult<String> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > INPUT_LIMITS.browser_url {
        return Err(OpenBotRuntimeError::Validation(format!(
            "{context_label} requires a valid HTTPS URL"
        )));
    }
    let parsed = Url::parse(trimmed).map_err(|_| {
        OpenBotRuntimeError::Validation(format!(
            "{context_label} requires a valid https:// URL (TLS secure origin)"
        ))
    })?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(OpenBotRuntimeError::Validation(format!(
            "{context_label} requires an https:// secure TLS URL; non-HTTPS origins are refused"
        )));
    }
    Ok(parsed.origin().ascii_serialization())
}

fn normalize_vault_website(raw: &str) -> RuntimeResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > VAULT_TEXT_LIMIT {
        return Err(OpenBotRuntimeError::Validation(
            "vault website must be between 1 and 512 characters".to_string(),
        ));
    }
    let parsed = parse_website_url(trimmed).ok_or_else(|| {
        OpenBotRuntimeError::Validation(format!("invalid vault website URL '{trimmed}'"))
    })?;
    if !is_web_scheme(&parsed) || parsed.host_str().is_none() {
        return Err(OpenBotRuntimeError::Validation(
            "vault website must use http:// or https://".to_string(),
        ));
    }
    Ok(parsed.to_string())
}

fn login_title(website: &str) -> String {
    parse_website_url(website)
        .and_then(|u| u.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| website.to_string())
}

pub fn detect_card_brand(number: &str) -> Option<VaultCardBrand> {
    let digits: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.starts_with('4') {
        return Some(VaultCardBrand::Visa);
    }
    if digits.len() >= 2 {
        let p2: u32 = digits[..2].parse().unwrap_or(0);
        if (51..=55).contains(&p2) {
            return Some(VaultCardBrand::Mastercard);
        }
        if p2 == 34 || p2 == 37 {
            return Some(VaultCardBrand::Amex);
        }
        if p2 == 65 {
            return Some(VaultCardBrand::Discover);
        }
    }
    if digits.len() >= 4 {
        let p4: u32 = digits[..4].parse().unwrap_or(0);
        if (2221..=2720).contains(&p4) {
            return Some(VaultCardBrand::Mastercard);
        }
        if p4 == 6011 {
            return Some(VaultCardBrand::Discover);
        }
    }
    if digits.len() >= 3 {
        let p3: u32 = digits[..3].parse().unwrap_or(0);
        if (644..=649).contains(&p3) {
            return Some(VaultCardBrand::Discover);
        }
    }
    None
}

pub fn mask_card_last4(last4: &str) -> String {
    let suffix = if last4.len() >= 4 {
        &last4[last4.len() - 4..]
    } else {
        last4
    };
    format!("**** **** **** {suffix}")
}

pub fn is_luhn_valid(raw: &str) -> bool {
    let digits: Vec<u32> = raw
        .chars()
        .filter(|c| c.is_ascii_digit())
        .filter_map(|c| c.to_digit(10))
        .collect();
    if !(12..=19).contains(&digits.len()) {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut val = d;
        if double {
            val *= 2;
            if val > 9 {
                val -= 9;
            }
        }
        sum += val;
        double = !double;
    }
    sum % 10 == 0
}

fn generate_virtual_card_pan_4242() -> RuntimeResult<Zeroizing<String>> {
    let mut rand_bytes = [0u8; 8];
    getrandom::getrandom(&mut rand_bytes).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("failed to generate virtual card entropy: {err}"))
    })?;
    let mut digits = [0u32; 16];
    digits[0] = 4;
    digits[1] = 5;
    digits[2] = 3;
    digits[3] = 2;
    for i in 0..7 {
        digits[4 + i] = u32::from(rand_bytes[i] % 10);
    }
    digits[12] = 4;
    digits[13] = 2;
    digits[14] = 4;
    digits[15] = 2;

    // Solve index 11 (odd index from left => 5th from right => weight 1 in Luhn)
    // so the complete 16-digit PAN is strictly Luhn-valid while ending in 4242.
    let mut sum = 0u32;
    for (idx, &d) in digits.iter().enumerate() {
        if idx == 11 {
            continue;
        }
        let val = if idx % 2 == 0 {
            let doubled = d * 2;
            if doubled > 9 { doubled - 9 } else { doubled }
        } else {
            d
        };
        sum += val;
    }
    digits[11] = (10 - (sum % 10)) % 10;

    let mut pan = Zeroizing::new(String::with_capacity(16));
    for d in digits {
        pan.push(char::from_digit(d, 10).unwrap_or('0'));
    }
    if pan.contains("4111") {
        // Re-randomize deterministically away from common test PAN prefix 4111.
        digits[4] = (digits[4] + 3) % 10;
        let mut recomputed = 0u32;
        for (idx, &d) in digits.iter().enumerate() {
            if idx == 11 {
                continue;
            }
            let val = if idx % 2 == 0 {
                let doubled = d * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                d
            };
            recomputed += val;
        }
        digits[11] = (10 - (recomputed % 10)) % 10;
        pan.clear();
        for d in digits {
            pan.push(char::from_digit(d, 10).unwrap_or('0'));
        }
    }
    Ok(pan)
}

fn generate_virtual_cvv() -> RuntimeResult<Zeroizing<String>> {
    let mut bytes = [0u8; 3];
    getrandom::getrandom(&mut bytes).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("failed to generate virtual CVV entropy: {err}"))
    })?;
    let code = (u32::from(bytes[0]) % 9 + 1) * 100
        + (u32::from(bytes[1]) % 10) * 10
        + (u32::from(bytes[2]) % 10);
    Ok(Zeroizing::new(format!("{code:03}")))
}

/// Strictly validates API key names against path traversal (`..`, `/`, `\`, NUL, leading dots).
pub fn validate_api_key_name(raw: &str) -> RuntimeResult<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(OpenBotRuntimeError::Validation(
            "API key name must be 1..=64 ASCII characters".to_string(),
        ));
    }
    if name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.starts_with('.')
    {
        return Err(OpenBotRuntimeError::Validation(format!(
            "invalid API key name '{name}': path traversal or separator characters are forbidden"
        )));
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_alphabetic()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
    {
        return Err(OpenBotRuntimeError::Validation(format!(
            "invalid API key name '{name}': must start with a letter and contain only [A-Za-z0-9_-]"
        )));
    }
    Ok(name.to_string())
}

fn register_vault_secrets(record: &VaultRecord) {
    let mut short_secrets: Vec<String> = Vec::new();
    let mut long_secrets: Vec<String> = Vec::new();
    for login in &record.logins {
        if !login.password.is_empty() {
            short_secrets.push(login.password.clone());
        }
        if let Some(ref key) = login.two_step_key {
            long_secrets.push(key.clone());
            if let Ok(parsed) = parse_two_step_key(key) {
                long_secrets.push(parsed.base32.as_str().to_string());
            }
        }
    }
    for card in &record.cards {
        if !card.number.is_empty() {
            long_secrets.push(card.number.clone());
            long_secrets.push(group_pan(&card.number, ' '));
            long_secrets.push(group_pan(&card.number, '-'));
        }
        if !card.security_code.is_empty() {
            short_secrets.push(card.security_code.clone());
        }
    }
    for link in &record.link_cards {
        if !link.number.is_empty() {
            long_secrets.push(link.number.clone());
            long_secrets.push(group_pan(&link.number, ' '));
            long_secrets.push(group_pan(&link.number, '-'));
        }
        if !link.security_code.is_empty() {
            short_secrets.push(link.security_code.clone());
        }
    }
    for api in &record.api_keys {
        if !api.key.is_empty() {
            short_secrets.push(api.key.clone());
        }
    }
    if !short_secrets.is_empty() {
        register_secret_values(&short_secrets, true);
    }
    if !long_secrets.is_empty() {
        register_secret_values(&long_secrets, false);
    }
}

fn group_pan(digits: &str, sep: char) -> String {
    let mut out = String::with_capacity(digits.len() + 4);
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && idx % 4 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

fn item_allows_agent(agents: Option<&[String]>, agent_id: Option<&str>) -> bool {
    match (agents, agent_id) {
        (None, _) | (_, None) => true,
        (Some(allowed), Some(id)) => allowed.iter().any(|entry| entry == id),
    }
}

fn validate_agent_scope(agents: Option<Vec<String>>) -> RuntimeResult<Option<Vec<String>>> {
    let Some(list) = agents else {
        return Ok(None);
    };
    if list.len() > VAULT_AGENT_SCOPE_LIMIT {
        return Err(OpenBotRuntimeError::LimitExceeded(format!(
            "vault item agent scope cannot exceed {VAULT_AGENT_SCOPE_LIMIT} agents"
        )));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(list.len());
    for raw in list {
        let id = raw.trim();
        if id.is_empty() || utf16_len(id) > INPUT_LIMITS.identifier {
            return Err(OpenBotRuntimeError::Validation(format!(
                "invalid agent identifier '{raw}' in vault scope"
            )));
        }
        if seen.insert(id.to_string()) {
            out.push(id.to_string());
        }
    }
    Ok(Some(out))
}

fn summarize_login(login: &StoredLogin) -> VaultItemSummary {
    VaultItemSummary::Login {
        id: login.id.clone(),
        website: login.website.clone(),
        username: login.username.clone(),
        notes: login.notes.clone(),
        has_password: !login.password.is_empty(),
        has_two_step: login.two_step_key.as_deref().is_some_and(|s| !s.is_empty()),
        added_by: login.added_by.clone(),
        agents: login.agents.clone(),
    }
}

fn summarize_card(card: &StoredCard) -> VaultItemSummary {
    let digits: String = card.number.chars().filter(|c| c.is_ascii_digit()).collect();
    let last4 = if digits.len() >= 4 {
        digits[digits.len() - 4..].to_string()
    } else {
        digits.clone()
    };
    VaultItemSummary::Card {
        id: card.id.clone(),
        label: card.label.clone(),
        name_on_card: card.name_on_card.clone(),
        brand: detect_card_brand(&digits),
        masked_number: mask_card_last4(&last4),
        last4,
        exp_month: card.exp_month,
        exp_year: card.exp_year,
        expiry: format!("{:02}/{:02}", card.exp_month, card.exp_year % 100),
        has_security_code: !card.security_code.is_empty(),
        billing_contact_id: card.billing_contact_id.clone(),
        fill_sites: card.fill_sites.iter().map(|s| s.page.clone()).collect(),
        added_by: card.added_by.clone(),
        agents: card.agents.clone(),
    }
}

fn summarize_contact(contact: &StoredContact) -> VaultItemSummary {
    VaultItemSummary::Contact {
        id: contact.id.clone(),
        fields: contact.fields.clone(),
        added_by: contact.added_by.clone(),
        agents: contact.agents.clone(),
    }
}

fn summarize_api_key(api_key: &StoredApiKey) -> VaultItemSummary {
    VaultItemSummary::ApiKey {
        id: api_key.id.clone(),
        name: api_key.name.clone(),
        notes: api_key.notes.clone(),
        has_key: !api_key.key.is_empty(),
        added_by: api_key.added_by.clone(),
        agents: api_key.agents.clone(),
    }
}

struct VaultInner {
    record: VaultRecord,
    status: VaultStatus,
    external_logins: Vec<ExternalLoginRef>,
    audit_log: Vec<VaultAuditEntry>,
}

/// Encrypted OpenBot Vault service managing logins, cards, contacts, `0600` API key files,
/// RFC 6238 TOTP generation, and one-time Link virtual payment cards.
pub struct VaultService {
    database: OpenBotDatabase,
    data_dir: PathBuf,
    vault_path: PathBuf,
    keys_dir: PathBuf,
    master_key: Zeroizing<[u8; 32]>,
    inner: RwLock<VaultInner>,
}

impl fmt::Debug for VaultService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultService")
            .field("data_dir", &self.data_dir)
            .field("vault_path", &self.vault_path)
            .field("keys_dir", &self.keys_dir)
            .field("master_key", &"[REDACTED]")
            .finish()
    }
}

impl VaultService {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        ensure_dir_mode_700(&data_dir)?;
        let master_key_path = data_dir.join(VAULT_MASTER_KEY_FILE_NAME);
        let vault_path = data_dir.join(VAULT_FILE_NAME);
        let keys_dir = data_dir.join(VAULT_KEYS_DIR_NAME);
        let master_key = load_or_create_master_key(&master_key_path)?;

        let (record, status) = if vault_path.exists() {
            let raw = fs::read_to_string(&vault_path)?;
            if raw.len() > MAX_VAULT_ENVELOPE_BYTES {
                (VaultRecord::default(), VaultStatus::Unreadable)
            } else {
                match serde_json::from_str::<VaultEnvelope>(&raw)
                    .map_err(OpenBotRuntimeError::Json)
                    .and_then(|env| decrypt_vault_envelope(&master_key, &env))
                {
                    Ok(rec) => (rec, VaultStatus::Ready),
                    Err(err) => {
                        tracing::warn!(error = %err, "Failed to decrypt OpenBot Vault envelope");
                        (VaultRecord::default(), VaultStatus::Unreadable)
                    }
                }
            }
        } else {
            (VaultRecord::default(), VaultStatus::Ready)
        };

        register_vault_secrets(&record);

        Ok(Arc::new(Self {
            database,
            data_dir,
            vault_path,
            keys_dir,
            master_key,
            inner: RwLock::new(VaultInner {
                record,
                status,
                external_logins: Vec::new(),
                audit_log: Vec::new(),
            }),
        }))
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn vault_path(&self) -> &Path {
        &self.vault_path
    }

    pub fn keys_dir(&self) -> &Path {
        &self.keys_dir
    }

    pub fn state(&self, agent_id: Option<&str>) -> VaultState {
        let guard = self.inner.read();
        VaultState {
            status: guard.status,
            items: if guard.status == VaultStatus::Ready {
                self.collect_summaries(&guard.record, agent_id)
            } else {
                Vec::new()
            },
            agent_scope: true,
        }
    }

    fn collect_summaries(
        &self,
        record: &VaultRecord,
        agent_id: Option<&str>,
    ) -> Vec<VaultItemSummary> {
        let mut out = Vec::new();
        for login in &record.logins {
            if item_allows_agent(login.agents.as_deref(), agent_id) {
                out.push(summarize_login(login));
            }
        }
        for card in &record.cards {
            if item_allows_agent(card.agents.as_deref(), agent_id) {
                out.push(summarize_card(card));
            }
        }
        for contact in &record.contacts {
            if item_allows_agent(contact.agents.as_deref(), agent_id) {
                out.push(summarize_contact(contact));
            }
        }
        for api_key in &record.api_keys {
            if item_allows_agent(api_key.agents.as_deref(), agent_id) {
                out.push(summarize_api_key(api_key));
            }
        }
        out
    }

    fn persist_record(&self, record: &VaultRecord) -> RuntimeResult<()> {
        let envelope = encrypt_vault_record(&self.master_key, record)?;
        write_json_file_atomically(&self.vault_path, &envelope, true)?;
        set_file_mode_600(&self.vault_path)?;
        register_vault_secrets(record);
        Ok(())
    }
}

impl VaultService {
    pub fn list_items(&self, agent_id: Option<&str>) -> Vec<VaultItemSummary> {
        let guard = self.inner.read();
        if guard.status != VaultStatus::Ready {
            return Vec::new();
        }
        self.collect_summaries(&guard.record, agent_id)
    }

    pub fn save_item(
        &self,
        input: VaultSaveInput,
        actor_agent_id: Option<&str>,
    ) -> RuntimeResult<VaultItemSummary> {
        let mut guard = self.inner.write();
        if guard.status != VaultStatus::Ready {
            return Err(OpenBotRuntimeError::Execution(
                "OpenBot Vault is not ready or unreadable".to_string(),
            ));
        }

        let mut next = guard.record.clone();
        let added_by = actor_agent_id.map(ToOwned::to_owned);

        let summary = match input {
            VaultSaveInput::Login {
                id,
                website,
                username,
                notes,
                password,
                two_step_key,
                agents,
            } => {
                let normalized_website = normalize_vault_website(&website)?;
                if utf16_len(&username) > VAULT_TEXT_LIMIT {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "login username exceeds {VAULT_TEXT_LIMIT} characters"
                    )));
                }
                if utf16_len(&notes) > VAULT_NOTES_LIMIT {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "login notes exceed {VAULT_NOTES_LIMIT} characters"
                    )));
                }
                let scoped_agents = validate_agent_scope(agents)?;
                let existing = id
                    .as_deref()
                    .and_then(|target_id| next.logins.iter().find(|l| l.id == target_id))
                    .cloned();
                if id.is_some() && existing.is_none() {
                    return Err(OpenBotRuntimeError::NotFound(format!(
                        "vault login '{}' not found",
                        id.unwrap_or_default()
                    )));
                }
                if let Some(ref ex) = existing {
                    if !item_allows_agent(ex.agents.as_deref(), actor_agent_id) {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "agent is not allowed to modify this vault login".to_string(),
                        ));
                    }
                }
                let final_password = match password {
                    Some(pw) => {
                        if pw.trim().is_empty() || utf16_len(&pw) > VAULT_PASSWORD_LIMIT {
                            return Err(OpenBotRuntimeError::Validation(format!(
                                "login password must be 1..={VAULT_PASSWORD_LIMIT} characters"
                            )));
                        }
                        pw
                    }
                    None => existing
                        .as_ref()
                        .map(|ex| ex.password.clone())
                        .ok_or_else(|| {
                            OpenBotRuntimeError::Validation(
                                "new vault login requires a password".to_string(),
                            )
                        })?,
                };
                let final_two_step = match two_step_key {
                    Some(raw) if raw.trim().is_empty() => None,
                    Some(raw) => Some(normalize_two_step_key(&raw)?),
                    None => existing.as_ref().and_then(|ex| ex.two_step_key.clone()),
                };
                let final_id = existing
                    .as_ref()
                    .map(|ex| ex.id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let stored = StoredLogin {
                    id: final_id.clone(),
                    website: normalized_website,
                    username: username.trim().to_string(),
                    password: final_password,
                    notes,
                    two_step_key: final_two_step,
                    added_by: existing
                        .as_ref()
                        .and_then(|ex| ex.added_by.clone())
                        .or(added_by),
                    agents: scoped_agents.or_else(|| existing.as_ref().and_then(|ex| ex.agents.clone())),
                };
                let summary = summarize_login(&stored);
                if let Some(slot) = next.logins.iter_mut().find(|l| l.id == final_id) {
                    *slot = stored;
                } else {
                    next.logins.push(stored);
                }
                summary
            }
            VaultSaveInput::Card {
                id,
                label,
                name_on_card,
                exp_month,
                exp_year,
                billing_contact_id,
                number,
                security_code,
                fill_sites,
                agents,
            } => {
                if utf16_len(&label) > VAULT_TEXT_LIMIT
                    || utf16_len(&name_on_card) > VAULT_TEXT_LIMIT
                {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "card text fields must not exceed {VAULT_TEXT_LIMIT} characters"
                    )));
                }
                if !(1..=12).contains(&exp_month) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "card expMonth must be 1..=12, got {exp_month}"
                    )));
                }
                if !(2000..=2099).contains(&exp_year) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "card expYear must be 2000..=2099, got {exp_year}"
                    )));
                }
                if let Some(ref contact_id) = billing_contact_id {
                    if !next.contacts.iter().any(|c| c.id == *contact_id) {
                        return Err(OpenBotRuntimeError::Validation(format!(
                            "billingContactId '{contact_id}' does not exist in vault"
                        )));
                    }
                }
                let scoped_agents = validate_agent_scope(agents)?;
                let existing = id
                    .as_deref()
                    .and_then(|target_id| next.cards.iter().find(|c| c.id == target_id))
                    .cloned();
                if id.is_some() && existing.is_none() {
                    return Err(OpenBotRuntimeError::NotFound(format!(
                        "vault card '{}' not found",
                        id.unwrap_or_default()
                    )));
                }
                let final_number = match number {
                    Some(raw) => {
                        let digits: String =
                            raw.chars().filter(|c| c.is_ascii_digit()).collect();
                        if !(12..=19).contains(&digits.len()) {
                            return Err(OpenBotRuntimeError::Validation(
                                "card number must contain 12..=19 digits".to_string(),
                            ));
                        }
                        digits
                    }
                    None => existing
                        .as_ref()
                        .map(|ex| ex.number.clone())
                        .ok_or_else(|| {
                            OpenBotRuntimeError::Validation(
                                "new vault card requires a card number".to_string(),
                            )
                        })?,
                };
                let final_cvv = match security_code {
                    Some(raw) => {
                        let trimmed = raw.trim();
                        if !trimmed.is_empty()
                            && (!(3..=4).contains(&trimmed.len())
                                || !trimmed.bytes().all(|b| b.is_ascii_digit()))
                        {
                            return Err(OpenBotRuntimeError::Validation(
                                "card security code must be 3 or 4 digits".to_string(),
                            ));
                        }
                        trimmed.to_string()
                    }
                    None => existing
                        .as_ref()
                        .map(|ex| ex.security_code.clone())
                        .unwrap_or_default(),
                };
                let final_sites = match fill_sites {
                    Some(sites) => {
                        let mut out = Vec::new();
                        for site in sites.into_iter().take(VAULT_FILL_SITE_LIMIT) {
                            let origin = canonical_https_origin(&site, "card fillSite")?;
                            if !out.iter().any(|e: &StoredCardFillSite| e.page == origin) {
                                out.push(StoredCardFillSite {
                                    page: origin.clone(),
                                    frames: vec![origin],
                                });
                            }
                        }
                        out
                    }
                    None => existing
                        .as_ref()
                        .map(|ex| ex.fill_sites.clone())
                        .unwrap_or_default(),
                };
                let final_id = existing
                    .as_ref()
                    .map(|ex| ex.id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let stored = StoredCard {
                    id: final_id.clone(),
                    label: label.trim().to_string(),
                    name_on_card: name_on_card.trim().to_string(),
                    number: final_number,
                    exp_month,
                    exp_year,
                    security_code: final_cvv,
                    billing_contact_id,
                    fill_sites: final_sites,
                    added_by: existing
                        .as_ref()
                        .and_then(|ex| ex.added_by.clone())
                        .or(added_by),
                    agents: scoped_agents.or_else(|| existing.as_ref().and_then(|ex| ex.agents.clone())),
                };
                let summary = summarize_card(&stored);
                if let Some(slot) = next.cards.iter_mut().find(|c| c.id == final_id) {
                    *slot = stored;
                } else {
                    next.cards.push(stored);
                }
                summary
            }
            VaultSaveInput::Contact { id, fields, agents } => {
                validate_contact_fields(&fields)?;
                let scoped_agents = validate_agent_scope(agents)?;
                let existing = id
                    .as_deref()
                    .and_then(|target_id| next.contacts.iter().find(|c| c.id == target_id))
                    .cloned();
                if id.is_some() && existing.is_none() {
                    return Err(OpenBotRuntimeError::NotFound(format!(
                        "vault contact '{}' not found",
                        id.unwrap_or_default()
                    )));
                }
                let final_id = existing
                    .as_ref()
                    .map(|ex| ex.id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let stored = StoredContact {
                    id: final_id.clone(),
                    fields,
                    added_by: existing
                        .as_ref()
                        .and_then(|ex| ex.added_by.clone())
                        .or(added_by),
                    agents: scoped_agents.or_else(|| existing.as_ref().and_then(|ex| ex.agents.clone())),
                };
                let summary = summarize_contact(&stored);
                if let Some(slot) = next.contacts.iter_mut().find(|c| c.id == final_id) {
                    *slot = stored;
                } else {
                    next.contacts.push(stored);
                }
                summary
            }
            VaultSaveInput::ApiKey {
                id,
                name,
                notes,
                key,
                agents,
            } => {
                let valid_name = validate_api_key_name(&name)?;
                if utf16_len(&notes) > VAULT_NOTES_LIMIT {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "API key notes exceed {VAULT_NOTES_LIMIT} characters"
                    )));
                }
                let scoped_agents = validate_agent_scope(agents)?;
                let existing = id
                    .as_deref()
                    .and_then(|target_id| next.api_keys.iter().find(|k| k.id == target_id))
                    .cloned();
                if id.is_some() && existing.is_none() {
                    return Err(OpenBotRuntimeError::NotFound(format!(
                        "vault API key '{}' not found",
                        id.unwrap_or_default()
                    )));
                }
                let final_key = match key {
                    Some(raw) => {
                        if raw.trim().is_empty() || utf16_len(&raw) > VAULT_API_KEY_LIMIT {
                            return Err(OpenBotRuntimeError::Validation(format!(
                                "API key value must be 1..={VAULT_API_KEY_LIMIT} characters"
                            )));
                        }
                        raw
                    }
                    None => existing
                        .as_ref()
                        .map(|ex| ex.key.clone())
                        .ok_or_else(|| {
                            OpenBotRuntimeError::Validation(
                                "new vault API key requires a key value".to_string(),
                            )
                        })?,
                };
                let final_id = existing
                    .as_ref()
                    .map(|ex| ex.id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let stored = StoredApiKey {
                    id: final_id.clone(),
                    name: valid_name,
                    key: final_key,
                    notes,
                    added_by: existing
                        .as_ref()
                        .and_then(|ex| ex.added_by.clone())
                        .or(added_by),
                    agents: scoped_agents.or_else(|| existing.as_ref().and_then(|ex| ex.agents.clone())),
                };
                let summary = summarize_api_key(&stored);
                if let Some(slot) = next.api_keys.iter_mut().find(|k| k.id == final_id) {
                    *slot = stored;
                } else {
                    next.api_keys.push(stored);
                }
                summary
            }
        };

        self.persist_record(&next)?;
        guard.record = next;
        Ok(summary)
    }
}

fn validate_contact_fields(fields: &VaultContactFields) -> RuntimeResult<()> {
    for (name, value) in [
        ("label", &fields.label),
        ("fullName", &fields.full_name),
        ("address", &fields.address),
        ("address2", &fields.address2),
        ("city", &fields.city),
        ("region", &fields.region),
        ("postalCode", &fields.postal_code),
        ("phone", &fields.phone),
        ("email", &fields.email),
    ] {
        if utf16_len(value) > VAULT_TEXT_LIMIT {
            return Err(OpenBotRuntimeError::Validation(format!(
                "contact {name} exceeds {VAULT_TEXT_LIMIT} characters"
            )));
        }
    }
    let country = fields.country.trim();
    if !country.is_empty() && (country.len() != 2 || !country.bytes().all(|b| b.is_ascii_alphabetic())) {
        return Err(OpenBotRuntimeError::Validation(format!(
            "contact country '{country}' must be an ISO 3166-1 alpha-2 code"
        )));
    }
    Ok(())
}

impl VaultService {
    pub fn delete_item(
        &self,
        id: &str,
        actor_agent_id: Option<&str>,
    ) -> RuntimeResult<VaultItemSummary> {
        let mut guard = self.inner.write();
        if guard.status != VaultStatus::Ready {
            return Err(OpenBotRuntimeError::Execution(
                "OpenBot Vault is not ready or unreadable".to_string(),
            ));
        }

        let mut next = guard.record.clone();
        let removed: VaultItemSummary = if let Some(pos) = next.logins.iter().position(|l| l.id == id) {
            if !item_allows_agent(next.logins[pos].agents.as_deref(), actor_agent_id) {
                return Err(OpenBotRuntimeError::Forbidden(
                    "agent is not allowed to delete this vault login".to_string(),
                ));
            }
            let item = next.logins.remove(pos);
            summarize_login(&item)
        } else if let Some(pos) = next.cards.iter().position(|c| c.id == id) {
            if !item_allows_agent(next.cards[pos].agents.as_deref(), actor_agent_id) {
                return Err(OpenBotRuntimeError::Forbidden(
                    "agent is not allowed to delete this vault card".to_string(),
                ));
            }
            let item = next.cards.remove(pos);
            summarize_card(&item)
        } else if let Some(pos) = next.contacts.iter().position(|c| c.id == id) {
            if !item_allows_agent(next.contacts[pos].agents.as_deref(), actor_agent_id) {
                return Err(OpenBotRuntimeError::Forbidden(
                    "agent is not allowed to delete this vault contact".to_string(),
                ));
            }
            let item = next.contacts.remove(pos);
            for card in &mut next.cards {
                if card.billing_contact_id.as_deref() == Some(id) {
                    card.billing_contact_id = None;
                }
            }
            summarize_contact(&item)
        } else if let Some(pos) = next.api_keys.iter().position(|k| k.id == id) {
            if !item_allows_agent(next.api_keys[pos].agents.as_deref(), actor_agent_id) {
                return Err(OpenBotRuntimeError::Forbidden(
                    "agent is not allowed to delete this vault API key".to_string(),
                ));
            }
            let item = next.api_keys.remove(pos);
            summarize_api_key(&item)
        } else {
            return Err(OpenBotRuntimeError::NotFound(format!(
                "vault item '{id}' not found"
            )));
        };

        self.persist_record(&next)?;
        guard.record = next;
        Ok(removed)
    }

    pub fn register_external_login(&self, entry: ExternalLoginRef) -> RuntimeResult<()> {
        if entry.id.trim().is_empty() || utf16_len(&entry.id) > INPUT_LIMITS.vault_login_id {
            return Err(OpenBotRuntimeError::Validation(
                "invalid external login id".to_string(),
            ));
        }
        let _ = normalize_vault_website(&entry.website)?;
        if let Some(ref pw) = entry.password {
            if !pw.is_empty() {
                register_secret_value(pw, true);
            }
        }
        if let Some(ref key) = entry.two_step_key {
            let _ = parse_two_step_key(key)?;
            register_secret_value(key, false);
        }
        let mut guard = self.inner.write();
        if let Some(slot) = guard.external_logins.iter_mut().find(|e| e.id == entry.id) {
            *slot = entry;
        } else {
            guard.external_logins.push(entry);
        }
        Ok(())
    }

    /// Lists Vault and external password-manager (1Password, Bitwarden, Proton Pass) logins
    /// matching `origin`. NEVER returns passwords or TOTP seeds.
    pub fn list_logins(
        &self,
        origin: &str,
        agent_id: Option<&str>,
    ) -> RuntimeResult<Vec<VaultLoginEntry>> {
        if origin.trim().is_empty() || parse_website_url(origin).is_none() {
            return Err(OpenBotRuntimeError::Validation(format!(
                "invalid origin '{origin}' for list_logins"
            )));
        }
        let guard = self.inner.read();
        let mut out = Vec::new();
        if guard.status == VaultStatus::Ready {
            for login in &guard.record.logins {
                if item_allows_agent(login.agents.as_deref(), agent_id)
                    && website_matches_origin(&login.website, origin)
                {
                    out.push(VaultLoginEntry {
                        id: login.id.clone(),
                        title: login_title(&login.website),
                        website: login.website.clone(),
                        username: if login.username.is_empty() {
                            None
                        } else {
                            Some(login.username.clone())
                        },
                        has_one_time_password: login
                            .two_step_key
                            .as_deref()
                            .is_some_and(|s| !s.is_empty()),
                        provider: LoginProviderKind::OpenBotVault,
                        provider_name: LoginProviderKind::OpenBotVault.display_name().to_string(),
                    });
                }
            }
        }
        for ext in &guard.external_logins {
            if website_matches_origin(&ext.website, origin) {
                out.push(VaultLoginEntry {
                    id: ext.id.clone(),
                    title: if ext.title.is_empty() {
                        login_title(&ext.website)
                    } else {
                        ext.title.clone()
                    },
                    website: ext.website.clone(),
                    username: ext.username.clone(),
                    has_one_time_password: ext
                        .two_step_key
                        .as_deref()
                        .is_some_and(|s| !s.is_empty()),
                    provider: ext.provider,
                    provider_name: ext.provider.display_name().to_string(),
                });
            }
        }
        Ok(out)
    }

    pub fn has_login(&self, login_id: &str) -> bool {
        let guard = self.inner.read();
        guard.record.logins.iter().any(|l| l.id == login_id)
            || guard.external_logins.iter().any(|e| e.id == login_id)
    }

    /// Resolves a password or live RFC 6238 TOTP code for an authorized login matching `origin`.
    pub fn resolve_login_secret(
        &self,
        login_id: &str,
        origin: &str,
        method: SecretFillMethod,
        agent_id: Option<&str>,
    ) -> RuntimeResult<Zeroizing<String>> {
        let guard = self.inner.read();
        if guard.status != VaultStatus::Ready {
            return Err(OpenBotRuntimeError::Execution(
                "OpenBot Vault is not ready".to_string(),
            ));
        }

        let now_ms = Utc::now().timestamp_millis().max(0) as u64;
        if let Some(login) = guard.record.logins.iter().find(|l| l.id == login_id) {
            if !item_allows_agent(login.agents.as_deref(), agent_id) {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "agent is not authorized to access vault login '{login_id}'"
                )));
            }
            if !website_matches_origin(&login.website, origin) {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "origin mismatch: vault login '{login_id}' is bound to '{}' and cannot be submitted to '{origin}'",
                    login.website
                )));
            }
            return match method {
                SecretFillMethod::Password => {
                    if login.password.is_empty() {
                        Err(OpenBotRuntimeError::NotFound(format!(
                            "vault login '{login_id}' has no password stored"
                        )))
                    } else {
                        Ok(Zeroizing::new(login.password.clone()))
                    }
                }
                SecretFillMethod::Authenticator | SecretFillMethod::Otp => {
                    let key = login.two_step_key.as_deref().ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!(
                            "vault login '{login_id}' has no twoStepKey configured"
                        ))
                    })?;
                    let generated = compute_totp_from_uri(&login.id, key, now_ms)?;
                    Ok(Zeroizing::new(generated.code))
                }
            };
        }

        if let Some(ext) = guard.external_logins.iter().find(|e| e.id == login_id) {
            if !website_matches_origin(&ext.website, origin) {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "origin mismatch: external login '{login_id}' ({}) is bound to '{}' and cannot be submitted to '{origin}'",
                    ext.provider.display_name(),
                    ext.website
                )));
            }
            return match method {
                SecretFillMethod::Password => ext
                    .password
                    .as_ref()
                    .filter(|p| !p.is_empty())
                    .map(|p| Zeroizing::new(p.clone()))
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!(
                            "external login '{login_id}' has no password available"
                        ))
                    }),
                SecretFillMethod::Authenticator | SecretFillMethod::Otp => {
                    let key = ext.two_step_key.as_deref().ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!(
                            "external login '{login_id}' has no authenticator key configured"
                        ))
                    })?;
                    let generated = compute_totp_from_uri(&ext.id, key, now_ms)?;
                    Ok(Zeroizing::new(generated.code))
                }
            };
        }

        Err(OpenBotRuntimeError::NotFound(format!(
            "vault login '{login_id}' not found"
        )))
    }

    pub fn all_totp_codes(&self, unix_ms: Option<u64>) -> RuntimeResult<Vec<VaultTwoStepCode>> {
        let guard = self.inner.read();
        if guard.status != VaultStatus::Ready {
            return Ok(Vec::new());
        }
        let at_ms = unix_ms.unwrap_or_else(|| Utc::now().timestamp_millis().max(0) as u64);
        let mut codes = Vec::new();
        for login in &guard.record.logins {
            if let Some(ref key) = login.two_step_key {
                if !key.is_empty() {
                    codes.push(compute_totp_from_uri(&login.id, key, at_ms)?);
                }
            }
        }
        Ok(codes)
    }
}

impl VaultService {
    /// Atomically writes an API key to `<data_dir>/vault-keys/<NAME>` with strict `0600` Unix permissions
    /// and path traversal sanitization, without ever echoing the raw key in its return value.
    pub fn save_api_key_file(
        &self,
        name: &str,
        value: &str,
        actor_agent_id: Option<&str>,
    ) -> RuntimeResult<SavedApiKeyFile> {
        let valid_name = validate_api_key_name(name)?;
        if value.trim().is_empty() || utf16_len(value) > VAULT_API_KEY_LIMIT {
            return Err(OpenBotRuntimeError::Validation(format!(
                "API key value must be 1..={VAULT_API_KEY_LIMIT} characters"
            )));
        }
        ensure_dir_mode_700(&self.keys_dir)?;
        let target_path = self.keys_dir.join(&valid_name);
        if target_path.parent() != Some(self.keys_dir.as_path()) {
            return Err(OpenBotRuntimeError::Forbidden(
                "API key path escapes vault-keys directory".to_string(),
            ));
        }

        register_secret_value(value, true);
        write_secret_file_atomically(&target_path, value.as_bytes())?;

        // Mirror into encrypted Vault so list_items stays synchronized.
        let mut guard = self.inner.write();
        if guard.status == VaultStatus::Ready {
            let mut next = guard.record.clone();
            if let Some(existing) = next.api_keys.iter_mut().find(|k| k.name == valid_name) {
                existing.key = value.to_string();
            } else {
                next.api_keys.push(StoredApiKey {
                    id: Uuid::new_v4().to_string(),
                    name: valid_name.clone(),
                    key: value.to_string(),
                    notes: String::new(),
                    added_by: actor_agent_id.map(ToOwned::to_owned),
                    agents: None,
                });
            }
            self.persist_record(&next)?;
            guard.record = next;
        }

        Ok(SavedApiKeyFile {
            name: valid_name,
            path: target_path.to_string_lossy().to_string(),
            size_bytes: value.len() as u64,
            updated_at: now_rfc3339_millis(),
        })
    }

    pub fn list_api_keys(&self) -> RuntimeResult<Vec<SavedApiKeyFile>> {
        if !self.keys_dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.keys_dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let file_name = entry.file_name();
            let Some(name_str) = file_name.to_str() else {
                continue;
            };
            if validate_api_key_name(name_str).is_err() {
                continue;
            }
            let meta = entry.metadata()?;
            let updated_at = meta
                .modified()
                .ok()
                .map(|t| {
                    let dt: chrono::DateTime<Utc> = t.into();
                    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
                })
                .unwrap_or_else(now_rfc3339_millis);
            entries.push(SavedApiKeyFile {
                name: name_str.to_string(),
                path: entry.path().to_string_lossy().to_string(),
                size_bytes: meta.len(),
                updated_at,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    pub fn remove_api_key(&self, name: &str) -> RuntimeResult<bool> {
        let valid_name = validate_api_key_name(name)?;
        let target_path = self.keys_dir.join(&valid_name);
        if target_path.parent() != Some(self.keys_dir.as_path()) {
            return Err(OpenBotRuntimeError::Forbidden(
                "API key path escapes vault-keys directory".to_string(),
            ));
        }

        let file_removed = match fs::remove_file(&target_path) {
            Ok(()) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(OpenBotRuntimeError::Io(err)),
        };

        let mut guard = self.inner.write();
        let mut vault_removed = false;
        if guard.status == VaultStatus::Ready {
            let before = guard.record.api_keys.len();
            let mut next = guard.record.clone();
            next.api_keys.retain(|k| k.name != valid_name);
            if next.api_keys.len() != before {
                vault_removed = true;
                self.persist_record(&next)?;
                guard.record = next;
            }
        }
        Ok(file_removed || vault_removed)
    }

    /// Requests a single-use Link virtual payment card.
    /// Strictly enforces `https://` checkout URL, `>= 100`-char purchase context justification,
    /// positive amount, valid ISO 4217 currency, and masks the issued card as `**** **** **** 4242`.
    pub fn request_link_payment(
        &self,
        request: LinkPaymentRequest,
    ) -> RuntimeResult<LinkPaymentResult> {
        if request.amount <= 0 || request.amount > LINK_PAYMENT_MAX_AMOUNT_MINOR {
            return Err(OpenBotRuntimeError::Validation(format!(
                "link payment amount must be between 1 and {LINK_PAYMENT_MAX_AMOUNT_MINOR} minor units"
            )));
        }
        let currency = match request.currency.as_deref() {
            None => "USD".to_string(),
            Some(raw) => {
                let c = raw.trim().to_ascii_uppercase();
                if c.len() != 3 || !c.bytes().all(|b| b.is_ascii_alphabetic()) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "invalid ISO 4217 currency code '{raw}'"
                    )));
                }
                c
            }
        };
        let merchant_name = request.merchant_name.trim();
        if merchant_name.is_empty() || utf16_len(merchant_name) > 200 {
            return Err(OpenBotRuntimeError::Validation(
                "merchantName must be between 1 and 200 characters".to_string(),
            ));
        }
        let merchant_origin = canonical_https_origin(&request.merchant_url, "Link payment merchantUrl")?;
        let trimmed_context = request.context.trim();
        let context_len = utf16_len(trimmed_context);
        if context_len < LINK_PAYMENT_MIN_CONTEXT_CHARS {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Link payment context is too short ({context_len} chars): must be at least {LINK_PAYMENT_MIN_CONTEXT_CHARS} characters (minimum 100)"
            )));
        }
        if context_len > LINK_PAYMENT_MAX_CONTEXT_CHARS {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Link payment context exceeds maximum {LINK_PAYMENT_MAX_CONTEXT_CHARS} characters"
            )));
        }

        if !request.approved {
            let now_ms = Utc::now().timestamp_millis().max(0) as u64;
            return Err(OpenBotRuntimeError::ApprovalRequired {
                action: "request_link_payment".to_string(),
                gate_id: format!("link-gate-{}", Uuid::new_v4()),
                expires_at_ms: now_ms + APPROVAL_TTL_MS,
                message: format!(
                    "User approval required in Link to issue one-time virtual card for {merchant_name} ({amount} {currency})",
                    amount = request.amount
                ),
            });
        }

        let mut guard = self.inner.write();
        if guard.status != VaultStatus::Ready {
            return Err(OpenBotRuntimeError::Execution(
                "OpenBot Vault is not ready".to_string(),
            ));
        }

        let pan = generate_virtual_card_pan_4242()?;
        let cvv = generate_virtual_cvv()?;
        let now = Utc::now();
        let valid_until = (now + Duration::minutes(30)).to_rfc3339_opts(SecondsFormat::Millis, true);
        let exp_month: u8 = 12;
        let exp_year: u16 = ((now.format("%Y").to_string().parse::<u16>().unwrap_or(2026)) + 2).min(2099);
        let card_id = format!("{LINK_CARD_ID_PREFIX}{}", Uuid::new_v4());
        let last4 = "4242".to_string();

        let stored = StoredLinkCard {
            card_id: card_id.clone(),
            number: pan.as_str().to_string(),
            security_code: cvv.as_str().to_string(),
            brand: Some(VaultCardBrand::Visa),
            last4: last4.clone(),
            name_on_card: "OpenBot Link Virtual Card".to_string(),
            exp_month,
            exp_year,
            amount: request.amount,
            currency: currency.clone(),
            merchant_name: merchant_name.to_string(),
            merchant_url: request.merchant_url.trim().to_string(),
            context: trimmed_context.to_string(),
            valid_until: valid_until.clone(),
            consumed: false,
            consumed_at: None,
        };

        let mut next = guard.record.clone();
        next.link_cards.push(stored);
        self.persist_record(&next)?;
        guard.record = next;

        tracing::info!(
            card_id = %card_id,
            masked_number = %mask_card_last4(&last4),
            merchant_origin = %merchant_origin,
            amount = request.amount,
            currency = %currency,
            agent_id = ?request.agent_id,
            "Issued single-use Link virtual card"
        );

        Ok(LinkPaymentResult {
            status: "approved".to_string(),
            card_id,
            brand: Some(VaultCardBrand::Visa),
            masked_number: mask_card_last4(&last4),
            last4,
            exp_month,
            exp_year,
            valid_until,
            amount: request.amount,
            currency,
            merchant_name: merchant_name.to_string(),
            merchant_origin,
            one_time: true,
        })
    }
}

impl VaultService {
    /// Controlled internal lookup returning card material ONLY for authorized HTTPS `fill_card`
    /// execution. Consumes one-time Link cards atomically (refusing replay) and records masked
    /// audit metadata without ever exposing raw PAN/CVV to agent responses or logs.
    pub fn get_card_for_fill(
        &self,
        card_id: &str,
        page_url: &str,
        agent_id: Option<&str>,
        approved: bool,
    ) -> RuntimeResult<CardForFill> {
        let page_origin = canonical_https_origin(page_url, "Card fill").map_err(|_| {
            OpenBotRuntimeError::Forbidden(
                "Card fill requires an HTTPS secure page. Use request_takeover for non-HTTPS sites."
                    .to_string(),
            )
        })?;

        let mut guard = self.inner.write();
        if guard.status != VaultStatus::Ready {
            return Err(OpenBotRuntimeError::Execution(
                "OpenBot Vault is not ready".to_string(),
            ));
        }

        if card_id.starts_with(LINK_CARD_ID_PREFIX) {
            let mut next = guard.record.clone();
            let link_idx = next
                .link_cards
                .iter()
                .position(|c| c.card_id == card_id)
                .ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!("Link virtual card '{card_id}' not found"))
                })?;

            if next.link_cards[link_idx].consumed {
                let consumed_at = next.link_cards[link_idx]
                    .consumed_at
                    .as_deref()
                    .unwrap_or("previously");
                return Err(OpenBotRuntimeError::Conflict(format!(
                    "one-time Link virtual card '{card_id}' was already consumed ({consumed_at}) and cannot be replayed"
                )));
            }
            if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&next.link_cards[link_idx].valid_until) {
                if Utc::now() > expiry.with_timezone(&Utc) {
                    return Err(OpenBotRuntimeError::Conflict(format!(
                        "one-time Link virtual card '{card_id}' expired at {}",
                        next.link_cards[link_idx].valid_until
                    )));
                }
            }
            if next.link_cards[link_idx].context.trim().len() < LINK_PAYMENT_MIN_CONTEXT_CHARS {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "Link card '{card_id}' missing valid purchase authorization context"
                )));
            }
            if !website_matches_origin(&next.link_cards[link_idx].merchant_url, page_url) {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "Link card '{card_id}' is bound to merchant '{}' ({}) and cannot be filled on '{page_origin}'",
                    next.link_cards[link_idx].merchant_name,
                    next.link_cards[link_idx].merchant_url
                )));
            }
            if !approved {
                let now_ms = Utc::now().timestamp_millis().max(0) as u64;
                return Err(OpenBotRuntimeError::ApprovalRequired {
                    action: "fill_card".to_string(),
                    gate_id: format!("card-gate-{}", Uuid::new_v4()),
                    expires_at_ms: now_ms + APPROVAL_TTL_MS,
                    message: format!(
                        "Owner approval required to fill Link card ({} {}) on {page_origin}",
                        next.link_cards[link_idx].amount,
                        next.link_cards[link_idx].currency
                    ),
                });
            }

            let now_ts = now_rfc3339_millis();
            next.link_cards[link_idx].consumed = true;
            next.link_cards[link_idx].consumed_at = Some(now_ts.clone());
            let card_snapshot = next.link_cards[link_idx].clone();
            self.persist_record(&next)?;
            guard.record = next;

            let masked = mask_card_last4(&card_snapshot.last4);
            let audit = VaultAuditEntry {
                timestamp: now_ts,
                action: "fill_card".to_string(),
                card_id: card_snapshot.card_id.clone(),
                masked_number: masked.clone(),
                last4: card_snapshot.last4.clone(),
                page_origin: page_origin.clone(),
                agent_id: agent_id.map(ToOwned::to_owned),
                one_time: true,
            };
            tracing::info!(
                card_id = %audit.card_id,
                masked_number = %audit.masked_number,
                last4 = %audit.last4,
                page_origin = %audit.page_origin,
                agent_id = ?audit.agent_id,
                one_time = true,
                "Authorized one-time Link virtual card lookup for HTTPS fill_card"
            );
            guard.audit_log.push(audit);

            return Ok(CardForFill {
                card_id: card_snapshot.card_id.clone(),
                masked_number: masked,
                last4: card_snapshot.last4.clone(),
                brand: card_snapshot.brand,
                name_on_card: card_snapshot.name_on_card.clone(),
                exp_month: card_snapshot.exp_month,
                exp_year: card_snapshot.exp_year,
                one_time: true,
                number: Zeroizing::new(card_snapshot.number.clone()),
                security_code: Zeroizing::new(card_snapshot.security_code.clone()),
            });
        }

        let mut next = guard.record.clone();
        let card_idx = next
            .cards
            .iter()
            .position(|c| c.id == card_id)
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("vault card '{card_id}' not found"))
            })?;

        if !item_allows_agent(next.cards[card_idx].agents.as_deref(), agent_id) {
            return Err(OpenBotRuntimeError::Forbidden(format!(
                "agent is not authorized to use vault card '{card_id}'"
            )));
        }

        let already_approved = next.cards[card_idx]
            .fill_sites
            .iter()
            .any(|s| s.page == page_origin || s.frames.iter().any(|f| f == &page_origin));
        if !already_approved && !approved {
            let now_ms = Utc::now().timestamp_millis().max(0) as u64;
            return Err(OpenBotRuntimeError::ApprovalRequired {
                action: "fill_card".to_string(),
                gate_id: format!("card-gate-{}", Uuid::new_v4()),
                expires_at_ms: now_ms + APPROVAL_TTL_MS,
                message: format!(
                    "The first fill of card '{card_id}' on {page_origin} requires owner approval"
                ),
            });
        }

        if !already_approved {
            next.cards[card_idx].fill_sites.push(StoredCardFillSite {
                page: page_origin.clone(),
                frames: vec![page_origin.clone()],
            });
            if next.cards[card_idx].fill_sites.len() > VAULT_FILL_SITE_LIMIT {
                let drop_count = next.cards[card_idx].fill_sites.len() - VAULT_FILL_SITE_LIMIT;
                next.cards[card_idx].fill_sites.drain(0..drop_count);
            }
            self.persist_record(&next)?;
            guard.record = next;
        }

        let stored = guard.record.cards[card_idx].clone();
        let digits: String = stored.number.chars().filter(|c| c.is_ascii_digit()).collect();
        let last4 = if digits.len() >= 4 {
            digits[digits.len() - 4..].to_string()
        } else {
            digits.clone()
        };
        let masked = mask_card_last4(&last4);
        let audit = VaultAuditEntry {
            timestamp: now_rfc3339_millis(),
            action: "fill_card".to_string(),
            card_id: stored.id.clone(),
            masked_number: masked.clone(),
            last4: last4.clone(),
            page_origin: page_origin.clone(),
            agent_id: agent_id.map(ToOwned::to_owned),
            one_time: false,
        };
        tracing::info!(
            card_id = %audit.card_id,
            masked_number = %audit.masked_number,
            last4 = %audit.last4,
            page_origin = %audit.page_origin,
            agent_id = ?audit.agent_id,
            one_time = false,
            "Authorized saved Vault card lookup for HTTPS fill_card"
        );
        guard.audit_log.push(audit);

        Ok(CardForFill {
            card_id: stored.id.clone(),
            masked_number: masked,
            last4,
            brand: detect_card_brand(&digits),
            name_on_card: stored.name_on_card.clone(),
            exp_month: stored.exp_month,
            exp_year: stored.exp_year,
            one_time: false,
            number: Zeroizing::new(digits),
            security_code: Zeroizing::new(stored.security_code.clone()),
        })
    }

    pub fn list_link_cards(&self) -> Vec<LinkPaymentResult> {
        let guard = self.inner.read();
        guard
            .record
            .link_cards
            .iter()
            .map(|c| LinkPaymentResult {
                status: if c.consumed {
                    "consumed".to_string()
                } else {
                    "approved".to_string()
                },
                card_id: c.card_id.clone(),
                brand: c.brand,
                masked_number: mask_card_last4(&c.last4),
                last4: c.last4.clone(),
                exp_month: c.exp_month,
                exp_year: c.exp_year,
                valid_until: c.valid_until.clone(),
                amount: c.amount,
                currency: c.currency.clone(),
                merchant_name: c.merchant_name.clone(),
                merchant_origin: canonical_https_origin(&c.merchant_url, "Link card")
                    .unwrap_or_else(|_| c.merchant_url.clone()),
                one_time: true,
            })
            .collect()
    }

    pub fn audit_log(&self) -> Vec<VaultAuditEntry> {
        self.inner.read().audit_log.clone()
    }

    pub fn reset(&self) -> RuntimeResult<VaultState> {
        let mut guard = self.inner.write();
        if self.vault_path.exists() {
            fs::remove_file(&self.vault_path)?;
        }
        guard.record = VaultRecord::default();
        guard.status = VaultStatus::Ready;
        guard.audit_log.clear();
        Ok(VaultState {
            status: VaultStatus::Ready,
            items: Vec::new(),
            agent_scope: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_vault() -> (TempDir, Arc<VaultService>) {
        let dir = TempDir::new().expect("temp dir");
        let db = OpenBotDatabase::open_in_memory().expect("in-memory db");
        let vault = VaultService::open(db, dir.path()).expect("open vault");
        (dir, vault)
    }

    #[test]
    fn rfc6238_totp_test_vector_sha1() {
        // RFC 6238 Appendix B: secret = "12345678901234567890" (Base32: GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ)
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(compute_totp(secret, 59, 30, 8).unwrap(), "94287082");
        assert_eq!(compute_totp(secret, 59, 30, 6).unwrap(), "287082");
        assert_eq!(compute_totp(secret, 1_111_111_109, 30, 8).unwrap(), "07081804");
        assert_eq!(compute_totp(secret, 1_234_567_890, 30, 8).unwrap(), "89005924");

        let uri = "otpauth://totp/Acme:alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&algorithm=SHA1&digits=6&period=30";
        let code = compute_totp_from_uri("login-1", uri, 59_000).unwrap();
        assert_eq!(code.code, "287082");
        assert_eq!(code.period, 30);
    }

    #[test]
    fn encrypted_vault_persists_masks_secrets_and_reloads() {
        let (dir, vault) = test_vault();
        let saved = vault
            .save_item(
                VaultSaveInput::Login {
                    id: None,
                    website: "https://app.acme.test/login".to_string(),
                    username: "alice@acme.test".to_string(),
                    notes: "prod account".to_string(),
                    password: Some("SuperSecretPass!987".to_string()),
                    two_step_key: Some("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".to_string()),
                    agents: None,
                },
                Some("agent-1"),
            )
            .expect("save login");

        let disk_raw = fs::read_to_string(vault.vault_path()).expect("read vault file");
        assert!(!disk_raw.contains("SuperSecretPass!987"));
        assert!(!disk_raw.contains("GEZDGNBVGY3TQOJQ"));

        let listed = vault.list_items(Some("agent-1"));
        assert_eq!(listed.len(), 1);
        let listed_json = serde_json::to_string(&listed).unwrap();
        assert!(!listed_json.contains("SuperSecretPass!987"));
        assert!(!listed_json.contains("GEZDGNBVGY3TQOJQ"));

        let db2 = OpenBotDatabase::open_in_memory().expect("db2");
        let reloaded = VaultService::open(db2, dir.path()).expect("reload vault");
        let logins = reloaded
            .list_logins("https://sub.acme.test/auth", Some("agent-1"))
            .expect("list logins");
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].id, saved.id());
        assert!(logins[0].has_one_time_password);

        let pw = reloaded
            .resolve_login_secret(
                saved.id(),
                "https://app.acme.test/login",
                SecretFillMethod::Password,
                Some("agent-1"),
            )
            .expect("resolve password");
        assert_eq!(pw.as_str(), "SuperSecretPass!987");

        let mismatch = reloaded.resolve_login_secret(
            saved.id(),
            "https://evil.example/login",
            SecretFillMethod::Password,
            Some("agent-1"),
        );
        assert!(matches!(mismatch, Err(OpenBotRuntimeError::Forbidden(_))));
    }

    #[test]
    fn api_key_files_enforce_0600_and_block_traversal() {
        let (_dir, vault) = test_vault();
        assert!(vault.save_api_key_file("../escape", "sk-1234", None).is_err());
        assert!(vault.save_api_key_file("a/b", "sk-1234", None).is_err());
        assert!(vault.save_api_key_file(".hidden", "sk-1234", None).is_err());

        let saved = vault
            .save_api_key_file("OPENAI_API_KEY", "sk-secret-value-999", Some("agent-1"))
            .expect("save api key");
        let path = PathBuf::from(&saved.path);
        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "sk-secret-value-999");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let keys = vault.list_api_keys().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "OPENAI_API_KEY");
        assert!(vault.remove_api_key("OPENAI_API_KEY").unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn link_payment_enforces_https_context_100_and_single_use_replay_guard() {
        let (_dir, vault) = test_vault();
        let short_err = vault.request_link_payment(LinkPaymentRequest {
            amount: 500,
            currency: Some("USD".to_string()),
            merchant_name: "Acme".to_string(),
            merchant_url: "https://pay.acme.test/checkout".to_string(),
            context: "too short".to_string(),
            approved: true,
            agent_id: Some("agent-1".to_string()),
        });
        assert!(matches!(short_err, Err(OpenBotRuntimeError::Validation(_))));

        let http_ctx = "x".repeat(120);
        let http_err = vault.request_link_payment(LinkPaymentRequest {
            amount: 500,
            currency: Some("USD".to_string()),
            merchant_name: "Acme".to_string(),
            merchant_url: "http://pay.acme.test/checkout".to_string(),
            context: http_ctx.clone(),
            approved: true,
            agent_id: Some("agent-1".to_string()),
        });
        assert!(matches!(http_err, Err(OpenBotRuntimeError::Validation(_))));

        let issued = vault
            .request_link_payment(LinkPaymentRequest {
                amount: 2500,
                currency: Some("USD".to_string()),
                merchant_name: "Acme".to_string(),
                merchant_url: "https://pay.acme.test/checkout".to_string(),
                context: http_ctx,
                approved: true,
                agent_id: Some("agent-1".to_string()),
            })
            .expect("issue link card");
        assert_eq!(issued.last4, "4242");
        assert_eq!(issued.masked_number, "**** **** **** 4242");
        assert!(issued.one_time);

        let card = vault
            .get_card_for_fill(
                &issued.card_id,
                "https://pay.acme.test/checkout",
                Some("agent-1"),
                true,
            )
            .expect("first card fill lookup");
        assert!(is_luhn_valid(card.pan_for_browser_fill()));
        assert!(card.pan_for_browser_fill().ends_with("4242"));
        assert!(!format!("{card:?}").contains(card.pan_for_browser_fill()));

        let replay = vault.get_card_for_fill(
            &issued.card_id,
            "https://pay.acme.test/checkout",
            Some("agent-1"),
            true,
        );
        assert!(matches!(replay, Err(OpenBotRuntimeError::Conflict(_))));
        assert_eq!(vault.audit_log().len(), 1);
    }

    #[test]
    fn registrable_domain_and_website_matches_origin_isolate_ips_and_public_suffixes() {
        // IP literals, numeric TLDs, single-label hosts, empty labels, and bare public suffixes -> None
        for non_registrable in [
            "",
            "localhost",
            "127.0.0.1",
            "203.0.0.1",
            "192.168.1.1",
            "10.0.1.1",
            "[::1]",
            "::1",
            "2001:db8::1",
            "1.2.3.400",
            "foo..bar.com",
            ".example.com",
            "example.com..",
            "co.uk",
            "co.kr",
            "co.za",
            "co.in",
            "github.io",
            "pages.github.io",
            "pages.dev",
            "vercel.app",
            "s3.amazonaws.com",
            "s3.us-east-1.amazonaws.com",
            "s3-us-west-2.amazonaws.com",
            "blob.core.windows.net",
            "storage.googleapis.com",
        ] {
            assert_eq!(
                registrable_domain(non_registrable),
                None,
                "expected None for non-registrable host '{non_registrable}'"
            );
        }

        // Valid registrable domains (standard, ICANN ccTLD, PSL private multi-tenant, regional cloud, FQDN dot)
        assert_eq!(
            registrable_domain("checkout.shop.test"),
            Some("shop.test".to_string())
        );
        assert_eq!(
            registrable_domain("checkout.shop.test."),
            Some("shop.test".to_string())
        );
        assert_eq!(
            registrable_domain("checkout.victim.github.io"),
            Some("victim.github.io".to_string())
        );
        assert_eq!(
            registrable_domain("victim.pages.dev"),
            Some("victim.pages.dev".to_string())
        );
        assert_eq!(
            registrable_domain("victim.vercel.app"),
            Some("victim.vercel.app".to_string())
        );
        assert_eq!(
            registrable_domain("victim.s3.amazonaws.com"),
            Some("victim.s3.amazonaws.com".to_string())
        );
        assert_eq!(
            registrable_domain("victim.s3.us-east-1.amazonaws.com"),
            Some("victim.s3.us-east-1.amazonaws.com".to_string())
        );
        assert_eq!(
            registrable_domain("victim.s3-us-west-2.amazonaws.com"),
            Some("victim.s3-us-west-2.amazonaws.com".to_string())
        );
        assert_eq!(
            registrable_domain("victim.blob.core.windows.net"),
            Some("victim.blob.core.windows.net".to_string())
        );
        assert_eq!(
            registrable_domain("merchant.co.kr"),
            Some("merchant.co.kr".to_string())
        );
        assert_eq!(
            registrable_domain("merchant.co.za"),
            Some("merchant.co.za".to_string())
        );
        assert_eq!(
            registrable_domain("merchant.co.in"),
            Some("merchant.co.in".to_string())
        );

        // Cross-host exploit pairs MUST NOT match
        let cross_host_pairs = [
            ("https://127.0.0.1/pay", "https://203.0.0.1/pay"),
            ("https://192.168.1.1/pay", "https://10.0.1.1/pay"),
            ("https://[::1]/login", "https://[2001:db8::1]/login"),
            ("https://victim.github.io/pay", "https://attacker.github.io/pay"),
            ("https://victim.pages.dev/pay", "https://attacker.pages.dev/pay"),
            ("https://victim.vercel.app/pay", "https://attacker.vercel.app/pay"),
            ("https://victim.s3.amazonaws.com/pay", "https://attacker.s3.amazonaws.com/pay"),
            (
                "https://victim.s3.us-east-1.amazonaws.com/pay",
                "https://attacker.s3.us-east-1.amazonaws.com/pay",
            ),
            ("https://merchant.co.kr/pay", "https://attacker.co.kr/pay"),
            ("https://merchant.co.za/pay", "https://attacker.co.za/pay"),
            ("https://merchant.co.in/pay", "https://attacker.co.in/pay"),
            ("https://co.uk/login", "https://victim.co.uk/login"),
            ("https://github.io/login", "https://victim.github.io/login"),
        ];
        for (saved, page) in cross_host_pairs {
            assert!(
                !website_matches_origin(saved, page),
                "cross-host origins MUST NOT match: saved='{saved}' vs page='{page}'"
            );
        }

        // Positive controls MUST match
        let valid_pairs = [
            ("https://127.0.0.1/pay", "https://127.0.0.1/other"),
            ("https://[::1]/login", "https://[::1]/dashboard"),
            ("https://checkout.shop.test/pay", "https://pay.shop.test/confirm"),
            (
                "https://checkout.victim.github.io/pay",
                "https://pay.victim.github.io/confirm",
            ),
            (
                "https://shop.merchant.co.kr/pay",
                "https://billing.merchant.co.kr/confirm",
            ),
            ("https://app.github.io/login", "https://app.github.io/auth"),
        ];
        for (saved, page) in valid_pairs {
            assert!(
                website_matches_origin(saved, page),
                "same-origin / same-tenant origins MUST match: saved='{saved}' vs page='{page}'"
            );
        }

        // End-to-end VaultService login + Link card isolation
        let (_dir, vault) = test_vault();
        let ctx = "x".repeat(120);
        for (saved_url, evil_url, same_url) in [
            (
                "https://127.0.0.1/login",
                "https://203.0.0.1/login",
                "https://127.0.0.1/checkout",
            ),
            (
                "https://victim.github.io/login",
                "https://attacker.github.io/login",
                "https://checkout.victim.github.io/pay",
            ),
            (
                "https://bank.co.kr/login",
                "https://evil.co.kr/login",
                "https://pay.bank.co.kr/checkout",
            ),
            (
                "https://victim.s3.us-east-1.amazonaws.com/pay",
                "https://attacker.s3.us-east-1.amazonaws.com/pay",
                "https://sub.victim.s3.us-east-1.amazonaws.com/pay",
            ),
        ] {
            let saved_login = vault
                .save_item(
                    VaultSaveInput::Login {
                        id: None,
                        website: saved_url.to_string(),
                        username: "alice".to_string(),
                        notes: String::new(),
                        password: Some("SecretPass!123".to_string()),
                        two_step_key: None,
                        agents: None,
                    },
                    Some("agent-1"),
                )
                .expect("save login");
            assert!(
                vault
                    .list_logins(evil_url, Some("agent-1"))
                    .expect("list evil logins")
                    .iter()
                    .all(|l| l.id != saved_login.id()),
                "list_logins leaked {saved_url} onto {evil_url}"
            );
            assert!(
                matches!(
                    vault.resolve_login_secret(
                        saved_login.id(),
                        evil_url,
                        SecretFillMethod::Password,
                        Some("agent-1"),
                    ),
                    Err(OpenBotRuntimeError::Forbidden(_))
                ),
                "resolve_login_secret allowed {saved_url} onto {evil_url}"
            );
            assert!(
                vault
                    .resolve_login_secret(
                        saved_login.id(),
                        same_url,
                        SecretFillMethod::Password,
                        Some("agent-1"),
                    )
                    .is_ok(),
                "resolve_login_secret should succeed for {saved_url} on {same_url}"
            );

            let card = vault
                .request_link_payment(LinkPaymentRequest {
                    amount: 1000,
                    currency: Some("USD".to_string()),
                    merchant_name: "Shop".to_string(),
                    merchant_url: saved_url.to_string(),
                    context: ctx.clone(),
                    approved: true,
                    agent_id: Some("agent-1".to_string()),
                })
                .expect("issue link card");
            assert!(
                matches!(
                    vault.get_card_for_fill(&card.card_id, evil_url, Some("agent-1"), true),
                    Err(OpenBotRuntimeError::Forbidden(_))
                ),
                "Link card bound to {saved_url} must be forbidden on {evil_url}"
            );
            assert!(
                vault
                    .get_card_for_fill(&card.card_id, same_url, Some("agent-1"), true)
                    .is_ok(),
                "Link card bound to {saved_url} must still succeed on {same_url} after forbidden rejection"
            );
        }
    }
}
