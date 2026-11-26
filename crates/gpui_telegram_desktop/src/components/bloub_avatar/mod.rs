//! OpenBot Bloub & Plush Bot Avatar Engine, Clusters, Editor, Image Normalizer, and `CrewPile` (`OB-F03`, `OB-F04`).
//!
//! Unifies:
//! - `silhouettes`: 64-ray radial sampling, 14 `EngineShapeId`s, 64-segment closed Catmull-Rom cubic Bezier paths
//! - `eyefit`: 3D spherical eye projection (`depth <= 0.02` horizon cull, `depth / 0.12` fade), 16 `EXPRESSIONS`, `decalage`
//! - `bot_engine`: `Mulberry32`, 15 `BloubParametricState`s, `departFige` mid-morph continuity, pointer gaze, SVG/GPUI canvas
//! - `plush_felt`: 73 `.webp` assets, sRGB-linear `tint_ramp`, 9-slice `plus-lighter` eye pads, 4-band `lid_mask`, 5 transitions
//! - `mascots_and_logo`: `ServerPlushLogo`, `PlushMascot`, `PlushLoader`, `GlobalSearchPlushState`, `MarketplaceNoMatchPlushState`, `AppLogoState`
//! - `clusters_and_editor`: `AgentAvatarController`, `SidebarFeltBadge`, `ChannelAvatarCluster`, `284px` `AgentAvatarEditorState`
//! - `image_normalizer`: `<= 10 MiB` -> square `<= 512 KiB` static & animated RIFF `WEBP` (`VP8X + ANIM + ANMF`)
//! - `crew_pile`: `240Hz` 4-iteration soft-body `CrewPile` (`CrewPilePhysics`) & GPUI `render_crew_pile_stage`

pub mod bot_engine;
pub mod clusters_and_editor;
pub mod crew_pile;
pub mod eyefit;
pub mod image_normalizer;
pub mod mascots_and_logo;
pub mod plush_felt;
pub mod silhouettes;

pub use bot_engine::*;
pub use clusters_and_editor::{
    ASLEEP_CUSTOM_IMAGE_OPACITY, AVATAR_BREATHE_PERIOD_SEC, AVATAR_FPS, AVATAR_OPTICAL_BOX_RATIO,
    AVATAR_OPTICAL_OFFSET_RATIO, AgentAvatarController, AgentAvatarEditorState, AgentAvatarProps,
    AgentAvatarResolvedState, AvatarEditorPatch, AvatarMotionMode, BACKGROUND_AVATAR_FPS,
    CHANNEL_ACTIVITY_EXIT_DURATION_MS, CHANNEL_ACTIVITY_LEAVING_MARGIN_END_PX,
    CHANNEL_ACTIVITY_OVERLAP_PX, CHANNEL_CLUSTER_CELLS, CHANNEL_CLUSTER_DEFAULT_SIZE_PX,
    CHANNEL_CLUSTER_PINNED_SIZE_PX, CHANNEL_DEFAULT_EDITOR_ICON, CHANNEL_ICON_COLOR_GRID_COLS,
    CHANNEL_ICON_GRID_COLS, CHANNEL_PICTURE_TRIGGER_SIZE_PX, CHANNEL_ROW_LIMIT,
    CHANNEL_ROW_OVERLAP_PX, CONVERSATION_TITLE_AVATAR_SIZE, CONVERSATION_TITLE_OPTICAL_OFFSET_PX,
    ChannelActivityIndicatorState, ChannelAgentBrief, ChannelAvatarCluster, ChannelAvatarLayout,
    ChannelClusterEval, ChannelClusterMode, ChannelFaceCell, ChannelPictureEditorState,
    ChannelWorkerBrief, DEFAULT_CHANNEL_ICON_HUE, EDITOR_BACKDROP_BLUR_PX,
    EDITOR_COLOR_CELL_HEIGHT_PX, EDITOR_COLOR_COLS, EDITOR_COLOR_ROWS, EDITOR_COLOR_SWATCH_PX,
    EDITOR_COLOR_TOTAL_CELLS, EDITOR_EYES_AVATAR_PX, EDITOR_EYES_CELL_HEIGHT_PX, EDITOR_EYES_COLS,
    EDITOR_GUTTER_PX, EDITOR_MAX_WIDTH_PX, EDITOR_MIN_PANEL_WIDTH_PX, EDITOR_RADIUS_PX,
    EDITOR_SHAPE_AVATAR_PX, EDITOR_SHAPE_CELL_HEIGHT_PX, EDITOR_SHAPE_COLS,
    EDITOR_UPLOAD_MIN_HEIGHT_PX, EDITOR_UPLOAD_PREVIEW_PX, KnownChannelIcon,
    PLUSH_BODY_SCALE as PLUSH_BODY_SCALE_F32, SIDEBAR_BADGE_BUMP_MS, SIDEBAR_BADGE_RESIZE_MS,
    SIDEBAR_BADGE_ROLL_IN_MS, SIDEBAR_BADGE_ROLL_OFFSET_RATIO, SIDEBAR_BADGE_ROLL_OUT_MS,
    SIDEBAR_FELT_DEFAULT_BOX_PX, SIDEBAR_FELT_RING_BORDER_PX, SIDEBAR_FELT_RING_WIDTH_PX,
    SIDEBAR_FELT_WAITING_BOX_PX, SIDEBAR_WAIT_TOOLTIP_OPEN_DELAY_MS, SidebarBadgeAnimator,
    SidebarBadgePhase, SidebarBadgeTransformSample, SidebarFeltBadge, SidebarPlushRingKind,
    SidebarPlushRingSpec, SidebarRoutinePhase, SidebarWaitReason, WORKING_FACE_NOMINAL_MS,
    channel_activity_sentence, channel_icon_glyph, channel_icon_tile_colors, known_channel_icon,
    render_agent_avatar, render_agent_flat_svg, render_avatar_editor_popover,
    render_channel_avatar, render_channel_icon_tile, render_sidebar_felt_badge,
    resolve_channel_cluster, resolve_channel_cluster_sized, sidebar_plush_ring_svg_defs,
};
pub use crew_pile::*;
pub use eyefit::*;
pub use image_normalizer::*;
pub use mascots_and_logo::*;
pub use plush_felt::*;
pub use silhouettes::*;

/// Public type alias matching `SCOPE.md` / `PROJECT.md` `CrewPilePhysics`.
pub type CrewPilePhysics = CrewPile;

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use openbot_contracts::avatar::{
    AvatarHue, AvatarMood, AvatarStyle, BloubExpression, BloubParametricState, BloubShape,
    EngineShapeId, PlushState, avatar_mood_presentation, avatar_style_for_seed,
    bloub_avatar_profile, is_avatar_color, nearest_avatar_hue, plush_mood_presentation,
    working_cycle_for_seed,
};
use openbot_contracts::channels::ChannelMember;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarEvalError {
    InvalidHue(u16),
    InvalidColor(String),
    InvalidStyle(String),
    UnsupportedShape(String),
    InvalidMood(String),
    InvalidState(String),
    InvalidExpression(String),
    InvalidPointer(String),
    InvalidPointerType(String),
    ImageNormalize(AvatarNormalizeError),
}

impl AvatarEvalError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ImageNormalize(AvatarNormalizeError::Io { .. }) => "execution_error",
            _ => "validation_error",
        }
    }

    pub const fn is_validation_error(&self) -> bool {
        !matches!(self, Self::ImageNormalize(AvatarNormalizeError::Io { .. }))
    }
}

impl fmt::Display for AvatarEvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHue(h) => write!(
                f,
                "unsupported avatar hue {h}: expected one of the 10 supported palette hues (0, 30, 55, 100, 150, 185, 215, 245, 280, 320)"
            ),
            Self::InvalidColor(c) => write!(
                f,
                "invalid avatar color '{c}': expected 6-digit #rrggbb lowercase/hex format"
            ),
            Self::InvalidStyle(s) => {
                write!(f, "invalid avatar style '{s}': expected 'plush' or 'flat'")
            }
            Self::UnsupportedShape(sh) => write!(
                f,
                "unsupported avatar shape '{sh}': expected one of the 13 supported silhouettes ('goutte' is excluded)"
            ),
            Self::InvalidMood(m) => write!(
                f,
                "invalid avatar mood '{m}': expected idle|working|waiting|failed|responded|connecting|asleep"
            ),
            Self::InvalidState(st) => write!(
                f,
                "invalid avatar state '{st}': expected one of the 15 Bloub states or 7 Plush states"
            ),
            Self::InvalidExpression(ex) => write!(
                f,
                "invalid avatar expression '{ex}': expected one of the 16 supported expressions"
            ),
            Self::InvalidPointer(p) => write!(
                f,
                "invalid pointer coordinates '{p}': expected finite 'nx,ny' floats"
            ),
            Self::InvalidPointerType(pt) => write!(
                f,
                "invalid pointer type '{pt}': expected 'mouse', 'pen', or 'touch'"
            ),
            Self::ImageNormalize(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AvatarEvalError {}

impl From<AvatarNormalizeError> for AvatarEvalError {
    fn from(value: AvatarNormalizeError) -> Self {
        Self::ImageNormalize(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AvatarEvalRequest {
    pub seed: String,
    pub hue: Option<u16>,
    pub color: Option<String>,
    pub style: Option<String>,
    pub shape: Option<String>,
    pub mood: Option<String>,
    pub state: Option<String>,
    pub expression: Option<String>,
    pub t: Option<f64>,
    pub pointer: Option<(f64, f64)>,
    pub pointer_type: Option<String>,
    pub reduced_motion: bool,
    pub working_faces: bool,
    pub cluster_count: Option<usize>,
    pub normalize_image: Option<PathBuf>,
    pub normalize_output_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarEvalProfileReport {
    pub style: String,
    pub shape: String,
    pub expression: String,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarEyeSideReport {
    pub w: f64,
    pub h: f64,
    pub alpha: f64,
    pub culled: bool,
    pub cutout_op: String,
    pub pad_blend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarEyesEvalReport {
    pub rest_open: f64,
    pub open_squash: f64,
    pub zero_clipping: bool,
    pub wander_active: bool,
    pub gaze: GazeAngles,
    pub left: AvatarEyeSideReport,
    pub right: AvatarEyeSideReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarOrnamentsEvalReport {
    pub busy_dots: usize,
    pub notify_color: Option<String>,
    pub orbit_arcs: usize,
    pub sleep_zs: usize,
    pub shake_deg: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarClusterEvalReport {
    pub member_count: usize,
    pub shown_faces: usize,
    pub hidden_count: usize,
    pub plus_pill: Option<String>,
    pub fallback_glyph: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarNormalizedImageEvalReport {
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: usize,
    pub magic: String,
    pub animated: bool,
    pub frame_count: usize,
    pub output_path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarEvalReport {
    pub seed: String,
    pub profile: AvatarEvalProfileReport,
    pub nearest_hue: Option<u16>,
    pub mood: String,
    pub state: String,
    pub view_box: String,
    pub paper: String,
    pub rays_count: usize,
    pub cubic_segments: usize,
    pub body_path: String,
    pub breath_sy: f64,
    pub eyes: AvatarEyesEvalReport,
    pub ornaments: AvatarOrnamentsEvalReport,
    pub tint_ramp: BTreeMap<String, [u8; 4]>,
    pub cluster: AvatarClusterEvalReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_image: Option<AvatarNormalizedImageEvalReport>,
}

pub fn parse_pointer_pair(raw: &str) -> Result<(f64, f64), AvatarEvalError> {
    let Some((lx, ly)) = raw.trim().split_once(',') else {
        return Err(AvatarEvalError::InvalidPointer(raw.to_string()));
    };
    let nx: f64 = lx
        .trim()
        .parse()
        .map_err(|_| AvatarEvalError::InvalidPointer(raw.to_string()))?;
    let ny: f64 = ly
        .trim()
        .parse()
        .map_err(|_| AvatarEvalError::InvalidPointer(raw.to_string()))?;
    if !nx.is_finite() || !ny.is_finite() {
        return Err(AvatarEvalError::InvalidPointer(raw.to_string()));
    }
    Ok((nx, ny))
}

pub fn parse_pointer_kind(raw: &str) -> Result<PointerKind, AvatarEvalError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "mouse" => Ok(PointerKind::Mouse),
        "pen" => Ok(PointerKind::Pen),
        "touch" => Ok(PointerKind::Touch),
        _ => Err(AvatarEvalError::InvalidPointerType(raw.to_string())),
    }
}

fn parse_parametric_state(raw: &str) -> Option<BloubParametricState> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "idle" => Some(BloubParametricState::Idle),
        "thinking" => Some(BloubParametricState::Thinking),
        "wink" => Some(BloubParametricState::Wink),
        "wide" => Some(BloubParametricState::Wide),
        "alert" => Some(BloubParametricState::Alert),
        "notify" => Some(BloubParametricState::Notify),
        "exclaim" => Some(BloubParametricState::Exclaim),
        "sleep" => Some(BloubParametricState::Sleep),
        "egg" => Some(BloubParametricState::Egg),
        "hexagon" => Some(BloubParametricState::Hexagon),
        "play" => Some(BloubParametricState::Play),
        "orbit" => Some(BloubParametricState::Orbit),
        "swirl" => Some(BloubParametricState::Swirl),
        "burst" => Some(BloubParametricState::Burst),
        "comet" => Some(BloubParametricState::Comet),
        _ => None,
    }
}

const fn parametric_state_str(st: BloubParametricState) -> &'static str {
    match st {
        BloubParametricState::Idle => "idle",
        BloubParametricState::Thinking => "thinking",
        BloubParametricState::Wink => "wink",
        BloubParametricState::Wide => "wide",
        BloubParametricState::Alert => "alert",
        BloubParametricState::Notify => "notify",
        BloubParametricState::Exclaim => "exclaim",
        BloubParametricState::Sleep => "sleep",
        BloubParametricState::Egg => "egg",
        BloubParametricState::Hexagon => "hexagon",
        BloubParametricState::Play => "play",
        BloubParametricState::Orbit => "orbit",
        BloubParametricState::Swirl => "swirl",
        BloubParametricState::Burst => "burst",
        BloubParametricState::Comet => "comet",
    }
}

fn parse_plush_state(raw: &str) -> Option<PlushState> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "idle" => Some(PlushState::Idle),
        "wink" => Some(PlushState::Wink),
        "wide" => Some(PlushState::Wide),
        "thinking" => Some(PlushState::Thinking),
        "notify" => Some(PlushState::Notify),
        "asleep" | "sleep" => Some(PlushState::Asleep),
        "failed" => Some(PlushState::Failed),
        _ => None,
    }
}

const fn plush_state_str(st: PlushState) -> &'static str {
    match st {
        PlushState::Idle => "idle",
        PlushState::Wink => "wink",
        PlushState::Wide => "wide",
        PlushState::Thinking => "thinking",
        PlushState::Notify => "notify",
        PlushState::Asleep => "asleep",
        PlushState::Failed => "failed",
    }
}

/// Evaluates a complete OpenBot Bloub/Plush avatar specification (`TEST_INFRA.md:L1839-L1867`).
pub fn evaluate_avatar_spec(req: &AvatarEvalRequest) -> Result<AvatarEvalReport, AvatarEvalError> {
    let resolved_hue = match req.hue {
        Some(deg) => Some(AvatarHue::from_degrees(deg).ok_or(AvatarEvalError::InvalidHue(deg))?),
        None => None,
    };

    let explicit_color = match req.color.as_deref() {
        Some(raw) => {
            let clean = raw.trim().to_ascii_lowercase();
            if !is_avatar_color(&clean) {
                return Err(AvatarEvalError::InvalidColor(raw.to_string()));
            }
            Some(clean)
        }
        None => None,
    };

    let resolved_style = match req.style.as_deref() {
        Some(raw) => AvatarStyle::parse(raw.trim().to_ascii_lowercase().as_str())
            .ok_or_else(|| AvatarEvalError::InvalidStyle(raw.to_string()))?,
        None => avatar_style_for_seed(&req.seed),
    };

    let base_profile =
        bloub_avatar_profile(&req.seed, resolved_hue, explicit_color.as_deref());

    let effective_shape = match req.shape.as_deref() {
        Some(raw) => BloubShape::parse(raw.trim().to_ascii_lowercase().as_str())
            .ok_or_else(|| AvatarEvalError::UnsupportedShape(raw.to_string()))?,
        None => base_profile.shape,
    };

    let resolved_mood = match req.mood.as_deref() {
        Some(raw) => AvatarMood::parse(raw.trim().to_ascii_lowercase().as_str())
            .ok_or_else(|| AvatarEvalError::InvalidMood(raw.to_string()))?,
        None => AvatarMood::Idle,
    };

    let explicit_expr = match req.expression.as_deref() {
        Some(raw) => Some(
            BloubExpression::parse(raw.trim().to_ascii_lowercase().as_str())
                .ok_or_else(|| AvatarEvalError::InvalidExpression(raw.to_string()))?,
        ),
        None => None,
    };

    let t_sec = req.t.unwrap_or(0.0);
    if !t_sec.is_finite() {
        return Err(AvatarEvalError::InvalidPointer(format!("t={t_sec}")));
    }

    let pointer_kind = match req.pointer_type.as_deref() {
        Some(raw) => parse_pointer_kind(raw)?,
        None => PointerKind::Mouse,
    };
    if let Some((nx, ny)) = req.pointer {
        if !nx.is_finite() || !ny.is_finite() {
            return Err(AvatarEvalError::InvalidPointer(format!("{nx},{ny}")));
        }
    }

    let effective_expr = if let Some(ex) = explicit_expr {
        ex
    } else if req.working_faces && matches!(resolved_mood, AvatarMood::Working | AvatarMood::Connecting) {
        let tick = ((t_sec.max(0.0) * 1000.0) / (WORKING_FACE_NOMINAL_MS as f64)).floor() as usize;
        working_cycle_for_seed(&req.seed, tick)
    } else {
        match resolved_style {
            AvatarStyle::Plush => plush_mood_presentation(resolved_mood)
                .expression
                .unwrap_or(base_profile.expression),
            AvatarStyle::Flat => avatar_mood_presentation(resolved_mood)
                .expression
                .unwrap_or(base_profile.expression),
        }
    };

    let effective_color = base_profile.color.to_ascii_lowercase();
    let nearest_hue = nearest_avatar_hue(&effective_color).map(AvatarHue::degrees);

    let (state_str, body_path, breath_sy, eyes_report, ornaments_report) = match resolved_style {
        AvatarStyle::Flat => {
            let param_state = match req.state.as_deref() {
                Some(raw) => parse_parametric_state(raw)
                    .ok_or_else(|| AvatarEvalError::InvalidState(raw.to_string()))?,
                None => avatar_mood_presentation(resolved_mood).state.to_parametric(),
            };
            let mut engine = BloubBotEngine::new(
                bloub_to_engine_shape(effective_shape),
                effective_expr,
                param_state,
            );
            if let Some((nx, ny)) = req.pointer {
                if pointer_kind == PointerKind::Touch {
                    engine.set_pointer(PointerKind::Touch, nx, ny, true, t_sec.max(0.0));
                } else {
                    let end_t = t_sec.max(0.0);
                    let steps = ((end_t / 0.02).ceil() as usize).clamp(1, 256);
                    for i in 0..=steps {
                        let step_t = if end_t == 0.0 {
                            0.0
                        } else {
                            end_t * (i as f64) / (steps as f64)
                        };
                        engine.set_pointer(pointer_kind, nx, ny, true, step_t);
                    }
                }
            }
            let sample = engine.sample(t_sec, req.reduced_motion);
            let left_tr = sample.eyes[0];
            let right_tr = sample.eyes[1];
            let eyes = AvatarEyesEvalReport {
                rest_open: left_tr.open,
                open_squash: left_tr.open_squash,
                zero_clipping: sample.zero_clipping,
                wander_active: sample.wander_active,
                gaze: sample.gaze,
                left: AvatarEyeSideReport {
                    w: left_tr.w / RAYON,
                    h: left_tr.h / RAYON,
                    alpha: left_tr.alpha,
                    culled: left_tr.culled,
                    cutout_op: "DstOut".to_string(),
                    pad_blend: PAD_BLEND_MODE.to_string(),
                },
                right: AvatarEyeSideReport {
                    w: right_tr.w / RAYON,
                    h: right_tr.h / RAYON,
                    alpha: right_tr.alpha,
                    culled: right_tr.culled,
                    cutout_op: "DstOut".to_string(),
                    pad_blend: PAD_BLEND_MODE.to_string(),
                },
            };
            let ornaments = AvatarOrnamentsEvalReport {
                busy_dots: if param_state == BloubParametricState::Thinking {
                    sample.dots.len() + usize::from(sample.body_alpha > 0.001)
                } else {
                    0
                },
                notify_color: sample.notif.map(|_| NOTIF_BLUE.to_string()),
                orbit_arcs: sample.arcs.len(),
                sleep_zs: if param_state == BloubParametricState::Sleep {
                    sample.dots.len()
                } else {
                    0
                },
                shake_deg: 0.0,
            };
            (
                parametric_state_str(param_state).to_string(),
                sample.body_path,
                sample.breath_sy,
                eyes,
                ornaments,
            )
        }
        AvatarStyle::Plush => {
            let plush_state = match req.state.as_deref() {
                Some(raw) => parse_plush_state(raw)
                    .ok_or_else(|| AvatarEvalError::InvalidState(raw.to_string()))?,
                None => plush_mood_presentation(resolved_mood).state,
            };
            let mut renderer = if plush_state == PlushState::Failed && !req.reduced_motion {
                let mut r = PlushFeltRenderer::new(
                    bloub_shape_to_engine(effective_shape),
                    &effective_color,
                    effective_expr,
                    PlushState::Idle,
                );
                r.set_state(PlushState::Failed, 0.0);
                r
            } else {
                PlushFeltRenderer::new(
                    bloub_shape_to_engine(effective_shape),
                    &effective_color,
                    effective_expr,
                    plush_state,
                )
            };
            let pointer_override = req.pointer.filter(|_| pointer_kind != PointerKind::Touch);
            renderer.set_pointer_gaze(pointer_override);
            let sample = renderer.sample_plush_frame(t_sec, req.reduced_motion);
            let left_tr = sample.eyes[0].transform;
            let right_tr = sample.eyes[1].transform;
            let eyes = AvatarEyesEvalReport {
                rest_open: left_tr.open,
                open_squash: left_tr.open_squash,
                zero_clipping: sample.zero_clipping,
                wander_active: !req.reduced_motion && pointer_override.is_none(),
                gaze: sample.gaze,
                left: AvatarEyeSideReport {
                    w: left_tr.w / RAYON,
                    h: left_tr.h / RAYON,
                    alpha: left_tr.alpha,
                    culled: left_tr.culled,
                    cutout_op: "DstOut".to_string(),
                    pad_blend: sample.pad_blend.to_string(),
                },
                right: AvatarEyeSideReport {
                    w: right_tr.w / RAYON,
                    h: right_tr.h / RAYON,
                    alpha: right_tr.alpha,
                    culled: right_tr.culled,
                    cutout_op: "DstOut".to_string(),
                    pad_blend: sample.pad_blend.to_string(),
                },
            };
            let ornaments = AvatarOrnamentsEvalReport {
                busy_dots: sample.busy_dots.len(),
                notify_color: sample.notify_color.map(str::to_string),
                orbit_arcs: 0,
                sleep_zs: sample.sleep_zs.len(),
                shake_deg: sample.shake_deg,
            };
            (
                plush_state_str(plush_state).to_string(),
                sample.body_path,
                sample.breath_sy,
                eyes,
                ornaments,
            )
        }
    };

    let full_ramp = tint_ramp(&effective_color);
    let mut ramp_out = BTreeMap::new();
    for sample_idx in [0usize, 64, 128, 140, 200, 255] {
        ramp_out.insert(sample_idx.to_string(), full_ramp[sample_idx]);
    }

    let member_count = req.cluster_count.unwrap_or(0);
    let members: Vec<ChannelMember> = (0..member_count)
        .map(|idx| ChannelMember {
            agent_id: format!("agent-{idx}"),
        })
        .collect();
    let cluster_eval =
        resolve_channel_cluster(&members, ChannelAvatarLayout::Cluster, None, None, false);
    let cluster = AvatarClusterEvalReport {
        member_count,
        shown_faces: cluster_eval.shown_faces,
        hidden_count: cluster_eval.hidden_count,
        plus_pill: cluster_eval.plus_pill,
        fallback_glyph: cluster_eval.fallback_glyph.map(str::to_string),
    };

    let normalized_image = match req.normalize_image.as_deref() {
        Some(src_path) => {
            let norm = AvatarImageNormalizer::normalize_file(src_path, None)?;
            let out_path_str = if let Some(ref dst) = req.normalize_output_path {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(dst, &norm.bytes).map_err(|e| AvatarNormalizeError::Io {
                    detail: e.to_string(),
                    i18n: "app.avatar.processFailed",
                })?;
                dst.display().to_string()
            } else {
                src_path.with_extension("webp").display().to_string()
            };
            Some(AvatarNormalizedImageEvalReport {
                mime: norm.mime,
                width: norm.width,
                height: norm.height,
                byte_length: norm.byte_length,
                magic: norm.magic,
                animated: norm.animated,
                frame_count: norm.frame_count,
                output_path: out_path_str,
            })
        }
        None => None,
    };

    let cubic_segments = body_path.bytes().filter(|&b| b == b'C').count();

    Ok(AvatarEvalReport {
        seed: req.seed.clone(),
        profile: AvatarEvalProfileReport {
            style: resolved_style.as_str().to_string(),
            shape: effective_shape.as_str().to_string(),
            expression: effective_expr.as_str().to_string(),
            color: effective_color,
        },
        nearest_hue,
        mood: resolved_mood.as_str().to_string(),
        state: state_str,
        view_box: VIEW_BOX_STR.to_string(),
        paper: DEFAULT_PAPER.to_string(),
        rays_count: 64,
        cubic_segments,
        body_path,
        breath_sy,
        eyes: eyes_report,
        ornaments: ornaments_report,
        tint_ramp: ramp_out,
        cluster,
        normalized_image,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewPilePhysicsProbeReport {
    pub step_hz: u32,
    pub solver_iterations: usize,
    pub body_count: usize,
    pub characters: Vec<String>,
    pub breathe_period_sec: f64,
    pub phase: CrewPilePhase,
    pub settled: bool,
}

/// Runs the `crew-pile-physics` M2 probe (`TEST_INFRA.md:L2005`).
pub fn probe_crew_pile_physics(
    room: Option<CrewRoom>,
    seed: Option<u32>,
) -> CrewPilePhysicsProbeReport {
    let mut pile = CrewPile::new(seed.unwrap_or(0xC0FF_EE42), false, room.unwrap_or(DEFAULT_ROOM));
    pile.step(0.6);
    let mut seen_chars: Vec<String> = CHARACTERS.iter().map(|c| c.slug.to_string()).collect();
    for member in &pile.crew {
        let s = member.slug.to_string();
        if !seen_chars.contains(&s) {
            seen_chars.push(s);
        }
    }
    CrewPilePhysicsProbeReport {
        step_hz: (1.0 / STEP).round() as u32,
        solver_iterations: ITERATIONS,
        body_count: pile.bodies.len(),
        characters: seen_chars,
        breathe_period_sec: BREATHE_PERIOD,
        phase: pile.phase(),
        settled: pile.settled(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewPileReducedMotionProbeReport {
    pub max_squash: f64,
    pub max_tilt: f64,
    pub all_faces_idle: bool,
    pub failed_url_fallback: bool,
    pub body_count: usize,
}

/// Runs the `crew-pile-reduced-motion-and-broken-url` M2 probe (`TEST_INFRA.md:L2005`).
pub fn probe_crew_pile_reduced_motion_and_broken_url(
    room: Option<CrewRoom>,
    seed: Option<u32>,
) -> CrewPileReducedMotionProbeReport {
    let pile = CrewPile::new(seed.unwrap_or(0xC0FF_EE42), true, room.unwrap_or(DEFAULT_ROOM));
    let summary = pile.reduced_motion_summary();

    let broken_url = "file:///nonexistent/openbot_corrupt_avatar.webp";
    let mut props = AgentAvatarProps::new("agent-1");
    props.avatar_url = Some(broken_url.to_string());
    props.reduced_motion = true;
    let mut controller = AgentAvatarController::new("agent-1");
    controller.sync_url(props.avatar_url.as_deref());
    controller.mark_image_failed(broken_url);
    let resolved = controller.resolve(&props);

    let failed_set = HashSet::from([broken_url.to_string()]);
    let _stage = render_crew_pile_stage(&pile, 0.0, Some(&failed_set));

    CrewPileReducedMotionProbeReport {
        max_squash: summary.max_squash,
        max_tilt: summary.max_tilt,
        all_faces_idle: summary.all_faces_idle,
        failed_url_fallback: !resolved.uses_custom_image,
        body_count: pile.bodies.len(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarHorizonCullProbeReport {
    pub culled_at_0_02: bool,
    pub alpha_at_0_02: f64,
    pub culled_at_0_06: bool,
    pub alpha_at_0_06: f64,
}

/// Runs the `avatar-horizon-cull` M2 probe (`TEST_INFRA.md:L2005`).
pub fn probe_avatar_horizon_cull() -> AvatarHorizonCullProbeReport {
    let split = 16.0;
    let find_yaw_for_depth = |target_depth: f64| -> f64 {
        let mut lo = 0.0_f64;
        let mut hi = 110.0_f64;
        for _ in 0..48 {
            let mid = (lo + hi) * 0.5;
            let basis = project_eyes_3d(
                GazeAngles {
                    yaw: mid,
                    pitch: 0.0,
                    roll: 0.0,
                },
                RAYON,
                split,
            );
            let min_d = basis[0].depth.min(basis[1].depth);
            if min_d > target_depth {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) * 0.5
    };

    let sil = Silhouette {
        radii: engine_shape_radii(EngineShapeId::Cercle),
        rot: 0.0,
        cx: 0.0,
        cy: 0.0,
        sx: 1.0,
        sy: 1.0,
    };
    let face = expression_def(BloubExpression::Neutre);
    let yaw_002 = find_yaw_for_depth(0.019_999);
    let eyes_002 = compose_eye_transforms(
        &sil,
        GazeAngles {
            yaw: yaw_002,
            pitch: 0.0,
            roll: 0.0,
        },
        split,
        &face.eyes,
        1.0,
        1.0,
        0.0,
        0.0,
        DecalageOffset::default(),
        RAYON,
        1.0,
    );
    let far_002 = if eyes_002[0].depth <= eyes_002[1].depth {
        eyes_002[0]
    } else {
        eyes_002[1]
    };

    let yaw_006 = find_yaw_for_depth(0.06);
    let eyes_006 = compose_eye_transforms(
        &sil,
        GazeAngles {
            yaw: yaw_006,
            pitch: 0.0,
            roll: 0.0,
        },
        split,
        &face.eyes,
        1.0,
        1.0,
        0.0,
        0.0,
        DecalageOffset::default(),
        RAYON,
        1.0,
    );
    let far_006 = if eyes_006[0].depth <= eyes_006[1].depth {
        eyes_006[0]
    } else {
        eyes_006[1]
    };

    AvatarHorizonCullProbeReport {
        culled_at_0_02: far_002.culled,
        alpha_at_0_02: far_002.alpha,
        culled_at_0_06: far_006.culled,
        alpha_at_0_06: far_006.alpha,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarMidMorphProbeReport {
    pub snapped_to_rest: bool,
    pub max_pose_jump: f64,
    pub switch_at_sec: f64,
}

/// Runs the `avatar-mid-morph` M2 probe (`idle -> wide -> wink` at `t = 0.15s`, `TEST_INFRA.md:L2005`).
pub fn probe_avatar_mid_morph() -> AvatarMidMorphProbeReport {
    let switch_t = 0.15;
    let rest_engine = BloubBotEngine::new(
        EngineShapeId::Pentagone,
        BloubExpression::Neutre,
        BloubParametricState::Idle,
    );
    let rest_sample = rest_engine.sample(switch_t, false);

    let mut engine = BloubBotEngine::new(
        EngineShapeId::Pentagone,
        BloubExpression::Neutre,
        BloubParametricState::Idle,
    );
    engine.set_state(BloubParametricState::Wide, 0.0, 0.0);
    let before_switch = engine.sample(switch_t, false);

    engine.set_state(BloubParametricState::Wink, switch_t, 0.0);
    let at_switch = engine.sample(switch_t, false);
    let shortly_after = engine.sample(switch_t + 0.004, false);

    let mut max_jump = 0.0_f64;
    for idx in 0..64 {
        let r_diff_0 = (at_switch.silhouette.radii[idx] - before_switch.silhouette.radii[idx]).abs();
        let r_diff_1 =
            (shortly_after.silhouette.radii[idx] - at_switch.silhouette.radii[idx]).abs();
        max_jump = max_jump.max(r_diff_0).max(r_diff_1);
    }
    for side in 0..2 {
        let ew_jump = ((shortly_after.eyes[side].w - before_switch.eyes[side].w) / RAYON).abs();
        let eh_jump = ((shortly_after.eyes[side].h - before_switch.eyes[side].h) / RAYON).abs();
        max_jump = max_jump.max(ew_jump).max(eh_jump);
    }

    let dist_from_rest = ((at_switch.eyes[0].h - rest_sample.eyes[0].h) / RAYON).abs()
        + ((at_switch.eyes[1].h - rest_sample.eyes[1].h) / RAYON).abs();
    let snapped_to_rest = dist_from_rest < 0.02;

    AvatarMidMorphProbeReport {
        snapped_to_rest,
        max_pose_jump: max_jump,
        switch_at_sec: switch_t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_avatar_spec_empty_seed_and_goutte_rejection() {
        let empty_eval = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: String::new(),
            ..AvatarEvalRequest::default()
        })
        .expect("empty seed evaluates cleanly");
        assert_eq!(empty_eval.profile.style, "plush");
        assert_eq!(empty_eval.profile.shape, "capsule");
        assert_eq!(empty_eval.profile.expression, "excite");
        assert_eq!(empty_eval.profile.color, "#e8483f");
        assert_eq!(empty_eval.tint_ramp.get("0"), Some(&[0, 0, 0, 255]));
        assert_eq!(empty_eval.tint_ramp.get("140"), Some(&[232, 72, 63, 255]));

        let goutte_err = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "agent-1".to_string(),
            shape: Some("goutte".to_string()),
            ..AvatarEvalRequest::default()
        })
        .expect_err("goutte must be rejected as unsupported silhouette");
        assert!(goutte_err.is_validation_error());
    }

    #[test]
    fn m2_avatar_probes_meet_test_infra_invariants() {
        let cp = probe_crew_pile_physics(None, None);
        assert_eq!(cp.step_hz, 240);
        assert_eq!(cp.solver_iterations, 4);
        assert!((8..=64).contains(&cp.body_count));
        assert!((cp.breathe_period_sec - 3.2).abs() < 1e-6);

        let cp_red = probe_crew_pile_reduced_motion_and_broken_url(None, None);
        assert_eq!(cp_red.max_squash, 0.0);
        assert_eq!(cp_red.max_tilt, 0.0);
        assert!(cp_red.all_faces_idle);
        assert!(cp_red.failed_url_fallback);

        let hz = probe_avatar_horizon_cull();
        assert!(hz.culled_at_0_02);
        assert!((hz.alpha_at_0_06 - 0.5).abs() < 0.01);

        let mm = probe_avatar_mid_morph();
        assert!(!mm.snapped_to_rest);
        assert!(mm.max_pose_jump < 0.05);
    }
}
