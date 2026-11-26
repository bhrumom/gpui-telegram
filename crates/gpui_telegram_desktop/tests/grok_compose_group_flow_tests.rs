//! Regression tests for Grok Bot-style multi-select compose, zero fake `"新信息"` sidebar row,
//! transient `"创建中…"` stand-in, P0 compose send-leak guard, and 1-vs-N dispatch decisions.

use gpui_telegram_client::net::http_client::ChatSummary;
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_authoritative_chat_list, evaluate_compose_send_decision, select_authoritative_dialog,
    try_consume_composer_for_send, validate_selected_send_target, ComposeSendDecision,
};
use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
use gpui_telegram_desktop::views::chat_sidebar::{
    ChatSidebar, ComposeRecipientChip, PendingComposeCreation,
};
use gpui_telegram_desktop::views::context_inspector::ContextInspector;
use gpui_telegram_desktop::views::conversation_stage::ConversationStage;

fn make_chip(
    user_id: u64,
    display_name: &str,
    username: Option<&str>,
    is_bot: bool,
    chat_id: Option<u64>,
) -> ComposeRecipientChip {
    ComposeRecipientChip {
        user_id,
        chat_id,
        username: username.map(str::to_string),
        display_name: display_name.to_string(),
        initials: display_name.chars().take(2).collect::<String>().to_uppercase(),
        is_bot,
        bot_id: if is_bot {
            username.map(str::to_string)
        } else {
            None
        },
        avatar_color: if is_bot { 0x8F59F9 } else { 0x0A84FF },
        avatar_shape: if is_bot {
            Some("blob".to_string())
        } else {
            None
        },
    }
}

fn selected_workspace(chat_id: u64) -> (ChatSidebar, ConversationStage, ContextInspector) {
    let mut sidebar = ChatSidebar::new();
    sidebar.set_palette(TdesktopThemePalette::build(
        TdesktopThemeMode::Night,
        0x0A84FF,
        14,
        18,
    ));
    let mut stage = ConversationStage::new();
    let mut inspector = ContextInspector::new();
    let chats = vec![ChatSummary {
        chat_id,
        title: "Existing Prior Chat".to_string(),
        chat_type: 0,
        last_message: Some("Hello prior chat".to_string()),
        unread_count: 0,
        is_group: Some(false),
        is_bot: Some(false),
        avatar_color: Some(0x2AABEE),
        updated_at: Some(1_730_000_000_000),
    }];
    apply_authoritative_chat_list(&mut sidebar, &mut stage, &mut inspector, &chats);
    assert!(select_authoritative_dialog(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        chat_id
    ));
    (sidebar, stage, inspector)
}

#[test]
fn compose_mode_never_emits_fake_new_message_sidebar_row() {
    // 1. Source contract: sidebar renderer must NOT contain the old synthetic `"新信息"` draft row,
    //    and MUST only mount a transient `"创建中…"` stand-in after submission (`pending_creation`).
    let render_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/views/sidebar/sidebar_main_render.rs");
    let src = std::fs::read_to_string(&render_path)
        .expect("Failed to read sidebar_main_render.rs");
    assert!(
        !src.contains("sidebar-compose-draft-row"),
        "Fake blue #sidebar-compose-draft-row must be completely removed"
    );
    assert!(
        !src.contains("\"新信息\""),
        "Sidebar must never render synthetic '新信息' draft row"
    );
    assert!(
        src.contains("sidebar-compose-creating-row") && src.contains("\"创建中…\""),
        "Sidebar must render transient '创建中…' stand-in only when pending_creation is Some"
    );

    // 2. Runtime state contract: opening compose does not inject any synthetic dialog into `sb.dialogs`.
    let (mut sidebar, _stage, _inspector) = selected_workspace(101);
    assert_eq!(sidebar.dialogs.len(), 1);
    sidebar.toggle_plus_menu();
    assert!(sidebar.is_apple_compose_active());
    assert!(sidebar.compose_draft.menu_open);
    assert!(sidebar.compose_draft.pending_creation.is_none());
    assert_eq!(
        sidebar.dialogs.len(),
        1,
        "Opening compose mode must never insert a synthetic dialog row"
    );
    assert!(
        !sidebar.dialogs.iter().any(|d| d.peer_name == "新信息" || d.peer_name == "New Message"),
        "No fake New Message dialog may exist in sidebar.dialogs"
    );
}

#[test]
fn multi_select_chips_add_remove_dedup_backspace_pop_and_auto_title_truncation() {
    let mut sb = ChatSidebar::new();
    sb.set_language("zh-CN");
    sb.toggle_plus_menu();

    let alice = make_chip(201, "Alice", Some("alice"), false, Some(501));
    let bob = make_chip(202, "Bob", Some("bob"), false, Some(502));
    let bot = make_chip(9001, "CodeBot", Some("code_bot"), true, Some(601));

    // Add first chip -> stays in single mode until >= 2 chips
    assert!(sb.add_compose_recipient(alice.clone()));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 1);
    assert!(!sb.compose_draft.is_group_mode);

    // Duplicate user_id is rejected
    assert!(!sb.add_compose_recipient(alice.clone()));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 1);

    // Adding second chip flips is_group_mode and keeps menu open
    assert!(sb.add_compose_recipient(bob.clone()));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 2);
    assert!(sb.compose_draft.is_group_mode);
    assert!(sb.compose_draft.menu_open);

    // Adding third chip (AI Bot)
    assert!(sb.add_compose_recipient(bot.clone()));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 3);
    assert!(sb.has_compose_bot_recipient());

    // Auto-generated narrow conjunction title in zh-CN
    assert_eq!(sb.auto_generate_group_title(None), "Alice、Bob、CodeBot");
    assert_eq!(
        sb.auto_generate_group_title(Some("Me")),
        "Me、Alice、Bob、CodeBot"
    );

    // Auto-generated narrow conjunction title in en-US
    sb.set_language("en-US");
    assert_eq!(sb.auto_generate_group_title(None), "Alice, Bob & CodeBot");
    assert_eq!(
        sb.auto_generate_group_title(Some("Me")),
        "Me, Alice, Bob & CodeBot"
    );

    // Toggle removes existing chip; remove_compose_recipient removes specific chip; Backspace pops last
    assert!(!sb.toggle_compose_recipient(bob.clone()));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 2);
    assert!(sb.toggle_compose_recipient(bob.clone()));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 3);

    let popped = sb.pop_last_compose_recipient().expect("should pop last chip");
    assert_eq!(popped.user_id, 202);
    assert_eq!(sb.compose_draft.selected_recipients.len(), 2);

    assert!(sb.remove_compose_recipient(201));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 1);
    assert_eq!(sb.compose_draft.selected_recipients[0].user_id, 9001);

    // Verify clean <= 72 char truncation for long multibyte names
    sb.set_language("zh-CN");
    sb.compose_draft.selected_recipients.clear();
    for i in 0..10 {
        sb.add_compose_recipient(make_chip(
            1000 + i,
            &format!("非常长的智能体协作群成员名称第{i}号"),
            None,
            i % 2 == 0,
            None,
        ));
    }
    let long_title = sb.auto_generate_group_title(None);
    assert!(
        long_title.chars().count() <= 72,
        "Auto-generated title must be <= 72 Unicode chars, got {}",
        long_title.chars().count()
    );
    assert!(long_title.ends_with('…'));
}

#[test]
fn dynamic_compose_placeholder_formatting_zh_and_en() {
    let mut sb = ChatSidebar::new();
    sb.toggle_plus_menu();

    assert_eq!(sb.compose_placeholder_text("zh-CN"), "发消息给联系人或 Bot");
    assert_eq!(sb.compose_placeholder_text("en-US"), "Message a contact or Bot");

    sb.add_compose_recipient(make_chip(11, "Alice", Some("alice"), false, None));
    assert_eq!(sb.compose_placeholder_text("zh-CN"), "发消息给 Alice");
    assert_eq!(sb.compose_placeholder_text("en-US"), "Message Alice");

    sb.add_compose_recipient(make_chip(12, "CodeBot", Some("code_bot"), true, None));
    assert_eq!(sb.compose_placeholder_text("zh-CN"), "发消息给 Alice 和 CodeBot");
    assert_eq!(sb.compose_placeholder_text("en-US"), "Message Alice and CodeBot");

    sb.add_compose_recipient(make_chip(13, "ResearchBot", Some("res_bot"), true, None));
    assert_eq!(
        sb.compose_placeholder_text("zh-CN"),
        "发消息给 Alice、CodeBot 和 ResearchBot"
    );
    assert_eq!(
        sb.compose_placeholder_text("en-US"),
        "Message Alice, CodeBot, and ResearchBot"
    );
}

#[test]
fn compose_zero_recipient_send_guard_never_leaks_to_previous_chat() {
    let (mut sidebar, stage, _inspector) = selected_workspace(101);
    let mut composer = CanonicalComposer::new();

    // Prior chat 101 is still selected in sidebar/stage when user clicks Compose!
    assert_eq!(sidebar.active_chat_id, Some(101));
    assert_eq!(stage.active_chat_id, Some(101));

    sidebar.toggle_plus_menu();
    assert!(sidebar.is_apple_compose_active());

    // Typing and submitting in bottom composer while in compose mode must NEVER resolve prior chat 101!
    composer.set_text("Do NOT leak this to prior chat 101");
    composer.submit_primary();
    assert!(
        validate_selected_send_target(true, Some("tok"), Some(10), &sidebar, &stage).is_none(),
        "validate_selected_send_target must block sending to prior chat while compose mode is open"
    );
    assert!(
        try_consume_composer_for_send(
            true,
            Some("tok"),
            Some(10),
            &sidebar,
            &stage,
            &mut composer,
        )
        .is_none(),
        "try_consume_composer_for_send must never leak compose submission to previous chat"
    );
    assert_eq!(
        composer.text(),
        "Do NOT leak this to prior chat 101",
        "Blocked send must preserve the user's draft safely in composer"
    );
    assert_eq!(
        evaluate_compose_send_decision(&sidebar, None),
        ComposeSendDecision::BlockedEmptyRecipients
    );

    // Exiting compose restores normal sending to chat 101 cleanly.
    sidebar.exit_compose_mode();
    assert!(!sidebar.is_apple_compose_active());
    composer.submit_primary();
    let (target, sent) = try_consume_composer_for_send(
        true,
        Some("tok"),
        Some(10),
        &sidebar,
        &stage,
        &mut composer,
    )
    .expect("Sending should work normally after exiting compose mode");
    assert_eq!(target.chat_id, 101);
    assert_eq!(sent, "Do NOT leak this to prior chat 101");
}

#[test]
fn compose_dispatch_decision_distinguishes_pending_single_dm_and_multi_member_group() {
    let mut sb = ChatSidebar::new();
    sb.set_language("zh-CN");
    sb.toggle_plus_menu();

    // 0 recipients -> blocked
    assert_eq!(
        evaluate_compose_send_decision(&sb, None),
        ComposeSendDecision::BlockedEmptyRecipients
    );

    // 1 recipient -> 1:1 DirectMessage
    let bot = make_chip(5001, "ArchitectBot", Some("@architect_bot"), true, Some(77));
    sb.add_compose_recipient(bot.clone());
    assert_eq!(
        evaluate_compose_send_decision(&sb, None),
        ComposeSendDecision::DirectMessage {
            recipient: bot.clone(),
        }
    );

    // 2+ recipients (mixed bot + human) -> CreateGroup with auto-generated title & deduplicated IDs/usernames
    let alice = make_chip(42, "Alice", Some("alice"), false, Some(88));
    let reviewer = make_chip(5002, "ReviewBot", Some("review_bot"), true, None);
    sb.add_compose_recipient(alice.clone());
    sb.add_compose_recipient(reviewer.clone());

    match evaluate_compose_send_decision(&sb, None) {
        ComposeSendDecision::CreateGroup {
            title,
            recipients,
            member_ids,
            member_usernames,
        } => {
            assert_eq!(title, "ArchitectBot、Alice、ReviewBot");
            assert_eq!(recipients.len(), 3);
            assert_eq!(member_ids, vec![5001, 42, 5002]);
            assert_eq!(
                member_usernames,
                vec![
                    "architect_bot".to_string(),
                    "alice".to_string(),
                    "review_bot".to_string()
                ]
            );
        }
        other => panic!("Expected CreateGroup decision, got {:?}", other),
    }

    // Custom group title overrides auto-generated title
    sb.compose_draft.custom_group_title = "Core Architecture War Room".to_string();
    match evaluate_compose_send_decision(&sb, None) {
        ComposeSendDecision::CreateGroup { title, .. } => {
            assert_eq!(title, "Core Architecture War Room");
        }
        other => panic!("Expected CreateGroup with custom title, got {:?}", other),
    }

    // In-flight pending creation blocks duplicate submission
    sb.compose_draft.pending_creation = Some(PendingComposeCreation {
        title: "Core Architecture War Room".to_string(),
        recipients: vec![bot, alice, reviewer],
        initial_message: "Let's review the RFC".to_string(),
        client_nonce: "grp-123-1".to_string(),
    });
    assert_eq!(
        evaluate_compose_send_decision(&sb, None),
        ComposeSendDecision::BlockedPendingCreation
    );

    // Exiting compose clears draft chips but does not drop in-flight pending creation stand-in
    sb.exit_compose_mode();
    assert!(!sb.is_plus_menu_open);
    assert!(sb.compose_draft.selected_recipients.is_empty());
    assert!(sb.compose_draft.pending_creation.is_some());
}

#[test]
fn open_compose_with_seed_recipient_populates_chip_and_enters_group_mode() {
    let mut sb = ChatSidebar::new();
    sb.set_collapsed(true);

    sb.open_compose_with_seed_recipient(
        777001,
        "Research Assistant".to_string(),
        Some("@research_bot".to_string()),
        true,
        Some("agent_research".to_string()),
        0x8F59F9,
    );

    assert!(!sb.is_collapsed, "Opening seeded compose should expand collapsed sidebar");
    assert!(sb.is_plus_menu_open);
    assert!(sb.compose_draft.is_group_mode);
    assert!(sb.compose_draft.menu_open);
    assert_eq!(sb.compose_draft.selected_recipients.len(), 1);

    let seed = &sb.compose_draft.selected_recipients[0];
    assert_eq!(seed.user_id, 777001);
    assert_eq!(seed.display_name, "Research Assistant");
    assert_eq!(seed.username.as_deref(), Some("research_bot"));
    assert!(seed.is_bot);
    assert_eq!(seed.bot_id.as_deref(), Some("agent_research"));
    assert_eq!(seed.avatar_color, 0x8F59F9);

    // Adding a second contact immediately forms a group
    sb.add_compose_recipient(make_chip(303, "Linus", Some("linus"), false, None));
    assert_eq!(sb.compose_draft.selected_recipients.len(), 2);
    assert_eq!(sb.auto_generate_group_title(None), "Research Assistant、Linus");

    // Highlight wraparound helper works smoothly
    sb.move_compose_highlight(1, 3);
    assert_eq!(sb.compose_draft.highlighted_index, 1);
    sb.move_compose_highlight(-2, 3);
    assert_eq!(sb.compose_draft.highlighted_index, 2);
}

#[test]
fn bot_to_group_chat_switch_isolates_inspector_bio_copied_feedback_and_stage_toast() {
    use gpui_telegram_desktop::views::drawer::inspector_tabs::{InspectorSubpage, InspectorTab};
    use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;

    let mut sidebar = ChatSidebar::new();
    let mut stage = ConversationStage::new();
    let mut inspector = ContextInspector::new();

    let chats = vec![
        ChatSummary {
            chat_id: 101,
            title: "全球法布施".to_string(),
            chat_type: 4,
            last_message: Some("Welcome".to_string()),
            unread_count: 0,
            is_group: Some(false),
            is_bot: Some(true),
            avatar_color: Some(0x8F59F9),
            updated_at: Some(1_730_000_000_000),
        },
        ChatSummary {
            chat_id: 202,
            title: "AA 记账分账助手".to_string(),
            chat_type: 1,
            last_message: Some("Group created".to_string()),
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: Some(0x2AABEE),
            updated_at: Some(1_730_000_001_000),
        },
    ];
    apply_authoritative_chat_list(&mut sidebar, &mut stage, &mut inspector, &chats);

    // 1. Select Bot chat 101 and populate Bot-specific inspector & stage state
    assert!(select_authoritative_dialog(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        101
    ));
    assert!(inspector.profile.is_bot);
    assert!(!inspector.members.is_group_or_channel);

    let bot_summary = FabushiBotSummaryDto {
        bot_user_id: 777001,
        agent_id: "fabushi".to_string(),
        username: "fabushi_bot".to_string(),
        display_name: "全球法布施".to_string(),
        ..Default::default()
    };
    inspector.profile.bio = "全球法布施与净心禅室 AI Bot".to_string();
    inspector.profile.username = "@fabushi_bot".to_string();
    inspector.profile.avatar_path = Some("/tmp/fabushi.png".to_string());
    inspector.profile.is_verified = true;
    inspector.copied_feedback = Some("已复制: @fabushi_bot".to_string());
    inspector.active_fabushi_bot = Some(bot_summary.clone());
    inspector.subpage = InspectorSubpage::ComputersList;
    inspector.active_tab = InspectorTab::Members;

    stage.toast_banner = Some("Copied to clipboard".to_string());
    stage.set_active_fabushi_bot(Some(bot_summary));
    stage.on_agent_floor_changed(101, Some("全球法布施".to_string()), 1, 1, 50_000);

    // 2. Switch to Group chat 202 -> synchronous isolation across Inspector and Stage
    assert!(select_authoritative_dialog(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        202
    ));
    assert_eq!(inspector.profile.name, "AA 记账分账助手");
    assert!(inspector.profile.bio.is_empty());
    assert!(inspector.profile.username.is_empty());
    assert!(inspector.profile.phone.is_empty());
    assert!(inspector.profile.avatar_path.is_none());
    assert!(!inspector.profile.is_verified);
    assert!(!inspector.profile.is_bot);
    assert!(inspector.members.is_group_or_channel);
    assert!(inspector.copied_feedback.is_none());
    assert!(inspector.active_fabushi_bot.is_none());
    assert_eq!(inspector.subpage, InspectorSubpage::Main);
    assert_eq!(inspector.active_tab, InspectorTab::Profile);

    assert!(stage.is_group_chat);
    assert!(!stage.is_bot_chat);
    assert!(stage.toast_banner.is_none());
    assert!(stage.active_fabushi_bot.is_none());
    assert!(stage.room_agent_states.is_empty());

    // 3. Same-chat refresh preserves populated Group bio/tab/facts
    inspector.profile.bio = "AA 分账群描述".to_string();
    inspector.active_tab = InspectorTab::Members;
    inspector.profile.facts = vec![("类型".to_string(), "群组".to_string())];
    let preserved = apply_authoritative_chat_list(&mut sidebar, &mut stage, &mut inspector, &chats);
    assert_eq!(preserved, Some(202));
    assert_eq!(inspector.profile.bio, "AA 分账群描述");
    assert_eq!(inspector.active_tab, InspectorTab::Members);
    assert_eq!(
        inspector.profile.facts,
        vec![("类型".to_string(), "群组".to_string())]
    );
}

#[test]
fn group_chat_reply_to_forwarding_and_bot_activity_non_corruption_contracts() {
    // 1. Runtime stage invariant: bot thinking activity in a group preserves is_group_chat
    //    and never flips is_bot_chat = true.
    let mut stage = ConversationStage::new();
    stage.set_chat(303, "RustBot, FinBot", "3 members", "RF", 0x2AABEE);
    stage.is_group_chat = true;
    stage.set_bot_activity("thinking", Some("RustBot 正在思考...".to_string()));
    assert!(stage.is_group_chat);
    assert!(!stage.is_bot_chat);
    assert_eq!(stage.bot_activity_state, "thinking");
    assert_eq!(
        stage.bot_activity_detail.as_deref(),
        Some("RustBot 正在思考...")
    );

    // 2. Source contracts across desktop shell files.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let ws_sync_src =
        std::fs::read_to_string(manifest_dir.join("src/views/app_shell/shell_ws_sync.rs"))
            .expect("read shell_ws_sync.rs");
    assert!(
        ws_sync_src.contains("c.replying_to.take()"),
        "append_outgoing_text_inner must consume composer.replying_to"
    );
    assert!(
        ws_sync_src.contains("reply_to: reply_to_id"),
        "RichSendMessageRequest must forward reply_to_id"
    );
    assert!(
        ws_sync_src.contains("let is_ai_bot = !is_group_or_channel"),
        "Group titles containing 'Bot' must not flip is_bot_chat = true"
    );

    let pts_src =
        std::fs::read_to_string(manifest_dir.join("src/views/app_shell/shell_pts_updates.rs"))
            .expect("read shell_pts_updates.rs");
    assert!(
        pts_src.contains("if !is_group_or_channel {\n                            sb.bot_ids.insert(target_chat_u64);"),
        "BotActivityChanged must not insert group chats into sidebar.bot_ids"
    );
    assert!(
        pts_src.contains("if !is_group_or_channel && !st.is_group_chat {\n                                st.is_bot_chat = true;"),
        "BotActivityChanged must not flip stage.is_bot_chat in group chats"
    );

    let stage_src =
        std::fs::read_to_string(manifest_dir.join("src/views/conversation_stage.rs"))
            .expect("read conversation_stage.rs");
    assert!(
        stage_src.contains("(self.is_bot_chat || self.is_group_chat)"),
        "ConversationStage must render bot thinking indicator in group chats"
    );

    let stage_actions_src =
        std::fs::read_to_string(manifest_dir.join("src/views/app_shell/shell_stage_actions.rs"))
            .expect("read shell_stage_actions.rs");
    assert!(
        stage_actions_src.contains("self.client.subscribe_chat(chat_id as i64)"),
        "Selecting a chat must subscribe the WebSocket gateway to chat_id"
    );
}

