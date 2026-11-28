//! Interactive OpenBot `GlobalSearch` (`Cmd/Ctrl+K`) floating command palette entity (`UI-F07`).
//!
//! Owns its own `FocusHandle` and `EntityInputHandler` `canvas` so macOS/Linux CJK IME
//! composition, keyboard navigation (`↑/↓`, `Tab`, `Enter`, `Cmd+1..9`, `Esc`), and
//! `56px` `GlobalSearchPlush` caret tracking work cleanly without touching `shell_input_ime.rs`.

use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::AvatarMood;

use crate::components::avatar::render_plush_bot_avatar;
use crate::components::ui::{
    sanitize_paste_text, utf16_len, utf16_offset_to_utf8, TextInputController,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::openbot::agent_studio::{
    GlobalSearchResultItem, GlobalSearchSectionKind, GlobalSearchState, GlobalSearchTab,
    GLOBAL_SEARCH_DEBOUNCE_MS, GLOBAL_SEARCH_RESULT_HEIGHT_PX, GLOBAL_SEARCH_SECTION_HEIGHT_PX,
    GLOBAL_SEARCH_SPACED_SECTION_HEIGHT_PX,
};
use crate::views::sidebar::sidebar_search_view::{snippet_around, split_highlight_segments};

fn now_wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Interactive floating `GlobalSearch` (`Cmd/Ctrl+K`) palette entity mounted at root `AppShell`.
pub struct GlobalSearchModal {
    pub state: GlobalSearchState,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub focus_handle: FocusHandle,
    pub ime: TextInputController,
    pub pending_activate: Option<GlobalSearchResultItem>,
    pub pending_close: bool,
    pub pending_remote_search: Option<(u64, String)>,
    pub selected_all: bool,
}

impl GlobalSearchModal {
    pub fn new(
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut state = GlobalSearchState::new(false);
        state.try_open_palette(false);
        Self {
            state,
            palette,
            language_id: language_id.into(),
            focus_handle: cx.focus_handle(),
            ime: TextInputController::new(),
            pending_activate: None,
            pending_close: false,
            pending_remote_search: None,
            selected_all: false,
        }
    }

    pub fn set_items(&mut self, items: Vec<GlobalSearchResultItem>) {
        self.state.results = items;
        let len = self.state.flat_filtered_results().len();
        if len == 0 {
            self.state.selected_index = 0;
        } else {
            self.state.selected_index = self.state.selected_index.min(len - 1);
        }
    }

    pub fn set_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
    ) {
        self.palette = palette;
        self.language_id = language_id.into();
    }

    pub fn take_pending_activate(&mut self) -> Option<GlobalSearchResultItem> {
        self.pending_activate.take()
    }

    pub fn take_pending_close(&mut self) -> bool {
        std::mem::take(&mut self.pending_close)
    }

    pub fn take_pending_remote_search(&mut self) -> Option<(u64, String)> {
        self.pending_remote_search.take()
    }

    pub fn apply_remote_results(
        &mut self,
        seq: u64,
        items: Vec<GlobalSearchResultItem>,
        cx: &mut Context<Self>,
    ) -> bool {
        let applied = self.state.apply_remote_results(seq, items);
        if applied {
            cx.notify();
        }
        applied
    }

    pub fn fail_remote_search(
        &mut self,
        seq: u64,
        err: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let applied = self.state.fail_remote_search(seq, err);
        if applied {
            cx.notify();
        }
        applied
    }

    fn schedule_debounced_remote_search(&mut self, cx: &mut Context<Self>) {
        if self.state.query.trim().is_empty() {
            self.state.remote_error = None;
            self.state.set_searching(false, now_wall_ms());
            return;
        }
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            app.background_executor()
                .timer(std::time::Duration::from_millis(GLOBAL_SEARCH_DEBOUNCE_MS))
                .await;
            let _ = weak.update(app, |this, cx| {
                if let Some(seq) = this.state.begin_remote_search(now_wall_ms()) {
                    this.pending_remote_search = Some((seq, this.state.debounced_query.clone()));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) {
        self.ime.clear_ime_state();
        self.state.close_palette();
        self.pending_close = true;
        cx.notify();
    }

    pub fn activate_item(&mut self, item: GlobalSearchResultItem, cx: &mut Context<Self>) {
        self.ime.clear_ime_state();
        if let Some(pos) = self
            .state
            .flat_filtered_results()
            .iter()
            .position(|r| r.id == item.id)
        {
            self.state.selected_index = pos;
        }
        let activated = self.state.execute_selected().unwrap_or_else(|| {
            self.state.last_activated_target_id = Some(item.id.clone());
            self.state.close_palette();
            item
        });
        self.pending_activate = Some(activated);
        self.pending_close = true;
        cx.notify();
    }

    pub fn insert_committed_text(&mut self, raw: &str, cx: &mut Context<Self>) {
        let clean = sanitize_paste_text(raw, false);
        if clean.is_empty() {
            return;
        }
        let now_ms = now_wall_ms();
        if self.selected_all {
            self.selected_all = false;
            self.state.clear_query(now_ms);
        }
        self.state.push_query_text(&clean, now_ms);
        self.ime.clear_preedit();
        self.schedule_debounced_remote_search(cx);
        cx.notify();
    }

    pub fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        let cmd = mods.platform || mods.control;
        let now_ms = now_wall_ms();

        if key.eq_ignore_ascii_case("escape") || (cmd && !mods.shift && !mods.alt && key.eq_ignore_ascii_case("k")) {
            cx.stop_propagation();
            self.request_close(cx);
            return;
        }
        if key.eq_ignore_ascii_case("up") {
            cx.stop_propagation();
            self.selected_all = false;
            self.state.move_selection(-1);
            cx.notify();
            return;
        }
        if key.eq_ignore_ascii_case("down") {
            cx.stop_propagation();
            self.selected_all = false;
            self.state.move_selection(1);
            cx.notify();
            return;
        }
        if key.eq_ignore_ascii_case("tab") {
            cx.stop_propagation();
            self.selected_all = false;
            self.state.cycle_tab(!mods.shift);
            cx.notify();
            return;
        }
        if key.eq_ignore_ascii_case("enter") {
            cx.stop_propagation();
            if self.ime.has_preedit() {
                if let Some(p) = self.ime.ime_preedit.take().filter(|s| !s.is_empty()) {
                    self.insert_committed_text(&p, cx);
                }
                return;
            }
            if let Some(item) = self.state.selected_result().cloned() {
                self.activate_item(item, cx);
            }
            return;
        }
        if cmd && !mods.shift && !mods.alt {
            if key.len() == 1 {
                if let Some(ch) = key.chars().next() {
                    if ('1'..='9').contains(&ch) {
                        cx.stop_propagation();
                        let digit = (ch as u8 - b'0') as usize;
                        if let Some(item) = self.state.result_at_shortcut_digit(digit).cloned() {
                            self.activate_item(item, cx);
                        }
                        return;
                    }
                }
            }
            if key.eq_ignore_ascii_case("a") {
                cx.stop_propagation();
                self.selected_all = !self.state.query.is_empty();
                cx.notify();
                return;
            }
            if key.eq_ignore_ascii_case("v") {
                cx.stop_propagation();
                if let Some(clip) = cx.read_from_clipboard() {
                    if let Some(text) = clip.text() {
                        self.insert_committed_text(&text, cx);
                    }
                }
                return;
            }
        }
        if key.eq_ignore_ascii_case("backspace") {
            cx.stop_propagation();
            if self.ime.has_preedit() {
                self.ime.clear_preedit();
                cx.notify();
                return;
            }
            if self.selected_all {
                self.selected_all = false;
                self.state.clear_query(now_ms);
            } else {
                self.state.backspace_query(now_ms);
            }
            self.schedule_debounced_remote_search(cx);
            cx.notify();
            return;
        }
        if !cmd && !mods.alt {
            if self.ime.has_preedit() || self.ime.take_just_committed() {
                return;
            }
            if key == "space" {
                cx.stop_propagation();
                self.insert_committed_text(" ", cx);
            } else if let Some(ref kc) = event.keystroke.key_char {
                if !kc.chars().all(|c| c.is_control()) {
                    cx.stop_propagation();
                    self.insert_committed_text(kc, cx);
                }
            }
        }
    }
}

impl EntityInputHandler for GlobalSearchModal {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let full = self
            .ime
            .ime_preedit
            .as_deref()
            .map_or(self.state.query.clone(), |p| format!("{}{}", self.state.query, p));
        let start = utf16_offset_to_utf8(&full, range_utf16.start);
        let end = utf16_offset_to_utf8(&full, range_utf16.end).max(start);
        Some(full.get(start..end).unwrap_or("").to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let range = if self.ime.has_preedit() {
            self.ime.selected_utf16_range(&self.state.query)?
        } else {
            let total = utf16_len(&self.state.query);
            total..total
        };
        Some(UTF16Selection {
            range,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if self.ime.has_preedit() {
            self.ime.marked_utf16_range(&self.state.query)
        } else {
            None
        }
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.ime.ime_preedit.take().filter(|s| !s.is_empty()) {
            self.insert_committed_text(&p, cx);
            self.ime.mark_ime_committed();
            return;
        }
        self.ime.clear_ime_state();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_composing = self.ime.has_preedit();
        self.insert_committed_text(text, cx);
        if was_composing {
            self.ime.mark_ime_committed();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(committed) = self.ime.handle_replace_and_mark(new_text) {
            self.insert_committed_text(&committed, cx);
            self.ime.mark_ime_committed();
        } else {
            let total_chars = self.state.query.chars().count()
                + self
                    .ime
                    .ime_preedit
                    .as_deref()
                    .map_or(0, |p| p.chars().count());
            self.state.set_caret_x((total_chars as f64) * 8.0, now_wall_ms());
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let chars = self.state.query.chars().count()
            + self
                .ime
                .ime_preedit
                .as_deref()
                .map_or(0, |p| p.chars().count());
        let caret_x =
            element_bounds.origin.x + px(42.0 + (chars as f32 * 8.0).min(240.0));
        Some(Bounds::new(
            point(caret_x, element_bounds.origin.y + px(14.0)),
            size(px(2.0), px(20.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(
            utf16_len(&self.state.query)
                + self
                    .ime
                    .ime_preedit
                    .as_deref()
                    .map_or(0, utf16_len),
        )
    }
}

fn render_highlighted_text(
    text: &str,
    query: &str,
    base_color: Rgba,
    match_color: Rgba,
    font_size: f32,
    base_weight: FontWeight,
) -> AnyElement {
    let (pre, hit, post) = split_highlight_segments(text, query);
    if hit.is_empty() {
        return div()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(font_size))
            .font_weight(base_weight)
            .text_color(base_color)
            .child(text.to_string())
            .into_any_element();
    }
    div()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .text_size(px(font_size))
        .when(!pre.is_empty(), |el| {
            el.child(div().font_weight(base_weight).text_color(base_color).child(pre))
        })
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(match_color)
                .child(hit),
        )
        .when(!post.is_empty(), |el| {
            el.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(base_weight)
                    .text_color(base_color)
                    .child(post),
            )
        })
        .into_any_element()
}

fn render_result_leading_visual(
    item: &GlobalSearchResultItem,
    text_muted: Rgba,
) -> AnyElement {
    match item.section {
        GlobalSearchSectionKind::Agents | GlobalSearchSectionKind::Messages => {
            let seed = item.avatar_seed.as_deref().unwrap_or(item.id.as_str());
            render_plush_bot_avatar(seed, None, None, None, 20.0, AvatarMood::Idle, None)
                .into_any_element()
        }
        GlobalSearchSectionKind::Channels => div()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_muted)
            .child("#")
            .into_any_element(),
        GlobalSearchSectionKind::Files => div()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(tg_icon_rgba(IconName::FileDoc, 15.0, text_muted))
            .into_any_element(),
        GlobalSearchSectionKind::Routines => div()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(tg_icon_rgba(IconName::ClockSending, 15.0, text_muted))
            .into_any_element(),
        GlobalSearchSectionKind::Actions => div()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(tg_icon_rgba(IconName::ComposePen, 14.0, text_muted))
            .into_any_element(),
        GlobalSearchSectionKind::Settings => div()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(tg_icon_rgba(IconName::SettingsGear, 15.0, text_muted))
            .into_any_element(),
    }
}

impl Render for GlobalSearchModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ob = self.palette.openbot_palette();
        let is_dark = self.palette.mode.is_dark();
        let lang = self.language_id.clone();
        let vp = window.viewport_size();
        let modal_layout = crate::views::openbot::agent_studio::compute_global_search_modal_layout(
            f32::from(vp.width),
            f32::from(vp.height),
            is_dark,
        );
        let card_bg = rgb(modal_layout.bg_rgb);
        let ring = ob
            .rgba("--openbot-dialog-ring")
            .unwrap_or_else(|| ob.border_strong_hairline());
        let divider = ob.border();
        let hover_bg = ob.hover();
        let active_bg = ob.bg_control_active();
        let kbd_bg = ob.bg_control_hover();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();
        let card_w = modal_layout.width_px;
        let top_offset = modal_layout.top_px;
        let list_max_h = modal_layout.list_max_h_px;
        let row_vms = self.state.build_visible_row_view_models();

        let fh = self.focus_handle.clone();
        let view = cx.entity().clone();
        let ime_canvas = canvas(
            |bounds, _, _| bounds,
            move |bounds, _, window, cx| {
                window.handle_input(&fh, ElementInputHandler::new(bounds, view.clone()), cx);
            },
        )
        .size_0();

        let query_snapshot = self.state.query.clone();
        let preedit_snapshot = self.ime.ime_preedit.clone().unwrap_or_default();
        let selected_idx = self.state.selected_index;
        let now_ms = now_wall_ms() as f64;
        let sections: Vec<(GlobalSearchSectionKind, Vec<GlobalSearchResultItem>)> = self
            .state
            .filtered_sections()
            .into_iter()
            .map(|(sec, items)| (sec, items.into_iter().cloned().collect()))
            .collect();

        let mut tab_strip = div().flex().items_center().gap(px(2.0)).flex_shrink_0();
        for tab in GlobalSearchTab::ALL {
            let active = self.state.active_tab == tab;
            tab_strip = tab_strip.child(
                div()
                    .id(SharedString::from(format!("global-search-tab-{}", tab.as_str())))
                    .h(px(24.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .when(active, |el| el.bg(active_bg))
                    .flex()
                    .items_center()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active { text_pri } else { text_muted })
                    .hover(move |s| if active { s } else { s.text_color(text_sec) })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.state.active_tab = tab;
                            this.state.selected_index = 0;
                            cx.notify();
                        }),
                    )
                    .child(tab.localized_label(&lang)),
            );
        }

        let mut list_body = div()
            .id("global-search-results-scroll")
            .flex_1()
            .min_h(px(0.0))
            .max_h(px(list_max_h))
            .overflow_y_scroll()
            .p(px(6.0))
            .flex()
            .flex_col();

        let mut flat_idx = 0usize;
        for (sec_idx, (sec_kind, items)) in sections.into_iter().enumerate() {
            let header_h = if sec_idx == 0 {
                GLOBAL_SEARCH_SECTION_HEIGHT_PX
            } else {
                GLOBAL_SEARCH_SPACED_SECTION_HEIGHT_PX
            };
            list_body = list_body.child(
                div()
                    .h(px(header_h))
                    .px(px(8.0))
                    .pb(px(4.0))
                    .flex()
                    .items_end()
                    .justify_between()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_muted)
                    .child(sec_kind.localized_label(&lang))
                    .child(items.len().to_string()),
            );
            for item in items {
                let row_idx = flat_idx;
                let row_vm = row_vms.get(row_idx);
                let selected = row_vm.map_or(row_idx == selected_idx, |vm| vm.selected);
                let kbd_enter_visible = row_vm.map_or(selected, |vm| vm.kbd_enter_visible);
                flat_idx += 1;
                let leading = render_result_leading_visual(&item, text_muted);
                let snippet = snippet_around(&item.subtitle, &query_snapshot);
                let title_el = render_highlighted_text(
                    &item.title,
                    &query_snapshot,
                    text_pri,
                    text_pri,
                    14.0,
                    FontWeight::MEDIUM,
                );
                let subtitle_el = (!snippet.is_empty()).then(|| {
                    render_highlighted_text(
                        &snippet,
                        &query_snapshot,
                        text_muted,
                        text_pri,
                        13.5,
                        FontWeight::NORMAL,
                    )
                });
                let badge = item.badge.clone();
                let item_for_click = item.clone();

                list_body = list_body.child(
                    div()
                        .id(SharedString::from(format!("global-search-row-{row_idx}")))
                        .h(px(GLOBAL_SEARCH_RESULT_HEIGHT_PX))
                        .flex_shrink_0()
                        .w_full()
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .when(selected, |el| el.bg(active_bg))
                        .hover(move |s| if selected { s } else { s.bg(hover_bg) })
                        .on_hover(cx.listener(move |this, hovered, _, cx| {
                            if *hovered && this.state.selected_index != row_idx {
                                this.state.selected_index = row_idx;
                                cx.notify();
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.activate_item(item_for_click.clone(), cx);
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(leading)
                                .child(
                                    div()
                                        .max_w(px(220.0))
                                        .min_w(px(0.0))
                                        .flex_shrink_0()
                                        .overflow_hidden()
                                        .flex()
                                        .items_center()
                                        .child(title_el),
                                )
                                .when_some(subtitle_el, |el, sub| el.child(sub)),
                        )
                        .when_some(badge, |el, b| {
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.5))
                                    .text_color(text_muted)
                                    .child(b),
                            )
                        })
                        .when(kbd_enter_visible, |el| {
                            el.child(
                                div()
                                    .h(px(20.0))
                                    .px(px(5.0))
                                    .rounded(px(4.0))
                                    .bg(kbd_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(11.0))
                                    .text_color(text_muted)
                                    .child("↵"),
                            )
                        }),
                );
            }
        }

        if let Some(remote_err) = self.state.remote_error.clone() {
            let danger = ob.danger();
            list_body = list_body.child(
                div()
                    .id("global-search-remote-error-banner")
                    .mx(px(12.0))
                    .my(px(6.0))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(active_bg)
                    .border_1()
                    .border_color(danger)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(danger)
                            .child(remote_err),
                    )
                    .child(
                        div()
                            .id("global-search-remote-retry-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(kbd_bg)
                            .hover(move |s| s.bg(hover_bg))
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_pri)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let seq = this.state.retry_remote_search(now_wall_ms());
                                let q = if this.state.debounced_query.trim().is_empty() {
                                    this.state.query.clone()
                                } else {
                                    this.state.debounced_query.clone()
                                };
                                this.pending_remote_search = Some((seq, q));
                                cx.notify();
                            }))
                            .child(tr_str(&lang, "重试", "Retry")),
                    ),
            );
        }

        if self.state.is_plush_visible() || flat_idx == 0 {
            let empty_plush = self
                .state
                .plush
                .render_element(now_ms, modal_layout.plush_size_px);
            list_body = list_body.child(
                div()
                    .min_h(px(148.0))
                    .py(px(12.0))
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .child(empty_plush)
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(text_muted)
                            .child(tr_str(&lang, "未找到匹配结果", "No results found")),
                    ),
            );
        }

        let kbd_chip = |label: &'static str| {
            div()
                .h(px(20.0))
                .px(px(5.0))
                .rounded(px(4.0))
                .bg(active_bg)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_sec)
                .child(label)
        };

        let input_display = if query_snapshot.is_empty() && preedit_snapshot.is_empty() {
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(15.0))
                .text_color(text_muted)
                .child(tr_str(&lang, "搜索", "Search"))
        } else {
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .text_size(px(15.0))
                .text_color(text_pri)
                .when(!query_snapshot.is_empty(), |el| {
                    el.child(
                        div()
                            .when(self.selected_all, |s| s.bg(ob.accent_soft()).rounded(px(3.0)))
                            .child(query_snapshot),
                    )
                })
                .when(!preedit_snapshot.is_empty(), |el| {
                    el.child(
                        div()
                            .border_b_1()
                            .border_color(text_pri)
                            .text_color(text_sec)
                            .child(preedit_snapshot),
                    )
                })
                .child(div().w(px(1.5)).h(px(17.0)).ml(px(1.0)).bg(text_pri))
        };

        div()
            .id("global-search-backdrop")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.handle_key_down(event, cx);
            }))
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x00000047))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(top_offset))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.request_close(cx);
                }),
            )
            .child(ime_canvas)
            .child(
                div()
                    .id("global-search-dialog-card")
                    .w(px(card_w))
                    .rounded(px(16.0))
                    .bg(card_bg)
                    .border_1()
                    .border_color(ring)
                    .shadow(self.palette.popover_shadows())
                    .overflow_hidden()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(modal_layout.input_height_px))
                            .flex_shrink_0()
                            .w_full()
                            .pl(px(16.0))
                            .pr(px(8.0))
                            .border_b_1()
                            .border_color(divider)
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(tg_icon_rgba(IconName::Search, 16.0, text_muted))
                            .child(input_display)
                            .child(tab_strip),
                    )
                    .child(list_body)
                    .child(
                        div()
                            .h(px(modal_layout.footer_height_px))
                            .flex_shrink_0()
                            .w_full()
                            .px(px(16.0))
                            .border_t_1()
                            .border_color(divider)
                            .flex()
                            .items_center()
                            .gap(px(16.0))
                            .text_size(px(13.0))
                            .text_color(text_muted)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(kbd_chip("↑"))
                                    .child(kbd_chip("↓"))
                                    .child(tr_str(&lang, "导航", "Navigate")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(kbd_chip("↵"))
                                    .child(tr_str(&lang, "打开", "Open")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(kbd_chip("Tab"))
                                    .child(tr_str(&lang, "筛选", "Filter")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(kbd_chip("Esc"))
                                    .child(tr_str(&lang, "关闭", "Close")),
                            ),
                    ),
            )
    }
}
