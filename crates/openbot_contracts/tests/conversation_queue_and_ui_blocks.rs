use std::collections::BTreeMap;

use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
    ConversationReaction, ConversationReactionActor, ConversationReadState, ConversationSnapshot,
    ImagePixelSize, MESSAGE_PINS_LIMIT, MESSAGE_PINS_LIST_LIMIT, MESSAGE_REACTIONS,
    MORE_MESSAGE_REACTIONS, MessagePinner, PinnedMessage, PinnedMessageAuthor,
    PinnedMessagesSnapshot, image_aspect_ratio_of, image_pixel_size, is_message_reaction,
    latest_turn_answer,
};
use openbot_contracts::queue::{
    AttachmentKind, AttachmentPreviewKind, AttachmentSummary, QUEUE_EDIT_HOLD_MS, QueueDelivery,
    QueueDeliveryStatus, QueueEditRequest, QueueSender,
};
use openbot_contracts::ui_blocks::{
    ConversationUiBlock, UI_BLOCK_SUBMIT_ACTION_ID, UI_BLOCK_VERSION, UiAction, UiActionStyle,
    UiBlockResponse, UiBlockSpec, UiBlockState, UiBlockStatus, UiBlockValue, UiChoiceOption,
    UiConfirmField, UiFormField, UiOption, is_ui_block_id, ui_block_action_is_privileged,
    ui_block_answers_from_response, ui_block_fallback_questions, ui_block_outcome_text,
    ui_block_response_from_answers, validate_ui_block_response,
};

#[test]
fn conversation_reactions_pins_and_queue_contracts() {
    assert_eq!(QUEUE_EDIT_HOLD_MS, 600_000);
    assert_eq!(MESSAGE_PINS_LIMIT, 50);
    assert_eq!(MESSAGE_PINS_LIST_LIMIT, 200);

    // All 14 preset reactions + ZWJ + skin tones + flags + keycaps pass; multi-emoji & ASCII fail
    for emoji in MESSAGE_REACTIONS.iter().chain(MORE_MESSAGE_REACTIONS.iter()) {
        assert!(is_message_reaction(emoji), "preset {emoji} should be valid");
    }
    assert!(is_message_reaction("👨‍💻"));
    assert!(is_message_reaction("👍🏽"));
    assert!(is_message_reaction("🇺🇸"));
    assert!(is_message_reaction("1️⃣"));
    assert!(is_message_reaction("1\u{20E3}"));
    assert!(is_message_reaction("#️⃣"));
    assert!(is_message_reaction("🏴󠁧󠁢󠁥󠁮󠁧󠁿"));
    assert!(is_message_reaction("🏴󠁧󠁢󠁳󠁣󠁴󠁿"));
    assert!(is_message_reaction("🏴󠁧󠁢󠁷󠁬󠁳󠁿"));
    assert!(is_message_reaction("👨🏻‍❤️‍💋‍👨🏼"));
    assert!(is_message_reaction("🫱🏻‍🫲🏿"));
    assert!(is_message_reaction("❤️"));
    assert!(is_message_reaction("❤"));
    assert!(is_message_reaction("©️"));
    assert!(is_message_reaction("©"));
    assert!(!is_message_reaction("👍👍"));
    assert!(!is_message_reaction("👍🏻🏿"));
    assert!(!is_message_reaction("🤖🏻"));
    assert!(!is_message_reaction("▰"));
    assert!(!is_message_reaction("🠀"));
    assert!(!is_message_reaction("👍\u{20E3}"));
    assert!(!is_message_reaction("🏴\u{E0020}"));
    assert!(!is_message_reaction("👍\u{E0020}"));
    assert!(!is_message_reaction("🏴󠁧󠁿"));
    assert!(!is_message_reaction("🇦🇦"));
    assert!(!is_message_reaction("thumbsup"));
    assert!(!is_message_reaction(""));

    let sz = image_pixel_size(Some("1024 × 512")).expect("parsed pixel size");
    assert_eq!(sz, ImagePixelSize { width: 1024, height: 512 });
    assert_eq!(format!("{:?}", image_aspect_ratio_of(sz)), "Landscape");

    let mut msg = ConversationMessage::new_simple(
        "msg-1",
        ConversationMessageAuthor::Assistant,
        "Final answer here",
        "2026-11-25T00:00:00Z",
        ConversationMessageStatus::Completed,
    );
    msg.turn_id = Some("turn-1".into());
    msg.reactions = Some(vec![ConversationReaction {
        emoji: "🎉".into(),
        actor: ConversationReactionActor::User,
    }]);
    assert!(msg.is_valid());
    assert!(msg.is_agent_answer());
    assert_eq!(latest_turn_answer(&[msg.clone()], "turn-1"), Some(&msg));

    let pin = PinnedMessage {
        message_id: "msg-1".into(),
        task_id: None,
        thread_title: Some("Main".into()),
        pinned_at: "2026-11-25T00:00:00Z".into(),
        pinned_by: MessagePinner::Agent { agent_id: "a1".into() },
        author: PinnedMessageAuthor::Agent,
        sender: None,
        sender_agent_id: Some("a1".into()),
        created_at: "2026-11-25T00:00:00Z".into(),
        text: "Final answer here".into(),
    };
    let snap = PinnedMessagesSnapshot { agent_id: "a1".into(), pins: vec![pin] };
    assert!(snap.is_valid());

    let item = QueueDelivery {
        id: "del-1".into(),
        message_id: "msg-2".into(),
        recipient_agent_id: "a1".into(),
        sender: QueueSender::User,
        text: "Next task".into(),
        attachments: vec![AttachmentSummary {
            id: "att-1".into(),
            name: "diagram.png".into(),
            size: 1024,
            kind: AttachmentKind::Image,
            mime_type: "image/png".into(),
            preview_kind: AttachmentPreviewKind::Image,
            preview_url: None,
        }],
        reply_to_message_id: None,
        status: QueueDeliveryStatus::Queued,
        position: Some(1),
        turn_id: None,
        error: None,
        created_at: "2026-11-25T00:00:00Z".into(),
        editing: Some(true),
        expects_reply: None,
        steer_fallback: None,
    };
    assert!(item.is_valid());

    let edit = QueueEditRequest::Save {
        delivery_id: "del-1".into(),
        edit_id: "ed-1".into(),
        text: "Updated".into(),
        keep_attachment_ids: vec!["att-1".into()],
        attachment_draft_ids: vec![],
    };
    assert!(edit.validate().is_ok());

    // ConversationSnapshot (activeTurnId: string | null) & ConversationReadState wire schemas
    let snap_wire = serde_json::json!({
        "agentId": "a1",
        "threadId": "th-1",
        "activeTurnId": null,
        "revision": 7,
        "messages": [msg]
    });
    let decoded_snap: ConversationSnapshot = serde_json::from_value(snap_wire.clone()).unwrap();
    assert_eq!(decoded_snap.active_turn_id, None);
    assert!(decoded_snap.is_valid());
    assert_eq!(serde_json::to_value(&decoded_snap).unwrap()["activeTurnId"], serde_json::Value::Null);

    let read_wire = serde_json::json!({
        "unreadCount": 3,
        "firstUnreadMessageId": "msg-1",
        "throughMessageId": null
    });
    let decoded_read: ConversationReadState = serde_json::from_value(read_wire.clone()).unwrap();
    assert_eq!(decoded_read.unread_count, 3);
    assert_eq!(decoded_read.first_unread_message_id.as_deref(), Some("msg-1"));
    assert_eq!(decoded_read.through_message_id, None);
    assert!(decoded_read.is_valid());
    assert_eq!(serde_json::to_value(&decoded_read).unwrap(), read_wire);

    // Omitted required nullable (`T | null`) keys must fail deserialization
    assert!(
        serde_json::from_value::<ConversationSnapshot>(serde_json::json!({
            "agentId": "a1",
            "threadId": null,
            "revision": 0,
            "messages": []
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ConversationSnapshot>(serde_json::json!({
            "agentId": "a1",
            "activeTurnId": null,
            "revision": 0,
            "messages": []
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ConversationReadState>(serde_json::json!({
            "unreadCount": 1,
            "throughMessageId": "m5"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ConversationReadState>(serde_json::json!({
            "unreadCount": 1,
            "firstUnreadMessageId": "m1"
        }))
        .is_err()
    );
}

#[test]
fn ui_blocks_all_four_kinds_secret_redaction_and_fallback_roundtrip() {
    // Reserved ID guards
    assert!(is_ui_block_id("approve_deploy"));
    assert!(!is_ui_block_id("_hidden"));
    assert!(!is_ui_block_id("botId"));
    assert!(!is_ui_block_id("recipientBotId"));
    assert!(!is_ui_block_id("recipientBotIds"));
    assert!(!is_ui_block_id("senderBotId"));

    // 1. Confirm block + privileged action check
    let confirm = UiBlockSpec::Confirm {
        title: "Deploy to production?".into(),
        danger: Some(true),
        confirm_hold: Some(1_200),
        fields: Some(vec![UiConfirmField::Select {
            label: "Region".into(),
            select: "region".into(),
            options: vec!["us-east".into(), "eu-west".into()],
        }]),
        preview: Some("git push prod main".into()),
        actions: vec![
            UiAction { id: "go".into(), label: "Deploy".into(), style: Some(UiActionStyle::Primary), confirm: None },
            UiAction { id: "no".into(), label: "Cancel".into(), style: Some(UiActionStyle::Ghost), confirm: None },
        ],
    };
    assert!(confirm.validate());
    assert!(ui_block_action_is_privileged(&confirm, "go"));
    assert!(!ui_block_action_is_privileged(&confirm, "no"));

    // 2. QuickReplies block
    let qr = UiBlockSpec::QuickReplies {
        title: Some("Pick next step".into()),
        options: vec![
            UiOption { id: "tests".into(), label: "Run tests".into() },
            UiOption { id: "ship".into(), label: "Ship PR".into() },
        ],
        allow_text: Some(true),
    };
    assert!(qr.validate());

    // 3. Choice block
    let choice = UiBlockSpec::Choice {
        title: "Select targets".into(),
        multiple: Some(false),
        options: vec![
            UiChoiceOption { id: "linux".into(), label: "Linux".into(), meta: Some("x86_64".into()), selected: Some(true) },
            UiChoiceOption { id: "mac".into(), label: "macOS".into(), meta: None, selected: None },
        ],
        submit: Some("Continue".into()),
    };
    assert!(choice.validate());

    // 4. Form block with required secret field + stored redaction tolerance!
    let form = UiBlockSpec::Form {
        title: "Connect API".into(),
        fields: vec![
            UiFormField::Text {
                id: "endpoint".into(),
                label: "Endpoint".into(),
                required: Some(true),
                placeholder: None,
                value: None,
                secret: None,
            },
            UiFormField::Text {
                id: "token".into(),
                label: "API Token".into(),
                required: Some(true),
                placeholder: None,
                value: None,
                secret: Some(true),
            },
            UiFormField::Date {
                id: "expires".into(),
                label: "Expires".into(),
                required: Some(false),
                value: Some("2026-12-31".into()),
            },
        ],
        submit: None,
    };
    assert!(form.validate());

    // Live submission WITHOUT required secret fails (`stored = false`), WITH secret succeeds
    let mut vals = BTreeMap::new();
    vals.insert("endpoint".into(), UiBlockValue::Text("https://api.example.com".into()));
    let redacted_resp = UiBlockResponse {
        action_id: UI_BLOCK_SUBMIT_ACTION_ID.into(),
        values: Some(vals.clone()),
        text: None,
    };
    assert!(validate_ui_block_response(&form, &redacted_resp, false).is_none());
    assert!(validate_ui_block_response(&form, &redacted_resp, true).is_some());

    // Stored ConversationUiBlock with redacted secret passes full validation
    let block = ConversationUiBlock {
        version: UI_BLOCK_VERSION,
        block_id: "blk-1".into(),
        spec: form.clone(),
        state: UiBlockState {
            status: UiBlockStatus::Answered,
            response: Some(redacted_resp),
            responded_by: None,
            responded_at: Some("2026-11-25T00:00:00Z".into()),
            outcome: Some("Endpoint: https://api.example.com".into()),
            auto_answer: None,
        },
        vault: None,
        api_key: Some(true),
        location: None,
    };
    assert!(block.validate());

    // Fallback prompt <-> UiBlockResponse round-trip
    let qs = ui_block_fallback_questions(&confirm);
    assert_eq!(qs.len(), 2);
    let mut ans = BTreeMap::new();
    ans.insert("action".into(), vec!["Deploy".into()]);
    ans.insert("region".into(), vec!["eu-west".into()]);
    let derived = ui_block_response_from_answers(&confirm, &ans).expect("structured confirm response");
    assert_eq!(derived.action_id, "go");
    let back = ui_block_answers_from_response(&confirm, &derived);
    assert_eq!(back.get("region"), Some(&vec!["eu-west".to_string()]));
    assert_eq!(ui_block_outcome_text(&confirm, &derived), "Deploy · eu-west");
}

#[test]
fn iter4_queue_conversation_and_ui_blocks_strict_wire_parity() {
    use openbot_contracts::conversation::{
        AgentPromptQuestion, ConversationPlan, ConversationTurnTrace,
    };
    use openbot_contracts::queue::QueueSnapshot;

    // 1. QueueSnapshot tri-state `hold?: QueueHold | null` + QueueDelivery & AttachmentSummary required nullables
    let q_omitted: QueueSnapshot =
        serde_json::from_str(r#"{"agentId":"a1","deliveries":[]}"#).unwrap();
    assert_eq!(q_omitted.hold, None);
    assert!(q_omitted.is_valid());

    let q_null: QueueSnapshot =
        serde_json::from_str(r#"{"agentId":"a1","deliveries":[],"hold":null}"#).unwrap();
    assert_eq!(q_null.hold, Some(None));
    assert!(q_null.is_valid());

    assert!(
        serde_json::from_str::<QueueSnapshot>(r#"{"agentId":"a1","deliveries":[],"welcome":null}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<AttachmentSummary>(
        r#"{"id":"att1","name":"a.png","size":10,"kind":"image","mimeType":"image/png","previewKind":"image"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<QueueDelivery>(
        r#"{"id":"d1","messageId":"m1","recipientAgentId":"a1","sender":{"kind":"user"},"text":"hi","attachments":[],"status":"queued","position":1,"turnId":null,"error":null,"createdAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());

    // 2. ConversationMessage rejects explicit null on `?: T` fields; TurnTrace enforces JS safe-integer tokens
    assert!(serde_json::from_str::<ConversationMessage>(
        r#"{"id":"m1","author":"user","text":"hi","createdAt":"2026-11-25T00:00:00Z","status":"completed","steered":null}"#
    )
    .is_err());
    let msg_null_reply: ConversationMessage = serde_json::from_str(
        r#"{"id":"m1","author":"user","text":"hi","createdAt":"2026-11-25T00:00:00Z","status":"completed","replyToMessageId":null,"reaction":null}"#,
    )
    .unwrap();
    assert_eq!(msg_null_reply.reply_to_message_id, Some(None));
    assert_eq!(msg_null_reply.reaction, Some(None));

    assert!(serde_json::from_str::<AgentPromptQuestion>(
        r#"{"id":"q1","header":"H","question":"Q","isSecret":false}"#
    )
    .is_err());
    assert!(serde_json::from_str::<ConversationPlan>(r#"{"steps":[]}"#).is_err());

    let trace_ok: ConversationTurnTrace = serde_json::from_value(serde_json::json!({
        "startedAt": "2026-11-25T00:00:00Z",
        "endedAt": null,
        "steps": [],
        "tokens": 9_007_199_254_740_991u64
    }))
    .unwrap();
    assert!(trace_ok.is_valid());
    let mut trace_overflow = trace_ok;
    trace_overflow.tokens = Some(9_007_199_254_740_992u64);
    assert!(!trace_overflow.is_valid());

    // 3. Challenger 2 Defect 3.2 reproducers on UiBlockSpec + UiConfirmField explicit `"select"` key branch
    assert!(serde_json::from_str::<UiBlockSpec>(
        r#"{"type":"quick_replies","title":null,"options":[{"id":"ok","label":"OK"}]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<UiBlockSpec>(
        r#"{"type":"confirm","title":"T","danger":null,"actions":[{"id":"ok","label":"OK"}]}"#
    )
    .is_err());

    // Presence of `"select"` key must NEVER fall through to `UiConfirmField::Value`
    assert!(serde_json::from_str::<UiConfirmField>(
        r#"{"label":"Env","select":null,"value":"prod"}"#
    )
    .is_err());
    let bad_select_spec: UiBlockSpec = serde_json::from_str(
        r#"{"type":"confirm","title":"T","fields":[{"label":"Env","select":"bad id!","options":["prod"],"value":"prod"}],"actions":[{"id":"ok","label":"OK"}]}"#,
    )
    .unwrap();
    assert!(!bad_select_spec.validate());

    // ui_block_outcome_text omits ` · ` separator when confirm field value is empty string
    let confirm_spec = UiBlockSpec::Confirm {
        title: "Ship?".into(),
        danger: None,
        confirm_hold: None,
        fields: None,
        preview: None,
        actions: vec![UiAction {
            id: "go".into(),
            label: "Deploy".into(),
            style: None,
            confirm: None,
        }],
    };
    let mut empty_vals = BTreeMap::new();
    empty_vals.insert("env".into(), UiBlockValue::Text(String::new()));
    let resp = UiBlockResponse {
        action_id: "go".into(),
        values: Some(empty_vals),
        text: None,
    };
    assert_eq!(ui_block_outcome_text(&confirm_spec, &resp), "Deploy");
}
