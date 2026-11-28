use crate::i18n::tr_str;
use crate::theme::assets::{tg_icon_rgba, IconName};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_action_row_desc, settings_card, settings_container, settings_empty_state,
    settings_group_card, settings_section_block, settings_static_row,
};
use gpui::*;

pub const K_FIRST_PAGE_LIMIT: usize = 20;

#[derive(Debug, Clone)]
pub struct StarsTransaction {
    pub id: String,
    pub title: String,
    pub date: String,
    pub amount: i64,
    pub is_outgoing: bool,
}

pub struct SettingsCreditsSections {
    pub balance_stars: u64,
    pub currency_code: String,
    pub transactions: Vec<StarsTransaction>,
    pub ton_balance: f64,
    pub is_withdrawal_enabled: bool,
    pub is_visible: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub pending_topup_stars: Option<u64>,
    pub pending_withdraw: bool,
}

impl SettingsCreditsSections {
    pub fn new() -> Self {
        Self {
            balance_stars: 1500,
            currency_code: "XTR".into(),
            transactions: vec![
                StarsTransaction { id: "tx_1".into(), title: "Bot Payment (@ShopBot)".into(), date: "Today, 14:20".into(), amount: -250, is_outgoing: true },
                StarsTransaction { id: "tx_2".into(), title: "Purchased Stars".into(), date: "Yesterday, 10:15".into(), amount: 1000, is_outgoing: false },
                StarsTransaction { id: "tx_3".into(), title: "Creator Channel Subscription".into(), date: "Oct 12, 2024".into(), amount: -100, is_outgoing: true },
            ],
            ton_balance: 12.45,
            is_withdrawal_enabled: true,
            is_visible: true,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12),
            language_id: "en".into(),
            pending_topup_stars: None,
            pending_withdraw: false,
        }
    }

    pub fn new_live(palette: TdesktopThemePalette, language_id: &str) -> Self {
        Self {
            balance_stars: 0,
            currency_code: "XTR".into(),
            transactions: Vec::new(),
            ton_balance: 0.0,
            is_withdrawal_enabled: false,
            is_visible: true,
            palette,
            language_id: language_id.into(),
            pending_topup_stars: None,
            pending_withdraw: false,
        }
    }

    pub fn sync_wallet(&mut self, stars: u64, ton_balance: f64, transactions: Vec<StarsTransaction>) {
        self.balance_stars = stars;
        self.ton_balance = ton_balance;
        self.is_withdrawal_enabled = stars >= 1000 || ton_balance > 0.0;
        self.transactions = transactions;
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, language_id: impl Into<String>) { self.language_id = language_id.into(); }
    pub fn add_stars(&mut self, amount: u64) { self.balance_stars += amount; }
    pub fn can_withdraw(&self) -> bool { self.is_withdrawal_enabled && self.balance_stars >= 1000 }

    pub fn topup(&mut self, amount: u64) {
        self.balance_stars += amount;
        self.pending_topup_stars = Some(amount);
        let lang = self.language_id.as_str();
        self.transactions.insert(0, StarsTransaction {
            id: format!("tx_{}", self.transactions.len() + 1),
            title: tr_str(lang, "星币充值", "Stars Top-Up").into(),
            date: tr_str(lang, "刚刚", "Just now").into(),
            amount: amount as i64,
            is_outgoing: false,
        });
    }

    pub fn credits(&self) -> u64 { self.balance_stars }
    pub fn toggle_visibility(&mut self) { self.is_visible = !self.is_visible; }
}

impl Default for SettingsCreditsSections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsCreditsSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let balance = self.balance_stars;
        let txs = self.transactions.clone();

        let ton_desc = if lang.starts_with("zh") {
            format!("可提取 {:.2} TON", self.ton_balance)
        } else {
            format!("{:.2} TON available", self.ton_balance)
        };

        let tx_body: AnyElement = if txs.is_empty() {
            settings_card(pal)
                .child(settings_empty_state(
                    IconName::WalletStars,
                    tr_str(&lang, "暂无星币交易记录", "No Stars transactions yet."),
                    pal,
                ))
                .into_any_element()
        } else {
            let rows: Vec<AnyElement> = txs
                .into_iter()
                .map(|tx| {
                    let amount_fg = if tx.is_outgoing {
                        ob.danger()
                    } else {
                        ob.success()
                    };
                    let sign = if tx.amount > 0 { "+" } else { "" };
                    settings_static_row(
                        &tx.title,
                        Some(&tx.date),
                        div()
                            .text_size(px(pal.scaled_px(13.0)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(amount_fg)
                            .child(format!("{}{} ★", sign, tx.amount)),
                        pal,
                    )
                })
                .collect();
            settings_group_card(pal, rows)
        };

        settings_container(pal)
            .child(settings_section_block(
                tr_str(&lang, "余额", "Balance"),
                Some(tr_str(
                    &lang,
                    "数字商品支付、频道订阅与创作者激励",
                    "Digital goods, channel subscriptions, and creator rewards",
                )),
                pal,
                settings_group_card(
                    pal,
                    vec![
                        div()
                            .w_full()
                            .px(px(pal.scaled_px(16.0)))
                            .py(px(pal.scaled_px(14.0)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(pal.scaled_px(12.0)))
                                            .text_color(ob.text_muted())
                                            .child(tr_str(&lang, "可用余额", "Available Balance")),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(tg_icon_rgba(
                                                IconName::PremiumStar,
                                                20.0,
                                                ob.text_primary(),
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(pal.scaled_px(22.0)))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(ob.text_primary())
                                                    .child(format!("{} {}", balance, self.currency_code)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(8.0))
                                    .bg(ob.surface_light())
                                    .text_size(px(pal.scaled_px(12.5)))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_on_light())
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.topup(500);
                                            cx.notify();
                                        }),
                                    )
                                    .child(format!("+ {}", tr_str(&lang, "购买星币", "Buy Stars"))),
                            )
                            .into_any_element(),
                        settings_action_row_desc(
                            Some(IconName::WalletStars),
                            tr_str(&lang, "创作者收益 (TON)", "Creator Rewards (TON)"),
                            Some(&ton_desc),
                            if self.can_withdraw() {
                                tr_str(&lang, "提现", "Withdraw")
                            } else {
                                tr_str(&lang, "未达门槛", "Unavailable")
                            },
                            pal,
                            false,
                            |this| {
                                if this.can_withdraw() {
                                    this.pending_withdraw = true;
                                }
                            },
                            cx,
                        ),
                    ],
                ),
            ))
            .child(settings_section_block(
                tr_str(&lang, "交易记录", "Transaction History"),
                None,
                pal,
                tx_body,
            ))
            .into_any_element()
    }
}
