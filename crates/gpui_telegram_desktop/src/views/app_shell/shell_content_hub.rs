//! `AppShell` coordinator for Apple Content Hub, `AppleAppSidebar`, Cloud Creations sync,
//! `PublishReviewModal`, live WS updates, `@CreatorStudioBot` launch, and `tg://apple/*` deep-links.

use gpui::{
    div, px, AnyElement, AsyncApp, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, WeakEntity,
};
use gpui_telegram_client::net::http_client::HttpClientError;
use gpui_telegram_core::core::deep_links::parse_apple_deep_link;
use gpui_telegram_proto::contracts::chats::CreateChatRequest;
use gpui_telegram_proto::creations::{
    AppStoreMiniAppEntry, CatalogSortMode, CreationKind, CreationReviewStatus, CreationVisibility,
    ReviewDecision, ReviewDecisionRequest, SubmitReviewRequest, UpdateCreationRequest,
    UserCreationDto, CREATOR_STUDIO_BOT_USERNAME, CREATOR_STUDIO_BOT_USER_ID,
};
use gpui_telegram_proto::miniapp_contracts::MiniAppLaunchMode;

use crate::i18n::tr_str;
use crate::views::app_shell::modal_container::{macos_modal_backdrop_rgba, macos_modal_card};
use crate::views::app_shell::shell_truth::SessionRequestFence;
use crate::views::apple_sidebar::{AppleSidebarEvent, AppleSidebarSection};
use crate::views::content_hub::{
    render_publish_review_modal_body, render_quick_creations_drawer, ContentHubEvent,
    ContentHubInputTarget, ContentHubSection, PublishModalField, ReviewerAccessState,
};
use crate::views::nav_rail::NavDestination;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};

/// Per-domain monotonic request generation fences for Content Hub async RPCs.
#[derive(Debug, Clone, Default)]
pub struct ShellContentHubState {
    pub creations_request_gen: u64,
    pub books_catalog_gen: u64,
    pub music_catalog_gen: u64,
    pub games_catalog_gen: u64,
    pub appstore_catalog_gen: u64,
    pub review_queue_request_gen: u64,
    pub detail_request_gen: u64,
    pub audit_request_gen: u64,
    pub creator_bot_chat_in_flight: bool,
}

impl ShellContentHubState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bump_creations_gen(&mut self) -> u64 {
        self.creations_request_gen = self.creations_request_gen.wrapping_add(1);
        self.creations_request_gen
    }

    pub fn bump_catalog_gen_for(&mut self, section: ContentHubSection) -> u64 {
        let slot = match section {
            ContentHubSection::Books => &mut self.books_catalog_gen,
            ContentHubSection::Music => &mut self.music_catalog_gen,
            ContentHubSection::Games => &mut self.games_catalog_gen,
            ContentHubSection::AppStore => &mut self.appstore_catalog_gen,
            ContentHubSection::MyCreations => &mut self.creations_request_gen,
        };
        *slot = slot.wrapping_add(1);
        *slot
    }

    pub fn catalog_gen_for(&self, section: ContentHubSection) -> u64 {
        match section {
            ContentHubSection::Books => self.books_catalog_gen,
            ContentHubSection::Music => self.music_catalog_gen,
            ContentHubSection::Games => self.games_catalog_gen,
            ContentHubSection::AppStore => self.appstore_catalog_gen,
            ContentHubSection::MyCreations => self.creations_request_gen,
        }
    }

    pub fn bump_review_queue_gen(&mut self) -> u64 {
        self.review_queue_request_gen = self.review_queue_request_gen.wrapping_add(1);
        self.review_queue_request_gen
    }

    pub fn bump_detail_gen(&mut self) -> u64 {
        self.detail_request_gen = self.detail_request_gen.wrapping_add(1);
        self.detail_request_gen
    }

    pub fn bump_audit_gen(&mut self) -> u64 {
        self.audit_request_gen = self.audit_request_gen.wrapping_add(1);
        self.audit_request_gen
    }

    pub fn reset_generations(&mut self) {
        self.bump_creations_gen();
        self.bump_catalog_gen_for(ContentHubSection::Books);
        self.bump_catalog_gen_for(ContentHubSection::Music);
        self.bump_catalog_gen_for(ContentHubSection::Games);
        self.bump_catalog_gen_for(ContentHubSection::AppStore);
        self.bump_review_queue_gen();
        self.bump_detail_gen();
        self.bump_audit_gen();
        self.creator_bot_chat_in_flight = false;
    }
}

fn strip_ascii_prefix_ignore_case<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    if input.len() >= prefix.len()
        && input.is_char_boundary(prefix.len())
        && input[..prefix.len()].eq_ignore_ascii_case(prefix)
    {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

/// Unwraps AT MOST ONE optional leading case-insensitive `"url:"` prefix, trimming
/// ONLY ASCII spaces (`' '`) so control/invisible smuggling characters remain intact
/// and are rejected by `parse_apple_deep_link`, while double-wrapped `"url:url:..."`
/// fails parsing and is swallowed by `is_apple_deep_link_candidate`.
pub fn unwrap_apple_callback_uri(raw: &str) -> &str {
    let trimmed = raw.trim_matches(' ');
    if let Some(after) = strip_ascii_prefix_ignore_case(trimmed, "url:") {
        after.trim_matches(' ')
    } else {
        trimmed
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Leniently decodes valid `%XX` byte sequences (leaving malformed `%` literals intact)
/// and converts the resulting bytes via `String::from_utf8_lossy`.
pub(crate) fn lenient_percent_decode_once(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const MAX_PERCENT_DECODE_PASSES: usize = 16;

/// Collapses arbitrary-depth repeated `%25` chains (`%252525...61` -> `%2561`) in a single O(N)
/// scan so even 100x-nested percent-encoding converges in <= 2 decode passes with zero DoS risk.
/// Safe for UTF-8 because `%`, `2`, and `5` are single-byte ASCII (`< 0x80`).
pub(crate) fn collapse_repeated_percent25_chains(input: &str) -> String {
    if !input.contains("%25") {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 4 < bytes.len()
            && bytes[i + 1] == b'2'
            && bytes[i + 2] == b'5'
            && bytes[i + 3] == b'2'
            && bytes[i + 4] == b'5'
        {
            out.extend_from_slice(b"%25");
            i += 3;
            while i + 1 < bytes.len() && bytes[i] == b'2' && bytes[i + 1] == b'5' {
                i += 2;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn strip_candidate_noise(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            !c.is_control()
                && c != '\u{FFFD}'
                && !crate::views::content_hub::publish_review_modal::is_invisible_format_char(c)
        })
        .map(crate::views::content_hub::publish_review_modal::fold_fullwidth_ascii)
        .collect()
}

/// Matches `<expected_scheme><optional Unicode whitespace>:<optional Unicode whitespace>`
/// (also tolerating inline Unicode whitespace inside the scheme token, e.g. `"t\u{00A0}g ://"`),
/// strictly rejecting prefixes containing `/`, `\`, `?`, or `#` before `:`.
fn strip_scheme_colon_ignore_case<'a>(input: &'a str, expected_scheme: &str) -> Option<&'a str> {
    let (head, rest) = input.split_once(':')?;
    if head.contains('/') || head.contains('\\') || head.contains('?') || head.contains('#') {
        return None;
    }
    let mut iter = head.chars().filter(|c| !c.is_whitespace());
    for exp in expected_scheme.bytes() {
        let ch = iter.next()?;
        if !ch.is_ascii() || !(ch as u8).eq_ignore_ascii_case(&exp) {
            return None;
        }
    }
    if iter.next().is_some() {
        return None;
    }
    Some(rest.trim_start())
}

fn matches_apple_host_prefix(s: &str) -> bool {
    if let Some(rest) = strip_ascii_prefix_ignore_case(s, "apple") {
        return match rest.chars().next() {
            None => true,
            Some(ch) => !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-',
        };
    }
    false
}

fn matches_apple_deep_link_candidate_view(clean: &str) -> bool {
    let mut trimmed = clean.trim();
    while let Some(after_url_prefix) = strip_scheme_colon_ignore_case(trimmed, "url") {
        trimmed = after_url_prefix;
    }
    if let Some(after_creation) = strip_scheme_colon_ignore_case(trimmed, "creation") {
        if let Some(rest) = strip_ascii_prefix_ignore_case(after_creation, "open") {
            return match rest.chars().next() {
                None => true,
                Some(ch) => !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-',
            };
        }
        let seg_end = after_creation
            .find([':', '/', '\\', '?', '#'])
            .unwrap_or(after_creation.len());
        let compact_seg: String = after_creation[..seg_end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        return compact_seg.eq_ignore_ascii_case("open");
    }
    let Some(mut normalized) = strip_scheme_colon_ignore_case(trimmed, "tg") else {
        return false;
    };
    loop {
        normalized = normalized.trim_start();
        if let Some(after_scheme) = strip_scheme_colon_ignore_case(normalized, "tg") {
            normalized = after_scheme;
            continue;
        }
        let after_slash = normalized.trim_start_matches(['/', '\\']);
        if after_slash.len() != normalized.len() {
            normalized = after_slash;
            continue;
        }
        break;
    }
    // Extract RFC 3986 authority strictly before the first '/', '\', '?', or '#'.
    let auth_end = normalized
        .find(['/', '\\', '?', '#'])
        .unwrap_or(normalized.len());
    let authority = &normalized[..auth_end];
    // Strip RFC 3986 userinfo up to the last '@' strictly within the authority segment.
    let host_part = match authority.rsplit_once('@') {
        Some((_, after_at)) => after_at.trim_start(),
        None => authority.trim_start(),
    };
    if matches_apple_host_prefix(host_part) {
        return true;
    }
    if host_part.chars().any(char::is_whitespace) {
        let compact_host: String = host_part.chars().filter(|c| !c.is_whitespace()).collect();
        if matches_apple_host_prefix(&compact_host) {
            return true;
        }
    }
    false
}

/// Returns `true` if `uri` targets the Apple Content Hub deep-link namespace (`tg://apple...`
/// or `creation:open:...`), including malformed/XSS/BiDi/invisible/noncharacter/Fullwidth/
/// arbitrary-depth-percent/userinfo/whitespace/backslash/url:-smuggled payloads rejected by
/// `parse_apple_deep_link` so callers never fall through to outgoing chat messages, external
/// `open`, clipboard, MiniApp launch, or `MediaViewer`.
pub fn is_apple_deep_link_candidate(uri: &str) -> bool {
    let mut current = strip_candidate_noise(uri);
    if matches_apple_deep_link_candidate_view(&current) {
        return true;
    }
    for _ in 0..MAX_PERCENT_DECODE_PASSES {
        if !current.contains('%') {
            break;
        }
        let collapsed = collapse_repeated_percent25_chains(&current);
        let decoded = lenient_percent_decode_once(&collapsed);
        let next = strip_candidate_noise(&decoded);
        if next == current {
            break;
        }
        if matches_apple_deep_link_candidate_view(&next) {
            return true;
        }
        current = next;
    }
    false
}

fn build_hub_fence(shell: &AppShell, request_generation: u64) -> Option<SessionRequestFence> {
    if !shell.is_authenticated {
        return None;
    }
    let token = shell.session_sync.session_token.clone()?;
    let user_id = shell.session_sync.user_id?;
    if token.is_empty() || user_id == 0 {
        return None;
    }
    Some(SessionRequestFence {
        auth_generation: shell.session_sync.auth_generation,
        request_generation,
        token,
        user_id,
    })
}

/// Uniform session-identity fence (`is_authenticated`, `auth_generation`, non-empty `token`, `user_id`)
/// for mutations and DM creation callbacks (immune to concurrent read-generation bumps).
fn hub_session_fence_matches(fence: &SessionRequestFence, shell: &AppShell) -> bool {
    fence.matches(
        shell.is_authenticated,
        shell.session_sync.auth_generation,
        fence.request_generation,
        shell.session_sync.session_token.as_deref(),
        shell.session_sync.user_id,
    )
}

/// Full session + monotonic request-generation fence for latest-wins read/fetch callbacks.
fn hub_fence_matches(fence: &SessionRequestFence, shell: &AppShell, current_req_gen: u64) -> bool {
    fence.matches(
        shell.is_authenticated,
        shell.session_sync.auth_generation,
        current_req_gen,
        shell.session_sync.session_token.as_deref(),
        shell.session_sync.user_id,
    )
}

fn format_creation_mutation_error(lang: &str, err: HttpClientError) -> (String, Option<Vec<String>>) {
    match err {
        HttpClientError::Conflict(detail) => (
            format!(
                "{} ({})",
                tr_str(lang, "版本冲突，请先同步最新云端修订号", "Revision conflict (409)"),
                detail
            ),
            None,
        ),
        HttpClientError::Api {
            status: 422,
            message,
        } => {
            let errs = message
                .split("; ")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            (message, (!errs.is_empty()).then_some(errs))
        }
        other => (other.to_string(), None),
    }
}

impl AppShell {
    /// Synchronizes `AppleAppSidebar`'s active highlight to match `self.active_panel`.
    pub fn sync_apple_sidebar_to_active_panel(&mut self, cx: &mut Context<Self>) {
        let target = match self.active_panel {
            ActiveWorkspacePanel::ContentHub(sec) => {
                AppleSidebarSection::from_content_hub_section(sec)
            }
            _ => AppleSidebarSection::Chats,
        };
        self.apple_sidebar.update(cx, |sb, scx| {
            if sb.active_section != target {
                sb.set_active_section(target);
                scx.notify();
            }
        });
    }

    /// Synchronizes unread chat count, cloud creations count, and pending review count to `AppleAppSidebar`.
    pub fn sync_apple_sidebar_badges(&mut self, cx: &mut Context<Self>) {
        let unread = self.sidebar.read(cx).total_unread_badge();
        let (creations_count, pending_count) = {
            let h = self.content_hub.read(cx);
            (
                h.my_creations
                    .iter()
                    .filter(|c| c.deleted_at_ms.is_none())
                    .count(),
                h.publish_modal.review_queue.len(),
            )
        };
        self.apple_sidebar.update(cx, |sb, scx| {
            if sb.unread_chats_count != unread
                || sb.creations_count != creations_count
                || sb.pending_review_count != pending_count
            {
                sb.set_badges(unread, creations_count, pending_count);
                scx.notify();
            }
        });
    }

    /// Switches between `Chats` and any Apple Content Hub vertical without clearing chat state.
    pub fn select_apple_sidebar_section(
        &mut self,
        section: AppleSidebarSection,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        self.leave_workspace_modal_panel_for_hub(cx);
        self.dismiss_transient_popovers_for_hub(cx);
        match section.to_content_hub_section() {
            None => {
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.last_handled_nav_dest = Some(NavDestination::Chats);
                self.nav_rail.update(cx, |r, rcx| {
                    r.set_active_destination(NavDestination::Chats);
                    rcx.notify();
                });
                self.apple_sidebar.update(cx, |sb, scx| {
                    sb.set_active_section(AppleSidebarSection::Chats);
                    scx.notify();
                });
                self.content_hub.update(cx, |h, hcx| {
                    if h.active_input != ContentHubInputTarget::QuickDrawerSearch {
                        h.active_input = ContentHubInputTarget::None;
                    }
                    hcx.notify();
                });
                if self.composer.read(cx).is_compose_isolated
                    && !self.is_compose_or_creation_modal_active(cx)
                {
                    self.exit_compose_or_modal_isolation(true, cx);
                }
                cx.notify();
            }
            Some(hub_section) => {
                self.switch_to_content_hub(hub_section, cx);
            }
        }
    }

    /// Mounts `ContentHubWorkspace` at `section` while keeping `ChatSidebar`/`ConversationStage` intact.
    pub fn switch_to_content_hub(
        &mut self,
        section: ContentHubSection,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        self.leave_workspace_modal_panel_for_hub(cx);
        self.dismiss_transient_popovers_for_hub(cx);
        self.active_panel = ActiveWorkspacePanel::ContentHub(section);
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        }
        self.apple_sidebar.update(cx, |sb, scx| {
            sb.set_active_section(AppleSidebarSection::from_content_hub_section(section));
            scx.notify();
        });
        self.content_hub.update(cx, |h, hcx| {
            h.set_section(section);
            hcx.notify();
        });
        self.sync_local_miniapps_to_content_hub(cx);
        if section == ContentHubSection::MyCreations {
            self.refresh_my_creations(cx);
            if self.content_hub.read(cx).publish_modal.reviewer_access
                == ReviewerAccessState::Unknown
            {
                self.probe_reviewer_queue(cx, false);
            }
        } else {
            self.refresh_content_hub_catalog(section, cx);
        }
        cx.notify();
    }

    pub(crate) fn leave_workspace_modal_panel_for_hub(&mut self, cx: &mut Context<Self>) {
        self.cleanup_settings_panel_if_active(cx);
        if matches!(
            self.active_panel,
            ActiveWorkspacePanel::Settings
                | ActiveWorkspacePanel::Calls
                | ActiveWorkspacePanel::TaskCenter
                | ActiveWorkspacePanel::McpTools
        ) || self.nav_rail.read(cx).active_destination != NavDestination::Chats
        {
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            let nav_trigger = self.nav_rail.update(cx, |r, rcx| {
                if r.active_destination != NavDestination::Chats {
                    r.set_active_destination(NavDestination::Chats);
                    rcx.notify();
                }
                r.trigger_counter
            });
            self.last_handled_nav_trigger = nav_trigger;
        }
    }

    fn dismiss_transient_popovers_for_hub(&mut self, cx: &mut Context<Self>) {
        self.clear_all_ime_preedits(cx);
        self.main_menu_drawer.update(cx, |m, mcx| {
            if m.is_open {
                m.close();
                mcx.notify();
            }
        });
        self.sidebar.update(cx, |sb, scx| {
            let changed = sb.is_plus_menu_open
                || !sb.compose_recipient_query.is_empty()
                || sb.compose_title_focused
                || sb.is_account_popover_open
                || sb.search_focused;
            sb.exit_compose_mode();
            sb.is_account_popover_open = false;
            sb.search_focused = false;
            if changed {
                scx.notify();
            }
        });
        self.stage.update(cx, |st, stcx| {
            let mut changed = st.more_menu_open
                || st.ttl_submenu_open
                || st.search_open
                || st.active_bot_card_field.is_some();
            st.more_menu_open = false;
            st.ttl_submenu_open = false;
            st.search_open = false;
            if st.active_bot_card_field.is_some() {
                st.blur_bot_card_input();
                changed = true;
            }
            if changed {
                stcx.notify();
            }
        });
        self.inspector.update(cx, |ins, icx| {
            if ins.is_bot_input_active() {
                ins.blur_bot_input();
                icx.notify();
            }
        });
        self.composer.update(cx, |c, ccx| {
            let mut changed = c.is_input_focused || c.ime_machine.is_composing();
            c.is_input_focused = false;
            if c.ime_machine.is_composing() {
                c.ime_machine.on_ime_cancel();
                c.sync_autocomplete();
            }
            for _ in 0..3 {
                if !c.handle_hermes_autocomplete_nav("escape", false) {
                    break;
                }
                changed = true;
            }
            if changed {
                ccx.notify();
            }
        });
    }

    /// Toggles the sidebar-adjacent Quick Creations Drawer (works over Chats AND ContentHub).
    pub fn toggle_quick_creations_drawer(&mut self, open: bool, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let drawer_owned_ime = !open
            && self.is_content_hub_input_active(cx)
            && self.content_hub.read(cx).active_input == ContentHubInputTarget::QuickDrawerSearch;
        self.apple_sidebar.update(cx, |sb, scx| {
            sb.set_creations_drawer_open(open);
            scx.notify();
        });
        let had_drawer_preedit = self.content_hub.update(cx, |h, hcx| {
            let had_preedit = h.quick_drawer_search.ime_preedit.is_some();
            h.quick_drawer_open = open;
            if !open {
                h.quick_drawer_search.set_ime_preedit(None);
                if h.active_input == ContentHubInputTarget::QuickDrawerSearch {
                    h.active_input = ContentHubInputTarget::None;
                }
            }
            hcx.notify();
            !open && had_preedit
        });
        if drawer_owned_ime
            || (had_drawer_preedit && !self.has_active_non_composer_ime_owner(cx))
        {
            self.clear_all_ime_preedits(cx);
        }
        if open {
            self.refresh_my_creations(cx);
        }
        cx.notify();
    }

    /// Opens the "推送审核上架" modal sheet for a creation DTO.
    pub fn open_publish_review_modal_for_creation(
        &mut self,
        creation: UserCreationDto,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        self.dismiss_transient_popovers_for_hub(cx);
        self.content_hub.update(cx, |h, hcx| {
            h.open_publish_modal_for_creation(creation, hcx);
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        }
        cx.notify();
    }

    /// Opens `PublishReviewModal` for a cached creation ID if present in `my_creations`.
    pub fn open_publish_review_modal_for_id(
        &mut self,
        creation_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return false;
        }
        let found = self
            .content_hub
            .read(cx)
            .find_my_creation(creation_id)
            .cloned();
        if let Some(creation) = found {
            self.open_publish_review_modal_for_creation(creation, cx);
            true
        } else {
            false
        }
    }

    /// Opens the Reviewer Queue tab inside `PublishReviewModal`.
    pub fn open_reviewer_queue_modal(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        self.dismiss_transient_popovers_for_hub(cx);
        self.content_hub.update(cx, |h, hcx| {
            h.open_reviewer_queue(hcx);
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        }
        cx.notify();
    }

    /// Closes `PublishReviewModal` without altering `self.active_panel`.
    pub fn close_publish_review_modal(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let should_clear_ime = self.content_hub.update(cx, |h, hcx| {
            let was_open = h.publish_modal.is_open;
            let pending_clear = std::mem::take(&mut h.publish_modal.pending_close_ime_clear);
            if was_open {
                h.publish_modal.close();
                h.publish_modal.pending_close_ime_clear = false;
            }
            if was_open || pending_clear {
                hcx.notify();
            }
            was_open || pending_clear
        });
        if should_clear_ime && self.overlays.computer_overlay.is_none() {
            self.clear_all_ime_preedits(cx);
        }
        if should_clear_ime {
            cx.notify();
        }
    }

    /// Handles events emitted by `AppleAppSidebar`.
    pub fn handle_apple_sidebar_event(&mut self, ev: &AppleSidebarEvent, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        match ev {
            AppleSidebarEvent::SelectSection(sec) => {
                self.select_apple_sidebar_section(*sec, cx);
            }
            AppleSidebarEvent::ToggleCreationsDrawer(open) => {
                self.toggle_quick_creations_drawer(*open, cx);
            }
            AppleSidebarEvent::SelectCreationKindFilter(kind_opt) => {
                self.content_hub.update(cx, |h, hcx| {
                    h.creations_kind_filter = *kind_opt;
                    h.quick_drawer_kind_filter = *kind_opt;
                    hcx.notify();
                });
                if !self.content_hub.read(cx).quick_drawer_open
                    && self.active_panel
                        != ActiveWorkspacePanel::ContentHub(ContentHubSection::MyCreations)
                {
                    self.toggle_quick_creations_drawer(true, cx);
                }
                cx.notify();
            }
            AppleSidebarEvent::ToggleCollapsed(_) => {
                cx.notify();
            }
            AppleSidebarEvent::OpenCreatorBot(kind_opt) => {
                let cmd = kind_opt.map(|k| match k {
                    CreationKind::Book => "/newbook ",
                    CreationKind::Music => "/newmusic ",
                    CreationKind::Game => "/newgame ",
                    CreationKind::App => "/mycreations",
                });
                self.open_creator_studio_bot_chat(cmd, cx);
            }
        }
    }

    /// Handles events emitted by `ContentHubWorkspace`.
    pub fn handle_content_hub_event(&mut self, ev: &ContentHubEvent, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        match ev {
            ContentHubEvent::RefreshMyCreations => self.refresh_my_creations(cx),
            ContentHubEvent::RefreshCatalog(sec) => self.refresh_content_hub_catalog(*sec, cx),
            ContentHubEvent::RefreshReviewQueue => self.probe_reviewer_queue(cx, true),
            ContentHubEvent::OpenCreatorBot(kind_opt) => {
                let cmd = kind_opt.map(|k| match k {
                    CreationKind::Book => "/newbook ",
                    CreationKind::Music => "/newmusic ",
                    CreationKind::Game => "/newgame ",
                    CreationKind::App => "/mycreations",
                });
                self.open_creator_studio_bot_chat(cmd, cx);
            }
            ContentHubEvent::OpenCreatorBotWithCommand(cmd) => {
                self.open_creator_studio_bot_chat(Some(cmd.as_str()), cx);
            }
            ContentHubEvent::OpenCreationDetail {
                section,
                creation_id,
                action,
            } => {
                self.leave_workspace_modal_panel_for_hub(cx);
                self.dismiss_transient_popovers_for_hub(cx);
                self.active_panel = ActiveWorkspacePanel::ContentHub(*section);
                if self.composer.read(cx).is_compose_isolated
                    && !self.is_compose_or_creation_modal_active(cx)
                {
                    self.exit_compose_or_modal_isolation(true, cx);
                }
                self.sync_apple_sidebar_to_active_panel(cx);
                self.content_hub.update(cx, |h, hcx| {
                    h.blur_input();
                    h.set_section(*section);
                    h.selected_creation_id = Some(creation_id.clone());
                    h.active_action_hint = action.clone();
                    if h.selected_catalog_item
                        .as_ref()
                        .is_none_or(|c| c.creation_id != *creation_id)
                    {
                        h.selected_catalog_item = h.find_catalog_item(creation_id).cloned();
                    }
                    if h.selected_detail_creation
                        .as_ref()
                        .is_none_or(|d| d.creation_id != *creation_id)
                    {
                        h.selected_detail_creation = h.find_my_creation(creation_id).cloned();
                    }
                    hcx.notify();
                });
                self.fetch_creation_detail_fenced(creation_id.clone(), cx);
                cx.notify();
            }
            ContentHubEvent::SaveCreationDraft { creation_id, req } => {
                self.save_creation_draft_fenced(creation_id.clone(), req.clone(), cx);
            }
            ContentHubEvent::SubmitCreationForReview {
                creation_id,
                update_req,
                submit_req,
            } => {
                self.submit_creation_for_review_fenced(
                    creation_id.clone(),
                    update_req.clone(),
                    submit_req.clone(),
                    cx,
                );
            }
            ContentHubEvent::WithdrawCreationReview { creation_id } => {
                self.withdraw_creation_review_fenced(creation_id.clone(), cx);
            }
            ContentHubEvent::UnpublishCreation { creation_id } => {
                self.unpublish_creation_fenced(creation_id.clone(), cx);
            }
            ContentHubEvent::DeleteCreation { creation_id } => {
                self.delete_creation_fenced(creation_id.clone(), cx);
            }
            ContentHubEvent::LoadCreationAudit { creation_id } => {
                self.fetch_creation_audit_fenced(creation_id.clone(), cx);
            }
            ContentHubEvent::SubmitReviewDecision { creation_id, req } => {
                self.submit_review_decision_fenced(creation_id.clone(), req.clone(), cx);
            }
            ContentHubEvent::LaunchMiniApp {
                bot_username,
                short_name,
                web_url,
            } => {
                if web_url
                    .as_deref()
                    .is_some_and(|u| self.try_handle_or_swallow_apple_deep_link(u, false, cx))
                {
                    return;
                }
                self.clear_ime_and_blur_content_hub_for_transition(true, cx);
                if let Some(url) = web_url.as_deref().filter(|u| !u.trim().is_empty()) {
                    self.launch_miniapp_with_url(
                        bot_username,
                        short_name,
                        None,
                        MiniAppLaunchMode::DirectLink,
                        None,
                        Some(url.to_string()),
                        cx,
                    );
                } else {
                    self.launch_miniapp(
                        bot_username,
                        short_name,
                        None,
                        MiniAppLaunchMode::DirectLink,
                        None,
                        cx,
                    );
                }
            }
            ContentHubEvent::CloseQuickDrawer => {
                self.toggle_quick_creations_drawer(false, cx);
            }
            ContentHubEvent::ClosePublishReviewModal => {
                self.close_publish_review_modal(cx);
            }
            ContentHubEvent::NavigateSection(sec) => {
                self.switch_to_content_hub(*sec, cx);
            }
            ContentHubEvent::FocusInputTaken => {
                let hub_focus_unoccluded = if self.is_publish_review_modal_active(cx) {
                    self.overlays.computer_overlay.is_none()
                } else if matches!(self.active_panel, ActiveWorkspacePanel::ContentHub(_)) {
                    self.is_content_hub_input_active(cx)
                } else {
                    self.is_chats_inline_unoccluded(cx)
                        && self.content_hub.read(cx).has_focused_input(false)
                };
                if !hub_focus_unoccluded {
                    return;
                }
                self.clear_all_ime_preedits(cx);
                self.blur_composer_and_cancel_ime(cx);
                self.sidebar.update(cx, |sb, scx| {
                    if sb.search_focused {
                        sb.search_focused = false;
                        scx.notify();
                    }
                });
                self.stage.update(cx, |st, stcx| {
                    let mut changed = false;
                    if st.active_bot_card_field.is_some() {
                        st.blur_bot_card_input();
                        changed = true;
                    }
                    if st.search_open {
                        st.search_open = false;
                        changed = true;
                    }
                    if changed {
                        stcx.notify();
                    }
                });
                self.inspector.update(cx, |ins, icx| {
                    if ins.is_bot_input_active() {
                        ins.blur_bot_input();
                        icx.notify();
                    }
                });
                cx.notify();
            }
        }
    }

    /// Renders the sidebar-adjacent Quick Creations Drawer sibling pane.
    pub fn render_quick_creations_drawer_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        render_quick_creations_drawer(self.content_hub.clone(), cx)
    }

    /// Renders the centered `PublishReviewModal` sheet overlay without forcing `Chats` on backdrop close.
    pub fn render_publish_review_modal_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let modal_w = pal.scaled_px(640.0).min(780.0);
        let modal_h = pal.scaled_px(680.0).min(760.0);
        div()
            .id("publish-review-modal-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(macos_modal_backdrop_rgba(pal))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_publish_review_modal(cx)),
            )
            .child(
                macos_modal_card("publish-review-modal-box", pal)
                    .w(px(modal_w))
                    .h(px(modal_h))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(render_publish_review_modal_body(
                        self.content_hub.clone(),
                        cx,
                    )),
            )
            .into_any_element()
    }
}

impl AppShell {
    fn sync_local_miniapps_to_content_hub(&mut self, cx: &mut Context<Self>) {
        let fallback: Vec<AppStoreMiniAppEntry> = self
            .miniapps
            .catalog_miniapps
            .iter()
            .map(|m| AppStoreMiniAppEntry {
                bot_user_id: m.bot_user_id,
                bot_username: m.bot_username.clone(),
                short_name: m.short_name.clone(),
                title: m.title.clone(),
                description: m.description.clone(),
                web_url: m.web_url.clone(),
                icon_url: None,
                deep_link: format!("tg://miniapp/{}/{}", m.bot_username, m.short_name),
            })
            .collect();
        self.content_hub.update(cx, |h, _| {
            h.local_miniapps_fallback = fallback;
        });
    }

    /// Hydrates cached cloud creations/catalog synchronously on login and launches fenced network syncs.
    pub fn refresh_content_hub_on_login(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            return;
        }
        self.content_hub_state.creator_bot_chat_in_flight = false;
        let cached_creations = self.client.load_cached_my_creations(None, None);
        let cached_states = self.client.load_cached_my_creation_states();
        let cached_books = self
            .client
            .load_cached_public_catalog(Some(CreationKind::Book));
        let cached_music = self
            .client
            .load_cached_public_catalog(Some(CreationKind::Music));
        let cached_games = self
            .client
            .load_cached_public_catalog(Some(CreationKind::Game));
        let cached_apps = self
            .client
            .load_cached_public_catalog(Some(CreationKind::App));
        self.content_hub.update(cx, |h, hcx| {
            if !cached_creations.is_empty() {
                h.set_my_creations(cached_creations);
            }
            for st in cached_states {
                h.my_creation_states.insert(st.creation_id.clone(), st);
            }
            if !cached_books.is_empty() {
                h.books_catalog = cached_books;
            }
            if !cached_music.is_empty() {
                h.music_catalog = cached_music;
            }
            if !cached_games.is_empty() {
                h.games_catalog = cached_games;
            }
            if !cached_apps.is_empty() {
                h.appstore_catalog.apps = cached_apps;
            }
            hcx.notify();
        });
        self.sync_local_miniapps_to_content_hub(cx);
        self.sync_apple_sidebar_badges(cx);
        self.refresh_my_creations(cx);
        self.probe_reviewer_queue(cx, false);
    }

    /// Runs a fenced cloud delta sync for the current user's creations and creation states.
    pub fn refresh_my_creations(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            return;
        }
        let req_gen = self.content_hub_state.bump_creations_gen();
        let Some(fence) = build_hub_fence(self, req_gen) else {
            return;
        };
        self.content_hub.update(cx, |h, hcx| {
            h.is_loading_creations = true;
            hcx.notify();
        });
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let creations_res = client.sync_my_creations_with_cache().await;
            let states_res = client.sync_my_creation_states_with_cache(None).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_fence_matches(&fence, shell, shell.content_hub_state.creations_request_gen)
                {
                    return;
                }
                let modal_closed_by_delete = shell.content_hub.update(cx, |h, hcx| {
                    let was_modal_open = h.publish_modal.is_open;
                    h.is_loading_creations = false;
                    match creations_res {
                        Ok(items) => {
                            h.set_my_creations(items);
                            h.error_message = None;
                        }
                        Err(err) => {
                            h.error_message = Some(err.to_string());
                        }
                    }
                    if let Ok(states) = states_res {
                        h.my_creation_states.clear();
                        for st in states {
                            h.my_creation_states.insert(st.creation_id.clone(), st);
                        }
                    }
                    hcx.notify();
                    was_modal_open && !h.publish_modal.is_open
                });
                if modal_closed_by_delete
                    && !shell.overlays.is_passcode_locked
                    && shell.overlays.computer_overlay.is_none()
                {
                    shell.clear_all_ime_preedits(cx);
                }
                shell.sync_apple_sidebar_badges(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Refreshes the public catalog feed for `section` using `SessionRequestFence`.
    pub fn refresh_content_hub_catalog(
        &mut self,
        section: ContentHubSection,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated {
            return;
        }
        if section == ContentHubSection::MyCreations {
            self.refresh_my_creations(cx);
            return;
        }
        let req_gen = self.content_hub_state.bump_catalog_gen_for(section);
        let Some(fence) = build_hub_fence(self, req_gen) else {
            return;
        };
        let is_active_section = self.active_panel == ActiveWorkspacePanel::ContentHub(section);
        let (query_opt, sort_mode) = if is_active_section {
            let h = self.content_hub.read(cx);
            let q = h.catalog_search.text.trim().to_string();
            ((!q.is_empty()).then_some(q), h.sort_mode)
        } else {
            // Background WS or post-approval refresh of an inactive vertical MUST NOT inherit
            // another vertical's search query or flip the active vertical's loading spinner.
            (None, CatalogSortMode::Featured)
        };
        if is_active_section {
            self.content_hub.update(cx, |h, hcx| {
                h.is_loading_catalog = true;
                hcx.notify();
            });
        }
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let q_ref = query_opt.as_deref();
            if section == ContentHubSection::AppStore {
                let res = client
                    .list_catalog_appstore(q_ref, None, Some(sort_mode), Some(50), Some(0))
                    .await;
                let _ = weak.update(app, |shell, cx| {
                    if !hub_fence_matches(
                        &fence,
                        shell,
                        shell.content_hub_state.catalog_gen_for(section),
                    ) {
                        return;
                    }
                    let still_active =
                        shell.active_panel == ActiveWorkspacePanel::ContentHub(section);
                    shell.content_hub.update(cx, |h, hcx| {
                        if still_active {
                            h.is_loading_catalog = false;
                        }
                        if let Ok(resp) = res {
                            h.appstore_catalog = resp;
                            if let Some(sel_id) = h.selected_creation_id.clone() {
                                if let Some(found) = h.find_catalog_item(&sel_id).cloned() {
                                    h.selected_catalog_item = Some(found);
                                }
                            }
                            if still_active {
                                h.error_message = None;
                            }
                        }
                        hcx.notify();
                    });
                    cx.notify();
                });
            } else {
                let kind = section.creation_kind();
                let res = client
                    .sync_public_catalog_with_cache(
                        kind,
                        q_ref,
                        None,
                        Some(sort_mode),
                        Some(50),
                        Some(0),
                    )
                    .await;
                let _ = weak.update(app, |shell, cx| {
                    if !hub_fence_matches(
                        &fence,
                        shell,
                        shell.content_hub_state.catalog_gen_for(section),
                    ) {
                        return;
                    }
                    let still_active =
                        shell.active_panel == ActiveWorkspacePanel::ContentHub(section);
                    shell.content_hub.update(cx, |h, hcx| {
                        if still_active {
                            h.is_loading_catalog = false;
                        }
                        if let Ok(items) = res {
                            match section {
                                ContentHubSection::Books => h.books_catalog = items,
                                ContentHubSection::Music => h.music_catalog = items,
                                ContentHubSection::Games => h.games_catalog = items,
                                _ => {}
                            }
                            if let Some(sel_id) = h.selected_creation_id.clone() {
                                if let Some(found) = h.find_catalog_item(&sel_id).cloned() {
                                    h.selected_catalog_item = Some(found);
                                }
                            }
                            if still_active {
                                h.error_message = None;
                            }
                        }
                        hcx.notify();
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Probes `/api/v1/creations/review-queue` (silently setting `Forbidden` on HTTP 403).
    pub fn probe_reviewer_queue(&mut self, cx: &mut Context<Self>, user_initiated: bool) {
        if !self.is_authenticated {
            return;
        }
        let req_gen = self.content_hub_state.bump_review_queue_gen();
        let Some(fence) = build_hub_fence(self, req_gen) else {
            return;
        };
        self.content_hub.update(cx, |h, hcx| {
            h.publish_modal.is_loading_queue = true;
            hcx.notify();
        });
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client
                .list_review_queue(Some(CreationReviewStatus::PendingReview), None)
                .await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_fence_matches(
                    &fence,
                    shell,
                    shell.content_hub_state.review_queue_request_gen,
                ) {
                    return;
                }
                let mut toast_err: Option<String> = None;
                let switched_field = shell.content_hub.update(cx, |h, hcx| {
                    let mut switched = false;
                    h.publish_modal.is_loading_queue = false;
                    match res {
                        Ok(list) => {
                            h.publish_modal.reviewer_access = ReviewerAccessState::Allowed;
                            h.apply_review_queue_snapshot(list.items);
                            h.publish_modal.reviewer_error = None;
                            if user_initiated
                                && h.publish_modal.is_open
                                && h.publish_modal.pending_open_reviewer_queue
                            {
                                h.publish_modal.open_reviewer_queue();
                                switched = true;
                            }
                        }
                        Err(HttpClientError::Api { status: 403, .. }) => {
                            h.publish_modal.reviewer_access = ReviewerAccessState::Forbidden;
                            h.publish_modal.review_queue.clear();
                            h.publish_modal.selected_review_id = None;
                            h.publish_modal.show_reviewer_queue_tab = false;
                            h.publish_modal.pending_open_reviewer_queue = false;
                            h.publish_modal.reviewer_error = None;
                            if h.publish_modal.focused_field
                                == Some(PublishModalField::RejectReason)
                            {
                                h.publish_modal.clear_all_preedits();
                                h.publish_modal.focused_field = Some(PublishModalField::Title);
                                switched = h.publish_modal.is_open;
                            }
                        }
                        Err(err) => {
                            if user_initiated {
                                let msg = err.to_string();
                                h.publish_modal.reviewer_error = Some(msg.clone());
                                toast_err = Some(msg);
                            }
                        }
                    }
                    hcx.notify();
                    switched
                });
                if switched_field
                    && !shell.overlays.is_passcode_locked
                    && shell.overlays.computer_overlay.is_none()
                {
                    shell.clear_all_ime_preedits(cx);
                }
                shell.sync_apple_sidebar_badges(cx);
                if let Some(msg) = toast_err {
                    shell.show_toast(msg, 2600, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn fetch_creation_detail_fenced(&mut self, creation_id: String, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let req_gen = self.content_hub_state.bump_detail_gen();
        let Some(fence) = build_hub_fence(self, req_gen) else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.get_creation(&creation_id).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_fence_matches(&fence, shell, shell.content_hub_state.detail_request_gen) {
                    return;
                }
                if let Ok(dto) = res {
                    let is_owner =
                        Some(dto.owner_user_id) == shell.session_sync.user_id.map(|u| u as i64);
                    let modal_closed_by_delete = shell.content_hub.update(cx, |h, hcx| {
                        let was_modal_open = h.publish_modal.is_open;
                        if dto.deleted_at_ms.is_some() {
                            h.evict_public_catalog_item(&dto.creation_id);
                        }
                        if is_owner {
                            h.upsert_my_creation(dto);
                        } else if dto.deleted_at_ms.is_some() {
                            h.record_deleted_creation(&dto.creation_id, dto.cloud_revision);
                            h.publish_modal.prune_review_queue_item(&dto.creation_id);
                            if h.selected_creation_id.as_deref() == Some(dto.creation_id.as_str()) {
                                h.selected_creation_id = None;
                            }
                            if h.selected_detail_creation
                                .as_ref()
                                .is_some_and(|d| d.creation_id == dto.creation_id)
                            {
                                h.selected_detail_creation = None;
                            }
                        } else if !h.is_deleted_at_or_above(&dto.creation_id, dto.cloud_revision) {
                            if dto.review_status != CreationReviewStatus::PendingReview {
                                let stale_for_queue = h
                                    .publish_modal
                                    .review_queue
                                    .iter()
                                    .find(|q| q.creation_id == dto.creation_id)
                                    .is_some_and(|q| dto.cloud_revision < q.cloud_revision);
                                if !stale_for_queue {
                                    h.record_resolved_review(&dto.creation_id, dto.cloud_revision);
                                    h.publish_modal.prune_review_queue_item(&dto.creation_id);
                                }
                            }
                            if h.selected_creation_id.as_deref() == Some(dto.creation_id.as_str())
                                && h.selected_detail_creation.as_ref().is_none_or(|cur| {
                                    cur.creation_id != dto.creation_id
                                        || dto.cloud_revision >= cur.cloud_revision
                                })
                            {
                                h.selected_detail_creation = Some(dto);
                            }
                        }
                        hcx.notify();
                        was_modal_open && !h.publish_modal.is_open
                    });
                    if modal_closed_by_delete
                        && !shell.overlays.is_passcode_locked
                        && shell.overlays.computer_overlay.is_none()
                    {
                        shell.clear_all_ime_preedits(cx);
                    }
                    shell.sync_apple_sidebar_badges(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn fetch_creation_audit_fenced(&mut self, creation_id: String, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let req_gen = self.content_hub_state.bump_audit_gen();
        let Some(fence) = build_hub_fence(self, req_gen) else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.get_creation_audit(&creation_id).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_fence_matches(&fence, shell, shell.content_hub_state.audit_request_gen) {
                    return;
                }
                if let Ok(audit) = res {
                    shell.content_hub.update(cx, |h, hcx| {
                        if h.publish_modal.target_creation_id.as_deref()
                            == Some(creation_id.as_str())
                        {
                            h.publish_modal.audit_trail = audit.items;
                            hcx.notify();
                        }
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn bump_catalog_fences_for_kind(&mut self, kind: CreationKind) -> bool {
        let vertical = ContentHubSection::from_creation_kind(kind);
        self.content_hub_state.bump_catalog_gen_for(vertical);
        let mut bumped_active = self.active_panel == ActiveWorkspacePanel::ContentHub(vertical);
        if vertical != ContentHubSection::AppStore {
            self.content_hub_state
                .bump_catalog_gen_for(ContentHubSection::AppStore);
            bumped_active |=
                self.active_panel == ActiveWorkspacePanel::ContentHub(ContentHubSection::AppStore);
        }
        bumped_active
    }

    pub fn save_creation_draft_fenced(
        &mut self,
        creation_id: String,
        req: UpdateCreationRequest,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let req_gen = self.content_hub_state.creations_request_gen;
        let Some(fence) = build_hub_fence(self, req_gen) else {
            self.content_hub.update(cx, |h, hcx| {
                if h.publish_modal.target_creation_id.as_deref() == Some(creation_id.as_str()) {
                    h.publish_modal.is_submitting = false;
                    hcx.notify();
                }
            });
            return;
        };
        let client = self.client.clone();
        let lang = self.active_lang_id().to_string();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.update_creation(&creation_id, &req).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                match res {
                    Ok(updated) => {
                        let cid = updated.creation_id.clone();
                        let (was_selected, was_in_review_queue) = {
                            let h = shell.content_hub.read(cx);
                            let selected = h.selected_creation_id.as_deref() == Some(cid.as_str())
                                || h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == cid);
                            let in_queue = h
                                .publish_modal
                                .review_queue
                                .iter()
                                .any(|q| q.creation_id == cid);
                            (selected, in_queue)
                        };
                        shell.content_hub_state.bump_creations_gen();
                        if was_in_review_queue {
                            shell.content_hub_state.bump_review_queue_gen();
                        }
                        if was_selected {
                            shell.content_hub_state.bump_detail_gen();
                        }
                        shell.content_hub.update(cx, |h, hcx| {
                            h.is_loading_creations = false;
                            if was_in_review_queue {
                                h.publish_modal.is_loading_queue = false;
                            }
                            h.upsert_my_creation(updated);
                            let same_modal = h.publish_modal.is_open
                                && h.publish_modal.target_creation_id.as_deref()
                                    == Some(creation_id.as_str());
                            if same_modal {
                                h.publish_modal.is_submitting = false;
                                h.publish_modal.server_error = None;
                                h.publish_modal.status_notice = Some(
                                    tr_str(&lang, "云端草稿已保存", "Cloud draft saved")
                                        .to_string(),
                                );
                            }
                            hcx.notify();
                        });
                        shell.sync_apple_sidebar_badges(cx);
                    }
                    Err(err) => {
                        let (msg, parsed_errs) = format_creation_mutation_error(&lang, err);
                        shell.content_hub.update(cx, |h, hcx| {
                            let same_modal = h.publish_modal.is_open
                                && h.publish_modal.target_creation_id.as_deref()
                                    == Some(creation_id.as_str());
                            if same_modal {
                                h.publish_modal.is_submitting = false;
                                h.publish_modal.server_error = Some(msg.clone());
                                if let Some(errs) = parsed_errs {
                                    h.publish_modal.validation_errors = errs;
                                }
                                hcx.notify();
                            }
                        });
                        shell.show_toast(msg, 3000, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn submit_creation_for_review_fenced(
        &mut self,
        creation_id: String,
        update_req: Option<UpdateCreationRequest>,
        submit_req: SubmitReviewRequest,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let req_gen = self.content_hub_state.creations_request_gen;
        let Some(fence) = build_hub_fence(self, req_gen) else {
            self.content_hub.update(cx, |h, hcx| {
                if h.publish_modal.target_creation_id.as_deref() == Some(creation_id.as_str()) {
                    h.publish_modal.is_submitting = false;
                    hcx.notify();
                }
            });
            return;
        };
        let client = self.client.clone();
        let lang = self.active_lang_id().to_string();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let mut updated_draft: Option<UserCreationDto> = None;
            if let Some(upd) = update_req {
                match client.update_creation(&creation_id, &upd).await {
                    Ok(dto) => {
                        updated_draft = Some(dto);
                    }
                    Err(err) => {
                        let _ = weak.update(app, |shell, cx| {
                            if !hub_session_fence_matches(&fence, shell) {
                                return;
                            }
                            let (msg, parsed_errs) = format_creation_mutation_error(&lang, err);
                            shell.content_hub.update(cx, |h, hcx| {
                                let same_modal = h.publish_modal.is_open
                                    && h.publish_modal.target_creation_id.as_deref()
                                        == Some(creation_id.as_str());
                                if same_modal {
                                    h.publish_modal.is_submitting = false;
                                    h.publish_modal.server_error = Some(msg.clone());
                                    if let Some(errs) = parsed_errs {
                                        h.publish_modal.validation_errors = errs;
                                    }
                                    hcx.notify();
                                }
                            });
                            shell.show_toast(msg, 3000, cx);
                            cx.notify();
                        });
                        return;
                    }
                }
            }
            let res = client
                .submit_creation_for_review(&creation_id, Some(&submit_req))
                .await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                match res {
                    Ok(submitted) => {
                        let cid = submitted.creation.creation_id.clone();
                        let was_selected = {
                            let h = shell.content_hub.read(cx);
                            h.selected_creation_id.as_deref() == Some(cid.as_str())
                                || h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == cid)
                        };
                        shell.content_hub_state.bump_creations_gen();
                        if was_selected {
                            shell.content_hub_state.bump_detail_gen();
                        }
                        let same_modal = shell.content_hub.update(cx, |h, hcx| {
                            h.is_loading_creations = false;
                            h.upsert_my_creation(submitted.creation);
                            let same_modal = h.publish_modal.is_open
                                && h.publish_modal.target_creation_id.as_deref()
                                    == Some(cid.as_str());
                            if same_modal {
                                h.publish_modal.is_submitting = false;
                                h.publish_modal.server_error = None;
                                h.publish_modal.validation_errors = submitted.validation_errors;
                                h.publish_modal.status_notice = Some(
                                    tr_str(
                                        &lang,
                                        "已提交审核，审核通过后将自动上架至对应苹果板块",
                                        "Submitted for review (PendingReview)",
                                    )
                                    .to_string(),
                                );
                            }
                            hcx.notify();
                            same_modal
                        });
                        if same_modal {
                            shell.fetch_creation_audit_fenced(cid, cx);
                        }
                        if shell.content_hub.read(cx).publish_modal.reviewer_access
                            == ReviewerAccessState::Allowed
                        {
                            shell.probe_reviewer_queue(cx, false);
                        }
                        shell.sync_apple_sidebar_badges(cx);
                    }
                    Err(err) => {
                        let (msg, parsed_errs) = format_creation_mutation_error(&lang, err);
                        let applied_updated_draft = updated_draft.is_some();
                        let mut was_in_review_queue = false;
                        if let Some(ref dto) = updated_draft {
                            let cid = dto.creation_id.as_str();
                            let (was_selected, in_queue) = {
                                let h = shell.content_hub.read(cx);
                                let selected = h.selected_creation_id.as_deref() == Some(cid)
                                    || h.selected_detail_creation
                                        .as_ref()
                                        .is_some_and(|d| d.creation_id == cid);
                                let q = h
                                    .publish_modal
                                    .review_queue
                                    .iter()
                                    .any(|item| item.creation_id == cid);
                                (selected, q)
                            };
                            was_in_review_queue = in_queue;
                            shell.content_hub_state.bump_creations_gen();
                            if was_in_review_queue {
                                shell.content_hub_state.bump_review_queue_gen();
                            }
                            if was_selected {
                                shell.content_hub_state.bump_detail_gen();
                            }
                        }
                        shell.content_hub.update(cx, |h, hcx| {
                            if let Some(dto) = updated_draft {
                                h.is_loading_creations = false;
                                if was_in_review_queue {
                                    h.publish_modal.is_loading_queue = false;
                                }
                                // Syncs cloud_revision (R -> R+1) in my_creations and matching
                                // base_creation WITHOUT clobbering user's review_notes or form edits.
                                h.upsert_my_creation(dto);
                            }
                            let same_modal = h.publish_modal.is_open
                                && h.publish_modal.target_creation_id.as_deref()
                                    == Some(creation_id.as_str());
                            if same_modal {
                                h.publish_modal.is_submitting = false;
                                h.publish_modal.server_error = Some(msg.clone());
                                // Applied AFTER upsert_my_creation so server 422 errors win over local recompute.
                                if let Some(errs) = parsed_errs {
                                    h.publish_modal.validation_errors = errs;
                                }
                            }
                            hcx.notify();
                        });
                        if applied_updated_draft {
                            shell.sync_apple_sidebar_badges(cx);
                        }
                        shell.show_toast(msg, 3200, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn withdraw_creation_review_fenced(
        &mut self,
        creation_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let Some(fence) = build_hub_fence(self, self.content_hub_state.creations_request_gen)
        else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.withdraw_creation_review(&creation_id).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                match res {
                    Ok(dto) => {
                        let cid = dto.creation_id.clone();
                        let was_selected = {
                            let h = shell.content_hub.read(cx);
                            h.selected_creation_id.as_deref() == Some(cid.as_str())
                                || h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == cid)
                        };
                        shell.content_hub_state.bump_creations_gen();
                        shell.content_hub_state.bump_review_queue_gen();
                        if was_selected {
                            shell.content_hub_state.bump_detail_gen();
                        }
                        shell.content_hub.update(cx, |h, hcx| {
                            h.is_loading_creations = false;
                            h.publish_modal.is_loading_queue = false;
                            h.upsert_my_creation(dto);
                            hcx.notify();
                        });
                        shell.sync_apple_sidebar_badges(cx);
                    }
                    Err(err) => shell.show_toast(err.to_string(), 2600, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn unpublish_creation_fenced(&mut self, creation_id: String, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let Some(fence) = build_hub_fence(self, self.content_hub_state.creations_request_gen)
        else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.unpublish_creation(&creation_id).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                match res {
                    Ok(dto) => {
                        let cid = dto.creation_id.clone();
                        let was_selected = {
                            let h = shell.content_hub.read(cx);
                            h.selected_creation_id.as_deref() == Some(cid.as_str())
                                || h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == cid)
                        };
                        shell.content_hub_state.bump_creations_gen();
                        shell.content_hub_state.bump_review_queue_gen();
                        let bumped_active_catalog = shell.bump_catalog_fences_for_kind(dto.kind);
                        if was_selected {
                            shell.content_hub_state.bump_detail_gen();
                        }
                        shell.content_hub.update(cx, |h, hcx| {
                            h.is_loading_creations = false;
                            h.publish_modal.is_loading_queue = false;
                            if bumped_active_catalog {
                                h.is_loading_catalog = false;
                            }
                            h.evict_public_catalog_item(&dto.creation_id);
                            h.upsert_my_creation(dto);
                            hcx.notify();
                        });
                        shell.sync_apple_sidebar_badges(cx);
                    }
                    Err(err) => shell.show_toast(err.to_string(), 2600, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn delete_creation_fenced(&mut self, creation_id: String, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        let Some(fence) = build_hub_fence(self, self.content_hub_state.creations_request_gen)
        else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.delete_creation(&creation_id).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                match res {
                    Ok(dto) => {
                        let cid = dto.creation_id.clone();
                        let (was_selected, was_modal_target) = {
                            let h = shell.content_hub.read(cx);
                            let selected = h.selected_creation_id.as_deref() == Some(cid.as_str())
                                || h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == cid);
                            let modal_target =
                                h.publish_modal.target_creation_id.as_deref() == Some(cid.as_str());
                            (selected, modal_target)
                        };
                        shell.content_hub_state.bump_creations_gen();
                        shell.content_hub_state.bump_review_queue_gen();
                        let bumped_active_catalog = shell.bump_catalog_fences_for_kind(dto.kind);
                        if was_selected {
                            shell.content_hub_state.bump_detail_gen();
                        }
                        if was_modal_target {
                            shell.content_hub_state.bump_audit_gen();
                        }
                        let modal_closed_by_delete = shell.content_hub.update(cx, |h, hcx| {
                            let was_modal_open = h.publish_modal.is_open;
                            h.is_loading_creations = false;
                            h.publish_modal.is_loading_queue = false;
                            if bumped_active_catalog {
                                h.is_loading_catalog = false;
                            }
                            h.evict_public_catalog_item(&dto.creation_id);
                            h.upsert_my_creation(dto);
                            hcx.notify();
                            was_modal_open && !h.publish_modal.is_open
                        });
                        if modal_closed_by_delete
                            && !shell.overlays.is_passcode_locked
                            && shell.overlays.computer_overlay.is_none()
                        {
                            shell.clear_all_ime_preedits(cx);
                        }
                        shell.sync_apple_sidebar_badges(cx);
                    }
                    Err(err) => shell.show_toast(err.to_string(), 2600, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn submit_review_decision_fenced(
        &mut self,
        creation_id: String,
        mut req: ReviewDecisionRequest,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        if req.decision == ReviewDecision::Reject {
            let Some(cleaned_reason) = req
                .reason
                .as_deref()
                .and_then(crate::views::content_hub::publish_review_modal::normalize_non_empty_reason)
            else {
                let lang = self.active_lang_id();
                let err = tr_str(
                    lang,
                    "拒绝上架时必须填写退回原因",
                    "Rejection reason is mandatory",
                )
                .to_string();
                self.content_hub.update(cx, |h, hcx| {
                    h.publish_modal.is_submitting = false;
                    h.publish_modal.reviewer_error = Some(err.clone());
                    hcx.notify();
                });
                return;
            };
            req.reason = Some(cleaned_reason);
        }
        let Some(fence) = build_hub_fence(self, self.content_hub_state.review_queue_request_gen)
        else {
            self.content_hub.update(cx, |h, hcx| {
                h.publish_modal.is_submitting = false;
                hcx.notify();
            });
            return;
        };
        let client = self.client.clone();
        let lang = self.active_lang_id().to_string();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.submit_review_decision(&creation_id, &req).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                match res {
                    Ok(dto) => {
                        let kind = dto.kind;
                        let reviewed_id = dto.creation_id.clone();
                        let is_owner =
                            Some(dto.owner_user_id) == shell.session_sync.user_id.map(|u| u as i64);
                        let was_selected = {
                            let h = shell.content_hub.read(cx);
                            h.selected_creation_id.as_deref() == Some(reviewed_id.as_str())
                                || h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == reviewed_id)
                        };
                        let evict_catalog = dto.deleted_at_ms.is_some()
                            || !dto.has_live_published_version
                            || dto.visibility != CreationVisibility::Public;
                        shell.content_hub_state.bump_review_queue_gen();
                        if is_owner {
                            shell.content_hub_state.bump_creations_gen();
                        }
                        let bumped_active_catalog = if evict_catalog {
                            shell.bump_catalog_fences_for_kind(kind)
                        } else {
                            false
                        };
                        if was_selected {
                            shell.content_hub_state.bump_detail_gen();
                        }
                        let was_modal_target = !shell.overlays.is_passcode_locked
                            && shell.overlays.computer_overlay.is_none()
                            && shell.resolve_input_target(cx)
                                == crate::views::app_shell::shell_input_ime::ShellInputTarget::PublishReviewModal;
                        shell.content_hub.update(cx, |h, hcx| {
                            h.publish_modal.is_submitting = false;
                            h.publish_modal.is_loading_queue = false;
                            if is_owner {
                                h.is_loading_creations = false;
                            }
                            if evict_catalog {
                                if bumped_active_catalog {
                                    h.is_loading_catalog = false;
                                }
                                h.evict_public_catalog_item(&reviewed_id);
                            }
                            if dto.review_status != CreationReviewStatus::PendingReview {
                                h.record_resolved_review(&reviewed_id, dto.cloud_revision);
                            }
                            h.publish_modal.prune_review_queue_item(&reviewed_id);
                            h.publish_modal.reject_reason.clear();
                            h.publish_modal.clear_all_preedits();
                            h.publish_modal.reviewer_error = None;
                            if is_owner {
                                h.upsert_my_creation(dto);
                            } else if dto.deleted_at_ms.is_some() {
                                h.record_deleted_creation(&reviewed_id, dto.cloud_revision);
                                if h.selected_creation_id.as_deref() == Some(reviewed_id.as_str()) {
                                    h.selected_creation_id = None;
                                }
                                if h.selected_detail_creation
                                    .as_ref()
                                    .is_some_and(|d| d.creation_id == reviewed_id)
                                {
                                    h.selected_detail_creation = None;
                                }
                            } else if h.selected_creation_id.as_deref()
                                == Some(reviewed_id.as_str())
                                && h.selected_detail_creation.as_ref().is_none_or(|cur| {
                                    cur.creation_id != reviewed_id
                                        || dto.cloud_revision >= cur.cloud_revision
                                })
                            {
                                h.selected_detail_creation = Some(dto);
                            }
                            hcx.notify();
                        });
                        if was_modal_target {
                            shell.clear_all_ime_preedits(cx);
                        }
                        shell.sync_apple_sidebar_badges(cx);
                        if req.decision == ReviewDecision::Approve {
                            let vertical = ContentHubSection::from_creation_kind(kind);
                            shell.refresh_content_hub_catalog(vertical, cx);
                            if shell.active_panel
                                == ActiveWorkspacePanel::ContentHub(ContentHubSection::AppStore)
                                && vertical != ContentHubSection::AppStore
                            {
                                shell.refresh_content_hub_catalog(ContentHubSection::AppStore, cx);
                            }
                        }
                        let toast = if req.decision == ReviewDecision::Approve {
                            tr_str(&lang, "已审核通过并更新公开板块", "Approved and published")
                        } else {
                            tr_str(&lang, "已退回修改并记录审计原因", "Rejected with reason")
                        };
                        shell.show_toast(toast, 2400, cx);
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        shell.content_hub.update(cx, |h, hcx| {
                            h.publish_modal.is_submitting = false;
                            h.publish_modal.reviewer_error = Some(msg.clone());
                            hcx.notify();
                        });
                        shell.show_toast(msg, 2800, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl AppShell {
    /// Applies an inbound WS `UpdateEvent::UserCreationChanged` frame with monotonic revision guards.
    pub fn handle_ws_user_creation_changed(
        &mut self,
        creation_id: String,
        owner_user_id: i64,
        _kind: CreationKind,
        review_status: CreationReviewStatus,
        cloud_revision: i64,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated {
            return;
        }
        let is_current_owner = self
            .session_sync
            .user_id
            .is_some_and(|uid| uid as i64 == owner_user_id);
        let (is_stale, was_in_review_queue, reviewer_allowed, modal_open) = {
            let h = self.content_hub.read(cx);
            let stale_deleted = h.is_deleted_at_or_above(&creation_id, cloud_revision);
            let stale_resolved = if review_status == CreationReviewStatus::PendingReview {
                h.is_resolved_review_at_or_above(&creation_id, cloud_revision)
            } else {
                h.resolved_review_revisions
                    .get(&creation_id)
                    .is_some_and(|&r| cloud_revision < r)
            };
            let stale_own = is_current_owner
                && h.my_creations
                    .iter()
                    .find(|c| c.creation_id == creation_id)
                    .is_some_and(|existing| {
                        cloud_revision < existing.cloud_revision
                            || (cloud_revision == existing.cloud_revision
                                && review_status != existing.review_status)
                    });
            let stale_base = is_current_owner
                && h.publish_modal.base_creation.as_ref().is_some_and(|b| {
                    b.creation_id == creation_id
                        && (cloud_revision < b.cloud_revision
                            || (cloud_revision == b.cloud_revision
                                && review_status != b.review_status))
                });
            let queue_entry = h
                .publish_modal
                .review_queue
                .iter()
                .find(|q| q.creation_id == creation_id);
            let stale_queue = queue_entry.is_some_and(|q| {
                cloud_revision < q.cloud_revision
                    || (cloud_revision == q.cloud_revision && review_status != q.review_status)
            });
            let stale_detail = h.selected_detail_creation.as_ref().is_some_and(|d| {
                d.creation_id == creation_id
                    && (cloud_revision < d.cloud_revision
                        || (cloud_revision == d.cloud_revision
                            && review_status != d.review_status))
            });
            (
                stale_deleted
                    || stale_resolved
                    || stale_own
                    || stale_base
                    || stale_queue
                    || stale_detail,
                queue_entry.is_some(),
                h.publish_modal.reviewer_access == ReviewerAccessState::Allowed,
                h.publish_modal.is_open,
            )
        };
        if is_stale {
            return;
        }
        let will_probe_queue = reviewer_allowed
            && (review_status == CreationReviewStatus::PendingReview
                || was_in_review_queue
                || modal_open);
        let prune_queue_without_probe = was_in_review_queue
            && review_status != CreationReviewStatus::PendingReview
            && !will_probe_queue;
        if prune_queue_without_probe {
            self.content_hub_state.bump_review_queue_gen();
        }
        self.content_hub.update(cx, |h, hcx| {
            if is_current_owner {
                h.my_creations_revision = h.my_creations_revision.max(cloud_revision);
                if let Some(existing) = h
                    .my_creations
                    .iter_mut()
                    .find(|c| c.creation_id == creation_id)
                {
                    existing.review_status = review_status;
                    existing.cloud_revision = existing.cloud_revision.max(cloud_revision);
                }
                if let Some(base) = h.publish_modal.base_creation.as_mut() {
                    if base.creation_id == creation_id && cloud_revision >= base.cloud_revision {
                        base.review_status = review_status;
                        base.cloud_revision = base.cloud_revision.max(cloud_revision);
                    }
                }
            }
            if let Some(detail) = h.selected_detail_creation.as_mut() {
                if detail.creation_id == creation_id && cloud_revision >= detail.cloud_revision {
                    detail.review_status = review_status;
                    detail.cloud_revision = detail.cloud_revision.max(cloud_revision);
                }
            }
            if review_status != CreationReviewStatus::PendingReview {
                h.record_resolved_review(&creation_id, cloud_revision);
            } else {
                h.clear_resolved_review_if_newer(&creation_id, cloud_revision);
            }
            if was_in_review_queue && review_status != CreationReviewStatus::PendingReview {
                if prune_queue_without_probe {
                    h.publish_modal.is_loading_queue = false;
                }
                h.publish_modal.prune_review_queue_item(&creation_id);
            } else if let Some(q) = h
                .publish_modal
                .review_queue
                .iter_mut()
                .find(|item| item.creation_id == creation_id)
            {
                q.review_status = review_status;
                q.cloud_revision = q.cloud_revision.max(cloud_revision);
            }
            hcx.notify();
        });
        if is_current_owner {
            self.refresh_my_creations(cx);
        }
        if will_probe_queue {
            self.probe_reviewer_queue(cx, false);
        }
        self.sync_apple_sidebar_badges(cx);
        cx.notify();
    }

    /// Applies an inbound WS `UpdateEvent::PublicCatalogChanged` frame with immediate eviction on `"removed"`.
    pub fn handle_ws_public_catalog_changed(
        &mut self,
        creation_id: String,
        kind: CreationKind,
        action: String,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated {
            return;
        }
        let is_removed = action.trim().eq_ignore_ascii_case("removed");
        if is_removed {
            let _ = self
                .client
                .local_store
                .remove_public_catalog_item(&creation_id);
            self.content_hub.update(cx, |h, hcx| {
                h.evict_public_catalog_item(&creation_id);
                hcx.notify();
            });
        }
        let target_section = ContentHubSection::from_creation_kind(kind);
        self.refresh_content_hub_catalog(target_section, cx);
        if self.active_panel == ActiveWorkspacePanel::ContentHub(ContentHubSection::AppStore)
            && target_section != ContentHubSection::AppStore
        {
            self.refresh_content_hub_catalog(ContentHubSection::AppStore, cx);
        } else if is_removed && target_section != ContentHubSection::AppStore {
            self.content_hub_state
                .bump_catalog_gen_for(ContentHubSection::AppStore);
        }
        cx.notify();
    }

    /// Stops playback, wipes in-memory creations/catalogs/review queue, closes modals/drawer,
    /// and resets `AppleAppSidebar` on logout or account switch.
    pub fn reset_content_hub_for_logout_or_account_switch(&mut self, cx: &mut Context<Self>) {
        self.content_hub_state.reset_generations();
        self.apple_sidebar.update(cx, |sb, scx| {
            sb.reset_for_logout_or_account_switch();
            scx.notify();
        });
        self.content_hub.update(cx, |h, hcx| {
            h.reset_for_logout_or_account_switch();
            hcx.notify();
        });
    }

    /// Single-point Apple deep-link router + zero-leak smuggling swallow guard.
    /// Returns `true` if `raw` was routed OR swallowed as a malformed/smuggled/unauth Apple payload.
    pub fn try_handle_or_swallow_apple_deep_link(
        &mut self,
        raw: &str,
        allow_url_prefix: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let candidate = if allow_url_prefix {
            unwrap_apple_callback_uri(raw)
        } else {
            raw
        };
        if parse_apple_deep_link(candidate).is_some() {
            let _ = self.handle_apple_deep_link(candidate, cx);
            return true;
        }
        is_apple_deep_link_candidate(raw)
    }

    /// Unified Chat-to-Hub deep-link router for `tg://apple/*` and `creation:open:<kind>:<id>`.
    pub fn handle_apple_deep_link(&mut self, uri: &str, cx: &mut Context<Self>) -> bool {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return false;
        }
        let Some(link) = parse_apple_deep_link(uri) else {
            return false;
        };
        let section = ContentHubSection::from_deep_link_section(link.section);
        // Dismiss ALL blocking layers strictly top-down (Z #15 -> Z #2) after parse succeeds:
        // Z #15: ComputerWorkbench
        if self.overlays.computer_overlay.is_some() {
            self.hide_remote_computer(cx);
        }
        // Z #14: PublishReviewModal
        self.close_publish_review_modal(cx);
        // Z #13: ModalContainer
        self.modal_container.update(cx, |m, mcx| {
            if m.is_modal_active() {
                m.hide_all();
                mcx.notify();
            }
        });
        // Z #11..#8: McpTools / TaskCenter / Settings / Calls + NavRail sync
        self.leave_workspace_modal_panel_for_hub(cx);
        // Z #7 & Z #6: CreationModal & ContactsModal
        self.sidebar.update(cx, |sb, scx| {
            let had_modals = sb.active_creation_modal.is_some() || sb.is_contacts_modal_open;
            if sb.active_creation_modal.is_some() {
                sb.close_creation_modal();
            }
            if sb.is_contacts_modal_open {
                sb.close_contacts_modal();
            }
            if had_modals {
                scx.notify();
            }
        });
        // Z #5e / #5d / #5c / #5b: BotProfileBox / MiniAppToS / PendingMiniAppConfirm / CreateManagedBotBox
        if let Some(bx) = self.miniapps.bot_profile_box.take() {
            bx.update(cx, |b, bcx| {
                let was_vis = b.is_visible;
                b.close();
                if was_vis {
                    bcx.notify();
                }
            });
        }
        self.miniapps.miniapp_tos_modal_open = false;
        self.miniapps.pending_miniapp_confirm = None;
        if let Some(bx) = self.miniapps.create_bot_box.take() {
            bx.update(cx, |b, bcx| {
                if b.is_visible {
                    b.is_visible = false;
                    bcx.notify();
                }
            });
        }
        // Z #5a: MiniAppOverlay (full minimize: is_open = false, clear popups, hide native WKWebView)
        if let Some(ov) = self.miniapps.miniapp_overlay.clone() {
            self.miniapps.is_miniapp_minimized = true;
            ov.update(cx, |w, wcx| {
                w.minimize();
                w.focused_input = None;
                wcx.notify();
            });
        }
        // Z #4 / #3 / #2: IvViewer / MediaViewer / TalkMode
        self.overlays.iv_viewer_overlay = None;
        self.overlays.media_viewer_overlay = None;
        if self.rtc_talk.talk_mode_overlay.is_some() {
            self.hide_talk_mode(cx);
        }
        // Transient popovers (AccountPopover, AppleCompose, MainMenuDrawer, searches, composer blur/IME)
        self.dismiss_transient_popovers_for_hub(cx);
        self.active_panel = ActiveWorkspacePanel::ContentHub(section);
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        }
        self.apple_sidebar.update(cx, |sb, scx| {
            sb.set_active_section(AppleSidebarSection::from_content_hub_section(section));
            scx.notify();
        });
        let needs_detail_fetch = self.content_hub.update(cx, |h, hcx| {
            h.apply_deep_link(&link);
            hcx.notify();
            link.creation_id
                .as_deref()
                .is_some_and(|_| h.selected_detail_creation.is_none())
        });
        self.sync_local_miniapps_to_content_hub(cx);
        if section == ContentHubSection::MyCreations {
            self.refresh_my_creations(cx);
        } else {
            self.refresh_content_hub_catalog(section, cx);
        }
        if needs_detail_fetch {
            if let Some(cid) = link.creation_id {
                self.fetch_creation_detail_fenced(cid, cx);
            }
        }
        cx.notify();
        true
    }

    /// Opens (or idempotently creates) the 1:1 DM with `@CreatorStudioBot` (`UID 777_020`)
    /// and optionally prefills a slash command such as `/newbook `, `/newmusic `, or `/newgame `.
    pub fn open_creator_studio_bot_chat(
        &mut self,
        prefill_command: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        self.close_publish_review_modal(cx);
        self.toggle_quick_creations_drawer(false, cx);
        self.leave_workspace_modal_panel_for_hub(cx);
        self.dismiss_transient_popovers_for_hub(cx);

        let bot_uid = CREATOR_STUDIO_BOT_USER_ID as u64;
        let prefill_owned = prefill_command
            .map(str::trim_start)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let existing_dm_chat_id = self
            .session_sync
            .rich_chats_by_id
            .values()
            .find(|rc| {
                !rc.is_group
                    && !rc.is_channel
                    && !matches!(rc.chat_type, 1 | 2 | 3)
                    && (rc.peer_user_id == Some(bot_uid)
                        || rc.peer_username.as_deref().is_some_and(|u| {
                            u.trim_start_matches('@')
                                .eq_ignore_ascii_case(CREATOR_STUDIO_BOT_USERNAME)
                        }))
            })
            .filter(|rc| rc.chat_id > 0)
            .map(|rc| rc.chat_id as u64)
            .or_else(|| {
                let sb = self.sidebar.read(cx);
                self.session_sync
                    .chat_peer_user_ids
                    .iter()
                    .find(|(cid, pid)| {
                        **pid == bot_uid
                            && !sb.group_chat_ids.contains(cid)
                            && !sb.channel_chat_ids.contains(cid)
                    })
                    .map(|(cid, _)| *cid)
            })
            .or_else(|| {
                self.session_sync
                    .contacts_list
                    .iter()
                    .find_map(|c| {
                        (c.user_id == bot_uid)
                            .then_some(c.chat_id)
                            .flatten()
                            .filter(|&id| id > 0)
                            .map(|id| id as u64)
                    })
            });

        if let Some(cid) = existing_dm_chat_id {
            self.open_or_select_chat(cid, Some("Creator Studio".to_string()), true, cx);
            self.sync_apple_sidebar_to_active_panel(cx);
            Self::apply_creator_bot_composer_prefill(self, prefill_owned.as_deref(), cx);
            return;
        }

        self.active_panel = ActiveWorkspacePanel::Chats;
        self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
        self.sync_apple_sidebar_to_active_panel(cx);
        cx.notify();

        if self.content_hub_state.creator_bot_chat_in_flight {
            return;
        }
        let Some(fence) = build_hub_fence(self, self.content_hub_state.creations_request_gen)
        else {
            return;
        };
        self.content_hub_state.creator_bot_chat_in_flight = true;
        let client = self.client.clone();
        let req = CreateChatRequest {
            title: "Creator Studio".to_string(),
            chat_type: Some(serde_json::json!(4)),
            peer_user_id: Some(bot_uid),
            ..Default::default()
        };
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.create_chat(&req).await;
            let _ = weak.update(app, |shell, cx| {
                if !hub_session_fence_matches(&fence, shell) {
                    return;
                }
                shell.content_hub_state.creator_bot_chat_in_flight = false;
                match res {
                    Ok(created) => {
                        let cid = created.chat_id.max(0) as u64;
                        shell.session_sync.chat_peer_user_ids.insert(cid, bot_uid);
                        let title = if created.title.trim().is_empty() {
                            "Creator Studio".to_string()
                        } else {
                            created.title
                        };
                        shell.open_or_select_chat(cid, Some(title), true, cx);
                        shell.sync_apple_sidebar_to_active_panel(cx);
                        Self::apply_creator_bot_composer_prefill(
                            shell,
                            prefill_owned.as_deref(),
                            cx,
                        );
                        shell.refresh_chats(cx);
                    }
                    Err(err) => {
                        shell.show_toast(err.to_string(), 2800, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn apply_creator_bot_composer_prefill(
        shell: &mut Self,
        prefill_command: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(cmd) = prefill_command.filter(|s| !s.is_empty()) {
            shell.stage.update(cx, |st, st_cx| {
                st.show_bot_start_bar = false;
                st_cx.notify();
            });
            let focus_composer = shell.can_focus_chat_composer(cx);
            shell.composer.update(cx, |c, c_cx| {
                c.set_text(cmd.to_string());
                c.sync_autocomplete();
                c.is_input_focused = focus_composer;
                c_cx.notify();
            });
            cx.notify();
        }
    }
}
