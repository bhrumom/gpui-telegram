//! Unit & regression tests for Grok Bot visual fusion, 9-hue bot tinting, live `@mention`
//! autocomplete, multi-agent `RoomAgentLiveState` lease expiry, sidebar preview priorities,
//! Inspector `AddMemberCandidate` deduplication/filtering, and `ToolPermissionDock` stepping.

use gpui_telegram_client::net::http_client::ChatMemberItem;
use gpui_telegram_desktop::components::tool_permission_dock::{
    PendingToolApproval, ToolApprovalDecision, ToolApprovalPolicyTier, ToolPermissionDock,
};
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::app_shell::shell_stage_actions::build_mention_candidates;
use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
use gpui_telegram_desktop::views::chat_sidebar::{
    BotGrokAvatarVisual, DialogDeliveryStatus, DialogItemModel,
};
use gpui_telegram_desktop::views::composer::field_autocomplete::{
    AutocompleteChooseMethod, AutocompleteType, FieldAutocomplete, MentionRow,
};
use gpui_telegram_desktop::views::context_inspector::{
    build_add_member_candidates, filter_add_member_candidates, ContextInspector,
};
use gpui_telegram_desktop::views::conversation_stage::{
    ConversationStage, RoomAgentLiveState, DEFAULT_ROOM_AGENT_STALE_AFTER_MS,
};
use gpui_telegram_desktop::views::sidebar::dialogs_row_view::{
    resolve_dialog_preview_priority, DialogPreviewPriority, SidebarDialogMeta,
};
use gpui_telegram_proto::contracts::auth::ContactDto;
use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;

fn make_contact(user_id: u64, first: &str, username: Option<&str>, is_bot: bool) -> ContactDto {
    ContactDto {
        user_id,
        first_name: first.to_string(),
        last_name: None,
        username: username.map(str::to_string),
        phone: None,
        is_bot,
        is_mutual: true,
        chat_id: None,
    }
}

fn make_fabushi_bot(
    bot_user_id: i64,
    agent_id: &str,
    username: &str,
    display_name: &str,
    avatar_color: &str,
) -> FabushiBotSummaryDto {
    FabushiBotSummaryDto {
        bot_user_id,
        owner_user_id: 1,
        agent_id: agent_id.to_string(),
        username: username.to_string(),
        display_name: display_name.to_string(),
        avatar_color: avatar_color.to_string(),
        ..Default::default()
    }
}

fn make_member(user_id: u64, role: &str) -> ChatMemberItem {
    ChatMemberItem {
        user_id,
        role: role.to_string(),
        admin_title: None,
        admin_rights: 0,
        permissions_bitmask: 0,
        joined_at_ms: 0,
    }
}

#[test]
fn test_grok_4_step_elevation_surface_ladder_dark_and_light() {
    let dark = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0, 14, 18);
    let light = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0, 14, 18);

    // Dark 4-step ladder: Sidebar #0e0e0e < Chrome #121212 < Pane #161616 < Elevated #1f1f1f < Hover #2c2c2c
    assert_eq!(dark.dialogs_bg, 0x0E0E0E);
    assert_eq!(dark.window_bg, 0x121212);
    assert_eq!(dark.topbar_bg, 0x121212);
    assert_eq!(dark.stage_bg, 0x161616);
    assert_eq!(dark.card_bg, 0x1F1F1F);
    assert_eq!(dark.menu_bg, 0x1F1F1F);
    assert_eq!(dark.menu_hover_bg, 0x2C2C2C);
    assert_eq!(dark.text_primary, 0xF5F5F5);
    assert!(
        dark.popover_shadows().is_empty(),
        "Grok Bot dark mode eliminates drop shadows in favor of elevation & specular highlights"
    );
    assert!((dark.specular_top_highlight_rgba().a - 0.12).abs() < 0.01);

    // Light 4-step ladder: Sidebar #f5f5f5 < Chrome #fcfcfc < Pane/Elevated #ffffff, hover #f1f1f1
    assert_eq!(light.dialogs_bg, 0xF5F5F5);
    assert_eq!(light.window_bg, 0xFCFCFC);
    assert_eq!(light.topbar_bg, 0xFCFCFC);
    assert_eq!(light.stage_bg, 0xFFFFFF);
    assert_eq!(light.card_bg, 0xFFFFFF);
    assert_eq!(light.menu_hover_bg, 0xF1F1F1);
    assert_eq!(light.text_primary, 0x1C1C1C);
    assert_eq!(light.popover_shadows().len(), 2);
}

#[test]
fn test_nine_hue_bot_color_palette_dark_and_light() {
    let hues = [
        "blue", "red", "green", "yellow", "orange", "cyan", "purple", "magenta", "brown",
    ];
    let mut seen_dark_bubbles = std::collections::HashSet::new();
    let mut seen_light_bubbles = std::collections::HashSet::new();

    for hue in hues {
        let dark = TdesktopThemePalette::bot_hue_colors(hue, true);
        let light = TdesktopThemePalette::bot_hue_colors(hue, false);
        assert_ne!(
            dark, light,
            "Hue '{hue}' should adapt colors between dark and light themes"
        );
        assert_ne!(dark.0, 0);
        assert_ne!(dark.1, 0);
        assert_ne!(dark.2, 0);
        assert_ne!(light.0, 0);
        assert_ne!(light.1, 0);
        assert_ne!(light.2, 0);
        assert!(seen_dark_bubbles.insert(dark.0));
        assert!(seen_light_bubbles.insert(light.0));
    }

    // Spot-check exact Grok Bot tokens for blue & purple
    assert_eq!(
        TdesktopThemePalette::bot_hue_colors("blue", true),
        (0x116DD2, 0x162639, 0x4D9BFF)
    );
    assert_eq!(
        TdesktopThemePalette::bot_hue_colors("blue", false),
        (0x126FD4, 0xE7F2FF, 0x0B6DD3)
    );
    assert_eq!(
        TdesktopThemePalette::bot_hue_colors("purple", true),
        (0x7845D7, 0x271F39, 0xA582FF)
    );
    assert_eq!(
        TdesktopThemePalette::bot_hue_colors("purple", false),
        (0x7442D1, 0xF4EEFE, 0x7547D0)
    );
    // Neutral names are not tinted chromatic hues
    assert!(!TdesktopThemePalette::is_tinted_bot_hue("black"));
    assert!(!TdesktopThemePalette::is_tinted_bot_hue("gray"));
    assert!(TdesktopThemePalette::is_tinted_bot_hue("purple"));
}

#[test]
fn test_mention_candidate_population_and_filtering_for_groups_and_dms() {
    let bots = vec![
        make_fabushi_bot(777_001, "coder", "coder_bot", "Coder Bot", "purple"),
        make_fabushi_bot(777_002, "research", "research_bot", "Research Agent", "cyan"),
    ];
    let contacts = vec![
        make_contact(101, "Alice Chen", Some("alice"), false),
        make_contact(102, "Bob Wu", Some("bob_wu"), false),
    ];
    let group_members = vec![
        make_member(101, "owner"),
        make_member(777_001, "member"),
        make_member(102, "admin"),
    ];

    // Group: @everyone first + members enriched with bot metadata & roles
    let group_rows = build_mention_candidates(true, &group_members, &contacts, &bots, None);
    assert_eq!(group_rows[0].0.username, "everyone");
    assert_eq!(group_rows[0].1.as_deref(), Some("所有人"));

    let bot_entry = group_rows
        .iter()
        .find(|(r, _)| r.user_id == 777_001)
        .expect("Coder Bot must be present in group mentions");
    assert!(bot_entry.0.is_bot);
    assert_eq!(bot_entry.0.username, "coder_bot");
    assert_eq!(bot_entry.1.as_deref(), Some("AI Bot"));

    let owner_entry = group_rows
        .iter()
        .find(|(r, _)| r.user_id == 101)
        .expect("Alice must be present in group mentions");
    assert!(!owner_entry.0.is_bot);
    assert_eq!(owner_entry.1.as_deref(), Some("群主"));

    // Wire into FieldAutocomplete & test live filtering
    let mut ac = FieldAutocomplete::new();
    ac.set_mention_candidates_with_roles(group_rows);
    ac.show_filtered("@cod");
    assert!(ac.is_open);
    assert_eq!(ac.autocomplete_type, AutocompleteType::Mentions);
    assert_eq!(ac.mention_rows.len(), 1);
    assert_eq!(ac.mention_rows[0].username, "coder_bot");
    assert_eq!(ac.role_hint_for(&ac.mention_rows[0]), "AI Bot");
    assert_eq!(
        ac.choose_selected(AutocompleteChooseMethod::ByEnter),
        Some("@coder_bot".to_string())
    );

    // Filter for @ev matches @everyone
    ac.show_filtered("@ev");
    assert_eq!(ac.mention_rows.len(), 1);
    assert_eq!(ac.mention_rows[0].username, "everyone");

    // DM: active peer + available Fabushi bots without duplication
    let dm_rows = build_mention_candidates(
        false,
        &[],
        &contacts,
        &bots,
        Some((101, "alice".to_string(), "Alice Chen".to_string(), false)),
    );
    assert_eq!(dm_rows.len(), 3);
    assert_eq!(dm_rows[0].0.username, "alice");
    assert!(dm_rows.iter().any(|(r, _)| r.username == "coder_bot" && r.is_bot));
    assert!(dm_rows.iter().any(|(r, _)| r.username == "research_bot" && r.is_bot));

    // Composer override contract methods
    let mut composer = CanonicalComposer::new();
    composer.set_compose_placeholder_override(Some("发消息给 Alice 和 Coder Bot".to_string()));
    composer.set_compose_send_disabled(true);
    assert_eq!(
        composer.compose_placeholder_override.as_deref(),
        Some("发消息给 Alice 和 Coder Bot")
    );
    assert!(composer.compose_send_disabled);
    composer.set_mention_candidates(vec![MentionRow::new(777_001, "coder_bot", "Coder Bot", true)]);
    assert_eq!(composer.autocomplete.all_mention_candidates.len(), 1);
}

#[test]
fn test_room_agent_live_state_floor_handoff_and_lease_expiration() {
    let mut state = RoomAgentLiveState::default();
    assert_eq!(state.stale_after_ms, DEFAULT_ROOM_AGENT_STALE_AFTER_MS);
    assert!(state.is_expired(1_000));
    assert!(!state.is_active(1_000));

    // Floor acquired at t = 10_000ms
    state.apply_floor_changed(Some("Coder Bot".to_string()), 2, 1, 10_000);
    assert!(!state.is_expired(20_000));
    assert!(state.is_active(20_000));
    assert_eq!(state.current_speaker.as_deref(), Some("Coder Bot"));
    assert_eq!(state.queued_count, 2);

    // Handoff at t = 15_000ms refreshes timestamp
    state.apply_handoff("Coder Bot".to_string(), "Reviewer Bot".to_string(), 2, 5, 15_000);
    assert_eq!(
        state.format_handoff_trail("zh-CN").as_deref(),
        Some("Coder Bot → Reviewer Bot · 第 2/5 轮")
    );
    assert_eq!(
        state.format_handoff_trail("en").as_deref(),
        Some("Coder Bot → Reviewer Bot · Turn 2/5")
    );

    // Workspace update refreshes timestamp
    state.apply_workspace_updated(4, "plan.md".to_string(), 20_000);
    assert_eq!(state.workspace_revision, 4);
    assert_eq!(state.last_workspace_key.as_deref(), Some("plan.md"));

    // Still active at exact boundary (20_000 + 25_000 = 45_000)
    assert!(!state.is_expired(45_000));
    assert!(state.is_active(45_000));

    // Expired at 45_001ms -> crashed turns never leave stuck indicators
    assert!(state.is_expired(45_001));
    assert!(!state.is_active(45_001));

    // Integration through ConversationStage
    let mut stage = ConversationStage::new();
    stage.set_chat(42, "Team Room", "3 members", "TR", 0x116DD2);
    stage.on_agent_floor_changed(42, Some("Research Bot".to_string()), 1, 1, 100_000);
    assert!(stage.active_room_agent_state(110_000).is_some());
    assert!(stage.active_room_agent_state(126_000).is_none());
}

#[test]
fn test_add_member_candidate_deduplication_and_filtering() {
    let bots = vec![
        make_fabushi_bot(777_001, "coder", "coder_bot", "Coder Bot", "purple"),
        make_fabushi_bot(777_002, "designer", "design_bot", "Design Bot", "magenta"),
    ];
    let contacts = vec![
        // Duplicate of Fabushi bot 777_001 in contacts should deduplicate cleanly
        make_contact(777_001, "Coder Duplicate", Some("coder_bot"), true),
        // Already in group -> must be excluded
        make_contact(201, "Alice Existing", Some("alice"), false),
        // Eligible human contact
        make_contact(202, "Charlie Lin", Some("charlie"), false),
        // Invalid user_id 0 -> must be skipped
        make_contact(0, "Invalid", None, false),
    ];

    let candidates = build_add_member_candidates(&[201], &contacts, &bots);
    assert_eq!(candidates.len(), 3);
    assert!(candidates.iter().all(|c| c.user_id != 201 && c.user_id != 0));
    // Fabushi bot metadata wins over contact duplicate
    let coder = candidates
        .iter()
        .find(|c| c.user_id == 777_001)
        .expect("Coder Bot present");
    assert_eq!(coder.display_name, "Coder Bot");
    assert!(coder.is_bot);
    assert_eq!(coder.bot_hue.as_deref(), Some("purple"));

    // Filtering
    assert_eq!(filter_add_member_candidates(&candidates, "").len(), 3);
    assert_eq!(filter_add_member_candidates(&candidates, "design").len(), 1);
    assert_eq!(filter_add_member_candidates(&candidates, "@charlie").len(), 1);
    assert_eq!(filter_add_member_candidates(&candidates, "bot").len(), 2);

    // Inspector state transitions
    let mut insp = ContextInspector::new();
    insp.open_add_member_picker(candidates);
    assert!(insp.add_member_picker_open);
    insp.request_add_member_to_active_group(777_002);
    assert_eq!(insp.take_pending_add_member_user_id(), Some(777_002));
    insp.remove_add_member_candidate(777_002);
    assert_eq!(insp.add_member_candidates.len(), 2);
}

#[test]
fn test_sidebar_preview_priority_order_and_tool_permission_dock_stepping() {
    let base_item = DialogItemModel {
        id: 55,
        peer_name: "Ops Bot".to_string(),
        avatar_initials: "OB".to_string(),
        avatar_color: 0x116DD2,
        last_message: "Last snippet".to_string(),
        timestamp: "12:00".to_string(),
        unread_count: 1,
        is_muted: false,
        is_pinned: false,
        is_online: true,
        typing_indicator: Some("正在执行命令…".to_string()),
        delivery_status: DialogDeliveryStatus::Read,
        folder: "Bots".to_string(),
    };
    let mut meta = SidebarDialogMeta {
        draft_text: Some("unsent text".to_string()),
        grok_avatar: Some(BotGrokAvatarVisual {
            agent_id: "ops".to_string(),
            shape: "shield".to_string(),
            color: "orange".to_string(),
            execution_state: "blocked".to_string(),
        }),
        ..Default::default()
    };

    // Priority 1: Blocked / pending approval beats Draft & LiveActivity
    assert!(matches!(
        resolve_dialog_preview_priority(&base_item, &meta, "zh-CN"),
        DialogPreviewPriority::BlockedApproval(_)
    ));

    // Priority 2: Draft beats LiveActivity
    meta.grok_avatar.as_mut().unwrap().execution_state = "thinking".to_string();
    assert_eq!(
        resolve_dialog_preview_priority(&base_item, &meta, "zh-CN"),
        DialogPreviewPriority::Draft("unsent text".to_string())
    );

    // Priority 3: LiveActivity beats last message
    meta.draft_text = None;
    assert_eq!(
        resolve_dialog_preview_priority(&base_item, &meta, "zh-CN"),
        DialogPreviewPriority::LiveActivity("正在执行命令…".to_string())
    );

    // Priority 4: Last message when idle
    let mut idle_item = base_item.clone();
    idle_item.typing_indicator = None;
    meta.grok_avatar.as_mut().unwrap().execution_state = "idle".to_string();
    assert_eq!(
        resolve_dialog_preview_priority(&idle_item, &meta, "zh-CN"),
        DialogPreviewPriority::Message("Last snippet".to_string())
    );

    // ToolPermissionDock 4-tier policy badges & prev/next stepping
    let mut dock = ToolPermissionDock::new(0);
    dock.push_request(
        PendingToolApproval::new("apr-1", "terminal", "Run cargo check")
            .with_target("cargo check --workspace")
            .with_cwd("/workspace")
            .with_policy_tier(ToolApprovalPolicyTier::Ask),
    );
    dock.push_request(
        PendingToolApproval::new("apr-2", "write_file", "Patch config")
            .with_target("config.toml")
            .with_policy_tier(ToolApprovalPolicyTier::AutoReview),
    );
    assert_eq!(dock.pending_count, 2);
    assert_eq!(dock.active_request().map(|r| r.id.as_str()), Some("apr-1"));
    assert_eq!(ToolApprovalPolicyTier::AutoReview.label("zh-CN"), "自动审核");
    assert_eq!(ToolApprovalPolicyTier::Ask.label("zh-CN"), "需要批准");
    assert_eq!(ToolApprovalPolicyTier::Blocked.label("zh-CN"), "已阻止");
    assert_eq!(ToolApprovalPolicyTier::AlwaysAllow.label("zh-CN"), "始终允许");

    dock.step_next();
    assert_eq!(dock.active_request().map(|r| r.id.as_str()), Some("apr-2"));
    dock.step_prev();
    assert_eq!(dock.active_request().map(|r| r.id.as_str()), Some("apr-1"));

    dock.decide("apr-1", ToolApprovalDecision::AllowOnce);
    assert_eq!(dock.pending_count, 1);
    assert_eq!(
        dock.take_last_decision(),
        Some(("apr-1".to_string(), ToolApprovalDecision::AllowOnce))
    );
    assert_eq!(dock.active_request().map(|r| r.id.as_str()), Some("apr-2"));
}
