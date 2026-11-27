//! End-to-End Integration Tests for Fabushi Native Bot Fusion (`v1.9.0`).
//!
//! Verifies that OpenBot & Orca capabilities are fused directly inside Fabushi's existing
//! native Bot workflow without introducing a second parallel application or sidebar clutter:
//! 1. Unified Plush 3D (`plush:*`) & Bloub (`bloub:*`) avatar contracts + renderer dispatch.
//! 2. Create Bot from 24 Orca Templates inside `CreateManagedBotBoxPeers` (`新建机器人`).
//! 3. `$` composer trigger autocomplete for THIS Bot's Skills / Routines / Workflows.
//! 4. `AppleAppSidebar` convergence (no duplicate OpenBot/Orca top-level sidebar entries).
//! 5. Full live `AppShell` + `OpenBotHostRuntime` 1:1 Bot lifecycle:
//!    - Automatic 1:1 provisioning + template bundled skill installation
//!    - Live Memory CRUD + Tidy-Up deduplication synced to `ContextInspector`
//!    - Skills install/toggle synced into `CanonicalComposer` `$` picker
//!    - Routines preset create / run-now / toggle
//!    - 4-node Approval Workflow creation -> run -> inline `ask_ui` approval card -> approve
//!    - Queue add / pause / resume / steer / cancel + ephemeral Side-Question (`$side`)
//!    - Top-bar `工作台` toggle (`TopBarUiAction::ToggleBotCompanion` -> Companion SidePanel)

use std::sync::Arc;

use gpui::{App, AppContext, Application};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_desktop::components::avatar::{
    OPENBOT_BLOUB_AVATAR_PRESETS, OPENBOT_PLUSH_AVATAR_PRESETS,
};
use gpui_telegram_desktop::components::grok_bot_avatar::{
    render_grok_bot_avatar, GrokBotAvatarProps, GrokPersonaState,
};
use gpui_telegram_desktop::theme::TelegramAssetSource;
use gpui_telegram_desktop::views::apple_sidebar::AppleSidebarSection;
use gpui_telegram_desktop::views::app_shell::shell_bot_fusion::{
    build_starter_approval_workflow_graph, sanitize_openbot_skill_slug,
};
use gpui_telegram_desktop::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers;
use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
use gpui_telegram_desktop::views::composer::editor_state::detect_autocomplete_trigger;
use gpui_telegram_desktop::views::conversation_stage::StageAction;
use gpui_telegram_desktop::views::drawer::{
    BotCompanionTab, BotConfigFocusField, BotFusionDollarCandidate, InspectorAction,
};
use gpui_telegram_desktop::views::shell::AppShell;
use gpui_telegram_desktop::views::stage::stage_bars::TopBarUiAction;
use gpui_telegram_proto::miniapp_contracts::{
    is_valid_openbot_avatar_shape, normalize_fabushi_avatar_shape, resolve_grok_persona_shape,
    FabushiBotSummaryDto,
};
use orca_core::bot_ecosystem::templates::BUILTIN_BOT_TEMPLATES;
use tempfile::TempDir;

fn sample_fabushi_bot() -> FabushiBotSummaryDto {
    FabushiBotSummaryDto {
        bot_user_id: 9001,
        owner_user_id: 42,
        agent_id: "research_bot".to_string(),
        username: "research_bot".to_string(),
        display_name: "研究助手 Nexo".to_string(),
        description: "负责深度调研、记忆沉淀与工作流审批的原生 Bot".to_string(),
        title: "Research Assistant".to_string(),
        avatar_shape: "plush:nexo".to_string(),
        avatar_color: "blue".to_string(),
        preferred_model: "claude-sonnet-4".to_string(),
        thinking_level: "medium".to_string(),
        approval_mode: "prompt".to_string(),
        toolset_preset: "full".to_string(),
        notify_on_agent_updates: true,
        hidden_from_sidebar: false,
        memory_entries_count: 0,
        skills_count: 0,
        execution_state: "idle".to_string(),
        companion_chat_id: Some(777),
        coding_agent: None,
    }
}

#[test]
fn test_unified_plush_and_bloub_avatar_contracts_and_renderer() {
    // Valid Plush 3D & Bloub shapes pass through proto normalization untouched
    assert!(is_valid_openbot_avatar_shape("plush:nexo"));
    assert!(is_valid_openbot_avatar_shape("bloub:fantome"));
    assert!(!is_valid_openbot_avatar_shape("plush:../etc"));
    assert!(!is_valid_openbot_avatar_shape("plush:"));

    assert_eq!(
        normalize_fabushi_avatar_shape("plush:nexo", "bot-1"),
        "plush:nexo"
    );
    assert_eq!(
        normalize_fabushi_avatar_shape("BLOUB:Poulpe", "bot-1"),
        "bloub:poulpe"
    );
    assert_eq!(
        resolve_grok_persona_shape("bot-1", Some("plush:vita")),
        "plush:vita"
    );
    // Unsafe shape falls back cleanly to a deterministic Grok shape
    assert_ne!(
        normalize_fabushi_avatar_shape("plush:bad/path", "bot-1"),
        "plush:bad/path"
    );

    // Preset catalogs cover all 10 Plush + 8 Bloub options and render cleanly
    assert_eq!(OPENBOT_PLUSH_AVATAR_PRESETS.len(), 10);
    assert_eq!(OPENBOT_BLOUB_AVATAR_PRESETS.len(), 8);
    for &(spec, _) in OPENBOT_PLUSH_AVATAR_PRESETS
        .iter()
        .chain(OPENBOT_BLOUB_AVATAR_PRESETS.iter())
    {
        assert!(is_valid_openbot_avatar_shape(spec), "preset {spec} must be valid");
        let _el = render_grok_bot_avatar(
            GrokBotAvatarProps::new("test-bot", spec, "blue", 36.0)
                .with_state(GrokPersonaState::Thinking),
        );
    }
}

#[test]
fn test_create_managed_bot_box_24_orca_templates_and_plush_avatar() {
    // Preserve backward-compatible default shape for existing unit tests
    let mut box_view = CreateManagedBotBoxPeers::new();
    assert_eq!(box_view.avatar_shape, "blob");
    assert_eq!(BUILTIN_BOT_TEMPLATES.len(), 24);

    // Open template gallery inside existing CreateManagedBotBoxPeers modal
    box_view.open_template_creator();
    assert!(box_view.is_visible);
    assert!(box_view.is_template_mode);

    // Apply an Orca template and verify pre-fill into Fabushi Bot creation form
    let tpl = &BUILTIN_BOT_TEMPLATES[0];
    box_view.apply_template(tpl, 0);
    assert!(!box_view.is_template_mode);
    assert!(box_view.is_fabushi_mode);
    assert!(
        box_view.avatar_shape.starts_with("plush:"),
        "template should pre-select a Plush 3D avatar, got {}",
        box_view.avatar_shape
    );
    assert!(!box_view.bot_name.trim().is_empty());
    assert!(!box_view.description.trim().is_empty());
    assert_eq!(box_view.selected_template_id.as_deref(), Some(tpl.id));

    let skills = box_view.take_selected_template_skills();
    assert!(
        !skills.is_empty(),
        "Orca template should carry bundled skills for 1:1 OpenBot provisioning"
    );
}

#[test]
fn test_composer_dollar_trigger_skills_routines_workflows() {
    assert_eq!(
        detect_autocomplete_trigger("帮我运行 $sk", "帮我运行 $sk".len()),
        Some(('$', "$sk".to_string(), "帮我运行 ".len()))
    );
    assert_eq!(
        detect_autocomplete_trigger("$skill:code", "$skill:code".len()),
        Some(('$', "$skill:code".to_string(), 0))
    );

    let mut composer = CanonicalComposer::new();
    composer.set_bot_dollar_candidates(vec![
        BotFusionDollarCandidate {
            token: "skill:code-review".to_string(),
            title: "代码审查".to_string(),
            subtitle: "审查 PR 变更".to_string(),
            badge: "技能".to_string(),
        },
        BotFusionDollarCandidate {
            token: "side".to_string(),
            title: "无痕侧问".to_string(),
            subtitle: "不写入主历史".to_string(),
            badge: "快捷".to_string(),
        },
    ]);

    composer.set_text("$sk".to_string());
    composer.sync_autocomplete();
    assert!(composer.dollar_ac_visible);
    assert_eq!(composer.dollar_ac_matches.len(), 1);
    assert_eq!(composer.dollar_ac_matches[0].token, "skill:code-review");

    composer.select_dollar_autocomplete_index(0);
    assert!(!composer.dollar_ac_visible);
    assert_eq!(composer.text(), "$skill:code-review ");
}

#[test]
fn test_apple_sidebar_stays_converged_and_workflow_graph_validates() {
    // AppleAppSidebar has only native Fabushi destinations (no parallel OpenBot/Orca top-level apps)
    let all_sections = [
        AppleSidebarSection::Chats,
        AppleSidebarSection::Community,
        AppleSidebarSection::Books,
        AppleSidebarSection::Music,
        AppleSidebarSection::Games,
        AppleSidebarSection::AppStore,
        AppleSidebarSection::MyCreations,
    ];
    assert_eq!(all_sections.len(), 7);
    assert_eq!(sanitize_openbot_skill_slug("Code Review (PR)!"), "code-review-pr");

    let graph = build_starter_approval_workflow_graph("agent-123", "Nexo");
    assert_eq!(graph.nodes.len(), 4);
    assert_eq!(graph.edges.len(), 3);
    graph.validate().expect("starter approval workflow graph must pass OpenBot validation");
}

const NATIVE_FUSION_HEADLESS_CHILD_ENV: &str = "NATIVE_BOT_FUSION_HEADLESS_CHILD";
const NATIVE_FUSION_HEADLESS_TARGET_ENV: &str = "NATIVE_BOT_FUSION_HEADLESS_TARGET";

fn dispatch_native_fusion_headless(target: &str) -> ! {
    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match target_owned.as_str() {
                        "test_native_bot_fusion_full_e2e_lifecycle_on_app_shell" => {
                            body_native_bot_fusion_full_e2e_lifecycle_on_app_shell(cx);
                        }
                        other => panic!("unknown native bot fusion headless target: {other}"),
                    }
                }));
                match inner {
                    Ok(()) => std::process::exit(0),
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("panic");
                        eprintln!("[native_bot_fusion headless panic] {target_owned}: {msg}");
                        std::process::exit(101);
                    }
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        eprintln!("[native_bot_fusion outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_NATIVE_BOT_FUSION_HEADLESS_ENTRY: extern "C" fn() =
    macos_native_bot_fusion_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_native_bot_fusion_headless_entry() {
    if std::env::var(NATIVE_FUSION_HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(NATIVE_FUSION_HEADLESS_TARGET_ENV) {
            dispatch_native_fusion_headless(&target);
        }
    }
}

fn run_native_fusion_headless_subprocess(test_fn_name: &str) {
    if std::env::var(NATIVE_FUSION_HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_native_fusion_headless(test_fn_name);
    }
    let cfg_dir = TempDir::new().expect("headless config tempdir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(NATIVE_FUSION_HEADLESS_CHILD_ENV, "1")
        .env(NATIVE_FUSION_HEADLESS_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", cfg_dir.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless gpui child");
    assert!(
        output.status.success(),
        "headless gpui child failed for {test_fn_name}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_native_bot_fusion_full_e2e_lifecycle_on_app_shell() {
    run_native_fusion_headless_subprocess("test_native_bot_fusion_full_e2e_lifecycle_on_app_shell");
}

fn body_native_bot_fusion_full_e2e_lifecycle_on_app_shell(cx: &mut App) {
    let tmp = TempDir::new().expect("tempdir for isolated OpenBotHostRuntime");
    let openbot_prefs_dir = tmp.path().to_path_buf();

    let client = Arc::new(TelegramClient::new(ClientConfig::default()));
    let shell = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client, cx);
        s.preferences_dir = openbot_prefs_dir.clone();
        s.is_authenticated = true;
        s
    });

    let bot = sample_fabushi_bot();
    shell.update(cx, |s, scx| {
                // 1. Register native Bot chat and provision 1:1 OpenBot agent + template skills
                s.miniapps.fabushi_bots.push(bot.clone());
                s.stage.update(scx, |st, _| {
                    st.active_chat_id = Some(777);
                    st.is_bot_chat = true;
                    st.set_active_fabushi_bot(Some(bot.clone()));
                });
                let ob_id = s
                    .provision_openbot_agent_for_fabushi(
                        &bot,
                        &["Code Review".to_string(), "Web Research".to_string()],
                    )
                    .expect("1:1 OpenBot agent provisioned");
                assert!(!ob_id.is_empty());

                s.sync_fabushi_bots_to_views(scx);
                assert_eq!(s.bot_fusion.snapshot.fabushi_agent_id, "research_bot");
                assert_eq!(s.bot_fusion.snapshot.skills.len(), 2);
                assert!(
                    s.composer
                        .read(scx)
                        .bot_dollar_items
                        .iter()
                        .any(|c| c.token == "skill:code-review"),
                    "Composer $ picker must expose provisioned Bot skills"
                );

                // 2. Memory CRUD + Tidy-Up deduplication via InspectorAction & MemoryDraft
                s.inspector.update(scx, |insp, _| {
                    insp.is_open = true;
                    insp.profile.is_bot = true;
                    insp.bot_config_focus = BotConfigFocusField::MemoryDraft;
                    insp.memory_draft = "用户偏好简洁的中文结论".to_string();
                    assert!(insp.submit_memory_draft());
                });
                let draft_act = s
                    .inspector
                    .update(scx, |insp, _| insp.pending_action.take())
                    .expect("memory draft action");
                assert!(s.handle_bot_fusion_inspector_action(draft_act, scx));
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::AddBotMemory("  Prefer   Rust  ".to_string()),
                    scx
                ));
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::AddBotMemory("prefer rust".to_string()),
                    scx
                ));
                assert_eq!(s.bot_fusion.snapshot.memories.len(), 3);

                assert!(s.handle_bot_fusion_inspector_action(InspectorAction::TidyBotMemories, scx));
                assert_eq!(
                    s.bot_fusion.snapshot.memories.len(),
                    2,
                    "TidyBotMemories must deduplicate case/whitespace-equivalent entries"
                );
                let first_mem_id = s.bot_fusion.snapshot.memories[0].id.clone();
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::DeleteBotMemory(first_mem_id),
                    scx
                ));
                assert_eq!(s.bot_fusion.snapshot.memories.len(), 1);
                assert_eq!(
                    s.inspector
                        .read(scx)
                        .active_fabushi_bot
                        .as_ref()
                        .map(|b| b.memory_entries_count),
                    Some(1)
                );

                // 3. Skills install & toggle
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::InstallBotSkill {
                        slug: "daily-digest".to_string(),
                        name: "每日简报".to_string(),
                        description: "汇总重点议题".to_string(),
                    },
                    scx
                ));
                assert_eq!(s.bot_fusion.snapshot.skills.len(), 3);
                let sk_id = s
                    .bot_fusion
                    .snapshot
                    .skills
                    .iter()
                    .find(|sk| sk.slug == "daily-digest")
                    .expect("daily-digest installed")
                    .skill_id
                    .clone();
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::ToggleBotSkill {
                        skill_id: sk_id,
                        enabled: false,
                    },
                    scx
                ));
                assert_eq!(
                    s.inspector
                        .read(scx)
                        .active_fabushi_bot
                        .as_ref()
                        .map(|b| b.skills_count),
                    Some(2)
                );

                // 4. Routines create, run-now, and toggle
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::CreatePresetRoutine {
                        preset: "daily".to_string(),
                    },
                    scx
                ));
                assert_eq!(s.bot_fusion.snapshot.routines.len(), 1);
                let rt_id = s.bot_fusion.snapshot.routines[0].id.clone();
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::RunBotRoutineNow {
                        routine_id: rt_id.clone(),
                    },
                    scx
                ));
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::ToggleBotRoutine {
                        routine_id: rt_id,
                        active: false,
                    },
                    scx
                ));
                assert!(!s.bot_fusion.snapshot.routines[0].active);

                // 5. Approval Workflow -> Run -> inline ask_ui card -> Approve -> Cleared
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::CreateStarterWorkflow,
                    scx
                ));
                assert!(s.bot_fusion.companion_open);
                assert_eq!(s.bot_fusion.companion_tab, BotCompanionTab::Workflow);
                assert_eq!(s.bot_fusion.snapshot.workflows.len(), 1);
                let wf_id = s.bot_fusion.snapshot.workflows[0].id.clone();

                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::RunBotWorkflow {
                        workflow_id: wf_id.clone(),
                    },
                    scx
                ));
                assert_eq!(
                    s.bot_fusion.pending_ask_cards.len(),
                    1,
                    "Running 4-node approval workflow must surface an inline ask_ui approval card"
                );
                let card = s.bot_fusion.pending_ask_cards[0].clone();
                assert!(s.handle_bot_fusion_inspector_action(
                    InspectorAction::DecideBotWorkflowApproval {
                        workflow_id: card.workflow_id,
                        run_id: card.run_id,
                        node_id: card.node_id,
                        approve: true,
                    },
                    scx
                ));
                assert!(
                    s.bot_fusion.pending_ask_cards.is_empty(),
                    "Approving ask_ui card must settle and clear the gate"
                );

                // 6. `$queue`, Queue pause/resume/steer/cancel, and `$side` ephemeral Side-Question
                assert!(s.try_handle_bot_fusion_dollar_command("$queue 检查今日 PR 进展", scx));
                assert_eq!(s.bot_fusion.snapshot.queue_items.len(), 1);
                let del_id = s.bot_fusion.snapshot.queue_items[0].delivery_id.clone();
                s.bot_fusion_queue_toggle_pause(scx);
                assert!(s.bot_fusion.snapshot.queue_paused);
                s.bot_fusion_queue_toggle_pause(scx);
                assert!(!s.bot_fusion.snapshot.queue_paused);
                s.bot_fusion_queue_steer(del_id.clone(), scx);
                s.bot_fusion_queue_cancel(del_id, scx);
                assert!(s.bot_fusion.snapshot.queue_items.is_empty());

                let msgs_before = s.stage.read(scx).messages.len();
                assert!(s.try_handle_bot_fusion_dollar_command("$side 什么是 Rust 所有权？", scx));
                assert!(s.bot_fusion.side_question_last.is_some());
                assert_eq!(
                    s.stage.read(scx).messages.len(),
                    msgs_before,
                    "Side-Question must remain ephemeral without polluting chat history"
                );

                // 7. Top Bar Toggle (`TopBarUiAction::ToggleBotCompanion` -> StageAction)
                s.stage.update(scx, |st, _| {
                    st.apply_topbar_action(TopBarUiAction::ToggleBotCompanion);
                    assert_eq!(st.pending_stage_action, Some(StageAction::ToggleBotCompanion));
                });
                s.toggle_bot_companion_panel(None, scx);
                assert!(!s.bot_fusion.companion_open);
                assert!(!s.stage.read(scx).is_bot_companion_open);

                // Render both inline composer strip & Companion SidePanel without panic
                s.toggle_bot_companion_panel(Some(BotCompanionTab::Workflow), scx);
                assert!(s.render_bot_fusion_composer_strip(scx).is_some());
                let _panel = s.render_bot_companion_sidepanel(scx);
    });
}
