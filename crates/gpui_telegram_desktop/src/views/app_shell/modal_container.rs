//! AppShell Modal Container: Orchestrates modal layer stack, backdrop effects,
//! and real interactive GPUI dialog boxes (`ActiveModalDialog`).
//!
//! Upstream: `Telegram/SourceFiles/ui/layers/standalone_layer_stack.cpp:L88-L172`
//! Upstream: `Telegram/SourceFiles/ui/effects/layer_backdrop.cpp:L210-L286`

use crate::components::ui::effects::LayerBackdropEffects;
use crate::components::ui::layers::{LayerEntry, LayerStackOption, StandaloneLayerStackLayers};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::boxes::modal_render::render_dialog_overlay;
use crate::views::modal_boxes_suite::{ActiveModalDialog, ModalActionOutcome, ModalDialogKind};
use gpui::prelude::FluentBuilder;
use gpui::*;
use zeroize::Zeroize as _;

pub struct ModalContainer {
    pub stack: StandaloneLayerStackLayers,
    pub backdrop: LayerBackdropEffects,
    pub dialog: ActiveModalDialog,
    pub action_menu: Option<ActionMenuState>,
}

impl ModalContainer {
    pub fn new() -> Self {
        Self {
            stack: StandaloneLayerStackLayers::new(),
            backdrop: LayerBackdropEffects::new(),
            dialog: ActiveModalDialog::new(Vec::new()),
            action_menu: None,
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.dialog.set_palette(palette);
    }

    pub fn set_language(&mut self, language_id: impl Into<String>) {
        self.dialog.set_language(language_id);
    }

    pub fn show_modal(
        &mut self,
        title: impl Into<String>,
        width: f32,
        height: f32,
        option: LayerStackOption,
    ) -> u64 {
        let title_str = title.into();
        self.backdrop.start(true);
        self.dialog.open_for_title(&title_str, Vec::new());
        self.stack.show_box(title_str, width, height, option)
    }

    pub fn show_dialog(
        &mut self,
        title: impl Into<String>,
        kind: ModalDialogKind,
        forward_targets: Vec<(u64, String, String, u32)>,
        width: f32,
        height: f32,
        option: LayerStackOption,
    ) -> u64 {
        self.backdrop.start(true);
        self.dialog.open(kind, forward_targets);
        self.stack.show_box(title, width, height, option)
    }

    pub fn dismiss_top(&mut self) -> Option<LayerEntry> {
        let entry = self.stack.close_top();
        if !self.stack.has_active_layers() {
            self.dialog.close();
            self.backdrop.invalidate();
        }
        entry
    }

    pub fn hide_all(&mut self) {
        self.stack.hide_layers();
        self.dialog.close();
        self.backdrop.invalidate();
        self.action_menu = None;
    }

    /// Dismisses all active modals and wipes account-scoped dialog data & secret input drafts
    /// across logout / account-switch boundaries while preserving `palette` and `language_id`.
    pub fn reset_for_logout_or_account_switch(&mut self) {
        self.hide_all();
        self.dialog.primary_input.zeroize();
        self.dialog.secondary_input.zeroize();
        self.dialog.tertiary_input.zeroize();
        self.dialog.poll_options = vec![String::new(), String::new()];
        self.dialog.selected_members.clear();
        self.dialog.contacts_list.clear();
        self.dialog.call_logs.clear();
        self.dialog.proxies.clear();
        self.dialog.translated_text = None;
        self.dialog.stories.clear();
        self.dialog.story_slide_idx = 0;
        self.dialog.forward_targets.clear();
        self.dialog.pending_outcome = None;
    }

    pub fn is_modal_active(&self) -> bool {
        self.stack.has_active_layers() || self.dialog.is_open()
    }

    pub fn current_box(&self) -> Option<&LayerEntry> {
        self.stack.current_box()
    }

    pub fn handle_key_input(&mut self, key: &str, key_char: Option<&str>) -> bool {
        if let Some(menu) = self.action_menu.as_mut() {
            if menu.open {
                match key {
                    "down" | "arrowdown" => {
                        menu.move_highlight_down();
                        return true;
                    }
                    "up" | "arrowup" => {
                        menu.move_highlight_up();
                        return true;
                    }
                    "enter" => {
                        menu.activate_highlighted();
                        return true;
                    }
                    "escape" => {
                        menu.open = false;
                        return true;
                    }
                    _ => {}
                }
            }
        }
        if key == "escape" && self.is_modal_active() {
            if self.stack.has_active_layers() {
                self.dismiss_top();
            } else {
                self.dialog.close();
            }
            return true;
        }
        self.dialog.handle_key_input(key, key_char)
    }

    pub fn take_outcome(&mut self) -> Option<ModalActionOutcome> {
        self.dialog.pending_outcome.take()
    }
}

impl Default for ModalContainer {
    fn default() -> Self {
        Self::new()
    }
}

/// Calibrated OpenBot modal sheet backdrop scrim (`0.56` dark / `0.28` light).
pub(crate) fn macos_modal_backdrop_rgba(pal: TdesktopThemePalette) -> Rgba {
    let alpha = if pal.mode.is_dark() { 0.56 } else { 0.28 };
    TdesktopThemePalette::rgba_with_alpha(0x000000, alpha)
}

/// Canonical OpenBot floating modal card shell: `16px` corner radius, `#161616`/`#ffffff`
/// dialog surface, `1px` dialog ring border, and ambient + contact popover shadows.
pub(crate) fn macos_modal_card(id: &'static str, pal: TdesktopThemePalette) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let card_bg = if ob.scheme.is_dark() {
        rgb(0x161616)
    } else {
        ob.bg_surface()
    };
    let dialog_ring = ob
        .rgba("--openbot-dialog-ring")
        .unwrap_or_else(|| ob.border_strong_hairline());
    div()
        .id(id)
        .rounded(px(pal.scaled_px(16.0)))
        .bg(card_bg)
        .border_1()
        .border_color(dialog_ring)
        .shadow(pal.popover_shadows())
}

pub const ACTION_MENU_MIN_WIDTH_PX: f32 = 192.0;
pub const ACTION_MENU_MAX_WIDTH_PX: f32 = 288.0;
pub const ACTION_MENU_DEFAULT_WIDTH_PX: f32 = 224.0;
pub const ACTION_MENU_RADIUS_PX: f32 = 14.0;
pub const ACTION_MENU_ROW_HEIGHT_PX: f32 = 32.0;

/// Individual row entry in OpenBot `ui-action-menu` (`UI-F19`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionMenuItem {
    pub id: String,
    pub label: String,
    pub disabled: bool,
    pub destructive: bool,
}

/// Interactive state for OpenBot `ui-action-menu` supporting viewport edge-flip and
/// keyboard navigation that skips disabled items without invoking their handlers (`UI-F19`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionMenuState {
    pub open: bool,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub width_px: f32,
    pub items: Vec<ActionMenuItem>,
    pub highlighted_index: Option<usize>,
    pub last_invoked_id: Option<String>,
    pub disabled_item_skipped: bool,
    pub disabled_handler_invoked: bool,
}

impl ActionMenuState {
    pub fn open_at(
        anchor_x: f32,
        anchor_y: f32,
        width_px: f32,
        items: Vec<ActionMenuItem>,
    ) -> Self {
        let highlighted_index = items.iter().position(|item| !item.disabled);
        Self {
            open: true,
            anchor_x,
            anchor_y,
            width_px,
            items,
            highlighted_index,
            last_invoked_id: None,
            disabled_item_skipped: false,
            disabled_handler_invoked: false,
        }
    }

    /// Moves keyboard highlight circularly by `delta.signum()`, skipping disabled entries.
    pub fn move_highlight(&mut self, delta: isize) -> Option<usize> {
        let len = self.items.len();
        if len == 0 || !self.items.iter().any(|item| !item.disabled) {
            self.highlighted_index = None;
            return None;
        }
        let step = delta.signum();
        if step == 0 {
            return self.highlighted_index;
        }
        let mut idx = match self.highlighted_index {
            Some(cur) => (cur as isize + step).rem_euclid(len as isize) as usize,
            None => {
                if step > 0 {
                    0
                } else {
                    len - 1
                }
            }
        };
        for _ in 0..len {
            if !self.items[idx].disabled {
                self.highlighted_index = Some(idx);
                return Some(idx);
            }
            self.disabled_item_skipped = true;
            idx = (idx as isize + step).rem_euclid(len as isize) as usize;
        }
        None
    }

    pub fn move_highlight_down(&mut self) -> Option<usize> {
        self.move_highlight(1)
    }

    pub fn move_highlight_up(&mut self) -> Option<usize> {
        self.move_highlight(-1)
    }

    /// Activates item at `idx`; refuses disabled items without closing the menu or invoking handlers.
    pub fn activate_index(&mut self, idx: usize) -> Option<&str> {
        let item = self.items.get(idx)?;
        if item.disabled {
            self.disabled_handler_invoked = false;
            return None;
        }
        self.highlighted_index = Some(idx);
        self.last_invoked_id = Some(item.id.clone());
        self.open = false;
        self.last_invoked_id.as_deref()
    }

    /// Activates the currently highlighted enabled item.
    pub fn activate_highlighted(&mut self) -> Option<&str> {
        let idx = self.highlighted_index?;
        self.activate_index(idx)
    }

    /// Computes live menu placement with automatic horizontal/vertical viewport edge-flip.
    pub fn compute_layout_with_flip(
        &self,
        vp_w: f32,
        vp_h: f32,
        pal: &TdesktopThemePalette,
    ) -> ActionMenuLayoutSnapshot {
        let ob = pal.openbot_palette();
        let clamped_w = if self.width_px.is_finite() {
            self.width_px
                .clamp(ACTION_MENU_MIN_WIDTH_PX, ACTION_MENU_MAX_WIDTH_PX)
        } else {
            ACTION_MENU_DEFAULT_WIDTH_PX
        };
        let bg_rgb = crate::views::content_hub::apple_primitives::rgba_to_rgb24(ob.bg_surface());
        let safe_vp_w = if vp_w.is_finite() && vp_w > 0.0 { vp_w } else { 0.0 };
        let safe_vp_h = if vp_h.is_finite() && vp_h > 0.0 { vp_h } else { 0.0 };
        let row_count = self.items.len().max(2) as f32;
        let menu_h = row_count * ACTION_MENU_ROW_HEIGHT_PX + 8.0;
        let anchor_x = if self.anchor_x.is_finite() {
            self.anchor_x
        } else {
            8.0
        };
        let anchor_y = if self.anchor_y.is_finite() {
            self.anchor_y
        } else {
            8.0
        };

        let flip_x = anchor_x + clamped_w + 8.0 > safe_vp_w;
        let flip_y = anchor_y + menu_h + 8.0 > safe_vp_h;
        let raw_x = if flip_x {
            anchor_x - clamped_w
        } else {
            anchor_x
        };
        let raw_y = if flip_y {
            anchor_y - menu_h
        } else {
            anchor_y
        };
        let resolved_x = if safe_vp_w >= clamped_w + 16.0 {
            raw_x.clamp(8.0, (safe_vp_w - clamped_w - 8.0).max(8.0))
        } else if safe_vp_w >= clamped_w {
            raw_x.clamp(0.0, (safe_vp_w - clamped_w).max(0.0))
        } else {
            0.0
        };
        let resolved_y = if safe_vp_h >= menu_h + 16.0 {
            raw_y.clamp(8.0, (safe_vp_h - menu_h - 8.0).max(8.0))
        } else if safe_vp_h >= menu_h {
            raw_y.clamp(0.0, (safe_vp_h - menu_h).max(0.0))
        } else {
            0.0
        };
        let within_viewport = safe_vp_w >= clamped_w
            && safe_vp_h >= menu_h
            && resolved_x >= 0.0
            && resolved_y >= 0.0
            && resolved_x + clamped_w <= safe_vp_w
            && resolved_y + menu_h <= safe_vp_h;

        ActionMenuLayoutSnapshot {
            open: self.open,
            width_px: clamped_w,
            radius_px: ACTION_MENU_RADIUS_PX,
            bg_hex: crate::views::content_hub::apple_primitives::rgb24_to_hex(bg_rgb),
            row_height_px: ACTION_MENU_ROW_HEIGHT_PX,
            z_index: crate::theme::OPENBOT_LAYER_DROPDOWN,
            within_viewport,
            resolved_x,
            resolved_y,
            flip_x,
            flip_y,
            disabled_item_skipped: self.disabled_item_skipped,
            disabled_handler_invoked: self.disabled_handler_invoked,
        }
    }
}

/// Single-source-of-truth OpenBot `ui-action-menu` layout snapshot (`UI-F19`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionMenuLayoutSnapshot {
    pub open: bool,
    pub width_px: f32,
    pub radius_px: f32,
    pub bg_hex: String,
    pub row_height_px: f32,
    pub z_index: usize,
    pub within_viewport: bool,
    pub resolved_x: f32,
    pub resolved_y: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub disabled_item_skipped: bool,
    pub disabled_handler_invoked: bool,
}

/// Computes OpenBot `ui-action-menu` geometry (`[192, 288]px`, `14px` radius, `#212121`/`#ffffff`,
/// `32px` rows, `z_index = OPENBOT_LAYER_DROPDOWN = 40`).
pub fn compute_action_menu_layout(
    open: bool,
    width_px: f32,
    vp_w: f32,
    vp_h: f32,
    pal: &TdesktopThemePalette,
) -> ActionMenuLayoutSnapshot {
    let mut state = ActionMenuState::open_at(8.0, 8.0, width_px, Vec::new());
    state.open = open;
    state.compute_layout_with_flip(vp_w, vp_h, pal)
}

/// Semantic toast tone (`UI-F19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBotToastTone {
    Info,
    Success,
    Warning,
    Error,
}

impl OpenBotToastTone {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Individual toast notification rendered inside OpenBot `ui-toaster`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenBotToastEntry {
    pub id: u64,
    pub tone: OpenBotToastTone,
    pub message: String,
    pub undo_item: Option<String>,
}

/// Unified OpenBot `ui-toaster` state (`z_index = 120`, typed tones, undo support).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenBotToasterState {
    pub z_index: usize,
    pub reduced_motion: bool,
    pub next_id: u64,
    pub toasts: Vec<OpenBotToastEntry>,
    pub last_undone_item: Option<String>,
}

impl Default for OpenBotToasterState {
    fn default() -> Self {
        Self::new(false)
    }
}

impl OpenBotToasterState {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            z_index: crate::theme::OPENBOT_LAYER_TOAST,
            reduced_motion,
            next_id: 1,
            toasts: Vec::new(),
            last_undone_item: None,
        }
    }

    pub fn push_toast(
        &mut self,
        tone: OpenBotToastTone,
        text: impl Into<String>,
        undo_item: Option<String>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.toasts.push(OpenBotToastEntry {
            id,
            tone,
            message: text.into(),
            undo_item,
        });
        id
    }

    pub fn trigger_undo(&mut self) -> Option<String> {
        let idx = self.toasts.iter().rposition(|t| t.undo_item.is_some())?;
        let entry = self.toasts.remove(idx);
        let item = entry.undo_item;
        self.last_undone_item = item.clone();
        item
    }
}

/// Single-source-of-truth OpenBot `ui-toaster` layout snapshot (`UI-F19`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotToasterLayoutSnapshot {
    pub z_index: usize,
    pub reduced_motion: bool,
    pub visible: bool,
    pub max_width_px: f32,
    pub radius_px: f32,
    pub toasts: Vec<OpenBotToastEntry>,
}

pub fn compute_toaster_layout(state: &OpenBotToasterState) -> OpenBotToasterLayoutSnapshot {
    OpenBotToasterLayoutSnapshot {
        z_index: state.z_index,
        reduced_motion: state.reduced_motion,
        visible: !state.toasts.is_empty(),
        max_width_px: 480.0,
        radius_px: 14.0,
        toasts: state.toasts.clone(),
    }
}

/// Canonical OpenBot floating popover menu surface (`#212121`/`#ffffff`, `14px` radius,
/// `1px` hairline ring border, and two-layer popover shadows).
pub(crate) fn macos_popover_surface(id: &'static str, pal: TdesktopThemePalette) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let layout = compute_action_menu_layout(true, ACTION_MENU_DEFAULT_WIDTH_PX, 1440.0, 900.0, &pal);
    let surface_bg = rgb(crate::views::content_hub::apple_primitives::parse_hex_color(
        &layout.bg_hex,
        0x212121,
    ));
    div()
        .id(id)
        .occlude()
        .rounded(px(pal.scaled_px(layout.radius_px)))
        .bg(surface_bg)
        .border_1()
        .border_color(ob.border_strong_hairline())
        .shadow(pal.popover_shadows())
}

/// OpenBot monochrome primary CTA button (`#f0f0f0`/`#141414` dark, `#1c1c1e`/`#ffffff` light).
pub(crate) fn macos_primary_pill_btn(
    id: impl Into<ElementId>,
    label: &'static str,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = rgb(if ob.scheme.is_dark() {
        0xDCDCDC
    } else {
        0x2C2C2E
    });
    div()
        .id(id)
        .cursor_pointer()
        .px(px(16.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(ob.surface_light())
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ob.text_on_light())
        .child(label)
}

/// OpenBot subtle outline/secondary button with `1px` hairline border.
pub(crate) fn macos_secondary_pill_btn(
    id: impl Into<ElementId>,
    label: &'static str,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(ob.bg_control())
        .border_1()
        .border_color(ob.border_strong_hairline())
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ob.text_primary())
        .child(label)
}

impl Render for ModalContainer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.dialog.palette;
        let ob = pal.openbot_palette();
        let vp = window.viewport_size();
        let vp_w = f32::from(vp.width);
        let vp_h = f32::from(vp.height);

        let action_menu_overlay: Option<AnyElement> = self
            .action_menu
            .as_ref()
            .filter(|m| m.open)
            .map(|menu| {
                let layout = menu.compute_layout_with_flip(vp_w, vp_h, &pal);
                let highlighted = menu.highlighted_index;
                let hover_bg = ob.hover();
                let mut rows = Vec::with_capacity(menu.items.len());
                for (idx, item) in menu.items.iter().enumerate() {
                    let disabled = item.disabled;
                    let is_hl = highlighted == Some(idx) && !disabled;
                    let fg = if disabled {
                        ob.text_dim()
                    } else if item.destructive {
                        ob.danger()
                    } else {
                        ob.text_primary()
                    };
                    rows.push(
                        div()
                            .id(("openbot-action-menu-item", idx))
                            .w_full()
                            .h(px(layout.row_height_px))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .flex()
                            .items_center()
                            .text_size(px(13.0))
                            .text_color(fg)
                            .when(is_hl, |el| el.bg(hover_bg))
                            .when(disabled, |el| el.opacity(0.45))
                            .when(!disabled, |el| {
                                el.cursor_pointer()
                                    .hover(move |s| s.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if let Some(m) = this.action_menu.as_mut() {
                                                m.activate_index(idx);
                                            }
                                            cx.notify();
                                        }),
                                    )
                            })
                            .child(item.label.clone())
                            .into_any_element(),
                    );
                }
                let surface = macos_popover_surface("openbot-action-menu-popover", pal)
                    .absolute()
                    .left(px(layout.resolved_x))
                    .top(px(layout.resolved_y))
                    .w(px(layout.width_px))
                    .p(px(4.0))
                    .flex()
                    .flex_col()
                    .children(rows);
                deferred(surface)
                    .with_priority(layout.z_index)
                    .into_any_element()
            });

        if !self.is_modal_active() {
            if let Some(menu_el) = action_menu_overlay {
                return div()
                    .id("modal-container-action-menu-host")
                    .absolute()
                    .inset_0()
                    .child(menu_el)
                    .into_any_element();
            }
            return div()
                .absolute()
                .w(px(0.0))
                .h(px(0.0))
                .overflow_hidden()
                .into_any_element();
        }

        if !self.dialog.is_open() {
            if let Some(entry) = self.stack.current_box().cloned() {
                self.dialog.open_for_title(&entry.title, Vec::new());
            }
        }

        let overlay = render_dialog_overlay(
            &self.dialog,
            |this| &mut this.dialog,
            |this| {
                if this.stack.has_active_layers() {
                    this.dismiss_top();
                } else {
                    this.dialog.close();
                    this.backdrop.invalidate();
                }
            },
            cx,
        );

        div()
            .id("modal-container-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .child(overlay)
            .when_some(action_menu_overlay, |el, menu_el| el.child(menu_el))
            .into_any_element()
    }
}
