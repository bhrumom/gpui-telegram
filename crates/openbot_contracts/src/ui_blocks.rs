//! Interactive `ask_ui` block contracts (`confirm`, `quick_replies`, `choice`, `form`),
//! reserved ID guards, secret redaction tolerance, Vault autofill requests, and fallback prompt round-tripping.
//! Mirrors `packages/contracts/src/{ui-blocks,ipc-vault}.ts`.

use std::collections::{BTreeMap, HashSet};

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::conversation::{AgentPromptOption, AgentPromptQuestion, ConversationMessageSender};
use crate::limits::{
    INPUT_LIMITS, clip_utf16, is_bounded_string, is_calendar_date, is_identifier,
    is_iso_timestamp, non_null_option, utf16_len,
};

pub const UI_BLOCK_VERSION: u32 = 1;
pub const VAULT_TEXT_LIMIT: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiBlockLimits {
    pub spec_json: usize,
    pub response_json: usize,
    pub id: usize,
    pub title: usize,
    pub label: usize,
    pub meta: usize,
    pub text: usize,
    pub preview: usize,
    pub field_value: usize,
    pub outcome: usize,
    pub actions: usize,
    pub quick_replies: usize,
    pub choice_options: usize,
    pub confirm_fields: usize,
    pub form_fields: usize,
    pub select_options: usize,
    pub segmented_options: usize,
    pub hold_min_ms: u32,
    pub hold_max_ms: u32,
    pub timestamp: usize,
}

pub const UI_BLOCK_LIMITS: UiBlockLimits = UiBlockLimits {
    spec_json: 64_000,
    response_json: 16_000,
    id: 64,
    title: 200,
    label: 120,
    meta: 60,
    text: 2_000,
    preview: 8_000,
    field_value: 8_000,
    outcome: 300,
    actions: 4,
    quick_replies: 8,
    choice_options: 20,
    confirm_fields: 8,
    form_fields: 20,
    select_options: 50,
    segmented_options: 6,
    hold_min_ms: 400,
    hold_max_ms: 3_000,
    timestamp: 160,
};

pub const UI_BLOCK_SUBMIT_ACTION_ID: &str = "submit";
pub const UI_BLOCK_TEXT_ACTION_ID: &str = "_text";
pub const UI_CHOICE_VALUES_KEY: &str = "selected";
pub const UI_CONFIRM_ACTION_QUESTION_ID: &str = "action";
pub const UI_QUICK_REPLIES_QUESTION_ID: &str = "reply";
pub const UI_CHOICE_QUESTION_ID: &str = "choice";
pub const UI_CONFIRM_EDIT_ACTION_ID: &str = "edit_preview";
pub const UI_CONFIRM_PREVIEW_ANSWER_ID: &str = "_preview";
pub const UI_API_KEY_SCOPE_FIELD_ID: &str = "scope";
pub const UI_LOCATION_FIELD_ID: &str = "location";

const LEGACY_RESERVED_KEY_IDS: [&str; 4] =
    ["botId", "recipientBotId", "recipientBotIds", "senderBotId"];

pub fn is_ui_block_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= UI_BLOCK_LIMITS.id
        && !value.starts_with('_')
        && !LEGACY_RESERVED_KEY_IDS.contains(&value)
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-')
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiActionStyle {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAction {
    pub id: String,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub style: Option<UiActionStyle>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub confirm: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiConfirmField {
    Select {
        label: String,
        select: String,
        options: Vec<String>,
    },
    Value {
        label: String,
        value: String,
    },
}

impl Serialize for UiConfirmField {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Select {
                label,
                select,
                options,
            } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("label", label)?;
                map.serialize_entry("select", select)?;
                map.serialize_entry("options", options)?;
                map.end()
            }
            Self::Value { label, value } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("label", label)?;
                map.serialize_entry("value", value)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for UiConfirmField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = val.as_object() else {
            return Err(serde::de::Error::custom("UiConfirmField must be an object"));
        };
        let label = obj
            .get("label")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("label"))?
            .to_string();
        if let Some(select_val) = obj.get("select") {
            let select = select_val
                .as_str()
                .ok_or_else(|| serde::de::Error::custom("UiConfirmField.select must be a string"))?
                .to_string();
            let options_val = obj
                .get("options")
                .ok_or_else(|| serde::de::Error::missing_field("options"))?;
            let options: Vec<String> =
                serde_json::from_value(options_val.clone()).map_err(serde::de::Error::custom)?;
            return Ok(Self::Select {
                label,
                select,
                options,
            });
        }
        let value = obj
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("value"))?
            .to_string();
        Ok(Self::Value { label, value })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiChoiceOption {
    pub id: String,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub meta: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum UiFormField {
    Text {
        id: String,
        label: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        required: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        placeholder: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        value: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        secret: Option<bool>,
    },
    Textarea {
        id: String,
        label: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        required: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        placeholder: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        value: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        secret: Option<bool>,
    },
    Select {
        id: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        label: Option<String>,
        options: Vec<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        required: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        value: Option<String>,
    },
    Segmented {
        id: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        label: Option<String>,
        options: Vec<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        required: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        value: Option<String>,
    },
    Date {
        id: String,
        label: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        required: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        value: Option<String>,
    },
}

fn non_blank_bounded(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && is_bounded_string(value, max)
}

fn unique_exact<'a, I: IntoIterator<Item = &'a str>>(items: I) -> bool {
    let mut seen = HashSet::new();
    items.into_iter().all(|s| seen.insert(s))
}

fn unique_case_insensitive<'a, I: IntoIterator<Item = &'a str>>(items: I) -> bool {
    let mut seen = HashSet::new();
    items.into_iter().all(|s| seen.insert(s.trim().to_lowercase()))
}

fn valid_string_options(options: &[String], max: usize) -> bool {
    !options.is_empty()
        && options.len() <= max
        && options.iter().all(|o| non_blank_bounded(o, UI_BLOCK_LIMITS.label))
        && unique_case_insensitive(options.iter().map(String::as_str))
}

impl UiFormField {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. }
            | Self::Textarea { id, .. }
            | Self::Select { id, .. }
            | Self::Segmented { id, .. }
            | Self::Date { id, .. } => id,
        }
    }

    pub fn display_label(&self) -> &str {
        match self {
            Self::Text { label, .. }
            | Self::Textarea { label, .. }
            | Self::Date { label, .. } => label,
            Self::Select { id, label, .. } | Self::Segmented { id, label, .. } => {
                label.as_deref().unwrap_or(id.as_str())
            }
        }
    }

    pub fn is_secret(&self) -> bool {
        matches!(self, Self::Text { secret: Some(true), .. })
    }

    pub fn is_required(&self) -> bool {
        match self {
            Self::Text { required, .. }
            | Self::Textarea { required, .. }
            | Self::Select { required, .. }
            | Self::Segmented { required, .. }
            | Self::Date { required, .. } => *required == Some(true),
        }
    }

    pub fn is_valid(&self) -> bool {
        if !is_ui_block_id(self.id()) {
            return false;
        }
        match self {
            Self::Text {
                label,
                placeholder,
                value,
                secret,
                ..
            } => {
                non_blank_bounded(label, UI_BLOCK_LIMITS.label)
                    && placeholder
                        .as_deref()
                        .is_none_or(|p| non_blank_bounded(p, UI_BLOCK_LIMITS.label))
                    && value
                        .as_deref()
                        .is_none_or(|v| is_bounded_string(v, UI_BLOCK_LIMITS.field_value))
                    && (*secret != Some(true) || value.is_none())
            }
            Self::Textarea {
                label,
                placeholder,
                value,
                secret,
                ..
            } => {
                non_blank_bounded(label, UI_BLOCK_LIMITS.label)
                    && placeholder
                        .as_deref()
                        .is_none_or(|p| non_blank_bounded(p, UI_BLOCK_LIMITS.label))
                    && value
                        .as_deref()
                        .is_none_or(|v| is_bounded_string(v, UI_BLOCK_LIMITS.field_value))
                    && *secret != Some(true)
            }
            Self::Select { label, options, value, .. } => {
                label
                    .as_deref()
                    .is_none_or(|l| non_blank_bounded(l, UI_BLOCK_LIMITS.label))
                    && valid_string_options(options, UI_BLOCK_LIMITS.select_options)
                    && value.as_ref().is_none_or(|v| options.contains(v))
            }
            Self::Segmented { label, options, value, .. } => {
                label
                    .as_deref()
                    .is_none_or(|l| non_blank_bounded(l, UI_BLOCK_LIMITS.label))
                    && valid_string_options(options, UI_BLOCK_LIMITS.segmented_options)
                    && value.as_ref().is_none_or(|v| options.contains(v))
            }
            Self::Date { label, value, .. } => {
                non_blank_bounded(label, UI_BLOCK_LIMITS.label)
                    && value.as_deref().is_none_or(is_calendar_date)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiBlockSpec {
    Confirm {
        title: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        danger: Option<bool>,
        #[serde(
            rename = "confirmHold",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        confirm_hold: Option<u32>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        fields: Option<Vec<UiConfirmField>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        preview: Option<String>,
        actions: Vec<UiAction>,
    },
    QuickReplies {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        title: Option<String>,
        options: Vec<UiOption>,
        #[serde(
            rename = "allowText",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        allow_text: Option<bool>,
    },
    Choice {
        title: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        multiple: Option<bool>,
        options: Vec<UiChoiceOption>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        submit: Option<String>,
    },
    Form {
        title: String,
        fields: Vec<UiFormField>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        submit: Option<String>,
    },
}

impl UiBlockSpec {
    pub fn validate(&self) -> bool {
        let Ok(encoded) = serde_json::to_string(self) else {
            return false;
        };
        if utf16_len(&encoded) > UI_BLOCK_LIMITS.spec_json {
            return false;
        }
        match self {
            Self::Confirm {
                title,
                confirm_hold,
                fields,
                preview,
                actions,
                ..
            } => {
                let actions_ok = (1..=UI_BLOCK_LIMITS.actions).contains(&actions.len())
                    && actions.iter().all(|a| {
                        is_ui_block_id(&a.id) && non_blank_bounded(&a.label, UI_BLOCK_LIMITS.label)
                    })
                    && unique_exact(actions.iter().map(|a| a.id.as_str()))
                    && unique_case_insensitive(actions.iter().map(|a| a.label.as_str()));
                let fields_ok = fields.as_ref().is_none_or(|f_list| {
                    f_list.len() <= UI_BLOCK_LIMITS.confirm_fields
                        && f_list.iter().all(|f| match f {
                            UiConfirmField::Value { label, value } => {
                                non_blank_bounded(label, UI_BLOCK_LIMITS.label)
                                    && is_bounded_string(value, UI_BLOCK_LIMITS.text)
                            }
                            UiConfirmField::Select { label, select, options } => {
                                non_blank_bounded(label, UI_BLOCK_LIMITS.label)
                                    && is_ui_block_id(select)
                                    && select != UI_CONFIRM_ACTION_QUESTION_ID
                                    && valid_string_options(options, UI_BLOCK_LIMITS.select_options)
                            }
                        })
                        && unique_exact(f_list.iter().filter_map(|f| match f {
                            UiConfirmField::Select { select, .. } => Some(select.as_str()),
                            UiConfirmField::Value { .. } => None,
                        }))
                });
                non_blank_bounded(title, UI_BLOCK_LIMITS.title)
                    && confirm_hold.is_none_or(|ms| {
                        (UI_BLOCK_LIMITS.hold_min_ms..=UI_BLOCK_LIMITS.hold_max_ms).contains(&ms)
                    })
                    && preview
                        .as_deref()
                        .is_none_or(|p| non_blank_bounded(p, UI_BLOCK_LIMITS.preview))
                    && actions_ok
                    && fields_ok
            }
            Self::QuickReplies { title, options, .. } => {
                title
                    .as_deref()
                    .is_none_or(|t| non_blank_bounded(t, UI_BLOCK_LIMITS.title))
                    && (1..=UI_BLOCK_LIMITS.quick_replies).contains(&options.len())
                    && options
                        .iter()
                        .all(|o| is_ui_block_id(&o.id) && non_blank_bounded(&o.label, UI_BLOCK_LIMITS.label))
                    && unique_exact(options.iter().map(|o| o.id.as_str()))
                    && unique_case_insensitive(options.iter().map(|o| o.label.as_str()))
            }
            Self::Choice {
                title,
                multiple,
                options,
                submit,
            } => {
                non_blank_bounded(title, UI_BLOCK_LIMITS.title)
                    && (1..=UI_BLOCK_LIMITS.choice_options).contains(&options.len())
                    && options.iter().all(|o| {
                        is_ui_block_id(&o.id)
                            && non_blank_bounded(&o.label, UI_BLOCK_LIMITS.label)
                            && o.meta
                                .as_deref()
                                .is_none_or(|m| non_blank_bounded(m, UI_BLOCK_LIMITS.meta))
                    })
                    && unique_exact(options.iter().map(|o| o.id.as_str()))
                    && unique_case_insensitive(options.iter().map(|o| o.label.as_str()))
                    && (*multiple == Some(true)
                        || options.iter().filter(|o| o.selected == Some(true)).count() <= 1)
                    && submit
                        .as_deref()
                        .is_none_or(|s| non_blank_bounded(s, UI_BLOCK_LIMITS.label))
            }
            Self::Form { title, fields, submit } => {
                non_blank_bounded(title, UI_BLOCK_LIMITS.title)
                    && (1..=UI_BLOCK_LIMITS.form_fields).contains(&fields.len())
                    && fields.iter().all(UiFormField::is_valid)
                    && unique_exact(fields.iter().map(UiFormField::id))
                    && submit
                        .as_deref()
                        .is_none_or(|s| non_blank_bounded(s, UI_BLOCK_LIMITS.label))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UiBlockValue {
    Text(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiBlockResponse {
    pub action_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub values: Option<BTreeMap<String, UiBlockValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiBlockStatus {
    Pending,
    Answered,
    Expired,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiBlockAutoAnswer {
    pub action_id: String,
    pub at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub done: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiBlockState {
    pub status: UiBlockStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub response: Option<UiBlockResponse>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub responded_by: Option<ConversationMessageSender>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub responded_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub outcome: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub auto_answer: Option<UiBlockAutoAnswer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultCardBrand {
    Visa,
    Mastercard,
    Amex,
    Discover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiVaultRequestKind {
    Login,
    Card,
    Contact,
}

pub const UI_VAULT_LOGIN_FIELDS: [&str; 3] = ["website", "username", "password"];
pub const UI_VAULT_CARD_FIELDS: [&str; 5] =
    ["label", "nameOnCard", "number", "expiry", "securityCode"];
pub const UI_VAULT_CONTACT_FIELDS: [&str; 10] = [
    "label", "fullName", "address", "address2", "city", "region", "postalCode", "country",
    "phone", "email",
];
pub const UI_VAULT_SECRET_FIELDS: [&str; 3] = ["password", "number", "securityCode"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiVaultSavedItem {
    pub id: String,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub brand: Option<VaultCardBrand>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub last4: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiBlockVaultRequest {
    pub kind: UiVaultRequestKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub saved: Option<UiVaultSavedItem>,
}

impl UiBlockVaultRequest {
    pub fn is_valid_for(&self, spec: &UiBlockSpec) -> bool {
        if !matches!(spec, UiBlockSpec::Form { .. }) {
            return false;
        }
        self.saved.as_ref().is_none_or(|s| {
            is_identifier(&s.id)
                && is_bounded_string(&s.name, VAULT_TEXT_LIMIT)
                && s.last4
                    .as_deref()
                    .is_none_or(|l| l.len() == 4 && l.bytes().all(|b| b.is_ascii_digit()))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationUiBlock {
    pub version: u32,
    pub block_id: String,
    pub spec: UiBlockSpec,
    pub state: UiBlockState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub vault: Option<UiBlockVaultRequest>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub api_key: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub location: Option<bool>,
}

pub fn validate_ui_block_response(
    spec: &UiBlockSpec,
    response: &UiBlockResponse,
    stored: bool,
) -> Option<UiBlockResponse> {
    let encoded = serde_json::to_string(response).ok()?;
    if utf16_len(&encoded) > UI_BLOCK_LIMITS.response_json {
        return None;
    }
    let action_id = response.action_id.as_str();
    if action_id == UI_BLOCK_TEXT_ACTION_ID {
        if response.values.is_some() {
            return None;
        }
        let text = response.text.as_deref()?;
        if !non_blank_bounded(text, UI_BLOCK_LIMITS.field_value) {
            return None;
        }
        return Some(UiBlockResponse {
            action_id: response.action_id.clone(),
            values: None,
            text: Some(text.to_string()),
        });
    }
    if response.text.is_some() {
        return None;
    }
    if let Some(raw_vals) = &response.values
        && !raw_vals.keys().all(|k| is_ui_block_id(k))
    {
        return None;
    }
    match spec {
        UiBlockSpec::QuickReplies { options, .. } => {
            if !options.iter().any(|o| o.id == action_id) || response.values.is_some() {
                return None;
            }
            Some(UiBlockResponse {
                action_id: response.action_id.clone(),
                values: None,
                text: None,
            })
        }
        UiBlockSpec::Confirm { fields, actions, .. } => {
            if !actions.iter().any(|a| a.id == action_id) {
                return None;
            }
            let Some(raw) = &response.values else {
                return Some(UiBlockResponse {
                    action_id: response.action_id.clone(),
                    values: None,
                    text: None,
                });
            };
            let selects: BTreeMap<&str, &[String]> = fields
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter_map(|f| match f {
                    UiConfirmField::Select { select, options, .. } => {
                        Some((select.as_str(), options.as_slice()))
                    }
                    UiConfirmField::Value { .. } => None,
                })
                .collect();
            if raw.len() > selects.len() {
                return None;
            }
            let mut normalized = BTreeMap::new();
            for (k, v) in raw {
                let allowed = selects.get(k.as_str())?;
                let UiBlockValue::Text(s) = v else {
                    return None;
                };
                if !allowed.contains(s) {
                    return None;
                }
                normalized.insert(k.clone(), UiBlockValue::Text(s.clone()));
            }
            Some(UiBlockResponse {
                action_id: response.action_id.clone(),
                values: Some(normalized),
                text: None,
            })
        }
        UiBlockSpec::Choice { multiple, options, .. } => {
            if action_id != UI_BLOCK_SUBMIT_ACTION_ID {
                return None;
            }
            let raw = response.values.as_ref()?;
            if raw.len() > 1 {
                return None;
            }
            let UiBlockValue::List(chosen) = raw.get(UI_CHOICE_VALUES_KEY)? else {
                return None;
            };
            let max_sel = if *multiple == Some(true) { options.len() } else { 1 };
            if chosen.is_empty()
                || chosen.len() > max_sel
                || !unique_exact(chosen.iter().map(String::as_str))
                || !chosen.iter().all(|id| options.iter().any(|o| &o.id == id))
            {
                return None;
            }
            let mut normalized = BTreeMap::new();
            normalized.insert(
                UI_CHOICE_VALUES_KEY.to_string(),
                UiBlockValue::List(chosen.clone()),
            );
            Some(UiBlockResponse {
                action_id: response.action_id.clone(),
                values: Some(normalized),
                text: None,
            })
        }
        UiBlockSpec::Form { fields, .. } => validate_form_response(fields, response, stored),
    }
}

fn validate_form_response(
    fields: &[UiFormField],
    response: &UiBlockResponse,
    stored: bool,
) -> Option<UiBlockResponse> {
    if response.action_id != UI_BLOCK_SUBMIT_ACTION_ID {
        return None;
    }
    let empty_map = BTreeMap::new();
    let raw = response.values.as_ref().unwrap_or(&empty_map);
    if raw.len() > fields.len() {
        return None;
    }
    let field_ids: HashSet<&str> = fields.iter().map(UiFormField::id).collect();
    if !raw.keys().all(|k| field_ids.contains(k.as_str())) {
        return None;
    }
    let mut normalized = BTreeMap::new();
    for field in fields {
        let secret = field.is_secret();
        let entry = if stored && secret {
            None
        } else {
            raw.get(field.id())
        };
        match entry {
            None => {
                if field.is_required() && !(stored && secret) {
                    return None;
                }
            }
            Some(UiBlockValue::Text(s)) if s.is_empty() => {
                if field.is_required() && !(stored && secret) {
                    return None;
                }
                normalized.insert(field.id().to_string(), UiBlockValue::Text(String::new()));
            }
            Some(UiBlockValue::Text(s)) => {
                let valid = match field {
                    UiFormField::Select { options, .. } | UiFormField::Segmented { options, .. } => {
                        options.contains(s)
                    }
                    UiFormField::Date { .. } => is_calendar_date(s),
                    UiFormField::Text { .. } | UiFormField::Textarea { .. } => {
                        is_bounded_string(s, UI_BLOCK_LIMITS.field_value)
                    }
                };
                if !valid {
                    return None;
                }
                normalized.insert(field.id().to_string(), UiBlockValue::Text(s.clone()));
            }
            Some(UiBlockValue::List(_)) => return None,
        }
    }
    Some(UiBlockResponse {
        action_id: response.action_id.clone(),
        values: Some(normalized),
        text: None,
    })
}

impl ConversationUiBlock {
    pub fn validate(&self) -> bool {
        if self.version != UI_BLOCK_VERSION || !is_ui_block_id(&self.block_id) || !self.spec.validate() {
            return false;
        }
        let state_resp_ok = match &self.state.response {
            Some(r) => validate_ui_block_response(&self.spec, r, true).is_some(),
            None => self.state.status != UiBlockStatus::Answered,
        };
        let sender_ok = self
            .state
            .responded_by
            .as_ref()
            .is_none_or(ConversationMessageSender::is_valid);
        let at_ok = self
            .state
            .responded_at
            .as_deref()
            .is_none_or(|t| non_blank_bounded(t, UI_BLOCK_LIMITS.timestamp));
        let outcome_ok = self
            .state
            .outcome
            .as_deref()
            .is_none_or(|o| non_blank_bounded(o, UI_BLOCK_LIMITS.outcome));
        let auto_ok = self.state.auto_answer.as_ref().is_none_or(|aa| {
            is_ui_block_id(&aa.action_id)
                && non_blank_bounded(&aa.at, UI_BLOCK_LIMITS.timestamp)
                && is_iso_timestamp(&aa.at)
                && matches!(&self.spec, UiBlockSpec::Confirm { actions, .. } if actions.iter().any(|a| a.id == aa.action_id))
        });
        let is_form = matches!(self.spec, UiBlockSpec::Form { .. });
        let vault_ok = self.vault.as_ref().is_none_or(|v| v.is_valid_for(&self.spec));
        let flags_ok = self.api_key.is_none_or(|v| v && is_form)
            && self.location.is_none_or(|v| v && is_form);
        state_resp_ok && sender_ok && at_ok && outcome_ok && auto_ok && vault_ok && flags_ok
    }
}

pub fn ui_block_action_is_privileged(spec: &UiBlockSpec, action_id: &str) -> bool {
    let UiBlockSpec::Confirm { danger, actions, .. } = spec else {
        return false;
    };
    let Some(action) = actions.iter().find(|a| a.id == action_id) else {
        return false;
    };
    action.style == Some(UiActionStyle::Danger)
        || action.confirm == Some(true)
        || (*danger == Some(true) && action.style == Some(UiActionStyle::Primary))
}

pub fn ui_block_has_privileged_action(spec: &UiBlockSpec) -> bool {
    match spec {
        UiBlockSpec::Confirm { actions, .. } => actions
            .iter()
            .any(|a| ui_block_action_is_privileged(spec, &a.id)),
        _ => false,
    }
}

pub fn ui_confirm_preview_editable(spec: &UiBlockSpec) -> bool {
    matches!(
        spec,
        UiBlockSpec::Confirm {
            preview: Some(_),
            actions,
            ..
        } if actions.iter().any(|a| a.id == UI_CONFIRM_EDIT_ACTION_ID)
    )
}

pub fn ui_confirm_preview_edit(
    spec: &UiBlockSpec,
    answers: &BTreeMap<String, Vec<String>>,
) -> Option<(BTreeMap<String, Vec<String>>, Option<String>)> {
    let mut rest = answers.clone();
    let Some(edit) = rest.remove(UI_CONFIRM_PREVIEW_ANSWER_ID) else {
        return Some((rest, None));
    };
    let UiBlockSpec::Confirm { preview, .. } = spec else {
        return None;
    };
    if !ui_confirm_preview_editable(spec) || edit.len() != 1 {
        return None;
    }
    let text = &edit[0];
    if !non_blank_bounded(text, UI_BLOCK_LIMITS.preview) {
        return None;
    }
    if preview.as_deref() == Some(text.as_str()) {
        Some((rest, None))
    } else {
        Some((rest, Some(text.clone())))
    }
}

fn build_prompt_question(
    id: &str,
    header: &str,
    body: &str,
    options: Option<Vec<(&str, &str)>>,
    is_secret: bool,
) -> AgentPromptQuestion {
    AgentPromptQuestion {
        id: id.to_string(),
        header: clip_utf16(header, INPUT_LIMITS.prompt_header),
        question: clip_utf16(body, INPUT_LIMITS.prompt_question),
        detail: None,
        is_secret,
        options: options.map(|opts| {
            opts.into_iter()
                .map(|(l, d)| AgentPromptOption {
                    label: clip_utf16(l, INPUT_LIMITS.prompt_option_label),
                    description: clip_utf16(d, INPUT_LIMITS.prompt_option_description),
                })
                .collect()
        }),
        multi_select: None,
    }
}

fn fitting_options(labels: &[String]) -> Option<Vec<(&str, &str)>> {
    (labels.len() <= INPUT_LIMITS.prompt_options)
        .then(|| labels.iter().map(|l| (l.as_str(), "")).collect())
}

fn bullet_list(labels: &[String]) -> String {
    labels.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
}

pub fn ui_block_fallback_questions(spec: &UiBlockSpec) -> Vec<AgentPromptQuestion> {
    match spec {
        UiBlockSpec::Confirm {
            title,
            fields,
            preview,
            actions,
            ..
        } => {
            let details: Vec<String> = fields
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|f| match f {
                    UiConfirmField::Select { label, options, .. } => {
                        format!("{label}: {}", options.first().map_or("", String::as_str))
                    }
                    UiConfirmField::Value { label, value } => format!("{label}: {value}"),
                })
                .collect();
            let details_joined = details.join("\n");
            let parts: Vec<&str> = [title.as_str(), details_joined.as_str(), preview.as_deref().unwrap_or("")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            let body = parts.join("\n\n");
            let action_opts = actions.iter().map(|a| (a.label.as_str(), "")).collect();
            let mut questions = vec![build_prompt_question(
                UI_CONFIRM_ACTION_QUESTION_ID,
                title,
                &body,
                Some(action_opts),
                false,
            )];
            for f in fields.as_deref().unwrap_or(&[]) {
                if let UiConfirmField::Select { label, select, options } = f {
                    let opts = fitting_options(options);
                    let q_body = if opts.is_some() {
                        label.clone()
                    } else {
                        format!("{label}\n\n{}", bullet_list(options))
                    };
                    questions.push(build_prompt_question(select, label, &q_body, opts, false));
                }
            }
            questions
        }
        UiBlockSpec::QuickReplies { title, options, .. } => {
            let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
            let opts = fitting_options(&labels);
            let hdr = title.clone().unwrap_or_else(|| labels.join(" / "));
            let body = if opts.is_some() {
                hdr.clone()
            } else {
                format!("{hdr}\n\n{}", bullet_list(&labels))
            };
            vec![build_prompt_question(
                UI_QUICK_REPLIES_QUESTION_ID,
                &hdr,
                &body,
                opts,
                false,
            )]
        }
        UiBlockSpec::Choice {
            title,
            multiple,
            options,
            ..
        } => {
            if *multiple != Some(true) && options.len() <= INPUT_LIMITS.prompt_options {
                let opts = options
                    .iter()
                    .map(|o| (o.label.as_str(), o.meta.as_deref().unwrap_or("")))
                    .collect();
                return vec![build_prompt_question(
                    UI_CHOICE_QUESTION_ID,
                    title,
                    title,
                    Some(opts),
                    false,
                )];
            }
            let labels: Vec<String> = options
                .iter()
                .map(|o| match &o.meta {
                    Some(m) => format!("{} ({m})", o.label),
                    None => o.label.clone(),
                })
                .collect();
            let body = format!("{title}\n\n{}", bullet_list(&labels));
            vec![build_prompt_question(UI_CHOICE_QUESTION_ID, title, &body, None, false)]
        }
        UiBlockSpec::Form { fields, .. } => fields
            .iter()
            .map(|field| {
                let label = field.display_label();
                match field {
                    UiFormField::Select { options, .. } | UiFormField::Segmented { options, .. } => {
                        let opts = fitting_options(options);
                        let body = if opts.is_some() {
                            label.to_string()
                        } else {
                            format!("{label}\n\n{}", bullet_list(options))
                        };
                        build_prompt_question(field.id(), label, &body, opts, false)
                    }
                    UiFormField::Date { .. } => build_prompt_question(
                        field.id(),
                        label,
                        &format!("{label} (YYYY-MM-DD)"),
                        None,
                        false,
                    ),
                    UiFormField::Text { .. } | UiFormField::Textarea { .. } => {
                        build_prompt_question(field.id(), label, label, None, field.is_secret())
                    }
                }
            })
            .collect(),
    }
}

fn match_id_label<'a>(items: &'a [(&'a str, &'a str)], answer: &str) -> Option<&'a str> {
    let trimmed = answer.trim();
    let lower = trimmed.to_lowercase();
    items
        .iter()
        .find(|(_, label)| *label == trimmed)
        .or_else(|| items.iter().find(|(_, label)| label.trim().to_lowercase() == lower))
        .or_else(|| items.iter().find(|(id, _)| *id == trimmed))
        .map(|(id, _)| *id)
}

fn match_string_option<'a>(options: &'a [String], answer: &str) -> Option<&'a str> {
    let lower = answer.trim().to_lowercase();
    options
        .iter()
        .find(|o| o.trim().to_lowercase() == lower)
        .map(String::as_str)
}

pub fn ui_block_response_from_answers(
    spec: &UiBlockSpec,
    answers: &BTreeMap<String, Vec<String>>,
) -> Option<UiBlockResponse> {
    let questions = ui_block_fallback_questions(spec);
    let text_lines: Vec<String> = questions
        .iter()
        .filter(|q| !q.is_secret)
        .filter_map(|q| {
            let given: Vec<&str> = answers
                .get(&q.id)?
                .iter()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty())
                .collect();
            (!given.is_empty()).then(|| format!("{}: {}", q.header, given.join(", ")))
        })
        .collect();
    let text = text_lines.join("\n");
    let has_secret_answer = questions.iter().any(|q| {
        q.is_secret
            && answers
                .get(&q.id)
                .is_some_and(|list| list.iter().any(|a| !a.trim().is_empty()))
    });
    if text.is_empty() && !has_secret_answer {
        return None;
    }

    let first_non_blank = |id: &str| -> Option<&str> {
        answers
            .get(id)?
            .iter()
            .map(String::as_str)
            .find(|s| !s.trim().is_empty())
    };

    let structured: Option<UiBlockResponse> = match spec {
        UiBlockSpec::Confirm { fields, actions, .. } => (|| {
            let pairs: Vec<(&str, &str)> =
                actions.iter().map(|a| (a.id.as_str(), a.label.as_str())).collect();
            let action_id = match_id_label(&pairs, first_non_blank(UI_CONFIRM_ACTION_QUESTION_ID)?)?;
            let mut vals = BTreeMap::new();
            for f in fields.as_deref().unwrap_or(&[]) {
                if let UiConfirmField::Select { select, options, .. } = f
                    && let Some(given) = first_non_blank(select)
                {
                    let matched = match_string_option(options, given)?;
                    vals.insert(select.clone(), UiBlockValue::Text(matched.to_string()));
                }
            }
            Some(UiBlockResponse {
                action_id: action_id.to_string(),
                values: (!vals.is_empty()).then_some(vals),
                text: None,
            })
        })(),
        UiBlockSpec::QuickReplies { options, .. } => (|| {
            let pairs: Vec<(&str, &str)> =
                options.iter().map(|o| (o.id.as_str(), o.label.as_str())).collect();
            let action_id = match_id_label(&pairs, first_non_blank(UI_QUICK_REPLIES_QUESTION_ID)?)?;
            Some(UiBlockResponse {
                action_id: action_id.to_string(),
                values: None,
                text: None,
            })
        })(),
        UiBlockSpec::Choice { options, .. } => (|| {
            let pairs: Vec<(&str, &str)> =
                options.iter().map(|o| (o.id.as_str(), o.label.as_str())).collect();
            let mut ids: Vec<String> = Vec::new();
            for raw in answers.get(UI_CHOICE_QUESTION_ID).map_or(&[][..], Vec::as_slice) {
                if raw.trim().is_empty() {
                    continue;
                }
                if let Some(whole) = match_id_label(&pairs, raw) {
                    if !ids.iter().any(|x| x == whole) {
                        ids.push(whole.to_string());
                    }
                    continue;
                }
                for line in raw.split('\n').filter(|l| !l.trim().is_empty()) {
                    if let Some(m) = match_id_label(&pairs, line) {
                        if !ids.iter().any(|x| x == m) {
                            ids.push(m.to_string());
                        }
                    } else {
                        for tok in line.split([',', ';']).filter(|t| !t.trim().is_empty()) {
                            let part = match_id_label(&pairs, tok)?;
                            if !ids.iter().any(|x| x == part) {
                                ids.push(part.to_string());
                            }
                        }
                    }
                }
            }
            let mut vals = BTreeMap::new();
            vals.insert(UI_CHOICE_VALUES_KEY.to_string(), UiBlockValue::List(ids));
            Some(UiBlockResponse {
                action_id: UI_BLOCK_SUBMIT_ACTION_ID.to_string(),
                values: Some(vals),
                text: None,
            })
        })(),
        UiBlockSpec::Form { fields, .. } => (|| {
            let mut vals = BTreeMap::new();
            for field in fields {
                let Some(given) = first_non_blank(field.id()) else {
                    continue;
                };
                let normalized = match field {
                    UiFormField::Select { options, .. } | UiFormField::Segmented { options, .. } => {
                        match_string_option(options, given)?.to_string()
                    }
                    UiFormField::Date { .. } => given.trim().to_string(),
                    UiFormField::Text { .. } | UiFormField::Textarea { .. } => given.to_string(),
                };
                vals.insert(field.id().to_string(), UiBlockValue::Text(normalized));
            }
            Some(UiBlockResponse {
                action_id: UI_BLOCK_SUBMIT_ACTION_ID.to_string(),
                values: Some(vals),
                text: None,
            })
        })(),
    };

    if let Some(cand) = structured
        && let Some(valid) = validate_ui_block_response(spec, &cand, false)
    {
        return Some(valid);
    }

    let single = if questions.len() == 1 && !questions[0].is_secret {
        answers
            .get(&questions[0].id)
            .map(|v| v.join("\n").trim().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let fallback_text = if single.is_empty() { text } else { single };
    Some(UiBlockResponse {
        action_id: UI_BLOCK_TEXT_ACTION_ID.to_string(),
        values: None,
        text: Some(clip_utf16(&fallback_text, UI_BLOCK_LIMITS.field_value)),
    })
}

pub fn ui_block_answers_from_response(
    spec: &UiBlockSpec,
    response: &UiBlockResponse,
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if response.action_id == UI_BLOCK_TEXT_ACTION_ID {
        if let Some(first) = ui_block_fallback_questions(spec).first() {
            out.insert(
                first.id.clone(),
                vec![response.text.clone().unwrap_or_default()],
            );
        }
        return out;
    }
    let empty_vals = BTreeMap::new();
    let vals = response.values.as_ref().unwrap_or(&empty_vals);
    match spec {
        UiBlockSpec::Confirm { actions, .. } => {
            let label = actions
                .iter()
                .find(|a| a.id == response.action_id)
                .map_or(response.action_id.as_str(), |a| a.label.as_str());
            out.insert(UI_CONFIRM_ACTION_QUESTION_ID.to_string(), vec![label.to_string()]);
            for (k, v) in vals {
                if let UiBlockValue::Text(s) = v {
                    out.insert(k.clone(), vec![s.clone()]);
                }
            }
        }
        UiBlockSpec::QuickReplies { options, .. } => {
            let label = options
                .iter()
                .find(|o| o.id == response.action_id)
                .map_or(response.action_id.as_str(), |o| o.label.as_str());
            out.insert(UI_QUICK_REPLIES_QUESTION_ID.to_string(), vec![label.to_string()]);
        }
        UiBlockSpec::Choice { multiple, options, .. } => {
            let ids = match vals.get(UI_CHOICE_VALUES_KEY) {
                Some(UiBlockValue::List(list)) => list.as_slice(),
                _ => &[],
            };
            let labels: Vec<String> = ids
                .iter()
                .map(|id| {
                    options
                        .iter()
                        .find(|o| &o.id == id)
                        .map_or_else(|| id.clone(), |o| o.label.clone())
                })
                .collect();
            let entry = if *multiple == Some(true) {
                vec![labels.join("\n")]
            } else {
                labels
            };
            out.insert(UI_CHOICE_QUESTION_ID.to_string(), entry);
        }
        UiBlockSpec::Form { fields, .. } => {
            for f in fields {
                if let Some(UiBlockValue::Text(s)) = vals.get(f.id())
                    && !s.is_empty()
                {
                    out.insert(f.id().to_string(), vec![s.clone()]);
                }
            }
        }
    }
    out
}

pub fn ui_confirm_answers_with_preview(
    spec: &UiBlockSpec,
    response: &UiBlockResponse,
    preview: Option<&str>,
) -> BTreeMap<String, Vec<String>> {
    let mut answers = ui_block_answers_from_response(spec, response);
    if let (Some(edited), UiBlockSpec::Confirm { preview: orig, .. }) = (preview, spec)
        && orig.as_deref() != Some(edited)
    {
        answers.insert(
            UI_CONFIRM_PREVIEW_ANSWER_ID.to_string(),
            vec![edited.to_string()],
        );
    }
    answers
}

pub fn ui_block_outcome_text(spec: &UiBlockSpec, response: &UiBlockResponse) -> String {
    if response.action_id == UI_BLOCK_TEXT_ACTION_ID {
        return clip_utf16(
            response.text.as_deref().unwrap_or(""),
            UI_BLOCK_LIMITS.outcome,
        );
    }
    let empty_vals = BTreeMap::new();
    let vals = response.values.as_ref().unwrap_or(&empty_vals);
    let raw = match spec {
        UiBlockSpec::QuickReplies { options, .. } => options
            .iter()
            .find(|o| o.id == response.action_id)
            .map_or_else(|| response.action_id.clone(), |o| o.label.clone()),
        UiBlockSpec::Choice { options, .. } => {
            let ids = match vals.get(UI_CHOICE_VALUES_KEY) {
                Some(UiBlockValue::List(l)) => l.as_slice(),
                _ => &[],
            };
            ids.iter()
                .map(|id| {
                    options
                        .iter()
                        .find(|o| &o.id == id)
                        .map_or(id.as_str(), |o| o.label.as_str())
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        UiBlockSpec::Form { fields, .. } => fields
            .iter()
            .filter(|f| !f.is_secret())
            .filter_map(|f| match vals.get(f.id()) {
                Some(UiBlockValue::Text(s)) if !s.is_empty() => {
                    Some(format!("{}: {s}", f.display_label()))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", "),
        UiBlockSpec::Confirm { actions, .. } => {
            let label = actions
                .iter()
                .find(|a| a.id == response.action_id)
                .map_or(response.action_id.as_str(), |a| a.label.as_str());
            let details: Vec<String> = vals
                .values()
                .map(|v| match v {
                    UiBlockValue::Text(s) => s.clone(),
                    UiBlockValue::List(l) => l.join(", "),
                })
                .collect();
            let joined = details.join(", ");
            if joined.is_empty() {
                label.to_string()
            } else {
                format!("{label} · {joined}")
            }
        }
    };
    clip_utf16(&raw, UI_BLOCK_LIMITS.outcome)
}
