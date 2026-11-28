//! Live interactive OpenBot-aligned avatar runtime & GPUI canvas renderer.
//!
//! Aligns Fabushi bot avatars 1:1 with `nightly-labs/openbot`:
//! - Persistent per-instance `AgentAvatarController`, `PlushFeltRenderer`, and `BloubBotEngine`
//! - Hover reactions (`HOVER_FACES`: `Wink`, `Surpris`, `Excite`, `Attentif`, `Curieux`) triggered
//!   both by direct avatar hover (`Hitbox` + `MouseMoveEvent`) and enclosing parent hover groups
//!   (sidebar rows, pinned tiles, compact rail buttons, stage header, message sender avatars,
//!   and group channel clusters)
//! - Smooth `easeOutQuint` morphing across hover enter/leave, working face cycles
//!   (`700..=3200ms`), tool activity moods (`1400ms` minimum hold), `Thinking` 3 pulsing dots +
//!   up-right gaze, `Failed` head shake + sad face, `Waiting` notify ball, and `Asleep` Zs
//! - GPU sprite-atlas-safe 2-slot frame ring buffer (`current_image` / `retired_image`) with
//!   explicit `Window::drop_image` retirement and zero-CPU static caching when idle
//! - Live 60fps `Working3Balls` hopping plush felt badge (`960ms` cycle, `0/130/260ms` stagger)

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gpui::{
    Bounds, Corners, DispatchPhase, Div, HitboxBehavior, MouseMoveEvent, ObjectFit, ParentElement,
    Pixels, RenderImage, Styled, StyledImage, Window, canvas, div, fill, img, point, px, rgb, size,
};
use openbot_contracts::avatar::{
    AvatarMood, AvatarStyle, BloubExpression, BloubShape, PlushState, bloub_avatar_profile,
};
use parking_lot::Mutex;

use super::bot_engine::{BloubBotEngine, PointerKind, paint_flat_bloub};
use super::clusters_and_editor::{
    AVATAR_BREATHE_PERIOD_SEC, AVATAR_FPS, AgentAvatarController, AgentAvatarProps,
    AgentAvatarResolvedState, BACKGROUND_AVATAR_FPS, SidebarPlushRingKind,
    WORKING_BADGE_HEIGHT_PX, WORKING_BADGE_WIDTH_PX, cached_custom_url_render_image,
    cached_ui_felt_webp_shared, compute_working_badge_layout,
};
use super::plush_felt::{
    PlushCharacterId, PlushFeltRenderer, bloub_shape_to_engine, cached_static_plush_render_image,
    composite_plush_rgba, plush_pixels, rgba_to_gpui_render_image, small_size_eye_scale,
};
use super::silhouettes::{
    DEFAULT_PAPER, Silhouette, VIEW_BOX, bloub_shape_radii, silhouette_gpui_path,
};

/// Duration after hover-leave or state/expression change during which the live avatar
/// continues animating at 60fps so smooth morphs, shakes, and decor exits complete cleanly.
const MORPH_SETTLE_SEC: f64 = 0.85;
const MAX_LIVE_AVATAR_INSTANCES: usize = 512;
const STALE_LIVE_AVATAR_TTL_SEC: f64 = 60.0;

static AVATAR_CLOCK_EPOCH: OnceLock<Instant> = OnceLock::new();

#[inline]
fn clock_epoch() -> Instant {
    *AVATAR_CLOCK_EPOCH.get_or_init(Instant::now)
}

/// Monotonic wall clock in seconds for avatar motion & smooth transitions.
pub fn avatar_clock_sec() -> f64 {
    Instant::now()
        .saturating_duration_since(clock_epoch())
        .as_secs_f64()
}

/// Monotonic wall clock in milliseconds for working-face cycles, activity holds, and badge hops.
pub fn avatar_clock_ms() -> u64 {
    Instant::now()
        .saturating_duration_since(clock_epoch())
        .as_millis() as u64
}

/// Specification for rendering a live interactive OpenBot avatar.
#[derive(Debug, Clone)]
pub struct LiveAvatarSpec {
    pub instance_key: String,
    pub hover_group_key: Option<String>,
    pub props: AgentAvatarProps,
    pub character: Option<PlushCharacterId>,
}

impl LiveAvatarSpec {
    pub fn new(instance_key: impl Into<String>, props: AgentAvatarProps) -> Self {
        Self {
            instance_key: instance_key.into(),
            hover_group_key: None,
            props,
            character: None,
        }
    }

    pub fn with_hover_group(mut self, group_key: impl Into<String>) -> Self {
        self.hover_group_key = Some(group_key.into());
        self
    }

    pub fn with_optional_hover_group(mut self, group_key: Option<String>) -> Self {
        self.hover_group_key = group_key.filter(|k| !k.trim().is_empty());
        self
    }

    pub fn with_character(mut self, character: Option<PlushCharacterId>) -> Self {
        self.character = character;
        self
    }
}

struct LiveAvatarRuntime {
    seed: String,
    controller: AgentAvatarController,
    plush: Option<PlushFeltRenderer>,
    flat: Option<BloubBotEngine>,
    direct_hovered: bool,
    pointer_gaze: Option<(f64, f64)>,
    last_state: Option<PlushState>,
    last_expression: Option<BloubExpression>,
    last_shape: Option<BloubShape>,
    last_color: Option<String>,
    last_character: Option<PlushCharacterId>,
    morph_settle_until_sec: f64,
    last_frame_sec: f64,
    last_seen_sec: f64,
    /// Dynamic animated frame painted in the current/most recent frame (owned by this runtime,
    /// NEVER a shared static cache entry).
    current_image: Option<Arc<RenderImage>>,
    /// Dynamic animated frame from the previous frame, safely dropped via `window.drop_image`
    /// once superseded so GPUI's GPU sprite atlas never leaks.
    retired_image: Option<Arc<RenderImage>>,
}

impl LiveAvatarRuntime {
    fn new(seed: &str, now_sec: f64) -> Self {
        Self {
            seed: seed.to_string(),
            controller: AgentAvatarController::new(seed),
            plush: None,
            flat: None,
            direct_hovered: false,
            pointer_gaze: None,
            last_state: None,
            last_expression: None,
            last_shape: None,
            last_color: None,
            last_character: None,
            morph_settle_until_sec: 0.0,
            last_frame_sec: f64::NEG_INFINITY,
            last_seen_sec: now_sec,
            current_image: None,
            retired_image: None,
        }
    }

    fn take_owned_images(&mut self, out: &mut Vec<Arc<RenderImage>>) {
        if let Some(img) = self.retired_image.take() {
            out.push(img);
        }
        if let Some(img) = self.current_image.take() {
            out.push(img);
        }
    }
}

#[derive(Default)]
struct LiveAvatarRegistry {
    instances: HashMap<String, LiveAvatarRuntime>,
    group_hovers: HashMap<String, bool>,
    pending_drop_images: Vec<Arc<RenderImage>>,
    last_prune_sec: f64,
}

impl LiveAvatarRegistry {
    fn is_group_hovered(&self, group_key: &str) -> bool {
        self.group_hovers.get(group_key).copied().unwrap_or(false)
    }

    fn set_group_hover(&mut self, group_key: &str, hovered: bool) -> bool {
        let prev = self.is_group_hovered(group_key);
        if prev == hovered {
            return false;
        }
        if hovered {
            self.group_hovers.insert(group_key.to_string(), true);
        } else {
            self.group_hovers.remove(group_key);
        }
        true
    }

    fn maybe_prune(&mut self, now_sec: f64) {
        if self.instances.len() <= MAX_LIVE_AVATAR_INSTANCES
            && (now_sec - self.last_prune_sec) < 15.0
        {
            return;
        }
        self.last_prune_sec = now_sec;
        let stale_keys: Vec<String> = self
            .instances
            .iter()
            .filter(|(_, rt)| {
                !rt.direct_hovered
                    && (now_sec - rt.last_seen_sec) > STALE_LIVE_AVATAR_TTL_SEC
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale_keys {
            if let Some(mut rt) = self.instances.remove(&key) {
                rt.take_owned_images(&mut self.pending_drop_images);
            }
        }
        if self.instances.len() > MAX_LIVE_AVATAR_INSTANCES {
            let mut by_age: Vec<(String, f64)> = self
                .instances
                .iter()
                .map(|(k, rt)| (k.clone(), rt.last_seen_sec))
                .collect();
            by_age.sort_by(|a, b| a.1.total_cmp(&b.1));
            let remove_count = self.instances.len() - MAX_LIVE_AVATAR_INSTANCES;
            for (key, _) in by_age.into_iter().take(remove_count) {
                if let Some(mut rt) = self.instances.remove(&key) {
                    rt.take_owned_images(&mut self.pending_drop_images);
                }
            }
        }
    }

    fn drain_pending_drops(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.pending_drop_images)
    }
}

static LIVE_AVATAR_REGISTRY: OnceLock<Mutex<LiveAvatarRegistry>> = OnceLock::new();

fn registry() -> &'static Mutex<LiveAvatarRegistry> {
    LIVE_AVATAR_REGISTRY.get_or_init(|| Mutex::new(LiveAvatarRegistry::default()))
}

/// Updates the shared parent hover-group state (OpenBot `HOVER_GROUP` semantics).
///
/// Returns `true` if the hover state changed (callers should call `cx.notify()` on `true`).
pub fn set_live_avatar_group_hover(group_key: &str, hovered: bool) -> bool {
    if group_key.trim().is_empty() {
        return false;
    }
    registry().lock().set_group_hover(group_key, hovered)
}

/// Returns whether a parent hover group is currently hovered.
pub fn is_live_avatar_group_hovered(group_key: &str) -> bool {
    if group_key.trim().is_empty() {
        return false;
    }
    registry().lock().is_group_hovered(group_key)
}

/// Inspects a live avatar's resolved state at explicit `(now_sec, now_ms)` (primarily for tests
/// and diagnostics).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveAvatarStepReport {
    pub interacting: bool,
    pub resolved: AgentAvatarResolvedState,
    pub settling: bool,
    pub is_animating: bool,
    pub plush_state: PlushState,
    pub expression: BloubExpression,
    pub busy_dots: usize,
    pub has_notif_ball: bool,
    pub sleep_zs: usize,
    pub shake_deg: f64,
    pub gaze_yaw: f64,
    pub gaze_pitch: f64,
}

fn step_runtime_controller(
    rt: &mut LiveAvatarRuntime,
    props: &AgentAvatarProps,
    interacting: bool,
    now_ms: u64,
) -> (AgentAvatarResolvedState, bool) {
    let seed = if props.seed.trim().is_empty() {
        "agent"
    } else {
        props.seed.as_str()
    };
    if rt.seed != seed {
        rt.seed = seed.to_string();
        rt.controller = AgentAvatarController::new(seed);
    }
    let profile = bloub_avatar_profile(seed, props.hue, props.color.as_deref());
    let resting = props.expression_override.unwrap_or(profile.expression);
    let prev_interacting = rt.controller.interacting;

    rt.controller.sync_url(props.avatar_url.as_deref());
    rt.controller.sync_activity(props.activity, now_ms);
    rt.controller.set_interacting(interacting, resting);

    if props.working_faces && props.mood == AvatarMood::Working && !props.reduced_motion {
        rt.controller.tick_working_faces(seed, now_ms, false);
    }

    let resolved = rt.controller.resolve(props);
    (resolved, prev_interacting != interacting)
}

fn sync_plush_renderer(
    rt: &mut LiveAvatarRuntime,
    props: &AgentAvatarProps,
    resolved: &AgentAvatarResolvedState,
    character: Option<PlushCharacterId>,
    interacting: bool,
    interacting_changed: bool,
    now_sec: f64,
    base_size: f32,
    window_focused: bool,
) -> (bool, bool) {
    let anim_t = now_sec + resolved.initial_phase;
    let eff_eye_scale = small_size_eye_scale(base_size).max(
        if props.eye_scale.is_finite() && props.eye_scale > 0.0 {
            props.eye_scale
        } else {
            1.0
        },
    );
    let use_character_resting_face = character.is_some()
        && !interacting
        && props.expression_override.is_none()
        && !resolved.busy
        && props.mood == AvatarMood::Idle;

    let mut changed = interacting_changed;

    if rt.plush.is_none() {
        let mut renderer = if let Some(ch) = character {
            let mut r = PlushFeltRenderer::for_character(ch, resolved.plush_state);
            if !use_character_resting_face {
                r.set_expression(resolved.expression);
            }
            r
        } else {
            PlushFeltRenderer::new(
                bloub_shape_to_engine(resolved.shape),
                &resolved.color,
                resolved.expression,
                resolved.plush_state,
            )
        };
        renderer.set_transition(props.transition);
        renderer.set_eye_scale(eff_eye_scale);
        rt.plush = Some(renderer);
        rt.last_character = character;
        rt.last_shape = Some(resolved.shape);
        rt.last_color = Some(resolved.color.clone());
        rt.last_state = Some(resolved.plush_state);
        rt.last_expression = if use_character_resting_face {
            None
        } else {
            Some(resolved.expression)
        };
        if resolved.plush_state == PlushState::Failed && !props.reduced_motion {
            if let Some(ref mut r) = rt.plush {
                r.set_state(PlushState::Idle, anim_t);
                r.set_state(PlushState::Failed, anim_t);
            }
            changed = true;
        }
    } else if let Some(ref mut renderer) = rt.plush {
        renderer.set_transition(props.transition);
        renderer.set_eye_scale(eff_eye_scale);

        if rt.last_character != character {
            if let Some(ch) = character {
                renderer.set_character(Some(ch), anim_t);
            } else {
                renderer.set_shape(bloub_shape_to_engine(resolved.shape), anim_t);
                renderer.set_color(&resolved.color, anim_t);
            }
            rt.last_character = character;
            rt.last_shape = Some(resolved.shape);
            rt.last_color = Some(resolved.color.clone());
            changed = true;
        } else if character.is_none() {
            if rt.last_shape != Some(resolved.shape) {
                renderer.set_shape(bloub_shape_to_engine(resolved.shape), anim_t);
                rt.last_shape = Some(resolved.shape);
                changed = true;
            }
            if rt.last_color.as_deref() != Some(resolved.color.as_str()) {
                renderer.set_color(&resolved.color, anim_t);
                rt.last_color = Some(resolved.color.clone());
                changed = true;
            }
        }

        if rt.last_state != Some(resolved.plush_state) {
            renderer.set_state(resolved.plush_state, anim_t);
            rt.last_state = Some(resolved.plush_state);
            changed = true;
        }

        if use_character_resting_face {
            if rt.last_expression.is_some() {
                renderer.clear_expression_override_at(anim_t);
                rt.last_expression = None;
                changed = true;
            }
        } else if rt.last_expression != Some(resolved.expression) {
            renderer.set_expression_at(resolved.expression, anim_t);
            rt.last_expression = Some(resolved.expression);
            changed = true;
        }
    }

    if let Some(ref mut renderer) = rt.plush {
        if props.follow_pointer && !props.reduced_motion && window_focused {
            renderer.set_pointer_gaze(rt.pointer_gaze);
        } else {
            renderer.set_pointer_gaze(None);
        }
    }

    if changed && !props.reduced_motion {
        rt.morph_settle_until_sec = rt.morph_settle_until_sec.max(now_sec + MORPH_SETTLE_SEC);
    }

    let in_trans = rt.plush.as_ref().is_some_and(|r| r.is_in_transition());
    let settling = !props.reduced_motion && (now_sec < rt.morph_settle_until_sec || in_trans);
    (changed, settling)
}

/// Steps a live avatar runtime deterministically for unit tests and probes.
pub fn step_live_avatar_for_test(
    spec: &LiveAvatarSpec,
    direct_hovered: Option<bool>,
    now_sec: f64,
    now_ms: u64,
) -> LiveAvatarStepReport {
    let mut reg = registry().lock();
    let group_hovered = spec
        .hover_group_key
        .as_deref()
        .is_some_and(|k| reg.is_group_hovered(k));
    let rt = reg
        .instances
        .entry(spec.instance_key.clone())
        .or_insert_with(|| LiveAvatarRuntime::new(&spec.props.seed, now_sec));
    rt.last_seen_sec = now_sec;
    if let Some(h) = direct_hovered {
        rt.direct_hovered = h;
    }
    let interacting = rt.direct_hovered || group_hovered;
    let (resolved, interacting_changed) =
        step_runtime_controller(rt, &spec.props, interacting, now_ms);
    let base_size = if spec.props.size_px.is_finite() {
        spec.props.size_px.clamp(8.0, 512.0)
    } else {
        32.0
    };
    let (_, settling) = sync_plush_renderer(
        rt,
        &spec.props,
        &resolved,
        spec.character,
        interacting,
        interacting_changed,
        now_sec,
        base_size,
        spec.props.document_focused,
    );
    let is_animating = !spec.props.reduced_motion && (resolved.animated || settling);
    let anim_t = now_sec + resolved.initial_phase;
    let sample = rt
        .plush
        .as_mut()
        .expect("plush renderer initialized")
        .sample_plush_frame(
            if is_animating {
                anim_t
            } else {
                resolved.frozen_at.unwrap_or(0.0)
            },
            !is_animating,
        );
    LiveAvatarStepReport {
        interacting,
        resolved,
        settling,
        is_animating,
        plush_state: sample.state,
        expression: sample.expression,
        busy_dots: sample.busy_dots.len(),
        has_notif_ball: sample.notif_ball.is_some(),
        sleep_zs: sample.sleep_zs.len(),
        shake_deg: sample.shake_deg,
        gaze_yaw: sample.gaze.yaw,
        gaze_pitch: sample.gaze.pitch,
    }
}

fn paint_cutout_ring(
    bounds: Bounds<Pixels>,
    ring_kind: SidebarPlushRingKind,
    shape: BloubShape,
    sil: &Silhouette,
    dark_scheme: bool,
    window: &mut Window,
) {
    let ring = ring_kind.spec();
    let center = bounds.center();
    let min_dim = f32::from(bounds.size.width.min(bounds.size.height));
    let px_per_vb = (min_dim / (VIEW_BOX as f32)).max(0.01);
    let ring_rgb = if dark_scheme {
        ring.dark_fallback_rgb
    } else {
        ring.light_fallback_rgb
    };
    let dilate_scale = 1.0 + f64::from(ring.dilate_radius_px / (min_dim * 0.32).max(1.0));
    let ring_sil = Silhouette {
        radii: bloub_shape_radii(shape),
        rot: sil.rot,
        cx: sil.cx,
        cy: sil.cy,
        sx: sil.sx * dilate_scale,
        sy: sil.sy * dilate_scale,
    };
    if let Some(ring_path) = silhouette_gpui_path(&ring_sil, center, px_per_vb) {
        window.paint_path(ring_path, rgb(ring_rgb));
    }
}

/// Renders an interactive, live-animated OpenBot avatar (`Plush`, `Flat`, or custom photo).
///
/// - Idle avatars render from the permanent static sprite cache at 0% CPU.
/// - Hovering the avatar or its enclosing `hover_group_key` triggers a smooth morph into a
///   non-repeating `HOVER_FACES` reaction at 60fps, then smoothly morphs back on leave.
/// - Working avatars smoothly cycle faces (`working_faces = true`) or animate thinking dots /
///   tool activity moods (`working_faces = false`).
pub fn render_live_agent_avatar(spec: LiveAvatarSpec) -> Div {
    let base_size = if spec.props.size_px.is_finite() {
        spec.props.size_px.clamp(8.0, 512.0)
    } else {
        32.0
    };

    // Fast pre-check for remote http(s) custom image URLs that GPUI fetches asynchronously via `img(url)`.
    let default_ctrl = AgentAvatarController::new(&spec.props.seed);
    let pre_resolved = default_ctrl.resolve(&spec.props);
    if pre_resolved.uses_custom_image
        && let Some(url) = pre_resolved.active_url.as_deref()
        && (url.starts_with("http://") || url.starts_with("https://"))
    {
        let now_sec = avatar_clock_sec();
        let breathe_scale = if pre_resolved.breathe_amount > 0.0 && !spec.props.reduced_motion {
            let wave = ((std::f64::consts::TAU * now_sec) / AVATAR_BREATHE_PERIOD_SEC).cos();
            (1.0 - pre_resolved.breathe_amount * wave) as f32
        } else {
            1.0
        };
        let disp = base_size * breathe_scale;
        let inset = (base_size - disp) * 0.5;
        let url_owned = url.to_string();
        let should_tick = pre_resolved.breathe_amount > 0.0 && !spec.props.reduced_motion;
        return div()
            .relative()
            .w(px(base_size))
            .h(px(base_size))
            .flex_shrink_0()
            .opacity(pre_resolved.custom_image_opacity)
            .child(
                div()
                    .absolute()
                    .left(px(inset))
                    .top(px(inset))
                    .w(px(disp))
                    .h(px(disp))
                    .rounded_full()
                    .overflow_hidden()
                    .child(
                        img(url_owned)
                            .w_full()
                            .h_full()
                            .object_fit(ObjectFit::Cover),
                    ),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        if should_tick {
                            window.request_animation_frame();
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );
    }

    let spec_arc = Arc::new(spec);
    div()
        .relative()
        .w(px(base_size))
        .h(px(base_size))
        .flex_shrink_0()
        .child(
            canvas(
                move |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                move |bounds, hitbox, window, _cx| {
                    let now_sec = avatar_clock_sec();
                    let now_ms = avatar_clock_ms();
                    let view_id = window.current_view();
                    let window_active = window.is_window_active();
                    let window_hovered = window.is_window_hovered();
                    let window_focused = window_active || window_hovered;
                    let scale_factor = window.scale_factor();

                    // Register pointer listener so direct avatar hover & followPointer work cleanly
                    // even without an enclosing parent `.on_hover(...)`.
                    let move_instance_key = spec_arc.instance_key.clone();
                    let move_follow_pointer =
                        spec_arc.props.follow_pointer && !spec_arc.props.reduced_motion;
                    let move_hitbox = hitbox.clone();
                    let move_bounds = bounds;
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, win, app| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let is_over = win.is_window_hovered() && move_hitbox.is_hovered(win);
                        let mut needs_notify = false;
                        {
                            let mut reg = registry().lock();
                            if let Some(rt) = reg.instances.get_mut(&move_instance_key) {
                                if rt.direct_hovered != is_over {
                                    rt.direct_hovered = is_over;
                                    needs_notify = true;
                                }
                                if move_follow_pointer && (win.is_window_active() || win.is_window_hovered()) {
                                    let vp = win.viewport_size();
                                    let cx_px = f32::from(move_bounds.center().x);
                                    let cy_px = f32::from(move_bounds.center().y);
                                    let mx_px = f32::from(event.position.x);
                                    let my_px = f32::from(event.position.y);
                                    let half_w = cx_px.max(f32::from(vp.width) - cx_px).max(1.0);
                                    let half_h = cy_px.max(f32::from(vp.height) - cy_px).max(1.0);
                                    let nx = f64::from(((mx_px - cx_px) / half_w).clamp(-1.0, 1.0));
                                    let ny = f64::from(((my_px - cy_px) / half_h).clamp(-1.0, 1.0));
                                    let changed_gaze = match rt.pointer_gaze {
                                        Some((ox, oy)) => {
                                            (ox - nx).abs() > 0.015 || (oy - ny).abs() > 0.015
                                        }
                                        None => true,
                                    };
                                    if changed_gaze {
                                        rt.pointer_gaze = Some((nx, ny));
                                        needs_notify = true;
                                    }
                                }
                            }
                        }
                        if needs_notify {
                            app.notify(view_id);
                        }
                    });

                    // Drain any GPU sprite images from pruned instances before painting.
                    let pending_drops = {
                        let mut reg = registry().lock();
                        reg.maybe_prune(now_sec);
                        reg.drain_pending_drops()
                    };
                    for stale_img in pending_drops {
                        let _ = window.drop_image(stale_img);
                    }

                    // Step controller and prepare frame state under lock.
                    let mut to_drop: Vec<Arc<RenderImage>> = Vec::new();
                    let should_request_frame;

                    // Resolve effective props for live painting.
                    let mut live_props = spec_arc.props.clone();
                    if !live_props.follow_pointer && live_props.motion.is_static() {
                        // Keep busy working & hover reactive while window is on screen.
                        live_props.document_focused = spec_arc.props.document_focused;
                    } else if live_props.follow_pointer || !live_props.motion.is_static() {
                        live_props.document_focused =
                            spec_arc.props.document_focused && window_focused;
                    }

                    enum PaintPlan {
                        CustomImage {
                            image: Arc<RenderImage>,
                            disp: f32,
                            inset: f32,
                        },
                        Flat {
                            sample: super::bot_engine::BloubFrameSample,
                            ink_hex: String,
                            optical_box: f32,
                            optical_offset_y: f32,
                        },
                        Plush {
                            image: Arc<RenderImage>,
                            ring_kind: Option<SidebarPlushRingKind>,
                            shape: BloubShape,
                            silhouette: Silhouette,
                            dark_scheme: bool,
                            plush_left: f32,
                            plush_top: f32,
                            plush_px: f32,
                        },
                        Empty,
                    }

                    let plan = {
                        let mut reg = registry().lock();
                        let group_hovered = spec_arc
                            .hover_group_key
                            .as_deref()
                            .is_some_and(|k| reg.is_group_hovered(k));
                        let rt = reg
                            .instances
                            .entry(spec_arc.instance_key.clone())
                            .or_insert_with(|| LiveAvatarRuntime::new(&live_props.seed, now_sec));
                        rt.last_seen_sec = now_sec;
                        if !window_hovered {
                            rt.direct_hovered = false;
                        }
                        let interacting = rt.direct_hovered || group_hovered;

                        let (resolved, interacting_changed) =
                            step_runtime_controller(rt, &live_props, interacting, now_ms);

                        if resolved.uses_custom_image
                            && let Some(url) = resolved.active_url.as_deref()
                            && let Some(img_arc) = cached_custom_url_render_image(url)
                        {
                            let breathe_active =
                                resolved.breathe_amount > 0.0 && !live_props.reduced_motion;
                            let breathe_scale = if breathe_active {
                                let wave = ((std::f64::consts::TAU * now_sec)
                                    / AVATAR_BREATHE_PERIOD_SEC)
                                    .cos();
                                (1.0 - resolved.breathe_amount * wave) as f32
                            } else {
                                1.0
                            };
                            should_request_frame = breathe_active;
                            let disp = base_size * breathe_scale;
                            let inset = (base_size - disp) * 0.5;
                            PaintPlan::CustomImage {
                                image: img_arc,
                                disp,
                                inset,
                            }
                        } else if resolved.style == AvatarStyle::Flat && spec_arc.character.is_none()
                        {
                            let anim_t = now_sec + resolved.initial_phase;
                            let eng_shape = bloub_shape_to_engine(resolved.shape);
                            let eng_state = resolved.flat_state.to_parametric();
                            let mut changed = interacting_changed;
                            if rt.flat.is_none() {
                                rt.flat = Some(BloubBotEngine::new(
                                    eng_shape,
                                    resolved.expression,
                                    eng_state,
                                ));
                                rt.last_shape = Some(resolved.shape);
                                rt.last_expression = Some(resolved.expression);
                            } else if let Some(ref mut eng) = rt.flat {
                                if eng.shape() != eng_shape {
                                    eng.set_shape(eng_shape, anim_t);
                                    changed = true;
                                }
                                if eng.expression() != resolved.expression {
                                    eng.set_expression(resolved.expression, anim_t);
                                    changed = true;
                                }
                                if eng.state() != eng_state {
                                    eng.set_state(eng_state, anim_t, 0.0);
                                    changed = true;
                                }
                                if live_props.follow_pointer
                                    && !live_props.reduced_motion
                                    && window_focused
                                {
                                    if let Some((nx, ny)) = rt.pointer_gaze {
                                        eng.set_pointer(PointerKind::Mouse, nx, ny, true, anim_t);
                                    }
                                } else if eng.is_pointer_active() {
                                    eng.set_pointer(PointerKind::Mouse, 0.0, 0.0, false, anim_t);
                                    changed = true;
                                }
                            }
                            if changed && !live_props.reduced_motion {
                                rt.morph_settle_until_sec =
                                    rt.morph_settle_until_sec.max(now_sec + MORPH_SETTLE_SEC);
                            }
                            let settling = !live_props.reduced_motion
                                && now_sec < rt.morph_settle_until_sec;
                            let is_animating =
                                !live_props.reduced_motion && (resolved.animated || settling);
                            should_request_frame = is_animating;
                            let breathe_scale =
                                if resolved.breathe_amount > 0.0 && is_animating {
                                    let wave = ((std::f64::consts::TAU * now_sec)
                                        / AVATAR_BREATHE_PERIOD_SEC)
                                        .cos();
                                    (1.0 - resolved.breathe_amount * wave) as f32
                                } else {
                                    1.0
                                };
                            let sample = rt.flat.as_ref().expect("flat engine").sample(
                                if is_animating {
                                    anim_t
                                } else {
                                    resolved.frozen_at.unwrap_or(0.0)
                                },
                                !is_animating,
                            );
                            PaintPlan::Flat {
                                sample,
                                ink_hex: resolved.color,
                                optical_box: live_props.optical_box_px() * breathe_scale,
                                optical_offset_y: live_props.optical_offset_px(),
                            }
                        } else {
                            let (changed, settling) = sync_plush_renderer(
                                rt,
                                &live_props,
                                &resolved,
                                spec_arc.character,
                                interacting,
                                interacting_changed,
                                now_sec,
                                base_size,
                                window_focused,
                            );
                            let is_animating =
                                !live_props.reduced_motion && (resolved.animated || settling);
                            should_request_frame = is_animating;

                            let breathe_scale =
                                if resolved.breathe_amount > 0.0 && is_animating {
                                    let wave = ((std::f64::consts::TAU * now_sec)
                                        / AVATAR_BREATHE_PERIOD_SEC)
                                        .cos();
                                    (1.0 - resolved.breathe_amount * wave) as f32
                                } else {
                                    1.0
                                };
                            let optical_box = live_props.optical_box_px() * breathe_scale;
                            let optical_offset_y = live_props.optical_offset_px();
                            let plush_px = optical_box * resolved.plush_body_scale;
                            let target_pixels = plush_pixels(plush_px, scale_factor);

                            if let Some(ref mut renderer) = rt.plush {
                                if !is_animating {
                                    let sample = renderer.sample_plush_frame(
                                        resolved.frozen_at.unwrap_or(0.0),
                                        true,
                                    );
                                    let static_img =
                                        cached_static_plush_render_image(&sample, target_pixels);
                                    if let Some(old) = rt.retired_image.take() {
                                        to_drop.push(old);
                                    }
                                    if let Some(prev) = rt.current_image.take() {
                                        rt.retired_image = Some(prev);
                                    }
                                    let plush_left = (base_size - plush_px) * 0.5;
                                    let plush_top =
                                        (base_size - plush_px) * 0.5 + optical_offset_y;
                                    PaintPlan::Plush {
                                        image: static_img,
                                        ring_kind: live_props.cutout_ring,
                                        shape: resolved.shape,
                                        silhouette: sample.silhouette,
                                        dark_scheme: live_props.dark_scheme,
                                        plush_left,
                                        plush_top,
                                        plush_px,
                                    }
                                } else {
                                    let target_fps = if settling
                                        || interacting
                                        || live_props.follow_pointer
                                        || !live_props.motion.is_static()
                                    {
                                        AVATAR_FPS
                                    } else {
                                        resolved.fps.max(BACKGROUND_AVATAR_FPS)
                                    };
                                    let min_dt = 1.0 / f64::from(target_fps.max(1));
                                    let need_fresh = rt.current_image.is_none()
                                        || changed
                                        || (now_sec - rt.last_frame_sec) >= min_dt * 0.85;
                                    let anim_t = now_sec + resolved.initial_phase;
                                    let sample = renderer.sample_plush_frame(anim_t, false);
                                    let img_to_paint = if need_fresh {
                                        let rgba = composite_plush_rgba(&sample, target_pixels);
                                        let fresh = rgba_to_gpui_render_image(rgba);
                                        if let Some(old) = rt.retired_image.take() {
                                            to_drop.push(old);
                                        }
                                        rt.retired_image = rt.current_image.replace(fresh.clone());
                                        rt.last_frame_sec = now_sec;
                                        fresh
                                    } else {
                                        rt.current_image
                                            .clone()
                                            .expect("current_image present when !need_fresh")
                                    };
                                    let shake_dx = (sample.shake_deg * 0.14) as f32;
                                    let plush_left = (base_size - plush_px) * 0.5 + shake_dx;
                                    let plush_top =
                                        (base_size - plush_px) * 0.5 + optical_offset_y;
                                    PaintPlan::Plush {
                                        image: img_to_paint,
                                        ring_kind: live_props.cutout_ring,
                                        shape: resolved.shape,
                                        silhouette: sample.silhouette,
                                        dark_scheme: live_props.dark_scheme,
                                        plush_left,
                                        plush_top,
                                        plush_px,
                                    }
                                }
                            } else {
                                PaintPlan::Empty
                            }
                        }
                    };

                    for stale in to_drop {
                        let _ = window.drop_image(stale);
                    }

                    match plan {
                        PaintPlan::CustomImage { image, disp, inset } => {
                            let img_bounds = Bounds::new(
                                point(
                                    bounds.origin.x + px(inset),
                                    bounds.origin.y + px(inset),
                                ),
                                size(px(disp), px(disp)),
                            );
                            let _ = window.paint_image(
                                img_bounds,
                                Corners::all(px(disp * 0.5)),
                                image,
                                0,
                                false,
                            );
                        }
                        PaintPlan::Flat {
                            sample,
                            ink_hex,
                            optical_box,
                            optical_offset_y,
                        } => {
                            let box_left = (base_size - optical_box) * 0.5;
                            let box_top = (base_size - optical_box) * 0.5 + optical_offset_y;
                            let flat_bounds = Bounds::new(
                                point(
                                    bounds.origin.x + px(box_left),
                                    bounds.origin.y + px(box_top),
                                ),
                                size(px(optical_box), px(optical_box)),
                            );
                            paint_flat_bloub(
                                flat_bounds,
                                &sample,
                                &ink_hex,
                                DEFAULT_PAPER,
                                None,
                                window,
                            );
                        }
                        PaintPlan::Plush {
                            image,
                            ring_kind,
                            shape,
                            silhouette,
                            dark_scheme,
                            plush_left,
                            plush_top,
                            plush_px,
                        } => {
                            let plush_bounds = Bounds::new(
                                point(
                                    bounds.origin.x + px(plush_left),
                                    bounds.origin.y + px(plush_top),
                                ),
                                size(px(plush_px), px(plush_px)),
                            );
                            if let Some(rk) = ring_kind {
                                paint_cutout_ring(
                                    plush_bounds,
                                    rk,
                                    shape,
                                    &silhouette,
                                    dark_scheme,
                                    window,
                                );
                            }
                            let _ = window.paint_image(
                                plush_bounds,
                                Corners::default(),
                                image,
                                0,
                                false,
                            );
                        }
                        PaintPlan::Empty => {}
                    }

                    if should_request_frame {
                        window.request_animation_frame();
                    }
                },
            )
            .w(px(base_size))
            .h(px(base_size)),
        )
}

/// Renders the live 60fps OpenBot `30×16px` 3-ball hopping plush working badge
/// (`sidebar-working-badge.css`: `960ms` cycle, `0/130/260ms` stagger, `-1.75px` hop).
pub fn render_live_working_balls_badge(
    dark_scheme: bool,
    reduced_motion: bool,
    opacity: f32,
) -> Div {
    let ball_imgs: [Option<Arc<RenderImage>>; 3] = [
        cached_ui_felt_webp_shared("openbot_plush/ui/ball-lilac.webp"),
        cached_ui_felt_webp_shared("openbot_plush/ui/ball-mint.webp"),
        cached_ui_felt_webp_shared("openbot_plush/ui/ball-pink.webp"),
    ];
    div()
        .relative()
        .w(px(WORKING_BADGE_WIDTH_PX))
        .h(px(WORKING_BADGE_HEIGHT_PX))
        .opacity(opacity)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    let now_ms = if reduced_motion {
                        0
                    } else {
                        avatar_clock_ms().max(1)
                    };
                    let layout = compute_working_badge_layout(dark_scheme, reduced_motion, now_ms);
                    let pill_radius = px(layout.height_px * 0.5);
                    window.paint_quad(fill(bounds, rgb(layout.bg_rgb)).corner_radii(pill_radius));

                    // Three 7px balls separated by 1px gap inside 30px pill -> centers at 7, 15, 23px.
                    let centers_x = [7.0_f32, 15.0_f32, 23.0_f32];
                    let baseline_y = f32::from(bounds.origin.y) + layout.height_px - 3.0;
                    for idx in 0..3 {
                        let (dy, bw, bh) = layout.ball_hops[idx];
                        let bx = f32::from(bounds.origin.x) + centers_x[idx] - bw * 0.5;
                        let by = baseline_y + dy - bh;
                        let ball_bounds = Bounds::new(point(px(bx), px(by)), size(px(bw), px(bh)));
                        if let Some(ref img_arc) = ball_imgs[idx] {
                            let _ = window.paint_image(
                                ball_bounds,
                                Corners::all(px(bw.min(bh) * 0.5)),
                                img_arc.clone(),
                                0,
                                false,
                            );
                        } else {
                            window.paint_quad(
                                fill(ball_bounds, rgb(layout.ball_fallback_rgbs[idx]))
                                    .corner_radii(px(bw.min(bh) * 0.5)),
                            );
                        }
                    }

                    if !reduced_motion {
                        window.request_animation_frame();
                    }
                },
            )
            .w(px(WORKING_BADGE_WIDTH_PX))
            .h(px(WORKING_BADGE_HEIGHT_PX)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbot_contracts::avatar::{AgentStepActivity, HOVER_FACES};

    #[test]
    fn live_avatar_hover_picks_non_repeating_hover_face_and_morphs_back_on_leave() {
        let mut props = AgentAvatarProps::new("live-hover-bot-1")
            .with_size(36.0)
            .with_shape_override(Some(BloubShape::Squircle))
            .with_allow_hover_face(true);
        props.mood = AvatarMood::Idle;
        let spec = LiveAvatarSpec::new("test-live-hover-1", props);

        let idle_before = step_live_avatar_for_test(&spec, Some(false), 10.0, 10_000);
        assert!(!idle_before.interacting);
        assert!(!idle_before.is_animating);

        let hover_1 = step_live_avatar_for_test(&spec, Some(true), 10.1, 10_100);
        assert!(hover_1.interacting);
        assert!(hover_1.is_animating);
        assert_eq!(hover_1.resolved.fps, AVATAR_FPS);
        let is_hover_face = HOVER_FACES.iter().any(|f| {
            f.state == hover_1.plush_state
                && (f.expression.is_none() || f.expression == Some(hover_1.expression))
        });
        assert!(
            is_hover_face,
            "hover must resolve to one of HOVER_FACES: state={:?} expr={:?}",
            hover_1.plush_state, hover_1.expression
        );

        // Leaving hover keeps settling animation active for smooth morph back, then sleeps to 0% CPU.
        let leaving = step_live_avatar_for_test(&spec, Some(false), 10.3, 10_300);
        assert!(!leaving.interacting);
        assert!(leaving.settling);
        assert!(leaving.is_animating);

        let settled = step_live_avatar_for_test(&spec, Some(false), 12.0, 12_000);
        assert!(!settled.settling);
        assert!(!settled.is_animating);

        // Hovering again must pick a different hover face than the previous hover.
        let hover_2 = step_live_avatar_for_test(&spec, Some(true), 12.1, 12_100);
        assert!(
            hover_2.plush_state != hover_1.plush_state || hover_2.expression != hover_1.expression,
            "consecutive hovers must not repeat the same hover face"
        );
    }

    #[test]
    fn live_avatar_hover_group_propagates_to_member_avatars() {
        let group_key = "test-group-hover-cluster-42";
        assert!(!is_live_avatar_group_hovered(group_key));
        assert!(set_live_avatar_group_hover(group_key, true));
        assert!(is_live_avatar_group_hovered(group_key));

        let props_a = AgentAvatarProps::new("cluster-member-a").with_allow_hover_face(true);
        let props_b = AgentAvatarProps::new("cluster-member-b").with_allow_hover_face(true);
        let spec_a =
            LiveAvatarSpec::new("test-cluster-a", props_a).with_hover_group(group_key);
        let spec_b =
            LiveAvatarSpec::new("test-cluster-b", props_b).with_hover_group(group_key);

        let rep_a = step_live_avatar_for_test(&spec_a, Some(false), 20.0, 20_000);
        let rep_b = step_live_avatar_for_test(&spec_b, Some(false), 20.0, 20_000);
        assert!(rep_a.interacting && rep_a.is_animating);
        assert!(rep_b.interacting && rep_b.is_animating);

        assert!(set_live_avatar_group_hover(group_key, false));
        assert!(!is_live_avatar_group_hovered(group_key));
    }

    #[test]
    fn live_avatar_working_modes_cover_thinking_dots_working_faces_and_activity_hold() {
        // 1. Generic working (stage): PlushState::Thinking with 3 dots + up-right gaze
        let stage_props = AgentAvatarProps::new("work-stage-bot")
            .with_mood(AvatarMood::Working)
            .with_working_faces(false);
        let stage_spec = LiveAvatarSpec::new("test-work-stage", stage_props);
        let thinking = step_live_avatar_for_test(&stage_spec, Some(false), 30.0, 30_000);
        assert!(thinking.is_animating);
        assert_eq!(thinking.plush_state, PlushState::Thinking);
        assert_eq!(thinking.busy_dots, 3);
        assert!(thinking.gaze_yaw > 10.0 && thinking.gaze_pitch > 10.0);

        // 2. Sidebar working (`working_faces = true`): cycles expressions over time
        let sb_props = AgentAvatarProps::new("work-sidebar-bot")
            .with_mood(AvatarMood::Working)
            .with_working_faces(true);
        let sb_spec = LiveAvatarSpec::new("test-work-sidebar", sb_props);
        let t0 = step_live_avatar_for_test(&sb_spec, Some(false), 40.0, 40_000);
        let t1 = step_live_avatar_for_test(&sb_spec, Some(false), 44.0, 44_000);
        assert!(t0.is_animating && t1.is_animating);
        assert_eq!(t0.plush_state, PlushState::Idle);
        assert_ne!(t0.expression, t1.expression);

        // 3. Tool activity with 1400ms hold
        let mut act_props = AgentAvatarProps::new("work-tool-bot")
            .with_mood(AvatarMood::Working)
            .with_activity(Some(AgentStepActivity::Searching));
        let act_spec_1 = LiveAvatarSpec::new("test-work-tool", act_props.clone());
        let a1 = step_live_avatar_for_test(&act_spec_1, Some(false), 50.0, 50_000);
        assert_eq!(a1.expression, BloubExpression::Curieux);

        act_props.activity = Some(AgentStepActivity::Running);
        let act_spec_2 = LiveAvatarSpec::new("test-work-tool", act_props);
        let a2_held = step_live_avatar_for_test(&act_spec_2, Some(false), 50.5, 50_500);
        assert_eq!(a2_held.expression, BloubExpression::Curieux);
        let a3_switched = step_live_avatar_for_test(&act_spec_2, Some(false), 51.6, 51_600);
        assert_eq!(a3_switched.expression, BloubExpression::Excite);
    }

    #[test]
    fn live_avatar_failed_waiting_asleep_and_reduced_motion_match_openbot() {
        // Failed head shake
        let failed_props = AgentAvatarProps::new("mood-failed-bot").with_mood(AvatarMood::Failed);
        let failed_spec = LiveAvatarSpec::new("test-mood-failed", failed_props);
        let _enter = step_live_avatar_for_test(&failed_spec, Some(false), 60.0, 60_000);
        let mid_shake = step_live_avatar_for_test(&failed_spec, Some(false), 60.15, 60_150);
        assert_eq!(mid_shake.plush_state, PlushState::Failed);
        assert!(mid_shake.shake_deg.abs() > 0.1);

        // Waiting notify ball
        let wait_props = AgentAvatarProps::new("mood-wait-bot").with_mood(AvatarMood::Waiting);
        let wait_spec = LiveAvatarSpec::new("test-mood-wait", wait_props);
        let wait_rep = step_live_avatar_for_test(&wait_spec, Some(false), 70.0, 70_000);
        assert_eq!(wait_rep.plush_state, PlushState::Notify);
        assert!(wait_rep.has_notif_ball);

        // Asleep Zs
        let sleep_props = AgentAvatarProps::new("mood-sleep-bot").with_mood(AvatarMood::Asleep);
        let sleep_spec = LiveAvatarSpec::new("test-mood-sleep", sleep_props);
        let sleep_rep = step_live_avatar_for_test(&sleep_spec, Some(false), 80.0, 80_000);
        assert_eq!(sleep_rep.plush_state, PlushState::Asleep);
        assert!(sleep_rep.sleep_zs > 0);

        // Reduced motion freezes hover & working
        let mut rm_props = AgentAvatarProps::new("mood-rm-bot")
            .with_mood(AvatarMood::Working)
            .with_allow_hover_face(true);
        rm_props.reduced_motion = true;
        let rm_spec = LiveAvatarSpec::new("test-mood-rm", rm_props);
        let rm_rep = step_live_avatar_for_test(&rm_spec, Some(true), 90.0, 90_000);
        assert!(!rm_rep.is_animating);
        assert_eq!(rm_rep.resolved.fps, 0);
    }
}
