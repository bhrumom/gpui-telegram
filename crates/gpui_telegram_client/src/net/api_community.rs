//! Typed async HTTP client for all 50 `/api/v1/community/*` REST endpoints
//! (`PROJECT.md § Interface Contracts #3`).
//!
//! Inherits `HttpClient`'s `X-Device-Id`, `.no_proxy()`, fail-closed proxy configuration,
//! timeouts, and Tokio runtime hopping while preserving exact HTTP status, `Content-Type`,
//! and JSON error envelopes for unambiguous online-vs-local fallback classification.

use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    ActorProfileResponse, AlgoPreferencesDto, AwardResponse, BookmarkRequest, BookmarkResponse,
    CommentSort, CommentsListResponse, CommentsQueryParams, CommunityActorDto,
    CommunityCommentDto, CommunityDetailResponse, CommunityIdentitiesResponse, CommunityListDto,
    CommunityListsResponse, CommunityMuteBlockDto, CommunityNotificationKind,
    CommunityPollVoteRequest, CommunityPostDto, CommunitySearchRequestDto,
    CommunitySearchResultDto, CommunitySearchSort, CommunitySearchTab, CommunityWikiRevisionDto,
    CreateCommentRequest, CreateCommentResponse, CreateCommunityListRequest,
    CreateCommunityRequest, CreatePostRequest, CreatePostResponse, DeleteEntityResponse,
    FeedRequestDto, FeedResponseDto, FeedTab, FollowActionResponse, GiveAwardRequest,
    JoinCommunityResponse, ListCommunitiesResponse, MarkNotificationsReadRequest,
    MarkNotificationsReadResponse, ModActionRequest, ModLogEntryDto, ModLogsResponse,
    ModQueueResponse, ModQueueStatus, MuteBlockListResponse, MuteBlockRequest,
    NotificationsListResponse, PollDto, PostDetailResponse, RecordSignalRequest,
    RecordSignalResponse, RegisterBotActorRequest, RegisterHumanActorRequest, RepostRequest,
    RepostResponse, SubCommunityDto, SummonBotRequest, SummonBotResponse, ToggleActionResponse,
    TrendingResponse, UpdateCommentRequest, UpdateCommunityActorRequest,
    UpdateCommunityListRequest, UpdateCommunityRequest, UpdateCommunityWikiRequest,
    UpdatePostRequest, VoteRequest, VoteResponse,
};
use reqwest::header::{HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical HTTP header used to act as a Human or owned Bot citizen.
pub const HEADER_COMMUNITY_ACTOR_ID: &str = "X-Community-Actor-Id";
/// Backwards-compatible alias for `HEADER_COMMUNITY_ACTOR_ID`.
pub const COMMUNITY_ACTOR_HEADER: &str = HEADER_COMMUNITY_ACTOR_ID;

/// Authentication scheme for `/api/v1/community/*` HTTP calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityAuth<'a> {
    Bearer(&'a str),
    Bot(&'a str),
}

/// Decoded result of `POST /api/v1/community/identities/bot`, supporting both
/// `{ "actor": ..., "bot_token"|"token": ... }` and bare `CommunityActorDto`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterBotActorResult {
    pub actor: CommunityActorDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

/// Unambiguous Community HTTP error separating fallback-eligible failures from live API errors.
#[derive(Debug, Clone, Error)]
pub enum CommunityApiError {
    /// TCP connect refused, closed port, DNS failure, timeout, reset, TLS, or body read failure.
    #[error("Community transport error: {message}")]
    Transport {
        message: String,
        is_timeout: bool,
        is_connect: bool,
    },
    /// HTTP 404 from a legacy server lacking `/api/v1/community/*` routes.
    #[error("Community endpoint not found on server (HTTP {status}): {message}")]
    LegacyRouteNotFound { status: u16, message: String },
    /// HTTP 404 JSON domain error from a live Community server (missing post/actor/community/etc.).
    #[error("Community entity not found (404 {code}): {message}")]
    DomainNotFound { code: String, message: String },
    /// Fail-closed proxy configuration error (MUST NOT fall back locally).
    #[error("Community proxy error: {0}")]
    Proxy(String),
    /// Genuine non-404 HTTP error from a reachable server (400, 401, 403, 409, 410, 422, 429, 5xx).
    #[error("Community API error (HTTP {status} {code}): {message}")]
    Http {
        status: u16,
        code: String,
        message: String,
    },
    /// Client-side input or header validation failure (e.g., CRLF injection, empty path ID).
    #[error("Community request validation error: {0}")]
    InvalidInput(String),
    /// Malformed JSON payload on a 2xx response or request serialization error.
    #[error("Community response serialization error: {0}")]
    Serialization(String),
}

impl CommunityApiError {
    /// Returns `true` ONLY for connectivity failures and legacy unmounted 404 routes.
    /// Live domain 404s, 400/401/403/409/410/422/429/5xx, proxy, validation, and serialization
    /// errors ALWAYS return `false` and MUST propagate cleanly.
    pub fn should_fallback_to_local(&self) -> bool {
        matches!(
            self,
            Self::Transport { .. } | Self::LegacyRouteNotFound { .. }
        )
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::LegacyRouteNotFound { status, .. } => Some(*status),
            Self::DomainNotFound { .. } => Some(404),
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Transport { is_timeout: true, .. })
    }
}

impl From<CommunityApiError> for TelegramError {
    fn from(err: CommunityApiError) -> Self {
        match err {
            CommunityApiError::Transport { message, .. } => TelegramError::Network(message),
            CommunityApiError::Proxy(m) => TelegramError::Network(format!("Proxy error: {m}")),
            CommunityApiError::LegacyRouteNotFound { message, .. }
            | CommunityApiError::DomainNotFound { message, .. } => TelegramError::NotFound(message),
            CommunityApiError::Http {
                status: 401,
                message,
                ..
            } => TelegramError::Auth(format!("HTTP 401: {message}")),
            CommunityApiError::Http {
                status: 403,
                message,
                ..
            } => TelegramError::PermissionDenied(format!("HTTP 403: {message}")),
            CommunityApiError::Http {
                status: status @ (400 | 409 | 422),
                message,
                ..
            } => TelegramError::Validation(format!("HTTP {status}: {message}")),
            CommunityApiError::Http {
                status: 410,
                message,
                ..
            } => TelegramError::NotFound(format!("HTTP 410: {message}")),
            CommunityApiError::Http {
                status: 429,
                message,
                ..
            } => TelegramError::Validation(format!("HTTP 429: {message}")),
            CommunityApiError::Http {
                status, message, ..
            } => TelegramError::Internal(format!("HTTP {status}: {message}")),
            CommunityApiError::InvalidInput(m) => TelegramError::Validation(m),
            CommunityApiError::Serialization(m) => TelegramError::Serialization(m),
        }
    }
}

#[derive(Debug)]
struct RawCommunityHttpResponse {
    status: u16,
    content_type: Option<String>,
    body: String,
}

/// Deterministic classifier separating legacy unmounted 404 routes from live Community domain 404s.
pub(crate) fn classify_404(content_type: Option<&str>, body: &str) -> CommunityApiError {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: "Not Found".to_string(),
        };
    }

    if let Some(ct) = content_type {
        let ct_lower = ct.to_ascii_lowercase();
        if ct_lower.starts_with("text/html") {
            return CommunityApiError::LegacyRouteNotFound {
                status: 404,
                message: "Not Found".to_string(),
            };
        }
    }

    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(trimmed)
    else {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: trimmed.to_string(),
        };
    };

    let Some(error_code) = obj
        .get("error")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: trimmed.to_string(),
        };
    };
    let message = obj.get("message").and_then(|v| v.as_str()).map(str::trim);

    let code_l = error_code.to_ascii_lowercase();
    let msg_l = message.unwrap_or("").to_ascii_lowercase();

    const LEGACY_CODES: &[&str] = &[
        "route_not_found",
        "endpoint_not_found",
        "path_not_found",
        "method_not_found",
        "not_implemented",
        "unsupported",
    ];
    if LEGACY_CODES.contains(&code_l.as_str()) {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: message
                .filter(|m| !m.is_empty())
                .unwrap_or("Not Found")
                .to_string(),
        };
    }

    const DOMAIN_NOT_FOUND_CODES: &[&str] = &[
        "post_not_found",
        "comment_not_found",
        "community_not_found",
        "actor_not_found",
        "bot_not_found",
        "list_not_found",
        "poll_not_found",
        "user_not_found",
        "rule_not_found",
        "wiki_not_found",
        "queue_item_not_found",
        "entity_not_found",
        "draft_not_found",
    ];
    if DOMAIN_NOT_FOUND_CODES.contains(&code_l.as_str()) {
        return CommunityApiError::DomainNotFound {
            code: error_code.to_string(),
            message: message
                .filter(|m| !m.is_empty())
                .unwrap_or(error_code)
                .to_string(),
        };
    }

    let Some(message_str) = message else {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: error_code.to_string(),
        };
    };
    if msg_l.is_empty() || msg_l == "not found" || msg_l == "404 not found" {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: if message_str.is_empty() {
                "Not Found".to_string()
            } else {
                message_str.to_string()
            },
        };
    }

    const LEGACY_MSG_MARKERS: &[&str] = &[
        "/api/v1/",
        "route not found",
        "no route",
        "unknown route",
        "route_not_mounted",
        "endpoint not found",
        "unknown endpoint",
        "invalid endpoint",
        "no handler",
        "path not found",
        "not supported",
        "unimplemented",
        "not implemented",
    ];
    if LEGACY_MSG_MARKERS.iter().any(|m| msg_l.contains(m)) {
        return CommunityApiError::LegacyRouteNotFound {
            status: 404,
            message: message_str.to_string(),
        };
    }

    const DOMAIN_NOUNS: &[&str] = &[
        "post",
        "comment",
        "community",
        "actor",
        "author",
        "creator",
        "target",
        "follower",
        "giver",
        "editor",
        "moderator",
        "list",
        "bot",
        "poll",
        "queue item",
        "wiki",
        "rule",
        "notification",
        "user",
        "draft",
        "flair",
        "thread",
        "bookmark",
        "award",
        "member",
        "slug",
        "report",
        "mute",
        "block",
        "entity",
        "revision",
    ];
    if code_l == "not_found" && DOMAIN_NOUNS.iter().any(|noun| msg_l.contains(noun)) {
        return CommunityApiError::DomainNotFound {
            code: error_code.to_string(),
            message: message_str.to_string(),
        };
    }

    CommunityApiError::LegacyRouteNotFound {
        status: 404,
        message: message_str.to_string(),
    }
}

fn classify_non_success(resp: &RawCommunityHttpResponse) -> CommunityApiError {
    if resp.status == 404 {
        return classify_404(resp.content_type.as_deref(), &resp.body);
    }
    let trimmed = resp.body.trim();
    let (code, message) = serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .and_then(|v| {
            let obj = v.as_object()?;
            let code = obj
                .get("error")
                .and_then(|x| x.as_str())
                .unwrap_or("http_error")
                .to_string();
            let msg = obj
                .get("message")
                .or_else(|| obj.get("error"))
                .and_then(|x| x.as_str())
                .unwrap_or(trimmed)
                .to_string();
            Some((code, msg))
        })
        .unwrap_or_else(|| {
            let fallback = if trimmed.is_empty() {
                reqwest::StatusCode::from_u16(resp.status)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                    .unwrap_or("HTTP error")
                    .to_string()
            } else {
                trimmed.to_string()
            };
            ("http_error".to_string(), fallback)
        });
    CommunityApiError::Http {
        status: resp.status,
        code,
        message,
    }
}

/// Validates and normalizes an explicit actor ID (`X-Community-Actor-Id` / `act_as`).
/// Rejects all ASCII control bytes (`< 0x20`, `0x7f`), ALL non-ASCII bytes (`>= 0x80`, covering
/// Unicode whitespace NBSP/U+3000/NEL/U+2028, zero-width/BOM U+200B..U+FEFF, and obs-text smuggling),
/// empty/ASCII-space-only values, and embedded ASCII spaces. Valid actor IDs (`u:123`, `b:456`,
/// `"  b:42  "`) normalize cleanly to printable non-space ASCII (`0x21..=0x7e`).
pub(crate) fn validate_and_normalize_actor_id(raw: &str) -> std::result::Result<&str, &'static str> {
    if raw.bytes().any(|b| b < 0x20 || b >= 0x7f) {
        return Err("actor_id contains invalid control or non-ASCII characters");
    }
    let trimmed = raw.trim_matches(' ');
    if trimmed.is_empty() {
        return Err("actor_id must not be empty");
    }
    if trimmed.bytes().any(|b| b == b' ') {
        return Err("actor_id must not contain embedded whitespace");
    }
    Ok(trimmed)
}

fn validate_header_value(field: &str, raw: &str) -> Result<String, CommunityApiError> {
    if raw.bytes().any(|b| b < 0x20 || b >= 0x7f) {
        return Err(CommunityApiError::InvalidInput(format!(
            "Invalid characters in {field} header"
        )));
    }
    let trimmed = raw.trim_matches(' ');
    if trimmed.is_empty() {
        return Err(CommunityApiError::InvalidInput(format!(
            "{field} must not be empty"
        )));
    }
    if HeaderValue::from_str(trimmed).is_err() {
        return Err(CommunityApiError::InvalidInput(format!(
            "Invalid characters in {field} header"
        )));
    }
    Ok(trimmed.to_string())
}

fn encode_non_empty_segment(field: &str, raw: &str) -> Result<String, CommunityApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CommunityApiError::InvalidInput(format!(
            "{field} path parameter must not be empty"
        )));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(CommunityApiError::InvalidInput(format!(
            "{field} must not be '.' or '..'"
        )));
    }
    Ok(percent_encode_rfc3986(trimmed))
}

fn encode_handle_or_id(raw: &str) -> Result<String, CommunityApiError> {
    let stripped = raw.trim().trim_start_matches('@');
    encode_non_empty_segment("handle_or_id", stripped)
}

fn encode_community_slug(raw: &str) -> Result<String, CommunityApiError> {
    let trimmed = raw.trim();
    let lowered = trimmed.to_ascii_lowercase();
    let clean = lowered
        .strip_prefix("c/")
        .or_else(|| lowered.strip_prefix("r/"))
        .unwrap_or(&lowered);
    encode_non_empty_segment("slug", clean)
}

fn search_tab_as_str(tab: CommunitySearchTab) -> &'static str {
    match tab {
        CommunitySearchTab::Top => "top",
        CommunitySearchTab::Latest => "latest",
        CommunitySearchTab::People => "people",
        CommunitySearchTab::Bots => "bots",
        CommunitySearchTab::Communities => "communities",
        CommunitySearchTab::Media => "media",
        CommunitySearchTab::Polls => "polls",
    }
}

fn search_sort_as_str(sort: CommunitySearchSort) -> &'static str {
    match sort {
        CommunitySearchSort::Relevance => "relevance",
        CommunitySearchSort::Latest => "latest",
        CommunitySearchSort::Top => "top",
    }
}

pub(crate) fn encode_feed_query(req: &FeedRequestDto) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(10);
    match &req.tab {
        FeedTab::ForYou => parts.push("tab=for_you".to_string()),
        FeedTab::Following => parts.push("tab=following".to_string()),
        FeedTab::Popular => parts.push("tab=popular".to_string()),
        FeedTab::All => parts.push("tab=all".to_string()),
        FeedTab::Saved => parts.push("tab=saved".to_string()),
        FeedTab::Community(slug) => {
            parts.push(format!("community={}", percent_encode_rfc3986(slug.trim())));
        }
        FeedTab::List(id) => {
            parts.push(format!("list_id={}", percent_encode_rfc3986(id.trim())));
        }
        FeedTab::ActorPosts(actor) => {
            parts.push(format!("actor={}", percent_encode_rfc3986(actor.trim())));
        }
    }
    parts.push(format!("sort={}", req.sort.as_str()));
    parts.push(format!("time={}", req.time_window.as_str()));
    parts.push(format!("actor_filter={}", req.actor_filter.as_str()));
    if let Some(flair) = req.flair.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("flair={}", percent_encode_rfc3986(flair)));
    }
    if let Some(cursor) = req
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("cursor={}", percent_encode_rfc3986(cursor)));
    }
    parts.push(format!("limit={}", req.limit.clamp(1, 100)));
    parts.join("&")
}

pub(crate) fn encode_comments_query(params: &CommentsQueryParams) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(sort) = params.sort {
        parts.push(format!("sort={}", sort.as_str()));
    }
    if let Some(pid) = params
        .parent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("parent_id={}", percent_encode_rfc3986(pid)));
    }
    if let Some(depth) = params.max_depth {
        parts.push(format!("max_depth={depth}"));
    }
    if let Some(limit) = params.limit {
        parts.push(format!("limit={}", limit.clamp(1, 200)));
    }
    if let Some(cursor) = params
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("cursor={}", percent_encode_rfc3986(cursor)));
    }
    parts.join("&")
}

pub(crate) fn encode_search_query(req: &CommunitySearchRequestDto) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(11);
    parts.push(format!("query={}", percent_encode_rfc3986(&req.query)));
    parts.push(format!("tab={}", search_tab_as_str(req.tab)));
    parts.push(format!("sort={}", search_sort_as_str(req.sort)));
    if let Some(slug) = req
        .community_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("community_slug={}", percent_encode_rfc3986(slug)));
    }
    if let Some(author) = req
        .author_handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("author_handle={}", percent_encode_rfc3986(author)));
    }
    if let Some(flair) = req.flair.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("flair={}", percent_encode_rfc3986(flair)));
    }
    if let Some(kind) = req.actor_kind {
        parts.push(format!("actor_kind={}", kind.as_str()));
    }
    if let Some(has_media) = req.has_media {
        parts.push(format!("has_media={has_media}"));
    }
    if let Some(has_poll) = req.has_poll {
        parts.push(format!("has_poll={has_poll}"));
    }
    parts.push(format!("limit={}", req.limit.clamp(1, 100)));
    parts.push(format!("offset={}", req.offset));
    parts.join("&")
}

fn bearer_auth(token: Option<&str>) -> Option<CommunityAuth<'_>> {
    token
        .filter(|t| !t.is_empty())
        .map(CommunityAuth::Bearer)
}

impl HttpClient {
    async fn community_send_raw(
        &self,
        method: Method,
        path: &str,
        auth: Option<CommunityAuth<'_>>,
        act_as_actor_id: Option<&str>,
        json_body: Option<serde_json::Value>,
    ) -> Result<RawCommunityHttpResponse, CommunityApiError> {
        let auth_header: Option<HeaderValue> = match auth {
            Some(CommunityAuth::Bearer(raw)) => {
                if raw.bytes().any(|b| b < 0x20 || b >= 0x7f) {
                    return Err(CommunityApiError::InvalidInput(
                        "Invalid control or non-ASCII characters in Authorization header".into(),
                    ));
                }
                if raw.trim_matches(' ').is_empty() {
                    None
                } else {
                    let clean = validate_header_value("Authorization", raw)?;
                    Some(
                        HeaderValue::from_str(&format!("Bearer {clean}")).map_err(|_| {
                            CommunityApiError::InvalidInput(
                                "Invalid Authorization Bearer header".into(),
                            )
                        })?,
                    )
                }
            }
            Some(CommunityAuth::Bot(raw)) => {
                if raw.bytes().any(|b| b < 0x20 || b >= 0x7f) {
                    return Err(CommunityApiError::InvalidInput(
                        "Invalid control or non-ASCII characters in Authorization header".into(),
                    ));
                }
                if raw.trim_matches(' ').is_empty() {
                    None
                } else {
                    let clean = validate_header_value("Authorization", raw)?;
                    Some(
                        HeaderValue::from_str(&format!("Bot {clean}")).map_err(|_| {
                            CommunityApiError::InvalidInput(
                                "Invalid Authorization Bot header".into(),
                            )
                        })?,
                    )
                }
            }
            None => None,
        };

        let actor_header: Option<HeaderValue> = match act_as_actor_id {
            Some(raw) if !raw.is_empty() => {
                if raw.bytes().any(|b| b < 0x20 || b >= 0x7f) {
                    return Err(CommunityApiError::InvalidInput(format!(
                        "Invalid control or non-ASCII characters in {HEADER_COMMUNITY_ACTOR_ID} header"
                    )));
                }
                let trimmed = raw.trim_matches(' ');
                if trimmed.is_empty() {
                    None
                } else {
                    let clean = validate_and_normalize_actor_id(trimmed).map_err(|msg| {
                        CommunityApiError::InvalidInput(format!(
                            "Invalid {HEADER_COMMUNITY_ACTOR_ID} header: {msg}"
                        ))
                    })?;
                    Some(HeaderValue::from_str(clean).map_err(|_| {
                        CommunityApiError::InvalidInput(format!(
                            "Invalid {HEADER_COMMUNITY_ACTOR_ID} header"
                        ))
                    })?)
                }
            }
            _ => None,
        };

        let client = self.client.clone();
        let url = self.endpoint_url(path);
        let outcome = self
            .run_on_tokio(async move {
                let mut req = client.request(method, &url);
                if let Some(auth_val) = auth_header {
                    req = req.header(AUTHORIZATION, auth_val);
                }
                if let Some(actor_val) = actor_header {
                    req = req.header(HeaderName::from_static("x-community-actor-id"), actor_val);
                }
                if let Some(ref val) = json_body {
                    req = req.json(val);
                }
                match req.send().await {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let content_type = resp
                            .headers()
                            .get(CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        match resp.text().await {
                            Ok(body) => Ok(Ok(RawCommunityHttpResponse {
                                status,
                                content_type,
                                body,
                            })),
                            Err(e) => Ok(Err(CommunityApiError::Transport {
                                message: e.to_string(),
                                is_timeout: e.is_timeout(),
                                is_connect: e.is_connect(),
                            })),
                        }
                    }
                    Err(e) => Ok(Err(CommunityApiError::Transport {
                        message: e.to_string(),
                        is_timeout: e.is_timeout(),
                        is_connect: e.is_connect(),
                    })),
                }
            })
            .await;

        match outcome {
            Ok(inner) => inner,
            Err(HttpClientError::Proxy(msg)) => Err(CommunityApiError::Proxy(msg)),
            Err(other) => Err(CommunityApiError::Transport {
                message: other.to_string(),
                is_timeout: false,
                is_connect: false,
            }),
        }
    }

    /// Executes a typed JSON Community REST call with arbitrary auth (`Bearer` or `Bot`)
    /// and optional `X-Community-Actor-Id`.
    pub async fn community_request_json<B, R>(
        &self,
        method: Method,
        path: &str,
        auth: Option<CommunityAuth<'_>>,
        act_as_actor_id: Option<&str>,
        body: Option<&B>,
    ) -> Result<R, CommunityApiError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let json_body = match body {
            Some(b) => Some(
                serde_json::to_value(b)
                    .map_err(|e| CommunityApiError::Serialization(e.to_string()))?,
            ),
            None => None,
        };
        let raw = self
            .community_send_raw(method, path, auth, act_as_actor_id, json_body)
            .await?;
        if !(200..300).contains(&raw.status) {
            return Err(classify_non_success(&raw));
        }
        serde_json::from_str::<R>(&raw.body)
            .map_err(|e| CommunityApiError::Serialization(e.to_string()))
    }
}

impl HttpClient {
    // -------------------------------------------------------------------------
    // A. Identities, Actors, Follow & Mutes/Blocks (Endpoints 1–10)
    // -------------------------------------------------------------------------

    /// 1. `GET /api/v1/community/identities`
    pub async fn community_get_identities(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
    ) -> Result<CommunityIdentitiesResponse, CommunityApiError> {
        self.community_request_json::<(), _>(
            Method::GET,
            "/api/v1/community/identities",
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 2. `POST /api/v1/community/identities/human`
    pub async fn community_register_human_actor(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &RegisterHumanActorRequest,
    ) -> Result<CommunityActorDto, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/identities/human",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 3. `POST /api/v1/community/identities/bot`
    /// Accepts both `{ "actor": ..., "bot_token"|"token": ... }` and bare `CommunityActorDto`.
    pub async fn community_register_bot_actor(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &RegisterBotActorRequest,
    ) -> Result<RegisterBotActorResult, CommunityApiError> {
        let value: serde_json::Value = self
            .community_request_json(
                Method::POST,
                "/api/v1/community/identities/bot",
                bearer_auth(token),
                act_as_actor_id,
                Some(req),
            )
            .await?;
        if let Some(actor_val) = value.get("actor") {
            let actor: CommunityActorDto = serde_json::from_value(actor_val.clone())
                .map_err(|e| CommunityApiError::Serialization(e.to_string()))?;
            let bot_token = value
                .get("bot_token")
                .or_else(|| value.get("token"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let bot_user_id = value.get("bot_user_id").and_then(|v| v.as_i64());
            let username = value
                .get("username")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            return Ok(RegisterBotActorResult {
                actor,
                bot_token,
                bot_user_id,
                username,
            });
        }
        let actor: CommunityActorDto = serde_json::from_value(value)
            .map_err(|e| CommunityApiError::Serialization(e.to_string()))?;
        Ok(RegisterBotActorResult {
            bot_user_id: actor.linked_user_id,
            username: Some(actor.handle.clone()),
            actor,
            bot_token: None,
        })
    }

    /// 4. `PATCH /api/v1/community/actors/{actor_id}`
    pub async fn community_update_actor(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        actor_id: &str,
        req: &UpdateCommunityActorRequest,
    ) -> Result<CommunityActorDto, CommunityApiError> {
        let seg = encode_non_empty_segment("actor_id", actor_id)?;
        let path = format!("/api/v1/community/actors/{seg}");
        self.community_request_json(
            Method::PATCH,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 5. `GET /api/v1/community/actors/{handle_or_id}`
    pub async fn community_get_actor_profile(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        handle_or_id: &str,
    ) -> Result<ActorProfileResponse, CommunityApiError> {
        let seg = encode_handle_or_id(handle_or_id)?;
        let path = format!("/api/v1/community/actors/{seg}");
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 6. `POST /api/v1/community/actors/{actor_id}/follow`
    pub async fn community_follow_actor(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        actor_id: &str,
    ) -> Result<FollowActionResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("actor_id", actor_id)?;
        let path = format!("/api/v1/community/actors/{seg}/follow");
        self.community_request_json::<(), _>(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 7. `DELETE /api/v1/community/actors/{actor_id}/follow`
    pub async fn community_unfollow_actor(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        actor_id: &str,
    ) -> Result<FollowActionResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("actor_id", actor_id)?;
        let path = format!("/api/v1/community/actors/{seg}/follow");
        self.community_request_json::<(), _>(
            Method::DELETE,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 8. `GET /api/v1/community/mutes-blocks`
    pub async fn community_list_mutes_blocks(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
    ) -> Result<MuteBlockListResponse, CommunityApiError> {
        self.community_request_json::<(), _>(
            Method::GET,
            "/api/v1/community/mutes-blocks",
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 9. `POST /api/v1/community/mutes-blocks`
    pub async fn community_set_mute_block(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &MuteBlockRequest,
    ) -> Result<CommunityMuteBlockDto, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/mutes-blocks",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 10. `DELETE /api/v1/community/mutes-blocks`
    /// Sends BOTH RFC 3986 query params and JSON body `MuteBlockRequest` for full server interop.
    pub async fn community_remove_mute_block(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &MuteBlockRequest,
    ) -> Result<DeleteEntityResponse, CommunityApiError> {
        let path = format!(
            "/api/v1/community/mutes-blocks?target_kind={}&target_value={}&action={}",
            req.target_kind.as_str(),
            percent_encode_rfc3986(req.target_value.trim()),
            req.action.as_str()
        );
        let body = serde_json::to_value(req)
            .map_err(|e| CommunityApiError::Serialization(e.to_string()))?;
        let raw = self
            .community_send_raw(
                Method::DELETE,
                &path,
                bearer_auth(token),
                act_as_actor_id,
                Some(body),
            )
            .await?;
        if !(200..300).contains(&raw.status) {
            return Err(classify_non_success(&raw));
        }
        if raw.status == 204 || raw.body.trim().is_empty() {
            return Ok(DeleteEntityResponse {
                id: format!(
                    "{}:{}:{}",
                    req.target_kind.as_str(),
                    req.action.as_str(),
                    req.target_value.trim()
                ),
                deleted: true,
            });
        }
        serde_json::from_str::<DeleteEntityResponse>(&raw.body)
            .map_err(|e| CommunityApiError::Serialization(e.to_string()))
    }
}

impl HttpClient {
    // -------------------------------------------------------------------------
    // B. Feed, Algorithm Preferences & Signals (Endpoints 11–14)
    // -------------------------------------------------------------------------

    /// 11. `GET /api/v1/community/feed`
    pub async fn community_get_feed(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &FeedRequestDto,
    ) -> Result<FeedResponseDto, CommunityApiError> {
        let query = encode_feed_query(req);
        let path = format!("/api/v1/community/feed?{query}");
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 12. `GET /api/v1/community/algo-prefs`
    pub async fn community_get_algo_prefs(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
    ) -> Result<AlgoPreferencesDto, CommunityApiError> {
        self.community_request_json::<(), _>(
            Method::GET,
            "/api/v1/community/algo-prefs",
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 13. `PUT /api/v1/community/algo-prefs`
    pub async fn community_update_algo_prefs(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        prefs: &AlgoPreferencesDto,
    ) -> Result<AlgoPreferencesDto, CommunityApiError> {
        self.community_request_json(
            Method::PUT,
            "/api/v1/community/algo-prefs",
            bearer_auth(token),
            act_as_actor_id,
            Some(prefs),
        )
        .await
    }

    /// 14. `POST /api/v1/community/signals`
    pub async fn community_record_signal(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &RecordSignalRequest,
    ) -> Result<RecordSignalResponse, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/signals",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    // -------------------------------------------------------------------------
    // C. Posts & Post Interactions (Endpoints 15–25)
    // -------------------------------------------------------------------------

    /// 15. `POST /api/v1/community/posts`
    pub async fn community_create_post(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &CreatePostRequest,
    ) -> Result<CreatePostResponse, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/posts",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 16. `GET /api/v1/community/posts/{post_id}`
    pub async fn community_get_post(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        comment_sort: Option<CommentSort>,
    ) -> Result<PostDetailResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = match comment_sort {
            Some(sort) => format!(
                "/api/v1/community/posts/{seg}?sort={0}&comment_sort={0}",
                sort.as_str()
            ),
            None => format!("/api/v1/community/posts/{seg}"),
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 17. `PATCH /api/v1/community/posts/{post_id}`
    pub async fn community_update_post(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &UpdatePostRequest,
    ) -> Result<CommunityPostDto, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}");
        self.community_request_json(
            Method::PATCH,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 18. `DELETE /api/v1/community/posts/{post_id}`
    pub async fn community_delete_post(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
    ) -> Result<DeleteEntityResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}");
        self.community_request_json::<(), _>(
            Method::DELETE,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 19. `POST /api/v1/community/posts/{post_id}/vote`
    pub async fn community_vote_post(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &VoteRequest,
    ) -> Result<VoteResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/vote");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 20. `POST /api/v1/community/posts/{post_id}/like`
    pub async fn community_toggle_post_like(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
    ) -> Result<ToggleActionResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/like");
        self.community_request_json::<(), _>(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 21. `POST /api/v1/community/posts/{post_id}/repost`
    pub async fn community_repost(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &RepostRequest,
    ) -> Result<RepostResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/repost");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 22. `POST /api/v1/community/posts/{post_id}/bookmark`
    pub async fn community_bookmark_post(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &BookmarkRequest,
    ) -> Result<BookmarkResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/bookmark");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }
}

impl HttpClient {
    /// 23. `POST /api/v1/community/posts/{post_id}/poll-vote`
    pub async fn community_vote_poll(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &CommunityPollVoteRequest,
    ) -> Result<PollDto, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/poll-vote");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 24. `POST /api/v1/community/posts/{post_id}/award`
    pub async fn community_give_post_award(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &GiveAwardRequest,
    ) -> Result<AwardResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/award");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 25. `POST /api/v1/community/posts/{post_id}/summon-bot`
    pub async fn community_summon_bot(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &SummonBotRequest,
    ) -> Result<SummonBotResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/summon-bot");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    // -------------------------------------------------------------------------
    // D. Comments & Comment Interactions (Endpoints 26–32)
    // -------------------------------------------------------------------------

    /// 26. `GET /api/v1/community/posts/{post_id}/comments`
    pub async fn community_get_comments(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        params: &CommentsQueryParams,
    ) -> Result<CommentsListResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let query = encode_comments_query(params);
        let path = if query.is_empty() {
            format!("/api/v1/community/posts/{seg}/comments")
        } else {
            format!("/api/v1/community/posts/{seg}/comments?{query}")
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 27. `POST /api/v1/community/posts/{post_id}/comments`
    pub async fn community_create_comment(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        post_id: &str,
        req: &CreateCommentRequest,
    ) -> Result<CreateCommentResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("post_id", post_id)?;
        let path = format!("/api/v1/community/posts/{seg}/comments");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 28. `PATCH /api/v1/community/comments/{comment_id}`
    pub async fn community_update_comment(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        comment_id: &str,
        req: &UpdateCommentRequest,
    ) -> Result<CommunityCommentDto, CommunityApiError> {
        let seg = encode_non_empty_segment("comment_id", comment_id)?;
        let path = format!("/api/v1/community/comments/{seg}");
        self.community_request_json(
            Method::PATCH,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 29. `DELETE /api/v1/community/comments/{comment_id}`
    pub async fn community_delete_comment(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        comment_id: &str,
    ) -> Result<DeleteEntityResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("comment_id", comment_id)?;
        let path = format!("/api/v1/community/comments/{seg}");
        self.community_request_json::<(), _>(
            Method::DELETE,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 30. `POST /api/v1/community/comments/{comment_id}/vote`
    pub async fn community_vote_comment(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        comment_id: &str,
        req: &VoteRequest,
    ) -> Result<VoteResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("comment_id", comment_id)?;
        let path = format!("/api/v1/community/comments/{seg}/vote");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 31. `POST /api/v1/community/comments/{comment_id}/like`
    pub async fn community_toggle_comment_like(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        comment_id: &str,
    ) -> Result<ToggleActionResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("comment_id", comment_id)?;
        let path = format!("/api/v1/community/comments/{seg}/like");
        self.community_request_json::<(), _>(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 32. `POST /api/v1/community/comments/{comment_id}/award`
    pub async fn community_give_comment_award(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        comment_id: &str,
        req: &GiveAwardRequest,
    ) -> Result<AwardResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("comment_id", comment_id)?;
        let path = format!("/api/v1/community/comments/{seg}/award");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }
}

impl HttpClient {
    // -------------------------------------------------------------------------
    // E. Communities (c/<slug>), Wiki & Moderation (Endpoints 33–42)
    // -------------------------------------------------------------------------

    /// 33. `GET /api/v1/community/communities`
    pub async fn community_list_communities(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        category: Option<&str>,
        limit: Option<usize>,
    ) -> Result<ListCommunitiesResponse, CommunityApiError> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(cat) = category.map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(format!("category={}", percent_encode_rfc3986(cat)));
        }
        if let Some(lim) = limit {
            parts.push(format!("limit={}", lim.clamp(1, 200)));
        }
        let path = if parts.is_empty() {
            "/api/v1/community/communities".to_string()
        } else {
            format!("/api/v1/community/communities?{}", parts.join("&"))
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 34. `POST /api/v1/community/communities`
    pub async fn community_create_community(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &CreateCommunityRequest,
    ) -> Result<SubCommunityDto, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/communities",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 35. `GET /api/v1/community/communities/{slug}`
    pub async fn community_get_community(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
    ) -> Result<CommunityDetailResponse, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = format!("/api/v1/community/communities/{seg}");
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 36. `PATCH /api/v1/community/communities/{slug}`
    pub async fn community_update_community(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
        req: &UpdateCommunityRequest,
    ) -> Result<SubCommunityDto, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = format!("/api/v1/community/communities/{seg}");
        self.community_request_json(
            Method::PATCH,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 37. `POST /api/v1/community/communities/{slug}/join`
    pub async fn community_join_community(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
    ) -> Result<JoinCommunityResponse, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = format!("/api/v1/community/communities/{seg}/join");
        self.community_request_json::<(), _>(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 38. `DELETE /api/v1/community/communities/{slug}/join`
    pub async fn community_leave_community(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
    ) -> Result<JoinCommunityResponse, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = format!("/api/v1/community/communities/{seg}/join");
        self.community_request_json::<(), _>(
            Method::DELETE,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 39. `PUT /api/v1/community/communities/{slug}/wiki`
    pub async fn community_update_wiki(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
        req: &UpdateCommunityWikiRequest,
    ) -> Result<CommunityWikiRevisionDto, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = format!("/api/v1/community/communities/{seg}/wiki");
        self.community_request_json(
            Method::PUT,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 40. `GET /api/v1/community/communities/{slug}/mod-queue`
    pub async fn community_get_mod_queue(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
        status: Option<ModQueueStatus>,
        limit: Option<usize>,
    ) -> Result<ModQueueResponse, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let mut parts: Vec<String> = Vec::new();
        if let Some(st) = status {
            parts.push(format!("status={}", st.as_str()));
        }
        if let Some(lim) = limit {
            parts.push(format!("limit={}", lim.clamp(1, 200)));
        }
        let path = if parts.is_empty() {
            format!("/api/v1/community/communities/{seg}/mod-queue")
        } else {
            format!(
                "/api/v1/community/communities/{seg}/mod-queue?{}",
                parts.join("&")
            )
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 41. `POST /api/v1/community/communities/{slug}/mod-actions`
    pub async fn community_execute_mod_action(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
        req: &ModActionRequest,
    ) -> Result<ModLogEntryDto, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = format!("/api/v1/community/communities/{seg}/mod-actions");
        self.community_request_json(
            Method::POST,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 42. `GET /api/v1/community/communities/{slug}/mod-logs`
    pub async fn community_get_mod_logs(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        slug: &str,
        limit: Option<usize>,
    ) -> Result<ModLogsResponse, CommunityApiError> {
        let seg = encode_community_slug(slug)?;
        let path = match limit {
            Some(lim) => format!(
                "/api/v1/community/communities/{seg}/mod-logs?limit={}",
                lim.clamp(1, 200)
            ),
            None => format!("/api/v1/community/communities/{seg}/mod-logs"),
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }
}

impl HttpClient {
    // -------------------------------------------------------------------------
    // F. Lists & Custom Feeds (Endpoints 43–46)
    // -------------------------------------------------------------------------

    /// 43. `GET /api/v1/community/lists`
    pub async fn community_list_lists(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
    ) -> Result<CommunityListsResponse, CommunityApiError> {
        self.community_request_json::<(), _>(
            Method::GET,
            "/api/v1/community/lists",
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 44. `POST /api/v1/community/lists`
    pub async fn community_create_list(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &CreateCommunityListRequest,
    ) -> Result<CommunityListDto, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/lists",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 45. `PATCH /api/v1/community/lists/{list_id}`
    pub async fn community_update_list(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        list_id: &str,
        req: &UpdateCommunityListRequest,
    ) -> Result<CommunityListDto, CommunityApiError> {
        let seg = encode_non_empty_segment("list_id", list_id)?;
        let path = format!("/api/v1/community/lists/{seg}");
        self.community_request_json(
            Method::PATCH,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }

    /// 46. `DELETE /api/v1/community/lists/{list_id}`
    pub async fn community_delete_list(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        list_id: &str,
    ) -> Result<DeleteEntityResponse, CommunityApiError> {
        let seg = encode_non_empty_segment("list_id", list_id)?;
        let path = format!("/api/v1/community/lists/{seg}");
        self.community_request_json::<(), _>(
            Method::DELETE,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    // -------------------------------------------------------------------------
    // G. Trending, Search & Notifications (Endpoints 47–50)
    // -------------------------------------------------------------------------

    /// 47. `GET /api/v1/community/trending`
    pub async fn community_get_trending(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<TrendingResponse, CommunityApiError> {
        let path = match limit {
            Some(lim) => format!("/api/v1/community/trending?limit={}", lim.clamp(1, 50)),
            None => "/api/v1/community/trending".to_string(),
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 48. `GET /api/v1/community/search`
    pub async fn community_search(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &CommunitySearchRequestDto,
    ) -> Result<CommunitySearchResultDto, CommunityApiError> {
        let query = encode_search_query(req);
        let path = format!("/api/v1/community/search?{query}");
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 49. `GET /api/v1/community/notifications`
    pub async fn community_get_notifications(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        kind: Option<CommunityNotificationKind>,
        limit: Option<usize>,
        unread_only: Option<bool>,
    ) -> Result<NotificationsListResponse, CommunityApiError> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(k) = kind {
            parts.push(format!("kind={}", k.as_str()));
        }
        if let Some(lim) = limit {
            parts.push(format!("limit={}", lim.clamp(1, 200)));
        }
        if let Some(unread) = unread_only {
            parts.push(format!("unread_only={unread}"));
        }
        let path = if parts.is_empty() {
            "/api/v1/community/notifications".to_string()
        } else {
            format!("/api/v1/community/notifications?{}", parts.join("&"))
        };
        self.community_request_json::<(), _>(
            Method::GET,
            &path,
            bearer_auth(token),
            act_as_actor_id,
            None,
        )
        .await
    }

    /// 50. `POST /api/v1/community/notifications/read`
    pub async fn community_mark_notifications_read(
        &self,
        token: Option<&str>,
        act_as_actor_id: Option<&str>,
        req: &MarkNotificationsReadRequest,
    ) -> Result<MarkNotificationsReadResponse, CommunityApiError> {
        self.community_request_json(
            Method::POST,
            "/api/v1/community/notifications/read",
            bearer_auth(token),
            act_as_actor_id,
            Some(req),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_telegram_proto::community_contracts::{ActorFilter, ActorKind, FeedSort, TimeWindow};

    #[test]
    fn classify_404_distinguishes_legacy_routes_from_live_domain_entities() {
        // Legacy / unmounted 404 cases -> LegacyRouteNotFound (fallback == true)
        for (ct, body) in [
            (None, ""),
            (Some("text/plain"), "Not Found"),
            (Some("text/html; charset=utf-8"), "<html><h1>404 Not Found</h1></html>"),
            (Some("application/json"), r#"{"detail":"Not Found"}"#),
            (Some("application/json"), r#"{"error":"not_found","message":"Not Found"}"#),
            (Some("application/json"), r#"{"error":"route_not_found","message":"route missing"}"#),
            (Some("application/json"), r#"{"error":"not_found","message":"No handler for /api/v1/community/feed"}"#),
        ] {
            let err = classify_404(ct, body);
            assert!(
                matches!(err, CommunityApiError::LegacyRouteNotFound { .. }),
                "expected LegacyRouteNotFound for body={body:?}, got {err:?}"
            );
            assert!(err.should_fallback_to_local());
        }

        // Live domain 404 cases -> DomainNotFound (fallback == false)
        for body in [
            r#"{"error":"not_found","message":"post 'p1' not found"}"#,
            r#"{"error":"not_found","message":"Actor 'ghost' not found"}"#,
            r#"{"error":"not_found","message":"community 'nope' not found"}"#,
            r#"{"error":"not_found","message":"community 'route66' not found"}"#,
            r#"{"error":"not_found","message":"Actor 'endpoint_bot' not found"}"#,
            r#"{"error":"not_found","message":"draft 'd1' not found"}"#,
            r#"{"error":"post_not_found","message":"missing"}"#,
            r#"{"error":"post_not_found","message":"post 'route_123' not found"}"#,
            r#"{"error":"post_not_found","message":""}"#,
            r#"{"error":"actor_not_found","message":"actor 'endpoint_bot' not found"}"#,
            r#"{"error":"actor_not_found","message":"Not Found"}"#,
            r#"{"error":"actor_not_found"}"#,
            r#"{"error":"draft_not_found"}"#,
        ] {
            let err = classify_404(Some("application/json"), body);
            assert!(
                matches!(err, CommunityApiError::DomainNotFound { .. }),
                "expected DomainNotFound for body={body:?}, got {err:?}"
            );
            assert!(!err.should_fallback_to_local());
        }
    }

    #[test]
    fn api_error_fallback_and_telegram_error_mapping_are_exact() {
        assert!(CommunityApiError::Transport {
            message: "refused".into(),
            is_timeout: false,
            is_connect: true,
        }
        .should_fallback_to_local());

        let proxy = CommunityApiError::Proxy("bad socks5".into());
        assert!(!proxy.should_fallback_to_local());
        assert!(matches!(TelegramError::from(proxy), TelegramError::Network(m) if m.contains("Proxy error:")));

        for (status, check) in [
            (400u16, "validation"),
            (401, "auth"),
            (403, "perm"),
            (409, "validation"),
            (410, "not_found"),
            (422, "validation"),
            (429, "validation"),
            (500, "internal"),
            (503, "internal"),
        ] {
            let err = CommunityApiError::Http {
                status,
                code: "err".into(),
                message: "boom".into(),
            };
            assert!(!err.should_fallback_to_local(), "status {status} must NOT fall back");
            let tg = TelegramError::from(err);
            match (check, tg) {
                ("validation", TelegramError::Validation(_))
                | ("auth", TelegramError::Auth(_))
                | ("perm", TelegramError::PermissionDenied(_))
                | ("not_found", TelegramError::NotFound(_))
                | ("internal", TelegramError::Internal(_)) => {}
                (expected, got) => panic!("status {status}: expected {expected}, got {got:?}"),
            }
        }
    }

    #[test]
    fn path_and_query_encoding_handles_colons_handles_slugs_and_cjk() {
        assert_eq!(encode_non_empty_segment("actor_id", "b:42").unwrap(), "b%3A42");
        assert_eq!(encode_handle_or_id("@rust_bot").unwrap(), "rust_bot");
        assert_eq!(encode_community_slug("c/Rust").unwrap(), "rust");
        assert_eq!(encode_community_slug("R/AI_Lab").unwrap(), "ai_lab");
        assert!(encode_non_empty_segment("post_id", "   ").is_err());
        assert!(matches!(
            encode_non_empty_segment("post_id", "."),
            Err(CommunityApiError::InvalidInput(_))
        ));
        assert!(matches!(
            encode_non_empty_segment("post_id", ".."),
            Err(CommunityApiError::InvalidInput(_))
        ));
        assert!(matches!(
            encode_handle_or_id("@.."),
            Err(CommunityApiError::InvalidInput(_))
        ));
        assert!(matches!(
            encode_handle_or_id("."),
            Err(CommunityApiError::InvalidInput(_))
        ));
        assert!(matches!(
            encode_community_slug("c/.."),
            Err(CommunityApiError::InvalidInput(_))
        ));
        assert!(matches!(
            encode_community_slug("r/."),
            Err(CommunityApiError::InvalidInput(_))
        ));
        assert_eq!(bearer_auth(Some("tok\r\n")), Some(CommunityAuth::Bearer("tok\r\n")));
        assert_eq!(bearer_auth(Some("\r\n")), Some(CommunityAuth::Bearer("\r\n")));
        assert!(validate_header_value(HEADER_COMMUNITY_ACTOR_ID, "b:1\r\nEvil: 1").is_err());

        let feed_q = encode_feed_query(&FeedRequestDto {
            tab: FeedTab::Community("rust".into()),
            sort: FeedSort::Top,
            time_window: TimeWindow::Week,
            actor_filter: ActorFilter::HumanBotDebates,
            flair: Some("深度讨论".into()),
            cursor: Some("offset:25".into()),
            limit: 30,
            ..Default::default()
        });
        assert!(feed_q.contains("community=rust"));
        assert!(feed_q.contains("sort=top"));
        assert!(feed_q.contains("time=week"));
        assert!(!feed_q.contains("time_window="));
        assert!(feed_q.contains("actor_filter=human_bot_debates"));
        assert!(feed_q.contains("cursor=offset%3A25"));

        let search_q = encode_search_query(&CommunitySearchRequestDto {
            query: "#开源 rust".into(),
            tab: CommunitySearchTab::Bots,
            sort: CommunitySearchSort::Latest,
            actor_kind: Some(ActorKind::Bot),
            has_poll: Some(true),
            limit: 15,
            offset: 5,
            ..Default::default()
        });
        assert!(search_q.starts_with("query=%23"));
        assert!(search_q.contains("tab=bots"));
        assert!(search_q.contains("sort=latest"));
        assert!(search_q.contains("actor_kind=bot"));
        assert!(search_q.contains("has_poll=true"));
        assert!(search_q.contains("offset=5"));
    }

    #[test]
    fn validate_and_normalize_actor_id_rejects_unicode_whitespace_obs_text_and_controls() {
        assert_eq!(validate_and_normalize_actor_id("u:1001").unwrap(), "u:1001");
        assert_eq!(validate_and_normalize_actor_id("  b:42  ").unwrap(), "b:42");
        assert_eq!(validate_and_normalize_actor_id("u:0").unwrap(), "u:0");

        for bad in [
            "",
            "   ",
            "\t",
            "\r\n",
            "\x00",
            "\x7f",
            "b:42\r\nX-Injected: 1",
            "b:42 evil",
            "\u{00A0}",
            "\u{3000}",
            " \u{00A0} ",
            "\u{0085}",
            "\u{2028}",
            "\u{2029}",
            "\u{200B}",
            "\u{200C}",
            "\u{200D}",
            "\u{FEFF}",
            "b:42\u{00A0}",
            "b:42\u{200B}",
            "b:42🦀",
        ] {
            assert!(
                validate_and_normalize_actor_id(bad).is_err(),
                "actor_id {bad:?} must be rejected"
            );
            assert!(
                validate_header_value(HEADER_COMMUNITY_ACTOR_ID, bad).is_err()
                    || bad == "b:42 evil",
                "validate_header_value({bad:?}) must reject control/non-ASCII/empty"
            );
        }
    }
}
