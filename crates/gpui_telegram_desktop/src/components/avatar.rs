use crate::theme::TdesktopThemePalette;
use gpui::prelude::FluentBuilder;
use gpui::*;

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

/// Renders one of the 8 Grok/Fabushi geometric avatar silhouettes (`blob`, `pebble`,
/// `squircle`, `tablet`, `wedge`, `hex`, `cloud`, `teardrop`, plus legacy aliases)
/// or classic circular initials when `shape` is `"classic"`.
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

    if let Some(openbot_div) =
        render_openbot_prefixed_avatar(norm, format!("#{:06x}", bg_hex & 0xFF_FFFF), label, GrokPersonaState::Idle, size_px, pal_paper_default())
    {
        return openbot_div;
    }

    render_grok_bot_avatar(GrokBotAvatarProps::new(
        label,
        shape,
        format!("#{:06X}", bg_hex & 0xFF_FFFF),
        size_px,
    ))
}

const fn pal_paper_default() -> &'static str {
    crate::components::bloub_avatar::DEFAULT_PAPER
}

fn grok_persona_to_openbot_mood(state: GrokPersonaState) -> openbot_contracts::avatar::AvatarMood {
    use openbot_contracts::avatar::AvatarMood;
    match state {
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
    }
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

fn is_openbot_prefixed_shape(shape: &str) -> bool {
    let lower = shape.trim().to_ascii_lowercase();
    lower == "bloub"
        || lower == "plush"
        || lower.starts_with("bloub:")
        || lower.starts_with("plush:")
}

fn render_openbot_prefixed_avatar(
    raw_shape: &str,
    color_hex: String,
    label: &str,
    persona_state: GrokPersonaState,
    size_px: f32,
    paper_hex: &str,
) -> Option<Div> {
    use crate::components::bloub_avatar::{
        BloubBotEngine, PlushCharacterId, PlushFeltRenderer, bloub_shape_to_engine,
        bloub_to_engine_shape, render_flat_bloub_element, render_plush_avatar_element,
        render_plush_seed_avatar, small_size_eye_scale,
    };
    use openbot_contracts::avatar::{
        BloubShape, avatar_mood_presentation, bloub_avatar_profile, plush_mood_presentation,
        seeded_expression,
    };

    let trimmed = raw_shape.trim();
    let lower = trimmed.to_ascii_lowercase();
    let mood = grok_persona_to_openbot_mood(persona_state);
    let explicit_hex = openbot_contracts::avatar::is_avatar_color(&color_hex).then_some(color_hex.as_str());

    if lower == "bloub" || lower.starts_with("bloub:") {
        let suffix = trimmed
            .split_once(':')
            .map(|(_, rest)| rest.trim())
            .unwrap_or("");
        let suffix_lower = suffix.to_ascii_lowercase();
        let mood_pres = avatar_mood_presentation(mood);
        if let Some(bloub_shape) = BloubShape::parse(&suffix_lower) {
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
            return Some(render_flat_bloub_element(&sample, ink, paper_hex, size_px));
        }
        let seed = if suffix.is_empty() { label.trim() } else { suffix };
        let profile = bloub_avatar_profile(seed, None, explicit_hex);
        let expr = mood_pres.expression.unwrap_or(profile.expression);
        let engine = BloubBotEngine::new(
            bloub_to_engine_shape(profile.shape),
            expr,
            mood_pres.state.to_parametric(),
        );
        let sample = engine.sample(0.0, true);
        return Some(render_flat_bloub_element(&sample, &profile.color, paper_hex, size_px));
    }

    if lower == "plush" || lower.starts_with("plush:") {
        let suffix = trimmed
            .split_once(':')
            .map(|(_, rest)| rest.trim())
            .unwrap_or("");
        let suffix_lower = suffix.to_ascii_lowercase();
        let mood_pres = plush_mood_presentation(mood);
        if let Some(ch) = PlushCharacterId::parse(&suffix_lower) {
            let mut renderer = PlushFeltRenderer::for_character(ch, mood_pres.state);
            if let Some(expr) = mood_pres.expression {
                renderer.set_expression(expr);
            }
            renderer.set_eye_scale(small_size_eye_scale(size_px));
            let sample = renderer.sample_plush_frame(0.0, true);
            return Some(render_plush_avatar_element(&sample, size_px));
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
            renderer.set_eye_scale(small_size_eye_scale(size_px));
            let sample = renderer.sample_plush_frame(0.0, true);
            return Some(render_plush_avatar_element(&sample, size_px));
        }
        let seed = if suffix.is_empty() { label.trim() } else { suffix };
        return Some(render_plush_seed_avatar(
            seed,
            None,
            explicit_hex,
            mood,
            0.0,
            true,
            size_px,
        ));
    }

    None
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
            if is_openbot_prefixed_shape(shape) && usable_url.is_none() {
                let hex_color = format!("#{:06x}", resolved_color & 0xFF_FFFF);
                let paper_hex = format!("#{:06x}", pal.dialogs_bg & 0xFF_FFFF);
                if let Some(openbot_el) = render_openbot_prefixed_avatar(
                    shape,
                    hex_color,
                    &self.label,
                    self.persona_state,
                    self.size,
                    &paper_hex,
                ) {
                    return openbot_el.when(self.is_online, |el| {
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
        }

        if let (Some(shape), None) = (&self.shape, &self.avatar_url) {
            if !shape.trim().eq_ignore_ascii_case("classic") {
                let color_arg = self
                    .color_spec
                    .clone()
                    .unwrap_or_else(|| format!("#{:06X}", resolved_color & 0xFF_FFFF));
                return render_grok_bot_avatar(
                    GrokBotAvatarProps::new(&self.label, shape, color_arg, self.size)
                        .with_state(self.persona_state)
                        .with_surface_bg(pal.dialogs_bg),
                )
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
                if let Some(ref url) = self.avatar_url {
                    el.overflow_hidden()
                        .child(img(url.clone()).w_full().h_full().object_fit(ObjectFit::Cover))
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
