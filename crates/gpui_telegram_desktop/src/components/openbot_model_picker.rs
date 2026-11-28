//! Shared OpenBot-styled Provider Icon set & Two-Column `ProviderModelPicker` popover.
//!
//! Used consistently across:
//! - `CreateManagedBotBoxPeers` ("新助手 / New agent" modal model selector)
//! - `ConversationStage` (chat header model selector pill & popover)
//! - `SettingsAiProvidersSections` ("服务商 / Providers" settings page)

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::net::hermes_client::MagpieStateDto;
use gpui_telegram_proto::{CodingProviderKind, DetectedCodingProvider};

use crate::i18n::tr_str;
use crate::theme::TdesktopThemePalette;
use crate::views::settings::sections::settings_ai_providers::{
    sub_dto_to_account_dto, SubscriptionAccountDto,
};
use crate::views::stage::openbot_stage_chrome::{openbot_stage_icon, OpenBotStageIcon};

/// Canonical reasoning effort / strength levels matching OpenBot's `强度` dropdown.
pub const OPENBOT_EFFORT_CHOICES: [(&str, &str, &str); 5] = [
    ("off", "关", "Off"),
    ("low", "低", "Low"),
    ("medium", "中", "Medium"),
    ("high", "高", "High"),
    ("xhigh", "超高", "Max"),
];

/// Returns the localized short label for a reasoning effort level (`关` / `低` / `中` / `高` / `超高`).
pub fn openbot_effort_label(effort: &str, lang: &str) -> &'static str {
    match effort.trim().to_ascii_lowercase().as_str() {
        "off" | "none" => tr_str(lang, "关", "Off"),
        "minimal" | "low" => tr_str(lang, "低", "Low"),
        "high" => tr_str(lang, "高", "High"),
        "xhigh" | "max" | "ultra" => tr_str(lang, "超高", "Max"),
        _ => tr_str(lang, "中", "Medium"),
    }
}

/// Normalizes any provider/subscription/preset identifier into a canonical vector icon key.
pub fn canonical_provider_icon_key(raw: &str) -> &'static str {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.contains("codex") || lower.contains("chatgpt") || lower.contains("openai") {
        "codex"
    } else if lower.contains("claude") || lower.contains("anthropic") {
        "claude"
    } else if lower.contains("opencode") {
        "opencode"
    } else if lower.contains("grok") || lower == "xai" || lower.contains("x.ai") {
        "grok"
    } else if lower.contains("gemini")
        || lower.contains("vertex")
        || lower.contains("antigravity")
        || lower.contains("google")
    {
        "gemini"
    } else if lower.contains("cursor") {
        "cursor"
    } else if lower.contains("cline") {
        "cline"
    } else if lower == "pi" || lower.starts_with("pi-") || lower.starts_with("pi:") {
        "pi"
    } else if lower.contains("deepseek") {
        "deepseek"
    } else if lower.contains("kimi") || lower.contains("moonshot") {
        "kimi"
    } else if lower.contains("copilot") {
        "copilot"
    } else if lower.contains("openrouter") {
        "openrouter"
    } else if lower.contains("mistral") {
        "mistral"
    } else if lower.contains("ollama")
        || lower.contains("lmstudio")
        || lower.contains("vllm")
        || lower.contains("local")
    {
        "ollama"
    } else if lower.contains("zhipu") || lower.contains("glm") {
        "zhipu"
    } else if lower.contains("qwen") || lower.contains("dashscope") || lower.contains("aliyun") {
        "qwen"
    } else {
        "generic"
    }
}

/// Infers a canonical provider icon key from a model ID when no explicit provider is known.
pub fn infer_provider_icon_key_from_model(model_id: &str) -> &'static str {
    let lower = model_id.trim().to_ascii_lowercase();
    if lower.is_empty()
        || lower.starts_with("gpt-")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.contains("codex")
    {
        "codex"
    } else if lower.contains("claude") {
        "claude"
    } else if lower.contains("gemini") {
        "gemini"
    } else if lower.contains("grok") {
        "grok"
    } else if lower.contains("deepseek") {
        "deepseek"
    } else if lower.contains("kimi") || lower.contains("moonshot") {
        "kimi"
    } else if lower.contains("mistral") || lower.contains("codestral") {
        "mistral"
    } else if lower.contains("glm") {
        "zhipu"
    } else if lower.contains("qwen") {
        "qwen"
    } else {
        "generic"
    }
}

/// Maps raw subscription/provider identifiers to clean OpenBot display names (`ChatGPT`, `Claude`, etc.).
pub fn friendly_provider_name(kind_or_id: &str, raw_label: &str) -> String {
    let lower = kind_or_id.trim().to_ascii_lowercase();
    match lower.as_str() {
        "codex" | "codex_chatgpt" | "chatgpt" => return "ChatGPT".to_string(),
        "claude" | "claude_code" | "anthropic" => return "Claude".to_string(),
        "opencode" => return "OpenCode".to_string(),
        "grok" | "xai" => return "Grok".to_string(),
        "google_vertex" | "google-vertex" => return "Google Vertex AI".to_string(),
        "gemini" | "gemini_code_assist" | "antigravity" => return "Gemini".to_string(),
        "cursor" | "cursor_cli" => return "Cursor".to_string(),
        "cline" => return "Cline".to_string(),
        "pi" => return "Pi".to_string(),
        "github_copilot" | "copilot" => return "GitHub Copilot".to_string(),
        "deepseek" => return "DeepSeek".to_string(),
        "kimi" | "moonshot" => return "Kimi".to_string(),
        "openrouter" => return "OpenRouter".to_string(),
        "mistral" => return "Mistral".to_string(),
        "ollama" => return "Ollama".to_string(),
        _ => {}
    }
    let cleaned = raw_label
        .trim()
        .strip_prefix("Magpie ")
        .unwrap_or(raw_label.trim())
        .trim_end_matches(" (Magpie)")
        .trim();
    if let Some((head, _)) = cleaned.split_once(" (") {
        let h = head.trim();
        if !h.is_empty() {
            if h.eq_ignore_ascii_case("codex") || h.eq_ignore_ascii_case("codex / chatgpt") {
                return "ChatGPT".to_string();
            }
            return h.to_string();
        }
    }
    if !cleaned.is_empty() {
        cleaned.to_string()
    } else {
        kind_or_id.to_string()
    }
}

/// Formats model IDs cleanly like OpenBot (`gpt-6-luna` -> `GPT-6-Luna`, `gpt-6.1-terra` -> `GPT-6.1 Terra`).
pub fn format_model_display_label(model_id: &str) -> String {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return "GPT-6-Luna".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("gpt-") {
        let parts: Vec<&str> = rest.split('-').collect();
        if parts.len() == 2 && parts[1] == "terra" {
            return format!("GPT-{} Terra", parts[0]);
        }
        let mut out = String::from("GPT");
        for part in parts {
            out.push('-');
            if part.eq_ignore_ascii_case("4o") {
                out.push_str("4o");
            } else if part.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                let mut chars = part.chars();
                if let Some(first) = chars.next() {
                    out.push(first.to_ascii_uppercase());
                    out.push_str(chars.as_str());
                }
            } else {
                out.push_str(part);
            }
        }
        return out;
    }
    trimmed.to_string()
}

fn stroke_poly(
    window: &mut Window,
    origin: Point<Pixels>,
    scale: f32,
    stroke_px: f32,
    pts: &[(f32, f32)],
    color: Rgba,
) {
    if pts.len() < 2 {
        return;
    }
    let ox = f32::from(origin.x);
    let oy = f32::from(origin.y);
    let mut b = PathBuilder::stroke(px(stroke_px));
    for (idx, (x, y)) in pts.iter().enumerate() {
        let p = point(px(ox + x * scale), px(oy + y * scale));
        if idx == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

fn fill_poly(
    window: &mut Window,
    origin: Point<Pixels>,
    scale: f32,
    pts: &[(f32, f32)],
    color: Rgba,
) {
    if pts.len() < 3 {
        return;
    }
    let ox = f32::from(origin.x);
    let oy = f32::from(origin.y);
    let mut b = PathBuilder::fill();
    for (idx, (x, y)) in pts.iter().enumerate() {
        let p = point(px(ox + x * scale), px(oy + y * scale));
        if idx == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// Renders a crisp double vertical chevron (`↕`) matching OpenBot's select trigger buttons.
pub fn render_chevrons_up_down(size_px: f32, color: Rgba) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let s = (size_px / 24.0).max(0.25);
            let w = (1.9 * s).clamp(1.0, 2.2);
            let o = bounds.origin;
            stroke_poly(window, o, s, w, &[(7.5, 9.0), (12.0, 4.5), (16.5, 9.0)], color);
            stroke_poly(window, o, s, w, &[(7.5, 15.0), (12.0, 19.5), (16.5, 15.0)], color);
        },
    )
    .w(px(size_px))
    .h(px(size_px))
    .flex_shrink_0()
    .into_any_element()
}

/// Renders a crisp OpenBot-style vector brand icon for any AI provider or preset key.
pub fn render_provider_icon(
    provider_key: &str,
    size_px: f32,
    monochrome_color: Rgba,
    use_brand_accent: bool,
) -> AnyElement {
    let key = canonical_provider_icon_key(provider_key);
    let paint_color = if use_brand_accent {
        match key {
            "claude" => rgba(0xD977_57FF),
            "gemini" => rgba(0x8E75_B2FF),
            "deepseek" => rgba(0x4F7B_FFFF),
            "openrouter" => rgba(0x7C6F_F7FF),
            "mistral" => rgba(0xF08A_24FF),
            _ => monochrome_color,
        }
    } else {
        monochrome_color
    };

    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let s = (size_px / 24.0).max(0.25);
            let w = (1.65 * s).clamp(1.0, 2.2);
            let o = bounds.origin;
            match key {
                "codex" => {
                    // OpenAI / ChatGPT 6-petal hexagonal blossom mark
                    let cx = 12.0_f32;
                    let cy = 12.0_f32;
                    for i in 0..6 {
                        let a0 = (i as f32) * std::f32::consts::FRAC_PI_3 - std::f32::consts::FRAC_PI_2;
                        let a1 = a0 + std::f32::consts::FRAC_PI_3;
                        let a_mid = a0 + std::f32::consts::FRAC_PI_6;
                        let p0 = (cx + 3.4 * a0.cos(), cy + 3.4 * a0.sin());
                        let p1 = (cx + 8.6 * (a0 + 0.18).cos(), cy + 8.6 * (a0 + 0.18).sin());
                        let p2 = (cx + 9.2 * a_mid.cos(), cy + 9.2 * a_mid.sin());
                        let p3 = (cx + 7.6 * a1.cos(), cy + 7.6 * a1.sin());
                        let p4 = (cx + 3.4 * a1.cos(), cy + 3.4 * a1.sin());
                        stroke_poly(window, o, s, w * 0.92, &[p0, p1, p2, p3, p4], paint_color);
                    }
                }
                "claude" => {
                    // Claude 8-ray starburst mark
                    let cx = 12.0_f32;
                    let cy = 12.0_f32;
                    for i in 0..8 {
                        let angle = (i as f32) * std::f32::consts::FRAC_PI_4;
                        let r_outer = if i % 2 == 0 { 9.2 } else { 7.8 };
                        let x1 = cx + 2.2 * angle.cos();
                        let y1 = cy + 2.2 * angle.sin();
                        let x2 = cx + r_outer * angle.cos();
                        let y2 = cy + r_outer * angle.sin();
                        stroke_poly(window, o, s, w * 1.1, &[(x1, y1), (x2, y2)], paint_color);
                    }
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(12.0, 9.6), (14.4, 12.0), (12.0, 14.4), (9.6, 12.0)],
                        paint_color,
                    );
                }
                "opencode" => {
                    // OpenCode minimal window / nested square icon
                    stroke_poly(
                        window,
                        o,
                        s,
                        w * 1.15,
                        &[
                            (5.0, 4.0),
                            (19.0, 4.0),
                            (19.0, 20.0),
                            (5.0, 20.0),
                            (5.0, 4.0),
                        ],
                        paint_color,
                    );
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(8.5, 9.0), (15.5, 9.0), (15.5, 16.5), (8.5, 16.5)],
                        paint_color,
                    );
                }
                "grok" => {
                    // Grok orbital ring with diagonal slash
                    let mut ring = Vec::with_capacity(13);
                    for i in 0..=12 {
                        let a = (i as f32) * std::f32::consts::TAU / 12.0;
                        ring.push((12.0 + 7.2 * a.cos(), 12.0 + 7.2 * a.sin()));
                    }
                    stroke_poly(window, o, s, w, &ring, paint_color);
                    stroke_poly(window, o, s, w * 1.15, &[(6.0, 18.5), (18.5, 5.5)], paint_color);
                }
                "gemini" => {
                    // Gemini 4-point sparkle diamond star
                    fill_poly(
                        window,
                        o,
                        s,
                        &[
                            (12.0, 3.0),
                            (14.3, 9.7),
                            (21.0, 12.0),
                            (14.3, 14.3),
                            (12.0, 21.0),
                            (9.7, 14.3),
                            (3.0, 12.0),
                            (9.7, 9.7),
                        ],
                        paint_color,
                    );
                }
                "cursor" => {
                    // Cursor isometric 3D cube
                    stroke_poly(
                        window,
                        o,
                        s,
                        w,
                        &[
                            (12.0, 3.5),
                            (19.5, 7.8),
                            (19.5, 16.2),
                            (12.0, 20.5),
                            (4.5, 16.2),
                            (4.5, 7.8),
                            (12.0, 3.5),
                        ],
                        paint_color,
                    );
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(12.0, 12.0), (19.5, 7.8), (19.5, 16.2), (12.0, 20.5)],
                        paint_color,
                    );
                    stroke_poly(window, o, s, w, &[(4.5, 7.8), (12.0, 12.0), (12.0, 20.5)], paint_color);
                }
                "cline" => {
                    // Cline rounded bot head with antenna & eyes
                    stroke_poly(
                        window,
                        o,
                        s,
                        w,
                        &[
                            (6.5, 8.0),
                            (17.5, 8.0),
                            (19.5, 10.0),
                            (19.5, 17.0),
                            (17.5, 19.0),
                            (6.5, 19.0),
                            (4.5, 17.0),
                            (4.5, 10.0),
                            (6.5, 8.0),
                        ],
                        paint_color,
                    );
                    stroke_poly(window, o, s, w, &[(12.0, 4.5), (12.0, 8.0)], paint_color);
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(8.3, 12.0), (10.5, 12.0), (10.5, 15.0), (8.3, 15.0)],
                        paint_color,
                    );
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(13.5, 12.0), (15.7, 12.0), (15.7, 15.0), (13.5, 15.0)],
                        paint_color,
                    );
                }
                "pi" => {
                    // Pi terminal prompt `>_`
                    stroke_poly(window, o, s, w * 1.15, &[(5.5, 7.5), (11.0, 12.0), (5.5, 16.5)], paint_color);
                    stroke_poly(window, o, s, w * 1.15, &[(12.5, 16.5), (19.0, 16.5)], paint_color);
                }
                "deepseek" => {
                    // DeepSeek geometric wave/whale emblem
                    stroke_poly(
                        window,
                        o,
                        s,
                        w * 1.1,
                        &[
                            (4.0, 14.5),
                            (7.5, 8.5),
                            (13.5, 7.5),
                            (19.5, 11.0),
                            (16.5, 16.5),
                            (9.0, 17.5),
                            (4.0, 14.5),
                        ],
                        paint_color,
                    );
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(13.2, 10.4), (15.2, 10.4), (15.2, 12.4), (13.2, 12.4)],
                        paint_color,
                    );
                }
                "kimi" => {
                    // Kimi crisp K mark
                    stroke_poly(window, o, s, w * 1.2, &[(7.0, 5.0), (7.0, 19.0)], paint_color);
                    stroke_poly(window, o, s, w * 1.2, &[(17.0, 5.5), (8.0, 12.0), (17.5, 18.5)], paint_color);
                }
                "copilot" => {
                    // GitHub Copilot visor mark
                    stroke_poly(
                        window,
                        o,
                        s,
                        w,
                        &[
                            (5.0, 10.5),
                            (9.0, 8.0),
                            (15.0, 8.0),
                            (19.0, 10.5),
                            (19.0, 16.5),
                            (15.0, 18.0),
                            (9.0, 18.0),
                            (5.0, 16.5),
                            (5.0, 10.5),
                        ],
                        paint_color,
                    );
                    stroke_poly(window, o, s, w, &[(9.5, 12.0), (9.5, 14.5)], paint_color);
                    stroke_poly(window, o, s, w, &[(14.5, 12.0), (14.5, 14.5)], paint_color);
                }
                "openrouter" => {
                    // Branching router paths
                    stroke_poly(window, o, s, w * 1.1, &[(4.5, 12.0), (11.0, 12.0), (18.5, 6.5)], paint_color);
                    stroke_poly(window, o, s, w * 1.1, &[(11.0, 12.0), (18.5, 17.5)], paint_color);
                    stroke_poly(window, o, s, w, &[(15.5, 6.0), (19.0, 6.5), (18.0, 10.0)], paint_color);
                    stroke_poly(window, o, s, w, &[(15.5, 18.0), (19.0, 17.5), (18.0, 14.0)], paint_color);
                }
                "mistral" => {
                    // Mistral stacked pixel M bars
                    fill_poly(window, o, s, &[(4.5, 6.0), (8.0, 6.0), (8.0, 18.0), (4.5, 18.0)], paint_color);
                    fill_poly(window, o, s, &[(16.0, 6.0), (19.5, 6.0), (19.5, 18.0), (16.0, 18.0)], paint_color);
                    fill_poly(window, o, s, &[(10.2, 10.0), (13.8, 10.0), (13.8, 15.0), (10.2, 15.0)], paint_color);
                }
                "ollama" => {
                    // Local server / Ollama box
                    stroke_poly(
                        window,
                        o,
                        s,
                        w,
                        &[
                            (5.0, 6.5),
                            (19.0, 6.5),
                            (19.0, 11.5),
                            (5.0, 11.5),
                            (5.0, 6.5),
                        ],
                        paint_color,
                    );
                    stroke_poly(
                        window,
                        o,
                        s,
                        w,
                        &[
                            (5.0, 13.5),
                            (19.0, 13.5),
                            (19.0, 18.5),
                            (5.0, 18.5),
                            (5.0, 13.5),
                        ],
                        paint_color,
                    );
                    fill_poly(window, o, s, &[(7.5, 8.3), (9.2, 8.3), (9.2, 9.8), (7.5, 9.8)], paint_color);
                    fill_poly(window, o, s, &[(7.5, 15.3), (9.2, 15.3), (9.2, 16.8), (7.5, 16.8)], paint_color);
                }
                _ => {
                    // Clean hexagonal spark node for other presets (Zhipu, Qwen, custom OpenAI, etc.)
                    stroke_poly(
                        window,
                        o,
                        s,
                        w,
                        &[
                            (12.0, 4.0),
                            (19.0, 8.0),
                            (19.0, 16.0),
                            (12.0, 20.0),
                            (5.0, 16.0),
                            (5.0, 8.0),
                            (12.0, 4.0),
                        ],
                        paint_color,
                    );
                    fill_poly(
                        window,
                        o,
                        s,
                        &[(12.0, 9.2), (14.8, 12.0), (12.0, 14.8), (9.2, 12.0)],
                        paint_color,
                    );
                }
            }
        },
    )
    .w(px(size_px))
    .h(px(size_px))
    .flex_shrink_0()
    .into_any_element()
}

/// One selectable model entry inside an OpenBot provider panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenBotPickerModelItem {
    pub id: String,
    pub label: String,
    pub is_default: bool,
    pub is_legacy: bool,
}

/// One provider tab in the left rail of OpenBot's `ProviderModelPicker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenBotPickerProvider {
    pub id: String,
    pub icon_key: String,
    pub display_name: String,
    pub subtitle: String,
    pub is_default_provider: bool,
    pub is_subscription: bool,
    pub models: Vec<OpenBotPickerModelItem>,
}

/// Interactive state for OpenBot's two-column `ProviderModelPicker` popover.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenBotModelPickerState {
    pub selected_provider_id: String,
    pub search_query: String,
    pub search_focused: bool,
    pub legacy_expanded: bool,
    pub effort_menu_open: bool,
    pub catalog_synced: bool,
    pub providers: Vec<OpenBotPickerProvider>,
}

/// Events emitted by `render_openbot_provider_model_picker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenBotModelPickerAction {
    SelectProvider(String),
    FocusSearch,
    ClearSearch,
    SelectModel {
        provider_id: String,
        model_id: String,
    },
    ToggleLegacy,
    ToggleEffortMenu,
    SelectEffort(String),
    OpenProvidersSettings,
    Dismiss,
}

fn is_obviously_legacy_model(model_id: &str, has_gpt6: bool, has_gemini3: bool) -> bool {
    let lower = model_id.trim().to_ascii_lowercase();
    if lower.starts_with("gpt-3")
        || lower.starts_with("gpt-4")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.contains("codex-mini")
        || lower.contains("codex-auto")
        || lower.contains("daybreak")
        || lower.starts_with("claude-3")
        || lower.starts_with("gemini-1.")
        || lower.starts_with("gemini-2.0")
    {
        return true;
    }
    if has_gpt6 && lower.starts_with("gpt-5") {
        return true;
    }
    if has_gemini3 && (lower.contains("-preview") || lower.contains("-lite") || lower.contains("nano-banana")) {
        return true;
    }
    false
}

/// Partitions a provider's model list into primary models (up to 5 flagship + default/active)
/// and collapsible `旧版模型` (legacy) models, mirroring OpenBot's `ProviderModelPicker`.
pub fn classify_provider_models(
    raw_models: &[String],
    default_model: Option<&str>,
    active_model: Option<&str>,
) -> Vec<OpenBotPickerModelItem> {
    let mut deduped: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for m in raw_models {
        let t = m.trim();
        if t.is_empty() {
            continue;
        }
        let key = t.to_ascii_lowercase();
        if seen.insert(key) {
            deduped.push(t.to_string());
        }
    }
    if let Some(dm) = default_model.map(str::trim).filter(|s| !s.is_empty()) {
        if seen.insert(dm.to_ascii_lowercase()) {
            deduped.insert(0, dm.to_string());
        }
    }
    if deduped.is_empty() {
        return Vec::new();
    }

    let has_gpt6 = deduped
        .iter()
        .any(|m| m.to_ascii_lowercase().starts_with("gpt-6"));
    let has_gemini3 = deduped
        .iter()
        .any(|m| m.to_ascii_lowercase().starts_with("gemini-3") || m.to_ascii_lowercase().starts_with("gemini-4"));

    let default_target = default_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            deduped
                .iter()
                .find(|m| !is_obviously_legacy_model(m, has_gpt6, has_gemini3))
                .or_else(|| deduped.first())
                .map(|m| m.to_ascii_lowercase())
        });
    let active_lower = active_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);

    let mut primary = Vec::new();
    let mut legacy = Vec::new();
    for m in deduped {
        let lower = m.to_ascii_lowercase();
        let is_default = default_target.as_deref() == Some(lower.as_str());
        let is_active = active_lower.as_deref() == Some(lower.as_str());
        let obvious_legacy = is_obviously_legacy_model(&m, has_gpt6, has_gemini3);
        let item = OpenBotPickerModelItem {
            label: format_model_display_label(&m),
            id: m,
            is_default,
            is_legacy: false,
        };
        if is_default || is_active || (!obvious_legacy && primary.len() < 5) {
            primary.push(item);
        } else {
            let mut leg = item;
            leg.is_legacy = true;
            legacy.push(leg);
        }
    }
    if primary.is_empty() && !legacy.is_empty() {
        for item in legacy.iter_mut().take(5) {
            item.is_legacy = false;
        }
        let mut split = legacy.split_off(legacy.len().min(5));
        primary.append(&mut legacy);
        legacy.append(&mut split);
    }
    if !primary.iter().any(|m| m.is_default) {
        if let Some(first) = primary.first_mut() {
            first.is_default = true;
        }
    }
    primary.extend(legacy);
    primary
}

fn detected_cli_version(
    detected: &[DetectedCodingProvider],
    kind: CodingProviderKind,
) -> Option<String> {
    detected
        .iter()
        .find(|d| d.kind == kind && d.installed)
        .and_then(|d| d.version.as_deref())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

impl OpenBotModelPickerState {
    /// Builds the live provider & model catalog from Settings' `MagpieStateDto`, discovered
    /// subscriptions, and local CLI detections.
    pub fn sync_from_magpie(
        &mut self,
        state: &MagpieStateDto,
        server_subs: &[SubscriptionAccountDto],
        local_subs: &[SubscriptionAccountDto],
        detected_coding: &[DetectedCodingProvider],
        current_selected_model: Option<&str>,
    ) {
        self.catalog_synced = true;
        let mut merged_subs: Vec<SubscriptionAccountDto> = if !server_subs.is_empty() {
            server_subs.to_vec()
        } else {
            state
                .subscriptions
                .iter()
                .map(sub_dto_to_account_dto)
                .collect()
        };
        for local in local_subs {
            if let Some(existing) = merged_subs.iter_mut().find(|s| s.id == local.id) {
                if !local.models.is_empty() {
                    let mut combined = local.models.clone();
                    for m in &existing.models {
                        if !combined.iter().any(|c| c.eq_ignore_ascii_case(m)) {
                            combined.push(m.clone());
                        }
                    }
                    existing.models = combined;
                }
            } else {
                merged_subs.push(local.clone());
            }
        }

        let active_pid = state.active_provider.trim();
        let active_model_opt = (!state.active_model.trim().is_empty())
            .then_some(state.active_model.trim());
        let mut providers: Vec<OpenBotPickerProvider> = Vec::new();
        let mut seen_ids = HashSet::new();

        for sub in &merged_subs {
            if !seen_ids.insert(sub.id.clone()) {
                continue;
            }
            let icon_key = canonical_provider_icon_key(&sub.kind).to_string();
            let display_name = friendly_provider_name(&sub.kind, &sub.label);
            let is_default_provider = !state.needs_provider_selection
                && (sub.active
                    || sub.id == active_pid
                    || (sub.kind == "google_vertex" && active_pid == "magpie_provider:google-vertex"));
            let cli_subtitle = match icon_key.as_str() {
                "codex" => detected_cli_version(detected_coding, CodingProviderKind::Codex)
                    .map(|v| format!("{v} (Codex CLI)"))
                    .or_else(|| sub.account_id.clone())
                    .unwrap_or_else(|| "Codex CLI".to_string()),
                "claude" => detected_cli_version(detected_coding, CodingProviderKind::ClaudeCode)
                    .map(|v| format!("{v} (Claude Code)"))
                    .or_else(|| sub.account_id.clone())
                    .unwrap_or_else(|| "Claude Code".to_string()),
                "opencode" => detected_cli_version(detected_coding, CodingProviderKind::OpenCode)
                    .map(|v| format!("{v} (OpenCode CLI)"))
                    .unwrap_or_else(|| "OpenCode CLI".to_string()),
                "gemini" if sub.kind.contains("vertex") => match (&sub.plan, &sub.account_id) {
                    (_, Some(acc)) if !acc.trim().is_empty() => format!("Vertex ADC · {acc}"),
                    _ => "Vertex ADC".to_string(),
                },
                _ => sub
                    .account_id
                    .clone()
                    .or_else(|| sub.plan.clone())
                    .unwrap_or_else(|| sub.kind.clone()),
            };
            let default_m = if is_default_provider {
                active_model_opt.or_else(|| sub.models.first().map(String::as_str))
            } else {
                sub.models.first().map(String::as_str)
            };
            let models = classify_provider_models(&sub.models, default_m, current_selected_model);
            providers.push(OpenBotPickerProvider {
                id: sub.id.clone(),
                icon_key,
                display_name,
                subtitle: cli_subtitle,
                is_default_provider,
                is_subscription: true,
                models,
            });
        }

        for prov in &state.providers {
            if prov.provider_id == "local_hermes"
                && (!state.allow_local_fallback || !providers.is_empty())
            {
                continue;
            }
            if !seen_ids.insert(prov.provider_id.clone()) {
                continue;
            }
            let mut raw_models = prov.models.clone();
            if !prov.default_model.trim().is_empty() {
                raw_models.insert(0, prov.default_model.trim().to_string());
            }
            if let Some(preset_id) = prov.preset_id.as_deref() {
                if let Some(preset) = state.presets.iter().find(|p| p.id == preset_id) {
                    raw_models.extend(preset.suggested_models.iter().cloned());
                }
            }
            let icon_key = canonical_provider_icon_key(
                prov.preset_id.as_deref().unwrap_or(&prov.provider_id),
            )
            .to_string();
            let display_name = if prov.name.trim().is_empty() {
                friendly_provider_name(&prov.provider_id, &prov.provider_id)
            } else {
                prov.name.trim().to_string()
            };
            let subtitle = if prov.no_key_required {
                format!("{} · local", prov.api_mode)
            } else if let Some(ref mk) = prov.masked_key {
                format!("API Key · {mk}")
            } else {
                prov.api_mode.clone()
            };
            let is_default_provider = !state.needs_provider_selection
                && (prov.is_active || prov.provider_id == active_pid);
            let default_m = if is_default_provider {
                active_model_opt.or_else(|| {
                    (!prov.default_model.trim().is_empty()).then_some(prov.default_model.trim())
                })
            } else {
                (!prov.default_model.trim().is_empty()).then_some(prov.default_model.trim())
            };
            let models = classify_provider_models(&raw_models, default_m, current_selected_model);
            providers.push(OpenBotPickerProvider {
                id: prov.provider_id.clone(),
                icon_key,
                display_name,
                subtitle,
                is_default_provider,
                is_subscription: prov.is_subscription,
                models,
            });
        }

        // Put default/active provider first when ordering isn't already established, preserving stable order
        self.providers = providers;
        self.ensure_selected_provider(current_selected_model);
    }

    /// Returns the live synced providers list (never fabricates hardcoded placeholder models).
    pub fn effective_providers(&self) -> Vec<OpenBotPickerProvider> {
        self.providers.clone()
    }

    /// Ensures `selected_provider_id` points to a valid provider tab (preferring the provider
    /// that owns `current_model`, then the default provider, then the first provider).
    pub fn ensure_selected_provider(&mut self, current_model: Option<&str>) {
        let list = self.effective_providers();
        if list.is_empty() {
            self.selected_provider_id.clear();
            return;
        }
        if !self.selected_provider_id.is_empty()
            && list.iter().any(|p| p.id == self.selected_provider_id)
        {
            return;
        }
        if let Some(m) = current_model.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(found) = list
                .iter()
                .find(|p| p.models.iter().any(|item| item.id.eq_ignore_ascii_case(m)))
            {
                self.selected_provider_id = found.id.clone();
                return;
            }
        }
        if let Some(def) = list.iter().find(|p| p.is_default_provider) {
            self.selected_provider_id = def.id.clone();
        } else {
            self.selected_provider_id = list[0].id.clone();
        }
    }

    /// Resolves the provider icon key and clean display name for the trigger pill/row.
    pub fn resolve_trigger_display(
        &self,
        selected_model: &str,
        fallback_label: &str,
    ) -> (&'static str, String) {
        let trimmed = selected_model.trim();
        let list = self.effective_providers();
        if !trimmed.is_empty() {
            if let Some(prov) = list
                .iter()
                .find(|p| p.models.iter().any(|m| m.id.eq_ignore_ascii_case(trimmed)))
            {
                let lbl = prov
                    .models
                    .iter()
                    .find(|m| m.id.eq_ignore_ascii_case(trimmed))
                    .map(|m| m.label.clone())
                    .unwrap_or_else(|| format_model_display_label(trimmed));
                return (canonical_provider_icon_key(&prov.icon_key), lbl);
            }
            return (
                infer_provider_icon_key_from_model(trimmed),
                format_model_display_label(trimmed),
            );
        }
        if let Some(def_prov) = list
            .iter()
            .find(|p| p.is_default_provider)
            .or_else(|| list.first())
        {
            let lbl = def_prov
                .models
                .iter()
                .find(|m| m.is_default)
                .or_else(|| def_prov.models.first())
                .map(|m| m.label.clone())
                .unwrap_or_else(|| fallback_label.to_string());
            return (canonical_provider_icon_key(&def_prov.icon_key), lbl);
        }
        ("generic", fallback_label.to_string())
    }

    /// Returns the first model matching the current `search_query` in the active provider tab.
    pub fn first_filtered_model(&self) -> Option<(String, String)> {
        let list = self.effective_providers();
        let active = list
            .iter()
            .find(|p| p.id == self.selected_provider_id)
            .or_else(|| list.first())?;
        let q = self.search_query.trim().to_ascii_lowercase();
        active
            .models
            .iter()
            .find(|m| {
                q.is_empty()
                    || m.id.to_ascii_lowercase().contains(&q)
                    || m.label.to_ascii_lowercase().contains(&q)
            })
            .map(|m| (active.id.clone(), m.id.clone()))
    }
}

/// Renders the exact OpenBot two-column `ProviderModelPicker` popover card (without backdrop/positioning).
/// Callers place this card in a floating `deferred(...)` overlay above/below their trigger control.
pub fn render_openbot_provider_model_picker_card<V: 'static>(
    id_prefix: &'static str,
    state: &OpenBotModelPickerState,
    active_model: &str,
    active_effort: &str,
    palette: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, OpenBotModelPickerAction, &mut Context<V>) + Clone + 'static,
) -> Stateful<Div> {
    let ob = palette.openbot_palette();
    let bg_surface = ob.bg_surface();
    let bg_control = ob.bg_control();
    let bg_active = ob.bg_control_active();
    let hover_bg = ob.hover();
    let border_hairline = ob.border_hairline();
    let border_strong = ob.border_strong_hairline();
    let text_pri = ob.text_primary();
    let text_sec = ob.text_secondary();
    let text_muted = ob.text_muted();
    let radio_checked_bg = ob.surface_light();
    let radio_checked_fg = ob.text_on_light();
    let radio_border = ob.border_strong();

    let providers = state.effective_providers();
    let selected_provider = providers
        .iter()
        .find(|p| p.id == state.selected_provider_id)
        .or_else(|| providers.iter().find(|p| p.is_default_provider))
        .or_else(|| providers.first())
        .cloned();

    // Left vertical provider icon rail (44px wide) + bottom `+` button
    let mut left_rail = div()
        .id(SharedString::from(format!("{id_prefix}-provider-rail")))
        .w(px(44.0))
        .flex_shrink_0()
        .py(px(8.0))
        .px(px(6.0))
        .border_r_1()
        .border_color(border_hairline)
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0));

    for (idx, prov) in providers.iter().enumerate() {
        let is_selected = selected_provider
            .as_ref()
            .is_some_and(|sp| sp.id == prov.id);
        let pid = prov.id.clone();
        let on_sel = on_action.clone();
        left_rail = left_rail.child(
            div()
                .id(SharedString::from(format!("{id_prefix}-rail-prov-{idx}")))
                .cursor_pointer()
                .w(px(32.0))
                .h(px(32.0))
                .rounded(px(8.0))
                .when(is_selected, |el| {
                    el.bg(bg_active).border_1().border_color(border_strong)
                })
                .hover(move |s| if is_selected { s } else { s.bg(hover_bg) })
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_sel(this, OpenBotModelPickerAction::SelectProvider(pid.clone()), cx);
                    }),
                )
                .child(render_provider_icon(
                    &prov.icon_key,
                    17.0,
                    if is_selected { text_pri } else { text_muted },
                    true,
                )),
        );
    }

    let on_add_prov = on_action.clone();
    left_rail = left_rail.child(
        div()
            .id(SharedString::from(format!("{id_prefix}-rail-add-provider")))
            .cursor_pointer()
            .mt(px(2.0))
            .w(px(32.0))
            .h(px(32.0))
            .rounded(px(8.0))
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    on_add_prov(this, OpenBotModelPickerAction::OpenProvidersSettings, cx);
                }),
            )
            .child(openbot_stage_icon(OpenBotStageIcon::Plus, 15.0, text_muted)),
    );

    // Right panel: Header + Search + Model Radio List + Legacy Disclosure + Effort Footer
    let mut right_panel = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col();

    if let Some(prov) = selected_provider {
        let q = state.search_query.trim().to_ascii_lowercase();
        let active_trimmed = active_model.trim();
        let effective_active_id = if !active_trimmed.is_empty() {
            Some(active_trimmed.to_string())
        } else {
            prov.models
                .iter()
                .find(|m| m.is_default)
                .or_else(|| prov.models.first())
                .map(|m| m.id.clone())
        };

        let matching_primary: Vec<&OpenBotPickerModelItem> = prov
            .models
            .iter()
            .filter(|m| {
                !m.is_legacy
                    && (q.is_empty()
                        || m.id.to_ascii_lowercase().contains(&q)
                        || m.label.to_ascii_lowercase().contains(&q))
            })
            .collect();
        let matching_legacy: Vec<&OpenBotPickerModelItem> = prov
            .models
            .iter()
            .filter(|m| {
                m.is_legacy
                    && (q.is_empty()
                        || m.id.to_ascii_lowercase().contains(&q)
                        || m.label.to_ascii_lowercase().contains(&q))
            })
            .collect();
        let total_legacy_count = prov.models.iter().filter(|m| m.is_legacy).count();
        let show_legacy_items = state.legacy_expanded || !q.is_empty();

        // Header: Provider Display Name + CLI/Account Subtitle
        right_panel = right_panel.child(
            div()
                .px(px(12.0))
                .pt(px(10.0))
                .pb(px(6.0))
                .flex()
                .flex_col()
                .gap(px(1.5))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(prov.display_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(text_muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(prov.subtitle.clone()),
                ),
        );

        // Search input box (`搜索模型`)
        let on_focus_search = on_action.clone();
        let on_clear_search = on_action.clone();
        let search_has_text = !state.search_query.is_empty();
        right_panel = right_panel.child(
            div()
                .px(px(10.0))
                .pb(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("{id_prefix}-model-search-box")))
                        .cursor_text()
                        .h(px(30.0))
                        .px(px(10.0))
                        .rounded(px(7.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(if state.search_focused {
                            text_muted
                        } else {
                            border_hairline
                        })
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                on_focus_search(this, OpenBotModelPickerAction::FocusSearch, cx);
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.5))
                                .text_color(if search_has_text { text_pri } else { text_muted })
                                .child(if !search_has_text {
                                    if state.search_focused {
                                        "|".to_string()
                                    } else {
                                        tr_str(lang, "搜索模型", "Search models").to_string()
                                    }
                                } else if state.search_focused {
                                    format!("{}|", state.search_query)
                                } else {
                                    state.search_query.clone()
                                }),
                        )
                        .when(search_has_text, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from(format!("{id_prefix}-model-search-clear")))
                                    .cursor_pointer()
                                    .w(px(18.0))
                                    .h(px(18.0))
                                    .rounded_full()
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            on_clear_search(
                                                this,
                                                OpenBotModelPickerAction::ClearSearch,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(openbot_stage_icon(
                                        OpenBotStageIcon::Close,
                                        11.0,
                                        text_muted,
                                    )),
                            )
                        }),
                ),
        );

        // Scrollable radio model list
        let mut model_list = div()
            .id(SharedString::from(format!("{id_prefix}-model-list-scroll")))
            .max_h(px(212.0))
            .overflow_y_scroll()
            .px(px(6.0))
            .pb(px(6.0))
            .flex()
            .flex_col()
            .gap(px(1.5));

        let build_model_row =
            |row_id: String,
             item: &OpenBotPickerModelItem,
             prov_id: &str,
             cx: &mut Context<V>| {
                let is_sel = effective_active_id
                    .as_deref()
                    .is_some_and(|cur| cur.eq_ignore_ascii_case(&item.id));
                let pid_owned = prov_id.to_string();
                let mid_owned = item.id.clone();
                let on_pick = on_action.clone();
                div()
                    .id(SharedString::from(row_id))
                    .cursor_pointer()
                    .h(px(30.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            on_pick(
                                this,
                                OpenBotModelPickerAction::SelectModel {
                                    provider_id: pid_owned.clone(),
                                    model_id: mid_owned.clone(),
                                },
                                cx,
                            );
                        }),
                    )
                    .child(if is_sel {
                        div()
                            .w(px(15.0))
                            .h(px(15.0))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(radio_checked_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(openbot_stage_icon(
                                OpenBotStageIcon::Check,
                                10.0,
                                radio_checked_fg,
                            ))
                    } else {
                        div()
                            .w(px(15.0))
                            .h(px(15.0))
                            .rounded_full()
                            .flex_shrink_0()
                            .border_1()
                            .border_color(radio_border)
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(13.0))
                                    .font_weight(if is_sel {
                                        FontWeight::MEDIUM
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if is_sel { text_pri } else { text_sec })
                                    .child(item.label.clone()),
                            )
                            .when(item.is_default, |row| {
                                row.child(
                                    div()
                                        .flex_shrink_0()
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(bg_active)
                                        .text_size(px(10.5))
                                        .text_color(text_muted)
                                        .child(tr_str(lang, "默认", "Default")),
                                )
                            }),
                    )
            };

        for (idx, item) in matching_primary.iter().enumerate() {
            model_list = model_list.child(build_model_row(
                format!("{id_prefix}-m-pri-{idx}"),
                item,
                &prov.id,
                cx,
            ));
        }

        if total_legacy_count > 0 && (!matching_legacy.is_empty() || q.is_empty()) {
            let legacy_badge_count = if q.is_empty() {
                total_legacy_count
            } else {
                matching_legacy.len()
            };
            let on_toggle_legacy = on_action.clone();
            model_list = model_list.child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-legacy-toggle")))
                    .cursor_pointer()
                    .mt(px(2.0))
                    .h(px(28.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            on_toggle_legacy(this, OpenBotModelPickerAction::ToggleLegacy, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .pl(px(22.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(text_muted)
                                    .child(tr_str(lang, "旧版模型", "Legacy models")),
                            )
                            .child(
                                div()
                                    .min_w(px(18.0))
                                    .h(px(16.0))
                                    .px(px(5.0))
                                    .rounded_full()
                                    .bg(bg_active)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(10.5))
                                    .text_color(text_muted)
                                    .child(legacy_badge_count.to_string()),
                            ),
                    )
                    .child(openbot_stage_icon(
                        if show_legacy_items {
                            OpenBotStageIcon::ChevronUp
                        } else {
                            OpenBotStageIcon::ChevronDown
                        },
                        12.0,
                        text_muted,
                    )),
            );
            if show_legacy_items {
                for (idx, item) in matching_legacy.iter().enumerate() {
                    model_list = model_list.child(build_model_row(
                        format!("{id_prefix}-m-leg-{idx}"),
                        item,
                        &prov.id,
                        cx,
                    ));
                }
            }
        }

        if matching_primary.is_empty() && matching_legacy.is_empty() {
            model_list = model_list.child(
                div()
                    .py(px(18.0))
                    .px(px(12.0))
                    .text_center()
                    .text_size(px(12.0))
                    .text_color(text_muted)
                    .child(tr_str(lang, "未找到匹配的模型", "No matching models")),
            );
        }

        right_panel = right_panel.child(model_list);
    } else {
        // Connected providers empty state (after live sync confirms 0 connected providers)
        let on_open_settings = on_action.clone();
        right_panel = right_panel.child(
            div()
                .flex_1()
                .p(px(16.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_pri)
                        .child(tr_str(lang, "尚未连接服务商", "No connected providers")),
                )
                .child(
                    div()
                        .text_center()
                        .text_size(px(11.5))
                        .text_color(text_muted)
                        .child(tr_str(
                            lang,
                            "前往「服务商」连接订阅或配置 API Key。",
                            "Open Providers settings to connect a subscription or API key.",
                        )),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id_prefix}-empty-open-providers")))
                        .cursor_pointer()
                        .h(px(30.0))
                        .px(px(12.0))
                        .rounded(px(7.0))
                        .bg(radio_checked_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(radio_checked_fg)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                on_open_settings(
                                    this,
                                    OpenBotModelPickerAction::OpenProvidersSettings,
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, "前往服务商设置", "Open Providers")),
                ),
        );
    }

    // Footer: `强度` + `[中 ↕]` dropdown selector (exact OpenBot footer!)
    let current_effort_lbl = openbot_effort_label(active_effort, lang);
    let on_toggle_eff = on_action.clone();
    let effort_menu = state.effort_menu_open.then(|| {
        div()
            .id(SharedString::from(format!("{id_prefix}-effort-menu")))
            .absolute()
            .bottom(px(34.0))
            .right(px(0.0))
            .w(px(116.0))
            .p(px(4.0))
            .rounded(px(9.0))
            .bg(bg_surface)
            .border_1()
            .border_color(border_strong)
            .shadow(palette.popover_shadows())
            .flex()
            .flex_col()
            .gap(px(1.5))
            .children(OPENBOT_EFFORT_CHOICES.into_iter().enumerate().map(
                |(idx, (val, zh, en))| {
                    let is_sel = if active_effort.trim().is_empty() {
                        val == "medium"
                    } else {
                        active_effort.eq_ignore_ascii_case(val)
                    };
                    let val_owned = val.to_string();
                    let on_pick_eff = on_action.clone();
                    div()
                        .id(SharedString::from(format!("{id_prefix}-eff-opt-{idx}")))
                        .cursor_pointer()
                        .h(px(26.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .when(is_sel, |el| el.bg(bg_active))
                        .hover(move |s| if is_sel { s } else { s.bg(hover_bg) })
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(12.0))
                        .text_color(if is_sel { text_pri } else { text_sec })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                on_pick_eff(
                                    this,
                                    OpenBotModelPickerAction::SelectEffort(val_owned.clone()),
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, zh, en))
                        .when(is_sel, |el| {
                            el.child(openbot_stage_icon(OpenBotStageIcon::Check, 11.0, text_pri))
                        })
                },
            ))
    });

    right_panel = right_panel.child(
        div()
            .px(px(12.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(border_hairline)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(text_muted)
                    .child(tr_str(lang, "强度", "Effort")),
            )
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id(SharedString::from(format!("{id_prefix}-effort-trigger")))
                            .cursor_pointer()
                            .min_w(px(108.0))
                            .h(px(28.0))
                            .px(px(10.0))
                            .rounded(px(7.0))
                            .bg(bg_control)
                            .border_1()
                            .border_color(border_strong)
                            .hover(move |s| s.bg(bg_active))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    on_toggle_eff(
                                        this,
                                        OpenBotModelPickerAction::ToggleEffortMenu,
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(text_pri)
                                    .child(current_effort_lbl),
                            )
                            .child(render_chevrons_up_down(12.0, text_muted)),
                    )
                    .when_some(effort_menu, |el, menu| el.child(menu)),
            ),
    );

    div()
        .id(SharedString::from(format!("{id_prefix}-picker-card")))
        .w(px(348.0))
        .rounded(px(12.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .shadow(palette.popover_shadows())
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .flex()
        .flex_row()
        .child(left_rail)
        .child(right_panel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn model_classification_splits_primary_and_legacy_with_default_badge() {
        let models = vec![
            "gpt-6.2-sol".to_string(),
            "gpt-6.2-astra".to_string(),
            "gpt-6.1-terra".to_string(),
            "gpt-6.1-sol".to_string(),
            "gpt-6-luna".to_string(),
            "gpt-5.4".to_string(),
            "gpt-5".to_string(),
            "gpt-4o".to_string(),
            "o3".to_string(),
        ];
        let classified = classify_provider_models(&models, Some("gpt-6-luna"), None);
        let primary: Vec<_> = classified.iter().filter(|m| !m.is_legacy).collect();
        let legacy: Vec<_> = classified.iter().filter(|m| m.is_legacy).collect();
        assert_eq!(primary.len(), 5);
        assert_eq!(legacy.len(), 4);
        assert!(primary.iter().any(|m| m.id == "gpt-6-luna" && m.is_default));
        assert_eq!(format_model_display_label("gpt-6-luna"), "GPT-6-Luna");
        assert_eq!(format_model_display_label("gpt-6.1-terra"), "GPT-6.1 Terra");
    }

    #[std::prelude::v1::test]
    fn provider_icon_and_friendly_name_mapping_covers_openbot_providers() {
        assert_eq!(canonical_provider_icon_key("codex_chatgpt"), "codex");
        assert_eq!(canonical_provider_icon_key("claude_code"), "claude");
        assert_eq!(canonical_provider_icon_key("google_vertex"), "gemini");
        assert_eq!(canonical_provider_icon_key("opencode"), "opencode");
        assert_eq!(canonical_provider_icon_key("grok"), "grok");
        assert_eq!(canonical_provider_icon_key("cursor"), "cursor");
        assert_eq!(canonical_provider_icon_key("cline"), "cline");
        assert_eq!(canonical_provider_icon_key("pi"), "pi");
        assert_eq!(friendly_provider_name("codex_chatgpt", "Codex (user@test.com)"), "ChatGPT");
        assert_eq!(friendly_provider_name("claude_code", "Claude Code"), "Claude");
        assert_eq!(openbot_effort_label("medium", "zh-CN"), "中");
        assert_eq!(openbot_effort_label("high", "en"), "High");
    }

    #[std::prelude::v1::test]
    fn sync_from_magpie_builds_live_providers_and_resolves_trigger_and_search() {
        let mut state = OpenBotModelPickerState::default();
        let magpie = MagpieStateDto {
            active_provider: "codex:u1".to_string(),
            active_model: "gpt-6-luna".to_string(),
            needs_provider_selection: false,
            providers: vec![gpui_telegram_client::net::hermes_client::MagpieProviderDto {
                provider_id: "deepseek".to_string(),
                preset_id: Some("deepseek".to_string()),
                name: "DeepSeek".to_string(),
                default_model: "deepseek-chat".to_string(),
                models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
                is_active: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let local_subs = vec![
            SubscriptionAccountDto {
                id: "codex:u1".to_string(),
                account_id: Some("alice@example.com".to_string()),
                kind: "codex_chatgpt".to_string(),
                label: "Codex / ChatGPT".to_string(),
                plan: Some("plus".to_string()),
                models: vec![
                    "gpt-6.2-sol".to_string(),
                    "gpt-6-luna".to_string(),
                    "gpt-4o".to_string(),
                ],
                active: true,
            },
            SubscriptionAccountDto {
                id: "claude:u1".to_string(),
                account_id: None,
                kind: "claude_code".to_string(),
                label: "Claude Code".to_string(),
                plan: Some("max".to_string()),
                models: vec!["claude-sonnet-4-6".to_string(), "claude-3-5-sonnet".to_string()],
                active: false,
            },
        ];

        state.sync_from_magpie(&magpie, &[], &local_subs, &[], Some("gpt-6-luna"));
        assert!(state.catalog_synced);
        assert_eq!(state.providers.len(), 3);
        assert_eq!(state.selected_provider_id, "codex:u1");
        assert_eq!(
            state.resolve_trigger_display("gpt-6-luna", "GPT-6-Luna"),
            ("codex", "GPT-6-Luna".to_string())
        );

        state.search_query = "4o".to_string();
        assert_eq!(
            state.first_filtered_model(),
            Some(("codex:u1".to_string(), "gpt-4o".to_string()))
        );

        state.selected_provider_id = "deepseek".to_string();
        state.search_query = "reasoner".to_string();
        assert_eq!(
            state.first_filtered_model(),
            Some(("deepseek".to_string(), "deepseek-reasoner".to_string()))
        );
    }
}
