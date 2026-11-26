"""Tier 3: Cross-Feature Pairwise Interaction Tests (20 tests: CS-T3-01..CS-T3-20).

Each test exercises >= 2 interacting subsystems across real HTTP state transitions.
NEVER imports _contract_double.py. Pure Python 3 stdlib only.
"""

from __future__ import annotations

from .harness import (
    CodeSceneEnv,
    TestRegistry,
    assert_valid_explanation,
    assert_valid_feed_response,
    assert_valid_generative_card,
    extract_follow_flag,
    unwrap_asset,
    unwrap_draft,
)


def register_tier3_tests(reg: TestRegistry) -> None:
    @reg.test("CS-T3-01", 3, "CS-F08+CS-F10+CS-F06", "AI Bot -> Validator -> Publish -> Cold-Start & Creator ForYou discovery")
    def t3_01(env: CodeSceneEnv) -> None:
        creator, viewer = env.register_user("t3_01c"), env.register_user("t3_01v")
        d = unwrap_draft(env.ai_generate(creator, "Neon hyper-cube animation", kind="animation").body)
        ok, ch, errs = env.extract_validation(env.validate_script(creator, d["source_code"], d["manifest"]))
        assert ok and ch and not errs
        rp = env.ai_publish(creator, draft_id=d["draft_id"])
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        c_ids = [it["asset"]["scene_id"] for it in assert_valid_feed_response(env.feed(creator, tab="for_you", limit=10).body)]
        assert sid in c_ids, f"Creator ForYou missing just-published {sid}"
        v_ids = [it["asset"]["scene_id"] for it in assert_valid_feed_response(env.feed(viewer, tab="for_you", limit=10).body)]
        assert sid in v_ids, f"Fresh viewer ForYou top-10 missing cold-start scene {sid}"

    @reg.test("CS-T3-02", 3, "CS-F05+CS-F06+CS-F07", "Fast-Skip x4 + Deep Game Play x3 pivots ForYou to Games within <= 2 refreshes")
    def t3_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_02")
        _, vids = env.list_assets(u, {"kind": "video"})
        _, anis = env.list_assets(u, {"kind": "animation"})
        _, games = env.list_assets(u, {"kind": "game"})
        for non_g in (vids[0], vids[1], anis[0], anis[1]):
            env.send_telemetry(u, env.make_event(u, non_g["scene_id"], "fast_skip", dwell_ms=400))
        for g in games[:3]:
            env.send_telemetry(u, [
                env.make_event(u, g["scene_id"], "game_interaction", game_play_ms=30000, game_inputs=50, game_score=240.0),
                env.make_event(u, g["scene_id"], "double_tap_like"),
            ])
        pivoted = False
        for ref in range(2):
            items = assert_valid_feed_response(env.feed(u, tab="for_you", session_id=f"piv_{ref}", limit=10).body)
            top = items[:6]
            g_cnt = sum(1 for it in top if it["asset"]["kind"] == "game")
            v_cnt = sum(1 for it in top if it["asset"]["kind"] == "video")
            if g_cnt >= 3 and g_cnt > v_cnt:
                pivoted = True
                break
        assert pivoted, "ForYou failed to pivot to Games within <= 2 refreshes"

    @reg.test("CS-T3-03", 3, "CS-F06+CS-F07+CS-F13", "ForYou scatter diversity vs strict kind-tab purity on same catalog")
    def t3_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_03")
        fy = assert_valid_feed_response(env.feed(u, tab="for_you", limit=18).body, expected_limit=18)
        assert len({it["asset"]["kind"] for it in fy}) >= 2, f"ForYou lacked multi-kind diversity: {[it['asset']['kind'] for it in fy]}"
        authors = [it["asset"]["author_user_id"] for it in fy]
        if len(set(authors)) >= 3:
            for i in range(len(authors) - 2):
                assert not (authors[i] == authors[i + 1] == authors[i + 2]), f"Scatter violated (3 consecutive identical authors): {authors}"
        for tab, kind in (("videos", "video"), ("animations", "animation"), ("games", "game")):
            titems = assert_valid_feed_response(env.feed(u, tab=tab, limit=20).body)
            assert len(titems) >= 6 and all(it["asset"]["kind"] == kind for it in titems)

    @reg.test("CS-T3-04", 3, "CS-F08+CS-F10+CS-F09", "AI Remix lineage (remix_parent_id) + child validation + parent remixes_count + 1")
    def t3_04(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_04")
        _, assets = env.list_assets(u)
        p = assets[0]
        rr = env.ai_remix(u, p["scene_id"], "Make holographic prism theme")
        assert rr.status in (200, 201)
        child = unwrap_asset(rr.body)
        assert child is not None and child["remix_parent_id"] == p["scene_id"]
        ok, ch, _ = env.extract_validation(env.validate_script(u, child["source_code"], child["manifest"]))
        assert ok and ch
        _, p_after = env.get_asset(u, p["scene_id"])
        assert p_after is not None and p_after["remixes_count"] == p["remixes_count"] + 1

    @reg.test("CS-T3-05", 3, "CS-F09+CS-F06+CS-F13", "Follow creator -> Following tab inclusion & ForYou creator_affinity boost")
    def t3_05(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_05")
        _, assets = env.list_assets(u)
        cid = assets[0]["author_user_id"]
        r0 = env.feed(u, tab="for_you", limit=30).body["items"]
        base = [it["explanation"]["breakdown"]["creator_affinity"] for it in r0 if it["asset"]["author_user_id"] == cid]
        assert extract_follow_flag(env.toggle_follow(u, cid).body) is True
        fol_items = assert_valid_feed_response(env.feed(u, tab="following", limit=30).body)
        assert len(fol_items) >= 1 and all(it["asset"]["author_user_id"] == cid for it in fol_items)
        r1 = env.feed(u, tab="for_you", limit=30).body["items"]
        boosted = [it["explanation"]["breakdown"]["creator_affinity"] for it in r1 if it["asset"]["author_user_id"] == cid]
        assert boosted and max(boosted) > (max(base) if base else 0.0) and max(boosted) > 0.0

    @reg.test("CS-T3-06", 3, "CS-F10+CS-F15", "AI Publish to group chat emits GenerativeCard with open/remix/share actions")
    def t3_06(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t3_06a"), env.register_user("t3_06b")
        cid = env.create_chat(ua, title="Code Scene Launch Group", member_ids=[ub.user_id])
        d = unwrap_draft(env.ai_generate(ua, "Neon starlight game", kind="game").body)
        rp = env.ai_publish(ua, draft_id=d["draft_id"], chat_id=cid)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        msgs = env.list_chat_messages(ub, cid)
        assert len(msgs) >= 1
        assert_valid_generative_card(msgs[-1], sid)

    @reg.test("CS-T3-07", 3, "CS-F09+CS-F06+CS-F05", "SQLite V42 durability across restart: AI scene, social state & recommender profile")
    def t3_07(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_07_rst")
        d = unwrap_draft(env.ai_generate(u, "Durable arcade game", kind="game").body)
        sid = unwrap_asset(env.ai_publish(u, draft_id=d["draft_id"]).body)["scene_id"]
        _, games = env.list_assets(u, {"kind": "game"})
        _, vids = env.list_assets(u, {"kind": "video"})
        other_author = next(a["author_user_id"] for a in games if a["author_user_id"] != u.user_id)
        env.toggle_like(u, sid)
        env.toggle_bookmark(u, sid)
        env.post_comment(u, sid, "Durable T3 comment")
        env.toggle_follow(u, other_author)
        for v in vids[:3]:
            env.send_telemetry(u, env.make_event(u, v["scene_id"], "fast_skip", dwell_ms=400))
        for g in games[:3]:
            env.send_telemetry(u, env.make_event(u, g["scene_id"], "game_interaction", game_play_ms=30000, game_inputs=60, game_score=200.0))
        env.restart_server()
        rd, detail = env.get_asset(u, sid)
        assert rd.status == 200 and detail is not None and detail["viewer_liked"] and detail["viewer_bookmarked"]
        assert detail["comments_count"] >= 1
        items, _ = env.feed_all_pages(u, tab="for_you", limit=20)
        g_top = sum(1 for it in items[:10] if it["asset"]["kind"] == "game")
        v_top = sum(1 for it in items[:10] if it["asset"]["kind"] == "video")
        g_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in items if it["asset"]["kind"] == "game"]
        v_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in items if it["asset"]["kind"] == "video"]
        assert g_top >= v_top and g_aff and v_aff and (sum(g_aff) / len(g_aff)) > (sum(v_aff) / len(v_aff))

    @reg.test("CS-T3-08", 3, "CS-F11+CS-F06+CS-F09", "Multi-user account isolation across recommender hides, likes & bookmarks")
    def t3_08(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t3_08a"), env.register_user("t3_08b")
        _, assets = env.list_assets(ua)
        s_dis, s_auth, s_like = assets[0], assets[1], assets[2]
        env.toggle_like(ua, s_like["scene_id"])
        env.toggle_bookmark(ua, s_like["scene_id"])
        env.send_telemetry(ua, [
            env.make_event(ua, s_dis["scene_id"], "not_interested"),
            env.make_event(ua, s_auth["scene_id"], "hide_author", author_user_id=s_auth["author_user_id"]),
        ])
        b_items, _ = env.feed_all_pages(ub, tab="for_you", limit=20)
        b_ids = {it["asset"]["scene_id"] for it in b_items}
        assert s_dis["scene_id"] in b_ids and s_auth["scene_id"] in b_ids
        _, d_like = env.get_asset(ub, s_like["scene_id"])
        assert d_like is not None and d_like["viewer_liked"] is False and d_like["viewer_bookmarked"] is False

    @reg.test("CS-T3-09", 3, "CS-F02+CS-F14+CS-F09", "Live Code Editor mutate -> validate new cover_hash -> publish -> my_works")
    def t3_09(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_09")
        corpus = env.ensure_corpus(u)
        s0, m0 = corpus.valid_script("animation")
        ok0, h0, _ = env.extract_validation(env.validate_script(u, s0, m0))
        s1, m1 = corpus.mutated_valid_script("animation", variant=15)
        ok1, h1, _ = env.extract_validation(env.validate_script(u, s1, m1))
        assert ok0 and ok1 and h0 != h1
        rp = env.publish_asset(u, "Edited Code Scene", "From editor", "animation", ["editor"], s1, m1)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        assert any(it["asset"]["scene_id"] == sid for it in env.feed(u, tab="my_works", limit=20).body["items"])
        assert env.get_asset(u, sid)[0].status == 200

    @reg.test("CS-T3-10", 3, "CS-F05+CS-F13+CS-F06", "Positive engagement stack vs fast_skip yields strictly higher affinity & score")
    def t3_10(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_10")
        _, games = env.list_assets(u, {"kind": "game"})
        _, vids = env.list_assets(u, {"kind": "video"})
        r_pos = env.send_telemetry(u, [
            env.make_event(u, games[0]["scene_id"], "dwell", dwell_ms=15000, completion_ratio=1.0),
            env.make_event(u, games[0]["scene_id"], "double_tap_like"),
            env.make_event(u, games[0]["scene_id"], "bookmark"),
        ])
        r_neg = env.send_telemetry(u, env.make_event(u, vids[0]["scene_id"], "fast_skip", dwell_ms=500))
        assert float(r_pos.body["reward"]) > 0.0 > float(r_neg.body["reward"])
        items, _ = env.feed_all_pages(u, tab="for_you", limit=20)
        g_scores = [it["explanation"]["breakdown"]["final_score"] for it in items if it["asset"]["scene_id"] == games[0]["scene_id"]]
        v_scores = [it["explanation"]["breakdown"]["final_score"] for it in items if it["asset"]["scene_id"] == vids[0]["scene_id"]]
        assert g_scores and v_scores and g_scores[0] > v_scores[0]

    @reg.test("CS-T3-11", 3, "CS-F08+CS-F10+CS-F02", "AI multi-turn iterate (v1->v2->v3) -> publish commits latest v3 validated code")
    def t3_11(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_11")
        d1 = unwrap_draft(env.ai_generate(u, "Make wave shader", kind="animation").body)
        draft_id = d1["draft_id"]
        d2 = unwrap_draft(env.ai_iterate(u, draft_id=draft_id, prompt="Add golden pulse rings").body)
        draft_id = d2.get("draft_id") or draft_id
        d3 = unwrap_draft(env.ai_iterate(u, draft_id=draft_id, prompt="Double orbital speed and cyan glow").body)
        draft_id = d3.get("draft_id") or draft_id
        assert d3["source_code"] != d1["source_code"] and d3["source_code"] != d2["source_code"]
        rp = env.ai_publish(u, draft_id=draft_id)
        assert rp.status in (200, 201)
        a = unwrap_asset(rp.body)
        assert a["source_code"] == d3["source_code"]
        ok, ch, _ = env.extract_validation(env.validate_script(u, a["source_code"], a["manifest"]))
        assert ok and ch

    @reg.test("CS-T3-12", 3, "CS-F14+CS-F09+CS-F13", "Comments + Likes + Bookmarks cross-endpoint count & flag consistency")
    def t3_12(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_12")
        _, assets = env.list_assets(u)
        sid = assets[9]["scene_id"]
        env.toggle_like(u, sid)
        env.toggle_bookmark(u, sid)
        env.post_comment(u, sid, "First T3 comment")
        env.post_comment(u, sid, "Second T3 comment")
        _, detail = env.get_asset(u, sid)
        _, cmts = env.list_comments(u, sid)
        feed_items, _ = env.feed_all_pages(u, tab="for_you", limit=20)
        from_feed = next(it["asset"] for it in feed_items if it["asset"]["scene_id"] == sid)
        assert detail["comments_count"] == len(cmts) == from_feed["comments_count"]
        for k in ("likes_count", "bookmarks_count", "viewer_liked", "viewer_bookmarked"):
            assert detail[k] == from_feed[k]

    @reg.test("CS-T3-13", 3, "CS-F07+CS-F13+CS-F04", "Search query + tag_filter + kind tab composition precision")
    def t3_13(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_13")
        src, man = env.ensure_corpus(u).mutated_valid_script("game", variant=16)
        rp = env.publish_asset(u, "QuantumOrbitX Arena", "Unique search game", "game", ["e2e_quantum", "arcade"], src, man)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        rg = env.feed(u, tab="games", tag_filter="e2e_quantum", query="QuantumOrbitX", limit=10)
        g_items = assert_valid_feed_response(rg.body)
        assert len(g_items) == 1 and g_items[0]["asset"]["scene_id"] == sid
        rv = env.feed(u, tab="videos", tag_filter="e2e_quantum", query="QuantumOrbitX", limit=10)
        assert assert_valid_feed_response(rv.body) == []

    @reg.test("CS-T3-14", 3, "CS-F09+CS-F13+CS-F14", "Follow -> Following tab -> Unfollow immediate removal & viewer flag flip")
    def t3_14(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_14")
        _, assets = env.list_assets(u)
        target = assets[0]
        cid = target["author_user_id"]
        assert extract_follow_flag(env.toggle_follow(u, cid).body) is True
        assert any(it["asset"]["author_user_id"] == cid for it in env.feed(u, tab="following", limit=30).body["items"])
        assert extract_follow_flag(env.toggle_follow(u, cid).body) is False
        assert all(it["asset"]["author_user_id"] != cid for it in env.feed(u, tab="following", limit=30).body["items"])
        _, d = env.get_asset(u, target["scene_id"])
        assert d is not None and d["viewer_following_author"] is False

    @reg.test("CS-T3-15", 3, "CS-F10+CS-F13+CS-F11", "Creator MyWorks aggregates direct publish, AI publish & AI remix exclusively")
    def t3_15(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t3_15a"), env.register_user("t3_15b")
        src, man = env.ensure_corpus(ua).mutated_valid_script("animation", variant=17)
        s_dir = unwrap_asset(env.publish_asset(ua, "Direct Work", "Direct", "animation", ["dir"], src, man).body)["scene_id"]
        d = unwrap_draft(env.ai_generate(ua, "AI work video", kind="video").body)
        s_ai = unwrap_asset(env.ai_publish(ua, draft_id=d["draft_id"]).body)["scene_id"]
        _, seeds = env.list_assets(ua)
        s_rmx = unwrap_asset(env.ai_remix(ua, seeds[0]["scene_id"], "Remix work").body)["scene_id"]
        # User B also publishes one to ensure isolation
        sb = unwrap_asset(env.publish_asset(ub, "Bob Work", "Other", "animation", ["other"], src, man).body)["scene_id"]
        mine = {it["asset"]["scene_id"] for it in assert_valid_feed_response(env.feed(ua, tab="my_works", limit=30).body)}
        assert {s_dir, s_ai, s_rmx}.issubset(mine) and sb not in mine

    @reg.test("CS-T3-16", 3, "CS-F06+CS-F15", "Bilingual RecommendationExplanations & recall_channels across cold, followed & affinity items")
    def t3_16(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_16")
        _, assets = env.list_assets(u)
        env.toggle_follow(u, assets[0]["author_user_id"])
        env.send_telemetry(u, env.make_event(u, assets[1]["scene_id"], "complete", completion_ratio=1.0))
        items = assert_valid_feed_response(env.feed(u, tab="for_you", limit=15).body)
        assert len(items) >= 6
        for it in items:
            assert_valid_explanation(it["explanation"], require_cjk_zh=True)

    @reg.test("CS-T3-17", 3, "CS-F05+CS-F06+CS-F07", "HideTag persistent suppression across paginated refreshes (0% tag leak)")
    def t3_17(env: CodeSceneEnv) -> None:
        u = env.register_user("t3_17")
        _, assets = env.list_assets(u)
        tag_counts: dict[str, int] = {}
        tag_scene: dict[str, str] = {}
        for a in assets:
            for t in a["tags"]:
                tag_counts[t] = tag_counts.get(t, 0) + 1
                tag_scene.setdefault(t, a["scene_id"])
        target_tag = next(t for t, c in tag_counts.items() if c >= 2)
        r = env.send_telemetry(u, env.make_event(u, tag_scene[target_tag], "hide_tag", reason=target_tag))
        assert r.status == 200 and float(r.body["reward"]) < 0.0
        for ref in range(2):
            walked, _ = env.feed_all_pages(u, tab="for_you", session_id=f"ht_{ref}", limit=8)
            for it in walked:
                assert target_tag not in it["asset"]["tags"], f"Hidden tag {target_tag!r} leaked"

    @reg.test("CS-T3-18", 3, "CS-F06+CS-F09+CS-F10", "CreatorJustPublished fast-track + fresh second-user cold-start discovery")
    def t3_18(env: CodeSceneEnv) -> None:
        creator, viewer = env.register_user("t3_18c"), env.register_user("t3_18v")
        d = unwrap_draft(env.ai_generate(creator, "Fresh launch game", kind="game").body)
        sid = unwrap_asset(env.ai_publish(creator, draft_id=d["draft_id"]).body)["scene_id"]
        c_top = [it["asset"]["scene_id"] for it in env.feed(creator, tab="for_you", limit=10).body["items"]]
        assert sid in c_top
        v_found = False
        cur = None
        for _ in range(2):
            rf = env.feed(viewer, tab="for_you", session_id="discover_new", cursor=cur, limit=10)
            if any(it["asset"]["scene_id"] == sid for it in assert_valid_feed_response(rf.body)):
                v_found = True
                break
            cur = rf.body.get("next_cursor")
            if cur is None:
                break
        assert v_found, f"Fresh viewer failed to discover newly published {sid} within 2 pages"

    @reg.test("CS-T3-19", 3, "CS-F03+CS-F05+CS-F12", "Kind-normalized reward parity: deep Game play vs Video completion+replay")
    def t3_19(env: CodeSceneEnv) -> None:
        ug, uv = env.register_user("t3_19g"), env.register_user("t3_19v")
        _, games = env.list_assets(ug, {"kind": "game"})
        _, vids = env.list_assets(uv, {"kind": "video"})
        rg = env.send_telemetry(ug, [
            env.make_event(ug, g["scene_id"], "game_interaction", game_play_ms=40000, game_inputs=75, game_score=300.0)
            for g in games[:3]
        ])
        rv = env.send_telemetry(uv, [
            env.make_event(uv, v["scene_id"], "complete", dwell_ms=15000, completion_ratio=1.0)
            for v in vids[:3]
        ] + [env.make_event(uv, vids[0]["scene_id"], "replay", loop_count=2, completion_ratio=2.0)])
        assert float(rg.body["reward"]) > 0.0 and float(rv.body["reward"]) > 0.0
        fg, _ = env.feed_all_pages(ug, tab="for_you", limit=20)
        fv, _ = env.feed_all_pages(uv, tab="for_you", limit=20)
        g_aff_u1 = sum(it["explanation"]["breakdown"]["tag_kind_affinity"] for it in fg if it["asset"]["kind"] == "game")
        v_aff_u1 = sum(it["explanation"]["breakdown"]["tag_kind_affinity"] for it in fg if it["asset"]["kind"] == "video")
        v_aff_u2 = sum(it["explanation"]["breakdown"]["tag_kind_affinity"] for it in fv if it["asset"]["kind"] == "video")
        g_aff_u2 = sum(it["explanation"]["breakdown"]["tag_kind_affinity"] for it in fv if it["asset"]["kind"] == "game")
        assert g_aff_u1 > v_aff_u1 and v_aff_u2 > g_aff_u2

    @reg.test("CS-T3-20", 3, "CS-F15+CS-F10+CS-F09", "Chat Card action round-trip: Publish -> Open -> Remix -> Share forward")
    def t3_20(env: CodeSceneEnv) -> None:
        ua, ub = env.register_user("t3_20a"), env.register_user("t3_20b")
        c1 = env.create_chat(ua, title="Origin Studio Chat", member_ids=[ub.user_id])
        c2 = env.create_chat(ub, title="Forwarded Friend Chat", member_ids=[ua.user_id])
        d = unwrap_draft(env.ai_generate(ua, "Neon synth video", kind="video").body)
        sid = unwrap_asset(env.ai_publish(ua, draft_id=d["draft_id"], chat_id=c1).body)["scene_id"]
        msgs1 = env.list_chat_messages(ub, c1)
        payloads = assert_valid_generative_card(msgs1[-1], sid)
        open_id = next(p.split(":", 2)[2] for p in payloads if p.startswith("code_scene:open:"))
        remix_id = next(p.split(":", 2)[2] for p in payloads if p.startswith("code_scene:remix:"))
        rd, detail = env.get_asset(ub, open_id)
        assert rd.status == 200 and detail is not None
        rr = env.ai_remix(ub, remix_id, "Remixed by Bob from chat")
        assert rr.status in (200, 201) and unwrap_asset(rr.body)["remix_parent_id"] == sid
        assert env.send_telemetry(ub, env.make_event(ub, sid, "share")).status == 200
        rs = env.client().json("POST", f"/api/v1/chats/{c2}/messages", {"content": msgs1[-1]["content"]}, token=ub.token)
        assert rs.status in (200, 201)
        msgs2 = env.list_chat_messages(ua, c2)
        assert assert_valid_generative_card(msgs2[-1], sid)
