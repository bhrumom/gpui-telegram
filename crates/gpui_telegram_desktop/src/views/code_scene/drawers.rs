//! Slide-over translucent interactive drawers for Code Scene (`CS-F14`).
//!
//! 1. Comments Drawer (Hot/Newest sort, replies, CJK IME input)
//! 2. Live Code Inspector & Hot-Reload Editor (`</>` with real-time `compile_and_verify`)
//! 3. Creator / Bot Profile Sheet (follow toggle + 2-column works grid)
//! 4. AI Bot Creation & Remix Studio (bot picker, modality, prompt, live preview, iterate, publish)
//! 5. Share-to-Chat Picker (forward rich `GenerativeCard` to any Telegram chat)

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_code_scene::OFFICIAL_CODE_SCENE_BOTS;
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneResponse, AssetKind, CodeSceneAssetDto, CodeSceneCommentDto,
    CodeSceneCommentSort, ValidationReportDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName};

/// Currently open slide-over drawer above the Code Scene stage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CodeSceneDrawer {
    #[default]
    None,
    Comments,
    CodeInspector,
    CreatorProfile {
        author_user_id: i64,
        author_username: String,
    },
    AiStudio,
    ShareToChat,
}

/// Focused text input field inside Code Scene (routed through `AppShell` `EntityInputHandler` for CJK IME).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveCodeSceneInput {
    #[default]
    None,
    Search,
    Comment,
    CodeEditor,
    StudioPrompt,
    StudioIterate,
    StudioTitle,
    StudioTags,
}

/// Interactive state for the In-Scene AI Bot Creation & Remix Studio drawer.
#[derive(Debug, Clone)]
pub struct AiStudioState {
    pub selected_bot_id: i64,
    pub selected_bot_username: String,
    pub selected_kind: AssetKind,
    pub prompt: String,
    pub iterate_instruction: String,
    pub title: String,
    pub tags_input: String,
    pub remix_parent_id: Option<String>,
    pub remix_parent_title: Option<String>,
    pub current_draft: Option<AiGenerateSceneResponse>,
    pub share_to_active_chat: bool,
    pub is_busy: bool,
    pub status_message: Option<String>,
}

impl Default for AiStudioState {
    fn default() -> Self {
        let first = &OFFICIAL_CODE_SCENE_BOTS[0];
        Self {
            selected_bot_id: first.user_id,
            selected_bot_username: first.username.to_string(),
            selected_kind: AssetKind::Animation,
            prompt: String::new(),
            iterate_instruction: String::new(),
            title: String::new(),
            tags_input: String::new(),
            remix_parent_id: None,
            remix_parent_title: None,
            current_draft: None,
            share_to_active_chat: false,
            is_busy: false,
            status_message: None,
        }
    }
}

/// Actions emitted by any open slide-over drawer.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawerAction {
    Close,
    SetCommentSort(CodeSceneCommentSort),
    FocusInput(ActiveCodeSceneInput),
    SubmitComment,
    SetReplyTarget(Option<(String, String)>),
    ValidateEditorCode,
    HotReloadEditorCode,
    CopyEditorCode,
    PublishEditorRemix,
    OpenAiRemixFromCurrent,
    ToggleFollowCreator(i64),
    OpenSceneById(String),
    StudioSelectBot(i64, String),
    StudioSelectKind(AssetKind),
    StudioPickPreset(String, AssetKind),
    StudioGenerate,
    StudioIterate,
    StudioClearRemixParent,
    StudioToggleShareAfterPublish,
    StudioPublish,
    ShareToTargetChat(u64),
}

/// Props passed to `render_active_drawer`.
pub struct DrawerRenderProps<'a> {
    pub drawer: &'a CodeSceneDrawer,
    pub active_asset: Option<&'a CodeSceneAssetDto>,
    pub active_input: ActiveCodeSceneInput,
    pub ime_preedit: Option<&'a str>,
    pub comments: &'a [CodeSceneCommentDto],
    pub comment_sort: CodeSceneCommentSort,
    pub comment_draft: &'a str,
    pub reply_target: Option<&'a (String, String)>,
    pub editor_source: &'a str,
    pub editor_validation: Option<&'a ValidationReportDto>,
    pub creator_works: &'a [CodeSceneAssetDto],
    pub creator_following: bool,
    pub studio: &'a AiStudioState,
    pub studio_preview_el: Option<AnyElement>,
    pub share_targets: &'a [(u64, String, String, u32)],
    pub language_id: &'a str,
}

fn input_display(
    value: &str,
    placeholder: &str,
    focused: bool,
    ime_preedit: Option<&str>,
) -> (String, bool) {
    let pre = if focused { ime_preedit.unwrap_or("") } else { "" };
    if value.is_empty() && pre.is_empty() {
        (
            if focused {
                format!("| {placeholder}")
            } else {
                placeholder.to_string()
            },
            true,
        )
    } else if focused {
        (format!("{value}{pre}|"), false)
    } else {
        (value.to_string(), false)
    }
}

fn drawer_shell<V: 'static>(
    id: &'static str,
    title: &str,
    width_px: f32,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
    body: AnyElement,
) -> AnyElement {
    let cb_close = on_action.clone();
    div()
        .id(id)
        .absolute()
        .top(px(64.0))
        .right(px(14.0))
        .bottom(px(58.0))
        .w(px(width_px))
        .rounded(px(16.0))
        .bg(rgba(0x10121CF2))
        .border_1()
        .border_color(rgba(0xFFFFFF24))
        .flex()
        .flex_col()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .h(px(46.0))
                .px(px(14.0))
                .border_b_1()
                .border_color(rgba(0xFFFFFF18))
                .flex()
                .items_center()
                .justify_between()
                .flex_shrink_0()
                .child(
                    div()
                        .text_size(px(14.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .id("cs-drawer-close-btn")
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(999.0))
                        .bg(rgba(0xFFFFFF14))
                        .hover(|s| s.bg(rgba(0xFFFFFF2A)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_close(this, DrawerAction::Close, cx);
                            }),
                        )
                        .child(tg_icon(IconName::Close, 13.0, 0xFFFFFF)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .flex()
                .flex_col()
                .child(body),
        )
        .into_any_element()
}

/// Renders whichever slide-over drawer is currently open (`None` returns `None`).
pub fn render_active_drawer<V: 'static>(
    props: DrawerRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    let lang = props.language_id;
    match props.drawer {
        CodeSceneDrawer::None => None,
        CodeSceneDrawer::Comments => {
            let count = props.comments.len();
            let title = format!("{} ({count})", tr_str(lang, "评论", "Comments"));
            let body = render_comments_body(&props, cx, on_action.clone());
            Some(drawer_shell("cs-comments-drawer", &title, 390.0, cx, on_action, body))
        }
        CodeSceneDrawer::CodeInspector => {
            let title = tr_str(lang, "代码查看与热更新编辑器 (cs1)", "Code Inspector & Hot Reload (cs1)");
            let body = render_code_inspector_body(&props, cx, on_action.clone());
            Some(drawer_shell("cs-code-drawer", title, 450.0, cx, on_action, body))
        }
        CodeSceneDrawer::CreatorProfile {
            author_user_id,
            author_username,
        } => {
            let title = format!("@{}", author_username.trim_start_matches('@'));
            let body = render_creator_profile_body(
                *author_user_id,
                author_username,
                &props,
                cx,
                on_action.clone(),
            );
            Some(drawer_shell("cs-creator-drawer", &title, 400.0, cx, on_action, body))
        }
        CodeSceneDrawer::AiStudio => {
            let title = tr_str(lang, "AI Bot 代码场景创作室", "AI Bot Code Scene Studio");
            let body = render_ai_studio_body(props, cx, on_action.clone());
            Some(drawer_shell("cs-ai-studio-drawer", title, 460.0, cx, on_action, body))
        }
        CodeSceneDrawer::ShareToChat => {
            let title = tr_str(lang, "转发到聊天会话", "Share Code Scene to Chat");
            let body = render_share_sheet_body(&props, cx, on_action.clone());
            Some(drawer_shell("cs-share-drawer", title, 360.0, cx, on_action, body))
        }
    }
}

fn render_comments_body<V: 'static>(
    props: &DrawerRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let lang = props.language_id;
    let cb_hot = on_action.clone();
    let cb_new = on_action.clone();
    let cb_focus = on_action.clone();
    let cb_submit = on_action.clone();
    let cb_clear_reply = on_action.clone();

    let comment_focused = props.active_input == ActiveCodeSceneInput::Comment;
    let (draft_text, is_placeholder) = input_display(
        props.comment_draft,
        tr_str(lang, "说点什么吧… (Enter 发送)", "Write a comment… (Enter to send)"),
        comment_focused,
        props.ime_preedit,
    );

    let comment_rows = props.comments.iter().enumerate().map(|(idx, c)| {
        let cid = c.comment_id.clone();
        let author = c.author_username.clone();
        let cb_reply = on_action.clone();
        div()
            .id(("cs-comment-row", idx))
            .p(px(10.0))
            .rounded(px(10.0))
            .bg(rgba(0xFFFFFF0D))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0x25F4EEFF))
                            .child(format!("@{}", c.author_username.trim_start_matches('@'))),
                    )
                    .child(
                        div()
                            .id(("cs-comment-reply-btn", idx))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgba(0xFFFFFF88))
                            .hover(|s| s.text_color(rgba(0xFFFFFFFF)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_reply(
                                        this,
                                        DrawerAction::SetReplyTarget(Some((cid.clone(), author.clone()))),
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "回复", "Reply")),
                    ),
            )
            .when_some(c.parent_comment_id.as_ref(), |el, _| {
                el.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgba(0xFFFFFF66))
                        .child(tr_str(lang, "↳ 回复评论", "↳ Reply")),
                )
            })
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgba(0xFFFFFFEE))
                    .child(c.content.clone()),
            )
    });

    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        // Sort bar
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .border_b_1()
                .border_color(rgba(0xFFFFFF12))
                .child(
                    div()
                        .id("cs-comment-sort-hot")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(if props.comment_sort == CodeSceneCommentSort::Hot {
                            rgba(0xFE2C55CC)
                        } else {
                            rgba(0xFFFFFF14)
                        })
                        .text_size(px(12.0))
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_hot(this, DrawerAction::SetCommentSort(CodeSceneCommentSort::Hot), cx);
                            }),
                        )
                        .child(tr_str(lang, "热门", "Hot")),
                )
                .child(
                    div()
                        .id("cs-comment-sort-new")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(if props.comment_sort == CodeSceneCommentSort::Newest {
                            rgba(0xFE2C55CC)
                        } else {
                            rgba(0xFFFFFF14)
                        })
                        .text_size(px(12.0))
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_new(this, DrawerAction::SetCommentSort(CodeSceneCommentSort::Newest), cx);
                            }),
                        )
                        .child(tr_str(lang, "最新", "Newest")),
                ),
        )
        // Scrollable comments list
        .child(
            div()
                .id("cs-comments-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .when(props.comments.is_empty(), |col| {
                    col.child(
                        div()
                            .py(px(28.0))
                            .text_size(px(13.0))
                            .text_color(rgba(0xFFFFFF6E))
                            .child(tr_str(
                                lang,
                                "还没有评论，来发表第一条互动评论吧！",
                                "No comments yet — be the first to comment!",
                            )),
                    )
                })
                .children(comment_rows),
        )
        // Bottom comment composer
        .child(
            div()
                .p(px(10.0))
                .border_t_1()
                .border_color(rgba(0xFFFFFF18))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .when_some(props.reply_target, |col, (_, target_user)| {
                    col.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(rgba(0xFFFFFF12))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgba(0x25F4EEFF))
                                    .child(format!(
                                        "{} @{}",
                                        tr_str(lang, "正在回复", "Replying to"),
                                        target_user.trim_start_matches('@')
                                    )),
                            )
                            .child(
                                div()
                                    .id("cs-clear-reply-btn")
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .text_color(rgba(0xFFFFFF99))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            cb_clear_reply(this, DrawerAction::SetReplyTarget(None), cx);
                                        }),
                                    )
                                    .child("×"),
                            ),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("cs-comment-input")
                                .flex_1()
                                .min_h(px(36.0))
                                .px(px(10.0))
                                .py(px(7.0))
                                .rounded(px(10.0))
                                .bg(rgba(0xFFFFFF12))
                                .border_1()
                                .border_color(if comment_focused {
                                    rgba(0xFE2C55CC)
                                } else {
                                    rgba(0xFFFFFF24)
                                })
                                .cursor_text()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        cb_focus(
                                            this,
                                            DrawerAction::FocusInput(ActiveCodeSceneInput::Comment),
                                            cx,
                                        );
                                    }),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(if is_placeholder {
                                            rgba(0xFFFFFF66)
                                        } else {
                                            rgba(0xFFFFFFFF)
                                        })
                                        .child(draft_text),
                                ),
                        )
                        .child(
                            div()
                                .id("cs-comment-submit-btn")
                                .cursor_pointer()
                                .h(px(36.0))
                                .px(px(14.0))
                                .rounded(px(10.0))
                                .bg(rgba(0xFE2C55FF))
                                .hover(|s| s.opacity(0.9))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0xFFFFFFFF))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        cb_submit(this, DrawerAction::SubmitComment, cx);
                                    }),
                                )
                                .child(tr_str(lang, "发送", "Send")),
                        ),
                ),
        )
        .into_any_element()
}

fn render_code_inspector_body<V: 'static>(
    props: &DrawerRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let lang = props.language_id;
    let editor_focused = props.active_input == ActiveCodeSceneInput::CodeEditor;
    let cb_focus = on_action.clone();
    let cb_validate = on_action.clone();
    let cb_reload = on_action.clone();
    let cb_copy = on_action.clone();
    let cb_publish = on_action.clone();
    let cb_remix = on_action.clone();

    let (code_display, _) = input_display(
        props.editor_source,
        "// CodeSceneScript (cs1)",
        editor_focused,
        props.ime_preedit,
    );

    let meta_line = props.active_asset.map(|a| {
        let hash = a.content_sha256();
        let short_hash = &hash[..hash.len().min(10)];
        format!(
            "{} · {}×{} @ {}fps · sha256:{}",
            a.code_format, a.manifest.width, a.manifest.height, a.manifest.fps, short_hash
        )
    });

    div()
        .flex_1()
        .min_h(px(0.0))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .when_some(meta_line, |col, m| {
            col.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgba(0xFFFFFF10))
                    .font_family("monospace")
                    .text_size(px(11.0))
                    .text_color(rgba(0x94A3B8FF))
                    .child(m),
            )
        })
        // Editable monospace source editor
        .child(
            div()
                .id("cs-code-editor-box")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(rgba(0x090A10FF))
                .border_1()
                .border_color(if editor_focused {
                    rgba(0xA78BFACC)
                } else {
                    rgba(0xFFFFFF20)
                })
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb_focus(
                            this,
                            DrawerAction::FocusInput(ActiveCodeSceneInput::CodeEditor),
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .font_family("monospace")
                        .text_size(px(11.5))
                        .text_color(rgba(0xE2E8F0FF))
                        .child(code_display),
                ),
        )
        // Validation diagnostic banner
        .when_some(props.editor_validation, |col, rep| {
            let status_txt = if rep.ok {
                format!(
                    "✓ {} ({} frames · peak {} ops)",
                    tr_str(lang, "校验通过", "Validated"),
                    rep.frames_executed,
                    rep.peak_ops_used
                )
            } else {
                rep.diagnostics
                    .first()
                    .map(|d| {
                        format!(
                            "✕ [{}] L{}: {}",
                            d.code,
                            d.line.unwrap_or(0),
                            d.message
                        )
                    })
                    .unwrap_or_else(|| tr_str(lang, "✕ 代码校验失败", "✕ Validation failed").to_string())
            };
            col.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(if rep.ok {
                        rgba(0x10B9812A)
                    } else {
                        rgba(0xEF444433)
                    })
                    .border_1()
                    .border_color(if rep.ok {
                        rgba(0x10B98188)
                    } else {
                        rgba(0xEF444488)
                    })
                    .font_family("monospace")
                    .text_size(px(11.5))
                    .text_color(if rep.ok {
                        rgba(0x6EE7B7FF)
                    } else {
                        rgba(0xFCA5A5FF)
                    })
                    .child(status_txt),
            )
        })
        // Action toolbar
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .child(
                    div()
                        .id("cs-code-validate-btn")
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgba(0xFFFFFF18))
                        .hover(|s| s.bg(rgba(0xFFFFFF2C)))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_validate(this, DrawerAction::ValidateEditorCode, cx);
                            }),
                        )
                        .child(tr_str(lang, "校验代码", "Validate")),
                )
                .child(
                    div()
                        .id("cs-code-hotreload-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgba(0x25F4EEDD))
                        .hover(|s| s.opacity(0.9))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0x090A10FF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_reload(this, DrawerAction::HotReloadEditorCode, cx);
                            }),
                        )
                        .child(tr_str(lang, "▶ 热更新运行", "▶ Hot Reload")),
                )
                .child(
                    div()
                        .id("cs-code-copy-btn")
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgba(0xFFFFFF18))
                        .hover(|s| s.bg(rgba(0xFFFFFF2C)))
                        .text_size(px(12.0))
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_copy(this, DrawerAction::CopyEditorCode, cx);
                            }),
                        )
                        .child(tr_str(lang, "复制", "Copy")),
                )
                .child(
                    div()
                        .id("cs-code-publish-remix-btn")
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgba(0xFE2C55DD))
                        .hover(|s| s.opacity(0.9))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_publish(this, DrawerAction::PublishEditorRemix, cx);
                            }),
                        )
                        .child(tr_str(lang, "发布改版", "Publish Edit")),
                )
                .child(
                    div()
                        .id("cs-code-ai-remix-btn")
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgba(0x7C3AEDDD))
                        .hover(|s| s.opacity(0.9))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_remix(this, DrawerAction::OpenAiRemixFromCurrent, cx);
                            }),
                        )
                        .child(tr_str(lang, "🤖 让 Bot 魔改", "🤖 AI Remix")),
                ),
        )
        .into_any_element()
}

fn render_creator_profile_body<V: 'static>(
    author_user_id: i64,
    author_username: &str,
    props: &DrawerRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let lang = props.language_id;
    let cb_follow = on_action.clone();
    let total_likes: u64 = props.creator_works.iter().map(|a| a.likes_count).sum();

    let cards = props.creator_works.iter().enumerate().map(|(idx, work)| {
        let sid = work.scene_id.clone();
        let cb_open = on_action.clone();
        let badge = if lang.starts_with("zh") {
            work.kind.label_zh()
        } else {
            work.kind.label_en()
        };
        div()
            .id(("cs-creator-work", idx))
            .cursor_pointer()
            .w(px(176.0))
            .p(px(10.0))
            .rounded(px(12.0))
            .bg(rgba(0xFFFFFF10))
            .border_1()
            .border_color(rgba(0xFFFFFF1E))
            .hover(|s| s.bg(rgba(0xFFFFFF20)))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb_open(this, DrawerAction::OpenSceneById(sid.clone()), cx);
                }),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgba(0x25F4EEFF))
                    .child(badge),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgba(0xFFFFFFFF))
                    .child(work.title.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgba(0xFFFFFF88))
                    .child(format!("♥ {} · ▶ {}", work.likes_count, work.plays_count)),
            )
    });

    div()
        .flex_1()
        .min_h(px(0.0))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0xFFFFFFFF))
                                .child(format!("@{}", author_username.trim_start_matches('@'))),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgba(0xFFFFFF99))
                                .child(format!(
                                    "{} {} · {} {}",
                                    props.creator_works.len(),
                                    tr_str(lang, "个代码作品", "scenes"),
                                    total_likes,
                                    tr_str(lang, "获赞", "likes")
                                )),
                        ),
                )
                .child(
                    div()
                        .id("cs-creator-follow-btn")
                        .cursor_pointer()
                        .px(px(14.0))
                        .py(px(7.0))
                        .rounded(px(999.0))
                        .bg(if props.creator_following {
                            rgba(0xFFFFFF22)
                        } else {
                            rgba(0xFE2C55FF)
                        })
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_follow(this, DrawerAction::ToggleFollowCreator(author_user_id), cx);
                            }),
                        )
                        .child(if props.creator_following {
                            tr_str(lang, "已关注", "Following")
                        } else {
                            tr_str(lang, "+ 关注", "+ Follow")
                        }),
                ),
        )
        .child(
            div()
                .id("cs-creator-works-grid")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .flex()
                .flex_wrap()
                .gap(px(10.0))
                .children(cards),
        )
        .into_any_element()
}

fn render_ai_studio_body<V: 'static>(
    props: DrawerRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let lang = props.language_id;
    let is_zh = lang.starts_with("zh");
    let studio = props.studio;
    let cb_clear_remix = on_action.clone();
    let cb_focus_prompt = on_action.clone();
    let cb_gen = on_action.clone();
    let cb_focus_iter = on_action.clone();
    let cb_iter = on_action.clone();
    let cb_focus_title = on_action.clone();
    let cb_focus_tags = on_action.clone();
    let cb_toggle_share = on_action.clone();
    let cb_publish = on_action.clone();

    let bot_pills = OFFICIAL_CODE_SCENE_BOTS.iter().enumerate().map(|(i, bot)| {
        let selected = studio.selected_bot_id == bot.user_id;
        let bot_id = bot.user_id;
        let bot_un = bot.username.to_string();
        let cb = on_action.clone();
        div()
            .id(("cs-studio-bot", i))
            .cursor_pointer()
            .px(px(9.0))
            .py(px(5.0))
            .rounded(px(999.0))
            .bg(if selected {
                rgba(0x7C3AEDDD)
            } else {
                rgba(0xFFFFFF12)
            })
            .border_1()
            .border_color(if selected {
                rgba(0xA78BFAFF)
            } else {
                rgba(0xFFFFFF20)
            })
            .text_size(px(11.5))
            .text_color(rgba(0xFFFFFFFF))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb(this, DrawerAction::StudioSelectBot(bot_id, bot_un.clone()), cx);
                }),
            )
            .child(format!("🤖 @{}", bot.username))
    });

    let kind_pills = AssetKind::ALL.into_iter().map(|kind| {
        let selected = studio.selected_kind == kind;
        let label = if is_zh { kind.label_zh() } else { kind.label_en() };
        let cb = on_action.clone();
        div()
            .id(("cs-studio-kind", kind as usize))
            .cursor_pointer()
            .flex_1()
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(if selected {
                rgba(0xFE2C55DD)
            } else {
                rgba(0xFFFFFF12)
            })
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.0))
            .font_weight(FontWeight::BOLD)
            .text_color(rgba(0xFFFFFFFF))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb(this, DrawerAction::StudioSelectKind(kind), cx);
                }),
            )
            .child(label)
    });

    let presets: [(&str, &str, AssetKind); 4] = [
        ("霓虹赛博雨夜倒计时", "Neon cyberpunk rain countdown video", AssetKind::Video),
        ("极光粒子星云动画", "Aurora nebula particle animation", AssetKind::Animation),
        ("霓虹打砖块小游戏", "Neon brick breaker arcade game", AssetKind::Game),
        ("星际弹幕射击游戏", "Space shooter arcade game", AssetKind::Game),
    ];
    let preset_chips = presets.into_iter().enumerate().map(|(i, (zh, en, kind))| {
        let text = if is_zh { zh } else { en }.to_string();
        let cb = on_action.clone();
        let text_cb = text.clone();
        div()
            .id(("cs-studio-preset", i))
            .cursor_pointer()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(999.0))
            .bg(rgba(0xFFFFFF12))
            .hover(|s| s.bg(rgba(0x25F4EE44)))
            .text_size(px(11.0))
            .text_color(rgba(0x25F4EEFF))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb(this, DrawerAction::StudioPickPreset(text_cb.clone(), kind), cx);
                }),
            )
            .child(format!("⚡ {text}"))
    });

    let (prompt_txt, prompt_ph) = input_display(
        &studio.prompt,
        tr_str(
            lang,
            "告诉 AI Bot 想创作的视频/动画/游戏，例如：做一个樱花粉太空射击小游戏…",
            "Describe the code video, animation, or game you want the AI Bot to build…",
        ),
        props.active_input == ActiveCodeSceneInput::StudioPrompt,
        props.ime_preedit,
    );
    let (iter_txt, iter_ph) = input_display(
        &studio.iterate_instruction,
        tr_str(lang, "多轮微调指令，如：加快速度、换成霓虹紫配色…", "Refinement prompt, e.g. faster tempo, neon violet…"),
        props.active_input == ActiveCodeSceneInput::StudioIterate,
        props.ime_preedit,
    );
    let (title_txt, title_ph) = input_display(
        &studio.title,
        tr_str(lang, "作品标题（自动生成可编辑）", "Scene title (auto-generated, editable)"),
        props.active_input == ActiveCodeSceneInput::StudioTitle,
        props.ime_preedit,
    );
    let (tags_txt, tags_ph) = input_display(
        &studio.tags_input,
        tr_str(lang, "标签，逗号分隔如：neon, game, ai", "Tags separated by commas, e.g. neon, game, ai"),
        props.active_input == ActiveCodeSceneInput::StudioTags,
        props.ime_preedit,
    );

    let has_draft = studio.current_draft.is_some();
    let draft_engine_info = studio.current_draft.as_ref().map(|d| {
        format!(
            "✓ {} · {} · {} frames · peak {} ops",
            d.engine_used,
            d.kind.as_str(),
            d.validation.frames_executed,
            d.validation.peak_ops_used
        )
    });

    div()
        .id("cs-studio-scroll")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Optional Remix Parent Banner
        .when_some(studio.remix_parent_id.as_ref(), |col, pid| {
            let ptitle = studio
                .remix_parent_title
                .as_deref()
                .unwrap_or(pid.as_str());
            col.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgba(0x7C3AED38))
                    .border_1()
                    .border_color(rgba(0xA78BFA77))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgba(0xE9D5FFFF))
                            .child(format!(
                                "✦ {}: {ptitle}",
                                tr_str(lang, "基于源码二创", "Remixing")
                            )),
                    )
                    .child(
                        div()
                            .id("cs-studio-clear-remix")
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgba(0xFFFFFFAA))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_clear_remix(this, DrawerAction::StudioClearRemixParent, cx);
                                }),
                            )
                            .child("×"),
                    ),
            )
        })
        // Bot Picker
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba(0xFFFFFFAA))
                        .child(tr_str(lang, "选择创作 AI Bot", "Select Creator AI Bot")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .children(bot_pills),
                ),
        )
        // Modality Picker
        .child(div().flex().gap(px(6.0)).children(kind_pills))
        // Prompt Input
        .child(
            div()
                .id("cs-studio-prompt-input")
                .min_h(px(62.0))
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(rgba(0xFFFFFF10))
                .border_1()
                .border_color(if props.active_input == ActiveCodeSceneInput::StudioPrompt {
                    rgba(0xFE2C55CC)
                } else {
                    rgba(0xFFFFFF22)
                })
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb_focus_prompt(
                            this,
                            DrawerAction::FocusInput(ActiveCodeSceneInput::StudioPrompt),
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(if prompt_ph {
                            rgba(0xFFFFFF66)
                        } else {
                            rgba(0xFFFFFFFF)
                        })
                        .child(prompt_txt),
                ),
        )
        // Quick Inspiration Chips
        .child(div().flex().flex_wrap().gap(px(6.0)).children(preset_chips))
        // Generate Preview Button
        .child(
            div()
                .id("cs-studio-generate-btn")
                .cursor_pointer()
                .h(px(38.0))
                .rounded(px(10.0))
                .bg(linear_gradient(
                    120.0,
                    linear_color_stop(rgba(0xFE2C55FF), 0.0),
                    linear_color_stop(rgba(0x7C3AEDFF), 1.0),
                ))
                .hover(|s| s.opacity(0.92))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xFFFFFFFF))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb_gen(this, DrawerAction::StudioGenerate, cx);
                    }),
                )
                .child(if studio.is_busy {
                    tr_str(lang, "🤖 AI Bot 正在编写并验证代码…", "🤖 AI Bot is coding & validating…")
                } else {
                    tr_str(lang, "🤖 让 AI Bot 生成代码与实时预览", "🤖 Generate Code & Live Preview")
                }),
        )
        // Status banner
        .when_some(studio.status_message.as_ref(), |col, msg| {
            col.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgba(0x25F4EE22))
                    .text_size(px(11.5))
                    .text_color(rgba(0x25F4EEFF))
                    .child(msg.clone()),
            )
        })
        // Live Draft Preview & Multi-Turn Iteration & Publish Controls
        .when(has_draft, |col| {
            col.when_some(props.studio_preview_el, |col, prev_el| {
                col.child(
                    div()
                        .w_full()
                        .h(px(210.0))
                        .rounded(px(12.0))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgba(0x25F4EE66))
                        .child(prev_el),
                )
            })
            .when_some(draft_engine_info, |col, info| {
                col.child(
                    div()
                        .font_family("monospace")
                        .text_size(px(11.0))
                        .text_color(rgba(0x6EE7B7FF))
                        .child(info),
                )
            })
            // Multi-turn iterate row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("cs-studio-iterate-input")
                            .flex_1()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(rgba(0xFFFFFF10))
                            .border_1()
                            .border_color(
                                if props.active_input == ActiveCodeSceneInput::StudioIterate {
                                    rgba(0x25F4EECC)
                                } else {
                                    rgba(0xFFFFFF20)
                                },
                            )
                            .cursor_text()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_focus_iter(
                                        this,
                                        DrawerAction::FocusInput(ActiveCodeSceneInput::StudioIterate),
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(if iter_ph {
                                        rgba(0xFFFFFF66)
                                    } else {
                                        rgba(0xFFFFFFFF)
                                    })
                                    .child(iter_txt),
                            ),
                    )
                    .child(
                        div()
                            .id("cs-studio-iterate-btn")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(rgba(0x25F4EEDD))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0x090A10FF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_iter(this, DrawerAction::StudioIterate, cx);
                                }),
                            )
                            .child(tr_str(lang, "✨ 迭代", "✨ Iterate")),
                    ),
            )
            // Editable Title
            .child(
                div()
                    .id("cs-studio-title-input")
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(rgba(0xFFFFFF10))
                    .border_1()
                    .border_color(if props.active_input == ActiveCodeSceneInput::StudioTitle {
                        rgba(0xFE2C55CC)
                    } else {
                        rgba(0xFFFFFF20)
                    })
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            cb_focus_title(
                                this,
                                DrawerAction::FocusInput(ActiveCodeSceneInput::StudioTitle),
                                cx,
                            );
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(if title_ph {
                                rgba(0xFFFFFF66)
                            } else {
                                rgba(0xFFFFFFFF)
                            })
                            .child(title_txt),
                    ),
            )
            // Editable Tags
            .child(
                div()
                    .id("cs-studio-tags-input")
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(rgba(0xFFFFFF10))
                    .border_1()
                    .border_color(if props.active_input == ActiveCodeSceneInput::StudioTags {
                        rgba(0xFE2C55CC)
                    } else {
                        rgba(0xFFFFFF20)
                    })
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            cb_focus_tags(
                                this,
                                DrawerAction::FocusInput(ActiveCodeSceneInput::StudioTags),
                                cx,
                            );
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(if tags_ph {
                                rgba(0xFFFFFF66)
                            } else {
                                rgba(0x25F4EEFF)
                            })
                            .child(tags_txt),
                    ),
            )
            // Share to chat checkbox + 1-click Publish CTA
            .child(
                div()
                    .id("cs-studio-share-toggle")
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            cb_toggle_share(this, DrawerAction::StudioToggleShareAfterPublish, cx);
                        }),
                    )
                    .child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(4.0))
                            .bg(if studio.share_to_active_chat {
                                rgba(0x10B981FF)
                            } else {
                                rgba(0xFFFFFF18)
                            })
                            .border_1()
                            .border_color(rgba(0xFFFFFF44))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.0))
                            .text_color(rgba(0xFFFFFFFF))
                            .child(if studio.share_to_active_chat { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgba(0xFFFFFFCC))
                            .child(tr_str(
                                lang,
                                "发布后同步发送互动卡片到当前会话",
                                "Also share interactive card to active chat",
                            )),
                    ),
            )
            .child(
                div()
                    .id("cs-studio-publish-btn")
                    .cursor_pointer()
                    .h(px(40.0))
                    .rounded(px(10.0))
                    .bg(rgba(0x10B981FF))
                    .hover(|s| s.opacity(0.92))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgba(0xFFFFFFFF))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            cb_publish(this, DrawerAction::StudioPublish, cx);
                        }),
                    )
                    .child(tr_str(lang, "🚀 一键发布到代码场景", "🚀 Publish to Code Scene")),
            )
        })
        .into_any_element()
}

fn render_share_sheet_body<V: 'static>(
    props: &DrawerRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DrawerAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let lang = props.language_id;
    let rows = props.share_targets.iter().enumerate().map(|(idx, (chat_id, name, initials, color))| {
        let target_id = *chat_id;
        let cb = on_action.clone();
        div()
            .id(("cs-share-target", idx))
            .cursor_pointer()
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(10.0))
            .bg(rgba(0xFFFFFF0D))
            .hover(|s| s.bg(rgba(0xFFFFFF22)))
            .flex()
            .items_center()
            .justify_between()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb(this, DrawerAction::ShareToTargetChat(target_id), cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded(px(999.0))
                            .bg(rgb(*color))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0xFFFFFFFF))
                            .child(initials.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgba(0xFFFFFFFF))
                            .child(name.clone()),
                    ),
            )
            .child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(999.0))
                    .bg(rgba(0xFE2C55CC))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgba(0xFFFFFFFF))
                    .child(tr_str(lang, "发送", "Send")),
            )
    });

    div()
        .id("cs-share-targets-scroll")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when(props.share_targets.is_empty(), |col| {
            col.child(
                div()
                    .py(px(24.0))
                    .text_size(px(13.0))
                    .text_color(rgba(0xFFFFFF88))
                    .child(tr_str(
                        lang,
                        "暂无可转发的会话，请先打开至少一个聊天会话。",
                        "No chats available to forward to yet.",
                    )),
            )
        })
        .children(rows)
        .into_any_element()
}
