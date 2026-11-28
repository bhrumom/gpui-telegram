//! Lazy-materialized Settings Router (`SettingsRouter` + `SettingsRouterEvent`).
//! Eliminates eager startup allocation of all 23 settings section entities by
//! initializing each section on demand, hydrating from `AppPreferences`,
//! applying active palette/locale, and wiring `cx.observe` subscriptions.

use crate::app::preferences::AppPreferences;
use crate::app::updater::DesktopUpdateSnapshot;
use crate::components::ui::TextInputController;
use crate::theme::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::settings::sections::{
    settings_ai_providers::{AiProviderField, SettingsAiProvidersEvent},
    settings_computers::{
        AssistInputField, CloudFormField, SettingsComputersEvent, SettingsComputersPage,
        SettingsComputersTab,
    },
    settings_folders::ChatFolderItem,
    settings_information::ProfileEditField,
    settings_local_passcode::AutoLockTimeout,
    settings_notifications::BadgeCountType,
    settings_notifications_reactions::ReactionsNotifySource,
    settings_privacy_security::SettingsVaultAction,
    SettingsActiveSessionsSections, SettingsAdvancedSections, SettingsAiProvidersSections,
    SettingsBlockedPeersSections, SettingsBusinessSections, SettingsCallsSections,
    SettingsChatSections, SettingsComputersSections, SettingsCreditsSections,
    SettingsFoldersSections, SettingsGlobalTtlSections, SettingsInformationSections,
    SettingsLocalPasscodeSections, SettingsLocalStorageSections, SettingsMainSections,
    SettingsNotificationsReactionsSections, SettingsNotificationsSections,
    SettingsNotificationsTypeSections, SettingsPasskeysSections, SettingsPremiumSections,
    SettingsPrivacySecuritySections, SettingsShortcutsSections, SettingsWebsitesSections,
};
use crate::views::settings::settings_power_saving::SettingsPowerSaving;
use crate::views::settings::settings_search::SettingsSearch;
use crate::views::settings::settings_type::SettingsSectionType;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter};
use zeroize::Zeroize as _;

/// High-level events emitted by `SettingsRouter` to `AppShell`.
#[derive(Debug, Clone)]
pub enum SettingsRouterEvent {
    CloseRequested,
    LogoutRequested,
    LockAppNow,
    CopyClipboard(String),
    UpdateInterfaceScale(u32),
    OpenModal {
        title: String,
        kind: ModalDialogKind,
        width: f32,
        height: f32,
    },
    ApplyTheme {
        mode: TdesktopThemeMode,
        accent: u32,
        wallpaper: String,
    },
    UpdateChatPrefs {
        font_size: u32,
        bubble_radius: u32,
        send_on_enter: bool,
        suggest_stickers: bool,
        loop_animated_stickers: bool,
        blur_wallpaper: bool,
    },
    SaveProfile {
        first_name: String,
        last_name: String,
        username: Option<String>,
        phone: String,
        bio: String,
    },
    TwoFactorMutation(Option<String>),
    PushUserSettings,
    RevokeSession(String),
    RevokeOtherSessions,
    UnblockPeer {
        user_id: u64,
        remaining: usize,
    },
    UpdateLocalPasscode {
        disable: bool,
        hash: Option<String>,
        auto_lock_seconds: u32,
    },
    UpdateGlobalTtl(u32),
    ApplyGlobalTtlToExistingChats(u32),
    UpdateNotificationPrefs {
        private_chats: bool,
        groups: bool,
        channels: bool,
        sound_enabled: bool,
        flash_taskbar: bool,
        include_muted_in_badge: bool,
        badge_count_messages: bool,
    },
    UpdateNotificationReactionsPrefs {
        notify_reactions: bool,
        notify_stories: bool,
        contacts_only: bool,
        sound_enabled: bool,
    },
    UpdateNotificationTypePrefs {
        alerts_enabled: bool,
        preview_enabled: bool,
        sound_enabled: bool,
    },
    UpdateFolderTags(bool),
    CreateFolder {
        id: u32,
        title: String,
    },
    DeleteFolder(u32),
    UpdateAdvancedPrefs {
        ask_download_path: bool,
        opengl_enabled: bool,
        native_window_frame: bool,
        tray_icon_enabled: bool,
        auto_update_enabled: bool,
    },
    OpenExportDataModal,
    UpdateLocalStoragePrefs {
        max_cache_gb: u32,
        retention_weeks: u32,
    },
    ClearLocalCache,
    UpdateCallPrefs {
        input_device: String,
        output_device: String,
        camera_device: String,
    },
    ToggleMicTest(bool),
    UpdatePowerSavingPrefs {
        enabled: bool,
        threshold_pct: u32,
        animate_stickers_chat: bool,
        animate_stickers_panel: bool,
        animate_emoji_panel: bool,
        autoplay_gifs: bool,
        autoplay_videos: bool,
    },
    TopupStars(u32),
    WithdrawStars,
    ActivatePremium(u32),
    UpdatePasskeysPrefs(Vec<String>),
    UpdateWebsitesPrefs(Vec<String>),
    UpdateShortcutsPrefs {
        enabled: bool,
        filter: String,
    },
    UpdateBusinessPrefs(Vec<String>),
    VaultCustody(SettingsVaultAction),
    Computers(SettingsComputersEvent),
    AiProviders(SettingsAiProvidersEvent),
    SelectLanguage(&'static str),
    ToggleLanguage,
    Navigated(SettingsSectionType),
    ActiveInputFieldChanged,
    CheckForUpdates,
    TriggerOneClickUpdate,
    ShowToast(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyEditField {
    TwoFactorPassword,
    RecoveryKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsActiveInputKey {
    Information(ProfileEditField),
    Privacy(PrivacyEditField),
    FoldersTitle,
    ComputersCloudForm {
        field: CloudFormField,
        form_gen: u64,
    },
    ComputersAssist(AssistInputField),
    AiProviders {
        field: AiProviderField,
        editor_gen: u64,
    },
    SearchQuery,
    NavSearchQuery,
}

/// Maximum number of instant cross-tab search results shown in Settings left nav
/// (matches OpenBot `SettingsPage.tsx` `SEARCH_LIMIT = 12`).
pub const SETTINGS_NAV_SEARCH_LIMIT: usize = 12;

/// Ranked instant search hit for OpenBot's 2-column `SettingsPage` left-nav search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsNavSearchHit {
    pub section: SettingsSectionType,
    pub title_en: &'static str,
    pub title_zh: &'static str,
    pub desc_en: &'static str,
    pub desc_zh: &'static str,
    pub rank: u8,
}

struct SettingsNavSearchRow {
    section: SettingsSectionType,
    title_en: &'static str,
    title_zh: &'static str,
    desc_en: &'static str,
    desc_zh: &'static str,
    keywords: &'static [&'static str],
}

const SETTINGS_NAV_SEARCH_CATALOG: &[SettingsNavSearchRow] = &[
    SettingsNavSearchRow {
        section: SettingsSectionType::Main,
        title_en: "General Overview",
        title_zh: "常规概览",
        desc_en: "Account summary, workspace preferences and quick links",
        desc_zh: "账号概览、工作区偏好与快捷入口",
        keywords: &["general", "overview", "main", "home", "常规", "概览", "主页"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Information,
        title_en: "Edit Profile",
        title_zh: "个人资料",
        desc_en: "Name, username, bio, phone number and birthday",
        desc_zh: "姓名、用户名、简介、手机号与生日",
        keywords: &["profile", "account", "name", "username", "bio", "phone", "姓名", "昵称", "简介", "手机号"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Chat,
        title_en: "Appearance & Chat",
        title_zh: "外观与聊天",
        desc_en: "Theme, dark mode, wallpaper, bubble radius, font size and Enter send",
        desc_zh: "主题、深色模式、壁纸、气泡圆角、字号与回车发送",
        keywords: &["theme", "dark", "light", "night", "wallpaper", "font", "bubble", "enter", "主题", "深色", "浅色", "夜间", "壁纸", "字体", "聊天"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Folders,
        title_en: "Chat Folders",
        title_zh: "聊天文件夹",
        desc_en: "Organize chats into custom tabs and filter groups",
        desc_zh: "自定义对话分组、标签页与过滤规则",
        keywords: &["folder", "filter", "tab", "group", "文件夹", "分组", "标签"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Notifications,
        title_en: "Notifications & Sounds",
        title_zh: "通知与声音",
        desc_en: "Desktop alerts, sound volume, badges and mute schedules",
        desc_zh: "桌面提醒、提示音、角标计数与免打扰设置",
        keywords: &["notification", "sound", "alert", "badge", "mute", "quiet", "通知", "声音", "提醒", "角标", "免打扰"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::NotificationsType,
        title_en: "Notification Scope",
        title_zh: "分类通知规则",
        desc_en: "Per-type alerts for private chats, groups and channels",
        desc_zh: "私聊、群组与频道的独立通知规则",
        keywords: &["private", "groups", "channels", "scope", "私聊", "群组", "频道", "通知"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::NotificationsReactions,
        title_en: "Reaction Alerts",
        title_zh: "回应通知",
        desc_en: "Notifications for emoji reactions on messages and stories",
        desc_zh: "消息与动态表情回应通知提醒",
        keywords: &["reaction", "emoji", "like", "回应", "表情", "通知"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Calls,
        title_en: "Calls & Audio",
        title_zh: "音视频通话",
        desc_en: "Microphone, speakers, camera, mic test and peer-to-peer mode",
        desc_zh: "麦克风、扬声器、摄像头、试麦与点对点连接",
        keywords: &["call", "mic", "microphone", "speaker", "camera", "audio", "video", "p2p", "通话", "麦克风", "扬声器", "摄像头"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Language,
        title_en: "Language",
        title_zh: "界面语言",
        desc_en: "Switch interface display language between Chinese and English",
        desc_zh: "切换简体中文与英文界面显示语言",
        keywords: &["language", "locale", "chinese", "english", "zh", "en", "translate", "语言", "中文", "英文", "多语言"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::PrivacySecurity,
        title_en: "Privacy & Security",
        title_zh: "隐私与安全",
        desc_en: "Two-step verification, cloud password, recovery key and visibility",
        desc_zh: "两步验证、云密码、恢复密钥与隐私可见范围",
        keywords: &["privacy", "security", "2fa", "password", "cloud", "recovery", "隐私", "安全", "两步验证", "密码"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::ActiveSessions,
        title_en: "Devices & Sessions",
        title_zh: "活跃会话与设备",
        desc_en: "Inspect logged-in devices and terminate remote sessions",
        desc_zh: "查看已登录设备并远程下线异常会话",
        keywords: &["session", "device", "login", "terminate", "设备", "会话", "登录"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::LocalPasscode,
        title_en: "Local Passcode",
        title_zh: "本地密码锁",
        desc_en: "Lock desktop client with passcode and idle auto-lock timeout",
        desc_zh: "为桌面端设置锁屏密码与空闲自动锁定",
        keywords: &["passcode", "lock", "pin", "autolock", "密码锁", "锁屏", "自动锁定"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Passkeys,
        title_en: "Passkeys",
        title_zh: "通行密钥",
        desc_en: "FIDO2, WebAuthn and biometric passwordless sign-in keys",
        desc_zh: "FIDO2、WebAuthn 与生物识别免密登录",
        keywords: &["passkey", "fido", "webauthn", "biometric", "通行密钥", "生物识别", "指纹"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::BlockedPeers,
        title_en: "Blocked Users",
        title_zh: "黑名单",
        desc_en: "Manage blocked contacts, bots and restricted peers",
        desc_zh: "管理已屏蔽的联系人、机器人与黑名单",
        keywords: &["block", "blocked", "blacklist", "ban", "黑名单", "屏蔽", "拉黑"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::GlobalTtl,
        title_en: "Auto-Delete Messages",
        title_zh: "自动删除消息",
        desc_en: "Default global message self-destruct timer (TTL)",
        desc_zh: "新对话的默认全局消息定时销毁周期",
        keywords: &["ttl", "autodelete", "auto-delete", "timer", "自动删除", "定时销毁", "阅后即焚"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Websites,
        title_en: "Connected Websites",
        title_zh: "已授权网站",
        desc_en: "Review and revoke OAuth and bot website login sessions",
        desc_zh: "查看并撤销网站登录授权与机器人连接",
        keywords: &["website", "oauth", "connected", "bot", "网站", "授权"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::AiProviders,
        title_en: "AI Providers & Models",
        title_zh: "AI 模型与供应商",
        desc_en: "Configure OpenAI, Anthropic, Gemini, DeepSeek, Kimi, Ollama & routing",
        desc_zh: "配置模型订阅、API 密钥、脱敏保护与智能路由",
        keywords: &["ai", "provider", "model", "api", "key", "openai", "claude", "anthropic", "gemini", "deepseek", "kimi", "ollama", "magpie", "供应商", "模型", "密钥", "订阅"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Computers,
        title_en: "Remote Computers",
        title_zh: "远程电脑",
        desc_en: "Cloud desktops, remote terminal nodes and remote assist codes",
        desc_zh: "云端主机、远程终端节点与远程协助连接码",
        keywords: &["computer", "remote", "terminal", "ssh", "cloud", "desktop", "assist", "远程电脑", "云电脑", "终端", "协助"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Shortcuts,
        title_en: "Keyboard Shortcuts",
        title_zh: "键盘快捷键",
        desc_en: "Customize global hotkeys, navigation and composer bindings",
        desc_zh: "查看与自定义全局热键、导航与输入快捷键",
        keywords: &["shortcut", "hotkey", "keyboard", "keybind", "快捷键", "热键", "键盘"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Business,
        title_en: "Business Tools",
        title_zh: "商业版工具",
        desc_en: "Opening hours, away messages, greetings and quick replies",
        desc_zh: "营业时间、离开回复、欢迎语与快捷回复模板",
        keywords: &["business", "hours", "greeting", "quick reply", "商业", "企业", "快捷回复"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Advanced,
        title_en: "Advanced & Updates",
        title_zh: "高级与更新",
        desc_en: "Connection proxy, downloads, data export, hardware acceleration & updates",
        desc_zh: "网络代理、下载目录、数据导出、硬件加速与版本更新",
        keywords: &["advanced", "proxy", "socks5", "download", "export", "update", "opengl", "高级", "代理", "下载", "导出", "更新"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::LocalStorage,
        title_en: "Local Storage & Cache",
        title_zh: "本地存储与缓存",
        desc_en: "Inspect disk usage, media cache limits and clear temporary files",
        desc_zh: "查看磁盘占用、媒体缓存上限并一键清理缓存",
        keywords: &["storage", "cache", "disk", "clear", "cleanup", "存储", "缓存", "磁盘", "清理"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::PowerSaving,
        title_en: "Power Saving",
        title_zh: "省电模式",
        desc_en: "Reduce animations, autoplay videos and sticker motion on battery",
        desc_zh: "低电量时降低动画、视频自动播放与动态贴纸能耗",
        keywords: &["power", "battery", "animation", "autoplay", "reduced motion", "省电", "电池", "动画"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Premium,
        title_en: "Premium",
        title_zh: "高级会员",
        desc_en: "Higher upload limits, faster downloads and exclusive perks",
        desc_zh: "更大文件上传、加速下载与专属会员权益",
        keywords: &["premium", "subscription", "perks", "4gb", "会员", "订阅", "特权"],
    },
    SettingsNavSearchRow {
        section: SettingsSectionType::Credits,
        title_en: "Stars & Credits",
        title_zh: "星币与额度",
        desc_en: "Check Stars balance, top-ups, usage history and gifts",
        desc_zh: "查看星币余额、充值记录与消费明细",
        keywords: &["stars", "credits", "balance", "wallet", "topup", "星币", "额度", "余额", "充值", "钱包"],
    },
];

/// Manages settings navigation stack, modal rendering, and lazy instantiation of all settings sections.
pub struct SettingsRouter {
    pub is_open: bool,
    pub save_bar: super::settings_theme_ui::SaveBarDockState,
    pub settings_section: SettingsSectionType,
    pub settings_stack: Vec<SettingsSectionType>,
    pub standalone_computers_mode: bool,
    pub nav_search_query: String,
    pub nav_search_focused: bool,
    pub nav_search_selected_index: usize,
    pub palette: TdesktopThemePalette,
    pub lang_id: &'static str,
    pub is_chinese_locale: bool,
    pub preferences: AppPreferences,
    pub settings_main: Option<Entity<SettingsMainSections>>,
    pub settings_information: Option<Entity<SettingsInformationSections>>,
    pub settings_notifications: Option<Entity<SettingsNotificationsSections>>,
    pub settings_notif_reactions: Option<Entity<SettingsNotificationsReactionsSections>>,
    pub settings_notif_type: Option<Entity<SettingsNotificationsTypeSections>>,
    pub settings_privacy: Option<Entity<SettingsPrivacySecuritySections>>,
    pub settings_blocked_peers: Option<Entity<SettingsBlockedPeersSections>>,
    pub settings_active_sessions: Option<Entity<SettingsActiveSessionsSections>>,
    pub settings_local_passcode: Option<Entity<SettingsLocalPasscodeSections>>,
    pub settings_passkeys: Option<Entity<SettingsPasskeysSections>>,
    pub settings_global_ttl: Option<Entity<SettingsGlobalTtlSections>>,
    pub settings_websites: Option<Entity<SettingsWebsitesSections>>,
    pub settings_chat: Option<Entity<SettingsChatSections>>,
    pub settings_folders: Option<Entity<SettingsFoldersSections>>,
    pub settings_advanced: Option<Entity<SettingsAdvancedSections>>,
    pub settings_local_storage: Option<Entity<SettingsLocalStorageSections>>,
    pub settings_power_saving: Option<Entity<SettingsPowerSaving>>,
    pub settings_calls: Option<Entity<SettingsCallsSections>>,
    pub settings_credits: Option<Entity<SettingsCreditsSections>>,
    pub settings_premium: Option<Entity<SettingsPremiumSections>>,
    pub settings_shortcuts: Option<Entity<SettingsShortcutsSections>>,
    pub settings_business: Option<Entity<SettingsBusinessSections>>,
    pub settings_computers: Option<Entity<SettingsComputersSections>>,
    pub settings_ai_providers: Option<Entity<SettingsAiProvidersSections>>,
    pub settings_search: Option<Entity<SettingsSearch>>,
    pub update_snapshot: Option<DesktopUpdateSnapshot>,
    pub(super) last_active_input_key: Option<SettingsActiveInputKey>,
}

impl EventEmitter<SettingsRouterEvent> for SettingsRouter {}

macro_rules! sync_loaded_theme {
    ($palette:expr, $lang_id:expr, $cx:expr, $($slot:expr),* $(,)?) => {
        $(
            if let Some(ref ent) = $slot {
                ent.update($cx, |view, view_cx| {
                    view.set_palette($palette);
                    view.set_language($lang_id);
                    view_cx.notify();
                });
            }
        )*
    };
}

macro_rules! lazy_section {
    ($self:ident, $cx:expr, $slot:ident, $build:expr, $observer:path) => {{
        if let Some(ref existing) = $self.$slot {
            return existing.clone();
        }
        let pal = $self.palette;
        let lang = $self.lang_id;
        let build = $build;
        let ent = $cx.new(|_| {
            let mut view = build(pal, lang);
            view.set_palette(pal);
            view.set_language(lang);
            view
        });
        $observer(&ent, $cx);
        $self.$slot = Some(ent.clone());
        ent
    }};
    ($self:ident, $cx:expr, $slot:ident, $build:expr, $hydrate:path, $observer:path) => {{
        if let Some(ref existing) = $self.$slot {
            return existing.clone();
        }
        let pal = $self.palette;
        let lang = $self.lang_id;
        let prefs = &$self.preferences;
        let build = $build;
        let ent = $cx.new(|_| {
            let mut view = build(pal, lang);
            $hydrate(prefs, &mut view);
            view.set_palette(pal);
            view.set_language(lang);
            view
        });
        $observer(&ent, $cx);
        $self.$slot = Some(ent.clone());
        ent
    }};
}

impl SettingsRouter {
    /// Creates a new `SettingsRouter` with all 23 settings section entities unallocated (`None`).
    pub fn new(
        palette: TdesktopThemePalette,
        is_chinese_locale: bool,
        preferences: AppPreferences,
    ) -> Self {
        let lang_id = if is_chinese_locale { "zh-CN" } else { "en" };
        Self {
            is_open: true,
            save_bar: super::settings_theme_ui::SaveBarDockState::default(),
            settings_section: SettingsSectionType::Main,
            settings_stack: Vec::new(),
            standalone_computers_mode: false,
            nav_search_query: String::new(),
            nav_search_focused: false,
            nav_search_selected_index: 0,
            palette,
            lang_id,
            is_chinese_locale,
            preferences,
            settings_main: None,
            settings_information: None,
            settings_notifications: None,
            settings_notif_reactions: None,
            settings_notif_type: None,
            settings_privacy: None,
            settings_blocked_peers: None,
            settings_active_sessions: None,
            settings_local_passcode: None,
            settings_passkeys: None,
            settings_global_ttl: None,
            settings_websites: None,
            settings_chat: None,
            settings_folders: None,
            settings_advanced: None,
            settings_local_storage: None,
            settings_power_saving: None,
            settings_calls: None,
            settings_credits: None,
            settings_premium: None,
            settings_shortcuts: None,
            settings_business: None,
            settings_computers: None,
            settings_ai_providers: None,
            settings_search: None,
            update_snapshot: None,
            last_active_input_key: None,
        }
    }

    /// Stores the latest updater snapshot and projects it into `SettingsAdvancedSections` if materialized.
    pub fn sync_update_snapshot(&mut self, snap: DesktopUpdateSnapshot, cx: &mut Context<Self>) {
        self.update_snapshot = Some(snap.clone());
        if let Some(ref adv) = self.settings_advanced {
            adv.update(cx, |sa, sa_cx| {
                sa.sync_update_snapshot(&snap);
                sa_cx.notify();
            });
        }
        cx.notify();
    }

    /// Updates active theme palette and locale on currently materialized sections without allocating `None` slots.
    pub fn apply_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        is_chinese_locale: bool,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        self.is_chinese_locale = is_chinese_locale;
        self.lang_id = if is_chinese_locale { "zh-CN" } else { "en" };
        sync_loaded_theme!(
            self.palette,
            self.lang_id,
            cx,
            self.settings_main,
            self.settings_information,
            self.settings_notifications,
            self.settings_notif_reactions,
            self.settings_notif_type,
            self.settings_privacy,
            self.settings_blocked_peers,
            self.settings_active_sessions,
            self.settings_local_passcode,
            self.settings_passkeys,
            self.settings_global_ttl,
            self.settings_websites,
            self.settings_chat,
            self.settings_folders,
            self.settings_advanced,
            self.settings_local_storage,
            self.settings_power_saving,
            self.settings_calls,
            self.settings_credits,
            self.settings_premium,
            self.settings_shortcuts,
            self.settings_business,
            self.settings_computers,
            self.settings_ai_providers,
            self.settings_search,
        );
        cx.notify();
    }

    /// Stores updated preferences and re-hydrates any already-materialized sections without allocating `None` slots.
    pub fn sync_preferences(&mut self, prefs: AppPreferences, cx: &mut Context<Self>) {
        macro_rules! rehydrate_loaded {
            ($prefs:expr, $cx:expr, $(($slot:expr, $hydrate:path)),* $(,)?) => {
                $(
                    if let Some(ref ent) = $slot {
                        ent.update($cx, |sec, sec_cx| {
                            $hydrate($prefs, sec);
                            sec_cx.notify();
                        });
                    }
                )*
            };
        }
        self.preferences = prefs;
        let p = &self.preferences;
        rehydrate_loaded!(
            p,
            cx,
            (self.settings_main, Self::hydrate_main),
            (self.settings_chat, Self::hydrate_chat),
            (self.settings_notifications, Self::hydrate_notifications),
            (self.settings_notif_reactions, Self::hydrate_notif_reactions),
            (self.settings_notif_type, Self::hydrate_notif_type),
            (self.settings_local_storage, Self::hydrate_local_storage),
            (self.settings_advanced, Self::hydrate_advanced),
            (self.settings_power_saving, Self::hydrate_power_saving),
            (self.settings_calls, Self::hydrate_calls),
            (self.settings_local_passcode, Self::hydrate_local_passcode),
            (self.settings_privacy, Self::hydrate_privacy),
            (self.settings_folders, Self::hydrate_folders),
            (self.settings_passkeys, Self::hydrate_passkeys),
            (self.settings_websites, Self::hydrate_websites),
            (self.settings_shortcuts, Self::hydrate_shortcuts),
            (self.settings_business, Self::hydrate_business),
        );
        cx.notify();
    }

    fn hydrate_main(prefs: &AppPreferences, m: &mut SettingsMainSections) {
        m.interface_scale_pct = prefs.interface_scale_pct.clamp(100, 300);
    }

    fn hydrate_notifications(prefs: &AppPreferences, sn: &mut SettingsNotificationsSections) {
        sn.private_chats_enabled = prefs.notifications.private_chats;
        sn.groups_enabled = prefs.notifications.groups;
        sn.channels_enabled = prefs.notifications.channels;
        sn.sound_enabled = prefs.notifications.sound_enabled;
        sn.flash_taskbar_icon = prefs.notifications.flash_taskbar;
        sn.include_muted_in_badge = prefs.notifications.include_muted_in_badge;
        sn.badge_count_type = if prefs.notifications.badge_count_messages {
            BadgeCountType::CountMessages
        } else {
            BadgeCountType::CountChats
        };
    }

    fn hydrate_notif_reactions(
        prefs: &AppPreferences,
        sr: &mut SettingsNotificationsReactionsSections,
    ) {
        sr.notify_reactions = prefs.notifications.reaction_notifications;
        sr.notify_stories = prefs.notifications.story_reaction_notifications;
        sr.source = if prefs.notifications.reaction_contacts_only {
            ReactionsNotifySource::Contacts
        } else {
            ReactionsNotifySource::All
        };
        sr.sound_enabled = prefs.notifications.reaction_sound_enabled;
    }

    fn hydrate_notif_type(prefs: &AppPreferences, st: &mut SettingsNotificationsTypeSections) {
        st.alerts_enabled = prefs.notifications.type_alerts_enabled;
        st.message_preview = prefs.notifications.type_preview_enabled;
        st.sound_enabled = prefs.notifications.type_sound_enabled;
    }

    fn hydrate_privacy(prefs: &AppPreferences, sp: &mut SettingsPrivacySecuritySections) {
        sp.passcode_enabled = prefs
            .local_passcode_hash
            .as_deref()
            .is_some_and(|h| !h.is_empty());
    }

    fn hydrate_local_passcode(prefs: &AppPreferences, lp: &mut SettingsLocalPasscodeSections) {
        lp.auto_lock = AutoLockTimeout::from_seconds(prefs.auto_lock_seconds);
        if let Some(ref hash) = prefs.local_passcode_hash {
            if !hash.is_empty() {
                lp.passcode_hash = Some(hash.clone());
                lp.has_passcode = true;
            } else {
                lp.passcode_hash = None;
                lp.has_passcode = false;
            }
        } else {
            lp.passcode_hash = None;
            lp.has_passcode = false;
        }
    }

    fn hydrate_chat(prefs: &AppPreferences, sc: &mut SettingsChatSections) {
        sc.font_size = prefs.appearance.font_size;
        sc.bubble_radius = prefs.appearance.bubble_radius;
        sc.send_on_enter = prefs.appearance.send_on_enter;
        sc.suggest_stickers_by_emoji = prefs.appearance.suggest_stickers_by_emoji;
        sc.loop_animated_stickers = prefs.appearance.loop_animated_stickers;
        sc.blur_wallpaper = prefs.appearance.blur_wallpaper;
    }

    fn hydrate_folders(prefs: &AppPreferences, sf: &mut SettingsFoldersSections) {
        sf.tags_enabled = prefs.folder_tags_enabled;
    }

    fn hydrate_advanced(prefs: &AppPreferences, sa: &mut SettingsAdvancedSections) {
        sa.ask_download_path = prefs.ask_download_path;
        sa.opengl_enabled = prefs.opengl_enabled;
        sa.native_window_frame = prefs.native_window_frame;
        sa.tray_icon_enabled = prefs.tray_icon_enabled;
        sa.auto_update_enabled = prefs.auto_update_enabled;
    }

    fn hydrate_local_storage(prefs: &AppPreferences, ls: &mut SettingsLocalStorageSections) {
        ls.max_cache_gb = prefs.max_cache_gb;
        ls.retention_weeks = prefs.retention_weeks;
    }

    fn hydrate_power_saving(prefs: &AppPreferences, ps: &mut SettingsPowerSaving) {
        ps.battery_saver_enabled = prefs.power_saving_enabled;
        ps.battery_threshold_percent = prefs.power_saving_threshold_pct;
        ps.animate_stickers_in_chat = prefs.animate_stickers_chat;
        ps.animate_stickers_in_panel = prefs.animate_stickers_panel;
        ps.animate_emoji_in_panel = prefs.animate_emoji_panel;
        ps.autoplay_gifs = prefs.autoplay_gifs;
        ps.autoplay_videos = prefs.autoplay_videos;
    }

    fn hydrate_calls(prefs: &AppPreferences, sc: &mut SettingsCallsSections) {
        if !prefs.audio_input_device.is_empty() {
            if !sc.available_inputs.contains(&prefs.audio_input_device) {
                sc.available_inputs.insert(0, prefs.audio_input_device.clone());
            }
            sc.input_device = prefs.audio_input_device.clone();
        }
        if !prefs.audio_output_device.is_empty() {
            if !sc.available_outputs.contains(&prefs.audio_output_device) {
                sc.available_outputs.insert(0, prefs.audio_output_device.clone());
            }
            sc.output_device = prefs.audio_output_device.clone();
        }
        if !prefs.camera_device.is_empty() {
            if !sc.available_cameras.contains(&prefs.camera_device) {
                sc.available_cameras.insert(0, prefs.camera_device.clone());
            }
            sc.camera_device = prefs.camera_device.clone();
        }
    }

    fn hydrate_passkeys(prefs: &AppPreferences, sp: &mut SettingsPasskeysSections) {
        sp.sync_from_names(&prefs.passkey_names);
    }

    fn hydrate_websites(prefs: &AppPreferences, sw: &mut SettingsWebsitesSections) {
        sw.sync_from_domains(&prefs.connected_website_domains);
    }

    fn hydrate_shortcuts(prefs: &AppPreferences, ss: &mut SettingsShortcutsSections) {
        ss.shortcuts_enabled = prefs.shortcuts_enabled;
        ss.search_query = prefs.shortcuts_filter.clone();
    }

    fn hydrate_business(prefs: &AppPreferences, sb: &mut SettingsBusinessSections) {
        sb.sync_configured_tools(&prefs.business_configured_tools);
    }

    fn stop_mic_test_if_active(&mut self, cx: &mut Context<Self>) {
        if let Some(ref sc_ent) = self.settings_calls {
            let was_active = sc_ent.update(cx, |sc, sc_cx| {
                if sc.mic_test_active {
                    sc.stop_mic_test_silent();
                    sc_cx.notify();
                    true
                } else {
                    false
                }
            });
            if was_active {
                cx.emit(SettingsRouterEvent::ToggleMicTest(false));
            }
        }
    }

    /// Lazily materializes `SettingsMainSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_main(&mut self, cx: &mut Context<Self>) -> Entity<SettingsMainSections> {
        lazy_section!(self, cx, settings_main, |_, _| SettingsMainSections::new_live(), Self::hydrate_main, Self::observe_main)
    }

    /// Lazily materializes `SettingsInformationSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_information(&mut self, cx: &mut Context<Self>) -> Entity<SettingsInformationSections> {
        lazy_section!(self, cx, settings_information, |_, _| SettingsInformationSections::new_live(), Self::observe_information)
    }

    /// Lazily materializes `SettingsNotificationsSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_notifications(&mut self, cx: &mut Context<Self>) -> Entity<SettingsNotificationsSections> {
        lazy_section!(self, cx, settings_notifications, |_, _| SettingsNotificationsSections::new_live(), Self::hydrate_notifications, Self::observe_notifications)
    }

    /// Lazily materializes `SettingsNotificationsReactionsSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_notif_reactions(&mut self, cx: &mut Context<Self>) -> Entity<SettingsNotificationsReactionsSections> {
        lazy_section!(self, cx, settings_notif_reactions, |_, _| SettingsNotificationsReactionsSections::new(), Self::hydrate_notif_reactions, Self::observe_notif_reactions)
    }

    /// Lazily materializes `SettingsNotificationsTypeSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_notif_type(&mut self, cx: &mut Context<Self>) -> Entity<SettingsNotificationsTypeSections> {
        lazy_section!(self, cx, settings_notif_type, |_, _| SettingsNotificationsTypeSections::new("All Chats"), Self::hydrate_notif_type, Self::observe_notif_type)
    }

    /// Lazily materializes `SettingsPrivacySecuritySections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_privacy(&mut self, cx: &mut Context<Self>) -> Entity<SettingsPrivacySecuritySections> {
        lazy_section!(self, cx, settings_privacy, |_, _| SettingsPrivacySecuritySections::new_live(), Self::hydrate_privacy, Self::observe_privacy)
    }

    /// Lazily materializes `SettingsBlockedPeersSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_blocked_peers(&mut self, cx: &mut Context<Self>) -> Entity<SettingsBlockedPeersSections> {
        lazy_section!(self, cx, settings_blocked_peers, |_, _| SettingsBlockedPeersSections::new_live(), Self::observe_blocked_peers)
    }

    /// Lazily materializes `SettingsActiveSessionsSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_active_sessions(&mut self, cx: &mut Context<Self>) -> Entity<SettingsActiveSessionsSections> {
        lazy_section!(self, cx, settings_active_sessions, |_, _| SettingsActiveSessionsSections::new_live(), Self::observe_active_sessions)
    }

    /// Lazily materializes `SettingsLocalPasscodeSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_local_passcode(&mut self, cx: &mut Context<Self>) -> Entity<SettingsLocalPasscodeSections> {
        lazy_section!(self, cx, settings_local_passcode, |_, _| SettingsLocalPasscodeSections::new_live(), Self::hydrate_local_passcode, Self::observe_local_passcode)
    }

    /// Lazily materializes `SettingsPasskeysSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_passkeys(&mut self, cx: &mut Context<Self>) -> Entity<SettingsPasskeysSections> {
        lazy_section!(self, cx, settings_passkeys, |_, _| SettingsPasskeysSections::new_live(), Self::hydrate_passkeys, Self::observe_passkeys)
    }

    /// Lazily materializes `SettingsGlobalTtlSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_global_ttl(&mut self, cx: &mut Context<Self>) -> Entity<SettingsGlobalTtlSections> {
        lazy_section!(self, cx, settings_global_ttl, |_, _| SettingsGlobalTtlSections::new_live(), Self::observe_global_ttl)
    }

    /// Lazily materializes `SettingsWebsitesSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_websites(&mut self, cx: &mut Context<Self>) -> Entity<SettingsWebsitesSections> {
        lazy_section!(self, cx, settings_websites, |_, _| SettingsWebsitesSections::new_live(), Self::hydrate_websites, Self::observe_websites)
    }

    /// Lazily materializes `SettingsChatSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_chat(&mut self, cx: &mut Context<Self>) -> Entity<SettingsChatSections> {
        lazy_section!(self, cx, settings_chat, |pal, lang| SettingsChatSections::new_live(pal, lang), Self::hydrate_chat, Self::observe_chat)
    }

    /// Lazily materializes `SettingsFoldersSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_folders(&mut self, cx: &mut Context<Self>) -> Entity<SettingsFoldersSections> {
        lazy_section!(self, cx, settings_folders, |pal, lang| SettingsFoldersSections::new_live(pal, lang), Self::hydrate_folders, Self::observe_folders)
    }

    /// Lazily materializes `SettingsAdvancedSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_advanced(&mut self, cx: &mut Context<Self>) -> Entity<SettingsAdvancedSections> {
        let was_none = self.settings_advanced.is_none();
        let ent = lazy_section!(self, cx, settings_advanced, |pal, lang| SettingsAdvancedSections::new_live(pal, lang), Self::hydrate_advanced, Self::observe_advanced);
        if was_none {
            if let Some(ref snap) = self.update_snapshot {
                let snap = snap.clone();
                ent.update(cx, |sa, _| sa.sync_update_snapshot(&snap));
            }
        }
        ent
    }

    /// Lazily materializes `SettingsLocalStorageSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_local_storage(&mut self, cx: &mut Context<Self>) -> Entity<SettingsLocalStorageSections> {
        lazy_section!(self, cx, settings_local_storage, |pal, lang| SettingsLocalStorageSections::new_live(pal, lang), Self::hydrate_local_storage, Self::observe_local_storage)
    }

    /// Lazily materializes `SettingsPowerSaving`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_power_saving(&mut self, cx: &mut Context<Self>) -> Entity<SettingsPowerSaving> {
        lazy_section!(self, cx, settings_power_saving, |pal, lang| SettingsPowerSaving::new_live(pal, lang), Self::hydrate_power_saving, Self::observe_power_saving)
    }

    /// Lazily materializes `SettingsCallsSections`, hydrates preferences, applies theme/locale, and attaches its observer.
    pub fn ensure_calls(&mut self, cx: &mut Context<Self>) -> Entity<SettingsCallsSections> {
        let (host_inputs, host_outputs) = gpui_telegram_media::enumerate_audio_devices();
        lazy_section!(
            self,
            cx,
            settings_calls,
            move |pal, lang| {
                let mut view = SettingsCallsSections::new_live(pal, lang);
                view.sync_devices(host_inputs.clone(), host_outputs.clone(), Vec::new());
                view
            },
            Self::hydrate_calls,
            Self::observe_calls
        )
    }

    /// Lazily materializes `SettingsCreditsSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_credits(&mut self, cx: &mut Context<Self>) -> Entity<SettingsCreditsSections> {
        lazy_section!(self, cx, settings_credits, |pal, lang| SettingsCreditsSections::new_live(pal, lang), Self::observe_credits)
    }

    /// Lazily materializes `SettingsPremiumSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_premium(&mut self, cx: &mut Context<Self>) -> Entity<SettingsPremiumSections> {
        let is_premium = self
            .settings_main
            .as_ref()
            .is_some_and(|sm| sm.read(cx).profile.is_premium);
        lazy_section!(self, cx, settings_premium, move |pal, lang| SettingsPremiumSections::new_live(is_premium, pal, lang), Self::observe_premium)
    }

    /// Lazily materializes `SettingsShortcutsSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_shortcuts(&mut self, cx: &mut Context<Self>) -> Entity<SettingsShortcutsSections> {
        lazy_section!(self, cx, settings_shortcuts, |pal, lang| SettingsShortcutsSections::new_live(pal, lang), Self::hydrate_shortcuts, Self::observe_shortcuts)
    }

    /// Lazily materializes `SettingsBusinessSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_business(&mut self, cx: &mut Context<Self>) -> Entity<SettingsBusinessSections> {
        let is_business = self
            .settings_premium
            .as_ref()
            .is_some_and(|sp| sp.read(cx).is_premium_active)
            || self
                .settings_main
                .as_ref()
                .is_some_and(|sm| sm.read(cx).profile.is_premium);
        lazy_section!(self, cx, settings_business, move |pal, lang| SettingsBusinessSections::new_live(is_business, pal, lang), Self::hydrate_business, Self::observe_business)
    }

    /// Lazily materializes `SettingsComputersSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_computers(&mut self, cx: &mut Context<Self>) -> Entity<SettingsComputersSections> {
        lazy_section!(self, cx, settings_computers, |pal, lang| SettingsComputersSections::new_live(pal, lang), Self::observe_computers)
    }

    /// Lazily materializes `SettingsAiProvidersSections`, applies theme/locale, and attaches its observer.
    pub fn ensure_ai_providers(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<SettingsAiProvidersSections> {
        lazy_section!(
            self,
            cx,
            settings_ai_providers,
            |pal, lang| SettingsAiProvidersSections::new_live(pal, lang),
            Self::observe_ai_providers
        )
    }

    /// Lazily materializes `SettingsSearch`, applies theme/locale, and attaches its observer.
    pub fn ensure_search(&mut self, cx: &mut Context<Self>) -> Entity<SettingsSearch> {
        lazy_section!(self, cx, settings_search, |pal, lang| SettingsSearch::new_live(pal, lang), Self::observe_search)
    }

    /// Ranks all 25 settings destinations using OpenBot's 4-tier `rankRows` scoring
    /// (`0` = title startsWith, `1` = title contains, `2` = description/keywords contains, `3` = excluded),
    /// capped at `SETTINGS_NAV_SEARCH_LIMIT` (`12`).
    pub fn rank_nav_search_entries(query: &str) -> Vec<SettingsNavSearchHit> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u8, usize, SettingsNavSearchHit)> = SETTINGS_NAV_SEARCH_CATALOG
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                let t_en = row.title_en.to_lowercase();
                let t_zh = row.title_zh.to_lowercase();
                let rank = if t_en.starts_with(&needle) || t_zh.starts_with(&needle) {
                    0
                } else if t_en.contains(&needle) || t_zh.contains(&needle) {
                    1
                } else if row.desc_en.to_lowercase().contains(&needle)
                    || row.desc_zh.to_lowercase().contains(&needle)
                    || row
                        .keywords
                        .iter()
                        .any(|kw| kw.to_lowercase().contains(&needle))
                {
                    2
                } else {
                    3
                };
                (rank < 3).then_some((
                    rank,
                    idx,
                    SettingsNavSearchHit {
                        section: row.section,
                        title_en: row.title_en,
                        title_zh: row.title_zh,
                        desc_en: row.desc_en,
                        desc_zh: row.desc_zh,
                        rank,
                    },
                ))
            })
            .collect();
        scored.sort_by_key(|(rank, idx, _)| (*rank, *idx));
        scored
            .into_iter()
            .take(SETTINGS_NAV_SEARCH_LIMIT)
            .map(|(_, _, hit)| hit)
            .collect()
    }

    /// Returns ranked instant search hits for `self.nav_search_query` (`SETTINGS_NAV_SEARCH_LIMIT = 12`).
    pub fn ranked_nav_search_results(&self) -> Vec<SettingsNavSearchHit> {
        Self::rank_nav_search_entries(&self.nav_search_query)
    }

    /// Maps a `GlobalSearch` (`Cmd+K`) `"setting:*"` item ID to its authoritative [`SettingsSectionType`] (`PAIR-12`).
    pub fn resolve_global_search_settings_section(item_id: &str) -> Option<SettingsSectionType> {
        match item_id.trim() {
            "setting:main" => Some(SettingsSectionType::Main),
            "setting:chat" => Some(SettingsSectionType::Chat),
            "setting:folders" => Some(SettingsSectionType::Folders),
            "setting:ai_providers" => Some(SettingsSectionType::AiProviders),
            "setting:privacy" => Some(SettingsSectionType::PrivacySecurity),
            "setting:shortcuts" => Some(SettingsSectionType::Shortcuts),
            _ => None,
        }
    }

    /// Opens the corresponding settings section directly from an activated `GlobalSearchResultItem` (`PAIR-12`).
    pub fn open_from_global_search_hit(
        &mut self,
        item: &crate::views::openbot::agent_studio::GlobalSearchResultItem,
    ) -> Option<SettingsSectionType> {
        let target = Self::resolve_global_search_settings_section(&item.id)?;
        self.is_open = true;
        self.nav_search_focused = false;
        if target != SettingsSectionType::Computers {
            self.standalone_computers_mode = false;
        }
        self.settings_stack.clear();
        self.settings_section = target;
        Some(target)
    }

    /// Navigates to `target`, clearing history when entering the settings modal or pushing onto the history stack.
    pub fn navigate_to(
        &mut self,
        target: SettingsSectionType,
        entering_settings_modal: bool,
        cx: &mut Context<Self>,
    ) {
        self.is_open = true;
        self.nav_search_focused = false;
        if self.settings_section == SettingsSectionType::Calls && target != SettingsSectionType::Calls {
            self.stop_mic_test_if_active(cx);
        }
        if target != SettingsSectionType::Computers {
            self.standalone_computers_mode = false;
        }
        if entering_settings_modal {
            self.clear_all_preedits(cx);
            self.settings_stack.clear();
            self.settings_section = target;
            self.last_active_input_key = self.current_active_input_key(cx);
            cx.emit(SettingsRouterEvent::Navigated(target));
            cx.notify();
            return;
        }
        if self.settings_section == target {
            return;
        }
        self.clear_all_preedits(cx);
        if self.settings_stack.last().copied() != Some(self.settings_section) {
            self.settings_stack.push(self.settings_section);
        }
        self.settings_section = target;
        self.last_active_input_key = self.current_active_input_key(cx);
        cx.emit(SettingsRouterEvent::Navigated(target));
        cx.notify();
    }

    /// Unwinds one step in the settings navigation stack or closes settings when already at `Main`.
    pub fn navigate_back(&mut self, cx: &mut Context<Self>) -> bool {
        if self.settings_section == SettingsSectionType::Calls {
            self.stop_mic_test_if_active(cx);
        }
        if self.settings_section == SettingsSectionType::Computers {
            if let Some(ref sc) = self.settings_computers {
                let unwound_subpage = sc.update(cx, |view, vcx| {
                    if view.permission_modal_open {
                        view.permission_modal_open = false;
                        vcx.notify();
                        return true;
                    }
                    if view.page != crate::views::settings::sections::SettingsComputersPage::Overview {
                        view.open_overview();
                        vcx.notify();
                        return true;
                    }
                    false
                });
                if unwound_subpage {
                    cx.notify();
                    return true;
                }
            }
            if self.standalone_computers_mode {
                self.close(cx);
                return false;
            }
        }
        if let Some(prev) = self.settings_stack.pop() {
            self.clear_all_preedits(cx);
            self.settings_section = prev;
            self.last_active_input_key = self.current_active_input_key(cx);
            cx.emit(SettingsRouterEvent::Navigated(prev));
            cx.notify();
            true
        } else if !self.settings_section.is_root() {
            let parent = self
                .settings_section
                .parent_section()
                .unwrap_or(SettingsSectionType::Main);
            self.clear_all_preedits(cx);
            self.settings_section = parent;
            self.last_active_input_key = self.current_active_input_key(cx);
            cx.emit(SettingsRouterEvent::Navigated(parent));
            cx.notify();
            true
        } else {
            self.close(cx);
            false
        }
    }

    /// Pure progressive Escape handler (`UI-F19`):
    /// 1st Esc clears `nav_search_query` (returns `true`, keeping settings open);
    /// 2nd Esc closes settings (`is_open = false`, returns `false`).
    pub fn handle_escape_key(&mut self) -> bool {
        if !self.nav_search_query.is_empty() {
            self.nav_search_query.clear();
            self.nav_search_selected_index = 0;
            return true;
        }
        self.nav_search_focused = false;
        self.is_open = false;
        false
    }

    /// Resets section and navigation stack and emits `SettingsRouterEvent::CloseRequested`.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.is_open = false;
        self.stop_mic_test_if_active(cx);
        self.clear_all_preedits(cx);
        self.standalone_computers_mode = false;
        self.nav_search_query.clear();
        self.nav_search_focused = false;
        self.nav_search_selected_index = 0;
        self.settings_section = SettingsSectionType::Main;
        self.settings_stack.clear();
        self.last_active_input_key = None;
        cx.emit(SettingsRouterEvent::CloseRequested);
        cx.notify();
    }

    /// Stops any active mic test, clears section preedits, and resets the navigation stack
    /// back to `Main` without emitting `SettingsRouterEvent::CloseRequested`.
    pub fn close_silent(&mut self, cx: &mut Context<Self>) {
        self.is_open = false;
        self.stop_mic_test_if_active(cx);
        self.clear_all_preedits(cx);
        let changed = self.settings_section != SettingsSectionType::Main
            || !self.settings_stack.is_empty()
            || !self.nav_search_query.is_empty()
            || self.nav_search_focused;
        self.nav_search_query.clear();
        self.nav_search_focused = false;
        self.nav_search_selected_index = 0;
        self.settings_section = SettingsSectionType::Main;
        self.settings_stack.clear();
        self.last_active_input_key = None;
        if changed {
            cx.notify();
        }
    }

    /// Zeroizes plaintext secret drafts, clears preedits & search/profile drafts, stops any active
    /// mic test, resets navigation stack to `Main`, and drops all 25 lazily-materialized section
    /// entities across logout / account-switch / server-URL boundaries while preserving app-wide
    /// `palette`, `lang_id`, `is_chinese_locale`, `preferences`, and `update_snapshot`.
    pub fn reset_for_logout_or_account_switch(&mut self, cx: &mut Context<Self>) {
        self.stop_mic_test_if_active(cx);
        self.clear_all_preedits(cx);
        if let Some(sp) = self.settings_privacy.take() {
            sp.update(cx, |sec, _| sec.clear_session_custody_state());
        }
        if let Some(sc) = self.settings_computers.take() {
            sc.update(cx, |sec, _| {
                sec.assist_custom_code_draft.zeroize();
                sec.assist_target_id_draft.zeroize();
                sec.assist_target_code_draft.zeroize();
                sec.form_credential.zeroize();
                sec.assist_focus = AssistInputField::None;
                sec.form_focus = CloudFormField::None;
                sec.pending_event = None;
            });
        }
        if let Some(ap) = self.settings_ai_providers.take() {
            ap.update(cx, |sec, _| {
                sec.draft_api_key.zeroize();
                sec.import_raw_draft.zeroize();
                sec.active_field = AiProviderField::None;
                sec.pending_event = None;
            });
        }
        if let Some(ss) = self.settings_search.take() {
            ss.update(cx, |sec, _| {
                sec.set_query("");
                sec.recent_entry_ids.clear();
                sec.selected_index = 0;
                sec.pending_selected_section = None;
            });
        }
        if let Some(si) = self.settings_information.take() {
            si.update(cx, |sec, _| {
                sec.first_name.clear();
                sec.last_name.clear();
                sec.bio.clear();
                sec.username.clear();
                sec.phone.clear();
                sec.birthday = None;
                sec.accounts.clear();
                sec.active_field = ProfileEditField::None;
                sec.pending_save_profile = false;
                sec.pending_logout = false;
                sec.status_message = None;
            });
        }
        if let Some(sf) = self.settings_folders.take() {
            sf.update(cx, |sec, _| {
                sec.folders.clear();
                sec.new_folder_title.clear();
                sec.is_editing_title = false;
                sec.selected_folder_id = None;
                sec.pending_create_folder = false;
                sec.pending_created_folder = None;
            });
        }
        self.settings_main = None;
        self.settings_notifications = None;
        self.settings_notif_reactions = None;
        self.settings_notif_type = None;
        self.settings_blocked_peers = None;
        self.settings_active_sessions = None;
        self.settings_local_passcode = None;
        self.settings_passkeys = None;
        self.settings_global_ttl = None;
        self.settings_websites = None;
        self.settings_chat = None;
        self.settings_advanced = None;
        self.settings_local_storage = None;
        self.settings_power_saving = None;
        self.settings_calls = None;
        self.settings_credits = None;
        self.settings_premium = None;
        self.settings_shortcuts = None;
        self.settings_business = None;
        self.nav_search_query.clear();
        self.nav_search_focused = false;
        self.nav_search_selected_index = 0;
        self.settings_section = SettingsSectionType::Main;
        self.settings_stack.clear();
        self.last_active_input_key = None;
        cx.notify();
    }

    /// Updates the selected interface language locally and emits `SettingsRouterEvent::SelectLanguage`.
    pub fn select_language_code(&mut self, lang_code: &'static str, cx: &mut Context<Self>) {
        self.preferences.localization.language_code = lang_code.to_string();
        let is_zh = lang_code.starts_with("zh");
        self.apply_theme_and_locale(self.palette, is_zh, cx);
        cx.emit(SettingsRouterEvent::SelectLanguage(lang_code));
    }

    /// Flips between Chinese and English UI locales locally and emits `SettingsRouterEvent::ToggleLanguage`.
    pub fn toggle_language(&mut self, cx: &mut Context<Self>) {
        let next_zh = !self.is_chinese_locale;
        self.preferences.localization.language_code = if next_zh {
            "zh-CN".to_string()
        } else {
            "en-US".to_string()
        };
        self.apply_theme_and_locale(self.palette, next_zh, cx);
        cx.emit(SettingsRouterEvent::ToggleLanguage);
    }

    /// Returns the canonical active input identity for the currently focused settings field, if any.
    pub fn current_active_input_key(&self, cx: &App) -> Option<SettingsActiveInputKey> {
        if self.nav_search_focused {
            return Some(SettingsActiveInputKey::NavSearchQuery);
        }
        match self.settings_section {
            SettingsSectionType::Information => self
                .settings_information
                .as_ref()
                .map(|si| si.read(cx).active_field)
                .filter(|&f| f != ProfileEditField::None)
                .map(SettingsActiveInputKey::Information),
            SettingsSectionType::PrivacySecurity => self.settings_privacy.as_ref().and_then(|sp| {
                let r = sp.read(cx);
                if r.recovery_key_editing {
                    Some(SettingsActiveInputKey::Privacy(PrivacyEditField::RecoveryKey))
                } else if r.two_factor_editing {
                    Some(SettingsActiveInputKey::Privacy(
                        PrivacyEditField::TwoFactorPassword,
                    ))
                } else {
                    None
                }
            }),
            SettingsSectionType::Folders => self
                .settings_folders
                .as_ref()
                .filter(|sf| sf.read(cx).is_editing_title)
                .map(|_| SettingsActiveInputKey::FoldersTitle),
            SettingsSectionType::Computers => self.settings_computers.as_ref().and_then(|sc| {
                let r = sc.read(cx);
                if r.page == SettingsComputersPage::CloudForm
                    && r.form_focus != CloudFormField::None
                {
                    Some(SettingsActiveInputKey::ComputersCloudForm {
                        field: r.form_focus,
                        form_gen: r.form_session_gen,
                    })
                } else if r.page == SettingsComputersPage::Overview
                    && r.overview_tab == SettingsComputersTab::RemoteAssist
                    && r.assist_focus != AssistInputField::None
                {
                    Some(SettingsActiveInputKey::ComputersAssist(r.assist_focus))
                } else {
                    None
                }
            }),
            SettingsSectionType::AiProviders => {
                self.settings_ai_providers.as_ref().and_then(|ap| {
                    let r = ap.read(cx);
                    (r.active_field != AiProviderField::None).then_some(
                        SettingsActiveInputKey::AiProviders {
                            field: r.active_field,
                            editor_gen: r.editor_session_gen,
                        },
                    )
                })
            }
            SettingsSectionType::Search => Some(SettingsActiveInputKey::SearchQuery),
            _ => None,
        }
    }

    pub(super) fn sync_active_input_key(&mut self, cx: &mut Context<Self>) {
        let cur = self.current_active_input_key(cx);
        let prev = self.last_active_input_key;
        self.last_active_input_key = cur;
        if prev.is_some() && cur != prev {
            self.clear_all_preedits(cx);
            cx.emit(SettingsRouterEvent::ActiveInputFieldChanged);
        }
    }

    /// Returns `true` when the active settings section currently focuses a text input field.
    pub fn is_input_active(&self, cx: &App) -> bool {
        self.current_active_input_key(cx).is_some()
    }

    /// Returns `(committed, preedit)` for the active settings text input if one is focused.
    pub fn active_input_text_and_preedit(
        &self,
        ime: &TextInputController,
        cx: &App,
    ) -> Option<(String, Option<String>)> {
        if self.nav_search_focused {
            return Some(ime.text_and_preedit(self.nav_search_query.clone()));
        }
        if !self.is_input_active(cx) {
            return None;
        }
        let committed = match self.settings_section {
            SettingsSectionType::Information => self
                .settings_information
                .as_ref()
                .and_then(|si| si.read(cx).active_field_text())
                .unwrap_or_default(),
            SettingsSectionType::PrivacySecurity => self
                .settings_privacy
                .as_ref()
                .map(|sp| {
                    let r = sp.read(cx);
                    if r.recovery_key_editing {
                        r.recovery_key_draft.as_str().to_string()
                    } else {
                        r.two_factor_password_draft.clone()
                    }
                })
                .unwrap_or_default(),
            SettingsSectionType::Folders => self
                .settings_folders
                .as_ref()
                .map(|sf| sf.read(cx).new_folder_title.clone())
                .unwrap_or_default(),
            SettingsSectionType::Computers => self
                .settings_computers
                .as_ref()
                .map(|sc| sc.read(cx).active_field_text().to_string())
                .unwrap_or_default(),
            SettingsSectionType::AiProviders => self
                .settings_ai_providers
                .as_ref()
                .map(|ap| ap.read(cx).active_field_text().to_string())
                .unwrap_or_default(),
            SettingsSectionType::Search => self
                .settings_search
                .as_ref()
                .map(|ss| ss.read(cx).query.clone())
                .unwrap_or_default(),
            _ => return None,
        };
        Some(ime.text_and_preedit(committed))
    }

    /// Mirrors IME preedit text onto the active settings section input controller (`Computers` or `AiProviders`).
    pub fn set_active_input_ime_preedit(
        &mut self,
        preedit: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(cur_key) = self.current_active_input_key(cx) else {
            return false;
        };
        if preedit.is_some_and(|s| !s.is_empty()) {
            self.last_active_input_key = Some(cur_key);
        }
        match self.settings_section {
            SettingsSectionType::PrivacySecurity => {
                let sp = self.ensure_privacy(cx);
                sp.update(cx, |sp, sp_cx| {
                    sp.recovery_key_ime_preedit = if sp.recovery_key_editing {
                        preedit.filter(|s| !s.is_empty()).map(str::to_string)
                    } else {
                        None
                    };
                    sp_cx.notify();
                });
                cx.notify();
                true
            }
            SettingsSectionType::Computers => {
                let sc = self.ensure_computers(cx);
                sc.update(cx, |sc, sc_cx| sc.set_ime_preedit(preedit, sc_cx));
                cx.notify();
                true
            }
            SettingsSectionType::AiProviders => {
                let ap = self.ensure_ai_providers(cx);
                ap.update(cx, |ap, ap_cx| ap.set_ime_preedit(preedit, ap_cx));
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Clears IME preedit across any already-instantiated settings section without lazy allocation.
    pub fn clear_all_preedits(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(sp) = self.settings_privacy.clone() {
            if sp.read(cx).recovery_key_ime_preedit.is_some() {
                sp.update(cx, |sp, sp_cx| {
                    sp.recovery_key_ime_preedit = None;
                    sp_cx.notify();
                });
                changed = true;
            }
        }
        if let Some(sc) = self.settings_computers.clone() {
            if sc.update(cx, |sc, sc_cx| sc.clear_all_preedits(sc_cx)) {
                changed = true;
            }
        }
        if let Some(ap) = self.settings_ai_providers.clone() {
            let ctrl_opt = ap.read(cx).ime_controller.clone();
            if let Some(ctrl) = ctrl_opt {
                let needs_clear = {
                    let r = ctrl.read(cx);
                    r.ime_preedit.is_some()
                        || r.just_committed_ime
                        || r.last_ime_commit_at.is_some()
                };
                if needs_clear {
                    ctrl.update(cx, |c, _| {
                        c.clear_ime_state();
                    });
                    ap.update(cx, |_, ap_cx| ap_cx.notify());
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Clears the active settings input field and returns `true` if a settings input was active.
    pub fn clear_active_input(&mut self, cx: &mut Context<Self>) -> bool {
        if self.nav_search_focused {
            self.nav_search_query.clear();
            self.nav_search_selected_index = 0;
            cx.notify();
            return true;
        }
        if !self.is_input_active(cx) {
            return false;
        }
        match self.settings_section {
            SettingsSectionType::Information => {
                let si = self.ensure_information(cx);
                si.update(cx, |si, si_cx| {
                    si.clear_field_text();
                    si_cx.notify();
                });
            }
            SettingsSectionType::PrivacySecurity => {
                let sp = self.ensure_privacy(cx);
                sp.update(cx, |sp, sp_cx| {
                    if sp.recovery_key_editing {
                        sp.recovery_key_draft.zeroize();
                        sp.recovery_key_ime_preedit = None;
                    } else {
                        sp.two_factor_password_draft.zeroize();
                    }
                    sp_cx.notify();
                });
            }
            SettingsSectionType::Folders => {
                let sf = self.ensure_folders(cx);
                sf.update(cx, |sf, sf_cx| {
                    sf.new_folder_title.clear();
                    sf_cx.notify();
                });
            }
            SettingsSectionType::Computers => {
                let sc = self.ensure_computers(cx);
                sc.update(cx, |sc, sc_cx| {
                    sc.clear_active_input(sc_cx);
                });
            }
            SettingsSectionType::AiProviders => {
                let ap = self.ensure_ai_providers(cx);
                ap.update(cx, |ap, ap_cx| {
                    ap.clear_active_input(ap_cx);
                });
            }
            SettingsSectionType::Search => {
                let ss = self.ensure_search(cx);
                ss.update(cx, |ss, ss_cx| {
                    ss.set_query("");
                    ss_cx.notify();
                });
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Appends committed text into the active settings input field and returns `true` if handled.
    pub fn push_input_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.nav_search_focused {
            self.nav_search_query.push_str(text);
            self.nav_search_selected_index = 0;
            cx.notify();
            return true;
        }
        if !self.is_input_active(cx) {
            return false;
        }
        match self.settings_section {
            SettingsSectionType::Information => {
                let si = self.ensure_information(cx);
                si.update(cx, |si, si_cx| {
                    si.push_field_text(text);
                    si_cx.notify();
                });
            }
            SettingsSectionType::PrivacySecurity => {
                let sp = self.ensure_privacy(cx);
                sp.update(cx, |sp, sp_cx| {
                    if sp.recovery_key_editing {
                        sp.recovery_key_ime_preedit = None;
                        sp.recovery_key_draft.push_str(text);
                    } else {
                        sp.two_factor_password_draft.push_str(text);
                    }
                    sp_cx.notify();
                });
            }
            SettingsSectionType::Folders => {
                let sf = self.ensure_folders(cx);
                sf.update(cx, |sf, sf_cx| {
                    sf.new_folder_title.push_str(text);
                    sf_cx.notify();
                });
            }
            SettingsSectionType::Computers => {
                let sc = self.ensure_computers(cx);
                sc.update(cx, |sc, sc_cx| {
                    sc.push_input_text(text, sc_cx);
                });
            }
            SettingsSectionType::AiProviders => {
                let ap = self.ensure_ai_providers(cx);
                ap.update(cx, |ap, ap_cx| {
                    ap.push_input_text(text, ap_cx);
                });
            }
            SettingsSectionType::Search => {
                let ss = self.ensure_search(cx);
                ss.update(cx, |ss, ss_cx| {
                    let mut q = ss.query.clone();
                    q.push_str(text);
                    ss.set_query(q);
                    ss_cx.notify();
                });
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Handles non-character key navigation/submission for an active settings text field.
    pub fn handle_non_char_key(
        &mut self,
        key: &str,
        _is_shift: bool,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        if self.nav_search_focused {
            match key {
                "backspace" => {
                    self.nav_search_query.pop();
                    self.nav_search_selected_index = 0;
                    cx.notify();
                    return Some(true);
                }
                "down" => {
                    let len = self.ranked_nav_search_results().len();
                    if len > 0 {
                        self.nav_search_selected_index = (self.nav_search_selected_index + 1) % len;
                    }
                    cx.notify();
                    return Some(true);
                }
                "up" => {
                    let len = self.ranked_nav_search_results().len();
                    if len > 0 {
                        self.nav_search_selected_index = if self.nav_search_selected_index == 0 {
                            len - 1
                        } else {
                            self.nav_search_selected_index - 1
                        };
                    }
                    cx.notify();
                    return Some(true);
                }
                "enter" => {
                    let hits = self.ranked_nav_search_results();
                    if let Some(hit) = hits
                        .get(self.nav_search_selected_index)
                        .or_else(|| hits.first())
                    {
                        let target = hit.section;
                        self.nav_search_query.clear();
                        self.nav_search_selected_index = 0;
                        self.nav_search_focused = false;
                        self.navigate_to(target, false, cx);
                        return Some(true);
                    }
                    return Some(false);
                }
                "escape" => {
                    if self.handle_escape_key() {
                        cx.notify();
                        return Some(true);
                    }
                    self.sync_active_input_key(cx);
                    cx.notify();
                    return Some(false);
                }
                _ => return Some(false),
            }
        }
        if !self.is_input_active(cx) {
            return None;
        }
        let handled = match self.settings_section {
            SettingsSectionType::Information => {
                if !matches!(key, "tab" | "backspace" | "enter") {
                    return Some(false);
                }
                let si = self.ensure_information(cx);
                si.update(cx, |si, si_cx| {
                    match key {
                        "tab" => si.cycle_field(),
                        "backspace" => si.backspace_field_text(),
                        "enter" => {
                            si.pending_save_profile = true;
                            si.save_pending = false;
                        }
                        _ => {}
                    }
                    si_cx.notify();
                });
                true
            }
            SettingsSectionType::PrivacySecurity => {
                if !matches!(key, "backspace" | "enter" | "escape") {
                    return Some(false);
                }
                let sp = self.ensure_privacy(cx);
                sp.update(cx, |sp, sp_cx| {
                    if sp.recovery_key_editing {
                        match key {
                            "backspace" => {
                                sp.backspace_recovery_key_draft();
                                sp_cx.notify();
                            }
                            "enter" => {
                                let _ = sp.submit_recovery_key_draft();
                                sp_cx.notify();
                            }
                            "escape" => {
                                sp.recovery_key_editing = false;
                                sp.recovery_key_ime_preedit = None;
                                sp_cx.notify();
                            }
                            _ => {}
                        }
                    } else if key == "backspace" {
                        sp.two_factor_password_draft.pop();
                        sp_cx.notify();
                    } else if key == "escape" {
                        sp.two_factor_editing = false;
                        sp_cx.notify();
                    } else if !sp.two_factor_password_draft.trim().is_empty() {
                        let pw = sp.two_factor_password_draft.trim().to_string();
                        sp.pending_2fa_action = Some(Some(pw));
                        sp.two_factor_editing = false;
                        sp.two_factor_password_draft.zeroize();
                        sp_cx.notify();
                    }
                });
                true
            }
            SettingsSectionType::Folders => {
                if !matches!(key, "backspace" | "enter") {
                    return Some(false);
                }
                let sf = self.ensure_folders(cx);
                sf.update(cx, |sf, sf_cx| {
                    if key == "backspace" {
                        sf.new_folder_title.pop();
                        sf_cx.notify();
                    } else if !sf.new_folder_title.trim().is_empty() {
                        let next_id = sf.folders.iter().map(|f| f.id).max().unwrap_or(0) + 1;
                        let title = sf.new_folder_title.trim().to_string();
                        sf.folders.push(ChatFolderItem {
                            id: next_id,
                            title: title.clone(),
                            icon_name: "folder".into(),
                            included_peers_count: 0,
                            excluded_peers_count: 0,
                            unread_count: 0,
                            is_removed: false,
                        });
                        sf.selected_folder_id = Some(next_id);
                        sf.new_folder_title.clear();
                        sf.is_editing_title = false;
                        sf.pending_create_folder = true;
                        sf.pending_created_folder = Some((next_id, title));
                        sf_cx.notify();
                    }
                });
                true
            }
            SettingsSectionType::Computers => {
                if !matches!(key, "backspace" | "delete" | "tab" | "enter") {
                    return Some(false);
                }
                let sc = self.ensure_computers(cx);
                let handled = sc.update(cx, |sc, sc_cx| {
                    sc.handle_non_char_key(key, _is_shift, false, sc_cx)
                });
                handled
            }
            SettingsSectionType::AiProviders => {
                if !matches!(key, "backspace" | "delete" | "tab" | "enter") {
                    return Some(false);
                }
                let ap = self.ensure_ai_providers(cx);
                let handled = ap.update(cx, |ap, ap_cx| {
                    ap.handle_non_char_key(key, _is_shift, false, ap_cx)
                });
                handled
            }
            SettingsSectionType::Search => {
                if !matches!(key, "backspace" | "up" | "down" | "enter") {
                    return Some(false);
                }
                let ss = self.ensure_search(cx);
                ss.update(cx, |ss, ss_cx| {
                    ss.handle_key_input(key);
                    ss_cx.notify();
                });
                true
            }
            _ => return None,
        };
        self.sync_active_input_key(cx);
        cx.notify();
        Some(handled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn ranked_nav_search_follows_openbot_4_tier_ranking_and_limit_12() {
        assert!(SettingsRouter::rank_nav_search_entries("   ").is_empty());

        // Rank 0: title prefix match beats rank 1/2
        let lang_hits = SettingsRouter::rank_nav_search_entries("lang");
        assert!(!lang_hits.is_empty());
        assert_eq!(lang_hits[0].section, SettingsSectionType::Language);
        assert_eq!(lang_hits[0].rank, 0);

        // Rank 1: title substring match (not prefix)
        let sessions_hits = SettingsRouter::rank_nav_search_entries("Sessions");
        assert_eq!(sessions_hits[0].section, SettingsSectionType::ActiveSessions);
        assert_eq!(sessions_hits[0].rank, 1);

        // Rank 2: description / keyword match
        let deepseek_hits = SettingsRouter::rank_nav_search_entries("deepseek");
        assert_eq!(deepseek_hits[0].section, SettingsSectionType::AiProviders);
        assert_eq!(deepseek_hits[0].rank, 2);

        // Chinese prefix & keyword matching
        let zh_hits = SettingsRouter::rank_nav_search_entries("远程电脑");
        assert_eq!(zh_hits[0].section, SettingsSectionType::Computers);
        assert_eq!(zh_hits[0].rank, 0);

        // Rank 3 excluded
        assert!(SettingsRouter::rank_nav_search_entries("zzzz_no_match_999").is_empty());

        // Limit cap = 12 even for broad single-letter queries
        let broad_hits = SettingsRouter::rank_nav_search_entries("a");
        assert!(broad_hits.len() <= SETTINGS_NAV_SEARCH_LIMIT);
        assert_eq!(broad_hits.len(), SETTINGS_NAV_SEARCH_LIMIT);
        assert!(broad_hits.windows(2).all(|w| w[0].rank <= w[1].rank));
    }
}
