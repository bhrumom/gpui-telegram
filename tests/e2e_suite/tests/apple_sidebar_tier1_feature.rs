mod apple_sidebar_common;

use apple_sidebar_common::*;
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;

// ============================================================================
// Area 1: Cloud Creations CRUD (F1, F4 — 6 tests: t1_01..t1_06)
// ============================================================================

#[tokio::test]
async fn t1_01_create_and_fetch_book_cloud_draft() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "book_author").await?;

    let req = make_book_creation(
        "The Glass Observatory",
        "A serene sci-fi novella exploring lunar telescopes and deep time.",
        "Clara Lin",
        "en-US",
        &["Sci-Fi", "Fiction"],
        &["space", "observatory"],
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, req).await?;
    let cid = created["creation_id"].as_str().unwrap();
    assert_eq!(created["kind"], "book");
    assert_eq!(created["owner_user_id"].as_u64(), Some(creator.user_id));
    assert_eq!(created["review_status"], "draft");
    assert_eq!(created["visibility"], "private");
    assert!(created["cloud_revision"].as_i64().unwrap_or(0) >= 1);
    assert_eq!(created["version"].as_u64(), Some(1));
    assert_eq!(created["has_live_published_version"], false);

    let (st, fetched) = get_creation_raw(&driver, Some(&creator.token), cid).await?;
    assert_eq!(st, 200, "owner fetch book failed: {fetched}");
    assert_eq!(fetched["creation_id"], cid);
    assert_eq!(fetched["title"], "The Glass Observatory");
    assert_eq!(fetched["payload"]["kind"], "book");
    assert_eq!(
        fetched["payload"]["chapters"].as_array().map(|c| c.len()),
        Some(1)
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_02_create_and_fetch_music_cloud_draft() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "music_maker").await?;

    let req = make_music_creation(
        "Midnight Harbor Synth",
        "Ambient electronic composition with warm piano and synced lyrics.",
        "Kai Sato",
        "en-US",
        &["Electronic", "Ambient"],
        &["synth", "night"],
        None,
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, req).await?;
    let cid = created["creation_id"].as_str().unwrap();
    assert_eq!(created["kind"], "music");
    assert_eq!(created["review_status"], "draft");
    assert_eq!(created["visibility"], "private");
    assert!(created["cloud_revision"].as_i64().unwrap_or(0) >= 1);

    let (st, fetched) = get_creation_raw(&driver, Some(&creator.token), cid).await?;
    assert_eq!(st, 200, "owner fetch music failed: {fetched}");
    assert_eq!(fetched["payload"]["kind"], "music");
    assert_eq!(fetched["payload"]["bpm"], 118);
    let tracks = fetched["payload"]["tracks"]
        .as_array()
        .expect("music tracks array");
    assert_eq!(tracks.len(), 1);
    assert!(tracks[0]["melody_notes"].as_array().unwrap().len() >= 4);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_03_create_and_fetch_game_cloud_draft() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "game_dev").await?;

    let req = make_game_creation(
        "Orbit Trivia Quest",
        "Fast-paced space and music trivia game with global leaderboards.",
        "Nova Arcade",
        "en-US",
        &["Trivia", "Arcade"],
        &["quiz", "orbit"],
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, req).await?;
    let cid = created["creation_id"].as_str().unwrap();
    assert_eq!(created["kind"], "game");
    assert_eq!(created["review_status"], "draft");
    assert_eq!(created["visibility"], "private");

    let (st, fetched) = get_creation_raw(&driver, Some(&creator.token), cid).await?;
    assert_eq!(st, 200, "owner fetch game failed: {fetched}");
    assert_eq!(fetched["payload"]["kind"], "game");
    assert_eq!(fetched["payload"]["supports_leaderboard"], true);
    assert_eq!(
        fetched["payload"]["spec"]["questions"]
            .as_array()
            .map(|q| q.len()),
        Some(2)
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_04_create_and_fetch_app_cloud_draft() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "app_dev").await?;

    let req = make_app_creation(
        "Focus Flow Studio",
        "Minimalist Apple-styled Pomodoro planner with cloud sync.",
        "Cupertino Labs",
        "en-US",
        &["Productivity"],
        &["timer", "focus"],
        None,
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, req).await?;
    let cid = created["creation_id"].as_str().unwrap();
    assert_eq!(created["kind"], "app");
    assert_eq!(created["review_status"], "draft");
    assert_eq!(created["visibility"], "private");

    let (st, fetched) = get_creation_raw(&driver, Some(&creator.token), cid).await?;
    assert_eq!(st, 200, "owner fetch app failed: {fetched}");
    assert_eq!(fetched["payload"]["kind"], "app");
    assert_eq!(fetched["payload"]["version_string"], "1.0.0");
    assert_eq!(fetched["payload"]["developer_name"], "Cupertino Labs");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_05_list_my_creations_filters_by_kind_status_and_query() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "filter_owner").await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "Solar Atlas Chronicle",
            "Comprehensive planetary history across the outer rim.",
            "Sora",
            "en-US",
            &["Sci-Fi"],
            &["solar"],
            None,
        ),
    )
    .await?;
    let music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation(
            "Velvet Rain Suite",
            "Atmospheric lo-fi piano sketches for rainy evenings.",
            "Sora",
            "en-US",
            &["Ambient"],
            &["rain"],
            None,
            None,
        ),
    )
    .await?;
    let book_id = book["creation_id"].as_str().unwrap();
    let music_id = music["creation_id"].as_str().unwrap();

    let (st_sub, sub_body) = submit_review_raw(&driver, &creator.token, book_id).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_body, "t1_05 book submit")?;

    let (st_kind, by_kind) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=book", Some(&creator.token), None)
        .await?;
    assert_eq!(st_kind, 200, "list by kind failed: {by_kind}");
    let items_kind = extract_items(&by_kind)?;
    assert_eq!(items_kind.len(), 1);
    assert_eq!(items_kind[0]["creation_id"], book_id);
    assert!(by_kind["latest_revision"].as_i64().unwrap_or(0) >= 2);

    let (st_status, by_status) = driver
        .json_req(
            Method::GET,
            "/api/v1/creations/my?status=pending_review",
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st_status, 200);
    let items_status = extract_items(&by_status)?;
    assert_eq!(items_status.len(), 1);
    assert_eq!(items_status[0]["creation_id"], book_id);

    let (st_q, by_q) = driver
        .json_req(
            Method::GET,
            "/api/v1/creations/my?q=Velvet",
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st_q, 200);
    let items_q = extract_items(&by_q)?;
    assert_eq!(items_q.len(), 1);
    assert_eq!(items_q[0]["creation_id"], music_id);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_06_update_and_soft_delete_cloud_creation_preserves_tombstone() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "crud_owner").await?;

    let mut payload = make_book_creation(
        "Draft Zero",
        "Initial story outline about an orbital library.",
        "Author One",
        "en-US",
        &["Fiction"],
        &["draft"],
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, payload.clone()).await?;
    let cid = created["creation_id"].as_str().unwrap();
    let rev1 = created["cloud_revision"].as_i64().unwrap();

    payload["title"] = json!("Draft Revised");
    payload["expected_revision"] = json!(rev1);
    let (st_upd, updated) = update_creation_raw(&driver, &creator.token, cid, payload).await?;
    assert_eq!(st_upd, 200, "update creation failed: {updated}");
    assert_eq!(updated["title"], "Draft Revised");
    let rev2 = updated["cloud_revision"].as_i64().unwrap();
    assert!(rev2 > rev1, "expected cloud_revision to increase: {rev1} -> {rev2}");

    let (st_del, del_body) = delete_creation_raw(&driver, &creator.token, cid).await?;
    assert!(st_del == 200 || st_del == 204, "delete failed {st_del}: {del_body}");

    let (st_my, my_body) = driver
        .json_req(Method::GET, "/api/v1/creations/my", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    assert!(
        !extract_items(&my_body)?
            .iter()
            .any(|i| i["creation_id"].as_str() == Some(cid) && i["deleted_at_ms"].is_null()),
        "deleted item should not remain active in /creations/my: {my_body}"
    );

    let (st_sync, sync_body) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/sync?since_revision={rev2}"),
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st_sync, 200, "sync after delete failed: {sync_body}");
    let items = extract_items(&sync_body)?;
    let tombstone = items
        .iter()
        .find(|i| i["creation_id"].as_str() == Some(cid))
        .expect("tombstone present in delta sync");
    assert!(
        tombstone["deleted_at_ms"].as_i64().is_some(),
        "tombstone must carry deleted_at_ms: {tombstone}"
    );

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 2: Cloud Delta Sync (F4 — 5 tests: t1_07..t1_11)
// ============================================================================

#[tokio::test]
async fn t1_07_sync_from_zero_returns_all_creations_and_monotonic_revision() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "sync_zero").await?;

    let c1 = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Sync Book 1", "First book stored in user cloud.", "A", "en-US", &["Sci-Fi"], &["s1"], None),
    )
    .await?;
    let c2 = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Sync Track 2", "Second track stored in user cloud.", "A", "en-US", &["Pop"], &["s2"], None, None),
    )
    .await?;
    let r1 = c1["cloud_revision"].as_i64().unwrap();
    let r2 = c2["cloud_revision"].as_i64().unwrap();
    assert!(r2 > r1, "revisions must strictly increase: {r1} vs {r2}");

    let (st, body) = driver
        .json_req(Method::GET, "/api/v1/creations/sync?since_revision=0", Some(&creator.token), None)
        .await?;
    assert_eq!(st, 200, "sync from 0 failed: {body}");
    assert_eq!(body["latest_revision"].as_i64(), Some(r2));
    let items = extract_items(&body)?;
    assert_eq!(items.len(), 2);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_08_sync_incremental_returns_only_newer_revisions() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "sync_inc").await?;

    let c1 = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Older Book", "Older book before sync watermark.", "A", "en-US", &["Fiction"], &["old"], None),
    )
    .await?;
    let r1 = c1["cloud_revision"].as_i64().unwrap();

    let c2 = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Newer Game", "Newer game created after watermark.", "A", "en-US", &["Trivia"], &["new"], None),
    )
    .await?;
    let cid2 = c2["creation_id"].as_str().unwrap();

    let (st, body) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/sync?since_revision={r1}"),
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st, 200, "incremental sync failed: {body}");
    let items = extract_items(&body)?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["creation_id"], cid2);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_09_sync_captures_update_incrementing_cloud_revision() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "sync_upd").await?;

    let mut payload = make_music_creation(
        "Echoes v1",
        "Minimalist synth piece ready for iteration.",
        "Composer",
        "en-US",
        &["Electronic"],
        &["echo"],
        None,
        None,
    );
    let c1 = create_creation_ok(&driver, &creator.token, payload.clone()).await?;
    let cid = c1["creation_id"].as_str().unwrap();
    let r1 = c1["cloud_revision"].as_i64().unwrap();

    payload["title"] = json!("Echoes v2 Master");
    payload["expected_revision"] = json!(r1);
    let (st_upd, upd) = update_creation_raw(&driver, &creator.token, cid, payload).await?;
    assert_eq!(st_upd, 200, "update failed: {upd}");
    let r2 = upd["cloud_revision"].as_i64().unwrap();
    assert!(r2 > r1);

    let (st_sync, sync_body) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/sync?since_revision={r1}"),
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st_sync, 200);
    assert_eq!(sync_body["latest_revision"].as_i64(), Some(r2));
    let items = extract_items(&sync_body)?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Echoes v2 Master");
    assert_eq!(items[0]["cloud_revision"].as_i64(), Some(r2));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_10_sync_delivers_soft_delete_tombstone_with_deleted_at_ms() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "sync_del").await?;

    let c1 = create_creation_ok(
        &driver,
        &creator.token,
        make_app_creation("Transient Utility", "Utility destined for soft-delete tombstone sync.", "Dev", "en-US", &["Utilities"], &["tmp"], None, None),
    )
    .await?;
    let cid = c1["creation_id"].as_str().unwrap();
    let r1 = c1["cloud_revision"].as_i64().unwrap();

    let (st_del, _) = delete_creation_raw(&driver, &creator.token, cid).await?;
    assert!(st_del == 200 || st_del == 204);

    let (st_sync, sync_body) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/sync?since_revision={r1}"),
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st_sync, 200);
    let items = extract_items(&sync_body)?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["creation_id"], cid);
    assert!(items[0]["deleted_at_ms"].as_i64().unwrap_or(0) > 0);
    assert!(sync_body["latest_revision"].as_i64().unwrap() > r1);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_11_sync_at_latest_revision_returns_empty_delta() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "sync_empty").await?;

    let c1 = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Steady Book", "Steady state book for empty delta verification.", "A", "en-US", &["Fiction"], &["steady"], None),
    )
    .await?;
    let rev = c1["cloud_revision"].as_i64().unwrap();

    let (st, body) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/sync?since_revision={rev}"),
            Some(&creator.token),
            None,
        )
        .await?;
    assert_eq!(st, 200);
    assert_eq!(body["latest_revision"].as_i64(), Some(rev));
    assert!(extract_items(&body)?.is_empty());

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 3: Review & Publishing FSM (F5 — 6 tests: t1_12..t1_17)
// ============================================================================

#[tokio::test]
async fn t1_12_submit_valid_creation_transitions_draft_to_pending_review() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "fsm_sub").await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Review Candidate", "Polished book ready for editorial review.", "A", "en-US", &["Fiction"], &["review"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();

    let (st, sub) = submit_review_raw(&driver, &creator.token, cid).await?;
    let sub_dto = assert_submit_review_ok(st, &sub, "t1_12 submit")?;
    assert_eq!(sub_dto["review_status"], "pending_review");
    assert!(sub_dto["submitted_at_ms"].as_i64().unwrap_or(0) > 0);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_13_withdraw_review_returns_pending_creation_to_draft() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "fsm_withdraw").await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Pending Single", "Track submitted then withdrawn for edits.", "A", "en-US", &["Pop"], &["withdraw"], None, None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();

    let (st_sub, sub_body) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_body, "t1_13 submit")?;

    let (st_wd, withdrawn) = withdraw_review_raw(&driver, &creator.token, cid).await?;
    assert_eq!(st_wd, 200, "withdraw-review failed: {withdrawn}");
    assert_eq!(withdrawn["review_status"], "draft");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_14_admin_review_queue_lists_pending_submissions_by_kind() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Queue Book", "Pending book awaiting moderation.", "Author", "en-US", &["Fiction"], &["qbook"], None),
    )
    .await?;
    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Queue Game", "Pending game awaiting moderation.", "Dev", "en-US", &["Trivia"], &["qgame"], None),
    )
    .await?;
    let book_id = book["creation_id"].as_str().unwrap();
    let game_id = game["creation_id"].as_str().unwrap();
    let (st_sb, sb_val) = submit_review_raw(&driver, &creator.token, book_id).await?;
    let _ = assert_submit_review_ok(st_sb, &sb_val, "t1_14 book submit")?;
    let (st_sg, sg_val) = submit_review_raw(&driver, &creator.token, game_id).await?;
    let _ = assert_submit_review_ok(st_sg, &sg_val, "t1_14 game submit")?;

    let (st_all, all_q) = get_review_queue_raw(&driver, &admin.token, "status=pending_review").await?;
    assert_eq!(st_all, 200, "review queue failed: {all_q}");
    let all_items = extract_items(&all_q)?;
    assert!(all_items.iter().any(|i| i["creation_id"] == book_id));
    assert!(all_items.iter().any(|i| i["creation_id"] == game_id));

    let (st_books, book_q) =
        get_review_queue_raw(&driver, &admin.token, "status=pending_review&kind=book").await?;
    assert_eq!(st_books, 200);
    let book_items = extract_items(&book_q)?;
    assert!(book_items.iter().any(|i| i["creation_id"] == book_id));
    assert!(!book_items.iter().any(|i| i["creation_id"] == game_id));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_15_admin_approve_public_publishes_creation_with_timestamps() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Approved Chronicle", "Story approved for public Apple Books.", "Author", "en-US", &["Sci-Fi"], &["pub"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    let approved = publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    assert_eq!(approved["review_status"], "published");
    assert_eq!(approved["visibility"], "public");
    assert_eq!(approved["has_live_published_version"], true);
    assert_eq!(approved["published_version"].as_u64(), Some(1));
    assert!(approved["reviewed_at_ms"].as_i64().unwrap_or(0) > 0);
    assert!(approved["published_at_ms"].as_i64().unwrap_or(0) > 0);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_16_admin_reject_records_reason_notes_and_allows_resubmit() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Needs Polish Game", "Arcade trivia needing clearer instructions.", "Dev", "en-US", &["Trivia"], &["rej"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    let (st_sub0, sub0) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub0, &sub0, "t1_16 initial submit")?;

    let (st_rej, rejected) = review_decision_raw(
        &driver,
        &admin.token,
        cid,
        json!({
            "decision": "reject",
            "reason": "Please expand rules_markdown before public release",
            "notes": "Re-submit after adding scoring explanation."
        }),
    )
    .await?;
    assert_eq!(st_rej, 200, "reject review failed: {rejected}");
    assert_eq!(rejected["review_status"], "rejected");
    assert_eq!(
        rejected["rejection_reason"].as_str(),
        Some("Please expand rules_markdown before public release")
    );
    assert!(rejected["reviewed_at_ms"].as_i64().unwrap_or(0) > 0);

    let (st_resub, resub) = submit_review_raw(&driver, &creator.token, cid).await?;
    let resub_dto = assert_submit_review_ok(st_resub, &resub, "t1_16 resubmit")?;
    assert_eq!(resub_dto["review_status"], "pending_review");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_17_owner_unpublish_and_audit_trail_records_all_transitions() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Audited Book", "Book traversing submit, approve, and unpublish.", "A", "en-US", &["History"], &["audit"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st_unpub, unpub) = unpublish_raw(&driver, &creator.token, cid).await?;
    assert_eq!(st_unpub, 200, "unpublish failed: {unpub}");
    assert_eq!(unpub["review_status"], "unpublished");
    assert_eq!(unpub["visibility"], "private");

    let (st_audit, audit_body) = get_audit_raw(&driver, &creator.token, cid).await?;
    assert_eq!(st_audit, 200, "get audit failed: {audit_body}");
    let entries = extract_items(&audit_body)?;
    assert!(
        entries.len() >= 3,
        "expected >= 3 audit entries (submit, approve, unpublish), got: {audit_body}"
    );
    let submit_audit = entries
        .iter()
        .find(|e| e["to_status"].as_str() == Some("pending_review"))
        .expect("missing pending_review audit entry");
    assert_eq!(submit_audit["from_status"].as_str(), Some("draft"));
    assert_eq!(submit_audit["actor_user_id"].as_u64(), Some(creator.user_id));
    assert!(submit_audit["created_at_ms"].as_i64().unwrap_or(0) > 0);

    let approve_audit = entries
        .iter()
        .find(|e| e["to_status"].as_str() == Some("published"))
        .expect("missing published audit entry");
    assert_eq!(approve_audit["from_status"].as_str(), Some("pending_review"));
    assert_eq!(approve_audit["actor_user_id"].as_u64(), Some(admin.user_id));
    assert!(approve_audit["created_at_ms"].as_i64().unwrap_or(0) > 0);

    let unpub_audit = entries
        .iter()
        .find(|e| e["to_status"].as_str() == Some("unpublished"))
        .expect("missing unpublished audit entry");
    assert_eq!(unpub_audit["from_status"].as_str(), Some("published"));
    assert_eq!(unpub_audit["actor_user_id"].as_u64(), Some(creator.user_id));
    assert!(unpub_audit["created_at_ms"].as_i64().unwrap_or(0) > 0);

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 4: Vertical Catalogs & Search/Ranking (F6 — 6 tests: t1_18..t1_23)
// ============================================================================

#[tokio::test]
async fn t1_18_catalog_books_returns_published_public_books() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Nordic Starlight", "Published Apple Books adventure across fjords.", "Freya", "en-US", &["Adventure"], &["nordic"], None),
    )
    .await?;
    let book_id = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, book_id).await?;

    let (st, body) = driver
        .json_req(Method::GET, "/api/v1/catalog/books", Some(&consumer.token), None)
        .await?;
    assert_eq!(st, 200, "catalog/books failed: {body}");
    let items = extract_items(&body)?;
    let found = items
        .iter()
        .find(|i| i["creation_id"].as_str() == Some(book_id))
        .expect("published book must appear in /api/v1/catalog/books");
    assert_eq!(found["kind"], "book");
    assert_eq!(found["title"], "Nordic Starlight");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_19_catalog_music_returns_published_public_music() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Neon Sakura Nocturne", "Published Apple Music chillwave album.", "Yuki", "en-US", &["Electronic"], &["sakura"], None, None),
    )
    .await?;
    let music_id = music["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, music_id).await?;

    let (st, body) = driver
        .json_req(Method::GET, "/api/v1/catalog/music", Some(&consumer.token), None)
        .await?;
    assert_eq!(st, 200, "catalog/music failed: {body}");
    let items = extract_items(&body)?;
    let found = items
        .iter()
        .find(|i| i["creation_id"].as_str() == Some(music_id))
        .expect("published music must appear in /api/v1/catalog/music");
    assert_eq!(found["kind"], "music");
    assert_eq!(found["title"], "Neon Sakura Nocturne");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_20_catalog_games_returns_published_public_games() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Starlight Trivia Arena", "Published Apple Games trivia showdown.", "ArcadeForge", "en-US", &["Trivia"], &["arena"], None),
    )
    .await?;
    let game_id = game["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, game_id).await?;

    let (st, body) = driver
        .json_req(Method::GET, "/api/v1/catalog/games", Some(&consumer.token), None)
        .await?;
    assert_eq!(st, 200, "catalog/games failed: {body}");
    let items = extract_items(&body)?;
    let found = items
        .iter()
        .find(|i| i["creation_id"].as_str() == Some(game_id))
        .expect("published game must appear in /api/v1/catalog/games");
    assert_eq!(found["kind"], "game");
    assert_eq!(found["title"], "Starlight Trivia Arena");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_21_catalog_appstore_returns_published_apps_and_featured_verticals() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let app = create_creation_ok(
        &driver,
        &creator.token,
        make_app_creation("Zen Notes Pro", "Apple App Store writing companion.", "ZenCraft", "en-US", &["Productivity"], &["notes"], None, None),
    )
    .await?;
    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Featured Horizon Book", "Featured sci-fi book visible on App Store showcase.", "Author", "en-US", &["Sci-Fi"], &["featured"], None),
    )
    .await?;
    let app_id = app["creation_id"].as_str().unwrap();
    let book_id = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, app_id).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, book_id).await?;

    let (st, body) = driver
        .json_req(Method::GET, "/api/v1/catalog/appstore", Some(&consumer.token), None)
        .await?;
    assert_eq!(st, 200, "catalog/appstore failed: {body}");
    assert!(
        value_contains_creation_id(&body, app_id),
        "published App creation must appear in /api/v1/catalog/appstore: {body}"
    );
    assert!(
        value_contains_creation_id(&body, book_id),
        "featured vertical creation should appear in /api/v1/catalog/appstore: {body}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_22_catalog_search_filters_by_english_and_cjk_keywords_and_genre() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let en_book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Quantum Reef Expedition", "Deep-sea quantum computing expedition chronicle.", "Dr. Lee", "en-US", &["Science"], &["quantum", "ocean"], None),
    )
    .await?;
    let zh_book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("星际漫游指南", "一部探索银河边缘与星际文明的硬核中文科幻小说。", "林深", "zh-CN", &["科幻"], &["银河", "科幻"], None),
    )
    .await?;
    let en_id = en_book["creation_id"].as_str().unwrap();
    let zh_id = zh_book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, en_id).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, zh_id).await?;

    let (st_en, res_en) = driver
        .json_req(
            Method::GET,
            "/api/v1/catalog?kind=book&q=Quantum&genre=Science",
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st_en, 200, "EN search failed: {res_en}");
    let en_items = extract_items(&res_en)?;
    assert!(en_items.iter().any(|i| i["creation_id"] == en_id));
    assert!(!en_items.iter().any(|i| i["creation_id"] == zh_id));

    let zh_q = percent_encode("科幻");
    let (st_zh, res_zh) = driver
        .json_req(
            Method::GET,
            &format!("/api/v1/catalog?kind=book&q={zh_q}"),
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st_zh, 200, "CJK search failed: {res_zh}");
    let zh_items = extract_items(&res_zh)?;
    assert!(
        zh_items.iter().any(|i| i["creation_id"] == zh_id),
        "CJK query must find published Chinese book: {res_zh}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_23_catalog_sorting_supports_featured_popular_top_rated_newest_and_pagination() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let consumer2 = register_actor(&driver, "sort_saver_two").await?;

    let mut ids = Vec::new();
    for idx in 1..=3 {
        let b = create_creation_ok(
            &driver,
            &creator.token,
            make_book_creation(
                &format!("Sortable Volume {idx}"),
                &format!("Volume {idx} published for sorting and pagination verification."),
                "Author",
                "en-US",
                &["Fiction"],
                &["sort"],
                None,
            ),
        )
        .await?;
        let cid = b["creation_id"].as_str().unwrap().to_string();
        publish_creation_public(&driver, &creator.token, &admin.token, &cid).await?;
        ids.push(cid);
        if idx < 3 {
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    // Give Volume 2 a 5-star review and Volume 1 a 2-star review so top_rated ordering is deterministic.
    let (st_r2, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{}/reviews", ids[1]),
            Some(&consumer.token),
            Some(json!({ "rating": 5, "title": "Masterpiece", "body": "Unforgettable pacing." })),
        )
        .await?;
    assert!(st_r2 == 200 || st_r2 == 201);
    let (st_r1, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{}/reviews", ids[0]),
            Some(&consumer.token),
            Some(json!({ "rating": 2, "title": "Okay", "body": "Slow opening." })),
        )
        .await?;
    assert!(st_r1 == 200 || st_r1 == 201);

    // Give Volume 3 two library saves from independent users so popular ordering is deterministic.
    for tok in [&consumer.token, &consumer2.token] {
        let (st_save, save_val) = driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{}/state", ids[2]),
                Some(tok),
                Some(json!({ "in_library": true, "favorite": true })),
            )
            .await?;
        assert_eq!(st_save, 200, "save state failed: {save_val}");
    }

    for sort in ["featured", "popular", "top_rated", "newest"] {
        let (st, body) = driver
            .json_req(
                Method::GET,
                &format!("/api/v1/catalog?kind=book&sort={sort}&limit=10&offset=0"),
                Some(&consumer.token),
                None,
            )
            .await?;
        assert_eq!(st, 200, "sort={sort} failed: {body}");
        let items = extract_items(&body)?;
        assert_eq!(items.len(), 3, "sort={sort} must return all 3 published books: {body}");
        match sort {
            "top_rated" => {
                assert_eq!(
                    items[0]["creation_id"].as_str(),
                    Some(ids[1].as_str()),
                    "5-star book should rank first in top_rated: {body}"
                );
            }
            "popular" => {
                assert_eq!(
                    items[0]["creation_id"].as_str(),
                    Some(ids[2].as_str()),
                    "most-saved book (2 library saves) should rank first in popular: {body}"
                );
            }
            "newest" => {
                assert_eq!(
                    items[0]["creation_id"].as_str(),
                    Some(ids[2].as_str()),
                    "most recently published book should rank first in newest: {body}"
                );
                let ts: Vec<i64> = items
                    .iter()
                    .map(|i| i["published_at_ms"].as_i64().unwrap_or(0))
                    .collect();
                assert!(
                    ts[0] >= ts[1] && ts[1] >= ts[2],
                    "newest sort must order published_at_ms descending, got {ts:?}: {body}"
                );
            }
            _ => {}
        }
    }

    let (st_p1, page1) = driver
        .json_req(Method::GET, "/api/v1/catalog?kind=book&sort=newest&limit=2&offset=0", Some(&consumer.token), None)
        .await?;
    let (st_p2, page2) = driver
        .json_req(Method::GET, "/api/v1/catalog?kind=book&sort=newest&limit=2&offset=2", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_p1, 200);
    assert_eq!(st_p2, 200);
    let p1_items = extract_items(&page1)?;
    let p2_items = extract_items(&page2)?;
    assert_eq!(p1_items.len(), 2);
    assert_eq!(p2_items.len(), 1);
    let p1_ids: HashSet<String> = p1_items
        .iter()
        .filter_map(|i| i["creation_id"].as_str().map(str::to_string))
        .collect();
    let p2_ids: HashSet<String> = p2_items
        .iter()
        .filter_map(|i| i["creation_id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(p1_ids.len(), 2, "page1 must contain 2 distinct IDs: {page1}");
    assert_eq!(p2_ids.len(), 1, "page2 must contain 1 distinct ID: {page2}");
    assert!(
        p1_ids.is_disjoint(&p2_ids),
        "page1 and page2 must be strictly disjoint: p1={p1_ids:?}, p2={p2_ids:?}"
    );
    let expected_all: HashSet<String> = ids.iter().cloned().collect();
    let union_ids: HashSet<String> = p1_ids.union(&p2_ids).cloned().collect();
    assert_eq!(
        union_ids, expected_all,
        "page1 U page2 must cover all 3 published books"
    );

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 5: Per-User States, Ratings & Leaderboards (F6 — 5 tests: t1_24..t1_28)
// ============================================================================

#[tokio::test]
async fn t1_24_user_book_reading_progress_and_bookmarks_cloud_state() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Reader Progress Novel", "Book for testing reading progress and bookmarks.", "Author", "en-US", &["Fiction"], &["reader"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st_put, put_val) = driver
        .json_req(
            Method::PUT,
            &format!("/api/v1/catalog/{cid}/state"),
            Some(&consumer.token),
            Some(json!({
                "in_library": true,
                "progress_percent": 64.5,
                "state_json": {
                    "current_chapter_index": 1,
                    "current_page_index": 3,
                    "progress_percent": 64.5,
                    "reader_theme": "sepia",
                    "reader_font_scale": 1.25,
                    "bookmarks": [
                        {
                            "bookmark_id": "bm_1",
                            "chapter_index": 1,
                            "page_index": 3,
                            "note": "Great scene",
                            "created_at_ms": 1700000000000i64
                        }
                    ],
                    "in_library": true
                }
            })),
        )
        .await?;
    assert_eq!(st_put, 200, "put book state failed: {put_val}");

    let (st_get, states_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/my-states", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_get, 200, "get my-states failed: {states_val}");
    let states = extract_items(&states_val)?;
    let entry = states
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(cid))
        .expect("book state persisted in /catalog/my-states");
    assert_eq!(entry["in_library"], true);
    assert!((entry["progress_percent"].as_f64().unwrap_or(0.0) - 64.5).abs() < 0.5);
    assert!(value_contains_substring(&entry["state_json"], "bm_1"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_25_user_music_loved_playback_position_and_library_state() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Cloud Synth Anthem", "Music track for testing loved and playback position state.", "DJ", "en-US", &["Electronic"], &["anthem"], None, None),
    )
    .await?;
    let cid = music["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st_put, put_val) = driver
        .json_req(
            Method::PUT,
            &format!("/api/v1/catalog/{cid}/state"),
            Some(&consumer.token),
            Some(json!({
                "in_library": true,
                "favorite": true,
                "progress_percent": 40.0,
                "state_json": {
                    "loved": true,
                    "in_library": true,
                    "last_track_id": "trk_01",
                    "last_position_ms": 16800,
                    "play_count": 4
                }
            })),
        )
        .await?;
    assert_eq!(st_put, 200, "put music state failed: {put_val}");

    let (st_get, states_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/my-states", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_get, 200);
    let entry = extract_items(&states_val)?
        .iter()
        .find(|s| s["creation_id"].as_str() == Some(cid))
        .expect("music state persisted");
    assert_eq!(entry["in_library"], true);
    assert_eq!(entry["favorite"], true);
    assert!((entry["progress_percent"].as_f64().unwrap_or(0.0) - 40.0).abs() < 0.5);
    assert_eq!(entry["state_json"]["last_track_id"], "trk_01");
    assert_eq!(entry["state_json"]["last_position_ms"].as_u64(), Some(16800));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_26_user_game_save_state_and_app_install_state_in_my_states() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Cloud Save Puzzle", "Game for testing save state persistence.", "Dev", "en-US", &["Trivia"], &["save"], None),
    )
    .await?;
    let app = create_creation_ok(
        &driver,
        &creator.token,
        make_app_creation("Cloud Install App", "App for testing installed state persistence.", "Dev", "en-US", &["Productivity"], &["install"], None, None),
    )
    .await?;
    let gid = game["creation_id"].as_str().unwrap();
    let aid = app["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, gid).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, aid).await?;

    assert_eq!(
        driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{gid}/state"),
                Some(&consumer.token),
                Some(json!({
                    "in_library": true,
                    "state_json": {
                        "high_score": 420,
                        "play_count": 2,
                        "unlocked_achievements": ["ach_first_clear"],
                        "save_state_json": { "current_level_id": "lvl_03" },
                        "in_library": true
                    }
                })),
            )
            .await?
            .0,
        200
    );
    assert_eq!(
        driver
            .json_req(
                Method::PUT,
                &format!("/api/v1/catalog/{aid}/state"),
                Some(&consumer.token),
                Some(json!({
                    "in_library": true,
                    "state_json": {
                        "is_installed": true,
                        "installed_version": "1.0.0"
                    }
                })),
            )
            .await?
            .0,
        200
    );

    let (st_get, states_val) = driver
        .json_req(Method::GET, "/api/v1/catalog/my-states", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_get, 200);
    let states = extract_items(&states_val)?;
    let g_state = states.iter().find(|s| s["creation_id"] == gid).expect("game state");
    let a_state = states.iter().find(|s| s["creation_id"] == aid).expect("app state");
    assert_eq!(g_state["in_library"], true);
    assert!(value_contains_substring(&g_state["state_json"], "lvl_03"));
    assert_eq!(a_state["in_library"], true);
    assert_eq!(a_state["state_json"]["is_installed"], true);
    assert_eq!(a_state["state_json"]["installed_version"], "1.0.0");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_27_public_reviews_and_ratings_update_catalog_rating_avg_and_count() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer1, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let consumer2 = register_actor(&driver, "reviewer_two").await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Rated Masterwork", "Book receiving multiple user ratings.", "Author", "en-US", &["Fiction"], &["rated"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st1, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{cid}/reviews"),
            Some(&consumer1.token),
            Some(json!({ "rating": 5, "title": "Superb", "body": "Loved every chapter." })),
        )
        .await?;
    let (st2, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{cid}/reviews"),
            Some(&consumer2.token),
            Some(json!({ "rating": 3, "title": "Solid", "body": "Good pacing overall." })),
        )
        .await?;
    assert!(st1 == 200 || st1 == 201);
    assert!(st2 == 200 || st2 == 201);

    let (st_rev, rev_list) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{cid}/reviews"), Some(&consumer1.token), None)
        .await?;
    assert_eq!(st_rev, 200, "get reviews failed: {rev_list}");
    assert_eq!(extract_items(&rev_list)?.len(), 2);

    let (st_cat, cat_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/books", Some(&consumer1.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    let item = extract_items(&cat_body)?
        .iter()
        .find(|i| i["creation_id"] == cid)
        .expect("book in catalog");
    assert_eq!(item["rating_count"].as_u64(), Some(2));
    assert!((item["rating_avg"].as_f64().unwrap_or(0.0) - 4.0).abs() < 0.05);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_28_published_game_scores_populate_sorted_leaderboard() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, p1, admin) = seed_creator_consumer_admin(&server, &driver).await?;
    let p2 = register_actor(&driver, "player_two").await?;

    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Leaderboard Quiz", "Published trivia game with persistent high scores.", "Dev", "en-US", &["Trivia"], &["lb"], None),
    )
    .await?;
    let gid = game["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, gid).await?;

    let (st1, _) = driver
        .json_req(Method::POST, &format!("/api/v1/catalog/{gid}/scores"), Some(&p1.token), Some(json!({ "score": 850 })))
        .await?;
    let (st2, _) = driver
        .json_req(Method::POST, &format!("/api/v1/catalog/{gid}/scores"), Some(&p2.token), Some(json!({ "score": 1420 })))
        .await?;
    assert!(st1 == 200 || st1 == 201);
    assert!(st2 == 200 || st2 == 201);

    let (st_lb, lb_body) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{gid}/leaderboard"), Some(&p1.token), None)
        .await?;
    assert_eq!(st_lb, 200, "get leaderboard failed: {lb_body}");
    let entries = extract_items(&lb_body)?;
    assert_eq!(entries.len(), 2);
    let score0 = entries[0]["best_score"].as_i64().or_else(|| entries[0]["score"].as_i64()).unwrap();
    let score1 = entries[1]["best_score"].as_i64().or_else(|| entries[1]["score"].as_i64()).unwrap();
    assert_eq!(score0, 1420);
    assert_eq!(score1, 850);
    assert_eq!(entries[0]["user_id"].as_u64(), Some(p2.user_id));

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 6: Bot `@CreatorStudioBot` Creation Commands (F7 — 6 tests: t1_29..t1_34)
// ============================================================================

#[tokio::test]
async fn t1_29_creator_studio_bot_discoverable_and_responds_to_start_help() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "bot_discover").await?;

    let (st_res, resolved) = driver
        .json_req(Method::GET, "/api/v1/peers/resolve?username=CreatorStudioBot", Some(&creator.token), None)
        .await?;
    assert_eq!(st_res, 200, "peers/resolve CreatorStudioBot failed: {resolved}");
    assert_eq!(resolved["user"]["user_id"].as_u64(), Some(CREATOR_STUDIO_BOT_UID));
    assert_eq!(resolved["user"]["is_bot"], true);

    let (st_search, search_val) = driver
        .json_req(Method::GET, "/api/v1/search?q=CreatorStudioBot&category=bots", Some(&creator.token), None)
        .await?;
    assert_eq!(st_search, 200, "search CreatorStudioBot failed: {search_val}");
    assert!(value_contains_substring(&search_val, "CreatorStudioBot"));

    let chat_id = open_creator_bot_dm(&driver, &creator.token).await?;
    let start_reply = send_bot_command_and_poll_reply(&driver, &creator.token, chat_id, "/start").await?;
    let start_lower = start_reply["text"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(
        start_lower.contains("/newbook")
            || start_lower.contains("/recommend")
            || start_lower.contains("creatorstudio"),
        "expected /start reply to reference CreatorStudio or studio commands, got: {start_reply}"
    );

    let help_reply = send_bot_command_and_poll_reply(&driver, &creator.token, chat_id, "/help").await?;
    let help_txt = help_reply["text"].as_str().unwrap_or("");
    assert!(
        help_txt.contains("/newbook")
            && help_txt.contains("/newmusic")
            && help_txt.contains("/newgame")
            && help_txt.contains("/recommend"),
        "expected /help reply to advertise creation & recommend commands, got: {help_reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_30_bot_newbook_creates_cloud_book_draft_with_card_and_deep_link() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "bot_book").await?;
    let chat_id = open_creator_bot_dm(&driver, &creator.token).await?;

    let reply = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        chat_id,
        "/newbook Lunar Gardens | A botanical sci-fi story set inside glass lunar domes",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=book", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let items = extract_items(&my_val)?;
    assert_eq!(items.len(), 1, "expected 1 book draft from /newbook: {my_val}");
    let cid = items[0]["creation_id"].as_str().unwrap();
    assert_eq!(items[0]["review_status"], "draft");
    assert_eq!(items[0]["creator_bot_user_id"].as_u64(), Some(CREATOR_STUDIO_BOT_UID));
    assert_eq!(items[0]["creator_chat_id"].as_i64(), Some(chat_id));
    assert!(items[0]["payload"]["chapters"].as_array().is_some_and(|c| !c.is_empty()));

    let links = extract_apple_deep_links_from_message(&reply);
    assert!(
        links.iter().any(|l| l.contains(cid) && (l.starts_with("tg://apple/books/") || l.starts_with("tg://apple/creations/"))),
        "expected tg://apple/books|creations/{cid} deep-link in bot card/buttons: {reply}"
    );
    let has_card_or_kbd = reply["content"]["type"] == "GenerativeCard"
        || reply["inline_keyboard"].as_array().is_some_and(|r| !r.is_empty());
    assert!(has_card_or_kbd, "expected GenerativeCard or inline_keyboard in reply: {reply}");

    // Bot-authored draft MUST pass strict review validator on submit-review (200 OK).
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t1_30 bot book submit-review")?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_31_bot_newmusic_creates_cloud_music_draft_with_playable_tracks() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "bot_music").await?;
    let chat_id = open_creator_bot_dm(&driver, &creator.token).await?;

    let reply = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        chat_id,
        "/newmusic Sunset Neon Waves | Warm lo-fi synthwave track at 112 BPM",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=music", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let items = extract_items(&my_val)?;
    assert_eq!(items.len(), 1, "expected 1 music draft from /newmusic: {my_val}");
    let cid = items[0]["creation_id"].as_str().unwrap();
    assert_eq!(items[0]["review_status"], "draft");
    assert_eq!(items[0]["creator_bot_user_id"].as_u64(), Some(CREATOR_STUDIO_BOT_UID));
    assert_eq!(items[0]["creator_chat_id"].as_i64(), Some(chat_id));
    let tracks = items[0]["payload"]["tracks"].as_array().expect("tracks array");
    assert!(!tracks.is_empty());
    assert!(
        tracks[0]["melody_notes"].as_array().is_some_and(|n| n.len() >= 4)
            || tracks[0]["audio_media_file_id"].as_str().is_some(),
        "bot-created track must be playable (melody_notes or audio_media_file_id): {}",
        items[0]
    );
    let links = extract_apple_deep_links_from_message(&reply);
    assert!(
        links.iter().any(|l| l.contains(cid)),
        "expected deep-link referencing {cid} in reply: {reply}"
    );

    // Bot-authored draft MUST pass strict review validator on submit-review (200 OK).
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t1_31 bot music submit-review")?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_32_bot_newgame_creates_cloud_game_draft_with_playable_spec() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "bot_game").await?;
    let chat_id = open_creator_bot_dm(&driver, &creator.token).await?;

    let reply = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        chat_id,
        "/newgame Cosmic Puzzle Run | Playable arcade trivia & puzzle adventure",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=game", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let items = extract_items(&my_val)?;
    assert_eq!(items.len(), 1, "expected 1 game draft from /newgame: {my_val}");
    let cid = items[0]["creation_id"].as_str().unwrap();
    assert_eq!(items[0]["review_status"], "draft");
    assert_eq!(items[0]["creator_bot_user_id"].as_u64(), Some(CREATOR_STUDIO_BOT_UID));
    assert_eq!(items[0]["creator_chat_id"].as_i64(), Some(chat_id));
    assert!(
        items[0]["payload"]["spec"]
            .as_object()
            .is_some_and(|m| !m.is_empty() && m.contains_key("type")),
        "game spec must be a non-empty tagged GameSpec object: {}",
        items[0]
    );
    let links = extract_apple_deep_links_from_message(&reply);
    assert!(
        links.iter().any(|l| l.contains(cid)),
        "expected deep-link referencing {cid} in reply: {reply}"
    );

    // Bot-authored draft MUST pass strict review validator on submit-review (200 OK).
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t1_32 bot game submit-review")?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_33_bot_mycreations_lists_users_cloud_creations() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "bot_list").await?;
    let chat_id = open_creator_bot_dm(&driver, &creator.token).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Aurora Cloud Diary", "Cloud diary visible through /mycreations.", "Author", "en-US", &["Fiction"], &["aurora"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();

    let reply = send_bot_command_and_poll_reply(&driver, &creator.token, chat_id, "/mycreations").await?;
    assert!(
        value_contains_substring(&reply, "Aurora Cloud Diary") || value_contains_creation_id(&reply, cid),
        "expected /mycreations reply to list user's cloud creation: {reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_34_bot_editwork_and_submitwork_manage_and_submit_draft() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "bot_edit").await?;
    let chat_id = open_creator_bot_dm(&driver, &creator.token).await?;

    let _ = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        chat_id,
        "/newbook Harbor Fog | Mystery book set in a coastal lighthouse",
    )
    .await?;

    let (st_my, my_val) = driver
        .json_req(Method::GET, "/api/v1/creations/my?kind=book", Some(&creator.token), None)
        .await?;
    assert_eq!(st_my, 200);
    let cid = extract_items(&my_val)?[0]["creation_id"].as_str().unwrap().to_string();

    let _edit_reply = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        chat_id,
        &format!("/editwork {cid} title=Harbor Lanterns"),
    )
    .await?;
    let (st_get1, after_edit) = get_creation_raw(&driver, Some(&creator.token), &cid).await?;
    assert_eq!(st_get1, 200);
    assert!(
        after_edit["title"].as_str().unwrap_or("").contains("Harbor Lanterns"),
        "expected /editwork to update title, got: {after_edit}"
    );

    let _sub_reply = send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        chat_id,
        &format!("/submitwork {cid}"),
    )
    .await?;
    let (st_get2, after_sub) = get_creation_raw(&driver, Some(&creator.token), &cid).await?;
    assert_eq!(st_get2, 200);
    assert_eq!(
        after_sub["review_status"], "pending_review",
        "expected /submitwork to transition creation to pending_review: {after_sub}"
    );

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 7: Bot Public Recommendations & Inline Queries (F8 — 6 tests: t1_35..t1_40)
// ============================================================================

#[tokio::test]
async fn t1_35_catalog_recommend_returns_ranked_public_items_with_apple_deep_links() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book: Value = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Voyager Nebula Tales", "Epic space opera across distant nebulae.", "Author", "en-US", &["Sci-Fi"], &["nebula", "space"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st, rec) = driver
        .json_req(
            Method::GET,
            "/api/v1/catalog/recommend?q=nebula&kind=book&limit=5",
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st, 200, "catalog/recommend failed: {rec}");
    let items = extract_items(&rec)?;
    let found = items
        .iter()
        .find(|i| i["creation_id"].as_str() == Some(cid))
        .expect("published book must be recommended");
    let dl = found["deep_link"].as_str().unwrap_or("");
    assert_eq!(
        dl,
        format!("tg://apple/books/{cid}"),
        "recommendation item must include canonical tg://apple/books/<id> deep_link: {found}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_36_catalog_recommend_filters_by_kind_genre_and_limit() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let m1 = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Jazz Horizon One", "Smooth twilight jazz composition.", "Artist", "en-US", &["Jazz"], &["smooth"], None, None),
    )
    .await?;
    let m2 = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Jazz Horizon Two", "Late night quartet jazz session.", "Artist", "en-US", &["Jazz"], &["quartet"], None, None),
    )
    .await?;
    let b1 = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Jazz History Book", "History of modern jazz improvisation.", "Author", "en-US", &["History"], &["jazz"], None),
    )
    .await?;
    publish_creation_public(&driver, &creator.token, &admin.token, m1["creation_id"].as_str().unwrap()).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, m2["creation_id"].as_str().unwrap()).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, b1["creation_id"].as_str().unwrap()).await?;

    let (st, rec) = driver
        .json_req(
            Method::GET,
            "/api/v1/catalog/recommend?q=Jazz&kind=music&genre=Jazz&limit=1",
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st, 200, "filtered recommend failed: {rec}");
    let items = extract_items(&rec)?;
    assert_eq!(items.len(), 1, "limit=1 must cap results: {rec}");
    assert_eq!(items[0]["kind"], "music");
    assert!(items[0]["deep_link"].as_str().unwrap_or("").starts_with("tg://apple/music/"));

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_37_bot_recommend_command_returns_published_items_in_english_and_chinese() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let en_game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Alpine Puzzle Blitz", "Crisp alpine trivia and logic challenge.", "Studio", "en-US", &["Trivia"], &["alpine"], None),
    )
    .await?;
    let zh_music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("青花瓷夜曲", "融合古筝与现代电子节奏的中文原创音乐专辑。", "林墨", "zh-CN", &["国风"], &["古筝", "夜曲"], None, None),
    )
    .await?;
    let en_id = en_game["creation_id"].as_str().unwrap();
    let zh_id = zh_music["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, en_id).await?;
    publish_creation_public(&driver, &creator.token, &admin.token, zh_id).await?;

    let chat_id = open_creator_bot_dm(&driver, &consumer.token).await?;
    let reply_en = send_bot_command_and_poll_reply(&driver, &consumer.token, chat_id, "/recommend Alpine").await?;
    assert!(
        value_contains_creation_id(&reply_en, en_id) || value_contains_substring(&reply_en, "Alpine Puzzle Blitz"),
        "EN /recommend must surface published game: {reply_en}"
    );
    let links_en = extract_apple_deep_links_from_message(&reply_en);
    assert!(
        links_en.iter().any(|l| l.starts_with(&format!("tg://apple/games/{en_id}"))),
        "EN /recommend must include canonical tg://apple/games/{en_id} deep-link: {reply_en}"
    );

    let reply_zh = send_bot_command_and_poll_reply(&driver, &consumer.token, chat_id, "/recommend 夜曲").await?;
    assert!(
        value_contains_creation_id(&reply_zh, zh_id) || value_contains_substring(&reply_zh, "青花瓷夜曲"),
        "ZH /recommend must surface published Chinese music: {reply_zh}"
    );
    let links_zh = extract_apple_deep_links_from_message(&reply_zh);
    assert!(
        links_zh.iter().any(|l| l.starts_with(&format!("tg://apple/music/{zh_id}"))),
        "ZH /recommend must include canonical tg://apple/music/{zh_id} deep-link: {reply_zh}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_38_bot_recommend_reply_includes_canonical_tg_apple_deep_link_buttons() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let music = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Solstice Ambient Drift", "Published ambient track for deep-link button verification.", "A", "en-US", &["Ambient"], &["solstice"], None, None),
    )
    .await?;
    let cid = music["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let chat_id = open_creator_bot_dm(&driver, &consumer.token).await?;
    let reply = send_bot_command_and_poll_reply(&driver, &consumer.token, chat_id, "/recommend Solstice").await?;
    let links = extract_apple_deep_links_from_message(&reply);
    assert!(
        links.iter().any(|l| l.starts_with(&format!("tg://apple/music/{cid}"))),
        "expected canonical tg://apple/music/{cid} deep-link in card/keyboard buttons: {reply}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_39_inline_query_endpoint_returns_published_public_creations() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Coral Reef Odyssey", "Underwater exploration guide for inline recommendations.", "Marine", "en-US", &["Science"], &["coral"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st, body) = driver
        .json_req(
            Method::GET,
            "/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query=Coral&kind=book&limit=10",
            Some(&consumer.token),
            None,
        )
        .await?;
    assert_eq!(st, 200, "inline-query endpoint failed: {body}");
    let results = extract_items(&body)?;
    assert!(
        results.iter().any(|r| value_contains_creation_id(r, cid)),
        "inline-query must return published public book `{cid}`: {body}"
    );
    assert!(
        results.iter().any(|r| value_contains_substring(r, &format!("tg://apple/books/{cid}"))),
        "inline-query result must include canonical tg://apple/books/{cid} deep-link: {body}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t1_40_telegram_bot_api_answer_inline_query_accepts_results_payload() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Bot API Inline Showcase", "Published book referenced by answerInlineQuery.", "Author", "en-US", &["Fiction"], &["botapi"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let bot_uname = format!("inline_rec_{}_bot", unique_suffix());
    let (st_bot, bot_val) = driver
        .json_req(
            Method::POST,
            "/api/v1/bots",
            Some(&creator.token),
            Some(json!({ "display_name": "Inline Rec Bot", "username": bot_uname })),
        )
        .await?;
    assert!(st_bot == 201 || st_bot == 200, "create bot failed {st_bot}: {bot_val}");
    let bot_token = bot_val["bot_token"].as_str().unwrap();

    let (st_ans, ans_val) = driver
        .json_req(
            Method::POST,
            &format!("/bot/{bot_token}/answerInlineQuery"),
            None,
            Some(json!({
                "inline_query_id": format!("iq_{}", unique_suffix()),
                "results": [
                    {
                        "type": "article",
                        "id": cid,
                        "title": "Bot API Inline Showcase",
                        "description": "Open in Apple Books",
                        "url": format!("tg://apple/books/{cid}"),
                        "input_message_content": {
                            "message_text": format!("Recommended book: tg://apple/books/{cid}")
                        }
                    }
                ],
                "cache_time": 30,
                "is_personal": false
            })),
        )
        .await?;
    assert_eq!(st_ans, 200, "answerInlineQuery must succeed with 200: {ans_val}");
    assert_eq!(ans_val["ok"], true, "expected Telegram Bot API ok=true: {ans_val}");

    // Negative check 1: invalid bot token must be rejected with 401 and ok == false
    let (st_bad_tok, bad_tok_val) = driver
        .json_req(
            Method::POST,
            "/bot/999999:invalid_bot_token_xyz/answerInlineQuery",
            None,
            Some(json!({ "inline_query_id": "iq_bad", "results": [] })),
        )
        .await?;
    assert_eq!(st_bad_tok, 401, "invalid bot token must return 401: {bad_tok_val}");
    assert_eq!(bad_tok_val["ok"], false, "expected ok=false on invalid bot token: {bad_tok_val}");

    // Negative check 2: missing inline_query_id must be rejected with 400 and ok == false
    let (st_missing_id, missing_id_val) = driver
        .json_req(
            Method::POST,
            &format!("/bot/{bot_token}/answerInlineQuery"),
            None,
            Some(json!({ "results": [] })),
        )
        .await?;
    assert_eq!(st_missing_id, 400, "missing inline_query_id must return 400: {missing_id_val}");
    assert_eq!(missing_id_val["ok"], false, "expected ok=false on missing inline_query_id: {missing_id_val}");

    server.stop().await;
    Ok(())
}
