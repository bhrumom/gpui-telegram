//! Live runtime probe for UI-F16 (Bot Profile inside 打开 助手设置) and UI-F17 (ChannelSettingsPane & ContactProfile).

use gpui_telegram_desktop::components::bloub_avatar::{
    AgentAvatarEditorState, ChannelPictureEditorState,
};
use gpui_telegram_desktop::views::boxes::modal_theme::{
    ConfirmDialogState, compute_confirm_dialog_layout,
};
use gpui_telegram_desktop::views::context_inspector::{
    BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS, BotIdentityAutosaveState, ContextInspector,
    ContextInspectorRenderRoute, validate_bot_profile_name,
};
use gpui_telegram_desktop::views::drawer::bot_computer_inspector::compute_openbot_bot_settings_layout;
use gpui_telegram_desktop::views::drawer::inspector_panels::{
    compute_openbot_channel_settings_layout, compute_openbot_contact_profile_layout,
};
use gpui_telegram_desktop::views::drawer::inspector_tabs::{
    BotPublishedCreationSummary, ChannelBrowserAccess, OpenBotConfirmGuard, OpenBotSettingsSubpage,
};
use gpui_telegram_desktop::views::openbot::channel_editor::{
    ChannelEditorAgentOption, ChannelEditorState, normalize_channel_icon_and_hue,
};
use openbot_contracts::channels::ChannelIcon;
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario};

fn mk_creation(id: &str, kind_code: &'static str) -> BotPublishedCreationSummary {
    BotPublishedCreationSummary {
        creation_id: id.into(), title: format!("Sample {id}"),
        kind_label: kind_code.into(), status_label: "Published".into(), kind_code,
    }
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();

    // UI-F16 Bot Settings Profile inside unified SidePanel
    let mut insp = ContextInspector::new();
    insp.sync_inspector_for_unified_side_panel(true);
    insp.profile.is_bot = true;
    insp.profile.name = "Research Assistant".into();
    insp.profile.username = "research_bot".into();
    insp.profile.bio = "Deep synthesis & workflow automation assistant.".into();
    insp.profile.status = "online".into();
    insp.bot_access_level = "workspace".into();
    insp.avatar_editor_open = true;
    insp.published_creations = vec![
        mk_creation("b1", "book"), mk_creation("b2", "book"),
        mk_creation("m1", "music"), mk_creation("g1", "game"),
    ];
    let _copied = insp.trigger_copy_bot_handle();
    let _flushed = {
        insp.bot_autosave = BotIdentityAutosaveState::new("bot-main", "Research Assistant", &insp.profile.bio);
        insp.bot_autosave.edit_name("Research Assistant", 100);
        insp.bot_autosave.flush_if_due(100 + BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS)
    };
    let bot_layout_root = compute_openbot_bot_settings_layout(&insp, &dark);
    insp.open_bot_subpage(OpenBotSettingsSubpage::Memories);
    let bot_layout_mem = compute_openbot_bot_settings_layout(&insp, &dark);
    insp.openbot_subpage = None;
    let back_ok = insp.resolve_context_inspector_render_route() == ContextInspectorRenderRoute::OpenBotBotSettingsRoot;
    let remote_label = if insp.computers.is_empty() {
        "No computer connected"
    } else {
        "Connected"
    };

    root.insert("agent_settings_profile".into(), json!({
        "mounted": bot_layout_root.mounted,
        "host": bot_layout_root.host,
        "avatar_px": bot_layout_root.hero_avatar_px as i32,
        "avatar_popover_open": bot_layout_root.avatar_popover_open,
        "avatar_popover_width_px": bot_layout_root.avatar_editor_width_px as i32,
        "avatar_shapes_count": bot_layout_root.avatar_shapes_count,
        "avatar_eyes_count": bot_layout_root.avatar_eyes_count,
        "avatar_colors_count": bot_layout_root.avatar_editor_total_cells,
        "online": bot_layout_root.online,
        "handle": bot_layout_root.display_handle,
        "handle_copied": bot_layout_root.handle_copied,
        "autosave_debounce_ms": bot_layout_root.autosave_debounce_ms,
        "name": bot_layout_root.name,
        "bio": bot_layout_root.bio,
        "inset_card_style": {
            "bg_hex": bot_layout_root.inset_card_bg_hex,
            "radius_px": bot_layout_root.inset_card_radius_px as i32,
            "row_height_px": bot_layout_root.inset_row_height_px as i32,
        },
        "profile_fields": bot_layout_root.profile_fields,
        "creations_counts": {
            "books": bot_layout_root.creations_counts.books,
            "music": bot_layout_root.creations_counts.music,
            "games": bot_layout_root.creations_counts.games,
        },
        "groups_present": bot_layout_root.groups_present,
        "active_subpage": bot_layout_mem.active_subpage.unwrap_or(""),
        "subpage_back_restores_root": back_ok,
        "permission_level": bot_layout_root.permission_level,
        "destructive_confirm_dialogs": bot_layout_root.destructive_confirm_dialogs,
        "context_inspector_live_reachable": bot_layout_root.context_inspector_live_reachable,
        "remote_computer_label": remote_label,
    }));

    let mut sw_save = BotIdentityAutosaveState::new("bot-a", "Alpha", "Bio A");
    sw_save.edit_name("Alpha-v2", 100);
    let _ = sw_save.flush_before_agent_switch("bot-b", "Beta", "Bio B");
    insert_scenario(sc, "f16_dirty_flush_switch", "agent_settings_profile", json!({
        "dirty_flushed_to_agent_id": sw_save.last_flushed_agent_id,
        "flushed_name": sw_save.last_flushed_name,
        "other_agent_name": sw_save.committed_name,
    }));

    let mut insp_widen = ContextInspector::new();
    insp_widen.sync_inspector_for_unified_side_panel(true);
    insp_widen.profile.is_bot = true;
    insp_widen.bot_access_level = "workspace".into();
    let widen_requires_confirm = insp_widen.request_bot_access_level_change("full");
    let perm_dlg = ConfirmDialogState::open_dialog(widen_requires_confirm);
    let perm_dlg_l = compute_confirm_dialog_layout(&perm_dlg, &dark);
    insert_scenario(sc, "f16_permission_widen_enter", "agent_settings_profile", json!({
        "permission_unchanged": insp_widen.bot_access_level == "workspace" && widen_requires_confirm,
    }));
    insert_scenario(sc, "f16_permission_widen_enter", "settings_and_overlays", json!({
        "confirm_dialog": {"open": perm_dlg_l.open, "initial_focus": perm_dlg_l.initial_focus},
    }));

    let mut insp_narrow = ContextInspector::new();
    insp_narrow.sync_inspector_for_unified_side_panel(true);
    insp_narrow.profile.is_bot = true;
    insp_narrow.bot_access_level = "full".into();
    let narrow_requires_confirm = insp_narrow.request_bot_access_level_change("workspace");
    let l_narrow = compute_openbot_bot_settings_layout(&insp_narrow, &dark);
    insert_scenario(sc, "f16_permission_narrow", "agent_settings_profile", json!({
        "permission_level": l_narrow.permission_level,
        "narrowing_confirm_required": narrow_requires_confirm,
    }));

    let mut insp_busy = ContextInspector::new();
    insp_busy.sync_inspector_for_unified_side_panel(true);
    insp_busy.profile.is_bot = true;
    insp_busy.bot_is_busy = true;
    let new_chat_busy = insp_busy.try_start_new_chat_from_settings();
    let del_busy_allowed = insp_busy.try_delete_bot_history_from_settings();
    let l_busy = compute_openbot_bot_settings_layout(&insp_busy, &dark);
    insert_scenario(sc, "f16_busy_guards", "agent_settings_profile", json!({
        "new_chat_busy_toast": new_chat_busy.new_chat_busy_toast,
        "chat_reset": new_chat_busy.chat_reset,
        "delete_history_disabled": l_busy.delete_history_disabled && !del_busy_allowed,
    }));
    insp_busy.bot_is_busy = false;
    let del_idle_allowed = insp_busy.try_delete_bot_history_from_settings();
    let l_idle = compute_openbot_bot_settings_layout(&insp_busy, &dark);
    insert_scenario(sc, "f16_idle_guards_unlocked", "agent_settings_profile", json!({
        "delete_history_disabled": l_idle.delete_history_disabled || !del_idle_allowed,
    }));

    let mut err_save = BotIdentityAutosaveState::new("bot-a", "Research Assistant", "Bio");
    err_save.edit_name("Transient Unsaved", 100);
    err_save.on_save_failed("RPC save failed");
    insert_scenario(sc, "f16_autosave_error", "agent_settings_profile", json!({
        "save_error_visible": err_save.save_error.is_some(),
        "name": err_save.draft_name,
        "rolled_back_name": err_save.committed_name,
    }));

    let mut avatar_ed = AgentAvatarEditorState::new("bot-main", Some("bot-main".into()), None, None, None);
    let avatar_before = (
        avatar_ed.seed.clone(),
        avatar_ed.hue,
        avatar_ed.color.clone(),
        avatar_ed.avatar_url.clone(),
    );
    let corrupt_upload_rejected = avatar_ed.upload_image_bytes(b"NOT_AN_IMAGE", None).is_err();
    let avatar_after = (
        avatar_ed.seed.clone(),
        avatar_ed.hue,
        avatar_ed.color.clone(),
        avatar_ed.avatar_url.clone(),
    );
    let avatar_unchanged = avatar_before == avatar_after && avatar_ed.avatar_url.is_none();
    let long_bot_name = "x".repeat(81);
    let name81_rejected = validate_bot_profile_name(&long_bot_name).is_err();
    let mut insp_name81 = ContextInspector::new();
    insp_name81.sync_inspector_for_unified_side_panel(true);
    insp_name81.profile.is_bot = true;
    insp_name81.profile.name = insp.profile.name.clone();
    insp_name81.bot_draft_name = long_bot_name.clone();
    let committed_81 = insp_name81.commit_bot_identity_draft();
    insp_name81.bot_autosave = BotIdentityAutosaveState::new("bot-main", &insp.profile.name, &insp.profile.bio);
    insp_name81.bot_autosave.edit_name(long_bot_name, 100);
    let _ = insp_name81.bot_autosave.flush_now();
    insert_scenario(sc, "f16_corrupt_avatar_and_name81", "agent_settings_profile", json!({
        "avatar_upload_error_toast": corrupt_upload_rejected,
        "avatar_unchanged": avatar_unchanged,
        "name_81_rejected": name81_rejected && !committed_81,
        "name": insp_name81.bot_autosave.draft_name,
    }));

    let mut zero_insp = ContextInspector::new();
    zero_insp.sync_inspector_for_unified_side_panel(true);
    zero_insp.profile.is_bot = true;
    zero_insp.profile.name = "New Bot".into();
    let zero_l = compute_openbot_bot_settings_layout(&zero_insp, &dark);
    insert_scenario(sc, "f16_zero_creations_bot", "agent_settings_profile", json!({
        "creations_counts": {"books": zero_l.creations_counts.books, "music": zero_l.creations_counts.music, "games": zero_l.creations_counts.games},
        "remote_computer_label": if zero_insp.computers.is_empty() { "No computer connected" } else { "Connected" },
        "context_inspector_live_reachable": zero_l.context_inspector_live_reachable,
    }));

    // UI-F17 ChannelSettingsPane & ContactProfile
    let mut ch_insp = ContextInspector::new();
    ch_insp.sync_inspector_for_unified_side_panel(true);
    ch_insp.profile.is_bot = false;
    ch_insp.members.is_group_or_channel = true;
    ch_insp.profile.name = "Launch War Room".into();
    ch_insp.channel_context_isolated = true;
    ch_insp.channel_browser_access = ChannelBrowserAccess::Everyone;
    let ch_l = compute_openbot_channel_settings_layout(&ch_insp);
    let mut pic_ed = ChannelPictureEditorState::new(Some(ChannelIcon { name: "code".into(), hue: 215 }), None);
    let icon_names: Vec<&str> = ChannelPictureEditorState::icon_names().to_vec();
    let chosen_before = pic_ed.chosen_icon().map(|k| k.name.as_str());
    let chosen_hue = pic_ed.chosen_icon().map(|k| k.hue.degrees());
    let can_reset_before = pic_ed.can_reset();
    pic_ed.reset();
    let reset_ok = !pic_ed.can_reset();

    let mut roster_ed = ChannelEditorState::new_create();
    let _ = roster_ed.add_member("bot-a");
    let _ = roster_ed.add_member("bot-b");
    let add_c_ok = roster_ed.add_member("bot-c").is_ok();
    let drag_ok = roster_ed.attempt_member_drag_reorder(0, 1).is_ok();

    root.insert("channel_settings".into(), json!({
        "mounted": ch_l.mounted,
        "name": ch_l.name,
        "picture_trigger_px": ch_l.avatar_px as i32,
        "picture_popover_width_px": ch_l.picture_popover_width_px as i32,
        "icon_catalog_count": ch_l.icon_catalog_count,
        "canonical_icons_present": icon_names,
        "color_palette_count": ch_l.icon_hues_count,
        "resolved_icon": chosen_before,
        "resolved_hue": chosen_hue,
        "reset_restores_cluster": reset_ok,
        "reset_visible": can_reset_before,
        "browser_access": ch_l.browser_policy,
        "isolated": ch_l.context_isolated,
        "subpages_present": ch_l.subpages_present,
        "subpage_counts": {"memories": ch_l.memories_count, "routines": ch_l.routines_count, "folders": ch_l.folders_count},
        "context_actions_visible": ch_l.context_actions_visible,
        "context_actions": ch_l.context_actions,
        "members": roster_ed.members,
        "lead_agent_id": roster_ed.lead_agent_id,
        "member_drag_reorder_supported": drag_ok,
        "member_add_accepted": add_c_ok,
        "read_only": ch_l.read_only,
        "compact_disabled": ch_l.compact_disabled,
    }));
    insert_scenario(sc, "f17_isolation_on", "conversation_header", json!({"isolation_lock_visible": ch_l.context_isolated}));
    let ctx_cd = ConfirmDialogState::open_dialog(true);
    let ctx_cd_l = compute_confirm_dialog_layout(&ctx_cd, &dark);
    insert_scenario(sc, "f17_context_actions", "settings_and_overlays", json!({"confirm_dialog": {"open": ctx_cd_l.open}}));

    let mut ch_val_insp = ContextInspector::new();
    ch_val_insp.sync_inspector_for_unified_side_panel(true);
    ch_val_insp.members.is_group_or_channel = true;
    ch_val_insp.profile.name = "Launch War Room".into();
    ch_val_insp.channel_draft_name = "   ".into();
    let blank_committed = ch_val_insp.commit_channel_identity_draft();
    assert!(
        !blank_committed
            && ch_val_insp.channel_draft_name == "Launch War Room"
            && ch_val_insp.channel_validation_error.is_some()
            && ch_val_insp.pending_openbot_action.is_none(),
        "blank channel name must revert to prior name on blur without emitting UpdateChatInfo"
    );
    ch_val_insp.channel_draft_instructions = "i".repeat(2001);
    let long_insp_committed = ch_val_insp.commit_channel_identity_draft();
    let long_insp_blocked = !long_insp_committed
        && ch_val_insp.channel_validation_error.is_some()
        && ch_val_insp.pending_openbot_action.is_none();

    let mut ed_long = ChannelEditorState::new_create();
    ed_long.name = ch_val_insp.channel_draft_name.clone();
    ed_long.instructions = "i".repeat(2001);
    let _ = ed_long.add_member("bot-a");
    let long_editor_invalid = ed_long.validate_draft().is_err();
    insert_scenario(sc, "f17_blank_name_and_long_instructions", "channel_settings", json!({
        "name": ch_val_insp.channel_draft_name,
        "instructions_invalid": long_insp_blocked && long_editor_invalid,
        "save_blocked": !blank_committed && long_insp_blocked && long_editor_invalid,
    }));

    let mut ch_sw = BotIdentityAutosaveState::new("ch-a", "Squad A", "Inst A");
    ch_sw.edit_name("Squad A2", 100);
    ch_sw.edit_name("Squad A3", 150);
    let flushed_prev = ch_sw.flush_before_agent_switch("ch-b", "Squad B", "Inst B");
    insert_scenario(sc, "f17_rapid_edits_switch", "channel_settings", json!({
        "serialized_save_order": flushed_prev && ch_sw.last_flushed_name.as_deref() == Some("Squad A3"),
        "dirty_isolated_on_switch": !ch_sw.dirty && ch_sw.committed_name == "Squad B",
    }));

    let mut ed_lead = ChannelEditorState::new_create();
    let _ = ed_lead.add_member("lead-removed");
    let _ = ed_lead.add_member("bot-b");
    let _ = ed_lead.remove_member("lead-removed");
    let removed_last = ed_lead.remove_member("bot-b");
    let last_blocked = !removed_last && ed_lead.members.len() == 1;
    insert_scenario(sc, "f17_remove_lead_and_last", "channel_settings", json!({
        "lead_agent_id": ed_lead.lead_agent_id, "members": ed_lead.members, "last_member_removal_blocked": last_blocked,
    }));

    let mut ch_arch = ContextInspector::new();
    ch_arch.sync_inspector_for_unified_side_panel(true);
    ch_arch.members.is_group_or_channel = true;
    ch_arch.channel_archived = true;
    let ch_arch_l = compute_openbot_channel_settings_layout(&ch_arch);
    insert_scenario(sc, "f17_archived_settings", "channel_settings", json!({
        "read_only": ch_arch_l.read_only, "context_actions_visible": ch_arch_l.context_actions_visible,
    }));
    let mut ch_nocomp = ContextInspector::new();
    ch_nocomp.sync_inspector_for_unified_side_panel(true);
    ch_nocomp.members.is_group_or_channel = true;
    ch_nocomp.channel_provider_supports_compact = false;
    let ch_nocomp_l = compute_openbot_channel_settings_layout(&ch_nocomp);
    insert_scenario(sc, "f17_compact_unsupported", "channel_settings", json!({
        "compact_disabled": ch_nocomp_l.compact_disabled,
        "context_actions_visible": ch_nocomp_l.context_actions_visible,
    }));

    let mut ed_100 = ChannelEditorState::new_create();
    for i in 0..100 {
        let _ = ed_100.add_member(format!("m-{i}"));
    }
    let add_101_ok = ed_100.add_member("m-100").is_ok();
    let candidates = vec![
        ChannelEditorAgentOption {
            id: "cand-eclair".into(),
            name: "Éclair".into(),
            title: "Design Critic".into(),
            description: "Reviews UI layouts".into(),
            avatar_seed: Some("cand-eclair".into()),
            avatar_hue: None,
            avatar_color: None,
            avatar_url: None,
        },
        ChannelEditorAgentOption {
            id: "cand-researcher".into(),
            name: "Researcher".into(),
            title: "Deep Research".into(),
            description: "Synthesizes papers".into(),
            avatar_seed: Some("cand-researcher".into()),
            avatar_hue: None,
            avatar_color: None,
            avatar_url: None,
        },
    ];
    ed_100.member_search_query = "eclair".into();
    assert_eq!(ed_100.filtered_agents(&candidates).len(), 1);
    ed_100.member_search_query = "zzz-none".into();
    let zero_match = ed_100.filtered_agents(&candidates).is_empty();
    insert_scenario(sc, "f17_member_cap_100", "channel_settings", json!({
        "members": ed_100.members, "member_add_accepted": add_101_ok, "zero_match_hint_visible": zero_match,
    }));

    let (inv_ico, norm_hue) = normalize_channel_icon_and_hue(Some("invalid_icon"), Some(999));
    let mut pic_empty = ChannelPictureEditorState::new(None, None);
    let bad_upload = pic_empty.upload_image_bytes(b"BAD", None).is_err();
    insert_scenario(sc, "f17_invalid_picture_icon_hue", "channel_settings", json!({
        "resolved_icon": inv_ico.map(|n| n.as_str()), "resolved_hue": norm_hue.degrees(),
        "reset_visible": pic_empty.can_reset(), "upload_error_toast": bad_upload,
    }));

    // Human Contact Profile inside unified SidePanel
    let mut hum_insp = ContextInspector::new();
    hum_insp.sync_inspector_for_unified_side_panel(true);
    hum_insp.profile.is_bot = false;
    hum_insp.members.is_group_or_channel = false;
    hum_insp.profile.name = "Alice Chen".into();
    hum_insp.profile.username = "@alice".into();
    hum_insp.profile.status = "online".into();
    hum_insp.pending_confirm_guard = Some(OpenBotConfirmGuard::ContactBlock);
    let cp_l = compute_openbot_contact_profile_layout(&hum_insp, &dark);
    root.insert("contact_profile".into(), json!({
        "mounted": cp_l.mounted,
        "host": cp_l.host,
        "handle": cp_l.handle,
        "online_dot_hex": cp_l.online_dot_hex,
        "destructive_confirm_gated": cp_l.block_confirm_open && cp_l.destructive_confirm_gated,
    }));
}
