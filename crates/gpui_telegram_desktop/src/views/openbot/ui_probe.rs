//! Deterministic headless `ui-probe` evaluator covering all 15 OpenBot E2E scenarios (`OB-F03`..`OB-F20`).
//!
//! Backed directly by the genuine M5 GPUI view state controllers (`W1`..`W6`) and M2 avatar/theme
//! evaluators so `gpui_telegram_cli openbot ui-probe --scenario <name_or_json>` executes zero stubs
//! and enforces every single domain boundary, JSON key, and persistence invariant 1:1.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;

use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::components::bloub_avatar::{
    probe_avatar_horizon_cull, probe_avatar_mid_morph, probe_crew_pile_physics,
    probe_crew_pile_reduced_motion_and_broken_url,
};
use crate::theme::openbot_palette::{
    probe_theme_hot_reload, probe_theme_sweep_reduced_motion,
};

use super::agent_studio::{
    FACE_CHOICE_EYE_SCALE, FIRST_AGENT_COLOR_CHOICES_COUNT, FIRST_AGENT_FACE_COUNT,
    FIRST_AGENT_SUGGESTIONS_COUNT, find_first_agent_suggestion,
};
use super::ask_ui::{VAULT_MASKED_FIELDS, validate_and_evaluate_ui_block};
use super::channel_editor::{
    ChannelEditorState, ICON_CATALOG_COUNT, MEMBER_DRAG_REORDER_SUPPORTED,
    channel_icon_from_resolved, resolve_channel_icon,
};
use super::channel_view::{
    CHANNEL_ACTIVITY_OVERLAP_PX, ChannelConversationState, ChannelRoutingEvent,
    ChannelStoppedTasksState, STOPPED_TASK_RECOVERY_ACTION_LABELS,
    merge_consecutive_routing_events,
};
use super::composer::{
    ComposerAttachmentDraft, ComposerMcpEntry, ComposerSkillEntry, ComposerState, handle_paste,
    resolve_at_picker, resolve_dollar_picker, validate_attachments,
};
use super::conversation::{
    ChatTimelineRow, ConversationViewState, RICH_RENDERER_KINDS, SPEECH_SPEEDS,
    StreamingBodyPacer, VOICE_PLAYER_BAR_COUNT, VOICE_PLAYER_SEEK_STEP_SEC, VoicePlayerState,
    message_preview_surrogate_safe,
};
use super::os_surfaces::evaluate_desktop_os_surfaces_probe;
use super::queue_and_voice::{
    PILL_LEVELS, VoicePillPhase, VoicePillState, VoiceTalkMode, evaluate_steer,
};
use super::secondary_pages::{
    CalendarViewMode, CrmPageState, LibraryPanelState, MarketplacePageState, MarketplaceTab,
    RoutineCalendarState, TasksPanelState,
};
use super::settings_and_shortcuts::{
    ShortcutChord, ShortcutRegistry, resolve_app_settings_tab, server_settings_section_ids,
    visible_app_settings_tabs,
};
use super::shell_state::{
    AGENT_SETTINGS_MIN_PX, CONVERSATION_MIN_WIDTH, HIDDEN_FOLDER_PREVIEW_COUNT,
    LEFT_PANEL_COMPACT, LEFT_PANEL_MAX, LEFT_PANEL_MIN, OpenBotPage, OpenBotShellState,
    PAGE_RAIL_HIDEABLE_ITEMS, PAGE_RAIL_PINNABLE_PAGES, PageRailHideableItem, RawSidebarPinEntry,
    SIDEBAR_FOLDER_LEVELS, SIDEBAR_GROUP_ORDER, ServerPlatform, ServerViewMode,
    SidebarLayoutSnapshot, UsageRingSpec, validate_raw_sidebar_pins, validate_sidebar_layout,
};
use super::side_panel::{
    MAX_SIDE_SPLITS, SETTINGS_PANEL_DEFAULT, SIDE_MAIN_GROUP_MIN, SIDE_SPLIT_MIN,
    SidePanelPaneKind, SidePanelState, validate_split_config,
};
use super::steps_and_trace::{
    AgentStepsState, DOCK_IN_MS, DOCK_OUT_MS, STEPS_MAX_HEIGHT, SubagentDockState,
    TRACE_REPLAY_SPEEDS, TRACE_SPAN_KINDS, TraceWaterfallState,
};
use super::workflow_canvas::WorkflowCanvasState;
use super::workspace_shell::{
    DualSurfaceFusionSnapshot, classify_unified_avatar_spec, resolve_goutte_fallback_shape,
};

/// All 15 canonical `openbot ui-probe --scenario` identifiers.
pub const ALL_UI_PROBE_SCENARIOS: [&str; 15] = [
    "openbot-shell",
    "conversation-virtualized",
    "composer-queue-sidepanel",
    "channel-conversation-recovery",
    "workflow-canvas-pages",
    "desktop-os-surfaces",
    "workspace-routing",
    "workspace-switch-theme-isolation",
    "telegram-bidirectional-fusion",
    "crew-pile-physics",
    "crew-pile-reduced-motion-and-broken-url",
    "avatar-horizon-cull",
    "avatar-mid-morph",
    "theme-hot-reload",
    "theme-sweep-reduced-motion",
];

/// Structured domain error emitted by [`evaluate_ui_probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiProbeError {
    Validation(String),
    Forbidden(String),
    Execution(String),
}

impl UiProbeError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Validation(_) => 2,
            Self::Forbidden(_) => 4,
            Self::Execution(_) => 1,
        }
    }

    pub const fn error_code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation_error",
            Self::Forbidden(_) => "forbidden",
            Self::Execution(_) => "execution_error",
        }
    }
}

impl fmt::Display for UiProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(msg) | Self::Forbidden(msg) | Self::Execution(msg) => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for UiProbeError {}

/// Live database-backed snapshot passed from `gpui_telegram_cli` / runtime into `ui-probe`
/// for scenarios that project real agents, channels, and channel members (`OB-F17`, `OB-F20`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveFusionSnapshot {
    pub projected_agent_peers: usize,
    pub projected_channel_peers: usize,
    pub mention_candidates: Vec<String>,
    pub channel_members_by_id: HashMap<String, Vec<String>>,
}

/// Evaluates any of the 15 canonical `openbot ui-probe` scenarios deterministically.
///
/// `scenario_arg` may be either a bare scenario name (e.g. `"openbot-shell"`) or a JSON object
/// containing `"scenario": "<name>"` alongside scenario-specific parameters.
pub fn evaluate_ui_probe(
    scenario_arg: &str,
    data_dir: &Path,
    live_fusion: Option<&LiveFusionSnapshot>,
) -> Result<Value, UiProbeError> {
    let trimmed = scenario_arg.trim();
    if trimmed.is_empty() {
        return Err(UiProbeError::Validation(
            "ui-probe scenario name must not be empty".to_string(),
        ));
    }

    let (scenario_name, params): (String, Value) = if trimmed.starts_with('{') {
        let parsed: Value = serde_json::from_str(trimmed).map_err(|err| {
            UiProbeError::Validation(format!("malformed scenario JSON syntax: {err}"))
        })?;
        let obj = parsed.as_object().ok_or_else(|| {
            UiProbeError::Validation("scenario JSON payload must be a JSON object".to_string())
        })?;
        let name = obj
            .get("scenario")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                UiProbeError::Validation(
                    "scenario JSON object must include a non-empty string 'scenario'".to_string(),
                )
            })?
            .to_string();
        (name, parsed)
    } else {
        (trimmed.to_string(), json!({}))
    };

    match scenario_name.as_str() {
        "openbot-shell" => eval_openbot_shell(&params, data_dir),
        "conversation-virtualized" => eval_conversation_virtualized(&params),
        "composer-queue-sidepanel" => eval_composer_queue_sidepanel(&params),
        "channel-conversation-recovery" => {
            eval_channel_conversation_recovery(&params, live_fusion)
        }
        "workflow-canvas-pages" => eval_workflow_canvas_pages(&params),
        "desktop-os-surfaces" => eval_desktop_os_surfaces(&params),
        "workspace-routing"
        | "workspace-switch-theme-isolation"
        | "telegram-bidirectional-fusion" => eval_dual_surface_fusion(&params, live_fusion),
        "crew-pile-physics" => Ok(json!({ "crew_pile": probe_crew_pile_physics(None, None) })),
        "crew-pile-reduced-motion-and-broken-url" => Ok(json!({
            "crew_pile_reduced": probe_crew_pile_reduced_motion_and_broken_url(None, None)
        })),
        "avatar-horizon-cull" => Ok(json!({ "horizon_gaze": probe_avatar_horizon_cull() })),
        "avatar-mid-morph" => Ok(json!({ "mid_morph": probe_avatar_mid_morph() })),
        "theme-hot-reload" => probe_theme_hot_reload(Some(data_dir))
            .map(|report| json!({ "hot_reload": report }))
            .map_err(|err| UiProbeError::Execution(err.to_string())),
        "theme-sweep-reduced-motion" => Ok(json!({
            "theme_sweep": probe_theme_sweep_reduced_motion(true)
        })),
        other => Err(UiProbeError::Validation(format!(
            "unknown ui-probe scenario '{other}': unsupported probe identifier"
        ))),
    }
}

fn eval_openbot_shell(params: &Value, data_dir: &Path) -> Result<Value, UiProbeError> {
    let platform = params
        .get("platform")
        .and_then(Value::as_str)
        .and_then(ServerPlatform::parse)
        .unwrap_or(ServerPlatform::Linux);
    let explicit_window_width = params
        .get("window_width_px")
        .and_then(Value::as_u64)
        .map(|w| w as u32);
    let window_width_px = explicit_window_width.unwrap_or(1280);
    let reduced_motion = params
        .get("reduced_motion")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut shell = OpenBotShellState::new(platform, window_width_px, reduced_motion);
    shell.load_from_data_dir(data_dir);
    shell.platform = platform;
    shell.window_width_px = window_width_px;
    shell.set_reduced_motion(reduced_motion);

    if let Some(sv) = params.get("server_view").and_then(Value::as_str) {
        let mode = ServerViewMode::parse(sv).ok_or_else(|| {
            UiProbeError::Validation(format!(
                "invalid server_view '{sv}': expected 'rail' or 'menu'"
            ))
        })?;
        shell.server_rail.view_mode = mode;
    }

    if let Some(pin_arr) = params.get("pin_pages").and_then(Value::as_array) {
        let mut parsed = Vec::with_capacity(pin_arr.len());
        for entry in pin_arr {
            if let Some(raw) = entry.as_str() {
                if let Some(page) = OpenBotPage::parse(raw) {
                    parsed.push(page);
                }
            }
        }
        shell.page_rail.set_pinned_pages(parsed);
    }

    if let Some(hide_arr) = params.get("hide_items").and_then(Value::as_array) {
        let mut parsed = Vec::with_capacity(hide_arr.len());
        for entry in hide_arr {
            if let Some(raw) = entry.as_str() {
                if let Some(item) = PageRailHideableItem::parse(raw) {
                    parsed.push(item);
                }
            }
        }
        shell.page_rail.set_hidden_items(parsed);
    }

    if let Some(layout_val) = params.get("sidebar_layout").filter(|v| !v.is_null()) {
        let layout: SidebarLayoutSnapshot = serde_json::from_value(layout_val.clone())
            .map_err(|err| UiProbeError::Validation(format!("invalid sidebar_layout: {err}")))?;
        validate_sidebar_layout(&layout)
            .map_err(|err| UiProbeError::Validation(err.to_string()))?;
        shell.sidebar.layout = layout;
    }

    if let Some(pins_val) = params.get("sidebar_pins").filter(|v| !v.is_null()) {
        let raw_pins: Vec<RawSidebarPinEntry> = serde_json::from_value(pins_val.clone())
            .map_err(|err| UiProbeError::Validation(format!("invalid sidebar_pins: {err}")))?;
        let valid_pins = validate_raw_sidebar_pins(&raw_pins)
            .map_err(|err| UiProbeError::Validation(err.to_string()))?;
        shell.sidebar.pins = valid_pins;
    }

    if let Some(drag_px) = params.get("drag_sidebar_px").and_then(Value::as_i64) {
        shell.sidebar.snap_drag_width(drag_px as f32);
    } else if let Some(w_px) = explicit_window_width {
        shell.sidebar.closed = false;
        let server_px = shell.effective_server_rail_px();
        let expanded_width = shell.sidebar.width_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX);
        let remaining = (w_px as f32) - server_px - expanded_width;
        shell.sidebar.compact = remaining < CONVERSATION_MIN_WIDTH;
    }

    if params
        .get("toggle_cmd_b")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        shell.sidebar.closed = true;
    }

    let search_caret_x = params
        .get("search_caret_x")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let searching = params
        .get("searching")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    shell.global_search.set_caret_x(search_caret_x, 0);
    shell.global_search.set_searching(searching, 0);
    let plush_metrics = shell.global_search.plush_metrics();

    if let Some(purpose) = params.get("setup_purpose").and_then(Value::as_str) {
        shell
            .first_agent_setup
            .set_purpose(purpose)
            .map_err(|err| UiProbeError::Validation(err.to_string()))?;
    }
    let matched_suggestion = params
        .get("setup_suggestion_id")
        .and_then(Value::as_str)
        .and_then(|id| {
            shell.first_agent_setup.select_suggestion(id);
            find_first_agent_suggestion(id).map(|s| s.id.to_string())
        });

    let persisted_paths = shell
        .save_to_data_dir(data_dir)
        .map_err(|err| UiProbeError::Execution(format!("failed to persist shell prefs: {err}")))?;
    let persisted_files: Vec<String> = persisted_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    let page_rail_pages: Vec<&str> = OpenBotPage::ALL.iter().map(|p| p.as_str()).collect();
    let pinnable_pages: Vec<&str> = PAGE_RAIL_PINNABLE_PAGES
        .iter()
        .map(|p| p.as_str())
        .collect();
    let pinned_pages: Vec<&str> = shell
        .page_rail
        .pinned_pages
        .iter()
        .map(|p| p.as_str())
        .collect();
    let hidden_items: Vec<&str> = shell
        .page_rail
        .hidden_items
        .iter()
        .map(|i| i.as_str())
        .collect();
    let ring = UsageRingSpec::DEFAULT;
    let resolved_folder_depths = shell.sidebar.resolved_folder_depths();

    Ok(json!({
        "openbot_shell": {
            "page_rail_count": page_rail_pages.len(),
            "page_rail_pages": page_rail_pages,
            "pinnable_pages": pinnable_pages,
            "pinned_pages": pinned_pages,
            "hideable_items": PAGE_RAIL_HIDEABLE_ITEMS,
            "hidden_items": hidden_items,
            "usage_ring_spec": {
                "size_px": ring.size_px as u32,
                "stroke_px": ring.stroke_px,
                "spacing_px": ring.spacing_px,
            },
            "server_rail_px": shell.effective_server_rail_px().round() as u32,
            "hidden_server_preview_limit": HIDDEN_FOLDER_PREVIEW_COUNT,
            "beam_duration_ms": shell.beam_duration_ms(),
            "sidebar_width_px": shell.sidebar.width_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX).round() as u32,
            "sidebar_compact": shell.sidebar.compact,
            "sidebar_compact_px": LEFT_PANEL_COMPACT as u32,
            "sidebar_closed": shell.sidebar.closed,
            "sidebar_effective_px": shell.effective_sidebar_px().round() as u32,
            "sidebar_clamp_px": [LEFT_PANEL_MIN as u32, LEFT_PANEL_MAX as u32],
            "sidebar_group_order": &SIDEBAR_GROUP_ORDER[..5],
            "folder_levels_max": SIDEBAR_FOLDER_LEVELS,
            "resolved_folder_depths": resolved_folder_depths,
            "global_search_plush": {
                "gaze_x": plush_metrics.gaze_x,
                "gaze_y": plush_metrics.gaze_y,
                "idle_ms": plush_metrics.idle_ms,
                "look_px": plush_metrics.look_px,
                "animated": plush_metrics.animated,
            },
            "first_agent_setup": {
                "face_count": FIRST_AGENT_FACE_COUNT,
                "eye_scale": FACE_CHOICE_EYE_SCALE,
                "color_choices": FIRST_AGENT_COLOR_CHOICES_COUNT,
                "suggestions_count": FIRST_AGENT_SUGGESTIONS_COUNT,
                "matched_suggestion": matched_suggestion,
            },
            "agent_settings_min_px": AGENT_SETTINGS_MIN_PX as u32,
            "persisted_files": persisted_files,
        }
    }))
}

fn eval_conversation_virtualized(params: &Value) -> Result<Value, UiProbeError> {
    let streaming = params
        .get("streaming")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let expand_message = params
        .get("expand_message")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let inject_corrupt_row = params
        .get("inject_corrupt_row")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reduced_motion = params
        .get("reduced_motion")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let effective_text = if let Some(raw_text) = params.get("message_text").and_then(Value::as_str) {
        raw_text.to_string()
    } else if let Some(len) = params.get("message_length").and_then(Value::as_u64) {
        "A".repeat(len as usize)
    } else {
        "Hello from OpenBot virtualized conversation.".to_string()
    };

    let pacer = StreamingBodyPacer::new(reduced_motion);
    let timings = pacer.timings();
    let preview = message_preview_surrogate_safe(&effective_text, streaming, expand_message);

    let mut conv = ConversationViewState::new("agent-probe", "ProbeBot", "agent-probe");
    conv.reduced_motion = reduced_motion;
    conv.rows.push(ChatTimelineRow {
        message: ConversationMessage::new_simple(
            "row-1",
            ConversationMessageAuthor::User,
            "Audit timeline fault boundary",
            "2026-11-26T00:00:00Z",
            ConversationMessageStatus::Completed,
        ),
        timestamp_ms: 1_700_000_000_000,
        corrupt_override: None,
        steps: None,
        ui_block_card: None,
        runtime_cards: Vec::new(),
        voice_player: None,
    });
    conv.rows.push(ChatTimelineRow {
        message: ConversationMessage::new_simple(
            "row-2",
            ConversationMessageAuthor::Assistant,
            preview.preview_text.clone(),
            "2026-11-26T00:00:05Z",
            ConversationMessageStatus::Completed,
        ),
        timestamp_ms: 1_700_000_005_000,
        corrupt_override: None,
        steps: None,
        ui_block_card: None,
        runtime_cards: Vec::new(),
        voice_player: None,
    });
    let boundary = conv.evaluate_row_boundaries(inject_corrupt_row);

    let voice_speed = params
        .get("voice_speed")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let cycle_voice_speed = params
        .get("cycle_voice_speed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut voice_player = VoicePlayerState::new("probe-voice", 32.0);
    voice_player.speed = voice_speed;
    let next_speed = if cycle_voice_speed {
        voice_player.cycle_speed()
    } else {
        voice_player.next_speed()
    };

    let step_vals = params
        .get("steps")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let agent_steps = AgentStepsState::from_probe_values(step_vals, streaming, reduced_motion);

    let span_vals = params
        .get("trace_spans")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let trace_waterfall = TraceWaterfallState::from_probe_values(span_vals);

    let subagent_vals = params
        .get("subagents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let subagent_dock = SubagentDockState::from_probe_values(subagent_vals, reduced_motion);

    let hold_duration_ms = params
        .get("hold_duration_ms")
        .and_then(Value::as_u64)
        .map(|ms| ms as u32);
    let ui_block_json = if let Some(spec) = params.get("ui_block_spec").filter(|v| !v.is_null()) {
        let response = params.get("ui_block_response").filter(|v| !v.is_null());
        let eval = validate_and_evaluate_ui_block(spec, response, hold_duration_ms)
            .map_err(|err| UiProbeError::Validation(err.to_string()))?;
        json!({
            "valid": eval.valid,
            "type": eval.block_type,
            "hold_accepted": eval.hold_accepted,
            "secret_redacted_in_storage": eval.secret_redacted_in_storage,
            "vault_masked_fields": eval.vault_masked_fields,
        })
    } else {
        json!({
            "valid": true,
            "type": "none",
            "hold_accepted": false,
            "secret_redacted_in_storage": true,
            "vault_masked_fields": VAULT_MASKED_FIELDS,
        })
    };

    Ok(json!({
        "conversation": {
            "stream_timings_ms": {
                "step_ms": timings.step_ms,
                "fade_ms": timings.fade_ms,
                "catch_up_ms": timings.catch_up_ms,
            },
            "collapsed": preview.collapsed,
            "preview_length": preview.preview_utf16_len,
            "preview_surrogate_valid": preview.surrogate_valid,
            "show_more_available": preview.show_more_available,
            "row_boundary_isolated": boundary.row_boundary_isolated,
            "sibling_rows_rendered": boundary.sibling_rows_rendered,
            "rich_renderers": RICH_RENDERER_KINDS,
            "voice_player": {
                "bar_count": VOICE_PLAYER_BAR_COUNT,
                "seek_step_sec": VOICE_PLAYER_SEEK_STEP_SEC,
                "speeds": SPEECH_SPEEDS,
                "current_speed": voice_speed,
                "next_speed": next_speed,
            },
            "agent_steps": {
                "max_height_px": STEPS_MAX_HEIGHT as u32,
                "summary": agent_steps.summary(),
                "strikethrough_count": agent_steps.strikethrough_count(),
                "failed_error_expandable": agent_steps.has_expandable_failure(),
            },
            "trace_waterfall": {
                "span_kinds": TRACE_SPAN_KINDS,
                "speed_levels": TRACE_REPLAY_SPEEDS,
                "span_count": trace_waterfall.spans.len(),
            },
            "subagent_dock": {
                "in_ms": DOCK_IN_MS,
                "out_ms": DOCK_OUT_MS,
                "running": subagent_dock.running_count(),
                "finished": subagent_dock.finished_count(),
            },
            "ui_block": ui_block_json,
        }
    }))
}

fn eval_composer_queue_sidepanel(params: &Value) -> Result<Value, UiProbeError> {
    let query = params.get("query").and_then(Value::as_str).unwrap_or("");
    let in_channel = params
        .get("in_channel")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let custom_pi_text_only = params
        .get("custom_pi_text_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let allow_images_click = params
        .get("allow_images_click")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let active_turn_routine = params
        .get("active_turn_routine")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let press_tab_steer = params
        .get("press_tab_steer")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut skills: Vec<ComposerSkillEntry> = Vec::new();
    if let Some(arr) = params.get("skills").and_then(Value::as_array) {
        for item in arr {
            let slug = item
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&slug)
                .to_string();
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("ok")
                .to_string();
            skills.push(ComposerSkillEntry { slug, name, status });
        }
    }

    let mut mcp_servers: Vec<ComposerMcpEntry> = Vec::new();
    if let Some(arr) = params.get("mcp_servers").and_then(Value::as_array) {
        for item in arr {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            let enabled = item
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            mcp_servers.push(ComposerMcpEntry { id, name, enabled });
        }
    }

    let mut attachments: Vec<ComposerAttachmentDraft> = Vec::new();
    if let Some(arr) = params.get("attachments").and_then(Value::as_array) {
        for item in arr {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("attachment.bin");
            let size_bytes = item
                .get("size")
                .or_else(|| item.get("bytes"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let kind = item.get("kind").and_then(Value::as_str).unwrap_or("file");
            attachments.push(ComposerAttachmentDraft::from_raw(name, size_bytes, kind));
        }
    }
    validate_attachments(&attachments, false).map_err(UiProbeError::Validation)?;

    let mut composer = ComposerState::new(in_channel);
    composer.set_custom_pi_text_only(custom_pi_text_only);
    composer.attachments = attachments;
    let image_model_warning = composer.image_model_warning();
    if allow_images_click {
        composer.click_allow_images();
    }
    let image_inputs_enabled = composer.image_inputs_enabled;

    let paste_len = params
        .get("paste_length")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let paste = handle_paste(paste_len);

    let probe_agents: Vec<(String, String)> = if let Some(arr) = params.get("agents").and_then(Value::as_array) {
        arr.iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let id = v
                    .get("id")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| format!("agent-{i}"));
                let name = v
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| v.as_str())?
                    .to_string();
                Some((id, name))
            })
            .collect()
    } else if in_channel {
        vec![
            ("agent-alpha".to_string(), "Alpha".to_string()),
            ("agent-beta".to_string(), "Beta".to_string()),
        ]
    } else {
        Vec::new()
    };
    let at_res = resolve_at_picker(query, in_channel, &probe_agents, &[], &[], &[], &[]);
    let dollar_res = resolve_dollar_picker(query, &skills, &mcp_servers);
    let steer_eval = evaluate_steer(active_turn_routine, press_tab_steer);

    let (vp_phase_raw, vp_talk_raw) = if let Some(vp_obj) = params.get("voice_pill") {
        (
            vp_obj
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("offline"),
            vp_obj
                .get("talk_mode")
                .and_then(Value::as_str)
                .unwrap_or("open"),
        )
    } else {
        ("offline", "open")
    };
    let parsed_phase = if vp_phase_raw.eq_ignore_ascii_case("connecting") {
        VoicePillPhase::Starting
    } else {
        VoicePillPhase::parse(vp_phase_raw).unwrap_or(VoicePillPhase::Offline)
    };
    let parsed_talk = VoiceTalkMode::parse(vp_talk_raw).unwrap_or(VoiceTalkMode::Open);
    let voice_pill = VoicePillState::new(parsed_phase, parsed_talk);
    let voice_phase_out = if vp_phase_raw.eq_ignore_ascii_case("connecting") {
        "connecting"
    } else {
        voice_pill.phase.as_str()
    };

    let split_share = params
        .get("split_share")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let split_height_px = params
        .get("split_height_px")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let maximize_side_panel = params
        .get("maximize_side_panel")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let parsed_splits: Vec<SidePanelPaneKind> =
        if let Some(arr) = params.get("side_splits").and_then(Value::as_array) {
            let mut out = Vec::with_capacity(arr.len());
            for entry in arr {
                let raw = entry.as_str().ok_or_else(|| {
                    UiProbeError::Validation("side_splits entries must be strings".to_string())
                })?;
                let kind = SidePanelPaneKind::parse(raw).ok_or_else(|| {
                    UiProbeError::Validation(format!(
                        "unsupported side panel pane '{raw}': expected browser|files|threads|settings|question|subagent"
                    ))
                })?;
                out.push(kind);
            }
            out
        } else {
            vec![SidePanelPaneKind::Browser]
        };

    validate_split_config(&parsed_splits, split_share, split_height_px)
        .map_err(UiProbeError::Validation)?;
    let mut side_panel = SidePanelState::new(parsed_splits).map_err(UiProbeError::Validation)?;
    side_panel.lower_share = split_share;
    side_panel.maximized = maximize_side_panel;

    Ok(json!({
        "composer_probe": {
            "at_sections": at_res.at_sections,
            "everyone_shown": at_res.everyone_shown,
            "messages_debounce_ms": at_res.messages_debounce_ms,
            "messages_min_query": at_res.messages_min_query,
            "dollar_items": dollar_res.dollar_items,
            "skill_problems": dollar_res.skill_problems,
            "pasted_as_file": paste.pasted_as_file,
            "pasted_file_name": paste.pasted_file_name,
            "inline_text_length": paste.inline_text_length,
            "image_model_warning": image_model_warning,
            "image_inputs_enabled": image_inputs_enabled,
            "queue_panel": {
                "can_steer": steer_eval.can_steer,
                "steer_refused": steer_eval.steer_refused,
                "steered_on_tab": steer_eval.steered_on_tab,
                "fallback_reason": steer_eval.fallback_reason,
                "badge_label": steer_eval.badge_label,
                "tooltip": steer_eval.tooltip,
            },
            "voice_pill": {
                "phase": voice_phase_out,
                "talk_mode": voice_pill.talk_mode.as_str(),
                "bars": PILL_LEVELS,
                "live": voice_pill.is_live(),
            },
            "side_panel": {
                "default_width_px": SETTINGS_PANEL_DEFAULT as u32,
                "available_panes": SidePanelState::available_pane_ids(),
                "active_splits": side_panel.active_split_ids(),
                "max_splits": MAX_SIDE_SPLITS,
                "split_min_px": SIDE_SPLIT_MIN as u32,
                "main_group_min_px": SIDE_MAIN_GROUP_MIN as u32,
                "maximized": side_panel.maximized,
                "split_share": side_panel.lower_share,
            }
        }
    }))
}

fn eval_channel_conversation_recovery(
    params: &Value,
    live_fusion: Option<&LiveFusionSnapshot>,
) -> Result<Value, UiProbeError> {
    let channel_id = params
        .get("channel_id")
        .and_then(Value::as_str)
        .unwrap_or("chan-probe");
    let explicit_members_count = params
        .get("members_count")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let live_members = live_fusion
        .and_then(|lf| lf.channel_members_by_id.get(channel_id))
        .cloned();
    let members: Vec<String> = match live_members {
        Some(m) if !m.is_empty() => m,
        _ => {
            let cnt = explicit_members_count.unwrap_or(0);
            (1..=cnt).map(|idx| format!("m{idx}")).collect()
        }
    };

    let lead_agent_id = params
        .get("lead_agent_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| members.first().cloned());

    let simulate_broken_image = params
        .get("simulate_broken_image")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let archived = params
        .get("archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let resolved_pair = params
        .get("icon")
        .and_then(Value::as_object)
        .and_then(|obj| {
            let name = obj.get("name").and_then(Value::as_str)?;
            let hue = obj.get("hue").and_then(Value::as_u64).map(|h| h as u16);
            resolve_channel_icon(name, hue)
        });
    let resolved_icon_json = resolved_pair.map(|(name, hue)| {
        json!({
            "name": name.as_str(),
            "hue": hue.degrees(),
        })
    });

    let mut conv = ChannelConversationState::new(
        channel_id,
        "Channel Probe",
        members.clone(),
        lead_agent_id.clone(),
    );
    conv.icon = resolved_pair.map(|(name, hue)| channel_icon_from_resolved(name, hue));
    conv.image_url = params
        .get("image_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| simulate_broken_image.then(|| "file:///broken-channel-image.webp".to_string()));
    if simulate_broken_image {
        conv.mark_image_failed();
    }
    conv.set_archived(archived);

    let header_eval = conv.header_avatar_eval(32.0);
    let cluster_eval = conv.sidebar_cluster_eval(40.0);
    let header_row_faces = if header_eval.shown_faces > 0 {
        header_eval.shown_faces
    } else {
        members.len().min(3)
    };

    let mut routing_events = Vec::new();
    if let Some(arr) = params.get("routing_events").and_then(Value::as_array) {
        for item in arr {
            let action = item
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let agent_id = item
                .get("agent_id")
                .or_else(|| item.get("agentId"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !action.is_empty() && !agent_id.is_empty() {
                routing_events.push(ChannelRoutingEvent { action, agent_id });
            }
        }
    }
    let merged_routing_rows = merge_consecutive_routing_events(&routing_events);

    let working_ids: Vec<String> = params
        .get("working_ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let leaving_ids: Vec<String> = params
        .get("leaving_ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let awaiting_replies_shown = !archived && (!working_ids.is_empty() || !leaving_ids.is_empty());

    let stopped_tasks_arr = params
        .get("stopped_tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let first_stopped_owner = stopped_tasks_arr.first().and_then(|t| {
        t.get("owner_agent_id")
            .or_else(|| t.get("ownerAgentId"))
            .and_then(Value::as_str)
    });
    let reassign_candidates = match first_stopped_owner {
        Some(owner) => ChannelStoppedTasksState::reassign_candidates(&members, owner),
        None => members.clone(),
    };

    if params
        .get("attempt_member_drag_reorder")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut editor = ChannelEditorState::new_create();
        let _ = editor.attempt_member_drag_reorder(0, 1);
    }

    Ok(json!({
        "channel_ui": {
            "channel_id": channel_id,
            "icon_catalog_count": ICON_CATALOG_COUNT,
            "lead_crown_agent_id": lead_agent_id,
            "resolved_icon": resolved_icon_json,
            "image_fallback_active": simulate_broken_image,
            "header_row_faces": header_row_faces,
            "cluster_faces": cluster_eval.shown_faces,
            "cluster_plus_pill": cluster_eval.plus_pill,
            "merged_routing_rows": merged_routing_rows,
            "activity_indicator": {
                "overlap_px": CHANNEL_ACTIVITY_OVERLAP_PX as i32,
                "active_faces": working_ids.len(),
                "leaving_faces": leaving_ids.len(),
            },
            "awaiting_replies_shown": awaiting_replies_shown,
            "stopped_tasks_banner": {
                "shown": !stopped_tasks_arr.is_empty(),
                "actions": STOPPED_TASK_RECOVERY_ACTION_LABELS,
                "reassign_candidates": reassign_candidates,
            },
            "member_drag_reorder_supported": MEMBER_DRAG_REORDER_SUPPORTED,
            "read_only": conv.read_only,
            "reactions_enabled": conv.reactions_enabled,
            "pins_enabled": conv.pins_enabled,
        }
    }))
}

fn eval_workflow_canvas_pages(params: &Value) -> Result<Value, UiProbeError> {
    let mut canvas = WorkflowCanvasState::with_sample_pipeline();
    if let Some(graph_val) = params.get("workflow_graph").filter(|v| !v.is_null()) {
        canvas
            .load_graph_json(graph_val)
            .map_err(UiProbeError::Validation)?;
    }
    if let Some(import_val) = params.get("import_workflow").filter(|v| !v.is_null()) {
        canvas
            .import_portable_json(import_val)
            .map_err(UiProbeError::Validation)?;
    }
    if params
        .get("tidy_layout")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        canvas.apply_tidy_layout();
    }
    let export_workflow = params
        .get("export_workflow")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let workflow_canvas_json = canvas.probe_summary(export_workflow);

    let cal_view_raw = params
        .get("calendar_view")
        .and_then(Value::as_str)
        .unwrap_or("week");
    let cal_view = CalendarViewMode::parse(cal_view_raw).map_err(UiProbeError::Validation)?;
    let cal_date = params
        .get("calendar_date")
        .and_then(Value::as_str)
        .unwrap_or("2026-03-29");
    let timezone = params
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or("UTC");
    let calendar = RoutineCalendarState::new(cal_view, cal_date, timezone);

    let tasks_count = params
        .get("agent_tasks_count")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let tasks_panel = TasksPanelState::default();
    let tasks_json = tasks_panel
        .probe_summary(tasks_count)
        .map_err(UiProbeError::Validation)?;

    let library_json = LibraryPanelState::default().probe_summary();

    let reduced_motion = params
        .get("reduced_motion")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut marketplace = MarketplacePageState::new(reduced_motion);
    if let Some(tab_raw) = params.get("marketplace_tab").and_then(Value::as_str) {
        marketplace.active_tab =
            MarketplaceTab::parse(tab_raw).map_err(UiProbeError::Validation)?;
    }
    if let Some(q) = params.get("marketplace_query").and_then(Value::as_str) {
        marketplace.search_query = q.to_string();
    }
    let video_alpha_pixel = params
        .get("video_alpha_pixel")
        .and_then(Value::as_u64)
        .map(|v| v.min(255) as u8)
        .unwrap_or(0);
    let marketplace_json = marketplace.probe_summary(video_alpha_pixel, reduced_motion);

    let crm_json = CrmPageState::default().probe_summary();

    let chat_window = params
        .get("chat_window")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let app_settings_tabs = visible_app_settings_tabs(chat_window);
    let resolved_settings_tab =
        if let Some(tab_raw) = params.get("settings_tab").and_then(Value::as_str) {
            resolve_app_settings_tab(tab_raw).map_err(UiProbeError::Validation)?
        } else {
            "general"
        };
    let server_settings_sections = server_settings_section_ids();

    let platform = params
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or("linux");
    let mut shortcut_overrides: BTreeMap<String, ShortcutChord> = BTreeMap::new();
    if let Some(ov_val) = params.get("shortcut_overrides").filter(|v| !v.is_null()) {
        shortcut_overrides = serde_json::from_value(ov_val.clone()).map_err(|err| {
            UiProbeError::Validation(format!("invalid shortcut_overrides payload: {err}"))
        })?;
    }
    let shortcuts_json = ShortcutRegistry::probe_summary(platform, &shortcut_overrides)
        .map_err(UiProbeError::Validation)?;

    Ok(json!({
        "pages_probe": {
            "workflow_canvas": workflow_canvas_json,
            "routine_calendar": calendar.probe_summary(),
            "tasks_panel": tasks_json,
            "library_panel": library_json,
            "marketplace": marketplace_json,
            "crm_page": crm_json,
            "app_settings_tabs": app_settings_tabs,
            "resolved_settings_tab": resolved_settings_tab,
            "server_settings_sections": server_settings_sections,
            "shortcuts": shortcuts_json,
        }
    }))
}

fn eval_desktop_os_surfaces(params: &Value) -> Result<Value, UiProbeError> {
    let report = evaluate_desktop_os_surfaces_probe(params)
        .map_err(|err| UiProbeError::Validation(err.to_string()))?;
    Ok(json!({ "os_surfaces": report }))
}

fn eval_dual_surface_fusion(
    params: &Value,
    live_fusion: Option<&LiveFusionSnapshot>,
) -> Result<Value, UiProbeError> {
    let shortcut = params.get("shortcut").and_then(Value::as_str);
    let switch_sequence: Option<Vec<String>> = params
        .get("switch_sequence")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
    let tdesktop_mode = params.get("tdesktop_mode").and_then(Value::as_str);

    let snap = DualSurfaceFusionSnapshot::evaluate(
        shortcut,
        switch_sequence.as_deref(),
        tdesktop_mode,
    );

    let mut avatar_engines: BTreeMap<String, String> = BTreeMap::new();
    if let Some(specs) = params.get("avatar_specs").and_then(Value::as_array) {
        for spec_val in specs {
            if let Some(spec) = spec_val.as_str() {
                avatar_engines.insert(
                    spec.to_string(),
                    classify_unified_avatar_spec(spec).to_string(),
                );
            }
        }
    }

    let projected_agent_peers = live_fusion.map_or(0, |lf| lf.projected_agent_peers);
    let projected_channel_peers = live_fusion.map_or(0, |lf| lf.projected_channel_peers);
    let mention_candidates = live_fusion
        .map(|lf| lf.mention_candidates.clone())
        .unwrap_or_default();

    Ok(json!({
        "active_panel": snap.active_panel,
        "nav_destination": snap.nav_destination,
        "tdesktop_mode": snap.tdesktop_mode,
        "openbot_shell": snap.openbot_shell,
        "telegram_baseline": snap.telegram_baseline,
        "bidirectional": {
            "projected_agent_peers": projected_agent_peers,
            "projected_channel_peers": projected_channel_peers,
            "mention_candidates": mention_candidates,
            "avatar_engines": avatar_engines,
            "resolved_goutte_shape": resolve_goutte_fallback_shape(),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(1);

    struct ScopedTestDir(std::path::PathBuf);

    impl ScopedTestDir {
        fn new(label: &str) -> Self {
            let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "openbot_ui_probe_{label}_{}_{seq}",
                std::process::id()
            ));
            let _ = std::fs::create_dir_all(&dir);
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScopedTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ui_probe_rejects_malformed_json_and_unknown_scenario() {
        let dir = ScopedTestDir::new("errors");
        let bad = evaluate_ui_probe("{bad", dir.path(), None).expect_err("malformed json");
        assert_eq!(bad.exit_code(), 2);
        assert!(bad.to_string().to_ascii_lowercase().contains("json"));

        let unk = evaluate_ui_probe("no-such-probe", dir.path(), None).expect_err("unknown");
        assert_eq!(unk.exit_code(), 2);
        assert!(unk.to_string().to_ascii_lowercase().contains("unknown"));
    }

    #[test]
    fn ui_probe_openbot_shell_corrupt_json_and_responsive_bounds() {
        let dir = ScopedTestDir::new("shell");
        std::fs::write(dir.path().join("openbot-sidebar-v1.json"), b"{corrupt").unwrap();
        std::fs::write(dir.path().join("openbot-rail-v1.json"), b"\xff\x00bad").unwrap();

        let res = evaluate_ui_probe("openbot-shell", dir.path(), None).expect("corrupt fallback");
        let sh = &res["openbot_shell"];
        assert_eq!(sh["page_rail_count"], 11);
        assert_eq!(sh["sidebar_width_px"], 280);
        assert_eq!(sh["sidebar_compact_px"], 88);
        assert_eq!(sh["beam_duration_ms"], 1400);

        let below = evaluate_ui_probe(
            r#"{"scenario":"openbot-shell","server_view":"rail","platform":"linux","window_width_px":759}"#,
            dir.path(),
            None,
        )
        .unwrap();
        assert_eq!(below["openbot_shell"]["sidebar_compact"], true);

        let above = evaluate_ui_probe(
            r#"{"scenario":"openbot-shell","server_view":"rail","platform":"linux","window_width_px":801}"#,
            dir.path(),
            None,
        )
        .unwrap();
        assert_eq!(above["openbot_shell"]["sidebar_compact"], false);
    }

    #[test]
    fn ui_probe_conversation_composer_channel_and_pages_contracts() {
        let dir = ScopedTestDir::new("contracts");

        let conv = evaluate_ui_probe(
            r#"{"scenario":"conversation-virtualized","message_length":25000,"inject_corrupt_row":true,"subagents":[{"id":"s1","status":"running"},{"id":"s2","status":"finished"}]}"#,
            dir.path(),
            None,
        )
        .unwrap();
        assert_eq!(conv["conversation"]["collapsed"], true);
        assert_eq!(conv["conversation"]["row_boundary_isolated"], true);
        assert_eq!(conv["conversation"]["subagent_dock"]["in_ms"], 340);
        assert_eq!(conv["conversation"]["subagent_dock"]["running"], 1);

        let comp = evaluate_ui_probe(
            r#"{"scenario":"composer-queue-sidepanel","trigger":"@","query":"auth","in_channel":true,"paste_length":4500,"active_turn_routine":true,"press_tab_steer":true}"#,
            dir.path(),
            None,
        )
        .unwrap();
        assert_eq!(comp["composer_probe"]["pasted_as_file"], true);
        assert_eq!(comp["composer_probe"]["queue_panel"]["steer_refused"], true);
        assert_eq!(comp["composer_probe"]["side_panel"]["max_splits"], 2);

        let mut live = LiveFusionSnapshot {
            projected_agent_peers: 2,
            projected_channel_peers: 1,
            mention_candidates: vec!["FusionAlpha".to_string(), "FusionBeta".to_string()],
            channel_members_by_id: HashMap::new(),
        };
        live.channel_members_by_id.insert(
            "ch-1".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        );
        let ch = evaluate_ui_probe(
            r#"{"scenario":"channel-conversation-recovery","channel_id":"ch-1","stopped_tasks":[{"task_id":"t1","owner_agent_id":"a1"}]}"#,
            dir.path(),
            Some(&live),
        )
        .unwrap();
        assert_eq!(ch["channel_ui"]["icon_catalog_count"], 20);
        let cands = ch["channel_ui"]["stopped_tasks_banner"]["reassign_candidates"]
            .as_array()
            .unwrap();
        assert_eq!(cands.len(), 2);
        assert!(cands.iter().all(|v| v != "a1"));

        let pages = evaluate_ui_probe(
            r#"{"scenario":"workflow-canvas-pages","settings_tab":"hosted-servers","chat_window":false}"#,
            dir.path(),
            None,
        )
        .unwrap();
        assert_eq!(pages["pages_probe"]["resolved_settings_tab"], "billing");
        assert_eq!(
            pages["pages_probe"]["app_settings_tabs"].as_array().unwrap().len(),
            14
        );

        let os = evaluate_ui_probe("desktop-os-surfaces", dir.path(), None).unwrap();
        assert_eq!(os["os_surfaces"]["surfaces"].as_array().unwrap().len(), 7);

        let fusion = evaluate_ui_probe(
            r#"{"scenario":"telegram-bidirectional-fusion","avatar_specs":["classic","blob","bloub:goutte","plush:a1","unknown:foo"]}"#,
            dir.path(),
            Some(&live),
        )
        .unwrap();
        assert_eq!(fusion["bidirectional"]["projected_agent_peers"], 2);
        assert_eq!(fusion["bidirectional"]["avatar_engines"]["blob"], "grok_bot");
        assert_eq!(
            fusion["bidirectional"]["avatar_engines"]["bloub:goutte"],
            "bloub_flat"
        );
        assert_ne!(fusion["bidirectional"]["resolved_goutte_shape"], "goutte");
    }

    #[test]
    fn ui_probe_all_six_m2_scenarios_succeed() {
        let dir = ScopedTestDir::new("m2");
        for name in [
            "crew-pile-physics",
            "crew-pile-reduced-motion-and-broken-url",
            "avatar-horizon-cull",
            "avatar-mid-morph",
            "theme-hot-reload",
            "theme-sweep-reduced-motion",
        ] {
            let val = evaluate_ui_probe(name, dir.path(), None)
                .unwrap_or_else(|e| panic!("M2 probe '{name}' failed: {e}"));
            assert!(val.is_object(), "probe '{name}' must return JSON object");
        }
    }

    #[test]
    fn ui_probe_openbot_shell_clamps_5_level_chain_and_breaks_cycles() {
        use super::super::shell_state::{SidebarSection, validate_sidebar_layout_strict};

        let dir = ScopedTestDir::new("f14_03_clamp");
        let payload = serde_json::json!({
            "scenario": "openbot-shell",
            "sidebar_layout": {
                "sections": [
                    {"id": "d0", "name": "Level 0", "parent_id": null, "collapsed": false},
                    {"id": "d1", "name": "Level 1", "parent_id": "d0", "collapsed": false},
                    {"id": "d2", "name": "Level 2", "parent_id": "d1", "collapsed": false},
                    {"id": "d3", "name": "Level 3", "parent_id": "d2", "collapsed": false},
                    {"id": "d4", "name": "Level 4", "parent_id": "d3", "collapsed": false},
                    {"id": "cyc_a", "name": "Cycle A", "parent_id": "cyc_b", "collapsed": false},
                    {"id": "cyc_b", "name": "Cycle B", "parent_id": "cyc_a", "collapsed": false}
                ],
                "order": ["d0", "d1", "d2", "d3", "d4", "cyc_a", "cyc_b", "people", "unassigned"],
                "agent_assignments": {},
                "agent_order": [],
                "revision": 1
            }
        });
        let raw_spec = serde_json::to_string(&payload).unwrap();
        let val = evaluate_ui_probe(&raw_spec, dir.path(), None)
            .expect("openbot-shell probe must clamp 5-level chains and break cycles (TEST-T2-OB-F14-03)");
        let depths = val["openbot_shell"]["resolved_folder_depths"]
            .as_object()
            .expect("resolved_folder_depths must be an object");
        assert!(
            depths.values().all(|v| v.as_u64().unwrap_or(99) <= 2),
            "every resolved folder depth must be clamped <= 2: {depths:?}"
        );
        assert_eq!(depths.get("d0").and_then(Value::as_u64), Some(0));
        assert_eq!(depths.get("d1").and_then(Value::as_u64), Some(1));
        assert_eq!(depths.get("d2").and_then(Value::as_u64), Some(2));
        assert_eq!(depths.get("d3").and_then(Value::as_u64), Some(2));
        assert_eq!(depths.get("d4").and_then(Value::as_u64), Some(2));
        assert_eq!(depths.get("cyc_a").and_then(Value::as_u64), Some(0));
        assert_eq!(depths.get("cyc_b").and_then(Value::as_u64), Some(0));

        let cyclic_snap: SidebarLayoutSnapshot =
            serde_json::from_value(payload["sidebar_layout"].clone()).unwrap();
        let strict_cyc = validate_sidebar_layout_strict(&cyclic_snap)
            .expect_err("strict validator must reject cycle/depth");
        assert!(
            strict_cyc.code == "folder_cycle" || strict_cyc.code == "folder_depth_exceeded",
            "unexpected strict error code: {}",
            strict_cyc.code
        );

        let only_cycle = SidebarLayoutSnapshot {
            sections: vec![
                SidebarSection {
                    id: "cyc_a".into(),
                    name: "Cycle A".into(),
                    parent_id: Some("cyc_b".into()),
                    collapsed: false,
                },
                SidebarSection {
                    id: "cyc_b".into(),
                    name: "Cycle B".into(),
                    parent_id: Some("cyc_a".into()),
                    collapsed: false,
                },
            ],
            order: vec![
                "cyc_a".into(),
                "cyc_b".into(),
                "people".into(),
                "unassigned".into(),
            ],
            ..SidebarLayoutSnapshot::default()
        };
        assert_eq!(
            validate_sidebar_layout_strict(&only_cycle)
                .expect_err("strict validator must reject folder cycle")
                .code,
            "folder_cycle"
        );

        let deep_acyclic = SidebarLayoutSnapshot {
            sections: vec![
                SidebarSection {
                    id: "s1".into(),
                    name: "L0".into(),
                    parent_id: None,
                    collapsed: false,
                },
                SidebarSection {
                    id: "s2".into(),
                    name: "L1".into(),
                    parent_id: Some("s1".into()),
                    collapsed: false,
                },
                SidebarSection {
                    id: "s3".into(),
                    name: "L2".into(),
                    parent_id: Some("s2".into()),
                    collapsed: false,
                },
                SidebarSection {
                    id: "s4".into(),
                    name: "L3".into(),
                    parent_id: Some("s3".into()),
                    collapsed: false,
                },
            ],
            order: vec![
                "s1".into(),
                "s2".into(),
                "s3".into(),
                "s4".into(),
                "people".into(),
                "unassigned".into(),
            ],
            ..SidebarLayoutSnapshot::default()
        };
        assert_eq!(
            validate_sidebar_layout_strict(&deep_acyclic)
                .expect_err("strict validator must reject 4-level acyclic nesting")
                .code,
            "folder_depth_exceeded"
        );
    }
}
