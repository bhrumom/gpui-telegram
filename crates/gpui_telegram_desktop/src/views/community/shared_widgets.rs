//! Reusable stateless GPUI chrome widgets shared across Community feed, cards, nav, and right rail.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::community_contracts::{ActorKind, VerifiedTier};

use crate::components::avatar::{parse_fabushi_hex_color, render_fabushi_shape_avatar};
use crate::components::grok_bot_avatar::{render_grok_persona_avatar, GrokPersonaState};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::macos_popover_surface;
use crate::views::stage::rich_meta::resolve_image_source;

use super::workspace::CommunityWorkspace;

/// Normalizes any `TdesktopThemePalette` onto OpenBot `383a35d2`'s monochrome surface hierarchy
/// for `CommunityWorkspace` while preserving `pal.mode` (`Day`/`Night`/`Classic`/`Tinted`) intact.
pub fn community_openbot_palette(pal: TdesktopThemePalette) -> TdesktopThemePalette {
    let mut p = pal.with_openbot_chrome();
    if p.mode.is_dark() {
        p.stage_bg = 0x1A1A1A;
        p.dialogs_bg = 0x1A1A1A;
        p.card_bg = 0x212121;
        p.input_bg = 0x212121;
        p.modal_bg = 0x212121;
        p.menu_bg = 0x212121;
        p.service_pill_bg = 0x242424;
        p.hover_bg = 0x242424;
        p.dialogs_hover_bg = 0x242424;
        p.dialogs_active_bg = 0x2A2A2A;
        p.border_fg = 0x262626;
        p.accent = 0xF0F0F0;
        p.accent_hover = 0xDCDCDC;
        p.active_button_fg = 0x141414;
        p.unread_badge_bg = 0xF0F0F0;
        p.unread_badge_fg = 0x141414;
    } else {
        p.stage_bg = 0xF5F5F6;
        p.dialogs_bg = 0xF5F5F6;
        p.card_bg = 0xFFFFFF;
        p.input_bg = 0xFFFFFF;
        p.modal_bg = 0xFFFFFF;
        p.menu_bg = 0xFFFFFF;
        p.service_pill_bg = 0xF0F0F2;
        p.hover_bg = 0xF0F0F2;
        p.dialogs_hover_bg = 0xF0F0F2;
        p.dialogs_active_bg = 0xE8E8EB;
        p.border_fg = 0xDCDCE0;
        p.accent = 0x1C1C1E;
        p.accent_hover = 0x3A3A3C;
        p.active_button_fg = 0xFFFFFF;
        p.unread_badge_bg = 0x1C1C1E;
        p.unread_badge_fg = 0xFFFFFF;
    }
    p
}

/// Computes live OpenBot Community Hub shell geometry and monochrome token audit (`UI-F18`),
/// deriving `legacy_blue_pill_count` strictly from `community_openbot_palette(pal)`.
pub fn compute_community_hub_layout(
    pal: &TdesktopThemePalette,
    vp_w: f32,
) -> crate::views::content_hub::apple_primitives::ContentHubShellLayoutSnapshot {
    use crate::views::content_hub::apple_primitives::{
        is_legacy_blue_rgb, rgb24_to_hex, ContentHubShellLayoutSnapshot,
        OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH, OPENBOT_MARKETPLACE_LEFT_NAV_WIDTH,
        OPENBOT_PAGE_HEADER_HEIGHT,
    };
    let comm_pal = community_openbot_palette(*pal);
    let mut legacy_blue_pill_count = 0usize;
    for color in [
        comm_pal.accent,
        comm_pal.unread_badge_bg,
        comm_pal.dialogs_active_bg,
        comm_pal.card_bg,
    ] {
        if is_legacy_blue_rgb(color) {
            legacy_blue_pill_count += 1;
        }
    }
    ContentHubShellLayoutSnapshot {
        active_page: "community",
        viewport_width_px: if vp_w.is_finite() && vp_w > 0.0 { vp_w } else { 0.0 },
        page_header_height_px: OPENBOT_PAGE_HEADER_HEIGHT,
        left_nav_width_px: OPENBOT_MARKETPLACE_LEFT_NAV_WIDTH,
        content_max_width_px: OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH,
        canvas_hex: rgb24_to_hex(comm_pal.stage_bg),
        card_surface_hex: rgb24_to_hex(comm_pal.card_bg),
        hairline_hex: rgb24_to_hex(comm_pal.border_fg),
        primary_cta_bg_hex: rgb24_to_hex(comm_pal.accent),
        primary_cta_fg_hex: rgb24_to_hex(comm_pal.active_button_fg),
        legacy_blue_pill_count,
    }
}

/// Resolves a community media attachment or avatar reference into an HTTP(S)/file URL or local path.
pub fn resolve_community_media_url(
    base_url: &str,
    media_id: Option<&str>,
    explicit_url: Option<&str>,
) -> Option<String> {
    let base = base_url.trim().trim_end_matches('/');
    let is_direct_scheme = |raw: &str| {
        let lower = raw.to_ascii_lowercase();
        lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("file://")
    };
    if let Some(raw) = explicit_url.map(str::trim).filter(|s| !s.is_empty()) {
        if is_direct_scheme(raw) || std::path::Path::new(raw).exists() {
            return Some(raw.to_string());
        }
        if raw.starts_with('/') && !base.is_empty() {
            return Some(format!("{base}{raw}"));
        }
        if !base.is_empty() {
            return Some(format!("{base}/api/v1/media/stream/{}", raw.trim_start_matches('/')));
        }
        return Some(raw.to_string());
    }
    if let Some(mid) = media_id.map(str::trim).filter(|s| !s.is_empty()) {
        if is_direct_scheme(mid) || std::path::Path::new(mid).exists() {
            return Some(mid.to_string());
        }
        if !base.is_empty() {
            return Some(format!("{base}/api/v1/media/stream/{}", mid.trim_start_matches('/')));
        }
        return Some(mid.to_string());
    }
    None
}

fn match_fence_opener(bytes: &[u8], line_start: usize) -> Option<(u8, usize, usize)> {
    let len = bytes.len();
    let mut j = line_start;
    let mut spaces = 0usize;
    while j < len && bytes[j] == b' ' && spaces < 3 {
        j += 1;
        spaces += 1;
    }
    if j >= len || (bytes[j] != b'`' && bytes[j] != b'~') {
        return None;
    }
    let fch = bytes[j];
    let mut k = j;
    while k < len && bytes[k] == fch {
        k += 1;
    }
    let run = k - j;
    if run < 3 {
        return None;
    }
    let mut eol = k;
    while eol < len && bytes[eol] != b'\n' {
        eol += 1;
    }
    // CommonMark: backtick fence info strings may not contain backticks.
    if fch == b'`' && bytes[k..eol].contains(&b'`') {
        return None;
    }
    let line_end = if eol < len { eol + 1 } else { eol };
    Some((fch, run, line_end))
}

fn match_fence_closer(bytes: &[u8], line_start: usize, fch: u8, min_run: usize) -> Option<usize> {
    let len = bytes.len();
    let mut j = line_start;
    let mut spaces = 0usize;
    while j < len && bytes[j] == b' ' && spaces < 3 {
        j += 1;
        spaces += 1;
    }
    let mut k = j;
    while k < len && bytes[k] == fch {
        k += 1;
    }
    if k - j < min_run {
        return None;
    }
    let mut tail = k;
    while tail < len && matches!(bytes[tail], b' ' | b'\t' | b'\r') {
        tail += 1;
    }
    if tail == len {
        Some(tail)
    } else if bytes[tail] == b'\n' {
        Some(tail + 1)
    } else {
        None
    }
}

fn is_blank_line_at(bytes: &[u8], after_nl: usize) -> bool {
    let mut p = after_nl;
    while p < bytes.len() && matches!(bytes[p], b' ' | b'\t' | b'\r') {
        p += 1;
    }
    p < bytes.len() && bytes[p] == b'\n'
}

fn find_matching_backtick_run(bytes: &[u8], start: usize, run_len: usize) -> Option<usize> {
    let len = bytes.len();
    let mut p = start;
    while p < len {
        if bytes[p] == b'\n' {
            if is_blank_line_at(bytes, p + 1) || match_fence_opener(bytes, p + 1).is_some() {
                return None;
            }
            p += 1;
            continue;
        }
        if bytes[p] == b'`' {
            let mut q = p;
            while q < len && bytes[q] == b'`' {
                q += 1;
            }
            if q - p == run_len {
                return Some(q);
            }
            p = q;
        } else {
            p += 1;
        }
    }
    None
}

fn find_markdown_inline_link_end(body: &str, start: usize) -> Option<usize> {
    let bytes = body.as_bytes();
    let len = bytes.len();
    if start >= len || bytes[start] != b'[' {
        return None;
    }
    let mut p = start + 1;
    let mut bracket_depth = 1usize;
    while p < len && bracket_depth > 0 {
        let b = bytes[p];
        if b == b'\\' && p + 1 < len {
            let step = body[p + 1..].chars().next().map_or(0, char::len_utf8);
            p += 1 + step;
            continue;
        }
        if b == b'`' {
            let mut q = p;
            while q < len && bytes[q] == b'`' {
                q += 1;
            }
            if let Some(code_end) = find_matching_backtick_run(bytes, q, q - p) {
                p = code_end;
                continue;
            }
            p = q;
            continue;
        }
        if b == b'\n'
            && (is_blank_line_at(bytes, p + 1) || match_fence_opener(bytes, p + 1).is_some())
        {
            return None;
        }
        if b == b'[' {
            bracket_depth += 1;
        } else if b == b']' {
            bracket_depth -= 1;
            if bracket_depth == 0 {
                break;
            }
        }
        p += 1;
    }
    if bracket_depth != 0 || p + 1 >= len || bytes[p + 1] != b'(' {
        return None;
    }
    let mut d = p + 2;
    let mut paren_depth = 1usize;
    while d < len && paren_depth > 0 {
        let b = bytes[d];
        if b == b'\\' && d + 1 < len {
            let step = body[d + 1..].chars().next().map_or(0, char::len_utf8);
            d += 1 + step;
            continue;
        }
        if b == b'\n'
            && (is_blank_line_at(bytes, d + 1) || match_fence_opener(bytes, d + 1).is_some())
        {
            return None;
        }
        if b == b'(' {
            paren_depth += 1;
        } else if b == b')' {
            paren_depth -= 1;
            if paren_depth == 0 {
                return Some(d + 1);
            }
        }
        d += 1;
    }
    None
}

fn starts_with_bare_url_scheme(s: &str) -> bool {
    let matches_ci = |pat: &str| {
        s.len() >= pat.len()
            && s.is_char_boundary(pat.len())
            && s[..pat.len()].eq_ignore_ascii_case(pat)
    };
    matches_ci("https://")
        || matches_ci("http://")
        || matches_ci("tg://")
        || matches_ci("mailto:")
}

/// Converts standalone `c/<slug>` tokens outside code fences, CommonMark inline code spans,
/// existing Markdown `[...](...)` links, and bare URLs into `[c/<slug>](tg://community/<slug>)`
/// so `render_rich_message_body` renders them clickably.
pub fn linkify_community_slugs(body: &str) -> String {
    if !body.contains("c/") {
        return body.to_string();
    }
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 32);
    let mut i = 0usize;
    let mut at_line_start = true;
    let mut active_fence: Option<(u8, usize)> = None;

    while i < len {
        // 1. Inside fenced code block: copy verbatim until matching closing fence line.
        if let Some((fch, min_run)) = active_fence {
            if at_line_start {
                if let Some(close_end) = match_fence_closer(bytes, i, fch, min_run) {
                    out.push_str(&body[i..close_end]);
                    i = close_end;
                    active_fence = None;
                    at_line_start = true;
                    continue;
                }
            }
            let mut eol = i;
            while eol < len && bytes[eol] != b'\n' {
                eol += 1;
            }
            let line_end = if eol < len { eol + 1 } else { eol };
            out.push_str(&body[i..line_end]);
            i = line_end;
            at_line_start = true;
            continue;
        }

        // 2. Line-start fenced code block opener (```+ or ~~~+).
        if at_line_start {
            if let Some((fch, run, line_end)) = match_fence_opener(bytes, i) {
                out.push_str(&body[i..line_end]);
                i = line_end;
                active_fence = Some((fch, run));
                at_line_start = true;
                continue;
            }
        }

        // 3. CommonMark inline code span matching exact N-backtick opener/closer run lengths.
        if bytes[i] == b'`' {
            let mut open_end = i;
            while open_end < len && bytes[open_end] == b'`' {
                open_end += 1;
            }
            let run_len = open_end - i;
            if let Some(close_end) = find_matching_backtick_run(bytes, open_end, run_len) {
                out.push_str(&body[i..close_end]);
                i = close_end;
            } else {
                out.push_str(&body[i..open_end]);
                i = open_end;
            }
            at_line_start = false;
            continue;
        }

        // 4. Existing Markdown inline links `[label](dest)` copied verbatim.
        if bytes[i] == b'[' {
            if let Some(link_end) = find_markdown_inline_link_end(body, i) {
                out.push_str(&body[i..link_end]);
                i = link_end;
                at_line_start = false;
                continue;
            }
        }

        // 5. Bare URLs (`http://`, `https://`, `tg://`, `mailto:`) copied verbatim.
        if starts_with_bare_url_scheme(&body[i..])
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let mut u = i;
            for ch in body[i..].chars() {
                if ch.is_whitespace() || matches!(ch, ')' | ']' | '>' | '"' | '\'') {
                    break;
                }
                u += ch.len_utf8();
            }
            out.push_str(&body[i..u]);
            i = u;
            at_line_start = false;
            continue;
        }

        // 6. Standalone `c/<slug>` matching boundary + slug length + non-path constraints.
        if bytes[i] == b'c' && i + 2 < len && bytes[i + 1] == b'/' {
            let prev_ok = match body[..i].chars().next_back() {
                None => true,
                Some(prev) => {
                    prev.is_whitespace()
                        || !prev.is_ascii()
                        || matches!(prev, '(' | ',' | ';' | '!')
                }
            };
            if prev_ok {
                let mut end = i + 2;
                while end < len
                    && (bytes[end].is_ascii_alphanumeric()
                        || bytes[end] == b'_'
                        || bytes[end] == b'-')
                {
                    end += 1;
                }
                let slug = &body[i + 2..end];
                let slug_len = slug.len();
                let valid_len = (gpui_telegram_community::model::MIN_SLUG_LEN
                    ..=gpui_telegram_community::model::MAX_SLUG_LEN)
                    .contains(&slug_len);
                let has_alnum = slug.bytes().any(|b| b.is_ascii_alphanumeric());
                let followed_by_slash = bytes.get(end) == Some(&b'/');
                if valid_len && has_alnum && !followed_by_slash {
                    out.push_str("[c/");
                    out.push_str(slug);
                    out.push_str("](tg://community/");
                    out.push_str(slug);
                    out.push(')');
                    i = end;
                    at_line_start = false;
                    continue;
                }
            }
        }

        if let Some(ch) = body[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
            at_line_start = ch == '\n';
        } else {
            break;
        }
    }
    out
}

/// Formats compact counts (`0`, `999`, `1.2K`, `3.4M`), preserving negative Reddit scores cleanly.
pub fn format_compact_count(n: i64) -> String {
    let abs = n.unsigned_abs();
    let sign = if n < 0 { "-" } else { "" };
    if abs >= 1_000_000 {
        format!("{sign}{:.1}M", abs as f64 / 1_000_000.0)
    } else if abs >= 1_000 {
        format!("{sign}{:.1}K", abs as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

/// Formats unread badges (`0` hidden by callers, `>999` clamped to `"999+"`).
pub fn format_badge_count(count: u32) -> String {
    if count > 999 {
        "999+".to_string()
    } else {
        count.to_string()
    }
}

/// Formats relative elapsed time since `created_at_ms` in Chinese or English.
pub fn format_relative_time(created_at_ms: i64, lang: &str) -> String {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let diff_secs = ((now_ms - created_at_ms) / 1000).max(0);
    if diff_secs < 60 {
        tr_str(lang, "刚刚", "just now").to_string()
    } else if diff_secs < 3600 {
        let m = diff_secs / 60;
        if lang.starts_with("zh") {
            format!("{m}分钟前")
        } else {
            format!("{m}m")
        }
    } else if diff_secs < 86_400 {
        let h = diff_secs / 3600;
        if lang.starts_with("zh") {
            format!("{h}小时前")
        } else {
            format!("{h}h")
        }
    } else if diff_secs < 86_400 * 30 {
        let d = diff_secs / 86_400;
        if lang.starts_with("zh") {
            format!("{d}天前")
        } else {
            format!("{d}d")
        }
    } else {
        let mo = (diff_secs / (86_400 * 30)).max(1);
        if lang.starts_with("zh") {
            format!("{mo}个月前")
        } else {
            format!("{mo}mo")
        }
    }
}

/// Renders a Human or Bot first-class actor avatar without creating per-card GPUI `Entity` handles.
#[allow(clippy::too_many_arguments)]
pub fn render_actor_avatar(
    actor_id: &str,
    display_name: &str,
    kind: ActorKind,
    avatar_media_id: Option<&str>,
    avatar_shape: &str,
    avatar_color: &str,
    size_px: f32,
    surface_bg: u32,
    base_url: &str,
    paused: bool,
    pal: &TdesktopThemePalette,
) -> Div {
    let sz = size_px.max(18.0);
    if kind.is_bot_like() {
        let shape = if avatar_shape.trim().is_empty() {
            "blob"
        } else {
            avatar_shape
        };
        let color = if avatar_color.trim().is_empty() {
            let hash = gpui_telegram_community::fnv1a_64(actor_id.as_bytes());
            TdesktopThemePalette::bot_hue_name_for_id(hash)
        } else {
            avatar_color
        };
        return render_grok_persona_avatar(
            actor_id,
            shape,
            color,
            display_name,
            sz,
            GrokPersonaState::Idle,
            surface_bg,
            paused,
        );
    }
    if let Some(url) = resolve_community_media_url(base_url, avatar_media_id, None) {
        if let Some(src) = resolve_image_source(&url, None) {
            return div()
                .w(px(sz))
                .h(px(sz))
                .flex_shrink_0()
                .rounded_full()
                .overflow_hidden()
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .child(img(src).size_full().object_fit(ObjectFit::Cover));
        }
    }
    let fallback_hash = gpui_telegram_community::fnv1a_64(actor_id.as_bytes()) as i64;
    let hex = parse_fabushi_hex_color(avatar_color)
        .unwrap_or_else(|| pal.peer_avatar_color(fallback_hash));
    render_fabushi_shape_avatar("classic", hex, display_name, sz)
}

/// Renders compact inline citizenship & trust badges (`Verified`, `BOT`/`AI Agent`, `MOD`, `OP`).
#[allow(clippy::too_many_arguments)]
pub fn render_actor_badge_row(
    kind: ActorKind,
    verified_tier: VerifiedTier,
    bot_badge: Option<&str>,
    is_mod: bool,
    is_op: bool,
    avatar_color: &str,
    pal: &TdesktopThemePalette,
    lang: &str,
) -> Div {
    let is_dark = pal.mode.is_dark();
    let (_, bot_pill_bg, bot_pill_fg) = TdesktopThemePalette::bot_hue_colors(avatar_color, is_dark);

    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .flex_shrink_0()
        .when(verified_tier != VerifiedTier::None, |row| {
            let icon_color = match verified_tier {
                VerifiedTier::OfficialBot => 0x8B5CF6,
                VerifiedTier::VerifiedOrg => 0xF59E0B,
                VerifiedTier::CommunityMod => 0x10B981,
                _ => pal.accent,
            };
            row.child(tg_icon(IconName::Verified, 13.0, icon_color))
        })
        .when(kind.is_bot_like(), |row| {
            let label = bot_badge
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| match kind {
                    ActorKind::HybridAgent => tr_str(lang, "AI 智能体", "AI Agent").to_string(),
                    _ => "BOT".to_string(),
                });
            row.child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(5.0))
                    .bg(rgb(bot_pill_bg))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(bot_pill_fg))
                    .child(label),
            )
        })
        .when(is_mod, |row| {
            row.child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(5.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x10B981))
                    .child("MOD"),
            )
        })
        .when(is_op, |row| {
            row.child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(5.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(pal.accent))
                    .child("OP"),
            )
        })
}

/// Reddit-style horizontal `[▲ score ▼]` vote pill with active orange-red / periwinkle tinting.
pub fn render_vote_stepper_pill<V: 'static>(
    id_prefix: String,
    score: i64,
    viewer_vote: i8,
    pal: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_vote: impl Fn(&mut V, i8, &mut Context<V>) + Clone + 'static,
) -> Div {
    let up_color = if viewer_vote == 1 { 0xFF4500 } else { pal.text_secondary };
    let down_color = if viewer_vote == -1 { 0x7193FF } else { pal.text_secondary };
    let score_color = match viewer_vote {
        1 => 0xFF4500,
        -1 => 0x7193FF,
        _ => pal.text_primary,
    };
    let pill_bg = match viewer_vote {
        1 => TdesktopThemePalette::rgba_with_alpha(0xFF4500, 0.12),
        -1 => TdesktopThemePalette::rgba_with_alpha(0x7193FF, 0.14),
        _ => pal.service_pill_rgba(),
    };
    let on_up = on_vote.clone();
    let on_down = on_vote;
    let up_id = SharedString::from(format!("{id_prefix}-up"));
    let down_id = SharedString::from(format!("{id_prefix}-down"));

    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .px(px(4.0))
        .py(px(2.0))
        .rounded_full()
        .bg(pill_bg)
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .child(
            div()
                .id(up_id)
                .cursor_pointer()
                .w(px(22.0))
                .h(px(22.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(0xFF4500, 0.14)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_up(this, 1, cx);
                    }),
                )
                .child(tg_icon(IconName::UpvoteArrow, 14.0, up_color)),
        )
        .child(
            div()
                .px(px(3.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(score_color))
                .child(format_compact_count(score)),
        )
        .child(
            div()
                .id(down_id)
                .cursor_pointer()
                .w(px(22.0))
                .h(px(22.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(0x7193FF, 0.16)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_down(this, -1, cx);
                    }),
                )
                .child(tg_icon(IconName::DownvoteArrow, 14.0, down_color)),
        )
}

/// Renders a post or community flair pill chip.
pub fn render_flair_chip(text: &str, accent_hex: Option<&str>, pal: &TdesktopThemePalette) -> Div {
    let hex = accent_hex
        .and_then(parse_fabushi_hex_color)
        .unwrap_or(pal.accent);
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded_full()
        .bg(TdesktopThemePalette::rgba_with_alpha(hex, 0.14))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(hex, 0.28))
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(hex))
        .child(text.to_string())
}

/// Renders an NSFW or Spoiler click-to-reveal veil covering sensitive post body/media.
pub fn render_spoiler_veil<V: 'static>(
    id: SharedString,
    is_nsfw: bool,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<V>,
    on_reveal: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let label = if is_nsfw {
        tr_str(lang, "NSFW 敏感内容 · 点击查看", "NSFW Content · Click to reveal")
    } else {
        tr_str(lang, "剧透折叠 · 点击展开内容", "Spoiler · Click to reveal")
    };
    let badge_col = if is_nsfw { pal.danger } else { pal.accent };
    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .py(px(18.0))
        .px(px(14.0))
        .rounded(px(10.0))
        .bg(pal.service_pill_rgba())
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(badge_col, 0.35))
        .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(badge_col, 0.12)))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_reveal(this, cx);
            }),
        )
        .child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(rgb(badge_col))
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(if is_nsfw { 0xFFFFFF } else { pal.active_button_fg }))
                .child(if is_nsfw { "18+" } else { "SPOILER" }),
        )
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.text_secondary))
                .child(label),
        )
}

/// Renders lightweight shimmer-style placeholder cards while loading feed items.
pub fn render_loading_skeleton(count: usize, pal: &TdesktopThemePalette) -> Div {
    let bar = if pal.mode.is_dark() {
        TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
    } else {
        TdesktopThemePalette::rgba_with_alpha(0x000000, 0.07)
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .children((0..count).map(move |_| {
            div()
                .w_full()
                .p(px(14.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(div().w(px(34.0)).h(px(34.0)).rounded_full().bg(bar))
                        .child(div().w(px(140.0)).h(px(12.0)).rounded(px(6.0)).bg(bar)),
                )
                .child(div().w_full().h(px(14.0)).rounded(px(6.0)).bg(bar))
                .child(div().w(px(240.0)).h(px(12.0)).rounded(px(6.0)).bg(bar))
        }))
}

/// Renders a clean centered empty state panel.
pub fn render_empty_state(
    icon: IconName,
    title: &str,
    subtitle: &str,
    pal: &TdesktopThemePalette,
) -> Div {
    div()
        .w_full()
        .py(px(42.0))
        .px(px(20.0))
        .rounded(px(14.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .child(tg_icon(icon, 28.0, pal.text_secondary))
        .child(
            div()
                .text_size(px(14.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_primary))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(pal.text_secondary))
                .child(subtitle.to_string()),
        )
}

/// Renders committed text + underlined IME composition preedit + caret inside custom GPUI text fields.
pub fn render_input_display(
    text: &str,
    preedit: Option<&str>,
    placeholder: &str,
    is_focused: bool,
    pal: &TdesktopThemePalette,
) -> Div {
    let pre = preedit.unwrap_or("");
    let is_empty = text.is_empty() && pre.is_empty();
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .text_size(px(13.0))
        .when(is_empty, |el| {
            el.text_color(rgb(pal.text_secondary)).child(placeholder.to_string())
        })
        .when(!is_empty, |el| {
            el.text_color(rgb(pal.text_primary))
                .when(!text.is_empty(), |row| row.child(text.to_string()))
                .when(!pre.is_empty(), |row| {
                    row.child(
                        div()
                            .px(px(2.0))
                            .rounded(px(3.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18))
                            .border_b_1()
                            .border_color(rgb(pal.accent))
                            .text_color(rgb(pal.text_primary))
                            .child(pre.to_string()),
                    )
                })
        })
        .when(is_focused, |el| {
            el.child(div().w(px(1.5)).h(px(15.0)).ml(px(1.0)).bg(rgb(pal.accent)))
        })
}

/// Renders the persistent 1-click Human <-> Owned Bot Identity Switcher popover.
pub fn render_identity_switcher_popover(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active_id = ws.state.active_actor_id.clone();
    let base_url = ws.media_base_url.clone();
    let (human_actor, bot_actors) = match ws.state.identities.as_ref() {
        Some(ids) => (ids.human_actor.clone(), ids.bot_actors.clone()),
        None => (None, Vec::new()),
    };
    let hover_bg = pal.row_hover_rgba();

    let mut card = macos_popover_surface("community-identity-popover", pal)
        .w(px(pal.scaled_px(292.0)))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(6.0))
                .pb(px(4.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "切换发帖与互动身份 (Act As)", "Switch Active Identity (Act As)")),
                ),
        );

    let mut actors = Vec::new();
    if let Some(h) = human_actor {
        actors.push(h);
    }
    actors.extend(bot_actors.iter().cloned());

    for actor in actors {
        let is_selected = actor.actor_id == active_id;
        let aid = actor.actor_id.clone();
        let row_id = SharedString::from(format!("identity-switch-{}", actor.actor_id));
        let karma = actor.post_karma + actor.comment_karma;
        card = card.child(
            div()
                .id(row_id)
                .cursor_pointer()
                .w_full()
                .px(px(8.0))
                .py(px(7.0))
                .rounded(px(9.0))
                .when(is_selected, |el| el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14)))
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .gap(px(9.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.switch_active_actor(aid.clone(), cx);
                    }),
                )
                .child(render_actor_avatar(
                    &actor.actor_id,
                    &actor.display_name,
                    actor.actor_kind,
                    actor.avatar_media_id.as_deref(),
                    &actor.avatar_shape,
                    &actor.avatar_color,
                    30.0,
                    pal.menu_bg,
                    &base_url,
                    true,
                    &pal,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(actor.display_name.clone()),
                                )
                                .child(render_actor_badge_row(
                                    actor.actor_kind,
                                    actor.verified_tier,
                                    actor.bot_badge.as_deref(),
                                    false,
                                    false,
                                    &actor.avatar_color,
                                    &pal,
                                    lang,
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("@{} · {} karma", actor.handle, format_compact_count(karma))),
                        ),
                )
                .when(is_selected, |el| el.child(tg_icon(IconName::SelectCheck, 14.0, pal.accent))),
        );
    }

    if bot_actors.is_empty() {
        card = card.child(
            div()
                .mt(px(4.0))
                .p(px(10.0))
                .rounded(px(9.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "还没有绑定 Bot 身份。在「消息」中直接与 @BotFather 对话即可创建并管理 Bot。",
                            "No bots linked yet. Chat directly with @BotFather in Messages to create and manage bots.",
                        )),
                ),
        );
    }

    card.into_any_element()
}
