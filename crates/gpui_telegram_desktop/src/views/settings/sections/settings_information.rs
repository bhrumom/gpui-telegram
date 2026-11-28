//! Upstream Source: Telegram/SourceFiles/settings/sections/settings_information.cpp

use crate::i18n::tr_str;
use crate::theme::assets::{tg_icon_rgba, IconName};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_action_row, settings_action_row_desc, settings_card, settings_container,
    settings_group_card, settings_section_block,
};
use crate::views::settings::settings_type::SettingsSectionType;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub const K_SAVE_BIO_TIMEOUT_MS: u64 = 1000;
pub const K_STANDARD_BIO_LIMIT: usize = 70;
pub const K_PREMIUM_BIO_LIMIT: usize = 140;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileEditField {
    None,
    FirstName,
    LastName,
    Username,
    Phone,
    Bio,
}

#[derive(Debug, Clone)]
pub struct AccountEntry {
    pub account_id: u64,
    pub name: String,
    pub phone: String,
    pub unread_count: usize,
    pub is_active: bool,
}

#[derive(Debug, Clone)]
pub struct SettingsInformationSections {
    pub first_name: String,
    pub last_name: String,
    pub bio: String,
    pub username: String,
    pub phone: String,
    pub birthday: Option<String>,
    pub is_premium: bool,
    pub accounts: Vec<AccountEntry>,
    pub save_pending: bool,
    pub active_field: ProfileEditField,
    pub pending_save_profile: bool,
    pub pending_logout: bool,
    pub pending_nav_section: Option<SettingsSectionType>,
    pub pending_copy_text: Option<String>,
    pub status_message: Option<String>,
    pub is_visible: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl SettingsInformationSections {
    pub fn new() -> Self {
        Self {
            first_name: "Pavel".into(),
            last_name: "Durov".into(),
            bio: "Exploring GPUI and high-performance Rust interfaces".into(),
            username: "durov".into(),
            phone: "+1 234 567 8900".into(),
            birthday: Some("October 10".into()),
            is_premium: true,
            accounts: vec![
                AccountEntry { account_id: 1, name: "Pavel Durov".into(), phone: "+1 234 567 8900".into(), unread_count: 0, is_active: true },
                AccountEntry { account_id: 2, name: "Work Account".into(), phone: "+1 415 555 0138".into(), unread_count: 4, is_active: false },
            ],
            save_pending: false,
            active_field: ProfileEditField::None,
            pending_save_profile: false,
            pending_logout: false,
            pending_nav_section: None,
            pending_copy_text: None,
            status_message: None,
            is_visible: true,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12),
            language_id: "en".into(),
        }
    }

    pub fn new_live() -> Self {
        let mut s = Self::new();
        s.first_name.clear();
        s.last_name.clear();
        s.bio.clear();
        s.username.clear();
        s.phone.clear();
        s.birthday = None;
        s.is_premium = false;
        s.accounts.clear();
        s
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang: impl Into<String>) {
        self.language_id = lang.into();
    }

    pub fn sync_from_profile(
        &mut self,
        user_id: u64,
        first_name: String,
        last_name: String,
        username: String,
        phone: String,
        bio: String,
        is_premium: bool,
    ) {
        let full = format!("{} {}", first_name, last_name).trim().to_string();
        self.first_name = first_name;
        self.last_name = last_name;
        self.username = username;
        self.phone = phone.clone();
        self.bio = bio;
        self.is_premium = is_premium;
        self.save_pending = false;
        self.accounts = vec![AccountEntry {
            account_id: user_id,
            name: if full.is_empty() { format!("User #{}", user_id) } else { full },
            phone,
            unread_count: 0,
            is_active: true,
        }];
    }

    pub fn bio_limit(&self) -> usize {
        if self.is_premium { K_PREMIUM_BIO_LIMIT } else { K_STANDARD_BIO_LIMIT }
    }

    pub fn update_bio(&mut self, bio: &str) -> bool {
        if bio.len() > self.bio_limit() { false } else { self.set_bio(bio); true }
    }

    pub fn set_bio(&mut self, bio: impl Into<String>) {
        let b = bio.into();
        let limit = self.bio_limit();
        self.bio = b.chars().take(limit).collect();
        self.save_pending = true;
    }

    pub fn set_names(&mut self, first: impl Into<String>, last: impl Into<String>) {
        self.first_name = first.into();
        self.last_name = last.into();
        self.save_pending = true;
    }

    pub fn set_username(&mut self, username: impl Into<String>) {
        self.username = username.into().chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
        self.save_pending = true;
    }

    pub fn active_field_text(&self) -> Option<String> {
        match self.active_field {
            ProfileEditField::None => None,
            ProfileEditField::FirstName => Some(self.first_name.clone()),
            ProfileEditField::LastName => Some(self.last_name.clone()),
            ProfileEditField::Username => Some(self.username.clone()),
            ProfileEditField::Phone => Some(self.phone.clone()),
            ProfileEditField::Bio => Some(self.bio.clone()),
        }
    }

    pub fn push_field_text(&mut self, text: &str) {
        match self.active_field {
            ProfileEditField::None => {}
            ProfileEditField::FirstName => {
                self.first_name.push_str(text);
                self.save_pending = true;
            }
            ProfileEditField::LastName => {
                self.last_name.push_str(text);
                self.save_pending = true;
            }
            ProfileEditField::Username => {
                for ch in text.chars() {
                    if ch.is_alphanumeric() || ch == '_' {
                        self.username.push(ch);
                    }
                }
                self.save_pending = true;
            }
            ProfileEditField::Phone => {
                for ch in text.chars() {
                    if ch.is_ascii_digit() || ch == '+' || ch == ' ' || ch == '-' {
                        self.phone.push(ch);
                    }
                }
                self.save_pending = true;
            }
            ProfileEditField::Bio => {
                let limit = self.bio_limit();
                for ch in text.chars() {
                    if self.bio.chars().count() < limit {
                        self.bio.push(ch);
                    }
                }
                self.save_pending = true;
            }
        }
    }

    pub fn backspace_field_text(&mut self) {
        match self.active_field {
            ProfileEditField::None => {}
            ProfileEditField::FirstName => { self.first_name.pop(); self.save_pending = true; }
            ProfileEditField::LastName => { self.last_name.pop(); self.save_pending = true; }
            ProfileEditField::Username => { self.username.pop(); self.save_pending = true; }
            ProfileEditField::Phone => { self.phone.pop(); self.save_pending = true; }
            ProfileEditField::Bio => { self.bio.pop(); self.save_pending = true; }
        }
    }

    pub fn clear_field_text(&mut self) {
        match self.active_field {
            ProfileEditField::None => {}
            ProfileEditField::FirstName => { self.first_name.clear(); self.save_pending = true; }
            ProfileEditField::LastName => { self.last_name.clear(); self.save_pending = true; }
            ProfileEditField::Username => { self.username.clear(); self.save_pending = true; }
            ProfileEditField::Phone => { self.phone.clear(); self.save_pending = true; }
            ProfileEditField::Bio => { self.bio.clear(); self.save_pending = true; }
        }
    }

    pub fn cycle_field(&mut self) {
        self.active_field = match self.active_field {
            ProfileEditField::None | ProfileEditField::Bio => ProfileEditField::FirstName,
            ProfileEditField::FirstName => ProfileEditField::LastName,
            ProfileEditField::LastName => ProfileEditField::Username,
            ProfileEditField::Username => ProfileEditField::Phone,
            ProfileEditField::Phone => ProfileEditField::Bio,
        };
    }

    pub fn switch_account(&mut self, account_id: u64) {
        for acc in &mut self.accounts {
            acc.is_active = acc.account_id == account_id;
        }
    }

    pub fn toggle_visibility(&mut self) {
        self.is_visible = !self.is_visible;
    }
}

impl Default for SettingsInformationSections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsInformationSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let active_field = self.active_field;
        let bio_count = self.bio.chars().count();
        let bio_limit = self.bio_limit();
        let hover_ctrl = ob.bg_control_hover();

        let full_name = format!("{} {}", self.first_name.trim(), self.last_name.trim())
            .trim()
            .to_string();
        let display_name = if full_name.is_empty() {
            if !self.username.is_empty() {
                format!("@{}", self.username)
            } else {
                tr_str(&lang, "我的账号", "My Account").to_string()
            }
        } else {
            full_name
        };
        let avatar_initial: String = display_name
            .chars()
            .find(|c| !c.is_whitespace() && *c != '@')
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "U".to_string());
        let handle_meta = match (!self.username.is_empty(), !self.phone.is_empty()) {
            (true, true) => format!("@{} · {}", self.username, self.phone),
            (true, false) => format!("@{}", self.username),
            (false, true) => self.phone.clone(),
            (false, false) => tr_str(&lang, "未设置用户名或手机号", "No username or phone set").to_string(),
        };

        let render_edit_box = |field: ProfileEditField,
                               label: &str,
                               value: &str,
                               placeholder: &str,
                               cx: &mut Context<Self>|
         -> AnyElement {
            let is_focused = active_field == field;
            let shown = if value.is_empty() {
                if is_focused {
                    "|".to_string()
                } else {
                    placeholder.to_string()
                }
            } else if is_focused {
                format!("{}|", value)
            } else {
                value.to_string()
            };
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_focused {
                            ob.text_primary()
                        } else {
                            ob.text_muted()
                        })
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .w_full()
                        .min_h(px(36.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(ob.bg_control())
                        .border_1()
                        .border_color(if is_focused {
                            ob.text_primary()
                        } else {
                            ob.border_hairline()
                        })
                        .cursor_text()
                        .flex()
                        .items_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _e, _w, cx| {
                                this.active_field = field;
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(if value.is_empty() && !is_focused {
                                    ob.text_muted()
                                } else {
                                    ob.text_primary()
                                })
                                .child(shown),
                        ),
                )
                .into_any_element()
        };

        let first_el = render_edit_box(
            ProfileEditField::FirstName,
            tr_str(&lang, "名字", "First Name"),
            &self.first_name.clone(),
            tr_str(&lang, "输入名字", "Enter first name"),
            cx,
        );
        let last_el = render_edit_box(
            ProfileEditField::LastName,
            tr_str(&lang, "姓氏", "Last Name"),
            &self.last_name.clone(),
            tr_str(&lang, "输入姓氏（可选）", "Enter last name (optional)"),
            cx,
        );
        let user_el = render_edit_box(
            ProfileEditField::Username,
            tr_str(&lang, "用户名", "Username"),
            &self.username.clone(),
            tr_str(&lang, "输入用户名", "Enter username"),
            cx,
        );
        let phone_el = render_edit_box(
            ProfileEditField::Phone,
            tr_str(&lang, "手机号码", "Phone Number"),
            &self.phone.clone(),
            "+86 138 0000 0000",
            cx,
        );
        let bio_label = format!("{} ({}/{})", tr_str(&lang, "个人简介", "Bio"), bio_count, bio_limit);
        let bio_el = render_edit_box(
            ProfileEditField::Bio,
            &bio_label,
            &self.bio.clone(),
            tr_str(&lang, "填写几句关于你自己的介绍…", "Add a few words about yourself..."),
            cx,
        );

        let username_for_copy = self.username.clone();
        let has_username = !username_for_copy.is_empty();

        let hero_card = settings_card(pal).child(
            div()
                .w_full()
                .py(px(4.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .child(
                            div()
                                .w(px(46.0))
                                .h(px(46.0))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(ob.bg_control_active())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(18.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(avatar_initial),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(16.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .truncate()
                                        .child(display_name),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(ob.text_muted())
                                        .truncate()
                                        .child(handle_meta),
                                ),
                        ),
                )
                .when(has_username, |el| {
                    el.child(
                        div()
                            .id("settings-profile-copy-handle")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(11.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_control())
                            .border_1()
                            .border_color(ob.border_strong_hairline())
                            .hover(move |s| s.bg(hover_ctrl))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let handle = format!("@{}", username_for_copy);
                                    this.pending_copy_text = Some(handle.clone());
                                    this.status_message = Some(if this.language_id.starts_with("zh") {
                                        format!("已复制 {handle}")
                                    } else {
                                        format!("Copied {handle}")
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Copy, 12.5, ob.text_muted()))
                            .child(tr_str(&lang, "复制用户名", "Copy handle")),
                    )
                }),
        );

        let form_card = settings_card(pal)
            .gap(px(12.0))
            .py(px(16.0))
            .child(div().w_full().flex().gap(px(12.0)).child(first_el).child(last_el))
            .child(div().w_full().flex().gap(px(12.0)).child(user_el).child(phone_el))
            .child(bio_el)
            .when_some(self.status_message.clone(), |el, msg| {
                el.child(div().text_size(px(12.0)).text_color(ob.success()).child(msg))
            })
            .child(
                div()
                    .pt(px(4.0))
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .id("settings-info-save-btn")
                            .px(px(16.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(ob.surface_light())
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_on_light())
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _e, _w, cx| {
                                    this.pending_save_profile = true;
                                    this.save_pending = false;
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(&lang, "保存修改", "Save changes")),
                    ),
            );

        let account_rows: Vec<AnyElement> = self
            .accounts
            .clone()
            .into_iter()
            .map(|acc| {
                let id = acc.account_id;
                let badge = if acc.is_active {
                    tr_str(&lang, "当前在线", "Active").to_string()
                } else {
                    acc.phone
                };
                settings_action_row(
                    IconName::ContactsUsers,
                    &acc.name,
                    &badge,
                    pal,
                    false,
                    move |this| {
                        this.switch_account(id);
                    },
                    cx,
                )
            })
            .collect();

        let premium_status = if self.is_premium {
            tr_str(&lang, "已开通", "Active")
        } else {
            tr_str(&lang, "查看方案", "Plans")
        };

        settings_container(pal)
            .child(hero_card)
            .child(settings_section_block(
                tr_str(&lang, "基本信息", "Profile Details"),
                Some(tr_str(
                    &lang,
                    "点击任意栏位即可编辑，保存后实时同步到账号。",
                    "Click any field to edit and sync to your account.",
                )),
                pal,
                form_card,
            ))
            .when(!account_rows.is_empty(), |el| {
                el.child(settings_section_block(
                    tr_str(&lang, "已登录账号", "Accounts"),
                    None,
                    pal,
                    settings_group_card(pal, account_rows),
                ))
            })
            .child(settings_section_block(
                tr_str(&lang, "会员与扩展", "Membership & Extras"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_action_row_desc(
                            Some(IconName::PremiumStar),
                            tr_str(&lang, "高级会员", "Premium"),
                            Some(tr_str(
                                &lang,
                                "更大文件上传、加速下载与专属特权。",
                                "Higher upload limits, faster downloads, and exclusive perks.",
                            )),
                            premium_status,
                            pal,
                            false,
                            |this| {
                                this.pending_nav_section = Some(SettingsSectionType::Premium);
                            },
                            cx,
                        ),
                        settings_action_row_desc(
                            Some(IconName::WalletStars),
                            tr_str(&lang, "星币钱包", "Stars"),
                            Some(tr_str(
                                &lang,
                                "查看可用余额、创作者收益与交易记录。",
                                "Balance, creator rewards, and transaction history.",
                            )),
                            "",
                            pal,
                            false,
                            |this| {
                                this.pending_nav_section = Some(SettingsSectionType::Credits);
                            },
                            cx,
                        ),
                        settings_action_row_desc(
                            Some(IconName::ChannelMegaphone),
                            tr_str(&lang, "商业功能", "Business"),
                            Some(tr_str(
                                &lang,
                                "营业时间、快捷回复、欢迎语与自动离开回复。",
                                "Opening hours, quick replies, greetings, and away messages.",
                            )),
                            "",
                            pal,
                            false,
                            |this| {
                                this.pending_nav_section = Some(SettingsSectionType::Business);
                            },
                            cx,
                        ),
                    ],
                ),
            ))
            .child(settings_section_block(
                tr_str(&lang, "登录状态", "Session"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![settings_action_row_desc(
                        Some(IconName::Close),
                        tr_str(&lang, "退出当前账号", "Log Out"),
                        Some(tr_str(
                            &lang,
                            "退出登录并清除本机当前会话凭证。",
                            "Sign out and clear local session credentials on this device.",
                        )),
                        "",
                        pal,
                        true,
                        |this| {
                            this.pending_logout = true;
                        },
                        cx,
                    )],
                ),
            ))
            .into_any_element()
    }
}
