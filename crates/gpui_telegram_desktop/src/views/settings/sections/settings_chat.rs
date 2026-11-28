use gpui::*;
use gpui::prelude::FluentBuilder;
use crate::i18n::tr_str;
use crate::theme::assets::IconName;
use crate::theme::palette::{
    ACCENT_SWATCHES, TdesktopThemeMode, TdesktopThemePalette, WALLPAPER_PRESETS,
};
use crate::views::settings::settings_theme_ui::{
    settings_card, settings_container, settings_group_card, settings_section_block,
    settings_static_row, settings_toggle_row,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatThemePreset {
    Classic,
    Day,
    Tinted,
    Night,
}

impl ChatThemePreset {
    pub fn to_mode(self) -> TdesktopThemeMode {
        match self {
            Self::Classic => TdesktopThemeMode::Classic,
            Self::Day => TdesktopThemeMode::Day,
            Self::Tinted => TdesktopThemeMode::Tinted,
            Self::Night => TdesktopThemeMode::Night,
        }
    }
}

pub struct SettingsChatSections {
    pub font_size: u32,
    pub bubble_radius: u32,
    pub theme_preset: ChatThemePreset,
    pub accent_color: u32,
    pub wallpaper_name: String,
    pub blur_wallpaper: bool,
    pub send_on_enter: bool,
    pub suggest_stickers_by_emoji: bool,
    pub loop_animated_stickers: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    theme_dirty: bool,
    chat_prefs_dirty: bool,
}

impl SettingsChatSections {
    pub fn new() -> Self {
        let palette = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12);
        Self {
            font_size: 13,
            bubble_radius: 12,
            theme_preset: ChatThemePreset::Day,
            accent_color: 0x3390ec,
            wallpaper_name: WALLPAPER_PRESETS[0].id.to_string(),
            blur_wallpaper: false,
            send_on_enter: true,
            suggest_stickers_by_emoji: true,
            loop_animated_stickers: true,
            palette,
            language_id: "en".to_string(),
            theme_dirty: false,
            chat_prefs_dirty: false,
        }
    }

    pub fn new_live(palette: TdesktopThemePalette, language_id: &str) -> Self {
        let theme_preset = match palette.mode {
            TdesktopThemeMode::Classic => ChatThemePreset::Classic,
            TdesktopThemeMode::Day => ChatThemePreset::Day,
            TdesktopThemeMode::Tinted => ChatThemePreset::Tinted,
            TdesktopThemeMode::Night => ChatThemePreset::Night,
        };
        Self {
            font_size: palette.font_size_px,
            bubble_radius: palette.bubble_radius_px,
            theme_preset,
            accent_color: palette.accent,
            wallpaper_name: WALLPAPER_PRESETS[0].id.to_string(),
            blur_wallpaper: palette.blur_wallpaper,
            send_on_enter: true,
            suggest_stickers_by_emoji: true,
            loop_animated_stickers: true,
            palette,
            language_id: language_id.to_string(),
            theme_dirty: false,
            chat_prefs_dirty: false,
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.theme_preset = match palette.mode {
            TdesktopThemeMode::Classic => ChatThemePreset::Classic,
            TdesktopThemeMode::Day => ChatThemePreset::Day,
            TdesktopThemeMode::Tinted => ChatThemePreset::Tinted,
            TdesktopThemeMode::Night => ChatThemePreset::Night,
        };
        self.accent_color = palette.accent;
        self.font_size = palette.font_size_px;
        self.bubble_radius = palette.bubble_radius_px;
        self.blur_wallpaper = palette.blur_wallpaper;
        self.palette = palette;
    }

    pub fn set_language(&mut self, language_id: impl Into<String>) { self.language_id = language_id.into(); }
    pub fn set_font_size(&mut self, size: u32) { self.font_size = size.clamp(12, 30); self.rebuild_palette(); self.chat_prefs_dirty = true; }
    pub fn set_bubble_radius(&mut self, radius: u32) { self.bubble_radius = radius.clamp(2, 18); self.rebuild_palette(); self.chat_prefs_dirty = true; }
    pub fn set_theme_preset(&mut self, preset: ChatThemePreset) { self.theme_preset = preset; self.rebuild_palette(); self.theme_dirty = true; }
    pub fn set_accent_color(&mut self, color: u32) { self.accent_color = color; self.rebuild_palette(); self.theme_dirty = true; }
    pub fn set_wallpaper(&mut self, id: &str) { self.wallpaper_name = id.to_string(); self.palette = self.palette.clone().with_wallpaper(id); self.theme_dirty = true; }
    pub fn toggle_send_on_enter(&mut self) { self.send_on_enter = !self.send_on_enter; self.chat_prefs_dirty = true; }
    pub fn toggle_loop_animated_stickers(&mut self) { self.loop_animated_stickers = !self.loop_animated_stickers; self.chat_prefs_dirty = true; }
    pub fn toggle_blur_wallpaper(&mut self) { self.blur_wallpaper = !self.blur_wallpaper; self.rebuild_palette(); self.chat_prefs_dirty = true; }

    fn rebuild_palette(&mut self) {
        let ui_scale_pct = self.palette.ui_scale_pct;
        self.palette = TdesktopThemePalette::build(self.theme_preset.to_mode(), self.accent_color, self.font_size.clamp(12, 20), self.bubble_radius.clamp(2, 18))
            .with_wallpaper(&self.wallpaper_name)
            .with_ui_scale(ui_scale_pct)
            .with_blur_wallpaper(self.blur_wallpaper);
    }

    pub fn take_theme_change(&mut self) -> Option<(TdesktopThemeMode, u32, String)> {
        if self.theme_dirty { self.theme_dirty = false; Some((self.theme_preset.to_mode(), self.accent_color, self.wallpaper_name.clone())) } else { None }
    }

    pub fn take_chat_prefs_change(&mut self) -> Option<(u32, u32, bool, bool, bool, bool)> {
        if self.chat_prefs_dirty {
            self.chat_prefs_dirty = false;
            Some((
                self.font_size,
                self.bubble_radius,
                self.send_on_enter,
                self.suggest_stickers_by_emoji,
                self.loop_animated_stickers,
                self.blur_wallpaper,
            ))
        } else {
            None
        }
    }

    pub fn theme_name(&self) -> &'static str { self.theme_preset.to_mode().as_name() }
    pub fn theme_name_localized(&self, lang: &str) -> &'static str {
        match self.theme_preset {
            ChatThemePreset::Night => tr_str(lang, "夜间", "Night"),
            ChatThemePreset::Tinted => tr_str(lang, "暗调", "Tinted"),
            ChatThemePreset::Day => tr_str(lang, "日间", "Day"),
            ChatThemePreset::Classic => tr_str(lang, "经典", "Classic"),
        }
    }
}

impl Default for SettingsChatSections {
    fn default() -> Self { Self::new() }
}

fn wallpaper_label_ink(bg_hex: u32) -> u32 {
    let linear = |channel: u32| -> f32 {
        let s = (channel & 0xFF) as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let lum = 0.2126 * linear(bg_hex >> 16)
        + 0.7152 * linear(bg_hex >> 8)
        + 0.0722 * linear(bg_hex);
    if lum > 0.179 {
        0x18222D
    } else {
        0xFFFFFF
    }
}

impl Render for SettingsChatSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let rad = self.bubble_radius as f32;
        let hover_ctrl = ob.bg_control_hover();
        let presets = [ChatThemePreset::Night, ChatThemePreset::Tinted, ChatThemePreset::Day, ChatThemePreset::Classic];

        let preview_box = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(16.0))
            .rounded(px(12.0))
            .bg(rgb(pal.wallpaper_top))
            .border_1()
            .border_color(ob.frame_card_border())
            .child(
                div().flex().justify_start().child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(pal.scaled_px(rad)))
                        .bg(rgb(pal.msg_in_bg))
                        .text_color(rgb(pal.text_primary))
                        .text_size(px(pal.scaled_px(self.font_size as f32)))
                        .child(tr_str(&lang, "早上好！新版界面简洁清爽了许多。", "Good morning! The new layout feels super clean.")),
                ),
            )
            .child(
                div().flex().justify_end().child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(pal.scaled_px(rad)))
                        .bg(rgb(pal.msg_out_bg))
                        .text_color(rgb(pal.text_primary))
                        .text_size(px(pal.scaled_px(self.font_size as f32)))
                        .child(tr_str(&lang, "原生丝滑，任何主题下都清晰易读。", "Native, crisp, and readable in every theme.")),
                ),
            );

        let theme_segmented = div()
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(ob.bg_control())
            .border_1()
            .border_color(ob.border_hairline())
            .flex()
            .items_center()
            .gap(px(2.0))
            .children(presets.into_iter().map(|preset| {
                let active = preset == self.theme_preset;
                let preset_label = match preset {
                    ChatThemePreset::Night => tr_str(&lang, "夜间", "Night"),
                    ChatThemePreset::Tinted => tr_str(&lang, "暗调", "Tinted"),
                    ChatThemePreset::Day => tr_str(&lang, "日间", "Day"),
                    ChatThemePreset::Classic => tr_str(&lang, "经典", "Classic"),
                };
                div()
                    .cursor_pointer()
                    .h(px(26.0))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .when(active, |el| el.bg(ob.surface_light()))
                    .when(!active, |el| el.hover(move |s| s.bg(hover_ctrl)))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(if active { ob.text_on_light() } else { ob.text_secondary() })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_theme_preset(preset);
                            cx.notify();
                        }),
                    )
                    .child(preset_label)
            }));

        let accent_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .children(ACCENT_SWATCHES.iter().map(|sw| {
                let sel = sw.hex == self.accent_color;
                div()
                    .size(px(22.0))
                    .rounded_full()
                    .bg(rgb(sw.hex))
                    .border_2()
                    .border_color(if sel { ob.text_primary() } else { ob.border_hairline() })
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let hex = sw.hex;
                            move |this, _, _, cx| {
                                this.set_accent_color(hex);
                                cx.notify();
                            }
                        }),
                    )
            }));

        let wallpaper_strip = settings_card(pal)
            .py(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(WALLPAPER_PRESETS.iter().map(|wp| {
                        let sel = wp.id == self.wallpaper_name || wp.name == self.wallpaper_name;
                        let label_ink = wallpaper_label_ink(wp.top_hex);
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .bg(rgb(wp.top_hex))
                            .border_1()
                            .border_color(if sel { ob.text_primary() } else { ob.border_hairline() })
                            .text_size(px(12.0))
                            .font_weight(if sel { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                            .text_color(rgb(label_ink))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let id = wp.id;
                                    move |this, _, _, cx| {
                                        this.set_wallpaper(id);
                                        cx.notify();
                                    }
                                }),
                            )
                            .child(wp.name)
                    })),
            );

        let font_stepper = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .cursor_pointer()
                    .w(px(28.0))
                    .h(px(26.0))
                    .rounded(px(6.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .hover(move |s| s.bg(hover_ctrl))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(ob.text_primary())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_font_size(this.font_size.saturating_sub(1));
                            cx.notify();
                        }),
                    )
                    .child("−"),
            )
            .child(
                div()
                    .w(px(44.0))
                    .flex()
                    .justify_center()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ob.text_primary())
                    .child(format!("{} pt", self.font_size)),
            )
            .child(
                div()
                    .cursor_pointer()
                    .w(px(28.0))
                    .h(px(26.0))
                    .rounded(px(6.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .hover(move |s| s.bg(hover_ctrl))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(ob.text_primary())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_font_size(this.font_size + 1);
                            cx.notify();
                        }),
                    )
                    .child("+"),
            );

        let radius_stepper = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .cursor_pointer()
                    .w(px(28.0))
                    .h(px(26.0))
                    .rounded(px(6.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .hover(move |s| s.bg(hover_ctrl))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(ob.text_primary())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_bubble_radius(this.bubble_radius.saturating_sub(2).max(2));
                            cx.notify();
                        }),
                    )
                    .child("−"),
            )
            .child(
                div()
                    .w(px(44.0))
                    .flex()
                    .justify_center()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ob.text_primary())
                    .child(format!("{} px", self.bubble_radius)),
            )
            .child(
                div()
                    .cursor_pointer()
                    .w(px(28.0))
                    .h(px(26.0))
                    .rounded(px(6.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .hover(move |s| s.bg(hover_ctrl))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(ob.text_primary())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_bubble_radius((this.bubble_radius + 2).min(18));
                            cx.notify();
                        }),
                    )
                    .child("+"),
            );

        settings_container(pal)
            .child(preview_box)
            .child(settings_section_block(
                tr_str(&lang, "颜色与主题", "Colors & Theme"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_static_row(
                            tr_str(&lang, "主题", "Theme"),
                            Some(tr_str(&lang, "选择浅色或深色配色方案。", "Choose light or dark color scheme.")),
                            theme_segmented,
                            pal,
                        ),
                        settings_static_row(
                            tr_str(&lang, "强调色", "Accent color"),
                            Some(tr_str(&lang, "用于聊天链接、气泡与高亮元素。", "Used for links, bubbles, and highlights.")),
                            accent_row,
                            pal,
                        ),
                    ],
                ),
            ))
            .child(settings_section_block(
                tr_str(&lang, "聊天背景", "Chat Wallpaper"),
                None,
                pal,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(wallpaper_strip)
                    .child(settings_group_card(
                        pal,
                        vec![settings_toggle_row(
                            IconName::SunDay,
                            tr_str(&lang, "模糊背景", "Blur wallpaper"),
                            Some(tr_str(&lang, "对聊天背景应用柔和毛玻璃模糊。", "Apply a subtle blur to the chat background.")),
                            self.blur_wallpaper,
                            pal,
                            |this| this.toggle_blur_wallpaper(),
                            cx,
                        )],
                    )),
            ))
            .child(settings_section_block(
                tr_str(&lang, "排版与对话交互", "Typography & Chat"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_static_row(
                            tr_str(&lang, "字体大小", "Message text size"),
                            None,
                            font_stepper,
                            pal,
                        ),
                        settings_static_row(
                            tr_str(&lang, "气泡圆角", "Bubble corner radius"),
                            None,
                            radius_stepper,
                            pal,
                        ),
                        settings_toggle_row(
                            IconName::SendPlane,
                            tr_str(&lang, "按 Enter 发送消息", "Send with Enter"),
                            Some(tr_str(&lang, "关闭后使用 ⌘Enter / Ctrl+Enter 发送。", "When off, use Cmd/Ctrl+Enter to send.")),
                            self.send_on_enter,
                            pal,
                            |this| this.toggle_send_on_enter(),
                            cx,
                        ),
                        settings_toggle_row(
                            IconName::Sticker,
                            tr_str(&lang, "按表情推荐贴纸", "Suggest stickers by emoji"),
                            None,
                            self.suggest_stickers_by_emoji,
                            pal,
                            |this| {
                                this.suggest_stickers_by_emoji = !this.suggest_stickers_by_emoji;
                                this.chat_prefs_dirty = true;
                            },
                            cx,
                        ),
                        settings_toggle_row(
                            IconName::Sticker,
                            tr_str(&lang, "循环播放动态贴纸", "Loop animated stickers"),
                            None,
                            self.loop_animated_stickers,
                            pal,
                            |this| this.toggle_loop_animated_stickers(),
                            cx,
                        ),
                    ],
                ),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn test_wallpaper_label_ink_matches_preset_brightness() {
        for wp in WALLPAPER_PRESETS {
            let expected = if wp.is_dark { 0xFFFFFF } else { 0x18222D };
            assert_eq!(
                wallpaper_label_ink(wp.top_hex),
                expected,
                "unexpected contrast ink for wallpaper {} (0x{:06X})",
                wp.name,
                wp.top_hex
            );
        }
    }
}
