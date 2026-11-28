use gpui_telegram_client::net::hermes_client::{
    HermesApprovalDto, HermesCommandDescriptor, HermesCronJobDto, HermesHeartbeatConfigDto,
    HermesHonchoStateDto, HermesInlineButtonDto, HermesKanbanBoardDto, HermesKanbanCardDto,
    HermesMemoryEntryDto, HermesMemoryStateDto, HermesProviderConfigDto, HermesRichMessageDto,
    HermesSessionDto, HermesSessionExportResponse, HermesSkillDto, HermesSoulDto,
    HermesStatusDto, HermesTerminalProcessDto, HermesUsageSummaryDto,
};
use gpui_telegram_client::ChatSummary;
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_authoritative_chat_list, decode_inline_keyboard_json, derive_initials,
    hermes_keyboard_to_stage, overlay_rich_message_metadata, select_authoritative_dialog,
};
use gpui_telegram_desktop::views::canonical_composer::extract_context_reference_badges;
use gpui_telegram_desktop::views::global_tasks_workspace::{
    default_28_hermes_toolsets, StudioInputTarget, TasksWorkspaceAction, TasksWorkspaceTab,
};
use gpui_telegram_desktop::views::stage::cards::{
    DeliveryStatus, InlineButtonType, MessageContent, MessageDirection, MessageItemModel,
};
use gpui_telegram_desktop::views::{
    CanonicalComposer, ChatSidebar, ContextInspector, ConversationStage, GlobalTasksWorkspace,
};
use serde_json::json;
use std::path::PathBuf;

fn desktop_src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn test_derive_initials_numeric_unicode_and_profile_inspector_sync() {
    // Regression: numeric chat title "23" previously yielded "?"
    assert_eq!(derive_initials("23"), "23");
    assert_eq!(derive_initials("404"), "40");
    // Unicode CJK titles
    assert_eq!(derive_initials("大乘"), "大");
    assert_eq!(derive_initials("Rust Core"), "RC");
    assert_eq!(derive_initials("   "), "");

    let mut sidebar = ChatSidebar::new();
    let mut stage = ConversationStage::new();
    let mut inspector = ContextInspector::new();

    let chats = vec![ChatSummary {
        chat_id: 23,
        title: "23".to_string(),
        chat_type: 4,
        last_message: Some("Ready".to_string()),
        unread_count: 1,
        is_group: Some(false),
        is_bot: Some(true),
        avatar_color: Some(0x2AABEE),
        updated_at: Some(1_700_000_000_000),
    }];
    let selected = apply_authoritative_chat_list(&mut sidebar, &mut stage, &mut inspector, &chats);
    assert_eq!(selected, None);
    assert_eq!(sidebar.dialogs.len(), 1);
    assert_eq!(sidebar.dialogs[0].folder, "Bots");
    assert_eq!(sidebar.dialogs[0].avatar_initials, "23");

    assert!(select_authoritative_dialog(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        23,
    ));
    assert_eq!(stage.chat_title, "23");
    assert_eq!(stage.chat_avatar_initials, "23");
    assert_eq!(inspector.profile.name, "23");
    assert_eq!(inspector.profile.avatar_initials, "23");
    assert_eq!(inspector.profile.status, "");

    inspector.set_peer("23", "Custom Fabushi persona", "23", 0x2AABEE);
    inspector.sync_profile_details(
        "@custom_ai_bot".to_string(),
        String::new(),
        "Fabushi AI Bot".to_string(),
        None,
        vec![("Engine".to_string(), "fabushi-agent".to_string())],
    );
    assert_eq!(inspector.profile.username, "@custom_ai_bot");
    assert_eq!(inspector.profile.status, "Fabushi AI Bot");
    assert_eq!(inspector.profile.bio, "Custom Fabushi persona");
    assert_eq!(
        inspector.profile.facts,
        vec![("Engine".to_string(), "fabushi-agent".to_string())]
    );
}

#[test]
fn test_rich_message_overlay_resolves_peer_identity_and_preserves_bot_keyboard() {
    let mut models = vec![MessageItemModel {
        id: 501,
        sender_id: 777_001,
        sender_name: "User 777001".to_string(),
        avatar_initials: "U7".to_string(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Incoming,
        timestamp: "14:20".to_string(),
        delivery_status: DeliveryStatus::Sent,
        reply_to: None,
        reactions: Vec::new(),
        content: MessageContent::Text {
            text: "你好，我已经准备好协助你。".to_string(),
            is_edited: false,
        },
        bot_keyboard: None,
        is_pinned: false,
    }];

    let rich = vec![HermesRichMessageDto {
        message_id: 501,
        chat_id: 23,
        sender_id: 777_001,
        sender_name: Some("Kestrel AI".to_string()),
        sender_initials: Some("KA".to_string()),
        is_bot_sender: true,
        text: "你好，我已经准备好协助你。".to_string(),
        content: None,
        date_ms: Some(1_700_000_000_000),
        reply_to: None,
        is_outgoing: Some(false),
        inline_keyboard: Some(vec![vec![
            HermesInlineButtonDto {
                text: "Launch Talk Mode".to_string(),
                callback_data: Some("talk:start".to_string()),
                url: None,
                web_app_url: None,
            },
            HermesInlineButtonDto {
                text: "Remote Computer".to_string(),
                callback_data: Some("cua:open".to_string()),
                url: None,
                web_app_url: None,
            },
        ]]),
        bot_keyboard: None,
    }];

    overlay_rich_message_metadata(&mut models, &rich);
    assert_eq!(models[0].sender_name, "Kestrel AI");
    assert_ne!(models[0].sender_name, "Peer");
    assert_eq!(models[0].avatar_initials, "KA");
    assert_ne!(models[0].avatar_initials, "PR");

    let kb = models[0]
        .bot_keyboard
        .as_ref()
        .expect("bot_keyboard preserved after rich overlay");
    assert_eq!(kb.len(), 1);
    assert_eq!(kb[0].len(), 2);
    assert_eq!(kb[0][0].text, "Launch Talk Mode");
    assert_eq!(
        kb[0][0].button_type,
        InlineButtonType::Callback("talk:start".to_string())
    );
    assert_eq!(kb[0][1].text, "Remote Computer");
    assert_eq!(
        kb[0][1].button_type,
        InlineButtonType::Callback("cua:open".to_string())
    );

    // JSON keyboard decode + URL / WebApp conversions
    let decoded = decode_inline_keyboard_json(&json!([[
        { "text": "Docs", "url": "https://example.com/docs" },
        { "text": "Mini App", "web_app_url": "https://example.com/app" }
    ]]))
    .expect("json inline keyboard decodes");
    assert_eq!(
        decoded[0][0].button_type,
        InlineButtonType::Url("https://example.com/docs".to_string())
    );
    assert_eq!(
        decoded[0][1].button_type,
        InlineButtonType::WebApp("https://example.com/app".to_string())
    );

    let converted = hermes_keyboard_to_stage(&[vec![HermesInlineButtonDto {
        text: "Run /help".to_string(),
        callback_data: Some("/help".to_string()),
        url: None,
        web_app_url: None,
    }]])
    .expect("hermes keyboard converts");
    assert_eq!(
        converted[0][0].button_type,
        InlineButtonType::Callback("/help".to_string())
    );
}

#[test]
fn test_canonical_composer_slash_autocomplete_and_context_badges() {
    let mut composer = CanonicalComposer::new();
    assert!(!composer.hermes_slash_enabled);
    composer.hermes_slash_enabled = true;
    assert!(
        composer.hermes_commands.len() >= 50,
        "Expected 50+ Hermes slash command autocomplete entries, got {}",
        composer.hermes_commands.len()
    );
    for required in [
        "/help",
        "/memory",
        "/soul",
        "/kanban",
        "/cron",
        "/goal",
        "/moa",
        "/skills",
        "/toolsets",
        "/rollback",
        "/undo",
        "/yolo",
        "/talk",
        "/export",
    ] {
        assert!(
            composer.hermes_commands.iter().any(|c| c.slash_name == required),
            "Missing Hermes command {required} in CanonicalComposer"
        );
    }

    // Typing slash query opens autocomplete & matches
    composer.set_text("/mem");
    composer.ime_machine.cursor_index = composer.text().len();
    composer.sync_autocomplete();
    assert!(composer.hermes_ac_visible);
    assert!(composer
        .hermes_ac_matches
        .iter()
        .any(|c| c.slash_name == "/memory"));

    // Keyboard navigation and completion
    assert!(composer.handle_hermes_autocomplete_nav("down", false));
    assert!(composer.handle_hermes_autocomplete_nav("up", false));
    assert!(composer.handle_hermes_autocomplete_nav("tab", false));
    assert!(!composer.hermes_ac_visible);
    assert!(composer.text().starts_with("/"));

    // Escape hides popup cleanly
    composer.set_text("/kan");
    composer.ime_machine.cursor_index = composer.text().len();
    composer.sync_autocomplete();
    assert!(composer.hermes_ac_visible);
    assert!(composer.handle_hermes_autocomplete_nav("escape", false));
    assert!(!composer.hermes_ac_visible);

    // Custom server descriptor sync replaces command table cleanly
    composer.set_hermes_commands(vec![HermesCommandDescriptor {
        name: "kanban".to_string(),
        slash_name: "/kanban".to_string(),
        aliases: vec!["board".to_string()],
        syntax: "/kanban [list|add]".to_string(),
        category: "tasks".to_string(),
        description: "Manage Kanban boards".to_string(),
        accepts_args: true,
    }]);
    composer.set_text("/boa");
    composer.ime_machine.cursor_index = composer.text().len();
    composer.sync_autocomplete();
    assert!(composer.hermes_ac_visible);

    // @file / @dir / @diff / @url context badges
    let badges = extract_context_reference_badges(
        "Check @file:src/main.rs, @dir:crates/agent, @diff and @url:https://hermes.local/spec",
    );
    assert_eq!(badges.len(), 4);
    assert_eq!(badges[0].0, "FILE");
    assert_eq!(badges[1].0, "DIR");
    assert_eq!(badges[2].0, "GIT");
    assert_eq!(badges[3].0, "URL");
}

#[test]
fn test_global_tasks_workspace_6_hermes_studio_tabs_and_state_sync() {
    let default_toolsets = default_28_hermes_toolsets();
    assert_eq!(default_toolsets.len(), 28);

    let mut ws = GlobalTasksWorkspace::new();
    assert_eq!(ws.toolsets.len(), 28);
    assert_eq!(ws.status_summary.toolsets_count, 28);
    assert_eq!(ws.status_summary.tools_count, 58);

    let tab_cases = [
        (TasksWorkspaceTab::Tasks, 0usize),
        (TasksWorkspaceTab::Skills, 1usize),
        (TasksWorkspaceTab::MemorySoul, 2usize),
        (TasksWorkspaceTab::McpTools, 3usize),
        (TasksWorkspaceTab::Cron, 4usize),
        (TasksWorkspaceTab::Sessions, 5usize),
    ];
    for (tab, expected_idx) in tab_cases {
        ws.set_tab(tab);
        assert_eq!(ws.tab, tab);
        assert_eq!(ws.active_tab, expected_idx);
    }
    ws.set_tab(TasksWorkspaceTab::Approvals);
    assert_eq!(ws.tab, TasksWorkspaceTab::Approvals);
    assert_eq!(ws.active_tab, 0);

    ws.sync_studio_status(HermesStatusDto {
        healthy: true,
        engine: "fabushi-agent".to_string(),
        active_model: "claude-sonnet-4".to_string(),
        active_provider: "anthropic".to_string(),
        api_mode: "anthropic_messages".to_string(),
        toolsets_count: 28,
        tools_count: 58,
        skills_count: 3,
        cron_count: 2,
        sessions_count: 4,
        pending_approvals_count: 1,
        heartbeat_enabled: true,
        ..Default::default()
    });
    assert_eq!(ws.active_api_mode, "anthropic_messages");

    ws.sync_kanban_boards(vec![HermesKanbanBoardDto {
        board_id: "b1".to_string(),
        name: "Hermes Sprint".to_string(),
        columns: vec!["Backlog".into(), "Doing".into(), "Done".into()],
        cards: vec![HermesKanbanCardDto {
            card_id: "c1".to_string(),
            board_id: "b1".to_string(),
            title: "Wire Studio".to_string(),
            description: "Wire Hermes Studio panels".to_string(),
            column: "Doing".to_string(),
            priority: "high".to_string(),
            assigned_agent: Some("hermes".to_string()),
            position: 0,
            created_at_ms: 1,
            updated_at_ms: 1,
        }],
        created_at_ms: 1,
    }]);
    assert_eq!(ws.active_board_id, "b1");

    ws.sync_hermes_approvals(vec![HermesApprovalDto {
        approval_id: "apr_1".to_string(),
        session_id: "s1".to_string(),
        tool_name: "terminal".to_string(),
        command_preview: "cargo check".to_string(),
        risk_level: "caution".to_string(),
        reason: "Confirmation needed".to_string(),
        status: "pending".to_string(),
        created_at_ms: 1,
        decided_at_ms: None,
    }]);
    assert_eq!(ws.hermes_approvals.len(), 1);

    ws.sync_skills(vec![HermesSkillDto {
        name: "wal-audit".to_string(),
        description: "Audits SQLite WAL".to_string(),
        category: "user".to_string(),
        content: "# WAL Audit".to_string(),
        enabled: true,
        approved: true,
        security_status: "safe".to_string(),
        tags: vec!["sqlite".into()],
        updated_at_ms: 1,
    }]);
    assert_eq!(ws.selected_skill_name.as_deref(), Some("wal-audit"));

    ws.sync_memory_state(HermesMemoryStateDto {
        entries: vec![HermesMemoryEntryDto {
            id: "mem_0".into(),
            key: "mem_0".into(),
            content: "Keep debug=0".into(),
            category: "memory".into(),
            scope: "memory".into(),
            char_count: 12,
            updated_at_ms: 1,
        }],
        memory_markdown: "Keep debug=0".into(),
        user_markdown: "Prefers Rust".into(),
        total_chars: 12,
        char_limit: 2200,
    });
    assert_eq!(ws.memory_edit_draft, "Keep debug=0");
    assert_eq!(ws.user_edit_draft, "Prefers Rust");

    ws.sync_soul_state(HermesSoulDto {
        bot_id: None,
        chat_id: None,
        persona_name: "Kestrel".into(),
        content: "Zero legacy persona".into(),
        system_prompt: None,
        welcome_message: None,
        model: Some("claude-sonnet-4".into()),
        updated_at_ms: 1,
    });
    assert_eq!(ws.soul_persona_draft, "Kestrel");

    ws.sync_honcho_state(HermesHonchoStateDto {
        enabled: true,
        peer_name: Some("Alice".into()),
        session_notes: Vec::new(),
        profile_summary: "Systems engineer".into(),
        updated_at_ms: 1,
    });
    assert!(ws.honcho_state.enabled);

    ws.sync_terminal_processes(vec![HermesTerminalProcessDto {
        process_id: "proc_1".into(),
        session_id: Some("s1".into()),
        command: "cat".into(),
        status: "running".into(),
        pid: Some(4242),
        exit_code: None,
        started_at_ms: 1,
        output_tail: Some("ready".into()),
    }]);
    assert_eq!(ws.terminal_processes.len(), 1);

    ws.sync_cron_jobs(vec![HermesCronJobDto {
        job_id: "cron_1".into(),
        name: "Nightly".into(),
        schedule: "0 2 * * *".into(),
        prompt: "Summarize".into(),
        chat_id: Some(23),
        bot_id: None,
        enabled: true,
        last_run_ms: None,
        next_run_ms: Some(2),
        last_status: None,
    }]);
    ws.sync_heartbeat(HermesHeartbeatConfigDto {
        enabled: true,
        interval_secs: 600,
        chat_id: Some(23),
        bot_id: None,
        prompt: "Check tasks".into(),
        last_beat_ms: None,
    });
    assert_eq!(ws.cron_jobs.len(), 1);
    assert!(ws.heartbeat.enabled);

    ws.sync_sessions(vec![HermesSessionDto {
        session_id: "s1".into(),
        chat_id: Some(23),
        bot_id: None,
        title: Some("Primary".into()),
        model: "claude-sonnet-4".into(),
        provider: "anthropic".into(),
        parent_session_id: None,
        branch_name: None,
        message_count: 4,
        token_count: 100,
        created_at_ms: 1,
        updated_at_ms: 2,
    }]);
    ws.sync_session_search_hits(vec![json!({ "session_id": "s1", "snippet": "Primary" })]);
    ws.sync_session_export(HermesSessionExportResponse {
        session_id: "s1".into(),
        format: "sharegpt".into(),
        content: "{\"conversations\":[]}".into(),
        exported_at_ms: 2,
    });
    ws.sync_providers(vec![HermesProviderConfigDto {
        provider_id: "openai".into(),
        name: "OpenAI".into(),
        api_mode: "codex_responses".into(),
        base_url: None,
        default_model: "gpt-5".into(),
        models: vec!["gpt-5".into()],
        enabled: true,
        has_api_key: true,
        is_active: true,
    }]);
    ws.sync_usage_summary(HermesUsageSummaryDto {
        total_turns: 5,
        prompt_tokens: 500,
        completion_tokens: 300,
        total_tokens: 800,
        estimated_cost_usd: 0.02,
        tool_calls: 2,
        by_model: Vec::new(),
    });
    assert_eq!(ws.active_api_mode, "codex_responses");
    assert_eq!(ws.subagent_lineage.len(), 1);

    // Interactive input emission
    ws.active_input = StudioInputTarget::NewTask;
    ws.task_draft = "Audit WAL lock".to_string();
    assert!(ws.apply_text_key_to_active_input("enter", None));
    assert_eq!(
        ws.take_action(),
        Some(TasksWorkspaceAction::CreateTask("Audit WAL lock".to_string()))
    );
}

#[test]
fn test_desktop_bot_call_inline_routing_and_zero_legacy_persona_contract() {
    let root = desktop_src_root();
    let stage_actions_src =
        std::fs::read_to_string(root.join("views/app_shell/shell_stage_actions.rs")).unwrap();
    let calls_rtc_src =
        std::fs::read_to_string(root.join("views/app_shell/shell_calls_rtc.rs")).unwrap();

    let mut shell_paths = vec![root.join("views/shell.rs")];
    let mut app_shell_paths: Vec<_> = std::fs::read_dir(root.join("views/app_shell"))
        .expect("Failed to read views/app_shell")
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("shell_") && n.ends_with(".rs"))
                .unwrap_or(false)
        })
        .collect();
    app_shell_paths.sort();
    assert!(!app_shell_paths.is_empty(), "Expected views/app_shell/shell_*.rs files");
    shell_paths.extend(app_shell_paths);

    let mut all_shell_src = String::new();
    for path in &shell_paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("shell");
        let src = std::fs::read_to_string(path).unwrap();
        assert!(
            !src.contains("大乘 AI 助手"),
            "{name} must never hardcode legacy persona string"
        );
        assert!(
            !src.contains("\"PR\".to_string()"),
            "{name} must never hardcode fallback PR initials"
        );
        all_shell_src.push('\n');
        all_shell_src.push_str(&src);
    }

    let mut studio_paths = vec![root.join("views/hermes_studio_panels.rs")];
    let mut studio_subpaths: Vec<_> = std::fs::read_dir(root.join("views/hermes_studio_panels"))
        .expect("Failed to read views/hermes_studio_panels")
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rs"))
        .collect();
    studio_subpaths.sort();
    assert!(
        !studio_subpaths.is_empty(),
        "Expected views/hermes_studio_panels/*.rs files"
    );
    studio_paths.extend(studio_subpaths);
    for path in studio_paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("studio");
        let src = std::fs::read_to_string(&path).unwrap();
        assert!(
            !src.contains("大乘 AI 助手"),
            "{name} must never hardcode legacy persona string"
        );
    }

    // Incoming NewMessage uses resolve_incoming_sender_identity rather than hardcoded Peer/PR
    assert!(all_shell_src.contains("resolve_incoming_sender_identity"));
    assert!(!all_shell_src.contains("\"PR\".to_string()"));

    // Bot inline callback + TopBar AI Bot call route through Hermes call/Talk/CUA handlers
    assert!(stage_actions_src.contains("execute_hermes_inline_callback"));
    assert!(calls_rtc_src.contains("pub fn is_chat_bot"));
    assert!(calls_rtc_src.contains("pub fn is_active_chat_bot"));
    assert!(calls_rtc_src.contains("pub fn current_peer_user_id"));
    assert!(calls_rtc_src.contains("hermes_call_invite") || stage_actions_src.contains("hermes_call_invite"));
}

#[test]
fn test_rich_text_multiline_and_cjk_inline_wrapping_regression() {
    use gpui_telegram_desktop::views::stage::{
        parse_rich_blocks, split_wrappable_inline_tokens, EntityKind, RichBlock, RichEntity,
    };

    let sample = "你好！我是 1 (@1_bot).\n\n你可以做什么\n使用 `read file <path>` 查看文件";
    let blocks = parse_rich_blocks(sample, &[]);

    assert!(blocks.len() >= 4, "expected line-per-block paragraphs, got {blocks:?}");
    for block in &blocks {
        if let RichBlock::Paragraph(segs) = block {
            for seg in segs {
                assert!(
                    !seg.text.contains('\n') && !seg.text.contains('\r'),
                    "paragraph segment must not contain embedded newlines: {:?}",
                    seg.text
                );
            }
        }
    }

    let first_line = match &blocks[0] {
        RichBlock::Paragraph(segs) => segs,
        other => panic!("expected Paragraph first block, got {other:?}"),
    };
    assert!(
        first_line.iter().any(|s| s.flags.mention.as_deref() == Some("@1_bot")),
        "expected @1_bot mention in first paragraph line: {first_line:?}"
    );
    assert!(
        first_line.last().map(|s| s.text.as_str()) == Some(")."),
        "expected trailing ').' to stay on first paragraph line: {first_line:?}"
    );

    // Explicit UTF-16 entity parsing must also split lines without embedded newlines
    let explicit_sample = "第一行 (@1_bot).\n\n第二行 你可以做什么";
    let utf16_len = explicit_sample.encode_utf16().count();
    let mention_utf16_offset = "第一行 (".encode_utf16().count();
    let explicit_entities = vec![RichEntity {
        offset_utf16: mention_utf16_offset,
        length_utf16: "@1_bot".encode_utf16().count(),
        kind: EntityKind::Mention,
    }];
    assert!(utf16_len > mention_utf16_offset);
    let explicit_blocks = parse_rich_blocks(explicit_sample, &explicit_entities);
    assert_eq!(explicit_blocks.len(), 3, "expected 2 content lines + 1 blank spacer, got {explicit_blocks:?}");
    for block in &explicit_blocks {
        if let RichBlock::Paragraph(segs) = block {
            for seg in segs {
                assert!(!seg.text.contains('\n') && !seg.text.contains('\r'));
            }
        }
    }

    // CJK wrapping keeps closing punctuation glued to preceding token and strips newlines
    let tokens = split_wrappable_inline_tokens("我可以帮你执行。\n(Session turns: 3)");
    assert!(tokens.iter().all(|t| !t.contains('\n') && !t.contains('\r')));
    assert!(
        tokens.iter().any(|t| t == "行。"),
        "closing punctuation should attach to preceding CJK character: {tokens:?}"
    );
}

#[test]
fn test_markdown_heading_divider_list_blockquote_and_link_parsing() {
    use gpui_telegram_desktop::views::stage::{parse_rich_blocks, plain_text_of_segments, RichBlock};

    let sample = concat!(
        "### 2. 💻 本地电脑控制\n",
        "- **文件与目录操作**: 查看日志 (`free -h`)\n",
        "> 💡 **关于安装 Hermes**: 说明文本\n",
        "---\n",
        "参考 [NousResearch](https://github.com/NousResearch/hermes-agent) 项目"
    );
    let blocks = parse_rich_blocks(sample, &[]);

    assert!(matches!(blocks.first(), Some(RichBlock::Heading { level: 3, .. })));
    if let RichBlock::Heading { segments, .. } = &blocks[0] {
        assert_eq!(plain_text_of_segments(segments), "2. 💻 本地电脑控制");
    }

    match &blocks[1] {
        RichBlock::ListItem { bullet, segments, .. } => {
            assert_eq!(bullet, "•");
            assert!(segments.iter().any(|s| s.flags.bold && s.text.contains("文件与目录操作")));
            assert!(segments.iter().any(|s| s.flags.code && s.text == "free -h"));
        }
        other => panic!("expected ListItem block, got {other:?}"),
    }

    match &blocks[2] {
        RichBlock::Blockquote { lines, .. } => {
            assert_eq!(lines.len(), 1);
            assert!(
                lines[0].iter().any(|s| s.flags.bold && s.text == "关于安装 Hermes"),
                "expected bold segment inside blockquote without literal asterisks: {lines:?}"
            );
            assert!(!plain_text_of_segments(&lines[0]).contains("**"));
        }
        other => panic!("expected Blockquote block, got {other:?}"),
    }

    assert!(matches!(blocks.get(3), Some(RichBlock::Divider)));

    match &blocks[4] {
        RichBlock::Paragraph(segs) => {
            assert!(
                segs.iter().any(|s| {
                    s.text == "NousResearch"
                        && s.flags.link_url.as_deref() == Some("https://github.com/NousResearch/hermes-agent")
                }),
                "expected parsed markdown link segment: {segs:?}"
            );
        }
        other => panic!("expected Paragraph block with link, got {other:?}"),
    }
}

#[test]
fn magpie_ai_providers_settings_search_main_and_studio_sync_work() {
    use gpui_telegram_client::net::hermes_client::{
        MagpieProviderDto, MagpieStateDto, MagpieSubscriptionDto,
    };
    use gpui_telegram_desktop::views::settings::{
        sections::SettingsMainSections, settings_search::SettingsSearch,
        settings_type::SettingsSectionType,
    };

    assert_eq!(SettingsSectionType::AiProviders.id_str(), "ai_providers");
    assert_eq!(
        SettingsSectionType::AiProviders.display_title_localized(true),
        "服务商"
    );
    assert_eq!(
        SettingsSectionType::AiProviders.breadcrumb_path(),
        vec![SettingsSectionType::Main, SettingsSectionType::AiProviders]
    );

    let mut search = SettingsSearch::with_default_entries();
    let zh_hits = search.search("供应商");
    assert!(
        zh_hits.iter().any(|r| r.section == SettingsSectionType::AiProviders),
        "expected Chinese query 供应商 to match AiProviders"
    );
    let en_hits = search.search("magpie");
    assert!(
        en_hits.iter().any(|r| r.section == SettingsSectionType::AiProviders),
        "expected English query magpie to match AiProviders"
    );

    let mut main = SettingsMainSections::new();
    main.sync_ai_providers_status(Some("请选择供应商".to_string()), true);
    assert!(main.needs_provider_selection);
    assert_eq!(main.ai_providers_summary.as_deref(), Some("请选择供应商"));

    let mut ws = GlobalTasksWorkspace::new();
    ws.sync_magpie_state(MagpieStateDto {
        active_provider: "deepseek".to_string(),
        active_model: "deepseek-chat".to_string(),
        needs_provider_selection: false,
        has_configured_provider: true,
        has_active_subscription: true,
        providers: vec![MagpieProviderDto {
            provider_id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            api_mode: "chat_completions".to_string(),
            default_model: "deepseek-chat".to_string(),
            is_active: true,
            enabled: true,
            ..Default::default()
        }],
        subscriptions: vec![MagpieSubscriptionDto {
            id: "sub-codex".to_string(),
            kind: "codex_chatgpt".to_string(),
            label: "Codex ChatGPT".to_string(),
            active: true,
            ..Default::default()
        }],
        routing_strategy: "smart".to_string(),
        ..Default::default()
    });
    assert_eq!(ws.active_api_mode, "chat_completions");
    assert!(!ws.status_summary.needs_provider_selection);
    assert!(ws.status_summary.has_active_subscription);
    assert!(ws.status_summary.has_configured_provider);
}

#[test]
fn test_local_fabushi_per_user_provider_hydration_and_isolation() {
    use bot_browser::e2ee_sync::AiOauthVaultBundle;
    use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
    use gpui_telegram_agent::hermes::providers::{ProviderConfig, ProviderRouter};
    use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::{
        build_ai_oauth_vault_bundle, has_usable_local_fabushi_provider_for_user,
        hydrate_ai_oauth_vault_bundle_into_fabushi_for_user,
        hydrate_local_subscriptions_into_fabushi_for_user,
    };
    use magpie_gateway::{
        ClientSubscriptionSyncItem, ProtocolKind, SubscriptionAccount, SubscriptionKind,
        VertexAdcCredentials,
    };
    use std::collections::BTreeMap;

    let mut headers = BTreeMap::new();
    headers.insert("x-goog-user-project".to_string(), "proj-123".to_string());
    let vertex_item = ClientSubscriptionSyncItem {
        account: SubscriptionAccount {
            id: "google_vertex:dev@example.com".to_string(),
            kind: SubscriptionKind::GoogleVertex,
            label: "Google Vertex AI (dev@example.com)".to_string(),
            email: Some("dev@example.com".to_string()),
            account_id: Some("proj-123".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: true,
            base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
            protocol: ProtocolKind::GeminiGenerateContent,
            default_model: Some("gemini-2.5-flash".to_string()),
            source_path: "/tmp/adc.json".to_string(),
            expires_at_ms: None,
            access_token: None,
            headers: headers.clone(),
        },
        access_token: None,
        raw_headers: headers,
        vertex_adc: Some(VertexAdcCredentials {
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            refresh_token: "rt".to_string(),
            quota_project_id: Some("proj-123".to_string()),
            account: Some("dev@example.com".to_string()),
        }),
        codex_oauth: None,
        region_id: Some("global".to_string()),
        available_models: Vec::new(),
    };

    let host = FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("ephemeral host");
    assert!(!has_usable_local_fabushi_provider_for_user(&host, Some("1001")));
    assert!(!has_usable_local_fabushi_provider_for_user(&host, Some("1002")));

    let hydrated = hydrate_local_subscriptions_into_fabushi_for_user(
        &host,
        &[vertex_item.clone()],
        Some("1001"),
    );
    assert_eq!(hydrated, 1);
    assert!(has_usable_local_fabushi_provider_for_user(&host, Some("1001")));
    assert!(
        !has_usable_local_fabushi_provider_for_user(&host, Some("1002")),
        "per-user hydration must isolate user 1002 from user 1001"
    );
    assert!(
        !has_usable_local_fabushi_provider_for_user(&host, None),
        "per-user hydration must not leak private subscriptions into platform base router"
    );
    assert!(!host.core_engine().resolve_providers(Some("1001")).needs_provider_selection());
    assert!(host.core_engine().resolve_providers(Some("1002")).needs_provider_selection());

    // Empty / placeholder-only providers_json bundle must return Ok(0) and stay unusable
    let placeholder_router = ProviderRouter::new();
    let placeholder_bundle = AiOauthVaultBundle {
        providers_json: serde_json::to_value(placeholder_router.snapshot()).unwrap(),
        subscriptions_json: json!([]),
        updated_at_ms: 1_700_000_000_000,
    };
    assert_eq!(
        hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
            &host,
            &placeholder_bundle,
            Some("1003")
        )
        .unwrap(),
        0
    );
    assert!(!has_usable_local_fabushi_provider_for_user(&host, Some("1003")));

    // Active subscription record stripped of token/ADC/OAuth credentials must return Ok(0) and stay unusable
    let mut stripped_state = placeholder_router.snapshot();
    stripped_state.subscriptions.push(SubscriptionAccount {
        id: "stripped_sub".to_string(),
        kind: SubscriptionKind::CodexChatGpt,
        label: "Stripped Sub".to_string(),
        email: None,
        account_id: None,
        plan: None,
        active: true,
        base_url: None,
        protocol: ProtocolKind::CodexResponses,
        default_model: None,
        source_path: String::new(),
        expires_at_ms: None,
        access_token: None,
        headers: BTreeMap::new(),
    });
    let stripped_bundle = AiOauthVaultBundle {
        providers_json: serde_json::to_value(&stripped_state).unwrap(),
        subscriptions_json: json!([]),
        updated_at_ms: 1_700_000_000_000,
    };
    assert_eq!(
        hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
            &host,
            &stripped_bundle,
            Some("1003")
        )
        .unwrap(),
        0
    );
    assert!(!has_usable_local_fabushi_provider_for_user(&host, Some("1003")));

    // API-key provider in providers_json (with empty subscriptions_json) for user "1003"
    let key_router = ProviderRouter::new();
    let cfg = ProviderConfig::openai_compatible(
        "deepseek_user_1003",
        "DeepSeek",
        "https://api.deepseek.com/v1",
        "deepseek-chat",
        "sk-test-1003",
    );
    key_router.register_provider(cfg);
    let _ = key_router.set_active("deepseek_user_1003", Some("deepseek-chat"));
    let api_key_bundle = build_ai_oauth_vault_bundle(&[], Some(&key_router)).unwrap();
    let cnt_1003 = hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
        &host,
        &api_key_bundle,
        Some("1003"),
    )
    .unwrap();
    assert!(cnt_1003 >= 1);
    assert!(has_usable_local_fabushi_provider_for_user(&host, Some("1003")));
    assert!(!has_usable_local_fabushi_provider_for_user(&host, Some("1004")));

    // Subscriptions bundle for user "1004"
    let sub_bundle = build_ai_oauth_vault_bundle(&[vertex_item], None).unwrap();
    let cnt_1004 =
        hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(&host, &sub_bundle, Some("1004"))
            .unwrap();
    assert_eq!(cnt_1004, 1);
    assert!(has_usable_local_fabushi_provider_for_user(&host, Some("1004")));
}

#[test]
fn test_bot_attach_webview_waits_for_inner_iframe_ready_and_rediscovers_tools() {
    spawn_headless_hermes_bugfix_test(
        "test_bot_attach_webview_waits_for_inner_iframe_ready_and_rediscovers_tools",
        "bot_attach_webview_iframe_ready",
    );
}

fn body_bot_attach_webview_waits_for_inner_iframe_ready_and_rediscovers_tools() {
    use gpui_telegram_desktop::components::bots::bot_attach_web_view::{
        BotAttachWebView, BotWebViewPendingAction,
    };
    use gpui_telegram_proto::miniapp_contracts::{
        MiniAppControlMode, MiniAppWebMcpCommand, MiniAppWebMcpResponse,
    };

    let mut wv = BotAttachWebView::new();
    wv.pending_webmcp_queue.push_back(MiniAppWebMcpCommand {
        request_id: "req_before_ready".to_string(),
        bot_username: "fabushi_bot".to_string(),
        short_name: "fabushi".to_string(),
        chat_id: 700_001,
        bot_user_id: 600_001,
        app_title: "全球法布施".to_string(),
        operation: "call_tool".to_string(),
        tool_name: Some("fabushi_send_dharma".to_string()),
        arguments: json!({ "sutra": "金刚经" }),
        status_message: None,
    });

    // 1. web_app_ready alone must NOT mark webmcp_page_ready = true
    wv.handle_bridge_event("web_app_ready", "{}");
    assert!(
        !wv.webmcp_page_ready,
        "web_app_ready must not flush WebMCP commands prematurely"
    );
    assert_eq!(wv.pending_webmcp_queue.len(), 1);

    // 2. host_wrapper_only: true must be ignored
    wv.handle_bridge_event(
        "__gpui_webmcp_page_ready",
        r#"{"host_wrapper_only":true,"url":"http://127.0.0.1:8080/miniapps/fabushi_bot/fabushi"}"#,
    );
    assert!(!wv.webmcp_page_ready);

    // 3. Inner iframe ready with tools sets readiness, populates discovered tools, and flushes queue
    wv.handle_bridge_event(
        "__gpui_webmcp_page_ready",
        &json!({
            "url": "http://127.0.0.1:18090/",
            "in_iframe": true,
            "tools": [
                {
                    "name": "fabushi_send_dharma",
                    "description": "Send Dharma",
                    "input_schema": { "type": "object" },
                    "read_only": false
                }
            ],
            "state_summary": { "kind": "fabushi", "wasm_ready": true }
        })
        .to_string(),
    );
    assert!(wv.webmcp_page_ready);
    assert!(wv.webmcp_iframe_ready);
    assert!(
        wv.webmcp_discovered_tools
            .iter()
            .any(|t| t.name == "fabushi_send_dharma"),
        "expected fabushi_send_dharma in webmcp_discovered_tools: {:?}",
        wv.webmcp_discovered_tools
    );
    assert!(
        wv.pending_webmcp_queue.is_empty(),
        "expected queued WebMCP command flushed in headless/test env"
    );
    wv.pending_actions.clear();

    // 4. gpui_webmcp_unready clears readiness/tools and marks webmcp_needs_rediscover
    wv.handle_bridge_event("gpui_webmcp_unready", r#"{"url":"http://127.0.0.1:18090/"}"#);
    assert!(!wv.webmcp_page_ready);
    assert!(!wv.webmcp_iframe_ready);
    assert!(wv.webmcp_discovered_tools.is_empty());
    assert!(wv.webmcp_needs_rediscover);

    // 5. gpui_webmcp_tools_changed updates tools dynamically
    wv.handle_bridge_event(
        "gpui_webmcp_tools_changed",
        &json!({
            "url": "http://127.0.0.1:18090/",
            "tools": [
                { "name": "get_state", "description": "State", "input_schema": {}, "read_only": true },
                { "name": "fabushi_switch_tab", "description": "Switch tab", "input_schema": {}, "read_only": false }
            ]
        })
        .to_string(),
    );
    assert!(wv.webmcp_page_ready);
    assert!(wv.webmcp_discovered_tools.iter().any(|t| t.name == "fabushi_switch_tab"));

    // 6. Internal __gpui_internal_discover_ webmcp_response updates tools without emitting WebMcpResponseReady
    let internal_resp = MiniAppWebMcpResponse {
        request_id: "__gpui_internal_discover_1".to_string(),
        ok: true,
        result: json!({ "count": 2 }),
        error: None,
        tools: vec![gpui_telegram_proto::miniapp_contracts::WebMcpToolDescriptor {
            name: "fabushi_send_dharma".to_string(),
            description: "Send Dharma".to_string(),
            input_schema: json!({}),
            read_only: false,
        }],
        state_summary: Some(json!({ "kind": "fabushi", "reloaded": true })),
        control_mode: MiniAppControlMode::AiControlling,
    };
    wv.handle_bridge_event(
        "webmcp_response",
        &serde_json::to_string(&internal_resp).unwrap(),
    );
    assert!(wv.webmcp_discovered_tools.iter().any(|t| t.name == "fabushi_send_dharma"));
    assert!(
        !wv.pending_actions
            .iter()
            .any(|a| matches!(a, BotWebViewPendingAction::WebMcpResponseReady(_))),
        "internal discover response must not emit BotWebViewPendingAction::WebMcpResponseReady"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_wkwebview_boot_js_cross_frame_and_async_tool_registration_contract() {
    use gpui_telegram_desktop::components::bots::macos_wkwebview::TELEGRAM_WEBVIEW_PROXY_BOOT_JS;

    assert!(TELEGRAM_WEBVIEW_PROXY_BOOT_JS.contains("gpui_webmcp_invoke"));
    assert!(TELEGRAM_WEBVIEW_PROXY_BOOT_JS.contains("gpui_webmcp_ready"));
    assert!(TELEGRAM_WEBVIEW_PROXY_BOOT_JS.contains("gpui_webmcp_unready"));
    assert!(TELEGRAM_WEBVIEW_PROXY_BOOT_JS.contains("gpui_webmcp_tools_changed"));
    assert!(TELEGRAM_WEBVIEW_PROXY_BOOT_JS.contains("waitForTool"));
    assert!(TELEGRAM_WEBVIEW_PROXY_BOOT_JS.contains("tools: (operation === 'discover' || !ok) ? listDescriptors() : []"));

    if let Ok(ver) = std::process::Command::new("node").arg("--version").output() {
        if ver.status.success() {
            let boot_js_lit = serde_json::to_string(TELEGRAM_WEBVIEW_PROXY_BOOT_JS).unwrap();
            let driver = format!(
                r#"
const assert = require('assert');
const vm = require('vm');
const bootJs = {boot_js_lit};

function createMockFrameEnv({{ isTop, hasTgAppFrame }}) {{
  const winListeners = {{}};
  const bridgeEvents = [];
  const parentMessages = [];
  const mockParent = isTop ? null : {{
    postMessage(msg, targetOrigin) {{ parentMessages.push({{ msg, targetOrigin }}); }}
  }};
  const doc = {{
    readyState: 'loading',
    title: 'Mock',
    getElementById(id) {{ return (hasTgAppFrame && id === 'tg-app-frame') ? {{ id: 'tg-app-frame' }} : null; }},
    querySelector() {{ return null; }},
    querySelectorAll() {{ return []; }}
  }};
  const win = {{
    location: {{ href: isTop ? 'http://127.0.0.1:8080/miniapps/fabushi_bot/fabushi' : 'http://127.0.0.1:18090/' }},
    top: null,
    parent: null,
    document: doc,
    webkit: {{
      messageHandlers: {{
        telegramWebviewProxy: {{
          postMessage(p) {{ bridgeEvents.push(p); }}
        }}
      }}
    }},
    addEventListener(type, fn) {{ (winListeners[type] = winListeners[type] || []).push(fn); }}
  }};
  win.top = isTop ? win : {{ isOuter: true }};
  win.parent = isTop ? win : mockParent;
  const ctx = vm.createContext({{
    window: win,
    document: doc,
    navigator: {{}},
    Event: function(t) {{ this.type = t; }},
    setTimeout,
    clearTimeout,
    Promise,
    Object,
    Array,
    String,
    Number,
    Date,
    JSON
  }});
  ctx.window.navigator = ctx.navigator;
  vm.runInContext(bootJs, ctx);
  return {{ ctx, win, doc, winListeners, bridgeEvents, parentMessages }};
}}

(async () => {{
  // 1. Top frame with #tg-app-frame suppresses premature __gpui_webmcp_page_ready on DOMContentLoaded
  const topEnv = createMockFrameEnv({{ isTop: true, hasTgAppFrame: true }});
  assert(
    topEnv.win.TelegramWebviewProxy && typeof topEnv.win.TelegramWebviewProxy.postEvent === 'function',
    'top frame must install window.TelegramWebviewProxy'
  );
  topEnv.doc.readyState = 'interactive';
  for (const fn of (topEnv.winListeners.DOMContentLoaded || [])) fn();
  assert(
    !topEnv.bridgeEvents.some(e => e.eventType === '__gpui_webmcp_page_ready'),
    'top frame with #tg-app-frame must suppress premature __gpui_webmcp_page_ready'
  );

  // 2. Child frame does NOT install window.TelegramWebviewProxy (prevents duplicate Telegram.WebApp bridging)
  const childEnv = createMockFrameEnv({{ isTop: false, hasTgAppFrame: false }});
  assert.strictEqual(
    childEnv.win.TelegramWebviewProxy,
    undefined,
    'non-top iframe must NOT define window.TelegramWebviewProxy'
  );
  // Simulate two-frame /js/telegram-web-app.js postToHost flow -> single native bridge event
  const sharedBridgeHits = [];
  const postToHostSim = (evType, evData) => {{
    if (childEnv.win.TelegramWebviewProxy && childEnv.win.TelegramWebviewProxy.postEvent) {{
      sharedBridgeHits.push({{ from: 'inner', eventType: evType }});
    }}
    if (childEnv.win.parent && childEnv.win.parent !== childEnv.win) {{
      childEnv.win.parent.postMessage({{ type: 'telegram_web_app_event', eventType: evType, eventData: evData }}, '*');
    }}
  }};
  postToHostSim('web_app_ready', {{}});
  // Simulate outer wrapper (bots_miniapps.rs) relaying telegram_web_app_event to topEnv.win.TelegramWebviewProxy
  for (const m of childEnv.parentMessages) {{
    if (m.msg && m.msg.type === 'telegram_web_app_event') {{
      topEnv.win.TelegramWebviewProxy.postEvent(m.msg.eventType, m.msg.eventData);
      // Outer BOOT_JS message listener ignores telegram_web_app_event when hasTgAppFrame() is true
      for (const fn of (topEnv.winListeners.message || [])) {{
        fn({{ source: childEnv.win, data: m.msg }});
      }}
    }}
  }}
  const readyHits = topEnv.bridgeEvents.filter(e => e.eventType === 'web_app_ready').length + sharedBridgeHits.length;
  assert.strictEqual(readyHits, 1, 'two-frame Telegram.WebApp.ready() must emit exactly 1 native bridge hit, got ' + readyHits);

  childEnv.doc.readyState = 'complete';
  for (const fn of (childEnv.winListeners.DOMContentLoaded || [])) fn();
  assert(
    childEnv.parentMessages.some(m => m.msg && m.msg.type === 'gpui_webmcp_ready'),
    'child frame must post gpui_webmcp_ready to window.parent'
  );
  assert(
    !childEnv.bridgeEvents.some(e => e.eventType === '__gpui_webmcp_page_ready'),
    'child frame must not bypass outer wrapper with direct __gpui_webmcp_page_ready'
  );

  const replyMsgs = [];
  const mockSource = {{ postMessage(msg, targetOrigin) {{ replyMsgs.push({{ msg, targetOrigin }}); }} }};
  const onChildMsg = (childEnv.winListeners.message || []).find(fn => {{
    // invoke all listeners on gpui_webmcp_invoke
    return true;
  }});
  for (const fn of (childEnv.winListeners.message || [])) {{
    fn({{
      source: mockSource,
      data: {{
        type: 'gpui_webmcp_invoke',
        request_id: 'req_async',
        operation: 'call_tool',
        tool_name: 'fabushi_send_dharma',
        arguments: {{ sutra: '金刚经' }}
      }}
    }});
  }}
  // Register tool asynchronously 50ms later
  setTimeout(() => {{
    childEnv.ctx.navigator.modelContext.registerTool({{
      name: 'fabushi_send_dharma',
      description: 'Send Dharma',
      inputSchema: {{ type: 'object' }},
      execute: (args) => ({{ accepted: true, sutra: args.sutra }})
    }});
  }}, 50);

  await new Promise(r => setTimeout(r, 150));
  assert.strictEqual(replyMsgs.length, 1, 'must deliver exactly one gpui_webmcp_response');
  assert.strictEqual(replyMsgs[0].msg.type, 'gpui_webmcp_response');
  assert.strictEqual(replyMsgs[0].msg.payload.request_id, 'req_async');
  assert.strictEqual(replyMsgs[0].msg.payload.ok, true);
  assert.strictEqual(replyMsgs[0].msg.payload.result.sutra, '金刚经');
  assert(
    childEnv.parentMessages.some(m => m.msg && m.msg.type === 'gpui_webmcp_tools_changed'),
    'registerTool must emit gpui_webmcp_tools_changed'
  );
  assert(
    !childEnv.bridgeEvents.some(e => e.eventType === 'webmcp_response'),
    'postMessage-invoked child frame must not double-post webmcp_response directly to bridge'
  );

  // 3. Failed call_tool returns currently registered tool descriptors in payload.tools
  for (const fn of (childEnv.winListeners.message || [])) {{
    fn({{
      source: mockSource,
      data: {{
        type: 'gpui_webmcp_invoke',
        request_id: 'req_missing',
        operation: 'call_tool',
        tool_name: '',
        arguments: {{}}
      }}
    }});
  }}
  await new Promise(r => setTimeout(r, 30));
  assert.strictEqual(replyMsgs.length, 2);
  assert.strictEqual(replyMsgs[1].msg.payload.ok, false);
  assert(
    replyMsgs[1].msg.payload.tools.some(t => t.name === 'fabushi_send_dharma'),
    'failed call_tool must include registered tools in response.tools'
  );
  process.exit(0);
}})().catch(err => {{ console.error(err); process.exit(1); }});
"#
            );
            let out = std::process::Command::new("node")
                .arg("-e")
                .arg(&driver)
                .output()
                .expect("run node boot js simulation");
            assert!(
                out.status.success(),
                "node boot js simulation failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

const HEADLESS_HERMES_BUGFIX_CHILD_ENV: &str = "GPUI_HERMES_BUGFIX_HEADLESS_CHILD";
const HEADLESS_HERMES_BUGFIX_TARGET_ENV: &str = "GPUI_HERMES_BUGFIX_HEADLESS_TARGET";

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_HEADLESS_HERMES_BUGFIX_ENTRY: extern "C" fn() = macos_mod_init_hermes_bugfix_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_hermes_bugfix_entry() {
    if std::env::var(HEADLESS_HERMES_BUGFIX_CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(HEADLESS_HERMES_BUGFIX_TARGET_ENV) {
            dispatch_headless_hermes_bugfix_target(&target);
        }
    }
}

fn dispatch_headless_hermes_bugfix_target(target: &str) -> ! {
    use futures::FutureExt;
    use gpui::{App, Application, AsyncApp};
    use gpui_telegram_desktop::theme::TelegramAssetSource;
    use std::future::Future;
    use std::pin::Pin;

    if target == "bot_attach_webview_iframe_ready" {
        body_bot_attach_webview_waits_for_inner_iframe_ready_and_rediscovers_tools();
        std::process::exit(0);
    }

    enum HeadlessRunner {
        Sync(fn(&mut App)),
        Async(Box<dyn FnOnce(AsyncApp) -> Pin<Box<dyn Future<Output = ()>>> + Send>),
    }

    let runner = match target {
        "resolve_fabushi_bot_target_owner_guard" => {
            HeadlessRunner::Sync(body_resolve_fabushi_bot_target_owner_guard)
        }
        "local_fabushi_provider_error_card_cta" => {
            HeadlessRunner::Sync(body_local_fabushi_provider_error_card_cta)
        }
        "webmcp_pending_commands_drain_and_no_pip_flash" => {
            HeadlessRunner::Sync(body_webmcp_pending_commands_drain_and_no_pip_flash)
        }
        "webmcp_alias_and_concurrent_launch_fence" => HeadlessRunner::Async(Box::new(|app| {
            Box::pin(body_webmcp_alias_and_concurrent_launch_fence(app))
        })),
        "webmcp_reused_overlay_switch_and_reload_isolation" => HeadlessRunner::Async(Box::new(|app| {
            Box::pin(body_webmcp_reused_overlay_switch_and_reload_isolation(app))
        })),
        "user_switch_clears_bot_browser_and_magpie" => {
            HeadlessRunner::Sync(body_user_switch_clears_bot_browser_and_magpie)
        }
        "stage_model_picker_live_sync_and_persistence" => HeadlessRunner::Async(Box::new(|app| {
            Box::pin(body_stage_model_picker_live_sync_and_persistence(app))
        })),
        other => {
            eprintln!("unknown headless hermes bugfix target: {other}");
            std::process::exit(101);
        }
    };
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| match runner {
                HeadlessRunner::Sync(body) => {
                    let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        body(cx);
                    }));
                    match inner {
                        Ok(()) => std::process::exit(0),
                        Err(e) => {
                            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = e.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "panic".to_string()
                            };
                            eprintln!("headless hermes bugfix test panicked: {msg}");
                            std::process::exit(102);
                        }
                    }
                }
                HeadlessRunner::Async(async_fn) => {
                    cx.spawn(async move |app: &mut AsyncApp| {
                        let app_clone = app.clone();
                        let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                            .catch_unwind()
                            .await;
                        match res {
                            Ok(()) => std::process::exit(0),
                            Err(e) => {
                                let msg = if let Some(s) = e.downcast_ref::<&str>() {
                                    (*s).to_string()
                                } else if let Some(s) = e.downcast_ref::<String>() {
                                    s.clone()
                                } else {
                                    "panic".to_string()
                                };
                                eprintln!("headless hermes bugfix async test panicked: {msg}");
                                std::process::exit(102);
                            }
                        }
                    })
                    .detach();
                }
            });
    }));
    match outer {
        Ok(()) => std::process::exit(0),
        Err(_) => std::process::exit(103),
    }
}

fn spawn_headless_hermes_bugfix_test(test_name: &str, target_key: &str) {
    if std::env::var(HEADLESS_HERMES_BUGFIX_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_hermes_bugfix_target(target_key);
    }
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .env(HEADLESS_HERMES_BUGFIX_CHILD_ENV, "1")
        .env(HEADLESS_HERMES_BUGFIX_TARGET_ENV, target_key)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless hermes bugfix child");
    assert!(
        out.status.success(),
        "headless hermes bugfix child '{target_key}' failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn body_resolve_fabushi_bot_target_owner_guard(cx: &mut gpui::App) {
    use gpui::{AppContext, Entity};
    use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::hydrate_local_subscriptions_into_fabushi_for_user;
    use gpui_telegram_desktop::views::AppShell;
    use gpui_telegram_proto::miniapp_contracts::{
        BotPublicProfileDto, BotRegistryEntry, FabushiBotSummaryDto,
    };
    use magpie_gateway::{
        ClientSubscriptionSyncItem, ProtocolKind, SubscriptionAccount, SubscriptionKind,
        VertexAdcCredentials,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let viewer_uid = 200_001u64;
    let other_uid = 999_888i64;
    let auth = AuthResponse {
        token: "tok_owner_guard".to_string(),
        session_id: "sess_owner_guard".to_string(),
        user_id: viewer_uid,
        username: "viewer_user".to_string(),
        first_name: Some("Viewer".to_string()),
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("ephemeral fabushi"),
    );

    let owned_chat_id = 720_001u64;
    let owned_bot_id = 620_001i64;
    let foreign_prof_chat_id = 720_002u64;
    let foreign_prof_bot_id = 620_002i64;
    let foreign_fab_chat_id = 720_003u64;
    let foreign_fab_bot_id = 620_003i64;

    let shell: Entity<AppShell> = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.session_sync.begin_session(
            auth.token.clone(),
            auth.session_id.clone(),
            auth.user_id,
            auth.username.clone(),
        );
        s.is_authenticated = true;
        s.attach_local_fabushi_host(Arc::clone(&host));

        s.session_sync.chat_peer_user_ids.insert(owned_chat_id, owned_bot_id as u64);
        s.session_sync.chat_peer_user_ids.insert(foreign_prof_chat_id, foreign_prof_bot_id as u64);
        s.session_sync.chat_peer_user_ids.insert(foreign_fab_chat_id, foreign_fab_bot_id as u64);

        s.miniapps.fabushi_bots = vec![
            FabushiBotSummaryDto {
                bot_user_id: owned_bot_id,
                owner_user_id: viewer_uid as i64,
                agent_id: "owned-fab".to_string(),
                username: "OwnedFabBot".to_string(),
                display_name: "Owned Fabushi".to_string(),
                companion_chat_id: Some(owned_chat_id as i64),
                coding_agent: None,
                ..Default::default()
            },
            FabushiBotSummaryDto {
                bot_user_id: foreign_fab_bot_id,
                owner_user_id: other_uid,
                agent_id: "foreign-fab".to_string(),
                username: "ForeignFabBot".to_string(),
                display_name: "Foreign Fabushi".to_string(),
                companion_chat_id: Some(foreign_fab_chat_id as i64),
                coding_agent: None,
                ..Default::default()
            },
        ];

        s.miniapps.bot_profiles_cache.insert(
            "foreignproffabbot".to_string(),
            BotPublicProfileDto {
                bot: BotRegistryEntry {
                    bot_user_id: foreign_prof_bot_id,
                    owner_user_id: other_uid,
                    agent_id: "foreign-prof".to_string(),
                    username: "ForeignProfFabBot".to_string(),
                    display_name: "Foreign Fabushi Portal".to_string(),
                    is_fabushi_bot: true,
                    coding_agent: None,
                    ..Default::default()
                },
                main_app: None,
                apps: Vec::new(),
                existing_chat_id: None,
                has_started_chat: false,
                allows_write_to_pm: false,
                app_tos_accepted: false,
                is_blocked: false,
                is_muted: false,
            },
        );

        let third_party_fabushi_chat_id = 600_000_999u64;
        let weather_bot_chat_id = 600_000_888u64;
        let chats = vec![
            ChatSummary {
                chat_id: owned_chat_id,
                title: "Owned Fabushi".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_000),
            },
            ChatSummary {
                chat_id: foreign_prof_chat_id,
                title: "Foreign Fabushi Portal".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_000),
            },
            ChatSummary {
                chat_id: foreign_fab_chat_id,
                title: "Foreign Fabushi".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_000),
            },
            ChatSummary {
                chat_id: third_party_fabushi_chat_id,
                title: "Fabushi Pro".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_000),
            },
            ChatSummary {
                chat_id: weather_bot_chat_id,
                title: "Weather Helper".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_000),
            },
        ];
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &chats);
                    select_authoritative_dialog(sb, st, insp, owned_chat_id);
                });
            });
        });
        s
    });

    shell.update(cx, |s, cx| {
        assert!(
            s.resolve_fabushi_bot_target(owned_chat_id, cx).is_some(),
            "owned Fabushi bot must resolve"
        );
        assert!(
            s.resolve_fabushi_bot_target(foreign_fab_chat_id, cx).is_none(),
            "non-owned Fabushi bot in fabushi_bots must fall back to server"
        );
        assert!(
            s.resolve_fabushi_bot_target(foreign_prof_chat_id, cx).is_none(),
            "non-owned Fabushi bot in bot_profiles_cache must fall back to server"
        );
        assert!(
            s.resolve_fabushi_bot_target(600_000_999, cx).is_none(),
            "third-party bot named 'Fabushi Pro' with peer_uid=None must NOT hijack into 777000"
        );
        s.stage.update(cx, |st, _| {
            st.active_chat_id = Some(owned_chat_id);
            st.chat_title = "Fabushi AI".to_string();
        });
        assert!(
            s.resolve_fabushi_bot_target(600_000_888, cx).is_none(),
            "non-active 'Weather Helper' bot must NOT inherit active stage title 'Fabushi AI'"
        );
        assert!(
            !s.can_run_local_fabushi_turn(cx),
            "can_run_local_fabushi_turn must be false before per-user hydration"
        );

        let mut headers = BTreeMap::new();
        headers.insert("x-goog-user-project".to_string(), "proj-1".to_string());
        let item = ClientSubscriptionSyncItem {
            account: SubscriptionAccount {
                id: "google_vertex:viewer@example.com".to_string(),
                kind: SubscriptionKind::GoogleVertex,
                label: "Vertex".to_string(),
                email: Some("viewer@example.com".to_string()),
                account_id: Some("proj-1".to_string()),
                plan: Some("vertex_adc".to_string()),
                active: true,
                base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
                protocol: ProtocolKind::GeminiGenerateContent,
                default_model: Some("gemini-2.5-flash".to_string()),
                source_path: "/tmp/adc.json".to_string(),
                expires_at_ms: None,
                access_token: None,
                headers: headers.clone(),
            },
            access_token: None,
            raw_headers: headers,
            vertex_adc: Some(VertexAdcCredentials {
                client_id: "cid".to_string(),
                client_secret: "csec".to_string(),
                refresh_token: "rt".to_string(),
                quota_project_id: Some("proj-1".to_string()),
                account: Some("viewer@example.com".to_string()),
            }),
            codex_oauth: None,
            region_id: Some("global".to_string()),
            available_models: Vec::new(),
        };
        hydrate_local_subscriptions_into_fabushi_for_user(
            &host,
            &[item],
            Some(&viewer_uid.to_string()),
        );
        assert!(
            s.can_run_local_fabushi_turn(cx),
            "can_run_local_fabushi_turn must be true after per-user hydration"
        );
    });
}

fn body_local_fabushi_provider_error_card_cta(cx: &mut gpui::App) {
    use gpui::{AppContext, Entity};
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
    use gpui_telegram_desktop::views::stage::cards::BOT_ASSIST_LOCAL_MSG_ID_BASE;
    use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
    use gpui_telegram_desktop::views::AppShell;
    use std::sync::Arc;

    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let viewer_uid = 300_001u64;
    let auth = AuthResponse {
        token: "tok_err_card".to_string(),
        session_id: "sess_err_card".to_string(),
        user_id: viewer_uid,
        username: "err_card_user".to_string(),
        first_name: Some("ErrCard".to_string()),
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let chat_id = 730_001u64;
    let bot_user_id = 630_001i64;
    let shell: Entity<AppShell> = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.session_sync.begin_session(
            auth.token.clone(),
            auth.session_id.clone(),
            auth.user_id,
            auth.username.clone(),
        );
        s.is_authenticated = true;
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(
                        sb,
                        st,
                        insp,
                        &[ChatSummary {
                            chat_id,
                            title: "Fabushi AI".to_string(),
                            chat_type: 4,
                            last_message: None,
                            unread_count: 0,
                            is_group: Some(false),
                            is_bot: Some(true),
                            avatar_color: Some(0x2AABEE),
                            updated_at: Some(1_700_000_000_000),
                        }],
                    );
                    select_authoritative_dialog(sb, st, insp, chat_id);
                });
            });
        });
        s
    });

    let long_err = "PROVIDER_ERROR: Configuration error: No AI subscription or provider configured in user router 300001; needs_provider_selection=true; checked providers=[local_hermes] and subscriptions=[] with allow_local_fallback=false and extra verbose diagnostic stack trace payload padding to exceed 250 characters easily.";
    assert!(long_err.len() > 250);

    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, _| {
            sb.set_typing(chat_id, Some("Thinking...".to_string()));
        });
        s.apply_local_fabushi_turn_error_to_shell(
            chat_id,
            bot_user_id,
            "Fabushi AI",
            long_err,
            cx,
        );

        let st = s.stage.read(cx);
        let card = st
            .messages
            .last()
            .expect("error card inserted into stage messages");
        assert!(card.id >= BOT_ASSIST_LOCAL_MSG_ID_BASE);
        assert_eq!(card.direction, MessageDirection::Incoming);
        let kb = card.bot_keyboard.as_ref().expect("inline CTA keyboard attached");
        let cta_btn = kb[0][0].clone();
        assert_eq!(
            cta_btn.button_type,
            InlineButtonType::Callback("hermes:open_ai_providers".to_string())
        );

        let toast = s
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .expect("concise toast enqueued");
        assert!(toast.text.chars().count() <= 80, "toast too long: {}", toast.text);
        assert!(!toast.text.contains('\n'), "toast must be single-line");
        assert!(
            !toast.text.contains("PROVIDER_ERROR"),
            "toast must not dump raw PROVIDER_ERROR trace: {}",
            toast.text
        );

        let typing = s
            .sidebar
            .read(cx)
            .dialogs
            .iter()
            .find(|d| d.id == chat_id)
            .and_then(|d| d.typing_indicator.clone());
        assert!(typing.is_none(), "typing indicator must be cleared");

        let card_id = card.id;
        s.handle_stage_bot_button(card_id, cta_btn.button_type, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::AiProviders);
    });
}

#[test]
fn test_resolve_fabushi_bot_target_owner_guard_and_server_fallback() {
    spawn_headless_hermes_bugfix_test(
        "test_resolve_fabushi_bot_target_owner_guard_and_server_fallback",
        "resolve_fabushi_bot_target_owner_guard",
    );
}

#[test]
fn test_local_fabushi_provider_error_inline_card_toast_and_cta_navigation() {
    use gpui_telegram_desktop::views::AppShell;

    let long_trace = "PROVIDER_ERROR: No AI subscription or provider configured; needs_provider_selection=true; verbose config dump=".to_string() + &"x".repeat(220);
    let (card_zh, kb_zh, toast_zh) =
        AppShell::build_local_fabushi_error_card(&long_trace, true, false);
    assert!(card_zh.contains("未配置可用的 AI 供应商"), "got {card_zh}");
    assert!(!card_zh.contains("PROVIDER_ERROR:"), "got {card_zh}");
    assert_eq!(
        kb_zh.as_ref().unwrap()[0][0].button_type,
        InlineButtonType::Callback("hermes:open_ai_providers".to_string())
    );
    assert!(toast_zh.chars().count() <= 80, "got {toast_zh}");
    assert!(!toast_zh.contains("verbose config dump"), "got {toast_zh}");

    let ansi_err = "\x1b[31mPROVIDER_ERROR:\x1b[0m \x1b[1;33mNo configured provider\x1b[0m";
    let (card_ansi, kb_ansi, _) = AppShell::build_local_fabushi_error_card(ansi_err, false, false);
    assert!(card_ansi.contains("No configured provider"), "got {card_ansi}");
    assert!(!card_ansi.contains('\x1b'), "ANSI escape leaked into card: {card_ansi:?}");
    assert!(!card_ansi.contains("[31m"), "ANSI CSI fragment leaked into card: {card_ansi:?}");
    assert!(kb_ansi.is_some());

    spawn_headless_hermes_bugfix_test(
        "test_local_fabushi_provider_error_inline_card_toast_and_cta_navigation",
        "local_fabushi_provider_error_card_cta",
    );
}

fn body_webmcp_pending_commands_drain_and_no_pip_flash(cx: &mut gpui::App) {
    use gpui::{AppContext, Entity};
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::components::bots::bot_attach_web_view::{
        AttachBotEntry, BotAttachWebView,
    };
    use gpui_telegram_desktop::views::AppShell;
    use gpui_telegram_proto::miniapp_contracts::MiniAppWebMcpCommand;
    use std::sync::Arc;

    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();
    client.set_auth_session(AuthResponse {
        token: "tok_webmcp_drain".to_string(),
        session_id: "sess_webmcp_drain".to_string(),
        user_id: 400_001,
        username: "webmcp_user".to_string(),
        first_name: None,
        created_at: None,
        expires_at: None,
    });

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    let ov = cx.new(|_| {
        let mut w = BotAttachWebView::new();
        w.bot_username = "fabushi_app_bot".to_string();
        w.active_bot = Some(AttachBotEntry {
            bot_id: 777_001,
            name: "Dharma".to_string(),
            short_name: "dharma".to_string(),
            allowed_peer_types: 0x0F,
            in_main_menu: true,
            in_attach_menu: true,
            disclaimer_required: false,
            request_write_access: false,
        });
        w.is_loading = true;
        w.is_open = false;
        w.is_floating_pip = true;
        w
    });

    let cmd_match = MiniAppWebMcpCommand {
        request_id: "req-1".to_string(),
        bot_username: "fabushi_app_bot".to_string(),
        short_name: "dharma".to_string(),
        chat_id: 100,
        bot_user_id: 777_001,
        app_title: "Dharma".to_string(),
        operation: "discover".to_string(),
        tool_name: None,
        arguments: serde_json::json!({}),
        status_message: None,
    };
    let cmd_other = MiniAppWebMcpCommand {
        request_id: "req-2".to_string(),
        bot_username: "other_bot".to_string(),
        short_name: "other_app".to_string(),
        chat_id: 101,
        bot_user_id: 777_002,
        app_title: "Other".to_string(),
        operation: "discover".to_string(),
        tool_name: None,
        arguments: serde_json::json!({}),
        status_message: None,
    };

    shell.update(cx, |s, cx| {
        s.miniapps.miniapp_overlay = Some(ov.clone());
        s.miniapps.is_miniapp_minimized = true;
        s.miniapps.pending_webmcp_commands.push_back(cmd_match.clone());
        s.miniapps.pending_webmcp_commands.push_back(cmd_other.clone());

        // 1. launch_miniapp_for_webmcp while w.is_loading == true on existing context must NOT flash w.is_open = true
        s.launch_miniapp_for_webmcp(
            MiniAppWebMcpCommand {
                request_id: "req-loading".to_string(),
                ..cmd_match.clone()
            },
            cx,
        );
        assert!(
            !ov.read(cx).is_open,
            "non-takeover WebMCP on loading existing context must stay in PiP without flashing open"
        );
        assert!(ov.read(cx).is_floating_pip);
        assert!(s.miniapps.is_miniapp_minimized);

        // 2. try_restore_running_miniapp drains matching pending_webmcp_commands, keeps non-matching,
        //    and always restores out of PiP even with non-takeover queued commands.
        let restored = s.try_restore_running_miniapp("fabushi_app_bot", "dharma", None, None, cx);
        assert!(restored);
        assert!(
            ov.read(cx).is_open && !ov.read(cx).is_floating_pip && !s.miniapps.is_miniapp_minimized,
            "try_restore_running_miniapp must restore out of PiP even when non-takeover WebMCP commands were queued"
        );
        assert_eq!(s.miniapps.pending_webmcp_commands.len(), 1);
        assert_eq!(
            s.miniapps.pending_webmcp_commands.front().unwrap().request_id,
            "req-2"
        );

        // 3. in_flight_same_app on an already-open loading overlay must NOT set is_miniapp_minimized = true
        s.session_sync.begin_session(
            "tok_webmcp_drain".to_string(),
            "sess_webmcp_drain".to_string(),
            400_001,
            "webmcp_user".to_string(),
        );
        s.is_authenticated = true;
        s.miniapps.miniapp_overlay = None;
        s.launch_miniapp_with_url(
            "open_loading_bot",
            "dharma",
            Some(102),
            gpui_telegram_proto::miniapp_contracts::MiniAppLaunchMode::CompactSheet,
            None,
            None,
            cx,
        );
        let open_ov = s
            .miniapps
            .miniapp_overlay
            .clone()
            .expect("open loading overlay created");
        assert!(open_ov.read(cx).is_open);
        assert!(open_ov.read(cx).is_loading);
        s.launch_miniapp_for_webmcp(
            MiniAppWebMcpCommand {
                request_id: "req-open-inflight".to_string(),
                bot_username: "open_loading_bot".to_string(),
                short_name: "dharma".to_string(),
                chat_id: 102,
                bot_user_id: 777_003,
                app_title: "Dharma".to_string(),
                operation: "discover".to_string(),
                tool_name: None,
                arguments: serde_json::json!({}),
                status_message: None,
            },
            cx,
        );
        assert!(
            open_ov.read(cx).is_open && !s.miniapps.is_miniapp_minimized,
            "in_flight_same_app must preserve is_open = true and not minimize an already-open overlay"
        );
    });
}

async fn spawn_mock_miniapp_launch_server() -> String {
    use gpui_telegram_proto::miniapp_contracts::{
        BotRegistryEntry, MiniAppEntry, MiniAppLaunchContext, MiniAppLaunchMode,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut total = 0usize;
                let header_end;
                loop {
                    let Ok(n) = sock.read(&mut buf[total..]).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    total += n;
                    if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = pos + 4;
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let first_line = head.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let _method = parts.next().unwrap_or("GET");
                let path = parts.next().unwrap_or("/").to_string();

                let (status, body) = if path == "/api/v1/miniapps/alias_bot/app/launch" {
                    let ctx = MiniAppLaunchContext {
                        app: MiniAppEntry {
                            app_id: "app-alias".to_string(),
                            bot_user_id: 880_001,
                            bot_username: "alias_bot".to_string(),
                            short_name: "dharma".to_string(),
                            title: "Dharma Alias".to_string(),
                            app_tos_accepted: true,
                            ..Default::default()
                        },
                        companion_bot: BotRegistryEntry {
                            bot_user_id: 880_001,
                            username: "alias_bot".to_string(),
                            display_name: "Alias Bot".to_string(),
                            ..Default::default()
                        },
                        companion_chat_id: 9001,
                        launch_mode: MiniAppLaunchMode::CompactSheet,
                        ..Default::default()
                    };
                    (200u16, serde_json::to_string(&ctx).unwrap())
                } else if path == "/api/v1/miniapps/race_bot/app_a_fail/launch" {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    (500u16, r#"{"error":"simulated launch failure"}"#.to_string())
                } else if path == "/api/v1/miniapps/race_bot/app_stale_ok/launch" {
                    tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                    let ctx = MiniAppLaunchContext {
                        app: MiniAppEntry {
                            app_id: "app-stale".to_string(),
                            bot_user_id: 880_002,
                            bot_username: "race_bot".to_string(),
                            short_name: "app_stale_ok".to_string(),
                            title: "Stale App".to_string(),
                            app_tos_accepted: true,
                            ..Default::default()
                        },
                        companion_bot: BotRegistryEntry {
                            bot_user_id: 880_002,
                            username: "race_bot".to_string(),
                            display_name: "Race Bot".to_string(),
                            ..Default::default()
                        },
                        companion_chat_id: 9002,
                        launch_mode: MiniAppLaunchMode::CompactSheet,
                        ..Default::default()
                    };
                    (200u16, serde_json::to_string(&ctx).unwrap())
                } else if path == "/api/v1/miniapps/race_bot/app_b/launch"
                    || path == "/api/v1/miniapps/race_bot/app_winner/launch"
                {
                    let short = if path.contains("app_b") {
                        "app_b"
                    } else {
                        "app_winner"
                    };
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    let ctx = MiniAppLaunchContext {
                        app: MiniAppEntry {
                            app_id: format!("id-{short}"),
                            bot_user_id: 880_002,
                            bot_username: "race_bot".to_string(),
                            short_name: short.to_string(),
                            title: format!("Title {short}"),
                            app_tos_accepted: true,
                            ..Default::default()
                        },
                        companion_bot: BotRegistryEntry {
                            bot_user_id: 880_002,
                            username: "race_bot".to_string(),
                            display_name: "Race Bot".to_string(),
                            ..Default::default()
                        },
                        companion_chat_id: 9002,
                        launch_mode: MiniAppLaunchMode::CompactSheet,
                        ..Default::default()
                    };
                    (200u16, serde_json::to_string(&ctx).unwrap())
                } else {
                    (200u16, r#"{"items":[]}"#.to_string())
                };
                let reason = if status == 200 { "OK" } else { "Internal Server Error" };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });

    format!("http://{addr}")
}

async fn body_webmcp_alias_and_concurrent_launch_fence(mut cx: gpui::AsyncApp) {
    use gpui::{AppContext, Entity};
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::views::AppShell;
    use gpui_telegram_proto::miniapp_contracts::MiniAppWebMcpCommand;
    use std::sync::Arc;
    use std::time::Duration;

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();
    let base_url = rt.block_on(spawn_mock_miniapp_launch_server());

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let auth = AuthResponse {
        token: "tok_alias_race".to_string(),
        session_id: "sess_alias_race".to_string(),
        user_id: 400_009,
        username: "alias_race_user".to_string(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let shell: Entity<AppShell> = cx
        .update(|app| {
            app.new(|cx| {
                let mut s = AppShell::new_with_client(client.clone(), cx);
                s.session_sync.begin_session(
                    auth.token.clone(),
                    auth.session_id.clone(),
                    auth.user_id,
                    auth.username.clone(),
                );
                s.is_authenticated = true;
                s
            })
        })
        .expect("create AppShell");

    // 1. Alias short_name ("app" -> server "dharma") drains pending_webmcp_commands and executes
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req-alias-1".to_string(),
                    bot_username: "alias_bot".to_string(),
                    short_name: "app".to_string(),
                    chat_id: 9001,
                    bot_user_id: 880_001,
                    app_title: "Dharma".to_string(),
                    operation: "discover".to_string(),
                    tool_name: None,
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                cx,
            );
        })
        .unwrap();

    for _ in 0..60 {
        cx.background_executor().timer(Duration::from_millis(20)).await;
        let done = shell
            .read_with(&cx, |s, cx| {
                s.miniapps.pending_webmcp_commands.is_empty()
                    && s.miniapps
                        .miniapp_overlay
                        .as_ref()
                        .is_some_and(|ov| ov.read(cx).last_webmcp_state_summary.is_some())
            })
            .unwrap();
        if done {
            break;
        }
    }
    shell
        .read_with(&cx, |s, cx| {
            assert!(
                s.miniapps.pending_webmcp_commands.is_empty(),
                "alias short_name 'app' resolved to 'dharma' must drain pending_webmcp_commands"
            );
            let ov = s
                .miniapps
                .miniapp_overlay
                .as_ref()
                .expect("alias overlay must remain present");
            let w = ov.read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.short_name.as_str()),
                Some("dharma")
            );
            assert!(
                w.last_webmcp_state_summary.is_some(),
                "alias command must be dispatched to overlay"
            );
        })
        .unwrap();

    // 2. Concurrent /launch: app_a_fail (120ms Err(500)) starts, then app_b (25ms Ok(200)) starts & opens;
    //    app_a_fail's late Err must NOT close or nullify app_b's miniapp_overlay.
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req-fail-a".to_string(),
                    bot_username: "race_bot".to_string(),
                    short_name: "app_a_fail".to_string(),
                    chat_id: 9002,
                    bot_user_id: 880_002,
                    app_title: "Fail A".to_string(),
                    operation: "discover".to_string(),
                    tool_name: None,
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                cx,
            );
        })
        .unwrap();
    cx.background_executor().timer(Duration::from_millis(15)).await;
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req-ok-b".to_string(),
                    bot_username: "race_bot".to_string(),
                    short_name: "app_b".to_string(),
                    chat_id: 9002,
                    bot_user_id: 880_002,
                    app_title: "App B".to_string(),
                    operation: "discover".to_string(),
                    tool_name: None,
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                cx,
            );
        })
        .unwrap();

    cx.background_executor().timer(Duration::from_millis(220)).await;
    shell
        .read_with(&cx, |s, cx| {
            let ov = s
                .miniapps
                .miniapp_overlay
                .as_ref()
                .expect("app_b miniapp_overlay must remain intact after stale app_a Err");
            let w = ov.read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.short_name.as_str()),
                Some("app_b")
            );
            assert!(w.launch_context.is_some(), "app_b launch_context must stay loaded");
            assert!(w.last_webmcp_state_summary.is_some());
            assert!(s.miniapps.pending_webmcp_commands.is_empty());
        })
        .unwrap();

    // 3. Concurrent /launch: app_stale_ok (140ms Ok(200)) starts, then app_winner (25ms Ok(200)) opens;
    //    app_stale_ok's late Ok must NOT overwrite app_winner's miniapp_overlay.
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req-stale-ok".to_string(),
                    bot_username: "race_bot".to_string(),
                    short_name: "app_stale_ok".to_string(),
                    chat_id: 9002,
                    bot_user_id: 880_002,
                    app_title: "Stale".to_string(),
                    operation: "discover".to_string(),
                    tool_name: None,
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                cx,
            );
        })
        .unwrap();
    cx.background_executor().timer(Duration::from_millis(15)).await;
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req-winner".to_string(),
                    bot_username: "race_bot".to_string(),
                    short_name: "app_winner".to_string(),
                    chat_id: 9002,
                    bot_user_id: 880_002,
                    app_title: "Winner".to_string(),
                    operation: "discover".to_string(),
                    tool_name: None,
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                cx,
            );
        })
        .unwrap();

    cx.background_executor().timer(Duration::from_millis(240)).await;
    shell
        .read_with(&cx, |s, cx| {
            let ov = s
                .miniapps
                .miniapp_overlay
                .as_ref()
                .expect("app_winner overlay must remain intact");
            let w = ov.read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.short_name.as_str()),
                Some("app_winner"),
                "late stale Ok must not overwrite newer miniapp_overlay"
            );
            assert!(s.miniapps.pending_webmcp_commands.is_empty());
        })
        .unwrap();
}

fn body_user_switch_clears_bot_browser_and_magpie(cx: &mut gpui::App) {
    use bot_browser::e2ee_sync::UnlockedUserVaultKey;
    use gpui::{AppContext, Entity};
    use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
    use gpui_telegram_client::net::hermes_client::HermesRichChatDto;
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::views::settings::sections::VaultCustodyStatus;
    use gpui_telegram_desktop::views::AppShell;
    use gpui_telegram_proto::contracts::auth::ContactDto;
    use gpui_telegram_proto::miniapp_contracts::MiniAppEntry;
    use std::sync::Arc;

    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    let auth_a = AuthResponse {
        token: "tok_user_a".to_string(),
        session_id: "sess_user_a".to_string(),
        user_id: 500_001,
        username: "user_a".to_string(),
        first_name: Some("UserA".to_string()),
        created_at: None,
        expires_at: None,
    };
    let auth_b = AuthResponse {
        token: "tok_user_b".to_string(),
        session_id: "sess_user_b".to_string(),
        user_id: 500_002,
        username: "user_b".to_string(),
        first_name: Some("UserB".to_string()),
        created_at: None,
        expires_at: None,
    };

    shell.update(cx, |s, cx| {
        s.apply_authenticated_session_with_vault(auth_a.clone(), None, cx);
        s.bot_browser.unlocked_vault_key = Some(Arc::new(UnlockedUserVaultKey::from_bytes([0xAA; 32])));
        s.bot_browser.vault_custody_status = VaultCustodyStatus::UnlockedAndSynced;
        s.bot_browser.hydrated_subscriptions_fingerprint = Some("fp_user_a".to_string());
        s.bot_browser.card_locations.insert("card_a".to_string(), (10, 20));
        s.bot_browser.local_fabushi = Some(Arc::new(
            FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("ephemeral host"),
        ));
        s.miniapps.catalog_miniapps.push(MiniAppEntry {
            app_id: "app_a".to_string(),
            bot_user_id: 777_001,
            bot_username: "bot_a".to_string(),
            short_name: "app_a".to_string(),
            title: "App A".to_string(),
            ..Default::default()
        });
        s.magpie_local.active_legacy_subscription_id = Some("sub_a".to_string());
        s.session_sync.contacts_list.push(ContactDto {
            user_id: 99_001,
            username: Some("friend_a".to_string()),
            first_name: "Friend".to_string(),
            last_name: Some("A".to_string()),
            phone: None,
            is_bot: false,
            is_mutual: true,
            chat_id: Some(88_001),
        });
        s.session_sync
            .rich_chats_by_id
            .insert(88_001, HermesRichChatDto::default());
        s.session_sync.chat_peer_user_ids.insert(88_001, 99_001);
        s.session_sync
            .last_draft_text_by_chat
            .insert(88_001, "user A secret draft".to_string());
        s.session_sync
            .last_saved_draft_by_chat
            .insert(88_001, "user A saved draft".to_string());

        // Same-user re-auth must preserve vault key and session caches
        s.apply_authenticated_session_with_vault(
            AuthResponse {
                token: "tok_user_a_refresh".to_string(),
                ..auth_a
            },
            None,
            cx,
        );
        assert!(
            s.bot_browser.unlocked_vault_key.is_some(),
            "Same-user re-auth must preserve unlocked_vault_key"
        );
        assert_eq!(s.session_sync.contacts_list.len(), 1);
        assert_eq!(s.session_sync.rich_chats_by_id.len(), 1);
        assert_eq!(s.session_sync.last_draft_text_by_chat.len(), 1);

        // Direct user switch without calling logout() first
        s.apply_authenticated_session_with_vault(auth_b, None, cx);

        assert!(
            s.bot_browser.unlocked_vault_key.is_none(),
            "User A unlocked_vault_key must be cleared on user switch"
        );
        assert!(
            s.bot_browser.local_fabushi.is_none(),
            "User A local_fabushi must be cleared on user switch"
        );
        assert!(
            s.bot_browser.hydrated_subscriptions_fingerprint.is_none(),
            "User A subscriptions fingerprint must be cleared on user switch"
        );
        assert!(
            s.bot_browser.card_locations.is_empty(),
            "User A card_locations must be cleared on user switch"
        );
        assert!(
            s.miniapps.catalog_miniapps.is_empty(),
            "User A miniapps catalog must be cleared on user switch"
        );
        assert!(
            s.magpie_local.active_legacy_subscription_id.is_none(),
            "User A magpie_local active_legacy_subscription_id must be cleared on user switch"
        );
        assert!(
            s.session_sync.contacts_list.is_empty(),
            "User A session_sync.contacts_list must be cleared on user switch"
        );
        assert!(
            s.session_sync.rich_chats_by_id.is_empty(),
            "User A session_sync.rich_chats_by_id must be cleared on user switch"
        );
        assert!(
            s.session_sync.chat_peer_user_ids.is_empty(),
            "User A session_sync.chat_peer_user_ids must be cleared on user switch"
        );
        assert!(
            s.session_sync.last_draft_text_by_chat.is_empty(),
            "User A session_sync.last_draft_text_by_chat must be cleared on user switch"
        );
        assert!(
            s.session_sync.last_saved_draft_by_chat.is_empty(),
            "User A session_sync.last_saved_draft_by_chat must be cleared on user switch"
        );
    });
}

#[test]
fn test_webmcp_pending_commands_drain_on_restore_and_loading_without_pip_flash() {
    spawn_headless_hermes_bugfix_test(
        "test_webmcp_pending_commands_drain_on_restore_and_loading_without_pip_flash",
        "webmcp_pending_commands_drain_and_no_pip_flash",
    );
}

#[test]
fn test_webmcp_alias_short_name_drain_and_concurrent_launch_generation_fence() {
    spawn_headless_hermes_bugfix_test(
        "test_webmcp_alias_short_name_drain_and_concurrent_launch_generation_fence",
        "webmcp_alias_and_concurrent_launch_fence",
    );
}

#[test]
fn test_apply_authenticated_session_user_switch_clears_bot_browser_and_magpie_state() {
    spawn_headless_hermes_bugfix_test(
        "test_apply_authenticated_session_user_switch_clears_bot_browser_and_magpie_state",
        "user_switch_clears_bot_browser_and_magpie",
    );
}

async fn spawn_mock_reused_overlay_launch_server() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use gpui_telegram_proto::miniapp_contracts::{
        BotRegistryEntry, MiniAppEntry, MiniAppLaunchContext, MiniAppLaunchMode,
    };
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let recorded_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_clone = Arc::clone(&recorded_paths);

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let recorded = Arc::clone(&recorded_clone);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut total = 0usize;
                let header_end;
                loop {
                    let Ok(n) = sock.read(&mut buf[total..]).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    total += n;
                    if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = pos + 4;
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let first_line = head.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let path = parts.next().unwrap_or("/").to_string();

                if method == "POST" && path.ends_with("/launch") {
                    recorded.lock().unwrap().push(path.clone());
                }

                let make_ctx = |bot_un: &str, short: &str, bot_uid: i64, chat_id: i64| {
                    MiniAppLaunchContext {
                        app: MiniAppEntry {
                            app_id: format!("app-{bot_un}-{short}"),
                            bot_user_id: bot_uid,
                            bot_username: bot_un.to_string(),
                            short_name: short.to_string(),
                            title: format!("{bot_un}/{short}"),
                            web_url: format!("https://miniapp.local/{bot_un}/{short}"),
                            app_tos_accepted: true,
                            ..Default::default()
                        },
                        companion_bot: BotRegistryEntry {
                            bot_user_id: bot_uid,
                            username: bot_un.to_string(),
                            display_name: bot_un.to_string(),
                            ..Default::default()
                        },
                        companion_chat_id: chat_id,
                        launch_mode: MiniAppLaunchMode::CompactSheet,
                        ..Default::default()
                    }
                };

                let (status, body) = if path == "/api/v1/miniapps/bot_a/dharma/launch" {
                    let ctx = make_ctx("bot_a", "dharma", 910_001, 920_001);
                    (200u16, serde_json::to_string(&ctx).unwrap())
                } else if path == "/api/v1/miniapps/bot_b/dharma/launch" {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    let ctx = make_ctx("bot_b", "dharma", 910_002, 920_002);
                    (200u16, serde_json::to_string(&ctx).unwrap())
                } else if path == "/api/v1/miniapps/bot_a/app_b/launch" {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    let ctx = make_ctx("bot_a", "app_b", 910_001, 920_001);
                    (200u16, serde_json::to_string(&ctx).unwrap())
                } else {
                    (200u16, r#"{"items":[]}"#.to_string())
                };
                let reason = if status == 200 { "OK" } else { "Internal Server Error" };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });

    (format!("http://{addr}"), recorded_paths)
}

async fn body_webmcp_reused_overlay_switch_and_reload_isolation(mut cx: gpui::AsyncApp) {
    use gpui::{AppContext, Entity};
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::components::bots::bot_attach_web_view::{
        AttachBotEntry, BotAttachWebView,
    };
    use gpui_telegram_desktop::views::AppShell;
    use gpui_telegram_proto::miniapp_contracts::{
        MiniAppLaunchMode, MiniAppWebMcpCommand, WebMcpToolDescriptor,
    };
    use std::sync::Arc;
    use std::time::Duration;

    // 1. Unit assertions on BotAttachWebView::current_short_name()
    {
        let mut w = BotAttachWebView::new();
        w.active_bot = Some(AttachBotEntry {
            bot_id: 910_001,
            name: "Dharma".to_string(),
            short_name: "dharma".to_string(),
            allowed_peer_types: 0x0F,
            in_main_menu: true,
            in_attach_menu: true,
            disclaimer_required: false,
            request_write_access: false,
        });
        w.is_loading = true;
        w.launching_short_name = "app_b".to_string();
        assert_eq!(w.current_short_name(), "app_b");

        w.is_loading = false;
        assert_eq!(w.current_short_name(), "dharma");

        w.is_loading = true;
        w.launching_short_name.clear();
        assert_eq!(w.current_short_name(), "dharma");
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();
    let (base_url, recorded_paths) = rt.block_on(spawn_mock_reused_overlay_launch_server());

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let auth = AuthResponse {
        token: "tok_reuse".to_string(),
        session_id: "sess_reuse".to_string(),
        user_id: 400_011,
        username: "reuse_user".to_string(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let shell: Entity<AppShell> = cx
        .update(|app| {
            app.new(|cx| {
                let mut s = AppShell::new_with_client(client.clone(), cx);
                s.session_sync.begin_session(
                    auth.token.clone(),
                    auth.session_id.clone(),
                    auth.user_id,
                    auth.username.clone(),
                );
                s.is_authenticated = true;
                s
            })
        })
        .expect("create AppShell");

    let wait_launch_done = async |cx: &mut gpui::AsyncApp, expected_bot: &str, expected_short: &str| {
        for _ in 0..80 {
            cx.background_executor().timer(Duration::from_millis(20)).await;
            let done = shell
                .read_with(cx, |s, cx| {
                    s.miniapps.pending_webmcp_commands.is_empty()
                        && s.miniapps.miniapp_overlay.as_ref().is_some_and(|ov| {
                            let w = ov.read(cx);
                            !w.is_loading
                                && w.launch_context.as_ref().is_some_and(|c| {
                                    c.app.bot_username.eq_ignore_ascii_case(expected_bot)
                                        && c.app.short_name.eq_ignore_ascii_case(expected_short)
                                })
                        })
                })
                .unwrap();
            if done {
                return;
            }
        }
        panic!("timed out waiting for {expected_bot}/{expected_short} launch completion");
    };

    let make_cmd = |req_id: &str, bot: &str, short: &str, chat_id: i64, bot_uid: i64| MiniAppWebMcpCommand {
        request_id: req_id.to_string(),
        bot_username: bot.to_string(),
        short_name: short.to_string(),
        chat_id,
        bot_user_id: bot_uid,
        app_title: format!("{bot}/{short}"),
        operation: "discover".to_string(),
        tool_name: None,
        arguments: serde_json::json!({}),
        status_message: None,
    };

    // 2. Probe 2A: Cross-bot same short_name (bot_a/dharma -> bot_b/dharma mid-flight WebMCP)
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_with_url(
                "bot_a",
                "dharma",
                Some(920_001),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );
        })
        .unwrap();
    wait_launch_done(&mut cx, "bot_a", "dharma").await;

    shell
        .update(&mut cx, |s, cx| {
            let ov = s.miniapps.miniapp_overlay.as_ref().unwrap().clone();
            ov.update(cx, |w, _| {
                w.url = "https://stale.example/bot_a/dharma".to_string();
                w.app_id = Some("stale-app-a".to_string());
                w.webmcp_page_ready = true;
                w.webmcp_iframe_ready = true;
                w.webmcp_discovered_tools.push(WebMcpToolDescriptor {
                    name: "stale_tool".to_string(),
                    description: String::new(),
                    input_schema: serde_json::json!({}),
                    read_only: false,
                });
                w.last_webmcp_state_summary = Some(serde_json::json!({"stale": true}));
            });

            s.launch_miniapp_with_url(
                "bot_b",
                "dharma",
                Some(920_002),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );

            let w = ov.read(cx);
            assert!(w.is_loading, "Probe 2A: bot_b/dharma must be loading");
            assert!(w.active_bot.is_none(), "Probe 2A: stale active_bot must be cleared");
            assert!(w.launch_context.is_none(), "Probe 2A: stale launch_context must be cleared");
            assert!(w.app_id.is_none(), "Probe 2A: stale app_id must be cleared");
            assert!(
                w.last_webmcp_state_summary.is_none(),
                "Probe 2A: stale last_webmcp_state_summary must be cleared"
            );
            assert!(!w.webmcp_page_ready, "Probe 2A: webmcp_page_ready must be reset");
            assert!(!w.webmcp_iframe_ready, "Probe 2A: webmcp_iframe_ready must be reset");
            assert!(
                w.webmcp_discovered_tools.is_empty(),
                "Probe 2A: webmcp_discovered_tools must be cleared"
            );
            assert!(w.url.is_empty(), "Probe 2A: stale url must be cleared");
            assert_eq!(w.bot_username, "bot_b");
            assert_eq!(w.current_short_name(), "dharma");

            let gen_before = s.miniapps.miniapp_launch_generation;
            s.launch_miniapp_for_webmcp(
                make_cmd("req-botb-mid", "bot_b", "dharma", 920_002, 910_002),
                cx,
            );
            assert_eq!(s.miniapps.pending_webmcp_commands.len(), 1);
            assert!(ov.read(cx).last_webmcp_state_summary.is_none());
            assert_eq!(s.miniapps.miniapp_launch_generation, gen_before);
        })
        .unwrap();

    wait_launch_done(&mut cx, "bot_b", "dharma").await;
    shell
        .read_with(&cx, |s, cx| {
            assert!(s.miniapps.pending_webmcp_commands.is_empty());
            let w = s.miniapps.miniapp_overlay.as_ref().unwrap().read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.bot_username.as_str()),
                Some("bot_b")
            );
            assert!(w.last_webmcp_state_summary.is_some());
        })
        .unwrap();
    let bot_b_count = recorded_paths
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_str() == "/api/v1/miniapps/bot_b/dharma/launch")
        .count();
    assert_eq!(bot_b_count, 1, "Probe 2A must issue exactly 1 /launch for bot_b/dharma");

    // 3. Probe 2B: Same-bot different short_name (bot_a/dharma -> bot_a/app_b mid-flight WebMCP)
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_with_url(
                "bot_a",
                "dharma",
                Some(920_001),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );
        })
        .unwrap();
    wait_launch_done(&mut cx, "bot_a", "dharma").await;

    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_with_url(
                "bot_a",
                "app_b",
                Some(920_001),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );
            let gen_before = s.miniapps.miniapp_launch_generation;
            let ov = s.miniapps.miniapp_overlay.as_ref().unwrap();
            assert_eq!(ov.read(cx).current_short_name(), "app_b");

            s.launch_miniapp_for_webmcp(
                make_cmd("req-appb-mid", "bot_a", "app_b", 920_001, 910_001),
                cx,
            );
            assert_eq!(s.miniapps.miniapp_launch_generation, gen_before);
            assert_eq!(s.miniapps.pending_webmcp_commands.len(), 1);
        })
        .unwrap();

    wait_launch_done(&mut cx, "bot_a", "app_b").await;
    shell
        .read_with(&cx, |s, cx| {
            assert!(s.miniapps.pending_webmcp_commands.is_empty());
            let w = s.miniapps.miniapp_overlay.as_ref().unwrap().read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.short_name.as_str()),
                Some("app_b")
            );
            assert!(w.last_webmcp_state_summary.is_some());
        })
        .unwrap();
    let app_b_count = recorded_paths
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_str() == "/api/v1/miniapps/bot_a/app_b/launch")
        .count();
    assert_eq!(app_b_count, 1, "Probe 2B must issue exactly 1 /launch for bot_a/app_b");

    // 4. Probe 2C: try_restore_running_miniapp mid-flight after bot_a/dharma -> bot_b/dharma
    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_with_url(
                "bot_a",
                "dharma",
                Some(920_001),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );
        })
        .unwrap();
    wait_launch_done(&mut cx, "bot_a", "dharma").await;

    shell
        .update(&mut cx, |s, cx| {
            s.launch_miniapp_with_url(
                "bot_b",
                "dharma",
                Some(920_002),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );
            s.launch_miniapp_for_webmcp(
                make_cmd("req-rest-premature", "bot_b", "dharma", 920_002, 910_002),
                cx,
            );
            assert_eq!(s.miniapps.pending_webmcp_commands.len(), 1);

            let restored = s.try_restore_running_miniapp("bot_b", "dharma", None, None, cx);
            assert!(restored, "Probe 2C: try_restore_running_miniapp must match in-flight bot_b/dharma");
            let w = s.miniapps.miniapp_overlay.as_ref().unwrap().read(cx);
            assert!(w.is_open);
            assert!(!w.is_floating_pip);
            assert!(!s.miniapps.is_miniapp_minimized);
            assert_eq!(
                s.miniapps.pending_webmcp_commands.len(),
                1,
                "Probe 2C: queued command must NOT drain prematurely while bot_b/dharma is loading"
            );
            assert!(w.last_webmcp_state_summary.is_none());
        })
        .unwrap();

    wait_launch_done(&mut cx, "bot_b", "dharma").await;
    shell
        .read_with(&cx, |s, cx| {
            assert!(s.miniapps.pending_webmcp_commands.is_empty());
            let w = s.miniapps.miniapp_overlay.as_ref().unwrap().read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.bot_username.as_str()),
                Some("bot_b")
            );
            assert!(w.last_webmcp_state_summary.is_some());
        })
        .unwrap();

    // 5. Force Reload: reload_miniapp_with_url("bot_b", "dharma") mid-flight WebMCP
    shell
        .update(&mut cx, |s, cx| {
            let ov = s.miniapps.miniapp_overlay.as_ref().unwrap().clone();
            assert!(ov.read(cx).last_webmcp_state_summary.is_some());

            s.reload_miniapp_with_url(
                "bot_b",
                "dharma",
                Some(920_002),
                MiniAppLaunchMode::CompactSheet,
                None,
                None,
                cx,
            );
            let gen_before = s.miniapps.miniapp_launch_generation;
            let w = ov.read(cx);
            assert!(w.active_bot.is_none());
            assert!(w.launch_context.is_none());
            assert!(w.last_webmcp_state_summary.is_none());
            assert!(w.is_loading);

            s.launch_miniapp_for_webmcp(
                make_cmd("req-reload-mid", "bot_b", "dharma", 920_002, 910_002),
                cx,
            );
            assert_eq!(s.miniapps.miniapp_launch_generation, gen_before);
            assert_eq!(s.miniapps.pending_webmcp_commands.len(), 1);
        })
        .unwrap();

    wait_launch_done(&mut cx, "bot_b", "dharma").await;
    shell
        .read_with(&cx, |s, cx| {
            assert!(s.miniapps.pending_webmcp_commands.is_empty());
            let w = s.miniapps.miniapp_overlay.as_ref().unwrap().read(cx);
            assert_eq!(
                w.launch_context.as_ref().map(|c| c.app.bot_username.as_str()),
                Some("bot_b")
            );
            assert!(w.last_webmcp_state_summary.is_some());
        })
        .unwrap();
}

#[test]
fn test_webmcp_reused_overlay_switch_and_reload_isolation() {
    spawn_headless_hermes_bugfix_test(
        "test_webmcp_reused_overlay_switch_and_reload_isolation",
        "webmcp_reused_overlay_switch_and_reload_isolation",
    );
}

async fn body_stage_model_picker_live_sync_and_persistence(mut cx: gpui::AsyncApp) {
    use gpui::{AppContext, Entity};
    use gpui_telegram_client::net::hermes_client::{
        MagpieProviderDto, MagpieStateDto, MagpieSubscriptionDto,
    };
    use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::views::stage::stage_bars::TopBarUiAction;
    use gpui_telegram_desktop::views::AppShell;
    use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct MockModelServerState {
        active_model: String,
        activated_models: Vec<String>,
        patched_bots: Vec<(i64, Option<String>, Option<String>)>,
        owned_bot_model: String,
        owned_bot_thinking: String,
    }

    let server_state = Arc::new(Mutex::new(MockModelServerState {
        active_model: "gemini-3.8-flash".to_string(),
        activated_models: Vec::new(),
        patched_bots: Vec::new(),
        owned_bot_model: String::new(),
        owned_bot_thinking: "medium".to_string(),
    }));
    let server_state_bg = Arc::clone(&server_state);

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();
    let base_url = rt.block_on(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let make_magpie_state = |active_model: &str| MagpieStateDto {
                active_provider: "google_vertex:bhrum21@gmail.com".to_string(),
                active_model: active_model.to_string(),
                allow_local_fallback: false,
                has_active_subscription: true,
                has_configured_provider: true,
                needs_provider_selection: false,
                subscriptions: vec![MagpieSubscriptionDto {
                    id: "google_vertex:bhrum21@gmail.com".to_string(),
                    kind: "google_vertex".to_string(),
                    label: "Google Vertex AI".to_string(),
                    active: true,
                    default_model: Some(active_model.to_string()),
                    ..Default::default()
                }],
                providers: vec![MagpieProviderDto {
                    provider_id: "google_vertex:bhrum21@gmail.com".to_string(),
                    name: "Google Vertex AI".to_string(),
                    default_model: active_model.to_string(),
                    models: vec![
                        "gemini-3.8-flash".to_string(),
                        "gemini-3.7-flash".to_string(),
                        "gemini-2.5-pro".to_string(),
                    ],
                    enabled: true,
                    is_active: true,
                    ..Default::default()
                }],
                ..Default::default()
            };

            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let st_arc = Arc::clone(&server_state_bg);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let mut total = 0usize;
                    let header_end;
                    loop {
                        let Ok(n) = sock.read(&mut buf[total..]).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        total += n;
                        if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = pos + 4;
                            break;
                        }
                    }
                    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let content_len: usize = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while total < header_end + content_len {
                        let Ok(n) = sock.read(&mut buf[total..]).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        total += n;
                    }
                    let body_str = String::from_utf8_lossy(
                        &buf[header_end..(header_end + content_len).min(total)],
                    )
                    .to_string();
                    let first_line = head.lines().next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let method = parts.next().unwrap_or("GET").to_string();
                    let path = parts.next().unwrap_or("/").to_string();

                    let resp_body = if method == "GET"
                        && path.starts_with("/api/v1/hermes/magpie/models/catalog")
                    {
                        r#"{"fetched_at_ms":1700000000000,"source":"mock","providers":{}}"#
                            .to_string()
                    } else if method == "GET" && path == "/api/v1/hermes/magpie/state" {
                        let cur = st_arc.lock().unwrap().active_model.clone();
                        serde_json::to_string(&make_magpie_state(&cur)).unwrap()
                    } else if method == "GET" && path == "/api/v1/hermes/magpie/subscriptions" {
                        let cur = st_arc.lock().unwrap().active_model.clone();
                        json!([{
                            "id": "google_vertex:bhrum21@gmail.com",
                            "kind": "google_vertex",
                            "label": "Google Vertex AI",
                            "models": ["gemini-3.8-flash", "gemini-3.7-flash", "gemini-2.5-pro"],
                            "active": true,
                            "active_model": cur,
                        }])
                        .to_string()
                    } else if method == "POST"
                        && path.starts_with("/api/v1/hermes/magpie/subscriptions/")
                        && path.ends_with("/activate")
                    {
                        let parsed: serde_json::Value =
                            serde_json::from_str(&body_str).unwrap_or_default();
                        let next_model = parsed
                            .get("model")
                            .and_then(|v| v.as_str())
                            .unwrap_or("gemini-3.8-flash")
                            .to_string();
                        {
                            let mut g = st_arc.lock().unwrap();
                            g.active_model = next_model.clone();
                            g.activated_models.push(next_model.clone());
                        }
                        json!({
                            "ok": true,
                            "active_provider_id": "google_vertex:bhrum21@gmail.com",
                            "active_model": next_model,
                            "state": make_magpie_state(&next_model),
                        })
                        .to_string()
                    } else if method == "PATCH" && path == "/api/v1/fabushi-bots/777001" {
                        let parsed: serde_json::Value =
                            serde_json::from_str(&body_str).unwrap_or_default();
                        let pm = parsed
                            .get("preferred_model")
                            .and_then(|v| v.as_str())
                            .map(ToString::to_string);
                        let tl = parsed
                            .get("thinking_level")
                            .and_then(|v| v.as_str())
                            .map(ToString::to_string);
                        let (cur_pm, cur_tl) = {
                            let mut g = st_arc.lock().unwrap();
                            if let Some(ref m) = pm {
                                g.owned_bot_model = m.clone();
                            }
                            if let Some(ref t) = tl {
                                g.owned_bot_thinking = t.clone();
                            }
                            g.patched_bots.push((777_001, pm.clone(), tl.clone()));
                            (g.owned_bot_model.clone(), g.owned_bot_thinking.clone())
                        };
                        serde_json::to_string(&FabushiBotSummaryDto {
                            bot_user_id: 777_001,
                            owner_user_id: 500_001,
                            agent_id: "owned_bot".to_string(),
                            username: "owned_bot".to_string(),
                            display_name: "My Owned Bot".to_string(),
                            preferred_model: cur_pm,
                            thinking_level: cur_tl,
                            companion_chat_id: Some(888_002),
                            ..Default::default()
                        })
                        .unwrap()
                    } else if path == "/api/v1/chats" || path.contains("/messages") {
                        "[]".to_string()
                    } else {
                        "{}".to_string()
                    };

                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let viewer_uid = 500_001u64;
    let auth = AuthResponse {
        token: "tok_model_picker_e2e".to_string(),
        session_id: "sess_model_picker_e2e".to_string(),
        user_id: viewer_uid,
        username: "picker_tester".to_string(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let official_fab = FabushiBotSummaryDto {
        bot_user_id: 666_001,
        owner_user_id: 0,
        agent_id: "fabushi_bot".to_string(),
        username: "fabushi_bot".to_string(),
        display_name: "全球法布施".to_string(),
        preferred_model: String::new(),
        thinking_level: "medium".to_string(),
        companion_chat_id: Some(888_001),
        ..Default::default()
    };
    let owned_fab = FabushiBotSummaryDto {
        bot_user_id: 777_001,
        owner_user_id: viewer_uid as i64,
        agent_id: "owned_bot".to_string(),
        username: "owned_bot".to_string(),
        display_name: "My Owned Bot".to_string(),
        preferred_model: String::new(),
        thinking_level: "medium".to_string(),
        companion_chat_id: Some(888_002),
        ..Default::default()
    };

    let shell: Entity<AppShell> = cx
        .update(|app| {
            app.new(|cx| {
                let mut s = AppShell::new_with_client(client.clone(), cx);
                s.session_sync.begin_session(
                    auth.token.clone(),
                    auth.session_id.clone(),
                    auth.user_id,
                    auth.username.clone(),
                );
                s.is_authenticated = true;
                s.miniapps.fabushi_bots = vec![official_fab.clone(), owned_fab.clone()];
                s.stage.update(cx, |st, _| {
                    st.set_chat(888_001, "全球法布施", "bot", "法", 0xE040FB);
                    st.is_bot_chat = true;
                    st.set_active_fabushi_bot(Some(official_fab.clone()));
                });
                s.refresh_magpie_state(cx);
                s
            })
        })
        .expect("create AppShell");

    // 1. Wait for refresh_magpie_state to hydrate ConversationStage, Inspector, and CreateManagedBotBoxPeers.
    for _ in 0..80 {
        cx.background_executor().timer(Duration::from_millis(20)).await;
        let synced = shell
            .read_with(&cx, |s, cx| s.stage.read(cx).model_catalog_synced)
            .unwrap();
        if synced {
            break;
        }
    }
    shell
        .update(&mut cx, |s, cx| {
            let st = s.stage.read(cx);
            assert!(st.model_catalog_synced, "stage must receive live Magpie catalog");
            assert_eq!(st.active_model_label, "gemini-3.8-flash");
            assert!(st.available_models.contains(&"gemini-3.8-flash".to_string()));
            assert!(st.available_models.contains(&"gemini-2.5-pro".to_string()));
            assert!(!st.available_models.iter().any(|m| m == "claude-sonnet-4-5" || m == "gpt-5"));

            let box_ent = s.ensure_create_bot_box(cx);
            assert_eq!(
                box_ent.read(cx).studio_model_display_label("zh-CN"),
                "gemini-3.8-flash"
            );
        })
        .unwrap();

    // 2. Switching model on official bot (全球法布施) activates the real Settings model on backend.
    shell
        .update(&mut cx, |s, cx| {
            s.stage.update(cx, |st, st_cx| {
                st.handle_topbar_action(
                    TopBarUiAction::SelectModel {
                        model_id: Some("gemini-2.5-pro".to_string()),
                        thinking_level: None,
                    },
                    st_cx,
                );
            });
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor().timer(Duration::from_millis(20)).await;
        if !server_state.lock().unwrap().activated_models.is_empty()
            && shell
                .read_with(&cx, |s, cx| s.inspector.read(cx).ai_models.active_model == "gemini-2.5-pro")
                .unwrap()
        {
            break;
        }
    }
    assert_eq!(
        server_state.lock().unwrap().activated_models,
        vec!["gemini-2.5-pro".to_string()]
    );
    shell
        .read_with(&cx, |s, cx| {
            assert_eq!(s.stage.read(cx).active_model_label, "gemini-2.5-pro");
            assert_eq!(s.inspector.read(cx).ai_models.active_model, "gemini-2.5-pro");
        })
        .unwrap();

    // 3. Switching model + thinking effort on viewer-owned Fabushi bot sends real PATCH /api/v1/fabushi-bots/777001.
    shell
        .update(&mut cx, |s, cx| {
            s.stage.update(cx, |st, _| {
                st.set_chat(888_002, "My Owned Bot", "bot", "MB", 0x2AABEE);
                st.is_bot_chat = true;
                st.set_active_fabushi_bot(Some(owned_fab.clone()));
            });
            s.inspector.update(cx, |insp, _| {
                insp.set_active_fabushi_bot(Some(owned_fab.clone()));
            });
            s.sync_fabushi_bots_to_views(cx);
            s.stage.update(cx, |st, st_cx| {
                st.handle_topbar_action(
                    TopBarUiAction::SelectModel {
                        model_id: Some("gemini-3.7-flash".to_string()),
                        thinking_level: None,
                    },
                    st_cx,
                );
            });
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor().timer(Duration::from_millis(20)).await;
        if !server_state.lock().unwrap().patched_bots.is_empty() {
            break;
        }
    }
    shell
        .update(&mut cx, |s, cx| {
            s.stage.update(cx, |st, st_cx| {
                st.handle_topbar_action(
                    TopBarUiAction::SelectModel {
                        model_id: None,
                        thinking_level: Some("high".to_string()),
                    },
                    st_cx,
                );
            });
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor().timer(Duration::from_millis(20)).await;
        if server_state.lock().unwrap().patched_bots.len() >= 2
            && shell
                .read_with(&cx, |s, cx| s.stage.read(cx).active_thinking_effort == "high")
                .unwrap()
        {
            break;
        }
    }
    {
        let g = server_state.lock().unwrap();
        assert_eq!(g.owned_bot_model, "gemini-3.7-flash");
        assert_eq!(g.owned_bot_thinking, "high");
        assert_eq!(g.patched_bots[0], (777_001, Some("gemini-3.7-flash".to_string()), None));
        assert_eq!(g.patched_bots[1], (777_001, None, Some("high".to_string())));
    }
    shell
        .read_with(&cx, |s, cx| {
            let st = s.stage.read(cx);
            assert_eq!(st.active_model_label, "gemini-3.7-flash");
            assert_eq!(st.active_thinking_effort, "high");
        })
        .unwrap();

    // 4. Logout clears stage model picker state so zero stale models leak across accounts.
    shell
        .update(&mut cx, |s, cx| {
            s.logout(cx);
            let st = s.stage.read(cx);
            assert!(!st.model_catalog_synced);
            assert!(st.available_models.is_empty());
            assert!(st.active_model_label.is_empty());
        })
        .unwrap();
}

#[test]
fn test_stage_model_picker_live_sync_and_persistence() {
    spawn_headless_hermes_bugfix_test(
        "test_stage_model_picker_live_sync_and_persistence",
        "stage_model_picker_live_sync_and_persistence",
    );
}

