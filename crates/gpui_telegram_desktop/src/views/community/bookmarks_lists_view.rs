//! Feature F20-Bookmarks & Lists — Saved Bookmarks with Custom Folders + X Lists &
//! Reddit Custom Feeds (CRUD, Human/Bot & `c/` Membership Editor, Pin-to-Nav, Live List Timeline).

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, BookmarkRequest, CommunityActorDto, CommunityListDto, CommunityListKind,
    CreateCommunityListRequest, FeedRequestDto, FeedSort, FeedTab, UpdateCommunityListRequest,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::post_card::render_post_card;
use super::shared_widgets::{
    render_actor_avatar, render_empty_state, render_input_display, render_loading_skeleton,
};
use super::state::{
    ActiveCommunityInput, BookmarksListsSection, CommunityWorkspaceAction, ListEditorDraft,
};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

/// Computes the ordered deduplicated bookmark folder names: `"All"`, `"Unsorted"`,
/// followed by every distinct custom folder from saved posts and user-created folders.
pub fn compute_bookmark_folders(ws: &CommunityWorkspace) -> Vec<String> {
    let mut folders = vec!["All".to_string(), "Unsorted".to_string()];
    let mut seen: HashSet<String> = folders.iter().map(|f| f.to_ascii_lowercase()).collect();
    for item in &ws.state.saved_items {
        if let Some(ref f) = item.post.viewer_bookmark_folder {
            let trimmed = f.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
                folders.push(trimmed.to_string());
            }
        }
    }
    for custom in &ws.state.bookmark_custom_folders {
        let trimmed = custom.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            folders.push(trimmed.to_string());
        }
    }
    folders
}

fn post_matches_folder(folder_opt: Option<&str>, selected_folder: &str) -> bool {
    if selected_folder.eq_ignore_ascii_case("All") {
        return true;
    }
    let post_folder = folder_opt.map(str::trim).unwrap_or("");
    if selected_folder.eq_ignore_ascii_case("Unsorted") {
        return post_folder.is_empty()
            || post_folder.eq_ignore_ascii_case("All")
            || post_folder.eq_ignore_ascii_case("Unsorted");
    }
    post_folder.eq_ignore_ascii_case(selected_folder)
}

fn list_kind_label(kind: CommunityListKind, lang: &str) -> &'static str {
    match kind {
        CommunityListKind::ActorList => tr_str(lang, "公民列表 (X List)", "Actor List"),
        CommunityListKind::CustomFeed => tr_str(lang, "自定义信息流 (Multireddit)", "Custom Feed"),
        CommunityListKind::Mixed => tr_str(lang, "混合列表", "Mixed"),
    }
}

impl CommunityWorkspace {
    pub fn set_bookmarks_section(
        &mut self,
        section: BookmarksListsSection,
        cx: &mut Context<Self>,
    ) {
        if self.state.bookmarks_section != section {
            self.state.bookmarks_section = section;
            if section == BookmarksListsSection::Lists && self.state.selected_list_id.is_none() {
                if let Some(first_id) = self.state.lists.first().map(|l| l.list_id.clone()) {
                    self.select_list_for_preview(first_id, cx);
                    return;
                }
            }
            cx.notify();
        }
    }

    pub fn select_bookmark_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        self.state.bookmark_selected_folder = folder;
        self.state.bookmark_moving_post_id = None;
        cx.notify();
    }

    pub fn create_bookmark_folder(&mut self, cx: &mut Context<Self>) {
        let name = self.state.bookmark_new_folder_draft.trim().to_string();
        if name.is_empty() {
            return;
        }
        if !name.eq_ignore_ascii_case("All")
            && !name.eq_ignore_ascii_case("Unsorted")
            && !self
                .state
                .bookmark_custom_folders
                .iter()
                .any(|f| f.eq_ignore_ascii_case(&name))
        {
            self.state.bookmark_custom_folders.push(name.clone());
        }
        self.state.bookmark_selected_folder = name.clone();
        self.state.bookmark_new_folder_draft.clear();
        self.state.blur_input();
        let lang = self.language_id.clone();
        self.emit_action(
            CommunityWorkspaceAction::ShowToast {
                text: if lang.starts_with("zh") {
                    format!("已创建收藏夹分组「{name}」")
                } else {
                    format!("Created bookmark folder \"{name}\"")
                },
                is_error: false,
            },
            cx,
        );
        cx.notify();
    }

    /// Deletes a custom bookmark folder while preserving all saved posts by moving them to `"All"`.
    pub fn delete_bookmark_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        if folder.eq_ignore_ascii_case("All") || folder.eq_ignore_ascii_case("Unsorted") {
            return;
        }
        self.state
            .bookmark_custom_folders
            .retain(|f| !f.eq_ignore_ascii_case(&folder));
        let affected_ids: Vec<String> = self
            .state
            .saved_items
            .iter()
            .filter(|it| {
                it.post
                    .viewer_bookmark_folder
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case(&folder))
            })
            .map(|it| it.post.post_id.clone())
            .collect();
        for pid in &affected_ids {
            self.state.visit_post_mut(pid, |p| {
                p.viewer_bookmark_folder = Some("All".to_string());
            });
        }
        if self
            .state
            .bookmark_selected_folder
            .eq_ignore_ascii_case(&folder)
        {
            self.state.bookmark_selected_folder = "All".to_string();
        }
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        if !affected_ids.is_empty() {
            cx.spawn(async move |_weak: WeakEntity<Self>, _app: &mut AsyncApp| {
                let _ = run_on_community_tokio(&handle, async move {
                    let req = BookmarkRequest {
                        folder: Some("All".to_string()),
                        remove: None,
                    };
                    for pid in affected_ids {
                        let _ = facade.bookmark_post(&pid, &req).await;
                    }
                })
                .await;
            })
            .detach();
        }
    }

    pub fn move_bookmark_to_folder(
        &mut self,
        post_id: String,
        folder: String,
        cx: &mut Context<Self>,
    ) {
        let target_folder = if folder.trim().is_empty() || folder.eq_ignore_ascii_case("Unsorted") {
            "All".to_string()
        } else {
            folder.trim().to_string()
        };
        self.state.bookmark_moving_post_id = None;
        self.state.visit_post_mut(&post_id, |p| {
            p.viewer_bookmarked = true;
            p.viewer_bookmark_folder = Some(target_folder.clone());
        });
        let req = BookmarkRequest {
            folder: Some(target_folder.clone()),
            remove: None,
        };
        let my_actor_gen = self.state.actor_switch_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.bookmark_post(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen {
                    return;
                }
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_bookmark(&resp);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn remove_bookmark_from_saved(&mut self, post_id: String, cx: &mut Context<Self>) {
        self.state.bookmark_moving_post_id = None;
        self.state
            .saved_items
            .retain(|it| it.post.post_id != post_id);
        self.state.visit_post_mut(&post_id, |p| {
            p.viewer_bookmarked = false;
            p.viewer_bookmark_folder = None;
        });
        let req = BookmarkRequest {
            folder: None,
            remove: Some(true),
        };
        let my_actor_gen = self.state.actor_switch_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.bookmark_post(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen {
                    return;
                }
                if let Ok(Ok(resp)) = res {
                    this.state.reconcile_bookmark(&resp);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl CommunityWorkspace {
    pub fn select_list_for_preview(&mut self, list_id: String, cx: &mut Context<Self>) {
        self.state.selected_list_id = Some(list_id.clone());
        self.load_selected_list_feed(list_id, cx);
    }

    pub fn load_selected_list_feed(&mut self, list_id: String, cx: &mut Context<Self>) {
        self.state.list_feed_gen = self.state.list_feed_gen.wrapping_add(1);
        let my_gen = self.state.list_feed_gen;
        self.state.list_feed_loading = true;
        let req = FeedRequestDto {
            tab: FeedTab::List(list_id),
            sort: self.state.list_feed_sort,
            actor_filter: self.state.list_feed_actor_filter,
            limit: 25,
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move { facade.get_feed(&req).await })
                .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.list_feed_gen != my_gen {
                    return;
                }
                this.state.list_feed_loading = false;
                if let Ok(Ok(resp)) = res {
                    this.state.list_feed_items = resp.items;
                } else {
                    this.state.list_feed_items.clear();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_list_feed_sort(&mut self, sort: FeedSort, cx: &mut Context<Self>) {
        if self.state.list_feed_sort != sort {
            self.state.list_feed_sort = sort;
            if let Some(lid) = self.state.selected_list_id.clone() {
                self.load_selected_list_feed(lid, cx);
            }
        }
    }

    pub fn set_list_feed_actor_filter(&mut self, filter: ActorFilter, cx: &mut Context<Self>) {
        if self.state.list_feed_actor_filter != filter {
            self.state.list_feed_actor_filter = filter;
            if let Some(lid) = self.state.selected_list_id.clone() {
                self.load_selected_list_feed(lid, cx);
            }
        }
    }

    pub fn open_create_list_form(&mut self, cx: &mut Context<Self>) {
        self.state.bookmarks_section = BookmarksListsSection::Lists;
        self.state.list_editor = ListEditorDraft::default();
        self.state.list_editor_open = true;
        self.state.focus_input(ActiveCommunityInput::ListEditorName);
        cx.notify();
    }

    pub fn open_edit_list_form(&mut self, list: CommunityListDto, cx: &mut Context<Self>) {
        self.state.bookmarks_section = BookmarksListsSection::Lists;
        self.state.list_editor = ListEditorDraft {
            editing_list_id: Some(list.list_id),
            name: list.name,
            description: list.description,
            kind: list.kind,
            is_private: list.is_private,
            is_pinned: list.is_pinned,
            member_actor_ids: list.member_actor_ids,
            community_slugs: list.community_slugs,
            submitting: false,
            error: None,
        };
        self.state.list_editor_open = true;
        self.state.focus_input(ActiveCommunityInput::ListEditorName);
        cx.notify();
    }

    pub fn close_list_editor(&mut self, cx: &mut Context<Self>) {
        self.state.list_editor_open = false;
        self.state.list_editor.error = None;
        if self.state.active_input.is_list_editor_input() {
            self.state.blur_input();
        }
        cx.notify();
    }

    pub fn submit_list_editor(&mut self, cx: &mut Context<Self>) {
        if self.state.list_editor.submitting {
            return;
        }
        let name = self.state.list_editor.name.trim().to_string();
        let lang = self.language_id.as_str();
        if name.is_empty() || name.chars().count() > 80 {
            self.state.list_editor.error = Some(
                tr_str(
                    lang,
                    "列表名称不能为空且不能超过 80 个字符",
                    "List name is required (1..=80 characters)",
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        let editing_id = self.state.list_editor.editing_list_id.clone();
        let editing_id_cb = editing_id.clone();
        let description = self.state.list_editor.description.trim().to_string();
        let kind = self.state.list_editor.kind;
        let is_private = self.state.list_editor.is_private;
        let is_pinned = self.state.list_editor.is_pinned;
        let member_actor_ids = self.state.list_editor.member_actor_ids.clone();
        let community_slugs = self.state.list_editor.community_slugs.clone();

        if let Some(ref lid) = editing_id {
            self.state.list_membership_dirty.remove(lid);
        }
        self.state.list_editor.submitting = true;
        self.state.list_editor.error = None;
        self.state.lists_gen = self.state.lists_gen.wrapping_add(1);
        let my_lists_gen = self.state.lists_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let my_actor_epoch = self.state.list_membership_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let saved = if let Some(lid) = editing_id {
                    facade
                        .update_list(
                            &lid,
                            &UpdateCommunityListRequest {
                                name: Some(name),
                                description: Some(description),
                                kind: Some(kind),
                                is_private: Some(is_private),
                                is_pinned: Some(is_pinned),
                                member_actor_ids: Some(member_actor_ids),
                                community_slugs: Some(community_slugs),
                            },
                        )
                        .await
                } else {
                    facade
                        .create_list(&CreateCommunityListRequest {
                            name,
                            description,
                            kind,
                            is_private,
                            is_pinned,
                            member_actor_ids,
                            community_slugs,
                        })
                        .await
                };
                let lists = facade.list_lists().await.ok();
                (saved, lists)
            })
            .await;

            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.list_membership_gen != my_actor_epoch
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                this.state.list_editor.submitting = false;
                let lang = this.language_id.clone();
                match res {
                    Ok((Ok(saved), lists)) => {
                        let edited_was_in_flight = editing_id_cb
                            .as_ref()
                            .is_some_and(|lid| this.state.list_membership_in_flight.contains_key(lid));
                        if this.state.lists_gen == my_lists_gen {
                            if let Some(l) = lists {
                                this.state.merge_refreshed_lists(l.lists);
                            } else if let Some(pos) = this
                                .state
                                .lists
                                .iter()
                                .position(|x| x.list_id == saved.list_id)
                            {
                                this.state.lists[pos] = saved.clone();
                            } else {
                                this.state.lists.insert(0, saved.clone());
                            }
                        }
                        if let Some(ref lid) = editing_id_cb {
                            if let Some(pos) = this.state.lists.iter().position(|x| &x.list_id == lid) {
                                this.state.lists[pos] = saved.clone();
                            }
                            if edited_was_in_flight {
                                // A pre-editor quick-toggle flight is still running; mark dirty so
                                // once it finishes it flushes the editor's authoritative saved state.
                                this.state.list_membership_dirty.insert(lid.clone());
                            } else {
                                this.state.list_membership_dirty.remove(lid);
                                this.state.list_membership_needs_feed_reload.remove(lid);
                                this.state.list_membership_last_synced.remove(lid);
                            }
                        }
                        this.state.list_editor_open = false;
                        if this.state.active_input.is_list_editor_input() {
                            this.state.blur_input();
                        }
                        this.select_list_for_preview(saved.list_id, cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "列表已保存", "List saved").to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok((Err(e), _)) => {
                        this.state.list_editor.error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.list_editor.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_list_pinned(&mut self, list_id: String, pinned: bool, cx: &mut Context<Self>) {
        if self.state.list_deleting.contains(&list_id) {
            return;
        }
        let Some(pos) = self.state.lists.iter().position(|x| x.list_id == list_id) else {
            return;
        };
        if !self.state.list_membership_last_synced.contains_key(&list_id) {
            let prev = &self.state.lists[pos];
            self.state.list_membership_last_synced.insert(
                list_id.clone(),
                (
                    prev.is_pinned,
                    prev.member_actor_ids.clone(),
                    prev.community_slugs.clone(),
                ),
            );
        }
        self.state.lists[pos].is_pinned = pinned;
        cx.notify();
        self.enqueue_list_sync(list_id, false, cx);
    }

    pub fn delete_list_action(&mut self, list_id: String, cx: &mut Context<Self>) {
        if self.state.list_deleting.contains(&list_id) {
            return;
        }
        self.state.list_deleting.insert(list_id.clone());
        self.state.list_membership_dirty.remove(&list_id);
        self.state.list_membership_needs_feed_reload.remove(&list_id);
        let my_actor_gen = self.state.actor_switch_gen;
        let my_actor_epoch = self.state.list_membership_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let del_id = list_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.delete_list(&del_id).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.list_membership_gen != my_actor_epoch
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                this.state.list_deleting.remove(&list_id);
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(_)) => {
                        this.state.lists_gen = this.state.lists_gen.wrapping_add(1);
                        this.state.list_membership_in_flight.remove(&list_id);
                        this.state.list_membership_dirty.remove(&list_id);
                        this.state.list_membership_needs_feed_reload.remove(&list_id);
                        this.state.list_membership_last_synced.remove(&list_id);
                        this.state.lists.retain(|l| l.list_id != list_id);
                        if this.state.selected_list_id.as_deref() == Some(list_id.as_str()) {
                            this.state.selected_list_id =
                                this.state.lists.first().map(|l| l.list_id.clone());
                            this.state.list_feed_items.clear();
                            if let Some(next_id) = this.state.selected_list_id.clone() {
                                this.load_selected_list_feed(next_id, cx);
                            }
                        }
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "列表已删除", "List deleted").to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_selected_list_actor(
        &mut self,
        list_id: String,
        actor_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.state.list_deleting.contains(&list_id) {
            return;
        }
        let Some(pos) = self.state.lists.iter().position(|l| l.list_id == list_id) else {
            return;
        };
        if !self.state.list_membership_last_synced.contains_key(&list_id) {
            let prev = &self.state.lists[pos];
            self.state.list_membership_last_synced.insert(
                list_id.clone(),
                (
                    prev.is_pinned,
                    prev.member_actor_ids.clone(),
                    prev.community_slugs.clone(),
                ),
            );
        }
        let existing = &mut self.state.lists[pos];
        if let Some(idx) = existing
            .member_actor_ids
            .iter()
            .position(|id| id == &actor_id)
        {
            existing.member_actor_ids.remove(idx);
        } else {
            existing.member_actor_ids.push(actor_id);
        }
        cx.notify();
        self.enqueue_list_sync(list_id, true, cx);
    }

    pub fn toggle_selected_list_community(
        &mut self,
        list_id: String,
        slug: String,
        cx: &mut Context<Self>,
    ) {
        if self.state.list_deleting.contains(&list_id) {
            return;
        }
        let Some(pos) = self.state.lists.iter().position(|l| l.list_id == list_id) else {
            return;
        };
        if !self.state.list_membership_last_synced.contains_key(&list_id) {
            let prev = &self.state.lists[pos];
            self.state.list_membership_last_synced.insert(
                list_id.clone(),
                (
                    prev.is_pinned,
                    prev.member_actor_ids.clone(),
                    prev.community_slugs.clone(),
                ),
            );
        }
        let existing = &mut self.state.lists[pos];
        if let Some(idx) = existing
            .community_slugs
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&slug))
        {
            existing.community_slugs.remove(idx);
        } else {
            existing.community_slugs.push(slug);
        }
        cx.notify();
        self.enqueue_list_sync(list_id, true, cx);
    }

    fn enqueue_list_sync(&mut self, list_id: String, reload_feed: bool, cx: &mut Context<Self>) {
        if self.state.list_deleting.contains(&list_id) {
            return;
        }
        if reload_feed {
            self.state
                .list_membership_needs_feed_reload
                .insert(list_id.clone());
        }
        if self.state.list_membership_in_flight.contains_key(&list_id) {
            // Single-flight per-list coalescing: NEVER spawn a concurrent update_list!
            self.state.list_membership_dirty.insert(list_id);
            return;
        }
        self.flush_list_sync(list_id, cx);
    }

    fn flush_list_sync(&mut self, list_id: String, cx: &mut Context<Self>) {
        if self.state.list_deleting.contains(&list_id) {
            self.state.list_membership_dirty.remove(&list_id);
            self.state.list_membership_needs_feed_reload.remove(&list_id);
            self.state.list_membership_last_synced.remove(&list_id);
            return;
        }
        let Some(snapshot) = self
            .state
            .lists
            .iter()
            .find(|x| x.list_id == list_id)
            .cloned()
        else {
            self.state.list_membership_dirty.remove(&list_id);
            self.state.list_membership_needs_feed_reload.remove(&list_id);
            self.state.list_membership_last_synced.remove(&list_id);
            return;
        };

        self.state.list_membership_dirty.remove(&list_id);
        self.state.list_membership_flight_seq =
            self.state.list_membership_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.list_membership_flight_seq;
        let my_actor_epoch = self.state.list_membership_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .list_membership_in_flight
            .insert(list_id.clone(), my_flight_seq);

        let req = UpdateCommunityListRequest {
            is_pinned: Some(snapshot.is_pinned),
            member_actor_ids: Some(snapshot.member_actor_ids),
            community_slugs: Some(snapshot.community_slugs),
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let req_list_id = list_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_list(&req_list_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                // Guard 1: Actor switch occurred (ABA-proof across A -> B -> A).
                if this.state.list_membership_gen != my_actor_epoch
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                // Guard 2: Flight token ownership.
                if this.state.list_membership_in_flight.get(&list_id).copied()
                    != Some(my_flight_seq)
                {
                    return;
                }
                this.state.list_membership_in_flight.remove(&list_id);

                // Guard 3: List is being deleted or already removed.
                let Some(pos) = this.state.lists.iter().position(|x| x.list_id == list_id) else {
                    this.state.list_membership_dirty.remove(&list_id);
                    this.state.list_membership_needs_feed_reload.remove(&list_id);
                    this.state.list_membership_last_synced.remove(&list_id);
                    cx.notify();
                    return;
                };
                if this.state.list_deleting.contains(&list_id) {
                    this.state.list_membership_dirty.remove(&list_id);
                    this.state.list_membership_needs_feed_reload.remove(&list_id);
                    this.state.list_membership_last_synced.remove(&list_id);
                    cx.notify();
                    return;
                }

                let is_dirty = this.state.list_membership_dirty.remove(&list_id);
                match res {
                    Ok(Ok(updated)) => {
                        this.state.list_membership_last_synced.insert(
                            list_id.clone(),
                            (
                                updated.is_pinned,
                                updated.member_actor_ids.clone(),
                                updated.community_slugs.clone(),
                            ),
                        );
                        if is_dirty {
                            // Preserve optimistic state.lists[pos] and flush strictly next.
                            this.flush_list_sync(list_id, cx);
                            return;
                        }
                        this.state.lists[pos] = updated;
                        this.state.list_membership_last_synced.remove(&list_id);
                        if this.state.list_membership_needs_feed_reload.remove(&list_id)
                            && this.state.selected_list_id.as_deref() == Some(list_id.as_str())
                        {
                            this.load_selected_list_feed(list_id, cx);
                        }
                    }
                    Ok(Err(e)) => {
                        this.state.list_membership_dirty.remove(&list_id);
                        this.state.list_membership_needs_feed_reload.remove(&list_id);
                        if let Some((prev_pinned, prev_members, prev_slugs)) =
                            this.state.list_membership_last_synced.remove(&list_id)
                        {
                            let entry = &mut this.state.lists[pos];
                            entry.is_pinned = prev_pinned;
                            entry.member_actor_ids = prev_members;
                            entry.community_slugs = prev_slugs;
                        }
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.list_membership_dirty.remove(&list_id);
                        this.state.list_membership_needs_feed_reload.remove(&list_id);
                        if let Some((prev_pinned, prev_members, prev_slugs)) =
                            this.state.list_membership_last_synced.remove(&list_id)
                        {
                            let entry = &mut this.state.lists[pos];
                            entry.is_pinned = prev_pinned;
                            entry.member_actor_ids = prev_members;
                            entry.community_slugs = prev_slugs;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Renders the full F20 Saved Bookmarks & X Lists / Custom Feeds view.
pub fn render_bookmarks_lists_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let section = ws.state.bookmarks_section;

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
            tr_str(lang, "收藏夹与自定义列表", "Bookmarks & Custom Lists").to_string(),
            Some(format!(
                "{} {} · {} {}",
                ws.state.saved_items.len(),
                tr_str(lang, "篇收藏", "saved"),
                ws.state.lists.len(),
                tr_str(lang, "个列表", "lists"),
            )),
            cx,
        ))
        .child(
            div()
                .id("community-bookmarks-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        // Top Dual-Section Switcher
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .id("bmk-section-bookmarks")
                                                .cursor_pointer()
                                                .px(px(13.0))
                                                .py(px(6.0))
                                                .rounded_full()
                                                .bg(if section == BookmarksListsSection::Bookmarks {
                                                    rgb(pal.accent).into()
                                                } else {
                                                    pal.service_pill_rgba()
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(5.0))
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(
                                                    if section == BookmarksListsSection::Bookmarks {
                                                        pal.active_button_fg
                                                    } else {
                                                        pal.text_primary
                                                    },
                                                ))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.set_bookmarks_section(
                                                            BookmarksListsSection::Bookmarks,
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(tg_icon(
                                                    IconName::Bookmark,
                                                    13.0,
                                                    if section == BookmarksListsSection::Bookmarks {
                                                        pal.active_button_fg
                                                    } else {
                                                        pal.text_secondary
                                                    },
                                                ))
                                                .child(tr_str(lang, "已收藏帖子", "Saved Bookmarks")),
                                        )
                                        .child(
                                            div()
                                                .id("bmk-section-lists")
                                                .cursor_pointer()
                                                .px(px(13.0))
                                                .py(px(6.0))
                                                .rounded_full()
                                                .bg(if section == BookmarksListsSection::Lists {
                                                    rgb(pal.accent).into()
                                                } else {
                                                    pal.service_pill_rgba()
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(5.0))
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(
                                                    if section == BookmarksListsSection::Lists {
                                                        pal.active_button_fg
                                                    } else {
                                                        pal.text_primary
                                                    },
                                                ))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.set_bookmarks_section(
                                                            BookmarksListsSection::Lists,
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(tg_icon(
                                                    IconName::CommunityGlobe,
                                                    13.0,
                                                    if section == BookmarksListsSection::Lists {
                                                        pal.active_button_fg
                                                    } else {
                                                        pal.text_secondary
                                                    },
                                                ))
                                                .child(tr_str(
                                                    lang,
                                                    "列表与自定义信息流",
                                                    "Lists & Custom Feeds",
                                                )),
                                        ),
                                )
                                .when(section == BookmarksListsSection::Lists, |row| {
                                    row.child(
                                        div()
                                            .id("lists-new-btn")
                                            .cursor_pointer()
                                            .px(px(12.0))
                                            .py(px(6.0))
                                            .rounded_full()
                                            .bg(rgb(pal.accent))
                                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.active_button_fg))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.open_create_list_form(cx);
                                                }),
                                            )
                                            .child(tr_str(lang, "+ 新建列表", "+ New List")),
                                    )
                                }),
                        )
                        .child(match section {
                            BookmarksListsSection::Bookmarks => {
                                render_saved_bookmarks_section(ws, cx)
                            }
                            BookmarksListsSection::Lists => render_lists_section(ws, cx),
                        }),
                ),
        )
        .into_any_element()
}

fn render_saved_bookmarks_section(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let folders = compute_bookmark_folders(ws);
    let selected_folder = ws.state.bookmark_selected_folder.clone();
    let new_folder_foc = ws.state.active_input == ActiveCommunityInput::BookmarkNewFolder;
    let filtered_saved: Vec<_> = ws
        .state
        .saved_items
        .iter()
        .filter(|it| {
            post_matches_folder(it.post.viewer_bookmark_folder.as_deref(), &selected_folder)
        })
        .cloned()
        .collect();
    let moving_pid = ws.state.bookmark_moving_post_id.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Folder filter tabs + inline New Folder input
        .child(
            div()
                .p(px(11.0))
                .rounded(px(11.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(5.0))
                        .children(folders.iter().map(|folder| {
                            let active = selected_folder.eq_ignore_ascii_case(folder);
                            let count = ws
                                .state
                                .saved_items
                                .iter()
                                .filter(|it| {
                                    post_matches_folder(
                                        it.post.viewer_bookmark_folder.as_deref(),
                                        folder,
                                    )
                                })
                                .count();
                            let is_custom = !folder.eq_ignore_ascii_case("All")
                                && !folder.eq_ignore_ascii_case("Unsorted");
                            let f_select = folder.clone();
                            let f_del = folder.clone();
                            div()
                                .id(SharedString::from(format!("bmk-folder-{folder}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.5))
                                .rounded_full()
                                .bg(if active {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if active {
                                    pal.active_button_fg
                                } else {
                                    pal.text_primary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.select_bookmark_folder(f_select.clone(), cx);
                                    }),
                                )
                                .child(format!("{folder} ({count})"))
                                .when(is_custom, |pill| {
                                    pill.child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "bmk-folder-del-{f_del}"
                                            )))
                                            .cursor_pointer()
                                            .px(px(3.0))
                                            .rounded_full()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.delete_bookmark_folder(f_del.clone(), cx);
                                                }),
                                            )
                                            .child(tg_icon(
                                                IconName::Close,
                                                10.0,
                                                if active {
                                                    pal.active_button_fg
                                                } else {
                                                    pal.text_secondary
                                                },
                                            )),
                                    )
                                })
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            div()
                                .id("bmk-new-folder-input")
                                .cursor_text()
                                .flex_1()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(if new_folder_foc {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.hairline_border_rgba()
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state
                                            .focus_input(ActiveCommunityInput::BookmarkNewFolder);
                                        cx.notify();
                                    }),
                                )
                                .child(render_input_display(
                                    &ws.state.bookmark_new_folder_draft,
                                    if new_folder_foc {
                                        ws.state.ime_preedit.as_deref()
                                    } else {
                                        None
                                    },
                                    tr_str(
                                        lang,
                                        "新建收藏分组名称 (如 Rust、AI 论文)…",
                                        "New folder name (e.g. Rust, Research)…",
                                    ),
                                    new_folder_foc,
                                    &pal,
                                )),
                        )
                        .child(
                            div()
                                .id("bmk-create-folder-btn")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.5))
                                .rounded(px(8.0))
                                .bg(rgb(pal.accent))
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.create_bookmark_folder(cx);
                                    }),
                                )
                                .child(tr_str(lang, "创建分组", "Create")),
                        ),
                ),
        )
        .when(ws.state.saved_loading && filtered_saved.is_empty(), |col| {
            col.child(render_loading_skeleton(3, &pal))
        })
        .when(!ws.state.saved_loading && filtered_saved.is_empty(), |col| {
            col.child(render_empty_state(
                IconName::Bookmark,
                tr_str(lang, "该分组暂无收藏帖子", "No saved posts in this folder"),
                tr_str(
                    lang,
                    "点击任意帖子底部的书签图标收藏，或将其移动到此分组。",
                    "Click the bookmark icon on any post or move existing bookmarks into this folder.",
                ),
                &pal,
            ))
        })
        .children(filtered_saved.iter().enumerate().map(|(i, item)| {
            let pid = item.post.post_id.clone();
            let pid_move_toggle = pid.clone();
            let pid_rm = pid.clone();
            let cur_folder = item
                .post
                .viewer_bookmark_folder
                .clone()
                .unwrap_or_else(|| "Unsorted".to_string());
            let show_picker = moving_pid.as_deref() == Some(pid.as_str());
            let picker_folders = folders.clone();

            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(render_post_card(ws, item, i, cx))
                .child(
                    div()
                        .px(px(11.0))
                        .py(px(6.0))
                        .rounded(px(9.0))
                        .bg(pal.card_surface_rgba())
                        .border_1()
                        .border_color(pal.hairline_border_rgba())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "{}: {}",
                                    tr_str(lang, "分组", "Folder"),
                                    cur_folder
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("bmk-move-btn-{pid}")))
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(3.5))
                                        .rounded_full()
                                        .bg(pal.service_pill_rgba())
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.accent))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if this.state.bookmark_moving_post_id.as_deref()
                                                    == Some(pid_move_toggle.as_str())
                                                {
                                                    this.state.bookmark_moving_post_id = None;
                                                } else {
                                                    this.state.bookmark_moving_post_id =
                                                        Some(pid_move_toggle.clone());
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "移动到分组", "Move to Folder")),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("bmk-remove-btn-{pid}")))
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(3.5))
                                        .rounded_full()
                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                            pal.danger, 0.14,
                                        ))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.danger))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.remove_bookmark_from_saved(pid_rm.clone(), cx);
                                            }),
                                        )
                                        .child(tr_str(lang, "取消收藏", "Remove")),
                                ),
                        )
                        .when(show_picker, |bar| {
                            bar.child(
                                div()
                                    .w_full()
                                    .pt(px(4.0))
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .children(picker_folders.into_iter().map(|target_f| {
                                        let p_move = pid.clone();
                                        let tf = target_f.clone();
                                        div()
                                            .id(SharedString::from(format!(
                                                "bmk-move-opt-{pid}-{target_f}"
                                            )))
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(3.0))
                                            .rounded_full()
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                pal.accent, 0.14,
                                            ))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.move_bookmark_to_folder(
                                                        p_move.clone(),
                                                        tf.clone(),
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(target_f)
                                    })),
                            )
                        }),
                )
        }))
}

fn collect_known_actors_for_list_picker(ws: &CommunityWorkspace) -> Vec<CommunityActorDto> {
    let mut actors: Vec<CommunityActorDto> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(ref ids) = ws.state.identities {
        for a in &ids.all_actors {
            if seen.insert(a.actor_id.clone()) {
                actors.push(a.clone());
            }
        }
    }
    for a in &ws.state.recommended_actors {
        if seen.insert(a.actor_id.clone()) {
            actors.push(a.clone());
        }
    }
    actors
}

fn render_list_editor_card(
    ws: &CommunityWorkspace,
    known_actors: &[CommunityActorDto],
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let ed = &ws.state.list_editor;
    let is_edit = ed.editing_list_id.is_some();
    let name_foc = ws.state.active_input == ActiveCommunityInput::ListEditorName;
    let desc_foc = ws.state.active_input == ActiveCommunityInput::ListEditorDesc;
    let kinds = [
        CommunityListKind::ActorList,
        CommunityListKind::CustomFeed,
        CommunityListKind::Mixed,
    ];
    let comms = ws.state.communities.clone();

    div()
        .p(px(13.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(rgb(pal.accent))
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(if is_edit {
                            tr_str(lang, "编辑自定义列表", "Edit List / Custom Feed")
                        } else {
                            tr_str(lang, "创建新列表 / 自定义信息流", "Create New List / Custom Feed")
                        }),
                )
                .child(
                    div()
                        .id("list-editor-close-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded_full()
                        .bg(pal.service_pill_rgba())
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_list_editor(cx);
                            }),
                        )
                        .child(tr_str(lang, "取消", "Cancel")),
                ),
        )
        .child(
            div()
                .id("list-editor-name-input")
                .cursor_text()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if name_foc {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.focus_input(ActiveCommunityInput::ListEditorName);
                        cx.notify();
                    }),
                )
                .child(render_input_display(
                    &ed.name,
                    if name_foc {
                        ws.state.ime_preedit.as_deref()
                    } else {
                        None
                    },
                    tr_str(lang, "列表名称 (如 AI 架构师 & Rust 社区)", "List name"),
                    name_foc,
                    &pal,
                )),
        )
        .child(
            div()
                .id("list-editor-desc-input")
                .cursor_text()
                .min_h(px(42.0))
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if desc_foc {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.focus_input(ActiveCommunityInput::ListEditorDesc);
                        cx.notify();
                    }),
                )
                .child(render_input_display(
                    &ed.description,
                    if desc_foc {
                        ws.state.ime_preedit.as_deref()
                    } else {
                        None
                    },
                    tr_str(lang, "列表说明 (可选)", "Description (optional)"),
                    desc_foc,
                    &pal,
                )),
        )
        // Kind & Privacy/Pin toggles
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .children(kinds.into_iter().map(|k| {
                    let active = ed.kind == k;
                    div()
                        .id(SharedString::from(format!("list-ed-kind-{k:?}")))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if active {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if active {
                            pal.active_button_fg
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.list_editor.kind = k;
                                cx.notify();
                            }),
                        )
                        .child(list_kind_label(k, lang))
                }))
                .child(
                    div()
                        .id("list-ed-private-toggle")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if ed.is_private {
                            TdesktopThemePalette::rgba_with_alpha(0xF59E0B, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .text_color(rgb(if ed.is_private {
                            0xF59E0B
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.list_editor.is_private =
                                    !this.state.list_editor.is_private;
                                cx.notify();
                            }),
                        )
                        .child(if ed.is_private {
                            tr_str(lang, "私密列表", "Private")
                        } else {
                            tr_str(lang, "公开列表", "Public")
                        }),
                )
                .child(
                    div()
                        .id("list-ed-pinned-toggle")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if ed.is_pinned {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .text_color(rgb(if ed.is_pinned {
                            pal.accent
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.list_editor.is_pinned =
                                    !this.state.list_editor.is_pinned;
                                cx.notify();
                            }),
                        )
                        .child(if ed.is_pinned {
                            tr_str(lang, "已置顶到导航", "Pinned to Nav")
                        } else {
                            tr_str(lang, "置顶到左侧导航", "Pin to Nav")
                        }),
                ),
        )
        // Quick Actor & Community Picker inside Editor
        .when(!known_actors.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "包含人类与 Bot 公民:",
                                "Include Humans & Bots:",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.0))
                            .children(known_actors.iter().take(12).map(|a| {
                                let aid = a.actor_id.clone();
                                let selected = ed.member_actor_ids.iter().any(|x| x == &aid);
                                div()
                                    .id(SharedString::from(format!("list-ed-actor-{aid}")))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded_full()
                                    .bg(if selected {
                                        rgb(pal.accent).into()
                                    } else {
                                        pal.service_pill_rgba()
                                    })
                                    .text_size(px(11.0))
                                    .text_color(rgb(if selected {
                                        pal.active_button_fg
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let vec = &mut this.state.list_editor.member_actor_ids;
                                            if let Some(pos) =
                                                vec.iter().position(|id| id == &aid)
                                            {
                                                vec.remove(pos);
                                            } else {
                                                vec.push(aid.clone());
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(format!("@{}", a.handle))
                            })),
                    ),
            )
        })
        .when(!comms.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "包含 c/ 社区:", "Include c/ Communities:")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.0))
                            .children(comms.into_iter().take(12).map(|c| {
                                let slug = c.slug.clone();
                                let selected = ed
                                    .community_slugs
                                    .iter()
                                    .any(|s| s.eq_ignore_ascii_case(&slug));
                                div()
                                    .id(SharedString::from(format!("list-ed-comm-{slug}")))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded_full()
                                    .bg(if selected {
                                        rgb(pal.accent).into()
                                    } else {
                                        pal.service_pill_rgba()
                                    })
                                    .text_size(px(11.0))
                                    .text_color(rgb(if selected {
                                        pal.active_button_fg
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let vec = &mut this.state.list_editor.community_slugs;
                                            if let Some(pos) = vec
                                                .iter()
                                                .position(|s| s.eq_ignore_ascii_case(&slug))
                                            {
                                                vec.remove(pos);
                                            } else {
                                                vec.push(slug.clone());
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(format!("c/{}", c.slug))
                            })),
                    ),
            )
        })
        .when_some(ed.error.clone(), |col, err| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.danger))
                    .child(err),
            )
        })
        .child(
            div()
                .id("list-editor-submit-btn")
                .cursor_pointer()
                .py(px(8.0))
                .rounded(px(9.0))
                .bg(rgb(pal.accent))
                .hover(move |s| s.bg(rgb(pal.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.submit_list_editor(cx);
                    }),
                )
                .child(if is_edit {
                    tr_str(lang, "保存列表修改", "Save Changes")
                } else {
                    tr_str(lang, "创建列表", "Create List")
                }),
        )
}

fn render_lists_section(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let lists = ws.state.lists.clone();
    let known_actors = collect_known_actors_for_list_picker(ws);
    let selected_id = ws
        .state
        .selected_list_id
        .clone()
        .or_else(|| lists.first().map(|l| l.list_id.clone()));
    let selected_list = selected_id
        .as_deref()
        .and_then(|id| lists.iter().find(|l| l.list_id == id).cloned());
    let active_actor_id = ws.state.active_actor_id.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .when(ws.state.list_editor_open, |col| {
            col.child(render_list_editor_card(ws, &known_actors, cx))
        })
        .when(lists.is_empty() && !ws.state.list_editor_open, |col| {
            col.child(render_empty_state(
                IconName::CommunityGlobe,
                tr_str(lang, "还没有自定义列表", "No lists or custom feeds yet"),
                tr_str(
                    lang,
                    "创建自定义列表，将你关注的人类、Bot 和 c/ 社区聚合成专属时间线！",
                    "Create a custom list to combine favorite Humans, Bots, and c/ communities into a dedicated feed.",
                ),
                &pal,
            ))
        })
        // Lists Directory Cards
        .when(!lists.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .children(lists.into_iter().map(|list| {
                        let is_selected = selected_id.as_deref() == Some(list.list_id.as_str());
                        let is_owner = list.owner_actor_id == active_actor_id
                            || ws.is_viewer_owned_actor(&list.owner_actor_id);
                        let lid_sel = list.list_id.clone();
                        let lid_pin = list.list_id.clone();
                        let lid_del = list.list_id.clone();
                        let lid_feed = list.list_id.clone();
                        let edit_list = list.clone();
                        let next_pin = !list.is_pinned;

                        div()
                            .id(SharedString::from(format!("bmk-list-{}", list.list_id)))
                            .cursor_pointer()
                            .p(px(11.0))
                            .rounded(px(11.0))
                            .bg(pal.card_surface_rgba())
                            .border_1()
                            .border_color(if is_selected {
                                rgb(pal.accent).into()
                            } else {
                                pal.hairline_border_rgba()
                            })
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.select_list_for_preview(lid_sel.clone(), cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(13.5))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(list.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .px(px(7.0))
                                                    .py(px(1.5))
                                                    .rounded_full()
                                                    .bg(pal.service_pill_rgba())
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(list_kind_label(list.kind, lang)),
                                            )
                                            .when(list.is_private, |row| {
                                                row.child(tg_icon(
                                                    IconName::Lock,
                                                    11.0,
                                                    pal.text_secondary,
                                                ))
                                            })
                                            .when(list.is_pinned, |row| {
                                                row.child(tg_icon(IconName::Pin, 11.0, pal.accent))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .flex_shrink_0()
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "list-open-feed-{}",
                                                        list.list_id
                                                    )))
                                                    .cursor_pointer()
                                                    .px(px(8.0))
                                                    .py(px(3.5))
                                                    .rounded_full()
                                                    .bg(TdesktopThemePalette::rgba_with_alpha(
                                                        pal.accent, 0.14,
                                                    ))
                                                    .text_size(px(10.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(pal.accent))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.select_feed_tab(
                                                                FeedTab::List(lid_feed.clone()),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .child(tr_str(lang, "在信息流打开", "Open in Feed")),
                                            )
                                            .when(is_owner, |row| {
                                                row.child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "list-pin-{}",
                                                            list.list_id
                                                        )))
                                                        .cursor_pointer()
                                                        .px(px(8.0))
                                                        .py(px(3.5))
                                                        .rounded_full()
                                                        .bg(pal.service_pill_rgba())
                                                        .text_size(px(10.5))
                                                        .text_color(rgb(if list.is_pinned {
                                                            pal.accent
                                                        } else {
                                                            pal.text_secondary
                                                        }))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.toggle_list_pinned(
                                                                    lid_pin.clone(),
                                                                    next_pin,
                                                                    cx,
                                                                );
                                                            }),
                                                        )
                                                        .child(if list.is_pinned {
                                                            tr_str(lang, "取消置顶", "Unpin")
                                                        } else {
                                                            tr_str(lang, "置顶", "Pin")
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "list-edit-{}",
                                                            list.list_id
                                                        )))
                                                        .cursor_pointer()
                                                        .px(px(8.0))
                                                        .py(px(3.5))
                                                        .rounded_full()
                                                        .bg(pal.service_pill_rgba())
                                                        .text_size(px(10.5))
                                                        .text_color(rgb(pal.text_primary))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.open_edit_list_form(
                                                                    edit_list.clone(),
                                                                    cx,
                                                                );
                                                            }),
                                                        )
                                                        .child(tr_str(lang, "编辑", "Edit")),
                                                )
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "list-del-{}",
                                                            list.list_id
                                                        )))
                                                        .cursor_pointer()
                                                        .px(px(8.0))
                                                        .py(px(3.5))
                                                        .rounded_full()
                                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                                            pal.danger, 0.14,
                                                        ))
                                                        .text_size(px(10.5))
                                                        .text_color(rgb(pal.danger))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.delete_list_action(
                                                                    lid_del.clone(),
                                                                    cx,
                                                                );
                                                            }),
                                                        )
                                                        .child(tr_str(lang, "删除", "Delete")),
                                                )
                                            }),
                                    ),
                            )
                            .when(!list.description.trim().is_empty(), |card| {
                                card.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(list.description.clone()),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "{} {} · {} c/",
                                        list.member_actor_ids.len(),
                                        tr_str(lang, "位人类/Bot 成员", "actors"),
                                        list.community_slugs.len(),
                                    )),
                            )
                    })),
            )
        })
        // Selected List Membership Editor + Inline Live Timeline Stream
        .when_some(selected_list, |col, list| {
            col.child(render_selected_list_detail(ws, list, &known_actors, cx))
        })
}

fn render_selected_list_detail(
    ws: &CommunityWorkspace,
    list: CommunityListDto,
    known_actors: &[CommunityActorDto],
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let is_owner = list.owner_actor_id == ws.state.active_actor_id
        || ws.is_viewer_owned_actor(&list.owner_actor_id);
    let lid_open = list.list_id.clone();
    let base_url = ws.media_base_url.clone();
    let sorts = [
        (FeedSort::New, "最新", "New"),
        (FeedSort::Hot, "最热", "Hot"),
        (FeedSort::Top, "高赞", "Top"),
        (FeedSort::Algo, "推荐", "Algo"),
    ];
    let actor_filters = [
        (ActorFilter::AllCitizens, "全部", "All"),
        (ActorFilter::HumansOnly, "人类", "Humans"),
        (ActorFilter::BotsOnly, "Bots", "Bots"),
        (ActorFilter::HumanBotDebates, "辩论", "Debates"),
    ];

    div()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{}: {}",
                            tr_str(lang, "列表实时时间线", "List Timeline"),
                            list.name
                        )),
                )
                .child(
                    div()
                        .id("selected-list-open-feed-cta")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(rgb(pal.accent))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.select_feed_tab(FeedTab::List(lid_open.clone()), cx);
                            }),
                        )
                        .child(tr_str(lang, "在主信息流打开", "Open in Feed")),
                ),
        )
        // Interactive Membership Chips (Owner can add/remove Actors & c/ Communities inline!)
        .when(is_owner, |col| {
            let lid_a = list.list_id.clone();
            let lid_c = list.list_id.clone();
            let mem_actors = list.member_actor_ids.clone();
            let mem_comms = list.community_slugs.clone();
            let comms = ws.state.communities.clone();
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "点击增减人类 / Bot 成员:",
                                "Toggle Human & Bot Members:",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.0))
                            .children(known_actors.iter().take(12).map(|a| {
                                let in_list = mem_actors.iter().any(|id| id == &a.actor_id);
                                let l_id = lid_a.clone();
                                let a_id = a.actor_id.clone();
                                div()
                                    .id(SharedString::from(format!(
                                        "list-mem-actor-{}-{}",
                                        l_id, a.actor_id
                                    )))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded_full()
                                    .bg(if in_list {
                                        rgb(pal.accent).into()
                                    } else {
                                        pal.service_pill_rgba()
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_size(px(11.0))
                                    .text_color(rgb(if in_list {
                                        pal.active_button_fg
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_selected_list_actor(
                                                l_id.clone(),
                                                a_id.clone(),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(render_actor_avatar(
                                        &a.actor_id,
                                        &a.display_name,
                                        a.actor_kind,
                                        a.avatar_media_id.as_deref(),
                                        &a.avatar_shape,
                                        &a.avatar_color,
                                        16.0,
                                        pal.card_bg,
                                        &base_url,
                                        true,
                                        &pal,
                                    ))
                                    .child(format!(
                                        "{}@{}",
                                        if in_list { "✓ " } else { "+ " },
                                        a.handle
                                    ))
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "点击增减 c/ 社区:", "Toggle c/ Communities:")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.0))
                            .children(comms.into_iter().take(12).map(|c| {
                                let in_list = mem_comms
                                    .iter()
                                    .any(|s| s.eq_ignore_ascii_case(&c.slug));
                                let l_id = lid_c.clone();
                                let slug = c.slug.clone();
                                div()
                                    .id(SharedString::from(format!("list-mem-comm-{l_id}-{slug}")))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded_full()
                                    .bg(if in_list {
                                        rgb(pal.accent).into()
                                    } else {
                                        pal.service_pill_rgba()
                                    })
                                    .text_size(px(11.0))
                                    .text_color(rgb(if in_list {
                                        pal.active_button_fg
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_selected_list_community(
                                                l_id.clone(),
                                                slug.clone(),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(format!(
                                        "{}c/{}",
                                        if in_list { "✓ " } else { "+ " },
                                        c.slug
                                    ))
                            })),
                    ),
            )
        })
        // Sort + Actor Filter Controls for Inline List Stream
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .children(sorts.into_iter().map(|(s_val, zh, en)| {
                            let active = ws.state.list_feed_sort == s_val;
                            div()
                                .id(SharedString::from(format!("list-feed-sort-{en}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.5))
                                .rounded(px(7.0))
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if active {
                                    pal.accent
                                } else {
                                    pal.text_secondary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_list_feed_sort(s_val, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .children(actor_filters.into_iter().map(|(af, zh, en)| {
                            let active = ws.state.list_feed_actor_filter == af;
                            div()
                                .id(SharedString::from(format!("list-feed-actor-{en}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.5))
                                .rounded(px(7.0))
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if active {
                                    0x8B5CF6
                                } else {
                                    pal.text_secondary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_list_feed_actor_filter(af, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                ),
        )
        // Live List Posts Stream
        .when(
            ws.state.list_feed_loading && ws.state.list_feed_items.is_empty(),
            |col| col.child(render_loading_skeleton(2, &pal)),
        )
        .when(
            !ws.state.list_feed_loading && ws.state.list_feed_items.is_empty(),
            |col| {
                col.child(render_empty_state(
                    IconName::CommunityGlobe,
                    tr_str(lang, "该列表暂无匹配帖子", "No posts in this list yet"),
                    tr_str(
                        lang,
                        "在上方勾选人类、Bot 成员或 c/ 社区，即可实时聚合他们的讨论。",
                        "Add Human/Bot members or c/ communities above to populate this custom feed.",
                    ),
                    &pal,
                ))
            },
        )
        .children(
            ws.state
                .list_feed_items
                .iter()
                .enumerate()
                .map(|(i, item)| render_post_card(ws, item, 1000 + i, cx)),
        )
}
