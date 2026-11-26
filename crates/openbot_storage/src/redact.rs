//! Secret & credential redaction engine for OpenBot storage, prompts, logs, and history tools.
//! Exact pure-Rust 1:1 port of `packages/logging/src/index.ts:L14-L480`.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::RwLock;
use serde_json::{Map, Value};

const MIN_REGISTERED_SECRET_LENGTH: usize = 8;
const MAX_EMBEDDED_SCANS: usize = 16;
const MAX_CONVERSION_DEPTH: usize = 32;
const UNSCANNED: &str = "[redacted-unscanned]";
const VALUE_MARKER: &str = "__openbot_registered_value__";
const LABEL_MARKER: &str = "__openbot_registered_secret_authorization";

const HOST_FLAG: u8 = 1;
const HOST_SEPARATOR_FLAG: u8 = 2;

const SECRET_CORES: [&str; 17] = [
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "credential",
    "authorization",
    "cookie",
    "apikey",
    "api_key",
    "api-key",
    "privatekey",
    "private_key",
    "private-key",
    "signingkey",
    "signing_key",
    "signing-key",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnscannedTextPolicy {
    Drop,
    KeepPastScanBound,
    Keep,
}

#[derive(Default)]
struct SecretRegistryInner {
    secrets_set: HashSet<String>,
    /// Sorted descending by byte length so longest registered forms match first.
    secrets_sorted: Vec<String>,
    public_values: HashSet<String>,
}

/// Dynamic runtime registry for Vault, OAuth, and provider API secrets (`packages/logging/src/index.ts:L52-L87`).
#[derive(Default)]
pub struct SecretRegistry {
    inner: RwLock<SecretRegistryInner>,
}

static GLOBAL_SECRET_REGISTRY: OnceLock<SecretRegistry> = OnceLock::new();

impl SecretRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn global() -> &'static Self {
        GLOBAL_SECRET_REGISTRY.get_or_init(Self::new)
    }

    pub fn clear(&self) {
        let mut guard = self.inner.write();
        guard.secrets_set.clear();
        guard.secrets_sorted.clear();
        guard.public_values.clear();
    }

    pub fn register_public_values<I, S>(&self, values: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut guard = self.inner.write();
        for v in values {
            let s = v.as_ref();
            if !s.is_empty() {
                guard.public_values.insert(s.to_string());
            }
        }
    }

    pub fn register_secret_value(&self, value: &str, allow_short: bool) {
        self.register_secret_values([value], allow_short);
    }

    pub fn register_secret_values<I, S>(&self, values: I, allow_short: bool)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut guard = self.inner.write();
        let prev_len = guard.secrets_set.len();
        for item in values {
            let v = item.as_ref();
            if v.is_empty() || (!allow_short && v.encode_utf16().count() < MIN_REGISTERED_SECRET_LENGTH) {
                continue;
            }
            guard.secrets_set.insert(v.to_string());
            if let Ok(json_quoted) = serde_json::to_string(v)
                && json_quoted.len() >= 2
            {
                guard
                    .secrets_set
                    .insert(json_quoted[1..json_quoted.len() - 1].to_string());
            }
            guard.secrets_set.insert(uri_encode_component(v));
        }
        if guard.secrets_set.len() != prev_len {
            let mut sorted: Vec<String> = guard.secrets_set.iter().cloned().collect();
            sorted.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
            guard.secrets_sorted = sorted;
        }
    }

    pub fn redact_text(&self, value: &str) -> String {
        self.redact_with_options(value, true, UnscannedTextPolicy::Drop)
    }

    pub fn redact_secrets(&self, value: &str) -> String {
        self.redact_with_options(value, false, UnscannedTextPolicy::KeepPastScanBound)
    }

    pub fn contains_secret(&self, value: &str) -> bool {
        self.redact_with_options(value, true, UnscannedTextPolicy::Keep) != value
    }

    pub fn redact_post_text(&self, value: &str) -> String {
        let formatted = apply_key_format_rules(value);
        self.mask_whole_registered_secrets(&formatted)
    }

    pub fn redact_json_value(&self, value: &Value, emails: bool) -> Value {
        self.convert_value(value, emails, UnscannedTextPolicy::Keep, 0)
    }

    pub fn redact_with_options(
        &self,
        value: &str,
        emails: bool,
        unscanned: UnscannedTextPolicy,
    ) -> String {
        let marked = self.mark_registered_secrets(value);
        let redacted = self
            .redact_serialized_json(&marked, emails)
            .unwrap_or_else(|| {
                let embedded = self.redact_embedded_json(&marked, emails, unscanned);
                apply_text_rules(&embedded, emails)
            });
        if marked == value {
            redacted
        } else {
            redacted
                .replace(VALUE_MARKER, "[redacted]")
                .replace(LABEL_MARKER, "[redacted]")
        }
    }
}

pub fn register_public_values<I: IntoIterator<Item = S>, S: AsRef<str>>(values: I) {
    SecretRegistry::global().register_public_values(values);
}

pub fn register_secret_value(value: &str, allow_short: bool) {
    SecretRegistry::global().register_secret_value(value, allow_short);
}

pub fn register_secret_values<I: IntoIterator<Item = S>, S: AsRef<str>>(values: I, allow_short: bool) {
    SecretRegistry::global().register_secret_values(values, allow_short);
}

pub fn redact_text(value: &str) -> String {
    SecretRegistry::global().redact_text(value)
}

pub fn redact_secrets(value: &str) -> String {
    SecretRegistry::global().redact_secrets(value)
}

pub fn contains_secret(value: &str) -> bool {
    SecretRegistry::global().contains_secret(value)
}

pub fn redact_post_text(value: &str) -> String {
    SecretRegistry::global().redact_post_text(value)
}

fn uri_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if matches!(
            b,
            b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'_'
                | b'.'
                | b'!'
                | b'~'
                | b'*'
                | b'\''
                | b'('
                | b')'
        ) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0').to_ascii_uppercase());
            out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0').to_ascii_uppercase());
        }
    }
    out
}

#[inline]
fn is_token_char_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'+' | b'/' | b'@' | b'%' | b'-')
}

#[inline]
fn is_post_token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '@' | '%' | '-')
}

#[inline]
fn is_post_token_tail_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '@' | '-')
}

fn ends_with_percent_escape(value: &str, end: usize) -> bool {
    if end < 3 {
        return false;
    }
    let bytes = value.as_bytes();
    bytes[end - 3] == b'%'
        && bytes[end - 2].is_ascii_hexdigit()
        && bytes[end - 1].is_ascii_hexdigit()
}

fn is_whole_post_token(value: &str, start: usize, end: usize) -> bool {
    if value[end..].starts_with('@') && start >= 3 && &value[start - 3..start] == "://" {
        return true;
    }
    if start > 0
        && let Some(prev) = value[..start].chars().next_back()
        && is_post_token_char(prev)
        && !ends_with_percent_escape(value, start)
    {
        return false;
    }
    let tail: String = value[end..]
        .chars()
        .take_while(|c| is_post_token_tail_char(*c))
        .collect();
    tail.chars().all(|c| c == '.')
}

fn url_hosts(value: &str) -> Vec<u8> {
    let mut flags = vec![0u8; value.len()];
    let bytes = value.as_bytes();
    let mut i = 0;
    while i + 7 <= bytes.len() {
        let scheme_len = if value[i..].len() >= 8 && value[i..i + 8].eq_ignore_ascii_case("https://") {
            8
        } else if value[i..].len() >= 7 && value[i..i + 7].eq_ignore_ascii_case("http://") {
            7
        } else {
            i += 1;
            continue;
        };
        if i > 0
            && (bytes[i - 1].is_ascii_alphanumeric()
                || matches!(bytes[i - 1], b'+' | b'.' | b'-'))
        {
            i += 1;
            continue;
        }
        let after_scheme = i + scheme_len;
        let auth_end = value[after_scheme..]
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#'))
            .map_or(value.len(), |off| after_scheme + off);
        let host_start = value[after_scheme..auth_end]
            .rfind('@')
            .map_or(after_scheme, |at| after_scheme + at + 1);
        let mut host_end = host_start;
        for (off, ch) in value[host_start..auth_end].char_indices() {
            if ch.is_alphanumeric() || matches!(ch, '.' | '-') {
                host_end = host_start + off + ch.len_utf8();
            } else {
                break;
            }
        }
        if host_end > host_start {
            flags[host_start..host_end].fill(HOST_FLAG);
            if host_start > after_scheme && bytes[host_start - 1] == b'@' {
                flags[host_start - 1] = HOST_SEPARATOR_FLAG;
            }
            i = host_end;
        } else {
            i = after_scheme;
        }
    }
    flags
}

pub fn is_secret_label(candidate: &str) -> bool {
    if candidate.is_empty()
        || !candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return false;
    }
    let lower = candidate.to_ascii_lowercase();
    for core in SECRET_CORES {
        let mut search_from = 0;
        while let Some(rel) = lower[search_from..].find(core) {
            let pos = search_from + rel;
            let suffix_len = lower.len() - (pos + core.len());
            if pos <= 64 && suffix_len <= 64 {
                return true;
            }
            search_from = pos + 1;
        }
    }
    false
}

pub fn is_secret_name(name: &str) -> bool {
    is_secret_label(name) || name.eq_ignore_ascii_case("key") || name.eq_ignore_ascii_case("keys")
}

fn has_rule_word(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    SECRET_CORES.iter().any(|w| lower.contains(w))
        || ["bearer", "basic", "digest", "headers", "keys", "key"]
            .iter()
            .any(|w| lower.contains(w))
}

impl SecretRegistry {
    fn is_public_token(&self, public_values: &HashSet<String>, token: &str, secret: &str) -> bool {
        if public_values.is_empty() {
            return false;
        }
        let is_around = |cand: &str| {
            cand.len() > secret.len() && cand.contains(secret) && public_values.contains(cand)
        };
        if is_around(token) {
            return true;
        }
        token.ends_with('.') && is_around(token.trim_end_matches('.'))
    }

    fn find_next_secret<'a>(
        secrets_sorted: &'a [String],
        haystack: &str,
        from: usize,
    ) -> Option<(usize, &'a str)> {
        let mut best: Option<(usize, &'a str)> = None;
        for secret in secrets_sorted {
            if let Some(rel) = haystack[from..].find(secret.as_str()) {
                let idx = from + rel;
                match best {
                    None => best = Some((idx, secret.as_str())),
                    Some((b_idx, b_sec))
                        if idx < b_idx || (idx == b_idx && secret.len() > b_sec.len()) =>
                    {
                        best = Some((idx, secret.as_str()));
                    }
                    _ => {}
                }
            }
        }
        best
    }

    fn mark_registered_secrets(&self, value: &str) -> String {
        let guard = self.inner.read();
        if guard.secrets_sorted.is_empty() {
            return value.to_string();
        }
        let bytes = value.as_bytes();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut hosts: Option<Vec<u8>> = None;
        let mut cursor = 0;
        while cursor < value.len() {
            let Some((mut start, matched)) =
                Self::find_next_secret(&guard.secrets_sorted, value, cursor)
            else {
                break;
            };
            let mut end = start + matched.len();
            cursor = end;
            if hosts.is_none() && value.contains("://") {
                hosts = Some(url_hosts(value));
            }
            if let Some(h) = &hosts
                && h[start..end].iter().all(|f| *f == HOST_FLAG)
            {
                continue;
            }
            let floor = ranges.last().map_or(0, |r| r.1);
            while start > floor
                && hosts.as_ref().is_none_or(|h| h[start - 1] == 0)
                && is_token_char_byte(bytes[start - 1])
            {
                start -= 1;
            }
            if ranges.last().is_some_and(|r| end <= r.1) {
                continue;
            }
            while end < bytes.len()
                && hosts.as_ref().is_none_or(|h| h[end] == 0)
                && is_token_char_byte(bytes[end])
            {
                end += 1;
            }
            if !value.is_char_boundary(start) || !value.is_char_boundary(end) {
                continue;
            }
            if self.is_public_token(&guard.public_values, &value[start..end], matched) {
                continue;
            }
            if let Some(prev) = ranges.last_mut()
                && start <= prev.1
            {
                prev.1 = end;
            } else {
                ranges.push((start, end));
            }
        }
        if ranges.is_empty() {
            return value.to_string();
        }
        let mut out = String::with_capacity(value.len());
        let mut copied = 0;
        for (start, end) in ranges {
            out.push_str(&value[copied..start]);
            out.push_str(if has_rule_word(&value[start..end]) {
                LABEL_MARKER
            } else {
                VALUE_MARKER
            });
            copied = end;
        }
        out.push_str(&value[copied..]);
        out
    }

    fn mask_whole_registered_secrets(&self, value: &str) -> String {
        let guard = self.inner.read();
        if guard.secrets_sorted.is_empty() {
            return value.to_string();
        }
        let mut out = String::with_capacity(value.len());
        let mut cursor = 0;
        while cursor < value.len() {
            let Some((start, matched)) =
                Self::find_next_secret(&guard.secrets_sorted, value, cursor)
            else {
                out.push_str(&value[cursor..]);
                break;
            };
            let end = start + matched.len();
            out.push_str(&value[cursor..start]);
            if is_whole_post_token(value, start, end) {
                out.push_str("[redacted]");
            } else {
                out.push_str(matched);
            }
            cursor = end;
        }
        out
    }
}

fn redact_key_formats_before_labels(value: &str) -> String {
    let s = redact_pem_private_keys(value);
    let s = redact_cookie_headers(&s);
    let s = redact_auth_headers(&s);
    redact_bearer_tokens(&s)
}

fn redact_pem_private_keys(value: &str) -> String {
    const BEGIN_PREFIX: &str = "-----BEGIN ";
    const PRIV_SUFFIX: &str = "PRIVATE KEY-----";
    const END_PREFIX: &str = "-----END ";
    if !value.contains(BEGIN_PREFIX) {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(rel) = value[cursor..].find(BEGIN_PREFIX) {
        let start = cursor + rel;
        let label_start = start + BEGIN_PREFIX.len();
        let mut matched_end: Option<usize> = None;
        let max_label_end = (label_start + 40).min(value.len());
        if let Some(priv_rel) = value[label_start..].find(PRIV_SUFFIX) {
            let priv_pos = label_start + priv_rel;
            if priv_pos <= max_label_end
                && value[label_start..priv_pos]
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ')
            {
                let body_start = priv_pos + PRIV_SUFFIX.len();
                let search_limit = value[body_start..]
                    .char_indices()
                    .nth(16_385)
                    .map_or(value.len(), |(off, _)| body_start + off);
                let mut end_search = body_start;
                while let Some(end_rel) = value[end_search..search_limit].find(END_PREFIX) {
                    let end_tag = end_search + end_rel;
                    let end_lbl_start = end_tag + END_PREFIX.len();
                    if let Some(end_priv_rel) = value[end_lbl_start..].find(PRIV_SUFFIX) {
                        let end_priv_pos = end_lbl_start + end_priv_rel;
                        if end_priv_pos - end_lbl_start <= 40
                            && value[end_lbl_start..end_priv_pos].bytes().all(|b| {
                                b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' '
                            })
                        {
                            matched_end = Some(end_priv_pos + PRIV_SUFFIX.len());
                            break;
                        }
                    }
                    end_search = end_tag + END_PREFIX.len();
                }
                if matched_end.is_none() {
                    let mut p = body_start;
                    let bytes = value.as_bytes();
                    while p < bytes.len() {
                        if bytes[p].is_ascii_whitespace() {
                            p += 1;
                        } else if p + 1 < bytes.len()
                            && bytes[p] == b'\\'
                            && matches!(bytes[p + 1], b'r' | b'n')
                        {
                            p += 2;
                        } else {
                            break;
                        }
                    }
                    let b64_len = bytes[p..]
                        .iter()
                        .take_while(|b| {
                            b.is_ascii_alphanumeric() || matches!(**b, b'+' | b'/' | b'=')
                        })
                        .count();
                    if b64_len >= 16 {
                        matched_end = Some(value.len());
                    }
                }
            }
        }
        if let Some(end) = matched_end {
            out.push_str(&value[cursor..start]);
            out.push_str("[redacted]");
            cursor = end;
        } else {
            let step = start + BEGIN_PREFIX.len();
            out.push_str(&value[cursor..step]);
            cursor = step;
        }
    }
    out.push_str(&value[cursor..]);
    out
}

fn is_ascii_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn redact_cookie_headers(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if !lower.contains("cookie") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("cookie") {
        let start = cursor + rel;
        if start > 0 && is_ascii_word_byte(bytes[start - 1]) {
            out.push_str(&value[cursor..start + 6]);
            cursor = start + 6;
            continue;
        }
        let mut p = start + 6;
        if p < bytes.len() && matches!(bytes[p], b'"' | b'\'') {
            p += 1;
        }
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b':' {
            out.push_str(&value[cursor..start + 6]);
            cursor = start + 6;
            continue;
        }
        p += 1;
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p < bytes.len() && matches!(bytes[p], b'"' | b'\'') {
            p += 1;
        }
        let prefix_end = p;
        let name_len = value[p..]
            .chars()
            .take_while(|c| {
                !c.is_whitespace() && !matches!(c, '=' | ';' | ',' | '"' | '\'' | '\\')
            })
            .count();
        let name_bytes: usize = value[p..]
            .chars()
            .take(name_len)
            .map(char::len_utf8)
            .sum();
        if !(1..=256).contains(&name_len)
            || p + name_bytes >= bytes.len()
            || bytes[p + name_bytes] != b'='
        {
            out.push_str(&value[cursor..start + 6]);
            cursor = start + 6;
            continue;
        }
        let mut v = p + name_bytes + 1;
        while v < bytes.len() {
            let b = bytes[v];
            if b == b'\\' && v + 1 < bytes.len() {
                let next_ch = value[v + 1..].chars().next().map_or(1, char::len_utf8);
                v += 1 + next_ch;
            } else if matches!(b, b'\r' | b'\n' | b'\'') {
                break;
            } else if b == b'"' {
                if let Some(close_rel) = value[v + 1..].find('"') {
                    let inner = &value[v + 1..v + 1 + close_rel];
                    let after_q = v + 1 + close_rel + 1;
                    let valid_quoted = inner.chars().count() <= 4096
                        && !inner.contains(['\r', '\n', '\\'])
                        && (after_q >= bytes.len()
                            || bytes[after_q] == b';'
                            || {
                                let ws = bytes[after_q..]
                                    .iter()
                                    .take_while(|c| matches!(**c, b' ' | b'\t'))
                                    .count();
                                after_q + ws >= bytes.len()
                                    || matches!(bytes[after_q + ws], b'\r' | b'\n')
                            });
                    if valid_quoted {
                        v = after_q;
                        continue;
                    }
                }
                break;
            } else {
                let step = value[v..].chars().next().map_or(1, char::len_utf8);
                v += step;
            }
        }
        out.push_str(&value[cursor..prefix_end]);
        out.push_str("[redacted]");
        cursor = v;
    }
    out.push_str(&value[cursor..]);
    out
}

fn is_bearer_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

fn redact_auth_headers(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if !lower.contains("authorization") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("authorization") {
        let start = cursor + rel;
        let mut p = start + "authorization".len();
        if p < bytes.len() && matches!(bytes[p], b'"' | b'\'') {
            p += 1;
        }
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        let sep_start = p;
        while p < bytes.len() && matches!(bytes[p], b'=' | b':') {
            p += 1;
        }
        if p == sep_start {
            out.push_str(&value[cursor..start + 13]);
            cursor = start + 13;
            continue;
        }
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        let prefix_end = p;
        if p >= bytes.len() || !bytes[p].is_ascii_alphabetic() {
            out.push_str(&value[cursor..start + 13]);
            cursor = start + 13;
            continue;
        }
        let scheme_tail = bytes[p + 1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
            .count();
        if scheme_tail > 31 {
            out.push_str(&value[cursor..start + 13]);
            cursor = start + 13;
            continue;
        }
        p += 1 + scheme_tail;
        let ws = bytes[p..].iter().take_while(|b| b.is_ascii_whitespace()).count();
        if ws == 0 {
            out.push_str(&value[cursor..start + 13]);
            cursor = start + 13;
            continue;
        }
        p += ws;
        let tok_len = bytes[p..].iter().take_while(|b| is_bearer_token_byte(**b)).count();
        if tok_len < 8 {
            out.push_str(&value[cursor..start + 13]);
            cursor = start + 13;
            continue;
        }
        out.push_str(&value[cursor..prefix_end]);
        out.push_str("[redacted]");
        cursor = p + tok_len;
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_bearer_tokens(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if !lower.contains("bearer") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("bearer") {
        let start = cursor + rel;
        let mut p = start + 6;
        let ws = bytes[p..].iter().take_while(|b| b.is_ascii_whitespace()).count();
        if ws == 0 {
            out.push_str(&value[cursor..p]);
            cursor = p;
            continue;
        }
        p += ws;
        let tok_len = bytes[p..].iter().take_while(|b| is_bearer_token_byte(**b)).count();
        if tok_len < 8 {
            out.push_str(&value[cursor..start + 6]);
            cursor = start + 6;
            continue;
        }
        out.push_str(&value[cursor..start]);
        out.push_str("[redacted]");
        cursor = p + tok_len;
    }
    out.push_str(&value[cursor..]);
    out
}

fn parse_assigned_value(value: &str, start: usize) -> Option<(usize, &'static str)> {
    if start >= value.len() {
        return None;
    }
    if value[start..].starts_with("[redacted]") {
        return Some((start + "[redacted]".len(), ""));
    }
    let bytes = value.as_bytes();
    if bytes[start] == b'"' {
        if let Some(close_rel) = value[start + 1..].find('"') {
            return Some((start + 1 + close_rel + 1, "\""));
        }
    } else if bytes[start] == b'\'' {
        if let Some(close_rel) = value[start + 1..].find('\'') {
            return Some((start + 1 + close_rel + 1, "'"));
        }
    }
    let len: usize = value[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, ',' | ';' | ')' | '}' | ']'))
        .map(char::len_utf8)
        .sum();
    (len > 0).then_some((start + len, ""))
}

fn redact_credential_assignments(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !value.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let open_quote = matches!(bytes[i], b'"' | b'\'').then(|| bytes[i]);
        let label_start = if open_quote.is_some() { i + 1 } else { i };
        let ident_len = bytes[label_start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'.' | b'-'))
            .count();
        if ident_len == 0 {
            let step = value[i..].chars().next().map_or(1, char::len_utf8);
            i += step;
            continue;
        }
        if open_quote.is_none()
            && i > 0
            && (bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'_' | b'.' | b'-'))
        {
            i += ident_len;
            continue;
        }
        let ident = &value[label_start..label_start + ident_len];
        let mut p = label_start + ident_len;
        if let Some(q) = open_quote {
            if p < bytes.len() && bytes[p] == q {
                p += 1;
            } else {
                i += 1;
                continue;
            }
        } else if p < bytes.len() && matches!(bytes[p], b'"' | b'\'') {
            p += 1;
        }
        if !is_secret_label(ident) {
            i = if open_quote.is_some() { p } else { label_start + ident_len };
            continue;
        }
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        let sep_start = p;
        while p < bytes.len() && matches!(bytes[p], b'=' | b':') {
            p += 1;
        }
        if p == sep_start {
            i = label_start + ident_len;
            continue;
        }
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        let Some((val_end, quote)) = parse_assigned_value(value, p) else {
            i = label_start + ident_len;
            continue;
        };
        out.push_str(&value[cursor..p]);
        out.push_str(quote);
        out.push_str("[redacted]");
        out.push_str(quote);
        cursor = val_end;
        i = val_end;
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_json_bare_keys(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if !lower.contains("\"key") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("\"key") {
        let start = cursor + rel;
        let mut p = start + 4;
        if p < bytes.len() && matches!(bytes[p], b's' | b'S') {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b'"' {
            out.push_str(&value[cursor..start + 4]);
            cursor = start + 4;
            continue;
        }
        p += 1;
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b':' {
            out.push_str(&value[cursor..start + 4]);
            cursor = start + 4;
            continue;
        }
        p += 1;
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        let Some((val_end, quote)) = parse_assigned_value(value, p) else {
            out.push_str(&value[cursor..start + 4]);
            cursor = start + 4;
            continue;
        };
        out.push_str(&value[cursor..p]);
        out.push_str(quote);
        out.push_str("[redacted]");
        out.push_str(quote);
        cursor = val_end;
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_query_secrets(value: &str) -> String {
    if !value.contains(['?', '&']) {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !matches!(bytes[i], b'?' | b'&') {
            i += 1;
            continue;
        }
        let rest_lower_head: String = value[i + 1..]
            .bytes()
            .take(8)
            .map(|b| (b as char).to_ascii_lowercase())
            .collect();
        let param_len = ["key=", "code=", "ticket=", "sig="]
            .iter()
            .find(|p| rest_lower_head.starts_with(**p))
            .map(|p| p.len());
        let Some(plen) = param_len else {
            i += 1;
            continue;
        };
        let val_start = i + 1 + plen;
        if value[val_start..].starts_with("[redacted]") {
            i = val_start + "[redacted]".len();
            continue;
        }
        let mut char_count = 0usize;
        let mut has_digit_within_257 = false;
        let mut val_bytes = 0usize;
        for ch in value[val_start..].chars() {
            if ch.is_whitespace() || matches!(ch, '&' | '#' | '"' | '\'' | '<' | '>' | ')' | ']') {
                break;
            }
            if char_count <= 256 && ch.is_ascii_digit() {
                has_digit_within_257 = true;
            }
            char_count += 1;
            val_bytes += ch.len_utf8();
        }
        if char_count >= 8 && has_digit_within_257 {
            out.push_str(&value[cursor..val_start]);
            out.push_str("[redacted]");
            cursor = val_start + val_bytes;
            i = cursor;
        } else {
            i = val_start;
        }
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_key_formats_after_labels(value: &str) -> String {
    let s = redact_url_passwords(value);
    let s = redact_webhook_urls(&s);
    let s = redact_known_secret_prefixes(&s);
    redact_jwts(&s)
}

fn apply_key_format_rules(value: &str) -> String {
    redact_key_formats_after_labels(&redact_key_formats_before_labels(value))
}

fn apply_text_rules(value: &str, emails: bool) -> String {
    let s = redact_key_formats_before_labels(value);
    let s = redact_credential_assignments(&s);
    let s = redact_json_bare_keys(&s);
    let s = redact_query_secrets(&s);
    let s = redact_key_formats_after_labels(&s);
    if emails { redact_emails(&s) } else { s }
}

fn redact_url_passwords(value: &str) -> String {
    if !value.contains("://") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(rel) = value[search..].find("://") {
        let sep = search + rel;
        let mut s_start = sep;
        while s_start > 0
            && (bytes[s_start - 1].is_ascii_alphanumeric()
                || matches!(bytes[s_start - 1], b'+' | b'.' | b'-'))
        {
            s_start -= 1;
        }
        let scheme_len = sep - s_start;
        if !(1..=32).contains(&scheme_len)
            || !bytes[s_start].is_ascii_alphabetic()
            || (s_start > 0 && is_ascii_word_byte(bytes[s_start - 1]))
        {
            search = sep + 3;
            continue;
        }
        let user_start = sep + 3;
        let mut u_chars = 0usize;
        let mut u_bytes = 0usize;
        for ch in value[user_start..].chars() {
            if ch.is_whitespace()
                || matches!(
                    ch,
                    ':' | '/' | '?' | '#' | '@' | '[' | ']' | '"' | '\'' | '<' | '>'
                )
            {
                break;
            }
            u_chars += 1;
            u_bytes += ch.len_utf8();
        }
        let colon_pos = user_start + u_bytes;
        if u_chars > 256 || colon_pos >= bytes.len() || bytes[colon_pos] != b':' {
            search = sep + 3;
            continue;
        }
        let pass_start = colon_pos + 1;
        let mut p_chars = 0usize;
        let mut p_bytes = 0usize;
        for ch in value[pass_start..].chars() {
            if ch.is_whitespace()
                || matches!(ch, '/' | '?' | '#' | '@' | '"' | '\'' | '<' | '>')
            {
                break;
            }
            p_chars += 1;
            p_bytes += ch.len_utf8();
        }
        let at_pos = pass_start + p_bytes;
        if (1..=256).contains(&p_chars) && at_pos < bytes.len() && bytes[at_pos] == b'@' {
            out.push_str(&value[cursor..pass_start]);
            out.push_str("[redacted]");
            cursor = at_pos;
            search = at_pos + 1;
        } else {
            search = sep + 3;
        }
    }
    out.push_str(&value[cursor..]);
    out
}

fn match_webhook_prefix_len(slice_lower: &str) -> Option<usize> {
    if !slice_lower.starts_with("https://") {
        return None;
    }
    let after = &slice_lower[8..];
    for slack in [
        "hooks.slack.com/services/",
        "hooks.slack.com/workflows/",
        "hooks.slack.com/triggers/",
    ] {
        if after.starts_with(slack) {
            return Some(8 + slack.len());
        }
    }
    let d_sub = after
        .strip_prefix("canary.")
        .or_else(|| after.strip_prefix("ptb."))
        .unwrap_or(after);
    let sub_len = after.len() - d_sub.len();
    let d_host = if let Some(r) = d_sub.strip_prefix("discordapp.com/api/") {
        (r, "discordapp.com/api/".len())
    } else if let Some(r) = d_sub.strip_prefix("discord.com/api/") {
        (r, "discord.com/api/".len())
    } else {
        return None;
    };
    let (api_rest, host_len) = d_host;
    let mut ver_len = 0usize;
    if let Some(v_rest) = api_rest.strip_prefix('v') {
        let digits = v_rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && v_rest.as_bytes().get(digits) == Some(&b'/') {
            ver_len = 1 + digits + 1;
        }
    }
    if api_rest[ver_len..].starts_with("webhooks/") {
        Some(8 + sub_len + host_len + ver_len + "webhooks/".len())
    } else {
        None
    }
}

fn redact_webhook_urls(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if !lower.contains("https://") {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(rel) = lower[search..].find("https://") {
        let start = search + rel;
        let Some(plen) = match_webhook_prefix_len(&lower[start..]) else {
            search = start + 8;
            continue;
        };
        let secret_start = start + plen;
        if value[secret_start..].starts_with("[redacted]") {
            search = secret_start + "[redacted]".len();
            continue;
        }
        let secret_bytes: usize = value[secret_start..]
            .chars()
            .take_while(|c| {
                !c.is_whitespace()
                    && !matches!(c, '"' | '\'' | '<' | '>' | '?' | '#' | ')' | ']')
            })
            .map(char::len_utf8)
            .sum();
        if secret_bytes > 0 {
            out.push_str(&value[cursor..secret_start]);
            out.push_str("[redacted]");
            cursor = secret_start + secret_bytes;
            search = cursor;
        } else {
            search = secret_start;
        }
    }
    out.push_str(&value[cursor..]);
    out
}

#[inline]
fn is_prefix_boundary_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

fn match_known_secret_prefix_at(value: &str, pos: usize) -> Option<usize> {
    let bytes = &value.as_bytes()[pos..];
    let rest = &value[pos..];
    for pfx in [
        "sk-ant",
        "sk-",
        "sk_",
        "xai-",
        "ghp_",
        "gho_",
        "ghu_",
        "ghr_",
        "ghs_",
        "github_pat_",
        "AKIA",
        "xapp-",
        "glpat-",
        "rk_live_",
        "rk_test_",
    ] {
        if rest.starts_with(pfx) {
            let tail = bytes[pfx.len()..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'.' | b'_' | b'-'))
                .count();
            if tail >= 8 {
                return Some(pfx.len() + tail);
            }
        }
    }
    if bytes.len() >= 5
        && rest.starts_with("xox")
        && matches!(bytes[3], b'a' | b'b' | b'e' | b'o' | b'p' | b'r' | b's')
        && bytes[4] == b'-'
    {
        let tail = bytes[5..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'.' | b'_' | b'-'))
            .count();
        if tail >= 8 {
            return Some(5 + tail);
        }
    }
    if rest.starts_with("whsec_") {
        let tail = bytes[6..]
            .iter()
            .take_while(|b| {
                b.is_ascii_alphanumeric() || matches!(**b, b'+' | b'/' | b'=' | b'.' | b'_' | b'-')
            })
            .count();
        if tail >= 8 {
            return Some(6 + tail);
        }
    }
    if rest.starts_with("AIza") {
        let tail = bytes[4..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
            .count();
        if tail >= 30 {
            return Some(4 + tail);
        }
    }
    for pfx in ["npm_", "hf_", "gsk_"] {
        if rest.starts_with(pfx) {
            let tail = bytes[pfx.len()..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric())
                .count();
            if tail >= 30 {
                return Some(pfx.len() + tail);
            }
        }
    }
    if rest.starts_with("ASIA") && bytes.len() >= 20 {
        let ok_16 = bytes[4..20]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        let after_ok = bytes.get(20).is_none_or(|b| !b.is_ascii_alphanumeric());
        if ok_16 && after_ok {
            return Some(20);
        }
    }
    for pfx in ["ya29.", "GOCSPX-"] {
        if rest.starts_with(pfx) {
            let tail = bytes[pfx.len()..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
                .count();
            if tail >= 20 {
                return Some(pfx.len() + tail);
            }
        }
    }
    let bot_off = if rest.starts_with("bot") { 3 } else { 0 };
    let digits = bytes[bot_off..].iter().take_while(|b| b.is_ascii_digit()).count();
    if (6..=12).contains(&digits) && bytes.get(bot_off + digits) == Some(&b':') {
        let tok_start = bot_off + digits + 1;
        let tok_len = bytes[tok_start..]
            .iter()
            .take_while(|b| is_prefix_boundary_byte(**b))
            .count();
        if tok_len == 35 {
            return Some(tok_start + 35);
        }
    }
    None
}

fn redact_known_secret_prefixes(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !value.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if i > 0 && is_prefix_boundary_byte(bytes[i - 1]) {
            i += 1;
            continue;
        }
        if let Some(len) = match_known_secret_prefix_at(value, i) {
            out.push_str(&value[cursor..i]);
            out.push_str("[redacted]");
            cursor = i + len;
            i = cursor;
        } else {
            let step = value[i..].chars().next().map_or(1, char::len_utf8);
            i += step;
        }
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_jwts(value: &str) -> String {
    if !value.contains("eyJ") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(rel) = value[search..].find("eyJ") {
        let start = search + rel;
        if start > 0 && is_prefix_boundary_byte(bytes[start - 1]) {
            search = start + 3;
            continue;
        }
        let seg1 = bytes[start + 3..]
            .iter()
            .take_while(|b| is_prefix_boundary_byte(**b))
            .count();
        let dot1 = start + 3 + seg1;
        if seg1 < 8 || !value[dot1..].starts_with(".eyJ") {
            search = start + 3;
            continue;
        }
        let seg2 = bytes[dot1 + 4..]
            .iter()
            .take_while(|b| is_prefix_boundary_byte(**b))
            .count();
        let dot2 = dot1 + 4 + seg2;
        if seg2 < 8 || bytes.get(dot2) != Some(&b'.') {
            search = start + 3;
            continue;
        }
        let seg3 = bytes[dot2 + 1..]
            .iter()
            .take_while(|b| is_prefix_boundary_byte(**b))
            .count();
        let end = dot2 + 1 + seg3;
        out.push_str(&value[cursor..start]);
        out.push_str("[redacted]");
        cursor = end;
        search = end;
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_emails(value: &str) -> String {
    if !value.contains('@') {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(rel) = value[search..].find('@') {
        let at = search + rel;
        let mut l = at;
        while l > cursor
            && (bytes[l - 1].is_ascii_alphanumeric()
                || matches!(bytes[l - 1], b'.' | b'_' | b'%' | b'+' | b'-'))
        {
            l -= 1;
        }
        let local_len = at - l;
        let mut r = at + 1;
        while r < bytes.len()
            && (bytes[r].is_ascii_alphanumeric() || matches!(bytes[r], b'.' | b'-'))
        {
            r += 1;
        }
        let dom = &value[at + 1..r];
        let valid = (1..=64).contains(&local_len)
            && dom.rfind('.').is_some_and(|dot| {
                (1..=255).contains(&dot)
                    && (2..=24).contains(&(dom.len() - dot - 1))
                    && dom[dot + 1..].bytes().all(|b| b.is_ascii_alphabetic())
            });
        if valid {
            out.push_str(&value[cursor..l]);
            out.push_str("[redacted-email]");
            cursor = r;
            search = r;
        } else {
            search = at + 1;
        }
    }
    out.push_str(&value[cursor..]);
    out
}

fn starts_like_json(value: &str, start: usize) -> bool {
    for b in value.as_bytes()[start + 1..].iter().copied() {
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'{' | b'[') {
            continue;
        }
        return b == b'"';
    }
    false
}

fn find_balanced_end(value: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (off, ch) in value[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + off + ch.len_utf8());
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

fn written_key_count(text: &str) -> isize {
    let bytes = text.as_bytes();
    let mut count = 0isize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i] != b'"' {
            i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        let mut next = i + 1;
        while next < bytes.len() && matches!(bytes[next], b' ' | b'\t' | b'\n' | b'\r') {
            next += 1;
        }
        if bytes.get(next) == Some(&b':') {
            count += 1;
        }
        i += 1;
    }
    count
}

fn unchanged_key_count(parsed: &Value, converted: &Value) -> isize {
    match (parsed, converted) {
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            let mut sum = 0isize;
            for (x, y) in a.iter().zip(b.iter()) {
                let inner = unchanged_key_count(x, y);
                if inner < 0 {
                    return -1;
                }
                sum += inner;
            }
            sum
        }
        (Value::Object(a), Value::Object(b)) if a.len() == b.len() => {
            let mut sum = a.len() as isize;
            for ((k1, v1), (k2, v2)) in a.iter().zip(b.iter()) {
                if k1 != k2 {
                    return -1;
                }
                let inner = unchanged_key_count(v1, v2);
                if inner < 0 {
                    return -1;
                }
                sum += inner;
            }
            sum
        }
        _ => {
            if parsed == converted {
                0
            } else {
                -1
            }
        }
    }
}

fn json_unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('u') => {
                chars.next();
                let mut hex = String::with_capacity(4);
                for _ in 0..4 {
                    if let Some(h) = chars.peek().copied().filter(char::is_ascii_hexdigit) {
                        hex.push(h);
                        chars.next();
                    }
                }
                if hex.len() == 4
                    && let Ok(code) = u16::from_str_radix(&hex, 16)
                    && let Some(decoded) = char::from_u32(u32::from(code))
                {
                    out.push(decoded);
                } else {
                    out.push_str("\\u");
                    out.push_str(&hex);
                }
            }
            Some(esc @ ('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't')) => {
                chars.next();
                out.push(match esc {
                    'b' => '\u{0008}',
                    'f' => '\u{000c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            _ => out.push('\\'),
        }
    }
    out
}

fn has_json_only_secret_shape(decoded: &str) -> bool {
    let lower = decoded.to_ascii_lowercase();
    if lower.contains("\"headers\"") {
        let mut p = 0;
        while let Some(rel) = lower[p..].find("\"headers\"") {
            let after = p + rel + 9;
            let ws = lower.as_bytes()[after..]
                .iter()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            if lower.as_bytes().get(after + ws) == Some(&b':') {
                return true;
            }
            p = after;
        }
    }
    for key_lit in ["\"name\"", "\"key\""] {
        let mut p = 0;
        while let Some(rel) = lower[p..].find(key_lit) {
            let mut cur = p + rel + key_lit.len();
            while cur < lower.len() && lower.as_bytes()[cur].is_ascii_whitespace() {
                cur += 1;
            }
            if lower.as_bytes().get(cur) == Some(&b':') {
                cur += 1;
                while cur < lower.len() && lower.as_bytes()[cur].is_ascii_whitespace() {
                    cur += 1;
                }
                if lower.as_bytes().get(cur) == Some(&b'"')
                    && let Some(end_q) = lower[cur + 1..].find('"')
                    && is_secret_name(&lower[cur + 1..cur + 1 + end_q])
                {
                    return true;
                }
            }
            p += rel + key_lit.len();
        }
    }
    false
}

fn hides_secret(text: &str, emails: bool) -> bool {
    let decoded = json_unescaped(text);
    has_json_only_secret_shape(&decoded)
        || (decoded != text && apply_text_rules(&decoded, emails) != decoded)
}

fn kept_or_unscanned(text: &str, emails: bool) -> String {
    if hides_secret(text, emails) {
        UNSCANNED.to_string()
    } else {
        text.to_string()
    }
}

fn redact_header_strings(value: Value) -> Value {
    match value {
        Value::String(_) => Value::String("[redacted]".to_string()),
        Value::Array(arr) => Value::Array(arr.into_iter().map(redact_header_strings).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, redact_header_strings(v)))
                .collect(),
        ),
        other => other,
    }
}

impl SecretRegistry {
    fn redact_serialized_json(&self, value: &str, emails: bool) -> Option<String> {
        let trimmed = value.trim();
        if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
            return None;
        }
        self.parse_redacted(trimmed, emails)
    }

    fn parse_redacted(&self, candidate: &str, emails: bool) -> Option<String> {
        let parsed: Value = serde_json::from_str(candidate).ok()?;
        let converted = self.convert_value(&parsed, emails, UnscannedTextPolicy::Drop, 0);
        if unchanged_key_count(&parsed, &converted) == written_key_count(candidate) {
            return Some(candidate.to_string());
        }
        serde_json::to_string(&converted).ok()
    }

    fn redact_embedded_json(
        &self,
        value: &str,
        emails: bool,
        unscanned: UnscannedTextPolicy,
    ) -> String {
        let mut result = String::with_capacity(value.len());
        let mut index = 0usize;
        let mut scans = 0usize;
        while index < value.len() {
            let Some(rel) = value[index..].find(['{', '[']) else {
                break;
            };
            let start = index + rel;
            if scans >= MAX_EMBEDDED_SCANS {
                return match unscanned {
                    UnscannedTextPolicy::KeepPastScanBound => {
                        result.push_str(&value[index..]);
                        result
                    }
                    UnscannedTextPolicy::Keep => {
                        result.push_str(&value[index..start]);
                        result.push_str(&kept_or_unscanned(&value[start..], emails));
                        result
                    }
                    UnscannedTextPolicy::Drop => {
                        result.push_str(&value[index..start]);
                        result.push_str(UNSCANNED);
                        result
                    }
                };
            }
            scans += 1;
            let end = find_balanced_end(value, start);
            if end.is_none() && starts_like_json(value, start) {
                if unscanned != UnscannedTextPolicy::Keep || hides_secret(&value[start..], emails) {
                    result.push_str(&value[index..start]);
                    result.push_str(UNSCANNED);
                    return result;
                }
            }
            let parsed = end.and_then(|e| self.parse_redacted(&value[start..e], emails));
            match (parsed, end) {
                (Some(redacted_json), Some(e)) => {
                    result.push_str(&value[index..start]);
                    result.push_str(&redacted_json);
                    index = e;
                }
                (None, Some(e)) if starts_like_json(value, start) => {
                    result.push_str(&value[index..start]);
                    if unscanned == UnscannedTextPolicy::Keep {
                        result.push_str(&kept_or_unscanned(&value[start..e], emails));
                    } else {
                        result.push_str(UNSCANNED);
                    }
                    index = e;
                }
                _ => {
                    let mut next = start + 1;
                    while next < value.len() && matches!(value.as_bytes()[next], b'{' | b'[') {
                        next += 1;
                    }
                    result.push_str(&value[index..next]);
                    index = next;
                }
            }
        }
        result.push_str(&value[index..]);
        result
    }

    fn convert_value(
        &self,
        value: &Value,
        emails: bool,
        unscanned: UnscannedTextPolicy,
        depth: usize,
    ) -> Value {
        if depth >= MAX_CONVERSION_DEPTH {
            return Value::String("[too deep]".to_string());
        }
        match value {
            Value::String(s) => Value::String(self.redact_with_options(s, emails, unscanned)),
            Value::Array(arr) => Value::Array(
                arr.iter()
                    .map(|v| self.convert_value(v, emails, unscanned, depth + 1))
                    .collect(),
            ),
            Value::Object(map) => {
                let secret_pair = map.iter().any(|(k, v)| {
                    (k == "name" || k == "key") && v.as_str().is_some_and(is_secret_name)
                });
                let mut out = Map::with_capacity(map.len());
                for (k, v) in map {
                    let redacted_key = self.redact_with_options(k, emails, unscanned);
                    let redacted_val = if secret_pair && k == "value" {
                        Value::String("[redacted]".to_string())
                    } else if is_secret_name(k) {
                        Value::String("[redacted]".to_string())
                    } else {
                        let converted = self.convert_value(v, emails, unscanned, depth + 1);
                        if k.eq_ignore_ascii_case("headers")
                            && matches!(v, Value::Object(_) | Value::Array(_))
                        {
                            redact_header_strings(converted)
                        } else {
                            converted
                        }
                    };
                    out.insert(redacted_key, redacted_val);
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_prefixes_headers_urls_and_jwts() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0123456789abcdef\n-----END RSA PRIVATE KEY-----";
        assert_eq!(redact_secrets(pem), "[redacted]");
        assert_eq!(
            redact_secrets("Authorization: Bearer abcdefgh12345678"),
            "Authorization: [redacted]"
        );
        assert_eq!(
            redact_secrets("Cookie: session_id=xyz987654321; theme=dark"),
            "Cookie: [redacted]"
        );
        assert_eq!(
            redact_secrets("https://alice:hunter2secret@db.internal:5432/app"),
            "https://alice:[redacted]@db.internal:5432/app"
        );
        assert_eq!(
            redact_secrets("https://hooks.slack.com/services/T000/B000/XXXXXXXXXXXXXXXX"),
            "https://hooks.slack.com/services/[redacted]"
        );
        assert_eq!(
            redact_secrets("https://discord.com/api/v10/webhooks/123456/abcdefg"),
            "https://discord.com/api/v10/webhooks/[redacted]"
        );
        assert_eq!(
            redact_secrets("token sk-ant-api03-1234567890abcdef and ghp_1234567890abcdef"),
            "token [redacted] and [redacted]"
        );
        assert_eq!(
            redact_secrets("bot123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsawX"),
            "[redacted]"
        );
        assert_eq!(
            redact_secrets("jwt=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig123"),
            "jwt=[redacted]"
        );
        assert_eq!(
            redact_secrets("https://api.example.com/v1?key=abc12345def&ok=1"),
            "https://api.example.com/v1?key=[redacted]&ok=1"
        );
    }

    #[test]
    fn redacts_json_objects_and_registered_vault_secrets() {
        let reg = SecretRegistry::new();
        reg.register_secret_value("my-vault-pass-9988", false);
        reg.register_public_values(["https://my-vault-pass-9988.public.example"]);
        let text = r#"sent {"apiKey":"plain123","headers":{"X-Custom":"val"}} and my-vault-pass-9988"#;
        let out = reg.redact_secrets(text);
        assert!(!out.contains("plain123"));
        assert!(!out.contains("my-vault-pass-9988"));
        assert!(out.contains("[redacted]"));
    }
}
