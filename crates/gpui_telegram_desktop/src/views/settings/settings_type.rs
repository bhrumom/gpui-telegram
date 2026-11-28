//! Upstream Source: Telegram/SourceFiles/settings/settings_type.h
//! Subsystem: settings | Feature ID: TCG-N31 | Disposition: rust_domain_or_server_1to1

use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Section type enumeration identifying every distinct Settings destination
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SettingsSectionType {
    Main,
    Information,
    Notifications,
    NotificationsType,
    NotificationsReactions,
    PrivacySecurity,
    BlockedPeers,
    ActiveSessions,
    Chat,
    Folders,
    Advanced,
    Calls,
    Credits,
    Premium,
    Shortcuts,
    Websites,
    Passkeys,
    LocalPasscode,
    GlobalTtl,
    PowerSaving,
    LocalStorage,
    Business,
    Computers,
    AiProviders,
    Language,
    Search,
}

impl SettingsSectionType {
    pub fn id_str(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Information => "information",
            Self::Notifications => "notifications",
            Self::NotificationsType => "notifications_type",
            Self::NotificationsReactions => "notifications_reactions",
            Self::PrivacySecurity => "privacy",
            Self::BlockedPeers => "blocked_peers",
            Self::ActiveSessions => "active_sessions",
            Self::Chat => "chat",
            Self::Folders => "folders",
            Self::Advanced => "advanced",
            Self::Calls => "calls",
            Self::Credits => "credits",
            Self::Premium => "premium",
            Self::Shortcuts => "shortcuts",
            Self::Websites => "websites",
            Self::Passkeys => "passkeys",
            Self::LocalPasscode => "local_passcode",
            Self::GlobalTtl => "global_ttl",
            Self::PowerSaving => "power_saving",
            Self::LocalStorage => "local_storage",
            Self::Business => "business",
            Self::Computers => "computers",
            Self::AiProviders => "ai_providers",
            Self::Language => "language",
            Self::Search => "search",
        }
    }

    pub fn display_title(&self) -> &'static str {
        self.display_title_localized(false)
    }

    pub fn display_title_localized(&self, is_chinese: bool) -> &'static str {
        if is_chinese {
            match self {
                Self::Main => "设置",
                Self::Information => "个人资料",
                Self::Notifications => "通知",
                Self::NotificationsType => "分类通知",
                Self::NotificationsReactions => "互动回应通知",
                Self::PrivacySecurity => "隐私与安全",
                Self::BlockedPeers => "已屏蔽用户",
                Self::ActiveSessions => "活跃设备与会话",
                Self::Chat => "外观",
                Self::Folders => "对话分组",
                Self::Advanced => "高级与更新",
                Self::Calls => "语音与通话",
                Self::Credits => "星币钱包",
                Self::Premium => "高级会员",
                Self::Shortcuts => "键盘",
                Self::Websites => "已登录网站",
                Self::Passkeys => "通行密钥",
                Self::LocalPasscode => "应用锁",
                Self::GlobalTtl => "自动删除消息",
                Self::PowerSaving => "省电模式",
                Self::LocalStorage => "数据与存储",
                Self::Business => "商业功能",
                Self::Computers => "电脑",
                Self::AiProviders => "服务商",
                Self::Language => "语言",
                Self::Search => "搜索设置",
            }
        } else {
            match self {
                Self::Main => "Settings",
                Self::Information => "Profile",
                Self::Notifications => "Notifications",
                Self::NotificationsType => "Notification Scope",
                Self::NotificationsReactions => "Reaction Alerts",
                Self::PrivacySecurity => "Privacy & Security",
                Self::BlockedPeers => "Blocked Users",
                Self::ActiveSessions => "Active Sessions",
                Self::Chat => "Appearance",
                Self::Folders => "Chat Folders",
                Self::Advanced => "Advanced & Updates",
                Self::Calls => "Voice & Calls",
                Self::Credits => "Stars",
                Self::Premium => "Premium",
                Self::Shortcuts => "Keyboard",
                Self::Websites => "Logged in Websites",
                Self::Passkeys => "Passkeys",
                Self::LocalPasscode => "Passcode Lock",
                Self::GlobalTtl => "Auto-Delete Messages",
                Self::PowerSaving => "Power Saving",
                Self::LocalStorage => "Storage & Cache",
                Self::Business => "Business",
                Self::Computers => "Computers",
                Self::AiProviders => "Providers",
                Self::Language => "Language",
                Self::Search => "Search Settings",
            }
        }
    }

    /// Concise OpenBot-style label displayed in the left navigation rail, page headings, and breadcrumbs.
    pub fn nav_title_localized(&self, is_chinese: bool) -> &'static str {
        match (self, is_chinese) {
            (Self::Main, true) => "通用",
            (Self::Main, false) => "General",
            (Self::Websites, true) => "已授权网站",
            (Self::Websites, false) => "Connected Websites",
            _ => self.display_title_localized(is_chinese),
        }
    }

    /// Concise one-line OpenBot-style page description shown beneath the page heading.
    pub fn subtitle_localized(&self, is_chinese: bool) -> &'static str {
        if is_chinese {
            match self {
                Self::Main => "管理语言、界面缩放、对话分组与默认服务商。",
                Self::Chat => "自定义颜色主题、强调色、聊天壁纸与对话交互方式。",
                Self::Notifications => "选择应用如何通过提醒、角标与声音通知你。",
                Self::NotificationsType => "按对话类型微调提醒、消息预览与例外列表。",
                Self::NotificationsReactions => "控制消息与动态表情回应的通知范围和提示音。",
                Self::Calls => "配置麦克风、扬声器、摄像头与点对点通话策略。",
                Self::Shortcuts => "查看与管理此设备上的全局和工作区键盘快捷键。",
                Self::Information => "管理你的个人资料、已登录账号与会员扩展。",
                Self::PrivacySecurity => "管理登录设备、密码锁、零知识保险库与隐私可见范围。",
                Self::BlockedPeers => "管理已屏蔽的联系人、机器人与黑名单用户。",
                Self::ActiveSessions => "查看当前登录的所有设备并远程下线其他会话。",
                Self::LocalPasscode => "为桌面端启用本地锁定密码与空闲自动锁定。",
                Self::Passkeys => "使用生物识别或安全密钥实现无密码安全登录。",
                Self::GlobalTtl => "为新发起的对话设置默认消息定时销毁周期。",
                Self::Websites => "查看并撤销第三方网站的登录授权。",
                Self::AiProviders => "管理 AI 订阅、模型供应商、路由组与脱敏导入。",
                Self::Computers => "连接并管理本地与云端远程电脑及协助会话。",
                Self::Advanced => "检查软件更新、管理本地存储、网络代理与系统集成。",
                Self::LocalStorage => "查看媒体缓存磁盘占用、配额上限并清理临时文件。",
                Self::PowerSaving => "在低电量时自动减少动画与媒体自动播放以节省能耗。",
                Self::Folders => "创建自定义对话分组，按工作与个人场景快速切换。",
                Self::Language => "选择界面显示语言，修改后立即生效并持久保存。",
                Self::Premium => "解锁更大文件上传、加速下载与专属会员权益。",
                Self::Credits => "查看星币余额、创作者收益与交易记录。",
                Self::Business => "配置营业时间、快捷回复与自动问候等商业工具。",
                Self::Search => "快速检索并跳转到任意设置项。",
            }
        } else {
            match self {
                Self::Main => "Choose language, interface scale, chat folders, and default AI provider.",
                Self::Chat => "Customize theme, accent color, chat wallpaper, typography, and sending behavior.",
                Self::Notifications => "Choose how Fabushi alerts you with badges, previews, and sounds.",
                Self::NotificationsType => "Fine-tune alerts, previews, and per-chat notification exceptions.",
                Self::NotificationsReactions => "Control alerts and tones for message and story reactions.",
                Self::Calls => "Configure microphone, speaker, camera, and peer-to-peer call preferences.",
                Self::Shortcuts => "View and toggle workspace keyboard shortcuts on this device.",
                Self::Information => "Manage how you appear, connected accounts, and membership extras.",
                Self::PrivacySecurity => "Manage active devices, locks, zero-knowledge vault, and visibility rules.",
                Self::BlockedPeers => "Review and unblock restricted contacts and bots.",
                Self::ActiveSessions => "Inspect signed-in devices and terminate remote sessions.",
                Self::LocalPasscode => "Protect the desktop app with a local passcode and idle auto-lock.",
                Self::Passkeys => "Sign in securely without passwords using biometrics or security keys.",
                Self::GlobalTtl => "Set a default self-destruct timer for messages in all new chats.",
                Self::Websites => "Review and revoke third-party website login authorizations.",
                Self::AiProviders => "Manage AI subscriptions, model providers, routing, and privacy redaction.",
                Self::Computers => "Connect and manage remote computers, terminals, and assist sessions.",
                Self::Advanced => "Keep Fabushi up to date, manage storage, proxy, and system integration.",
                Self::LocalStorage => "Inspect media cache disk usage, limits, and clear cached files.",
                Self::PowerSaving => "Reduce animations and media autoplay to conserve battery life.",
                Self::Folders => "Organize chats into custom folders and quickly switch between them.",
                Self::Language => "Choose the display language for menus, buttons, and messages.",
                Self::Premium => "Unlock higher upload limits, faster downloads, and exclusive perks.",
                Self::Credits => "Check Stars balance, creator rewards, and transaction history.",
                Self::Business => "Configure opening hours, quick replies, and automated messages.",
                Self::Search => "Search and jump directly to any settings destination.",
            }
        }
    }

    /// Returns `true` if this section is one of the 10 top-level destinations shown in the left rail.
    pub fn is_top_level_nav_tab(&self) -> bool {
        matches!(
            self,
            Self::Main
                | Self::Chat
                | Self::Notifications
                | Self::Calls
                | Self::Shortcuts
                | Self::Information
                | Self::PrivacySecurity
                | Self::AiProviders
                | Self::Computers
                | Self::Advanced
        )
    }

    /// Maps any settings section (including drill-down sub-pages) to its owning top-level sidebar tab.
    pub fn nav_parent(&self) -> Self {
        match self {
            Self::Language | Self::Folders | Self::Search => Self::Main,
            Self::NotificationsType | Self::NotificationsReactions => Self::Notifications,
            Self::Premium | Self::Credits | Self::Business => Self::Information,
            Self::BlockedPeers
            | Self::ActiveSessions
            | Self::LocalPasscode
            | Self::Passkeys
            | Self::GlobalTtl
            | Self::Websites => Self::PrivacySecurity,
            Self::LocalStorage | Self::PowerSaving => Self::Advanced,
            other => *other,
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Main => "⚙️",
            Self::Information => "👤",
            Self::Notifications | Self::NotificationsType | Self::NotificationsReactions => "🔔",
            Self::PrivacySecurity | Self::LocalPasscode | Self::Passkeys => "🔒",
            Self::BlockedPeers => "🚫",
            Self::ActiveSessions => "💻",
            Self::GlobalTtl => "⏳",
            Self::Chat => "💬",
            Self::Folders => "📁",
            Self::Advanced => "🛠️",
            Self::LocalStorage => "💾",
            Self::PowerSaving => "🔋",
            Self::Calls => "📞",
            Self::Credits => "⭐",
            Self::Premium => "👑",
            Self::Shortcuts => "⌨️",
            Self::Websites => "🌐",
            Self::Business => "💼",
            Self::Computers => "💻",
            Self::AiProviders => "🤖",
            Self::Language => "🗣️",
            Self::Search => "🔍",
        }
    }

    pub fn icon_name(&self) -> IconName {
        match self {
            Self::Main => IconName::SettingsGear,
            Self::Advanced => IconName::ArrowUp,
            Self::Information => IconName::ContactsUsers,
            Self::Notifications | Self::NotificationsType => IconName::Speaker,
            Self::NotificationsReactions => IconName::HeartReaction,
            Self::PrivacySecurity | Self::LocalPasscode | Self::Passkeys => IconName::Lock,
            Self::BlockedPeers => IconName::Close,
            Self::ActiveSessions => IconName::ComputerDesktop,
            Self::GlobalTtl => IconName::ClockSending,
            Self::Chat => IconName::NightMoon,
            Self::Folders => IconName::FolderAll,
            Self::LocalStorage => IconName::FileDoc,
            Self::PowerSaving => IconName::SunDay,
            Self::Calls => IconName::Mic,
            Self::Credits => IconName::WalletStars,
            Self::Premium => IconName::PremiumStar,
            Self::Shortcuts => IconName::TerminalMcp,
            Self::Websites => IconName::CommunityGlobe,
            Self::Business => IconName::ChannelMegaphone,
            Self::Computers => IconName::ComputerDesktop,
            Self::AiProviders => IconName::HubSparkles,
            Self::Language => IconName::Translate,
            Self::Search => IconName::Search,
        }
    }

    pub fn is_root(&self) -> bool {
        matches!(self, Self::Main)
    }

    pub fn parent_section(&self) -> Option<Self> {
        match self {
            Self::Main => None,
            Self::BlockedPeers
            | Self::ActiveSessions
            | Self::LocalPasscode
            | Self::Passkeys
            | Self::GlobalTtl
            | Self::Websites => Some(Self::PrivacySecurity),
            Self::NotificationsType | Self::NotificationsReactions => Some(Self::Notifications),
            Self::LocalStorage | Self::PowerSaving => Some(Self::Advanced),
            _ => Some(Self::Main),
        }
    }

    pub fn breadcrumb_path(&self) -> Vec<Self> {
        let mut path = Vec::new();
        let mut curr = Some(*self);
        while let Some(sec) = curr {
            path.push(sec);
            curr = sec.parent_section();
        }
        path.reverse();
        path
    }
}

impl fmt::Display for SettingsSectionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_title())
    }
}

/// SettingsType view displaying section identity and navigation context
#[derive(Debug, Clone)]
pub struct SettingsType {
    pub current_section: SettingsSectionType,
    pub title: String,
    pub is_visible: bool,
    pub palette: TdesktopThemePalette,
}

impl SettingsType {
    pub fn new() -> Self {
        Self {
            current_section: SettingsSectionType::Main,
            title: SettingsSectionType::Main.display_title().into(),
            is_visible: true,
            palette: TdesktopThemePalette::default(),
        }
    }

    pub fn for_section(section: SettingsSectionType) -> Self {
        Self {
            current_section: section,
            title: section.display_title().into(),
            is_visible: true,
            palette: TdesktopThemePalette::default(),
        }
    }

    pub fn with_palette(mut self, palette: TdesktopThemePalette) -> Self {
        self.palette = palette;
        self
    }

    pub fn set_section(&mut self, section: SettingsSectionType) {
        self.current_section = section;
        self.title = section.display_title().into();
    }

    pub fn toggle_visibility(&mut self) {
        self.is_visible = !self.is_visible;
    }
}

impl Default for SettingsType {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for SettingsType {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px(px(16.0))
            .py(px(8.0))
            .bg(rgb(pal.top_bar_bg))
            .border_b_1()
            .border_color(rgb(pal.border_fg))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(tg_icon(self.current_section.icon_name(), 16.0, pal.accent))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(pal.input_bg))
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("#{}", self.current_section.id_str())),
                    ),
            )
            .into_any_element()
    }
}
