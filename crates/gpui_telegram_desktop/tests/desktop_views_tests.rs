//! Comprehensive Automated Tests for GPUI Desktop Views (`crates/gpui_telegram_desktop`).
//!
//! Covers:
//! 1. ConversationStage & 11 Canonical Message Cards:
//!    - TextMessageView (reply references, delivery status, reactions toggle)
//!    - MediaMessageView (photo, video, aspect ratio, duration, streaming)
//!    - VoiceMessageView (5-bit waveform renderer samples in [0, 31], duration, speed)
//!    - AlbumMessageView (mosaic collage items, caption)
//!    - LocationMessageView (coordinates, venue, address)
//!    - PollMessageView (quiz vs regular, voting calculation, percentage bars)
//!    - ServiceMessageView (centered system events, date divider)
//!    - CallMessageView (voice/video, incoming/outgoing/missed)
//!    - StoryMessageView (story reply, ring, expiry)
//!    - InvoiceMessageView (Telegram Stars ⭐ payments, receipt status)
//!    - InlineBotKeyboard (callback, URL, web_app buttons)
//! 2. CanonicalComposer:
//!    - IME safety state machine (Idle, Composing, Committed, Enter safety guard)
//!    - Voice hold-to-record gesture state machine (Hold, Slide-to-cancel, Lock, Complete, Discard)
//!    - Formatting bar & Attachments drawer
//! 3. ContextInspectorDrawer:
//!    - 320px width contract & open/close toggle
//!    - Tab switching: Profile, Members, Shared Media, Settings
//! 4. Capability Overlays:
//!    - Talk Mode Orb Overlay (full-duplex states, live transcript, mute)
//!    - Remote Computer Workbench (host specs, capture toggles, HD/SD quality)
//!    - Media Viewer Lightbox (zoom levels, gallery next/prev, video playback)
//! 5. AppShell Integration:
//!    - Multi-view orchestration and overlay presentations

use gpui_telegram_desktop::{views::*, TelegramApp};
use gpui_telegram_desktop::views::conversation_stage::*;
use gpui::{px, Point, Size};

// ============================================================================
// 1. ConversationStage & 11 Canonical Message Cards Tests
// ============================================================================

#[test]
fn test_conversation_stage_initialization_and_metadata() {
    let mut stage = ConversationStage::new_demo();
    assert_eq!(stage.active_chat_id, Some(1));
    assert_eq!(stage.chat_title, "Alice Smith");
    assert_eq!(stage.chat_subtitle, "online");
    assert_eq!(stage.chat_avatar_initials, "AS");
    assert!(stage.is_inspector_open);
    assert!(stage.pinned_message.is_some());

    // Switch chat
    stage.set_chat(42, "Rust Dev Group", "12 members", "RD", 0x9B59B6);
    assert_eq!(stage.active_chat_id, Some(42));
    assert_eq!(stage.chat_title, "Rust Dev Group");
    assert_eq!(stage.chat_subtitle, "12 members");
    assert_eq!(stage.chat_avatar_initials, "RD");

    // Toggle inspector
    stage.toggle_inspector();
    assert!(!stage.is_inspector_open);
    stage.toggle_inspector();
    assert!(stage.is_inspector_open);
}

#[test]
fn test_conversation_stage_demo_messages_cover_11_canonical_cards() {
    let stage = ConversationStage::new_demo();
    assert!(stage.message_count() >= 10);

    let mut has_service = false;
    let mut has_text = false;
    let mut has_voice = false;
    let mut has_media = false;
    let mut has_album = false;
    let mut has_location = false;
    let mut has_poll = false;
    let mut has_call = false;
    let mut has_story = false;
    let mut has_invoice = false;
    let mut has_bot_keyboard = false;

    for msg in &stage.messages {
        match &msg.content {
            MessageContent::Service { .. } => has_service = true,
            MessageContent::Text { .. } => has_text = true,
            MessageContent::Voice { .. } => has_voice = true,
            MessageContent::Media { .. } => has_media = true,
            MessageContent::Album { .. } => has_album = true,
            MessageContent::Location { .. } => has_location = true,
            MessageContent::Poll { .. } => has_poll = true,
            MessageContent::Call { .. } => has_call = true,
            MessageContent::Story { .. } => has_story = true,
            MessageContent::Invoice { .. } => has_invoice = true,
            MessageContent::BotKeyboard { .. } => has_bot_keyboard = true,
            MessageContent::GenerativeCard(_) => {}
            MessageContent::LocalBotAssistCard(_) => {}
        }
        if msg.bot_keyboard.is_some() {
            has_bot_keyboard = true;
        }
    }

    assert!(has_service, "Missing Service message card");
    assert!(has_text, "Missing Text message card");
    assert!(has_voice, "Missing Voice message card");
    assert!(has_media, "Missing Media message card");
    assert!(has_album, "Missing Album message card");
    assert!(has_location, "Missing Location message card");
    assert!(has_poll, "Missing Poll message card");
    assert!(has_call, "Missing Call message card");
    assert!(has_story, "Missing Story message card");
    assert!(has_invoice, "Missing Invoice message card");
    assert!(has_bot_keyboard, "Missing InlineBotKeyboard card");
}

#[test]
fn test_canonical_text_message_reactions_toggle() {
    let mut stage = ConversationStage::new_demo();
    let text_msg_id = 101;

    // Initially has 👍 and 🔥
    let msg = stage.messages.iter().find(|m| m.id == text_msg_id).unwrap();
    let thumbs = msg.reactions.iter().find(|r| r.emoji == "👍").unwrap();
    assert_eq!(thumbs.count, 3);
    assert!(thumbs.user_reacted);

    // Toggle existing reacted emoji -> removes user reaction and decrements count
    stage.toggle_reaction(text_msg_id, "👍");
    let msg = stage.messages.iter().find(|m| m.id == text_msg_id).unwrap();
    let thumbs = msg.reactions.iter().find(|r| r.emoji == "👍").unwrap();
    assert_eq!(thumbs.count, 2);
    assert!(!thumbs.user_reacted);

    // Toggle new emoji -> adds reaction
    stage.toggle_reaction(text_msg_id, "❤️");
    let msg = stage.messages.iter().find(|m| m.id == text_msg_id).unwrap();
    let heart = msg.reactions.iter().find(|r| r.emoji == "❤️").unwrap();
    assert_eq!(heart.count, 1);
    assert!(heart.user_reacted);
}

#[test]
fn test_canonical_voice_message_5bit_waveform_renderer_samples() {
    let stage = ConversationStage::new_demo();
    let voice_msg = stage.messages.iter().find(|m| matches!(m.content, MessageContent::Voice { .. })).unwrap();

    if let MessageContent::Voice {
        duration_secs,
        ref waveform_5bit,
        playback_position_secs,
        playback_speed,
        ..
    } = voice_msg.content
    {
        assert_eq!(duration_secs, 42);
        assert!(playback_position_secs > 0.0);
        assert_eq!(playback_speed, 1.0);
        assert!(!waveform_5bit.is_empty());

        // Validate that each sample is authentic 5-bit (0..31)
        for &sample in waveform_5bit {
            assert!(sample <= 31, "Waveform sample {} exceeds 5-bit limit (31)!", sample);
        }
    } else {
        panic!("Expected Voice message");
    }
}

#[test]
fn test_canonical_poll_voting_and_percentages() {
    let mut stage = ConversationStage::new_demo();
    let poll_msg_id = 106;

    // Vote for option 2
    stage.vote_poll(poll_msg_id, 2);

    let msg = stage.messages.iter().find(|m| m.id == poll_msg_id).unwrap();
    if let MessageContent::Poll {
        total_votes,
        ref options,
        selected_option_id,
        ..
    } = msg.content
    {
        assert_eq!(selected_option_id, Some(2));
        assert_eq!(total_votes, 51);
        let opt2 = options.iter().find(|o| o.id == 2).unwrap();
        assert_eq!(opt2.votes, 6);
        assert!(opt2.is_selected);
        // Total percentage sum should be roughly 100%
        let sum_pct: f32 = options.iter().map(|o| o.percentage).sum();
        assert!((sum_pct - 100.0).abs() < 2.0);
    } else {
        panic!("Expected Poll message");
    }
}

#[test]
fn test_canonical_invoice_stars_payment_card() {
    let stage = ConversationStage::new_demo();
    let invoice_msg = stage.messages.iter().find(|m| matches!(m.content, MessageContent::Invoice { .. })).unwrap();

    if let MessageContent::Invoice {
        ref title,
        amount_stars,
        ref currency,
        status,
        ref receipt_id,
        ..
    } = invoice_msg.content
    {
        assert_eq!(title, "Cloud Agent Workspace Subscription");
        assert_eq!(amount_stars, 250);
        assert_eq!(currency, "Telegram Stars");
        assert_eq!(status, InvoiceStatus::Paid);
        assert_eq!(receipt_id.as_deref(), Some("TG-STARS-948123"));
    } else {
        panic!("Expected Invoice message");
    }
}

// ============================================================================
// 2. CanonicalComposer & State Machines Tests
// ============================================================================

#[test]
fn test_ime_safety_machine_enter_key_behavior() {
    let mut ime = ImeSafetyMachine::new();
    assert_eq!(ime.state, ImeState::Idle);

    // 1. Idle with text -> Enter sends message
    ime.insert_text("Hello world");
    assert_eq!(ime.handle_enter_key(false), KeyAction::SendMessage);

    // 2. Idle with Shift+Enter -> Inserts newline
    assert_eq!(ime.handle_enter_key(true), KeyAction::InsertNewline);
    assert_eq!(ime.text(), "Hello world\n");

    // 3. User begins IME composition (typing Pinyin 'nihao')
    ime.on_ime_start();
    assert!(ime.is_composing());
    ime.on_ime_update("你好", 2);
    assert_eq!(ime.preedit_text(), Some("你好"));

    // Critical: When IME is composing, Enter commits candidate and MUST NOT send the message!
    let action = ime.handle_enter_key(false);
    assert_eq!(action, KeyAction::ImeCandidateChosen);
    assert!(matches!(ime.state, ImeState::Committed { .. }));
    assert!(ime.just_committed);
    assert!(ime.text().contains("你好"));

    // Trailing Cocoa Return event in same tick is absorbed by commit fence!
    let action_trailing = ime.handle_enter_key(false);
    assert_eq!(action_trailing, KeyAction::ImeCandidateChosen);
    assert_eq!(ime.state, ImeState::Idle);
    assert!(!ime.just_committed);

    // Subsequent Enter now sends the message!
    let action_send = ime.handle_enter_key(false);
    assert_eq!(action_send, KeyAction::SendMessage);
}

#[test]
fn test_voice_hold_to_record_gesture_state_machine() {
    let mut gesture = VoiceGestureMachine::new();
    assert_eq!(gesture.state, VoiceGestureState::Idle);

    // Case A: Short tap (<0.5s) is discarded
    gesture.on_press_start(100.0, 200.0);
    assert!(gesture.is_recording());
    gesture.tick(0.2, 0.5);
    gesture.on_press_end();
    assert_eq!(gesture.state, VoiceGestureState::Discarded);

    // Case B: Normal hold (>0.5s) completes successfully
    gesture.on_press_start(100.0, 200.0);
    gesture.tick(0.6, 0.8);
    gesture.tick(0.4, 0.6);
    gesture.on_press_end();
    if let VoiceGestureState::Completed { duration_secs, ref waveform } = gesture.state {
        assert!(duration_secs >= 1.0);
        assert_eq!(waveform.len(), 2);
        for &s in waveform {
            assert!(s <= 31);
        }
    } else {
        panic!("Expected Completed voice recording");
    }

    // Case C: Slide left cancels recording
    gesture.on_press_start(100.0, 200.0);
    gesture.tick(0.5, 0.4);
    gesture.on_pointer_move(10.0, 200.0); // delta_x = -90px < -80px threshold
    assert!(matches!(gesture.state, VoiceGestureState::SlideCanceling { .. }));
    gesture.on_press_end();
    assert_eq!(gesture.state, VoiceGestureState::Discarded);

    // Case D: Slide up locks into hands-free recording
    gesture.on_press_start(100.0, 200.0);
    gesture.tick(0.5, 0.4);
    gesture.on_pointer_move(100.0, 140.0); // delta_y = -60px < -50px threshold
    assert!(gesture.is_locked());
    gesture.on_press_end();
    assert!(gesture.is_locked(), "Locked recording should stay locked after releasing mouse");
    gesture.tick(1.0, 0.7);
    gesture.stop_and_send();
    assert!(matches!(gesture.state, VoiceGestureState::Completed { .. }));
}

#[test]
fn test_canonical_composer_rich_formatting_and_attachments() {
    let mut composer = CanonicalComposer::new();
    assert_eq!(composer.text(), "");
    assert!(!composer.is_recording_voice());

    // Formatting Bar toggles
    assert!(!composer.is_formatting_open);
    composer.toggle_formatting();
    assert!(composer.is_formatting_open);
    composer.formatting_bar.toggle_style(TextStyleKind::Bold);
    assert!(composer.formatting_bar.has_style(TextStyleKind::Bold));
    assert!(!composer.formatting_bar.has_style(TextStyleKind::Italic));

    // Attachments Drawer toggles
    assert!(!composer.attachments_drawer.is_open);
    composer.toggle_attachments();
    assert!(composer.attachments_drawer.is_open);
    composer.toggle_attachments();
    assert!(!composer.attachments_drawer.is_open);

    // Set text and clear
    composer.set_text("Testing multi-line composer");
    assert_eq!(composer.text(), "Testing multi-line composer");
    composer.clear();
    assert_eq!(composer.text(), "");
}

// ============================================================================
// 3. ContextInspectorDrawer Tests
// ============================================================================

#[test]
fn test_context_inspector_drawer_320px_and_tabs() {
    let mut inspector = ContextInspector::new_demo();
    assert!(inspector.is_open);
    assert_eq!(inspector.active_tab, InspectorTab::Profile);

    // Profile Details
    assert_eq!(inspector.profile.name, "Alice Smith");
    assert_eq!(inspector.profile.username, "@alice_smith");
    assert!(inspector.profile.is_notifications_enabled);

    // Switch to Members tab
    inspector.set_tab(InspectorTab::Members);
    assert_eq!(inspector.active_tab, InspectorTab::Members);
    assert_eq!(inspector.members.members.len(), 2);
    assert_eq!(inspector.members.members[0].role, "Owner");

    // Switch to Shared Media tab
    inspector.set_tab(InspectorTab::SharedMedia);
    assert_eq!(inspector.active_tab, InspectorTab::SharedMedia);
    assert_eq!(inspector.shared_media.files.len(), 0);
    assert_eq!(inspector.shared_media.links.len(), 0);

    // Switch to Settings tab
    inspector.set_tab(InspectorTab::Settings);
    assert_eq!(inspector.active_tab, InspectorTab::Settings);
    assert!(!inspector.settings.e2ee_fingerprint.is_empty());

    // Auto-delete timer setting
    inspector.set_auto_delete_period("24 Hours");
    assert_eq!(inspector.settings.auto_delete_period, "24 Hours");

    // Close and open drawer
    inspector.close();
    assert!(!inspector.is_open);
    inspector.open();
    assert!(inspector.is_open);
}

// ============================================================================
// 4. Capability Overlays Tests
// ============================================================================

#[test]
fn test_talk_mode_orb_overlay_state_machine_and_transcript() {
    let mut overlay = CallOverlay::new("Alice Smith".to_string());
    assert_eq!(overlay.state, TalkModeState::Listening);
    assert!(!overlay.is_mic_muted);
    assert_eq!(overlay.transcript.len(), 0, "Authentic CallOverlay starts with an empty transcript");

    // Mute microphone
    overlay.toggle_mute();
    assert!(overlay.is_mic_muted);
    assert_eq!(overlay.state, TalkModeState::Muted);

    // Unmute
    overlay.toggle_mute();
    assert!(!overlay.is_mic_muted);
    assert_eq!(overlay.state, TalkModeState::Listening);

    // Transition state
    overlay.set_state(TalkModeState::Thinking);
    assert_eq!(overlay.state, TalkModeState::Thinking);
    overlay.set_state(TalkModeState::Speaking);
    assert_eq!(overlay.state, TalkModeState::Speaking);

    // Add transcript turn
    overlay.add_transcript("You", "Run full test suite", true);
    assert_eq!(overlay.transcript.len(), 1);
    assert_eq!(overlay.transcript.last().unwrap().text, "Run full test suite");
}

#[test]
fn test_remote_computer_workbench_controls() {
    let mut workbench = ComputerWorkbench::new("Ubuntu-Remote-Dev".to_string());
    assert_eq!(workbench.device.name, "Ubuntu-Remote-Dev");
    assert_eq!(workbench.device.fps, 60);
    assert!(workbench.device.is_tunnel_encrypted);
    assert!(workbench.is_mouse_captured);
    assert!(workbench.is_keyboard_captured);
    assert!(workbench.is_quality_hd);

    // Toggle controls
    workbench.toggle_mouse_capture();
    assert!(!workbench.is_mouse_captured);
    workbench.toggle_keyboard_capture();
    assert!(!workbench.is_keyboard_captured);
    workbench.toggle_quality();
    assert!(!workbench.is_quality_hd);
}

#[test]
fn test_media_viewer_lightbox_navigation_and_zoom() {
    let mut viewer = MediaViewer::new_demo();
    assert_eq!(viewer.items.len(), 3);
    assert_eq!(viewer.current_index, 0);
    assert_eq!(viewer.zoom_level, 1.0);

    // Next item
    viewer.next();
    assert_eq!(viewer.current_index, 1);
    // Prev item
    viewer.prev();
    assert_eq!(viewer.current_index, 0);

    // Zoom controls
    viewer.zoom_in();
    assert_eq!(viewer.zoom_level, 1.25);
    viewer.zoom_in();
    assert_eq!(viewer.zoom_level, 1.5);
    viewer.zoom_out();
    assert_eq!(viewer.zoom_level, 1.25);
    viewer.reset_zoom();
    assert_eq!(viewer.zoom_level, 1.0);

    // Toggle video playback
    assert!(!viewer.is_playing);
    viewer.toggle_play();
    assert!(viewer.is_playing);
}

// ============================================================================
// 5. App Integration Tests
// ============================================================================

#[test]
fn test_app_shell_views_and_overlay_lifecycle() {
    // Verify TelegramApp constructor
    let _app = TelegramApp::new();

    // Verify submodules exist and expose correct types
    let stage = ConversationStage::new_demo();
    assert!(stage.message_count() > 0);

    let composer = CanonicalComposer::new();
    assert_eq!(composer.text(), "");

    let inspector = ContextInspector::new_demo();
    assert!(inspector.is_open);
}

#[test]
fn test_interactive_click_state_transitions_and_modal_container() {
    // 1. Poll option deselection when voting across all 3 options
    let mut stage = ConversationStage::new_demo();
    let poll_msg_id = 106;
    stage.vote_poll(poll_msg_id, 1);
    stage.vote_poll(poll_msg_id, 2);
    stage.vote_poll(poll_msg_id, 3);
    let msg = stage.messages.iter().find(|m| m.id == poll_msg_id).unwrap();
    if let MessageContent::Poll { ref options, selected_option_id, .. } = msg.content {
        assert_eq!(selected_option_id, Some(3));
        let opt1 = options.iter().find(|o| o.id == 1).unwrap();
        let opt2 = options.iter().find(|o| o.id == 2).unwrap();
        let opt3 = options.iter().find(|o| o.id == 3).unwrap();
        assert!(!opt1.is_selected, "Option 1 should be deselected");
        assert!(!opt2.is_selected, "Option 2 should be deselected");
        assert!(opt3.is_selected, "Newly voted option 3 should be selected");
    } else {
        panic!("Expected Poll message");
    }

    // 2. Text message click pins snippet and toggles reaction
    stage.pinned_message = Some("Hey! Have you tested the new GPUI 120fps Telegram Desktop build?".to_string());
    stage.toggle_reaction(101, "👍");
    assert!(stage.pinned_message.as_deref().unwrap().contains("120fps"));
    let msg101 = stage.messages.iter().find(|m| m.id == 101).unwrap();
    assert!(msg101.reactions.iter().any(|r| r.emoji == "👍"));

    // 3. ContextInspector peer synchronization, quick actions, and interactive sub-tabs
    let mut inspector = ContextInspector::new_demo();
    inspector.set_peer("Rust Core Team", "12 members", "RC", 0xE67E22);
    assert_eq!(inspector.profile.name, "Rust Core Team");
    assert!(inspector.profile.username.is_empty(), "Display names must not invent usernames");
    assert_eq!(inspector.profile.avatar_initials, "RC");
    assert_eq!(inspector.profile.avatar_color, 0xE67E22);

    inspector.is_secret_chat_active = !inspector.is_secret_chat_active;
    assert!(inspector.is_secret_chat_active);
    inspector.is_contact_blocked = !inspector.is_contact_blocked;
    assert!(inspector.is_contact_blocked);
    inspector.pending_media_open = Some(("https://gpui.rs/docs".to_string(), false));
    assert_eq!(inspector.pending_media_open.as_ref().unwrap().0, "https://gpui.rs/docs");

    inspector.shared_media.active_subtab = drawer::inspector_tabs::SharedMediaSubTab::Files;
    assert_eq!(inspector.shared_media.active_subtab, drawer::inspector_tabs::SharedMediaSubTab::Files);
    inspector.shared_media.active_subtab = drawer::inspector_tabs::SharedMediaSubTab::Voice;
    assert_eq!(inspector.shared_media.active_subtab, drawer::inspector_tabs::SharedMediaSubTab::Voice);
    inspector.shared_media.active_subtab = drawer::inspector_tabs::SharedMediaSubTab::Links;
    assert_eq!(inspector.shared_media.active_subtab, drawer::inspector_tabs::SharedMediaSubTab::Links);

    // 4. ModalContainer push/dismiss stack (including "Docs" inline bot keyboard modal)
    let mut modal = ModalContainer::new();
    assert!(!modal.is_modal_active());
    modal.show_modal(
        "Docs • GPUI Telegram Docs",
        440.0,
        240.0,
        gpui_telegram_desktop::components::ui::layers::LayerStackOption::CloseOther,
    );
    assert!(modal.is_modal_active());
    assert_eq!(modal.current_box().unwrap().title, "Docs • GPUI Telegram Docs");
    assert!(modal.dismiss_top().is_some());
    assert!(!modal.is_modal_active());
}

#[test]
fn test_cloud_workspace_configuration_removes_demo_clutter() {
    let server_url = "http://127.0.0.1:8080";
    let mut sidebar = ChatSidebar::new_demo();
    sidebar.configure_for_cloud_workspace(server_url);
    assert_eq!(sidebar.active_chat_id, None);
    assert!(sidebar.dialogs.is_empty());
    assert!(!sidebar.suggestions.is_visible);
    assert!(sidebar.stories.elements.is_empty());
    assert!(sidebar.top_peers.entries.is_empty());

    let mut stage = ConversationStage::new_demo();
    stage.configure_for_cloud_workspace(server_url, Some("cloud_user"));
    assert_eq!(stage.active_chat_id, None);
    assert!(stage.chat_subtitle.is_empty());
    assert!(stage.pinned_message.is_none());
    assert!(!stage.is_inspector_open);
    assert!(stage.messages.is_empty());
    assert!(stage.chat_histories.is_empty());

    let mut inspector = ContextInspector::new_demo();
    inspector.configure_for_cloud_workspace(server_url);
    assert!(!inspector.is_open);
    assert!(inspector.profile.name.is_empty());
    assert!(inspector.profile.username.is_empty());
    assert!(inspector.profile.phone.is_empty());
    assert!(inspector.members.members.is_empty());
    assert!(inspector.shared_media.files.is_empty());
    assert!(inspector.shared_media.links.is_empty());
}

// ============================================================================
// 6. Milestone 1: 4-Pane Telegram Desktop 1:1 Geometry Tests
// ============================================================================

#[test]
fn test_nav_rail_1to1_geometry_metrics() {
    assert_eq!(nav_rail::NAV_RAIL_WIDTH, 52.0);
    assert_eq!(nav_rail::USERPIC_DIAMETER, 36.0);
    assert_eq!(nav_rail::NAV_AVATAR_SIZE, 36.0);
    assert_eq!(nav_rail::ONLINE_DOT_DIAMETER, 9.0);
    assert_eq!(nav_rail::NAV_ONLINE_DOT_SIZE, 9.0);
    assert_eq!(nav_rail::BUTTON_SIZE, 40.0);
    assert_eq!(nav_rail::NAV_BUTTON_SIZE, 40.0);
    assert_eq!(nav_rail::NAV_DIVIDER_WIDTH, 32.0);
    assert_eq!(nav_rail::NAV_DIVIDER_HEIGHT, 1.0);

    let rail = NavRail::new();
    assert_eq!(rail.items.len(), 12);
    assert!(!rail.is_connected);
    assert_eq!(rail.account_initials, "TG");
}

#[test]
fn test_chat_sidebar_1to1_geometry_and_resize_bounds() {
    assert_eq!(chat_sidebar::DEFAULT_WIDTH, 288.0);
    assert_eq!(chat_sidebar::DEFAULT_SIDEBAR_WIDTH, 288.0);
    assert_eq!(chat_sidebar::MIN_WIDTH, 240.0);
    assert_eq!(chat_sidebar::MIN_SIDEBAR_WIDTH, 240.0);
    assert_eq!(chat_sidebar::MAX_WIDTH, 360.0);
    assert_eq!(chat_sidebar::MAX_SIDEBAR_WIDTH, 360.0);
    assert_eq!(chat_sidebar::ROW_HEIGHT, 62.0);
    assert_eq!(chat_sidebar::DIALOG_ROW_HEIGHT, 62.0);
    assert_eq!(chat_sidebar::AVATAR_SIZE, 46.0);
    assert_eq!(chat_sidebar::DIALOG_AVATAR_SIZE, 46.0);
    assert_eq!(chat_sidebar::UNREAD_HEIGHT, 19.0);
    assert_eq!(chat_sidebar::DIALOG_UNREAD_HEIGHT, 19.0);
    assert_eq!(chat_sidebar::DIALOG_UNREAD_PADDING, 5.0);
    assert_eq!(chat_sidebar::SEARCH_HEIGHT, 35.0);
    assert_eq!(chat_sidebar::SEARCH_BAR_HEIGHT, 35.0);
    assert_eq!(chat_sidebar::SEARCH_RADIUS, 18.0);
    assert_eq!(chat_sidebar::SEARCH_BAR_RADIUS, 18.0);
    assert_eq!(chat_sidebar::SEARCH_HEADER_HEIGHT, 48.0);
    assert_eq!(chat_sidebar::DIALOG_NAME_LEFT, 68.0);

    let mut sidebar = ChatSidebar::new_demo();
    assert_eq!(sidebar.width, 288.0);

    // Test resize clamp within [240, 360]
    sidebar.set_width(200.0);
    assert_eq!(sidebar.width, 240.0, "Width below minimum must clamp to 240px");

    sidebar.set_width(400.0);
    assert_eq!(sidebar.width, 360.0, "Width above maximum must clamp to 360px");

    sidebar.set_width(310.0);
    assert_eq!(sidebar.width, 310.0, "Width inside bounds must update directly");

    sidebar.reset_width();
    assert_eq!(sidebar.width, 288.0);
}

#[test]
fn test_context_inspector_1to1_geometry_and_header_height() {
    assert_eq!(context_inspector::DRAWER_WIDTH, 320.0);
    assert_eq!(context_inspector::CONTEXT_INSPECTOR_WIDTH, 320.0);
    assert_eq!(context_inspector::HEADER_HEIGHT, 54.0);
    assert_eq!(context_inspector::CONTEXT_INSPECTOR_HEADER_HEIGHT, 54.0);
    assert_eq!(context_inspector::AVATAR_SIZE, 80.0);
    assert_eq!(context_inspector::CONTEXT_INSPECTOR_AVATAR_SIZE, 80.0);

    let inspector = ContextInspector::new_demo();
    assert!(inspector.is_open);
    assert_eq!(inspector.active_tab, drawer::inspector_tabs::InspectorTab::Profile);
    assert_eq!(inspector.profile.avatar_initials, "AS");
}

#[test]
fn test_message_bubble_1to1_bounds_and_rounding() {
    use conversation_stage::{
        compute_attachment, BubbleRounding, BUBBLE_RADIUS_LARGE, BUBBLE_RADIUS_SMALL,
        K_ATTACH_MESSAGE_TO_PREVIOUS_SECONDS_DELTA, MSG_MAX_WIDTH, MSG_MIN_WIDTH,
    };

    assert_eq!(MSG_MIN_WIDTH, 160.0);
    assert_eq!(MSG_MAX_WIDTH, 430.0);
    assert_eq!(BUBBLE_RADIUS_LARGE, 16.0);
    assert_eq!(BUBBLE_RADIUS_SMALL, 4.0);
    assert_eq!(K_ATTACH_MESSAGE_TO_PREVIOUS_SECONDS_DELTA, 900);

    // Isolated message rounding (outgoing)
    let isolated_out = BubbleRounding::compute(true, false, false);
    assert_eq!(isolated_out.top_left, 16.0);
    assert_eq!(isolated_out.bottom_left, 16.0);
    assert_eq!(isolated_out.top_right, 16.0);
    assert!(isolated_out.has_tail);

    // Isolated message rounding (incoming)
    let isolated_in = BubbleRounding::compute(false, false, false);
    assert_eq!(isolated_in.top_right, 16.0);
    assert_eq!(isolated_in.bottom_right, 16.0);
    assert_eq!(isolated_in.top_left, 16.0);
    assert!(isolated_in.has_tail);

    // Consecutive messages from same sender within 900s:
    // Middle attached message (attached to prev and next)
    let middle_out = BubbleRounding::compute(true, true, true);
    assert_eq!(middle_out.top_right, 4.0);
    assert_eq!(middle_out.bottom_right, 4.0);
    assert!(!middle_out.has_tail);

    // Terminal attached message (attached to prev, but not next -> has tail)
    let terminal_out = BubbleRounding::compute(true, true, false);
    assert_eq!(terminal_out.top_right, 4.0);
    assert!(terminal_out.has_tail);

    // Test compute_attachment with message list
    let msgs = vec![
        MessageItemModel {
            id: 1,
            sender_id: 10,
            sender_name: "Alice".to_string(),
            avatar_initials: "A".to_string(),
            avatar_color: 0x2AABEE,
            direction: MessageDirection::Incoming,
            timestamp: "10:00:00".to_string(),
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: vec![],
            content: MessageContent::Text {
                text: "First message".to_string(),
                is_edited: false,
            },
            bot_keyboard: None,
            is_pinned: false,
        },
        MessageItemModel {
            id: 2,
            sender_id: 10,
            sender_name: "Alice".to_string(),
            avatar_initials: "A".to_string(),
            avatar_color: 0x2AABEE,
            direction: MessageDirection::Incoming,
            timestamp: "10:05:00".to_string(), // +300s <= 900s
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: vec![],
            content: MessageContent::Text {
                text: "Second message".to_string(),
                is_edited: false,
            },
            bot_keyboard: None,
            is_pinned: false,
        },
        MessageItemModel {
            id: 3,
            sender_id: 10,
            sender_name: "Alice".to_string(),
            avatar_initials: "A".to_string(),
            avatar_color: 0x2AABEE,
            direction: MessageDirection::Incoming,
            timestamp: "10:30:00".to_string(), // +1500s > 900s
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: vec![],
            content: MessageContent::Text {
                text: "Third message (after 25m)".to_string(),
                is_edited: false,
            },
            bot_keyboard: None,
            is_pinned: false,
        },
    ];

    let (p0, n0) = compute_attachment(&msgs, 0);
    assert!(!p0, "First message should not attach to previous");
    assert!(n0, "First message should attach to second (within 300s)");

    let (p1, n1) = compute_attachment(&msgs, 1);
    assert!(p1, "Second message should attach to first");
    assert!(!n1, "Second message should NOT attach to third (delta 1500s > 900s)");

    let (p2, n2) = compute_attachment(&msgs, 2);
    assert!(!p2, "Third message should not attach to second (delta > 900s)");
    assert!(!n2, "Third message should not attach to next (last message)");
}

#[test]
fn test_canonical_composer_dynamic_auto_expansion() {
    assert_eq!(canonical_composer::MIN_COMPOSER_HEIGHT, 44.0);
    assert_eq!(canonical_composer::MAX_COMPOSER_HEIGHT, 192.0);
    assert_eq!(canonical_composer::LINE_HEIGHT, 20.0);
    assert_eq!(canonical_composer::ACTION_BTN_WIDTH, 44.0);
    assert_eq!(canonical_composer::ACTION_BTN_HEIGHT, 46.0);

    let mut composer = CanonicalComposer::new();

    // Empty text -> 1 line -> 44px
    assert_eq!(composer.calculate_line_count(), 1);
    assert_eq!(f32::from(composer.calculate_content_height()), 44.0);
    assert!(!composer.is_overflow_scroll_active());

    // Single line text -> 44px
    composer.set_text("Hello world");
    assert_eq!(composer.calculate_line_count(), 1);
    assert_eq!(f32::from(composer.calculate_content_height()), 44.0);

    // 2 explicit lines -> 44 + 1 * 20 = 64px
    composer.set_text("Line 1\nLine 2");
    assert_eq!(composer.calculate_line_count(), 2);
    assert_eq!(f32::from(composer.calculate_content_height()), 64.0);

    // 3 explicit lines -> 44 + 2 * 20 = 84px
    composer.set_text("Line 1\nLine 2\nLine 3");
    assert_eq!(composer.calculate_line_count(), 3);
    assert_eq!(f32::from(composer.calculate_content_height()), 84.0);

    // 8 explicit lines -> 44 + 7 * 20 = 184px (still <= 192px)
    let eight_lines = "1\n2\n3\n4\n5\n6\n7\n8";
    composer.set_text(eight_lines);
    assert_eq!(composer.calculate_line_count(), 8);
    assert_eq!(f32::from(composer.calculate_content_height()), 184.0);
    assert!(!composer.is_overflow_scroll_active());

    // 12 lines -> clamped to max 192px, overflow scroll active
    let twelve_lines = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
    composer.set_text(twelve_lines);
    assert_eq!(composer.calculate_line_count(), 12);
    assert_eq!(f32::from(composer.calculate_content_height()), 192.0);
    assert!(composer.is_overflow_scroll_active());
}

#[test]
fn test_theme_palette_1to1_token_parity() {
    use gpui_telegram_desktop::theme::palette::ThemePalette;

    // Day Blue
    assert_eq!(ThemePalette::DAY_BLUE_WINDOW_BG, 0xFFFFFF);
    assert_eq!(ThemePalette::DAY_BLUE_ACTIVE, 0x40A7E3);
    assert_eq!(ThemePalette::DAY_BLUE_MSG_OUT_BG, 0xDEF1FD);
    assert_eq!(ThemePalette::DAY_BLUE_MSG_IN_BG, 0xFFFFFF);

    // Night
    assert_eq!(ThemePalette::NIGHT_WINDOW_BG, 0x17212B);
    assert_eq!(ThemePalette::NIGHT_ACTIVE, 0x40A7E3);
    assert_eq!(ThemePalette::NIGHT_ACCENT, 0x5288C1);
    assert_eq!(ThemePalette::NIGHT_MSG_OUT_BG, 0x2B5278);
    assert_eq!(ThemePalette::NIGHT_MSG_IN_BG, 0x182533);
}

// ============================================================================
// Milestone 2 Feature Tests (F7, F8, F9, F10)
// ============================================================================

#[test]
fn test_ime_fsm_three_state_transitions() {
    let mut ime = ImeSafetyMachine::new();
    assert_eq!(ime.state, ImeState::Idle);
    assert!(!ime.just_committed);

    // 1. Start composition
    ime.on_ime_start();
    assert!(ime.is_composing());
    assert!(!ime.just_committed);

    // 2. Update preedit
    ime.on_ime_update("ceshi", 5);
    assert_eq!(ime.preedit_text(), Some("ceshi"));

    // 3. Commit candidate
    ime.on_ime_commit("测试");
    assert_eq!(ime.text(), "测试");
    assert!(ime.just_committed, "Commit fence must be raised upon commit");
    assert!(matches!(ime.state, ImeState::Committed { .. }));

    // 4. Trailing Return keydown must be absorbed by commit fence
    let action = ime.handle_enter_key(false);
    assert_eq!(action, KeyAction::ImeCandidateChosen, "Trailing Enter must be absorbed");
    assert!(!ime.just_committed, "Fence must reset after absorption");
    assert_eq!(ime.state, ImeState::Idle, "State must transition to Idle");

    // 5. Subsequent Enter keydown actually sends the message
    let action2 = ime.handle_enter_key(false);
    assert_eq!(action2, KeyAction::SendMessage, "Second Enter sends message");
}

#[test]
fn test_ime_direct_enter_selection_without_pre_commit() {
    let mut ime = ImeSafetyMachine::new();
    ime.on_ime_start();
    ime.on_ime_update("nihao", 5);

    // User hits Enter directly to select raw text
    let action = ime.handle_enter_key(false);
    assert_eq!(action, KeyAction::ImeCandidateChosen);
    assert_eq!(ime.text(), "nihao");

    // Trailing Cocoa Return event in same tick
    let trailing_action = ime.handle_enter_key(false);
    assert_eq!(trailing_action, KeyAction::ImeCandidateChosen);

    // Next deliberate Enter sends
    let send_action = ime.handle_enter_key(false);
    assert_eq!(send_action, KeyAction::SendMessage);
}

#[test]
fn test_shift_enter_inserts_newline_and_never_sends() {
    let mut ime = ImeSafetyMachine::new();
    ime.insert_text("Line 1");

    let action = ime.handle_enter_key(true);
    assert_eq!(action, KeyAction::InsertNewline);
    assert_eq!(ime.text(), "Line 1\n");

    ime.insert_text("Line 2");
    let send_action = ime.handle_enter_key(false);
    assert_eq!(send_action, KeyAction::SendMessage);
    assert_eq!(ime.text(), "Line 1\nLine 2");
}

#[test]
fn test_post_commit_typing_clears_fence() {
    let mut ime = ImeSafetyMachine::new();
    ime.on_ime_commit("你好");
    assert!(ime.just_committed);

    // User types exclamation mark outside IME
    ime.insert_text("!");
    assert!(!ime.just_committed);
    assert_eq!(ime.state, ImeState::Idle);

    // Enter now sends "你好!"
    let action = ime.handle_enter_key(false);
    assert_eq!(action, KeyAction::SendMessage);
}

#[test]
fn test_stage_scroll_handle_initialized_and_accessible() {
    let stage = ConversationStage::new_demo();
    assert_eq!(stage.scroll_handle.offset(), Point::default());
    assert!(stage.is_at_bottom());
}

#[test]
fn test_row_height_estimator_covers_all_11_canonical_cards() {
    let stage = ConversationStage::new_demo();
    assert!(stage.messages.len() >= 10);
    for msg in stage.messages.iter() {
        let h = RowHeightCache::estimate_row_height(msg, false);
        assert!(h >= 24.0, "Card type {:?} estimated height too small: {}", msg.content, h);
        assert!(h <= 600.0, "Card type {:?} estimated height too large: {}", msg.content, h);
    }
}

#[test]
fn test_row_height_cache_prefix_sums_and_binary_search() {
    let mut cache = RowHeightCache::new();
    let stage = ConversationStage::new_demo();
    cache.rebuild_prefix_sums(&stage.messages);

    assert_eq!(cache.prefix_sums.len(), stage.messages.len());
    assert!(cache.total_height > 0.0);

    // Range search at top
    let (s, e) = cache.visible_range(0.0, 400.0, 100.0, stage.messages.len());
    assert_eq!(s, 0);
    assert!(e > 0 && e <= stage.messages.len());

    // Range search at bottom
    let (s_bot, e_bot) = cache.visible_range(cache.total_height - 300.0, 400.0, 100.0, stage.messages.len());
    assert!(s_bot > 0);
    assert_eq!(e_bot, stage.messages.len() - 1);
}

#[test]
fn test_virtualized_windowing_slice_bound_with_1000_messages() {
    let mut stage = ConversationStage::new_demo();
    stage.populate_stress_messages(1000);
    assert_eq!(stage.message_count(), 1000);

    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    let (start_idx, end_idx) = stage.row_height_cache.visible_range(12000.0, 800.0, 300.0, 1000);

    // Window should only encompass a small fraction (~15-25 items)
    let visible_count = end_idx - start_idx + 1;
    assert!(visible_count < 40, "Windowed slice too large: {}", visible_count);

    let top_spacer = stage.row_height_cache.prefix_sums[start_idx - 1];
    let bottom_spacer = stage.row_height_cache.total_height - stage.row_height_cache.prefix_sums[end_idx];
    assert!(top_spacer > 0.0);
    assert!(bottom_spacer > 0.0);
}

#[test]
fn test_scroll_anchoring_dispatch_and_history_isolation() {
    let mut stage = ConversationStage::new_demo();

    // 1. Outgoing message forces scroll_to_bottom
    stage.add_outgoing_text_message("Hello from user");
    assert!(stage.is_at_bottom());

    // 2. Simulate user scrolled up to read history
    stage.set_simulated_scroll_offset(Point::new(px(0.0), px(-200.0)), Size::new(px(0.0), px(1500.0)));
    assert!(!stage.is_at_bottom());

    // 3. Incoming message while reading history does NOT steal focus
    stage.add_incoming_message(1001, "Background update");
    assert!(!stage.is_at_bottom(), "Incoming message erroneously forced scroll to bottom!");
}

#[test]
fn test_chat_switching_isolation() {
    let mut stage = ConversationStage::new_demo();
    assert_eq!(stage.active_chat_id, Some(1));
    let initial_count = stage.messages.len();
    assert!(initial_count > 0, "Chat 1 should have initial messages");
    assert!(stage.pinned_message.is_some(), "Chat 1 should have pinned message");

    // Switch to Chat 99 (unvisited peer)
    stage.set_chat(99, "New Peer", "offline", "NP", 0x1ABC9C);
    assert_eq!(stage.active_chat_id, Some(99));
    assert_eq!(stage.messages.len(), 0, "Chat 99 history must be isolated and empty");
    assert!(stage.pinned_message.is_none(), "Chat 99 pinned message must be None (no bleeding)");

    // Add a message to Chat 99
    stage.add_message(MessageItemModel {
        id: 9901,
        sender_id: 99,
        sender_name: "New Peer".to_string(),
        avatar_initials: "NP".to_string(),
        avatar_color: 0x1ABC9C,
        direction: MessageDirection::Incoming,
        timestamp: "12:00".to_string(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: vec![],
        content: MessageContent::Text {
            text: "Hello from 99".to_string(),
            is_edited: false,
        },
        bot_keyboard: None,
        is_pinned: false,
    });
    assert_eq!(stage.messages.len(), 1);

    // Switch to Chat 2
    stage.set_chat(2, "Rust Core Devs", "12 members", "RC", 0x9B59B6);
    assert_eq!(stage.active_chat_id, Some(2));
    assert_ne!(stage.messages.len(), 1);
    assert!(stage.messages.iter().all(|m| m.id != 9901));

    // Switch back to Chat 99
    stage.set_chat(99, "New Peer", "offline", "NP", 0x1ABC9C);
    assert_eq!(stage.messages.len(), 1);
    assert_eq!(stage.messages[0].id, 9901);

    // Switch back to Chat 1
    stage.set_chat(1, "Alice Smith", "online", "AS", 0x2AABEE);
    assert_eq!(stage.messages.len(), initial_count);
    assert!(stage.pinned_message.is_some());
}

#[test]
fn test_chat_switching_preserves_independent_scroll_positions() {
    let mut stage = ConversationStage::new_demo();

    // Chat 1: Scrolled to top
    stage.set_simulated_scroll_offset(Point::new(px(0.0), px(0.0)), Size::new(px(0.0), px(1000.0)));

    // Switch to Chat 2
    stage.set_chat(2, "Rust Core Devs", "online", "RC", 0x9B59B6);
    // Chat 2 defaults to bottom
    assert!(stage.is_at_bottom());

    // Switch back to Chat 1 -> should retain offset
    stage.set_chat(1, "Alice Smith", "online", "AS", 0x2AABEE);
    assert_eq!(stage.scroll_handle.offset().y, px(0.0));
}

#[test]
fn test_no_auto_pin_on_text_click() {
    let mut stage = ConversationStage::new_demo();
    // When a text message is added, verify it starts without auto-pinning or auto-reactions
    stage.add_outgoing_text_message("Normal text without auto pin");
    let text_msg = stage.messages.last().unwrap();
    assert!(!text_msg.is_pinned);
    assert!(text_msg.reactions.is_empty());
    assert_ne!(stage.pinned_message.as_deref(), Some("Normal text without auto pin"));
}

#[test]
fn test_markdown_rich_text_parsing() {
    let input = "Hello **bold** and *italic* and _italic2_ and __under__ and ~~strike~~ and `code block` and ||secret|| and https://telegram.org!";
    let spans = parse_markdown_spans(input);

    assert!(spans.iter().any(|s| s.text == "bold" && s.kind == RichSpanKind::Bold));
    assert!(spans.iter().any(|s| s.text == "italic" && s.kind == RichSpanKind::Italic));
    assert!(spans.iter().any(|s| s.text == "italic2" && s.kind == RichSpanKind::Italic));
    assert!(spans.iter().any(|s| s.text == "under" && s.kind == RichSpanKind::Underline));
    assert!(spans.iter().any(|s| s.text == "strike" && s.kind == RichSpanKind::Strikethrough));
    assert!(spans.iter().any(|s| s.text == "code block" && s.kind == RichSpanKind::Code));
    assert!(spans.iter().any(|s| s.text == "secret" && s.kind == RichSpanKind::Spoiler));
    assert!(spans.iter().any(|s| s.text == "https://telegram.org" && matches!(s.kind, RichSpanKind::Link(_))));
}

#[test]
fn test_context_menu_operations() {
    let mut stage = ConversationStage::new_demo();
    let text_msg_id = 101;

    // 1. Copy
    let text = stage.copy_message_text(text_msg_id);
    assert!(text.is_some());
    assert!(text.unwrap().contains("canonical"));

    // 2. Pin / Unpin
    assert!(!stage.messages.iter().find(|m| m.id == text_msg_id).unwrap().is_pinned);
    stage.toggle_pin_message(text_msg_id);
    assert!(stage.messages.iter().find(|m| m.id == text_msg_id).unwrap().is_pinned);
    assert!(stage.pinned_message.is_some());
    stage.toggle_pin_message(text_msg_id);
    assert!(!stage.messages.iter().find(|m| m.id == text_msg_id).unwrap().is_pinned);

    // 3. Delete
    let before_count = stage.messages.len();
    stage.delete_message(text_msg_id);
    assert_eq!(stage.messages.len(), before_count - 1);
    assert!(stage.messages.iter().all(|m| m.id != text_msg_id));

    // 4. Verify context menu state structure supports 5 actions
    let ctx = MessageContextMenuState {
        message_id: 102,
        position: Point::new(px(100.0), px(200.0)),
        is_outgoing: false,
        is_pinned: false,
        text_snippet: Some("Snippet".to_string()),
    };
    stage.active_context_menu = Some(ctx.clone());
    assert_eq!(stage.active_context_menu, Some(ctx));
}

// ============================================================================
// 7. Adversarial Hardening & Stress Testing (Milestone 2 Challenge Suite)
// ============================================================================

#[test]
fn test_adversarial_ime_rapid_candidate_selection_bursts() {
    let mut ime = composer::ImeSafetyMachine::new();
    let candidates = [
        ("nihao", "你好"),
        ("shijie", "世界"),
        ("kuaijie", "快捷"),
        ("shuru", "输入"),
        ("zhongwen", "中文"),
        ("gpui", "GPUI"),
        ("telegram", "TG"),
    ];

    // Rapid burst of 70 candidate selections
    for i in 0..70 {
        let (preedit, candidate) = candidates[i % candidates.len()];
        ime.on_ime_start();
        assert!(ime.is_composing());
        assert!(!ime.just_committed);

        ime.on_ime_update(preedit, preedit.len());
        assert_eq!(ime.preedit_text(), Some(preedit));

        // Candidate selected by user (Cocoa commits candidate)
        ime.on_ime_commit(candidate);
        assert!(ime.just_committed, "Commit fence MUST be active after candidate selection");
        assert!(matches!(ime.state, composer::ImeState::Committed { .. }));

        // Cocoa fires immediate trailing Return keydown
        let trailing_return_action = ime.handle_enter_key(false);
        assert_eq!(
            trailing_return_action,
            composer::KeyAction::ImeCandidateChosen,
            "Immediate Return keydown following IME candidate commit MUST be trapped by fence and never dispatch SendMessage!"
        );
        assert!(!ime.just_committed, "Fence must be consumed by the trailing Return keydown");
        assert_eq!(ime.state, composer::ImeState::Idle, "IME state must return to Idle after fence consumption");
    }

    assert!(!ime.text().is_empty());
    // Subsequent intentional Return keydown sends the complete message
    let intentional_send = ime.handle_enter_key(false);
    assert_eq!(intentional_send, composer::KeyAction::SendMessage);
}

#[test]
fn test_adversarial_ime_cancel_and_replacement_edge_cases() {
    let mut ime = composer::ImeSafetyMachine::new();

    // 1. Composition started and cancelled on empty buffer
    ime.on_ime_start();
    ime.on_ime_update("ceshi", 5);
    ime.on_ime_cancel();
    assert!(!ime.just_committed);
    assert_eq!(ime.state, composer::ImeState::Idle);
    // Enter on empty buffer produces None
    assert_eq!(ime.handle_enter_key(false), composer::KeyAction::None);

    // 2. Buffer has text, composition started and cancelled
    ime.insert_text("Existing message text");
    ime.on_ime_start();
    ime.on_ime_update("abc", 3);
    ime.on_ime_cancel();
    assert!(!ime.just_committed);
    assert_eq!(ime.state, composer::ImeState::Idle);
    // Enter on populated buffer after cancel produces SendMessage
    assert_eq!(ime.handle_enter_key(false), composer::KeyAction::SendMessage);
    ime.clear();

    // 3. Marked text replaced repeatedly without committing
    ime.on_ime_start();
    ime.on_ime_update("pre1", 4);
    ime.on_ime_update("pre2_replacement", 16);
    ime.on_ime_update("pre3_final", 10);
    // Commit the final replacement
    ime.on_ime_commit("最终文本");
    assert!(ime.just_committed);
    assert_eq!(ime.handle_enter_key(false), composer::KeyAction::ImeCandidateChosen);
    assert_eq!(ime.text(), "最终文本");
}

#[test]
fn test_adversarial_ime_shift_enter_multiline_and_unicode_boundary() {
    let mut composer = CanonicalComposer::new();

    // 1. Stress Shift+Enter multiline insertions (50 newlines)
    for _ in 0..50 {
        let action = composer.handle_enter(true);
        assert_eq!(action, composer::KeyAction::InsertNewline, "Shift+Enter must ALWAYS insert newline and never send");
    }
    assert_eq!(composer.text().matches('\n').count(), 50);
    assert_eq!(composer.calculate_line_count(), 51);
    assert_eq!(composer.calculate_content_height(), px(canonical_composer::MAX_COMPOSER_HEIGHT));
    assert!(composer.is_overflow_scroll_active());

    composer.clear();

    // 2. Insert mixed Unicode CJK + Emoji + RTL + multi-byte characters
    let complex_unicode = "你好世界！Hello 🦀 Rocket 🚀 Fire 🔥 Arabic: مرحبا بالعالم\nNext line: 🇯🇵 🇨🇳 🇺🇸";
    composer.ime_machine.insert_text(complex_unicode);
    assert_eq!(composer.text(), complex_unicode);

    // 3. Stress test backspace across multi-byte UTF-8 boundaries
    // Ensure backspace does not slice mid-character and panic
    for _ in 0..complex_unicode.chars().count() {
        composer.ime_machine.backspace();
    }
    assert_eq!(composer.text(), "");
    assert_eq!(composer.ime_machine.cursor_index, 0);

    // 4. Typing immediately clears commit fence
    composer.ime_machine.on_ime_commit("中文候选");
    assert!(composer.ime_machine.just_committed);
    composer.ime_machine.insert_text(" "); // normal typing resumes
    assert!(!composer.ime_machine.just_committed, "Normal character typing must clear commit fence");
    assert_eq!(composer.ime_machine.state, composer::ImeState::Idle);
    // Next Return sends message without absorption
    assert_eq!(composer.handle_enter(false), composer::KeyAction::SendMessage);
}

#[test]
fn test_adversarial_markdown_parser_fuzz_and_edge_cases() {
    // 1. Zero-length and extreme whitespace inputs
    assert!(parse_markdown_spans("").is_empty());
    let ws_spans = parse_markdown_spans("   \t   \r\n   ");
    assert_eq!(ws_spans.len(), 1);
    assert_eq!(ws_spans[0].kind, RichSpanKind::Plain);

    // 2. Unclosed delimiters: MUST NOT panic, loop indefinitely, or crash
    let unclosed_inputs = [
        "**unclosed bold text without closing",
        "__unclosed underline text",
        "~~unclosed strikethrough text",
        "||unclosed spoiler text",
        "*unclosed italic text",
        "_unclosed italic text",
        "`unclosed code block text",
        "**",
        "__",
        "~~",
        "||",
        "*",
        "_",
        "`",
        "****",
        "______",
        "||||||",
        "````",
    ];
    for input in &unclosed_inputs {
        let spans = parse_markdown_spans(input);
        assert!(!spans.is_empty(), "Failed to parse spans for: {}", input);
    }

    // 3. Nested formatting: **_bold italic_**
    let nested = "**_bold italic_**";
    let nested_spans = parse_markdown_spans(nested);
    assert_eq!(nested_spans.len(), 1);
    assert_eq!(nested_spans[0].text, "_bold italic_");
    assert_eq!(nested_spans[0].kind, RichSpanKind::Bold);

    // 4. Multiple adjacent and interleaved formatting spans
    let mixed = "**Bold**_Italic_`Code`||Spoiler||~~Strike~~__Under__";
    let mixed_spans = parse_markdown_spans(mixed);
    assert_eq!(mixed_spans.len(), 6);
    assert_eq!(mixed_spans[0].kind, RichSpanKind::Bold);
    assert_eq!(mixed_spans[1].kind, RichSpanKind::Italic);
    assert_eq!(mixed_spans[2].kind, RichSpanKind::Code);
    assert_eq!(mixed_spans[3].kind, RichSpanKind::Spoiler);
    assert_eq!(mixed_spans[4].kind, RichSpanKind::Strikethrough);
    assert_eq!(mixed_spans[5].kind, RichSpanKind::Underline);

    // 5. Malicious & edge URLs:
    let urls = [
        "https://",
        "http://",
        "https://example.com/path?foo=bar&baz=qux#heading",
        "Check this link: https://t.me/durov, and this https://rust-lang.org! Also (https://google.com).",
    ];
    for u in &urls {
        let spans = parse_markdown_spans(u);
        assert!(!spans.is_empty());
    }
    // Verify trailing punctuation was trimmed from URLs
    let punctuation_spans = parse_markdown_spans("Visit https://telegram.org, immediately!");
    let url_span = punctuation_spans.iter().find(|s| matches!(s.kind, RichSpanKind::Link(_))).unwrap();
    assert_eq!(url_span.text, "https://telegram.org");

    // 6. High-volume stress test (50,000 chars) ensuring linear O(N) parsing without quadratic explosion
    let large_stress = format!(
        "{} **bold** {} *italic* {} `code` {} ||spoiler||",
        "a".repeat(10000),
        "b".repeat(10000),
        "c".repeat(10000),
        "d".repeat(10000)
    );
    let start = std::time::Instant::now();
    let stress_spans = parse_markdown_spans(&large_stress);
    let elapsed = start.elapsed();
    assert!(elapsed.as_millis() < 200, "Markdown parsing took too long: {:?}", elapsed);
    assert!(stress_spans.len() >= 8);
}

#[test]
fn test_adversarial_context_menu_extreme_coordinates_and_actions() {
    let mut stage = ConversationStage::new_demo();

    // 1. Extreme coordinates stress testing
    let extreme_coords = [
        Point::new(px(0.0), px(0.0)),
        Point::new(px(-10000.0), px(-10000.0)),
        Point::new(px(99999.0), px(99999.0)),
        Point::new(px(f32::MIN_POSITIVE), px(1e6)),
    ];
    for coord in extreme_coords {
        stage.active_context_menu = Some(MessageContextMenuState {
            message_id: 101,
            position: coord,
            is_outgoing: false,
            is_pinned: false,
            text_snippet: Some("Extreme coord snippet".to_string()),
        });
        assert!(stage.active_context_menu.is_some());
        assert_eq!(stage.active_context_menu.as_ref().unwrap().position, coord);
    }
    stage.active_context_menu = None;

    // 2. Action handlers on varied message types
    // Text message
    stage.add_outgoing_text_message("Action test text message");
    let text_id = stage.messages.last().unwrap().id;
    assert_eq!(stage.copy_message_text(text_id), Some("Action test text message".to_string()));
    stage.toggle_pin_message(text_id);
    assert!(stage.messages.iter().find(|m| m.id == text_id).unwrap().is_pinned);
    assert_eq!(stage.pinned_message.as_deref(), Some("Action test text message"));
    stage.delete_message(text_id);
    assert!(stage.messages.iter().all(|m| m.id != text_id));

    // Media message with caption
    let media_with_caption = MessageItemModel {
        id: 8801,
        sender_id: 1,
        sender_name: "Me".to_string(),
        avatar_initials: "ME".to_string(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Outgoing,
        timestamp: "12:00".to_string(),
        delivery_status: DeliveryStatus::Sent,
        reply_to: None,
        reactions: vec![],
        content: MessageContent::Media {
            media_type: MediaType::Photo,
            url: "https://example.com/photo.jpg".to_string(),
            width: 800,
            height: 600,
            duration_secs: None,
            file_size_bytes: 102400,
            caption: Some("Scenic landscape photo caption".to_string()),
            is_streaming: false,
        },
        bot_keyboard: None,
        is_pinned: false,
    };
    stage.add_message(media_with_caption);
    assert_eq!(stage.copy_message_text(8801), Some("Scenic landscape photo caption".to_string()));
    stage.toggle_pin_message(8801);
    assert!(stage.messages.iter().find(|m| m.id == 8801).unwrap().is_pinned);
    stage.delete_message(8801);
    assert!(stage.messages.iter().all(|m| m.id != 8801));

    // Voice message
    let voice_msg = MessageItemModel {
        id: 8802,
        sender_id: 2,
        sender_name: "Alice".to_string(),
        avatar_initials: "AS".to_string(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Incoming,
        timestamp: "12:01".to_string(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: vec![],
        content: MessageContent::Voice {
            duration_secs: 15,
            waveform_5bit: vec![10, 20, 30],
            playback_position_secs: 0.0,
            is_playing: false,
            playback_speed: 1.0,
        },
        bot_keyboard: None,
        is_pinned: false,
    };
    stage.add_message(voice_msg);
    assert_eq!(stage.copy_message_text(8802), None);
    stage.toggle_pin_message(8802);
    assert!(stage.messages.iter().find(|m| m.id == 8802).unwrap().is_pinned);
    stage.delete_message(8802);
    assert!(stage.messages.iter().all(|m| m.id != 8802));

    // Poll message
    let poll_msg = MessageItemModel {
        id: 8803,
        sender_id: 1,
        sender_name: "Me".to_string(),
        avatar_initials: "ME".to_string(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Outgoing,
        timestamp: "12:02".to_string(),
        delivery_status: DeliveryStatus::Sent,
        reply_to: None,
        reactions: vec![],
        content: MessageContent::Poll {
            question: "Adversarial test question?".to_string(),
            poll_type: PollType::Regular,
            options: vec![],
            total_votes: 0,
            is_closed: false,
            selected_option_id: None,
        },
        bot_keyboard: None,
        is_pinned: false,
    };
    stage.add_message(poll_msg);
    assert_eq!(stage.copy_message_text(8803), None);
    stage.toggle_pin_message(8803);
    assert!(stage.messages.iter().find(|m| m.id == 8803).unwrap().is_pinned);
    stage.delete_message(8803);
    assert!(stage.messages.iter().all(|m| m.id != 8803));

    // 3. Message text bubble click isolation:
    stage.add_outgoing_text_message("Zero side effect text message");
    let clean_msg = stage.messages.last().unwrap();
    assert!(!clean_msg.is_pinned);
    assert!(clean_msg.reactions.is_empty());
}

#[test]
fn test_preferences_sidebar_collapsed_persistence_and_legacy_default() {
    use gpui_telegram_desktop::app::preferences::AppPreferences;

    struct ScopedTempDir(std::path::PathBuf);
    impl ScopedTempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "gpui_sidebar_pref_{}_{}_{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create temp test dir");
            Self(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for ScopedTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // 1. Default preferences expand the sidebar
    let defaults = AppPreferences::default();
    assert!(!defaults.appearance.sidebar_collapsed);

    // 2. Legacy JSON without `sidebar_collapsed` deserializes to false via `#[serde(default)]`
    let legacy_dir = ScopedTempDir::new("legacy");
    let mut legacy_prefs = AppPreferences::default();
    legacy_prefs.appearance.font_size = 16;
    let mut legacy_val = serde_json::to_value(&legacy_prefs).expect("serialize legacy prefs");
    if let Some(app_obj) = legacy_val
        .get_mut("appearance")
        .and_then(|v| v.as_object_mut())
    {
        app_obj.remove("sidebar_collapsed");
    }
    let legacy_json = serde_json::to_string_pretty(&legacy_val).expect("format legacy prefs JSON");
    std::fs::write(legacy_dir.path().join("preferences.json"), legacy_json)
        .expect("write legacy preferences.json");
    let loaded_legacy = AppPreferences::load_from_dir(legacy_dir.path())
        .expect("legacy preferences.json should load");
    assert!(!loaded_legacy.appearance.sidebar_collapsed);
    assert_eq!(loaded_legacy.appearance.font_size, 16);

    // 3. Round-trip saving `sidebar_collapsed = true`
    let roundtrip_dir = ScopedTempDir::new("roundtrip");
    let mut prefs = AppPreferences::default();
    prefs.appearance.sidebar_collapsed = true;
    prefs
        .save_to_dir(roundtrip_dir.path())
        .expect("save preferences.json");
    let reloaded = AppPreferences::load_from_dir(roundtrip_dir.path())
        .expect("saved preferences.json should reload");
    assert!(reloaded.appearance.sidebar_collapsed);
}

#[test]
fn test_unified_sidebar_collapsed_constants() {
    use gpui_telegram_desktop::views::{chat_sidebar, nav_rail};

    assert!((nav_rail::NAV_RAIL_WIDTH - 52.0).abs() < f32::EPSILON);
    assert!((chat_sidebar::DEFAULT_WIDTH - 288.0).abs() < f32::EPSILON);
    assert!((chat_sidebar::COLLAPSED_WIDTH - 64.0).abs() < f32::EPSILON);
}

#[test]
fn test_collapsed_sidebar_rail_avatars_match_expanded_dialog_avatars() {
    use gpui_telegram_desktop::components::grok_bot_avatar::GrokPersonaState;
    use gpui_telegram_desktop::views::chat_sidebar::{
        BotGrokAvatarVisual, ChatSidebar, DialogDeliveryStatus, DialogItemModel,
    };
    use gpui_telegram_desktop::views::sidebar::dialogs_row_view::{
        resolve_dialog_avatar_kind, DialogAvatarKind,
    };

    let mut sb = ChatSidebar::new();
    let palette = sb.palette;

    let make_dialog = |id: u64, name: &str, initials: &str, color: u32, folder: &str, pinned: bool| {
        DialogItemModel {
            id,
            peer_name: name.to_string(),
            avatar_initials: initials.to_string(),
            avatar_color: color,
            last_message: String::new(),
            timestamp: "09:15".to_string(),
            unread_count: 0,
            is_muted: false,
            is_pinned: pinned,
            is_online: true,
            typing_indicator: None,
            delivery_status: DialogDeliveryStatus::Read,
            folder: folder.to_string(),
        }
    };

    sb.dialogs = vec![
        make_dialog(1001, "Alice", "AL", 0x3B82F6, "All", false),
        make_dialog(601440062, "全球法布施", "全", 0x2AABEE, "Bots", true),
        make_dialog(1002, "工程讨论群", "工", 0x9B59B6, "Groups", false),
        make_dialog(1003, "已保存消息", "已", 0x2AABEE, "All", false),
        make_dialog(1004, "Shape Bot", "SB", 0x2AABEE, "Bots", false),
        make_dialog(1005, "Plain Bot", "PB", 0x2AABEE, "Bots", false),
        make_dialog(1006, "Classic Bot", "CB", 0x2AABEE, "Bots", false),
        make_dialog(1007, "Archived Chat", "AC", 0x64748B, "All", false),
    ];
    sb.archived_ids.push(1007);
    sb.saved_messages_id = Some(1003);
    sb.bot_ids.extend([601440062, 1004, 1005, 1006]);
    sb.bot_grok_avatars.insert(
        601440062,
        BotGrokAvatarVisual {
            agent_id: "fabushi_bot".to_string(),
            shape: "blob".to_string(),
            color: "pink".to_string(),
            execution_state: "working".to_string(),
        },
    );
    sb.bot_fabushi_visuals
        .insert(1004, ("hex".to_string(), 0x10B981));
    sb.bot_grok_avatars.insert(
        1006,
        BotGrokAvatarVisual {
            agent_id: "classic_bot".to_string(),
            shape: "classic".to_string(),
            color: "#F59E0B".to_string(),
            execution_state: "idle".to_string(),
        },
    );
    sb.bot_fabushi_visuals
        .insert(1006, ("classic".to_string(), 0xF59E0B));

    let collapsed = sb.collapsed_rail_dialogs();
    // Archived dialog excluded; pinned dialog (全球法布施) placed first
    let ids: Vec<u64> = collapsed.iter().map(|(d, _)| d.id).collect();
    assert_eq!(ids, vec![601440062, 1001, 1002, 1003, 1004, 1005, 1006]);

    // Every dialog in the collapsed rail must resolve to the exact same avatar kind as expanded mode
    for (dialog, collapsed_meta) in &collapsed {
        let expanded_meta = sb.dialog_meta(dialog);
        assert_eq!(
            resolve_dialog_avatar_kind(dialog, collapsed_meta, &palette),
            resolve_dialog_avatar_kind(dialog, &expanded_meta, &palette),
            "collapsed and expanded avatar mismatch for dialog {}",
            dialog.id
        );
    }

    // Verify specific avatar kinds match expected visual types (not regressed to plain initials)
    let kind_by_id = |id: u64| {
        let (d, m) = collapsed
            .iter()
            .find(|(d, _)| d.id == id)
            .expect("dialog present in collapsed rail");
        resolve_dialog_avatar_kind(d, m, &palette)
    };

    assert_eq!(
        kind_by_id(601440062),
        DialogAvatarKind::GrokBot {
            source_id: "fabushi_bot".to_string(),
            shape: "blob".to_string(),
            color: "pink".to_string(),
            state: GrokPersonaState::Working,
        }
    );
    assert_eq!(
        kind_by_id(1001),
        DialogAvatarKind::ClassicInitials {
            bg_hex: 0x3B82F6,
            initials: "AL".to_string(),
        }
    );
    assert_eq!(
        kind_by_id(1002),
        DialogAvatarKind::CompositeGroup { seed_id: 1002 }
    );
    assert_eq!(
        kind_by_id(1003),
        DialogAvatarKind::SavedMessages {
            bg_hex: palette.accent
        }
    );
    assert_eq!(
        kind_by_id(1004),
        DialogAvatarKind::FabushiShape {
            shape: "hex".to_string(),
            bg_hex: 0x10B981,
        }
    );
    assert!(matches!(
        kind_by_id(1005),
        DialogAvatarKind::GrokBot {
            state: GrokPersonaState::Idle,
            ..
        }
    ));
    assert_eq!(
        kind_by_id(1006),
        DialogAvatarKind::ClassicInitials {
            bg_hex: 0xF59E0B,
            initials: "CB".to_string(),
        }
    );
}

#[test]
fn test_conversation_stage_bot_thinking_avatar_activity_lifecycle_and_chat_switch_reset() {
    let mut stage = gpui_telegram_desktop::views::ConversationStage::new();
    stage.set_chat(777, "全球法布施", "bot", "法", 0x2AABEE);
    stage.is_bot_chat = true;
    assert_eq!(stage.bot_activity_state, "idle");

    stage.set_bot_activity("thinking", Some("正在分析意图...".to_string()));
    assert_eq!(stage.bot_activity_state, "thinking");
    assert_eq!(stage.bot_activity_detail.as_deref(), Some("正在分析意图..."));

    stage.set_bot_activity("tool", Some("fabushi_send_dharma".to_string()));
    assert_eq!(stage.bot_activity_state, "tool");
    assert_eq!(stage.bot_activity_detail.as_deref(), Some("fabushi_send_dharma"));

    stage.set_bot_activity("idle", None);
    assert_eq!(stage.bot_activity_state, "idle");
    assert!(stage.bot_activity_detail.is_none());

    // Switching to another chat resets bot activity & bot flag cleanly
    stage.set_bot_activity("thinking", Some("正在思考".to_string()));
    stage.set_chat(888, "Alice", "online", "AL", 0x5588EE);
    assert!(!stage.is_bot_chat);
    assert_eq!(stage.bot_activity_state, "idle");
    assert!(stage.bot_activity_detail.is_none());
}

// ============================================================================
// 6. M1 Inspector Isolation, Group Branching & Copy/Stage Toast Tests
// ============================================================================

#[test]
fn test_context_inspector_clear_peer_facts_and_close_isolation() {
    use gpui_telegram_desktop::views::drawer::inspector_tabs::{
        InspectorSubpage, InspectorTab, SharedMediaItem,
    };
    use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;

    let mut inspector = ContextInspector::new();
    inspector.open();
    inspector.active_tab = InspectorTab::Members;
    inspector.subpage = InspectorSubpage::ComputersList;
    inspector.profile.name = "全球法布施 Bot".to_string();
    inspector.profile.username = "@fabushi_bot".to_string();
    inspector.profile.phone = "+15551234567".to_string();
    inspector.profile.bio = "全球法布施与净心禅室 AI 助手".to_string();
    inspector.profile.avatar_path = Some("/tmp/fabushi_bot.png".to_string());
    inspector.profile.is_bot = true;
    inspector.profile.is_verified = true;
    inspector.profile.facts.push(("Engine".to_string(), "Fabushi".to_string()));
    inspector.media_items.push(SharedMediaItem {
        id: 1,
        url: "https://example.com/1.png".to_string(),
        local_path: None,
        caption: "cap".to_string(),
        is_video: false,
        duration_sec: 0,
    });
    inspector.copied_feedback = Some("已复制: @fabushi_bot".to_string());
    inspector.channel_boost_level = 5;
    inspector.channel_boosts_current = 12;
    inspector.channel_boosts_target = 50;
    inspector.pending_leave_chat = true;
    inspector.set_active_fabushi_bot(Some(FabushiBotSummaryDto {
        bot_user_id: 777001,
        agent_id: "fabushi".to_string(),
        username: "fabushi_bot".to_string(),
        display_name: "全球法布施 Bot".to_string(),
        ..Default::default()
    }));

    let prev_gen = inspector.copied_feedback_gen;
    inspector.clear_peer_facts();

    assert!(inspector.profile.bio.is_empty());
    assert!(inspector.profile.username.is_empty());
    assert!(inspector.profile.phone.is_empty());
    assert!(inspector.profile.avatar_path.is_none());
    assert!(!inspector.profile.is_verified);
    assert!(!inspector.profile.is_bot);
    assert!(inspector.profile.facts.is_empty());
    assert!(inspector.media_items.is_empty());
    assert!(inspector.copied_feedback.is_none());
    assert_eq!(inspector.copied_feedback_gen, prev_gen.wrapping_add(1));
    assert_eq!(inspector.subpage, InspectorSubpage::Main);
    assert_eq!(inspector.active_tab, InspectorTab::Profile);
    assert!(inspector.active_fabushi_bot.is_none());
    assert_eq!(inspector.channel_boost_level, 0);
    assert_eq!(inspector.channel_boosts_current, 0);
    assert_eq!(inspector.channel_boosts_target, 30);
    assert!(!inspector.pending_leave_chat);

    // close() and toggle_open() -> closed immediately clear copied_feedback & bump generation
    inspector.open();
    inspector.copied_feedback = Some("已复制: 123".to_string());
    let g1 = inspector.copied_feedback_gen;
    inspector.close();
    assert!(!inspector.is_open);
    assert!(inspector.copied_feedback.is_none());
    assert_eq!(inspector.copied_feedback_gen, g1.wrapping_add(1));

    inspector.open();
    inspector.copied_feedback = Some("已复制: 456".to_string());
    let g2 = inspector.copied_feedback_gen;
    inspector.toggle_open();
    assert!(!inspector.is_open);
    assert!(inspector.copied_feedback.is_none());
    assert_eq!(inspector.copied_feedback_gen, g2.wrapping_add(1));
}

#[test]
fn test_group_vs_bot_vs_user_profile_branching_and_no_duplicate_bio() {
    use gpui_telegram_client::net::http_client::ChatDetails;
    use gpui_telegram_desktop::theme::TdesktopThemePalette;
    use gpui_telegram_desktop::views::app_shell::shell_actions::build_chat_profile_facts;
    use gpui_telegram_desktop::views::drawer::inspector_panels::build_contact_profile_layout;
    use gpui_telegram_desktop::views::drawer::inspector_tabs::{
        MemberItem, MembersTabModel, ProfileTabModel,
    };

    let pal = TdesktopThemePalette::default();

    // 1. build_chat_profile_facts must NOT duplicate Bio/简介 into facts
    let group_details = ChatDetails {
        chat_id: 202,
        title: "AA 记账分账助手".to_string(),
        chat_type: 2,
        creator_id: 1,
        about: Some("群聊共享记账说明".to_string()),
        members_count: 5,
        is_forum: false,
        photo_file_id: None,
        pts: 0,
    };
    let facts_zh = build_chat_profile_facts(&group_details, "zh-CN");
    assert!(
        !facts_zh.iter().any(|(k, _)| k == "简介" || k.eq_ignore_ascii_case("Bio")),
        "build_chat_profile_facts must not emit duplicate Bio row: {facts_zh:?}"
    );
    assert!(facts_zh.iter().any(|(k, v)| k == "类型" && v == "超级群组"));
    assert!(facts_zh.iter().any(|(k, v)| k == "成员" && v == "5"));

    // 2. Group with empty bio: shows Group Info, Type, Members, Invite Link, Edit Group, Leave Group;
    //    NEVER shows phone, bio, Secret Chat, Edit Contact Info, or Block Contact.
    let make_base_profile = || ProfileTabModel {
        name: "AA 记账分账助手".to_string(),
        username: String::new(),
        phone: "+15559998888".to_string(), // even if somehow present, group layout must hide phone
        bio: String::new(),
        status: "2 位成员".to_string(),
        avatar_initials: "AA".to_string(),
        avatar_color: 0x2AABEE,
        avatar_path: None,
        facts: vec![
            ("类型".to_string(), "群组".to_string()),
            ("简介".to_string(), "旧简介".to_string()),
        ],
        is_notifications_enabled: true,
        is_bot: false,
        bot_id: 0,
        bot_persona_prompt: String::new(),
        bot_commands: Vec::new(),
        miniapp_title: None,
        miniapp_url: None,
        in_attach_menu: false,
        bot_mau_count: 0,
        bot_has_main_app: false,
        bot_privacy_policy_url: String::new(),
        bot_has_settings: false,
        bot_can_join_groups: false,
        is_viewer_owner: false,
        is_verified: false,
        is_blocked: false,
    };
    let group_profile = make_base_profile();
    let group_members = MembersTabModel {
        members: vec![
            MemberItem {
                id: 1,
                name: "Alice".to_string(),
                initials: "AL".to_string(),
                color: 0x2AABEE,
                role: "Owner".to_string(),
                is_online: true,
            },
            MemberItem {
                id: 2,
                name: "Bob".to_string(),
                initials: "BO".to_string(),
                color: 0x8E44AD,
                role: "Member".to_string(),
                is_online: false,
            },
        ],
        search_query: String::new(),
        invite_link: "https://t.me/+group_invite".to_string(),
        is_group_or_channel: true,
    };
    let group_layout = build_contact_profile_layout(&group_profile, &group_members, pal, "zh-CN");
    assert!(group_layout.is_group_or_channel);
    assert!(!group_layout.is_channel);
    assert_eq!(group_layout.info_section_title, "群组信息");
    assert_eq!(group_layout.privacy_section_title, "通知与管理");
    assert!(!group_layout.show_secret_chat);
    assert!(!group_layout.show_block_contact);
    assert!(group_layout.show_members_row);
    assert_eq!(group_layout.edit_row_label, "编辑群组信息");
    assert_eq!(group_layout.leave_row_label.as_deref(), Some("退出群组"));
    assert!(group_layout.info_rows.iter().all(|r| r.value != "+15559998888"));
    assert!(group_layout.info_rows.iter().all(|r| r.subtitle != "简介" && r.subtitle != "群组简介"));
    assert!(group_layout.info_rows.iter().any(|r| r.subtitle.contains("邀请链接") && r.value == "https://t.me/+group_invite"));
    assert!(group_layout.info_rows.iter().any(|r| r.subtitle == "成员" && r.value == "2"));

    // 3. Channel with non-empty bio renders Channel Info / 频道简介 once & Leave Channel
    let mut channel_profile = make_base_profile();
    channel_profile.bio = "Official channel updates".to_string();
    channel_profile.username = "@official_news".to_string();
    channel_profile.facts = vec![
        ("Type".to_string(), "Channel".to_string()),
        ("Bio".to_string(), "Official channel updates".to_string()),
    ];
    let chan_layout = build_contact_profile_layout(&channel_profile, &group_members, pal, "en-US");
    assert!(chan_layout.is_channel);
    assert_eq!(chan_layout.info_section_title, "Channel Info");
    assert_eq!(chan_layout.edit_row_label, "Edit Channel Info");
    assert_eq!(chan_layout.leave_row_label.as_deref(), Some("Leave Channel"));
    assert_eq!(
        chan_layout
            .info_rows
            .iter()
            .filter(|r| r.subtitle == "Channel Description" || r.subtitle == "Bio")
            .count(),
        1
    );
    assert!(chan_layout.info_rows.iter().any(|r| r.subtitle.contains("Public Link") && r.value == "@official_news"));

    // 4. 1v1 User layout keeps Contact Info, Secret Chat, Edit Contact Info, Block Contact (and no Leave Group)
    let mut user_profile = group_profile;
    user_profile.bio = "Rust developer".to_string();
    user_profile.username = "@alice".to_string();
    user_profile.facts.clear();
    let user_members = MembersTabModel {
        members: Vec::new(),
        search_query: String::new(),
        invite_link: String::new(),
        is_group_or_channel: false,
    };
    let user_layout = build_contact_profile_layout(&user_profile, &user_members, pal, "zh-CN");
    assert!(!user_layout.is_group_or_channel);
    assert_eq!(user_layout.info_section_title, "联系信息");
    assert_eq!(user_layout.privacy_section_title, "通知与隐私");
    assert!(user_layout.show_secret_chat);
    assert!(user_layout.show_block_contact);
    assert!(!user_layout.show_members_row);
    assert_eq!(user_layout.edit_row_label, "编辑联系人信息");
    assert!(user_layout.leave_row_label.is_none());
    assert!(user_layout.info_rows.iter().any(|r| r.value == "+15559998888"));
}

#[test]
fn test_stage_set_chat_clears_toast_banner_active_fabushi_bot_and_room_agent_states() {
    use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;

    let mut stage = ConversationStage::new();
    stage.set_chat(101, "Fabushi Bot", "bot", "FB", 0x8F59F9);
    stage.toast_banner = Some("Copied to clipboard".to_string());
    let gen_before = stage.toast_banner_gen;
    stage.set_active_fabushi_bot(Some(FabushiBotSummaryDto {
        bot_user_id: 777001,
        agent_id: "fabushi".to_string(),
        username: "fabushi_bot".to_string(),
        display_name: "Fabushi Bot".to_string(),
        ..Default::default()
    }));
    stage.on_agent_floor_changed(101, Some("Fabushi Bot".to_string()), 1, 1, 10_000);
    assert!(stage.active_room_agent_state(15_000).is_some());

    // Same-chat set_chat preserves active_fabushi_bot
    stage.set_chat(101, "Fabushi Bot", "bot", "FB", 0x8F59F9);
    assert!(stage.active_fabushi_bot.is_some());

    // Switching to another chat clears toast_banner (+ bumps gen), active_fabushi_bot, and room_agent_states
    stage.set_chat(202, "AA 记账分账助手", "3 members", "AA", 0x2AABEE);
    assert!(stage.toast_banner.is_none());
    assert_eq!(stage.toast_banner_gen, gen_before.wrapping_add(1));
    assert!(stage.active_fabushi_bot.is_none());
    assert!(!stage.is_bot_chat);
    assert!(stage.room_agent_states.is_empty());
    assert!(stage.active_room_agent_state(15_000).is_none());

    // reset() also clears toast_banner, active_fabushi_bot, and room_agent_states
    stage.toast_banner = Some("Toast".to_string());
    stage.set_active_fabushi_bot(Some(FabushiBotSummaryDto {
        bot_user_id: 777002,
        ..Default::default()
    }));
    stage.on_agent_floor_changed(202, Some("Helper".to_string()), 0, 1, 20_000);
    stage.reset();
    assert!(stage.toast_banner.is_none());
    assert!(stage.active_fabushi_bot.is_none());
    assert!(stage.room_agent_states.is_empty());
}

mod m1_headless_gpui {
    use futures::FutureExt;
    use gpui::{App, AppContext, Application, AsyncApp};
    use gpui_telegram_desktop::theme::{TelegramAssetSource, TdesktopThemePalette};
    use gpui_telegram_desktop::views::boxes::peers::BotProfileBox;
    use gpui_telegram_desktop::views::context_inspector::ContextInspector;
    use gpui_telegram_desktop::views::conversation_stage::ConversationStage;
    use gpui_telegram_desktop::views::drawer::inspector_panels::render_contact_profile_panel;
    use gpui_telegram_proto::miniapp_contracts::{BotPublicProfileDto, BotRegistryEntry};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    const CHILD_ENV: &str = "GPUI_DESKTOP_VIEWS_HEADLESS_CHILD";
    const CHILD_TARGET_ENV: &str = "GPUI_DESKTOP_VIEWS_HEADLESS_TARGET";
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TempConfigDirGuard {
        path: PathBuf,
    }

    impl TempConfigDirGuard {
        fn new(prefix: &str) -> Self {
            let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "gpui_tg_desktop_views_{prefix}_{}_{}",
                std::process::id(),
                seq
            ));
            std::fs::create_dir_all(&dir).expect("create isolated temp config dir");
            Self { path: dir }
        }
    }

    impl Drop for TempConfigDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
        if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        }
    }

    fn dispatch_headless_target(target: &str) -> ! {
        let async_fn: fn(
            AsyncApp,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>> = match target {
            "test_headless_gpui_copied_feedback_auto_dismiss_generation_fence_and_switch_clear" => {
                |app| Box::pin(body_copied_feedback_timers(app))
            }
            "test_headless_gpui_stage_toast_banner_auto_dismiss_generation_fence_and_set_chat_clear" => {
                |app| Box::pin(body_stage_toast_timers(app))
            }
            "test_headless_gpui_render_contact_profile_panel_branches" => {
                |app| Box::pin(body_render_contact_profile_panel_branches(app))
            }
            "test_headless_gpui_inspector_pending_leave_chat_immediate_modal_no_double_trigger" => {
                |app| Box::pin(body_inspector_pending_leave_chat_immediate_modal(app))
            }
            other => {
                eprintln!("unknown headless gpui test target: {other}");
                std::process::exit(101);
            }
        };
        let target_owned = target.to_string();
        let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Application::headless()
                .with_assets(TelegramAssetSource::new())
                .run(move |cx: &mut App| {
                    let t = target_owned.clone();
                    cx.spawn(async move |app: &mut AsyncApp| {
                        let app_clone = app.clone();
                        let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                            .catch_unwind()
                            .await;
                        match res {
                            Ok(()) => std::process::exit(0),
                            Err(payload) => {
                                let msg = panic_payload_to_string(payload);
                                eprintln!("[headless async child panic] {t}: {msg}");
                                std::process::exit(101);
                            }
                        }
                    })
                    .detach();
                });
        }));
        if let Err(payload) = outer {
            let msg = panic_payload_to_string(payload);
            eprintln!("[headless outer panic] {target}: {msg}");
            std::process::exit(101);
        }
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    #[used]
    #[unsafe(link_section = "__DATA,__mod_init_func")]
    static MACOS_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = macos_mod_init_headless_entry;

    #[cfg(target_os = "macos")]
    extern "C" fn macos_mod_init_headless_entry() {
        if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
            if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
                dispatch_headless_target(&target);
            }
        }
    }

    fn run_in_headless_subprocess(test_fn_name: &str) {
        if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
            dispatch_headless_target(test_fn_name);
        }
        let cfg_guard = TempConfigDirGuard::new(test_fn_name);
        let exe = std::env::current_exe().expect("current_exe");
        let output = std::process::Command::new(exe)
            .env(CHILD_ENV, "1")
            .env(CHILD_TARGET_ENV, test_fn_name)
            .env("GPUI_TELEGRAM_CONFIG_DIR", &cfg_guard.path)
            .env("GPUI_OFFLINE_TEST", "1")
            .arg("--exact")
            .arg(test_fn_name)
            .arg("--nocapture")
            .output()
            .expect("spawn headless gpui child");

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "headless child test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
                output.status.code()
            );
        }
    }

    async fn body_copied_feedback_timers(mut app: AsyncApp) {
        let insp = app
            .new(|_| ContextInspector::new())
            .expect("create ContextInspector entity");

        // 1. Auto-dismiss & generation fence on ContextInspector
        insp.update(&mut app, |i, cx| {
            i.open();
            i.show_copied_feedback_with_duration("已复制: first", Duration::from_millis(50), cx);
            assert_eq!(i.copied_feedback.as_deref(), Some("已复制: first"));
            assert_eq!(i.copied_feedback_gen, 1);
        })
        .unwrap();

        app.background_executor().timer(Duration::from_millis(15)).await;

        insp.update(&mut app, |i, cx| {
            i.show_copied_feedback_with_duration("已复制: second", Duration::from_millis(120), cx);
            assert_eq!(i.copied_feedback.as_deref(), Some("已复制: second"));
            assert_eq!(i.copied_feedback_gen, 2);
        })
        .unwrap();

        // Past first 50ms timer, before second 120ms timer: generation fence keeps "second"
        app.background_executor().timer(Duration::from_millis(55)).await;
        insp.read_with(&app, |i, _| {
            assert_eq!(i.copied_feedback.as_deref(), Some("已复制: second"));
        })
        .unwrap();

        // Past second timer: auto-dismissed
        app.background_executor().timer(Duration::from_millis(95)).await;
        insp.read_with(&app, |i, _| {
            assert!(i.copied_feedback.is_none());
        })
        .unwrap();

        // 2. Chat switch (clear_peer_facts) & close() clear immediately and fence pending timer
        insp.update(&mut app, |i, cx| {
            i.show_copied_feedback_with_duration("已复制: switch", Duration::from_millis(60), cx);
            assert!(i.copied_feedback.is_some());
            i.clear_peer_facts();
            assert!(i.copied_feedback.is_none());
            i.show_copied_feedback_with_duration("已复制: close", Duration::from_millis(60), cx);
            assert!(i.copied_feedback.is_some());
            i.close();
            assert!(i.copied_feedback.is_none());
        })
        .unwrap();

        app.background_executor().timer(Duration::from_millis(80)).await;
        insp.read_with(&app, |i, _| {
            assert!(i.copied_feedback.is_none());
        })
        .unwrap();

        // 3. BotProfileBox generation-fenced auto-dismiss and close() clear
        let profile = BotPublicProfileDto {
            bot: BotRegistryEntry {
                bot_user_id: 600_001,
                username: "grok_bot".to_string(),
                display_name: "Grok Bot".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let box_ent = app
            .new(|_| BotProfileBox::new(profile, TdesktopThemePalette::default(), "zh-CN"))
            .expect("create BotProfileBox entity");

        box_ent
            .update(&mut app, |b, cx| {
                b.show_copied_feedback_with_duration(
                    "已复制: @grok_bot",
                    Duration::from_millis(50),
                    cx,
                );
                assert_eq!(b.copied_feedback.as_deref(), Some("已复制: @grok_bot"));
            })
            .unwrap();
        app.background_executor().timer(Duration::from_millis(15)).await;
        box_ent
            .update(&mut app, |b, cx| {
                b.show_copied_feedback_with_duration(
                    "已复制: https://t.me/grok_bot",
                    Duration::from_millis(120),
                    cx,
                );
            })
            .unwrap();
        app.background_executor().timer(Duration::from_millis(55)).await;
        box_ent
            .read_with(&app, |b, _| {
                assert_eq!(
                    b.copied_feedback.as_deref(),
                    Some("已复制: https://t.me/grok_bot")
                );
            })
            .unwrap();
        app.background_executor().timer(Duration::from_millis(95)).await;
        box_ent
            .read_with(&app, |b, _| {
                assert!(b.copied_feedback.is_none());
            })
            .unwrap();

        box_ent
            .update(&mut app, |b, cx| {
                b.show_copied_feedback_with_duration("已复制: temp", Duration::from_millis(100), cx);
                b.close();
                assert!(b.copied_feedback.is_none());
            })
            .unwrap();
    }

    async fn body_stage_toast_timers(mut app: AsyncApp) {
        let stage = app
            .new(|_| ConversationStage::new())
            .expect("create ConversationStage entity");

        stage
            .update(&mut app, |st, cx| {
                st.set_chat(1, "Bot", "bot", "BT", 0x2AABEE);
                let base_gen = st.toast_banner_gen;
                st.show_toast_banner_with_duration("Copied A", Duration::from_millis(50), cx);
                assert_eq!(st.toast_banner.as_deref(), Some("Copied A"));
                assert_eq!(st.toast_banner_gen, base_gen + 1);
            })
            .unwrap();

        app.background_executor().timer(Duration::from_millis(15)).await;

        stage
            .update(&mut app, |st, cx| {
                let prev_gen = st.toast_banner_gen;
                st.show_toast_banner_with_duration("Copied B", Duration::from_millis(120), cx);
                assert_eq!(st.toast_banner.as_deref(), Some("Copied B"));
                assert_eq!(st.toast_banner_gen, prev_gen + 1);
            })
            .unwrap();

        app.background_executor().timer(Duration::from_millis(55)).await;
        stage
            .read_with(&app, |st, _| {
                assert_eq!(st.toast_banner.as_deref(), Some("Copied B"));
            })
            .unwrap();

        app.background_executor().timer(Duration::from_millis(95)).await;
        stage
            .read_with(&app, |st, _| {
                assert!(st.toast_banner.is_none());
            })
            .unwrap();

        // Chat switch clears active toast immediately and fences pending timer
        stage
            .update(&mut app, |st, cx| {
                st.show_toast_banner_with_duration("Copied C", Duration::from_millis(60), cx);
                assert!(st.toast_banner.is_some());
                st.set_chat(2, "Group", "2 members", "GR", 0x2AABEE);
                assert!(st.toast_banner.is_none());
            })
            .unwrap();

        app.background_executor().timer(Duration::from_millis(80)).await;
        stage
            .read_with(&app, |st, _| {
                assert!(st.toast_banner.is_none());
            })
            .unwrap();

        // Manual dismiss clears toast immediately
        stage
            .update(&mut app, |st, cx| {
                st.show_toast_banner_with_duration("Copied D", Duration::from_millis(200), cx);
                st.dismiss_toast_banner();
                assert!(st.toast_banner.is_none());
            })
            .unwrap();
    }

    async fn body_render_contact_profile_panel_branches(mut app: AsyncApp) {
        let pal = TdesktopThemePalette::default();
        let insp = app
            .new(|_| {
                let mut i = ContextInspector::new();
                i.open();
                i.set_palette(pal);
                i.set_language("zh-CN");
                i
            })
            .expect("create ContextInspector entity");

        insp.update(&mut app, |i, cx| {
            // Group branch renders cleanly
            i.profile.name = "AA 记账分账助手".to_string();
            i.profile.is_bot = false;
            i.profile.facts = vec![
                ("类型".to_string(), "群组".to_string()),
                ("成员".to_string(), "3".to_string()),
            ];
            i.members.is_group_or_channel = true;
            i.members.invite_link = "https://t.me/+test".to_string();
            let _group_el = render_contact_profile_panel(
                &i.profile,
                &i.members,
                &i.shared_media,
                &i.collapsed_sections,
                i.is_secret_chat_active,
                i.is_contact_blocked,
                i.copied_feedback.as_ref(),
                pal,
                "zh-CN",
                cx,
            );

            // 1v1 User branch renders cleanly
            i.clear_peer_facts();
            i.profile.name = "Alice".to_string();
            i.profile.phone = "+15550001111".to_string();
            i.profile.username = "@alice".to_string();
            i.profile.bio = "Hello".to_string();
            let _user_el = render_contact_profile_panel(
                &i.profile,
                &i.members,
                &i.shared_media,
                &i.collapsed_sections,
                i.is_secret_chat_active,
                i.is_contact_blocked,
                i.copied_feedback.as_ref(),
                pal,
                "zh-CN",
                cx,
            );
        })
        .unwrap();
    }

    #[test]
    fn test_headless_gpui_copied_feedback_auto_dismiss_generation_fence_and_switch_clear() {
        run_in_headless_subprocess(
            "test_headless_gpui_copied_feedback_auto_dismiss_generation_fence_and_switch_clear",
        );
    }

    #[test]
    fn test_headless_gpui_stage_toast_banner_auto_dismiss_generation_fence_and_set_chat_clear() {
        run_in_headless_subprocess(
            "test_headless_gpui_stage_toast_banner_auto_dismiss_generation_fence_and_set_chat_clear",
        );
    }

    #[test]
    fn test_headless_gpui_render_contact_profile_panel_branches() {
        run_in_headless_subprocess("test_headless_gpui_render_contact_profile_panel_branches");
    }

    async fn body_inspector_pending_leave_chat_immediate_modal(mut app: AsyncApp) {
        use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
        use gpui_telegram_desktop::views::app_shell::shell_truth::{
            apply_authoritative_chat_list, select_authoritative_dialog,
        };
        use gpui_telegram_desktop::views::boxes::modal_types::ModalDialogKind;
        use gpui_telegram_desktop::views::AppShell;
        use gpui_telegram_client::ChatSummary;
        use std::sync::Arc;

        let mut cfg = ClientConfig::default();
        cfg.server_url = "http://127.0.0.1:9".to_string();
        let client = Arc::new(TelegramClient::new(cfg));
        let _rt = client.runtime().enter();
        client.set_auth_session(AuthResponse {
            token: "tok_leave_group".to_string(),
            session_id: "sess_leave_group".to_string(),
            user_id: 1001,
            username: "tester".to_string(),
            first_name: None,
            created_at: None,
            expires_at: None,
        });

        let shell = app
            .new(|cx| {
                let mut s = AppShell::new_with_client(client.clone(), cx);
                s.is_authenticated = true;
                s.sidebar.update(cx, |sb, sb_cx| {
                    s.stage.update(sb_cx, |st, st_cx| {
                        s.inspector.update(st_cx, |insp, _| {
                            apply_authoritative_chat_list(
                                sb,
                                st,
                                insp,
                                &[ChatSummary {
                                    chat_id: 505,
                                    title: "AA 记账分账助手".to_string(),
                                    chat_type: 1,
                                    last_message: None,
                                    unread_count: 0,
                                    is_group: Some(true),
                                    is_bot: Some(false),
                                    avatar_color: Some(0x2AABEE),
                                    updated_at: Some(1_700_000_000_000),
                                }],
                            );
                            select_authoritative_dialog(sb, st, insp, 505);
                            st.chat_subtitle = "3 位成员".to_string();
                            st.is_group_chat = false;
                            insp.members.is_group_or_channel = true;
                        });
                    });
                });
                s
            })
            .expect("create AppShell entity");

        let inspector = shell.read_with(&app, |s, _| s.inspector.clone()).unwrap();
        inspector
            .update(&mut app, |i, icx| {
                i.pending_leave_chat = true;
                icx.notify();
            })
            .unwrap();

        app.background_executor().timer(Duration::from_millis(20)).await;

        shell
            .update(&mut app, |s, cx| {
                assert!(
                    !s.inspector.read(cx).pending_leave_chat,
                    "pending_leave_chat must be drained immediately by inspector observer"
                );
                assert!(
                    s.modal_container.read(cx).is_modal_active(),
                    "LeaveOrDeleteChat modal must open immediately on pending_leave_chat notify"
                );
                assert!(
                    matches!(
                        s.modal_container.read(cx).dialog.kind,
                        Some(ModalDialogKind::LeaveOrDeleteChat { is_group: true, .. })
                    ),
                    "Expected LeaveOrDeleteChat with is_group=true, got {:?}",
                    s.modal_container.read(cx).dialog.kind
                );

                // Dismiss modal and verify subsequent handle_stage_pending_actions does NOT double-trigger
                s.modal_container.update(cx, |m, mcx| {
                    m.hide_all();
                    mcx.notify();
                });
                assert!(!s.modal_container.read(cx).is_modal_active());
                s.handle_stage_pending_actions(cx);
                assert!(
                    !s.modal_container.read(cx).is_modal_active(),
                    "handle_stage_pending_actions must not double-trigger LeaveOrDeleteChat"
                );
            })
            .unwrap();
    }

    #[test]
    fn test_headless_gpui_inspector_pending_leave_chat_immediate_modal_no_double_trigger() {
        run_in_headless_subprocess(
            "test_headless_gpui_inspector_pending_leave_chat_immediate_modal_no_double_trigger",
        );
    }
}
