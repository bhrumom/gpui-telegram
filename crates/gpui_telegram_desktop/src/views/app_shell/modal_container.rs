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
use gpui::*;
use zeroize::Zeroize as _;

pub struct ModalContainer {
    pub stack: StandaloneLayerStackLayers,
    pub backdrop: LayerBackdropEffects,
    pub dialog: ActiveModalDialog,
}

impl ModalContainer {
    pub fn new() -> Self {
        Self {
            stack: StandaloneLayerStackLayers::new(),
            backdrop: LayerBackdropEffects::new(),
            dialog: ActiveModalDialog::new(Vec::new()),
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

/// Canonical OpenBot floating popover menu surface (`#212121`/`#ffffff`, `14px` radius,
/// `1px` hairline ring border, and two-layer popover shadows).
pub(crate) fn macos_popover_surface(id: &'static str, pal: TdesktopThemePalette) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    div()
        .id(id)
        .occlude()
        .rounded(px(pal.scaled_px(14.0)))
        .bg(ob.bg_surface())
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_modal_active() {
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
            .into_any_element()
    }
}
