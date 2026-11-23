//! Phase 4 Tests: 4 Dark-Minimalist GPUI Cards, Native Chinese IME + Password Masking,
//! Zeroized Draft Lifecycle, Click-Captcha Coordinate Clamping, and Local-Only MTProto Isolation.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use bot_browser::e2ee_sync::{
    derive_split_key, generate_split_key_salt_b64, Argon2KdfParams, DeviceSyncKeypair,
    UnlockedUserVaultKey, UserVaultCategory, UserVaultCrypto,
};
use bot_browser::orchestrator::{BotBrowserOrchestrator, OrchestratorConfig};
use bot_browser::vault::{CredentialVault, MemoryMasterKeyStore, VaultSyncCredentialPayload};
use bot_browser::{
    AssistCardPayload, AssistCardStatus, BotBrowserEvent, ElementRect, NormalizedClickPoint,
    ReusePolicy,
};
use futures::FutureExt;
use gpui::{point, px, size, App, AppContext, Application, AsyncApp, Bounds, Entity};
use gpui_telegram_agent::fabushi_bot::{FabushiBotHost, OFFICIAL_DEFAULT_BOT_USER_ID};
use gpui_telegram_client::net::http_client::HistoricalMessage;
use gpui_telegram_client::net::{
    ArchiveLocalBotReplyResponse, RichSendMessageResponse, UpsertUserVaultItemRequest,
    UpsertUserVaultItemResponse, UserVaultEnvelopeResponse, UserVaultItemDto,
    UserVaultKeyEnvelopeDto, UserVaultSyncPullResponse,
};
use gpui_telegram_client::{AuthResponse, ChatSummary, ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::TelegramAssetSource;
use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::{
    build_ai_oauth_vault_bundle, has_usable_local_fabushi_provider,
    hydrate_ai_oauth_vault_bundle_into_fabushi, hydrate_local_subscriptions_into_fabushi,
};
use gpui_telegram_desktop::views::app_shell::shell_bot_browser::{
    is_safe_profile_item_key, parse_local_vault_command, project_credential_metas,
    pull_and_apply_user_vault_items, push_all_local_vault_items, seal_and_put_vault_item,
    user_vault_envelope_to_proto, LocalVaultCommand, ShellBotBrowserState,
};
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_authoritative_chat_history, apply_authoritative_chat_list,
    apply_fenced_history_response, select_authoritative_dialog, SessionRequestFence,
};
use gpui_telegram_desktop::views::auth::PendingAuthVaultBootstrap;
use gpui_telegram_desktop::views::settings::sections::{
    PendingDeviceSyncRow, SettingsPrivacySecuritySections, SettingsVaultAction, VaultCustodyStatus,
};
use gpui_telegram_desktop::views::settings::SettingsSectionType;
use gpui_telegram_desktop::views::stage::cards::{
    pixel_to_normalized_click_point, BotAssistCardAction, BotAssistCardModel, BotAssistClickDraft,
    BotAssistLoginDraft, BotAssistLoginField, DeliveryStatus, MessageContent, MessageDirection,
    MessageItemModel, BOT_ASSIST_LOCAL_MSG_ID_BASE,
};
use gpui_telegram_desktop::views::{AppShell, ChatSidebar, ContextInspector, ConversationStage};
use gpui_telegram_proto::user_vault::{
    ApproveDeviceSyncRequest, AuthCustodyUpdateResponse, DeviceSyncActionResponse,
    DeviceSyncRequestBody, DeviceSyncRequestResponse, DeviceSyncStatusResponse,
    PendingDeviceSyncItem, PendingDeviceSyncListResponse, UpgradeSplitKeyRequest,
    DEVICE_SYNC_CODE_DOMAIN_V1,
};
use magpie_gateway::{
    ClientSubscriptionSyncItem, ProtocolKind, SubscriptionAccount, SubscriptionKind,
    VertexAdcCredentials,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zeroize::Zeroizing;

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

enum HeadlessRunner {
    Sync(fn(&mut App)),
    Async(Box<dyn FnOnce(AsyncApp) -> Pin<Box<dyn Future<Output = ()>>>>),
}

fn dispatch_headless_bot_cards_target(target: &str) -> ! {
    let runner = match target {
        "appshell_ime_routing_and_bot_browser_bridge_roundtrip" => {
            HeadlessRunner::Sync(body_appshell_ime_routing_and_bot_browser_bridge_roundtrip)
        }
        "appshell_local_vault_interception_and_fabushi_routing" => {
            HeadlessRunner::Async(Box::new(|app| {
                Box::pin(body_appshell_local_vault_interception_and_fabushi_routing(app))
            }))
        }
        "appshell_zero_friction_split_key_and_v1_upgrade" => HeadlessRunner::Async(Box::new(|app| {
            Box::pin(body_appshell_zero_friction_split_key_and_v1_upgrade(app))
        })),
        "appshell_device_trust_ring_x25519_approval_and_recovery_unlock" => {
            HeadlessRunner::Async(Box::new(|app| {
                Box::pin(body_appshell_device_trust_ring_x25519_approval_and_recovery_unlock(app))
            }))
        }
        other => {
            eprintln!("unknown headless bot_assist_cards test target: {other}");
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
                }
                HeadlessRunner::Async(async_fn) => {
                    cx.spawn(async move |app: &mut AsyncApp| {
                        let app_clone = app.clone();
                        let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                            .catch_unwind()
                            .await;
                        match res {
                            Ok(()) => std::process::exit(0),
                            Err(payload) => {
                                let msg = payload
                                    .downcast_ref::<&str>()
                                    .copied()
                                    .or_else(|| {
                                        payload.downcast_ref::<String>().map(|s| s.as_str())
                                    })
                                    .unwrap_or("panic");
                                eprintln!("[headless async child panic] {msg}");
                                std::process::exit(101);
                            }
                        }
                    })
                    .detach();
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

#[derive(Clone, Default)]
struct MockDeviceSyncEntry {
    request_id: String,
    device_name: String,
    device_public_key_b64: String,
    verification_code: String,
    status: String,
    approver_ephemeral_pub_b64: Option<String>,
    encrypted_root_key_b64: Option<String>,
    nonce_b64: Option<String>,
    created_at: i64,
    expires_at: i64,
}

#[derive(Default)]
struct MockCloudVaultState {
    requests: Vec<(String, String, String)>,
    envelope: Option<UserVaultKeyEnvelopeDto>,
    items: HashMap<(String, String), UserVaultItemDto>,
    device_sync: HashMap<String, MockDeviceSyncEntry>,
    upgrade_requests: Vec<UpgradeSplitKeyRequest>,
    auth_version: i32,
    latest_revision: i64,
    next_msg_id: i64,
    next_sync_seq: i64,
    forced_rev_jump_once: bool,
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let content_len: usize = lines
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_len {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(
        &buf[header_end..buf.len().min(header_end + content_len)],
    )
    .to_string();
    Some((method, path, body))
}

async fn spawn_mock_cloud_vault_server(
    forced_rev_jump_once: bool,
) -> (String, Arc<StdMutex<MockCloudVaultState>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock server");
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(StdMutex::new(MockCloudVaultState {
        next_msg_id: 500,
        forced_rev_jump_once,
        ..Default::default()
    }));
    let st_bg = Arc::clone(&state);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let st_conn = Arc::clone(&st_bg);
            tokio::spawn(async move {
                let Some((method, full_path, body)) = read_http_request(&mut stream).await else {
                    return;
                };
                let path = full_path.split('?').next().unwrap_or(&full_path);
                let query = full_path.split_once('?').map(|(_, q)| q).unwrap_or("");
                let resp_json = {
                    let mut guard = st_conn.lock().unwrap();
                    guard
                        .requests
                        .push((method.clone(), full_path.clone(), body.clone()));
                    if method == "GET" && path == "/api/v1/user-vault/envelope" {
                        serde_json::to_string(&UserVaultEnvelopeResponse {
                            envelope: guard.envelope.clone(),
                        })
                        .unwrap()
                    } else if method == "PUT" && path == "/api/v1/user-vault/envelope" {
                        let mut env: UserVaultKeyEnvelopeDto =
                            serde_json::from_str(&body).expect("valid envelope dto");
                        env.updated_at_ms = 1_700_000_123_000;
                        guard.envelope = Some(env.clone());
                        serde_json::to_string(&UserVaultEnvelopeResponse {
                            envelope: Some(env),
                        })
                        .unwrap()
                    } else if method == "GET" && path == "/api/v1/user-vault/items" {
                        let since_rev = query
                            .split('&')
                            .find_map(|p| p.strip_prefix("since_revision="))
                            .and_then(|v| v.parse::<i64>().ok())
                            .unwrap_or(0);
                        let mut items: Vec<UserVaultItemDto> = guard
                            .items
                            .values()
                            .filter(|it| it.revision > since_rev)
                            .cloned()
                            .collect();
                        items.sort_by_key(|it| it.revision);
                        serde_json::to_string(&UserVaultSyncPullResponse {
                            latest_revision: guard.latest_revision,
                            items,
                        })
                        .unwrap()
                    } else if method == "PUT" && path.starts_with("/api/v1/user-vault/items/") {
                        let rest = path.trim_start_matches("/api/v1/user-vault/items/");
                        let (cat, key) = rest.split_once('/').unwrap_or(("password", rest));
                        let req: UpsertUserVaultItemRequest =
                            serde_json::from_str(&body).expect("valid item upsert");
                        if guard.forced_rev_jump_once {
                            guard.forced_rev_jump_once = false;
                            guard.latest_revision += 4;
                        }
                        guard.latest_revision += 1;
                        let rev = guard.latest_revision;
                        let dto = UserVaultItemDto {
                            category: cat.to_string(),
                            item_key: key.to_string(),
                            revision: rev,
                            nonce_b64: req.nonce_b64,
                            ciphertext_b64: req.ciphertext_b64,
                            deleted: req.deleted,
                            updated_at_ms: 1_700_000_200_000 + rev,
                        };
                        guard
                            .items
                            .insert((cat.to_string(), key.to_string()), dto);
                        serde_json::to_string(&UpsertUserVaultItemResponse {
                            ok: true,
                            category: cat.to_string(),
                            item_key: key.to_string(),
                            revision: rev,
                            updated_at_ms: 1_700_000_200_000 + rev,
                        })
                        .unwrap()
                    } else if method == "DELETE" && path.starts_with("/api/v1/user-vault/items/") {
                        let rest = path.trim_start_matches("/api/v1/user-vault/items/");
                        let (cat, key) = rest.split_once('/').unwrap_or(("password", rest));
                        guard.latest_revision += 1;
                        let rev = guard.latest_revision;
                        let dto = UserVaultItemDto {
                            category: cat.to_string(),
                            item_key: key.to_string(),
                            revision: rev,
                            nonce_b64: String::new(),
                            ciphertext_b64: String::new(),
                            deleted: true,
                            updated_at_ms: 1_700_000_200_000 + rev,
                        };
                        guard
                            .items
                            .insert((cat.to_string(), key.to_string()), dto);
                        serde_json::to_string(&UpsertUserVaultItemResponse {
                            ok: true,
                            category: cat.to_string(),
                            item_key: key.to_string(),
                            revision: rev,
                            updated_at_ms: 1_700_000_200_000 + rev,
                        })
                        .unwrap()
                    } else if method == "POST" && path == "/api/v1/auth/upgrade-split-key" {
                        let req: UpgradeSplitKeyRequest =
                            serde_json::from_str(&body).expect("valid upgrade split-key payload");
                        guard.auth_version = 2;
                        guard.upgrade_requests.push(req);
                        serde_json::to_string(&AuthCustodyUpdateResponse {
                            ok: true,
                            auth_version: 2,
                        })
                        .unwrap()
                    } else if method == "POST" && path == "/api/v1/user-vault/device-sync/request" {
                        let req: DeviceSyncRequestBody =
                            serde_json::from_str(&body).expect("valid device sync request");
                        let code = DeviceSyncKeypair::verification_code(&req.device_public_key_b64)
                            .unwrap_or_else(|_| {
                                let _ = DEVICE_SYNC_CODE_DOMAIN_V1;
                                "000-000".to_string()
                            });
                        guard.next_sync_seq += 1;
                        let request_id = format!("sync-req-{}", guard.next_sync_seq);
                        let now = chrono::Utc::now().timestamp();
                        let expires_at = now + 600;
                        guard.device_sync.insert(
                            request_id.clone(),
                            MockDeviceSyncEntry {
                                request_id: request_id.clone(),
                                device_name: req.device_name,
                                device_public_key_b64: req.device_public_key_b64,
                                verification_code: code.clone(),
                                status: "pending".to_string(),
                                approver_ephemeral_pub_b64: None,
                                encrypted_root_key_b64: None,
                                nonce_b64: None,
                                created_at: now,
                                expires_at,
                            },
                        );
                        serde_json::to_string(&DeviceSyncRequestResponse {
                            request_id,
                            verification_code: code,
                            expires_at,
                        })
                        .unwrap()
                    } else if method == "GET" && path == "/api/v1/user-vault/device-sync/pending" {
                        let items: Vec<PendingDeviceSyncItem> = guard
                            .device_sync
                            .values()
                            .filter(|e| e.status == "pending")
                            .map(|e| PendingDeviceSyncItem {
                                request_id: e.request_id.clone(),
                                device_name: e.device_name.clone(),
                                device_public_key_b64: e.device_public_key_b64.clone(),
                                verification_code: e.verification_code.clone(),
                                status: e.status.clone(),
                                created_at: e.created_at,
                                expires_at: e.expires_at,
                            })
                            .collect();
                        serde_json::to_string(&PendingDeviceSyncListResponse { items }).unwrap()
                    } else if method == "POST"
                        && path.starts_with("/api/v1/user-vault/device-sync/")
                        && path.ends_with("/approve")
                    {
                        let rid = path
                            .trim_start_matches("/api/v1/user-vault/device-sync/")
                            .trim_end_matches("/approve")
                            .to_string();
                        let req: ApproveDeviceSyncRequest =
                            serde_json::from_str(&body).expect("valid approve body");
                        if let Some(entry) = guard.device_sync.get_mut(&rid) {
                            entry.status = "approved".to_string();
                            entry.approver_ephemeral_pub_b64 = Some(req.approver_ephemeral_pub_b64);
                            entry.encrypted_root_key_b64 = Some(req.encrypted_root_key_b64);
                            entry.nonce_b64 = Some(req.nonce_b64);
                        }
                        serde_json::to_string(&DeviceSyncActionResponse {
                            ok: true,
                            request_id: rid,
                            status: "approved".to_string(),
                        })
                        .unwrap()
                    } else if method == "POST"
                        && path.starts_with("/api/v1/user-vault/device-sync/")
                        && path.ends_with("/deny")
                    {
                        let rid = path
                            .trim_start_matches("/api/v1/user-vault/device-sync/")
                            .trim_end_matches("/deny")
                            .to_string();
                        if let Some(entry) = guard.device_sync.get_mut(&rid) {
                            entry.status = "denied".to_string();
                        }
                        serde_json::to_string(&DeviceSyncActionResponse {
                            ok: true,
                            request_id: rid,
                            status: "denied".to_string(),
                        })
                        .unwrap()
                    } else if method == "GET"
                        && path.starts_with("/api/v1/user-vault/device-sync/")
                        && path.ends_with("/status")
                    {
                        let rid = path
                            .trim_start_matches("/api/v1/user-vault/device-sync/")
                            .trim_end_matches("/status")
                            .to_string();
                        let resp = if let Some(entry) = guard.device_sync.get_mut(&rid) {
                            if entry.status == "approved" {
                                let out = DeviceSyncStatusResponse {
                                    status: "approved".to_string(),
                                    approver_ephemeral_pub_b64: entry
                                        .approver_ephemeral_pub_b64
                                        .take(),
                                    encrypted_root_key_b64: entry.encrypted_root_key_b64.take(),
                                    nonce_b64: entry.nonce_b64.take(),
                                    expires_at: entry.expires_at,
                                };
                                entry.status = "consumed".to_string();
                                out
                            } else {
                                DeviceSyncStatusResponse {
                                    status: entry.status.clone(),
                                    approver_ephemeral_pub_b64: None,
                                    encrypted_root_key_b64: None,
                                    nonce_b64: None,
                                    expires_at: entry.expires_at,
                                }
                            }
                        } else {
                            DeviceSyncStatusResponse {
                                status: "expired".to_string(),
                                approver_ephemeral_pub_b64: None,
                                encrypted_root_key_b64: None,
                                nonce_b64: None,
                                expires_at: 0,
                            }
                        };
                        serde_json::to_string(&resp).unwrap()
                    } else if method == "POST" && path == "/api/v1/messages/send" {
                        guard.next_msg_id += 1;
                        let mid = guard.next_msg_id;
                        serde_json::to_string(&RichSendMessageResponse {
                            message_id: mid,
                            id: mid,
                            pts: mid as u64,
                            pts_count: 1,
                            duplicate: false,
                            date_ms: 1_700_000_250_000,
                            date: "2026-11-20T00:00:00Z".to_string(),
                            sender_id: 42_999,
                            chat_id: 700_000_077,
                            text: String::new(),
                            client_nonce: None,
                            reply_to: None,
                            status: "sent".to_string(),
                        })
                        .unwrap()
                    } else if method == "POST" && path.ends_with("/messages/archive-local-bot-reply") {
                        guard.next_msg_id += 1;
                        let mid = guard.next_msg_id;
                        serde_json::to_string(&ArchiveLocalBotReplyResponse {
                            message_id: mid,
                            pts: mid as u64,
                            date_ms: 1_700_000_300_000,
                        })
                        .unwrap()
                    } else {
                        "{\"ok\":true}".to_string()
                    }
                };
                let http_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_json.len(),
                    resp_json
                );
                let _ = stream.write_all(http_resp.as_bytes()).await;
            });
        }
    });
    (format!("http://{}", addr), state)
}

fn sample_local_vertex_sync_item() -> ClientSubscriptionSyncItem {
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("x-goog-user-project".to_string(), "proj-local-e2ee".to_string());
    ClientSubscriptionSyncItem {
        account: SubscriptionAccount {
            id: "google_vertex:e2ee@example.com".to_string(),
            kind: SubscriptionKind::GoogleVertex,
            label: "Vertex AI (e2ee@example.com)".to_string(),
            email: Some("e2ee@example.com".to_string()),
            account_id: Some("proj-local-e2ee".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: true,
            base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
            protocol: ProtocolKind::GeminiGenerateContent,
            default_model: Some("gemini-2.5-flash".to_string()),
            source_path: String::new(),
            expires_at_ms: None,
            access_token: None,
            headers: headers.clone(),
        },
        access_token: None,
        raw_headers: headers,
        vertex_adc: Some(VertexAdcCredentials {
            client_id: "vertex-cid-secret".to_string(),
            client_secret: "vertex-csec-secret".to_string(),
            refresh_token: "vertex-refresh-token-secret".to_string(),
            quota_project_id: Some("proj-local-e2ee".to_string()),
            account: Some("e2ee@example.com".to_string()),
        }),
        codex_oauth: None,
        region_id: Some("global".to_string()),
        available_models: vec!["gemini-2.5-flash".to_string()],
    }
}

#[test]
fn local_vault_command_parser_intercepts_and_redacts_secrets() {
    match parse_local_vault_command("/vault-setup MySuperSecretPass!99") {
        Some(LocalVaultCommand::Setup { passphrase }) => {
            assert_eq!(passphrase.as_str(), "MySuperSecretPass!99");
            let dbg = format!(
                "{:?}",
                LocalVaultCommand::Setup {
                    passphrase: passphrase.clone()
                }
            );
            assert!(!dbg.contains("MySuperSecretPass!99"));
            assert!(dbg.contains("[REDACTED]"));
        }
        other => panic!("unexpected parse result: {other:?}"),
    }

    match parse_local_vault_command("  /vault-unlock@fabushi_bot AnotherPass42 ") {
        Some(LocalVaultCommand::Unlock { passphrase }) => {
            assert_eq!(passphrase.as_str(), "AnotherPass42");
        }
        other => panic!("unexpected unlock parse: {other:?}"),
    }

    match parse_local_vault_command("/vault-recover ABCD-EFGH-IJKL-MNOP") {
        Some(LocalVaultCommand::Recover { recovery_key }) => {
            assert_eq!(recovery_key.as_str(), "ABCD-EFGH-IJKL-MNOP");
            let dbg = format!(
                "{:?}",
                LocalVaultCommand::Recover {
                    recovery_key: recovery_key.clone()
                }
            );
            assert!(!dbg.contains("ABCD-EFGH-IJKL-MNOP"));
            assert!(dbg.contains("[REDACTED]"));
        }
        other => panic!("unexpected recover parse: {other:?}"),
    }

    assert!(matches!(
        parse_local_vault_command("/vault-sync"),
        Some(LocalVaultCommand::Sync)
    ));
    // Bare /vault-setup without passphrase is still intercepted locally as UsageError!
    assert!(matches!(
        parse_local_vault_command("/vault-setup   "),
        Some(LocalVaultCommand::UsageError { .. })
    ));
    // Normal messages or bot commands are NOT intercepted as vault commands
    assert_eq!(parse_local_vault_command("hello world"), None);
    assert_eq!(parse_local_vault_command("/start"), None);
    assert_eq!(parse_local_vault_command("/vault_status"), None);

    assert!(is_safe_profile_item_key("local_chat_77_bot_777000__portal.example.com_abc123"));
    assert!(!is_safe_profile_item_key("../escape"));
    assert!(!is_safe_profile_item_key("dir/sub"));
    assert!(!is_safe_profile_item_key(""));
}

#[test]
fn e2ee_sync_helpers_encrypt_pull_merge_and_hydrate_fabushi_subscriptions() {
    let dir_a = tempfile::TempDir::new().expect("dir_a");
    let dir_b = tempfile::TempDir::new().expect("dir_b");
    let user_id = 55_123u64;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (base_url, mock_state) = rt.block_on(spawn_mock_cloud_vault_server(true));

    let mut cfg_a = ClientConfig::default();
    cfg_a.server_url = base_url.clone();
    let client_a = TelegramClient::open_with_dir(cfg_a, dir_a.path()).unwrap();
    client_a.set_auth_session(AuthResponse {
        token: "tok_sync_test".to_string(),
        session_id: "sess_sync_test".to_string(),
        user_id,
        username: "alice".to_string(),
        first_name: Some("Alice".to_string()),
        created_at: None,
        expires_at: None,
    });

    let (unlocked_key, _env, _rec) =
        UserVaultCrypto::initialize_new(user_id, "Passphrase!2026", Argon2KdfParams::fast_for_tests())
            .unwrap();

    // Seed Device A vault password, Obscura browser profile cookies, and AI OAuth subscription
    let vault_a = CredentialVault::open(dir_a.path().join("vault.db"), &unlocked_key).unwrap();
    vault_a
        .store_credential(
            "https://portal.example.com",
            "alice@portal.example.com",
            "TopSecretVaultPw!88",
            Some("JBSWY3DPEHPK3PXP"),
            None,
            Some(ReusePolicy::AutoReuse),
        )
        .unwrap();

    let profile_key = "local_chat_77_bot_777000__portal.example.com_0123456789ab";
    let profiles_a = dir_a.path().join("profiles");
    std::fs::create_dir_all(profiles_a.join(profile_key)).unwrap();
    std::fs::write(
        profiles_a.join(profile_key).join("Cookies"),
        b"sid=super-secret-obscura-cookie",
    )
    .unwrap();

    let subs_a = vec![sample_local_vertex_sync_item()];
    let ai_bundle = build_ai_oauth_vault_bundle(&subs_a, None).unwrap();

    let mut rev_a = 0i64;
    let push_report = rt
        .block_on(push_all_local_vault_items(
            &client_a,
            &unlocked_key,
            user_id,
            &vault_a,
            &profiles_a,
            Some(&ai_bundle),
            &mut rev_a,
        ))
        .expect("push_all_local_vault_items");
    assert_eq!(push_report.passwords_pushed, 1);
    assert_eq!(push_report.cookies_pushed, 1);
    assert!(push_report.ai_oauth_pushed);
    assert!(push_report.latest_revision >= 6);

    // Server must NEVER hold any plaintext password, TOTP, cookie, or OAuth refresh token!
    {
        let st = mock_state.lock().unwrap();
        let stored_vals: Vec<_> = st.items.values().collect();
        let all_stored = serde_json::to_string(&stored_vals).unwrap();
        assert!(!all_stored.contains("TopSecretVaultPw!88"));
        assert!(!all_stored.contains("JBSWY3DPEHPK3PXP"));
        assert!(!all_stored.contains("super-secret-obscura-cookie"));
        assert!(!all_stored.contains("vertex-refresh-token-secret"));
    }

    // Verify concurrent revision jump self-healing in seal_and_put_vault_item
    {
        let mut st = mock_state.lock().unwrap();
        st.forced_rev_jump_once = true;
    }
    let healed_rev = rt
        .block_on(seal_and_put_vault_item(
            &client_a,
            &unlocked_key,
            user_id,
            UserVaultCategory::Password,
            "extra_test_cred",
            b"{}",
            &mut rev_a,
        ))
        .expect("self-healing revision seal");
    assert!(healed_rev > push_report.latest_revision);

    // Device B: fresh vault + fresh FabushiBotHost pulls and decrypts everything locally
    let mut cfg_b = ClientConfig::default();
    cfg_b.server_url = base_url;
    let client_b = TelegramClient::open_with_dir(cfg_b, dir_b.path()).unwrap();
    client_b.set_auth_session(AuthResponse {
        token: "tok_sync_test".to_string(),
        session_id: "sess_sync_test_b".to_string(),
        user_id,
        username: "alice".to_string(),
        first_name: Some("Alice".to_string()),
        created_at: None,
        expires_at: None,
    });
    let vault_b = CredentialVault::open(dir_b.path().join("vault.db"), &unlocked_key).unwrap();
    let profiles_b = dir_b.path().join("profiles");
    // Remove extra_test_cred so pull applies only valid records cleanly
    {
        let mut st = mock_state.lock().unwrap();
        st.items
            .remove(&("password".to_string(), "extra_test_cred".to_string()));
    }
    let pull_report = rt
        .block_on(pull_and_apply_user_vault_items(
            &client_b,
            &unlocked_key,
            user_id,
            &vault_b,
            &profiles_b,
            0,
        ))
        .expect("pull_and_apply_user_vault_items");
    assert_eq!(pull_report.passwords_applied, 1);
    assert_eq!(pull_report.cookies_applied, 1);
    let pulled_bundle = pull_report.ai_oauth_bundle.expect("pulled ai_oauth_bundle");

    let metas_b = vault_b.list_for_origin("https://portal.example.com").unwrap();
    assert_eq!(metas_b.len(), 1);
    let decrypted = vault_b
        .decrypt_for_origin(&metas_b[0].credential_id, "https://portal.example.com", None)
        .unwrap();
    assert_eq!(decrypted.username.as_str(), "alice@portal.example.com");
    assert_eq!(decrypted.password.as_str(), "TopSecretVaultPw!88");
    assert_eq!(
        decrypted.totp_secret.as_deref().map(|s| s.as_str()),
        Some("JBSWY3DPEHPK3PXP")
    );
    assert_eq!(
        std::fs::read(profiles_b.join(profile_key).join("Cookies")).unwrap(),
        b"sid=super-secret-obscura-cookie"
    );

    let host_b = FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap();
    assert!(!has_usable_local_fabushi_provider(&host_b));
    let hydrated_cnt = hydrate_ai_oauth_vault_bundle_into_fabushi(&host_b, &pulled_bundle).unwrap();
    assert_eq!(hydrated_cnt, 1);
    assert!(has_usable_local_fabushi_provider(&host_b));
}

#[test]
fn appshell_local_vault_interception_and_fabushi_routing() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_bot_cards_target("appshell_local_vault_interception_and_fabushi_routing");
    }
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(HEADLESS_CHILD_ENV, "1")
        .env(
            HEADLESS_TARGET_ENV,
            "appshell_local_vault_interception_and_fabushi_routing",
        )
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg("appshell_local_vault_interception_and_fabushi_routing")
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

async fn body_appshell_local_vault_interception_and_fabushi_routing(mut cx: AsyncApp) {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let seed_client = TelegramClient::new(ClientConfig::default());
    let (base_url, mock_state) = seed_client
        .runtime()
        .block_on(spawn_mock_cloud_vault_server(false));

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, tmp.path()).expect("open isolated client"),
    );
    let _rt = client.runtime().enter();
    let user_id = 42_999u64;
    let auth = AuthResponse {
        token: "tok_shell_test".to_string(),
        session_id: "sess_shell_test".to_string(),
        user_id,
        username: "alice".to_string(),
        first_name: Some("Alice".to_string()),
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let fabushi_chat_id = 700_000_077u64;
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
                s.session_sync
                    .chat_peer_user_ids
                    .insert(fabushi_chat_id, OFFICIAL_DEFAULT_BOT_USER_ID as u64);
                s.sidebar.update(cx, |sb, sb_cx| {
                    s.stage.update(sb_cx, |st, st_cx| {
                        s.inspector.update(st_cx, |insp, _| {
                            apply_authoritative_chat_list(
                                sb,
                                st,
                                insp,
                                &[ChatSummary {
                                    chat_id: fabushi_chat_id,
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
                            select_authoritative_dialog(sb, st, insp, fabushi_chat_id);
                        });
                    });
                });
                s
            })
        })
        .unwrap();

    // 1. /vault-setup MUST be intercepted locally — NEVER stored in stage/sidebar or sent to /messages/send
    shell
        .update(&mut cx, |s, cx| {
            s.append_outgoing_text("/vault-setup TopSecretVaultPassphrase#99".to_string(), cx);
            assert!(
                s.stage.read(cx).messages.is_empty(),
                "passphrase command must NEVER be inserted into stage messages"
            );
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let ready = shell
            .read_with(&cx, |s, _| {
                s.bot_browser.unlocked_vault_key.is_some()
                    && s.bot_browser.local_fabushi.is_some()
                    && s.bot_browser.orchestrator.is_some()
            })
            .unwrap();
        if ready {
            break;
        }
    }

    shell
        .read_with(&cx, |s, cx| {
            assert!(s.bot_browser.unlocked_vault_key.is_some());
            assert!(s.bot_browser.last_recovery_key_notice.is_some());
            assert!(s.bot_browser.local_fabushi.is_some());
            assert!(s.bot_browser.orchestrator.is_some());
            assert!(!s.bot_browser.vault_locked_needs_passphrase);
            // Recovery key notice shown locally must NOT contain the passphrase
            let all_stage_json = serde_json::to_string(&s.stage.read(cx).messages).unwrap_or_default();
            assert!(!all_stage_json.contains("TopSecretVaultPassphrase#99"));
        })
        .unwrap();

    // 2. /vault-unlock and /vault-recover MUST also be intercepted locally without sending to /messages/send
    shell
        .update(&mut cx, |s, cx| {
            s.append_outgoing_text("/vault-unlock TopSecretVaultPassphrase#99".to_string(), cx);
            s.append_outgoing_text("/vault-recover AAAA-BBBB-CCCC-DDDD".to_string(), cx);
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(120))
        .await;

    {
        let st = mock_state.lock().unwrap();
        assert!(
            !st.requests.iter().any(|(_, p, _)| p == "/api/v1/messages/send"),
            "vault commands must NEVER trigger /api/v1/messages/send: {:?}",
            st.requests
        );
        let serialized_reqs = serde_json::to_string(&st.requests).unwrap();
        assert!(!serialized_reqs.contains("TopSecretVaultPassphrase#99"));
        assert!(!serialized_reqs.contains("AAAA-BBBB-CCCC-DDDD"));
        assert!(st.envelope.is_some());
    }

    // 3. Test ShellBotBrowserState local FabushiBotHost + BotBrowserOrchestrator wiring & routing
    let fresh_vault = Arc::new(
        CredentialVault::open_in_memory(&MemoryMasterKeyStore::new()).expect("memory vault"),
    );
    let fresh_orch = Arc::new(BotBrowserOrchestrator::new(
        OrchestratorConfig::new(tmp.path().join("orch_routing")),
        fresh_vault,
    ));
    let fresh_host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("ephemeral fabushi"),
    );

    shell
        .update(&mut cx, |s, cx| {
            s.attach_bot_browser_orchestrator(Arc::clone(&fresh_orch), cx);
            s.attach_local_fabushi_host(Arc::clone(&fresh_host));
            // FabushiBotHost::set_bot_browser wired orchestrator onto core_engine
            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            assert!(fresh_host
                .core_engine()
                .build_tool_context("s", None, None, 1, None, 0, cancel)
                .bot_browser
                .is_some());

            // Without a real local provider -> can_run_local_fabushi_turn is false -> skip_server_bot_reply: false
            assert!(!s.can_run_local_fabushi_turn(cx));
            s.append_outgoing_text("hello cloud fallback".to_string(), cx);
        })
        .unwrap();

    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
        let has_send = mock_state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(_, p, body)| p == "/api/v1/messages/send" && body.contains("hello cloud fallback"));
        if has_send {
            break;
        }
    }

    // Hydrate local provider -> can_run_local_fabushi_turn becomes true -> skip_server_bot_reply: true
    shell
        .update(&mut cx, |s, cx| {
            hydrate_local_subscriptions_into_fabushi(
                &fresh_host,
                &[sample_local_vertex_sync_item()],
            );
            assert!(s.can_run_local_fabushi_turn(cx));
            s.append_outgoing_text("hello local fabushi".to_string(), cx);
        })
        .unwrap();

    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
        let has_local_send = mock_state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(_, p, body)| p == "/api/v1/messages/send" && body.contains("hello local fabushi"));
        if has_local_send {
            break;
        }
    }

    {
        let st = mock_state.lock().unwrap();
        let cloud_send_body = st
            .requests
            .iter()
            .find(|(_, p, b)| p == "/api/v1/messages/send" && b.contains("hello cloud fallback"))
            .map(|(_, _, b)| b.clone())
            .expect("cloud fallback send captured");
        let cloud_json: serde_json::Value = serde_json::from_str(&cloud_send_body).unwrap();
        assert_eq!(cloud_json["skip_server_bot_reply"], false);

        let local_send_body = st
            .requests
            .iter()
            .find(|(_, p, b)| p == "/api/v1/messages/send" && b.contains("hello local fabushi"))
            .map(|(_, _, b)| b.clone())
            .expect("local fabushi send captured");
        let local_json: serde_json::Value = serde_json::from_str(&local_send_body).unwrap();
        assert_eq!(local_json["skip_server_bot_reply"], true);
    }

    // 4. Logout clears unlocked_vault_key, local_fabushi, last_recovery_key_notice, and vault_sync_revision
    shell
        .update(&mut cx, |s, cx| {
            s.logout(cx);
            assert!(s.bot_browser.unlocked_vault_key.is_none());
            assert!(s.bot_browser.local_fabushi.is_none());
            assert!(s.bot_browser.orchestrator.is_none());
            assert!(s.bot_browser.last_recovery_key_notice.is_none());
            assert_eq!(s.bot_browser.vault_sync_revision, 0);
        })
        .unwrap();

    // Ensure ShellBotBrowserState::reset_on_logout directly resets all fields as well
    let mut state = ShellBotBrowserState::new();
    state.unlocked_vault_key = Some(Arc::new(UnlockedUserVaultKey::from_bytes([0x11; 32])));
    state.vault_sync_revision = 42;
    state.vault_locked_needs_passphrase = true;
    state.last_recovery_key_notice = Some(zeroize::Zeroizing::new("ABCD".to_string()));
    state.reset_on_logout();
    assert!(state.unlocked_vault_key.is_none());
    assert!(state.local_fabushi.is_none());
    assert_eq!(state.vault_sync_revision, 0);
    assert!(!state.vault_locked_needs_passphrase);
    assert!(state.last_recovery_key_notice.is_none());
    assert_eq!(state.vault_custody_status, VaultCustodyStatus::LockedOrUninitialized);
    assert!(state.pending_device_sync.is_none());
    assert!(state.pending_device_approvals.is_empty());
    assert!(state.cached_credential_metas.is_empty());
    assert!(!state.first_creation_banner_open);
}

#[test]
fn pending_auth_vault_bootstrap_redacts_secrets_and_zeroizes() {
    let salt_b64 = generate_split_key_salt_b64();
    let params = Argon2KdfParams::fast_for_tests();
    let bundle = derive_split_key("MyDesktopSecret#2026", &salt_b64, &params).unwrap();
    let legacy_pw = Zeroizing::new("LegacyUpgradeSecret!9".to_string());

    let bootstrap = PendingAuthVaultBootstrap::SplitKeyV2 {
        split_kek: bundle.vault_kek.clone(),
        auth_salt_b64: salt_b64.clone(),
        kdf_params: params,
        legacy_upgrade_password: Some(legacy_pw),
        is_new_registration: false,
    };

    let dbg = format!("{bootstrap:?}");
    assert!(dbg.contains("[REDACTED]"));
    assert!(dbg.contains(&salt_b64));
    assert!(!dbg.contains("MyDesktopSecret#2026"));
    assert!(!dbg.contains("LegacyUpgradeSecret!9"));

    let pwdless = PendingAuthVaultBootstrap::PasswordlessDeviceOnly;
    assert_eq!(format!("{pwdless:?}"), "PasswordlessDeviceOnly");
}

#[test]
fn project_credential_metas_excludes_secrets_and_sorts_by_recent_use() {
    let records = vec![
        VaultSyncCredentialPayload {
            credential_id: "cred-old".to_string(),
            origin: "https://alpha.example.com".to_string(),
            account_hint: "al***@example.com".to_string(),
            reuse_policy: ReusePolicy::AutoReuse,
            username: "alice_secret_user".to_string(),
            password: "alice_secret_password".to_string(),
            totp_secret: None,
            created_at_ms: 1_000,
            last_used_at_ms: Some(1_500),
            deleted: false,
        },
        VaultSyncCredentialPayload {
            credential_id: "cred-deleted".to_string(),
            origin: "https://deleted.example.com".to_string(),
            account_hint: "del@example.com".to_string(),
            reuse_policy: ReusePolicy::ConfirmEveryTime,
            username: "deleted_user".to_string(),
            password: "deleted_password".to_string(),
            totp_secret: Some("JBSWY3DPEHPK3PXP".to_string()),
            created_at_ms: 9_000,
            last_used_at_ms: Some(9_500),
            deleted: true,
        },
        VaultSyncCredentialPayload {
            credential_id: "cred-recent".to_string(),
            origin: "https://stripe.com".to_string(),
            account_hint: "bo***@corp.dev".to_string(),
            reuse_policy: ReusePolicy::ConfirmEveryTime,
            username: "bob_secret_user".to_string(),
            password: "bob_secret_password".to_string(),
            totp_secret: Some("JBSWY3DPEHPK3PXP".to_string()),
            created_at_ms: 2_000,
            last_used_at_ms: Some(5_000),
            deleted: false,
        },
    ];

    let metas = project_credential_metas(records);
    assert_eq!(metas.len(), 2);
    assert_eq!(metas[0].credential_id, "cred-recent");
    assert_eq!(metas[0].origin, "https://stripe.com");
    assert!(metas[0].has_totp);
    assert_eq!(metas[0].reuse_policy, ReusePolicy::ConfirmEveryTime);
    assert_eq!(metas[1].credential_id, "cred-old");
    assert!(!metas[1].has_totp);

    let serialized = serde_json::to_string(&metas).unwrap();
    assert!(!serialized.contains("bob_secret_password"));
    assert!(!serialized.contains("alice_secret_password"));
    assert!(!serialized.contains("JBSWY3DPEHPK3PXP"));
    assert!(!serialized.contains("deleted_user"));
}

#[test]
fn settings_privacy_security_custody_input_and_ime_unit_transitions() {
    let mut sec = SettingsPrivacySecuritySections::new();
    sec.recovery_key_editing = true;
    assert!(!sec.submit_recovery_key_draft(), "blank draft must not submit");

    *sec.recovery_key_draft = "EK-ABCD-1234X".to_string();
    assert!(sec.backspace_recovery_key_draft());
    assert_eq!(sec.recovery_key_draft.as_str(), "EK-ABCD-1234");

    assert!(sec.submit_recovery_key_draft());
    assert!(!sec.recovery_key_editing);
    assert!(sec.recovery_key_draft.is_empty());
    let act = sec.pending_vault_action.take().expect("emitted recovery action");
    let dbg = format!("{act:?}");
    assert!(dbg.contains("[REDACTED]"));
    assert!(!dbg.contains("EK-ABCD-1234"));
    match act {
        SettingsVaultAction::SubmitRecoveryKey(rk) => assert_eq!(rk.as_str(), "EK-ABCD-1234"),
        other => panic!("unexpected action: {other:?}"),
    }

    sec.sync_vault_custody_state(
        VaultCustodyStatus::AwaitingDeviceApproval {
            request_id: "req-1".to_string(),
            verification_code: "482-910".to_string(),
            expires_at: 9_999_999,
        },
        false,
        None,
        0,
        7,
        Some("waiting".to_string()),
        Some(Zeroizing::new("EK-ONE-TIME".to_string())),
        vec![PendingDeviceSyncRow {
            request_id: "req-2".to_string(),
            device_name: "MacBook Pro".to_string(),
            device_public_key_b64: "pub".to_string(),
            verification_code: "111-222".to_string(),
            created_at: 100,
            expires_at: 700,
        }],
        Vec::new(),
    );
    assert_eq!(sec.vault_sync_revision, 7);
    assert_eq!(sec.pending_device_sync_requests.len(), 1);
    assert_eq!(sec.session_emergency_kit.as_deref().map(|s| s.as_str()), Some("EK-ONE-TIME"));

    sec.clear_session_custody_state();
    assert_eq!(sec.vault_status, VaultCustodyStatus::LockedOrUninitialized);
    assert!(sec.session_emergency_kit.is_none());
    assert!(sec.pending_device_sync_requests.is_empty());
    assert!(sec.saved_credentials.is_empty());
    assert_eq!(sec.vault_sync_revision, 0);
}

fn run_headless_child_target(target: &str) {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_bot_cards_target(target);
    }
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(HEADLESS_CHILD_ENV, "1")
        .env(HEADLESS_TARGET_ENV, target)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(target)
        .arg("--nocapture")
        .output()
        .expect("spawn headless child");
    assert!(
        output.status.success(),
        "headless gpui child `{target}` failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn appshell_zero_friction_split_key_and_v1_upgrade() {
    run_headless_child_target("appshell_zero_friction_split_key_and_v1_upgrade");
}

#[test]
fn appshell_device_trust_ring_x25519_approval_and_recovery_unlock() {
    run_headless_child_target("appshell_device_trust_ring_x25519_approval_and_recovery_unlock");
}

async fn body_appshell_zero_friction_split_key_and_v1_upgrade(mut cx: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _rt_guard = rt.enter();
    let (base_url, mock_state) = rt.block_on(spawn_mock_cloud_vault_server(false));

    // A. Brand-new Split-Key V2 registration initializes split_key_v2 envelope with ZERO /vault-setup prompts
    let prefs_a = tempfile::TempDir::new().unwrap();
    let mut cfg_a = ClientConfig::default();
    cfg_a.server_url = base_url.clone();
    let client_a = Arc::new(
        TelegramClient::open_with_dir(cfg_a, prefs_a.path()).expect("open isolated client A"),
    );
    let auth_a = AuthResponse {
        user_id: 77_001,
        username: "alice_v2".to_string(),
        token: "tok-a".to_string(),
        session_id: "sess-a".to_string(),
        first_name: Some("Alice".to_string()),
        created_at: None,
        expires_at: None,
    };

    let salt_v2 = generate_split_key_salt_b64();
    let params_v2 = Argon2KdfParams::fast_for_tests();
    let bundle_v2 = derive_split_key("SplitKeyPass!2026", &salt_v2, &params_v2).unwrap();
    let bootstrap_reg = PendingAuthVaultBootstrap::SplitKeyV2 {
        split_kek: bundle_v2.vault_kek.clone(),
        auth_salt_b64: salt_v2.clone(),
        kdf_params: params_v2,
        legacy_upgrade_password: None,
        is_new_registration: true,
    };

    let shell_a = cx
        .update(|app| {
            let client_init = Arc::clone(&client_a);
            let prefs_path = prefs_a.path().to_path_buf();
            app.new(|wcx| {
                let mut s = AppShell::new_with_client(client_init, wcx);
                s.preferences_dir = prefs_path;
                s.apply_authenticated_session_with_vault(auth_a.clone(), Some(bootstrap_reg), wcx);
                s
            })
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let ready = shell_a
            .read_with(&cx, |s, _| {
                s.bot_browser.unlocked_vault_key.is_some()
                    && s.bot_browser.vault_custody_status == VaultCustodyStatus::UnlockedAndSynced
            })
            .unwrap();
        if ready {
            break;
        }
    }

    shell_a
        .update(&mut cx, |s, cx| {
            assert!(s.bot_browser.unlocked_vault_key.is_some());
            assert_eq!(
                s.bot_browser.vault_custody_status,
                VaultCustodyStatus::UnlockedAndSynced
            );
            assert!(s.bot_browser.last_recovery_key_notice.is_some());
            assert!(s.bot_browser.first_creation_banner_open);

            // Never prompt user with /vault-setup or /vault-unlock!
            for t in s
                .window_runtime
                .controller
                .current_toast
                .iter()
                .chain(s.window_runtime.controller.toast_queue.iter())
            {
                assert!(!t.text.contains("/vault-setup"), "unexpected toast: {}", t.text);
                assert!(!t.text.contains("/vault-unlock"), "unexpected toast: {}", t.text);
            }
            let st_json = serde_json::to_string(&s.stage.read(cx).messages).unwrap_or_default();
            assert!(!st_json.contains("/vault-setup"));
            assert!(!st_json.contains("/vault-unlock"));

            // Navigating to Settings -> PrivacySecurity propagates custody state immediately
            s.settings_router.update(cx, |r, rcx| {
                r.navigate_to(SettingsSectionType::PrivacySecurity, true, rcx);
            });
            s.sync_vault_custody_to_settings(cx);
            let sp = s
                .settings_router
                .read(cx)
                .settings_privacy
                .clone()
                .expect("privacy section materialized on navigate");
            assert_eq!(
                sp.read(cx).vault_status,
                VaultCustodyStatus::UnlockedAndSynced
            );
            assert!(sp.read(cx).session_emergency_kit.is_some());
        })
        .unwrap();

    {
        let st = mock_state.lock().unwrap();
        let env = st.envelope.as_ref().expect("split_key_v2 envelope uploaded");
        assert_eq!(env.kdf_alg, "split_key_v2");
        assert_eq!(env.kdf_salt_b64, salt_v2);
    }

    // Store one site credential in Vault A and push via Settings toggle + delete verification
    let orch_a = shell_a
        .read_with(&cx, |s, _| s.bot_browser.orchestrator.clone().unwrap())
        .unwrap();
    let stored_meta = orch_a
        .vault()
        .store_credential(
            "https://github.com/login",
            "alice_gh",
            "ghp_super_secret_pw",
            Some("JBSWY3DPEHPK3PXP"),
            None,
            Some(ReusePolicy::AutoReuse),
        )
        .unwrap();
    let cred_id = stored_meta.credential_id.clone();

    shell_a
        .update(&mut cx, |s, cx| {
            s.handle_settings_vault_action(&SettingsVaultAction::RefreshVaultState, cx);
        })
        .unwrap();
    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let has_cred = shell_a
            .read_with(&cx, |s, _| {
                s.bot_browser
                    .cached_credential_metas
                    .iter()
                    .any(|m| m.credential_id == cred_id)
            })
            .unwrap();
        if has_cred {
            break;
        }
    }
    shell_a
        .update(&mut cx, |s, cx| {
            s.handle_settings_vault_action(
                &SettingsVaultAction::ToggleCredentialAutoReuse {
                    credential_id: cred_id.clone(),
                    allow_auto_reuse: false,
                },
                cx,
            );
        })
        .unwrap();
    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let toggled = shell_a
            .read_with(&cx, |s, _| {
                s.bot_browser
                    .cached_credential_metas
                    .iter()
                    .find(|m| m.credential_id == cred_id)
                    .is_some_and(|m| m.reuse_policy == ReusePolicy::ConfirmEveryTime)
            })
            .unwrap();
        if toggled {
            break;
        }
    }

    // B. Fresh Device B logs in with Split-Key V2 -> seamlessly unlocks & pulls credential from mock cloud!
    ClientConfig::clear_session();
    let prefs_b = tempfile::TempDir::new().unwrap();
    let mut cfg_b = ClientConfig::default();
    cfg_b.server_url = base_url.clone();
    let client_b = Arc::new(
        TelegramClient::open_with_dir(cfg_b, prefs_b.path()).expect("open isolated client B"),
    );
    let bootstrap_login = PendingAuthVaultBootstrap::SplitKeyV2 {
        split_kek: bundle_v2.vault_kek.clone(),
        auth_salt_b64: salt_v2.clone(),
        kdf_params: params_v2,
        legacy_upgrade_password: None,
        is_new_registration: false,
    };
    let shell_b = cx
        .update(|app| {
            let client_init = Arc::clone(&client_b);
            let prefs_path = prefs_b.path().to_path_buf();
            let auth_b = auth_a.clone();
            app.new(|wcx| {
                let mut s = AppShell::new_with_client(client_init, wcx);
                s.preferences_dir = prefs_path;
                s.apply_authenticated_session_with_vault(auth_b, Some(bootstrap_login), wcx);
                s
            })
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let synced = shell_b
            .read_with(&cx, |s, _| {
                s.bot_browser.unlocked_vault_key.is_some()
                    && s.bot_browser
                        .cached_credential_metas
                        .iter()
                        .any(|m| m.credential_id == cred_id)
            })
            .unwrap();
        if synced {
            break;
        }
    }
    let (req_summary, item_keys) = {
        let st = mock_state.lock().unwrap();
        (
            st.requests
                .iter()
                .map(|(m, p, _)| format!("{m} {p}"))
                .collect::<Vec<_>>(),
            st.items.keys().cloned().collect::<Vec<_>>(),
        )
    };
    shell_b
        .read_with(&cx, |s, _| {
            assert_eq!(
                s.bot_browser.vault_custody_status,
                VaultCustodyStatus::UnlockedAndSynced,
                "status note={:?}, toast={:?}, metas={:?}, reqs={:?}",
                s.bot_browser.vault_custody_status_note,
                s.window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| &t.text),
                s.bot_browser.cached_credential_metas,
                req_summary,
            );
            let meta = s
                .bot_browser
                .cached_credential_metas
                .iter()
                .find(|m| m.credential_id == cred_id)
                .unwrap_or_else(|| {
                    panic!(
                        "credential {cred_id} not synced to Device B; cached_metas={:?}, items={:?}, reqs={:?}",
                        s.bot_browser.cached_credential_metas,
                        item_keys,
                        req_summary,
                    )
                });
            assert_eq!(meta.origin, "https://github.com");
            assert_eq!(meta.reuse_policy, ReusePolicy::ConfirmEveryTime);
            assert!(meta.has_totp);
        })
        .unwrap();

    // Test Settings delete emits encrypted tombstone downstream
    shell_a
        .update(&mut cx, |s, cx| {
            s.handle_settings_vault_action(
                &SettingsVaultAction::DeleteCredential {
                    credential_id: cred_id.clone(),
                },
                cx,
            );
        })
        .unwrap();
    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let has_delete = mock_state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, p, _)| m == "DELETE" && p.contains(&cred_id));
        if has_delete {
            break;
        }
    }
    assert!(mock_state
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, p, _)| m == "DELETE" && p.contains(&cred_id)));

    // C. Legacy V1 user silently uploads split_key_v2 envelope AND calls POST /api/v1/auth/upgrade-split-key
    ClientConfig::clear_session();
    let (legacy_url, legacy_mock) = rt.block_on(spawn_mock_cloud_vault_server(false));
    legacy_mock.lock().unwrap().auth_version = 1;
    let salt_v1 = generate_split_key_salt_b64();
    let bundle_v1 = derive_split_key("LegacyPass!99", &salt_v1, &params_v2).unwrap();
    let bootstrap_v1 = PendingAuthVaultBootstrap::SplitKeyV2 {
        split_kek: bundle_v1.vault_kek.clone(),
        auth_salt_b64: salt_v1.clone(),
        kdf_params: params_v2,
        legacy_upgrade_password: Some(Zeroizing::new("LegacyPass!99".to_string())),
        is_new_registration: false,
    };
    let prefs_v1 = tempfile::TempDir::new().unwrap();
    let mut cfg_v1 = ClientConfig::default();
    cfg_v1.server_url = legacy_url;
    let client_v1 = Arc::new(
        TelegramClient::open_with_dir(cfg_v1, prefs_v1.path()).expect("open isolated client v1"),
    );
    let auth_v1 = AuthResponse {
        user_id: 77_009,
        username: "legacy_v1_user".to_string(),
        token: "tok-v1".to_string(),
        session_id: "sess-v1".to_string(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    let shell_v1 = cx
        .update(|app| {
            let client_init = Arc::clone(&client_v1);
            let prefs_path = prefs_v1.path().to_path_buf();
            app.new(|wcx| {
                let mut s = AppShell::new_with_client(client_init, wcx);
                s.preferences_dir = prefs_path;
                s.apply_authenticated_session_with_vault(auth_v1, Some(bootstrap_v1), wcx);
                s
            })
        })
        .unwrap();

    for _ in 0..120 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let upgraded = legacy_mock.lock().unwrap().auth_version == 2
            && !legacy_mock.lock().unwrap().upgrade_requests.is_empty();
        let client_unlocked = shell_v1
            .read_with(&cx, |s, _| {
                s.bot_browser.vault_custody_status == VaultCustodyStatus::UnlockedAndSynced
            })
            .unwrap_or(false);
        if upgraded && client_unlocked {
            break;
        }
    }
    {
        let st = legacy_mock.lock().unwrap();
        assert_eq!(st.auth_version, 2);
        assert_eq!(st.upgrade_requests.len(), 1);
        assert_eq!(st.upgrade_requests[0].auth_salt_b64, salt_v1);
        assert_eq!(
            st.upgrade_requests[0].auth_verifier_b64,
            bundle_v1.auth_verifier_b64.as_str()
        );
        assert_eq!(
            st.envelope.as_ref().map(|e| e.kdf_alg.as_str()),
            Some("split_key_v2")
        );
    }
    shell_v1
        .read_with(&cx, |s, _| {
            assert_eq!(
                s.bot_browser.vault_custody_status,
                VaultCustodyStatus::UnlockedAndSynced
            );
        })
        .unwrap();
}

async fn body_appshell_device_trust_ring_x25519_approval_and_recovery_unlock(mut cx: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _rt_guard = rt.enter();
    let (base_url, mock_state) = rt.block_on(spawn_mock_cloud_vault_server(false));
    let user_id = 88_123u64;

    // Seed server with a device_keychain_v2 envelope and one encrypted password item
    let (envelope, root_key, recovery_key) =
        UserVaultCrypto::initialize_keychain_only(user_id).unwrap();
    let seed_dir = tempfile::TempDir::new().unwrap();
    let seed_vault =
        CredentialVault::open(seed_dir.path().join("vault.db"), &root_key).unwrap();
    let saved_meta = seed_vault
        .store_credential(
            "https://cloudflare.com/login",
            "cf_admin",
            "cf-secret-pw!",
            None,
            None,
            Some(ReusePolicy::ConfirmEveryTime),
        )
        .unwrap();
    let cred_id = saved_meta.credential_id.clone();
    let exported = seed_vault.export_for_sync(&cred_id).unwrap();
    let blob = root_key
        .seal_item(
            user_id,
            UserVaultCategory::Password,
            &cred_id,
            1,
            &serde_json::to_vec(&exported).unwrap(),
        )
        .unwrap();
    {
        let mut guard = mock_state.lock().unwrap();
        guard.envelope = Some(user_vault_envelope_to_proto(&envelope));
        guard.latest_revision = 1;
        guard.items.insert(
            ("password".to_string(), cred_id.clone()),
            UserVaultItemDto {
                category: "password".to_string(),
                item_key: cred_id.clone(),
                revision: 1,
                nonce_b64: blob.nonce_b64,
                ciphertext_b64: blob.ciphertext_b64,
                deleted: false,
                updated_at_ms: 1_700_000_555_000,
            },
        );
    }

    let auth = AuthResponse {
        user_id,
        username: "alipay_user".to_string(),
        token: "tok-pwdless".to_string(),
        session_id: "sess-pwdless".to_string(),
        first_name: Some("Alipay".to_string()),
        created_at: None,
        expires_at: None,
    };

    // Device A logs in (initially AwaitingDeviceApproval), unlocks via Settings Recovery Key
    let prefs_a = tempfile::TempDir::new().unwrap();
    let mut cfg_a = ClientConfig::default();
    cfg_a.server_url = base_url.clone();
    let client_a = Arc::new(
        TelegramClient::open_with_dir(cfg_a, prefs_a.path()).expect("open isolated client A"),
    );
    let shell_a = cx
        .update(|app| {
            let auth_a = auth.clone();
            let prefs_path = prefs_a.path().to_path_buf();
            app.new(|wcx| {
                let mut s = AppShell::new_with_client(Arc::clone(&client_a), wcx);
                s.preferences_dir = prefs_path;
                s.apply_authenticated_session_with_vault(
                    auth_a,
                    Some(PendingAuthVaultBootstrap::PasswordlessDeviceOnly),
                    wcx,
                );
                s
            })
        })
        .unwrap();

    for _ in 0..60 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let waiting = shell_a
            .read_with(&cx, |s, _| {
                matches!(
                    s.bot_browser.vault_custody_status,
                    VaultCustodyStatus::AwaitingDeviceApproval { .. }
                )
            })
            .unwrap();
        if waiting {
            break;
        }
    }

    shell_a
        .update(&mut cx, |s, cx| {
            s.handle_settings_vault_action(
                &SettingsVaultAction::SubmitRecoveryKey(recovery_key.clone()),
                cx,
            );
        })
        .unwrap();

    for _ in 0..80 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let unlocked = shell_a
            .read_with(&cx, |s, _| {
                s.bot_browser.unlocked_vault_key.is_some()
                    && s.bot_browser
                        .cached_credential_metas
                        .iter()
                        .any(|m| m.credential_id == cred_id)
            })
            .unwrap();
        if unlocked {
            break;
        }
    }
    shell_a
        .read_with(&cx, |s, _| {
            assert_eq!(
                s.bot_browser.vault_custody_status,
                VaultCustodyStatus::UnlockedAndSynced
            );
        })
        .unwrap();

    // Device B (fresh Mac on passwordless account): automatically requests X25519 device sync & starts poller
    ClientConfig::clear_session();
    let prefs_b = tempfile::TempDir::new().unwrap();
    let mut cfg_b = ClientConfig::default();
    cfg_b.server_url = base_url;
    let client_b = Arc::new(
        TelegramClient::open_with_dir(cfg_b, prefs_b.path()).expect("open isolated client B"),
    );
    let shell_b = cx
        .update(|app| {
            let auth_b = auth.clone();
            let prefs_path = prefs_b.path().to_path_buf();
            app.new(|wcx| {
                let mut s = AppShell::new_with_client(Arc::clone(&client_b), wcx);
                s.preferences_dir = prefs_path;
                s.apply_authenticated_session_with_vault(
                    auth_b,
                    Some(PendingAuthVaultBootstrap::PasswordlessDeviceOnly),
                    wcx,
                );
                s
            })
        })
        .unwrap();

    let mut device_b_req_id = String::new();
    let mut device_b_code = String::new();
    for _ in 0..80 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let awaiting = shell_b
            .read_with(&cx, |s, _| match &s.bot_browser.vault_custody_status {
                VaultCustodyStatus::AwaitingDeviceApproval {
                    request_id,
                    verification_code,
                    ..
                } => Some((request_id.clone(), verification_code.clone())),
                _ => None,
            })
            .unwrap();
        if let Some((rid, code)) = awaiting {
            device_b_req_id = rid;
            device_b_code = code;
            break;
        }
    }
    if device_b_req_id.is_empty() {
        let reqs_snapshot: Vec<String> = {
            mock_state
                .lock()
                .unwrap()
                .requests
                .iter()
                .map(|(m, p, _)| format!("{m} {p}"))
                .collect()
        };
        shell_b
            .read_with(&cx, |s, _| {
                panic!(
                    "Device B did not enter AwaitingDeviceApproval; status={:?}, note={:?}, unlocked={}, toast={:?}, reqs={:?}",
                    s.bot_browser.vault_custody_status,
                    s.bot_browser.vault_custody_status_note,
                    s.bot_browser.unlocked_vault_key.is_some(),
                    s.window_runtime
                        .controller
                        .current_toast
                        .as_ref()
                        .map(|t| &t.text),
                    reqs_snapshot,
                );
            })
            .unwrap();
    }

    // Device A refreshes pending device sync requests via Settings and approves Device B!
    shell_a
        .update(&mut cx, |s, cx| {
            s.handle_settings_vault_action(&SettingsVaultAction::RefreshVaultState, cx);
        })
        .unwrap();

    let mut pub_for_approve = String::new();
    for _ in 0..60 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let found = shell_a
            .read_with(&cx, |s, _| {
                s.bot_browser
                    .pending_device_approvals
                    .iter()
                    .find(|r| r.request_id == device_b_req_id)
                    .map(|r| (r.verification_code.clone(), r.device_public_key_b64.clone()))
            })
            .unwrap();
        if let Some((code, pk)) = found {
            assert_eq!(code, device_b_code);
            pub_for_approve = pk;
            break;
        }
    }
    assert!(!pub_for_approve.is_empty(), "Device A must see Device B pending request");

    shell_a
        .update(&mut cx, |s, cx| {
            s.handle_settings_vault_action(
                &SettingsVaultAction::ApproveDeviceSync {
                    request_id: device_b_req_id.clone(),
                    device_public_key_b64: pub_for_approve,
                },
                cx,
            );
        })
        .unwrap();

    // Device B poller automatically unwraps X25519 transfer, pulls synced credential, and unlocks!
    for _ in 0..100 {
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
        let synced = shell_b
            .read_with(&cx, |s, _| {
                s.bot_browser.vault_custody_status == VaultCustodyStatus::UnlockedAndSynced
                    && s.bot_browser.unlocked_vault_key.is_some()
                    && s.bot_browser
                        .cached_credential_metas
                        .iter()
                        .any(|m| m.credential_id == cred_id)
            })
            .unwrap();
        if synced {
            break;
        }
    }

    shell_b
        .update(&mut cx, |s, cx| {
            assert_eq!(
                s.bot_browser.vault_custody_status,
                VaultCustodyStatus::UnlockedAndSynced
            );
            assert!(s.bot_browser.unlocked_vault_key.is_some());
            assert!(s.bot_browser.pending_device_sync.is_none());
            assert!(s
                .bot_browser
                .cached_credential_metas
                .iter()
                .any(|m| m.credential_id == cred_id && m.origin == "https://cloudflare.com"));

            // Ensure zero /vault-setup or /vault-unlock prompts shown anywhere on Device B
            for t in s
                .window_runtime
                .controller
                .current_toast
                .iter()
                .chain(s.window_runtime.controller.toast_queue.iter())
            {
                assert!(!t.text.contains("/vault-setup"));
                assert!(!t.text.contains("/vault-unlock"));
            }

            // Logout clears all custody state in AppShell AND SettingsPrivacySecuritySections
            s.settings_router.update(cx, |r, rcx| {
                r.ensure_privacy(rcx);
            });
            s.sync_vault_custody_to_settings(cx);
            s.logout(cx);
            assert_eq!(
                s.bot_browser.vault_custody_status,
                VaultCustodyStatus::LockedOrUninitialized
            );
            assert!(s.bot_browser.cached_credential_metas.is_empty());
            if let Some(sp) = s.settings_router.read(cx).settings_privacy.clone() {
                assert_eq!(
                    sp.read(cx).vault_status,
                    VaultCustodyStatus::LockedOrUninitialized
                );
                assert!(sp.read(cx).saved_credentials.is_empty());
            }
        })
        .unwrap();
}


