//! Semantic `@[Name](kind:id)` / `@[Name](kind+uri:id)` tag parser, serializer, expander,
//! file quote targets, and `leading_address` scanner.
//! Mirrors `packages/contracts/src/chat-tag-references.ts`.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const QUOTE_TEXT_LIMIT: usize = 2_000;
pub const REFERENCE_NAME_LIMIT: usize = 80;
pub const EVERYONE_MENTION: &str = "@everyone";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatTagKind {
    Agent,
    Skill,
    Mcp,
    Quote,
    Tab,
    Thread,
}

impl ChatTagKind {
    pub const ALL: [Self; 6] = [
        Self::Agent,
        Self::Skill,
        Self::Mcp,
        Self::Quote,
        Self::Tab,
        Self::Thread,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Skill => "skill",
            Self::Mcp => "mcp",
            Self::Quote => "quote",
            Self::Tab => "tab",
            Self::Thread => "thread",
        }
    }

    pub fn from_tag_str(value: &str) -> Self {
        match value {
            "skill" => Self::Skill,
            "mcp" => Self::Mcp,
            "quote" => Self::Quote,
            "tab" => Self::Tab,
            "thread" => Self::Thread,
            _ => Self::Agent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTagReference {
    pub kind: ChatTagKind,
    pub id: String,
    pub name: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileQuoteLines {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum FileQuoteTarget {
    Workspace {
        path: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::limits::non_null_option"
        )]
        lines: Option<FileQuoteLines>,
    },
    Shared {
        path: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::limits::non_null_option"
        )]
        lines: Option<FileQuoteLines>,
    },
    Attachment {
        #[serde(rename = "attachmentId")]
        attachment_id: String,
        name: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::limits::non_null_option"
        )]
        lines: Option<FileQuoteLines>,
    },
}

static CHAT_TAG_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@\[([^\]\r\n]+)\]\((agent|skill|mcp|quote|tab|thread)(\+uri)?:([^)\r\n]+)\)")
        .expect("valid chat tag regex")
});

static FILE_QUOTE_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(workspace|shared|attachment)-file:(.+?)(?:#L(\d+)(?:-(\d+))?)?$")
        .expect("valid file quote regex")
});

static WHITESPACE_RUN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid whitespace regex"));

pub(crate) static MENTION_CONT_CHAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\p{L}\p{N}_'\x{2019}-]").expect("valid mention continuation regex")
});

pub fn limited_quote_text(text: &str) -> String {
    text.chars()
        .take(QUOTE_TEXT_LIMIT)
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn reference_name(name: &str) -> String {
    let collapsed = WHITESPACE_RUN_RE.replace_all(name, " ");
    let trimmed = collapsed.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() > REFERENCE_NAME_LIMIT {
        let head: String = chars[..REFERENCE_NAME_LIMIT - 1].iter().collect();
        format!("{}…", head.trim_end())
    } else {
        chars.into_iter().collect()
    }
}

pub fn file_quote_id(target: &FileQuoteTarget) -> String {
    let (source, file, lines) = match target {
        FileQuoteTarget::Workspace { path, lines } => ("workspace", path.clone(), *lines),
        FileQuoteTarget::Shared { path, lines } => ("shared", path.clone(), *lines),
        FileQuoteTarget::Attachment {
            attachment_id,
            name,
            lines,
        } => ("attachment", format!("{attachment_id}/{name}"), *lines),
    };
    let suffix = match lines {
        None => String::new(),
        Some(l) if l.end > l.start => format!("#L{}-{}", l.start, l.end),
        Some(l) => format!("#L{}", l.start),
    };
    format!("{source}-file:{file}{suffix}")
}

pub fn file_quote_target(id: &str) -> Option<FileQuoteTarget> {
    let caps = FILE_QUOTE_ID_RE.captures(id)?;
    let source = caps.get(1)?.as_str();
    let file = caps.get(2)?.as_str();
    if file.is_empty() {
        return None;
    }
    let lines = caps.get(3).and_then(|m| {
        let start: u64 = m.as_str().parse().ok()?;
        if start < 1 {
            return None;
        }
        let end = caps
            .get(4)
            .and_then(|e| e.as_str().parse::<u64>().ok())
            .unwrap_or(start)
            .max(start);
        Some(FileQuoteLines { start, end })
    });
    match source {
        "workspace" => Some(FileQuoteTarget::Workspace {
            path: file.to_string(),
            lines,
        }),
        "shared" => Some(FileQuoteTarget::Shared {
            path: file.to_string(),
            lines,
        }),
        "attachment" => {
            let slash = file.find('/')?;
            if slash < 1 || slash == file.len() - 1 {
                return None;
            }
            Some(FileQuoteTarget::Attachment {
                attachment_id: file[..slash].to_string(),
                name: file[slash + 1..].to_string(),
                lines,
            })
        }
        _ => None,
    }
}

/// RFC-3986 percent-encoding matching `encodeURIComponent(v).replace(/[!'()*]/gu, %XX)`.
pub fn encode_chat_tag_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn strict_decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex_pair = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let decoded = u8::from_str_radix(hex_pair, 16).ok()?;
            out.push(decoded);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn decode_chat_tag_component(value: &str, encoded: bool) -> String {
    if !encoded {
        return value.to_string();
    }
    strict_decode_uri_component(value).unwrap_or_else(|| value.to_string())
}

fn has_unsafe_name(kind: ChatTagKind, name: &str) -> bool {
    match kind {
        ChatTagKind::Quote | ChatTagKind::Tab | ChatTagKind::Thread => name
            .chars()
            .any(|c| matches!(c, '[' | ']' | '\\' | '`' | '<' | '\r' | '\n')),
        ChatTagKind::Agent | ChatTagKind::Skill | ChatTagKind::Mcp => {
            name.chars().any(|c| matches!(c, ']' | '\r' | '\n'))
        }
    }
}

pub fn serialize_chat_tag_reference(kind: ChatTagKind, name: &str, id: &str) -> String {
    let safe_id = !id.chars().any(|c| matches!(c, ')' | '\r' | '\n'));
    if !name.is_empty() && !id.is_empty() && !has_unsafe_name(kind, name) && safe_id {
        return format!("@[{name}]({}:{id})", kind.as_str());
    }
    format!(
        "@[{}]({}+uri:{})",
        encode_chat_tag_component(name),
        kind.as_str(),
        encode_chat_tag_component(id)
    )
}

pub fn chat_tag_references(value: &str) -> Vec<ChatTagReference> {
    CHAT_TAG_REFERENCE_RE
        .captures_iter(value)
        .filter_map(|caps| {
            let full = caps.get(0)?;
            let encoded = caps.get(3).is_some();
            let kind = ChatTagKind::from_tag_str(caps.get(2)?.as_str());
            let name = decode_chat_tag_component(caps.get(1)?.as_str(), encoded);
            let id = decode_chat_tag_component(caps.get(4)?.as_str(), encoded);
            Some(ChatTagReference {
                kind,
                id,
                name,
                start: full.start(),
                end: full.end(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadingAddress {
    pub mentions: Vec<ChatTagReference>,
    pub everyone: bool,
    pub end: usize,
}

fn consume_address_separators(gap: &str) -> (usize, bool) {
    let mut pos = 0;
    let mut saw_everyone = false;
    while pos < gap.len() {
        let rest = &gap[pos..];
        let Some(first) = rest.chars().next() else {
            break;
        };
        if first.is_whitespace() || first == ',' || first == '&' {
            pos += first.len_utf8();
            continue;
        }
        if rest.len() >= 3 && rest.is_char_boundary(3) && rest[..3].eq_ignore_ascii_case("and") {
            let boundary = rest[3..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            if boundary {
                pos += 3;
                continue;
            }
        }
        if rest.len() >= EVERYONE_MENTION.len()
            && rest.is_char_boundary(EVERYONE_MENTION.len())
            && rest[..EVERYONE_MENTION.len()].eq_ignore_ascii_case(EVERYONE_MENTION)
        {
            let after = &rest[EVERYONE_MENTION.len()..];
            if !MENTION_CONT_CHAR_RE.is_match(after) {
                saw_everyone = true;
                pos += EVERYONE_MENTION.len();
                continue;
            }
        }
        break;
    }
    (pos, saw_everyone)
}

pub fn leading_address(value: &str) -> LeadingAddress {
    let mut mentions = Vec::new();
    let mut end = 0usize;
    let mut everyone = false;
    let refs = chat_tag_references(value);
    for reference in refs.into_iter().map(Some).chain(std::iter::once(None)) {
        let gap = match &reference {
            Some(r) => &value[end..r.start],
            None => &value[end..],
        };
        let (sep_len, saw_everyone) = consume_address_separators(gap);
        everyone |= saw_everyone;
        let Some(r) = reference else {
            break;
        };
        if !matches!(r.kind, ChatTagKind::Agent | ChatTagKind::Quote) || sep_len != gap.len() {
            break;
        }
        if r.kind == ChatTagKind::Agent {
            mentions.push(r.clone());
        }
        end = r.end;
    }
    LeadingAddress {
        mentions,
        everyone,
        end,
    }
}

fn file_quote_heading(file: &FileQuoteTarget) -> String {
    let (name, kind_prefix, lines) = match file {
        FileQuoteTarget::Workspace { path, lines } => (path.as_str(), "", *lines),
        FileQuoteTarget::Shared { path, lines } => (path.as_str(), "the shared file ", *lines),
        FileQuoteTarget::Attachment { name, lines, .. } => {
            (name.as_str(), "the attached file ", *lines)
        }
    };
    let quoted_name = if name.contains('`') {
        name.to_string()
    } else {
        format!("`{name}`")
    };
    let from = format!("From {kind_prefix}{quoted_name}");
    match lines {
        None => format!("{from}:"),
        Some(l) if l.end > l.start => format!("{from}, lines {}-{}:", l.start, l.end),
        Some(l) => format!("{from}, line {}:", l.start),
    }
}

fn expanded_quote(
    quoted: &str,
    file: Option<&FileQuoteTarget>,
    value: &str,
    start: usize,
    end: usize,
    first_non_ws: Option<usize>,
    last_trimmed: usize,
    prev_quote_end: Option<usize>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(f) = file {
        lines.push(file_quote_heading(f));
    }
    let raw_lines: Vec<&str> = quoted.split('\n').collect();
    for (idx, line) in raw_lines.iter().copied().enumerate() {
        let clean = if idx + 1 < raw_lines.len() {
            line.strip_suffix('\r').unwrap_or(line)
        } else {
            line
        };
        if clean
            .trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
            .is_empty()
        {
            lines.push(">".to_string());
        } else {
            lines.push(format!("> {clean}"));
        }
    }
    let quote_body = lines.join("\n");
    let after_quote = prev_quote_end.is_some_and(|qe| {
        qe <= start
            && value[qe..start]
                .chars()
                .all(|c| c.is_whitespace() && c != '\r' && c != '\n')
    });
    let starts_after_blank = start >= 2 && &value[start - 2..start] == "\n\n";
    let separated = first_non_ws.is_none_or(|f| start <= f) || starts_after_blank || after_quote;
    let prefix = if separated {
        ""
    } else if start >= 1 && value.as_bytes()[start - 1] == b'\n' {
        "\n"
    } else {
        "\n\n"
    };
    let suffix = if end >= last_trimmed || value[end..].starts_with("\n\n") {
        ""
    } else if end < value.len() && value.as_bytes()[end] == b'\n' {
        "\n"
    } else {
        "\n\n"
    };
    format!("{prefix}{quote_body}{suffix}")
}

pub fn expand_chat_tag_references<F>(
    value: &str,
    mut resolve_name: Option<F>,
    file_headings: bool,
) -> String
where
    F: FnMut(&ChatTagReference) -> Option<String>,
{
    let first_non_ws = value.char_indices().find(|(_, c)| !c.is_whitespace()).map(|(i, _)| i);
    let last_trimmed = value.trim_end().len();
    let refs = chat_tag_references(value);
    if refs.is_empty() {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;
    let mut prev_quote_end: Option<usize> = None;
    for r in refs {
        out.push_str(&value[cursor..r.start]);
        let replacement = match r.kind {
            ChatTagKind::Quote => {
                if let Some(custom) = resolve_name.as_mut().and_then(|f| f(&r)) {
                    custom
                } else {
                    let target = file_headings.then(|| file_quote_target(&r.id)).flatten();
                    let expanded = expanded_quote(
                        &r.name,
                        target.as_ref(),
                        value,
                        r.start,
                        r.end,
                        first_non_ws,
                        last_trimmed,
                        prev_quote_end,
                    );
                    prev_quote_end = Some(r.end);
                    expanded
                }
            }
            ChatTagKind::Tab | ChatTagKind::Thread => resolve_name
                .as_mut()
                .and_then(|f| f(&r))
                .unwrap_or_else(|| r.name.clone()),
            ChatTagKind::Agent => {
                let n = resolve_name.as_mut().and_then(|f| f(&r)).unwrap_or_else(|| r.name.clone());
                format!("@{n}")
            }
            ChatTagKind::Mcp => {
                let n = resolve_name.as_mut().and_then(|f| f(&r)).unwrap_or_else(|| r.name.clone());
                format!("{n} (MCP server)")
            }
            ChatTagKind::Skill => {
                let n = resolve_name.as_mut().and_then(|f| f(&r)).unwrap_or_else(|| r.name.clone());
                format!("{n} (skill)")
            }
        };
        out.push_str(&replacement);
        cursor = r.end;
    }
    out.push_str(&value[cursor..]);
    out
}
