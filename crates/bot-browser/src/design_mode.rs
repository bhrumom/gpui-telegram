//! Embedded Browser Design Mode element inspector, pure-Rust Source Map v3 VLQ
//! & framework metadata resolver, CDP DOM/BoxModel/CSS/Screenshot capture pipeline,
//! and Markdown chat-handoff formatter.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::Utc;
use image::{GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::backend::BrowserSessionBackend;
use crate::cdp::{
    CdpBridge, crop_element_png_if_needed, decode_screenshot_png, parse_eval_json,
};
use crate::types::{BotBrowserError, ElementRect, SemanticLocator};

pub use orca_core::orchestration_protocol::{
    DesignCaptureContract, DesignDiffMode, DesignModeError, DesignModeSpec, DesignOverlaySpec,
    DesignPin, DesignSourceMapRef, DesignViewport,
};

pub const DEFAULT_MAX_OUTER_HTML_BYTES: usize = 16_384;
pub const DEFAULT_MAX_ANCESTOR_DEPTH: usize = 12;

pub const CURATED_DESIGN_CSS_PROPERTIES: &[&str] = &[
    "display",
    "position",
    "top",
    "right",
    "bottom",
    "left",
    "z-index",
    "box-sizing",
    "width",
    "height",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "gap",
    "row-gap",
    "column-gap",
    "flex-direction",
    "flex-wrap",
    "justify-content",
    "align-items",
    "align-self",
    "grid-template-columns",
    "grid-template-rows",
    "color",
    "background-color",
    "background-image",
    "font-family",
    "font-size",
    "font-weight",
    "line-height",
    "letter-spacing",
    "text-align",
    "text-decoration",
    "border-width",
    "border-style",
    "border-color",
    "border-radius",
    "box-shadow",
    "opacity",
    "overflow",
    "cursor",
];

const ALWAYS_KEEP_LAYOUT_CSS: &[&str] = &[
    "display",
    "position",
    "width",
    "height",
    "color",
    "background-color",
    "font-family",
    "font-size",
    "font-weight",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignBoxEdges {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignBoxModel {
    pub content_width: f64,
    pub content_height: f64,
    pub margin: DesignBoxEdges,
    pub border: DesignBoxEdges,
    pub padding: DesignBoxEdges,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomAncestorNode {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub nth_of_type: u32,
    #[serde(default)]
    pub selector_segment: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMapFramework {
    ReactFiber,
    VueSfc,
    Svelte,
    ViteDataSource,
    WebpackSourceMap,
    VlqSourceMap,
    DataAttribute,
    Unknown,
}

impl SourceMapFramework {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReactFiber => "react_fiber",
            Self::VueSfc => "vue_sfc",
            Self::Svelte => "svelte",
            Self::ViteDataSource => "vite_data_source",
            Self::WebpackSourceMap => "webpack_source_map",
            Self::VlqSourceMap => "vlq_source_map",
            Self::DataAttribute => "data_attribute",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMapResolution {
    pub source_ref: DesignSourceMapRef,
    pub framework: SourceMapFramework,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_script_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_column: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSourceMapProbe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_debug_stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vue_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vue_component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svelte_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svelte_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svelte_column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_mapping_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_script_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignHoverTarget {
    pub selector: String,
    pub tag_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_attr: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    pub bounding_box: ElementRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_model: Option<DesignBoxModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_map: Option<DesignSourceMapRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignModeCaptureOptions {
    pub capture_screenshot: bool,
    pub crop_padding_px: u32,
    pub include_all_computed_styles: bool,
    pub max_outer_html_bytes: usize,
    pub max_ancestor_depth: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_source_maps: BTreeMap<String, String>,
}

impl Default for DesignModeCaptureOptions {
    fn default() -> Self {
        Self {
            capture_screenshot: true,
            crop_padding_px: 0,
            include_all_computed_styles: false,
            max_outer_html_bytes: DEFAULT_MAX_OUTER_HTML_BYTES,
            max_ancestor_depth: DEFAULT_MAX_ANCESTOR_DEPTH,
            external_source_maps: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignModeCapture {
    pub contract: DesignCaptureContract,
    pub selector: String,
    pub semantic_locator: SemanticLocator,
    pub tag_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_attr: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(default)]
    pub text_preview: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dom_hierarchy: Vec<DomAncestorNode>,
    pub bounding_box: ElementRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_model: Option<DesignBoxModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cropped_png_bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cropped_png_dimensions: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_map_hint: Option<SourceMapResolution>,
    #[serde(default)]
    pub redacted_sensitive_content: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignModeChatHandoff {
    pub contract: DesignCaptureContract,
    pub selector: String,
    pub dom_hierarchy_breadcrumb: String,
    pub outer_html_snippet: String,
    pub computed_styles: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_map: Option<DesignSourceMapRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cropped_png_bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<DesignPin>,
    pub prompt_markdown: String,
}

impl DesignModeCapture {
    pub fn to_contract(&self) -> DesignCaptureContract {
        self.contract.clone()
    }

    pub fn to_orca_capture_contract(&self) -> DesignCaptureContract {
        self.contract.clone()
    }

    pub fn into_mode_spec(
        &self,
        workspace_id: impl Into<String>,
        overlay: DesignOverlaySpec,
    ) -> DesignModeSpec {
        DesignModeSpec {
            workspace_id: workspace_id.into(),
            viewport: self.contract.viewport,
            overlay,
            last_capture: Some(self.contract.clone()),
        }
    }

    pub fn dom_breadcrumb(&self) -> String {
        if self.dom_hierarchy.is_empty() {
            return self.selector.clone();
        }
        self.dom_hierarchy
            .iter()
            .map(|node| {
                if !node.selector_segment.trim().is_empty() {
                    node.selector_segment.clone()
                } else {
                    let mut seg = node.tag.clone();
                    if let Some(ref id) = node.id {
                        seg.push('#');
                        seg.push_str(id);
                    }
                    for c in node.classes.iter().take(2) {
                        seg.push('.');
                        seg.push_str(c);
                    }
                    seg
                }
            })
            .collect::<Vec<_>>()
            .join(" > ")
    }

    pub fn cropped_png_base64(&self) -> Option<String> {
        self.cropped_png_bytes
            .as_ref()
            .map(|bytes| BASE64_STANDARD.encode(bytes))
    }

    pub fn normalized_center_bp(&self) -> (u16, u16) {
        let vw = self.contract.viewport.width_px.max(1) as f64;
        let vh = self.contract.viewport.height_px.max(1) as f64;
        let cx = self.bounding_box.x + (self.bounding_box.width / 2.0);
        let cy = self.bounding_box.y + (self.bounding_box.height / 2.0);
        let x_bp = ((cx / vw) * 10_000.0).round().clamp(0.0, 10_000.0) as u16;
        let y_bp = ((cy / vh) * 10_000.0).round().clamp(0.0, 10_000.0) as u16;
        (x_bp, y_bp)
    }

    pub fn to_chat_handoff(
        &self,
        user_note: Option<&str>,
        pins: &[DesignPin],
    ) -> DesignModeChatHandoff {
        let prompt_markdown = format_design_handoff_prompt(self, user_note, pins);
        DesignModeChatHandoff {
            contract: self.contract.clone(),
            selector: self.selector.clone(),
            dom_hierarchy_breadcrumb: self.dom_breadcrumb(),
            outer_html_snippet: self.contract.outer_html.clone(),
            computed_styles: self.contract.computed_styles.clone(),
            source_map: self.contract.source_map.clone(),
            cropped_png_bytes: self.cropped_png_bytes.clone(),
            screenshot_sha256: self.contract.screenshot_sha256.clone(),
            user_note: user_note
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            pins: pins.to_vec(),
            prompt_markdown,
        }
    }

    pub fn format_chat_attachment_markdown(&self) -> String {
        format_design_handoff_prompt(self, None, &[])
    }
}

/// Pure DOM payload input for deterministic headless `DesignModeCapture::from_dom_payload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignDomPayload {
    pub url: String,
    pub viewport: DesignViewport,
    pub selector: String,
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub id_attr: Option<String>,
    #[serde(default)]
    pub classes: Vec<String>,
    #[serde(default)]
    pub text_preview: String,
    #[serde(default)]
    pub outer_html: String,
    #[serde(default)]
    pub computed_styles: BTreeMap<String, String>,
    #[serde(default)]
    pub dom_hierarchy: Vec<DomAncestorNode>,
    pub bounding_box: ElementRect,
    #[serde(default)]
    pub box_model: Option<DesignBoxModel>,
    #[serde(default)]
    pub semantic_locator: Option<SemanticLocator>,
    #[serde(default)]
    pub source_map_probe: Option<RawSourceMapProbe>,
    #[serde(default)]
    pub cropped_png_bytes: Option<Vec<u8>>,
    #[serde(default)]
    pub captured_at_ms: Option<i64>,
}

impl DesignModeCapture {
    /// Deterministically constructs and validates a [`DesignModeCapture`] from a DOM payload
    /// without performing any network or CDP I/O.
    pub fn from_dom_payload(
        payload: DesignDomPayload,
        options: &DesignModeCaptureOptions,
    ) -> Result<Self, DesignModeError> {
        if payload.url.trim().is_empty() {
            return Err(DesignModeError::EmptyCaptureUrl);
        }
        if payload.viewport.width_px == 0
            || payload.viewport.height_px == 0
            || payload.viewport.device_scale_factor_x100 == 0
        {
            return Err(DesignModeError::InvalidViewport);
        }

        let (outer_html, redacted_sensitive_content) =
            sanitize_outer_html(&payload.outer_html, options.max_outer_html_bytes);
        let computed_styles = filter_computed_styles_map(
            &payload.computed_styles,
            options.include_all_computed_styles,
        );
        let source_map_hint = payload
            .source_map_probe
            .as_ref()
            .and_then(|p| resolve_source_map_metadata(p, &options.external_source_maps));
        let source_map = source_map_hint.as_ref().map(|r| r.source_ref.clone());

        let (cropped_png_bytes, screenshot_sha256, cropped_png_dimensions) =
            match payload.cropped_png_bytes {
                Some(bytes) if !bytes.is_empty() => match compute_png_sha256_and_dims(&bytes) {
                    Ok((sha, dims)) => (Some(bytes), Some(sha), Some(dims)),
                    Err(_) => {
                        let sha = hex::encode(Sha256::digest(&bytes));
                        (Some(bytes), Some(sha), None)
                    }
                },
                _ => (None, None, None),
            };

        let captured_at_ms = payload
            .captured_at_ms
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let contract = DesignCaptureContract {
            url: payload.url.trim().to_string(),
            viewport: payload.viewport,
            outer_html,
            computed_styles,
            screenshot_sha256,
            source_map,
            captured_at_ms,
        };

        let tag_name = if payload.tag_name.trim().is_empty() {
            "div".to_string()
        } else {
            payload.tag_name.trim().to_ascii_lowercase()
        };
        let selector = if payload.selector.trim().is_empty() {
            tag_name.clone()
        } else {
            payload.selector.trim().to_string()
        };
        let semantic_locator = payload
            .semantic_locator
            .unwrap_or_else(|| SemanticLocator::from_css(selector.clone()));

        let mut dom_hierarchy = payload.dom_hierarchy;
        if dom_hierarchy.len() > options.max_ancestor_depth && options.max_ancestor_depth > 0 {
            let start = dom_hierarchy.len() - options.max_ancestor_depth;
            dom_hierarchy = dom_hierarchy.split_off(start);
        }

        Ok(Self {
            contract,
            selector,
            semantic_locator,
            tag_name,
            id_attr: payload.id_attr,
            classes: payload.classes,
            text_preview: payload.text_preview,
            dom_hierarchy,
            bounding_box: payload.bounding_box,
            box_model: payload.box_model,
            cropped_png_bytes,
            cropped_png_dimensions,
            source_map_hint,
            redacted_sensitive_content,
        })
    }
}

// ---------------------------------------------------------------------------
// Pure-Rust Source Map v3 (Base64-VLQ) & Framework Source Resolver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMapV3 {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(
        default,
        rename = "sourceRoot",
        alias = "source_root",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_root: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub mappings: String,
}

fn base64_vlq_char_value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decodes a single Base64-VLQ segment (comma-delimited element in Source Map v3 `mappings`).
pub fn decode_vlq_segment(segment: &str) -> Result<Vec<i64>, BotBrowserError> {
    let mut values = Vec::new();
    let bytes = segment.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            if idx >= bytes.len() {
                return Err(BotBrowserError::Cdp(
                    "truncated Base64-VLQ continuation sequence".to_string(),
                ));
            }
            let digit = base64_vlq_char_value(bytes[idx]).ok_or_else(|| {
                BotBrowserError::Cdp(format!(
                    "invalid Base64-VLQ character '{}'",
                    bytes[idx] as char
                ))
            })?;
            idx += 1;
            let continuation = (digit & 0x20) != 0;
            let payload = u64::from(digit & 0x1F);
            if shift >= 60 {
                return Err(BotBrowserError::Cdp("Base64-VLQ integer overflow".to_string()));
            }
            result |= payload << shift;
            shift += 5;
            if !continuation {
                break;
            }
        }
        let is_negative = (result & 1) == 1;
        let magnitude = (result >> 1) as i64;
        values.push(if is_negative { -magnitude } else { magnitude });
    }
    Ok(values)
}

pub fn parse_source_map_v3(raw_json: &str) -> Result<SourceMapV3, BotBrowserError> {
    let map: SourceMapV3 = serde_json::from_str(raw_json)
        .map_err(|e| BotBrowserError::Cdp(format!("invalid SourceMap v3 JSON: {e}")))?;
    if map.version != 3 {
        return Err(BotBrowserError::Cdp(format!(
            "unsupported source map version {} (expected 3)",
            map.version
        )));
    }
    Ok(map)
}

pub fn parse_inline_source_map_url(url: &str) -> Result<Option<SourceMapV3>, BotBrowserError> {
    let trimmed = url.trim();
    if !trimmed.starts_with("data:") {
        return Ok(None);
    }
    let Some((header, payload)) = trimmed.split_once(',') else {
        return Ok(None);
    };
    let lower_header = header.to_ascii_lowercase();
    if !lower_header.contains("application/json") {
        return Ok(None);
    }
    let raw_json = if lower_header.ends_with(";base64") {
        let decoded = BASE64_STANDARD
            .decode(payload.trim())
            .map_err(|e| BotBrowserError::Cdp(format!("invalid inline source map base64: {e}")))?;
        String::from_utf8(decoded)
            .map_err(|e| BotBrowserError::Cdp(format!("invalid inline source map utf-8: {e}")))?
    } else {
        percent_decode_utf8(payload)
    };
    parse_source_map_v3(&raw_json).map(Some)
}

fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex_pair = &input[i + 1..i + 3];
            if let Ok(val) = u8::from_str_radix(hex_pair, 16) {
                out.push(val);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn extract_source_mapping_url(asset_text: &str) -> Option<String> {
    for line in asset_text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("//# sourceMappingURL=")
            .or_else(|| trimmed.strip_prefix("//@ sourceMappingURL="))
        {
            let url = rest.trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
        if let Some(rest) = trimmed
            .strip_prefix("/*# sourceMappingURL=")
            .or_else(|| trimmed.strip_prefix("/*@ sourceMappingURL="))
        {
            let url = rest.trim_end_matches("*/").trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn normalize_source_path(source_root: Option<&str>, raw_source: &str) -> String {
    let joined = match source_root.map(str::trim).filter(|s| !s.is_empty()) {
        Some(root)
            if !raw_source.starts_with('/')
                && !raw_source.contains("://")
                && !raw_source.starts_with("webpack:") =>
        {
            format!("{}/{}", root.trim_end_matches('/'), raw_source.trim_start_matches('/'))
        }
        _ => raw_source.to_string(),
    };
    let mut s = joined.trim().to_string();
    for prefix in [
        "webpack:///./",
        "webpack:///",
        "webpack://",
        "file:///",
        "file://",
        "vite-plugin://",
    ] {
        if let Some(stripped) = s.strip_prefix(prefix) {
            s = stripped.to_string();
            break;
        }
    }
    if let Some((path, _query)) = s.split_once('?') {
        s = path.to_string();
    }
    if let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

/// Resolves a 1-based generated line & 0-based generated column against a parsed Source Map v3.
pub fn resolve_generated_position(
    map: &SourceMapV3,
    generated_line_1based: u32,
    generated_col_0based: u32,
) -> Option<DesignSourceMapRef> {
    if generated_line_1based == 0 || map.mappings.is_empty() {
        return None;
    }
    let target_line_idx = (generated_line_1based - 1) as usize;
    let target_col = i64::from(generated_col_0based);

    let mut source_idx: i64 = 0;
    let mut orig_line: i64 = 0;
    let mut orig_col: i64 = 0;
    let mut name_idx: i64 = 0;

    let mut best: Option<(i64, usize, i64, i64, Option<usize>)> = None;

    for (line_idx, line_str) in map.mappings.split(';').enumerate() {
        if line_idx > target_line_idx {
            break;
        }
        let mut gen_col: i64 = 0;
        if line_str.is_empty() {
            continue;
        }
        for seg in line_str.split(',') {
            if seg.is_empty() {
                continue;
            }
            let Ok(fields) = decode_vlq_segment(seg) else {
                return None;
            };
            if fields.is_empty() {
                continue;
            }
            gen_col += fields[0];
            if fields.len() >= 4 {
                source_idx += fields[1];
                orig_line += fields[2];
                orig_col += fields[3];
                let current_name = if fields.len() >= 5 {
                    name_idx += fields[4];
                    usize::try_from(name_idx).ok()
                } else {
                    None
                };
                if line_idx == target_line_idx && gen_col <= target_col {
                    if let Ok(s_idx) = usize::try_from(source_idx) {
                        best = Some((gen_col, s_idx, orig_line, orig_col, current_name));
                    }
                }
            }
        }
    }

    let (_, s_idx, o_line, o_col, n_idx) = best?;
    let raw_src = map.sources.get(s_idx)?;
    let file_path = normalize_source_path(map.source_root.as_deref(), raw_src);
    if file_path.is_empty() {
        return None;
    }
    let component_name = n_idx
        .and_then(|idx| map.names.get(idx))
        .cloned()
        .filter(|s| !s.trim().is_empty());
    let line = u32::try_from(o_line.saturating_add(1).max(1)).ok()?;
    let column = u32::try_from(o_col.saturating_add(1).max(1)).ok();
    Some(DesignSourceMapRef {
        file_path,
        line,
        column,
        component_name,
    })
}

/// Parses framework source attributes and stack frames such as:
/// - `src/Button.tsx:42:8`
/// - `webpack:///./src/App.vue:18`
/// - `src/Card.svelte:10:4`
/// - `at PrimaryButton (http://localhost:5173/src/Button.tsx:42:8)`
pub fn parse_framework_source_attr(raw: &str) -> Option<DesignSourceMapRef> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Handle multi-line React 19 `_debugStack` frames
    if trimmed.contains('\n') || trimmed.starts_with("at ") || trimmed.contains("    at ") {
        for line in trimmed.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("at ") {
                let (component_name, loc_str) = if let Some(paren_start) = rest.find('(') {
                    let comp = rest[..paren_start].trim();
                    let inner = rest[paren_start + 1..]
                        .trim_end_matches(')')
                        .trim();
                    (
                        (!comp.is_empty() && comp != "Object.anonymous").then(|| comp.to_string()),
                        inner,
                    )
                } else {
                    (None, rest)
                };
                if let Some(mut parsed) = parse_single_source_loc(loc_str) {
                    if parsed.component_name.is_none() {
                        parsed.component_name = component_name;
                    }
                    return Some(parsed);
                }
            }
        }
    }

    parse_single_source_loc(trimmed)
}

fn parse_single_source_loc(loc: &str) -> Option<DesignSourceMapRef> {
    let clean = loc.trim().trim_matches(|c| c == '(' || c == ')' || c == '"');
    if clean.is_empty() {
        return None;
    }
    let mut parts = clean.rsplitn(3, ':');
    let p_last = parts.next()?;
    let p_mid = parts.next();
    let p_head = parts.next();

    let (raw_path, line, column) = match (p_head, p_mid) {
        (Some(head), Some(mid))
            if p_last.chars().all(|c| c.is_ascii_digit())
                && !p_last.is_empty()
                && mid.chars().all(|c| c.is_ascii_digit())
                && !mid.is_empty() =>
        {
            let ln = mid.parse::<u32>().ok().filter(|&n| n > 0)?;
            let col = p_last.parse::<u32>().ok();
            (head, ln, col)
        }
        (head_opt, Some(mid))
            if p_last.chars().all(|c| c.is_ascii_digit()) && !p_last.is_empty() =>
        {
            let ln = p_last.parse::<u32>().ok().filter(|&n| n > 0)?;
            let path_joined = match head_opt {
                Some(h) => format!("{h}:{mid}"),
                None => mid.to_string(),
            };
            let normalized = strip_http_origin_if_present(&path_joined);
            return Some(DesignSourceMapRef {
                file_path: normalize_source_path(None, &normalized),
                line: ln,
                column: None,
                component_name: None,
            });
        }
        _ => return None,
    };

    let stripped = strip_http_origin_if_present(raw_path);
    let file_path = normalize_source_path(None, &stripped);
    if file_path.is_empty() {
        return None;
    }
    Some(DesignSourceMapRef {
        file_path,
        line,
        column,
        component_name: None,
    })
}

fn strip_http_origin_if_present(raw: &str) -> String {
    if let Ok(parsed) = url::Url::parse(raw) {
        if matches!(parsed.scheme(), "http" | "https") {
            let p = parsed.path().trim_start_matches('/');
            if !p.is_empty() {
                return p.to_string();
            }
        }
    }
    raw.to_string()
}

/// Resolves source-map metadata in priority order:
/// 1. React Fiber `_debugSource` / `_debugStack`
/// 2. Vue SFC `__vueParentComponent.type.__file`
/// 3. Svelte `__svelte_meta.loc`
/// 4. Explicit `data-source` / `data-source-loc` / `data-v-inspector` / `data-component-file`
/// 5. Inline `data:application/json;base64,...` or external `*.map` Base64-VLQ lookup
pub fn resolve_source_map_metadata(
    raw: &RawSourceMapProbe,
    external_maps: &BTreeMap<String, String>,
) -> Option<SourceMapResolution> {
    // 1. React Fiber `_debugSource`
    if let Some(ref file) = raw.react_file
        && !file.trim().is_empty()
    {
        return Some(SourceMapResolution {
            source_ref: DesignSourceMapRef {
                file_path: normalize_source_path(None, &strip_http_origin_if_present(file)),
                line: raw.react_line.unwrap_or(1).max(1),
                column: raw.react_column,
                component_name: raw.react_component.clone(),
            },
            framework: SourceMapFramework::ReactFiber,
            raw_script_url: raw.script_url.clone(),
            generated_line: raw.generated_line,
            generated_column: raw.generated_column,
        });
    }
    // 1b. React 19 `_debugStack`
    if let Some(ref stack) = raw.react_debug_stack
        && let Some(mut sref) = parse_framework_source_attr(stack)
    {
        if sref.component_name.is_none() {
            sref.component_name = raw.react_component.clone();
        }
        return Some(SourceMapResolution {
            source_ref: sref,
            framework: SourceMapFramework::ReactFiber,
            raw_script_url: raw.script_url.clone(),
            generated_line: raw.generated_line,
            generated_column: raw.generated_column,
        });
    }
    // 2. Vue SFC
    if let Some(ref file) = raw.vue_file
        && !file.trim().is_empty()
    {
        let sref = parse_framework_source_attr(file).unwrap_or(DesignSourceMapRef {
            file_path: normalize_source_path(None, &strip_http_origin_if_present(file)),
            line: 1,
            column: None,
            component_name: raw.vue_component.clone(),
        });
        return Some(SourceMapResolution {
            source_ref: DesignSourceMapRef {
                component_name: sref.component_name.or_else(|| raw.vue_component.clone()),
                ..sref
            },
            framework: SourceMapFramework::VueSfc,
            raw_script_url: raw.script_url.clone(),
            generated_line: raw.generated_line,
            generated_column: raw.generated_column,
        });
    }
    // 3. Svelte `__svelte_meta`
    if let Some(ref file) = raw.svelte_file
        && !file.trim().is_empty()
    {
        return Some(SourceMapResolution {
            source_ref: DesignSourceMapRef {
                file_path: normalize_source_path(None, &strip_http_origin_if_present(file)),
                line: raw.svelte_line.unwrap_or(1).max(1),
                column: raw.svelte_column,
                component_name: raw.data_component.clone(),
            },
            framework: SourceMapFramework::Svelte,
            raw_script_url: raw.script_url.clone(),
            generated_line: raw.generated_line,
            generated_column: raw.generated_column,
        });
    }
    // 4. Explicit `data-source*` / `data-v-inspector`
    if let Some(ref attr) = raw.data_source
        && let Some(mut sref) = parse_framework_source_attr(attr)
    {
        if sref.component_name.is_none() {
            sref.component_name = raw.data_component.clone();
        }
        let framework = if attr.contains("webpack://") {
            SourceMapFramework::WebpackSourceMap
        } else if attr.contains(".tsx") || attr.contains(".vue") || attr.contains(".svelte") {
            SourceMapFramework::ViteDataSource
        } else {
            SourceMapFramework::DataAttribute
        };
        return Some(SourceMapResolution {
            source_ref: sref,
            framework,
            raw_script_url: raw.script_url.clone(),
            generated_line: raw.generated_line,
            generated_column: raw.generated_column,
        });
    }
    // 5. Inline or external Source Map v3 VLQ resolution
    let gen_line = raw.generated_line.unwrap_or(1);
    let gen_col = raw.generated_column.unwrap_or(0);
    let mapping_url = raw.source_mapping_url.clone().or_else(|| {
        raw.inline_script_text
            .as_deref()
            .and_then(extract_source_mapping_url)
    });
    if let Some(ref url) = mapping_url {
        if let Ok(Some(map)) = parse_inline_source_map_url(url)
            && let Some(sref) = resolve_generated_position(&map, gen_line, gen_col)
        {
            return Some(SourceMapResolution {
                source_ref: sref,
                framework: SourceMapFramework::VlqSourceMap,
                raw_script_url: raw.script_url.clone(),
                generated_line: Some(gen_line),
                generated_column: Some(gen_col),
            });
        }
        if let Some(raw_map_json) = external_maps.get(url)
            && let Ok(map) = parse_source_map_v3(raw_map_json)
            && let Some(sref) = resolve_generated_position(&map, gen_line, gen_col)
        {
            return Some(SourceMapResolution {
                source_ref: sref,
                framework: SourceMapFramework::VlqSourceMap,
                raw_script_url: raw.script_url.clone(),
                generated_line: Some(gen_line),
                generated_column: Some(gen_col),
            });
        }
    }
    if let Some(ref script_url) = raw.script_url
        && let Some(raw_map_json) = external_maps
            .get(script_url)
            .or_else(|| external_maps.get(&format!("{script_url}.map")))
        && let Ok(map) = parse_source_map_v3(raw_map_json)
        && let Some(sref) = resolve_generated_position(&map, gen_line, gen_col)
    {
        let framework = if map.sources.iter().any(|s| s.starts_with("webpack://")) {
            SourceMapFramework::WebpackSourceMap
        } else {
            SourceMapFramework::VlqSourceMap
        };
        return Some(SourceMapResolution {
            source_ref: sref,
            framework,
            raw_script_url: Some(script_url.clone()),
            generated_line: Some(gen_line),
            generated_column: Some(gen_col),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// CDP Box Model, Computed Styles, HTML Sanitizer, PNG Crop & Prompt Formatter
// ---------------------------------------------------------------------------

fn quad_bounds(quad: &[f64]) -> Option<(f64, f64, f64, f64)> {
    if quad.len() < 8 {
        return None;
    }
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    let min_x = xs.into_iter().fold(f64::INFINITY, f64::min);
    let max_x = xs.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.into_iter().fold(f64::INFINITY, f64::min);
    let max_y = ys.into_iter().fold(f64::NEG_INFINITY, f64::max);
    if !min_x.is_finite() || !max_x.is_finite() || !min_y.is_finite() || !max_y.is_finite() {
        return None;
    }
    Some((min_x, min_y, max_x, max_y))
}

fn read_quad(model: &Value, key: &str) -> Option<(f64, f64, f64, f64)> {
    let arr = model.get(key)?.as_array()?;
    let nums: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
    quad_bounds(&nums)
}

fn edge_delta(outer: (f64, f64, f64, f64), inner: (f64, f64, f64, f64)) -> DesignBoxEdges {
    DesignBoxEdges {
        top: (inner.1 - outer.1).max(0.0),
        right: (outer.2 - inner.2).max(0.0),
        bottom: (outer.3 - inner.3).max(0.0),
        left: (inner.0 - outer.0).max(0.0),
    }
}

/// Parses a CDP `DOM.getBoxModel` response (`{ model: { content, padding, border, margin } }`)
/// into `(ElementRect, DesignBoxModel)` using the border-box for `ElementRect`.
pub fn parse_cdp_box_model(cdp_result: &Value, dpr: f64) -> Option<(ElementRect, DesignBoxModel)> {
    let model = cdp_result.get("model").unwrap_or(cdp_result);
    let content = read_quad(model, "content")?;
    let padding = read_quad(model, "padding").unwrap_or(content);
    let border = read_quad(model, "border").unwrap_or(padding);
    let margin = read_quad(model, "margin").unwrap_or(border);
    let safe_dpr = if dpr.is_finite() && dpr > 0.0 { dpr } else { 1.0 };

    let rect = ElementRect {
        x: border.0,
        y: border.1,
        width: (border.2 - border.0).max(0.0),
        height: (border.3 - border.1).max(0.0),
        device_pixel_ratio: safe_dpr,
    };
    let box_model = DesignBoxModel {
        content_width: (content.2 - content.0).max(0.0),
        content_height: (content.3 - content.1).max(0.0),
        padding: edge_delta(padding, content),
        border: edge_delta(border, padding),
        margin: edge_delta(margin, border),
    };
    Some((rect, box_model))
}

fn is_noisy_default_css_value(name: &str, value: &str) -> bool {
    if ALWAYS_KEEP_LAYOUT_CSS.contains(&name) {
        return false;
    }
    matches!(
        value.trim(),
        "" | "none" | "normal" | "auto" | "0px" | "0" | "rgba(0, 0, 0, 0)" | "transparent"
    )
}

pub fn filter_computed_styles_from_cdp(
    cdp_result: &Value,
    include_all: bool,
) -> BTreeMap<String, String> {
    let mut raw = BTreeMap::new();
    if let Some(arr) = cdp_result
        .get("computedStyle")
        .and_then(Value::as_array)
    {
        for entry in arr {
            if let (Some(name), Some(val)) = (
                entry.get("name").and_then(Value::as_str),
                entry.get("value").and_then(Value::as_str),
            ) {
                raw.insert(name.to_ascii_lowercase(), val.trim().to_string());
            }
        }
    }
    filter_computed_styles_map(&raw, include_all)
}

pub fn filter_computed_styles_map(
    raw: &BTreeMap<String, String>,
    include_all: bool,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        if key.is_empty() || val.is_empty() {
            continue;
        }
        if !include_all {
            if !CURATED_DESIGN_CSS_PROPERTIES.contains(&key.as_str()) {
                continue;
            }
            if is_noisy_default_css_value(&key, val) {
                continue;
            }
        }
        out.insert(key, val.to_string());
    }
    out
}

fn is_sensitive_html_tag(tag_lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "type=\"password\"",
        "type='password'",
        "password",
        "passwd",
        "passcode",
        "one-time-code",
        "totp",
        "secret",
        "api_key",
        "apikey",
        "auth_token",
        "access_token",
        "token",
        "otp",
    ];
    (tag_lower.starts_with("<input") || tag_lower.starts_with("<textarea"))
        && MARKERS.iter().any(|m| tag_lower.contains(m))
}

fn redact_value_attr_in_tag(tag: &str) -> (String, bool) {
    let lower = tag.to_ascii_lowercase();
    if !is_sensitive_html_tag(&lower) {
        return (tag.to_string(), false);
    }
    let mut out = String::with_capacity(tag.len());
    let mut cursor = 0usize;
    let mut redacted = false;
    while let Some(pos) = lower[cursor..].find("value=") {
        let abs = cursor + pos;
        let boundary_ok = abs == 0
            || tag.as_bytes()[abs - 1].is_ascii_whitespace();
        if !boundary_ok {
            out.push_str(&tag[cursor..abs + 6]);
            cursor = abs + 6;
            continue;
        }
        out.push_str(&tag[cursor..abs]);
        let after_eq = abs + 6;
        let rest = &tag[after_eq..];
        let end_rel = if let Some(q) = rest.chars().next().filter(|&c| c == '"' || c == '\'') {
            rest[1..]
                .find(q)
                .map(|i| i + 2)
                .unwrap_or(rest.len())
        } else {
            rest.find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                .unwrap_or(rest.len())
        };
        out.push_str("value=\"[REDACTED]\"");
        redacted = true;
        cursor = after_eq + end_rel;
    }
    out.push_str(&tag[cursor..]);
    (out, redacted)
}

/// Scrubs `value="..."` on sensitive password/token/secret/OTP inputs and truncates
/// cleanly on a UTF-8 character boundary to at most `max_bytes`.
pub fn sanitize_outer_html(raw_html: &str, max_bytes: usize) -> (String, bool) {
    let mut sanitized = String::with_capacity(raw_html.len());
    let mut redacted = false;
    let mut cursor = 0usize;
    while let Some(rel_lt) = raw_html[cursor..].find('<') {
        let lt = cursor + rel_lt;
        sanitized.push_str(&raw_html[cursor..lt]);
        if let Some(rel_gt) = raw_html[lt..].find('>') {
            let gt = lt + rel_gt + 1;
            let (clean_tag, did_redact) = redact_value_attr_in_tag(&raw_html[lt..gt]);
            redacted |= did_redact;
            sanitized.push_str(&clean_tag);
            cursor = gt;
        } else {
            sanitized.push_str(&raw_html[lt..]);
            cursor = raw_html.len();
            break;
        }
    }
    if cursor < raw_html.len() {
        sanitized.push_str(&raw_html[cursor..]);
    }
    if max_bytes > 0 && sanitized.len() > max_bytes {
        let suffix = "\n<!-- truncated -->";
        let budget = max_bytes.saturating_sub(suffix.len());
        let mut cut = budget.min(sanitized.len());
        while cut > 0 && !sanitized.is_char_boundary(cut) {
            cut -= 1;
        }
        sanitized.truncate(cut);
        if max_bytes >= suffix.len() {
            sanitized.push_str(suffix);
        }
    }
    (sanitized, redacted)
}

/// Computes lowercase 64-char hex SHA-256 and PNG pixel dimensions `(width, height)`.
pub fn compute_png_sha256_and_dims(
    png_bytes: &[u8],
) -> Result<(String, (u32, u32)), BotBrowserError> {
    let img = image::load_from_memory_with_format(png_bytes, ImageFormat::Png)
        .map_err(|e| BotBrowserError::Cdp(format!("invalid PNG bytes: {e}")))?;
    let dims = img.dimensions();
    let sha = hex::encode(Sha256::digest(png_bytes));
    Ok((sha, dims))
}

/// Expands `rect` by `padding_px` clamped to `(viewport_w, viewport_h)` and delegates
/// DPR-aware cropping to `crate::cdp::crop_element_png_if_needed`.
pub fn crop_png_with_padding(
    raw_png: Vec<u8>,
    rect: &ElementRect,
    viewport_w: f64,
    viewport_h: f64,
    padding_px: u32,
) -> Result<Vec<u8>, BotBrowserError> {
    let pad = f64::from(padding_px);
    let max_w = if viewport_w > 0.0 {
        viewport_w
    } else {
        (rect.x + rect.width + pad).max(1.0)
    };
    let max_h = if viewport_h > 0.0 {
        viewport_h
    } else {
        (rect.y + rect.height + pad).max(1.0)
    };
    let x = (rect.x - pad).max(0.0);
    let y = (rect.y - pad).max(0.0);
    let right = (rect.x + rect.width + pad).min(max_w);
    let bottom = (rect.y + rect.height + pad).min(max_h);
    let expanded = ElementRect {
        x,
        y,
        width: (right - x).max(1.0),
        height: (bottom - y).max(1.0),
        device_pixel_ratio: rect.device_pixel_ratio,
    };
    crop_element_png_if_needed(raw_png, &expanded, viewport_w, viewport_h)
}

/// Formats a deterministic Markdown handoff prompt containing URL, viewport, selector,
/// DOM breadcrumb, source-map `file:line:col`, user note/pins, curated CSS, sanitized HTML,
/// and cropped screenshot SHA-256.
pub fn format_design_handoff_prompt(
    capture: &DesignModeCapture,
    user_note: Option<&str>,
    pins: &[DesignPin],
) -> String {
    let mut out = String::new();
    out.push_str("## Browser Design Mode Capture\n\n");
    out.push_str(&format!("- **URL**: `{}`\n", capture.contract.url));
    out.push_str(&format!(
        "- **Viewport**: `{}x{}` (`{:.2}x` DPR{})\n",
        capture.contract.viewport.width_px,
        capture.contract.viewport.height_px,
        f64::from(capture.contract.viewport.device_scale_factor_x100) / 100.0,
        if capture.contract.viewport.is_mobile {
            ", mobile"
        } else {
            ""
        }
    ));
    out.push_str(&format!("- **Selector**: `{}`\n", capture.selector));
    out.push_str(&format!("- **DOM Path**: `{}`\n", capture.dom_breadcrumb()));
    out.push_str(&format!(
        "- **Bounds**: `x={:.1}, y={:.1}, w={:.1}, h={:.1}`\n",
        capture.bounding_box.x,
        capture.bounding_box.y,
        capture.bounding_box.width,
        capture.bounding_box.height
    ));
    if let Some(ref sm) = capture.contract.source_map {
        let mut loc = format!("{}:{}", sm.file_path, sm.line);
        if let Some(col) = sm.column {
            loc.push_str(&format!(":{col}"));
        }
        let comp = sm
            .component_name
            .as_deref()
            .map(|c| format!(" (`{c}`)"))
            .unwrap_or_default();
        let fw = capture
            .source_map_hint
            .as_ref()
            .map(|h| format!(" [{}]", h.framework.as_str()))
            .unwrap_or_default();
        out.push_str(&format!("- **Source Map**: `{loc}`{comp}{fw}\n"));
    }
    if let Some(ref sha) = capture.contract.screenshot_sha256 {
        let dims_note = capture
            .cropped_png_dimensions
            .map(|(w, h)| format!(" ({w}x{h}px)"))
            .unwrap_or_default();
        out.push_str(&format!("- **Screenshot SHA-256**: `{sha}`{dims_note}\n"));
    }
    if capture.redacted_sensitive_content {
        out.push_str("- **Privacy**: Sensitive input values were redacted.\n");
    }

    if let Some(note) = user_note.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("\n### Design Note\n");
        out.push_str(note);
        out.push('\n');
    }

    if !pins.is_empty() {
        out.push_str("\n### Pinned Annotations\n");
        for (i, pin) in pins.iter().enumerate() {
            let status = if pin.resolved { "resolved" } else { "open" };
            out.push_str(&format!(
                "{}. `[{}]` `{}` @ ({:.1}%, {:.1}%): {}\n",
                i + 1,
                status,
                pin.selector,
                f64::from(pin.x_norm_bp) / 100.0,
                f64::from(pin.y_norm_bp) / 100.0,
                pin.note.trim()
            ));
        }
    }

    if !capture.contract.computed_styles.is_empty() {
        out.push_str("\n### Computed CSS\n```css\n");
        for (k, v) in &capture.contract.computed_styles {
            out.push_str(&format!("{k}: {v};\n"));
        }
        out.push_str("```\n");
    }

    if !capture.contract.outer_html.is_empty() {
        out.push_str("\n### Outer HTML\n```html\n");
        out.push_str(&capture.contract.outer_html);
        if !capture.contract.outer_html.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
    }
    out
}

#[derive(Debug, Deserialize)]
struct JsProbePayload {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    url: String,
    #[serde(default)]
    viewport_width: u32,
    #[serde(default)]
    viewport_height: u32,
    #[serde(default)]
    dpr: f64,
    #[serde(default)]
    is_mobile: bool,
    #[serde(default)]
    selector: String,
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    id_attr: Option<String>,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    text_preview: String,
    #[serde(default)]
    outer_html: String,
    #[serde(default)]
    computed_styles: BTreeMap<String, String>,
    #[serde(default)]
    dom_hierarchy: Vec<DomAncestorNode>,
    #[serde(default)]
    rect: Option<ElementRect>,
    #[serde(default)]
    semantic_locator: Option<SemanticLocator>,
    #[serde(default)]
    source_map_probe: Option<RawSourceMapProbe>,
}

fn build_dom_inspect_script(
    target_js_expr: &str,
    max_ancestor_depth: usize,
    include_full_capture: bool,
) -> String {
    let curated_json = serde_json::to_string(CURATED_DESIGN_CSS_PROPERTIES)
        .unwrap_or_else(|_| "[]".to_string());
    let depth = max_ancestor_depth.max(1);
    let full_flag = if include_full_capture { "true" } else { "false" };
    format!(
        r#"(() => {{
  const el = {target_js_expr};
  if (!el || el.nodeType !== 1) return JSON.stringify({{ found: false }});
  const dpr = Number(window.devicePixelRatio || 1);
  const r = el.getBoundingClientRect();
  const rect = {{ x: r.left, y: r.top, width: r.width, height: r.height, device_pixel_ratio: dpr }};
  const tag = (el.tagName || 'div').toLowerCase();
  const idAttr = el.id ? String(el.id) : null;
  const classes = el.classList ? Array.from(el.classList).slice(0, 16) : [];
  const cssEsc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : String(s).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
  const segFor = (node) => {{
    const t = (node.tagName || 'div').toLowerCase();
    if (node.id) return '#' + cssEsc(node.id);
    const tid = node.getAttribute && node.getAttribute('data-testid');
    if (tid) return t + '[data-testid="' + String(tid).replace(/"/g, '\\"') + '"]';
    let s = t;
    const cls = node.classList ? Array.from(node.classList).slice(0, 2) : [];
    if (cls.length) s += '.' + cls.map(cssEsc).join('.');
    const parent = node.parentElement;
    if (parent) {{
      const same = Array.from(parent.children).filter(c => c.tagName === node.tagName);
      if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
    }}
    return s;
  }};
  const hierarchy = [];
  let cur = el;
  while (cur && cur.nodeType === 1 && hierarchy.length < {depth}) {{
    const parent = cur.parentElement;
    let nth = 1;
    if (parent) {{
      const same = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
      nth = Math.max(1, same.indexOf(cur) + 1);
    }}
    hierarchy.unshift({{
      tag: (cur.tagName || 'div').toLowerCase(),
      id: cur.id ? String(cur.id) : null,
      classes: cur.classList ? Array.from(cur.classList).slice(0, 8) : [],
      role: cur.getAttribute ? cur.getAttribute('role') : null,
      nth_of_type: nth,
      selector_segment: segFor(cur)
    }});
    if (cur.id) break;
    cur = parent;
  }}
  const selector = hierarchy.map(h => h.selector_segment).join(' > ') || segFor(el);
  const probe = {{}};
  try {{
    const keys = Object.keys(el);
    const fiberKey = keys.find(k => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$'));
    if (fiberKey) {{
      let f = el[fiberKey];
      let hops = 0;
      while (f && hops < 16) {{
        const name = (f.type && (f.type.displayName || f.type.name)) || null;
        if (name && !probe.react_component) probe.react_component = String(name);
        if (f._debugSource && f._debugSource.fileName) {{
          probe.react_file = String(f._debugSource.fileName);
          probe.react_line = Number(f._debugSource.lineNumber || 1);
          if (f._debugSource.columnNumber != null) probe.react_column = Number(f._debugSource.columnNumber);
          break;
        }}
        if (f._debugStack && f._debugStack.stack && !probe.react_debug_stack) {{
          probe.react_debug_stack = String(f._debugStack.stack);
        }}
        f = f.return;
        hops++;
      }}
    }}
    if (el.__vueParentComponent && el.__vueParentComponent.type) {{
      const vt = el.__vueParentComponent.type;
      if (vt.__file) probe.vue_file = String(vt.__file);
      if (vt.name || vt.__name) probe.vue_component = String(vt.name || vt.__name);
    }}
    if (el.__svelte_meta && el.__svelte_meta.loc) {{
      const sl = el.__svelte_meta.loc;
      if (sl.file) probe.svelte_file = String(sl.file);
      if (sl.line != null) probe.svelte_line = Number(sl.line) + 1;
      if (sl.char != null) probe.svelte_column = Number(sl.char) + 1;
    }}
    const dataSrc = el.getAttribute('data-source')
      || el.getAttribute('data-source-loc')
      || el.getAttribute('data-v-inspector')
      || el.getAttribute('data-component-file');
    if (dataSrc) probe.data_source = String(dataSrc);
    const dataComp = el.getAttribute('data-component') || el.getAttribute('data-component-name');
    if (dataComp) probe.data_component = String(dataComp);
    const scripts = Array.from(document.querySelectorAll('script[src]'));
    if (scripts.length) probe.script_url = String(scripts[scripts.length - 1].src || '');
  }} catch (_) {{}}
  let outerHtml = '';
  const computedStyles = {{}};
  let textPreview = '';
  if ({full_flag}) {{
    outerHtml = String(el.outerHTML || '');
    const cs = window.getComputedStyle ? window.getComputedStyle(el) : null;
    if (cs) {{
      for (const prop of {curated_json}) {{
        const v = cs.getPropertyValue(prop);
        if (v) computedStyles[prop] = v.trim();
      }}
    }}
    const isSensitive = (tag === 'input' || tag === 'textarea')
      && /password|passwd|secret|token|otp|totp/i.test((el.type || '') + ' ' + (el.name || '') + ' ' + (el.id || ''));
    textPreview = isSensitive ? '[REDACTED]' : String(el.innerText || el.textContent || '').trim().slice(0, 160);
  }}
  const semantic = {{
    test_id: el.getAttribute('data-testid') || null,
    aria_label: el.getAttribute('aria-label') || null,
    role: el.getAttribute('role') || null,
    text_anchor: textPreview ? textPreview.slice(0, 64) : null,
    css_selector: selector,
    name_attr: el.getAttribute('name') || null
  }};
  return JSON.stringify({{
    found: true,
    url: String(window.location && window.location.href ? window.location.href : ''),
    viewport_width: Number(window.innerWidth || document.documentElement.clientWidth || 1280),
    viewport_height: Number(window.innerHeight || document.documentElement.clientHeight || 800),
    dpr,
    is_mobile: Boolean((window.innerWidth || 1280) <= 640 && ('ontouchstart' in window || navigator.maxTouchPoints > 0)),
    selector,
    tag_name: tag,
    id_attr: idAttr,
    classes,
    text_preview: textPreview,
    outer_html: outerHtml,
    computed_styles: computedStyles,
    dom_hierarchy: hierarchy,
    rect,
    semantic_locator: semantic,
    source_map_probe: probe
  }});
}})()"#
    )
}

// ---------------------------------------------------------------------------
// Async CDP Bridge & Unified BrowserSessionBackend Capture Pipeline
// ---------------------------------------------------------------------------

async fn resolve_cdp_node_for_point(bridge: &CdpBridge, x: f64, y: f64) -> Option<i64> {
    let _ = bridge.send_guarded_command("DOM.enable", json!({})).await;
    let _ = bridge
        .send_guarded_command("DOM.getDocument", json!({ "depth": 1, "pierce": true }))
        .await;
    let res = bridge
        .send_guarded_command(
            "DOM.getNodeForLocation",
            json!({
                "x": x.round() as i64,
                "y": y.round() as i64,
                "includeUserAgentShadowDOM": false,
                "ignorePointerEventsNone": true
            }),
        )
        .await
        .ok()?;
    if let Some(node_id) = res.get("nodeId").and_then(Value::as_i64).filter(|&id| id > 0) {
        return Some(node_id);
    }
    if let Some(backend_id) = res
        .get("backendNodeId")
        .and_then(Value::as_i64)
        .filter(|&id| id > 0)
    {
        if let Ok(pushed) = bridge
            .send_guarded_command(
                "DOM.pushNodesByBackendIdsToFrontend",
                json!({ "backendNodeIds": [backend_id] }),
            )
            .await
        {
            if let Some(id) = pushed
                .get("nodeIds")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(Value::as_i64)
                .filter(|&id| id > 0)
            {
                return Some(id);
            }
        }
    }
    None
}

async fn resolve_cdp_node_for_selector(bridge: &CdpBridge, selector: &str) -> Option<i64> {
    let _ = bridge.send_guarded_command("DOM.enable", json!({})).await;
    let doc = bridge
        .send_guarded_command("DOM.getDocument", json!({ "depth": 1, "pierce": true }))
        .await
        .ok()?;
    let root_id = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(Value::as_i64)?;
    let queried = bridge
        .send_guarded_command(
            "DOM.querySelector",
            json!({ "nodeId": root_id, "selector": selector }),
        )
        .await
        .ok()?;
    queried
        .get("nodeId")
        .and_then(Value::as_i64)
        .filter(|&id| id > 0)
}

pub async fn probe_element_at_point_via_cdp(
    bridge: &CdpBridge,
    x: f64,
    y: f64,
) -> Result<Option<DesignHoverTarget>, BotBrowserError> {
    let cdp_node_id = resolve_cdp_node_for_point(bridge, x, y).await;
    let box_model_pair = if let Some(node_id) = cdp_node_id {
        bridge
            .send_guarded_command("DOM.getBoxModel", json!({ "nodeId": node_id }))
            .await
            .ok()
            .and_then(|val| parse_cdp_box_model(&val, 1.0))
    } else {
        None
    };

    let target_expr = format!("document.elementFromPoint({x}, {y})");
    let script = build_dom_inspect_script(&target_expr, DEFAULT_MAX_ANCESTOR_DEPTH, false);
    let eval_res = bridge.evaluate_script(&script).await?;
    let probe: JsProbePayload = parse_eval_json(&eval_res, "design_mode.probe_at_point")?;
    if !probe.found {
        return Ok(None);
    }

    let dpr = if probe.dpr.is_finite() && probe.dpr > 0.0 {
        probe.dpr
    } else {
        1.0
    };
    let (bounding_box, box_model) = match box_model_pair {
        Some((mut cdp_rect, bm)) => {
            cdp_rect.device_pixel_ratio = dpr;
            (cdp_rect, Some(bm))
        }
        None => (
            probe.rect.unwrap_or(ElementRect {
                x,
                y,
                width: 0.0,
                height: 0.0,
                device_pixel_ratio: dpr,
            }),
            None,
        ),
    };
    let empty_maps = BTreeMap::new();
    let source_map = probe
        .source_map_probe
        .as_ref()
        .and_then(|p| resolve_source_map_metadata(p, &empty_maps))
        .map(|r| r.source_ref);

    Ok(Some(DesignHoverTarget {
        selector: probe.selector,
        tag_name: probe.tag_name,
        id_attr: probe.id_attr,
        classes: probe.classes,
        bounding_box,
        box_model,
        source_map,
    }))
}

async fn capture_via_cdp_inner(
    bridge: &CdpBridge,
    cdp_node_id: Option<i64>,
    target_js_expr: &str,
    options: &DesignModeCaptureOptions,
) -> Result<DesignModeCapture, BotBrowserError> {
    let _ = bridge.send_guarded_command("DOM.enable", json!({})).await;
    let _ = bridge.send_guarded_command("CSS.enable", json!({})).await;

    let mut cdp_outer_html: Option<String> = None;
    let mut cdp_box_model: Option<(ElementRect, DesignBoxModel)> = None;
    let mut cdp_styles: Option<BTreeMap<String, String>> = None;

    if let Some(node_id) = cdp_node_id {
        if let Ok(html_res) = bridge
            .send_guarded_command("DOM.getOuterHTML", json!({ "nodeId": node_id }))
            .await
        {
            cdp_outer_html = html_res
                .get("outerHTML")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if let Ok(bm_res) = bridge
            .send_guarded_command("DOM.getBoxModel", json!({ "nodeId": node_id }))
            .await
        {
            cdp_box_model = parse_cdp_box_model(&bm_res, 1.0);
        }
        if let Ok(css_res) = bridge
            .send_guarded_command("CSS.getComputedStyleForNode", json!({ "nodeId": node_id }))
            .await
        {
            let filtered = filter_computed_styles_from_cdp(
                &css_res,
                options.include_all_computed_styles,
            );
            if !filtered.is_empty() {
                cdp_styles = Some(filtered);
            }
        }
    }

    let script = build_dom_inspect_script(target_js_expr, options.max_ancestor_depth, true);
    let eval_res = bridge.evaluate_script(&script).await?;
    let probe: JsProbePayload = parse_eval_json(&eval_res, "design_mode.capture")?;
    if !probe.found {
        return Err(BotBrowserError::LocatorNotFound(
            "no DOM element found at target".to_string(),
        ));
    }

    let dpr = if probe.dpr.is_finite() && probe.dpr > 0.0 {
        probe.dpr
    } else {
        1.0
    };
    let vw = probe.viewport_width.max(1);
    let vh = probe.viewport_height.max(1);
    let (bounding_box, box_model) = match cdp_box_model {
        Some((mut r, bm)) => {
            r.device_pixel_ratio = dpr;
            (r, Some(bm))
        }
        None => (
            probe.rect.unwrap_or(ElementRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
                device_pixel_ratio: dpr,
            }),
            None,
        ),
    };

    let cropped_png_bytes = if options.capture_screenshot && bounding_box.width > 0.0 && bounding_box.height > 0.0 {
        let pad = f64::from(options.crop_padding_px);
        let clip_x = (bounding_box.x - pad).max(0.0);
        let clip_y = (bounding_box.y - pad).max(0.0);
        let clip_w = ((bounding_box.x + bounding_box.width + pad).min(f64::from(vw)) - clip_x).max(1.0);
        let clip_h = ((bounding_box.y + bounding_box.height + pad).min(f64::from(vh)) - clip_y).max(1.0);
        let shot = bridge
            .send_guarded_command(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "fromSurface": true,
                    "clip": {
                        "x": clip_x,
                        "y": clip_y,
                        "width": clip_w,
                        "height": clip_h,
                        "scale": 1
                    }
                }),
            )
            .await
            .and_then(|res| decode_screenshot_png(&res));
        match shot {
            Ok(raw_png) => Some(crop_png_with_padding(
                raw_png,
                &bounding_box,
                f64::from(vw),
                f64::from(vh),
                options.crop_padding_px,
            )?),
            Err(_) => None,
        }
    } else {
        None
    };

    let url = if probe.url.trim().is_empty() {
        "about:blank".to_string()
    } else {
        probe.url
    };
    let payload = DesignDomPayload {
        url,
        viewport: DesignViewport {
            width_px: vw,
            height_px: vh,
            device_scale_factor_x100: (dpr * 100.0).round().clamp(50.0, 800.0) as u16,
            is_mobile: probe.is_mobile,
        },
        selector: probe.selector,
        tag_name: probe.tag_name,
        id_attr: probe.id_attr,
        classes: probe.classes,
        text_preview: probe.text_preview,
        outer_html: cdp_outer_html.unwrap_or(probe.outer_html),
        computed_styles: cdp_styles.unwrap_or(probe.computed_styles),
        dom_hierarchy: probe.dom_hierarchy,
        bounding_box,
        box_model,
        semantic_locator: probe.semantic_locator,
        source_map_probe: probe.source_map_probe,
        cropped_png_bytes,
        captured_at_ms: Some(Utc::now().timestamp_millis()),
    };
    DesignModeCapture::from_dom_payload(payload, options)
        .map_err(|e| BotBrowserError::Cdp(e.to_string()))
}

pub async fn capture_element_at_point_via_cdp(
    bridge: &CdpBridge,
    x: f64,
    y: f64,
    options: &DesignModeCaptureOptions,
) -> Result<DesignModeCapture, BotBrowserError> {
    let node_id = resolve_cdp_node_for_point(bridge, x, y).await;
    let target_expr = format!("document.elementFromPoint({x}, {y})");
    capture_via_cdp_inner(bridge, node_id, &target_expr, options).await
}

pub async fn capture_element_by_selector_via_cdp(
    bridge: &CdpBridge,
    selector: &str,
    options: &DesignModeCaptureOptions,
) -> Result<DesignModeCapture, BotBrowserError> {
    let trimmed = selector.trim();
    if trimmed.is_empty() {
        return Err(BotBrowserError::LocatorNotFound(
            "empty CSS selector".to_string(),
        ));
    }
    let node_id = resolve_cdp_node_for_selector(bridge, trimmed).await;
    let sel_json = serde_json::to_string(trimmed)
        .map_err(|e| BotBrowserError::Cdp(format!("failed to encode selector: {e}")))?;
    let target_expr = format!("document.querySelector({sel_json})");
    capture_via_cdp_inner(bridge, node_id, &target_expr, options).await
}

fn parse_backend_eval_payload<T: for<'de> Deserialize<'de>>(
    val: Value,
    op_name: &str,
) -> Result<T, BotBrowserError> {
    if let Some(s) = val.as_str() {
        serde_json::from_str(s)
            .map_err(|e| BotBrowserError::Cdp(format!("{op_name}: invalid JSON payload: {e}")))
    } else if let Some(inner) = val.get("result").and_then(|r| r.get("value")) {
        if let Some(s) = inner.as_str() {
            serde_json::from_str(s)
                .map_err(|e| BotBrowserError::Cdp(format!("{op_name}: invalid JSON payload: {e}")))
        } else {
            serde_json::from_value(inner.clone())
                .map_err(|e| BotBrowserError::Cdp(format!("{op_name}: unexpected payload: {e}")))
        }
    } else {
        serde_json::from_value(val)
            .map_err(|e| BotBrowserError::Cdp(format!("{op_name}: unexpected payload: {e}")))
    }
}

async fn capture_via_backend_eval(
    backend: &dyn BrowserSessionBackend,
    target_js_expr: &str,
    options: &DesignModeCaptureOptions,
) -> Result<DesignModeCapture, BotBrowserError> {
    let script = build_dom_inspect_script(target_js_expr, options.max_ancestor_depth, true);
    let val = backend.eval_js(&script).await?;
    let probe: JsProbePayload = parse_backend_eval_payload(val, "design_mode.backend_capture")?;
    if !probe.found {
        return Err(BotBrowserError::LocatorNotFound(
            "no DOM element found at target".to_string(),
        ));
    }
    let dpr = if probe.dpr.is_finite() && probe.dpr > 0.0 {
        probe.dpr
    } else {
        1.0
    };
    let vw = probe.viewport_width.max(1);
    let vh = probe.viewport_height.max(1);
    let bounding_box = probe.rect.unwrap_or(ElementRect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
        device_pixel_ratio: dpr,
    });
    let cropped_png_bytes = if options.capture_screenshot
        && bounding_box.width > 0.0
        && bounding_box.height > 0.0
    {
        match backend.capture_screenshot().await {
            Ok(raw_png) => Some(crop_png_with_padding(
                raw_png,
                &bounding_box,
                f64::from(vw),
                f64::from(vh),
                options.crop_padding_px,
            )?),
            Err(_) => None,
        }
    } else {
        None
    };
    let url = if probe.url.trim().is_empty() {
        "about:blank".to_string()
    } else {
        probe.url
    };
    let payload = DesignDomPayload {
        url,
        viewport: DesignViewport {
            width_px: vw,
            height_px: vh,
            device_scale_factor_x100: (dpr * 100.0).round().clamp(50.0, 800.0) as u16,
            is_mobile: probe.is_mobile,
        },
        selector: probe.selector,
        tag_name: probe.tag_name,
        id_attr: probe.id_attr,
        classes: probe.classes,
        text_preview: probe.text_preview,
        outer_html: probe.outer_html,
        computed_styles: probe.computed_styles,
        dom_hierarchy: probe.dom_hierarchy,
        bounding_box,
        box_model: None,
        semantic_locator: probe.semantic_locator,
        source_map_probe: probe.source_map_probe,
        cropped_png_bytes,
        captured_at_ms: Some(Utc::now().timestamp_millis()),
    };
    DesignModeCapture::from_dom_payload(payload, options)
        .map_err(|e| BotBrowserError::Cdp(e.to_string()))
}

pub async fn probe_element_at_point(
    backend: &dyn BrowserSessionBackend,
    x: f64,
    y: f64,
) -> Result<Option<DesignHoverTarget>, BotBrowserError> {
    if let Some(bridge) = backend.cdp_bridge() {
        return probe_element_at_point_via_cdp(bridge, x, y).await;
    }
    let target_expr = format!("document.elementFromPoint({x}, {y})");
    let script = build_dom_inspect_script(&target_expr, DEFAULT_MAX_ANCESTOR_DEPTH, false);
    let val = backend.eval_js(&script).await?;
    let probe: JsProbePayload = parse_backend_eval_payload(val, "design_mode.backend_probe")?;
    if !probe.found {
        return Ok(None);
    }
    let dpr = if probe.dpr.is_finite() && probe.dpr > 0.0 {
        probe.dpr
    } else {
        1.0
    };
    let empty_maps = BTreeMap::new();
    let source_map = probe
        .source_map_probe
        .as_ref()
        .and_then(|p| resolve_source_map_metadata(p, &empty_maps))
        .map(|r| r.source_ref);
    Ok(Some(DesignHoverTarget {
        selector: probe.selector,
        tag_name: probe.tag_name,
        id_attr: probe.id_attr,
        classes: probe.classes,
        bounding_box: probe.rect.unwrap_or(ElementRect {
            x,
            y,
            width: 0.0,
            height: 0.0,
            device_pixel_ratio: dpr,
        }),
        box_model: None,
        source_map,
    }))
}

pub async fn capture_element_at_point(
    backend: &dyn BrowserSessionBackend,
    x: f64,
    y: f64,
    options: &DesignModeCaptureOptions,
) -> Result<DesignModeCapture, BotBrowserError> {
    if let Some(bridge) = backend.cdp_bridge() {
        return capture_element_at_point_via_cdp(bridge, x, y, options).await;
    }
    let target_expr = format!("document.elementFromPoint({x}, {y})");
    capture_via_backend_eval(backend, &target_expr, options).await
}

pub async fn capture_element_by_selector(
    backend: &dyn BrowserSessionBackend,
    selector: &str,
    options: &DesignModeCaptureOptions,
) -> Result<DesignModeCapture, BotBrowserError> {
    if let Some(bridge) = backend.cdp_bridge() {
        return capture_element_by_selector_via_cdp(bridge, selector, options).await;
    }
    let trimmed = selector.trim();
    if trimmed.is_empty() {
        return Err(BotBrowserError::LocatorNotFound(
            "empty CSS selector".to_string(),
        ));
    }
    let sel_json = serde_json::to_string(trimmed)
        .map_err(|e| BotBrowserError::Cdp(format!("failed to encode selector: {e}")))?;
    let target_expr = format!("document.querySelector({sel_json})");
    capture_via_backend_eval(backend, &target_expr, options).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_solid_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([0x20, 0x40, 0x80, 0xFF]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut cursor, ImageFormat::Png)
            .expect("encode png");
        cursor.into_inner()
    }

    #[test]
    fn design_mode_vlq_decode_and_source_map_v3_resolution() {
        assert_eq!(decode_vlq_segment("AAAA").unwrap(), vec![0, 0, 0, 0]);
        assert_eq!(decode_vlq_segment("EACK").unwrap(), vec![2, 0, 1, 5]);
        let raw = r#"{
            "version": 3,
            "sources": ["src/Button.tsx"],
            "names": ["PrimaryButton"],
            "mappings": "AAAA,IAAI"
        }"#;
        let sm = parse_source_map_v3(raw).expect("parse v3 map");
        let resolved = resolve_generated_position(&sm, 1, 0).expect("resolve gen pos");
        assert_eq!(resolved.file_path, "src/Button.tsx");
        assert_eq!(resolved.line, 1);

        let attr_ref = parse_framework_source_attr("src/Card.tsx:42:8").expect("parse attr");
        assert_eq!(attr_ref.file_path, "src/Card.tsx");
        assert_eq!(attr_ref.line, 42);
        assert_eq!(attr_ref.column, Some(8));
    }

    #[test]
    fn design_mode_sanitize_outer_html_and_curated_css_filtering() {
        let (clean, redacted) = sanitize_outer_html(
            "<form><input type=\"password\" value=\"super-secret-42\" /><button>Deploy</button></form>",
            DEFAULT_MAX_OUTER_HTML_BYTES,
        );
        assert!(redacted);
        assert!(clean.contains("Deploy"));
        assert!(clean.contains("value=\"[REDACTED]\""));
        assert!(!clean.contains("super-secret-42"));

        let huge = format!("<div>{}</div>", "x".repeat(400));
        let (clipped, was_redacted) = sanitize_outer_html(&huge, 64);
        assert!(!was_redacted);
        assert!(clipped.contains("truncated"));
        assert!(clipped.len() <= 64);

        let mut styles = BTreeMap::new();
        styles.insert("color".to_string(), "rgb(255, 255, 255)".to_string());
        styles.insert("font-family".to_string(), "Inter".to_string());
        styles.insert("non-curated-prop".to_string(), "ignored".to_string());
        let filtered = filter_computed_styles_map(&styles, false);
        assert_eq!(filtered.get("font-family").map(String::as_str), Some("Inter"));
        assert!(!filtered.contains_key("non-curated-prop"));
    }

    #[test]
    fn design_mode_png_crop_padding_and_sha256_integrity() {
        let png = make_solid_png(100, 80);
        let rect = ElementRect {
            x: 20.0,
            y: 15.0,
            width: 30.0,
            height: 20.0,
            device_pixel_ratio: 1.0,
        };
        let cropped = crop_png_with_padding(png, &rect, 100.0, 80.0, 4).expect("crop png");
        assert!(cropped.starts_with(b"\x89PNG\r\n\x1a\n"));
        let (sha, (w, h)) = compute_png_sha256_and_dims(&cropped).expect("sha and dims");
        assert_eq!(sha.len(), 64);
        assert_eq!((w, h), (38, 28));
    }

    #[test]
    fn design_mode_capture_from_dom_payload_and_contract_handoff() {
        let png = make_solid_png(64, 32);
        let payload = DesignDomPayload {
            url: "http://feat.orca.localhost:3000/dashboard".to_string(),
            viewport: DesignViewport {
                width_px: 1280,
                height_px: 800,
                device_scale_factor_x100: 200,
                is_mobile: false,
            },
            selector: "button.primary-cta".to_string(),
            tag_name: "button".to_string(),
            id_attr: Some("deploy-btn".to_string()),
            classes: vec!["primary-cta".to_string()],
            text_preview: "Deploy".to_string(),
            outer_html: "<button id=\"deploy-btn\" class=\"primary-cta\">Deploy</button>".to_string(),
            computed_styles: BTreeMap::from([
                ("color".to_string(), "rgb(255, 255, 255)".to_string()),
                ("font-family".to_string(), "Inter".to_string()),
            ]),
            dom_hierarchy: vec![DomAncestorNode {
                tag: "main".to_string(),
                id: None,
                classes: vec!["shell".to_string()],
                role: Some("main".to_string()),
                nth_of_type: 1,
                selector_segment: "main.shell".to_string(),
            }],
            bounding_box: ElementRect {
                x: 100.0,
                y: 200.0,
                width: 120.0,
                height: 40.0,
                device_pixel_ratio: 2.0,
            },
            box_model: None,
            semantic_locator: None,
            source_map_probe: Some(RawSourceMapProbe {
                data_source: Some("src/Button.tsx:42:8".to_string()),
                ..Default::default()
            }),
            cropped_png_bytes: Some(png),
            captured_at_ms: Some(1_700_000_000_000),
        };
        let capture = DesignModeCapture::from_dom_payload(payload, &DesignModeCaptureOptions::default())
            .expect("build capture");
        let contract = capture.to_orca_capture_contract();
        assert_eq!(contract.source_map.as_ref().map(|s| s.file_path.as_str()), Some("src/Button.tsx"));
        assert_eq!(contract.source_map.as_ref().map(|s| s.line), Some(42));
        assert!(contract.screenshot_sha256.as_ref().is_some_and(|h| h.len() == 64));
        assert!(capture.cropped_png_base64().is_some());
        let (cx_bp, cy_bp) = capture.normalized_center_bp();
        assert!(cx_bp > 0 && cx_bp < 10_000 && cy_bp > 0 && cy_bp < 10_000);

        let handoff = capture.to_chat_handoff(Some("Increase button contrast"), &[]);
        let md = capture.format_chat_attachment_markdown();
        assert!(md.contains("button.primary-cta"));
        assert!(handoff.prompt_markdown.contains("Increase button contrast"));
    }

    #[test]
    fn design_mode_spec_validation_and_breadcrumb_formatting() {
        let payload = DesignDomPayload {
            url: "http://127.0.0.1:4173".to_string(),
            viewport: DesignViewport {
                width_px: 390,
                height_px: 844,
                device_scale_factor_x100: 300,
                is_mobile: true,
            },
            selector: "nav > a.active".to_string(),
            tag_name: "a".to_string(),
            id_attr: None,
            classes: vec!["active".to_string()],
            text_preview: "Home".to_string(),
            outer_html: "<a class=\"active\">Home</a>".to_string(),
            computed_styles: BTreeMap::new(),
            dom_hierarchy: vec![DomAncestorNode {
                tag: "nav".to_string(),
                id: Some("top".to_string()),
                classes: vec![],
                role: Some("navigation".to_string()),
                nth_of_type: 1,
                selector_segment: "nav#top".to_string(),
            }],
            bounding_box: ElementRect {
                x: 10.0,
                y: 12.0,
                width: 80.0,
                height: 32.0,
                device_pixel_ratio: 3.0,
            },
            box_model: None,
            semantic_locator: None,
            source_map_probe: None,
            cropped_png_bytes: None,
            captured_at_ms: Some(1_700_000_000_001),
        };
        let capture = DesignModeCapture::from_dom_payload(payload, &DesignModeCaptureOptions::default())
            .expect("mobile capture");
        assert!(capture.dom_breadcrumb().contains("nav#top"));
        let overlay = DesignOverlaySpec {
            enabled: true,
            diff_mode: DesignDiffMode::Overlay,
            opacity_percent: 85,
            show_spacing_guides: true,
            pins: Vec::new(),
        };
        let spec = capture.into_mode_spec("ws-mobile", overlay);
        assert!(spec.validate().is_ok());
        assert!(spec.viewport.is_mobile);
    }
}
