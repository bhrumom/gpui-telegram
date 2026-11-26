//! NFD diacritic-stripped `agent_mention_rank` (`0..=6`), longest-unique `@AgentName` `typed_address`,
//! 3-stage `resolve_channel_recipients`, follow-up/continue regexes, and `<OwnName>:` label stripper.
//! Mirrors `packages/contracts/src/agent-mention-rank.ts` and `src/backend/channel-service.ts`.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::agents::AgentSummaryRecord;
use crate::limits::utf16_len;
use crate::tags::{MENTION_CONT_CHAR_RE, chat_tag_references, leading_address};

pub trait MentionableAgent {
    fn mention_name(&self) -> &str;
    fn mention_title(&self) -> &str;
    fn mention_description(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MentionableAgentInfo {
    pub name: String,
    pub title: String,
    pub description: String,
}

impl MentionableAgent for MentionableAgentInfo {
    fn mention_name(&self) -> &str {
        &self.name
    }
    fn mention_title(&self) -> &str {
        &self.title
    }
    fn mention_description(&self) -> &str {
        &self.description
    }
}

impl<P> MentionableAgent for AgentSummaryRecord<P> {
    fn mention_name(&self) -> &str {
        &self.name
    }
    fn mention_title(&self) -> &str {
        &self.title
    }
    fn mention_description(&self) -> &str {
        &self.description
    }
}

static UNICODE_MARK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{M}+").expect("valid Unicode Mark regex"));

pub static FOLLOW_UP_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:also|instead|actually|please change|change that|correction|continue|resume|go on|keep going|yes|no|use that|make it)\b",
    )
    .expect("valid follow-up prefix regex")
});

pub static CONTINUE_STOPPED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:continue|resume|go on|keep going)\b")
        .expect("valid continue stopped regex")
});

static OWN_NAME_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:\*\*|__)?([^\n:*_]{1,80}?)\s*(?:\*\*|__)?\s*:\s*(?:\*\*|__)?\s*")
        .expect("valid own name prefix regex")
});

const OWN_LABEL_WORDS: [&str; 10] = [
    "answer", "check", "here", "item", "note", "reply", "report", "result", "summary", "update",
];

/// Exact NFD + `\p{M}` (`Mn | Mc | Me`) stripping + lowercase folding (`agent-mention-rank.ts:L7-L9`).
pub fn fold_mention_text(text: &str) -> String {
    let decomposed: String = text.nfd().collect();
    UNICODE_MARK_RE
        .replace_all(&decomposed, "")
        .to_lowercase()
}

pub fn agent_mention_rank<A: MentionableAgent>(agent: &A, query: &str) -> Option<u8> {
    let folded = fold_mention_text(query.trim());
    if folded.is_empty() {
        return Some(0);
    }
    let name = fold_mention_text(agent.mention_name());
    if name == folded {
        return Some(0);
    }
    if name.starts_with(&folded) {
        return Some(1);
    }
    if name
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .any(|word| word.starts_with(&folded))
    {
        return Some(2);
    }
    if name.contains(&folded) {
        return Some(3);
    }
    let title = fold_mention_text(agent.mention_title());
    if title.contains(&folded) {
        return Some(4);
    }
    let description = fold_mention_text(agent.mention_description());
    if description.contains(&folded) {
        return Some(5);
    }
    if format!("{name} {title} {description}").contains(&folded) {
        Some(6)
    } else {
        None
    }
}

pub fn rank_agent_mentions<T: MentionableAgent + Clone>(agents: &[T], query: &str) -> Vec<T> {
    let mut ranked: Vec<(u8, T)> = agents
        .iter()
        .filter_map(|agent| agent_mention_rank(agent, query).map(|r| (r, agent.clone())))
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, agent)| agent).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedChannelAgent {
    pub id: String,
    pub name: String,
}

fn split_at_utf16_len(s: &str, target_units: usize) -> Option<(&str, &str)> {
    if target_units == 0 {
        return Some(("", s));
    }
    let mut units = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        units += ch.len_utf16();
        if units == target_units {
            let split = byte_idx + ch.len_utf8();
            return Some((&s[..split], &s[split..]));
        }
        if units > target_units {
            return None;
        }
    }
    None
}

fn strip_typed_separators(mut s: &str) -> &str {
    loop {
        let Some(first) = s.chars().next() else {
            break;
        };
        if first.is_whitespace() || first == ',' || first == '&' {
            s = &s[first.len_utf8()..];
            continue;
        }
        if s.len() >= 3 && s.is_char_boundary(3) && s[..3].eq_ignore_ascii_case("and") {
            let boundary = s[3..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            if boundary {
                s = &s[3..];
                continue;
            }
        }
        break;
    }
    s
}

/// Longest-unique case-insensitive `@AgentName` parser (`channel-service.ts:#typedAddress`, `L1309-L1327`).
pub fn typed_address(eligible_agents: &[NamedChannelAgent], text: &str) -> Vec<String> {
    let members: Vec<(&NamedChannelAgent, &str, usize, String)> = eligible_agents
        .iter()
        .filter_map(|a| {
            let trimmed = a.name.trim();
            (!trimmed.is_empty()).then(|| (a, trimmed, utf16_len(trimmed), trimmed.to_lowercase()))
        })
        .collect();
    let mut found = Vec::new();
    let mut typed = text.trim_start();
    while let Some(after_at) = typed.strip_prefix('@') {
        let mut best_len = 0usize;
        let mut best_matches: Vec<(&NamedChannelAgent, &str)> = Vec::new();
        for (agent, _, name_units, name_lower) in &members {
            let Some((candidate, after)) = split_at_utf16_len(after_at, *name_units) else {
                continue;
            };
            if candidate.to_lowercase() == *name_lower && !MENTION_CONT_CHAR_RE.is_match(after) {
                if *name_units > best_len {
                    best_len = *name_units;
                    best_matches.clear();
                    best_matches.push((agent, after));
                } else if *name_units == best_len {
                    best_matches.push((agent, after));
                }
            }
        }
        if best_matches.len() != 1 {
            break;
        }
        let (winner, remainder) = best_matches[0];
        found.push(winner.id.clone());
        typed = strip_typed_separators(remainder);
    }
    found
}

/// Scans mid-text `@` mentions outside tag spans (`/(?<=^|[\s(])@/gu`, used by Lead Router `idle` override).
pub fn mid_text_typed_addresses(eligible_agents: &[NamedChannelAgent], text: &str) -> Vec<String> {
    let tags = chat_tag_references(text);
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut prev_char: Option<char> = None;
    for (idx, ch) in text.char_indices() {
        let inside_tag = tags.iter().any(|t| idx >= t.start && idx < t.end);
        if ch == '@'
            && !inside_tag
            && prev_char.is_none_or(|p| p.is_whitespace() || p == '(')
        {
            for id in typed_address(eligible_agents, &text[idx..]) {
                if seen.insert(id.clone()) {
                    found.push(id);
                }
            }
        }
        prev_char = Some(ch);
    }
    found
}

/// 3-stage channel recipient resolver (`channel-service.ts:#recipients`, `L1297-L1307`).
pub fn resolve_channel_recipients(
    eligible_agents: &[NamedChannelAgent],
    text: &str,
    explicit_recipient: Option<&str>,
) -> Vec<String> {
    let eligible_set: HashSet<&str> = eligible_agents.iter().map(|a| a.id.as_str()).collect();
    let address = leading_address(text);
    let mentions: Vec<String> = address
        .mentions
        .iter()
        .filter(|m| eligible_set.contains(m.id.as_str()))
        .map(|m| m.id.clone())
        .collect();

    let addressed = if !mentions.is_empty() || address.everyone {
        mentions
    } else {
        typed_address(eligible_agents, &text[address.end..])
    };

    let mut result = Vec::new();
    let mut seen = HashSet::new();
    if let Some(explicit) = explicit_recipient.filter(|s| !s.is_empty())
        && seen.insert(explicit.to_string())
    {
        result.push(explicit.to_string());
    }
    for id in addressed {
        if seen.insert(id.clone()) {
            result.push(id);
        }
    }
    if address.everyone {
        for agent in eligible_agents {
            if seen.insert(agent.id.clone()) {
                result.push(agent.id.clone());
            }
        }
    }
    result
}

pub fn is_follow_up_message(text: &str) -> bool {
    FOLLOW_UP_PREFIX_RE.is_match(text.trim())
}

pub fn is_continue_stopped_message(text: &str) -> bool {
    CONTINUE_STOPPED_RE.is_match(text.trim())
}

/// Strips redundant `<OwnName>:` / `**<OwnName> report**:` prefixes (`channel-service.ts:L4007-L4017`).
pub fn without_own_name_label(text: &str, name: &str, streaming: bool) -> String {
    let own = name.trim().to_lowercase();
    if own.is_empty() {
        return text.to_string();
    }
    let Some(caps) = OWN_NAME_PREFIX_RE.captures(text) else {
        return text.to_string();
    };
    let Some(full) = caps.get(0) else {
        return text.to_string();
    };
    let Some(raw_label) = caps.get(1) else {
        return text.to_string();
    };
    let label = raw_label.as_str().trim().to_lowercase();
    let own_space = format!("{own} ");
    let suffix_word = label.strip_prefix(&own_space).map(str::trim);
    let matches_own = label == own
        || suffix_word.is_some_and(|w| OWN_LABEL_WORDS.contains(&w));
    if !matches_own {
        return text.to_string();
    }
    let rest = &text[full.end()..];
    if !rest.trim().is_empty() {
        rest.to_string()
    } else if streaming {
        String::new()
    } else {
        text.to_string()
    }
}
