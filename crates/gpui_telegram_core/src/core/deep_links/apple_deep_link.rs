//! Canonical Apple Hub Deep-Link parser, formatter, and router registration.
//!
//! Supports:
//! - Canonical URI: `tg://apple/<books|music|games|appstore|creations>[/<creation_id>][?tab=...&action=...]`
//! - Callback alias: `creation:open:<book|music|game|app>:<creation_id>`

use super::deep_links_router::DeepLinksRouterDeepLinks;
use super::deep_links_types::{DeepLinkAction, DeepLinkContext, DeepLinkEntry};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

pub const MAX_APPLE_DEEP_LINK_LEN: usize = 2048;
pub const MAX_APPLE_CREATION_ID_LEN: usize = 128;
pub const MAX_APPLE_QUERY_SLUG_LEN: usize = 64;

/// Top-level Apple-style content hub sections routable via `tg://apple/<slug>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppleDeepLinkSection {
    Books,
    Music,
    Games,
    #[serde(rename = "appstore", alias = "app_store")]
    AppStore,
    Creations,
}

impl AppleDeepLinkSection {
    pub const ALL: [Self; 5] = [
        Self::Books,
        Self::Music,
        Self::Games,
        Self::AppStore,
        Self::Creations,
    ];

    pub fn as_slug(self) -> &'static str {
        match self {
            Self::Books => "books",
            Self::Music => "music",
            Self::Games => "games",
            Self::AppStore => "appstore",
            Self::Creations => "creations",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        if slug.eq_ignore_ascii_case("books") {
            Some(Self::Books)
        } else if slug.eq_ignore_ascii_case("music") {
            Some(Self::Music)
        } else if slug.eq_ignore_ascii_case("games") {
            Some(Self::Games)
        } else if slug.eq_ignore_ascii_case("appstore") {
            Some(Self::AppStore)
        } else if slug.eq_ignore_ascii_case("creations") {
            Some(Self::Creations)
        } else {
            None
        }
    }

    pub fn from_creation_kind(kind: &str) -> Option<Self> {
        if kind.eq_ignore_ascii_case("book") {
            Some(Self::Books)
        } else if kind.eq_ignore_ascii_case("music") {
            Some(Self::Music)
        } else if kind.eq_ignore_ascii_case("game") {
            Some(Self::Games)
        } else if kind.eq_ignore_ascii_case("app") {
            Some(Self::AppStore)
        } else {
            None
        }
    }

    pub fn creation_kind_slug(self) -> Option<&'static str> {
        match self {
            Self::Books => Some("book"),
            Self::Music => Some("music"),
            Self::Games => Some("game"),
            Self::AppStore => Some("app"),
            Self::Creations => None,
        }
    }
}

impl fmt::Display for AppleDeepLinkSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_slug())
    }
}

/// Canonical parsed representation of an Apple Hub deep link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppleDeepLink {
    pub section: AppleDeepLinkSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl AppleDeepLink {
    pub fn new(section: AppleDeepLinkSection) -> Self {
        Self {
            section,
            creation_id: None,
            tab: None,
            action: None,
        }
    }

    pub fn for_creation(section: AppleDeepLinkSection, creation_id: impl Into<String>) -> Self {
        Self {
            section,
            creation_id: Some(creation_id.into()),
            tab: None,
            action: None,
        }
    }

    pub fn with_tab(mut self, tab: impl Into<String>) -> Self {
        self.tab = Some(tab.into());
        self
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn to_uri(&self) -> String {
        format_apple_deep_link(self)
    }

    pub fn to_callback_alias(&self) -> Option<String> {
        let kind_slug = self.section.creation_kind_slug()?;
        let id = self.creation_id.as_deref()?;
        if !is_valid_creation_id(id) {
            return None;
        }
        Some(format!("creation:open:{kind_slug}:{id}"))
    }
}

impl fmt::Display for AppleDeepLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_apple_deep_link(self))
    }
}

/// Registrar for Apple Hub deep-link routes on `DeepLinksRouterDeepLinks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DeepLinksAppleDeepLinks;

impl DeepLinksAppleDeepLinks {
    pub fn register(router: &mut DeepLinksRouterDeepLinks) {
        for section in AppleDeepLinkSection::ALL {
            let slug = section.as_slug();
            router.add(
                "apple",
                DeepLinkEntry {
                    path: slug.to_string(),
                    action: DeepLinkAction::HandlerFunction(format!("open_apple_{slug}")),
                    requires_auth: true,
                    skip_activation: false,
                },
            );
        }
    }
}

/// Parses either a canonical `tg://apple/<section>[/<id>][?tab=...&action=...]` URI
/// or a callback alias `creation:open:<book|music|game|app>:<id>`.
pub fn parse_apple_deep_link(uri: &str) -> Option<AppleDeepLink> {
    if uri.is_empty() || uri.len() > MAX_APPLE_DEEP_LINK_LEN || has_forbidden_control_or_bidi(uri) {
        return None;
    }

    let trimmed = uri.trim_matches(' ');
    if trimmed.is_empty()
        || trimmed.chars().any(char::is_whitespace)
        || trimmed.contains('#')
        || has_forbidden_scheme_or_xss(trimmed)
    {
        return None;
    }

    if let Some(rest) = strip_ascii_prefix_ignore_case(trimmed, "creation:open:") {
        return parse_creation_callback_alias(rest);
    }

    if let Some(rest) = strip_ascii_prefix_ignore_case(trimmed, "tg://") {
        return parse_tg_apple_uri(rest);
    }

    None
}

/// Formats an `AppleDeepLink` into its canonical `tg://apple/...` URI string.
pub fn format_apple_deep_link(link: &AppleDeepLink) -> String {
    let mut out = format!("tg://apple/{}", link.section.as_slug());
    if let Some(id) = link.creation_id.as_deref() {
        out.push('/');
        out.push_str(&percent_encode_rfc3986(id));
    }

    let mut has_query = false;
    if let Some(tab) = link.tab.as_deref() {
        out.push('?');
        out.push_str("tab=");
        out.push_str(&percent_encode_rfc3986(&tab.to_ascii_lowercase()));
        has_query = true;
    }
    if let Some(action) = link.action.as_deref() {
        out.push(if has_query { '&' } else { '?' });
        out.push_str("action=");
        out.push_str(&percent_encode_rfc3986(&action.to_ascii_lowercase()));
    }
    out
}

pub(crate) fn is_apple_deep_link_candidate(uri: &str) -> bool {
    let trimmed = uri.trim();
    if strip_ascii_prefix_ignore_case(trimmed, "creation:open:").is_some() {
        return true;
    }
    let mut normalized = trimmed;
    while let Some(after_scheme) = strip_ascii_prefix_ignore_case(normalized, "tg://") {
        normalized = after_scheme;
    }
    normalized = normalized.trim_start_matches('/');
    if let Some(rest) = strip_ascii_prefix_ignore_case(normalized, "apple") {
        return match rest.chars().next() {
            None => true,
            Some(ch) => !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-',
        };
    }
    false
}

pub(crate) fn to_deep_link_context(link: &AppleDeepLink, has_session: bool) -> DeepLinkContext {
    let path = match link.creation_id.as_deref() {
        Some(id) => format!("{}/{}", link.section.as_slug(), id),
        None => link.section.as_slug().to_string(),
    };
    let mut params = HashMap::new();
    if let Some(tab) = link.tab.as_deref() {
        params.insert("tab".to_string(), tab.to_string());
    }
    if let Some(action) = link.action.as_deref() {
        params.insert("action".to_string(), action.to_string());
    }
    DeepLinkContext {
        section: "apple".to_string(),
        path,
        params,
        has_active_session: has_session,
    }
}

pub(crate) fn validate_apple_context(ctx: &DeepLinkContext) -> Option<AppleDeepLink> {
    if !ctx.section.eq_ignore_ascii_case("apple") {
        return None;
    }
    if ctx.path.is_empty()
        || ctx.path.len() > MAX_APPLE_DEEP_LINK_LEN
        || ctx.path.starts_with('/')
        || ctx.path.ends_with('/')
        || ctx.path.contains("//")
        || ctx.path.contains('\\')
        || has_forbidden_control_or_bidi(&ctx.path)
        || has_forbidden_scheme_or_xss(&ctx.path)
    {
        return None;
    }

    let parts: Vec<&str> = ctx.path.split('/').collect();
    if parts.len() != 1 && parts.len() != 2 {
        return None;
    }
    let section = AppleDeepLinkSection::from_slug(parts[0])?;
    let creation_id = if parts.len() == 2 {
        let id = parts[1];
        if !is_valid_creation_id(id) {
            return None;
        }
        Some(id.to_string())
    } else {
        None
    };

    let mut tab = None;
    let mut action = None;
    let mut total_len: usize = ctx.path.len();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for (k, v) in &ctx.params {
        total_len = total_len
            .saturating_add(k.len())
            .saturating_add(v.len())
            .saturating_add(2);
        if total_len > MAX_APPLE_DEEP_LINK_LEN {
            return None;
        }
        if k.is_empty()
            || v.is_empty()
            || has_forbidden_control_or_bidi(k)
            || has_forbidden_control_or_bidi(v)
            || has_forbidden_scheme_or_xss(k)
            || has_forbidden_scheme_or_xss(v)
        {
            return None;
        }
        let kl = k.to_ascii_lowercase();
        if !seen_keys.insert(kl.clone()) {
            return None;
        }
        match kl.as_str() {
            "tab" => {
                if !is_valid_query_slug(v) {
                    return None;
                }
                tab = Some(v.to_ascii_lowercase());
            }
            "action" => {
                if !is_valid_query_slug(v) {
                    return None;
                }
                action = Some(v.to_ascii_lowercase());
            }
            _ => {
                if !is_valid_unknown_query_token(k) || !is_valid_unknown_query_token(v) {
                    return None;
                }
            }
        }
    }

    Some(AppleDeepLink {
        section,
        creation_id,
        tab,
        action,
    })
}

fn parse_creation_callback_alias(rest: &str) -> Option<AppleDeepLink> {
    if rest.contains('/') || rest.contains('\\') || rest.contains('?') {
        return None;
    }
    let mut parts = rest.split(':');
    let raw_kind = parts.next()?;
    let raw_id = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let section = AppleDeepLinkSection::from_creation_kind(raw_kind)?;
    let id = strict_percent_decode(raw_id)?;
    if has_forbidden_control_or_bidi(&id)
        || has_forbidden_scheme_or_xss(&id)
        || !is_valid_creation_id(&id)
    {
        return None;
    }
    Some(AppleDeepLink {
        section,
        creation_id: Some(id),
        tab: None,
        action: Some("open".to_string()),
    })
}

fn parse_tg_apple_uri(rest: &str) -> Option<AppleDeepLink> {
    let (path_part, query_opt) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };

    if path_part.is_empty()
        || path_part.starts_with('/')
        || path_part.ends_with('/')
        || path_part.contains("//")
        || path_part.contains('\\')
    {
        return None;
    }

    let parts: Vec<&str> = path_part.split('/').collect();
    if parts.len() != 2 && parts.len() != 3 {
        return None;
    }
    if !parts[0].eq_ignore_ascii_case("apple") {
        return None;
    }

    let section = AppleDeepLinkSection::from_slug(parts[1])?;
    let creation_id = if parts.len() == 3 {
        let decoded = strict_percent_decode(parts[2])?;
        if has_forbidden_control_or_bidi(&decoded)
            || has_forbidden_scheme_or_xss(&decoded)
            || !is_valid_creation_id(&decoded)
        {
            return None;
        }
        Some(decoded)
    } else {
        None
    };

    let mut tab: Option<String> = None;
    let mut action: Option<String> = None;

    if let Some(query) = query_opt {
        if query.is_empty() {
            return None;
        }
        let mut seen_keys: HashSet<String> = HashSet::new();
        for pair in query.split('&') {
            if pair.is_empty() {
                return None;
            }
            let (raw_k, raw_v) = pair.split_once('=')?;
            let k = strict_percent_decode(raw_k)?;
            let v = strict_percent_decode(raw_v)?;

            if k.is_empty()
                || v.is_empty()
                || has_forbidden_control_or_bidi(&k)
                || has_forbidden_control_or_bidi(&v)
                || has_forbidden_scheme_or_xss(&k)
                || has_forbidden_scheme_or_xss(&v)
                || k.chars().any(char::is_whitespace)
                || v.chars().any(char::is_whitespace)
            {
                return None;
            }

            let kl = k.to_ascii_lowercase();
            if !seen_keys.insert(kl.clone()) {
                return None;
            }

            match kl.as_str() {
                "tab" => {
                    if !is_valid_query_slug(&v) {
                        return None;
                    }
                    tab = Some(v.to_ascii_lowercase());
                }
                "action" => {
                    if !is_valid_query_slug(&v) {
                        return None;
                    }
                    action = Some(v.to_ascii_lowercase());
                }
                _ => {
                    if !is_valid_unknown_query_token(&k) || !is_valid_unknown_query_token(&v) {
                        return None;
                    }
                }
            }
        }
    }

    Some(AppleDeepLink {
        section,
        creation_id,
        tab,
        action,
    })
}

fn strip_ascii_prefix_ignore_case<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    if input.len() < prefix.len() || !input.is_char_boundary(prefix.len()) {
        return None;
    }
    let head = &input[..prefix.len()];
    if head.eq_ignore_ascii_case(prefix) {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

fn has_forbidden_control_or_bidi(s: &str) -> bool {
    s.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
    })
}

fn has_forbidden_scheme_or_xss(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("<script")
        || lower.contains("javascript:")
        || lower.contains("vbscript:")
        || lower.contains("data:")
}

fn is_valid_creation_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > MAX_APPLE_CREATION_ID_LEN
        || id.starts_with('.')
        || id.ends_with('.')
        || id.contains("..")
    {
        return false;
    }
    id.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn is_valid_query_slug(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_APPLE_QUERY_SLUG_LEN {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn is_valid_unknown_query_token(s: &str) -> bool {
    if s.is_empty()
        || s.len() > MAX_APPLE_QUERY_SLUG_LEN
        || s.starts_with('.')
        || s.ends_with('.')
        || s.contains("..")
    {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn strict_percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_nibble(bytes[i + 1])?;
            let lo = hex_nibble(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn percent_encode_rfc3986(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    out
}
