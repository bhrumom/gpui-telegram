use crate::i18n::tr_str;
use crate::theme::assets::{tg_icon_rgba, IconName};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_card_bg, settings_container, settings_group_card, settings_section_block,
};
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Debug, Clone)]
pub struct PremiumTier {
    pub months: u32,
    pub title: &'static str,
    pub price_per_month: f32,
    pub total_price: f32,
    pub discount_pct: u32,
}

pub struct SettingsPremiumSections {
    pub is_premium_active: bool,
    pub selected_months: u32,
    pub tiers: Vec<PremiumTier>,
    pub is_visible: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub pending_subscribe_months: Option<u32>,
}

impl SettingsPremiumSections {
    pub fn new() -> Self {
        Self {
            is_premium_active: true,
            selected_months: 12,
            tiers: vec![
                PremiumTier { months: 12, title: "Annual", price_per_month: 2.99, total_price: 35.88, discount_pct: 40 },
                PremiumTier { months: 6, title: "Semi-Annual", price_per_month: 3.99, total_price: 23.94, discount_pct: 20 },
                PremiumTier { months: 1, title: "Monthly", price_per_month: 4.99, total_price: 4.99, discount_pct: 0 },
            ],
            is_visible: true,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12),
            language_id: "en".into(),
            pending_subscribe_months: None,
        }
    }

    pub fn new_live(is_premium_active: bool, palette: TdesktopThemePalette, language_id: &str) -> Self {
        let mut s = Self::new();
        s.is_premium_active = is_premium_active;
        s.palette = palette;
        s.language_id = language_id.into();
        s
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, language_id: impl Into<String>) { self.language_id = language_id.into(); }
    pub fn select_tier(&mut self, months: u32) { self.selected_months = months; }

    pub fn subscribe(&mut self, months: u32) {
        self.selected_months = months;
        self.is_premium_active = true;
        self.pending_subscribe_months = Some(months);
    }

    pub fn toggle_visibility(&mut self) { self.is_visible = !self.is_visible; }
}

impl Default for SettingsPremiumSections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsPremiumSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let selected = self.selected_months;
        let is_active = self.is_premium_active;

        let perks = [
            (IconName::Paperclip, tr_str(&lang, "4 GB 超大文件上传", "4 GB Uploads"), tr_str(&lang, "单个文档与媒体体积上限提升至 4 GB", "Send documents and media up to 4 GB each")),
            (IconName::SendPlane, tr_str(&lang, "极速下载通道", "Faster Download Speed"), tr_str(&lang, "优先调度全球媒体与文件 CDN 节点", "Priority media and file CDN delivery")),
            (IconName::Mic, tr_str(&lang, "语音转文字实时识别", "Voice-to-Text Transcription"), tr_str(&lang, "语音与视频圆圈消息一键转写文本", "Instant transcripts for voice and video notes")),
            (IconName::PremiumStar, tr_str(&lang, "无限自定义表情回应", "Infinite Custom Reactions"), tr_str(&lang, "动态表情状态徽章与扩展互动贴纸", "Animated emoji status and extended reactions")),
            (IconName::FolderAll, tr_str(&lang, "高级对话分组管理", "Advanced Chat Management"), tr_str(&lang, "自定义默认分组与无限收藏夹标签", "Default folders and unlimited Saved Messages tags")),
        ];

        let plan_cards = div().flex().flex_row().gap(px(10.0)).children(self.tiers.iter().map(|tier| {
            let months = tier.months;
            let is_sel = months == selected;
            let tier_title = if lang.starts_with("zh") {
                match tier.title {
                    "Annual" => "年度方案",
                    "Semi-Annual" => "半年方案",
                    "Monthly" => "月度方案",
                    _ => tier.title,
                }
            } else {
                tier.title
            };
            div()
                .flex_1()
                .p(px(14.0))
                .rounded(px(12.0))
                .bg(if is_sel { ob.bg_control_hover() } else { settings_card_bg(pal) })
                .border_1()
                .border_color(if is_sel { ob.text_primary() } else { ob.frame_card_border() })
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.select_tier(months);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(pal.scaled_px(13.5)))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(tier_title),
                                )
                                .when(tier.discount_pct > 0, |el| {
                                    el.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(6.0))
                                            .bg(ob.bg_control())
                                            .text_size(px(pal.scaled_px(11.0)))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(ob.success())
                                            .child(format!("-{}%", tier.discount_pct)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(pal.scaled_px(13.0)))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(if lang.starts_with("zh") {
                                    format!("${:.2}/月", tier.price_per_month)
                                } else {
                                    format!("${:.2}/mo", tier.price_per_month)
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(pal.scaled_px(11.5)))
                                .text_color(ob.text_muted())
                                .child(if lang.starts_with("zh") {
                                    format!("合计 ${:.2}", tier.total_price)
                                } else {
                                    format!("${:.2} total", tier.total_price)
                                }),
                        ),
                )
        }));

        let perk_rows: Vec<AnyElement> = perks
            .into_iter()
            .map(|(icon, title, desc)| {
                div()
                    .w_full()
                    .px(px(pal.scaled_px(16.0)))
                    .py(px(pal.scaled_px(12.0)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(tg_icon_rgba(icon, pal.scaled_px(16.0), ob.text_muted()))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(pal.scaled_px(13.5)))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_primary())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(pal.scaled_px(12.0)))
                                    .text_color(ob.text_muted())
                                    .child(desc),
                            ),
                    )
                    .into_any_element()
            })
            .collect();

        settings_container(pal)
            .child(settings_section_block(
                tr_str(&lang, "订阅方案", "Plans"),
                Some(if is_active {
                    tr_str(&lang, "会员特权已生效", "Active Subscriber")
                } else {
                    tr_str(&lang, "订阅解锁全部专属功能与更高配额", "Subscribe to unlock exclusive features")
                }),
                pal,
                plan_cards,
            ))
            .child(settings_section_block(
                tr_str(&lang, "专属特权", "Included Features"),
                None,
                pal,
                settings_group_card(pal, perk_rows),
            ))
            .child(
                div()
                    .w_full()
                    .py(px(11.0))
                    .rounded(px(10.0))
                    .bg(ob.surface_light())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(pal.scaled_px(13.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ob.text_on_light())
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.subscribe(selected);
                            cx.notify();
                        }),
                    )
                    .child(if lang.starts_with("zh") {
                        if is_active {
                            format!("切换订阅周期 ({} 个月)", selected)
                        } else {
                            format!("立即开通会员 ({} 个月)", selected)
                        }
                    } else if is_active {
                        format!("Switch Plan ({} mo)", selected)
                    } else {
                        format!("Subscribe ({} mo)", selected)
                    }),
            )
            .into_any_element()
    }
}
