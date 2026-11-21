use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::user_vault::{
    ApproveDeviceSyncRequest, Argon2KdfParamsDto, DeviceSyncRequestResponse,
    DeviceSyncStatusResponse, PendingDeviceSyncItem, UpsertUserVaultItemRequest,
    UpsertUserVaultItemResponse, UserVaultItemDto, UserVaultKeyEnvelopeDto,
    UserVaultSyncPullResponse,
};
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserVaultUpsertOutcome {
    Saved(UpsertUserVaultItemResponse),
    Conflict { expected: i64, current: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSplitKeyAuthRecord {
    pub user_id: UserId,
    pub username: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
    pub password_hash: Option<String>,
    pub account_generation: u64,
    pub created_at_ms: i64,
    pub auth_version: i32,
    pub auth_salt_b64: Option<String>,
    pub kdf_params: Argon2KdfParamsDto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSyncApproveOutcome {
    Approved,
    NotFound,
    Forbidden,
    Expired,
    InvalidState(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSyncDenyOutcome {
    Denied,
    NotFound,
    Forbidden,
    InvalidState(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSyncPollOutcome {
    Found(DeviceSyncStatusResponse),
    NotFound,
    Forbidden,
}

impl ServerDatabase {
    pub fn get_user_vault_envelope(&self, user_id: UserId) -> Result<Option<UserVaultKeyEnvelopeDto>> {
        let uid = user_id.0 as i64;
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT key_version, kdf_alg, kdf_salt_b64, kdf_m_cost_kib, kdf_t_cost, kdf_p_cost,
                          wrapped_root_key_b64, recovery_wrapped_root_key_b64, updated_at_ms
                   FROM user_vault_key_envelopes
                   WHERE user_id = ?1"#,
                params![uid],
                |r| {
                    Ok(UserVaultKeyEnvelopeDto {
                        key_version: r.get::<_, i64>(0)? as u32,
                        kdf_alg: r.get(1)?,
                        kdf_salt_b64: r.get(2)?,
                        kdf_m_cost_kib: r.get::<_, i64>(3)? as u32,
                        kdf_t_cost: r.get::<_, i64>(4)? as u32,
                        kdf_p_cost: r.get::<_, i64>(5)? as u32,
                        wrapped_root_key_b64: r.get(6)?,
                        recovery_wrapped_root_key_b64: r.get(7)?,
                        updated_at_ms: r.get(8)?,
                    })
                },
            )
            .optional()
            .map_db()
        })
    }

    pub fn upsert_user_vault_envelope(
        &self,
        user_id: UserId,
        env: &UserVaultKeyEnvelopeDto,
        now_ms: i64,
    ) -> Result<UserVaultKeyEnvelopeDto> {
        let uid = user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"INSERT INTO user_vault_key_envelopes (
                       user_id, key_version, kdf_alg, kdf_salt_b64,
                       kdf_m_cost_kib, kdf_t_cost, kdf_p_cost,
                       wrapped_root_key_b64, recovery_wrapped_root_key_b64, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                   ON CONFLICT(user_id) DO UPDATE SET
                       key_version = excluded.key_version,
                       kdf_alg = excluded.kdf_alg,
                       kdf_salt_b64 = excluded.kdf_salt_b64,
                       kdf_m_cost_kib = excluded.kdf_m_cost_kib,
                       kdf_t_cost = excluded.kdf_t_cost,
                       kdf_p_cost = excluded.kdf_p_cost,
                       wrapped_root_key_b64 = excluded.wrapped_root_key_b64,
                       recovery_wrapped_root_key_b64 = excluded.recovery_wrapped_root_key_b64,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![
                    uid,
                    env.key_version as i64,
                    env.kdf_alg.trim(),
                    env.kdf_salt_b64.trim(),
                    env.kdf_m_cost_kib as i64,
                    env.kdf_t_cost as i64,
                    env.kdf_p_cost as i64,
                    env.wrapped_root_key_b64.trim(),
                    env.recovery_wrapped_root_key_b64.as_deref().map(str::trim),
                    now_ms,
                ],
            )
            .map_db()?;
            Ok(UserVaultKeyEnvelopeDto {
                key_version: env.key_version,
                kdf_alg: env.kdf_alg.trim().to_string(),
                kdf_salt_b64: env.kdf_salt_b64.trim().to_string(),
                kdf_m_cost_kib: env.kdf_m_cost_kib,
                kdf_t_cost: env.kdf_t_cost,
                kdf_p_cost: env.kdf_p_cost,
                wrapped_root_key_b64: env.wrapped_root_key_b64.trim().to_string(),
                recovery_wrapped_root_key_b64: env
                    .recovery_wrapped_root_key_b64
                    .as_deref()
                    .map(|s| s.trim().to_string()),
                updated_at_ms: now_ms,
            })
        })
    }

    pub fn pull_user_vault_items(
        &self,
        user_id: UserId,
        since_revision: i64,
        category: Option<&str>,
    ) -> Result<UserVaultSyncPullResponse> {
        let uid = user_id.0 as i64;
        self.pool.with_reader(|conn| {
            let latest_revision: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(revision), 0) FROM user_vault_items WHERE user_id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
                .map_db()?;

            let items = if let Some(cat) = category {
                let mut stmt = conn
                    .prepare(
                        r#"SELECT category, item_key, revision, nonce_b64, ciphertext_b64, deleted, updated_at_ms
                           FROM user_vault_items
                           WHERE user_id = ?1 AND revision > ?2 AND category = ?3
                           ORDER BY revision ASC"#,
                    )
                    .map_db()?;
                let rows = stmt
                    .query_map(params![uid, since_revision, cat], |r| {
                        Ok(UserVaultItemDto {
                            category: r.get(0)?,
                            item_key: r.get(1)?,
                            revision: r.get(2)?,
                            nonce_b64: r.get(3)?,
                            ciphertext_b64: r.get(4)?,
                            deleted: r.get::<_, i64>(5)? != 0,
                            updated_at_ms: r.get(6)?,
                        })
                    })
                    .map_db()?;
                rows.collect::<std::result::Result<Vec<_>, _>>().map_db()?
            } else {
                let mut stmt = conn
                    .prepare(
                        r#"SELECT category, item_key, revision, nonce_b64, ciphertext_b64, deleted, updated_at_ms
                           FROM user_vault_items
                           WHERE user_id = ?1 AND revision > ?2
                           ORDER BY revision ASC"#,
                    )
                    .map_db()?;
                let rows = stmt
                    .query_map(params![uid, since_revision], |r| {
                        Ok(UserVaultItemDto {
                            category: r.get(0)?,
                            item_key: r.get(1)?,
                            revision: r.get(2)?,
                            nonce_b64: r.get(3)?,
                            ciphertext_b64: r.get(4)?,
                            deleted: r.get::<_, i64>(5)? != 0,
                            updated_at_ms: r.get(6)?,
                        })
                    })
                    .map_db()?;
                rows.collect::<std::result::Result<Vec<_>, _>>().map_db()?
            };

            Ok(UserVaultSyncPullResponse {
                latest_revision,
                items,
            })
        })
    }

    pub fn upsert_user_vault_item(
        &self,
        user_id: UserId,
        category: &str,
        item_key: &str,
        req: &UpsertUserVaultItemRequest,
        now_ms: i64,
    ) -> Result<UserVaultUpsertOutcome> {
        let uid = user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            let current_rev: i64 = tx
                .query_row(
                    r#"SELECT revision FROM user_vault_items
                       WHERE user_id = ?1 AND category = ?2 AND item_key = ?3"#,
                    params![uid, category, item_key],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?
                .unwrap_or(0);

            if let Some(expected) = req.expected_revision {
                if expected != current_rev {
                    return Ok(UserVaultUpsertOutcome::Conflict {
                        expected,
                        current: current_rev,
                    });
                }
            }

            let next_rev: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(revision), 0) + 1 FROM user_vault_items WHERE user_id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
                .map_db()?;

            tx.execute(
                r#"INSERT INTO user_vault_items (
                       user_id, category, item_key, revision, nonce_b64, ciphertext_b64, deleted, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                   ON CONFLICT(user_id, category, item_key) DO UPDATE SET
                       revision = excluded.revision,
                       nonce_b64 = excluded.nonce_b64,
                       ciphertext_b64 = excluded.ciphertext_b64,
                       deleted = excluded.deleted,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![
                    uid,
                    category,
                    item_key,
                    next_rev,
                    req.nonce_b64.trim(),
                    req.ciphertext_b64.trim(),
                    req.deleted as i64,
                    now_ms,
                ],
            )
            .map_db()?;

            Ok(UserVaultUpsertOutcome::Saved(UpsertUserVaultItemResponse {
                ok: true,
                category: category.to_string(),
                item_key: item_key.to_string(),
                revision: next_rev,
                updated_at_ms: now_ms,
            }))
        })
    }

    pub fn delete_user_vault_item(
        &self,
        user_id: UserId,
        category: &str,
        item_key: &str,
        expected_revision: Option<i64>,
        now_ms: i64,
    ) -> Result<UserVaultUpsertOutcome> {
        let tombstone_req = UpsertUserVaultItemRequest {
            expected_revision,
            nonce_b64: String::new(),
            ciphertext_b64: String::new(),
            deleted: true,
        };
        self.upsert_user_vault_item(user_id, category, item_key, &tombstone_req, now_ms)
    }

    pub fn get_or_init_server_kv(
        &self,
        namespace: &str,
        key: &str,
        default_value: &str,
        now_secs: i64,
    ) -> Result<String> {
        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"INSERT OR IGNORE INTO hermes_kv_state (namespace, key, value, updated_at)
                   VALUES (?1, ?2, ?3, ?4)"#,
                params![namespace, key, default_value, now_secs],
            )
            .map_db()?;
            tx.query_row(
                "SELECT value FROM hermes_kv_state WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |r| r.get(0),
            )
            .map_db()
        })
    }

    fn map_split_key_auth_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UserSplitKeyAuthRecord> {
        let default_params = Argon2KdfParamsDto::default();
        let m_kib = r
            .get::<_, Option<i64>>(8)?
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
            .unwrap_or(default_params.m_cost_kib);
        let t_cost = r
            .get::<_, Option<i64>>(9)?
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
            .unwrap_or(default_params.t_cost);
        let p_cost = r
            .get::<_, Option<i64>>(10)?
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
            .unwrap_or(default_params.p_cost);
        Ok(UserSplitKeyAuthRecord {
            user_id: UserId(r.get::<_, i64>(0)? as u64),
            username: r.get(1)?,
            first_name: r.get(2)?,
            last_name: r.get(3)?,
            password_hash: r.get(4)?,
            account_generation: (r.get::<_, i64>(5)?).max(1) as u64,
            created_at_ms: r.get(6)?,
            auth_version: r.get::<_, i64>(7)? as i32,
            auth_salt_b64: r.get(11)?,
            kdf_params: Argon2KdfParamsDto {
                m_cost_kib: m_kib,
                t_cost,
                p_cost,
            },
        })
    }

    pub fn get_user_split_key_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<UserSplitKeyAuthRecord>> {
        let trimmed = identifier.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT user_id, username, first_name, last_name, password_hash,
                          account_generation, created_at_ms, auth_version,
                          auth_kdf_m_kib, auth_kdf_t_cost, auth_kdf_p_cost, auth_salt_b64
                   FROM users
                   WHERE username = ?1 COLLATE NOCASE OR email = ?1 COLLATE NOCASE
                   ORDER BY CASE WHEN username = ?1 COLLATE NOCASE THEN 0 ELSE 1 END
                   LIMIT 1"#,
                params![trimmed],
                Self::map_split_key_auth_row,
            )
            .optional()
            .map_db()
        })
    }

    pub fn get_user_split_key_auth_by_id(
        &self,
        user_id: UserId,
    ) -> Result<Option<UserSplitKeyAuthRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT user_id, username, first_name, last_name, password_hash,
                          account_generation, created_at_ms, auth_version,
                          auth_kdf_m_kib, auth_kdf_t_cost, auth_kdf_p_cost, auth_salt_b64
                   FROM users
                   WHERE user_id = ?1
                   LIMIT 1"#,
                params![user_id.0 as i64],
                Self::map_split_key_auth_row,
            )
            .optional()
            .map_db()
        })
    }

    pub fn set_user_split_key_params(
        &self,
        user_id: UserId,
        auth_version: i32,
        auth_salt_b64: Option<&str>,
        kdf_params: Option<Argon2KdfParamsDto>,
    ) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"UPDATE users
                   SET auth_version = ?1,
                       auth_salt_b64 = ?2,
                       auth_kdf_m_kib = ?3,
                       auth_kdf_t_cost = ?4,
                       auth_kdf_p_cost = ?5
                   WHERE user_id = ?6"#,
                params![
                    auth_version as i64,
                    auth_salt_b64,
                    kdf_params.map(|p| p.m_cost_kib as i64),
                    kdf_params.map(|p| p.t_cost as i64),
                    kdf_params.map(|p| p.p_cost as i64),
                    user_id.0 as i64,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn update_user_password_and_split_key(
        &self,
        user_id: UserId,
        password_hash: &str,
        auth_version: i32,
        auth_salt_b64: &str,
        kdf_params: Argon2KdfParamsDto,
        now_ms: i64,
    ) -> Result<()> {
        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"UPDATE users
                   SET password_hash = ?1,
                       auth_version = ?2,
                       auth_salt_b64 = ?3,
                       auth_kdf_m_kib = ?4,
                       auth_kdf_t_cost = ?5,
                       auth_kdf_p_cost = ?6,
                       updated_at_ms = ?7
                   WHERE user_id = ?8"#,
                params![
                    password_hash,
                    auth_version as i64,
                    auth_salt_b64,
                    kdf_params.m_cost_kib as i64,
                    kdf_params.t_cost as i64,
                    kdf_params.p_cost as i64,
                    now_ms,
                    user_id.0 as i64,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn create_device_sync_request(
        &self,
        user_id: UserId,
        request_id: &str,
        device_name: &str,
        device_public_key_b64: &str,
        verification_code: &str,
        created_at: i64,
        expires_at: i64,
    ) -> Result<DeviceSyncRequestResponse> {
        let uid = user_id.0 as i64;
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO user_vault_device_sync_requests (
                       request_id, user_id, device_name, device_public_key_b64,
                       verification_code, status, approver_ephemeral_pub_b64,
                       encrypted_root_key_b64, nonce_b64, created_at, expires_at
                   ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, NULL, NULL, ?6, ?7)"#,
                params![
                    request_id,
                    uid,
                    device_name,
                    device_public_key_b64,
                    verification_code,
                    created_at,
                    expires_at,
                ],
            )
            .map_db()?;
            Ok(DeviceSyncRequestResponse {
                request_id: request_id.to_string(),
                verification_code: verification_code.to_string(),
                expires_at,
            })
        })
    }

    pub fn list_pending_device_sync_requests(
        &self,
        user_id: UserId,
        now_secs: i64,
    ) -> Result<Vec<PendingDeviceSyncItem>> {
        let uid = user_id.0 as i64;
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"SELECT request_id, device_name, device_public_key_b64,
                              verification_code, status, created_at, expires_at
                       FROM user_vault_device_sync_requests
                       WHERE user_id = ?1 AND status = 'pending' AND expires_at > ?2
                       ORDER BY created_at ASC"#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![uid, now_secs], |r| {
                    Ok(PendingDeviceSyncItem {
                        request_id: r.get(0)?,
                        device_name: r.get(1)?,
                        device_public_key_b64: r.get(2)?,
                        verification_code: r.get(3)?,
                        status: r.get(4)?,
                        created_at: r.get(5)?,
                        expires_at: r.get(6)?,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn approve_device_sync_request(
        &self,
        user_id: UserId,
        request_id: &str,
        req: &ApproveDeviceSyncRequest,
        now_secs: i64,
    ) -> Result<DeviceSyncApproveOutcome> {
        let uid = user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            let row: Option<(i64, String, i64)> = tx
                .query_row(
                    "SELECT user_id, status, expires_at FROM user_vault_device_sync_requests WHERE request_id = ?1",
                    params![request_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_db()?;
            let Some((owner_uid, status, expires_at)) = row else {
                return Ok(DeviceSyncApproveOutcome::NotFound);
            };
            if owner_uid != uid {
                return Ok(DeviceSyncApproveOutcome::Forbidden);
            }
            if expires_at <= now_secs {
                if status == "pending" {
                    tx.execute(
                        "UPDATE user_vault_device_sync_requests SET status = 'expired' WHERE request_id = ?1",
                        params![request_id],
                    )
                    .map_db()?;
                }
                return Ok(DeviceSyncApproveOutcome::Expired);
            }
            if status != "pending" {
                return Ok(DeviceSyncApproveOutcome::InvalidState(status));
            }
            tx.execute(
                r#"UPDATE user_vault_device_sync_requests
                   SET status = 'approved',
                       approver_ephemeral_pub_b64 = ?1,
                       encrypted_root_key_b64 = ?2,
                       nonce_b64 = ?3
                   WHERE request_id = ?4 AND user_id = ?5 AND status = 'pending'"#,
                params![
                    req.approver_ephemeral_pub_b64.trim(),
                    req.encrypted_root_key_b64.trim(),
                    req.nonce_b64.trim(),
                    request_id,
                    uid,
                ],
            )
            .map_db()?;
            Ok(DeviceSyncApproveOutcome::Approved)
        })
    }

    pub fn deny_device_sync_request(
        &self,
        user_id: UserId,
        request_id: &str,
    ) -> Result<DeviceSyncDenyOutcome> {
        let uid = user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT user_id, status FROM user_vault_device_sync_requests WHERE request_id = ?1",
                    params![request_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_db()?;
            let Some((owner_uid, status)) = row else {
                return Ok(DeviceSyncDenyOutcome::NotFound);
            };
            if owner_uid != uid {
                return Ok(DeviceSyncDenyOutcome::Forbidden);
            }
            if status == "consumed" || status == "expired" {
                return Ok(DeviceSyncDenyOutcome::InvalidState(status));
            }
            tx.execute(
                r#"UPDATE user_vault_device_sync_requests
                   SET status = 'denied',
                       approver_ephemeral_pub_b64 = NULL,
                       encrypted_root_key_b64 = NULL,
                       nonce_b64 = NULL
                   WHERE request_id = ?1 AND user_id = ?2"#,
                params![request_id, uid],
            )
            .map_db()?;
            Ok(DeviceSyncDenyOutcome::Denied)
        })
    }

    pub fn poll_and_consume_device_sync_status(
        &self,
        user_id: UserId,
        request_id: &str,
        now_secs: i64,
    ) -> Result<DeviceSyncPollOutcome> {
        let uid = user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            let row: Option<(i64, String, Option<String>, Option<String>, Option<String>, i64)> = tx
                .query_row(
                    r#"SELECT user_id, status, approver_ephemeral_pub_b64,
                              encrypted_root_key_b64, nonce_b64, expires_at
                       FROM user_vault_device_sync_requests
                       WHERE request_id = ?1"#,
                    params![request_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                )
                .optional()
                .map_db()?;
            let Some((owner_uid, status, eph_pub, enc_key, nonce, expires_at)) = row else {
                return Ok(DeviceSyncPollOutcome::NotFound);
            };
            if owner_uid != uid {
                return Ok(DeviceSyncPollOutcome::Forbidden);
            }
            if expires_at <= now_secs && (status == "pending" || status == "approved") {
                tx.execute(
                    r#"UPDATE user_vault_device_sync_requests
                       SET status = 'expired',
                           approver_ephemeral_pub_b64 = NULL,
                           encrypted_root_key_b64 = NULL,
                           nonce_b64 = NULL
                       WHERE request_id = ?1 AND user_id = ?2"#,
                    params![request_id, uid],
                )
                .map_db()?;
                return Ok(DeviceSyncPollOutcome::Found(DeviceSyncStatusResponse {
                    status: "expired".to_string(),
                    approver_ephemeral_pub_b64: None,
                    encrypted_root_key_b64: None,
                    nonce_b64: None,
                    expires_at,
                }));
            }
            if status == "approved" {
                tx.execute(
                    r#"UPDATE user_vault_device_sync_requests
                       SET status = 'consumed',
                           approver_ephemeral_pub_b64 = NULL,
                           encrypted_root_key_b64 = NULL,
                           nonce_b64 = NULL
                       WHERE request_id = ?1 AND user_id = ?2 AND status = 'approved'"#,
                    params![request_id, uid],
                )
                .map_db()?;
                return Ok(DeviceSyncPollOutcome::Found(DeviceSyncStatusResponse {
                    status: "approved".to_string(),
                    approver_ephemeral_pub_b64: eph_pub,
                    encrypted_root_key_b64: enc_key,
                    nonce_b64: nonce,
                    expires_at,
                }));
            }
            Ok(DeviceSyncPollOutcome::Found(DeviceSyncStatusResponse {
                status,
                approver_ephemeral_pub_b64: None,
                encrypted_root_key_b64: None,
                nonce_b64: None,
                expires_at,
            }))
        })
    }
}
