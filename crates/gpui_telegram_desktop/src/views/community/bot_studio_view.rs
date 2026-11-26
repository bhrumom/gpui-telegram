//! Feature F21-Bot Studio — First-Class Bot Citizenship Studio:
//! Owned & Community Bots Directory, Register New Bot Form (with 8×11 `GrokBotAvatar`
//! Live Customizer), Selected Owned Bot Editor, Bot API Token & `/bot/{token}/community.createPost`
//! Snippet Card (with real Clipboard Copy), Live Test-Post / Trigger-Bot-Reply Playground,
//! and Bot Karma & Activity Analytics.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::normalize_handle;
use gpui_telegram_proto::community_contracts::{
    ActorKind, BotAutonomyMode, CommunityActorDto, CreatePostRequest, FeedItemDto, PostFormat,
    PostStatus, RegisterBotActorRequest, SummonBotRequest, UpdateCommunityActorRequest,
};
use gpui_telegram_proto::miniapp_contracts::{GROK_AVATAR_COLOR_IDS, GROK_AVATAR_SHAPES};

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, trigger_grok_avatar_bounce, GrokBotAvatarProps, GROK_COLOR_PALETTE,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::shared_widgets::{
    format_compact_count, render_actor_avatar, render_actor_badge_row, render_input_display,
};
use super::state::{ActiveCommunityInput, BotStudioEditDraft, CommunityWorkspaceAction};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

const PRESET_CAPABILITIES: [&str; 6] = [
    "community",
    "debate",
    "code_review",
    "research",
    "summarize",
    "moderation",
];

/// Clamps arbitrary shape & color indices safely into the `8 × 11` Grok avatar palette
/// without ever panicking on out-of-range inputs.
pub fn clamp_grok_avatar_indices(shape_idx: usize, color_idx: usize) -> (usize, usize) {
    let s = shape_idx.min(GROK_AVATAR_SHAPES.len().saturating_sub(1));
    let c = color_idx.min(GROK_AVATAR_COLOR_IDS.len().saturating_sub(1));
    (s, c)
}

/// Resolves safe `(shape, color_id)` string literals from arbitrary indices.
pub fn resolve_grok_avatar_pair(shape_idx: usize, color_idx: usize) -> (&'static str, &'static str) {
    let (s, c) = clamp_grok_avatar_indices(shape_idx, color_idx);
    (GROK_AVATAR_SHAPES[s], GROK_AVATAR_COLOR_IDS[c])
}

/// Validates Bot Citizenship Studio registration inputs (display name, `@handle` rules,
/// cooldown seconds, and hourly rate limit).
pub fn validate_bot_studio_registration(
    display_name: &str,
    raw_handle: &str,
    cooldown_raw: &str,
    max_posts_raw: &str,
) -> Result<(String, u32, u32), String> {
    if display_name.trim().is_empty() {
        return Err("Bot display name is required".to_string());
    }
    let clean_handle = normalize_handle(raw_handle).map_err(|e| e.to_string())?;
    if clean_handle.len() < 5 || !clean_handle.ends_with("bot") {
        return Err(
            "Bot @handle must be at least 5 characters and end with 'bot' (e.g. rust_critic_bot)"
                .to_string(),
        );
    }
    let cooldown: u32 = if cooldown_raw.trim().is_empty() {
        30
    } else {
        let parsed = cooldown_raw
            .trim()
            .parse::<u32>()
            .map_err(|_| "Cooldown seconds must be an integer in 0..=86400".to_string())?;
        if parsed > 86_400 {
            return Err("Cooldown seconds must be between 0 and 86400".to_string());
        }
        parsed
    };
    let max_posts: u32 = if max_posts_raw.trim().is_empty() {
        12
    } else {
        let parsed = max_posts_raw
            .trim()
            .parse::<u32>()
            .map_err(|_| "Max posts/hour must be an integer in 1..=500".to_string())?;
        if !(1..=500).contains(&parsed) {
            return Err("Max posts/hour must be between 1 and 500".to_string());
        }
        parsed
    };
    Ok((clean_handle, cooldown, max_posts))
}

/// Resolves a bot's API token strictly from the per-actor token map (never leaking another bot's token).
pub fn resolve_bot_studio_token_for_actor(
    tokens_by_actor: &std::collections::HashMap<String, String>,
    actor_id: &str,
) -> Option<String> {
    tokens_by_actor.get(actor_id).cloned()
}

/// Formats a ready-to-run `curl` snippet targeting `/bot/{token}/community.createPost`.
pub fn format_bot_api_snippet(base_url: &str, token: Option<&str>, community_slug: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    let tok = token.unwrap_or("<LOCAL_BOT_TOKEN>");
    let comm = community_slug.unwrap_or("rust");
    format!(
        "curl -X POST \"{base}/bot/{tok}/community.createPost\" \\\n  -H \"Content-Type: application/json\" \\\n  -d '{{\"community_slug\":\"{comm}\",\"format\":\"micro\",\"body\":\"Hello from my first-class Bot citizen!\"}}'"
    )
}

/// Formats a visible `curl` snippet for UI rendering, masking the secret token unless `revealed == true`.
pub fn format_bot_api_snippet_for_display(
    base_url: &str,
    token: Option<&str>,
    revealed: bool,
    community_slug: Option<&str>,
) -> String {
    let display_tok = token.map(|tok| {
        if revealed {
            tok.to_string()
        } else {
            mask_bot_token(tok)
        }
    });
    format_bot_api_snippet(base_url, display_tok.as_deref(), community_slug)
}

fn mask_bot_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 10 {
        return "••••••••".to_string();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}••••••••{tail}")
}

fn autonomy_choice_label(mode: BotAutonomyMode, lang: &str) -> &'static str {
    match mode {
        BotAutonomyMode::Manual => tr_str(lang, "手动 (Manual)", "Manual"),
        BotAutonomyMode::MentionOnly => tr_str(lang, "@提及 (MentionOnly)", "MentionOnly"),
        BotAutonomyMode::Reactive => tr_str(lang, "响应 (Reactive)", "Reactive"),
        BotAutonomyMode::Autonomous => tr_str(lang, "自主 (Autonomous)", "Autonomous"),
    }
}

impl CommunityWorkspace {
    /// Resolves the currently selected owned bot (or first owned bot fallback).
    pub fn resolve_selected_owned_bot(&self) -> Option<CommunityActorDto> {
        let bots = &self.state.identities.as_ref()?.bot_actors;
        self.state
            .bot_studio_selected_bot_id
            .as_deref()
            .and_then(|id| bots.iter().find(|b| b.actor_id == id).cloned())
            .or_else(|| bots.first().cloned())
    }

    /// Hydrates `bot_studio_edit` from `bot` only when `bot_studio_edit.actor_id != bot.actor_id`,
    /// preserving any in-progress unhydrated user edits if `actor_id` was previously empty.
    pub fn ensure_bot_studio_edit_hydrated(&mut self, bot: &CommunityActorDto) {
        if self.state.bot_studio_edit.actor_id == bot.actor_id {
            return;
        }
        let prev = self.state.bot_studio_edit.clone();
        let shape_idx = GROK_AVATAR_SHAPES
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&bot.avatar_shape))
            .unwrap_or(0);
        let color_idx = GROK_AVATAR_COLOR_IDS
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&bot.avatar_color))
            .unwrap_or(0);
        self.state.bot_studio_edit_save_seq =
            self.state.bot_studio_edit_save_seq.wrapping_add(1);
        self.state.bot_studio_selected_bot_id = Some(bot.actor_id.clone());
        let mut draft = BotStudioEditDraft {
            actor_id: bot.actor_id.clone(),
            display_name: bot.display_name.clone(),
            bio: bot.bio.clone(),
            bot_badge: bot.bot_badge.clone().unwrap_or_default(),
            bot_model: bot.bot_model.clone().unwrap_or_default(),
            bot_capabilities: bot.bot_capabilities.clone(),
            custom_cap_draft: String::new(),
            bot_persona: bot.bot_persona.clone().unwrap_or_default(),
            bot_autonomy_mode: bot.bot_autonomy_mode,
            shape_idx,
            color_idx,
            shape_edited: false,
            color_edited: false,
            autonomy_edited: false,
            saving: false,
            error: None,
        };
        if prev.actor_id.is_empty() {
            if !prev.display_name.is_empty() {
                draft.display_name = prev.display_name;
            }
            if !prev.bio.is_empty() {
                draft.bio = prev.bio;
            }
            if !prev.bot_badge.is_empty() {
                draft.bot_badge = prev.bot_badge;
            }
            if !prev.bot_model.is_empty() {
                draft.bot_model = prev.bot_model;
            }
            if !prev.bot_persona.is_empty() {
                draft.bot_persona = prev.bot_persona;
            }
            if !prev.bot_capabilities.is_empty() {
                draft.bot_capabilities = prev.bot_capabilities;
            }
            if !prev.custom_cap_draft.is_empty() {
                draft.custom_cap_draft = prev.custom_cap_draft;
            }
            if prev.shape_edited || prev.shape_idx != 0 {
                draft.shape_idx = clamp_grok_avatar_indices(prev.shape_idx, 0).0;
                draft.shape_edited = true;
            }
            if prev.color_edited || prev.color_idx != 0 {
                draft.color_idx = clamp_grok_avatar_indices(0, prev.color_idx).1;
                draft.color_edited = true;
            }
            if prev.autonomy_edited || prev.bot_autonomy_mode != BotAutonomyMode::Manual {
                draft.bot_autonomy_mode = prev.bot_autonomy_mode;
                draft.autonomy_edited = true;
            }
        }
        self.state.bot_studio_edit = draft;
    }

    /// Selects an owned bot in Bot Studio and hydrates its edit draft cleanly.
    pub fn select_bot_for_studio(&mut self, actor: CommunityActorDto, cx: &mut Context<Self>) {
        self.state.bot_studio_edit_save_seq =
            self.state.bot_studio_edit_save_seq.wrapping_add(1);
        let shape_idx = GROK_AVATAR_SHAPES
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&actor.avatar_shape))
            .unwrap_or(0);
        let color_idx = GROK_AVATAR_COLOR_IDS
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&actor.avatar_color))
            .unwrap_or(0);
        self.state.bot_studio_selected_bot_id = Some(actor.actor_id.clone());
        self.state.bot_studio_edit = BotStudioEditDraft {
            actor_id: actor.actor_id,
            display_name: actor.display_name,
            bio: actor.bio,
            bot_badge: actor.bot_badge.unwrap_or_default(),
            bot_model: actor.bot_model.unwrap_or_default(),
            bot_capabilities: actor.bot_capabilities,
            custom_cap_draft: String::new(),
            bot_persona: actor.bot_persona.unwrap_or_default(),
            bot_autonomy_mode: actor.bot_autonomy_mode,
            shape_idx,
            color_idx,
            shape_edited: false,
            color_edited: false,
            autonomy_edited: false,
            saving: false,
            error: None,
        };
        self.state.bot_studio_token_revealed = false;
        cx.notify();
    }

    pub fn set_bot_studio_avatar_indices(
        &mut self,
        shape_idx: usize,
        color_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let (s, c) = clamp_grok_avatar_indices(shape_idx, color_idx);
        self.state.bot_studio_shape_idx = s;
        self.state.bot_studio_color_idx = c;
        trigger_grok_avatar_bounce("studio-new-bot-preview");
        cx.notify();
    }

    pub fn set_bot_studio_edit_avatar_indices(
        &mut self,
        shape_idx: usize,
        color_idx: usize,
        cx: &mut Context<Self>,
    ) {
        if self.state.bot_studio_edit.actor_id.is_empty() {
            if let Some(bot) = self.resolve_selected_owned_bot() {
                self.ensure_bot_studio_edit_hydrated(&bot);
            }
        }
        let (s, c) = clamp_grok_avatar_indices(shape_idx, color_idx);
        self.state.bot_studio_edit.shape_idx = s;
        self.state.bot_studio_edit.color_idx = c;
        self.state.bot_studio_edit.shape_edited = true;
        self.state.bot_studio_edit.color_edited = true;
        let preview_id = format!("studio-edit-{}", self.state.bot_studio_edit.actor_id);
        trigger_grok_avatar_bounce(&preview_id);
        cx.notify();
    }

    pub fn set_bot_studio_edit_autonomy(
        &mut self,
        mode: BotAutonomyMode,
        cx: &mut Context<Self>,
    ) {
        if self.state.bot_studio_edit.actor_id.is_empty() {
            if let Some(bot) = self.resolve_selected_owned_bot() {
                self.ensure_bot_studio_edit_hydrated(&bot);
            }
        }
        self.state.bot_studio_edit.bot_autonomy_mode = mode;
        self.state.bot_studio_edit.autonomy_edited = true;
        cx.notify();
    }

    pub fn toggle_bot_studio_capability(&mut self, cap: String, cx: &mut Context<Self>) {
        let norm = cap.trim().to_ascii_lowercase();
        if norm.is_empty() {
            return;
        }
        if let Some(pos) = self
            .state
            .bot_studio_capabilities
            .iter()
            .position(|x| x.eq_ignore_ascii_case(&norm))
        {
            self.state.bot_studio_capabilities.remove(pos);
        } else {
            self.state.bot_studio_capabilities.push(norm);
        }
        cx.notify();
    }

    pub fn add_bot_studio_custom_capability(&mut self, cx: &mut Context<Self>) {
        let raw = self.state.bot_studio_custom_cap_draft.trim().to_ascii_lowercase();
        if !raw.is_empty()
            && !self
                .state
                .bot_studio_capabilities
                .iter()
                .any(|x| x.eq_ignore_ascii_case(&raw))
        {
            self.state.bot_studio_capabilities.push(raw);
        }
        self.state.bot_studio_custom_cap_draft.clear();
        cx.notify();
    }

    pub fn toggle_bot_studio_edit_capability(&mut self, cap: String, cx: &mut Context<Self>) {
        if self.state.bot_studio_edit.actor_id.is_empty() {
            if let Some(bot) = self.resolve_selected_owned_bot() {
                self.ensure_bot_studio_edit_hydrated(&bot);
            }
        }
        let norm = cap.trim().to_ascii_lowercase();
        if norm.is_empty() {
            return;
        }
        if let Some(pos) = self
            .state
            .bot_studio_edit
            .bot_capabilities
            .iter()
            .position(|x| x.eq_ignore_ascii_case(&norm))
        {
            self.state.bot_studio_edit.bot_capabilities.remove(pos);
        } else {
            self.state.bot_studio_edit.bot_capabilities.push(norm);
        }
        cx.notify();
    }

    pub fn add_bot_studio_edit_custom_capability(&mut self, cx: &mut Context<Self>) {
        if self.state.bot_studio_edit.actor_id.is_empty() {
            if let Some(bot) = self.resolve_selected_owned_bot() {
                self.ensure_bot_studio_edit_hydrated(&bot);
            }
        }
        let raw = self
            .state
            .bot_studio_edit
            .custom_cap_draft
            .trim()
            .to_ascii_lowercase();
        if !raw.is_empty()
            && !self
                .state
                .bot_studio_edit
                .bot_capabilities
                .iter()
                .any(|x| x.eq_ignore_ascii_case(&raw))
        {
            self.state.bot_studio_edit.bot_capabilities.push(raw);
        }
        self.state.bot_studio_edit.custom_cap_draft.clear();
        cx.notify();
    }
}

impl CommunityWorkspace {
    /// Registers a brand-new first-class Bot citizen with validation, 8×11 Grok avatar,
    /// capabilities, persona, autonomy mode, and captures `RegisterBotActorResult`.
    pub fn submit_register_bot(&mut self, cx: &mut Context<Self>) {
        if self.state.bot_studio_submitting {
            return;
        }
        let name = self.state.bot_studio_name.trim().to_string();
        let validated = validate_bot_studio_registration(
            &name,
            &self.state.bot_studio_handle,
            &self.state.bot_studio_cooldown_secs,
            &self.state.bot_studio_max_posts_per_hour,
        );
        let (clean_handle, cooldown_secs, max_posts_per_hour) = match validated {
            Ok(v) => v,
            Err(err) => {
                self.state.bot_studio_error = Some(err);
                cx.notify();
                return;
            }
        };
        let (shape, color) = resolve_grok_avatar_pair(
            self.state.bot_studio_shape_idx,
            self.state.bot_studio_color_idx,
        );
        let persona = {
            let p = self.state.bot_studio_persona.trim().to_string();
            if p.is_empty() { None } else { Some(p) }
        };
        let bio = {
            let b = self.state.bot_studio_bio.trim().to_string();
            if b.is_empty() { persona.clone() } else { Some(b) }
        };
        let badge = {
            let b = self.state.bot_studio_badge.trim().to_string();
            if b.is_empty() { Some("AI Agent".to_string()) } else { Some(b) }
        };
        let model = {
            let m = self.state.bot_studio_model.trim().to_string();
            if m.is_empty() { Some("Fabushi-1".to_string()) } else { Some(m) }
        };
        let caps = if self.state.bot_studio_capabilities.is_empty() {
            vec!["community".to_string()]
        } else {
            self.state.bot_studio_capabilities.clone()
        };

        let req = RegisterBotActorRequest {
            bot_user_id: None,
            actor_kind: Some(ActorKind::Bot),
            handle: clean_handle,
            display_name: name,
            bio,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: Some(shape.to_string()),
            avatar_color: Some(color.to_string()),
            bot_badge: badge,
            bot_model: model,
            bot_capabilities: caps,
            bot_autonomy_mode: self.state.bot_studio_autonomy,
            bot_persona: persona,
            cooldown_secs: Some(cooldown_secs),
            max_posts_per_hour: Some(max_posts_per_hour),
        };
        self.state.bot_studio_submitting = true;
        self.state.bot_studio_error = None;
        self.state.identities_gen = self.state.identities_gen.wrapping_add(1);
        let my_id_gen = self.state.identities_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let created = facade.register_bot_actor(&req).await;
                let ids = facade.get_identities().await.ok();
                (created, ids)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                this.state.bot_studio_submitting = false;
                match res {
                    Ok((Ok(reg), ids)) => {
                        this.state.bot_studio_name.clear();
                        this.state.bot_studio_handle.clear();
                        this.state.bot_studio_bio.clear();
                        this.state.bot_studio_persona.clear();
                        if let Some(ref tok) = reg.bot_token {
                            this.state
                                .bot_studio_tokens_by_actor
                                .insert(reg.actor.actor_id.clone(), tok.clone());
                        }
                        this.state.bot_studio_last_token = reg.bot_token.clone();
                        this.state
                            .known_owned_actor_ids
                            .insert(reg.actor.actor_id.clone());
                        if this.state.identities_gen == my_id_gen {
                            if let Some(i) = ids {
                                this.state.identities = Some(i);
                            }
                        }
                        this.select_bot_for_studio(reg.actor, cx);
                        let lang = this.language_id.clone();
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(
                                    &lang,
                                    "Bot 社区公民注册成功！现可管理、编辑或在身份切换器中选择它发帖。",
                                    "Bot citizen registered! Manage it below or select it in the Identity Switcher.",
                                )
                                .to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok((Err(e), _)) => {
                        this.state.bot_studio_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.bot_studio_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Synchronizes `bot_studio_edit` metadata fields when an external surface (`submit_edit_profile`)
    /// commits newer metadata for the same selected bot, without touching `bot_studio_edit.saving`
    /// or `bot_studio_edit_save_seq`.
    pub(super) fn sync_bot_studio_edit_metadata_if_matching(&mut self, actor: &CommunityActorDto) {
        if self.state.bot_studio_edit.actor_id != actor.actor_id {
            return;
        }
        let shape_idx = GROK_AVATAR_SHAPES
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&actor.avatar_shape))
            .unwrap_or(self.state.bot_studio_edit.shape_idx);
        let color_idx = GROK_AVATAR_COLOR_IDS
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&actor.avatar_color))
            .unwrap_or(self.state.bot_studio_edit.color_idx);
        self.state.bot_studio_edit.display_name = actor.display_name.clone();
        self.state.bot_studio_edit.bio = actor.bio.clone();
        self.state.bot_studio_edit.bot_badge = actor.bot_badge.clone().unwrap_or_default();
        self.state.bot_studio_edit.bot_model = actor.bot_model.clone().unwrap_or_default();
        self.state.bot_studio_edit.bot_capabilities = actor.bot_capabilities.clone();
        self.state.bot_studio_edit.bot_persona = actor.bot_persona.clone().unwrap_or_default();
        self.state.bot_studio_edit.bot_autonomy_mode = actor.bot_autonomy_mode;
        self.state.bot_studio_edit.shape_idx = shape_idx;
        self.state.bot_studio_edit.color_idx = color_idx;
    }

    /// Saves edits to the currently selected owned Bot via `facade.update_actor`.
    pub fn save_selected_bot_edits(&mut self, cx: &mut Context<Self>) {
        if self.state.bot_studio_edit.saving {
            return;
        }
        if self.state.bot_studio_edit.actor_id.trim().is_empty() {
            if let Some(bot) = self.resolve_selected_owned_bot() {
                self.ensure_bot_studio_edit_hydrated(&bot);
            }
        }
        let actor_id = self.state.bot_studio_edit.actor_id.trim().to_string();
        let display_name = self.state.bot_studio_edit.display_name.trim().to_string();
        if actor_id.is_empty() || display_name.is_empty() {
            let lang = self.language_id.as_str();
            self.state.bot_studio_edit.error = Some(
                tr_str(lang, "Bot 显示名称不能为空", "Bot display name cannot be empty")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        let (shape, color) = resolve_grok_avatar_pair(
            self.state.bot_studio_edit.shape_idx,
            self.state.bot_studio_edit.color_idx,
        );
        let req = UpdateCommunityActorRequest {
            display_name: Some(display_name),
            bio: Some(self.state.bot_studio_edit.bio.trim().to_string()),
            avatar_shape: Some(shape.to_string()),
            avatar_color: Some(color.to_string()),
            bot_badge: Some(self.state.bot_studio_edit.bot_badge.trim().to_string()),
            bot_model: Some(self.state.bot_studio_edit.bot_model.trim().to_string()),
            bot_capabilities: Some(self.state.bot_studio_edit.bot_capabilities.clone()),
            bot_autonomy_mode: Some(self.state.bot_studio_edit.bot_autonomy_mode),
            bot_persona: Some(self.state.bot_studio_edit.bot_persona.trim().to_string()),
            ..Default::default()
        };
        self.state.bot_studio_edit_save_seq =
            self.state.bot_studio_edit_save_seq.wrapping_add(1);
        let my_save_seq = self.state.bot_studio_edit_save_seq;
        self.state.bot_studio_edit.saving = true;
        self.state.bot_studio_edit.error = None;
        self.state.identities_gen = self.state.identities_gen.wrapping_add(1);
        let my_id_gen = self.state.identities_gen;
        let actor_edit_entry = self
            .state
            .actor_edit_seq
            .entry(actor_id.clone())
            .or_insert(0);
        *actor_edit_entry = actor_edit_entry.wrapping_add(1);
        let my_actor_edit_seq = *actor_edit_entry;
        let actor_id_cb = actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let updated = facade.update_actor(&actor_id, &req).await;
                let ids = facade.get_identities().await.ok();
                (updated, ids)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                let lang = this.language_id.clone();
                let still_current_save = this.state.bot_studio_edit_save_seq == my_save_seq
                    && this.state.bot_studio_selected_bot_id.as_deref()
                        == Some(actor_id_cb.as_str());
                let latest_identities = this.state.identities_gen == my_id_gen;
                let is_latest_actor_edit = this
                    .state
                    .actor_edit_seq
                    .get(&actor_id_cb)
                    .copied()
                    == Some(my_actor_edit_seq);
                if still_current_save {
                    this.state.bot_studio_edit.saving = false;
                }
                match res {
                    Ok((Ok(updated), ids)) => {
                        if latest_identities {
                            if let Some(i) = ids {
                                this.state.identities = Some(i);
                            }
                        }
                        if is_latest_actor_edit {
                            this.state.patch_edited_actor_metadata(&updated);
                        }
                        this.reload_actor_route_if_loading(cx);
                        if still_current_save && is_latest_actor_edit {
                            this.select_bot_for_studio(updated, cx);
                        }
                        if is_latest_actor_edit {
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: tr_str(
                                        &lang,
                                        "Bot 资料与外观已更新",
                                        "Bot profile & avatar updated",
                                    )
                                    .to_string(),
                                    is_error: false,
                                },
                                cx,
                            );
                        }
                    }
                    Ok((Err(e), _)) => {
                        if still_current_save {
                            this.state.bot_studio_edit.error = Some(e.to_string());
                        }
                    }
                    Err(e) => {
                        if still_current_save {
                            this.state.bot_studio_edit.error = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn copy_bot_api_snippet(&mut self, snippet: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(snippet));
        let lang = self.language_id.clone();
        self.emit_action(
            CommunityWorkspaceAction::ShowToast {
                text: tr_str(
                    &lang,
                    "Bot API 调用示例已复制到剪贴板",
                    "Bot API snippet copied to clipboard",
                )
                .to_string(),
                is_error: false,
            },
            cx,
        );
    }

    pub fn copy_bot_token(&mut self, token: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(token));
        let lang = self.language_id.clone();
        self.emit_action(
            CommunityWorkspaceAction::ShowToast {
                text: tr_str(&lang, "Bot Token 已复制到剪贴板", "Bot token copied to clipboard")
                    .to_string(),
                is_error: false,
            },
            cx,
        );
    }

    /// Publishes a live test post under the selected owned Bot identity, restoring the viewer's
    /// previous active actor cleanly afterward.
    pub fn run_bot_studio_test_post(&mut self, bot_actor_id: String, cx: &mut Context<Self>) {
        if self.state.bot_studio_test_submitting {
            return;
        }
        let body = {
            let trimmed = self.state.bot_studio_test_content.trim().to_string();
            if trimmed.is_empty() {
                "Hello community! Live test post published from Bot Citizenship Studio.".to_string()
            } else {
                trimmed
            }
        };
        let community_slug = self.state.bot_studio_test_community.clone();
        let req = CreatePostRequest {
            community_slug,
            format: PostFormat::Micro,
            body,
            status: PostStatus::Published,
            ..Default::default()
        };
        self.state.bot_studio_test_submitting = true;
        self.state.bot_studio_test_feedback = None;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.create_post_as(&bot_actor_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                this.state.bot_studio_test_submitting = false;
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(created)) => {
                        let pid = created.post.post_id.clone();
                        this.state.bot_studio_test_content.clear();
                        this.state.bot_studio_test_last_post_id = Some(pid.clone());
                        this.state.feed_items.insert(
                            0,
                            FeedItemDto {
                                post: created.post,
                                explanation: None,
                                rank_score: 100.0,
                                is_exploration_slot: false,
                            },
                        );
                        if this.state.feed_loading || this.state.feed_loading_more {
                            this.load_feed(cx);
                        }
                        let msg = if lang.starts_with("zh") {
                            format!("Bot 测试帖子已发布 ({pid})")
                        } else {
                            format!("Bot test post published ({pid})")
                        };
                        this.state.bot_studio_test_feedback = Some((msg.clone(), false));
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: msg,
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        let msg = e.to_string();
                        this.state.bot_studio_test_feedback = Some((msg.clone(), true));
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: msg,
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(e) => {
                        this.state.bot_studio_test_feedback = Some((e, true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Triggers a live bot reply (`facade.summon_bot`) on the latest community post.
    pub fn run_bot_studio_trigger_reply(&mut self, bot_actor_id: String, cx: &mut Context<Self>) {
        if self.state.bot_studio_test_submitting {
            return;
        }
        let target_post_id = self
            .state
            .bot_studio_test_last_post_id
            .clone()
            .or_else(|| {
                self.state
                    .feed_items
                    .first()
                    .map(|it| it.post.post_id.clone())
            });
        let Some(post_id) = target_post_id else {
            let lang = self.language_id.clone();
            self.state.bot_studio_test_feedback = Some((
                tr_str(
                    &lang,
                    "请先发布一篇测试帖子或加载信息流后再触发 Bot 回复",
                    "Publish a test post or load the feed before triggering a bot reply",
                )
                .to_string(),
                true,
            ));
            cx.notify();
            return;
        };
        let prompt = {
            let p = self.state.bot_studio_test_content.trim().to_string();
            if p.is_empty() {
                Some("Share an insightful reply from Bot Studio playground.".to_string())
            } else {
                Some(p)
            }
        };
        self.state.bot_studio_test_submitting = true;
        self.state.bot_studio_test_feedback = None;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let jump_pid = post_id.clone();
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .summon_bot(
                        &post_id,
                        &SummonBotRequest {
                            bot_actor_id: Some(bot_actor_id),
                            bot_handle: None,
                            prompt,
                            parent_comment_id: None,
                        },
                    )
                    .await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                this.state.bot_studio_test_submitting = false;
                match res {
                    Ok(Ok(resp)) => {
                        if resp.triggered {
                            this.reload_detail_if_loading_post(&jump_pid, true, cx);
                        }
                        this.state.bot_studio_test_last_post_id = Some(jump_pid);
                        let is_err = !resp.triggered;
                        let msg = if resp.triggered {
                            format!("Bot replied: {}", resp.reason)
                        } else {
                            format!("Bot did not reply: {}", resp.reason)
                        };
                        this.state.bot_studio_test_feedback = Some((msg.clone(), is_err));
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: msg,
                                is_error: is_err,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        let msg = e.to_string();
                        this.state.bot_studio_test_feedback = Some((msg.clone(), true));
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: msg,
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(e) => {
                        this.state.bot_studio_test_feedback = Some((e, true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn render_studio_input_box(
    id: &'static str,
    target: ActiveCommunityInput,
    value: &str,
    placeholder: &str,
    multiline: bool,
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    render_studio_input_box_with_hydrate(id, target, value, placeholder, multiline, None, ws, cx)
}

#[allow(clippy::too_many_arguments)]
fn render_studio_input_box_with_hydrate(
    id: &'static str,
    target: ActiveCommunityInput,
    value: &str,
    placeholder: &str,
    multiline: bool,
    hydrate_bot: Option<CommunityActorDto>,
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let pal = ws.palette;
    let foc = ws.state.active_input == target;
    let preedit = if foc {
        ws.state.ime_preedit.as_deref()
    } else {
        None
    };
    div()
        .id(id)
        .cursor_text()
        .w_full()
        .when(multiline, |d| d.min_h(px(46.0)))
        .px(px(9.0))
        .py(px(6.5))
        .rounded(px(8.0))
        .bg(rgb(pal.input_bg))
        .border_1()
        .border_color(if foc {
            rgb(pal.accent).into()
        } else {
            pal.hairline_border_rgba()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if let Some(ref bot) = hydrate_bot {
                    this.ensure_bot_studio_edit_hydrated(bot);
                }
                this.state.focus_input(target);
                cx.notify();
            }),
        )
        .child(render_input_display(value, preedit, placeholder, foc, &pal))
}

/// Interactive 8-Shape × 11-Hue `GrokBotAvatar` live customizer widget.
#[allow(clippy::too_many_arguments)]
fn render_grok_avatar_customizer(
    id_prefix: &'static str,
    preview_source_id: &str,
    shape_idx: usize,
    color_idx: usize,
    is_edit: bool,
    hydrate_bot: Option<CommunityActorDto>,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let (s_idx, c_idx) = clamp_grok_avatar_indices(shape_idx, color_idx);
    let shape = GROK_AVATAR_SHAPES[s_idx];
    let color = GROK_AVATAR_COLOR_IDS[c_idx];

    div()
        .p(px(10.0))
        .rounded(px(10.0))
        .bg(pal.service_pill_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(render_grok_bot_avatar(
                    GrokBotAvatarProps::new(preview_source_id, shape, color, 52.0)
                        .with_surface_bg(pal.card_bg)
                        .with_paused(true),
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(
                                    lang,
                                    "Grok 动态形象定制器 (8 形态 × 11 色系)",
                                    "Grok Dynamic Avatar Customizer (8 Shapes × 11 Hues)",
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{shape} · {color}")),
                        ),
                ),
        )
        // 8 Shapes Row
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(GROK_AVATAR_SHAPES.iter().enumerate().map(|(idx, sh)| {
                    let selected = idx == s_idx;
                    let hb = hydrate_bot.clone();
                    div()
                        .id(SharedString::from(format!("{id_prefix}-shape-{idx}")))
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(if selected {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.20)
                        } else {
                            pal.card_surface_rgba()
                        })
                        .border_1()
                        .border_color(if selected {
                            rgb(pal.accent).into()
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if is_edit {
                                    if let Some(ref b) = hb {
                                        this.ensure_bot_studio_edit_hydrated(b);
                                    }
                                    this.set_bot_studio_edit_avatar_indices(
                                        idx,
                                        this.state.bot_studio_edit.color_idx,
                                        cx,
                                    );
                                } else {
                                    this.set_bot_studio_avatar_indices(
                                        idx,
                                        this.state.bot_studio_color_idx,
                                        cx,
                                    );
                                }
                            }),
                        )
                        .child(render_grok_bot_avatar(
                            GrokBotAvatarProps::new(format!("{id_prefix}-s-{idx}"), *sh, color, 20.0)
                                .with_surface_bg(pal.card_bg)
                                .with_paused(true),
                        ))
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_primary))
                                .child(*sh),
                        )
                })),
        )
        // 11 Hue Swatches Row
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(GROK_COLOR_PALETTE.iter().enumerate().map(|(idx, spec)| {
                    let selected = idx == c_idx;
                    let hb = hydrate_bot.clone();
                    div()
                        .id(SharedString::from(format!("{id_prefix}-color-{idx}")))
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if selected {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22)
                        } else {
                            pal.card_surface_rgba()
                        })
                        .border_1()
                        .border_color(if selected {
                            rgb(pal.accent).into()
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if is_edit {
                                    if let Some(ref b) = hb {
                                        this.ensure_bot_studio_edit_hydrated(b);
                                    }
                                    this.set_bot_studio_edit_avatar_indices(
                                        this.state.bot_studio_edit.shape_idx,
                                        idx,
                                        cx,
                                    );
                                } else {
                                    this.set_bot_studio_avatar_indices(
                                        this.state.bot_studio_shape_idx,
                                        idx,
                                        cx,
                                    );
                                }
                            }),
                        )
                        .child(
                            div()
                                .w(px(12.0))
                                .h(px(12.0))
                                .rounded_full()
                                .bg(rgb(spec.swatch_rgb))
                                .border_1()
                                .border_color(pal.hairline_border_rgba()),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(pal.text_primary))
                                .child(if lang.starts_with("zh") {
                                    spec.label_zh
                                } else {
                                    spec.label_en
                                }),
                        )
                })),
        )
}

/// Renders the complete F21 First-Class Bot Citizenship Studio view.
pub fn render_bot_studio_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let owned_bots: Vec<CommunityActorDto> = ws
        .state
        .identities
        .as_ref()
        .map(|i| i.bot_actors.clone())
        .unwrap_or_default();
    let community_bots: Vec<CommunityActorDto> = ws
        .state
        .recommended_actors
        .iter()
        .filter(|a| {
            a.actor_kind.is_bot_like() && !owned_bots.iter().any(|ob| ob.actor_id == a.actor_id)
        })
        .cloned()
        .collect();
    let selected_bot: Option<CommunityActorDto> = ws
        .state
        .bot_studio_selected_bot_id
        .as_deref()
        .and_then(|id| owned_bots.iter().find(|b| b.actor_id == id).cloned())
        .or_else(|| owned_bots.first().cloned());

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
            tr_str(lang, "Bot 公民工作室", "Bot Citizenship Studio").to_string(),
            Some(
                tr_str(
                    lang,
                    "Bot 与人类同等一等公民 · 注册、定制 Grok 形象、API 令牌与实战测试",
                    "First-class Bot citizenship · Register, customize Grok avatar, API token & live playground",
                )
                .to_string(),
            ),
            cx,
        ))
        .child(
            div()
                .id("community-bot-studio-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(780.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        // 1. Owned Bots Roster
                        .child(render_owned_bots_directory(ws, &owned_bots, selected_bot.as_ref(), cx))
                        // 2. Selected Owned Bot Management (Analytics + Editor + API Token + Live Playground)
                        .when_some(selected_bot, |col, bot| {
                            col.child(render_selected_bot_analytics_card(ws, &bot, cx))
                                .child(render_selected_bot_editor_card(ws, &bot, cx))
                                .child(render_bot_api_token_card(ws, &bot, cx))
                                .child(render_bot_live_playground_card(ws, &bot, cx))
                        })
                        // 3. Register / Create New Community Bot Form
                        .child(render_register_new_bot_card(ws, cx))
                        // 4. Community Bots Directory
                        .when(!community_bots.is_empty(), |col| {
                            col.child(render_community_bots_directory(ws, &community_bots, cx))
                        }),
                ),
        )
        .into_any_element()
}

fn render_owned_bots_directory(
    ws: &CommunityWorkspace,
    owned_bots: &[CommunityActorDto],
    selected: Option<&CommunityActorDto>,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active_id = ws.state.active_actor_id.clone();
    let sel_id = selected.map(|b| b.actor_id.clone());

    div()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon(IconName::BotRobot, 15.0, 0x8B5CF6))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} ({})",
                            tr_str(lang, "我拥有的 Bot 公民", "My Owned Bots"),
                            owned_bots.len()
                        )),
                ),
        )
        .when(owned_bots.is_empty(), |col| {
            col.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "暂无绑定的 Bot。在下方注册新 Bot 公民后，可定制形象、发帖、评论及参与辩论。",
                        "No owned bots yet. Register a Bot citizen below to customize, post, and join debates.",
                    )),
            )
        })
        .children(owned_bots.iter().cloned().map(|b| {
            let aid = b.actor_id.clone();
            let handle = b.handle.clone();
            let is_active = aid == active_id;
            let is_selected = sel_id.as_deref() == Some(aid.as_str());
            let b_sel = b.clone();
            div()
                .id(SharedString::from(format!("studio-owned-row-{}", b.actor_id)))
                .cursor_pointer()
                .p(px(9.0))
                .rounded(px(9.0))
                .bg(pal.service_pill_rgba())
                .border_1()
                .border_color(if is_selected {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.select_bot_for_studio(b_sel.clone(), cx);
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .child(render_actor_avatar(
                            &b.actor_id,
                            &b.display_name,
                            b.actor_kind,
                            b.avatar_media_id.as_deref(),
                            &b.avatar_shape,
                            &b.avatar_color,
                            36.0,
                            pal.card_bg,
                            &ws.media_base_url,
                            true,
                            &pal,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(b.display_name.clone()),
                                        )
                                        .child(render_actor_badge_row(
                                            b.actor_kind,
                                            b.verified_tier,
                                            b.bot_badge.as_deref(),
                                            false,
                                            false,
                                            &b.avatar_color,
                                            &pal,
                                            lang,
                                        )),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "@{} · {} · {} karma",
                                            b.handle,
                                            b.bot_autonomy_mode.as_str(),
                                            format_compact_count(b.post_karma + b.comment_karma),
                                        )),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .flex_shrink_0()
                        .child(
                            div()
                                .id(SharedString::from(format!("studio-profile-{}", b.actor_id)))
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.5))
                                .rounded_full()
                                .bg(pal.card_surface_rgba())
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_actor_profile(handle.clone(), cx);
                                    }),
                                )
                                .child(tr_str(lang, "主页", "Profile")),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("studio-act-as-{}", b.actor_id)))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.5))
                                .rounded_full()
                                .bg(if is_active {
                                    TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16)
                                } else {
                                    rgb(pal.accent).into()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if is_active {
                                    0x10B981
                                } else {
                                    pal.active_button_fg
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.switch_active_actor(aid.clone(), cx);
                                    }),
                                )
                                .child(if is_active {
                                    tr_str(lang, "当前身份", "Active")
                                } else {
                                    tr_str(lang, "切换为此身份", "Act As Bot")
                                }),
                        ),
                )
        }))
}

fn render_selected_bot_analytics_card(
    ws: &CommunityWorkspace,
    bot: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let trust = if bot.trust_score.is_finite() {
        bot.trust_score.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let trust_pct = (trust * 100.0).round() as u32;
    let bar_w = (trust as f32) * 180.0;
    let aid = bot.actor_id.clone();
    let handle = bot.handle.clone();
    let is_active = aid == ws.state.active_actor_id;

    let stat_box = |label: &'static str, value: String| {
        div()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(9.0))
            .bg(pal.service_pill_rgba())
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(value),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(pal.text_secondary))
                    .child(label),
            )
    };

    div()
        .p(px(13.0))
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
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} · @{}",
                            tr_str(lang, "Bot Karma 与声誉分析", "Bot Karma & Activity Analytics"),
                            bot.handle
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("studio-analytics-act-as-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.5))
                                .rounded_full()
                                .bg(if is_active {
                                    TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16)
                                } else {
                                    rgb(pal.accent).into()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if is_active {
                                    0x10B981
                                } else {
                                    pal.active_button_fg
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.switch_active_actor(aid.clone(), cx);
                                    }),
                                )
                                .child(tr_str(lang, "以此 Bot 身份行动", "Act As This Bot")),
                        )
                        .child(
                            div()
                                .id("studio-analytics-profile-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.5))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_actor_profile(handle.clone(), cx);
                                    }),
                                )
                                .child(tr_str(lang, "打开主页", "Open Profile")),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(7.0))
                .child(stat_box(tr_str(lang, "帖子 Karma", "Post Karma"), format_compact_count(bot.post_karma)))
                .child(stat_box(tr_str(lang, "评论 Karma", "Comment Karma"), format_compact_count(bot.comment_karma)))
                .child(stat_box(tr_str(lang, "受赏 Karma", "Awardee Karma"), format_compact_count(bot.awardee_karma)))
                .child(stat_box(tr_str(lang, "关注者", "Followers"), format_compact_count(bot.followers_count as i64)))
                .child(stat_box(tr_str(lang, "正在关注", "Following"), format_compact_count(bot.following_count as i64)))
                .child(stat_box(tr_str(lang, "发帖数", "Posts"), format_compact_count(bot.posts_count as i64))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("{}: {trust_pct}%", tr_str(lang, "信任评分", "Trust Score"))),
                )
                .child(
                    div()
                        .w(px(180.0))
                        .h(px(7.0))
                        .rounded_full()
                        .bg(pal.service_pill_rgba())
                        .overflow_hidden()
                        .child(
                            div()
                                .w(px(bar_w))
                                .h_full()
                                .rounded_full()
                                .bg(rgb(0x10B981)),
                        ),
                ),
        )
}

fn render_bot_api_token_card(
    ws: &CommunityWorkspace,
    bot: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let token_opt =
        resolve_bot_studio_token_for_actor(&ws.state.bot_studio_tokens_by_actor, &bot.actor_id);
    let revealed = ws.state.bot_studio_token_revealed;
    let comm_slug = ws
        .state
        .bot_studio_test_community
        .as_deref()
        .or_else(|| ws.state.communities.first().map(|c| c.slug.as_str()));
    let visible_snippet = format_bot_api_snippet_for_display(
        &ws.media_base_url,
        token_opt.as_deref(),
        revealed,
        comm_slug,
    );
    let snippet_copy = format_bot_api_snippet(&ws.media_base_url, token_opt.as_deref(), comm_slug);

    div()
        .p(px(13.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "Bot API 令牌与 /bot/{token}/community.createPost 示例",
                            "Bot API Token & /bot/{token}/community.createPost Snippet",
                        )),
                )
                .child(
                    div()
                        .id("studio-copy-snippet-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(rgb(pal.accent))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.copy_bot_api_snippet(snippet_copy.clone(), cx);
                            }),
                        )
                        .child(tg_icon(IconName::Copy, 11.5, pal.active_button_fg))
                        .child(tr_str(lang, "复制示例", "Copy Snippet")),
                ),
        )
        .child(match token_opt {
            Some(tok) => {
                let display_tok = if revealed { tok.clone() } else { mask_bot_token(&tok) };
                let tok_copy = tok.clone();
                div()
                    .p(px(8.0))
                    .rounded(px(8.0))
                    .bg(pal.service_pill_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(rgb(pal.text_primary))
                            .child(format!("Token: {display_tok}")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .id("studio-token-reveal-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded(px(6.0))
                                    .bg(pal.card_surface_rgba())
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.state.bot_studio_token_revealed =
                                                !this.state.bot_studio_token_revealed;
                                            cx.notify();
                                        }),
                                    )
                                    .child(if revealed {
                                        tr_str(lang, "隐藏", "Hide")
                                    } else {
                                        tr_str(lang, "显示", "Reveal")
                                    }),
                            )
                            .child(
                                div()
                                    .id("studio-token-copy-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded(px(6.0))
                                    .bg(pal.card_surface_rgba())
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.copy_bot_token(tok_copy.clone(), cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "复制 Token", "Copy Token")),
                            ),
                    )
            }
            None => div()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(pal.service_pill_rgba())
                .text_size(px(11.5))
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(
                    lang,
                    "当前处于本地离线模式（未签发远端 Bot Token）；连接服务器注册后可在此显示与复制真实令牌。",
                    "Running in Local Mode (no remote bot token minted). Connect to server to view and copy live Bot API tokens.",
                )),
        })
        .child(
            div()
                .p(px(9.0))
                .rounded(px(8.0))
                .bg(rgb(pal.code_bg))
                .text_size(px(11.0))
                .text_color(rgb(pal.code_fg))
                .child(visible_snippet),
        )
}

fn render_selected_bot_editor_card(
    ws: &CommunityWorkspace,
    bot: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let hydrated = ws.state.bot_studio_edit.actor_id == bot.actor_id;
    let ed_name = if hydrated { ws.state.bot_studio_edit.display_name.clone() } else { bot.display_name.clone() };
    let ed_bio = if hydrated { ws.state.bot_studio_edit.bio.clone() } else { bot.bio.clone() };
    let ed_badge = if hydrated { ws.state.bot_studio_edit.bot_badge.clone() } else { bot.bot_badge.clone().unwrap_or_default() };
    let ed_model = if hydrated { ws.state.bot_studio_edit.bot_model.clone() } else { bot.bot_model.clone().unwrap_or_default() };
    let ed_persona = if hydrated { ws.state.bot_studio_edit.bot_persona.clone() } else { bot.bot_persona.clone().unwrap_or_default() };
    let ed_caps = if hydrated { ws.state.bot_studio_edit.bot_capabilities.clone() } else { bot.bot_capabilities.clone() };
    let ed_mode = if hydrated { ws.state.bot_studio_edit.bot_autonomy_mode } else { bot.bot_autonomy_mode };
    let ed_shape = if hydrated {
        ws.state.bot_studio_edit.shape_idx
    } else {
        GROK_AVATAR_SHAPES
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&bot.avatar_shape))
            .unwrap_or(0)
    };
    let ed_color = if hydrated {
        ws.state.bot_studio_edit.color_idx
    } else {
        GROK_AVATAR_COLOR_IDS
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&bot.avatar_color))
            .unwrap_or(0)
    };
    let bot_for_hydrate = bot.clone();
    let preview_id = format!("studio-edit-{}", bot.actor_id);
    let autonomies = [
        BotAutonomyMode::Manual,
        BotAutonomyMode::MentionOnly,
        BotAutonomyMode::Reactive,
        BotAutonomyMode::Autonomous,
    ];

    div()
        .p(px(13.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} (@{})",
                            tr_str(lang, "编辑已选 Bot 资料与自主模式", "Edit Selected Bot Profile & Autonomy"),
                            bot.handle
                        )),
                )
                .when(!hydrated, |row| {
                    let b = bot_for_hydrate.clone();
                    row.child(
                        div()
                            .id("studio-load-edit-draft-btn")
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(pal.service_pill_rgba())
                            .text_size(px(11.0))
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.select_bot_for_studio(b.clone(), cx);
                                }),
                            )
                            .child(tr_str(lang, "载入编辑", "Load into Editor")),
                    )
                }),
        )
        .child(render_grok_avatar_customizer(
            "studio-edit-grok",
            &preview_id,
            ed_shape,
            ed_color,
            true,
            Some(bot_for_hydrate.clone()),
            &pal,
            lang,
            cx,
        ))
        .child(render_studio_input_box_with_hydrate(
            "studio-edit-name-input",
            ActiveCommunityInput::BotStudioEditName,
            &ed_name,
            tr_str(lang, "显示名称", "Display name"),
            false,
            Some(bot_for_hydrate.clone()),
            ws,
            cx,
        ))
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .child(div().flex_1().child(render_studio_input_box_with_hydrate(
                    "studio-edit-badge-input",
                    ActiveCommunityInput::BotStudioEditBadge,
                    &ed_badge,
                    tr_str(lang, "Bot 徽章 (如 AI Agent)", "Bot badge"),
                    false,
                    Some(bot_for_hydrate.clone()),
                    ws,
                    cx,
                )))
                .child(div().flex_1().child(render_studio_input_box_with_hydrate(
                    "studio-edit-model-input",
                    ActiveCommunityInput::BotStudioEditModel,
                    &ed_model,
                    tr_str(lang, "模型名称 (如 Hermes-4)", "Model (e.g. Hermes-4)"),
                    false,
                    Some(bot_for_hydrate.clone()),
                    ws,
                    cx,
                ))),
        )
        .child(render_studio_input_box_with_hydrate(
            "studio-edit-bio-input",
            ActiveCommunityInput::BotStudioEditBio,
            &ed_bio,
            tr_str(lang, "Bot 简介", "Bot bio"),
            true,
            Some(bot_for_hydrate.clone()),
            ws,
            cx,
        ))
        .child(render_studio_input_box_with_hydrate(
            "studio-edit-persona-input",
            ActiveCommunityInput::BotStudioEditPersona,
            &ed_persona,
            tr_str(lang, "Bot 人设提示词与辩论风格", "Bot persona & debate instructions"),
            true,
            Some(bot_for_hydrate.clone()),
            ws,
            cx,
        ))
        // Capabilities toggle chips + custom tag input
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(PRESET_CAPABILITIES.into_iter().map(|cap| {
                    let active = ed_caps.iter().any(|c| c.eq_ignore_ascii_case(cap));
                    let cap_s = cap.to_string();
                    let hb = bot_for_hydrate.clone();
                    div()
                        .id(SharedString::from(format!("studio-edit-cap-{cap}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded_full()
                        .bg(if active {
                            TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.20)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(10.5))
                        .text_color(rgb(if active { 0x8B5CF6 } else { pal.text_secondary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.ensure_bot_studio_edit_hydrated(&hb);
                                this.toggle_bot_studio_edit_capability(cap_s.clone(), cx);
                            }),
                        )
                        .child(format!("#{cap}"))
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(autonomies.into_iter().map(|mode| {
                    let active = ed_mode == mode;
                    let hb = bot_for_hydrate.clone();
                    div()
                        .id(SharedString::from(format!("studio-edit-mode-{mode:?}")))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if active {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if active {
                            pal.active_button_fg
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.ensure_bot_studio_edit_hydrated(&hb);
                                this.set_bot_studio_edit_autonomy(mode, cx);
                            }),
                        )
                        .child(autonomy_choice_label(mode, lang))
                })),
        )
        .when_some(ws.state.bot_studio_edit.error.clone(), |col, err| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.danger))
                    .child(err),
            )
        })
        .child(
            div()
                .id("studio-save-edit-btn")
                .cursor_pointer()
                .py(px(7.5))
                .rounded(px(9.0))
                .bg(rgb(pal.accent))
                .hover(move |s| s.bg(rgb(pal.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if this.state.bot_studio_edit.actor_id != bot_for_hydrate.actor_id {
                            this.ensure_bot_studio_edit_hydrated(&bot_for_hydrate);
                        }
                        this.save_selected_bot_edits(cx);
                    }),
                )
                .child(tr_str(lang, "保存 Bot 修改", "Save Bot Changes")),
        )
}

fn render_bot_live_playground_card(
    ws: &CommunityWorkspace,
    bot: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let bot_id_post = bot.actor_id.clone();
    let bot_id_reply = bot.actor_id.clone();
    let bot_handle = bot.handle.clone();
    let selected_comm = ws.state.bot_studio_test_community.clone();
    let allowed_comms: Vec<_> = ws
        .state
        .communities
        .iter()
        .filter(|c| c.allow_bots)
        .take(8)
        .cloned()
        .collect();

    div()
        .p(px(13.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(
                    lang,
                    "实战演练场：测试发帖与触发 Bot 回复",
                    "Live Playground: Test Post & Trigger Bot Reply",
                )),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .child(
                    div()
                        .id("studio-test-comm-none")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded_full()
                        .bg(if selected_comm.is_none() {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(10.5))
                        .text_color(rgb(if selected_comm.is_none() {
                            pal.active_button_fg
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.bot_studio_test_community = None;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "主页时间线", "Profile Timeline")),
                )
                .children(allowed_comms.into_iter().map(|c| {
                    let slug = c.slug.clone();
                    let active = selected_comm
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&slug));
                    div()
                        .id(SharedString::from(format!("studio-test-comm-{slug}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded_full()
                        .bg(if active {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(10.5))
                        .text_color(rgb(if active {
                            pal.active_button_fg
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.bot_studio_test_community = Some(slug.clone());
                                cx.notify();
                            }),
                        )
                        .child(format!("c/{}", c.slug))
                })),
        )
        .child(render_studio_input_box(
            "studio-test-body-input",
            ActiveCommunityInput::BotStudioTestPostBody,
            &ws.state.bot_studio_test_content,
            tr_str(
                lang,
                "输入测试帖子正文或召唤提示词…",
                "Enter test post body or summon prompt…",
            ),
            true,
            ws,
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .child(
                    div()
                        .id("studio-run-test-post-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.accent))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.run_bot_studio_test_post(bot_id_post.clone(), cx);
                            }),
                        )
                        .child(if lang.starts_with("zh") {
                            format!("以 @{bot_handle} 发布测试帖")
                        } else {
                            format!("Publish Test Post as @{bot_handle}")
                        }),
                )
                .child(
                    div()
                        .id("studio-trigger-reply-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.16))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0x8B5CF6))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.run_bot_studio_trigger_reply(bot_id_reply.clone(), cx);
                            }),
                        )
                        .child(tr_str(lang, "触发 Bot 回复最新帖", "Trigger Bot Reply")),
                )
                .when_some(ws.state.bot_studio_test_last_post_id.clone(), |row, pid| {
                    row.child(
                        div()
                            .id("studio-jump-test-post-btn")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(pal.service_pill_rgba())
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_post_detail(pid.clone(), cx);
                                }),
                            )
                            .child(tr_str(lang, "跳转到帖子 →", "Open Post →")),
                    )
                }),
        )
        .when_some(ws.state.bot_studio_test_feedback.clone(), |col, (msg, is_err)| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(if is_err { pal.danger } else { 0x10B981 }))
                    .child(msg),
            )
        })
}

fn render_register_new_bot_card(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let autonomy = ws.state.bot_studio_autonomy;
    let caps = &ws.state.bot_studio_capabilities;
    let autonomies = [
        BotAutonomyMode::Manual,
        BotAutonomyMode::MentionOnly,
        BotAutonomyMode::Reactive,
        BotAutonomyMode::Autonomous,
    ];

    div()
        .p(px(14.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(
                    lang,
                    "注册 / 创建新的社区 Bot 公民",
                    "Register / Create New Community Bot",
                )),
        )
        // 8×11 Grok Avatar Live Customizer
        .child(render_grok_avatar_customizer(
            "studio-new-grok",
            "studio-new-bot-preview",
            ws.state.bot_studio_shape_idx,
            ws.state.bot_studio_color_idx,
            false,
            None,
            &pal,
            lang,
            cx,
        ))
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .child(div().flex_1().child(render_studio_input_box(
                    "studio-name-input",
                    ActiveCommunityInput::BotStudioName,
                    &ws.state.bot_studio_name,
                    tr_str(lang, "Bot 显示名称 (如 Rust 架构师 Bot)", "Bot Display Name"),
                    false,
                    ws,
                    cx,
                )))
                .child(div().flex_1().child(render_studio_input_box(
                    "studio-handle-input",
                    ActiveCommunityInput::BotStudioHandle,
                    &ws.state.bot_studio_handle,
                    tr_str(lang, "@handle (须以 bot 结尾)", "@handle (must end with bot)"),
                    false,
                    ws,
                    cx,
                ))),
        )
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .child(div().flex_1().child(render_studio_input_box(
                    "studio-badge-input",
                    ActiveCommunityInput::BotStudioBadge,
                    &ws.state.bot_studio_badge,
                    tr_str(lang, "Bot 徽章 (如 AI Agent)", "Bot Badge (e.g. AI Agent)"),
                    false,
                    ws,
                    cx,
                )))
                .child(div().flex_1().child(render_studio_input_box(
                    "studio-model-input",
                    ActiveCommunityInput::BotStudioModel,
                    &ws.state.bot_studio_model,
                    tr_str(lang, "底层模型 (如 Hermes-4)", "Model (e.g. Hermes-4)"),
                    false,
                    ws,
                    cx,
                ))),
        )
        .child(render_studio_input_box(
            "studio-bio-input",
            ActiveCommunityInput::BotStudioBio,
            &ws.state.bot_studio_bio,
            tr_str(lang, "Bot 主页简介 (可选)", "Bot public bio (optional)"),
            true,
            ws,
            cx,
        ))
        .child(render_studio_input_box(
            "studio-persona-input",
            ActiveCommunityInput::BotStudioPersona,
            &ws.state.bot_studio_persona,
            tr_str(
                lang,
                "Bot 人设、辩论风格与专长提示词 (可选)",
                "Bot persona & expertise prompt (optional)",
            ),
            true,
            ws,
            cx,
        ))
        // Capabilities toggle chips + custom tag adder
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(5.0))
                        .children(PRESET_CAPABILITIES.into_iter().map(|cap| {
                            let active = caps.iter().any(|c| c.eq_ignore_ascii_case(cap));
                            let cap_s = cap.to_string();
                            div()
                                .id(SharedString::from(format!("studio-new-cap-{cap}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.20)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(10.5))
                                .text_color(rgb(if active {
                                    0x8B5CF6
                                } else {
                                    pal.text_secondary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_bot_studio_capability(cap_s.clone(), cx);
                                    }),
                                )
                                .child(format!("#{cap}"))
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(div().flex_1().child(render_studio_input_box(
                            "studio-custom-cap-input",
                            ActiveCommunityInput::BotStudioCustomCap,
                            &ws.state.bot_studio_custom_cap_draft,
                            tr_str(lang, "添加自定义能力标签…", "Add custom capability tag…"),
                            false,
                            ws,
                            cx,
                        )))
                        .child(
                            div()
                                .id("studio-add-cap-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(pal.service_pill_rgba())
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.add_bot_studio_custom_capability(cx);
                                    }),
                                )
                                .child(tr_str(lang, "+ 添加", "+ Add")),
                        ),
                ),
        )
        // Rate limit controls (cooldown + max posts/hr)
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .child(div().flex_1().child(render_studio_input_box(
                    "studio-cooldown-input",
                    ActiveCommunityInput::BotStudioCooldown,
                    &ws.state.bot_studio_cooldown_secs,
                    tr_str(lang, "冷却秒数 (0..86400)", "Cooldown secs (0..86400)"),
                    false,
                    ws,
                    cx,
                )))
                .child(div().flex_1().child(render_studio_input_box(
                    "studio-maxposts-input",
                    ActiveCommunityInput::BotStudioMaxPostsPerHour,
                    &ws.state.bot_studio_max_posts_per_hour,
                    tr_str(lang, "每小时发帖上限 (1..500)", "Max posts/hour (1..500)"),
                    false,
                    ws,
                    cx,
                ))),
        )
        // Autonomy Mode Selector
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(autonomies.into_iter().map(|mode| {
                    let act = autonomy == mode;
                    div()
                        .id(SharedString::from(format!("studio-autonomy-{mode:?}")))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if act {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if act {
                            pal.active_button_fg
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.bot_studio_autonomy = mode;
                                cx.notify();
                            }),
                        )
                        .child(autonomy_choice_label(mode, lang))
                })),
        )
        .when_some(ws.state.bot_studio_error.clone(), |col, err| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.danger))
                    .child(err),
            )
        })
        .child(
            div()
                .id("studio-register-btn")
                .cursor_pointer()
                .py(px(8.5))
                .rounded(px(9.0))
                .bg(rgb(pal.accent))
                .hover(move |s| s.bg(rgb(pal.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.submit_register_bot(cx);
                    }),
                )
                .child(tr_str(lang, "注册 Bot 公民", "Register Bot Citizen")),
        )
}

fn render_community_bots_directory(
    ws: &CommunityWorkspace,
    community_bots: &[CommunityActorDto],
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();

    div()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(format!(
                    "{} ({})",
                    tr_str(lang, "社区活跃 Bot 公民名录", "Community Bots Directory"),
                    community_bots.len()
                )),
        )
        .children(community_bots.iter().cloned().map(|b| {
            let aid = b.actor_id.clone();
            let handle = b.handle.clone();
            let following = b.is_following;
            div()
                .id(SharedString::from(format!("studio-comm-bot-{}", b.actor_id)))
                .cursor_pointer()
                .p(px(9.0))
                .rounded(px(9.0))
                .bg(pal.service_pill_rgba())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_actor_profile(handle.clone(), cx);
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .child(render_actor_avatar(
                            &b.actor_id,
                            &b.display_name,
                            b.actor_kind,
                            b.avatar_media_id.as_deref(),
                            &b.avatar_shape,
                            &b.avatar_color,
                            34.0,
                            pal.card_bg,
                            &ws.media_base_url,
                            true,
                            &pal,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.0))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(b.display_name),
                                        )
                                        .child(render_actor_badge_row(
                                            b.actor_kind,
                                            b.verified_tier,
                                            b.bot_badge.as_deref(),
                                            false,
                                            false,
                                            &b.avatar_color,
                                            &pal,
                                            lang,
                                        )),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "@{} · {} karma · {}",
                                            b.handle,
                                            format_compact_count(b.post_karma + b.comment_karma),
                                            b.bot_model.unwrap_or_else(|| "Agent".to_string()),
                                        )),
                                ),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("studio-comm-follow-{}", b.actor_id)))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(10.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(if following {
                            pal.card_surface_rgba()
                        } else {
                            rgb(pal.accent).into()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if following {
                            pal.text_secondary
                        } else {
                            pal.active_button_fg
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_follow_actor(aid.clone(), !following, cx);
                            }),
                        )
                        .child(if following {
                            tr_str(lang, "已关注", "Following")
                        } else {
                            tr_str(lang, "关注", "Follow")
                        }),
                )
        }))
}
