//! Deep Links routing and handling submodules.

pub mod deep_links_chats;
pub use deep_links_chats::DeepLinksChatsDeepLinks;

pub mod deep_links_contacts;
pub use deep_links_contacts::DeepLinksContactsDeepLinks;

pub mod deep_links_new;
pub use deep_links_new::DeepLinksNewDeepLinks;

pub mod deep_links_router;
pub use deep_links_router::DeepLinksRouterDeepLinks;

pub mod deep_links_settings;
pub use deep_links_settings::DeepLinksSettingsDeepLinks;

pub mod apple_deep_link;
pub use apple_deep_link::{
    format_apple_deep_link, parse_apple_deep_link, AppleDeepLink, AppleDeepLinkSection,
    DeepLinksAppleDeepLinks, MAX_APPLE_CREATION_ID_LEN, MAX_APPLE_DEEP_LINK_LEN,
    MAX_APPLE_QUERY_SLUG_LEN,
};

pub mod deep_links_types;
pub use deep_links_types::{
    DeepLinkAction, DeepLinkContext, DeepLinkEntry, DeepLinkResult,
    DeepLinkSettingsControl, DeepLinkSettingsSection, DeepLinksTypesDeepLinks,
};
