//! Windows System Tray Icon & Shell Notify Bridge (`Shell_NotifyIconW` + Popup Menu)
//! Upstream: Telegram/SourceFiles/platform/win/tray_win.h, Telegram/SourceFiles/platform/win/tray_win.cpp
//! Subsystem: platform | Feature ID: TCG-N46 | Disposition: rust_domain_or_server_1to1

use crate::platform::mac::tray_mac::tray_action_ids;
use gpui::*;
use gpui_telegram_core::error::Result;
use serde::{Deserialize, Serialize};

/// Action entry on Windows notification area tray menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinTrayAction {
    pub id: String,
    pub title: String,
    pub is_separator: bool,
    pub is_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct TrayWinWin {
    pub is_registered: bool,
    pub unread_counter: u32,
    pub tooltip: String,
    pub icon_clicks_count: usize,
    pub show_requests_count: usize,
    pub is_supported: bool,
    pub is_enabled: bool,
    pub platform_name: String,
    pub actions: Vec<WinTrayAction>,
    pub last_action_triggered: Option<String>,
    pub is_online: bool,
    pub controlled_count: usize,
    pub controlled_by_ai: bool,
    pub allow_remote_control: bool,
    pub assist_id: String,
    pub assist_code: Option<String>,
    pub language_id: String,
}

fn format_assist_id(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 9 {
        format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9])
    } else if !raw.trim().is_empty() {
        raw.trim().to_string()
    } else {
        "--- --- ---".to_string()
    }
}

impl TrayWinWin {
    pub fn new() -> Self {
        Self {
            is_registered: true,
            unread_counter: 0,
            tooltip: "Telegram Desktop".into(),
            icon_clicks_count: 0,
            show_requests_count: 0,
            is_supported: true,
            is_enabled: true,
            platform_name: "WindowsTrayNotify".into(),
            actions: Vec::new(),
            last_action_triggered: None,
            is_online: true,
            controlled_count: 0,
            controlled_by_ai: false,
            allow_remote_control: true,
            assist_id: String::new(),
            assist_code: None,
            language_id: "en".to_string(),
        }
    }

    /// Upstream method: `Window::LogoNoMargin`
    pub fn logo_no_margin(&mut self) -> Result<()> {
        Ok(())
    }

    /// Upstream method: `Tray::Tray`
    pub fn tray(&mut self) -> Result<()> {
        self.is_registered = true;
        #[cfg(windows)]
        win_native::sync_notify_icon(&self.tooltip, self.is_registered);
        Ok(())
    }

    /// Upstream method: `rpl::iconClicks`
    pub fn icon_clicks(&mut self) -> Result<()> {
        self.icon_clicks_count += 1;
        Ok(())
    }

    /// Upstream method: `rpl::showFromTrayRequests`
    pub fn show_from_tray_requests(&mut self) -> Result<()> {
        self.show_requests_count += 1;
        Ok(())
    }

    /// Upstream method: `createIcon`
    pub fn create_icon(&mut self) -> Result<()> {
        self.is_registered = true;
        #[cfg(windows)]
        win_native::sync_notify_icon(&self.tooltip, true);
        Ok(())
    }

    /// Upstream method: `Core::TrayIconToolTip`
    pub fn tray_icon_tool_tip(&mut self) -> Result<()> {
        if self.controlled_count > 0 {
            self.tooltip = if self.language_id.starts_with("zh") {
                format!("Fabushi - 本机正在被 {} 台远程会话控制", self.controlled_count)
            } else {
                format!(
                    "Fabushi - Controlled by {} active session(s)",
                    self.controlled_count
                )
            };
        } else if self.unread_counter > 0 {
            self.tooltip = format!("Telegram Desktop ({} unread)", self.unread_counter);
        } else {
            self.tooltip = "Telegram Desktop".into();
        }
        #[cfg(windows)]
        win_native::sync_notify_icon(&self.tooltip, self.is_registered);
        Ok(())
    }

    pub fn set_unread_counter(&mut self, counter: u32) {
        self.unread_counter = counter;
        let _ = self.tray_icon_tool_tip();
    }

    pub fn add_action(&mut self, id: impl Into<String>, title: impl Into<String>) {
        self.actions.push(WinTrayAction {
            id: id.into(),
            title: title.into(),
            is_separator: false,
            is_enabled: true,
        });
    }

    pub fn add_separator(&mut self) {
        self.actions.push(WinTrayAction {
            id: String::new(),
            title: String::new(),
            is_separator: true,
            is_enabled: false,
        });
    }

    pub fn trigger_action(&mut self, id: &str) -> bool {
        if let Some(act) = self.actions.iter().find(|a| a.id == id && a.is_enabled) {
            self.last_action_triggered = Some(act.id.clone());
            true
        } else {
            false
        }
    }

    /// Rebuilds the Windows notification-area context menu for Fabushi remote control.
    pub fn configure_fabushi_remote_menu(&mut self, lang: &str) {
        self.language_id = lang.to_string();
        let is_zh = lang.starts_with("zh");
        self.actions.clear();

        let status_title = if self.controlled_count > 0 {
            if self.controlled_by_ai {
                if is_zh {
                    format!("● 本机被控中 · AI Agent ({})", self.controlled_count)
                } else {
                    format!("● Controlled · AI Agent ({})", self.controlled_count)
                }
            } else if is_zh {
                format!("● 本机被控中 · {} 台设备", self.controlled_count)
            } else {
                format!("● Controlled · {} device(s)", self.controlled_count)
            }
        } else if self.is_online {
            if is_zh {
                "● 在线 · 远控服务就绪".to_string()
            } else {
                "● Online · Remote Ready".to_string()
            }
        } else if is_zh {
            "○ 离线".to_string()
        } else {
            "○ Offline".to_string()
        };

        self.actions.push(WinTrayAction {
            id: "status_indicator".to_string(),
            title: status_title,
            is_separator: false,
            is_enabled: false,
        });
        self.add_separator();
        self.add_action(
            tray_action_ids::SHOW_FABUSHI,
            if is_zh {
                "显示 Fabushi 主窗口"
            } else {
                "Show Fabushi"
            },
        );
        self.add_action(
            tray_action_ids::CONTROLLED_STATUS,
            if is_zh {
                "本机被控状态窗口"
            } else {
                "Controlled Status Window"
            },
        );
        self.actions.push(WinTrayAction {
            id: tray_action_ids::DISCONNECT_ALL.to_string(),
            title: if is_zh {
                "断开所有远控".to_string()
            } else {
                "Disconnect All Remote Control".to_string()
            },
            is_separator: false,
            is_enabled: self.controlled_count > 0,
        });
        self.add_separator();
        self.add_action(
            tray_action_ids::TOGGLE_ALLOW_REMOTE,
            if is_zh {
                if self.allow_remote_control {
                    "✓ 允许远程控制本机"
                } else {
                    "  允许远程控制本机"
                }
            } else if self.allow_remote_control {
                "✓ Allow Remote Control"
            } else {
                "  Allow Remote Control"
            },
        );
        let formatted_id = format_assist_id(&self.assist_id);
        self.actions.push(WinTrayAction {
            id: tray_action_ids::COPY_ASSIST.to_string(),
            title: if is_zh {
                format!("复制协助码 ({formatted_id})")
            } else {
                format!("Copy Assist ID & Code ({formatted_id})")
            },
            is_separator: false,
            is_enabled: !self.assist_id.trim().is_empty(),
        });
        self.add_separator();
        self.add_action(
            tray_action_ids::QUIT,
            if is_zh {
                "退出 Fabushi"
            } else {
                "Quit Fabushi"
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_remote_control_status(
        &mut self,
        is_online: bool,
        controlled_count: usize,
        controlled_by_ai: bool,
        allow_remote_control: bool,
        assist_id: impl Into<String>,
        assist_code: Option<String>,
        lang: &str,
    ) {
        self.is_online = is_online;
        self.controlled_count = controlled_count;
        self.controlled_by_ai = controlled_by_ai;
        self.allow_remote_control = allow_remote_control;
        self.assist_id = assist_id.into();
        self.assist_code = assist_code;
        self.configure_fabushi_remote_menu(lang);
        let _ = self.tray_icon_tool_tip();
    }

    pub fn poll_native_action(&mut self) -> Option<String> {
        #[cfg(windows)]
        {
            if let Some(id) = win_native::pop_pending_action() {
                self.last_action_triggered = Some(id.clone());
                return Some(id);
            }
        }
        None
    }
}

impl Default for TrayWinWin {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for TrayWinWin {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .p(px(6.0))
            .gap(px(6.0))
            .bg(rgb(0xFFFFFF))
            .border_1()
            .border_color(rgb(0xE0E0E0))
            .rounded_md()
            .child(
                div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .rounded_md()
                    .bg(rgb(0x2AABEE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xFFFFFF))
                    .text_size(px(10.0))
                    .child("TG"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .child(format!(
                        "WinTray: Unread={}, Ctrl={}, Clicks={}",
                        self.unread_counter, self.controlled_count, self.icon_clicks_count
                    )),
            )
    }
}

#[cfg(windows)]
mod win_native {
    use parking_lot::Mutex;
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};

    static REGISTERED: AtomicBool = AtomicBool::new(false);
    static PENDING_ACTIONS: Mutex<Option<VecDeque<String>>> = Mutex::new(None);

    const NIM_ADD: u32 = 0x0000_0000;
    const NIM_MODIFY: u32 = 0x0000_0001;
    const NIM_DELETE: u32 = 0x0000_0002;
    const NIF_MESSAGE: u32 = 0x0000_0001;
    const NIF_TIP: u32 = 0x0000_0004;
    const WM_APP: u32 = 0x8000;
    const TRAY_UID: u32 = 0x4642_5348;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    struct NotifyIconDataW {
        cb_size: u32,
        hwnd: *mut c_void,
        uid: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: *mut c_void,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_version_or_timeout: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: Guid,
        h_balloon_icon: *mut c_void,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn Shell_NotifyIconW(dw_message: u32, lp_data: *mut NotifyIconDataW) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetActiveWindow() -> *mut c_void;
    }

    fn native_allowed() -> bool {
        !cfg!(test) && std::env::var_os("GPUI_OFFLINE_TEST").is_none()
    }

    pub fn sync_notify_icon(tooltip: &str, visible: bool) {
        if !native_allowed() {
            return;
        }
        unsafe {
            let hwnd = GetActiveWindow();
            if hwnd.is_null() {
                return;
            }
            let mut tip_buf = [0u16; 128];
            for (idx, code_unit) in tooltip.encode_utf16().take(127).enumerate() {
                tip_buf[idx] = code_unit;
            }
            let mut nid = NotifyIconDataW {
                cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
                hwnd,
                uid: TRAY_UID,
                u_flags: NIF_MESSAGE | NIF_TIP,
                u_callback_message: WM_APP + 42,
                h_icon: std::ptr::null_mut(),
                sz_tip: tip_buf,
                dw_state: 0,
                dw_state_mask: 0,
                sz_info: [0u16; 256],
                u_version_or_timeout: 4,
                sz_info_title: [0u16; 64],
                dw_info_flags: 0,
                guid_item: Guid {
                    data1: 0,
                    data2: 0,
                    data3: 0,
                    data4: [0; 8],
                },
                h_balloon_icon: std::ptr::null_mut(),
            };
            if !visible {
                if REGISTERED.swap(false, Ordering::SeqCst) {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
                }
                return;
            }
            let msg = if REGISTERED.swap(true, Ordering::SeqCst) {
                NIM_MODIFY
            } else {
                NIM_ADD
            };
            let _ = Shell_NotifyIconW(msg, &mut nid);
        }
    }

    pub fn pop_pending_action() -> Option<String> {
        if !native_allowed() {
            return None;
        }
        PENDING_ACTIONS.lock().as_mut().and_then(|q| q.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn test_tray_win_lifecycle_and_methods() {
        let mut tray = TrayWinWin::new();
        assert!(tray.logo_no_margin().is_ok());
        assert!(tray.tray().is_ok());
        assert!(tray.create_icon().is_ok());
        assert!(tray.tray_icon_tool_tip().is_ok());
        assert_eq!(tray.tooltip, "Telegram Desktop");

        tray.set_unread_counter(12);
        assert_eq!(tray.unread_counter, 12);
        assert!(tray.tooltip.contains("12 unread"));

        assert!(tray.icon_clicks().is_ok());
        assert_eq!(tray.icon_clicks_count, 1);

        assert!(tray.show_from_tray_requests().is_ok());
        assert_eq!(tray.show_requests_count, 1);

        tray.set_remote_control_status(
            true,
            1,
            false,
            true,
            "987654321",
            Some("Pass1234".to_string()),
            "zh-CN",
        );
        assert!(tray.tooltip.contains("1 台远程会话"));
        assert!(tray.trigger_action(tray_action_ids::DISCONNECT_ALL));
        assert_eq!(
            tray.last_action_triggered.as_deref(),
            Some(tray_action_ids::DISCONNECT_ALL)
        );
    }
}
