//! Live OpenBot UI/UX Refactor E2E Runtime Probe & 29-Test Verification Suite (`UI-F01`..`UI-F20`).
//!
//! Matches `TEST_READY.md` §8.3 1:1 with ALL 29 exact verbatim `#[test]` functions:
//! - 1 probe emitter (`emit_openbot_uiux_probe_json`) invoked by `scripts/run_openbot_uiux_e2e.py`
//! - 20 core feature tests (`ui_f01_*`..`ui_f20_*`) directly exercising live production Rust APIs
//! - 8 dedicated `v2.0.0` defect regression tests (`C3`, `C5`, `C7`, `C8`, `C9`, `C10`, `C11`, `C13/F12/F13`)
//!
//! Zero `Application::headless().run(...)` calls, zero dead helpers, zero hardcoded probe echoes.

mod openbot_uiux_probe;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_telegram_desktop::app::preferences::AppPreferences;
use gpui_telegram_desktop::components::bloub_avatar::{
    AgentAvatarController, AgentAvatarProps, BundledPlushAssets, ChannelActivityIndicatorState,
    ChannelAvatarLayout, ChannelClusterMode, ChannelOverflowPillTone, ChannelPictureEditorState,
    ChannelWorkerBrief, EDITOR_COLOR_TOTAL_CELLS, MarketplaceNoMatchPlushState, PlushFeltRenderer,
    SIDEBAR_BADGE_ROLL_OUT_MS, SidebarBadgeAnimator, SidebarBadgePhase, SidebarFeltBadge,
    SidebarWaitReason, composite_plush_rgba, composite_plush_rgba_with_assets,
    compute_working_badge_layout, resolve_channel_cluster, resolve_ink_hex, tint_ramp,
};
use gpui_telegram_desktop::theme::openbot_palette::{
    AppAppearanceMode, AppearancePreferencePatch, AppearancePreferenceStore, CHAT_COLOR_PRESETS,
    OpenBotColorScheme, OpenBotPalette, readable_chat_background, solid_bubble_takes_dark_text,
    solid_bubble_text_hex,
};
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::app_shell::modal_container::{
    ACTION_MENU_DEFAULT_WIDTH_PX, ActionMenuItem, ActionMenuState, OpenBotToastTone,
    OpenBotToasterState, compute_action_menu_layout, compute_toaster_layout,
};
use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    ChatNavHistory, LayoutRect, OpenBotShellFrameInput, apply_cmd_b_sidebar_toggle,
    compute_openbot_shell_frame_layout, compute_shell_account_footer_audit,
    configure_openbot_sidebar_for_shell, default_openbot_shell_wiring,
    evaluate_headless_shell_layout_suite, resolve_right_drawer_slot,
};
use gpui_telegram_desktop::views::app_shell::shell_bot_fusion::OpenBotRightSidePanelState;
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleAppSidebar, AppleSidebarSection, DEFAULT_PINNED_SECTIONS, OPENBOT_PAGE_RAIL_WIDTH,
    compute_tooltip_layout, load_page_rail_pins,
};
use gpui_telegram_desktop::views::boxes::modal_theme::{
    ConfirmDialogState, compute_confirm_dialog_layout,
};
use gpui_telegram_desktop::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers;
use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
use gpui_telegram_desktop::views::chat_sidebar::{
    COLLAPSED_WIDTH as LEGACY_SB_COLLAPSED, ChatSidebar, DEFAULT_WIDTH as LEGACY_SB_DEFAULT,
    DialogDeliveryStatus, DialogItemModel, OPENBOT_SIDEBAR_COMPACT_WIDTH,
    OPENBOT_SIDEBAR_DEFAULT_WIDTH, ROW_HEIGHT as LEGACY_SB_ROW_H,
};
use gpui_telegram_desktop::views::community::shared_widgets::compute_community_hub_layout;
use gpui_telegram_desktop::views::composer::composer_ui::{
    compute_openbot_composer_strip_layout, extract_composer_inline_chips,
};
use gpui_telegram_desktop::views::composer::editor_state::{
    atomic_reference_chip_span_before_cursor, detect_autocomplete_trigger,
};
use gpui_telegram_desktop::views::composer::ime_machine::{ImeState, KeyAction};
use gpui_telegram_desktop::views::composer::popups_ui::compute_openbot_picker_popup_layout;
use gpui_telegram_desktop::views::content_hub::ContentHubSection;
use gpui_telegram_desktop::views::content_hub::apple_primitives::{
    compute_content_hub_shell_layout, is_legacy_blue_rgb,
};
use gpui_telegram_desktop::views::content_hub::book_reader::BookReaderState;
use gpui_telegram_desktop::views::content_hub::game_runtime::GameInput;
use gpui_telegram_desktop::views::content_hub::game_runtime_sheet::ActiveGameSession;
use gpui_telegram_desktop::views::content_hub::publish_review_modal::PublishReviewModalState;
use gpui_telegram_desktop::views::context_inspector::{
    BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS, BotIdentityAutosaveState, ContextInspector,
    ContextInspectorRenderRoute, DRAWER_WIDTH as LEGACY_INSPECTOR_W, validate_bot_profile_name,
};
use gpui_telegram_desktop::views::conversation_stage::{
    ConversationStage, DirectDmState, MessageContent, MessageDirection, MessageItemModel,
    compute_agent_activity_layout,
};
use gpui_telegram_desktop::views::drawer::bot_computer_inspector::compute_openbot_bot_settings_layout;
use gpui_telegram_desktop::views::drawer::inspector_panels::{
    compute_openbot_channel_settings_layout, compute_openbot_contact_profile_layout,
};
use gpui_telegram_desktop::views::drawer::inspector_tabs::{
    BotPublishedCreationSummary, OpenBotConfirmGuard, OpenBotSettingsSubpage,
};
use gpui_telegram_desktop::views::main_menu_drawer::{
    MainMenuDrawer, compute_main_menu_drawer_layout,
};
use gpui_telegram_desktop::views::nav_rail::NAV_RAIL_WIDTH;
use gpui_telegram_desktop::views::openbot::agent_studio::{
    FIRST_AGENT_COLOR_CHOICES_COUNT, FIRST_AGENT_PURPOSE_MAX_CHARS, FirstAgentSetupMode,
    FirstAgentSetupState, GLOBAL_SEARCH_DEBOUNCE_MS, GlobalSearchResultItem,
    GlobalSearchSectionKind, GlobalSearchState, GlobalSearchTab, compute_first_agent_setup_layout,
    compute_global_search_modal_layout,
};
use gpui_telegram_desktop::views::openbot::ask_ui::{
    UI_BLOCK_CARD_MAX_WIDTH_PX, validate_and_evaluate_ui_block,
};
use gpui_telegram_desktop::views::openbot::channel_editor::{
    ChannelEditorState, ChannelEditorValidationError, resolve_channel_icon,
};
use gpui_telegram_desktop::views::openbot::channel_view::{
    AwaitingRepliesTracker, AwaitingReplyEntry, AwaitingReplyState, ChannelConversationState,
    ChannelRoutingEvent, ChannelStoppedTasksState, StoppedChannelTaskBannerItem,
    merge_consecutive_routing_events,
};
use gpui_telegram_desktop::views::openbot::composer::{
    ComposerAddMenuItem, ComposerAttachmentDraft, ComposerSkillEntry, ComposerState,
    ComposerTrigger, DollarPickerItem, DollarPickerItemKind,
};
use gpui_telegram_desktop::views::openbot::conversation::{
    ChatRowBoundary, ChatTimelineRow, ConversationViewState, compute_message_preview,
};
use gpui_telegram_desktop::views::openbot::queue_and_voice::{
    ComposerNoticeState, ComposerStackInput, QueuePanelState, compute_composer_stack_layout,
    evaluate_steer,
};
use gpui_telegram_desktop::views::openbot::settings_and_shortcuts::{
    FixedShortcutGroup, ShortcutChord, ShortcutCommand, ShortcutRegistry,
};
use gpui_telegram_desktop::views::openbot::shell_state::{
    PAGE_RAIL_WIDTH_PX, RailPersistedState, SIDEBAR_GROUP_ORDER, ServerPlatform, ServerRailState,
    ServerViewMode, SidebarLayoutSnapshot, SidebarSection, UsageRingMetric,
    validate_sidebar_layout_strict,
};
use gpui_telegram_desktop::views::openbot::side_panel::{
    CONVERSATION_PANEL_MIN, MAX_SIDE_SPLITS, SETTINGS_PANEL_DEFAULT, SETTINGS_PANEL_MAX,
    SIDE_QUESTION_MAX_TURNS, SidePanelPaneKind, SidePanelSettingsTarget,
};
use gpui_telegram_desktop::views::openbot::steps_and_trace::AgentStepsState;
use gpui_telegram_desktop::views::openbot::ui_probe::evaluate_ui_probe;
use gpui_telegram_desktop::views::openbot::workspace_shell::OpenBotWorkspaceShell;
use gpui_telegram_desktop::views::settings::SettingsRouter;
use gpui_telegram_desktop::views::settings::settings_theme_ui::compute_settings_page_layout;
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::sidebar::dialogs_overlays::{
    OPENBOT_THREAD_SUBROW_HEIGHT_PX, compute_openbot_sidebar_tabs_layout,
};
use gpui_telegram_desktop::views::sidebar::dialogs_renderer::{
    compute_openbot_pinned_grid_layout, compute_sidebar_empty_state_layout,
};
use gpui_telegram_desktop::views::sidebar::dialogs_row_view::{
    SidebarDialogMeta, compute_openbot_dialog_row_layout, openbot_context_menu_actions,
    sidebar_context_action_key,
};
use gpui_telegram_desktop::views::sidebar::openbot_sidebar_chrome::{
    compute_openbot_sidebar_search_row_layout, compute_openbot_sidebar_topbar_layout,
};
use gpui_telegram_desktop::views::stage::cards::models::DeliveryStatus;
use gpui_telegram_desktop::views::stage::entities_render::extract_rendered_message_reference_chips;
use gpui_telegram_desktop::views::stage::history_element::{MSG_MAX_WIDTH, MSG_MIN_WIDTH};
use gpui_telegram_desktop::views::stage::history_view_highlight_manager::{
    ACTIVE_FADE_IN_DURATION_MS, ACTIVE_FADE_OUT_DURATION_MS, FADE_WRAP_DURATION_MS,
    HistoryViewHighlightManager, OPENBOT_REPLY_FLASH_DURATION_MS, SelectedQuote,
};
use gpui_telegram_desktop::views::stage::openbot_stage_chrome::compute_openbot_reference_chip_layout;
use gpui_telegram_desktop::views::stage::stage_bars::{
    HeaderModelPickerFitMode, OPENBOT_HEADER_HEIGHT, compute_openbot_header_layout,
};
use gpui_telegram_desktop::views::stage::stage_bubble::{
    BubbleGroupLayout, OPENBOT_ENTRY_GAP_PX, OPENBOT_GROUPED_GAP_PX, compute_openbot_bubble_layout,
    extract_structured_error_payload,
};
use gpui_telegram_desktop::views::stage::wallpaper::AGENT_EMPTY_STATE_HEADING_PX;
use gpui_telegram_media::media::audio::{
    MusicPlaybackState, MusicTrackEntry, MusicTransportEngine,
};
use gpui_telegram_proto::creations::{
    BookChapter, BookPage, BookPayload, BookRichBlock, CoverArtwork, CreationKind, CreationPayload,
    CreationReviewStatus, CreationVisibility, GameCategory, GameEngineKind, GamePayload, GameSpec,
    TriviaQuestion, UserCreationDto,
};
use gpui_telegram_proto::miniapp_contracts::{FABUSHI_AVATAR_SHAPES, default_fabushi_avatar_shape};
use openbot_contracts::avatar::{
    AVATAR_COLOR_CHOICES, AVATAR_EXPRESSIONS, AgentStepActivity, AvatarMood, BloubShape,
    EngineShapeId, PlushState, SEEDED_AVATAR_SILHOUETTES, SUPPORTED_AVATAR_SILHOUETTES,
    activity_mood_presentation, bloub_avatar_profile, compute_agent_avatar_mood, stable_index,
};
use openbot_contracts::channels::{ChannelIcon, ChannelMember};
use openbot_contracts::conversation::ConversationMessage;
use openbot_contracts::providers::AgentProviderId;
use openbot_contracts::queue::QueueSender;
use openbot_contracts::tags::{ChatTagKind, chat_tag_references};
use serde_json::{Value, json};

fn sample_creation_dto(
    id: &str,
    kind: CreationKind,
    title: &str,
    summary: &str,
    payload: CreationPayload,
) -> UserCreationDto {
    UserCreationDto {
        creation_id: id.into(),
        owner_user_id: 1,
        creator_bot_user_id: None,
        creator_chat_id: None,
        kind,
        title: title.into(),
        subtitle: String::new(),
        summary: summary.into(),
        author_display_name: "OpenBot Studio".into(),
        language: "en".into(),
        genres: vec!["Tech".into()],
        tags: vec!["openbot".into()],
        cover: CoverArtwork::default(),
        age_rating: "4+".into(),
        payload,
        version: 1,
        cloud_revision: 1,
        review_status: CreationReviewStatus::Draft,
        visibility: CreationVisibility::Private,
        has_live_published_version: false,
        published_version: None,
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: None,
        created_at_ms: 1000,
        updated_at_ms: 1000,
        deleted_at_ms: None,
        rating_avg: 4.8,
        rating_count: 10,
        engagement_count: 50,
        save_or_install_count: 20,
    }
}

#[test]
fn ui_f01_tokens_oklab_wcag_and_tdesktop_sync() {
    let dark_ob = OpenBotPalette::dark();
    let light_ob = OpenBotPalette::light();
    assert_eq!(dark_ob.token("--openbot-bg-frame"), Some("#0d0d0d"));
    assert_eq!(dark_ob.token("--openbot-bg-canvas"), Some("#1a1a1a"));
    assert_eq!(light_ob.token("--openbot-bg-frame"), Some("#e8e8eb"));
    assert!(!dark_ob.window_glass_tokens().contains_key("--openbot-bg-surface"));

    let td = openbot_uiux_probe::dark_palette();
    assert!(!is_legacy_blue_rgb(td.msg_out_bg));
    assert!(!is_legacy_blue_rgb(td.topbar_bg));
    assert_eq!(td.msg_out_bg, 0x242424);

    assert_eq!(CHAT_COLOR_PRESETS.len(), 11);
    for hx in ["#4699fe", "#ff00ff"] {
        let out = readable_chat_background(hx, OpenBotColorScheme::Dark).expect("oklab dark");
        assert_eq!(out.len(), 7);
        assert!(out.starts_with('#'));
    }
    assert!(solid_bubble_takes_dark_text("#e5a50a"));
    assert_eq!(solid_bubble_text_hex("#e5a50a"), "#141414");
    assert!(!solid_bubble_takes_dark_text("#242424"));
    assert_eq!(solid_bubble_text_hex("#242424"), "#ffffff");

    let tmp = tempfile::tempdir().expect("tempdir prefs");
    let mut store = AppearancePreferenceStore::for_data_dir(tmp.path());
    std::fs::create_dir_all(store.themes_dir()).expect("create themes dir");
    let custom = json!({
        "id": "midnight-studio", "name": "Midnight Studio", "appearance": "dark", "scheme": "dark",
        "colors": {
            "canvas": "#16181c", "frame": "#0c0d10", "sidebar": "#14161a", "surface": "#1e2026",
            "text": "#f4f6f8", "textMuted": "#8f96a3", "border": "#2a2d36", "accent": "#007cf7",
            "accentForeground": "#ffffff", "success": "#31cf76", "warning": "#ff9412", "danger": "#fd2f3b"
        }
    });
    std::fs::write(store.themes_dir().join("midnight-studio.json"), custom.to_string()).unwrap();
    let _ = store.load();
    let _ = store.set_palette(Some("midnight-studio".into()));
    assert!(store.selected_custom_palette().is_some());
    let _ = store.set_palette(None);
    assert_eq!(store.effective_palette().token("--openbot-bg-frame"), Some("#0d0d0d"));

    let bad_err = OpenBotPalette::from_theme_json_str("BAD ID!", r#"{"id":"BAD ID!","colors":{}}"#).unwrap_err();
    assert!(bad_err.is_validation_error());
    let font_err = store
        .update(AppearancePreferencePatch {
            interface_font: Some(Some("Inter\"; body {}".into())),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(font_err.code(), "invalid_font_family");
    assert!(store.update(AppearancePreferencePatch {
        interface_font: Some(Some("Inter, SF Pro Display".into())),
        ..Default::default()
    }).is_ok());
    assert!(AppAppearanceMode::parse("neon-invalid").is_none());
}

#[test]
fn ui_f02_plush_assets_lut_moods_and_felt_badges() {
    let (webp, webm, valid) = openbot_uiux_probe::f01_f02::scan_plush_assets();
    assert_eq!(webp, 73);
    assert_eq!(webm, 1);
    assert!(valid);

    let mut creator = CreateManagedBotBoxPeers::new();
    creator.open_fabushi_creator();
    assert!(creator.avatar_shape.starts_with("plush:"));
    assert_eq!(EDITOR_COLOR_TOTAL_CELLS, 18);
    assert_eq!(BloubShape::ALL.len(), 13);
    assert!(BloubShape::ALL.iter().all(|s| s.as_str() != "goutte"));

    assert_eq!(tint_ramp("#3b93f0").len(), 256);
    let assets = BundledPlushAssets::new();
    let t1 = assets.tinted_rgba("bloub/boule.webp", "#3b93f0", None).expect("tint 1");
    let t2 = assets.tinted_rgba("bloub/boule.webp", "#3b93f0", None).expect("tint 2");
    assert!(Arc::ptr_eq(&t1, &t2));
    assert_ne!(resolve_ink_hex("not-a-hex"), "not-a-hex");
    let frame = PlushFeltRenderer::for_seed("a", None, None, AvatarMood::Idle).sample_plush_frame(0.0, false);
    assert!(composite_plush_rgba(&frame, 32).pixels().any(|p| p.0[3] > 0));

    let g_seed = (0..256)
        .map(|i| format!("goutte-seed-{i}"))
        .find(|s| SEEDED_AVATAR_SILHOUETTES[stable_index(&format!("{s}:shape"), SEEDED_AVATAR_SILHOUETTES.len())] == EngineShapeId::Goutte)
        .expect("goutte seed");
    assert_ne!(bloub_avatar_profile(&g_seed, None, None).shape.as_str(), "goutte");

    assert_eq!(compute_agent_avatar_mood(true, true, true, true, true, true), AvatarMood::Failed);
    for act in [
        AgentStepActivity::Searching, AgentStepActivity::Browsing, AgentStepActivity::Reading,
        AgentStepActivity::Running, AgentStepActivity::Editing, AgentStepActivity::Delegating,
        AgentStepActivity::Working, AgentStepActivity::Reviewing,
    ] {
        let pres = activity_mood_presentation(act);
        if pres.state == PlushState::Idle {
            assert!(pres.expression.is_some());
        }
    }
    assert_eq!(SIDEBAR_BADGE_ROLL_OUT_MS, 200);
    let wb = compute_working_badge_layout(true, false, 240);
    assert_eq!(wb.width_px, 30.0);
    assert_eq!(wb.ball_count, 3);
    assert!(!wb.balls_paused);
    assert!(compute_working_badge_layout(true, true, 240).balls_paused);

    let mut anim = SidebarBadgeAnimator::default();
    anim.update(Some(SidebarFeltBadge::Unread { count: 1 }), 0, false);
    anim.update(None, 100, false);
    assert_eq!(anim.phase, SidebarBadgePhase::Out);
    assert!(anim.current.is_some());
    assert!(SidebarFeltBadge::Waiting { reason: SidebarWaitReason::Approval }.box_px() > 0.0);
}

#[test]
fn ui_f02_defect_c3_reduced_motion_and_offscreen_fps_must_be_zero() {
    let ctrl = AgentAvatarController::new("c3-agent");
    let p_act = AgentAvatarProps::new("c3-agent").with_mood(AvatarMood::Working);
    let r_act = ctrl.resolve(&p_act);
    assert!(r_act.animated && r_act.fps > 0 && r_act.frozen_at.is_none());

    let mut p_rm = p_act.clone();
    p_rm.reduced_motion = true;
    let r_rm = ctrl.resolve(&p_rm);
    assert_eq!(r_rm.fps, 0);
    assert!(!r_rm.animated);
    assert!(r_rm.frozen_at.is_some());

    let mut p_off = p_act.clone();
    p_off.on_screen = false;
    let r_off = ctrl.resolve(&p_off);
    assert_eq!(r_off.fps, 0);
    assert!(!r_off.animated);

    let mut p_blur = p_act;
    p_blur.document_focused = false;
    let r_blur = ctrl.resolve(&p_blur);
    assert_eq!(r_blur.fps, 0);
    assert!(!r_blur.animated);
}

#[test]
fn ui_f03_shell_frame_constants_and_server_rail_widths() {
    let dark = openbot_uiux_probe::dark_palette();
    let light = openbot_uiux_probe::light_palette();

    let f_dk = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1440.0, 900.0, dark));
    assert_eq!(f_dk.frame_bg_hex, "#0d0d0d");
    assert_eq!(f_dk.canvas_bg_hex, "#1a1a1a");
    assert_eq!(f_dk.workspace_card_radius_px, 14.0);
    assert_eq!(f_dk.workspace_card_border_px, 1);
    assert_eq!(f_dk.workspace_margin_px, 4.0);
    assert_eq!(f_dk.top_chrome_height_px, 38.0);
    assert!(f_dk.page_rail_visible && f_dk.page_rail_width_px == 52.0);
    assert_eq!(f_dk.sidebar_rect.width, 280.0);
    assert!(!f_dk.narrow_viewport);

    let f_lt = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1440.0, 900.0, light));
    assert_eq!(f_lt.frame_bg_hex, "#e8e8eb");

    let f_720 = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(720.0, 800.0, dark));
    assert!(f_720.narrow_viewport && !f_720.page_rail_visible && f_720.workspace_card_radius_px == 0.0 && f_720.workspace_margin_px == 0.0);
    let f_721 = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(721.0, 800.0, dark));
    assert!(!f_721.narrow_viewport && f_721.page_rail_visible && f_721.workspace_card_radius_px == 14.0);
    let f_0 = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(0.0, 0.0, dark));
    assert_eq!(f_0.canvas_card_rect, LayoutRect::ZERO);

    let mut inp_wg = OpenBotShellFrameInput::desktop_default(1440.0, 900.0, dark);
    inp_wg.window_glass = true;
    let f_wg = compute_openbot_shell_frame_layout(&inp_wg);
    assert!(f_wg.window_glass_active && !f_wg.tab_shape_enabled);

    let mut srv = ServerRailState::new();
    srv.view_mode = ServerViewMode::Rail;
    assert_eq!(srv.effective_width_px(ServerPlatform::MacOs, 1440), 72.0);
    assert_eq!(srv.effective_width_px(ServerPlatform::Linux, 1440), 64.0);
    assert_eq!(srv.effective_width_px(ServerPlatform::Linux, 801), 64.0);
    assert_eq!(srv.effective_width_px(ServerPlatform::Linux, 800), 56.0);
}

#[test]
fn ui_f03_defect_c5_shell_state_page_rail_52px_and_server_rail_default_menu() {
    assert_eq!(PAGE_RAIL_WIDTH_PX, 52.0);
    assert_eq!(OPENBOT_PAGE_RAIL_WIDTH, 52.0);
    let srv = ServerRailState::new();
    assert_eq!(srv.view_mode, ServerViewMode::Menu);
    assert_eq!(srv.effective_width_px(ServerPlatform::Linux, 1440), 0.0);
    assert_eq!(srv.effective_width_px(ServerPlatform::MacOs, 1440), 0.0);
    assert_eq!(RailPersistedState::default().server_view_mode, ServerViewMode::Menu);
}

#[test]
fn ui_f04_page_rail_52px_geometry_pins_and_narrow_hide() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut rail = AppleAppSidebar::new(dark, "zh-CN");
    let l = rail.compute_layout(900.0);
    assert_eq!(l.width_px, 52.0);
    assert_eq!(l.button_size_px, 36.0);
    assert!(l.visible && l.bottom_items.help_visible && l.bottom_items.settings_visible);
    assert_eq!(rail.pinned_sections, DEFAULT_PINNED_SECTIONS.to_vec());
    assert!(!AppleSidebarSection::Chats.is_pinnable());
    assert!(!rail.toggle_pin_local(AppleSidebarSection::Chats));

    rail.hide_bottom_item_by_id("help");
    assert!(!rail.compute_layout(900.0).bottom_items.help_visible);
    assert!(rail.undo_toast_item.is_some());
    rail.undo_hide_bottom_item();
    assert!(rail.compute_layout(900.0).bottom_items.help_visible);

    rail.hide_bottom_item_by_id("settings");
    rail.hide_bottom_item_by_id("account");
    assert!(rail.hidden_bottom_items.is_empty());

    rail.set_usage_ring_metrics((0..5).map(|i| UsageRingMetric {
        provider: AgentProviderId::Codex,
        label: format!("P{i}"),
        primary_ratio: Some(0.4),
        secondary_ratio: None,
        unavailable: false,
        reset_label: None,
        detail_summary: None,
    }).collect());
    assert!(rail.compute_layout(360.0).bottom_items.collapsed_more_rings_count > 0);

    rail.toggle_more_menu();
    assert!(rail.more_menu_open);
    rail.set_narrow_hidden(true);
    assert!(!rail.more_menu_open);
    assert!(!rail.compute_layout(800.0).visible);
}

#[test]
fn ui_f04_defect_c7_corrupt_page_rail_pins_must_preserve_valid_entries() {
    let d1 = tempfile::tempdir().unwrap();
    std::fs::write(
        d1.path().join("openbot-page-rail-pins-v1.json"),
        r#"{"version":1,"pinned_sections":["books","nonexistent_tab","","chats","books"]}"#,
    ).unwrap();
    assert_eq!(load_page_rail_pins(d1.path()), vec![AppleSidebarSection::Books]);

    let d2 = tempfile::tempdir().unwrap();
    std::fs::write(d2.path().join("openbot-page-rail-pins-v1.json"), r#"{"version":1,"pinned_sections":[]}"#).unwrap();
    assert!(load_page_rail_pins(d2.path()).is_empty());

    let d3 = tempfile::tempdir().unwrap();
    std::fs::write(d3.path().join("openbot-page-rail-pins-v1.json"), b"corrupt{").unwrap();
    assert_eq!(load_page_rail_pins(d3.path()), DEFAULT_PINNED_SECTIONS.to_vec());
}

#[test]
fn ui_f05_sidebar_chrome_hysteresis_and_cmd_b_toggle() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut sb = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb, default_openbot_shell_wiring());
    sb.set_palette(dark);
    assert_eq!(sb.rendered_sidebar_width(), OPENBOT_SIDEBAR_DEFAULT_WIDTH);

    sb.apply_openbot_drag_width(105.0);
    assert!(sb.is_collapsed && sb.rendered_sidebar_width() == OPENBOT_SIDEBAR_COMPACT_WIDTH);
    sb.apply_openbot_drag_width(114.0);
    assert!(sb.is_collapsed && sb.rendered_sidebar_width() == OPENBOT_SIDEBAR_COMPACT_WIDTH);
    sb.apply_openbot_drag_width(117.0);
    assert!(sb.is_collapsed && sb.rendered_sidebar_width() == OPENBOT_SIDEBAR_COMPACT_WIDTH);
    sb.apply_openbot_drag_width(118.0);
    assert!(!sb.is_collapsed && sb.rendered_sidebar_width() == 128.0);
    sb.apply_openbot_drag_width(114.0);
    assert!(!sb.is_collapsed && sb.rendered_sidebar_width() == 128.0);
    sb.apply_openbot_drag_width(650.0);
    assert_eq!(sb.rendered_sidebar_width(), 400.0);

    assert!(!apply_cmd_b_sidebar_toggle(ActiveWorkspacePanel::ContentHub(ContentHubSection::Books), &mut sb));
    assert!(!sb.is_sidebar_closed);
    assert!(apply_cmd_b_sidebar_toggle(ActiveWorkspacePanel::Chats, &mut sb));
    assert!(sb.is_sidebar_closed && sb.rendered_sidebar_width() == 0.0);
    assert!(apply_cmd_b_sidebar_toggle(ActiveWorkspacePanel::Chats, &mut sb));
    assert!(!sb.is_sidebar_closed);

    let tmp = tempfile::tempdir().unwrap();
    sb.apply_openbot_drag_width(340.0);
    sb.save_openbot_width_prefs(tmp.path()).unwrap();
    let mut sb2 = ChatSidebar::new();
    assert_eq!(sb2.load_openbot_width_prefs(tmp.path()), 340.0);

    let mut hist = ChatNavHistory::new();
    assert_eq!(hist.step(isize::MIN), None);
    assert_eq!(hist.step(isize::MAX), None);
    hist.push(101);
    hist.push(202);
    assert!(hist.can_back() && !hist.can_forward());
    assert_eq!(hist.step(isize::MIN), None);
    assert_eq!(hist.step(isize::MAX), None);
    assert_eq!(hist.go_back(), Some(101));
    assert!(hist.can_forward());
    assert_eq!(hist.step(isize::MIN), None);
    assert_eq!(hist.step(isize::MAX), None);
    assert_eq!(hist.go_forward(), Some(202));

    assert_eq!(compute_openbot_sidebar_topbar_layout(true, false, true, true).height_px, 38.0);
    let sr = compute_openbot_sidebar_search_row_layout(false, true, 0, true, &dark, "zh-CN");
    assert!(sr.filter_empty_visible && sr.clear_filter_visible && sr.add_menu_open && sr.add_menu_items.len() == 3);
}

#[test]
fn ui_f05_defect_c8_chat_sidebar_drag_non_finite_must_preserve_width() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut sb = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb, default_openbot_shell_wiring());
    sb.set_palette(dark);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        sb.apply_openbot_drag_width(bad);
        assert!(sb.openbot_width.is_finite());
        assert_eq!(sb.rendered_sidebar_width(), 280.0);
    }
    sb.apply_openbot_drag_width(320.0);
    sb.apply_openbot_drag_width(f32::NAN);
    assert!(!sb.is_collapsed);
    assert_eq!(sb.rendered_sidebar_width(), 320.0);
}

#[test]
fn ui_f06_sidebar_list_tabs_folders_and_no_dup_footer() {
    let dark = openbot_uiux_probe::dark_palette();
    let tabs = compute_openbot_sidebar_tabs_layout(false, false, false, "All", true, &dark);
    assert!(!tabs.uses_blue_pill && tabs.height_px == 32.0 && tabs.visible);
    assert!(!compute_openbot_pinned_grid_layout(0, false, &dark).visible);
    let pg3 = compute_openbot_pinned_grid_layout(3, false, &dark);
    assert!(pg3.visible && !pg3.legacy_oversized_circles && pg3.is_compact_grid);
    assert_eq!((pg3.tile_w_px, pg3.tile_h_px, pg3.avatar_px), (72.0, 102.0, 56.0));
    let es = compute_sidebar_empty_state_layout(0, &dark, "zh-CN");
    assert!(es.visible && es.cta_label == "创建第一个助手");

    let item = DialogItemModel {
        id: 101,
        peer_name: "法布施规划助手".into(),
        avatar_initials: "法".into(),
        avatar_color: 0x3B93F0,
        last_message: "法布施工作区智能助手👩‍👩‍👧‍👦正在梳理季度产品路线图并整理关键里程碑与交付清单以及全部自动化校验项".into(),
        timestamp: "10:42".into(),
        unread_count: 1,
        is_muted: false,
        is_pinned: false,
        is_online: true,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "All".into(),
    };
    let r280 = compute_openbot_dialog_row_layout(&item, &SidebarDialogMeta::default(), 280.0, "zh-CN");
    assert!(r280.height_px == 62.0 && r280.min_w_0 && r280.preview_truncated && !r280.preview_displayed.contains('\u{FFFD}') && r280.time_visible);
    let r210 = compute_openbot_dialog_row_layout(&item, &SidebarDialogMeta::default(), 210.0, "zh-CN");
    assert!(!r210.time_visible);
    let mut m_arch = SidebarDialogMeta::default();
    m_arch.is_archived = true;
    let r_arch = compute_openbot_dialog_row_layout(&item, &m_arch, 280.0, "zh-CN");
    assert!(r_arch.archived_or_hidden && !r_arch.time_visible && r_arch.grayscale && r_arch.opacity < 1.0);

    let blank_err = validate_sidebar_layout_strict(&SidebarLayoutSnapshot {
        revision: 1,
        sections: vec![SidebarSection { id: "s1".into(), name: "  ".into(), parent_id: None, collapsed: false }],
        order: vec!["s1".into(), "unassigned".into(), "people".into()],
        agent_assignments: Default::default(),
        agent_order: Default::default(),
    }).unwrap_err();
    assert_eq!(blank_err.code, "empty_section_name");
    let deep_err = validate_sidebar_layout_strict(&SidebarLayoutSnapshot {
        revision: 1,
        sections: vec![
            SidebarSection { id: "s1".into(), name: "L1".into(), parent_id: None, collapsed: false },
            SidebarSection { id: "s2".into(), name: "L2".into(), parent_id: Some("s1".into()), collapsed: false },
            SidebarSection { id: "s3".into(), name: "L3".into(), parent_id: Some("s2".into()), collapsed: false },
            SidebarSection { id: "s4".into(), name: "L4".into(), parent_id: Some("s3".into()), collapsed: false },
        ],
        order: vec!["s1".into(), "s2".into(), "s3".into(), "s4".into(), "unassigned".into(), "people".into()],
        agent_assignments: Default::default(),
        agent_order: Default::default(),
    }).unwrap_err();
    assert_eq!(deep_err.code, "folder_depth_exceeded");

    let rail = AppleAppSidebar::new(dark, "zh-CN");
    let mut sb = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb, default_openbot_shell_wiring());
    let audit = compute_shell_account_footer_audit(&rail, &sb);
    assert_eq!(audit.total_account_footers_in_shell, 1);
    assert!(!audit.has_duplicate_account_footer);
}

#[test]
fn ui_f06_defect_c9_sidebar_group_order_must_include_archived() {
    assert_eq!(SIDEBAR_GROUP_ORDER, ["pinned", "waiting", "unread", "sections", "hidden", "archived"]);
    assert_eq!(OPENBOT_THREAD_SUBROW_HEIGHT_PX, 30.0);
    let keys: Vec<&str> = openbot_context_menu_actions(101).into_iter().map(sidebar_context_action_key).collect();
    for expected in ["move_to_folder", "hide", "settings", "archive"] {
        assert!(keys.contains(&expected), "missing context action {expected}");
    }
}

#[test]
fn ui_f07_global_search_tabs_debounce_and_highlight() {
    assert_eq!(GLOBAL_SEARCH_DEBOUNCE_MS, 150);
    let mut gs = GlobalSearchState::new(false);
    let mut seen = Vec::new();
    for _ in 0..GlobalSearchTab::ALL.len() {
        gs.cycle_tab(true);
        seen.push(gs.active_tab);
    }
    assert_eq!(seen.len(), 6);
    assert_eq!(gs.active_tab, GlobalSearchTab::All);

    assert!(!gs.try_open_palette(true));
    assert!(!gs.open);
    assert!(gs.try_open_palette(false));
    assert!(gs.open);
    gs.set_query("planner", 80.0, 0);
    let s1 = gs.begin_remote_search(200).expect("seq 1");
    gs.set_query("alpha", 90.0, 210);
    let s2 = gs.begin_remote_search(400).expect("seq 2");
    let mk = |id: &str, title: &str| GlobalSearchResultItem {
        id: id.into(), section: GlobalSearchSectionKind::Agents, title: title.into(),
        subtitle: "Match".into(), badge: None, avatar_seed: Some(id.into()), shortcut_hint: None,
    };
    gs.apply_remote_results(s2, vec![mk("a2", "Alpha Agent")]);
    gs.apply_remote_results(s1, vec![mk("old", "Stale Agent")]);
    assert!(gs.remote_stale_discarded >= 1);
    assert_eq!(gs.results[0].id, "a2");

    gs.fail_remote_search(s2, "timeout");
    assert!(gs.remote_error.is_some());
    let s3 = gs.retry_remote_search(600);
    gs.apply_remote_results(s3, vec![mk("a3", "Alpha Retry")]);
    assert!(gs.remote_error.is_none());

    gs.move_selection(-1);
    gs.move_selection(100);
    assert!(!gs.build_visible_row_view_models().is_empty());
    let activated = gs.execute_selected();
    assert!(activated.is_some());
    assert!(!gs.open);

    let l1440 = compute_global_search_modal_layout(1440.0, 900.0, true);
    assert_eq!((l1440.width_px, l1440.top_px), (640.0, 112.0));
    assert_eq!(compute_global_search_modal_layout(480.0, 800.0, true).width_px, 432.0);
}

#[test]
fn ui_f07_defect_c10_global_search_plush_metrics_reduced_motion_must_zero_tilt() {
    let mut gs_rm = GlobalSearchState::new(true);
    gs_rm.set_query("search", 120.0, 0);
    let m = gs_rm.plush_metrics();
    assert_eq!(m.tilt_deg, 0.0);
    assert_eq!(m.gaze_x, 0.0);
    assert_eq!(m.gaze_y, 0.0);
    assert!(!m.animated);

    let mut gs_norm = GlobalSearchState::new(false);
    gs_norm.set_caret_x(160.0, 0);
    let mn = gs_norm.plush_metrics();
    assert!(mn.animated && mn.tilt_deg != 0.0 && mn.gaze_x > 0.0);
}

#[test]
fn ui_f08_agent_studio_modes_templates_and_creator_submit() {
    assert_eq!(FIRST_AGENT_COLOR_CHOICES_COUNT, 17);
    let dark_ob = OpenBotPalette::dark();
    let l1 = compute_first_agent_setup_layout(&FirstAgentSetupState::first_run(), &dark_ob, "zh-CN");
    assert_eq!((l1.form_width_px, l1.hero_size_px, l1.swatch_total), (420.0, 72.0, 18));
    assert!(!l1.shapes_contain_goutte && !l1.header_cancel_visible);

    let mut st_add = FirstAgentSetupState::new(FirstAgentSetupMode::Additional);
    assert!(compute_first_agent_setup_layout(&st_add, &dark_ob, "zh-CN").header_cancel_visible);
    st_add.select_color_choice(AVATAR_COLOR_CHOICES[3]);
    st_add.select_face(6);
    st_add.select_suggestion("trip-planner");
    assert_eq!(st_add.matched_suggestion_id.as_deref(), Some("trip-planner"));
    assert!(!st_add.purpose.trim().is_empty());
    let over = st_add.set_purpose("x".repeat(FIRST_AGENT_PURPOSE_MAX_CHARS + 1)).unwrap_err();
    assert_eq!(over.code, "setup_purpose_too_long");

    let mut ws = OpenBotWorkspaceShell::new();
    ws.first_agent_setup = FirstAgentSetupState::first_run();
    assert!(!ws.handle_first_agent_setup_escape());
    ws.first_agent_setup = st_add;
    assert!(ws.handle_first_agent_setup_escape());
    ws.first_agent_setup.name = "Planner Bot".into();
    let created = ws.submit_first_agent_setup().expect("submit");
    assert!(!created.is_empty());

    let mut creator = CreateManagedBotBoxPeers::new();
    creator.open_fabushi_creator();
    assert!(creator.avatar_shape.starts_with("plush:"));
}

#[test]
fn ui_f08_defect_c11_first_agent_setup_blank_name_and_manual_edit_clears_suggestion() {
    let mut st = FirstAgentSetupState::first_run();
    st.name = "   \t ".into();
    assert!(st.validate().is_err_and(|e| e.code == "agent_name_empty"));

    st.select_suggestion("inbox-helper");
    assert_eq!(st.matched_suggestion_id.as_deref(), Some("inbox-helper"));
    st.set_purpose("Custom manual instructions").unwrap();
    assert_eq!(st.matched_suggestion_id, None);
}

#[test]
fn ui_f09_conversation_header_height_and_stage_state() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut stage = ConversationStage::new();
    stage.append_and_focus_thread_tab("Follow-up");
    assert_eq!(stage.thread_tabs.len(), 1);

    let p = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    let l = compute_openbot_header_layout(&p, &dark);
    assert_eq!(l.height_px, OPENBOT_HEADER_HEIGHT);
    assert_eq!(l.position_mode, "flex_flow");
    assert!(!l.overlaps_timeline && l.header_rect.y + l.header_rect.h <= l.timeline_rect.y);
    assert_eq!(l.title_btn_height_px, 28.0);
    assert_eq!(l.title_click_target_pane, "assistant_settings");
    assert!(l.tab_shape_visible && l.thread_tabs_count == 2);
    assert_eq!(l.model_picker_fit_mode, HeaderModelPickerFitMode::Full);

    let mut p_mid = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    p_mid.available_width_px = 620.0;
    assert_eq!(compute_openbot_header_layout(&p_mid, &dark).model_picker_fit_mode, HeaderModelPickerFitMode::Name);
    let mut p_tight = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    p_tight.available_width_px = 424.0;
    assert_eq!(compute_openbot_header_layout(&p_tight, &dark).model_picker_fit_mode, HeaderModelPickerFitMode::Icon);

    let mut p_wg = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    p_wg.window_glass = true;
    let l_wg = compute_openbot_header_layout(&p_wg, &dark);
    assert!(!l_wg.tab_shape_visible && l_wg.tab_disabled_by_motion_or_glass);
    let mut p_rm = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    p_rm.reduced_motion = true;
    assert_eq!(compute_openbot_header_layout(&p_rm, &dark).tab_transition_ms, 0);

    let mut p_grp = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    p_grp.is_bot_chat = false;
    p_grp.is_group_chat = true;
    let l_grp = compute_openbot_header_layout(&p_grp, &dark);
    assert_eq!(l_grp.title_click_target_pane, "channel_settings");
    assert!(!l_grp.model_picker_visible);
    p_grp.is_archived = true;
    assert!(compute_openbot_header_layout(&p_grp, &dark).title_disabled);

    let mut p_closed = openbot_uiux_probe::f09_f10::default_header_props(&stage.thread_tabs);
    p_closed.sidebar_closed = true;
    p_closed.topbar_controls_right_edge_px = 44.0;
    assert!(compute_openbot_header_layout(&p_closed, &dark).header_rect.x >= 44.0);
}

#[test]
fn ui_f10_chat_stream_preview_surrogates_error_quarantine_and_boundary() {
    let dark = openbot_uiux_probe::dark_palette();
    let grp = BubbleGroupLayout {
        attached_to_prev: false, attached_to_next: false, show_sender_name: false,
        show_bottom_avatar: true, show_tail: false, reserve_avatar_column: true,
    };
    let mk_msg = |id: u64, out: bool, text: &str| MessageItemModel {
        id, sender_id: if out { 1 } else { 2 },
        sender_name: if out { "You".into() } else { "Bot".into() },
        avatar_initials: "B".into(), avatar_color: 0x3B93F0,
        direction: if out { MessageDirection::Outgoing } else { MessageDirection::Incoming },
        timestamp: "10:42".into(), delivery_status: DeliveryStatus::Read,
        reply_to: None, reactions: vec![],
        content: MessageContent::Text { text: text.into(), is_edited: false },
        bot_keyboard: None, is_pinned: false,
    };
    let b_out = compute_openbot_bubble_layout(&mk_msg(1, true, "Hello"), grp, false, false, &dark, "en");
    assert_eq!(b_out.bg_hex, "#242424");
    assert!(!b_out.uses_legacy_blue);
    assert_eq!((b_out.entry_gap_px, b_out.grouped_gap_px), (OPENBOT_ENTRY_GAP_PX, OPENBOT_GROUPED_GAP_PX));

    let m_err = mk_msg(2, false, r#"{"error":"rate_limit","status":429}"#);
    let b_err_c = compute_openbot_bubble_layout(&m_err, grp, false, false, &dark, "en");
    let b_err_o = compute_openbot_bubble_layout(&m_err, grp, false, true, &dark, "en");
    assert!(b_err_c.structured_error_quarantined && !b_err_c.contains_raw_json_wall);
    assert_eq!(b_err_c.error_drawer_btn_label, "Show error");
    assert_eq!(b_err_o.error_drawer_btn_label, "Hide error");
    assert!(extract_structured_error_payload(r#"{"error":"rate_limit","status":429}"#).is_some());
    assert!(extract_structured_error_payload("Hello!").is_none());

    assert!(!compute_agent_activity_layout(true, 2, false, false).elapsed_visible);
    let act6 = compute_agent_activity_layout(true, 6, false, false);
    assert!(act6.elapsed_visible && act6.bubble_less && act6.shimmer_active);
    assert!(compute_agent_activity_layout(true, 61, true, false).elapsed_visible);
    let act_rm = compute_agent_activity_layout(true, 8, false, true);
    assert!(act_rm.shimmer_frozen_reduced_motion && !act_rm.shimmer_active);

    let p20k = compute_message_preview(&"a".repeat(20_000), false, false);
    assert!(!p20k.collapsed && !p20k.show_more_available);
    let p20k1 = compute_message_preview(&format!("{}🚀{}", "a".repeat(2999), "b".repeat(17_001)), false, false);
    assert!(p20k1.collapsed && p20k1.show_more_available && p20k1.surrogate_valid && p20k1.preview_utf16_len <= 3_000);

    let mut conv = ConversationViewState::new("a1", "Bot", "seed");
    for (id, author, text) in [("m1", "user", "hi"), ("m2", "assistant", "ok")] {
        let cm: ConversationMessage = serde_json::from_value(json!({"id": id, "author": author, "text": text, "createdAt": "10:00", "status": "completed"})).unwrap();
        let row = ChatTimelineRow { message: cm, timestamp_ms: 1000, corrupt_override: None, steps: None, ui_block_card: None, runtime_cards: vec![], voice_player: None };
        assert!(!ChatRowBoundary::check_row(&row).corrupt);
        conv.rows.push(row);
    }
    let eval = conv.evaluate_row_boundaries(true);
    assert!(eval.row_boundary_isolated && eval.isolated_corrupt_rows == 1 && eval.sibling_rows_rendered == 2);

    let mut dm = DirectDmState { peer_online: true, ..Default::default() };
    dm.on_text_input("Hello!", 1000);
    let snap_act = dm.snapshot("Hello!");
    assert!(snap_act.typing_active && !snap_act.send_disabled);
    dm.tick(5000);
    assert!(!dm.snapshot("Hello!").typing_active);
    assert!(dm.snapshot("   ").send_disabled);

    assert_eq!(UI_BLOCK_CARD_MAX_WIDTH_PX, 540.0);
    assert_eq!(AGENT_EMPTY_STATE_HEADING_PX, 32.0);
    let ui_spec = json!({"version": 1, "type": "confirm", "title": "Deploy", "actions": [{"id": "confirm", "label": "Confirm"}]});
    let ui_eval = validate_and_evaluate_ui_block(&ui_spec, Some(&json!({"actionId": "confirm"})), None).unwrap();
    assert!(ui_eval.sanitized_response.is_some());
}

#[test]
fn ui_f10_f12_f13_defect_steps_add_menu_and_currency_trigger() {
    assert!(!AgentStepsState::default().expanded, "AgentSteps must default to collapsed");
    assert_eq!(ComposerAddMenuItem::ALL.len(), 6);
    assert!(ComposerAddMenuItem::ALL.contains(&ComposerAddMenuItem::VoiceConversation));
    assert!(detect_autocomplete_trigger("$100", 4).is_none());
    assert!(detect_autocomplete_trigger("price is $49.99", 15).is_none());
    assert!(detect_autocomplete_trigger("user@example.com", 16).is_none());
    assert!(detect_autocomplete_trigger("$sk", 3).is_some());
    assert!(detect_autocomplete_trigger("@bot", 4).is_some());
}

#[test]
fn ui_f11_channel_avatar_clusters_routing_and_editor_validation() {
    let mk_members = |n: usize| -> Vec<ChannelMember> {
        (0..n).map(|i| ChannelMember { agent_id: format!("a-{i}") }).collect()
    };
    let e1 = resolve_channel_cluster(&mk_members(1), ChannelAvatarLayout::Cluster, None, None, false);
    assert_eq!((e1.shown_faces, e1.hidden_count, e1.overflow_pill_tone), (1, 0, ChannelOverflowPillTone::None));
    let e4 = resolve_channel_cluster(&mk_members(4), ChannelAvatarLayout::Cluster, None, None, false);
    assert_eq!((e4.shown_faces, e4.hidden_count, e4.overflow_pill_tone), (4, 0, ChannelOverflowPillTone::None));
    let e5 = resolve_channel_cluster(&mk_members(5), ChannelAvatarLayout::Cluster, None, None, false);
    assert_eq!((e5.shown_faces, e5.hidden_count, e5.overflow_pill_tone), (3, 2, ChannelOverflowPillTone::Lilac));
    let e13 = resolve_channel_cluster(&mk_members(13), ChannelAvatarLayout::Cluster, None, None, false);
    assert_eq!((e13.shown_faces, e13.hidden_count, e13.overflow_pill_tone), (2, 11, ChannelOverflowPillTone::Amber));

    let sp = ChannelIcon { name: "sparkles".into(), hue: 215 };
    assert_eq!(resolve_channel_cluster(&mk_members(3), ChannelAvatarLayout::Row, None, None, false).mode, ChannelClusterMode::MemberFaces);
    assert_eq!(resolve_channel_cluster(&mk_members(3), ChannelAvatarLayout::Cluster, Some(&sp), None, false).mode, ChannelClusterMode::IconTile);
    assert_eq!(resolve_channel_cluster(&mk_members(3), ChannelAvatarLayout::Cluster, Some(&sp), Some("https://cdn.local/grp.png"), false).mode, ChannelClusterMode::CustomImage);
    let broken = resolve_channel_cluster(&mk_members(3), ChannelAvatarLayout::Cluster, Some(&sp), Some("/nonexistent/openbot_404.png"), false);
    assert!(broken.image_failed && broken.mode == ChannelClusterMode::IconTile);
    assert!(resolve_channel_icon("dragon", Some(999)).is_none());
    assert_eq!(resolve_channel_icon("code", Some(999)).map(|(_, h)| h.degrees()), Some(215));

    let mut act = ChannelActivityIndicatorState::default();
    act.sync_workers(&[], 0, false);
    assert!(!act.snapshot(0).visible);
    let workers = vec![
        ChannelWorkerBrief { id: "a".into(), name: "A".into(), avatar_seed: None, avatar_hue: None, avatar_color: None, avatar_url: None },
        ChannelWorkerBrief { id: "b".into(), name: "B".into(), avatar_seed: None, avatar_hue: None, avatar_color: None, avatar_url: None },
    ];
    act.sync_workers(&workers, 0, false);
    let snap = act.snapshot(6_000);
    assert!(snap.visible && snap.worker_count == 2 && snap.shimmer_animated);
    act.sync_workers(&workers, 0, true);
    assert!(!act.snapshot(6_000).shimmer_animated);

    let merged = merge_consecutive_routing_events(&[
        ChannelRoutingEvent { action: "asked".into(), agent_id: "bot-a".into() },
        ChannelRoutingEvent { action: "asked".into(), agent_id: "bot-b".into() },
    ]);
    assert_eq!(merged.len(), 1);
    let mut hl = HistoryViewHighlightManager::new();
    hl.highlight_openbot(SelectedQuote { message_id: 10, ..Default::default() }, 1000);
    assert_eq!(hl.active_flash_duration_ms(), OPENBOT_REPLY_FLASH_DURATION_MS);

    let mut ch_state = ChannelConversationState::new("c1", "Squad", vec!["a".into()], Some("a".into()));
    assert_eq!(ch_state.resolve_author_label("deleted-bot"), "Unavailable agent");
    assert!(!ch_state.mark_unread_enabled && !ch_state.thread_branch_enabled);
    ch_state.set_archived(true);
    assert_eq!(ch_state.context_menu_items(), vec!["Restore"]);

    let mut ed = ChannelEditorState::new_create();
    ed.name = "Empty".into();
    assert!(matches!(ed.validate_draft(), Err(ChannelEditorValidationError::EmptyMembers)));
    let _ = ed.add_member("bot-a");
    ed.name = "x".repeat(81);
    assert!(matches!(ed.validate_draft(), Err(ChannelEditorValidationError::NameTooLong { .. })));
    ed.name = "Squad".into();
    let _ = ed.add_member("bot-b");
    assert!(ed.create_channel_from_editor().is_ok());
}

#[test]
fn ui_f12_composer_pill_paste_attachments_text_only_and_ime() {
    let dark = openbot_uiux_probe::dark_palette();
    let l_idle = compute_openbot_composer_strip_layout("", 24.0, 0, false, false, false, true, &dark);
    assert!(l_idle.compact && l_idle.height_px == 48.0 && l_idle.border_radius_px == 24.0 && l_idle.dictation_bars == 12);
    assert!(!l_idle.legacy_chip_strip_present && !l_idle.detached_slash_present && !l_idle.focus_border_is_telegram_blue && !l_idle.send_btn_is_telegram_blue);
    assert_eq!(l_idle.action_circle_kind, "voice");

    let l_multi = compute_openbot_composer_strip_layout("a\nb", 64.0, 0, false, false, false, true, &dark);
    assert!(!l_multi.compact && l_multi.min_height_px == 96.0 && l_multi.border_radius_px == 14.0 && l_multi.action_circle_kind == "send");
    assert_eq!(compute_openbot_composer_strip_layout("", 24.0, 0, true, false, false, true, &dark).action_circle_kind, "stop");

    let legacy_pal = TdesktopThemePalette::build(TdesktopThemeMode::Classic, 0x3390EC, 14, 16);
    let l_leg = compute_openbot_composer_strip_layout("", 24.0, 0, false, false, false, true, &legacy_pal);
    assert!(l_leg.legacy_chip_strip_present && l_leg.focus_border_is_telegram_blue);

    let mut c = ComposerState::new(false);
    assert!(!c.try_toggle_add_menu(true) && !c.add_menu_open);
    let r4000 = c.paste_text(&"a".repeat(4000));
    assert!(!r4000.pasted_as_file && r4000.inline_text_length == 4000);
    let mut c2 = ComposerState::new(false);
    let r4001 = c2.paste_text(&"a".repeat(4001));
    assert!(r4001.pasted_as_file && r4001.pasted_file_name.is_some());

    let mut c_cap = ComposerState::new(false);
    for i in 0..10 {
        assert!(c_cap.add_attachment(ComposerAttachmentDraft::from_raw(format!("f{i}.txt"), 1024, "file"), false).is_ok());
    }
    assert!(c_cap.add_attachment(ComposerAttachmentDraft::from_raw("f11.txt", 1024, "file"), false).is_err());
    assert_eq!(c_cap.attachments.len(), 10);

    let mut c_img = ComposerState::new(false);
    c_img.set_custom_pi_text_only(true);
    let _ = c_img.add_attachment(ComposerAttachmentDraft::from_raw("shot.png", 2048, "image"), false);
    assert!(c_img.image_warning_banner_visible());
    c_img.click_allow_images();
    assert!(c_img.image_inputs_enabled && !c_img.image_warning_banner_visible());

    let mut cmp = CanonicalComposer::new();
    cmp.ime_machine.on_ime_start();
    cmp.ime_machine.on_ime_update("你好", 6);
    cmp.ime_machine.on_ime_commit("你好");
    assert_ne!(cmp.ime_machine.handle_enter_key(false), KeyAction::SendMessage);
    cmp.ime_machine.last_commit_at = Instant::now().checked_sub(Duration::from_millis(200));
    assert_eq!(cmp.ime_machine.handle_enter_key(false), KeyAction::SendMessage);
    cmp.ime_machine.on_ime_start();
    cmp.ime_machine.on_ime_cancel();
    assert!(!matches!(cmp.ime_machine.state, ImeState::Composing { .. }));
}

#[test]
fn ui_f13_at_and_dollar_pickers_nfd_and_atomic_chip_delete() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut c = ComposerState::new(true);
    c.available_agents = vec![("a1".into(), "Éclair".into()), ("a2".into(), "Researcher".into())];
    c.open_picker(ComposerTrigger::At, "eclair");
    assert!(c.resolve_active_at_picker().agents.iter().any(|a| a.name == "Éclair"));
    c.open_picker(ComposerTrigger::At, "every");
    assert!(c.resolve_active_at_picker().everyone_shown);

    let mut c_1to1 = ComposerState::new(false);
    c_1to1.available_agents = c.available_agents.clone();
    c_1to1.open_picker(ComposerTrigger::At, "every");
    assert!(!c_1to1.resolve_active_at_picker().everyone_shown);

    c.open_picker(ComposerTrigger::At, "ab");
    assert_eq!(c.resolve_active_at_picker().at_sections, vec!["agents", "files", "tabs", "threads"]);
    c.open_picker(ComposerTrigger::At, "abc");
    let r3 = c.resolve_active_at_picker();
    assert!(r3.at_sections.iter().any(|s| s == "messages") && r3.messages_debounce_ms == 150);

    let mut c_dol = ComposerState::new(false);
    c_dol.available_skills = vec![
        ComposerSkillEntry { slug: "web-search".into(), name: "Search".into(), status: "ok".into() },
        ComposerSkillEntry { slug: "broken-tool".into(), name: "Broken".into(), status: "broken".into() },
    ];
    c_dol.open_picker(ComposerTrigger::Dollar, "");
    let dol_res = c_dol.resolve_active_dollar_picker();
    assert!(!dol_res.skill_problems.is_empty());
    assert!(dol_res.dollar_items.iter().all(|i| i.id != "broken-tool"));

    c.accept_at_everyone();
    assert!(c.active_trigger.is_none());
    c_dol.accept_dollar_item(&DollarPickerItem {
        kind: DollarPickerItemKind::Skill, id: "web-search".into(), name: "Search".into(),
        display_label: "Search".into(), slug: Some("web-search".into()), rank: 0,
    });
    assert!(c_dol.active_trigger.is_none());
    let pl = compute_openbot_picker_popup_layout(true, '@', 12, 3, &dark);
    assert_eq!((pl.bg_hex, pl.max_visible_rows), ("#121212", 7));

    let mut c_chip = ComposerState::new(true);
    c_chip.insert_tag_token(ChatTagKind::Agent, "Researcher", "a2");
    let chips = extract_composer_inline_chips(&c_chip.text, &dark);
    assert_eq!(chips.len(), 1);
    assert_eq!((chips[0].height_px, chips[0].radius_px, chips[0].bg_hex.as_str()), (24.0, 6.0, "#262626"));
    assert_eq!(chat_tag_references(&c_chip.text).len(), 1);
    let (s, e) = atomic_reference_chip_span_before_cursor(&c_chip.text, c_chip.text.len()).expect("atomic span");
    let after_bs = format!("{}{}", &c_chip.text[..s], &c_chip.text[e..]);
    assert!(chat_tag_references(&after_bs).is_empty());
    assert_eq!(extract_rendered_message_reference_chips("Check @[Researcher](agent:a2) and $skill:web-search and @everyone", &dark).len(), 3);
    let unav = compute_openbot_reference_chip_layout("agent", "gone", "Deleted", "@[Deleted](agent:gone)", true, &dark);
    assert!(!unav.try_navigate_on_click());
}

#[test]
fn ui_f14_composer_stack_queue_steer_awaiting_and_stopped() {
    let idle = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    assert!(!idle.visible && idle.height_px == 0.0);

    let ten: Vec<String> = (1..=10).map(|i| format!("Item {i}")).collect();
    let long_notice = ComposerNoticeState { title: Some("Err".into()), body: "x".repeat(320), reference: Some("r1".into()), dismissible: true };
    let active = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &ten, queue_can_steer: true, reply_sender: Some("BotA"), awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 25, notice: Some(&long_notice),
    });
    assert!(active.visible && active.overlap_bottom_px == 24.0 && active.z_index == 10 && active.composer_z_index == 11);
    assert_eq!((active.queue_row_height_px, active.queue_visible_rows), (29.0, 7));
    assert!(active.reply_preview_visible && active.reply_preview_min_height_px == 42.0);
    assert_eq!(active.open_waits_count, 20);
    assert!(active.notice_visible && active.notice_has_copy_details && active.notice_truncated);

    let mut qp = QueuePanelState::default();
    let _ = qp.push_item("d1", "M1", QueueSender::User);
    let _ = qp.push_item("d2", "M2", QueueSender::User);
    let _ = qp.push_item("d3", "M3", QueueSender::User);
    let _ = qp.request_delete("d1", true);
    let _ = qp.reorder(1, 0);
    let _ = qp.begin_edit_at("d2", 100);
    let _ = qp.save_edit("M2-edited");
    let texts: Vec<&str> = qp.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, vec!["M3", "M2-edited"]);

    let st_ok = evaluate_steer(false, true);
    assert!(st_ok.can_steer && st_ok.steered_on_tab && !st_ok.steer_refused);
    let st_no = evaluate_steer(true, true);
    assert!(!st_no.can_steer && !st_no.steered_on_tab && st_no.steer_refused);

    let mut aw = AwaitingRepliesTracker::default();
    aw.upsert_entry(AwaitingReplyEntry { agent_id: "a1".into(), agent_name: "BotA".into(), state: AwaitingReplyState::Working, preview: None, delivery_id: None, waiting_reason: None });
    assert!(!aw.all_settled());
    aw.entries[0].state = AwaitingReplyState::Replied;
    assert!(aw.all_settled());

    let mut stopped = ChannelStoppedTasksState::new(vec![StoppedChannelTaskBannerItem {
        task_id: "t1".into(), owner_agent_id: Some("a1".into()), error: Some("paused".into()),
        run_agent_ids: vec!["a1".into()], is_assignment_loop: false,
    }]);
    assert!(stopped.is_visible(false) && !stopped.is_visible(true));
    assert!(stopped.begin_resume("t1"));
    assert!(!stopped.begin_resume("t1"));
    assert_eq!(stopped.resume_dispatch_count, 1);
}

#[test]
fn ui_f15_side_panel_launcher_clamping_splits_and_side_question() {
    let dark = openbot_uiux_probe::dark_palette();
    let wiring = default_openbot_shell_wiring();
    let mut sp = OpenBotRightSidePanelState::default();
    sp.return_to_launcher();
    let l = sp.compute_layout(1108.0, 640.0);
    assert!(l.launcher_visible);
    let slot = resolve_right_drawer_slot(
        wiring, true, false, dark.is_apple_style(), l.open, l.maximized, l.width_px,
        sp.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant,
    );
    assert_eq!(slot.active_right_drawer_count, 1);
    assert!(!slot.mount_legacy_standalone_inspector && !slot.mount_legacy_companion_panel);

    assert_eq!(SidePanelPaneKind::Settings.launcher_label(SidePanelSettingsTarget::Assistant, true), "打开 助手设置");
    assert_eq!(SidePanelPaneKind::Settings.launcher_label(SidePanelSettingsTarget::Channel, true), "打开 群聊设置");
    assert_eq!(SidePanelPaneKind::Settings.launcher_label(SidePanelSettingsTarget::Contact, true), "打开 联系人资料");

    assert_eq!(sp.clamp_width_for_workspace(200.0, 948.0), 360.0);
    let w_hi = sp.clamp_width_for_workspace(2200.0, 948.0);
    assert!(948.0 - w_hi >= CONVERSATION_PANEL_MIN);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let w_req = sp.clamp_width_for_workspace(bad, 948.0);
        assert!(w_req.is_finite() && (SETTINGS_PANEL_DEFAULT..=SETTINGS_PANEL_MAX).contains(&w_req));
        let w_vp = sp.clamp_width_for_workspace(480.0, bad);
        assert!(w_vp.is_finite() && w_vp == SETTINGS_PANEL_DEFAULT);
        let slot_bad = resolve_right_drawer_slot(
            wiring, true, false, dark.is_apple_style(), true, false, bad,
            sp.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant,
        );
        assert!(slot_bad.slot_width_px.is_finite() && slot_bad.slot_width_px == SETTINGS_PANEL_DEFAULT);
    }
    let slot_huge = resolve_right_drawer_slot(
        wiring, true, false, dark.is_apple_style(), true, false, 99_999.0,
        sp.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant,
    );
    assert_eq!(slot_huge.slot_width_px, SETTINGS_PANEL_MAX);

    sp.open_pane(SidePanelPaneKind::Settings);
    sp.state.toggle_maximize();
    assert_eq!(sp.compute_layout(1108.0, 640.0).effective_width_pct, 100);
    sp.close_tab(SidePanelPaneKind::Settings);
    let l_after_close = sp.compute_layout(1108.0, 640.0);
    assert!(!l_after_close.maximized && l_after_close.launcher_visible);

    let mut sp_s = OpenBotRightSidePanelState::default();
    sp_s.open_pane(SidePanelPaneKind::Browser);
    sp_s.split_down(SidePanelPaneKind::Question);
    assert_eq!(sp_s.state.active_split_ids().len(), MAX_SIDE_SPLITS);
    assert!(!sp_s.try_split_down(SidePanelPaneKind::Threads));

    let mut sp_ov = OpenBotRightSidePanelState::default();
    for p in SidePanelPaneKind::ALL {
        sp_ov.open_pane(p);
    }
    let h_ov = sp_ov.compute_header_layout(SidePanelSettingsTarget::Assistant, 360.0, true);
    assert!(h_ov.overflow_chevron_visible && h_ov.hidden_tabs_count > 0 && h_ov.combined_actions_menu);
    assert!(sp_ov.sync_file_tab_path_availability("files:gone", std::path::Path::new("/nonexistent/openbot_deleted.md")));
    assert!(sp_ov.compute_header_layout(SidePanelSettingsTarget::Assistant, 360.0, true).unavailable_tab_flagged);

    let stage = ConversationStage::new();
    let msg_before = stage.messages.len();
    for i in 0..SIDE_QUESTION_MAX_TURNS {
        assert!(sp_s.state.question.append_turn(format!("Q{i}"), format!("A{i}")).is_ok());
    }
    sp_s.state.question.draft_question = "Draft 21".into();
    assert!(sp_s.state.question.append_turn("Draft 21", "A21").is_err());
    assert_eq!(sp_s.state.question.draft_question, "Draft 21");
    assert_eq!(stage.messages.len().saturating_sub(msg_before), 0);
}

#[test]
fn ui_f16_agent_settings_profile_and_context_inspector_embed() {
    let dark = openbot_uiux_probe::dark_palette();
    let mk_c = |id: &str, kind_code: &'static str| BotPublishedCreationSummary {
        creation_id: id.into(), title: format!("Sample {id}"),
        kind_label: kind_code.into(), status_label: "Published".into(), kind_code,
    };
    let mut insp = ContextInspector::new();
    insp.sync_inspector_for_unified_side_panel(true);
    insp.profile.is_bot = true;
    insp.profile.name = "Research Assistant".into();
    insp.profile.username = "research_bot".into();
    insp.published_creations = vec![mk_c("b1", "book"), mk_c("b2", "book"), mk_c("m1", "music"), mk_c("g1", "game")];
    assert_eq!(insp.resolve_context_inspector_render_route(), ContextInspectorRenderRoute::OpenBotBotSettingsRoot);
    assert!(insp.trigger_copy_bot_handle().is_some());

    let l = compute_openbot_bot_settings_layout(&insp, &dark);
    assert!(l.mounted && l.host == "side_panel_assistant_settings" && l.hero_avatar_px == 62.0 && l.avatar_editor_width_px == 284.0);
    assert_eq!(l.avatar_shapes_count, SUPPORTED_AVATAR_SILHOUETTES.len());
    assert_eq!(l.avatar_eyes_count, AVATAR_EXPRESSIONS.len());
    assert_eq!(l.avatar_editor_total_cells, 18);
    assert_eq!((l.inset_card_bg_hex.as_str(), l.inset_card_radius_px, l.inset_row_height_px), ("#212121", 8.0, 40.0));
    assert_eq!(l.creations_counts.books, 2);
    assert!(l.context_inspector_live_reachable && !l.delete_history_disabled);

    insp.open_bot_subpage(OpenBotSettingsSubpage::Memories);
    assert_eq!(compute_openbot_bot_settings_layout(&insp, &dark).active_subpage, Some("memories"));
    insp.openbot_subpage = None;
    assert_eq!(insp.resolve_context_inspector_render_route(), ContextInspectorRenderRoute::OpenBotBotSettingsRoot);

    insp.bot_access_level = "workspace".into();
    assert!(ContextInspector::requires_permission_confirm("workspace", "full"));
    assert!(!ContextInspector::requires_permission_confirm("full", "workspace"));
    assert!(insp.request_bot_access_level_change("full"));
    assert_eq!(insp.bot_access_level, "workspace");
    insp.confirm_pending_access_level_change();
    assert_eq!(insp.bot_access_level, "full");
    assert!(!insp.request_bot_access_level_change("workspace"));
    assert_eq!(insp.bot_access_level, "workspace");

    insp.bot_is_busy = true;
    let busy_out = insp.try_start_new_chat_from_settings();
    assert!(busy_out.new_chat_busy_toast && !busy_out.chat_reset);
    assert!(!insp.try_delete_bot_history_from_settings());
    assert!(compute_openbot_bot_settings_layout(&insp, &dark).delete_history_disabled);
    insp.bot_is_busy = false;
    assert!(insp.try_delete_bot_history_from_settings());

    assert!(validate_bot_profile_name("").is_err());
    assert!(validate_bot_profile_name(&"x".repeat(81)).is_err());
    assert!(validate_bot_profile_name("Research Assistant").is_ok());

    let mut sw = BotIdentityAutosaveState::new("bot-a", "Alpha", "Bio A");
    assert_eq!(sw.debounce_ms, BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS);
    sw.edit_name("Alpha-v2", 100);
    assert!(sw.flush_before_agent_switch("bot-b", "Beta", "Bio B"));
    assert_eq!(sw.last_flushed_agent_id.as_deref(), Some("bot-a"));
    assert_eq!(sw.committed_name, "Beta");
    assert!(!sw.dirty);
    sw.on_save_failed("err");
    assert!(sw.save_error.is_some() && sw.committed_name == "Beta");
}

#[test]
fn ui_f17_channel_settings_picture_editor_and_contact_profile() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut ch = ContextInspector::new();
    ch.sync_inspector_for_unified_side_panel(true);
    ch.members.is_group_or_channel = true;
    ch.channel_context_isolated = true;
    let ch_l = compute_openbot_channel_settings_layout(&ch);
    assert!(ch_l.mounted && ch_l.avatar_px == 62.0 && ch_l.icon_catalog_count >= 20 && ch_l.icon_hues_count == 10 && !ch_l.read_only && ch_l.context_actions_visible && !ch_l.compact_disabled);
    ch.channel_provider_supports_compact = false;
    assert!(compute_openbot_channel_settings_layout(&ch).compact_disabled);
    ch.channel_archived = true;
    let ch_arch = compute_openbot_channel_settings_layout(&ch);
    assert!(ch_arch.read_only && !ch_arch.context_actions_visible);

    let mut pic = ChannelPictureEditorState::new(Some(ChannelIcon { name: "code".into(), hue: 215 }), None);
    assert!(pic.can_reset());
    pic.reset();
    assert!(!pic.can_reset());

    let empty_tabs: Vec<(String, String)> = vec![];
    let mut hdr_p = openbot_uiux_probe::f09_f10::default_header_props(&empty_tabs);
    hdr_p.is_isolated_lock = ch_l.context_isolated;
    assert!(compute_openbot_header_layout(&hdr_p, &dark).isolation_lock_visible);

    let mut ed100 = ChannelEditorState::new_create();
    for i in 0..100 {
        assert!(ed100.add_member(format!("m-{i}")).is_ok());
    }
    assert!(matches!(ed100.add_member("m-100"), Err(ChannelEditorValidationError::TooManyMembers { max: 100 })));
    assert!(matches!(ed100.attempt_member_drag_reorder(0, 1), Err(ChannelEditorValidationError::MemberDragReorderUnsupported)));

    let mut ed_lead = ChannelEditorState::new_create();
    let _ = ed_lead.add_member("lead-a");
    let _ = ed_lead.add_member("bot-b");
    assert!(ed_lead.remove_member("lead-a"));
    assert_eq!(ed_lead.lead_agent_id.as_deref(), Some("bot-b"));
    assert!(!ed_lead.remove_member("bot-b"));
    assert_eq!(ed_lead.members, vec!["bot-b".to_string()]);

    let mut hum = ContextInspector::new();
    hum.sync_inspector_for_unified_side_panel(true);
    hum.profile.is_bot = false;
    hum.members.is_group_or_channel = false;
    hum.profile.username = "@alice".into();
    hum.profile.status = "online".into();
    hum.pending_confirm_guard = Some(OpenBotConfirmGuard::ContactBlock);
    let cp = compute_openbot_contact_profile_layout(&hum, &dark);
    assert!(cp.mounted && cp.host == "side_panel_contact_profile" && cp.handle == "@alice" && cp.online && cp.block_confirm_open && cp.destructive_confirm_gated);
}

#[test]
fn ui_f18_content_hubs_no_match_reader_and_publish_xss_guard() {
    let dark = openbot_uiux_probe::dark_palette();
    for page in ["books", "music", "games", "app_store", "my_creations"] {
        let s = compute_content_hub_shell_layout(&dark, 1440.0, page);
        assert_eq!((s.page_header_height_px, s.left_nav_width_px, s.content_max_width_px, s.legacy_blue_pill_count), (38.0, 280.0, 880.0, 0));
    }
    let comm = compute_community_hub_layout(&dark, 1440.0);
    assert_eq!((comm.page_header_height_px, comm.left_nav_width_px, comm.content_max_width_px, comm.legacy_blue_pill_count), (38.0, 280.0, 880.0, 0));

    assert!(!MarketplaceNoMatchPlushState::new(false).reduced_motion);
    assert_eq!(MarketplaceNoMatchPlushState::VIDEO_ASSET_NAME, "fishing.webm");
    assert!(MarketplaceNoMatchPlushState::new(true).reduced_motion);
    assert_eq!(MarketplaceNoMatchPlushState::STILL_ASSET_NAME, "fishing-still.webp");

    let bp = BookPayload {
        author: "OpenBot Studio".into(), series_name: None, series_index: None,
        blurb: "Comprehensive guide to responsive OpenBot desktop workspaces.".into(),
        estimated_read_mins: 5, word_count: 500,
        chapters: vec![BookChapter {
            chapter_id: "ch1".into(), chapter_number: 1, title: "Ch 1".into(), subtitle: None,
            pages: vec![
                BookPage { page_number: 1, blocks: vec![BookRichBlock::Paragraph { text: "Page one text with enough substance for validation.".into() }] },
                BookPage { page_number: 2, blocks: vec![BookRichBlock::Paragraph { text: "Page two text with enough substance for validation.".into() }] },
            ],
        }],
    };
    let book_dto = sample_creation_dto("b1", CreationKind::Book, "OpenBot Guide", "Clean summary for hub validation.", CreationPayload::Book(bp.clone()));
    let mut reader = BookReaderState::open(&book_dto, bp.clone(), None, dark);
    let _ = reader.next_page();
    assert!(reader.current_global_page_index() >= 1);

    let mut music = MusicTransportEngine::new();
    assert!(music.play().is_err());
    music.set_queue(vec![MusicTrackEntry {
        track_id: "t1".into(), creation_id: None, title: "Felt".into(), artist: "OpenBot".into(),
        album: "Studio".into(), duration_ms: 120_000, sample_rate: 44_100, channels: 2,
        pcm_samples: vec![], wav_bytes: vec![], waveform_bars: vec![], waveform_5bit: vec![], lyrics: vec![],
    }]);
    assert!(music.play().is_ok());
    let _ = music.tick_ms(12_000);
    assert_eq!(music.snapshot().state, MusicPlaybackState::Playing);

    let gp = GamePayload {
        developer: "OpenBot".into(), category: GameCategory::Trivia, controls_hint: "Pick".into(),
        rules_markdown: "Answer".into(), engine_kind: GameEngineKind::TriviaQuiz,
        supports_leaderboard: true, estimated_session_mins: 2,
        spec: GameSpec::TriviaQuiz {
            questions: vec![TriviaQuestion {
                question_id: "q1".into(), prompt: "Rail?".into(), choices: vec!["52px".into(), "120px".into()],
                correct_index: 0, explanation: None, points: 100,
            }],
            time_limit_secs: Some(60),
        },
        achievements: vec![],
    };
    let g_dto = sample_creation_dto("g1", CreationKind::Game, "Quiz", "Clean quiz summary.", CreationPayload::Game(gp.clone()));
    let mut sess = ActiveGameSession::launch(&g_dto, &gp, None, 42);
    assert!(sess.apply_input(GameInput::SelectOption(0)).score > 0);

    let mut pub_ok = PublishReviewModalState::default();
    pub_ok.open_for_creation(book_dto);
    pub_ok.recompute_validation();
    assert!(pub_ok.validation_errors.is_empty());

    let mut pub_xss = PublishReviewModalState::default();
    pub_xss.open_for_creation(sample_creation_dto(
        "xss", CreationKind::Book, "<script>alert(1)</script>", "javascript:alert(1)", CreationPayload::Book(bp),
    ));
    pub_xss.recompute_validation();
    assert!(!pub_xss.validation_errors.is_empty());
}

#[test]
fn ui_f19_settings_search_shortcuts_matrix_and_modals() {
    let dark = openbot_uiux_probe::dark_palette();
    let mut router = SettingsRouter::new(dark, true, AppPreferences::default());
    router.nav_search_query = "chat".into();
    let sp = compute_settings_page_layout(&router);
    assert_eq!((sp.left_nav_px, sp.content_max_px, sp.card_bg_hex.as_str(), sp.card_radius_px, sp.row_min_height_px), (220.0, 960.0, "#1e1e1e", 12.0, 64.0));
    assert!(sp.search_results_count > 0);
    router.nav_search_query = "zzz_impossible_xyz".into();
    assert_eq!(compute_settings_page_layout(&router).search_results_count, 0);
    router.nav_search_query = "chat".into();
    let _ = router.handle_escape_key();
    assert!(router.is_open && router.nav_search_query.is_empty());
    let _ = router.handle_escape_key();
    assert!(!router.is_open);

    let mut toaster = OpenBotToasterState::new(false);
    router.save_bar.mark_dirty();
    assert!(router.save_bar.is_visible());
    router.save_bar.commit_save(&mut toaster);
    assert!(!router.save_bar.is_visible());
    let tl = compute_toaster_layout(&toaster);
    assert_eq!(tl.z_index, 120);
    assert!(tl.toasts.iter().any(|t| t.tone == OpenBotToastTone::Success && t.message == "Saved"));

    let mut cd = ConfirmDialogState::open_dialog(true);
    let cdl = compute_confirm_dialog_layout(&cd, &dark);
    assert_eq!((cdl.max_width_px, cdl.radius_px, cdl.bg_hex.as_str(), cdl.initial_focus), (460.0, 16.0, "#161616", "cancel"));
    cd.set_pending(true);
    assert!(!cd.try_dismiss_backdrop() && cd.backdrop_dismiss_blocked());
    cd.resolve_and_restore_focus();
    assert!(!cd.open && cd.focus_restored_to_opener);

    let am_base = compute_action_menu_layout(true, ACTION_MENU_DEFAULT_WIDTH_PX, 1440.0, 900.0, &dark);
    assert!(am_base.z_index == 40 && am_base.radius_px == 14.0 && am_base.within_viewport);
    let mut am = ActionMenuState::open_at(
        1400.0,
        880.0,
        ACTION_MENU_DEFAULT_WIDTH_PX,
        vec![
            ActionMenuItem { id: "copy".into(), label: "Copy".into(), disabled: false, destructive: false },
            ActionMenuItem { id: "delete".into(), label: "Delete".into(), disabled: true, destructive: true },
            ActionMenuItem { id: "archive".into(), label: "Archive".into(), disabled: false, destructive: false },
        ],
    );
    assert_eq!(am.move_highlight_down(), Some(2));
    assert!(am.activate_index(1).is_none());
    let am_flip = am.compute_layout_with_flip(1440.0, 900.0, &dark);
    assert!(am_flip.flip_x && am_flip.flip_y && am_flip.within_viewport && am_flip.disabled_item_skipped && !am_flip.disabled_handler_invoked);

    assert_eq!(compute_tooltip_layout(true, "Settings").z_index, 240);
    let mut drawer = MainMenuDrawer::new(vec![], 0);
    drawer.set_palette(dark);
    assert!(!compute_main_menu_drawer_layout(&drawer).uses_legacy_blue_header);

    assert_eq!(ShortcutCommand::ALL.len(), 11);
    assert_eq!(FixedShortcutGroup::ALL.len(), 4);
    assert!(ShortcutRegistry::new("darwin").resolve().is_ok());
    assert!(ShortcutRegistry::validate_chord("darwin", &ShortcutChord::new(false, false, false, false, "KeyA")).is_err());
    assert!(ShortcutRegistry::validate_chord("darwin", &ShortcutChord::new(false, false, false, true, "KeyQ")).is_err());
    let mut dup = BTreeMap::new();
    dup.insert(ShortcutCommand::GlobalSearch.id().to_string(), ShortcutChord::new(false, false, false, true, "KeyB"));
    assert!(ShortcutRegistry::apply_overrides("darwin", &dup).is_err());
    assert!(!ShortcutChord::new(false, false, false, true, "KeyK").matches(false, false, false, true, "KeyK", true));
}

#[test]
fn emit_openbot_uiux_probe_json() {
    let mut payload_a = openbot_uiux_probe::build_full_probe_json();
    let payload_b = openbot_uiux_probe::build_full_probe_json();
    assert!(
        payload_a["compat_and_ledger"].get("replay_deterministic").is_none(),
        "build_full_probe_json must NOT hardcode replay_deterministic before live replay comparison"
    );

    let s_a = serde_json::to_string(&payload_a).expect("serialize pass 1");
    let s_b = serde_json::to_string(&payload_b).expect("serialize pass 2");
    let is_deterministic = s_a == s_b;
    assert!(is_deterministic, "probe payload must be byte-for-byte deterministic across live replay");

    if let Some(cl) = payload_a.get_mut("compat_and_ledger").and_then(Value::as_object_mut) {
        cl.insert("replay_deterministic".into(), json!(is_deterministic));
    }

    assert_eq!(payload_a["schema_version"], 1);
    assert_eq!(payload_a["suite"], "openbot-uiux-refactor-v1");
    assert_eq!(payload_a["panics"], 0);
    assert_eq!(payload_a["compat_and_ledger"]["replay_deterministic"], true);
    assert_eq!(payload_a["compat_and_ledger"]["headless_gpui_render_ok"], true);
    assert_eq!(payload_a["compat_and_ledger"]["ledger_closure_pass"], true);
    assert_eq!(payload_a["compat_and_ledger"]["allow_dead_code_count"], 0);
    assert_eq!(payload_a["compat_and_ledger"]["plush_asset_count"], 74);
    assert!(payload_a["compat_and_ledger"]["ledger_targets_verified"].as_u64().unwrap() >= 1090);

    let defects = payload_a["defects_audit"].as_object().expect("defects_audit object");
    assert_eq!(defects.len(), 12);
    for (k, v) in defects {
        assert_eq!(v, &json!(false), "defect {k} must be eliminated");
    }
    for section in [
        "tokens", "plush", "shell", "page_rail", "sidebar_chrome", "sidebar_list",
        "global_search", "agent_studio", "conversation_header", "chat_stream",
        "channel_stage", "composer", "mention_picker", "composer_stack", "side_panel",
        "agent_settings_profile", "channel_settings", "contact_profile",
        "content_hub_pages", "settings_and_overlays", "compat_and_ledger", "defects_audit",
    ] {
        assert!(payload_a.get(section).is_some_and(Value::is_object), "missing required probe section {section}");
    }

    if let Ok(out_path) = std::env::var("OPENBOT_UIUX_EMIT_PROBE_JSON") {
        let path = PathBuf::from(out_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let serialized = serde_json::to_string_pretty(&payload_a).expect("serialize probe json");
        std::fs::write(&path, serialized).expect("write probe json");
    }
}

#[test]
fn ui_f20_compat_consts_plush_assets_and_ui_probe_validation() {
    use gpui_telegram_desktop::views::stage::cards::text_card::{
        MSG_MAX_WIDTH as TEXT_CARD_MSG_MAX_WIDTH, MSG_MIN_WIDTH as TEXT_CARD_MSG_MIN_WIDTH,
    };
    use openbot_uiux_probe::f18_f20::{
        desktop_src_dir, has_crate_allow_dead_code_or_unused, scan_allow_dead_code_count,
        strip_rust_comments, verify_disk_ledger_and_capabilities, workspace_root,
    };

    assert_eq!(NAV_RAIL_WIDTH, 52.0);
    assert_eq!(LEGACY_SB_DEFAULT, 288.0);
    assert_eq!(LEGACY_SB_COLLAPSED, 64.0);
    assert_eq!(LEGACY_SB_ROW_H, 62.0);
    assert_eq!(LEGACY_INSPECTOR_W, 320.0);
    assert_eq!(MSG_MIN_WIDTH, 160.0);
    assert_eq!(MSG_MAX_WIDTH, 430.0);
    assert_eq!(TEXT_CARD_MSG_MIN_WIDTH, 160.0);
    assert_eq!(TEXT_CARD_MSG_MAX_WIDTH, 430.0);
    assert_eq!(ACTIVE_FADE_IN_DURATION_MS, 200);
    assert_eq!(FADE_WRAP_DURATION_MS, 300);
    assert_eq!(ACTIVE_FADE_OUT_DURATION_MS, 600);
    assert_eq!(FIRST_AGENT_COLOR_CHOICES_COUNT, 17);
    assert!(FABUSHI_AVATAR_SHAPES.contains(&default_fabushi_avatar_shape("openbot-default")));

    assert!(strip_rust_comments("// hidden\nfn ok() {}").contains("fn ok()"));
    assert!(!has_crate_allow_dead_code_or_unused("// #![allow(dead_code)]\nfn main() {}"));
    assert!(!has_crate_allow_dead_code_or_unused("/* outer /* inner */ #![allow(unused)] */ fn f() {}"));
    assert!(!has_crate_allow_dead_code_or_unused("const S: &str = \"#![allow(dead_code)]\";"));
    assert!(!has_crate_allow_dead_code_or_unused("const R: &str = r#\"#![allow(unused)]\"#;"));
    assert!(!has_crate_allow_dead_code_or_unused("#[allow(dead_code)]\nfn helper() {}"));
    assert!(has_crate_allow_dead_code_or_unused("/* doc */ #![allow(dead_code)]\nfn f() {}"));
    assert!(has_crate_allow_dead_code_or_unused("#![allow(\n  unused\n)]"));
    assert_eq!(scan_allow_dead_code_count(&desktop_src_dir()), 0);

    let (unique_targets, ledger_ok, cap_ok) = verify_disk_ledger_and_capabilities(&workspace_root());
    assert!(unique_targets >= 1090, "expected >= 1090 physical ledger targets, got {unique_targets}");
    assert!(ledger_ok && cap_ok);

    let audit = evaluate_headless_shell_layout_suite();
    assert_eq!(audit.panics, 0);
    assert!(audit.headless_gpui_render_ok);
    assert!(audit.defects.all_eliminated());

    let empty_dir = tempfile::tempdir().unwrap();
    let empty_assets = BundledPlushAssets::with_root(empty_dir.path());
    assert!(empty_assets.resolve_asset_path("bloub/boule.webp").is_none());
    let frame = PlushFeltRenderer::for_seed("fallback-seed", None, None, AvatarMood::Idle).sample_plush_frame(0.0, false);
    let img = composite_plush_rgba_with_assets(&frame, 32, 1.0, &empty_assets);
    assert!(img.pixels().any(|p| p.0[3] > 0));

    let probe_tmp = tempfile::tempdir().unwrap();
    let inv = std::panic::catch_unwind(|| evaluate_ui_probe("unknown-scenario-xyz", probe_tmp.path(), None)).expect("no panic");
    assert_eq!(inv.unwrap_err().exit_code(), 2);
    let mal = std::panic::catch_unwind(|| evaluate_ui_probe("{malformed-json", probe_tmp.path(), None)).expect("no panic");
    assert_eq!(mal.unwrap_err().exit_code(), 2);

    let deep_cycle_payload = json!({
        "scenario": "openbot-shell",
        "sidebar_layout": {
            "revision": 1,
            "sections": [
                {"id": "d0", "name": "D0", "parent_id": null, "collapsed": false},
                {"id": "d1", "name": "D1", "parent_id": "d0", "collapsed": false},
                {"id": "d2", "name": "D2", "parent_id": "d1", "collapsed": false},
                {"id": "d3", "name": "D3", "parent_id": "d2", "collapsed": false},
                {"id": "d4", "name": "D4", "parent_id": "d3", "collapsed": false},
                {"id": "cyc_a", "name": "CA", "parent_id": "cyc_b", "collapsed": false},
                {"id": "cyc_b", "name": "CB", "parent_id": "cyc_a", "collapsed": false}
            ],
            "order": ["d0", "d1", "d2", "d3", "d4", "cyc_a", "cyc_b", "unassigned", "people"],
            "agent_assignments": {},
            "agent_order": []
        }
    })
    .to_string();
    let shell_val = evaluate_ui_probe(&deep_cycle_payload, probe_tmp.path(), None)
        .expect("legacy 5-level + cycle snapshot must clamp cleanly in ui_probe");
    let depths = shell_val["openbot_shell"]["resolved_folder_depths"]
        .as_object()
        .expect("resolved_folder_depths");
    for (k, v) in depths {
        assert!(v.as_u64().unwrap() <= 2, "folder {k} depth exceeded 2: {v}");
    }
}
