use gpui_telegram_core::core::deep_links::{
    format_apple_deep_link, parse_apple_deep_link, AppleDeepLink, AppleDeepLinkSection,
    DeepLinkContext, DeepLinkResult, DeepLinksAppleDeepLinks, DeepLinksRouterDeepLinks,
};
use std::collections::HashMap;

#[test]
fn test_parse_all_five_apple_sections_without_id() {
    let cases = [
        ("tg://apple/books", AppleDeepLinkSection::Books),
        ("tg://apple/music", AppleDeepLinkSection::Music),
        ("tg://apple/games", AppleDeepLinkSection::Games),
        ("tg://apple/appstore", AppleDeepLinkSection::AppStore),
        ("tg://apple/creations", AppleDeepLinkSection::Creations),
        ("TG://APPLE/BOOKS", AppleDeepLinkSection::Books),
        ("  tg://apple/AppStore  ", AppleDeepLinkSection::AppStore),
    ];

    for (uri, expected_section) in cases {
        let parsed = parse_apple_deep_link(uri)
            .unwrap_or_else(|| panic!("expected valid Apple deep link for {uri}"));
        assert_eq!(parsed.section, expected_section);
        assert_eq!(parsed.creation_id, None);
        assert_eq!(parsed.tab, None);
        assert_eq!(parsed.action, None);

        let canonical = format_apple_deep_link(&parsed);
        assert_eq!(canonical, format!("tg://apple/{}", expected_section.as_slug()));
        assert_eq!(parse_apple_deep_link(&canonical), Some(parsed));
    }
}

#[test]
fn test_parse_with_id_preserves_case_and_canonicalizes_query_order() {
    let uri_ab = "tg://apple/books/cr_AbC-01_x.v2?tab=Discover&action=Read";
    let uri_ba = "TG://APPLE/Books/cr_AbC-01_x.v2?ACTION=read&TAB=discover";

    let parsed_ab = parse_apple_deep_link(uri_ab).expect("valid tab+action");
    let parsed_ba = parse_apple_deep_link(uri_ba).expect("valid action+tab");
    assert_eq!(parsed_ab, parsed_ba);
    assert_eq!(parsed_ab.section, AppleDeepLinkSection::Books);
    assert_eq!(parsed_ab.creation_id.as_deref(), Some("cr_AbC-01_x.v2"));
    assert_eq!(parsed_ab.tab.as_deref(), Some("discover"));
    assert_eq!(parsed_ab.action.as_deref(), Some("read"));

    let formatted = format_apple_deep_link(&parsed_ab);
    assert_eq!(
        formatted,
        "tg://apple/books/cr_AbC-01_x.v2?tab=discover&action=read"
    );
    assert_eq!(parsed_ab.to_uri(), formatted);
    assert_eq!(parsed_ab.to_string(), formatted);
    assert_eq!(parse_apple_deep_link(&formatted), Some(parsed_ab));

    // Sub-tabs used by Apple sections (reading_now, my_creations, installed, arcade)
    let subtab = parse_apple_deep_link("tg://apple/creations?tab=my_creations&action=open")
        .expect("sub-tab valid");
    assert_eq!(subtab.section, AppleDeepLinkSection::Creations);
    assert_eq!(subtab.tab.as_deref(), Some("my_creations"));
    assert_eq!(subtab.action.as_deref(), Some("open"));
    assert_eq!(subtab.to_callback_alias(), None);

    // Benign extra query param passes validation without breaking canonical fields
    let with_extra = parse_apple_deep_link("tg://apple/music/cr_M1?ref=bot_card&action=play")
        .expect("benign extra query param");
    assert_eq!(with_extra.section, AppleDeepLinkSection::Music);
    assert_eq!(with_extra.creation_id.as_deref(), Some("cr_M1"));
    assert_eq!(with_extra.action.as_deref(), Some("play"));
}

#[test]
fn test_parse_callback_aliases_and_roundtrip() {
    let cases = [
        ("creation:open:book:cr_Book_01", AppleDeepLinkSection::Books, "cr_Book_01"),
        ("creation:open:music:cr_Music-02", AppleDeepLinkSection::Music, "cr_Music-02"),
        ("creation:open:game:cr_Game.03", AppleDeepLinkSection::Games, "cr_Game.03"),
        ("creation:open:app:cr_App_04", AppleDeepLinkSection::AppStore, "cr_App_04"),
        ("CREATION:OPEN:GAME:cr_MixCase", AppleDeepLinkSection::Games, "cr_MixCase"),
    ];

    for (alias, expected_section, expected_id) in cases {
        let parsed = parse_apple_deep_link(alias)
            .unwrap_or_else(|| panic!("expected valid alias {alias}"));
        assert_eq!(parsed.section, expected_section);
        assert_eq!(parsed.creation_id.as_deref(), Some(expected_id));
        assert_eq!(parsed.tab, None);
        assert_eq!(parsed.action.as_deref(), Some("open"));

        let canonical = format_apple_deep_link(&parsed);
        assert_eq!(
            canonical,
            format!("tg://apple/{}/{}?action=open", expected_section.as_slug(), expected_id)
        );
        assert_eq!(parse_apple_deep_link(&canonical), Some(parsed.clone()));

        let rebuilt_alias = parsed.to_callback_alias().expect("callback alias");
        assert_eq!(
            rebuilt_alias,
            format!(
                "creation:open:{}:{}",
                expected_section.creation_kind_slug().unwrap(),
                expected_id
            )
        );
    }
}

#[test]
fn test_serde_appstore_slug_and_alias() {
    let link = AppleDeepLink::for_creation(AppleDeepLinkSection::AppStore, "cr_app_9")
        .with_tab("today")
        .with_action("install");
    let json = serde_json::to_string(&link).expect("serialize");
    assert!(json.contains("\"section\":\"appstore\""), "got json: {json}");
    let decoded: AppleDeepLink = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, link);

    let alias_json = r#"{"section":"app_store","creation_id":"cr_app_9"}"#;
    let decoded_alias: AppleDeepLink =
        serde_json::from_str(alias_json).expect("deserialize app_store alias");
    assert_eq!(decoded_alias.section, AppleDeepLinkSection::AppStore);
    assert_eq!(decoded_alias.creation_id.as_deref(), Some("cr_app_9"));
}

#[test]
fn test_router_integration_auth_gating_and_malicious_manual_context() {
    let router = DeepLinksRouterDeepLinks::default();

    // Default router already registers Apple section routes
    assert!(router.routes.contains_key("apple"));
    assert_eq!(router.routes["apple"].len(), 5);

    // Explicit registrar works on clean router too
    let mut custom = DeepLinksRouterDeepLinks {
        routes: HashMap::new(),
    };
    DeepLinksAppleDeepLinks::register(&mut custom);
    assert_eq!(custom.routes["apple"].len(), 5);

    // Auth gating
    let ctx_no_auth = DeepLinksRouterDeepLinks::parse_command("tg://apple/books/cr_1?action=read", false);
    assert_eq!(ctx_no_auth.section, "apple");
    assert_eq!(ctx_no_auth.path, "books/cr_1");
    assert_eq!(ctx_no_auth.param("action"), Some("read"));
    assert_eq!(router.dispatch(&ctx_no_auth), (DeepLinkResult::NeedsAuth, false));
    assert!(!router.try_handle("tg://apple/books/cr_1", false));

    // Authenticated dispatch succeeds for both URI and callback alias
    assert!(router.try_handle("tg://apple/books/cr_1?tab=library&action=read", true));
    assert!(router.try_handle("tg://apple/music", true));
    assert!(router.try_handle("tg://apple/games/cr_game_7?action=play", true));
    assert!(router.try_handle("tg://apple/appstore/cr_app_3?action=install", true));
    assert!(router.try_handle("tg://apple/creations?tab=my_creations", true));
    assert!(router.try_handle("creation:open:music:cr_song_1", true));
    assert!(DeepLinksRouterDeepLinks::parse_apple_deep_link("creation:open:game:cr_g1").is_some());

    // Manually constructed malicious DeepLinkContext must be blocked in dispatch()
    let malicious_contexts = [
        DeepLinkContext {
            section: "apple".to_string(),
            path: "books/../../etc/passwd".to_string(),
            params: HashMap::new(),
            has_active_session: true,
        },
        DeepLinkContext {
            section: "apple".to_string(),
            path: "books/<script>alert(1)</script>".to_string(),
            params: HashMap::new(),
            has_active_session: true,
        },
        DeepLinkContext {
            section: "apple".to_string(),
            path: "books/".to_string(),
            params: HashMap::new(),
            has_active_session: true,
        },
        DeepLinkContext {
            section: "apple".to_string(),
            path: "books/cr_1".to_string(),
            params: HashMap::from([("tab".to_string(), "<script>".to_string())]),
            has_active_session: true,
        },
    ];

    for ctx in &malicious_contexts {
        assert_eq!(
            router.dispatch(ctx),
            (DeepLinkResult::Unsupported, false),
            "malicious context unexpectedly accepted: {ctx:?}"
        );
    }
}

#[test]
fn test_adversarial_apple_deep_link_rejection_matrix() {
    let router = DeepLinksRouterDeepLinks::default();
    let long_id = format!("tg://apple/books/{}", "a".repeat(129));
    let long_uri = format!("tg://apple/books?tab={}", "a".repeat(2050));

    let invalid_vectors: Vec<&str> = vec![
        // Empty / missing sections or IDs
        "",
        "   ",
        "tg://",
        "tg://apple",
        "tg://apple/",
        "tg://apple/books/",
        "tg://apple/books/?tab=discover",
        "tg://apple/books//cr_1",
        "tg:///apple/books",
        "creation:open:",
        "creation:open:book:",
        "creation:open:book",
        // Extra segments
        "tg://apple/books/cr_1/extra",
        "creation:open:book:cr_1:extra",
        "creation:open:book:javascript:alert(1)",
        // Unknown section or creation kind
        "tg://apple/unknown",
        "tg://apple/app_store",
        "creation:open:video:cr_1",
        "creation:open:creations:cr_1",
        // Whitespace, control chars, BiDi overrides (raw and encoded)
        "tg://apple/books/cr 1",
        "tg://apple/books/cr%201",
        "tg://apple/books/cr_1\n",
        "tg://apple/books/cr_1\r\n",
        "tg://apple/books/cr_1\t",
        "tg://apple/books/cr_1\0",
        "tg://apple/books/cr%001",
        "tg://apple/books/\u{202E}cr_1",
        "tg://apple/books/\u{2066}cr_1",
        // Path traversal & encoded traversal
        "tg://apple/books/..",
        "tg://apple/books/.",
        "tg://apple/books/.hidden",
        "tg://apple/books/trailing.",
        "tg://apple/books/a..b",
        "tg://apple/books/../../etc/passwd",
        "tg://apple/books/%2e%2e",
        "tg://apple/books/%252e%252e",
        "tg://apple/books/cr%2f1",
        "tg://apple/books/cr%5c1",
        "tg://apple/books/cr\\1",
        "creation:open:book:../etc",
        "creation:open:book:%2e%2e",
        // XSS, dangerous schemes, fragments
        "tg://apple/books/<script>alert(1)</script>",
        "tg://apple/books/%3cscript%3e",
        "tg://apple/books/javascript:alert(1)",
        "tg://apple/books/vbscript:msgbox(1)",
        "tg://apple/books/data:text/html,hi",
        "http://apple/books",
        "https://apple/books/cr_1",
        "javascript:alert(1)",
        "tg://apple/books/cr_1#frag",
        // Malformed queries, duplicate keys, bad percent escapes
        "tg://apple/books?",
        "tg://apple/books?tab=",
        "tg://apple/books?=val",
        "tg://apple/books?tab",
        "tg://apple/books?tab=discover&&action=open",
        "tg://apple/books?tab=discover&tab=library",
        "tg://apple/books?action=open&ACTION=read",
        "tg://apple/books?foo=<script>",
        "tg://apple/books?x=javascript:1",
        "tg://apple/books?p=../a",
        "tg://apple/books/cr_%ZZ",
        "tg://apple/books/cr_%2",
        "tg://apple/books/cr_%FF",
        "tg://apple/books?tab=%ZZ",
        // Oversized inputs
        long_id.as_str(),
        long_uri.as_str(),
    ];

    for uri in invalid_vectors {
        assert_eq!(
            parse_apple_deep_link(uri),
            None,
            "malicious/invalid URI unexpectedly parsed: {uri:?}"
        );
        let ctx = DeepLinksRouterDeepLinks::parse_command(uri, true);
        let (res, _) = router.dispatch(&ctx);
        assert!(
            matches!(res, DeepLinkResult::Unsupported | DeepLinkResult::NotFound),
            "malicious/invalid URI unexpectedly dispatched as {res:?}: {uri:?}"
        );
        assert!(
            !router.try_handle(uri, true),
            "try_handle unexpectedly returned true for {uri:?}"
        );
    }

    // Boundary check: exact max creation_id length (128) is accepted
    let exact_128 = format!("tg://apple/books/{}", "a".repeat(128));
    let parsed_128 = parse_apple_deep_link(&exact_128).expect("128-char ID should be valid");
    assert_eq!(parsed_128.creation_id.as_deref().map(str::len), Some(128));
}

#[test]
fn test_parser_differential_and_manual_context_hardening() {
    let router = DeepLinksRouterDeepLinks::default();

    // 2,700+ byte schemeless apple URI with many query parameters
    let long_schemeless_query = (0..400)
        .map(|i| format!("k{i}=v"))
        .collect::<Vec<_>>()
        .join("&");
    let oversized_schemeless = format!("apple/books/cr_1?{long_schemeless_query}");
    assert!(oversized_schemeless.len() > 2048);

    let differential_vectors: Vec<&str> = vec![
        "tg://tg://apple/books/cr_1",
        "tg://tg://apple/books/cr_1/",
        "tg://tg://apple/books/cr_1\r\n",
        "tg://tg://apple/books/cr_1?tab=a&tab=b",
        "TG://tg://APPLE/books/cr_1",
        "apple/books/cr_1",
        "apple/books/cr_1/",
        "/apple/books/cr_1",
        "apple/books/cr_1\r\n",
        "apple/books/cr_1?",
        "apple/books/cr_1?tab=discover&&action=open",
        "apple/books/cr_1?tab=a&tab=b",
        oversized_schemeless.as_str(),
    ];

    for uri in differential_vectors {
        assert_eq!(
            parse_apple_deep_link(uri),
            None,
            "parser-differential vector unexpectedly parsed: {uri:?}"
        );
        let ctx = DeepLinksRouterDeepLinks::parse_command(uri, true);
        assert_eq!(
            router.dispatch(&ctx),
            (DeepLinkResult::Unsupported, false),
            "parser-differential vector unexpectedly dispatched: {uri:?}"
        );
        assert!(
            !router.try_handle(uri, true),
            "try_handle unexpectedly accepted parser-differential vector: {uri:?}"
        );
    }

    // Manual DeepLinkContext with duplicate case-insensitive keys rejected
    let dup_case_ctx = DeepLinkContext {
        section: "apple".to_string(),
        path: "books/cr_1".to_string(),
        params: HashMap::from([
            ("tab".to_string(), "discover".to_string()),
            ("TAB".to_string(), "library".to_string()),
        ]),
        has_active_session: true,
    };
    assert_eq!(
        router.dispatch(&dup_case_ctx),
        (DeepLinkResult::Unsupported, false)
    );

    // Manual DeepLinkContext with oversized params (> 2048 bytes total) rejected
    let mut oversized_params = HashMap::new();
    for i in 0..80 {
        oversized_params.insert(format!("param_key_{i:03}"), "value_1234567890".to_string());
    }
    let oversized_ctx = DeepLinkContext {
        section: "apple".to_string(),
        path: "books/cr_1".to_string(),
        params: oversized_params,
        has_active_session: true,
    };
    assert_eq!(
        router.dispatch(&oversized_ctx),
        (DeepLinkResult::Unsupported, false)
    );

    // Legacy non-Apple routes remain 100% intact
    assert!(router.try_handle("tg://chats/search", true));
    assert!(router.try_handle("tg://settings/privacy", true));
    let long_legacy = format!("tg://resolve?domain={}", "a".repeat(65_536));
    let legacy_ctx = DeepLinksRouterDeepLinks::parse_command(&long_legacy, true);
    assert_eq!(legacy_ctx.section, "resolve");
    assert_eq!(legacy_ctx.param("domain").map(str::len), Some(65_536));
}
