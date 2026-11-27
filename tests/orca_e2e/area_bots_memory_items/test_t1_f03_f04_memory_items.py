"""Tier 1 opaque-box E2E tests for F03 (Memory Governance & Tidy-Up) and F04 (Bot Items & Needs You)."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import Any

import pytest

from harness import BOT_ITEM_STATES_6, TIDY_UP_MAX_FILED, TIDY_UP_MAX_NEW_FACTS, CliResult, OrcaApiClient, OrcaCliRunner


def _unwrap(payload: Any, key: str) -> Any:
    return payload[key] if isinstance(payload, dict) and key in payload else payload


def _cli_json_ok(res: CliResult) -> Any:
    data = res.assert_ok()
    assert isinstance(data, (dict, list)), f"expected JSON dict/list from CLI, got {data!r}"
    if isinstance(data, dict):
        assert data.get("ok") is not False, f"CLI returned ok=false: {data!r}"
    return data


def _create_bot(orca_api: OrcaApiClient, name: str, agent: str = "claude", **extra: Any) -> dict[str, Any]:
    bot = _unwrap(orca_api.assert_ok(orca_api.post("/api/v1/orca/bots", json_body={"name": name, "agent": agent, **extra})), "bot")
    assert isinstance(bot, dict) and isinstance(bot.get("id"), str) and bot["id"]
    return bot


def _create_item(orca_api: OrcaApiClient, bot_id: str, title: str, **extra: Any) -> dict[str, Any]:
    item = _unwrap(orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/items", json_body={"title": title, **extra})), "item")
    assert isinstance(item, dict) and isinstance(item.get("number"), int) and item["number"] > 0
    return item


def _memory_text(orca_api: OrcaApiClient, bot_id: str) -> str:
    data = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/bots/{bot_id}/memory"))
    text = data["content"] if isinstance(data, dict) and isinstance(data.get("content"), str) else str(data)
    assert isinstance(text, str)
    return text


@pytest.mark.tier1
@pytest.mark.f03
def test_f03_t1_owner_and_external_turn_memory_provenance_tagging(orca_api: OrcaApiClient) -> None:
    bot = _create_bot(orca_api, "Memory Prov Bot", agent="claude")
    bot_id = bot["id"]
    base = "# Memory\n\n- 2026-11-24 Fact: Existing baseline note.\n"
    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/memory", json_body={"content": base}))

    owner_next = base + "- 2026-11-25 Fact: Workspace uses Rust edition 2024.\n"
    orca_api.assert_rpc_ok("bots.memory.applyTurn", {"botId": bot_id, "source": None, "content": owner_next})

    src = "Slack, Dana <https://slack.example/C1/p1>"
    ext_next = owner_next + "- 2026-11-25 Lesson (ci): Mold cuts Linux link time.\n"
    orca_api.assert_rpc_ok("bots.memory.applyTurn", {"botId": bot_id, "source": src, "content": ext_next})

    lines = [ln.strip() for ln in _memory_text(orca_api, bot_id).splitlines() if ln.strip().startswith("- ")]
    base_line = next(ln for ln in lines if "Existing baseline note." in ln)
    owner_line = next(ln for ln in lines if "Workspace uses Rust edition 2024." in ln)
    ext_line = next(ln for ln in lines if "Mold cuts Linux link time." in ln)
    assert "· from:" not in base_line
    assert "· from:" not in owner_line
    assert ext_line.endswith(f" · from: {src}")


@pytest.mark.tier1
@pytest.mark.f03
def test_f03_t1_explicit_memory_forget_and_one_click_undo_rest_and_cli(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    bot = _create_bot(orca_api, "Forget Undo Bot", agent="claude")
    bot_id = bot["id"]
    seed = (
        "# Memory\n\n"
        "- 2026-11-25 Fact: Staging host is staging.internal.\n"
        "- 2026-11-25 Fact: Release branch is main.\n"
    )
    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/memory", json_body={"content": seed}))

    _cli_json_ok(orca_cli.run("bot", "memory-forget", "--bot", bot_id, "--fact", "staging.internal"))
    after_cli_forget = _memory_text(orca_api, bot_id)
    assert "staging.internal" not in after_cli_forget
    assert "Release branch is main." in after_cli_forget

    _cli_json_ok(orca_cli.run("bot", "memory-undo", "--bot", bot_id))
    assert "Staging host is staging.internal." in _memory_text(orca_api, bot_id)

    orca_api.assert_ok(
        orca_api.post(f"/api/v1/orca/bots/{bot_id}/memory/forget", json_body={"fact": "Release branch is main"})
    )
    assert "Release branch is main." not in _memory_text(orca_api, bot_id)
    assert "staging.internal" in _memory_text(orca_api, bot_id)

    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/memory/undo", json_body={}))
    restored = _memory_text(orca_api, bot_id)
    assert "Release branch is main." in restored and "staging.internal" in restored


@pytest.mark.tier1
@pytest.mark.f03
def test_f03_t1_deterministic_monthly_journal_on_item_close_owner_and_non_owner(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    bot = _create_bot(orca_api, "Journal Bot", agent="claude")
    bot_id = bot["id"]
    item1 = _create_item(orca_api, bot_id, "Ship relay reconnect fix", links=[{"kind": "pr", "ref": "PR #42"}])
    _cli_json_ok(orca_cli.run("bot", "item-done", str(item1["number"]), "--note", "Merged"))

    item2 = _create_item(
        orca_api,
        bot_id,
        "Investigate flaky staging job",
        madeBy={"kind": "person", "name": "Dana", "source": "Slack"},
    )
    orca_api.assert_ok(
        orca_api.patch(
            f"/api/v1/orca/items/{item2['number']}",
            json_body={"status": "canceled", "close_note": "Duplicate"},
        )
    )

    orca_api.assert_rpc_ok("bots.memory.journal", {"botId": bot_id, "sync": True})
    j2 = orca_api.assert_rpc_ok("bots.memory.journal", {"botId": bot_id, "sync": True})
    content = str(j2.get("content", ""))
    lines = [ln for ln in content.splitlines() if f"item #{item1['number']}" in ln]
    lines2 = [ln for ln in content.splitlines() if f"item #{item2['number']}" in ln]
    assert len(lines) == 1
    assert "| done |" in lines[0] and "PR #42" in lines[0] and "Ship relay reconnect fix" in lines[0]
    assert "not your owner" not in lines[0]
    assert len(lines2) == 1
    assert "| canceled |" in lines2[0] and "Investigate flaky staging job" in lines2[0]
    assert "for Dana via Slack, not your owner" in lines2[0]


@pytest.mark.tier1
@pytest.mark.f03
def test_f03_t1_nightly_tidy_up_valid_consolidation_history_and_undo(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    bot = _create_bot(orca_api, "TidyUp Bot", agent="claude", tidyUp={"schedule": "night"})
    bot_id = bot["id"]
    before = (
        "# Memory\n\n"
        "- 2026-11-20 Fact: Old staging URL is old.staging.local.\n"
        "- 2026-11-21 Lesson (ci): Shard 1 caches cargo registry.\n"
        "- 2026-11-22 Lesson (ci): Shard 2 caches cargo registry.\n"
    )
    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/memory", json_body={"content": before}))

    draft = {
        "learned": ["- 2026-11-25 Fact: Nightly release runs at 04:00 UTC."],
        "filed": [{"topic": "ci", "lines": ["- 2026-11-25 Lesson (ci): All shards share cargo registry cache."]}],
        "replaced": [{"old": "- 2026-11-20 Fact: Old staging URL is old.staging.local.", "new": "- 2026-11-25 Fact: Staging URL is staging.internal."}],
        "merged": [{
            "sources": ["- 2026-11-21 Lesson (ci): Shard 1 caches cargo registry.", "- 2026-11-22 Lesson (ci): Shard 2 caches cargo registry."],
            "into": "- 2026-11-25 Lesson (ci): All CI shards cache cargo registry.",
        }],
    }
    assert len(draft["learned"]) <= TIDY_UP_MAX_NEW_FACTS and len(draft["filed"]) <= TIDY_UP_MAX_FILED
    res = orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/tidy-up", json_body={"draft": draft}))
    assert res["changesCount"] > 0
    assert f"Tidied memory · {res['changesCount']} changes" in str(res.get("statusNote", ""))
    assert res.get("historyId") or res.get("undoable") is True
    assert "staging.internal" in _memory_text(orca_api, bot_id)

    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/tidy-up/undo", json_body={}))
    assert _memory_text(orca_api, bot_id).strip() == before.strip()

    cli_tidy = _cli_json_ok(orca_cli.run("bot", "tidy-up", "--bot", bot_id))
    assert isinstance(cli_tidy, dict) and any(k in cli_tidy for k in ("changesCount", "summary", "statusNote"))


@pytest.mark.tier1
@pytest.mark.f03
def test_f03_t1_tidy_up_owner_only_input_filter_and_sleep_catchup_schedule(orca_api: OrcaApiClient) -> None:
    bot = _create_bot(orca_api, "Schedule Tidy Bot", agent="claude", tidyUp={"schedule": "night"})
    bot_id = bot["id"]
    for local_time, since, expected_due in (
        ("2026-11-25T02:00:00", "2026-11-24T12:00:00", False),
        ("2026-11-25T04:30:00", "2026-11-24T12:00:00", True),
        ("2026-11-25T10:00:00", "2026-11-24T12:00:00", True),
        ("2026-11-25T10:00:00", "2026-11-25T09:00:00", False),
    ):
        ev = orca_api.assert_rpc_ok(
            "bots.tidyUp.evaluateSchedule",
            {"botId": bot_id, "schedule": "night", "localNow": local_time, "since": since},
        )
        assert ev["dueNow"] is expected_due

    turns = [
        {"chatId": f"c-{i}", "author": "owner", "source": "owner", "text": f"Owner insight {i}"}
        for i in range(25)
    ] + [
        {"chatId": "c-slack", "author": "dana", "source": "slack", "text": "Slack noise"},
        {"chatId": "c-hook", "author": "gh", "source": "webhook", "text": "Webhook noise"},
        {"chatId": "c-mail", "author": "peer", "source": "mail", "text": "Peer mail noise"},
        {"chatId": "c-auto", "author": "cron", "source": "automation", "text": "Automation noise"},
    ]
    collected = orca_api.assert_rpc_ok("bots.tidyUp.collectInputs", {"botId": bot_id, "turns": turns})
    kept = collected["turns"]
    assert 0 < len(kept) <= 20
    assert all(t["source"] == "owner" for t in kept)
    assert sum(len(str(t["text"])) for t in kept) <= 40_000


@pytest.mark.tier1
@pytest.mark.f03
def test_f03_t1_typed_memory_prefixes_and_scoped_rule_classification(orca_api: OrcaApiClient) -> None:
    bot = _create_bot(orca_api, "Typed Memory Bot", agent="claude")
    bot_id = bot["id"]
    content = (
        "# Memory\n\n"
        "- 2026-11-25 Rule (gpui-telegram): Never delete ledger files.\n"
        "- 2026-11-25 Prefers (rust): Lean cargo profiles.\n"
        "- 2026-11-25 Decided: Release v1.7.0.\n"
        "- 2026-11-25 Lesson (ci): Cache mold.\n"
        "- 2026-11-25 Fact: Default port is ephemeral.\n"
    )
    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/memory", json_body={"content": content}))
    inspected = orca_api.assert_rpc_ok("bots.memory.inspect", {"botId": bot_id})
    entries = {e["kind"]: e for e in inspected["entries"]}
    assert set(entries.keys()) == {"Rule", "Prefers", "Decided", "Lesson", "Fact"}
    assert entries["Rule"]["scope"] == "gpui-telegram" and entries["Rule"]["protected"] is True
    assert entries["Prefers"]["scope"] == "rust" and entries["Prefers"]["protected"] is True
    assert entries["Decided"]["protected"] is True
    assert entries["Lesson"]["scope"] == "ci" and entries["Lesson"]["protected"] is False
    assert entries["Fact"]["protected"] is False


@pytest.mark.tier1
@pytest.mark.f04
def test_t1_f04_item_six_state_lifecycle_and_event_log(orca_api: OrcaApiClient) -> None:
    assert BOT_ITEM_STATES_6 == ("planned", "working", "waiting", "needs_you", "done", "canceled")
    bot_id = _create_bot(orca_api, "Lifecycle Item Bot", agent="claude")["id"]
    item_a = _create_item(
        orca_api,
        bot_id,
        "Audit CI flakes",
        description="Inspect retry logs",
        done_means="All shards green",
        status="planned",
    )
    num_a = item_a["number"]
    prev_ver = int(item_a["version"])
    transitions = [
        {"status": "working"},
        {"status": "waiting", "wait": "Waiting on GitHub Actions shard #3"},
        {"status": "needs_you", "comment": "Approve fallback?"},
        {"status": "done", "close_note": "Merged PR #101"},
    ]
    for patch_body in transitions:
        upd = orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{num_a}", json_body=patch_body))
        rec = upd["item"] if isinstance(upd, dict) and isinstance(upd.get("item"), dict) else upd
        assert int(rec["version"]) > prev_ver
        prev_ver = int(rec["version"])
        if patch_body["status"] == "waiting":
            assert rec.get("wait_event") is not None
        if patch_body["status"] == "needs_you":
            assert rec.get("question_event") is not None
        if patch_body["status"] == "done":
            assert rec.get("closed_at") is not None and rec.get("close_note") == "Merged PR #101"

    item_b = _create_item(orca_api, bot_id, "Superseded task", status="planned")
    assert item_b["number"] > num_a
    orca_api.assert_ok(
        orca_api.patch(f"/api/v1/orca/items/{item_b['number']}", json_body={"status": "canceled", "close_note": "Superseded"})
    )

    detail = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{num_a}"))
    events = detail["events"]
    assert isinstance(events, list) and len(events) >= 5
    listed = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/bots/{bot_id}/items"))
    items_arr = listed["items"] if isinstance(listed, dict) and "items" in listed else listed
    by_num = {it["number"]: it for it in items_arr}
    assert by_num[num_a]["status"] == "done" and by_num[item_b["number"]]["status"] == "canceled"


@pytest.mark.tier1
@pytest.mark.f04
def test_t1_f04_multi_decision_ask_card_and_quick_reply_mail_owed(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "Ask Card Bot", agent="claude")["id"]
    num = _create_item(orca_api, bot_id, "Release rollout decision")["number"]
    ask_payload = {
        "questions": [
            {
                "prompt": "Deploy strategy?", "multi": False, "recommended": 0,
                "options": [{"label": "Canary", "description": "10% traffic"}, {"label": "Blue-Green", "description": "Instant cutover"}],
            },
            {"prompt": "Target regions?", "multi": True, "options": [{"label": "us-east"}, {"label": "eu-west"}, {"label": "ap-south"}]},
        ]
    }
    orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{num}", json_body={"status": "needs_you", "ask": ask_payload}))
    before = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{num}"))
    rec = _unwrap(before, "item")
    assert rec["status"] == "needs_you" and rec.get("question_event") is not None
    questions = before.get("ask", rec.get("ask", {})).get("questions", [])
    assert len(questions) == 2 and questions[0]["recommended"] == 0 and questions[1]["multi"] is True
    fallback = before.get("plainTextFallback") or rec.get("plainTextFallback")
    assert isinstance(fallback, str) and "Deploy strategy?" in fallback and "Canary" in fallback

    reply_text = "Deploy strategy? → Canary\nTarget regions? → us-east, eu-west"
    orca_api.assert_ok(
        orca_api.post(
            f"/api/v1/orca/items/{num}/comment",
            json_body={"actor": {"kind": "owner"}, "text": reply_text, "answers": {"Deploy strategy?": ["Canary"], "Target regions?": ["us-east", "eu-west"]}},
        )
    )
    after = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{num}"))
    assert _unwrap(after, "item")["status"] == "working"
    comment_ev = next((ev for ev in after["events"] if ev.get("kind") == "commented"), None)
    assert comment_ev is not None and comment_ev.get("mail") == "owed"


@pytest.mark.tier1
@pytest.mark.f04
def test_t1_f04_typed_links_and_dispatch_lineage_dedup(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    bot_id = _create_bot(orca_api, "Link Lineage Bot", agent="claude")["id"]
    num = _create_item(orca_api, bot_id, "Track multi-link task")["number"]
    links = [
        {"kind": "dispatch", "ref": "disp-chain-1-run-1", "chain_id": "chain-1", "state": "completed"},
        {"kind": "pr", "ref": "acme/repo#42", "state": "open"},
        {"kind": "url", "ref": "https://github.com/acme/repo/issues/17"},
        {"kind": "file", "ref": "crates/orca_bots/src/items.rs"},
        {"kind": "automation", "ref": "auto-watch-42", "active": True},
    ]
    for lnk in links:
        orca_api.assert_rpc_ok("bots.items.link", {"number": num, "link": lnk})

    _cli_json_ok(
        orca_cli.run(
            "bot", "item-link", str(num), "--kind", "dispatch", "--ref", "disp-chain-1-run-2",
            "--chain-id", "chain-1", "--parent-dispatch", "disp-chain-1-run-1",
        )
    )
    detail = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{num}"))
    stored = detail["links"] if "links" in detail else detail["item"]["links"]
    assert {lnk["kind"] for lnk in stored} == {"dispatch", "pr", "url", "file", "automation"}
    chain_dispatches = [lnk for lnk in stored if lnk["kind"] == "dispatch" and lnk.get("chain_id") == "chain-1"]
    assert len(chain_dispatches) == 1 and chain_dispatches[0]["ref"] == "disp-chain-1-run-2"
    assert len(stored) == 5 and all(lnk.get("added_event") is not None for lnk in stored)


@pytest.mark.tier1
@pytest.mark.f04
def test_t1_f04_needs_you_priority_folded_inbox_and_stale_policy(orca_api: OrcaApiClient) -> None:
    bot_a = _create_bot(orca_api, "Inbox Bot A", agent="claude", itemsStale=3)
    bot_b = _create_bot(orca_api, "Inbox Bot B", agent="codex")
    assert bot_a["itemsStale"] == 3 and bot_b["itemsStale"] == 5

    ny_a = _create_item(orca_api, bot_a["id"], "Bot A approval")["number"]
    orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{ny_a}", json_body={"status": "needs_you", "comment": "Approve A?"}))
    ny_b = _create_item(orca_api, bot_b["id"], "Bot B approval")["number"]
    orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{ny_b}", json_body={"status": "needs_you", "comment": "Approve B?"}))
    planned_num = 0
    for st in ("waiting", "working", "planned", "done", "canceled"):
        n = _create_item(orca_api, bot_a["id"], f"Item {st}")["number"]
        if st == "planned":
            planned_num = n
        else:
            orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{n}", json_body={"status": st}))

    inbox = orca_api.assert_ok(orca_api.get("/api/v1/orca/needs-you"))
    queue = inbox["items"] if isinstance(inbox, dict) and "items" in inbox else inbox
    nums = [it["number"] for it in queue]
    assert ny_a in nums and ny_b in nums and all(it["status"] not in ("done", "canceled") for it in queue)
    seen_other = False
    for entry in queue:
        if entry.get("status") == "needs_you":
            assert not seen_other, f"needs_you item appeared after non-needs_you: {queue!r}"
        else:
            seen_other = True
    ny_d = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{ny_a}"))
    pl_d = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{planned_num}"))
    ny_rec, pl_rec = _unwrap(ny_d, "item"), _unwrap(pl_d, "item")
    assert (pl_rec.get("staleAfterDays") or pl_d.get("staleAfterDays")) == 9
    assert (ny_rec.get("staleAfterDays") or ny_d.get("staleAfterDays")) == 3


@pytest.mark.tier1
@pytest.mark.f04
def test_t1_f04_owner_snooze_and_listing_pagination(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "Snooze Page Bot", agent="claude")["id"]
    nums = [_create_item(orca_api, bot_id, f"Paged item {i}")["number"] for i in range(6)]
    orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{nums[0]}", json_body={"status": "needs_you", "comment": "Review"}))
    until = (datetime.now(timezone.utc) + timedelta(hours=24)).isoformat()
    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/items/{nums[0]}/snooze", json_body={"snoozed_until": until}))

    detail = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/items/{nums[0]}"))
    rec = detail["item"] if isinstance(detail, dict) and isinstance(detail.get("item"), dict) else detail
    assert rec.get("snoozed_until") is not None
    assert any(ev.get("kind") == "snoozed" for ev in detail["events"])

    inbox = orca_api.assert_ok(orca_api.get("/api/v1/orca/needs-you"))
    active = inbox["items"] if isinstance(inbox, dict) and "items" in inbox else inbox
    assert all(it["number"] != nums[0] for it in active)

    seen: list[int] = []
    for offset in (0, 2, 4):
        page = orca_api.assert_ok(orca_api.get(f"/api/v1/orca/bots/{bot_id}/items", params={"limit": 2, "offset": offset}))
        arr = page["items"] if isinstance(page, dict) and "items" in page else page
        assert len(arr) == 2
        seen.extend(it["number"] for it in arr)
    assert len(set(seen)) == 6 and set(seen) == set(nums)


@pytest.mark.tier1
@pytest.mark.f04
def test_t1_f04_cli_item_workflow_and_unified_rpc_parity(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    bot_id = _create_bot(orca_api, "CLI Items Bot", agent="claude")["id"]
    added = _cli_json_ok(orca_cli.run("bot", "item-add", "--bot", bot_id, "--title", "Ship v1.7"))
    item_rec = added["item"] if isinstance(added, dict) and isinstance(added.get("item"), dict) else added
    n1 = int(item_rec["number"])

    _cli_json_ok(
        orca_cli.run(
            "bot", "item-set", str(n1), "--status", "needs-you", "--comment", "Choose tag",
            "--ask", "Release channel?", "--option", "stable", "--option", "beta", "--recommended", "0",
        )
    )
    ny = _cli_json_ok(orca_cli.run("bot", "needs-you"))
    ny_items = ny["items"] if isinstance(ny, dict) and "items" in ny else ny
    assert any(it["number"] == n1 for it in ny_items)

    _cli_json_ok(orca_cli.run("bot", "item-link", str(n1), "--kind", "pr", "--ref", "acme/repo#7"))
    _cli_json_ok(orca_cli.run("bot", "item-comment", str(n1), "--text", "Go stable"))
    cli_show = _unwrap(_cli_json_ok(orca_cli.run("bot", "item-show", str(n1))), "item")
    assert int(cli_show["number"]) == n1
    n2 = _create_item(orca_api, bot_id, "Dup task")["number"]
    cli_list_after = _cli_json_ok(orca_cli.run("bot", "item-list", "--bot", bot_id))
    cli_items = cli_list_after["items"] if isinstance(cli_list_after, dict) and "items" in cli_list_after else cli_list_after
    cli_nums = {int(it["number"]) for it in cli_items}
    assert n1 in cli_nums and n2 in cli_nums
    _cli_json_ok(orca_cli.run("bot", "item-done", str(n1), "--note", "Tagged"))
    _cli_json_ok(orca_cli.run("bot", "item-cancel", str(n2), "--note", "Duplicate"))

    rpc_item = orca_api.assert_rpc_ok("bots.items.show", {"number": n1})
    rec = rpc_item["item"] if isinstance(rpc_item, dict) and isinstance(rpc_item.get("item"), dict) else rpc_item
    links = rpc_item.get("links", rec.get("links", []))
    assert rec["status"] == "done" and rec["close_note"] == "Tagged"
    assert any(lnk.get("kind") == "pr" and lnk.get("ref") == "acme/repo#7" for lnk in links)
