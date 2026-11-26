//! Feature F21-Moderation — Community Moderation Console (Strict Role-Gated Mod Queue,
//! Declarative AutoMod Rule Editor + Live Pure-Rust `AutoModEngine` Dry-Run Tester,
//! and Transparent Public Mod Audit Log).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::{AutoModEngine, ModPermissionValidator};
use gpui_telegram_proto::community_contracts::{
    ActorKind, AutoModActionKind, AutoModRuleDto, CommunityActorDto, ModActionKind,
    ModActionRequest, ModLogEntryDto, ModQueueItemDto, ModQueueStatus, SubCommunityDto,
    UpdateCommunityRequest, VerifiedTier,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::shared_widgets::{
    format_relative_time, render_actor_avatar, render_actor_badge_row, render_empty_state,
    render_input_display, render_loading_skeleton,
};
use super::state::{
    ActiveCommunityInput, AutoModRuleDraft, CommunityPopover, CommunityRoute,
    CommunityWorkspaceAction, ModConsoleTab,
};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

/// Validates an AutoMod regex pattern directly via [`AutoModEngine::validate_regex`],
/// sharing the exact `regex::RegexBuilder` 64 KiB NFA/DFA limits used during evaluation.
pub fn validate_automod_regex_syntax(pattern: &str) -> Result<(), String> {
    AutoModEngine::validate_regex(pattern)
}

fn automod_action_label(action: AutoModActionKind, lang: &str) -> &'static str {
    match action {
        AutoModActionKind::FlagForReview => tr_str(lang, "送审队列 (Queue)", "Queue / Flag"),
        AutoModActionKind::Remove => tr_str(lang, "自动移除 (Remove)", "Remove"),
        AutoModActionKind::AssignFlair => tr_str(lang, "自动标签 (Flair)", "Assign Flair"),
        AutoModActionKind::AutoReply => tr_str(lang, "自动回复 (Reply)", "Auto Reply"),
        AutoModActionKind::Approve => tr_str(lang, "自动通过 (Approve)", "Approve"),
    }
}

fn mod_action_badge_color(action: ModActionKind, pal: &TdesktopThemePalette) -> u32 {
    match action {
        ModActionKind::Approve | ModActionKind::UnbanActor | ModActionKind::UnmuteActor => 0x10B981,
        ModActionKind::Remove | ModActionKind::BanActor => pal.danger,
        ModActionKind::Lock | ModActionKind::MuteActor => 0xF59E0B,
        ModActionKind::Pin | ModActionKind::Distinguish | ModActionKind::PromoteMod => pal.accent,
        _ => pal.text_secondary,
    }
}

impl CommunityWorkspace {
    /// Returns all communities where the active Human or Bot identity holds moderator rights.
    pub fn moderated_communities(&self) -> Vec<SubCommunityDto> {
        let mut out: Vec<SubCommunityDto> = self
            .state
            .communities
            .iter()
            .filter(|c| ModPermissionValidator::can_moderate(c.viewer_role))
            .cloned()
            .collect();
        if let Some(ref mc) = self.state.mod_community {
            if ModPermissionValidator::can_moderate(mc.viewer_role)
                && !out.iter().any(|c| c.slug.eq_ignore_ascii_case(&mc.slug))
            {
                out.insert(0, mc.clone());
            }
        }
        out
    }

    /// Returns `true` if the active identity holds moderator permissions for the specific `slug`.
    pub fn can_moderate_community_slug(&self, slug: &str) -> bool {
        let target = slug.trim();
        if target.is_empty() {
            return false;
        }
        if let Some(ref mc) = self.state.mod_community {
            if mc.slug.eq_ignore_ascii_case(target) {
                return ModPermissionValidator::can_moderate(mc.viewer_role);
            }
        }
        if let Some(ref det) = self.state.community_detail {
            if det.community.slug.eq_ignore_ascii_case(target) {
                return ModPermissionValidator::can_moderate(det.community.viewer_role);
            }
        }
        if let Some(c) = self
            .state
            .communities
            .iter()
            .find(|c| c.slug.eq_ignore_ascii_case(target))
        {
            return ModPermissionValidator::can_moderate(c.viewer_role);
        }
        false
    }

    /// Returns `true` if the active identity can moderate the currently selected `mod_slug`.
    pub fn can_moderate_active_mod_community(&self) -> bool {
        if let Some(ref slug) = self.state.mod_slug {
            return self.can_moderate_community_slug(slug);
        }
        if let Some(ref mc) = self.state.mod_community {
            return ModPermissionValidator::can_moderate(mc.viewer_role);
        }
        false
    }

    pub fn set_mod_tab(&mut self, tab: ModConsoleTab, cx: &mut Context<Self>) {
        if self.state.mod_tab != tab {
            self.state.mod_tab = tab;
            cx.notify();
        }
    }

    pub fn set_mod_status_filter(
        &mut self,
        status: Option<ModQueueStatus>,
        cx: &mut Context<Self>,
    ) {
        if self.state.mod_status_filter != status {
            self.state.mod_status_filter = status;
            if let Some(slug) = self.state.mod_slug.clone() {
                self.load_mod_queue(slug, cx);
            } else {
                cx.notify();
            }
        }
    }

    /// Executes a typed moderation action with strict `target_kind` enforcement and
    /// refreshes both Mod Queue and Mod Logs on completion.
    pub fn execute_mod_console_action(
        &mut self,
        slug: String,
        action: ModActionKind,
        target_kind: String,
        target_id: String,
        queue_item_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.language_id.clone();
        if !self.can_moderate_community_slug(&slug) {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(
                        &lang,
                        "你当前身份没有该社区的版主权限",
                        "Active identity does not have moderator permissions for this community",
                    )
                    .to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let reason = {
            let r = self.state.mod_reason_draft.trim().to_string();
            if r.is_empty() {
                Some(format!("Moderation action: {}", action.as_str()))
            } else {
                Some(r)
            }
        };
        let flair = if matches!(action, ModActionKind::SetPostFlair | ModActionKind::SetUserFlair) {
            let f = self.state.mod_flair_draft.trim().to_string();
            if f.is_empty() {
                self.emit_action(
                    CommunityWorkspaceAction::ShowToast {
                        text: tr_str(&lang, "请先填写目标 Flair 文本", "Enter flair text first")
                            .to_string(),
                        is_error: true,
                    },
                    cx,
                );
                return;
            }
            Some(f)
        } else {
            None
        };
        let req = ModActionRequest {
            action,
            target_kind,
            target_id,
            reason,
            flair,
            queue_item_id,
        };
        let my_actor_gen = self.state.actor_switch_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_slug = slug.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.execute_mod_action(&slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(_)) => {
                        this.state.mod_reason_draft.clear();
                        this.state.mod_flair_draft.clear();
                        if this
                            .state
                            .mod_slug
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&reload_slug))
                        {
                            this.load_mod_queue(reload_slug, cx);
                        }
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "审核操作已执行", "Moderation action applied")
                                    .to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl CommunityWorkspace {
    /// Validates and appends a new declarative [`AutoModRuleDto`] to the active moderated community.
    pub fn submit_add_automod_rule(&mut self, cx: &mut Context<Self>) {
        if self.state.automod_draft.saving {
            return;
        }
        let lang = self.language_id.as_str();
        if !self.can_moderate_active_mod_community() {
            self.state.automod_draft.error = Some(
                tr_str(lang, "无版主权限，无法编辑 AutoMod 规则", "Moderator permissions required")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        let Some(comm) = self.state.mod_community.clone() else {
            self.state.automod_draft.error = Some(
                tr_str(lang, "请先选择要管理的社区", "Select a community first").to_string(),
            );
            cx.notify();
            return;
        };
        let draft = &self.state.automod_draft;
        let name = draft.name.trim().to_string();
        if name.is_empty() {
            self.state.automod_draft.error = Some(
                tr_str(lang, "规则名称不能为空", "Rule name cannot be empty").to_string(),
            );
            cx.notify();
            return;
        }
        let regex_pattern = {
            let pat = draft.regex_pattern.trim().to_string();
            if pat.is_empty() {
                None
            } else {
                if let Err(err) = validate_automod_regex_syntax(&pat) {
                    self.state.automod_draft.error = Some(err);
                    cx.notify();
                    return;
                }
                Some(pat)
            }
        };
        let keywords: Vec<String> = draft
            .keywords
            .split([',', '，', ';'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        let min_author_karma = {
            let raw = draft.min_karma.trim();
            if raw.is_empty() {
                None
            } else {
                match raw.parse::<i64>() {
                    Ok(v) => Some(v),
                    Err(_) => {
                        self.state.automod_draft.error = Some(
                            tr_str(
                                lang,
                                "最低 Karma 必须是有效整数",
                                "Min karma must be a valid integer",
                            )
                            .to_string(),
                        );
                        cx.notify();
                        return;
                    }
                }
            }
        };
        let max_links = {
            let raw = draft.max_links.trim();
            if raw.is_empty() {
                None
            } else {
                match raw.parse::<usize>() {
                    Ok(v) => Some(v),
                    Err(_) => {
                        self.state.automod_draft.error = Some(
                            tr_str(
                                lang,
                                "最大链接数必须是非负整数",
                                "Max links must be a non-negative integer",
                            )
                            .to_string(),
                        );
                        cx.notify();
                        return;
                    }
                }
            }
        };
        if keywords.is_empty()
            && regex_pattern.is_none()
            && min_author_karma.is_none()
            && max_links.is_none()
        {
            self.state.automod_draft.error = Some(
                tr_str(
                    lang,
                    "请至少设置一个触发条件（关键词、正则、最低 Karma 或最大链接数）",
                    "Specify at least one trigger (keywords, regex, min karma, or max links)",
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let next_priority = (comm.automod_rules.len() as i32 + 1) * 10;
        let assign_flair = Some(draft.assign_flair.trim().to_string()).filter(|s| !s.is_empty());
        let auto_reply_template =
            Some(draft.auto_reply_template.trim().to_string()).filter(|s| !s.is_empty());
        let new_rule = AutoModRuleDto {
            rule_id: format!("am_{}_{}", comm.slug, now_ms),
            community_id: comm.community_id.clone(),
            name,
            enabled: true,
            priority: next_priority,
            keywords,
            regex_pattern,
            min_author_karma,
            min_account_age_secs: None,
            max_links,
            block_duplicate_spam: false,
            apply_to_bots: draft.apply_to_bots,
            apply_to_humans: draft.apply_to_humans,
            action: draft.action,
            assign_flair,
            auto_reply_template,
            created_at_ms: now_ms,
        };
        let slug = comm.slug.clone();
        let slug_key = slug.to_ascii_lowercase();
        self.state
            .automod_last_synced
            .entry(slug_key)
            .or_insert_with(|| comm.automod_rules.clone());
        if let Some(ref mut mc) = self.state.mod_community {
            if mc.slug.eq_ignore_ascii_case(&slug) {
                mc.automod_rules.push(new_rule);
            }
        }
        self.persist_automod_rules(slug, true, cx);
    }

    pub fn toggle_automod_rule_enabled(&mut self, rule_id: String, cx: &mut Context<Self>) {
        if !self.can_moderate_active_mod_community() {
            return;
        }
        let Some((slug, prev_rules)) = self
            .state
            .mod_community
            .as_ref()
            .map(|c| (c.slug.clone(), c.automod_rules.clone()))
        else {
            return;
        };
        let slug_key = slug.to_ascii_lowercase();
        self.state
            .automod_last_synced
            .entry(slug_key)
            .or_insert(prev_rules);
        if let Some(ref mut mc) = self.state.mod_community {
            for r in &mut mc.automod_rules {
                if r.rule_id == rule_id {
                    r.enabled = !r.enabled;
                }
            }
        }
        self.persist_automod_rules(slug, false, cx);
    }

    pub fn delete_automod_rule(&mut self, rule_id: String, cx: &mut Context<Self>) {
        if !self.can_moderate_active_mod_community() {
            return;
        }
        let Some((slug, prev_rules)) = self
            .state
            .mod_community
            .as_ref()
            .map(|c| (c.slug.clone(), c.automod_rules.clone()))
        else {
            return;
        };
        let slug_key = slug.to_ascii_lowercase();
        self.state
            .automod_last_synced
            .entry(slug_key)
            .or_insert(prev_rules);
        if let Some(ref mut mc) = self.state.mod_community {
            mc.automod_rules.retain(|r| r.rule_id != rule_id);
        }
        self.persist_automod_rules(slug, false, cx);
    }

    fn persist_automod_rules(&mut self, slug: String, reset_draft: bool, cx: &mut Context<Self>) {
        let slug_key = slug.to_ascii_lowercase();
        if reset_draft {
            self.state.automod_reset_draft_pending.insert(slug_key.clone());
        }
        if self.state.automod_in_flight.contains_key(&slug_key) {
            self.state.automod_dirty.insert(slug_key);
            self.state.automod_draft.saving = true;
            cx.notify();
            return;
        }
        self.flush_automod_rules_sync(slug, cx);
    }

    fn flush_automod_rules_sync(&mut self, slug: String, cx: &mut Context<Self>) {
        let slug_key = slug.to_ascii_lowercase();
        let Some(rules) = self
            .state
            .mod_community
            .as_ref()
            .filter(|c| c.slug.eq_ignore_ascii_case(&slug))
            .map(|c| c.automod_rules.clone())
        else {
            self.state.automod_dirty.remove(&slug_key);
            self.state.automod_reset_draft_pending.remove(&slug_key);
            self.state.automod_last_synced.remove(&slug_key);
            self.state.automod_draft.saving = !self.state.automod_in_flight.is_empty();
            cx.notify();
            return;
        };

        self.state.automod_dirty.remove(&slug_key);
        self.state.automod_flight_seq = self.state.automod_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.automod_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        self.state
            .automod_in_flight
            .insert(slug_key.clone(), my_flight_seq);
        self.state.automod_draft.saving = true;
        self.state.automod_draft.error = None;

        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let req_slug = slug.clone();
        let req = UpdateCommunityRequest {
            automod_rules: Some(rules),
            ..Default::default()
        };
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_community(&req_slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                // Fence 1: Actor switch occurred (ABA-proof via actor_switch_gen + actor_id).
                // Never clear another actor's saving state or write submitter's `viewer_role`
                // into `mod_community`, `communities`, or `community_detail`.
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                // Fence 2: Flight token ownership.
                if this.state.automod_in_flight.get(&slug_key).copied() != Some(my_flight_seq) {
                    return;
                }
                this.state.automod_in_flight.remove(&slug_key);

                let still_same_slug = this
                    .state
                    .mod_slug
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&slug));
                let is_dirty = this.state.automod_dirty.remove(&slug_key);
                let lang = this.language_id.clone();

                match res {
                    Ok(Ok(mut updated)) => {
                        this.state.automod_rules_gen =
                            this.state.automod_rules_gen.wrapping_add(1);
                        this.state.overlay_in_flight_community_mutations(&mut updated);
                        this.state
                            .automod_last_synced
                            .insert(slug_key.clone(), updated.automod_rules.clone());
                        let mut cache_dto = updated.clone();
                        if is_dirty && still_same_slug {
                            if let Some(ref mc) = this.state.mod_community {
                                if mc.slug.eq_ignore_ascii_case(&updated.slug) {
                                    cache_dto.automod_rules = mc.automod_rules.clone();
                                }
                            }
                        }
                        if let Some(pos) = this
                            .state
                            .communities
                            .iter()
                            .position(|c| c.slug.eq_ignore_ascii_case(&cache_dto.slug))
                        {
                            this.state.communities[pos] = cache_dto.clone();
                        }
                        if let Some(ref mut cdet) = this.state.community_detail {
                            if cdet.community.slug.eq_ignore_ascii_case(&cache_dto.slug) {
                                cdet.community = cache_dto;
                            }
                        }
                        if this.state.automod_reset_draft_pending.remove(&slug_key) {
                            this.state.automod_draft = AutoModRuleDraft::default();
                            if this.state.active_input.is_automod_input() {
                                this.state.blur_input();
                            }
                        }
                        if is_dirty && still_same_slug {
                            // Newer optimistic rule toggles arrived mid-flight: preserve optimistic
                            // `mod_community.automod_rules` and flush latest snapshot strictly next.
                            this.flush_automod_rules_sync(slug, cx);
                            return;
                        }
                        this.state.automod_draft.saving = !this.state.automod_in_flight.is_empty();
                        this.state.automod_last_synced.remove(&slug_key);
                        if still_same_slug {
                            this.state.mod_community = Some(updated);
                            if this.state.mod_loading {
                                this.load_mod_queue(slug.clone(), cx);
                            }
                        }
                        if this.state.communities_loading {
                            this.reload_communities_list(cx);
                        }
                        if this.state.community_loading
                            && matches!(
                                &this.state.route,
                                CommunityRoute::CommunityPage(s) if s.eq_ignore_ascii_case(&slug)
                            )
                        {
                            this.load_community_page(slug, cx);
                        }
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "AutoMod 规则已更新", "AutoMod rules saved")
                                    .to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        this.state.automod_dirty.remove(&slug_key);
                        this.state.automod_reset_draft_pending.remove(&slug_key);
                        this.state.automod_draft.saving = !this.state.automod_in_flight.is_empty();
                        if let Some(prev_rules) = this.state.automod_last_synced.remove(&slug_key) {
                            if still_same_slug {
                                if let Some(ref mut mc) = this.state.mod_community {
                                    if mc.slug.eq_ignore_ascii_case(&slug) {
                                        mc.automod_rules = prev_rules.clone();
                                    }
                                }
                            }
                            if let Some(pos) = this
                                .state
                                .communities
                                .iter()
                                .position(|c| c.slug.eq_ignore_ascii_case(&slug))
                            {
                                this.state.communities[pos].automod_rules = prev_rules.clone();
                            }
                            if let Some(ref mut cdet) = this.state.community_detail {
                                if cdet.community.slug.eq_ignore_ascii_case(&slug) {
                                    cdet.community.automod_rules = prev_rules;
                                }
                            }
                        }
                        if this.state.mod_loading && still_same_slug {
                            this.load_mod_queue(slug.clone(), cx);
                        }
                        if this.state.communities_loading {
                            this.reload_communities_list(cx);
                        }
                        if this.state.community_loading
                            && matches!(
                                &this.state.route,
                                CommunityRoute::CommunityPage(s) if s.eq_ignore_ascii_case(&slug)
                            )
                        {
                            this.load_community_page(slug, cx);
                        }
                        this.state.automod_draft.error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.automod_dirty.remove(&slug_key);
                        this.state.automod_reset_draft_pending.remove(&slug_key);
                        this.state.automod_draft.saving = !this.state.automod_in_flight.is_empty();
                        if let Some(prev_rules) = this.state.automod_last_synced.remove(&slug_key) {
                            if still_same_slug {
                                if let Some(ref mut mc) = this.state.mod_community {
                                    if mc.slug.eq_ignore_ascii_case(&slug) {
                                        mc.automod_rules = prev_rules.clone();
                                    }
                                }
                            }
                            if let Some(pos) = this
                                .state
                                .communities
                                .iter()
                                .position(|c| c.slug.eq_ignore_ascii_case(&slug))
                            {
                                this.state.communities[pos].automod_rules = prev_rules.clone();
                            }
                            if let Some(ref mut cdet) = this.state.community_detail {
                                if cdet.community.slug.eq_ignore_ascii_case(&slug) {
                                    cdet.community.automod_rules = prev_rules;
                                }
                            }
                        }
                        if this.state.mod_loading && still_same_slug {
                            this.load_mod_queue(slug.clone(), cx);
                        }
                        if this.state.communities_loading {
                            this.reload_communities_list(cx);
                        }
                        if this.state.community_loading
                            && matches!(
                                &this.state.route,
                                CommunityRoute::CommunityPage(s) if s.eq_ignore_ascii_case(&slug)
                            )
                        {
                            this.load_community_page(slug, cx);
                        }
                        this.state.automod_draft.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Evaluates the active community's AutoMod rules locally in pure Rust via `AutoModEngine::evaluate`.
    pub fn run_automod_dry_run(&mut self, cx: &mut Context<Self>) {
        let rules = self
            .state
            .mod_community
            .as_ref()
            .map(|c| c.automod_rules.clone())
            .unwrap_or_default();
        let mut sample_actor = self
            .state
            .active_actor()
            .cloned()
            .unwrap_or_else(|| CommunityActorDto {
                actor_id: "u:sample".to_string(),
                linked_user_id: Some(1),
                owner_user_id: None,
                actor_kind: ActorKind::Human,
                handle: "sample_citizen".to_string(),
                display_name: "Sample Citizen".to_string(),
                bio: String::new(),
                location: None,
                website: None,
                avatar_media_id: None,
                banner_media_id: None,
                avatar_shape: "classic".to_string(),
                avatar_color: "blue".to_string(),
                verified_tier: VerifiedTier::None,
                bot_badge: None,
                bot_model: None,
                bot_capabilities: Vec::new(),
                bot_autonomy_mode: Default::default(),
                bot_persona: None,
                post_karma: 10,
                comment_karma: 5,
                awardee_karma: 0,
                followers_count: 1,
                following_count: 1,
                posts_count: 1,
                trust_score: 0.6,
                is_following: false,
                is_muted: false,
                is_blocked: false,
                created_at_ms: 0,
            });
        if self.state.automod_dry_run_is_bot {
            sample_actor.actor_kind = ActorKind::Bot;
            sample_actor.handle = "sample_test_bot".to_string();
        } else {
            sample_actor.actor_kind = ActorKind::Human;
        }
        let title_opt = {
            let t = self.state.automod_dry_run_title.trim();
            if t.is_empty() { None } else { Some(t) }
        };
        let decision = AutoModEngine::evaluate(
            &rules,
            &sample_actor,
            title_opt,
            &self.state.automod_dry_run_body,
        );
        self.state.automod_dry_run_result = Some(decision);
        cx.notify();
    }
}

/// Renders the full F21 Community Moderation Console view.
pub fn render_mod_queue_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let mod_comms = ws.moderated_communities();
    let can_mod = ws.can_moderate_active_mod_community();
    let active_slug = ws.state.mod_slug.clone().or_else(|| {
        mod_comms.first().map(|c| c.slug.clone())
    });
    let active_tab = ws.state.mod_tab;
    let subtitle = active_slug
        .as_deref()
        .map(|s| format!("c/{s}"))
        .or_else(|| {
            Some(
                tr_str(lang, "版主管理台与 AutoMod 引擎", "Moderation & AutoMod Console")
                    .to_string(),
            )
        });

    let tabs = [
        (ModConsoleTab::Queue, "审核队列", "Mod Queue"),
        (ModConsoleTab::AutoModRules, "AutoMod 规则", "AutoMod Rules"),
        (ModConsoleTab::ModLog, "透明审计日志", "Mod Log"),
    ];

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
            tr_str(lang, "社区审核与 AutoMod 管理台", "Community Moderation Console").to_string(),
            subtitle,
            cx,
        ))
        .child(
            div()
                .id("community-mod-scroll")
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
                        // Moderated Community Selector (ONLY shows moderated communities)
                        .when(!mod_comms.is_empty(), |col| {
                            col.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(lang, "管理社区:", "Moderating:")),
                                    )
                                    .children(mod_comms.iter().map(|c| {
                                        let slug = c.slug.clone();
                                        let active = active_slug
                                            .as_deref()
                                            .is_some_and(|s| s.eq_ignore_ascii_case(&slug));
                                        let role_label = c
                                            .viewer_role
                                            .map(|r| r.as_str())
                                            .unwrap_or("mod");
                                        div()
                                            .id(SharedString::from(format!("mod-comm-{slug}")))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.5))
                                            .rounded_full()
                                            .bg(if active {
                                                rgb(pal.accent).into()
                                            } else {
                                                pal.service_pill_rgba()
                                            })
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if active {
                                                pal.active_button_fg
                                            } else {
                                                pal.text_primary
                                            }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.load_mod_queue(slug.clone(), cx);
                                                }),
                                            )
                                            .child(format!("c/{} ({role_label})", c.slug))
                                    })),
                            )
                        })
                        // 0 moderated communities onboarding OR explicit unauthorized slug guard
                        .when(!can_mod && active_tab != ModConsoleTab::ModLog, |col| {
                            col.child(render_mod_unauthorized_panel(
                                active_slug.as_deref(),
                                mod_comms.is_empty(),
                                &pal,
                                lang,
                                cx,
                            ))
                        })
                        // Sub-tabs (shown whenever a community is selected or moderated)
                        .when(can_mod || active_slug.is_some(), |col| {
                            col.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(6.0))
                                    .children(tabs.into_iter().map(|(tab, zh, en)| {
                                        let active = active_tab == tab;
                                        div()
                                            .id(SharedString::from(format!("mod-tab-{tab:?}")))
                                            .cursor_pointer()
                                            .px(px(12.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(if active {
                                                rgb(pal.accent).into()
                                            } else {
                                                pal.service_pill_rgba()
                                            })
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if active {
                                                pal.active_button_fg
                                            } else {
                                                pal.text_primary
                                            }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.set_mod_tab(tab, cx);
                                                }),
                                            )
                                            .child(tr_str(lang, zh, en))
                                    })),
                            )
                        })
                        // Tab Content
                        .when(can_mod && active_tab == ModConsoleTab::Queue, |col| {
                            col.child(render_mod_queue_tab(
                                ws,
                                active_slug.as_deref().unwrap_or(""),
                                cx,
                            ))
                        })
                        .when(can_mod && active_tab == ModConsoleTab::AutoModRules, |col| {
                            col.child(render_automod_rules_tab(ws, cx))
                        })
                        .when(active_tab == ModConsoleTab::ModLog && active_slug.is_some(), |col| {
                            col.child(render_mod_log_tab(ws, cx))
                        }),
                ),
        )
        .into_any_element()
}

fn render_mod_unauthorized_panel(
    slug: Option<&str>,
    no_moderated_any: bool,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let title = if no_moderated_any {
        tr_str(
            lang,
            "当前身份尚未管理任何社区",
            "No moderated communities for this identity",
        )
    } else {
        tr_str(
            lang,
            "无权访问该社区的版主队列",
            "Moderator permission required for this community",
        )
    };
    let subtitle = if let Some(s) = slug.filter(|_| !no_moderated_any) {
        if lang.starts_with("zh") {
            format!("你在 c/{s} 不是 Owner / Mod / BotMod，无法执行审核或编辑 AutoMod 规则。你可以查看公开审计日志、切换拥有版主权限的 Human/Bot 身份，或创建新社区。")
        } else {
            format!("Your active identity is not an Owner, Mod, or BotMod in c/{s}. Switch to a moderator identity, create a new community, or inspect the public Mod Log.")
        }
    } else {
        tr_str(
            lang,
            "创建一个新社区成为 Owner，或切换为担任版主/BotMod 的身份即可开启审核队列与 AutoMod 控制台。",
            "Create a new community to become its Owner, or switch to a Human/Bot identity with moderator permissions.",
        )
        .to_string()
    };

    div()
        .p(px(16.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.0))
        .child(tg_icon(IconName::ShieldMod, 28.0, pal.accent))
        .child(
            div()
                .text_size(px(14.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(pal.text_secondary))
                .child(subtitle),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .child(
                    div()
                        .id("mod-onboard-create-comm-btn")
                        .cursor_pointer()
                        .px(px(13.0))
                        .py(px(7.0))
                        .rounded_full()
                        .bg(rgb(pal.accent))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_create_community_modal(cx);
                            }),
                        )
                        .child(tr_str(lang, "+ 创建新社区", "+ Create Community")),
                )
                .child(
                    div()
                        .id("mod-onboard-switch-id-btn")
                        .cursor_pointer()
                        .px(px(13.0))
                        .py(px(7.0))
                        .rounded_full()
                        .bg(pal.service_pill_rgba())
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
                            }),
                        )
                        .child(tr_str(lang, "切换身份 (Human/Bot)", "Switch Identity")),
                ),
        )
}

fn render_mod_queue_tab(
    ws: &CommunityWorkspace,
    slug: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let status_filter = ws.state.mod_status_filter;
    let reason_foc = ws.state.active_input == ActiveCommunityInput::ModReason;
    let flair_foc = ws.state.active_input == ActiveCommunityInput::ModFlair;
    let statuses = [
        (Some(ModQueueStatus::Pending), "待处理", "Pending"),
        (Some(ModQueueStatus::Approved), "已通过", "Approved"),
        (Some(ModQueueStatus::Removed), "已移除", "Removed"),
        (None, "全部", "All"),
    ];
    let items: Vec<ModQueueItemDto> = ws
        .state
        .mod_queue
        .iter()
        .filter(|it| status_filter.map(|st| it.status == st).unwrap_or(true))
        .cloned()
        .collect();

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Status filter pills + Shared Reason & Optional Flair inputs
        .child(
            div()
                .p(px(11.0))
                .rounded(px(11.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(5.0))
                        .children(statuses.into_iter().map(|(st, zh, en)| {
                            let active = status_filter == st;
                            div()
                                .id(SharedString::from(format!("mod-status-{en}")))
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
                                        this.set_mod_status_filter(st, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("mod-reason-input")
                                .cursor_text()
                                .flex_1()
                                .min_w(px(180.0))
                                .px(px(9.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(if reason_foc {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.hairline_border_rgba()
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.focus_input(ActiveCommunityInput::ModReason);
                                        cx.notify();
                                    }),
                                )
                                .child(render_input_display(
                                    &ws.state.mod_reason_draft,
                                    if reason_foc {
                                        ws.state.ime_preedit.as_deref()
                                    } else {
                                        None
                                    },
                                    tr_str(lang, "审核理由 (记入透明日志)…", "Moderation reason (recorded in Mod Log)…"),
                                    reason_foc,
                                    &pal,
                                )),
                        )
                        .child(
                            div()
                                .id("mod-flair-input")
                                .cursor_text()
                                .w(px(170.0))
                                .px(px(9.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(if flair_foc {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.hairline_border_rgba()
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.focus_input(ActiveCommunityInput::ModFlair);
                                        cx.notify();
                                    }),
                                )
                                .child(render_input_display(
                                    &ws.state.mod_flair_draft,
                                    if flair_foc {
                                        ws.state.ime_preedit.as_deref()
                                    } else {
                                        None
                                    },
                                    tr_str(lang, "设置 Flair (可选)", "Set Flair (optional)"),
                                    flair_foc,
                                    &pal,
                                )),
                        ),
                ),
        )
        .when(ws.state.mod_loading && items.is_empty(), |col| {
            col.child(render_loading_skeleton(3, &pal))
        })
        .when(!ws.state.mod_loading && items.is_empty(), |col| {
            col.child(render_empty_state(
                IconName::ShieldMod,
                tr_str(lang, "当前审核队列已清空", "Moderation queue is clear"),
                tr_str(
                    lang,
                    "所有举报与 AutoMod 触发项均已处理完毕。",
                    "All reports and AutoMod triggers in this view are resolved.",
                ),
                &pal,
            ))
        })
        .children(items.into_iter().map(|it| render_mod_queue_item_card(ws, slug, it, cx)))
}

fn render_mod_queue_item_card(
    ws: &CommunityWorkspace,
    slug: &str,
    item: ModQueueItemDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let is_post = item.target_kind.eq_ignore_ascii_case("post");
    let is_comment = item.target_kind.eq_ignore_ascii_case("comment");
    let is_actor = item.target_kind.eq_ignore_ascii_case("actor");
    let target_post_author = if is_post {
        ws.state
            .find_post(&item.target_id)
            .map(|p| p.author_actor_id.clone())
    } else {
        None
    };
    let actor_target = if is_actor {
        Some(item.target_id.clone())
    } else {
        target_post_author
    };

    let make_btn = |id_suffix: &str,
                    label: &'static str,
                    color_rgb: u32,
                    action: ModActionKind,
                    t_kind: String,
                    t_id: String,
                    q_id: Option<String>,
                    cx: &mut Context<CommunityWorkspace>| {
        let s = slug.to_string();
        div()
            .id(SharedString::from(format!("mod-act-{}-{}", item.item_id, id_suffix)))
            .cursor_pointer()
            .px(px(9.0))
            .py(px(4.5))
            .rounded(px(7.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(color_rgb, 0.16))
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(color_rgb))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.execute_mod_console_action(
                        s.clone(),
                        action,
                        t_kind.clone(),
                        t_id.clone(),
                        q_id.clone(),
                        cx,
                    );
                }),
            )
            .child(label)
    };

    let mut actions = div().flex().flex_wrap().gap(px(5.0));
    if is_post || is_comment {
        let tk = item.target_kind.clone();
        let tid = item.target_id.clone();
        let qid = Some(item.item_id.clone());
        actions = actions
            .child(make_btn("approve", tr_str(lang, "通过", "Approve"), 0x10B981, ModActionKind::Approve, tk.clone(), tid.clone(), qid.clone(), cx))
            .child(make_btn("remove", tr_str(lang, "移除", "Remove"), pal.danger, ModActionKind::Remove, tk.clone(), tid.clone(), qid.clone(), cx))
            .child(make_btn("pin", tr_str(lang, "置顶", "Pin"), pal.accent, ModActionKind::Pin, tk.clone(), tid.clone(), None, cx))
            .child(make_btn("dist", tr_str(lang, "版主徽章", "Distinguish"), 0x10B981, ModActionKind::Distinguish, tk.clone(), tid.clone(), None, cx));
        if is_post {
            actions = actions
                .child(make_btn("lock", tr_str(lang, "锁定", "Lock"), 0xF59E0B, ModActionKind::Lock, "post".to_string(), tid.clone(), None, cx))
                .child(make_btn("flair", tr_str(lang, "设 Flair", "Set Flair"), pal.accent, ModActionKind::SetPostFlair, "post".to_string(), tid.clone(), None, cx));
        }
    }
    if let Some(aid) = actor_target {
        actions = actions
            .child(make_btn("mute", tr_str(lang, "禁言作者", "Mute"), 0xF59E0B, ModActionKind::MuteActor, "actor".to_string(), aid.clone(), None, cx))
            .child(make_btn("ban", tr_str(lang, "封禁作者", "Ban"), pal.danger, ModActionKind::BanActor, "actor".to_string(), aid, None, cx));
    }

    let inspect_pid = if is_post { Some(item.target_id.clone()) } else { None };
    div()
        .p(px(12.0))
        .rounded(px(11.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                    if item.source == "automod" { 0x8B5CF6 } else { 0xF59E0B },
                                    0.18,
                                ))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(if item.source == "automod" {
                                    0x8B5CF6
                                } else {
                                    0xF59E0B
                                }))
                                .child(item.source.to_ascii_uppercase()),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("[{}] {}", item.target_kind, item.reason)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format_relative_time(item.created_at_ms, lang)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .text_size(px(11.0))
                .text_color(rgb(pal.text_secondary))
                .child(format!("target_id: {}", item.target_id))
                .when_some(item.rule_triggered.clone(), |row, rule| {
                    row.child(format!("· rule: {rule}"))
                })
                .when_some(item.reporter_actor_id.clone(), |row, rep| {
                    row.child(format!("· reporter: {rep}"))
                })
                .when_some(inspect_pid, |row, pid| {
                    row.child(
                        div()
                            .id(SharedString::from(format!("mod-open-post-{}", item.item_id)))
                            .cursor_pointer()
                            .text_color(rgb(pal.accent))
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_post_detail(pid.clone(), cx);
                                }),
                            )
                            .child(tr_str(lang, "查看原帖 →", "Inspect Post →")),
                    )
                }),
        )
        .child(actions)
}

fn render_mod_log_tab(ws: &CommunityWorkspace, _cx: &mut Context<CommunityWorkspace>) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let logs: Vec<ModLogEntryDto> = ws.state.mod_logs.clone();
    let base_url = ws.media_base_url.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when(logs.is_empty(), |col| {
            col.child(render_empty_state(
                IconName::ShieldMod,
                tr_str(lang, "暂无审计日志记录", "No moderation log entries yet"),
                tr_str(
                    lang,
                    "所有版主与 BotMod 的审核操作均会公开透明地记录于此。",
                    "Every moderator and BotMod action is transparently logged here.",
                ),
                &pal,
            ))
        })
        .children(logs.into_iter().map(|log| {
            let color = mod_action_badge_color(log.action, &pal);
            let shape = if log.moderator_kind.is_bot_like() {
                "blob"
            } else {
                "classic"
            };
            let hue = if log.moderator_kind.is_bot_like() {
                "violet"
            } else {
                "green"
            };
            div()
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(render_actor_avatar(
                            &log.moderator_actor_id,
                            &log.moderator_handle,
                            log.moderator_kind,
                            None,
                            shape,
                            hue,
                            28.0,
                            pal.card_bg,
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
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(format!("@{}", log.moderator_handle)),
                                        )
                                        .child(render_actor_badge_row(
                                            log.moderator_kind,
                                            VerifiedTier::CommunityMod,
                                            None,
                                            true,
                                            false,
                                            hue,
                                            &pal,
                                            lang,
                                        ))
                                        .child(
                                            div()
                                                .px(px(7.0))
                                                .py(px(1.5))
                                                .rounded_full()
                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                    color, 0.16,
                                                ))
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(color))
                                                .child(log.action.as_str()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "[{}] {} · {}",
                                            log.target_kind,
                                            log.target_id,
                                            log.reason.unwrap_or_default()
                                        )),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format_relative_time(log.created_at_ms, lang)),
                )
        }))
}

fn render_automod_input_box(
    id: &'static str,
    target: ActiveCommunityInput,
    value: &str,
    placeholder: &str,
    multiline: bool,
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
        .when(multiline, |d| d.min_h(px(44.0)))
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
                this.state.focus_input(target);
                cx.notify();
            }),
        )
        .child(render_input_display(value, preedit, placeholder, foc, &pal))
}

fn render_automod_rules_tab(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let rules = ws
        .state
        .mod_community
        .as_ref()
        .map(|c| c.automod_rules.clone())
        .unwrap_or_default();
    let draft = &ws.state.automod_draft;
    let dry_res = ws.state.automod_dry_run_result.clone();
    let actions = [
        AutoModActionKind::FlagForReview,
        AutoModActionKind::Remove,
        AutoModActionKind::AssignFlair,
        AutoModActionKind::AutoReply,
        AutoModActionKind::Approve,
    ];

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        // 1. Existing Rules List
        .child(
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
                            tr_str(lang, "已配置的 AutoMod 规则", "Configured AutoMod Rules"),
                            rules.len()
                        )),
                )
                .when(rules.is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "暂无 AutoMod 规则，在下方添加一条声明式自动化审核规则。",
                                "No AutoMod rules yet. Add a declarative rule below.",
                            )),
                    )
                })
                .children(rules.into_iter().map(|r| {
                    let rid_toggle = r.rule_id.clone();
                    let rid_del = r.rule_id.clone();
                    let enabled = r.enabled;
                    let mut conds = Vec::new();
                    if !r.keywords.is_empty() {
                        conds.push(format!("keywords: [{}]", r.keywords.join(", ")));
                    }
                    if let Some(ref rx) = r.regex_pattern {
                        conds.push(format!("regex: /{rx}/"));
                    }
                    if let Some(mk) = r.min_author_karma {
                        conds.push(format!("min_karma: {mk}"));
                    }
                    if let Some(ml) = r.max_links {
                        conds.push(format!("max_links: {ml}"));
                    }
                    div()
                        .p(px(10.0))
                        .rounded(px(9.0))
                        .bg(pal.service_pill_rgba())
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(r.name),
                                        )
                                        .child(
                                            div()
                                                .px(px(7.0))
                                                .py(px(1.5))
                                                .rounded_full()
                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                    pal.accent, 0.16,
                                                ))
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.accent))
                                                .child(automod_action_label(r.action, lang)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(conds.join(" · ")),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("am-toggle-{}", r.rule_id)))
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(4.0))
                                        .rounded_full()
                                        .bg(if enabled {
                                            TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16)
                                        } else {
                                            pal.card_surface_rgba()
                                        })
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if enabled {
                                            0x10B981
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.toggle_automod_rule_enabled(
                                                    rid_toggle.clone(),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(if enabled {
                                            tr_str(lang, "已启用", "Enabled")
                                        } else {
                                            tr_str(lang, "已禁用", "Disabled")
                                        }),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("am-del-{}", r.rule_id)))
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded_full()
                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                            pal.danger, 0.14,
                                        ))
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.danger))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.delete_automod_rule(rid_del.clone(), cx);
                                            }),
                                        )
                                        .child(tr_str(lang, "删除", "Delete")),
                                ),
                        )
                })),
        )
        // 2. Add AutoMod Rule Form
        .child(
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
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "新增 AutoMod 规则", "Add AutoMod Rule")),
                )
                .child(render_automod_input_box(
                    "am-name-input",
                    ActiveCommunityInput::AutoModRuleName,
                    &draft.name,
                    tr_str(lang, "规则名称 (如: 反广告链接守卫)", "Rule name (e.g. Anti-Spam Guard)"),
                    false,
                    ws,
                    cx,
                ))
                .child(render_automod_input_box(
                    "am-keywords-input",
                    ActiveCommunityInput::AutoModRuleKeywords,
                    &draft.keywords,
                    tr_str(lang, "关键词列表，逗号分隔 (可选)", "Keywords, comma-separated (optional)"),
                    false,
                    ws,
                    cx,
                ))
                .child(render_automod_input_box(
                    "am-regex-input",
                    ActiveCommunityInput::AutoModRuleRegex,
                    &draft.regex_pattern,
                    tr_str(lang, "正则表达式，如 (scam|airdrop) (可选)", "Regex pattern, e.g. (scam|airdrop) (optional)"),
                    false,
                    ws,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(div().flex_1().child(render_automod_input_box(
                            "am-karma-input",
                            ActiveCommunityInput::AutoModRuleMinKarma,
                            &draft.min_karma,
                            tr_str(lang, "最低 Karma 门槛 (可选)", "Min karma floor (optional)"),
                            false,
                            ws,
                            cx,
                        )))
                        .child(div().flex_1().child(render_automod_input_box(
                            "am-maxlinks-input",
                            ActiveCommunityInput::AutoModRuleMaxLinks,
                            &draft.max_links,
                            tr_str(lang, "最大外链数量 (可选)", "Max links limit (optional)"),
                            false,
                            ws,
                            cx,
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(5.0))
                        .children(actions.into_iter().map(|act| {
                            let active = draft.action == act;
                            div()
                                .id(SharedString::from(format!("am-action-{act:?}")))
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
                                        this.state.automod_draft.action = act;
                                        cx.notify();
                                    }),
                                )
                                .child(automod_action_label(act, lang))
                        })),
                )
                .when(draft.action == AutoModActionKind::AssignFlair, |col| {
                    col.child(render_automod_input_box(
                        "am-flair-input",
                        ActiveCommunityInput::AutoModRuleFlair,
                        &draft.assign_flair,
                        tr_str(lang, "自动分配的 Flair 标签", "Flair to assign"),
                        false,
                        ws,
                        cx,
                    ))
                })
                .when(draft.action == AutoModActionKind::AutoReply, |col| {
                    col.child(render_automod_input_box(
                        "am-reply-input",
                        ActiveCommunityInput::AutoModRuleReply,
                        &draft.auto_reply_template,
                        tr_str(
                            lang,
                            "自动回复模板，支持 {handle} 和 {community}",
                            "Auto-reply template (supports {handle} and {community})",
                        ),
                        true,
                        ws,
                        cx,
                    ))
                })
                .when_some(draft.error.clone(), |col, err| {
                    col.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(pal.danger))
                            .child(err),
                    )
                })
                .child(
                    div()
                        .id("am-save-rule-btn")
                        .cursor_pointer()
                        .py(px(8.0))
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
                                this.submit_add_automod_rule(cx);
                            }),
                        )
                        .child(tr_str(lang, "保存 AutoMod 规则", "Add & Save AutoMod Rule")),
                ),
        )
        // 3. Interactive Pure-Rust "Dry-Run Test AutoMod" Bar
        .child(
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
                                    "即时沙盒演练 (Dry-Run Test AutoMod)",
                                    "Dry-Run Test AutoMod (Live Engine)",
                                )),
                        )
                        .child(
                            div()
                                .id("am-dryrun-actor-toggle")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.automod_dry_run_is_bot =
                                            !this.state.automod_dry_run_is_bot;
                                        cx.notify();
                                    }),
                                )
                                .child(if ws.state.automod_dry_run_is_bot {
                                    tr_str(lang, "模拟作者: Bot", "Sample Author: Bot")
                                } else {
                                    tr_str(lang, "模拟作者: 人类", "Sample Author: Human")
                                }),
                        ),
                )
                .child(render_automod_input_box(
                    "am-dryrun-title-input",
                    ActiveCommunityInput::AutoModDryRunTitle,
                    &ws.state.automod_dry_run_title,
                    tr_str(lang, "测试标题 (可选)", "Sample title (optional)"),
                    false,
                    ws,
                    cx,
                ))
                .child(render_automod_input_box(
                    "am-dryrun-body-input",
                    ActiveCommunityInput::AutoModDryRunBody,
                    &ws.state.automod_dry_run_body,
                    tr_str(lang, "输入待测试帖子正文以实时评估规则命中结果…", "Enter sample post body to evaluate rules live…"),
                    true,
                    ws,
                    cx,
                ))
                .child(
                    div()
                        .id("am-dryrun-eval-btn")
                        .cursor_pointer()
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.16))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0x8B5CF6))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.run_automod_dry_run(cx);
                            }),
                        )
                        .child(tr_str(lang, "运行 AutoMod 演练", "Run Dry-Run Evaluation")),
                )
                .when_some(dry_res, |col, dec| {
                    let matched = if dec.matched_rule_names.is_empty() {
                        tr_str(lang, "无规则触发（直接通过）", "No rules matched (Approved)").to_string()
                    } else {
                        dec.matched_rule_names.join(", ")
                    };
                    col.child(
                        div()
                            .p(px(9.0))
                            .rounded(px(8.0))
                            .bg(pal.service_pill_rgba())
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!(
                                        "{}: {}",
                                        tr_str(lang, "判定结果", "Decision"),
                                        automod_action_label(dec.action, lang)
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "{}: {matched}",
                                        tr_str(lang, "命中规则", "Matched rules")
                                    )),
                            )
                            .when(!dec.reasons.is_empty(), |box_div| {
                                box_div.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(dec.reasons.join(" | ")),
                                )
                            }),
                    )
                }),
        )
}
