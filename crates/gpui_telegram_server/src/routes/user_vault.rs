use crate::auth::{hash_password, verify_password, MAX_PASSWORD_BYTES};
use crate::db::{
    DeviceSyncApproveOutcome, DeviceSyncDenyOutcome, DeviceSyncPollOutcome, UserVaultUpsertOutcome,
};
use crate::routes::{err_resp, map_telegram_err};
use crate::server::{AppState, AuthBearer};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use base64::{engine::general_purpose::STANDARD as BASE64_STD, Engine as _};
use chrono::Utc;
use gpui_telegram_proto::user_vault::{
    ApproveDeviceSyncRequest, Argon2KdfParamsDto, AuthCustodyUpdateResponse,
    ChangePasswordRequest, DeviceSyncActionResponse, DeviceSyncRequestBody,
    KdfParamsResponse, PendingDeviceSyncListResponse, UpgradeSplitKeyRequest,
    UpsertUserVaultItemRequest, UserVaultEnvelopeResponse, UserVaultKeyEnvelopeDto,
    DEVICE_SYNC_CODE_DOMAIN_V1, USER_VAULT_CATEGORY_AI_OAUTH, USER_VAULT_CATEGORY_BROWSER_COOKIE,
    USER_VAULT_CATEGORY_PASSWORD, USER_VAULT_KDF_ALG_ARGON2ID,
    USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2, USER_VAULT_KDF_ALG_SPLIT_KEY_V2,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

const MAX_VAULT_CIPHERTEXT_BYTES: usize = 24 * 1024 * 1024;
const MAX_VAULT_NONCE_BYTES: usize = 128;
const MAX_VAULT_ITEM_KEY_LEN: usize = 128;
const DEVICE_SYNC_TTL_SECS: i64 = 600;
const MAX_DEVICE_NAME_LEN: usize = 128;

fn validate_category(category: &str) -> Result<&'static str, Response> {
    match category.trim() {
        USER_VAULT_CATEGORY_PASSWORD => Ok(USER_VAULT_CATEGORY_PASSWORD),
        USER_VAULT_CATEGORY_BROWSER_COOKIE => Ok(USER_VAULT_CATEGORY_BROWSER_COOKIE),
        USER_VAULT_CATEGORY_AI_OAUTH => Ok(USER_VAULT_CATEGORY_AI_OAUTH),
        _ => Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_category",
            "category must be one of: password, browser_cookie, ai_oauth",
        )),
    }
}

fn validate_item_key(item_key: &str) -> Result<&str, Response> {
    let trimmed = item_key.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_VAULT_ITEM_KEY_LEN {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_item_key",
            "item_key must be 1..=128 characters",
        ));
    }
    if trimmed.contains("..")
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_item_key",
            "item_key contains unsupported characters",
        ));
    }
    Ok(trimmed)
}

fn decode_b64_bounded(
    raw: &str,
    field: &str,
    min_bytes: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, Response> {
    let trimmed = raw.trim();
    if trimmed.is_empty() && min_bytes == 0 {
        return Ok(Vec::new());
    }
    let decoded = BASE64_STD.decode(trimmed).map_err(|_| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_base64",
            &format!("{field} must be valid standard base64"),
        )
    })?;
    if decoded.len() < min_bytes || decoded.len() > max_bytes {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_payload_size",
            &format!("{field} decoded byte length must be within {min_bytes}..={max_bytes}"),
        ));
    }
    Ok(decoded)
}

pub(crate) fn validate_kdf_params(params: Argon2KdfParamsDto) -> Result<(), Response> {
    if !(1024..=1_048_576).contains(&params.m_cost_kib)
        || !(1..=64).contains(&params.t_cost)
        || !(1..=16).contains(&params.p_cost)
    {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_kdf_params",
            "Argon2id KDF cost parameters are out of allowed bounds",
        ));
    }
    Ok(())
}

pub(crate) fn validate_split_key_salt_b64(raw: &str) -> Result<(), Response> {
    decode_b64_bounded(raw, "auth_salt_b64", 16, 16)?;
    Ok(())
}

pub(crate) fn validate_split_key_verifier_b64(raw: &str) -> Result<(), Response> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_auth_verifier",
            "auth_verifier_b64 cannot be empty",
        ));
    }
    if trimmed.as_bytes().len() > MAX_PASSWORD_BYTES {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_auth_verifier",
            "auth_verifier_b64 exceeds maximum allowed length",
        ));
    }
    decode_b64_bounded(trimmed, "auth_verifier_b64", 32, 128)?;
    Ok(())
}

fn validate_envelope(env: &UserVaultKeyEnvelopeDto) -> Result<(), Response> {
    if env.key_version == 0 {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_key_version",
            "key_version must be >= 1",
        ));
    }
    let alg = env.kdf_alg.trim();
    if !matches!(
        alg,
        USER_VAULT_KDF_ALG_ARGON2ID
            | USER_VAULT_KDF_ALG_SPLIT_KEY_V2
            | USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2
    ) {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_kdf_alg",
            "kdf_alg must be one of: split_key_v2, device_keychain_v2, argon2id",
        ));
    }
    validate_kdf_params(Argon2KdfParamsDto {
        m_cost_kib: env.kdf_m_cost_kib,
        t_cost: env.kdf_t_cost,
        p_cost: env.kdf_p_cost,
    })?;
    if alg == USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2 {
        if !env.kdf_salt_b64.trim().is_empty() {
            decode_b64_bounded(&env.kdf_salt_b64, "kdf_salt_b64", 8, 64)?;
        }
        if !env.wrapped_root_key_b64.trim().is_empty() {
            decode_b64_bounded(&env.wrapped_root_key_b64, "wrapped_root_key_b64", 16, 256)?;
        }
        let rec = env
            .recovery_wrapped_root_key_b64
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                err_resp(
                    StatusCode::BAD_REQUEST,
                    "missing_recovery_wrapped_root_key",
                    "device_keychain_v2 requires recovery_wrapped_root_key_b64",
                )
            })?;
        decode_b64_bounded(rec, "recovery_wrapped_root_key_b64", 16, 256)?;
    } else {
        decode_b64_bounded(&env.kdf_salt_b64, "kdf_salt_b64", 8, 64)?;
        decode_b64_bounded(&env.wrapped_root_key_b64, "wrapped_root_key_b64", 16, 256)?;
        if let Some(ref rec) = env.recovery_wrapped_root_key_b64 {
            decode_b64_bounded(rec, "recovery_wrapped_root_key_b64", 16, 256)?;
        }
    }
    Ok(())
}

fn compute_deterministic_fake_salt(secret: &[u8; 32], normalized_identifier: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret)
        .expect("HMAC-SHA256 accepts arbitrary 32-byte key");
    mac.update(b"kdf-fake-salt:");
    mac.update(normalized_identifier.as_bytes());
    let digest = mac.finalize().into_bytes();
    BASE64_STD.encode(&digest[..16])
}

fn compute_device_sync_verification_code(pubkey_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DEVICE_SYNC_CODE_DOMAIN_V1);
    hasher.update(pubkey_bytes);
    let digest = hasher.finalize();
    let code = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{:03}-{:03}", code / 1_000, code % 1_000)
}

fn validate_x25519_pubkey_b64(raw: &str, field: &str) -> Result<Vec<u8>, Response> {
    let bytes = decode_b64_bounded(raw, field, 32, 32)?;
    if bytes.iter().all(|&b| b == 0) {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_public_key",
            &format!("{field} cannot be all zeros"),
        ));
    }
    Ok(bytes)
}

pub async fn auth_kdf_params(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let raw_ident = params
        .get("username")
        .or_else(|| params.get("username_or_email"))
        .map(String::as_str)
        .unwrap_or("");
    let normalized = raw_ident.trim().to_lowercase();
    if normalized.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_username",
            "username query parameter is required",
        );
    }

    match state.db.get_user_split_key_auth_by_identifier(&normalized) {
        Ok(Some(rec)) => {
            if rec.auth_version == 2 {
                (
                    StatusCode::OK,
                    Json(KdfParamsResponse {
                        auth_version: 2,
                        auth_salt_b64: rec.auth_salt_b64.unwrap_or_default(),
                        kdf_params: rec.kdf_params,
                    }),
                )
                    .into_response()
            } else {
                (
                    StatusCode::OK,
                    Json(KdfParamsResponse {
                        auth_version: 1,
                        auth_salt_b64: String::new(),
                        kdf_params: Argon2KdfParamsDto::default(),
                    }),
                )
                    .into_response()
            }
        }
        Ok(None) => {
            if state
                .session_manager
                .get_user_by_username(&normalized)
                .or_else(|| state.session_manager.get_user_by_email(&normalized))
                .is_some()
            {
                return (
                    StatusCode::OK,
                    Json(KdfParamsResponse {
                        auth_version: 1,
                        auth_salt_b64: String::new(),
                        kdf_params: Argon2KdfParamsDto::default(),
                    }),
                )
                    .into_response();
            }
            let fake_salt_b64 =
                compute_deterministic_fake_salt(&state.kdf_fake_salt_secret, &normalized);
            (
                StatusCode::OK,
                Json(KdfParamsResponse {
                    auth_version: 2,
                    auth_salt_b64: fake_salt_b64,
                    kdf_params: Argon2KdfParamsDto::default(),
                }),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn auth_upgrade_split_key(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpgradeSplitKeyRequest>,
) -> Response {
    if req.old_password.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_credentials",
            "old_password cannot be empty",
        );
    }
    if let Err(r) = validate_split_key_salt_b64(&req.auth_salt_b64) {
        return r;
    }
    if let Err(r) = validate_split_key_verifier_b64(&req.auth_verifier_b64) {
        return r;
    }
    if let Err(r) = validate_kdf_params(req.kdf_params) {
        return r;
    }

    let rec = match state.db.get_user_split_key_auth_by_id(auth.user_id) {
        Ok(Some(r)) => r,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return map_telegram_err(e),
    };
    if rec.auth_version != 1 {
        return err_resp(
            StatusCode::CONFLICT,
            "already_upgraded",
            "Account is already using Split-Key v2",
        );
    }
    let Some(ref current_hash) = rec.password_hash else {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "no_password_configured",
            "Passwordless account cannot upgrade legacy password",
        );
    };
    if !verify_password(&req.old_password, current_hash) {
        return err_resp(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Current password is incorrect",
        );
    }

    let new_hash = match hash_password(req.auth_verifier_b64.trim()) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, "invalid_auth_verifier", &e.to_string()),
    };
    let now_ms = Utc::now().timestamp_millis();
    if let Err(e) = state.db.update_user_password_and_split_key(
        auth.user_id,
        &new_hash,
        2,
        req.auth_salt_b64.trim(),
        req.kdf_params,
        now_ms,
    ) {
        return map_telegram_err(e);
    }
    state
        .session_manager
        .update_password_hash(auth.user_id, new_hash);

    (
        StatusCode::OK,
        Json(AuthCustodyUpdateResponse {
            ok: true,
            auth_version: 2,
        }),
    )
        .into_response()
}

pub async fn auth_change_password(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    if req.auth_version != 2 {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_auth_version",
            "change-password requires auth_version == 2",
        );
    }
    if req.old_secret.trim().is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_credentials",
            "old_secret cannot be empty",
        );
    }
    if let Err(r) = validate_split_key_salt_b64(&req.new_auth_salt_b64) {
        return r;
    }
    if let Err(r) = validate_split_key_verifier_b64(&req.new_auth_verifier_b64) {
        return r;
    }
    if let Err(r) = validate_kdf_params(req.new_kdf_params) {
        return r;
    }

    let rec = match state.db.get_user_split_key_auth_by_id(auth.user_id) {
        Ok(Some(r)) => r,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return map_telegram_err(e),
    };
    let Some(ref current_hash) = rec.password_hash else {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "no_password_configured",
            "Account has no password configured",
        );
    };
    let candidate = if rec.auth_version == 2 {
        req.old_secret.trim()
    } else {
        req.old_secret.as_str()
    };
    if !verify_password(candidate, current_hash) {
        return err_resp(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Current credential is incorrect",
        );
    }

    let new_hash = match hash_password(req.new_auth_verifier_b64.trim()) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, "invalid_auth_verifier", &e.to_string()),
    };
    let now_ms = Utc::now().timestamp_millis();
    if let Err(e) = state.db.update_user_password_and_split_key(
        auth.user_id,
        &new_hash,
        2,
        req.new_auth_salt_b64.trim(),
        req.new_kdf_params,
        now_ms,
    ) {
        return map_telegram_err(e);
    }
    state
        .session_manager
        .update_password_hash(auth.user_id, new_hash);

    (
        StatusCode::OK,
        Json(AuthCustodyUpdateResponse {
            ok: true,
            auth_version: 2,
        }),
    )
        .into_response()
}

pub async fn get_user_vault_envelope(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.db.get_user_vault_envelope(auth.user_id) {
        Ok(envelope) => (StatusCode::OK, Json(UserVaultEnvelopeResponse { envelope })).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn put_user_vault_envelope(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UserVaultKeyEnvelopeDto>,
) -> Response {
    if let Err(r) = validate_envelope(&req) {
        return r;
    }
    let now_ms = Utc::now().timestamp_millis();
    match state.db.upsert_user_vault_envelope(auth.user_id, &req, now_ms) {
        Ok(saved) => (
            StatusCode::OK,
            Json(UserVaultEnvelopeResponse {
                envelope: Some(saved),
            }),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn pull_user_vault_items(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let since_revision = match params.get("since_revision") {
        None => 0i64,
        Some(s) => match s.trim().parse::<i64>() {
            Ok(v) if v >= 0 => v,
            _ => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_since_revision",
                    "since_revision must be a non-negative integer",
                );
            }
        },
    };
    let category_filter = match params.get("category").map(String::as_str).map(str::trim) {
        None | Some("") => None,
        Some(cat) => match validate_category(cat) {
            Ok(valid) => Some(valid),
            Err(r) => return r,
        },
    };
    match state
        .db
        .pull_user_vault_items(auth.user_id, since_revision, category_filter)
    {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn put_user_vault_item(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((category, item_key)): Path<(String, String)>,
    Json(req): Json<UpsertUserVaultItemRequest>,
) -> Response {
    let valid_category = match validate_category(&category) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let valid_key = match validate_item_key(&item_key) {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(exp) = req.expected_revision {
        if exp < 0 {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_expected_revision",
                "expected_revision must be >= 0",
            );
        }
    }
    if req.deleted {
        if !req.nonce_b64.trim().is_empty() {
            if let Err(r) = decode_b64_bounded(&req.nonce_b64, "nonce_b64", 1, MAX_VAULT_NONCE_BYTES) {
                return r;
            }
        }
        if !req.ciphertext_b64.trim().is_empty() {
            if let Err(r) =
                decode_b64_bounded(&req.ciphertext_b64, "ciphertext_b64", 1, MAX_VAULT_CIPHERTEXT_BYTES)
            {
                return r;
            }
        }
    } else {
        if let Err(r) = decode_b64_bounded(&req.nonce_b64, "nonce_b64", 1, MAX_VAULT_NONCE_BYTES) {
            return r;
        }
        if let Err(r) =
            decode_b64_bounded(&req.ciphertext_b64, "ciphertext_b64", 1, MAX_VAULT_CIPHERTEXT_BYTES)
        {
            return r;
        }
    }

    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .upsert_user_vault_item(auth.user_id, valid_category, valid_key, &req, now_ms)
    {
        Ok(UserVaultUpsertOutcome::Saved(resp)) => (StatusCode::OK, Json(resp)).into_response(),
        Ok(UserVaultUpsertOutcome::Conflict { expected, current }) => err_resp(
            StatusCode::CONFLICT,
            "revision_conflict",
            &format!("expected revision {expected} does not match current revision {current}"),
        ),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn delete_user_vault_item(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((category, item_key)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let valid_category = match validate_category(&category) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let valid_key = match validate_item_key(&item_key) {
        Ok(k) => k,
        Err(r) => return r,
    };
    let expected_revision = match params.get("expected_revision") {
        None => None,
        Some(s) => match s.trim().parse::<i64>() {
            Ok(v) if v >= 0 => Some(v),
            _ => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_expected_revision",
                    "expected_revision must be >= 0",
                );
            }
        },
    };
    let now_ms = Utc::now().timestamp_millis();
    match state.db.delete_user_vault_item(
        auth.user_id,
        valid_category,
        valid_key,
        expected_revision,
        now_ms,
    ) {
        Ok(UserVaultUpsertOutcome::Saved(resp)) => (StatusCode::OK, Json(resp)).into_response(),
        Ok(UserVaultUpsertOutcome::Conflict { expected, current }) => err_resp(
            StatusCode::CONFLICT,
            "revision_conflict",
            &format!("expected revision {expected} does not match current revision {current}"),
        ),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn device_sync_request(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeviceSyncRequestBody>,
) -> Response {
    let device_name = req.device_name.trim();
    if device_name.is_empty() || device_name.chars().count() > MAX_DEVICE_NAME_LEN {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_device_name",
            "device_name must be 1..=128 characters",
        );
    }
    let pubkey_bytes = match validate_x25519_pubkey_b64(&req.device_public_key_b64, "device_public_key_b64") {
        Ok(b) => b,
        Err(r) => return r,
    };
    let verification_code = compute_device_sync_verification_code(&pubkey_bytes);
    let request_id = format!("dsr_{}", Uuid::new_v4().simple());
    let now_secs = Utc::now().timestamp();
    let expires_at = now_secs + DEVICE_SYNC_TTL_SECS;

    match state.db.create_device_sync_request(
        auth.user_id,
        &request_id,
        device_name,
        req.device_public_key_b64.trim(),
        &verification_code,
        now_secs,
        expires_at,
    ) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn device_sync_pending(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let now_secs = Utc::now().timestamp();
    match state.db.list_pending_device_sync_requests(auth.user_id, now_secs) {
        Ok(items) => (StatusCode::OK, Json(PendingDeviceSyncListResponse { items })).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn device_sync_approve(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
    Json(req): Json<ApproveDeviceSyncRequest>,
) -> Response {
    if let Err(r) = validate_x25519_pubkey_b64(&req.approver_ephemeral_pub_b64, "approver_ephemeral_pub_b64") {
        return r;
    }
    if let Err(r) = decode_b64_bounded(&req.nonce_b64, "nonce_b64", 12, 12) {
        return r;
    }
    if let Err(r) = decode_b64_bounded(&req.encrypted_root_key_b64, "encrypted_root_key_b64", 16, 256) {
        return r;
    }

    let now_secs = Utc::now().timestamp();
    match state
        .db
        .approve_device_sync_request(auth.user_id, request_id.trim(), &req, now_secs)
    {
        Ok(DeviceSyncApproveOutcome::Approved) => (
            StatusCode::OK,
            Json(DeviceSyncActionResponse {
                ok: true,
                request_id: request_id.trim().to_string(),
                status: "approved".to_string(),
            }),
        )
            .into_response(),
        Ok(DeviceSyncApproveOutcome::NotFound) => {
            err_resp(StatusCode::NOT_FOUND, "not_found", "Device sync request not found")
        }
        Ok(DeviceSyncApproveOutcome::Forbidden) => err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Device sync request belongs to another user",
        ),
        Ok(DeviceSyncApproveOutcome::Expired) => err_resp(
            StatusCode::GONE,
            "request_expired",
            "Device sync request has expired",
        ),
        Ok(DeviceSyncApproveOutcome::InvalidState(st)) => err_resp(
            StatusCode::CONFLICT,
            "invalid_request_status",
            &format!("Device sync request is in state `{st}`, expected `pending`"),
        ),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn device_sync_deny(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
) -> Response {
    match state.db.deny_device_sync_request(auth.user_id, request_id.trim()) {
        Ok(DeviceSyncDenyOutcome::Denied) => (
            StatusCode::OK,
            Json(DeviceSyncActionResponse {
                ok: true,
                request_id: request_id.trim().to_string(),
                status: "denied".to_string(),
            }),
        )
            .into_response(),
        Ok(DeviceSyncDenyOutcome::NotFound) => {
            err_resp(StatusCode::NOT_FOUND, "not_found", "Device sync request not found")
        }
        Ok(DeviceSyncDenyOutcome::Forbidden) => err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Device sync request belongs to another user",
        ),
        Ok(DeviceSyncDenyOutcome::InvalidState(st)) => err_resp(
            StatusCode::CONFLICT,
            "invalid_request_status",
            &format!("Device sync request is in state `{st}` and cannot be denied"),
        ),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn device_sync_status(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
) -> Response {
    let now_secs = Utc::now().timestamp();
    match state
        .db
        .poll_and_consume_device_sync_status(auth.user_id, request_id.trim(), now_secs)
    {
        Ok(DeviceSyncPollOutcome::Found(resp)) => (StatusCode::OK, Json(resp)).into_response(),
        Ok(DeviceSyncPollOutcome::NotFound) => {
            err_resp(StatusCode::NOT_FOUND, "not_found", "Device sync request not found")
        }
        Ok(DeviceSyncPollOutcome::Forbidden) => err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Device sync request belongs to another user",
        ),
        Err(e) => map_telegram_err(e),
    }
}
