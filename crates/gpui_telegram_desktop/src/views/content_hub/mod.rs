//! Content Hub views and pure-Rust interactive engines.

pub mod game_runtime;
pub use game_runtime::*;

pub mod app_store_view;
pub mod apple_primitives;
pub mod book_reader;
pub mod books_view;
pub mod creations_view;
pub mod game_runtime_sheet;
pub mod games_view;
pub mod marketplace_state;
pub mod marketplace_view;
pub mod music_player_bar;
pub mod music_view;
pub mod publish_review_modal;

use std::collections::{BTreeMap, HashMap};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_core::core::deep_links::{AppleDeepLink, AppleDeepLinkSection};
use gpui_telegram_media::media::audio::MusicTransportEngine;
use gpui_telegram_proto::creations::{
    AppStoreCatalogResponse, AppStoreMiniAppEntry, CatalogSortMode, CreateCreationReviewRequest,
    CreationKind, CreationPayload, CreationReviewEntryDto, CreationReviewStatus,
    GameLeaderboardEntryDto, PublicCatalogItemDto, ReviewDecisionRequest, SubmitGameScoreRequest,
    SubmitReviewRequest, UpdateCreationRequest, UpsertCreationStateRequest, UserCreationDto,
    UserCreationStateDto,
};
use openbot_storage::{McpKeyValue, McpTransport};

use crate::components::ui::TextInputController;
use crate::i18n::tr_str;
use crate::theme::TdesktopThemePalette;
use apple_primitives::openbot_primary_cta_btn;

pub use apple_primitives::EditableFieldState;
pub use book_reader::{
    BookReaderFontFamily, BookReaderState, BookReaderTheme, DEFAULT_READER_FONT_SIZE,
    MAX_READER_FONT_SIZE, MIN_READER_FONT_SIZE,
};
pub use creations_view::{
    render_my_creations_view, render_quick_creations_drawer, QUICK_DRAWER_WIDTH,
};
pub use game_runtime_sheet::{extract_game_payload, ActiveGameSession};
pub use marketplace_state::{
    ConnectorSetupDraft, CustomServerFormDraft, MarketplaceAppEntry, MarketplaceBotSummary,
    MarketplaceHubState, MarketplaceNavTab, MarketplaceRoute, MarketplaceSkillSortMode,
    MarketplaceSourceChoice,
};
pub use music_player_bar::{build_music_track_entries_from_creation, synthesize_entry_from_track};
pub use publish_review_modal::{
    render_publish_review_modal_body, PublishModalField, PublishReviewModalState,
    ReviewerAccessState,
};

use apple_primitives::{
    apple_grouped_card, apple_shelf_header, render_editable_field_box,
};

/// Top-level creative vertical or personal cloud studio section mounted inside `ContentHubWorkspace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentHubSection {
    Marketplace,
    Books,
    Music,
    Games,
    AppStore,
    MyCreations,
}

impl ContentHubSection {
    pub fn from_deep_link_section(section: AppleDeepLinkSection) -> Self {
        match section {
            AppleDeepLinkSection::Books => Self::Books,
            AppleDeepLinkSection::Music => Self::Music,
            AppleDeepLinkSection::Games => Self::Games,
            AppleDeepLinkSection::AppStore => Self::AppStore,
            AppleDeepLinkSection::Creations => Self::MyCreations,
        }
    }

    pub fn from_creation_kind(kind: CreationKind) -> Self {
        match kind {
            CreationKind::Book => Self::Books,
            CreationKind::Music => Self::Music,
            CreationKind::Game => Self::Games,
            CreationKind::App => Self::AppStore,
        }
    }

    pub fn creation_kind(self) -> Option<CreationKind> {
        match self {
            Self::Marketplace => None,
            Self::Books => Some(CreationKind::Book),
            Self::Music => Some(CreationKind::Music),
            Self::Games => Some(CreationKind::Game),
            Self::AppStore => Some(CreationKind::App),
            Self::MyCreations => None,
        }
    }

    pub fn default_sub_tab(self) -> &'static str {
        match self {
            Self::Marketplace => "apps",
            Self::Books => "reading_now",
            Self::Music => "home",
            Self::Games => "discover",
            Self::AppStore => "today",
            Self::MyCreations => "all",
        }
    }
}

/// Active non-modal text input target owned by `ContentHubWorkspace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentHubInputTarget {
    #[default]
    None,
    CatalogSearch,
    CreationsSearch,
    QuickDrawerSearch,
    ReviewTitle,
    ReviewBody,
    MarketplaceSearch,
    MarketplaceSetupPrimary,
    MarketplaceSetupSecondary,
    MarketplaceCustomName,
    MarketplaceCustomTagline,
    MarketplaceCustomCommand,
    MarketplaceCustomArgs,
    MarketplaceCustomEnv,
    MarketplaceCustomWorkDir,
    MarketplaceCustomUrl,
    MarketplaceCustomHeaders,
}

/// Typed events emitted by `ContentHubWorkspace` to `AppShell` (`shell_content_hub.rs`).
#[derive(Debug, Clone)]
pub enum ContentHubEvent {
    RefreshMyCreations,
    RefreshCatalog(ContentHubSection),
    RefreshMarketplaceState,
    RefreshReviewQueue,
    OpenCreatorBot(Option<CreationKind>),
    OpenCreatorBotWithCommand(String),
    OpenCreationDetail {
        section: ContentHubSection,
        creation_id: String,
        action: Option<String>,
    },
    OpenPublishModalForCreation(UserCreationDto),
    SaveCreationDraft {
        creation_id: String,
        req: UpdateCreationRequest,
    },
    SubmitCreationForReview {
        creation_id: String,
        update_req: Option<UpdateCreationRequest>,
        submit_req: SubmitReviewRequest,
    },
    WithdrawCreationReview {
        creation_id: String,
    },
    UnpublishCreation {
        creation_id: String,
    },
    DeleteCreation {
        creation_id: String,
    },
    LoadCreationAudit {
        creation_id: String,
    },
    SubmitReviewDecision {
        creation_id: String,
        req: ReviewDecisionRequest,
    },
    UpsertCreationState {
        creation_id: String,
        req: UpsertCreationStateRequest,
    },
    LoadCreationReviews {
        creation_id: String,
    },
    SubmitCreationReview {
        creation_id: String,
        req: CreateCreationReviewRequest,
    },
    LoadGameLeaderboard {
        creation_id: String,
    },
    SubmitGameScore {
        creation_id: String,
        req: SubmitGameScoreRequest,
    },
    PlayMusicCreation {
        creation: UserCreationDto,
        track_index: usize,
        shuffle: bool,
    },
    PlayMusicRadio {
        genre_filter: Option<String>,
    },
    MusicTransportChanged,
    LaunchGameSession {
        creation: UserCreationDto,
    },
    CloseGameSession,
    LaunchMiniApp {
        bot_username: String,
        short_name: String,
        web_url: Option<String>,
    },
    MarketplaceConnectPlugin {
        slug: String,
        auth_values: BTreeMap<String, String>,
        override_url: Option<String>,
    },
    MarketplaceConnectPublished {
        app_id: String,
        auth_values: BTreeMap<String, String>,
        override_url: Option<String>,
    },
    MarketplaceStartOAuth {
        slug: String,
        override_url: Option<String>,
    },
    MarketplaceProbeApp {
        app_id: String,
    },
    MarketplaceConnectNative {
        connector_id: String,
        account_label: Option<String>,
        credential: Option<String>,
        metadata: BTreeMap<String, String>,
    },
    MarketplaceDisconnectApp {
        app_id: String,
    },
    MarketplaceSaveCustomMcp {
        edit_server_id: Option<String>,
        name: String,
        transport: McpTransport,
        command: String,
        args: Vec<String>,
        env: Vec<McpKeyValue>,
        working_dir: String,
        url: String,
        headers: Vec<McpKeyValue>,
    },
    MarketplacePublishCustomApp {
        name: String,
        tagline: String,
        category: String,
        transport: McpTransport,
        command: String,
        args: Vec<String>,
        env: Vec<McpKeyValue>,
        url: String,
        headers: Vec<McpKeyValue>,
    },
    MarketplaceTestCustomMcp {
        name: String,
        transport: McpTransport,
        command: String,
        args: Vec<String>,
        env: Vec<McpKeyValue>,
        working_dir: String,
        url: String,
        headers: Vec<McpKeyValue>,
    },
    MarketplaceRemoveCustomMcp {
        server_id: String,
    },
    MarketplaceToggleServerAgentAccess {
        server_id: String,
        agent_id: String,
        allowed: bool,
    },
    MarketplaceAddAgent {
        slug: String,
    },
    MarketplaceOpenAgentChat {
        slug: String,
    },
    MarketplaceToggleSkillForAgent {
        slug: String,
        agent_id: String,
        install: bool,
    },
    MarketplaceRunPrompt {
        app_name: String,
        prompt: String,
    },
    MarketplaceOpenExternalUrl {
        url: String,
    },
    MarketplaceCopyLink {
        url: String,
    },
    CloseQuickDrawer,
    ClosePublishReviewModal,
    NavigateSection(ContentHubSection),
    FocusInputTaken,
}

/// Primary Apple Content Hub workspace entity holding synced cloud creations, vertical catalogs,
/// unified OpenBot Marketplace state, embedded Apple Books reader, headless-safe
/// `MusicTransportEngine`, interactive `ActiveGameSession`, Quick Creations Drawer state,
/// and `PublishReviewModalState`.
pub struct ContentHubWorkspace {
    pub active_section: ContentHubSection,
    pub active_sub_tab: String,
    pub active_action_hint: Option<String>,
    pub quick_drawer_open: bool,
    pub marketplace: MarketplaceHubState,
    pub music_engine: MusicTransportEngine,
    pub music_lyrics_open: bool,
    pub music_queue_open: bool,
    pub music_now_playing_creation_id: Option<String>,
    pub book_reader: Option<BookReaderState>,
    pub active_game: Option<ActiveGameSession>,
    pub my_creations: Vec<UserCreationDto>,
    pub my_creations_revision: i64,
    pub deleted_creation_revisions: HashMap<String, i64>,
    pub resolved_review_revisions: HashMap<String, i64>,
    pub my_creation_states: HashMap<String, UserCreationStateDto>,
    pub books_catalog: Vec<PublicCatalogItemDto>,
    pub music_catalog: Vec<PublicCatalogItemDto>,
    pub games_catalog: Vec<PublicCatalogItemDto>,
    pub appstore_catalog: AppStoreCatalogResponse,
    pub local_miniapps_fallback: Vec<AppStoreMiniAppEntry>,
    pub selected_creation_id: Option<String>,
    pub selected_catalog_item: Option<PublicCatalogItemDto>,
    pub selected_detail_creation: Option<UserCreationDto>,
    pub selected_genre_filter: Option<String>,
    pub active_reviews: Vec<CreationReviewEntryDto>,
    pub active_leaderboard: Vec<GameLeaderboardEntryDto>,
    pub review_draft_rating: u8,
    pub review_draft_title: EditableFieldState,
    pub review_draft_body: EditableFieldState,
    pub is_submitting_review: bool,
    pub review_status_notice: Option<String>,
    pub catalog_search: EditableFieldState,
    pub creations_search: EditableFieldState,
    pub quick_drawer_search: EditableFieldState,
    pub creations_kind_filter: Option<CreationKind>,
    pub creations_status_filter: Option<CreationReviewStatus>,
    pub quick_drawer_kind_filter: Option<CreationKind>,
    pub sort_mode: CatalogSortMode,
    pub active_input: ContentHubInputTarget,
    pub is_loading_creations: bool,
    pub is_loading_catalog: bool,
    pub error_message: Option<String>,
    pub publish_modal: PublishReviewModalState,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl EventEmitter<ContentHubEvent> for ContentHubWorkspace {}

impl ContentHubWorkspace {
    pub fn new(palette: TdesktopThemePalette, language_id: impl Into<String>) -> Self {
        let language_id = language_id.into();
        let mut publish_modal = PublishReviewModalState::default();
        publish_modal.language_id = language_id.clone();
        Self {
            active_section: ContentHubSection::Marketplace,
            active_sub_tab: ContentHubSection::Marketplace.default_sub_tab().to_string(),
            active_action_hint: None,
            quick_drawer_open: false,
            marketplace: MarketplaceHubState::new(),
            music_engine: MusicTransportEngine::new(),
            music_lyrics_open: false,
            music_queue_open: false,
            music_now_playing_creation_id: None,
            book_reader: None,
            active_game: None,
            my_creations: Vec::new(),
            my_creations_revision: 0,
            deleted_creation_revisions: HashMap::new(),
            resolved_review_revisions: HashMap::new(),
            my_creation_states: HashMap::new(),
            books_catalog: Vec::new(),
            music_catalog: Vec::new(),
            games_catalog: Vec::new(),
            appstore_catalog: AppStoreCatalogResponse::default(),
            local_miniapps_fallback: Vec::new(),
            selected_creation_id: None,
            selected_catalog_item: None,
            selected_detail_creation: None,
            selected_genre_filter: None,
            active_reviews: Vec::new(),
            active_leaderboard: Vec::new(),
            review_draft_rating: 5,
            review_draft_title: EditableFieldState::default(),
            review_draft_body: EditableFieldState::default(),
            is_submitting_review: false,
            review_status_notice: None,
            catalog_search: EditableFieldState::default(),
            creations_search: EditableFieldState::default(),
            quick_drawer_search: EditableFieldState::default(),
            creations_kind_filter: None,
            creations_status_filter: None,
            quick_drawer_kind_filter: None,
            sort_mode: CatalogSortMode::Featured,
            active_input: ContentHubInputTarget::None,
            is_loading_creations: false,
            is_loading_catalog: false,
            error_message: None,
            publish_modal,
            palette,
            language_id,
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang_id: impl Into<String>) {
        self.language_id = lang_id.into();
        self.publish_modal.language_id = self.language_id.clone();
        if self.publish_modal.is_open {
            self.publish_modal.recompute_validation();
        }
    }

    pub fn set_section(&mut self, section: ContentHubSection) {
        self.set_active_input_ime_preedit(None);
        if self.active_section != section {
            self.active_section = section;
            self.active_sub_tab = section.default_sub_tab().to_string();
            self.active_action_hint = None;
            self.selected_creation_id = None;
            self.selected_catalog_item = None;
            self.selected_detail_creation = None;
            self.selected_genre_filter = None;
            self.book_reader = None;
            self.active_game = None;
            self.active_reviews.clear();
            self.active_leaderboard.clear();
            self.review_status_notice = None;
            self.is_loading_catalog = false;
            if self.active_input != ContentHubInputTarget::QuickDrawerSearch {
                self.active_input = ContentHubInputTarget::None;
            }
        }
        match section {
            ContentHubSection::Books => self.marketplace.select_tab(MarketplaceNavTab::Books),
            ContentHubSection::Games => self.marketplace.select_tab(MarketplaceNavTab::Games),
            ContentHubSection::AppStore => self.marketplace.select_tab(MarketplaceNavTab::MiniApps),
            _ => {}
        }
    }

    pub fn select_marketplace_tab_with_refresh(
        &mut self,
        tab: MarketplaceNavTab,
        cx: &mut Context<Self>,
    ) {
        self.set_active_input_ime_preedit(None);
        self.active_section = ContentHubSection::Marketplace;
        self.marketplace.select_tab(tab);
        self.selected_creation_id = None;
        self.selected_catalog_item = None;
        self.selected_detail_creation = None;
        self.selected_genre_filter = None;
        self.active_reviews.clear();
        self.active_leaderboard.clear();
        self.review_status_notice = None;
        if self.active_input != ContentHubInputTarget::QuickDrawerSearch {
            self.active_input = ContentHubInputTarget::None;
        }
        match tab {
            MarketplaceNavTab::MiniApps => {
                self.active_sub_tab = ContentHubSection::AppStore.default_sub_tab().to_string();
                cx.emit(ContentHubEvent::RefreshCatalog(ContentHubSection::AppStore));
            }
            MarketplaceNavTab::Books => {
                self.active_sub_tab = ContentHubSection::Books.default_sub_tab().to_string();
                cx.emit(ContentHubEvent::RefreshCatalog(ContentHubSection::Books));
            }
            MarketplaceNavTab::Games => {
                self.active_sub_tab = ContentHubSection::Games.default_sub_tab().to_string();
                cx.emit(ContentHubEvent::RefreshCatalog(ContentHubSection::Games));
            }
            MarketplaceNavTab::Apps | MarketplaceNavTab::Agents | MarketplaceNavTab::Skills => {
                cx.emit(ContentHubEvent::RefreshMarketplaceState);
            }
        }
        cx.notify();
    }

    pub fn is_publish_review_modal_open(&self) -> bool {
        self.publish_modal.is_open
    }

    pub fn has_focused_input(&self, in_content_hub: bool) -> bool {
        match self.active_input {
            ContentHubInputTarget::None => false,
            ContentHubInputTarget::QuickDrawerSearch => self.quick_drawer_open,
            ContentHubInputTarget::CatalogSearch
            | ContentHubInputTarget::CreationsSearch
            | ContentHubInputTarget::ReviewTitle
            | ContentHubInputTarget::ReviewBody
            | ContentHubInputTarget::MarketplaceSearch
            | ContentHubInputTarget::MarketplaceSetupPrimary
            | ContentHubInputTarget::MarketplaceSetupSecondary
            | ContentHubInputTarget::MarketplaceCustomName
            | ContentHubInputTarget::MarketplaceCustomTagline
            | ContentHubInputTarget::MarketplaceCustomCommand
            | ContentHubInputTarget::MarketplaceCustomArgs
            | ContentHubInputTarget::MarketplaceCustomEnv
            | ContentHubInputTarget::MarketplaceCustomWorkDir
            | ContentHubInputTarget::MarketplaceCustomUrl
            | ContentHubInputTarget::MarketplaceCustomHeaders => in_content_hub,
        }
    }

    pub fn focus_input(&mut self, target: ContentHubInputTarget, cx: &mut Context<Self>) {
        self.set_active_input_ime_preedit(None);
        self.active_input = target;
        cx.emit(ContentHubEvent::FocusInputTaken);
        cx.notify();
    }

    pub fn blur_input(&mut self) {
        self.active_input = ContentHubInputTarget::None;
        self.catalog_search.set_ime_preedit(None);
        self.creations_search.set_ime_preedit(None);
        self.quick_drawer_search.set_ime_preedit(None);
        self.review_draft_title.set_ime_preedit(None);
        self.review_draft_body.set_ime_preedit(None);
        self.marketplace.clear_all_preedits();
    }

    fn active_field(&self) -> Option<&EditableFieldState> {
        match self.active_input {
            ContentHubInputTarget::None => None,
            ContentHubInputTarget::CatalogSearch => Some(&self.catalog_search),
            ContentHubInputTarget::CreationsSearch => Some(&self.creations_search),
            ContentHubInputTarget::QuickDrawerSearch => Some(&self.quick_drawer_search),
            ContentHubInputTarget::ReviewTitle => Some(&self.review_draft_title),
            ContentHubInputTarget::ReviewBody => Some(&self.review_draft_body),
            ContentHubInputTarget::MarketplaceSearch => Some(&self.marketplace.search),
            ContentHubInputTarget::MarketplaceSetupPrimary => {
                self.marketplace.setup_draft.as_ref().map(|d| &d.primary)
            }
            ContentHubInputTarget::MarketplaceSetupSecondary => {
                self.marketplace.setup_draft.as_ref().map(|d| &d.secondary)
            }
            ContentHubInputTarget::MarketplaceCustomName => Some(&self.marketplace.custom_form.name),
            ContentHubInputTarget::MarketplaceCustomTagline => {
                Some(&self.marketplace.custom_form.tagline)
            }
            ContentHubInputTarget::MarketplaceCustomCommand => {
                Some(&self.marketplace.custom_form.command)
            }
            ContentHubInputTarget::MarketplaceCustomArgs => Some(&self.marketplace.custom_form.args),
            ContentHubInputTarget::MarketplaceCustomEnv => {
                Some(&self.marketplace.custom_form.env_lines)
            }
            ContentHubInputTarget::MarketplaceCustomWorkDir => {
                Some(&self.marketplace.custom_form.working_dir)
            }
            ContentHubInputTarget::MarketplaceCustomUrl => Some(&self.marketplace.custom_form.url),
            ContentHubInputTarget::MarketplaceCustomHeaders => {
                Some(&self.marketplace.custom_form.header_lines)
            }
        }
    }

    fn active_field_mut(&mut self) -> Option<&mut EditableFieldState> {
        match self.active_input {
            ContentHubInputTarget::None => None,
            ContentHubInputTarget::CatalogSearch => Some(&mut self.catalog_search),
            ContentHubInputTarget::CreationsSearch => Some(&mut self.creations_search),
            ContentHubInputTarget::QuickDrawerSearch => Some(&mut self.quick_drawer_search),
            ContentHubInputTarget::ReviewTitle => Some(&mut self.review_draft_title),
            ContentHubInputTarget::ReviewBody => Some(&mut self.review_draft_body),
            ContentHubInputTarget::MarketplaceSearch => Some(&mut self.marketplace.search),
            ContentHubInputTarget::MarketplaceSetupPrimary => {
                self.marketplace.setup_draft.as_mut().map(|d| &mut d.primary)
            }
            ContentHubInputTarget::MarketplaceSetupSecondary => {
                self.marketplace.setup_draft.as_mut().map(|d| &mut d.secondary)
            }
            ContentHubInputTarget::MarketplaceCustomName => {
                Some(&mut self.marketplace.custom_form.name)
            }
            ContentHubInputTarget::MarketplaceCustomTagline => {
                Some(&mut self.marketplace.custom_form.tagline)
            }
            ContentHubInputTarget::MarketplaceCustomCommand => {
                Some(&mut self.marketplace.custom_form.command)
            }
            ContentHubInputTarget::MarketplaceCustomArgs => {
                Some(&mut self.marketplace.custom_form.args)
            }
            ContentHubInputTarget::MarketplaceCustomEnv => {
                Some(&mut self.marketplace.custom_form.env_lines)
            }
            ContentHubInputTarget::MarketplaceCustomWorkDir => {
                Some(&mut self.marketplace.custom_form.working_dir)
            }
            ContentHubInputTarget::MarketplaceCustomUrl => {
                Some(&mut self.marketplace.custom_form.url)
            }
            ContentHubInputTarget::MarketplaceCustomHeaders => {
                Some(&mut self.marketplace.custom_form.header_lines)
            }
        }
    }

    pub fn active_input_text_and_preedit(
        &self,
        fallback_ime: &TextInputController,
    ) -> (String, Option<String>) {
        self.active_field()
            .map(|f| f.text_and_preedit(fallback_ime))
            .unwrap_or_else(|| (String::new(), fallback_ime.ime_preedit.clone()))
    }

    pub fn insert_text_to_active_input(&mut self, text: &str) -> bool {
        if let Some(field) = self.active_field_mut() {
            field.insert_text(text);
            true
        } else {
            false
        }
    }

    pub fn clear_active_input(&mut self) -> bool {
        if let Some(field) = self.active_field_mut() {
            field.clear();
            true
        } else {
            false
        }
    }

    pub fn select_all_active_input(&mut self) -> bool {
        if let Some(field) = self.active_field_mut() {
            field.select_all_text();
            true
        } else {
            false
        }
    }

    pub fn set_active_input_ime_preedit(&mut self, preedit: Option<&str>) {
        if preedit.is_none() {
            self.catalog_search.set_ime_preedit(None);
            self.creations_search.set_ime_preedit(None);
            self.quick_drawer_search.set_ime_preedit(None);
            self.review_draft_title.set_ime_preedit(None);
            self.review_draft_body.set_ime_preedit(None);
            self.marketplace.clear_all_preedits();
        } else if let Some(field) = self.active_field_mut() {
            field.set_ime_preedit(preedit);
        }
    }

    pub fn clear_all_preedits(&mut self) -> bool {
        let had = self.catalog_search.ime_preedit.is_some()
            || self.creations_search.ime_preedit.is_some()
            || self.quick_drawer_search.ime_preedit.is_some()
            || self.review_draft_title.ime_preedit.is_some()
            || self.review_draft_body.ime_preedit.is_some()
            || self.marketplace.has_any_preedit()
            || self.publish_modal.has_any_preedit();
        if had {
            self.set_active_input_ime_preedit(None);
            self.publish_modal.clear_all_preedits();
        }
        had
    }

    pub fn handle_non_char_key_for_active_input(&mut self, key: &str, is_alt: bool) -> bool {
        let Some(field) = self.active_field_mut() else {
            return false;
        };
        match key {
            "backspace" => {
                field.backspace(is_alt);
                true
            }
            "delete" => {
                field.delete_forward();
                true
            }
            "left" | "arrowleft" => {
                field.move_left();
                true
            }
            "right" | "arrowright" => {
                field.move_right();
                true
            }
            "home" => {
                field.move_home();
                true
            }
            "end" => {
                field.move_end();
                true
            }
            "enter" => true,
            _ => false,
        }
    }
}

impl ContentHubWorkspace {
    pub fn open_publish_modal_for_creation(
        &mut self,
        creation: UserCreationDto,
        cx: &mut Context<Self>,
    ) {
        let cid = creation.creation_id.clone();
        self.blur_input();
        self.publish_modal.open_for_creation(creation);
        cx.emit(ContentHubEvent::FocusInputTaken);
        cx.emit(ContentHubEvent::LoadCreationAudit { creation_id: cid });
        cx.notify();
    }

    pub fn open_reviewer_queue(&mut self, cx: &mut Context<Self>) {
        self.blur_input();
        self.publish_modal.open_reviewer_queue();
        cx.emit(ContentHubEvent::FocusInputTaken);
        cx.emit(ContentHubEvent::RefreshReviewQueue);
        cx.notify();
    }

    pub fn find_my_creation(&self, creation_id: &str) -> Option<&UserCreationDto> {
        self.my_creations
            .iter()
            .find(|c| c.creation_id == creation_id)
    }

    pub fn find_catalog_item(&self, creation_id: &str) -> Option<&PublicCatalogItemDto> {
        self.books_catalog
            .iter()
            .chain(self.music_catalog.iter())
            .chain(self.games_catalog.iter())
            .chain(self.appstore_catalog.featured.iter())
            .chain(self.appstore_catalog.apps.iter())
            .chain(self.appstore_catalog.cross_vertical.iter())
            .find(|i| i.creation_id == creation_id)
    }

    pub fn record_deleted_creation(&mut self, cid: &str, rev: i64) {
        self.deleted_creation_revisions
            .entry(cid.to_string())
            .and_modify(|prev| *prev = (*prev).max(rev))
            .or_insert(rev);
        self.record_resolved_review(cid, rev);
    }

    pub fn is_deleted_at_or_above(&self, cid: &str, rev: i64) -> bool {
        self.deleted_creation_revisions
            .get(cid)
            .is_some_and(|&del_rev| rev <= del_rev)
    }

    pub fn record_resolved_review(&mut self, cid: &str, rev: i64) {
        self.resolved_review_revisions
            .entry(cid.to_string())
            .and_modify(|prev| *prev = (*prev).max(rev))
            .or_insert(rev);
    }

    pub fn clear_resolved_review_if_newer(&mut self, cid: &str, rev: i64) {
        if self
            .resolved_review_revisions
            .get(cid)
            .is_some_and(|&prev| rev > prev)
        {
            self.resolved_review_revisions.remove(cid);
        }
    }

    pub fn is_resolved_review_at_or_above(&self, cid: &str, rev: i64) -> bool {
        self.resolved_review_revisions
            .get(cid)
            .is_some_and(|&res_rev| rev <= res_rev)
    }

    pub fn apply_review_queue_snapshot(&mut self, items: Vec<UserCreationDto>) {
        let mut filtered = Vec::with_capacity(items.len());
        for item in items {
            if item.deleted_at_ms.is_some()
                || item.review_status != CreationReviewStatus::PendingReview
            {
                continue;
            }
            if self.is_deleted_at_or_above(&item.creation_id, item.cloud_revision)
                || self.is_resolved_review_at_or_above(&item.creation_id, item.cloud_revision)
            {
                continue;
            }
            if let Some(existing) = self.find_my_creation(&item.creation_id).cloned() {
                if existing.review_status != CreationReviewStatus::PendingReview
                    && existing.cloud_revision >= item.cloud_revision
                {
                    continue;
                }
                self.deleted_creation_revisions.remove(&item.creation_id);
                self.clear_resolved_review_if_newer(&item.creation_id, item.cloud_revision);
                if existing.review_status == CreationReviewStatus::PendingReview
                    && existing.cloud_revision > item.cloud_revision
                {
                    filtered.push(existing);
                } else {
                    filtered.push(item);
                }
            } else {
                self.deleted_creation_revisions.remove(&item.creation_id);
                self.clear_resolved_review_if_newer(&item.creation_id, item.cloud_revision);
                filtered.push(item);
            }
        }
        self.publish_modal.review_queue = filtered;
        self.publish_modal.reconcile_selected_review_id();
    }

    pub fn set_my_creations(&mut self, items: Vec<UserCreationDto>) {
        let prev_by_id: HashMap<String, UserCreationDto> = self
            .my_creations
            .iter()
            .map(|c| (c.creation_id.clone(), c.clone()))
            .collect();
        let mut tombstone_ids = std::collections::HashSet::new();
        let mut seen_ids = std::collections::HashSet::with_capacity(items.len());
        let mut max_rev = self.my_creations_revision;
        let mut active_items = Vec::with_capacity(items.len());
        for item in items {
            max_rev = max_rev.max(item.cloud_revision);
            seen_ids.insert(item.creation_id.clone());
            if item.deleted_at_ms.is_some() {
                self.record_deleted_creation(&item.creation_id, item.cloud_revision);
                tombstone_ids.insert(item.creation_id);
            } else if self.is_deleted_at_or_above(&item.creation_id, item.cloud_revision) {
                tombstone_ids.insert(item.creation_id);
            } else {
                self.deleted_creation_revisions.remove(&item.creation_id);
                let chosen = match prev_by_id.get(&item.creation_id) {
                    Some(prev) if prev.cloud_revision > item.cloud_revision => prev.clone(),
                    _ => item,
                };
                if chosen.review_status != CreationReviewStatus::PendingReview {
                    self.record_resolved_review(&chosen.creation_id, chosen.cloud_revision);
                } else {
                    self.clear_resolved_review_if_newer(&chosen.creation_id, chosen.cloud_revision);
                }
                active_items.push(chosen);
            }
        }
        for (prev_id, prev_dto) in &prev_by_id {
            if !seen_ids.contains(prev_id) && !tombstone_ids.contains(prev_id) {
                self.record_deleted_creation(prev_id, prev_dto.cloud_revision);
            }
        }
        self.my_creations = active_items;
        self.my_creations
            .sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
        self.my_creations_revision = max_rev;

        if let Some(target_id) = self.publish_modal.target_creation_id.clone() {
            if let Some(fresh) = self.find_my_creation(&target_id).cloned() {
                if self.publish_modal.base_creation.as_ref().is_none_or(|cur| {
                    cur.creation_id != fresh.creation_id
                        || fresh.cloud_revision >= cur.cloud_revision
                }) {
                    self.publish_modal.base_creation = Some(fresh);
                    self.publish_modal.recompute_validation();
                }
            } else {
                self.publish_modal.target_creation_id = None;
                self.publish_modal.base_creation = None;
                self.publish_modal.audit_trail.clear();
                if self.publish_modal.is_open && !self.publish_modal.is_showing_reviewer_queue() {
                    self.publish_modal.close();
                    self.publish_modal.pending_close_ime_clear = false;
                }
            }
        }

        if let Some(sel_id) = self
            .selected_detail_creation
            .as_ref()
            .map(|d| d.creation_id.clone())
            .or_else(|| self.selected_creation_id.clone())
        {
            if let Some(fresh) = self.find_my_creation(&sel_id).cloned() {
                if self.selected_creation_id.as_deref() == Some(sel_id.as_str())
                    && self.selected_detail_creation.as_ref().is_none_or(|cur| {
                        cur.creation_id != fresh.creation_id
                            || fresh.cloud_revision >= cur.cloud_revision
                    })
                {
                    self.selected_detail_creation = Some(fresh);
                }
            } else if (prev_by_id.contains_key(&sel_id) || tombstone_ids.contains(&sel_id))
                && self.find_catalog_item(&sel_id).is_none()
            {
                if self.selected_creation_id.as_deref() == Some(sel_id.as_str()) {
                    self.selected_creation_id = None;
                }
                if self
                    .selected_catalog_item
                    .as_ref()
                    .is_some_and(|c| c.creation_id == sel_id)
                {
                    self.selected_catalog_item = None;
                }
                if self
                    .selected_detail_creation
                    .as_ref()
                    .is_some_and(|d| d.creation_id == sel_id)
                {
                    self.selected_detail_creation = None;
                }
            }
        }

        let active_by_id: HashMap<&str, &UserCreationDto> = self
            .my_creations
            .iter()
            .map(|c| (c.creation_id.as_str(), c))
            .collect();
        let mut updated_queue = Vec::with_capacity(self.publish_modal.review_queue.len());
        for mut q in std::mem::take(&mut self.publish_modal.review_queue) {
            if tombstone_ids.contains(&q.creation_id) {
                continue;
            }
            if let Some(&fresh) = active_by_id.get(q.creation_id.as_str()) {
                if fresh.review_status != CreationReviewStatus::PendingReview {
                    continue;
                }
                if fresh.cloud_revision >= q.cloud_revision {
                    q = fresh.clone();
                }
                updated_queue.push(q);
            } else if prev_by_id.contains_key(&q.creation_id)
                || self.is_deleted_at_or_above(&q.creation_id, q.cloud_revision)
                || self.is_resolved_review_at_or_above(&q.creation_id, q.cloud_revision)
            {
                continue;
            } else {
                updated_queue.push(q);
            }
        }
        self.publish_modal.review_queue = updated_queue;
        self.publish_modal.reconcile_selected_review_id();
    }

    pub fn upsert_my_creation(&mut self, creation: UserCreationDto) {
        let cid = creation.creation_id.clone();
        if creation.deleted_at_ms.is_none() {
            if self.is_deleted_at_or_above(&cid, creation.cloud_revision) {
                return;
            }
            let known_rev = self
                .my_creations
                .iter()
                .find(|c| c.creation_id == cid)
                .map(|c| c.cloud_revision)
                .into_iter()
                .chain(
                    self.selected_detail_creation
                        .as_ref()
                        .filter(|d| d.creation_id == cid)
                        .map(|d| d.cloud_revision),
                )
                .chain(
                    self.publish_modal
                        .base_creation
                        .as_ref()
                        .filter(|b| b.creation_id == cid)
                        .map(|b| b.cloud_revision),
                )
                .max();
            if known_rev.is_some_and(|rev| creation.cloud_revision < rev) {
                return;
            }
            self.deleted_creation_revisions.remove(&cid);
        }
        self.my_creations_revision = self.my_creations_revision.max(creation.cloud_revision);
        if creation.deleted_at_ms.is_some() {
            self.record_deleted_creation(&cid, creation.cloud_revision);
            self.my_creations.retain(|c| c.creation_id != cid);
            self.publish_modal.prune_review_queue_item(&cid);
            if self.selected_creation_id.as_deref() == Some(cid.as_str()) {
                self.selected_creation_id = None;
            }
            if self
                .selected_catalog_item
                .as_ref()
                .is_some_and(|c| c.creation_id == cid)
            {
                self.selected_catalog_item = None;
            }
            if self
                .selected_detail_creation
                .as_ref()
                .is_some_and(|d| d.creation_id == cid)
            {
                self.selected_detail_creation = None;
            }
            if self
                .book_reader
                .as_ref()
                .is_some_and(|r| r.creation_id == cid)
            {
                self.book_reader = None;
            }
            if self
                .active_game
                .as_ref()
                .is_some_and(|g| g.creation_id == cid)
            {
                self.active_game = None;
            }
            if self.publish_modal.target_creation_id.as_deref() == Some(cid.as_str()) {
                self.publish_modal.target_creation_id = None;
                self.publish_modal.base_creation = None;
                self.publish_modal.audit_trail.clear();
                if self.publish_modal.is_open && !self.publish_modal.is_showing_reviewer_queue() {
                    self.publish_modal.close();
                    self.publish_modal.pending_close_ime_clear = false;
                }
            }
            return;
        }
        if let Some(slot) = self.my_creations.iter_mut().find(|c| c.creation_id == cid) {
            *slot = creation.clone();
        } else {
            self.my_creations.insert(0, creation.clone());
        }
        if creation.review_status != CreationReviewStatus::PendingReview {
            self.record_resolved_review(&cid, creation.cloud_revision);
            self.publish_modal.prune_review_queue_item(&cid);
        } else {
            self.clear_resolved_review_if_newer(&cid, creation.cloud_revision);
            if let Some(q) = self
                .publish_modal
                .review_queue
                .iter_mut()
                .find(|item| item.creation_id == cid)
            {
                if creation.cloud_revision >= q.cloud_revision {
                    *q = creation.clone();
                }
            }
        }
        if self.selected_creation_id.as_deref() == Some(cid.as_str()) {
            self.selected_detail_creation = Some(creation.clone());
        }
        if self.publish_modal.target_creation_id.as_deref() == Some(cid.as_str()) {
            self.publish_modal.base_creation = Some(creation);
            self.publish_modal.recompute_validation();
        }
    }

    pub fn evict_public_catalog_item(&mut self, creation_id: &str) -> bool {
        let mut removed = false;
        let before = self.books_catalog.len();
        self.books_catalog.retain(|i| i.creation_id != creation_id);
        removed |= self.books_catalog.len() != before;

        let before = self.music_catalog.len();
        self.music_catalog.retain(|i| i.creation_id != creation_id);
        removed |= self.music_catalog.len() != before;

        let before = self.games_catalog.len();
        self.games_catalog.retain(|i| i.creation_id != creation_id);
        removed |= self.games_catalog.len() != before;

        let before = self.appstore_catalog.featured.len();
        self.appstore_catalog
            .featured
            .retain(|i| i.creation_id != creation_id);
        removed |= self.appstore_catalog.featured.len() != before;

        let before = self.appstore_catalog.apps.len();
        self.appstore_catalog
            .apps
            .retain(|i| i.creation_id != creation_id);
        removed |= self.appstore_catalog.apps.len() != before;

        let before = self.appstore_catalog.cross_vertical.len();
        self.appstore_catalog
            .cross_vertical
            .retain(|i| i.creation_id != creation_id);
        removed |= self.appstore_catalog.cross_vertical.len() != before;

        if self
            .selected_catalog_item
            .as_ref()
            .is_some_and(|i| i.creation_id == creation_id)
        {
            self.selected_catalog_item = None;
            removed = true;
        }
        if self.find_my_creation(creation_id).is_none() {
            if self.selected_creation_id.as_deref() == Some(creation_id) {
                self.selected_creation_id = None;
                self.selected_detail_creation = None;
            }
            if self
                .book_reader
                .as_ref()
                .is_some_and(|r| r.creation_id == creation_id)
            {
                self.book_reader = None;
            }
            if self
                .active_game
                .as_ref()
                .is_some_and(|g| g.creation_id == creation_id)
            {
                self.active_game = None;
            }
        }
        removed
    }

    pub fn filtered_my_creations(&self) -> Vec<UserCreationDto> {
        let q = self.creations_search.text.trim().to_lowercase();
        self.my_creations
            .iter()
            .filter(|c| c.deleted_at_ms.is_none())
            .filter(|c| self.creations_kind_filter.is_none_or(|k| c.kind == k))
            .filter(|c| {
                self.creations_status_filter
                    .is_none_or(|st| c.review_status == st)
            })
            .filter(|c| {
                q.is_empty()
                    || c.title.to_lowercase().contains(&q)
                    || c.subtitle.to_lowercase().contains(&q)
                    || c.summary.to_lowercase().contains(&q)
                    || c.author_display_name.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    pub fn filtered_quick_drawer_creations(&self) -> Vec<UserCreationDto> {
        let q = self.quick_drawer_search.text.trim().to_lowercase();
        self.my_creations
            .iter()
            .filter(|c| c.deleted_at_ms.is_none())
            .filter(|c| self.quick_drawer_kind_filter.is_none_or(|k| c.kind == k))
            .filter(|c| {
                q.is_empty()
                    || c.title.to_lowercase().contains(&q)
                    || c.summary.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    pub fn apply_deep_link(&mut self, link: &AppleDeepLink) {
        let section = ContentHubSection::from_deep_link_section(link.section);
        self.set_section(section);
        self.active_sub_tab = link
            .tab
            .clone()
            .unwrap_or_else(|| section.default_sub_tab().to_string());
        self.active_action_hint = link.action.clone();
        self.selected_creation_id = link.creation_id.clone();
        self.selected_catalog_item = None;
        self.selected_detail_creation = None;
        self.book_reader = None;
        self.active_game = None;
        self.active_reviews.clear();
        self.active_leaderboard.clear();
        self.review_status_notice = None;
        if let Some(cid) = link.creation_id.as_deref() {
            if let Some(cat) = self.find_catalog_item(cid).cloned() {
                self.selected_catalog_item = Some(cat);
            }
            if let Some(own) = self.find_my_creation(cid).cloned() {
                self.selected_detail_creation = Some(own);
            }
        }
    }

    pub fn open_book_reader_for_creation(&mut self, creation: &UserCreationDto) -> bool {
        let payload = match &creation.payload {
            CreationPayload::Book(bp) => bp.clone(),
            _ => return false,
        };
        let saved = self.my_creation_states.get(&creation.creation_id);
        self.book_reader = Some(BookReaderState::open(
            creation,
            payload,
            saved,
            self.palette,
        ));
        true
    }

    pub fn launch_game_for_creation(&mut self, creation: &UserCreationDto, seed: u64) -> bool {
        let Some(payload) = extract_game_payload(creation) else {
            return false;
        };
        let saved = self.my_creation_states.get(&creation.creation_id);
        self.active_game = Some(ActiveGameSession::launch(creation, &payload, saved, seed));
        true
    }

    pub fn reset_for_logout_or_account_switch(&mut self) {
        self.music_engine.stop();
        self.music_engine.clear_queue();
        self.music_lyrics_open = false;
        self.music_queue_open = false;
        self.music_now_playing_creation_id = None;
        self.book_reader = None;
        self.active_game = None;
        self.active_section = ContentHubSection::Marketplace;
        self.active_sub_tab = ContentHubSection::Marketplace.default_sub_tab().to_string();
        self.active_action_hint = None;
        self.quick_drawer_open = false;
        self.marketplace = MarketplaceHubState::new();
        self.my_creations.clear();
        self.my_creations_revision = 0;
        self.deleted_creation_revisions.clear();
        self.resolved_review_revisions.clear();
        self.my_creation_states.clear();
        self.books_catalog.clear();
        self.music_catalog.clear();
        self.games_catalog.clear();
        self.appstore_catalog = AppStoreCatalogResponse::default();
        self.local_miniapps_fallback.clear();
        self.selected_creation_id = None;
        self.selected_catalog_item = None;
        self.selected_detail_creation = None;
        self.selected_genre_filter = None;
        self.active_reviews.clear();
        self.active_leaderboard.clear();
        self.review_draft_rating = 5;
        self.review_draft_title.clear();
        self.review_draft_body.clear();
        self.is_submitting_review = false;
        self.review_status_notice = None;
        self.catalog_search.clear();
        self.creations_search.clear();
        self.quick_drawer_search.clear();
        self.creations_kind_filter = None;
        self.creations_status_filter = None;
        self.quick_drawer_kind_filter = None;
        self.sort_mode = CatalogSortMode::Featured;
        self.active_input = ContentHubInputTarget::None;
        self.is_loading_creations = false;
        self.is_loading_catalog = false;
        self.error_message = None;
        self.publish_modal.reset_for_logout_or_account_switch();
    }
}

impl ContentHubWorkspace {
    pub(crate) fn active_catalog_items(&self, section: ContentHubSection) -> Vec<PublicCatalogItemDto> {
        let base: Vec<PublicCatalogItemDto> = match section {
            ContentHubSection::Marketplace => Vec::new(),
            ContentHubSection::Books => self.books_catalog.clone(),
            ContentHubSection::Music => self.music_catalog.clone(),
            ContentHubSection::Games => self.games_catalog.clone(),
            ContentHubSection::AppStore => {
                let mut merged = Vec::new();
                for item in self
                    .appstore_catalog
                    .featured
                    .iter()
                    .chain(self.appstore_catalog.apps.iter())
                    .chain(self.appstore_catalog.cross_vertical.iter())
                {
                    if !merged
                        .iter()
                        .any(|m: &PublicCatalogItemDto| m.creation_id == item.creation_id)
                    {
                        merged.push(item.clone());
                    }
                }
                merged
            }
            ContentHubSection::MyCreations => Vec::new(),
        };
        let q = self.catalog_search.text.trim().to_lowercase();
        if q.is_empty() {
            base
        } else {
            base.into_iter()
                .filter(|item| {
                    item.title.to_lowercase().contains(&q)
                        || item.subtitle.to_lowercase().contains(&q)
                        || item.summary.to_lowercase().contains(&q)
                        || item.author_display_name.to_lowercase().contains(&q)
                })
                .collect()
        }
    }

    pub(crate) fn resolve_selected_detail_creation(&self) -> Option<UserCreationDto> {
        let selected_id = self.selected_creation_id.as_deref()?;
        if let Some(detail) = self
            .selected_detail_creation
            .as_ref()
            .filter(|d| d.creation_id == selected_id)
        {
            return Some(detail.clone());
        }
        let cat = self
            .selected_catalog_item
            .as_ref()
            .filter(|c| c.creation_id == selected_id)?;
        let fallback_payload = cat.payload.clone().unwrap_or_else(|| match cat.kind {
            CreationKind::Book => CreationPayload::Book(gpui_telegram_proto::creations::BookPayload {
                author: cat.author_display_name.clone(),
                series_name: None,
                series_index: None,
                blurb: cat.summary.clone(),
                estimated_read_mins: 5,
                word_count: cat.summary.chars().count() as u32,
                chapters: Vec::new(),
            }),
            CreationKind::Music => CreationPayload::Music(gpui_telegram_proto::creations::MusicPayload {
                artist: cat.author_display_name.clone(),
                album_title: cat.title.clone(),
                release_year: 2026,
                primary_genre: cat.genres.first().cloned().unwrap_or_default(),
                mood_tags: cat.tags.clone(),
                bpm: 112,
                musical_key: "C Major".to_string(),
                time_signature: "4/4".to_string(),
                total_duration_secs: 60,
                tracks: Vec::new(),
            }),
            CreationKind::Game => CreationPayload::Game(gpui_telegram_proto::creations::GamePayload {
                developer: cat.author_display_name.clone(),
                category: gpui_telegram_proto::creations::GameCategory::Puzzle,
                controls_hint: String::new(),
                rules_markdown: cat.summary.clone(),
                engine_kind: gpui_telegram_proto::creations::GameEngineKind::TriviaQuiz,
                supports_leaderboard: true,
                estimated_session_mins: 5,
                spec: gpui_telegram_proto::creations::GameSpec::TriviaQuiz {
                    time_limit_secs: None,
                    questions: Vec::new(),
                },
                achievements: Vec::new(),
            }),
            CreationKind::App => CreationPayload::App(gpui_telegram_proto::creations::AppStoreListingPayload {
                developer_name: cat.author_display_name.clone(),
                version_string: format!("v{}.0", cat.published_version),
                release_notes: String::new(),
                category: cat.genres.first().cloned().unwrap_or_else(|| "Utilities".to_string()),
                long_description: cat.summary.clone(),
                screenshots: Vec::new(),
                size_label: "Cloud App".to_string(),
                price_label: "Free".to_string(),
                permissions: Vec::new(),
                linked_miniapp: None,
                ui_schema_json: None,
            }),
        });
        Some(UserCreationDto {
            creation_id: cat.creation_id.clone(),
            owner_user_id: cat.owner_user_id,
            creator_bot_user_id: None,
            creator_chat_id: None,
            kind: cat.kind,
            title: cat.title.clone(),
            subtitle: cat.subtitle.clone(),
            summary: cat.summary.clone(),
            author_display_name: cat.author_display_name.clone(),
            language: cat.language.clone(),
            genres: cat.genres.clone(),
            tags: cat.tags.clone(),
            cover: cat.cover.clone(),
            age_rating: cat.age_rating.clone(),
            payload: fallback_payload,
            version: cat.published_version,
            cloud_revision: 0,
            review_status: CreationReviewStatus::Published,
            visibility: gpui_telegram_proto::creations::CreationVisibility::Public,
            has_live_published_version: true,
            published_version: Some(cat.published_version),
            rejection_reason: None,
            review_notes: None,
            submitted_at_ms: None,
            reviewed_at_ms: None,
            published_at_ms: Some(cat.published_at_ms),
            created_at_ms: cat.published_at_ms,
            updated_at_ms: cat.updated_at_ms,
            deleted_at_ms: None,
            rating_avg: cat.rating_avg,
            rating_count: cat.rating_count,
            engagement_count: cat.engagement_count,
            save_or_install_count: cat.save_or_install_count,
        })
    }
}

/// Shared OpenBot-styled Ratings & Reviews panel rendered across Books, Music, Games, and App Store detail views.
pub(crate) fn render_community_reviews_panel(
    ws: &mut ContentHubWorkspace,
    creation_id: &str,
    can_review: bool,
    _accent_rgb: u32,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let ob = pal.openbot_palette();
    let lang = ws.language_id.clone();
    let reviews = ws.active_reviews.clone();
    let draft_rating = ws.review_draft_rating.clamp(1, 5);
    let title_field = ws.review_draft_title.clone();
    let body_field = ws.review_draft_body.clone();
    let title_focused = ws.active_input == ContentHubInputTarget::ReviewTitle;
    let body_focused = ws.active_input == ContentHubInputTarget::ReviewBody;
    let notice = ws.review_status_notice.clone();
    let cid = creation_id.to_string();

    apple_grouped_card(pal)
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(apple_shelf_header(
            tr_str(&lang, "评分与评论", "Ratings & Reviews"),
            Some(format!(
                "{} {}",
                reviews.len(),
                tr_str(&lang, "条评论", "reviews")
            )),
            pal,
        ))
        .when(can_review, |el| {
            let cid_submit = cid.clone();
            el.child(
                div()
                    .p(px(12.0))
                    .rounded(px(10.0))
                    .bg(ob.bg_canvas())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_muted())
                                    .child(tr_str(&lang, "打分：", "Tap to Rate:")),
                            )
                            .children((1u8..=5u8).map(|star| {
                                let filled = star <= draft_rating;
                                div()
                                    .id(SharedString::from(format!("review-star-{star}")))
                                    .cursor_pointer()
                                    .px(px(4.0))
                                    .text_size(px(16.0))
                                    .text_color(if filled { ob.warning() } else { ob.text_dim() })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.review_draft_rating = star;
                                            cx.notify();
                                        }),
                                    )
                                    .child(if filled { "★" } else { "☆" })
                            })),
                    )
                    .child(
                        render_editable_field_box(
                            "content-hub-review-title-input",
                            &title_field,
                            tr_str(&lang, "评论标题（可选）", "Review Title (optional)"),
                            title_focused,
                            false,
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_input(ContentHubInputTarget::ReviewTitle, cx);
                            }),
                        ),
                    )
                    .child(
                        render_editable_field_box(
                            "content-hub-review-body-input",
                            &body_field,
                            tr_str(&lang, "写下你对这部作品的评价…", "Write a review…"),
                            body_focused,
                            true,
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_input(ContentHubInputTarget::ReviewBody, cx);
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(ob.text_secondary())
                                    .child(notice.unwrap_or_default()),
                            )
                            .child(
                                openbot_primary_cta_btn(
                                    "content-hub-review-submit-btn",
                                    tr_str(&lang, "发表评论", "Submit Review"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        let body = this.review_draft_body.text.trim().to_string();
                                        if body.is_empty() {
                                            this.review_status_notice = Some(
                                                tr_str(
                                                    &this.language_id,
                                                    "请先填写评论内容",
                                                    "Please write a review comment first",
                                                )
                                                .to_string(),
                                            );
                                            cx.notify();
                                            return;
                                        }
                                        let title = {
                                            let t = this.review_draft_title.text.trim().to_string();
                                            (!t.is_empty()).then_some(t)
                                        };
                                        this.is_submitting_review = true;
                                        this.review_status_notice = None;
                                        cx.emit(ContentHubEvent::SubmitCreationReview {
                                            creation_id: cid_submit.clone(),
                                            req: CreateCreationReviewRequest {
                                                rating: this.review_draft_rating.clamp(1, 5),
                                                title,
                                                body,
                                            },
                                        });
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            )
        })
        .when(reviews.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(ob.text_muted())
                    .child(tr_str(
                        &lang,
                        "暂无社区评价，欢迎留下第一条评论。",
                        "No community reviews yet — be the first to rate this creation.",
                    )),
            )
        })
        .when(!reviews.is_empty(), |el| {
            el.children(reviews.iter().take(12).map(|rev| {
                let stars: String = (0..5)
                    .map(|i| if i < rev.rating as usize { '★' } else { '☆' })
                    .collect();
                div()
                    .p(px(10.0))
                    .rounded(px(10.0))
                    .bg(ob.bg_canvas())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_primary())
                                    .child(rev.title.clone().unwrap_or_else(|| {
                                        if rev.author_display_name.trim().is_empty() {
                                            format!("User {}", rev.user_id)
                                        } else {
                                            rev.author_display_name.clone()
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(ob.warning())
                                    .child(stars),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(ob.text_secondary())
                            .child(rev.body.clone()),
                    )
            }))
        })
        .into_any_element()
}

impl Render for ContentHubWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let vp_w = f32::from(window.viewport_size().width);
        let active_page = match self.active_section {
            ContentHubSection::Marketplace => "marketplace",
            ContentHubSection::Books => "books",
            ContentHubSection::Music => "music",
            ContentHubSection::Games => "games",
            ContentHubSection::AppStore => "app_store",
            ContentHubSection::MyCreations => "my_creations",
        };
        let shell_layout =
            apple_primitives::compute_content_hub_shell_layout(&pal, vp_w, active_page);
        let canvas_bg = rgb(apple_primitives::parse_hex_color(
            &shell_layout.canvas_hex,
            0x1A1A1A,
        ));
        let show_music_bar = self.music_engine.current_track().is_some()
            || (self.active_section == ContentHubSection::Music
                && self.active_game.is_none()
                && self.book_reader.is_none());

        let body = if self.active_game.is_some() {
            game_runtime_sheet::render_game_runtime_sheet(self, cx)
        } else if self.book_reader.is_some() {
            book_reader::render_book_reader(self, cx)
        } else {
            match self.active_section {
                ContentHubSection::Marketplace => {
                    marketplace_view::render_marketplace_hub_view(self, cx)
                }
                ContentHubSection::Books => {
                    if self.marketplace.active_tab != MarketplaceNavTab::Books {
                        self.marketplace.select_tab(MarketplaceNavTab::Books);
                    }
                    marketplace_view::render_marketplace_hub_view(self, cx)
                }
                ContentHubSection::Games => {
                    if self.marketplace.active_tab != MarketplaceNavTab::Games {
                        self.marketplace.select_tab(MarketplaceNavTab::Games);
                    }
                    marketplace_view::render_marketplace_hub_view(self, cx)
                }
                ContentHubSection::AppStore => {
                    if self.marketplace.active_tab != MarketplaceNavTab::MiniApps {
                        self.marketplace.select_tab(MarketplaceNavTab::MiniApps);
                    }
                    marketplace_view::render_marketplace_hub_view(self, cx)
                }
                ContentHubSection::Music => music_view::render_music_hub_view(self, cx),
                ContentHubSection::MyCreations => render_my_creations_view(self, cx),
            }
        };

        div()
            .id("content-hub-workspace")
            .size_full()
            .bg(canvas_bg)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(div().flex_1().min_h(px(0.0)).w_full().overflow_hidden().child(body))
            .when(show_music_bar, |el| {
                el.child(music_player_bar::render_music_player_bar(self, cx))
            })
    }
}
