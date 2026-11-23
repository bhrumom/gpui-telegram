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

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use fabushi_remote_engine::macos_permissions::{
        SckProbeOutcome, can_read_other_window_names, current_signing_identity,
        is_accessibility_trusted, is_sck_live_verified, preflight_screen_capture_access,
        probe_sck_screen_access,
    };
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use std::ffi::{CString, c_char, c_void};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    #[link(name = "CoreGraphics", kind = "framework")]
    #[link(name = "ApplicationServices", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;

        static kAXTrustedCheckOptionPrompt: *const c_void;
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;

        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

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
    }

    #[derive(Debug, Default, Clone, Serialize, Deserialize)]
    struct PersistedTccSignatureState {
        #[serde(default)]
        last_verified_screen_cdhash: Option<String>,
        #[serde(default)]
        last_verified_ax_cdhash: Option<String>,
        #[serde(default)]
        last_bundle_path: Option<String>,
    }

    #[derive(Debug)]
    struct RuntimePermState {
        signing: MacSigningIdentity,
        persisted: PersistedTccSignatureState,
        screen_granted_at_launch: bool,
        ax_granted_at_launch: bool,
        auto_reset_screen_done: bool,
        auto_reset_ax_done: bool,
        screen_settings_opened: bool,
        ax_settings_opened: bool,
        screen_drag_completed: bool,
        ax_drag_completed: bool,
        last_seen_drag_seq: u64,
        last_probe_at: Option<Instant>,
        sck_outcome: Option<SckProbeOutcome>,
        fresh_probe_screen: Option<bool>,
        fresh_probe_ax: Option<bool>,
        last_probe_detail: Option<String>,
    }

    static STATE: OnceLock<Mutex<RuntimePermState>> = OnceLock::new();
    static PROBE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
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
            let signing = if is_test_or_offline() {
                MacSigningIdentity::default()
            } else {
                current_signing_identity()
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
            if !signing.cdhash_hex.is_empty() {
                if screen_at_launch
                    && persisted_mut.last_verified_screen_cdhash.as_deref()
                        != Some(signing.cdhash_hex.as_str())
                {
                    persisted_mut.last_verified_screen_cdhash = Some(signing.cdhash_hex.clone());
                    dirty = true;
                }
                if ax_at_launch
                    && persisted_mut.last_verified_ax_cdhash.as_deref()
                        != Some(signing.cdhash_hex.as_str())
                {
                    persisted_mut.last_verified_ax_cdhash = Some(signing.cdhash_hex.clone());
                    dirty = true;
                }
            }
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
                persisted: persisted_mut,
                screen_granted_at_launch: screen_at_launch,
                ax_granted_at_launch: ax_at_launch,
                auto_reset_screen_done: false,
                auto_reset_ax_done: false,
                screen_settings_opened: false,
                ax_settings_opened: false,
                screen_drag_completed: false,
                ax_drag_completed: false,
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

    fn compute_screen_state(st: &RuntimePermState, preflight_now: bool) -> ScreenPermissionState {
        if is_test_or_offline() {
            return ScreenPermissionState::GrantedActive;
        }
        if is_sck_live_verified() || matches!(st.sck_outcome, Some(SckProbeOutcome::GrantedActive)) {
            return ScreenPermissionState::GrantedActive;
        }
        let fresh_screen = st.fresh_probe_screen.unwrap_or(false);
        let tcc_says_granted = preflight_now || fresh_screen;
        if tcc_says_granted {
            // If we explicitly probed SCK and it succeeded -> handled above.
            // If SCK returned PermissionDenied while TCC says granted -> definitely needs restart!
            if matches!(st.sck_outcome, Some(SckProbeOutcome::PermissionDenied(_))) {
                return ScreenPermissionState::GrantedNeedsRestart;
            }
            // If launched without Screen Recording and now TCC says granted -> needs restart
            // unless SCK live probe proves active!
            if !st.screen_granted_at_launch {
                return ScreenPermissionState::GrantedNeedsRestart;
            }
            // If launched with Screen Recording already granted and SCK is unavailable (macOS < 14)
            // or not yet probed, trust preflight/window-names check.
            if matches!(st.sck_outcome, Some(SckProbeOutcome::Unavailable))
                || st.sck_outcome.is_none()
            {
                return ScreenPermissionState::GrantedActive;
            }
            if can_read_other_window_names() {
                return ScreenPermissionState::GrantedActive;
            }
            return ScreenPermissionState::GrantedNeedsRestart;
        }
        ScreenPermissionState::NotGranted
    }

    fn snapshot_from_state(st: &RuntimePermState) -> MacPermissionsSnapshot {
        let preflight_now = preflight_screen_capture_access();
        let ax_now = is_accessibility_trusted();
        let screen_state = compute_screen_state(st, preflight_now);

        let cdhash = st.signing.cdhash_hex.as_str();
        let screen_cdhash_changed = !cdhash.is_empty()
            && st
                .persisted
                .last_verified_screen_cdhash
                .as_deref()
                .is_some_and(|prev| prev != cdhash);
        let ax_cdhash_changed = !cdhash.is_empty()
            && st
                .persisted
                .last_verified_ax_cdhash
                .as_deref()
                .is_some_and(|prev| prev != cdhash);

        let stale_screen_suspected = screen_state == ScreenPermissionState::NotGranted
            && st.signing.is_adhoc
            && (screen_cdhash_changed
                || st.screen_drag_completed
                || (st.screen_settings_opened && st.fresh_probe_screen == Some(false)));

        let stale_ax_suspected = !ax_now
            && st.signing.is_adhoc
            && (ax_cdhash_changed
                || st.ax_drag_completed
                || (st.ax_settings_opened && st.fresh_probe_ax == Some(false)));

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
        let hud_kind = hud_target_from_raw(unsafe { fabushi_permission_hud_active_kind() });
        let mut trigger_drag_refresh = false;

        let snap = {
            let mut st = state_lock().lock();
            if drag_seq > st.last_seen_drag_seq {
                st.last_seen_drag_seq = drag_seq;
                match hud_kind {
                    Some(PermissionKind::Accessibility) => st.ax_drag_completed = true,
                    Some(PermissionKind::ScreenRecording) | None => {
                        st.screen_drag_completed = true;
                    }
                }
                trigger_drag_refresh = true;
            }
            snapshot_from_state(&st)
        };

        if trigger_drag_refresh {
            request_permissions_refresh(true);
        }

        snap
    }

    pub fn permissions_snapshot_generation() -> u64 {
        SNAPSHOT_GENERATION.load(Ordering::Acquire)
    }

    /// Schedules an asynchronous non-blocking macOS permission probe (fresh child TCC check
    /// + safe ScreenCaptureKit verification) and updates the cached snapshot.
    pub fn request_permissions_refresh(force: bool) {
        if is_test_or_offline() {
            return;
        }

        {
            let st = state_lock().lock();
            if !force {
                if let Some(last) = st.last_probe_at {
                    if last.elapsed() < Duration::from_millis(750) {
                        return;
                    }
                }
            }
        }

        if PROBE_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
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

            let preflight_now = preflight_screen_capture_access();
            let ax_now = is_accessibility_trusted();

            // Only run the out-of-process fresh TCC probe when screen isn't already verified active
            let fresh = if is_sck_live_verified() && !force {
                Some((true, ax_now))
            } else {
                run_fresh_child_probe()
            };

            let fresh_screen = fresh.map(|(s, _)| s);
            let fresh_ax = fresh.map(|(_, a)| a);

            // Only probe ScreenCaptureKit live when TCC already reports granted (zero popup!)
            // or when explicitly forced after a user drag/drop.
            let should_probe_sck =
                preflight_now || fresh_screen == Some(true) || is_sck_live_verified();
            let sck_res = if should_probe_sck {
                Some(probe_sck_screen_access())
            } else {
                None
            };

            let mut hide_hud_now = false;
            {
                let mut st = state_lock().lock();
                st.last_probe_at = Some(Instant::now());
                if let Some(fs) = fresh_screen {
                    st.fresh_probe_screen = Some(fs);
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

                // Persist CDHash whenever a permission is genuinely verified!
                let mut persisted_dirty = false;
                let screen_verified =
                    matches!(st.sck_outcome, Some(SckProbeOutcome::GrantedActive))
                        || fresh_screen == Some(true)
                        || preflight_now;
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
                if persisted_dirty {
                    let clone = st.persisted.clone();
                    save_persisted_state(&clone);
                }

                // Auto-dismiss floating drag HUD once its target permission is granted!
                let hud_visible = unsafe { fabushi_permission_hud_is_visible() };
                if hud_visible {
                    let active_kind =
                        hud_target_from_raw(unsafe { fabushi_permission_hud_active_kind() });
                    match active_kind {
                        Some(PermissionKind::Accessibility) if ax_now => {
                            hide_hud_now = true;
                        }
                        Some(PermissionKind::ScreenRecording)
                            if matches!(
                                compute_screen_state(&st, preflight_now),
                                ScreenPermissionState::GrantedActive
                                    | ScreenPermissionState::GrantedNeedsRestart
                            ) =>
                        {
                            hide_hud_now = true;
                        }
                        _ => {}
                    }
                }
            }

            if hide_hud_now {
                hide_drag_hud();
            }

            SNAPSHOT_GENERATION.fetch_add(1, Ordering::AcqRel);
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

    fn register_with_macos_tcc(kind: PermissionKind) {
        match kind {
            PermissionKind::ScreenRecording => unsafe {
                let _ = CGRequestScreenCaptureAccess();
            },
            PermissionKind::Accessibility => unsafe {
                let keys = [kAXTrustedCheckOptionPrompt];
                let values = [kCFBooleanTrue];
                let opts = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks as *const c_void,
                    &kCFTypeDictionaryValueCallBacks as *const c_void,
                );
                if !opts.is_null() {
                    let _ = AXIsProcessTrustedWithOptions(opts);
                    CFRelease(opts);
                }
            },
        }
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
                format!("Drag the app to allow \"{app_name}\" in \"Screen & System Audio Recording\""),
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
    /// If the running app is ad-hoc signed and TCC denied this process at launch (meaning any
    /// pre-existing ON toggle in System Settings is provably a stale CDHash ghost entry from an
    /// older build), clears only our own stale entry once before opening System Settings so
    /// dragging/toggling cleanly binds the current build's signature.
    pub fn open_permission_with_drag_hud(kind: PermissionKind, lang: &str) {
        if is_test_or_offline() {
            return;
        }

        let (signing, should_auto_reset) = {
            let mut st = state_lock().lock();
            match kind {
                PermissionKind::ScreenRecording => st.screen_settings_opened = true,
                PermissionKind::Accessibility => st.ax_settings_opened = true,
            }
            let auto_reset = match kind {
                PermissionKind::ScreenRecording => {
                    let eligible = st.signing.is_adhoc
                        && !st.signing.bundle_id.is_empty()
                        && !st.screen_granted_at_launch
                        && !st.auto_reset_screen_done
                        && st.fresh_probe_screen != Some(true)
                        && !preflight_screen_capture_access();
                    if eligible {
                        st.auto_reset_screen_done = true;
                    }
                    eligible
                }
                PermissionKind::Accessibility => {
                    let eligible = st.signing.is_adhoc
                        && !st.signing.bundle_id.is_empty()
                        && !st.ax_granted_at_launch
                        && !st.auto_reset_ax_done
                        && !is_accessibility_trusted();
                    if eligible {
                        st.auto_reset_ax_done = true;
                    }
                    eligible
                }
            };
            (st.signing.clone(), auto_reset)
        };

        // Show floating drag HUD immediately on main thread so it stays responsive
        show_drag_hud_native(kind, lang, &signing);

        let bundle_id = signing.bundle_id.clone();
        let url = system_settings_url(kind);
        std::thread::spawn(move || {
            if should_auto_reset {
                reset_own_tcc_service(kind, &bundle_id);
            }
            // Register entry in macOS TCC list (so user can EITHER toggle OR drag)
            register_with_macos_tcc(kind);
            // Open System Settings directly to the target privacy pane
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
                    st.fresh_probe_screen = Some(false);
                    st.sck_outcome = None;
                    st.auto_reset_screen_done = true;
                }
                PermissionKind::Accessibility => {
                    st.ax_settings_opened = true;
                    st.ax_drag_completed = false;
                    st.fresh_probe_ax = Some(false);
                    st.auto_reset_ax_done = true;
                }
            }
            st.signing.clone()
        };

        show_drag_hud_native(kind, lang, &signing);

        let bundle_id = signing.bundle_id.clone();
        let url = system_settings_url(kind);
        std::thread::spawn(move || {
            reset_own_tcc_service(kind, &bundle_id);
            std::thread::sleep(Duration::from_millis(120));
            register_with_macos_tcc(kind);
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
            "i=0; while [ \"$i\" -lt 60 ]; do if ! kill -0 {pid} 2>/dev/null; then break; fi; sleep 0.15; i=$((i+1)); done; {launch_cmd}"
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
