use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::channels::{
    CHANNEL_ASSIGNMENT_LIMIT, CHANNEL_PARALLEL_LIMIT, CHANNEL_REPLY_ID_PREFIX,
    CHANNEL_TOOL_DEFINITIONS, Channel, ChannelDraft, ChannelIconName, ChannelMember,
    ChannelMessage, ChannelMessageAuthor, ChannelMessageAuthorKind, ChannelReplyOutcome,
    ChannelRoutingDecision, ChannelTask, ChannelTaskState, ChatTagKind, CreateChannelToolIcon,
    EXTERNAL_CHANNEL_TOOL_NAMES, FileQuoteLines, FileQuoteTarget, MentionableAgentInfo,
    NamedChannelAgent, UpdateChannelToolInput, agent_mention_rank, build_member_alias_map,
    chat_tag_references, check_channel_reply, deterministic_create_channel_id,
    expand_chat_tag_references, file_quote_id, file_quote_target, is_continue_stopped_message,
    is_follow_up_message, is_task_settled, leading_address, rank_agent_mentions,
    released_parent_task, resolve_channel_recipients, serialize_chat_tag_reference, typed_address,
    without_own_name_label,
};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};

#[test]
fn semantic_tags_uri_encoding_and_leading_address() {
    for kind in ChatTagKind::ALL {
        let plain = serialize_chat_tag_reference(kind, "Alice", "a-1");
        assert_eq!(plain, format!("@[Alice]({}:a-1)", kind.as_str()));
        let parsed = chat_tag_references(&plain);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, kind);
        assert_eq!(parsed[0].name, "Alice");
        assert_eq!(parsed[0].id, "a-1");
    }

    // Special chars trigger +uri RFC-3986 encoding (including ! ' ( ) *)
    let encoded = serialize_chat_tag_reference(
        ChatTagKind::Quote,
        "line 1]\n`code`!",
        "workspace-file:src/a(1).rs#L2-5",
    );
    assert!(encoded.contains("quote+uri:"));
    assert!(encoded.contains("%21"));
    assert!(encoded.contains("%28"));
    let refs = chat_tag_references(&encoded);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].name, "line 1]\n`code`!");
    assert_eq!(refs[0].id, "workspace-file:src/a(1).rs#L2-5");

    // FileQuoteTarget round-trip and expansion with file_headings
    let target = FileQuoteTarget::Attachment {
        attachment_id: "att-9".into(),
        name: "spec.md".into(),
        lines: Some(FileQuoteLines { start: 10, end: 25 }),
    };
    let fid = file_quote_id(&target);
    assert_eq!(fid, "attachment-file:att-9/spec.md#L10-25");
    assert_eq!(file_quote_target(&fid), Some(target));

    let msg = format!("{encoded}Please review @[Deploy](skill:s1) and @[GitHub](mcp:m1)");
    let expanded = expand_chat_tag_references(
        &msg,
        None::<fn(&openbot_contracts::channels::ChatTagReference) -> Option<String>>,
        true,
    );
    assert!(expanded.contains("From `src/a(1).rs`, lines 2-5:"));
    assert!(expanded.contains("Deploy (skill)"));
    assert!(expanded.contains("GitHub (MCP server)"));

    // leading_address with @everyone, connectors, and boundary guard
    let addr = leading_address("@[Alice](agent:a1), @[Bob](agent:a2) and @everyone ship this");
    assert_eq!(addr.mentions.len(), 2);
    assert!(addr.everyone);
    let false_everyone = leading_address("@everyoneelse hi");
    assert!(!false_everyone.everyone);

    // Standalone \r is preserved (split strictly on /\r?\n/u)
    assert_eq!(
        expand_chat_tag_references(
            "@[hello%0D](quote+uri:q1)",
            None::<fn(&openbot_contracts::channels::ChatTagReference) -> Option<String>>,
            false,
        ),
        "> hello\r"
    );
    assert_eq!(
        expand_chat_tag_references(
            "@[line1%0D%0Aline2%0D](quote+uri:q1)",
            None::<fn(&openbot_contracts::channels::ChatTagReference) -> Option<String>>,
            false,
        ),
        "> line1\n> line2\r"
    );
}

#[test]
fn mentions_nfd_ranking_typed_address_and_channel_contracts() {
    let renee = MentionableAgentInfo {
        name: "Renée Code-Reviewer".into(),
        title: "Frontend Architect".into(),
        description: "Audits SolidJS and GPUI components".into(),
    };
    assert_eq!(agent_mention_rank(&renee, "renee code-reviewer"), Some(0));
    assert_eq!(agent_mention_rank(&renee, "renee"), Some(1));
    assert_eq!(agent_mention_rank(&renee, "reviewer"), Some(2));
    assert_eq!(agent_mention_rank(&renee, "ode-rev"), Some(3));
    assert_eq!(agent_mention_rank(&renee, "architect"), Some(4));
    assert_eq!(agent_mention_rank(&renee, "gpui"), Some(5));
    assert_eq!(agent_mention_rank(&renee, "reviewer frontend"), Some(6));
    assert_eq!(agent_mention_rank(&renee, "rust-kernel"), None);

    let list = vec![
        MentionableAgentInfo { name: "Beta".into(), title: "Code".into(), description: "".into() },
        MentionableAgentInfo { name: "Code".into(), title: "".into(), description: "".into() },
    ];
    let ranked = rank_agent_mentions(&list, "code");
    assert_eq!(ranked[0].name, "Code");

    // Longest-unique @AgentName & tie rejection
    let eligible = vec![
        NamedChannelAgent { id: "a1".into(), name: "Code".into() },
        NamedChannelAgent { id: "a2".into(), name: "Code Reviewer".into() },
    ];
    assert_eq!(typed_address(&eligible, "@Code Reviewer & @Code check"), vec!["a2", "a1"]);
    let tied = vec![
        NamedChannelAgent { id: "x1".into(), name: "Bot".into() },
        NamedChannelAgent { id: "x2".into(), name: "bot".into() },
    ];
    assert!(typed_address(&tied, "@bot hi").is_empty());

    let recips = resolve_channel_recipients(&eligible, "@everyone please help", Some("a2"));
    assert_eq!(recips, vec!["a2", "a1"]);
    assert!(is_follow_up_message("Actually use Rust instead"));
    assert!(is_continue_stopped_message("keep going please"));
    assert_eq!(without_own_name_label("**Atlas summary**: Done!", "Atlas", false), "Done!");

    // Asymmetric Channel decode_normalized (32_000 + legacy purpose + lead coercion) vs validate_strict (2_000)
    assert_eq!(CHANNEL_PARALLEL_LIMIT, 2);
    assert_eq!(CHANNEL_ASSIGNMENT_LIMIT, 8);
    assert_eq!(CHANNEL_TOOL_DEFINITIONS.len(), 15);
    assert_eq!(EXTERNAL_CHANNEL_TOOL_NAMES.len(), 4);

    let long_30k = "x".repeat(30_000);
    let raw = serde_json::json!({
        "id": "ch-1",
        "name": "Eng",
        "purpose": long_30k,
        "members": [{ "agentId": "a1" }],
        "leadAgentId": "removed-agent",
        "browser": { "kind": "member", "agentId": "removed-agent" },
        "icon": { "name": "INVALID!", "hue": 999 },
        "archived": false,
        "revision": 1,
        "createdAt": "2026-11-25T00:00:00Z"
    });
    let ch = Channel::decode_normalized(&raw).expect("lenient read succeeds");
    assert_eq!(ch.draft.lead_agent_id.as_deref(), Some("a1"));
    assert_eq!(ch.draft.instructions.len(), 30_000);
    assert!(ch.draft.icon.is_none());

    // Strict write rejects new >2_000 instructions, allows unchanged 30_000
    assert!(ch.draft.validate_strict(None).is_err());
    assert!(ch.draft.validate_strict(Some(&long_30k)).is_ok());

    let bad_draft = ChannelDraft {
        name: "Eng".into(),
        title: "".into(),
        instructions: "short".into(),
        members: vec![ChannelMember { agent_id: "a1".into() }],
        lead_agent_id: Some("not-a-member".into()),
        isolated: None,
        browser: None,
        icon: None,
    };
    assert!(bad_draft.validate_strict(None).is_err());

    // 5-shape Lead Router 1-key Serde + m1..mN alias collision skip & dedup + fallback to lead
    let members = vec!["m1".to_string(), "real-agent".to_string()];
    let (a2m, m2a) = build_member_alias_map(&members);
    assert_eq!(a2m.get("m1").map(String::as_str), Some("m2"));
    assert_eq!(a2m.get("real-agent").map(String::as_str), Some("m3"));

    // Duplicate `[...members, lead.id]` preserves bijection (`a1 -> m1, a2 -> m2`)
    let dup_members = vec!["a1".to_string(), "a2".to_string(), "a1".to_string()];
    let (dup_a2m, dup_m2a) = build_member_alias_map(&dup_members);
    assert_eq!(dup_a2m.len(), 2);
    assert_eq!(dup_m2a.len(), 2);
    assert_eq!(dup_a2m.get("a1").map(String::as_str), Some("m1"));
    assert_eq!(dup_a2m.get("a2").map(String::as_str), Some("m2"));
    assert_eq!(dup_m2a.get("m1").map(String::as_str), Some("a1"));
    assert_eq!(dup_m2a.get("m2").map(String::as_str), Some("a2"));

    let dec = ChannelRoutingDecision::decode_strict(
        &serde_json::json!({ "agentIds": ["m3"] }),
        Some(&m2a),
    )
    .unwrap();
    assert_eq!(dec, ChannelRoutingDecision::AssignAgents { agent_ids: vec!["real-agent".into()] });
    assert_eq!(serde_json::to_string(&ChannelRoutingDecision::Idle).unwrap(), r#"{"idle":true}"#);
    assert_eq!(
        serde_json::from_str::<ChannelRoutingDecision>(r#"{"idle":true}"#).unwrap(),
        ChannelRoutingDecision::Idle
    );
    assert_eq!(
        serde_json::to_string(&dec).unwrap(),
        r#"{"agentIds":["real-agent"]}"#
    );
    assert_eq!(
        serde_json::from_str::<ChannelRoutingDecision>(r#"{"taskId":"t-9"}"#).unwrap(),
        ChannelRoutingDecision::ResumeTask { task_id: "t-9".into() }
    );
    assert_eq!(
        serde_json::from_str::<ChannelRoutingDecision>(r#"{"agentId":"lead-1"}"#).unwrap(),
        ChannelRoutingDecision::AskLead { agent_id: "lead-1".into() }
    );
    assert_eq!(
        serde_json::from_str::<ChannelRoutingDecision>(r#"{"question":"Which branch?"}"#).unwrap(),
        ChannelRoutingDecision::ClarifyQuestion { question: "Which branch?".into() }
    );
    assert!(serde_json::from_str::<ChannelRoutingDecision>(r#"{"idle":true,"agentId":"a1"}"#).is_err());
    assert!(serde_json::from_str::<ChannelRoutingDecision>(r#"{"idle":false}"#).is_err());
    assert!(serde_json::from_str::<ChannelRoutingDecision>(r#"{"agentIds":[]}"#).is_err());
    assert!(ChannelRoutingDecision::decode_strict(&serde_json::json!({"idle": true, "extra": 1}), None).is_err());
    assert_eq!(
        ChannelRoutingDecision::decode_or_lead_fallback("not-json", None, "lead-1"),
        ChannelRoutingDecision::AskLead { agent_id: "lead-1".into() }
    );

    // UpdateChannelToolInput::icon enforces CreateChannelToolIcon (20 icon names + 10 AvatarHue ints)
    let valid_upd: UpdateChannelToolInput = serde_json::from_str(
        r#"{"channelId":"ch-1","icon":{"name":"sparkles","hue":215}}"#,
    )
    .unwrap();
    assert_eq!(
        valid_upd.icon,
        Some(Some(CreateChannelToolIcon {
            name: ChannelIconName::Sparkles,
            hue: AvatarHue::Blue,
        }))
    );
    let clear_upd: UpdateChannelToolInput =
        serde_json::from_str(r#"{"channelId":"ch-1","icon":null}"#).unwrap();
    assert_eq!(clear_upd.icon, Some(None));
    assert!(serde_json::from_str::<UpdateChannelToolInput>(
        r#"{"channelId":"ch-1","icon":{"name":"not-an-icon","hue":215}}"#
    )
    .is_err());
    assert!(serde_json::from_str::<UpdateChannelToolInput>(
        r#"{"channelId":"ch-1","icon":{"name":"sparkles","hue":999}}"#
    )
    .is_err());

    // Exact V8 golden UUID vectors for deterministic_create_channel_id (`channel-agent-tools.ts:L187-L192`)
    assert_eq!(
        deterministic_create_channel_id("turn-1:call-1"),
        "15279afb-9294-8d4a-aa7b-13ef8847a966"
    );
    assert_eq!(
        deterministic_create_channel_id("thread-9:turn-2:call-7"),
        "d4b0c5ed-d564-84fa-a65a-f78b54830d7d"
    );
    assert_eq!(
        deterministic_create_channel_id(""),
        "b356f1e2-871c-84d1-ad2f-77134e1953ae"
    );
    assert_eq!(
        deterministic_create_channel_id("unicode:🦞"),
        "e744b8f8-328e-8abc-af66-2153c3b8a09e"
    );
    let root = ChannelTask {
        id: "root-1".into(),
        channel_id: "ch-1".into(),
        parent_task_id: None,
        root_task_id: "root-1".into(),
        owner_agent_id: Some("a1".into()),
        request_message_id: "req-1".into(),
        instruction: "go".into(),
        attachment_draft_ids: vec![],
        expected_result: "ok".into(),
        source_message_ids: vec!["req-1".into()],
        dependencies: vec!["reply-task-1".into()],
        resources: vec![],
        state: ChannelTaskState::Waiting,
        revision: 1,
        assignment_count: 1,
        error: None,
        seen_dependencies: None,
    };
    let reply_id = format!("{CHANNEL_REPLY_ID_PREFIX}1");
    let reply_subtask = ChannelTask {
        id: "reply-task-1".into(),
        parent_task_id: Some("root-1".into()),
        request_message_id: reply_id.clone(),
        source_message_ids: vec![reply_id],
        dependencies: vec![],
        state: ChannelTaskState::Running,
        ..root.clone()
    };
    assert!(is_task_settled(&reply_subtask, ChannelTaskState::Failed));
    let resumed = released_parent_task(&[root.clone(), reply_subtask.clone()], &reply_subtask, ChannelTaskState::Failed)
        .expect("parent released even when reply subtask fails");
    assert_eq!(resumed.state, ChannelTaskState::Queued);

    let peer_msg = ChannelMessage {
        id: "m-peer".into(),
        channel_id: "ch-1".into(),
        sequence: 2,
        author: ChannelMessageAuthor { kind: ChannelMessageAuthorKind::Agent, id: "a2".into(), name: "Bob".into() },
        task_id: Some("root-1".into()),
        superseded: false,
        message: ConversationMessage::new_simple(
            "m-peer",
            ConversationMessageAuthor::Assistant,
            "Need clarification",
            "2026-11-25T00:00:00Z",
            ConversationMessageStatus::Completed,
        ),
        deleted: None,
        sender_task_id: None,
    };
    let ok = check_channel_reply(Some("m-peer"), Some("Agreed!"), "a1", &root, &[root.clone()], Some(&peer_msg), false, |_| true);
    assert!(matches!(ok, ChannelReplyOutcome::Accepted { .. }));
    let self_refused = check_channel_reply(
        Some("m-peer"),
        Some("Hi"),
        "a2",
        &root,
        std::slice::from_ref(&root),
        Some(&peer_msg),
        false,
        |_| true,
    );
    assert!(matches!(self_refused, ChannelReplyOutcome::Refused { reason } if reason.ends_with("Do not mention this refusal.")));
}

#[test]
fn iter4_channels_and_tags_strict_wire_parity() {
    use openbot_contracts::channels::{
        ChannelAssignment, ChannelBrowser, ChannelCommand, ChannelCreateRoutineToolInput,
        ChannelHandoffToolInput, ChannelHistoryToolInput, ChannelSummary,
        ChannelUpdateRoutineToolInput, CreateChannelToolInput, ListChannelsToolInput,
        SendToChannelToolInput,
    };

    // FileQuoteTarget rejects `"lines": null` across all 3 variants
    assert!(
        serde_json::from_str::<FileQuoteTarget>(r#"{"source":"workspace","path":"a.rs","lines":null}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<FileQuoteTarget>(r#"{"source":"shared","path":"b.rs","lines":null}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<FileQuoteTarget>(
        r#"{"source":"attachment","attachmentId":"att-1","name":"c.rs","lines":null}"#
    )
    .is_err());

    // ChannelBrowser rejects excess keys & invalid member identifiers (`1..=128` UTF-16 units)
    assert!(serde_json::from_str::<ChannelBrowser>(r#"{"kind":"members","extra":1}"#).is_err());
    assert!(serde_json::from_str::<ChannelBrowser>(r#"{"kind":"none","extra":1}"#).is_err());
    assert!(serde_json::from_str::<ChannelBrowser>(r#"{"kind":"member","agentId":"a1","extra":1}"#).is_err());
    assert!(serde_json::from_str::<ChannelBrowser>(r#"{"kind":"member","agentId":""}"#).is_err());
    assert!(serde_json::from_value::<ChannelBrowser>(serde_json::json!({
        "kind": "member",
        "agentId": "x".repeat(129)
    }))
    .is_err());

    // Channel / ChannelDraft / decode_normalized: omitted leadAgentId & null isolated/folders fail;
    // invalid browser fails strict Channel serde but normalizes to ChannelBrowser::None in decode_normalized!
    let base_ch = serde_json::json!({
        "id": "ch-1",
        "name": "Core",
        "title": "Core Team",
        "instructions": "Ship cleanly",
        "members": [{ "agentId": "a1" }],
        "leadAgentId": "a1",
        "archived": false,
        "revision": 1,
        "createdAt": "2026-11-25T00:00:00Z"
    });
    let mut no_lead = base_ch.clone();
    no_lead.as_object_mut().unwrap().remove("leadAgentId");
    assert!(serde_json::from_value::<Channel>(no_lead.clone()).is_err());
    assert!(Channel::decode_normalized(&no_lead).is_err());

    for bad_null_field in ["isolated", "folders"] {
        let mut bad = base_ch.clone();
        bad.as_object_mut().unwrap().insert(bad_null_field.into(), serde_json::Value::Null);
        assert!(serde_json::from_value::<Channel>(bad.clone()).is_err());
        assert!(Channel::decode_normalized(&bad).is_err());
    }

    for invalid_browser in [
        serde_json::Value::Null,
        serde_json::json!({"kind": "members", "extra": 1}),
    ] {
        let mut raw_ch = base_ch.clone();
        raw_ch.as_object_mut().unwrap().insert("browser".into(), invalid_browser);
        assert!(serde_json::from_value::<Channel>(raw_ch.clone()).is_err());
        let normalized = Channel::decode_normalized(&raw_ch).expect("decodeChannel normalizes browser");
        assert_eq!(normalized.draft.browser, Some(ChannelBrowser::None));
    }

    // ChannelCommand required-nullable recipientAgentId / replyToMessageId & Save.update non_null_option
    assert!(serde_json::from_str::<ChannelCommand>(
        r#"{"type":"send","operationId":"op1","channelId":"ch1","text":"hi","replyToMessageId":null,"attachmentDraftIds":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<ChannelCommand>(
        r#"{"type":"stop","operationId":"op1","channelId":"ch1","taskId":"t1"}"#
    )
    .is_err());
    assert!(serde_json::from_value::<ChannelCommand>(serde_json::json!({
        "type": "save",
        "operationId": "op1",
        "channelId": "ch1",
        "draft": {
            "name": "Core",
            "title": "",
            "instructions": "",
            "members": [],
            "leadAgentId": null
        },
        "update": null
    }))
    .is_err());

    // ChannelAssignment: pendingRevision & pendingOutcome are required-nullable (emit `null` when None)
    assert!(serde_json::from_str::<ChannelAssignment>(
        r#"{"id":"as1","channelId":"ch1","taskId":"t1","agentId":"a1","taskRevision":1,"requestMessageId":null,"resources":[],"deliveryId":null,"turnId":null,"state":"queued","throughSequence":0,"summaryVersion":0,"awaitedTaskIds":[]}"#
    )
    .is_err());
    let assign: ChannelAssignment = serde_json::from_str(
        r#"{"id":"as1","channelId":"ch1","taskId":"t1","agentId":"a1","taskRevision":1,"requestMessageId":null,"resources":[],"deliveryId":null,"turnId":null,"state":"queued","throughSequence":0,"summaryVersion":0,"awaitedTaskIds":[],"pendingRevision":null,"pendingOutcome":null}"#,
    )
    .unwrap();
    let assign_json = serde_json::to_value(&assign).unwrap();
    assert_eq!(assign_json["pendingRevision"], serde_json::Value::Null);
    assert_eq!(assign_json["pendingOutcome"], serde_json::Value::Null);

    // ChannelSummary requires `lastMessage` (`ChannelPreview | null`)
    let mut sum_missing_last = base_ch.clone();
    sum_missing_last["unreadCount"] = serde_json::json!(0);
    sum_missing_last["activeTasks"] = serde_json::json!(0);
    assert!(serde_json::from_value::<ChannelSummary>(sum_missing_last.clone()).is_err());
    sum_missing_last["lastMessage"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<ChannelSummary>(sum_missing_last).is_ok());

    // Strict tool schemas (`z.strictObject` / `routineToolArguments`) vs permissive `z.object` tool schemas
    assert!(serde_json::from_str::<CreateChannelToolIcon>(r#"{"name":"sparkles","hue":215,"extra":1}"#).is_err());
    assert!(serde_json::from_str::<CreateChannelToolInput>(
        r#"{"name":"Eng","memberAgentIds":["a1"],"extra":1}"#
    )
    .is_err());
    assert!(serde_json::from_str::<CreateChannelToolInput>(
        r#"{"name":"Eng","memberAgentIds":["a1"],"title":null}"#
    )
    .is_err());
    assert!(serde_json::from_str::<UpdateChannelToolInput>(
        r#"{"channelId":"ch1","extra":1}"#
    )
    .is_err());
    assert!(serde_json::from_str::<UpdateChannelToolInput>(
        r#"{"channelId":"ch1","archived":null}"#
    )
    .is_err());
    assert!(serde_json::from_str::<SendToChannelToolInput>(
        r#"{"channelId":"ch1","text":"hi","extra":1}"#
    )
    .is_err());
    assert!(serde_json::from_str::<ChannelCreateRoutineToolInput>(
        r#"{"name":"R","instruction":"I","schedule":{"kind":"hourly","minute":0},"extra":1}"#
    )
    .is_err());
    assert!(serde_json::from_str::<ChannelUpdateRoutineToolInput>(
        r#"{"routineId":"r1","extra":1}"#
    )
    .is_err());

    // ListChannelsToolInput, ChannelHandoffToolInput, ChannelHistoryToolInput allow extra keys (`z.object`) but reject null on `?: T`
    assert!(serde_json::from_str::<ListChannelsToolInput>(r#"{"archived":true,"extra":1}"#).is_ok());
    assert!(serde_json::from_str::<ListChannelsToolInput>(r#"{"archived":null}"#).is_err());
    assert!(serde_json::from_str::<ChannelHistoryToolInput>(r#"{"beforeSequence":10,"extra":1}"#).is_ok());
    assert!(serde_json::from_str::<ChannelHistoryToolInput>(r#"{"beforeSequence":null}"#).is_err());
    assert!(serde_json::from_str::<ChannelHandoffToolInput>(
        r#"{"recipientAgentId":"a1","task":"t","expectedResult":"r","sourceMessageIds":[],"extra":1}"#
    )
    .is_ok());
}
