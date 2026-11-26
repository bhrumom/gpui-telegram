// Source: Telegram/SourceFiles/settings/sections/settings_privacy_security.cpp

use bot_browser::{CredentialMeta, ReusePolicy};
use crate::components::ui::pop_last_char;
use crate::i18n::tr_str;
use crate::theme::assets::{tg_icon, IconName};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::computer_workbench::render_ime_text_preview;
use crate::views::settings::settings_theme_ui::{
    settings_action_row, settings_card, settings_container, settings_info_row,
    settings_section_header, settings_subsection_title, settings_toggle_row,
};
use crate::views::settings::settings_type::SettingsSectionType;
use gpui::prelude::FluentBuilder;
use gpui::*;
use zeroize::{Zeroize, Zeroizing};

/// Real-time Zero-Knowledge Vault custody state displayed in Settings → Privacy & Security.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VaultCustodyStatus {
    #[default]
    LockedOrUninitialized,
    UnlockedAndSynced,
    AwaitingDeviceApproval {
        request_id: String,
        verification_code: String,
        expires_at: i64,
    },
    NeedsRecoveryKey,
}

/// Non-secret summary of a pending X25519 Device Trust Ring sync request from another device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDeviceSyncRow {
    pub request_id: String,
    pub device_name: String,
    pub device_public_key_b64: String,
    pub verification_code: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// User actions emitted from the Privacy & Security Zero-Knowledge Vault panel.
#[derive(Clone, PartialEq, Eq)]
pub enum SettingsVaultAction {
    RefreshVaultState,
    CopyEmergencyKit,
    SubmitRecoveryKey(Zeroizing<String>),
    RequestDeviceSync,
    ApproveDeviceSync {
        request_id: String,
        device_public_key_b64: String,
    },
    DenyDeviceSync {
        request_id: String,
    },
    ToggleCredentialAutoReuse {
        credential_id: String,
        allow_auto_reuse: bool,
    },
    DeleteCredential {
        credential_id: String,
    },
}

impl std::fmt::Debug for SettingsVaultAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RefreshVaultState => f.write_str("RefreshVaultState"),
            Self::CopyEmergencyKit => f.write_str("CopyEmergencyKit"),
            Self::SubmitRecoveryKey(_) => f
                .debug_tuple("SubmitRecoveryKey")
                .field(&"[REDACTED]")
                .finish(),
            Self::RequestDeviceSync => f.write_str("RequestDeviceSync"),
            Self::ApproveDeviceSync {
                request_id,
                device_public_key_b64,
            } => f
                .debug_struct("ApproveDeviceSync")
                .field("request_id", request_id)
                .field("device_public_key_b64", device_public_key_b64)
                .finish(),
            Self::DenyDeviceSync { request_id } => f
                .debug_struct("DenyDeviceSync")
                .field("request_id", request_id)
                .finish(),
            Self::ToggleCredentialAutoReuse {
                credential_id,
                allow_auto_reuse,
            } => f
                .debug_struct("ToggleCredentialAutoReuse")
                .field("credential_id", credential_id)
                .field("allow_auto_reuse", allow_auto_reuse)
                .finish(),
            Self::DeleteCredential { credential_id } => f
                .debug_struct("DeleteCredential")
                .field("credential_id", credential_id)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub struct SettingsPrivacySecuritySections {
    pub blocked_count: usize,
    pub passcode_enabled: bool,
    pub two_factor_enabled: bool,
    pub active_sessions_count: usize,
    pub ttl_months: u32,
    pub global_ttl_seconds: u32,
    pub sensitive_content_enabled: bool,
    pub phone_privacy: String,
    pub last_seen_privacy: String,
    pub photo_privacy: String,
    pub calls_privacy: String,
    pub forwards_privacy: String,
    pub groups_privacy: String,
    pub two_factor_editing: bool,
    pub two_factor_password_draft: String,
    pub vault_status: VaultCustodyStatus,
    pub vault_keychain_saved: bool,
    pub vault_last_sync_label: Option<String>,
    pub vault_last_sync_count: usize,
    pub vault_sync_revision: i64,
    pub vault_status_note: Option<String>,
    pub session_emergency_kit: Option<Zeroizing<String>>,
    pub recovery_key_editing: bool,
    pub recovery_key_draft: Zeroizing<String>,
    pub recovery_key_ime_preedit: Option<String>,
    pub pending_device_sync_requests: Vec<PendingDeviceSyncRow>,
    pub saved_credentials: Vec<CredentialMeta>,
    pub pending_vault_action: Option<SettingsVaultAction>,
    pub pending_nav_section: Option<SettingsSectionType>,
    pub pending_edit_privacy_category: Option<String>,
    pub pending_2fa_action: Option<Option<String>>,
    pub pending_save_settings: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl std::fmt::Debug for SettingsPrivacySecuritySections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsPrivacySecuritySections")
            .field("blocked_count", &self.blocked_count)
            .field("passcode_enabled", &self.passcode_enabled)
            .field("two_factor_enabled", &self.two_factor_enabled)
            .field("active_sessions_count", &self.active_sessions_count)
            .field("ttl_months", &self.ttl_months)
            .field("global_ttl_seconds", &self.global_ttl_seconds)
            .field("vault_status", &self.vault_status)
            .field("vault_keychain_saved", &self.vault_keychain_saved)
            .field("vault_sync_revision", &self.vault_sync_revision)
            .field("has_emergency_kit", &self.session_emergency_kit.is_some())
            .field(
                "pending_device_sync_requests",
                &self.pending_device_sync_requests.len(),
            )
            .field("saved_credentials_count", &self.saved_credentials.len())
            .finish()
    }
}

impl SettingsPrivacySecuritySections {
    pub fn new() -> Self {
        Self {
            blocked_count: 0,
            passcode_enabled: false,
            two_factor_enabled: true,
            active_sessions_count: 2,
            ttl_months: 6,
            global_ttl_seconds: 0,
            sensitive_content_enabled: false,
            phone_privacy: "Contacts".into(),
            last_seen_privacy: "Everybody".into(),
            photo_privacy: "Everybody".into(),
            calls_privacy: "Contacts".into(),
            forwards_privacy: "Everybody".into(),
            groups_privacy: "Contacts".into(),
            two_factor_editing: false,
            two_factor_password_draft: String::new(),
            vault_status: VaultCustodyStatus::LockedOrUninitialized,
            vault_keychain_saved: false,
            vault_last_sync_label: None,
            vault_last_sync_count: 0,
            vault_sync_revision: 0,
            vault_status_note: None,
            session_emergency_kit: None,
            recovery_key_editing: false,
            recovery_key_draft: Zeroizing::new(String::new()),
            recovery_key_ime_preedit: None,
            pending_device_sync_requests: Vec::new(),
            saved_credentials: Vec::new(),
            pending_vault_action: None,
            pending_nav_section: None,
            pending_edit_privacy_category: None,
            pending_2fa_action: None,
            pending_save_settings: false,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12),
            language_id: "en".into(),
        }
    }

    pub fn new_live() -> Self {
        let mut s = Self::new();
        s.two_factor_enabled = false;
        s.active_sessions_count = 0;
        s
    }

    pub fn submit_recovery_key_draft(&mut self) -> bool {
        let trimmed = self.recovery_key_draft.trim().to_string();
        self.recovery_key_draft.zeroize();
        self.recovery_key_ime_preedit = None;
        self.recovery_key_editing = false;
        if trimmed.is_empty() {
            return false;
        }
        self.pending_vault_action =
            Some(SettingsVaultAction::SubmitRecoveryKey(Zeroizing::new(trimmed)));
        true
    }

    pub fn backspace_recovery_key_draft(&mut self) -> bool {
        let mut buf = std::mem::take(&mut *self.recovery_key_draft);
        let changed = pop_last_char(&mut buf);
        *self.recovery_key_draft = buf;
        changed
    }

    pub fn sync_vault_custody_state(
        &mut self,
        status: VaultCustodyStatus,
        keychain_saved: bool,
        last_sync_label: Option<String>,
        last_sync_count: usize,
        sync_revision: i64,
        status_note: Option<String>,
        session_emergency_kit: Option<Zeroizing<String>>,
        pending_device_sync_requests: Vec<PendingDeviceSyncRow>,
        saved_credentials: Vec<CredentialMeta>,
    ) {
        self.vault_status = status;
        self.vault_keychain_saved = keychain_saved;
        self.vault_last_sync_label = last_sync_label;
        self.vault_last_sync_count = last_sync_count;
        self.vault_sync_revision = sync_revision;
        self.vault_status_note = status_note;
        self.session_emergency_kit = session_emergency_kit;
        self.pending_device_sync_requests = pending_device_sync_requests;
        self.saved_credentials = saved_credentials;
    }

    pub fn clear_session_custody_state(&mut self) {
        self.vault_status = VaultCustodyStatus::LockedOrUninitialized;
        self.vault_keychain_saved = false;
        self.vault_last_sync_label = None;
        self.vault_last_sync_count = 0;
        self.vault_sync_revision = 0;
        self.vault_status_note = None;
        if let Some(ref mut ek) = self.session_emergency_kit {
            ek.zeroize();
        }
        self.session_emergency_kit = None;
        self.two_factor_editing = false;
        self.two_factor_password_draft.zeroize();
        if let Some(Some(ref mut pw)) = self.pending_2fa_action {
            pw.zeroize();
        }
        self.pending_2fa_action = None;
        self.recovery_key_editing = false;
        self.recovery_key_draft.zeroize();
        self.recovery_key_ime_preedit = None;
        self.pending_device_sync_requests.clear();
        self.saved_credentials.clear();
        self.pending_vault_action = None;
        self.pending_nav_section = None;
        self.pending_edit_privacy_category = None;
        self.pending_save_settings = false;
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, lang: impl Into<String>) { self.language_id = lang.into(); }
    pub fn set_blocked_count(&mut self, count: usize) { self.blocked_count = count; }
    pub fn set_passcode_enabled(&mut self, enabled: bool) { self.passcode_enabled = enabled; }
    pub fn set_two_factor_enabled(&mut self, enabled: bool) { self.two_factor_enabled = enabled; }

    pub fn set_ttl_months(&mut self, months: u32) {
        self.ttl_months = match months {
            1 | 3 | 6 | 12 | 18 | 24 => months,
            _ => 6,
        };
    }

    pub fn toggle_sensitive_content(&mut self) {
        self.sensitive_content_enabled = !self.sensitive_content_enabled;
        self.pending_save_settings = true;
    }

    pub fn ttl_description(&self) -> String {
        match self.ttl_months {
            1 => "1 month".into(),
            3 => "3 months".into(),
            6 => "6 months".into(),
            12 => "1 year".into(),
            18 => "18 months".into(),
            24 => "2 years".into(),
            n => format!("{} months", n),
        }
    }

    pub fn ttl_description_localized(&self, lang: &str) -> String {
        if !lang.starts_with("zh") {
            return self.ttl_description();
        }
        match self.ttl_months {
            1 => "1 个月".into(),
            3 => "3 个月".into(),
            6 => "6 个月".into(),
            12 => "1 年".into(),
            18 => "18 个月".into(),
            24 => "2 年".into(),
            n => format!("{} 个月", n),
        }
    }

    pub fn global_ttl_label(&self) -> &'static str {
        match self.global_ttl_seconds {
            0 => "Off",
            86_400 => "1 Day",
            604_800 => "1 Week",
            2_678_400 | 2_592_000 => "1 Month",
            _ => "Custom",
        }
    }

    pub fn global_ttl_label_localized(&self, lang: &str) -> &'static str {
        match self.global_ttl_seconds {
            0 => tr_str(lang, "关闭", "Off"),
            86_400 => tr_str(lang, "1 天", "1 Day"),
            604_800 => tr_str(lang, "1 周", "1 Week"),
            2_678_400 | 2_592_000 => tr_str(lang, "1 个月", "1 Month"),
            _ => tr_str(lang, "自定义", "Custom"),
        }
    }

    fn localize_privacy_value<'a>(val: &'a str, lang: &str) -> &'a str {
        if !lang.starts_with("zh") {
            return val;
        }
        match val {
            "Everybody" => "所有人",
            "Contacts" | "My Contacts" => "我的联系人",
            "Nobody" => "没有人",
            "Premium" | "Premium Only" => "仅限高级会员",
            "Paid (Stars)" => "星币付费",
            _ => val,
        }
    }
}

impl SettingsPrivacySecuritySections {
    fn render_vault_custody_card(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let (status_label, status_bg, status_fg) = match &self.vault_status {
            VaultCustodyStatus::UnlockedAndSynced => (
                tr_str(lang, "已解锁并同步", "Unlocked & Synced").to_string(),
                TdesktopThemePalette::rgba_with_alpha(pal.success, 0.18),
                pal.success,
            ),
            VaultCustodyStatus::AwaitingDeviceApproval {
                verification_code, ..
            } => (
                if lang.starts_with("zh") {
                    format!("等待设备批准 · 验证码 {verification_code}")
                } else {
                    format!("Awaiting Device Approval (Verification Code {verification_code})")
                },
                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18),
                pal.accent,
            ),
            VaultCustodyStatus::NeedsRecoveryKey => (
                tr_str(lang, "需要恢复密钥解锁", "Needs Recovery Key").to_string(),
                TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.18),
                pal.danger,
            ),
            VaultCustodyStatus::LockedOrUninitialized => (
                tr_str(lang, "本地未同步", "Locked / Local").to_string(),
                TdesktopThemePalette::rgba_with_alpha(pal.text_secondary, 0.16),
                pal.text_secondary,
            ),
        };

        let keychain_val = if self.vault_keychain_saved {
            tr_str(lang, "已安全存入系统钥匙串", "Stored in OS Keychain")
        } else {
            tr_str(lang, "当前未保存在系统钥匙串", "Not saved in OS Keychain")
        };

        let sync_val = {
            let count_lbl = if lang.starts_with("zh") {
                format!(
                    "{} 条凭据 · rev {}",
                    self.saved_credentials.len(),
                    self.vault_sync_revision
                )
            } else {
                format!(
                    "{} credential(s) · rev {}",
                    self.saved_credentials.len(),
                    self.vault_sync_revision
                )
            };
            match self.vault_last_sync_label.as_deref().filter(|s| !s.is_empty()) {
                Some(ts) => format!("{ts} · {count_lbl}"),
                None => count_lbl,
            }
        };

        let awaiting_code = match &self.vault_status {
            VaultCustodyStatus::AwaitingDeviceApproval {
                verification_code, ..
            } => Some(verification_code.clone()),
            _ => None,
        };
        let needs_recovery_or_sync = matches!(
            self.vault_status,
            VaultCustodyStatus::NeedsRecoveryKey | VaultCustodyStatus::AwaitingDeviceApproval { .. }
        );
        let has_session_ek = self.session_emergency_kit.is_some();
        let pending_reqs = self.pending_device_sync_requests.clone();
        let saved_creds = self.saved_credentials.clone();
        let status_note = self.vault_status_note.clone();

        settings_card(pal)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(settings_subsection_title(
                        tr_str(
                            lang,
                            "安全保管库与多端信任 (Zero-Knowledge Vault)",
                            "Zero-Knowledge Vault & Device Trust",
                        ),
                        pal,
                    ))
                    .child(
                        div()
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(rgb(pal.border_fg))
                            .hover(move |s| s.bg(rgb(pal.hover_bg)))
                            .text_size(px(11.0))
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _e, _w, cx| {
                                    this.pending_vault_action =
                                        Some(SettingsVaultAction::RefreshVaultState);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "刷新", "Refresh")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(tg_icon(IconName::Lock, 15.0, status_fg))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "保管库状态", "Vault Status")),
                            ),
                    )
                    .child(
                        div()
                            .px(px(9.0))
                            .py(px(3.0))
                            .rounded(px(999.0))
                            .bg(status_bg)
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(status_fg))
                            .child(status_label),
                    ),
            )
            .child(settings_info_row(
                tr_str(lang, "钥匙串托管", "OS Keychain"),
                keychain_val,
                pal,
            ))
            .child(settings_info_row(
                tr_str(lang, "同步记录", "Sync Summary"),
                &sync_val,
                pal,
            ))
            .when_some(awaiting_code, |el, code| {
                el.child(
                    div()
                        .w_full()
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10))
                        .border_1()
                        .border_color(rgb(pal.accent))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(if lang.starts_with("zh") {
                                    format!("配对验证码: {code}")
                                } else {
                                    format!("Device Verification Code: {code}")
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "请在已登录的其它可信设备上打开「设置 → 隐私与安全」，核对此验证码并点击批准。",
                                    "Open Settings → Privacy & Security on an already trusted device, verify this code, and approve.",
                                )),
                        ),
                )
            })
            .when(needs_recovery_or_sync, |el| {
                el.child(
                    div()
                        .w_full()
                        .flex()
                        .justify_end()
                        .child(
                            div()
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(pal.accent))
                                .hover(move |s| s.bg(rgb(pal.hover_bg)))
                                .text_size(px(11.5))
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _e, _w, cx| {
                                        this.pending_vault_action =
                                            Some(SettingsVaultAction::RequestDeviceSync);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(
                                    lang,
                                    "发起/重试多端信任请求",
                                    "Request Trusted Device Sync",
                                )),
                        ),
                )
            })
            .when_some(status_note, |el, note| {
                el.child(
                    div()
                        .w_full()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .text_size(px(11.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(note),
                )
            })
            .when(!pending_reqs.is_empty(), |el| {
                el.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .pt(px(4.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.accent))
                                .child(tr_str(
                                    lang,
                                    "待批准的新设备同步请求",
                                    "Pending Device Sync Approvals",
                                )),
                        )
                        .children(pending_reqs.into_iter().map(|req| {
                            let req_id_approve = req.request_id.clone();
                            let req_pub_approve = req.device_public_key_b64.clone();
                            let req_id_deny = req.request_id.clone();
                            div()
                                .w_full()
                                .p(px(9.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(pal.border_fg))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(req.device_name),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(pal.accent))
                                                .child(if lang.starts_with("zh") {
                                                    format!("验证码: {}", req.verification_code)
                                                } else {
                                                    format!("Code: {}", req.verification_code)
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .cursor_pointer()
                                                .px(px(9.0))
                                                .py(px(4.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(pal.danger))
                                                .text_size(px(11.5))
                                                .text_color(rgb(pal.active_button_fg))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _e, _w, cx| {
                                                        this.pending_vault_action =
                                                            Some(SettingsVaultAction::DenyDeviceSync {
                                                                request_id: req_id_deny.clone(),
                                                            });
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tr_str(lang, "拒绝", "Deny")),
                                        )
                                        .child(
                                            div()
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(4.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(pal.accent))
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.active_button_fg))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _e, _w, cx| {
                                                        this.pending_vault_action = Some(
                                                            SettingsVaultAction::ApproveDeviceSync {
                                                                request_id: req_id_approve.clone(),
                                                                device_public_key_b64: req_pub_approve
                                                                    .clone(),
                                                            },
                                                        );
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tr_str(lang, "批准", "Approve")),
                                        ),
                                )
                        })),
                )
            })
            .child(
                div()
                    .w_full()
                    .pt(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "恢复密钥 (Emergency Kit)", "Emergency Kit")),
                    )
                    .when(has_session_ek, |el| {
                        el.child(
                            div()
                                .w_full()
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(8.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.10))
                                .border_1()
                                .border_color(rgb(pal.success))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(
                                            lang,
                                            "本次会话已生成一次性恢复密钥 (EK-••••)",
                                            "Session Emergency Kit ready (EK-••••)",
                                        )),
                                )
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(pal.accent))
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.active_button_fg))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _e, _w, cx| {
                                                this.pending_vault_action =
                                                    Some(SettingsVaultAction::CopyEmergencyKit);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "复制恢复密钥", "Copy Emergency Kit")),
                                ),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(if self.recovery_key_editing {
                                        rgb(pal.accent)
                                    } else {
                                        rgb(pal.border_fg)
                                    })
                                    .cursor_text()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _e, _w, cx| {
                                            this.recovery_key_editing = true;
                                            this.two_factor_editing = false;
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_ime_text_preview(
                                        self.recovery_key_draft.as_str(),
                                        self.recovery_key_ime_preedit.as_deref(),
                                        tr_str(
                                            lang,
                                            "输入恢复密钥 EK-...",
                                            "Enter Emergency Kit EK-...",
                                        ),
                                        self.recovery_key_editing,
                                        false,
                                        pal.text_primary,
                                        pal,
                                    )),
                            )
                            .child(
                                div()
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.accent))
                                    .hover(move |s| s.bg(rgb(pal.accent_hover)))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _e, _w, cx| {
                                            if this.submit_recovery_key_draft() {
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child(tr_str(lang, "使用恢复密钥解锁", "Unlock")),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .pt(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(if lang.starts_with("zh") {
                                format!("已保存网站凭据 ({})", saved_creds.len())
                            } else {
                                format!("Saved Site Credentials ({})", saved_creds.len())
                            }),
                    )
                    .when(saved_creds.is_empty(), |el| {
                        el.child(
                            div()
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "暂无已保存的网站登录凭据（密码仅本地加密保存）",
                                    "No saved site credentials yet (passwords remain encrypted client-side)",
                                )),
                        )
                    })
                    .children(saved_creds.into_iter().map(|cred| {
                        let cid_toggle = cred.credential_id.clone();
                        let cid_delete = cred.credential_id.clone();
                        let is_auto = cred.reuse_policy == ReusePolicy::AutoReuse;
                        div()
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(rgb(pal.border_fg))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(cred.origin),
                                            )
                                            .when(cred.has_totp, |badge| {
                                                badge.child(
                                                    div()
                                                        .px(px(5.0))
                                                        .py(px(1.0))
                                                        .rounded(px(4.0))
                                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                                            pal.accent, 0.18,
                                                        ))
                                                        .text_size(px(10.0))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(pal.accent))
                                                        .child("2FA"),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(cred.account_hint),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(6.0))
                                            .bg(if is_auto {
                                                TdesktopThemePalette::rgba_with_alpha(
                                                    pal.success, 0.18,
                                                )
                                            } else {
                                                TdesktopThemePalette::rgba_with_alpha(
                                                    pal.text_secondary,
                                                    0.15,
                                                )
                                            })
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if is_auto {
                                                pal.success
                                            } else {
                                                pal.text_secondary
                                            }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _e, _w, cx| {
                                                    this.pending_vault_action = Some(
                                                        SettingsVaultAction::ToggleCredentialAutoReuse {
                                                            credential_id: cid_toggle.clone(),
                                                            allow_auto_reuse: !is_auto,
                                                        },
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(if is_auto {
                                                tr_str(lang, "自动复用", "Auto Reuse")
                                            } else {
                                                tr_str(lang, "每次确认", "Confirm")
                                            }),
                                    )
                                    .child(
                                        div()
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(6.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                pal.danger, 0.16,
                                            ))
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.danger))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _e, _w, cx| {
                                                    this.pending_vault_action = Some(
                                                        SettingsVaultAction::DeleteCredential {
                                                            credential_id: cid_delete.clone(),
                                                        },
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "删除", "Delete")),
                                    ),
                            )
                    })),
            )
    }
}

impl Default for SettingsPrivacySecuritySections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsPrivacySecuritySections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.clone();
        let on_lbl = tr_str(&lang, "已开启", "On");
        let off_lbl = tr_str(&lang, "已关闭", "Off");
        let blocked_val = format!("{}", self.blocked_count);
        let sessions_val = format!("{}", self.active_sessions_count);
        let passcode_val = if self.passcode_enabled { on_lbl } else { off_lbl };
        let twofa_val = if self.two_factor_enabled { on_lbl } else { off_lbl };
        let global_ttl_val = self.global_ttl_label_localized(&lang);
        let ttl_desc = self.ttl_description_localized(&lang);
        let is_2fa_editing = self.two_factor_editing;
        let two_factor_enabled = self.two_factor_enabled;
        let masked_pw: String = "•".repeat(self.two_factor_password_draft.chars().count());

        let privacy_rows = [
            ("Phone Number", tr_str(&lang, "手机号码", "Phone Number"), self.phone_privacy.clone()),
            ("Last Seen & Online", tr_str(&lang, "最后上线状态", "Last Seen & Online"), self.last_seen_privacy.clone()),
            ("Profile Photos", tr_str(&lang, "个人头像", "Profile Photos"), self.photo_privacy.clone()),
            ("Calls", tr_str(&lang, "语音与视频通话", "Calls"), self.calls_privacy.clone()),
            ("Forwarded Messages", tr_str(&lang, "转发消息链接", "Forwarded Messages"), self.forwards_privacy.clone()),
            ("Groups & Channels", tr_str(&lang, "群组与频道邀请", "Groups & Channels"), self.groups_privacy.clone()),
        ];

        settings_container(pal)
            .child(
                settings_card(pal)
                    .child(settings_section_header(
                        IconName::Lock,
                        tr_str(&lang, "隐私与安全", "Privacy and Security"),
                        Some(tr_str(&lang, "管理登录设备、两步验证密码与隐私可见范围", "Manage active devices, 2FA, and privacy visibility")),
                        pal,
                    ))
                    .child(settings_action_row(
                        IconName::Close,
                        tr_str(&lang, "已屏蔽用户", "Blocked Users"),
                        &blocked_val,
                        pal,
                        false,
                        |this| {
                            this.pending_nav_section = Some(SettingsSectionType::BlockedPeers);
                        },
                        cx,
                    ))
                    .child(settings_action_row(
                        IconName::ComputerDesktop,
                        tr_str(&lang, "活跃设备与会话", "Active Sessions"),
                        &sessions_val,
                        pal,
                        false,
                        |this| {
                            this.pending_nav_section = Some(SettingsSectionType::ActiveSessions);
                        },
                        cx,
                    ))
                    .child(settings_action_row(
                        IconName::Lock,
                        tr_str(&lang, "本地密码锁", "Local Passcode"),
                        passcode_val,
                        pal,
                        false,
                        |this| {
                            this.pending_nav_section = Some(SettingsSectionType::LocalPasscode);
                        },
                        cx,
                    ))
                    .child(settings_action_row(
                        IconName::Verified,
                        tr_str(&lang, "两步验证密码", "Two-Step Verification"),
                        twofa_val,
                        pal,
                        false,
                        |this| {
                            this.two_factor_editing = !this.two_factor_editing;
                            if this.two_factor_editing {
                                this.recovery_key_editing = false;
                            }
                        },
                        cx,
                    ))
                    .when(is_2fa_editing, |el| {
                        el.child(
                            div()
                                .w_full()
                                .p(px(10.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(pal.accent))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_primary))
                                        .child(if masked_pw.is_empty() {
                                            tr_str(&lang, "直接输入云端两步验证密码…", "Type cloud 2FA password...").to_string()
                                        } else {
                                            format!("{}|", masked_pw)
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.0))
                                        .when(two_factor_enabled, |row| {
                                            row.child(
                                                div()
                                                    .px(px(12.0))
                                                    .py(px(5.0))
                                                    .rounded(px(6.0))
                                                    .bg(rgb(pal.danger))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(pal.active_button_fg))
                                                    .cursor_pointer()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _e, _w, cx| {
                                                            this.pending_2fa_action = Some(None);
                                                            this.two_factor_editing = false;
                                                            this.two_factor_password_draft.zeroize();
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(tr_str(&lang, "关闭两步验证", "Disable 2FA")),
                                            )
                                        })
                                        .child(
                                            div()
                                                .px(px(12.0))
                                                .py(px(5.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(pal.accent))
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.active_button_fg))
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _e, _w, cx| {
                                                        if !this.two_factor_password_draft.trim().is_empty() {
                                                            let pw = this.two_factor_password_draft.trim().to_string();
                                                            this.pending_2fa_action = Some(Some(pw));
                                                            this.two_factor_editing = false;
                                                            this.two_factor_password_draft.zeroize();
                                                            cx.notify();
                                                        }
                                                    }),
                                                )
                                                .child(tr_str(&lang, "保存两步验证密码", "Save 2FA Password")),
                                        ),
                                ),
                        )
                    })
                    .child(settings_action_row(
                        IconName::ClockSending,
                        tr_str(&lang, "全局自动删除消息", "Auto-Delete Messages"),
                        global_ttl_val,
                        pal,
                        false,
                        |this| {
                            this.pending_nav_section = Some(SettingsSectionType::GlobalTtl);
                        },
                        cx,
                    ))
                    .child(settings_action_row(
                        IconName::Lock,
                        tr_str(&lang, "通行密钥 (Passkeys)", "Passkeys"),
                        "›",
                        pal,
                        false,
                        |this| {
                            this.pending_nav_section = Some(SettingsSectionType::Passkeys);
                        },
                        cx,
                    ))
                    .child(settings_action_row(
                        IconName::BotRobot,
                        tr_str(&lang, "已授权网站", "Connected Websites"),
                        "›",
                        pal,
                        false,
                        |this| {
                            this.pending_nav_section = Some(SettingsSectionType::Websites);
                        },
                        cx,
                    )),
            )
            .child(self.render_vault_custody_card(pal, &lang, cx))
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(tr_str(&lang, "隐私权限", "Privacy"), pal))
                    .children(privacy_rows.into_iter().map(|(cat_key, label, val)| {
                        let cat = cat_key.to_string();
                        let val_loc = Self::localize_privacy_value(&val, &lang).to_string();
                        settings_action_row(
                            IconName::Lock,
                            label,
                            &val_loc,
                            pal,
                            false,
                            move |this| {
                                this.pending_edit_privacy_category = Some(cat.clone());
                            },
                            cx,
                        )
                    })),
            )
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(tr_str(&lang, "账号与内容控制", "Account & Content"), pal))
                    .child(settings_action_row(
                        IconName::ClockSending,
                        tr_str(&lang, "账号自毁期限", "If Away For"),
                        &ttl_desc,
                        pal,
                        false,
                        |this| {
                            let next = match this.ttl_months { 1 => 3, 3 => 6, 6 => 12, 12 => 18, 18 => 24, _ => 1 };
                            this.set_ttl_months(next);
                            this.pending_save_settings = true;
                        },
                        cx,
                    ))
                    .child(settings_toggle_row(
                        IconName::Close,
                        tr_str(&lang, "关闭敏感内容过滤", "Disable Filtering"),
                        None,
                        self.sensitive_content_enabled,
                        pal,
                        Self::toggle_sensitive_content,
                        cx,
                    )),
            )
            .into_any_element()
    }
}
