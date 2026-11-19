//! Phase 4 Tests: 4 Dark-Minimalist GPUI Cards, Native Chinese IME + Password Masking,
//! Zeroized Draft Lifecycle, Click-Captcha Coordinate Clamping, and Local-Only MTProto Isolation.

use std::collections::HashSet;
use std::sync::Arc;

use bot_browser::orchestrator::{BotBrowserOrchestrator, OrchestratorConfig};
use bot_browser::vault::{CredentialVault, MemoryMasterKeyStore};
use bot_browser::{
    AssistCardPayload, AssistCardStatus, BotBrowserEvent, ElementRect, NormalizedClickPoint,
    ReusePolicy,
};
use gpui::{point, px, size, App, AppContext, Application, Bounds, Entity};
use gpui_telegram_client::net::http_client::HistoricalMessage;
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_client::ChatSummary;
use gpui_telegram_desktop::theme::TelegramAssetSource;
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_authoritative_chat_history, apply_authoritative_chat_list,
    apply_fenced_history_response, select_authoritative_dialog, SessionRequestFence,
};
use gpui_telegram_desktop::views::stage::cards::{
    pixel_to_normalized_click_point, BotAssistCardAction, BotAssistCardModel, BotAssistClickDraft,
    BotAssistLoginDraft, BotAssistLoginField, DeliveryStatus, MessageContent, MessageDirection,
    MessageItemModel, BOT_ASSIST_LOCAL_MSG_ID_BASE,
};
use gpui_telegram_desktop::views::{AppShell, ChatSidebar, ContextInspector, ConversationStage};

fn sample_vault_login_payload(
    card_id: &str,
    status: AssistCardStatus,
    high_sensitivity: bool,
) -> AssistCardPayload {
    AssistCardPayload::VaultLogin {
        card_id: card_id.to_string(),
        task_id: "task_1".to_string(),
        session_id: "sess_1".to_string(),
        chat_id: Some(77),
        origin: if high_sensitivity {
            "https://dashboard.stripe.com".to_string()
        } else {
            "https://example.com".to_string()
        },
        target_url: if high_sensitivity {
            "https://dashboard.stripe.com/login".to_string()
        } else {
            "https://example.com/login".to_string()
        },
        site_title: "Example Login".to_string(),
        suggested_reuse_policy: if high_sensitivity {
            ReusePolicy::ConfirmEveryTime
        } else {
            ReusePolicy::AutoReuse
        },
        is_high_sensitivity: high_sensitivity,
        status,
        status_note: None,
        saved_credential_id: None,
        saved_account_hint: None,
        created_at_ms: 1_700_000_000_000,
    }
}

fn make_local_card_msg(msg_id: u64, payload: AssistCardPayload) -> MessageItemModel {
    MessageItemModel {
        id: msg_id,
        sender_id: 0,
        sender_name: "安全助手".to_string(),
        avatar_initials: "🔒".to_string(),
        avatar_color: 0x1E293B,
        direction: MessageDirection::Incoming,
        timestamp: "12:00".to_string(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: Vec::new(),
        content: MessageContent::LocalBotAssistCard(BotAssistCardModel::new(payload)),
        bot_keyboard: None,
        is_pinned: false,
    }
}

fn make_text_msg(msg_id: u64, text: &str, is_edited: bool) -> MessageItemModel {
    MessageItemModel {
        id: msg_id,
        sender_id: 10,
        sender_name: "Bot".to_string(),
        avatar_initials: "BT".to_string(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Incoming,
        timestamp: "12:00".to_string(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: Vec::new(),
        content: MessageContent::Text {
            text: text.to_string(),
            is_edited,
        },
        bot_keyboard: None,
        is_pinned: false,
    }
}

#[test]
fn bot_assist_login_draft_ime_cjk_masking_tab_cycling_and_zeroize() {
    let mut draft = BotAssistLoginDraft::new(true);
    assert_eq!(draft.active_field, None);

    // Tab cycles None -> Username -> Password -> TotpSecret -> Username
    assert_eq!(draft.cycle_field(false), BotAssistLoginField::Username);
    assert!(draft.insert_text("张三你好世界"));
    assert_eq!(draft.active_text(), "张三你好世界");

    // CJK backspace pops one Unicode scalar character cleanly without corrupting UTF-8
    assert!(draft.backspace(false));
    assert_eq!(draft.username.as_str(), "张三你好世");
    assert!(draft.backspace(false));
    assert_eq!(draft.username.as_str(), "张三你好");

    // Tab -> Password
    assert_eq!(draft.cycle_field(false), BotAssistLoginField::Password);
    assert!(draft.insert_text("s3cr3t"));
    assert_eq!(draft.masked_password(), "••••••");
    assert_eq!(draft.password_display(), "••••••");
    draft.show_password = true;
    assert_eq!(draft.password_display(), "s3cr3t");
    draft.show_password = false;
    assert_eq!(draft.password_display(), "••••••");

    // Tab -> TotpSecret
    assert_eq!(draft.cycle_field(false), BotAssistLoginField::TotpSecret);
    assert!(draft.insert_text("JBSWY3DPEHPK3PXP"));

    // Reverse Tab -> Password
    assert_eq!(draft.cycle_field(true), BotAssistLoginField::Password);

    // Debug output must NEVER expose secret values
    let dbg_draft = format!("{:?}", draft);
    assert!(!dbg_draft.contains("张三你好"));
    assert!(!dbg_draft.contains("s3cr3t"));
    assert!(!dbg_draft.contains("JBSWY3DPEHPK3PXP"));
    assert!(dbg_draft.contains("[REDACTED]"));

    // Take submission extracts secrets and immediately zeroizes draft buffers
    let submission = draft
        .take_submission("card_login_1")
        .expect("valid credentials should submit");
    assert_eq!(submission.card_id, "card_login_1");
    assert_eq!(submission.username.as_str(), "张三你好");
    assert_eq!(submission.password.as_str(), "s3cr3t");
    assert_eq!(
        submission.totp_secret.as_ref().map(|s| s.as_str()),
        Some("JBSWY3DPEHPK3PXP")
    );
    assert!(submission.allow_auto_reuse);

    let dbg_sub = format!("{:?}", submission);
    assert!(!dbg_sub.contains("张三你好"));
    assert!(!dbg_sub.contains("s3cr3t"));
    assert!(!dbg_sub.contains("JBSWY3DPEHPK3PXP"));

    assert!(draft.username.is_empty());
    assert!(draft.password.is_empty());
    assert!(draft.totp_secret.is_empty());
    assert_eq!(draft.active_field, None);
    assert!(!draft.show_password);
}

#[test]
fn apply_authoritative_chat_history_preserves_local_bot_cards_across_reloads() {
    let mut stage = ConversationStage::new();
    stage.set_chat(77, "Assistant Bot", "bot", "AB", 0x2AABEE);

    // Initial stage has 1 remote message + 1 strictly local bot card
    let remote_v1 = make_text_msg(10, "Original remote text", false);
    let local_card = make_local_card_msg(
        BOT_ASSIST_LOCAL_MSG_ID_BASE + 1,
        sample_vault_login_payload("card_persist_1", AssistCardStatus::AwaitingUser, false),
    );
    stage.add_message(remote_v1);
    stage.add_message(local_card);
    assert_eq!(stage.messages.len(), 2);

    // Server reloads history: message 10 is edited, message 11 is newly added; server knows nothing of local_card
    let server_reload = vec![
        make_text_msg(10, "Updated remote text from server", true),
        make_text_msg(11, "New remote reply", false),
    ];
    apply_authoritative_chat_history(&mut stage, 77, server_reload, &HashSet::new());

    assert_eq!(stage.messages.len(), 3);
    assert_eq!(stage.messages[0].id, 10);
    assert!(matches!(
        &stage.messages[0].content,
        MessageContent::Text { text, is_edited: true } if text == "Updated remote text from server"
    ));
    assert_eq!(stage.messages[1].id, 11);
    assert_eq!(stage.messages[2].id, BOT_ASSIST_LOCAL_MSG_ID_BASE + 1);
    assert!(stage.messages[2].is_local_only());
    assert!(matches!(
        &stage.messages[2].content,
        MessageContent::LocalBotAssistCard(card) if card.card_id() == "card_persist_1"
    ));

    // Also verify fenced history reload for an inactive background chat preserves local card
    let mut sidebar = ChatSidebar::new();
    let mut inspector = ContextInspector::new();
    apply_authoritative_chat_list(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        &[
            ChatSummary {
                chat_id: 77,
                title: "Assistant Bot".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_000),
            },
            ChatSummary {
                chat_id: 88,
                title: "Other Chat".to_string(),
                chat_type: 0,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(false),
                avatar_color: Some(0x8E44AD),
                updated_at: Some(1_700_000_000_000),
            },
        ],
    );
    assert!(select_authoritative_dialog(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        77
    ));
    stage.upsert_bot_assist_card(
        77,
        BOT_ASSIST_LOCAL_MSG_ID_BASE + 2,
        sample_vault_login_payload("card_persist_2", AssistCardStatus::AwaitingUser, false),
        "12:05".to_string(),
    );
    // Switch away to chat 88 so 77 is in background `chat_histories`
    assert!(select_authoritative_dialog(
        &mut sidebar,
        &mut stage,
        &mut inspector,
        88
    ));

    let fence = SessionRequestFence {
        auth_generation: 1,
        request_generation: 1,
        token: "tok".to_string(),
        user_id: 1,
    };
    assert!(apply_fenced_history_response(
        &fence,
        true,
        1,
        1,
        Some("tok"),
        Some(1),
        &sidebar,
        &mut stage,
        77,
        vec![HistoricalMessage {
            message_id: 50,
            sender_id: 10,
            text: "Reloaded background message".to_string(),
            content: None,
            date: Some(1_700_000_000_000),
            reply_to: None,
            is_outgoing: Some(false),
            ..Default::default()
        }],
    ));

    let bg_hist = stage.chat_histories.get(&77).expect("chat 77 history");
    assert_eq!(bg_hist.len(), 3);
    assert_eq!(bg_hist[0].id, 50);
    assert_eq!(bg_hist[1].id, BOT_ASSIST_LOCAL_MSG_ID_BASE + 1);
    assert_eq!(bg_hist[2].id, BOT_ASSIST_LOCAL_MSG_ID_BASE + 2);
    assert!(bg_hist[1].is_local_only());
    assert!(bg_hist[2].is_local_only());
}

#[test]
fn message_item_model_is_local_only_true_for_all_4_cards_and_false_for_text() {
    let normal = make_text_msg(100, "hello world", false);
    assert!(!normal.is_local_only());

    let high_id_local = make_text_msg(BOT_ASSIST_LOCAL_MSG_ID_BASE, "synthetic local", false);
    assert!(high_id_local.is_local_only());

    let card_1 = make_local_card_msg(
        1,
        sample_vault_login_payload("c1", AssistCardStatus::AwaitingUser, false),
    );
    let card_2 = make_local_card_msg(
        2,
        AssistCardPayload::ReuseConfirm {
            card_id: "c2".to_string(),
            task_id: "t2".to_string(),
            session_id: "s2".to_string(),
            chat_id: Some(77),
            credential_id: "cred_1".to_string(),
            origin: "https://dashboard.stripe.com".to_string(),
            target_url: "https://dashboard.stripe.com/login".to_string(),
            account_hint: "al***@example.com".to_string(),
            has_totp: true,
            require_biometric_hint: true,
            status: AssistCardStatus::AwaitingUser,
            status_note: None,
            created_at_ms: 1_700_000_000_000,
        },
    );
    let card_3 = make_local_card_msg(
        3,
        AssistCardPayload::QrLogin {
            card_id: "c3".to_string(),
            task_id: "t3".to_string(),
            session_id: "s3".to_string(),
            chat_id: Some(77),
            origin: "https://example.com".to_string(),
            target_url: "https://example.com/qr".to_string(),
            png_bytes: vec![137, 80, 78, 71],
            countdown_secs: 180,
            refresh_interval_secs: 3,
            poll_attempt: 1,
            status: AssistCardStatus::AwaitingUser,
            status_note: None,
            redirected_url: None,
            created_at_ms: 1_700_000_000_000,
        },
    );
    let card_4 = make_local_card_msg(
        4,
        AssistCardPayload::ClickCaptcha {
            card_id: "c4".to_string(),
            task_id: "t4".to_string(),
            session_id: "s4".to_string(),
            chat_id: Some(77),
            origin: "https://example.com".to_string(),
            target_url: "https://example.com/captcha".to_string(),
            png_bytes: vec![137, 80, 78, 71],
            element_rect: ElementRect {
                x: 10.0,
                y: 20.0,
                width: 300.0,
                height: 150.0,
                device_pixel_ratio: 2.0,
            },
            instruction: "请依次点击：山、水".to_string(),
            max_clicks: 2,
            status: AssistCardStatus::AwaitingUser,
            status_note: None,
            created_at_ms: 1_700_000_000_000,
        },
    );

    assert!(card_1.is_local_only());
    assert!(card_2.is_local_only());
    assert!(card_3.is_local_only());
    assert!(card_4.is_local_only());
}

#[test]
fn click_captcha_pixel_to_normalized_clamps_to_unit_interval() {
    let bounds = Bounds::new(point(px(100.0), px(50.0)), size(px(200.0), px(100.0)));

    let center = pixel_to_normalized_click_point(point(px(200.0), px(100.0)), bounds);
    assert!((center.norm_x - 0.5).abs() < 1e-6);
    assert!((center.norm_y - 0.5).abs() < 1e-6);

    let underflow = pixel_to_normalized_click_point(point(px(20.0), px(-10.0)), bounds);
    assert_eq!(underflow, NormalizedClickPoint::new(0.0, 0.0));

    let overflow = pixel_to_normalized_click_point(point(px(999.0), px(500.0)), bounds);
    assert_eq!(overflow, NormalizedClickPoint::new(1.0, 1.0));

    let zero_bounds = Bounds::new(point(px(10.0), px(10.0)), size(px(0.0), px(0.0)));
    let safe_zero = pixel_to_normalized_click_point(point(px(25.0), px(25.0)), zero_bounds);
    assert_eq!(safe_zero, NormalizedClickPoint::new(0.0, 0.0));

    let mut click_draft = BotAssistClickDraft::new();
    click_draft.push_point(NormalizedClickPoint::new(1.8, -0.3), 2);
    click_draft.push_point(NormalizedClickPoint::new(0.25, 0.75), 2);
    click_draft.push_point(NormalizedClickPoint::new(0.9, 0.9), 2); // capped at max_clicks = 2
    assert_eq!(click_draft.points.len(), 2);
    assert_eq!(click_draft.points[0], NormalizedClickPoint::new(1.0, 0.0));
    assert_eq!(click_draft.points[1], NormalizedClickPoint::new(0.25, 0.75));
    click_draft.clear();
    assert!(click_draft.points.is_empty());
}

#[test]
fn awaiting_user_to_timed_out_resumable_preserves_editability_and_locks_after_save() {
    let mut stage = ConversationStage::new();
    stage.set_chat(77, "Assistant Bot", "bot", "AB", 0x2AABEE);

    let local_id = BOT_ASSIST_LOCAL_MSG_ID_BASE + 10;
    stage.upsert_bot_assist_card(
        77,
        local_id,
        sample_vault_login_payload("card_timeout", AssistCardStatus::AwaitingUser, true),
        "12:10".to_string(),
    );

    assert!(stage.is_bot_card_editable("card_timeout"));
    assert!(stage.focus_bot_card_field("card_timeout", BotAssistLoginField::Username));
    assert!(stage.insert_bot_card_text("bob@stripe-test.com"));

    // Simulate 300s tool timeout transitioning card to TimedOutResumable
    assert!(stage.update_bot_assist_card_status(
        Some(77),
        "card_timeout",
        AssistCardStatus::TimedOutResumable,
        Some("上轮已超时，补填后将自动保存并续跑".to_string()),
        None,
        None,
        None,
    ));

    // Card MUST remain 100% editable and retain partial draft on TimedOutResumable
    assert!(stage.is_bot_card_editable("card_timeout"));
    assert_eq!(
        stage
            .bot_assist_login_drafts
            .get("card_timeout")
            .map(|d| d.username.as_str()),
        Some("bob@stripe-test.com")
    );
    // High-sensitivity origin defaults allow_auto_reuse to false
    assert_eq!(
        stage
            .bot_assist_login_drafts
            .get("card_timeout")
            .map(|d| d.allow_auto_reuse),
        Some(false)
    );

    // Late password entry while TimedOutResumable
    assert!(stage.focus_bot_card_field("card_timeout", BotAssistLoginField::Password));
    assert!(stage.insert_bot_card_text("late_pw_99"));

    // Never leak draft secrets via copy or search
    assert_eq!(stage.copy_message_text(local_id), None);
    stage.search_query = "late_pw_99".to_string();
    stage.recompute_search_matches();
    assert!(stage.search_match_ids.is_empty());

    // Submit & take zeroizes draft
    let sub = stage
        .take_bot_card_login_submission("card_timeout")
        .expect("late submission must succeed");
    assert_eq!(sub.username.as_str(), "bob@stripe-test.com");
    assert_eq!(sub.password.as_str(), "late_pw_99");
    assert!(!stage.bot_assist_login_drafts.contains_key("card_timeout"));
    assert_eq!(stage.active_bot_card_field, None);

    // Transition to SavedAndLoggedIn -> card locks and stores only masked hint
    assert!(stage.update_bot_assist_card_status(
        Some(77),
        "card_timeout",
        AssistCardStatus::SavedAndLoggedIn,
        None,
        Some("cred_abc".to_string()),
        Some("bo***@stripe-test.com".to_string()),
        None,
    ));
    assert!(!stage.is_bot_card_editable("card_timeout"));
    assert!(!stage.focus_bot_card_field("card_timeout", BotAssistLoginField::Username));

    let card = stage
        .find_bot_assist_card("card_timeout")
        .expect("card remains visible in transcript");
    match &card.payload {
        AssistCardPayload::VaultLogin {
            status,
            saved_credential_id,
            saved_account_hint,
            ..
        } => {
            assert_eq!(*status, AssistCardStatus::SavedAndLoggedIn);
            assert_eq!(saved_credential_id.as_deref(), Some("cred_abc"));
            assert_eq!(saved_account_hint.as_deref(), Some("bo***@stripe-test.com"));
        }
        other => panic!("unexpected payload: {:?}", other),
    }
}

#[test]
fn stage_bot_card_field_cycling_backspace_clear_blur_and_qr_refresh() {
    let mut stage = ConversationStage::new();
    stage.set_chat(91, "QR & Login Bot", "bot", "QB", 0x2AABEE);

    let login_id = BOT_ASSIST_LOCAL_MSG_ID_BASE + 20;
    stage.upsert_bot_assist_card(
        91,
        login_id,
        sample_vault_login_payload("card_cycle", AssistCardStatus::AwaitingUser, false),
        "13:00".to_string(),
    );

    assert!(stage.focus_bot_card_field("card_cycle", BotAssistLoginField::Username));
    assert_eq!(
        stage.active_bot_card_field,
        Some(("card_cycle".to_string(), BotAssistLoginField::Username))
    );
    assert!(stage.insert_bot_card_text("用户甲"));
    assert_eq!(stage.active_bot_card_text(), "用户甲");
    assert!(stage.backspace_bot_card_field(false));
    assert_eq!(stage.active_bot_card_text(), "用户");

    assert_eq!(
        stage.cycle_bot_card_field(false),
        Some(BotAssistLoginField::Password)
    );
    assert!(stage.insert_bot_card_text("pass_123"));
    assert_eq!(stage.active_bot_card_text(), "pass_123");
    assert!(stage.clear_active_bot_card_field());
    assert_eq!(stage.active_bot_card_text(), "");

    stage.toggle_bot_card_show_password("card_cycle");
    stage.toggle_bot_card_auto_reuse("card_cycle");
    let draft = stage.bot_assist_login_drafts.get("card_cycle").unwrap();
    assert!(draft.show_password);
    assert!(!draft.allow_auto_reuse);

    stage.blur_bot_card_input();
    assert_eq!(stage.active_bot_card_field, None);

    // QR card dynamic refresh & completion status
    let qr_id = BOT_ASSIST_LOCAL_MSG_ID_BASE + 21;
    stage.upsert_bot_assist_card(
        91,
        qr_id,
        AssistCardPayload::QrLogin {
            card_id: "card_qr_1".to_string(),
            task_id: "t_qr".to_string(),
            session_id: "s_qr".to_string(),
            chat_id: Some(91),
            origin: "https://example.com".to_string(),
            target_url: "https://example.com/qr".to_string(),
            png_bytes: vec![1, 2, 3],
            countdown_secs: 180,
            refresh_interval_secs: 3,
            poll_attempt: 0,
            status: AssistCardStatus::AwaitingUser,
            status_note: None,
            redirected_url: None,
            created_at_ms: 1_700_000_000_000,
        },
        "13:01".to_string(),
    );
    assert!(stage.refresh_bot_assist_qr(Some(91), "card_qr_1", vec![9, 8, 7, 6], 174, 2));
    assert!(stage.update_bot_assist_card_status(
        Some(91),
        "card_qr_1",
        AssistCardStatus::Completed,
        None,
        None,
        None,
        Some("https://example.com/dashboard".to_string()),
    ));
    match &stage
        .find_bot_assist_card("card_qr_1")
        .expect("qr card")
        .payload
    {
        AssistCardPayload::QrLogin {
            png_bytes,
            countdown_secs,
            poll_attempt,
            status,
            redirected_url,
            ..
        } => {
            assert_eq!(png_bytes, &vec![9, 8, 7, 6]);
            assert_eq!(*countdown_secs, 174);
            assert_eq!(*poll_attempt, 2);
            assert_eq!(*status, AssistCardStatus::Completed);
            assert_eq!(
                redirected_url.as_deref(),
                Some("https://example.com/dashboard")
            );
        }
        other => panic!("expected QrLogin, got {:?}", other),
    }
}

const HEADLESS_CHILD_ENV: &str = "GPUI_BOT_CARDS_HEADLESS_CHILD";
const HEADLESS_TARGET_ENV: &str = "GPUI_BOT_CARDS_HEADLESS_TARGET";

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_HEADLESS_BOT_CARDS_ENTRY: extern "C" fn() = macos_mod_init_bot_cards_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_bot_cards_entry() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(HEADLESS_TARGET_ENV) {
            dispatch_headless_bot_cards_target(&target);
        }
    }
}

fn dispatch_headless_bot_cards_target(target: &str) -> ! {
    let body: fn(&mut App) = match target {
        "appshell_ime_routing_and_bot_browser_bridge_roundtrip" => {
            body_appshell_ime_routing_and_bot_browser_bridge_roundtrip
        }
        other => {
            eprintln!("unknown headless bot_assist_cards test target: {other}");
            std::process::exit(101);
        }
    };
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    body(cx);
                }));
                match inner {
                    Ok(()) => std::process::exit(0),
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                            .unwrap_or("panic");
                        eprintln!("[headless child panic] {msg}");
                        std::process::exit(101);
                    }
                }
            });
    }));
    if outer.is_err() {
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[test]
fn appshell_ime_routing_and_bot_browser_bridge_roundtrip() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_bot_cards_target("appshell_ime_routing_and_bot_browser_bridge_roundtrip");
    }
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(HEADLESS_CHILD_ENV, "1")
        .env(
            HEADLESS_TARGET_ENV,
            "appshell_ime_routing_and_bot_browser_bridge_roundtrip",
        )
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg("appshell_ime_routing_and_bot_browser_bridge_roundtrip")
        .arg("--nocapture")
        .output()
        .expect("spawn headless child");
    assert!(
        output.status.success(),
        "headless gpui child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn body_appshell_ime_routing_and_bot_browser_bridge_roundtrip(cx: &mut App) {
    let tmp = tempfile::TempDir::new().expect("temp dir for vault");
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let vault = Arc::new(
        CredentialVault::open_in_memory(&MemoryMasterKeyStore::new()).expect("in-memory vault"),
    );
    let orch = Arc::new(BotBrowserOrchestrator::new(
        OrchestratorConfig::new(tmp.path()),
        vault,
    ));

    let shell: Entity<AppShell> = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.is_authenticated = true;
        s.attach_bot_browser_orchestrator(orch.clone(), cx);
        s.stage.update(cx, |st, _| {
            st.set_chat(77, "Assist Bot", "bot", "AB", 0x2AABEE);
        });
        s
    });

    shell.update(cx, |s, cx| {
        assert!(!s.is_bot_card_input_active(cx));

        // 1. Upsert a local VaultLogin card through BotBrowserEvent
        let payload = sample_vault_login_payload("card_shell_1", AssistCardStatus::AwaitingUser, false);
        s.apply_bot_browser_event(BotBrowserEvent::UpsertCard(payload), cx);
        assert!(s.bot_browser.card_locations.contains_key("card_shell_1"));
        assert_eq!(s.stage.read(cx).messages.len(), 1);
        assert!(s.stage.read(cx).messages[0].is_local_only());

        // 2. Focus Username field -> is_bot_card_input_active becomes true
        s.stage.update(cx, |st, _| {
            assert!(st.focus_bot_card_field("card_shell_1", BotAssistLoginField::Username));
        });
        assert!(s.is_bot_card_input_active(cx));

        // 3. IME preedit & committed CJK text route to BotAssistLoginDraft (NOT composer!)
        s.ime.ime_preedit = Some("zhangsan".to_string());
        s.stage.update(cx, |st, _| {
            st.bot_assist_ime_preedit = Some("zhangsan".to_string());
        });
        assert_eq!(
            s.active_text_and_preedit(cx),
            (String::new(), Some("zhangsan".to_string()))
        );
        // Enter while composing commits preedit rather than submitting form
        assert!(s.handle_non_char_key_with_alt("enter", false, false, cx));
        assert_eq!(s.stage.read(cx).active_bot_card_text(), "zhangsan");
        s.clear_active_field(cx);
        assert_eq!(s.stage.read(cx).active_bot_card_text(), "");
        s.insert_committed_text("张三@example.com", cx);
        assert_eq!(s.composer.read(cx).text(), "");
        assert_eq!(
            s.active_text_and_preedit(cx),
            ("张三@example.com".to_string(), None)
        );

        // 4. Tab cycles Username -> Password, Backspace edits password, Escape blurs
        assert!(s.handle_non_char_key_with_alt("tab", false, false, cx));
        assert_eq!(
            s.stage.read(cx).active_bot_card_field,
            Some(("card_shell_1".to_string(), BotAssistLoginField::Password))
        );
        s.insert_committed_text("secret12x", cx);
        assert!(s.handle_non_char_key_with_alt("backspace", false, false, cx));
        assert_eq!(s.stage.read(cx).active_bot_card_text(), "secret12");
        assert_eq!(
            s.stage
                .read(cx)
                .bot_assist_login_drafts
                .get("card_shell_1")
                .map(|d| d.masked_password()),
            Some("••••••••".to_string())
        );

        assert!(s.handle_non_char_key_with_alt("escape", false, false, cx));
        assert!(!s.is_bot_card_input_active(cx));

        // 5. CancelCard transitions locally and clears ephemeral drafts
        s.handle_bot_assist_card_action(
            BotAssistCardAction::CancelCard {
                card_id: "card_shell_1".to_string(),
            },
            cx,
        );
        assert!(!s
            .stage
            .read(cx)
            .bot_assist_login_drafts
            .contains_key("card_shell_1"));
        assert_eq!(
            s.stage
                .read(cx)
                .find_bot_assist_card("card_shell_1")
                .map(|c| c.status()),
            Some(AssistCardStatus::Cancelled)
        );
    });
}


