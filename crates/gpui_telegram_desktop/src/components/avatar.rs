use crate::theme::TdesktopThemePalette;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub use crate::components::bloub_avatar::{
    ChannelAgentBrief, ChannelAvatarLayout, FeltStatusBadgeState, SidebarFeltBadge,
    render_channel_avatar, render_felt_status_badge,
};
pub use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, render_grok_persona_avatar, trigger_grok_avatar_bounce,
    trigger_grok_avatar_spin, GrokBotAvatarProps, GrokPersonaState, GROK_COLOR_PALETTE,
};

pub fn parse_fabushi_hex_color(hex_or_name: &str) -> Option<u32> {
    let trimmed = hex_or_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(spec) = GROK_COLOR_PALETTE.iter().find(|spec| spec.id == lower) {
        return Some(spec.swatch_rgb);
    }
    let clean = trimmed.trim_start_matches('#');
    if clean.len() == 6 && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(clean, 16).ok()
    } else if clean.len() == 3 && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        let v = u32::from_str_radix(clean, 16).ok()?;
        let r = (v >> 8) & 0xF;
        let g = (v >> 4) & 0xF;
        let b = v & 0xF;
        Some(((r * 0x11) << 16) | ((g * 0x11) << 8) | (b * 0x11))
    } else {
        None
    }
}

fn avatar_initials(label: &str) -> String {
    let mut out = String::new();
    for part in label.split_whitespace().take(2) {
        if let Some(c) = part.chars().next() {
            out.push(c);
        }
    }
    if out.is_empty() {
        out.push('?');
    }
    out.to_uppercase()
}

/// Renders a bot avatar — defaulting all Fabushi/OpenBot bot silhouettes (`plush:*`, `blob`,
/// `pebble`, `squircle`, `tablet`, `wedge`, `hex`, `cloud`, `teardrop`, `""`, etc.) to 3D tactile
/// felt [`crate::components::bloub_avatar::PlushFeltRenderer`], flat `bloub:*` to `BloubBotEngine`,
/// `grok2d:*` to legacy 2D vector, and `"classic"` to circular initials.
pub fn render_fabushi_shape_avatar(
    shape: &str,
    bg_hex: u32,
    label: &str,
    size_px: f32,
) -> Div {
    let norm = shape.trim();
    if norm.eq_ignore_ascii_case("classic") {
        let initials = avatar_initials(label);
        return div()
            .relative()
            .w(px(size_px))
            .h(px(size_px))
            .flex_shrink_0()
            .rounded_full()
            .bg(rgb(bg_hex))
            .border_1()
            .border_color(rgba(0xFFFFFF38))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0xFFFFFF))
            .text_size(px((size_px * 0.36).clamp(9.0, 24.0)))
            .font_weight(FontWeight::BOLD)
            .child(initials);
    }

    if norm.to_ascii_lowercase().starts_with("grok2d:") {
        return render_grok_bot_avatar(GrokBotAvatarProps::new(
            label,
            norm,
            format!("#{:06X}", bg_hex & 0xFF_FFFF),
            size_px,
        ));
    }

    render_openbot_bot_plush_or_flat(
        norm,
        Some(format!("#{:06x}", bg_hex & 0xFF_FFFF)),
        label,
        GrokPersonaState::Idle,
        size_px,
        pal_paper_default(),
    )
}

const fn pal_paper_default() -> &'static str {
    crate::components::bloub_avatar::DEFAULT_PAPER
}

fn grok_persona_to_openbot_mood_and_activity(
    state: GrokPersonaState,
) -> (
    openbot_contracts::avatar::AvatarMood,
    Option<openbot_contracts::avatar::AgentStepActivity>,
) {
    use openbot_contracts::avatar::{AgentStepActivity, AvatarMood};
    let mood = match state {
        GrokPersonaState::Sleeping | GrokPersonaState::Drowsy | GrokPersonaState::PoweringDown => {
            AvatarMood::Asleep
        }
        GrokPersonaState::Thinking
        | GrokPersonaState::Searching
        | GrokPersonaState::Working
        | GrokPersonaState::Writing
        | GrokPersonaState::Sending
        | GrokPersonaState::Uploading => AvatarMood::Working,
        GrokPersonaState::Loading | GrokPersonaState::Spawning | GrokPersonaState::Receiving => {
            AvatarMood::Connecting
        }
        GrokPersonaState::Notifying
        | GrokPersonaState::Alerting
        | GrokPersonaState::Listening
        | GrokPersonaState::Dictating => AvatarMood::Waiting,
        GrokPersonaState::Sad | GrokPersonaState::Scared | GrokPersonaState::Angry => {
            AvatarMood::Failed
        }
        GrokPersonaState::Happy
        | GrokPersonaState::Laughing
        | GrokPersonaState::Celebrate
        | GrokPersonaState::Proud
        | GrokPersonaState::Excited => AvatarMood::Responded,
        _ => AvatarMood::Idle,
    };
    let activity = match state {
        GrokPersonaState::Searching => Some(AgentStepActivity::Searching),
        GrokPersonaState::Writing => Some(AgentStepActivity::Editing),
        GrokPersonaState::Working | GrokPersonaState::Sending | GrokPersonaState::Uploading => {
            Some(AgentStepActivity::Working)
        }
        _ => None,
    };
    (mood, activity)
}

/// Maps explicit legacy Fabushi/Grok shape identifiers onto their OpenBot [`BloubShape`] equivalent.
/// Generic `"blob"`, `"teardrop"`, `""`, and `"plush"` return `None` so deterministic FNV-1a seeding applies.
pub fn legacy_shape_to_bloub_shape(shape: &str) -> Option<openbot_contracts::avatar::BloubShape> {
    use openbot_contracts::avatar::BloubShape;
    match shape.trim().to_ascii_lowercase().as_str() {
        "pebble" | "galet" => Some(BloubShape::Galet),
        "squircle" => Some(BloubShape::Squircle),
        "tablet" | "capsule" => Some(BloubShape::Capsule),
        "hex" | "hexagon" | "hexagone" => Some(BloubShape::Hexagone),
        "cloud" | "nuage" => Some(BloubShape::Nuage),
        "wedge" | "triangle" => Some(BloubShape::Triangle),
        "diamond" | "losange" => Some(BloubShape::Losange),
        "circle" | "cercle" => Some(BloubShape::Cercle),
        "shield" | "pentagone" => Some(BloubShape::Pentagone),
        "carre" => Some(BloubShape::Carre),
        "fantome" => Some(BloubShape::Fantome),
        "meduse" => Some(BloubShape::Meduse),
        "poulpe" => Some(BloubShape::Poulpe),
        _ => None,
    }
}

/// Renders a 3D tactile felt OpenBot `PlushBot` avatar across all 10+3 silhouettes, 18 colors,
/// 7 moods, and 8 tool activities (`PROJECT.md` M1 contract).
pub fn render_plush_bot_avatar(
    seed: &str,
    shape: Option<openbot_contracts::avatar::BloubShape>,
    hue: Option<openbot_contracts::avatar::AvatarHue>,
    color: Option<&str>,
    size_px: f32,
    mood: openbot_contracts::avatar::AvatarMood,
    activity: Option<openbot_contracts::avatar::AgentStepActivity>,
) -> Div {
    use crate::components::bloub_avatar::{
        PlushFeltRenderer, render_plush_avatar_element, small_size_eye_scale,
    };
    let safe_size = if size_px.is_finite() && size_px > 0.0 {
        size_px
    } else {
        32.0
    };
    let clean_seed = if seed.trim().is_empty() {
        "agent"
    } else {
        seed.trim()
    };
    let normalized_color = color
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| openbot_contracts::avatar::is_avatar_color(c));
    let mut renderer = PlushFeltRenderer::for_seed_with_activity(
        clean_seed,
        shape,
        hue,
        normalized_color.as_deref(),
        mood,
        activity,
    );
    renderer.set_eye_scale(small_size_eye_scale(safe_size));
    let sample = renderer.sample_plush_frame(0.0, true);
    render_plush_avatar_element(&sample, safe_size)
}

/// Renders an OpenBot 3D `PlushBot` (or flat `:flat:\d+` / custom photo) avatar through a stateful
/// [`crate::components::bloub_avatar::AgentAvatarController`], enforcing `ACTIVITY_HOLD_MS = 1400ms`.
pub fn render_plush_bot_avatar_with_controller(
    props: &crate::components::bloub_avatar::AgentAvatarProps,
    controller: &mut crate::components::bloub_avatar::AgentAvatarController,
    now_ms: u64,
) -> Div {
    controller.sync_url(props.avatar_url.as_deref());
    controller.sync_activity(props.activity, now_ms);
    crate::components::bloub_avatar::render_agent_avatar(
        props,
        Some(controller),
        (now_ms as f64) / 1000.0,
    )
}

fn is_usable_custom_avatar_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix("data:") {
        let Some((_, b64)) = rest.split_once(";base64,") else {
            return false;
        };
        use base64::Engine as _;
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return false;
        };
        return image::guess_format(&raw).is_ok();
    }
    if trimmed.starts_with('/') || trimmed.starts_with("file://") {
        let fs_path = trimmed.strip_prefix("file://").unwrap_or(trimmed);
        return std::path::Path::new(fs_path).is_file();
    }
    trimmed.starts_with("http://") || trimmed.starts_with("https://")
}

/// Curated OpenBot Plush 3D character & felt-shape avatar specs (`plush:*`) shared by
/// the Create Bot dialog and Bot Inspector avatar editor.
pub const OPENBOT_PLUSH_AVATAR_PRESETS: [(&str, &str); 10] = [
    ("plush:nexo", "Nexo"),
    ("plush:vita", "Vita"),
    ("plush:cloe", "Cloe"),
    ("plush:malu", "Malu"),
    ("plush:yoshi", "Yoshi"),
    ("plush:fred", "Fred"),
    ("plush:squircle", "Squircle"),
    ("plush:nuage", "Nuage"),
    ("plush:fantome", "Fantome"),
    ("plush:poulpe", "Poulpe"),
];

/// Curated OpenBot flat parametric Bloub avatar specs (`bloub:*`) shared by the Create Bot
/// dialog and Bot Inspector avatar editor.
pub const OPENBOT_BLOUB_AVATAR_PRESETS: [(&str, &str); 8] = [
    ("bloub:squircle", "Squircle"),
    ("bloub:cercle", "Cercle"),
    ("bloub:galet", "Galet"),
    ("bloub:nuage", "Nuage"),
    ("bloub:hexagone", "Hex"),
    ("bloub:fantome", "Fantome"),
    ("bloub:meduse", "Meduse"),
    ("bloub:poulpe", "Poulpe"),
];

pub(crate) fn render_openbot_bot_plush_or_flat(
    raw_shape: &str,
    color_hex: Option<String>,
    label: &str,
    persona_state: GrokPersonaState,
    size_px: f32,
    paper_hex: &str,
) -> Div {
    use crate::components::bloub_avatar::{
        BloubBotEngine, PlushCharacterId, PlushFeltRenderer, bloub_shape_to_engine,
        bloub_to_engine_shape, render_flat_bloub_element, render_plush_avatar_element,
        small_size_eye_scale,
    };
    use openbot_contracts::avatar::{
        AvatarStyle, BloubShape, activity_mood_presentation, avatar_mood_presentation,
        avatar_style_for_seed, bloub_avatar_profile, plush_mood_presentation, seeded_expression,
    };

    let safe_size = if size_px.is_finite() && size_px > 0.0 {
        size_px
    } else {
        32.0
    };
    let trimmed = raw_shape.trim();
    let lower = trimmed.to_ascii_lowercase();
    let (mood, activity) = grok_persona_to_openbot_mood_and_activity(persona_state);
    let normalized_hex = color_hex
        .as_deref()
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| openbot_contracts::avatar::is_avatar_color(c));
    let explicit_hex = normalized_hex.as_deref();
    let base_seed = if label.trim().is_empty() {
        "agent"
    } else {
        label.trim()
    };

    if lower == "bloub"
        || lower.starts_with("bloub:")
        || avatar_style_for_seed(base_seed) == AvatarStyle::Flat
    {
        let suffix = trimmed
            .split_once(':')
            .map(|(_, rest)| rest.trim())
            .unwrap_or("");
        let suffix_lower = suffix.to_ascii_lowercase();
        let mood_pres = avatar_mood_presentation(mood);
        if (lower == "bloub" || lower.starts_with("bloub:"))
            && let Some(bloub_shape) = BloubShape::parse(&suffix_lower)
        {
            let seed = if label.trim().is_empty() {
                &suffix_lower
            } else {
                label.trim()
            };
            let expr = mood_pres.expression.unwrap_or_else(|| seeded_expression(seed));
            let engine = BloubBotEngine::new(
                bloub_to_engine_shape(bloub_shape),
                expr,
                mood_pres.state.to_parametric(),
            );
            let sample = engine.sample(0.0, true);
            let ink = explicit_hex.unwrap_or("#e8483f");
            return render_flat_bloub_element(&sample, ink, paper_hex, safe_size);
        }
        let seed = if (lower == "bloub" || lower.starts_with("bloub:")) && !suffix.is_empty() {
            suffix
        } else {
            base_seed
        };
        let profile = bloub_avatar_profile(seed, None, explicit_hex);
        let expr = mood_pres.expression.unwrap_or(profile.expression);
        let engine = BloubBotEngine::new(
            bloub_to_engine_shape(profile.shape),
            expr,
            mood_pres.state.to_parametric(),
        );
        let sample = engine.sample(0.0, true);
        return render_flat_bloub_element(&sample, &profile.color, paper_hex, safe_size);
    }

    if lower == "plush" || lower.starts_with("plush:") {
        let suffix = trimmed
            .split_once(':')
            .map(|(_, rest)| rest.trim())
            .unwrap_or("");
        let suffix_lower = suffix.to_ascii_lowercase();
        let mood_pres = match activity {
            Some(act) => activity_mood_presentation(act),
            None => plush_mood_presentation(mood),
        };
        if let Some(ch) = PlushCharacterId::parse(&suffix_lower) {
            let mut renderer = PlushFeltRenderer::for_character(ch, mood_pres.state);
            if let Some(expr) = mood_pres.expression {
                renderer.set_expression(expr);
            }
            renderer.set_eye_scale(small_size_eye_scale(safe_size));
            let sample = renderer.sample_plush_frame(0.0, true);
            return render_plush_avatar_element(&sample, safe_size);
        }
        if let Some(bloub_shape) = BloubShape::parse(&suffix_lower) {
            let seed = if label.trim().is_empty() {
                &suffix_lower
            } else {
                label.trim()
            };
            let expr = mood_pres.expression.unwrap_or_else(|| seeded_expression(seed));
            let ink = explicit_hex.unwrap_or("#e8483f");
            let mut renderer = PlushFeltRenderer::new(
                bloub_shape_to_engine(bloub_shape),
                ink,
                expr,
                mood_pres.state,
            );
            renderer.set_eye_scale(small_size_eye_scale(safe_size));
            let sample = renderer.sample_plush_frame(0.0, true);
            return render_plush_avatar_element(&sample, safe_size);
        }
        let seed = if suffix.is_empty() { base_seed } else { suffix };
        return render_plush_bot_avatar(seed, None, None, explicit_hex, safe_size, mood, activity);
    }

    let mapped_shape = legacy_shape_to_bloub_shape(trimmed);
    render_plush_bot_avatar(
        base_seed,
        mapped_shape,
        None,
        explicit_hex,
        safe_size,
        mood,
        activity,
    )
}

pub struct Avatar {
    pub label: String,
    pub size: f32,
    pub color: u32,
    pub avatar_url: Option<String>,
    pub shape: Option<String>,
    pub color_spec: Option<String>,
    pub persona_state: GrokPersonaState,
    pub is_online: bool,
    pub palette: TdesktopThemePalette,
}

impl Avatar {
    pub fn new(label: impl Into<String>, size: f32) -> Self {
        Self {
            label: label.into(),
            size,
            color: 0x2AABEE,
            avatar_url: None,
            shape: None,
            color_spec: None,
            persona_state: GrokPersonaState::Idle,
            is_online: false,
            palette: TdesktopThemePalette::default(),
        }
    }

    pub fn with_color(mut self, color: u32) -> Self {
        self.color = color;
        self
    }

    pub fn with_image_url(mut self, url: Option<String>) -> Self {
        self.avatar_url = url.filter(|u| !u.trim().is_empty());
        self
    }

    pub fn with_shape(mut self, shape: impl Into<String>) -> Self {
        let s = shape.into();
        self.shape = if s.trim().is_empty() { None } else { Some(s) };
        self
    }

    pub fn with_color_spec(mut self, color_spec: impl Into<String>) -> Self {
        let s = color_spec.into();
        self.color_spec = if s.trim().is_empty() { None } else { Some(s) };
        self
    }

    pub fn with_persona_state(mut self, persona_state: GrokPersonaState) -> Self {
        self.persona_state = persona_state;
        self
    }

    pub fn with_online(mut self, is_online: bool) -> Self {
        self.is_online = is_online;
        self
    }

    pub fn with_palette(mut self, palette: TdesktopThemePalette) -> Self {
        self.palette = palette;
        self
    }

    fn initials(&self) -> String {
        avatar_initials(&self.label)
    }
}

impl Render for Avatar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let initials = self.initials();
        let dot_size = (self.size * 0.26).clamp(8.0, 14.0);
        let resolved_color = self
            .color_spec
            .as_deref()
            .and_then(parse_fabushi_hex_color)
            .unwrap_or(self.color);

        let usable_url = self
            .avatar_url
            .as_deref()
            .filter(|u| is_usable_custom_avatar_url(u));

        if let Some(shape) = &self.shape {
            if usable_url.is_none() && !shape.trim().eq_ignore_ascii_case("classic") {
                let rendered = if shape.trim().to_ascii_lowercase().starts_with("grok2d:") {
                    let color_arg = self
                        .color_spec
                        .clone()
                        .unwrap_or_else(|| format!("#{:06X}", resolved_color & 0xFF_FFFF));
                    render_grok_bot_avatar(
                        GrokBotAvatarProps::new(&self.label, shape, color_arg, self.size)
                            .with_state(self.persona_state)
                            .with_surface_bg(pal.dialogs_bg),
                    )
                } else {
                    let hex_color = format!("#{:06x}", resolved_color & 0xFF_FFFF);
                    let paper_hex = format!("#{:06x}", pal.dialogs_bg & 0xFF_FFFF);
                    render_openbot_bot_plush_or_flat(
                        shape,
                        Some(hex_color),
                        &self.label,
                        self.persona_state,
                        self.size,
                        &paper_hex,
                    )
                };
                return rendered.when(self.is_online, |el| {
                    el.child(
                        div()
                            .absolute()
                            .right(px(0.0))
                            .bottom(px(0.0))
                            .w(px(dot_size))
                            .h(px(dot_size))
                            .rounded_full()
                            .bg(rgb(pal.online_dot_fg))
                            .border_2()
                            .border_color(rgb(pal.dialogs_bg)),
                    )
                });
            }
        }

        div()
            .relative()
            .w(px(self.size))
            .h(px(self.size))
            .rounded_full()
            .bg(rgb(resolved_color))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(pal.active_button_fg))
            .text_size(px((self.size * 0.36).clamp(11.0, 26.0)))
            .font_weight(FontWeight::BOLD)
            .map(|el| {
                if let Some(url) = usable_url {
                    el.overflow_hidden()
                        .child(img(url.to_string()).w_full().h_full().object_fit(ObjectFit::Cover))
                } else {
                    el.child(initials)
                }
            })
            .when(self.is_online, |el| {
                el.child(
                    div()
                        .absolute()
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .w(px(dot_size))
                        .h(px(dot_size))
                        .rounded_full()
                        .bg(rgb(pal.online_dot_fg))
                        .border_2()
                        .border_color(rgb(pal.dialogs_bg)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn parse_fabushi_hex_color_supports_hex6_hex3_and_grok_palette_ids() {
        assert_eq!(parse_fabushi_hex_color("#10B981"), Some(0x10B981));
        assert_eq!(parse_fabushi_hex_color("4f46e5"), Some(0x4F46E5));
        assert_eq!(parse_fabushi_hex_color("#f60"), Some(0xFF6600));
        assert_eq!(parse_fabushi_hex_color("#0ab"), Some(0x00AABB));
        assert_eq!(parse_fabushi_hex_color(""), None);
        assert_eq!(parse_fabushi_hex_color("#12345"), None);
        assert_eq!(parse_fabushi_hex_color("unknown"), None);

        for spec in GROK_COLOR_PALETTE {
            assert_eq!(parse_fabushi_hex_color(spec.id), Some(spec.swatch_rgb));
            assert_eq!(
                parse_fabushi_hex_color(&spec.id.to_ascii_uppercase()),
                Some(spec.swatch_rgb)
            );
        }
    }
}
