//! Non-themable host-controlled `SecuritySurfaceModal` (F19 / F22 anti-spoofing chrome).
//!
//! Security-critical approval & verification dialogs MUST NEVER consume `TdesktopThemePalette`,
//! `AppPreferences`, or any plugin theme token. Every color is compile-time fixed to
//! `SECURITY_SURFACE_PALETTE` (WCAG AAA >= 7:1 contrast) and anchored to
//! `orca_ide::SecuritySurface::protected(...)` (`theme_override_allowed == false`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_bots::ConnectorAccessRequest;
use orca_core::bot_ecosystem::TeamSharePublicIdentity;
use orca_core::orchestration_protocol::{PluginManifestV2, PluginTier};
use orca_ide::{PluginContributionPoint, PluginHostV2, SecuritySurface, audit_actor_for_manifest};
use sha2::{Digest, Sha256};

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

/// Compile-time fixed high-contrast host chrome colors immune to user and plugin themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecuritySurfacePalette {
    pub backdrop_rgba: u32,
    pub surface_bg: u32,
    pub header_bg: u32,
    pub card_bg: u32,
    pub border: u32,
    pub fg_primary: u32,
    pub fg_muted: u32,
    pub approve_bg: u32,
    pub deny_bg: u32,
    pub badge_bg: u32,
    pub badge_fg: u32,
}

pub const SECURITY_SURFACE_BACKDROP_RGBA: u32 = 0x05070CE6;
pub const SECURITY_SURFACE_BG: u32 = 0x0B0F19;
pub const SECURITY_SURFACE_HEADER_BG: u32 = 0x161D2E;
pub const SECURITY_SURFACE_CARD_BG: u32 = 0x111827;
pub const SECURITY_SURFACE_BORDER: u32 = 0xF59E0B;
pub const SECURITY_SURFACE_FG_PRIMARY: u32 = 0xF8FAFC;
pub const SECURITY_SURFACE_FG_MUTED: u32 = 0xCBD5E1;
pub const SECURITY_SURFACE_APPROVE_BG: u32 = 0x16A34A;
pub const SECURITY_SURFACE_DENY_BG: u32 = 0xDC2626;
pub const SECURITY_SURFACE_BADGE_BG: u32 = 0x78350F;
pub const SECURITY_SURFACE_BADGE_FG: u32 = 0xFEF3C7;

pub const SECURITY_SURFACE_PALETTE: SecuritySurfacePalette = SecuritySurfacePalette {
    backdrop_rgba: SECURITY_SURFACE_BACKDROP_RGBA,
    surface_bg: SECURITY_SURFACE_BG,
    header_bg: SECURITY_SURFACE_HEADER_BG,
    card_bg: SECURITY_SURFACE_CARD_BG,
    border: SECURITY_SURFACE_BORDER,
    fg_primary: SECURITY_SURFACE_FG_PRIMARY,
    fg_muted: SECURITY_SURFACE_FG_MUTED,
    approve_bg: SECURITY_SURFACE_APPROVE_BG,
    deny_bg: SECURITY_SURFACE_DENY_BG,
    badge_bg: SECURITY_SURFACE_BADGE_BG,
    badge_fg: SECURITY_SURFACE_BADGE_FG,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityModalKind {
    PluginApproval {
        plugin_id: String,
        publisher: Option<String>,
        version: String,
        tier: PluginTier,
        audit_actor: String,
        capability_fingerprint: String,
        permissions: Vec<String>,
        tools: Vec<String>,
        contributions: Vec<PluginContributionPoint>,
        hooks_count: usize,
    },
    BotAccessApproval {
        request_id: String,
        bot_id: String,
        host_id: String,
        member_name: String,
        member_source: String,
        member_id: String,
        connector_workspace_id: Option<String>,
        expires_at_ms: i64,
    },
    E2eeVerification {
        peer_host_id: String,
        x25519_public_b64: String,
        ed25519_public_b64: String,
        identity_fingerprint_sha256: String,
    },
    OrcaYamlTrustApproval {
        repo_root: String,
        trust_hash: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecuritySurfaceDecision {
    Approved,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecuritySurfaceModalState {
    surface: SecuritySurface,
    pub kind: SecurityModalKind,
    pub decision: Option<SecuritySurfaceDecision>,
}

impl SecuritySurfaceModalState {
    pub fn open_plugin_approval(
        manifest: &PluginManifestV2,
        contributions: &[PluginContributionPoint],
    ) -> Self {
        let surface_id = format!("security:plugin-approval:{}", manifest.id.trim());
        Self {
            surface: SecuritySurface::protected(surface_id),
            kind: SecurityModalKind::PluginApproval {
                plugin_id: manifest.id.clone(),
                publisher: manifest.publisher.clone(),
                version: manifest.version.clone(),
                tier: manifest.tier,
                audit_actor: audit_actor_for_manifest(manifest),
                capability_fingerprint: manifest.capability_fingerprint(),
                permissions: manifest.permissions.clone(),
                tools: manifest.tools.clone(),
                contributions: contributions.to_vec(),
                hooks_count: manifest.hooks.len(),
            },
            decision: None,
        }
    }

    pub fn open_bot_access_approval(req: &ConnectorAccessRequest) -> Self {
        let surface_id = format!("security:bot-access:{}", req.request_id.trim());
        Self {
            surface: SecuritySurface::protected(surface_id),
            kind: SecurityModalKind::BotAccessApproval {
                request_id: req.request_id.clone(),
                bot_id: req.bot_id.clone(),
                host_id: req.host_id.clone(),
                member_name: req.member.name.clone(),
                member_source: req
                    .member
                    .source
                    .clone()
                    .unwrap_or_else(|| "external".to_string()),
                member_id: req.member.id.clone(),
                connector_workspace_id: req.connector_workspace_id.clone(),
                expires_at_ms: req.expires_at_ms,
            },
            decision: None,
        }
    }

    pub fn open_e2ee_verification(
        surface_id: &str,
        peer_identity: &TeamSharePublicIdentity,
    ) -> Self {
        let mut h = Sha256::new();
        h.update(b"orca-e2ee-peer-identity\0");
        h.update(peer_identity.host_id.as_bytes());
        h.update([0]);
        h.update(peer_identity.x25519_public_b64.as_bytes());
        h.update([0]);
        h.update(peer_identity.ed25519_public_b64.as_bytes());
        let fp = format!("{:x}", h.finalize());

        let id = if surface_id.trim().is_empty() {
            format!("security:e2ee-verify:{}", peer_identity.host_id)
        } else {
            surface_id.trim().to_string()
        };
        Self {
            surface: SecuritySurface::protected(id),
            kind: SecurityModalKind::E2eeVerification {
                peer_host_id: peer_identity.host_id.clone(),
                x25519_public_b64: peer_identity.x25519_public_b64.clone(),
                ed25519_public_b64: peer_identity.ed25519_public_b64.clone(),
                identity_fingerprint_sha256: fp,
            },
            decision: None,
        }
    }

    pub fn open_orca_yaml_trust(repo_root: &str, trust_hash: &str) -> Self {
        let surface_id = format!("security:orca-yaml-trust:{}", trust_hash.trim());
        Self {
            surface: SecuritySurface::protected(surface_id),
            kind: SecurityModalKind::OrcaYamlTrustApproval {
                repo_root: repo_root.to_string(),
                trust_hash: trust_hash.to_string(),
            },
            decision: None,
        }
    }

    pub fn security_surface(&self) -> &SecuritySurface {
        &self.surface
    }

    /// Proves theme invariance: returns the compile-time constant `SECURITY_SURFACE_PALETTE`
    /// regardless of ambient `TdesktopThemePalette` or plugin theme requests.
    pub fn effective_palette(
        &self,
        _ambient_theme: &TdesktopThemePalette,
        _plugin_theme: Option<&str>,
    ) -> SecuritySurfacePalette {
        SECURITY_SURFACE_PALETTE
    }

    pub const fn effective_chrome_colors(&self) -> SecuritySurfacePalette {
        SECURITY_SURFACE_PALETTE
    }

    /// Queries `PluginHostV2::can_apply_plugin_theme` to confirm plugin theming is denied.
    pub fn is_plugin_theme_allowed(&self, plugin_host: &PluginHostV2) -> bool {
        plugin_host.can_apply_plugin_theme(&self.surface)
    }

    pub fn approve(&mut self) {
        self.decision = Some(SecuritySurfaceDecision::Approved);
    }

    pub fn deny(&mut self) {
        self.decision = Some(SecuritySurfaceDecision::Denied);
    }
}

/// Non-themable GPUI `Entity` rendering host-controlled security approval & verification dialogs.
///
/// Deliberately does NOT store or accept `TdesktopThemePalette` or any plugin theme tokens.
pub struct SecuritySurfaceModal {
    pub state: SecuritySurfaceModalState,
    pub language_id: String,
    focus_handle: FocusHandle,
    pending_decision: Option<SecuritySurfaceDecision>,
}

impl SecuritySurfaceModal {
    pub fn new(
        state: SecuritySurfaceModalState,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state,
            language_id: language_id.into(),
            focus_handle: cx.focus_handle(),
            pending_decision: None,
        }
    }

    pub fn set_locale(&mut self, language_id: impl Into<String>, cx: &mut Context<Self>) {
        self.language_id = language_id.into();
        cx.notify();
    }

    pub fn security_surface(&self) -> &SecuritySurface {
        self.state.security_surface()
    }

    pub fn effective_palette(
        &self,
        ambient_theme: &TdesktopThemePalette,
        plugin_theme: Option<&str>,
    ) -> SecuritySurfacePalette {
        self.state.effective_palette(ambient_theme, plugin_theme)
    }

    pub const fn effective_chrome_colors(&self) -> SecuritySurfacePalette {
        self.state.effective_chrome_colors()
    }

    pub fn take_decision(&mut self) -> Option<SecuritySurfaceDecision> {
        self.pending_decision.take()
    }

    pub fn trigger_approve(&mut self, cx: &mut Context<Self>) {
        self.state.approve();
        self.pending_decision = Some(SecuritySurfaceDecision::Approved);
        cx.notify();
    }

    pub fn trigger_deny(&mut self, cx: &mut Context<Self>) {
        self.state.deny();
        self.pending_decision = Some(SecuritySurfaceDecision::Denied);
        cx.notify();
    }

    pub fn handle_key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if ev.keystroke.key.eq_ignore_ascii_case("escape") {
            self.trigger_deny(cx);
            return true;
        }
        true
    }

    fn render_kv_row(label: &str, value: &str, mono: bool) -> AnyElement {
        let p = SECURITY_SURFACE_PALETTE;
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .bg(rgb(p.card_bg))
            .border_1()
            .border_color(rgba(0xFFFFFF22))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.fg_muted))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.fg_primary))
                    .when(mono, |el| el.font_weight(FontWeight::MEDIUM))
                    .child(value.to_string()),
            )
            .into_any_element()
    }

    fn render_kind_body(&self) -> AnyElement {
        let lang = self.language_id.as_str();
        let mut body = div().flex().flex_col().gap(px(8.0));
        match &self.state.kind {
            SecurityModalKind::PluginApproval {
                plugin_id,
                publisher,
                version,
                tier,
                audit_actor,
                capability_fingerprint,
                permissions,
                tools,
                contributions,
                hooks_count,
            } => {
                let pub_str = publisher.as_deref().unwrap_or("local");
                let contrib_str = if contributions.is_empty() {
                    "none".to_string()
                } else {
                    contributions
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                body = body
                    .child(Self::render_kv_row(
                        tr_str(lang, "插件与分级", "Plugin & Tier"),
                        &format!("{plugin_id} v{version} · publisher={pub_str} · tier={}", tier.as_str()),
                        false,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "审计主体", "Audit Actor"),
                        audit_actor,
                        true,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "SHA-256 能力指纹", "SHA-256 Capability Fingerprint"),
                        capability_fingerprint,
                        true,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "声明权限 / 工具 / 扩展点", "Permissions / Tools / Contributions"),
                        &format!(
                            "perms=[{}] · tools=[{}] · hooks={hooks_count} · points=[{contrib_str}]",
                            permissions.join(", "),
                            tools.join(", ")
                        ),
                        false,
                    ));
            }
            SecurityModalKind::BotAccessApproval {
                request_id,
                bot_id,
                host_id,
                member_name,
                member_source,
                member_id,
                connector_workspace_id,
                expires_at_ms,
            } => {
                let ws_str = connector_workspace_id.as_deref().unwrap_or("default");
                body = body
                    .child(Self::render_kv_row(
                        tr_str(lang, "请求方", "Requester"),
                        &format!("{member_name} ({member_source}:{member_id})"),
                        false,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "目标机器人与主机", "Target Bot & Host"),
                        &format!("bot={bot_id} · host={host_id} · workspace={ws_str}"),
                        true,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "请求 ID 与过期时间", "Request ID & Expiration"),
                        &format!("{request_id} · expires_at_ms={expires_at_ms}"),
                        true,
                    ));
            }
            SecurityModalKind::E2eeVerification {
                peer_host_id,
                x25519_public_b64,
                ed25519_public_b64,
                identity_fingerprint_sha256,
            } => {
                body = body
                    .child(Self::render_kv_row(
                        tr_str(lang, "对端主机", "Peer Host"),
                        peer_host_id,
                        true,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "身份指纹 (SHA-256)", "Identity Fingerprint (SHA-256)"),
                        identity_fingerprint_sha256,
                        true,
                    ))
                    .child(Self::render_kv_row(
                        "X25519 Public Key",
                        x25519_public_b64,
                        true,
                    ))
                    .child(Self::render_kv_row(
                        "Ed25519 Public Key",
                        ed25519_public_b64,
                        true,
                    ));
            }
            SecurityModalKind::OrcaYamlTrustApproval {
                repo_root,
                trust_hash,
            } => {
                body = body
                    .child(Self::render_kv_row(
                        tr_str(lang, "仓库路径", "Repository Root"),
                        repo_root,
                        true,
                    ))
                    .child(Self::render_kv_row(
                        tr_str(lang, "orca.yaml 信任哈希 (SHA-256)", "orca.yaml Setup Trust Hash (SHA-256)"),
                        trust_hash,
                        true,
                    ));
            }
        }
        body.into_any_element()
    }
}

impl Focusable for SecuritySurfaceModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SecuritySurfaceModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = SECURITY_SURFACE_PALETTE;
        let lang = self.language_id.as_str();
        let title = match &self.state.kind {
            SecurityModalKind::PluginApproval { .. } => {
                tr_str(lang, "审批插件能力指纹", "Approve Plugin Capabilities")
            }
            SecurityModalKind::BotAccessApproval { .. } => {
                tr_str(lang, "审批外部访问请求 (Who Can Ask)", "Approve Bot Access Request")
            }
            SecurityModalKind::E2eeVerification { .. } => {
                tr_str(lang, "验证端到端加密对端身份", "Verify E2EE Team Share Peer")
            }
            SecurityModalKind::OrcaYamlTrustApproval { .. } => {
                tr_str(lang, "信任 orca.yaml 初始化脚本", "Trust orca.yaml Setup Script")
            }
        };
        let surface_id = self.state.security_surface().surface_id.clone();

        div()
            .id("orca-security-surface-backdrop")
            .track_focus(&self.focus_handle)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(rgba(p.backdrop_rgba))
            .flex()
            .items_center()
            .justify_center()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                this.handle_key_down(ev, cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("orca-security-surface-card")
                    .w(px(560.0))
                    .max_w(px(620.0))
                    .rounded(px(12.0))
                    .bg(rgb(p.surface_bg))
                    .border_2()
                    .border_color(rgb(p.border))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .bg(rgb(p.header_bg))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(tg_icon(IconName::ShieldMod, 18.0, p.border))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(p.fg_primary))
                                                    .child(title.to_string()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .px(px(8.0))
                                            .py(px(2.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(p.badge_bg))
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(p.badge_fg))
                                            .child("theme_override_allowed: false"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.fg_muted))
                                    .child(format!(
                                        "Host Security Surface — Plugin Themes Disabled · {surface_id}"
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(self.render_kind_body()),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .bg(rgb(p.header_bg))
                            .border_t_1()
                            .border_color(rgba(0xFFFFFF22))
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .id("orca-security-deny-btn")
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(7.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(p.deny_bg))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.fg_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.trigger_deny(cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "拒绝", "Deny")),
                            )
                            .child(
                                div()
                                    .id("orca-security-approve-btn")
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(7.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(p.approve_bg))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.fg_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.trigger_approve(cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "确认批准", "Approve")),
                            ),
                    ),
            )
            .into_any_element()
    }
}
