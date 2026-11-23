//! Native macOS `WKWebView` child view host and `TelegramWebviewProxy` JS bridge.

#![cfg(target_os = "macos")]
#![allow(unexpected_cfgs)]

use std::collections::VecDeque;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::ptr;
use std::sync::{Arc, Mutex, Once};

use gpui::{Bounds, Pixels, Window};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Protocol, Sel, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct CgPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct CgSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct CgRect {
    pub origin: CgPoint,
    pub size: CgSize,
}

unsafe impl Encode for CgPoint {
    fn encode() -> Encoding {
        unsafe { Encoding::from_str("{CGPoint=dd}") }
    }
}

unsafe impl Encode for CgSize {
    fn encode() -> Encoding {
        unsafe { Encoding::from_str("{CGSize=dd}") }
    }
}

unsafe impl Encode for CgRect {
    fn encode() -> Encoding {
        unsafe { Encoding::from_str("{CGRect={CGPoint=dd}{CGSize=dd}}") }
    }
}

const SCRIPT_HANDLER_NAME: &str = "telegramWebviewProxy";
const BRIDGE_IVAR_NAME: &str = "bridgeState";

struct BridgeState {
    queue: Arc<Mutex<VecDeque<(String, String)>>>,
    notify_tx: UnboundedSender<()>,
}

pub const TELEGRAM_WEBVIEW_PROXY_BOOT_JS: &str = r#"
(function() {
  if (window.__gpuiTelegramProxyInstalled) return;
  window.__gpuiTelegramProxyInstalled = true;
  var isTopFrame = (window === window.top);
  function hasTgAppFrame() {
    try {
      return !!(document && document.getElementById && document.getElementById('tg-app-frame')) || !!window.__GPUI_WEBMCP_HOST_WRAPPER__;
    } catch (_) {
      return !!window.__GPUI_WEBMCP_HOST_WRAPPER__;
    }
  }
  var postBridgeEvent = function(eventType, eventData) {
    try {
      var payload = {
        eventType: String(eventType || ''),
        eventData: typeof eventData === 'string' ? eventData : JSON.stringify(eventData == null ? {} : eventData)
      };
      if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.telegramWebviewProxy) {
        window.webkit.messageHandlers.telegramWebviewProxy.postMessage(payload);
      }
    } catch (e) {}
  };
  if (isTopFrame) {
    window.TelegramWebviewProxy = {
      postEvent: postBridgeEvent
    };
  }
  window.TelegramGameProxy = window.TelegramGameProxy || {
    receiveEvent: function(eventType, eventData) {
      if (window.Telegram && window.Telegram.WebView && typeof window.Telegram.WebView.receiveEvent === 'function') {
        window.Telegram.WebView.receiveEvent(eventType, eventData);
      }
    }
  };
  window.addEventListener('message', function(ev) {
    try {
      var d = typeof ev.data === 'string' ? JSON.parse(ev.data) : ev.data;
      if (!d || typeof d !== 'object') return;
      if (d.type && String(d.type).indexOf('gpui_webmcp_') === 0) return;
      if (d.type === 'telegram_web_app_event' && hasTgAppFrame()) return;
      if (d.eventType) {
        postBridgeEvent(d.eventType, d.eventData);
      }
    } catch (e) {}
  });
  (function installEarlyWebMcp() {
    var existing = window.__GPUI_WEBMCP__ || {};
    var toolsMap = existing._tools || {};
    var stateProvider = typeof existing._stateProvider === 'function' ? existing._stateProvider : null;
    var pageErrors = Array.isArray(existing._pageErrors) ? existing._pageErrors : [];
    var pendingToolWaiters = [];
    var toolsChangedTimer = null;
    var domReadyAt = (document.readyState === 'interactive' || document.readyState === 'complete') ? Date.now() : 0;
    function pushPageError(msg) {
      var clean = String(msg || '').replace(/\s+/g, ' ').trim().slice(0, 240);
      if (!clean) return;
      if (pageErrors.length && pageErrors[pageErrors.length - 1] === clean) return;
      pageErrors.push(clean);
      if (pageErrors.length > 5) pageErrors.shift();
    }
    if (!window.__gpuiWebMcpErrorListenersInstalled) {
      window.__gpuiWebMcpErrorListenersInstalled = true;
      window.addEventListener('error', function(ev) {
        try {
          var m = (ev && ev.message) || (ev && ev.error && ev.error.message) || 'Page script error';
          pushPageError(m);
        } catch (_) {}
      });
      window.addEventListener('unhandledrejection', function(ev) {
        try {
          var r = ev && ev.reason;
          var m = (r && r.message) ? r.message : String(r || 'Unhandled promise rejection');
          pushPageError(m);
        } catch (_) {}
      });
    }
    function readVisibleToast() {
      try {
        var sels = ['#toast', '[role="alert"]', '.toast', '.error-banner'];
        for (var i = 0; i < sels.length; i++) {
          var el = document.querySelector(sels[i]);
          if (!el) continue;
          var cs = window.getComputedStyle ? window.getComputedStyle(el) : null;
          if (cs && (cs.display === 'none' || cs.visibility === 'hidden' || cs.opacity === '0')) continue;
          var txt = (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 240);
          if (txt) return txt;
        }
      } catch (_) {}
      return '';
    }
    function enrichSummaryWithDiagnostics(summary) {
      var out = (summary && typeof summary === 'object') ? Object.assign({}, summary) : {};
      var toast = readVisibleToast();
      if (toast && !out.visible_toast) out.visible_toast = toast;
      if (pageErrors.length && !out.page_errors) out.page_errors = pageErrors.slice();
      return out;
    }
    function readStateSummary() {
      try {
        if (typeof stateProvider === 'function') {
          var provided = stateProvider();
          if (provided && typeof provided === 'object') return enrichSummaryWithDiagnostics(provided);
        }
        if (window.STATE && typeof window.STATE === 'object') return enrichSummaryWithDiagnostics(window.STATE);
      } catch (_) {}
      return enrichSummaryWithDiagnostics({
        kind: 'external_webview',
        title: document.title || '',
        url: String(window.location && window.location.href || '')
      });
    }
    function inspectPage() {
      var controls = [];
      try {
        var nodes = document.querySelectorAll('button, a, input, select, textarea, [role="button"], .chip');
        for (var i = 0; i < nodes.length && controls.length < 48; i++) {
          var el = nodes[i];
          controls.push({
            index: i,
            tag: (el.tagName || '').toLowerCase(),
            id: el.id || null,
            type: el.type || null,
            placeholder: el.placeholder || null,
            text: (el.innerText || el.textContent || el.value || '').trim().slice(0, 64),
            disabled: !!el.disabled
          });
        }
      } catch (_) {}
      var res = { title: document.title || '', controls: controls, state_summary: readStateSummary() };
      var toast = readVisibleToast();
      if (toast) res.visible_toast = toast;
      if (pageErrors.length) res.page_errors = pageErrors.slice();
      return res;
    }
    function builtins() {
      return {
        get_state: {
          name: 'get_state',
          description: '读取当前页面运行状态或摘要',
          inputSchema: { type: 'object', properties: {} },
          annotations: { readOnlyHint: true },
          execute: function() { return readStateSummary(); }
        },
        page_inspect: {
          name: 'page_inspect',
          description: '探测当前页面可交互控件与结构',
          inputSchema: { type: 'object', properties: {} },
          annotations: { readOnlyHint: true },
          execute: function() { return inspectPage(); }
        },
        page_click: {
          name: 'page_click',
          description: '点击匹配 selector 或文案的控件',
          inputSchema: { type: 'object', properties: { selector: { type: 'string' }, text: { type: 'string' } } },
          execute: function(args) {
            var a = args || {};
            var t = a.selector ? document.querySelector(String(a.selector)) : null;
            if (!t && a.text) {
              var needle = String(a.text).trim();
              var els = document.querySelectorAll('button, a, [role="button"], .chip, .card, .btn');
              for (var i = 0; i < els.length; i++) {
                if ((els[i].innerText || els[i].textContent || '').indexOf(needle) !== -1) { t = els[i]; break; }
              }
            }
            if (!t) throw new Error('未找到可点击目标元素');
            t.click();
            return { clicked: (t.innerText || t.id || t.tagName || '').trim().slice(0, 80), state: readStateSummary() };
          }
        },
        page_fill: {
          name: 'page_fill',
          description: '填写输入框或更新组件值',
          inputSchema: { type: 'object', required: ['value'], properties: { selector: { type: 'string' }, widget_id: { type: 'string' }, value: {} } },
          execute: function(args) {
            var a = args || {};
            if (a.widget_id && typeof window.dispatchAction === 'function') {
              return Promise.resolve(window.dispatchAction('widget_event', a.widget_id, a.value)).then(function() {
                return { updated_widget: a.widget_id, state: readStateSummary() };
              });
            }
            var inp = a.selector ? document.querySelector(String(a.selector)) : document.querySelector('input, textarea, select');
            if (!inp) throw new Error('未找到输入框');
            inp.value = a.value == null ? '' : String(a.value);
            inp.dispatchEvent(new Event('input', { bubbles: true }));
            inp.dispatchEvent(new Event('change', { bubbles: true }));
            return { filled: inp.id || inp.tagName, value: inp.value, state: readStateSummary() };
          }
        },
        page_press_main_button: {
          name: 'page_press_main_button',
          description: '触发 MainButton 点击',
          inputSchema: { type: 'object', properties: {} },
          execute: function() {
            if (window.Telegram && window.Telegram.WebApp && window.Telegram.WebApp.MainButton && window.Telegram.WebApp.MainButton._fireClick) {
              window.Telegram.WebApp.MainButton._fireClick();
            }
            return { pressed: 'main_button', state: readStateSummary() };
          }
        },
        page_press_secondary_button: {
          name: 'page_press_secondary_button',
          description: '触发 SecondaryButton 点击',
          inputSchema: { type: 'object', properties: {} },
          execute: function() {
            if (window.Telegram && window.Telegram.WebApp && window.Telegram.WebApp.SecondaryButton && window.Telegram.WebApp.SecondaryButton._fireClick) {
              window.Telegram.WebApp.SecondaryButton._fireClick();
            }
            return { pressed: 'secondary_button', state: readStateSummary() };
          }
        }
      };
    }
    function listDescriptors() {
      var merged = {};
      var b = builtins();
      for (var bk in b) if (Object.prototype.hasOwnProperty.call(b, bk)) merged[bk] = b[bk];
      for (var tk in toolsMap) if (Object.prototype.hasOwnProperty.call(toolsMap, tk)) merged[tk] = toolsMap[tk];
      return Object.keys(merged).map(function(name) {
        var t = merged[name];
        return { name: t.name, description: String(t.description || ''), input_schema: t.inputSchema || { type: 'object', properties: {} }, read_only: !!(t.annotations && t.annotations.readOnlyHint) };
      });
    }
    function wakeToolWaiters(toolName) {
      if (!pendingToolWaiters.length) return;
      var remaining = [];
      for (var i = 0; i < pendingToolWaiters.length; i++) {
        var w = pendingToolWaiters[i];
        var found = toolsMap[w.toolName] || builtins()[w.toolName];
        if ((w.toolName === toolName || found) && found && typeof found.execute === 'function') {
          if (w.timer) clearTimeout(w.timer);
          try { w.resolve(found); } catch (_) {}
        } else {
          remaining.push(w);
        }
      }
      pendingToolWaiters = remaining;
    }
    function waitForTool(toolName, maxWaitMs) {
      var b = builtins();
      var existingTool = toolsMap[toolName] || b[toolName];
      if (existingTool && typeof existingTool.execute === 'function') {
        return Promise.resolve(existingTool);
      }
      var budget = Number(maxWaitMs || 2500);
      if (domReadyAt > 0 && document.readyState === 'complete' && (Date.now() - domReadyAt) > 1200) {
        budget = Math.min(budget, 1200);
      }
      return new Promise(function(resolve) {
        var entry = { toolName: String(toolName || ''), resolve: resolve, timer: null };
        entry.timer = setTimeout(function() {
          pendingToolWaiters = pendingToolWaiters.filter(function(item) { return item !== entry; });
          var latest = toolsMap[entry.toolName] || builtins()[entry.toolName] || null;
          resolve(latest);
        }, budget);
        pendingToolWaiters.push(entry);
      });
    }
    function scheduleToolsChanged() {
      if (toolsChangedTimer) clearTimeout(toolsChangedTimer);
      toolsChangedTimer = setTimeout(function() {
        toolsChangedTimer = null;
        var descriptors = listDescriptors();
        var curUrl = String(window.location && window.location.href || '');
        if (!isTopFrame && window.parent && window.parent !== window) {
          try {
            window.parent.postMessage({
              type: 'gpui_webmcp_tools_changed',
              url: curUrl,
              tools: descriptors
            }, '*');
          } catch (_) {}
        } else if (!hasTgAppFrame()) {
          postBridgeEvent('gpui_webmcp_tools_changed', {
            url: curUrl,
            in_iframe: false,
            tools: descriptors
          });
        }
      }, 0);
    }
    function registerTool(def) {
      if (!def || !def.name || typeof def.execute !== 'function') return;
      var name = String(def.name);
      toolsMap[name] = def;
      wakeToolWaiters(name);
      scheduleToolsChanged();
    }
    function setStateProvider(fn) {
      stateProvider = typeof fn === 'function' ? fn : null;
      if (window.__GPUI_WEBMCP__ && window.__GPUI_WEBMCP__._isEarlyShim) {
        window.__GPUI_WEBMCP__._stateProvider = stateProvider;
      }
    }
    function reportStatus(statusOrSummary) {
      var payload = {};
      if (typeof statusOrSummary === 'string') {
        payload = { status_text: statusOrSummary };
      } else if (statusOrSummary && typeof statusOrSummary === 'object') {
        payload = Object.assign({}, statusOrSummary);
      }
      if (!isTopFrame && window.parent && window.parent !== window) {
        try {
          window.parent.postMessage({ type: 'gpui_webmcp_state_update', payload: payload }, '*');
        } catch (_) {}
      } else {
        postBridgeEvent('webmcp_state_update', payload);
      }
    }
    function emitReadySignal() {
      var curUrl = String(window.location && window.location.href || '');
      if (!isTopFrame && window.parent && window.parent !== window) {
        try {
          window.parent.postMessage({
            type: 'gpui_webmcp_ready',
            url: curUrl,
            tools: listDescriptors(),
            state_summary: readStateSummary()
          }, '*');
        } catch (_) {}
        return true;
      }
      if (hasTgAppFrame()) {
        return false;
      }
      postBridgeEvent('__gpui_webmcp_page_ready', {
        url: curUrl,
        in_iframe: false,
        tools: listDescriptors(),
        state_summary: readStateSummary()
      });
      return true;
    }
    var mc = {
      registerTool: registerTool,
      unregisterTool: function(n) { delete toolsMap[String(n || '')]; scheduleToolsChanged(); },
      provideContext: function(c) {
        toolsMap = {};
        var arr = (c && Array.isArray(c.tools)) ? c.tools : [];
        for (var i = 0; i < arr.length; i++) registerTool(arr[i]);
        if (c && typeof c.getState === 'function') setStateProvider(c.getState);
        scheduleToolsChanged();
      },
      clearContext: function() { toolsMap = {}; setStateProvider(null); scheduleToolsChanged(); },
      setStateProvider: setStateProvider,
      reportStatus: reportStatus
    };
    try {
      if (!navigator.modelContext) Object.defineProperty(navigator, 'modelContext', { value: mc, configurable: true });
      else {
        navigator.modelContext.registerTool = mc.registerTool;
        navigator.modelContext.unregisterTool = mc.unregisterTool;
        navigator.modelContext.provideContext = mc.provideContext;
        navigator.modelContext.clearContext = mc.clearContext;
        navigator.modelContext.setStateProvider = mc.setStateProvider;
        navigator.modelContext.reportStatus = mc.reportStatus;
      }
    } catch (_) {}
    if (!window.__GPUI_WEBMCP__ || !window.__GPUI_WEBMCP__._isHostWrapper) {
      window.__GPUI_WEBMCP__ = {
        _isEarlyShim: true,
        _tools: toolsMap,
        _stateProvider: stateProvider,
        _pageErrors: pageErrors,
        discover: listDescriptors,
        readState: readStateSummary,
        checkReady: emitReadySignal,
        invoke: function(requestId, operation, toolName, args, replyTarget) {
          function reply(ok, result, err) {
            var respPayload = {
              request_id: String(requestId || ''),
              ok: !!ok,
              result: result == null ? {} : result,
              error: err ? String(err) : null,
              tools: (operation === 'discover' || !ok) ? listDescriptors() : [],
              state_summary: readStateSummary(),
              control_mode: 'ai_controlling'
            };
            if (replyTarget && typeof replyTarget.postMessage === 'function') {
              try {
                replyTarget.postMessage({
                  type: 'gpui_webmcp_response',
                  request_id: respPayload.request_id,
                  payload: respPayload
                }, '*');
                return;
              } catch (_) {}
            }
            postBridgeEvent('webmcp_response', respPayload);
          }
          try {
            if (operation === 'discover') { reply(true, { count: listDescriptors().length }, null); return; }
            if (operation === 'get_state') { reply(true, readStateSummary(), null); return; }
            var b = builtins();
            var t = toolsMap[toolName] || b[toolName];
            if (!t || typeof t.execute !== 'function') {
              if (toolName) {
                waitForTool(String(toolName), 2500).then(function(found) {
                  if (!found || typeof found.execute !== 'function') {
                    reply(false, {}, '未注册的 WebMCP 工具: ' + String(toolName || ''));
                    return;
                  }
                  try {
                    Promise.resolve(found.execute(args || {})).then(function(r) { reply(true, r, null); }).catch(function(e) { reply(false, {}, e && e.message ? e.message : String(e)); });
                  } catch (e) {
                    reply(false, {}, e && e.message ? e.message : String(e));
                  }
                });
                return;
              }
              reply(false, {}, '未注册的 WebMCP 工具: ' + String(toolName || ''));
              return;
            }
            Promise.resolve(t.execute(args || {})).then(function(r) { reply(true, r, null); }).catch(function(e) { reply(false, {}, e && e.message ? e.message : String(e)); });
          } catch (e) { reply(false, {}, e && e.message ? e.message : String(e)); }
        }
      };
    }
    if (!isTopFrame) {
      window.addEventListener('message', function(ev) {
        try {
          var d = typeof ev.data === 'string' ? JSON.parse(ev.data) : ev.data;
          if (!d || typeof d !== 'object') return;
          if (d.type === 'gpui_webmcp_invoke') {
            var target = ev.source || window.parent;
            if (window.__GPUI_WEBMCP__ && typeof window.__GPUI_WEBMCP__.invoke === 'function') {
              window.__GPUI_WEBMCP__.invoke(d.request_id, d.operation, d.tool_name, d.arguments, target);
            }
          } else if (d.type === 'gpui_webmcp_ping_ready') {
            if (document.readyState === 'interactive' || document.readyState === 'complete') {
              emitReadySignal();
            }
          }
        } catch (_) {}
      });
      var sendUnready = function() {
        try {
          if (window.parent && window.parent !== window) {
            window.parent.postMessage({
              type: 'gpui_webmcp_unready',
              url: String(window.location && window.location.href || '')
            }, '*');
          }
        } catch (_) {}
      };
      window.addEventListener('pagehide', sendUnready);
      window.addEventListener('beforeunload', sendUnready);
    }
    window.addEventListener('DOMContentLoaded', function() {
      domReadyAt = Date.now();
      emitReadySignal();
    });
    if (document.readyState === 'interactive' || document.readyState === 'complete') {
      domReadyAt = Date.now();
      emitReadySignal();
    }
  })();
})();
"#;

/// Returns `true` when running inside headless/offline tests where native AppKit child views must not be attached.
pub fn is_headless_or_offline_test_env() -> bool {
    std::env::var("GPUI_OFFLINE_TEST").is_ok()
        || std::env::var("GPUI_MINIAPP_HEADLESS_CHILD").is_ok()
}

unsafe fn alloc_nsstring(s: &str) -> *mut Object {
    let bytes = s.as_bytes();
    unsafe {
        let obj: *mut Object = msg_send![class!(NSString), alloc];
        let obj: *mut Object = msg_send![
            obj,
            initWithBytes: bytes.as_ptr() as *const c_void
            length: bytes.len()
            encoding: 4usize // NSUTF8StringEncoding
        ];
        obj
    }
}

unsafe fn nsstring_to_rust(ns_str: *mut Object) -> String {
    if ns_str.is_null() {
        return String::new();
    }
    unsafe {
        let ptr: *const c_char = msg_send![ns_str, UTF8String];
        if ptr.is_null() {
            return String::new();
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

unsafe fn nsdata_to_rust(ns_data: *mut Object) -> String {
    if ns_data.is_null() {
        return String::new();
    }
    unsafe {
        let s: *mut Object = msg_send![class!(NSString), alloc];
        let s: *mut Object = msg_send![s, initWithData: ns_data encoding: 4usize];
        if s.is_null() {
            return String::new();
        }
        let out = nsstring_to_rust(s);
        let _: () = msg_send![s, release];
        out
    }
}

unsafe fn extract_bridge_event(body: *mut Object) -> Option<(String, String)> {
    if body.is_null() {
        return None;
    }
    unsafe {
        let is_dict: BOOL = msg_send![body, isKindOfClass: class!(NSDictionary)];
        if is_dict == YES {
            let k_type = alloc_nsstring("eventType");
            let k_data = alloc_nsstring("eventData");
            let raw_type: *mut Object = msg_send![body, objectForKey: k_type];
            let raw_data: *mut Object = msg_send![body, objectForKey: k_data];
            let _: () = msg_send![k_type, release];
            let _: () = msg_send![k_data, release];

            let event_type = if raw_type.is_null() {
                String::new()
            } else {
                let is_str: BOOL = msg_send![raw_type, isKindOfClass: class!(NSString)];
                if is_str == YES {
                    nsstring_to_rust(raw_type)
                } else {
                    let desc: *mut Object = msg_send![raw_type, description];
                    nsstring_to_rust(desc)
                }
            };
            if event_type.trim().is_empty() {
                return None;
            }
            let event_data = if raw_data.is_null() {
                "{}".to_string()
            } else {
                let is_str: BOOL = msg_send![raw_data, isKindOfClass: class!(NSString)];
                if is_str == YES {
                    nsstring_to_rust(raw_data)
                } else {
                    let json_cls = class!(NSJSONSerialization);
                    let valid: BOOL = msg_send![json_cls, isValidJSONObject: raw_data];
                    if valid == YES {
                        let data: *mut Object = msg_send![
                            json_cls,
                            dataWithJSONObject: raw_data
                            options: 0usize
                            error: ptr::null_mut::<*mut Object>()
                        ];
                        nsdata_to_rust(data)
                    } else {
                        let desc: *mut Object = msg_send![raw_data, description];
                        nsstring_to_rust(desc)
                    }
                }
            };
            return Some((event_type, event_data));
        }

        let is_str: BOOL = msg_send![body, isKindOfClass: class!(NSString)];
        if is_str == YES {
            let raw = nsstring_to_rust(body);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                let event_type = val
                    .get("eventType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if event_type.trim().is_empty() {
                    return None;
                }
                let event_data = match val.get("eventData") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => "{}".to_string(),
                };
                return Some((event_type, event_data));
            }
        }
        None
    }
}

extern "C" fn did_receive_script_message(
    this: &Object,
    _cmd: Sel,
    _user_content_controller: *mut Object,
    message: *mut Object,
) {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar(BRIDGE_IVAR_NAME);
        if ptr.is_null() {
            return;
        }
        let state = &*(ptr as *const BridgeState);
        let body: *mut Object = msg_send![message, body];
        if let Some((ev_type, ev_data)) = extract_bridge_event(body) {
            if let Ok(mut q) = state.queue.lock() {
                q.push_back((ev_type, ev_data));
            }
            let _ = state.notify_tx.send(());
        }
    }
}

extern "C" fn handler_dealloc(this: &mut Object, _cmd: Sel) {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar(BRIDGE_IVAR_NAME);
        if !ptr.is_null() {
            let _ = Box::from_raw(ptr as *mut BridgeState);
            this.set_ivar(BRIDGE_IVAR_NAME, ptr::null_mut::<c_void>());
        }
        let _: () = msg_send![super(this, class!(NSObject)), dealloc];
    }
}

fn get_or_register_script_handler_class() -> Option<&'static Class> {
    static REGISTER_ONCE: Once = Once::new();
    REGISTER_ONCE.call_once(|| {
        if Class::get("GPUIWKScriptMessageHandler").is_some() {
            return;
        }
        let Some(superclass) = Class::get("NSObject") else {
            return;
        };
        let Some(mut decl) = ClassDecl::new("GPUIWKScriptMessageHandler", superclass) else {
            return;
        };
        decl.add_ivar::<*mut c_void>(BRIDGE_IVAR_NAME);
        if let Some(proto) = Protocol::get("WKScriptMessageHandler") {
            decl.add_protocol(proto);
        }
        unsafe {
            decl.add_method(
                sel!(userContentController:didReceiveScriptMessage:),
                did_receive_script_message as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(dealloc),
                handler_dealloc as extern "C" fn(&mut Object, Sel),
            );
        }
        decl.register();
    });
    Class::get("GPUIWKScriptMessageHandler")
}

/// Attached native macOS `WKWebView` child view inside GPUI's `NSView`.
pub struct MacOsWkWebView {
    parent_ns_view: *mut Object,
    webview: *mut Object,
    user_content_controller: *mut Object,
    script_handler: *mut Object,
    event_queue: Arc<Mutex<VecDeque<(String, String)>>>,
    loaded_url: String,
    is_hidden: bool,
    last_frame: Option<(f64, f64, f64, f64)>,
    last_zoom: Option<f64>,
}

impl MacOsWkWebView {
    /// Returns `true` when running inside headless/offline tests where native AppKit child views must not be attached.
    pub fn is_headless_or_offline_test_env() -> bool {
        is_headless_or_offline_test_env()
    }

    /// Attaches a child `WKWebView` to `window`'s native `NSView` when running in a live macOS desktop window.
    /// Returns `(handle, wakeup_receiver)` or `None` under headless/offline test environments.
    pub fn attach(window: &Window) -> Option<(Self, UnboundedReceiver<()>)> {
        if is_headless_or_offline_test_env() {
            return None;
        }
        let win_handle = HasWindowHandle::window_handle(window).ok()?;
        let parent_ns_view = match win_handle.as_raw() {
            RawWindowHandle::AppKit(appkit) => appkit.ns_view.as_ptr() as *mut Object,
            _ => return None,
        };
        if parent_ns_view.is_null() {
            return None;
        }
        let wk_cls = Class::get("WKWebView")?;
        let cfg_cls = Class::get("WKWebViewConfiguration")?;
        let script_cls = Class::get("WKUserScript")?;
        let handler_cls = get_or_register_script_handler_class()?;

        let event_queue = Arc::new(Mutex::new(VecDeque::new()));
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();

        unsafe {
            let config: *mut Object = msg_send![cfg_cls, new];
            if config.is_null() {
                return None;
            }
            let ucc: *mut Object = msg_send![config, userContentController];
            if ucc.is_null() {
                let _: () = msg_send![config, release];
                return None;
            }
            let _: *mut Object = msg_send![ucc, retain];

            // Inject TelegramWebviewProxy.postEvent at document start.
            let boot_js = alloc_nsstring(TELEGRAM_WEBVIEW_PROXY_BOOT_JS);
            let user_script: *mut Object = msg_send![script_cls, alloc];
            let user_script: *mut Object = msg_send![
                user_script,
                initWithSource: boot_js
                injectionTime: 0isize // WKUserScriptInjectionTimeAtDocumentStart
                forMainFrameOnly: NO
            ];
            let _: () = msg_send![boot_js, release];
            if !user_script.is_null() {
                let _: () = msg_send![ucc, addUserScript: user_script];
                let _: () = msg_send![user_script, release];
            }

            let handler: *mut Object = msg_send![handler_cls, new];
            if handler.is_null() {
                let _: () = msg_send![ucc, release];
                let _: () = msg_send![config, release];
                return None;
            }
            let bridge_box = Box::into_raw(Box::new(BridgeState {
                queue: event_queue.clone(),
                notify_tx,
            }));
            (*handler).set_ivar(BRIDGE_IVAR_NAME, bridge_box as *mut c_void);

            let handler_name = alloc_nsstring(SCRIPT_HANDLER_NAME);
            let _: () = msg_send![ucc, addScriptMessageHandler: handler name: handler_name];
            let _: () = msg_send![handler_name, release];

            let init_rect = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize {
                    width: 480.0,
                    height: 540.0,
                },
            };
            let webview: *mut Object = msg_send![wk_cls, alloc];
            let webview: *mut Object = msg_send![webview, initWithFrame: init_rect configuration: config];
            let _: () = msg_send![config, release];
            if webview.is_null() {
                let name_rm = alloc_nsstring(SCRIPT_HANDLER_NAME);
                let _: () = msg_send![ucc, removeScriptMessageHandlerForName: name_rm];
                let _: () = msg_send![name_rm, release];
                let _: () = msg_send![handler, release];
                let _: () = msg_send![ucc, release];
                return None;
            }

            let draws_bg_key = alloc_nsstring("drawsBackground");
            let no_num: *mut Object = msg_send![class!(NSNumber), numberWithBool: NO];
            let _: () = msg_send![webview, setValue: no_num forKey: draws_bg_key];
            let _: () = msg_send![draws_bg_key, release];

            let _: () = msg_send![webview, setHidden: YES];
            let _: () = msg_send![parent_ns_view, addSubview: webview];

            Some((
                Self {
                    parent_ns_view,
                    webview,
                    user_content_controller: ucc,
                    script_handler: handler,
                    event_queue,
                    loaded_url: String::new(),
                    is_hidden: true,
                    last_frame: None,
                    last_zoom: None,
                },
                notify_rx,
            ))
        }
    }

    pub fn matches_window(&self, window: &Window) -> bool {
        let Ok(win_handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        match win_handle.as_raw() {
            RawWindowHandle::AppKit(appkit) => {
                (appkit.ns_view.as_ptr() as *mut Object) == self.parent_ns_view
            }
            _ => false,
        }
    }

    pub fn load_url(&mut self, url: &str) {
        let trimmed = url.trim();
        if trimmed.is_empty() || self.loaded_url == trimmed || self.webview.is_null() {
            return;
        }
        unsafe {
            let ns_url_str = alloc_nsstring(trimmed);
            let ns_url: *mut Object = msg_send![class!(NSURL), URLWithString: ns_url_str];
            let _: () = msg_send![ns_url_str, release];
            if ns_url.is_null() {
                return;
            }
            let req: *mut Object = msg_send![class!(NSURLRequest), requestWithURL: ns_url];
            if !req.is_null() {
                let _: *mut Object = msg_send![self.webview, loadRequest: req];
                self.loaded_url = trimmed.to_string();
            }
        }
    }

    pub fn loaded_url(&self) -> &str {
        &self.loaded_url
    }

    pub fn reload(&mut self) {
        if self.webview.is_null() {
            return;
        }
        unsafe {
            let _: *mut Object = msg_send![self.webview, reload];
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.is_hidden
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        if self.webview.is_null() || self.is_hidden == hidden {
            return;
        }
        self.is_hidden = hidden;
        unsafe {
            let flag: BOOL = if hidden { YES } else { NO };
            let _: () = msg_send![self.webview, setHidden: flag];
        }
    }

    /// Parks the WebView at a 1x1 near-transparent live frame during Floating PiP mode so
    /// macOS does not throttle JS timers/WASM while hidden from the main full-screen overlay.
    pub fn park_for_pip(&mut self) {
        if self.webview.is_null() || self.parent_ns_view.is_null() {
            return;
        }
        unsafe {
            let rect = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize {
                    width: 1.0,
                    height: 1.0,
                },
            };
            let _: () = msg_send![self.webview, setFrame: rect];
            let alpha: f64 = 0.01;
            let _: () = msg_send![self.webview, setAlphaValue: alpha];
            let flag: BOOL = NO;
            let _: () = msg_send![self.webview, setHidden: flag];
            self.is_hidden = false;
            self.last_frame = Some((0.0, 0.0, 1.0, 1.0));
        }
    }

    /// Synchronizes child `WKWebView` frame and visibility against GPUI canvas `bounds` (top-left origin).
    pub fn sync_bounds(&mut self, bounds: Bounds<Pixels>, visible: bool) {
        if self.webview.is_null() || self.parent_ns_view.is_null() {
            return;
        }
        let w = f64::from(f32::from(bounds.size.width));
        let h = f64::from(f32::from(bounds.size.height));
        if !visible || w < 2.0 || h < 2.0 {
            self.set_hidden(true);
            return;
        }
        unsafe {
            let parent_bounds: CgRect = msg_send![self.parent_ns_view, bounds];
            let x = parent_bounds.origin.x + f64::from(f32::from(bounds.origin.x));
            let y = (parent_bounds.origin.y + parent_bounds.size.height
                - f64::from(f32::from(bounds.origin.y))
                - h)
                .max(0.0);
            let next_tuple = (x, y, w, h);
            if self.last_frame != Some(next_tuple) {
                let rect = CgRect {
                    origin: CgPoint { x, y },
                    size: CgSize {
                        width: w,
                        height: h,
                    },
                };
                let _: () = msg_send![self.webview, setFrame: rect];
                self.last_frame = Some(next_tuple);
            }
            let alpha: f64 = 1.0;
            let _: () = msg_send![self.webview, setAlphaValue: alpha];
        }
        self.set_hidden(false);
    }

    pub fn set_page_zoom(&mut self, zoom: f64) {
        if self.webview.is_null() || self.last_zoom == Some(zoom) {
            return;
        }
        unsafe {
            let responds: BOOL = msg_send![self.webview, respondsToSelector: sel!(setPageZoom:)];
            if responds != NO {
                let _: () = msg_send![self.webview, setPageZoom: zoom];
                self.last_zoom = Some(zoom);
            }
        }
    }

    /// Dispatches a Telegram Mini App event back into the loaded `WKWebView`.
    pub fn dispatch_telegram_event(&self, event_type: &str, event_data_json: &str) {
        if self.webview.is_null() {
            return;
        }
        let Ok(ev_lit) = serde_json::to_string(event_type) else {
            return;
        };
        let js = format!(
            "(function(){{try{{if(window.Telegram&&window.Telegram.WebView&&typeof window.Telegram.WebView.receiveEvent==='function'){{window.Telegram.WebView.receiveEvent({ev_lit},{event_data_json});}}else if(window.TelegramGameProxy&&typeof window.TelegramGameProxy.receiveEvent==='function'){{window.TelegramGameProxy.receiveEvent({ev_lit},{event_data_json});}}}}catch(_){{}}}})();"
        );
        self.evaluate_js(&js);
    }

    pub fn evaluate_js(&self, js: &str) {
        if self.webview.is_null() {
            return;
        }
        unsafe {
            let script = alloc_nsstring(js);
            let _: () = msg_send![
                self.webview,
                evaluateJavaScript: script
                completionHandler: ptr::null_mut::<c_void>()
            ];
            let _: () = msg_send![script, release];
        }
    }

    pub fn drain_events(&self) -> Vec<(String, String)> {
        let Ok(mut q) = self.event_queue.lock() else {
            return Vec::new();
        };
        q.drain(..).collect()
    }

    pub fn destroy(&mut self) {
        if self.webview.is_null() {
            return;
        }
        unsafe {
            if !self.user_content_controller.is_null() {
                let name = alloc_nsstring(SCRIPT_HANDLER_NAME);
                let _: () = msg_send![
                    self.user_content_controller,
                    removeScriptMessageHandlerForName: name
                ];
                let _: () = msg_send![name, release];
                let _: () = msg_send![self.user_content_controller, release];
                self.user_content_controller = ptr::null_mut();
            }
            let _: () = msg_send![self.webview, stopLoading];
            let _: () = msg_send![self.webview, removeFromSuperview];
            let _: () = msg_send![self.webview, release];
            self.webview = ptr::null_mut();
            if !self.script_handler.is_null() {
                let _: () = msg_send![self.script_handler, release];
                self.script_handler = ptr::null_mut();
            }
        }
        self.parent_ns_view = ptr::null_mut();
        self.loaded_url.clear();
        self.last_frame = None;
        self.last_zoom = None;
    }
}

impl Drop for MacOsWkWebView {
    fn drop(&mut self) {
        self.destroy();
    }
}
