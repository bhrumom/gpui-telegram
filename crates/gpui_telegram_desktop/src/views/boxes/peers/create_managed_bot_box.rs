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
    legacy_shape_to_bloub_shape, parse_fabushi_hex_color, render_plush_bot_avatar,
};
use crate::components::bloub_avatar::{render_agent_avatar, AgentAvatarProps};
use crate::i18n::tr_str;
use crate::theme::openbot_palette::parse_css_color_to_rgba;
use crate::theme::{tg_icon_rgba, IconName, TdesktopThemePalette};
use gpui_telegram_proto::miniapp_contracts::{
    CreateFabushiBotRequest, CLASSIC_INITIALS_AVATAR_SHAPE,
};
use gpui_telegram_proto::{
    CodingAgentBotConfig, CodingPermissionMode, CodingProviderKind, DetectedCodingProvider,
};
use openbot_contracts::avatar::{
    AvatarColorChoice, AvatarMood, BloubExpression, BloubShape, PlushTransition,
    AVATAR_COLOR_CHOICES,
};
use orca_core::bot_ecosystem::templates::{
    BotTemplate, BotTemplateCategory, BUILTIN_BOT_TEMPLATES,
};

const STUDIO_FACE_CHOICES: [(BloubShape, BloubExpression); 12] = [
    (BloubShape::Cercle, BloubExpression::Attentif),
    (BloubShape::Squircle, BloubExpression::Curieux),
    (BloubShape::Triangle, BloubExpression::Neutre),
    (BloubShape::Hexagone, BloubExpression::Surpris),
    (BloubShape::Carre, BloubExpression::Attentif),
    (BloubShape::Galet, BloubExpression::Curieux),
    (BloubShape::Pentagone, BloubExpression::Neutre),
    (BloubShape::Capsule, BloubExpression::Attentif),
    (BloubShape::Losange, BloubExpression::Curieux),
    (BloubShape::Nuage, BloubExpression::Surpris),
    (BloubShape::Fantome, BloubExpression::Neutre),
    (BloubShape::Poulpe, BloubExpression::Curieux),
];

const STUDIO_AUTO_HEX: &str = "#8b5cf6";

struct StudioStarterSuggestion {
    id: &'static str,
    title_zh: &'static str,
    title_en: &'static str,
    desc_zh: &'static str,
    desc_en: &'static str,
    prompt_zh: &'static str,
    prompt_en: &'static str,
    shape: BloubShape,
    color_hex: &'static str,
}

const STUDIO_STARTER_SUGGESTIONS: [StudioStarterSuggestion; 6] = [
    StudioStarterSuggestion {
        id: "inbox-helper",
        title_zh: "收件箱助手",
        title_en: "Inbox Helper",
        desc_zh: "起草回复，避免遗漏跟进。",
        desc_en: "Draft crisp replies and never miss a follow-up.",
        prompt_zh: "帮我整理收件箱、提炼紧急事项并起草得体的回复，避免遗漏跟进。",
        prompt_en: "Help me triage my inbox, surface urgent threads, and draft crisp replies.",
        shape: BloubShape::Squircle,
        color_hex: "#e152b0",
    },
    StudioStarterSuggestion {
        id: "trip-planner",
        title_zh: "行程规划师",
        title_en: "Trip Planner",
        desc_zh: "比较方案，制定实用的行程。",
        desc_en: "Compare options and build practical day-by-day itineraries.",
        prompt_zh: "帮我比较机酒与交通方案，制定细致实用的每日行程计划。",
        prompt_en: "Compare flights and hotels and build practical day-by-day travel itineraries.",
        shape: BloubShape::Triangle,
        color_hex: "#3b93f0",
    },
    StudioStarterSuggestion {
        id: "personal-organizer",
        title_zh: "个人整理师",
        title_en: "Personal Organizer",
        desc_zh: "把笔记、杂事和任务整理成简单的计划。",
        desc_en: "Turn notes, errands, and tasks into a simple plan.",
        prompt_zh: "把我的零散笔记、日常杂事和待办任务整理成清晰可执行的计划。",
        prompt_en: "Turn my notes, errands, and tasks into a clean, actionable daily plan.",
        shape: BloubShape::Hexagone,
        color_hex: "#f0b429",
    },
    StudioStarterSuggestion {
        id: "shopping-scout",
        title_zh: "购物侦察员",
        title_en: "Shopping Scout",
        desc_zh: "对比参数、口碑与价格，减少踩坑。",
        desc_en: "Compare specs, reviews, and prices before every purchase.",
        prompt_zh: "帮我研究商品参数、用户口碑和价格走势，客观分析优缺点。",
        prompt_en: "Research product specs, verified reviews, and prices and highlight trade-offs.",
        shape: BloubShape::Capsule,
        color_hex: "#3ecf8e",
    },
    StudioStarterSuggestion {
        id: "writing-partner",
        title_zh: "写作搭档",
        title_en: "Writing Partner",
        desc_zh: "列大纲、润色文稿并打磨表达。",
        desc_en: "Outline, edit, and sharpen memos and long-form drafts.",
        prompt_zh: "和我一起列大纲、润色文档并给出清晰的表达修改建议。",
        prompt_en: "Collaborate on outlines, edit prose clearly, and sharpen long-form drafts.",
        shape: BloubShape::Nuage,
        color_hex: "#5d67ee",
    },
    StudioStarterSuggestion {
        id: "learning-coach",
        title_zh: "学习教练",
        title_en: "Learning Coach",
        desc_zh: "拆解难点、设计练习并循序渐进。",
        desc_en: "Break down tough concepts with active practice.",
        prompt_zh: "用循序渐进的例子和练习题，帮我彻底掌握复杂概念。",
        prompt_en: "Teach tough concepts step by step with concise examples and active quizzes.",
        shape: BloubShape::Galet,
        color_hex: "#f08a24",
    },
];

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

fn template_plush_avatar_spec(idx: usize) -> String {
    let shape = STUDIO_FACE_CHOICES[idx % STUDIO_FACE_CHOICES.len()].0;
    format!("plush:{}", shape.as_str())
}

fn thinking_level_label(level: &str, lang: &str) -> &'static str {
    match level {
        "off" => tr_str(lang, "关", "Off"),
        "low" => tr_str(lang, "低", "Low"),
        "high" => tr_str(lang, "高", "High"),
        _ => tr_str(lang, "中", "Med"),
    }
}

fn resolve_studio_shape_and_expr(avatar_shape: &str) -> (BloubShape, BloubExpression) {
    let raw = avatar_shape
        .trim()
        .strip_prefix("plush:")
        .or_else(|| avatar_shape.trim().strip_prefix("PLUSH:"))
        .unwrap_or(avatar_shape.trim());
    let shape = BloubShape::parse(raw)
        .or_else(|| legacy_shape_to_bloub_shape(raw))
        .unwrap_or(BloubShape::Cercle);
    let expr = STUDIO_FACE_CHOICES
        .iter()
        .find(|(s, _)| *s == shape)
        .map(|(_, e)| *e)
        .unwrap_or(BloubExpression::Attentif);
    (shape, expr)
}

fn resolve_studio_hex(avatar_color: &str, auto_color: bool) -> String {
    if auto_color {
        return STUDIO_AUTO_HEX.to_string();
    }
    if let Some(rgb_u32) = parse_fabushi_hex_color(avatar_color) {
        return format!("#{:06x}", rgb_u32 & 0xFF_FFFF);
    }
    STUDIO_AUTO_HEX.to_string()
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
    pub auto_avatar_color: bool,
    pub model_picker_open: bool,
    pub advanced_open: bool,
    pub hero_wink_toggle: bool,
    pub ai_models: crate::views::drawer::InspectorAiModelCatalog,
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
            auto_avatar_color: true,
            model_picker_open: false,
            advanced_open: false,
            hero_wink_toggle: false,
            ai_models: crate::views::drawer::InspectorAiModelCatalog::default(),
        }
    }

    pub fn sync_ai_models(&mut self, catalog: crate::views::drawer::InspectorAiModelCatalog) {
        self.ai_models = catalog;
    }

    /// Returns the real model label shown on the Bot Studio model button (never hardcoded fake models).
    pub fn studio_model_display_label(&self, lang: &str) -> String {
        let trimmed = self.preferred_model.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        if !self.ai_models.needs_provider_selection && !self.ai_models.active_model.trim().is_empty()
        {
            self.ai_models.active_model.trim().to_string()
        } else if self.ai_models.synced && self.ai_models.needs_provider_selection {
            tr_str(lang, "未配置模型", "No model").to_string()
        } else {
            tr_str(lang, "跟随设置默认", "Follow Settings").to_string()
        }
    }

    /// Returns real selectable `(model_id, display_label)` choices sourced exclusively from Settings.
    pub fn studio_model_options(&self, lang: &str) -> Vec<(String, String)> {
        let follow_lbl = if !self.ai_models.needs_provider_selection
            && !self.ai_models.active_model.trim().is_empty()
        {
            if lang.starts_with("zh") {
                format!("跟随设置默认 ({})", self.ai_models.active_model.trim())
            } else {
                format!("Follow Settings ({})", self.ai_models.active_model.trim())
            }
        } else {
            tr_str(lang, "跟随设置默认", "Follow Settings").to_string()
        };
        let mut opts = vec![(String::new(), follow_lbl)];
        for model in self.ai_models.selectable_models() {
            let trimmed = model.trim();
            if !trimmed.is_empty()
                && !opts
                    .iter()
                    .any(|(id, _)| !id.is_empty() && id.eq_ignore_ascii_case(trimmed))
            {
                opts.push((trimmed.to_string(), trimmed.to_string()));
            }
        }
        opts
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
        self.avatar_shape = template_plush_avatar_spec(idx);
        self.avatar_color = tpl.avatar_color.to_ascii_lowercase();
        self.auto_avatar_color = false;
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
                && self.description.chars().count()
                    <= crate::views::openbot::agent_studio::FIRST_AGENT_PURPOSE_MAX_CHARS
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
        self.model_picker_open = false;
        self.status_message = None;
        if self.avatar_shape == "blob" && self.avatar_color == "blue" {
            self.avatar_shape = "plush:cercle".to_string();
            self.avatar_color = STUDIO_AUTO_HEX.to_string();
            self.auto_avatar_color = true;
        }
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

    /// Renders OpenBot's `FirstAgentSetup` avatar studio block: `72x72px` interactive 3D plush
    /// hero + `9x2` 18 circular color swatches (`#42a0ff` active ring) + 12 `33x33px` plush faces.
    fn render_studio_avatar_picker(
        &self,
        pal: TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let ob = pal.openbot_palette();
        let dark = pal.mode.is_dark();
        let accent_ring = ob.accent_text();
        let accent_soft = ob.accent_soft();
        let hover_bg = ob.hover();
        let border_dot = ob.border_strong();

        let (active_shape, active_expr) = resolve_studio_shape_and_expr(&self.avatar_shape);
        let active_hex = resolve_studio_hex(&self.avatar_color, self.auto_avatar_color);
        let hero_seed = if self.bot_name.trim().is_empty() {
            "new-agent".to_string()
        } else {
            self.bot_name.trim().to_string()
        };

        let hero_visual: AnyElement = if self.use_dynamic_avatar {
            let mut props = AgentAvatarProps::new(&hero_seed)
                .with_size(crate::views::openbot::agent_studio::FIRST_AGENT_HERO_SIZE_PX)
                .with_mood(if self.hero_wink_toggle {
                    AvatarMood::Responded
                } else {
                    AvatarMood::Idle
                })
                .with_hue_and_color(None, Some(active_hex.clone()));
            props.shape_override = Some(active_shape);
            props.expression_override = Some(if self.hero_wink_toggle {
                BloubExpression::Heureux
            } else {
                active_expr
            });
            props.eye_scale = 1.25;
            props.transition = PlushTransition::Cut;
            props.dark_scheme = dark;
            render_agent_avatar(&props, None, 0.0).into_any_element()
        } else {
            let bg_rgb = parse_fabushi_hex_color(&active_hex).unwrap_or(0x8B5CF6);
            let initials = classic_preview_initials(if self.bot_name.trim().is_empty() {
                "New agent"
            } else {
                self.bot_name.as_str()
            });
            div()
                .w(px(72.0))
                .h(px(72.0))
                .rounded_full()
                .bg(rgb(bg_rgb))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(24.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(initials)
                .into_any_element()
        };

        let hero_btn = div()
            .id("studio-hero-plush-btn")
            .cursor_pointer()
            .w(px(76.0))
            .h(px(76.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.hero_wink_toggle = !this.hero_wink_toggle;
                    cx.notify();
                }),
            )
            .child(hero_visual);

        // Exact 9x2 = 18 circular swatches: index 0 Auto (#8b5cf6) + 17 AVATAR_COLOR_CHOICES
        let mut swatch_cells: Vec<AnyElement> = Vec::with_capacity(18);
        let auto_selected = self.auto_avatar_color;
        swatch_cells.push(
            div()
                .id("studio-color-swatch-auto")
                .cursor_pointer()
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .when(auto_selected, |el| el.border_1().border_color(accent_ring))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.auto_avatar_color = true;
                        this.avatar_color = STUDIO_AUTO_HEX.to_string();
                        this.use_dynamic_avatar = true;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(18.0))
                        .h(px(18.0))
                        .rounded_full()
                        .bg(rgb(0x8B5CF6))
                        .border_1()
                        .border_color(border_dot),
                )
                .into_any_element(),
        );
        for (idx, choice) in AVATAR_COLOR_CHOICES.iter().enumerate() {
            let swatch_hex = match *choice {
                AvatarColorChoice::Hue { swatch, .. }
                | AvatarColorChoice::Preset { swatch, .. } => swatch,
            };
            let normalized_hex = swatch_hex.to_ascii_lowercase();
            let is_selected = !self.auto_avatar_color
                && active_hex.eq_ignore_ascii_case(&normalized_hex);
            let swatch_rgba =
                parse_css_color_to_rgba(swatch_hex).unwrap_or_else(|| rgb(0x3B93F0));
            let hex_for_click = normalized_hex.clone();
            swatch_cells.push(
                div()
                    .id(SharedString::from(format!("studio-color-swatch-{idx}")))
                    .cursor_pointer()
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded_full()
                    .when(is_selected, |el| el.border_1().border_color(accent_ring))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.auto_avatar_color = false;
                            this.avatar_color = hex_for_click.clone();
                            this.use_dynamic_avatar = true;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w(px(18.0))
                            .h(px(18.0))
                            .rounded_full()
                            .bg(swatch_rgba)
                            .border_1()
                            .border_color(border_dot),
                    )
                    .into_any_element(),
            );
        }
        let row_1: Vec<AnyElement> = swatch_cells.drain(..9.min(swatch_cells.len())).collect();
        let row_2: Vec<AnyElement> = swatch_cells;
        let color_grid = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .child(div().flex().items_center().justify_center().gap(px(4.0)).children(row_1))
            .child(div().flex().items_center().justify_center().gap(px(4.0)).children(row_2));

        // Exact 12 selectable 33x33px 3D plush faces (24x24px preview, rainbow in Auto mode)
        const RAINBOW_AUTO_HEX: [&str; 12] = [
            "#8b5cf6", "#e8483f", "#3b93f0", "#e152b0", "#5d67ee", "#f08a24",
            "#38bdf8", "#8b5cf6", "#e8483f", "#f27ba5", "#3b93f0", "#e8483f",
        ];
        let mut face_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(px(2.0));
        for (idx, (shape, expr)) in STUDIO_FACE_CHOICES.iter().enumerate() {
            let is_selected = self.use_dynamic_avatar && *shape == active_shape;
            let face_color = if self.auto_avatar_color {
                RAINBOW_AUTO_HEX[idx].to_string()
            } else {
                active_hex.clone()
            };
            let face_seed = format!("studio-face-{idx}-{}", shape.as_str());
            let mut fp = AgentAvatarProps::new(&face_seed)
                .with_size(24.0)
                .with_hue_and_color(None, Some(face_color));
            fp.shape_override = Some(*shape);
            fp.expression_override = Some(*expr);
            fp.eye_scale = 1.25;
            fp.transition = PlushTransition::Cut;
            fp.dark_scheme = dark;
            let shape_spec = format!("plush:{}", shape.as_str());
            face_row = face_row.child(
                div()
                    .id(SharedString::from(format!("studio-face-choice-{idx}")))
                    .cursor_pointer()
                    .w(px(33.0))
                    .h(px(33.0))
                    .rounded(px(8.0))
                    .when(is_selected, |el| {
                        el.bg(accent_soft).border_1().border_color(accent_ring)
                    })
                    .hover(move |s| if is_selected { s } else { s.bg(hover_bg) })
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.avatar_shape = shape_spec.clone();
                            this.use_dynamic_avatar = true;
                            cx.notify();
                        }),
                    )
                    .child(render_agent_avatar(&fp, None, 0.0)),
            );
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .child(hero_btn)
            .child(color_grid)
            .child(face_row)
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
    /// Renders OpenBot's bottom `推荐` horizontal suggestion carousel (`256x112px` cards,
    /// `42x42px` 3D plush, 6 OpenBot starter suggestions + all 24 Orca `BUILTIN_BOT_TEMPLATES`).
    fn render_studio_suggestions_carousel(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let ob = pal.openbot_palette();
        let border = ob.border_strong_hairline();
        let hover_bg = ob.hover();
        let active_bg = ob.bg_control_active();
        let accent_ring = ob.accent_text();
        let accent_soft = ob.accent_soft();
        let text_pri = ob.text_primary();
        let text_muted = ob.text_muted();
        let is_zh = lang.starts_with("zh");

        let categories: [Option<BotTemplateCategory>; 7] = [
            None,
            Some(BotTemplateCategory::Personal),
            Some(BotTemplateCategory::Planning),
            Some(BotTemplateCategory::Coding),
            Some(BotTemplateCategory::Support),
            Some(BotTemplateCategory::Ops),
            Some(BotTemplateCategory::Releases),
        ];

        let cat_pills = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .flex_wrap()
            .children(categories.into_iter().enumerate().map(|(idx, cat)| {
                let active = self.template_category_filter == cat;
                let label = match cat {
                    None => tr_str(lang, "全部", "All"),
                    Some(c) => {
                        if is_zh {
                            template_category_label_zh(c)
                        } else {
                            c.label()
                        }
                    }
                };
                div()
                    .id(SharedString::from(format!("studio-tpl-cat-{idx}")))
                    .cursor_pointer()
                    .h(px(22.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .when(active, |el| el.bg(active_bg))
                    .hover(move |s| if active { s } else { s.bg(hover_bg) })
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active { text_pri } else { text_muted })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.template_category_filter = cat;
                            cx.notify();
                        }),
                    )
                    .child(label)
            }));

        let mut cards: Vec<AnyElement> = Vec::new();

        if self.template_category_filter.is_none() {
            for (idx, sug) in STUDIO_STARTER_SUGGESTIONS.iter().enumerate() {
                let selected = self.selected_template_id.as_deref() == Some(sug.id);
                let title = if is_zh { sug.title_zh } else { sug.title_en };
                let desc = if is_zh { sug.desc_zh } else { sug.desc_en };
                let prompt = if is_zh { sug.prompt_zh } else { sug.prompt_en };
                let plush = render_plush_bot_avatar(
                    sug.id,
                    Some(sug.shape),
                    None,
                    Some(sug.color_hex),
                    crate::views::openbot::agent_studio::FIRST_AGENT_REC_CARD_AVATAR_PX,
                    AvatarMood::Idle,
                    None,
                );
                let sug_id = sug.id;
                let sug_shape = format!("plush:{}", sug.shape.as_str());
                let sug_color = sug.color_hex;
                cards.push(
                    div()
                        .id(SharedString::from(format!("studio-starter-card-{idx}")))
                        .cursor_pointer()
                        .w(px(crate::views::openbot::agent_studio::FIRST_AGENT_REC_CARD_WIDTH_PX))
                        .h(px(crate::views::openbot::agent_studio::FIRST_AGENT_REC_CARD_HEIGHT_PX))
                        .flex_shrink_0()
                        .p(px(16.0))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(if selected { accent_ring } else { border })
                        .when(selected, |el| el.bg(accent_soft))
                        .hover(move |s| if selected { s } else { s.bg(hover_bg) })
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.bot_name = title.to_string();
                                this.description = prompt.to_string();
                                this.avatar_shape = sug_shape.clone();
                                this.avatar_color = sug_color.to_string();
                                this.auto_avatar_color = false;
                                this.use_dynamic_avatar = true;
                                this.selected_template_id = Some(sug_id.to_string());
                                this.selected_template_skills.clear();
                                this.is_template_mode = false;
                                this.is_coding_agent_mode = false;
                                this.is_ai_mode = false;
                                this.is_fabushi_mode = true;
                                this.active_field = 1;
                                cx.notify();
                            }),
                        )
                        .child(plush)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .w_full()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_pri)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .line_height(px(18.0))
                                        .text_color(text_muted)
                                        .child(desc),
                                ),
                        )
                        .into_any_element(),
                );
            }
        }

        for (global_idx, tpl) in BUILTIN_BOT_TEMPLATES.iter().enumerate() {
            if let Some(cat) = self.template_category_filter {
                if tpl.category != cat {
                    continue;
                }
            }
            let selected = self.selected_template_id.as_deref() == Some(tpl.id);
            let shape = STUDIO_FACE_CHOICES[global_idx % STUDIO_FACE_CHOICES.len()].0;
            let color_hex = parse_fabushi_hex_color(tpl.avatar_color)
                .map(|c| format!("#{:06x}", c & 0xFF_FFFF));
            let plush = render_plush_bot_avatar(
                tpl.id,
                Some(shape),
                None,
                color_hex.as_deref(),
                42.0,
                AvatarMood::Idle,
                None,
            );
            cards.push(
                div()
                    .id(SharedString::from(format!("studio-orca-tpl-card-{global_idx}")))
                    .cursor_pointer()
                    .w(px(256.0))
                    .h(px(112.0))
                    .flex_shrink_0()
                    .p(px(16.0))
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(if selected { accent_ring } else { border })
                    .when(selected, |el| el.bg(accent_soft))
                    .hover(move |s| if selected { s } else { s.bg(hover_bg) })
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some(t) = BUILTIN_BOT_TEMPLATES.get(global_idx) {
                                this.apply_template(t, global_idx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(plush)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_pri)
                                    .child(tpl.name),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .line_height(px(18.0))
                                    .text_color(text_muted)
                                    .child(tpl.tagline),
                            ),
                    )
                    .into_any_element(),
            );
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_muted)
                            .child(tr_str(lang, "推荐", "Suggestions")),
                    )
                    .child(cat_pills),
            )
            .child(
                div()
                    .id("studio-suggestions-carousel-scroll")
                    .w_full()
                    .min_w(px(0.0))
                    .overflow_x_scroll()
                    .pb(px(4.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .children(cards),
            )
    }
}

impl Render for CreateManagedBotBoxPeers {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.as_str();
        let is_coding = self.is_coding_agent_mode;
        let is_template = self.is_template_mode && !is_coding;
        let is_fabushi = (self.is_fabushi_mode || is_template) && !is_coding;
        let is_ai = self.is_ai_mode && !is_coding && !is_fabushi;
        let is_token = !is_coding && !is_fabushi && !is_ai;
        let prompt = self.nl_prompt.clone();
        let can_submit = if is_coding || is_fabushi {
            self.is_valid() && !self.is_submitting
        } else if is_ai {
            !prompt.trim().is_empty() && !self.is_submitting
        } else {
            self.is_valid() && !self.is_submitting
        };

        let bg_canvas = ob.bg_canvas();
        let bg_control = ob.bg_control();
        let bg_surface = ob.bg_surface();
        let bg_active = ob.bg_control_active();
        let hover_bg = ob.hover();
        let border_subtle = ob.border();
        let border_strong = ob.border_strong_hairline();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();
        let danger = ob.danger();
        let cta_bg = ob.surface_light();
        let cta_fg = ob.text_on_light();

        let (active_shape, _) = resolve_studio_shape_and_expr(&self.avatar_shape);
        let active_hex = resolve_studio_hex(&self.avatar_color, self.auto_avatar_color);
        let header_name = if self.bot_name.trim().is_empty() {
            tr_str(lang, "新助手", "New agent").to_string()
        } else {
            self.bot_name.trim().to_string()
        };
        let header_plush = render_plush_bot_avatar(
            &header_name,
            Some(active_shape),
            None,
            Some(&active_hex),
            18.0,
            AvatarMood::Idle,
            None,
        );

        let mode_pill = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .cursor_pointer()
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .when(active, |el| el.bg(bg_active))
                .hover(move |s| if active { s } else { s.bg(hover_bg) })
                .flex()
                .items_center()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active { text_pri } else { text_muted })
                .child(label)
        };

        let header = div()
            .id("create-bot-studio-header")
            .h(px(38.0))
            .flex_shrink_0()
            .w_full()
            .px(px(12.0))
            .border_b_1()
            .border_color(border_subtle)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(header_plush)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(text_pri)
                            .child(header_name),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .flex_shrink_0()
                    .child(
                        mode_pill("create-bot-tab-fabushi", tr_str(lang, "助手", "Agent"), is_fabushi)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open_fabushi_creator();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        mode_pill("create-bot-tab-coding", tr_str(lang, "编程", "Coding"), is_coding)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open_coding_agent_creator();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        mode_pill("create-bot-tab-ai", "AI", is_ai).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.open_ai_creator();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        mode_pill("create-bot-tab-manual", "Token", is_token).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.is_coding_agent_mode = false;
                                this.is_template_mode = false;
                                this.is_fabushi_mode = false;
                                this.is_ai_mode = false;
                                this.active_field = 1;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(div().w(px(1.0)).h(px(14.0)).mx(px(4.0)).bg(border_strong))
                    .child(
                        div()
                            .id("create-managed-bot-close-btn")
                            .cursor_pointer()
                            .h(px(26.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(12.0))
                            .text_color(text_muted)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.is_visible = false;
                                    this.pending_close = true;
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "取消", "Cancel")),
                    ),
            );

        let model_display = self.studio_model_display_label(lang);
        let think_display = thinking_level_label(&self.thinking_level, lang);
        let studio_model_opts = self.studio_model_options(lang);
        let has_live_studio_models = studio_model_opts.len() > 1;
        let studio_models_synced = self.ai_models.synced;

        let fabushi_body = is_fabushi.then(|| {
            let avatar_picker = self.render_studio_avatar_picker(pal, cx);
            let suggestions = self.render_studio_suggestions_carousel(pal, lang, cx);
            let name_active = self.active_field == 1;
            let desc_active = self.active_field == 3;
            let user_active = self.active_field == 2;
            let model_field_active = self.active_field == 4;

            let model_dropdown = self.model_picker_open.then(|| {
                let mut opts_col = div()
                    .id("studio-model-picker-scroll")
                    .max_h(px(200.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(studio_model_opts.into_iter().enumerate().map(|(idx, (id_val, lbl))| {
                        let selected = self.preferred_model.trim().eq_ignore_ascii_case(&id_val);
                        let owned = id_val.clone();
                        div()
                            .id(SharedString::from(format!("studio-model-opt-{idx}")))
                            .cursor_pointer()
                            .min_h(px(30.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .when(selected, |el| el.bg(bg_active))
                            .hover(move |s| if selected { s } else { s.bg(hover_bg) })
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(13.0))
                            .text_color(if selected { text_pri } else { text_sec })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.preferred_model = owned.clone();
                                    this.model_picker_open = false;
                                    cx.notify();
                                }),
                            )
                            .child(lbl)
                    }));
                if !has_live_studio_models {
                    opts_col = opts_col.child(
                        div()
                            .px(px(8.0))
                            .py(px(6.0))
                            .text_size(px(11.5))
                            .text_color(text_muted)
                            .child(if studio_models_synced {
                                tr_str(
                                    lang,
                                    "设置中尚未配置可用模型，请先在「设置 → AI 模型供应商」中配置",
                                    "No models configured yet — configure a provider in Settings → AI Providers",
                                )
                            } else {
                                tr_str(
                                    lang,
                                    "正在同步设置中的模型列表…",
                                    "Syncing available models from Settings…",
                                )
                            }),
                    );
                }
                div()
                    .id("studio-model-picker-dropdown")
                    .w_full()
                    .p(px(8.0))
                    .rounded(px(10.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(border_strong)
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(opts_col)
                    .child(div().h(px(1.0)).w_full().bg(border_subtle))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(text_muted)
                                    .child(tr_str(lang, "推理深度", "Reasoning")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .children(["off", "low", "medium", "high"].into_iter().enumerate().map(|(idx, lvl)| {
                                        let active = self.thinking_level == lvl;
                                        let owned = lvl.to_string();
                                        div()
                                            .id(SharedString::from(format!("fab-think-pick-{idx}")))
                                            .cursor_pointer()
                                            .h(px(24.0))
                                            .px(px(8.0))
                                            .rounded(px(6.0))
                                            .when(active, |el| el.bg(bg_active))
                                            .hover(move |s| if active { s } else { s.bg(hover_bg) })
                                            .flex()
                                            .items_center()
                                            .text_size(px(12.0))
                                            .text_color(if active { text_pri } else { text_muted })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.thinking_level = owned.clone();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(thinking_level_label(lvl, lang))
                                    })),
                            ),
                    )
            });

            let advanced_section = self.advanced_open.then(|| {
                div()
                    .w_full()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(bg_control)
                    .border_1()
                    .border_color(border_strong)
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("fab-field-username")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(bg_canvas)
                            .border_1()
                            .border_color(if user_active { text_muted } else { border_strong })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.active_field = 2;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(text_muted)
                                    .child(tr_str(lang, "@用户名 (可选，自动生成 *_bot)", "@username (optional)")),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(if self.bot_username.is_empty() { text_muted } else { text_pri })
                                    .child(if self.bot_username.is_empty() {
                                        "auto_agent_bot".to_string()
                                    } else {
                                        self.bot_username.clone()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("fab-field-custom-model")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(bg_canvas)
                            .border_1()
                            .border_color(if model_field_active { text_muted } else { border_strong })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.active_field = 4;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(text_muted)
                                    .child(tr_str(lang, "自定义模型 ID (可选)", "Custom model ID (optional)")),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(if self.preferred_model.is_empty() { text_muted } else { text_pri })
                                    .child(if self.preferred_model.is_empty() {
                                        tr_str(lang, "留空继承默认模型", "Inherit default").to_string()
                                    } else {
                                        self.preferred_model.clone()
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
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(div().text_size(px(11.5)).text_color(text_muted).child(tr_str(lang, "审批", "Approval")))
                                    .children(["prompt", "autonomous", "strict"].into_iter().enumerate().map(|(idx, mode)| {
                                        let active = self.approval_mode == mode;
                                        let owned = mode.to_string();
                                        div()
                                            .id(SharedString::from(format!("fab-app-pick-{idx}")))
                                            .cursor_pointer()
                                            .h(px(22.0))
                                            .px(px(7.0))
                                            .rounded(px(6.0))
                                            .when(active, |el| el.bg(bg_active))
                                            .hover(move |s| if active { s } else { s.bg(hover_bg) })
                                            .flex()
                                            .items_center()
                                            .text_size(px(11.0))
                                            .text_color(if active { text_pri } else { text_muted })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.approval_mode = owned.clone();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(if mode == "autonomous" { "auto" } else { mode })
                                    })),
                            )
                            .child(
                                div()
                                    .id("studio-toggle-classic-avatar")
                                    .cursor_pointer()
                                    .h(px(22.0))
                                    .px(px(7.0))
                                    .rounded(px(6.0))
                                    .when(!self.use_dynamic_avatar, |el| el.bg(bg_active))
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .text_size(px(11.0))
                                    .text_color(if !self.use_dynamic_avatar { text_pri } else { text_muted })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.use_dynamic_avatar = !this.use_dynamic_avatar;
                                            cx.notify();
                                        }),
                                    )
                                    .child(if self.use_dynamic_avatar {
                                        tr_str(lang, "3D 毛绒头像", "3D Plush")
                                    } else {
                                        tr_str(lang, "经典首字母", "Classic")
                                    }),
                            ),
                    )
            });

            div()
                .w_full()
                .flex()
                .flex_col()
                .justify_between()
                .gap(px(24.0))
                .child(
                    div()
                        .w(px(420.0))
                        .max_w_full()
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.0))
                        .child(avatar_picker)
                        .child(
                            div()
                                .w_full()
                                .mt(px(4.0))
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(3.0))
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(text_sec)
                                                .child(tr_str(lang, "名称", "Name"))
                                                .child(div().text_color(danger).child("*")),
                                        )
                                        .child(
                                            div()
                                                .id("fab-field-1")
                                                .cursor_pointer()
                                                .w_full()
                                                .h(px(38.0))
                                                .px(px(12.0))
                                                .rounded(px(8.0))
                                                .bg(bg_control)
                                                .border_1()
                                                .border_color(if name_active { text_muted } else { border_strong })
                                                .flex()
                                                .items_center()
                                                .text_size(px(14.0))
                                                .text_color(if self.bot_name.is_empty() { text_muted } else { text_pri })
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.active_field = 1;
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(if self.bot_name.is_empty() {
                                                    tr_str(lang, "新助手", "New agent").to_string()
                                                } else if name_active {
                                                    format!("{}|", self.bot_name)
                                                } else {
                                                    self.bot_name.clone()
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(text_sec)
                                                .child(tr_str(
                                                    lang,
                                                    "这个助手要帮你做什么？",
                                                    "What should this agent help you with?",
                                                )),
                                        )
                                        .child(
                                            div()
                                                .id("fab-field-3")
                                                .cursor_pointer()
                                                .w_full()
                                                .min_h(px(64.0))
                                                .max_h(px(180.0))
                                                .p(px(10.0))
                                                .rounded(px(8.0))
                                                .bg(bg_control)
                                                .border_1()
                                                .border_color(if desc_active { text_muted } else { border_strong })
                                                .text_size(px(13.5))
                                                .line_height(px(19.0))
                                                .text_color(if self.description.is_empty() { text_muted } else { text_pri })
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.active_field = 3;
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(if self.description.is_empty() {
                                                    tr_str(
                                                        lang,
                                                        "规划行程、比较方案，或者处理日常工作。",
                                                        "Plan trips, compare options, or handle daily work.",
                                                    )
                                                    .to_string()
                                                } else if desc_active {
                                                    format!("{}|", self.description)
                                                } else {
                                                    self.description.clone()
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("studio-model-selector-row")
                                        .cursor_pointer()
                                        .w_full()
                                        .h(px(44.0))
                                        .px(px(12.0))
                                        .rounded(px(8.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(if self.model_picker_open { text_muted } else { border_strong })
                                        .hover(move |s| s.bg(bg_active))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.model_picker_open = !this.model_picker_open;
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(text_pri)
                                                .child(tr_str(lang, "模型", "Model")),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .text_size(px(13.0))
                                                .text_color(text_sec)
                                                .child(format!("{model_display} · {think_display}"))
                                                .child(tg_icon_rgba(IconName::ChevronDown, 12.0, text_muted)),
                                        ),
                                )
                                .when_some(model_dropdown, |el, dd| el.child(dd))
                                .child(
                                    div()
                                        .id("studio-advanced-toggle")
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap(px(4.0))
                                        .text_size(px(11.5))
                                        .text_color(text_muted)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.advanced_open = !this.advanced_open;
                                                cx.notify();
                                            }),
                                        )
                                        .child(if self.advanced_open {
                                            tr_str(lang, "收起高级选项", "Hide advanced")
                                        } else {
                                            tr_str(lang, "高级选项 (@用户名 / 审批)", "Advanced (@username / approval)")
                                        }),
                                )
                                .when_some(advanced_section, |el, adv| el.child(adv))
                                .when_some(self.status_message.clone(), |el, msg| {
                                    el.child(div().text_size(px(12.0)).text_color(ob.success()).child(msg))
                                })
                                .child(
                                    div()
                                        .mt(px(4.0))
                                        .w_full()
                                        .flex()
                                        .justify_center()
                                        .child(
                                            div()
                                                .id("create-managed-bot-submit-btn")
                                                .cursor_pointer()
                                                .min_w(px(132.0))
                                                .h(px(36.0))
                                                .px(px(18.0))
                                                .rounded(px(8.0))
                                                .bg(cta_bg)
                                                .when(!can_submit, |el| el.opacity(0.42))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(cta_fg)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.submit();
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(if self.is_submitting {
                                                    tr_str(lang, "正在创建助手…", "Creating agent…")
                                                } else {
                                                    tr_str(lang, "创建助手", "Create agent")
                                                }),
                                        ),
                                ),
                        ),
                )
                .child(suggestions)
        });

        let presets: [(&str, &str); 3] = [
            ("☕ 咖啡点单", "创建一个精品咖啡点单和会员积分小程序"),
            ("✅ 团队待办", "创建一个支持分配负责人、优先级和 AI 总结的待办清单小程序"),
            ("📊 投票决策", "创建一个实时团队投票和数据分析小程序"),
        ];

        let secondary_submit = (!is_fabushi).then(|| {
            div()
                .mt(px(8.0))
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("create-managed-bot-submit-btn")
                        .cursor_pointer()
                        .min_w(px(164.0))
                        .h(px(36.0))
                        .px(px(18.0))
                        .rounded(px(8.0))
                        .bg(cta_bg)
                        .when(!can_submit, |el| el.opacity(0.42))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cta_fg)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.submit();
                                cx.notify();
                            }),
                        )
                        .child(if self.is_submitting {
                            tr_str(lang, "正在创建…", "Provisioning…")
                        } else if is_coding {
                            tr_str(lang, "创建编程助手", "Create Coding Agent")
                        } else if is_ai {
                            tr_str(lang, "立即生成助手", "Create AI Agent")
                        } else {
                            tr_str(lang, "保存并连接", "Connect Bot")
                        }),
                )
        });

        div()
            .id("create-managed-bot-modal")
            .w(px(860.0))
            .max_w(px(900.0))
            .max_h(px(740.0))
            .rounded(px(14.0))
            .bg(bg_canvas)
            .border_1()
            .border_color(ob.border_hairline())
            .shadow(pal.popover_shadows())
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(header)
            .child(
                div()
                    .id("create-bot-studio-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(24.0))
                    .py(px(20.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .when_some(fabushi_body, |el, body| el.child(body))
                    .when(is_coding, |el| {
                        el.child(
                            div()
                                .w(px(560.0))
                                .max_w_full()
                                .mx_auto()
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .child(self.render_coding_agent_panel(pal, lang, cx))
                                .when_some(self.status_message.clone(), |el, msg| {
                                    el.child(div().text_size(px(12.0)).text_color(ob.success()).child(msg))
                                }),
                        )
                    })
                    .when(is_ai, |el| {
                        el.child(
                            div()
                                .w(px(460.0))
                                .max_w_full()
                                .mx_auto()
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(text_sec)
                                        .child(tr_str(
                                            lang,
                                            "描述你想要的助手与小程序，系统将自动创建可交互 Mini App 并绑定专属 AI 助手：",
                                            "Describe the Bot + Mini App you want to generate:",
                                        )),
                                )
                                .child(
                                    div()
                                        .id("create-managed-bot-nl-input")
                                        .w_full()
                                        .min_h(px(90.0))
                                        .p(px(12.0))
                                        .rounded(px(8.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .text_size(px(13.5))
                                        .text_color(if prompt.is_empty() { text_muted } else { text_pri })
                                        .child(if prompt.is_empty() {
                                            tr_str(
                                                lang,
                                                "例如：帮我做一个极简咖啡点单小程序和 AI 店长…",
                                                "e.g. Build a specialty coffee ordering Mini App with AI barista…",
                                            )
                                            .to_string()
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
                                                .px(px(10.0))
                                                .py(px(4.0))
                                                .rounded(px(8.0))
                                                .bg(bg_control)
                                                .border_1()
                                                .border_color(border_strong)
                                                .hover(move |s| s.bg(hover_bg))
                                                .text_size(px(12.0))
                                                .text_color(text_sec)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.nl_prompt = full_owned.clone();
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(pill)
                                        })),
                                ),
                        )
                    })
                    .when(is_token, |el| {
                        let fields: [(u8, &str, String); 3] = [
                            (1, tr_str(lang, "助手名称", "Bot Name"), self.bot_name.clone()),
                            (2, tr_str(lang, "@用户名 (以 bot 结尾)", "@username (ends with bot)"), self.bot_username.clone()),
                            (3, "HTTP API Token", self.bot_token.clone()),
                        ];
                        el.child(
                            div()
                                .w(px(420.0))
                                .max_w_full()
                                .mx_auto()
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .children(fields.into_iter().map(|(fid, label, val)| {
                                    let active = self.active_field == fid;
                                    div()
                                        .id(("adv-bot-field", fid as u64))
                                        .cursor_pointer()
                                        .p(px(10.0))
                                        .rounded(px(8.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(if active { text_muted } else { border_strong })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.active_field = fid;
                                                cx.notify();
                                            }),
                                        )
                                        .child(div().text_size(px(11.5)).text_color(text_muted).child(label))
                                        .child(
                                            div()
                                                .text_size(px(13.5))
                                                .text_color(if val.is_empty() { text_muted } else { text_pri })
                                                .child(if val.is_empty() { "—".to_string() } else { val }),
                                        )
                                })),
                        )
                    })
                    .when_some(secondary_submit, |el, btn| el.child(btn)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::grok_bot_avatar::GROK_COLOR_PALETTE;
    use gpui_telegram_proto::miniapp_contracts::GROK_AVATAR_SHAPES;

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
