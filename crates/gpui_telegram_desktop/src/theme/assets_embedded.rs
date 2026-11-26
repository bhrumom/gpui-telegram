//! Embedded vector SVG icons and localized `.strings` resource tables.

macro_rules! define_icons {
    (
        canonical {
            $($variant:ident => $file:literal),* $(,)?
        }
        extra {
            $($extra_variant:ident => $extra_file:literal),* $(,)?
        }
    ) => {
        /// Every crisp vector SVG icon embedded in the desktop theme asset pack.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum IconName {
            $($variant,)*
            $($extra_variant,)*
        }

        impl IconName {
            /// All 64 canonical icon variants in registration order (kept stable for asset harness).
            pub const ALL: &'static [IconName] = &[
                $(IconName::$variant,)*
            ];

            /// Additional Apple Messages UI icon variants embedded alongside `ALL`.
            pub const EXTRA: &'static [IconName] = &[
                $(IconName::$extra_variant,)*
            ];

            /// Relative asset path resolved by [`super::assets::TelegramAssetSource`].
            pub const fn path(self) -> &'static str {
                match self {
                    $(IconName::$variant => concat!("icons/", $file),)*
                    $(IconName::$extra_variant => concat!("icons/", $extra_file),)*
                }
            }

            /// Bare SVG filename within the `icons/` directory.
            pub const fn filename(self) -> &'static str {
                match self {
                    $(IconName::$variant => $file,)*
                    $(IconName::$extra_variant => $extra_file,)*
                }
            }

            /// Compiled-in UTF-8 SVG bytes embedded via `include_bytes!`.
            pub const fn embedded_bytes(self) -> &'static [u8] {
                match self {
                    $(IconName::$variant => include_bytes!(concat!("../../assets/icons/", $file)),)*
                    $(IconName::$extra_variant => include_bytes!(concat!("../../assets/icons/", $extra_file)),)*
                }
            }
        }
    };
}

define_icons! {
    canonical {
        HamburgerMenu => "hamburger_menu.svg",
        Search => "search.svg",
        Close => "close.svg",
        BackArrow => "back_arrow.svg",
        ComposePen => "compose_pen.svg",
        Lock => "lock.svg",
        Pin => "pin.svg",
        Mute => "mute.svg",
        Verified => "verified.svg",
        PremiumStar => "premium_star.svg",
        SavedMessages => "saved_messages.svg",
        ArchivedChats => "archived_chats.svg",
        FolderAll => "folder_all.svg",
        FolderChat => "folder_chat.svg",
        FolderBots => "folder_bots.svg",
        FolderWork => "folder_work.svg",
        Paperclip => "paperclip.svg",
        SmileEmoji => "smile_emoji.svg",
        Sticker => "sticker.svg",
        Gif => "gif.svg",
        Mic => "mic.svg",
        VideoRound => "video_round.svg",
        SendPlane => "send_plane.svg",
        CheckSingle => "check_single.svg",
        CheckDouble => "check_double.svg",
        ClockSending => "clock_sending.svg",
        Reply => "reply.svg",
        Forward => "forward.svg",
        Edit => "edit.svg",
        DeleteTrash => "delete_trash.svg",
        Copy => "copy.svg",
        SelectCheck => "select_check.svg",
        Translate => "translate.svg",
        PhoneCall => "phone_call.svg",
        VideoCall => "video_call.svg",
        SidebarPanel => "sidebar_panel.svg",
        SidebarLeftOpen => "sidebar_left_open.svg",
        SidebarLeftClose => "sidebar_left_close.svg",
        MoreVertical => "more_vertical.svg",
        Play => "play.svg",
        Pause => "pause.svg",
        Speaker => "speaker.svg",
        ScreenShare => "screen_share.svg",
        Hangup => "hangup.svg",
        Camera => "camera.svg",
        Poll => "poll.svg",
        LocationPin => "location_pin.svg",
        FileDoc => "file_doc.svg",
        HeartReaction => "heart_reaction.svg",
        BubbleTailIn => "bubble_tail_in.svg",
        BubbleTailOut => "bubble_tail_out.svg",
        ChevronDown => "chevron_down.svg",
        ChevronRight => "chevron_right.svg",
        NightMoon => "night_moon.svg",
        SunDay => "sun_day.svg",
        SettingsGear => "settings_gear.svg",
        ContactsUsers => "contacts_users.svg",
        GroupUsers => "group_users.svg",
        ChannelMegaphone => "channel_megaphone.svg",
        BotRobot => "bot_robot.svg",
        WalletStars => "wallet_stars.svg",
        TerminalMcp => "terminal_mcp.svg",
        ComputerDesktop => "computer_desktop.svg",
        TelegramPlaneLogo => "telegram_plane_logo.svg",
    }
    extra {
        ArrowUp => "arrow_up.svg",
        CodeScene => "code_scene.svg",
        HubBooks => "hub_books.svg",
        HubMusic => "hub_music.svg",
        HubGames => "hub_games.svg",
        HubAppStore => "hub_appstore.svg",
        HubCreations => "hub_creations.svg",
        HubPublish => "hub_publish.svg",
        HubSparkles => "hub_sparkles.svg",
        CommunityGlobe => "community_globe.svg",
        UpvoteArrow => "community_upvote.svg",
        DownvoteArrow => "community_downvote.svg",
        Repost => "community_repost.svg",
        Bookmark => "community_bookmark.svg",
        FlameTrending => "community_trending.svg",
        ShieldMod => "community_mod_shield.svg",
        SparkleAlgo => "community_algo_spark.svg",
        CommunityAward => "community_award.svg",
        CommunityThread => "community_thread.svg",
    }
}

pub const EMBEDDED_EN_STRINGS: &[u8] = include_bytes!("../../assets/langs/en.strings");
pub const EMBEDDED_ZH_CN_STRINGS: &[u8] = include_bytes!("../../assets/langs/zh-CN.strings");

pub fn lookup_embedded_asset(normalized: &str) -> Option<&'static [u8]> {
    match normalized {
        "langs/en.strings" => Some(EMBEDDED_EN_STRINGS),
        "langs/zh-CN.strings" => Some(EMBEDDED_ZH_CN_STRINGS),
        _ => IconName::ALL
            .iter()
            .chain(IconName::EXTRA.iter())
            .copied()
            .find(|icon| icon.path() == normalized)
            .map(IconName::embedded_bytes),
    }
}
