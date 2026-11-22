//! macOS Menu Bar Status Item Tray (`NSStatusBar` + `NSStatusItem` + `NSMenu`)
//! Upstream Source: Telegram/SourceFiles/platform/mac/tray_mac.h, Telegram/SourceFiles/platform/mac/tray_mac.mm
//! Subsystem: platform | Feature ID: TCG-N46 | Disposition: rust_domain_or_server_1to1

use gpui_telegram_core::error::Result;
use serde::{Deserialize, Serialize};

/// Canonical tray action identifiers shared across macOS/Windows system trays and `AppShell`.
pub mod tray_action_ids {
    pub const SHOW_FABUSHI: &str = "show_fabushi";
    pub const OPEN_ALIAS: &str = "open";
    pub const CONTROLLED_STATUS: &str = "controlled_status";
    pub const DISCONNECT_ALL: &str = "disconnect_all";
    pub const TOGGLE_ALLOW_REMOTE: &str = "toggle_allow_remote";
    pub const COPY_ASSIST: &str = "copy_assist";
    pub const QUIT: &str = "quit";
}

/// Action item on macOS system menu bar tray.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacTrayAction {
    pub id: String,
    pub title: String,
    pub is_separator: bool,
    pub is_enabled: bool,
}

/// macOS Status Item system tray implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayMacMac {
    pub has_icon: bool,
    pub is_dark_appearance: bool,
    pub unread_count: u32,
    pub actions: Vec<MacTrayAction>,
    pub last_action_triggered: Option<String>,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub controlled_count: usize,
    #[serde(default)]
    pub controlled_by_ai: bool,
    #[serde(default = "default_allow_remote")]
    pub allow_remote_control: bool,
    #[serde(default)]
    pub assist_id: String,
    #[serde(default)]
    pub assist_code: Option<String>,
    #[serde(default = "default_lang_en")]
    pub language_id: String,
    #[serde(skip)]
    native_slot_id: u64,
}

fn default_allow_remote() -> bool {
    true
}

fn default_lang_en() -> String {
    "en".to_string()
}

fn format_assist_id_display(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 9 {
        format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9])
    } else if !raw.trim().is_empty() {
        raw.trim().to_string()
    } else {
        "--- --- ---".to_string()
    }
}

impl TrayMacMac {
    pub fn new() -> Self {
        Self {
            has_icon: false,
            is_dark_appearance: true,
            unread_count: 0,
            actions: Vec::new(),
            last_action_triggered: None,
            is_online: true,
            controlled_count: 0,
            controlled_by_ai: false,
            allow_remote_control: true,
            assist_id: String::new(),
            assist_code: None,
            language_id: "en".to_string(),
            native_slot_id: 0,
        }
    }

    pub fn create_icon(&mut self) -> Result<()> {
        self.has_icon = true;
        self.sync_native_tray();
        Ok(())
    }

    pub fn destroy_icon(&mut self) -> Result<()> {
        self.has_icon = false;
        #[cfg(target_os = "macos")]
        mac_native::destroy_status_item(&mut self.native_slot_id);
        Ok(())
    }

    pub fn update_appearance(&mut self, is_dark: bool) -> Result<()> {
        self.is_dark_appearance = is_dark;
        if self.has_icon {
            self.sync_native_tray();
        }
        Ok(())
    }

    pub fn set_unread_count(&mut self, count: u32) -> Result<()> {
        self.unread_count = count;
        if self.has_icon {
            self.sync_native_tray();
        }
        Ok(())
    }

    pub fn add_action(&mut self, id: impl Into<String>, title: impl Into<String>) -> Result<()> {
        self.actions.push(MacTrayAction {
            id: id.into(),
            title: title.into(),
            is_separator: false,
            is_enabled: true,
        });
        if self.has_icon {
            self.sync_native_tray();
        }
        Ok(())
    }

    pub fn add_separator(&mut self) -> Result<()> {
        self.actions.push(MacTrayAction {
            id: String::new(),
            title: String::new(),
            is_separator: true,
            is_enabled: false,
        });
        if self.has_icon {
            self.sync_native_tray();
        }
        Ok(())
    }

    pub fn trigger_action(&mut self, id: &str) -> Result<bool> {
        if let Some(pos) = self.actions.iter().position(|a| a.id == id && a.is_enabled) {
            let act_id = self.actions[pos].id.clone();
            self.last_action_triggered = Some(act_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Rebuilds the standard Fabushi remote-control & app tray menu in `lang`.
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

        self.actions.push(MacTrayAction {
            id: "status_indicator".to_string(),
            title: status_title,
            is_separator: false,
            is_enabled: false,
        });
        self.actions.push(MacTrayAction {
            id: String::new(),
            title: String::new(),
            is_separator: true,
            is_enabled: false,
        });
        self.actions.push(MacTrayAction {
            id: tray_action_ids::SHOW_FABUSHI.to_string(),
            title: if is_zh {
                "显示 Fabushi 主窗口".to_string()
            } else {
                "Show Fabushi".to_string()
            },
            is_separator: false,
            is_enabled: true,
        });
        self.actions.push(MacTrayAction {
            id: tray_action_ids::CONTROLLED_STATUS.to_string(),
            title: if self.controlled_count > 0 {
                if is_zh {
                    format!("本机被控状态窗口 ({})", self.controlled_count)
                } else {
                    format!("Controlled Status Window ({})", self.controlled_count)
                }
            } else if is_zh {
                "本机被控状态窗口".to_string()
            } else {
                "Controlled Status Window".to_string()
            },
            is_separator: false,
            is_enabled: true,
        });
        self.actions.push(MacTrayAction {
            id: tray_action_ids::DISCONNECT_ALL.to_string(),
            title: if is_zh {
                "断开所有远控".to_string()
            } else {
                "Disconnect All Remote Control".to_string()
            },
            is_separator: false,
            is_enabled: self.controlled_count > 0,
        });
        self.actions.push(MacTrayAction {
            id: String::new(),
            title: String::new(),
            is_separator: true,
            is_enabled: false,
        });
        self.actions.push(MacTrayAction {
            id: tray_action_ids::TOGGLE_ALLOW_REMOTE.to_string(),
            title: if is_zh {
                if self.allow_remote_control {
                    "✓ 允许远程控制本机".to_string()
                } else {
                    "  允许远程控制本机".to_string()
                }
            } else if self.allow_remote_control {
                "✓ Allow Remote Control".to_string()
            } else {
                "  Allow Remote Control".to_string()
            },
            is_separator: false,
            is_enabled: true,
        });

        let formatted_id = format_assist_id_display(&self.assist_id);
        let assist_label = if is_zh {
            format!("复制协助码 ({formatted_id})")
        } else {
            format!("Copy Assist ID & Code ({formatted_id})")
        };
        self.actions.push(MacTrayAction {
            id: tray_action_ids::COPY_ASSIST.to_string(),
            title: assist_label,
            is_separator: false,
            is_enabled: !self.assist_id.trim().is_empty(),
        });
        self.actions.push(MacTrayAction {
            id: String::new(),
            title: String::new(),
            is_separator: true,
            is_enabled: false,
        });
        self.actions.push(MacTrayAction {
            id: tray_action_ids::QUIT.to_string(),
            title: if is_zh {
                "退出 Fabushi".to_string()
            } else {
                "Quit Fabushi".to_string()
            },
            is_separator: false,
            is_enabled: true,
        });

        if self.has_icon {
            self.sync_native_tray();
        }
    }

    /// Updates online/controlled/assist telemetry and refreshes native macOS status bar item + menu.
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
    }

    /// Computes concise status bar title and tooltip reflecting online/controlled/unread state.
    pub fn status_bar_title_and_tooltip(&self) -> (String, String) {
        let is_zh = self.language_id.starts_with("zh");
        let title = if self.controlled_count > 0 {
            if self.controlled_by_ai {
                if is_zh {
                    format!("Fabushi · AI被控({})", self.controlled_count)
                } else {
                    format!("Fabushi · AI Ctrl({})", self.controlled_count)
                }
            } else if is_zh {
                format!("Fabushi · 被控({})", self.controlled_count)
            } else {
                format!("Fabushi · Ctrl({})", self.controlled_count)
            }
        } else if self.unread_count > 0 {
            format!("Fabushi ({})", self.unread_count)
        } else if !self.is_online {
            if is_zh {
                "Fabushi (离线)".to_string()
            } else {
                "Fabushi (Offline)".to_string()
            }
        } else {
            "Fabushi".to_string()
        };

        let tooltip = if self.controlled_count > 0 {
            if is_zh {
                format!("Fabushi - 本机正在被 {} 个远程会话控制", self.controlled_count)
            } else {
                format!(
                    "Fabushi - Controlled by {} active session(s)",
                    self.controlled_count
                )
            }
        } else if self.is_online {
            if is_zh {
                "Fabushi - 在线".to_string()
            } else {
                "Fabushi - Online".to_string()
            }
        } else if is_zh {
            "Fabushi - 离线".to_string()
        } else {
            "Fabushi - Offline".to_string()
        };

        (title, tooltip)
    }

    /// Formats the clipboard string for "Copy Assist ID & Code".
    pub fn formatted_assist_clipboard_text(&self) -> String {
        let id = format_assist_id_display(&self.assist_id);
        let is_zh = self.language_id.starts_with("zh");
        match self.assist_code.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(code) => {
                if is_zh {
                    format!("Fabushi 远程协助 ID: {id}\n验证码: {code}")
                } else {
                    format!("Fabushi Remote Assist ID: {id}\nCode: {code}")
                }
            }
            None => {
                if is_zh {
                    format!("Fabushi 远程协助 ID: {id}")
                } else {
                    format!("Fabushi Remote Assist ID: {id}")
                }
            }
        }
    }

    /// Polls any queued action triggered from the native macOS `NSMenuItem` callback.
    pub fn poll_native_action(&mut self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            if let Some(id) = mac_native::pop_pending_action() {
                self.last_action_triggered = Some(id.clone());
                return Some(id);
            }
        }
        None
    }

    fn sync_native_tray(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let (title, tooltip) = self.status_bar_title_and_tooltip();
            mac_native::sync_status_item(
                &mut self.native_slot_id,
                &title,
                &tooltip,
                &self.actions,
            );
        }
    }
}

impl Default for TrayMacMac {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod mac_native {
    use super::MacTrayAction;
    use objc::declare::ClassDecl;
    use objc::rc::autoreleasepool;
    use objc::runtime::{Class, Object, Sel, BOOL, NO, YES};
    use objc::{msg_send, sel, sel_impl};
    use parking_lot::Mutex;
    use std::collections::{HashMap, VecDeque};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Once;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    static NEXT_SLOT_ID: AtomicU64 = AtomicU64::new(1);
    static INIT_TARGET_CLASS: Once = Once::new();
    static TARGET_INSTANCE_PTR: AtomicU64 = AtomicU64::new(0);
    static STATUS_ITEMS: Mutex<Option<HashMap<u64, usize>>> = Mutex::new(None);
    static TAG_TO_ACTION: Mutex<Option<Vec<String>>> = Mutex::new(None);
    static PENDING_ACTIONS: Mutex<Option<VecDeque<String>>> = Mutex::new(None);

    fn native_allowed() -> bool {
        !cfg!(test) && std::env::var_os("GPUI_OFFLINE_TEST").is_none()
    }

    extern "C" fn on_tray_menu_item(_this: &Object, _cmd: Sel, sender: *mut Object) {
        if sender.is_null() {
            return;
        }
        unsafe {
            let tag: isize = msg_send![sender, tag];
            if tag < 0 {
                return;
            }
            let idx = tag as usize;
            let action_id = {
                let guard = TAG_TO_ACTION.lock();
                guard.as_ref().and_then(|v| v.get(idx).cloned())
            };
            if let Some(id) = action_id {
                let mut q = PENDING_ACTIONS.lock();
                q.get_or_insert_with(VecDeque::new).push_back(id);
            }
        }
    }

    unsafe fn ensure_target_instance() -> *mut Object {
        INIT_TARGET_CLASS.call_once(|| unsafe {
            if let Some(ns_object) = Class::get("NSObject") {
                if let Some(mut decl) = ClassDecl::new("FabushiTrayMenuTarget", ns_object) {
                    decl.add_method(
                        sel!(onTrayMenuItem:),
                        on_tray_menu_item as extern "C" fn(&Object, Sel, *mut Object),
                    );
                    let cls = decl.register();
                    let inst: *mut Object = msg_send![cls, new];
                    TARGET_INSTANCE_PTR.store(inst as usize as u64, Ordering::SeqCst);
                } else if let Some(existing) = Class::get("FabushiTrayMenuTarget") {
                    let inst: *mut Object = msg_send![existing, new];
                    TARGET_INSTANCE_PTR.store(inst as usize as u64, Ordering::SeqCst);
                }
            }
        });
        TARGET_INSTANCE_PTR.load(Ordering::SeqCst) as usize as *mut Object
    }

    unsafe fn ns_string(s: &str) -> *mut Object {
        let Some(cls) = Class::get("NSString") else {
            return std::ptr::null_mut();
        };
        let alloc: *mut Object = msg_send![cls, alloc];
        if alloc.is_null() {
            return std::ptr::null_mut();
        }
        let bytes = s.as_ptr() as *const c_void;
        let obj: *mut Object =
            msg_send![alloc, initWithBytes:bytes length:s.len() encoding:4usize];
        let _: *mut Object = msg_send![obj, autorelease];
        obj
    }

    unsafe fn is_main_thread_with_appkit() -> bool {
        let Some(ns_thread) = Class::get("NSThread") else {
            return false;
        };
        let is_main: BOOL = msg_send![ns_thread, isMainThread];
        if is_main == NO {
            return false;
        }
        let Some(ns_app) = Class::get("NSApplication") else {
            return false;
        };
        let shared: *mut Object = msg_send![ns_app, sharedApplication];
        !shared.is_null()
    }

    pub fn sync_status_item(
        slot_id: &mut u64,
        title: &str,
        tooltip: &str,
        actions: &[MacTrayAction],
    ) {
        if !native_allowed() {
            return;
        }
        unsafe {
            if !is_main_thread_with_appkit() {
                return;
            }
            autoreleasepool(|| {
                let Some(ns_status_bar) = Class::get("NSStatusBar") else {
                    return;
                };
                let bar: *mut Object = msg_send![ns_status_bar, systemStatusBar];
                if bar.is_null() {
                    return;
                }

                let item_ptr: *mut Object = {
                    let mut map_guard = STATUS_ITEMS.lock();
                    let map = map_guard.get_or_insert_with(HashMap::new);
                    if *slot_id != 0 {
                        if let Some(&ptr) = map.get(slot_id) {
                            ptr as *mut Object
                        } else {
                            let item: *mut Object =
                                msg_send![bar, statusItemWithLength: -1.0f64];
                            if item.is_null() {
                                return;
                            }
                            let _: *mut Object = msg_send![item, retain];
                            map.insert(*slot_id, item as usize);
                            item
                        }
                    } else {
                        let new_id = NEXT_SLOT_ID.fetch_add(1, Ordering::Relaxed);
                        let item: *mut Object = msg_send![bar, statusItemWithLength: -1.0f64];
                        if item.is_null() {
                            return;
                        }
                        let _: *mut Object = msg_send![item, retain];
                        *slot_id = new_id;
                        map.insert(new_id, item as usize);
                        item
                    }
                };

                let button: *mut Object = msg_send![item_ptr, button];
                if !button.is_null() {
                    let ns_title = ns_string(title);
                    if !ns_title.is_null() {
                        let _: () = msg_send![button, setTitle: ns_title];
                    }
                    let ns_tip = ns_string(tooltip);
                    if !ns_tip.is_null() {
                        let _: () = msg_send![button, setToolTip: ns_tip];
                    }
                }

                let Some(ns_menu_cls) = Class::get("NSMenu") else {
                    return;
                };
                let Some(ns_item_cls) = Class::get("NSMenuItem") else {
                    return;
                };

                let menu: *mut Object = msg_send![ns_menu_cls, new];
                if menu.is_null() {
                    return;
                }
                let _: () = msg_send![menu, setAutoenablesItems: NO];

                let target = ensure_target_instance();
                let mut tag_map = Vec::with_capacity(actions.len());

                for act in actions {
                    if act.is_separator {
                        let sep: *mut Object = msg_send![ns_item_cls, separatorItem];
                        if !sep.is_null() {
                            let _: () = msg_send![menu, addItem: sep];
                        }
                        continue;
                    }
                    let tag_idx = tag_map.len();
                    tag_map.push(act.id.clone());

                    let item_title = ns_string(&act.title);
                    let empty_key = ns_string("");
                    let mi: *mut Object = msg_send![ns_item_cls, alloc];
                    let mi: *mut Object = msg_send![
                        mi,
                        initWithTitle: item_title
                        action: sel!(onTrayMenuItem:)
                        keyEquivalent: empty_key
                    ];
                    if !mi.is_null() {
                        if !target.is_null() {
                            let _: () = msg_send![mi, setTarget: target];
                        }
                        let _: () = msg_send![mi, setTag: tag_idx as isize];
                        let enabled_flag: BOOL = if act.is_enabled { YES } else { NO };
                        let _: () = msg_send![mi, setEnabled: enabled_flag];
                        let _: () = msg_send![menu, addItem: mi];
                        let _: () = msg_send![mi, release];
                    }
                }

                *TAG_TO_ACTION.lock() = Some(tag_map);
                let _: () = msg_send![item_ptr, setMenu: menu];
                let _: () = msg_send![menu, release];
            });
        }
    }

    pub fn destroy_status_item(slot_id: &mut u64) {
        let id = *slot_id;
        *slot_id = 0;
        if id == 0 || !native_allowed() {
            return;
        }
        let ptr_opt = {
            let mut guard = STATUS_ITEMS.lock();
            guard.as_mut().and_then(|map| map.remove(&id))
        };
        let Some(ptr_val) = ptr_opt else {
            return;
        };
        unsafe {
            if !is_main_thread_with_appkit() {
                return;
            }
            autoreleasepool(|| {
                if let Some(ns_status_bar) = Class::get("NSStatusBar") {
                    let bar: *mut Object = msg_send![ns_status_bar, systemStatusBar];
                    let item = ptr_val as *mut Object;
                    if !bar.is_null() && !item.is_null() {
                        let _: () = msg_send![bar, removeStatusItem: item];
                        let _: () = msg_send![item, release];
                    }
                }
            });
        }
    }

    pub fn pop_pending_action() -> Option<String> {
        if !native_allowed() {
            return None;
        }
        let mut guard = PENDING_ACTIONS.lock();
        guard.as_mut().and_then(|q| q.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tray_menu_actions() {
        let mut tray = TrayMacMac::new();
        tray.create_icon().unwrap();
        assert!(tray.has_icon);

        tray.add_action("open", "Show Telegram").unwrap();
        tray.add_separator().unwrap();
        tray.add_action("quit", "Quit Telegram").unwrap();
        assert_eq!(tray.actions.len(), 3);

        assert!(tray.trigger_action("open").unwrap());
        assert_eq!(tray.last_action_triggered, Some("open".into()));

        tray.set_unread_count(5).unwrap();
        assert_eq!(tray.unread_count, 5);
    }

    #[test]
    fn test_fabushi_remote_tray_menu_status_and_assist_formatting() {
        let mut tray = TrayMacMac::new();
        tray.set_remote_control_status(
            true,
            2,
            true,
            true,
            "123456789",
            Some("Abc12345".to_string()),
            "zh-CN",
        );
        tray.create_icon().unwrap();

        let (title, tip) = tray.status_bar_title_and_tooltip();
        assert!(title.contains("AI被控(2)"));
        assert!(tip.contains("2 个远程会话"));

        assert!(tray
            .actions
            .iter()
            .any(|a| a.id == tray_action_ids::DISCONNECT_ALL && a.is_enabled));
        assert!(tray
            .actions
            .iter()
            .any(|a| a.id == tray_action_ids::CONTROLLED_STATUS && a.is_enabled));
        assert!(tray
            .actions
            .iter()
            .any(|a| a.id == tray_action_ids::TOGGLE_ALLOW_REMOTE && a.title.contains('✓')));
        assert!(tray
            .actions
            .iter()
            .any(|a| a.id == tray_action_ids::COPY_ASSIST && a.title.contains("123 456 789")));

        let clip = tray.formatted_assist_clipboard_text();
        assert!(clip.contains("123 456 789"));
        assert!(clip.contains("Abc12345"));

        assert!(tray.trigger_action(tray_action_ids::DISCONNECT_ALL).unwrap());
        assert_eq!(
            tray.last_action_triggered.as_deref(),
            Some(tray_action_ids::DISCONNECT_ALL)
        );
        assert!(tray.poll_native_action().is_none());
        tray.destroy_icon().unwrap();
        assert!(!tray.has_icon);
    }
}
