"""Tier 2: Boundary & Corner Case Tests (79 tests across CS-F01..CS-F15).

Covers fail-closed sandbox budgets, manifest bounds, extreme telemetry values,
auth/404/403 security sweeps, SQLi/XSS, toggle idempotency, and pagination limits.
NEVER imports _contract_double.py. Pure Python 3 stdlib only.
"""

from __future__ import annotations

import math
from typing import Any, Optional

from .harness import (
    CodeSceneEnv,
    TestRegistry,
    assert_all_floats_finite,
    assert_valid_asset,
    assert_valid_feed_response,
    assert_valid_generative_card,
    assert_valid_manifest,
    assert_valid_score_breakdown,
    extract_comment_text,
    extract_follow_flag,
    unwrap_asset,
    unwrap_draft,
)


def _assert_rejected_validation(env: CodeSceneEnv, r_val, r_pub=None, label: str = "") -> None:
    ok, _, _ = env.extract_validation(r_val)
    assert (r_val.status == 422) or (r_val.status == 200 and not ok), (
        f"{label}: expected validation rejection (422 or valid=False), got {r_val.status} {r_val.body!r}"
    )
    if r_pub is not None:
        assert r_pub.status == 422, f"{label}: expected publish 422, got {r_pub.status} {r_pub.body!r}"


def _extract_explicit_draw_count(body: Any) -> Optional[int]:
    if not isinstance(body, dict):
        return None
    if "draw_cmds_count" in body and isinstance(body["draw_cmds_count"], int) and not isinstance(body["draw_cmds_count"], bool):
        return body["draw_cmds_count"]
    per_frame = body.get("draw_cmds_per_frame")
    if isinstance(per_frame, list) and len(per_frame) >= 1 and all(
        isinstance(x, int) and not isinstance(x, bool) for x in per_frame
    ):
        return max(per_frame)
    return None


def register_tier2_tests(reg: TestRegistry) -> None:
    # ===================== CS-F01: Shared Contracts Boundaries (5) =====================
    @reg.test("CS-T2-F01-01", 2, "CS-F01", "Invalid AssetKind strings ('3d_vr', '', 'VIDEO') rejected 400/422")
    def t2_f01_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f01")
        src, man = env.ensure_corpus(u).valid_script("animation")
        for bad in ("3d_vr", "", "VIDEO"):
            m = dict(man, kind=bad)
            r = env.publish_asset(u, "Bad Kind", "Desc", bad, ["x"], src, m)
            assert r.status in (400, 422), f"kind={bad!r} got {r.status}"

    @reg.test("CS-T2-F01-02", 2, "CS-F01", "Invalid CodeSceneFeedTab ('trending', '') rejected 400/422")
    def t2_f01_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f01")
        for bad in ("trending", "", "FOR_YOU"):
            r = env.feed(u, tab=bad, limit=5)
            assert r.status in (400, 422), f"tab={bad!r} got {r.status}"

    @reg.test("CS-T2-F01-03", 2, "CS-F01", "Unsupported code_format ('js', 'wasm', '') rejected 400/422")
    def t2_f01_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f01")
        src, man = env.ensure_corpus(u).valid_script("animation")
        for fmt in ("js", "wasm", "cs2", ""):
            r = env.publish_asset(u, "Bad Format", "Desc", "animation", ["x"], src, man, code_format=fmt)
            assert r.status in (400, 422), f"code_format={fmt!r} got {r.status}"

    @reg.test("CS-T2-F01-04", 2, "CS-F01", "Missing required fields ({}, missing source_code/manifest) rejected 400/422")
    def t2_f01_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f01")
        r_pub = env.client().json("POST", "/api/v1/code-scene/assets", {}, token=u.token)
        assert r_pub.status in (400, 422), f"Empty dict on POST /assets got {r_pub.status}: {r_pub.body!r}"
        r_val = env.client().json("POST", "/api/v1/code-scene/assets/validate", {}, token=u.token)
        ok_val, _, errs_val = env.extract_validation(r_val)
        assert (r_val.status in (400, 422)) or (r_val.status == 200 and not ok_val and len(errs_val) >= 1), (
            f"Empty dict on POST /assets/validate was not rejected: {r_val.status} {r_val.body!r}"
        )

    @reg.test("CS-T2-F01-05", 2, "CS-F01", "Malformed/truncated JSON & wrong root type rejected 400/422 (never 500)")
    def t2_f01_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f01")
        hdrs = {"Content-Type": "application/json"}
        for raw in (b"{broken", b"[1,2,3]", b"not-json"):
            r = env.client().request("POST", "/api/v1/code-scene/assets", raw_body=raw, headers=hdrs, token=u.token)
            assert r.status in (400, 422), f"Malformed body {raw!r} got {r.status}"

    # ===================== CS-F02: cs1 VM & Sandbox Hostile Inputs (6) =====================
    @reg.test("CS-T2-F02-01", 2, "CS-F02", "Oversized scripts (65_537B & 70KB) rejected with 422 / valid=False")
    def t2_f02_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f02")
        corpus = env.ensure_corpus(u)
        for sz in (65_537, 70_000):
            src, man = corpus.oversized_script(sz)
            rv = env.validate_script(u, src, man)
            rp = env.publish_asset(u, "Oversized", "Too big", "animation", ["big"], src, man)
            _assert_rejected_validation(env, rv, rp, f"size={sz}")

    @reg.test("CS-T2-F02-02", 2, "CS-F02", "Infinite-loop script rejected with 422 / valid=False in < 2.0s wall time")
    def t2_f02_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f02")
        corpus = env.ensure_corpus(u)
        b_src, b_man = corpus.splice_benign_noop("animation")
        ok_b, _, _ = env.extract_validation(env.validate_script(u, b_src, b_man))
        assert ok_b, "Differential benign control splice failed validation"
        src, man = corpus.hostile_infinite_loop_script()
        rv = env.validate_script(u, src, man)
        assert rv.elapsed_ms < 2000.0, f"Infinite loop took too long: {rv.elapsed_ms:.1f}ms"
        rp = env.publish_asset(u, "InfLoop", "Hostile loop", "animation", ["bad"], src, man)
        assert rp.elapsed_ms < 2000.0
        _assert_rejected_validation(env, rv, rp, "infinite_loop")

    @reg.test("CS-T2-F02-03", 2, "CS-F02", "Deep recursion (>48 call depth) rejected cleanly without server crash")
    def t2_f02_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f02")
        src, man = env.ensure_corpus(u).hostile_deep_recursion_script()
        rv = env.validate_script(u, src, man)
        rp = env.publish_asset(u, "RecursionBomb", "Stack bomb", "animation", ["bad"], src, man)
        _assert_rejected_validation(env, rv, rp, "deep_recursion")
        assert env.client().request("GET", "/health").status == 200

    @reg.test("CS-T2-F02-04", 2, "CS-F02", "OOM heap bomb (>32_768 values) & bytecode bomb (>16_384 ops) rejected")
    def t2_f02_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f02")
        corpus = env.ensure_corpus(u)
        h_src, h_man = corpus.hostile_heap_bomb_script()
        _assert_rejected_validation(env, env.validate_script(u, h_src, h_man), env.publish_asset(u, "Heap", "Bomb", "animation", ["bad"], h_src, h_man), "heap_bomb")
        b_src, b_man = corpus.hostile_bytecode_bomb_script()
        _assert_rejected_validation(env, env.validate_script(u, b_src, b_man), env.publish_asset(u, "Bytecode", "Bomb", "animation", ["bad"], b_src, b_man), "bytecode_bomb")

    @reg.test("CS-T2-F02-05", 2, "CS-F02", "NaN/Inf coordinates & div-by-zero safely rejected or sanitized (finite JSON, no 500)")
    def t2_f02_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f02")
        src, man = env.ensure_corpus(u).hostile_nan_inf_script()
        rv = env.validate_script(u, src, man)
        assert rv.status in (200, 422), f"NaN script caused unexpected status {rv.status}"
        if rv.status == 200:
            assert isinstance(rv.body, dict), f"200 validate response must be JSON object, got {rv.body!r}"
            flag = rv.body.get("valid") if "valid" in rv.body else rv.body.get("ok")
            assert isinstance(flag, bool), f"200 validate response missing bool valid/ok: {rv.body!r}"
            ok, ch, errs = env.extract_validation(rv)
            if ok:
                assert ch, f"200 valid=True response missing cover_hash: {rv.body!r}"
            else:
                assert len(errs) >= 1, f"200 valid=False response missing diagnostics/errors: {rv.body!r}"
        assert_all_floats_finite(rv.body, "validate_nan_inf")
        assert env.client().request("GET", "/health").status == 200

    @reg.test("CS-T2-F02-06", 2, "CS-F02", "AST nesting bomb (>64 depth) & empty/comment-only sources rejected")
    def t2_f02_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f02")
        corpus = env.ensure_corpus(u)
        ast_src, ast_man = corpus.hostile_ast_depth_script(80)
        _assert_rejected_validation(env, env.validate_script(u, ast_src, ast_man), env.publish_asset(u, "AstBomb", "Deep", "animation", ["bad"], ast_src, ast_man), "ast_depth")
        for empty in corpus.hostile_empty_scripts():
            _assert_rejected_validation(env, env.validate_script(u, empty, ast_man), env.publish_asset(u, "Empty", "None", "animation", ["bad"], empty, ast_man), "empty_src")

    # ===================== CS-F03: Execution Mode Boundaries (5) =====================
    @reg.test("CS-T2-F03-01", 2, "CS-F03", "Video invalid/missing duration_sec (None, 0.0, -5.0) rejected or clamped")
    def t2_f03_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f03")
        src, base = env.ensure_corpus(u).valid_script("video")
        for bad_dur in (None, 0.0, -5.0):
            m = dict(base, duration_sec=bad_dur)
            r = env.publish_asset(u, "Bad Video Dur", "Desc", "video", ["v"], src, m)
            assert r.status in (400, 422), f"duration_sec={bad_dur!r} unexpectedly returned {r.status}"
        m_hi = dict(base, duration_sec=10000.0)
        r_hi = env.publish_asset(u, "Hi Video Dur", "Desc", "video", ["v"], src, m_hi)
        assert r_hi.status in (200, 201, 400, 422)
        if r_hi.status in (200, 201):
            a = unwrap_asset(r_hi.body)
            assert 5.0 <= float(a["manifest"]["duration_sec"]) <= 120.0

    @reg.test("CS-T2-F03-02", 2, "CS-F03", "Zero-draw invisible script rejected by 5-frame dry-run (422 / valid=False)")
    def t2_f03_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f03")
        src, man = env.ensure_corpus(u).hostile_zero_draw_script()
        rv = env.validate_script(u, src, man)
        rp = env.publish_asset(u, "ZeroDraw", "Invisible", "animation", ["bad"], src, man)
        _assert_rejected_validation(env, rv, rp, "zero_draw")

    @reg.test("CS-T2-F03-03", 2, "CS-F03", "Manifest min (160x160@15fps) & max (720x1280@60fps) boundaries succeed")
    def t2_f03_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f03")
        src, base = env.ensure_corpus(u).valid_script("animation")
        for w, h, fps in ((160, 160, 15), (720, 1280, 60)):
            m = dict(base, width=w, height=h, fps=fps)
            rp = env.publish_asset(u, f"Bounds {w}x{h}", "Valid edge", "animation", ["edge"], src, m)
            assert rp.status in (200, 201), f"Valid edge {(w,h,fps)} failed: {rp.status} {rp.body!r}"
            assert_valid_manifest(unwrap_asset(rp.body)["manifest"], expected_kind="animation")

    @reg.test("CS-T2-F03-04", 2, "CS-F03", "Mismatched asset.kind vs manifest.kind rejected 400/422")
    def t2_f03_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f03")
        src, man = env.ensure_corpus(u).valid_script("game")
        r = env.publish_asset(u, "Mismatch", "Wrong kind", "video", ["bad"], src, man)
        assert r.status in (400, 422), f"Kind mismatch returned {r.status}"

    @reg.test("CS-T2-F03-05", 2, "CS-F03", "Exact 65_536-byte boundary valid cs1 script accepted cleanly")
    def t2_f03_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f03")
        src, man = env.ensure_corpus(u).exact_64k_script("animation")
        ok, ch, errs = env.extract_validation(env.validate_script(u, src, man))
        assert ok and ch and not errs, f"Exact 65_536B valid script rejected: {errs}"
        rp = env.publish_asset(u, "Exact 64KB", "Boundary valid", "animation", ["64k"], src, man)
        assert rp.status in (200, 201), f"Exact 65_536B publish failed: {rp.status}"

    # ===================== CS-F04: Seed Catalog Boundaries (5) =====================
    @reg.test("CS-T2-F04-01", 2, "CS-F04", "GET /assets/nonexistent-seed-id-999 returns strictly 404")
    def t2_f04_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f04")
        r, _ = env.get_asset(u, "nonexistent-seed-id-999")
        assert r.status == 404, f"Expected 404, got {r.status}"

    @reg.test("CS-T2-F04-02", 2, "CS-F04", "Nonexistent tag_filter returns 200 with empty items & next_cursor=None")
    def t2_f04_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f04")
        r = env.feed(u, tab="for_you", tag_filter="zzz_nonexistent_tag_999", limit=10)
        assert r.status == 200 and r.body["items"] == [] and r.body.get("next_cursor") is None

    @reg.test("CS-T2-F04-03", 2, "CS-F04", "Extreme feed limits (0, 500, -5) clamped or rejected cleanly (never 500)")
    def t2_f04_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f04")
        r0 = env.feed(u, tab="for_you", limit=0)
        assert r0.status in (200, 400, 422)
        r500 = env.feed(u, tab="for_you", limit=500)
        assert r500.status == 200 and len(r500.body["items"]) <= 100
        rneg = env.feed(u, tab="for_you", limit=-5)
        assert rneg.status in (200, 400, 422) and rneg.status != 500

    @reg.test("CS-T2-F04-04", 2, "CS-F04", "Seed catalog has 100% unique scene_ids and non-negative counters")
    def t2_f04_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f04")
        _, assets = env.list_assets(u)
        ids = [a["scene_id"] for a in assets]
        assert len(ids) == len(set(ids)) and len(ids) >= 18
        for a in assets[:18]:
            assert_valid_asset(a)

    @reg.test("CS-T2-F04-05", 2, "CS-F04", "Path-traversal & SQLi scene_id on GET /assets/{id} returns 400/404 (never 500)")
    def t2_f04_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f04")
        for bad_id in ("../etc/passwd", "%2e%2e%2fsecret", "' OR 1=1--"):
            r, _ = env.get_asset(u, bad_id)
            assert r.status in (400, 404), f"bad_id={bad_id!r} returned {r.status}"

    # ===================== CS-F05: Telemetry & Reward Boundaries (6) =====================
    @reg.test("CS-T2-F05-01", 2, "CS-F05", "Extreme dwell_ms (0, -500, 1e9) handled safely with finite reward")
    def t2_f05_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f05")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        for dms in (0, -500, 1_000_000_000):
            r = env.send_telemetry(u, env.make_event(u, sid, "dwell", dwell_ms=dms))
            assert r.status in (200, 400, 422)
            if r.status == 200:
                assert math.isfinite(float(r.body["reward"]))

    @reg.test("CS-T2-F05-02", 2, "CS-F05", "Extreme completion_ratio (-1.0, 0.0, 3.0, 999.0) yields bounded finite reward")
    def t2_f05_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f05")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        r3 = env.send_telemetry(u, env.make_event(u, sid, "complete", completion_ratio=3.0))
        r999 = env.send_telemetry(u, env.make_event(u, sid, "complete", completion_ratio=999.0))
        rneg = env.send_telemetry(u, env.make_event(u, sid, "complete", completion_ratio=-1.0))
        assert r3.status == 200 and r999.status in (200, 400, 422) and rneg.status in (200, 400, 422)
        if r999.status == 200:
            assert math.isfinite(float(r999.body["reward"])) and float(r999.body["reward"]) <= float(r3.body["reward"]) + 0.5

    @reg.test("CS-T2-F05-03", 2, "CS-F05", "Fast-skip thresholds preserve monotonic reward (500ms <= 2200ms < 12000ms)")
    def t2_f05_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f05")
        _, assets = env.list_assets(u)
        r500 = env.send_telemetry(u, env.make_event(u, assets[0]["scene_id"], "fast_skip", dwell_ms=500))
        r2200 = env.send_telemetry(u, env.make_event(u, assets[1]["scene_id"], "fast_skip", dwell_ms=2200))
        r12k = env.send_telemetry(u, env.make_event(u, assets[2]["scene_id"], "dwell", dwell_ms=12000, completion_ratio=0.9))
        assert float(r500.body["reward"]) <= float(r2200.body["reward"]) < float(r12k.body["reward"])

    @reg.test("CS-T2-F05-04", 2, "CS-F05", "Unknown event_kind ('hack_boost', '') rejected 400/422")
    def t2_f05_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f05")
        _, assets = env.list_assets(u)
        for bad in ("hack_boost", "", "LIKE"):
            r = env.send_telemetry(u, env.make_event(u, assets[0]["scene_id"], bad))
            assert r.status in (400, 422), f"Unknown kind={bad!r} returned {r.status}"

    @reg.test("CS-T2-F05-05", 2, "CS-F05", "Empty telemetry batch & nonexistent scene_id handled without crash")
    def t2_f05_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f05")
        re_empty = env.send_telemetry(u, [])
        assert re_empty.status in (200, 400)
        if re_empty.status == 200:
            assert int(re_empty.body["accepted"]) == 0
        re_miss = env.send_telemetry(u, env.make_event(u, "nonexistent_scene_xyz", "like"))
        assert re_miss.status in (200, 400, 404) and re_miss.status != 500

    @reg.test("CS-T2-F05-06", 2, "CS-F05", "20 rapid duplicate positive events keep ScoreBreakdown strictly finite")
    def t2_f05_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f05")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        batch = [env.make_event(u, sid, "double_tap_like") for _ in range(20)]
        assert env.send_telemetry(u, batch).status == 200
        rf = env.feed(u, tab="for_you", limit=10)
        for it in assert_valid_feed_response(rf.body):
            assert_valid_score_breakdown(it["explanation"]["breakdown"])

    # ===================== CS-F06: Recommender Boundaries (6) =====================
    @reg.test("CS-T2-F06-01", 2, "CS-F06", "Brand-new cold-start user receives diverse multi-kind ForYou with finite scores")
    def t2_f06_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f06_cold")
        r = env.feed(u, tab="for_you", limit=12)
        items = assert_valid_feed_response(r.body)
        assert len({it["asset"]["kind"] for it in items}) >= 2
        for it in items:
            assert_valid_score_breakdown(it["explanation"]["breakdown"])

    @reg.test("CS-T2-F06-02", 2, "CS-F06", "0% re-appearance across paginated refreshes after not_interested/hide_author/hide_tag")
    def t2_f06_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f06")
        items0 = assert_valid_feed_response(env.feed(u, tab="for_you", limit=18).body)
        assert len(items0) >= 6
        s_dis = items0[0]["asset"]["scene_id"]
        a_hid = items0[1]["asset"]["author_user_id"]
        # Pick a specific tag (prefer non-kind tag) so >= 6 scenes remain for multi-page walking
        t_hid = next(
            (t for t in items0[2]["asset"]["tags"] if t.lower() not in ("video", "animation", "game")),
            items0[2]["asset"]["tags"][0],
        )
        env.send_telemetry(u, [
            env.make_event(u, s_dis, "not_interested"),
            env.make_event(u, items0[1]["asset"]["scene_id"], "hide_author", author_user_id=a_hid),
            env.make_event(u, items0[2]["asset"]["scene_id"], "hide_tag", reason=t_hid),
        ])
        for refresh in range(2):
            walked, pages = env.feed_all_pages(u, tab="for_you", session_id=f"ref_{refresh}", limit=5)
            assert pages >= 2, f"Expected >= 2 paginated pages (cursor != None) on refresh {refresh}, got {pages}"
            assert len(walked) >= 6, f"Expected >= 6 remaining scenes across pages, got {len(walked)}"
            for it in walked:
                a = it["asset"]
                assert a["scene_id"] != s_dis, f"Dismissed scene {s_dis} leaked on refresh {refresh}"
                assert a["author_user_id"] != a_hid, f"Hidden author {a_hid} leaked on refresh {refresh}"
                assert t_hid not in a["tags"], f"Hidden tag {t_hid!r} leaked on refresh {refresh}"

    @reg.test("CS-T2-F06-03", 2, "CS-F06", "Hiding ALL authors on page 1 returns 200 with zero leaked hidden authors")
    def t2_f06_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f06")
        items0 = env.feed(u, tab="for_you", limit=10).body["items"]
        hidden = set()
        evs = []
        for it in items0:
            aid = it["asset"]["author_user_id"]
            if aid not in hidden:
                hidden.add(aid)
                evs.append(env.make_event(u, it["asset"]["scene_id"], "hide_author", author_user_id=aid))
        env.send_telemetry(u, evs)
        rf = env.feed(u, tab="for_you", limit=30)
        assert rf.status == 200
        for it in assert_valid_feed_response(rf.body):
            assert it["asset"]["author_user_id"] not in hidden

    @reg.test("CS-T2-F06-04", 2, "CS-F06", "Contradictory rapid feedback keeps scores finite & honors final not_interested")
    def t2_f06_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f06")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        env.send_telemetry(u, [
            env.make_event(u, sid, "like"), env.make_event(u, sid, "unlike"),
            env.make_event(u, sid, "fast_skip", dwell_ms=400),
            env.make_event(u, sid, "complete", completion_ratio=1.0),
            env.make_event(u, sid, "not_interested"),
        ])
        items, _ = env.feed_all_pages(u, tab="for_you", limit=20)
        assert len(items) >= 1, "Expected remaining non-dismissed items in ForYou"
        assert all(it["asset"]["scene_id"] != sid for it in items), f"Dismissed scene {sid} leaked after contradictory feedback"

    @reg.test("CS-T2-F06-05", 2, "CS-F06", "Full catalog exhaustion cursor walk (limit=5) ends cleanly with 0 duplicates")
    def t2_f06_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f06")
        seen: set[str] = set()
        cur = None
        pages = 0
        max_pages = 40
        total_catalog = 0
        while pages < max_pages:
            r = env.feed(u, tab="for_you", session_id="exhaust_5", cursor=cur, limit=5)
            assert r.status == 200, f"limit=5 page {pages} failed: {r.status} {r.body!r}"
            items = assert_valid_feed_response(r.body, expected_limit=5)
            if pages == 0:
                total_catalog = int(r.body.get("total_catalog") or 0)
                max_pages = max(40, (total_catalog // 5) + 15)
            for it in items:
                sid = it["asset"]["scene_id"]
                assert sid not in seen, f"Duplicate scene_id {sid!r} across limit=5 cursor pages (page={pages}, seen={len(seen)})"
                seen.add(sid)
            cur = r.body.get("next_cursor")
            pages += 1
            if cur is None:
                break
        assert cur is None and len(seen) >= 18, (
            f"limit=5 exhaustion failed: cur={cur!r}, seen={len(seen)}, pages={pages}, total_catalog={total_catalog}"
        )

    @reg.test("CS-T2-F06-06", 2, "CS-F06", "Malformed or out-of-range cursor returns 400 or empty page (never 500)")
    def t2_f06_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f06")
        for bad_cur in ("!!!bad-cursor!!!", "99999999", "cur_-5"):
            r = env.feed(u, tab="for_you", cursor=bad_cur, limit=5)
            assert r.status in (200, 400, 422), f"cursor={bad_cur!r} returned {r.status}"
            if r.status == 200:
                assert r.body["items"] == []

    # ===================== CS-F07: Feed Session & Tab Boundaries (5) =====================
    @reg.test("CS-T2-F07-01", 2, "CS-F07", "limit=1 pagination walk yields >=18 strictly distinct items until None")
    def t2_f07_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f07")
        seen: set[str] = set()
        cur = None
        pages = 0
        max_pages = 80
        total_catalog = 0
        while pages < max_pages:
            r = env.feed(u, tab="for_you", session_id="lim1", cursor=cur, limit=1)
            assert r.status == 200, f"limit=1 page {pages} failed: {r.status} {r.body!r}"
            items = assert_valid_feed_response(r.body, expected_limit=1)
            if pages == 0:
                total_catalog = int(r.body.get("total_catalog") or 0)
                max_pages = max(80, total_catalog + 20)
            for it in items:
                sid = it["asset"]["scene_id"]
                assert sid not in seen, f"Duplicate scene_id {sid!r} across limit=1 cursor pages (page={pages}, seen={len(seen)})"
                seen.add(sid)
            cur = r.body.get("next_cursor")
            pages += 1
            if cur is None:
                break
        assert cur is None and len(seen) >= 18, (
            f"limit=1 exhaustion failed: cur={cur!r}, seen={len(seen)}, pages={pages}, total_catalog={total_catalog}"
        )

    @reg.test("CS-T2-F07-02", 2, "CS-F07", "Empty session_id & 512-char session_id handled cleanly (never 500)")
    def t2_f07_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f07")
        for sid in ("", "s" * 512):
            r = env.feed(u, tab="for_you", session_id=sid, limit=5)
            assert r.status in (200, 400, 422) and r.status != 500
            if r.status == 200:
                assert_valid_feed_response(r.body, expected_limit=5)

    @reg.test("CS-T2-F07-03", 2, "CS-F07", "Conflicting query + tag_filter returns 200 with items=[] & next_cursor=None")
    def t2_f07_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f07")
        _, assets = env.list_assets(u, {"limit": 50})
        existing_tag = next((t for a in assets for t in (a.get("tags") or []) if isinstance(t, str) and t.strip()), "neon")
        r = env.feed(u, tab="for_you", query="ImpossibleQuery999888", tag_filter=existing_tag, limit=10)
        assert r.status == 200 and r.body["items"] == [] and r.body.get("next_cursor") is None

    @reg.test("CS-T2-F07-04", 2, "CS-F07", "CJK/emoji search query & whitespace-only query return valid 200")
    def t2_f07_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f07")
        for q in ("赛博🚀", "   \t  "):
            r = env.feed(u, tab="for_you", query=q, limit=10)
            assert r.status == 200
            assert_valid_feed_response(r.body)

    @reg.test("CS-T2-F07-05", 2, "CS-F07", "Mid-session tab switching never leaks wrong AssetKind into kind tabs")
    def t2_f07_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f07")
        sess = "sess_switch_mid"
        assert env.feed(u, tab="for_you", session_id=sess, limit=6).status == 200
        rg = env.feed(u, tab="games", session_id=sess, limit=15)
        g_items = assert_valid_feed_response(rg.body)
        assert len(g_items) >= 1 and all(it["asset"]["kind"] == "game" for it in g_items)
        rv = env.feed(u, tab="videos", session_id=sess, limit=15)
        v_items = assert_valid_feed_response(rv.body)
        assert len(v_items) >= 1 and all(it["asset"]["kind"] == "video" for it in v_items)
        assert env.feed(u, tab="for_you", session_id=sess, limit=6).status == 200

    # ===================== CS-F08: AI Synthesizer Boundaries (5) =====================
    @reg.test("CS-T2-F08-01", 2, "CS-F08", "Empty or whitespace-only prompt on ai/generate rejected 400/422")
    def t2_f08_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f08")
        for p in ("", "   \n\t "):
            r = env.ai_generate(u, p)
            assert r.status in (400, 422), f"Empty prompt {p!r} returned {r.status}"

    @reg.test("CS-T2-F08-02", 2, "CS-F08", "Ultra-long 16KB prompt safely bounded or rejected (never 500/timeout)")
    def t2_f08_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f08")
        r = env.ai_generate(u, "neon shader wave " * 1024, kind="animation")
        assert r.status in (200, 201, 400, 422) and r.status != 500
        if r.status in (200, 201):
            d = unwrap_draft(r.body)
            assert isinstance(d.get("source_code"), str) and d["source_code"].strip(), (
                f"200/201 ai/generate on 16KB prompt missing source_code: {r.body!r}"
            )
            assert_valid_manifest(d.get("manifest"))

    @reg.test("CS-T2-F08-03", 2, "CS-F08", "CJK + Emoji + punctuation prompt generates valid runnable cs1 draft")
    def t2_f08_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f08")
        r = env.ai_generate(u, "🎮 做个霓虹星际弹球！✨🔥 — 60fps")
        assert r.status in (200, 201)
        d = unwrap_draft(r.body)
        ok, ch, _ = env.extract_validation(env.validate_script(u, d["source_code"], d["manifest"]))
        assert ok and ch

    @reg.test("CS-T2-F08-04", 2, "CS-F08", "SQLi + XSS prompt keeps DB catalog 100% intact (>=18 assets)")
    def t2_f08_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f08")
        r = env.ai_generate(u, "Make game '; DROP TABLE code_scenes; -- <script>alert(1)</script>")
        assert r.status in (200, 201, 400, 422)
        assert env.feed(u, tab="for_you", limit=5).body["total_catalog"] >= 18

    @reg.test("CS-T2-F08-05", 2, "CS-F08", "POST /ai/iterate with nonexistent draft_id returns strictly 404")
    def t2_f08_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f08")
        r = env.ai_iterate(u, draft_id="draft-does-not-exist-999", prompt="Make it brighter")
        assert r.status == 404, f"Expected 404, got {r.status}"

    # ===================== CS-F09: Server SQLite V42 & Security Boundaries (6) =====================
    @reg.test("CS-T2-F09-01", 2, "CS-F09", "401 sweep across ALL /api/v1/code-scene/* endpoints (missing/forged/SQLi tokens)")
    def t2_f09_01(env: CodeSceneEnv) -> None:
        endpoints = [
            ("POST", "/api/v1/code-scene/feed", {"tab": "for_you", "limit": 5}),
            ("GET", "/api/v1/code-scene/assets", None),
            ("GET", "/api/v1/code-scene/assets/seed_vid_01", None),
            ("POST", "/api/v1/code-scene/assets", {}),
            ("POST", "/api/v1/code-scene/assets/validate", {}),
            ("POST", "/api/v1/code-scene/assets/seed_vid_01/like", {}),
            ("POST", "/api/v1/code-scene/assets/seed_vid_01/bookmark", {}),
            ("GET", "/api/v1/code-scene/assets/seed_vid_01/comments", None),
            ("POST", "/api/v1/code-scene/assets/seed_vid_01/comments", {"content": "hi"}),
            ("POST", "/api/v1/code-scene/creators/777000/follow", {}),
            ("POST", "/api/v1/code-scene/telemetry", {"events": []}),
            ("POST", "/api/v1/code-scene/ai/generate", {"prompt": "test"}),
            ("POST", "/api/v1/code-scene/ai/iterate", {"draft_id": "x", "prompt": "y"}),
            ("POST", "/api/v1/code-scene/ai/publish", {"draft_id": "x"}),
            ("POST", "/api/v1/code-scene/ai/remix", {"scene_id": "x", "prompt": "y"}),
        ]
        bad_tokens = [None, "", "forged.token.abc123", "' OR '1'='1"]
        c = env.client()
        for method, path, body in endpoints:
            for tok in bad_tokens:
                r = c.json(method, path, body, token=tok)
                assert r.status == 401, f"{method} {path} with tok={tok!r} returned {r.status} instead of 401"

    @reg.test("CS-T2-F09-02", 2, "CS-F09", "404 sweep across detail, like, bookmark, comments, follow & ai/remix")
    def t2_f09_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f09")
        miss = "nonexistent_scene_404_check"
        assert env.get_asset(u, miss)[0].status == 404
        assert env.toggle_like(u, miss).status == 404
        assert env.toggle_bookmark(u, miss).status == 404
        assert env.list_comments(u, miss)[0].status == 404
        assert env.post_comment(u, miss, "hello").status == 404
        assert env.toggle_follow(u, 999_999_999).status == 404
        assert env.ai_remix(u, miss, "make neon").status == 404

    @reg.test("CS-T2-F09-03", 2, "CS-F09", "SQLi payloads in title, description, tags, comment & query keep DB intact without row mutation")
    def t2_f09_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f09")
        corpus = env.ensure_corpus(u)
        canary_title = f"Canary Safe Scene {u.username}"
        src_c, man_c = corpus.mutated_valid_script("animation", variant=11)
        rc = env.publish_asset(u, canary_title, "Benign canary description", "animation", ["canary"], src_c, man_c)
        assert rc.status in (200, 201)
        canary_sid = unwrap_asset(rc.body)["scene_id"]
        before_total = env.feed(u, tab="for_you", limit=5).body["total_catalog"]
        src, man = corpus.mutated_valid_script("animation", variant=12)
        sqli = "'; DROP TABLE code_scenes; --"
        sqli_desc = "' OR 1=1 --"
        rp = env.publish_asset(u, sqli, sqli_desc, "animation", [sqli], src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        assert env.post_comment(u, sid, sqli).status in (200, 201)
        rf = env.feed(u, tab="for_you", query=sqli, limit=10)
        assert rf.status == 200 and rf.body["total_catalog"] >= before_total + 1
        rd_sqli, d_sqli = env.get_asset(u, sid)
        assert rd_sqli.status == 200 and d_sqli is not None
        assert d_sqli["title"] == sqli, f"SQLi title mutated: {d_sqli['title']!r}"
        assert d_sqli["description"] == sqli_desc, f"SQLi description mutated: {d_sqli['description']!r}"
        rl_c, cmts = env.list_comments(u, sid)
        assert rl_c.status == 200 and any(extract_comment_text(c) == sqli for c in cmts), (
            f"SQLi comment not preserved verbatim: {cmts!r}"
        )
        rd_can, d_can = env.get_asset(u, canary_sid)
        assert rd_can.status == 200 and d_can is not None
        assert d_can["title"] == canary_title, (
            f"Canary row mutated by SQLi payload: expected {canary_title!r}, got {d_can['title']!r}"
        )

    @reg.test("CS-T2-F09-04", 2, "CS-F09", "XSS payloads in title, description & comment serialize safely without crash")
    def t2_f09_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f09")
        src, man = env.ensure_corpus(u).mutated_valid_script("animation", variant=13)
        xss = "<script>alert(1)</script> ' onerror='alert(1)"
        rp = env.publish_asset(u, xss, xss, "animation", ["xss"], src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        assert env.post_comment(u, sid, xss).status in (200, 201)
        rd, d = env.get_asset(u, sid)
        assert rd.status == 200 and d is not None

    @reg.test("CS-T2-F09-05", 2, "CS-F09", "Empty/whitespace title & empty/whitespace comment rejected 400/422")
    def t2_f09_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f09")
        src, man = env.ensure_corpus(u).valid_script("animation")
        for bad_t in ("", "   \t\n"):
            assert env.publish_asset(u, bad_t, "Desc", "animation", ["x"], src, man).status in (400, 422)
        _, assets = env.list_assets(u)
        for bad_c in ("", "   "):
            assert env.post_comment(u, assets[0]["scene_id"], bad_c).status in (400, 422)

    @reg.test("CS-T2-F09-06", 2, "CS-F09", "Unicode/CJK/emoji title, description & tags round-trip losslessly")
    def t2_f09_06(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f09")
        src, man = env.ensure_corpus(u).mutated_valid_script("game", variant=14)
        title = "赛博弹球🎮✨"
        desc = "日本語と中文テスト🚀"
        tags = ["霓虹🎨", "游戏🎮", "日本語"]
        rp = env.publish_asset(u, title, desc, "game", tags, src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        _, d = env.get_asset(u, sid)
        assert d is not None and d["title"] == title and d["description"] == desc and d["tags"] == tags

    # ===================== CS-F10: AI Bot Pipeline Boundaries (5) =====================
    @reg.test("CS-T2-F10-01", 2, "CS-F10", "POST /ai/publish with nonexistent draft_id returns strictly 404")
    def t2_f10_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f10")
        assert env.ai_publish(u, draft_id="nonexistent_draft_xyz").status == 404

    @reg.test("CS-T2-F10-02", 2, "CS-F10", "POST /ai/publish overriding draft with hostile code fails closed (422, zero DB insert)")
    def t2_f10_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f10")
        d = unwrap_draft(env.ai_generate(u, "Make neon tunnel", kind="animation").body)
        before = env.feed(u, tab="for_you", limit=5).body["total_catalog"]
        bad_src, bad_man = env.ensure_corpus(u).hostile_infinite_loop_script()
        rp = env.ai_publish(u, draft_id=d["draft_id"], overrides={"source_code": bad_src, "manifest": bad_man})
        assert rp.status == 422, f"Expected 422, got {rp.status}"
        assert env.feed(u, tab="for_you", limit=5).body["total_catalog"] == before

    @reg.test("CS-T2-F10-03", 2, "CS-F10", "POST /ai/publish with nonexistent or outsider chat_id rejected 403/404 with zero leak")
    def t2_f10_03(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t2f10a"), env.register_user("t2f10b")
        b_chat = env.create_chat(ub, title="Bob Private Room")
        d = unwrap_draft(env.ai_generate(ua, "Neon wave", kind="animation").body)
        assert env.ai_publish(ua, draft_id=d["draft_id"], chat_id=999_999_999).status in (400, 404)
        r_out = env.ai_publish(ua, draft_id=d["draft_id"], chat_id=b_chat)
        assert r_out.status in (403, 404)
        assert env.list_chat_messages(ub, b_chat) == []

    @reg.test("CS-T2-F10-04", 2, "CS-F10", "POST /ai/remix with nonexistent scene_id returns strictly 404")
    def t2_f10_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f10")
        assert env.ai_remix(u, "nonexistent_parent_scene", "Add rainbows").status == 404

    @reg.test("CS-T2-F10-05", 2, "CS-F10", "Double-publishing same draft_id is safe (409 or idempotent, never 500)")
    def t2_f10_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f10")
        d = unwrap_draft(env.ai_generate(u, "Solar flare animation", kind="animation").body)
        r1 = env.ai_publish(u, draft_id=d["draft_id"])
        assert r1.status in (200, 201)
        r2 = env.ai_publish(u, draft_id=d["draft_id"])
        assert r2.status in (200, 201, 409) and r2.status != 500

    # ===================== CS-F11: Client Sync & Account Isolation Boundaries (5) =====================
    @reg.test("CS-T2-F11-01", 2, "CS-F11", "Cross-user draft hijack: User B cannot iterate or publish User A's draft (403/404)")
    def t2_f11_01(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t2f11a"), env.register_user("t2f11b")
        d = unwrap_draft(env.ai_generate(ua, "Alice secret shader", kind="animation").body)
        assert env.ai_iterate(ub, draft_id=d["draft_id"], prompt="Hijack").status in (403, 404)
        assert env.ai_publish(ub, draft_id=d["draft_id"]).status in (403, 404)

    @reg.test("CS-T2-F11-02", 2, "CS-F11", "Revoked session token & forged token rejected 401 while valid User B token succeeds 200")
    def t2_f11_02(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t2f11a"), env.register_user("t2f11b")
        assert ua.session_id, f"Missing session_id from register response for {ua.username}"
        r_rev = env.revoke_session(ua)
        assert r_rev.status in (200, 204), f"DELETE /api/v1/auth/sessions/{ua.session_id} failed: {r_rev.status} {r_rev.body!r}"
        r_old_assets = env.client().json("GET", "/api/v1/code-scene/assets", token=ua.token)
        assert r_old_assets.status == 401, f"Revoked User A token was NOT rejected on /assets: {r_old_assets.status}"
        r_old_feed = env.feed(ua, tab="for_you", limit=5)
        assert r_old_feed.status == 401, f"Revoked User A token was NOT rejected on /feed: {r_old_feed.status}"
        r_forged = env.client().json("GET", "/api/v1/code-scene/assets", token="forged.fake.token")
        assert r_forged.status == 401, f"Forged token returned {r_forged.status} instead of 401"
        assert env.feed(ub, tab="for_you", limit=5).status == 200
        assert env.client().json("GET", "/api/v1/code-scene/assets", token=ub.token).status == 200

    @reg.test("CS-T2-F11-03", 2, "CS-F11", "Cross-user chat card injection blocked (403) & victim transcript stays clean")
    def t2_f11_03(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t2f11a"), env.register_user("t2f11b")
        cid = env.create_chat(ub, title="B Only")
        card = {"type": "GenerativeCard", "data": {"card_id": "c1", "title": "Spam", "layout": "VerticalStack", "components": [], "status": "ok"}}
        r = env.client().json("POST", f"/api/v1/chats/{cid}/messages", {"content": card}, token=ua.token)
        assert r.status in (403, 404)
        assert env.list_chat_messages(ub, cid) == []

    @reg.test("CS-T2-F11-04", 2, "CS-F11", "Duplicate/out-of-order offline telemetry outbox replay stays finite & stable")
    def t2_f11_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f11")
        _, assets = env.list_assets(u)
        ev = env.make_event(u, assets[0]["scene_id"], "complete", dwell_ms=10000, completion_ratio=1.0, timestamp_ms=1_700_000_000_000)
        assert env.send_telemetry(u, [ev, ev, ev]).status == 200
        rf = env.feed(u, tab="for_you", limit=8)
        for it in assert_valid_feed_response(rf.body):
            assert_valid_score_breakdown(it["explanation"]["breakdown"])

    @reg.test("CS-T2-F11-05", 2, "CS-F11", "Fresh User B registered after User A's 30 game events gets clean cold-start diversity")
    def t2_f11_05(env: CodeSceneEnv) -> None:
        ua = env.register_user("t2f11_heavy")
        _, games = env.list_assets(ua, {"kind": "game"})
        batch = [
            env.make_event(ua, games[i % len(games)]["scene_id"], "game_interaction", game_play_ms=30000, game_inputs=50)
            for i in range(30)
        ]
        assert env.send_telemetry(ua, batch).status == 200
        ub = env.register_user("t2f11_fresh")
        rb = env.feed(ub, tab="for_you", limit=12)
        items = assert_valid_feed_response(rb.body)
        assert len({it["asset"]["kind"] for it in items}) >= 2

    # ===================== CS-F12: Viewport & Game Focus Boundaries (5) =====================
    @reg.test("CS-T2-F12-01", 2, "CS-F12", "Out-of-bounds manifest width/height (50, 2000, 4000) clamped to spec or 422")
    def t2_f12_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f12")
        src, base = env.ensure_corpus(u).valid_script("animation")
        for w, h in ((50, 50), (2000, 4000)):
            m = dict(base, width=w, height=h)
            r = env.publish_asset(u, f"OOB {w}x{h}", "Bounds", "animation", ["oob"], src, m)
            assert r.status in (200, 201, 400, 422)
            if r.status in (200, 201):
                assert_valid_manifest(unwrap_asset(r.body)["manifest"])

    @reg.test("CS-T2-F12-02", 2, "CS-F12", "Out-of-bounds fps (0, 10, 240) strictly clamped to [15, 60] or rejected 422")
    def t2_f12_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f12")
        src, base = env.ensure_corpus(u).valid_script("animation")
        for fps in (0, 10, 240):
            m = dict(base, fps=fps)
            r = env.publish_asset(u, f"FPS {fps}", "Bounds", "animation", ["fps"], src, m)
            assert r.status in (200, 201, 400, 422)
            if r.status in (200, 201):
                assert 15 <= unwrap_asset(r.body)["manifest"]["fps"] <= 60

    @reg.test("CS-T2-F12-03", 2, "CS-F12", "Pixel area overflow (>76_800px) & draw flood (>2_048 draws) rejected or strictly capped")
    def t2_f12_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f12")
        corpus = env.ensure_corpus(u)
        psrc, pman = corpus.hostile_pixel_overflow_script()
        _assert_rejected_validation(env, env.validate_script(u, psrc, pman), env.publish_asset(u, "PixOOB", "Too big", "animation", ["bad"], psrc, pman), "pixel_overflow")
        dsrc, dman = corpus.hostile_draw_flood_script()
        rv = env.validate_script(u, dsrc, dman)
        assert rv.status in (200, 422), f"Draw flood returned unexpected status {rv.status}"
        ok, _, _ = env.extract_validation(rv)
        if ok:
            draw_cnt = _extract_explicit_draw_count(rv.body)
            assert draw_cnt is not None and 0 < draw_cnt <= 2_048, (
                f"Draw flood accepted (ok=True) without explicit bounded draw count (0 < count <= 2048): {rv.body!r}"
            )

    @reg.test("CS-T2-F12-04", 2, "CS-F12", "Zero-input game_interaction (0,0,0,0) processed without NaN or 500")
    def t2_f12_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f12")
        _, games = env.list_assets(u, {"kind": "game"})
        r = env.send_telemetry(
            u,
            env.make_event(
                u, games[0]["scene_id"], "game_interaction",
                game_inputs=0, game_play_ms=0, game_restarts=0, game_score=0.0,
            ),
        )
        assert r.status == 200 and math.isfinite(float(r.body["reward"]))

    @reg.test("CS-T2-F12-05", 2, "CS-F12", "Negative/extreme game_interaction fields clamped/rejected cleanly (no NaN/500)")
    def t2_f12_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f12")
        _, games = env.list_assets(u, {"kind": "game"})
        r = env.send_telemetry(
            u,
            env.make_event(
                u, games[0]["scene_id"], "game_interaction",
                game_inputs=-5, game_play_ms=-100, game_restarts=-1, game_score=1_000_000_000.0,
            ),
        )
        assert r.status in (200, 400, 422) and r.status != 500
        if r.status == 200:
            assert math.isfinite(float(r.body["reward"]))

    # ===================== CS-F13: Feed, HUD & Toggle Idempotency Boundaries (5) =====================
    @reg.test("CS-T2-F13-01", 2, "CS-F13", "Rapid 4x like toggle returns to exact initial likes_count & viewer_liked=False")
    def t2_f13_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f13")
        _, assets = env.list_assets(u)
        sid = assets[0]["scene_id"]
        n0 = assets[0]["likes_count"]
        expected = [True, False, True, False]
        for exp_liked in expected:
            r = env.toggle_like(u, sid)
            assert r.status == 200 and r.body["liked"] is exp_liked
            assert r.body["likes_count"] == (n0 + 1 if exp_liked else n0)
        _, d = env.get_asset(u, sid)
        assert d is not None and d["likes_count"] == n0 and d["viewer_liked"] is False

    @reg.test("CS-T2-F13-02", 2, "CS-F13", "Rapid 4x bookmark toggle returns to exact initial bookmarks_count >= 0")
    def t2_f13_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f13")
        _, assets = env.list_assets(u)
        sid = assets[1]["scene_id"]
        n0 = assets[1]["bookmarks_count"]
        for exp_bmk in (True, False, True, False):
            r = env.toggle_bookmark(u, sid)
            assert r.status == 200 and r.body["bookmarked"] is exp_bmk
            assert r.body["bookmarks_count"] == (n0 + 1 if exp_bmk else n0)
        _, d = env.get_asset(u, sid)
        assert d is not None and d["bookmarks_count"] == n0 and d["viewer_bookmarked"] is False

    @reg.test("CS-T2-F13-03", 2, "CS-F13", "Self-follow & rapid 4x creator follow/unfollow leave clean state (no 500)")
    def t2_f13_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f13")
        r_self = env.toggle_follow(u, u.user_id)
        assert r_self.status in (200, 400, 422) and r_self.status != 500
        _, assets = env.list_assets(u)
        cid = assets[0]["author_user_id"]
        for exp in (True, False, True, False):
            r = env.toggle_follow(u, cid)
            assert r.status == 200 and extract_follow_flag(r.body) is exp
        _, d = env.get_asset(u, assets[0]["scene_id"])
        assert d is not None and d["viewer_following_author"] is False

    @reg.test("CS-T2-F13-04", 2, "CS-F13", "following tab for user following 0 creators returns 200 items=[] next_cursor=None")
    def t2_f13_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f13_zero")
        r = env.feed(u, tab="following", limit=10)
        assert r.status == 200 and r.body["items"] == [] and r.body.get("next_cursor") is None

    @reg.test("CS-T2-F13-05", 2, "CS-F13", "my_works tab for user with 0 published scenes returns 200 items=[] next_cursor=None")
    def t2_f13_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f13_zero")
        r = env.feed(u, tab="my_works", limit=10)
        assert r.status == 200 and r.body["items"] == [] and r.body.get("next_cursor") is None

    # ===================== CS-F14: Slide-Over Drawers Boundaries (5) =====================
    @reg.test("CS-T2-F14-01", 2, "CS-F14", "Live Code Editor syntax errors return structured valid=False diagnostic (never 500)")
    def t2_f14_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f14")
        corpus = env.ensure_corpus(u)
        _, man = corpus.valid_script("animation")
        for bad_src in corpus.hostile_syntax_scripts():
            rv = env.validate_script(u, bad_src, man)
            ok, _, errs = env.extract_validation(rv)
            assert (rv.status == 422) or (rv.status == 200 and not ok and len(errs) >= 1)

    @reg.test("CS-T2-F14-02", 2, "CS-F14", "Comments drawer empty/whitespace comment rejected 400/422; count unchanged")
    def t2_f14_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f14")
        _, assets = env.list_assets(u)
        sid = assets[2]["scene_id"]
        c0 = assets[2]["comments_count"]
        for bad in ("", "   \n\t "):
            assert env.post_comment(u, sid, bad).status in (400, 422)
        _, d = env.get_asset(u, sid)
        assert d is not None and d["comments_count"] == c0

    @reg.test("CS-T2-F14-03", 2, "CS-F14", "Oversized (>4KB) & hostile CJK/RTL/XSS/SQLi comments bounded cleanly")
    def t2_f14_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f14")
        _, assets = env.list_assets(u)
        sid = assets[3]["scene_id"]
        r_big = env.post_comment(u, sid, "A" * 5000)
        assert r_big.status in (400, 422), f"Oversized 5KB comment returned {r_big.status}"
        tricky = "好酷🔥 \u202e <script>alert(1)</script> ' OR 1=1--"
        r_ok = env.post_comment(u, sid, tricky)
        assert r_ok.status in (200, 201)
        assert env.feed(u, tab="for_you", limit=5).body["total_catalog"] >= 18

    @reg.test("CS-T2-F14-04", 2, "CS-F14", "Creator Sheet follow against nonexistent creator_id returns 404")
    def t2_f14_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f14")
        assert env.toggle_follow(u, 999_999_999).status == 404

    @reg.test("CS-T2-F14-05", 2, "CS-F14", "ai/iterate with empty instruction rejected 400/422 while original draft remains publishable")
    def t2_f14_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f14")
        d = unwrap_draft(env.ai_generate(u, "Neon grid animation", kind="animation").body)
        assert env.ai_iterate(u, draft_id=d["draft_id"], prompt="").status in (400, 422)
        rp = env.ai_publish(u, draft_id=d["draft_id"])
        assert rp.status in (200, 201) and unwrap_asset(rp.body) is not None

    # ===================== CS-F15: Sidebar, AppShell & Chat Interop Boundaries (5) =====================
    @reg.test("CS-T2-F15-01", 2, "CS-F15", "GenerativeCard XSS title sanitization preserves valid code_scene:* buttons")
    def t2_f15_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f15")
        cid = env.create_chat(u, title="XSS Card Chat")
        d = unwrap_draft(env.ai_generate(u, "Plasma field", kind="animation").body)
        rp = env.ai_publish(u, draft_id=d["draft_id"], chat_id=cid, overrides={"title": "Neon <script>alert(1)</script> 'test'"})
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        msgs = env.list_chat_messages(u, cid)
        assert len(msgs) >= 1 and assert_valid_generative_card(msgs[-1], sid)
        card_title = msgs[-1]["content"]["data"]["title"]
        assert "<script>" not in card_title, f"Unescaped <script> tag in GenerativeCard title: {card_title!r}"

    @reg.test("CS-T2-F15-02", 2, "CS-F15", "Deep-link code_scene:open:<nonexistent> resolves cleanly to 404")
    def t2_f15_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f15")
        r, _ = env.get_asset(u, "nonexistent_deep_link_scene_id")
        assert r.status == 404

    @reg.test("CS-T2-F15-03", 2, "CS-F15", "ai/publish with invalid chat_id (0 or -42) rejected cleanly without DB corruption")
    def t2_f15_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f15")
        d = unwrap_draft(env.ai_generate(u, "Orbital animation", kind="animation").body)
        for bad_cid in (0, -42):
            r = env.ai_publish(u, draft_id=d["draft_id"], chat_id=bad_cid)
            assert r.status in (400, 404, 422) and r.status != 500

    @reg.test("CS-T2-F15-04", 2, "CS-F15", "ai/publish with chat_id=None succeeds without posting spurious chat messages")
    def t2_f15_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t2f15")
        cid = env.create_chat(u, title="Quiet Room")
        d = unwrap_draft(env.ai_generate(u, "Quiet starfield", kind="animation").body)
        rp = env.ai_publish(u, draft_id=d["draft_id"], chat_id=None)
        assert rp.status in (200, 201)
        assert env.list_chat_messages(u, cid) == []

    @reg.test("CS-T2-F15-05", 2, "CS-F15", "Zero Telegram regression: /health, /status, /auth/register & /chats intact")
    def t2_f15_05(env: CodeSceneEnv) -> None:
        c = env.client()
        assert c.request("GET", "/health").status == 200
        rs = c.json("GET", "/api/v1/status")
        assert rs.status == 200 and rs.body["status"] == "healthy"
        u = env.register_user("t2f15_reg")
        rc = c.json("GET", "/api/v1/chats", token=u.token)
        assert rc.status == 200 and rc.body["total"] == 0
        cb_cid = env.create_chat(u, title="Callback Check")
        rm = c.json("POST", f"/api/v1/chats/{cb_cid}/messages", {"text": "hello"}, token=u.token)
        assert rm.status in (200, 201)
        rcb_bad = c.json("POST", f"/api/v1/chats/{cb_cid}/callback", {"message_id": rm.body["message_id"], "callback_data": ""}, token=u.token)
        assert rcb_bad.status == 400
