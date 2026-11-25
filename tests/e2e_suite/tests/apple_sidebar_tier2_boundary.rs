mod apple_sidebar_common;

use apple_sidebar_common::*;
use reqwest::Method;
use serde_json::{json, Value};

fn assert_validation_422(st: u16, body: &Value, context: &str) {
    assert_eq!(st, 422, "{context}: expected 422 Unprocessable Entity, got {st}: {body}");
    let errs = body["validation_errors"]
        .as_array()
        .expect("422 response must contain validation_errors array");
    assert!(
        !errs.is_empty(),
        "{context}: validation_errors must be non-empty: {body}"
    );
}

// ============================================================================
// Area 1: Payload Validation (`422` + `validation_errors` — 6 tests: t2_01..t2_06)
// ============================================================================

#[tokio::test]
async fn t2_01_submit_invalid_book_empty_chapters_or_short_text_returns_422() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "val_book").await?;

    let mut empty_ch = make_book_creation(
        "Empty Chapter Book",
        "A book draft missing all chapter entries.",
        "Author",
        "en-US",
        &["Fiction"],
        &["invalid"],
        None,
    );
    empty_ch["payload"]["chapters"] = json!([]);
    let c1 = create_creation_ok(&driver, &creator.token, empty_ch).await?;
    let (st1, b1) = submit_review_raw(&driver, &creator.token, c1["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st1, &b1, "empty chapters");

    let mut short_body = make_book_creation(
        "Too Short Book",
        "A book draft whose chapter text is far under 50 characters.",
        "Author",
        "en-US",
        &["Fiction"],
        &["short"],
        None,
    );
    short_body["payload"]["chapters"] = json!([
        {
            "chapter_id": "ch_1",
            "chapter_number": 1,
            "title": "Tiny",
            "subtitle": null,
            "pages": [{ "page_number": 1, "blocks": [{ "type": "paragraph", "text": "Too short." }] }]
        }
    ]);
    let c2 = create_creation_ok(&driver, &creator.token, short_body).await?;
    let (st2, b2) = submit_review_raw(&driver, &creator.token, c2["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st2, &b2, "book < 50 non-whitespace chars");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_02_submit_invalid_music_out_of_range_bpm_or_missing_notes_returns_422() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "val_music").await?;

    let mut bad_bpm = make_music_creation(
        "Hyper BPM Track",
        "Music draft with an impossible BPM outside 30..=300.",
        "Artist",
        "en-US",
        &["Electronic"],
        &["badbpm"],
        None,
        None,
    );
    bad_bpm["payload"]["bpm"] = json!(999);
    let c1 = create_creation_ok(&driver, &creator.token, bad_bpm).await?;
    let (st1, b1) = submit_review_raw(&driver, &creator.token, c1["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st1, &b1, "bpm=999");

    let mut empty_notes = make_music_creation(
        "Silent Track",
        "Music draft with neither melody_notes nor audio_media_file_id.",
        "Artist",
        "en-US",
        &["Ambient"],
        &["silent"],
        None,
        None,
    );
    empty_notes["payload"]["tracks"][0]["melody_notes"] = json!([]);
    empty_notes["payload"]["tracks"][0]["audio_media_file_id"] = Value::Null;
    let c2 = create_creation_ok(&driver, &creator.token, empty_notes).await?;
    let (st2, b2) = submit_review_raw(&driver, &creator.token, c2["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st2, &b2, "empty melody_notes and no audio_media_file_id");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_03_submit_invalid_game_unplayable_spec_or_bad_trivia_index_returns_422() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "val_game").await?;

    let mut bad_idx = make_game_creation(
        "Broken Quiz Game",
        "Trivia game whose correct_index points past available choices.",
        "Dev",
        "en-US",
        &["Trivia"],
        &["broken"],
        None,
    );
    bad_idx["payload"]["spec"]["questions"][0]["correct_index"] = json!(9);
    let c1 = create_creation_ok(&driver, &creator.token, bad_idx).await?;
    let (st1, b1) = submit_review_raw(&driver, &creator.token, c1["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st1, &b1, "trivia correct_index out of bounds");

    let mut empty_q = make_game_creation(
        "Empty Quiz Game",
        "Trivia game containing zero playable questions.",
        "Dev",
        "en-US",
        &["Trivia"],
        &["empty"],
        None,
    );
    empty_q["payload"]["spec"]["questions"] = json!([]);
    let c2 = create_creation_ok(&driver, &creator.token, empty_q).await?;
    let (st2, b2) = submit_review_raw(&driver, &creator.token, c2["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st2, &b2, "empty trivia questions");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_04_submit_xss_script_and_javascript_uri_rejected_with_422() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "val_xss").await?;

    let script_book = make_book_creation(
        "Harmless <script>alert(1)</script> Title",
        "Summary attempting to smuggle script tags through review.",
        "Author",
        "en-US",
        &["Fiction"],
        &["xss"],
        None,
    );
    let c1 = create_creation_ok(&driver, &creator.token, script_book).await?;
    let (st1, b1) = submit_review_raw(&driver, &creator.token, c1["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st1, &b1, "<script> in title");

    let js_uri_book = make_book_creation(
        "Clean Book Title",
        "Click javascript:alert(document.cookie) to unlock secret chapters.",
        "Author",
        "en-US",
        &["Fiction"],
        &["xss"],
        None,
    );
    let c2 = create_creation_ok(&driver, &creator.token, js_uri_book).await?;
    let (st2, b2) = submit_review_raw(&driver, &creator.token, c2["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st2, &b2, "javascript: URI in summary");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_05_submit_oversized_title_short_summary_and_bad_hex_rejected_with_422() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "val_meta").await?;

    // Check 1 (isolated): oversized title (> 120 chars), valid summary & valid hex
    let long_title = "T".repeat(140);
    let bad_title = make_book_creation(
        &long_title,
        "Valid summary exceeding ten characters for isolated title check.",
        "Author",
        "en-US",
        &["Fiction"],
        &["meta"],
        None,
    );
    let c1 = create_creation_ok(&driver, &creator.token, bad_title).await?;
    let (st1, b1) = submit_review_raw(&driver, &creator.token, c1["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st1, &b1, "oversized title (>120 chars)");

    // Check 2 (isolated): short summary (< 10 chars), valid title & valid hex
    let bad_summary = make_book_creation(
        "Valid Short Summary Check",
        "Short",
        "Author",
        "en-US",
        &["Fiction"],
        &["meta"],
        None,
    );
    let c2 = create_creation_ok(&driver, &creator.token, bad_summary).await?;
    let (st2, b2) = submit_review_raw(&driver, &creator.token, c2["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st2, &b2, "short summary (<10 chars)");

    // Check 3 (isolated): invalid cover hex, valid title & valid summary
    let mut bad_hex = make_book_creation(
        "Valid Bad Hex Check",
        "Valid summary exceeding ten characters for isolated hex check.",
        "Author",
        "en-US",
        &["Fiction"],
        &["meta"],
        None,
    );
    bad_hex["cover"]["gradient_start_hex"] = json!("not-a-hex");
    let c3 = create_creation_ok(&driver, &creator.token, bad_hex).await?;
    let (st3, b3) = submit_review_raw(&driver, &creator.token, c3["creation_id"].as_str().unwrap()).await?;
    assert_validation_422(st3, &b3, "bad cover gradient_start_hex");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_06_submit_or_create_payload_exceeding_4mib_rejected() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "val_4mib").await?;

    let giant_text = "A".repeat(4 * 1024 * 1024 + 65_536);
    let mut huge = make_book_creation(
        "Oversized 4MiB Book",
        "Book carrying a payload exceeding the 4 MiB safety limit.",
        "Author",
        "en-US",
        &["Fiction"],
        &["huge"],
        None,
    );
    huge["payload"]["chapters"][0]["pages"][0]["blocks"][1]["text"] = json!(giant_text);

    let (st_create, body_create) = create_creation_raw(&driver, &creator.token, huge).await?;
    if st_create == 200 || st_create == 201 {
        let cid = body_create["creation_id"].as_str().unwrap();
        let (st_sub, body_sub) = submit_review_raw(&driver, &creator.token, cid).await?;
        assert_validation_422(st_sub, &body_sub, "payload > 4 MiB at submit-review");
    } else {
        assert!(
            matches!(st_create, 400 | 413 | 422),
            "oversized >4MiB create must return 400/413/422, got {st_create}: {body_create}"
        );
    }

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 2: CAS Concurrency & Access Control (6 tests: t2_07..t2_12)
// ============================================================================

#[tokio::test]
async fn t2_07_update_with_stale_expected_revision_returns_409_revision_conflict() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "cas_owner").await?;

    let mut payload = make_book_creation(
        "CAS Book v1",
        "Book verifying optimistic revision conflict detection.",
        "Author",
        "en-US",
        &["Fiction"],
        &["cas"],
        None,
    );
    let created = create_creation_ok(&driver, &creator.token, payload.clone()).await?;
    let cid = created["creation_id"].as_str().unwrap();
    let rev1 = created["cloud_revision"].as_i64().unwrap();

    payload["title"] = json!("CAS Book v2");
    payload["expected_revision"] = json!(rev1);
    let (st_ok, updated) = update_creation_raw(&driver, &creator.token, cid, payload.clone()).await?;
    assert_eq!(st_ok, 200, "fresh expected_revision must succeed: {updated}");

    payload["title"] = json!("CAS Book Stale Overwrite");
    payload["expected_revision"] = json!(rev1);
    let (st_conflict, conflict_body) = update_creation_raw(&driver, &creator.token, cid, payload).await?;
    assert_eq!(st_conflict, 409, "stale expected_revision must return 409 Conflict: {conflict_body}");
    assert!(
        value_contains_substring(&conflict_body, "revision_conflict")
            || value_contains_substring(&conflict_body, "revision"),
        "409 body should describe revision_conflict: {conflict_body}"
    );

    let (st_get, persisted) = get_creation_raw(&driver, Some(&creator.token), cid).await?;
    assert_eq!(st_get, 200);
    assert_eq!(persisted["title"], "CAS Book v2");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_08_unauthenticated_creations_and_catalog_requests_return_401() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let seed_owner = register_actor(&driver, "auth_seed").await?;
    let seeded = create_creation_ok(
        &driver,
        &seed_owner.token,
        make_book_creation(
            "Auth Guard Target",
            "Seeded creation used to verify 401 on {id} endpoints.",
            "Owner",
            "en-US",
            &["Fiction"],
            &["auth"],
            None,
        ),
    )
    .await?;
    let cid = seeded["creation_id"].as_str().unwrap().to_string();

    let endpoints: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/v1/creations/my".to_string(), None),
        (Method::GET, "/api/v1/creations/sync?since_revision=0".to_string(), None),
        (Method::GET, "/api/v1/creations/review-queue".to_string(), None),
        (Method::GET, "/api/v1/catalog?kind=all".to_string(), None),
        (Method::GET, "/api/v1/catalog/books".to_string(), None),
        (Method::GET, "/api/v1/catalog/music".to_string(), None),
        (Method::GET, "/api/v1/catalog/games".to_string(), None),
        (Method::GET, "/api/v1/catalog/appstore".to_string(), None),
        (Method::GET, "/api/v1/catalog/recommend?q=test".to_string(), None),
        (Method::GET, "/api/v1/catalog/my-states".to_string(), None),
        (
            Method::GET,
            "/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query=test".to_string(),
            None,
        ),
        (
            Method::POST,
            "/api/v1/creations".to_string(),
            Some(make_book_creation("Anon", "Unauthenticated attempt to write cloud data.", "Anon", "en-US", &["Fiction"], &["a"], None)),
        ),
        (Method::GET, format!("/api/v1/creations/{cid}"), None),
        (
            Method::PUT,
            format!("/api/v1/creations/{cid}"),
            Some(json!({ "title": "Unauth Edit" })),
        ),
        (Method::DELETE, format!("/api/v1/creations/{cid}"), None),
        (
            Method::POST,
            format!("/api/v1/creations/{cid}/submit-review"),
            Some(json!({})),
        ),
        (
            Method::POST,
            format!("/api/v1/creations/{cid}/withdraw-review"),
            Some(json!({})),
        ),
        (
            Method::POST,
            format!("/api/v1/creations/{cid}/unpublish"),
            Some(json!({})),
        ),
        (
            Method::POST,
            format!("/api/v1/creations/{cid}/review"),
            Some(json!({ "decision": "approve", "visibility": "public" })),
        ),
        (Method::GET, format!("/api/v1/creations/{cid}/audit"), None),
        (
            Method::PUT,
            format!("/api/v1/catalog/{cid}/state"),
            Some(json!({ "in_library": true })),
        ),
        (
            Method::POST,
            format!("/api/v1/catalog/{cid}/reviews"),
            Some(json!({ "rating": 5, "body": "Unauth review" })),
        ),
        (
            Method::POST,
            format!("/api/v1/catalog/{cid}/scores"),
            Some(json!({ "score": 100 })),
        ),
    ];

    for (method, path, body) in endpoints {
        let (st, resp) = driver.json_req(method.clone(), &path, None, body).await?;
        assert_eq!(st, 401, "{method} {path} without Bearer token must return 401: {resp}");
    }

    let (st_bad, bad_resp) = driver
        .json_req(Method::GET, "/api/v1/creations/my", Some("invalid.token.value"), None)
        .await?;
    assert_eq!(st_bad, 401, "invalid token must return 401: {bad_resp}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_09_cross_user_draft_read_edit_submit_withdraw_unpublish_delete_denied() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (owner, intruder, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let mut payload = make_book_creation(
        "Private Owner Manuscript",
        "Strict private draft inaccessible to any other user.",
        "Owner",
        "en-US",
        &["Fiction"],
        &["private"],
        None,
    );
    let created = create_creation_ok(&driver, &owner.token, payload.clone()).await?;
    let cid = created["creation_id"].as_str().unwrap();

    // 1. While Draft: every intruder action must fail 403/404
    let (st_read, b_read) = get_creation_raw(&driver, Some(&intruder.token), cid).await?;
    assert!(matches!(st_read, 403 | 404), "intruder read draft must be 403/404, got {st_read}: {b_read}");

    payload["title"] = json!("Hijacked Title");
    let (st_upd, b_upd) = update_creation_raw(&driver, &intruder.token, cid, payload.clone()).await?;
    assert!(matches!(st_upd, 403 | 404), "intruder edit draft must be 403/404, got {st_upd}: {b_upd}");

    let (st_sub, b_sub) = submit_review_raw(&driver, &intruder.token, cid).await?;
    assert!(matches!(st_sub, 403 | 404), "intruder submit must be 403/404, got {st_sub}: {b_sub}");

    let (st_wd, b_wd) = withdraw_review_raw(&driver, &intruder.token, cid).await?;
    assert!(matches!(st_wd, 403 | 404), "intruder withdraw draft must be 403/404, got {st_wd}: {b_wd}");

    let (st_unpub, b_unpub) = unpublish_raw(&driver, &intruder.token, cid).await?;
    assert!(matches!(st_unpub, 403 | 404), "intruder unpublish draft must be 403/404, got {st_unpub}: {b_unpub}");

    let (st_del, b_del) = delete_creation_raw(&driver, &intruder.token, cid).await?;
    assert!(matches!(st_del, 403 | 404), "intruder delete draft must be 403/404, got {st_del}: {b_del}");

    let (st_verify, intact) = get_creation_raw(&driver, Some(&owner.token), cid).await?;
    assert_eq!(st_verify, 200);
    assert_eq!(intact["title"], "Private Owner Manuscript");
    assert_eq!(intact["review_status"], "draft");

    // 2. While PendingReview: intruder withdraw-review must fail 403/404 (not state-guard masked)
    let (st_own_sub, own_sub) = submit_review_raw(&driver, &owner.token, cid).await?;
    let _ = assert_submit_review_ok(st_own_sub, &own_sub, "t2_09 owner submit")?;
    let (st_wd_pend, b_wd_pend) = withdraw_review_raw(&driver, &intruder.token, cid).await?;
    assert!(
        matches!(st_wd_pend, 403 | 404),
        "intruder withdraw on pending_review must be 403/404, got {st_wd_pend}: {b_wd_pend}"
    );
    let (st_chk_pend, chk_pend) = get_creation_raw(&driver, Some(&owner.token), cid).await?;
    assert_eq!(st_chk_pend, 200);
    assert_eq!(chk_pend["review_status"], "pending_review");

    // 3. While Published: intruder PUT, unpublish, and DELETE must fail 403/404
    let (st_app, app_val) = review_decision_raw(
        &driver,
        &admin.token,
        cid,
        json!({ "decision": "approve", "visibility": "public" }),
    )
    .await?;
    assert_eq!(st_app, 200, "admin approve failed: {app_val}");

    let (st_upd_pub, b_upd_pub) = update_creation_raw(&driver, &intruder.token, cid, payload).await?;
    assert!(
        matches!(st_upd_pub, 403 | 404),
        "intruder PUT on published creation must be 403/404, got {st_upd_pub}: {b_upd_pub}"
    );
    let (st_unpub_pub, b_unpub_pub) = unpublish_raw(&driver, &intruder.token, cid).await?;
    assert!(
        matches!(st_unpub_pub, 403 | 404),
        "intruder unpublish on published creation must be 403/404, got {st_unpub_pub}: {b_unpub_pub}"
    );
    let (st_del_pub, b_del_pub) = delete_creation_raw(&driver, &intruder.token, cid).await?;
    assert!(
        matches!(st_del_pub, 403 | 404),
        "intruder DELETE on published creation must be 403/404, got {st_del_pub}: {b_del_pub}"
    );

    let (st_chk_pub, chk_pub) = get_creation_raw(&driver, Some(&owner.token), cid).await?;
    assert_eq!(st_chk_pub, 200);
    assert_eq!(chk_pub["title"], "Private Owner Manuscript");
    assert_eq!(chk_pub["review_status"], "published");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_10_non_admin_review_queue_and_approve_reject_return_403() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "creator_no_admin").await?;
    let regular = register_actor(&driver, "regular_no_admin").await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Pending Mod Track", "Awaiting genuine admin review.", "Artist", "en-US", &["Pop"], &["mod"], None, None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t2_10 submit")?;

    let (st_q, b_q) = get_review_queue_raw(&driver, &regular.token, "status=pending_review").await?;
    assert_eq!(st_q, 403, "non-admin review-queue must return 403: {b_q}");

    let (st_app, b_app) = review_decision_raw(
        &driver,
        &regular.token,
        cid,
        json!({ "decision": "approve", "visibility": "public" }),
    )
    .await?;
    assert_eq!(st_app, 403, "non-admin approve must return 403: {b_app}");

    let (st_rej, b_rej) = review_decision_raw(
        &driver,
        &regular.token,
        cid,
        json!({ "decision": "reject", "reason": "Unauthorized non-admin rejection" }),
    )
    .await?;
    assert_eq!(st_rej, 403, "non-admin reject must return 403: {b_rej}");

    let (st_self, b_self) = review_decision_raw(
        &driver,
        &creator.token,
        cid,
        json!({ "decision": "approve", "visibility": "public" }),
    )
    .await?;
    assert_eq!(st_self, 403, "non-admin owner self-approve must return 403: {b_self}");

    let (st_self_rej, b_self_rej) = review_decision_raw(
        &driver,
        &creator.token,
        cid,
        json!({ "decision": "reject", "reason": "Unauthorized owner self-reject" }),
    )
    .await?;
    assert_eq!(st_self_rej, 403, "non-admin owner self-reject must return 403: {b_self_rej}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_11_non_owner_non_admin_cannot_read_creation_audit_log() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Audit Privacy Book", "Book whose audit trail is restricted to owner and admins.", "Author", "en-US", &["Fiction"], &["audit"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st_con, b_con) = get_audit_raw(&driver, &consumer.token, cid).await?;
    assert!(matches!(st_con, 403 | 404), "non-owner/non-admin audit read must be 403/404, got {st_con}: {b_con}");

    let (st_adm, _) = get_audit_raw(&driver, &admin.token, cid).await?;
    assert_eq!(st_adm, 200, "admin should be permitted to inspect audit log");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_12_unlisted_published_creation_direct_fetch_allowed_but_hidden_from_catalog() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Direct Link Only Folio", "Published unlisted folio accessible only by exact ID.", "Author", "en-US", &["Essays"], &["unlistedonly"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    publish_creation_unlisted(&driver, &creator.token, &admin.token, cid).await?;

    let (st_direct, direct) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert_eq!(st_direct, 200, "unlisted published creation must allow direct GET by ID: {direct}");
    assert_eq!(direct["creation_id"], cid);
    assert_eq!(direct["visibility"], "unlisted");

    let (st_cat, cat_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/books?q=Direct", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    assert!(!value_contains_creation_id(&cat_body, cid), "unlisted item must NOT appear in /catalog/books: {cat_body}");

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 3: Invalid Review FSM Transitions (5 tests: t2_13..t2_17)
// ============================================================================

#[tokio::test]
async fn t2_13_fsm_rejects_direct_approve_on_draft_without_pending_review() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let draft = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Unsubmitted Draft", "Never submitted to pending_review.", "Author", "en-US", &["Fiction"], &["fsm"], None),
    )
    .await?;
    let cid = draft["creation_id"].as_str().unwrap();

    let (st, body) = review_decision_raw(
        &driver,
        &admin.token,
        cid,
        json!({ "decision": "approve", "visibility": "public" }),
    )
    .await?;
    assert!(
        matches!(st, 400 | 409 | 422),
        "Draft -> Published bypass must be rejected with 400/409/422, got {st}: {body}"
    );
    let (st_still, still_draft) = get_creation_raw(&driver, Some(&creator.token), cid).await?;
    assert_eq!(st_still, 200);
    assert_eq!(still_draft["review_status"], "draft");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_14_fsm_rejects_review_rejection_without_mandatory_reason() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation("Reasonless Reject Test", "Pending track testing mandatory rejection reason.", "A", "en-US", &["Pop"], &["reason"], None, None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t2_14 submit")?;

    let (st_empty, b_empty) = review_decision_raw(
        &driver,
        &admin.token,
        cid,
        json!({ "decision": "reject", "reason": "   " }),
    )
    .await?;
    assert!(
        matches!(st_empty, 400 | 422),
        "reject with whitespace reason must fail 400/422, got {st_empty}: {b_empty}"
    );

    let (st_missing, b_missing) = review_decision_raw(
        &driver,
        &admin.token,
        cid,
        json!({ "decision": "reject" }),
    )
    .await?;
    assert!(
        matches!(st_missing, 400 | 422),
        "reject without reason must fail 400/422, got {st_missing}: {b_missing}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_15_fsm_rejects_withdraw_review_on_non_pending_creation() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Not Pending Book", "Testing withdraw on draft and published states.", "A", "en-US", &["Fiction"], &["wd"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();

    let (st_draft, b_draft) = withdraw_review_raw(&driver, &creator.token, cid).await?;
    assert!(matches!(st_draft, 400 | 409 | 422), "withdraw on draft must fail, got {st_draft}: {b_draft}");

    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;
    let (st_pub, b_pub) = withdraw_review_raw(&driver, &creator.token, cid).await?;
    assert!(matches!(st_pub, 400 | 409 | 422), "withdraw on published must fail, got {st_pub}: {b_pub}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_16_fsm_rejects_unpublish_on_draft_or_rejected_creation() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, _consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Unpublish FSM Game", "Testing unpublish rejection on draft and rejected states.", "Dev", "en-US", &["Trivia"], &["fsm"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();

    let (st1, b1) = unpublish_raw(&driver, &creator.token, cid).await?;
    assert!(matches!(st1, 400 | 409 | 422), "unpublish on draft must fail, got {st1}: {b1}");

    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t2_16 submit")?;
    assert_eq!(
        review_decision_raw(
            &driver,
            &admin.token,
            cid,
            json!({ "decision": "reject", "reason": "Needs balance tuning" }),
        )
        .await?
        .0,
        200
    );
    let (st2, b2) = unpublish_raw(&driver, &creator.token, cid).await?;
    assert!(matches!(st2, 400 | 409 | 422), "unpublish on rejected must fail, got {st2}: {b2}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_17_fsm_rejects_submit_review_on_soft_deleted_creation() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let creator = register_actor(&driver, "del_fsm").await?;

    let created = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Deleted Candidate", "Soft-deleted draft cannot enter review.", "Author", "en-US", &["Fiction"], &["del"], None),
    )
    .await?;
    let cid = created["creation_id"].as_str().unwrap();
    let (st_del, _) = delete_creation_raw(&driver, &creator.token, cid).await?;
    assert!(st_del == 200 || st_del == 204);

    let (st_sub, b_sub) = submit_review_raw(&driver, &creator.token, cid).await?;
    assert!(
        matches!(st_sub, 400 | 404 | 409 | 410 | 422),
        "submit-review on soft-deleted item must fail, got {st_sub}: {b_sub}"
    );

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 4: Adversarial Zero-Leak Public Catalog & Bot Isolation (6 tests: t2_18..t2_23)
// ============================================================================

#[tokio::test]
async fn t2_18_zero_leak_draft_en_and_cjk_invisible_across_all_catalog_and_bot_surfaces() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    // Companion published public control sharing EN+CJK query prefix (eliminates empty-stub false passes)
    let ctrl = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "ZephyrPrefix PublicGuide 绝密星云簇公开卷",
            "Published public control matching 绝密星云簇.",
            "PublicAuthor",
            "zh-CN",
            &["Sci-Fi", "科幻"],
            &["ZephyrPrefix", "绝密星云簇"],
            None,
        ),
    )
    .await?;
    let ctrl_id = ctrl["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, ctrl_id).await?;

    let draft = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "ZephyrPrefix CanarySecretDraft99 绝密星云簇隐藏稿",
            "Private unpublished draft containing distinct secret canary tokens 隐藏稿.",
            "HiddenDraftAuthor",
            "zh-CN",
            &["Sci-Fi", "科幻"],
            &["ZephyrPrefix", "绝密星云簇"],
            None,
        ),
    )
    .await?;
    let cid = draft["creation_id"].as_str().unwrap();
    let bot_chat = open_creator_bot_dm(&driver, &consumer.token).await?;

    assert_zero_leak_across_all_surfaces(
        &driver,
        &consumer.token,
        bot_chat,
        "books",
        cid,
        &["ZephyrPrefix", "绝密星云簇"],
        &["CanarySecretDraft99", "隐藏稿", "HiddenDraftAuthor"],
        Some(ctrl_id),
    )
    .await?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_19_zero_leak_pending_review_en_and_cjk_invisible_across_all_surfaces() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let ctrl = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation(
            "PendingPrefix PublicSuite 待审霓虹簇公开曲",
            "Published public control music matching 待审霓虹簇.",
            "PublicArtist",
            "zh-CN",
            &["Electronic", "电子"],
            &["PendingPrefix", "待审霓虹簇"],
            None,
            None,
        ),
    )
    .await?;
    let ctrl_id = ctrl["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, ctrl_id).await?;

    let item = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation(
            "PendingPrefix CanaryPending88 待审霓虹簇秘密曲",
            "Submitted music awaiting moderation 秘密曲.",
            "PendingHiddenArtist",
            "zh-CN",
            &["Electronic", "电子"],
            &["PendingPrefix", "待审霓虹簇"],
            None,
            None,
        ),
    )
    .await?;
    let cid = item["creation_id"].as_str().unwrap();
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t2_19 pending submit")?;

    // Direct non-owner fetch of PendingReview creation must be denied (403/404)
    let (st_direct, direct_val) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert!(
        matches!(st_direct, 403 | 404),
        "non-owner direct GET on pending_review must return 403/404, got {st_direct}: {direct_val}"
    );

    let bot_chat = open_creator_bot_dm(&driver, &consumer.token).await?;
    assert_zero_leak_across_all_surfaces(
        &driver,
        &consumer.token,
        bot_chat,
        "music",
        cid,
        &["PendingPrefix", "待审霓虹簇"],
        &["CanaryPending88", "秘密曲", "PendingHiddenArtist"],
        Some(ctrl_id),
    )
    .await?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_20_zero_leak_rejected_en_and_cjk_invisible_across_all_surfaces() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let ctrl = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation(
            "RejectedPrefix PublicArena 驳回迷宫簇公开游",
            "Published public control game matching 驳回迷宫簇.",
            "PublicStudio",
            "zh-CN",
            &["Trivia", "益智"],
            &["RejectedPrefix", "驳回迷宫簇"],
            None,
        ),
    )
    .await?;
    let ctrl_id = ctrl["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, ctrl_id).await?;

    let item = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation(
            "RejectedPrefix CanaryRejected77 驳回迷宫簇隐藏游",
            "Game rejected during review 隐藏游.",
            "RejectedHiddenDev",
            "zh-CN",
            &["Trivia", "益智"],
            &["RejectedPrefix", "驳回迷宫簇"],
            None,
        ),
    )
    .await?;
    let cid = item["creation_id"].as_str().unwrap();
    let (st_sub, sub_val) = submit_review_raw(&driver, &creator.token, cid).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_val, "t2_20 reject submit")?;
    assert_eq!(
        review_decision_raw(
            &driver,
            &admin.token,
            cid,
            json!({ "decision": "reject", "reason": "Incomplete balance pass" }),
        )
        .await?
        .0,
        200
    );

    // Direct non-owner fetch of Rejected creation must be denied (403/404)
    let (st_direct, direct_val) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert!(
        matches!(st_direct, 403 | 404),
        "non-owner direct GET on rejected must return 403/404, got {st_direct}: {direct_val}"
    );

    let bot_chat = open_creator_bot_dm(&driver, &consumer.token).await?;
    assert_zero_leak_across_all_surfaces(
        &driver,
        &consumer.token,
        bot_chat,
        "games",
        cid,
        &["RejectedPrefix", "驳回迷宫簇"],
        &["CanaryRejected77", "隐藏游", "RejectedHiddenDev"],
        Some(ctrl_id),
    )
    .await?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_21_zero_leak_unlisted_en_and_cjk_invisible_across_all_catalog_and_bot_surfaces() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let ctrl = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "UnlistedPrefix PublicVolume 非公开秘卷簇公开册",
            "Published public control book matching 非公开秘卷簇.",
            "PublicWriter",
            "zh-CN",
            &["Mystery", "悬疑"],
            &["UnlistedPrefix", "非公开秘卷簇"],
            None,
        ),
    )
    .await?;
    let ctrl_id = ctrl["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, ctrl_id).await?;

    let item = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "UnlistedPrefix CanaryUnlisted66 非公开秘卷簇私链卷",
            "Approved unlisted book excluded from public discovery 私链卷.",
            "UnlistedHiddenAuthor",
            "zh-CN",
            &["Mystery", "悬疑"],
            &["UnlistedPrefix", "非公开秘卷簇"],
            None,
        ),
    )
    .await?;
    let cid = item["creation_id"].as_str().unwrap();
    publish_creation_unlisted(&driver, &creator.token, &admin.token, cid).await?;
    let bot_chat = open_creator_bot_dm(&driver, &consumer.token).await?;

    assert_zero_leak_across_all_surfaces(
        &driver,
        &consumer.token,
        bot_chat,
        "books",
        cid,
        &["UnlistedPrefix", "非公开秘卷簇"],
        &["CanaryUnlisted66", "私链卷", "UnlistedHiddenAuthor"],
        Some(ctrl_id),
    )
    .await?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_22_zero_leak_unpublished_immediately_disappears_from_all_surfaces() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let ctrl = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation(
            "VanishedPrefix PublicOpus 下架乐章簇公开篇",
            "Published public control music matching 下架乐章簇.",
            "PublicMaestro",
            "zh-CN",
            &["Classical", "古典"],
            &["VanishedPrefix", "下架乐章簇"],
            None,
            None,
        ),
    )
    .await?;
    let ctrl_id = ctrl["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, ctrl_id).await?;

    let item = create_creation_ok(
        &driver,
        &creator.token,
        make_music_creation(
            "VanishedPrefix CanaryVanished55 下架乐章簇隐藏曲",
            "Public music later unpublished 隐藏曲.",
            "VanishedHiddenComposer",
            "zh-CN",
            &["Classical", "古典"],
            &["VanishedPrefix", "下架乐章簇"],
            None,
            None,
        ),
    )
    .await?;
    let cid = item["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    // Confirm initially discoverable before unpublish
    let (st_before, before_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/music?q=VanishedPrefix", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_before, 200);
    assert!(value_contains_creation_id(&before_body, cid));

    assert_eq!(unpublish_raw(&driver, &creator.token, cid).await?.0, 200);

    // Direct non-owner fetch of Unpublished creation must be denied (403/404)
    let (st_direct, direct_val) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert!(
        matches!(st_direct, 403 | 404),
        "non-owner direct GET on unpublished must return 403/404, got {st_direct}: {direct_val}"
    );

    let bot_chat = open_creator_bot_dm(&driver, &consumer.token).await?;
    assert_zero_leak_across_all_surfaces(
        &driver,
        &consumer.token,
        bot_chat,
        "music",
        cid,
        &["VanishedPrefix", "下架乐章簇"],
        &["CanaryVanished55", "隐藏曲", "VanishedHiddenComposer"],
        Some(ctrl_id),
    )
    .await?;

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_23_zero_leak_soft_deleted_published_item_immediately_purged_from_all_surfaces() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let ctrl = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation(
            "PurgedPrefix PublicQuest 已删街机簇公开版",
            "Published public control game matching 已删街机簇.",
            "PublicArcade",
            "zh-CN",
            &["Trivia", "街机"],
            &["PurgedPrefix", "已删街机簇"],
            None,
        ),
    )
    .await?;
    let ctrl_id = ctrl["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, ctrl_id).await?;

    let cover_png = make_unique_png_bytes("t2_23_purged_cover");
    let cover_id = upload_raw_media(
        &driver,
        &creator.token,
        "purged_game_cover.png",
        "image/png",
        cover_png.clone(),
    )
    .await?;

    let item = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation(
            "PurgedPrefix CanaryPurged44 已删街机簇秘密局",
            "Published game immediately soft-deleted 秘密局.",
            "PurgedHiddenStudio",
            "zh-CN",
            &["Trivia", "街机"],
            &["PurgedPrefix", "已删街机簇"],
            Some(&cover_id),
        ),
    )
    .await?;
    let cid = item["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    // Verify pre-delete discoverability AND public CAS media access
    let (st_before, before_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/games?q=PurgedPrefix", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_before, 200);
    assert!(value_contains_creation_id(&before_body, cid), "item must be discoverable before delete: {before_body}");
    let (st_media_before, got_png) = download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_media_before, 200, "public CAS media must be 200 before delete");
    assert_eq!(got_png, cover_png);

    let (st_del, _) = delete_creation_raw(&driver, &creator.token, cid).await?;
    assert!(st_del == 200 || st_del == 204);

    // Post-delete CAS media access MUST immediately revert to 403
    let (st_media_after, _) = download_media(&driver, &consumer.token, &cover_id).await?;
    assert_eq!(st_media_after, 403, "public CAS media must immediately revert to 403 after delete");

    let bot_chat = open_creator_bot_dm(&driver, &consumer.token).await?;
    assert_zero_leak_across_all_surfaces(
        &driver,
        &consumer.token,
        bot_chat,
        "games",
        cid,
        &["PurgedPrefix", "已删街机簇"],
        &["CanaryPurged44", "秘密局", "PurgedHiddenStudio"],
        Some(ctrl_id),
    )
    .await?;

    let (st_direct, _) = get_creation_raw(&driver, Some(&consumer.token), cid).await?;
    assert!(matches!(st_direct, 403 | 404 | 410));

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 5: CJK Search, Escaping, Pagination & Bot Security (5 tests: t2_24..t2_28)
// ============================================================================

#[tokio::test]
async fn t2_24_cjk_unspaced_substring_and_bigram_search_matches_published_creations() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "深空量子奇点编年史",
            "无空格中文长标题用于验证子串与双字切分检索精度。",
            "周明",
            "zh-CN",
            &["硬核科幻"],
            &["深空探索", "奇点文明"],
            None,
        ),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    for q in ["量子奇点", "编年史", "硬核科幻", "深空探索"] {
        let (st, body) = driver
            .json_req(
                Method::GET,
                &format!("/api/v1/catalog?kind=book&q={}", percent_encode(q)),
                Some(&consumer.token),
                None,
            )
            .await?;
        assert_eq!(st, 200, "CJK substring query `{q}` failed: {body}");
        assert!(
            value_contains_creation_id(&body, cid),
            "CJK substring `{q}` must match unspaced Chinese creation: {body}"
        );
    }

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_25_fts5_syntax_operators_and_sql_like_wildcards_handled_safely() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation(
            "Regular Safe Anthology",
            "Clean published volume to test wildcard non-explosion.",
            "Author",
            "en-US",
            &["Fiction"],
            &["safe"],
            None,
        ),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let hostile_queries = [
        "\"",
        "\"unclosed quote AND OR NOT NEAR(",
        "***)))(((^^^",
        "title:foo OR 1=1; DROP TABLE public_catalog_entries;--",
        "%%%%____",
        "科幻%_'\"*",
    ];
    for q in hostile_queries {
        let path = format!("/api/v1/catalog?kind=all&q={}", percent_encode(q));
        let (st, body) = driver
            .json_req(Method::GET, &path, Some(&consumer.token), None)
            .await?;
        assert_eq!(st, 200, "hostile query `{q}` must not 500 or crash: {body}");
        let items = extract_items(&body)?;
        if q == "%%%%____" {
            assert!(
                !items.iter().any(|i| i["creation_id"] == cid),
                "escaped SQL LIKE wildcards `%`/`_` must not wildcard-match unrelated rows: {body}"
            );
        }
    }

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_26_catalog_pagination_extreme_limit_and_offset_boundaries() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    for idx in 1..=3 {
        let c = create_creation_ok(
            &driver,
            &creator.token,
            make_book_creation(
                &format!("PageBound Book {idx}"),
                "Book for extreme pagination boundary validation.",
                "Author",
                "en-US",
                &["Fiction"],
                &["page"],
                None,
            ),
        )
        .await?;
        publish_creation_public(&driver, &creator.token, &admin.token, c["creation_id"].as_str().unwrap()).await?;
    }

    let (st_far, far_body) = driver
        .json_req(Method::GET, "/api/v1/catalog?kind=book&limit=10&offset=9999", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_far, 200);
    assert!(extract_items(&far_body)?.is_empty());

    let (st_big, big_body) = driver
        .json_req(Method::GET, "/api/v1/catalog?kind=book&limit=5000&offset=0", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_big, 200, "huge limit should clamp safely without error: {big_body}");
    assert_eq!(extract_items(&big_body)?.len(), 3);

    let (st_zero, zero_body) = driver
        .json_req(Method::GET, "/api/v1/catalog?kind=book&limit=0&offset=0", Some(&consumer.token), None)
        .await?;
    assert!(
        st_zero == 200 || st_zero == 400,
        "limit=0 must either clamp safely (200) or reject cleanly (400), got {st_zero}: {zero_body}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_27_bot_cross_user_editwork_and_submitwork_strictly_blocked() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let owner = register_actor(&driver, "bot_owner").await?;
    let attacker = register_actor(&driver, "bot_attacker").await?;

    let draft = create_creation_ok(
        &driver,
        &owner.token,
        make_book_creation("Owner Only Draft", "Draft protected against bot cross-user manipulation.", "Owner", "en-US", &["Fiction"], &["own"], None),
    )
    .await?;
    let cid = draft["creation_id"].as_str().unwrap();

    let atk_chat = open_creator_bot_dm(&driver, &attacker.token).await?;
    let edit_reply = send_bot_command_and_poll_reply(
        &driver,
        &attacker.token,
        atk_chat,
        &format!("/editwork {cid} title=PwnedByBot"),
    )
    .await?;
    assert!(
        !value_contains_substring(&edit_reply, "Owner Only Draft"),
        "cross-user /editwork reply must not leak owner draft title: {edit_reply}"
    );
    let sub_reply = send_bot_command_and_poll_reply(
        &driver,
        &attacker.token,
        atk_chat,
        &format!("/submitwork {cid}"),
    )
    .await?;
    assert!(
        !value_contains_substring(&sub_reply, "Owner Only Draft"),
        "cross-user /submitwork reply must not leak owner draft title: {sub_reply}"
    );

    let (st, intact) = get_creation_raw(&driver, Some(&owner.token), cid).await?;
    assert_eq!(st, 200);
    assert_eq!(intact["title"], "Owner Only Draft", "attacker bot /editwork mutated owner draft!");
    assert_eq!(intact["review_status"], "draft", "attacker bot /submitwork mutated owner draft!");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_28_bot_adversarial_prompt_targeting_private_draft_id_never_leaks() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let owner = register_actor(&driver, "secret_owner").await?;
    let attacker = register_actor(&driver, "secret_prober").await?;

    let draft = create_creation_ok(
        &driver,
        &owner.token,
        make_book_creation(
            "TopSecretPrefix CanaryTitleX99",
            "Confidential manuscript CanarySecretPayload777 never to be disclosed.",
            "SecretHiddenOwner",
            "en-US",
            &["Thriller"],
            &["TopSecretPrefix"],
            None,
        ),
    )
    .await?;
    let cid = draft["creation_id"].as_str().unwrap();

    let atk_chat = open_creator_bot_dm(&driver, &attacker.token).await?;
    // 1. Targeting private creation_id directly: zero title/summary/author canaries & zero deep-link
    let reply_id = send_bot_command_and_poll_reply(
        &driver,
        &attacker.token,
        atk_chat,
        &format!("/recommend {cid}"),
    )
    .await?;
    for canary in ["TopSecretPrefix", "CanaryTitleX99", "CanarySecretPayload777", "SecretHiddenOwner"] {
        assert!(
            !value_contains_substring(&reply_id, canary),
            "ADVERSARIAL BOT LEAK (`{canary}`) on /recommend <cid>: {reply_id}"
        );
    }
    assert!(
        !extract_apple_deep_links_from_message(&reply_id).iter().any(|l| l.contains(cid)),
        "ADVERSARIAL BOT LEAK: deep-link to private draft `{cid}` returned: {reply_id}"
    );

    // 2. Querying prefix without putting canary in query (no self-echo false fail)
    let reply_q = send_bot_command_and_poll_reply(
        &driver,
        &attacker.token,
        atk_chat,
        "/recommend TopSecretPrefix",
    )
    .await?;
    assert!(!value_contains_creation_id(&reply_q, cid), "ADVERSARIAL BOT LEAK of cid: {reply_q}");
    for canary in ["CanaryTitleX99", "CanarySecretPayload777", "SecretHiddenOwner"] {
        assert!(
            !value_contains_substring(&reply_q, canary),
            "ADVERSARIAL BOT LEAK (`{canary}`) on /recommend prefix: {reply_q}"
        );
    }

    // 3. Attacker's /mycreations must never include owner's draft
    let reply_my = send_bot_command_and_poll_reply(&driver, &attacker.token, atk_chat, "/mycreations").await?;
    assert!(!value_contains_creation_id(&reply_my, cid), "ADVERSARIAL BOT LEAK in /mycreations: {reply_my}");
    for canary in ["TopSecretPrefix", "CanaryTitleX99", "CanarySecretPayload777", "SecretHiddenOwner"] {
        assert!(
            !value_contains_substring(&reply_my, canary),
            "ADVERSARIAL BOT LEAK (`{canary}`) in /mycreations: {reply_my}"
        );
    }

    server.stop().await;
    Ok(())
}

// ============================================================================
// Area 6: Ratings, Reviews & Scores Bounds (5 tests: t2_29..t2_33)
// ============================================================================

#[tokio::test]
async fn t2_29_review_rating_bounds_zero_and_six_rejected() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Star Bounds Book", "Published book for rating bounds testing.", "A", "en-US", &["Fiction"], &["star"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    for bad_rating in [0, 6, -1] {
        let (st, body) = driver
            .json_req(
                Method::POST,
                &format!("/api/v1/catalog/{cid}/reviews"),
                Some(&consumer.token),
                Some(json!({ "rating": bad_rating, "title": "Invalid", "body": "Out of bounds star rating." })),
            )
            .await?;
        assert!(
            matches!(st, 400 | 422),
            "rating={bad_rating} must be rejected with 400/422, got {st}: {body}"
        );
    }

    let (st_revs, revs_body) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{cid}/reviews"), Some(&consumer.token), None)
        .await?;
    assert_eq!(st_revs, 200, "get reviews after rejected writes failed: {revs_body}");
    assert!(
        extract_items(&revs_body)?.is_empty(),
        "rejected out-of-bounds ratings must leave reviews empty: {revs_body}"
    );

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_30_posting_review_on_unpublished_draft_rejected() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, _admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let draft = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Unpublished Review Target", "Draft book that cannot accept public reviews.", "A", "en-US", &["Fiction"], &["draft"], None),
    )
    .await?;
    let cid = draft["creation_id"].as_str().unwrap();

    let (st, body) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{cid}/reviews"),
            Some(&consumer.token),
            Some(json!({ "rating": 5, "title": "Premature", "body": "Should fail." })),
        )
        .await?;
    assert!(
        matches!(st, 400 | 403 | 404 | 409 | 422),
        "reviewing unpublished draft must be rejected, got {st}: {body}"
    );

    let (st_revs, revs_body) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{cid}/reviews"), Some(&creator.token), None)
        .await?;
    if st_revs == 200 {
        assert!(
            extract_items(&revs_body)?.is_empty(),
            "rejected draft review must leave reviews empty: {revs_body}"
        );
    } else {
        assert!(
            matches!(st_revs, 403 | 404),
            "GET reviews on unpublished draft must be 403/404 or empty 200, got {st_revs}: {revs_body}"
        );
    }

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_31_posting_score_on_non_game_or_draft_game_rejected() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, player, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Non-Game Published Book", "Published book cannot accept game leaderboard scores.", "A", "en-US", &["Fiction"], &["book"], None),
    )
    .await?;
    let bid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, bid).await?;

    let draft_game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("Draft Unscored Game", "Unpublished game cannot accept public leaderboard scores.", "Dev", "en-US", &["Trivia"], &["draft"], None),
    )
    .await?;
    let dgid = draft_game["creation_id"].as_str().unwrap();

    let (st_book, b_book) = driver
        .json_req(Method::POST, &format!("/api/v1/catalog/{bid}/scores"), Some(&player.token), Some(json!({ "score": 500 })))
        .await?;
    assert!(
        matches!(st_book, 400 | 404 | 409 | 422),
        "posting game score on book must be rejected, got {st_book}: {b_book}"
    );

    let (st_draft, b_draft) = driver
        .json_req(Method::POST, &format!("/api/v1/catalog/{dgid}/scores"), Some(&player.token), Some(json!({ "score": 500 })))
        .await?;
    assert!(
        matches!(st_draft, 400 | 403 | 404 | 409 | 422),
        "posting game score on draft game must be rejected, got {st_draft}: {b_draft}"
    );

    for (label, target_id) in [("book", bid), ("draft_game", dgid)] {
        let (st_lb, lb_body) = driver
            .json_req(Method::GET, &format!("/api/v1/catalog/{target_id}/leaderboard"), Some(&player.token), None)
            .await?;
        if st_lb == 200 {
            assert!(
                extract_items(&lb_body)?.is_empty(),
                "rejected score write on {label} must leave leaderboard empty: {lb_body}"
            );
        } else {
            assert!(
                matches!(st_lb, 400 | 403 | 404),
                "leaderboard on {label} must be 400/403/404 or empty 200, got {st_lb}: {lb_body}"
            );
        }
    }

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_32_duplicate_user_review_upserts_without_inflating_rating_count() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, consumer, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let book = create_creation_ok(
        &driver,
        &creator.token,
        make_book_creation("Upsert Rating Book", "Book verifying per-user review upsert semantics.", "A", "en-US", &["Fiction"], &["upsert"], None),
    )
    .await?;
    let cid = book["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, cid).await?;

    let (st1, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{cid}/reviews"),
            Some(&consumer.token),
            Some(json!({ "rating": 2, "title": "First pass", "body": "Initial impression." })),
        )
        .await?;
    assert!(st1 == 200 || st1 == 201);

    let (st2, _) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/catalog/{cid}/reviews"),
            Some(&consumer.token),
            Some(json!({ "rating": 5, "title": "Updated pass", "body": "Much better on second read." })),
        )
        .await?;
    assert!(st2 == 200 || st2 == 201);

    let (st_cat, cat_body) = driver
        .json_req(Method::GET, "/api/v1/catalog/books", Some(&consumer.token), None)
        .await?;
    assert_eq!(st_cat, 200);
    let item = extract_items(&cat_body)?
        .iter()
        .find(|i| i["creation_id"] == cid)
        .expect("book in catalog");
    assert_eq!(item["rating_count"].as_u64(), Some(1), "duplicate user review must not inflate rating_count: {item}");
    assert!((item["rating_avg"].as_f64().unwrap_or(0.0) - 5.0).abs() < 0.05);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn t2_33_lower_game_score_preserves_higher_best_score_on_leaderboard() -> anyhow::Result<()> {
    let (mut server, driver) = boot_server().await?;
    let (creator, player, admin) = seed_creator_consumer_admin(&server, &driver).await?;

    let game = create_creation_ok(
        &driver,
        &creator.token,
        make_game_creation("High Watermark Quiz", "Game verifying monotonic best_score retention.", "Dev", "en-US", &["Trivia"], &["best"], None),
    )
    .await?;
    let gid = game["creation_id"].as_str().unwrap();
    publish_creation_public(&driver, &creator.token, &admin.token, gid).await?;

    let (st1, _) = driver
        .json_req(Method::POST, &format!("/api/v1/catalog/{gid}/scores"), Some(&player.token), Some(json!({ "score": 2500 })))
        .await?;
    assert!(st1 == 200 || st1 == 201);

    let (st2, _) = driver
        .json_req(Method::POST, &format!("/api/v1/catalog/{gid}/scores"), Some(&player.token), Some(json!({ "score": 900 })))
        .await?;
    assert!(st2 == 200 || st2 == 201);

    let (st_lb, lb_body) = driver
        .json_req(Method::GET, &format!("/api/v1/catalog/{gid}/leaderboard"), Some(&player.token), None)
        .await?;
    assert_eq!(st_lb, 200);
    let entries = extract_items(&lb_body)?;
    assert_eq!(entries.len(), 1);
    let best = entries[0]["best_score"].as_i64().or_else(|| entries[0]["score"].as_i64()).unwrap();
    assert_eq!(best, 2500, "lower follow-up score must never overwrite higher best_score: {lb_body}");

    server.stop().await;
    Ok(())
}
