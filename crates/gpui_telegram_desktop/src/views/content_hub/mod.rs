//! Content Hub views and pure-Rust interactive engines.

pub mod game_runtime;
pub use game_runtime::*;

pub mod apple_primitives;
pub mod creations_view;
pub mod publish_review_modal;

use std::collections::HashMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_core::core::deep_links::{AppleDeepLink, AppleDeepLinkSection};
use gpui_telegram_media::media::audio::MusicTransportEngine;
use gpui_telegram_proto::creations::{
    AppStoreCatalogResponse, AppStoreMiniAppEntry, CatalogSortMode, CreationKind,
    CreationReviewStatus, PublicCatalogItemDto, ReviewDecisionRequest, SubmitReviewRequest,
    UpdateCreationRequest, UserCreationDto, UserCreationStateDto,
};

use crate::components::ui::TextInputController;
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};

pub use apple_primitives::EditableFieldState;
pub use creations_view::{
    render_my_creations_view, render_quick_creations_drawer, QUICK_DRAWER_WIDTH,
};
pub use publish_review_modal::{
    render_publish_review_modal_body, PublishModalField, PublishReviewModalState,
    ReviewerAccessState,
};

use apple_primitives::{
    apple_grouped_card, cover_artwork_box, creation_kind_accent, creation_kind_label,
    empty_state_card_shell, filter_pill, hero_banner, rating_pill, render_editable_field_box,
};

/// Top-level creative vertical or personal cloud studio section mounted inside `ContentHubWorkspace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentHubSection {
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
            Self::Books => Some(CreationKind::Book),
            Self::Music => Some(CreationKind::Music),
            Self::Games => Some(CreationKind::Game),
            Self::AppStore => Some(CreationKind::App),
            Self::MyCreations => None,
        }
    }

    pub fn default_sub_tab(self) -> &'static str {
        match self {
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
}

/// Typed events emitted by `ContentHubWorkspace` to `AppShell` (`shell_content_hub.rs`).
#[derive(Debug, Clone)]
pub enum ContentHubEvent {
    RefreshMyCreations,
    RefreshCatalog(ContentHubSection),
    RefreshReviewQueue,
    OpenCreatorBot(Option<CreationKind>),
    OpenCreatorBotWithCommand(String),
    OpenCreationDetail {
        section: ContentHubSection,
        creation_id: String,
        action: Option<String>,
    },
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
    LaunchMiniApp {
        bot_username: String,
        short_name: String,
        web_url: Option<String>,
    },
    CloseQuickDrawer,
    ClosePublishReviewModal,
    NavigateSection(ContentHubSection),
    FocusInputTaken,
}

/// Primary Apple Content Hub workspace entity holding synced cloud creations, vertical catalogs,
/// headless-safe `MusicTransportEngine`, Quick Creations Drawer state, and `PublishReviewModalState`.
pub struct ContentHubWorkspace {
    pub active_section: ContentHubSection,
    pub active_sub_tab: String,
    pub active_action_hint: Option<String>,
    pub quick_drawer_open: bool,
    pub music_engine: MusicTransportEngine,
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
            active_section: ContentHubSection::Books,
            active_sub_tab: ContentHubSection::Books.default_sub_tab().to_string(),
            active_action_hint: None,
            quick_drawer_open: false,
            music_engine: MusicTransportEngine::new(),
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
            self.is_loading_catalog = false;
            if self.active_input != ContentHubInputTarget::QuickDrawerSearch {
                self.active_input = ContentHubInputTarget::None;
            }
        }
    }

    pub fn is_publish_review_modal_open(&self) -> bool {
        self.publish_modal.is_open
    }

    pub fn has_focused_input(&self, in_content_hub: bool) -> bool {
        match self.active_input {
            ContentHubInputTarget::None => false,
            ContentHubInputTarget::QuickDrawerSearch => self.quick_drawer_open,
            ContentHubInputTarget::CatalogSearch | ContentHubInputTarget::CreationsSearch => {
                in_content_hub
            }
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
    }

    fn active_field(&self) -> Option<&EditableFieldState> {
        match self.active_input {
            ContentHubInputTarget::None => None,
            ContentHubInputTarget::CatalogSearch => Some(&self.catalog_search),
            ContentHubInputTarget::CreationsSearch => Some(&self.creations_search),
            ContentHubInputTarget::QuickDrawerSearch => Some(&self.quick_drawer_search),
        }
    }

    fn active_field_mut(&mut self) -> Option<&mut EditableFieldState> {
        match self.active_input {
            ContentHubInputTarget::None => None,
            ContentHubInputTarget::CatalogSearch => Some(&mut self.catalog_search),
            ContentHubInputTarget::CreationsSearch => Some(&mut self.creations_search),
            ContentHubInputTarget::QuickDrawerSearch => Some(&mut self.quick_drawer_search),
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
        } else if let Some(field) = self.active_field_mut() {
            field.set_ime_preedit(preedit);
        }
    }

    pub fn clear_all_preedits(&mut self) -> bool {
        let had = self.catalog_search.ime_preedit.is_some()
            || self.creations_search.ime_preedit.is_some()
            || self.quick_drawer_search.ime_preedit.is_some()
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
        if self.selected_creation_id.as_deref() == Some(creation_id)
            && self.find_my_creation(creation_id).is_none()
        {
            self.selected_creation_id = None;
            self.selected_detail_creation = None;
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
        if let Some(cid) = link.creation_id.as_deref() {
            if let Some(cat) = self.find_catalog_item(cid).cloned() {
                self.selected_catalog_item = Some(cat);
            }
            if let Some(own) = self.find_my_creation(cid).cloned() {
                self.selected_detail_creation = Some(own);
            }
        }
    }

    pub fn reset_for_logout_or_account_switch(&mut self) {
        self.music_engine.stop();
        self.music_engine.clear_queue();
        self.active_section = ContentHubSection::Books;
        self.active_sub_tab = ContentHubSection::Books.default_sub_tab().to_string();
        self.active_action_hint = None;
        self.quick_drawer_open = false;
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
    fn active_catalog_items(&self, section: ContentHubSection) -> Vec<PublicCatalogItemDto> {
        let base: Vec<PublicCatalogItemDto> = match section {
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

    fn render_vertical_hub(
        &mut self,
        section: ContentHubSection,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.clone();
        let search_focused = self.active_input == ContentHubInputTarget::CatalogSearch;
        let search_field = self.catalog_search.clone();
        let sort_mode = self.sort_mode;
        let items = self.active_catalog_items(section);
        let selected_id = self.selected_creation_id.clone();
        let selected_detail = self
            .selected_detail_creation
            .as_ref()
            .filter(|d| selected_id.as_deref() == Some(d.creation_id.as_str()))
            .cloned();
        let selected_cat = self
            .selected_catalog_item
            .as_ref()
            .filter(|c| selected_id.as_deref() == Some(c.creation_id.as_str()))
            .cloned();
        let miniapps: Vec<AppStoreMiniAppEntry> = if section == ContentHubSection::AppStore {
            if !self.appstore_catalog.mini_apps.is_empty() {
                self.appstore_catalog.mini_apps.clone()
            } else {
                self.local_miniapps_fallback.clone()
            }
        } else {
            Vec::new()
        };

        let (kicker, title, subtitle, grad_start, grad_end, icon, create_kind) = match section {
            ContentHubSection::Books => (
                tr_str(&lang, "APPLE BOOKS 风格书库", "APPLE BOOKS"),
                tr_str(&lang, "图书与互动阅读", "Books & Reading"),
                tr_str(
                    &lang,
                    "探索已审核公开上架的原创图书，或使用 @CreatorStudioBot 创作新书。",
                    "Discover published community books or craft your own with @CreatorStudioBot.",
                ),
                "#FF9500",
                "#FF5E3A",
                IconName::HubBooks,
                Some(CreationKind::Book),
            ),
            ContentHubSection::Music => (
                tr_str(&lang, "APPLE MUSIC 风格乐库", "APPLE MUSIC"),
                tr_str(&lang, "音乐与原创专辑", "Music & Albums"),
                tr_str(
                    &lang,
                    "浏览公开上架的合成曲目与歌词专辑，随时播放与收藏。",
                    "Browse published synthesized albums and tracks with synced lyrics.",
                ),
                "#FA2D48",
                "#A21CAF",
                IconName::HubMusic,
                Some(CreationKind::Music),
            ),
            ContentHubSection::Games => (
                tr_str(&lang, "APPLE GAMES & ARCADE", "APPLE GAMES & ARCADE"),
                tr_str(&lang, "游戏与街机排行", "Games & Arcade"),
                tr_str(
                    &lang,
                    "即点即玩已审核上架的益智、问答与街机小游戏，挑战云端排行榜。",
                    "Play published puzzles, trivia, and arcade games with cloud leaderboards.",
                ),
                "#5856D6",
                "#007AFF",
                IconName::HubGames,
                Some(CreationKind::Game),
            ),
            ContentHubSection::AppStore | ContentHubSection::MyCreations => (
                tr_str(&lang, "APPLE APP STORE 统一商店", "APPLE APP STORE"),
                tr_str(&lang, "发现应用、小程序与精选创作", "Today · Apps & Featured Works"),
                tr_str(
                    &lang,
                    "汇集公开上架应用、已发布 Mini Apps 以及精选图书、音乐与游戏。",
                    "Unified storefront of published apps, Mini Apps, and featured books, music & games.",
                ),
                "#007AFF",
                "#34C759",
                IconName::HubAppStore,
                None,
            ),
        };

        let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
            (CatalogSortMode::Featured, "hub-sort-featured", "精选", "Featured"),
            (CatalogSortMode::Popular, "hub-sort-popular", "热门", "Popular"),
            (CatalogSortMode::TopRated, "hub-sort-rated", "高分", "Top Rated"),
            (CatalogSortMode::Newest, "hub-sort-newest", "最新", "Newest"),
        ];

        let (cover_w, cover_h) = if section == ContentHubSection::Books {
            (72.0, 108.0)
        } else {
            (84.0, 84.0)
        };

        div()
            .id("content-hub-vertical-root")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .px(px(24.0))
                    .py(px(14.0))
                    .border_b_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div().w(px(280.0)).child(
                            render_editable_field_box(
                                "content-hub-catalog-search-input",
                                &search_field,
                                tr_str(&lang, "搜索公开上架内容…", "Search published catalog…"),
                                search_focused,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(ContentHubInputTarget::CatalogSearch, cx);
                                }),
                            ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .children(sort_options.iter().map(|(mode, pill_id, zh, en)| {
                                let m = *mode;
                                filter_pill(*pill_id, tr_str(&lang, zh, en), sort_mode == m, pal)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.sort_mode = m;
                                            cx.emit(ContentHubEvent::RefreshCatalog(section));
                                            cx.notify();
                                        }),
                                    )
                            }))
                            .child(
                                macos_secondary_pill_btn(
                                    "content-hub-refresh-catalog-btn",
                                    tr_str(&lang, "刷新", "Refresh"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_this, _, _, cx| {
                                        cx.emit(ContentHubEvent::RefreshCatalog(section));
                                    }),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .id("content-hub-vertical-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .p(px(24.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(hero_banner(
                        "content-hub-hero-banner",
                        kicker,
                        title,
                        subtitle,
                        grad_start,
                        grad_end,
                        icon,
                        pal,
                    ))
                    .when_some(selected_detail.or_else(|| {
                        selected_cat.as_ref().map(|c| {
                            UserCreationDto {
                                creation_id: c.creation_id.clone(),
                                owner_user_id: c.owner_user_id,
                                creator_bot_user_id: None,
                                creator_chat_id: None,
                                kind: c.kind,
                                title: c.title.clone(),
                                subtitle: c.subtitle.clone(),
                                summary: c.summary.clone(),
                                author_display_name: c.author_display_name.clone(),
                                language: c.language.clone(),
                                genres: c.genres.clone(),
                                tags: c.tags.clone(),
                                cover: c.cover.clone(),
                                age_rating: c.age_rating.clone(),
                                payload: c.payload.clone().unwrap_or_else(|| {
                                    gpui_telegram_proto::creations::CreationPayload::Book(
                                        gpui_telegram_proto::creations::BookPayload {
                                            author: c.author_display_name.clone(),
                                            series_name: None,
                                            series_index: None,
                                            blurb: c.summary.clone(),
                                            estimated_read_mins: 5,
                                            word_count: 100,
                                            chapters: Vec::new(),
                                        },
                                    )
                                }),
                                version: c.published_version,
                                cloud_revision: 0,
                                review_status: CreationReviewStatus::Published,
                                visibility: gpui_telegram_proto::creations::CreationVisibility::Public,
                                has_live_published_version: true,
                                published_version: Some(c.published_version),
                                rejection_reason: None,
                                review_notes: None,
                                submitted_at_ms: None,
                                reviewed_at_ms: None,
                                published_at_ms: Some(c.published_at_ms),
                                created_at_ms: c.published_at_ms,
                                updated_at_ms: c.updated_at_ms,
                                deleted_at_ms: None,
                                rating_avg: c.rating_avg,
                                rating_count: c.rating_count,
                                engagement_count: c.engagement_count,
                                save_or_install_count: c.save_or_install_count,
                            }
                        })
                    }), |el, detail| {
                        let accent = creation_kind_accent(detail.kind);
                        let action_label = self
                            .active_action_hint
                            .as_deref()
                            .unwrap_or("open")
                            .to_string();
                        el.child(
                            apple_grouped_card(pal)
                                .p(px(16.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(14.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(12.0))
                                        .child(cover_artwork_box(&detail.cover, 60.0, 60.0, 12.0, accent))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(3.0))
                                                .child(
                                                    div()
                                                        .text_size(px(16.0))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(pal.text_primary))
                                                        .child(detail.title.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(pal.text_secondary))
                                                        .child(format!(
                                                            "{} · {} · action={}",
                                                            creation_kind_label(detail.kind, &lang),
                                                            detail.author_display_name,
                                                            action_label
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(pal.text_secondary))
                                                        .child(detail.summary.clone()),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("content-hub-detail-close-btn")
                                        .cursor_pointer()
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .rounded(px(7.0))
                                        .hover(move |s| s.bg(pal.row_hover_rgba()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.selected_creation_id = None;
                                                this.selected_catalog_item = None;
                                                this.selected_detail_creation = None;
                                                cx.notify();
                                            }),
                                        )
                                        .child(tg_icon(IconName::Close, 14.0, pal.icon_fg)),
                                ),
                        )
                    })
                    .when(!miniapps.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(&lang, "精选 Mini Apps", "Featured Mini Apps")),
                                )
                                .child(div().flex().flex_wrap().gap(px(10.0)).children(
                                    miniapps.iter().map(|app| {
                                        let bot_un = app.bot_username.clone();
                                        let short = app.short_name.clone();
                                        let web_url = (!app.web_url.trim().is_empty())
                                            .then(|| app.web_url.clone());
                                        apple_grouped_card(pal)
                                            .w(px(260.0))
                                            .p(px(12.0))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(10.0))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(2.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(13.0))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(pal.text_primary))
                                                            .child(app.title.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(pal.text_secondary))
                                                            .child(app.description.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "appstore-launch-miniapp-{}-{}",
                                                        app.bot_username, app.short_name
                                                    )))
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(4.0))
                                                    .rounded(px(999.0))
                                                    .bg(rgb(pal.accent))
                                                    .text_size(px(11.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(pal.active_button_fg))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |_this, _, _, cx| {
                                                            cx.emit(ContentHubEvent::LaunchMiniApp {
                                                                bot_username: bot_un.clone(),
                                                                short_name: short.clone(),
                                                                web_url: web_url.clone(),
                                                            });
                                                        }),
                                                    )
                                                    .child(tr_str(&lang, "打开", "Open")),
                                            )
                                    }),
                                )),
                        )
                    })
                    .when(items.is_empty() && miniapps.is_empty(), |el| {
                        let cta = macos_primary_pill_btn(
                            "content-hub-empty-bot-cta",
                            tr_str(&lang, "用 @CreatorStudioBot 创作", "Create with @CreatorStudioBot"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ContentHubEvent::OpenCreatorBot(create_kind));
                            }),
                        );
                        el.child(empty_state_card_shell(
                            icon,
                            tr_str(&lang, "暂无公开上架作品", "No Published Works Yet"),
                            tr_str(
                                &lang,
                                "创作者通过 @CreatorStudioBot 创作并审核上架后，作品会立即展示在这里。",
                                "Published creations approved for the public catalog appear here automatically.",
                            ),
                            cta,
                            pal,
                        ))
                    })
                    .when(!items.is_empty(), |el| {
                        el.child(div().flex().flex_wrap().gap(px(14.0)).children(items.iter().map(
                            |item| {
                                let cid = item.creation_id.clone();
                                let item_clone = item.clone();
                                let accent = creation_kind_accent(item.kind);
                                let is_sel = selected_id.as_deref() == Some(cid.as_str());
                                div()
                                    .id(SharedString::from(format!("catalog-card-{cid}")))
                                    .cursor_pointer()
                                    .w(px(280.0))
                                    .rounded(px(12.0))
                                    .bg(pal.card_surface_rgba())
                                    .border_1()
                                    .border_color(if is_sel {
                                        TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.72)
                                    } else {
                                        pal.separator_rgba()
                                    })
                                    .p(px(12.0))
                                    .flex()
                                    .items_start()
                                    .gap(px(12.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.selected_creation_id = Some(cid.clone());
                                            this.selected_catalog_item = Some(item_clone.clone());
                                            if this
                                                .selected_detail_creation
                                                .as_ref()
                                                .is_some_and(|d| d.creation_id != cid)
                                            {
                                                this.selected_detail_creation =
                                                    this.find_my_creation(&cid).cloned();
                                            }
                                            cx.emit(ContentHubEvent::OpenCreationDetail {
                                                section,
                                                creation_id: cid.clone(),
                                                action: Some("open".to_string()),
                                            });
                                            cx.notify();
                                        }),
                                    )
                                    .child(cover_artwork_box(
                                        &item.cover,
                                        cover_w,
                                        cover_h,
                                        10.0,
                                        accent,
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .text_size(px(14.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(item.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(item.author_display_name.clone()),
                                            )
                                            .child(rating_pill(
                                                item.rating_avg,
                                                item.rating_count,
                                                pal,
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(item.summary.clone()),
                                            ),
                                    )
                            },
                        )))
                    }),
            )
            .into_any_element()
    }
}

impl Render for ContentHubWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let body = match self.active_section {
            ContentHubSection::MyCreations => render_my_creations_view(self, cx),
            other => self.render_vertical_hub(other, cx),
        };
        div()
            .id("content-hub-workspace")
            .size_full()
            .bg(rgb(pal.stage_bg))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(body)
    }
}
