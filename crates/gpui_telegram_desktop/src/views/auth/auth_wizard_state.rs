//! State types, navigation enums, emitted events, and validation helpers for `AuthWizardView`.

use bot_browser::e2ee_sync::{Argon2KdfParams, SplitKeyVaultKek};
use gpui_telegram_client::AuthResponse;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Strictly in-memory vault bootstrap secrets passed from `AuthWizardView` to `AppShell`
/// upon successful sign-in or registration. Zeroized on consume/drop; never written to disk or logs.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub enum PendingAuthVaultBootstrap {
    SplitKeyV2 {
        split_kek: SplitKeyVaultKek,
        #[zeroize(skip)]
        auth_salt_b64: String,
        #[zeroize(skip)]
        kdf_params: Argon2KdfParams,
        legacy_upgrade_password: Option<Zeroizing<String>>,
        #[zeroize(skip)]
        is_new_registration: bool,
    },
    PasswordlessDeviceOnly,
}

impl std::fmt::Debug for PendingAuthVaultBootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SplitKeyV2 {
                auth_salt_b64,
                kdf_params,
                legacy_upgrade_password,
                is_new_registration,
                ..
            } => f
                .debug_struct("SplitKeyV2")
                .field("split_kek", &"[REDACTED]")
                .field("auth_salt_b64", auth_salt_b64)
                .field("kdf_params", kdf_params)
                .field("has_legacy_upgrade", &legacy_upgrade_password.is_some())
                .field("is_new_registration", is_new_registration)
                .finish(),
            Self::PasswordlessDeviceOnly => f.write_str("PasswordlessDeviceOnly"),
        }
    }
}

/// High-level steps of the unauthenticated Telegram Desktop onboarding & login wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginWizardStep {
    #[default]
    Start,
    Phone,
    QrCode,
    Code,
}

/// Multi-tab navigation modes for the Login Wizard card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthWizardTab {
    #[default]
    PasswordLogin,
    AlipayQr,
    EmailRegister,
}

/// Focusable text input fields inside the Auth Wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginInputField {
    Username,
    Password,
    FirstName,
    Email,
    EmailCode,
}

/// Events emitted by `AuthWizardView` for `AppShell` to coordinate session, theme, locale, and toasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthWizardEvent {
    Authenticated(AuthResponse),
    ServerUrlApplied(String),
    Toast { message: String, duration_ms: u64 },
    ToggleTheme,
    ToggleLanguage,
}

/// Cycles focus to the next input field when Tab is pressed on `LoginWizardStep::Start`.
pub fn next_login_field(
    tab: AuthWizardTab,
    current: LoginInputField,
    is_register_mode: bool,
) -> LoginInputField {
    if tab == AuthWizardTab::EmailRegister {
        match current {
            LoginInputField::Email => LoginInputField::EmailCode,
            LoginInputField::EmailCode => LoginInputField::Username,
            LoginInputField::Username => LoginInputField::Password,
            LoginInputField::Password => LoginInputField::FirstName,
            LoginInputField::FirstName => LoginInputField::Email,
        }
    } else {
        match current {
            LoginInputField::Username => LoginInputField::Password,
            LoginInputField::Password => {
                if is_register_mode {
                    LoginInputField::FirstName
                } else {
                    LoginInputField::Username
                }
            }
            LoginInputField::FirstName => LoginInputField::Username,
            _ => LoginInputField::Username,
        }
    }
}

/// Validates basic email syntax before dispatching an OTP send or registration request.
pub fn validate_email_address(email: &str) -> bool {
    let trimmed = email.trim();
    !trimmed.is_empty() && trimmed.contains('@') && trimmed.contains('.')
}

/// Returns `true` if the current login error string indicates a network/connectivity failure.
pub fn has_network_error(err: Option<&str>) -> bool {
    err.is_some_and(|e| {
        e.contains("Network connection error")
            || e.contains("timed out")
            || e.contains("Connection refused")
    })
}
