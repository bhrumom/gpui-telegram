mod apple_sidebar_common;

use apple_sidebar_common::*;
use reqwest::Method;
use serde_json::json;

// ============================================================================
// Tier 4 — Real-World Multi-User Scenarios & Cold Restart Durability (7 tests)
// ============================================================================

#[tokio::test]
async fn t4_01_author_bot_book_publish_reader_progress_review_survives_cold_restart() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (author, reader, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    // 1. Author writes a Book via @CreatorStudioBot + keeps a second private cloud draft
    let author_dm = open_creator_bot_dm(&driver, &author.token).await?;
    let _ = send_bot_command_and_poll_reply(
        &driver,
        &author.token,
        author_dm,
        "/newbook Glass Archivist | A literary sci-fi novel about cloud libraries",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=book", Some(&author.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let mut bot_book = extract_items(&my_val)?[0].clone();
    let pub_id = bot_book["creation_id"].as_str().unwrap().to_string();
    let rev = bot_book["cloud_revision"].as_i64().unwrap();

    // Author polishes via cloud edit, then submits & admin approves
    bot_book["title"] = json!("Glass Archivist: Director Cut");
    bot_book["expected_revision"] = json!(rev);
    assert_eq!(update_creation_raw(&driver, &author.token, &pub_id, bot_book).await?.0, 200);
    publish_creation_public(&driver, &author.token, &admin.token, &pub_id).await?;

    // Second private draft stays in author's cloud
    let priv_draft = create_creation_ok(
        &driver,
        &author.token,
        make_book_creation("Sequel Draft Secret", "Private outline saved in author cloud only.", "Author", "en-US", &["Sci-Fi"], &["sequel"], None),
    )
    .await?;
    let draft_id = priv_draft["creation_id"].as_str().unwrap().to_string();

    // 2. Reader discovers in Apple Books, saves reading progress/bookmarks, and posts 5-star review
    let (st_cat, cat_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/books?q=Archivist", Some(&reader.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    assert!(value_contains_creation_id(&cat_val, &pub_id));

    assert_eq!(
        driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{pub_id}/state"),
                Some(&reader.token),
                Some(json!({
                    "in_library": true,
                    "progress_percent": 78.0,
                    "state_json": {
                        "current_chapter_index": 1,
                        "current_page_index": 4,
                        "progress_percent": 78.0,
                        "reader_theme": "night",
                        "bookmarks": [
                            {
                                "bookmark_id": "bm_cold_1",
                                "chapter_index": 1,
                                "page_index": 4,
                                "note": "Favorite quote",
                                "created_at_ms": 1700000000000i64
                            }
                        ],
                        "in_library": true
                    }
                })),
            )
            .await?
            .0,
        200
    );
    let (st_rev, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{pub_id}/reviews"),
            Some(&reader.token),
            Some(json!({ "rating": 5, "title": "Timeless", "body": "Unforgettable world-building." })),
        )
        .await?;
    assert!(st_rev == 200 || st_rev == 201);

    // 3. Cold restart server on same SQLite WAL / CAS dir -> refresh driver for new port!
    let driver = restart_server(&mut server).await?;

    // 4. Verify cloud drafts, published catalog, reading progress, bookmarks, reviews & audit survive
    let (st_a_my, a_my) = driver.json_req(Method::GET, "/api/v1/creations/my", Some(&author.token), None).await?;
    assert_eq!(st_a_my, 200);
    assert!(value_contains_creation_id(&a_my, &pub_id));
    assert!(value_contains_creation_id(&a_my, &draft_id));

    let (st_cat2, cat2) = driver.json_req(Method::GET, "/api/v1/catalog/books?q=Archivist", Some(&reader.token), None).await?;
    assert_eq!(st_cat2, 200);
    let item2 = extract_items(&cat2)?.iter().find(|i| i["creation_id"] == pub_id).expect("published book survived restart");
    assert_eq!(item2["title"], "Glass Archivist: Director Cut");
    assert_eq!(item2["rating_count"].as_u64(), Some(1));

    // Author's private draft MUST remain strictly hidden from reader AFTER cold restart
    let (st_priv_get, priv_get_val) = get_creation_raw(&driver, Some(&reader.token), &draft_id).await?;
    assert!(
        matches!(st_priv_get, 403 | 404),
        "private draft must remain denied (403/404) to reader after restart, got {st_priv_get}: {priv_get_val}"
    );
    for cat_path in ["/api/v1/catalog/books", "/api/v1/catalog/books?q=Sequel"] {
        let (st_c, c_val) = driver.json_req(Method::GET, cat_path, Some(&reader.token), None).await?;
        assert_eq!(st_c, 200);
        let entries = extract_surface_entries(&c_val)?;
        assert!(
            !entries.iter().any(|e| {
                value_contains_creation_id(e, &draft_id)
                    || value_contains_substring(e, "Sequel Draft Secret")
            }),
            "private draft leaked in `{cat_path}` after cold restart: {c_val}"
        );
    }

    let (st_st2, st2) = driver.json_req(Method::GET, "/api/v1/catalog/my-states", Some(&reader.token), None).await?;
    assert_eq!(st_st2, 200);
    let r_entry = extract_items(&st2)?
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(pub_id.as_str()))
        .expect("reader book state survived restart");
    assert_eq!(r_entry["in_library"], true);
    assert!((r_entry["progress_percent"].as_f64().unwrap_or(0.0) - 78.0).abs() < 0.5);
    assert!(value_contains_substring(&r_entry["state_json"], "bm_cold_1"));

    let (st_revs, revs2) = driver.json_req(Method::GET, &format!("/api/v1/catalog/{pub_id}/reviews"), Some(&reader.token), None).await?;
    assert_eq!(st_revs, 200);
    assert_eq!(extract_items(&revs2)?.len(), 1);

    let (st_aud, aud2) = get_audit_raw(&driver, &author.token, &pub_id).await?;
    assert_eq!(st_aud, 200);
    assert!(extract_items(&aud2)?.len() >= 2);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t4_02_composer_music_cas_wav_stream_bot_recommend_survives_cold_restart() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (composer, listener, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let third_user = register_actor(&driver, "music_fan3").await?;

    let wav_bytes = make_unique_wav_bytes("t4_02_symphony");
    let wav_id = upload_raw_media(&driver, &composer.token, "starlight_ep.wav", "audio/wav", wav_bytes.clone()).await?;

    // Composer creates via @CreatorStudioBot then attaches CAS WAV audio track
    let comp_dm = open_creator_bot_dm(&driver, &composer.token).await?;
    let _ = send_bot_command_and_poll_reply(
        &driver,
        &composer.token,
        comp_dm,
        "/newmusic Starlight Glacier | Ethereal ambient synth suite with synced lyrics",
    )
    .await?;

    let (st_my_music, my_music) = driver.json_req(Method::GET, "/api/v1/creations/my?kind=music", Some(&composer.token), None).await?;
    assert_eq!(st_my_music, 200, "list composer music failed: {my_music}");
    let mut track = extract_items(&my_music)?[0].clone();
    let mid = track["creation_id"].as_str().unwrap().to_string();
    let rev = track["cloud_revision"].as_i64().unwrap();
    track["payload"]["tracks"][0]["audio_media_file_id"] = json!(wav_id);
    track["expected_revision"] = json!(rev);
    assert_eq!(update_creation_raw(&driver, &composer.token, &mid, track).await?.0, 200);

    // Non-owner blocked before publish
    assert_eq!(download_media(&driver, &listener.token, &wav_id).await?.0, 403);
    publish_creation_public(&driver, &composer.token, &admin.token, &mid).await?;

    // Listener discovers, streams WAV, loves & saves playback position
    let (st_cat, cat_val) = driver.json_req(Method::GET, "/api/v1/catalog/music?q=Glacier", Some(&listener.token), None).await?;
    assert_eq!(st_cat, 200);
    assert!(value_contains_creation_id(&cat_val, &mid));

    let (st_wav, got_wav) = download_media(&driver, &listener.token, &wav_id).await?;
    assert_eq!(st_wav, 200);
    assert_eq!(got_wav, wav_bytes);

    assert_eq!(
        driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{mid}/state"),
                Some(&listener.token),
                Some(json!({
                    "in_library": true,
                    "favorite": true,
                    "progress_percent": 55.0,
                    "state_json": {
                        "loved": true,
                        "in_library": true,
                        "last_track_id": "trk_01",
                        "last_position_ms": 23100,
                        "play_count": 3
                    }
                })),
            )
            .await?
            .0,
        200
    );

    // Cold restart server -> verify WAV stream, listener state, and third-user bot /recommend survive!
    let driver = restart_server(&mut server).await?;

    let (st_wav2, got_wav2) = download_media(&driver, &listener.token, &wav_id).await?;
    assert_eq!(st_wav2, 200);
    assert_eq!(got_wav2, wav_bytes);

    let (st_ls, ls_val) = driver.json_req(Method::GET, "/api/v1/catalog/my-states", Some(&listener.token), None).await?;
    assert_eq!(st_ls, 200);
    let ls_entry = extract_items(&ls_val)?
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(mid.as_str()))
        .expect("listener music state survived cold restart");
    assert_eq!(ls_entry["in_library"], true);
    assert_eq!(ls_entry["favorite"], true);
    assert!((ls_entry["progress_percent"].as_f64().unwrap_or(0.0) - 55.0).abs() < 0.5);
    assert_eq!(
        ls_entry["state_json"]["last_position_ms"].as_u64(),
        Some(23100),
        "expected persisted integer last_position_ms == 23100: {ls_entry}"
    );

    let u3_dm = open_creator_bot_dm(&driver, &third_user.token).await?;
    let rec_reply = send_bot_command_and_poll_reply(&driver, &third_user.token, u3_dm, "/recommend Glacier").await?;
    let links = extract_apple_deep_links_from_message(&rec_reply);
    assert!(
        links.iter().any(|l| l.starts_with(&format!("tg://apple/music/{mid}"))),
        "post-restart bot /recommend must include tg://apple/music/{mid}: {rec_reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t4_03_game_creator_three_player_leaderboard_and_saves_survive_cold_restart() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, p1, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let p2 = register_actor(&driver, "arcade_p2").await?;
    let p3 = register_actor(&driver, "arcade_p3").await?;

    let dev_dm = open_creator_bot_dm(&driver, &creator.token).await?;
    let _ = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        dev_dm,
        "/newgame Hypernova Arcade | Competitive arcade trivia & puzzle sprint",
    )
    .await?;

    let (st_my_games, my_games) = driver.json_req(Method::GET, "/api/v1/creations/my?kind=game", Some(&creator.token), None).await?;
    assert_eq!(st_my_games, 200, "list creator games failed: {my_games}");
    let gid = extract_items(&my_games)?[0]["creation_id"].as_str().unwrap().to_string();
    publish_creation_public(&driver, &creator.token, &admin.token, &gid).await?;

    for (player, score, save_lvl) in [(&p1, 1550, "stage_3"), (&p2, 3200, "stage_7"), (&p3, 2100, "stage_5")] {
        let (st_sc, _) = driver
            .json_req(Method::POST, &format!("/api/v1/catalog/{gid}/scores"), Some(&player.token), Some(json!({ "score": score })))
            .await?;
        assert!(st_sc == 200 || st_sc == 201);
        let (st_st, _) = driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{gid}/state"),
                Some(&player.token),
                Some(json!({
                    "in_library": true,
                    "state_json": {
                        "high_score": score,
                        "save_state_json": { "current_level_id": save_lvl },
                        "in_library": true
                    }
                })),
            )
            .await?;
        assert_eq!(st_st, 200);
    }

    let driver = restart_server(&mut server).await?;

    let (st_lb, lb_body) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{gid}/leaderboard"), Some(&p1.token), None)
        .await?;
    assert_eq!(st_lb, 200);
    let lb = extract_items(&lb_body)?;
    assert_eq!(lb.len(), 3);
    assert_eq!(lb[0]["user_id"].as_u64(), Some(p2.user_id));
    assert_eq!(lb[1]["user_id"].as_u64(), Some(p3.user_id));
    assert_eq!(lb[2]["user_id"].as_u64(), Some(p1.user_id));
    assert_eq!(lb[0]["best_score"].as_i64().or_else(|| lb[0]["score"].as_i64()), Some(3200));

    let (st_s3, s3_body) = driver.json_req(Method::GET, "/api/v1/catalog/my-states", Some(&p3.token), None).await?;
    assert_eq!(st_s3, 200);
    let s3_entry = extract_items(&s3_body)?
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(gid.as_str()))
        .expect("player3 game state survived restart");
    assert_eq!(s3_entry["in_library"], true);
    assert_eq!(s3_entry["state_json"]["high_score"].as_i64(), Some(2100));
    assert!(value_contains_substring(&s3_entry["state_json"], "stage_5"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t4_04_multi_vertical_creator_all_four_catalogs_appstore_merge_and_bilingual_bot() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Aurora Odyssey 星辰远征", "Bilingual sci-fi epic 星辰远征 for Apple Books.", "StudioX", "zh-CN", &["Sci-Fi", "科幻"], &["Aurora", "星辰"], None),
    )
    .await?;
    let music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Aurora Nocturne 极光夜曲", "Bilingual ambient soundtrack 极光夜曲 for Apple Music.", "StudioX", "zh-CN", &["Ambient", "电子"], &["Aurora", "极光"], None, None),
    )
    .await?;
    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Aurora Quest 极光试炼", "Bilingual puzzle trivia 极光试炼 for Apple Games.", "StudioX", "zh-CN", &["Trivia", "益智"], &["Aurora", "试炼"], None),
    )
    .await?;
    let app = create_creation_ok(
        &driver,
        &creator.token,
        make_app_creation("Aurora Studio 极光工坊", "Bilingual utility app 极光工坊 for Apple App Store.", "StudioX", "zh-CN", &["Productivity", "效率"], &["Aurora", "工坊"], None, None),
    )
    .await?;

    let bid = book["creation_id"].as_str().unwrap();
    let mid = music["creation_id"].as_str().unwrap();
    let gid = game["creation_id"].as_str().unwrap();
    let aid = app["creation_id"].as_str().unwrap();
    for id in [bid, mid, gid, aid] {
        publish_creation_public(&driver, &creator.token, &admin.token, id).await?;
    }

    let bot_uname = format!("auroramini_{}_bot", unique_suffix());
    let (_tok, miniapp) = publish_miniapp(&driver, &creator.token, &bot_uname, "aurora_mini", "Aurora Companion MiniApp").await?;
    let mini_id = miniapp["app_id"].as_str().unwrap();

    for (path, expected_id) in [
        ("/api/v1/catalog/books", bid),
        ("/api/v1/catalog/music", mid),
        ("/api/v1/catalog/games", gid),
        ("/api/v1/catalog/appstore", aid),
    ] {
        let (st, body) = driver.json_req(Method::GET, path, Some(&consumer.token), None).await?;
        assert_eq!(st, 200, "vertical {path} failed: {body}");
        assert!(value_contains_creation_id(&body, expected_id), "{expected_id} missing from {path}: {body}");
    }

    let (st_store, store) = driver.json_req(Method::GET, "/api/v1/catalog/appstore", Some(&consumer.token), None).await?;
    assert_eq!(st_store, 200, "catalog/appstore failed: {store}");
    assert!(
        value_contains_substring(&store, mini_id)
            || value_contains_substring(&store, &bot_uname)
            || value_contains_substring(&store, "Aurora Companion MiniApp"),
        "MiniApp missing from unified /catalog/appstore: {store}"
    );

    let dm = open_creator_bot_dm(&driver, &consumer.token).await?;
    let en_rec = send_bot_command_and_poll_reply(&driver, &consumer.token, dm, "/recommend Aurora").await?;
    assert!(!extract_apple_deep_links_from_message(&en_rec).is_empty(), "EN bot /recommend missing tg://apple/* deep-links: {en_rec}");

    let zh_rec = send_bot_command_and_poll_reply(&driver, &consumer.token, dm, "/recommend 极光").await?;
    assert!(!extract_apple_deep_links_from_message(&zh_rec).is_empty(), "ZH bot /recommend missing tg://apple/* deep-links: {zh_rec}");

    let (st_iq, iq_zh) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query={}&limit=10", percent_encode("星辰")),
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st_iq, 200);
    assert!(value_contains_creation_id(&iq_zh, bid) || value_contains_substring(&iq_zh, "星辰远征"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t4_05_two_accounts_isolated_cloud_revisions_drafts_and_states_across_relogin_restart() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let acc_a = register_actor(&driver, "acct_alpha").await?;
    let acc_b = register_actor(&driver, "acct_beta").await?;
    let admin = register_actor(&driver, "acct_admin").await?;
    promote_user_to_admin(&server, admin.user_id)?;

    let a_draft = create_creation_ok(
        &driver,
        &acc_a.token,
        make_book_creation("Alpha Private Draft", "Only visible to Account Alpha.", "Alpha", "en-US", &["Fiction"], &["alpha"], None),
    )
    .await?;
    let b_draft = create_creation_ok(
        &driver,
        &acc_b.token,
        make_music_creation("Beta Private Track", "Only visible to Account Beta.", "Beta", "en-US", &["Pop"], &["beta"], None, None),
    )
    .await?;
    let shared_book = create_creation_ok(
        &driver,
        &acc_a.token,
        make_book_creation("Shared Public Novel", "Public novel with independent per-account reading progress.", "Alpha", "en-US", &["Fiction"], &["shared"], None),
    )
    .await?;
    let a_id = a_draft["creation_id"].as_str().unwrap().to_string();
    let b_id = b_draft["creation_id"].as_str().unwrap().to_string();
    let s_id = shared_book["creation_id"].as_str().unwrap().to_string();
    publish_creation_public(&driver, &acc_a.token, &admin.token, &s_id).await?;

    assert_eq!(
        driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{s_id}/state"),
                Some(&acc_a.token),
                Some(json!({ "in_library": true, "progress_percent": 25.0, "state_json": { "marker": "alpha_pos" } })),
            )
            .await?
            .0,
        200
    );
    assert_eq!(
        driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{s_id}/state"),
                Some(&acc_b.token),
                Some(json!({ "in_library": true, "progress_percent": 90.0, "state_json": { "marker": "beta_pos" } })),
            )
            .await?
            .0,
        200
    );

    // Cold restart + fresh POST /api/v1/auth/login for both accounts
    let driver = restart_server(&mut server).await?;
    let a_relogin = login_actor(&driver, &acc_a.username, DEFAULT_PASS).await?;
    let b_relogin = login_actor(&driver, &acc_b.username, DEFAULT_PASS).await?;

    let (st_a, my_a) = driver.json_req(Method::GET, "/api/v1/creations/my", Some(&a_relogin.token), None).await?;
    let (st_b, my_b) = driver.json_req(Method::GET, "/api/v1/creations/my", Some(&b_relogin.token), None).await?;
    assert_eq!(st_a, 200);
    assert_eq!(st_b, 200);
    assert!(value_contains_creation_id(&my_a, &a_id) && !value_contains_creation_id(&my_a, &b_id));
    assert!(value_contains_creation_id(&my_b, &b_id) && !value_contains_creation_id(&my_b, &a_id));

    // Verify /creations/sync isolation across both accounts after cold restart
    let (st_sync_a, sync_a) = driver
        .json_req(Method::GET, "/api/v1/creations/sync?since_revision=0", Some(&a_relogin.token), None)
        .await?;
    let (st_sync_b, sync_b) = driver
        .json_req(Method::GET, "/api/v1/creations/sync?since_revision=0", Some(&b_relogin.token), None)
        .await?;
    assert_eq!(st_sync_a, 200);
    assert_eq!(st_sync_b, 200);
    assert!(
        value_contains_creation_id(&sync_a, &a_id)
            && value_contains_creation_id(&sync_a, &s_id)
            && !value_contains_creation_id(&sync_a, &b_id),
        "Account A /creations/sync isolation failed: {sync_a}"
    );
    assert!(
        value_contains_creation_id(&sync_b, &b_id) && !value_contains_creation_id(&sync_b, &a_id),
        "Account B /creations/sync isolation failed: {sync_b}"
    );

    // Verify post-restart cross-account direct draft GET is strictly denied (403/404)
    let (st_b_read_a, b_read_a) = get_creation_raw(&driver, Some(&b_relogin.token), &a_id).await?;
    assert!(
        matches!(st_b_read_a, 403 | 404),
        "Account B direct GET on Account A draft after restart must return 403/404, got {st_b_read_a}: {b_read_a}"
    );
    let (st_a_read_b, a_read_b) = get_creation_raw(&driver, Some(&a_relogin.token), &b_id).await?;
    assert!(
        matches!(st_a_read_b, 403 | 404),
        "Account A direct GET on Account B draft after restart must return 403/404, got {st_a_read_b}: {a_read_b}"
    );

    let (st_a_st, st_a_states) = driver.json_req(Method::GET, "/api/v1/catalog/my-states", Some(&a_relogin.token), None).await?;
    let (st_b_st, st_b_states) = driver.json_req(Method::GET, "/api/v1/catalog/my-states", Some(&b_relogin.token), None).await?;
    assert_eq!(st_a_st, 200);
    assert_eq!(st_b_st, 200);
    let a_entry = extract_items(&st_a_states)?
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(s_id.as_str()))
        .expect("Account A state survived restart");
    let b_entry = extract_items(&st_b_states)?
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(s_id.as_str()))
        .expect("Account B state survived restart");
    assert!((a_entry["progress_percent"].as_f64().unwrap_or(0.0) - 25.0).abs() < 0.5);
    assert!((b_entry["progress_percent"].as_f64().unwrap_or(0.0) - 90.0).abs() < 0.5);
    assert!(value_contains_substring(&st_a_states, "alpha_pos") && !value_contains_substring(&st_a_states, "beta_pos"));
    assert!(value_contains_substring(&st_b_states, "beta_pos") && !value_contains_substring(&st_b_states, "alpha_pos"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t4_06_full_moderation_lifecycle_422_reject_v1_live_edit_v2_unpublish_delete_restart() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, reader, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let cover_png = make_unique_png_bytes("t4_06_lifecycle_cover");
    let cover_id = upload_raw_media(
        &driver,
        &creator.token,
        "lifecycle_cover.png",
        "image/png",
        cover_png.clone(),
    )
    .await?;

    // Step 1: Create invalid draft (empty chapters) -> CAS 403 + 422 validation error
    let mut draft = make_book_creation(
        "LifecyclePrefix CanaryPhoenixV1",
        "Journey through every review state and cold restart.",
        "Author",
        "en-US",
        &["Fiction"],
        &["LifecyclePrefix"],
        Some(&cover_id),
    );
    draft["payload"]["chapters"] = json!([]);
    let created = create_creation_ok(&driver, &creator.token, draft).await?;
    let cid = created["creation_id"].as_str().unwrap().to_string();
    let rev1 = created["cloud_revision"].as_i64().unwrap();

    assert_eq!(download_media(&driver, &reader.token, &cover_id).await?.0, 403);
    let (st_422, err_422) = submit_review_raw(&driver, &creator.token, &cid).await?;
    assert_eq!(st_422, 422, "invalid book must fail with 422: {err_422}");
    assert!(!err_422["validation_errors"].as_array().unwrap().is_empty());

    // Step 2: Fix payload -> submit (CAS stays 403 in PendingReview) -> reject (CAS stays 403 in Rejected)
    let mut fixed = make_book_creation(
        "LifecyclePrefix CanaryPhoenixV1",
        "Journey through every review state and cold restart.",
        "Author",
        "en-US",
        &["Fiction"],
        &["LifecyclePrefix"],
        Some(&cover_id),
    );
    fixed["expected_revision"] = json!(rev1);
    let (st_fix, fixed_dto) = update_creation_raw(&driver, &creator.token, &cid, fixed.clone()).await?;
    assert_eq!(st_fix, 200);
    assert!(fixed_dto["cloud_revision"].as_i64().unwrap() > rev1);
    let (st_sub1, sub1_val) = submit_review_raw(&driver, &creator.token, &cid).await?;
    let _ = assert_submit_review_ok(st_sub1, &sub1_val, "t4_06 pending")?;
    assert_eq!(
        download_media(&driver, &reader.token, &cover_id).await?.0,
        403,
        "CAS media must stay 403 while PendingReview"
    );
    assert_eq!(
        review_decision_raw(
            &driver,
            &admin.token,
            &cid,
            json!({ "decision": "reject", "reason": "Add subtitle polish" }),
        )
        .await?
        .0,
        200
    );
    assert_eq!(
        download_media(&driver, &reader.token, &cover_id).await?.0,
        403,
        "CAS media must stay 403 while Rejected"
    );

    // Step 3: Revise & approve V1 -> CAS becomes 200
    let mut rev_v1 = fixed.clone();
    rev_v1["subtitle"] = json!("Polished First Edition");
    let (st_cur, cur_after_rej) = get_creation_raw(&driver, Some(&creator.token), &cid).await?;
    assert_eq!(st_cur, 200);
    rev_v1["expected_revision"] = json!(cur_after_rej["cloud_revision"].as_i64().unwrap());
    assert_eq!(update_creation_raw(&driver, &creator.token, &cid, rev_v1).await?.0, 200);
    let pub_v1 = publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;
    let (st_med_v1, got_png_v1) = download_media(&driver, &reader.token, &cover_id).await?;
    assert_eq!(st_med_v1, 200);
    assert_eq!(got_png_v1, cover_png);

    // Step 4: Live-edit BOTH title AND chapter body to V2 while readers still see V1 & FTS hides V2
    let mut v2 = fixed.clone();
    v2["title"] = json!("LifecyclePrefix CanaryPhoenixV2");
    v2["payload"]["chapters"][0]["pages"][0]["blocks"][1]["text"] = json!(
        "LifecycleV2BodyCanary774 rewritten orbital chronicle awaiting editorial re-approval."
    );
    v2["expected_revision"] = json!(pub_v1["cloud_revision"].as_i64().unwrap());
    assert_eq!(update_creation_raw(&driver, &creator.token, &cid, v2).await?.0, 200);

    let (st_rv1, reader_v1) = get_creation_raw(&driver, Some(&reader.token), &cid).await?;
    assert_eq!(st_rv1, 200);
    assert_eq!(reader_v1["title"], "LifecyclePrefix CanaryPhoenixV1");
    assert!(value_contains_substring(&reader_v1, "Europa Station"));
    assert!(
        !value_contains_substring(&reader_v1, "LifecycleV2BodyCanary774"),
        "unapproved V2 body leaked to reader: {reader_v1}"
    );
    let (st_v2q, v2q_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/books?q=CanaryPhoenixV2", Some(&reader.token), None)
        .await?;
    assert_eq!(st_v2q, 200);
    assert!(
        !extract_items(&v2q_body)?.iter().any(|i| i["creation_id"] == cid),
        "unapproved V2 must not be searchable before re-approval: {v2q_body}"
    );

    // Step 5: Approve V2 -> readers now see V2 title + body in GET and catalog
    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;
    let (st_rv2, reader_v2) = get_creation_raw(&driver, Some(&reader.token), &cid).await?;
    assert_eq!(st_rv2, 200);
    assert_eq!(reader_v2["title"], "LifecyclePrefix CanaryPhoenixV2");
    assert!(value_contains_substring(&reader_v2, "LifecycleV2BodyCanary774"));
    let (st_cat_v2, cat_v2) = driver
        .json_req(Method::GET, "/api/v1/catalog/books", Some(&reader.token), None)
        .await?;
    assert_eq!(st_cat_v2, 200);
    assert!(value_contains_substring(&cat_v2, "LifecyclePrefix CanaryPhoenixV2"));

    // Step 6: Unpublish -> CAS 403 + zero-leak across all surfaces -> republish (CAS 200) -> DELETE revokes CAS (403)
    assert_eq!(unpublish_raw(&driver, &creator.token, &cid).await?.0, 200);
    assert_eq!(download_media(&driver, &reader.token, &cover_id).await?.0, 403);
    let reader_dm = open_creator_bot_dm(&driver, &reader.token).await?;
    assert_zero_leak_across_all_surfaces(
        &driver,
        &reader.token,
        reader_dm,
        "books",
        &cid,
        &["LifecyclePrefix"],
        &["CanaryPhoenixV1", "CanaryPhoenixV2", "LifecycleV2BodyCanary774"],
        None,
    )
    .await?;

    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;
    assert_eq!(download_media(&driver, &reader.token, &cover_id).await?.0, 200);
    let (st_del, _) = delete_creation_raw(&driver, &creator.token, &cid).await?;
    assert!(st_del == 200 || st_del == 204);
    assert_eq!(
        download_media(&driver, &reader.token, &cover_id).await?.0,
        403,
        "DELETE /creations/{cid} must immediately revoke public CAS media access"
    );

    // Step 7: Cold restart -> tombstone & full audit trail survive!
    let driver = restart_server(&mut server).await?;
    let (st_sync, sync_body) = driver
        .json_req(Method::GET, "/api/v1/creations/sync?since_revision=0", Some(&creator.token), None)
        .await?;
    assert_eq!(st_sync, 200);
    let tomb = extract_items(&sync_body)?
        .iter()
        .find(|i| i["creation_id"] == cid)
        .expect("deleted tombstone survives cold restart");
    assert!(tomb["deleted_at_ms"].as_i64().unwrap_or(0) > 0);

    let (st_aud, aud_body) = get_audit_raw(&driver, &creator.token, &cid).await?;
    assert_eq!(st_aud, 200);
    assert!(extract_items(&aud_body)?.len() >= 5, "full audit trail must survive restart: {aud_body}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t4_07_mixed_visibility_matrix_zero_leak_across_all_verticals_before_and_after_restart() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (c1, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let c2 = register_actor(&driver, "matrix_creator2").await?;

    // Seed 1 public item + 5 non-public items across all 4 verticals sharing a common tag family "MatrixBeacon" / "矩阵信标"
    let pub_book = create_creation_ok(
        &driver,
        &c1.token,
        make_book_creation("MatrixBeacon PublicBook 矩阵信标公开书", "Only publicly visible item in matrix 矩阵信标公开书.", "C1", "zh-CN", &["Sci-Fi"], &["MatrixBeacon", "矩阵信标"], None),
    )
    .await?;
    let pub_id = pub_book["creation_id"].as_str().unwrap().to_string();
    publish_creation_public(&driver, &c1.token, &admin.token, &pub_id).await?;

    let draft_music = create_creation_ok(
        &driver,
        &c1.token,
        make_music_creation("MatrixBeacon DraftSecret1 矩阵信标草稿曲", "Private draft music 矩阵信标草稿曲.", "C1", "zh-CN", &["Pop"], &["MatrixBeacon", "矩阵信标"], None, None),
    )
    .await?;
    let draft_id = draft_music["creation_id"].as_str().unwrap().to_string();

    let pend_game = create_creation_ok(
        &driver,
        &c2.token,
        make_game_creation("MatrixBeacon PendingSecret2 矩阵信标待审游", "Pending game 矩阵信标待审游.", "C2", "zh-CN", &["Trivia"], &["MatrixBeacon", "矩阵信标"], None),
    )
    .await?;
    let pend_id = pend_game["creation_id"].as_str().unwrap().to_string();
    let (st_sub_p, sub_p_val) = submit_review_raw(&driver, &c2.token, &pend_id).await?;
    let _ = assert_submit_review_ok(st_sub_p, &sub_p_val, "t4_07 pend_game")?;

    let rej_app = create_creation_ok(
        &driver,
        &c2.token,
        make_app_creation("MatrixBeacon RejectedSecret3 矩阵信标驳回应用", "Rejected app 矩阵信标驳回应用.", "C2", "zh-CN", &["Utilities"], &["MatrixBeacon", "矩阵信标"], None, None),
    )
    .await?;
    let rej_id = rej_app["creation_id"].as_str().unwrap().to_string();
    let (st_sub_r, sub_r_val) = submit_review_raw(&driver, &c2.token, &rej_id).await?;
    let _ = assert_submit_review_ok(st_sub_r, &sub_r_val, "t4_07 rej_app")?;
    assert_eq!(review_decision_raw(&driver, &admin.token, &rej_id, json!({ "decision": "reject", "reason": "Matrix reject" })).await?.0, 200);

    let unl_book = create_creation_ok(
        &driver,
        &c1.token,
        make_book_creation("MatrixBeacon UnlistedSecret4 矩阵信标直链书", "Unlisted book 矩阵信标直链书.", "C1", "zh-CN", &["Fiction"], &["MatrixBeacon", "矩阵信标"], None),
    )
    .await?;
    let unl_id = unl_book["creation_id"].as_str().unwrap().to_string();
    publish_creation_unlisted(&driver, &c1.token, &admin.token, &unl_id).await?;

    let unpub_music = create_creation_ok(
        &driver,
        &c2.token,
        make_music_creation("MatrixBeacon UnpubSecret5 矩阵信标下架曲", "Unpublished track 矩阵信标下架曲.", "C2", "zh-CN", &["Jazz"], &["MatrixBeacon", "矩阵信标"], None, None),
    )
    .await?;
    let unpub_id = unpub_music["creation_id"].as_str().unwrap().to_string();
    publish_creation_public(&driver, &c2.token, &admin.token, &unpub_id).await?;
    assert_eq!(unpublish_raw(&driver, &c2.token, &unpub_id).await?.0, 200);

    let hidden_ids = [
        draft_id.as_str(),
        pend_id.as_str(),
        rej_id.as_str(),
        unl_id.as_str(),
        unpub_id.as_str(),
    ];
    let hidden_markers = [
        "DraftSecret1", "矩阵信标草稿曲",
        "PendingSecret2", "矩阵信标待审游",
        "RejectedSecret3", "矩阵信标驳回应用",
        "UnlistedSecret4", "矩阵信标直链书",
        "UnpubSecret5", "矩阵信标下架曲",
    ];

    // Verify BEFORE and AFTER cold restart: ONLY pub_id appears, ZERO leaks across unfiltered & queried surfaces!
    let mut cur_driver = driver;
    for phase in ["before_restart", "after_restart"] {
        if phase == "after_restart" {
            cur_driver = restart_server(&mut server).await?;
        }
        let dm = open_creator_bot_dm(&cur_driver, &consumer.token).await?;

        // 1. Unfiltered (no `q` / empty `query`) checks across all 7 HTTP surfaces + bare `/recommend`
        for path in [
            "/api/v1/catalog?kind=all&limit=50",
            "/api/v1/catalog/books?limit=50",
            "/api/v1/catalog/music?limit=50",
            "/api/v1/catalog/games?limit=50",
            "/api/v1/catalog/appstore?limit=50",
            "/api/v1/catalog/recommend?kind=all&limit=20",
            "/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query=&limit=20",
        ] {
            let (st, body) = cur_driver.json_req(Method::GET, path, Some(&consumer.token), None).await?;
            assert_eq!(st, 200, "{phase} unfiltered {path} failed: {body}");
            let entries = extract_surface_entries(&body)?;
            for entry in &entries {
                for hid in hidden_ids {
                    assert!(!value_contains_creation_id(entry, hid), "{phase} UNFILTERED LEAK {hid} in {path}: {body}");
                }
                for m in hidden_markers {
                    assert!(!value_contains_substring(entry, m), "{phase} UNFILTERED LEAK marker {m} in {path}: {body}");
                }
            }
            if path.starts_with("/api/v1/catalog?")
                || path.starts_with("/api/v1/catalog/books")
                || path.starts_with("/api/v1/catalog/recommend")
            {
                assert!(
                    entries.iter().any(|e| value_contains_creation_id(e, &pub_id)),
                    "{phase} public item missing in unfiltered {path}: {body}"
                );
            }
        }
        let bare_reply = send_bot_command_and_poll_reply(&cur_driver, &consumer.token, dm, "/recommend").await?;
        for hid in hidden_ids {
            assert!(!value_contains_creation_id(&bare_reply, hid), "{phase} BOT LEAK {hid} on bare /recommend: {bare_reply}");
        }
        for m in hidden_markers {
            assert!(!value_contains_substring(&bare_reply, m), "{phase} BOT LEAK marker {m} on bare /recommend: {bare_reply}");
        }
        assert!(
            value_contains_creation_id(&bare_reply, &pub_id)
                || value_contains_substring(&bare_reply, "PublicBook")
                || value_contains_substring(&bare_reply, "公开书"),
            "{phase} bare BOT /recommend must return public item: {bare_reply}"
        );

        // 2. Bilingual EN + CJK queried checks across all 7 HTTP surfaces + `/recommend {q}`
        for q in ["MatrixBeacon", "矩阵信标"] {
            let enc = percent_encode(q);
            for path in [
                format!("/api/v1/catalog?kind=all&q={enc}&limit=50"),
                format!("/api/v1/catalog/books?q={enc}&limit=50"),
                format!("/api/v1/catalog/music?q={enc}&limit=50"),
                format!("/api/v1/catalog/games?q={enc}&limit=50"),
                format!("/api/v1/catalog/appstore?q={enc}&limit=50"),
                format!("/api/v1/catalog/recommend?q={enc}&kind=all&limit=20"),
                format!("/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query={enc}&limit=20"),
            ] {
                let (st, body) = cur_driver.json_req(Method::GET, &path, Some(&consumer.token), None).await?;
                assert_eq!(st, 200, "{phase} {path} failed: {body}");
                let entries = extract_surface_entries(&body)?;
                for entry in &entries {
                    for hid in hidden_ids {
                        assert!(!value_contains_creation_id(entry, hid), "{phase} LEAK {hid} in {path}: {body}");
                    }
                    for m in hidden_markers {
                        assert!(!value_contains_substring(entry, m), "{phase} LEAK marker {m} in {path}: {body}");
                    }
                }
                if path.starts_with("/api/v1/catalog?")
                    || path.starts_with("/api/v1/catalog/books")
                    || path.starts_with("/api/v1/catalog/recommend")
                    || path.starts_with("/api/v1/bots/inline-query")
                {
                    assert!(
                        entries.iter().any(|e| value_contains_creation_id(e, &pub_id)),
                        "{phase} public item missing in {path}: {body}"
                    );
                }
            }
            let reply = send_bot_command_and_poll_reply(&cur_driver, &consumer.token, dm, &format!("/recommend {q}")).await?;
            for hid in hidden_ids {
                assert!(!value_contains_creation_id(&reply, hid), "{phase} BOT LEAK {hid} on /recommend {q}: {reply}");
            }
            for m in hidden_markers {
                assert!(!value_contains_substring(&reply, m), "{phase} BOT LEAK marker {m} on /recommend {q}: {reply}");
            }
            assert!(
                value_contains_creation_id(&reply, &pub_id)
                    || value_contains_substring(&reply, "PublicBook")
                    || value_contains_substring(&reply, "公开书"),
                "{phase} BOT /recommend {q} must return public item: {reply}"
            );
        }
    }

    server.stop().await;
    Ok(())
}
