# Telegram `@GrokAI` & Bot / Mini App Native Interaction Specification & Parity Breakdown

**Document ID**: `DOC-GROK-BOT-INTERACTION-SPEC`  
**Revision**: `1.0.0` (Milestone M2 Alignment Baseline)  
**Date**: `2026-11-23`  
**Status**: Authoritative Specification & Implementation Parity Matrix  
**Target Path**: `docs/grok_bot_interaction_spec.md`  

**Authoritative Upstream & Workspace References**:
- **Telegram Bot API** (`core.telegram.org/bots/api` — `MenuButton`, `MenuButtonWebApp`, `MenuButtonCommands`, `MenuButtonDefault`, `WebAppInfo`, `InlineKeyboardButton`, `ReplyKeyboardMarkup`, `ReplyKeyboardRemove`, `ForceReply`, `BotCommand`, `setMyDescription`, `sendChatAction`, `sendMessageDraft`)
- **Telegram Mini Apps** (`core.telegram.org/bots/webapps` — Main Mini App, `messages.requestMainWebView`, `AttachMenuBot`, Bot Profile `"Open App"`, Privacy Policy / Bot Developer Terms)
- **Telegram Bot Features** (`core.telegram.org/bots/features` — `/start`, `/help`, `/settings`, command scopes, inline mode, attachment menu)
- **MTProto / TDLib Schema** (`botInfo`, `userFullInfo`, `basicGroupFullInfo`, `supergroupFullInfo`, `ChatType`, `BotMenuButton`)
- **Telegram Desktop (`tdesktop`)** (`HistoryView::BotMenuButton`, `HistoryView::ComposeControls`, `HistoryWidget`, `Info::Profile`, `Ui::Toast`, `PeerListBox`)
- **Telegram `@GrokAI` Official Rollout** (March 2025 xAI / Telegram integration — 1v1 conversational assistant, group `@GrokAI` mentions, inline queries, command shortcuts, clean input capsule)
- **Cross-References**: `docs/architecture.md`, `crates/gpui_telegram_desktop/src/views/`

---

## 1. `@GrokAI` Native Interaction Model & Visual Design Grammar

Telegram's official `@GrokAI` bot behaves strictly as a standard native Telegram Bot enhanced with streaming drafts, group mention routing, inline queries, and minimal visual noise. No bespoke buttons or Mini App launchers are embedded inside the text input field.

### 1.1 Interaction Contexts
1. **1v1 Private Chat (`@GrokAI` / Fabushi / Standard Bots)**:
   - Direct conversational prompts without requiring `@mention` prefix.
   - Standard command shortcuts (`/start`, `/help`, `/clear`, `/reset`, `/model`, `/status`) triggered from `/` autocomplete or clickable command tokens in message bubbles.
   - Clean text input capsule containing only `+`/attach, text buffer, formatting (`Aa`), optional reply-keyboard toggle, emoji, and mic/send.
2. **Group & Supergroup Chats (`@GrokAI <query>`, `@everyone`)**:
   - Bots respond when explicitly `@mention`ed (or replied to), replying directly to the triggering message.
   - Multi-bot room orchestration uses `RoomAgentLiveState` (`crates/gpui_telegram_desktop/src/views/conversation_stage.rs`) with bounded lease expiry (`DEFAULT_ROOM_AGENT_STALE_AFTER_MS`) so interrupted or crashed turns never leave stuck speaker/handoff indicators.
   - `@mention` autocomplete (`FieldAutocomplete`, `build_mention_candidates` in `shell_stage_actions.rs`) surfaces `@everyone` (`所有人`), room bots (`AI Bot`), and group members (`群主`/`管理员`/`成员`).
3. **Inline Mode (`@GrokAI <query>`)**:
   - Typing `@bot_username <query>` in any chat triggers inline query results or pre-fills inline bot queries (`CanonicalComposer::prefill_inline_bot_query`).

### 1.2 Visual Surface Ladder & 9-Hue Bot Identity System
- **4-Step Elevation Surface Ladder** (`TdesktopThemePalette::build` in `crates/gpui_telegram_desktop/src/theme/tdesktop_palette.rs`, re-exported via `theme/palette.rs`; presets in `theme/palette_presets.rs`):
  - **Dark (`Night` / Apple Dark)**: Sidebar `#0E0E0E` < Chrome/Topbar `#121212` < Stage Pane `#161616` < Elevated Card/Menu `#1F1F1F` < Hover `#2C2C2C` (`text_primary = #F5F5F5`), eliminating heavy drop shadows in favor of surface contrast and `12%` top specular highlights (`specular_top_highlight_rgba`).
  - **Light (`Day` / Apple Light)**: Sidebar `#F5F5F5` < Chrome/Topbar `#FCFCFC` < Stage/Card `#FFFFFF` (`menu_hover_bg = #F1F1F1`, `text_primary = #1C1C1C`).
- **9-Hue Bot Identity Tinting** (`TdesktopThemePalette::bot_hue_colors`, `GrokBotAvatarProps` in `components/grok_bot_avatar.rs`):
  - Supports 9 chromatic bot hues (`blue`, `red`, `green`, `yellow`, `orange`, `cyan`, `purple`, `magenta`, `brown`) adapting distinct bubble/tint/accent tokens across dark and light modes, plus neutral monochrome fallback.

---

## 2. Conversation Area & Bot Lifecycle Specification

### 2.1 Initial Pre-Start Bot State (`"What can this bot do?"` + `START` Bar)
- **Native Telegram Spec**: Bots cannot message first. Opening an unstarted bot chat with empty history renders a centered description card (`"What can this bot do?"` / `BotInfo.description` + optional media) and replaces the bottom message composer with a full-width `"START"` action bar.
- **`gpui-telegram` Implementation**:
  - `ConversationStage::show_bot_start_bar`, `bot_description_card`, and `pending_bot_start` (`conversation_stage.rs`) render `#stage-bot-description-card` and the `52px` `#stage-bot-start-bar` / `#stage-bot-start-btn` (`"开始"` / `"START"`).
  - `AppShell::should_hide_composer` and `AppShell::render` (`shell.rs`) suppress `self.composer` whenever `!is_apple_compose_mode && self.stage.read(cx).show_bot_start_bar` is `true`, and `observe_stage_composer_inspector_entities` (`shell_observers.rs`) ensures `composer.is_input_focused = false` while the `START` bar is active.
  - Clicking `#stage-bot-start-btn` triggers `materialize_prestart_bot_and_start` (`shell_miniapp_catalog.rs`), adding the contact/chat if provisional, sending `/start`, setting `show_bot_start_bar = false`, and revealing `CanonicalComposer`.

### 2.2 Streaming Drafts (`sendMessageDraft` & `sendChatAction`)
- **Native Telegram Spec**: AI bots stream partial responses via ephemeral `sendChatAction` (`typing`, `<= 5s` TTL) and `sendMessageDraft` (keyed by `draft_id != 0`, showing `"Thinking…"` when text is empty, with optional Stop control) before persisting the final message bubble.
- **`gpui-telegram` Implementation**:
  - Handled in `shell_pts_updates.rs` and `conversation_stage.rs` via live draft/streaming state (`GrokPersonaState` in `components/grok_bot_avatar.rs`: idle/thinking/executing/blocked) that smoothly transitions to the final persisted `MessageItemModel` on turn completion.

### 2.3 Inline Keyboards & Reply Keyboards (`BotKeyboard`)
- **Inline Keyboards (`InlineKeyboardMarkup`)**:
  - Rendered per message bubble (`InlineBotKeyboardView` in `stage_bubble.rs`, `MessageItemModel.bot_keyboard`).
  - Supports `callback_data`, `url`, `web_app` (launching Mini App without clipboard contamination, and restoring minimized floating PiP overlays via `launch_miniapp_from_uri`), and `switch_inline_query`.
- **Reply Keyboards (`ReplyKeyboardMarkup` / `ReplyKeyboardRemove` / `ForceReply`)**:
  - **UI & Composer Wiring (M2)**: `BotKeyboard` (`views/composer/bot_keyboard.rs`) supports `SINGLE_USE`, `RESIZE`, `PERSISTENT`, and `FORCE_REPLY` flags, `clear()`, and palette-aware interactive `render_reply_keyboard`. `CanonicalComposer` owns `bot_keyboard`, `is_bot_keyboard_open`, and `pending_bot_keyboard_press`, exposes `#cmp-bot-kb-toggle` inside `#cmp-input-wrap` whenever `bot_keyboard.has_markup()`, and `AppShell` (`shell_observers.rs`) dispatches button presses (`Default` -> send text, `WebApp` -> launch Mini App, `Url` -> open URL/Mini App, `SwitchInline` -> prefill query) and auto-hides `single_use` keyboards. `bot_keyboard` is cleared on chat switch and `reset_for_compose_picker`.
  - **Documented Protocol Gap**: `gpui_telegram_proto::MessageItem` (`crates/gpui_telegram_proto/src/message/item.rs`) and `gpui_telegram_server/src/routes/telegram_bot_api.rs` (`parse_inline_keyboard_markup`) currently deserialize only `inline_keyboard` and do not yet carry `reply_markup` (`keyboard` / `remove_keyboard` / `force_reply`) fields on wire messages. Extending the proto/server schema in a protocol-scoped follow-up only requires calling `composer.set_bot_reply_keyboard(...)` or `composer.clear_bot_reply_keyboard()` from chat sync / PTS update handlers.

---

## 3. Input Bar, `MenuButton` & Mini App Entry Specification

### 3.1 `MenuButton` Strictly Outside Text Input Capsule (`R2-3`, `R1`)
Per `core.telegram.org/bots/api#menubutton` and `tdesktop` (`HistoryView::BotMenuButton` + `HistoryView::ComposeControls`), a bot's `MenuButton` (`MenuButtonWebApp` or `MenuButtonCommands`) MUST sit **outside** the text input capsule, to its left, leaving the text capsule solely for attachment `+`, text entry, `Aa`, optional reply-keyboard toggle, emoji, and mic/send.

```
Apple-Style Bottom Composer Row (max-w 740px, items-end, gap 8px)
+-----------------------------------------------------------------------------------+
| [☰ Open App] [/]   ( +   Write a message...                     [⌘]  Aa  ☺  🎙  ↑ ) |
| ^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ |
| Left External      #cmp-input-wrap (.flex_1, min-w 0, min-h 46px, rounded 23px)   |
| Wrapper (h 46px)   ONLY: #cmp-attach-btn, #cmp-text-scroll, [#cmp-bot-kb-toggle], |
| #composer-bot-     #cmp-fmt-toggle, #cmp-emoji-toggle, #cmp-mic-btn, #cmp-send-btn|
| menu-btn + / btn                                                                  |
+-----------------------------------------------------------------------------------+

Classic TDesktop Bottom Composer Row
+-----------------------------------------------------------------------------------+
| [☰ Open App] [/]  📎  [ Write a message...                 [⌘] Aa ☺ ]     [🎙/➤] |
+-----------------------------------------------------------------------------------+
```

- **Apple-Style Layout (`views/composer/composer_ui.rs`)**:
  - Both optional `bot_menu_view` and `#cmp-input-wrap` share a centered `div().w_full().max_w(px(740.0)).flex().items_end().gap(px(8.0))` container.
  - `bot_menu_view` sits in a `h(px(palette.scaled_px(46.0))).flex().items_center().flex_shrink_0()` left sibling wrapper, guaranteeing `menu_btn.right <= input_wrap.left`.
  - `#cmp-input-wrap` uses `.flex_1().min_w(px(0.0))` so the capsule shrinks gracefully beside external bot controls at all viewport widths.
- **Coexistence of `MenuButtonWebApp` and `/` Commands (`canonical_composer.rs`, `composer_event_handlers.rs`)**:
  - When a bot has **both** an active WebApp menu (`bot_menu_controls.is_active`) and slash commands (`!bot_slash_commands.is_empty() || hermes_slash_enabled`), `CanonicalComposer::render` renders **both** `#composer-bot-menu-btn` and `#composer-bot-slash-btn` side-by-side (`gap(px(6.0))`) outside `#cmp-input-wrap`.
  - Per-bot `/` command isolation is preserved: when `bot_slash_commands` is non-empty, `/` autocomplete and `toggle_bot_commands_popup()` use `bot_slash_commands`; only when `bot_slash_commands` is empty and `hermes_slash_enabled == true` (viewer-owned Fabushi bots) does `/` show `hermes_commands` (`hermes_ac_pinned_by_button`). Standard bots with 0 commands show neither popup nor `/` button.
- **Compact Typing Shrink (`is_small`, `history_view_bot_menu_button.rs`)**:
  - `CanonicalComposer::sync_bot_menu_compact_state` tracks `!text.is_empty() || preedit_non_empty`. When typing begins, `HistoryViewBotMenuButtonViewControls::refresh(..., true)` collapses `[☰ Open App]` into a `36x36` circular `"☰"` pill (`width_px = 36.0`) to maximize horizontal typing space, expanding smoothly back when the input becomes empty.

### 3.2 Attachment Menu (`AttachMenuBot`) & Main Mini App Launch
- **Attachment Menu (`AttachMenuBot`)**: `CanonicalComposer::set_attach_mini_apps(apps, peer_type_mask)` filters installed attachment-menu Mini Apps by `AttachMenuPeerType` (`USER` / `BOT` / `GROUP` / `BROADCAST`) inside `AttachmentsDrawer` (`popups_ui.rs`).
- **Main Mini App (`messages.requestMainWebView`)**: Bots with `has_main_app` launch via `#composer-bot-menu-btn`, Bot Profile `"Open App"`, or dialog quick actions, displaying first-time third-party Mini App ToS confirmation sheets (`pending_miniapp_confirm` in `shell_overlays_render.rs`) and hosting the app inside `BotAttachWebView` + `MacOsWkWebView` (with WebMCP iframe bridge in M3).

---

## 4. Right-Hand Inspector Profile Separation & Zero Cross-Chat Leakage

*(Owned by Milestone M1 across `context_inspector.rs`, `shell_truth.rs`, `shell_stage_actions.rs`, `drawer/inspector_panels.rs`, `drawer/bot_computer_inspector.rs`, and `boxes/peers/bot_profile_box.rs`.)*

| Field / Section | Bot (`is_bot == true`) | Group / Supergroup / Channel (`is_group_or_channel`) | 1v1 User (`!is_bot && !is_group_or_channel`) |
|---|---|---|---|
| **Header & Subtitle** | Bot name + Verified badge + `"{MAU} monthly users"` / `"bot"` | Group title + Type (`"Group"` / `"Supergroup"` / `"Channel"`) + `"N members"` | User name + Online / Last-seen status |
| **Identity Rows** | `@username` (copyable), About / Bot Description | Group Description / About (ONLY if non-empty for *this* group), Primary `invite_link` | Mobile Phone (copyable), `@username` (copyable), User Bio (ONLY if non-empty) |
| **Bot / Group Actions** | `"Open App"` CTA, Privacy Policy / Terms, Quick Commands, Start New Session (owner Fabushi), Block/Stop Bot | Members Tab + Add Member + Role Badges, Edit Group, Leave Group, Auto-Delete | Start Secret Chat, Block Contact |
| **Forbidden Cross-Pollination** | Never Group Members or Phone | Never Bot `bio`, Bot commands, `"Open App"`, or 1v1 `"Block Contact"` / `"Secret Chat"` | Never Group Members tab or Bot commands / Mini App rows |

- **Synchronous Chat-Switch Reset (M1)**: `select_authoritative_dialog` (`shell_truth.rs`) invokes `ContextInspector::clear_peer_facts()` before populating the destination dialog, and `sync_selected_chat_inspector_and_stage` (`shell_stage_actions.rs`) checks `st.active_chat_id == Some(chat_id)` and unconditionally overwrites `bio` and `avatar_path` (even when empty).
- **Copy Feedback (`已复制`) Lifecycle (M1)**: `ContextInspector::show_copied_feedback` (and `BotProfileBox`) bumps a monotonic generation counter (`copied_feedback_gen`), spawns a `~2s` async auto-dismiss timer that clears `copied_feedback` iff generation matches, and clears `copied_feedback` immediately in `clear_peer_facts()` / inspector close.

---

## 5. `+` New Chat / Group / Channel Creation Flow & State Isolation

### 5.1 Apple Compose Stage (`render_apple_compose_stage`)
- Clicking `+` (`#sidebar-plus-action-btn` / `#rail-compose-btn` / `Cmd+N`) opens the inline `#apple-compose-stage` (`shell_overlays_render.rs`) in place of `ConversationStage`, keeping `stage.active_chat_id` untouched so canceling (`#apple-compose-close-btn` or `Esc`) returns seamlessly without creating synthetic `"新信息"` sidebar rows.
- Multi-select recipient chips auto-generate group titles (`2+` chips) or allow direct DM commit (`#apple-compose-open-dm-btn` / `Enter` on `1` chip via `commit_single_compose_recipient_to_dm`), while group creation displays a transient `"Creating group…"` stand-in (`compose_draft.pending_creation`).

### 5.2 Composer Suppression & Isolation Contract (M2)
1. **Entry Isolation (`CanonicalComposer::reset_for_compose_picker`)**:
   - Entering `+` compose mode or any creation/contacts modal (`is_compose_or_creation_modal_active` across `shell.rs`, `shell_observers.rs`, and `shell_overlays_render.rs`) first flushes the background chat's unsent draft via `flush_pending_draft_for_chat(cid, cx)`, then calls `CanonicalComposer::reset_for_compose_picker(Some(cx))` which clears:
     - `bot_menu_controls` & `bot_menu_entity`
     - `bot_slash_commands`, `hermes_slash_enabled`, `bot_ac_*`, `hermes_ac_*`
     - `replying_to`, `editing_message`, `send_as_options`, `selected_send_as_peer_id`
     - `editor.attachments`, `submitted_attachment`, `pending_attached_files`, `undo_stack`, `redo_stack`
     - `autocomplete` mention candidates (`mention_rows`, `all_mention_candidates`, `mention_role_hints`)
     - `ime_machine` draft text (`clear()`), open drawers/popups, `bot_keyboard`, and `is_input_focused = false`
     - Sets `is_compose_isolated = true`.
2. **Draft & Send-As Mutation Guard (`shell_observers.rs`)**:
   - `observe(&self.composer)` guards `send_as_peers.set_chosen` and `handle_composer_draft_change(cx)` with `!is_compose_or_modal && !comp_isolated`, guaranteeing that clearing the composer or typing an opening message in `+` compose mode NEVER overwrites the previous chat's persisted draft.
3. **Render Suppression (`AppShell::should_hide_composer`, `shell.rs`)**:
   - `self.composer` is hidden from the layout tree whenever:
     - `is_apple_compose_mode && (!compose_has_recipients || compose_pending_creation)` (`0` recipients selected or creation in flight)
     - `creation_modal.is_some()` / `contacts_open` / `create_bot_modal_open` / `creation_dialog_open`
     - `!is_apple_compose_mode && stage.show_bot_start_bar`
   - As soon as `>= 1` recipient chip is selected in `+` mode (and creation is not in flight), `self.composer` mounts cleanly with `compose_placeholder_override` (`"发消息给 …"`) and zero residual bot buttons or state.
4. **Exit & Restoration (`exit_compose_or_modal_isolation`)**:
   - Canceling `+` (`×`, `Esc`, toggling `+`, closing creation modal, or clicking the *same* active chat in the sidebar) clears `is_compose_isolated = false`, restores the active chat's saved draft, `send_as_options`, and mention candidates, and re-runs `sync_compose_state_to_composer` + `sync_composer_and_inspector_miniapp_state` so the active chat's `MenuButton` and `/` commands restore cleanly.
   - Committing a single chip via `commit_single_compose_recipient_to_dm` carries over `carry_draft` without being clobbered by stale draft restoration.

---

## 6. Error Feedback, Toast Lifecycle & WebMCP Tool Execution UX

### 6.1 Two-Layer Toast & Banner Architecture
1. **Stage-Local Pill (`ConversationStage::toast_banner` / `#stage-toast-pill`, M1)**:
   - Ephemeral stage-local notifications inside `ConversationStage`'s transcript viewport; auto-dismissed and cleared synchronously in `ConversationStage::set_chat`.
2. **Window Toast Banner (`WindowController` / `AppShell` / `#window-toast-banner`, M2)**:
   - **`WindowController` (`window_controller.rs`)**: `ToastNotification` carries monotonic `id: u64`, real `created_at` epoch millis, `duration_ms`, and `#[serde(default)] pub chat_id: Option<u64>`. Exposes `show_toast` (backward-compatible), `show_chat_toast`, `dismiss_toast_if_id`, `stamp_current_toast_chat_id_if_unset`, `clear_toasts_for_chat`, and `clear_all_toasts`.
   - **Id-Fenced Auto-Dismiss & Chat-Switch Clear (`shell.rs`, `shell_observers.rs`)**:
     - `AppShell::schedule_current_toast_dismiss` fences on `scheduled_toast_id: Option<u64>` and spawns a GPUI background timer (`app.background_executor().timer(delay).await`) calling `dismiss_toast_if_id(target_id)` — guaranteeing manual dismissal or newer toasts are never prematurely closed by stale timers.
     - Untargeted toasts raised while viewing an active chat are automatically stamped with `Some(active_chat_id)` (while settings/auth/global toasts remain `chat_id: None`).
     - Switching chats (`stage.active_chat_id` transitions from `Some(old_cid)`) or entering `+` compose mode calls `clear_toasts_for_chat(old_cid)`, immediately clearing chat-scoped error banners while preserving global toasts.
   - **Non-Overlapping Stage-Bounded Layout (`shell.rs`)**:
     - The center column nests `div().id("app-shell-stage-slot").relative().flex_1().min_h(px(0.0)).w_full().flex().flex_col()` strictly **above** `.when(!hide_composer, |el| el.child(self.composer.clone()))`.
     - During normal chat view (`!has_blocking_overlay`), `#window-toast-banner` mounts inside `#app-shell-stage-slot` with `.bottom(px(bottom_gap)).left(px(16.0)).right(px(16.0))` (`bottom_gap = 16.0` scaled, or `68.0` scaled above the `52px` `#stage-bot-start-bar`). Because `#app-shell-stage-slot` shrinks automatically whenever `CanonicalComposer` grows (multiline input, reply/edit banners, attachments, or `BotKeyboard`), `toast.bottom <= composer.top - 16px` holds unconditionally across all viewport sizes.
     - `#window-toast-banner` enforces `.occlude()`, `.max_w(px(pal.scaled_px(480.0)))`, `.min_w(px(0.0))`, `.rounded(px(pal.scaled_px(14.0)))`, wrapping `.flex_1().min_w(px(0.0))` text, error border/badge styling for `PROVIDER_ERROR` / failures, and an explicit `#window-toast-banner-close` (`×`) button.

### 6.2 `PROVIDER_ERROR` Inline Recovery & `webmcp_call_tool` UX *(Milestone M3)*
- **Per-User Local Fabushi Provider Hydration & Inline `PROVIDER_ERROR` Card (M3)**:
  - `local_subscription_bridge.rs` hydrates both `core.providers()` and `core.providers_for_user(uid)`, and `shell_bot_browser.rs` renders `PROVIDER_ERROR` failures as structured inline chat error cards with an actionable `hermes:open_ai_providers` CTA rather than dumping raw multi-line provider tracebacks into bottom toasts.
- **WebMCP Cross-Frame Bridge & `webmcp_call_tool` Trace Guard (M3)**:
  - `bots_miniapps.rs`, `macos_wkwebview.rs`, and `bot_attach_web_view.rs` relay WebMCP `invoke` / `response` / `ready` / `state_update` across `<iframe id="tg-app-frame">` and gate readiness on inner iframe registration.
  - `hermes.rs` and `hermes_im_bridge.rs` format `webmcp_call_tool` steps concisely without raw internal JSON walls, surface actionable WebView-open guidance when unregistered, and prevent fabricated success text when critical WebMCP calls fail.

---

## 7. Implementation Parity Matrix & Verification Map

| Rule | Feature / Requirement | Authoritative Files & Symbols | Milestone | Automated Verification |
|---|---|---|---|---|
| **R1-A** | Bot `START` Bar (`What can this bot do?` + hide composer), streaming drafts, inline & reply keyboards (`BotKeyboard`) | `shell.rs` (`should_hide_composer`, `AppShell::render`), `conversation_stage.rs` (`#stage-bot-start-bar`, `#stage-bot-description-card`), `views/composer/bot_keyboard.rs` (`BotKeyboard`, `render_reply_keyboard`), `canonical_composer.rs` (`set_bot_reply_keyboard`, `clear_bot_reply_keyboard`, `#cmp-bot-kb-toggle`), `shell_observers.rs` | M2 | `grok_stage_agent_ux_tests.rs`, `miniapp_botfather_desktop_tests.rs` |
| **R1-B / R2-3** | Mini App `MenuButton` outside `#cmp-input-wrap`, `/` slash coexistence (`hermes_slash_enabled` + `bot_slash_commands`), `is_small` compact shrink | `composer_ui.rs` (`render_text_input_strip_with_full_options`), `history_view_bot_menu_button.rs` (`HistoryViewBotMenuButtonViewControls`), `canonical_composer.rs` (`sync_bot_menu_compact_state`, `toggle_bot_commands_popup`, `hermes_ac_pinned_by_button`), `composer_event_handlers.rs` | M2 | `miniapp_botfather_desktop_tests.rs` |
| **R2-1** | Group vs Bot vs 1v1 Profile separation & zero cross-chat leakage | `context_inspector.rs` (`clear_peer_facts`), `shell_truth.rs` (`select_authoritative_dialog`), `shell_stage_actions.rs` (`sync_selected_chat_inspector_and_stage`), `drawer/inspector_panels.rs`, `drawer/bot_computer_inspector.rs` | M1 | `desktop_views_tests.rs`, `grok_compose_group_flow_tests.rs` |
| **R2-2** | Copy feedback (`已复制`) auto-dismiss & chat-switch clear; stage toast isolation | `context_inspector.rs` (`show_copied_feedback`, `copied_feedback_gen`), `bot_profile_box.rs`, `conversation_stage.rs` (`toast_banner`, `set_chat`) | M1 | `desktop_views_tests.rs`, `grok_compose_group_flow_tests.rs` |
| **R2-4** | `+` New Chat / Creation Flow composer suppression, draft/modal isolation & restore | `canonical_composer.rs` (`reset_for_compose_picker`, `is_compose_isolated`), `shell.rs` (`is_compose_or_creation_modal_active`, `should_hide_composer`, `enter_compose_or_modal_isolation`, `exit_compose_or_modal_isolation`), `shell_observers.rs`, `shell_overlays_render.rs` | M2 | `grok_stage_agent_ux_tests.rs`, `miniapp_botfather_desktop_tests.rs` |
| **R2-5 (Part A)** | Window toast id-fenced auto-dismiss, chat-switch clear, & non-overlapping `#window-toast-banner` layout | `window_controller.rs` (`ToastNotification.chat_id`, `show_chat_toast`, `dismiss_toast_if_id`, `clear_toasts_for_chat`), `shell.rs` (`schedule_current_toast_dismiss`, `render_toast_banner_slot`, `#app-shell-stage-slot`, `#window-toast-banner-close`), `shell_observers.rs` | M2 | `grok_stage_agent_ux_tests.rs`, `miniapp_botfather_desktop_tests.rs`, `adversarial_layout_window_tests.rs`, `window_themes_subsystem_tests.rs` |
| **R2-5 (Part B)** | Per-user local Fabushi hydration, inline `PROVIDER_ERROR` card, WebMCP iframe bridge & `webmcp_call_tool` trace guard | `local_subscription_bridge.rs`, `shell_bot_browser.rs`, `bots_miniapps.rs`, `macos_wkwebview.rs`, `bot_attach_web_view.rs`, `hermes.rs`, `hermes_im_bridge.rs` | M3 | `hermes_desktop_bugfix_tests.rs`, `hermes_gateway_and_bot_bugfix_tests.rs`, `fabushi_miniapp_e2e_tests.rs` |
| **E2E** | End-to-End Opaque-Box Verification (Tiers 1–4 & Live Desktop) | `grok_bot_alignment_e2e_tests.rs`, `tests/e2e/test_grok_bot_alignment_live_e2e.py` | E2E_TRACK / M4 | Full workspace suite + live desktop E2E |
