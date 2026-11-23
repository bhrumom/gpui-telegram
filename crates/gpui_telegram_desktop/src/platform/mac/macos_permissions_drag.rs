//! Native macOS privacy permission engine, Gemini-style floating drag-to-enable HUD,
//! fresh out-of-process TCC verification, stale ad-hoc CDHash mismatch recovery, and
//! clean app relaunch support.

use fabushi_remote_engine::macos_permissions::MacSigningIdentity;

/// Which macOS privacy permission is being configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionKind {
    ScreenRecording,
    Accessibility,
}

/// Authoritative Screen Recording state distinguishing live capture readiness from
/// newly-granted permissions that require restarting the macOS app process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenPermissionState {
    /// Screen Recording is granted AND active right now in this running process.
    GrantedActive,
    /// Screen Recording is granted in System Settings for THIS running binary,
    /// but macOS requires restarting Fabushi for capture to take effect.
    GrantedNeedsRestart,
    /// Screen Recording is not granted (or an older ad-hoc signature is stale in TCC).
    NotGranted,
}

impl ScreenPermissionState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::GrantedActive)
    }

    pub fn needs_restart(self) -> bool {
        matches!(self, Self::GrantedNeedsRestart)
    }
}

/// Snapshot of macOS Remote Control privacy permissions safe to read during UI `render()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacPermissionsSnapshot {
    pub screen_state: ScreenPermissionState,
    pub accessibility_granted: bool,
    pub preflight_screen: bool,
    pub fresh_probe_screen: Option<bool>,
    pub signing: MacSigningIdentity,
    pub stale_screen_suspected: bool,
    pub stale_ax_suspected: bool,
    pub screen_settings_opened: bool,
    pub ax_settings_opened: bool,
    pub hud_visible: bool,
    pub hud_target: Option<PermissionKind>,
    pub last_drag_seq: u64,
    pub last_probe_detail: Option<String>,
}

impl MacPermissionsSnapshot {
    pub fn all_ready(&self) -> bool {
        self.screen_state == ScreenPermissionState::GrantedActive && self.accessibility_granted
    }

    pub fn any_missing(&self) -> bool {
        !self.all_ready()
    }
}

/// Pure Screen Recording state machine shared by the macOS runtime and cross-platform tests.
#[allow(clippy::too_many_arguments)]
pub fn compute_screen_state_from_probe(
    screen_granted_at_launch: bool,
    fresh_probe_screen: Option<bool>,
    sck_granted_active: bool,
    sck_permission_denied: bool,
    sck_unavailable: bool,
    sck_probed: bool,
    can_read_window_names: bool,
    preflight_now: bool,
) -> ScreenPermissionState {
    if sck_granted_active {
        return ScreenPermissionState::GrantedActive;
    }
    let tcc_says_granted = match fresh_probe_screen {
        Some(false) => false,
        Some(true) => true,
        None => preflight_now,
    };
    if tcc_says_granted {
        if sck_permission_denied {
            return ScreenPermissionState::GrantedNeedsRestart;
        }
        if !screen_granted_at_launch {
            return ScreenPermissionState::GrantedNeedsRestart;
        }
        if sck_unavailable || !sck_probed {
            return ScreenPermissionState::GrantedActive;
        }
        if can_read_window_names {
            return ScreenPermissionState::GrantedActive;
        }
        return ScreenPermissionState::GrantedNeedsRestart;
    }
    ScreenPermissionState::NotGranted
}

/// Pure decision helper determining whether `open_permission_with_drag_hud` should run
/// a one-time `tccutil reset` for our own bundle ID before opening System Settings.
///
/// Crucially does NOT gate on `is_adhoc`: upgrading between ad-hoc and Developer ID builds
/// leaves a stale `csreq` row in macOS `TCC.db` that System Settings drag/toggle does NOT
/// overwrite, causing `-67050` (`errSecCSReqFailed`) even after restart. Resetting once
/// when TCC currently denies our running bundle cannot revoke a working grant and guarantees
/// System Settings writes a fresh entry bound to the current code signature.
pub fn should_auto_reset_tcc_service(
    bundle_id: &str,
    granted_at_launch: bool,
    already_reset: bool,
    preflight_or_live_now: bool,
    fresh_probe_now: Option<bool>,
) -> bool {
    let trimmed = bundle_id.trim();
    !trimmed.is_empty()
        && trimmed.contains('.')
        && !granted_at_launch
        && !already_reset
        && !preflight_or_live_now
        && fresh_probe_now != Some(true)
}

/// Pure decision helper determining whether a stale/poisoned TCC `csreq` entry is suspected
/// so the UI can surface the amber "重置并重新拖入" recovery action.
///
/// Applies to BOTH Developer ID and ad-hoc builds (never gated on `is_adhoc`).
#[allow(clippy::too_many_arguments)]
pub fn is_stale_tcc_entry_suspected(
    is_currently_denied: bool,
    current_cdhash: &str,
    current_team_id: &str,
    current_is_adhoc: bool,
    last_verified_cdhash: Option<&str>,
    last_verified_team_id: Option<&str>,
    last_was_adhoc: Option<bool>,
    drag_probe_failed: bool,
    settings_opened: bool,
    fresh_probe: Option<bool>,
) -> bool {
    if !is_currently_denied {
        return false;
    }
    let cdhash_changed = !current_cdhash.is_empty()
        && last_verified_cdhash.is_some_and(|prev| !prev.is_empty() && prev != current_cdhash);
    let team_changed = !current_team_id.is_empty()
        && last_verified_team_id.is_some_and(|prev| !prev.is_empty() && prev != current_team_id);
    let signing_mode_changed = last_was_adhoc.is_some_and(|prev| prev != current_is_adhoc);

    cdhash_changed
        || team_changed
        || signing_mode_changed
        || drag_probe_failed
        || (settings_opened && fresh_probe == Some(false))
}

/// Pure decision helper determining whether the background probe may safely query
/// `SCShareableContent` without triggering an intrusive macOS TCC consent dialog.
pub fn should_probe_sck_live(
    screen_granted_at_launch: bool,
    preflight_now: bool,
    fresh_probe_screen: Option<bool>,
    sck_live_verified: bool,
) -> bool {
    if fresh_probe_screen == Some(false) {
        return false;
    }
    if !preflight_now {
        return false;
    }
    screen_granted_at_launch || sck_live_verified
}

/// Returns `true` when opening a local macOS remote workbench should immediately display
/// our Gemini-style permission modal instead of dispatching an initial `CaptureScreen` op.
pub fn should_defer_local_capture_for_permissions(
    is_local_device: bool,
    is_macos: bool,
    screen_state: ScreenPermissionState,
) -> bool {
    is_local_device && is_macos && !screen_state.is_active()
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use fabushi_remote_engine::macos_permissions::{
        SckProbeOutcome, can_read_other_window_names, clear_sck_live_verified,
        current_bundle_identity, current_signing_identity, is_accessibility_trusted,
        is_sck_live_verified, preflight_screen_capture_access, probe_sck_screen_access,
    };
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use std::ffi::{CString, c_char};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn fabushi_permission_hud_show(
            kind: i32,
            app_name: *const c_char,
            app_path: *const c_char,
            instruction_text: *const c_char,
            drag_hint_text: *const c_char,
        );
        fn fabushi_permission_hud_hide();
        fn fabushi_permission_hud_is_visible() -> bool;
        fn fabushi_permission_hud_active_kind() -> i32;
        fn fabushi_permission_hud_drag_event_seq() -> u64;
        fn fabushi_permission_hud_last_dragged_kind() -> i32;
    }

    #[derive(Debug, Default, Clone, Serialize, Deserialize)]
    struct PersistedTccSignatureState {
        #[serde(default)]
        last_verified_screen_cdhash: Option<String>,
        #[serde(default)]
        last_verified_ax_cdhash: Option<String>,
        #[serde(default)]
        last_bundle_path: Option<String>,
        #[serde(default)]
        last_verified_team_id: Option<String>,
        #[serde(default)]
        last_was_adhoc: Option<bool>,
    }

    #[derive(Debug)]
    struct RuntimePermState {
        signing: MacSigningIdentity,
        signing_loaded: bool,
        persisted: PersistedTccSignatureState,
        screen_granted_at_launch: bool,
        ax_granted_at_launch: bool,
        cached_preflight_screen: bool,
        cached_ax_granted: bool,
        cached_can_read_window_names: bool,
        auto_reset_screen_done: bool,
        auto_reset_ax_done: bool,
        screen_settings_opened: bool,
        ax_settings_opened: bool,
        screen_drag_completed: bool,
        ax_drag_completed: bool,
        screen_drag_at: Option<Instant>,
        ax_drag_at: Option<Instant>,
        screen_drag_probe_failed: bool,
        ax_drag_probe_failed: bool,
        last_seen_drag_seq: u64,
        last_probe_at: Option<Instant>,
        sck_outcome: Option<SckProbeOutcome>,
        fresh_probe_screen: Option<bool>,
        fresh_probe_ax: Option<bool>,
        last_probe_detail: Option<String>,
    }

    static STATE: OnceLock<Mutex<RuntimePermState>> = OnceLock::new();
    static PROBE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
    static PROBE_PENDING_FORCE: AtomicBool = AtomicBool::new(false);
    static SNAPSHOT_GENERATION: AtomicU64 = AtomicU64::new(1);

    fn is_test_or_offline() -> bool {
        cfg!(test)
            || std::env::var_os("GPUI_OFFLINE_TEST").is_some()
            || fabushi_remote_engine::is_headless_test_env()
    }

    fn tcc_state_file_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Fabushi")
                .join("tcc_signature_state.json"),
        )
    }

    fn load_persisted_state() -> PersistedTccSignatureState {
        let Some(path) = tcc_state_file_path() else {
            return PersistedTccSignatureState::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_persisted_state(st: &PersistedTccSignatureState) {
        if is_test_or_offline() {
            return;
        }
        let Some(path) = tcc_state_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(st) {
            let _ = std::fs::write(path, json);
        }
    }

    fn state_lock() -> &'static Mutex<RuntimePermState> {
        STATE.get_or_init(|| {
            // Use fast NSBundle/dyld lookup on initialization so we NEVER call
            // SecCodeCopySigningInformation / SecTrustEvaluateIfNecessary on the main thread.
            let signing = if is_test_or_offline() {
                MacSigningIdentity::default()
            } else {
                current_bundle_identity()
            };
            let persisted = if is_test_or_offline() {
                PersistedTccSignatureState::default()
            } else {
                load_persisted_state()
            };
            let screen_at_launch = preflight_screen_capture_access();
            let ax_at_launch = is_accessibility_trusted();

            let mut persisted_mut = persisted;
            let mut dirty = false;
            if !signing.bundle_path.is_empty()
                && persisted_mut.last_bundle_path.as_deref() != Some(signing.bundle_path.as_str())
            {
                persisted_mut.last_bundle_path = Some(signing.bundle_path.clone());
                dirty = true;
            }
            if dirty {
                save_persisted_state(&persisted_mut);
            }

            Mutex::new(RuntimePermState {
                signing,
                signing_loaded: false,
                persisted: persisted_mut,
                screen_granted_at_launch: screen_at_launch,
                ax_granted_at_launch: ax_at_launch,
                cached_preflight_screen: screen_at_launch,
                cached_ax_granted: ax_at_launch,
                cached_can_read_window_names: false,
                auto_reset_screen_done: false,
                auto_reset_ax_done: false,
                screen_settings_opened: false,
                ax_settings_opened: false,
                screen_drag_completed: false,
                ax_drag_completed: false,
                screen_drag_at: None,
                ax_drag_at: None,
                screen_drag_probe_failed: false,
                ax_drag_probe_failed: false,
                last_seen_drag_seq: 0,
                last_probe_at: None,
                sck_outcome: None,
                fresh_probe_screen: None,
                fresh_probe_ax: None,
                last_probe_detail: None,
            })
        })
    }

    fn hud_target_from_raw(raw: i32) -> Option<PermissionKind> {
        match raw {
            0 => Some(PermissionKind::ScreenRecording),
            1 => Some(PermissionKind::Accessibility),
            _ => None,
        }
    }

    /// Runs a lightweight ~15ms fresh out-of-process check of the exact same running
    /// executable (`--probe-macos-permissions`) so macOS evaluates TCC with an uncached
    /// WindowServer/TCC token — reliably detecting when Screen Recording was just enabled
    /// in System Settings and requires restarting Fabushi.
    fn run_fresh_child_probe() -> Option<(bool, bool)> {
        if is_test_or_offline() {
            return Some((true, true));
        }
        let exe = std::env::current_exe().ok()?;
        let output = Command::new(exe)
            .arg("--probe-macos-permissions")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let val: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        let screen = val.get("screen_preflight")?.as_bool()?;
        let ax = val.get("accessibility")?.as_bool()?;
        Some((screen, ax))
    }

    fn compute_screen_state(st: &RuntimePermState) -> ScreenPermissionState {
        if is_test_or_offline() {
            return ScreenPermissionState::GrantedActive;
        }
        let sck_active = (is_sck_live_verified()
            || matches!(st.sck_outcome, Some(SckProbeOutcome::GrantedActive)))
            && st.fresh_probe_screen != Some(false);
        super::compute_screen_state_from_probe(
            st.screen_granted_at_launch,
            st.fresh_probe_screen,
            sck_active,
            matches!(st.sck_outcome, Some(SckProbeOutcome::PermissionDenied(_))),
            matches!(st.sck_outcome, Some(SckProbeOutcome::Unavailable)),
            st.sck_outcome.is_some(),
            st.cached_can_read_window_names,
            st.cached_preflight_screen,
        )
    }

    fn snapshot_from_state(st: &RuntimePermState) -> MacPermissionsSnapshot {
        // Strictly read cached values + lock-free HUD atomics — ZERO synchronous TCC IPCs
        // or CGWindowList calls on the GPUI render thread!
        let preflight_now = st.cached_preflight_screen;
        let ax_now = st.cached_ax_granted;
        let screen_state = compute_screen_state(st);
        let last_was_adhoc = if st.signing_loaded {
            st.persisted.last_was_adhoc
        } else {
            None
        };

        let stale_screen_suspected = super::is_stale_tcc_entry_suspected(
            screen_state == ScreenPermissionState::NotGranted,
            &st.signing.cdhash_hex,
            &st.signing.team_id,
            st.signing.is_adhoc,
            st.persisted.last_verified_screen_cdhash.as_deref(),
            st.persisted.last_verified_team_id.as_deref(),
            last_was_adhoc,
            st.screen_drag_probe_failed,
            st.screen_settings_opened,
            st.fresh_probe_screen,
        );

        let stale_ax_suspected = super::is_stale_tcc_entry_suspected(
            !ax_now,
            &st.signing.cdhash_hex,
            &st.signing.team_id,
            st.signing.is_adhoc,
            st.persisted.last_verified_ax_cdhash.as_deref(),
            st.persisted.last_verified_team_id.as_deref(),
            last_was_adhoc,
            st.ax_drag_probe_failed,
            st.ax_settings_opened,
            st.fresh_probe_ax,
        );

        let hud_visible = if is_test_or_offline() {
            false
        } else {
            unsafe { fabushi_permission_hud_is_visible() }
        };
        let hud_target = if hud_visible {
            hud_target_from_raw(unsafe { fabushi_permission_hud_active_kind() })
        } else {
            None
        };
        let drag_seq = if is_test_or_offline() {
            0
        } else {
            unsafe { fabushi_permission_hud_drag_event_seq() }
        };

        MacPermissionsSnapshot {
            screen_state,
            accessibility_granted: ax_now,
            preflight_screen: preflight_now,
            fresh_probe_screen: st.fresh_probe_screen,
            signing: st.signing.clone(),
            stale_screen_suspected,
            stale_ax_suspected,
            screen_settings_opened: st.screen_settings_opened,
            ax_settings_opened: st.ax_settings_opened,
            hud_visible,
            hud_target,
            last_drag_seq: drag_seq,
            last_probe_detail: st.last_probe_detail.clone(),
        }
    }

    fn schedule_post_drag_followup_probes() {
        std::thread::spawn(|| {
            for delay_ms in [250u64, 750u64, 1500u64] {
                std::thread::sleep(Duration::from_millis(delay_ms));
                let done = {
                    let st = state_lock().lock();
                    let screen_resolved = matches!(
                        compute_screen_state(&st),
                        ScreenPermissionState::GrantedActive
                            | ScreenPermissionState::GrantedNeedsRestart
                    );
                    (!st.screen_drag_completed || screen_resolved)
                        && (!st.ax_drag_completed || st.cached_ax_granted)
                };
                if done {
                    break;
                }
                request_permissions_refresh(true);
            }
        });
    }

    /// Fast, non-blocking permission snapshot safe to call inside GPUI `render()`.
    pub fn cached_permissions_snapshot() -> MacPermissionsSnapshot {
        if is_test_or_offline() {
            return MacPermissionsSnapshot {
                screen_state: ScreenPermissionState::GrantedActive,
                accessibility_granted: true,
                preflight_screen: true,
                fresh_probe_screen: Some(true),
                signing: MacSigningIdentity::default(),
                stale_screen_suspected: false,
                stale_ax_suspected: false,
                screen_settings_opened: false,
                ax_settings_opened: false,
                hud_visible: false,
                hud_target: None,
                last_drag_seq: 0,
                last_probe_detail: None,
            };
        }

        let drag_seq = unsafe { fabushi_permission_hud_drag_event_seq() };
        let mut trigger_drag_refresh = false;

        let snap = {
            let mut st = state_lock().lock();
            if drag_seq > st.last_seen_drag_seq {
                // Read last_dragged_kind (stored atomically before drag_event_seq increment
                // and before HUD auto-dismiss resets active_kind to -1).
                let dragged_kind =
                    hud_target_from_raw(unsafe { fabushi_permission_hud_last_dragged_kind() })
                        .or_else(|| {
                            hud_target_from_raw(unsafe { fabushi_permission_hud_active_kind() })
                        });
                let now = Instant::now();
                st.last_seen_drag_seq = drag_seq;
                match dragged_kind {
                    Some(PermissionKind::Accessibility) => {
                        st.ax_drag_completed = true;
                        st.ax_drag_at = Some(now);
                        st.ax_drag_probe_failed = false;
                    }
                    Some(PermissionKind::ScreenRecording) | None => {
                        st.screen_drag_completed = true;
                        st.screen_drag_at = Some(now);
                        st.screen_drag_probe_failed = false;
                    }
                }
                trigger_drag_refresh = true;
            }
            snapshot_from_state(&st)
        };

        if trigger_drag_refresh {
            SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
            request_permissions_refresh(true);
            schedule_post_drag_followup_probes();
        }

        snap
    }

    pub fn permissions_snapshot_generation() -> u64 {
        SNAPSHOT_GENERATION.load(Ordering::Acquire)
    }

    fn execute_single_background_probe(force: bool) {
        let (need_signing, screen_granted_at_launch) = {
            let st = state_lock().lock();
            (!st.signing_loaded, st.screen_granted_at_launch)
        };

        // Load full SecCode signing identity off the main thread once (avoids main-thread
        // SecTrustEvaluateIfNecessary warnings).
        let loaded_signing = if need_signing {
            Some(current_signing_identity())
        } else {
            None
        };

        let preflight_now = preflight_screen_capture_access();
        let ax_now = is_accessibility_trusted();

        // Only run the out-of-process fresh TCC probe when screen isn't already verified active
        // or when explicitly forced.
        let fresh = if is_sck_live_verified() && preflight_now && !force {
            Some((true, ax_now))
        } else {
            run_fresh_child_probe()
        };

        let fresh_screen = fresh.map(|(s, _)| s);
        let fresh_ax = fresh.map(|(_, a)| a);

        if fresh_screen == Some(false) {
            clear_sck_live_verified();
        }

        // Only probe ScreenCaptureKit live when Screen Recording was granted at launch and
        // TCC still reports granted — NEVER call SCShareableContent before restart when
        // !screen_granted_at_launch (prevents macOS 15+/27 ScreenCaptureKit consent popups!).
        let should_probe_sck = super::should_probe_sck_live(
            screen_granted_at_launch,
            preflight_now,
            fresh_screen,
            is_sck_live_verified(),
        );
        let sck_res = if should_probe_sck {
            Some(probe_sck_screen_access())
        } else {
            None
        };
        let can_read_names = if should_probe_sck {
            can_read_other_window_names()
        } else {
            false
        };

        let mut hide_hud_now = false;
        let changed;
        {
            let mut st = state_lock().lock();
            let before_snap = snapshot_from_state(&st);

            if let Some(full_signing) = loaded_signing {
                st.signing = full_signing;
                st.signing_loaded = true;
            }
            st.cached_preflight_screen = preflight_now;
            st.cached_ax_granted = ax_now;
            st.cached_can_read_window_names = can_read_names;
            st.last_probe_at = Some(Instant::now());

            if let Some(fs) = fresh_screen {
                st.fresh_probe_screen = Some(fs);
                if !fs {
                    st.sck_outcome = None;
                }
            }
            if let Some(fa) = fresh_ax {
                st.fresh_probe_ax = Some(fa);
            }
            if let Some(ref sck) = sck_res {
                st.sck_outcome = Some(sck.clone());
                st.last_probe_detail = match sck {
                    SckProbeOutcome::GrantedActive => None,
                    SckProbeOutcome::PermissionDenied(m) => Some(m.clone()),
                    SckProbeOutcome::Unavailable => None,
                    SckProbeOutcome::Error(m) => Some(m.clone()),
                };
            }

            let screen_tcc_granted = fresh_screen.unwrap_or(preflight_now);
            if screen_tcc_granted {
                st.screen_drag_probe_failed = false;
            } else if st.screen_drag_completed
                && fresh_screen == Some(false)
                && st
                    .screen_drag_at
                    .is_some_and(|t| t.elapsed() >= Duration::from_millis(850))
            {
                st.screen_drag_probe_failed = true;
            }

            if ax_now {
                st.ax_drag_probe_failed = false;
            } else if st.ax_drag_completed
                && !ax_now
                && st
                    .ax_drag_at
                    .is_some_and(|t| t.elapsed() >= Duration::from_millis(850))
            {
                st.ax_drag_probe_failed = true;
            }

            // Persist signing metadata whenever a permission is genuinely verified!
            let mut persisted_dirty = false;
            let screen_verified = (matches!(st.sck_outcome, Some(SckProbeOutcome::GrantedActive))
                || screen_tcc_granted)
                && fresh_screen != Some(false);
            if st.signing_loaded {
                if screen_verified
                    && !st.signing.cdhash_hex.is_empty()
                    && st.persisted.last_verified_screen_cdhash.as_deref()
                        != Some(st.signing.cdhash_hex.as_str())
                {
                    st.persisted.last_verified_screen_cdhash = Some(st.signing.cdhash_hex.clone());
                    persisted_dirty = true;
                }
                if ax_now
                    && !st.signing.cdhash_hex.is_empty()
                    && st.persisted.last_verified_ax_cdhash.as_deref()
                        != Some(st.signing.cdhash_hex.as_str())
                {
                    st.persisted.last_verified_ax_cdhash = Some(st.signing.cdhash_hex.clone());
                    persisted_dirty = true;
                }
                if screen_verified || ax_now {
                    if !st.signing.team_id.is_empty()
                        && st.persisted.last_verified_team_id.as_deref()
                            != Some(st.signing.team_id.as_str())
                    {
                        st.persisted.last_verified_team_id = Some(st.signing.team_id.clone());
                        persisted_dirty = true;
                    }
                    if st.persisted.last_was_adhoc != Some(st.signing.is_adhoc) {
                        st.persisted.last_was_adhoc = Some(st.signing.is_adhoc);
                        persisted_dirty = true;
                    }
                }
            }
            if persisted_dirty {
                let clone = st.persisted.clone();
                save_persisted_state(&clone);
            }

            // Auto-dismiss floating drag HUD session once its target permission is granted
            // (check active_kind directly so even a temporarily backgrounded HUD session ends!).
            let active_kind = hud_target_from_raw(unsafe { fabushi_permission_hud_active_kind() });
            match active_kind {
                Some(PermissionKind::Accessibility) if ax_now => {
                    hide_hud_now = true;
                }
                Some(PermissionKind::ScreenRecording)
                    if matches!(
                        compute_screen_state(&st),
                        ScreenPermissionState::GrantedActive
                            | ScreenPermissionState::GrantedNeedsRestart
                    ) =>
                {
                    hide_hud_now = true;
                }
                _ => {}
            }

            let after_snap = snapshot_from_state(&st);
            changed = after_snap != before_snap;
        }

        if hide_hud_now {
            hide_drag_hud();
        }
        if changed {
            SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Schedules an asynchronous non-blocking macOS permission probe (fresh child TCC check
    /// + safe ScreenCaptureKit verification) and updates the cached snapshot.
    pub fn request_permissions_refresh(force: bool) {
        if is_test_or_offline() {
            return;
        }

        if !force {
            let st = state_lock().lock();
            let active_setup = st.screen_settings_opened
                || st.ax_settings_opened
                || st.screen_drag_completed
                || st.ax_drag_completed
                || unsafe { fabushi_permission_hud_active_kind() >= 0 };
            let min_interval = if active_setup {
                Duration::from_millis(400)
            } else {
                Duration::from_millis(1200)
            };
            if let Some(last) = st.last_probe_at {
                if last.elapsed() < min_interval {
                    return;
                }
            }
        }

        if PROBE_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            if force {
                PROBE_PENDING_FORCE.store(true, Ordering::Release);
            }
            return;
        }

        std::thread::spawn(move || {
            struct InFlightGuard;
            impl Drop for InFlightGuard {
                fn drop(&mut self) {
                    PROBE_IN_FLIGHT.store(false, Ordering::Release);
                }
            }
            let _guard = InFlightGuard;

            execute_single_background_probe(force);
            while PROBE_PENDING_FORCE.swap(false, Ordering::AcqRel) {
                execute_single_background_probe(true);
            }
        });
    }

    pub fn has_screen_recording_permission() -> bool {
        cached_permissions_snapshot().screen_state.is_active()
    }

    pub fn has_accessibility_permission() -> bool {
        if is_test_or_offline() {
            return true;
        }
        is_accessibility_trusted()
    }

    fn tcc_service_name(kind: PermissionKind) -> &'static str {
        match kind {
            PermissionKind::ScreenRecording => "ScreenCapture",
            PermissionKind::Accessibility => "Accessibility",
        }
    }

    fn system_settings_url(kind: PermissionKind) -> &'static str {
        match kind {
            PermissionKind::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            PermissionKind::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
        }
    }

    fn reset_own_tcc_service(kind: PermissionKind, bundle_id: &str) {
        if is_test_or_offline() || bundle_id.trim().is_empty() {
            return;
        }
        let _ = Command::new("/usr/bin/tccutil")
            .args(["reset", tcc_service_name(kind), bundle_id.trim()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    fn show_drag_hud_native(kind: PermissionKind, lang: &str, signing: &MacSigningIdentity) {
        if is_test_or_offline() {
            return;
        }
        let app_name = signing.display_app_name();
        let app_path = signing.drag_target_path();
        let is_zh = lang.starts_with("zh");
        let (instruction, hint) = match (kind, is_zh) {
            (PermissionKind::ScreenRecording, true) => (
                format!("拖放应用，以允许“{app_name}”使用“录屏与系统录音”设置"),
                "按住拖入",
            ),
            (PermissionKind::ScreenRecording, false) => (
                format!(
                    "Drag the app to allow \"{app_name}\" in \"Screen & System Audio Recording\""
                ),
                "Drag",
            ),
            (PermissionKind::Accessibility, true) => (
                format!("拖放应用，以允许“{app_name}”使用“辅助功能”设置"),
                "按住拖入",
            ),
            (PermissionKind::Accessibility, false) => (
                format!("Drag the app to allow \"{app_name}\" in \"Accessibility\""),
                "Drag",
            ),
        };

        let kind_i32 = match kind {
            PermissionKind::ScreenRecording => 0,
            PermissionKind::Accessibility => 1,
        };

        let c_name = CString::new(app_name).unwrap_or_else(|_| CString::new("Fabushi").unwrap());
        let c_path = CString::new(app_path).unwrap_or_else(|_| CString::new("").unwrap());
        let c_inst = CString::new(instruction).unwrap_or_else(|_| CString::new("").unwrap());
        let c_hint = CString::new(hint).unwrap_or_else(|_| CString::new("").unwrap());

        unsafe {
            fabushi_permission_hud_show(
                kind_i32,
                c_name.as_ptr(),
                c_path.as_ptr(),
                c_inst.as_ptr(),
                c_hint.as_ptr(),
            );
        }
    }

    /// Opens the requested System Settings privacy pane AND displays the Gemini-style
    /// floating draggable App HUD anchored over System Settings.
    ///
    /// Before opening System Settings, runs a fast fresh out-of-process TCC check on a background
    /// thread: for any service where TCC currently denies our running bundle (whether ad-hoc OR
    /// Developer ID — e.g. stale ad-hoc `csreq` poisoning after upgrading to Developer ID),
    /// runs a one-time `tccutil reset` for our own bundle ID so dragging/toggling cleanly binds
    /// the current signature. Never invokes `CGRequestScreenCaptureAccess` or `AX` prompt alerts.
    pub fn open_permission_with_drag_hud(kind: PermissionKind, lang: &str) {
        if is_test_or_offline() {
            return;
        }

        let signing = {
            let mut st = state_lock().lock();
            match kind {
                PermissionKind::ScreenRecording => st.screen_settings_opened = true,
                PermissionKind::Accessibility => st.ax_settings_opened = true,
            }
            st.signing.clone()
        };

        // Show floating drag HUD immediately on main thread so it stays responsive
        show_drag_hud_native(kind, lang, &signing);

        let initial_bundle_id = signing.bundle_id.clone();
        let url = system_settings_url(kind);
        std::thread::spawn(move || {
            let bundle_id = if initial_bundle_id.trim().is_empty() {
                let full = current_signing_identity();
                let mut st = state_lock().lock();
                st.signing = full.clone();
                st.signing_loaded = true;
                full.bundle_id
            } else {
                initial_bundle_id
            };

            let preflight_now = preflight_screen_capture_access();
            let ax_now = is_accessibility_trusted();
            let fresh = run_fresh_child_probe();
            let fresh_screen = fresh.map(|(s, _)| s);
            let fresh_ax = fresh.map(|(_, a)| a);
            if fresh_screen == Some(false) {
                clear_sck_live_verified();
            }

            // If the clicked permission is ALREADY granted in TCC right now, never reset it —
            // update state, dismiss HUD, and immediately surface "重启 Fabushi" / "完成"!
            // Always prefer uncached `fresh_screen` over process-cached `preflight_now` so
            // runtime revocation (`fresh_screen == Some(false)`) is never masked!
            let clicked_already_granted = match kind {
                PermissionKind::ScreenRecording => fresh_screen.unwrap_or(preflight_now),
                PermissionKind::Accessibility => ax_now,
            };
            if clicked_already_granted {
                {
                    let mut st = state_lock().lock();
                    st.cached_preflight_screen = preflight_now;
                    st.cached_ax_granted = ax_now;
                    if let Some(fs) = fresh_screen {
                        st.fresh_probe_screen = Some(fs);
                        if !fs {
                            st.sck_outcome = None;
                        }
                    }
                    if let Some(fa) = fresh_ax {
                        st.fresh_probe_ax = Some(fa);
                    }
                    st.last_probe_at = Some(Instant::now());
                }
                hide_drag_hud();
                SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
                return;
            }

            // Decide one-time stale TCC cleanup for any currently-denied service (works for
            // BOTH Developer ID and ad-hoc builds; never wipes a working grant).
            let (reset_screen, reset_ax) = {
                let mut st = state_lock().lock();
                st.cached_preflight_screen = preflight_now;
                st.cached_ax_granted = ax_now;
                if let Some(fs) = fresh_screen {
                    st.fresh_probe_screen = Some(fs);
                    if !fs {
                        st.sck_outcome = None;
                    }
                }
                if let Some(fa) = fresh_ax {
                    st.fresh_probe_ax = Some(fa);
                }
                let do_screen = super::should_auto_reset_tcc_service(
                    &bundle_id,
                    st.screen_granted_at_launch,
                    st.auto_reset_screen_done,
                    preflight_now,
                    fresh_screen,
                );
                if do_screen {
                    st.auto_reset_screen_done = true;
                }
                let do_ax = super::should_auto_reset_tcc_service(
                    &bundle_id,
                    st.ax_granted_at_launch,
                    st.auto_reset_ax_done,
                    ax_now,
                    fresh_ax,
                );
                if do_ax {
                    st.auto_reset_ax_done = true;
                }
                (do_screen, do_ax)
            };

            if reset_screen {
                reset_own_tcc_service(PermissionKind::ScreenRecording, &bundle_id);
            }
            if reset_ax {
                reset_own_tcc_service(PermissionKind::Accessibility, &bundle_id);
            }
            if reset_screen || reset_ax {
                std::thread::sleep(Duration::from_millis(120));
            }

            // Open System Settings directly to the target privacy pane (ZERO OS TCC popups:
            // dragging Fabushi.app from the floating HUD registers & grants cleanly).
            let _ = Command::new("/usr/bin/open").arg(url).spawn();
        });

        SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
    }

    /// Explicitly resets a stale/broken TCC entry for our own bundle ID, re-opens System
    /// Settings, and displays the floating drag HUD so the user can freshly authorize this build.
    pub fn reset_stale_permission_and_reopen(kind: PermissionKind, lang: &str) {
        if is_test_or_offline() {
            return;
        }
        let signing = {
            let mut st = state_lock().lock();
            match kind {
                PermissionKind::ScreenRecording => {
                    st.screen_settings_opened = true;
                    st.screen_drag_completed = false;
                    st.screen_drag_at = None;
                    st.screen_drag_probe_failed = false;
                    st.fresh_probe_screen = Some(false);
                    st.sck_outcome = None;
                    st.auto_reset_screen_done = true;
                }
                PermissionKind::Accessibility => {
                    st.ax_settings_opened = true;
                    st.ax_drag_completed = false;
                    st.ax_drag_at = None;
                    st.ax_drag_probe_failed = false;
                    st.fresh_probe_ax = Some(false);
                    st.auto_reset_ax_done = true;
                }
            }
            st.signing.clone()
        };

        show_drag_hud_native(kind, lang, &signing);

        let initial_bundle_id = signing.bundle_id.clone();
        let url = system_settings_url(kind);
        std::thread::spawn(move || {
            let bundle_id = if initial_bundle_id.trim().is_empty() {
                current_signing_identity().bundle_id
            } else {
                initial_bundle_id
            };
            reset_own_tcc_service(kind, &bundle_id);
            std::thread::sleep(Duration::from_millis(120));
            let _ = Command::new("/usr/bin/open").arg(url).spawn();
        });

        SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
    }

    pub fn hide_drag_hud() {
        if is_test_or_offline() {
            return;
        }
        unsafe {
            fabushi_permission_hud_hide();
        }
        SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
    }

    pub fn open_screen_recording_settings() {
        open_permission_with_drag_hud(PermissionKind::ScreenRecording, "zh");
    }

    pub fn open_accessibility_settings() {
        open_permission_with_drag_hud(PermissionKind::Accessibility, "zh");
    }

    fn shell_single_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    /// Spawns a tiny detached relaunch helper that waits for the current PID to terminate
    /// and re-opens the exact running `.app` bundle (or executable) so macOS applies newly
    /// granted Screen Recording permissions immediately.
    pub fn relaunch_current_app() -> bool {
        if is_test_or_offline() {
            return false;
        }
        hide_drag_hud();

        let signing = { state_lock().lock().signing.clone() };
        let pid = std::process::id();
        let launch_cmd = if signing.is_app_bundle && !signing.bundle_path.is_empty() {
            format!("/usr/bin/open -n {}", shell_single_quote(&signing.bundle_path))
        } else if !signing.executable_path.is_empty() {
            format!("{} >/dev/null 2>&1 &", shell_single_quote(&signing.executable_path))
        } else {
            "/usr/bin/open -n '/Applications/Fabushi.app'".to_string()
        };

        let script = format!(
            "i=0; while [ \"$i\" -lt 60 ]; do if ! kill -0 {pid} 2>/dev/null; then break; fi; sleep 0.15; i=$((i+1)); done; sleep 0.25; {launch_cmd}"
        );

        Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn cached_permissions_snapshot() -> MacPermissionsSnapshot {
        MacPermissionsSnapshot {
            screen_state: ScreenPermissionState::GrantedActive,
            accessibility_granted: true,
            preflight_screen: true,
            fresh_probe_screen: Some(true),
            signing: MacSigningIdentity::default(),
            stale_screen_suspected: false,
            stale_ax_suspected: false,
            screen_settings_opened: false,
            ax_settings_opened: false,
            hud_visible: false,
            hud_target: None,
            last_drag_seq: 0,
            last_probe_detail: None,
        }
    }

    pub fn permissions_snapshot_generation() -> u64 {
        0
    }

    pub fn request_permissions_refresh(_force: bool) {}
    pub fn has_screen_recording_permission() -> bool {
        true
    }
    pub fn has_accessibility_permission() -> bool {
        true
    }
    pub fn open_permission_with_drag_hud(_kind: PermissionKind, _lang: &str) {}
    pub fn reset_stale_permission_and_reopen(_kind: PermissionKind, _lang: &str) {}
    pub fn hide_drag_hud() {}
    pub fn open_screen_recording_settings() {}
    pub fn open_accessibility_settings() {}
    pub fn relaunch_current_app() -> bool {
        false
    }
}

pub use imp::{
    cached_permissions_snapshot, has_accessibility_permission, has_screen_recording_permission,
    hide_drag_hud, open_accessibility_settings, open_permission_with_drag_hud,
    open_screen_recording_settings, permissions_snapshot_generation, relaunch_current_app,
    request_permissions_refresh, reset_stale_permission_and_reopen,
};
