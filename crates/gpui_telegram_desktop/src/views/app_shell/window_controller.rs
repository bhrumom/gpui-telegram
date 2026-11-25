//! WindowController: Master window lifecycle, layer management, and modal coordination
//! Upstream: Telegram/SourceFiles/window/window_controller.h, window_controller.cpp

use gpui::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WindowId {
    Primary,
    Chat(u64),
    Media(u64),
    Custom(u64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastNotification {
    pub id: u64,
    pub text: String,
    pub duration_ms: u64,
    pub created_at: u64,
    #[serde(default)]
    pub chat_id: Option<u64>,
    #[serde(default)]
    pub allow_chat_auto_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerKind {
    BoxModal(String),
    Settings,
    ThemeEditor,
    PasscodeLock,
    SetupEmail,
    TermsLock,
    MediaPreview,
}

#[derive(Debug, Clone)]
pub struct WindowController {
    pub id: WindowId,
    pub title: String,
    pub is_visible: bool,
    pub is_active: bool,
    pub is_locked: bool,
    pub is_email_lock: bool,
    pub active_layers: Vec<LayerKind>,
    pub toast_queue: VecDeque<ToastNotification>,
    pub current_toast: Option<ToastNotification>,
    pub highlight_control_id: Option<String>,
    pub right_column_visible: bool,
    next_toast_id: u64,
}

impl WindowController {
    pub fn new() -> Self {
        Self::with_id(WindowId::Primary)
    }

    pub fn with_id(id: WindowId) -> Self {
        Self {
            id,
            title: "Telegram Desktop".to_string(),
            is_visible: true,
            is_active: true,
            is_locked: false,
            is_email_lock: false,
            active_layers: Vec::new(),
            toast_queue: VecDeque::new(),
            current_toast: None,
            highlight_control_id: None,
            right_column_visible: false,
            next_toast_id: 1,
        }
    }

    pub fn is_primary(&self) -> bool {
        matches!(self.id, WindowId::Primary)
    }

    pub fn locked(&self) -> bool {
        self.is_locked || self.is_email_lock
    }

    pub fn setup_passcode_lock(&mut self) {
        self.is_locked = true;
        if !self.active_layers.contains(&LayerKind::PasscodeLock) {
            self.active_layers.push(LayerKind::PasscodeLock);
        }
    }

    pub fn clear_passcode_lock(&mut self) {
        self.is_locked = false;
        self.active_layers.retain(|l| l != &LayerKind::PasscodeLock);
    }

    pub fn setup_setup_email_lock(&mut self) {
        self.is_email_lock = true;
        if !self.active_layers.contains(&LayerKind::SetupEmail) {
            self.active_layers.push(LayerKind::SetupEmail);
        }
    }

    pub fn clear_setup_email_lock(&mut self) {
        self.is_email_lock = false;
        self.active_layers.retain(|l| l != &LayerKind::SetupEmail);
    }

    pub fn show_box(&mut self, title: impl Into<String>) {
        self.active_layers.push(LayerKind::BoxModal(title.into()));
    }

    pub fn show_layer(&mut self, kind: LayerKind) {
        self.active_layers.push(kind);
    }

    pub fn hide_layer(&mut self) -> Option<LayerKind> {
        self.active_layers.pop()
    }

    pub fn hide_settings_and_layer(&mut self) {
        self.active_layers.retain(|l| {
            !matches!(l, LayerKind::Settings | LayerKind::ThemeEditor | LayerKind::BoxModal(_))
        });
    }

    pub fn close_layer_by_back_button(&mut self) -> bool {
        if self.is_locked {
            return false;
        }
        self.active_layers.pop().is_some()
    }

    pub fn is_layer_shown(&self) -> bool {
        !self.active_layers.is_empty()
    }

    pub fn show_toast(&mut self, text: impl Into<String>, duration_ms: u64) {
        let _ = self.enqueue_toast_with_chat(None, text, duration_ms);
    }

    pub fn show_non_chat_toast(&mut self, text: impl Into<String>, duration_ms: u64) {
        let id = self.next_toast_id;
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let toast = ToastNotification {
            id,
            text: text.into(),
            duration_ms: if duration_ms == 0 { 3000 } else { duration_ms },
            created_at: now_ms,
            chat_id: None,
            allow_chat_auto_scope: false,
        };
        self.next_toast_id += 1;
        if self.current_toast.is_none() {
            self.current_toast = Some(toast);
        } else {
            self.toast_queue.push_back(toast);
        }
    }

    pub fn show_chat_toast(&mut self, chat_id: u64, text: impl Into<String>, duration_ms: u64) -> u64 {
        self.enqueue_toast_with_chat(Some(chat_id), text, duration_ms)
    }

    fn enqueue_toast_with_chat(
        &mut self,
        chat_id: Option<u64>,
        text: impl Into<String>,
        duration_ms: u64,
    ) -> u64 {
        let id = self.next_toast_id;
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let toast = ToastNotification {
            id,
            text: text.into(),
            duration_ms: if duration_ms == 0 { 3000 } else { duration_ms },
            created_at: now_ms,
            chat_id,
            allow_chat_auto_scope: chat_id.is_none(),
        };
        self.next_toast_id += 1;
        if self.current_toast.is_none() {
            self.current_toast = Some(toast);
        } else {
            self.toast_queue.push_back(toast);
        }
        id
    }

    pub fn dismiss_current_toast(&mut self) {
        self.current_toast = self.toast_queue.pop_front();
    }

    pub fn dismiss_toast_if_id(&mut self, toast_id: u64) -> bool {
        if self.current_toast.as_ref().is_some_and(|t| t.id == toast_id) {
            self.current_toast = self.toast_queue.pop_front();
            true
        } else {
            false
        }
    }

    pub fn stamp_current_toast_chat_id_if_unset(&mut self, chat_id: u64) {
        if let Some(ref mut toast) = self.current_toast {
            if toast.chat_id.is_none() && toast.allow_chat_auto_scope {
                toast.chat_id = Some(chat_id);
                toast.allow_chat_auto_scope = false;
            }
        }
    }

    pub fn clear_toasts_for_chat(&mut self, chat_id: u64) -> bool {
        let queue_before = self.toast_queue.len();
        self.toast_queue.retain(|t| t.chat_id != Some(chat_id));
        for t in &mut self.toast_queue {
            if t.chat_id.is_none() {
                t.allow_chat_auto_scope = false;
            }
        }
        let mut changed = self.toast_queue.len() != queue_before;
        if self
            .current_toast
            .as_ref()
            .is_some_and(|t| t.chat_id == Some(chat_id))
        {
            self.current_toast = self.toast_queue.pop_front();
            changed = true;
        }
        if let Some(ref mut cur) = self.current_toast {
            if cur.chat_id.is_none() {
                cur.allow_chat_auto_scope = false;
            }
        }
        changed
    }

    pub fn clear_all_toasts(&mut self) {
        self.current_toast = None;
        self.toast_queue.clear();
    }

    pub fn show_right_column(&mut self) {
        self.right_column_visible = true;
    }

    pub fn hide_right_column(&mut self) {
        self.right_column_visible = false;
    }

    pub fn toggle_right_column(&mut self) {
        self.right_column_visible = !self.right_column_visible;
    }

    pub fn activate(&mut self) {
        self.is_active = true;
        self.is_visible = true;
    }

    pub fn minimize(&mut self) {
        self.is_visible = false;
        self.is_active = false;
    }

    pub fn close(&mut self) {
        self.is_visible = false;
        self.is_active = false;
    }

    pub fn set_highlight_control_id(&mut self, id: impl Into<String>) {
        self.highlight_control_id = Some(id.into());
    }

    pub fn take_highlight_control_id(&mut self, id: &str) -> bool {
        if self.highlight_control_id.as_deref() == Some(id) {
            self.highlight_control_id = None;
            true
        } else {
            false
        }
    }

    pub fn check_lock_by_terms(&self) -> bool {
        false
    }

    pub fn finish_first_show(&mut self) {
        self.is_visible = true;
        self.is_active = true;
    }
}

impl Default for WindowController {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for WindowController {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let is_modal_active = !self.active_layers.is_empty();
        let top_label = match self.active_layers.last() {
            Some(LayerKind::PasscodeLock) => Some("Passcode Locked"),
            Some(LayerKind::SetupEmail) => Some("Confirm Setup Email"),
            Some(LayerKind::TermsLock) => Some("Terms of Service"),
            Some(LayerKind::Settings) => Some("Settings"),
            Some(LayerKind::ThemeEditor) => Some("Theme Editor"),
            Some(LayerKind::MediaPreview) => Some("Media Viewer"),
            Some(LayerKind::BoxModal(title)) => Some(title.as_str()),
            None => None,
        };

        div()
            .relative()
            .size_full()
            .children(is_modal_active.then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x000000A8))
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(top_label.map(|label| {
                        div()
                            .bg(rgb(0x18222D))
                            .p(px(20.0))
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(rgb(0x2B5278))
                            .text_color(rgb(0xFFFFFF))
                            .child(label.to_string())
                    }))
            }))
            .children(self.current_toast.as_ref().map(|t| {
                div()
                    .absolute()
                    .bottom(px(32.0))
                    .left_1_2()
                    .bg(rgb(0x282E33))
                    .text_color(rgb(0xFFFFFF))
                    .px(px(16.0))
                    .py(px(8.0))
                    .rounded(px(16.0))
                    .shadow_lg()
                    .text_xs()
                    .child(t.text.clone())
            }))
    }
}
