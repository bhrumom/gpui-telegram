//! Create Managed Bot Dialog Box
//!
//! Upstream C++ Reference:
//! - `Telegram/SourceFiles/boxes/peers/create_managed_bot_box.h:L1-L35`
//! - `Telegram/SourceFiles/boxes/peers/create_managed_bot_box.cpp:L1-L522`
//! - `create_managed_bot_box.cpp:L40-L150` (Bot creation flow)
//!
//! Subsystem: boxes, Feature: TCG-N11

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::components::avatar::{
    parse_fabushi_hex_color, OPENBOT_BLOUB_AVATAR_PRESETS, OPENBOT_PLUSH_AVATAR_PRESETS,
};
use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, trigger_grok_avatar_bounce, trigger_grok_avatar_spin,
    GrokBotAvatarProps, GrokPersonaState, GROK_COLOR_PALETTE,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use gpui_telegram_proto::miniapp_contracts::{
    CreateFabushiBotRequest, CLASSIC_INITIALS_AVATAR_SHAPE, GROK_AVATAR_SHAPES,
};
use gpui_telegram_proto::{
    CodingAgentBotConfig, CodingPermissionMode, CodingProviderKind, DetectedCodingProvider,
};
use orca_core::bot_ecosystem::templates::{
    BotTemplate, BotTemplateCategory, BUILTIN_BOT_TEMPLATES,
};

fn template_category_label_zh(cat: BotTemplateCategory) -> &'static str {
    match cat {
        BotTemplateCategory::Coding => "编程",
        BotTemplateCategory::Releases => "发布",
        BotTemplateCategory::Support => "支持",
        BotTemplateCategory::Planning => "规划",
        BotTemplateCategory::Ops => "运维",
        BotTemplateCategory::Personal => "个人",
    }
}

fn template_plush_avatar_spec(idx: usize) -> &'static str {
    OPENBOT_PLUSH_AVATAR_PRESETS[idx % OPENBOT_PLUSH_AVATAR_PRESETS.len()].0
}

fn parse_coding_extra_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        match quote {
            Some(q) if ch == q => {
                quote = None;
            }
            Some(_) => {
                current.push(ch);
            }
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn coding_provider_avatar_preset(kind: CodingProviderKind) -> (&'static str, &'static str) {
    match kind {
        CodingProviderKind::ClaudeCode => ("squircle", "orange"),
        CodingProviderKind::Codex => ("hex", "green"),
        CodingProviderKind::OpenCode => ("blob", "cyan"),
        CodingProviderKind::CursorCli => ("wedge", "violet"),
        CodingProviderKind::Antigravity => ("cloud", "blue"),
        CodingProviderKind::AcpCustom => ("pebble", "magenta"),
    }
}

fn format_detected_provider_badge(
    kind: CodingProviderKind,
    detected: Option<&DetectedCodingProvider>,
    lang: &str,
) -> String {
    if kind == CodingProviderKind::AcpCustom {
        if let Some(ver) = detected
            .and_then(|d| d.version.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return if ver.starts_with('v') || ver.starts_with('V') {
                format!("✓ {ver}")
            } else {
                format!("✓ v{ver}")
            };
        }
        return "Custom ACP".to_string();
    }
    if let Some(info) = detected.filter(|d| d.installed) {
        if let Some(ver) = info
            .version
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if ver.starts_with('v') || ver.starts_with('V') {
                format!("✓ {ver}")
            } else {
                format!("✓ v{ver}")
            }
        } else {
            tr_str(lang, "✓ 已安装", "✓ Installed").to_string()
        }
    } else {
        tr_str(lang, "未安装", "Not installed").to_string()
    }
}

fn classic_preview_initials(label: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return "FB".to_string();
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() >= 2 {
        let mut out = String::new();
        for part in words.iter().take(2) {
            if let Some(c) = part.chars().next() {
                out.push(c);
            }
        }
        if !out.is_empty() {
            return out.to_uppercase();
        }
    }
    if trimmed.chars().count() <= 2 && trimmed.chars().all(|c| c.is_ascii_uppercase()) {
        return trimmed.to_string();
    }
    trimmed
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "FB".to_string())
}

/// Dialog box for creating a first-class Fabushi Bot (`Agent` aggregate),
/// instantiating from 24 built-in role templates, generating a full Bot + Mini App via
/// natural language, or connecting a managed token.
/// Target symbol in ledger: `CreateManagedBotBoxPeers`.
#[derive(Debug, Clone)]
pub struct CreateManagedBotBoxPeers {
    pub is_visible: bool,
    pub width: Pixels,
    pub height: Pixels,
    pub bot_name: String,
    pub bot_username: String,
    pub bot_token: String,
    pub description: String,
    pub avatar_shape: String,
    pub avatar_color: String,
    pub use_dynamic_avatar: bool,
    /// Active avatar style tab: `0` = OpenBot Plush 3D, `1` = OpenBot Bloub, `2` = Grok Geometry.
    pub avatar_style_tab: u8,
    pub preferred_model: String,
    pub thinking_level: String,
    pub approval_mode: String,
    pub is_business_bot: bool,
    pub is_submitting: bool,
    pub is_fabushi_mode: bool,
    pub is_coding_agent_mode: bool,
    pub is_template_mode: bool,
    pub is_ai_mode: bool,
    pub template_category_filter: Option<BotTemplateCategory>,
    pub selected_template_id: Option<String>,
    pub selected_template_skills: Vec<String>,
    pub detected_coding_providers: Vec<DetectedCodingProvider>,
    pub selected_coding_provider: CodingProviderKind,
    pub coding_workspace_path: String,
    pub coding_use_worktree: bool,
    pub coding_permission_mode: CodingPermissionMode,
    pub coding_executable_override: String,
    pub coding_extra_args: String,
    pub coding_allow_group_members: bool,
    pub coding_bound_device_id: Option<String>,
    pub nl_prompt: String,
    pub active_field: u8,
    pub pending_submit_fabushi: Option<CreateFabushiBotRequest>,
    pub pending_submit_nl: Option<String>,
    pub pending_submit_credentials: Option<(String, String, String, bool)>,
    pub pending_close: bool,
    pub status_message: Option<String>,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl Default for CreateManagedBotBoxPeers {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateManagedBotBoxPeers {
    pub fn new() -> Self {
        Self {
            is_visible: true,
            width: px(540.0),
            height: px(540.0),
            bot_name: String::new(),
            bot_username: String::new(),
            bot_token: String::new(),
            description: String::new(),
            avatar_shape: "blob".to_string(),
            avatar_color: "blue".to_string(),
            use_dynamic_avatar: true,
            avatar_style_tab: 0,
            preferred_model: String::new(),
            thinking_level: "medium".to_string(),
            approval_mode: "prompt".to_string(),
            is_business_bot: true,
            is_submitting: false,
            is_fabushi_mode: false,
            is_coding_agent_mode: false,
            is_template_mode: false,
            is_ai_mode: true,
            template_category_filter: None,
            selected_template_id: None,
            selected_template_skills: Vec::new(),
            detected_coding_providers: Vec::new(),
            selected_coding_provider: CodingProviderKind::ClaudeCode,
            coding_workspace_path: String::new(),
            coding_use_worktree: false,
            coding_permission_mode: CodingPermissionMode::Ask,
            coding_executable_override: String::new(),
            coding_extra_args: String::new(),
            coding_allow_group_members: false,
            coding_bound_device_id: None,
            nl_prompt: String::new(),
            active_field: 0,
            pending_submit_fabushi: None,
            pending_submit_nl: None,
            pending_submit_credentials: None,
            pending_close: false,
            status_message: None,
            palette: TdesktopThemePalette::default(),
            language_id: "en".to_string(),
        }
    }

    /// Pre-fills the unified Fabushi Bot form from one of the 24 built-in templates.
    pub fn apply_template(&mut self, tpl: &BotTemplate, idx: usize) {
        self.bot_name = tpl.name.to_string();
        let slug: String = tpl
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        self.bot_username = format!("{slug}_bot");
        let soul = tpl.soul_md.trim();
        self.description = if soul.is_empty() {
            tpl.tagline.to_string()
        } else {
            format!("{} — {}", tpl.tagline, soul.lines().take(3).collect::<Vec<_>>().join(" "))
        };
        self.avatar_shape = template_plush_avatar_spec(idx).to_string();
        self.avatar_color = tpl.avatar_color.to_string();
        self.use_dynamic_avatar = true;
        self.avatar_style_tab = 0;
        self.selected_template_id = Some(tpl.id.to_string());
        self.selected_template_skills = tpl.bundled_skills.iter().map(|s| (*s).to_string()).collect();
        self.is_template_mode = false;
        self.is_coding_agent_mode = false;
        self.is_fabushi_mode = true;
        self.is_ai_mode = false;
        self.active_field = 1;
        self.status_message = Some(format!(
            "✓ Loaded template '{}' ({} skills)",
            tpl.name,
            tpl.bundled_skills.len()
        ));
    }

    /// Takes any bundled skills associated with the selected template for local OpenBot provisioning.
    pub fn take_selected_template_skills(&mut self) -> Vec<String> {
        std::mem::take(&mut self.selected_template_skills)
    }

    /// Returns `(avatar_shape, avatar_color)` matching whether dynamic character mode is active.
    pub fn effective_avatar_choice(&self) -> (Option<String>, Option<String>) {
        if self.use_dynamic_avatar {
            (
                Some(self.avatar_shape.clone()),
                Some(self.avatar_color.clone()),
            )
        } else {
            (Some(CLASSIC_INITIALS_AVATAR_SHAPE.to_string()), None)
        }
    }

    pub fn set_credentials(&mut self, name: &str, username: &str, token: &str) {
        self.bot_name = name.trim().to_string();
        self.bot_username = username.trim().to_string();
        self.bot_token = token.trim().to_string();
    }

    pub fn is_valid(&self) -> bool {
        if self.is_coding_agent_mode {
            !self.bot_name.trim().is_empty()
                && !self.coding_workspace_path.trim().is_empty()
                && (self.bot_username.trim().is_empty()
                    || gpui_telegram_proto::miniapp_contracts::validate_bot_username(
                        &self.bot_username,
                    )
                    .is_ok())
                && (self.selected_coding_provider != CodingProviderKind::AcpCustom
                    || !self.coding_executable_override.trim().is_empty())
        } else if self.is_fabushi_mode {
            !self.bot_name.trim().is_empty()
                && (self.bot_username.trim().is_empty()
                    || gpui_telegram_proto::miniapp_contracts::validate_bot_username(
                        &self.bot_username,
                    )
                    .is_ok())
        } else {
            !self.bot_name.trim().is_empty()
                && gpui_telegram_proto::miniapp_contracts::validate_bot_username(&self.bot_username)
                    .is_ok()
        }
    }

    pub fn set_detected_coding_providers(
        &mut self,
        providers: Vec<DetectedCodingProvider>,
        bound_device_id: Option<String>,
    ) {
        self.detected_coding_providers = providers;
        if let Some(dev) = bound_device_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if self.coding_bound_device_id.is_none() {
                self.coding_bound_device_id = Some(dev);
            }
        }
    }

    pub fn select_coding_provider(&mut self, kind: CodingProviderKind) {
        self.selected_coding_provider = kind;
        self.bot_name = kind.display_name().to_string();
        self.bot_username = kind.recommended_username().to_string();
        let (shape, color) = coding_provider_avatar_preset(kind);
        self.avatar_shape = shape.to_string();
        self.avatar_color = color.to_string();
    }

    pub fn open_coding_agent_creator(&mut self) {
        self.is_visible = true;
        self.is_coding_agent_mode = true;
        self.is_fabushi_mode = false;
        self.is_template_mode = false;
        self.is_ai_mode = false;
        self.active_field = 10;
        self.is_submitting = false;
        self.status_message = None;
        if self.bot_name.trim().is_empty() && self.bot_username.trim().is_empty() {
            self.select_coding_provider(self.selected_coding_provider);
        }
    }

    pub fn open_fabushi_creator(&mut self) {
        self.is_visible = true;
        self.is_coding_agent_mode = false;
        self.is_fabushi_mode = true;
        self.is_template_mode = false;
        self.is_ai_mode = false;
        self.active_field = 1;
        self.is_submitting = false;
        self.status_message = None;
    }

    pub fn open_template_creator(&mut self) {
        self.is_visible = true;
        self.is_coding_agent_mode = false;
        self.is_fabushi_mode = false;
        self.is_template_mode = true;
        self.is_ai_mode = false;
        self.active_field = 0;
        self.is_submitting = false;
        self.status_message = None;
    }

    pub fn open_ai_creator(&mut self) {
        self.is_visible = true;
        self.is_coding_agent_mode = false;
        self.is_fabushi_mode = false;
        self.is_template_mode = false;
        self.is_ai_mode = true;
        self.active_field = 0;
        self.is_submitting = false;
        self.status_message = None;
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang: impl Into<String>) {
        self.language_id = lang.into();
    }

    pub fn active_field_text(&self) -> &str {
        if self.is_coding_agent_mode {
            match self.active_field {
                11 => &self.bot_name,
                12 => &self.bot_username,
                13 => &self.preferred_model,
                14 => &self.coding_executable_override,
                15 => &self.coding_extra_args,
                _ => &self.coding_workspace_path,
            }
        } else if self.is_fabushi_mode {
            match self.active_field {
                2 => &self.bot_username,
                3 => &self.description,
                4 => &self.preferred_model,
                _ => &self.bot_name,
            }
        } else if self.is_ai_mode || self.active_field == 0 {
            &self.nl_prompt
        } else {
            match self.active_field {
                1 => &self.bot_name,
                2 => &self.bot_username,
                _ => &self.bot_token,
            }
        }
    }

    pub fn clear_active_field(&mut self) {
        if self.is_coding_agent_mode {
            match self.active_field {
                11 => self.bot_name.clear(),
                12 => self.bot_username.clear(),
                13 => self.preferred_model.clear(),
                14 => self.coding_executable_override.clear(),
                15 => self.coding_extra_args.clear(),
                _ => self.coding_workspace_path.clear(),
            }
        } else if self.is_fabushi_mode {
            match self.active_field {
                2 => self.bot_username.clear(),
                3 => self.description.clear(),
                4 => self.preferred_model.clear(),
                _ => self.bot_name.clear(),
            }
        } else if self.is_ai_mode || self.active_field == 0 {
            self.nl_prompt.clear();
        } else {
            match self.active_field {
                1 => self.bot_name.clear(),
                2 => self.bot_username.clear(),
                _ => self.bot_token.clear(),
            }
        }
    }

    pub fn cycle_active_field(&mut self) {
        if self.is_coding_agent_mode {
            self.active_field = match self.active_field {
                10 => 11,
                11 => 12,
                12 => 13,
                13 => 14,
                14 => 15,
                _ => 10,
            };
        } else if self.is_fabushi_mode {
            self.active_field = match self.active_field {
                1 => 2,
                2 => 3,
                3 => 4,
                _ => 1,
            };
        } else if !self.is_ai_mode {
            self.active_field = match self.active_field {
                1 => 2,
                2 => 3,
                _ => 1,
            };
        }
    }

    pub fn push_text(&mut self, s: &str) {
        if self.is_coding_agent_mode {
            match self.active_field {
                11 => self.bot_name.push_str(s),
                12 => self.bot_username.push_str(s),
                13 => self.preferred_model.push_str(s),
                14 => self.coding_executable_override.push_str(s),
                15 => self.coding_extra_args.push_str(s),
                _ => self.coding_workspace_path.push_str(s),
            }
        } else if self.is_fabushi_mode {
            match self.active_field {
                2 => self.bot_username.push_str(s),
                3 => self.description.push_str(s),
                4 => self.preferred_model.push_str(s),
                _ => self.bot_name.push_str(s),
            }
        } else if self.is_ai_mode || self.active_field == 0 {
            self.nl_prompt.push_str(s);
        } else {
            match self.active_field {
                1 => self.bot_name.push_str(s),
                2 => self.bot_username.push_str(s),
                _ => self.bot_token.push_str(s),
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.is_coding_agent_mode {
            match self.active_field {
                11 => {
                    self.bot_name.pop();
                }
                12 => {
                    self.bot_username.pop();
                }
                13 => {
                    self.preferred_model.pop();
                }
                14 => {
                    self.coding_executable_override.pop();
                }
                15 => {
                    self.coding_extra_args.pop();
                }
                _ => {
                    self.coding_workspace_path.pop();
                }
            }
        } else if self.is_fabushi_mode {
            match self.active_field {
                2 => {
                    self.bot_username.pop();
                }
                3 => {
                    self.description.pop();
                }
                4 => {
                    self.preferred_model.pop();
                }
                _ => {
                    self.bot_name.pop();
                }
            }
        } else if self.is_ai_mode || self.active_field == 0 {
            self.nl_prompt.pop();
        } else {
            match self.active_field {
                1 => {
                    self.bot_name.pop();
                }
                2 => {
                    self.bot_username.pop();
                }
                _ => {
                    self.bot_token.pop();
                }
            }
        }
    }

    pub fn submit(&mut self) {
        let opt_str = |s: &str| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        };
        if self.is_coding_agent_mode {
            if self.is_valid() {
                self.status_message = None;
                self.is_submitting = true;
                let (av_shape, av_color) = self.effective_avatar_choice();
                let model_override = opt_str(&self.preferred_model);
                let bound_device_id = self
                    .coding_bound_device_id
                    .as_deref()
                    .and_then(opt_str);
                let config = CodingAgentBotConfig {
                    provider_kind: self.selected_coding_provider,
                    workspace_path: self.coding_workspace_path.trim().to_string(),
                    use_worktree: self.coding_use_worktree,
                    permission_mode: self.coding_permission_mode,
                    executable_override: opt_str(&self.coding_executable_override),
                    extra_args: parse_coding_extra_args(&self.coding_extra_args),
                    bound_device_id,
                    bound_node_id: None,
                    model_override: model_override.clone(),
                    allow_group_members: self.coding_allow_group_members,
                };
                self.pending_submit_fabushi = Some(CreateFabushiBotRequest {
                    display_name: self.bot_name.trim().to_string(),
                    username: opt_str(&self.bot_username),
                    agent_id: None,
                    description: format!(
                        "Local {} coding agent ({})",
                        self.selected_coding_provider.display_name(),
                        self.coding_workspace_path.trim()
                    ),
                    title: String::new(),
                    avatar_shape: av_shape,
                    avatar_color: av_color,
                    preferred_model: model_override,
                    thinking_level: None,
                    approval_mode: None,
                    toolset_preset: None,
                    coding_agent: Some(config),
                });
            } else {
                self.status_message = Some(if self.coding_workspace_path.trim().is_empty() {
                    "请填写本地工作区路径 (Workspace Path)".to_string()
                } else if self.selected_coding_provider == CodingProviderKind::AcpCustom
                    && self.coding_executable_override.trim().is_empty()
                {
                    "Custom ACP 需要填写可执行命令路径".to_string()
                } else {
                    "请检查机器人名称与 @username 格式".to_string()
                });
            }
        } else if self.is_fabushi_mode {
            if self.is_valid() {
                self.is_submitting = true;
                let (av_shape, av_color) = self.effective_avatar_choice();
                self.pending_submit_fabushi = Some(CreateFabushiBotRequest {
                    display_name: self.bot_name.trim().to_string(),
                    username: opt_str(&self.bot_username),
                    agent_id: None,
                    description: self.description.trim().to_string(),
                    title: String::new(),
                    avatar_shape: av_shape,
                    avatar_color: av_color,
                    preferred_model: opt_str(&self.preferred_model),
                    thinking_level: Some(self.thinking_level.clone()),
                    approval_mode: Some(self.approval_mode.clone()),
                    toolset_preset: Some("full".to_string()),
                    coding_agent: None,
                });
            }
        } else if self.is_ai_mode {
            let trimmed = self.nl_prompt.trim().to_string();
            if !trimmed.is_empty() {
                self.is_submitting = true;
                self.pending_submit_nl = Some(trimmed);
            }
        } else if self.is_valid() {
            self.is_submitting = true;
            self.pending_submit_credentials = Some((
                self.bot_name.clone(),
                self.bot_username.clone(),
                self.bot_token.clone(),
                self.is_business_bot,
            ));
        }
    }

    pub fn take_submit_fabushi(&mut self) -> Option<CreateFabushiBotRequest> {
        self.pending_submit_fabushi.take()
    }

    pub fn take_submit_nl(&mut self) -> Option<String> {
        self.pending_submit_nl.take()
    }

    pub fn take_submit_credentials(&mut self) -> Option<(String, String, String, bool)> {
        self.pending_submit_credentials.take()
    }

    pub fn take_close(&mut self) -> bool {
        let was = self.pending_close;
        self.pending_close = false;
        was
    }

    fn render_avatar_chooser_card(
        &self,
        preview_label: &str,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let preview_bg = parse_fabushi_hex_color(&self.avatar_color).unwrap_or(pal.accent);
        let preview_initials = classic_preview_initials(preview_label);

        let preview_stage = div()
            .id("create-bot-modal-preview-stage")
            .cursor_pointer()
            .w(px(72.0))
            .h(px(72.0))
            .flex_shrink_0()
            .rounded(px(12.0))
            .bg(rgb(pal.card_bg))
            .border_1()
            .border_color(rgb(pal.border_fg))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    trigger_grok_avatar_spin("create-bot-modal-preview");
                    cx.notify();
                }),
            )
            .child(if self.use_dynamic_avatar {
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(
                        "create-bot-modal-preview",
                        &self.avatar_shape,
                        &self.avatar_color,
                        64.0,
                    )
                    .with_state(GrokPersonaState::Happy)
                    .with_follow_pointer(true)
                    .with_surface_bg(pal.card_bg),
                )
            } else {
                div()
                    .w(px(64.0))
                    .h(px(64.0))
                    .rounded_full()
                    .bg(rgb(preview_bg))
                    .border_1()
                    .border_color(rgba(0xFFFFFF38))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xFFFFFF))
                    .text_size(px(22.0))
                    .font_weight(FontWeight::BOLD)
                    .child(preview_initials)
            });

        let right_controls = if self.use_dynamic_avatar {
            let style_tab = self.avatar_style_tab.min(2);
            let style_tabs: [(u8, &str); 3] = [
                (0, tr_str(lang, "毛绒 Plush", "Plush 3D")),
                (1, tr_str(lang, "拟人 Bloub", "Bloub")),
                (2, tr_str(lang, "几何 Grok", "Geometric")),
            ];
            let choices: Vec<(&'static str, &'static str)> = match style_tab {
                0 => OPENBOT_PLUSH_AVATAR_PRESETS.to_vec(),
                1 => OPENBOT_BLOUB_AVATAR_PRESETS.to_vec(),
                _ => GROK_AVATAR_SHAPES.iter().map(|&s| (s, s)).collect(),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .children(style_tabs.into_iter().map(|(tab_id, label)| {
                            let active = style_tab == tab_id;
                            div()
                                .id(SharedString::from(format!("fab-avatar-style-tab-{tab_id}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(2.5))
                                .rounded(px(999.0))
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    rgb(pal.input_bg).into()
                                })
                                .border_1()
                                .border_color(if active {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.separator_rgba()
                                })
                                .text_size(px(10.5))
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(rgb(if active {
                                    pal.accent
                                } else {
                                    pal.text_secondary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.avatar_style_tab = tab_id;
                                        cx.notify();
                                    }),
                                )
                                .child(label)
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.0))
                        .children(choices.into_iter().enumerate().map(|(idx, (shape, label))| {
                            let active = self.avatar_shape.eq_ignore_ascii_case(shape);
                            let shape_owned = shape.to_string();
                            let opt_id = format!("create-bot-shape-opt-{style_tab}-{idx}");
                            let btn_bg = if active {
                                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                            } else {
                                rgb(pal.input_bg).into()
                            };
                            div()
                                .id(SharedString::from(format!("fab-shape-pick-{style_tab}-{idx}")))
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(8.0))
                                .bg(btn_bg)
                                .border_1()
                                .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.avatar_shape = shape_owned.clone();
                                        trigger_grok_avatar_bounce("create-bot-modal-preview");
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    render_grok_bot_avatar(
                                        GrokBotAvatarProps::new(
                                            opt_id.as_str(),
                                            shape,
                                            &self.avatar_color,
                                            26.0,
                                        )
                                        .with_state(GrokPersonaState::Idle)
                                        .with_paused(true)
                                        .with_surface_bg(pal.input_bg),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .font_weight(if active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(rgb(if active {
                                            pal.text_primary
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .child(label),
                                )
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(5.0))
                        .children(GROK_COLOR_PALETTE.iter().enumerate().map(|(idx, spec)| {
                            let active = self.avatar_color.eq_ignore_ascii_case(spec.id);
                            let spec_id = spec.id;
                            div()
                                .id(SharedString::from(format!("fab-color-pick-{idx}")))
                                .cursor_pointer()
                                .w(px(20.0))
                                .h(px(20.0))
                                .rounded_full()
                                .bg(rgb(spec.swatch_rgb))
                                .border_2()
                                .border_color(rgb(if active {
                                    pal.text_primary
                                } else if spec_id == "black" {
                                    pal.border_fg
                                } else {
                                    pal.card_bg
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.avatar_color = spec_id.to_string();
                                        trigger_grok_avatar_bounce("create-bot-modal-preview");
                                        cx.notify();
                                    }),
                                )
                        })),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "已启用经典字母圆形头像",
                            "Classic Initials Avatar Enabled",
                        )),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "将使用名称首字母生成经典纯色头像；点击右上角「✦ 动态角色」可随时切回 Grok 角色。",
                            "Uses classic circular initials. Switch to ✦ Dynamic anytime for an animated Grok character.",
                        )),
                )
        };

        div()
            .id("create-bot-avatar-chooser-card")
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(10.0))
            .rounded(px(10.0))
            .bg(rgb(pal.card_bg))
            .border_1()
            .border_color(rgb(pal.border_fg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(
                                lang,
                                "Grok Bot 动态角色头像",
                                "Grok Dynamic Character",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("create-bot-avatar-mode-dynamic")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(if self.use_dynamic_avatar {
                                        pal.accent
                                    } else {
                                        pal.input_bg
                                    }))
                                    .border_1()
                                    .border_color(rgb(if self.use_dynamic_avatar {
                                        pal.accent
                                    } else {
                                        pal.border_fg
                                    }))
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if self.use_dynamic_avatar {
                                        0xFFFFFF
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.use_dynamic_avatar = true;
                                            trigger_grok_avatar_bounce("create-bot-modal-preview");
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "✦ 动态角色", "✦ Dynamic")),
                            )
                            .child(
                                div()
                                    .id("create-bot-avatar-mode-classic")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(if !self.use_dynamic_avatar {
                                        pal.accent
                                    } else {
                                        pal.input_bg
                                    }))
                                    .border_1()
                                    .border_color(rgb(if !self.use_dynamic_avatar {
                                        pal.accent
                                    } else {
                                        pal.border_fg
                                    }))
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if !self.use_dynamic_avatar {
                                        0xFFFFFF
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.use_dynamic_avatar = false;
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "Aa 经典字母", "Aa Classic")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(preview_stage)
                    .child(right_controls),
            )
    }

    fn render_coding_agent_panel(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let provider_pills = div()
            .flex()
            .flex_wrap()
            .gap(px(5.0))
            .children(CodingProviderKind::ALL.into_iter().enumerate().map(|(idx, kind)| {
                let active = self.selected_coding_provider == kind;
                let detected = self
                    .detected_coding_providers
                    .iter()
                    .find(|p| p.kind == kind);
                let installed = kind == CodingProviderKind::AcpCustom
                    || detected.is_some_and(|d| d.installed);
                let badge = format_detected_provider_badge(kind, detected, lang);
                let badge_color = if active {
                    0xFFFFFF
                } else if installed {
                    pal.success
                } else {
                    pal.text_secondary
                };
                div()
                    .id(SharedString::from(format!("coding-provider-pill-{idx}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .bg(rgb(if active { pal.accent } else { pal.card_bg }))
                    .border_1()
                    .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.select_coding_provider(kind);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if active { 0xFFFFFF } else { pal.text_primary }))
                            .child(kind.display_name()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(badge_color))
                            .child(badge),
                    )
            }));

        let exec_label = if self.selected_coding_provider == CodingProviderKind::AcpCustom {
            tr_str(
                lang,
                "ACP 可执行命令路径 *",
                "ACP Executable Path *",
            )
        } else {
            tr_str(
                lang,
                "自定义可执行文件覆盖（可选）",
                "Executable Override (optional)",
            )
        };
        let coding_fields: [(u8, &str, &str, String); 6] = [
            (
                10,
                tr_str(lang, "工作区本地目录 *", "Local Workspace Path *"),
                tr_str(
                    lang,
                    "例如：/Users/dev/project",
                    "e.g. /Users/dev/project",
                ),
                self.coding_workspace_path.clone(),
            ),
            (
                11,
                tr_str(lang, "智能体名称 *", "Agent Name *"),
                tr_str(lang, "例如：Claude Code", "e.g. Claude Code"),
                self.bot_name.clone(),
            ),
            (
                12,
                tr_str(
                    lang,
                    "@用户名（可选，留空自动生成）",
                    "@username (optional, auto *_bot)",
                ),
                self.selected_coding_provider.recommended_username(),
                self.bot_username.clone(),
            ),
            (
                13,
                tr_str(lang, "模型覆盖（可选）", "Model Override (optional)"),
                tr_str(lang, "留空使用 CLI 默认模型", "Leave blank for provider default"),
                self.preferred_model.clone(),
            ),
            (
                14,
                exec_label,
                self.selected_coding_provider
                    .default_binary()
                    .unwrap_or("/path/to/acp-agent"),
                self.coding_executable_override.clone(),
            ),
            (
                15,
                tr_str(lang, "额外 CLI 参数（可选）", "Extra CLI Args (optional)"),
                "--verbose",
                self.coding_extra_args.clone(),
            ),
        ];

        let permission_pills = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "权限模式:", "Permission:")),
            )
            .children(
                [
                    CodingPermissionMode::Ask,
                    CodingPermissionMode::AutoEdit,
                    CodingPermissionMode::FullAccess,
                ]
                .into_iter()
                .enumerate()
                .map(|(idx, mode)| {
                    let active = self.coding_permission_mode == mode;
                    let label = if lang.starts_with("zh") {
                        mode.label_zh()
                    } else {
                        mode.label_en()
                    };
                    div()
                        .id(SharedString::from(format!("coding-perm-pick-{idx}")))
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(rgb(if active { pal.accent } else { pal.card_bg }))
                        .border_1()
                        .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                        .text_size(px(10.5))
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(if active { 0xFFFFFF } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.coding_permission_mode = mode;
                                cx.notify();
                            }),
                        )
                        .child(label)
                }),
            );

        let toggles_row = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.0))
            .child(
                div()
                    .id("coding-worktree-toggle")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(7.0))
                    .bg(rgb(if self.coding_use_worktree {
                        pal.accent
                    } else {
                        pal.card_bg
                    }))
                    .border_1()
                    .border_color(rgb(if self.coding_use_worktree {
                        pal.accent
                    } else {
                        pal.border_fg
                    }))
                    .text_size(px(10.5))
                    .text_color(rgb(if self.coding_use_worktree {
                        0xFFFFFF
                    } else {
                        pal.text_primary
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.coding_use_worktree = !this.coding_use_worktree;
                            cx.notify();
                        }),
                    )
                    .child(if self.coding_use_worktree {
                        tr_str(lang, "✓ Git Worktree 隔离", "✓ Git Worktree Isolation")
                    } else {
                        tr_str(lang, "Git Worktree 隔离（关）", "Git Worktree Isolation (Off)")
                    }),
            )
            .child(
                div()
                    .id("coding-group-members-toggle")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(7.0))
                    .bg(rgb(if self.coding_allow_group_members {
                        pal.accent
                    } else {
                        pal.card_bg
                    }))
                    .border_1()
                    .border_color(rgb(if self.coding_allow_group_members {
                        pal.accent
                    } else {
                        pal.border_fg
                    }))
                    .text_size(px(10.5))
                    .text_color(rgb(if self.coding_allow_group_members {
                        0xFFFFFF
                    } else {
                        pal.text_primary
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.coding_allow_group_members = !this.coding_allow_group_members;
                            cx.notify();
                        }),
                    )
                    .child(if self.coding_allow_group_members {
                        tr_str(lang, "✓ 允许群成员触发", "✓ Allow Group Members")
                    } else {
                        tr_str(lang, "仅所有者可操作", "Owner Only")
                    }),
            )
            .when_some(self.coding_bound_device_id.clone(), |row, dev_id| {
                row.child(
                    div()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(rgb(pal.border_fg))
                        .text_size(px(10.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "{}: {}",
                            tr_str(lang, "绑定设备", "Host"),
                            dev_id
                        )),
                )
            });

        div()
            .id("create-coding-agent-scroll")
            .max_h(px(330.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "选择本地 Coding CLI / ACP 提供方",
                        "Select Local Coding CLI / ACP Provider",
                    )),
            )
            .child(provider_pills)
            .children(coding_fields.into_iter().map(|(fid, label, placeholder, val)| {
                let active = self.active_field == fid;
                div()
                    .id(("coding-field", fid as u64))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.active_field = fid;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(if val.is_empty() {
                                pal.text_secondary
                            } else {
                                pal.text_primary
                            }))
                            .child(if val.is_empty() {
                                placeholder.to_string()
                            } else {
                                val
                            }),
                    )
            }))
            .child(permission_pills)
            .child(toggles_row)
    }
    fn render_template_gallery_panel(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let active_cat = self.template_category_filter;
        let is_zh = lang.starts_with("zh");

        let mut cat_row = div().flex().flex_wrap().items_center().gap(px(4.0)).child(
            div()
                .id("tpl-cat-all")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(999.0))
                .bg(if active_cat.is_none() {
                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                } else {
                    rgb(pal.card_bg).into()
                })
                .border_1()
                .border_color(if active_cat.is_none() {
                    rgb(pal.accent).into()
                } else {
                    pal.separator_rgba()
                })
                .text_size(px(10.5))
                .font_weight(if active_cat.is_none() {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(rgb(if active_cat.is_none() {
                    pal.accent
                } else {
                    pal.text_secondary
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.template_category_filter = None;
                        cx.notify();
                    }),
                )
                .child(tr_str(lang, "全部 (24)", "All (24)")),
        );

        for (idx, cat) in BotTemplateCategory::ALL.into_iter().enumerate() {
            let selected = active_cat == Some(cat);
            let label = if is_zh {
                template_category_label_zh(cat)
            } else {
                cat.label()
            };
            cat_row = cat_row.child(
                div()
                    .id(SharedString::from(format!("tpl-cat-{idx}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(999.0))
                    .bg(if selected {
                        TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                    } else {
                        rgb(pal.card_bg).into()
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(pal.accent).into()
                    } else {
                        pal.separator_rgba()
                    })
                    .text_size(px(10.5))
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(rgb(if selected {
                        pal.accent
                    } else {
                        pal.text_secondary
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.template_category_filter = Some(cat);
                            cx.notify();
                        }),
                    )
                    .child(label),
            );
        }

        let mut grid = div()
            .id("create-bot-template-grid")
            .w_full()
            .max_h(px(230.0))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .gap(px(6.0));

        for (idx, tpl) in BUILTIN_BOT_TEMPLATES.iter().enumerate() {
            if let Some(filter_cat) = active_cat {
                if tpl.category != filter_cat {
                    continue;
                }
            }
            let selected = self.selected_template_id.as_deref() == Some(tpl.id);
            let plush_spec = template_plush_avatar_spec(idx);
            let tpl_copy = *tpl;
            let skill_badge = if is_zh {
                format!("{} 项技能", tpl.bundled_skills.len())
            } else {
                format!("{} skills", tpl.bundled_skills.len())
            };
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("bot-tpl-card-{idx}")))
                    .cursor_pointer()
                    .w(px(244.0))
                    .p(px(8.0))
                    .rounded(px(10.0))
                    .bg(if selected {
                        TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14)
                    } else {
                        rgb(pal.card_bg).into()
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(pal.accent).into()
                    } else {
                        pal.separator_rgba()
                    })
                    .hover(|s| s.bg(rgb(pal.hover_bg)))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.apply_template(&tpl_copy, idx);
                            trigger_grok_avatar_bounce("create-bot-modal-preview");
                            cx.notify();
                        }),
                    )
                    .child(render_grok_bot_avatar(
                        GrokBotAvatarProps::new(
                            format!("tpl-prev-{idx}"),
                            plush_spec,
                            tpl.avatar_color,
                            34.0,
                        )
                        .with_state(GrokPersonaState::Idle)
                        .with_paused(true)
                        .with_surface_bg(pal.card_bg),
                    ))
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
                                    .justify_between()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tpl.name),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .rounded(px(999.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                pal.accent, 0.14,
                                            ))
                                            .text_size(px(9.5))
                                            .text_color(rgb(pal.accent))
                                            .child(skill_badge),
                                    ),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tpl.tagline),
                            ),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "选择一个角色模板，自动填充人设、毛绒形象与内置技能并创建为原生消息机器人：",
                        "Pick a role template to pre-fill persona, Plush avatar, and bundled skills as a native Bot:",
                    )),
            )
            .child(cat_row)
            .child(grid)
    }
}

impl Render for CreateManagedBotBoxPeers {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let is_coding = self.is_coding_agent_mode;
        let is_template = self.is_template_mode && !is_coding;
        let is_fabushi = self.is_fabushi_mode && !is_coding && !is_template;
        let is_ai = self.is_ai_mode && !is_coding && !is_template && !is_fabushi;
        let prompt = self.nl_prompt.clone();
        let can_submit = if is_template {
            false
        } else if is_coding || is_fabushi {
            self.is_valid() && !self.is_submitting
        } else if is_ai {
            !prompt.trim().is_empty() && !self.is_submitting
        } else {
            self.is_valid() && !self.is_submitting
        };

        let presets: [(&str, &str); 3] = [
            ("☕ 咖啡点单", "创建一个精品咖啡点单和会员积分小程序"),
            ("✅ 团队待办", "创建一个支持分配负责人、优先级和 AI 总结的待办清单小程序"),
            ("📊 投票决策", "创建一个实时团队投票和数据分析小程序"),
        ];
        let preview_label = if self.bot_name.trim().is_empty() {
            "FB"
        } else {
            self.bot_name.as_str()
        };
        let avatar_chooser_card = self.render_avatar_chooser_card(preview_label, pal, lang, cx);

        div()
            .id("create-managed-bot-modal")
            .w(if is_coding || is_template { px(560.0) } else { self.width })
            .min_h(self.height)
            .p(px(16.0))
            .rounded(px(14.0))
            .bg(rgb(pal.modal_bg))
            .border_1()
            .border_color(rgb(pal.border_fg))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(tg_icon(IconName::BotRobot, 20.0, pal.accent))
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(if is_coding {
                                        tr_str(lang, "新建编程智能体 (Coding Agent)", "New Coding Agent Bot")
                                    } else if is_template {
                                        tr_str(lang, "从角色模板创建机器人 (24 套)", "Create Bot from Template (24)")
                                    } else if is_fabushi {
                                        tr_str(lang, "新建 Fabushi Bot 智能体", "New Fabushi Bot Agent")
                                    } else if is_ai {
                                        tr_str(lang, "BotFather · AI 快速创建", "BotFather · AI Bot & Mini App")
                                    } else {
                                        tr_str(lang, "BotFather · 连接托管 Bot", "BotFather · Connect Managed Bot")
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("create-managed-bot-close-btn")
                            .cursor_pointer()
                            .p(px(4.0))
                            .rounded_full()
                            .hover(|s| s.bg(rgb(pal.hover_bg)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.is_visible = false;
                                this.pending_close = true;
                                cx.notify();
                            }))
                            .child(tg_icon(IconName::Close, 16.0, pal.icon_fg)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(5.0))
                    .child(
                        div()
                            .id("create-bot-tab-fabushi")
                            .cursor_pointer()
                            .flex_1()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(if is_fabushi { pal.accent } else { pal.card_bg }))
                            .flex()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if is_fabushi { 0xFFFFFF } else { pal.text_primary }))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.open_fabushi_creator();
                                cx.notify();
                            }))
                            .child(tr_str(lang, "✦ 智能体", "✦ Agent")),
                    )
                    .child(
                        div()
                            .id("create-bot-tab-templates")
                            .cursor_pointer()
                            .flex_1()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(if is_template { pal.accent } else { pal.card_bg }))
                            .flex()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if is_template { 0xFFFFFF } else { pal.text_primary }))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.open_template_creator();
                                cx.notify();
                            }))
                            .child(tr_str(lang, "📦 模板(24)", "📦 Templates")),
                    )
                    .child(
                        div()
                            .id("create-bot-tab-coding")
                            .cursor_pointer()
                            .flex_1()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(if is_coding { pal.accent } else { pal.card_bg }))
                            .flex()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if is_coding { 0xFFFFFF } else { pal.text_primary }))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.open_coding_agent_creator();
                                cx.notify();
                            }))
                            .child(tr_str(lang, "💻 编程", "💻 Coding")),
                    )
                    .child(
                        div()
                            .id("create-bot-tab-ai")
                            .cursor_pointer()
                            .flex_1()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(if is_ai { pal.accent } else { pal.card_bg }))
                            .flex()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if is_ai { 0xFFFFFF } else { pal.text_primary }))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.open_ai_creator();
                                cx.notify();
                            }))
                            .child(tr_str(lang, "⚡ 一句话", "⚡ AI")),
                    )
                    .child(
                        div()
                            .id("create-bot-tab-manual")
                            .cursor_pointer()
                            .flex_1()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(if !is_coding && !is_template && !is_fabushi && !is_ai { pal.accent } else { pal.card_bg }))
                            .flex()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if !is_coding && !is_template && !is_fabushi && !is_ai { 0xFFFFFF } else { pal.text_primary }))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.is_coding_agent_mode = false;
                                this.is_template_mode = false;
                                this.is_fabushi_mode = false;
                                this.is_ai_mode = false;
                                this.active_field = 1;
                                cx.notify();
                            }))
                            .child(tr_str(lang, "🔑 Token", "🔑 Token")),
                    ),
            )
            .when(!is_template, |el| el.child(avatar_chooser_card))
            .when(is_template, |el| el.child(self.render_template_gallery_panel(pal, lang, cx)))
            .when(is_coding, |el| el.child(self.render_coding_agent_panel(pal, lang, cx)))
            .when(is_fabushi, |el| {
                let fab_fields: [(u8, &str, &str, String); 4] = [
                    (
                        1,
                        tr_str(lang, "智能体名称 *", "Agent Name *"),
                        tr_str(lang, "例如：Research Lead", "e.g. Research Lead"),
                        self.bot_name.clone(),
                    ),
                    (
                        2,
                        tr_str(lang, "@用户名（可选，留空自动生成 *_bot）", "@username (optional, auto *_bot)"),
                        tr_str(lang, "留空自动生成 research_lead_bot", "Leave blank for auto *_bot"),
                        self.bot_username.clone(),
                    ),
                    (
                        3,
                        tr_str(lang, "人设与职责 (SOUL / Mandate)", "Persona & Mandate (SOUL)"),
                        tr_str(lang, "描述其职责与记忆边界…", "Describe capabilities and persona…"),
                        self.description.clone(),
                    ),
                    (
                        4,
                        tr_str(lang, "首选模型（可选）", "Preferred Model (optional)"),
                        tr_str(lang, "留空继承 Host Active Provider", "Inherit Host Active Provider"),
                        self.preferred_model.clone(),
                    ),
                ];
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .children(fab_fields.into_iter().map(|(fid, label, placeholder, val)| {
                            let active = self.active_field == fid;
                            div()
                                .id(("fab-field", fid as u64))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                    this.active_field = fid;
                                    cx.notify();
                                }))
                                .child(div().text_size(px(10.5)).text_color(rgb(pal.text_secondary)).child(label))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(rgb(if val.is_empty() { pal.text_secondary } else { pal.text_primary }))
                                        .child(if val.is_empty() { placeholder.to_string() } else { val }),
                                )
                        }))
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
                                        .gap(px(4.0))
                                        .child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "思考:", "Think:")))
                                        .children(["off", "low", "medium", "high"].into_iter().enumerate().map(|(idx, lvl)| {
                                            let active = self.thinking_level == lvl;
                                            let owned = lvl.to_string();
                                            div()
                                                .id(SharedString::from(format!("fab-think-pick-{idx}")))
                                                .cursor_pointer()
                                                .px(px(6.0))
                                                .py(px(2.5))
                                                .rounded(px(6.0))
                                                .bg(rgb(if active { pal.accent } else { pal.card_bg }))
                                                .text_size(px(10.5))
                                                .text_color(rgb(if active { 0xFFFFFF } else { pal.text_primary }))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                                    this.thinking_level = owned.clone();
                                                    cx.notify();
                                                }))
                                                .child(lvl)
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "审批:", "Approval:")))
                                        .children(["prompt", "autonomous", "strict"].into_iter().enumerate().map(|(idx, mode)| {
                                            let active = self.approval_mode == mode;
                                            let owned = mode.to_string();
                                            div()
                                                .id(SharedString::from(format!("fab-app-pick-{idx}")))
                                                .cursor_pointer()
                                                .px(px(6.0))
                                                .py(px(2.5))
                                                .rounded(px(6.0))
                                                .bg(rgb(if active { pal.accent } else { pal.card_bg }))
                                                .text_size(px(10.5))
                                                .text_color(rgb(if active { 0xFFFFFF } else { pal.text_primary }))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                                    this.approval_mode = owned.clone();
                                                    cx.notify();
                                                }))
                                                .child(if mode == "autonomous" { "yolo" } else { mode })
                                        })),
                                ),
                        ),
                )
            })
            .when(is_ai, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "描述你想要的机器人和小程序，系统将自动创建可交互 Mini App 并绑定专属 AI 伴侣机器人：",
                                    "Describe the Bot + Mini App you want. Companion AI Bot + interactive Mini App will be provisioned automatically:",
                                )),
                        )
                        .child(
                            div()
                                .id("create-managed-bot-nl-input")
                                .w_full()
                                .min_h(px(76.0))
                                .p(px(10.0))
                                .rounded(px(10.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(pal.accent))
                                .text_size(px(13.0))
                                .text_color(rgb(if prompt.is_empty() { pal.text_secondary } else { pal.text_primary }))
                                .child(if prompt.is_empty() {
                                    tr_str(lang, "例如：帮我做一个极简咖啡点单小程序和AI店长…", "e.g., Build a specialty coffee ordering Mini App with AI barista…").to_string()
                                } else {
                                    prompt
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(6.0))
                                .children(presets.into_iter().map(|(pill, full)| {
                                    let full_owned = full.to_string();
                                    div()
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(pal.card_bg))
                                        .border_1()
                                        .border_color(rgb(pal.border_fg))
                                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_link))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                            this.nl_prompt = full_owned.clone();
                                            cx.notify();
                                        }))
                                        .child(pill)
                                })),
                        ),
                )
            })
            .when(!is_coding && !is_template && !is_fabushi && !is_ai, |el| {
                let fields: [(u8, &str, String); 3] = [
                    (1, "Bot Name", self.bot_name.clone()),
                    (2, "@username (must end with bot)", self.bot_username.clone()),
                    (3, "HTTP API Token", self.bot_token.clone()),
                ];
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(fields.into_iter().map(|(fid, label, val)| {
                            let active = self.active_field == fid;
                            div()
                                .id(("adv-bot-field", fid as u64))
                                .cursor_pointer()
                                .p(px(9.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                    this.active_field = fid;
                                    cx.notify();
                                }))
                                .child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(label))
                                .child(div().text_size(px(13.0)).text_color(rgb(pal.text_primary)).child(if val.is_empty() { "—".to_string() } else { val }))
                        })),
                )
            })
            .when_some(self.status_message.clone(), |el, msg| {
                el.child(div().text_size(px(12.0)).text_color(rgb(pal.success)).child(msg))
            })
            .child(
                div()
                    .id("create-managed-bot-submit-btn")
                    .cursor_pointer()
                    .w_full()
                    .h(px(36.0))
                    .rounded(px(10.0))
                    .bg(rgb(if can_submit { pal.accent } else { pal.muted_badge_bg }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.submit();
                        cx.notify();
                    }))
                    .child(if self.is_submitting {
                        tr_str(lang, "正在创建并部署…", "Provisioning…")
                    } else if is_coding {
                        tr_str(lang, "💻 创建本地编程智能体", "💻 Create Coding Agent Bot")
                    } else if is_fabushi {
                        tr_str(lang, "✦ 创建独立 Fabushi 智能体", "✦ Create Isolated Fabushi Agent")
                    } else if is_ai {
                        tr_str(lang, "✨ 立即创建 Bot & 小程序", "✨ Create Bot & Mini App")
                    } else {
                        tr_str(lang, "保存并连接 Bot", "Connect Managed Bot")
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn test_create_bot_box_defaults_and_dynamic_classic_toggle() {
        let mut box_ui = CreateManagedBotBoxPeers::new();
        assert_eq!(box_ui.avatar_shape, "blob");
        assert_eq!(box_ui.avatar_color, "blue");
        assert!(box_ui.use_dynamic_avatar);
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some("blob".to_string()), Some("blue".to_string()))
        );

        // Select custom Grok shape & color
        box_ui.avatar_shape = GROK_AVATAR_SHAPES[6].to_string(); // "cloud"
        box_ui.avatar_color = GROK_COLOR_PALETTE[8].id.to_string(); // "violet"
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some("cloud".to_string()), Some("violet".to_string()))
        );

        // Toggle to Classic initials mode
        box_ui.use_dynamic_avatar = false;
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some(CLASSIC_INITIALS_AVATAR_SHAPE.to_string()), None)
        );

        // Toggle back to Dynamic mode preserves shape & color selection
        box_ui.use_dynamic_avatar = true;
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some("cloud".to_string()), Some("violet".to_string()))
        );
    }

    #[std::prelude::v1::test]
    fn test_effective_avatar_choice_across_all_three_creation_modes() {
        let mut box_ui = CreateManagedBotBoxPeers::new();

        // 1. Fabushi Agent tab
        box_ui.open_fabushi_creator();
        assert!(box_ui.is_fabushi_mode);
        assert!(!box_ui.is_ai_mode);
        box_ui.bot_name = "Research Lead".to_string();
        box_ui.avatar_shape = "teardrop".to_string();
        box_ui.avatar_color = "cyan".to_string();
        box_ui.submit();
        let fab_req = box_ui
            .take_submit_fabushi()
            .expect("fabushi submit payload");
        assert_eq!(fab_req.avatar_shape.as_deref(), Some("teardrop"));
        assert_eq!(fab_req.avatar_color.as_deref(), Some("cyan"));

        box_ui.is_submitting = false;
        box_ui.use_dynamic_avatar = false;
        box_ui.submit();
        let fab_classic_req = box_ui
            .take_submit_fabushi()
            .expect("fabushi classic submit payload");
        assert_eq!(
            fab_classic_req.avatar_shape.as_deref(),
            Some(CLASSIC_INITIALS_AVATAR_SHAPE)
        );
        assert_eq!(fab_classic_req.avatar_color, None);

        // 2. One-Click AI tab
        box_ui.open_ai_creator();
        assert!(!box_ui.is_fabushi_mode);
        assert!(box_ui.is_ai_mode);
        box_ui.use_dynamic_avatar = true;
        box_ui.avatar_shape = "pebble".to_string();
        box_ui.avatar_color = "orange".to_string();
        box_ui.push_text("Build a daily standup mini app");
        box_ui.submit();
        assert_eq!(
            box_ui.take_submit_nl().as_deref(),
            Some("Build a daily standup mini app")
        );
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some("pebble".to_string()), Some("orange".to_string()))
        );

        // 3. Advanced / Token tab
        box_ui.is_fabushi_mode = false;
        box_ui.is_ai_mode = false;
        box_ui.is_submitting = false;
        box_ui.set_credentials("Ops Helper", "ops_helper_bot", "600010:token");
        box_ui.avatar_shape = "wedge".to_string();
        box_ui.avatar_color = "black".to_string();
        box_ui.submit();
        let creds = box_ui
            .take_submit_credentials()
            .expect("credentials submit payload");
        assert_eq!(creds.0, "Ops Helper");
        assert_eq!(creds.1, "ops_helper_bot");
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some("wedge".to_string()), Some("black".to_string()))
        );
        box_ui.use_dynamic_avatar = false;
        assert_eq!(
            box_ui.effective_avatar_choice(),
            (Some(CLASSIC_INITIALS_AVATAR_SHAPE.to_string()), None)
        );
    }
}
