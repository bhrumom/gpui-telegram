"""Tier 1: Isolated Feature Coverage Tests (78 tests across CS-F01..CS-F15).

Each feature has >= 5 genuine opaque-box HTTP/JSON tests.
NEVER imports _contract_double.py. Pure Python 3 stdlib only.
"""

from __future__ import annotations

import math
import re
from typing import List

from .harness import (
    CodeSceneEnv,
    TestRegistry,
    VALID_ASSET_KINDS,
    VALID_FEED_TABS,
    assert_valid_asset,
    assert_valid_explanation,
    assert_valid_feed_response,
    assert_valid_generative_card,
    assert_valid_manifest,
    assert_valid_ranked_item,
    extract_comment_text,
    extract_follow_flag,
    unwrap_asset,
    unwrap_draft,
)


def register_tier1_tests(reg: TestRegistry) -> None:
    # ===================== CS-F01: Shared Code Scene Contracts (5) =====================
    @reg.test("CS-T1-F01-01", 1, "CS-F01", "Feed response schema & total_catalog >= 18")
    def t1_f01_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f01")
        r = env.feed(u, tab="for_you", limit=10)
        assert r.status == 200, f"Expected 200, got {r.status}: {r.body!r}"
        items = assert_valid_feed_response(r.body, expected_limit=10)
        assert r.body["total_catalog"] >= 18 and len(items) >= 1

    @reg.test("CS-T1-F01-02", 1, "CS-F01", "Asset detail returns all 25 CodeSceneAssetDto fields")
    def t1_f01_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f01")
        r, assets = env.list_assets(u)
        assert r.status == 200 and len(assets) >= 18
        sid = assets[0]["scene_id"]
        rd, detail = env.get_asset(u, sid)
        assert rd.status == 200 and detail is not None
        assert detail["scene_id"] == sid
        assert_valid_asset(detail)

    @reg.test("CS-T1-F01-03", 1, "CS-F01", "AssetKind snake_case & manifest.kind parity across endpoints")
    def t1_f01_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f01")
        r = env.feed(u, tab="for_you", limit=18)
        items = assert_valid_feed_response(r.body)
        seen_kinds = set()
        for it in items:
            a = it["asset"]
            assert a["kind"] in VALID_ASSET_KINDS
            assert_valid_manifest(a["manifest"], expected_kind=a["kind"])
            seen_kinds.add(a["kind"])
        assert seen_kinds == VALID_ASSET_KINDS, f"Expected all 3 kinds, got {seen_kinds}"

    @reg.test("CS-T1-F01-04", 1, "CS-F01", "RecommendationExplanation & 10 finite ScoreBreakdown fields")
    def t1_f01_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f01")
        r = env.feed(u, tab="for_you", limit=8)
        assert r.status == 200
        items = assert_valid_feed_response(r.body)
        assert len(items) >= 1
        for it in items:
            assert_valid_explanation(it["explanation"], require_cjk_zh=True)

    @reg.test("CS-T1-F01-05", 1, "CS-F01", "All 6 snake_case CodeSceneFeedTab enum values return 200")
    def t1_f01_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f01")
        for tab in sorted(VALID_FEED_TABS):
            r = env.feed(u, tab=tab, limit=6)
            assert r.status == 200, f"tab={tab} failed: {r.status} {r.body!r}"
            items = assert_valid_feed_response(r.body, expected_limit=6)
            if tab in ("for_you", "videos", "animations", "games"):
                assert len(items) >= 1, f"Populated tab {tab!r} unexpectedly returned 0 items"

    # ===================== CS-F02: cs1 VM & Sandbox (6) =====================
    @reg.test("CS-T1-F02-01", 1, "CS-F02", "Validate canonical Video cs1 script")
    def t1_f02_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f02")
        src, man = env.ensure_corpus(u).valid_script("video")
        r = env.validate_script(u, src, man)
        ok, ch, errs = env.extract_validation(r)
        assert ok and ch and not errs, f"Video validation failed: {r.status} {r.body!r}"

    @reg.test("CS-T1-F02-02", 1, "CS-F02", "Validate canonical Animation cs1 script with bounded ops")
    def t1_f02_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f02")
        src, man = env.ensure_corpus(u).valid_script("animation")
        r = env.validate_script(u, src, man)
        ok, ch, _ = env.extract_validation(r)
        assert ok and ch
        if isinstance(r.body, dict) and r.body.get("ops_used") is not None:
            assert 0 <= float(r.body["ops_used"]) <= 100_000

    @reg.test("CS-T1-F02-03", 1, "CS-F02", "Validate canonical Playable Game cs1 script")
    def t1_f02_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f02")
        src, man = env.ensure_corpus(u).valid_script("game")
        r = env.validate_script(u, src, man)
        ok, ch, errs = env.extract_validation(r)
        assert ok and ch and not errs

    @reg.test("CS-T1-F02-04", 1, "CS-F02", "Deterministic VM cover_hash across repeated validation")
    def t1_f02_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f02")
        src, man = env.ensure_corpus(u).valid_script("animation")
        ok1, h1, _ = env.extract_validation(env.validate_script(u, src, man))
        ok2, h2, _ = env.extract_validation(env.validate_script(u, src, man))
        assert ok1 and ok2 and h1 and h1 == h2, f"Non-deterministic cover_hash: {h1!r} != {h2!r}"

    @reg.test("CS-T1-F02-05", 1, "CS-F02", "cover_hash changes when visible code or seed mutates")
    def t1_f02_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f02")
        corpus = env.ensure_corpus(u)
        src1, man1 = corpus.valid_script("animation")
        src2, man2 = corpus.mutated_valid_script("animation", variant=9)
        ok1, h1, _ = env.extract_validation(env.validate_script(u, src1, man1))
        ok2, h2, _ = env.extract_validation(env.validate_script(u, src2, man2))
        assert ok1 and ok2 and h1 and h2 and h1 != h2, f"cover_hash did not change: {h1!r}"

    @reg.test("CS-T1-F02-06", 1, "CS-F02", "Pixel-shader cs1 script within 240x320 pixel budget passes")
    def t1_f02_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f02")
        src, man = env.ensure_corpus(u).pixel_shader_script()
        ok, ch, errs = env.extract_validation(env.validate_script(u, src, man))
        assert ok and ch and not errs, f"Pixel shader rejected: {errs}"

    # ===================== CS-F03: Video / Animation / Game Execution Modes (5) =====================
    @reg.test("CS-T1-F03-01", 1, "CS-F03", "Publish Video asset preserves duration_sec=15.0 & fps=30")
    def t1_f03_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f03")
        src, man = env.ensure_corpus(u).valid_script("video")
        man["duration_sec"], man["fps"] = 15.0, 30
        r = env.publish_asset(u, "T1 Video Clip", "Keyframed clip", "video", ["video", "test"], src, man, controls_hint="Space / Scrub")
        assert r.status in (200, 201), f"Publish failed: {r.status} {r.body!r}"
        a = unwrap_asset(r.body)
        assert a is not None and a["kind"] == "video" and math.isclose(a["manifest"]["duration_sec"], 15.0)
        assert a["manifest"]["fps"] == 30 and a["controls_hint"]

    @reg.test("CS-T1-F03-02", 1, "CS-F03", "Publish Animation asset preserves continuous loop manifest")
    def t1_f03_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f03")
        src, man = env.ensure_corpus(u).valid_script("animation")
        man["duration_sec"], man["fps"] = None, 60
        r = env.publish_asset(u, "T1 Shader Art", "Continuous shader", "animation", ["shader"], src, man)
        assert r.status in (200, 201)
        a = unwrap_asset(r.body)
        assert a is not None and a["kind"] == "animation" and a["manifest"].get("duration_sec") is None

    @reg.test("CS-T1-F03-03", 1, "CS-F03", "Publish Playable Game asset preserves interactive metadata")
    def t1_f03_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f03")
        src, man = env.ensure_corpus(u).valid_script("game")
        r = env.publish_asset(u, "T1 Arcade Game", "Playable paddle", "game", ["arcade"], src, man, controls_hint="Arrow keys / Tap")
        assert r.status in (200, 201)
        a = unwrap_asset(r.body)
        assert a is not None and a["kind"] == "game" and "Arrow" in a["controls_hint"]

    @reg.test("CS-T1-F03-04", 1, "CS-F03", "Video dwell/complete/replay telemetry yields positive reward")
    def t1_f03_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f03")
        _, vids = env.list_assets(u, {"kind": "video"})
        sid = vids[0]["scene_id"]
        evs = [
            env.make_event(u, sid, "dwell", dwell_ms=10000, completion_ratio=0.8),
            env.make_event(u, sid, "complete", dwell_ms=15000, completion_ratio=1.0, loop_count=1),
            env.make_event(u, sid, "replay", dwell_ms=15000, completion_ratio=2.0, loop_count=2),
        ]
        r = env.send_telemetry(u, evs)
        assert r.status == 200 and r.body["accepted"] >= 3 and float(r.body["reward"]) > 0.0

    @reg.test("CS-T1-F03-05", 1, "CS-F03", "Game session telemetry accepted with positive reward")
    def t1_f03_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f03")
        _, games = env.list_assets(u, {"kind": "game"})
        sid = games[0]["scene_id"]
        ev = env.make_event(
            u, sid, "game_interaction",
            game_inputs=42, game_play_ms=25000, game_restarts=1, game_score=180.0,
        )
        r = env.send_telemetry(u, ev)
        assert r.status == 200 and int(r.body["accepted"]) >= 1 and float(r.body["reward"]) > 0.0 and r.body["profile_updated"] is True

    # ===================== CS-F04: 18-Asset Real-Code Seed Library (5) =====================
    @reg.test("CS-T1-F04-01", 1, "CS-F04", "Fresh catalog exposes >= 18 seeded assets")
    def t1_f04_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f04")
        r, assets = env.list_assets(u)
        assert r.status == 200 and len(assets) >= 18
        rf = env.feed(u, tab="for_you", limit=5)
        assert rf.status == 200 and rf.body["total_catalog"] >= 18

    @reg.test("CS-T1-F04-02", 1, "CS-F04", "Seed library balance: >= 6 videos, >= 6 animations, >= 6 games")
    def t1_f04_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f04")
        for tab, kind in (("videos", "video"), ("animations", "animation"), ("games", "game")):
            r = env.feed(u, tab=tab, limit=30)
            items = assert_valid_feed_response(r.body)
            assert len(items) >= 6, f"Expected >= 6 {kind}s, got {len(items)}"
            assert all(it["asset"]["kind"] == kind for it in items)

    @reg.test("CS-T1-F04-03", 1, "CS-F04", "Every seeded asset carries real cs1 code, tags, hints & bot attribution")
    def t1_f04_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f04")
        _, assets = env.list_assets(u)
        assert len(assets) >= 18
        for a in assets[:18]:
            assert_valid_asset(a)
            assert len(a["tags"]) >= 1 and a["controls_hint"].strip()
            assert (a.get("creator_bot_username") or a.get("author_username"))

    @reg.test("CS-T1-F04-04", 1, "CS-F04", "Genuine VM proof: sampled Video/Animation/Game seeds pass /validate")
    def t1_f04_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f04")
        for kind in ("video", "animation", "game"):
            _, rows = env.list_assets(u, {"kind": kind})
            assert len(rows) >= 1
            ok, ch, errs = env.extract_validation(env.validate_script(u, rows[0]["source_code"], rows[0]["manifest"]))
            assert ok and ch and not errs, f"Seed {rows[0]['scene_id']} failed validate: {errs}"

    @reg.test("CS-T1-F04-05", 1, "CS-F04", "Seed bootstrap idempotency across server restart (no doubling)")
    def t1_f04_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f04")
        r_before, before = env.list_assets(u, {"limit": 200})
        assert r_before.status == 200 and len(before) >= 18, (
            f"Pre-restart list_assets failed or under-seeded: status={r_before.status}, count={len(before)}"
        )
        before_ids = {a["scene_id"] for a in before}
        env.restart_server()
        r_after, after = env.list_assets(u, {"limit": 200})
        assert r_after.status == 200 and len(after) >= 18, (
            f"Post-restart list_assets failed: status={r_after.status}, count={len(after)}"
        )
        after_ids = {a["scene_id"] for a in after}
        assert len(after) == len(before) and before_ids == after_ids, (
            f"Seed count/IDs drifted across restart: {len(before)} -> {len(after)}"
        )

    # ===================== CS-F05: Behavioral Telemetry & Reward Shaping (5) =====================
    @reg.test("CS-T1-F05-01", 1, "CS-F05", "Single dwell/complete telemetry yields positive reward & profile_updated")
    def t1_f05_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f05")
        _, assets = env.list_assets(u)
        r = env.send_telemetry(u, env.make_event(u, assets[0]["scene_id"], "complete", dwell_ms=12000, completion_ratio=1.0))
        assert r.status == 200 and int(r.body["accepted"]) >= 1 and float(r.body["reward"]) > 0.0 and r.body["profile_updated"] is True

    @reg.test("CS-T1-F05-02", 1, "CS-F05", "Batch telemetry submission (4 events) accepted atomically")
    def t1_f05_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f05")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        batch = [
            env.make_event(u, sid, "impression"),
            env.make_event(u, sid, "dwell", dwell_ms=9000, completion_ratio=0.8),
            env.make_event(u, sid, "double_tap_like"),
            env.make_event(u, sid, "bookmark"),
        ]
        r = env.send_telemetry(u, batch)
        assert r.status == 200 and int(r.body["accepted"]) == 4 and float(r.body["reward"]) > 0.0

    @reg.test("CS-T1-F05-03", 1, "CS-F05", "Negative fast_skip (<1.5s) reward < 0 < complete reward")
    def t1_f05_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f05")
        _, assets = env.list_assets(u)
        r_skip = env.send_telemetry(u, env.make_event(u, assets[0]["scene_id"], "fast_skip", dwell_ms=600, completion_ratio=0.05))
        r_comp = env.send_telemetry(u, env.make_event(u, assets[1]["scene_id"], "complete", dwell_ms=15000, completion_ratio=1.0))
        assert r_skip.status == 200 and r_comp.status == 200
        assert float(r_skip.body["reward"]) < 0.0 < float(r_comp.body["reward"])

    @reg.test("CS-T1-F05-04", 1, "CS-F05", "Explicit negatives (not_interested/hide_author/hide_tag) yield reward < 0")
    def t1_f05_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f05")
        _, assets = env.list_assets(u)
        for idx, ek in enumerate(("not_interested", "hide_author", "hide_tag")):
            a = assets[idx]
            extra = {"reason": a["tags"][0]} if ek == "hide_tag" else ({"author_user_id": a["author_user_id"]} if ek == "hide_author" else {})
            r = env.send_telemetry(u, env.make_event(u, a["scene_id"], ek, **extra))
            assert r.status == 200 and float(r.body["reward"]) < 0.0 and r.body["profile_updated"] is True

    @reg.test("CS-T1-F05-05", 1, "CS-F05", "Explicit positives (like/share/comment/follow/inspect/remix) accepted > 0")
    def t1_f05_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f05")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        for ek in ("like", "share", "comment", "follow_author", "inspect_code", "remix"):
            r = env.send_telemetry(u, env.make_event(u, sid, ek))
            assert r.status == 200 and float(r.body["reward"]) > 0.0 and math.isfinite(float(r.body["reward"]))

    # ===================== CS-F06: 5-Stage Douyin Recommender (6) =====================
    @reg.test("CS-T1-F06-01", 1, "CS-F06", "ForYou exposes recall_channels, traffic_tier & calibrated finite ScoreBreakdown")
    def t1_f06_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f06")
        r = env.feed(u, tab="for_you", limit=10)
        items = assert_valid_feed_response(r.body, expected_limit=10)
        assert len(items) >= 5
        for it in items:
            assert_valid_ranked_item(it, require_cjk_zh=True)

    @reg.test("CS-T1-F06-02", 1, "CS-F06", "Positive game interactions measurably boost game affinity in ForYou")
    def t1_f06_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f06")
        _, games = env.list_assets(u, {"kind": "game"})
        r0 = env.feed(u, tab="for_you", limit=18)
        base_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in r0.body["items"] if it["asset"]["kind"] == "game"]
        base_mean = sum(base_aff) / max(1, len(base_aff))
        for g in games[:4]:
            env.send_telemetry(u, [
                env.make_event(u, g["scene_id"], "complete", dwell_ms=20000, completion_ratio=1.0),
                env.make_event(u, g["scene_id"], "like"),
                env.make_event(u, g["scene_id"], "bookmark"),
            ])
        r1 = env.feed(u, tab="for_you", limit=18)
        new_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in r1.body["items"] if it["asset"]["kind"] == "game"]
        assert sum(new_aff) / max(1, len(new_aff)) > base_mean, "Game affinity failed to rise after positive feedback"

    @reg.test("CS-T1-F06-03", 1, "CS-F06", "Following & liking an author boosts creator_affinity on their scenes")
    def t1_f06_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f06")
        _, assets = env.list_assets(u)
        target = assets[0]
        cid = target["author_user_id"]
        r0 = env.feed(u, tab="for_you", limit=30)
        before = [it["explanation"]["breakdown"]["creator_affinity"] for it in r0.body["items"] if it["asset"]["author_user_id"] == cid]
        env.toggle_follow(u, cid)
        env.send_telemetry(u, env.make_event(u, target["scene_id"], "like"))
        r1 = env.feed(u, tab="for_you", limit=30)
        after = [it["explanation"]["breakdown"]["creator_affinity"] for it in r1.body["items"] if it["asset"]["author_user_id"] == cid]
        assert after and max(after) > (max(before) if before else 0.0)

    @reg.test("CS-T1-F06-04", 1, "CS-F06", "Sliding-window scatter diversity in ForYou (>=2 kinds, no >=3 same author runs)")
    def t1_f06_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f06")
        r = env.feed(u, tab="for_you", limit=12)
        items = assert_valid_feed_response(r.body, expected_limit=12)
        kinds = {it["asset"]["kind"] for it in items}
        assert len(kinds) >= 2, f"Expected >=2 distinct kinds in ForYou, got {kinds}"
        authors = [it["asset"]["author_user_id"] for it in items]
        if len(set(authors)) >= 3:
            for i in range(len(authors) - 2):
                assert not (authors[i] == authors[i + 1] == authors[i + 2]), f"3 consecutive identical authors: {authors}"

    @reg.test("CS-T1-F06-05", 1, "CS-F06", "Hard negative filtering: 0% re-appearance after not_interested/hide_author/hide_tag")
    def t1_f06_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f06")
        r0 = env.feed(u, tab="for_you", limit=18)
        items0 = r0.body["items"]
        s_dis = items0[0]["asset"]["scene_id"]
        a_hid = items0[1]["asset"]["author_user_id"]
        t_hid = items0[2]["asset"]["tags"][0]
        env.send_telemetry(u, [
            env.make_event(u, s_dis, "not_interested"),
            env.make_event(u, items0[1]["asset"]["scene_id"], "hide_author", author_user_id=a_hid),
            env.make_event(u, items0[2]["asset"]["scene_id"], "hide_tag", reason=t_hid),
        ])
        all_after, _ = env.feed_all_pages(u, tab="for_you", limit=20)
        for it in all_after:
            a = it["asset"]
            assert a["scene_id"] != s_dis, f"Disliked scene {s_dis} leaked"
            assert a["author_user_id"] != a_hid, f"Hidden author {a_hid} leaked"
            assert t_hid not in a["tags"], f"Hidden tag {t_hid} leaked"

    @reg.test("CS-T1-F06-06", 1, "CS-F06", "CreatorJustPublished fast-track surfaces newly published scene in top 10")
    def t1_f06_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f06")
        src, man = env.ensure_corpus(u).mutated_valid_script("animation", variant=3)
        rp = env.publish_asset(u, "FastTrack Neon", "Just published scene", "animation", ["fasttrack", "neon"], src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        rf = env.feed(u, tab="for_you", limit=10)
        ids = [it["asset"]["scene_id"] for it in rf.body["items"]]
        assert sid in ids, f"Newly published scene {sid} missing from creator ForYou top-10: {ids}"

    # ===================== CS-F07: Feed Session & Pagination (5) =====================
    @reg.test("CS-T1-F07-01", 1, "CS-F07", "Cursor pagination walks entire catalog with 0 duplicate scene_ids")
    def t1_f07_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f07")
        seen: set[str] = set()
        cur = None
        pages = 0
        max_pages = 30
        while pages < max_pages:
            r = env.feed(u, tab="for_you", session_id="sess_page_walk", cursor=cur, limit=6)
            assert r.status == 200
            items = assert_valid_feed_response(r.body, expected_limit=6)
            if pages == 0:
                max_pages = max(30, (int(r.body.get("total_catalog") or 0) // 6) + 15)
            for it in items:
                sid = it["asset"]["scene_id"]
                assert sid not in seen, f"Duplicate scene_id {sid} across cursor pages"
                seen.add(sid)
            cur = r.body.get("next_cursor")
            pages += 1
            if cur is None:
                break
        assert cur is None and len(seen) >= 18

    @reg.test("CS-T1-F07-02", 1, "CS-F07", "Live unseen-tail re-ranking shifts toward Games within <= 2 refreshes")
    def t1_f07_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f07")
        _, games = env.list_assets(u, {"kind": "game"})
        _, vids = env.list_assets(u, {"kind": "video"})
        for v in vids[:4]:
            env.send_telemetry(u, env.make_event(u, v["scene_id"], "fast_skip", session_id="sess_live", dwell_ms=450))
        for g in games[:4]:
            env.send_telemetry(u, [
                env.make_event(u, g["scene_id"], "game_interaction", session_id="sess_live", game_play_ms=30000, game_inputs=55, game_score=200.0),
                env.make_event(u, g["scene_id"], "double_tap_like", session_id="sess_live"),
            ])
        r = env.feed(u, tab="for_you", session_id="sess_live", limit=10)
        items = assert_valid_feed_response(r.body)
        g_cnt = sum(1 for it in items[:6] if it["asset"]["kind"] == "game")
        v_cnt = sum(1 for it in items[:6] if it["asset"]["kind"] == "video")
        assert g_cnt >= v_cnt and g_cnt >= 2, f"Expected games to rise: games={g_cnt}, videos={v_cnt}"

    @reg.test("CS-T1-F07-03", 1, "CS-F07", "Kind-tab purity for videos, animations, and games tabs")
    def t1_f07_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f07")
        for tab, kind in (("videos", "video"), ("animations", "animation"), ("games", "game")):
            r = env.feed(u, tab=tab, limit=25)
            items = assert_valid_feed_response(r.body)
            assert len(items) >= 6
            assert all(it["asset"]["kind"] == kind for it in items), f"Tab {tab} leaked wrong kind"

    @reg.test("CS-T1-F07-04", 1, "CS-F07", "tag_filter & query return only matching scenes")
    def t1_f07_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f07")
        r_all, all_assets = env.list_assets(u, {"limit": 100})
        assert r_all.status == 200 and len(all_assets) >= 1
        sample_tag = next(t for a in all_assets for t in (a.get("tags") or []) if isinstance(t, str) and t.strip())
        rf = env.feed(u, tab="for_you", tag_filter=sample_tag, limit=20)
        items = assert_valid_feed_response(rf.body)
        assert len(items) >= 1 and all(
            sample_tag.lower() in [t.lower() for t in it["asset"]["tags"]] for it in items
        ), f"tag_filter={sample_tag!r} returned non-matching items"
        sample_asset = next((a for a in all_assets if isinstance(a.get("title"), str) and a["title"].strip()), all_assets[0])
        tokens = [tok for tok in re.split(r"[\s_\-:]+", sample_asset["title"].strip()) if len(tok) >= 3]
        sample_query = tokens[0] if tokens else sample_asset["title"].strip()
        rq = env.feed(u, tab="for_you", query=sample_query, limit=20)
        qitems = assert_valid_feed_response(rq.body)
        assert len(qitems) >= 1, f"query={sample_query!r} returned 0 items"
        ql = sample_query.lower()
        assert all(
            ql in (it["asset"]["title"] + " " + it["asset"]["description"] + " " + " ".join(it["asset"]["tags"])).lower()
            for it in qitems
        ), f"query={sample_query!r} returned non-matching items"

    @reg.test("CS-T1-F07-05", 1, "CS-F07", "Publishing 1 scene increments total_catalog by +1")
    def t1_f07_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f07")
        before = env.feed(u, tab="for_you", limit=5).body["total_catalog"]
        src, man = env.ensure_corpus(u).mutated_valid_script("video", variant=4)
        rp = env.publish_asset(u, "Catalog Inc", "Test catalog count", "video", ["video"], src, man)
        assert rp.status in (200, 201)
        after = env.feed(u, tab="for_you", limit=5).body["total_catalog"]
        assert after == before + 1, f"Expected total_catalog {before + 1}, got {after}"

    # ===================== CS-F08: Dual-Engine AI Synthesizer (5) =====================
    @reg.test("CS-T1-F08-01", 1, "CS-F08", "Offline ai/generate produces valid Video, Animation & Game cs1 drafts")
    def t1_f08_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f08")
        for kind in ("video", "animation", "game"):
            r = env.ai_generate(u, f"Generate a cool neon {kind} scene", kind=kind)
            assert r.status in (200, 201)
            d = unwrap_draft(r.body)
            ok, ch, errs = env.extract_validation(env.validate_script(u, d["source_code"], d["manifest"]))
            assert ok and ch and not errs, f"AI {kind} draft failed validation: {errs}"

    @reg.test("CS-T1-F08-02", 1, "CS-F08", "Chinese zh-CN game prompt returns valid Game draft with draft_id")
    def t1_f08_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f08")
        r = env.ai_generate(u, "做一个霓虹打砖块小游戏")
        assert r.status in (200, 201)
        d = unwrap_draft(r.body)
        assert d.get("draft_id") and d["manifest"]["kind"] == "game"
        ok, ch, _ = env.extract_validation(env.validate_script(u, d["source_code"], d["manifest"]))
        assert ok and ch

    @reg.test("CS-T1-F08-03", 1, "CS-F08", "English animation shader prompt returns valid Animation draft")
    def t1_f08_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f08")
        r = env.ai_generate(u, "Create a glowing plasma wave animation shader")
        assert r.status in (200, 201)
        d = unwrap_draft(r.body)
        assert d["manifest"]["kind"] == "animation"
        ok, ch, _ = env.extract_validation(env.validate_script(u, d["source_code"], d["manifest"]))
        assert ok and ch

    @reg.test("CS-T1-F08-04", 1, "CS-F08", "ai/iterate refines existing draft_id into updated valid cs1 code")
    def t1_f08_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f08")
        d1 = unwrap_draft(env.ai_generate(u, "Make neon circle animation", kind="animation").body)
        ri = env.ai_iterate(u, draft_id=d1["draft_id"], prompt="Make faster particles and brighter glow")
        assert ri.status == 200
        d2 = unwrap_draft(ri.body)
        assert d2["source_code"] and d2["source_code"] != d1["source_code"]
        ok, ch, _ = env.extract_validation(env.validate_script(u, d2["source_code"], d2["manifest"]))
        assert ok and ch

    @reg.test("CS-T1-F08-05", 1, "CS-F08", "ai/remix produces runnable derived cs1 asset with remix_parent_id")
    def t1_f08_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f08")
        _, assets = env.list_assets(u)
        parent = assets[0]
        rr = env.ai_remix(u, parent["scene_id"], "Turn colors into synthwave magenta")
        assert rr.status in (200, 201)
        child = unwrap_asset(rr.body) or unwrap_draft(rr.body)
        assert child.get("remix_parent_id") == parent["scene_id"]
        ok, ch, _ = env.extract_validation(env.validate_script(u, child["source_code"], child["manifest"]))
        assert ok and ch

    # ===================== CS-F09: Server SQLite V42 Store & REST API (6) =====================
    @reg.test("CS-T1-F09-01", 1, "CS-F09", "POST /assets persists scene retrievable via GET /assets/{id} & /assets")
    def t1_f09_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f09")
        src, man = env.ensure_corpus(u).mutated_valid_script("game", variant=5)
        rp = env.publish_asset(u, "V42 Persisted Game", "DB persistence test", "game", ["v42"], src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        rd, detail = env.get_asset(u, sid)
        assert rd.status == 200 and detail is not None and detail["title"] == "V42 Persisted Game"
        rl, listed = env.list_assets(u, {"q": "V42 Persisted Game", "limit": 100})
        assert rl.status == 200 and any(a["scene_id"] == sid for a in listed)

    @reg.test("CS-T1-F09-02", 1, "CS-F09", "POST /assets/{id}/like toggles true->false->true with exact likes_count")
    def t1_f09_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f09")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        n0 = assets[0]["likes_count"]
        r1 = env.toggle_like(u, sid)
        assert r1.status == 200 and r1.body["liked"] is True and r1.body["likes_count"] == n0 + 1
        r2 = env.toggle_like(u, sid)
        assert r2.status == 200 and r2.body["liked"] is False and r2.body["likes_count"] == n0
        r3 = env.toggle_like(u, sid)
        assert r3.status == 200 and r3.body["liked"] is True and r3.body["likes_count"] == n0 + 1
        _, d = env.get_asset(u, sid)
        assert d is not None and d["viewer_liked"] is True and d["likes_count"] == n0 + 1

    @reg.test("CS-T1-F09-03", 1, "CS-F09", "POST /assets/{id}/bookmark toggles cleanly with exact bookmarks_count")
    def t1_f09_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f09")
        _, assets = env.list_assets(u)
        sid = assets[1]["scene_id"]
        n0 = assets[1]["bookmarks_count"]
        r1 = env.toggle_bookmark(u, sid)
        assert r1.status == 200 and r1.body["bookmarked"] is True and r1.body["bookmarks_count"] == n0 + 1
        r2 = env.toggle_bookmark(u, sid)
        assert r2.status == 200 and r2.body["bookmarked"] is False and r2.body["bookmarks_count"] == n0
        _, d = env.get_asset(u, sid)
        assert d is not None and d["viewer_bookmarked"] is False and d["bookmarks_count"] == n0

    @reg.test("CS-T1-F09-04", 1, "CS-F09", "POST/GET /assets/{id}/comments persists comment & increments count")
    def t1_f09_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f09")
        _, assets = env.list_assets(u)
        sid = assets[2]["scene_id"]
        c0 = assets[2]["comments_count"]
        rc = env.post_comment(u, sid, "Great procedural shader! 很酷🔥")
        assert rc.status in (200, 201)
        rl, cmts = env.list_comments(u, sid)
        assert rl.status == 200 and any("很酷🔥" in extract_comment_text(c) for c in cmts)
        _, d = env.get_asset(u, sid)
        assert d is not None and d["comments_count"] == c0 + 1

    @reg.test("CS-T1-F09-05", 1, "CS-F09", "POST /creators/{id}/follow toggles follow & updates viewer_following_author")
    def t1_f09_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f09")
        _, assets = env.list_assets(u)
        a = assets[0]
        r1 = env.toggle_follow(u, a["author_user_id"])
        assert r1.status == 200 and extract_follow_flag(r1.body) is True
        _, d1 = env.get_asset(u, a["scene_id"])
        assert d1 is not None and d1["viewer_following_author"] is True
        r2 = env.toggle_follow(u, a["author_user_id"])
        assert r2.status == 200 and extract_follow_flag(r2.body) is False

    @reg.test("CS-T1-F09-06", 1, "CS-F09", "SQLite V42 restart durability: scene, like, bookmark, comment & follow survive")
    def t1_f09_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f09_rst")
        src, man = env.ensure_corpus(u).mutated_valid_script("animation", variant=6)
        rp = env.publish_asset(u, "Survives Restart", "Durability proof", "animation", ["durability"], src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        _, seeds = env.list_assets(u)
        seed_author = next(a["author_user_id"] for a in seeds if a["author_user_id"] != u.user_id)
        env.toggle_like(u, sid)
        env.toggle_bookmark(u, sid)
        env.post_comment(u, sid, "Pre-restart durable comment")
        env.toggle_follow(u, seed_author)
        env.restart_server()
        rd, d = env.get_asset(u, sid)
        assert rd.status == 200 and d is not None and d["title"] == "Survives Restart"
        assert d["viewer_liked"] is True and d["likes_count"] >= 1
        assert d["viewer_bookmarked"] is True and d["bookmarks_count"] >= 1
        assert d["comments_count"] >= 1
        _, cmts = env.list_comments(u, sid)
        assert any("Pre-restart durable comment" in extract_comment_text(c) for c in cmts)
        rf = env.feed(u, tab="following", limit=20)
        assert rf.status == 200 and any(it["asset"]["author_user_id"] == seed_author for it in rf.body["items"])

    # ===================== CS-F10: AI Bot Creation & Publishing Pipeline (5) =====================
    @reg.test("CS-T1-F10-01", 1, "CS-F10", "ai/generate -> ai/publish persists scene with bot attribution")
    def t1_f10_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f10")
        d = unwrap_draft(env.ai_generate(u, "Make a cosmic particle tunnel animation", kind="animation").body)
        rp = env.ai_publish(u, draft_id=d["draft_id"])
        assert rp.status in (200, 201)
        a = unwrap_asset(rp.body)
        assert a is not None and (a.get("creator_bot_username") or a.get("author_username"))
        rd, detail = env.get_asset(u, a["scene_id"])
        assert rd.status == 200 and detail is not None

    @reg.test("CS-T1-F10-02", 1, "CS-F10", "Multi-turn ai/generate -> ai/iterate -> ai/publish publishes latest code")
    def t1_f10_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f10")
        d1 = unwrap_draft(env.ai_generate(u, "Simple pong game", kind="game").body)
        draft_id = d1["draft_id"]
        d2 = unwrap_draft(env.ai_iterate(u, draft_id=draft_id, prompt="Add neon trails and combo score").body)
        draft_id = d2.get("draft_id") or draft_id
        rp = env.ai_publish(u, draft_id=draft_id)
        assert rp.status in (200, 201)
        a = unwrap_asset(rp.body)
        assert a is not None and a["source_code"] == d2["source_code"] and a["source_code"] != d1["source_code"]

    @reg.test("CS-T1-F10-03", 1, "CS-F10", "ai/publish with chat_id posts GenerativeCard into target chat")
    def t1_f10_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f10")
        cid = env.create_chat(u, title="Bot Studio Room")
        d = unwrap_draft(env.ai_generate(u, "Neon rhythm video", kind="video").body)
        rp = env.ai_publish(u, draft_id=d["draft_id"], chat_id=cid)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        msgs = env.list_chat_messages(u, cid)
        assert any(isinstance(m.get("content"), dict) and m["content"].get("type") == "GenerativeCard" for m in msgs)
        card_msg = next(m for m in msgs if isinstance(m.get("content"), dict) and m["content"].get("type") == "GenerativeCard")
        assert_valid_generative_card(card_msg, sid)

    @reg.test("CS-T1-F10-04", 1, "CS-F10", "ai/remix sets remix_parent_id and increments parent remixes_count")
    def t1_f10_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f10")
        _, assets = env.list_assets(u)
        parent = assets[0]
        p_before = parent["remixes_count"]
        rr = env.ai_remix(u, parent["scene_id"], "Make it golden cyberpunk")
        assert rr.status in (200, 201)
        child = unwrap_asset(rr.body)
        assert child is not None and child["remix_parent_id"] == parent["scene_id"]
        _, p_after = env.get_asset(u, parent["scene_id"])
        assert p_after is not None and p_after["remixes_count"] == p_before + 1

    @reg.test("CS-T1-F10-05", 1, "CS-F10", "Published AI scene appears immediately in my_works & ForYou")
    def t1_f10_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f10")
        d = unwrap_draft(env.ai_generate(u, "Glowing firefly animation", kind="animation").body)
        sid = unwrap_asset(env.ai_publish(u, draft_id=d["draft_id"]).body)["scene_id"]
        rm = env.feed(u, tab="my_works", limit=20)
        assert any(it["asset"]["scene_id"] == sid for it in rm.body["items"])
        rf = env.feed(u, tab="for_you", limit=10)
        assert any(it["asset"]["scene_id"] == sid for it in rf.body["items"])

    # ===================== CS-F11: Client Sync & Account Isolation (5) =====================
    @reg.test("CS-T1-F11-01", 1, "CS-F11", "Multi-account social isolation: global counts shared, viewer flags isolated")
    def t1_f11_01(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t1f11a"), env.register_user("t1f11b")
        _, assets = env.list_assets(ua)
        a = assets[3]
        env.toggle_like(ua, a["scene_id"])
        env.toggle_bookmark(ua, a["scene_id"])
        env.toggle_follow(ua, a["author_user_id"])
        _, db = env.get_asset(ub, a["scene_id"])
        assert db is not None and db["likes_count"] >= 1 and db["bookmarks_count"] >= 1
        assert db["viewer_liked"] is False and db["viewer_bookmarked"] is False and db["viewer_following_author"] is False

    @reg.test("CS-T1-F11-02", 1, "CS-F11", "Multi-account negative feedback isolation: A's hides do not affect B")
    def t1_f11_02(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t1f11a"), env.register_user("t1f11b")
        _, assets = env.list_assets(ua)
        target = assets[0]
        env.send_telemetry(ua, [
            env.make_event(ua, target["scene_id"], "not_interested"),
            env.make_event(ua, target["scene_id"], "hide_author", author_user_id=target["author_user_id"]),
        ])
        b_items, _ = env.feed_all_pages(ub, tab="for_you", limit=20)
        assert any(it["asset"]["scene_id"] == target["scene_id"] for it in b_items)

    @reg.test("CS-T1-F11-03", 1, "CS-F11", "Offline telemetry outbox batch sync (6 events) accepted atomically")
    def t1_f11_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f11")
        _, assets = env.list_assets(u)
        batch = [env.make_event(u, assets[i]["scene_id"], "dwell", dwell_ms=6000 + i * 500) for i in range(6)]
        r = env.send_telemetry(u, batch)
        assert r.status == 200 and int(r.body["accepted"]) == 6 and r.body["profile_updated"] is True

    @reg.test("CS-T1-F11-04", 1, "CS-F11", "Account-scoped my_works isolation between User A and User B")
    def t1_f11_04(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t1f11a"), env.register_user("t1f11b")
        src, man = env.ensure_corpus(ua).mutated_valid_script("video", variant=8)
        sid = unwrap_asset(env.publish_asset(ua, "User A Private Work", "Scoped", "video", ["mine"], src, man).body)["scene_id"]
        assert any(it["asset"]["scene_id"] == sid for it in env.feed(ua, tab="my_works", limit=20).body["items"])
        assert all(it["asset"]["scene_id"] != sid for it in env.feed(ub, tab="my_works", limit=20).body["items"])

    @reg.test("CS-T1-F11-05", 1, "CS-F11", "Feed-vs-detail cache coherence after like + bookmark + comment")
    def t1_f11_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f11")
        _, assets = env.list_assets(u)
        sid = assets[4]["scene_id"]
        env.toggle_like(u, sid)
        env.toggle_bookmark(u, sid)
        env.post_comment(u, sid, "Coherence check")
        _, detail = env.get_asset(u, sid)
        feed_items, _ = env.feed_all_pages(u, tab="for_you", limit=20)
        from_feed = next(it["asset"] for it in feed_items if it["asset"]["scene_id"] == sid)
        for k in ("likes_count", "bookmarks_count", "comments_count", "viewer_liked", "viewer_bookmarked", "updated_at_ms"):
            assert detail[k] == from_feed[k], f"Mismatch on {k}: detail={detail[k]} feed={from_feed[k]}"

    # ===================== CS-F12: Hybrid 60fps Viewport & Game Focus (5) =====================
    @reg.test("CS-T1-F12-01", 1, "CS-F12", "100% of catalog assets obey viewport manifest bounds & cs1 format")
    def t1_f12_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f12")
        _, assets = env.list_assets(u)
        assert len(assets) >= 18
        for a in assets:
            assert a["code_format"] == "cs1"
            assert_valid_manifest(a["manifest"], expected_kind=a["kind"])

    @reg.test("CS-T1-F12-02", 1, "CS-F12", "Dry-run active draw contract across Video, Animation & Game")
    def t1_f12_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f12")
        corpus = env.ensure_corpus(u)
        for kind in ("video", "animation", "game"):
            src, man = corpus.valid_script(kind)
            r = env.validate_script(u, src, man)
            ok, ch, _ = env.extract_validation(r)
            assert ok and ch
            if isinstance(r.body, dict):
                if r.body.get("draw_cmds_count") is not None:
                    assert 1 <= int(r.body["draw_cmds_count"]) <= 2_048
                elif isinstance(r.body.get("draw_cmds_per_frame"), list) and r.body["draw_cmds_per_frame"]:
                    assert 1 <= max(int(x) for x in r.body["draw_cmds_per_frame"]) <= 2_048

    @reg.test("CS-T1-F12-03", 1, "CS-F12", "Multi-aspect-ratio publishing (360x640, 320x320, 720x1280) preserved")
    def t1_f12_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f12")
        src, base_man = env.ensure_corpus(u).valid_script("animation")
        for idx, (w, h, fps) in enumerate(((360, 640, 60), (320, 320, 30), (720, 1280, 60))):
            man = dict(base_man, width=w, height=h, fps=fps, seed=500 + idx)
            rp = env.publish_asset(u, f"Aspect {w}x{h}", "Aspect test", "animation", ["aspect"], src, man)
            assert rp.status in (200, 201)
            a = unwrap_asset(rp.body)
            assert (a["manifest"]["width"], a["manifest"]["height"], a["manifest"]["fps"]) == (w, h, fps)

    @reg.test("CS-T1-F12-04", 1, "CS-F12", "Playable Game focus metadata & game_interaction distinct from dwell")
    def t1_f12_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f12")
        _, games = env.list_assets(u, {"kind": "game"})
        g = games[0]
        assert g["controls_hint"].strip()
        r_dwell = env.send_telemetry(u, env.make_event(u, g["scene_id"], "dwell", dwell_ms=5000))
        r_game = env.send_telemetry(
            u,
            env.make_event(
                u, g["scene_id"], "game_interaction",
                game_play_ms=30000, game_inputs=60, game_restarts=1, game_score=250.0,
            ),
        )
        assert r_dwell.status == 200 and r_game.status == 200
        assert float(r_game.body["reward"]) > float(r_dwell.body["reward"]) > 0.0

    @reg.test("CS-T1-F12-05", 1, "CS-F12", "Video seeds have finite duration_sec > 0; Animation seeds have fps >= 30")
    def t1_f12_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f12")
        _, vids = env.list_assets(u, {"kind": "video"})
        _, anis = env.list_assets(u, {"kind": "animation"})
        assert len(vids) >= 6 and len(anis) >= 6
        assert all(isinstance(v["manifest"]["duration_sec"], (int, float)) and v["manifest"]["duration_sec"] > 0 for v in vids)
        assert all(a["manifest"]["fps"] >= 30 for a in anis)

    # ===================== CS-F13: Douyin Vertical Snap Feed, HUD & Heart Burst (5) =====================
    @reg.test("CS-T1-F13-01", 1, "CS-F13", "Switching across all 6 floating top tabs with consistent session returns tab-accurate feeds")
    def t1_f13_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f13")
        sess = "sess_tabs_hud"
        for tab in ("for_you", "following", "videos", "animations", "games", "my_works"):
            r = env.feed(u, tab=tab, session_id=sess, limit=10)
            assert r.status == 200
            items = assert_valid_feed_response(r.body, expected_limit=10)
            if tab == "for_you":
                assert len(items) >= 1, "for_you tab returned empty list"
            elif tab == "videos":
                assert len(items) >= 1 and all(it["asset"]["kind"] == "video" for it in items)
            elif tab == "animations":
                assert len(items) >= 1 and all(it["asset"]["kind"] == "animation" for it in items)
            elif tab == "games":
                assert len(items) >= 1 and all(it["asset"]["kind"] == "game" for it in items)

    @reg.test("CS-T1-F13-02", 1, "CS-F13", "Double-tap heart burst: double_tap_like telemetry + POST /like converge")
    def t1_f13_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f13")
        _, assets = env.list_assets(u)
        sid = assets[5]["scene_id"]
        n0 = assets[5]["likes_count"]
        rt = env.send_telemetry(u, env.make_event(u, sid, "double_tap_like"))
        assert rt.status == 200 and float(rt.body["reward"]) > 0.0
        rl = env.toggle_like(u, sid)
        assert rl.status == 200 and rl.body["liked"] is True and rl.body["likes_count"] == n0 + 1
        _, d = env.get_asset(u, sid)
        assert d is not None and d["viewer_liked"] is True

    @reg.test("CS-T1-F13-03", 1, "CS-F13", "Right action rail counter convergence (Like/Bookmark/Comment/Share/Remix)")
    def t1_f13_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f13")
        _, assets = env.list_assets(u)
        a0 = assets[6]
        sid = a0["scene_id"]
        env.toggle_like(u, sid)
        env.toggle_bookmark(u, sid)
        env.post_comment(u, sid, "Right rail comment")
        env.send_telemetry(u, env.make_event(u, sid, "share"))
        env.ai_remix(u, sid, "Right rail remix")
        _, a1 = env.get_asset(u, sid)
        assert a1 is not None
        assert a1["likes_count"] == a0["likes_count"] + 1
        assert a1["bookmarks_count"] == a0["bookmarks_count"] + 1
        assert a1["comments_count"] == a0["comments_count"] + 1
        assert a1["shares_count"] >= a0["shares_count"] + 1
        assert a1["remixes_count"] == a0["remixes_count"] + 1

    @reg.test("CS-T1-F13-04", 1, "CS-F13", "Bottom-left HUD payload completeness on ForYou cards")
    def t1_f13_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f13")
        r = env.feed(u, tab="for_you", limit=10)
        items = assert_valid_feed_response(r.body)
        assert len(items) >= 5
        for it in items:
            a, e = it["asset"], it["explanation"]
            assert a["author_username"] and a["title"] and len(a["tags"]) >= 1
            assert a["manifest"]["fps"] >= 15 and a["controls_hint"].strip()
            assert e["primary_reason_zh"].strip() and e["primary_reason_en"].strip()

    @reg.test("CS-T1-F13-05", 1, "CS-F13", "Video timeline complete + replay + dwell reporting updates engagement")
    def t1_f13_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f13")
        _, vids = env.list_assets(u, {"kind": "video"})
        v0 = vids[0]
        evs = [
            env.make_event(u, v0["scene_id"], "complete", dwell_ms=15000, completion_ratio=1.0),
            env.make_event(u, v0["scene_id"], "replay", dwell_ms=30000, completion_ratio=2.5, loop_count=3),
            env.make_event(u, v0["scene_id"], "dwell", dwell_ms=12000, completion_ratio=0.9),
        ]
        r = env.send_telemetry(u, evs)
        assert r.status == 200 and int(r.body["accepted"]) == 3 and float(r.body["reward"]) > 0.0
        _, v1 = env.get_asset(u, v0["scene_id"])
        assert v1 is not None and v1["plays_count"] >= v0["plays_count"] + 1

    # ===================== CS-F14: Slide-Over Drawers (5) =====================
    @reg.test("CS-T1-F14-01", 1, "CS-F14", "Comments drawer CRUD preserves ASCII + CJK + emoji & exact count")
    def t1_f14_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f14")
        _, assets = env.list_assets(u)
        sid = assets[7]["scene_id"]
        c0 = assets[7]["comments_count"]
        texts = ["First ASCII comment", "代码场景太丝滑了✨🚀", "Mixed CJK & emoji 🎨123"]
        for t in texts:
            assert env.post_comment(u, sid, t).status in (200, 201)
        rl, cmts = env.list_comments(u, sid)
        assert rl.status == 200
        bodies = [extract_comment_text(c) for c in cmts]
        for t in texts:
            assert t in bodies, f"Missing comment {t!r}"
        _, d = env.get_asset(u, sid)
        assert d is not None and d["comments_count"] == c0 + len(texts)

    @reg.test("CS-T1-F14-02", 1, "CS-F14", "Live Code Editor: edit seeded cs1 -> validate updated cover_hash -> publish")
    def t1_f14_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f14")
        corpus = env.ensure_corpus(u)
        src1, man1 = corpus.valid_script("animation")
        ok1, h1, _ = env.extract_validation(env.validate_script(u, src1, man1))
        src2, man2 = corpus.mutated_valid_script("animation", variant=11)
        ok2, h2, _ = env.extract_validation(env.validate_script(u, src2, man2))
        assert ok1 and ok2 and h1 != h2
        rp = env.publish_asset(u, "HotReload Edited Shader", "Edited via </>", "animation", ["edited"], src2, man2)
        assert rp.status in (200, 201) and unwrap_asset(rp.body) is not None

    @reg.test("CS-T1-F14-03", 1, "CS-F14", "Creator Profile Sheet follow propagates across ALL creator scenes")
    def t1_f14_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f14")
        _, assets = env.list_assets(u)
        by_author: dict[int, List[str]] = {}
        for a in assets:
            by_author.setdefault(a["author_user_id"], []).append(a["scene_id"])
        cid, sids = next((k, v) for k, v in by_author.items() if len(v) >= 2)
        assert env.toggle_follow(u, cid).status == 200
        for sid in sids[:3]:
            _, d = env.get_asset(u, sid)
            assert d is not None and d["viewer_following_author"] is True

    @reg.test("CS-T1-F14-04", 1, "CS-F14", "In-Scene AI Bot Studio flow: generate -> iterate -> publish -> GET")
    def t1_f14_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f14")
        d1 = unwrap_draft(env.ai_generate(u, "Neon vortex shader", kind="animation").body)
        draft_id = d1["draft_id"]
        d2 = unwrap_draft(env.ai_iterate(u, draft_id=draft_id, prompt="Increase glow rings").body)
        draft_id = d2.get("draft_id") or draft_id
        rp = env.ai_publish(u, draft_id=draft_id)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        rd, detail = env.get_asset(u, sid)
        assert rd.status == 200 and detail is not None and detail["source_code"] == d2["source_code"]

    @reg.test("CS-T1-F14-05", 1, "CS-F14", "Studio Remix flow: remix seeded asset -> parent remixes_count + 1")
    def t1_f14_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f14")
        _, assets = env.list_assets(u)
        p = assets[8]
        rr = env.ai_remix(u, p["scene_id"], "Make night-mode cyber grid")
        assert rr.status in (200, 201)
        c = unwrap_asset(rr.body)
        assert c is not None and c["remix_parent_id"] == p["scene_id"]
        _, p2 = env.get_asset(u, p["scene_id"])
        assert p2 is not None and p2["remixes_count"] == p["remixes_count"] + 1

    # ===================== CS-F15: Left Sidebar, AppShell & Chat Interop (5) =====================
    @reg.test("CS-T1-F15-01", 1, "CS-F15", "Chat GenerativeCard exposes code_scene:open, remix & share payloads")
    def t1_f15_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f15")
        cid = env.create_chat(u, title="Code Scene Card Chat")
        d = unwrap_draft(env.ai_generate(u, "Cyber arcade game", kind="game").body)
        rp = env.ai_publish(u, draft_id=d["draft_id"], chat_id=cid)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        msgs = env.list_chat_messages(u, cid)
        card_msg = next(m for m in reversed(msgs) if isinstance(m.get("content"), dict) and m["content"].get("type") == "GenerativeCard")
        assert_valid_generative_card(card_msg, sid)

    @reg.test("CS-T1-F15-02", 1, "CS-F15", "Deep-link code_scene:open:<id> resolves exact scene via GET /assets/<id>")
    def t1_f15_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f15")
        cid = env.create_chat(u, title="Deep Link Open")
        d = unwrap_draft(env.ai_generate(u, "Starlight animation", kind="animation").body)
        sid = unwrap_asset(env.ai_publish(u, draft_id=d["draft_id"], chat_id=cid).body)["scene_id"]
        msgs = env.list_chat_messages(u, cid)
        card_msg = next(m for m in reversed(msgs) if isinstance(m.get("content"), dict) and m["content"].get("type") == "GenerativeCard")
        payloads = assert_valid_generative_card(card_msg, sid)
        open_id = next(p.split(":", 2)[2] for p in payloads if p.startswith("code_scene:open:"))
        rd, detail = env.get_asset(u, open_id)
        assert rd.status == 200 and detail is not None and detail["scene_id"] == sid

    @reg.test("CS-T1-F15-03", 1, "CS-F15", "Deep-link code_scene:remix:<id> resolves & creates valid child remix")
    def t1_f15_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f15")
        cid = env.create_chat(u, title="Deep Link Remix")
        d = unwrap_draft(env.ai_generate(u, "Retro synth video", kind="video").body)
        sid = unwrap_asset(env.ai_publish(u, draft_id=d["draft_id"], chat_id=cid).body)["scene_id"]
        msgs = env.list_chat_messages(u, cid)
        card_msg = next(m for m in reversed(msgs) if isinstance(m.get("content"), dict) and m["content"].get("type") == "GenerativeCard")
        payloads = assert_valid_generative_card(card_msg, sid)
        remix_id = next(p.split(":", 2)[2] for p in payloads if p.startswith("code_scene:remix:"))
        rr = env.ai_remix(u, remix_id, "Remix from chat card")
        assert rr.status in (200, 201) and unwrap_asset(rr.body)["remix_parent_id"] == sid

    @reg.test("CS-T1-F15-04", 1, "CS-F15", "Dual-locale zh-CN (CJK) & en (English) explanation parity on ForYou")
    def t1_f15_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t1f15")
        r = env.feed(u, tab="for_you", limit=12)
        items = assert_valid_feed_response(r.body)
        assert len(items) >= 5
        for it in items:
            assert_valid_explanation(it["explanation"], require_cjk_zh=True)

    @reg.test("CS-T1-F15-05", 1, "CS-F15", "Fresh user empty chats invariant & Share-to-Chat GenerativeCard forwarding")
    def t1_f15_05(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t1f15a"), env.register_user("t1f15b")
        rc0 = env.client().json("GET", "/api/v1/chats", token=ua.token)
        assert rc0.status == 200 and rc0.body["total"] == 0 and rc0.body["chats"] == []
        _, assets = env.list_assets(ua)
        sid = assets[0]["scene_id"]
        cid = env.create_chat(ua, title="Share Forward Chat", member_ids=[ub.user_id])
        card = {
            "type": "GenerativeCard",
            "data": {
                "card_id": f"fwd_{sid}", "title": assets[0]["title"], "subtitle": "Shared Code Scene",
                "layout": "VerticalStack", "status": "published",
                "components": [
                    {"type": "Button", "id": "open", "label": "Open", "action_payload": f"code_scene:open:{sid}"},
                    {"type": "Button", "id": "remix", "label": "Remix", "action_payload": f"code_scene:remix:{sid}"},
                    {"type": "Button", "id": "share", "label": "Share", "action_payload": f"code_scene:share:{sid}"},
                ],
            },
        }
        rs = env.client().json("POST", f"/api/v1/chats/{cid}/messages", {"content": card}, token=ua.token)
        assert rs.status in (200, 201)
        b_msgs = env.list_chat_messages(ub, cid)
        assert len(b_msgs) >= 1 and assert_valid_generative_card(b_msgs[-1], sid)
