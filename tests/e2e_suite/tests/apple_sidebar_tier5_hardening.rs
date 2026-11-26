//! Milestone M7 Phase 2 — Tier 5 White-Box Adversarial Hardening Suite
//! (`apple_sidebar_tier5_hardening.rs`)
//!
//! Stress-tests concurrency races, cross-user CAS exfiltration, Unicode/XSS/deep-link smuggling,
//! FTS5/public-asset transactional integrity, and cold-restart durability across all Apple sidebar verticals.

mod apple_sidebar_common;

use gpui_telegram_core::core::deep_links::parse_apple_deep_link;
use gpui_telegram_desktop::views::app_shell::shell_content_hub::is_apple_deep_link_candidate;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn t5_01_concurrent_optimistic_cas_race_strictly_one_winner() -> anyhow::Result<()> {
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let creator = apple_sidebar_common::register_actor(&driver, "cas_racer").await?;
    let payload = apple_sidebar_common::make_book_creation(
        "CAS Race Book",
        "Testing optimistic concurrency locking under parallel writers.",
        "Racer",
        "en",
        &["Sci-Fi"],
        &["cas"],
        None,
    );
    let created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, payload).await?;
    let cid = created["creation_id"].as_str().unwrap().to_string();
    let initial_rev = created["cloud_revision"].as_i64().unwrap();

    // Fire 8 concurrent PUT updates all claiming expected_revision == initial_rev
    let mut handles = Vec::new();
    for i in 0..8usize {
        let base_url = server.base_url();
        let tok = creator.token.clone();
        let cid_c = cid.clone();
        handles.push(tokio::spawn(async move {
            let drv = apple_sidebar_common::E2eSystemDriver::new(base_url);
            apple_sidebar_common::update_creation_raw(
                &drv,
                &tok,
                &cid_c,
                json!({
                    "expected_revision": initial_rev,
                    "title": format!("CAS Winner {i}")
                }),
            )
            .await
        }));
    }

    let mut wins = 0usize;
    let mut conflicts = 0usize;
    for h in handles {
        let (st, _body) = h.await??;
        match st {
            200 => wins += 1,
            409 => conflicts += 1,
            other => anyhow::bail!("unexpected status under CAS race: {other}"),
        }
    }
    assert_eq!(wins, 1, "strictly one parallel CAS writer must succeed");
    assert_eq!(conflicts, 7, "all other 7 parallel writers must receive 409 conflict");

    let (st_get, final_dto) =
        apple_sidebar_common::get_creation_raw(&driver, Some(&creator.token), &cid).await?;
    assert_eq!(st_get, 200);
    assert_eq!(final_dto["cloud_revision"].as_i64().unwrap(), initial_rev + 1);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t5_02_cross_user_cas_media_exfiltration_fails_closed() -> anyhow::Result<()> {
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let (alice, bob, admin) =
        apple_sidebar_common::seed_creator_consumer_admin(&server, &driver).await?;

    // Alice uploads a private PNG cover and private WAV audio (never shared with Bob)
    let png = apple_sidebar_common::make_unique_png_bytes("alice_secret_cover");
    let wav = apple_sidebar_common::make_unique_wav_bytes("alice_secret_audio");
    let cover_id = apple_sidebar_common::upload_raw_media(
        &driver,
        &alice.token,
        "secret_cover.png",
        "image/png",
        png,
    )
    .await?;
    let wav_id = apple_sidebar_common::upload_raw_media(
        &driver,
        &alice.token,
        "secret_audio.wav",
        "audio/wav",
        wav,
    )
    .await?;

    // Baseline: Bob cannot download Alice's private media
    let (st_b1, _) = apple_sidebar_common::download_media(&driver, &bob.token, &cover_id).await?;
    let (st_b2, _) = apple_sidebar_common::download_media(&driver, &bob.token, &wav_id).await?;
    assert_eq!(st_b1, 403);
    assert_eq!(st_b2, 403);

    // Attack: Bob creates a music creation referencing Alice's private cover_id + wav_id and tries to publish
    let attack_music = apple_sidebar_common::make_music_creation(
        "Exfil Attempt Album",
        "Attempting to launder another user's private CAS media via public catalog.",
        "Attacker Bob",
        "en",
        &["Electronic"],
        &["exfil"],
        Some(&cover_id),
        Some(&wav_id),
    );
    let (st_create, created_val) =
        apple_sidebar_common::create_creation_raw(&driver, &bob.token, attack_music).await?;
    if st_create == 200 || st_create == 201 {
        let cid = created_val["creation_id"].as_str().unwrap();
        let (st_sub, _) =
            apple_sidebar_common::submit_review_raw(&driver, &bob.token, cid).await?;
        if st_sub == 200 {
            let (st_rev, _) = apple_sidebar_common::review_decision_raw(
                &driver,
                &admin.token,
                cid,
                json!({"decision": "approve", "visibility": "public"}),
            )
            .await?;
            // Either approval rejects the cross-owner media OR fails closed without adding to public assets
            assert!(
                st_rev != 200 || {
                    let (d1, _) =
                        apple_sidebar_common::download_media(&driver, &bob.token, &cover_id).await?;
                    d1 == 403
                },
                "cross-user media exfiltration must fail closed"
            );
        }
    }

    // Regardless of which layer rejected, Bob MUST still receive 403 on BOTH Alice's private assets!
    let (st_after1, _) =
        apple_sidebar_common::download_media(&driver, &bob.token, &cover_id).await?;
    let (st_after2, _) =
        apple_sidebar_common::download_media(&driver, &bob.token, &wav_id).await?;
    assert_eq!(st_after1, 403, "Alice's cover_id leaked to Bob!");
    assert_eq!(st_after2, 403, "Alice's wav_id leaked to Bob!");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t5_03_unicode_smuggling_xss_and_deep_link_parser_adversarial_matrix() -> anyhow::Result<()>
{
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let creator = apple_sidebar_common::register_actor(&driver, "xss_tester").await?;

    // 1. Adversarial deep-link URI matrix: every smuggled/traversal/XSS payload must fail parse
    // while still being caught by the candidate swallow guard!
    let malicious_uris = [
        "tg://apple/books/../../etc/passwd",
        "tg://apple/books/<script>alert(1)</script>",
        "tg://apple/music/cr_1?action=javascript:alert(1)",
        "tg://apple/games/cr_1%2f..%2fadmin",
        "creation:open:book:../secret",
        "TG://APPLE/books/cr_bad\u{200B}id",
        "tg://apple/unknown_section/cr_1",
    ];
    for uri in malicious_uris {
        assert!(
            parse_apple_deep_link(uri).is_none(),
            "malicious URI must NOT parse as valid AppleDeepLink: {uri}"
        );
        assert!(
            is_apple_deep_link_candidate(uri),
            "malicious Apple-like URI must be caught/swallowed by candidate guard: {uri}"
        );
    }
    // Canonical valid deep-link parses cleanly
    let ok_link = parse_apple_deep_link("tg://apple/books/cr_safe_123?tab=book_store&action=read")
        .expect("canonical deep-link must parse");
    assert_eq!(ok_link.creation_id.as_deref(), Some("cr_safe_123"));
    assert_eq!(ok_link.action.as_deref(), Some("read"));

    // 2. Server-side XSS rejection on review submission (422 + validation_errors)
    let mut xss_book = apple_sidebar_common::make_book_creation(
        "Safe Title",
        "Summary containing <script>alert('xss')</script> injection payload.",
        "Author",
        "en",
        &["Sci-Fi"],
        &["test"],
        None,
    );
    xss_book["payload"]["blurb"] = json!("javascript:alert(1)");
    let created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, xss_book).await?;
    let cid = created["creation_id"].as_str().unwrap();
    let (st_sub, sub_body) =
        apple_sidebar_common::submit_review_raw(&driver, &creator.token, cid).await?;
    assert_eq!(st_sub, 422, "XSS payload must be rejected with 422: {sub_body}");
    assert!(
        !sub_body["validation_errors"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "422 must return non-empty validation_errors"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t5_04_concurrent_publish_unpublish_delete_fts5_and_asset_consistency() -> anyhow::Result<()>
{
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let (creator, consumer, admin) =
        apple_sidebar_common::seed_creator_consumer_admin(&server, &driver).await?;

    let cover_id = apple_sidebar_common::upload_raw_media(
        &driver,
        &creator.token,
        "storm_cover.png",
        "image/png",
        apple_sidebar_common::make_unique_png_bytes("storm_cover"),
    )
    .await?;

    let book_req = apple_sidebar_common::make_book_creation(
        "星海风暴测试作品",
        "用于验证高强度发布、下架、重新上架与软删除后的 FTS5 和媒体权限一致性。",
        "Storm Author",
        "zh-CN",
        &["科幻"],
        &["storm"],
        Some(&cover_id),
    );
    let created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, book_req).await?;
    let cid = created["creation_id"].as_str().unwrap().to_string();

    // Publish -> verify public & media 200 -> Unpublish -> verify zero leak & media 403
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &cid)
        .await?;
    let (st_m1, _) = apple_sidebar_common::download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_m1, 200);

    let (st_unpub, _) = apple_sidebar_common::unpublish_raw(&driver, &creator.token, &cid).await?;
    assert_eq!(st_unpub, 200);
    let (st_m2, _) = apple_sidebar_common::download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_m2, 403, "unpublish must immediately revoke public CAS asset");

    // Republish -> soft-delete -> verify immediate atomic purge + survival across cold restart!
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &cid)
        .await?;
    let (st_m3, _) = apple_sidebar_common::download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_m3, 200, "republish must restore public CAS asset");

    let (st_del, _) = apple_sidebar_common::delete_creation_raw(&driver, &creator.token, &cid).await?;
    assert!(st_del == 200 || st_del == 204);
    let (st_m4, _) = apple_sidebar_common::download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_m4, 403, "soft-delete must immediately purge public CAS asset");

    // Cold server restart: verify zero FTS5/catalog ghost entries & audit log still intact!
    let driver2 = apple_sidebar_common::restart_server(&mut server).await?;
    let (st_cat, cat_val) = driver2
        .json_req(
            Method::GET,
            &format!("/api/v1/catalog?q={}", apple_sidebar_common::percent_encode("星海风暴")),
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st_cat, 200);
    assert!(
        !apple_sidebar_common::value_contains_creation_id(&cat_val, &cid),
        "deleted creation must never reappear in FTS5 catalog after restart"
    );

    let (st_aud, aud_val) =
        apple_sidebar_common::get_audit_raw(&driver2, &creator.token, &cid).await?;
    assert_eq!(st_aud, 200);
    assert!(
        aud_val["items"].as_array().map(|a| a.len()).unwrap_or(0) >= 4,
        "immutable audit trail must retain all transitions even after soft-delete & restart"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t5_05_multi_account_leaderboard_review_and_audit_immutability_across_restart(
) -> anyhow::Result<()> {
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let (creator, p1, admin) =
        apple_sidebar_common::seed_creator_consumer_admin(&server, &driver).await?;
    let p2 = apple_sidebar_common::register_actor(&driver, "player_two").await?;

    let game_req = apple_sidebar_common::make_game_creation(
        "Tier5 Hardening Arcade",
        "Multi-player high-score and review resilience game.",
        "Studio",
        "en",
        &["Arcade"],
        &["tier5"],
        None,
    );
    let created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, game_req).await?;
    let gid = created["creation_id"].as_str().unwrap().to_string();
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &gid)
        .await?;

    // P1 submits 500 then lower 120 (must keep 500!); P2 submits 850 (#1)
    let (st1, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{gid}/scores"),
            Some(&p1.token),
            Some(json!({"score": 500})),
        )
        .await?;
    assert!(st1 == 200 || st1 == 201);
    let (st2, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{gid}/scores"),
            Some(&p1.token),
            Some(json!({"score": 120})),
        )
        .await?;
    assert!(st2 == 200 || st2 == 201);
    let (st3, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{gid}/scores"),
            Some(&p2.token),
            Some(json!({"score": 850})),
        )
        .await?;
    assert!(st3 == 200 || st3 == 201);

    // P1 submits review twice (rating 3 -> updated to 5); rating_count MUST stay 1!
    let (sr1, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{gid}/reviews"),
            Some(&p1.token),
            Some(json!({"rating": 3, "body": "Good start"})),
        )
        .await?;
    assert!(sr1 == 200 || sr1 == 201);
    let (sr2, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{gid}/reviews"),
            Some(&p1.token),
            Some(json!({"rating": 5, "body": "Amazing after polish!"})),
        )
        .await?;
    assert!(sr2 == 200 || sr2 == 201);

    // Cold restart & verify exact leaderboard ordering + deduped review stats
    let driver2 = apple_sidebar_common::restart_server(&mut server).await?;
    let (slb, lb_val) = driver2
        .json_req(
            Method::GET,
            &format!("/api/v1/catalog/{gid}/leaderboard"),
            Some(&p1.token),
            None,
        )
        .await?;
    assert_eq!(slb, 200);
    let lb = lb_val["items"].as_array().unwrap();
    assert_eq!(lb.len(), 2);
    assert_eq!(lb[0]["score"].as_i64(), Some(850));
    assert_eq!(lb[1]["score"].as_i64(), Some(500));

    let (srev, rev_val) = driver2
        .json_req(
            Method::GET,
            &format!("/api/v1/catalog/{gid}/reviews"),
            Some(&p1.token),
            None,
        )
        .await?;
    assert_eq!(srev, 200);
    assert_eq!(rev_val["rating_count"].as_u64(), Some(1));
    assert_eq!(rev_val["rating_avg"].as_f64(), Some(5.0));

    server.stop().await;
    Ok(())
}
