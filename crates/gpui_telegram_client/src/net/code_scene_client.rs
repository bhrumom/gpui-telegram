use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneRequest, AiGenerateSceneResponse, AiIterateSceneRequest, AiPublishSceneRequest,
    CodeSceneAssetDto, CodeSceneCommentSort, CodeSceneFeedRequest, CodeSceneFeedResponse,
    CreateCodeSceneCommentRequest, CreateCodeSceneCommentResponse, ListCodeSceneAssetsQuery,
    ListCodeSceneAssetsResponse, ListCodeSceneCommentsQuery, ListCodeSceneCommentsResponse,
    PublishCodeSceneRequest, PublishCodeSceneResponse, RemixSceneRequest, RemixSceneResponse,
    SceneBehaviorEventDto, TelemetryBatchRequest, TelemetryIngestResponse, TelemetrySubmitPayload,
    ToggleBookmarkResponse, ToggleFollowResponse, ToggleLikeResponse, ValidateCodeSceneRequest,
    ValidationReportDto,
};
use reqwest::Method;

const MAX_CLIENT_SCENE_ID_LEN: usize = 128;

fn require_bearer_token(token: &str) -> Result<&str, HttpClientError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(HttpClientError::Unauthorized(
            "Missing Bearer token".to_string(),
        ));
    }
    Ok(trimmed)
}

fn validate_client_scene_id(raw_id: &str) -> Result<&str, HttpClientError> {
    let id = raw_id.trim();
    if id.is_empty()
        || id.len() > MAX_CLIENT_SCENE_ID_LEN
        || id.eq_ignore_ascii_case("validate")
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(HttpClientError::Api {
            status: 400,
            message:
                "scene_id must be 1..=128 chars of [a-zA-Z0-9_-] and cannot be 'validate'".into(),
        });
    }
    Ok(id)
}

fn validate_creator_user_id(creator_user_id: i64) -> Result<i64, HttpClientError> {
    if creator_user_id <= 0 {
        return Err(HttpClientError::Api {
            status: 400,
            message: "creator_user_id must be positive".into(),
        });
    }
    Ok(creator_user_id)
}

fn comment_sort_str(sort: CodeSceneCommentSort) -> &'static str {
    match sort {
        CodeSceneCommentSort::Hot => "hot",
        CodeSceneCommentSort::Newest => "newest",
    }
}

/// Typed HTTP client for all `/api/v1/code-scene/*` endpoints.
/// Delegates transport to `HttpClient` so calls are safe from both Tokio tasks and non-Tokio GPUI threads.
#[derive(Clone)]
pub struct CodeSceneHttpClient {
    http: HttpClient,
}

impl CodeSceneHttpClient {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub fn from_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(base_url.into()),
        }
    }

    pub fn http_client(&self) -> &HttpClient {
        &self.http
    }

    pub fn base_url(&self) -> &str {
        &self.http.base_url
    }

    /// 1. `POST /api/v1/code-scene/feed`
    pub async fn fetch_feed(
        &self,
        token: &str,
        req: &CodeSceneFeedRequest,
    ) -> Result<CodeSceneFeedResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(Method::POST, "/api/v1/code-scene/feed", Some(tok), Some(req))
            .await
    }

    /// 2. `GET /api/v1/code-scene/assets`
    pub async fn list_assets(
        &self,
        token: &str,
        query: &ListCodeSceneAssetsQuery,
    ) -> Result<ListCodeSceneAssetsResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let mut params: Vec<String> = Vec::new();
        if let Some(q) = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("q={}", percent_encode_rfc3986(q)));
        }
        if let Some(kind) = query.kind {
            params.push(format!("kind={}", kind.as_str()));
        }
        if let Some(tag) = query.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("tag={}", percent_encode_rfc3986(tag)));
        }
        if let Some(uid) = query.author_user_id {
            params.push(format!("author_user_id={uid}"));
        }
        if let Some(bid) = query.creator_bot_id {
            params.push(format!("creator_bot_id={bid}"));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        let path = if params.is_empty() {
            "/api/v1/code-scene/assets".to_string()
        } else {
            format!("/api/v1/code-scene/assets?{}", params.join("&"))
        };
        self.http
            .request_json::<(), _>(Method::GET, &path, Some(tok), None)
            .await
    }

    /// 3. `GET /api/v1/code-scene/assets/{id}`
    pub async fn get_asset(
        &self,
        token: &str,
        scene_id: &str,
    ) -> Result<CodeSceneAssetDto, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let id = validate_client_scene_id(scene_id)?;
        let path = format!("/api/v1/code-scene/assets/{id}");
        self.http
            .request_json::<(), _>(Method::GET, &path, Some(tok), None)
            .await
    }

    /// 4. `POST /api/v1/code-scene/assets`
    pub async fn publish_asset(
        &self,
        token: &str,
        req: &PublishCodeSceneRequest,
    ) -> Result<PublishCodeSceneResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        if let Some(custom_id) = req
            .scene_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            validate_client_scene_id(custom_id)?;
        }
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/assets",
                Some(tok),
                Some(req),
            )
            .await
    }

    /// 5. `POST /api/v1/code-scene/assets/validate`
    pub async fn validate_asset(
        &self,
        token: &str,
        req: &ValidateCodeSceneRequest,
    ) -> Result<ValidationReportDto, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/assets/validate",
                Some(tok),
                Some(req),
            )
            .await
    }

    /// 6. `POST /api/v1/code-scene/assets/{id}/like`
    pub async fn toggle_like(
        &self,
        token: &str,
        scene_id: &str,
    ) -> Result<ToggleLikeResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let id = validate_client_scene_id(scene_id)?;
        let path = format!("/api/v1/code-scene/assets/{id}/like");
        self.http
            .request_json::<(), _>(Method::POST, &path, Some(tok), None)
            .await
    }

    /// 7. `POST /api/v1/code-scene/assets/{id}/bookmark`
    pub async fn toggle_bookmark(
        &self,
        token: &str,
        scene_id: &str,
    ) -> Result<ToggleBookmarkResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let id = validate_client_scene_id(scene_id)?;
        let path = format!("/api/v1/code-scene/assets/{id}/bookmark");
        self.http
            .request_json::<(), _>(Method::POST, &path, Some(tok), None)
            .await
    }

    /// 8. `GET /api/v1/code-scene/assets/{id}/comments`
    pub async fn list_comments(
        &self,
        token: &str,
        scene_id: &str,
        query: &ListCodeSceneCommentsQuery,
    ) -> Result<ListCodeSceneCommentsResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let id = validate_client_scene_id(scene_id)?;
        let mut params: Vec<String> = Vec::new();
        if let Some(sort) = query.sort {
            params.push(format!("sort={}", comment_sort_str(sort)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        let path = if params.is_empty() {
            format!("/api/v1/code-scene/assets/{id}/comments")
        } else {
            format!(
                "/api/v1/code-scene/assets/{id}/comments?{}",
                params.join("&")
            )
        };
        self.http
            .request_json::<(), _>(Method::GET, &path, Some(tok), None)
            .await
    }

    /// 9. `POST /api/v1/code-scene/assets/{id}/comments`
    pub async fn add_comment(
        &self,
        token: &str,
        scene_id: &str,
        req: &CreateCodeSceneCommentRequest,
    ) -> Result<CreateCodeSceneCommentResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let id = validate_client_scene_id(scene_id)?;
        let path = format!("/api/v1/code-scene/assets/{id}/comments");
        self.http
            .request_json(Method::POST, &path, Some(tok), Some(req))
            .await
    }

    /// 10. `POST /api/v1/code-scene/creators/{id}/follow`
    pub async fn toggle_follow(
        &self,
        token: &str,
        creator_user_id: i64,
    ) -> Result<ToggleFollowResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let uid = validate_creator_user_id(creator_user_id)?;
        let path = format!("/api/v1/code-scene/creators/{uid}/follow");
        self.http
            .request_json::<(), _>(Method::POST, &path, Some(tok), None)
            .await
    }

    /// 11a. `POST /api/v1/code-scene/telemetry` (single or batch payload)
    pub async fn submit_telemetry(
        &self,
        token: &str,
        payload: &TelemetrySubmitPayload,
    ) -> Result<TelemetryIngestResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/telemetry",
                Some(tok),
                Some(payload),
            )
            .await
    }

    /// 11b. `POST /api/v1/code-scene/telemetry` (single event)
    pub async fn submit_telemetry_event(
        &self,
        token: &str,
        event: &SceneBehaviorEventDto,
    ) -> Result<TelemetryIngestResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/telemetry",
                Some(tok),
                Some(event),
            )
            .await
    }

    /// 11c. `POST /api/v1/code-scene/telemetry` (batch)
    pub async fn submit_telemetry_batch(
        &self,
        token: &str,
        events: Vec<SceneBehaviorEventDto>,
    ) -> Result<TelemetryIngestResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        let body = TelemetryBatchRequest { events };
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/telemetry",
                Some(tok),
                Some(&body),
            )
            .await
    }

    /// 12. `POST /api/v1/code-scene/ai/generate`
    pub async fn ai_generate(
        &self,
        token: &str,
        req: &AiGenerateSceneRequest,
    ) -> Result<AiGenerateSceneResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/ai/generate",
                Some(tok),
                Some(req),
            )
            .await
    }

    /// 13. `POST /api/v1/code-scene/ai/iterate`
    pub async fn ai_iterate(
        &self,
        token: &str,
        req: &AiIterateSceneRequest,
    ) -> Result<AiGenerateSceneResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/ai/iterate",
                Some(tok),
                Some(req),
            )
            .await
    }

    /// 14. `POST /api/v1/code-scene/ai/publish`
    pub async fn ai_publish(
        &self,
        token: &str,
        req: &AiPublishSceneRequest,
    ) -> Result<PublishCodeSceneResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/ai/publish",
                Some(tok),
                Some(req),
            )
            .await
    }

    /// 15. `POST /api/v1/code-scene/ai/remix`
    pub async fn ai_remix(
        &self,
        token: &str,
        req: &RemixSceneRequest,
    ) -> Result<RemixSceneResponse, HttpClientError> {
        let tok = require_bearer_token(token)?;
        validate_client_scene_id(&req.parent_scene_id)?;
        self.http
            .request_json(
                Method::POST,
                "/api/v1/code-scene/ai/remix",
                Some(tok),
                Some(req),
            )
            .await
    }
}
