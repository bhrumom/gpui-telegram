//! Native macOS `WKWebView` child view host, `TelegramWebviewProxy` JS bridge,
//! generalized `EmbeddedWkWebView`, and `WkWebViewBackend` (`BrowserSessionBackend`).

#![cfg(target_os = "macos")]
#![allow(unexpected_cfgs)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_void, CStr};
use std::future::Future;
use std::os::raw::c_char;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use bot_browser::{
    ActionOutcome, BrowserBackendKind, BrowserCookie, BrowserPerformanceMetrics,
    BrowserSessionBackend, BrowserSnapshot, DomNodeSnapshot, ElementRect, KeyEventKind,
    KeyInputEvent, NavigationResult, NormalizedOrigin, PointerButton, PointerEventKind,
    PointerInputEvent, SameSitePolicy, SemanticLocator, StepTimingMetric,
};
use bot_browser::types::BotBrowserError;
use chrono::Utc;
use gpui::{Bounds, Pixels, Window};
use objc::declare::ClassDecl;
use objc::rc::autoreleasepool;
use objc::runtime::{Class, Object, Protocol, Sel, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};
use parking_lot::RwLock;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::{oneshot, Notify};

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFBooleanGetTypeID() -> usize;
    fn CFGetTypeID(cf: *const c_void) -> usize;
}

unsafe extern "C" {
    static _NSConcreteStackBlock: usize;
    fn _Block_copy(aBlock: *const c_void) -> *mut c_void;
    fn _Block_release(aBlock: *const c_void);

    static _dispatch_main_q: usize;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

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
const RECORDER_HANDLER_NAME: &str = "gpuiBrowserRecorder";
const BRIDGE_IVAR_NAME: &str = "bridgeState";
const SHARED_IVAR_NAME: &str = "sharedState";
const MAX_RECENT_STEP_METRICS: usize = 256;
const DEFAULT_NAV_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_OP_TIMEOUT: Duration = Duration::from_secs(15);
const BLOCK_HAS_COPY_DISPOSE: i32 = 1 << 25;

static NEXT_WEBVIEW_ID: AtomicU64 = AtomicU64::new(1);
static KVO_CONTEXT_TAG: u8 = 0;

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

pub const WK_DOM_RECORDER_JS: &str = r#"
(function() {
  if (window.__gpuiDomRecorderInstalled) return;
  window.__gpuiDomRecorderInstalled = true;
  if (typeof window.__gpuiDomRecorderEnabled !== 'boolean') {
    window.__gpuiDomRecorderEnabled = false;
  }

  function hasSensitiveMarker(raw) {
    if (!raw) return false;
    var spaced = String(raw)
      .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, ' ')
      .trim();
    if (!spaced) return false;
    var compact = spaced.replace(/\s+/g, '');
    var spacedMarkers = [
      'password', 'passwd', 'passcode', 'one time code', 'totp',
      'secret', 'api key', 'private key', 'secret key',
      'access token', 'auth token', 'session id',
      'authorization', 'credential', 'mnemonic', 'ct0'
    ];
    for (var i = 0; i < spacedMarkers.length; i++) {
      if (spaced.indexOf(spacedMarkers[i]) !== -1) return true;
    }
    var compactMarkers = [
      'apikey', 'privatekey', 'secretkey', 'accesstoken',
      'authtoken', 'sessionid', 'onetimecode'
    ];
    for (var j = 0; j < compactMarkers.length; j++) {
      if (compact.indexOf(compactMarkers[j]) !== -1) return true;
    }
    var tokens = spaced.split(' ');
    for (var k = 0; k < tokens.length; k++) {
      var tok = tokens[k];
      if (tok === 'pwd' || tok === 'otp' || tok === 'token' || tok === 'pin' || tok === 'cookie' || tok === 'seed' || tok === 'ct0') {
        return true;
      }
    }
    return false;
  }

  function isSensitive(el) {
    if (!el || el.nodeType !== 1) return false;
    var getAttr = function(node, attr) {
      return (node && typeof node.getAttribute === 'function') ? node.getAttribute(attr) : null;
    };
    var t = String(getAttr(el, 'type') || el.type || '').toLowerCase();
    if (t === 'password') return true;
    var ac = String(getAttr(el, 'autocomplete') || '').toLowerCase();
    if (ac.indexOf('password') !== -1 || ac === 'one-time-code') return true;
    var ownAttrs = [
      ac,
      getAttr(el, 'name'),
      getAttr(el, 'id'),
      getAttr(el, 'aria-label'),
      getAttr(el, 'placeholder'),
      getAttr(el, 'data-testid'),
      getAttr(el, 'role')
    ];
    for (var i = 0; i < ownAttrs.length; i++) {
      if (hasSensitiveMarker(ownAttrs[i])) return true;
    }
    var eid = getAttr(el, 'id');
    if (eid && typeof document !== 'undefined' && document.querySelectorAll) {
      var labels = document.querySelectorAll('label[for]');
      for (var li = 0; li < labels.length; li++) {
        if (labels[li].getAttribute('for') === eid) {
          var ltxt = labels[li].innerText || labels[li].textContent || getAttr(labels[li], 'aria-label');
          if (hasSensitiveMarker(ltxt)) return true;
        }
      }
    }
    var labelledBy = getAttr(el, 'aria-labelledby');
    if (labelledBy && typeof document !== 'undefined' && document.getElementById) {
      var refIds = String(labelledBy).trim().split(/\s+/);
      for (var ri = 0; ri < refIds.length; ri++) {
        if (!refIds[ri]) continue;
        var refEl = document.getElementById(refIds[ri]);
        if (refEl) {
          var rtxt = refEl.innerText || refEl.textContent || getAttr(refEl, 'aria-label');
          if (hasSensitiveMarker(rtxt)) return true;
        }
      }
    }
    var anc = el.parentElement;
    var depth = 0;
    while (anc && anc.nodeType === 1 && depth < 6) {
      var ancTag = String(anc.tagName || '').toLowerCase();
      if (ancTag === 'body' || ancTag === 'html') break;
      var ancAttrs = [
        getAttr(anc, 'data-testid'),
        getAttr(anc, 'aria-label'),
        getAttr(anc, 'id'),
        getAttr(anc, 'name'),
        getAttr(anc, 'role')
      ];
      for (var ai = 0; ai < ancAttrs.length; ai++) {
        if (hasSensitiveMarker(ancAttrs[ai])) return true;
      }
      if (ancTag === 'label') {
        var wrapTxt = anc.innerText || anc.textContent;
        if (hasSensitiveMarker(wrapTxt)) return true;
      }
      anc = anc.parentElement;
      depth++;
    }
    return false;
  }

  function implicitRole(el) {
    var explicit = el.getAttribute && el.getAttribute('role');
    if (explicit) return String(explicit).toLowerCase();
    var tag = String(el.tagName || '').toLowerCase();
    if (tag === 'a' && el.hasAttribute && el.hasAttribute('href')) return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'select') return 'combobox';
    if (tag === 'input') {
      var tp = String((el.getAttribute && el.getAttribute('type')) || 'text').toLowerCase();
      if (tp === 'submit' || tp === 'button' || tp === 'reset') return 'button';
      if (tp === 'checkbox') return 'checkbox';
      if (tp === 'radio') return 'radio';
      if (tp !== 'hidden') return 'textbox';
    }
    if (/^h[1-6]$/.test(tag)) return 'heading';
    return '';
  }

  function cssEscapeIdent(val) {
    try {
      if (window.CSS && typeof CSS.escape === 'function') return CSS.escape(val);
    } catch (_) {}
    return String(val).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
  }

  function isUniqueSelector(sel, el) {
    if (!sel || typeof document === 'undefined' || !document.querySelectorAll) return false;
    try {
      var list = document.querySelectorAll(sel);
      return list.length === 1 && list[0] === el;
    } catch (_) {
      return false;
    }
  }

  function cssPath(el) {
    if (!el || el.nodeType !== 1) return '';
    if (el.id) {
      var idSel = '#' + cssEscapeIdent(el.id);
      if (isUniqueSelector(idSel, el)) return idSel;
    }
    var tid = el.getAttribute && el.getAttribute('data-testid');
    if (tid) {
      var tidSel = '[data-testid="' + String(tid).replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"]';
      if (isUniqueSelector(tidSel, el)) return tidSel;
    }
    var nm = el.getAttribute && el.getAttribute('name');
    var tag = String(el.tagName || 'div').toLowerCase();
    if (nm) {
      var nmSel = tag + '[name="' + String(nm).replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"]';
      if (isUniqueSelector(nmSel, el)) return nmSel;
    }
    var parts = [];
    var cur = el;
    while (cur && cur.nodeType === 1 && parts.length < 16) {
      var t = String(cur.tagName || 'div').toLowerCase();
      if (cur !== el && cur.id) {
        var ancIdSel = '#' + cssEscapeIdent(cur.id);
        if (isUniqueSelector(ancIdSel, cur)) {
          parts.unshift(ancIdSel);
          var joinedWithId = parts.join(' > ');
          if (isUniqueSelector(joinedWithId, el)) return joinedWithId;
          break;
        }
      }
      var idx = 1;
      var sib = cur.previousElementSibling;
      while (sib) {
        if (sib.tagName === cur.tagName) idx++;
        sib = sib.previousElementSibling;
      }
      parts.unshift(t + ':nth-of-type(' + idx + ')');
      var candidate = parts.join(' > ');
      if (isUniqueSelector(candidate, el)) return candidate;
      if (t === 'html') break;
      cur = cur.parentElement;
    }
    return parts.join(' > ');
  }

  function buildLocator(el) {
    var tid = el.getAttribute ? el.getAttribute('data-testid') : null;
    var aria = el.getAttribute ? el.getAttribute('aria-label') : null;
    var role = implicitRole(el) || null;
    var nameAttr = el.getAttribute ? el.getAttribute('name') : null;
    var rawText = String(el.innerText || el.textContent || (el.getAttribute && el.getAttribute('placeholder')) || '').replace(/\s+/g, ' ').trim();
    var textAnchor = rawText ? rawText.slice(0, 80) : null;
    var css = cssPath(el) || null;
    return {
      test_id: tid || null,
      aria_label: aria || null,
      role: role,
      text_anchor: textAnchor,
      css_selector: css,
      name_attr: nameAttr || null
    };
  }

  function emit(kind, el) {
    if (!window.__gpuiDomRecorderEnabled) return;
    if (!el || el.nodeType !== 1) return;
    var sensitive = isSensitive(el);
    var value = null;
    if (kind === 'input' || kind === 'change') {
      if (sensitive) {
        value = '[REDACTED]';
      } else if (typeof el.value === 'string') {
        value = el.value.slice(0, 500);
      } else if (el.isContentEditable) {
        value = String(el.innerText || el.textContent || '').slice(0, 500);
      }
    }
    var loc = buildLocator(el);
    if (sensitive && loc.text_anchor) {
      loc.text_anchor = '[REDACTED]';
    }
    var payload = {
      event_kind: kind,
      locator: loc,
      value: value,
      is_sensitive: !!sensitive,
      page_url: String((window.location && window.location.href) || ''),
      timestamp_ms: Date.now()
    };
    try {
      if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.gpuiBrowserRecorder) {
        window.webkit.messageHandlers.gpuiBrowserRecorder.postMessage(payload);
      }
    } catch (_) {}
  }

  function findInteractiveTarget(node) {
    var cur = node;
    while (cur && cur.nodeType !== 1) cur = cur.parentElement;
    while (cur && cur.nodeType === 1) {
      var tag = String(cur.tagName || '').toLowerCase();
      if (
        tag === 'button' || tag === 'a' || tag === 'input' || tag === 'select' || tag === 'textarea' ||
        (cur.getAttribute && (cur.getAttribute('data-testid') || cur.getAttribute('role') || cur.getAttribute('aria-label'))) ||
        cur.isContentEditable
      ) {
        return cur;
      }
      cur = cur.parentElement;
    }
    return node && node.nodeType === 1 ? node : null;
  }

  document.addEventListener('click', function(ev) {
    var t = findInteractiveTarget(ev.target);
    if (t) emit('click', t);
  }, true);
  document.addEventListener('input', function(ev) {
    var t = ev.target && ev.target.nodeType === 1 ? ev.target : null;
    if (t) emit('input', t);
  }, true);
  document.addEventListener('change', function(ev) {
    var t = ev.target && ev.target.nodeType === 1 ? ev.target : null;
    if (t) emit('change', t);
  }, true);
  document.addEventListener('submit', function(ev) {
    var t = ev.target && ev.target.nodeType === 1 ? ev.target : null;
    if (t) emit('submit', t);
  }, true);
})();
"#;

/// Returns `true` when running inside headless/offline tests where native AppKit child views must not be attached.
pub fn is_headless_or_offline_test_env() -> bool {
    std::env::var("GPUI_OFFLINE_TEST").is_ok()
        || std::env::var("GPUI_MINIAPP_HEADLESS_CHILD").is_ok()
}

/// Configuration for attaching an [`EmbeddedWkWebView`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WkWebViewConfig {
    pub profile_id: Option<String>,
    pub ephemeral: bool,
    pub inject_telegram_proxy: bool,
    pub enable_dom_recorder: bool,
    pub load_popups_in_same_tab: bool,
    pub allow_file_urls: bool,
}

impl WkWebViewConfig {
    pub fn miniapp_default() -> Self {
        Self {
            profile_id: None,
            ephemeral: false,
            inject_telegram_proxy: true,
            enable_dom_recorder: false,
            load_popups_in_same_tab: false,
            allow_file_urls: false,
        }
    }

    pub fn browser_profile(profile_id: impl Into<String>) -> Self {
        let trimmed = profile_id.into().trim().to_string();
        Self {
            profile_id: if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            },
            ephemeral: false,
            inject_telegram_proxy: false,
            enable_dom_recorder: true,
            load_popups_in_same_tab: true,
            allow_file_urls: false,
        }
    }
}

impl Default for WkWebViewConfig {
    fn default() -> Self {
        Self::miniapp_default()
    }
}

/// Semantic DOM event recorded by `WK_DOM_RECORDER_JS` inside `WKWebView`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordedDomEvent {
    pub event_kind: String,
    pub locator: SemanticLocator,
    pub value: Option<String>,
    pub is_sensitive: bool,
    pub page_url: String,
    pub timestamp_ms: i64,
}

/// Events emitted from an [`EmbeddedWkWebView`] to GPUI subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WkWebViewEvent {
    BridgeMessage {
        event_type: String,
        event_data: String,
    },
    NavigationChanged {
        url: String,
        can_go_back: bool,
        can_go_forward: bool,
    },
    TitleChanged {
        title: String,
    },
    LoadingChanged {
        is_loading: bool,
    },
    NavigationFailed {
        url: String,
        error: String,
    },
    PopupRequested {
        url: String,
    },
    RecordedDomAction(RecordedDomEvent),
    BecameFirstResponder,
}

fn normalize_sensitivity_text(raw: &str) -> (String, String) {
    let mut spaced = String::with_capacity(raw.len() + 8);
    let mut prev_is_lower_or_digit = false;
    let mut prev_was_space = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && prev_is_lower_or_digit && !prev_was_space {
                spaced.push(' ');
            }
            spaced.push(ch.to_ascii_lowercase());
            prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
            prev_was_space = false;
        } else {
            if !prev_was_space {
                spaced.push(' ');
                prev_was_space = true;
            }
            prev_is_lower_or_digit = false;
        }
    }
    let trimmed = spaced.trim().to_string();
    let compact: String = trimmed.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    (trimmed, compact)
}

pub fn has_sensitive_marker_text(raw: &str) -> bool {
    let (spaced, compact) = normalize_sensitivity_text(raw);
    if spaced.is_empty() {
        return false;
    }
    const SPACED_MARKERS: &[&str] = &[
        "password",
        "passwd",
        "passcode",
        "one time code",
        "totp",
        "secret",
        "api key",
        "private key",
        "secret key",
        "access token",
        "auth token",
        "session id",
        "authorization",
        "credential",
        "mnemonic",
        "ct0",
    ];
    if SPACED_MARKERS.iter().any(|m| spaced.contains(m)) {
        return true;
    }
    const COMPACT_MARKERS: &[&str] = &[
        "apikey",
        "privatekey",
        "secretkey",
        "accesstoken",
        "authtoken",
        "sessionid",
        "onetimecode",
    ];
    if COMPACT_MARKERS.iter().any(|m| compact.contains(m)) {
        return true;
    }
    spaced.split(' ').any(|tok| {
        matches!(
            tok,
            "pwd" | "otp" | "token" | "pin" | "cookie" | "seed" | "ct0"
        )
    })
}

pub fn is_sensitive_locator_hint(locator: &SemanticLocator) -> bool {
    if locator.is_sensitive_hint() {
        return true;
    }
    [
        locator.test_id.as_deref(),
        locator.aria_label.as_deref(),
        locator.role.as_deref(),
        locator.text_anchor.as_deref(),
        locator.css_selector.as_deref(),
        locator.name_attr.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(has_sensitive_marker_text)
}

pub fn parse_recorded_dom_event(val: &Value) -> Option<RecordedDomEvent> {
    let mut parsed: RecordedDomEvent = serde_json::from_value(val.clone()).ok()?;
    if parsed.event_kind.trim().is_empty() {
        return None;
    }
    if parsed.is_sensitive || is_sensitive_locator_hint(&parsed.locator) {
        parsed.is_sensitive = true;
        if parsed.value.is_some()
            || parsed.event_kind == "input"
            || parsed.event_kind == "change"
        {
            parsed.value = Some("[REDACTED]".to_string());
        }
        if parsed.locator.text_anchor.is_some() {
            parsed.locator.text_anchor = Some("[REDACTED]".to_string());
        }
    }
    Some(parsed)
}

/// Computes the `NSHTTPCookieDomain` string for a normalized [`BrowserCookie`].
///
/// When `host_only` is `false`, `WKHTTPCookieStore` / `NSHTTPCookie` requires a leading `.`
/// so exported cookies preserve `host_only == false` and subdomain matching. When `host_only`
/// is `true`, any leading `.` is stripped.
pub fn wk_ns_cookie_domain(cookie: &BrowserCookie) -> String {
    let trimmed = cookie.domain.trim().to_ascii_lowercase();
    let bare = trimmed.trim_start_matches('.');
    if cookie.host_only {
        bare.to_string()
    } else if bare.is_empty() {
        String::new()
    } else {
        format!(".{bare}")
    }
}

pub fn validate_and_normalize_wk_cookie(
    cookie: &mut BrowserCookie,
) -> Result<(), BotBrowserError> {
    if cookie
        .name
        .chars()
        .any(|c| c.is_ascii_control() || matches!(c, '\r' | '\n' | '\0' | ';' | '='))
    {
        return Err(BotBrowserError::CookieImport(
            "cookie name contains disallowed control characters".to_string(),
        ));
    }
    let trimmed_name = cookie.name.trim();
    if trimmed_name.is_empty() {
        return Err(BotBrowserError::CookieImport(
            "cookie name cannot be empty".to_string(),
        ));
    }
    cookie.name = trimmed_name.to_string();

    if cookie
        .value
        .chars()
        .any(|c| c.is_ascii_control() || matches!(c, '\r' | '\n' | '\0' | ';'))
    {
        return Err(BotBrowserError::CookieImport(format!(
            "cookie '{}' value contains disallowed control characters",
            cookie.name
        )));
    }

    let trimmed_domain = cookie.domain.trim().to_ascii_lowercase();
    if trimmed_domain.is_empty()
        || trimmed_domain
            .chars()
            .any(|c| c.is_whitespace() || c.is_ascii_control() || matches!(c, '\0' | ';'))
    {
        return Err(BotBrowserError::CookieImport(format!(
            "cookie '{}' has invalid domain",
            cookie.name
        )));
    }
    cookie.domain = trimmed_domain;

    if cookie
        .path
        .chars()
        .any(|c| c.is_ascii_control() || matches!(c, '\r' | '\n' | '\0' | ';'))
    {
        return Err(BotBrowserError::CookieImport(format!(
            "cookie '{}' path contains disallowed control characters or ';'",
            cookie.name
        )));
    }
    let trimmed_path = cookie.path.trim();
    if trimmed_path.is_empty() {
        cookie.path = "/".to_string();
    } else if !trimmed_path.starts_with('/') {
        cookie.path = format!("/{trimmed_path}");
    } else {
        cookie.path = trimmed_path.to_string();
    }

    if cookie.expires.is_some_and(|e| e <= 0) {
        cookie.expires = None;
    }
    cookie.session = cookie.expires.is_none();

    Ok(())
}

fn is_blocked_metadata_host(raw_host: &str) -> bool {
    let trimmed = raw_host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split('%')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();

    if trimmed == "metadata.google.internal"
        || trimmed.ends_with(".metadata.google.internal")
        || trimmed == "metadata.goog"
        || trimmed.ends_with(".metadata.goog")
        || trimmed == "metadata"
        || trimmed == "169.254.169.254"
    {
        return true;
    }

    if let Ok(ip) = trimmed.parse::<std::net::IpAddr>() {
        let v4_opt = match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(v6) => {
                let segs = v6.segments();
                if segs[0] == 0xfd00 && segs[1] == 0x0ec2 {
                    return true;
                }
                v6.to_ipv4_mapped()
            }
        };
        if let Some(v4) = v4_opt {
            if v4.is_link_local() {
                return true;
            }
        }
    }

    false
}

pub fn validate_wk_navigation_url(
    url: &str,
    allow_file_urls: bool,
) -> Result<Option<NormalizedOrigin>, BotBrowserError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(BotBrowserError::InvalidOrigin(
            "navigation URL cannot be empty".to_string(),
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("about:") || lower.starts_with("blob:") || lower.starts_with("data:") {
        return Ok(None);
    }
    if lower.starts_with("file://") {
        if allow_file_urls {
            return Ok(None);
        }
        return Err(BotBrowserError::InvalidOrigin(
            "file:// navigation is disabled for this WebView".to_string(),
        ));
    }
    let origin = NormalizedOrigin::parse(trimmed)?;
    if is_blocked_metadata_host(origin.host()) {
        return Err(BotBrowserError::InvalidOrigin(
            "blocked cloud metadata endpoint URL".to_string(),
        ));
    }
    Ok(Some(origin))
}

fn deterministic_profile_uuid_string(profile_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gpui_telegram_wkwebview_profile:");
    hasher.update(profile_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Set UUID v5-compatible version (5) and RFC4122 variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub struct SharedWebViewState {
    pub id: u64,
    pub alive: AtomicBool,
    pub current_url: RwLock<String>,
    pub page_title: RwLock<String>,
    pub is_loading: AtomicBool,
    pub can_go_back: AtomicBool,
    pub can_go_forward: AtomicBool,
    pub nav_seq: AtomicU64,
    pub last_nav_error: RwLock<Option<String>>,
    pub nav_notify: Notify,
    pub dom_recording_enabled: AtomicBool,
    pub allow_file_urls: AtomicBool,
    pub load_popups_in_same_tab: AtomicBool,
    pub allow_first_responder: AtomicBool,
    pub ignore_hit_test: AtomicBool,
    pub became_first_responder: AtomicBool,
    pub event_queue: Arc<Mutex<VecDeque<WkWebViewEvent>>>,
    pub wakeup_tx: UnboundedSender<()>,
}

impl SharedWebViewState {
    fn new(id: u64, config: &WkWebViewConfig, wakeup_tx: UnboundedSender<()>) -> Self {
        Self {
            id,
            alive: AtomicBool::new(true),
            current_url: RwLock::new(String::new()),
            page_title: RwLock::new(String::new()),
            is_loading: AtomicBool::new(false),
            can_go_back: AtomicBool::new(false),
            can_go_forward: AtomicBool::new(false),
            nav_seq: AtomicU64::new(0),
            last_nav_error: RwLock::new(None),
            nav_notify: Notify::new(),
            dom_recording_enabled: AtomicBool::new(config.enable_dom_recorder),
            allow_file_urls: AtomicBool::new(config.allow_file_urls),
            load_popups_in_same_tab: AtomicBool::new(config.load_popups_in_same_tab),
            allow_first_responder: AtomicBool::new(false),
            ignore_hit_test: AtomicBool::new(true),
            became_first_responder: AtomicBool::new(false),
            event_queue: Arc::new(Mutex::new(VecDeque::new())),
            wakeup_tx,
        }
    }

    pub fn new_detached_closed_for_test() -> Arc<Self> {
        let (tx, _rx) = mpsc::unbounded_channel();
        let state = Self::new(
            NEXT_WEBVIEW_ID.fetch_add(1, Ordering::Relaxed),
            &WkWebViewConfig::browser_profile("test-closed"),
            tx,
        );
        state.alive.store(false, Ordering::Release);
        Arc::new(state)
    }

    fn push_event(&self, ev: WkWebViewEvent) {
        if let Ok(mut q) = self.event_queue.lock() {
            q.push_back(ev);
        }
        let _ = self.wakeup_tx.send(());
    }
}

struct MainThreadWebViewHandles {
    webview: *mut Object,
    cookie_store: *mut Object,
    user_content_controller: *mut Object,
    script_handler: *mut Object,
    recorder_handler: *mut Object,
    nav_delegate: *mut Object,
    ui_delegate: *mut Object,
    kvo_observer: *mut Object,
}

thread_local! {
    static WEBVIEW_REGISTRY: RefCell<HashMap<u64, MainThreadWebViewHandles>> =
        RefCell::new(HashMap::new());
}

extern "C" fn main_queue_trampoline(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    autoreleasepool(|| {
        let boxed: Box<Box<dyn FnOnce() + Send>> =
            unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
        (*boxed)();
    });
}

fn run_on_main_thread<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    let is_main: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
    if is_main == YES {
        autoreleasepool(|| f());
        return;
    }
    let outer: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    let raw = Box::into_raw(outer) as *mut c_void;
    unsafe {
        dispatch_async_f(
            (&raw const _dispatch_main_q) as *const c_void,
            raw,
            main_queue_trampoline,
        );
    }
}

#[repr(C)]
struct ObjcBlockLayout {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
}

#[repr(C)]
struct BlockDescriptorRaw {
    reserved: usize,
    size: usize,
    copy_helper: unsafe extern "C" fn(*mut c_void, *const c_void),
    dispose_helper: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
struct RustCompletionBlock<S: CompletionBlockState> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: *const BlockDescriptorRaw,
    state: *const S,
}

trait CompletionBlockState: Sized + Send + Sync + 'static {
    const DESCRIPTOR: BlockDescriptorRaw = BlockDescriptorRaw {
        reserved: 0,
        size: std::mem::size_of::<RustCompletionBlock<Self>>(),
        copy_helper: block_copy_helper::<Self>,
        dispose_helper: block_dispose_helper::<Self>,
    };
}

unsafe extern "C" fn block_copy_helper<S: CompletionBlockState>(
    _dst: *mut c_void,
    src: *const c_void,
) {
    let src_blk = unsafe { &*(src as *const RustCompletionBlock<S>) };
    if !src_blk.state.is_null() {
        unsafe { Arc::increment_strong_count(src_blk.state) };
    }
}

unsafe extern "C" fn block_dispose_helper<S: CompletionBlockState>(blk_ptr: *mut c_void) {
    let blk = unsafe { &mut *(blk_ptr as *mut RustCompletionBlock<S>) };
    if !blk.state.is_null() {
        let ptr = blk.state;
        blk.state = ptr::null();
        unsafe { drop(Arc::from_raw(ptr)) };
    }
}

struct OneshotBlockState<T: Send + 'static> {
    tx: Mutex<Option<oneshot::Sender<T>>>,
}

impl<T: Send + Sync + 'static> OneshotBlockState<T> {
    fn new(tx: oneshot::Sender<T>) -> Self {
        Self {
            tx: Mutex::new(Some(tx)),
        }
    }

    fn send(&self, val: T) {
        if let Ok(mut guard) = self.tx.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(val);
            }
        }
    }
}

impl<T: Send + Sync + 'static> CompletionBlockState for OneshotBlockState<T> {}

unsafe fn with_heap_completion_block<S: CompletionBlockState, F: FnOnce(*const c_void)>(
    state: S,
    invoke_fn: *const c_void,
    call: F,
) {
    let state_arc = Arc::new(state);
    let mut stack_blk = RustCompletionBlock {
        isa: (&raw const _NSConcreteStackBlock) as *const c_void,
        flags: BLOCK_HAS_COPY_DISPOSE,
        reserved: 0,
        invoke: invoke_fn,
        descriptor: (&S::DESCRIPTOR) as *const BlockDescriptorRaw,
        state: Arc::as_ptr(&state_arc),
    };
    let heap_blk = unsafe { _Block_copy(&mut stack_blk as *mut _ as *const c_void) };
    call(heap_blk as *const c_void);
    if !heap_blk.is_null() {
        unsafe { _Block_release(heap_blk as *const c_void) };
    }
    drop(state_arc);
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

unsafe fn nsdata_to_bytes(ns_data: *mut Object) -> Vec<u8> {
    if ns_data.is_null() {
        return Vec::new();
    }
    unsafe {
        let len: usize = msg_send![ns_data, length];
        if len == 0 {
            return Vec::new();
        }
        let ptr: *const u8 = msg_send![ns_data, bytes];
        if ptr.is_null() {
            return Vec::new();
        }
        std::slice::from_raw_parts(ptr, len).to_vec()
    }
}

unsafe fn nsurl_to_rust(ns_url: *mut Object) -> String {
    if ns_url.is_null() {
        return String::new();
    }
    unsafe {
        let abs: *mut Object = msg_send![ns_url, absoluteString];
        nsstring_to_rust(abs)
    }
}

unsafe fn nserror_to_string(err: *mut Object) -> String {
    if err.is_null() {
        return "unknown WebKit error".to_string();
    }
    unsafe {
        let desc: *mut Object = msg_send![err, localizedDescription];
        let mut msg = nsstring_to_rust(desc);
        let user_info: *mut Object = msg_send![err, userInfo];
        if !user_info.is_null() {
            let key = alloc_nsstring("WKJavaScriptExceptionMessage");
            let js_msg: *mut Object = msg_send![user_info, objectForKey: key];
            let _: () = msg_send![key, release];
            if !js_msg.is_null() {
                let extra = nsstring_to_rust(js_msg);
                if !extra.trim().is_empty() {
                    if msg.is_empty() {
                        msg = extra;
                    } else {
                        msg = format!("{msg}: {extra}");
                    }
                }
            }
        }
        if msg.is_empty() {
            "WebKit error".to_string()
        } else {
            msg
        }
    }
}

unsafe fn objc_value_to_json(obj: *mut Object) -> Value {
    if obj.is_null() {
        return Value::Null;
    }
    unsafe {
        let is_null: BOOL = msg_send![obj, isKindOfClass: class!(NSNull)];
        if is_null == YES {
            return Value::Null;
        }
        let is_str: BOOL = msg_send![obj, isKindOfClass: class!(NSString)];
        if is_str == YES {
            return Value::String(nsstring_to_rust(obj));
        }
        let is_num: BOOL = msg_send![obj, isKindOfClass: class!(NSNumber)];
        if is_num == YES {
            if CFGetTypeID(obj as *const c_void) == CFBooleanGetTypeID() {
                let b: BOOL = msg_send![obj, boolValue];
                return Value::Bool(b != NO);
            }
            let objc_type_ptr: *const c_char = msg_send![obj, objCType];
            let objc_type = if objc_type_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(objc_type_ptr).to_str().unwrap_or("")
            };
            if objc_type == "d" || objc_type == "f" {
                let d: f64 = msg_send![obj, doubleValue];
                return serde_json::Number::from_f64(d)
                    .map(Value::Number)
                    .unwrap_or(Value::Null);
            }
            let i: i64 = msg_send![obj, longLongValue];
            return Value::Number(serde_json::Number::from(i));
        }
        let json_cls = class!(NSJSONSerialization);
        let valid: BOOL = msg_send![json_cls, isValidJSONObject: obj];
        if valid == YES {
            let data: *mut Object = msg_send![
                json_cls,
                dataWithJSONObject: obj
                options: 0usize
                error: ptr::null_mut::<*mut Object>()
            ];
            if !data.is_null() {
                let text = nsdata_to_rust(data);
                if let Ok(val) = serde_json::from_str::<Value>(&text) {
                    return val;
                }
            }
        }
        let desc: *mut Object = msg_send![obj, description];
        Value::String(nsstring_to_rust(desc))
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
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                let event_type = val
                    .get("eventType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if event_type.trim().is_empty() {
                    return None;
                }
                let event_data = match val.get("eventData") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => "{}".to_string(),
                };
                return Some((event_type, event_data));
            }
        }
        None
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum ScriptHandlerMode {
    TelegramBridge,
    DomRecorder,
}

struct BridgeState {
    shared: Arc<SharedWebViewState>,
    mode: ScriptHandlerMode,
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
        if !state.shared.alive.load(Ordering::Acquire) {
            return;
        }
        if !state.shared.allow_first_responder.load(Ordering::Acquire) {
            let msg_webview: *mut Object = msg_send![message, webView];
            restore_superview_first_responder_for_webview(msg_webview);
        }
        let body: *mut Object = msg_send![message, body];
        match state.mode {
            ScriptHandlerMode::TelegramBridge => {
                if let Some((event_type, event_data)) = extract_bridge_event(body) {
                    state.shared.push_event(WkWebViewEvent::BridgeMessage {
                        event_type,
                        event_data,
                    });
                }
            }
            ScriptHandlerMode::DomRecorder => {
                if !state.shared.dom_recording_enabled.load(Ordering::Acquire) {
                    return;
                }
                let json_val = objc_value_to_json(body);
                if let Some(ev) = parse_recorded_dom_event(&json_val) {
                    state
                        .shared
                        .push_event(WkWebViewEvent::RecordedDomAction(ev));
                }
            }
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

unsafe fn get_shared_from_ivar<'a>(this: &Object) -> Option<&'a SharedWebViewState> {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar(SHARED_IVAR_NAME);
        if ptr.is_null() {
            None
        } else {
            Some(&*(ptr as *const SharedWebViewState))
        }
    }
}

extern "C" fn shared_ivar_dealloc(this: &mut Object, _cmd: Sel) {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar(SHARED_IVAR_NAME);
        if !ptr.is_null() {
            drop(Arc::from_raw(ptr as *const SharedWebViewState));
            this.set_ivar(SHARED_IVAR_NAME, ptr::null_mut::<c_void>());
        }
        let _: () = msg_send![super(this, class!(NSObject)), dealloc];
    }
}

/// Ensures the host GPUI `NSView` owns `NSWindow.firstResponder` so keyboard and macOS IME
/// events (`keyDown:`, `NSTextInputClient`) route back to GPUI after native `WKWebView` activity.
pub fn restore_gpui_first_responder(window: &Window) {
    if is_headless_or_offline_test_env() {
        return;
    }
    let Ok(win_handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let parent_ns_view = match win_handle.as_raw() {
        RawWindowHandle::AppKit(appkit) => appkit.ns_view.as_ptr() as *mut Object,
        _ => return,
    };
    if parent_ns_view.is_null() {
        return;
    }
    unsafe {
        let ns_window: *mut Object = msg_send![parent_ns_view, window];
        if ns_window.is_null() {
            return;
        }
        let current_fr: *mut Object = msg_send![ns_window, firstResponder];
        if current_fr != parent_ns_view {
            let _: BOOL = msg_send![ns_window, makeFirstResponder: parent_ns_view];
        }
    }
}

unsafe fn restore_parent_first_responder_if_owned_by_webview(
    parent_ns_view: *mut Object,
    webview: *mut Object,
) {
    if parent_ns_view.is_null() {
        return;
    }
    unsafe {
        let ns_window: *mut Object = msg_send![parent_ns_view, window];
        if ns_window.is_null() {
            return;
        }
        let current_fr: *mut Object = msg_send![ns_window, firstResponder];
        if current_fr == parent_ns_view {
            return;
        }
        let should_restore = if current_fr.is_null() || current_fr == ns_window {
            true
        } else if !webview.is_null() {
            if current_fr == webview {
                true
            } else {
                let is_ns_view: BOOL = msg_send![current_fr, isKindOfClass: class!(NSView)];
                if is_ns_view != NO {
                    let is_desc: BOOL = msg_send![current_fr, isDescendantOf: webview];
                    is_desc != NO
                } else {
                    false
                }
            }
        } else {
            false
        };
        if should_restore {
            let _: BOOL = msg_send![ns_window, makeFirstResponder: parent_ns_view];
        }
    }
}

unsafe fn restore_superview_first_responder_for_webview(webview: *mut Object) {
    if webview.is_null() {
        return;
    }
    unsafe {
        let superview: *mut Object = msg_send![webview, superview];
        if superview.is_null() {
            return;
        }
        restore_parent_first_responder_if_owned_by_webview(superview, webview);
    }
}

extern "C" fn gpui_wkwebview_accepts_first_responder(this: &Object, _cmd: Sel) -> BOOL {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.allow_first_responder.load(Ordering::Acquire) {
                return NO;
            }
        }
        if let Some(wk_cls) = Class::get("WKWebView") {
            msg_send![super(this, wk_cls), acceptsFirstResponder]
        } else {
            NO
        }
    }
}

extern "C" fn gpui_wkwebview_become_first_responder(this: &Object, _cmd: Sel) -> BOOL {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.allow_first_responder.load(Ordering::Acquire) {
                return NO;
            }
        }
        let accepted: BOOL = if let Some(wk_cls) = Class::get("WKWebView") {
            msg_send![super(this, wk_cls), becomeFirstResponder]
        } else {
            NO
        };
        if accepted != NO {
            if let Some(shared) = get_shared_from_ivar(this) {
                if shared.alive.load(Ordering::Acquire) {
                    shared.became_first_responder.store(true, Ordering::Release);
                    shared.push_event(WkWebViewEvent::BecameFirstResponder);
                }
            }
        }
        accepted
    }
}

extern "C" fn gpui_wkwebview_hit_test(this: &Object, _cmd: Sel, point: CgPoint) -> *mut Object {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if shared.ignore_hit_test.load(Ordering::Acquire) {
                return ptr::null_mut();
            }
        }
        if let Some(wk_cls) = Class::get("WKWebView") {
            msg_send![super(this, wk_cls), hitTest: point]
        } else {
            ptr::null_mut()
        }
    }
}

extern "C" fn gpui_wkwebview_mouse_down(this: &Object, _cmd: Sel, event: *mut Object) {
    unsafe {
        let allow_fr = get_shared_from_ivar(this)
            .map(|s| s.allow_first_responder.load(Ordering::Acquire))
            .unwrap_or(true);
        if let Some(wk_cls) = Class::get("WKWebView") {
            let _: () = msg_send![super(this, wk_cls), mouseDown: event];
        }
        if !allow_fr {
            restore_superview_first_responder_for_webview(this as *const Object as *mut Object);
        }
    }
}

extern "C" fn gpui_wkwebview_dealloc(this: &mut Object, _cmd: Sel) {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar(SHARED_IVAR_NAME);
        if !ptr.is_null() {
            drop(Arc::from_raw(ptr as *const SharedWebViewState));
            this.set_ivar(SHARED_IVAR_NAME, ptr::null_mut::<c_void>());
        }
        if let Some(wk_cls) = Class::get("WKWebView") {
            let _: () = msg_send![super(this, wk_cls), dealloc];
        }
    }
}

fn get_or_register_wkwebview_class() -> Option<&'static Class> {
    static REGISTER_ONCE: Once = Once::new();
    REGISTER_ONCE.call_once(|| {
        if Class::get("GPUIWKWebView").is_some() {
            return;
        }
        let Some(superclass) = Class::get("WKWebView") else {
            return;
        };
        let Some(mut decl) = ClassDecl::new("GPUIWKWebView", superclass) else {
            return;
        };
        decl.add_ivar::<*mut c_void>(SHARED_IVAR_NAME);
        unsafe {
            decl.add_method(
                sel!(acceptsFirstResponder),
                gpui_wkwebview_accepts_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
            );
            decl.add_method(
                sel!(becomeFirstResponder),
                gpui_wkwebview_become_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
            );
            decl.add_method(
                sel!(hitTest:),
                gpui_wkwebview_hit_test as extern "C" fn(&Object, Sel, CgPoint) -> *mut Object,
            );
            decl.add_method(
                sel!(mouseDown:),
                gpui_wkwebview_mouse_down as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(dealloc),
                gpui_wkwebview_dealloc as extern "C" fn(&mut Object, Sel),
            );
        }
        decl.register();
    });
    Class::get("GPUIWKWebView")
}

unsafe fn sync_shared_nav_snapshot(shared: &SharedWebViewState, webview: *mut Object) {
    if webview.is_null() {
        return;
    }
    unsafe {
        let ns_url: *mut Object = msg_send![webview, URL];
        let url_str = nsurl_to_rust(ns_url);
        let ns_title: *mut Object = msg_send![webview, title];
        let title_str = nsstring_to_rust(ns_title);
        let back: BOOL = msg_send![webview, canGoBack];
        let fwd: BOOL = msg_send![webview, canGoForward];
        let loading: BOOL = msg_send![webview, isLoading];

        let back_bool = back != NO;
        let fwd_bool = fwd != NO;
        let loading_bool = loading != NO;

        let prev_back = shared.can_go_back.swap(back_bool, Ordering::AcqRel);
        let prev_fwd = shared.can_go_forward.swap(fwd_bool, Ordering::AcqRel);
        let prev_loading = shared.is_loading.swap(loading_bool, Ordering::AcqRel);

        let url_changed = {
            let mut guard = shared.current_url.write();
            if !url_str.is_empty() && *guard != url_str {
                *guard = url_str.clone();
                true
            } else {
                false
            }
        };
        if url_changed || prev_back != back_bool || prev_fwd != fwd_bool {
            let current = if url_str.is_empty() {
                shared.current_url.read().clone()
            } else {
                url_str
            };
            shared.push_event(WkWebViewEvent::NavigationChanged {
                url: current,
                can_go_back: back_bool,
                can_go_forward: fwd_bool,
            });
        }

        let title_changed = {
            let mut guard = shared.page_title.write();
            if *guard != title_str {
                *guard = title_str.clone();
                true
            } else {
                false
            }
        };
        if title_changed {
            shared.push_event(WkWebViewEvent::TitleChanged { title: title_str });
        }

        if prev_loading != loading_bool {
            shared.push_event(WkWebViewEvent::LoadingChanged {
                is_loading: loading_bool,
            });
        }
    }
}

extern "C" fn nav_decide_policy(
    this: &Object,
    _cmd: Sel,
    _webview: *mut Object,
    navigation_action: *mut Object,
    decision_handler: *mut c_void,
) {
    let mut allow = true;
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !navigation_action.is_null() {
                let req: *mut Object = msg_send![navigation_action, request];
                if !req.is_null() {
                    let ns_url: *mut Object = msg_send![req, URL];
                    let url_str = nsurl_to_rust(ns_url);
                    if !url_str.trim().is_empty() {
                        let allow_file = shared.allow_file_urls.load(Ordering::Acquire);
                        if validate_wk_navigation_url(&url_str, allow_file).is_err() {
                            allow = false;
                        }
                    }
                }
            }
        }
        if !decision_handler.is_null() {
            let blk = decision_handler as *mut ObjcBlockLayout;
            let f: unsafe extern "C" fn(*mut ObjcBlockLayout, isize) =
                std::mem::transmute((*blk).invoke);
            f(blk, if allow { 1 } else { 0 });
        }
    }
}

extern "C" fn nav_did_start(
    this: &Object,
    _cmd: Sel,
    webview: *mut Object,
    _navigation: *mut Object,
) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.alive.load(Ordering::Acquire) {
                return;
            }
            shared.is_loading.store(true, Ordering::Release);
            shared.push_event(WkWebViewEvent::LoadingChanged { is_loading: true });
            sync_shared_nav_snapshot(shared, webview);
        }
    }
}

extern "C" fn nav_did_redirect(
    this: &Object,
    _cmd: Sel,
    webview: *mut Object,
    _navigation: *mut Object,
) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if shared.alive.load(Ordering::Acquire) {
                sync_shared_nav_snapshot(shared, webview);
            }
        }
    }
}

extern "C" fn nav_did_commit(
    this: &Object,
    _cmd: Sel,
    webview: *mut Object,
    _navigation: *mut Object,
) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.alive.load(Ordering::Acquire) {
                return;
            }
            *shared.last_nav_error.write() = None;
            sync_shared_nav_snapshot(shared, webview);
        }
    }
}

extern "C" fn nav_did_finish(
    this: &Object,
    _cmd: Sel,
    webview: *mut Object,
    _navigation: *mut Object,
) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.alive.load(Ordering::Acquire) {
                return;
            }
            if !shared.allow_first_responder.load(Ordering::Acquire) {
                restore_superview_first_responder_for_webview(webview);
            }
            *shared.last_nav_error.write() = None;
            shared.is_loading.store(false, Ordering::Release);
            sync_shared_nav_snapshot(shared, webview);
            shared.nav_seq.fetch_add(1, Ordering::AcqRel);
            shared.nav_notify.notify_waiters();
            shared.push_event(WkWebViewEvent::LoadingChanged { is_loading: false });
        }
    }
}

extern "C" fn nav_did_fail(
    this: &Object,
    _cmd: Sel,
    webview: *mut Object,
    _navigation: *mut Object,
    error: *mut Object,
) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.alive.load(Ordering::Acquire) {
                return;
            }
            let err_code: isize = if error.is_null() {
                0
            } else {
                msg_send![error, code]
            };
            // NSURLErrorCancelled (-999) happens on normal fast redirects/back clicks.
            if err_code == -999 {
                sync_shared_nav_snapshot(shared, webview);
                return;
            }
            let err_msg = nserror_to_string(error);
            *shared.last_nav_error.write() = Some(err_msg.clone());
            shared.is_loading.store(false, Ordering::Release);
            sync_shared_nav_snapshot(shared, webview);
            let url = shared.current_url.read().clone();
            shared.nav_seq.fetch_add(1, Ordering::AcqRel);
            shared.nav_notify.notify_waiters();
            shared.push_event(WkWebViewEvent::NavigationFailed {
                url,
                error: err_msg,
            });
            shared.push_event(WkWebViewEvent::LoadingChanged { is_loading: false });
        }
    }
}

extern "C" fn nav_process_terminated(this: &Object, _cmd: Sel, webview: *mut Object) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if !shared.alive.load(Ordering::Acquire) {
                return;
            }
            let err_msg = "WKWebView WebContent process terminated".to_string();
            *shared.last_nav_error.write() = Some(err_msg.clone());
            shared.is_loading.store(false, Ordering::Release);
            sync_shared_nav_snapshot(shared, webview);
            let url = shared.current_url.read().clone();
            shared.nav_seq.fetch_add(1, Ordering::AcqRel);
            shared.nav_notify.notify_waiters();
            shared.push_event(WkWebViewEvent::NavigationFailed {
                url,
                error: err_msg,
            });
        }
    }
}

fn get_or_register_navigation_delegate_class() -> Option<&'static Class> {
    static REGISTER_ONCE: Once = Once::new();
    REGISTER_ONCE.call_once(|| {
        if Class::get("GPUINavigationDelegate").is_some() {
            return;
        }
        let Some(superclass) = Class::get("NSObject") else {
            return;
        };
        let Some(mut decl) = ClassDecl::new("GPUINavigationDelegate", superclass) else {
            return;
        };
        decl.add_ivar::<*mut c_void>(SHARED_IVAR_NAME);
        if let Some(proto) = Protocol::get("WKNavigationDelegate") {
            decl.add_protocol(proto);
        }
        unsafe {
            decl.add_method(
                sel!(webView:decidePolicyForNavigationAction:decisionHandler:),
                nav_decide_policy
                    as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut c_void),
            );
            decl.add_method(
                sel!(webView:didStartProvisionalNavigation:),
                nav_did_start as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(webView:didReceiveServerRedirectForProvisionalNavigation:),
                nav_did_redirect as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(webView:didCommitNavigation:),
                nav_did_commit as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(webView:didFinishNavigation:),
                nav_did_finish as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(webView:didFailNavigation:withError:),
                nav_did_fail as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(webView:didFailProvisionalNavigation:withError:),
                nav_did_fail as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(webViewWebContentProcessDidTerminate:),
                nav_process_terminated as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(dealloc),
                shared_ivar_dealloc as extern "C" fn(&mut Object, Sel),
            );
        }
        decl.register();
    });
    Class::get("GPUINavigationDelegate")
}

extern "C" fn ui_create_webview_for_popup(
    this: &Object,
    _cmd: Sel,
    webview: *mut Object,
    _configuration: *mut Object,
    navigation_action: *mut Object,
    _window_features: *mut Object,
) -> *mut Object {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if shared.alive.load(Ordering::Acquire) && !navigation_action.is_null() {
                let req: *mut Object = msg_send![navigation_action, request];
                if !req.is_null() {
                    let ns_url: *mut Object = msg_send![req, URL];
                    let url_str = nsurl_to_rust(ns_url);
                    if !url_str.trim().is_empty() {
                        let allow_file = shared.allow_file_urls.load(Ordering::Acquire);
                        if validate_wk_navigation_url(&url_str, allow_file).is_ok() {
                            shared.push_event(WkWebViewEvent::PopupRequested {
                                url: url_str,
                            });
                            if shared.load_popups_in_same_tab.load(Ordering::Acquire)
                                && !webview.is_null()
                            {
                                let _: *mut Object = msg_send![webview, loadRequest: req];
                            }
                        }
                    }
                }
            }
        }
    }
    ptr::null_mut()
}

extern "C" fn ui_run_js_alert(
    _this: &Object,
    _cmd: Sel,
    _webview: *mut Object,
    _message: *mut Object,
    _frame: *mut Object,
    completion_handler: *mut c_void,
) {
    if !completion_handler.is_null() {
        unsafe {
            let blk = completion_handler as *mut ObjcBlockLayout;
            let f: unsafe extern "C" fn(*mut ObjcBlockLayout) =
                std::mem::transmute((*blk).invoke);
            f(blk);
        }
    }
}

extern "C" fn ui_run_js_confirm(
    _this: &Object,
    _cmd: Sel,
    _webview: *mut Object,
    _message: *mut Object,
    _frame: *mut Object,
    completion_handler: *mut c_void,
) {
    if !completion_handler.is_null() {
        unsafe {
            let blk = completion_handler as *mut ObjcBlockLayout;
            let f: unsafe extern "C" fn(*mut ObjcBlockLayout, BOOL) =
                std::mem::transmute((*blk).invoke);
            f(blk, NO);
        }
    }
}

extern "C" fn ui_run_js_prompt(
    _this: &Object,
    _cmd: Sel,
    _webview: *mut Object,
    _prompt: *mut Object,
    _default_text: *mut Object,
    _frame: *mut Object,
    completion_handler: *mut c_void,
) {
    if !completion_handler.is_null() {
        unsafe {
            let blk = completion_handler as *mut ObjcBlockLayout;
            let f: unsafe extern "C" fn(*mut ObjcBlockLayout, *mut Object) =
                std::mem::transmute((*blk).invoke);
            f(blk, ptr::null_mut());
        }
    }
}

fn get_or_register_ui_delegate_class() -> Option<&'static Class> {
    static REGISTER_ONCE: Once = Once::new();
    REGISTER_ONCE.call_once(|| {
        if Class::get("GPUIUIDelegate").is_some() {
            return;
        }
        let Some(superclass) = Class::get("NSObject") else {
            return;
        };
        let Some(mut decl) = ClassDecl::new("GPUIUIDelegate", superclass) else {
            return;
        };
        decl.add_ivar::<*mut c_void>(SHARED_IVAR_NAME);
        if let Some(proto) = Protocol::get("WKUIDelegate") {
            decl.add_protocol(proto);
        }
        unsafe {
            decl.add_method(
                sel!(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:),
                ui_create_webview_for_popup
                    as extern "C" fn(
                        &Object,
                        Sel,
                        *mut Object,
                        *mut Object,
                        *mut Object,
                        *mut Object,
                    ) -> *mut Object,
            );
            decl.add_method(
                sel!(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:),
                ui_run_js_alert
                    as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object, *mut c_void),
            );
            decl.add_method(
                sel!(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:),
                ui_run_js_confirm
                    as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object, *mut c_void),
            );
            decl.add_method(
                sel!(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:),
                ui_run_js_prompt
                    as extern "C" fn(
                        &Object,
                        Sel,
                        *mut Object,
                        *mut Object,
                        *mut Object,
                        *mut Object,
                        *mut c_void,
                    ),
            );
            decl.add_method(
                sel!(dealloc),
                shared_ivar_dealloc as extern "C" fn(&mut Object, Sel),
            );
        }
        decl.register();
    });
    Class::get("GPUIUIDelegate")
}

extern "C" fn kvo_observe_value(
    this: &Object,
    _cmd: Sel,
    _key_path: *mut Object,
    object: *mut Object,
    _change: *mut Object,
    _context: *mut c_void,
) {
    unsafe {
        if let Some(shared) = get_shared_from_ivar(this) {
            if shared.alive.load(Ordering::Acquire) && !object.is_null() {
                sync_shared_nav_snapshot(shared, object);
            }
        }
    }
}

fn get_or_register_kvo_observer_class() -> Option<&'static Class> {
    static REGISTER_ONCE: Once = Once::new();
    REGISTER_ONCE.call_once(|| {
        if Class::get("GPUIWebViewKvoObserver").is_some() {
            return;
        }
        let Some(superclass) = Class::get("NSObject") else {
            return;
        };
        let Some(mut decl) = ClassDecl::new("GPUIWebViewKvoObserver", superclass) else {
            return;
        };
        decl.add_ivar::<*mut c_void>(SHARED_IVAR_NAME);
        unsafe {
            decl.add_method(
                sel!(observeValueForKeyPath:ofObject:change:context:),
                kvo_observe_value
                    as extern "C" fn(
                        &Object,
                        Sel,
                        *mut Object,
                        *mut Object,
                        *mut Object,
                        *mut c_void,
                    ),
            );
            decl.add_method(
                sel!(dealloc),
                shared_ivar_dealloc as extern "C" fn(&mut Object, Sel),
            );
        }
        decl.register();
    });
    Class::get("GPUIWebViewKvoObserver")
}

const KVO_KEYS: &[&str] = &["URL", "title", "loading", "canGoBack", "canGoForward"];

unsafe extern "C" fn eval_js_completion_invoke(
    blk_ptr: *mut c_void,
    result: *mut Object,
    error: *mut Object,
) {
    let blk = unsafe { &*(blk_ptr as *const RustCompletionBlock<OneshotBlockState<Result<Value, BotBrowserError>>>) };
    if blk.state.is_null() {
        return;
    }
    let state = unsafe { &*blk.state };
    if !error.is_null() {
        let msg = unsafe { nserror_to_string(error) };
        state.send(Err(BotBrowserError::Cdp(format!(
            "WKWebView evaluateJavaScript failed: {msg}"
        ))));
    } else {
        let val = unsafe { objc_value_to_json(result) };
        state.send(Ok(val));
    }
}

unsafe extern "C" fn snapshot_completion_invoke(
    blk_ptr: *mut c_void,
    ns_image: *mut Object,
    error: *mut Object,
) {
    let blk = unsafe { &*(blk_ptr as *const RustCompletionBlock<OneshotBlockState<Result<Vec<u8>, BotBrowserError>>>) };
    if blk.state.is_null() {
        return;
    }
    let state = unsafe { &*blk.state };
    if !error.is_null() {
        let msg = unsafe { nserror_to_string(error) };
        state.send(Err(BotBrowserError::Cdp(format!(
            "WKWebView snapshot failed: {msg}"
        ))));
        return;
    }
    if ns_image.is_null() {
        state.send(Err(BotBrowserError::Cdp(
            "WKWebView snapshot returned nil NSImage".to_string(),
        )));
        return;
    }
    unsafe {
        let cg_img: *mut c_void = msg_send![
            ns_image,
            CGImageForProposedRect: ptr::null_mut::<CgRect>()
            context: ptr::null_mut::<Object>()
            hints: ptr::null_mut::<Object>()
        ];
        if cg_img.is_null() {
            state.send(Err(BotBrowserError::Cdp(
                "WKWebView snapshot failed converting NSImage to CGImage".to_string(),
            )));
            return;
        }
        let rep_alloc: *mut Object = msg_send![class!(NSBitmapImageRep), alloc];
        let rep: *mut Object = msg_send![rep_alloc, initWithCGImage: cg_img];
        if rep.is_null() {
            state.send(Err(BotBrowserError::Cdp(
                "WKWebView snapshot failed allocating NSBitmapImageRep".to_string(),
            )));
            return;
        }
        let props: *mut Object = msg_send![class!(NSDictionary), dictionary];
        // NSBitmapImageFileTypePNG = 4
        let png_data: *mut Object = msg_send![
            rep,
            representationUsingType: 4usize
            properties: props
        ];
        let bytes = nsdata_to_bytes(png_data);
        let _: () = msg_send![rep, release];
        if bytes.is_empty() {
            state.send(Err(BotBrowserError::Cdp(
                "WKWebView snapshot produced empty PNG bytes".to_string(),
            )));
        } else {
            state.send(Ok(bytes));
        }
    }
}

unsafe extern "C" fn get_cookies_completion_invoke(
    blk_ptr: *mut c_void,
    ns_cookies: *mut Object,
) {
    let blk = unsafe {
        &*(blk_ptr as *const RustCompletionBlock<OneshotBlockState<Vec<BrowserCookie>>>)
    };
    if blk.state.is_null() {
        return;
    }
    let state = unsafe { &*blk.state };
    let mut out = Vec::new();
    if !ns_cookies.is_null() {
        unsafe {
            let count: usize = msg_send![ns_cookies, count];
            out.reserve(count);
            for idx in 0..count {
                let c: *mut Object = msg_send![ns_cookies, objectAtIndex: idx];
                if c.is_null() {
                    continue;
                }
                let name = nsstring_to_rust(msg_send![c, name]);
                let value = nsstring_to_rust(msg_send![c, value]);
                let domain = nsstring_to_rust(msg_send![c, domain]);
                let path = nsstring_to_rust(msg_send![c, path]);
                let is_secure: BOOL = msg_send![c, isSecure];
                let is_http_only: BOOL = msg_send![c, isHTTPOnly];
                let is_session: BOOL = msg_send![c, isSessionOnly];
                let exp_date: *mut Object = msg_send![c, expiresDate];
                let expires = if is_session == YES || exp_date.is_null() {
                    None
                } else {
                    let ts: f64 = msg_send![exp_date, timeIntervalSince1970];
                    if ts.is_finite() && ts > 0.0 {
                        Some(ts.round() as i64)
                    } else {
                        None
                    }
                };
                let ss_obj: *mut Object = msg_send![c, sameSitePolicy];
                let same_site = if ss_obj.is_null() {
                    None
                } else {
                    match nsstring_to_rust(ss_obj).trim().to_ascii_lowercase().as_str() {
                        "strict" => Some(SameSitePolicy::Strict),
                        "lax" => Some(SameSitePolicy::Lax),
                        "none" => Some(SameSitePolicy::None),
                        _ => None,
                    }
                };
                let host_only = !domain.starts_with('.');
                out.push(BrowserCookie {
                    name,
                    value,
                    domain,
                    path: if path.is_empty() {
                        "/".to_string()
                    } else {
                        path
                    },
                    secure: is_secure != NO,
                    http_only: is_http_only != NO,
                    same_site,
                    expires,
                    host_only,
                    session: is_session != NO || expires.is_none(),
                });
            }
        }
    }
    state.send(out);
}

unsafe extern "C" fn set_cookie_completion_invoke(blk_ptr: *mut c_void) {
    let blk = unsafe { &*(blk_ptr as *const RustCompletionBlock<OneshotBlockState<()>>) };
    if blk.state.is_null() {
        return;
    }
    let state = unsafe { &*blk.state };
    state.send(());
}

async fn eval_js_with_shared(
    shared: &Arc<SharedWebViewState>,
    script: &str,
) -> Result<Value, BotBrowserError> {
    if !shared.alive.load(Ordering::Acquire) {
        return Err(BotBrowserError::Cdp("WKWebView is closed".to_string()));
    }
    let id = shared.id;
    let shared_clone = Arc::clone(shared);
    let script_owned = script.to_string();
    let (tx, rx) = oneshot::channel::<Result<Value, BotBrowserError>>();

    run_on_main_thread(move || {
        if !shared_clone.alive.load(Ordering::Acquire) {
            let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
            return;
        }
        WEBVIEW_REGISTRY.with(|reg| {
            let map = reg.borrow();
            let Some(handles) = map.get(&id) else {
                let _ = tx.send(Err(BotBrowserError::Cdp(
                    "WKWebView handle not registered on main thread".to_string(),
                )));
                return;
            };
            if handles.webview.is_null() {
                let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is null".to_string())));
                return;
            }
            unsafe {
                let ns_script = alloc_nsstring(&script_owned);
                with_heap_completion_block(
                    OneshotBlockState::new(tx),
                    eval_js_completion_invoke as *const c_void,
                    |blk| {
                        let _: () = msg_send![
                            handles.webview,
                            evaluateJavaScript: ns_script
                            completionHandler: blk
                        ];
                    },
                );
                let _: () = msg_send![ns_script, release];
            }
        });
    });

    match tokio::time::timeout(DEFAULT_OP_TIMEOUT, rx).await {
        Ok(Ok(res)) => res,
        Ok(Err(_)) => Err(BotBrowserError::Cdp(
            "WKWebView evaluateJavaScript channel closed".to_string(),
        )),
        Err(_) => Err(BotBrowserError::Cdp(
            "WKWebView evaluateJavaScript timed out".to_string(),
        )),
    }
}

async fn capture_snapshot_with_shared(
    shared: &Arc<SharedWebViewState>,
) -> Result<Vec<u8>, BotBrowserError> {
    if !shared.alive.load(Ordering::Acquire) {
        return Err(BotBrowserError::Cdp("WKWebView is closed".to_string()));
    }
    let id = shared.id;
    let shared_clone = Arc::clone(shared);
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, BotBrowserError>>();

    run_on_main_thread(move || {
        if !shared_clone.alive.load(Ordering::Acquire) {
            let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
            return;
        }
        WEBVIEW_REGISTRY.with(|reg| {
            let map = reg.borrow();
            let Some(handles) = map.get(&id) else {
                let _ = tx.send(Err(BotBrowserError::Cdp(
                    "WKWebView handle not registered on main thread".to_string(),
                )));
                return;
            };
            if handles.webview.is_null() {
                let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is null".to_string())));
                return;
            }
            unsafe {
                let cfg_cls = class!(WKSnapshotConfiguration);
                let snap_cfg: *mut Object = msg_send![cfg_cls, new];
                if !snap_cfg.is_null() {
                    let _: () = msg_send![snap_cfg, setAfterScreenUpdates: YES];
                }
                with_heap_completion_block(
                    OneshotBlockState::new(tx),
                    snapshot_completion_invoke as *const c_void,
                    |blk| {
                        let _: () = msg_send![
                            handles.webview,
                            takeSnapshotWithConfiguration: snap_cfg
                            completionHandler: blk
                        ];
                    },
                );
                if !snap_cfg.is_null() {
                    let _: () = msg_send![snap_cfg, release];
                }
            }
        });
    });

    match tokio::time::timeout(DEFAULT_OP_TIMEOUT, rx).await {
        Ok(Ok(res)) => res,
        Ok(Err(_)) => Err(BotBrowserError::Cdp(
            "WKWebView snapshot channel closed".to_string(),
        )),
        Err(_) => Err(BotBrowserError::Cdp(
            "WKWebView snapshot timed out".to_string(),
        )),
    }
}

async fn export_cookies_with_shared(
    shared: &Arc<SharedWebViewState>,
    domain_filter: Option<&str>,
) -> Result<Vec<BrowserCookie>, BotBrowserError> {
    if !shared.alive.load(Ordering::Acquire) {
        return Err(BotBrowserError::Cdp("WKWebView is closed".to_string()));
    }
    let id = shared.id;
    let shared_clone = Arc::clone(shared);
    let (tx, rx) = oneshot::channel::<Vec<BrowserCookie>>();

    run_on_main_thread(move || {
        if !shared_clone.alive.load(Ordering::Acquire) {
            let _ = tx.send(Vec::new());
            return;
        }
        WEBVIEW_REGISTRY.with(|reg| {
            let map = reg.borrow();
            let Some(handles) = map.get(&id) else {
                let _ = tx.send(Vec::new());
                return;
            };
            if handles.cookie_store.is_null() {
                let _ = tx.send(Vec::new());
                return;
            }
            unsafe {
                with_heap_completion_block(
                    OneshotBlockState::new(tx),
                    get_cookies_completion_invoke as *const c_void,
                    |blk| {
                        let _: () = msg_send![handles.cookie_store, getAllCookies: blk];
                    },
                );
            }
        });
    });

    let all = match tokio::time::timeout(DEFAULT_OP_TIMEOUT, rx).await {
        Ok(Ok(list)) => list,
        Ok(Err(_)) => {
            return Err(BotBrowserError::Cdp(
                "WKHTTPCookieStore export channel closed".to_string(),
            ))
        }
        Err(_) => {
            return Err(BotBrowserError::Cdp(
                "WKHTTPCookieStore export timed out".to_string(),
            ))
        }
    };

    if let Some(filter) = domain_filter.map(str::trim).filter(|s| !s.is_empty()) {
        Ok(all
            .into_iter()
            .filter(|c| c.matches_host(filter))
            .collect())
    } else {
        Ok(all)
    }
}

async fn import_cookies_with_shared(
    shared: &Arc<SharedWebViewState>,
    cookies: &[BrowserCookie],
) -> Result<usize, BotBrowserError> {
    if !shared.alive.load(Ordering::Acquire) {
        return Err(BotBrowserError::Cdp("WKWebView is closed".to_string()));
    }
    let mut normalized_list = Vec::with_capacity(cookies.len());
    for cookie in cookies {
        if cookie.name.trim().is_empty() {
            continue;
        }
        let mut copy = cookie.clone();
        validate_and_normalize_wk_cookie(&mut copy)?;
        normalized_list.push(copy);
    }
    if normalized_list.is_empty() {
        return Ok(0);
    }

    let id = shared.id;
    let shared_clone = Arc::clone(shared);
    let (done_tx, done_rx) = oneshot::channel::<Result<Vec<oneshot::Receiver<()>>, BotBrowserError>>();

    run_on_main_thread(move || {
        if !shared_clone.alive.load(Ordering::Acquire) {
            let _ = done_tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
            return;
        }
        WEBVIEW_REGISTRY.with(|reg| {
            let map = reg.borrow();
            let Some(handles) = map.get(&id) else {
                let _ = done_tx.send(Err(BotBrowserError::Cdp(
                    "WKWebView handle not registered".to_string(),
                )));
                return;
            };
            if handles.cookie_store.is_null() {
                let _ = done_tx.send(Err(BotBrowserError::Cdp(
                    "WKHTTPCookieStore unavailable".to_string(),
                )));
                return;
            }
            let mut receivers = Vec::with_capacity(normalized_list.len());
            unsafe {
                let k_name = alloc_nsstring("Name");
                let k_value = alloc_nsstring("Value");
                let k_domain = alloc_nsstring("Domain");
                let k_path = alloc_nsstring("Path");
                let k_secure = alloc_nsstring("Secure");
                let k_http_only = alloc_nsstring("HttpOnly");
                let k_expires = alloc_nsstring("Expires");
                let k_same_site = alloc_nsstring("SameSite");

                for c in &normalized_list {
                    let dict: *mut Object = msg_send![class!(NSMutableDictionary), new];
                    if dict.is_null() {
                        continue;
                    }
                    let v_name = alloc_nsstring(&c.name);
                    let v_val = alloc_nsstring(&c.value);
                    let domain_val = wk_ns_cookie_domain(c);
                    let v_dom = alloc_nsstring(&domain_val);
                    let v_path = alloc_nsstring(&c.path);
                    let _: () = msg_send![dict, setObject: v_name forKey: k_name];
                    let _: () = msg_send![dict, setObject: v_val forKey: k_value];
                    let _: () = msg_send![dict, setObject: v_dom forKey: k_domain];
                    let _: () = msg_send![dict, setObject: v_path forKey: k_path];
                    let _: () = msg_send![v_name, release];
                    let _: () = msg_send![v_val, release];
                    let _: () = msg_send![v_dom, release];
                    let _: () = msg_send![v_path, release];

                    if c.secure {
                        let v_true = alloc_nsstring("TRUE");
                        let _: () = msg_send![dict, setObject: v_true forKey: k_secure];
                        let _: () = msg_send![v_true, release];
                    }
                    if c.http_only {
                        let v_true = alloc_nsstring("TRUE");
                        let _: () = msg_send![dict, setObject: v_true forKey: k_http_only];
                        let _: () = msg_send![v_true, release];
                    }
                    if !c.session {
                        if let Some(exp) = c.expires.filter(|&e| e > 0) {
                            let ns_date: *mut Object =
                                msg_send![class!(NSDate), dateWithTimeIntervalSince1970: exp as f64];
                            if !ns_date.is_null() {
                                let _: () = msg_send![dict, setObject: ns_date forKey: k_expires];
                            }
                        }
                    }
                    if let Some(ss) = c.same_site {
                        let ss_str = match ss {
                            SameSitePolicy::Strict => "strict",
                            SameSitePolicy::Lax => "lax",
                            SameSitePolicy::None => "none",
                        };
                        let v_ss = alloc_nsstring(ss_str);
                        let _: () = msg_send![dict, setObject: v_ss forKey: k_same_site];
                        let _: () = msg_send![v_ss, release];
                    }

                    let ns_cookie: *mut Object =
                        msg_send![class!(NSHTTPCookie), cookieWithProperties: dict];
                    let _: () = msg_send![dict, release];
                    if !ns_cookie.is_null() {
                        let (tx, rx) = oneshot::channel::<()>();
                        with_heap_completion_block(
                            OneshotBlockState::new(tx),
                            set_cookie_completion_invoke as *const c_void,
                            |blk| {
                                let _: () = msg_send![
                                    handles.cookie_store,
                                    setCookie: ns_cookie
                                    completionHandler: blk
                                ];
                            },
                        );
                        receivers.push(rx);
                    }
                }

                let _: () = msg_send![k_name, release];
                let _: () = msg_send![k_value, release];
                let _: () = msg_send![k_domain, release];
                let _: () = msg_send![k_path, release];
                let _: () = msg_send![k_secure, release];
                let _: () = msg_send![k_http_only, release];
                let _: () = msg_send![k_expires, release];
                let _: () = msg_send![k_same_site, release];
            }
            let _ = done_tx.send(Ok(receivers));
        });
    });

    let receivers = match tokio::time::timeout(DEFAULT_OP_TIMEOUT, done_rx).await {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(e))) => return Err(e),
        Ok(Err(_)) => {
            return Err(BotBrowserError::Cdp(
                "WKHTTPCookieStore import channel closed".to_string(),
            ))
        }
        Err(_) => {
            return Err(BotBrowserError::Cdp(
                "WKHTTPCookieStore import timed out".to_string(),
            ))
        }
    };

    let count = receivers.len();
    let _ = tokio::time::timeout(DEFAULT_OP_TIMEOUT, futures::future::join_all(receivers)).await;
    Ok(count)
}

/// Generalized native macOS `WKWebView` child view host supporting both Mini Apps and full browser workspaces.
pub struct EmbeddedWkWebView {
    shared: Arc<SharedWebViewState>,
    backend: Arc<WkWebViewBackend>,
    parent_ns_view: *mut Object,
    webview: *mut Object,
    loaded_url: String,
    is_hidden: bool,
    last_frame: Option<(f64, f64, f64, f64)>,
    last_zoom: Option<f64>,
}

impl EmbeddedWkWebView {
    pub fn is_headless_or_offline_test_env() -> bool {
        is_headless_or_offline_test_env()
    }

    pub fn attach(window: &Window) -> Option<(Self, UnboundedReceiver<()>)> {
        Self::attach_with_config(window, WkWebViewConfig::miniapp_default())
    }

    pub fn attach_with_config(
        window: &Window,
        config: WkWebViewConfig,
    ) -> Option<(Self, UnboundedReceiver<()>)> {
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
        let wk_cls = get_or_register_wkwebview_class().or_else(|| Class::get("WKWebView"))?;
        let cfg_cls = Class::get("WKWebViewConfiguration")?;
        let script_cls = Class::get("WKUserScript")?;
        let handler_cls = get_or_register_script_handler_class()?;
        let nav_cls = get_or_register_navigation_delegate_class()?;
        let ui_cls = get_or_register_ui_delegate_class()?;
        let kvo_cls = get_or_register_kvo_observer_class()?;

        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let id = NEXT_WEBVIEW_ID.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(SharedWebViewState::new(id, &config, notify_tx));

        unsafe {
            let wk_config: *mut Object = msg_send![cfg_cls, new];
            if wk_config.is_null() {
                return None;
            }

            // Configure profile-scoped or ephemeral WKWebsiteDataStore.
            if let Some(ds_cls) = Class::get("WKWebsiteDataStore") {
                let data_store: *mut Object = if config.ephemeral {
                    msg_send![ds_cls, nonPersistentDataStore]
                } else if let Some(ref pid) = config.profile_id {
                    let responds: BOOL =
                        msg_send![ds_cls, respondsToSelector: sel!(dataStoreForIdentifier:)];
                    if responds != NO {
                        let uuid_str = deterministic_profile_uuid_string(pid);
                        let ns_uuid_str = alloc_nsstring(&uuid_str);
                        let ns_uuid_alloc: *mut Object = msg_send![class!(NSUUID), alloc];
                        let ns_uuid: *mut Object =
                            msg_send![ns_uuid_alloc, initWithUUIDString: ns_uuid_str];
                        let _: () = msg_send![ns_uuid_str, release];
                        let store: *mut Object = if !ns_uuid.is_null() {
                            let s: *mut Object =
                                msg_send![ds_cls, dataStoreForIdentifier: ns_uuid];
                            let _: () = msg_send![ns_uuid, release];
                            s
                        } else {
                            ptr::null_mut()
                        };
                        if store.is_null() {
                            msg_send![ds_cls, defaultDataStore]
                        } else {
                            store
                        }
                    } else {
                        msg_send![ds_cls, defaultDataStore]
                    }
                } else {
                    msg_send![ds_cls, defaultDataStore]
                };
                if !data_store.is_null() {
                    let _: () = msg_send![wk_config, setWebsiteDataStore: data_store];
                }
            }

            let ucc: *mut Object = msg_send![wk_config, userContentController];
            if ucc.is_null() {
                let _: () = msg_send![wk_config, release];
                return None;
            }
            let _: *mut Object = msg_send![ucc, retain];

            if config.inject_telegram_proxy {
                let boot_js = alloc_nsstring(TELEGRAM_WEBVIEW_PROXY_BOOT_JS);
                let user_script: *mut Object = msg_send![script_cls, alloc];
                let user_script: *mut Object = msg_send![
                    user_script,
                    initWithSource: boot_js
                    injectionTime: 0isize
                    forMainFrameOnly: NO
                ];
                let _: () = msg_send![boot_js, release];
                if !user_script.is_null() {
                    let _: () = msg_send![ucc, addUserScript: user_script];
                    let _: () = msg_send![user_script, release];
                }
            }

            let recorder_boot_source = if config.enable_dom_recorder {
                format!("window.__gpuiDomRecorderEnabled = true;\n{WK_DOM_RECORDER_JS}")
            } else {
                format!("window.__gpuiDomRecorderEnabled = false;\n{WK_DOM_RECORDER_JS}")
            };
            let rec_js = alloc_nsstring(&recorder_boot_source);
            let rec_script: *mut Object = msg_send![script_cls, alloc];
            let rec_script: *mut Object = msg_send![
                rec_script,
                initWithSource: rec_js
                injectionTime: 0isize
                forMainFrameOnly: YES
            ];
            let _: () = msg_send![rec_js, release];
            if !rec_script.is_null() {
                let _: () = msg_send![ucc, addUserScript: rec_script];
                let _: () = msg_send![rec_script, release];
            }

            let script_handler: *mut Object = msg_send![handler_cls, new];
            let recorder_handler: *mut Object = msg_send![handler_cls, new];
            if script_handler.is_null() || recorder_handler.is_null() {
                if !script_handler.is_null() {
                    let _: () = msg_send![script_handler, release];
                }
                if !recorder_handler.is_null() {
                    let _: () = msg_send![recorder_handler, release];
                }
                let _: () = msg_send![ucc, release];
                let _: () = msg_send![wk_config, release];
                return None;
            }

            let bridge_box = Box::into_raw(Box::new(BridgeState {
                shared: Arc::clone(&shared),
                mode: ScriptHandlerMode::TelegramBridge,
            }));
            (*script_handler).set_ivar(BRIDGE_IVAR_NAME, bridge_box as *mut c_void);

            let recorder_box = Box::into_raw(Box::new(BridgeState {
                shared: Arc::clone(&shared),
                mode: ScriptHandlerMode::DomRecorder,
            }));
            (*recorder_handler).set_ivar(BRIDGE_IVAR_NAME, recorder_box as *mut c_void);

            let handler_name = alloc_nsstring(SCRIPT_HANDLER_NAME);
            let _: () = msg_send![ucc, addScriptMessageHandler: script_handler name: handler_name];
            let _: () = msg_send![handler_name, release];

            let rec_name = alloc_nsstring(RECORDER_HANDLER_NAME);
            let _: () = msg_send![ucc, addScriptMessageHandler: recorder_handler name: rec_name];
            let _: () = msg_send![rec_name, release];

            let init_rect = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize {
                    width: 480.0,
                    height: 540.0,
                },
            };
            let webview: *mut Object = msg_send![wk_cls, alloc];
            let webview: *mut Object =
                msg_send![webview, initWithFrame: init_rect configuration: wk_config];
            let _: () = msg_send![wk_config, release];
            if webview.is_null() {
                let n1 = alloc_nsstring(SCRIPT_HANDLER_NAME);
                let _: () = msg_send![ucc, removeScriptMessageHandlerForName: n1];
                let _: () = msg_send![n1, release];
                let n2 = alloc_nsstring(RECORDER_HANDLER_NAME);
                let _: () = msg_send![ucc, removeScriptMessageHandlerForName: n2];
                let _: () = msg_send![n2, release];
                let _: () = msg_send![script_handler, release];
                let _: () = msg_send![recorder_handler, release];
                let _: () = msg_send![ucc, release];
                return None;
            }
            if let Some(sub_cls) = Class::get("GPUIWKWebView") {
                let is_sub: BOOL = msg_send![webview, isKindOfClass: sub_cls];
                if is_sub != NO {
                    (*webview).set_ivar(
                        SHARED_IVAR_NAME,
                        Arc::into_raw(Arc::clone(&shared)) as *mut c_void,
                    );
                }
            }

            let nav_delegate: *mut Object = msg_send![nav_cls, new];
            if !nav_delegate.is_null() {
                (*nav_delegate).set_ivar(
                    SHARED_IVAR_NAME,
                    Arc::into_raw(Arc::clone(&shared)) as *mut c_void,
                );
                let _: () = msg_send![webview, setNavigationDelegate: nav_delegate];
            }
            let ui_delegate: *mut Object = msg_send![ui_cls, new];
            if !ui_delegate.is_null() {
                (*ui_delegate).set_ivar(
                    SHARED_IVAR_NAME,
                    Arc::into_raw(Arc::clone(&shared)) as *mut c_void,
                );
                let _: () = msg_send![webview, setUIDelegate: ui_delegate];
            }
            let kvo_observer: *mut Object = msg_send![kvo_cls, new];
            if !kvo_observer.is_null() {
                (*kvo_observer).set_ivar(
                    SHARED_IVAR_NAME,
                    Arc::into_raw(Arc::clone(&shared)) as *mut c_void,
                );
                for key in KVO_KEYS {
                    let ns_key = alloc_nsstring(key);
                    let _: () = msg_send![
                        webview,
                        addObserver: kvo_observer
                        forKeyPath: ns_key
                        options: 0x01usize // NSKeyValueObservingOptionNew
                        context: (&raw const KVO_CONTEXT_TAG) as *mut c_void
                    ];
                    let _: () = msg_send![ns_key, release];
                }
            }

            let cfg_out: *mut Object = msg_send![webview, configuration];
            let ds_out: *mut Object = if !cfg_out.is_null() {
                msg_send![cfg_out, websiteDataStore]
            } else {
                ptr::null_mut()
            };
            let cookie_store: *mut Object = if !ds_out.is_null() {
                let cs: *mut Object = msg_send![ds_out, httpCookieStore];
                if !cs.is_null() {
                    let _: *mut Object = msg_send![cs, retain];
                }
                cs
            } else {
                ptr::null_mut()
            };

            let draws_bg_key = alloc_nsstring("drawsBackground");
            let no_num: *mut Object = msg_send![class!(NSNumber), numberWithBool: NO];
            let _: () = msg_send![webview, setValue: no_num forKey: draws_bg_key];
            let _: () = msg_send![draws_bg_key, release];

            let _: () = msg_send![webview, setHidden: YES];
            let _: () = msg_send![parent_ns_view, addSubview: webview];
            restore_parent_first_responder_if_owned_by_webview(parent_ns_view, webview);

            WEBVIEW_REGISTRY.with(|reg| {
                reg.borrow_mut().insert(
                    id,
                    MainThreadWebViewHandles {
                        webview,
                        cookie_store,
                        user_content_controller: ucc,
                        script_handler,
                        recorder_handler,
                        nav_delegate,
                        ui_delegate,
                        kvo_observer,
                    },
                );
            });

            let backend = Arc::new(WkWebViewBackend::from_shared(Arc::clone(&shared)));
            Some((
                Self {
                    shared,
                    backend,
                    parent_ns_view,
                    webview,
                    loaded_url: String::new(),
                    is_hidden: true,
                    last_frame: None,
                    last_zoom: None,
                },
                notify_rx,
            ))
        }
    }

    pub fn backend(&self) -> Arc<WkWebViewBackend> {
        Arc::clone(&self.backend)
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
        if trimmed.is_empty() || self.webview.is_null() {
            return;
        }
        let allow_file = self.shared.allow_file_urls.load(Ordering::Acquire);
        if validate_wk_navigation_url(trimmed, allow_file).is_err() {
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
                *self.shared.current_url.write() = trimmed.to_string();
            }
        }
    }

    pub fn load_url_if_changed(&mut self, url: &str) {
        let trimmed = url.trim();
        if trimmed.is_empty() || self.loaded_url == trimmed {
            return;
        }
        self.load_url(trimmed);
    }

    pub fn loaded_url(&self) -> &str {
        &self.loaded_url
    }

    pub fn go_back(&mut self) -> bool {
        if self.webview.is_null() || !self.can_go_back() {
            return false;
        }
        unsafe {
            let _: *mut Object = msg_send![self.webview, goBack];
        }
        true
    }

    pub fn go_forward(&mut self) -> bool {
        if self.webview.is_null() || !self.can_go_forward() {
            return false;
        }
        unsafe {
            let _: *mut Object = msg_send![self.webview, goForward];
        }
        true
    }

    pub fn reload(&mut self) {
        if self.webview.is_null() {
            return;
        }
        unsafe {
            let _: *mut Object = msg_send![self.webview, reload];
        }
    }

    pub fn can_go_back(&self) -> bool {
        if self.webview.is_null() {
            return false;
        }
        unsafe {
            let flag: BOOL = msg_send![self.webview, canGoBack];
            let b = flag != NO;
            self.shared.can_go_back.store(b, Ordering::Release);
            b
        }
    }

    pub fn can_go_forward(&self) -> bool {
        if self.webview.is_null() {
            return false;
        }
        unsafe {
            let flag: BOOL = msg_send![self.webview, canGoForward];
            let b = flag != NO;
            self.shared.can_go_forward.store(b, Ordering::Release);
            b
        }
    }

    pub fn current_url(&self) -> String {
        if !self.webview.is_null() {
            unsafe {
                let ns_url: *mut Object = msg_send![self.webview, URL];
                let s = nsurl_to_rust(ns_url);
                if !s.is_empty() {
                    *self.shared.current_url.write() = s.clone();
                    return s;
                }
            }
        }
        self.shared.current_url.read().clone()
    }

    pub fn page_title(&self) -> String {
        if !self.webview.is_null() {
            unsafe {
                let ns_title: *mut Object = msg_send![self.webview, title];
                let s = nsstring_to_rust(ns_title);
                if !s.is_empty() {
                    *self.shared.page_title.write() = s.clone();
                    return s;
                }
            }
        }
        self.shared.page_title.read().clone()
    }

    pub fn is_loading(&self) -> bool {
        if !self.webview.is_null() {
            unsafe {
                let flag: BOOL = msg_send![self.webview, isLoading];
                let b = flag != NO;
                self.shared.is_loading.store(b, Ordering::Release);
                return b;
            }
        }
        self.shared.is_loading.load(Ordering::Acquire)
    }

    pub fn set_dom_recording(&self, enabled: bool) {
        self.shared
            .dom_recording_enabled
            .store(enabled, Ordering::Release);
        let flag = if enabled { "true" } else { "false" };
        self.evaluate_js(&format!("window.__gpuiDomRecorderEnabled = {flag};"));
    }

    pub fn is_dom_recording(&self) -> bool {
        self.shared.dom_recording_enabled.load(Ordering::Acquire)
    }

    pub fn is_hidden(&self) -> bool {
        self.is_hidden
    }

    pub fn restore_parent_first_responder(&self) {
        unsafe {
            restore_parent_first_responder_if_owned_by_webview(self.parent_ns_view, self.webview);
        }
    }

    pub fn blur_active_dom_element(&self) {
        self.evaluate_js(
            "(function(){try{if(document.activeElement&&typeof document.activeElement.blur==='function'){document.activeElement.blur();}}catch(_){}})();",
        );
    }

    pub fn set_allow_first_responder(&mut self, allow: bool) {
        let prev = self
            .shared
            .allow_first_responder
            .swap(allow, Ordering::AcqRel);
        if !allow {
            self.shared
                .became_first_responder
                .store(false, Ordering::Release);
            if prev {
                self.blur_active_dom_element();
            }
            self.restore_parent_first_responder();
        }
    }

    pub fn has_pending_became_first_responder(&self) -> bool {
        self.shared.became_first_responder.load(Ordering::Acquire)
    }

    pub fn take_became_first_responder(&self) -> bool {
        self.shared
            .became_first_responder
            .swap(false, Ordering::AcqRel)
    }

    pub fn clear_became_first_responder(&self) {
        self.shared
            .became_first_responder
            .store(false, Ordering::Release);
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        if self.webview.is_null() {
            return;
        }
        if hidden {
            let was_allowed = self
                .shared
                .allow_first_responder
                .swap(false, Ordering::AcqRel);
            self.shared.ignore_hit_test.store(true, Ordering::Release);
            self.shared
                .became_first_responder
                .store(false, Ordering::Release);
            if was_allowed {
                self.blur_active_dom_element();
            }
            self.restore_parent_first_responder();
            if self.is_hidden {
                return;
            }
            self.is_hidden = true;
            unsafe {
                let flag: BOOL = YES;
                let _: () = msg_send![self.webview, setHidden: flag];
            }
            self.restore_parent_first_responder();
            return;
        }
        self.shared.ignore_hit_test.store(false, Ordering::Release);
        if self.is_hidden {
            self.is_hidden = false;
            unsafe {
                let flag: BOOL = NO;
                let _: () = msg_send![self.webview, setHidden: flag];
            }
        }
        if !self.shared.allow_first_responder.load(Ordering::Acquire) {
            self.restore_parent_first_responder();
        }
    }

    /// Parks the WebView at a 1x1 near-transparent live frame during Floating PiP mode so
    /// macOS does not throttle JS timers/WASM while hidden from the main full-screen overlay.
    pub fn park_for_pip(&mut self) {
        if self.webview.is_null() || self.parent_ns_view.is_null() {
            return;
        }
        let was_allowed = self
            .shared
            .allow_first_responder
            .swap(false, Ordering::AcqRel);
        let was_ignoring_hit = self.shared.ignore_hit_test.swap(true, Ordering::AcqRel);
        self.shared
            .became_first_responder
            .store(false, Ordering::Release);
        if !self.is_hidden
            && self.last_frame == Some((0.0, 0.0, 1.0, 1.0))
            && !was_allowed
            && was_ignoring_hit
        {
            self.restore_parent_first_responder();
            return;
        }
        self.blur_active_dom_element();
        self.restore_parent_first_responder();
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
        self.restore_parent_first_responder();
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
        self.shared.ignore_hit_test.store(false, Ordering::Release);
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

    pub async fn eval_js_async(&self, js: &str) -> Result<Value, BotBrowserError> {
        eval_js_with_shared(&self.shared, js).await
    }

    pub async fn capture_png_snapshot(&self) -> Result<Vec<u8>, BotBrowserError> {
        capture_snapshot_with_shared(&self.shared).await
    }

    pub async fn export_cookies(
        &self,
        domain_filter: Option<&str>,
    ) -> Result<Vec<BrowserCookie>, BotBrowserError> {
        export_cookies_with_shared(&self.shared, domain_filter).await
    }

    pub async fn import_cookies(
        &self,
        cookies: &[BrowserCookie],
    ) -> Result<usize, BotBrowserError> {
        import_cookies_with_shared(&self.shared, cookies).await
    }

    pub fn drain_webview_events(&self) -> Vec<WkWebViewEvent> {
        let Ok(mut q) = self.shared.event_queue.lock() else {
            return Vec::new();
        };
        q.drain(..).collect()
    }

    pub fn destroy(&mut self) {
        self.shared.alive.store(false, Ordering::Release);
        self.shared
            .allow_first_responder
            .store(false, Ordering::Release);
        self.shared.ignore_hit_test.store(true, Ordering::Release);
        self.shared
            .became_first_responder
            .store(false, Ordering::Release);
        self.shared.nav_notify.notify_waiters();
        if self.webview.is_null() {
            return;
        }
        let parent_view = self.parent_ns_view;
        let webview_ptr = self.webview;
        unsafe {
            restore_parent_first_responder_if_owned_by_webview(parent_view, webview_ptr);
        }
        let id = self.shared.id;
        let handles_opt = WEBVIEW_REGISTRY.with(|reg| reg.borrow_mut().remove(&id));
        if let Some(handles) = handles_opt {
            unsafe {
                let _: () = msg_send![handles.webview, setNavigationDelegate: ptr::null_mut::<Object>()];
                let _: () = msg_send![handles.webview, setUIDelegate: ptr::null_mut::<Object>()];
                if !handles.kvo_observer.is_null() {
                    for key in KVO_KEYS {
                        let ns_key = alloc_nsstring(key);
                        let _: () = msg_send![
                            handles.webview,
                            removeObserver: handles.kvo_observer
                            forKeyPath: ns_key
                            context: (&raw const KVO_CONTEXT_TAG) as *mut c_void
                        ];
                        let _: () = msg_send![ns_key, release];
                    }
                    let _: () = msg_send![handles.kvo_observer, release];
                }
                if !handles.user_content_controller.is_null() {
                    let n1 = alloc_nsstring(SCRIPT_HANDLER_NAME);
                    let _: () =
                        msg_send![handles.user_content_controller, removeScriptMessageHandlerForName: n1];
                    let _: () = msg_send![n1, release];
                    let n2 = alloc_nsstring(RECORDER_HANDLER_NAME);
                    let _: () =
                        msg_send![handles.user_content_controller, removeScriptMessageHandlerForName: n2];
                    let _: () = msg_send![n2, release];
                    let _: () = msg_send![handles.user_content_controller, release];
                }
                let _: () = msg_send![handles.webview, stopLoading];
                let _: () = msg_send![handles.webview, removeFromSuperview];
                let _: () = msg_send![handles.webview, release];
                if !handles.cookie_store.is_null() {
                    let _: () = msg_send![handles.cookie_store, release];
                }
                if !handles.script_handler.is_null() {
                    let _: () = msg_send![handles.script_handler, release];
                }
                if !handles.recorder_handler.is_null() {
                    let _: () = msg_send![handles.recorder_handler, release];
                }
                if !handles.nav_delegate.is_null() {
                    let _: () = msg_send![handles.nav_delegate, release];
                }
                if !handles.ui_delegate.is_null() {
                    let _: () = msg_send![handles.ui_delegate, release];
                }
            }
        } else {
            unsafe {
                let _: () = msg_send![self.webview, stopLoading];
                let _: () = msg_send![self.webview, removeFromSuperview];
                let _: () = msg_send![self.webview, release];
            }
        }
        unsafe {
            restore_parent_first_responder_if_owned_by_webview(parent_view, ptr::null_mut());
        }
        self.webview = ptr::null_mut();
        self.parent_ns_view = ptr::null_mut();
        self.loaded_url.clear();
        self.last_frame = None;
        self.last_zoom = None;
    }
}

impl Drop for EmbeddedWkWebView {
    fn drop(&mut self) {
        self.destroy();
    }
}

/// Backward-compatible Mini App wrapper preserving the exact `MacOsWkWebView` public API.
pub struct MacOsWkWebView {
    inner: EmbeddedWkWebView,
    legacy_loaded_url: String,
}

impl MacOsWkWebView {
    /// Returns `true` when running inside headless/offline tests where native AppKit child views must not be attached.
    pub fn is_headless_or_offline_test_env() -> bool {
        is_headless_or_offline_test_env()
    }

    /// Attaches a child `WKWebView` to `window`'s native `NSView` when running in a live macOS desktop window.
    /// Returns `(handle, wakeup_receiver)` or `None` under headless/offline test environments.
    pub fn attach(window: &Window) -> Option<(Self, UnboundedReceiver<()>)> {
        let (inner, rx) =
            EmbeddedWkWebView::attach_with_config(window, WkWebViewConfig::miniapp_default())?;
        Some((
            Self {
                inner,
                legacy_loaded_url: String::new(),
            },
            rx,
        ))
    }

    pub fn matches_window(&self, window: &Window) -> bool {
        self.inner.matches_window(window)
    }

    pub fn load_url(&mut self, url: &str) {
        let trimmed = url.trim();
        if trimmed.is_empty() || self.legacy_loaded_url == trimmed {
            return;
        }
        self.inner.load_url(trimmed);
        self.legacy_loaded_url = trimmed.to_string();
    }

    pub fn loaded_url(&self) -> &str {
        &self.legacy_loaded_url
    }

    pub fn reload(&mut self) {
        self.inner.reload();
    }

    pub fn is_hidden(&self) -> bool {
        self.inner.is_hidden()
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.inner.set_hidden(hidden);
    }

    pub fn set_allow_first_responder(&mut self, allow: bool) {
        self.inner.set_allow_first_responder(allow);
    }

    pub fn restore_parent_first_responder(&self) {
        self.inner.restore_parent_first_responder();
    }

    /// Parks the WebView at a 1x1 near-transparent live frame during Floating PiP mode so
    /// macOS does not throttle JS timers/WASM while hidden from the main full-screen overlay.
    pub fn park_for_pip(&mut self) {
        self.inner.park_for_pip();
    }

    /// Synchronizes child `WKWebView` frame and visibility against GPUI canvas `bounds` (top-left origin).
    pub fn sync_bounds(&mut self, bounds: Bounds<Pixels>, visible: bool) {
        self.inner.sync_bounds(bounds, visible);
    }

    pub fn set_page_zoom(&mut self, zoom: f64) {
        self.inner.set_page_zoom(zoom);
    }

    /// Dispatches a Telegram Mini App event back into the loaded `WKWebView`.
    pub fn dispatch_telegram_event(&self, event_type: &str, event_data_json: &str) {
        self.inner
            .dispatch_telegram_event(event_type, event_data_json);
    }

    pub fn evaluate_js(&self, js: &str) {
        self.inner.evaluate_js(js);
    }

    pub fn drain_events(&self) -> Vec<(String, String)> {
        self.inner
            .drain_webview_events()
            .into_iter()
            .filter_map(|ev| match ev {
                WkWebViewEvent::BridgeMessage {
                    event_type,
                    event_data,
                } => Some((event_type, event_data)),
                _ => None,
            })
            .collect()
    }

    pub fn destroy(&mut self) {
        self.inner.destroy();
        self.legacy_loaded_url.clear();
    }
}

struct TelemetryRecorder {
    backend: BrowserBackendKind,
    total_operations: u64,
    failed_operations: u64,
    total_duration_ms: u64,
    max_duration_ms: u64,
    recent_steps: VecDeque<StepTimingMetric>,
}

impl TelemetryRecorder {
    fn new(backend: BrowserBackendKind) -> Self {
        Self {
            backend,
            total_operations: 0,
            failed_operations: 0,
            total_duration_ms: 0,
            max_duration_ms: 0,
            recent_steps: VecDeque::with_capacity(MAX_RECENT_STEP_METRICS),
        }
    }

    fn record(&mut self, mut metric: StepTimingMetric) {
        if metric.backend.is_none() {
            metric.backend = Some(self.backend);
        }
        if metric.timestamp_ms == 0 {
            metric.timestamp_ms = Utc::now().timestamp_millis();
        }
        self.total_operations = self.total_operations.saturating_add(1);
        if !metric.success {
            self.failed_operations = self.failed_operations.saturating_add(1);
        }
        self.total_duration_ms = self.total_duration_ms.saturating_add(metric.duration_ms);
        self.max_duration_ms = self.max_duration_ms.max(metric.duration_ms);
        if self.recent_steps.len() >= MAX_RECENT_STEP_METRICS {
            self.recent_steps.pop_front();
        }
        self.recent_steps.push_back(metric);
    }

    fn snapshot(&self) -> BrowserPerformanceMetrics {
        let avg_duration_ms = if self.total_operations > 0 {
            self.total_duration_ms / self.total_operations
        } else {
            0
        };
        let p95_duration_ms = if self.recent_steps.is_empty() {
            0
        } else {
            let mut sorted: Vec<u64> = self.recent_steps.iter().map(|s| s.duration_ms).collect();
            sorted.sort_unstable();
            let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
            let clamped = idx.saturating_sub(1).min(sorted.len() - 1);
            sorted[clamped]
        };
        BrowserPerformanceMetrics {
            backend: Some(self.backend),
            total_operations: self.total_operations,
            failed_operations: self.failed_operations,
            total_duration_ms: self.total_duration_ms,
            avg_duration_ms,
            max_duration_ms: self.max_duration_ms,
            p95_duration_ms,
            recent_steps: self.recent_steps.iter().cloned().collect(),
        }
    }
}

/// Thread-safe `BrowserSessionBackend` bridge driving an attached [`EmbeddedWkWebView`] on macOS.
pub struct WkWebViewBackend {
    shared: Arc<SharedWebViewState>,
    telemetry: Arc<Mutex<TelemetryRecorder>>,
}

impl WkWebViewBackend {
    pub fn from_shared(shared: Arc<SharedWebViewState>) -> Self {
        Self {
            shared,
            telemetry: Arc::new(Mutex::new(TelemetryRecorder::new(
                BrowserBackendKind::WkWebView,
            ))),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::Acquire)
    }

    fn record_op(
        &self,
        operation: &str,
        label: impl Into<String>,
        started: Instant,
        success: bool,
        detail: Option<String>,
    ) -> u64 {
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Ok(mut guard) = self.telemetry.lock() {
            guard.record(StepTimingMetric {
                step_index: None,
                operation: operation.to_string(),
                label: label.into(),
                backend: Some(BrowserBackendKind::WkWebView),
                duration_ms,
                success,
                timestamp_ms: Utc::now().timestamp_millis(),
                detail,
            });
        }
        duration_ms
    }

    fn ensure_alive(&self) -> Result<(), BotBrowserError> {
        if self.shared.alive.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(BotBrowserError::Cdp("WKWebView session is closed".to_string()))
        }
    }

    async fn resolve_locator_internal(
        &self,
        locator: &SemanticLocator,
        scroll_into_view: bool,
        focus: bool,
    ) -> Result<ResolvedWkLocator, BotBrowserError> {
        if locator.is_empty() {
            return Err(BotBrowserError::LocatorNotFound(
                "empty semantic locator".to_string(),
            ));
        }
        let script = build_wk_resolve_locator_js(locator, scroll_into_view, focus)?;
        let val = self.eval_js(&script).await?;
        let resolved: ResolvedWkLocator = parse_wk_json_value(val, "resolve_locator")?;
        if !resolved.found {
            return Err(BotBrowserError::LocatorNotFound(locator.display_label()));
        }
        Ok(resolved)
    }
}

#[derive(Deserialize)]
struct ResolvedWkLocator {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    strategy: Option<String>,
    #[serde(default)]
    is_sensitive: bool,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    center_x: Option<f64>,
    #[serde(default)]
    center_y: Option<f64>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    acted: bool,
    #[serde(default)]
    refused_sensitive: bool,
}

#[derive(Deserialize)]
struct RawWkDomNode {
    ref_id: String,
    tag: String,
    #[serde(default)]
    input_type: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    aria_label: Option<String>,
    #[serde(default)]
    placeholder: Option<String>,
    #[serde(default)]
    test_id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    autocomplete: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    is_password: bool,
    selector: String,
    #[serde(default)]
    frame_origin: Option<String>,
    #[serde(default)]
    rect: Option<ElementRect>,
}

#[derive(Deserialize)]
struct RawWkSnapshot {
    #[serde(default)]
    url: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text_excerpt: String,
    #[serde(default)]
    interactive_elements: Vec<RawWkDomNode>,
    #[serde(default)]
    redacted_field_count: usize,
}

fn parse_wk_json_value<T: for<'de> Deserialize<'de>>(
    val: Value,
    op_name: &str,
) -> Result<T, BotBrowserError> {
    if let Some(s) = val.as_str() {
        serde_json::from_str::<T>(s)
            .map_err(|e| BotBrowserError::Cdp(format!("{op_name}: invalid JSON payload: {e}")))
    } else {
        serde_json::from_value::<T>(val)
            .map_err(|e| BotBrowserError::Cdp(format!("{op_name}: unexpected result shape: {e}")))
    }
}

fn is_sensitive_wk_node(node: &RawWkDomNode) -> bool {
    if node.is_password {
        return true;
    }
    if node
        .input_type
        .as_deref()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("password"))
    {
        return true;
    }
    [
        node.autocomplete.as_deref(),
        node.name.as_deref(),
        node.id.as_deref(),
        node.aria_label.as_deref(),
        node.placeholder.as_deref(),
        node.test_id.as_deref(),
        node.role.as_deref(),
        Some(node.selector.as_str()),
    ]
    .into_iter()
    .flatten()
    .any(has_sensitive_marker_text)
}

fn sanitize_wk_snapshot(raw: RawWkSnapshot) -> BrowserSnapshot {
    let mut redacted_count = 0usize;
    let mut elements = Vec::with_capacity(raw.interactive_elements.len());
    let mut text_excerpt = raw.text_excerpt;

    for node in raw.interactive_elements {
        let sensitive = is_sensitive_wk_node(&node);
        if sensitive {
            let trimmed_text = node.text.trim();
            if trimmed_text.len() >= 4 && trimmed_text != "[REDACTED]" {
                text_excerpt = text_excerpt.replace(trimmed_text, "[REDACTED]");
            }
            if let Some(raw_val) = node.value.as_deref().map(str::trim) {
                if raw_val.len() >= 4 && raw_val != "[REDACTED]" {
                    text_excerpt = text_excerpt.replace(raw_val, "[REDACTED]");
                }
            }
        }
        let value = if sensitive {
            redacted_count += 1;
            Some("[REDACTED]".to_string())
        } else {
            node.value
        };
        let text = if sensitive {
            "[REDACTED]".to_string()
        } else {
            node.text
        };
        elements.push(DomNodeSnapshot {
            ref_id: node.ref_id,
            tag: node.tag,
            input_type: node.input_type,
            name: node.name,
            id: node.id,
            aria_label: node.aria_label,
            placeholder: node.placeholder,
            test_id: node.test_id,
            role: node.role,
            text,
            value,
            is_password: sensitive,
            selector: node.selector,
            frame_origin: node.frame_origin,
            rect: node.rect,
        });
    }

    let origin = NormalizedOrigin::parse(&raw.origin)
        .or_else(|_| NormalizedOrigin::parse(&raw.url))
        .map(|o| o.as_str().to_string())
        .unwrap_or(raw.origin);

    BrowserSnapshot {
        url: raw.url,
        origin,
        title: raw.title,
        text_excerpt,
        interactive_elements: elements,
        redacted_field_count: redacted_count.max(raw.redacted_field_count),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WkLocatorAction<'a> {
    Resolve,
    Click,
    Fill {
        value: &'a str,
        allow_sensitive: bool,
    },
}

pub fn build_wk_locator_action_js(
    locator: &SemanticLocator,
    scroll_into_view: bool,
    focus: bool,
    action: WkLocatorAction<'_>,
) -> Result<String, BotBrowserError> {
    let loc_json = serde_json::to_string(locator)
        .map_err(|e| BotBrowserError::Cdp(format!("failed serializing SemanticLocator: {e}")))?
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let scroll_flag = if scroll_into_view { "true" } else { "false" };
    let focus_flag = if focus { "true" } else { "false" };
    let (action_kind, fill_val_lit, allow_sensitive_flag) = match action {
        WkLocatorAction::Resolve => ("\"resolve\"", "null".to_string(), "false"),
        WkLocatorAction::Click => ("\"click\"", "null".to_string(), "false"),
        WkLocatorAction::Fill {
            value,
            allow_sensitive,
        } => {
            let val_lit = serde_json::to_string(value)
                .map_err(|e| BotBrowserError::Cdp(format!("failed serializing fill value: {e}")))?
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029");
            (
                "\"fill\"",
                val_lit,
                if allow_sensitive { "true" } else { "false" },
            )
        }
    };

    let template = r####"(() => {
  const loc = __LOC_JSON__;
  const shouldScroll = __SCROLL_FLAG__;
  const shouldFocus = __FOCUS_FLAG__;
  const actionKind = __ACTION_KIND__;
  const fillVal = __FILL_VAL__;
  const allowSensitive = __ALLOW_SENSITIVE__;

  function isVisible(el) {
    if (!el || el.nodeType !== 1) return false;
    try {
      const style = window.getComputedStyle(el);
      if (style && (style.display === "none" || style.visibility === "hidden" || style.opacity === "0")) {
        return false;
      }
    } catch (_) {}
    if (typeof el.getBoundingClientRect === "function") {
      const r = el.getBoundingClientRect();
      if (r && (r.width > 0 || r.height > 0)) return true;
    }
    return (el.offsetWidth || 0) > 0 || (el.offsetHeight || 0) > 0;
  }

  function pickPreferred(candidates) {
    if (!candidates || candidates.length === 0) return null;
    for (const el of candidates) {
      if (isVisible(el)) return el;
    }
    return candidates[0];
  }

  function hasSensitiveMarker(raw) {
    if (!raw) return false;
    const spaced = String(raw)
      .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, " ")
      .trim();
    if (!spaced) return false;
    const compact = spaced.replace(/\s+/g, "");
    const spacedMarkers = [
      "password", "passwd", "passcode", "one time code", "totp",
      "secret", "api key", "private key", "secret key",
      "access token", "auth token", "session id",
      "authorization", "credential", "mnemonic", "ct0"
    ];
    for (const m of spacedMarkers) {
      if (spaced.includes(m)) return true;
    }
    const compactMarkers = [
      "apikey", "privatekey", "secretkey", "accesstoken",
      "authtoken", "sessionid", "onetimecode"
    ];
    for (const m of compactMarkers) {
      if (compact.includes(m)) return true;
    }
    const tokens = spaced.split(" ");
    for (const tok of tokens) {
      if (tok === "pwd" || tok === "otp" || tok === "token" || tok === "pin" || tok === "cookie" || tok === "seed" || tok === "ct0") {
        return true;
      }
    }
    return false;
  }

  function isSensitive(el) {
    if (!el || el.nodeType !== 1) return false;
    const getAttr = (node, attr) =>
      node && typeof node.getAttribute === "function" ? node.getAttribute(attr) : null;
    const t = String(getAttr(el, "type") || el.type || "").toLowerCase();
    if (t === "password") return true;
    const ac = String(getAttr(el, "autocomplete") || "").toLowerCase();
    if (ac.includes("password") || ac === "one-time-code") return true;
    const ownAttrs = [
      ac,
      getAttr(el, "name"),
      getAttr(el, "id"),
      getAttr(el, "aria-label"),
      getAttr(el, "placeholder"),
      getAttr(el, "data-testid"),
      getAttr(el, "role")
    ];
    for (const a of ownAttrs) {
      if (hasSensitiveMarker(a)) return true;
    }
    const eid = getAttr(el, "id");
    if (eid && typeof document !== "undefined" && document.querySelectorAll) {
      const labels = document.querySelectorAll("label[for]");
      for (const lbl of labels) {
        if (lbl.getAttribute("for") === eid) {
          const ltxt = lbl.innerText || lbl.textContent || getAttr(lbl, "aria-label");
          if (hasSensitiveMarker(ltxt)) return true;
        }
      }
    }
    const labelledBy = getAttr(el, "aria-labelledby");
    if (labelledBy && typeof document !== "undefined" && document.getElementById) {
      for (const refId of String(labelledBy).trim().split(/\s+/)) {
        if (!refId) continue;
        const refEl = document.getElementById(refId);
        if (refEl) {
          const rtxt = refEl.innerText || refEl.textContent || getAttr(refEl, "aria-label");
          if (hasSensitiveMarker(rtxt)) return true;
        }
      }
    }
    let anc = el.parentElement;
    let depth = 0;
    while (anc && anc.nodeType === 1 && depth < 6) {
      const ancTag = String(anc.tagName || "").toLowerCase();
      if (ancTag === "body" || ancTag === "html") break;
      const ancAttrs = [
        getAttr(anc, "data-testid"),
        getAttr(anc, "aria-label"),
        getAttr(anc, "id"),
        getAttr(anc, "name"),
        getAttr(anc, "role")
      ];
      for (const a of ancAttrs) {
        if (hasSensitiveMarker(a)) return true;
      }
      if (ancTag === "label") {
        const wrapTxt = anc.innerText || anc.textContent;
        if (hasSensitiveMarker(wrapTxt)) return true;
      }
      anc = anc.parentElement;
      depth++;
    }
    return false;
  }

  function cssEscapeIdent(val) {
    try {
      if (window.CSS && typeof CSS.escape === "function") return CSS.escape(val);
    } catch (_) {}
    return String(val).replace(/[^a-zA-Z0-9_-]/g, "\\$&");
  }

  function isUniqueSelector(sel, el) {
    if (!sel || typeof document === "undefined" || !document.querySelectorAll) return false;
    try {
      const list = document.querySelectorAll(sel);
      return list.length === 1 && list[0] === el;
    } catch (_) {
      return false;
    }
  }

  function cssPath(el) {
    if (!el || el.nodeType !== 1) return "";
    if (el.id) {
      const idSel = "#" + cssEscapeIdent(el.id);
      if (isUniqueSelector(idSel, el)) return idSel;
    }
    const tid = el.getAttribute("data-testid");
    if (tid) {
      const tidSel = '[data-testid="' + String(tid).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"]';
      if (isUniqueSelector(tidSel, el)) return tidSel;
    }
    const nm = el.getAttribute("name");
    const tag = (el.tagName || "div").toLowerCase();
    if (nm) {
      const nmSel = tag + '[name="' + String(nm).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"]';
      if (isUniqueSelector(nmSel, el)) return nmSel;
    }
    const parts = [];
    let cur = el;
    while (cur && cur.nodeType === 1 && parts.length < 16) {
      const t = (cur.tagName || "div").toLowerCase();
      if (cur !== el && cur.id) {
        const ancIdSel = "#" + cssEscapeIdent(cur.id);
        if (isUniqueSelector(ancIdSel, cur)) {
          parts.unshift(ancIdSel);
          const joinedWithId = parts.join(" > ");
          if (isUniqueSelector(joinedWithId, el)) return joinedWithId;
          break;
        }
      }
      let idx = 1;
      let sib = cur.previousElementSibling;
      while (sib) {
        if (sib.tagName === cur.tagName) idx++;
        sib = sib.previousElementSibling;
      }
      parts.unshift(t + ":nth-of-type(" + idx + ")");
      const candidate = parts.join(" > ");
      if (isUniqueSelector(candidate, el)) return candidate;
      if (t === "html") break;
      cur = cur.parentElement;
    }
    return parts.join(" > ");
  }

  function implicitRole(el) {
    const explicit = el.getAttribute("role");
    if (explicit) return String(explicit).toLowerCase();
    const tag = String(el.tagName || "").toLowerCase();
    if (tag === "a" && el.hasAttribute("href")) return "link";
    if (tag === "button") return "button";
    if (tag === "textarea") return "textbox";
    if (tag === "select") return "combobox";
    if (tag === "input") {
      const tp = String(el.getAttribute("type") || "text").toLowerCase();
      if (tp === "submit" || tp === "button" || tp === "reset") return "button";
      if (tp === "checkbox") return "checkbox";
      if (tp === "radio") return "radio";
      if (tp !== "hidden") return "textbox";
    }
    if (/^h[1-6]$/.test(tag)) return "heading";
    return "";
  }

  function elementText(el) {
    const raw = (el.innerText || el.textContent || el.getAttribute("aria-label") || el.getAttribute("placeholder") || "");
    return String(raw).replace(/\s+/g, " ").trim();
  }

  let matched = null;
  let strategy = null;
  const allEls = Array.from(document.querySelectorAll("*"));

  if (loc.test_id) {
    matched = pickPreferred(allEls.filter(el => el.getAttribute && el.getAttribute("data-testid") === loc.test_id));
    if (matched) strategy = "test_id";
  }
  if (!matched && loc.aria_label) {
    const roleWant = loc.role ? String(loc.role).toLowerCase() : null;
    const cands = allEls.filter(el => el.getAttribute && el.getAttribute("aria-label") === loc.aria_label);
    if (roleWant) matched = pickPreferred(cands.filter(el => implicitRole(el) === roleWant));
    if (!matched) matched = pickPreferred(cands);
    if (matched) strategy = "aria_label";
  }
  if (!matched && loc.name_attr) {
    matched = pickPreferred(allEls.filter(el => el.getAttribute && el.getAttribute("name") === loc.name_attr));
    if (matched) strategy = "name_attr";
  }
  if (!matched && loc.role && loc.text_anchor) {
    const roleWant = String(loc.role).toLowerCase();
    const anchor = String(loc.text_anchor).replace(/\s+/g, " ").trim().toLowerCase();
    const roleCands = allEls.filter(el => implicitRole(el) === roleWant);
    const exact = roleCands.filter(el => elementText(el).toLowerCase() === anchor);
    matched = pickPreferred(exact.length > 0 ? exact : roleCands.filter(el => elementText(el).toLowerCase().includes(anchor)));
    if (matched) strategy = "role_and_text";
  }
  if (!matched && loc.text_anchor) {
    const anchor = String(loc.text_anchor).replace(/\s+/g, " ").trim().toLowerCase();
    const interactive = Array.from(document.querySelectorAll("a, button, input, select, textarea, [role], label"));
    const iExact = interactive.filter(el => elementText(el).toLowerCase() === anchor);
    matched = pickPreferred(iExact.length > 0 ? iExact : interactive.filter(el => elementText(el).toLowerCase().includes(anchor)));
    if (!matched) {
      const leafCands = allEls.filter(el => {
        const tag = String(el.tagName || "").toLowerCase();
        if (tag === "html" || tag === "body" || tag === "script" || tag === "style") return false;
        return elementText(el).toLowerCase().includes(anchor);
      });
      leafCands.sort((a, b) => elementText(a).length - elementText(b).length);
      matched = pickPreferred(leafCands);
    }
    if (matched) strategy = "text_anchor";
  }
  if (!matched && loc.css_selector) {
    try {
      matched = pickPreferred(Array.from(document.querySelectorAll(loc.css_selector)));
      if (matched) strategy = "css_selector";
    } catch (_) {}
  }
  if (!matched && loc.role) {
    const roleWant = String(loc.role).toLowerCase();
    matched = pickPreferred(allEls.filter(el => implicitRole(el) === roleWant));
    if (matched) strategy = "role";
  }

  if (!matched) {
    return JSON.stringify({ found: false });
  }

  const sensitive = isSensitive(matched);
  const tag = String(matched.tagName || "").toLowerCase();
  const uniqueSel = cssPath(matched);

  if (actionKind === "fill" && !allowSensitive && sensitive) {
    return JSON.stringify({
      found: true,
      strategy,
      is_sensitive: true,
      selector: uniqueSel,
      acted: false,
      refused_sensitive: true
    });
  }

  if (shouldScroll && typeof matched.scrollIntoView === "function") {
    try { matched.scrollIntoView({ block: "center", inline: "center" }); } catch (_) {}
  }
  if (shouldFocus && typeof matched.focus === "function") {
    try { matched.focus(); } catch (_) {}
  }

  let acted = false;
  if (actionKind === "click") {
    if (typeof matched.click === "function") {
      matched.click();
    } else {
      matched.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    }
    acted = true;
  } else if (actionKind === "fill") {
    const val = String(fillVal === null ? "" : fillVal);
    if (matched.isContentEditable) {
      try {
        document.execCommand("selectAll", false, null);
        if (!document.execCommand("insertText", false, val)) {
          matched.textContent = val;
        }
      } catch (_) {
        matched.textContent = val;
      }
      matched.dispatchEvent(new Event("input", { bubbles: true }));
      matched.dispatchEvent(new Event("change", { bubbles: true }));
      acted = true;
    } else {
      let proto = null;
      if (tag === "input" && window.HTMLInputElement) proto = window.HTMLInputElement.prototype;
      else if (tag === "textarea" && window.HTMLTextAreaElement) proto = window.HTMLTextAreaElement.prototype;
      else if (tag === "select" && window.HTMLSelectElement) proto = window.HTMLSelectElement.prototype;
      const desc = proto ? Object.getOwnPropertyDescriptor(proto, "value") : null;
      if (desc && typeof desc.set === "function") {
        desc.set.call(matched, val);
      } else {
        matched.value = val;
      }
      matched.dispatchEvent(new Event("input", { bubbles: true }));
      matched.dispatchEvent(new Event("change", { bubbles: true }));
      acted = true;
    }
  }

  const r = typeof matched.getBoundingClientRect === "function" ? matched.getBoundingClientRect() : null;
  const cx = r && r.width > 0 ? Number(r.x || 0) + Number(r.width) / 2 : null;
  const cy = r && r.height > 0 ? Number(r.y || 0) + Number(r.height) / 2 : null;
  let safeText = "";
  if (!sensitive) {
    if ((tag === "input" || tag === "textarea" || tag === "select") && typeof matched.value === "string" && matched.value.length > 0) {
      safeText = matched.value;
    } else {
      safeText = String(matched.innerText || matched.textContent || "").trim();
    }
  }
  return JSON.stringify({
    found: true,
    strategy,
    is_sensitive: sensitive,
    selector: uniqueSel,
    center_x: cx,
    center_y: cy,
    text: safeText,
    acted,
    refused_sensitive: false
  });
})()"####;

    Ok(template
        .replace("__LOC_JSON__", &loc_json)
        .replace("__SCROLL_FLAG__", scroll_flag)
        .replace("__FOCUS_FLAG__", focus_flag)
        .replace("__ACTION_KIND__", action_kind)
        .replace("__FILL_VAL__", &fill_val_lit)
        .replace("__ALLOW_SENSITIVE__", allow_sensitive_flag))
}

pub fn build_wk_resolve_locator_js(
    locator: &SemanticLocator,
    scroll_into_view: bool,
    focus: bool,
) -> Result<String, BotBrowserError> {
    build_wk_locator_action_js(locator, scroll_into_view, focus, WkLocatorAction::Resolve)
}

pub const WK_CAPTURE_SNAPSHOT_JS: &str = r####"(() => {
  const root = document.body || document.documentElement;
  const pageUrl = String(window.location.href || "");
  const pageOrigin = String(window.location.origin || "");
  const title = String(document.title || "");

  function cssEscapeIdent(val) {
    try {
      if (window.CSS && typeof CSS.escape === "function") return CSS.escape(val);
    } catch (_) {}
    return String(val).replace(/[^a-zA-Z0-9_-]/g, "\\$&");
  }

  function isUniqueSelector(sel, el) {
    if (!sel || typeof document === "undefined" || !document.querySelectorAll) return false;
    try {
      const list = document.querySelectorAll(sel);
      return list.length === 1 && list[0] === el;
    } catch (_) {
      return false;
    }
  }

  function cssPath(el) {
    if (!el || el.nodeType !== 1) return "";
    if (el.id) {
      const idSel = "#" + cssEscapeIdent(el.id);
      if (isUniqueSelector(idSel, el)) return idSel;
    }
    const tid = el.getAttribute("data-testid");
    if (tid) {
      const tidSel = '[data-testid="' + String(tid).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"]';
      if (isUniqueSelector(tidSel, el)) return tidSel;
    }
    const tag = (el.tagName || "div").toLowerCase();
    const nm = el.getAttribute("name");
    if (nm) {
      const nmSel = tag + '[name="' + String(nm).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"]';
      if (isUniqueSelector(nmSel, el)) return nmSel;
    }
    const parts = [];
    let cur = el;
    while (cur && cur.nodeType === 1 && parts.length < 16) {
      const t = (cur.tagName || "div").toLowerCase();
      if (cur !== el && cur.id) {
        const ancIdSel = "#" + cssEscapeIdent(cur.id);
        if (isUniqueSelector(ancIdSel, cur)) {
          parts.unshift(ancIdSel);
          const joinedWithId = parts.join(" > ");
          if (isUniqueSelector(joinedWithId, el)) return joinedWithId;
          break;
        }
      }
      let idx = 1;
      let sib = cur.previousElementSibling;
      while (sib) {
        if (sib.tagName === cur.tagName) idx++;
        sib = sib.previousElementSibling;
      }
      parts.unshift(t + ":nth-of-type(" + idx + ")");
      const candidate = parts.join(" > ");
      if (isUniqueSelector(candidate, el)) return candidate;
      if (t === "html") break;
      cur = cur.parentElement;
    }
    return parts.join(" > ");
  }

  function hasSensitiveMarker(raw) {
    if (!raw) return false;
    const spaced = String(raw)
      .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, " ")
      .trim();
    if (!spaced) return false;
    const compact = spaced.replace(/\s+/g, "");
    const spacedMarkers = [
      "password", "passwd", "passcode", "one time code", "totp",
      "secret", "api key", "private key", "secret key",
      "access token", "auth token", "session id",
      "authorization", "credential", "mnemonic", "ct0"
    ];
    for (const m of spacedMarkers) {
      if (spaced.includes(m)) return true;
    }
    const compactMarkers = [
      "apikey", "privatekey", "secretkey", "accesstoken",
      "authtoken", "sessionid", "onetimecode"
    ];
    for (const m of compactMarkers) {
      if (compact.includes(m)) return true;
    }
    const tokens = spaced.split(" ");
    for (const tok of tokens) {
      if (tok === "pwd" || tok === "otp" || tok === "token" || tok === "pin" || tok === "cookie" || tok === "seed" || tok === "ct0") {
        return true;
      }
    }
    return false;
  }

  function isSensitiveField(el) {
    if (!el || el.nodeType !== 1) return false;
    const getAttr = (node, attr) =>
      node && typeof node.getAttribute === "function" ? node.getAttribute(attr) : null;
    const inputType = String(getAttr(el, "type") || el.type || "").toLowerCase();
    if (inputType === "password") return true;
    const ac = String(getAttr(el, "autocomplete") || "").toLowerCase();
    if (ac.includes("password") || ac === "one-time-code") return true;
    const ownAttrs = [
      ac,
      getAttr(el, "name"),
      getAttr(el, "id"),
      getAttr(el, "aria-label"),
      getAttr(el, "placeholder"),
      getAttr(el, "data-testid"),
      getAttr(el, "role")
    ];
    for (const a of ownAttrs) {
      if (hasSensitiveMarker(a)) return true;
    }
    const eid = getAttr(el, "id");
    if (eid && typeof document !== "undefined" && document.querySelectorAll) {
      const labels = document.querySelectorAll("label[for]");
      for (const lbl of labels) {
        if (lbl.getAttribute("for") === eid) {
          const ltxt = lbl.innerText || lbl.textContent || getAttr(lbl, "aria-label");
          if (hasSensitiveMarker(ltxt)) return true;
        }
      }
    }
    const labelledBy = getAttr(el, "aria-labelledby");
    if (labelledBy && typeof document !== "undefined" && document.getElementById) {
      for (const refId of String(labelledBy).trim().split(/\s+/)) {
        if (!refId) continue;
        const refEl = document.getElementById(refId);
        if (refEl) {
          const rtxt = refEl.innerText || refEl.textContent || getAttr(refEl, "aria-label");
          if (hasSensitiveMarker(rtxt)) return true;
        }
      }
    }
    let anc = el.parentElement;
    let depth = 0;
    while (anc && anc.nodeType === 1 && depth < 6) {
      const ancTag = String(anc.tagName || "").toLowerCase();
      if (ancTag === "body" || ancTag === "html") break;
      const ancAttrs = [
        getAttr(anc, "data-testid"),
        getAttr(anc, "aria-label"),
        getAttr(anc, "id"),
        getAttr(anc, "name"),
        getAttr(anc, "role")
      ];
      for (const a of ancAttrs) {
        if (hasSensitiveMarker(a)) return true;
      }
      if (ancTag === "label") {
        const wrapTxt = anc.innerText || anc.textContent;
        if (hasSensitiveMarker(wrapTxt)) return true;
      }
      anc = anc.parentElement;
      depth++;
    }
    return false;
  }

  let textExcerpt = root && typeof root.innerText === "string" ? root.innerText.slice(0, 12000) : "";
  const candidates = root
    ? Array.from(root.querySelectorAll('a, button, input, select, textarea, [data-testid], [role="button"], [role="link"], [role="textbox"], [role="tab"], [role="menuitem"], [role="checkbox"], [role="switch"], [contenteditable]:not([contenteditable="false"])')).slice(0, 200)
    : [];
  let redactedCount = 0;
  const interactive_elements = candidates.map((el, idx) => {
    const sensitive = isSensitiveField(el);
    if (sensitive) redactedCount += 1;
    const rectRaw = typeof el.getBoundingClientRect === "function" ? el.getBoundingClientRect() : null;
    const dpr = Number(window.devicePixelRatio || 1);
    const rect = rectRaw ? {
      x: Number(rectRaw.x || 0),
      y: Number(rectRaw.y || 0),
      width: Number(rectRaw.width || 0),
      height: Number(rectRaw.height || 0),
      device_pixel_ratio: dpr
    } : null;
    const frameOrigin = (el.ownerDocument && el.ownerDocument.location && el.ownerDocument.location.origin) || pageOrigin || null;
    const rawVal = typeof el.value === "string" ? el.value : null;
    const rawText = String(el.innerText || el.textContent || "").trim();
    if (sensitive) {
      if (rawText.length >= 4 && rawText !== "[REDACTED]") {
        textExcerpt = textExcerpt.split(rawText).join("[REDACTED]");
      }
      if (rawVal !== null) {
        const trimmedVal = rawVal.trim();
        if (trimmedVal.length >= 4 && trimmedVal !== "[REDACTED]") {
          textExcerpt = textExcerpt.split(trimmedVal).join("[REDACTED]");
        }
      }
    }
    const value = sensitive ? "[REDACTED]" : (rawVal !== null ? rawVal.slice(0, 500) : null);
    const text = sensitive ? "[REDACTED]" : rawText.slice(0, 200);
    return {
      ref_id: "el_" + (idx + 1),
      tag: String(el.tagName || "").toLowerCase(),
      input_type: el.getAttribute("type") || (typeof el.type === "string" ? el.type : null),
      name: el.getAttribute("name"),
      id: el.getAttribute("id"),
      aria_label: el.getAttribute("aria-label"),
      placeholder: el.getAttribute("placeholder"),
      test_id: el.getAttribute("data-testid"),
      role: el.getAttribute("role"),
      autocomplete: el.getAttribute("autocomplete"),
      text,
      value,
      is_password: sensitive,
      selector: cssPath(el),
      frame_origin: frameOrigin,
      rect
    };
  });

  return JSON.stringify({
    url: pageUrl,
    origin: pageOrigin,
    title,
    text_excerpt: textExcerpt,
    interactive_elements,
    redacted_field_count: redactedCount
  });
})()"####;

type WkBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl BrowserSessionBackend for WkWebViewBackend {
    fn backend_kind(&self) -> BrowserBackendKind {
        BrowserBackendKind::WkWebView
    }

    fn navigate<'life0, 'life1, 'async_trait>(
        &'life0 self,
        url: &'life1 str,
    ) -> WkBoxFuture<'async_trait, Result<NavigationResult, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let allow_file = self.shared.allow_file_urls.load(Ordering::Acquire);
        let validated_origin = validate_wk_navigation_url(url, allow_file)?;

        let prev_seq = self.shared.nav_seq.load(Ordering::Acquire);
        *self.shared.last_nav_error.write() = None;
        let id = self.shared.id;
        let url_owned = url.trim().to_string();
        let shared_clone = Arc::clone(&self.shared);
        let (dispatch_tx, dispatch_rx) = oneshot::channel::<Result<(), BotBrowserError>>();

        run_on_main_thread(move || {
            if !shared_clone.alive.load(Ordering::Acquire) {
                let _ = dispatch_tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
                return;
            }
            WEBVIEW_REGISTRY.with(|reg| {
                let map = reg.borrow();
                let Some(handles) = map.get(&id) else {
                    let _ = dispatch_tx.send(Err(BotBrowserError::Cdp(
                        "WKWebView handle not registered".to_string(),
                    )));
                    return;
                };
                unsafe {
                    let ns_url_str = alloc_nsstring(&url_owned);
                    let ns_url: *mut Object = msg_send![class!(NSURL), URLWithString: ns_url_str];
                    let _: () = msg_send![ns_url_str, release];
                    if ns_url.is_null() {
                        let _ = dispatch_tx.send(Err(BotBrowserError::InvalidOrigin(format!(
                            "invalid URL `{url_owned}`"
                        ))));
                        return;
                    }
                    let req: *mut Object = msg_send![class!(NSURLRequest), requestWithURL: ns_url];
                    if req.is_null() {
                        let _ = dispatch_tx.send(Err(BotBrowserError::Cdp(
                            "failed creating NSURLRequest".to_string(),
                        )));
                        return;
                    }
                    let _: *mut Object = msg_send![handles.webview, loadRequest: req];
                    let _ = dispatch_tx.send(Ok(()));
                }
            });
        });

        match dispatch_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.record_op("navigate", url, started, false, Some(e.to_string()));
                return Err(e);
            }
            Err(_) => {
                let err = BotBrowserError::Cdp("navigation dispatch cancelled".to_string());
                self.record_op("navigate", url, started, false, Some(err.to_string()));
                return Err(err);
            }
        }

        let wait_res = tokio::time::timeout(DEFAULT_NAV_TIMEOUT, async {
            loop {
                let notified = self.shared.nav_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if !self.shared.alive.load(Ordering::Acquire) {
                    return Err(BotBrowserError::Cdp(
                        "WKWebView closed during navigation".to_string(),
                    ));
                }
                if self.shared.nav_seq.load(Ordering::Acquire) > prev_seq {
                    if let Some(err_msg) = self.shared.last_nav_error.read().clone() {
                        return Err(BotBrowserError::Cdp(err_msg));
                    }
                    return Ok(());
                }
                notified.await;
            }
        })
        .await;

        if let Err(_) = wait_res {
            let err = BotBrowserError::Cdp(format!("WKWebView navigation timed out for {url}"));
            self.record_op("navigate", url, started, false, Some(err.to_string()));
            return Err(err);
        }
        if let Ok(Err(err)) = wait_res {
            self.record_op("navigate", url, started, false, Some(err.to_string()));
            return Err(err);
        }

        let cur_url = self.current_url().await.unwrap_or_else(|_| url.trim().to_string());
        let title = self.page_title().await.unwrap_or_default();
        let origin_str = NormalizedOrigin::parse(&cur_url)
            .ok()
            .or(validated_origin)
            .map(|o| o.as_str().to_string())
            .unwrap_or_else(|| cur_url.clone());

        let duration_ms = self.record_op("navigate", &cur_url, started, true, None);
        Ok(NavigationResult {
            url: cur_url,
            origin: origin_str,
            title,
            duration_ms,
            backend: BrowserBackendKind::WkWebView,
        })
        })
    }

    fn go_back<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let id = self.shared.id;
        let shared_clone = Arc::clone(&self.shared);
        let (tx, rx) = oneshot::channel::<Result<(), BotBrowserError>>();

        run_on_main_thread(move || {
            if !shared_clone.alive.load(Ordering::Acquire) {
                let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
                return;
            }
            WEBVIEW_REGISTRY.with(|reg| {
                let map = reg.borrow();
                let Some(handles) = map.get(&id) else {
                    let _ = tx.send(Err(BotBrowserError::Cdp(
                        "WKWebView handle not registered".to_string(),
                    )));
                    return;
                };
                unsafe {
                    let can: BOOL = msg_send![handles.webview, canGoBack];
                    if can == NO {
                        let _ = tx.send(Err(BotBrowserError::Cdp(
                            "WKWebView has no back history entry".to_string(),
                        )));
                        return;
                    }
                    let _: *mut Object = msg_send![handles.webview, goBack];
                    let _ = tx.send(Ok(()));
                }
            });
        });

        let res = match rx.await {
            Ok(r) => r,
            Err(_) => Err(BotBrowserError::Cdp("go_back channel closed".to_string())),
        };
        self.record_op(
            "go_back",
            "history",
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn go_forward<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let id = self.shared.id;
        let shared_clone = Arc::clone(&self.shared);
        let (tx, rx) = oneshot::channel::<Result<(), BotBrowserError>>();

        run_on_main_thread(move || {
            if !shared_clone.alive.load(Ordering::Acquire) {
                let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
                return;
            }
            WEBVIEW_REGISTRY.with(|reg| {
                let map = reg.borrow();
                let Some(handles) = map.get(&id) else {
                    let _ = tx.send(Err(BotBrowserError::Cdp(
                        "WKWebView handle not registered".to_string(),
                    )));
                    return;
                };
                unsafe {
                    let can: BOOL = msg_send![handles.webview, canGoForward];
                    if can == NO {
                        let _ = tx.send(Err(BotBrowserError::Cdp(
                            "WKWebView has no forward history entry".to_string(),
                        )));
                        return;
                    }
                    let _: *mut Object = msg_send![handles.webview, goForward];
                    let _ = tx.send(Ok(()));
                }
            });
        });

        let res = match rx.await {
            Ok(r) => r,
            Err(_) => Err(BotBrowserError::Cdp("go_forward channel closed".to_string())),
        };
        self.record_op(
            "go_forward",
            "history",
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn reload<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let id = self.shared.id;
        let shared_clone = Arc::clone(&self.shared);
        let (tx, rx) = oneshot::channel::<Result<(), BotBrowserError>>();

        run_on_main_thread(move || {
            if !shared_clone.alive.load(Ordering::Acquire) {
                let _ = tx.send(Err(BotBrowserError::Cdp("WKWebView is closed".to_string())));
                return;
            }
            WEBVIEW_REGISTRY.with(|reg| {
                let map = reg.borrow();
                let Some(handles) = map.get(&id) else {
                    let _ = tx.send(Err(BotBrowserError::Cdp(
                        "WKWebView handle not registered".to_string(),
                    )));
                    return;
                };
                unsafe {
                    let _: *mut Object = msg_send![handles.webview, reload];
                    let _ = tx.send(Ok(()));
                }
            });
        });

        let res = match rx.await {
            Ok(r) => r,
            Err(_) => Err(BotBrowserError::Cdp("reload channel closed".to_string())),
        };
        self.record_op(
            "reload",
            "page",
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn current_url<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        self.ensure_alive()?;
        if let Ok(val) = self.eval_js("String(window.location.href || '')").await {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    *self.shared.current_url.write() = s.to_string();
                    return Ok(s.to_string());
                }
            }
        }
        Ok(self.shared.current_url.read().clone())
        })
    }

    fn page_title<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        self.ensure_alive()?;
        if let Ok(val) = self.eval_js("String(document.title || '')").await {
            if let Some(s) = val.as_str() {
                *self.shared.page_title.write() = s.to_string();
                return Ok(s.to_string());
            }
        }
        Ok(self.shared.page_title.read().clone())
        })
    }

    fn snapshot_dom<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<BrowserSnapshot, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let res = match self.eval_js(WK_CAPTURE_SNAPSHOT_JS).await {
            Ok(val) => parse_wk_json_value::<RawWkSnapshot>(val, "snapshot_dom")
                .map(sanitize_wk_snapshot),
            Err(e) => Err(e),
        };
        self.record_op(
            "snapshot_dom",
            "document",
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn eval_js<'life0, 'life1, 'async_trait>(
        &'life0 self,
        script: &'life1 str,
    ) -> WkBoxFuture<'async_trait, Result<Value, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { eval_js_with_shared(&self.shared, script).await })
    }

    fn click_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
    ) -> WkBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        let label = locator.display_label();
        self.ensure_alive()?;
        if locator.is_empty() {
            let err = BotBrowserError::LocatorNotFound("empty semantic locator".to_string());
            self.record_op("click_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }
        let click_js = match build_wk_locator_action_js(locator, true, false, WkLocatorAction::Click) {
            Ok(js) => js,
            Err(e) => {
                self.record_op("click_locator", &label, started, false, Some(e.to_string()));
                return Err(e);
            }
        };
        let resolved: ResolvedWkLocator = match self.eval_js(&click_js).await {
            Ok(val) => match parse_wk_json_value(val, "click_locator") {
                Ok(r) => r,
                Err(e) => {
                    self.record_op("click_locator", &label, started, false, Some(e.to_string()));
                    return Err(e);
                }
            },
            Err(e) => {
                self.record_op("click_locator", &label, started, false, Some(e.to_string()));
                return Err(e);
            }
        };
        if !resolved.found || !resolved.acted {
            let err = BotBrowserError::LocatorNotFound(label.clone());
            self.record_op("click_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }
        let duration_ms = self.record_op("click_locator", &label, started, true, None);
        Ok(ActionOutcome {
            ok: true,
            operation: "click_locator".to_string(),
            duration_ms,
            matched_strategy: resolved.strategy,
            matched_selector: resolved.selector,
            x: resolved.center_x,
            y: resolved.center_y,
            extracted_text: None,
        })
        })
    }

    fn fill_locator<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        value: &'life2 str,
        is_secret: bool,
    ) -> WkBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        let label = locator.display_label();
        self.ensure_alive()?;
        if locator.is_empty() {
            let err = BotBrowserError::LocatorNotFound("empty semantic locator".to_string());
            self.record_op("fill_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }

        if !is_secret && is_sensitive_locator_hint(locator) {
            let err = BotBrowserError::Cdp(format!(
                "refusing non-secret fill into sensitive locator ({label})"
            ));
            self.record_op("fill_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }

        let fill_js = match build_wk_locator_action_js(
            locator,
            true,
            true,
            WkLocatorAction::Fill {
                value,
                allow_sensitive: is_secret,
            },
        ) {
            Ok(js) => js,
            Err(e) => {
                self.record_op("fill_locator", &label, started, false, Some(e.to_string()));
                return Err(e);
            }
        };

        let resolved: ResolvedWkLocator = match self.eval_js(&fill_js).await {
            Ok(val) => match parse_wk_json_value(val, "fill_locator") {
                Ok(r) => r,
                Err(e) => {
                    self.record_op("fill_locator", &label, started, false, Some(e.to_string()));
                    return Err(e);
                }
            },
            Err(e) => {
                self.record_op("fill_locator", &label, started, false, Some(e.to_string()));
                return Err(e);
            }
        };

        if !resolved.found {
            let err = BotBrowserError::LocatorNotFound(label.clone());
            self.record_op("fill_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }

        if resolved.refused_sensitive || (!is_secret && resolved.is_sensitive) {
            let err = BotBrowserError::Cdp(format!(
                "refusing non-secret fill into sensitive/password element ({label})"
            ));
            self.record_op("fill_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }

        if !resolved.acted {
            let err = BotBrowserError::LocatorNotFound(label.clone());
            self.record_op("fill_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }

        let detail = if is_secret {
            Some("secret_filled".to_string())
        } else {
            None
        };
        let duration_ms = self.record_op("fill_locator", &label, started, true, detail);
        Ok(ActionOutcome {
            ok: true,
            operation: "fill_locator".to_string(),
            duration_ms,
            matched_strategy: resolved.strategy,
            matched_selector: resolved.selector,
            x: resolved.center_x,
            y: resolved.center_y,
            extracted_text: None,
        })
        })
    }

    fn press_key<'life0, 'life1, 'async_trait>(
        &'life0 self,
        key: &'life1 str,
    ) -> WkBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let res = self
            .dispatch_key_event(KeyInputEvent {
                kind: KeyEventKind::KeyDown,
                key: key.to_string(),
                code: None,
                text: None,
                windows_virtual_key_code: None,
                modifiers: 0,
            })
            .await;
        let res = match res {
            Ok(()) => {
                self.dispatch_key_event(KeyInputEvent {
                    kind: KeyEventKind::KeyUp,
                    key: key.to_string(),
                    code: None,
                    text: None,
                    windows_virtual_key_code: None,
                    modifiers: 0,
                })
                .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            self.record_op("press_key", key, started, false, Some(e.to_string()));
            return Err(e);
        }
        let duration_ms = self.record_op("press_key", key, started, true, None);
        Ok(ActionOutcome {
            ok: true,
            operation: "press_key".to_string(),
            duration_ms,
            matched_strategy: None,
            matched_selector: None,
            x: None,
            y: None,
            extracted_text: None,
        })
        })
    }

    fn scroll<'life0, 'async_trait>(
        &'life0 self,
        delta_x: f64,
        delta_y: f64,
    ) -> WkBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        if !delta_x.is_finite() || !delta_y.is_finite() {
            let err = BotBrowserError::Cdp(format!(
                "scroll requires finite deltas, got ({delta_x}, {delta_y})"
            ));
            self.record_op(
                "scroll",
                format!("({delta_x}, {delta_y})"),
                started,
                false,
                Some(err.to_string()),
            );
            return Err(err);
        }
        let js = format!("window.scrollBy({delta_x}, {delta_y}); true");
        let res = self.eval_js(&js).await.map(|_| ());
        if let Err(e) = res {
            self.record_op(
                "scroll",
                format!("({delta_x}, {delta_y})"),
                started,
                false,
                Some(e.to_string()),
            );
            return Err(e);
        }
        let duration_ms =
            self.record_op("scroll", format!("({delta_x}, {delta_y})"), started, true, None);
        Ok(ActionOutcome {
            ok: true,
            operation: "scroll".to_string(),
            duration_ms,
            matched_strategy: None,
            matched_selector: None,
            x: None,
            y: None,
            extracted_text: None,
        })
        })
    }

    fn wait_for_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        timeout_ms: u64,
    ) -> WkBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        let label = locator.display_label();
        self.ensure_alive()?;
        if locator.is_empty() {
            let err = BotBrowserError::LocatorNotFound("empty semantic locator".to_string());
            self.record_op("wait_for_locator", &label, started, false, Some(err.to_string()));
            return Err(err);
        }

        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(timeout_ms.max(1)));
        loop {
            match self.resolve_locator_internal(locator, false, false).await {
                Ok(resolved) => {
                    let duration_ms =
                        self.record_op("wait_for_locator", &label, started, true, None);
                    return Ok(ActionOutcome {
                        ok: true,
                        operation: "wait_for_locator".to_string(),
                        duration_ms,
                        matched_strategy: resolved.strategy,
                        matched_selector: resolved.selector,
                        x: resolved.center_x,
                        y: resolved.center_y,
                        extracted_text: None,
                    });
                }
                Err(BotBrowserError::LocatorNotFound(_)) => {
                    if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                        let err = BotBrowserError::LocatorNotFound(format!(
                            "timed out after {timeout_ms}ms waiting for {label}"
                        ));
                        self.record_op(
                            "wait_for_locator",
                            &label,
                            started,
                            false,
                            Some(err.to_string()),
                        );
                        return Err(err);
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(err) => {
                    self.record_op(
                        "wait_for_locator",
                        &label,
                        started,
                        false,
                        Some(err.to_string()),
                    );
                    return Err(err);
                }
            }
        }
        })
    }

    fn extract_text_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
    ) -> WkBoxFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        let label = locator.display_label();
        self.ensure_alive()?;
        if is_sensitive_locator_hint(locator) {
            let err = BotBrowserError::Cdp(format!(
                "refusing to extract value from sensitive locator ({label})"
            ));
            self.record_op(
                "extract_text_locator",
                &label,
                started,
                false,
                Some(err.to_string()),
            );
            return Err(err);
        }
        let resolved = match self.resolve_locator_internal(locator, false, false).await {
            Ok(r) => r,
            Err(e) => {
                self.record_op(
                    "extract_text_locator",
                    &label,
                    started,
                    false,
                    Some(e.to_string()),
                );
                return Err(e);
            }
        };
        if resolved.is_sensitive {
            let err = BotBrowserError::Cdp(format!(
                "refusing to extract value from sensitive field ({label})"
            ));
            self.record_op(
                "extract_text_locator",
                &label,
                started,
                false,
                Some(err.to_string()),
            );
            return Err(err);
        }
        self.record_op("extract_text_locator", &label, started, true, None);
        Ok(resolved.text.unwrap_or_default())
        })
    }

    fn capture_screenshot<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<Vec<u8>, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let res = capture_snapshot_with_shared(&self.shared).await;
        self.record_op(
            "capture_screenshot",
            "viewport",
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn export_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        domain_filter: Option<&'life1 str>,
    ) -> WkBoxFuture<'async_trait, Result<Vec<BrowserCookie>, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let res = export_cookies_with_shared(&self.shared, domain_filter).await;
        self.record_op(
            "export_cookies",
            domain_filter.unwrap_or("*"),
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn import_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        cookies: &'life1 [BrowserCookie],
    ) -> WkBoxFuture<'async_trait, Result<usize, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        let started = Instant::now();
        self.ensure_alive()?;
        let res = import_cookies_with_shared(&self.shared, cookies).await;
        self.record_op(
            "import_cookies",
            format!("{} cookies", cookies.len()),
            started,
            res.is_ok(),
            res.as_ref().err().map(ToString::to_string),
        );
        res
        })
    }

    fn dispatch_pointer_event<'life0, 'async_trait>(
        &'life0 self,
        event: PointerInputEvent,
    ) -> WkBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        self.ensure_alive()?;
        if !event.x.is_finite()
            || !event.y.is_finite()
            || !event.delta_x.is_finite()
            || !event.delta_y.is_finite()
        {
            return Err(BotBrowserError::Cdp(format!(
                "dispatch_pointer_event requires finite coordinates and deltas, got ({}, {}) delta ({}, {})",
                event.x, event.y, event.delta_x, event.delta_y
            )));
        }
        let event_name = match event.kind {
            PointerEventKind::Move => "mousemove",
            PointerEventKind::Down => "mousedown",
            PointerEventKind::Up => "mouseup",
            PointerEventKind::Wheel => "wheel",
        };
        let button_idx: i32 = match event.button {
            PointerButton::None | PointerButton::Left => 0,
            PointerButton::Middle => 1,
            PointerButton::Right => 2,
        };
        let should_synth_click =
            event.kind == PointerEventKind::Up && matches!(event.button, PointerButton::Left);
        let x = event.x;
        let y = event.y;
        let dx = event.delta_x;
        let dy = event.delta_y;
        let alt = (event.modifiers & 1) != 0;
        let ctrl = (event.modifiers & 2) != 0;
        let meta = (event.modifiers & 4) != 0;
        let shift = (event.modifiers & 8) != 0;
        let js = format!(
            r#"(() => {{
  const target = document.elementFromPoint({x}, {y}) || document.body || document.documentElement;
  if (!target) return false;
  if ("{event_name}" === "wheel") {{
    target.dispatchEvent(new WheelEvent("wheel", {{
      bubbles: true, cancelable: true, clientX: {x}, clientY: {y},
      deltaX: {dx}, deltaY: {dy},
      altKey: {alt}, ctrlKey: {ctrl}, metaKey: {meta}, shiftKey: {shift}
    }}));
    window.scrollBy({dx}, {dy});
    return true;
  }}
  target.dispatchEvent(new MouseEvent("{event_name}", {{
    bubbles: true, cancelable: true, clientX: {x}, clientY: {y},
    button: {button_idx},
    altKey: {alt}, ctrlKey: {ctrl}, metaKey: {meta}, shiftKey: {shift}
  }}));
  if ({should_synth_click} && typeof target.click === "function") {{
    target.click();
  }}
  return true;
}})()"#
        );
        self.eval_js(&js).await.map(|_| ())
        })
    }

    fn dispatch_key_event<'life0, 'async_trait>(
        &'life0 self,
        event: KeyInputEvent,
    ) -> WkBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        self.ensure_alive()?;
        if event.kind == KeyEventKind::Char {
            if let Some(ref text) = event.text {
                let text_lit = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
                let js = format!(
                    r#"(() => {{
  const el = document.activeElement;
  if (!el) return false;
  const txt = {text_lit};
  if (el.isContentEditable) {{
    try {{ document.execCommand('insertText', false, txt); }} catch (_) {{ el.textContent = (el.textContent || '') + txt; }}
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    return true;
  }}
  if (typeof el.value === 'string') {{
    el.value = el.value + txt;
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    return true;
  }}
  return false;
}})()"#
                );
                return self.eval_js(&js).await.map(|_| ());
            }
        }
        let kind_str = match event.kind {
            KeyEventKind::KeyDown => "keydown",
            KeyEventKind::KeyUp => "keyup",
            KeyEventKind::Char => "keypress",
        };
        let key_lit = serde_json::to_string(&event.key).unwrap_or_else(|_| "\"\"".to_string());
        let code_lit = serde_json::to_string(event.code.as_deref().unwrap_or(""))
            .unwrap_or_else(|_| "\"\"".to_string());
        let alt = (event.modifiers & 1) != 0;
        let ctrl = (event.modifiers & 2) != 0;
        let meta = (event.modifiers & 4) != 0;
        let shift = (event.modifiers & 8) != 0;
        let js = format!(
            r#"(() => {{
  const el = document.activeElement || document.body || document.documentElement;
  if (!el) return false;
  el.dispatchEvent(new KeyboardEvent("{kind_str}", {{
    bubbles: true, cancelable: true, key: {key_lit}, code: {code_lit},
    altKey: {alt}, ctrlKey: {ctrl}, metaKey: {meta}, shiftKey: {shift}
  }}));
  return true;
}})()"#
        );
        self.eval_js(&js).await.map(|_| ())
        })
    }

    fn performance_metrics(&self) -> BrowserPerformanceMetrics {
        match self.telemetry.lock() {
            Ok(guard) => guard.snapshot(),
            Err(_) => BrowserPerformanceMetrics {
                backend: Some(BrowserBackendKind::WkWebView),
                ..Default::default()
            },
        }
    }

    fn close<'life0, 'async_trait>(
        &'life0 self,
    ) -> WkBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
        self.shared.alive.store(false, Ordering::Release);
        self.shared.nav_notify.notify_waiters();
        Ok(())
        })
    }
}
