//! IME Safety State Machine for Telegram Multi-line Composer (`SPEC-02-GPUI-COMPONENT-HIERARCHY` § 2.4).
//! Prevents premature message submission when composing with Chinese/Japanese/Korean Input Method Editors.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImeState {
    /// Idle: No active IME preedit composition. Enter key sends message (or Shift+Enter newlines).
    Idle,
    /// Composing: User is actively typing IME phonetic/radical keys (e.g. Pinyin or Romaji).
    /// Enter key selects/confirms current preedit candidate and MUST NOT submit the message.
    Composing {
        preedit_text: String,
        cursor_pos: usize,
    },
    /// Committed: The selected IME candidate has been inserted into the buffer.
    Committed {
        committed_text: String,
    },
}

impl Default for ImeState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Send the composed message buffer
    SendMessage,
    /// Insert a newline into multi-line buffer
    InsertNewline,
    /// Confirm IME composition candidate without sending
    ImeCandidateChosen,
    /// Normal character insertion / edit
    DefaultEdit,
    /// Ignored
    None,
}

/// Trailing macOS Return keydowns emitted in the same tick as an IME commit arrive within a few ms.
const IME_COMMIT_FENCE_WINDOW_MS: u64 = 80;

pub struct ImeSafetyMachine {
    pub state: ImeState,
    pub text_buffer: String,
    pub cursor_index: usize,
    /// Fence flag preventing trailing Cocoa Return keydown from dispatching a message
    pub just_committed: bool,
    pub last_commit_at: Option<std::time::Instant>,
}

impl ImeSafetyMachine {
    pub fn new() -> Self {
        Self {
            state: ImeState::Idle,
            text_buffer: String::new(),
            cursor_index: 0,
            just_committed: false,
            last_commit_at: None,
        }
    }

    pub fn with_text(text: impl Into<String>) -> Self {
        let text_buffer = text.into();
        let cursor_index = text_buffer.len();
        Self {
            state: ImeState::Idle,
            text_buffer,
            cursor_index,
            just_committed: false,
            last_commit_at: None,
        }
    }

    /// Called when OS IME begins composition
    pub fn on_ime_start(&mut self) {
        self.just_committed = false;
        self.last_commit_at = None;
        self.state = ImeState::Composing {
            preedit_text: String::new(),
            cursor_pos: 0,
        };
    }

    /// Called as user types phonetic/radical keystrokes in IME preedit window
    pub fn on_ime_update(&mut self, preedit: &str, cursor_pos: usize) {
        self.just_committed = false;
        self.last_commit_at = None;
        self.state = ImeState::Composing {
            preedit_text: preedit.to_string(),
            cursor_pos,
        };
    }

    /// Called when user commits a candidate from IME into the real text buffer
    pub fn on_ime_commit(&mut self, committed: &str) {
        self.insert_text(committed);
        self.state = ImeState::Committed {
            committed_text: committed.to_string(),
        };
        // Set fence flag so trailing Return keydown within this tick is absorbed
        self.just_committed = true;
        self.last_commit_at = Some(std::time::Instant::now());
    }

    /// Called when IME composition is canceled without committing
    pub fn on_ime_cancel(&mut self) {
        self.just_committed = false;
        self.last_commit_at = None;
        self.state = ImeState::Idle;
    }

    /// Reset commit fence when user types non-IME characters or explicit resets
    pub fn reset_commit_fence(&mut self) {
        self.just_committed = false;
        self.last_commit_at = None;
        if matches!(self.state, ImeState::Committed { .. }) {
            self.state = ImeState::Idle;
        }
    }

    /// Process Enter key press with IME safety guard
    pub fn handle_enter_key(&mut self, is_shift: bool) -> KeyAction {
        let within_fence_window = self
            .last_commit_at
            .is_none_or(|t| t.elapsed() < std::time::Duration::from_millis(IME_COMMIT_FENCE_WINDOW_MS));

        // If just committed via IME within the same tick, absorb the trailing Return keydown.
        // If > 80ms have elapsed (e.g. user committed Chinese text via Space and later pressed Enter),
        // treat Enter as an intentional send/newline keypress.
        if self.just_committed {
            self.just_committed = false;
            self.last_commit_at = None;
            self.state = ImeState::Idle;
            if within_fence_window {
                return KeyAction::ImeCandidateChosen;
            }
        }

        match &self.state {
            ImeState::Composing { preedit_text, .. } => {
                // If IME is composing, Enter selects the preedit text
                let committed = preedit_text.clone();
                self.on_ime_commit(&committed);
                KeyAction::ImeCandidateChosen
            }
            ImeState::Committed { .. } => {
                self.state = ImeState::Idle;
                self.just_committed = false;
                self.last_commit_at = None;
                if within_fence_window {
                    KeyAction::ImeCandidateChosen
                } else if is_shift {
                    self.insert_text("\n");
                    KeyAction::InsertNewline
                } else if !self.text_buffer.trim().is_empty() {
                    KeyAction::SendMessage
                } else {
                    KeyAction::None
                }
            }
            ImeState::Idle => {
                if is_shift {
                    self.insert_text("\n");
                    KeyAction::InsertNewline
                } else if !self.text_buffer.trim().is_empty() {
                    KeyAction::SendMessage
                } else {
                    KeyAction::None
                }
            }
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        if !self.is_composing() {
            self.just_committed = false;
            self.last_commit_at = None;
            if matches!(self.state, ImeState::Committed { .. }) {
                self.state = ImeState::Idle;
            }
        }
        let cur = super::editor_state::clamp_char_boundary(&self.text_buffer, self.cursor_index);
        if cur >= self.text_buffer.len() {
            self.text_buffer.push_str(text);
            self.cursor_index = self.text_buffer.len();
        } else {
            self.text_buffer.insert_str(cur, text);
            self.cursor_index = cur + text.len();
        }
    }

    pub fn backspace(&mut self) {
        let cur = super::editor_state::clamp_char_boundary(&self.text_buffer, self.cursor_index);
        self.cursor_index = cur;
        if cur == 0 || self.text_buffer.is_empty() {
            return;
        }
        if let Some((start, end)) = super::editor_state::atomic_reference_chip_span_before_cursor(
            &self.text_buffer,
            cur,
        ) {
            self.text_buffer.replace_range(start..end, "");
            self.cursor_index = start;
            return;
        }
        let prev = super::editor_state::prev_char_boundary(&self.text_buffer, cur);
        self.text_buffer.replace_range(prev..cur, "");
        self.cursor_index = prev;
    }

    pub fn clear(&mut self) {
        self.text_buffer.clear();
        self.cursor_index = 0;
        self.state = ImeState::Idle;
        self.just_committed = false;
        self.last_commit_at = None;
    }

    pub fn text(&self) -> &str {
        &self.text_buffer
    }

    pub fn is_composing(&self) -> bool {
        matches!(self.state, ImeState::Composing { .. })
    }

    pub fn is_composing_or_committed(&self) -> bool {
        self.just_committed || matches!(self.state, ImeState::Composing { .. } | ImeState::Committed { .. })
    }

    pub fn preedit_text(&self) -> Option<&str> {
        match &self.state {
            ImeState::Composing { preedit_text, .. } => Some(preedit_text.as_str()),
            _ => None,
        }
    }
}

impl Default for ImeSafetyMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_insert_and_backspace_guard_oob_and_mid_utf8_cursor() {
        let mut ime = ImeSafetyMachine::new();
        ime.insert_text("你好🚀");
        // Mid-UTF-8 byte offset inside '你' (3 bytes) and '🚀' (4 bytes)
        ime.cursor_index = 1;
        ime.insert_text("X");
        assert_eq!(ime.text(), "X你好🚀");

        // OOB cursor > text_buffer.len() must not panic in backspace
        ime.cursor_index = 999;
        ime.backspace();
        assert_eq!(ime.text(), "X你好");

        // Mid-UTF-8 byte offset in backspace removes the preceding char cleanly
        ime.cursor_index = 2; // inside '你' (bytes 1..4), clamps to 1
        ime.backspace();
        assert_eq!(ime.text(), "你好");
    }
}
