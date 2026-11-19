//! Four dark-minimalist GPUI Bot Assist Cards + zeroized local draft state.
//!
//! Strictly local to the desktop client (`MessageContent::LocalBotAssistCard`).
//! Plaintext credentials exist ONLY in-memory inside `ConversationStage::bot_assist_login_drafts`
//! wrapped in `Zeroizing<String>` and are zeroized immediately on submit, clear, or drop.

use std::sync::Arc;

use bot_browser::{
    AssistCardPayload, AssistCardStatus, ElementRect, LocalLoginSubmission, NormalizedClickPoint,
    ReusePolicy,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::components::ui::{delete_prev_word, pop_last_char, sanitize_paste_text};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::computer_workbench::render_ime_text_preview;
use crate::views::stage::cards_interactive::InteractiveCardAction;

const CARD_BG_DARK: u32 = 0x151922;
const CARD_BORDER_DARK: u32 = 0x2A3140;
const CARD_FIELD_BG_DARK: u32 = 0x0E1219;
const CARD_PILL_BG_DARK: u32 = 0x1E2532;
const CARD_TEXT_PRIMARY: u32 = 0xF1F5F9;
const CARD_TEXT_SECONDARY: u32 = 0x94A3B8;
const CARD_ACCENT: u32 = 0x3B82F6;
const CARD_SUCCESS: u32 = 0x10B981;
const CARD_AMBER: u32 = 0xF59E0B;
const CARD_DANGER: u32 = 0xEF4444;

/// Non-secret local card model embedded inside `MessageContent::LocalBotAssistCard`.
/// NEVER holds plaintext user credentials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotAssistCardModel {
    pub payload: AssistCardPayload,
}

impl BotAssistCardModel {
    pub fn new(payload: AssistCardPayload) -> Self {
        Self { payload }
    }

    pub fn from_payload(payload: AssistCardPayload) -> Self {
        Self { payload }
    }

    pub fn card_id(&self) -> &str {
        self.payload.card_id()
    }

    pub fn status(&self) -> AssistCardStatus {
        self.payload.status()
    }

    pub fn origin(&self) -> &str {
        match &self.payload {
            AssistCardPayload::VaultLogin { origin, .. }
            | AssistCardPayload::ReuseConfirm { origin, .. }
            | AssistCardPayload::QrLogin { origin, .. }
            | AssistCardPayload::ClickCaptcha { origin, .. } => origin,
        }
    }

    pub fn is_login_editable(&self) -> bool {
        matches!(
            self.payload,
            AssistCardPayload::VaultLogin {
                status: AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable,
                ..
            }
        )
    }

    pub fn apply_status_update(
        &mut self,
        new_status: AssistCardStatus,
        status_note: Option<String>,
        new_credential_id: Option<String>,
        new_account_hint: Option<String>,
        new_redirected_url: Option<String>,
    ) {
        self.payload.set_status(new_status, status_note);
        match &mut self.payload {
            AssistCardPayload::VaultLogin {
                saved_credential_id,
                saved_account_hint,
                ..
            } => {
                if let Some(cid) = new_credential_id {
                    *saved_credential_id = Some(cid);
                }
                if let Some(hint) = new_account_hint {
                    *saved_account_hint = Some(hint);
                }
            }
            AssistCardPayload::QrLogin { redirected_url, .. } => {
                if let Some(url) = new_redirected_url {
                    *redirected_url = Some(url);
                }
            }
            AssistCardPayload::ReuseConfirm { .. } | AssistCardPayload::ClickCaptcha { .. } => {}
        }
    }

    pub fn refresh_qr_image(&mut self, png: Vec<u8>, countdown: u32, attempt: u32) -> bool {
        if let AssistCardPayload::QrLogin {
            png_bytes,
            countdown_secs,
            poll_attempt,
            ..
        } = &mut self.payload
        {
            *png_bytes = png;
            *countdown_secs = countdown;
            *poll_attempt = attempt;
            true
        } else {
            false
        }
    }
}

impl From<AssistCardPayload> for BotAssistCardModel {
    fn from(payload: AssistCardPayload) -> Self {
        Self::new(payload)
    }
}

/// Active input field within Card 1 (`VaultLogin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BotAssistLoginField {
    Username,
    Password,
    TotpSecret,
}

impl BotAssistLoginField {
    pub fn next(self) -> Self {
        match self {
            Self::Username => Self::Password,
            Self::Password => Self::TotpSecret,
            Self::TotpSecret => Self::Username,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Username => Self::TotpSecret,
            Self::Password => Self::Username,
            Self::TotpSecret => Self::Password,
        }
    }
}

/// Ephemeral in-memory draft for Card 1 (`VaultLogin`).
/// Zeroized on submit, clear, or drop; `Debug` redacts all secret fields.
pub struct BotAssistLoginDraft {
    pub username: Zeroizing<String>,
    pub password: Zeroizing<String>,
    pub totp_secret: Zeroizing<String>,
    pub show_password: bool,
    pub allow_auto_reuse: bool,
    pub active_field: Option<BotAssistLoginField>,
    pub error_note: Option<String>,
}

impl Default for BotAssistLoginDraft {
    fn default() -> Self {
        Self::new(true)
    }
}

impl std::fmt::Debug for BotAssistLoginDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotAssistLoginDraft")
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("has_totp", &(!self.totp_secret.is_empty()))
            .field("show_password", &self.show_password)
            .field("allow_auto_reuse", &self.allow_auto_reuse)
            .field("active_field", &self.active_field)
            .field("error_note", &self.error_note)
            .finish()
    }
}

impl Drop for BotAssistLoginDraft {
    fn drop(&mut self) {
        self.zeroize_secrets();
    }
}

impl BotAssistLoginDraft {
    pub fn new(allow_auto_reuse: bool) -> Self {
        Self {
            username: Zeroizing::new(String::new()),
            password: Zeroizing::new(String::new()),
            totp_secret: Zeroizing::new(String::new()),
            show_password: false,
            allow_auto_reuse,
            active_field: None,
            error_note: None,
        }
    }

    pub fn for_payload(payload: &AssistCardPayload) -> Self {
        let allow_auto = match payload {
            AssistCardPayload::VaultLogin {
                suggested_reuse_policy,
                is_high_sensitivity,
                ..
            } => !*is_high_sensitivity && *suggested_reuse_policy == ReusePolicy::AutoReuse,
            _ => false,
        };
        Self::new(allow_auto)
    }

    pub fn zeroize_secrets(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
        self.totp_secret.zeroize();
    }

    pub fn clear_and_zeroize(&mut self) {
        self.zeroize_secrets();
        self.show_password = false;
        self.active_field = None;
        self.error_note = None;
    }

    pub fn field_text(&self, field: BotAssistLoginField) -> &str {
        match field {
            BotAssistLoginField::Username => &self.username,
            BotAssistLoginField::Password => &self.password,
            BotAssistLoginField::TotpSecret => &self.totp_secret,
        }
    }

    pub fn active_text(&self) -> &str {
        match self.active_field {
            Some(field) => self.field_text(field),
            None => "",
        }
    }

    fn active_field_mut(&mut self) -> Option<&mut Zeroizing<String>> {
        match self.active_field {
            Some(BotAssistLoginField::Username) => Some(&mut self.username),
            Some(BotAssistLoginField::Password) => Some(&mut self.password),
            Some(BotAssistLoginField::TotpSecret) => Some(&mut self.totp_secret),
            None => None,
        }
    }

    /// Inserts sanitized committed/pasted text (supporting UTF-8 / CJK) into the active field.
    pub fn insert_text(&mut self, raw_text: &str) -> bool {
        let clean = Zeroizing::new(sanitize_paste_text(raw_text, false));
        if clean.is_empty() {
            return false;
        }
        let Some(target) = self.active_field_mut() else {
            return false;
        };
        target.push_str(&clean);
        self.error_note = None;
        true
    }

    /// Deletes the trailing Unicode scalar character (or previous word if `delete_word`) in the active field.
    pub fn backspace(&mut self, delete_word: bool) -> bool {
        let Some(target) = self.active_field_mut() else {
            return false;
        };
        let changed = if delete_word {
            delete_prev_word(target)
        } else {
            pop_last_char(target)
        };
        if changed {
            self.error_note = None;
        }
        changed
    }

    /// Zeroizes and clears only the currently focused field.
    pub fn clear_active_field(&mut self) -> bool {
        let Some(target) = self.active_field_mut() else {
            return false;
        };
        target.zeroize();
        self.error_note = None;
        true
    }

    /// Always returns `"•"` bullet masking matching password char length.
    pub fn masked_password(&self) -> String {
        mask_secret_bullets(&self.password)
    }

    /// Returns plaintext password when `show_password` is toggled on, otherwise `"•"` bullets.
    pub fn password_display(&self) -> String {
        if self.show_password {
            self.password.to_string()
        } else {
            self.masked_password()
        }
    }

    /// Cycles focus across `Username -> Password -> TotpSecret` (or reverse when `reverse == true`).
    pub fn cycle_field(&mut self, reverse: bool) -> BotAssistLoginField {
        let next = match (self.active_field, reverse) {
            (None, false) => BotAssistLoginField::Username,
            (None, true) => BotAssistLoginField::TotpSecret,
            (Some(cur), false) => cur.next(),
            (Some(cur), true) => cur.prev(),
        };
        self.active_field = Some(next);
        next
    }

    /// Extracts a `LocalLoginSubmission` while zeroizing all sensitive buffers in `self`.
    pub fn take_submission(
        &mut self,
        card_id: impl Into<String>,
    ) -> Result<LocalLoginSubmission, String> {
        let trimmed_user = self.username.trim();
        if trimmed_user.is_empty() {
            let msg = "请输入账号 / 用户名".to_string();
            self.error_note = Some(msg.clone());
            return Err(msg);
        }
        if self.password.is_empty() {
            let msg = "请输入登录密码".to_string();
            self.error_note = Some(msg.clone());
            return Err(msg);
        }

        let username = if trimmed_user.len() == self.username.len() {
            std::mem::take(&mut self.username)
        } else {
            let u = Zeroizing::new(trimmed_user.to_string());
            self.username.zeroize();
            u
        };
        let password = std::mem::take(&mut self.password);
        let totp_trimmed = self.totp_secret.trim();
        let totp_secret = if totp_trimmed.is_empty() {
            self.totp_secret.zeroize();
            None
        } else if totp_trimmed.len() == self.totp_secret.len() {
            Some(std::mem::take(&mut self.totp_secret))
        } else {
            let t = Zeroizing::new(totp_trimmed.to_string());
            self.totp_secret.zeroize();
            Some(t)
        };

        self.zeroize_secrets();
        self.show_password = false;
        self.active_field = None;
        self.error_note = None;

        Ok(LocalLoginSubmission {
            card_id: card_id.into(),
            username,
            password,
            totp_secret,
            account_hint: None,
            allow_auto_reuse: self.allow_auto_reuse,
        })
    }
}

/// Masks a string with `"•"` bullets preserving Unicode character count.
pub fn mask_secret_bullets(raw: &str) -> String {
    "•".repeat(raw.chars().count())
}

/// Ephemeral click coordinate draft for Card 4 (`ClickCaptcha`).
#[derive(Debug, Clone, Default)]
pub struct BotAssistClickDraft {
    pub points: Vec<NormalizedClickPoint>,
    pub image_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
}

impl BotAssistClickDraft {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_point(&mut self, point: NormalizedClickPoint, max_clicks: u8) {
        let clamped = point.clamped();
        let cap = usize::from(max_clicks.max(1));
        if cap == 1 {
            self.points.clear();
            self.points.push(clamped);
        } else if self.points.len() < cap {
            self.points.push(clamped);
        }
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }
}

/// Converts a window-space pixel click position against `bounds` into normalized `[0.0, 1.0]` coordinates.
pub fn pixel_to_normalized_click_point(
    click_pos: Point<Pixels>,
    bounds: Bounds<Pixels>,
) -> NormalizedClickPoint {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return NormalizedClickPoint::new(0.0, 0.0);
    }
    let nx = (f32::from(click_pos.x - bounds.origin.x) / w) as f64;
    let ny = (f32::from(click_pos.y - bounds.origin.y) / h) as f64;
    NormalizedClickPoint::new(nx, ny)
}

/// Computes deterministic display `(width, height)` in pixels for an `ElementRect` inside a card.
pub fn compute_captcha_display_size(rect: &ElementRect) -> (f32, f32) {
    let max_w = 296.0_f32;
    let raw_w = if rect.width.is_finite() && rect.width > 1.0 {
        rect.width as f32
    } else {
        296.0
    };
    let raw_h = if rect.height.is_finite() && rect.height > 1.0 {
        rect.height as f32
    } else {
        160.0
    };
    let aspect = (raw_w / raw_h).clamp(0.4, 3.2);
    let disp_w = raw_w.clamp(180.0, max_w);
    let disp_h = (disp_w / aspect).clamp(96.0, 220.0);
    (disp_w, disp_h)
}

/// User actions emitted by any of the 4 Bot Assist GPUI cards.
#[derive(Debug, Clone)]
pub enum BotAssistCardAction {
    FocusLoginField {
        card_id: String,
        field: BotAssistLoginField,
    },
    ToggleShowPassword {
        card_id: String,
    },
    ToggleAutoReuse {
        card_id: String,
    },
    SubmitLogin {
        card_id: String,
    },
    ApproveReuse {
        card_id: String,
        approved: bool,
    },
    CancelCard {
        card_id: String,
    },
    ClickCaptchaPoint {
        card_id: String,
        point: NormalizedClickPoint,
    },
    ResetCaptchaClicks {
        card_id: String,
    },
    SubmitCaptchaClicks {
        card_id: String,
    },
}

pub fn render_bot_assist_card<V: 'static>(
    card: &BotAssistCardModel,
    login_draft: Option<&BotAssistLoginDraft>,
    active_login_field: Option<BotAssistLoginField>,
    ime_preedit: Option<&str>,
    click_draft: Option<&BotAssistClickDraft>,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, InteractiveCardAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    match &card.payload {
        AssistCardPayload::VaultLogin {
            card_id,
            origin,
            site_title,
            suggested_reuse_policy,
            is_high_sensitivity,
            status,
            status_note,
            saved_account_hint,
            ..
        } => render_vault_login_card(
            card_id,
            origin,
            site_title,
            *suggested_reuse_policy,
            *is_high_sensitivity,
            *status,
            status_note.as_deref(),
            saved_account_hint.as_deref(),
            login_draft,
            active_login_field,
            ime_preedit,
            palette,
            cx,
            on_action,
        ),
        AssistCardPayload::ReuseConfirm {
            card_id,
            origin,
            account_hint,
            has_totp,
            require_biometric_hint,
            status,
            status_note,
            ..
        } => render_reuse_confirm_card(
            card_id,
            origin,
            account_hint,
            *has_totp,
            *require_biometric_hint,
            *status,
            status_note.as_deref(),
            cx,
            on_action,
        ),
        AssistCardPayload::QrLogin {
            card_id,
            origin,
            png_bytes,
            countdown_secs,
            refresh_interval_secs,
            poll_attempt,
            status,
            status_note,
            redirected_url,
            ..
        } => render_qr_login_card(
            card_id,
            origin,
            png_bytes,
            *countdown_secs,
            *refresh_interval_secs,
            *poll_attempt,
            *status,
            status_note.as_deref(),
            redirected_url.as_deref(),
            cx,
            on_action,
        ),
        AssistCardPayload::ClickCaptcha {
            card_id,
            origin,
            png_bytes,
            element_rect,
            instruction,
            max_clicks,
            status,
            status_note,
            ..
        } => render_click_captcha_card(
            card_id,
            origin,
            png_bytes,
            element_rect,
            instruction,
            *max_clicks,
            *status,
            status_note.as_deref(),
            click_draft,
            cx,
            on_action,
        ),
    }
}

fn card_shell_container(width_px: f32) -> Div {
    div()
        .w(px(width_px))
        .max_w(px(360.0))
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(rgb(CARD_BG_DARK))
        .border_1()
        .border_color(rgb(CARD_BORDER_DARK))
        .flex()
        .flex_col()
        .gap(px(9.0))
}

fn render_card_header(
    badge_text: &str,
    title_text: &str,
    origin: &str,
    badge_color: u32,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .w(px(20.0))
                                .h(px(20.0))
                                .rounded(px(6.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(badge_color, 0.16))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(tg_icon(IconName::Lock, 11.0, badge_color)),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(CARD_TEXT_PRIMARY))
                                .child(title_text.to_string()),
                        ),
                )
                .child(
                    div()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(999.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(badge_color, 0.14))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(badge_color, 0.32))
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(badge_color))
                        .child(badge_text.to_string()),
                ),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .bg(rgb(CARD_PILL_BG_DARK))
                .border_1()
                .border_color(rgb(CARD_BORDER_DARK))
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xCBD5E1))
                .truncate()
                .child(origin.to_string()),
        )
        .into_any_element()
}

fn render_collapsed_status_badge(
    ok: bool,
    headline: &str,
    subline: Option<String>,
) -> AnyElement {
    let tone = if ok { CARD_SUCCESS } else { CARD_TEXT_SECONDARY };
    div()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(tone, 0.12))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(tone, 0.35))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(tone))
                .child(headline.to_string()),
        )
        .when_some(subline, |el, sub| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(CARD_TEXT_SECONDARY))
                    .truncate()
                    .child(sub),
            )
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_vault_login_card<V: 'static>(
    card_id: &str,
    origin: &str,
    site_title: &str,
    suggested_reuse_policy: ReusePolicy,
    is_high_sensitivity: bool,
    status: AssistCardStatus,
    status_note: Option<&str>,
    saved_account_hint: Option<&str>,
    login_draft: Option<&BotAssistLoginDraft>,
    active_login_field: Option<BotAssistLoginField>,
    ime_preedit: Option<&str>,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, InteractiveCardAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let header_title = if site_title.trim().is_empty() {
        "安全登录凭据".to_string()
    } else {
        format!("安全登录 · {}", site_title.trim())
    };
    let mut root = card_shell_container(328.0).child(render_card_header(
        "LOCAL VAULT",
        &header_title,
        origin,
        if is_high_sensitivity {
            CARD_AMBER
        } else {
            CARD_ACCENT
        },
    ));

    match status {
        AssistCardStatus::SavedAndLoggedIn | AssistCardStatus::Completed => {
            let hint_line = saved_account_hint
                .filter(|s| !s.trim().is_empty())
                .map(|h| format!("已绑定账号：{}", h));
            return root
                .child(render_collapsed_status_badge(
                    true,
                    "✓ 安全存储 · 登录信息已保存 / 已登录",
                    hint_line,
                ))
                .into_any_element();
        }
        AssistCardStatus::Cancelled | AssistCardStatus::Expired | AssistCardStatus::Denied => {
            let label = match status {
                AssistCardStatus::Expired => "登录卡片已过期",
                AssistCardStatus::Denied => "登录请求已拒绝",
                _ => "已取消本地凭据录入",
            };
            return root
                .child(render_collapsed_status_badge(
                    false,
                    label,
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        AssistCardStatus::ApprovedAndFilling => {
            return root
                .child(render_collapsed_status_badge(
                    true,
                    "✓ 正在本地安全填充并登录...",
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable => {}
    }

    let default_auto_reuse = !is_high_sensitivity && suggested_reuse_policy == ReusePolicy::AutoReuse;
    let allow_auto_reuse = login_draft
        .map(|d| d.allow_auto_reuse)
        .unwrap_or(default_auto_reuse);
    let show_password = login_draft.is_some_and(|d| d.show_password);
    let username_val = login_draft.map(|d| d.username.as_str()).unwrap_or("");
    let password_val = login_draft.map(|d| d.password.as_str()).unwrap_or("");
    let totp_val = login_draft.map(|d| d.totp_secret.as_str()).unwrap_or("");
    let err_note = login_draft.and_then(|d| d.error_note.as_deref());

    let mut preview_pal = *palette;
    preview_pal.accent = CARD_ACCENT;
    preview_pal.text_primary = CARD_TEXT_PRIMARY;
    preview_pal.text_secondary = CARD_TEXT_SECONDARY;

    let cid_u = card_id.to_string();
    let cid_p = card_id.to_string();
    let cid_eye = card_id.to_string();
    let cid_t = card_id.to_string();
    let cid_reuse = card_id.to_string();
    let cid_submit = card_id.to_string();
    let cid_cancel = card_id.to_string();

    let act_u = on_action.clone();
    let act_p = on_action.clone();
    let act_eye = on_action.clone();
    let act_t = on_action.clone();
    let act_reuse = on_action.clone();
    let act_submit = on_action.clone();
    let act_cancel = on_action;

    let user_focused = active_login_field == Some(BotAssistLoginField::Username);
    let pass_focused = active_login_field == Some(BotAssistLoginField::Password);
    let totp_focused = active_login_field == Some(BotAssistLoginField::TotpSecret);

    root = root
        .child(render_login_input_box(
            format!("bot-vault-user-{}", card_id),
            "账号 / 用户名",
            username_val,
            if user_focused { ime_preedit } else { None },
            "请输入账号、邮箱或手机号",
            user_focused,
            false,
            None,
            preview_pal,
            cx,
            move |this, cx| {
                act_u(
                    this,
                    InteractiveCardAction::BotAssist(BotAssistCardAction::FocusLoginField {
                        card_id: cid_u.clone(),
                        field: BotAssistLoginField::Username,
                    }),
                    cx,
                );
            },
        ))
        .child(render_login_input_box(
            format!("bot-vault-pass-{}", card_id),
            "密码",
            password_val,
            if pass_focused { ime_preedit } else { None },
            "仅保存在本地加密保险库",
            pass_focused,
            !show_password,
            Some((
                format!("bot-vault-eye-{}", card_id),
                if show_password { "隐藏" } else { "显示" },
                Box::new(move |this, cx| {
                    act_eye(
                        this,
                        InteractiveCardAction::BotAssist(BotAssistCardAction::ToggleShowPassword {
                            card_id: cid_eye.clone(),
                        }),
                        cx,
                    );
                }),
            )),
            preview_pal,
            cx,
            move |this, cx| {
                act_p(
                    this,
                    InteractiveCardAction::BotAssist(BotAssistCardAction::FocusLoginField {
                        card_id: cid_p.clone(),
                        field: BotAssistLoginField::Password,
                    }),
                    cx,
                );
            },
        ))
        .child(render_login_input_box(
            format!("bot-vault-totp-{}", card_id),
            "2FA TOTP 密钥（可选）",
            totp_val,
            if totp_focused { ime_preedit } else { None },
            "Base32 密钥或 otpauth:// URI",
            totp_focused,
            false,
            None,
            preview_pal,
            cx,
            move |this, cx| {
                act_t(
                    this,
                    InteractiveCardAction::BotAssist(BotAssistCardAction::FocusLoginField {
                        card_id: cid_t.clone(),
                        field: BotAssistLoginField::TotpSecret,
                    }),
                    cx,
                );
            },
        ))
        .child(
            div()
                .id(SharedString::from(format!("bot-vault-reuse-{}", card_id)))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(rgb(CARD_FIELD_BG_DARK))
                .border_1()
                .border_color(rgb(if allow_auto_reuse && is_high_sensitivity {
                    CARD_AMBER
                } else {
                    CARD_BORDER_DARK
                }))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        act_reuse(
                            this,
                            InteractiveCardAction::BotAssist(BotAssistCardAction::ToggleAutoReuse {
                                card_id: cid_reuse.clone(),
                            }),
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            div()
                                .w(px(15.0))
                                .h(px(15.0))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(rgb(if allow_auto_reuse {
                                    CARD_ACCENT
                                } else {
                                    CARD_TEXT_SECONDARY
                                }))
                                .when(allow_auto_reuse, |el| {
                                    el.bg(rgb(CARD_ACCENT))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(tg_icon(IconName::SelectCheck, 9.0, 0xFFFFFF))
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(CARD_TEXT_PRIMARY))
                                .child("允许 Bot 自动复用"),
                        ),
                )
                .when(is_high_sensitivity, |el| {
                    el.child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(CARD_AMBER))
                            .child("⚠ 支付/云控制台高敏域名：建议每次弹窗确认"),
                    )
                }),
        );

    if let Some(err) = err_note {
        root = root.child(
            div()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(CARD_DANGER, 0.14))
                .border_1()
                .border_color(TdesktopThemePalette::rgba_with_alpha(CARD_DANGER, 0.45))
                .text_size(px(11.0))
                .text_color(rgb(CARD_DANGER))
                .child(err.to_string()),
        );
    }

    if status == AssistCardStatus::TimedOutResumable {
        let timeout_msg = status_note
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("上轮已超时，补填后将自动保存并续跑");
        root = root.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(CARD_AMBER, 0.14))
                .border_1()
                .border_color(TdesktopThemePalette::rgba_with_alpha(CARD_AMBER, 0.45))
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(CARD_AMBER))
                .child(timeout_msg.to_string()),
        );
    }

    root.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id(SharedString::from(format!("bot-vault-submit-{}", card_id)))
                    .flex_1()
                    .h(px(34.0))
                    .rounded(px(8.0))
                    .bg(rgb(CARD_ACCENT))
                    .hover(|s| s.opacity(0.9))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            act_submit(
                                this,
                                InteractiveCardAction::BotAssist(BotAssistCardAction::SubmitLogin {
                                    card_id: cid_submit.clone(),
                                }),
                                cx,
                            );
                        }),
                    )
                    .child("安全保存并登录"),
            )
            .child(
                div()
                    .id(SharedString::from(format!("bot-vault-cancel-{}", card_id)))
                    .px(px(11.0))
                    .h(px(34.0))
                    .rounded(px(8.0))
                    .bg(rgb(CARD_PILL_BG_DARK))
                    .border_1()
                    .border_color(rgb(CARD_BORDER_DARK))
                    .hover(|s| s.opacity(0.85))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(CARD_TEXT_SECONDARY))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            act_cancel(
                                this,
                                InteractiveCardAction::BotAssist(BotAssistCardAction::CancelCard {
                                    card_id: cid_cancel.clone(),
                                }),
                                cx,
                            );
                        }),
                    )
                    .child("取消"),
            ),
    )
    .into_any_element()
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn render_login_input_box<V: 'static>(
    id: String,
    label: &str,
    committed: &str,
    preedit: Option<&str>,
    placeholder: &str,
    focused: bool,
    masked: bool,
    trailing_toggle: Option<(String, &'static str, Box<dyn Fn(&mut V, &mut Context<V>) + 'static>)>,
    preview_pal: TdesktopThemePalette,
    cx: &mut Context<V>,
    on_focus: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(CARD_TEXT_SECONDARY))
                .child(label.to_string()),
        )
        .child(
            div()
                .id(SharedString::from(id))
                .cursor_text()
                .min_h(px(34.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(rgb(CARD_FIELD_BG_DARK))
                .border_1()
                .border_color(rgb(if focused {
                    CARD_ACCENT
                } else {
                    CARD_BORDER_DARK
                }))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_focus(this, cx);
                    }),
                )
                .child(div().flex_1().min_w(px(0.0)).child(render_ime_text_preview(
                    committed,
                    preedit,
                    placeholder,
                    focused,
                    masked,
                    CARD_TEXT_PRIMARY,
                    preview_pal,
                )))
                .when_some(trailing_toggle, |el, (btn_id, btn_label, on_toggle)| {
                    el.child(
                        div()
                            .id(SharedString::from(btn_id))
                            .cursor_pointer()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .bg(rgb(CARD_PILL_BG_DARK))
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(CARD_TEXT_SECONDARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    on_toggle(this, cx);
                                }),
                            )
                            .child(btn_label),
                    )
                }),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_reuse_confirm_card<V: 'static>(
    card_id: &str,
    origin: &str,
    account_hint: &str,
    has_totp: bool,
    require_biometric_hint: bool,
    status: AssistCardStatus,
    status_note: Option<&str>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, InteractiveCardAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let mut root = card_shell_container(318.0).child(render_card_header(
        "REUSE AUTH",
        "快捷复用确认",
        origin,
        CARD_AMBER,
    ));

    match status {
        AssistCardStatus::Completed | AssistCardStatus::SavedAndLoggedIn => {
            return root
                .child(render_collapsed_status_badge(
                    true,
                    "✓ 已授权本次填充登录",
                    Some(format!("账号：{}", account_hint)),
                ))
                .into_any_element();
        }
        AssistCardStatus::ApprovedAndFilling => {
            return root
                .child(render_collapsed_status_badge(
                    true,
                    "✓ 正在安全填充凭据...",
                    Some(format!("账号：{}", account_hint)),
                ))
                .into_any_element();
        }
        AssistCardStatus::Denied => {
            return root
                .child(render_collapsed_status_badge(
                    false,
                    "已拒绝本次登录复用",
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        AssistCardStatus::Expired | AssistCardStatus::Cancelled => {
            return root
                .child(render_collapsed_status_badge(
                    false,
                    if status == AssistCardStatus::Expired {
                        "授权请求已过期"
                    } else {
                        "已取消复用确认"
                    },
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable => {}
    }

    let cid_ok = card_id.to_string();
    let cid_no = card_id.to_string();
    let act_ok = on_action.clone();
    let act_no = on_action;

    root = root.child(
        div()
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(rgb(CARD_FIELD_BG_DARK))
            .border_1()
            .border_color(rgb(CARD_BORDER_DARK))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(CARD_TEXT_PRIMARY))
                            .child(account_hint.to_string()),
                    )
                    .when(has_totp, |el| {
                        el.child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(999.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(CARD_SUCCESS, 0.16))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(CARD_SUCCESS))
                                .child("2FA 自动算码"),
                        )
                    }),
            )
            .when(require_biometric_hint, |el| {
                el.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(CARD_AMBER))
                        .child("🔒 高敏站点需确认 · Touch ID / 本地安全鉴权"),
                )
            }),
    );

    if status == AssistCardStatus::TimedOutResumable {
        let note = status_note
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("上轮已超时，确认后将自动填充并续跑");
        root = root.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(CARD_AMBER, 0.14))
                .border_1()
                .border_color(TdesktopThemePalette::rgba_with_alpha(CARD_AMBER, 0.45))
                .text_size(px(11.0))
                .text_color(rgb(CARD_AMBER))
                .child(note.to_string()),
        );
    }

    root.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id(SharedString::from(format!("bot-reuse-allow-{}", card_id)))
                    .flex_1()
                    .h(px(34.0))
                    .rounded(px(8.0))
                    .bg(rgb(CARD_SUCCESS))
                    .hover(|s| s.opacity(0.9))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            act_ok(
                                this,
                                InteractiveCardAction::BotAssist(BotAssistCardAction::ApproveReuse {
                                    card_id: cid_ok.clone(),
                                    approved: true,
                                }),
                                cx,
                            );
                        }),
                    )
                    .child("允许本次登录"),
            )
            .child(
                div()
                    .id(SharedString::from(format!("bot-reuse-deny-{}", card_id)))
                    .px(px(14.0))
                    .h(px(34.0))
                    .rounded(px(8.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(CARD_DANGER, 0.16))
                    .border_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(CARD_DANGER, 0.45))
                    .hover(|s| s.opacity(0.88))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(CARD_DANGER))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            act_no(
                                this,
                                InteractiveCardAction::BotAssist(BotAssistCardAction::ApproveReuse {
                                    card_id: cid_no.clone(),
                                    approved: false,
                                }),
                                cx,
                            );
                        }),
                    )
                    .child("拒绝"),
            ),
    )
    .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_qr_login_card<V: 'static>(
    card_id: &str,
    origin: &str,
    png_bytes: &[u8],
    countdown_secs: u32,
    refresh_interval_secs: u32,
    poll_attempt: u32,
    status: AssistCardStatus,
    status_note: Option<&str>,
    redirected_url: Option<&str>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, InteractiveCardAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let root = card_shell_container(300.0).child(render_card_header(
        &format!("{}s", countdown_secs),
        "二维码动态轮询",
        origin,
        CARD_ACCENT,
    ));

    match status {
        AssistCardStatus::Completed | AssistCardStatus::SavedAndLoggedIn => {
            return root
                .child(render_collapsed_status_badge(
                    true,
                    "✓ 扫码登录成功，已唤醒 Bot",
                    redirected_url.map(str::to_string),
                ))
                .into_any_element();
        }
        AssistCardStatus::Expired | AssistCardStatus::Cancelled | AssistCardStatus::Denied => {
            return root
                .child(render_collapsed_status_badge(
                    false,
                    if status == AssistCardStatus::Expired {
                        "二维码轮询已超时"
                    } else {
                        "已取消扫码登录"
                    },
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        _ => {}
    }

    let qr_img = (!png_bytes.is_empty()).then(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png_bytes.to_vec(),
        ))
    });
    let cid_cancel = card_id.to_string();
    let act_cancel = on_action;
    let refresh_note = if refresh_interval_secs <= 3 {
        "每 3 秒静默刷新防过期".to_string()
    } else {
        format!("每 {} 秒静默刷新防过期", refresh_interval_secs)
    };

    root.child(
        div()
            .w_full()
            .py(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(176.0))
                    .h(px(176.0))
                    .p(px(8.0))
                    .rounded(px(10.0))
                    .bg(rgb(0xFFFFFF))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(qr_img, |el, src| {
                        el.child(img(src).size_full().object_fit(ObjectFit::Contain))
                    })
                    .when(png_bytes.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(0x475569))
                                .child("等待二维码图像..."),
                        )
                    }),
            ),
    )
    .child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(CARD_TEXT_SECONDARY))
                    .child(if poll_attempt > 0 {
                        format!("{} · #{}", refresh_note, poll_attempt)
                    } else {
                        refresh_note
                    }),
            )
            .child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(rgb(CARD_PILL_BG_DARK))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(CARD_AMBER))
                    .child(format!("{}s", countdown_secs)),
            ),
    )
    .child(
        div()
            .id(SharedString::from(format!("bot-qr-cancel-{}", card_id)))
            .w_full()
            .h(px(32.0))
            .rounded(px(8.0))
            .bg(rgb(CARD_PILL_BG_DARK))
            .border_1()
            .border_color(rgb(CARD_BORDER_DARK))
            .hover(|s| s.opacity(0.85))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(CARD_TEXT_SECONDARY))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    act_cancel(
                        this,
                        InteractiveCardAction::BotAssist(BotAssistCardAction::CancelCard {
                            card_id: cid_cancel.clone(),
                        }),
                        cx,
                    );
                }),
            )
            .child("取消"),
    )
    .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_click_captcha_card<V: 'static>(
    card_id: &str,
    origin: &str,
    png_bytes: &[u8],
    element_rect: &ElementRect,
    instruction: &str,
    max_clicks: u8,
    status: AssistCardStatus,
    status_note: Option<&str>,
    click_draft: Option<&BotAssistClickDraft>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, InteractiveCardAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let root = card_shell_container(324.0).child(render_card_header(
        "CLICK CAPTCHA",
        "静态点选验证",
        origin,
        CARD_ACCENT,
    ));

    match status {
        AssistCardStatus::Completed | AssistCardStatus::SavedAndLoggedIn => {
            return root
                .child(render_collapsed_status_badge(
                    true,
                    "✓ 验证点击已提交，已继续任务",
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        AssistCardStatus::Expired | AssistCardStatus::Cancelled | AssistCardStatus::Denied => {
            return root
                .child(render_collapsed_status_badge(
                    false,
                    if status == AssistCardStatus::Expired {
                        "点选验证已过期"
                    } else {
                        "已取消点选验证"
                    },
                    status_note.map(str::to_string),
                ))
                .into_any_element();
        }
        _ => {}
    }

    let (disp_w, disp_h) = compute_captcha_display_size(element_rect);
    let cap_img = (!png_bytes.is_empty()).then(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png_bytes.to_vec(),
        ))
    });
    let points: Vec<NormalizedClickPoint> =
        click_draft.map(|d| d.points.clone()).unwrap_or_default();
    let bounds_arc = click_draft
        .map(|d| d.image_bounds.clone())
        .unwrap_or_else(|| Arc::new(Mutex::new(None)));
    let bounds_for_canvas = bounds_arc.clone();
    let bounds_for_click = bounds_arc;

    let cid_click = card_id.to_string();
    let cid_reset = card_id.to_string();
    let cid_submit = card_id.to_string();
    let act_click = on_action.clone();
    let act_reset = on_action.clone();
    let act_submit = on_action;

    let mut image_box = div()
        .id(SharedString::from(format!("bot-captcha-img-{}", card_id)))
        .relative()
        .w(px(disp_w))
        .h(px(disp_h))
        .rounded(px(8.0))
        .overflow_hidden()
        .bg(rgb(CARD_FIELD_BG_DARK))
        .border_1()
        .border_color(rgb(CARD_BORDER_DARK))
        .cursor_crosshair()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let measured = *bounds_for_click.lock();
                let target_bounds = measured
                    .filter(|b| f32::from(b.size.width) > 1.0 && f32::from(b.size.height) > 1.0)
                    .unwrap_or_else(|| {
                        Bounds::new(Point::default(), size(px(disp_w), px(disp_h)))
                    });
                let pt = pixel_to_normalized_click_point(ev.position, target_bounds);
                act_click(
                    this,
                    InteractiveCardAction::BotAssist(BotAssistCardAction::ClickCaptchaPoint {
                        card_id: cid_click.clone(),
                        point: pt,
                    }),
                    cx,
                );
            }),
        )
        .child(
            canvas(
                move |bounds, _window, _cx| {
                    *bounds_for_canvas.lock() = Some(bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
        .when_some(cap_img, |el, src| {
            el.child(img(src).size_full().object_fit(ObjectFit::Fill))
        })
        .when(png_bytes.is_empty(), |el| {
            el.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .text_color(rgb(CARD_TEXT_SECONDARY))
                    .child("暂无验证截图"),
            )
        });

    for (idx, pt) in points.iter().enumerate() {
        let pin_x = (pt.norm_x as f32 * disp_w - 10.0).clamp(0.0, (disp_w - 20.0).max(0.0));
        let pin_y = (pt.norm_y as f32 * disp_h - 10.0).clamp(0.0, (disp_h - 20.0).max(0.0));
        image_box = image_box.child(
            div()
                .absolute()
                .left(px(pin_x))
                .top(px(pin_y))
                .w(px(20.0))
                .h(px(20.0))
                .rounded_full()
                .bg(rgb(CARD_ACCENT))
                .border_2()
                .border_color(rgb(0xFFFFFF))
                .shadow_sm()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(format!("{}", idx + 1)),
        );
    }

    let click_count = points.len();
    let max_label = max_clicks.max(1);

    root.when(!instruction.trim().is_empty(), |el| {
        el.child(
            div()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(rgb(CARD_FIELD_BG_DARK))
                .text_size(px(11.5))
                .text_color(rgb(CARD_TEXT_PRIMARY))
                .child(instruction.to_string()),
        )
    })
    .child(div().w_full().flex().justify_center().child(image_box))
    .child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(10.5))
            .text_color(rgb(CARD_TEXT_SECONDARY))
            .child(format!("点击顺序：{} / {}", click_count, max_label))
            .child("按顺序点击后提交"),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id(SharedString::from(format!("bot-captcha-reset-{}", card_id)))
                    .px(px(14.0))
                    .h(px(32.0))
                    .rounded(px(8.0))
                    .bg(rgb(CARD_PILL_BG_DARK))
                    .border_1()
                    .border_color(rgb(CARD_BORDER_DARK))
                    .hover(|s| s.opacity(0.85))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(CARD_TEXT_SECONDARY))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            act_reset(
                                this,
                                InteractiveCardAction::BotAssist(
                                    BotAssistCardAction::ResetCaptchaClicks {
                                        card_id: cid_reset.clone(),
                                    },
                                ),
                                cx,
                            );
                        }),
                    )
                    .child("重置"),
            )
            .child(
                div()
                    .id(SharedString::from(format!("bot-captcha-submit-{}", card_id)))
                    .flex_1()
                    .h(px(32.0))
                    .rounded(px(8.0))
                    .bg(rgb(if click_count > 0 {
                        CARD_ACCENT
                    } else {
                        CARD_PILL_BG_DARK
                    }))
                    .when(click_count > 0, |el| el.hover(|s| s.opacity(0.9)).cursor_pointer())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(if click_count > 0 {
                        0xFFFFFF
                    } else {
                        CARD_TEXT_SECONDARY
                    }))
                    .when(click_count > 0, |el| {
                        el.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                act_submit(
                                    this,
                                    InteractiveCardAction::BotAssist(
                                        BotAssistCardAction::SubmitCaptchaClicks {
                                            card_id: cid_submit.clone(),
                                        },
                                    ),
                                    cx,
                                );
                            }),
                        )
                    })
                    .child("提交验证"),
            ),
    )
    .into_any_element()
}
