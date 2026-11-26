"""Tier 4: Real-World Multi-User E2E Workloads (11 scenarios: CS-T4-01..CS-T4-11).

Simulates complete Douyin browsing sessions, mid-session interest pivots,
multi-user AI Bot creation & viral chat cascades, 5-thread concurrent stress,
hostile traffic under live load, and physical server restarts.
NEVER imports _contract_double.py. Pure Python 3 stdlib only.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import List

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


def register_tier4_tests(reg: TestRegistry) -> None:
    @reg.test("CS-T4-01", 4, "CS-F06+CS-F05+CS-F13", "Game Enthusiast 25-Swipe Douyin Session")
    def t4_01(env: CodeSceneEnv) -> None:
        u = env.register_user("t4_gamer")
        swipes = 0
        late_items: List[dict] = []
        for rnd in range(6):
            r = env.feed(u, tab="for_you", session_id=f"gamer_rnd_{rnd}", limit=5)
            items = assert_valid_feed_response(r.body, expected_limit=5)
            for it in items:
                assert_valid_explanation(it["explanation"], require_cjk_zh=True)
                a = it["asset"]
                if a["kind"] == "game":
                    evs = [
                        env.make_event(u, a["scene_id"], "game_interaction", game_play_ms=35000, game_inputs=65, game_score=220.0),
                        env.make_event(u, a["scene_id"], "double_tap_like"),
                    ]
                    if swipes % 3 == 0:
                        evs.append(env.make_event(u, a["scene_id"], "bookmark"))
                    assert env.send_telemetry(u, evs).status == 200
                else:
                    assert env.send_telemetry(u, env.make_event(u, a["scene_id"], "fast_skip", dwell_ms=500)).status == 200
                swipes += 1
                if swipes >= 25:
                    break
            if swipes >= 25:
                break
        assert swipes >= 25
        final_feed = assert_valid_feed_response(env.feed(u, tab="for_you", session_id="gamer_final", limit=10).body)
        late_items = final_feed[:8]
        game_share = sum(1 for it in late_items if it["asset"]["kind"] == "game") / max(1, len(late_items))
        assert game_share >= 0.5, f"Expected late-session game share >= 50%, got {game_share:.2%}"
        authors = [it["asset"]["author_user_id"] for it in late_items]
        if len(set(authors)) >= 3:
            for i in range(len(authors) - 2):
                assert not (authors[i] == authors[i + 1] == authors[i + 2]), f"Author spam in late feed: {authors}"

    @reg.test("CS-T4-02", 4, "CS-F14+CS-F02+CS-F06", "Shader / Animation Artist Cold-Start, Code Inspection & Editing Workflow")
    def t4_02(env: CodeSceneEnv) -> None:
        u = env.register_user("t4_artist")
        cold = assert_valid_feed_response(env.feed(u, tab="for_you", limit=12).body)
        assert len({it["asset"]["kind"] for it in cold}) >= 2, "Cold-start artist feed lacked multi-kind diversity"
        base_ani_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in cold if it["asset"]["kind"] == "animation"]
        _, anis = env.list_assets(u, {"kind": "animation"})
        chosen = anis[0]
        assert env.send_telemetry(u, env.make_event(u, chosen["scene_id"], "inspect_code", dwell_ms=14000)).status == 200
        _, fetched = env.get_asset(u, chosen["scene_id"])
        assert fetched is not None and fetched["source_code"]
        ok0, h0, _ = env.extract_validation(env.validate_script(u, fetched["source_code"], fetched["manifest"]))
        mut_src, mut_man = env.ensure_corpus(u).mutated_valid_script("animation", variant=21)
        ok1, h1, _ = env.extract_validation(env.validate_script(u, mut_src, mut_man))
        assert ok0 and ok1 and h0 != h1
        for a in anis[:2]:
            assert env.toggle_bookmark(u, a["scene_id"]).status == 200
            assert env.send_telemetry(u, [
                env.make_event(u, a["scene_id"], "bookmark"),
                env.make_event(u, a["scene_id"], "complete", dwell_ms=15000, completion_ratio=1.0),
            ]).status == 200
        tab_anis = assert_valid_feed_response(env.feed(u, tab="animations", limit=20).body)
        assert len(tab_anis) >= 6 and all(it["asset"]["kind"] == "animation" for it in tab_anis)
        after = assert_valid_feed_response(env.feed(u, tab="for_you", limit=18).body)
        after_ani_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in after if it["asset"]["kind"] == "animation"]
        assert (sum(after_ani_aff) / len(after_ani_aff)) > (sum(base_ani_aff) / max(1, len(base_ani_aff)))

    @reg.test("CS-T4-03", 4, "CS-F06+CS-F05+CS-F07", "Mid-Session Interest Pivot (Videos -> Games in <= 2 Refreshes)")
    def t4_03(env: CodeSceneEnv) -> None:
        u = env.register_user("t4_pivot")
        _, vids = env.list_assets(u, {"kind": "video"})
        _, games = env.list_assets(u, {"kind": "game"})
        # Phase 1: Deep Video affinity
        for v in vids[:6]:
            env.send_telemetry(u, [
                env.make_event(u, v["scene_id"], "complete", dwell_ms=15000, completion_ratio=1.0),
                env.make_event(u, v["scene_id"], "replay", loop_count=2, completion_ratio=2.0),
                env.make_event(u, v["scene_id"], "like"),
            ])
        p1 = assert_valid_feed_response(env.feed(u, tab="for_you", session_id="piv_p1", limit=12).body)
        p1_v = sum(1 for it in p1[:6] if it["asset"]["kind"] == "video")
        p1_g_mean = sum(it["explanation"]["breakdown"]["final_score"] for it in p1 if it["asset"]["kind"] == "game") / max(1, sum(1 for it in p1 if it["asset"]["kind"] == "game"))
        assert p1_v >= 2, f"Phase 1 failed to establish video preference: p1_v={p1_v}"
        # Phase 2: Abrupt pivot to Games
        for v in vids[:4]:
            env.send_telemetry(u, env.make_event(u, v["scene_id"], "fast_skip", dwell_ms=400))
        for g in games[:4]:
            env.send_telemetry(u, [
                env.make_event(u, g["scene_id"], "game_interaction", game_play_ms=35000, game_inputs=70, game_score=280.0),
                env.make_event(u, g["scene_id"], "double_tap_like"),
                env.make_event(u, g["scene_id"], "bookmark"),
            ])
        overtook = False
        for ref in range(2):
            p2 = assert_valid_feed_response(env.feed(u, tab="for_you", session_id=f"piv_p2_{ref}", limit=12).body)
            top = p2[:6]
            g_top = sum(1 for it in top if it["asset"]["kind"] == "game")
            v_top = sum(1 for it in top if it["asset"]["kind"] == "video")
            p2_g_scores = [it["explanation"]["breakdown"]["final_score"] for it in p2 if it["asset"]["kind"] == "game"]
            p2_g_mean = sum(p2_g_scores) / max(1, len(p2_g_scores))
            if g_top >= v_top and g_top >= 2 and p2_g_mean > p1_g_mean:
                overtook = True
                break
        assert overtook, "Mid-session Video->Game pivot did not complete within <= 2 refreshes"

    @reg.test("CS-T4-04", 4, "CS-F10+CS-F08+CS-F06+CS-F14", "AI Bot Studio Lifecycle -> Cold-Start Discovery -> Social & Remix")
    def t4_04(env: CodeSceneEnv) -> None:
        creator = env.register_user("t4_creator")
        vb, vc = env.register_user("t4_view_b"), env.register_user("t4_view_c")
        d1 = unwrap_draft(env.ai_generate(creator, "Make a glowing neon space shooter game", kind="game").body)
        draft_id = d1["draft_id"]
        d2 = unwrap_draft(env.ai_iterate(creator, draft_id=draft_id, prompt="Add boss wave patterns").body)
        draft_id = d2.get("draft_id") or draft_id
        d3 = unwrap_draft(env.ai_iterate(creator, draft_id=draft_id, prompt="Add synth glow trails").body)
        draft_id = d3.get("draft_id") or draft_id
        rp = env.ai_publish(creator, draft_id=draft_id)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        assert unwrap_asset(rp.body)["source_code"] == d3["source_code"]
        for viewer, msg in ((vb, "Amazing bot game!"), (vc, "Love the glow trails!")):
            rf = env.feed(viewer, tab="for_you", limit=15)
            assert any(it["asset"]["scene_id"] == sid for it in assert_valid_feed_response(rf.body)), f"Cold-start viewer missed {sid}"
            assert env.toggle_like(viewer, sid).status == 200
            assert env.toggle_bookmark(viewer, sid).status == 200
            assert env.post_comment(viewer, sid, msg).status in (200, 201)
        rr = env.ai_remix(vc, sid, "Remix into underwater theme")
        assert rr.status in (200, 201)
        child = unwrap_asset(rr.body)
        assert child is not None and child["remix_parent_id"] == sid
        _, final_d = env.get_asset(creator, sid)
        assert final_d is not None
        assert final_d["likes_count"] == 2 and final_d["bookmarks_count"] >= 2
        assert final_d["comments_count"] == 2 and final_d["remixes_count"] == 1

    @reg.test("CS-T4-05", 4, "CS-F09+CS-F11+CS-F06", "Concurrent 5-User Multi-Threaded Telemetry, Social & Publishing Stress Test")
    def t4_05(env: CodeSceneEnv) -> None:
        users = [env.register_user(f"t4_conc_{i}") for i in range(5)]
        _, assets = env.list_assets(users[0])
        shared = assets[0]
        shared_id = shared["scene_id"]
        init_likes = shared["likes_count"]
        init_bmk = shared["bookmarks_count"]
        init_cmt = shared["comments_count"]
        corpus = env.ensure_corpus(users[0])

        def worker(idx: int) -> str:
            u = users[idx]
            rf = env.feed(u, tab="for_you", session_id=f"conc_{idx}", limit=8)
            assert rf.status == 200
            assert_valid_feed_response(rf.body)
            rt = env.send_telemetry(u, [
                env.make_event(u, shared_id, "dwell", dwell_ms=9000),
                env.make_event(u, shared_id, "double_tap_like"),
            ])
            assert rt.status == 200
            assert env.toggle_like(u, shared_id).status == 200
            assert env.toggle_bookmark(u, shared_id).status == 200
            assert env.post_comment(u, shared_id, f"Concurrent comment #{idx}").status in (200, 201)
            src, man = corpus.mutated_valid_script("animation", variant=30 + idx)
            ok, ch, _ = env.extract_validation(env.validate_script(u, src, man))
            assert ok and ch
            d = unwrap_draft(env.ai_generate(u, f"Concurrent wave #{idx}", kind="animation").body)
            rp = env.ai_publish(u, draft_id=d["draft_id"])
            assert rp.status in (200, 201)
            return unwrap_asset(rp.body)["scene_id"]

        created_ids: List[str] = []
        with ThreadPoolExecutor(max_workers=5) as ex:
            futs = [ex.submit(worker, i) for i in range(5)]
            for fut in as_completed(futs):
                created_ids.append(fut.result())

        assert len(created_ids) == 5 and len(set(created_ids)) == 5
        _, after = env.get_asset(users[0], shared_id)
        assert after is not None
        assert after["likes_count"] == init_likes + 5
        assert after["bookmarks_count"] == init_bmk + 5
        assert after["comments_count"] == init_cmt + 5
        for sid in created_ids:
            rd, det = env.get_asset(users[0], sid)
            assert rd.status == 200 and det is not None

    @reg.test("CS-T4-06", 4, "CS-F02+CS-F09+CS-F12", "Hostile Adversarial Bot Script Injection Under Healthy Live Traffic")
    def t4_06(env: CodeSceneEnv) -> None:
        attacker = env.register_user("t4_atk")
        benign = env.register_user("t4_benign")
        corpus = env.ensure_corpus(benign)
        before_total = env.feed(benign, tab="for_you", limit=5).body["total_catalog"]
        hostiles = [
            ("oversized", corpus.oversized_script(70_000)),
            ("inf_loop", corpus.hostile_infinite_loop_script()),
            ("recursion", corpus.hostile_deep_recursion_script()),
            ("heap_bomb", corpus.hostile_heap_bomb_script()),
            ("ast_bomb", corpus.hostile_ast_depth_script(80)),
            ("zero_draw", corpus.hostile_zero_draw_script()),
        ]
        for label, (h_src, h_man) in hostiles:
            rv = env.validate_script(attacker, h_src, h_man)
            ok, _, _ = env.extract_validation(rv)
            assert not ok, f"Hostile {label} unexpectedly passed validation"
            rp = env.publish_asset(attacker, f"Evil {label}", "Attack", "animation", ["evil"], h_src, h_man)
            assert rp.status == 422, f"Hostile {label} publish returned {rp.status}, expected 422"
        # Benign live user executes cleanly alongside hostile traffic
        rf = env.feed(benign, tab="for_you", limit=8)
        assert rf.status == 200 and len(assert_valid_feed_response(rf.body)) >= 5
        b_src, b_man = corpus.mutated_valid_script("game", variant=41)
        ok_b, ch_b, _ = env.extract_validation(env.validate_script(benign, b_src, b_man))
        assert ok_b and ch_b
        r_pub = env.publish_asset(benign, "Healthy Benign Game", "Clean code", "game", ["clean"], b_src, b_man)
        assert r_pub.status in (200, 201)
        after_total = env.feed(benign, tab="for_you", limit=5).body["total_catalog"]
        assert after_total == before_total + 1, f"Catalog count expected +1, got {before_total} -> {after_total}"

    @reg.test("CS-T4-07", 4, "CS-F09+CS-F11+CS-F06", "Full Multi-User Server Restart Continuity (3 Users)")
    def t4_07(env: CodeSceneEnv) -> None:
        u1, u2, u3 = env.register_user("t4_r1"), env.register_user("t4_r2"), env.register_user("t4_r3")
        d = unwrap_draft(env.ai_generate(u1, "Multi-user restart arcade", kind="game").body)
        s1 = unwrap_asset(env.ai_publish(u1, draft_id=d["draft_id"]).body)["scene_id"]
        _, games = env.list_assets(u1, {"kind": "game"})
        _, vids = env.list_assets(u1, {"kind": "video"})
        for v in vids[:3]:
            env.send_telemetry(u1, env.make_event(u1, v["scene_id"], "fast_skip", dwell_ms=450))
        for g in games[:3]:
            env.send_telemetry(u1, env.make_event(u1, g["scene_id"], "game_interaction", game_play_ms=30000, game_inputs=60, game_score=250.0))
        assert env.toggle_like(u2, s1).status == 200
        assert env.toggle_bookmark(u2, s1).status == 200
        assert env.post_comment(u2, s1, "U2 durable comment").status in (200, 201)
        assert extract_follow_flag(env.toggle_follow(u2, u1.user_id).body) is True
        rr = env.ai_remix(u3, s1, "U3 remix before restart")
        assert rr.status in (200, 201)
        s3 = unwrap_asset(rr.body)["scene_id"]
        hid_author = vids[0]["author_user_id"]
        assert env.send_telemetry(u3, env.make_event(u3, vids[0]["scene_id"], "hide_author", author_user_id=hid_author)).status == 200

        env.restart_server()

        _, d1 = env.get_asset(u2, s1)
        assert d1 is not None and d1["viewer_liked"] and d1["viewer_bookmarked"] and d1["viewer_following_author"]
        assert d1["comments_count"] >= 1 and d1["remixes_count"] >= 1
        _, d3 = env.get_asset(u3, s3)
        assert d3 is not None and d3["remix_parent_id"] == s1
        u3_feed, _ = env.feed_all_pages(u3, tab="for_you", limit=20)
        assert len(u3_feed) >= 1 and all(it["asset"]["author_user_id"] != hid_author for it in u3_feed), (
            "Hidden author leaked post-restart"
        )
        u1_feed, _ = env.feed_all_pages(u1, tab="for_you", limit=20)
        assert len(u1_feed) >= 6
        g_top = sum(1 for it in u1_feed[:10] if it["asset"]["kind"] == "game")
        v_top = sum(1 for it in u1_feed[:10] if it["asset"]["kind"] == "video")
        g_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in u1_feed if it["asset"]["kind"] == "game"]
        v_aff = [it["explanation"]["breakdown"]["tag_kind_affinity"] for it in u1_feed if it["asset"]["kind"] == "video"]
        assert g_top >= v_top and g_aff and v_aff and (sum(g_aff) / len(g_aff)) > (sum(v_aff) / len(v_aff))

    @reg.test("CS-T4-08", 4, "CS-F15+CS-F10+CS-F06", "Multi-Creator Viral Cascade & Chat Card Amplification")
    def t4_08(env: CodeSceneEnv) -> None:
        creator = env.register_user("t4_viral_c")
        bob, carol, dave = env.register_user("t4_bob"), env.register_user("t4_carol"), env.register_user("t4_dave")
        cold_pre, cold = env.register_user("t4_cold_pre"), env.register_user("t4_cold4")
        gid = env.create_chat(creator, title="Viral Scene Squad", member_ids=[bob.user_id, carol.user_id, dave.user_id])
        d = unwrap_draft(env.ai_generate(creator, "Viral laser pong game", kind="game").body)
        rp = env.ai_publish(creator, draft_id=d["draft_id"], chat_id=gid)
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        pre_items, _ = env.feed_all_pages(cold_pre, tab="for_you", session_id="pre_viral", limit=20)
        pre_item = next((it for it in pre_items if it["asset"]["scene_id"] == sid), None)
        assert pre_item is not None, f"Newly published scene {sid} missing from pre-viral cold feed"
        pre_ecpm = float(pre_item["explanation"]["breakdown"]["quality_ecpm"])
        for member in (bob, carol, dave):
            msgs = env.list_chat_messages(member, gid)
            payloads = assert_valid_generative_card(msgs[-1], sid)
            open_id = next(p.split(":", 2)[2] for p in payloads if p.startswith("code_scene:open:"))
            assert env.get_asset(member, open_id)[0].status == 200
            env.send_telemetry(member, env.make_event(member, sid, "complete", dwell_ms=18000, completion_ratio=1.0))
            assert env.toggle_like(member, sid).status == 200
            assert env.post_comment(member, sid, f"Awesome from {member.username}!").status in (200, 201)
        for remixer in (bob, carol):
            rr = env.ai_remix(remixer, sid, f"Viral remix by {remixer.username}")
            assert rr.status in (200, 201) and unwrap_asset(rr.body)["remix_parent_id"] == sid
        _, parent_after = env.get_asset(creator, sid)
        assert parent_after is not None
        assert parent_after["likes_count"] == 3 and parent_after["comments_count"] == 3 and parent_after["remixes_count"] == 2
        cold_items = assert_valid_feed_response(env.feed(cold, tab="for_you", limit=15).body)
        viral_item = next((it for it in cold_items if it["asset"]["scene_id"] == sid), None)
        assert viral_item is not None, f"Viral scene {sid} missing from cold viewer top-15"
        viral_ecpm = float(viral_item["explanation"]["breakdown"]["quality_ecpm"])
        assert viral_ecpm > 0.0 and viral_ecpm > pre_ecpm, (
            f"Expected scale-invariant viral quality_ecpm > pre-viral baseline ({pre_ecpm}), got {viral_ecpm}"
        )

    @reg.test("CS-T4-09", 4, "CS-F06+CS-F05+CS-F07", "Aggressive Negative Feedback & Feed Hygiene Workout (20 Swipes)")
    def t4_09(env: CodeSceneEnv) -> None:
        u = env.register_user("t4_hygiene")
        dismissed_scenes: set[str] = set()
        hidden_authors: set[int] = set()
        hidden_tags: set[str] = set()
        swipes = 0
        early_paginated_checked = False
        for step in range(20):
            rf = env.feed(u, tab="for_you", session_id=f"hyg_{step}", limit=5)
            items = assert_valid_feed_response(rf.body)
            for it in items:
                a = it["asset"]
                assert a["scene_id"] not in dismissed_scenes, f"Dismissed scene leaked at step {step}"
                assert a["author_user_id"] not in hidden_authors, f"Hidden author leaked at step {step}"
                assert all(t not in hidden_tags for t in a["tags"]), f"Hidden tag leaked at step {step}"
            if not items:
                break
            target = items[0]["asset"]
            mod = step % 3
            if mod == 0:
                dismissed_scenes.add(target["scene_id"])
                assert env.send_telemetry(u, env.make_event(u, target["scene_id"], "not_interested")).status == 200
            elif mod == 1 and len(hidden_authors) < 3:
                hidden_authors.add(target["author_user_id"])
                assert env.send_telemetry(u, env.make_event(u, target["scene_id"], "hide_author", author_user_id=target["author_user_id"])).status == 200
            elif target["tags"] and len(hidden_tags) < 2:
                tg = next(
                    (t for t in target["tags"] if t.lower() not in ("video", "animation", "game")),
                    target["tags"][0],
                )
                hidden_tags.add(tg)
                assert env.send_telemetry(u, env.make_event(u, target["scene_id"], "hide_tag", reason=tg)).status == 200
            else:
                dismissed_scenes.add(target["scene_id"])
                assert env.send_telemetry(u, env.make_event(u, target["scene_id"], "not_interested")).status == 200
            swipes += 1
            if not early_paginated_checked and dismissed_scenes and hidden_authors:
                # Explicitly walk >= 2 pages (cursor != None) while ample catalog remains
                early_items, early_pages = env.feed_all_pages(u, tab="for_you", session_id="hyg_early_walk", limit=5)
                assert early_pages >= 2, f"Expected >= 2 paginated pages after hide_author, got {early_pages}"
                for it in early_items:
                    a = it["asset"]
                    assert a["scene_id"] not in dismissed_scenes, "Dismissed scene leaked on paginated walk"
                    assert a["author_user_id"] not in hidden_authors, "Hidden author leaked on paginated walk"
                    assert all(t not in hidden_tags for t in a["tags"]), "Hidden tag leaked on paginated walk"
                early_paginated_checked = True
        assert swipes >= 1 and early_paginated_checked
        # Full paginated exhaustion check verifying 0% re-appearance across all remaining items
        final_items, _ = env.feed_all_pages(u, tab="for_you", session_id="hyg_final_walk", limit=4)
        for it in final_items:
            a = it["asset"]
            assert a["scene_id"] not in dismissed_scenes
            assert a["author_user_id"] not in hidden_authors
            assert all(t not in hidden_tags for t in a["tags"])

    @reg.test("CS-T4-10", 4, "CS-F08+CS-F10+CS-F14+CS-F15", "Multilingual CJK / Emoji / English Full-Lifecycle Studio Session")
    def t4_10(env: CodeSceneEnv) -> None:
        u = env.register_user("t4_cjk")
        d = unwrap_draft(env.ai_generate(u, "做一个赛博烟花倒计时视频🎆", kind="video").body)
        ok, ch, _ = env.extract_validation(env.validate_script(u, d["source_code"], d["manifest"]))
        assert ok and ch
        rp = env.ai_publish(u, draft_id=d["draft_id"], overrides={
            "title": "霓虹烟花✨",
            "description": "纯代码烟花倒计时 Firework Countdown 🎆",
            "tags": ["烟花🎆", "赛博", "video"],
        })
        assert rp.status in (200, 201)
        sid = unwrap_asset(rp.body)["scene_id"]
        assert env.post_comment(u, sid, "烟花粒子太震撼了！Awesome 🎆🚀").status in (200, 201)
        rq = env.feed(u, tab="for_you", query="霓虹烟花", limit=10)
        q_items = assert_valid_feed_response(rq.body)
        assert any(it["asset"]["scene_id"] == sid for it in q_items)
        rt = env.feed(u, tab="for_you", tag_filter="烟花🎆", limit=10)
        t_items = assert_valid_feed_response(rt.body)
        assert any(it["asset"]["scene_id"] == sid for it in t_items)
        for it in t_items:
            assert_valid_explanation(it["explanation"], require_cjk_zh=True)

    @reg.test("CS-T4-11", 4, "CS-F13+CS-F09+CS-F06", "Following-Feed Curator Lifecycle (Empty -> Follow 2 -> Live Publish -> Unfollow 1)")
    def t4_11(env: CodeSceneEnv) -> None:
        curator = env.register_user("t4_curator")
        human_creator = env.register_user("t4_human_c")
        assert env.feed(curator, tab="following", limit=10).body["items"] == []
        _, seeds = env.list_assets(curator)
        bot_id = seeds[0]["author_user_id"]
        assert extract_follow_flag(env.toggle_follow(curator, bot_id).body) is True
        assert extract_follow_flag(env.toggle_follow(curator, human_creator.user_id).body) is True
        src, man = env.ensure_corpus(human_creator).mutated_valid_script("animation", variant=55)
        rp = env.publish_asset(human_creator, "Curated Live Drop", "Fresh drop", "animation", ["curated"], src, man)
        assert rp.status in (200, 201)
        new_sid = unwrap_asset(rp.body)["scene_id"]
        fol1 = assert_valid_feed_response(env.feed(curator, tab="following", limit=30).body)
        fol1_authors = {it["asset"]["author_user_id"] for it in fol1}
        assert fol1_authors == {bot_id, human_creator.user_id}
        assert any(it["asset"]["scene_id"] == new_sid for it in fol1)
        assert extract_follow_flag(env.toggle_follow(curator, bot_id).body) is False
        fol2 = assert_valid_feed_response(env.feed(curator, tab="following", limit=30).body)
        assert all(it["asset"]["author_user_id"] == human_creator.user_id for it in fol2)
        assert any(it["asset"]["scene_id"] == new_sid for it in fol2)
