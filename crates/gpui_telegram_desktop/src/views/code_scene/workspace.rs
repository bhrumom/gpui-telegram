//! Root `CodeSceneWorkspace` GPUI entity combining the hybrid 60fps `CodeSceneViewport`,
//! Douyin vertical snap gestures & double-tap heart burst, floating HUD, 4 slide-over drawers,
//! local-first `LocalCodeSceneService` + live server sync, and `[i-1, i, i+1]` pre-warming.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::LocalCodeSceneService;
use gpui_telegram_code_scene::{compile_and_verify, VerifiedProgram};
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneRequest, AiIterateSceneRequest, AiPublishSceneRequest, AssetKind,
    CodeSceneAssetDto, CodeSceneCommentDto, CodeSceneCommentSort, CodeSceneFeedTab,
    CreateCodeSceneCommentRequest, ListCodeSceneAssetsQuery, ListCodeSceneCommentsQuery,
    PublishCodeSceneRequest, RankedSceneItemDto, RemixSceneRequest, SceneBehaviorEventKind,
    ValidationReportDto, CODE_SCENE_FORMAT_CS1,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::code_scene::drawers::{
    render_active_drawer, ActiveCodeSceneInput, AiStudioState, CodeSceneDrawer, DrawerAction,
    DrawerRenderProps,
};
use crate::views::code_scene::gesture::{HeartBurst, SwipeGestureController, SwipeSnapDirection};
use crate::views::code_scene::hud_overlay::{
    render_bottom_metadata_and_scrubber, render_right_action_rail, render_top_floating_bar,
    HudAction, HudRenderProps,
};
use crate::views::code_scene::viewport::CodeSceneViewport;

/// Shell-level action emitted by `CodeSceneWorkspace` and consumed by `AppShell` observer.
#[derive(Debug, Clone)]
pub enum CodeScenePendingAction {
    ExitToChats,
    ToggleSidebarCollapse,
    ShareSceneToChat {
        chat_id: u64,
        asset: CodeSceneAssetDto,
    },
    SharePublishedToActiveChat {
        asset: CodeSceneAssetDto,
    },
    NotifyToast(String),
}

/// Configuration used by `AppShell` or headless tests to construct a `CodeSceneWorkspace`.
pub struct CodeSceneWorkspaceConfig {
    pub service: Arc<LocalCodeSceneService>,
    pub tokio_handle: Option<tokio::runtime::Handle>,
    pub session_token: Option<String>,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

/// Full-pane Douyin-like vertical feed workspace for executable Code Scene assets.
pub struct CodeSceneWorkspace {
    pub service: Arc<LocalCodeSceneService>,
    pub tokio_handle: Option<tokio::runtime::Handle>,
    pub session_token: Option<String>,
    pub viewport: CodeSceneViewport,
    pub studio_preview_viewport: CodeSceneViewport,
    pub prewarm_cache: HashMap<String, VerifiedProgram>,
    pub gesture: SwipeGestureController,
    pub heart_bursts: Vec<HeartBurst>,
    pub next_heart_id: u64,
    pub active_drawer: CodeSceneDrawer,
    pub active_input: ActiveCodeSceneInput,
    pub ime_preedit: Option<String>,
    pub show_more_menu: bool,
    pub show_score_breakdown: bool,
    pub search_open: bool,
    pub search_query: String,
    pub comment_draft: String,
    pub reply_target: Option<(String, String)>,
    pub comments: Vec<CodeSceneCommentDto>,
    pub comment_sort: CodeSceneCommentSort,
    pub editor_source: String,
    pub editor_validation: Option<ValidationReportDto>,
    pub studio: AiStudioState,
    pub share_targets: Vec<(u64, String, String, u32)>,
    pub pending_action: Option<CodeScenePendingAction>,
    pub is_visible: bool,
    pub is_occluded: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub focus_handle: FocusHandle,
}

impl CodeSceneWorkspace {
    pub fn new(cfg: CodeSceneWorkspaceConfig, cx: &mut Context<Self>) -> Self {
        cfg.service.ensure_feed_loaded();
        let mut ws = Self {
            service: cfg.service,
            tokio_handle: cfg.tokio_handle,
            session_token: cfg.session_token,
            viewport: CodeSceneViewport::new(),
            studio_preview_viewport: CodeSceneViewport::new(),
            prewarm_cache: HashMap::new(),
            gesture: SwipeGestureController::new(),
            heart_bursts: Vec::new(),
            next_heart_id: 1,
            active_drawer: CodeSceneDrawer::None,
            active_input: ActiveCodeSceneInput::None,
            ime_preedit: None,
            show_more_menu: false,
            show_score_breakdown: false,
            search_open: false,
            search_query: String::new(),
            comment_draft: String::new(),
            reply_target: None,
            comments: Vec::new(),
            comment_sort: CodeSceneCommentSort::Hot,
            editor_source: String::new(),
            editor_validation: None,
            studio: AiStudioState::default(),
            share_targets: Vec::new(),
            pending_action: None,
            is_visible: true,
            is_occluded: false,
            palette: cfg.palette,
            language_id: cfg.language_id,
            focus_handle: cx.focus_handle(),
        };
        ws.load_current_card_into_viewport(cx);
        ws.prewarm_neighbor_cards();
        ws
    }

    pub fn set_session_token(&mut self, token: Option<String>) {
        self.session_token = token;
    }

    pub fn sync_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        lang_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        self.language_id = lang_id.into();
        cx.notify();
    }

    pub fn sync_share_targets(&mut self, targets: Vec<(u64, String, String, u32)>) {
        self.share_targets = targets;
    }

    pub fn take_pending_action(&mut self) -> Option<CodeScenePendingAction> {
        self.pending_action.take()
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.is_visible == visible {
            return;
        }
        self.is_visible = visible;
        if !visible {
            self.viewport.set_paused(true);
            self.viewport.set_game_input_captured(false);
            self.studio_preview_viewport.set_paused(true);
            self.service.flush_active_card_telemetry();
            self.active_input = ActiveCodeSceneInput::None;
            self.ime_preedit = None;
        } else {
            if self.viewport.vm.is_none() {
                self.service.ensure_feed_loaded();
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
            } else {
                self.viewport.set_paused(false);
            }
            self.trigger_background_sync(cx);
        }
        cx.notify();
    }

    pub fn set_occluded(&mut self, occluded: bool, cx: &mut Context<Self>) {
        if self.is_occluded == occluded {
            return;
        }
        self.is_occluded = occluded;
        if occluded {
            self.viewport.set_paused(true);
            self.viewport.set_game_input_captured(false);
        } else if self.is_visible {
            self.viewport.set_paused(false);
        }
        cx.notify();
    }

    pub fn teardown(&mut self, cx: &mut Context<Self>) {
        self.service.flush_active_card_telemetry();
        self.viewport.unload(cx);
        self.studio_preview_viewport.unload(cx);
        self.prewarm_cache.clear();
        self.heart_bursts.clear();
        self.active_drawer = CodeSceneDrawer::None;
        self.active_input = ActiveCodeSceneInput::None;
        self.ime_preedit = None;
    }

    /// Returns the currently active ranked feed item.
    pub fn current_item(&self) -> Option<RankedSceneItemDto> {
        self.service.current_item()
    }

    pub fn is_game_input_captured(&self) -> bool {
        self.is_visible
            && !self.is_occluded
            && self.active_drawer == CodeSceneDrawer::None
            && !self.has_focused_text_input()
            && self.viewport.game_input_captured
    }

    pub fn has_focused_text_input(&self) -> bool {
        self.active_input != ActiveCodeSceneInput::None
    }

    pub fn active_input_text(&self) -> String {
        match self.active_input {
            ActiveCodeSceneInput::None => String::new(),
            ActiveCodeSceneInput::Search => self.search_query.clone(),
            ActiveCodeSceneInput::Comment => self.comment_draft.clone(),
            ActiveCodeSceneInput::CodeEditor => self.editor_source.clone(),
            ActiveCodeSceneInput::StudioPrompt => self.studio.prompt.clone(),
            ActiveCodeSceneInput::StudioIterate => self.studio.iterate_instruction.clone(),
            ActiveCodeSceneInput::StudioTitle => self.studio.title.clone(),
            ActiveCodeSceneInput::StudioTags => self.studio.tags_input.clone(),
        }
    }

    fn active_input_buf_mut(&mut self) -> Option<&mut String> {
        match self.active_input {
            ActiveCodeSceneInput::None => None,
            ActiveCodeSceneInput::Search => Some(&mut self.search_query),
            ActiveCodeSceneInput::Comment => Some(&mut self.comment_draft),
            ActiveCodeSceneInput::CodeEditor => Some(&mut self.editor_source),
            ActiveCodeSceneInput::StudioPrompt => Some(&mut self.studio.prompt),
            ActiveCodeSceneInput::StudioIterate => Some(&mut self.studio.iterate_instruction),
            ActiveCodeSceneInput::StudioTitle => Some(&mut self.studio.title),
            ActiveCodeSceneInput::StudioTags => Some(&mut self.studio.tags_input),
        }
    }

    pub fn set_ime_preedit(&mut self, preedit: Option<String>, cx: &mut Context<Self>) {
        self.ime_preedit = preedit.filter(|s| !s.is_empty());
        cx.notify();
    }

    pub fn insert_text_input(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.active_input == ActiveCodeSceneInput::None {
            return false;
        }
        let target = self.active_input;
        if let Some(buf) = self.active_input_buf_mut() {
            buf.push_str(text);
        }
        if target == ActiveCodeSceneInput::Search {
            let q = self.search_query.trim().to_string();
            self.service
                .set_query(if q.is_empty() { None } else { Some(q) });
            self.load_current_card_into_viewport(cx);
            self.prewarm_neighbor_cards();
        }
        cx.notify();
        true
    }

    pub fn clear_active_input(&mut self, cx: &mut Context<Self>) -> bool {
        let target = self.active_input;
        if let Some(buf) = self.active_input_buf_mut() {
            buf.clear();
            self.ime_preedit = None;
            if target == ActiveCodeSceneInput::Search {
                self.service.set_query(None);
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
            }
            cx.notify();
            return true;
        }
        false
    }

    /// Handles non-character control keys (Backspace, Enter, Escape, arrows, shortcuts, and game keys).
    pub fn handle_non_char_key_input(
        &mut self,
        key: &str,
        shift: bool,
        alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let key_lower = key.to_ascii_lowercase();

        // 1. Focused text input mode (Search, Comment, Code Editor, AI Studio)
        if self.has_focused_text_input() {
            match key_lower.as_str() {
                "escape" => {
                    self.active_input = ActiveCodeSceneInput::None;
                    self.ime_preedit = None;
                    cx.notify();
                    return true;
                }
                "backspace" => {
                    let target = self.active_input;
                    if let Some(buf) = self.active_input_buf_mut() {
                        if alt {
                            while buf.ends_with(char::is_whitespace) {
                                buf.pop();
                            }
                            while !buf.is_empty() && !buf.ends_with(char::is_whitespace) {
                                buf.pop();
                            }
                        } else {
                            buf.pop();
                        }
                    }
                    if target == ActiveCodeSceneInput::Search {
                        let q = self.search_query.trim().to_string();
                        self.service
                            .set_query(if q.is_empty() { None } else { Some(q) });
                        self.load_current_card_into_viewport(cx);
                        self.prewarm_neighbor_cards();
                    }
                    cx.notify();
                    return true;
                }
                "enter" | "return" => {
                    match self.active_input {
                        ActiveCodeSceneInput::CodeEditor => {
                            self.editor_source.push('\n');
                            cx.notify();
                        }
                        ActiveCodeSceneInput::Comment => {
                            self.submit_comment(cx);
                        }
                        ActiveCodeSceneInput::Search => {
                            let q = self.search_query.trim().to_string();
                            self.service
                                .set_query(if q.is_empty() { None } else { Some(q) });
                            self.load_current_card_into_viewport(cx);
                            self.prewarm_neighbor_cards();
                            self.active_input = ActiveCodeSceneInput::None;
                            cx.notify();
                        }
                        ActiveCodeSceneInput::StudioPrompt if !shift => {
                            self.trigger_studio_generate(cx);
                        }
                        ActiveCodeSceneInput::StudioPrompt => {
                            self.studio.prompt.push('\n');
                            cx.notify();
                        }
                        ActiveCodeSceneInput::StudioIterate => {
                            self.trigger_studio_iterate(cx);
                        }
                        ActiveCodeSceneInput::StudioTitle | ActiveCodeSceneInput::StudioTags => {
                            self.active_input = ActiveCodeSceneInput::None;
                            cx.notify();
                        }
                        ActiveCodeSceneInput::None => {}
                    }
                    return true;
                }
                "tab" => {
                    if self.active_input == ActiveCodeSceneInput::CodeEditor {
                        self.editor_source.push_str("  ");
                        cx.notify();
                        return true;
                    }
                    return false;
                }
                _ => return false,
            }
        }

        // 2. Active Playable Game input capture mode
        if self.is_game_input_captured() {
            match key_lower.as_str() {
                "escape" => {
                    self.viewport.set_game_input_captured(false);
                    cx.notify();
                    return true;
                }
                "r" => {
                    let _ = self.viewport.restart(cx);
                    cx.notify();
                    return true;
                }
                "pageup" => {
                    self.swipe_card(SwipeSnapDirection::Prev, cx);
                    return true;
                }
                "pagedown" => {
                    self.swipe_card(SwipeSnapDirection::Next, cx);
                    return true;
                }
                _ => {
                    if self.viewport.set_game_key(&key_lower, true) {
                        cx.notify();
                        return true;
                    }
                }
            }
        }

        // 3. Layered Escape unwind: More Menu -> Score Breakdown -> Search -> Drawer -> Exit to Chats
        if key_lower == "escape" {
            if self.show_more_menu {
                self.show_more_menu = false;
                cx.notify();
                return true;
            }
            if self.show_score_breakdown {
                self.show_score_breakdown = false;
                cx.notify();
                return true;
            }
            if self.search_open {
                self.search_open = false;
                self.active_input = ActiveCodeSceneInput::None;
                cx.notify();
                return true;
            }
            if self.active_drawer != CodeSceneDrawer::None {
                self.close_drawer(cx);
                return true;
            }
            self.pending_action = Some(CodeScenePendingAction::ExitToChats);
            cx.notify();
            return true;
        }

        // 4. Feed browsing shortcuts (`Up/Down/J/K/Space/L/C/M/R/F`)
        match key_lower.as_str() {
            "down" | "arrowdown" | "j" | "pagedown" => {
                self.swipe_card(SwipeSnapDirection::Next, cx);
                true
            }
            "up" | "arrowup" | "k" | "pageup" => {
                self.swipe_card(SwipeSnapDirection::Prev, cx);
                true
            }
            "space" | " " => {
                self.viewport.toggle_pause();
                cx.notify();
                true
            }
            "l" => {
                self.toggle_like_active(false, cx);
                true
            }
            "c" => {
                self.open_comments_drawer(cx);
                true
            }
            "m" => {
                self.viewport.audio_muted = !self.viewport.audio_muted;
                cx.notify();
                true
            }
            "r" => {
                let _ = self.viewport.restart(cx);
                cx.notify();
                true
            }
            "f" => {
                self.pending_action = Some(CodeScenePendingAction::ToggleSidebarCollapse);
                cx.notify();
                true
            }
            _ => false,
        }
    }

    pub fn handle_key_up(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.viewport.set_game_key(key, false) {
            cx.notify();
        }
    }

    /// Deep-links directly to a specific `scene_id` (used by chat `GenerativeCard` `code_scene:open:<id>`).
    pub fn open_scene_by_id(&mut self, scene_id: &str, cx: &mut Context<Self>) {
        self.active_drawer = CodeSceneDrawer::None;
        self.active_input = ActiveCodeSceneInput::None;
        self.show_more_menu = false;
        self.service.ensure_feed_loaded();
        if self.service.open_scene_by_id(scene_id).is_some() {
            self.load_current_card_into_viewport(cx);
            self.prewarm_neighbor_cards();
        } else {
            let msg = format!(
                "{}: {scene_id}",
                tr_str(&self.language_id, "未找到对应代码场景", "Code scene not found")
            );
            self.pending_action = Some(CodeScenePendingAction::NotifyToast(msg));
        }
        cx.notify();
    }

    /// Opens the AI Bot Creation & Remix Studio pre-seeded with `scene_id` (`code_scene:remix:<id>`).
    pub fn open_remix_studio_for_scene(&mut self, scene_id: &str, cx: &mut Context<Self>) {
        self.service.ensure_feed_loaded();
        if let Some(asset) = self.service.get_asset(scene_id) {
            self.service.open_scene_by_id(scene_id);
            self.load_current_card_into_viewport(cx);
            self.studio.selected_kind = asset.kind;
            self.studio.remix_parent_id = Some(asset.scene_id.clone());
            self.studio.remix_parent_title = Some(asset.title.clone());
            self.studio.title = format!("{} Remix", asset.title);
            self.studio.tags_input = asset.tags.join(", ");
            self.studio.current_draft = None;
            self.studio.status_message = None;
            if let Some(bid) = asset.creator_bot_id {
                self.studio.selected_bot_id = bid;
            }
            if let Some(bun) = asset.creator_bot_username {
                self.studio.selected_bot_username = bun;
            }
            self.active_drawer = CodeSceneDrawer::AiStudio;
            self.active_input = ActiveCodeSceneInput::StudioPrompt;
            self.viewport.set_game_input_captured(false);
            self.service
                .record_interaction(SceneBehaviorEventKind::Remix, Some(asset.scene_id));
        } else {
            self.active_drawer = CodeSceneDrawer::AiStudio;
            self.active_input = ActiveCodeSceneInput::StudioPrompt;
        }
        cx.notify();
    }

    /// Opens Share-to-Chat picker focused on `scene_id` (`code_scene:share:<id>`).
    pub fn open_share_drawer_for_scene(&mut self, scene_id: &str, cx: &mut Context<Self>) {
        self.service.ensure_feed_loaded();
        let _ = self.service.open_scene_by_id(scene_id);
        self.load_current_card_into_viewport(cx);
        self.active_drawer = CodeSceneDrawer::ShareToChat;
        self.active_input = ActiveCodeSceneInput::None;
        self.viewport.set_game_input_captured(false);
        cx.notify();
    }

    /// Runs non-blocking server synchronization (flushes telemetry outbox, pushes local drafts/scenes, pulls fresh catalog).
    pub fn trigger_background_sync(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.session_token.clone().filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let Some(handle) = self.tokio_handle.clone() else {
            return;
        };
        let service = Arc::clone(&self.service);
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let sync_res = handle
                .spawn(async move { service.sync_with_server(&token).await })
                .await;
            if let Ok(Ok(report)) = sync_res {
                if report.pulled_assets > 0 || report.reconciled_social > 0 {
                    let _ = weak.update(app, |ws, wcx| {
                        ws.prewarm_neighbor_cards();
                        wcx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_current_card_into_viewport(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.service.current_item() else {
            self.viewport.unload(cx);
            return;
        };
        let prewarmed = self.prewarm_cache.get(&item.asset.scene_id).cloned();
        if let Err(err) = self.viewport.load_asset(&item.asset, prewarmed, cx) {
            self.viewport.last_error = Some(err);
        }
        self.editor_source = item.asset.source_code.clone();
        self.editor_validation = None;
        self.show_more_menu = false;
        self.show_score_breakdown = false;
    }

    fn prewarm_neighbor_cards(&mut self) {
        let window_items = self.service.preload_window_items();
        let keep_ids: Vec<String> = window_items
            .iter()
            .map(|(_, it)| it.asset.scene_id.clone())
            .collect();
        self.prewarm_cache.retain(|k, _| keep_ids.contains(k));
        for (_, item) in window_items {
            if !self.prewarm_cache.contains_key(&item.asset.scene_id) {
                if let Ok(verified) =
                    compile_and_verify(&item.asset.source_code, &item.asset.manifest)
                {
                    self.prewarm_cache
                        .insert(item.asset.scene_id.clone(), verified);
                }
            }
        }
    }

    fn viewport_height_px(&self) -> f32 {
        self.viewport
            .viewport_bounds
            .lock()
            .map(|b| f32::from(b.size.height))
            .filter(|h| h.is_finite() && *h > 100.0)
            .unwrap_or(720.0)
    }

    pub fn swipe_card(&mut self, dir: SwipeSnapDirection, cx: &mut Context<Self>) {
        let prev_id = self.viewport.active_scene_id.clone();
        let switched = match dir {
            SwipeSnapDirection::Next => self.service.swipe_next(),
            SwipeSnapDirection::Prev => self.service.swipe_prev(),
        };
        if let Some(next_item) = switched {
            let changed = prev_id.as_deref() != Some(next_item.asset.scene_id.as_str());
            if changed {
                let vh = self.viewport_height_px();
                self.gesture.trigger_card_transition(dir, vh);
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
                if self.active_drawer == CodeSceneDrawer::Comments {
                    self.refresh_comments_for_active(cx);
                }
            }
        }
        cx.notify();
    }

    fn toggle_like_active(&mut self, is_double_tap: bool, cx: &mut Context<Self>) {
        let Some(item) = self.service.current_item() else {
            return;
        };
        let scene_id = item.asset.scene_id.clone();
        if is_double_tap {
            let _ = self
                .service
                .record_interaction(SceneBehaviorEventKind::DoubleTapLike, None);
            if item.asset.viewer_liked {
                cx.notify();
                return;
            }
        }
        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            let sid = scene_id.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let _ = handle
                    .spawn(async move { service.toggle_like(&sid, Some(&token)).await })
                    .await;
                let _ = weak.update(app, |ws, wcx| {
                    ws.prewarm_neighbor_cards();
                    wcx.notify();
                });
            })
            .detach();
        } else {
            self.service.toggle_like_local(&scene_id);
        }
        cx.notify();
    }

    fn toggle_bookmark_active(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.service.current_item() else {
            return;
        };
        let scene_id = item.asset.scene_id.clone();
        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            let sid = scene_id.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let _ = handle
                    .spawn(async move { service.toggle_bookmark(&sid, Some(&token)).await })
                    .await;
                let _ = weak.update(app, |_, wcx| wcx.notify());
            })
            .detach();
        } else {
            self.service.toggle_bookmark_local(&scene_id);
        }
        cx.notify();
    }

    fn toggle_follow_author(&mut self, author_user_id: i64, cx: &mut Context<Self>) {
        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let _ = handle
                    .spawn(async move { service.toggle_follow(author_user_id, Some(&token)).await })
                    .await;
                let _ = weak.update(app, |_, wcx| wcx.notify());
            })
            .detach();
        } else {
            self.service.toggle_follow_local(author_user_id);
        }
        cx.notify();
    }

    fn open_comments_drawer(&mut self, cx: &mut Context<Self>) {
        self.active_drawer = CodeSceneDrawer::Comments;
        self.active_input = ActiveCodeSceneInput::Comment;
        self.viewport.set_game_input_captured(false);
        self.refresh_comments_for_active(cx);
        cx.notify();
    }

    fn refresh_comments_for_active(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.service.current_item() else {
            self.comments.clear();
            return;
        };
        let sid = item.asset.scene_id.clone();
        let sort = self.comment_sort;
        self.comments = self.service.list_comments_local(&sid, sort).comments;

        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            let query = ListCodeSceneCommentsQuery {
                sort: Some(sort),
                limit: Some(50),
                offset: Some(0),
            };
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = handle
                    .spawn(async move { service.fetch_comments(&sid, &query, Some(&token)).await })
                    .await;
                if let Ok(Ok(resp)) = res {
                    let _ = weak.update(app, |ws, wcx| {
                        ws.comments = resp.comments;
                        wcx.notify();
                    });
                }
            })
            .detach();
        }
    }

    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let content = self.comment_draft.trim().to_string();
        if content.is_empty() {
            return;
        }
        let Some(item) = self.service.current_item() else {
            return;
        };
        let sid = item.asset.scene_id.clone();
        let parent_comment_id = self.reply_target.take().map(|(id, _)| id);
        self.comment_draft.clear();
        self.ime_preedit = None;
        let _ = self
            .service
            .record_interaction(SceneBehaviorEventKind::Comment, None);

        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            let req = CreateCodeSceneCommentRequest {
                content: content.clone(),
                parent_comment_id: parent_comment_id.clone(),
            };
            let sort = self.comment_sort;
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = handle
                    .spawn(async move {
                        let _ = service.add_comment(&sid, &req, Some(&token)).await;
                        service.list_comments_local(&sid, sort)
                    })
                    .await;
                if let Ok(list) = res {
                    let _ = weak.update(app, |ws, wcx| {
                        ws.comments = list.comments;
                        wcx.notify();
                    });
                }
            })
            .detach();
        } else {
            let _ = self
                .service
                .add_comment_local(&sid, &content, parent_comment_id);
            self.comments = self.service.list_comments_local(&sid, self.comment_sort).comments;
        }
        cx.notify();
    }

    fn close_drawer(&mut self, cx: &mut Context<Self>) {
        self.active_drawer = CodeSceneDrawer::None;
        self.active_input = ActiveCodeSceneInput::None;
        self.ime_preedit = None;
        self.studio_preview_viewport.set_paused(true);
        cx.notify();
    }

    fn trigger_studio_generate(&mut self, cx: &mut Context<Self>) {
        if self.studio.is_busy {
            return;
        }
        let prompt = self.studio.prompt.trim().to_string();
        if prompt.is_empty() {
            self.studio.status_message = Some(
                tr_str(&self.language_id, "请先输入你想创作的场景描述", "Please enter a prompt first")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        self.studio.is_busy = true;
        self.studio.status_message = Some(
            tr_str(&self.language_id, "AI Bot 正在编写并预演验证代码…", "AI Bot is writing & dry-running code…")
                .to_string(),
        );
        cx.notify();

        let bot_id = Some(self.studio.selected_bot_id);
        let bot_username = Some(self.studio.selected_bot_username.clone());
        let kind = self.studio.selected_kind;
        let remix_parent = self.studio.remix_parent_id.clone();

        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = handle
                    .spawn(async move {
                        if let Some(pid) = remix_parent {
                            let req = RemixSceneRequest {
                                parent_scene_id: pid,
                                prompt,
                                bot_id,
                                bot_username,
                                auto_publish: false,
                                chat_id: None,
                            };
                            service.ai_remix(&req, Some(&token)).await.map(|r| r.draft)
                        } else {
                            let req = AiGenerateSceneRequest {
                                prompt,
                                kind: Some(kind),
                                bot_id,
                                bot_username,
                                style_tags: None,
                                seed: None,
                            };
                            service.ai_generate(&req, Some(&token)).await
                        }
                    })
                    .await;
                let _ = weak.update(app, |ws, wcx| {
                    ws.studio.is_busy = false;
                    match res {
                        Ok(Ok(draft)) => ws.apply_studio_draft(draft, wcx),
                        Ok(Err(e)) => {
                            ws.studio.status_message = Some(format!("Error: {e}"));
                        }
                        Err(e) => {
                            ws.studio.status_message = Some(format!("Task error: {e}"));
                        }
                    }
                    wcx.notify();
                });
            })
            .detach();
        } else {
            let draft_res = if let Some(pid) = remix_parent {
                let req = RemixSceneRequest {
                    parent_scene_id: pid,
                    prompt,
                    bot_id,
                    bot_username,
                    auto_publish: false,
                    chat_id: None,
                };
                self.service
                    .ai_remix_local(&req)
                    .map(|r| r.draft)
                    .map_err(|e| e.to_string())
            } else {
                let req = AiGenerateSceneRequest {
                    prompt,
                    kind: Some(kind),
                    bot_id,
                    bot_username,
                    style_tags: None,
                    seed: None,
                };
                Ok(self.service.ai_generate_local(&req).draft)
            };
            self.studio.is_busy = false;
            match draft_res {
                Ok(draft) => self.apply_studio_draft(draft, cx),
                Err(err) => self.studio.status_message = Some(err),
            }
            cx.notify();
        }
    }

    fn trigger_studio_iterate(&mut self, cx: &mut Context<Self>) {
        if self.studio.is_busy {
            return;
        }
        let instruction = self.studio.iterate_instruction.trim().to_string();
        let Some(cur_draft) = self.studio.current_draft.clone() else {
            return;
        };
        if instruction.is_empty() {
            return;
        }
        self.studio.is_busy = true;
        self.studio.iterate_instruction.clear();
        cx.notify();

        let req = AiIterateSceneRequest {
            draft_id: Some(cur_draft.draft_id.clone()),
            source_code: Some(cur_draft.source_code.clone()),
            manifest: Some(cur_draft.manifest.clone()),
            instruction,
            bot_id: Some(self.studio.selected_bot_id),
            bot_username: Some(self.studio.selected_bot_username.clone()),
        };

        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = handle
                    .spawn(async move { service.ai_iterate(&req, Some(&token)).await })
                    .await;
                let _ = weak.update(app, |ws, wcx| {
                    ws.studio.is_busy = false;
                    match res {
                        Ok(Ok(draft)) => ws.apply_studio_draft(draft, wcx),
                        Ok(Err(e)) => ws.studio.status_message = Some(format!("Error: {e}")),
                        Err(e) => ws.studio.status_message = Some(format!("Task error: {e}")),
                    }
                    wcx.notify();
                });
            })
            .detach();
        } else {
            self.studio.is_busy = false;
            match self.service.ai_iterate_local(&req) {
                Ok(rec) => self.apply_studio_draft(rec.draft, cx),
                Err(err) => self.studio.status_message = Some(err.to_string()),
            }
            cx.notify();
        }
    }

    fn apply_studio_draft(
        &mut self,
        draft: gpui_telegram_proto::code_scene_contracts::AiGenerateSceneResponse,
        cx: &mut Context<Self>,
    ) {
        self.studio.title = draft.title.clone();
        self.studio.tags_input = draft.tags.join(", ");
        self.studio.selected_kind = draft.kind;
        let preview_asset = CodeSceneAssetDto {
            scene_id: draft.draft_id.clone(),
            title: draft.title.clone(),
            description: draft.description.clone(),
            kind: draft.kind,
            tags: draft.tags.clone(),
            source_code: draft.source_code.clone(),
            code_format: draft.code_format.clone(),
            manifest: draft.manifest.clone(),
            controls_hint: draft.controls_hint.clone(),
            ..CodeSceneAssetDto::default()
        };
        let _ = self
            .studio_preview_viewport
            .load_asset(&preview_asset, None, cx);
        self.studio.status_message = Some(
            tr_str(
                &self.language_id,
                "已通过 5 帧沙箱预演校验，可直接迭代或一键发布！",
                "Passed 5-frame sandbox dry-run! Ready to iterate or publish.",
            )
            .to_string(),
        );
        self.studio.current_draft = Some(draft);
    }

    fn trigger_studio_publish(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.studio.current_draft.clone() else {
            return;
        };
        let title = if self.studio.title.trim().is_empty() {
            draft.title.clone()
        } else {
            self.studio.title.trim().to_string()
        };
        let tags: Vec<String> = self
            .studio
            .tags_input
            .split(|c| c == ',' || c == '，' || c == ' ')
            .map(|s| s.trim().trim_start_matches('#').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let req = AiPublishSceneRequest {
            draft_id: Some(draft.draft_id.clone()),
            title: Some(title),
            description: Some(draft.description.clone()),
            tags: if tags.is_empty() {
                Some(draft.tags.clone())
            } else {
                Some(tags)
            },
            source_code: Some(draft.source_code.clone()),
            manifest: Some(draft.manifest.clone()),
            controls_hint: Some(draft.controls_hint.clone()),
            bot_id: Some(self.studio.selected_bot_id),
            bot_username: Some(self.studio.selected_bot_username.clone()),
            remix_parent_id: self.studio.remix_parent_id.clone(),
            chat_id: None,
        };
        let share_after = self.studio.share_to_active_chat;
        self.studio.is_busy = true;
        cx.notify();

        if let (Some(token), Some(handle)) = (self.session_token.clone(), self.tokio_handle.clone()) {
            let service = Arc::clone(&self.service);
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = handle
                    .spawn(async move { service.ai_publish(&req, Some(&token)).await })
                    .await;
                let _ = weak.update(app, |ws, wcx| {
                    ws.studio.is_busy = false;
                    match res {
                        Ok(Ok(pub_resp)) => ws.finish_studio_publish(pub_resp.asset, share_after, wcx),
                        Ok(Err(e)) => ws.studio.status_message = Some(format!("Publish error: {e}")),
                        Err(e) => ws.studio.status_message = Some(format!("Task error: {e}")),
                    }
                    wcx.notify();
                });
            })
            .detach();
        } else {
            let pub_req = PublishCodeSceneRequest {
                scene_id: None,
                title: req.title.unwrap_or_else(|| draft.title.clone()),
                description: req.description.unwrap_or_else(|| draft.description.clone()),
                kind: draft.kind,
                tags: req.tags.unwrap_or_else(|| draft.tags.clone()),
                source_code: draft.source_code,
                code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                manifest: draft.manifest,
                controls_hint: draft.controls_hint,
                creator_bot_id: req.bot_id,
                creator_bot_username: req.bot_username,
                remix_parent_id: req.remix_parent_id,
                share_to_chat_id: None,
            };
            self.studio.is_busy = false;
            match self.service.publish_scene_local(&pub_req) {
                Ok(resp) => self.finish_studio_publish(resp.asset, share_after, cx),
                Err(err) => self.studio.status_message = Some(err.to_string()),
            }
            cx.notify();
        }
    }

    fn finish_studio_publish(
        &mut self,
        asset: CodeSceneAssetDto,
        share_after: bool,
        cx: &mut Context<Self>,
    ) {
        let scene_id = asset.scene_id.clone();
        self.studio.current_draft = None;
        self.studio.remix_parent_id = None;
        self.studio.remix_parent_title = None;
        self.studio.prompt.clear();
        self.studio.iterate_instruction.clear();
        self.studio.status_message = None;
        self.close_drawer(cx);

        self.service.switch_tab(CodeSceneFeedTab::ForYou);
        let _ = self.service.open_scene_by_id(&scene_id);
        self.load_current_card_into_viewport(cx);
        self.prewarm_neighbor_cards();

        if share_after {
            self.pending_action = Some(CodeScenePendingAction::SharePublishedToActiveChat { asset });
        } else {
            let msg = format!(
                "{}「{}」",
                tr_str(&self.language_id, "已发布到代码场景：", "Published to Code Scene: "),
                asset.title
            );
            self.pending_action = Some(CodeScenePendingAction::NotifyToast(msg));
        }
    }

    pub fn handle_hud_action(&mut self, action: HudAction, cx: &mut Context<Self>) {
        match action {
            HudAction::SelectTab(tab) => {
                self.service.switch_tab(tab);
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
            }
            HudAction::ToggleSearch => {
                self.search_open = !self.search_open;
                if self.search_open {
                    self.active_input = ActiveCodeSceneInput::Search;
                    self.viewport.set_game_input_captured(false);
                } else {
                    self.active_input = ActiveCodeSceneInput::None;
                }
            }
            HudAction::FocusSearch => {
                self.search_open = true;
                self.active_input = ActiveCodeSceneInput::Search;
                self.viewport.set_game_input_captured(false);
            }
            HudAction::ClearSearch => {
                self.search_query.clear();
                self.ime_preedit = None;
                self.service.set_query(None);
                self.search_open = false;
                self.active_input = ActiveCodeSceneInput::None;
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
            }
            HudAction::ClearTagFilter => {
                self.service.set_tag_filter(None);
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
            }
            HudAction::SelectTagFilter(tag) => {
                self.service.set_tag_filter(Some(tag));
                self.load_current_card_into_viewport(cx);
                self.prewarm_neighbor_cards();
            }
            HudAction::OpenAiStudio => {
                self.studio.remix_parent_id = None;
                self.studio.remix_parent_title = None;
                self.active_drawer = CodeSceneDrawer::AiStudio;
                self.active_input = ActiveCodeSceneInput::StudioPrompt;
                self.viewport.set_game_input_captured(false);
            }
            HudAction::ToggleImmersive => {
                self.pending_action = Some(CodeScenePendingAction::ToggleSidebarCollapse);
            }
            HudAction::PrevCard => {
                self.swipe_card(SwipeSnapDirection::Prev, cx);
                return;
            }
            HudAction::NextCard => {
                self.swipe_card(SwipeSnapDirection::Next, cx);
                return;
            }
            HudAction::ToggleLike => {
                self.toggle_like_active(false, cx);
                return;
            }
            HudAction::ToggleBookmark => {
                self.toggle_bookmark_active(cx);
                return;
            }
            HudAction::ToggleFollow => {
                if let Some(item) = self.service.current_item() {
                    self.toggle_follow_author(item.asset.author_user_id, cx);
                    return;
                }
            }
            HudAction::OpenComments => {
                self.open_comments_drawer(cx);
                return;
            }
            HudAction::OpenCodeInspector => {
                if let Some(item) = self.service.current_item() {
                    self.editor_source = item.asset.source_code.clone();
                    self.editor_validation =
                        Some(self.service.validate_source(&item.asset.source_code, &item.asset.manifest));
                    self.active_drawer = CodeSceneDrawer::CodeInspector;
                    self.active_input = ActiveCodeSceneInput::None;
                    self.viewport.set_game_input_captured(false);
                    let _ = self
                        .service
                        .record_interaction(SceneBehaviorEventKind::InspectCode, None);
                }
            }
            HudAction::OpenCreatorProfile => {
                if let Some(item) = self.service.current_item() {
                    self.active_drawer = CodeSceneDrawer::CreatorProfile {
                        author_user_id: item.asset.author_user_id,
                        author_username: item.asset.author_username.clone(),
                    };
                    self.active_input = ActiveCodeSceneInput::None;
                    self.viewport.set_game_input_captured(false);
                }
            }
            HudAction::OpenRemixStudio => {
                if let Some(item) = self.service.current_item() {
                    let sid = item.asset.scene_id.clone();
                    self.open_remix_studio_for_scene(&sid, cx);
                    return;
                }
            }
            HudAction::OpenShareToChat => {
                self.active_drawer = CodeSceneDrawer::ShareToChat;
                self.active_input = ActiveCodeSceneInput::None;
                self.viewport.set_game_input_captured(false);
            }
            HudAction::ToggleMute => {
                self.viewport.audio_muted = !self.viewport.audio_muted;
            }
            HudAction::ToggleMoreMenu => {
                self.show_more_menu = !self.show_more_menu;
            }
            HudAction::NotInterested => {
                self.show_more_menu = false;
                let _ = self.service.record_interaction(
                    SceneBehaviorEventKind::NotInterested,
                    Some("user_not_interested".to_string()),
                );
                self.swipe_card(SwipeSnapDirection::Next, cx);
                return;
            }
            HudAction::HideAuthor => {
                self.show_more_menu = false;
                let author = self
                    .service
                    .current_item()
                    .map(|it| it.asset.author_user_id.to_string());
                let _ = self
                    .service
                    .record_interaction(SceneBehaviorEventKind::HideAuthor, author);
                self.swipe_card(SwipeSnapDirection::Next, cx);
                return;
            }
            HudAction::RestartScene => {
                self.show_more_menu = false;
                let _ = self.viewport.restart(cx);
            }
            HudAction::TogglePlayPause => {
                self.viewport.toggle_pause();
            }
            HudAction::CycleSpeed => {
                self.viewport.cycle_playback_speed();
            }
            HudAction::SeekVideoRatio(ratio) => {
                let dur = self
                    .viewport
                    .timeline_progress()
                    .1
                    .or(self.viewport.manifest.duration_sec)
                    .unwrap_or(15.0);
                let target = (ratio.clamp(0.0, 1.0)) * dur;
                let _ = self.viewport.seek_video(target, cx);
            }
            HudAction::ToggleScoreBreakdown => {
                self.show_score_breakdown = !self.show_score_breakdown;
            }
            HudAction::CaptureGameFocus => {
                self.active_input = ActiveCodeSceneInput::None;
                self.viewport.set_paused(false);
                self.viewport.set_game_input_captured(true);
            }
            HudAction::ReleaseGameFocus => {
                self.viewport.set_game_input_captured(false);
            }
        }
        cx.notify();
    }

    pub fn handle_drawer_action(&mut self, action: DrawerAction, cx: &mut Context<Self>) {
        match action {
            DrawerAction::Close => {
                self.close_drawer(cx);
                return;
            }
            DrawerAction::SetCommentSort(sort) => {
                self.comment_sort = sort;
                self.refresh_comments_for_active(cx);
            }
            DrawerAction::FocusInput(inp) => {
                self.active_input = inp;
                self.ime_preedit = None;
                self.viewport.set_game_input_captured(false);
            }
            DrawerAction::SubmitComment => {
                self.submit_comment(cx);
                return;
            }
            DrawerAction::SetReplyTarget(target) => {
                self.reply_target = target;
                self.active_input = ActiveCodeSceneInput::Comment;
            }
            DrawerAction::ValidateEditorCode => {
                if let Some(item) = self.service.current_item() {
                    let rep = self
                        .service
                        .validate_source(&self.editor_source, &item.asset.manifest);
                    self.editor_validation = Some(rep);
                }
            }
            DrawerAction::HotReloadEditorCode => {
                if let Some(item) = self.service.current_item() {
                    let rep = self
                        .service
                        .validate_source(&self.editor_source, &item.asset.manifest);
                    let ok = rep.ok;
                    self.editor_validation = Some(rep);
                    if ok {
                        let _ = self.viewport.hot_reload_source(
                            &item.asset.scene_id,
                            &self.editor_source,
                            &item.asset.manifest,
                            cx,
                        );
                    }
                }
            }
            DrawerAction::CopyEditorCode => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.editor_source.clone()));
                self.pending_action = Some(CodeScenePendingAction::NotifyToast(
                    tr_str(&self.language_id, "已复制源码到剪贴板", "Copied source code").to_string(),
                ));
            }
            DrawerAction::PublishEditorRemix => {
                if let Some(item) = self.service.current_item() {
                    let req = PublishCodeSceneRequest {
                        scene_id: None,
                        title: format!("{} (Edit)", item.asset.title),
                        description: item.asset.description.clone(),
                        kind: item.asset.kind,
                        tags: item.asset.tags.clone(),
                        source_code: self.editor_source.clone(),
                        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                        manifest: item.asset.manifest.clone(),
                        controls_hint: item.asset.controls_hint.clone(),
                        creator_bot_id: item.asset.creator_bot_id,
                        creator_bot_username: item.asset.creator_bot_username.clone(),
                        remix_parent_id: Some(item.asset.scene_id.clone()),
                        share_to_chat_id: None,
                    };
                    match self.service.publish_scene_local(&req) {
                        Ok(resp) => {
                            self.editor_validation = Some(resp.validation);
                            let sid = resp.asset.scene_id.clone();
                            self.close_drawer(cx);
                            self.service.switch_tab(CodeSceneFeedTab::ForYou);
                            let _ = self.service.open_scene_by_id(&sid);
                            self.load_current_card_into_viewport(cx);
                            self.prewarm_neighbor_cards();
                            self.pending_action = Some(CodeScenePendingAction::NotifyToast(
                                tr_str(&self.language_id, "改版作品已发布到代码场景", "Edited remix published")
                                    .to_string(),
                            ));
                            return;
                        }
                        Err(_) => {
                            self.editor_validation = Some(
                                self.service
                                    .validate_source(&self.editor_source, &item.asset.manifest),
                            );
                        }
                    }
                }
            }
            DrawerAction::OpenAiRemixFromCurrent => {
                if let Some(item) = self.service.current_item() {
                    let sid = item.asset.scene_id.clone();
                    self.open_remix_studio_for_scene(&sid, cx);
                    return;
                }
            }
            DrawerAction::ToggleFollowCreator(uid) => {
                self.toggle_follow_author(uid, cx);
                return;
            }
            DrawerAction::OpenSceneById(sid) => {
                self.open_scene_by_id(&sid, cx);
                return;
            }
            DrawerAction::StudioSelectBot(bid, bun) => {
                self.studio.selected_bot_id = bid;
                self.studio.selected_bot_username = bun;
            }
            DrawerAction::StudioSelectKind(kind) => {
                self.studio.selected_kind = kind;
            }
            DrawerAction::StudioPickPreset(text, kind) => {
                self.studio.prompt = text;
                self.studio.selected_kind = kind;
                self.trigger_studio_generate(cx);
                return;
            }
            DrawerAction::StudioGenerate => {
                self.trigger_studio_generate(cx);
                return;
            }
            DrawerAction::StudioIterate => {
                self.trigger_studio_iterate(cx);
                return;
            }
            DrawerAction::StudioClearRemixParent => {
                self.studio.remix_parent_id = None;
                self.studio.remix_parent_title = None;
            }
            DrawerAction::StudioToggleShareAfterPublish => {
                self.studio.share_to_active_chat = !self.studio.share_to_active_chat;
            }
            DrawerAction::StudioPublish => {
                self.trigger_studio_publish(cx);
                return;
            }
            DrawerAction::ShareToTargetChat(chat_id) => {
                if let Some(item) = self.service.current_item() {
                    let _ = self
                        .service
                        .record_interaction(SceneBehaviorEventKind::Share, None);
                    self.pending_action = Some(CodeScenePendingAction::ShareSceneToChat {
                        chat_id,
                        asset: item.asset,
                    });
                    self.close_drawer(cx);
                    return;
                }
            }
        }
        cx.notify();
    }
}

impl Focusable for CodeSceneWorkspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CodeSceneWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_visible = self.is_visible && !self.is_occluded;

        // 1. Advance live sandboxed VM strictly on the active visible card
        if let Some((delta_ms, frame)) = self.viewport.tick(active_visible, cx) {
            let game_tel = if self.viewport.manifest.kind == AssetKind::Game {
                self.viewport.game_telemetry()
            } else {
                None
            };
            self.service.on_frame_output(delta_ms, &frame, game_tel);
        }

        // 2. Advance Studio preview VM only when AI Studio drawer is open with a live draft
        let studio_preview_active = active_visible
            && self.active_drawer == CodeSceneDrawer::AiStudio
            && self.studio.current_draft.is_some();
        if studio_preview_active {
            let _ = self.studio_preview_viewport.tick(true, cx);
        }

        // 3. Advance vertical snap transition & prune expired double-tap heart bursts
        let (visual_offset_y, gesture_animating) = self.gesture.current_visual_offset_y();
        let now = Instant::now();
        self.heart_bursts.retain(|hb| hb.sample(now).is_some());
        let has_hearts = !self.heart_bursts.is_empty();

        let should_pump_raf = active_visible
            && ((!self.viewport.paused && self.viewport.vm.is_some())
                || gesture_animating
                || has_hearts);

        let current_item = self.service.current_item();
        let active_tab = self.service.active_tab();
        let active_index = self.service.current_index();
        let total_items = self.service.items().len();
        let tag_filter = self.service.tag_filter();
        let (timeline_sec, duration_sec, loop_count) = self.viewport.timeline_progress();

        let stage_el = if current_item.is_some() {
            self.viewport
                .render_stage_layers("cs-primary-stage", should_pump_raf, visual_offset_y)
        } else {
            let lang = self.language_id.as_str();
            div()
                .size_full()
                .bg(rgb(0x090A10))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .child(tg_icon(IconName::CodeScene, 42.0, 0x25F4EE))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .child(tr_str(lang, "当前筛选下暂无代码场景", "No code scenes match this filter")),
                )
                .child(
                    div()
                        .id("cs-empty-reset-btn")
                        .cursor_pointer()
                        .px(px(14.0))
                        .py(px(7.0))
                        .rounded(px(999.0))
                        .bg(rgba(0xFE2C55FF))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.service.set_tag_filter(None);
                                this.service.set_query(None);
                                this.service.switch_tab(CodeSceneFeedTab::ForYou);
                                this.search_query.clear();
                                this.load_current_card_into_viewport(cx);
                                this.prewarm_neighbor_cards();
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "返回推荐流", "Back to For You")),
                )
                .into_any_element()
        };

        let hud_props = HudRenderProps {
            item: current_item.as_ref(),
            active_tab,
            active_index,
            total_items,
            tag_filter: tag_filter.as_deref(),
            search_open: self.search_open,
            search_focused: self.active_input == ActiveCodeSceneInput::Search,
            search_query: &self.search_query,
            search_ime_preedit: if self.active_input == ActiveCodeSceneInput::Search {
                self.ime_preedit.as_deref()
            } else {
                None
            },
            paused: self.viewport.paused,
            playback_speed: self.viewport.playback_speed,
            timeline_sec,
            duration_sec,
            loop_count,
            measured_fps: self.viewport.measured_fps,
            audio_muted: self.viewport.audio_muted,
            audio_bars: self.viewport.audio_bars,
            game_input_captured: self.viewport.game_input_captured,
            game_hud: self.viewport.game_hud(),
            show_more_menu: self.show_more_menu,
            show_score_breakdown: self.show_score_breakdown,
            language_id: &self.language_id,
            palette: self.palette,
        };

        let top_bar = render_top_floating_bar(&hud_props, cx, |this, act, cx| {
            this.handle_hud_action(act, cx);
        });
        let right_rail = render_right_action_rail(&hud_props, cx, |this, act, cx| {
            this.handle_hud_action(act, cx);
        });
        let bottom_hud = render_bottom_metadata_and_scrubber(&hud_props, cx, |this, act, cx| {
            this.handle_hud_action(act, cx);
        });

        // Creator works & follow status when CreatorProfile drawer is open
        let (creator_works, creator_following) = match &self.active_drawer {
            CodeSceneDrawer::CreatorProfile { author_user_id, .. } => {
                let works = self
                    .service
                    .list_assets(&ListCodeSceneAssetsQuery {
                        author_user_id: Some(*author_user_id),
                        limit: Some(30),
                        ..ListCodeSceneAssetsQuery::default()
                    })
                    .assets;
                let following = self
                    .service
                    .export_active_profile()
                    .followed_creators
                    .contains(author_user_id);
                (works, following)
            }
            _ => (Vec::new(), false),
        };

        let studio_preview_el = if studio_preview_active {
            Some(
                self.studio_preview_viewport
                    .render_stage_layers("cs-studio-live-preview", true, 0.0),
            )
        } else {
            None
        };

        let active_drawer_el = render_active_drawer(
            DrawerRenderProps {
                drawer: &self.active_drawer,
                active_asset: current_item.as_ref().map(|i| &i.asset),
                active_input: self.active_input,
                ime_preedit: self.ime_preedit.as_deref(),
                comments: &self.comments,
                comment_sort: self.comment_sort,
                comment_draft: &self.comment_draft,
                reply_target: self.reply_target.as_ref(),
                editor_source: &self.editor_source,
                editor_validation: self.editor_validation.as_ref(),
                creator_works: &creator_works,
                creator_following,
                studio: &self.studio,
                studio_preview_el,
                share_targets: &self.share_targets,
                language_id: &self.language_id,
            },
            cx,
            |this, act, cx| {
                this.handle_drawer_action(act, cx);
            },
        );

        // Double-tap floating heart burst elements
        let heart_els: Vec<AnyElement> = self
            .heart_bursts
            .iter()
            .filter_map(|hb| {
                let (size_px, top_y, opacity) = hb.sample(now)?;
                let half = f32::from(size_px) * 0.5;
                Some(
                    div()
                        .id(("cs-heart-burst", hb.id as usize))
                        .absolute()
                        .left(px(hb.x - half))
                        .top(px(top_y - half))
                        .opacity(opacity)
                        .child(tg_icon(IconName::HeartReaction, f32::from(size_px), 0xFE2C55))
                        .into_any_element(),
                )
            })
            .collect();

        // Center pause indicator badge (when paused on Video/Animation)
        let show_center_pause = self.viewport.paused
            && current_item.is_some()
            && self.active_drawer == CodeSceneDrawer::None;

        div()
            .id("code-scene-workspace-root")
            .relative()
            .flex_1()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .bg(rgb(0x07070B))
            .track_focus(&self.focus_handle)
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                if this.active_drawer != CodeSceneDrawer::None || this.is_game_input_captured() {
                    return;
                }
                let vh = this.viewport_height_px();
                if let Some(dir) = this.gesture.on_scroll_wheel(ev.delta, vh) {
                    cx.stop_propagation();
                    this.swipe_card(dir, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.show_more_menu = false;
                    if this.active_input != ActiveCodeSceneInput::None {
                        this.active_input = ActiveCodeSceneInput::None;
                        this.ime_preedit = None;
                    }
                    // Double-click -> Douyin Heart Burst + Like
                    if ev.click_count >= 2 && !this.is_game_input_captured() {
                        let bounds_origin = this
                            .viewport
                            .viewport_bounds
                            .lock()
                            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)))
                            .unwrap_or((0.0, 0.0));
                        let lx = f32::from(ev.position.x) - bounds_origin.0;
                        let ly = f32::from(ev.position.y) - bounds_origin.1;
                        let id = this.next_heart_id;
                        this.next_heart_id = this.next_heart_id.wrapping_add(1);
                        this.heart_bursts.push(HeartBurst::new(id, lx, ly));
                        this.toggle_like_active(true, cx);
                        return;
                    }
                    // Update pointer inside VM (Animations react to pointer; Games capture focus & click)
                    this.viewport
                        .update_pointer_from_window(ev.position, Some(true), true);
                    if this.viewport.manifest.kind == AssetKind::Game
                        && this.active_drawer == CodeSceneDrawer::None
                    {
                        this.viewport.set_paused(false);
                        this.viewport.set_game_input_captured(true);
                        cx.notify();
                        return;
                    }
                    if this.active_drawer == CodeSceneDrawer::None {
                        this.gesture.on_drag_start(f32::from(ev.position.y));
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.viewport.update_pointer_from_window(
                    ev.position,
                    Some(ev.pressed_button == Some(MouseButton::Left)),
                    false,
                );
                if this.gesture.dragging {
                    this.gesture.on_drag_move(f32::from(ev.position.y));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    this.viewport
                        .update_pointer_from_window(ev.position, Some(false), false);
                    if this.gesture.dragging {
                        let travel = this.gesture.drag_travel_px();
                        let vh = this.viewport_height_px();
                        if let Some(dir) = this.gesture.on_drag_end(vh) {
                            this.swipe_card(dir, cx);
                        } else if travel < 6.0 && this.viewport.manifest.kind != AssetKind::Game {
                            // Single tap on Video/Animation toggles Play/Pause just like Douyin
                            this.viewport.toggle_pause();
                            cx.notify();
                        } else {
                            cx.notify();
                        }
                    }
                }),
            )
            .child(stage_el)
            .when(show_center_pause, |el| {
                el.child(
                    div()
                        .id("cs-center-pause-indicator")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(68.0))
                                .h(px(68.0))
                                .rounded(px(999.0))
                                .bg(rgba(0x0B0B0FAA))
                                .border_1()
                                .border_color(rgba(0xFFFFFF33))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(tg_icon(IconName::Play, 30.0, 0xFFFFFF)),
                        ),
                )
            })
            .children(heart_els)
            .child(top_bar)
            .child(right_rail)
            .child(bottom_hud)
            .when_some(active_drawer_el, |el, drawer| el.child(drawer))
            .when_some(self.viewport.last_error.clone(), |el, err| {
                el.child(
                    div()
                        .absolute()
                        .top(px(64.0))
                        .left(px(18.0))
                        .max_w(px(420.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(rgba(0xEF4444DD))
                        .text_size(px(12.0))
                        .text_color(rgba(0xFFFFFFFF))
                        .child(format!(
                            "{}: {err}",
                            tr_str(&self.language_id, "沙箱运行保护触发", "Sandbox guard triggered")
                        )),
                )
            })
    }
}
