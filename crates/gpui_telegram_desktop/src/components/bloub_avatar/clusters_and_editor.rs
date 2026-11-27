//! 1:1 OpenBot `AgentAvatar`, Cutout Rings, Felt Badges, `ChannelAvatar` Cluster, Activity, & `284px` Editor (`OB-F04`).
//! Mirrors `AgentAvatar.tsx`, `SidebarFrame.tsx`, `SidebarAgentIndicator.tsx`, `ChannelAvatar.tsx`,
//! `ChannelPictureEditor.tsx`, `ChannelActivityIndicator.tsx`, and `AgentSettingsPanel.tsx`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use base64::Engine as _;
use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{
    ACTIVITY_HOLD_MS, AVATAR_COLOR_CHOICES, AVATAR_EXPRESSIONS, AVATAR_HUE_CHOICES,
    AgentStepActivity, AvatarColorChoice, AvatarContractError, AvatarHue, AvatarMood, AvatarStyle,
    BloubAvatarProfile, BloubExpression, BloubShape, EngineShapeId,
    HOVER_FACES, HoverFace, PHOTO_BREATHE, PlushMoodPresentation, PlushState, PlushTransition,
    SUPPORTED_AVATAR_SILHOUETTES, ShapeSafeStateId, WORKING_FACE_MAX_MS, WORKING_FACE_MIN_MS,
    activity_mood_presentation, avatar_animation_phase, avatar_breathe_amount, avatar_default_seed,
    avatar_head_color, avatar_hue_for_color, avatar_hue_swatch, avatar_mood_presentation,
    avatar_seed_for_face, avatar_style_for_seed, bloub_avatar_profile, compute_agent_avatar_mood,
    is_avatar_color_choice, plush_mood_presentation, working_cycle_for_seed,
};
use openbot_contracts::channels::{
    CHANNEL_ICON_NAMES, ChannelIcon, ChannelIconName, ChannelMember,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::bot_engine::*;
use super::image_normalizer::{
    AvatarImageNormalizer, AvatarNormalizeError, NormalizedAvatarOutput,
};
use super::plush_felt::*;
use super::silhouettes::*;

pub const AVATAR_FPS: u32 = 80;
pub const BACKGROUND_AVATAR_FPS: u32 = 30;
pub const PLUSH_BODY_SCALE: f32 = 1.15;
pub const AVATAR_OPTICAL_BOX_RATIO: f32 = 1.28;
pub const AVATAR_OPTICAL_OFFSET_RATIO: f32 = -0.14;
pub const CONVERSATION_TITLE_AVATAR_SIZE: f32 = 18.0;
pub const CONVERSATION_TITLE_OPTICAL_OFFSET_PX: f32 = -2.0;
pub const WORKING_FACE_NOMINAL_MS: u64 = 1_800;
pub const ASLEEP_CUSTOM_IMAGE_OPACITY: f32 = 0.6;
pub const AVATAR_BREATHE_PERIOD_SEC: f64 = 2.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvatarMotionMode {
    Hover,
    Idle,
    Always,
}

impl AvatarMotionMode {
    pub const fn is_static(self) -> bool {
        matches!(self, Self::Hover | Self::Idle)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hover => "hover",
            Self::Idle => "idle",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarPlushRingKind {
    Default,
    Raised,
    Hover,
    Selected,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPlushRingSpec {
    pub kind: SidebarPlushRingKind,
    pub filter_id: &'static str,
    pub css_color_var: &'static str,
    pub blur_std_deviation: f32,
    pub alpha_slope: f32,
    pub alpha_intercept: f32,
    pub dilate_radius_px: f32,
    pub dark_fallback_rgb: u32,
    pub light_fallback_rgb: u32,
}

impl SidebarPlushRingKind {
    pub const ALL: [Self; 4] = [Self::Default, Self::Raised, Self::Hover, Self::Selected];

    pub const fn spec(self) -> SidebarPlushRingSpec {
        match self {
            Self::Default => SidebarPlushRingSpec {
                kind: Self::Default,
                filter_id: "sidebar-plush-ring",
                css_color_var: "var(--openbot-bg-sidebar)",
                blur_std_deviation: 1.5,
                alpha_slope: 8.0,
                alpha_intercept: -2.4,
                dilate_radius_px: 3.0,
                dark_fallback_rgb: 0x1A1A1A,
                light_fallback_rgb: 0xF5F5F6,
            },
            Self::Raised => SidebarPlushRingSpec {
                kind: Self::Raised,
                filter_id: "sidebar-plush-ring-raised",
                css_color_var: "var(--openbot-bg-control)",
                blur_std_deviation: 1.5,
                alpha_slope: 8.0,
                alpha_intercept: -2.4,
                dilate_radius_px: 3.0,
                dark_fallback_rgb: 0x212121,
                light_fallback_rgb: 0xFFFFFF,
            },
            Self::Hover => SidebarPlushRingSpec {
                kind: Self::Hover,
                filter_id: "sidebar-plush-ring-hover",
                css_color_var: "var(--openbot-bg-control-hover)",
                blur_std_deviation: 1.5,
                alpha_slope: 8.0,
                alpha_intercept: -2.4,
                dilate_radius_px: 3.0,
                dark_fallback_rgb: 0x242424,
                light_fallback_rgb: 0xF0F0F2,
            },
            Self::Selected => SidebarPlushRingSpec {
                kind: Self::Selected,
                filter_id: "sidebar-plush-ring-selected",
                css_color_var: "var(--sidebar-row-selected)",
                blur_std_deviation: 1.5,
                alpha_slope: 8.0,
                alpha_intercept: -2.4,
                dilate_radius_px: 3.0,
                dark_fallback_rgb: 0x2A2A2A,
                light_fallback_rgb: 0xE4E4E7,
            },
        }
    }

    pub const fn select_for_row(
        hovered: bool,
        active: bool,
        fine_pointer_wide: bool,
        light_scheme: bool,
        sidebar_dragging: bool,
    ) -> Self {
        if sidebar_dragging && hovered && !active {
            return Self::Default;
        }
        if light_scheme && active {
            return Self::Selected;
        }
        if hovered && fine_pointer_wide {
            return Self::Hover;
        }
        if hovered || active {
            return Self::Raised;
        }
        Self::Default
    }
}

pub fn sidebar_plush_ring_svg_defs() -> String {
    let mut svg = String::from(
        r#"<svg class="sidebar-plush-ring-defs" width="0" height="0" aria-hidden="true">"#,
    );
    for ring in SidebarPlushRingKind::ALL {
        let s = ring.spec();
        svg.push_str(&format!(
            concat!(
                r#"<filter id="{}" x="-50%" y="-50%" width="200%" height="200%" color-interpolation-filters="sRGB">"#,
                r#"<feGaussianBlur in="SourceAlpha" stdDeviation="{}" result="soft"/>"#,
                r#"<feComponentTransfer in="soft" result="hard"><feFuncA type="linear" slope="{}" intercept="{}"/></feComponentTransfer>"#,
                r#"<feMorphology in="hard" operator="dilate" radius="{}" result="grown"/>"#,
                r#"<feFlood style="flood-color:{}"/>"#,
                r#"<feComposite in2="grown" operator="in" result="ring"/>"#,
                r#"<feComposite in="SourceGraphic" in2="hard" operator="in" result="face"/>"#,
                r#"<feMerge><feMergeNode in="ring"/><feMergeNode in="face"/></feMerge>"#,
                r#"</filter>"#,
            ),
            s.filter_id,
            s.blur_std_deviation,
            s.alpha_slope,
            s.alpha_intercept,
            s.dilate_radius_px,
            s.css_color_var,
        ));
    }
    svg.push_str("</svg>");
    svg
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentAvatarProps {
    pub seed: String,
    pub hue: Option<AvatarHue>,
    pub color: Option<String>,
    pub avatar_url: Option<String>,
    pub failed_url: Option<String>,
    pub size_px: f32,
    pub optical_offset_override_px: Option<f32>,
    pub motion: AvatarMotionMode,
    pub mood: AvatarMood,
    pub animation_offset: Option<f64>,
    pub shape_override: Option<BloubShape>,
    pub expression_override: Option<BloubExpression>,
    pub transition: PlushTransition,
    pub follow_pointer: bool,
    pub working_faces: bool,
    pub activity: Option<AgentStepActivity>,
    pub eye_scale: f64,
    pub reduced_motion: bool,
    pub ambient_motion: bool,
    pub document_focused: bool,
    pub on_screen: bool,
    pub cutout_ring: Option<SidebarPlushRingKind>,
    pub dark_scheme: bool,
}

impl Default for AgentAvatarProps {
    fn default() -> Self {
        Self::new("agent")
    }
}

impl AgentAvatarProps {
    pub fn new(seed: impl Into<String>) -> Self {
        Self {
            seed: seed.into(),
            hue: None,
            color: None,
            avatar_url: None,
            failed_url: None,
            size_px: 32.0,
            optical_offset_override_px: None,
            motion: AvatarMotionMode::Hover,
            mood: AvatarMood::Idle,
            animation_offset: None,
            shape_override: None,
            expression_override: None,
            transition: PlushTransition::Pop,
            follow_pointer: false,
            working_faces: false,
            activity: None,
            eye_scale: 1.0,
            reduced_motion: false,
            ambient_motion: true,
            document_focused: true,
            on_screen: true,
            cutout_ring: None,
            dark_scheme: true,
        }
    }

    pub fn with_size(mut self, size_px: f32) -> Self {
        self.size_px = size_px;
        self
    }

    pub fn with_mood(mut self, mood: AvatarMood) -> Self {
        self.mood = mood;
        self
    }

    pub fn with_mood_flags(
        mut self,
        failed_turn: bool,
        waiting_input: bool,
        is_working: bool,
        recent_reply: bool,
        asleep: bool,
        connecting: bool,
    ) -> Self {
        self.mood = compute_agent_avatar_mood(
            failed_turn,
            waiting_input,
            is_working,
            recent_reply,
            asleep,
            connecting,
        );
        self
    }

    pub fn with_url(mut self, url: Option<String>) -> Self {
        self.avatar_url = url.filter(|s| !s.trim().is_empty());
        self
    }

    pub fn with_hue_and_color(mut self, hue: Option<AvatarHue>, color: Option<String>) -> Self {
        self.hue = hue;
        self.color = color;
        self
    }

    pub fn optical_box_px(&self) -> f32 {
        self.size_px * AVATAR_OPTICAL_BOX_RATIO
    }

    pub fn optical_offset_px(&self) -> f32 {
        if let Some(explicit) = self.optical_offset_override_px {
            explicit
        } else if (self.size_px - CONVERSATION_TITLE_AVATAR_SIZE).abs() < 0.1 {
            CONVERSATION_TITLE_OPTICAL_OFFSET_PX
        } else {
            self.size_px * AVATAR_OPTICAL_OFFSET_RATIO
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentAvatarResolvedState {
    pub uses_custom_image: bool,
    pub active_url: Option<String>,
    pub custom_image_opacity: f32,
    pub style: AvatarStyle,
    pub shape: BloubShape,
    pub color: String,
    pub plush_state: PlushState,
    pub flat_state: ShapeSafeStateId,
    pub expression: BloubExpression,
    pub busy: bool,
    pub breathe_amount: f64,
    pub animated: bool,
    pub fps: u32,
    pub initial_phase: f64,
    pub frozen_at: Option<f64>,
    pub plush_body_scale: f32,
}

#[derive(Debug, Clone)]
pub struct AgentAvatarController {
    pub tracked_url: Option<String>,
    pub failed_url: Option<String>,
    pub interacting: bool,
    pub hover_face: Option<HoverFace>,
    pub working_tick: usize,
    pub working_face: BloubExpression,
    pub next_working_switch_ms: u64,
    pub held_activity: Option<AgentStepActivity>,
    pub activity_shown_at_ms: Option<u64>,
    pub pending_activity: Option<(Option<AgentStepActivity>, u64)>,
    prng: Mulberry32,
}

impl Default for AgentAvatarController {
    fn default() -> Self {
        Self::new("agent")
    }
}

impl AgentAvatarController {
    pub fn new(seed: &str) -> Self {
        let seed_hash = openbot_contracts::avatar::fnv1a_utf16_32(seed);
        Self {
            tracked_url: None,
            failed_url: None,
            interacting: false,
            hover_face: None,
            working_tick: 0,
            working_face: working_cycle_for_seed(seed, 0),
            next_working_switch_ms: WORKING_FACE_NOMINAL_MS,
            held_activity: None,
            activity_shown_at_ms: None,
            pending_activity: None,
            prng: Mulberry32::new(seed_hash),
        }
    }

    pub fn sync_url(&mut self, next_url: Option<&str>) {
        let normalized = next_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if normalized != self.tracked_url {
            self.tracked_url = normalized;
            self.failed_url = None;
        }
    }

    pub fn mark_image_failed(&mut self, url: &str) {
        self.failed_url = Some(url.trim().to_string());
    }

    pub fn set_interacting(&mut self, interacting: bool, resting: BloubExpression) {
        if interacting && !self.interacting {
            let prev = self.hover_face;
            let choices: Vec<HoverFace> = HOVER_FACES
                .iter()
                .copied()
                .filter(|f| Some(*f) != prev && f.expression != Some(resting))
                .collect();
            let chosen = if choices.is_empty() {
                HOVER_FACES[0]
            } else {
                let idx = ((self.prng.next_f64() * (choices.len() as f64)).floor() as usize)
                    .min(choices.len() - 1);
                choices[idx]
            };
            self.hover_face = Some(chosen);
        }
        self.interacting = interacting;
    }

    /// Enforces `ACTIVITY_HOLD_MS = 1_400`ms minimum display hold (`AgentAvatar.tsx:L422-L439`).
    pub fn sync_activity(&mut self, requested: Option<AgentStepActivity>, now_ms: u64) {
        if let Some((pending, due_ms)) = self.pending_activity
            && now_ms >= due_ms
        {
            self.held_activity = pending;
            self.activity_shown_at_ms = Some(now_ms);
            self.pending_activity = None;
        }
        if requested == self.held_activity && self.pending_activity.is_none() {
            return;
        }
        let hold = u64::from(ACTIVITY_HOLD_MS);
        match self.activity_shown_at_ms {
            None => {
                self.held_activity = requested;
                self.activity_shown_at_ms = Some(now_ms);
                self.pending_activity = None;
            }
            Some(shown_at) => {
                let unlock_at = shown_at.saturating_add(hold);
                if now_ms >= unlock_at {
                    self.held_activity = requested;
                    self.activity_shown_at_ms = Some(now_ms);
                    self.pending_activity = None;
                } else {
                    self.pending_activity = Some((requested, unlock_at));
                }
            }
        }
    }

    /// Advances working expression cycle (`1800ms` nominal, `700..=3200ms` live jitter).
    pub fn tick_working_faces(
        &mut self,
        seed: &str,
        now_ms: u64,
        deterministic_nominal: bool,
    ) -> BloubExpression {
        if now_ms >= self.next_working_switch_ms {
            self.working_tick = self.working_tick.saturating_add(1);
            self.working_face = working_cycle_for_seed(seed, self.working_tick);
            let span = u64::from(WORKING_FACE_MAX_MS - WORKING_FACE_MIN_MS);
            let delta = if deterministic_nominal {
                WORKING_FACE_NOMINAL_MS
            } else {
                u64::from(WORKING_FACE_MIN_MS)
                    + ((self.prng.next_f64() * (span as f64)).round() as u64)
            };
            self.next_working_switch_ms = now_ms.saturating_add(delta);
        }
        self.working_face
    }

    pub fn resolve(&self, props: &AgentAvatarProps) -> AgentAvatarResolvedState {
        let seed = if props.seed.trim().is_empty() {
            "agent"
        } else {
            props.seed.as_str()
        };
        let profile = bloub_avatar_profile(seed, props.hue, props.color.as_deref());
        let style = avatar_style_for_seed(seed);
        let resting = props.expression_override.unwrap_or(profile.expression);

        let url_candidate = props
            .avatar_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let failed = props
            .failed_url
            .as_deref()
            .or(self.failed_url.as_deref())
            .map(str::trim);
        let active_url = url_candidate
            .filter(|u| Some(*u) != failed && is_renderable_avatar_url(u))
            .map(str::to_string);
        let uses_custom_image = active_url.is_some();

        let cycles_faces = props.working_faces && props.mood == AvatarMood::Working;
        let requested_activity =
            if props.mood == AvatarMood::Working && !props.working_faces && !props.reduced_motion {
                self.held_activity.or(props.activity)
            } else {
                None
            };
        let step_mood: Option<PlushMoodPresentation> =
            requested_activity.map(activity_mood_presentation);

        let plush_mood = if cycles_faces {
            PlushMoodPresentation {
                state: PlushState::Idle,
                expression: Some(if props.reduced_motion {
                    BloubExpression::Attentif
                } else {
                    self.working_face
                }),
                busy: true,
            }
        } else {
            step_mood.unwrap_or_else(|| plush_mood_presentation(props.mood))
        };

        let hovered = self.interacting
            && !props.reduced_motion
            && props.mood == AvatarMood::Idle
            && props.shape_override.is_none()
            && props.expression_override.is_none();
        let hover = if hovered { self.hover_face } else { None };

        let plush_state = hover.map_or(plush_mood.state, |h| h.state);
        let plush_expr = plush_mood
            .expression
            .or_else(|| hover.and_then(|h| h.expression))
            .unwrap_or(resting);

        let flat_pres = avatar_mood_presentation(props.mood);
        let working_expr = if cycles_faces {
            plush_mood.expression
        } else {
            step_mood.and_then(|m| m.expression)
        };
        let step_safe = match step_mood.map(|m| m.state) {
            Some(PlushState::Wide) => Some(ShapeSafeStateId::Wide),
            Some(PlushState::Wink) => Some(ShapeSafeStateId::Wink),
            _ => None,
        };
        let flat_state = if hover.is_some_and(|h| h.state == PlushState::Wink) {
            ShapeSafeStateId::Wink
        } else {
            step_safe.unwrap_or(flat_pres.state)
        };
        let flat_expr = working_expr
            .or(flat_pres.expression)
            .or_else(|| hover.and_then(|h| h.expression))
            .unwrap_or(resting);

        let expression = match style {
            AvatarStyle::Plush => plush_expr,
            AvatarStyle::Flat => flat_expr,
        };
        let breathe_amount = if !props.ambient_motion || props.reduced_motion {
            0.0
        } else if uses_custom_image {
            if plush_mood_presentation(props.mood).busy {
                PHOTO_BREATHE
            } else {
                0.0
            }
        } else {
            avatar_breathe_amount(props.mood, style)
        };
        let animated = !props.reduced_motion
            && ((props.follow_pointer && props.document_focused && props.on_screen)
                || (plush_mood.busy && props.ambient_motion)
                || (!props.motion.is_static() && props.document_focused)
                || self.interacting);
        let fps = if self.interacting || props.follow_pointer || !props.motion.is_static() {
            AVATAR_FPS
        } else {
            BACKGROUND_AVATAR_FPS
        };
        let frozen_at = props.reduced_motion.then(|| match style {
            AvatarStyle::Plush => plush_state.frozen_pose_time(),
            AvatarStyle::Flat => flat_state.to_parametric().canonical_pose_time(),
        });

        AgentAvatarResolvedState {
            uses_custom_image,
            active_url,
            custom_image_opacity: if uses_custom_image && props.mood == AvatarMood::Asleep {
                ASLEEP_CUSTOM_IMAGE_OPACITY
            } else {
                1.0
            },
            style,
            shape: props.shape_override.unwrap_or(profile.shape),
            color: profile.color,
            plush_state,
            flat_state,
            expression,
            busy: plush_mood.busy,
            breathe_amount,
            animated,
            fps,
            initial_phase: props
                .animation_offset
                .unwrap_or_else(|| avatar_animation_phase(seed)),
            frozen_at,
            plush_body_scale: PLUSH_BODY_SCALE,
        }
    }
}

fn is_renderable_avatar_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix("data:") {
        let Some((mime, b64)) = rest.split_once(";base64,") else {
            return false;
        };
        if !mime.starts_with("image/") {
            return false;
        }
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return false;
        };
        return image::guess_format(&raw).is_ok();
    }
    if trimmed.starts_with('/') || trimmed.starts_with("file://") {
        let fs_path = trimmed.strip_prefix("file://").unwrap_or(trimmed);
        let Ok(raw) = std::fs::read(fs_path) else {
            return false;
        };
        return image::guess_format(&raw).is_ok();
    }
    trimmed.starts_with("http://") || trimmed.starts_with("https://")
}

fn plush_asset_file(rel: &str) -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(rel);
    if manifest.is_file() {
        return Some(manifest);
    }
    let cwd = PathBuf::from("crates/gpui_telegram_desktop/assets").join(rel);
    if cwd.is_file() {
        return Some(cwd);
    }
    None
}

fn parse_hex_u32(hex: &str, fallback: u32) -> u32 {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() >= 6 {
        u32::from_str_radix(&clean[0..6], 16).unwrap_or(fallback)
    } else {
        fallback
    }
}

fn bloub_to_engine_shape(shape: BloubShape) -> EngineShapeId {
    parse_engine_shape(shape.as_str()).unwrap_or(EngineShapeId::Cercle)
}

pub fn render_agent_flat_svg(
    props: &AgentAvatarProps,
    controller: Option<&AgentAvatarController>,
    now_sec: f64,
) -> String {
    let default_ctrl = AgentAvatarController::new(&props.seed);
    let resolved = controller.unwrap_or(&default_ctrl).resolve(props);
    let eng_shape = bloub_to_engine_shape(resolved.shape);
    let eng_state = resolved.flat_state.to_parametric();
    let sample_t = resolved
        .frozen_at
        .unwrap_or(now_sec + resolved.initial_phase);
    let sample = BloubBotEngine::new(eng_shape, resolved.expression, eng_state)
        .sample(sample_t, props.reduced_motion || !resolved.animated);
    render_flat_svg(&sample, &resolved.color, DEFAULT_PAPER)
}

fn decode_url_to_render_image(url: &str) -> Option<Arc<RenderImage>> {
    let trimmed = url.trim();
    let raw = if let Some(rest) = trimmed.strip_prefix("data:") {
        let (_, b64) = rest.split_once(";base64,")?;
        base64::engine::general_purpose::STANDARD.decode(b64).ok()?
    } else if trimmed.starts_with('/') || trimmed.starts_with("file://") {
        let fs_path = trimmed.strip_prefix("file://").unwrap_or(trimmed);
        std::fs::read(fs_path).ok()?
    } else {
        return None;
    };
    let mut rgba = image::load_from_memory(&raw).ok()?.into_rgba8();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

pub fn render_agent_avatar(
    props: &AgentAvatarProps,
    controller: Option<&AgentAvatarController>,
    now_sec: f64,
) -> Div {
    let default_ctrl = AgentAvatarController::new(&props.seed);
    let resolved = controller.unwrap_or(&default_ctrl).resolve(props);
    let base_size = props.size_px.max(8.0);
    let breathe_scale = if resolved.breathe_amount > 0.0 && !props.reduced_motion {
        let wave = ((std::f64::consts::TAU * now_sec) / AVATAR_BREATHE_PERIOD_SEC).cos();
        (1.0 - resolved.breathe_amount * wave) as f32
    } else {
        1.0
    };

    if resolved.uses_custom_image
        && let Some(url) = resolved.active_url.as_deref()
    {
        let disp = base_size * breathe_scale;
        let inset = (base_size - disp) * 0.5;
        let decoded = decode_url_to_render_image(url);
        let url_owned = url.to_string();
        return div()
            .relative()
            .w(px(base_size))
            .h(px(base_size))
            .flex_shrink_0()
            .opacity(resolved.custom_image_opacity)
            .child(
                div()
                    .absolute()
                    .left(px(inset))
                    .top(px(inset))
                    .w(px(disp))
                    .h(px(disp))
                    .rounded_full()
                    .overflow_hidden()
                    .map(|el| {
                        if let Some(render_img) = decoded {
                            el.child(
                                img(render_img)
                                    .w_full()
                                    .h_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                        } else {
                            el.child(
                                img(url_owned)
                                    .w_full()
                                    .h_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                        }
                    }),
            );
    }

    let optical_box = props.optical_box_px() * breathe_scale;
    let optical_offset_y = props.optical_offset_px();
    let box_left = (base_size - optical_box) * 0.5;
    let box_top = (base_size - optical_box) * 0.5 + optical_offset_y;
    let sample_t = resolved
        .frozen_at
        .unwrap_or(now_sec + resolved.initial_phase);
    let eng_shape = bloub_to_engine_shape(resolved.shape);

    if resolved.style == AvatarStyle::Flat {
        let eng_state = resolved.flat_state.to_parametric();
        let sample = BloubBotEngine::new(eng_shape, resolved.expression, eng_state)
            .sample(sample_t, props.reduced_motion || !resolved.animated);
        let flat_el =
            render_flat_bloub_element(&sample, &resolved.color, DEFAULT_PAPER, optical_box);
        return div()
            .relative()
            .w(px(base_size))
            .h(px(base_size))
            .flex_shrink_0()
            .child(
                div()
                    .absolute()
                    .left(px(box_left))
                    .top(px(box_top))
                    .w(px(optical_box))
                    .h(px(optical_box))
                    .child(flat_el),
            );
    }

    let mut renderer = PlushFeltRenderer::new(
        eng_shape,
        &resolved.color,
        resolved.expression,
        resolved.plush_state,
    );
    let eff_eye_scale = small_size_eye_scale(base_size).max(if props.eye_scale.is_finite() && props.eye_scale > 0.0 {
        props.eye_scale
    } else {
        1.0
    });
    renderer.set_eye_scale(eff_eye_scale);
    let plush_sample =
        renderer.sample_plush_frame(sample_t, props.reduced_motion || !resolved.animated);

    let plush_size = base_size * breathe_scale;
    let plush_inset = (base_size - plush_size) * 0.5;
    let extra_top = optical_offset_y;
    let plush_px = optical_box * resolved.plush_body_scale;
    let plush_left = (base_size - plush_px) * 0.5;
    let plush_top = (base_size - plush_px) * 0.5 + optical_offset_y;
    let ring_spec = props.cutout_ring.map(SidebarPlushRingKind::spec);
    let dark_scheme = props.dark_scheme;
    let shape = resolved.shape;
    let sil_rot = plush_sample.silhouette.rot;
    let sil_cx = plush_sample.silhouette.cx;
    let sil_cy = plush_sample.silhouette.cy;
    let sil_sx = plush_sample.silhouette.sx;
    let sil_sy = plush_sample.silhouette.sy;

    div()
        .relative()
        .w(px(base_size))
        .h(px(base_size))
        .flex_shrink_0()
        .when_some(ring_spec, |el, ring| {
            el.child(
                div()
                    .absolute()
                    .left(px(plush_left))
                    .top(px(plush_top))
                    .w(px(plush_px))
                    .h(px(plush_px))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _cx| {
                                let center = bounds.center();
                                let min_dim =
                                    f32::from(bounds.size.width.min(bounds.size.height));
                                let px_per_vb = (min_dim / (VIEW_BOX as f32)).max(0.01);
                                let ring_rgb = if dark_scheme {
                                    ring.dark_fallback_rgb
                                } else {
                                    ring.light_fallback_rgb
                                };
                                let dilate_scale = 1.0
                                    + f64::from(
                                        ring.dilate_radius_px / (min_dim * 0.32).max(1.0),
                                    );
                                let ring_sil = Silhouette {
                                    radii: bloub_shape_radii(shape),
                                    rot: sil_rot,
                                    cx: sil_cx,
                                    cy: sil_cy,
                                    sx: sil_sx * dilate_scale,
                                    sy: sil_sy * dilate_scale,
                                };
                                if let Some(ring_path) =
                                    silhouette_gpui_path(&ring_sil, center, px_per_vb)
                                {
                                    window.paint_path(ring_path, rgb(ring_rgb));
                                }
                            },
                        )
                        .w(px(plush_px))
                        .h(px(plush_px)),
                    ),
            )
        })
        .child(
            div()
                .absolute()
                .left(px(plush_inset))
                .top(px(plush_inset + extra_top))
                .child(render_plush_avatar_element(&plush_sample, plush_size)),
        )
}

pub const SIDEBAR_WAIT_TOOLTIP_OPEN_DELAY_MS: u64 = 350;
pub const SIDEBAR_BADGE_RESIZE_MS: u64 = 280;
pub const SIDEBAR_BADGE_BUMP_MS: u64 = 360;
pub const SIDEBAR_BADGE_ROLL_IN_MS: u64 = 280;
pub const SIDEBAR_BADGE_ROLL_OUT_MS: u64 = 200;
pub const SIDEBAR_BADGE_ROLL_OFFSET_RATIO: f32 = 0.65;
pub const SIDEBAR_FELT_RING_WIDTH_PX: f32 = 3.5;
pub const SIDEBAR_FELT_RING_BORDER_PX: f32 = 3.0;
pub const SIDEBAR_FELT_DEFAULT_BOX_PX: f32 = 16.0;
pub const SIDEBAR_FELT_WAITING_BOX_PX: f32 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarWaitReason {
    Question,
    Approval,
    Takeover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarRoutinePhase {
    Running,
    Queued,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SidebarFeltBadge {
    Unread { count: u64 },
    Waiting { reason: SidebarWaitReason },
    Routine { phase: SidebarRoutinePhase },
    Working,
    Limited,
    Responded,
}

impl SidebarFeltBadge {
    pub const fn routine_failed() -> Self {
        Self::Routine {
            phase: SidebarRoutinePhase::Failed,
        }
    }

    pub const fn is_felt_surface(self) -> bool {
        matches!(
            self,
            Self::Unread { .. }
                | Self::Waiting { .. }
                | Self::Routine {
                    phase: SidebarRoutinePhase::Failed
                }
        )
    }

    pub const fn felt_asset_path(self) -> Option<&'static str> {
        match self {
            Self::Unread { .. } => Some("openbot_plush/ui/felt-pill-lilac.webp"),
            Self::Waiting { .. } => Some("openbot_plush/ui/felt-pill-amber.webp"),
            Self::Routine {
                phase: SidebarRoutinePhase::Failed,
            } => Some("openbot_plush/ui/felt-x-ball.webp"),
            _ => None,
        }
    }

    pub const fn box_px(self) -> f32 {
        match self {
            Self::Waiting { .. } => SIDEBAR_FELT_WAITING_BOX_PX,
            _ => SIDEBAR_FELT_DEFAULT_BOX_PX,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Unread { .. } => "unread",
            Self::Waiting { .. } => "waiting",
            Self::Routine {
                phase: SidebarRoutinePhase::Running,
            } => "routine-running",
            Self::Routine {
                phase: SidebarRoutinePhase::Queued,
            } => "routine-queued",
            Self::Routine {
                phase: SidebarRoutinePhase::Failed,
            } => "routine-failed",
            Self::Working => "working",
            Self::Limited => "limited",
            Self::Responded => "responded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarBadgePhase {
    Idle,
    In,
    Bump,
    BumpB,
    Out,
}

#[derive(Debug, Clone)]
pub struct SidebarBadgeAnimator {
    pub current: Option<SidebarFeltBadge>,
    pub phase: SidebarBadgePhase,
    pub previous_count: Option<u64>,
    pub phase_started_ms: u64,
    pub from_width_px: f32,
    pub to_width_px: f32,
}

impl Default for SidebarBadgeAnimator {
    fn default() -> Self {
        Self {
            current: None,
            phase: SidebarBadgePhase::Idle,
            previous_count: None,
            phase_started_ms: 0,
            from_width_px: 22.0,
            to_width_px: 22.0,
        }
    }
}

impl SidebarBadgeAnimator {
    fn estimate_pill_width(count: u64) -> f32 {
        let digits = count.max(1).to_string().len() as f32;
        let inner = (6.0 + digits * 8.5).max(SIDEBAR_FELT_DEFAULT_BOX_PX);
        inner + 2.0 * SIDEBAR_FELT_RING_BORDER_PX
    }

    pub fn update(&mut self, next: Option<SidebarFeltBadge>, now_ms: u64, reduced_motion: bool) {
        if reduced_motion {
            self.current = next;
            self.phase = SidebarBadgePhase::Idle;
            self.previous_count = None;
            if let Some(SidebarFeltBadge::Unread { count }) = next {
                let w = Self::estimate_pill_width(count);
                self.from_width_px = w;
                self.to_width_px = w;
            }
            return;
        }
        match (self.current, next) {
            (Some(prev), None) => {
                if matches!(
                    prev,
                    SidebarFeltBadge::Unread { .. }
                        | SidebarFeltBadge::Routine {
                            phase: SidebarRoutinePhase::Failed
                        }
                ) {
                    self.phase = SidebarBadgePhase::Out;
                    self.phase_started_ms = now_ms;
                } else {
                    self.current = None;
                    self.phase = SidebarBadgePhase::Idle;
                    self.previous_count = None;
                }
            }
            (
                Some(SidebarFeltBadge::Unread { count: old_c }),
                Some(SidebarFeltBadge::Unread { count: new_c }),
            ) => {
                if old_c == new_c {
                    return;
                }
                self.from_width_px = Self::estimate_pill_width(old_c);
                self.to_width_px = Self::estimate_pill_width(new_c);
                self.previous_count = (new_c > 0).then_some(old_c);
                self.current = next;
                self.phase = if self.phase == SidebarBadgePhase::Bump {
                    SidebarBadgePhase::BumpB
                } else {
                    SidebarBadgePhase::Bump
                };
                self.phase_started_ms = now_ms;
            }
            (prev, Some(next_badge)) => {
                let changed_key = prev.map(SidebarFeltBadge::key) != Some(next_badge.key());
                self.previous_count = None;
                self.current = Some(next_badge);
                if let SidebarFeltBadge::Unread { count } = next_badge {
                    let w = Self::estimate_pill_width(count);
                    self.from_width_px = w;
                    self.to_width_px = w;
                }
                if changed_key
                    && (next_badge.is_felt_surface() || next_badge == SidebarFeltBadge::Responded)
                {
                    self.phase = SidebarBadgePhase::In;
                    self.phase_started_ms = now_ms;
                }
            }
            (None, None) => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarBadgeTransformSample {
    pub scale_x: f32,
    pub scale_y: f32,
    pub opacity: f32,
    pub width_px: f32,
    pub roll_in_translate_ratio: f32,
    pub roll_out_translate_ratio: f32,
}

impl SidebarBadgeAnimator {
    pub fn sample(&self, now_ms: u64) -> SidebarBadgeTransformSample {
        let elapsed = now_ms.saturating_sub(self.phase_started_ms) as f32;
        match self.phase {
            SidebarBadgePhase::Idle => SidebarBadgeTransformSample {
                scale_x: 1.0,
                scale_y: 1.0,
                opacity: 1.0,
                width_px: self.to_width_px,
                roll_in_translate_ratio: 0.0,
                roll_out_translate_ratio: -SIDEBAR_BADGE_ROLL_OFFSET_RATIO,
            },
            SidebarBadgePhase::In => {
                let u = (elapsed / 200.0).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - u).powi(3);
                let s = 0.7 + 0.3 * eased;
                SidebarBadgeTransformSample {
                    scale_x: s,
                    scale_y: s,
                    opacity: eased,
                    width_px: self.to_width_px,
                    roll_in_translate_ratio: 0.0,
                    roll_out_translate_ratio: -SIDEBAR_BADGE_ROLL_OFFSET_RATIO,
                }
            }
            SidebarBadgePhase::Bump | SidebarBadgePhase::BumpB => {
                let u = (elapsed / (SIDEBAR_BADGE_BUMP_MS as f32)).clamp(0.0, 1.0);
                let (sx, sy) = if u < 0.35 {
                    let k = u / 0.35;
                    (1.0 + 0.12 * k, 1.0 - 0.12 * k)
                } else if u < 0.65 {
                    let k = (u - 0.35) / 0.30;
                    (1.12 - 0.15 * k, 0.88 + 0.16 * k)
                } else {
                    let k = (u - 0.65) / 0.35;
                    (0.97 + 0.03 * k, 1.04 - 0.04 * k)
                };
                let rw = (elapsed / (SIDEBAR_BADGE_RESIZE_MS as f32)).clamp(0.0, 1.0);
                let rw_ease = 1.0 - (1.0 - rw).powi(3);
                let width_px =
                    self.from_width_px + (self.to_width_px - self.from_width_px) * rw_ease;
                let ri = (elapsed / (SIDEBAR_BADGE_ROLL_IN_MS as f32)).clamp(0.0, 1.0);
                let ro = (elapsed / (SIDEBAR_BADGE_ROLL_OUT_MS as f32)).clamp(0.0, 1.0);
                SidebarBadgeTransformSample {
                    scale_x: sx,
                    scale_y: sy,
                    opacity: 1.0,
                    width_px,
                    roll_in_translate_ratio: SIDEBAR_BADGE_ROLL_OFFSET_RATIO * (1.0 - ri),
                    roll_out_translate_ratio: -SIDEBAR_BADGE_ROLL_OFFSET_RATIO * ro,
                }
            }
            SidebarBadgePhase::Out => {
                let u = (elapsed / 240.0).clamp(0.0, 1.0);
                let (sx, sy) = if u < 0.30 {
                    let k = u / 0.30;
                    (1.0 + 0.10 * k, 1.0 - 0.10 * k)
                } else {
                    let k = (u - 0.30) / 0.70;
                    (1.10 - 0.90 * k, 0.90 - 0.70 * k)
                };
                SidebarBadgeTransformSample {
                    scale_x: sx,
                    scale_y: sy,
                    opacity: 1.0 - u,
                    width_px: self.to_width_px,
                    roll_in_translate_ratio: 0.0,
                    roll_out_translate_ratio: -SIDEBAR_BADGE_ROLL_OFFSET_RATIO,
                }
            }
        }
    }
}

fn cached_ui_felt_webp(rel: &'static str) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Arc<RenderImage>>>>> =
        OnceLock::new();
    let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(found) = map.lock().get(rel).cloned() {
        return found;
    }
    let loaded = plush_asset_file(rel)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|raw| {
            let mut rgba = image::load_from_memory(&raw).ok()?.into_rgba8();
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
        });
    map.lock().insert(rel, loaded.clone());
    loaded
}

/// High-level semantic OpenBot felt status badge state (`PROJECT.md` M1 contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeltStatusBadgeState {
    Working3Balls,
    UnreadPlum(u32),
    WaitingAmber,
    RespondedGreen,
    ErrorRaspberry,
}

impl FeltStatusBadgeState {
    pub const fn to_sidebar_badge(self) -> SidebarFeltBadge {
        match self {
            Self::Working3Balls => SidebarFeltBadge::Working,
            Self::UnreadPlum(count) => SidebarFeltBadge::Unread {
                count: count as u64,
            },
            Self::WaitingAmber => SidebarFeltBadge::Waiting {
                reason: SidebarWaitReason::Approval,
            },
            Self::RespondedGreen => SidebarFeltBadge::Responded,
            Self::ErrorRaspberry => SidebarFeltBadge::routine_failed(),
        }
    }
}

/// Computes `(dy_px, width_px, height_px)` for ball `ball_idx` (`0..3`) in the `30x16px`
/// OpenBot working pill (`960ms` cycle, `0/130/260ms` stagger, peak `-1.75px`, static at `now_ms == 0`).
pub fn working_ball_hop_sample(now_ms: u64, ball_idx: usize) -> (f32, f32, f32) {
    const BASE_PX: f32 = 7.0;
    if now_ms == 0 {
        return (0.0, BASE_PX, BASE_PX);
    }
    const PERIOD_MS: u64 = 960;
    const STAGGER_MS: u64 = 130;
    let stagger = (ball_idx as u64) * STAGGER_MS;
    let phase = ((now_ms + PERIOD_MS - (stagger % PERIOD_MS)) % PERIOD_MS) as f32 / (PERIOD_MS as f32);
    let (dy, sx, sy) = if phase < 0.08 {
        let u = phase / 0.08;
        (0.0, 1.0 + 0.10 * u, 1.0 - 0.10 * u)
    } else if phase < 0.26 {
        let u = (phase - 0.08) / 0.18;
        let ease = 1.0 - (1.0 - u) * (1.0 - u);
        (-1.75 * ease, 1.0 - 0.06 * (1.0 - u), 1.0 + 0.08 * (1.0 - u))
    } else if phase < 0.46 {
        let u = (phase - 0.26) / 0.20;
        (-1.75 * (1.0 - u * u), 1.0, 1.0)
    } else if phase < 0.56 {
        let u = (phase - 0.46) / 0.10;
        let sq = (u * std::f32::consts::PI).sin();
        (0.0, 1.0 + 0.08 * sq, 1.0 - 0.08 * sq)
    } else {
        (0.0, 1.0, 1.0)
    };
    (dy, BASE_PX * sx, BASE_PX * sy)
}

#[inline]
fn rgb_is_dark(hex: u32) -> bool {
    let r = ((hex >> 16) & 0xFF) as f32;
    let g = ((hex >> 8) & 0xFF) as f32;
    let b = (hex & 0xFF) as f32;
    (0.299 * r + 0.587 * g + 0.114 * b) < 128.0
}

fn render_sidebar_felt_badge_inner(
    badge: SidebarFeltBadge,
    ring_rgb: u32,
    dark_scheme: bool,
    animator: Option<&SidebarBadgeAnimator>,
    now_ms: u64,
) -> Div {
    let tf = animator.map(|a| a.sample(now_ms));

    if badge == SidebarFeltBadge::Working {
        const BALL_SPECS: [(&str, u32); 3] = [
            ("openbot_plush/ui/ball-lilac.webp", 0xC6A4E8),
            ("openbot_plush/ui/ball-mint.webp", 0x7AD9B0),
            ("openbot_plush/ui/ball-pink.webp", 0xF29BB8),
        ];
        let pill_bg = if dark_scheme { 0x212121 } else { 0xFFFFFF };
        let mut pill = div()
            .relative()
            .w(px(30.0))
            .h(px(16.0))
            .px(px(3.5))
            .pb(px(3.0))
            .rounded_full()
            .bg(rgb(pill_bg))
            .opacity(tf.map_or(1.0, |t| t.opacity))
            .flex()
            .items_end()
            .justify_center()
            .gap(px(1.0));
        for (idx, (asset_rel, fallback_rgb)) in BALL_SPECS.iter().enumerate() {
            let (dy, bw, bh) = working_ball_hop_sample(now_ms, idx);
            let ball_img = cached_ui_felt_webp(asset_rel);
            let ball_el = div()
                .relative()
                .w(px(bw))
                .h(px(bh))
                .mb(px((-dy).max(0.0)))
                .rounded_full()
                .when(ball_img.is_none(), |el| el.bg(rgb(*fallback_rgb)))
                .when_some(ball_img, |el, img_arc| {
                    el.child(
                        img(img_arc)
                            .w(px(bw))
                            .h(px(bh))
                            .object_fit(ObjectFit::Contain),
                    )
                });
            pill = pill.child(ball_el);
        }
        return pill;
    }

    let box_px = badge.box_px();
    let outer_h = box_px + 2.0 * SIDEBAR_FELT_RING_BORDER_PX;
    let outer_w = match badge {
        SidebarFeltBadge::Unread { count } => tf
            .map(|t| t.width_px)
            .unwrap_or_else(|| SidebarBadgeAnimator::estimate_pill_width(count)),
        _ => outer_h,
    };
    let bg_fill: Background = match badge {
        SidebarFeltBadge::Unread { .. } => linear_gradient(
            180.0,
            linear_color_stop(rgb(0x705083), 0.0),
            linear_color_stop(rgb(0x4A285C), 1.0),
        ),
        SidebarFeltBadge::Waiting { .. } => linear_gradient(
            180.0,
            linear_color_stop(rgb(0xFF9511), 0.0),
            linear_color_stop(rgb(0xCB680C), 1.0),
        ),
        SidebarFeltBadge::Routine {
            phase: SidebarRoutinePhase::Failed,
        } => linear_gradient(
            180.0,
            linear_color_stop(rgb(0xF52070), 0.0),
            linear_color_stop(rgb(0xB40641), 1.0),
        ),
        SidebarFeltBadge::Responded => rgb(0x31CF76).into(),
        SidebarFeltBadge::Limited | SidebarFeltBadge::Routine { .. } | SidebarFeltBadge::Working => {
            rgb(if dark_scheme { 0x2A2A2A } else { 0xE8E8EB }).into()
        }
    };
    let felt_img = badge.felt_asset_path().and_then(cached_ui_felt_webp);
    let suppress_label = matches!(
        badge,
        SidebarFeltBadge::Routine {
            phase: SidebarRoutinePhase::Failed
        }
    ) && felt_img.is_some();
    let label = match badge {
        SidebarFeltBadge::Unread { count } => count.to_string(),
        SidebarFeltBadge::Waiting {
            reason: SidebarWaitReason::Question,
        } => "?".to_string(),
        SidebarFeltBadge::Waiting {
            reason: SidebarWaitReason::Approval,
        } => "!".to_string(),
        SidebarFeltBadge::Waiting {
            reason: SidebarWaitReason::Takeover,
        } => "*".to_string(),
        SidebarFeltBadge::Routine {
            phase: SidebarRoutinePhase::Failed,
        } => "×".to_string(),
        SidebarFeltBadge::Responded => "✓".to_string(),
        SidebarFeltBadge::Working => "…".to_string(),
        SidebarFeltBadge::Limited | SidebarFeltBadge::Routine { .. } => "◷".to_string(),
    };

    div()
        .relative()
        .min_w(px(outer_w))
        .h(px(outer_h))
        .px(px(3.0))
        .rounded_full()
        .bg(bg_fill)
        .border(px(SIDEBAR_FELT_RING_BORDER_PX))
        .border_color(rgb(ring_rgb))
        .opacity(tf.map_or(1.0, |t| t.opacity))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .when_some(felt_img, |el, img_arc| {
            el.child(
                img(img_arc)
                    .absolute()
                    .inset_0()
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Fill),
            )
        })
        .when(!suppress_label, |el| {
            el.child(
                div()
                    .relative()
                    .text_color(rgb(0xF6EFE2))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(label),
            )
        })
}

pub fn render_sidebar_felt_badge(
    badge: SidebarFeltBadge,
    ring_rgb: u32,
    animator: Option<&SidebarBadgeAnimator>,
    now_ms: u64,
) -> Div {
    render_sidebar_felt_badge_inner(badge, ring_rgb, rgb_is_dark(ring_rgb), animator, now_ms)
}

/// Renders an OpenBot felt status badge (`Working3Balls`, `UnreadPlum`, `WaitingAmber`,
/// `RespondedGreen`, `ErrorRaspberry`) with exact `30x16px` working pill & felt gradients.
pub fn render_felt_status_badge(
    state: FeltStatusBadgeState,
    ring_rgb: u32,
    dark_scheme: bool,
    now_ms: u64,
) -> Div {
    render_sidebar_felt_badge_inner(
        state.to_sidebar_badge(),
        ring_rgb,
        dark_scheme,
        None,
        now_ms,
    )
}

pub const CHANNEL_ROW_LIMIT: usize = 3;
pub const CHANNEL_CLUSTER_CELLS: usize = 4;
pub const CHANNEL_ROW_OVERLAP_PX: f32 = -16.0;
pub const CHANNEL_CLUSTER_DEFAULT_SIZE_PX: f32 = 36.0;
pub const CHANNEL_CLUSTER_PINNED_SIZE_PX: f32 = 56.0;
pub const CHANNEL_PICTURE_TRIGGER_SIZE_PX: f32 = 62.0;
pub const DEFAULT_CHANNEL_ICON_HUE: AvatarHue = AvatarHue::Blue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownChannelIcon {
    pub name: ChannelIconName,
    pub hue: AvatarHue,
}

pub fn known_channel_icon(icon: Option<&ChannelIcon>) -> Option<KnownChannelIcon> {
    let icon = icon?;
    let name = ChannelIconName::parse(&icon.name)?;
    let hue = AvatarHue::from_degrees(icon.hue).unwrap_or(DEFAULT_CHANNEL_ICON_HUE);
    Some(KnownChannelIcon { name, hue })
}

fn srgb_u8_to_linear(b: u8) -> f64 {
    let c = f64::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(c: f64) -> u8 {
    let v = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055
    };
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

fn hex_to_oklab(hex: &str) -> [f64; 3] {
    let rgb_u = parse_hex_u32(hex, 0x212121);
    let r = srgb_u8_to_linear(((rgb_u >> 16) & 0xFF) as u8);
    let g = srgb_u8_to_linear(((rgb_u >> 8) & 0xFF) as u8);
    let b = srgb_u8_to_linear((rgb_u & 0xFF) as u8);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
    ]
}

fn oklab_mix_hex(c1: &str, w1: f64, c2: &str) -> String {
    let lab1 = hex_to_oklab(c1);
    let lab2 = hex_to_oklab(c2);
    let p1 = w1.clamp(0.0, 1.0);
    let p2 = 1.0 - p1;
    let l = lab1[0] * p1 + lab2[0] * p2;
    let a = lab1[1] * p1 + lab2[1] * p2;
    let b = lab1[2] * p1 + lab2[2] * p2;
    let l2 = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m2 = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s2 = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    let r = 4.076_741_662_1 * l2 - 3.307_711_591_3 * m2 + 0.230_969_929_2 * s2;
    let g = -1.268_438_004_6 * l2 + 2.609_757_401_1 * m2 - 0.341_319_396_5 * s2;
    let bl = -0.004_196_086_3 * l2 - 0.703_418_614_7 * m2 + 1.707_614_701 * s2;
    format!(
        "#{:02x}{:02x}{:02x}",
        linear_to_srgb_u8(r),
        linear_to_srgb_u8(g),
        linear_to_srgb_u8(bl)
    )
}

/// Computes `ChannelIconTile` `(bg_hex, fg_hex)` via OKLab `26%` bg / `78%` fg (`channel-avatar.css:L166-L167`).
pub fn channel_icon_tile_colors(
    hue: AvatarHue,
    bg_control_hex: &str,
    text_primary_hex: &str,
) -> (String, String) {
    let swatch = avatar_hue_swatch(hue);
    let bg = oklab_mix_hex(swatch, 0.26, bg_control_hex);
    let fg = oklab_mix_hex(swatch, 0.78, text_primary_hex);
    (bg, fg)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelAvatarLayout {
    Row,
    Cluster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelClusterMode {
    CustomImage,
    IconTile,
    MemberFaces,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelFaceCell {
    pub agent_id: String,
    pub grid_row_start: u8,
    pub grid_row_span: u8,
    pub grid_col_start: u8,
    pub grid_col_span: u8,
    pub face_px: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelClusterEval {
    pub mode: ChannelClusterMode,
    pub layout: ChannelAvatarLayout,
    pub active_image_url: Option<String>,
    pub image_failed: bool,
    pub resolved_icon: Option<KnownChannelIcon>,
    pub unique_member_ids: Vec<String>,
    pub shown_member_ids: Vec<String>,
    pub shown_faces: usize,
    pub hidden_count: usize,
    pub plus_pill: Option<String>,
    pub fallback_glyph: Option<&'static str>,
    pub size_px: f32,
    pub pitch_px: f32,
    pub face_px: f32,
    pub custom_face_px: f32,
    pub pill_box_px: f32,
    pub row_overlap_px: f32,
    pub cells: Vec<ChannelFaceCell>,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelAvatarCluster {
    pub failed_url: Option<String>,
}

impl ChannelAvatarCluster {
    pub fn new() -> Self {
        Self { failed_url: None }
    }

    pub fn on_image_error(&mut self, url: &str) {
        self.failed_url = Some(url.trim().to_string());
    }

    pub fn clear_error(&mut self) {
        self.failed_url = None;
    }

    pub fn evaluate(
        &self,
        members: &[ChannelMember],
        layout: ChannelAvatarLayout,
        icon: Option<&ChannelIcon>,
        image_url: Option<&str>,
    ) -> ChannelClusterEval {
        let is_failed = match (image_url.map(str::trim), self.failed_url.as_deref()) {
            (Some(u), Some(f)) if !u.is_empty() && u == f => true,
            _ => false,
        };
        resolve_channel_cluster(members, layout, icon, image_url, is_failed)
    }
}

fn is_broken_local_or_data_url(url: &str) -> bool {
    if url.starts_with("data:") || url.starts_with('/') || url.starts_with("file://") {
        !is_renderable_avatar_url(url)
    } else {
        false
    }
}

pub fn resolve_channel_cluster(
    members: &[ChannelMember],
    layout: ChannelAvatarLayout,
    icon: Option<&ChannelIcon>,
    image_url: Option<&str>,
    image_failed: bool,
) -> ChannelClusterEval {
    let default_size = match layout {
        ChannelAvatarLayout::Row => 32.0,
        ChannelAvatarLayout::Cluster => CHANNEL_CLUSTER_DEFAULT_SIZE_PX,
    };
    resolve_channel_cluster_sized(members, layout, icon, image_url, image_failed, default_size)
}

pub fn resolve_channel_cluster_sized(
    members: &[ChannelMember],
    layout: ChannelAvatarLayout,
    icon: Option<&ChannelIcon>,
    image_url: Option<&str>,
    image_failed: bool,
    size_px: f32,
) -> ChannelClusterEval {
    let size_px = size_px.max(16.0);
    let pitch_px = size_px * 0.38;
    let face_px = size_px * 0.54;
    let custom_face_px = size_px * 0.46;
    let pill_box_px = size_px * 0.46;

    let mut seen = BTreeSet::new();
    let mut unique_member_ids = Vec::with_capacity(members.len());
    for m in members {
        if seen.insert(m.agent_id.clone()) {
            unique_member_ids.push(m.agent_id.clone());
        }
    }

    let trimmed_url = image_url.map(str::trim).filter(|s| !s.is_empty());
    let effective_failed = match trimmed_url {
        Some(u) => image_failed || is_broken_local_or_data_url(u),
        None => image_failed,
    };

    if let Some(u) = trimmed_url
        && !effective_failed
    {
        return ChannelClusterEval {
            mode: ChannelClusterMode::CustomImage,
            layout,
            active_image_url: Some(u.to_string()),
            image_failed: false,
            resolved_icon: known_channel_icon(icon),
            unique_member_ids,
            shown_member_ids: Vec::new(),
            shown_faces: 0,
            hidden_count: 0,
            plus_pill: None,
            fallback_glyph: None,
            size_px,
            pitch_px,
            face_px,
            custom_face_px,
            pill_box_px,
            row_overlap_px: CHANNEL_ROW_OVERLAP_PX,
            cells: Vec::new(),
        };
    }

    if let Some(known) = known_channel_icon(icon) {
        return ChannelClusterEval {
            mode: ChannelClusterMode::IconTile,
            layout,
            active_image_url: None,
            image_failed: effective_failed,
            resolved_icon: Some(known),
            unique_member_ids,
            shown_member_ids: Vec::new(),
            shown_faces: 0,
            hidden_count: 0,
            plus_pill: None,
            fallback_glyph: None,
            size_px,
            pitch_px,
            face_px,
            custom_face_px,
            pill_box_px,
            row_overlap_px: CHANNEL_ROW_OVERLAP_PX,
            cells: Vec::new(),
        };
    }

    let n = unique_member_ids.len();
    if n == 0 {
        return ChannelClusterEval {
            mode: ChannelClusterMode::MemberFaces,
            layout,
            active_image_url: None,
            image_failed: effective_failed,
            resolved_icon: None,
            unique_member_ids,
            shown_member_ids: Vec::new(),
            shown_faces: 0,
            hidden_count: 0,
            plus_pill: None,
            fallback_glyph: Some("UsersRound"),
            size_px,
            pitch_px,
            face_px,
            custom_face_px,
            pill_box_px,
            row_overlap_px: CHANNEL_ROW_OVERLAP_PX,
            cells: Vec::new(),
        };
    }

    match layout {
        ChannelAvatarLayout::Row => {
            let shown_faces = n.min(CHANNEL_ROW_LIMIT);
            let shown_member_ids: Vec<String> = unique_member_ids
                .iter()
                .take(shown_faces)
                .cloned()
                .collect();
            let cells = shown_member_ids
                .iter()
                .enumerate()
                .map(|(idx, id)| ChannelFaceCell {
                    agent_id: id.clone(),
                    grid_row_start: 1,
                    grid_row_span: 1,
                    grid_col_start: (idx as u8) + 1,
                    grid_col_span: 1,
                    face_px: size_px,
                })
                .collect();
            ChannelClusterEval {
                mode: ChannelClusterMode::MemberFaces,
                layout,
                active_image_url: None,
                image_failed: effective_failed,
                resolved_icon: None,
                unique_member_ids,
                shown_member_ids,
                shown_faces,
                hidden_count: n.saturating_sub(shown_faces),
                plus_pill: None,
                fallback_glyph: None,
                size_px,
                pitch_px,
                face_px: size_px,
                custom_face_px,
                pill_box_px,
                row_overlap_px: CHANNEL_ROW_OVERLAP_PX,
                cells,
            }
        }
        ChannelAvatarLayout::Cluster => {
            let shown_faces = if n <= CHANNEL_CLUSTER_CELLS {
                n
            } else {
                let count_with_three = n - (CHANNEL_CLUSTER_CELLS - 1);
                if count_with_three >= 10 {
                    CHANNEL_CLUSTER_CELLS - 2
                } else {
                    CHANNEL_CLUSTER_CELLS - 1
                }
            };
            let hidden_count = n.saturating_sub(shown_faces);
            let plus_pill = (hidden_count > 0).then(|| format!("+{hidden_count}"));
            let shown_member_ids: Vec<String> = unique_member_ids
                .iter()
                .take(shown_faces)
                .cloned()
                .collect();
            let total_slots = shown_faces + usize::from(plus_pill.is_some());
            let cells = build_cluster_cells(&shown_member_ids, total_slots, size_px, face_px);
            ChannelClusterEval {
                mode: ChannelClusterMode::MemberFaces,
                layout,
                active_image_url: None,
                image_failed: effective_failed,
                resolved_icon: None,
                unique_member_ids,
                shown_member_ids,
                shown_faces,
                hidden_count,
                plus_pill,
                fallback_glyph: None,
                size_px,
                pitch_px,
                face_px,
                custom_face_px,
                pill_box_px,
                row_overlap_px: CHANNEL_ROW_OVERLAP_PX,
                cells,
            }
        }
    }
}

fn build_cluster_cells(
    shown_ids: &[String],
    total_slots: usize,
    size_px: f32,
    face_px: f32,
) -> Vec<ChannelFaceCell> {
    shown_ids
        .iter()
        .enumerate()
        .map(|(idx, id)| match total_slots {
            1 => ChannelFaceCell {
                agent_id: id.clone(),
                grid_row_start: 1,
                grid_row_span: 2,
                grid_col_start: 1,
                grid_col_span: 2,
                face_px: size_px,
            },
            2 => ChannelFaceCell {
                agent_id: id.clone(),
                grid_row_start: 1,
                grid_row_span: 2,
                grid_col_start: (idx as u8) + 1,
                grid_col_span: 1,
                face_px,
            },
            3 => {
                if idx == 2 {
                    ChannelFaceCell {
                        agent_id: id.clone(),
                        grid_row_start: 2,
                        grid_row_span: 1,
                        grid_col_start: 1,
                        grid_col_span: 2,
                        face_px,
                    }
                } else {
                    ChannelFaceCell {
                        agent_id: id.clone(),
                        grid_row_start: 1,
                        grid_row_span: 1,
                        grid_col_start: (idx as u8) + 1,
                        grid_col_span: 1,
                        face_px,
                    }
                }
            }
            _ => ChannelFaceCell {
                agent_id: id.clone(),
                grid_row_start: ((idx / 2) as u8) + 1,
                grid_row_span: 1,
                grid_col_start: ((idx % 2) as u8) + 1,
                grid_col_span: 1,
                face_px,
            },
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAgentBrief {
    pub id: String,
    pub avatar_seed: Option<String>,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
}

pub const fn channel_icon_glyph(name: ChannelIconName) -> &'static str {
    match name {
        ChannelIconName::Megaphone => "📣",
        ChannelIconName::Chart => "📈",
        ChannelIconName::Lifebuoy => "🛟",
        ChannelIconName::Search => "🔍",
        ChannelIconName::Briefcase => "💼",
        ChannelIconName::Code => "</>",
        ChannelIconName::Globe => "🌐",
        ChannelIconName::Mail => "✉",
        ChannelIconName::Calendar => "📅",
        ChannelIconName::Gift => "🎁",
        ChannelIconName::Palette => "🎨",
        ChannelIconName::Brain => "🧠",
        ChannelIconName::Book => "📖",
        ChannelIconName::Sparkles => "✨",
        ChannelIconName::Star => "★",
        ChannelIconName::Zap => "⚡",
        ChannelIconName::Shield => "🛡",
        ChannelIconName::Store => "🏪",
        ChannelIconName::Workflow => "⇄",
        ChannelIconName::Card => "💳",
    }
}

pub fn render_channel_icon_tile(
    icon: KnownChannelIcon,
    size_px: f32,
    radius_px: f32,
    dark_scheme: bool,
) -> Div {
    let (bg_ctrl, text_pri) = if dark_scheme {
        ("#212121", "#ffffff")
    } else {
        ("#ffffff", "#141414")
    };
    let (bg_hex, fg_hex) = channel_icon_tile_colors(icon.hue, bg_ctrl, text_pri);
    let bg_rgb = parse_hex_u32(&bg_hex, 0x263244);
    let fg_rgb = parse_hex_u32(&fg_hex, 0x6BB4FF);
    div()
        .w(px(size_px))
        .h(px(size_px))
        .rounded(px(radius_px))
        .bg(rgb(bg_rgb))
        .text_color(rgb(fg_rgb))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size_px * 0.50).clamp(11.0, 28.0)))
        .font_weight(FontWeight::BOLD)
        .child(channel_icon_glyph(icon.name))
}

#[allow(clippy::too_many_arguments)]
pub fn render_channel_avatar(
    members: &[ChannelMember],
    agents: &[ChannelAgentBrief],
    layout: ChannelAvatarLayout,
    icon: Option<&ChannelIcon>,
    image_url: Option<&str>,
    image_failed: bool,
    size_px: f32,
    ring_kind: Option<SidebarPlushRingKind>,
    dark_scheme: bool,
    now_sec: f64,
) -> Div {
    let eval =
        resolve_channel_cluster_sized(members, layout, icon, image_url, image_failed, size_px);
    let tile_radius = match layout {
        ChannelAvatarLayout::Row => 6.0,
        ChannelAvatarLayout::Cluster if eval.size_px >= 50.0 => 20.0,
        ChannelAvatarLayout::Cluster => 12.0,
    };

    match eval.mode {
        ChannelClusterMode::CustomImage => {
            let url = eval.active_image_url.unwrap_or_default();
            let decoded = decode_url_to_render_image(&url);
            return div()
                .w(px(eval.size_px))
                .h(px(eval.size_px))
                .rounded(px(tile_radius))
                .overflow_hidden()
                .bg(rgb(if dark_scheme { 0x212121 } else { 0xFFFFFF }))
                .map(|el| {
                    if let Some(render_img) = decoded {
                        el.child(
                            img(render_img)
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    } else {
                        el.child(img(url).w_full().h_full().object_fit(ObjectFit::Cover))
                    }
                });
        }
        ChannelClusterMode::IconTile => {
            if let Some(known) = eval.resolved_icon {
                return render_channel_icon_tile(known, eval.size_px, tile_radius, dark_scheme);
            }
        }
        ChannelClusterMode::MemberFaces => {}
    }

    if eval.fallback_glyph.is_some() {
        return div()
            .w(px(eval.size_px))
            .h(px(eval.size_px))
            .rounded(px(tile_radius))
            .bg(rgb(if dark_scheme { 0x212121 } else { 0xFFFFFF }))
            .border_1()
            .border_color(rgb(if dark_scheme { 0x262626 } else { 0xE8E8EB }))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(if dark_scheme { 0x979797 } else { 0x6B6B70 }))
            .text_size(px((eval.size_px * 0.48).clamp(11.0, 24.0)))
            .font_weight(FontWeight::SEMIBOLD)
            .child("#");
    }

    let find_agent = |agent_id: &str| agents.iter().find(|a| a.id == agent_id);

    match layout {
        ChannelAvatarLayout::Row => {
            let visual_overlap = -(eval.size_px * 0.44).clamp(6.0, 16.0);
            let mut row = div().flex().items_center().h(px(eval.size_px));
            for (idx, agent_id) in eval.shown_member_ids.iter().enumerate() {
                let brief = find_agent(agent_id);
                let mut p = AgentAvatarProps::new(
                    brief
                        .and_then(|b| b.avatar_seed.clone())
                        .unwrap_or_else(|| agent_id.clone()),
                )
                .with_size(eval.size_px)
                .with_hue_and_color(
                    brief.and_then(|b| b.avatar_hue),
                    brief.and_then(|b| b.avatar_color.clone()),
                )
                .with_url(brief.and_then(|b| b.avatar_url.clone()));
                p.cutout_ring = ring_kind;
                p.dark_scheme = dark_scheme;
                let face_el = render_agent_avatar(&p, None, now_sec);
                row = row.child(
                    div()
                        .when(idx > 0, |el| el.ml(px(visual_overlap)))
                        .child(face_el),
                );
            }
            row
        }
        ChannelAvatarLayout::Cluster => {
            let origin = (eval.size_px - 2.0 * eval.pitch_px) * 0.5;
            let mut container = div()
                .relative()
                .w(px(eval.size_px))
                .h(px(eval.size_px))
                .flex_shrink_0();

            let two_member_diagonal = eval.cells.len() == 2 && eval.plus_pill.is_none();
            for (cell_idx, cell) in eval.cells.iter().enumerate() {
                let brief = find_agent(&cell.agent_id);
                let has_custom_url = brief
                    .and_then(|b| b.avatar_url.as_deref())
                    .is_some_and(is_renderable_avatar_url);
                let effective_face = if eval.cells.len() == 1 && eval.plus_pill.is_none() {
                    eval.size_px
                } else if has_custom_url {
                    eval.custom_face_px
                } else {
                    cell.face_px
                };
                let cell_w = (cell.grid_col_span as f32) * eval.pitch_px;
                let cell_h = (cell.grid_row_span as f32) * eval.pitch_px;
                let cx = origin + ((cell.grid_col_start - 1) as f32) * eval.pitch_px + cell_w * 0.5;
                let diag_dy = if two_member_diagonal {
                    if cell_idx == 0 {
                        -0.08 * eval.size_px
                    } else {
                        0.08 * eval.size_px
                    }
                } else {
                    0.0
                };
                let cy = origin
                    + ((cell.grid_row_start - 1) as f32) * eval.pitch_px
                    + cell_h * 0.5
                    + diag_dy;

                let mut p = AgentAvatarProps::new(
                    brief
                        .and_then(|b| b.avatar_seed.clone())
                        .unwrap_or_else(|| cell.agent_id.clone()),
                )
                .with_size(effective_face)
                .with_hue_and_color(
                    brief.and_then(|b| b.avatar_hue),
                    brief.and_then(|b| b.avatar_color.clone()),
                )
                .with_url(brief.and_then(|b| b.avatar_url.clone()));
                p.cutout_ring = ring_kind;
                p.dark_scheme = dark_scheme;

                container = container.child(
                    div()
                        .absolute()
                        .left(px(cx - effective_face * 0.5))
                        .top(px(cy - effective_face * 0.5))
                        .child(render_agent_avatar(&p, None, now_sec)),
                );
            }

            if let Some(pill_text) = &eval.plus_pill {
                let total_slots = eval.shown_faces + 1;
                let (col_start, col_span) = if total_slots == 3 {
                    (0.0, 2.0)
                } else {
                    (1.0, 1.0)
                };
                let cx = origin + col_start * eval.pitch_px + (col_span * eval.pitch_px) * 0.5;
                let cy = origin + 1.5 * eval.pitch_px + 1.5;
                let ring_px = if eval.size_px <= 40.0 { 2.0 } else { 3.0 };
                let ring_spec = ring_kind.unwrap_or(SidebarPlushRingKind::Default).spec();
                let ring_rgb = if dark_scheme {
                    ring_spec.dark_fallback_rgb
                } else {
                    ring_spec.light_fallback_rgb
                };
                let pill_h = eval.pill_box_px + 2.0 * ring_px;
                let pill_w = (eval.pill_box_px
                    + (pill_text.len().saturating_sub(2) as f32) * 5.5
                    + 2.0 * ring_px)
                    .max(pill_h);
                let use_amber = eval.hidden_count >= 10;
                let pill_bg = if use_amber {
                    linear_gradient(
                        180.0,
                        linear_color_stop(rgb(0xFF9511), 0.0),
                        linear_color_stop(rgb(0xCB680C), 1.0),
                    )
                } else {
                    linear_gradient(
                        180.0,
                        linear_color_stop(rgb(0x705083), 0.0),
                        linear_color_stop(rgb(0x4A285C), 1.0),
                    )
                };
                let felt_tex = cached_ui_felt_webp(if use_amber {
                    "openbot_plush/ui/felt-pill-amber.webp"
                } else {
                    "openbot_plush/ui/felt-pill-lilac.webp"
                });

                container = container.child(
                    div()
                        .absolute()
                        .left(px(cx - pill_w * 0.5))
                        .top(px(cy - pill_h * 0.5))
                        .min_w(px(pill_w))
                        .h(px(pill_h))
                        .px(px(4.0))
                        .rounded_full()
                        .bg(pill_bg)
                        .border(px(ring_px))
                        .border_color(rgb(ring_rgb))
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(felt_tex, |el, img_arc| {
                            el.child(
                                img(img_arc)
                                    .absolute()
                                    .inset_0()
                                    .w_full()
                                    .h_full()
                                    .object_fit(ObjectFit::Fill),
                            )
                        })
                        .child(
                            div()
                                .relative()
                                .text_color(rgb(0xF6EFE2))
                                .text_size(px((eval.pill_box_px * 0.62).clamp(9.0, 14.0)))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(pill_text.clone()),
                        ),
                );
            }

            container
        }
    }
}

pub const CHANNEL_ACTIVITY_OVERLAP_PX: f32 = -8.0;
pub const CHANNEL_ACTIVITY_LEAVING_MARGIN_END_PX: f32 = -24.0;
pub const CHANNEL_ACTIVITY_EXIT_DURATION_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelWorkerBrief {
    pub id: String,
    pub name: String,
    pub avatar_seed: Option<String>,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
}

pub fn channel_activity_sentence(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("{only} is working"),
        [rest @ .., last] => format!("{} and {last} are working", rest.join(", ")),
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChannelActivityIndicatorState {
    pub face_ids: Vec<String>,
    pub leaving_ids: BTreeSet<String>,
    leaving_deadlines_ms: BTreeMap<String, u64>,
    last_seen: BTreeMap<String, ChannelWorkerBrief>,
    active_workers: Vec<ChannelWorkerBrief>,
}

impl ChannelActivityIndicatorState {
    pub fn new(workers: &[ChannelWorkerBrief]) -> Self {
        let mut s = Self::default();
        s.sync_workers(workers, 0, true);
        s
    }

    pub fn sync_workers(
        &mut self,
        workers: &[ChannelWorkerBrief],
        now_ms: u64,
        reduced_motion: bool,
    ) {
        self.tick(now_ms);
        for w in workers {
            self.last_seen.insert(w.id.clone(), w.clone());
        }
        self.active_workers = workers.to_vec();

        let working_set: BTreeSet<&str> = workers.iter().map(|w| w.id.as_str()).collect();
        for id in &working_set {
            self.leaving_ids.remove(*id);
            self.leaving_deadlines_ms.remove(*id);
        }

        let gone: Vec<String> = self
            .face_ids
            .iter()
            .filter(|id| {
                !working_set.contains(id.as_str()) && !self.leaving_ids.contains(id.as_str())
            })
            .cloned()
            .collect();

        for w in workers {
            if !self.face_ids.contains(&w.id) {
                self.face_ids.push(w.id.clone());
            }
        }

        if reduced_motion {
            self.face_ids.retain(|id| working_set.contains(id.as_str()));
            self.leaving_ids.clear();
            self.leaving_deadlines_ms.clear();
            return;
        }

        let deadline = now_ms.saturating_add(CHANNEL_ACTIVITY_EXIT_DURATION_MS);
        for id in gone {
            self.leaving_ids.insert(id.clone());
            self.leaving_deadlines_ms.insert(id, deadline);
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        let expired: Vec<String> = self
            .leaving_deadlines_ms
            .iter()
            .filter_map(|(id, due)| (now_ms >= *due).then(|| id.clone()))
            .collect();
        if expired.is_empty() {
            return;
        }
        for id in &expired {
            self.leaving_ids.remove(id);
            self.leaving_deadlines_ms.remove(id);
        }
        self.face_ids.retain(|id| !expired.contains(id));
    }

    pub fn worker_name_color(&self, worker_id: &str) -> String {
        let brief = self
            .active_workers
            .iter()
            .find(|w| w.id == worker_id)
            .or_else(|| self.last_seen.get(worker_id));
        match brief {
            Some(w) => avatar_head_color(
                w.avatar_seed.as_deref().unwrap_or(&w.id),
                w.avatar_hue,
                w.avatar_color.as_deref(),
            ),
            None => avatar_head_color(worker_id, None, None),
        }
    }

    pub fn render(&self, label_text: &str, dark_scheme: bool, now_sec: f64) -> Div {
        let mut faces_row = div().flex().items_center().flex_shrink_0();
        for (idx, id) in self.face_ids.iter().enumerate() {
            let brief = self
                .active_workers
                .iter()
                .find(|w| w.id == *id)
                .or_else(|| self.last_seen.get(id));
            let is_leaving = self.leaving_ids.contains(id);
            let face_size = if is_leaving { 14.0 } else { 22.0 };
            let mut p = AgentAvatarProps::new(
                brief
                    .and_then(|b| b.avatar_seed.clone())
                    .unwrap_or_else(|| id.clone()),
            )
            .with_size(face_size)
            .with_mood(AvatarMood::Working)
            .with_hue_and_color(
                brief.and_then(|b| b.avatar_hue),
                brief.and_then(|b| b.avatar_color.clone()),
            )
            .with_url(brief.and_then(|b| b.avatar_url.clone()));
            p.dark_scheme = dark_scheme;

            faces_row = faces_row.child(
                div()
                    .when(idx > 0, |el| el.ml(px(CHANNEL_ACTIVITY_OVERLAP_PX)))
                    .when(is_leaving, |el| {
                        el.mr(px(CHANNEL_ACTIVITY_LEAVING_MARGIN_END_PX))
                            .mt(px(4.0))
                            .opacity(0.0)
                    })
                    .child(render_agent_avatar(&p, None, now_sec)),
            );
        }

        let muted_rgb = if dark_scheme { 0x979797 } else { 0x6B6B70 };
        let mut names_row = div()
            .ml(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_size(px(13.0))
            .text_color(rgb(muted_rgb));

        for (idx, w) in self.active_workers.iter().enumerate() {
            if idx > 0 {
                let sep = if idx + 1 == self.active_workers.len() {
                    "and"
                } else {
                    ","
                };
                names_row = names_row.child(sep);
            }
            let head_rgb = parse_hex_u32(&self.worker_name_color(&w.id), muted_rgb);
            names_row = names_row.child(
                div()
                    .text_color(rgb(head_rgb))
                    .font_weight(FontWeight::MEDIUM)
                    .child(w.name.clone()),
            );
        }

        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(faces_row)
            .child(names_row)
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(muted_rgb))
                    .child(label_text.to_string()),
            )
    }
}

pub const CHANNEL_ICON_GRID_COLS: usize = 5;
pub const CHANNEL_ICON_COLOR_GRID_COLS: usize = 8;
pub const CHANNEL_DEFAULT_EDITOR_ICON: ChannelIconName = ChannelIconName::Star;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPictureEditorState {
    pub open: bool,
    pub busy: bool,
    pub icon: Option<ChannelIcon>,
    pub image_url: Option<String>,
}

impl Default for ChannelPictureEditorState {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl ChannelPictureEditorState {
    pub fn new(icon: Option<ChannelIcon>, image_url: Option<String>) -> Self {
        Self {
            open: false,
            busy: false,
            icon,
            image_url: image_url.filter(|s| !s.trim().is_empty()),
        }
    }

    pub fn has_image(&self) -> bool {
        self.image_url.is_some()
    }

    pub fn known_icon(&self) -> Option<KnownChannelIcon> {
        known_channel_icon(self.icon.as_ref())
    }

    pub fn chosen_icon(&self) -> Option<KnownChannelIcon> {
        if self.has_image() {
            None
        } else {
            self.known_icon()
        }
    }

    pub fn active_hue(&self) -> AvatarHue {
        self.known_icon()
            .map_or(DEFAULT_CHANNEL_ICON_HUE, |k| k.hue)
    }

    pub fn can_reset(&self) -> bool {
        self.has_image() || self.known_icon().is_some()
    }

    pub fn icon_names() -> [&'static str; 20] {
        CHANNEL_ICON_NAMES
    }

    pub fn select_icon(&mut self, name: ChannelIconName) -> ChannelIcon {
        let hue = self.active_hue();
        let next = ChannelIcon {
            name: name.as_str().to_string(),
            hue: hue.degrees(),
        };
        self.image_url = None;
        self.icon = Some(next.clone());
        next
    }

    /// Selecting a hue when no icon is selected defaults `name` to `"star"` (`ChannelPictureEditor.tsx:L67`).
    pub fn select_hue(&mut self, next_hue: AvatarHue) -> ChannelIcon {
        let name = self
            .known_icon()
            .map_or(CHANNEL_DEFAULT_EDITOR_ICON, |k| k.name);
        let next = ChannelIcon {
            name: name.as_str().to_string(),
            hue: next_hue.degrees(),
        };
        self.image_url = None;
        self.icon = Some(next.clone());
        next
    }

    pub fn upload_image_bytes(
        &mut self,
        bytes: &[u8],
        mime_hint: Option<&str>,
    ) -> Result<NormalizedAvatarOutput, AvatarNormalizeError> {
        let out = AvatarImageNormalizer::normalize_bytes(bytes, mime_hint)?;
        self.image_url = Some(out.to_data_url());
        Ok(out)
    }

    pub fn remove_image(&mut self) {
        self.image_url = None;
    }

    pub fn reset(&mut self) {
        self.image_url = None;
        self.icon = None;
    }

    pub fn render_popover(&self, panel_width_px: f32, dark_scheme: bool) -> Div {
        let pop_w = (panel_width_px - 30.0).clamp(240.0, EDITOR_MAX_WIDTH_PX);
        let bg_rgb = if dark_scheme { 0x1A1A1A } else { 0xF5F5F6 };
        let border_rgb = if dark_scheme { 0x333335 } else { 0xE3E3E6 };
        let text_sec = if dark_scheme { 0xDCDCDC } else { 0x3A3A3C };
        let chosen = self.chosen_icon();
        let hue = self.active_hue();

        let mut icon_rows = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(10.0))
            .pb(px(12.0));
        for row_chunk in ChannelIconName::ALL.chunks(CHANNEL_ICON_GRID_COLS) {
            let mut row = div().flex().items_center().gap(px(4.0));
            for &icon_name in row_chunk {
                let selected = chosen.is_some_and(|c| c.name == icon_name);
                row = row.child(
                    div()
                        .flex_1()
                        .h(px(44.0))
                        .rounded(px(8.0))
                        .when(selected, |el| el.bg(rgb(border_rgb)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(render_channel_icon_tile(
                            KnownChannelIcon {
                                name: icon_name,
                                hue,
                            },
                            32.0,
                            6.0,
                            dark_scheme,
                        )),
                );
            }
            icon_rows = icon_rows.child(row);
        }

        let mut hue_rows = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(10.0))
            .pb(px(12.0));
        for hue_chunk in AVATAR_HUE_CHOICES.chunks(CHANNEL_ICON_COLOR_GRID_COLS) {
            let mut row = div().flex().items_center().gap(px(2.0));
            for choice in hue_chunk {
                let selected = chosen.is_some_and(|c| c.hue == choice.hue);
                let swatch_rgb = parse_hex_u32(avatar_hue_swatch(choice.hue), 0x3B93F0);
                row = row.child(
                    div()
                        .w(px(30.0))
                        .h(px(32.0))
                        .rounded(px(8.0))
                        .when(selected, |el| el.bg(rgb(border_rgb)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(18.0))
                                .h(px(18.0))
                                .rounded_full()
                                .bg(rgb(swatch_rgb)),
                        ),
                );
            }
            hue_rows = hue_rows.child(row);
        }

        div()
            .w(px(pop_w))
            .rounded(px(EDITOR_RADIUS_PX))
            .bg(rgb(bg_rgb))
            .border_1()
            .border_color(rgb(border_rgb))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.0))
                    .px(px(13.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(13.0))
                    .text_color(rgb(text_sec))
                    .child("ICON")
                    .when(self.can_reset(), |el| el.child("↺")),
            )
            .child(icon_rows)
            .child(div().h(px(1.0)).bg(rgb(border_rgb)))
            .child(
                div()
                    .h(px(40.0))
                    .px(px(13.0))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .text_color(rgb(text_sec))
                    .child("COLOR"),
            )
            .child(hue_rows)
    }
}

pub const EDITOR_MAX_WIDTH_PX: f32 = 284.0;
pub const EDITOR_MIN_PANEL_WIDTH_PX: f32 = 360.0;
pub const EDITOR_GUTTER_PX: f32 = 11.0;
pub const EDITOR_RADIUS_PX: f32 = 12.0;
pub const EDITOR_BACKDROP_BLUR_PX: f32 = 18.0;
pub const EDITOR_UPLOAD_MIN_HEIGHT_PX: f32 = 64.0;
pub const EDITOR_UPLOAD_PREVIEW_PX: f32 = 42.0;
pub const EDITOR_SHAPE_COLS: usize = 5;
pub const EDITOR_SHAPE_CELL_HEIGHT_PX: f32 = 44.0;
pub const EDITOR_SHAPE_AVATAR_PX: f32 = 30.0;
pub const EDITOR_EYES_COLS: usize = 4;
pub const EDITOR_EYES_CELL_HEIGHT_PX: f32 = 46.0;
pub const EDITOR_EYES_AVATAR_PX: f32 = 38.0;
pub const EDITOR_COLOR_COLS: usize = 9;
pub const EDITOR_COLOR_ROWS: usize = 2;
pub const EDITOR_COLOR_TOTAL_CELLS: usize = 18;
pub const EDITOR_COLOR_CELL_HEIGHT_PX: f32 = 32.0;
pub const EDITOR_COLOR_SWATCH_PX: f32 = 18.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarEditorPatch {
    pub avatar_seed: Option<String>,
    pub avatar_hue: Option<Option<AvatarHue>>,
    pub avatar_color: Option<Option<String>>,
    pub cleared_custom_avatar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAvatarEditorState {
    pub agent_id: String,
    pub seed: String,
    pub hue: Option<AvatarHue>,
    pub color: Option<String>,
    pub avatar_url: Option<String>,
    pub picker_open: bool,
    pub upload_busy: bool,
}

impl AgentAvatarEditorState {
    pub fn new(
        agent_id: impl Into<String>,
        seed: Option<String>,
        hue: Option<AvatarHue>,
        color: Option<String>,
        avatar_url: Option<String>,
    ) -> Self {
        let id = agent_id.into();
        let resolved_seed = seed
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| id.clone());
        Self {
            agent_id: id,
            seed: resolved_seed,
            hue,
            color: color.filter(|c| !c.trim().is_empty()),
            avatar_url: avatar_url.filter(|u| !u.trim().is_empty()),
            picker_open: false,
            upload_busy: false,
        }
    }

    pub fn popover_width_for_panel(panel_width_px: f32) -> f32 {
        let clamped_panel = panel_width_px.max(EDITOR_MIN_PANEL_WIDTH_PX);
        (clamped_panel - 30.0).min(EDITOR_MAX_WIDTH_PX)
    }

    pub fn style(&self) -> AvatarStyle {
        avatar_style_for_seed(&self.seed)
    }

    pub fn reset_seed(&self) -> Result<String, AvatarContractError> {
        avatar_default_seed(&self.agent_id, self.style())
    }

    pub fn can_reset_to_id(&self) -> bool {
        self.reset_seed().is_ok_and(|s| s != self.seed)
    }

    pub const fn automatic_color_selected(&self) -> bool {
        self.hue.is_none() && self.color.is_none()
    }

    pub fn is_color_choice_selected(&self, choice: AvatarColorChoice) -> bool {
        is_avatar_color_choice(choice, self.hue, self.color.as_deref())
    }

    pub fn current_face(&self) -> BloubAvatarProfile {
        bloub_avatar_profile(&self.seed, self.hue, self.color.as_deref())
    }

    pub fn select_generated_avatar(
        &mut self,
        seed: String,
        current_color: &str,
    ) -> AvatarEditorPatch {
        let seed_color = bloub_avatar_profile(&seed, None, None).color;
        let mut hue_patch: Option<Option<AvatarHue>> = None;
        if self.automatic_color_selected() && seed_color != current_color {
            if let Some(locked_hue) = avatar_hue_for_color(current_color) {
                hue_patch = Some(Some(locked_hue));
            }
        } else if seed == self.agent_id
            && self.color.is_none()
            && self.hue == avatar_hue_for_color(&seed_color)
        {
            hue_patch = Some(None);
        }

        let cleared_custom_avatar = self.avatar_url.take().is_some();
        self.seed = seed.clone();
        if let Some(next_hue) = hue_patch {
            self.hue = next_hue;
        }
        AvatarEditorPatch {
            avatar_seed: Some(seed),
            avatar_hue: hue_patch,
            avatar_color: None,
            cleared_custom_avatar,
        }
    }

    pub fn select_face(
        &mut self,
        shape: Option<BloubShape>,
        expression: Option<BloubExpression>,
        style: Option<AvatarStyle>,
    ) -> Result<AvatarEditorPatch, AvatarContractError> {
        let current = self.current_face();
        let next_shape = shape.unwrap_or(current.shape);
        let next_expr = expression.unwrap_or(current.expression);
        let next_style = style.unwrap_or_else(|| self.style());
        let own = bloub_avatar_profile(&self.agent_id, None, None);
        let next_seed = if next_style == AvatarStyle::Plush
            && own.shape == next_shape
            && own.expression == next_expr
        {
            self.agent_id.clone()
        } else {
            avatar_seed_for_face(&self.agent_id, next_shape, next_expr, next_style)?
        };
        Ok(self.select_generated_avatar(next_seed, &current.color))
    }

    pub fn set_style(
        &mut self,
        next_style: AvatarStyle,
    ) -> Result<Option<AvatarEditorPatch>, AvatarContractError> {
        if next_style == self.style() {
            return Ok(None);
        }
        self.select_face(None, None, Some(next_style)).map(Some)
    }

    pub fn reset_to_id(&mut self) -> Result<AvatarEditorPatch, AvatarContractError> {
        let reset = self.reset_seed()?;
        let own_color = bloub_avatar_profile(&self.agent_id, None, None).color;
        Ok(self.select_generated_avatar(reset, &own_color))
    }

    pub fn shuffle(
        &mut self,
        rng: &mut Mulberry32,
    ) -> Result<AvatarEditorPatch, AvatarContractError> {
        let current = self.current_face();
        let other_shapes: Vec<BloubShape> = SUPPORTED_AVATAR_SILHOUETTES
            .iter()
            .copied()
            .filter(|&s| s != current.shape)
            .collect();
        let other_exprs: Vec<BloubExpression> = AVATAR_EXPRESSIONS
            .iter()
            .copied()
            .filter(|&e| e != current.expression)
            .collect();
        let s_idx = ((rng.next_f64() * (other_shapes.len() as f64)).floor() as usize)
            .min(other_shapes.len().saturating_sub(1));
        let e_idx = ((rng.next_f64() * (other_exprs.len() as f64)).floor() as usize)
            .min(other_exprs.len().saturating_sub(1));
        self.select_face(
            other_shapes.get(s_idx).copied(),
            other_exprs.get(e_idx).copied(),
            None,
        )
    }
}

impl AgentAvatarEditorState {
    pub fn select_auto_color(&mut self) -> AvatarEditorPatch {
        self.hue = None;
        self.color = None;
        AvatarEditorPatch {
            avatar_seed: None,
            avatar_hue: Some(None),
            avatar_color: Some(None),
            cleared_custom_avatar: false,
        }
    }

    pub fn select_color_choice(&mut self, choice: AvatarColorChoice) -> AvatarEditorPatch {
        match choice {
            AvatarColorChoice::Hue { hue, .. } => {
                self.hue = Some(hue);
                self.color = None;
                AvatarEditorPatch {
                    avatar_seed: None,
                    avatar_hue: Some(Some(hue)),
                    avatar_color: Some(None),
                    cleared_custom_avatar: false,
                }
            }
            AvatarColorChoice::Preset { hue, color, .. } => {
                self.hue = hue;
                self.color = Some(color.to_string());
                AvatarEditorPatch {
                    avatar_seed: None,
                    avatar_hue: Some(hue),
                    avatar_color: Some(Some(color.to_string())),
                    cleared_custom_avatar: false,
                }
            }
        }
    }

    pub fn upload_image_bytes(
        &mut self,
        bytes: &[u8],
        mime_hint: Option<&str>,
    ) -> Result<NormalizedAvatarOutput, AvatarNormalizeError> {
        let normalized = AvatarImageNormalizer::normalize_bytes(bytes, mime_hint)?;
        self.avatar_url = Some(normalized.to_data_url());
        Ok(normalized)
    }

    pub fn remove_custom_image(&mut self) {
        self.avatar_url = None;
    }
}

pub fn render_avatar_editor_popover(
    state: &AgentAvatarEditorState,
    panel_width_px: f32,
    dark_scheme: bool,
    now_sec: f64,
) -> Div {
    let pop_w = AgentAvatarEditorState::popover_width_for_panel(panel_width_px);
    let bg_overlay = if dark_scheme { 0x1A1A1AEB } else { 0xF5F5F6EB };
    let border_rgb = if dark_scheme { 0x333335 } else { 0xE3E3E6 };
    let hover_rgb = if dark_scheme { 0x262628 } else { 0xECECEE };
    let text_pri = if dark_scheme { 0xFFFFFF } else { 0x141414 };
    let text_sec = if dark_scheme { 0xDCDCDC } else { 0x3A3A3C };
    let text_muted = if dark_scheme { 0x979797 } else { 0x6B6B70 };

    let has_custom = state.avatar_url.is_some();
    let face = state.current_face();
    let style = state.style();

    let preview_node = if has_custom {
        let p = AgentAvatarProps::new(&state.seed)
            .with_size(EDITOR_UPLOAD_PREVIEW_PX)
            .with_hue_and_color(state.hue, state.color.clone())
            .with_url(state.avatar_url.clone());
        render_agent_avatar(&p, None, now_sec)
    } else {
        div()
            .w(px(EDITOR_UPLOAD_PREVIEW_PX))
            .h(px(EDITOR_UPLOAD_PREVIEW_PX))
            .rounded_full()
            .bg(rgb(hover_rgb))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(text_muted))
            .text_size(px(18.0))
            .child("+")
    };

    let mut shape_grid = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .px(px(10.0))
        .pb(px(12.0));
    for row_shapes in SUPPORTED_AVATAR_SILHOUETTES.chunks(EDITOR_SHAPE_COLS) {
        let mut row = div().flex().items_center().gap(px(4.0));
        for &shape_opt in row_shapes {
            let selected = !has_custom && face.shape == shape_opt;
            let mut p = AgentAvatarProps::new(&state.seed)
                .with_size(EDITOR_SHAPE_AVATAR_PX)
                .with_hue_and_color(state.hue, state.color.clone());
            p.shape_override = Some(shape_opt);
            p.transition = PlushTransition::Cut;
            p.dark_scheme = dark_scheme;

            row = row.child(
                div()
                    .flex_1()
                    .h(px(EDITOR_SHAPE_CELL_HEIGHT_PX))
                    .rounded(px(8.0))
                    .when(selected, |el| el.bg(rgb(border_rgb)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(render_agent_avatar(&p, None, now_sec)),
            );
        }
        shape_grid = shape_grid.child(row);
    }

    let mut eyes_grid = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .px(px(10.0))
        .pb(px(12.0));
    for row_exprs in AVATAR_EXPRESSIONS.chunks(EDITOR_EYES_COLS) {
        let mut row = div().flex().items_center().gap(px(4.0));
        for &expr_opt in row_exprs {
            let selected = !has_custom && face.expression == expr_opt;
            let mut p = AgentAvatarProps::new(&state.seed)
                .with_size(EDITOR_EYES_AVATAR_PX)
                .with_hue_and_color(state.hue, state.color.clone());
            p.expression_override = Some(expr_opt);
            p.transition = PlushTransition::Cut;
            p.dark_scheme = dark_scheme;

            row = row.child(
                div()
                    .flex_1()
                    .h(px(EDITOR_EYES_CELL_HEIGHT_PX))
                    .rounded(px(8.0))
                    .when(selected, |el| el.bg(rgb(border_rgb)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(render_agent_avatar(&p, None, now_sec)),
            );
        }
        eyes_grid = eyes_grid.child(row);
    }

    let color_grid = render_editor_color_grid(state, border_rgb, text_sec, text_muted);

    div()
        .w(px(pop_w))
        .rounded(px(EDITOR_RADIUS_PX))
        .bg(rgba(bg_overlay))
        .border_1()
        .border_color(rgb(border_rgb))
        .flex()
        .flex_col()
        .child(
            div()
                .min_h(px(40.0))
                .px(px(13.0))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(13.0))
                .text_color(rgb(text_sec))
                .child("IMAGE")
                .when(has_custom, |el| el.child("Remove")),
        )
        .child(
            div()
                .mx(px(10.0))
                .mb(px(12.0))
                .p(px(9.0))
                .min_h(px(EDITOR_UPLOAD_MIN_HEIGHT_PX))
                .rounded(px(EDITOR_RADIUS_PX))
                .bg(rgb(hover_rgb))
                .border_1()
                .border_color(rgb(border_rgb))
                .flex()
                .items_center()
                .gap(px(11.0))
                .child(preview_node)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(text_pri))
                                .child(if has_custom {
                                    "Replace image"
                                } else {
                                    "Upload image"
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(text_muted))
                                .child("PNG, JPEG, GIF, or WebP"),
                        ),
                ),
        )
        .child(div().h(px(1.0)).bg(rgb(border_rgb)))
        .child(render_shape_header(
            state, style, hover_rgb, text_pri, text_sec,
        ))
        .child(shape_grid)
        .child(div().h(px(1.0)).bg(rgb(border_rgb)))
        .child(
            div()
                .min_h(px(40.0))
                .px(px(13.0))
                .flex()
                .items_center()
                .text_size(px(13.0))
                .text_color(rgb(text_sec))
                .child("EYES"),
        )
        .child(eyes_grid)
        .child(div().h(px(1.0)).bg(rgb(border_rgb)))
        .child(
            div()
                .min_h(px(40.0))
                .px(px(13.0))
                .flex()
                .items_center()
                .text_size(px(13.0))
                .text_color(rgb(text_sec))
                .child("COLOR"),
        )
        .child(color_grid)
}

fn render_shape_header(
    state: &AgentAvatarEditorState,
    style: AvatarStyle,
    hover_rgb: u32,
    text_pri: u32,
    text_sec: u32,
) -> Div {
    let tabs = div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(px(8.0))
        .bg(rgb(hover_rgb))
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .when(style == AvatarStyle::Plush, |el| {
                    el.bg(rgb(0x3A3A3D)).text_color(rgb(text_pri))
                })
                .text_size(px(12.0))
                .child("Plush"),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .when(style == AvatarStyle::Flat, |el| {
                    el.bg(rgb(0x3A3A3D)).text_color(rgb(text_pri))
                })
                .text_size(px(12.0))
                .child("Flat"),
        );

    div()
        .min_h(px(40.0))
        .px(px(13.0))
        .flex()
        .items_center()
        .justify_between()
        .text_size(px(13.0))
        .text_color(rgb(text_sec))
        .child("SHAPE")
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tabs)
                .when(state.can_reset_to_id(), |el| el.child("↺"))
                .child("⇄"),
        )
}

fn render_editor_color_grid(
    state: &AgentAvatarEditorState,
    border_rgb: u32,
    text_sec: u32,
    text_muted: u32,
) -> Div {
    let mut cells: Vec<Div> = Vec::with_capacity(EDITOR_COLOR_TOTAL_CELLS);
    let auto_selected = state.automatic_color_selected();
    cells.push(
        div()
            .flex_1()
            .h(px(EDITOR_COLOR_CELL_HEIGHT_PX))
            .rounded(px(8.0))
            .when(auto_selected, |el| el.bg(rgb(border_rgb)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(EDITOR_COLOR_SWATCH_PX))
                    .h(px(EDITOR_COLOR_SWATCH_PX))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(text_muted))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(text_sec))
                    .child("A"),
            ),
    );

    for choice in AVATAR_COLOR_CHOICES {
        let selected = state.is_color_choice_selected(choice);
        let swatch_hex = match choice {
            AvatarColorChoice::Hue { swatch, .. } | AvatarColorChoice::Preset { swatch, .. } => {
                swatch
            }
        };
        let swatch_rgb = parse_hex_u32(swatch_hex, 0xE8483F);
        cells.push(
            div()
                .flex_1()
                .h(px(EDITOR_COLOR_CELL_HEIGHT_PX))
                .rounded(px(8.0))
                .when(selected, |el| el.bg(rgb(border_rgb)))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(EDITOR_COLOR_SWATCH_PX))
                        .h(px(EDITOR_COLOR_SWATCH_PX))
                        .rounded_full()
                        .bg(rgb(swatch_rgb)),
                ),
        );
    }

    let mut grid = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(10.0))
        .pb(px(12.0));
    let mut iter = cells.into_iter();
    for _ in 0..EDITOR_COLOR_ROWS {
        let mut row = div().flex().items_center().gap(px(2.0));
        for _ in 0..EDITOR_COLOR_COLS {
            if let Some(cell) = iter.next() {
                row = row.child(cell);
            }
        }
        grid = grid.child(row);
    }
    grid
}
