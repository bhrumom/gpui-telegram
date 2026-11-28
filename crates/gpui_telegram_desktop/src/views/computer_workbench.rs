use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::remote_control_contracts::{
    ColorSubsampling, DisplayInfoDto, FileConflictPolicy, FileTransferDirection,
    FileTransferStatus, FileTransferTaskDto, PowerActionKind, RemoteStreamSettingsDto,
    StreamCodec, StreamQuality,
};
use gpui_telegram_proto::{
    ComputerNodeDto, ComputerNodeKind, ComputerRunPolicy, TailnetPeerLiveState, TailnetRelayMode,
};

use crate::components::macos_permission_modal::{
    render_gemini_permission_modal, render_remote_host_permission_modal,
};
use crate::components::ui::TextInputController;
use crate::i18n::tr_str;
use crate::platform::mac::macos_permissions_drag::{
    ScreenPermissionState, cached_permissions_snapshot, hide_drag_hud,
    permissions_snapshot_generation, request_permissions_refresh,
};
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::remote_terminal_view::{RemoteTerminalAction, RemoteTerminalView};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDeviceSpec {
    pub name: String,
    pub ip_address: String,
    pub os_name: String,
    pub resolution: String,
    pub latency_ms: u32,
    pub fps: u32,
    pub is_tunnel_encrypted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkbenchTab {
    #[default]
    DesktopScreen,
    RemoteTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLogEntry {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseControlMode {
    #[default]
    Absolute,
    GameRelative,
    CursorLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkbenchHudPopover {
    Quality,
    Displays,
    Hotkeys,
    Audio,
    Power,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkbenchHotkeyPreset {
    CtrlAltDel,
    AppSwitcher,
    SpotlightOrStart,
    Copy,
    Paste,
    CloseWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkbenchCommand {
    RefreshScreen,
    SendGuiClick { x: i32, y: i32 },
    SendGuiText { text: String },
    RunShell { command: String, cwd: Option<String> },
    ListFiles { path: String },
    ReadRemoteFile { path: String },
    SetStreamSettings(RemoteStreamSettingsDto),
    ListDisplays,
    SwitchDisplay { display_index: u32 },
    ToggleVirtualDisplay {
        enabled: bool,
        width: u32,
        height: u32,
    },
    TogglePrivacyScreen {
        enabled: bool,
        message: Option<String>,
    },
    SendRelativeMouseDelta { dx: i32, dy: i32 },
    SendHotkey { key: String, modifiers: Vec<String> },
    PowerAction(PowerActionKind),
    TakeScreenshot,
    FileTransferAction {
        transfer_id: String,
        action: String,
        conflict_policy: FileConflictPolicy,
    },
    TerminalAction(RemoteTerminalAction),
}

/// Decodes an inline `data:image/<fmt>;base64,...` frame URL into a cached GPUI `Image`.
pub fn decode_data_url_image(data_url: &str) -> Option<Arc<gpui::Image>> {
    let rest = data_url.strip_prefix("data:image/")?;
    let (meta, b64) = rest.split_once(',')?;
    let meta_lower = meta.to_ascii_lowercase();
    if !meta_lower.contains(";base64") {
        return None;
    }
    let fmt = if meta_lower.starts_with("png") {
        gpui::ImageFormat::Png
    } else if meta_lower.starts_with("jpeg") || meta_lower.starts_with("jpg") {
        gpui::ImageFormat::Jpeg
    } else if meta_lower.starts_with("webp") {
        gpui::ImageFormat::Webp
    } else if meta_lower.starts_with("gif") {
        gpui::ImageFormat::Gif
    } else {
        gpui::ImageFormat::Png
    };
    let bytes = decode_base64_bytes(b64)?;
    if bytes.is_empty() {
        return None;
    }
    Some(Arc::new(gpui::Image::from_bytes(fmt, bytes)))
}

fn decode_base64_bytes(input: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let mut cleaned = Vec::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        cleaned.push(b);
    }
    if cleaned.is_empty() || cleaned.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity((cleaned.len() * 3) / 4);
    let mut i = 0;
    while i < cleaned.len() {
        let a = val(cleaned[i])?;
        let b = val(*cleaned.get(i + 1)?)?;
        let c_byte = *cleaned.get(i + 2).unwrap_or(&b'=');
        let d_byte = *cleaned.get(i + 3).unwrap_or(&b'=');
        out.push((a << 2) | (b >> 4));
        if c_byte != b'=' {
            let c = val(c_byte)?;
            out.push((b << 4) | (c >> 2));
            if d_byte != b'=' {
                let d = val(d_byte)?;
                out.push((c << 6) | d);
            }
        }
        i += 4;
    }
    Some(out)
}

/// Renders an editable field's committed text, IME preedit underline, and caret indicator.
pub fn render_ime_text_preview(
    committed: &str,
    preedit: Option<&str>,
    placeholder: &str,
    focused: bool,
    masked: bool,
    text_color: u32,
    pal: TdesktopThemePalette,
) -> AnyElement {
    let display_committed = if masked {
        "•".repeat(committed.chars().count())
    } else {
        committed.to_string()
    };
    let pre = preedit.filter(|s| !s.is_empty());
    let is_empty = display_committed.is_empty() && pre.is_none();

    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(1.0));

    if is_empty {
        row = row.child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(pal.text_secondary))
                .child(placeholder.to_string()),
        );
    } else {
        if !display_committed.is_empty() {
            row = row.child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(text_color))
                    .child(display_committed),
            );
        }
        if let Some(p) = pre {
            row = row.child(
                div()
                    .px(px(2.0))
                    .rounded(px(3.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18))
                    .border_b_2()
                    .border_color(rgb(pal.accent))
                    .text_size(px(12.5))
                    .text_color(rgb(text_color))
                    .child(p.to_string()),
            );
        }
    }

    if focused {
        row = row.child(
            div()
                .w(px(1.5))
                .h(px(14.0))
                .rounded(px(1.0))
                .bg(rgb(pal.accent)),
        );
    }
    row.into_any_element()
}

/// Parses `"1920x1080 @ 60Hz"` or `"1440×900"` into `(width, height)`.
fn parse_resolution_dims(raw: &str) -> Option<(f32, f32)> {
    let token = raw.split_whitespace().next()?;
    let (w_str, h_str) = token
        .split_once('x')
        .or_else(|| token.split_once('X'))
        .or_else(|| token.split_once('×'))?;
    let w: f32 = w_str.trim().parse().ok()?;
    let h: f32 = h_str.trim().parse().ok()?;
    if w >= 64.0 && h >= 64.0 {
        Some((w, h))
    } else {
        None
    }
}

pub struct ComputerWorkbench {
    pub device: HostDeviceSpec,
    pub is_mouse_captured: bool,
    pub is_keyboard_captured: bool,
    pub is_clipboard_synced: bool,
    pub is_quality_hd: bool,
    pub is_connected: bool,
    pub should_close: bool,
    pub frame_source: Option<String>,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub pending_action: Option<String>,

    // Extended multi-device & cloud computer workbench state
    pub node_id: Option<String>,
    pub node_kind: ComputerNodeKind,
    pub run_policy: ComputerRunPolicy,
    pub active_tab: WorkbenchTab,
    pub terminal_input: Option<Entity<TextInputController>>,
    pub terminal_view: Option<Entity<RemoteTerminalView>>,
    pub terminal_draft: String,
    pub terminal_focused: bool,
    pub terminal_history: Vec<TerminalLogEntry>,
    pub remote_cwd: String,
    pub remote_files: Vec<String>,
    pub file_preview: Option<(String, String)>,
    pub status_banner: Option<String>,
    pub pending_command: Option<WorkbenchCommand>,
    pub pending_commands: VecDeque<WorkbenchCommand>,
    pub tailnet_peer: Option<TailnetPeerLiveState>,
    pub is_local_device: bool,
    pub needs_screen_permission_guide: bool,
    pub needs_accessibility_permission_guide: bool,
    pub force_local_permission_modal: bool,
    pub remote_permission_error: Option<String>,
    pub hud_expanded: bool,
    pub hud_active_popover: Option<WorkbenchHudPopover>,
    pub stream_settings: RemoteStreamSettingsDto,
    pub mouse_mode: MouseControlMode,
    pub virtual_display_enabled: bool,
    pub displays: Vec<DisplayInfoDto>,
    pub file_transfers: Vec<FileTransferTaskDto>,
    pub file_conflict_policy: FileConflictPolicy,
    pub file_drawer_open: bool,
    pub wol_mac_address: String,
    cached_frame_src: Option<String>,
    cached_frame_image: Option<Arc<gpui::Image>>,
    viewport_bounds: Arc<std::sync::Mutex<Option<Bounds<Pixels>>>>,
    permission_poll_started: bool,
    last_perm_gen: u64,
    last_perm_screen_state: ScreenPermissionState,
    last_perm_ax_active: bool,
    last_perm_hud_visible: bool,
    last_perm_stale_screen: bool,
    last_perm_stale_ax: bool,
    last_perm_drag_seq: u64,
}

impl ComputerWorkbench {
    pub fn new(device_name: String) -> Self {
        Self {
            device: HostDeviceSpec {
                name: device_name,
                ip_address: "10.0.4.12".to_string(),
                os_name: "Ubuntu 24.04 LTS (x86_64)".to_string(),
                resolution: "1920x1080 @ 60Hz".to_string(),
                latency_ms: 14,
                fps: 60,
                is_tunnel_encrypted: true,
            },
            is_mouse_captured: true,
            is_keyboard_captured: true,
            is_clipboard_synced: true,
            is_quality_hd: true,
            is_connected: true,
            should_close: false,
            frame_source: None,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12),
            language_id: "en".to_string(),
            pending_action: None,
            node_id: None,
            node_kind: ComputerNodeKind::LocalDevice,
            run_policy: ComputerRunPolicy::AskEveryTime,
            active_tab: WorkbenchTab::DesktopScreen,
            terminal_input: None,
            terminal_view: None,
            terminal_draft: String::new(),
            terminal_focused: false,
            terminal_history: Vec::new(),
            remote_cwd: "~".to_string(),
            remote_files: Vec::new(),
            file_preview: None,
            status_banner: None,
            pending_command: None,
            pending_commands: VecDeque::new(),
            tailnet_peer: None,
            is_local_device: false,
            needs_screen_permission_guide: false,
            needs_accessibility_permission_guide: false,
            force_local_permission_modal: false,
            remote_permission_error: None,
            hud_expanded: true,
            hud_active_popover: None,
            stream_settings: RemoteStreamSettingsDto::default(),
            mouse_mode: MouseControlMode::Absolute,
            virtual_display_enabled: false,
            displays: vec![DisplayInfoDto {
                display_id: 0,
                name: "Primary Display".to_string(),
                width: 1920,
                height: 1080,
                scale_factor_x100: 100,
                is_primary: true,
                is_virtual: false,
                refresh_hz: 60,
            }],
            file_transfers: Vec::new(),
            file_conflict_policy: FileConflictPolicy::Overwrite,
            file_drawer_open: false,
            wol_mac_address: String::new(),
            cached_frame_src: None,
            cached_frame_image: None,
            viewport_bounds: Arc::new(std::sync::Mutex::new(None)),
            permission_poll_started: false,
            last_perm_gen: 0,
            last_perm_screen_state: ScreenPermissionState::NotGranted,
            last_perm_ax_active: false,
            last_perm_hud_visible: false,
            last_perm_stale_screen: false,
            last_perm_stale_ax: false,
            last_perm_drag_seq: 0,
        }
    }

    pub fn new_live(device_name: String, palette: TdesktopThemePalette, language_id: &str) -> Self {
        let mut s = Self::new(device_name);
        s.device.ip_address = String::new();
        s.device.os_name = String::new();
        s.device.resolution = String::new();
        s.device.latency_ms = 0;
        s.device.fps = 0;
        s.displays.clear();
        s.is_connected = false;
        s.palette = palette;
        s.language_id = language_id.to_string();
        s
    }

    pub fn ensure_permission_poll_loop(&mut self, cx: &mut Context<Self>) {
        if self.permission_poll_started
            || !cfg!(target_os = "macos")
            || cfg!(test)
            || std::env::var_os("GPUI_OFFLINE_TEST").is_some()
        {
            return;
        }
        self.permission_poll_started = true;
        let init_snap = cached_permissions_snapshot();
        self.last_perm_gen = permissions_snapshot_generation();
        self.last_perm_screen_state = init_snap.screen_state;
        self.last_perm_ax_active = init_snap.accessibility_granted;
        self.last_perm_hud_visible = init_snap.hud_visible;
        self.last_perm_stale_screen = init_snap.stale_screen_suspected;
        self.last_perm_stale_ax = init_snap.stale_ax_suspected;
        self.last_perm_drag_seq = init_snap.last_drag_seq;

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| loop {
            app.background_executor()
                .timer(Duration::from_millis(450))
                .await;
            let keep = weak
                .update(app, |this, cx| {
                    let before_snap = cached_permissions_snapshot();
                    let modal_open = this.needs_screen_permission_guide
                        || this.needs_accessibility_permission_guide
                        || this.force_local_permission_modal;
                    if (modal_open && (this.is_local_device || this.force_local_permission_modal))
                        || before_snap.hud_visible
                    {
                        request_permissions_refresh(false);
                        let snap = cached_permissions_snapshot();
                        let perm_gen = permissions_snapshot_generation();
                        let screen_state = snap.screen_state;
                        let ax_active = snap.accessibility_granted;
                        let changed = perm_gen != this.last_perm_gen
                            || screen_state != this.last_perm_screen_state
                            || ax_active != this.last_perm_ax_active
                            || snap.hud_visible != this.last_perm_hud_visible
                            || snap.stale_screen_suspected != this.last_perm_stale_screen
                            || snap.stale_ax_suspected != this.last_perm_stale_ax
                            || snap.last_drag_seq != this.last_perm_drag_seq;

                        if changed {
                            let prev_screen_active = this.last_perm_screen_state.is_active();
                            let screen_just_became_active =
                                !prev_screen_active && screen_state.is_active();
                            this.last_perm_gen = perm_gen;
                            this.last_perm_screen_state = screen_state;
                            this.last_perm_ax_active = ax_active;
                            this.last_perm_hud_visible = snap.hud_visible;
                            this.last_perm_stale_screen = snap.stale_screen_suspected;
                            this.last_perm_stale_ax = snap.stale_ax_suspected;
                            this.last_perm_drag_seq = snap.last_drag_seq;

                            if ax_active {
                                this.needs_accessibility_permission_guide = false;
                            }
                            if screen_state.needs_restart() && this.is_local_device {
                                this.status_banner = Some(
                                    tr_str(
                                        &this.language_id,
                                        "录屏权限已在系统设置中开启，请点击「重启 Fabushi」使其生效",
                                        "Screen Recording enabled in Settings — click Restart Fabushi to apply",
                                    )
                                    .to_string(),
                                );
                            }
                            if screen_just_became_active {
                                this.needs_screen_permission_guide = false;
                                if ax_active {
                                    this.force_local_permission_modal = false;
                                }
                                this.enqueue_command(WorkbenchCommand::RefreshScreen);
                            }
                            cx.notify();
                        }
                    }
                    true
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    pub fn ensure_terminal_input(&mut self, cx: &mut Context<Self>) {
        if self.terminal_input.is_none() {
            self.terminal_input = Some(cx.new(|_| TextInputController::new()));
        }
    }

    pub fn ensure_terminal_view(&mut self, cx: &mut Context<Self>) {
        if self.terminal_view.is_none() {
            let pal = self.palette;
            let lang = self.language_id.clone();
            let entity = cx.new(|_| RemoteTerminalView::new(pal, &lang));
            cx.observe(&entity, |this, term, cx| {
                let acts = term.update(cx, |t, _| t.take_actions());
                for act in acts {
                    this.enqueue_command(WorkbenchCommand::TerminalAction(act));
                }
                cx.notify();
            })
            .detach();
            self.terminal_view = Some(entity);
        }
    }

    pub fn sync_lease(
        &mut self,
        device: HostDeviceSpec,
        frame_source: Option<String>,
        connected: bool,
    ) {
        self.device = device;
        self.update_frame_url(frame_source);
        self.is_connected = connected;
    }

    pub fn set_target_node(&mut self, node: &ComputerNodeDto) {
        if self.node_id.as_deref() != Some(node.node_id.as_str()) {
            self.needs_screen_permission_guide = false;
            self.needs_accessibility_permission_guide = false;
            self.force_local_permission_modal = false;
            self.remote_permission_error = None;
        }
        self.node_id = Some(node.node_id.clone());
        self.node_kind = node.kind;
        self.is_local_device = node.is_current_device;
        self.run_policy = node.run_policy;
        self.is_connected = node.online;
        self.tailnet_peer = None;
        let ip_or_host = node
            .host
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&node.ip_address)
            .to_string();
        self.device = HostDeviceSpec {
            name: node.display_name.clone(),
            ip_address: ip_or_host,
            os_name: node.os_name.clone(),
            resolution: if !node.resolution.is_empty() {
                node.resolution.clone()
            } else if node.latest_frame_data_url.is_some() {
                "1280x800 @ Live".to_string()
            } else {
                "Remote Control".to_string()
            },
            latency_ms: if node.online { 18 } else { 0 },
            fps: if node.online { 30 } else { 0 },
            is_tunnel_encrypted: true,
        };
        if let Some(wd) = node.workdir.as_ref().filter(|s| !s.trim().is_empty()) {
            self.remote_cwd = wd.clone();
        }
        self.update_frame_url(node.latest_frame_data_url.clone());
    }

    pub fn refresh_target_node_status(&mut self, node: &ComputerNodeDto) {
        self.run_policy = node.run_policy;
        self.is_connected = node.online;
        self.device.name = node.display_name.clone();
        self.device.ip_address = node
            .host
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&node.ip_address)
            .to_string();
        if node.latest_frame_data_url.is_some() {
            self.update_frame_url(node.latest_frame_data_url.clone());
        }
    }

    pub fn sync_tailnet_peer_state(&mut self, peer: Option<&TailnetPeerLiveState>) {
        self.tailnet_peer = peer.cloned();
        if let Some(peer) = peer {
            if peer.online {
                self.is_connected = true;
            }
            if !peer.tailnet_ip.trim().is_empty() {
                self.device.ip_address = peer.tailnet_ip.clone();
            }
            if let Some(rtt) = peer.rtt_ms {
                self.device.latency_ms = rtt;
            }
            self.device.is_tunnel_encrypted = true;
        }
    }

    pub fn update_frame_url(&mut self, url: Option<String>) {
        if self.cached_frame_src != url {
            self.cached_frame_image = url.as_deref().and_then(decode_data_url_image);
            self.cached_frame_src = url.clone();
        }
        self.frame_source = url;
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }
    pub fn set_language(&mut self, language_id: impl Into<String>) {
        self.language_id = language_id.into();
    }
    pub fn take_action(&mut self) -> Option<String> {
        self.pending_action.take()
    }
    pub fn take_command(&mut self) -> Option<WorkbenchCommand> {
        self.pending_command.take()
    }
    pub fn enqueue_command(&mut self, cmd: WorkbenchCommand) {
        if let Some(prev) = self.pending_command.take() {
            self.pending_commands.push_back(prev);
        }
        self.pending_command = Some(cmd);
    }
    pub fn take_commands(&mut self) -> Vec<WorkbenchCommand> {
        let mut out: Vec<WorkbenchCommand> = self.pending_commands.drain(..).collect();
        if let Some(last) = self.pending_command.take() {
            out.push(last);
        }
        out
    }
    pub fn toggle_mouse_capture(&mut self) {
        self.is_mouse_captured = !self.is_mouse_captured;
    }
    pub fn toggle_keyboard_capture(&mut self) {
        self.is_keyboard_captured = !self.is_keyboard_captured;
    }
    pub fn toggle_clipboard_sync(&mut self) {
        self.is_clipboard_synced = !self.is_clipboard_synced;
    }
    pub fn toggle_quality(&mut self) {
        self.is_quality_hd = !self.is_quality_hd;
        self.stream_settings.quality = if self.is_quality_hd {
            StreamQuality::Hd
        } else {
            StreamQuality::Smooth
        };
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_hud_expanded(&mut self) {
        self.hud_expanded = !self.hud_expanded;
        if !self.hud_expanded {
            self.hud_active_popover = None;
        }
    }
    pub fn set_stream_fps(&mut self, fps: u32) {
        self.stream_settings.target_fps = fps;
        self.device.fps = fps;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn set_stream_quality(&mut self, quality: StreamQuality) {
        self.stream_settings.quality = quality;
        self.is_quality_hd = matches!(
            quality,
            StreamQuality::Hd | StreamQuality::Ultra4K | StreamQuality::Original
        );
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn set_stream_codec(&mut self, codec: StreamCodec) {
        self.stream_settings.codec = codec;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_truecolor_444(&mut self) {
        self.stream_settings.color_mode = match self.stream_settings.color_mode {
            ColorSubsampling::Yuv420 => ColorSubsampling::TrueColor444,
            ColorSubsampling::TrueColor444 => ColorSubsampling::Yuv420,
        };
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_hdr(&mut self) {
        self.stream_settings.hdr = !self.stream_settings.hdr;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn set_mouse_mode(&mut self, mode: MouseControlMode) {
        self.mouse_mode = mode;
        self.stream_settings.relative_mouse =
            matches!(mode, MouseControlMode::GameRelative | MouseControlMode::CursorLock);
        self.stream_settings.lock_cursor_in_window = matches!(mode, MouseControlMode::CursorLock);
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_macos_key_remap(&mut self) {
        self.stream_settings.macos_hotkey_mapping = !self.stream_settings.macos_hotkey_mapping;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_audio_enabled(&mut self) {
        self.stream_settings.audio_enabled = !self.stream_settings.audio_enabled;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_mute_remote_speaker(&mut self) {
        self.stream_settings.mute_remote_speaker = !self.stream_settings.mute_remote_speaker;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn toggle_mic_passthrough(&mut self) {
        self.stream_settings.mic_passthrough = !self.stream_settings.mic_passthrough;
        self.enqueue_command(WorkbenchCommand::SetStreamSettings(
            self.stream_settings.clone(),
        ));
    }
    pub fn set_displays(&mut self, displays: Vec<DisplayInfoDto>) {
        if !displays.is_empty() {
            self.displays = displays;
        }
    }
    pub fn switch_display(&mut self, display_index: u32) {
        self.stream_settings.display_index = display_index;
        self.hud_active_popover = None;
        self.enqueue_command(WorkbenchCommand::SwitchDisplay { display_index });
    }
    pub fn toggle_virtual_display(&mut self) {
        self.virtual_display_enabled = !self.virtual_display_enabled;
        let enabled = self.virtual_display_enabled;
        self.enqueue_command(WorkbenchCommand::ToggleVirtualDisplay {
            enabled,
            width: 1920,
            height: 1080,
        });
    }
    pub fn toggle_privacy_screen(&mut self) {
        self.stream_settings.privacy_screen = !self.stream_settings.privacy_screen;
        let enabled = self.stream_settings.privacy_screen;
        self.enqueue_command(WorkbenchCommand::TogglePrivacyScreen {
            enabled,
            message: if enabled {
                Some("Remote session privacy screen active".to_string())
            } else {
                None
            },
        });
    }
    pub fn trigger_hotkey(&mut self, preset: WorkbenchHotkeyPreset) {
        let remap_to_meta = self.stream_settings.macos_hotkey_mapping
            && self.device.os_name.to_ascii_lowercase().contains("mac");
        let (key, modifiers): (&str, Vec<String>) = match preset {
            WorkbenchHotkeyPreset::CtrlAltDel => (
                "Delete",
                vec!["Control".to_string(), "Alt".to_string()],
            ),
            WorkbenchHotkeyPreset::AppSwitcher => (
                "Tab",
                vec![if remap_to_meta { "Meta" } else { "Alt" }.to_string()],
            ),
            WorkbenchHotkeyPreset::SpotlightOrStart => (
                if remap_to_meta { "Space" } else { "Meta" },
                if remap_to_meta {
                    vec!["Meta".to_string()]
                } else {
                    vec![]
                },
            ),
            WorkbenchHotkeyPreset::Copy => (
                "c",
                vec![if remap_to_meta { "Meta" } else { "Control" }.to_string()],
            ),
            WorkbenchHotkeyPreset::Paste => (
                "v",
                vec![if remap_to_meta { "Meta" } else { "Control" }.to_string()],
            ),
            WorkbenchHotkeyPreset::CloseWindow => (
                if remap_to_meta { "w" } else { "F4" },
                vec![if remap_to_meta { "Meta" } else { "Alt" }.to_string()],
            ),
        };
        self.hud_active_popover = None;
        self.enqueue_command(WorkbenchCommand::SendHotkey {
            key: key.to_string(),
            modifiers,
        });
    }
    pub fn trigger_power_action(&mut self, action: PowerActionKind) {
        self.hud_active_popover = None;
        self.enqueue_command(WorkbenchCommand::PowerAction(action));
    }
    pub fn trigger_wake_on_lan(&mut self) {
        let mac = if self.wol_mac_address.trim().is_empty() {
            "FF:FF:FF:FF:FF:FF".to_string()
        } else {
            self.wol_mac_address.trim().to_string()
        };
        self.trigger_power_action(PowerActionKind::WakeOnLan {
            mac_address: mac,
            broadcast_ip: None,
        });
    }
    pub fn trigger_screenshot(&mut self) {
        self.enqueue_command(WorkbenchCommand::TakeScreenshot);
    }
    pub fn set_file_conflict_policy(&mut self, policy: FileConflictPolicy) {
        self.file_conflict_policy = policy;
        for task in &mut self.file_transfers {
            if matches!(task.status, FileTransferStatus::Queued | FileTransferStatus::Paused) {
                task.conflict_policy = policy;
            }
        }
    }
    pub fn upsert_file_transfer(&mut self, task: FileTransferTaskDto) {
        if let Some(existing) = self
            .file_transfers
            .iter_mut()
            .find(|t| t.transfer_id == task.transfer_id)
        {
            *existing = task;
        } else {
            self.file_transfers.push(task);
        }
    }
    pub fn resume_file_transfer(&mut self, transfer_id: &str) {
        if let Some(t) = self
            .file_transfers
            .iter_mut()
            .find(|t| t.transfer_id == transfer_id)
        {
            t.status = FileTransferStatus::Running;
        }
        self.enqueue_command(WorkbenchCommand::FileTransferAction {
            transfer_id: transfer_id.to_string(),
            action: "resume".to_string(),
            conflict_policy: self.file_conflict_policy,
        });
    }
    pub fn pause_file_transfer(&mut self, transfer_id: &str) {
        if let Some(t) = self
            .file_transfers
            .iter_mut()
            .find(|t| t.transfer_id == transfer_id)
        {
            t.status = FileTransferStatus::Paused;
        }
        self.enqueue_command(WorkbenchCommand::FileTransferAction {
            transfer_id: transfer_id.to_string(),
            action: "pause".to_string(),
            conflict_policy: self.file_conflict_policy,
        });
    }

    pub fn push_terminal_output(
        &mut self,
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
        new_cwd: Option<String>,
    ) {
        if let Some(cwd) = new_cwd.filter(|s| !s.trim().is_empty()) {
            self.remote_cwd = cwd;
        }
        self.terminal_history.push(TerminalLogEntry {
            command,
            stdout,
            stderr,
            exit_code,
            cwd: self.remote_cwd.clone(),
        });
        if self.terminal_history.len() > 80 {
            let excess = self.terminal_history.len() - 80;
            self.terminal_history.drain(0..excess);
        }
    }

    pub fn set_remote_files(&mut self, cwd: String, files: Vec<String>) {
        if !cwd.trim().is_empty() {
            self.remote_cwd = cwd;
        }
        self.remote_files = files;
    }

    pub fn set_file_preview(&mut self, path: String, content: String) {
        self.file_preview = Some((path, content));
    }

    pub fn submit_terminal_draft(&mut self) -> bool {
        let cmd = self.terminal_draft.trim().to_string();
        if cmd.is_empty() {
            return false;
        }
        self.terminal_draft.clear();
        let cwd = Some(self.remote_cwd.clone()).filter(|s| !s.is_empty());
        self.pending_command = Some(WorkbenchCommand::RunShell { command: cmd, cwd });
        self.pending_action = Some("command_enqueued".to_string());
        true
    }

    pub fn is_terminal_input_active(&self) -> bool {
        self.active_tab == WorkbenchTab::RemoteTerminal && self.terminal_focused
    }

    pub fn is_interactive_pty_focused(&self, cx: &App) -> bool {
        self.active_tab == WorkbenchTab::RemoteTerminal
            && self
                .terminal_view
                .as_ref()
                .is_some_and(|t| {
                    let view = t.read(cx);
                    view.is_focused || view.renaming_tab_index.is_some()
                })
    }

    pub fn feed_pty_output(
        &mut self,
        session_id: Option<&str>,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) {
        if let Some(term) = self.terminal_view.as_ref() {
            term.update(cx, |t, tcx| {
                let target_id = session_id
                    .map(|s| s.to_string())
                    .or_else(|| t.active_tab().map(|tab| tab.session_id.clone()));
                if let Some(id) = target_id {
                    t.feed_session_output(&id, bytes);
                }
                tcx.notify();
            });
            cx.notify();
        }
    }

    pub fn terminal_text_and_preedit(&self, cx: &App) -> (String, Option<String>) {
        if self.is_interactive_pty_focused(cx) {
            if let Some(term) = self.terminal_view.as_ref() {
                if let Some(pre) = term.read(cx).ime_preedit.clone() {
                    return (self.terminal_draft.clone(), Some(pre));
                }
            }
        }
        if let Some(ctrl) = self.terminal_input.as_ref() {
            return ctrl.read(cx).text_and_preedit(self.terminal_draft.clone());
        }
        (self.terminal_draft.clone(), None)
    }

    pub fn clear_terminal_input(&mut self, cx: &mut Context<Self>) {
        self.terminal_draft.clear();
        if let Some(ctrl) = self.terminal_input.as_ref() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(term) = self.terminal_view.as_ref() {
            term.update(cx, |t, tcx| {
                t.ime_preedit = None;
                tcx.notify();
            });
        }
        cx.notify();
    }

    pub fn push_terminal_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.is_interactive_pty_focused(cx) {
            if let Some(term) = self.terminal_view.as_ref() {
                let handled = term.update(cx, |t, tcx| {
                    let ok = t.push_committed_text(text);
                    if ok {
                        tcx.notify();
                    }
                    ok
                });
                if handled {
                    cx.notify();
                    return;
                }
            }
        }
        if let Some(ctrl) = self.terminal_input.as_ref() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        self.terminal_draft.push_str(text);
        cx.notify();
    }

    pub fn handle_terminal_key_stroke(
        &mut self,
        key: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
        platform: bool,
        key_char: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_interactive_pty_focused(cx) {
            return false;
        }
        if let Some(term) = self.terminal_view.as_ref() {
            let handled = term.update(cx, |t, tcx| {
                let ok = t.handle_key_stroke(key, shift, ctrl, alt, platform, key_char);
                if ok {
                    tcx.notify();
                }
                ok
            });
            if handled {
                cx.notify();
            }
            return handled;
        }
        false
    }

    pub fn handle_terminal_non_char_key(
        &mut self,
        key: &str,
        is_alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_terminal_input_active() {
            return false;
        }
        if self.is_interactive_pty_focused(cx)
            && self.handle_terminal_key_stroke(key, false, false, is_alt, false, None, cx)
        {
            return true;
        }
        match key {
            "backspace" | "delete" => {
                let changed = if is_alt {
                    crate::components::ui::delete_prev_word(&mut self.terminal_draft)
                } else {
                    crate::components::ui::pop_last_char(&mut self.terminal_draft)
                };
                if changed {
                    cx.notify();
                }
                true
            }
            "enter" => {
                if self.submit_terminal_draft() {
                    cx.notify();
                }
                true
            }
            _ => false,
        }
    }

    pub fn replace_and_mark_terminal_input(
        &mut self,
        new_text: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if self.is_interactive_pty_focused(cx) {
            if let Some(term) = self.terminal_view.as_ref() {
                term.update(cx, |t, tcx| {
                    t.ime_preedit = if new_text.is_empty() {
                        None
                    } else {
                        Some(new_text.to_string())
                    };
                    tcx.notify();
                });
                cx.notify();
                return None;
            }
        }
        self.ensure_terminal_input(cx);
        let commit = self
            .terminal_input
            .as_ref()
            .and_then(|c| c.update(cx, |ctrl, _| ctrl.handle_replace_and_mark(new_text)));
        if let Some(text) = commit.as_ref() {
            self.terminal_draft.push_str(text);
        }
        cx.notify();
        commit
    }

    fn hud_chip(
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        accent_hex: u32,
        pal: &TdesktopThemePalette,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(7.0))
            .py(px(3.5))
            .rounded(px(6.0))
            .bg(if active {
                TdesktopThemePalette::rgba_with_alpha(accent_hex, 0.24)
            } else {
                rgba(0xFFFFFF12)
            })
            .hover(|s| s.bg(rgba(0xFFFFFF24)))
            .border_1()
            .border_color(if active {
                rgb(accent_hex).into()
            } else {
                rgba(0xFFFFFF22)
            })
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if active { 0xFFFFFF } else { pal.text_primary }))
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                cx.stop_propagation();
                on_click(ev, window, cx);
            })
            .child(label.into())
    }

    fn render_floating_hud(
        &self,
        pal: &TdesktopThemePalette,
        lang: &str,
        dev: &HostDeviceSpec,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_zh = lang.starts_with("zh");
        let is_p2p = self
            .tailnet_peer
            .as_ref()
            .is_some_and(|p| p.relay_mode == TailnetRelayMode::DirectUdp);
        let relay_badge = match self.tailnet_peer.as_ref().map(|p| p.relay_mode) {
            Some(TailnetRelayMode::DirectUdp) => {
                if is_zh { "P2P直连" } else { "P2P" }
            }
            Some(TailnetRelayMode::DerpRelay) => {
                if is_zh { "DERP中继" } else { "DERP" }
            }
            Some(TailnetRelayMode::HttpFallback) => "HTTP",
            Some(TailnetRelayMode::Offline) | None => {
                if self.is_connected { "Live" } else { "Offline" }
            }
        };
        let stats_text = format!(
            "{} · {}ms · {}FPS",
            relay_badge, dev.latency_ms, self.stream_settings.target_fps
        );
        let pill_color = if is_p2p { 0x22C55E } else { pal.accent };

        if !self.hud_expanded {
            return div()
                .id("wb-hud-collapsed-pill")
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(999.0))
                .bg(rgba(0x0F172AE0))
                .border_1()
                .border_color(rgba(0xFFFFFF26))
                .shadow_lg()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded(px(999.0))
                        .bg(rgb(pill_color)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xF8FAFC))
                        .child(stats_text),
                )
                .child(Self::hud_chip(
                    "wb-hud-expand-btn",
                    tr_str(lang, "展开工具栏", "HUD"),
                    true,
                    pal.accent,
                    pal,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_hud_expanded();
                        cx.notify();
                    }),
                ))
                .into_any_element();
        }

        let mouse_label = match self.mouse_mode {
            MouseControlMode::Absolute => tr_str(lang, "绝对鼠标", "Abs"),
            MouseControlMode::GameRelative => tr_str(lang, "游戏相对", "Rel"),
            MouseControlMode::CursorLock => tr_str(lang, "锁定光标", "Lock"),
        };
        let next_mouse_mode = match self.mouse_mode {
            MouseControlMode::Absolute => MouseControlMode::GameRelative,
            MouseControlMode::GameRelative => MouseControlMode::CursorLock,
            MouseControlMode::CursorLock => MouseControlMode::Absolute,
        };
        let quality_label = format!(
            "{} {}fps{}",
            match self.stream_settings.quality {
                StreamQuality::Ultra4K | StreamQuality::Original => "4K",
                StreamQuality::Hd => "HD",
                StreamQuality::Smooth => "Smooth",
                StreamQuality::Auto => "Auto",
            },
            self.stream_settings.target_fps,
            if self.stream_settings.color_mode == ColorSubsampling::TrueColor444 {
                " 4:4:4"
            } else {
                ""
            }
        );

        let mut col = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("wb-hud-expanded-bar")
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(12.0))
                    .bg(rgba(0x0F172AE6))
                    .border_1()
                    .border_color(rgba(0xFFFFFF28))
                    .shadow_xl()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(3.5))
                            .rounded(px(999.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pill_color, 0.20))
                            .border_1()
                            .border_color(rgb(pill_color))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child(stats_text),
                    )
                    .child(Self::hud_chip(
                        "wb-hud-quality-btn",
                        quality_label,
                        self.hud_active_popover == Some(WorkbenchHudPopover::Quality),
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.hud_active_popover =
                                if this.hud_active_popover == Some(WorkbenchHudPopover::Quality) {
                                    None
                                } else {
                                    Some(WorkbenchHudPopover::Quality)
                                };
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-display-btn",
                        format!("{} #{}", tr_str(lang, "屏幕", "Disp"), self.stream_settings.display_index),
                        self.hud_active_popover == Some(WorkbenchHudPopover::Displays),
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.enqueue_command(WorkbenchCommand::ListDisplays);
                            this.hud_active_popover =
                                if this.hud_active_popover == Some(WorkbenchHudPopover::Displays) {
                                    None
                                } else {
                                    Some(WorkbenchHudPopover::Displays)
                                };
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-virtual-disp-btn",
                        tr_str(lang, "虚拟屏", "VirtDisp"),
                        self.virtual_display_enabled,
                        0x8B5CF6,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_virtual_display();
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-privacy-btn",
                        tr_str(lang, "隐私屏", "Privacy"),
                        self.stream_settings.privacy_screen,
                        0x14B8A6,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_privacy_screen();
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-mouse-mode-btn",
                        mouse_label,
                        self.mouse_mode != MouseControlMode::Absolute,
                        0xF59E0B,
                        pal,
                        cx.listener(move |this, _, _, cx| {
                            this.set_mouse_mode(next_mouse_mode);
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-hotkeys-btn",
                        tr_str(lang, "快捷键", "Hotkeys"),
                        self.hud_active_popover == Some(WorkbenchHudPopover::Hotkeys),
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.hud_active_popover =
                                if this.hud_active_popover == Some(WorkbenchHudPopover::Hotkeys) {
                                    None
                                } else {
                                    Some(WorkbenchHudPopover::Hotkeys)
                                };
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-audio-btn",
                        if self.stream_settings.audio_enabled {
                            tr_str(lang, "音频开", "Audio On")
                        } else {
                            tr_str(lang, "音频关", "Audio Off")
                        },
                        self.hud_active_popover == Some(WorkbenchHudPopover::Audio)
                            || self.stream_settings.audio_enabled,
                        0x22C55E,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.hud_active_popover =
                                if this.hud_active_popover == Some(WorkbenchHudPopover::Audio) {
                                    None
                                } else {
                                    Some(WorkbenchHudPopover::Audio)
                                };
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-files-btn",
                        format!("{} ({})", tr_str(lang, "传输", "Files"), self.file_transfers.len()),
                        self.file_drawer_open,
                        0x38BDF8,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.file_drawer_open = !this.file_drawer_open;
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-power-btn",
                        tr_str(lang, "电源", "Power"),
                        self.hud_active_popover == Some(WorkbenchHudPopover::Power),
                        pal.danger,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.hud_active_popover =
                                if this.hud_active_popover == Some(WorkbenchHudPopover::Power) {
                                    None
                                } else {
                                    Some(WorkbenchHudPopover::Power)
                                };
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-screenshot-btn",
                        tr_str(lang, "截屏", "Snap"),
                        false,
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.trigger_screenshot();
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-hud-collapse-btn",
                        tr_str(lang, "收起", "Hide"),
                        false,
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_hud_expanded();
                            cx.notify();
                        }),
                    )),
            );

        if let Some(popover) = self.hud_active_popover {
            col = col.child(self.render_hud_popover(popover, pal, lang, cx));
        }
        col.into_any_element()
    }

    fn render_hud_popover(
        &self,
        popover: WorkbenchHudPopover,
        pal: &TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut box_el = div()
            .px(px(12.0))
            .py(px(10.0))
            .rounded(px(10.0))
            .bg(rgba(0x0F172AF2))
            .border_1()
            .border_color(rgba(0xFFFFFF2A))
            .shadow_xl()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        match popover {
            WorkbenchHudPopover::Quality => {
                box_el = box_el
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94A3B8))
                                    .child(tr_str(lang, "帧率:", "FPS:")),
                            )
                            .children([30u32, 60, 120, 144].into_iter().map(|fps| {
                                let id: &'static str = match fps {
                                    30 => "wb-fps-30",
                                    60 => "wb-fps-60",
                                    120 => "wb-fps-120",
                                    _ => "wb-fps-144",
                                };
                                Self::hud_chip(
                                    id,
                                    format!("{fps}fps"),
                                    self.stream_settings.target_fps == fps,
                                    pal.accent,
                                    pal,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_stream_fps(fps);
                                        cx.notify();
                                    }),
                                )
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94A3B8))
                                    .child(tr_str(lang, "画质:", "Quality:")),
                            )
                            .child(Self::hud_chip(
                                "wb-qual-auto",
                                "Auto",
                                self.stream_settings.quality == StreamQuality::Auto,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.set_stream_quality(StreamQuality::Auto);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-qual-hd",
                                "HD 1080P",
                                self.stream_settings.quality == StreamQuality::Hd,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.set_stream_quality(StreamQuality::Hd);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-qual-4k",
                                "4K Ultra",
                                self.stream_settings.quality == StreamQuality::Ultra4K,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.set_stream_quality(StreamQuality::Ultra4K);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-qual-444",
                                "TrueColor 4:4:4",
                                self.stream_settings.color_mode == ColorSubsampling::TrueColor444,
                                0x8B5CF6,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_truecolor_444();
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-qual-hdr",
                                "HDR",
                                self.stream_settings.hdr,
                                0xF59E0B,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_hdr();
                                    cx.notify();
                                }),
                            )),
                    );
            }
            WorkbenchHudPopover::Displays => {
                box_el = box_el.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xE2E8F0))
                        .child(tr_str(lang, "切换远程显示器", "Switch Remote Display")),
                );
                for (i, d) in self.displays.iter().enumerate() {
                    let did = d.display_id;
                    let label = format!(
                        "#{} {} ({}x{} @ {}Hz{})",
                        did,
                        d.name,
                        d.width,
                        d.height,
                        d.refresh_hz,
                        if d.is_virtual { " · Virtual" } else { "" }
                    );
                    let id: &'static str = match i {
                        0 => "wb-disp-0",
                        1 => "wb-disp-1",
                        2 => "wb-disp-2",
                        _ => "wb-disp-n",
                    };
                    box_el = box_el.child(Self::hud_chip(
                        id,
                        label,
                        self.stream_settings.display_index == did,
                        pal.accent,
                        pal,
                        cx.listener(move |this, _, _, cx| {
                            this.switch_display(did);
                            cx.notify();
                        }),
                    ));
                }
            }
            WorkbenchHudPopover::Hotkeys => {
                box_el = box_el
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xE2E8F0))
                                    .child(tr_str(lang, "组合键与按键映射", "Hotkeys & Key Remap")),
                            )
                            .child(Self::hud_chip(
                                "wb-hotkey-remap-toggle",
                                tr_str(lang, "macOS ⇄ Win 映射", "macOS ⇄ Win Remap"),
                                self.stream_settings.macos_hotkey_mapping,
                                0x14B8A6,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_macos_key_remap();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(5.0))
                            .child(Self::hud_chip(
                                "wb-hk-cad",
                                "Ctrl+Alt+Del",
                                false,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_hotkey(WorkbenchHotkeyPreset::CtrlAltDel);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-hk-switch",
                                "Cmd/Alt+Tab",
                                false,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_hotkey(WorkbenchHotkeyPreset::AppSwitcher);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-hk-spot",
                                "Cmd+Space / Win",
                                false,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_hotkey(WorkbenchHotkeyPreset::SpotlightOrStart);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-hk-copy",
                                tr_str(lang, "复制", "Copy"),
                                false,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_hotkey(WorkbenchHotkeyPreset::Copy);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-hk-paste",
                                tr_str(lang, "粘贴", "Paste"),
                                false,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_hotkey(WorkbenchHotkeyPreset::Paste);
                                    cx.notify();
                                }),
                            )),
                    );
            }
            WorkbenchHudPopover::Audio => {
                box_el = box_el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(Self::hud_chip(
                            "wb-audio-stream-toggle",
                            tr_str(lang, "同步远程音频", "Remote Audio"),
                            self.stream_settings.audio_enabled,
                            0x22C55E,
                            pal,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_audio_enabled();
                                cx.notify();
                            }),
                        ))
                        .child(Self::hud_chip(
                            "wb-audio-mute-host-toggle",
                            tr_str(lang, "静音被控端扬声器", "Mute Host Speaker"),
                            self.stream_settings.mute_remote_speaker,
                            0xF59E0B,
                            pal,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_mute_remote_speaker();
                                cx.notify();
                            }),
                        ))
                        .child(Self::hud_chip(
                            "wb-audio-mic-toggle",
                            tr_str(lang, "麦克风透传", "Mic Passthrough"),
                            self.stream_settings.mic_passthrough,
                            0x38BDF8,
                            pal,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_mic_passthrough();
                                cx.notify();
                            }),
                        )),
                );
            }
            WorkbenchHudPopover::Power => {
                box_el = box_el
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xE2E8F0))
                            .child(tr_str(lang, "远程电源与唤醒控制", "Remote Power & WOL")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(6.0))
                            .child(Self::hud_chip(
                                "wb-pwr-lock",
                                tr_str(lang, "锁屏", "Lock"),
                                false,
                                pal.accent,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_power_action(PowerActionKind::Lock);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-pwr-sleep",
                                tr_str(lang, "休眠", "Sleep"),
                                false,
                                0xF59E0B,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_power_action(PowerActionKind::Sleep);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-pwr-reboot",
                                tr_str(lang, "重启", "Reboot"),
                                false,
                                pal.danger,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_power_action(PowerActionKind::Reboot);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-pwr-shutdown",
                                tr_str(lang, "关机", "Shutdown"),
                                false,
                                pal.danger,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_power_action(PowerActionKind::Shutdown);
                                    cx.notify();
                                }),
                            ))
                            .child(Self::hud_chip(
                                "wb-pwr-wol",
                                tr_str(lang, "WOL 网络唤醒", "Wake-on-LAN"),
                                false,
                                0x22C55E,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_wake_on_lan();
                                    cx.notify();
                                }),
                            )),
                    );
            }
        }
        box_el.into_any_element()
    }

    fn render_file_transfer_drawer(
        &self,
        pal: &TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col().gap(px(6.0));
        if self.file_transfers.is_empty() {
            list = list.child(
                div()
                    .py(px(16.0))
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "暂无文件传输任务，可从「远程终端与文件」页上传或下载文件（支持断点续传）",
                        "No active transfers. Supports chunked resumable upload/download.",
                    )),
            );
        } else {
            for (idx, t) in self.file_transfers.iter().enumerate() {
                let pct = if t.total_bytes == 0 {
                    100u64
                } else {
                    (t.transferred_bytes.saturating_mul(100) / t.total_bytes).min(100)
                };
                let tid_resume = t.transfer_id.clone();
                let tid_pause = t.transfer_id.clone();
                let dir_tag = match t.direction {
                    FileTransferDirection::Upload => "↑ UP",
                    FileTransferDirection::Download => "↓ DOWN",
                };
                let is_paused = matches!(
                    t.status,
                    FileTransferStatus::Paused | FileTransferStatus::Failed
                );
                let toggle_id = SharedString::from(format!("wb-xfer-toggle-{idx}"));
                list = list.child(
                    div()
                        .p(px(8.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.window_bg))
                        .border_1()
                        .border_color(rgb(pal.border_fg))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .truncate()
                                        .child(format!("{dir_tag} {}", t.remote_path)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.accent))
                                        .child(format!("{pct}%")),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .h(px(4.0))
                                .rounded(px(2.0))
                                .bg(rgb(pal.input_bg))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(pct as f32 / 100.0))
                                        .rounded(px(2.0))
                                        .bg(rgb(pal.accent)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "{}/{} bytes · {:?}",
                                            t.transferred_bytes, t.total_bytes, t.status
                                        )),
                                )
                                .child(
                                    div()
                                        .id(toggle_id)
                                        .cursor_pointer()
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(pal.hover_bg))
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if is_paused {
                                                    this.resume_file_transfer(&tid_resume);
                                                } else {
                                                    this.pause_file_transfer(&tid_pause);
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_paused {
                                            tr_str(lang, "续传", "Resume")
                                        } else {
                                            tr_str(lang, "暂停", "Pause")
                                        }),
                                ),
                        ),
                );
            }
        }

        div()
            .id("wb-file-transfer-drawer")
            .w(px(310.0))
            .max_h(px(360.0))
            .rounded(px(12.0))
            .bg(rgb(pal.card_bg))
            .border_1()
            .border_color(rgb(pal.border_fg))
            .shadow_xl()
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "文件传输队列", "File Transfers")),
                    )
                    .child(
                        div()
                            .id("wb-file-drawer-close")
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.file_drawer_open = false;
                                    cx.notify();
                                }),
                            )
                            .child("×"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "冲突策略:", "Conflict:")),
                    )
                    .child(Self::hud_chip(
                        "wb-conflict-overwrite",
                        tr_str(lang, "覆盖", "Overwrite"),
                        self.file_conflict_policy == FileConflictPolicy::Overwrite,
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.set_file_conflict_policy(FileConflictPolicy::Overwrite);
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-conflict-rename",
                        tr_str(lang, "重命名", "Rename"),
                        self.file_conflict_policy == FileConflictPolicy::Rename,
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.set_file_conflict_policy(FileConflictPolicy::Rename);
                            cx.notify();
                        }),
                    ))
                    .child(Self::hud_chip(
                        "wb-conflict-skip",
                        tr_str(lang, "跳过", "Skip"),
                        self.file_conflict_policy == FileConflictPolicy::Skip,
                        pal.accent,
                        pal,
                        cx.listener(|this, _, _, cx| {
                            this.set_file_conflict_policy(FileConflictPolicy::Skip);
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .id("wb-file-drawer-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element()
    }

    fn toggle_chip(label: &str, active: bool, pal: &TdesktopThemePalette) -> Div {
        div()
            .cursor_pointer()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(if active {
                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
            } else {
                rgb(pal.input_bg).into()
            })
            .border_1()
            .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if active {
                pal.accent
            } else {
                pal.text_secondary
            }))
            .child(label.to_string())
    }

    fn render_screen_pane(
        &self,
        pal: &TdesktopThemePalette,
        lang: &str,
        dev: &HostDeviceSpec,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview = if let Some(ref arc_img) = self.cached_frame_image {
            img(arc_img.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else if let Some(src) = self.frame_source.as_ref().filter(|s| !s.starts_with("data:")) {
            img(src.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            div()
                .size_full()
                .rounded(px(10.0))
                .bg(rgb(pal.history_bg))
                .border_1()
                .border_color(rgb(pal.border_fg))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .child(tg_icon(IconName::ComputerDesktop, 44.0, pal.icon_fg))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(if self.is_connected {
                            tr_str(
                                lang,
                                "点击画布即可控制鼠标，或点击下方捕获最新桌面画面",
                                "Click the viewport to send mouse events or refresh live frame",
                            )
                        } else {
                            tr_str(
                                lang,
                                "远程设备当前离线或待连接",
                                "Remote computer is offline or awaiting connection",
                            )
                        }),
                )
                .when(dev.is_tunnel_encrypted, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::Lock, 12.0, pal.success))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(if self.tailnet_peer.is_some() {
                                        tr_str(
                                            lang,
                                            "WireGuard 端到端加密通道已就绪 (X25519 + ChaCha20-Poly1305)",
                                            "WireGuard E2E Channel Ready (X25519 + ChaCha20-Poly1305)",
                                        )
                                    } else {
                                        tr_str(
                                            lang,
                                            "安全控制通道已就绪",
                                            "Encrypted Control Channel Ready",
                                        )
                                    }),
                            ),
                    )
                })
                .into_any_element()
        };

        let vp_bounds_for_canvas = self.viewport_bounds.clone();
        let bounds_tracker = canvas(
            |bounds, _, _| bounds,
            move |bounds, _, _, _| {
                if let Ok(mut guard) = vp_bounds_for_canvas.lock() {
                    *guard = Some(bounds);
                }
            },
        )
        .absolute()
        .inset_0();

        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .id("workbench-desktop-viewport")
                    .relative()
                    .cursor_pointer()
                    .flex_1()
                    .min_h(px(0.0))
                    .rounded(px(12.0))
                    .overflow_hidden()
                    .bg(rgb(pal.history_bg))
                    .border_1()
                    .border_color(rgb(pal.border_fg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            if !this.is_mouse_captured {
                                this.status_banner = Some(
                                    tr_str(
                                        &this.language_id,
                                        "鼠标控制已暂停，请先点击顶部「鼠标」按钮启用",
                                        "Mouse control is paused; enable Mouse in the toolbar first",
                                    )
                                    .to_string(),
                                );
                                cx.notify();
                                return;
                            }
                            let (remote_w, remote_h) = parse_resolution_dims(&this.device.resolution)
                                .unwrap_or((1440.0, 900.0));
                            let (x, y) = if let Ok(guard) = this.viewport_bounds.lock() {
                                if let Some(b) = *guard {
                                    let bw = f32::from(b.size.width).max(1.0);
                                    let bh = f32::from(b.size.height).max(1.0);
                                    // Account for ObjectFit::Contain letterboxing inside viewport.
                                    let remote_aspect = remote_w / remote_h.max(1.0);
                                    let vp_aspect = bw / bh;
                                    let (img_x, img_y, img_w, img_h) = if vp_aspect > remote_aspect {
                                        let w = bh * remote_aspect;
                                        ((bw - w) * 0.5, 0.0, w, bh)
                                    } else {
                                        let h = bw / remote_aspect;
                                        (0.0, (bh - h) * 0.5, bw, h)
                                    };
                                    let rel_x = f32::from(ev.position.x) - f32::from(b.origin.x) - img_x;
                                    let rel_y = f32::from(ev.position.y) - f32::from(b.origin.y) - img_y;
                                    let nx = (rel_x / img_w.max(1.0)).clamp(0.0, 1.0);
                                    let ny = (rel_y / img_h.max(1.0)).clamp(0.0, 1.0);
                                    ((nx * remote_w).round() as i32, (ny * remote_h).round() as i32)
                                } else {
                                    (
                                        f32::from(ev.position.x).max(0.0) as i32,
                                        f32::from(ev.position.y).max(0.0) as i32,
                                    )
                                }
                            } else {
                                (
                                    f32::from(ev.position.x).max(0.0) as i32,
                                    f32::from(ev.position.y).max(0.0) as i32,
                                )
                            };
                            this.terminal_focused = false;
                            this.status_banner = Some(format!(
                                "{} ({x}, {y})",
                                tr_str(&this.language_id, "已发送远程点击", "Sent GUI click")
                            ));
                            this.pending_command = Some(WorkbenchCommand::SendGuiClick { x, y });
                            this.pending_action = Some(format!("gui_click:{x}:{y}"));
                            cx.notify();
                        }),
                    )
                    .child(preview)
                    .child(bounds_tracker)
                    .child(
                        div()
                            .id("wb-hud-top-overlay")
                            .absolute()
                            .top(px(10.0))
                            .left(px(12.0))
                            .right(px(12.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(self.render_floating_hud(pal, lang, dev, cx)),
                    )
                    .when(self.file_drawer_open, |vp| {
                        vp.child(
                            div()
                                .absolute()
                                .top(px(56.0))
                                .right(px(12.0))
                                .child(self.render_file_transfer_drawer(pal, lang, cx)),
                        )
                    })
                    .when(self.stream_settings.privacy_screen, |vp| {
                        vp.child(
                            div()
                                .id("wb-privacy-badge")
                                .absolute()
                                .bottom(px(12.0))
                                .left(px(12.0))
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(999.0))
                                .bg(rgba(0x0F766EE0))
                                .border_1()
                                .border_color(rgb(0x14B8A6))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .child(tr_str(
                                    lang,
                                    "被控端隐私屏已启用（对方屏幕已隐藏）",
                                    "Host privacy screen active",
                                )),
                        )
                    })
                    .when(
                        (self.needs_screen_permission_guide
                            || self.needs_accessibility_permission_guide
                            || self.force_local_permission_modal)
                            && (cfg!(target_os = "macos") || !self.is_local_device),
                        |vp| {
                            if (self.is_local_device || self.force_local_permission_modal)
                                && cfg!(target_os = "macos")
                            {
                                let snap = cached_permissions_snapshot();
                                vp.child(render_gemini_permission_modal(
                                    &snap,
                                    *pal,
                                    lang,
                                    cx.listener(|this, _, _, cx| {
                                        this.needs_screen_permission_guide = false;
                                        this.needs_accessibility_permission_guide = false;
                                        this.force_local_permission_modal = false;
                                        this.remote_permission_error = None;
                                        cx.notify();
                                    }),
                                    cx.listener(|this, _, _, cx| {
                                        let snap = cached_permissions_snapshot();
                                        this.needs_screen_permission_guide = false;
                                        this.needs_accessibility_permission_guide = false;
                                        this.force_local_permission_modal = false;
                                        this.remote_permission_error = None;
                                        if !this.is_local_device
                                            || !cfg!(target_os = "macos")
                                            || snap.screen_state.is_active()
                                        {
                                            this.enqueue_command(WorkbenchCommand::RefreshScreen);
                                        }
                                        cx.notify();
                                    }),
                                ))
                            } else if !self.is_local_device {
                                vp.child(render_remote_host_permission_modal(
                                    &dev.name,
                                    self.needs_screen_permission_guide,
                                    self.needs_accessibility_permission_guide,
                                    self.remote_permission_error.as_deref(),
                                    *pal,
                                    lang,
                                    cx.listener(|this, _, _, cx| {
                                        this.needs_screen_permission_guide = false;
                                        this.needs_accessibility_permission_guide = false;
                                        this.remote_permission_error = None;
                                        cx.notify();
                                    }),
                                    cx.listener(|this, _, _, cx| {
                                        this.needs_screen_permission_guide = false;
                                        this.needs_accessibility_permission_guide = false;
                                        this.remote_permission_error = None;
                                        this.enqueue_command(WorkbenchCommand::RefreshScreen);
                                        cx.notify();
                                    }),
                                    cx.listener(|this, _, _, cx| {
                                        this.force_local_permission_modal = true;
                                        request_permissions_refresh(true);
                                        cx.notify();
                                    }),
                                ))
                            } else {
                                vp
                            }
                        },
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(rgb(pal.border_fg))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .when(cfg!(target_os = "macos") || !self.is_local_device, |row| {
                                row.child(Self::quick_cmd_btn(
                                    "wb-perm-guide-btn",
                                    tr_str(lang, "系统权限", "Permissions"),
                                    self.needs_screen_permission_guide
                                        || self.needs_accessibility_permission_guide
                                        || self.force_local_permission_modal,
                                    pal,
                                    cx.listener(|this, _, _, cx| {
                                        let is_open = this.needs_screen_permission_guide
                                            || this.needs_accessibility_permission_guide
                                            || this.force_local_permission_modal;
                                        if is_open {
                                            this.needs_screen_permission_guide = false;
                                            this.needs_accessibility_permission_guide = false;
                                            this.force_local_permission_modal = false;
                                            this.remote_permission_error = None;
                                            hide_drag_hud();
                                        } else {
                                            this.force_local_permission_modal = true;
                                            request_permissions_refresh(true);
                                        }
                                        cx.notify();
                                    }),
                                ))
                            })
                            .child(Self::quick_cmd_btn(
                                "wb-refresh-btn",
                                tr_str(lang, "刷新画面", "Refresh Screen"),
                                true,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_command = Some(WorkbenchCommand::RefreshScreen);
                                    this.status_banner = Some(
                                        tr_str(
                                            &this.language_id,
                                            "正在抓取远程桌面画面...",
                                            "Capturing remote screen...",
                                        )
                                        .to_string(),
                                    );
                                    cx.notify();
                                }),
                            ))
                            .child(Self::quick_cmd_btn(
                                "wb-open-term-btn",
                                tr_str(lang, "打开终端", "Terminal"),
                                false,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.active_tab = WorkbenchTab::RemoteTerminal;
                                    this.terminal_focused = true;
                                    this.ensure_terminal_view(cx);
                                    if let Some(term) = this.terminal_view.as_ref() {
                                        term.update(cx, |t, _| t.is_focused = true);
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(Self::quick_cmd_btn(
                                "wb-list-files-btn",
                                tr_str(lang, "浏览文件", "Files"),
                                false,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.active_tab = WorkbenchTab::RemoteTerminal;
                                    this.pending_command = Some(WorkbenchCommand::ListFiles {
                                        path: this.remote_cwd.clone(),
                                    });
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!(
                                "{} · {}",
                                tr_str(lang, "策略", "Policy"),
                                if lang.starts_with("zh") {
                                    self.run_policy.label_zh()
                                } else {
                                    self.run_policy.label_en()
                                }
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_terminal_pane(
        &self,
        pal: &TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (draft_text, draft_preedit) = self
            .terminal_input
            .as_ref()
            .map(|c| c.read(cx).text_and_preedit(self.terminal_draft.clone()))
            .unwrap_or_else(|| (self.terminal_draft.clone(), None));
        let is_zh = lang.starts_with("zh");
        let term_bg = match pal.mode {
            TdesktopThemeMode::Day => 0x111827,
            _ => pal.window_bg,
        };

        let mut logs_col = div().flex().flex_col().gap(px(8.0));
        if self.terminal_history.is_empty() {
            logs_col = logs_col.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x94A3B8))
                    .child(if is_zh {
                        "在下方输入 Shell 命令以在远程计算机执行（策略为“每次询问”时会弹出授权确认）。"
                    } else {
                        "Enter a shell command below to run on this remote computer."
                    }),
            );
        } else {
            for entry in &self.terminal_history {
                logs_col = logs_col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .pb(px(6.0))
                        .border_b_1()
                        .border_color(rgb(0x1E293B))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0x38BDF8))
                                        .child(format!("{} $ {}", entry.cwd, entry.command)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(if entry.exit_code == 0 {
                                            0x4ADE80
                                        } else {
                                            0xF87171
                                        }))
                                        .child(format!("exit {}", entry.exit_code)),
                                ),
                        )
                        .when(!entry.stdout.is_empty(), |col| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xE2E8F0))
                                    .child(entry.stdout.clone()),
                            )
                        })
                        .when(!entry.stderr.is_empty(), |col| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xFCA5A5))
                                    .child(entry.stderr.clone()),
                            )
                        }),
                );
            }
        }

        let mut files_col = div().flex().flex_col().gap(px(4.0));
        files_col = files_col.child(
            div()
                .id("wb-parent-dir")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                .text_xs()
                .text_color(rgb(pal.accent))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        let next = parent_path(&this.remote_cwd);
                        this.pending_command = Some(WorkbenchCommand::ListFiles { path: next });
                        cx.notify();
                    }),
                )
                .child(".. (上级目录)"),
        );
        if self.remote_files.is_empty() {
            files_col = files_col.child(
                div()
                    .px(px(8.0))
                    .py(px(10.0))
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(if is_zh {
                        "点击“刷新目录”加载远程文件列表"
                    } else {
                        "Click Refresh to load remote directory"
                    }),
            );
        } else {
            for (idx, item) in self.remote_files.iter().enumerate() {
                let item_name = item.clone();
                let is_dir = item_name.ends_with('/');
                files_col = files_col.child(
                    div()
                        .id(("wb-file-item", idx))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let clean = item_name.trim_end_matches('/');
                                let joined = join_remote_path(&this.remote_cwd, clean);
                                if is_dir {
                                    this.pending_command =
                                        Some(WorkbenchCommand::ListFiles { path: joined });
                                } else {
                                    this.pending_command =
                                        Some(WorkbenchCommand::ReadRemoteFile { path: joined });
                                }
                                cx.notify();
                            }),
                        )
                        .child(tg_icon(
                            if is_dir {
                                IconName::FolderAll
                            } else {
                                IconName::FileDoc
                            },
                            13.0,
                            if is_dir { pal.accent } else { pal.icon_fg },
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .truncate()
                                .child(item.clone()),
                        ),
                );
            }
        }

        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_row()
            .gap(px(10.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .rounded(px(12.0))
                    .bg(rgb(term_bg))
                    .border_1()
                    .border_color(rgb(pal.border_fg))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(rgb(0x1E293B))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(tg_icon(IconName::TerminalMcp, 14.0, 0x38BDF8))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xE2E8F0))
                                            .child(format!("cwd: {}", self.remote_cwd)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .child(Self::quick_cmd_btn(
                                        "wb-quick-uname",
                                        "uname -a",
                                        false,
                                        pal,
                                        cx.listener(|this, _, _, cx| {
                                            this.pending_command =
                                                Some(WorkbenchCommand::RunShell {
                                                    command: "uname -a".to_string(),
                                                    cwd: Some(this.remote_cwd.clone()),
                                                });
                                            cx.notify();
                                        }),
                                    ))
                                    .child(Self::quick_cmd_btn(
                                        "wb-quick-ls",
                                        "ls -la",
                                        false,
                                        pal,
                                        cx.listener(|this, _, _, cx| {
                                            this.pending_command =
                                                Some(WorkbenchCommand::RunShell {
                                                    command: "ls -la".to_string(),
                                                    cwd: Some(this.remote_cwd.clone()),
                                                });
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        if let Some(term) = self.terminal_view.as_ref() {
                            div()
                                .id("wb-pty-view-host")
                                .flex_1()
                                .min_h(px(0.0))
                                .flex()
                                .flex_col()
                                .child(term.clone())
                                .into_any_element()
                        } else {
                            div()
                                .id("wb-term-history")
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .p(px(12.0))
                                .child(logs_col)
                                .into_any_element()
                        },
                    )
                    .child(
                        div()
                            .p(px(10.0))
                            .border_t_1()
                            .border_color(rgb(0x1E293B))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("wb-term-input-box")
                                    .cursor_text()
                                    .flex_1()
                                    .min_h(px(34.0))
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(0x0F172A))
                                    .border_1()
                                    .border_color(rgb(if self.terminal_focused {
                                        pal.accent
                                    } else {
                                        0x334155
                                    }))
                                    .flex()
                                    .items_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.ensure_terminal_input(cx);
                                            this.terminal_focused = true;
                                            if let Some(term) = this.terminal_view.as_ref() {
                                                term.update(cx, |t, _| t.is_focused = false);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_ime_text_preview(
                                        &draft_text,
                                        draft_preedit.as_deref(),
                                        tr_str(
                                            lang,
                                            "输入远程 Shell 命令并回车...",
                                            "Enter remote shell command...",
                                        ),
                                        self.terminal_focused,
                                        false,
                                        0xF8FAFC,
                                        *pal,
                                    )),
                            )
                            .child(Self::quick_cmd_btn(
                                "wb-run-shell-btn",
                                tr_str(lang, "执行", "Run"),
                                true,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    if this.submit_terminal_draft() {
                                        cx.notify();
                                    }
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .w(px(270.0))
                    .flex_shrink_0()
                    .h_full()
                    .rounded(px(12.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(rgb(pal.border_fg))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(rgb(pal.border_fg))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(lang, "远程文件", "Remote Files")),
                            )
                            .child(Self::quick_cmd_btn(
                                "wb-refresh-files-btn",
                                tr_str(lang, "刷新目录", "Refresh"),
                                false,
                                pal,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_command = Some(WorkbenchCommand::ListFiles {
                                        path: this.remote_cwd.clone(),
                                    });
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .id("wb-files-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .p(px(8.0))
                            .child(files_col),
                    )
                    .when_some(self.file_preview.as_ref(), |panel, (fname, content)| {
                        panel.child(
                            div()
                                .max_h(px(160.0))
                                .border_t_1()
                                .border_color(rgb(pal.border_fg))
                                .p(px(8.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.accent))
                                        .truncate()
                                        .child(fname.clone()),
                                )
                                .child(
                                    div()
                                        .id("wb-file-preview-scroll")
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(content.clone()),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn quick_cmd_btn(
        id: &'static str,
        label: &str,
        primary: bool,
        pal: &TdesktopThemePalette,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(7.0))
            .bg(rgb(if primary { pal.accent } else { pal.input_bg }))
            .border_1()
            .border_color(rgb(if primary { pal.accent } else { pal.border_fg }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if primary {
                pal.active_button_fg
            } else {
                pal.text_primary
            }))
            .on_mouse_down(MouseButton::Left, on_click)
            .child(label.to_string())
    }
}

fn parent_path(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "~" {
        return "~".to_string();
    }
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => "~".to_string(),
    }
}

fn join_remote_path(base: &str, child: &str) -> String {
    if child.starts_with('/') {
        return child.to_string();
    }
    let clean_base = base.trim_end_matches('/');
    if clean_base.is_empty() {
        child.to_string()
    } else {
        format!("{clean_base}/{child}")
    }
}

impl Default for ComputerWorkbench {
    fn default() -> Self {
        Self::new("Dev-Workstation-01".to_string())
    }
}

impl Render for ComputerWorkbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_terminal_input(cx);
        self.ensure_terminal_view(cx);
        self.ensure_permission_poll_loop(cx);
        let pal = self.palette;
        let lang = self.language_id.clone();
        let dev = self.device.clone();
        let metrics = if dev.ip_address.is_empty() && dev.latency_ms == 0 {
            if self.is_connected {
                tr_str(&lang, "已连接", "Connected").to_string()
            } else {
                tr_str(&lang, "离线", "Offline").to_string()
            }
        } else {
            format!(
                "{} • {}ms • {} FPS",
                dev.ip_address, dev.latency_ms, dev.fps
            )
        };
        let is_desktop = self.active_tab == WorkbenchTab::DesktopScreen;

        div()
            .size_full()
            .min_w(px(640.0))
            .min_h(px(440.0))
            .max_w_full()
            .max_h_full()
            .bg(rgb(pal.modal_bg))
            .border_1()
            .border_color(rgb(pal.border_fg))
            .rounded(px(16.0))
            .shadow_xl()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(11.0))
                    .bg(rgb(pal.top_bar_bg))
                    .border_b_1()
                    .border_color(rgb(pal.border_fg))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(tg_icon(IconName::ComputerDesktop, 18.0, pal.accent))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(dev.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .px(px(6.0))
                                                    .py(px(1.0))
                                                    .rounded(px(999.0))
                                                    .bg(rgb(pal.input_bg))
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(if self.is_connected {
                                                        pal.success
                                                    } else {
                                                        pal.text_secondary
                                                    }))
                                                    .child(if self.node_kind == ComputerNodeKind::CloudServer {
                                                        tr_str(&lang, "云电脑", "Cloud")
                                                    } else {
                                                        tr_str(&lang, "本地", "Local")
                                                    }),
                                            )
                                            .when_some(self.tailnet_peer.as_ref(), |row, peer| {
                                                let is_zh = lang.starts_with("zh");
                                                let mode_label = if is_zh {
                                                    peer.relay_mode.label_zh()
                                                } else {
                                                    peer.relay_mode.label_en()
                                                };
                                                let badge_color =
                                                    if peer.relay_mode == TailnetRelayMode::DirectUdp {
                                                        pal.success
                                                    } else {
                                                        pal.accent
                                                    };
                                                row.child(
                                                    div()
                                                        .px(px(7.0))
                                                        .py(px(1.5))
                                                        .rounded(px(999.0))
                                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                                            badge_color,
                                                            0.14,
                                                        ))
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(4.0))
                                                        .child(tg_icon(IconName::Lock, 10.0, badge_color))
                                                        .child(
                                                            div()
                                                                .text_size(px(10.5))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(badge_color))
                                                                .child(format!("{mode_label} · E2E")),
                                                        ),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(pal.text_secondary))
                                            .child(metrics),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(Self::quick_cmd_btn(
                                "wb-tab-desktop",
                                tr_str(&lang, "远程桌面", "Desktop"),
                                is_desktop,
                                &pal,
                                cx.listener(|this, _, _, cx| {
                                    this.active_tab = WorkbenchTab::DesktopScreen;
                                    this.terminal_focused = false;
                                    if let Some(term) = this.terminal_view.as_ref() {
                                        term.update(cx, |t, _| t.is_focused = false);
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(Self::quick_cmd_btn(
                                "wb-tab-terminal",
                                tr_str(&lang, "远程终端与文件", "Terminal & Files"),
                                !is_desktop,
                                &pal,
                                cx.listener(|this, _, _, cx| {
                                    this.active_tab = WorkbenchTab::RemoteTerminal;
                                    this.terminal_focused = true;
                                    this.ensure_terminal_view(cx);
                                    if let Some(term) = this.terminal_view.as_ref() {
                                        term.update(cx, |t, _| t.is_focused = true);
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(
                                Self::toggle_chip(
                                    tr_str(&lang, "鼠标", "Mouse"),
                                    self.is_mouse_captured,
                                    &pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_mouse_capture();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                Self::toggle_chip(
                                    tr_str(&lang, "键盘", "Keys"),
                                    self.is_keyboard_captured,
                                    &pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_keyboard_capture();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                Self::toggle_chip(
                                    tr_str(&lang, "剪贴板", "Clip"),
                                    self.is_clipboard_synced,
                                    &pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_clipboard_sync();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                Self::toggle_chip("HD", self.is_quality_hd, &pal).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_quality();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .id("wb-disconnect-btn")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.danger))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            hide_drag_hud();
                                            this.should_close = true;
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(&lang, "断开", "Disconnect")),
                            ),
                    ),
            )
            .when_some(self.status_banner.as_ref(), |col, msg| {
                col.child(
                    div()
                        .px(px(16.0))
                        .py(px(6.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .border_b_1()
                        .border_color(rgb(pal.border_fg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(msg.clone()),
                )
            })
            .child(div().flex_1().min_h(px(0.0)).p(px(14.0)).flex().child(
                if is_desktop {
                    self.render_screen_pane(&pal, &lang, &dev, cx)
                } else {
                    self.render_terminal_pane(&pal, &lang, cx)
                },
            ))
    }
}
