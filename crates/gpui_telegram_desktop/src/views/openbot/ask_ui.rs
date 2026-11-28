//! Interactive `ask_ui` block validation, secret redaction, Vault autofill masking,
//! GPUI `0.2.2` interactive card renderers (`confirm`, `quick_replies`, `choice`, `form`),
//! and OpenBot runtime turn cards (`OB-F15`).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::limits::{is_bounded_string, is_calendar_date, utf16_len};
use openbot_contracts::ui_blocks::{
    ConversationUiBlock, UI_BLOCK_LIMITS, UI_BLOCK_SUBMIT_ACTION_ID, UI_BLOCK_TEXT_ACTION_ID,
    UI_BLOCK_VERSION, UI_CHOICE_VALUES_KEY, UI_CONFIRM_ACTION_QUESTION_ID, UI_VAULT_SECRET_FIELDS,
    UiActionStyle, UiBlockResponse, UiBlockSpec, UiBlockStatus, UiBlockValue, UiBlockVaultRequest,
    UiConfirmField, UiFormField, UiVaultRequestKind, is_ui_block_id,
    ui_block_action_is_privileged, ui_block_outcome_text, ui_confirm_preview_editable,
    validate_ui_block_response,
};
use serde::{Deserialize, Serialize};

use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

/// Vault fields strictly masked in UI and stripped from persisted `ask_ui` responses.
pub const VAULT_MASKED_FIELDS: [&str; 3] = UI_VAULT_SECRET_FIELDS;
/// Minimum press-and-hold duration for `confirm` privileged actions (`400ms`).
pub const CONFIRM_HOLD_MIN_MS: u32 = UI_BLOCK_LIMITS.hold_min_ms;
/// Maximum press-and-hold duration for `confirm` privileged actions (`3,000ms`).
pub const CONFIRM_HOLD_MAX_MS: u32 = UI_BLOCK_LIMITS.hold_max_ms;
/// Default credit threshold above which media generation prompts for approval (`50` credits).
pub const DEFAULT_MEDIA_APPROVAL_CREDITS: u64 = 50;
/// Canonical `UiBlockCard` maximum width (`min(540px, 100%)`, `UI-F10-04`).
pub const UI_BLOCK_CARD_MAX_WIDTH_PX: f32 = 540.0;
pub const UI_BLOCK_CARD_WIDTH_EXPR: &str = "min(540px, 100%)";

const RESERVED_LEGACY_IDS: [&str; 4] = [
    "botId",
    "recipientBotId",
    "recipientBotIds",
    "senderBotId",
];

/// Domain validation failure returned by [`validate_and_evaluate_ui_block`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiBlockValidationError {
    pub code: String,
    pub message: String,
}

impl UiBlockValidationError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for UiBlockValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for UiBlockValidationError {}

/// Complete deterministic validation & evaluation summary for an `ask_ui` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiBlockEvaluationResult {
    pub valid: bool,
    pub block_type: String,
    pub hold_accepted: bool,
    pub secret_redacted_in_storage: bool,
    pub vault_masked_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sanitized_response: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_text: Option<String>,
}

fn check_ui_id(id: &str, context: &str) -> Result<(), UiBlockValidationError> {
    if id.is_empty() {
        return Err(UiBlockValidationError::new(
            "invalid_id",
            format!("{context} id cannot be empty"),
        ));
    }
    if id.starts_with('_') {
        return Err(UiBlockValidationError::new(
            "reserved_id",
            format!("{context} id '{id}' starting with underscore '_' is reserved and not allowed"),
        ));
    }
    if RESERVED_LEGACY_IDS.contains(&id) {
        return Err(UiBlockValidationError::new(
            "reserved_id",
            format!("{context} id '{id}' is a reserved legacy identifier and not allowed"),
        ));
    }
    if !is_ui_block_id(id) {
        return Err(UiBlockValidationError::new(
            "invalid_id",
            format!(
                "{context} id '{id}' is invalid: must be 1..={} ASCII [A-Za-z0-9_.:-]",
                UI_BLOCK_LIMITS.id
            ),
        ));
    }
    Ok(())
}

fn check_non_blank_str(
    val: Option<&serde_json::Value>,
    max_utf16: usize,
    label: &str,
) -> Result<String, UiBlockValidationError> {
    let raw = val.and_then(serde_json::Value::as_str).ok_or_else(|| {
        UiBlockValidationError::new("invalid_string", format!("{label} must be a string"))
    })?;
    if raw.trim().is_empty() {
        return Err(UiBlockValidationError::new(
            "blank_string",
            format!("{label} cannot be empty or blank"),
        ));
    }
    if !is_bounded_string(raw, max_utf16) {
        return Err(UiBlockValidationError::new(
            "string_limit",
            format!("{label} exceeds maximum length of {max_utf16} UTF-16 code units"),
        ));
    }
    Ok(raw.to_string())
}

fn check_string_options(
    val: Option<&serde_json::Value>,
    max_count: usize,
    context: &str,
) -> Result<Vec<String>, UiBlockValidationError> {
    let arr = val.and_then(serde_json::Value::as_array).ok_or_else(|| {
        UiBlockValidationError::new("invalid_options", format!("{context} options must be an array"))
    })?;
    if arr.is_empty() {
        return Err(UiBlockValidationError::new(
            "empty_options",
            format!("{context} options cannot be empty (at least 1 required, limit {max_count})"),
        ));
    }
    if arr.len() > max_count {
        return Err(UiBlockValidationError::new(
            "options_limit",
            format!(
                "{context} options exceed maximum limit of {max_count} (got {})",
                arr.len()
            ),
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    let mut seen_ci = HashSet::new();
    for (idx, item) in arr.iter().enumerate() {
        let s = check_non_blank_str(
            Some(item),
            UI_BLOCK_LIMITS.label,
            &format!("{context} option[{idx}]"),
        )?;
        if !seen_ci.insert(s.trim().to_lowercase()) {
            return Err(UiBlockValidationError::new(
                "duplicate_option",
                format!("{context} options contain duplicate label '{s}' (case-insensitive)"),
            ));
        }
        out.push(s);
    }
    Ok(out)
}

fn validate_raw_confirm(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<u32>, UiBlockValidationError> {
    check_non_blank_str(obj.get("title"), UI_BLOCK_LIMITS.title, "confirm title")?;

    let confirm_hold = if let Some(raw_hold) = obj.get("confirmHold") {
        let num = raw_hold.as_number().ok_or_else(|| {
            UiBlockValidationError::new(
                "invalid_confirm_hold",
                format!(
                    "confirmHold must be an integer between {CONFIRM_HOLD_MIN_MS} and {CONFIRM_HOLD_MAX_MS} ms"
                ),
            )
        })?;
        let Some(ms_u64) = num.as_u64() else {
            return Err(UiBlockValidationError::new(
                "invalid_confirm_hold",
                format!(
                    "confirmHold must be an integer between {CONFIRM_HOLD_MIN_MS} and {CONFIRM_HOLD_MAX_MS} ms (got non-integer or negative number)"
                ),
            ));
        };
        if ms_u64 < u64::from(CONFIRM_HOLD_MIN_MS) || ms_u64 > u64::from(CONFIRM_HOLD_MAX_MS) {
            return Err(UiBlockValidationError::new(
                "confirm_hold_range",
                format!(
                    "confirmHold {ms_u64} out of range: must be an integer between {CONFIRM_HOLD_MIN_MS} and {CONFIRM_HOLD_MAX_MS} ms"
                ),
            ));
        }
        Some(ms_u64 as u32)
    } else {
        None
    };

    if let Some(prev) = obj.get("preview") {
        check_non_blank_str(Some(prev), UI_BLOCK_LIMITS.preview, "confirm preview")?;
    }

    let actions = obj
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            UiBlockValidationError::new("invalid_actions", "confirm actions must be an array")
        })?;
    if actions.is_empty() {
        return Err(UiBlockValidationError::new(
            "empty_actions",
            "confirm actions cannot be empty (at least 1 required, at most 4 allowed)",
        ));
    }
    if actions.len() > UI_BLOCK_LIMITS.actions {
        return Err(UiBlockValidationError::new(
            "actions_limit",
            format!(
                "confirm actions exceed limit: at most {} actions allowed (got {})",
                UI_BLOCK_LIMITS.actions,
                actions.len()
            ),
        ));
    }
    let mut seen_ids = HashSet::new();
    let mut seen_labels = HashSet::new();
    for (idx, act) in actions.iter().enumerate() {
        let aobj = act.as_object().ok_or_else(|| {
            UiBlockValidationError::new("invalid_action", format!("confirm action[{idx}] must be an object"))
        })?;
        let aid = aobj.get("id").and_then(serde_json::Value::as_str).unwrap_or("");
        check_ui_id(aid, &format!("confirm action[{idx}]"))?;
        if !seen_ids.insert(aid.to_string()) {
            return Err(UiBlockValidationError::new(
                "duplicate_action_id",
                format!("confirm actions contain duplicate id '{aid}'"),
            ));
        }
        let lbl = check_non_blank_str(
            aobj.get("label"),
            UI_BLOCK_LIMITS.label,
            &format!("confirm action[{idx}] label"),
        )?;
        if !seen_labels.insert(lbl.trim().to_lowercase()) {
            return Err(UiBlockValidationError::new(
                "duplicate_action_label",
                format!("confirm actions contain duplicate label '{lbl}' (case-insensitive)"),
            ));
        }
    }

    if let Some(raw_fields) = obj.get("fields") {
        let fields = raw_fields.as_array().ok_or_else(|| {
            UiBlockValidationError::new("invalid_fields", "confirm fields must be an array")
        })?;
        if fields.len() > UI_BLOCK_LIMITS.confirm_fields {
            return Err(UiBlockValidationError::new(
                "confirm_fields_limit",
                format!(
                    "confirm fields exceed maximum limit of {} (got {})",
                    UI_BLOCK_LIMITS.confirm_fields,
                    fields.len()
                ),
            ));
        }
        let mut seen_selects = HashSet::new();
        for (idx, field) in fields.iter().enumerate() {
            let fobj = field.as_object().ok_or_else(|| {
                UiBlockValidationError::new("invalid_field", format!("confirm field[{idx}] must be an object"))
            })?;
            check_non_blank_str(
                fobj.get("label"),
                UI_BLOCK_LIMITS.label,
                &format!("confirm field[{idx}] label"),
            )?;
            if let Some(sel_val) = fobj.get("select") {
                let sel = sel_val.as_str().ok_or_else(|| {
                    UiBlockValidationError::new("invalid_select", format!("confirm field[{idx}].select must be a string"))
                })?;
                if sel == UI_CONFIRM_ACTION_QUESTION_ID {
                    return Err(UiBlockValidationError::new(
                        "reserved_select_id",
                        "confirm field select='action' is reserved and not allowed",
                    ));
                }
                check_ui_id(sel, &format!("confirm field[{idx}].select"))?;
                if !seen_selects.insert(sel.to_string()) {
                    return Err(UiBlockValidationError::new(
                        "duplicate_select_id",
                        format!("confirm fields contain duplicate select id '{sel}'"),
                    ));
                }
                check_string_options(
                    fobj.get("options"),
                    UI_BLOCK_LIMITS.select_options,
                    &format!("confirm select field '{sel}'"),
                )?;
            } else {
                let v = fobj
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        UiBlockValidationError::new("invalid_field_value", format!("confirm field[{idx}].value must be a string"))
                    })?;
                if !is_bounded_string(v, UI_BLOCK_LIMITS.text) {
                    return Err(UiBlockValidationError::new(
                        "field_value_limit",
                        format!("confirm field[{idx}].value exceeds maximum limit of {}", UI_BLOCK_LIMITS.text),
                    ));
                }
            }
        }
    }

    Ok(confirm_hold)
}

fn validate_raw_quick_replies(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), UiBlockValidationError> {
    if let Some(title) = obj.get("title") {
        check_non_blank_str(Some(title), UI_BLOCK_LIMITS.title, "quick_replies title")?;
    }
    let options = obj
        .get("options")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            UiBlockValidationError::new("invalid_options", "quick_replies options must be an array")
        })?;
    if options.is_empty() {
        return Err(UiBlockValidationError::new(
            "empty_options",
            format!(
                "quick_replies options cannot be empty (at least 1 required, limit {})",
                UI_BLOCK_LIMITS.quick_replies
            ),
        ));
    }
    if options.len() > UI_BLOCK_LIMITS.quick_replies {
        return Err(UiBlockValidationError::new(
            "options_limit",
            format!(
                "quick_replies options exceed maximum limit of {} (got {})",
                UI_BLOCK_LIMITS.quick_replies,
                options.len()
            ),
        ));
    }
    let mut seen_ids = HashSet::new();
    let mut seen_labels = HashSet::new();
    for (idx, opt) in options.iter().enumerate() {
        let oobj = opt.as_object().ok_or_else(|| {
            UiBlockValidationError::new("invalid_option", format!("quick_replies option[{idx}] must be an object"))
        })?;
        let oid = oobj.get("id").and_then(serde_json::Value::as_str).unwrap_or("");
        check_ui_id(oid, &format!("quick_replies option[{idx}]"))?;
        if !seen_ids.insert(oid.to_string()) {
            return Err(UiBlockValidationError::new(
                "duplicate_option_id",
                format!("quick_replies options contain duplicate id '{oid}'"),
            ));
        }
        let lbl = check_non_blank_str(
            oobj.get("label"),
            UI_BLOCK_LIMITS.label,
            &format!("quick_replies option[{idx}] label"),
        )?;
        if !seen_labels.insert(lbl.trim().to_lowercase()) {
            return Err(UiBlockValidationError::new(
                "duplicate_option_label",
                format!("quick_replies options contain duplicate label '{lbl}' (case-insensitive)"),
            ));
        }
    }
    Ok(())
}

fn validate_raw_choice(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), UiBlockValidationError> {
    check_non_blank_str(obj.get("title"), UI_BLOCK_LIMITS.title, "choice title")?;
    let multiple = obj
        .get("multiple")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let options = obj
        .get("options")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            UiBlockValidationError::new("invalid_options", "choice options must be an array")
        })?;
    if options.is_empty() {
        return Err(UiBlockValidationError::new(
            "empty_options",
            format!(
                "choice options cannot be empty (at least 1 required, limit {})",
                UI_BLOCK_LIMITS.choice_options
            ),
        ));
    }
    if options.len() > UI_BLOCK_LIMITS.choice_options {
        return Err(UiBlockValidationError::new(
            "options_limit",
            format!(
                "choice options exceed maximum limit of {} (got {})",
                UI_BLOCK_LIMITS.choice_options,
                options.len()
            ),
        ));
    }
    let mut seen_ids = HashSet::new();
    let mut seen_labels = HashSet::new();
    let mut preselected = 0usize;
    for (idx, opt) in options.iter().enumerate() {
        let oobj = opt.as_object().ok_or_else(|| {
            UiBlockValidationError::new("invalid_option", format!("choice option[{idx}] must be an object"))
        })?;
        let oid = oobj.get("id").and_then(serde_json::Value::as_str).unwrap_or("");
        check_ui_id(oid, &format!("choice option[{idx}]"))?;
        if !seen_ids.insert(oid.to_string()) {
            return Err(UiBlockValidationError::new(
                "duplicate_option_id",
                format!("choice options contain duplicate id '{oid}'"),
            ));
        }
        let lbl = check_non_blank_str(
            oobj.get("label"),
            UI_BLOCK_LIMITS.label,
            &format!("choice option[{idx}] label"),
        )?;
        if !seen_labels.insert(lbl.trim().to_lowercase()) {
            return Err(UiBlockValidationError::new(
                "duplicate_option_label",
                format!("choice options contain duplicate label '{lbl}' (case-insensitive)"),
            ));
        }
        if let Some(meta) = oobj.get("meta") {
            check_non_blank_str(Some(meta), UI_BLOCK_LIMITS.meta, &format!("choice option[{idx}] meta"))?;
        }
        if oobj.get("selected").and_then(serde_json::Value::as_bool) == Some(true) {
            preselected += 1;
        }
    }
    if !multiple && preselected > 1 {
        return Err(UiBlockValidationError::new(
            "multiple_not_allowed",
            "choice allows at most one selected option when multiple is false",
        ));
    }
    Ok(())
}

fn validate_raw_form(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), UiBlockValidationError> {
    check_non_blank_str(obj.get("title"), UI_BLOCK_LIMITS.title, "form title")?;
    let fields = obj
        .get("fields")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            UiBlockValidationError::new("invalid_fields", "form fields must be an array")
        })?;
    if fields.is_empty() {
        return Err(UiBlockValidationError::new(
            "empty_fields",
            format!(
                "form fields cannot be empty (at least 1 required, limit {})",
                UI_BLOCK_LIMITS.form_fields
            ),
        ));
    }
    if fields.len() > UI_BLOCK_LIMITS.form_fields {
        return Err(UiBlockValidationError::new(
            "fields_limit",
            format!(
                "form fields exceed maximum limit of {} (got {})",
                UI_BLOCK_LIMITS.form_fields,
                fields.len()
            ),
        ));
    }
    let mut seen_ids = HashSet::new();
    for (idx, field) in fields.iter().enumerate() {
        let fobj = field.as_object().ok_or_else(|| {
            UiBlockValidationError::new("invalid_field", format!("form field[{idx}] must be an object"))
        })?;
        let fid = fobj.get("id").and_then(serde_json::Value::as_str).unwrap_or("");
        check_ui_id(fid, &format!("form field[{idx}]"))?;
        if !seen_ids.insert(fid.to_string()) {
            return Err(UiBlockValidationError::new(
                "duplicate_field_id",
                format!("form fields contain duplicate id '{fid}'"),
            ));
        }
        let kind = fobj.get("kind").and_then(serde_json::Value::as_str).ok_or_else(|| {
            UiBlockValidationError::new("invalid_field_kind", format!("form field '{fid}' missing kind"))
        })?;
        let is_secret = fobj.get("secret").and_then(serde_json::Value::as_bool) == Some(true);
        match kind {
            "text" => {
                check_non_blank_str(fobj.get("label"), UI_BLOCK_LIMITS.label, &format!("text field '{fid}' label"))?;
                if is_secret && fobj.get("value").is_some_and(|v| !v.is_null()) {
                    return Err(UiBlockValidationError::new(
                        "secret_prefill_forbidden",
                        format!("secret field '{fid}' cannot have a prefill value (not allowed)"),
                    ));
                }
            }
            "textarea" => {
                check_non_blank_str(fobj.get("label"), UI_BLOCK_LIMITS.label, &format!("textarea field '{fid}' label"))?;
                if is_secret {
                    return Err(UiBlockValidationError::new(
                        "secret_textarea_forbidden",
                        format!("secret is only allowed on single-line text fields, not textarea ('{fid}')"),
                    ));
                }
            }
            "segmented" => {
                let opts = check_string_options(
                    fobj.get("options"),
                    UI_BLOCK_LIMITS.segmented_options,
                    &format!("segmented field '{fid}'"),
                )?;
                if let Some(val) = fobj.get("value").and_then(serde_json::Value::as_str)
                    && !opts.iter().any(|o| o == val)
                {
                    return Err(UiBlockValidationError::new(
                        "invalid_default",
                        format!("segmented field '{fid}' default value '{val}' is not in options"),
                    ));
                }
            }
            "select" => {
                let opts = check_string_options(
                    fobj.get("options"),
                    UI_BLOCK_LIMITS.select_options,
                    &format!("select field '{fid}'"),
                )?;
                if let Some(val) = fobj.get("value").and_then(serde_json::Value::as_str)
                    && !opts.iter().any(|o| o == val)
                {
                    return Err(UiBlockValidationError::new(
                        "invalid_default",
                        format!("select field '{fid}' default value '{val}' is not in options"),
                    ));
                }
            }
            "date" => {
                check_non_blank_str(fobj.get("label"), UI_BLOCK_LIMITS.label, &format!("date field '{fid}' label"))?;
                if let Some(val) = fobj.get("value").and_then(serde_json::Value::as_str)
                    && !is_calendar_date(val)
                {
                    return Err(UiBlockValidationError::new(
                        "invalid_date",
                        format!("invalid date '{val}' on field '{fid}': not a valid Gregorian calendar date"),
                    ));
                }
            }
            other => {
                return Err(UiBlockValidationError::new(
                    "invalid_field_kind",
                    format!("unsupported form field kind '{other}' on field '{fid}'"),
                ));
            }
        }
    }
    Ok(())
}

/// Strips secret form fields and Vault secret keys (`password`, `number`, `securityCode`)
/// from a validated [`UiBlockResponse`] before persistence.
///
/// Returns `(sanitized_response, redacted_any_secret)`.
pub fn redact_ui_block_response_for_storage(
    spec: &UiBlockSpec,
    response: &UiBlockResponse,
    vault: Option<&UiBlockVaultRequest>,
) -> (UiBlockResponse, bool) {
    let mut secret_ids: HashSet<&str> = HashSet::new();
    if let UiBlockSpec::Form { fields, .. } = spec {
        for f in fields {
            if f.is_secret() {
                secret_ids.insert(f.id());
            }
        }
    }
    if vault.is_some() {
        for masked in VAULT_MASKED_FIELDS {
            secret_ids.insert(masked);
        }
    }
    let mut sanitized = response.clone();
    let mut redacted = false;
    if let Some(vals) = sanitized.values.as_mut() {
        let keys_to_drop: Vec<String> = vals
            .keys()
            .filter(|k| secret_ids.contains(k.as_str()))
            .cloned()
            .collect();
        for key in keys_to_drop {
            vals.remove(&key);
            redacted = true;
        }
    }
    (sanitized, redacted)
}

/// Strictly validates a raw `ask_ui` JSON specification and optional user response,
/// enforcing all `confirm`, `quick_replies`, `choice`, and `form` invariants and redacting
/// all `secret: true` and Vault masked fields from the returned `sanitized_response`.
pub fn validate_and_evaluate_ui_block(
    spec: &serde_json::Value,
    response: Option<&serde_json::Value>,
    hold_duration_ms: Option<u32>,
) -> Result<UiBlockEvaluationResult, UiBlockValidationError> {
    let serialized_spec = serde_json::to_string(spec).map_err(|e| {
        UiBlockValidationError::new("invalid_spec_json", format!("failed to serialize spec: {e}"))
    })?;
    if utf16_len(&serialized_spec) > UI_BLOCK_LIMITS.spec_json {
        return Err(UiBlockValidationError::new(
            "spec_size_limit",
            format!(
                "ui_block spec exceeds maximum JSON length of {} UTF-16 code units",
                UI_BLOCK_LIMITS.spec_json
            ),
        ));
    }
    let obj = spec.as_object().ok_or_else(|| {
        UiBlockValidationError::new("invalid_spec", "ui_block spec must be a JSON object")
    })?;

    if let Some(ver_val) = obj.get("version") {
        let Some(ver_u64) = ver_val.as_u64() else {
            return Err(UiBlockValidationError::new(
                "invalid_version",
                "ui_block version must be integer 1",
            ));
        };
        if ver_u64 != u64::from(UI_BLOCK_VERSION) {
            return Err(UiBlockValidationError::new(
                "unsupported_version",
                format!("unsupported ui_block version {ver_u64}: expected {UI_BLOCK_VERSION}"),
            ));
        }
    }

    let block_type = obj
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            UiBlockValidationError::new(
                "missing_type",
                "ui_block spec requires string 'type' in confirm|quick_replies|choice|form",
            )
        })?;

    let confirm_hold_ms = match block_type {
        "confirm" => validate_raw_confirm(obj)?,
        "quick_replies" => {
            validate_raw_quick_replies(obj)?;
            None
        }
        "choice" => {
            validate_raw_choice(obj)?;
            None
        }
        "form" => {
            validate_raw_form(obj)?;
            None
        }
        other => {
            return Err(UiBlockValidationError::new(
                "invalid_type",
                format!(
                    "unsupported ui_block type '{other}': expected confirm|quick_replies|choice|form"
                ),
            ));
        }
    };

    let parsed_vault: Option<UiBlockVaultRequest> = if let Some(raw_vault) = obj.get("vault") {
        if block_type != "form" {
            return Err(UiBlockValidationError::new(
                "invalid_vault",
                "vault autofill metadata is only allowed on 'form' ui_block specs",
            ));
        }
        let v: UiBlockVaultRequest = serde_json::from_value(raw_vault.clone()).map_err(|e| {
            UiBlockValidationError::new("invalid_vault", format!("invalid vault request: {e}"))
        })?;
        Some(v)
    } else {
        None
    };

    let parsed_spec: UiBlockSpec = serde_json::from_value(spec.clone()).map_err(|e| {
        UiBlockValidationError::new("invalid_spec_schema", format!("invalid ui_block spec: {e}"))
    })?;
    if !parsed_spec.validate() {
        return Err(UiBlockValidationError::new(
            "invalid_spec",
            format!("ui_block spec '{block_type}' failed contract invariant validation"),
        ));
    }
    if let Some(ref vault_req) = parsed_vault
        && !vault_req.is_valid_for(&parsed_spec)
    {
        return Err(UiBlockValidationError::new(
            "invalid_vault",
            "vault autofill metadata failed contract validation",
        ));
    }

    let hold_accepted = match confirm_hold_ms {
        Some(required_ms) => hold_duration_ms.unwrap_or(0) >= required_ms,
        None => true,
    };

    let has_secret_field = matches!(
        &parsed_spec,
        UiBlockSpec::Form { fields, .. } if fields.iter().any(UiFormField::is_secret)
    ) || parsed_vault.is_some();

    let (sanitized_response, secret_redacted_in_storage, outcome_text) =
        if let Some(raw_resp) = response {
            let encoded_resp = serde_json::to_string(raw_resp).map_err(|e| {
                UiBlockValidationError::new(
                    "invalid_response",
                    format!("failed to serialize ui_block response: {e}"),
                )
            })?;
            if utf16_len(&encoded_resp) > UI_BLOCK_LIMITS.response_json {
                return Err(UiBlockValidationError::new(
                    "response_size_limit",
                    format!(
                        "ui_block response exceeds maximum length of {} UTF-16 code units",
                        UI_BLOCK_LIMITS.response_json
                    ),
                ));
            }
            let parsed_resp: UiBlockResponse =
                serde_json::from_value(raw_resp.clone()).map_err(|e| {
                    UiBlockValidationError::new(
                        "invalid_response_schema",
                        format!("invalid ui_block response: {e}"),
                    )
                })?;

            validate_response_details(&parsed_spec, &parsed_resp)?;

            let validated_live =
                validate_ui_block_response(&parsed_spec, &parsed_resp, false).ok_or_else(|| {
                    UiBlockValidationError::new(
                        "invalid_response",
                        format!("ui_block response is invalid for '{block_type}' specification"),
                    )
                })?;

            let (sanitized, redacted_any) = redact_ui_block_response_for_storage(
                &parsed_spec,
                &validated_live,
                parsed_vault.as_ref(),
            );
            let _stored_check =
                validate_ui_block_response(&parsed_spec, &sanitized, true).ok_or_else(|| {
                    UiBlockValidationError::new(
                        "invalid_stored_response",
                        "redacted ui_block response failed storage invariant validation",
                    )
                })?;
            let outcome = ui_block_outcome_text(&parsed_spec, &sanitized);
            let sanitized_json = serde_json::to_value(&sanitized).map_err(|e| {
                UiBlockValidationError::new(
                    "serialize_error",
                    format!("failed to encode sanitized response: {e}"),
                )
            })?;
            (
                Some(sanitized_json),
                redacted_any || has_secret_field,
                Some(outcome),
            )
        } else {
            (None, has_secret_field, None)
        };

    Ok(UiBlockEvaluationResult {
        valid: true,
        block_type: block_type.to_string(),
        hold_accepted,
        secret_redacted_in_storage,
        vault_masked_fields: VAULT_MASKED_FIELDS.iter().map(|s| (*s).to_string()).collect(),
        sanitized_response,
        outcome_text,
    })
}

fn validate_response_details(
    spec: &UiBlockSpec,
    resp: &UiBlockResponse,
) -> Result<(), UiBlockValidationError> {
    if resp.action_id == UI_BLOCK_TEXT_ACTION_ID {
        if let UiBlockSpec::QuickReplies { allow_text, .. } = spec
            && *allow_text == Some(false)
        {
            return Err(UiBlockValidationError::new(
                "text_reply_disallowed",
                "quick_replies has allowText=false; '_text' free-text response is not allowed",
            ));
        }
        let text = resp.text.as_deref().unwrap_or("");
        if text.trim().is_empty() {
            return Err(UiBlockValidationError::new(
                "blank_text_reply",
                "free-text '_text' response requires non-blank text",
            ));
        }
        return Ok(());
    }
    match spec {
        UiBlockSpec::Choice { multiple, .. } => {
            if *multiple != Some(true)
                && let Some(vals) = &resp.values
                && let Some(UiBlockValue::List(selected)) = vals.get(UI_CHOICE_VALUES_KEY)
                && selected.len() > 1
            {
                return Err(UiBlockValidationError::new(
                    "multiple_selection_forbidden",
                    format!(
                        "choice response selected {} options when multiple is false (single option required)",
                        selected.len()
                    ),
                ));
            }
        }
        UiBlockSpec::Form { fields, .. } => {
            if let Some(vals) = &resp.values {
                for field in fields {
                    if let UiFormField::Date { id, .. } = field
                        && let Some(UiBlockValue::Text(date_str)) = vals.get(id)
                        && !date_str.is_empty()
                        && !is_calendar_date(date_str)
                    {
                        return Err(UiBlockValidationError::new(
                            "invalid_date",
                            format!(
                                "invalid date '{date_str}' on calendar field '{id}': not a valid Gregorian calendar date (YYYY-MM-DD)"
                            ),
                        ));
                    }
                }
            }
        }
        UiBlockSpec::Confirm { .. } | UiBlockSpec::QuickReplies { .. } => {}
    }
    Ok(())
}

/// Interactive client state for an inline `ask_ui` card inside a conversation turn.
#[derive(Debug, Clone, PartialEq)]
pub struct UiBlockCardState {
    pub block: ConversationUiBlock,
    pub holding_action_id: Option<String>,
    pub hold_elapsed_ms: u32,
    pub draft_values: BTreeMap<String, String>,
    pub selected_choice_ids: Vec<String>,
    pub free_text_draft: String,
    pub preview_edit_draft: Option<String>,
    pub secret_revealed_fields: BTreeSet<String>,
    pub vault_autofill_label: Option<String>,
    pub validation_error: Option<String>,
}

impl UiBlockCardState {
    pub fn new(block: ConversationUiBlock) -> Self {
        let mut draft_values = BTreeMap::new();
        let mut selected_choice_ids = Vec::new();
        let mut preview_edit_draft = None;

        match &block.spec {
            UiBlockSpec::Confirm {
                fields, preview, ..
            } => {
                if let Some(list) = fields {
                    for f in list {
                        if let UiConfirmField::Select {
                            select, options, ..
                        } = f
                            && let Some(first) = options.first()
                        {
                            draft_values.insert(select.clone(), first.clone());
                        }
                    }
                }
                if ui_confirm_preview_editable(&block.spec) {
                    preview_edit_draft = preview.clone();
                }
            }
            UiBlockSpec::Choice { options, .. } => {
                for opt in options {
                    if opt.selected == Some(true) {
                        selected_choice_ids.push(opt.id.clone());
                    }
                }
            }
            UiBlockSpec::Form { fields, .. } => {
                for field in fields {
                    let initial = match field {
                        UiFormField::Text { value, secret, .. } => {
                            if *secret == Some(true) {
                                None
                            } else {
                                value.clone()
                            }
                        }
                        UiFormField::Textarea { value, .. }
                        | UiFormField::Select { value, .. }
                        | UiFormField::Segmented { value, .. }
                        | UiFormField::Date { value, .. } => value.clone(),
                    };
                    if let Some(v) = initial {
                        draft_values.insert(field.id().to_string(), v);
                    }
                }
            }
            UiBlockSpec::QuickReplies { .. } => {}
        }

        Self {
            block,
            holding_action_id: None,
            hold_elapsed_ms: 0,
            draft_values,
            selected_choice_ids,
            free_text_draft: String::new(),
            preview_edit_draft,
            secret_revealed_fields: BTreeSet::new(),
            vault_autofill_label: None,
            validation_error: None,
        }
    }

    /// Required hold threshold in ms if this block is a `confirm` card with `confirmHold`.
    pub fn required_hold_ms(&self) -> u32 {
        match &self.block.spec {
            UiBlockSpec::Confirm { confirm_hold, .. } => confirm_hold.unwrap_or(0),
            _ => 0,
        }
    }

    /// Fractional fill (`0.0..=1.0`) for an active `UiHoldButton` press-and-hold action.
    pub fn hold_progress(&self, action_id: &str) -> f32 {
        let req = self.required_hold_ms();
        if req == 0 || self.holding_action_id.as_deref() != Some(action_id) {
            return 0.0;
        }
        (self.hold_elapsed_ms as f32 / req as f32).clamp(0.0, 1.0)
    }

    /// Advances press-and-hold timer; returns `true` when threshold is met.
    pub fn advance_hold(&mut self, action_id: &str, elapsed_ms: u32) -> bool {
        self.holding_action_id = Some(action_id.to_string());
        self.hold_elapsed_ms = elapsed_ms;
        let req = self.required_hold_ms();
        req == 0 || elapsed_ms >= req
    }

    /// Cancels an active press-and-hold gesture cleanly.
    pub fn cancel_hold(&mut self) {
        self.holding_action_id = None;
        self.hold_elapsed_ms = 0;
    }

    /// Toggles selection of a `choice` option while enforcing single-select when `multiple != true`.
    pub fn toggle_choice(&mut self, option_id: &str) {
        let multi = matches!(
            self.block.spec,
            UiBlockSpec::Choice {
                multiple: Some(true),
                ..
            }
        );
        if let Some(pos) = self.selected_choice_ids.iter().position(|id| id == option_id) {
            self.selected_choice_ids.remove(pos);
        } else {
            if !multi {
                self.selected_choice_ids.clear();
            }
            self.selected_choice_ids.push(option_id.to_string());
        }
    }

    /// Updates a form or confirm select field value.
    pub fn set_field_value(&mut self, field_id: impl Into<String>, value: impl Into<String>) {
        self.draft_values.insert(field_id.into(), value.into());
        self.validation_error = None;
    }

    /// Toggles temporary plaintext reveal for a single masked secret field.
    pub fn toggle_secret_visibility(&mut self, field_id: &str) -> bool {
        if self.secret_revealed_fields.contains(field_id) {
            self.secret_revealed_fields.remove(field_id);
            false
        } else {
            self.secret_revealed_fields.insert(field_id.to_string());
            true
        }
    }

    /// Records a Vault autofill selection label (keeping raw secret values out of display).
    pub fn apply_vault_autofill(&mut self, item_name: &str, masked_last4: Option<&str>) {
        let label = match masked_last4 {
            Some(last4) => format!("{item_name} •••• {last4}"),
            None => item_name.to_string(),
        };
        self.vault_autofill_label = Some(label);
    }

    /// Builds and validates a [`UiBlockResponse`] for `action_id`, returning both the live
    /// response and the storage-redacted response.
    pub fn submit_action(
        &mut self,
        action_id: &str,
    ) -> Result<(UiBlockResponse, UiBlockResponse), UiBlockValidationError> {
        let response = if action_id == UI_BLOCK_TEXT_ACTION_ID {
            UiBlockResponse {
                action_id: UI_BLOCK_TEXT_ACTION_ID.to_string(),
                values: None,
                text: Some(self.free_text_draft.clone()),
            }
        } else {
            match &self.block.spec {
                UiBlockSpec::Confirm { .. } => {
                    if ui_block_action_is_privileged(&self.block.spec, action_id) {
                        let req = self.required_hold_ms();
                        if req > 0 && self.hold_elapsed_ms < req {
                            let err = UiBlockValidationError::new(
                                "hold_required",
                                format!("hold action '{action_id}' for at least {req}ms to confirm"),
                            );
                            self.validation_error = Some(err.message.clone());
                            return Err(err);
                        }
                    }
                    let vals: BTreeMap<String, UiBlockValue> = self
                        .draft_values
                        .iter()
                        .map(|(k, v)| (k.clone(), UiBlockValue::Text(v.clone())))
                        .collect();
                    UiBlockResponse {
                        action_id: action_id.to_string(),
                        values: (!vals.is_empty()).then_some(vals),
                        text: None,
                    }
                }
                UiBlockSpec::QuickReplies { .. } => UiBlockResponse {
                    action_id: action_id.to_string(),
                    values: None,
                    text: None,
                },
                UiBlockSpec::Choice { .. } => {
                    let mut vals = BTreeMap::new();
                    vals.insert(
                        UI_CHOICE_VALUES_KEY.to_string(),
                        UiBlockValue::List(self.selected_choice_ids.clone()),
                    );
                    UiBlockResponse {
                        action_id: UI_BLOCK_SUBMIT_ACTION_ID.to_string(),
                        values: Some(vals),
                        text: None,
                    }
                }
                UiBlockSpec::Form { .. } => {
                    let vals: BTreeMap<String, UiBlockValue> = self
                        .draft_values
                        .iter()
                        .map(|(k, v)| (k.clone(), UiBlockValue::Text(v.clone())))
                        .collect();
                    UiBlockResponse {
                        action_id: UI_BLOCK_SUBMIT_ACTION_ID.to_string(),
                        values: Some(vals),
                        text: None,
                    }
                }
            }
        };

        validate_response_details(&self.block.spec, &response)?;
        let Some(live) = validate_ui_block_response(&self.block.spec, &response, false) else {
            let err = UiBlockValidationError::new(
                "invalid_response",
                "response failed ask_ui contract validation",
            );
            self.validation_error = Some(err.message.clone());
            return Err(err);
        };
        let (stored, _) =
            redact_ui_block_response_for_storage(&self.block.spec, &live, self.block.vault.as_ref());
        self.validation_error = None;
        Ok((live, stored))
    }
}

fn pal_rgba(palette: &OpenBotPalette, token: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback_hex))
}

fn status_badge_label(status: UiBlockStatus) -> &'static str {
    match status {
        UiBlockStatus::Pending => "Pending",
        UiBlockStatus::Answered => "Answered",
        UiBlockStatus::Expired => "Expired",
        UiBlockStatus::Closed => "Closed",
    }
}

/// Renders an interactive 1:1 OpenBot `ask_ui` card (`confirm`, `quick_replies`, `choice`, or `form`)
/// including press-and-hold `UiHoldButton` progress bars, masked secret inputs, and Vault / API key /
/// Location consent overlays.
pub fn render_ui_block_card(state: &UiBlockCardState, palette: &OpenBotPalette) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let surface_raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border_strong = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal_rgba(palette, "--openbot-accent-soft", 0x1A365D);
    let text_on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xFFFFFF);
    let danger = pal_rgba(palette, "--openbot-danger", 0xFD2F3B);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x4A1B20);
    let success = pal_rgba(palette, "--openbot-success", 0x31CF76);

    let is_danger_card = matches!(
        state.block.spec,
        UiBlockSpec::Confirm {
            danger: Some(true),
            ..
        }
    );
    let card_border = if is_danger_card { danger } else { border_strong };

    let title_str = match &state.block.spec {
        UiBlockSpec::Confirm { title, .. }
        | UiBlockSpec::Choice { title, .. }
        | UiBlockSpec::Form { title, .. } => title.clone(),
        UiBlockSpec::QuickReplies { title, .. } => {
            title.clone().unwrap_or_else(|| "Quick Replies".to_string())
        }
    };

    let status_color = match state.block.state.status {
        UiBlockStatus::Pending => accent,
        UiBlockStatus::Answered => success,
        UiBlockStatus::Expired | UiBlockStatus::Closed => text_muted,
    };

    let mut card = div()
        .w_full()
        .max_w(px(UI_BLOCK_CARD_MAX_WIDTH_PX))
        .rounded(px(14.0))
        .bg(surface)
        .border_1()
        .border_color(card_border)
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .child(tg_icon_rgba(
                            if is_danger_card {
                                IconName::Lock
                            } else {
                                IconName::Poll
                            },
                            15.0,
                            if is_danger_card { danger } else { accent },
                        ))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .truncate()
                                .child(title_str),
                        ),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded_full()
                        .bg(surface_raised)
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(status_color)
                        .child(status_badge_label(state.block.state.status)),
                ),
        );

    if let Some(ref vault) = state.block.vault {
        let kind_label = match vault.kind {
            UiVaultRequestKind::Login => "Vault Login Autofill",
            UiVaultRequestKind::Card => "Vault Payment Card",
            UiVaultRequestKind::Contact => "Vault Contact Card",
        };
        let saved_hint = state
            .vault_autofill_label
            .clone()
            .or_else(|| {
                vault.saved.as_ref().map(|s| match &s.last4 {
                    Some(l4) => format!("{} •••• {l4}", s.name),
                    None => s.name.clone(),
                })
            })
            .unwrap_or_else(|| format!("Masked: {}", VAULT_MASKED_FIELDS.join(", ")));
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(10.0))
                .bg(accent_soft)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::Lock, 13.0, accent))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .child(kind_label),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_secondary)
                        .child(saved_hint),
                ),
        );
    }

    if state.block.api_key == Some(true) {
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface_raised)
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon_rgba(IconName::Lock, 13.0, accent))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_secondary)
                        .child("API Key consent · secret values are masked and never stored"),
                ),
        );
    }

    if state.block.location == Some(true) {
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface_raised)
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon_rgba(IconName::LocationPin, 13.0, accent))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_secondary)
                        .child("Location sharing consent requested"),
                ),
        );
    }

    card = match &state.block.spec {
        UiBlockSpec::Confirm {
            confirm_hold,
            fields,
            preview,
            actions,
            ..
        } => {
            if let Some(f_list) = fields {
                let mut f_col = div().flex().flex_col().gap(px(6.0));
                for f in f_list {
                    let row = match f {
                        UiConfirmField::Value { label, value } => div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(surface_raised)
                            .child(div().text_size(px(12.0)).text_color(text_muted).child(label.clone()))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_primary)
                                    .child(value.clone()),
                            ),
                        UiConfirmField::Select {
                            label,
                            select,
                            options,
                        } => {
                            let active_val = state
                                .draft_values
                                .get(select)
                                .cloned()
                                .or_else(|| options.first().cloned())
                                .unwrap_or_default();
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(surface_raised)
                                .child(div().text_size(px(12.0)).text_color(text_muted).child(label.clone()))
                                .child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded(px(6.0))
                                        .bg(control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .text_size(px(12.0))
                                        .text_color(text_primary)
                                        .child(format!("{active_val} ▾")),
                                )
                        }
                    };
                    f_col = f_col.child(row);
                }
                card = card.child(f_col);
            }
            if let Some(prev) = preview {
                let editable = ui_confirm_preview_editable(&state.block.spec);
                let shown = state
                    .preview_edit_draft
                    .as_deref()
                    .unwrap_or(prev.as_str())
                    .to_string();
                card = card.child(
                    div()
                        .rounded(px(10.0))
                        .bg(surface_raised)
                        .border_1()
                        .border_color(border_strong)
                        .p(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .when(editable, |el| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(accent)
                                    .child("Editable preview"),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_secondary)
                                .line_height(relative(1.4))
                                .child(shown),
                        ),
                );
            }
            let mut action_row = div().flex().flex_wrap().items_center().gap(px(8.0)).pt(px(2.0));
            for action in actions {
                let privileged = ui_block_action_is_privileged(&state.block.spec, &action.id);
                let hold_ms = if privileged {
                    confirm_hold.unwrap_or(0)
                } else {
                    0
                };
                let (btn_bg, btn_fg) = match action.style.unwrap_or(UiActionStyle::Secondary) {
                    UiActionStyle::Primary => (accent, text_on_accent),
                    UiActionStyle::Danger => (danger, text_on_accent),
                    UiActionStyle::Secondary => (surface_raised, text_primary),
                    UiActionStyle::Ghost => (rgba(0x00000000), text_secondary),
                };
                let progress = state.hold_progress(&action.id);
                let label_text = if hold_ms > 0 {
                    format!("Hold {} ({:.1}s)", action.label, f64::from(hold_ms) / 1000.0)
                } else {
                    action.label.clone()
                };
                action_row = action_row.child(
                    div()
                        .relative()
                        .overflow_hidden()
                        .h(px(32.0))
                        .px(px(12.0))
                        .rounded(px(8.0))
                        .bg(if privileged && hold_ms > 0 {
                            danger_soft
                        } else {
                            btn_bg
                        })
                        .border_1()
                        .border_color(if privileged { danger } else { border_strong })
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(hold_ms > 0 && progress > 0.0, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(px(0.0))
                                    .bottom(px(0.0))
                                    .left(px(0.0))
                                    .w(relative(progress))
                                    .bg(danger),
                            )
                        })
                        .child(
                            div()
                                .relative()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if privileged && hold_ms > 0 {
                                    text_primary
                                } else {
                                    btn_fg
                                })
                                .child(label_text),
                        ),
                );
            }
            card.child(action_row)
        }
        UiBlockSpec::QuickReplies {
            options,
            allow_text,
            ..
        } => {
            let mut pills = div().flex().flex_wrap().gap(px(6.0));
            for opt in options {
                pills = pills.child(
                    div()
                        .h(px(30.0))
                        .px(px(12.0))
                        .rounded_full()
                        .bg(surface_raised)
                        .border_1()
                        .border_color(border_strong)
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .text_color(text_primary)
                        .child(opt.label.clone()),
                );
            }
            card = card.child(pills);
            if *allow_text == Some(true) {
                let placeholder = if state.free_text_draft.is_empty() {
                    "Type a custom reply…".to_string()
                } else {
                    state.free_text_draft.clone()
                };
                card = card.child(
                    div()
                        .h(px(34.0))
                        .px(px(10.0))
                        .rounded(px(8.0))
                        .bg(control)
                        .border_1()
                        .border_color(border_strong)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(if state.free_text_draft.is_empty() {
                                    text_muted
                                } else {
                                    text_primary
                                })
                                .child(placeholder),
                        )
                        .child(tg_icon_rgba(IconName::SendPlane, 14.0, accent)),
                );
            }
            card
        }
        UiBlockSpec::Choice {
            multiple,
            options,
            submit,
            ..
        } => {
            let is_multi = *multiple == Some(true);
            let mut list = div().flex().flex_col().gap(px(6.0));
            for opt in options {
                let checked = state.selected_choice_ids.iter().any(|id| id == &opt.id);
                let marker = if is_multi {
                    if checked { "☑" } else { "☐" }
                } else if checked {
                    "●"
                } else {
                    "○"
                };
                list = list.child(
                    div()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(if checked { accent_soft } else { surface_raised })
                        .border_1()
                        .border_color(if checked { accent } else { border_strong })
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .cursor_pointer()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(if checked { accent } else { text_muted })
                                        .child(marker),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_primary)
                                        .child(opt.label.clone()),
                                ),
                        )
                        .when_some(opt.meta.clone(), |el, meta| {
                            el.child(div().text_size(px(11.0)).text_color(text_muted).child(meta))
                        }),
                );
            }
            card.child(list).child(
                div()
                    .h(px(32.0))
                    .px(px(14.0))
                    .rounded(px(8.0))
                    .bg(accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_on_accent)
                    .child(submit.clone().unwrap_or_else(|| "Submit".to_string())),
            )
        }
        UiBlockSpec::Form { fields, submit, .. } => {
            let mut form_col = div().flex().flex_col().gap(px(8.0));
            for field in fields {
                let fid = field.id();
                let raw_val = state.draft_values.get(fid).cloned().unwrap_or_default();
                let req_mark = if field.is_required() { " *" } else { "" };
                let label_text = format!("{}{req_mark}", field.display_label());
                let field_el = match field {
                    UiFormField::Text {
                        placeholder,
                        secret,
                        ..
                    } => {
                        let is_secret = *secret == Some(true)
                            || VAULT_MASKED_FIELDS.contains(&fid);
                        let revealed = state.secret_revealed_fields.contains(fid);
                        let display_val = if raw_val.is_empty() {
                            placeholder
                                .clone()
                                .unwrap_or_else(|| if is_secret { "••••••••".to_string() } else { "Enter value…".to_string() })
                        } else if is_secret && !revealed {
                            "•".repeat(raw_val.chars().count().clamp(4, 16))
                        } else {
                            raw_val.clone()
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_size(px(12.0)).text_color(text_secondary).child(label_text))
                            .child(
                                div()
                                    .h(px(34.0))
                                    .px(px(10.0))
                                    .rounded(px(8.0))
                                    .bg(control)
                                    .border_1()
                                    .border_color(border_strong)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(if raw_val.is_empty() { text_muted } else { text_primary })
                                            .child(display_val),
                                    )
                                    .when(is_secret, |el| {
                                        el.child(
                                            div()
                                                .px(px(6.0))
                                                .py(px(2.0))
                                                .rounded(px(4.0))
                                                .bg(surface_raised)
                                                .text_size(px(10.0))
                                                .text_color(text_muted)
                                                .child(if revealed { "Hide" } else { "Secret" }),
                                        )
                                    }),
                            )
                    }
                    UiFormField::Textarea { placeholder, .. } => div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(div().text_size(px(12.0)).text_color(text_secondary).child(label_text))
                        .child(
                            div()
                                .min_h(px(64.0))
                                .p(px(10.0))
                                .rounded(px(8.0))
                                .bg(control)
                                .border_1()
                                .border_color(border_strong)
                                .text_size(px(13.0))
                                .text_color(if raw_val.is_empty() { text_muted } else { text_primary })
                                .child(if raw_val.is_empty() {
                                    placeholder.clone().unwrap_or_else(|| "Enter details…".to_string())
                                } else {
                                    raw_val
                                }),
                        ),
                    UiFormField::Segmented { options, .. } => {
                        let mut seg = div()
                            .p(px(3.0))
                            .rounded(px(8.0))
                            .bg(surface_raised)
                            .flex()
                            .gap(px(4.0));
                        for opt in options {
                            let active = raw_val == *opt;
                            seg = seg.child(
                                div()
                                    .flex_1()
                                    .h(px(26.0))
                                    .px(px(8.0))
                                    .rounded(px(6.0))
                                    .bg(if active { accent } else { rgba(0x00000000) })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.0))
                                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                    .text_color(if active { text_on_accent } else { text_secondary })
                                    .child(opt.clone()),
                            );
                        }
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_size(px(12.0)).text_color(text_secondary).child(label_text))
                            .child(seg)
                    }
                    UiFormField::Select { options, .. } => {
                        let chosen = if raw_val.is_empty() {
                            options.first().cloned().unwrap_or_else(|| "Select…".to_string())
                        } else {
                            raw_val
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_size(px(12.0)).text_color(text_secondary).child(label_text))
                            .child(
                                div()
                                    .h(px(34.0))
                                    .px(px(10.0))
                                    .rounded(px(8.0))
                                    .bg(control)
                                    .border_1()
                                    .border_color(border_strong)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(13.0))
                                    .text_color(text_primary)
                                    .child(chosen)
                                    .child(tg_icon_rgba(IconName::ChevronDown, 13.0, text_muted)),
                            )
                    }
                    UiFormField::Date { .. } => div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(div().text_size(px(12.0)).text_color(text_secondary).child(label_text))
                        .child(
                            div()
                                .h(px(34.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .bg(control)
                                .border_1()
                                .border_color(border_strong)
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(13.0))
                                .text_color(if raw_val.is_empty() { text_muted } else { text_primary })
                                .child(if raw_val.is_empty() {
                                    "YYYY-MM-DD".to_string()
                                } else {
                                    raw_val
                                })
                                .child(div().text_size(px(11.0)).text_color(text_muted).child("CAL")),
                        ),
                };
                form_col = form_col.child(field_el);
            }
            card.child(form_col).child(
                div()
                    .h(px(32.0))
                    .px(px(14.0))
                    .rounded(px(8.0))
                    .bg(accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_on_accent)
                    .child(submit.clone().unwrap_or_else(|| "Submit".to_string())),
            )
        }
    };

    if let Some(ref err_msg) = state.validation_error {
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(danger_soft)
                .text_size(px(12.0))
                .text_color(danger)
                .child(err_msg.clone()),
        );
    }

    if let Some(ref outcome) = state.block.state.outcome {
        card = card.child(
            div()
                .pt(px(4.0))
                .border_t_1()
                .border_color(border_strong)
                .text_size(px(12.0))
                .text_color(text_muted)
                .child(outcome.clone()),
        );
    }

    card
}

/// OpenBot runtime turn cards rendered inline in the conversation timeline (`OB-F15`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeTurnCard {
    ApprovalCard {
        request_id: String,
        tool_name: String,
        summary: String,
        command_preview: Option<String>,
        standing_allowed: bool,
        standing_label: Option<String>,
        decided: Option<String>,
    },
    MediaApprovalCard {
        request_id: String,
        media_kind: String,
        model: String,
        prompt: String,
        cost_credits: u64,
        threshold_credits: u64,
        wallet_balance: u64,
        outcome: Option<String>,
    },
    BrowserTakeoverPreview {
        tab_id: String,
        url: String,
        title: String,
        reason: String,
        active: bool,
    },
    BrowserSecretCard {
        tab_id: String,
        site: String,
        method: String,
        submission: String,
        masked_hint: String,
        approved: bool,
    },
    UsageLimitAskCard {
        provider: String,
        model: String,
        reset_hint: Option<String>,
        backup_provider: Option<String>,
        backup_model: Option<String>,
    },
    McpSignInChatCard {
        server_id: String,
        server_name: String,
        auth_url: Option<String>,
        status: String,
    },
    SkillSuggestionChatCard {
        name: String,
        slug: Option<String>,
        description: Option<String>,
        installed: bool,
    },
    RoutineChatCard {
        routine_id: String,
        name: String,
        schedule_summary: String,
        quiet_policy: Option<String>,
        next_run_at: Option<String>,
        active: bool,
    },
}

fn turn_card_shell(
    palette: &OpenBotPalette,
    icon: IconName,
    accent_color: Rgba,
    title: impl Into<String>,
    badge: impl Into<String>,
) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    div()
        .w_full()
        .max_w(px(520.0))
        .rounded(px(14.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .min_w(px(0.0))
                        .child(tg_icon_rgba(icon, 14.0, accent_color))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .truncate()
                                .child(title.into()),
                        ),
                )
                .child(
                    div()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded_full()
                        .bg(raised)
                        .text_size(px(11.0))
                        .text_color(accent_color)
                        .child(badge.into()),
                ),
        )
}

fn pill_btn(label: impl Into<String>, bg_c: Rgba, fg_c: Rgba) -> Div {
    div()
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(8.0))
        .bg(bg_c)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg_c)
        .child(label.into())
}

/// Renders any of the 8 OpenBot runtime turn cards (`ApprovalCard`, `MediaApprovalCard`,
/// `BrowserTakeoverPreview`, `BrowserSecretCard`, `UsageLimitAskCard`, `McpSignInChatCard`,
/// `SkillSuggestionChatCard`, `RoutineChatCard`).
pub fn render_runtime_turn_card(card: &RuntimeTurnCard, palette: &OpenBotPalette) -> Div {
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xFFFFFF);
    let warning = pal_rgba(palette, "--openbot-warning", 0xFF9412);
    let danger = pal_rgba(palette, "--openbot-danger", 0xFD2F3B);
    let success = pal_rgba(palette, "--openbot-success", 0x31CF76);

    match card {
        RuntimeTurnCard::ApprovalCard {
            request_id,
            tool_name,
            summary,
            command_preview,
            standing_allowed,
            standing_label,
            decided,
        } => {
            let badge = decided
                .clone()
                .unwrap_or_else(|| format!("Approval · {request_id}"));
            turn_card_shell(palette, IconName::Lock, warning, tool_name.clone(), badge)
                .child(div().text_size(px(13.0)).text_color(text_secondary).child(summary.clone()))
                .when_some(command_preview.clone(), |el, cmd| {
                    el.child(
                        div()
                            .p(px(8.0))
                            .rounded(px(8.0))
                            .bg(raised)
                            .text_size(px(12.0))
                            .text_color(text_primary)
                            .child(cmd),
                    )
                })
                .when(decided.is_none(), |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .child(pill_btn("Approve", accent, on_accent))
                            .when(*standing_allowed, |row| {
                                row.child(pill_btn(
                                    standing_label
                                        .clone()
                                        .unwrap_or_else(|| "Always allow".to_string()),
                                    raised,
                                    text_primary,
                                ))
                            })
                            .child(pill_btn("Deny", raised, danger)),
                    )
                })
        }
        RuntimeTurnCard::MediaApprovalCard {
            media_kind,
            model,
            prompt,
            cost_credits,
            threshold_credits,
            wallet_balance,
            outcome,
            ..
        } => {
            let short = *wallet_balance < *cost_credits;
            let badge = outcome.clone().unwrap_or_else(|| {
                if short {
                    format!("Short · {cost_credits} cr")
                } else {
                    format!("{cost_credits} credits")
                }
            });
            turn_card_shell(
                palette,
                IconName::WalletStars,
                if short { danger } else { warning },
                format!("Generate {media_kind} ({model})"),
                badge,
            )
            .child(div().text_size(px(12.0)).text_color(text_secondary).child(prompt.clone()))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(format!(
                        "Wallet: {wallet_balance} cr · Approval threshold: {threshold_credits} cr"
                    )),
            )
            .when(outcome.is_none(), |el| {
                el.child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .child(pill_btn(
                            if short { "Add credits" } else { "Approve spend" },
                            if short { warning } else { accent },
                            on_accent,
                        ))
                        .child(pill_btn("Decline", raised, text_secondary)),
                )
            })
        }
        RuntimeTurnCard::BrowserTakeoverPreview {
            tab_id,
            url,
            title,
            reason,
            active,
        } => turn_card_shell(
            palette,
            IconName::ComputerDesktop,
            if *active { warning } else { text_muted },
            format!("Browser Takeover · {title}"),
            if *active {
                format!("Live · {tab_id}")
            } else {
                "Completed".to_string()
            },
        )
        .child(div().text_size(px(12.0)).text_color(accent).truncate().child(url.clone()))
        .child(div().text_size(px(12.0)).text_color(text_secondary).child(reason.clone()))
        .when(*active, |el| {
            el.child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(pill_btn("Take control", accent, on_accent))
                    .child(pill_btn("Done", raised, success)),
            )
        }),
        RuntimeTurnCard::BrowserSecretCard {
            site,
            method,
            submission,
            masked_hint,
            approved,
            ..
        } => turn_card_shell(
            palette,
            IconName::Lock,
            if *approved { success } else { accent },
            format!("Browser Vault · {site}"),
            format!("{method} / {submission}"),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(text_secondary)
                .child(format!("Masked credential: {masked_hint}")),
        )
        .when(!approved, |el| {
            el.child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(pill_btn("Fill securely", accent, on_accent))
                    .child(pill_btn("Cancel", raised, text_secondary)),
            )
        }),
        RuntimeTurnCard::UsageLimitAskCard {
            provider,
            model,
            reset_hint,
            backup_provider,
            backup_model,
        } => turn_card_shell(
            palette,
            IconName::ClockSending,
            warning,
            format!("Usage limit reached · {provider}"),
            model.clone(),
        )
        .when_some(reset_hint.clone(), |el, hint| {
            el.child(div().text_size(px(12.0)).text_color(text_muted).child(hint))
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .when_some(backup_model.clone(), |row, bm| {
                    let bp = backup_provider.as_deref().unwrap_or("backup");
                    row.child(pill_btn(format!("Switch to {bp}/{bm}"), accent, on_accent))
                })
                .child(pill_btn("Wait for reset", raised, text_secondary)),
        ),
        RuntimeTurnCard::McpSignInChatCard {
            server_name,
            auth_url,
            status,
            ..
        } => turn_card_shell(
            palette,
            IconName::TerminalMcp,
            accent,
            format!("MCP Sign-In · {server_name}"),
            status.clone(),
        )
        .when_some(auth_url.clone(), |el, url| {
            el.child(div().text_size(px(12.0)).text_color(text_muted).truncate().child(url))
        })
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .child(pill_btn("Sign in", accent, on_accent))
                .child(pill_btn("Dismiss", raised, text_secondary)),
        ),
        RuntimeTurnCard::SkillSuggestionChatCard {
            name,
            slug,
            description,
            installed,
        } => turn_card_shell(
            palette,
            IconName::PremiumStar,
            if *installed { success } else { accent },
            format!("Suggested Skill · {name}"),
            if *installed {
                "Installed".to_string()
            } else {
                slug.clone().unwrap_or_else(|| "New skill".to_string())
            },
        )
        .when_some(description.clone(), |el, desc| {
            el.child(div().text_size(px(12.0)).text_color(text_secondary).child(desc))
        })
        .when(!installed, |el| {
            el.child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(pill_btn("Save skill", accent, on_accent))
                    .child(pill_btn("Ignore", raised, text_secondary)),
            )
        }),
        RuntimeTurnCard::RoutineChatCard {
            name,
            schedule_summary,
            quiet_policy,
            next_run_at,
            active,
            ..
        } => turn_card_shell(
            palette,
            IconName::ClockSending,
            if *active { success } else { text_muted },
            name.clone(),
            if *active { "Active" } else { "Paused" },
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(text_secondary)
                .child(schedule_summary.clone()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(text_muted)
                .child(format!(
                    "Quiet: {} · Next: {}",
                    quiet_policy.as_deref().unwrap_or("always"),
                    next_run_at.as_deref().unwrap_or("unscheduled"),
                )),
        ),
    }
}
