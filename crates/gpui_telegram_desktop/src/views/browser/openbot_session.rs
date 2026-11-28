//! OpenBot 1:1 Browser Session, Smart Omnibox, Shortcuts, Action Labels,
//! Teach Bar, and Floating BrowserDock / BrowserIsland State Engine.
//!
//! Pure-Rust port of upstream `nightly-labs/openbot`:
//! - `packages/ui/src/features/browser/browser-suggestions.ts`
//! - `packages/ui/src/features/browser/browser-view-shortcuts.ts`
//! - `packages/ui/src/features/browser/browser-action-labels.ts`
//! - `src/renderer/src/features/browser/browser-tab-session.ts`
//! - `src/renderer/src/features/browser/browser-window-model.ts`
//! - `src/renderer/src/features/conversation/stores/browser-store.ts`
//! - `packages/ui/src/features/browser/BrowserDock.tsx` & `BrowserIsland.tsx`

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{Modifiers, RenderImage};
use openbot_contracts::avatar::{AvatarHue, BloubShape};
use openbot_contracts::limits::INPUT_LIMITS;

/// Default URL opened for new OpenBot browser tabs (`browser-suggestions.ts`).
pub const BROWSER_NEW_TAB_URL: &str = "https://www.google.com";
/// Default Google search base URL used by omnibox search fallback.
pub const BROWSER_SEARCH_URL: &str = "https://www.google.com/search?q=";
/// Maximum concurrent open tabs (`INPUT_LIMITS.browser_tabs = 25`).
pub const BROWSER_MAX_TABS: usize = INPUT_LIMITS.browser_tabs;
/// Maximum closed tabs remembered for `Cmd/Ctrl+Shift+T` reopen (`browser-tab-session.ts`).
pub const BROWSER_CLOSED_LIMIT: usize = 25;
/// Maximum history entries kept for omnibox completion (`browser-tab-session.ts`).
pub const BROWSER_HISTORY_LIMIT: usize = 200;
/// Maximum rows shown in the omnibox suggestions popover (`browser-suggestions.ts`).
pub const BROWSER_MAX_SUGGESTIONS: usize = 8;
/// Preview thumbnail refresh interval while tab is loading or agent-busy (`BrowserPreviewSidebar.tsx`).
pub const BROWSER_BUSY_CAPTURE_INTERVAL_MS: u64 = 3_000;
/// Preview thumbnail refresh interval while tab is idle (`BrowserPreviewSidebar.tsx`).
pub const BROWSER_IDLE_CAPTURE_INTERVAL_MS: u64 = 20_000;
/// Auto-dismiss timeout for non-question agent replies in `BrowserDock` (`BrowserDock.tsx`).
pub const BROWSER_DOCK_REPLY_CLOSE_MS: u64 = 6_000;

/// Exact OpenBot geometry constants (`browser-full-view.css`, `browser-dock.css`, `browser-previews.css`).
pub const BROWSER_FULL_TOP_HEIGHT_PX: f32 = 38.0;
pub const BROWSER_ADDRESS_ROW_HEIGHT_PX: f32 = 44.0;
pub const BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX: f32 = 82.0;
pub const BROWSER_DOCK_MAX_WIDTH_PX: f32 = 640.0;
pub const BROWSER_TAB_MIN_WIDTH_PX: f32 = 120.0;
pub const BROWSER_TAB_MAX_WIDTH_PX: f32 = 200.0;
pub const BROWSER_PREVIEW_ASPECT_RATIO: f32 = 16.0 / 10.0;

// ============================================================================
// Smart Omnibox URL Resolution & Autocomplete Suggestions (`browser-suggestions.ts`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserHistoryEntry {
    pub url: String,
    pub title: String,
    pub visits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSuggestionKind {
    Tab,
    History,
    Search,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSuggestion {
    pub id: String,
    pub kind: BrowserSuggestionKind,
    pub text: String,
    pub title: String,
    pub detail: Option<String>,
    pub url: String,
    pub tab_id: Option<String>,
}

fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input.as_bytes() {
        let b = *byte;
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode_query(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Builds a Google search URL for `query` (`browserSearchUrl`).
pub fn browser_search_url(query: &str) -> String {
    format!("{BROWSER_SEARCH_URL}{}", percent_encode_query(query))
}

/// Extracts the Google search query if `url` is a Google search results URL (`browserSearchQuery`).
pub fn browser_search_query(url: &str) -> Option<String> {
    let rest = url.strip_prefix(BROWSER_SEARCH_URL)?;
    let raw = rest.split('&').next().unwrap_or("");
    Some(percent_decode_query(raw))
}

/// Strips `http(s)://`, optional leading `www.`, and trailing `/` (`browserDisplayUrl`).
pub fn browser_display_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let stripped = if lower.starts_with("https://www.") {
        &url[12..]
    } else if lower.starts_with("http://www.") {
        &url[11..]
    } else if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else {
        url
    };
    stripped.strip_suffix('/').unwrap_or(stripped).to_string()
}

fn extract_url_host(candidate: &str) -> Option<&str> {
    let lower = candidate.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &candidate[8..]
    } else if lower.starts_with("http://") {
        &candidate[7..]
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        let host = &host_port[..=end];
        if host.len() > 2 {
            return Some(host);
        }
        return None;
    }
    let host = match host_port.split_once(':') {
        Some((h, port)) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            h
        }
        None => host_port,
    };
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return None;
    }
    if host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
    {
        Some(host)
    } else {
        None
    }
}

/// Resolves arbitrary omnibox input into a navigable URL or Google search URL (`browserAddressUrl`).
pub fn browser_address_url(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return BROWSER_NEW_TAB_URL.to_string();
    }
    let lower = value.to_ascii_lowercase();
    if lower == "about:blank" {
        return "about:blank".to_string();
    }
    let has_protocol = lower.starts_with("http://") || lower.starts_with("https://");
    let candidate = if has_protocol {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    if !value.chars().any(char::is_whitespace) {
        if let Some(host) = extract_url_host(&candidate) {
            if has_protocol
                || host.contains('.')
                || host.eq_ignore_ascii_case("localhost")
                || host.starts_with('[')
            {
                return candidate;
            }
        }
    }
    let query = if lower.starts_with("https://") {
        &value[8..]
    } else if lower.starts_with("http://") {
        &value[7..]
    } else {
        value
    };
    browser_search_url(query)
}

/// Returns whether a URL should be recorded in history & closed-tab stack (`recordable`).
pub fn is_recordable_browser_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Inline omnibox URL completion against visited history (`browserCompletion`).
pub fn browser_completion<'a>(
    typed: &str,
    history: &'a [BrowserHistoryEntry],
) -> Option<&'a BrowserHistoryEntry> {
    let text = typed.to_ascii_lowercase();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let mut best: Option<&BrowserHistoryEntry> = None;
    for entry in history {
        if browser_search_query(&entry.url).is_some() {
            continue;
        }
        let shown = browser_display_url(&entry.url).to_ascii_lowercase();
        if shown.len() <= text.len() || !shown.starts_with(&text) {
            continue;
        }
        if best.is_none_or(|b| entry.visits > b.visits) {
            best = Some(entry);
        }
    }
    best
}

fn search_row(query: &str) -> BrowserSuggestion {
    BrowserSuggestion {
        id: format!("search:{}", query.to_ascii_lowercase()),
        kind: BrowserSuggestionKind::Search,
        text: query.to_string(),
        title: query.to_string(),
        detail: None,
        url: browser_search_url(query),
        tab_id: None,
    }
}

fn history_row(entry: &BrowserHistoryEntry) -> BrowserSuggestion {
    let query = browser_search_query(&entry.url);
    let display = browser_display_url(&entry.url);
    BrowserSuggestion {
        id: format!("history:{}", entry.url),
        kind: BrowserSuggestionKind::History,
        text: query.clone().unwrap_or_else(|| display.clone()),
        title: query.clone().unwrap_or_else(|| {
            if entry.title.is_empty() {
                display.clone()
            } else {
                entry.title.clone()
            }
        }),
        detail: if query.is_none() { Some(display) } else { None },
        url: entry.url.clone(),
        tab_id: None,
    }
}

fn tab_row(tab: &BrowserTabEntry) -> BrowserSuggestion {
    let display = browser_display_url(&tab.url);
    BrowserSuggestion {
        id: format!("tab:{}", tab.id),
        kind: BrowserSuggestionKind::Tab,
        text: display.clone(),
        title: if tab.title.is_empty() {
            display.clone()
        } else {
            tab.title.clone()
        },
        detail: Some(display),
        url: tab.url.clone(),
        tab_id: Some(tab.id.clone()),
    }
}

/// Computes up to 8 omnibox suggestions matching `browserSuggestions` in `browser-suggestions.ts`.
pub fn browser_suggestions(
    typed: &str,
    completion: Option<&BrowserHistoryEntry>,
    tabs: &[BrowserTabEntry],
    active_tab_id: Option<&str>,
    history: &[BrowserHistoryEntry],
) -> Vec<BrowserSuggestion> {
    let query = typed.trim();
    let text = query.to_ascii_lowercase();
    let open_urls: HashSet<&str> = tabs.iter().map(|t| t.url.as_str()).collect();
    if text.is_empty() {
        return history
            .iter()
            .filter(|entry| !open_urls.contains(entry.url.as_str()))
            .take(6)
            .map(history_row)
            .collect();
    }

    let matches_page = |url: &str, title: &str| {
        title.to_ascii_lowercase().contains(&text)
            || browser_display_url(url).to_ascii_lowercase().contains(&text)
    };
    let mut rows: Vec<BrowserSuggestion> = Vec::new();
    let mut add = |row: BrowserSuggestion| {
        if !rows.iter().any(|existing| existing.id == row.id) {
            rows.push(row);
        }
    };

    let completed_tab = completion.and_then(|c| {
        tabs.iter()
            .find(|t| t.url == c.url && Some(t.id.as_str()) != active_tab_id)
    });
    let address = browser_address_url(query);
    if let Some(tab) = completed_tab {
        add(tab_row(tab));
    } else if let Some(entry) = completion {
        add(history_row(entry));
    } else if address != browser_search_url(query) {
        add(BrowserSuggestion {
            id: format!("url:{address}"),
            kind: BrowserSuggestionKind::Url,
            text: query.to_string(),
            title: browser_display_url(&address),
            detail: None,
            url: address,
            tab_id: None,
        });
    }
    add(search_row(query));

    for tab in tabs
        .iter()
        .filter(|t| {
            Some(t.id.as_str()) != active_tab_id
                && !t.url.is_empty()
                && matches_page(&t.url, &t.title)
        })
        .take(3)
    {
        add(tab_row(tab));
    }
    for entry in history
        .iter()
        .filter(|e| !open_urls.contains(e.url.as_str()) && matches_page(&e.url, &e.title))
        .take(4)
    {
        add(history_row(entry));
    }
    rows.truncate(BROWSER_MAX_SUGGESTIONS);
    rows
}

// ============================================================================
// Keyboard Shortcuts (`browser-view-shortcuts.ts`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserShortcutPlatform {
    Mac,
    Pc,
}

impl BrowserShortcutPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Pc
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserTabIndexTarget {
    Index(usize),
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserViewCommand {
    Select(isize),
    SelectIndex(BrowserTabIndexTarget),
    Move(isize),
    NewTab,
    Reopen,
    Close,
    FocusAddress,
    Reload,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserShortcutName {
    NewTab,
    Reopen,
    Close,
    Reload,
    Forward,
    MoveLeft,
    MoveRight,
}

/// Maps key + modifiers to OpenBot's canonical `BrowserViewCommand` (`browserViewCommand`).
pub fn resolve_browser_view_command(
    raw_key: &str,
    mods: Modifiers,
    platform: BrowserShortcutPlatform,
) -> Option<BrowserViewCommand> {
    let key = raw_key.to_ascii_lowercase();
    let mod_primary = match platform {
        BrowserShortcutPlatform::Mac => mods.platform && !mods.control,
        BrowserShortcutPlatform::Pc => mods.control && !mods.platform,
    };
    if mods.control && !mods.platform && !mods.alt && key == "tab" {
        return Some(BrowserViewCommand::Select(if mods.shift { -1 } else { 1 }));
    }
    if mods.control
        && mods.shift
        && !mods.platform
        && !mods.alt
        && (key == "pageup" || key == "pagedown")
    {
        return Some(BrowserViewCommand::Move(if key == "pagedown" { 1 } else { -1 }));
    }
    if !mod_primary {
        return None;
    }
    if mods.alt {
        if mods.shift || platform != BrowserShortcutPlatform::Mac {
            return None;
        }
        return match key.as_str() {
            "right" | "arrowright" => Some(BrowserViewCommand::Select(1)),
            "left" | "arrowleft" => Some(BrowserViewCommand::Select(-1)),
            _ => None,
        };
    }
    if key == "t" {
        return Some(if mods.shift {
            BrowserViewCommand::Reopen
        } else {
            BrowserViewCommand::NewTab
        });
    }
    if mods.shift {
        return None;
    }
    match key.as_str() {
        "w" => Some(BrowserViewCommand::Close),
        "l" => Some(BrowserViewCommand::FocusAddress),
        "r" => Some(BrowserViewCommand::Reload),
        "[" => Some(BrowserViewCommand::Back),
        "]" => Some(BrowserViewCommand::Forward),
        "1" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(0))),
        "2" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(1))),
        "3" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(2))),
        "4" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(3))),
        "5" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(4))),
        "6" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(5))),
        "7" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(6))),
        "8" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(7))),
        "9" => Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Last)),
        _ => None,
    }
}

/// Returns the platform-appropriate shortcut hint glyph (`browserShortcutGlyph`).
pub const fn browser_shortcut_glyph(
    name: BrowserShortcutName,
    platform: BrowserShortcutPlatform,
) -> &'static str {
    match (platform, name) {
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::NewTab) => "⌘T",
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::Reopen) => "⇧⌘T",
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::Close) => "⌘W",
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::Reload) => "⌘R",
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::Forward) => "⌘]",
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::MoveLeft) => "⌃⇧PgUp",
        (BrowserShortcutPlatform::Mac, BrowserShortcutName::MoveRight) => "⌃⇧PgDn",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::NewTab) => "Ctrl+T",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::Reopen) => "Ctrl+Shift+T",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::Close) => "Ctrl+W",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::Reload) => "Ctrl+R",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::Forward) => "Ctrl+]",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::MoveLeft) => "Ctrl+Shift+PgUp",
        (BrowserShortcutPlatform::Pc, BrowserShortcutName::MoveRight) => "Ctrl+Shift+PgDn",
    }
}

// ============================================================================
// Localized Action Labels (`browser-action-labels.ts` & `zh/browser.ts`)
// ============================================================================

/// Localized agent browser action status string (`browserActionLabel`).
pub fn browser_action_label(action: &str, is_zh: bool) -> &'static str {
    match (action.trim().to_ascii_lowercase().as_str(), is_zh) {
        ("open", true) => "正在打开页面…",
        ("open", false) => "Opening page…",
        ("list-tabs" | "list_tabs", true) => "正在检查标签页…",
        ("list-tabs" | "list_tabs", false) => "Checking tabs…",
        ("snapshot", true) => "正在读取页面…",
        ("snapshot", false) => "Reading page…",
        ("click" | "double_click", true) => "正在点击…",
        ("click" | "double_click", false) => "Clicking…",
        ("type" | "fill" | "fill_fields", true) => "正在输入…",
        ("type" | "fill" | "fill_fields", false) => "Typing…",
        ("key" | "press" | "press_key", true) => "正在使用键盘…",
        ("key" | "press" | "press_key", false) => "Using keyboard…",
        ("scroll", true) => "正在滚动…",
        ("scroll", false) => "Scrolling…",
        ("back", true) => "正在后退…",
        ("back", false) => "Going back…",
        ("forward", true) => "正在前进…",
        ("forward", false) => "Going forward…",
        ("reload", true) => "正在刷新…",
        ("reload", false) => "Reloading…",
        ("screenshot", true) => "正在截图…",
        ("screenshot", false) => "Taking screenshot…",
        ("status", true) => "正在检查浏览器状态…",
        ("status", false) => "Checking browser status…",
        ("navigate", true) => "正在导航…",
        ("navigate", false) => "Navigating…",
        ("hover", true) => "正在悬停…",
        ("hover", false) => "Hovering…",
        ("select-option" | "select_option", true) => "正在选择选项…",
        ("select-option" | "select_option", false) => "Selecting option…",
        ("set-checked" | "set_checked", true) => "正在更改控件…",
        ("set-checked" | "set_checked", false) => "Updating control…",
        ("drag", true) => "正在拖拽…",
        ("drag", false) => "Dragging…",
        ("upload-files" | "upload_files" | "upload_file", true) => "正在上传文件…",
        ("upload-files" | "upload_files" | "upload_file", false) => "Uploading files…",
        ("wait-for" | "wait_for", true) => "正在等待页面…",
        ("wait-for" | "wait_for", false) => "Waiting for page…",
        ("evaluate", true) => "正在执行页面 JavaScript…",
        ("evaluate", false) => "Running page script…",
        ("set-environment" | "set_environment", true) => "正在调整视口…",
        ("set-environment" | "set_environment", false) => "Adjusting viewport…",
        ("recording-start" | "recording_start", true) => "正在开始录制…",
        ("recording-start" | "recording_start", false) => "Starting recording…",
        ("recording-stop" | "recording_stop", true) => "正在保存录制…",
        ("recording-stop" | "recording_stop", false) => "Saving recording…",
        ("close-tab" | "close_tab", true) => "正在关闭标签页…",
        ("close-tab" | "close_tab", false) => "Closing tab…",
        (_, true) => "正在处理中…",
        (_, false) => "Working on it…",
    }
}

// ============================================================================
// Multi-Tab Browser Session (`browser-tab-session.ts` + `browser-store.ts`)
// ============================================================================

/// Visual identity of the agent that owns or controls a browser tab / `BrowserDock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserTabOwner {
    pub agent_id: String,
    pub name: String,
    pub avatar_seed: String,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<BloubShape>,
    pub avatar_url: Option<String>,
}

impl Default for BrowserTabOwner {
    fn default() -> Self {
        Self {
            agent_id: "default-agent".to_string(),
            name: "New agent".to_string(),
            avatar_seed: "plush:galet".to_string(),
            avatar_hue: Some(AvatarHue::Violet),
            avatar_color: None,
            avatar_shape: Some(BloubShape::Galet),
            avatar_url: None,
        }
    }
}

/// Optional popup-blocked alert attached to a browser tab (`BrowserFullView.tsx`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPopupFailure {
    pub id: String,
    pub message: String,
}

/// Cached 16:10 thumbnail preview for a browser tab (`BrowserPreviewSidebar.tsx`).
#[derive(Clone, Default)]
pub struct BrowserTabPreview {
    pub image: Option<Arc<RenderImage>>,
    pub width: u32,
    pub height: u32,
    pub failed: bool,
    pub captured_at_ms: u64,
}

impl std::fmt::Debug for BrowserTabPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserTabPreview")
            .field("has_image", &self.image.is_some())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("failed", &self.failed)
            .field("captured_at_ms", &self.captured_at_ms)
            .finish()
    }
}

/// Full per-tab model mirroring `BrowserTab` in `@openbot/contracts/ipc-browser`.
#[derive(Debug, Clone)]
pub struct BrowserTabEntry {
    pub id: String,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub nav_history: Vec<String>,
    pub nav_index: usize,
    pub owner_agent_id: Option<String>,
    pub owner_thread_id: Option<String>,
    pub profile_id: String,
    pub profile_hue: Option<u16>,
    pub proxy_name: Option<String>,
    pub recording: bool,
    pub diagnostic_error_count: usize,
    pub user_control: bool,
    pub popup_failure: Option<BrowserPopupFailure>,
    pub preview: BrowserTabPreview,
}

impl BrowserTabEntry {
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        owner_agent_id: Option<String>,
        owner_thread_id: Option<String>,
        profile_id: impl Into<String>,
    ) -> Self {
        let url_str = url.into();
        let initial_title = derive_initial_title(&url_str);
        let nav_history = if url_str.is_empty() {
            Vec::new()
        } else {
            vec![url_str.clone()]
        };
        Self {
            id: id.into(),
            title: initial_title,
            url: url_str,
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            nav_history,
            nav_index: 0,
            owner_agent_id,
            owner_thread_id,
            profile_id: profile_id.into(),
            profile_hue: None,
            proxy_name: None,
            recording: false,
            diagnostic_error_count: 0,
            user_control: false,
            popup_failure: None,
            preview: BrowserTabPreview::default(),
        }
    }

    /// Mirrors `useBrowserTabTitle` (`BrowserTabIcon.tsx`).
    pub fn display_title(&self, is_zh: bool) -> String {
        if self.loading && self.title.is_empty() {
            return if is_zh { "加载中…" } else { "Loading…" }.to_string();
        }
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        let host = browser_display_url(&self.url);
        if !host.is_empty() {
            return host;
        }
        if is_zh {
            "新建浏览器标签页".to_string()
        } else {
            "New tab".to_string()
        }
    }

    /// Mirrors `BrowserPreviewCard` fallback title (`BrowserPreviewSidebar.tsx`).
    pub fn preview_title(&self, is_zh: bool) -> String {
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        let host = browser_display_url(&self.url);
        if !host.is_empty() {
            return host;
        }
        if is_zh {
            "浏览器页面".to_string()
        } else {
            "Browser page".to_string()
        }
    }

    /// Records a committed URL into this tab's local back/forward history stack.
    pub fn record_navigation(&mut self, url: &str) {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.nav_history.get(self.nav_index).map(String::as_str) != Some(trimmed) {
            if !self.nav_history.is_empty() {
                self.nav_history.truncate(self.nav_index.saturating_add(1));
            }
            self.nav_history.push(trimmed.to_string());
            self.nav_index = self.nav_history.len().saturating_sub(1);
        }
        self.url = trimmed.to_string();
        self.can_go_back = self.nav_index > 0;
        self.can_go_forward = self.nav_index + 1 < self.nav_history.len();
    }
}

fn derive_initial_title(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("google.com") && browser_search_query(url).is_none() {
        return "Google".to_string();
    }
    if let Some(q) = browser_search_query(url) {
        if !q.trim().is_empty() {
            return format!("{q} - Google");
        }
    }
    if let Some(host) = extract_url_host(url) {
        let clean = host.strip_prefix("www.").unwrap_or(host);
        if !clean.is_empty() {
            return clean.to_string();
        }
    }
    String::new()
}

/// Closed tab entry stored for `Cmd/Ctrl+Shift+T` restore (`browser-tab-session.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedBrowserTab {
    pub url: String,
    pub title: String,
    pub index: usize,
}

/// Explicit insertion placement for new/duplicated/reopened tabs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabPlacement {
    pub after_id: Option<String>,
    pub index: Option<usize>,
}

/// Persistent single-source-of-truth OpenBot browser session shared between
/// the Chat Right SidePanel (`BrowserPreviewSidebar`) and `BrowserFullView`.
#[derive(Debug, Clone)]
pub struct OpenBotBrowserSession {
    pub tabs: Vec<BrowserTabEntry>,
    pub active_tab_id: Option<String>,
    pub next_tab_seq: u64,
    pub last_shown_by_agent: HashMap<String, (String, usize)>,
    pub closed_tabs: Vec<ClosedBrowserTab>,
    pub history: Vec<BrowserHistoryEntry>,
    pub noticed_tab_ids: HashSet<String>,
    pub dismissed_popup_failures: HashSet<String>,
    /// OpenBot SidePanel toggle: `false` = single "浏览器" tab with preview cards (`image.png`),
    /// `true` = individual browser pages as tabs in SidePanel header (`SidePanel.tsx`).
    pub page_tabs_shown: bool,
}

impl Default for OpenBotBrowserSession {
    fn default() -> Self {
        Self::new_with_default_tab()
    }
}

impl OpenBotBrowserSession {
    /// Creates a session initialized with one default Google tab (`https://www.google.com`),
    /// matching OpenBot's initial state shown in `image.png` / `image_2.png`.
    pub fn new_with_default_tab() -> Self {
        let mut session = Self::new_empty();
        let _ = session.open_tab(
            BROWSER_NEW_TAB_URL,
            None,
            None,
            "default",
            TabPlacement::default(),
            true,
        );
        session
    }

    /// Creates a completely empty session (0 tabs -> empty preview sidebar state).
    pub fn new_empty() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab_id: None,
            next_tab_seq: 1,
            last_shown_by_agent: HashMap::new(),
            closed_tabs: Vec::new(),
            history: Vec::new(),
            noticed_tab_ids: HashSet::new(),
            dismissed_popup_failures: HashSet::new(),
            page_tabs_shown: false,
        }
    }

    pub fn active_tab(&self) -> Option<&BrowserTabEntry> {
        let id = self.active_tab_id.as_deref()?;
        self.tabs
            .iter()
            .find(|t| t.id == id)
            .or_else(|| self.tabs.first())
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut BrowserTabEntry> {
        let idx = self.active_tab_index()?;
        self.tabs.get_mut(idx)
    }

    pub fn active_tab_index(&self) -> Option<usize> {
        if let Some(ref id) = self.active_tab_id {
            if let Some(idx) = self.tabs.iter().position(|t| &t.id == id) {
                return Some(idx);
            }
        }
        if self.tabs.is_empty() {
            None
        } else {
            Some(0)
        }
    }

    pub fn tab_by_id(&self, tab_id: &str) -> Option<&BrowserTabEntry> {
        self.tabs.iter().find(|t| t.id == tab_id)
    }

    pub fn tab_by_id_mut(&mut self, tab_id: &str) -> Option<&mut BrowserTabEntry> {
        self.tabs.iter_mut().find(|t| t.id == tab_id)
    }

    pub fn can_reopen(&self) -> bool {
        !self.closed_tabs.is_empty()
    }

    /// Opens a new tab enforcing the `BROWSER_MAX_TABS` (25) limit and OpenBot placement semantics.
    pub fn open_tab(
        &mut self,
        raw_url: &str,
        owner_agent_id: Option<String>,
        owner_thread_id: Option<String>,
        profile_id: &str,
        placement: TabPlacement,
        noticed: bool,
    ) -> Result<String, String> {
        if self.tabs.len() >= BROWSER_MAX_TABS {
            return Err(format!(
                "Browser tab count exceeds maximum limit of {BROWSER_MAX_TABS} tabs."
            ));
        }
        let url = if raw_url.trim().is_empty() {
            BROWSER_NEW_TAB_URL.to_string()
        } else {
            browser_address_url(raw_url)
        };
        let id = format!("browser-tab-{}", self.next_tab_seq);
        self.next_tab_seq = self.next_tab_seq.saturating_add(1);
        let entry = BrowserTabEntry::new(
            id.clone(),
            url.clone(),
            owner_agent_id.clone(),
            owner_thread_id,
            profile_id,
        );
        let insert_idx = if let Some(ref after_id) = placement.after_id {
            self.tabs
                .iter()
                .position(|t| &t.id == after_id)
                .map(|i| i + 1)
                .unwrap_or(self.tabs.len())
        } else if let Some(idx) = placement.index {
            idx.min(self.tabs.len())
        } else {
            self.tabs.len()
        };
        self.tabs.insert(insert_idx, entry);
        self.active_tab_id = Some(id.clone());
        if let Some(agent_id) = owner_agent_id {
            self.last_shown_by_agent
                .insert(agent_id, (id.clone(), insert_idx));
        }
        if noticed {
            self.noticed_tab_ids.insert(id.clone());
        }
        let title = self
            .tabs
            .get(insert_idx)
            .map(|t| t.title.clone())
            .unwrap_or_default();
        self.record_visit(&url, &title);
        Ok(id)
    }

    /// Activates `tab_id`, updating per-agent last-shown tracking (`browser-store.ts`).
    pub fn activate_tab(&mut self, tab_id: &str) -> bool {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return false;
        };
        self.active_tab_id = Some(tab_id.to_string());
        self.noticed_tab_ids.insert(tab_id.to_string());
        if let Some(agent_id) = self.tabs[idx].owner_agent_id.clone() {
            self.last_shown_by_agent
                .insert(agent_id, (tab_id.to_string(), idx));
        }
        true
    }

    /// Duplicates `tab_id` immediately to its right (`BrowserTabMenu.tsx` + `browser-full-view-controls.ts`).
    pub fn duplicate_tab(&mut self, tab_id: &str) -> Result<String, String> {
        let Some(src) = self.tabs.iter().find(|t| t.id == tab_id).cloned() else {
            return Err("Tab not found".to_string());
        };
        self.open_tab(
            &src.url,
            src.owner_agent_id,
            src.owner_thread_id,
            &src.profile_id,
            TabPlacement {
                after_id: Some(tab_id.to_string()),
                index: None,
            },
            true,
        )
    }

    /// Reorders `tab_id` to `to_index` (`moveBrowserTab`).
    pub fn move_tab(&mut self, tab_id: &str, to_index: usize) -> bool {
        let Some(from) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return false;
        };
        let target = to_index.min(self.tabs.len().saturating_sub(1));
        if from == target {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(target, tab);
        true
    }

    /// Closes one or more tabs in reverse index order, remembering recordable URLs in `closed_tabs`
    /// for `Cmd/Ctrl+Shift+T` restore (`closeBrowserTabsInOrder`).
    /// Returns the number of tabs actually closed.
    pub fn close_tabs(&mut self, tab_ids: &[String]) -> usize {
        if tab_ids.is_empty() {
            return 0;
        }
        let id_set: HashSet<&str> = tab_ids.iter().map(String::as_str).collect();
        let mut to_close: Vec<(usize, String)> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| id_set.contains(t.id.as_str()))
            .map(|(idx, t)| (idx, t.id.clone()))
            .collect();
        if to_close.is_empty() {
            return 0;
        }
        let prev_active_idx = self.active_tab_index();
        let closing_active = self
            .active_tab_id
            .as_deref()
            .is_some_and(|aid| id_set.contains(aid));

        // Sort descending by index to preserve original indices while popping (matches OpenBot)
        to_close.sort_by(|a, b| b.0.cmp(&a.0));
        let count = to_close.len();
        for (idx, _id) in to_close {
            let removed = self.tabs.remove(idx);
            self.noticed_tab_ids.remove(&removed.id);
            if is_recordable_browser_url(&removed.url) {
                self.closed_tabs.insert(
                    0,
                    ClosedBrowserTab {
                        url: removed.url,
                        title: removed.title,
                        index: idx,
                    },
                );
                self.closed_tabs.truncate(BROWSER_CLOSED_LIMIT);
            }
        }

        if self.tabs.is_empty() {
            self.active_tab_id = None;
        } else if closing_active || self.active_tab_id.is_none() {
            let idx = prev_active_idx.unwrap_or(0);
            let next_idx = idx.min(self.tabs.len().saturating_sub(1));
            self.active_tab_id = self.tabs.get(next_idx).map(|t| t.id.clone());
        }
        count
    }

    pub fn close_left(&mut self, tab_id: &str) -> usize {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return 0;
        };
        let ids: Vec<String> = self.tabs[..idx].iter().map(|t| t.id.clone()).collect();
        self.close_tabs(&ids)
    }

    pub fn close_right(&mut self, tab_id: &str) -> usize {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return 0;
        };
        let ids: Vec<String> = self.tabs[idx + 1..].iter().map(|t| t.id.clone()).collect();
        self.close_tabs(&ids)
    }

    pub fn close_others(&mut self, tab_id: &str) -> usize {
        if !self.tabs.iter().any(|t| t.id == tab_id) {
            return 0;
        }
        let ids: Vec<String> = self
            .tabs
            .iter()
            .filter(|t| t.id != tab_id)
            .map(|t| t.id.clone())
            .collect();
        self.close_tabs(&ids)
    }

    /// Reopens the most recently closed tab at its original index (`takeClosedBrowserTab`).
    pub fn reopen_closed_tab(
        &mut self,
        owner_agent_id: Option<String>,
        owner_thread_id: Option<String>,
        profile_id: &str,
    ) -> Result<Option<String>, String> {
        if self.closed_tabs.is_empty() {
            return Ok(None);
        }
        if self.tabs.len() >= BROWSER_MAX_TABS {
            return Err(format!(
                "Browser tab count exceeds maximum limit of {BROWSER_MAX_TABS} tabs."
            ));
        }
        let closed = self.closed_tabs.remove(0);
        let id = self.open_tab(
            &closed.url,
            owner_agent_id,
            owner_thread_id,
            profile_id,
            TabPlacement {
                after_id: None,
                index: Some(closed.index),
            },
            true,
        )?;
        if !closed.title.is_empty() {
            if let Some(tab) = self.tab_by_id_mut(&id) {
                tab.title = closed.title;
            }
        }
        Ok(Some(id))
    }

    /// Records a visited URL in session history (`recordBrowserVisit`).
    pub fn record_visit(&mut self, url: &str, title: &str) {
        if !is_recordable_browser_url(url) {
            return;
        }
        let prev = self
            .history
            .iter()
            .position(|e| e.url == url)
            .map(|idx| self.history.remove(idx));
        let visits = prev.as_ref().map(|p| p.visits).unwrap_or(0).saturating_add(1);
        let resolved_title = if !title.trim().is_empty() {
            title.trim().to_string()
        } else if let Some(p) = prev {
            p.title
        } else {
            derive_initial_title(url)
        };
        self.history.insert(
            0,
            BrowserHistoryEntry {
                url: url.to_string(),
                title: resolved_title,
                visits,
            },
        );
        self.history.truncate(BROWSER_HISTORY_LIMIT);
    }

    /// Updates the title for a visited URL in session history (`recordBrowserTitle`).
    pub fn record_title(&mut self, url: &str, title: &str) {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return;
        }
        for entry in &mut self.history {
            if entry.url == url && entry.title != trimmed {
                entry.title = trimmed.to_string();
            }
        }
    }

    /// Returns unseen tabs (for `ConversationBrowserTabNotice`).
    pub fn unseen_tabs(&self) -> Vec<&BrowserTabEntry> {
        self.tabs
            .iter()
            .filter(|t| !self.noticed_tab_ids.contains(&t.id))
            .collect()
    }

    /// Marks all currently open tabs as noticed.
    pub fn mark_all_tabs_noticed(&mut self) {
        for tab in &self.tabs {
            self.noticed_tab_ids.insert(tab.id.clone());
        }
    }

    /// Cycles active tab by signed offset (+1 next, -1 previous, wrapping).
    pub fn select_offset(&mut self, offset: isize) -> Option<String> {
        if self.tabs.is_empty() {
            return None;
        }
        let len = self.tabs.len() as isize;
        let current = self.active_tab_index().unwrap_or(0) as isize;
        let next = ((current + offset) % len + len) % len;
        let id = self.tabs[next as usize].id.clone();
        self.activate_tab(&id);
        Some(id)
    }

    /// Selects tab by index or last (`Cmd+1..9`).
    pub fn select_index(&mut self, target: BrowserTabIndexTarget) -> Option<String> {
        if self.tabs.is_empty() {
            return None;
        }
        let idx = match target {
            BrowserTabIndexTarget::Index(i) => {
                if i >= self.tabs.len() {
                    return None;
                }
                i
            }
            BrowserTabIndexTarget::Last => self.tabs.len() - 1,
        };
        let id = self.tabs[idx].id.clone();
        self.activate_tab(&id);
        Some(id)
    }

    /// Moves the active tab by `offset` (`Ctrl+Shift+PgUp/PgDn`).
    pub fn move_active_tab(&mut self, offset: isize) -> bool {
        let Some(idx) = self.active_tab_index() else {
            return false;
        };
        let id = self.tabs[idx].id.clone();
        let next = (idx as isize + offset)
            .clamp(0, self.tabs.len().saturating_sub(1) as isize) as usize;
        self.move_tab(&id, next)
    }
}

// ============================================================================
// Floating BrowserDock & BrowserIsland State (`BrowserDock.tsx`, `BrowserIsland.tsx`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserIslandView {
    ActivityAgent {
        label: String,
    },
    ActivityUser,
    ActivityRequest {
        agent_name: String,
        reason: String,
    },
    Reply {
        id: String,
        body: String,
        is_question: bool,
        answer_in_chat: bool,
        shown_at_ms: u64,
    },
    Approval {
        request_id: String,
        title: String,
        detail: String,
    },
}

impl BrowserIslandView {
    pub fn needs_answer(&self) -> bool {
        match self {
            Self::Reply { is_question, .. } => *is_question,
            Self::Approval { .. } => true,
            _ => false,
        }
    }

    pub fn asks_for_page(&self) -> bool {
        matches!(self, Self::ActivityRequest { .. })
    }
}

/// Interactive state for the bottom floating `BrowserDock` pill inside `BrowserFullView`.
#[derive(Debug, Clone, Default)]
pub struct BrowserDockUiState {
    pub active_agent: BrowserTabOwner,
    pub draft: String,
    pub tucked: bool,
    pub folded_for_takeover: bool,
    pub working: bool,
    pub activity_label: Option<String>,
    pub latest_reply: Option<(String, String, bool, u64)>,
    pub dismissed_reply_ids: HashSet<String>,
    pub queued_messages: Vec<(String, String)>,
    pub pending_approval: Option<(String, String, String)>,
    pub reply_hovered: bool,
}

impl BrowserDockUiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes the active `BrowserIslandView` following OpenBot's exact priority order
    /// (`BrowserDockAdapter.tsx` lines 115-140):
    /// takeover request > user control > approval > agent acting > working > latest reply.
    pub fn resolve_island(
        &self,
        control_mode: &bot_browser::BrowserControlMode,
        now_ms: u64,
        is_zh: bool,
    ) -> Option<BrowserIslandView> {
        match control_mode {
            bot_browser::BrowserControlMode::WaitingForTakeover { reason } => {
                return Some(BrowserIslandView::ActivityRequest {
                    agent_name: self.active_agent.name.clone(),
                    reason: reason.clone(),
                });
            }
            bot_browser::BrowserControlMode::UserTakeover { .. } => {
                return Some(BrowserIslandView::ActivityUser);
            }
            bot_browser::BrowserControlMode::AiControlling { status_text, .. } => {
                let label = if status_text.trim().is_empty() {
                    browser_action_label("working", is_zh).to_string()
                } else {
                    status_text.clone()
                };
                return Some(BrowserIslandView::ActivityAgent { label });
            }
            bot_browser::BrowserControlMode::Idle => {}
        }

        if let Some((req_id, title, detail)) = &self.pending_approval {
            return Some(BrowserIslandView::Approval {
                request_id: req_id.clone(),
                title: title.clone(),
                detail: detail.clone(),
            });
        }

        if self.working {
            let label = self
                .activity_label
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    if is_zh {
                        "正在处理中…".to_string()
                    } else {
                        "Working on it…".to_string()
                    }
                });
            return Some(BrowserIslandView::ActivityAgent { label });
        }

        if let Some((id, body, is_question, shown_at_ms)) = &self.latest_reply {
            if !self.dismissed_reply_ids.contains(id) {
                let expired = !*is_question
                    && !self.reply_hovered
                    && now_ms.saturating_sub(*shown_at_ms) >= BROWSER_DOCK_REPLY_CLOSE_MS;
                if !expired {
                    return Some(BrowserIslandView::Reply {
                        id: id.clone(),
                        body: body.clone(),
                        is_question: *is_question,
                        answer_in_chat: true,
                        shown_at_ms: *shown_at_ms,
                    });
                }
            }
        }
        None
    }

    /// Synchronizes auto-tuck behavior (`BrowserDock.tsx` lines 163-188):
    /// - Auto-tucks when agent requests page interaction and draft is empty.
    /// - Re-opens automatically when takeover request finishes or answer is needed.
    pub fn reconcile_auto_tuck(&mut self, island: Option<&BrowserIslandView>) {
        if island.is_some_and(BrowserIslandView::needs_answer) {
            self.tucked = false;
            self.folded_for_takeover = false;
            return;
        }
        let asks = island.is_some_and(BrowserIslandView::asks_for_page);
        if asks {
            if !self.tucked && self.draft.trim().is_empty() {
                self.tucked = true;
                self.folded_for_takeover = true;
            }
        } else if self.folded_for_takeover {
            self.folded_for_takeover = false;
            self.tucked = false;
        }
    }

    /// Label rendered inside the tucked floating pill (`BrowserDock.tsx` `tabLabel`).
    pub fn tucked_label(&self, island: Option<&BrowserIslandView>, is_zh: bool) -> String {
        let name = &self.active_agent.name;
        match island {
            Some(BrowserIslandView::ActivityAgent { label }) => label.clone(),
            Some(BrowserIslandView::ActivityUser) => {
                if is_zh {
                    "你正在控制该页面".to_string()
                } else {
                    "You're controlling this page".to_string()
                }
            }
            Some(BrowserIslandView::ActivityRequest { .. }) => {
                if is_zh {
                    format!("{name} 需要你在该页面操作")
                } else {
                    format!("{name} needs you on this page")
                }
            }
            Some(BrowserIslandView::Approval { .. }) => {
                if is_zh {
                    format!("{name} 需要你的批准")
                } else {
                    format!("{name} needs your approval")
                }
            }
            Some(BrowserIslandView::Reply { is_question, .. }) => {
                if *is_question {
                    if is_zh {
                        format!("{name} 需要你的回答")
                    } else {
                        format!("{name} needs your answer")
                    }
                } else if is_zh {
                    format!("{name} 已回复")
                } else {
                    format!("{name} replied")
                }
            }
            None => {
                if is_zh {
                    format!("发消息给 {name}")
                } else {
                    format!("Message {name}")
                }
            }
        }
    }

    pub fn dismiss_reply(&mut self, id: &str) {
        self.dismissed_reply_ids.insert(id.to_string());
    }
}

// ============================================================================
// BrowserTeachBar State (`BrowserTeachBar.tsx`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowserTeachUiState {
    #[default]
    Inactive,
    Ready,
    Recording {
        started_at_ms: u64,
        step_count: usize,
    },
}

impl BrowserTeachUiState {
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Inactive)
    }

    pub fn is_recording(&self) -> bool {
        matches!(self, Self::Recording { .. })
    }

    /// Formats elapsed recording time as `m:ss` (`elapsedLabel` in `BrowserTeachBar.tsx`).
    pub fn elapsed_label(&self, now_ms: u64) -> String {
        let elapsed_ms = match self {
            Self::Recording { started_at_ms, .. } => now_ms.saturating_sub(*started_at_ms),
            _ => 0,
        };
        let seconds = elapsed_ms / 1_000;
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

/// Formats `BrowserTeachBar` steps into OpenBot's teach prompt (`browser-teach-message.ts`).
pub fn format_teach_message_for_chat(steps_summary: &[String], is_zh: bool) -> String {
    let header = if is_zh {
        "我在浏览器中向你演示了一项任务。请从录制内容学习这项任务，并将其保存为技能。然后用几句简单的话告诉我你学到了什么。"
    } else {
        "I demonstrated a task in the browser. Please learn this task from the recording and save it as a skill, then briefly tell me what you learned."
    };
    if steps_summary.is_empty() {
        return header.to_string();
    }
    let note = if is_zh {
        "录制内容不含输入的文字：请向我询问我输入的每个值。"
    } else {
        "The recording omits typed text: ask me for each value to fill in."
    };
    let numbered: Vec<String> = steps_summary
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {s}", i + 1))
        .collect();
    format!("{header}\n\n{note}\n{}", numbered.join("\n"))
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omnibox_url_resolution_and_display_match_openbot() {
        assert_eq!(browser_display_url("https://www.google.com/"), "google.com");
        assert_eq!(browser_display_url("http://example.org/docs/"), "example.org/docs");
        assert_eq!(browser_address_url("github.com/nightly-labs/openbot"), "https://github.com/nightly-labs/openbot");
        assert_eq!(browser_address_url("http://127.0.0.1:8080/home"), "http://127.0.0.1:8080/home");
        assert_eq!(browser_address_url("localhost:3000"), "https://localhost:3000");
        assert_eq!(
            browser_address_url("gpui rust browser"),
            "https://www.google.com/search?q=gpui%20rust%20browser"
        );
        assert_eq!(
            browser_search_query("https://www.google.com/search?q=hello+world&hl=zh"),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn omnibox_suggestions_rank_tabs_history_url_and_search() {
        let mut session = OpenBotBrowserSession::new_empty();
        let t1 = session
            .open_tab("https://www.google.com", None, None, "default", TabPlacement::default(), true)
            .unwrap();
        let t2 = session
            .open_tab("https://github.com/nightly-labs/openbot", None, None, "default", TabPlacement::default(), true)
            .unwrap();
        session.record_visit("https://rust-lang.org", "Rust");
        session.record_visit("https://rust-lang.org", "Rust");

        let comp = browser_completion("rust", &session.history);
        assert_eq!(comp.map(|c| c.url.as_str()), Some("https://rust-lang.org"));

        let rows = browser_suggestions("git", None, &session.tabs, Some(&t1), &session.history);
        assert!(rows.iter().any(|r| r.kind == BrowserSuggestionKind::Search));
        assert!(rows.iter().any(|r| r.kind == BrowserSuggestionKind::Tab && r.tab_id.as_deref() == Some(t2.as_str())));

        let empty_rows = browser_suggestions("", None, &session.tabs, Some(&t1), &session.history);
        assert_eq!(empty_rows.len(), 1);
        assert_eq!(empty_rows[0].url, "https://rust-lang.org");
    }

    #[test]
    fn multi_tab_session_order_duplicate_close_reopen_and_cap() {
        let mut session = OpenBotBrowserSession::new_empty();
        let t1 = session
            .open_tab("https://google.com", None, None, "default", TabPlacement::default(), true)
            .unwrap();
        let t2 = session
            .open_tab("https://example.com", None, None, "default", TabPlacement::default(), true)
            .unwrap();
        let t1_dup = session.duplicate_tab(&t1).unwrap();
        assert_eq!(session.tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec![t1.as_str(), t1_dup.as_str(), t2.as_str()]);

        assert!(session.move_tab(&t2, 0));
        assert_eq!(session.tabs[0].id, t2);

        assert_eq!(session.close_tabs(&[t1_dup.clone()]), 1);
        assert!(session.can_reopen());
        let reopened = session.reopen_closed_tab(None, None, "default").unwrap().unwrap();
        assert_eq!(session.tab_by_id(&reopened).unwrap().url, "https://google.com");

        // Enforce 25-tab cap
        while session.tabs.len() < BROWSER_MAX_TABS {
            session
                .open_tab("https://example.org", None, None, "default", TabPlacement::default(), true)
                .unwrap();
        }
        assert!(session
            .open_tab("https://overflow.example", None, None, "default", TabPlacement::default(), true)
            .is_err());
    }

    #[test]
    fn shortcuts_and_dock_auto_tuck_match_openbot() {
        let mac_cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_browser_view_command("t", mac_cmd, BrowserShortcutPlatform::Mac),
            Some(BrowserViewCommand::NewTab)
        );
        assert_eq!(
            resolve_browser_view_command(
                "t",
                Modifiers {
                    platform: true,
                    shift: true,
                    ..Default::default()
                },
                BrowserShortcutPlatform::Mac
            ),
            Some(BrowserViewCommand::Reopen)
        );
        assert_eq!(
            resolve_browser_view_command("9", mac_cmd, BrowserShortcutPlatform::Mac),
            Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Last))
        );

        let mut dock = BrowserDockUiState::new();
        let req = BrowserIslandView::ActivityRequest {
            agent_name: "New agent".into(),
            reason: "Captcha".into(),
        };
        dock.reconcile_auto_tuck(Some(&req));
        assert!(dock.tucked && dock.folded_for_takeover);
        dock.reconcile_auto_tuck(None);
        assert!(!dock.tucked && !dock.folded_for_takeover);
    }
}
