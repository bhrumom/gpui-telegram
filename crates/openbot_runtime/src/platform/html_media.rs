//! Sandboxed HTML widgets (`preview`, `page`, `backdrop`), `openbotPage` JSON-RPC bridge,
//! backdrop scene validation, and credit-metered media generation gating (`HtmlAndCreditMediaService`, `OB-F12`).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_storage::{JsonPreferenceStore, preference_files};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const HTML_LIMIT: usize = 512_000;
pub const MIN_HEIGHT: u32 = 80;
pub const MAX_HEIGHT: u32 = 2_000;
pub const DEFAULT_HEIGHT: u32 = 240;
pub const MIN_PREVIEW_WIDTH: u32 = 240;
pub const MAX_PREVIEW_WIDTH: u32 = 1_600;
pub const DEFAULT_PREVIEW_WIDTH: u32 = 728;
pub const CONTEXT_LIMIT: usize = 2_000;
pub const CSP_HEADER: &str = "sandbox allow-scripts allow-forms";
pub const DEFAULT_MEDIA_APPROVAL_CREDITS: i64 = 50;

const MAX_MEDIA_APPROVAL_CREDITS: i64 = 100_000;
const CREDIT_MEDIA_PROMPT_LIMIT: usize = 4_000;
const TITLE_LIMIT: usize = 200;
const CONTENT_BLOCK_LIMIT: usize = 32;
const IMAGE_SOURCE_LIMIT: usize = 8 * 1_024 * 1_024;
const DEFAULT_IMAGE_MODEL: &str = "gpt-image-2.6";
const DEFAULT_VIDEO_MODEL: &str = "seedance-2.5";
const DEFAULT_IMAGE_COST: i64 = 2;
const DEFAULT_VIDEO_COST: i64 = 39;
const DEFAULT_WALLET_BALANCE: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HtmlRenderMode {
    Preview,
    Page,
    Backdrop,
}

impl HtmlRenderMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Page => "page",
            Self::Backdrop => "backdrop",
        }
    }

    fn parse(raw: Option<&str>) -> RuntimeResult<Self> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Self::Page),
            Some(s) if s.eq_ignore_ascii_case("preview") => Ok(Self::Preview),
            Some(s) if s.eq_ignore_ascii_case("page") => Ok(Self::Page),
            Some(s) if s.eq_ignore_ascii_case("backdrop") => Ok(Self::Backdrop),
            Some(other) => Err(OpenBotRuntimeError::Validation(format!(
                "Invalid html render mode '{other}'; expected preview, page, or backdrop."
            ))),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HtmlRenderRequest {
    pub mode: Option<String>,
    pub html: Option<String>,
    pub title: Option<String>,
    pub frame_height: Option<u32>,
    pub preview_width: Option<u32>,
    pub appearance: Option<String>,
    pub bubbles_yours: Option<String>,
    pub bubbles_agents: Option<String>,
    pub backdrop: Option<String>,
    pub rpc_message: Option<Value>,
    pub generate_media: bool,
    pub media_kind: Option<String>,
    pub media_model: Option<String>,
    pub media_prompt: Option<String>,
    pub cost_credits: Option<i64>,
    pub wallet_balance: Option<i64>,
    pub approval_threshold: Option<i64>,
    pub agents_credits_allowed: Option<bool>,
    pub approve_media: bool,
    pub decline_media: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotPageRpcOutcome {
    pub rpc_type: String,
    pub id: Option<Value>,
    pub refused: bool,
    pub payload: Value,
    pub reply_json: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CreditMediaOutcome {
    Generated,
    Declined,
    Short,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HtmlRenderOutcome {
    pub mode: String,
    pub sandboxed: bool,
    pub csp_header: String,
    pub csp_enforced: bool,
    pub openbot_page: bool,
    pub frame_height: Option<u32>,
    pub preview_width: Option<u32>,
    pub appearance: String,
    pub rendered_html: Option<String>,
    pub bubbles_meta: Option<String>,
    pub rpc_result: Option<Value>,
    pub backdrop: Option<String>,
    pub media_kind: Option<String>,
    pub media_model: Option<String>,
    pub cost_credits: Option<i64>,
    pub approval_threshold: i64,
    pub approval_required: bool,
    pub outcome: Option<CreditMediaOutcome>,
    pub charged_credits: Option<i64>,
    pub remaining_balance: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaApprovalCreditsSetting {
    agents: bool,
    ask_when_blocked: bool,
    credits: i64,
    image_model: Option<String>,
    video_model: Option<String>,
}

impl Default for MediaApprovalCreditsSetting {
    fn default() -> Self {
        Self {
            agents: true,
            ask_when_blocked: true,
            credits: DEFAULT_MEDIA_APPROVAL_CREDITS,
            image_model: None,
            video_model: None,
        }
    }
}

pub struct HtmlAndCreditMediaService {
    media_settings: JsonPreferenceStore<MediaApprovalCreditsSetting>,
}

impl HtmlAndCreditMediaService {
    pub fn open(data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let path = data_dir
            .as_ref()
            .join(preference_files::MEDIA_APPROVAL_CREDITS_V1);
        let media_settings =
            JsonPreferenceStore::new(path, MediaApprovalCreditsSetting::default());
        let _ = media_settings.load()?;
        Ok(Arc::new(Self { media_settings }))
    }

    pub fn validate_backdrop(&self, scene: &str) -> RuntimeResult<String> {
        let normalized = scene.trim().to_ascii_lowercase();
        let canonical = match normalized.as_str() {
            "pond" => "pond",
            "jellyfish" | "forest" => "jellyfish",
            "sunset" | "city" => "sunset",
            "snowfall" | "snow" => "snowfall",
            "fireflies" | "desert" => "fireflies",
            "coast" => "coast",
            _ => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Unknown backdrop scene '{scene}'; expected one of pond, jellyfish, sunset, snowfall, fireflies, coast."
                )));
            }
        };
        Ok(canonical.to_string())
    }

    pub fn evaluate_openbot_page_rpc(&self, msg: &Value) -> OpenBotPageRpcOutcome {
        let Some(obj) = msg.as_object() else {
            return refused_rpc(None, "RPC payload must be a JSON object.");
        };
        let id = extract_rpc_id(obj.get("id"));
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return refused_rpc(id, "jsonrpc must equal '2.0'.");
        }
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return refused_rpc(id, "Missing JSON-RPC method.");
        };
        let Some(params) = obj.get("params").and_then(Value::as_object) else {
            return refused_rpc(id, "Missing JSON-RPC params object.");
        };

        match method {
            "ui/notifications/size-changed" => {
                let Some(height_f) = params.get("height").and_then(Value::as_f64) else {
                    return refused_rpc(None, "size-changed requires a finite non-negative height.");
                };
                if !height_f.is_finite() || height_f < 0.0 {
                    return refused_rpc(None, "size-changed height must be finite and >= 0.");
                }
                let clamped = (height_f.ceil() as u64).clamp(MIN_HEIGHT as u64, MAX_HEIGHT as u64) as u32;
                OpenBotPageRpcOutcome {
                    rpc_type: "size".to_string(),
                    id: None,
                    refused: false,
                    payload: json!({ "height": clamped }),
                    reply_json: None,
                }
            }
            "ui/open-link" => {
                let Some(req_id) = id else {
                    return refused_rpc(None, "ui/open-link requires a valid JSON-RPC id.");
                };
                let Some(raw_url) = params.get("url").and_then(Value::as_str) else {
                    return refused_rpc(Some(req_id), "ui/open-link requires a string url.");
                };
                if utf16_len(raw_url) > INPUT_LIMITS.browser_url {
                    return refused_rpc(Some(req_id), "ui/open-link url exceeds maximum length.");
                }
                let Ok(parsed) = Url::parse(raw_url) else {
                    return refused_rpc(Some(req_id), "ui/open-link url is malformed.");
                };
                if !matches!(parsed.scheme(), "http" | "https") {
                    return refused_rpc(
                        Some(req_id),
                        "ui/open-link only permits http: and https: URLs.",
                    );
                }
                OpenBotPageRpcOutcome {
                    rpc_type: "open-link".to_string(),
                    id: Some(req_id.clone()),
                    refused: false,
                    payload: json!({ "url": parsed.as_str() }),
                    reply_json: Some(json!({ "jsonrpc": "2.0", "id": req_id, "result": {} })),
                }
            }
            "ui/request-display-mode" => {
                let Some(req_id) = id else {
                    return refused_rpc(None, "ui/request-display-mode requires a valid JSON-RPC id.");
                };
                let Some(mode) = params.get("mode").and_then(Value::as_str) else {
                    return refused_rpc(Some(req_id), "ui/request-display-mode requires mode.");
                };
                if !matches!(mode, "inline" | "fullscreen") {
                    return refused_rpc(
                        Some(req_id),
                        "Display mode must be 'inline' or 'fullscreen'.",
                    );
                }
                OpenBotPageRpcOutcome {
                    rpc_type: "display-mode".to_string(),
                    id: Some(req_id.clone()),
                    refused: false,
                    payload: json!({ "mode": mode }),
                    reply_json: Some(
                        json!({ "jsonrpc": "2.0", "id": req_id, "result": { "mode": mode } }),
                    ),
                }
            }
            "ui/message" => {
                let Some(req_id) = id else {
                    return refused_rpc(None, "ui/message requires a valid JSON-RPC id.");
                };
                if let Some(role) = params.get("role").and_then(Value::as_str)
                    && role != "user"
                {
                    return refused_rpc(Some(req_id), "ui/message role must be 'user'.");
                }
                let Some(text) = extract_message_text(params, INPUT_LIMITS.message_text, false) else {
                    return refused_rpc(
                        Some(req_id),
                        "ui/message text is missing, empty, or exceeds 100000 characters.",
                    );
                };
                OpenBotPageRpcOutcome {
                    rpc_type: "message".to_string(),
                    id: Some(req_id.clone()),
                    refused: false,
                    payload: json!({ "text": text }),
                    reply_json: Some(json!({ "jsonrpc": "2.0", "id": req_id, "result": {} })),
                }
            }
            "ui/update-model-context" => {
                let Some(req_id) = id else {
                    return refused_rpc(None, "ui/update-model-context requires a valid JSON-RPC id.");
                };
                let Some(text) = extract_context_text(params, CONTEXT_LIMIT) else {
                    return refused_rpc(
                        Some(req_id),
                        "ui/update-model-context payload is invalid or exceeds 2000 characters.",
                    );
                };
                OpenBotPageRpcOutcome {
                    rpc_type: "model-context".to_string(),
                    id: Some(req_id.clone()),
                    refused: false,
                    payload: json!({ "text": text }),
                    reply_json: Some(json!({ "jsonrpc": "2.0", "id": req_id, "result": {} })),
                }
            }
            "openbot/notifications/open-image" => {
                let Some(src) = params.get("src").and_then(Value::as_str) else {
                    return refused_rpc(None, "open-image requires a string src.");
                };
                if src.len() > IMAGE_SOURCE_LIMIT || !is_allowed_image_src(src) {
                    return refused_rpc(
                        None,
                        "open-image src must be https: or an allowed data:image/* URI <= 8 MiB.",
                    );
                }
                let name = match params.get("name") {
                    None => String::new(),
                    Some(Value::String(s)) if utf16_len(s) <= TITLE_LIMIT => s.clone(),
                    _ => return refused_rpc(None, "open-image name must be a string <= 200 chars."),
                };
                OpenBotPageRpcOutcome {
                    rpc_type: "open-image".to_string(),
                    id: None,
                    refused: false,
                    payload: json!({ "src": src, "name": name }),
                    reply_json: None,
                }
            }
            other => refused_rpc(id, &format!("Unsupported openbotPage RPC method '{other}'.")),
        }
    }

    pub fn render_or_generate(&self, req: HtmlRenderRequest) -> RuntimeResult<HtmlRenderOutcome> {
        let mode = HtmlRenderMode::parse(req.mode.as_deref())?;
        let appearance = match req
            .appearance
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => "dark".to_string(),
            Some(a) if a.eq_ignore_ascii_case("dark") => "dark".to_string(),
            Some(a) if a.eq_ignore_ascii_case("light") => "light".to_string(),
            Some(other) => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid appearance '{other}'; expected 'dark' or 'light'."
                )));
            }
        };

        if let Some(h) = req.frame_height
            && !(MIN_HEIGHT..=MAX_HEIGHT).contains(&h)
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "frameHeight {h} is out of bounds; must be between {MIN_HEIGHT} and {MAX_HEIGHT}."
            )));
        }
        if let Some(w) = req.preview_width
            && !(MIN_PREVIEW_WIDTH..=MAX_PREVIEW_WIDTH).contains(&w)
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "previewWidth {w} is out of bounds; must be between {MIN_PREVIEW_WIDTH} and {MAX_PREVIEW_WIDTH}."
            )));
        }
        if let Some(title) = req.title.as_deref()
            && utf16_len(title) > TITLE_LIMIT
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "HTML title exceeds maximum length of {TITLE_LIMIT} characters."
            )));
        }
        if let Some(html) = req.html.as_deref() {
            let len = utf16_len(html);
            if len > HTML_LIMIT {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "HTML content length ({len}) exceeds limit of {HTML_LIMIT} characters."
                )));
            }
        }

        let backdrop = match req
            .backdrop
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(scene) => Some(self.validate_backdrop(scene)?),
            None => None,
        };

        let bubbles_meta = build_bubbles_meta(
            req.bubbles_yours.as_deref(),
            req.bubbles_agents.as_deref(),
        )?;

        let openbot_page = mode == HtmlRenderMode::Page;
        let rendered_html = req.html.as_deref().map(|raw| {
            build_sandboxed_document(
                raw,
                &appearance,
                req.title.as_deref(),
                openbot_page,
                bubbles_meta.as_deref(),
            )
        });

        let rpc_result = req.rpc_message.as_ref().map(|msg| {
            let rpc = if openbot_page {
                self.evaluate_openbot_page_rpc(msg)
            } else {
                refused_rpc(
                    msg.get("id").cloned(),
                    "openbotPage RPC bridge is only available in page mode.",
                )
            };
            json!({
                "type": rpc.rpc_type,
                "rpcType": rpc.rpc_type,
                "id": rpc.id,
                "refused": rpc.refused,
                "payload": rpc.payload,
                "replyJson": rpc.reply_json,
            })
        });

        let mut persisted = self.media_settings.get();
        let threshold = req.approval_threshold.unwrap_or(persisted.credits);
        if !(0..=MAX_MEDIA_APPROVAL_CREDITS).contains(&threshold) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "approvalThreshold ({threshold}) must be between 0 and {MAX_MEDIA_APPROVAL_CREDITS}."
            )));
        }
        let agents_allowed = req.agents_credits_allowed.unwrap_or(persisted.agents);
        if req.approval_threshold.is_some()
            || req.agents_credits_allowed.is_some()
            || (req.generate_media && !self.media_settings.path().exists())
        {
            persisted.credits = threshold;
            persisted.agents = agents_allowed;
            let _ = self.media_settings.set(persisted.clone())?;
        }

        let (
            media_kind,
            media_model,
            cost_credits,
            approval_required,
            outcome,
            charged_credits,
            remaining_balance,
        ) = if req.generate_media {
            let kind = match req
                .media_kind
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                None => "image".to_string(),
                Some(k) if k.eq_ignore_ascii_case("image") => "image".to_string(),
                Some(k) if k.eq_ignore_ascii_case("video") => "video".to_string(),
                Some(other) => {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "Invalid media kind '{other}'; expected 'image' or 'video'."
                    )));
                }
            };
            if let Some(prompt) = req.media_prompt.as_deref()
                && utf16_len(prompt) > CREDIT_MEDIA_PROMPT_LIMIT
            {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Media prompt exceeds limit of {CREDIT_MEDIA_PROMPT_LIMIT} characters."
                )));
            }
            let model = req
                .media_model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    if kind == "video" {
                        persisted.video_model.clone()
                    } else {
                        persisted.image_model.clone()
                    }
                })
                .unwrap_or_else(|| {
                    if kind == "video" {
                        DEFAULT_VIDEO_MODEL.to_string()
                    } else {
                        DEFAULT_IMAGE_MODEL.to_string()
                    }
                });
            let cost = req.cost_credits.unwrap_or(if kind == "video" {
                DEFAULT_VIDEO_COST
            } else {
                DEFAULT_IMAGE_COST
            });
            if cost < 0 {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "costCredits ({cost}) must be non-negative."
                )));
            }
            let balance = req.wallet_balance.unwrap_or(DEFAULT_WALLET_BALANCE);
            let needs_approval = cost > threshold;

            let (media_outcome, charged, remaining) = if !agents_allowed {
                (CreditMediaOutcome::Unavailable, 0, balance)
            } else if cost > balance {
                (CreditMediaOutcome::Short, 0, balance)
            } else if req.decline_media || (needs_approval && !req.approve_media) {
                (CreditMediaOutcome::Declined, 0, balance)
            } else {
                (CreditMediaOutcome::Generated, cost, balance - cost)
            };

            (
                Some(kind),
                Some(model),
                Some(cost),
                needs_approval,
                Some(media_outcome),
                Some(charged),
                Some(remaining),
            )
        } else {
            (None, None, None, false, None, None, None)
        };

        let resolved_height = match mode {
            HtmlRenderMode::Page => Some(req.frame_height.unwrap_or(DEFAULT_HEIGHT)),
            HtmlRenderMode::Preview | HtmlRenderMode::Backdrop => req.frame_height,
        };
        let resolved_width = match mode {
            HtmlRenderMode::Preview => Some(req.preview_width.unwrap_or(DEFAULT_PREVIEW_WIDTH)),
            HtmlRenderMode::Page | HtmlRenderMode::Backdrop => req.preview_width,
        };

        Ok(HtmlRenderOutcome {
            mode: mode.as_str().to_string(),
            sandboxed: true,
            csp_header: CSP_HEADER.to_string(),
            csp_enforced: true,
            openbot_page,
            frame_height: resolved_height,
            preview_width: resolved_width,
            appearance,
            rendered_html,
            bubbles_meta,
            rpc_result,
            backdrop,
            media_kind,
            media_model,
            cost_credits,
            approval_threshold: threshold,
            approval_required,
            outcome,
            charged_credits,
            remaining_balance,
        })
    }

    pub fn list_media_models(&self, kind: Option<&str>) -> RuntimeResult<Value> {
        let filter = match kind.map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(k) if k.eq_ignore_ascii_case("image") => Some("image"),
            Some(k) if k.eq_ignore_ascii_case("video") => Some("video"),
            Some(other) => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid media kind '{other}'; expected 'image' or 'video'."
                )));
            }
        };
        let persisted = self.media_settings.get();
        let default_image = persisted
            .image_model
            .clone()
            .unwrap_or_else(|| DEFAULT_IMAGE_MODEL.to_string());
        let default_video = persisted
            .video_model
            .clone()
            .unwrap_or_else(|| DEFAULT_VIDEO_MODEL.to_string());

        let mut models = Vec::new();
        if filter.is_none() || filter == Some("image") {
            models.push(json!({
                "id": DEFAULT_IMAGE_MODEL,
                "kind": "image",
                "default": default_image == DEFAULT_IMAGE_MODEL,
                "costCredits": DEFAULT_IMAGE_COST,
                "resolutions": ["1K", "2K"],
                "aspects": ["1:1", "4:3", "3:4", "16:9", "9:16"],
            }));
            models.push(json!({
                "id": "gpt-image-1.5",
                "kind": "image",
                "default": default_image == "gpt-image-1.5",
                "costCredits": DEFAULT_IMAGE_COST,
                "resolutions": ["1K", "2K"],
                "aspects": ["1:1", "4:3", "3:4", "16:9", "9:16"],
            }));
            models.push(json!({
                "id": "nano-banana-pro",
                "kind": "image",
                "default": default_image == "nano-banana-pro",
                "costCredits": 4,
                "resolutions": ["1K", "2K", "4K"],
                "aspects": ["1:1", "3:2", "2:3", "16:9", "9:16"],
            }));
        }
        if filter.is_none() || filter == Some("video") {
            models.push(json!({
                "id": DEFAULT_VIDEO_MODEL,
                "kind": "video",
                "default": default_video == DEFAULT_VIDEO_MODEL,
                "costCredits": DEFAULT_VIDEO_COST,
                "resolutions": ["720p", "1080p"],
                "aspects": ["16:9", "9:16", "1:1"],
                "audio": true,
            }));
            models.push(json!({
                "id": "veo-3.1",
                "kind": "video",
                "default": default_video == "veo-3.1",
                "costCredits": 60,
                "resolutions": ["720p", "1080p"],
                "aspects": ["16:9", "9:16"],
                "audio": true,
            }));
        }

        Ok(json!({
            "models": models,
            "defaultImageModel": default_image,
            "defaultVideoModel": default_video,
            "askAboveCredits": persisted.credits,
            "agentsCreditsAllowed": persisted.agents,
            "walletBalance": DEFAULT_WALLET_BALANCE,
        }))
    }
}

const DARK_THEME_VARS: &str = "color-scheme:dark;--background:#1a1a1a;--foreground:#ffffff;--muted-foreground:#979797;--muted:#212121;--card:#212121;--card-foreground:#ffffff;--border:rgba(255,255,255,0.045);--accent:#42a0ff;--destructive:#ff6069;--success:#31cf76;";
const LIGHT_THEME_VARS: &str = "color-scheme:light;--background:#f5f5f7;--foreground:#141414;--muted-foreground:rgba(20,20,20,0.64);--muted:#ffffff;--card:#ffffff;--card-foreground:#141414;--border:rgba(20,20,20,0.16);--accent:#5b52df;--destructive:#b02a2f;--success:#1e6f3b;";
const SHARED_THEME_VARS: &str = "--chart-1:#3987e5;--chart-2:#d95926;--chart-3:#199e70;--chart-4:#9085e9;--chart-5:#d6adf2;--chart-6:#6bc7d9;--radius:12px;--font-sans:\"Inter Variable\",-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;--font-mono:ui-monospace,SFMono-Regular,\"SF Mono\",Menlo,Monaco,Consolas,monospace;";
const BASE_STYLE: &str = "body{margin:0;color:var(--foreground);font:14px/1.5 var(--font-sans)}::-webkit-scrollbar{display:none}";

const OPENBOT_PAGE_BOOTSTRAP: &str = concat!(
    "(()=>{const p=window.parent,r=document.documentElement;let n=1,dm='inline';const pend=new Map();",
    "const send=(m)=>p.postMessage(Object.assign({jsonrpc:'2.0'},m),'*');",
    "const req=(method,params)=>new Promise((resolve,reject)=>{const id=n++;pend.set(id,{resolve,reject});send({id,method,params});});",
    "addEventListener('message',(e)=>{const d=e.data;if(e.source!==p||!d||d.jsonrpc!=='2.0')return;",
    "if(d.method==='ui/notifications/host-context-changed'){const pr=d.params||{};if(pr.displayMode==='inline'||pr.displayMode==='fullscreen')dm=pr.displayMode;return;}",
    "const w=d.method===undefined&&pend.get(d.id);if(!w)return;pend.delete(d.id);",
    "if(d.error)w.reject(new Error(String(d.error.message||'Refused')));else w.resolve(d.result);});",
    "const txt=(v)=>v===undefined||v===null||v===''?[]:[{type:'text',text:String(v)}];",
    "window.openbotPage=Object.freeze({",
    "sendMessage:(v)=>req('ui/message',{role:'user',content:txt(v)}),",
    "setContext:(v)=>req('ui/update-model-context',{content:txt(v)}),",
    "requestFullscreen:()=>req('ui/request-display-mode',{mode:'fullscreen'}),",
    "exitFullscreen:()=>req('ui/request-display-mode',{mode:'inline'}),",
    "get displayMode(){return dm;}});",
    "const rep=()=>send({method:'ui/notifications/size-changed',params:{height:Math.ceil(r.getBoundingClientRect().height)}});",
    "if(typeof ResizeObserver!=='undefined')new ResizeObserver(rep).observe(r);",
    "addEventListener('DOMContentLoaded',rep);addEventListener('load',rep);})();"
);

fn refused_rpc(id: Option<Value>, reason: &str) -> OpenBotPageRpcOutcome {
    let reply_json = id.as_ref().map(|req_id| {
        json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": { "code": -32000, "message": "Refused" }
        })
    });
    OpenBotPageRpcOutcome {
        rpc_type: "refused".to_string(),
        id,
        refused: true,
        payload: json!({ "reason": reason }),
        reply_json,
    }
}

fn extract_rpc_id(val: Option<&Value>) -> Option<Value> {
    match val? {
        Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Some(Value::Number(n.clone())),
        Value::String(s) if !s.is_empty() && utf16_len(s) <= INPUT_LIMITS.identifier => {
            Some(Value::String(s.clone()))
        }
        _ => None,
    }
}

fn extract_content_blocks(content: &Value, limit: usize) -> Option<String> {
    let arr = content.as_array()?;
    if arr.len() > CONTENT_BLOCK_LIMIT {
        return None;
    }
    let mut parts = Vec::with_capacity(arr.len());
    for block in arr {
        let obj = block.as_object()?;
        if obj.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        let text = obj.get("text").and_then(Value::as_str)?;
        if utf16_len(text) > limit {
            return None;
        }
        parts.push(text);
    }
    let joined = parts.join("\n").trim().to_string();
    (utf16_len(&joined) <= limit).then_some(joined)
}

fn extract_message_text(
    params: &Map<String, Value>,
    limit: usize,
    allow_empty: bool,
) -> Option<String> {
    let text = if let Some(content) = params.get("content") {
        extract_content_blocks(content, limit)?
    } else if let Some(raw) = params.get("text").and_then(Value::as_str) {
        let trimmed = raw.trim().to_string();
        if utf16_len(&trimmed) > limit {
            return None;
        }
        trimmed
    } else {
        return None;
    };
    if !allow_empty && text.is_empty() {
        return None;
    }
    Some(text)
}

fn extract_context_text(params: &Map<String, Value>, limit: usize) -> Option<String> {
    if let Some(content) = params.get("content") {
        return extract_content_blocks(content, limit);
    }
    let raw = params
        .get("context")
        .or_else(|| params.get("text"))
        .and_then(Value::as_str)?;
    let trimmed = raw.trim().to_string();
    (utf16_len(&trimmed) <= limit).then_some(trimmed)
}

fn is_allowed_image_src(src: &str) -> bool {
    if data_image_regex().is_match(src) {
        return true;
    }
    Url::parse(src)
        .map(|u| u.scheme() == "https")
        .unwrap_or(false)
}

fn validate_hex_color(value: &str, label: &str) -> RuntimeResult<String> {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err(OpenBotRuntimeError::Validation(format!(
            "Invalid {label} bubble color '{value}'; expected #RRGGBB hex color."
        )));
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn build_bubbles_meta(
    yours: Option<&str>,
    agents: Option<&str>,
) -> RuntimeResult<Option<String>> {
    let mut parts = Vec::new();
    if let Some(y) = yours.map(str::trim).filter(|s| !s.is_empty()) {
        let color = validate_hex_color(y, "yours")?;
        parts.push(format!("yours={color}"));
    }
    if let Some(a) = agents.map(str::trim).filter(|s| !s.is_empty()) {
        let color = validate_hex_color(a, "agents")?;
        parts.push(format!("agents={color}"));
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "<meta name=\"openbot-bubbles\" content=\"{}\">",
        parts.join("; ")
    )))
}

fn build_sandboxed_document(
    raw_html: &str,
    appearance: &str,
    title: Option<&str>,
    include_bootstrap: bool,
    bubbles_meta: Option<&str>,
) -> String {
    let stripped = if bubbles_meta.is_some() {
        bubbles_meta_regex().replace_all(raw_html, "").into_owned()
    } else {
        raw_html.to_string()
    };
    let theme_vars = if appearance == "light" {
        LIGHT_THEME_VARS
    } else {
        DARK_THEME_VARS
    };
    let mut head = String::new();
    head.push_str("<meta charset=\"utf-8\">");
    head.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    if let Some(t) = title.map(str::trim).filter(|s| !s.is_empty()) {
        head.push_str("<title>");
        head.push_str(&escape_html(t));
        head.push_str("</title>");
    }
    if let Some(meta) = bubbles_meta {
        head.push_str(meta);
    }
    head.push_str("<style id=\"openbot-visual-base\">:root{");
    head.push_str(theme_vars);
    head.push_str(SHARED_THEME_VARS);
    head.push_str("}");
    head.push_str(BASE_STYLE);
    head.push_str("</style>");
    if include_bootstrap {
        head.push_str("<script id=\"openbot-visual-bootstrap\">");
        head.push_str(OPENBOT_PAGE_BOOTSTRAP);
        head.push_str("</script>");
    }

    if let Some(m) = head_tag_regex().find(&stripped) {
        let mut out = String::with_capacity(stripped.len() + head.len() + 32);
        if !doctype_regex().is_match(&stripped) {
            out.push_str("<!doctype html>");
        }
        out.push_str(&stripped[..m.end()]);
        out.push_str(&head);
        out.push_str(&stripped[m.end()..]);
        out
    } else if let Some(m) = html_tag_regex().find(&stripped) {
        let mut out = String::with_capacity(stripped.len() + head.len() + 48);
        if !doctype_regex().is_match(&stripped) {
            out.push_str("<!doctype html>");
        }
        out.push_str(&stripped[..m.end()]);
        out.push_str("<head>");
        out.push_str(&head);
        out.push_str("</head>");
        out.push_str(&stripped[m.end()..]);
        out
    } else {
        format!("<!doctype html><html><head>{head}</head><body>{stripped}</body></html>")
    }
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn data_image_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^data:image/(?:png|jpeg|gif|webp|avif|svg\+xml)[;,]")
            .expect("valid data image regex")
    })
}

fn bubbles_meta_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta\b[^>]*\bname\s*=\s*["']openbot-bubbles["'][^>]*>"#)
            .expect("valid bubbles meta regex")
    })
}

fn doctype_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*<!doctype\b[^>]*>").expect("valid doctype regex"))
}

fn head_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<head\b[^>]*>").expect("valid head tag regex"))
}

fn html_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<html\b[^>]*>").expect("valid html tag regex"))
}
