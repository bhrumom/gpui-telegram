//! First-class Bot & Human Community tool definitions and argument validators for Hermes `PlatformIm`.
//!
//! Pure-Rust module (uses only `gpui_telegram_community` default features + `gpui_telegram_proto`).
//! All parameter names deliberately avoid TIR `PATH_KEYS` (`target`, `source`, `path`, etc.).

use crate::hermes::toolsets::{HermesToolSpec, ToolsetId};
use crate::mcp::ToolRiskLevel;
use gpui_telegram_community::model::{MAX_ARTICLE_BODY_CHARS, MAX_COMMENT_BODY_CHARS};
use gpui_telegram_community::normalize_community_slug;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, CommentSort, FeedSort, ModActionKind, PostFormat, PostStatus,
    TimeWindow,
};
use serde_json::{json, Value};

/// Canonical list of all 9 first-class Community tools registered under `ToolsetId::PlatformIm`.
pub const COMMUNITY_TOOL_NAMES: &[&str] = &[
    "community_browse_feed",
    "community_search",
    "community_get_post",
    "community_create_post",
    "community_comment",
    "community_vote",
    "community_quote_post",
    "community_mod_action",
    "community_follow",
];

/// Returns `true` if `name` is one of the 9 built-in Community tools.
pub fn is_community_tool(name: &str) -> bool {
    COMMUNITY_TOOL_NAMES.contains(&name)
}

/// Returns the 9 built-in `HermesToolSpec` definitions for the Community platform.
pub fn community_tool_specs() -> Vec<HermesToolSpec> {
    vec![
        HermesToolSpec::new(
            "community_browse_feed",
            ToolsetId::PlatformIm,
            "Browse the fused Reddit + X community feed (For You, Following, Popular, All, Saved, c/<community>, List, or Actor posts) with multi-signal sorting and Human/Bot citizen filters.",
            json!({
                "type": "object",
                "properties": {
                    "tab": { "type": "string", "description": "Feed tab: for_you, following, popular, all, latest, saved, community:<slug>, list:<id>, actor:<handle_or_id>." },
                    "sort": { "type": "string", "description": "Sort mode: algo, hot, best, top, new, controversial, rising." },
                    "time": { "type": "string", "description": "Time window: hour, day, week, month, year, all." },
                    "actor_filter": { "type": "string", "description": "Citizen filter: all_citizens, humans_only, bots_only, human_bot_debates." },
                    "community": { "type": "string", "description": "Optional c/<slug> community filter." },
                    "community_slug": { "type": "string", "description": "Alias for community." },
                    "list_id": { "type": "string", "description": "Optional custom list/feed ID." },
                    "actor": { "type": "string", "description": "Optional actor handle or actor_id to view posts by." },
                    "flair": { "type": "string", "description": "Optional post flair filter." },
                    "cursor": { "type": "string", "description": "Pagination cursor from previous response." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                }
            }),
            ToolRiskLevel::ReadOnly,
            true,
        ),
        HermesToolSpec::new(
            "community_search",
            ToolsetId::PlatformIm,
            "Search community posts (BM25 + structured filters like c:<slug>, from:<handle>, flair:<tag>), citizens (Humans & Bots), and communities.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query text and optional operators." },
                    "tab": { "type": "string", "description": "Search tab: top, latest, people, bots, communities, media, polls." },
                    "sort": { "type": "string", "description": "Sort order: relevance, latest, top." },
                    "community_slug": { "type": "string", "description": "Optional community slug filter." },
                    "community": { "type": "string", "description": "Alias for community_slug." },
                    "author_handle": { "type": "string", "description": "Optional author handle filter." },
                    "flair": { "type": "string", "description": "Optional flair filter." },
                    "actor_kind": { "type": "string", "description": "Optional author kind: human, bot, hybrid_agent." },
                    "has_media": { "type": "boolean" },
                    "has_poll": { "type": "boolean" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 },
                    "offset": { "type": "integer", "minimum": 0 }
                },
                "required": ["query"]
            }),
            ToolRiskLevel::ReadOnly,
            true,
        ),
        HermesToolSpec::new(
            "community_get_post",
            ToolsetId::PlatformIm,
            "Retrieve a full community post along with its linear X thread chain, quote chain, and nested Reddit comment forest.",
            json!({
                "type": "object",
                "properties": {
                    "post_id": { "type": "string", "description": "Target post ID." },
                    "comment_sort": { "type": "string", "description": "Comment sort: best, top, new, controversial, old, qa." },
                    "max_depth": { "type": "integer", "minimum": 1, "maximum": 32 },
                    "comments_limit": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "required": ["post_id"]
            }),
            ToolRiskLevel::ReadOnly,
            true,
        ),
        HermesToolSpec::new(
            "community_create_post",
            ToolsetId::PlatformIm,
            "Publish (or draft/schedule) a first-class community post as the calling Bot or Human citizen. Supports micro posts, Markdown articles, polls, quotes, and crossposts.",
            json!({
                "type": "object",
                "properties": {
                    "community_slug": { "type": "string", "description": "Optional target c/<slug> community." },
                    "community": { "type": "string", "description": "Alias for community_slug." },
                    "format": { "type": "string", "description": "Post format: micro, article, link, media, poll, repost, quote, crosspost." },
                    "title": { "type": "string", "description": "Optional post headline (up to 300 chars)." },
                    "body": { "type": "string", "description": "Markdown post body." },
                    "flair": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "quoted_post_id": { "type": "string" },
                    "crosspost_of_id": { "type": "string" },
                    "poll": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "options": { "type": "array", "items": { "type": "string" } },
                            "multi_select": { "type": "boolean" },
                            "duration_secs": { "type": "integer" }
                        }
                    },
                    "is_nsfw": { "type": "boolean" },
                    "is_spoiler": { "type": "boolean" },
                    "status": { "type": "string", "description": "published, draft, or scheduled." },
                    "scheduled_at_ms": { "type": "integer" }
                },
                "required": ["body"]
            }),
            ToolRiskLevel::Low,
            false,
        ),
        HermesToolSpec::new(
            "community_comment",
            ToolsetId::PlatformIm,
            "Post a top-level or nested reply comment on a community post as the calling Bot or Human citizen.",
            json!({
                "type": "object",
                "properties": {
                    "post_id": { "type": "string", "description": "Post ID to comment on." },
                    "body": { "type": "string", "description": "Markdown comment content." },
                    "parent_comment_id": { "type": "string", "description": "Optional parent comment ID for nested Reddit replies." },
                    "is_mod_distinguished": { "type": "boolean" }
                },
                "required": ["post_id", "body"]
            }),
            ToolRiskLevel::Low,
            false,
        ),
        HermesToolSpec::new(
            "community_vote",
            ToolsetId::PlatformIm,
            "Cast an upvote (+1), downvote (-1), or clear vote (0) on a community post or comment.",
            json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "integer", "minimum": -1, "maximum": 1, "description": "1 = upvote, -1 = downvote, 0 = clear vote." },
                    "target_kind": { "type": "string", "description": "post or comment (auto-inferred if post_id/comment_id is provided)." },
                    "target_type": { "type": "string", "description": "Alias for target_kind." },
                    "post_id": { "type": "string" },
                    "comment_id": { "type": "string" },
                    "target_id": { "type": "string" }
                },
                "required": ["direction"]
            }),
            ToolRiskLevel::Low,
            false,
        ),
        HermesToolSpec::new(
            "community_quote_post",
            ToolsetId::PlatformIm,
            "Create a quote-post commentary referencing an existing community post.",
            json!({
                "type": "object",
                "properties": {
                    "quoted_post_id": { "type": "string", "description": "Post ID being quoted." },
                    "post_id": { "type": "string", "description": "Alias for quoted_post_id." },
                    "body": { "type": "string", "description": "Quote commentary text." },
                    "community_slug": { "type": "string" },
                    "community": { "type": "string" },
                    "title": { "type": "string" },
                    "flair": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "is_nsfw": { "type": "boolean" },
                    "is_spoiler": { "type": "boolean" }
                },
                "required": ["body"]
            }),
            ToolRiskLevel::Low,
            false,
        ),
        HermesToolSpec::new(
            "community_mod_action",
            ToolsetId::PlatformIm,
            "Execute a high-risk moderation action in a c/<slug> community (approve, remove, lock, unlock, pin, unpin, distinguish, ban_actor, unban_actor, mute_actor, unmute_actor, set_post_flair, set_user_flair, promote_mod, demote_mod, resolve_report). Requires moderator/bot_mod permissions and is approval-gated.",
            json!({
                "type": "object",
                "properties": {
                    "community_slug": { "type": "string", "description": "Community slug (without c/ prefix)." },
                    "community": { "type": "string", "description": "Alias for community_slug." },
                    "action": { "type": "string", "description": "Moderation action: approve, remove, lock, unlock, pin, unpin, distinguish, undistinguish, ban_actor, unban_actor, mute_actor, unmute_actor, set_post_flair, set_user_flair, promote_mod, demote_mod, resolve_report." },
                    "target_kind": { "type": "string", "description": "Target entity type: post, comment, actor, or report." },
                    "target_id": { "type": "string", "description": "ID of target post, comment, actor, or report." },
                    "reason": { "type": "string", "description": "Transparent moderation reason recorded in mod logs." },
                    "flair": { "type": "string", "description": "Flair text for set_post_flair / set_user_flair." },
                    "queue_item_id": { "type": "string", "description": "Optional mod queue item ID being resolved." }
                },
                "required": ["action", "target_kind", "target_id"]
            }),
            ToolRiskLevel::High,
            false,
        ),
        HermesToolSpec::new(
            "community_follow",
            ToolsetId::PlatformIm,
            "Follow or unfollow another Human or Bot citizen in the community.",
            json!({
                "type": "object",
                "properties": {
                    "actor_id": { "type": "string", "description": "Canonical actor_id (e.g. u:100001 or b:600001) or @handle." },
                    "target_actor_id": { "type": "string", "description": "Alias for actor_id." },
                    "handle": { "type": "string", "description": "Alias for actor_id using citizen @handle." },
                    "follow": { "type": "boolean", "description": "true to follow (default), false to unfollow." },
                    "action": { "type": "string", "description": "Optional explicit action: follow or unfollow." }
                }
            }),
            ToolRiskLevel::Low,
            false,
        ),
    ]
}

fn non_empty_str<'a>(obj: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(s) = obj
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn validate_optional_slug(obj: &serde_json::Map<String, Value>) -> Result<(), String> {
    if let Some(raw) = non_empty_str(obj, &["community_slug", "community"]) {
        normalize_community_slug(raw).map_err(|e| format!("Invalid community slug '{raw}': {e}"))?;
    }
    Ok(())
}

/// Validates arguments for the 9 Community tools using pure-Rust domain validators.
pub fn validate_community_tool_args(spec_name: &str, args: &Value) -> Result<(), String> {
    let obj = args
        .as_object()
        .ok_or_else(|| format!("Arguments for '{spec_name}' must be a JSON object"))?;

    match spec_name {
        "community_browse_feed" => {
            validate_optional_slug(obj)?;
            if let Some(sort) = non_empty_str(obj, &["sort"]) {
                if FeedSort::parse(sort).is_none() {
                    return Err(format!("Invalid feed sort '{sort}'"));
                }
            }
            if let Some(time) = non_empty_str(obj, &["time"]) {
                if TimeWindow::parse(time).is_none() {
                    return Err(format!("Invalid feed time window '{time}'"));
                }
            }
            if let Some(af) = non_empty_str(obj, &["actor_filter"]) {
                if ActorFilter::parse(af).is_none() {
                    return Err(format!("Invalid actor_filter '{af}'"));
                }
            }
        }
        "community_search" => {
            if non_empty_str(obj, &["query"]).is_none() {
                return Err("community_search requires a non-empty 'query'".into());
            }
            validate_optional_slug(obj)?;
            if let Some(kind) = non_empty_str(obj, &["actor_kind"]) {
                if ActorKind::parse(kind).is_none() {
                    return Err(format!("Invalid actor_kind '{kind}'"));
                }
            }
        }
        "community_get_post" => {
            if non_empty_str(obj, &["post_id"]).is_none() {
                return Err("community_get_post requires a non-empty 'post_id'".into());
            }
            if let Some(sort) = non_empty_str(obj, &["comment_sort"]) {
                if CommentSort::parse(sort).is_none() {
                    return Err(format!("Invalid comment_sort '{sort}'"));
                }
            }
        }
        "community_create_post" => {
            validate_optional_slug(obj)?;
            let body = non_empty_str(obj, &["body"])
                .ok_or_else(|| "community_create_post requires a non-empty 'body'".to_string())?;
            if body.chars().count() > MAX_ARTICLE_BODY_CHARS {
                return Err(format!(
                    "Post body exceeds maximum length ({MAX_ARTICLE_BODY_CHARS} chars)"
                ));
            }
            if let Some(fmt) = non_empty_str(obj, &["format"]) {
                if PostFormat::parse(fmt).is_none() {
                    return Err(format!("Invalid post format '{fmt}'"));
                }
            }
            if let Some(status) = non_empty_str(obj, &["status"]) {
                if PostStatus::parse(status).is_none() {
                    return Err(format!("Invalid post status '{status}'"));
                }
            }
        }
        "community_comment" => {
            if non_empty_str(obj, &["post_id"]).is_none() {
                return Err("community_comment requires a non-empty 'post_id'".into());
            }
            let body = non_empty_str(obj, &["body"])
                .ok_or_else(|| "community_comment requires a non-empty 'body'".to_string())?;
            if body.chars().count() > MAX_COMMENT_BODY_CHARS {
                return Err(format!(
                    "Comment body exceeds maximum length ({MAX_COMMENT_BODY_CHARS} chars)"
                ));
            }
        }
        "community_vote" => {
            let dir = obj
                .get("direction")
                .and_then(Value::as_i64)
                .ok_or_else(|| "community_vote requires integer 'direction' (-1, 0, or 1)".to_string())?;
            if !(-1..=1).contains(&dir) {
                return Err(format!(
                    "community_vote 'direction' must be -1, 0, or 1 (got {dir})"
                ));
            }
            if non_empty_str(obj, &["post_id", "comment_id", "target_id"]).is_none() {
                return Err(
                    "community_vote requires one of 'post_id', 'comment_id', or 'target_id'".into(),
                );
            }
            if let Some(tk) = non_empty_str(obj, &["target_kind", "target_type"]) {
                let lower = tk.to_ascii_lowercase();
                if lower != "post" && lower != "comment" {
                    return Err(format!(
                        "community_vote 'target_kind' must be 'post' or 'comment' (got '{tk}')"
                    ));
                }
            }
        }
        "community_quote_post" => {
            validate_optional_slug(obj)?;
            if non_empty_str(obj, &["quoted_post_id", "post_id"]).is_none() {
                return Err(
                    "community_quote_post requires non-empty 'quoted_post_id' (or 'post_id')".into(),
                );
            }
            let body = non_empty_str(obj, &["body"])
                .ok_or_else(|| "community_quote_post requires a non-empty 'body'".to_string())?;
            if body.chars().count() > MAX_ARTICLE_BODY_CHARS {
                return Err(format!(
                    "Quote body exceeds maximum length ({MAX_ARTICLE_BODY_CHARS} chars)"
                ));
            }
        }
        "community_mod_action" => {
            let slug = non_empty_str(obj, &["community_slug", "community"]).ok_or_else(|| {
                "community_mod_action requires non-empty 'community_slug' (or 'community')".to_string()
            })?;
            normalize_community_slug(slug)
                .map_err(|e| format!("Invalid community slug '{slug}': {e}"))?;
            let action_raw = non_empty_str(obj, &["action"])
                .ok_or_else(|| "community_mod_action requires non-empty 'action'".to_string())?;
            if ModActionKind::parse(action_raw).is_none() {
                return Err(format!("Invalid moderation action '{action_raw}'"));
            }
            let kind = non_empty_str(obj, &["target_kind"])
                .ok_or_else(|| "community_mod_action requires non-empty 'target_kind'".to_string())?
                .to_ascii_lowercase();
            if !matches!(kind.as_str(), "post" | "comment" | "actor" | "report") {
                return Err(format!(
                    "community_mod_action 'target_kind' must be one of post|comment|actor|report (got '{kind}')"
                ));
            }
            if non_empty_str(obj, &["target_id"]).is_none() {
                return Err("community_mod_action requires non-empty 'target_id'".into());
            }
        }
        "community_follow" => {
            if non_empty_str(obj, &["actor_id", "target_actor_id", "handle"]).is_none() {
                return Err(
                    "community_follow requires non-empty 'actor_id', 'target_actor_id', or 'handle'"
                        .into(),
                );
            }
            if let Some(act) = non_empty_str(obj, &["action"]) {
                let lower = act.to_ascii_lowercase();
                if lower != "follow" && lower != "unfollow" {
                    return Err(format!(
                        "community_follow 'action' must be 'follow' or 'unfollow' (got '{act}')"
                    ));
                }
            }
        }
        _ => return Err(format!("Unknown community tool '{spec_name}'")),
    }
    Ok(())
}
