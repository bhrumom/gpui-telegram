use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::{extract_api_error, percent_encode_rfc3986};
use gpui_telegram_proto::creations::{
    AppStoreCatalogResponse, CatalogQueryResponse, CatalogRecommendResponse, CoverArtwork,
    CreateCreationRequest, CreateCreationReviewRequest, CreationAuditListResponse, CreationKind,
    CreationListResponse, CreationReviewEntryDto, CreationReviewStatus, CreationReviewsResponse,
    CreationSyncResponse, GameLeaderboardEntryDto, GameLeaderboardResponse, ReviewDecisionRequest,
    SubmitGameScoreRequest, SubmitReviewRequest, SubmitReviewResponse, UpdateCreationRequest,
    UpsertCreationStateRequest, UserCreationDto, UserCreationStateDto, UserCreationStatesResponse,
    CatalogSortMode,
};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Normalized inline-query result item returned by `GET /api/v1/bots/inline-query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotInlineQueryResultDto {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<String>,
    #[serde(default)]
    pub creation_id: String,
    pub kind: CreationKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub author_display_name: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<CoverArtwork>,
    #[serde(default)]
    pub rating_avg: f32,
    #[serde(default)]
    pub rating_count: u32,
    #[serde(default)]
    pub score: f32,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub deep_link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_message_content: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<serde_json::Value>,
}

/// Decoded response envelope for `GET /api/v1/bots/inline-query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BotInlineQueryResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    #[serde(default)]
    pub bot_username: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub items: Vec<BotInlineQueryResultDto>,
}

fn encode_creation_id(creation_id: &str) -> Result<String, HttpClientError> {
    let trimmed = creation_id.trim();
    if trimmed.is_empty() {
        return Err(HttpClientError::Api {
            status: 400,
            message: "creation_id must not be empty".to_string(),
        });
    }
    if trimmed == "." || trimmed == ".." {
        return Err(HttpClientError::Api {
            status: 400,
            message: "creation_id must not be '.' or '..'".to_string(),
        });
    }
    Ok(percent_encode_rfc3986(trimmed))
}

fn extract_creation_api_error(status: StatusCode, body: &str) -> HttpClientError {
    let trimmed = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if status == StatusCode::CONFLICT {
            let err_code = v
                .get("error")
                .and_then(|s| s.as_str())
                .map(str::trim)
                .unwrap_or("");
            let base_msg = v
                .get("message")
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(err_code);
            let mut msg = if !err_code.is_empty()
                && !base_msg.is_empty()
                && !base_msg.contains(err_code)
            {
                format!("{err_code}: {base_msg}")
            } else if !base_msg.is_empty() {
                base_msg.to_string()
            } else {
                "Conflict".to_string()
            };
            if let (Some(exp), Some(cur)) = (
                v.get("expected_revision").and_then(|x| x.as_i64()),
                v.get("current_revision").and_then(|x| x.as_i64()),
            ) {
                let has_exp = msg.contains(&format!("expected revision {exp}"))
                    || msg.contains(&format!("expected_revision={exp}"));
                let has_cur = msg.contains(&format!("current revision {cur}"))
                    || msg.contains(&format!("current_revision={cur}"));
                if !has_exp || !has_cur {
                    msg.push_str(&format!(
                        " (expected_revision={exp}, current_revision={cur})"
                    ));
                }
            }
            return HttpClientError::Conflict(msg);
        }

        if status == StatusCode::UNPROCESSABLE_ENTITY {
            let base_msg = v
                .get("message")
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    v.get("error")
                        .and_then(|s| s.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or("Validation failed");
            let errors: Vec<String> = v
                .get("validation_errors")
                .and_then(|arr| arr.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            item.as_str()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let message = if errors.is_empty() {
                base_msg.to_string()
            } else {
                format!("{base_msg}: {}", errors.join("; "))
            };
            return HttpClientError::Api {
                status: status.as_u16(),
                message,
            };
        }
    }
    extract_api_error(status, body)
}

fn parse_bot_inline_query_response(
    body: &str,
    fallback_bot_username: &str,
    fallback_query: &str,
) -> Result<BotInlineQueryResponse, HttpClientError> {
    let raw: serde_json::Value = serde_json::from_str(body)?;
    let (query_id, bot_username, query, reported_total, raw_items) = match raw {
        serde_json::Value::Array(arr) => (
            None,
            fallback_bot_username.to_string(),
            fallback_query.to_string(),
            None,
            arr,
        ),
        serde_json::Value::Object(map) => {
            let qid = map
                .get("query_id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let bot = map
                .get("bot_username")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_bot_username)
                .to_string();
            let q = map
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or(fallback_query)
                .to_string();
            let total = map
                .get("total")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let arr = ["results", "items", "entries"]
                .iter()
                .find_map(|key| {
                    map.get(*key)
                        .and_then(|v| v.as_array())
                        .filter(|a| !a.is_empty())
                        .cloned()
                })
                .or_else(|| {
                    ["results", "items", "entries"]
                        .iter()
                        .find_map(|key| map.get(*key).and_then(|v| v.as_array()).cloned())
                })
                .unwrap_or_default();
            (qid, bot, q, total, arr)
        }
        other => {
            return match serde_json::from_value::<BotInlineQueryResponse>(other) {
                Ok(parsed) => Ok(parsed),
                Err(err) => Err(HttpClientError::Serialization(err)),
            };
        }
    };

    let mut items = Vec::with_capacity(raw_items.len());
    for entry in raw_items {
        let mut dto: BotInlineQueryResultDto = serde_json::from_value(entry)?;
        if dto.id.is_empty() && !dto.creation_id.is_empty() {
            dto.id = dto.creation_id.clone();
        } else if dto.creation_id.is_empty() && !dto.id.is_empty() {
            dto.creation_id = dto.id.clone();
        }
        if dto.description.is_empty() && !dto.summary.is_empty() {
            dto.description = dto.summary.clone();
        } else if dto.summary.is_empty() && !dto.description.is_empty() {
            dto.summary = dto.description.clone();
        }
        items.push(dto);
    }

    let total = reported_total.unwrap_or(items.len());
    Ok(BotInlineQueryResponse {
        query_id,
        bot_username,
        query,
        total,
        items,
    })
}

fn append_catalog_query_params(
    params: &mut Vec<String>,
    q: Option<&str>,
    genre: Option<&str>,
    sort: Option<CatalogSortMode>,
    limit: Option<usize>,
    offset: Option<usize>,
) {
    if let Some(q_val) = q.map(str::trim).filter(|s| !s.is_empty()) {
        params.push(format!("q={}", percent_encode_rfc3986(q_val)));
    }
    if let Some(g_val) = genre.map(str::trim).filter(|s| !s.is_empty()) {
        params.push(format!("genre={}", percent_encode_rfc3986(g_val)));
    }
    if let Some(sort_mode) = sort {
        params.push(format!("sort={}", sort_mode.as_str()));
    }
    if let Some(lim) = limit {
        params.push(format!("limit={lim}"));
    }
    if let Some(off) = offset {
        params.push(format!("offset={off}"));
    }
}

fn build_path_with_query(base: &str, params: &[String]) -> String {
    if params.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", params.join("&"))
    }
}

impl HttpClient {
    async fn request_creation_json<B: Serialize + ?Sized, R: DeserializeOwned + Send + 'static>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&B>,
    ) -> Result<R, HttpClientError> {
        let client = self.client.clone();
        let url = self.endpoint_url(path);
        let bearer = token.map(|t| t.to_string());
        let json_body = match body {
            Some(b) => Some(serde_json::to_value(b)?),
            None => None,
        };
        self.run_on_tokio(async move {
            let mut req = client.request(method, &url);
            if let Some(ref t) = bearer {
                req = req.bearer_auth(t);
            }
            if let Some(ref val) = json_body {
                req = req.json(val);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                return Err(extract_creation_api_error(status, &text));
            }
            Ok(serde_json::from_str(&text)?)
        })
        .await
    }

    pub async fn create_creation(
        &self,
        token: &str,
        req: &CreateCreationRequest,
    ) -> Result<UserCreationDto, HttpClientError> {
        self.request_creation_json(Method::POST, "/api/v1/creations", Some(token), Some(req))
            .await
    }

    pub async fn list_my_creations(
        &self,
        token: &str,
        kind: Option<CreationKind>,
        status: Option<CreationReviewStatus>,
        q: Option<&str>,
    ) -> Result<CreationListResponse, HttpClientError> {
        let mut params = Vec::new();
        if let Some(k) = kind {
            params.push(format!("kind={}", k.as_str()));
        }
        if let Some(s) = status {
            params.push(format!("status={}", s.as_str()));
        }
        if let Some(q_val) = q.map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("q={}", percent_encode_rfc3986(q_val)));
        }
        let path = build_path_with_query("/api/v1/creations/my", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn sync_creations(
        &self,
        token: &str,
        since_revision: Option<i64>,
    ) -> Result<CreationSyncResponse, HttpClientError> {
        let mut params = Vec::new();
        if let Some(rev) = since_revision {
            params.push(format!("since_revision={rev}"));
        }
        let path = build_path_with_query("/api/v1/creations/sync", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn get_creation(
        &self,
        token: &str,
        creation_id: &str,
    ) -> Result<UserCreationDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}");
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn update_creation(
        &self,
        token: &str,
        creation_id: &str,
        req: &UpdateCreationRequest,
    ) -> Result<UserCreationDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}");
        self.request_creation_json(Method::PUT, &path, Some(token), Some(req))
            .await
    }

    pub async fn delete_creation(
        &self,
        token: &str,
        creation_id: &str,
    ) -> Result<UserCreationDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}");
        self.request_creation_json(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn submit_creation_for_review(
        &self,
        token: &str,
        creation_id: &str,
        req: Option<&SubmitReviewRequest>,
    ) -> Result<SubmitReviewResponse, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}/submit-review");
        let body = req.cloned().unwrap_or_default();
        self.request_creation_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn withdraw_creation_review(
        &self,
        token: &str,
        creation_id: &str,
    ) -> Result<UserCreationDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}/withdraw-review");
        let empty = serde_json::json!({});
        self.request_creation_json(Method::POST, &path, Some(token), Some(&empty))
            .await
    }

    pub async fn unpublish_creation(
        &self,
        token: &str,
        creation_id: &str,
    ) -> Result<UserCreationDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}/unpublish");
        let empty = serde_json::json!({});
        self.request_creation_json(Method::POST, &path, Some(token), Some(&empty))
            .await
    }

    pub async fn get_creation_audit(
        &self,
        token: &str,
        creation_id: &str,
    ) -> Result<CreationAuditListResponse, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}/audit");
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_review_queue(
        &self,
        token: &str,
        status: Option<CreationReviewStatus>,
        kind: Option<CreationKind>,
    ) -> Result<CreationListResponse, HttpClientError> {
        let mut params = Vec::new();
        if let Some(s) = status {
            params.push(format!("status={}", s.as_str()));
        }
        if let Some(k) = kind {
            params.push(format!("kind={}", k.as_str()));
        }
        let path = build_path_with_query("/api/v1/creations/review-queue", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn submit_review_decision(
        &self,
        token: &str,
        creation_id: &str,
        req: &ReviewDecisionRequest,
    ) -> Result<UserCreationDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/creations/{id}/review");
        self.request_creation_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn search_public_catalog(
        &self,
        token: &str,
        kind: Option<CreationKind>,
        q: Option<&str>,
        genre: Option<&str>,
        sort: Option<CatalogSortMode>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<CatalogQueryResponse, HttpClientError> {
        let mut params = Vec::new();
        if let Some(k) = kind {
            params.push(format!("kind={}", k.as_str()));
        }
        append_catalog_query_params(&mut params, q, genre, sort, limit, offset);
        let path = build_path_with_query("/api/v1/catalog", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_catalog_books(
        &self,
        token: &str,
        q: Option<&str>,
        genre: Option<&str>,
        sort: Option<CatalogSortMode>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<CatalogQueryResponse, HttpClientError> {
        let mut params = Vec::new();
        append_catalog_query_params(&mut params, q, genre, sort, limit, offset);
        let path = build_path_with_query("/api/v1/catalog/books", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_catalog_music(
        &self,
        token: &str,
        q: Option<&str>,
        genre: Option<&str>,
        sort: Option<CatalogSortMode>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<CatalogQueryResponse, HttpClientError> {
        let mut params = Vec::new();
        append_catalog_query_params(&mut params, q, genre, sort, limit, offset);
        let path = build_path_with_query("/api/v1/catalog/music", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_catalog_games(
        &self,
        token: &str,
        q: Option<&str>,
        genre: Option<&str>,
        sort: Option<CatalogSortMode>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<CatalogQueryResponse, HttpClientError> {
        let mut params = Vec::new();
        append_catalog_query_params(&mut params, q, genre, sort, limit, offset);
        let path = build_path_with_query("/api/v1/catalog/games", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn list_catalog_appstore(
        &self,
        token: &str,
        q: Option<&str>,
        genre: Option<&str>,
        sort: Option<CatalogSortMode>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<AppStoreCatalogResponse, HttpClientError> {
        let mut params = Vec::new();
        append_catalog_query_params(&mut params, q, genre, sort, limit, offset);
        let path = build_path_with_query("/api/v1/catalog/appstore", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn recommend_public_catalog(
        &self,
        token: &str,
        q: Option<&str>,
        kind: Option<CreationKind>,
        genre: Option<&str>,
        limit: Option<usize>,
    ) -> Result<CatalogRecommendResponse, HttpClientError> {
        let mut params = Vec::new();
        if let Some(q_val) = q.map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("q={}", percent_encode_rfc3986(q_val)));
        }
        if let Some(k) = kind {
            params.push(format!("kind={}", k.as_str()));
        }
        if let Some(g_val) = genre.map(str::trim).filter(|s| !s.is_empty()) {
            params.push(format!("genre={}", percent_encode_rfc3986(g_val)));
        }
        if let Some(lim) = limit {
            params.push(format!("limit={lim}"));
        }
        let path = build_path_with_query("/api/v1/catalog/recommend", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn upsert_creation_state(
        &self,
        token: &str,
        creation_id: &str,
        req: &UpsertCreationStateRequest,
    ) -> Result<UserCreationStateDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/catalog/{id}/state");
        self.request_creation_json(Method::PUT, &path, Some(token), Some(req))
            .await
    }

    pub async fn list_my_creation_states(
        &self,
        token: &str,
        kind: Option<CreationKind>,
    ) -> Result<UserCreationStatesResponse, HttpClientError> {
        let mut params = Vec::new();
        if let Some(k) = kind {
            params.push(format!("kind={}", k.as_str()));
        }
        let path = build_path_with_query("/api/v1/catalog/my-states", &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn submit_creation_review(
        &self,
        token: &str,
        creation_id: &str,
        req: &CreateCreationReviewRequest,
    ) -> Result<CreationReviewEntryDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/catalog/{id}/reviews");
        self.request_creation_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn list_creation_reviews(
        &self,
        token: &str,
        creation_id: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<CreationReviewsResponse, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let mut params = Vec::new();
        if let Some(lim) = limit {
            params.push(format!("limit={lim}"));
        }
        if let Some(off) = offset {
            params.push(format!("offset={off}"));
        }
        let path = build_path_with_query(&format!("/api/v1/catalog/{id}/reviews"), &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn submit_game_score(
        &self,
        token: &str,
        creation_id: &str,
        req: &SubmitGameScoreRequest,
    ) -> Result<GameLeaderboardEntryDto, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let path = format!("/api/v1/catalog/{id}/scores");
        self.request_creation_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn get_game_leaderboard(
        &self,
        token: &str,
        creation_id: &str,
        limit: Option<usize>,
    ) -> Result<GameLeaderboardResponse, HttpClientError> {
        let id = encode_creation_id(creation_id)?;
        let mut params = Vec::new();
        if let Some(lim) = limit {
            params.push(format!("limit={lim}"));
        }
        let path = build_path_with_query(&format!("/api/v1/catalog/{id}/leaderboard"), &params);
        self.request_creation_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn query_bot_inline(
        &self,
        token: &str,
        bot_username: &str,
        query: &str,
        kind: Option<CreationKind>,
        limit: Option<usize>,
    ) -> Result<BotInlineQueryResponse, HttpClientError> {
        let normalized_bot = bot_username.trim().trim_start_matches('@').to_string();
        let mut params = Vec::new();
        if !normalized_bot.is_empty() {
            params.push(format!(
                "bot_username={}",
                percent_encode_rfc3986(&normalized_bot)
            ));
        }
        params.push(format!("query={}", percent_encode_rfc3986(query)));
        if let Some(k) = kind {
            params.push(format!("kind={}", k.as_str()));
        }
        if let Some(lim) = limit {
            params.push(format!("limit={lim}"));
        }
        let path = build_path_with_query("/api/v1/bots/inline-query", &params);
        let client = self.client.clone();
        let url = self.endpoint_url(&path);
        let bearer = token.to_string();
        let query_owned = query.to_string();
        self.run_on_tokio(async move {
            let resp = client.request(Method::GET, &url).bearer_auth(&bearer).send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                return Err(extract_creation_api_error(status, &text));
            }
            parse_bot_inline_query_response(&text, &normalized_bot, &query_owned)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_error_preserves_409_revision_conflict_and_422_validation_errors() {
        let err409 = extract_creation_api_error(
            StatusCode::CONFLICT,
            r#"{"error":"revision_conflict","message":"expected revision 2 does not match current revision 4","expected_revision":2,"current_revision":4}"#,
        );
        match err409 {
            HttpClientError::Conflict(msg) => {
                assert!(msg.contains("revision_conflict"), "msg={msg}");
                assert!(msg.contains('2') && msg.contains('4'), "msg={msg}");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        // Empty message on 409 must still preserve error code and revision numbers
        let err409_empty = extract_creation_api_error(
            StatusCode::CONFLICT,
            r#"{"error":"revision_conflict","message":"","expected_revision":12,"current_revision":2}"#,
        );
        match err409_empty {
            HttpClientError::Conflict(msg) => {
                assert!(msg.contains("revision_conflict"), "msg={msg}");
                assert!(msg.contains("expected_revision=12"), "msg={msg}");
                assert!(msg.contains("current_revision=2"), "msg={msg}");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        let err422 = extract_creation_api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":"validation_failed","message":"Creation failed review validation","validation_errors":["title must be non-empty","book requires at least 50 chars"]}"#,
        );
        match err422 {
            HttpClientError::Api { status, message } => {
                assert_eq!(status, 422);
                assert!(message.contains("title must be non-empty"), "msg={message}");
                assert!(
                    message.contains("book requires at least 50 chars"),
                    "msg={message}"
                );
            }
            other => panic!("expected Api 422, got {other:?}"),
        }

        // Empty message on 422 falls back to error code
        let err422_empty = extract_creation_api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":"validation_failed","message":"   ","validation_errors":["bad cover",""]}"#,
        );
        match err422_empty {
            HttpClientError::Api { status, message } => {
                assert_eq!(status, 422);
                assert_eq!(message, "validation_failed: bad cover");
            }
            other => panic!("expected Api 422, got {other:?}"),
        }
    }

    #[test]
    fn inline_query_parser_handles_m3_live_envelope_bare_array_results_items_entries_and_dual_keys() {
        let bare = r#"[{"creation_id":"cr_1","kind":"book","title":"Star Atlas","summary":"Space guide","deep_link":"tg://apple/books/cr_1"}]"#;
        let parsed_bare = parse_bot_inline_query_response(bare, "CreatorStudioBot", "star").unwrap();
        assert_eq!(parsed_bare.query_id, None);
        assert_eq!(parsed_bare.total, 1);
        assert_eq!(parsed_bare.items.len(), 1);
        assert_eq!(parsed_bare.items[0].id, "cr_1");
        assert_eq!(parsed_bare.items[0].creation_id, "cr_1");
        assert_eq!(parsed_bare.items[0].description, "Space guide");
        assert_eq!(parsed_bare.items[0].summary, "Space guide");

        // Exact M3 live envelope: query_id + bot_username + total + dual results/items + omitted query
        let m3_live = r#"{
            "query_id": "iq_0123456789abcdef",
            "bot_username": "CreatorStudioBot",
            "total": 1,
            "results": [{
                "id": "cr_m3_1",
                "type": "article",
                "creation_id": "cr_m3_1",
                "kind": "music",
                "title": "Night Jazz",
                "subtitle": "Live Trio",
                "description": "Smooth trio",
                "summary": "Smooth trio",
                "author_display_name": "Miles",
                "genres": ["jazz"],
                "rating_avg": 4.8,
                "rating_count": 5,
                "score": 9.2,
                "reason": "Top rated",
                "deep_link": "tg://apple/music/cr_m3_1",
                "input_message_content": {"message_text": "Night Jazz"},
                "reply_markup": {"inline_keyboard": [[{"text": "Open", "url": "tg://apple/music/cr_m3_1"}]]}
            }],
            "items": [{
                "id": "cr_m3_1",
                "type": "article",
                "creation_id": "cr_m3_1",
                "kind": "music",
                "title": "Night Jazz",
                "description": "Smooth trio",
                "summary": "Smooth trio",
                "deep_link": "tg://apple/music/cr_m3_1"
            }]
        }"#;
        let parsed_m3 = parse_bot_inline_query_response(m3_live, "fallback", "jazz").unwrap();
        assert_eq!(parsed_m3.query_id.as_deref(), Some("iq_0123456789abcdef"));
        assert_eq!(parsed_m3.bot_username, "CreatorStudioBot");
        assert_eq!(parsed_m3.query, "jazz");
        assert_eq!(parsed_m3.total, 1);
        assert_eq!(parsed_m3.items.len(), 1);
        assert_eq!(parsed_m3.items[0].result_type.as_deref(), Some("article"));
        assert_eq!(parsed_m3.items[0].creation_id, "cr_m3_1");
        assert!(parsed_m3.items[0].input_message_content.is_some());
        assert!(parsed_m3.items[0].reply_markup.is_some());

        let entries_shape = r#"{"entries":[{"id":"cr_ent","kind":"game","title":"Maze","description":"Run","deep_link":"tg://apple/games/cr_ent"}]}"#;
        let parsed_entries = parse_bot_inline_query_response(entries_shape, "Bot", "maze").unwrap();
        assert_eq!(parsed_entries.total, 1);
        assert_eq!(parsed_entries.items[0].creation_id, "cr_ent");

        // Invalid non-object/non-array JSON returns Serialization error without panic
        assert!(matches!(
            parse_bot_inline_query_response("null", "Bot", ""),
            Err(HttpClientError::Serialization(_))
        ));
    }

    #[test]
    fn encode_creation_id_rejects_empty_and_dot_segments_and_percent_encodes() {
        assert!(matches!(
            encode_creation_id("   "),
            Err(HttpClientError::Api { status: 400, .. })
        ));
        assert!(matches!(
            encode_creation_id("."),
            Err(HttpClientError::Api { status: 400, .. })
        ));
        assert!(matches!(
            encode_creation_id(".."),
            Err(HttpClientError::Api { status: 400, .. })
        ));
        assert_eq!(
            encode_creation_id("cr/abc 123?x=1").unwrap(),
            "cr%2Fabc%20123%3Fx%3D1"
        );
    }
}
