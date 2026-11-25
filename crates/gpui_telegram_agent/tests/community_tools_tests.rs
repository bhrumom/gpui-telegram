use gpui_telegram_agent::community_tools::{community_tool_specs, is_community_tool, COMMUNITY_TOOL_NAMES};
use gpui_telegram_agent::hermes::toolsets::{
    all_builtin_tool_specs, export_anthropic_tools, export_openai_tools, find_tool_spec,
    BoxFuture, HermesToolContext, HermesToolExecutor, PlatformImBridge, PlatformImCaller,
    ToolsetId, ToolsetPreset,
};
use gpui_telegram_agent::{ApprovalMode, ToolRiskLevel};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Debug, Clone)]
struct RecordedCall {
    tool_name: String,
    args: Value,
    caller: PlatformImCaller,
}

#[derive(Default)]
struct RecordingBridge {
    calls: Mutex<Vec<RecordedCall>>,
}

impl RecordingBridge {
    fn snapshot(&self) -> Vec<RecordedCall> {
        self.calls.lock().clone()
    }
}

impl PlatformImBridge for RecordingBridge {
    fn execute_im_tool<'a>(
        &'a self,
        tool_name: &'a str,
        args: &'a Value,
        caller: PlatformImCaller,
    ) -> BoxFuture<'a, Result<Value, String>> {
        self.calls.lock().push(RecordedCall {
            tool_name: tool_name.to_string(),
            args: args.clone(),
            caller: caller.clone(),
        });
        let name = tool_name.to_string();
        Box::pin(async move {
            Ok(json!({
                "ok": true,
                "tool": name,
                "user_id": caller.user_id,
                "bot_id": caller.bot_id,
            }))
        })
    }
}

#[test]
fn test_community_tools_registration_risk_levels_and_tir_schema_safety() {
    assert_eq!(ToolsetId::all().len(), 29);
    assert_eq!(COMMUNITY_TOOL_NAMES.len(), 9);

    let all_specs = all_builtin_tool_specs();
    let openai = export_openai_tools(&all_specs);
    let anthropic = export_anthropic_tools(&all_specs);
    assert_eq!(openai.len(), all_specs.len());
    assert_eq!(anthropic.len(), all_specs.len());

    const FORBIDDEN_TIR_PATH_KEYS: &[&str] = &[
        "target",
        "source",
        "path",
        "file_path",
        "cwd",
        "workdir",
        "working_dir",
        "output_path",
        "audio_path",
        "image_path",
        "destination",
    ];

    let comm_specs = community_tool_specs();
    assert_eq!(comm_specs.len(), 9);
    for &name in COMMUNITY_TOOL_NAMES {
        assert!(is_community_tool(name));
        let spec = find_tool_spec(name).unwrap_or_else(|| panic!("Missing tool spec '{name}'"));
        assert_eq!(spec.toolset, ToolsetId::PlatformIm);

        if let Some(props) = spec
            .parameters_schema
            .get("properties")
            .and_then(Value::as_object)
        {
            for key in props.keys() {
                assert!(
                    !FORBIDDEN_TIR_PATH_KEYS.contains(&key.as_str()),
                    "Tool '{name}' property '{key}' collides with TIR PATH_KEYS!"
                );
            }
        }
    }

    for ro in ["community_browse_feed", "community_search", "community_get_post"] {
        let s = find_tool_spec(ro).unwrap();
        assert_eq!(s.risk_level, ToolRiskLevel::ReadOnly);
        assert!(s.read_only);
        assert!(!s.requires_approval);
    }
    let mod_spec = find_tool_spec("community_mod_action").unwrap();
    assert_eq!(mod_spec.risk_level, ToolRiskLevel::High);
    assert!(!mod_spec.read_only);
    assert!(mod_spec.requires_approval);

    assert!(!ToolsetPreset::Minimal.toolsets().contains(&ToolsetId::PlatformIm));
    assert!(ToolsetPreset::HermesTelegram.toolsets().contains(&ToolsetId::PlatformIm));
    assert!(ToolsetPreset::Safe.toolsets().contains(&ToolsetId::PlatformIm));
    assert!(ToolsetPreset::Full.toolsets().contains(&ToolsetId::PlatformIm));
}

#[tokio::test]
async fn test_minimal_preset_and_unattached_bridge_reject_cleanly() {
    let dir = TempDir::new().expect("tempdir");
    let bridge = Arc::new(RecordingBridge::default());

    let ctx_min = HermesToolContext::new(dir.path())
        .with_session("sess_min", 1)
        .with_chat_and_bot(None, Some(600001))
        .with_preset(ToolsetPreset::Minimal)
        .with_platform_im_bridge(bridge.clone());

    for &name in COMMUNITY_TOOL_NAMES {
        let res = HermesToolExecutor::execute(name, json!({}), &ctx_min).await;
        assert!(!res.success, "Minimal preset must disable {name}");
    }
    assert!(
        bridge.snapshot().is_empty(),
        "Disabled toolset must never invoke bridge"
    );

    let ctx_no_bridge = HermesToolContext::new(dir.path())
        .with_session("sess_nobridge", 1)
        .with_chat_and_bot(None, Some(600001))
        .with_preset(ToolsetPreset::HermesTelegram);
    let res = HermesToolExecutor::execute(
        "community_browse_feed",
        json!({ "tab": "for_you" }),
        &ctx_no_bridge,
    )
    .await;
    assert!(!res.success);
    assert!(res.error.unwrap_or_default().contains("PlatformImBridge"));
}

#[tokio::test]
async fn test_approval_mode_deny_blocks_mod_action_and_bot_caller_dispatches_all_9_tools() {
    let dir = TempDir::new().expect("tempdir");
    let bridge = Arc::new(RecordingBridge::default());

    // ApprovalMode::Deny blocks community_mod_action BEFORE calling bridge
    let ctx_deny = HermesToolContext::new(dir.path())
        .with_session("sess_deny", 1)
        .with_chat_and_bot(Some(555), Some(600001))
        .with_preset(ToolsetPreset::Full)
        .with_approval_mode(ApprovalMode::Deny)
        .with_platform_im_bridge(bridge.clone());

    let denied = HermesToolExecutor::execute(
        "community_mod_action",
        json!({
            "community_slug": "rust",
            "action": "remove",
            "target_kind": "post",
            "target_id": "post_1",
            "reason": "spam"
        }),
        &ctx_deny,
    )
    .await;
    assert!(!denied.success, "community_mod_action must be denied under ApprovalMode::Deny");
    assert!(
        bridge.snapshot().is_empty(),
        "Denied high-risk mod action must NEVER reach the bridge"
    );

    // Low-risk / read-only tools still dispatch under ApprovalMode::Deny
    let browse = HermesToolExecutor::execute(
        "community_browse_feed",
        json!({ "tab": "for_you", "sort": "algo", "actor_filter": "bots_only" }),
        &ctx_deny,
    )
    .await;
    assert!(browse.success, "browse should succeed: {:?}", browse.error);
    let snap = bridge.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].tool_name, "community_browse_feed");
    assert_eq!(snap[0].caller.user_id, None);
    assert_eq!(snap[0].caller.bot_id, Some(600001));
    assert!(
        snap[0].args.get("chat_id").is_none(),
        "Community tools must NOT receive spurious ctx.chat_id injection!"
    );

    // Human caller context forwards user_id cleanly with bot_id = None
    let ctx_human = HermesToolContext::new(dir.path())
        .with_session("sess_human", 1)
        .with_user_id(Some(100001))
        .with_preset(ToolsetPreset::HermesTelegram)
        .with_platform_im_bridge(bridge.clone());
    let h_res = HermesToolExecutor::execute(
        "community_browse_feed",
        json!({ "tab": "following" }),
        &ctx_human,
    )
    .await;
    assert!(h_res.success);
    let last_call = bridge.snapshot().pop().unwrap();
    assert_eq!(last_call.tool_name, "community_browse_feed");
    assert_eq!(last_call.caller.user_id, Some(100001));
    assert_eq!(last_call.caller.bot_id, None);

    // Full dispatch of all 9 tools under pure Bot caller + Prompt/Yolo approval
    let ctx_bot = HermesToolContext::new(dir.path())
        .with_session("sess_bot", 1)
        .with_chat_and_bot(Some(777), Some(600002))
        .with_preset(ToolsetPreset::HermesTelegram)
        .with_approval_mode(ApprovalMode::Prompt)
        .with_platform_im_bridge(bridge.clone());

    let payloads: Vec<(&str, Value)> = vec![
        ("community_browse_feed", json!({ "tab": "popular", "sort": "hot", "limit": 10 })),
        ("community_search", json!({ "query": "rust gpui", "tab": "top" })),
        ("community_get_post", json!({ "post_id": "post_1", "comment_sort": "best" })),
        ("community_create_post", json!({ "community_slug": "rust", "body": "Hello from pure bot!" })),
        ("community_comment", json!({ "post_id": "post_1", "body": "Bot reply!" })),
        ("community_vote", json!({ "post_id": "post_1", "direction": 1 })),
        ("community_quote_post", json!({ "quoted_post_id": "post_1", "body": "Great point!" })),
        ("community_mod_action", json!({ "community_slug": "rust", "action": "lock", "target_kind": "post", "target_id": "post_1" })),
        ("community_follow", json!({ "actor_id": "u:100001", "follow": true })),
    ];
    for (tool, args) in payloads {
        let res = HermesToolExecutor::execute(tool, args, &ctx_bot).await;
        assert!(res.success, "Tool '{tool}' failed unexpectedly: {:?}", res.error);
    }

    // Argument validation checks (must reject before reaching bridge)
    let before = bridge.snapshot().len();
    let invalid_cases = vec![
        ("community_vote", json!({ "post_id": "post_1", "direction": 2 })),
        ("community_vote", json!({ "direction": 1 })),
        ("community_search", json!({ "query": "   " })),
        ("community_create_post", json!({ "body": "   " })),
        ("community_mod_action", json!({ "community_slug": "rust", "action": "nuke_everything", "target_kind": "post", "target_id": "p1" })),
        ("community_mod_action", json!({ "community_slug": "!", "action": "remove", "target_kind": "post", "target_id": "p1" })),
        ("community_follow", json!({ "action": "follow" })),
    ];
    for (tool, args) in invalid_cases {
        let res = HermesToolExecutor::execute(tool, args, &ctx_bot).await;
        assert!(!res.success, "Invalid call to '{tool}' must be rejected");
    }
    assert_eq!(bridge.snapshot().len(), before, "Invalid args must never reach bridge");
}
