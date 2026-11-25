use crate::db::automation_app_store::{
    AutomationAppRecord, InstallAutomationAppOutcome, PublishAutomationAppOutcome,
    RecordAutomationRunOutcome,
};
use crate::routes::{err_resp, map_telegram_err};
use crate::server::{AppState, AuthBearer, OptionalAuthBearer};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use bot_browser::{
    scan_json_value_for_secret_leaks, scan_text_for_secret_leaks,
    validate_script_app_safe_for_publish, AutomationScriptApp, BotBrowserError,
};
use chrono::Utc;
use gpui_telegram_proto::automation_apps::{
    AutomationAppDetail, AutomationAppParameterDto, AutomationAppSummary,
    InstallAutomationAppResponse, InstalledAutomationApp, ListAutomationAppsResponse,
    ListInstalledAutomationAppsResponse, PublishAutomationAppRequest, RecordAutomationRunRequest,
    RecordAutomationRunResponse, UninstallAutomationAppResponse,
};
use serde_json::Value;
use std::sync::Arc;

const MAX_PUBLISH_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_APP_ID_LEN: usize = 128;
const MAX_STEPS_COUNT: usize = 200;
const MAX_PARAMETERS_COUNT: usize = 64;
const MAX_DOMAINS_COUNT: usize = 32;
const MAX_NAME_LEN: usize = 160;
const MAX_VERSION_LEN: usize = 64;
const MAX_CATEGORY_LEN: usize = 64;

fn validate_app_id(raw: &str) -> Result<&str, Response> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_APP_ID_LEN
        || trimmed.eq_ignore_ascii_case("installed")
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_app_id",
            "app_id must be 1..=128 characters of [a-zA-Z0-9_-] and cannot be 'installed'",
        ));
    }
    Ok(trimmed)
}

fn extract_manifest_summary_fields(
    manifest_json: &str,
) -> (
    Vec<String>,
    Vec<String>,
    usize,
    Vec<AutomationAppParameterDto>,
) {
    if let Ok(app) = serde_json::from_str::<AutomationScriptApp>(manifest_json) {
        let params = app
            .parameters
            .into_iter()
            .map(|p| AutomationAppParameterDto {
                name: p.name,
                description: p.description,
                required: p.required,
                default_value: p.default_value,
                is_secret: p.is_secret,
            })
            .collect();
        (
            app.target_domains,
            app.required_capabilities,
            app.steps.len(),
            params,
        )
    } else {
        (Vec::new(), Vec::new(), 0, Vec::new())
    }
}

fn record_to_summary(rec: &AutomationAppRecord, is_installed: bool) -> AutomationAppSummary {
    let (parsed_domains, required_capabilities, step_count, parameters) =
        extract_manifest_summary_fields(&rec.manifest_json);
    let target_domains = if parsed_domains.is_empty() {
        serde_json::from_str::<Vec<String>>(&rec.target_domains_json).unwrap_or_default()
    } else {
        parsed_domains
    };
    AutomationAppSummary {
        app_id: rec.app_id.clone(),
        slug: rec.slug.clone(),
        author_user_id: rec.author_user_id.max(0) as u64,
        author_username: rec.author_username.clone(),
        name: rec.name.clone(),
        description: rec.description.clone(),
        category: rec.category.clone(),
        target_domains,
        latest_version: rec.latest_version.clone(),
        required_capabilities,
        step_count,
        parameters,
        install_count: rec.install_count.max(0) as u64,
        run_count: rec.run_count.max(0) as u64,
        is_published: rec.is_published,
        is_installed,
        created_at_ms: rec.created_at_ms,
        updated_at_ms: rec.updated_at_ms,
    }
}

fn record_to_detail(
    rec: &AutomationAppRecord,
    versions: Vec<String>,
    content_sha256: String,
    installed_version: Option<String>,
) -> AutomationAppDetail {
    let target_domains =
        serde_json::from_str::<Vec<String>>(&rec.target_domains_json).unwrap_or_default();
    let manifest: Value = serde_json::from_str(&rec.manifest_json).unwrap_or(Value::Null);
    AutomationAppDetail {
        app_id: rec.app_id.clone(),
        slug: rec.slug.clone(),
        author_user_id: rec.author_user_id.max(0) as u64,
        author_username: rec.author_username.clone(),
        name: rec.name.clone(),
        description: rec.description.clone(),
        category: rec.category.clone(),
        target_domains,
        latest_version: rec.latest_version.clone(),
        versions,
        manifest,
        content_sha256,
        install_count: rec.install_count.max(0) as u64,
        run_count: rec.run_count.max(0) as u64,
        is_published: rec.is_published,
        is_installed: installed_version.is_some(),
        installed_version,
        created_at_ms: rec.created_at_ms,
        updated_at_ms: rec.updated_at_ms,
    }
}

pub async fn automation_apps_publish(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "empty_body",
            "Request body cannot be empty",
        );
    }
    if body.len() > MAX_PUBLISH_PAYLOAD_BYTES {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "payload_too_large",
            "Publish payload exceeds maximum size of 512 KiB",
        );
    }

    let raw_val: Value = match serde_json::from_slice::<Value>(&body) {
        Ok(v) if v.is_object() => v,
        Ok(_) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "Expected JSON object",
            );
        }
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("Malformed JSON: {e}"),
            );
        }
    };

    let req: PublishAutomationAppRequest = match serde_json::from_value(raw_val.clone()) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                &format!("Invalid publish request payload: {e}"),
            );
        }
    };

    if !req.manifest.is_object() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_manifest",
            "manifest must be a JSON object",
        );
    }

    let mut manifest_val = req.manifest.clone();
    let mut app: AutomationScriptApp = match serde_json::from_value(manifest_val.clone()) {
        Ok(a) => a,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_manifest",
                &format!("Invalid AutomationScriptApp manifest: {e}"),
            );
        }
    };

    let app_id = match validate_app_id(&app.app_id) {
        Ok(id) => id.to_string(),
        Err(resp) => return resp,
    };
    app.app_id = app_id.clone();

    if app.steps.len() > MAX_STEPS_COUNT {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "steps exceeds maximum allowed count (200)",
        );
    }
    if app.parameters.len() > MAX_PARAMETERS_COUNT {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "parameters exceeds maximum allowed count (64)",
        );
    }
    if app.target_domains.len() > MAX_DOMAINS_COUNT {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "target_domains exceeds maximum allowed count (32)",
        );
    }
    if app.name.trim().len() > MAX_NAME_LEN {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "name exceeds maximum length (160)",
        );
    }
    if app.version.trim().len() > MAX_VERSION_LEN {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "version exceeds maximum length (64)",
        );
    }
    if app.category.trim().len() > MAX_CATEGORY_LEN {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "category exceeds maximum length (64)",
        );
    }

    // Author anti-spoofing
    let caller_author_id = auth.user_id.0.to_string();
    if let Some(ref declared_author) = app.author_id {
        if declared_author.trim() != caller_author_id {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "manifest author_id does not match authenticated user",
            );
        }
    } else {
        app.author_id = Some(caller_author_id.clone());
        if let Some(obj) = manifest_val.as_object_mut() {
            obj.insert("author_id".to_string(), Value::String(caller_author_id));
        }
    }

    // 1. Server-side defense-in-depth scan over raw request JSON (catches unknown fields,
    //    changelog, and standalone `Basic <b64>` credentials).
    if let Err(err) = scan_json_value_for_secret_leaks(&raw_val, "request", &app.parameters) {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "secret_leak_detected",
            &err.to_string(),
        );
    }
    if let Some(ref changelog) = req.changelog {
        if let Err(err) = scan_text_for_secret_leaks(changelog, "request.changelog", &app.parameters)
        {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "secret_leak_detected",
                &err.to_string(),
            );
        }
    }

    // 2. Canonical `bot-browser` publish safety validator.
    if let Err(err) = validate_script_app_safe_for_publish(&app) {
        return match err {
            BotBrowserError::SecretLeakDetected(msg) => {
                err_resp(StatusCode::UNPROCESSABLE_ENTITY, "secret_leak_detected", &msg)
            }
            other => err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_manifest",
                &other.to_string(),
            ),
        };
    }

    let trimmed_slug = app.slug.trim().to_string();
    let trimmed_name = app.name.trim().to_string();
    let trimmed_version = app.version.trim().to_string();
    let category = if app.category.trim().is_empty() {
        "general".to_string()
    } else {
        app.category.trim().to_string()
    };
    if let Some(obj) = manifest_val.as_object_mut() {
        obj.insert("app_id".to_string(), Value::String(app.app_id.clone()));
        obj.insert("slug".to_string(), Value::String(trimmed_slug.clone()));
        obj.insert("name".to_string(), Value::String(trimmed_name.clone()));
        obj.insert("version".to_string(), Value::String(trimmed_version.clone()));
        obj.insert("category".to_string(), Value::String(category.clone()));
    }

    let manifest_json = match serde_json::to_string(&manifest_val) {
        Ok(s) => s,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_manifest",
                &format!("Failed to serialize manifest: {e}"),
            );
        }
    };
    let target_domains_json =
        serde_json::to_string(&app.target_domains).unwrap_or_else(|_| "[]".to_string());
    let now_ms = Utc::now().timestamp_millis();

    let outcome = match state.db.publish_automation_app(
        &app.app_id,
        &trimmed_slug,
        auth.user_id.0 as i64,
        &trimmed_name,
        app.description.trim(),
        &category,
        &target_domains_json,
        &trimmed_version,
        &manifest_json,
        req.is_published,
        now_ms,
    ) {
        Ok(o) => o,
        Err(e) => return map_telegram_err(e),
    };

    match outcome {
        PublishAutomationAppOutcome::ForbiddenOwner => err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only the app author can publish updates for this app_id",
        ),
        PublishAutomationAppOutcome::DuplicateVersion { app_id, version } => err_resp(
            StatusCode::CONFLICT,
            "version_conflict",
            &format!("Version '{version}' is already published for app '{app_id}'"),
        ),
        PublishAutomationAppOutcome::Published {
            app: rec,
            version: ver,
            created_new_app,
        } => {
            let versions = state
                .db
                .list_automation_app_versions(&rec.app_id)
                .map(|vs| vs.into_iter().map(|v| v.version).collect())
                .unwrap_or_else(|_| vec![ver.version.clone()]);
            let installed_version = state
                .db
                .is_automation_app_installed_map(auth.user_id.0 as i64)
                .ok()
                .and_then(|m| m.get(&rec.app_id).map(|u| u.installed_version.clone()));
            let detail = record_to_detail(&rec, versions, ver.content_sha256, installed_version);
            let status = if created_new_app {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(detail)).into_response()
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct ListAutomationAppsQueryParams {
    pub q: Option<String>,
    pub category: Option<String>,
    pub domain: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

pub async fn automation_apps_list(
    OptionalAuthBearer(maybe_auth): OptionalAuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListAutomationAppsQueryParams>,
) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 100) as u32;
    let offset = query.offset.unwrap_or(0).min(10_000) as u32;
    let q_str = query.q.as_deref().unwrap_or("");

    let (records, total) = match state.db.search_automation_apps(
        q_str,
        query.category.as_deref(),
        query.domain.as_deref(),
        limit as usize,
        offset as usize,
    ) {
        Ok(r) => r,
        Err(e) => return map_telegram_err(e),
    };

    let installed_map = maybe_auth
        .and_then(|a| state.db.is_automation_app_installed_map(a.user_id.0 as i64).ok())
        .unwrap_or_default();

    let apps = records
        .iter()
        .map(|rec| record_to_summary(rec, installed_map.contains_key(&rec.app_id)))
        .collect();

    (
        StatusCode::OK,
        Json(ListAutomationAppsResponse {
            apps,
            total,
            limit,
            offset,
        }),
    )
        .into_response()
}

pub async fn automation_apps_installed(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let rows = match state
        .db
        .list_user_installed_automation_apps(auth.user_id.0 as i64)
    {
        Ok(r) => r,
        Err(e) => return map_telegram_err(e),
    };

    let apps: Vec<InstalledAutomationApp> = rows
        .into_iter()
        .map(|(rec, ua, sha, local_runs)| {
            let manifest: Value = serde_json::from_str(&rec.manifest_json).unwrap_or(Value::Null);
            let slug = manifest
                .get("slug")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or(rec.slug);
            let name = manifest
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or(rec.name);
            let description = manifest
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(String::from)
                .unwrap_or(rec.description);
            let category = manifest
                .get("category")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or(rec.category);
            let target_domains = manifest
                .get("target_domains")
                .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                .unwrap_or_else(|| {
                    serde_json::from_str(&rec.target_domains_json).unwrap_or_default()
                });
            InstalledAutomationApp {
                app_id: rec.app_id,
                slug,
                name,
                description,
                category,
                target_domains,
                installed_version: ua.installed_version,
                latest_version: rec.latest_version,
                manifest,
                content_sha256: sha,
                installed_at_ms: ua.installed_at_ms,
                last_run_at_ms: ua.last_run_at_ms,
                local_run_count: local_runs,
            }
        })
        .collect();
    let total = apps.len();

    (
        StatusCode::OK,
        Json(ListInstalledAutomationAppsResponse { apps, total }),
    )
        .into_response()
}

pub async fn automation_app_get(
    OptionalAuthBearer(maybe_auth): OptionalAuthBearer,
    State(state): State<Arc<AppState>>,
    Path(raw_app_id): Path<String>,
) -> Response {
    let app_id = match validate_app_id(&raw_app_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let rec = match state.db.get_automation_app(app_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "not_found",
                "Automation app not found",
            );
        }
        Err(e) => return map_telegram_err(e),
    };

    let caller_id = maybe_auth.as_ref().map(|a| a.user_id.0 as i64);
    if !rec.is_published && caller_id != Some(rec.author_user_id) {
        return err_resp(
            StatusCode::NOT_FOUND,
            "not_found",
            "Automation app not found",
        );
    }

    let version_rows = state
        .db
        .list_automation_app_versions(&rec.app_id)
        .unwrap_or_default();
    let content_sha256 = version_rows
        .iter()
        .find(|v| v.version == rec.latest_version)
        .map(|v| v.content_sha256.clone())
        .unwrap_or_else(|| crate::db::mcp_oauth_store::sha256_hex(&rec.manifest_json));
    let versions: Vec<String> = version_rows.into_iter().map(|v| v.version).collect();

    let installed_version = caller_id
        .and_then(|uid| state.db.is_automation_app_installed_map(uid).ok())
        .and_then(|m| m.get(&rec.app_id).map(|u| u.installed_version.clone()));

    let detail = record_to_detail(&rec, versions, content_sha256, installed_version);
    (StatusCode::OK, Json(detail)).into_response()
}

pub async fn automation_app_install(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(raw_app_id): Path<String>,
) -> Response {
    let app_id = match validate_app_id(&raw_app_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    let outcome = match state
        .db
        .install_automation_app(auth.user_id.0 as i64, app_id, now_ms)
    {
        Ok(o) => o,
        Err(e) => return map_telegram_err(e),
    };

    match outcome {
        InstallAutomationAppOutcome::NotFound => err_resp(
            StatusCode::NOT_FOUND,
            "not_found",
            "Automation app not found",
        ),
        InstallAutomationAppOutcome::Installed {
            app,
            install,
            content_sha256,
            local_run_count,
            newly_installed,
        } => {
            let installed = InstalledAutomationApp {
                app_id: app.app_id,
                slug: app.slug,
                name: app.name,
                description: app.description,
                category: app.category,
                target_domains: serde_json::from_str(&app.target_domains_json).unwrap_or_default(),
                installed_version: install.installed_version,
                latest_version: app.latest_version,
                manifest: serde_json::from_str(&app.manifest_json).unwrap_or(Value::Null),
                content_sha256,
                installed_at_ms: install.installed_at_ms,
                last_run_at_ms: install.last_run_at_ms,
                local_run_count,
            };
            (
                StatusCode::OK,
                Json(InstallAutomationAppResponse {
                    ok: true,
                    app: installed,
                    install_count: app.install_count.max(0) as u64,
                    newly_installed,
                }),
            )
                .into_response()
        }
    }
}

pub async fn automation_app_uninstall(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(raw_app_id): Path<String>,
) -> Response {
    let app_id = match validate_app_id(&raw_app_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .uninstall_automation_app(auth.user_id.0 as i64, app_id, now_ms)
    {
        Ok(None) => err_resp(
            StatusCode::NOT_FOUND,
            "not_found",
            "Automation app not found",
        ),
        Ok(Some(removed)) => (
            StatusCode::OK,
            Json(UninstallAutomationAppResponse {
                ok: true,
                app_id: app_id.to_string(),
                removed,
            }),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn automation_app_record_run(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(raw_app_id): Path<String>,
    Json(req): Json<RecordAutomationRunRequest>,
) -> Response {
    let app_id = match validate_app_id(&raw_app_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let trimmed_version = match req.version.as_deref() {
        Some(raw_v) => {
            let v = raw_v.trim();
            if v.is_empty() || v.len() > MAX_VERSION_LEN {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    "version must be 1..=64 characters when provided",
                );
            }
            Some(v.to_string())
        }
        None => None,
    };
    let now_ms = Utc::now().timestamp_millis();
    let outcome = match state.db.record_automation_app_run(
        auth.user_id.0 as i64,
        app_id,
        trimmed_version.as_deref(),
        req.success,
        req.steps_executed,
        req.total_duration_ms,
        req.used_llm,
        req.error.as_deref(),
        now_ms,
    ) {
        Ok(o) => o,
        Err(e) => return map_telegram_err(e),
    };

    match outcome {
        RecordAutomationRunOutcome::NotFound => err_resp(
            StatusCode::NOT_FOUND,
            "not_found",
            "Automation app not found",
        ),
        RecordAutomationRunOutcome::VersionNotFound { version } => err_resp(
            StatusCode::BAD_REQUEST,
            "validation_error",
            &format!("Unknown version '{version}' for automation app '{app_id}'"),
        ),
        RecordAutomationRunOutcome::Recorded {
            app,
            last_run_at_ms,
        } => (
            StatusCode::OK,
            Json(RecordAutomationRunResponse {
                ok: true,
                app_id: app.app_id,
                run_count: app.run_count.max(0) as u64,
                last_run_at_ms,
            }),
        )
            .into_response(),
    }
}
