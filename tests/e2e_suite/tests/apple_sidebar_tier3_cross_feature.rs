mod apple_sidebar_common;

use apple_sidebar_common::*;
use reqwest::Method;
use serde_json::json;
use std::time::Duration;

// ============================================================================
// Tier 3 — Pairwise Cross-Feature Combinations (14 tests: t3_01..t3_14)
// ============================================================================

#[tokio::test]
async fn t3_01_bot_book_create_to_cloud_edit_submit_approve_catalog_and_bot_recommend() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let creator_dm = open_creator_bot_dm(&driver, &creator.token).await?;
    let _create_reply = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        creator_dm,
        "/newbook Obsidian Lanterns | Atmospheric fantasy book about starlight cartographers",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=book", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let mut draft = extract_items(&my_val)?[0].clone();
    let cid = draft["creation_id"].as_str().unwrap().to_string();
    let rev = draft["cloud_revision"].as_i64().unwrap();

    draft["title"] = json!("Obsidian Lanterns Definitive");
    draft["expected_revision"] = json!(rev);
    let (st_upd, _) = update_creation_raw(&driver, &creator.token, &cid, draft).await?;
    assert_eq!(st_upd, 200);

    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;

    let (st_cat, cat_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/books?q=Obsidian", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    assert!(value_contains_creation_id(&cat_val, &cid));
    assert!(
        value_contains_substring(&cat_val, "Obsidian Lanterns Definitive"),
        "edited title must propagate to /catalog/books: {cat_val}"
    );

    let consumer_dm = open_creator_bot_dm(&driver, &consumer.token).await?;
    let rec_reply = send_bot_command_and_poll_reply(
        &driver,
        &consumer.token,
        consumer_dm,
        "/recommend Obsidian",
    )
    .await?;
    let links = extract_apple_deep_links_from_message(&rec_reply);
    assert!(
        links.iter().any(|l| l.starts_with(&format!("tg://apple/books/{cid}"))),
        "expected tg://apple/books/{cid} in consumer bot recommendation: {rec_reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_02_bot_music_create_to_cloud_edit_submit_approve_catalog_and_bot_recommend() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let creator_dm = open_creator_bot_dm(&driver, &creator.token).await?;
    let _ = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        creator_dm,
        "/newmusic Velvet Comet Drift | Lush ambient electronic album at 110 BPM",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=music", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let mut draft = extract_items(&my_val)?[0].clone();
    let cid = draft["creation_id"].as_str().unwrap().to_string();
    let rev = draft["cloud_revision"].as_i64().unwrap();

    draft["title"] = json!("Velvet Comet Drift Remastered");
    draft["expected_revision"] = json!(rev);
    assert_eq!(update_creation_raw(&driver, &creator.token, &cid, draft).await?.0, 200);

    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;

    let (st_cat, cat_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/music?q=Velvet", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    assert!(value_contains_creation_id(&cat_val, &cid));
    assert!(
        value_contains_substring(&cat_val, "Velvet Comet Drift Remastered"),
        "edited title must propagate to /catalog/music: {cat_val}"
    );

    let consumer_dm = open_creator_bot_dm(&driver, &consumer.token).await?;
    let rec_reply = send_bot_command_and_poll_reply(
        &driver,
        &consumer.token,
        consumer_dm,
        "/recommend Velvet",
    )
    .await?;
    let links = extract_apple_deep_links_from_message(&rec_reply);
    assert!(
        links.iter().any(|l| l.starts_with(&format!("tg://apple/music/{cid}"))),
        "expected tg://apple/music/{cid} in consumer bot recommendation: {rec_reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_03_bot_game_create_to_cloud_edit_submit_approve_catalog_and_bot_recommend() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let creator_dm = open_creator_bot_dm(&driver, &creator.token).await?;
    let _ = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        creator_dm,
        "/newgame Prism Circuit Arcade | Playable reflex and trivia puzzle game",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=game", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let mut draft = extract_items(&my_val)?[0].clone();
    let cid = draft["creation_id"].as_str().unwrap().to_string();
    let rev = draft["cloud_revision"].as_i64().unwrap();

    draft["title"] = json!("Prism Circuit Arcade Deluxe");
    draft["expected_revision"] = json!(rev);
    assert_eq!(update_creation_raw(&driver, &creator.token, &cid, draft).await?.0, 200);

    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;

    let (st_cat, cat_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/games?q=Prism", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    assert!(value_contains_creation_id(&cat_val, &cid));
    assert!(
        value_contains_substring(&cat_val, "Prism Circuit Arcade Deluxe"),
        "edited title must propagate to /catalog/games: {cat_val}"
    );

    let consumer_dm = open_creator_bot_dm(&driver, &consumer.token).await?;
    let rec_reply = send_bot_command_and_poll_reply(
        &driver,
        &consumer.token,
        consumer_dm,
        "/recommend Prism",
    )
    .await?;
    let links = extract_apple_deep_links_from_message(&rec_reply);
    assert!(
        links.iter().any(|l| l.starts_with(&format!("tg://apple/games/{cid}"))),
        "expected tg://apple/games/{cid} in consumer bot recommendation: {rec_reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_04_live_edit_published_creation_preserves_frozen_snapshot_until_v2_approved() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let mut payload = make_book_creation(
        "Frozen Edition V1",
        "Version 1 live published content visible to all readers.",
        "Author",
        "en-US",
        &["Fiction"],
        &["frozen"],
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, payload.clone()).await?;
    let cid = created["creation_id"].as_str().unwrap();
    let pub_v1 = publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    let rev_after_pub = pub_v1["cloud_revision"].as_i64().unwrap();

    // Owner live-edits BOTH title AND chapter body payload to V2 without re-approving yet
    payload["title"] = json!("Unapproved Draft V2");
    payload["payload"]["chapters"][0]["pages"][0]["blocks"][1]["text"] = json!(
        "V2SecretParagraphCanary881 rewritten orbital epilogue awaiting editorial approval before public visibility."
    );
    payload["expected_revision"] = json!(rev_after_pub);
    let (st_upd, upd_v2) = update_creation_raw(&driver, &creator.token, cid, payload).await?;
    assert_eq!(st_upd, 200);
    assert_eq!(upd_v2["title"], "Unapproved Draft V2");
    assert_eq!(upd_v2["has_live_published_version"], true);
    assert_eq!(upd_v2["published_version"].as_u64(), Some(1));

    // Non-owner MUST still see frozen V1 title + V1 body (zero V2 canary) AND FTS5 must not index V2 yet
    let (st_non, non_view) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert_eq!(st_non, 200);
    assert_eq!(non_view["title"], "Frozen Edition V1", "unapproved V2 title leaked to non-owner: {non_view}");
    assert!(
        value_contains_substring(&non_view, "Europa Station"),
        "frozen V1 paragraph body missing for non-owner: {non_view}"
    );
    assert!(
        !value_contains_substring(&non_view, "V2SecretParagraphCanary881"),
        "unapproved V2 chapter body leaked to non-owner: {non_view}"
    );

    let (st_cat, cat_v1) = driver
        .json_req(Method::GET, "/api/v1/catalog/books", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    let live_item = extract_items(&cat_v1)?.iter().find(|i| i["creation_id"] == cid).expect("live book");
    assert_eq!(live_item["title"], "Frozen Edition V1");

    let (st_v2q, cat_v2q) = driver
        .json_req(Method::GET, "/api/v1/catalog/books?q=Unapproved", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_v2q, 200);
    assert!(
        !extract_items(&cat_v2q)?.iter().any(|i| i["creation_id"] == cid),
        "unapproved V2 title must not be searchable in /catalog/books before re-approval: {cat_v2q}"
    );

    // Once owner re-submits and admin approves V2, consumers see V2 title + V2 body in GET and catalog
    let pub_v2 = publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    assert!(pub_v2["published_version"].as_u64().unwrap_or(0) >= 2);
    let (st_non_v2, non_view_v2) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert_eq!(st_non_v2, 200);
    assert_eq!(non_view_v2["title"], "Unapproved Draft V2");
    assert!(
        value_contains_substring(&non_view_v2, "V2SecretParagraphCanary881"),
        "approved V2 paragraph body must be visible to non-owner: {non_view_v2}"
    );

    let (st_cat_v2, cat_v2) = driver
        .json_req(Method::GET, "/api/v1/catalog/books", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat_v2, 200);
    let live_item_v2 = extract_items(&cat_v2)?.iter().find(|i| i["creation_id"] == cid).expect("live V2 book");
    assert_eq!(live_item_v2["title"], "Unapproved Draft V2");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_05_live_edit_v2_rejected_keeps_v1_frozen_published_snapshot_intact() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let mut payload = make_music_creation(
        "Stable Track V1",
        "Published V1 soundtrack that stays live if V2 is rejected.",
        "Artist",
        "en-US",
        &["Ambient"],
        &["stable"],
        None,
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, payload.clone()).await?;
    let cid = created["creation_id"].as_str().unwrap();
    let pub_v1 = publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    payload["title"] = json!("Experimental Rejected V2");
    payload["expected_revision"] = json!(pub_v1["cloud_revision"].as_i64().unwrap());
    assert_eq!(update_creation_raw(&driver, &creator.token, cid, payload).await?.0, 200);
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t3_05 v2 submit")?;
    let (st_rej, rej_val) = review_decision_raw(
        &driver,
        &admin.token,
        cid,
        json!({ "decision": "reject", "reason": "Keep V1 live while remix is polished" }),
    )
    .await?;
    assert_eq!(st_rej, 200);
    assert_eq!(rej_val["review_status"], "rejected");

    let (st_con, con_view) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert_eq!(st_con, 200, "live V1 should remain accessible after V2 rejection: {con_view}");
    assert_eq!(con_view["title"], "Stable Track V1");
    assert!(!value_contains_substring(&con_view, "Experimental Rejected V2"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_06_appstore_aggregates_published_apps_miniapps_and_featured_verticals() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let app_c = create_creation_ok(
        &driver,
        &creator.token,
        make_app_creation("Native AppStore Tool", "Published native App creation.", "AppleDev", "en-US", &["Productivity"], &["native"], None, None),
    )
    .await?;
    let book_c = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Showcase Store Book", "Published book surfaced in App Store.", "Author", "en-US", &["Fiction"], &["showcase"], None),
    )
    .await?;
    let music_c = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Showcase Store Track", "Published track surfaced in App Store.", "Artist", "en-US", &["Electronic"], &["showcase"], None, None),
    )
    .await?;
    let game_c = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Showcase Store Game", "Published game surfaced in App Store.", "Dev", "en-US", &["Trivia"], &["showcase"], None),
    )
    .await?;

    let app_id = app_c["creation_id"].as_str().unwrap();
    let book_id = book_c["creation_id"].as_str().unwrap();
    let music_id = music_c["creation_id"].as_str().unwrap();
    let game_id = game_c["creation_id"].as_str().unwrap();
    for id in [app_id, book_id, music_id, game_id] {
        publish_creation_public(&driver, &creator.token, &admin.token, id).await?;
    }

    let bot_uname = format!("storemerge_{}_bot", unique_suffix());
    let (_bot_tok, miniapp) = publish_miniapp(
        &driver,
        &creator.token,
        &bot_uname,
        "merge_tool",
        "Merged Telegram MiniApp",
    )
    .await?;
    let miniapp_id = miniapp["app_id"].as_str().unwrap();

    let (st, store_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/appstore", Some(&consumer.token), None)
        .await?;
    assert_eq!(st, 200, "catalog/appstore failed: {store_body}");
    let entries = extract_surface_entries(&store_body)?;
    assert!(
        entries.iter().any(|e| value_contains_creation_id(e, app_id)),
        "missing published App creation in /catalog/appstore: {store_body}"
    );
    assert!(
        entries.iter().any(|e| {
            value_contains_substring(e, miniapp_id)
                || value_contains_substring(e, &bot_uname)
                || value_contains_substring(e, "Merged Telegram MiniApp")
        }),
        "missing published mini_app in /catalog/appstore: {store_body}"
    );
    assert!(
        entries.iter().any(|e| value_contains_creation_id(e, book_id))
            && entries.iter().any(|e| value_contains_creation_id(e, music_id))
            && entries.iter().any(|e| value_contains_creation_id(e, game_id)),
        "ALL THREE cross-vertical featured creations (book + music + game) must appear in /catalog/appstore: {store_body}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_07_cas_media_asset_access_403_draft_200_published_403_unpublished() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let png_bytes = make_unique_png_bytes("t3_07_cover");
    let wav_bytes = make_unique_wav_bytes("t3_07_audio");
    let cover_id = upload_raw_media(&driver, &creator.token, "cover_t3_07.png", "image/png", png_bytes.clone()).await?;
    let audio_id = upload_raw_media(&driver, &creator.token, "track_t3_07.wav", "audio/wav", wav_bytes.clone()).await?;

    let music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation(
            "CAS Protected Single",
            "Music with private CAS cover and WAV audio.",
            "Artist",
            "en-US",
            &["Electronic"],
            &["casmedia"],
            Some(&cover_id),
            Some(&audio_id),
        ),
    )
    .await?;
    let cid = music["creation_id"].as_str().unwrap();

    // 1. Draft: non-owner MUST get 403 on both cover PNG and audio WAV
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 403);
    assert_eq!(download_media(&driver, &consumer.token, &audio_id).await?.0, 403);

    // 2. PendingReview: non-owner MUST still get 403 on both cover PNG and audio WAV
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t3_07 pending")?;
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 403);
    assert_eq!(download_media(&driver, &consumer.token, &audio_id).await?.0, 403);

    // 3. Rejected: non-owner MUST still get 403 on both cover PNG and audio WAV
    assert_eq!(
        review_decision_raw(
            &driver,
            &admin.token,
            cid,
            json!({ "decision": "reject", "reason": "Verify CAS stays 403 while rejected" }),
        )
        .await?
        .0,
        200
    );
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 403);
    assert_eq!(download_media(&driver, &consumer.token, &audio_id).await?.0, 403);

    // 4. Published + Public: non-owner MUST get 200 with exact bytes
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    let (st_c_pub, got_png) = download_media(&driver, &consumer.token, &cover_id).await?;
    let (st_a_pub, got_wav) = download_media(&driver, &consumer.token, &audio_id).await?;
    assert_eq!(st_c_pub, 200, "cover PNG must be readable after public publish");
    assert_eq!(st_a_pub, 200, "audio WAV must be readable after public publish");
    assert_eq!(got_png, png_bytes);
    assert_eq!(got_wav, wav_bytes);

    // 5. Unpublished: non-owner MUST immediately revert to 403
    assert_eq!(unpublish_raw(&driver, &creator.token, cid).await?.0, 200);
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 403);
    assert_eq!(download_media(&driver, &consumer.token, &audio_id).await?.0, 403);

    // 6. Republish (200) then DELETE -> non-owner MUST immediately revert to 403!
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 200);
    assert_eq!(download_media(&driver, &consumer.token, &audio_id).await?.0, 200);
    let (st_del, _) = delete_creation_raw(&driver, &creator.token, cid).await?;
    assert!(st_del == 200 || st_del == 204);
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 403);
    assert_eq!(download_media(&driver, &consumer.token, &audio_id).await?.0, 403);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_08_ws_delivers_user_creation_changed_to_owner_across_fsm_transitions() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let mut owner_ws = connect_raw_ws(&server, &creator.token).await?;
    let mut consumer_ws = connect_raw_ws(&server, &consumer.token).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("WS Owner Book", "Book triggering UserCreationChanged WS events.", "Author", "en-US", &["Fiction"], &["ws"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap().to_string();

    let (_ev_draft, d_draft) = recv_ws_event_matching(&mut owner_ws, Duration::from_secs(5), |var, data| {
        var == "UserCreationChanged" && data["creation_id"].as_str() == Some(cid.as_str())
    })
    .await?;
    assert_eq!(d_draft["review_status"], "draft");
    let rev_draft = d_draft["cloud_revision"].as_i64().unwrap_or(0);
    assert!(rev_draft >= 1, "expected cloud_revision >= 1 on draft WS event: {d_draft}");

    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, &cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t3_08 ws submit")?;
    let (_ev_pend, d_pend) = recv_ws_event_matching(&mut owner_ws, Duration::from_secs(5), |var, data| {
        var == "UserCreationChanged"
            && data["creation_id"].as_str() == Some(cid.as_str())
            && data["review_status"].as_str() == Some("pending_review")
    })
    .await?;
    assert_eq!(d_pend["owner_user_id"].as_u64(), Some(creator.user_id));
    let rev_pend = d_pend["cloud_revision"].as_i64().unwrap_or(0);
    assert!(
        rev_pend > rev_draft,
        "cloud_revision must strictly increase draft({rev_draft}) -> pending({rev_pend})"
    );

    assert_eq!(
        review_decision_raw(&driver, &admin.token, &cid, json!({ "decision": "approve", "visibility": "public" }))
            .await?
            .0,
        200
    );
    let (_ev_pub, d_pub) = recv_ws_event_matching(&mut owner_ws, Duration::from_secs(5), |var, data| {
        var == "UserCreationChanged"
            && data["creation_id"].as_str() == Some(cid.as_str())
            && data["review_status"].as_str() == Some("published")
    })
    .await?;
    assert_eq!(d_pub["kind"], "book");
    let rev_pub = d_pub["cloud_revision"].as_i64().unwrap_or(0);
    assert!(
        rev_pub > rev_pend,
        "cloud_revision must strictly increase pending({rev_pend}) -> published({rev_pub})"
    );

    // Privacy isolation: non-owner consumer_ws MUST receive ZERO UserCreationChanged events for creator's creation!
    assert_no_ws_event_matching(
        &mut consumer_ws,
        Duration::from_millis(600),
        |var, data| var == "UserCreationChanged" && data["creation_id"].as_str() == Some(cid.as_str()),
        "non-owner received owner's private UserCreationChanged",
    )
    .await?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_09_ws_delivers_public_catalog_changed_on_publish_and_unpublish() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let mut listener_ws = connect_raw_ws(&server, &consumer.token).await?;

    // Unlisted publish MUST NOT broadcast PublicCatalogChanged to other listeners!
    let unlisted = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("WS Unlisted Track", "Unlisted track that must stay silent on PublicCatalogChanged.", "DJ", "en-US", &["Electronic"], &["unlistedws"], None, None),
    )
    .await?;
    let unlisted_id = unlisted["creation_id"].as_str().unwrap().to_string();
    publish_creation_unlisted(&driver, &creator.token, &admin.token, &unlisted_id).await?;
    assert_no_ws_event_matching(
        &mut listener_ws,
        Duration::from_millis(600),
        |var, data| {
            var == "PublicCatalogChanged"
                && data["creation_id"].as_str() == Some(unlisted_id.as_str())
        },
        "Unlisted publish must never broadcast PublicCatalogChanged",
    )
    .await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("WS Catalog Track", "Track broadcasting PublicCatalogChanged events.", "DJ", "en-US", &["Electronic"], &["ws"], None, None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap().to_string();

    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;
    let (_ev_pub, d_pub) = recv_ws_event_matching(&mut listener_ws, Duration::from_secs(5), |var, data| {
        var == "PublicCatalogChanged"
            && data["creation_id"].as_str() == Some(cid.as_str())
            && data["action"].as_str() == Some("published")
    })
    .await?;
    assert_eq!(d_pub["kind"], "music");

    assert_eq!(unpublish_raw(&driver, &creator.token, &cid).await?.0, 200);
    let (_ev_rem, d_rem) = recv_ws_event_matching(&mut listener_ws, Duration::from_secs(5), |var, data| {
        var == "PublicCatalogChanged"
            && data["creation_id"].as_str() == Some(cid.as_str())
            && data["action"].as_str() == Some("removed")
    })
    .await?;
    assert_eq!(d_rem["kind"], "music");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_10_ratings_and_engagement_drive_top_rated_and_recommend_ranking() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, u1, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let u2 = register_actor(&driver, "rater_two").await?;

    let low = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Aurora Signal Alpha", "Sci-fi aurora anthology edition alpha.", "A", "en-US", &["Sci-Fi"], &["aurora"], None),
    )
    .await?;
    let high = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Aurora Signal Omega", "Sci-fi aurora anthology edition omega.", "A", "en-US", &["Sci-Fi"], &["aurora"], None),
    )
    .await?;
    let low_id = low["creation_id"].as_str().unwrap();
    let high_id = high["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, low_id).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, high_id).await?;

    for tok in [&u1.token, &u2.token] {
        let (st_h, _) = driver
            .json_req(Method::POST, &format!("/api/v1/catalog/{high_id}/reviews"), Some(tok), Some(json!({ "rating": 5, "title": "Peak", "body": "Brilliant." })))
            .await?;
        let (st_l, _) = driver
            .json_req(Method::POST, &format!("/api/v1/catalog/{low_id}/reviews"), Some(tok), Some(json!({ "rating": 1, "title": "Weak", "body": "Flat." })))
            .await?;
        assert!(st_h == 200 || st_h == 201);
        assert!(st_l == 200 || st_l == 201);
        let (st_s, _) = driver
            .json_req(Method::PUT, &format!("/api/v1/catalog/{high_id}/state"), Some(tok), Some(json!({ "in_library": true, "progress_percent": 100.0 })))
            .await?;
        assert_eq!(st_s, 200);
    }

    let (st_top, top_body) = driver
        .json_req(Method::GET, "/api/v1/catalog?kind=book&sort=top_rated", Some(&u1.token), None)
        .await?;
    assert_eq!(st_top, 200);
    assert_eq!(extract_items(&top_body)?[0]["creation_id"].as_str(), Some(high_id));

    let (st_rec, rec_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/recommend?q=Aurora&kind=book&limit=5", Some(&u1.token), None)
        .await?;
    assert_eq!(st_rec, 200);
    assert_eq!(extract_items(&rec_body)?[0]["creation_id"].as_str(), Some(high_id));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_11_multi_player_game_leaderboard_and_per_user_states_stay_consistent() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, p1, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let p2 = register_actor(&driver, "mp_player2").await?;
    let p3 = register_actor(&driver, "mp_player3").await?;

    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("MultiPlayer Quiz Bowl", "Three-player competitive leaderboard game.", "Dev", "en-US", &["Trivia"], &["mp"], None),
    )
    .await?;
    let gid = game["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, gid).await?;

    for (actor, score, lvl) in [(&p1, 1200, "lvl_2"), (&p2, 2400, "lvl_4"), (&p3, 1800, "lvl_3")] {
        let (st_sc, _) = driver
            .json_req(Method::POST, &format!("/api/v1/catalog/{gid}/scores"), Some(&actor.token), Some(json!({ "score": score })))
            .await?;
        assert!(st_sc == 200 || st_sc == 201);
        let (st_st, _) = driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{gid}/state"),
                Some(&actor.token),
                Some(json!({
                    "in_library": true,
                    "state_json": {
                        "high_score": score,
                        "save_state_json": { "current_level_id": lvl },
                        "in_library": true
                    }
                })),
            )
            .await?;
        assert_eq!(st_st, 200);
    }

    let (st_lb, lb_body) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{gid}/leaderboard"), Some(&p1.token), None)
        .await?;
    assert_eq!(st_lb, 200);
    let lb = extract_items(&lb_body)?;
    assert_eq!(lb.len(), 3);
    assert_eq!(lb[0]["user_id"].as_u64(), Some(p2.user_id));
    assert_eq!(lb[1]["user_id"].as_u64(), Some(p3.user_id));
    assert_eq!(lb[2]["user_id"].as_u64(), Some(p1.user_id));

    let (st_s2, s2_body) = driver.json_req(Method::GET, "/api/v1/catalog/my-states", Some(&p2.token), None).await?;
    assert_eq!(st_s2, 200);
    let s2_entry = extract_items(&s2_body)?
        .iter()
        .find(|s| s["creation_id"] == gid)
        .expect("p2 game state");
    assert_eq!(s2_entry["in_library"], true);
    assert_eq!(s2_entry["state_json"]["high_score"].as_i64(), Some(2400));
    assert!(value_contains_substring(&s2_entry["state_json"], "lvl_4"));
    assert!(!value_contains_substring(&s2_body, "lvl_2"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_12_inline_query_live_reflects_publish_and_unpublish_transitions() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("InlineLiveTransitionX", "Book testing live inline query publish/unpublish.", "A", "en-US", &["Sci-Fi"], &["InlineLiveTransitionX"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();

    let iq_path = "/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query=InlineLiveTransitionX&kind=book&limit=10";
    let (st1, iq1) = driver.json_req(Method::GET, iq_path, Some(&consumer.token), None).await?;
    assert_eq!(st1, 200);
    assert!(
        !extract_items(&iq1)?.iter().any(|item| value_contains_creation_id(item, cid)),
        "draft must not appear in inline-query results: {iq1}"
    );

    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    let (st2, iq2) = driver.json_req(Method::GET, iq_path, Some(&consumer.token), None).await?;
    assert_eq!(st2, 200);
    assert!(
        extract_items(&iq2)?.iter().any(|item| value_contains_creation_id(item, cid)),
        "published creation `{cid}` must appear inside inline-query results array: {iq2}"
    );

    assert_eq!(unpublish_raw(&driver, &creator.token, cid).await?.0, 200);
    let (st3, iq3) = driver.json_req(Method::GET, iq_path, Some(&consumer.token), None).await?;
    assert_eq!(st3, 200);
    assert!(
        !extract_items(&iq3)?.iter().any(|item| value_contains_creation_id(item, cid)),
        "unpublished creation `{cid}` must disappear from inline-query results array: {iq3}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_13_incremental_delta_sync_captures_bot_creation_and_review_fsm_changes() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let (st0, s0) = driver
        .json_req(Method::GET, "/api/v1/creations/sync?since_revision=0", Some(&creator.token), None)
        .await?;
    assert_eq!(st0, 200);
    let rev0 = s0["latest_revision"].as_i64().unwrap_or(0);

    let dm = open_creator_bot_dm(&driver, &creator.token).await?;
    let _ = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        dm,
        "/newbook SyncBot Chronicle | Bot authored book synced via cloud delta",
    )
    .await?;

    let (st1, s1) = driver
        .json_req(Method::GET, &format!("/api/v1/creations/sync?since_revision={rev0}"), Some(&creator.token), None)
        .await?;
    assert_eq!(st1, 200);
    let items1 = extract_items(&s1)?;
    assert_eq!(items1.len(), 1);
    let cid = items1[0]["creation_id"].as_str().unwrap().to_string();
    let rev1 = s1["latest_revision"].as_i64().unwrap();
    assert!(rev1 > rev0);
    assert_eq!(items1[0]["review_status"], "draft");

    publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;
    let (st2, s2) = driver
        .json_req(Method::GET, &format!("/api/v1/creations/sync?since_revision={rev1}"), Some(&creator.token), None)
        .await?;
    assert_eq!(st2, 200);
    let items2 = extract_items(&s2)?;
    assert_eq!(items2.len(), 1);
    assert_eq!(items2[0]["creation_id"], cid);
    assert_eq!(items2[0]["review_status"], "published");
    assert!(s2["latest_revision"].as_i64().unwrap() > rev1);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t3_14_republish_after_unpublish_restores_public_catalog_and_cas_media_access() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let png = make_unique_png_bytes("t3_14_repub");
    let cover_id = upload_raw_media(&driver, &creator.token, "repub_cover.png", "image/png", png.clone()).await?;
    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Phoenix Republish Book", "Book unpublished then republished cleanly.", "A", "en-US", &["Fiction"], &["phoenix"], Some(&cover_id)),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();

    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 200);

    assert_eq!(unpublish_raw(&driver, &creator.token, cid).await?.0, 200);
    assert_eq!(download_media(&driver, &consumer.token, &cover_id).await?.0, 403);
    let (st_cat_off, cat_off) = driver.json_req(Method::GET, "/api/v1/catalog/books?q=Phoenix", Some(&consumer.token), None).await?;
    assert_eq!(st_cat_off, 200);
    assert!(!value_contains_creation_id(&cat_off, cid));

    // Re-submit & re-approve -> both public catalog AND CAS media access restored!
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    let (st_media, bytes) = download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_media, 200);
    assert_eq!(bytes, png);
    let (st_cat, cat_on) = driver.json_req(Method::GET, "/api/v1/catalog/books?q=Phoenix", Some(&consumer.token), None).await?;
    assert_eq!(st_cat, 200);
    assert!(value_contains_creation_id(&cat_on, cid));

    server.stop().await;
    Ok(())
}
