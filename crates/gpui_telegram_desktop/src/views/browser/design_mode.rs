//! Native GPUI `0.2.2` Browser Design Mode overlay, coordinate projectors,
//! headless-testable [`DesignModePanelState`], and Design Mode inspector panel (`F18` / `F22`).

use std::sync::Arc;

use bot_browser::design_mode::{
    DesignDiffMode, DesignHoverTarget, DesignModeCapture, DesignModeChatHandoff, DesignModeError,
    DesignModeSpec, DesignOverlaySpec, DesignPin, DesignViewport,
};
use bot_browser::ElementRect;
use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::browser::recorder_panel::{
    browser_input_box, panel_action_btn, panel_section_card,
};
use crate::views::browser::workspace::{ActiveBrowserInput, BrowserWorkspace};
use crate::views::tdesktop_theme::tr_str;

/// Destination for a Design Mode element capture handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignHandoffTarget {
    ActiveChat,
    OrcaBot { bot_id: Option<String> },
    BrowserCopilot,
}

/// Typed event emitted by [`BrowserWorkspace`] for shell-level Design Mode handoff routing.
#[derive(Debug, Clone)]
pub enum BrowserWorkspaceEvent {
    SendDesignCaptureToChat {
        target: DesignHandoffTarget,
        handoff: DesignModeChatHandoff,
    },
}

/// Projected element rectangle in overlay-local pixels relative to `browser-cdp-viewport-host`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayBoxPx {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Computes the `(img_x, img_y, draw_w, draw_h)` letterboxed image rectangle inside a
/// container of size `(bounds_w, bounds_h)` laid out with `ObjectFit::Contain`.
/// Mirrors `cdp_surface::map_point_contain` exactly.
pub fn contain_draw_rect(
    bounds_w: f32,
    bounds_h: f32,
    image_size: (u32, u32),
) -> (f32, f32, f32, f32) {
    let bw = if bounds_w.is_finite() {
        bounds_w.max(1.0)
    } else {
        1.0
    };
    let bh = if bounds_h.is_finite() {
        bounds_h.max(1.0)
    } else {
        1.0
    };
    let img_w = (image_size.0.max(1)) as f32;
    let img_h = (image_size.1.max(1)) as f32;
    let img_aspect = img_w / img_h;
    let vp_aspect = bw / bh;
    if vp_aspect > img_aspect {
        let w = bh * img_aspect;
        ((bw - w) * 0.5, 0.0, w, bh)
    } else {
        let h = bw / img_aspect;
        (0.0, (bh - h) * 0.5, bw, h)
    }
}

/// Projects a CSS-pixel [`ElementRect`] into overlay-local pixel coordinates matching
/// `CdpViewportSurface`'s `ObjectFit::Contain` layout (exact inverse of `map_point_contain`).
pub fn project_css_rect_to_overlay_px(
    rect: &ElementRect,
    bounds: Bounds<Pixels>,
    image_size: (u32, u32),
    viewport_size: (u32, u32),
) -> Option<OverlayBoxPx> {
    if !rect.x.is_finite()
        || !rect.y.is_finite()
        || !rect.width.is_finite()
        || !rect.height.is_finite()
        || rect.width <= 0.0
        || rect.height <= 0.0
    {
        return None;
    }
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    if !bw.is_finite() || !bh.is_finite() || bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let (img_x, img_y, draw_w, draw_h) = contain_draw_rect(bw, bh, image_size);
    let vp_w = f64::from(viewport_size.0.max(1));
    let vp_h = f64::from(viewport_size.1.max(1));

    let x0 = (rect.x / vp_w).clamp(0.0, 1.0) as f32;
    let y0 = (rect.y / vp_h).clamp(0.0, 1.0) as f32;
    let x1 = ((rect.x + rect.width) / vp_w).clamp(0.0, 1.0) as f32;
    let y1 = ((rect.y + rect.height) / vp_h).clamp(0.0, 1.0) as f32;

    let left = img_x + x0 * draw_w;
    let top = img_y + y0 * draw_h;
    let width = ((x1 - x0) * draw_w).max(2.0);
    let height = ((y1 - y0) * draw_h).max(2.0);
    Some(OverlayBoxPx {
        left,
        top,
        width,
        height,
    })
}

/// Projects normalized basis-point coordinates (`0..=10_000`) of a [`DesignPin`] into
/// overlay-local `(x_px, y_px)` inside `bounds`.
pub fn project_norm_bp_to_overlay_px(
    x_norm_bp: u16,
    y_norm_bp: u16,
    bounds: Bounds<Pixels>,
    image_size: (u32, u32),
) -> (f32, f32) {
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    let (img_x, img_y, draw_w, draw_h) = contain_draw_rect(bw, bh, image_size);
    let nx = (f32::from(x_norm_bp.min(10_000))) / 10_000.0;
    let ny = (f32::from(y_norm_bp.min(10_000))) / 10_000.0;
    (img_x + nx * draw_w, img_y + ny * draw_h)
}

/// Headless-testable state model for Browser Design Mode owned by [`BrowserWorkspace`].
pub struct DesignModePanelState {
    pub enabled: bool,
    pub picker_active: bool,
    pub diff_mode: DesignDiffMode,
    pub opacity_percent: u8,
    pub show_spacing_guides: bool,
    pub hover_target: Option<DesignHoverTarget>,
    pub selected_capture: Option<DesignModeCapture>,
    pub preview_image: Option<Arc<gpui::RenderImage>>,
    pub preview_dimensions: Option<(u32, u32)>,
    pub pins: Vec<DesignPin>,
    pub note_draft: String,
    pub selector_query_draft: String,
    pub is_capturing: bool,
    pub last_error: Option<String>,
    pub last_status: Option<String>,
    pub last_handoff: Option<DesignModeChatHandoff>,
    pub(crate) hover_probe_generation: u64,
    pub(crate) capture_generation: u64,
    pub(crate) last_hover_css_point: Option<(f64, f64)>,
    next_pin_seq: u64,
}

impl Default for DesignModePanelState {
    fn default() -> Self {
        Self {
            enabled: false,
            picker_active: false,
            diff_mode: DesignDiffMode::Overlay,
            opacity_percent: 85,
            show_spacing_guides: true,
            hover_target: None,
            selected_capture: None,
            preview_image: None,
            preview_dimensions: None,
            pins: Vec::new(),
            note_draft: String::new(),
            selector_query_draft: String::new(),
            is_capturing: false,
            last_error: None,
            last_status: None,
            last_handoff: None,
            hover_probe_generation: 0,
            capture_generation: 0,
            last_hover_css_point: None,
            next_pin_seq: 1,
        }
    }
}

impl DesignModePanelState {
    /// Toggles Design Mode on or off. Enabling activates the element picker; disabling
    /// turns off picking and clears hover state.
    pub fn toggle_enabled(&mut self) -> bool {
        self.enabled = !self.enabled;
        if self.enabled {
            self.picker_active = true;
            self.last_error = None;
        } else {
            self.picker_active = false;
            self.hover_target = None;
            self.last_hover_css_point = None;
        }
        self.enabled
    }

    pub fn set_diff_mode(&mut self, mode: DesignDiffMode) {
        self.diff_mode = mode;
    }

    pub fn set_opacity_percent(&mut self, opacity: u8) {
        self.opacity_percent = opacity.min(100);
    }

    /// Applies a captured element into panel state (headless-testable without GPUI window).
    pub fn apply_capture(&mut self, capture: DesignModeCapture) {
        self.selector_query_draft = capture.selector.clone();
        self.preview_dimensions = capture.cropped_png_dimensions;
        self.selected_capture = Some(capture);
        self.is_capturing = false;
        self.last_error = None;
    }

    /// Clears the selected capture and hover state, returning any previous `RenderImage`
    /// so the caller can safely drop it via `cx.drop_image(prev, None)`.
    pub fn clear_selection(&mut self) -> Option<Arc<gpui::RenderImage>> {
        self.hover_target = None;
        self.selected_capture = None;
        self.preview_dimensions = None;
        self.last_hover_css_point = None;
        self.last_error = None;
        self.last_status = None;
        self.preview_image.take()
    }

    /// Adds a normalized [`DesignPin`] anchored to the currently selected capture's center.
    pub fn add_pin_from_selected(&mut self) -> Result<DesignPin, String> {
        let capture = self
            .selected_capture
            .as_ref()
            .ok_or_else(|| "Select or capture an element before placing a pin".to_string())?;
        let (x_norm_bp, y_norm_bp) = capture.normalized_center_bp();
        if x_norm_bp > 10_000 || y_norm_bp > 10_000 {
            return Err("Pin coordinates out of normalized bounds".to_string());
        }
        let pin_id = format!("pin-{}", self.next_pin_seq);
        self.next_pin_seq = self.next_pin_seq.saturating_add(1);
        let note_text = self.note_draft.trim();
        let note = if note_text.is_empty() {
            format!("Review {}", capture.selector)
        } else {
            note_text.to_string()
        };
        let pin = DesignPin {
            pin_id,
            x_norm_bp,
            y_norm_bp,
            selector: capture.selector.clone(),
            note,
            resolved: false,
        };
        self.pins.push(pin.clone());
        self.last_error = None;
        Ok(pin)
    }

    pub fn toggle_pin_resolved(&mut self, pin_id: &str) -> bool {
        if let Some(pin) = self.pins.iter_mut().find(|p| p.pin_id == pin_id) {
            pin.resolved = !pin.resolved;
            true
        } else {
            false
        }
    }

    pub fn remove_pin(&mut self, pin_id: &str) -> bool {
        let before = self.pins.len();
        self.pins.retain(|p| p.pin_id != pin_id);
        self.pins.len() != before
    }

    pub fn to_overlay_spec(&self) -> DesignOverlaySpec {
        DesignOverlaySpec {
            enabled: self.enabled,
            diff_mode: self.diff_mode,
            opacity_percent: self.opacity_percent.min(100),
            show_spacing_guides: self.show_spacing_guides,
            pins: self.pins.clone(),
        }
    }

    pub fn to_mode_spec(
        &self,
        workspace_id: impl Into<String>,
        fallback_viewport: (u32, u32),
    ) -> Result<DesignModeSpec, DesignModeError> {
        let overlay = self.to_overlay_spec();
        let spec = if let Some(ref capture) = self.selected_capture {
            capture.into_mode_spec(workspace_id, overlay)
        } else {
            DesignModeSpec {
                workspace_id: workspace_id.into(),
                viewport: DesignViewport {
                    width_px: fallback_viewport.0,
                    height_px: fallback_viewport.1,
                    device_scale_factor_x100: 100,
                    is_mobile: fallback_viewport.0 <= 640,
                },
                overlay,
                last_capture: None,
            }
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn build_handoff_payload(&self) -> Result<DesignModeChatHandoff, String> {
        let capture = self
            .selected_capture
            .as_ref()
            .ok_or_else(|| "No Design Mode element captured yet".to_string())?;
        let note = {
            let trimmed = self.note_draft.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };
        Ok(capture.to_chat_handoff(note, &self.pins))
    }
}

fn diff_mode_label(mode: DesignDiffMode, lang: &str) -> &'static str {
    match mode {
        DesignDiffMode::Overlay => tr_str(lang, "叠层", "Overlay"),
        DesignDiffMode::SplitSwipe => tr_str(lang, "分屏", "Split"),
        DesignDiffMode::OnionSkin => tr_str(lang, "洋葱皮", "Onion"),
        DesignDiffMode::SideBySide => tr_str(lang, "并排", "Side"),
    }
}

fn format_hover_badge(hover: &DesignHoverTarget) -> String {
    let mut label = hover.tag_name.clone();
    if let Some(ref id) = hover.id_attr
        && !id.is_empty()
    {
        label.push('#');
        label.push_str(id);
    }
    for cls in hover.classes.iter().take(2) {
        if !cls.is_empty() {
            label.push('.');
            label.push_str(cls);
        }
    }
    format!(
        "{label} · {}×{}",
        hover.bounding_box.width.round() as i64,
        hover.bounding_box.height.round() as i64
    )
}

/// Renders the interactive Design Mode overlay mounted inside `browser-cdp-viewport-host`
/// above `CdpViewportSurface`.
pub fn render_design_mode_overlay(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let dm = &ws.design_mode;
    let geometry = ws.cdp_surface_geometry(cx);
    let overlay_alpha = (f32::from(dm.opacity_percent) / 100.0).clamp(0.05, 1.0);

    let hover_box = geometry.and_then(|(bounds, img_sz, vp_sz)| {
        dm.hover_target.as_ref().and_then(|h| {
            project_css_rect_to_overlay_px(&h.bounding_box, bounds, img_sz, vp_sz)
                .map(|b| (b, format_hover_badge(h)))
        })
    });

    let selected_box = geometry.and_then(|(bounds, img_sz, vp_sz)| {
        dm.selected_capture.as_ref().and_then(|cap| {
            project_css_rect_to_overlay_px(&cap.bounding_box, bounds, img_sz, vp_sz).map(|b| {
                let guide = if let Some(ref bm) = cap.box_model {
                    format!(
                        "M {:.0}/{:.0}/{:.0}/{:.0} · P {:.0}/{:.0}/{:.0}/{:.0}",
                        bm.margin.top,
                        bm.margin.right,
                        bm.margin.bottom,
                        bm.margin.left,
                        bm.padding.top,
                        bm.padding.right,
                        bm.padding.bottom,
                        bm.padding.left,
                    )
                } else {
                    format!(
                        "{}×{} px",
                        cap.bounding_box.width.round() as i64,
                        cap.bounding_box.height.round() as i64
                    )
                };
                (b, cap.selector.clone(), guide)
            })
        })
    });

    let projected_pins: Vec<(usize, String, f32, f32, bool)> = geometry
        .map(|(bounds, img_sz, _)| {
            dm.pins
                .iter()
                .enumerate()
                .map(|(idx, pin)| {
                    let (px_x, px_y) =
                        project_norm_bp_to_overlay_px(pin.x_norm_bp, pin.y_norm_bp, bounds, img_sz);
                    (idx + 1, pin.pin_id.clone(), px_x, px_y, pin.resolved)
                })
                .collect()
        })
        .unwrap_or_default();

    let picker_active = dm.picker_active;
    let mode_text = diff_mode_label(dm.diff_mode, lang);
    let status_pill_label = format!(
        "{} · {} · {} · {}%",
        tr_str(lang, "设计模式", "Design Mode"),
        if picker_active {
            tr_str(lang, "拾取元素", "Pick Element")
        } else {
            tr_str(lang, "检查中", "Inspecting")
        },
        mode_text,
        dm.opacity_percent
    );

    let mut overlay = div()
        .id("browser-design-mode-overlay")
        .absolute()
        .inset_0()
        .overflow_hidden()
        .when(picker_active, |el| {
            el.cursor_pointer()
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    this.probe_design_hover_at_window_point(ev.position, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        this.capture_design_element_at_window_point(ev.position, cx);
                        cx.stop_propagation();
                    }),
                )
        });

    if let Some((hbox, badge_text)) = hover_box {
        let label_top = if hbox.top >= 24.0 {
            hbox.top - 22.0
        } else {
            hbox.top + 2.0
        };
        overlay = overlay.child(
            div()
                .id("design-overlay-hover-box")
                .absolute()
                .left(px(hbox.left))
                .top(px(hbox.top))
                .w(px(hbox.width))
                .h(px(hbox.height))
                .border_2()
                .border_color(rgb(pal.accent))
                .bg(TdesktopThemePalette::rgba_with_alpha(
                    pal.accent,
                    (0.16 * overlay_alpha).clamp(0.05, 0.30),
                )),
        );
        overlay = overlay.child(
            div()
                .id("design-overlay-hover-badge")
                .absolute()
                .left(px(hbox.left))
                .top(px(label_top))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(rgb(pal.accent))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .child(badge_text),
        );
    }

    if let Some((sbox, selector_label, guide_label)) = selected_box {
        overlay = overlay.child(
            div()
                .id("design-overlay-selected-box")
                .absolute()
                .left(px(sbox.left))
                .top(px(sbox.top))
                .w(px(sbox.width))
                .h(px(sbox.height))
                .border_2()
                .border_color(rgb(pal.success))
                .bg(TdesktopThemePalette::rgba_with_alpha(
                    pal.success,
                    (0.12 * overlay_alpha).clamp(0.04, 0.24),
                )),
        );
        if dm.show_spacing_guides {
            let guide_top = (sbox.top + sbox.height + 3.0).max(0.0);
            overlay = overlay.child(
                div()
                    .id("design-overlay-spacing-guide")
                    .absolute()
                    .left(px(sbox.left))
                    .top(px(guide_top))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(rgb(pal.success))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(0xFFFFFF))
                    .child(format!("{selector_label} · {guide_label}")),
            );
        }
    }

    for (num, pin_id, px_x, px_y, resolved) in projected_pins {
        let pin_bg = if resolved { pal.text_secondary } else { pal.accent };
        overlay = overlay.child(
            div()
                .id(SharedString::from(format!("design-overlay-pin-{pin_id}")))
                .absolute()
                .left(px((px_x - 11.0).max(0.0)))
                .top(px((px_y - 11.0).max(0.0)))
                .w(px(22.0))
                .h(px(22.0))
                .rounded_full()
                .bg(rgb(pin_bg))
                .border_1()
                .border_color(rgb(0xFFFFFF))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(format!("{num}")),
        );
    }

    // Top-left floating mode status pill + Exit Design Mode button
    overlay = overlay.child(
        div()
            .id("design-overlay-status-pill")
            .absolute()
            .left(px(10.0))
            .top(px(10.0))
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(pal.modal_bg, 0.92))
            .border_1()
            .border_color(rgb(pal.accent))
            .shadow_md()
            .flex()
            .items_center()
            .gap(px(8.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(tg_icon(IconName::Camera, 13.0, pal.accent))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(status_pill_label),
            )
            .child(
                div()
                    .id("design-overlay-exit-btn")
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(rgb(pal.input_bg))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(pal.text_primary))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_design_mode_enabled(false, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(tr_str(lang, "退出", "Exit")),
            ),
    );

    overlay.into_any_element()
}

fn render_design_controls_card(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let dm = &ws.design_mode;
    let picker_label = if dm.enabled && dm.picker_active {
        tr_str(lang, "拾取中…", "Picking…")
    } else {
        tr_str(lang, "拾取元素", "Pick Element")
    };
    let picker_bg = if dm.enabled && dm.picker_active {
        pal.accent
    } else {
        pal.input_bg
    };
    let picker_fg = if dm.enabled && dm.picker_active {
        pal.active_button_fg
    } else {
        pal.text_primary
    };

    let diff_modes = [
        DesignDiffMode::Overlay,
        DesignDiffMode::SplitSwipe,
        DesignDiffMode::OnionSkin,
        DesignDiffMode::SideBySide,
    ];

    let mut card = panel_section_card(
        tr_str(lang, "设计模式与叠层控制", "Design Mode & Overlay"),
        IconName::Camera,
        pal,
    )
    .child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(panel_action_btn(
                "design-toggle-picker-btn",
                picker_label,
                picker_bg,
                picker_fg,
                |this, cx| {
                    if !this.design_mode.enabled {
                        this.set_design_mode_enabled(true, cx);
                    } else {
                        let next = !this.design_mode.picker_active;
                        this.set_design_picker_active(next, cx);
                    }
                },
                cx,
            ))
            .child(panel_action_btn(
                "design-toggle-guides-btn",
                if dm.show_spacing_guides {
                    tr_str(lang, "间距线: 开", "Guides: On")
                } else {
                    tr_str(lang, "间距线: 关", "Guides: Off")
                },
                if dm.show_spacing_guides {
                    pal.accent
                } else {
                    pal.input_bg
                },
                if dm.show_spacing_guides {
                    pal.active_button_fg
                } else {
                    pal.text_primary
                },
                |this, cx| {
                    this.design_mode.show_spacing_guides = !this.design_mode.show_spacing_guides;
                    cx.notify();
                },
                cx,
            ))
            .child(panel_action_btn(
                "design-clear-selection-btn",
                tr_str(lang, "清空", "Clear"),
                pal.input_bg,
                pal.text_primary,
                |this, cx| {
                    this.clear_design_selection(cx);
                },
                cx,
            )),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div().flex_1().min_w(px(0.0)).child(browser_input_box(
                    "design-selector-query-input",
                    &dm.selector_query_draft,
                    tr_str(lang, "CSS 选择器 (如 button.primary)", "CSS selector (e.g. button.primary)"),
                    ActiveBrowserInput::DesignSelectorQuery,
                    &ws.active_input,
                    pal,
                    false,
                    cx,
                )),
            )
            .child(panel_action_btn(
                "design-capture-selector-btn",
                if dm.is_capturing {
                    tr_str(lang, "捕获中…", "Capturing…")
                } else {
                    tr_str(lang, "捕获", "Capture")
                },
                pal.accent,
                pal.active_button_fg,
                |this, cx| {
                    let sel = this.design_mode.selector_query_draft.clone();
                    this.capture_design_element_by_selector(sel, cx);
                },
                cx,
            )),
    )
    .child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .children(diff_modes.into_iter().map(|mode| {
                let active = dm.diff_mode == mode;
                panel_action_btn(
                    SharedString::from(format!("design-diff-mode-{mode:?}")),
                    diff_mode_label(mode, lang),
                    if active { pal.accent } else { pal.input_bg },
                    if active {
                        pal.active_button_fg
                    } else {
                        pal.text_secondary
                    },
                    move |this, cx| {
                        this.design_mode.set_diff_mode(mode);
                        cx.notify();
                    },
                    cx,
                )
            })),
    )
    .child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "{}: {}%",
                        tr_str(lang, "不透明度", "Opacity"),
                        dm.opacity_percent
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(panel_action_btn(
                        "design-opacity-dec-btn",
                        "-10%",
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| {
                            let next = this.design_mode.opacity_percent.saturating_sub(10);
                            this.design_mode.set_opacity_percent(next);
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(panel_action_btn(
                        "design-opacity-inc-btn",
                        "+10%",
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| {
                            let next = this.design_mode.opacity_percent.saturating_add(10).min(100);
                            this.design_mode.set_opacity_percent(next);
                            cx.notify();
                        },
                        cx,
                    )),
            ),
    );

    if let Some(ref err) = dm.last_error {
        card = card.child(
            div()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                .text_xs()
                .text_color(rgb(pal.danger))
                .child(err.clone()),
        );
    } else if let Some(ref status) = dm.last_status {
        card = card.child(
            div()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                .text_xs()
                .text_color(rgb(pal.success))
                .child(status.clone()),
        );
    }

    card.into_any_element()
}

fn render_design_preview_and_sourcemap(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> Vec<AnyElement> {
    let dm = &ws.design_mode;
    let mut cards = Vec::new();

    // 2. Cropped Element Preview card
    let mut preview_card = panel_section_card(
        tr_str(lang, "元素裁切预览", "Cropped Element Preview"),
        IconName::Camera,
        pal,
    );
    if let Some(ref preview) = dm.preview_image {
        let dims = dm
            .preview_dimensions
            .map(|(w, h)| format!("{w}×{h} px"))
            .unwrap_or_else(|| "PNG".to_string());
        let sha_prefix = dm
            .selected_capture
            .as_ref()
            .and_then(|c| c.contract.screenshot_sha256.as_deref())
            .map(|s| &s[..s.len().min(12)])
            .unwrap_or("—");
        preview_card = preview_card
            .child(
                div()
                    .w_full()
                    .p(px(6.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.history_bg))
                    .border_1()
                    .border_color(rgb(pal.border_fg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(preview.clone())
                            .w_full()
                            .max_h(px(150.0))
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(dims)
                    .child(format!("sha256: {sha_prefix}")),
            );
    } else {
        preview_card = preview_card.child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(
                    lang,
                    "点击页面元素或输入 CSS 选择器以捕获裁切截图、DOM 与样式",
                    "Click a page element or enter a CSS selector to capture cropped PNG, DOM & styles.",
                )),
        );
    }
    cards.push(preview_card.into_any_element());

    // 3. Source Map & Component card
    if let Some(ref cap) = dm.selected_capture {
        let mut sm_card = panel_section_card(
            tr_str(lang, "源码映射与组件", "Source Map & Component"),
            IconName::FileDoc,
            pal,
        );
        if let Some(ref sm_ref) = cap.contract.source_map {
            let loc = match sm_ref.column {
                Some(col) => format!("{}:{}:{}", sm_ref.file_path, sm_ref.line, col),
                None => format!("{}:{}", sm_ref.file_path, sm_ref.line),
            };
            let loc_copy = loc.clone();
            let fw_badge = cap
                .source_map_hint
                .as_ref()
                .map(|r| r.framework.as_str())
                .unwrap_or("source_map");
            sm_card = sm_card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .child(fw_badge.to_string()),
                        )
                        .child(panel_action_btn(
                            "design-copy-sourcemap-btn",
                            tr_str(lang, "复制位置", "Copy"),
                            pal.input_bg,
                            pal.text_primary,
                            move |this, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(loc_copy.clone()));
                                this.design_mode.last_status =
                                    Some(format!("Copied source location: {loc_copy}"));
                                cx.notify();
                            },
                            cx,
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .child(loc),
                );
            if let Some(ref comp) = sm_ref.component_name {
                sm_card = sm_card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("<{comp} />")),
                );
            }
        } else {
            sm_card = sm_card.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "当前元素未检测到 React/Vue/Svelte/SourceMap 源码元数据",
                        "No React/Vue/Svelte/SourceMap metadata detected for this element.",
                    )),
            );
        }
        cards.push(sm_card.into_any_element());

        // 4. DOM Hierarchy & Outer HTML card
        let breadcrumb = cap.dom_breadcrumb();
        let html_preview = if cap.contract.outer_html.len() > 720 {
            let mut cut = 720;
            while cut > 0 && !cap.contract.outer_html.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}…", &cap.contract.outer_html[..cut])
        } else {
            cap.contract.outer_html.clone()
        };
        let mut dom_card = panel_section_card(
            tr_str(lang, "DOM 层级与 HTML", "DOM Hierarchy & Outer HTML"),
            IconName::Search,
            pal,
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.accent))
                .child(breadcrumb),
        )
        .child(
            div()
                .p(px(7.0))
                .rounded(px(6.0))
                .bg(rgb(pal.code_bg))
                .text_xs()
                .text_color(rgb(pal.code_fg))
                .child(html_preview),
        );
        if cap.redacted_sensitive_content {
            dom_card = dom_card.child(
                div()
                    .px(px(6.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                    .text_xs()
                    .text_color(rgb(pal.danger))
                    .child(tr_str(
                        lang,
                        "已自动脱敏敏感表单值 ([REDACTED])",
                        "Sensitive input values automatically redacted ([REDACTED]).",
                    )),
            );
        }
        cards.push(dom_card.into_any_element());
    }

    cards
}

fn render_design_styles_pins_and_handoff(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> Vec<AnyElement> {
    let dm = &ws.design_mode;
    let mut cards = Vec::new();

    // 5. Computed CSS Properties card
    if let Some(ref cap) = dm.selected_capture {
        let total_props = cap.contract.computed_styles.len();
        let mut css_card = panel_section_card(
            format!(
                "{} ({total_props})",
                tr_str(lang, "计算 CSS 属性", "Computed CSS Properties")
            ),
            IconName::Edit,
            pal,
        );
        if total_props == 0 {
            css_card = css_card.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "无可用计算样式", "No computed styles captured.")),
            );
        } else {
            let mut rows = div().flex().flex_col().gap(px(3.0));
            for (k, v) in cap.contract.computed_styles.iter().take(36) {
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .px(px(6.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text_secondary))
                                .child(k.clone()),
                        )
                        .child(
                            div()
                                .text_color(rgb(pal.text_primary))
                                .child(v.clone()),
                        ),
                );
            }
            if total_props > 36 {
                rows = rows.child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("+{} more properties", total_props - 36)),
                );
            }
            css_card = css_card.child(rows);
        }
        cards.push(css_card.into_any_element());
    }

    // 6. Pins & Note card
    let mut pins_card = panel_section_card(
        format!(
            "{} ({})",
            tr_str(lang, "标注与说明", "Pins & Note"),
            dm.pins.len()
        ),
        IconName::Pin,
        pal,
    )
    .child(browser_input_box(
        "design-note-draft-input",
        &dm.note_draft,
        tr_str(
            lang,
            "填写设计评审意见或修改说明…",
            "Write design review note or requested visual tweak…",
        ),
        ActiveBrowserInput::DesignNote,
        &ws.active_input,
        pal,
        true,
        cx,
    ))
    .child(
        div().flex().items_center().gap(px(6.0)).child(panel_action_btn(
            "design-add-pin-btn",
            tr_str(lang, "添加标注", "Add Pin"),
            pal.accent,
            pal.active_button_fg,
            |this, cx| {
                match this.design_mode.add_pin_from_selected() {
                    Ok(pin) => {
                        this.design_mode.last_status =
                            Some(format!("Added pin {} on {}", pin.pin_id, pin.selector));
                    }
                    Err(e) => {
                        this.design_mode.last_error = Some(e);
                    }
                }
                cx.notify();
            },
            cx,
        )),
    );

    if !dm.pins.is_empty() {
        let mut pin_list = div().flex().flex_col().gap(px(5.0));
        for (idx, pin) in dm.pins.iter().enumerate() {
            let pid_toggle = pin.pin_id.clone();
            let pid_remove = pin.pin_id.clone();
            let resolved = pin.resolved;
            pin_list = pin_list.child(
                div()
                    .p(px(7.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(rgb(if resolved { pal.success } else { pal.border_fg }))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!("#{} · {}", idx + 1, pin.selector)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(panel_action_btn(
                                        SharedString::from(format!("design-pin-resolve-{}", pin.pin_id)),
                                        if resolved {
                                            tr_str(lang, "重开", "Reopen")
                                        } else {
                                            tr_str(lang, "已解决", "Resolve")
                                        },
                                        if resolved { pal.card_bg } else { pal.success },
                                        if resolved { pal.text_primary } else { 0xFFFFFF },
                                        move |this, cx| {
                                            this.design_mode.toggle_pin_resolved(&pid_toggle);
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(panel_action_btn(
                                        SharedString::from(format!("design-pin-remove-{}", pin.pin_id)),
                                        tr_str(lang, "删除", "Delete"),
                                        pal.card_bg,
                                        pal.danger,
                                        move |this, cx| {
                                            this.design_mode.remove_pin(&pid_remove);
                                            cx.notify();
                                        },
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(pin.note.clone()),
                    ),
            );
        }
        pins_card = pins_card.child(pin_list);
    }
    cards.push(pins_card.into_any_element());

    // 7. Send to Chat / Bot card
    let handoff_card = panel_section_card(
        tr_str(lang, "发送到聊天或机器人", "Send to Chat / Bot"),
        IconName::SendPlane,
        pal,
    )
    .child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(panel_action_btn(
                "design-send-active-chat-btn",
                tr_str(lang, "发送到当前聊天", "Send to Active Chat"),
                pal.accent,
                pal.active_button_fg,
                |this, cx| {
                    if let Err(err) =
                        this.send_design_capture_to_target(DesignHandoffTarget::ActiveChat, cx)
                    {
                        this.design_mode.last_error = Some(err);
                        cx.notify();
                    }
                },
                cx,
            ))
            .child(panel_action_btn(
                "design-send-orca-bot-btn",
                tr_str(lang, "发送到 Orca 机器人", "Send to Orca Bot"),
                pal.success,
                0xFFFFFF,
                |this, cx| {
                    if let Err(err) = this.send_design_capture_to_target(
                        DesignHandoffTarget::OrcaBot { bot_id: None },
                        cx,
                    ) {
                        this.design_mode.last_error = Some(err);
                        cx.notify();
                    }
                },
                cx,
            ))
            .child(panel_action_btn(
                "design-send-copilot-btn",
                tr_str(lang, "发送到 Copilot", "Send to Copilot"),
                pal.input_bg,
                pal.text_primary,
                |this, cx| {
                    if let Err(err) =
                        this.send_design_capture_to_target(DesignHandoffTarget::BrowserCopilot, cx)
                    {
                        this.design_mode.last_error = Some(err);
                        cx.notify();
                    }
                },
                cx,
            ))
            .child(panel_action_btn(
                "design-copy-prompt-btn",
                tr_str(lang, "复制提示词", "Copy Prompt"),
                pal.input_bg,
                pal.text_primary,
                |this, cx| match this.design_mode.build_handoff_payload() {
                    Ok(handoff) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(handoff.prompt_markdown));
                        this.design_mode.last_status =
                            Some("Copied Design Mode handoff prompt to clipboard".to_string());
                        this.design_mode.last_error = None;
                        cx.notify();
                    }
                    Err(err) => {
                        this.design_mode.last_error = Some(err);
                        cx.notify();
                    }
                },
                cx,
            )),
    );
    cards.push(handoff_card.into_any_element());

    cards
}

/// Renders the scrollable `Design` inspector tab inside `BrowserWorkspace`.
pub fn render_design_mode_tab(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let mut col = div()
        .id("browser-design-mode-tab-scroll")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(render_design_controls_card(ws, pal, lang, cx));

    for card in render_design_preview_and_sourcemap(ws, pal, lang, cx) {
        col = col.child(card);
    }
    for card in render_design_styles_pins_and_handoff(ws, pal, lang, cx) {
        col = col.child(card);
    }

    col.into_any_element()
}
