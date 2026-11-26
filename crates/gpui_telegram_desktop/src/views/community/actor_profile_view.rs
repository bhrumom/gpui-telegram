//! Feature F20-Profile — Unified First-Class Human & Bot `@handle` Profile View
//! (Banner, Grok/Human Avatar, Bot Citizenship Card, Dual Reddit Karma + X Social Stats,
//! Telegram DM Bridge, Follow/Mute/Block/Act-As, 8 Tabs, and Edit Profile Modal).

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::community_contracts::{
    BotAutonomyMode, CommunityActorDto, CommunityPostDto, FeedItemDto, MuteBlockActionKind,
    MuteBlockRequest, MuteBlockTargetKind, PostStatus, UpdateCommunityActorRequest,
};
use gpui_telegram_proto::miniapp_contracts::GROK_AVATAR_SHAPES;

use crate::components::avatar::parse_fabushi_hex_color;
use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, GrokBotAvatarProps, GROK_COLOR_PALETTE,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_modal_backdrop_rgba, macos_modal_card, macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::stage::entities_render::render_rich_message_body;

use super::post_card::render_post_card;
use super::shared_widgets::{
    format_compact_count, format_relative_time, linkify_community_slugs, render_actor_avatar,
    render_actor_badge_row, render_empty_state, render_flair_chip, render_input_display,
    render_loading_skeleton,
};
use super::state::{
    ActiveCommunityInput, ActorProfileTab, CommunityModal, CommunityRoute,
    CommunityWorkspaceAction, EditProfileDraft,
};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

fn resolve_actor_banner_rgb(avatar_color: &str, pal: &TdesktopThemePalette) -> u32 {
    if let Some(spec) = GROK_COLOR_PALETTE
        .iter()
        .find(|s| s.id.eq_ignore_ascii_case(avatar_color.trim()))
    {
        return spec.swatch_rgb;
    }
    parse_fabushi_hex_color(avatar_color).unwrap_or(pal.accent)
}

/// Formats `created_at_ms` as a clean UTC calendar date (`YYYY-MM-DD`) without external deps.
fn format_cake_day(created_at_ms: i64) -> String {
    if created_at_ms <= 0 {
        return "2026-01-01".to_string();
    }
    let days = (created_at_ms / 86_400_000).max(0);
    // Howard Hinnant's civil_from_days algorithm (exact Gregorian calendar)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}")
}

fn autonomy_label(mode: BotAutonomyMode, lang: &str) -> &'static str {
    match mode {
        BotAutonomyMode::Manual => tr_str(lang, "手动控制 (Manual)", "Manual"),
        BotAutonomyMode::MentionOnly => tr_str(lang, "仅 @提及触发 (MentionOnly)", "MentionOnly"),
        BotAutonomyMode::Reactive => tr_str(lang, "话题响应 (Reactive)", "Reactive"),
        BotAutonomyMode::Autonomous => tr_str(lang, "自主活跃 (Autonomous)", "Autonomous"),
    }
}

impl CommunityWorkspace {
    pub fn set_profile_tab(&mut self, tab: ActorProfileTab, cx: &mut Context<Self>) {
        if self.state.profile_tab != tab {
            self.state.profile_tab = tab;
            cx.notify();
        }
    }

    pub fn toggle_mute_actor(
        &mut self,
        actor_id: String,
        currently_muted: bool,
        cx: &mut Context<Self>,
    ) {
        if actor_id == self.state.active_actor_id {
            return;
        }
        let effective_current = self
            .state
            .actor_mute_pending_target
            .get(&actor_id)
            .copied()
            .unwrap_or(currently_muted);
        let next_muted = !effective_current;
        if !self.state.actor_mute_last_synced.contains_key(&actor_id) {
            let orig_muted = self
                .state
                .find_actor(&actor_id)
                .map(|a| a.is_muted)
                .unwrap_or(effective_current);
            self.state
                .actor_mute_last_synced
                .insert(actor_id.clone(), orig_muted);
        }
        self.state
            .actor_mute_pending_target
            .insert(actor_id.clone(), next_muted);
        self.state
            .reconcile_mute_block(&actor_id, Some(next_muted), None);
        cx.notify();
        if self.state.actor_mute_in_flight.contains_key(&actor_id) {
            self.state.actor_mute_dirty.insert(actor_id);
            return;
        }
        self.flush_actor_mute_sync(actor_id, cx);
    }

    fn flush_actor_mute_sync(&mut self, actor_id: String, cx: &mut Context<Self>) {
        let Some(target_muted) = self.state.actor_mute_pending_target.get(&actor_id).copied()
        else {
            self.state.actor_mute_dirty.remove(&actor_id);
            self.state.actor_mute_last_synced.remove(&actor_id);
            return;
        };
        self.state.actor_mute_dirty.remove(&actor_id);
        self.state.actor_mute_flight_seq = self.state.actor_mute_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.actor_mute_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .actor_mute_in_flight
            .insert(actor_id.clone(), my_flight_seq);
        let req = MuteBlockRequest {
            target_kind: MuteBlockTargetKind::Actor,
            target_value: actor_id.clone(),
            action: MuteBlockActionKind::Mute,
            reason: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                if target_muted {
                    facade.set_mute_block(&req).await.map(|_| ())
                } else {
                    facade.remove_mute_block(&req).await.map(|_| ())
                }
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.actor_mute_in_flight.get(&actor_id).copied() != Some(my_flight_seq) {
                    return;
                }
                this.state.actor_mute_in_flight.remove(&actor_id);
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(())) => {
                        this.state
                            .actor_mute_last_synced
                            .insert(actor_id.clone(), target_muted);
                        let is_dirty = this.state.actor_mute_dirty.remove(&actor_id);
                        if is_dirty
                            && this
                                .state
                                .actor_mute_pending_target
                                .get(&actor_id)
                                .copied()
                                .is_some_and(|want| want != target_muted)
                        {
                            this.flush_actor_mute_sync(actor_id, cx);
                            return;
                        }
                        this.state.actor_mute_pending_target.remove(&actor_id);
                        this.state.actor_mute_last_synced.remove(&actor_id);
                        this.state
                            .reconcile_mute_block(&actor_id, Some(target_muted), None);
                        this.reload_actor_route_if_loading(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: if target_muted {
                                    tr_str(&lang, "已静音该公民", "Actor muted").to_string()
                                } else {
                                    tr_str(&lang, "已取消静音", "Actor unmuted").to_string()
                                },
                                is_error: false,
                            },
                            cx,
                        );
                        cx.notify();
                    }
                    Ok(Err(e)) => {
                        this.state.actor_mute_dirty.remove(&actor_id);
                        this.state.actor_mute_pending_target.remove(&actor_id);
                        let orig_muted = this
                            .state
                            .actor_mute_last_synced
                            .remove(&actor_id)
                            .unwrap_or(!target_muted);
                        this.state
                            .reconcile_mute_block(&actor_id, Some(orig_muted), None);
                        this.reload_actor_route_if_loading(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                        cx.notify();
                    }
                    Err(_) => {
                        this.state.actor_mute_dirty.remove(&actor_id);
                        this.state.actor_mute_pending_target.remove(&actor_id);
                        let orig_muted = this
                            .state
                            .actor_mute_last_synced
                            .remove(&actor_id)
                            .unwrap_or(!target_muted);
                        this.state
                            .reconcile_mute_block(&actor_id, Some(orig_muted), None);
                        this.reload_actor_route_if_loading(cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn toggle_block_actor(
        &mut self,
        actor_id: String,
        currently_blocked: bool,
        cx: &mut Context<Self>,
    ) {
        if actor_id == self.state.active_actor_id {
            return;
        }
        let effective_current = self
            .state
            .actor_block_pending_target
            .get(&actor_id)
            .copied()
            .unwrap_or(currently_blocked);
        let next_blocked = !effective_current;
        if !self.state.actor_block_last_synced.contains_key(&actor_id) {
            let orig = self
                .state
                .find_actor(&actor_id)
                .map(|a| (a.is_blocked, a.is_following))
                .unwrap_or((effective_current, false));
            self.state
                .actor_block_last_synced
                .insert(actor_id.clone(), orig);
        }
        self.state
            .actor_block_pending_target
            .insert(actor_id.clone(), next_blocked);
        if next_blocked {
            self.state
                .actor_block_chain_had_block
                .insert(actor_id.clone());
            // Cross-lane Follow -> Block fence: invalidate any in-flight follow flight
            // so a delayed Follow response can NEVER set `is_following = true` onto a blocked actor.
            self.state.actor_follow_in_flight.remove(&actor_id);
            self.state.actor_follow_dirty.remove(&actor_id);
            self.state.actor_follow_pending_target.remove(&actor_id);
            self.state.actor_follow_last_synced.remove(&actor_id);
        }
        self.state
            .reconcile_mute_block(&actor_id, None, Some(next_blocked));
        cx.notify();
        if self.state.actor_block_in_flight.contains_key(&actor_id) {
            self.state.actor_block_dirty.insert(actor_id);
            return;
        }
        self.flush_actor_block_sync(actor_id, cx);
    }

    fn flush_actor_block_sync(&mut self, actor_id: String, cx: &mut Context<Self>) {
        let Some(target_blocked) = self.state.actor_block_pending_target.get(&actor_id).copied()
        else {
            self.state.actor_block_dirty.remove(&actor_id);
            self.state.actor_block_chain_had_block.remove(&actor_id);
            self.state.actor_block_last_synced.remove(&actor_id);
            return;
        };
        if target_blocked {
            self.state
                .actor_block_chain_had_block
                .insert(actor_id.clone());
        }
        self.state.actor_block_dirty.remove(&actor_id);
        self.state.actor_block_flight_seq = self.state.actor_block_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.actor_block_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .actor_block_in_flight
            .insert(actor_id.clone(), my_flight_seq);
        let req = MuteBlockRequest {
            target_kind: MuteBlockTargetKind::Actor,
            target_value: actor_id.clone(),
            action: MuteBlockActionKind::Block,
            reason: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                if target_blocked {
                    facade.set_mute_block(&req).await.map(|_| ())
                } else {
                    facade.remove_mute_block(&req).await.map(|_| ())
                }
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.actor_block_in_flight.get(&actor_id).copied() != Some(my_flight_seq)
                {
                    return;
                }
                this.state.actor_block_in_flight.remove(&actor_id);
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(())) => {
                        let committed_following = if target_blocked {
                            false
                        } else {
                            this.state
                                .actor_block_last_synced
                                .get(&actor_id)
                                .map(|&(_, f)| f)
                                .unwrap_or(false)
                        };
                        this.state
                            .actor_block_last_synced
                            .insert(actor_id.clone(), (target_blocked, committed_following));
                        let is_dirty = this.state.actor_block_dirty.remove(&actor_id);
                        if is_dirty
                            && this
                                .state
                                .actor_block_pending_target
                                .get(&actor_id)
                                .copied()
                                .is_some_and(|want| want != target_blocked)
                        {
                            this.flush_actor_block_sync(actor_id, cx);
                            return;
                        }
                        this.state.actor_block_pending_target.remove(&actor_id);
                        this.state.actor_block_chain_had_block.remove(&actor_id);
                        this.state.actor_block_last_synced.remove(&actor_id);
                        this.state.restore_block_and_follow(
                            &actor_id,
                            target_blocked,
                            committed_following,
                        );
                        this.reload_actor_route_if_loading(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: if target_blocked {
                                    tr_str(&lang, "已屏蔽该公民并取消互关", "Actor blocked")
                                        .to_string()
                                } else {
                                    tr_str(&lang, "已解除屏蔽", "Actor unblocked").to_string()
                                },
                                is_error: false,
                            },
                            cx,
                        );
                        cx.notify();
                    }
                    Ok(Err(e)) => {
                        this.state.actor_block_dirty.remove(&actor_id);
                        this.state.actor_block_pending_target.remove(&actor_id);
                        this.state.actor_block_chain_had_block.remove(&actor_id);
                        let (orig_blocked, orig_following) = this
                            .state
                            .actor_block_last_synced
                            .remove(&actor_id)
                            .unwrap_or((!target_blocked, false));
                        this.state
                            .restore_block_and_follow(&actor_id, orig_blocked, orig_following);
                        this.reload_actor_route_if_loading(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                        cx.notify();
                    }
                    Err(_) => {
                        this.state.actor_block_dirty.remove(&actor_id);
                        this.state.actor_block_pending_target.remove(&actor_id);
                        this.state.actor_block_chain_had_block.remove(&actor_id);
                        let (orig_blocked, orig_following) = this
                            .state
                            .actor_block_last_synced
                            .remove(&actor_id)
                            .unwrap_or((!target_blocked, false));
                        this.state
                            .restore_block_and_follow(&actor_id, orig_blocked, orig_following);
                        this.reload_actor_route_if_loading(cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn open_edit_profile_modal(&mut self, actor: CommunityActorDto, cx: &mut Context<Self>) {
        self.state.edit_profile = EditProfileDraft {
            actor_id: actor.actor_id,
            is_bot: actor.actor_kind.is_bot_like(),
            display_name: actor.display_name,
            bio: actor.bio,
            location: actor.location.unwrap_or_default(),
            website: actor.website.unwrap_or_default(),
            avatar_shape: actor.avatar_shape,
            avatar_color: actor.avatar_color,
            bot_badge: actor.bot_badge.unwrap_or_default(),
            bot_model: actor.bot_model.unwrap_or_default(),
            bot_autonomy_mode: actor.bot_autonomy_mode,
            bot_persona: actor.bot_persona.unwrap_or_default(),
            submitting: false,
            error: None,
        };
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::EditProfile);
        self.state.focus_input(ActiveCommunityInput::ProfileEditDisplayName);
        cx.notify();
    }

    pub fn close_edit_profile_modal(&mut self, cx: &mut Context<Self>) {
        if self.state.active_modal == Some(CommunityModal::EditProfile) {
            self.state.active_modal = None;
        }
        if self.state.active_input.is_profile_edit_input() {
            self.state.blur_input();
        }
        cx.notify();
    }

    pub fn submit_edit_profile(&mut self, cx: &mut Context<Self>) {
        if self.state.edit_profile.submitting {
            return;
        }
        let lang = self.language_id.clone();
        let d = self.state.edit_profile.clone();
        let display_name = d.display_name.trim().to_string();
        if display_name.is_empty() || display_name.chars().count() > 64 {
            self.state.edit_profile.error = Some(
                tr_str(&lang, "昵称需为 1–64 个字符", "Display name must be 1–64 characters")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        // Empty strings `Some("".into())` explicitly clear optional fields on server & local store.
        let req = UpdateCommunityActorRequest {
            display_name: Some(display_name),
            bio: Some(d.bio.trim().to_string()),
            location: Some(d.location.trim().to_string()),
            website: Some(d.website.trim().to_string()),
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: d.is_bot.then_some(d.avatar_shape),
            avatar_color: d.is_bot.then_some(d.avatar_color),
            bot_badge: d.is_bot.then_some(d.bot_badge.trim().to_string()),
            bot_model: d.is_bot.then_some(d.bot_model.trim().to_string()),
            bot_capabilities: None,
            bot_autonomy_mode: d.is_bot.then_some(d.bot_autonomy_mode),
            bot_persona: d.is_bot.then_some(d.bot_persona.trim().to_string()),
        };
        let aid = d.actor_id.clone();
        self.state.edit_profile.submitting = true;
        self.state.edit_profile.error = None;
        self.state.identities_gen = self.state.identities_gen.wrapping_add(1);
        let my_id_gen = self.state.identities_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let updated = facade.update_actor(&aid, &req).await?;
                let ids = facade.get_identities().await.ok();
                Ok::<_, gpui_telegram_core::TelegramError>((updated, ids))
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen {
                    if this.state.identities_gen == my_id_gen {
                        if let Ok(Ok((_, Some(id_resp)))) = res {
                            this.state.identities = Some(id_resp);
                            this.ensure_bot_studio_selected_hydrated(cx);
                            cx.notify();
                        }
                    }
                    return;
                }
                this.state.edit_profile.submitting = false;
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok((updated, ids))) => {
                        if let Some(ref mut prof) = this.state.actor_profile {
                            if prof.actor.actor_id == updated.actor_id {
                                prof.actor = updated.clone();
                            }
                        }
                        if this.state.profile_loading {
                            if let CommunityRoute::ActorProfile(h) = this.state.route.clone() {
                                this.load_actor_profile(h, cx);
                            }
                        }
                        if this.state.identities_gen == my_id_gen {
                            if let Some(id_resp) = ids {
                                this.apply_identities_response(id_resp, cx);
                            }
                        }
                        this.close_edit_profile_modal(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "个人资料已更新！", "Profile updated!").to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        this.state.edit_profile.error = Some(e.to_string());
                        cx.notify();
                    }
                    Err(e) => {
                        this.state.edit_profile.error = Some(e);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// Renders the full F20-Profile Unified First-Class Human & Bot `@handle` Profile view.
pub fn render_actor_profile_view(
    ws: &CommunityWorkspace,
    handle: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let prof = ws.state.actor_profile.clone();

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
            format!("@{handle}"),
            prof.as_ref().map(|p| p.actor.display_name.clone()),
            cx,
        ))
        .child(
            div()
                .id("community-profile-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .when(ws.state.profile_loading && prof.is_none(), |col| {
                            col.child(render_loading_skeleton(2, &pal))
                        })
                        .when(!ws.state.profile_loading && prof.is_none(), |col| {
                            let sub = ws.state.profile_error.as_deref().unwrap_or_else(|| {
                                tr_str(
                                    lang,
                                    "未找到该人类或 Bot 公民主页。",
                                    "This Human or Bot citizen profile could not be found.",
                                )
                            });
                            col.child(render_empty_state(
                                IconName::BotRobot,
                                tr_str(lang, "未找到该用户或 Bot", "Actor not found"),
                                sub,
                                &pal,
                            ))
                        })
                        .when_some(prof, |col, p| {
                            col.child(render_profile_header_card(ws, &p.actor, cx))
                                .when(p.actor.actor_kind.is_bot_like(), |c| {
                                    c.child(render_bot_citizenship_card(ws, &p.actor, cx))
                                })
                                .child(render_profile_tabs_bar(ws, &p.actor, cx))
                                .child(render_profile_tab_body(ws, &p, cx))
                        }),
                ),
        )
        .into_any_element()
}

fn render_profile_header_card(
    ws: &CommunityWorkspace,
    actor: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let banner_rgb = resolve_actor_banner_rgb(&actor.avatar_color, &pal);
    let is_self_active = actor.actor_id == ws.state.active_actor_id;
    let is_owned = ws.is_viewer_owned_actor(&actor.actor_id);
    let viewer_human_uid = ws
        .state
        .identities
        .as_ref()
        .and_then(|ids| ids.human_actor.as_ref())
        .and_then(|h| h.linked_user_id);
    let dm_uid = actor
        .linked_user_id
        .filter(|&uid| uid > 0 && Some(uid) != viewer_human_uid);

    let aid_follow = actor.actor_id.clone();
    let following = actor.is_following;
    let aid_mute = actor.actor_id.clone();
    let muted = actor.is_muted;
    let aid_block = actor.actor_id.clone();
    let blocked = actor.is_blocked;
    let aid_act_as = actor.actor_id.clone();
    let edit_actor = actor.clone();
    let hover_bg = pal.row_hover_rgba();

    div()
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .overflow_hidden()
        .flex()
        .flex_col()
        // Accent Banner Strip
        .child(
            div()
                .w_full()
                .h(px(72.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(banner_rgb, 0.28))
                .border_b_1()
                .border_color(TdesktopThemePalette::rgba_with_alpha(banner_rgb, 0.42)),
        )
        .child(
            div()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                // Top Row: Avatar + Name + Action Buttons
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .flex_wrap()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(render_actor_avatar(
                                    &actor.actor_id,
                                    &actor.display_name,
                                    actor.actor_kind,
                                    actor.avatar_media_id.as_deref(),
                                    &actor.avatar_shape,
                                    &actor.avatar_color,
                                    56.0,
                                    pal.card_bg,
                                    &ws.media_base_url,
                                    false,
                                    &pal,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .flex_wrap()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_size(px(16.5))
                                                        .font_weight(FontWeight::BOLD)
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
                                                .text_size(px(12.0))
                                                .text_color(rgb(pal.text_secondary))
                                                .child(format!(
                                                    "@{} · 🎂 {} {}",
                                                    actor.handle,
                                                    tr_str(lang, "加入于", "Cake Day"),
                                                    format_cake_day(actor.created_at_ms)
                                                )),
                                        ),
                                ),
                        )
                        // Action Bar
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .flex_wrap()
                                .gap(px(6.0))
                                .when(is_owned && !is_self_active, |row| {
                                    row.child(
                                        div()
                                            .id("profile-act-as-btn")
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                            .hover(move |s| s.bg(hover_bg))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.switch_active_actor(aid_act_as.clone(), cx);
                                                }),
                                            )
                                            .child(tr_str(lang, "切换为此身份", "Act As")),
                                    )
                                })
                                .when(is_owned, |row| {
                                    row.child(
                                        div()
                                            .id("profile-edit-btn")
                                            .cursor_pointer()
                                            .px(px(11.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(pal.service_pill_rgba())
                                            .hover(move |s| s.bg(hover_bg))
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.open_edit_profile_modal(edit_actor.clone(), cx);
                                                }),
                                            )
                                            .child(tg_icon(IconName::Edit, 12.0, pal.text_primary))
                                            .child(tr_str(lang, "编辑资料", "Edit Profile")),
                                    )
                                })
                                .when_some(dm_uid, |row, uid| {
                                    row.child(
                                        div()
                                            .id("profile-dm-btn")
                                            .cursor_pointer()
                                            .px(px(11.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(pal.service_pill_rgba())
                                            .hover(move |s| s.bg(hover_bg))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.emit_action(
                                                        CommunityWorkspaceAction::OpenDmWithActor {
                                                            linked_user_id: uid,
                                                        },
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(tr_str(lang, "Telegram 私聊", "Message in Telegram")),
                                    )
                                })
                                .when(!is_self_active, |row| {
                                    row.child(
                                        div()
                                            .id("profile-follow-btn")
                                            .cursor_pointer()
                                            .px(px(13.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(if following {
                                                pal.service_pill_rgba()
                                            } else {
                                                rgb(pal.accent).into()
                                            })
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if following {
                                                pal.text_primary
                                            } else {
                                                pal.active_button_fg
                                            }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_follow_actor(aid_follow.clone(), !following, cx);
                                                }),
                                            )
                                            .child(if following {
                                                tr_str(lang, "已关注", "Following")
                                            } else {
                                                tr_str(lang, "关注", "Follow")
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("profile-mute-btn")
                                            .cursor_pointer()
                                            .px(px(9.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(if muted {
                                                TdesktopThemePalette::rgba_with_alpha(0xF59E0B, 0.18)
                                            } else {
                                                pal.service_pill_rgba()
                                            })
                                            .text_size(px(11.0))
                                            .text_color(rgb(if muted { 0xF59E0B } else { pal.text_secondary }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_mute_actor(aid_mute.clone(), muted, cx);
                                                }),
                                            )
                                            .child(if muted {
                                                tr_str(lang, "取消静音", "Unmute")
                                            } else {
                                                tr_str(lang, "静音", "Mute")
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("profile-block-btn")
                                            .cursor_pointer()
                                            .px(px(9.0))
                                            .py(px(5.5))
                                            .rounded_full()
                                            .bg(if blocked {
                                                TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.18)
                                            } else {
                                                pal.service_pill_rgba()
                                            })
                                            .text_size(px(11.0))
                                            .text_color(rgb(if blocked { pal.danger } else { pal.text_secondary }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_block_actor(aid_block.clone(), blocked, cx);
                                                }),
                                            )
                                            .child(if blocked {
                                                tr_str(lang, "解除屏蔽", "Unblock")
                                            } else {
                                                tr_str(lang, "屏蔽", "Block")
                                            }),
                                    )
                                }),
                        ),
                )
                // Bio, Location, Website
                .when(!actor.bio.trim().is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(pal.text_primary))
                            .child(actor.bio.clone()),
                    )
                })
                .when(
                    actor.location.as_deref().is_some_and(|s| !s.trim().is_empty())
                        || actor.website.as_deref().is_some_and(|s| !s.trim().is_empty()),
                    |col| {
                        col.child(
                            div()
                                .flex()
                                .items_center()
                                .flex_wrap()
                                .gap(px(12.0))
                                .when_some(
                                    actor.location.clone().filter(|s| !s.trim().is_empty()),
                                    |row, loc| {
                                        row.child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(pal.text_secondary))
                                                .child(format!("📍 {loc}")),
                                        )
                                    },
                                )
                                .when_some(
                                    actor.website.clone().filter(|s| !s.trim().is_empty()),
                                    |row, url| {
                                        let open_url = url.clone();
                                        row.child(
                                            div()
                                                .id("profile-website-link")
                                                .cursor_pointer()
                                                .text_size(px(11.5))
                                                .text_color(rgb(pal.text_link))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.emit_action(
                                                            CommunityWorkspaceAction::OpenExternalUrl(
                                                                open_url.clone(),
                                                            ),
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(format!("🔗 {url}")),
                                        )
                                    },
                                ),
                        )
                    },
                )
                // Dual Reputation Bar: Reddit Karma + X Social Counts
                .child(
                    div()
                        .pt(px(8.0))
                        .border_t_1()
                        .border_color(pal.hairline_border_rgba())
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(10.0))
                        .child(render_stat_pill(
                            tr_str(lang, "帖子声望", "Post Karma"),
                            format_compact_count(actor.post_karma),
                            &pal,
                        ))
                        .child(render_stat_pill(
                            tr_str(lang, "评论声望", "Comment Karma"),
                            format_compact_count(actor.comment_karma),
                            &pal,
                        ))
                        .child(render_stat_pill(
                            tr_str(lang, "受奖声望", "Awardee Karma"),
                            format_compact_count(actor.awardee_karma),
                            &pal,
                        ))
                        .child(div().w(px(1.0)).h(px(18.0)).bg(pal.separator_rgba()))
                        .child(render_stat_pill(
                            tr_str(lang, "粉丝", "Followers"),
                            format_compact_count(actor.followers_count as i64),
                            &pal,
                        ))
                        .child(render_stat_pill(
                            tr_str(lang, "关注", "Following"),
                            format_compact_count(actor.following_count as i64),
                            &pal,
                        ))
                        .child(render_stat_pill(
                            tr_str(lang, "发帖", "Posts"),
                            format_compact_count(actor.posts_count as i64),
                            &pal,
                        )),
                ),
        )
}

fn render_stat_pill(label: &str, value: String, pal: &TdesktopThemePalette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(value),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(pal.text_secondary))
                .child(label.to_string()),
        )
}

fn render_bot_citizenship_card(
    ws: &CommunityWorkspace,
    actor: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let (_, bot_bg, bot_fg) = TdesktopThemePalette::bot_hue_colors("violet", pal.mode.is_dark());
    let trust = if actor.trust_score.is_finite() {
        actor.trust_score.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let trust_pct = (trust * 100.0).round() as u32;
    let is_owned = ws.is_viewer_owned_actor(&actor.actor_id);

    div()
        .p(px(13.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(bot_fg, 0.35))
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon(IconName::BotRobot, 15.0, bot_fg))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(bot_fg))
                                .child(tr_str(lang, "一等公民 Bot 档案卡片", "First-Class Bot Citizenship Card")),
                        ),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(2.5))
                        .rounded_full()
                        .bg(rgb(bot_bg))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(bot_fg))
                        .child(autonomy_label(actor.bot_autonomy_mode, lang)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(8.0))
                .text_size(px(11.5))
                .text_color(rgb(pal.text_secondary))
                .when_some(actor.bot_badge.as_deref().filter(|s| !s.trim().is_empty()), |r, b| {
                    r.child(render_flair_chip(b, Some("#8B5CF6"), &pal))
                })
                .when_some(actor.bot_model.as_deref().filter(|s| !s.trim().is_empty()), |r, m| {
                    r.child(div().child(format!("{}: {m}", tr_str(lang, "模型", "Model"))))
                })
                .when_some(actor.owner_user_id, |r, owner_uid| {
                    r.child(div().child(format!("{}: #{owner_uid}", tr_str(lang, "所属人类主理人", "Owner"))))
                }),
        )
        .when_some(actor.bot_persona.as_deref().filter(|s| !s.trim().is_empty()), |col, persona| {
            col.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(pal.text_primary))
                    .child(persona.to_string()),
            )
        })
        .when(!actor.bot_capabilities.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(5.0))
                    .children(actor.bot_capabilities.iter().map(|cap| {
                        render_flair_chip(cap, Some("#3B82F6"), &pal)
                    })),
            )
        })
        // Trust Score Bar
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{} {trust_pct}%", tr_str(lang, "社区信任分", "Trust Score"))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .max_w(px(200.0))
                                .h(px(6.0))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(px((trust as f32) * 200.0))
                                        .rounded_full()
                                        .bg(rgb(pal.success)),
                                ),
                        ),
                )
                .when(is_owned, |row| {
                    row.child(
                        div()
                            .id("profile-bot-open-studio")
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(rgb(bot_bg))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(bot_fg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.navigate_to(CommunityRoute::BotStudio, cx);
                                }),
                            )
                            .child(tr_str(lang, "在 Bot 工作室管理", "Manage in Bot Studio")),
                    )
                }),
        )
}

fn render_profile_tabs_bar(
    ws: &CommunityWorkspace,
    actor: &CommunityActorDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active = ws.state.profile_tab;
    let is_bot = actor.actor_kind.is_bot_like();
    let mut tabs: Vec<(ActorProfileTab, &'static str, &'static str)> = vec![
        (ActorProfileTab::Posts, "帖子", "Posts"),
        (ActorProfileTab::Replies, "回复", "Replies"),
        (ActorProfileTab::Threads, "串贴", "Threads"),
        (ActorProfileTab::Media, "媒体", "Media"),
        (ActorProfileTab::Likes, "喜欢", "Likes"),
        (ActorProfileTab::Upvoted, "赞过", "Upvoted"),
        (ActorProfileTab::Communities, "社区", "Communities"),
    ];
    if is_bot {
        tabs.push((ActorProfileTab::BotActivity, "Bot 活动", "Bot Activity"));
    }

    div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(5.0))
        .pb(px(4.0))
        .border_b_1()
        .border_color(pal.separator_rgba())
        .children(tabs.into_iter().map(|(tab, zh, en)| {
            let sel = active == tab;
            div()
                .id(SharedString::from(format!("profile-tab-{tab:?}")))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(5.5))
                .rounded_full()
                .bg(if sel { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if sel { pal.active_button_fg } else { pal.text_primary }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_profile_tab(tab, cx);
                    }),
                )
                .child(tr_str(lang, zh, en))
        }))
}

fn collect_viewer_posts_matching(
    ws: &CommunityWorkspace,
    is_viewer_self: bool,
    prof_posts: &[CommunityPostDto],
    pred: impl Fn(&CommunityPostDto) -> bool,
) -> Vec<CommunityPostDto> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in prof_posts {
        if p.status == PostStatus::Published && pred(p) && seen.insert(p.post_id.clone()) {
            out.push(p.clone());
        }
    }
    if is_viewer_self {
        let iter = ws
            .state
            .feed_items
            .iter()
            .map(|i| &i.post)
            .chain(ws.state.saved_items.iter().map(|i| &i.post))
            .chain(ws.state.community_posts.iter().map(|i| &i.post));
        for p in iter {
            if p.status == PostStatus::Published && pred(p) && seen.insert(p.post_id.clone()) {
                out.push(p.clone());
            }
        }
    }
    out
}

fn render_profile_tab_body(
    ws: &CommunityWorkspace,
    prof: &gpui_telegram_proto::community_contracts::ActorProfileResponse,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let tab = ws.state.profile_tab;
    let hover_bg = pal.row_hover_rgba();

    // Online/local parity: strictly filter to Published posts!
    let published_posts: Vec<CommunityPostDto> = prof
        .recent_posts
        .iter()
        .filter(|p| p.status == PostStatus::Published)
        .cloned()
        .collect();

    match tab {
        ActorProfileTab::Posts => {
            if published_posts.is_empty() {
                return div().child(render_empty_state(
                    IconName::ComposePen,
                    tr_str(lang, "暂无公开帖子", "No published posts yet"),
                    tr_str(lang, "该公民尚未公开发布帖子。", "This citizen hasn't published any posts yet."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(published_posts.into_iter().enumerate().map(|(i, post)| {
                    let it = FeedItemDto { post, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                    render_post_card(ws, &it, i, cx)
                }))
        }
        ActorProfileTab::Replies => {
            if prof.recent_comments.is_empty() {
                return div().child(render_empty_state(
                    IconName::Reply,
                    tr_str(lang, "暂无回复记录", "No replies yet"),
                    tr_str(lang, "参与帖子讨论后的评论将展示在此处。", "Replies made across discussions will appear here."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(prof.recent_comments.iter().enumerate().map(|(i, cmt)| {
                    let nav_pid = cmt.post_id.clone();
                    let nav_cid = cmt.comment_id.clone();
                    let linked = linkify_community_slugs(&cmt.body);
                    let msg_id = gpui_telegram_community::fnv1a_64(cmt.comment_id.as_bytes())
                        .wrapping_add(300 + i as u64);
                    div()
                        .id(SharedString::from(format!("profile-reply-card-{}-{i}", cmt.comment_id)))
                        .cursor_pointer()
                        .p(px(11.0))
                        .rounded(px(10.0))
                        .bg(pal.card_surface_rgba())
                        .border_1()
                        .border_color(pal.hairline_border_rgba())
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_post_detail(nav_pid.clone(), cx);
                                this.state.detail_focused_comment_id = Some(nav_cid.clone());
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "▲ {} · ♥ {} · {}",
                                    format_compact_count(cmt.score),
                                    format_compact_count(cmt.likes_count as i64),
                                    format_relative_time(cmt.created_at_ms, lang)
                                ))
                                .child(
                                    div()
                                        .text_color(rgb(pal.accent))
                                        .child(tr_str(lang, "查看原帖 →", "Jump to Post →")),
                                ),
                        )
                        .child(render_rich_message_body(
                            msg_id,
                            &linked,
                            &[],
                            &ws.state.revealed_spoilers,
                            &ws.state.expanded_quotes,
                            ws.state.copied_code_msg,
                            &pal,
                            lang,
                            false,
                            cx,
                            |this, action, cx| this.handle_rich_text_action(action, cx),
                        ))
                }))
        }
        ActorProfileTab::Threads => {
            let threads: Vec<CommunityPostDto> = published_posts
                .into_iter()
                .filter(|p| p.thread_total > 1 || p.thread_root_id.is_some())
                .collect();
            if threads.is_empty() {
                return div().child(render_empty_state(
                    IconName::CommunityThread,
                    tr_str(lang, "暂无串贴", "No multi-post threads yet"),
                    tr_str(lang, "发布 1/N 连续串贴后会集中显示在此标签页。", "Multi-post 1/N threads authored by this citizen appear here."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(threads.into_iter().enumerate().map(|(i, post)| {
                    let it = FeedItemDto { post, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                    render_post_card(ws, &it, 100 + i, cx)
                }))
        }
        ActorProfileTab::Media => {
            let media_posts: Vec<CommunityPostDto> = published_posts
                .into_iter()
                .filter(|p| !p.media.is_empty())
                .collect();
            if media_posts.is_empty() {
                return div().child(render_empty_state(
                    IconName::Camera,
                    tr_str(lang, "暂无媒体帖子", "No media posts yet"),
                    tr_str(lang, "包含图片或视频的帖子会展示在这里。", "Posts with photos or videos will show up here."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(media_posts.into_iter().enumerate().map(|(i, post)| {
                    let it = FeedItemDto { post, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                    render_post_card(ws, &it, 200 + i, cx)
                }))
        }
        ActorProfileTab::Likes => {
            let is_self = prof.actor.actor_id == ws.state.active_actor_id;
            let liked = collect_viewer_posts_matching(ws, is_self, &published_posts, |p| p.viewer_liked);
            if liked.is_empty() {
                return div().child(render_empty_state(
                    IconName::HeartReaction,
                    tr_str(lang, "暂无喜欢记录", "No liked posts yet"),
                    tr_str(lang, "点亮红心的帖子会汇总显示在此处。", "Posts liked with a heart will appear here."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(liked.into_iter().enumerate().map(|(i, post)| {
                    let it = FeedItemDto { post, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                    render_post_card(ws, &it, 300 + i, cx)
                }))
        }
        ActorProfileTab::Upvoted => {
            let is_self = prof.actor.actor_id == ws.state.active_actor_id;
            let upvoted = collect_viewer_posts_matching(ws, is_self, &published_posts, |p| p.viewer_vote == 1);
            if upvoted.is_empty() {
                return div().child(render_empty_state(
                    IconName::UpvoteArrow,
                    tr_str(lang, "暂无顶帖记录", "No upvoted posts yet"),
                    tr_str(lang, "投出赞成票的帖子会展示在此处。", "Posts you upvoted will appear here."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(upvoted.into_iter().enumerate().map(|(i, post)| {
                    let it = FeedItemDto { post, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                    render_post_card(ws, &it, 400 + i, cx)
                }))
        }
        ActorProfileTab::Communities => {
            let mut slugs: Vec<(String, bool)> = prof
                .moderated_communities
                .iter()
                .map(|s| (s.clone(), true))
                .collect();
            if prof.actor.actor_id == ws.state.active_actor_id {
                let seen: HashSet<String> = slugs.iter().map(|(s, _)| s.to_lowercase()).collect();
                for c in &ws.state.communities {
                    if c.is_joined && !seen.contains(&c.slug.to_lowercase()) {
                        slugs.push((c.slug.clone(), false));
                    }
                }
            }
            if slugs.is_empty() {
                return div().child(render_empty_state(
                    IconName::CommunityGlobe,
                    tr_str(lang, "暂无关联社区", "No communities yet"),
                    tr_str(lang, "加入或担任版主的 c/ 社区将显示在此。", "Communities joined or moderated appear here."),
                    &pal,
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .children(slugs.into_iter().map(|(slug, is_mod)| {
                    let nav_slug = slug.clone();
                    div()
                        .id(SharedString::from(format!("profile-comm-row-{slug}")))
                        .cursor_pointer()
                        .p(px(10.0))
                        .rounded(px(9.0))
                        .bg(pal.card_surface_rgba())
                        .border_1()
                        .border_color(pal.hairline_border_rgba())
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_between()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.navigate_to(CommunityRoute::CommunityPage(nav_slug.clone()), cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("c/{slug}")),
                        )
                        .child(render_flair_chip(
                            if is_mod {
                                tr_str(lang, "版主", "Moderator")
                            } else {
                                tr_str(lang, "成员", "Member")
                            },
                            if is_mod { Some("#10B981") } else { Some("#3B82F6") },
                            &pal,
                        ))
                }))
        }
        ActorProfileTab::BotActivity => {
            let a = &prof.actor;
            let is_owned = ws.is_viewer_owned_actor(&a.actor_id);
            div()
                .p(px(13.0))
                .rounded(px(10.0))
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
                        .child(tr_str(lang, "Bot 公民活动与声望总览", "Bot Activity & Reputation Summary")),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "• {} : {}\n• {} : {} {} / {} {}\n• {} : {} Post + {} Comment + {} Awardee",
                            tr_str(lang, "自主模式", "Autonomy Mode"),
                            autonomy_label(a.bot_autonomy_mode, lang),
                            tr_str(lang, "近期内容", "Recent Output"),
                            published_posts.len(),
                            tr_str(lang, "篇帖子", "posts"),
                            prof.recent_comments.len(),
                            tr_str(lang, "条回复", "replies"),
                            tr_str(lang, "总声望构成", "Karma Breakdown"),
                            format_compact_count(a.post_karma),
                            format_compact_count(a.comment_karma),
                            format_compact_count(a.awardee_karma),
                        )),
                )
                .when(is_owned, |col| {
                    col.child(
                        div()
                            .flex()
                            .child(
                                div()
                                    .id("profile-bot-activity-studio")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded_full()
                                    .bg(rgb(pal.accent))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.navigate_to(CommunityRoute::BotStudio, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "前往 Bot 工作室测试发帖", "Summon / Test in Bot Studio")),
                            ),
                    )
                })
        }
    }
}

/// Renders the centered `macos_modal_card` Edit Profile modal for owned Human or Bot citizens.
pub fn render_edit_profile_modal(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let d = &ws.state.edit_profile;
    let err = d.error.clone();
    let submitting = d.submitting;
    let is_bot = d.is_bot;
    let hover_bg = pal.row_hover_rgba();

    let modes: [(BotAutonomyMode, &'static str); 4] = [
        (BotAutonomyMode::Manual, "Manual"),
        (BotAutonomyMode::MentionOnly, "MentionOnly"),
        (BotAutonomyMode::Reactive, "Reactive"),
        (BotAutonomyMode::Autonomous, "Autonomous"),
    ];

    let field_box = |id: &'static str,
                     text: &str,
                     placeholder: &str,
                     target: ActiveCommunityInput,
                     min_h_px: f32,
                     cx: &mut Context<CommunityWorkspace>| {
        let focused = ws.state.active_input == target;
        let preedit = if focused { ws.state.ime_preedit.as_deref() } else { None };
        div()
            .id(id)
            .cursor_text()
            .w_full()
            .min_h(px(min_h_px))
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(8.0))
            .bg(rgb(pal.input_bg))
            .border_1()
            .border_color(if focused {
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
            .child(render_input_display(text, preedit, placeholder, focused, &pal))
    };

    let card = macos_modal_card("community-edit-profile-modal-card", pal)
        .w(px(pal.scaled_px(560.0).min(620.0)))
        .max_w_full()
        .max_h(px(660.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(14.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(if is_bot {
                            tr_str(lang, "编辑 Bot 公民资料", "Edit Bot Citizen Profile")
                        } else {
                            tr_str(lang, "编辑个人资料", "Edit Profile")
                        }),
                )
                .child(
                    div()
                        .id("edit-profile-close")
                        .cursor_pointer()
                        .p(px(5.0))
                        .rounded_full()
                        .hover(move |s| s.bg(hover_bg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_edit_profile_modal(cx);
                            }),
                        )
                        .child(tg_icon(IconName::Close, 14.0, pal.text_secondary)),
                ),
        )
        .child(
            div()
                .id("edit-profile-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "昵称", "Display Name")))
                .child(field_box("edit-prof-name", &d.display_name, tr_str(lang, "输入展示名称…", "Display name…"), ActiveCommunityInput::ProfileEditDisplayName, 34.0, cx))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "个人简介", "Bio")))
                .child(field_box("edit-prof-bio", &d.bio, tr_str(lang, "介绍一下你自己…", "Tell the community about yourself…"), ActiveCommunityInput::ProfileEditBio, 60.0, cx))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "所在地（留空可清除）", "Location (leave blank to clear)")))
                .child(field_box("edit-prof-location", &d.location, "San Francisco / Shanghai", ActiveCommunityInput::ProfileEditLocation, 32.0, cx))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "个人网站（留空可清除）", "Website (leave blank to clear)")))
                .child(field_box("edit-prof-website", &d.website, "https://example.com", ActiveCommunityInput::ProfileEditWebsite, 32.0, cx))
                .when(is_bot, |col| {
                    let preview_shape = d.avatar_shape.clone();
                    let preview_color = d.avatar_color.clone();
                    let aid = d.actor_id.clone();
                    col.child(
                        div()
                            .pt(px(8.0))
                            .border_t_1()
                            .border_color(pal.separator_rgba())
                            .flex()
                            .flex_col()
                            .gap(px(7.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(render_grok_bot_avatar(
                                        GrokBotAvatarProps::new(&aid, &preview_shape, &preview_color, 44.0)
                                            .with_surface_bg(pal.modal_bg)
                                            .with_paused(true),
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tr_str(lang, "Bot 形象与自主参数", "Bot Avatar & Citizenship Settings")),
                                    ),
                            )
                            // 8 Grok shapes
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .children(GROK_AVATAR_SHAPES.into_iter().map(|shape| {
                                        let active = d.avatar_shape == shape;
                                        div()
                                            .id(SharedString::from(format!("edit-prof-shape-{shape}")))
                                            .cursor_pointer()
                                            .px(px(7.5))
                                            .py(px(3.5))
                                            .rounded(px(6.0))
                                            .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                            .text_size(px(10.5))
                                            .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.state.edit_profile.avatar_shape = shape.to_string();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(shape)
                                    })),
                            )
                            // 11 Grok color swatches
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap(px(6.0))
                                    .children(GROK_COLOR_PALETTE.iter().map(|spec| {
                                        let cid = spec.id;
                                        let active = d.avatar_color.eq_ignore_ascii_case(cid);
                                        div()
                                            .id(SharedString::from(format!("edit-prof-color-{cid}")))
                                            .cursor_pointer()
                                            .w(px(22.0))
                                            .h(px(22.0))
                                            .rounded_full()
                                            .bg(rgb(spec.swatch_rgb))
                                            .border_2()
                                            .border_color(if active { rgb(pal.text_primary).into() } else { pal.hairline_border_rgba() })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.state.edit_profile.avatar_color = cid.to_string();
                                                    cx.notify();
                                                }),
                                            )
                                    })),
                            )
                            // Autonomy Mode selector
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .children(modes.into_iter().map(|(m, lbl)| {
                                        let active = d.bot_autonomy_mode == m;
                                        div()
                                            .id(SharedString::from(format!("edit-prof-mode-{lbl}")))
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded_full()
                                            .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                            .text_size(px(11.0))
                                            .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.state.edit_profile.bot_autonomy_mode = m;
                                                    cx.notify();
                                                }),
                                            )
                                            .child(lbl)
                                    })),
                            )
                            .child(field_box("edit-prof-bot-badge", &d.bot_badge, tr_str(lang, "Bot 徽章标签 (如 AI BOT)", "Bot badge (e.g. AI BOT)"), ActiveCommunityInput::ProfileEditBotBadge, 32.0, cx))
                            .child(field_box("edit-prof-bot-model", &d.bot_model, tr_str(lang, "底层模型 (如 hermes-4)", "Model (e.g. hermes-4)"), ActiveCommunityInput::ProfileEditBotModel, 32.0, cx))
                            .child(field_box("edit-prof-bot-persona", &d.bot_persona, tr_str(lang, "Bot 人设与特长描述…", "Bot persona & expertise…"), ActiveCommunityInput::ProfileEditBotPersona, 56.0, cx)),
                    )
                }),
        )
        .child(
            div()
                .px(px(16.0))
                .py(px(11.0))
                .border_t_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(7.0))
                .when_some(err, |col, msg| {
                    col.child(
                        div()
                            .px(px(9.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                            .text_size(px(11.5))
                            .text_color(rgb(pal.danger))
                            .child(msg),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            macos_secondary_pill_btn("edit-profile-cancel", tr_str(lang, "取消", "Cancel"), pal)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_edit_profile_modal(cx);
                                    }),
                                ),
                        )
                        .child(
                            macos_primary_pill_btn(
                                "edit-profile-save",
                                if submitting {
                                    tr_str(lang, "保存中…", "Saving…")
                                } else {
                                    tr_str(lang, "保存资料", "Save Profile")
                                },
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.submit_edit_profile(cx);
                                }),
                            ),
                        ),
                ),
        );

    div()
        .id("community-edit-profile-modal-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .bg(macos_modal_backdrop_rgba(pal))
        .p(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_edit_profile_modal(cx);
            }),
        )
        .child(card)
        .into_any_element()
}
