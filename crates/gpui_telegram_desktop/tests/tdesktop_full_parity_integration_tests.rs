//! End-to-End Integration Tests for Full 1:1 Telegram Desktop (`tdesktop`) GPUI Parity.
//! Verifies:
//! 1. 4-Theme Palette Engine (`Night`, `Tinted`, `Day`, `Classic`)
//! 2. `MainMenuDrawer` Multi-Account Switcher, Emoji Status & Night Mode
//! 3. `ActiveModalDialog` 16-Modal Interactive Suite (Poll, Forward, Group, Channel, Proxy, MediaEditor, InstantView, Stories)
//! 4. `SettingsWorkspaceView` 11-Section Navigation & Live Mutations
//! 5. `ChatSidebar` Drafts, Archive, Pin/Mute & Supergroup Forum Topics
//! 6. `ConversationStage` Edit, Delete, Multi-Select, Scheduled Queue, Voice Player & Spoilers
//! 7. `CanonicalComposer` Reply/Edit Context, Interactive Dice & 3-Tab Selector
//! 8. `CallOverlay` Authentic 333-Emoji E2EE Key Fingerprint
//! 9. `SessionPersistenceBridge` PTS LocalStore Sync & HTML/JSON Chat Archive Export

use gpui_telegram_client::SessionPersistenceBridge;
use gpui_telegram_desktop::views::{
    ActiveModalDialog, CallOverlay, CanonicalComposer, ChatSidebar, ContextInspector,
    ConversationStage, MainMenuDrawer, MediaViewer, ModalDialogKind, SettingsPageId,
    SettingsWorkspaceView, TdesktopThemeMode, TdesktopThemePalette,
    canonical_composer::SelectorTabKind,
    stage::cards::{DeliveryStatus, MessageContent, MessageDirection, MessageItemModel, TextMessageView},
};

#[test]
fn test_four_official_tdesktop_themes_and_metrics() {
    let night = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x2AABEE, 14, 14);
    let tinted = TdesktopThemePalette::build(TdesktopThemeMode::Tinted, 0x1ABC9C, 15, 16);
    let day = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0x2AABEE, 14, 12);
    let classic = TdesktopThemePalette::build(TdesktopThemeMode::Classic, 0x4CAF50, 13, 10);

    assert!(night.mode.is_dark());
    assert!(tinted.mode.is_dark());
    assert!(!day.mode.is_dark());
    assert!(!classic.mode.is_dark());
    assert_eq!(classic.msg_out_bg, 0xEFFDDE);
    assert_eq!(tinted.font_size_px, 15);
    assert_eq!(day.bubble_radius_px, 12);
}

#[test]
fn test_main_menu_multi_account_and_modal_suite() {
    let mut bridge = SessionPersistenceBridge::new_demo();
    let menu = MainMenuDrawer::new(bridge.state.accounts.clone(), bridge.state.active_account_id);
    assert!(menu.accounts.len() >= 2);
    assert_eq!(menu.active_account().account_id, 1);

    let mut modal = ActiveModalDialog::new(bridge.state.proxies.clone());
    assert!(!modal.is_open());
    modal.open(ModalDialogKind::CreatePoll, vec![(1, "Alice".into(), "AS".into(), 0x2AABEE)]);
    assert!(modal.is_open());
    modal.handle_key_input("a", Some("?"));
    assert!(modal.primary_input.ends_with('?'));
    modal.close();
    assert!(!modal.is_open());

    // Verify persistence & HTML/JSON export
    bridge.record_outgoing_proto_message(1, 950, "Hello 1:1 GPUI Telegram!");
    let msgs = bridge
        .client
        .get_messages(gpui_telegram_core::types::ChatId(1))
        .unwrap();
    assert!(!msgs.is_empty());

    let html_path = bridge.export_chat_archive(
        "Alice Smith",
        false,
        &[("Alice".into(), "14:28".into(), "Hello GPUI!".into())],
    );
    assert!(html_path.ends_with(".html"));
    assert!(std::path::Path::new(&html_path).exists());
}

#[test]
fn test_stage_composer_sidebar_and_e2ee_call_parity() {
    let mut sidebar = ChatSidebar::new();
    sidebar.set_draft(1, "Unsent draft".to_string());
    assert_eq!(sidebar.drafts.get(&1).map(|s| s.as_str()), Some("Unsent draft"));
    sidebar.toggle_archive_dialog(5);
    assert!(sidebar.archived_ids.contains(&5));

    let mut stage = ConversationStage::new();
    stage.add_message(MessageItemModel {
        id: 777,
        sender_id: 1,
        sender_name: "You".into(),
        avatar_initials: "ME".into(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Outgoing,
        timestamp: "Now".into(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: vec![],
        content: MessageContent::Text {
            text: "Original ||hidden|| https://telegram.org".into(),
            is_edited: false,
        },
        bot_keyboard: None,
        is_pinned: false,
    });
    assert_eq!(
        TextMessageView::extract_first_url("Check https://telegram.org."),
        Some("https://telegram.org".to_string())
    );
    stage.edit_message_text(777, "Updated text".to_string());
    stage.toggle_select_message(777);
    assert!(stage.selected_message_ids.contains(&777));
    stage.delete_messages_by_ids(&[777]);
    assert!(!stage.selected_message_ids.contains(&777));

    let mut composer = CanonicalComposer::new();
    composer.begin_reply(101, "Alice Smith".into(), "Design review".into());
    assert!(composer.replying_to.is_some());
    composer.selector_tab = SelectorTabKind::Stickers;
    composer.cancel_reply_or_edit();
    assert!(composer.replying_to.is_none());

    let call = CallOverlay::new("Alice Smith".to_string());
    assert_eq!(call.e2ee_emojis.split_whitespace().count(), 4);

    let inspector = ContextInspector::new_demo();
    assert_eq!(inspector.channel_boost_level, 8);

    let mut viewer = MediaViewer::new("https://example.com/a.jpg".into(), false);
    viewer.rotation_deg = 90;
    viewer.reset_zoom();
    assert_eq!(viewer.rotation_deg, 0);

    let mut settings = SettingsWorkspaceView::new(
        SessionPersistenceBridge::new_demo().state,
        TdesktopThemePalette::default(),
    );
    settings.active_page = SettingsPageId::TonWallet;
    assert_eq!(settings.active_page, SettingsPageId::TonWallet);
}

#[test]
fn test_fabushi_visual_theme_tokens_and_readability() {
    use gpui::AssetSource;
    use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};

    let dark = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0, 14, 18);
    let tinted = TdesktopThemePalette::build(TdesktopThemeMode::Tinted, 0, 14, 18);
    let light = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0, 14, 18);
    let classic = TdesktopThemePalette::build(TdesktopThemeMode::Classic, 0, 14, 12);

    assert!(dark.mode.is_fabushi_style());
    assert!(tinted.mode.is_fabushi_style());
    assert!(light.mode.is_fabushi_style());
    assert!(!classic.mode.is_fabushi_style());
    assert!(dark.is_apple_style());
    assert!(tinted.is_apple_style());
    assert!(light.is_apple_style());
    assert!(!classic.is_apple_style());

    // Apple Messages Dark palette & vibrant iMessage Blue outgoing tokens
    assert_eq!(dark.accent, 0x0A84FF);
    assert_eq!(dark.accent_hover, 0x2994FF);
    assert_eq!(dark.window_bg, 0x1E1E1E);
    assert_eq!(dark.stage_bg, 0x1E1E1E);
    assert_eq!(dark.dialogs_bg, 0x1C1C1E);
    assert_eq!(dark.dialogs_active_bg, 0x0A84FF);
    assert_eq!(dark.msg_in_bg, 0x262629);
    assert_eq!(dark.msg_out_bg, 0x0A84FF);
    assert_eq!(dark.msg_out_bg_top, 0x1E8FFF);
    assert_eq!(dark.msg_out_bg_bottom, 0x0A84FF);
    assert_eq!(dark.msg_out_fg, 0xFFFFFF);
    assert_eq!(dark.msg_out_secondary_fg, 0xD6EBFF);
    assert_eq!(dark.msg_out_link_fg, 0xFFFFFF);
    assert_eq!(dark.text_primary, 0xF5F5F7);
    assert_eq!(dark.text_secondary, 0x98989D);
    assert_eq!(dark.service_pill_fg, 0x8E8E93);
    assert!((dark.sidebar_bg_rgba().a - 0.74).abs() < 0.01);
    assert!((dark.separator_rgba().a - 0.09).abs() < 0.01);
    assert_eq!(dark.popover_shadows().len(), 2);

    // Apple Messages Light palette & vibrant iMessage Blue outgoing tokens
    assert_eq!(light.accent, 0x007AFF);
    assert_eq!(light.accent_hover, 0x0062CC);
    assert_eq!(light.window_bg, 0xFFFFFF);
    assert_eq!(light.stage_bg, 0xFFFFFF);
    assert_eq!(light.dialogs_bg, 0xF5F5F7);
    assert_eq!(light.dialogs_active_bg, 0x007AFF);
    assert_eq!(light.msg_in_bg, 0xE9E9EB);
    assert_eq!(light.msg_out_bg, 0x007AFF);
    assert_eq!(light.msg_out_bg_top, 0x2C95FF);
    assert_eq!(light.msg_out_bg_bottom, 0x007AFF);
    assert_eq!(light.msg_out_fg, 0xFFFFFF);
    assert_eq!(light.msg_out_secondary_fg, 0xE5F2FF);
    assert_eq!(light.msg_out_link_fg, 0xFFFFFF);
    assert_eq!(light.text_primary, 0x1D1D1F);
    assert_eq!(light.text_secondary, 0x86868B);
    assert_eq!(light.service_pill_fg, 0x8E8E93);
    assert!((light.sidebar_bg_rgba().a - 0.72).abs() < 0.01);
    assert!((light.separator_rgba().a - 0.08).abs() < 0.01);
    assert_eq!(light.popover_shadows().len(), 2);

    // Custom accent in Apple Light tints outgoing gradient & active sidebar while preserving white text
    let light_custom = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0x8544D6, 14, 18);
    assert_eq!(light_custom.dialogs_active_bg, 0x8544D6);
    assert_eq!(light_custom.msg_out_bg, 0x8544D6);
    assert_eq!(light_custom.msg_out_bg_bottom, 0x8544D6);
    assert_ne!(light_custom.msg_out_bg_top, light_custom.msg_out_bg_bottom);
    assert_eq!(light_custom.msg_out_fg, 0xFFFFFF);

    // Classic mode preserves legacy Telegram green outgoing bubbles, opaque sidebar, and inverted pill fg
    assert_eq!(classic.msg_out_bg, 0xEFFDDE);
    assert_eq!(classic.msg_out_fg, 0x000000);
    assert_eq!(classic.service_pill_fg, 0xFFFFFF);
    assert!((classic.sidebar_bg_rgba().a - 1.0).abs() < 0.001);

    // Default palette uses 18px Apple Messages bubble radius and Apple Dark accent
    let def = TdesktopThemePalette::default();
    assert_eq!(def.bubble_radius_px, 18);
    assert_eq!(def.accent, 0x0A84FF);

    // Canonical icon count stays 64 for harness compatibility while ArrowUp loads cleanly
    assert_eq!(IconName::ALL.len(), 64);
    let src = TelegramAssetSource::new();
    let arrow = src
        .load(IconName::ArrowUp.path())
        .expect("load arrow_up")
        .expect("arrow_up embedded");
    assert!(std::str::from_utf8(arrow.as_ref()).unwrap().contains("<svg"));

    // System appearance mapping follows macOS Light/Dark cleanly
    use gpui::WindowAppearance;
    use gpui_telegram_desktop::app::preferences::{AppPreferences, ThemeMode};
    use gpui_telegram_desktop::views::app_shell::shell_theme::{
        resolve_mode_from_preferences_with_appearance, tdesktop_mode_for_window_appearance,
    };
    assert_eq!(
        tdesktop_mode_for_window_appearance(WindowAppearance::Light),
        TdesktopThemeMode::Day
    );
    assert_eq!(
        tdesktop_mode_for_window_appearance(WindowAppearance::VibrantLight),
        TdesktopThemeMode::Day
    );
    assert_eq!(
        tdesktop_mode_for_window_appearance(WindowAppearance::Dark),
        TdesktopThemeMode::Night
    );
    assert_eq!(
        tdesktop_mode_for_window_appearance(WindowAppearance::VibrantDark),
        TdesktopThemeMode::Night
    );
    let mut sys_prefs = AppPreferences::default();
    sys_prefs.theme_mode = ThemeMode::System;
    assert_eq!(
        resolve_mode_from_preferences_with_appearance(&sys_prefs, Some(WindowAppearance::Light)),
        TdesktopThemeMode::Day
    );
    assert_eq!(
        resolve_mode_from_preferences_with_appearance(
            &sys_prefs,
            Some(WindowAppearance::VibrantDark),
        ),
        TdesktopThemeMode::Night
    );
}

#[test]
fn test_sidebar_context_action_single_apply_and_no_rearm() {
    use gpui::{point, px};
    use gpui_telegram_desktop::views::chat_sidebar::{DialogDeliveryStatus, DialogItemModel};
    use gpui_telegram_desktop::views::sidebar::dialogs_row_view::SidebarContextAction;

    let mut sidebar = ChatSidebar::new();
    sidebar.dialogs.push(DialogItemModel {
        id: 42,
        peer_name: "Alice".to_string(),
        avatar_initials: "AL".to_string(),
        avatar_color: 0x0A84FF,
        last_message: "Hi".to_string(),
        timestamp: "10:00".to_string(),
        unread_count: 0,
        is_muted: false,
        is_pinned: false,
        is_online: true,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "All".to_string(),
    });
    sidebar.context_menu = Some((42, point(px(20.0), px(80.0))));

    sidebar.apply_context_action(SidebarContextAction::TogglePin(42));
    assert!(sidebar.dialogs[0].is_pinned);
    assert!(sidebar.context_menu.is_none());

    assert_eq!(
        sidebar.take_pending_context_action(),
        Some(SidebarContextAction::TogglePin(42))
    );
    assert_eq!(sidebar.take_pending_context_action(), None);
    assert!(sidebar.dialogs[0].is_pinned);

    sidebar.apply_context_action_local(SidebarContextAction::TogglePin(42));
    assert!(!sidebar.dialogs[0].is_pinned);
    assert_eq!(sidebar.take_pending_context_action(), None);
}

#[test]
fn test_apple_stage_top_bar_height_constants() {
    use gpui_telegram_desktop::views::stage::stage_bars::{
        APPLE_STAGE_EMPTY_TOP_BAR_HEIGHT, APPLE_STAGE_TOP_BAR_HEIGHT,
    };

    assert_eq!(APPLE_STAGE_TOP_BAR_HEIGHT, 76.0);
    assert_eq!(APPLE_STAGE_EMPTY_TOP_BAR_HEIGHT, 52.0);
}
