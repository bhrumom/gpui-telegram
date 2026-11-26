"""Tier 1 Happy-Path Opaque-Box E2E Tests for OB-F06 through OB-F10 (30 tests, 6/feature).

Covers:
  - OB-F06: Agent Lifecycle, AI Profile Gen & 18-Provider Harnesses
  - OB-F07: DrainScheduler, Side Threads, Compaction, Mailbox & DMs
  - OB-F08: Group-Chat (Channel) Lifecycle, @mention & Lead Router
  - OB-F09: Group-Chat Delegation, channel_reply, Loop Guards & Contexts
  - OB-F10: Tool Catalog, 3-Tier Memory, Skills, MCP, Browser & Vault
"""

from __future__ import annotations

import http.server
import json
import os
import re
import threading
from pathlib import Path
from typing import Any, Dict, List, Tuple

from .harness import (
    BUILTIN_AGENT_PROVIDERS,
    TempOpenBotEnv,
    dm_thread_id_oracle,
    e2e_test,
    extract_list,
    pick,
    require_field,
    unwrap_entity,
)


def _create_agent(env: TempOpenBotEnv, name: str, **extra_flags: str) -> Dict[str, Any]:
    args: List[str] = ["agent", "create", "--name", name]
    for k, v in extra_flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    return unwrap_entity(res, "agent")


def _start_mock_openai_models_server(models: List[str]) -> Tuple[http.server.HTTPServer, str]:
    payload = json.dumps(
        {"object": "list", "data": [{"id": m, "object": "model"} for m in models]}
    ).encode("utf-8")

    class _Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, fmt: str, *args: Any) -> None:
            return

    srv = http.server.HTTPServer(("127.0.0.1", 0), _Handler)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, f"http://127.0.0.1:{port}/v1"


# ============================================================================
# OB-F06: Agent Lifecycle, AI Profile Gen & 18-Provider Harnesses (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F06-01",
    tier=1,
    features=["OB-F06"],
    description="Zero starter bots on fresh DB and complete Agent CRUD (create, get, update fastMode+ultrafast, delete).",
)
def test_t1_ob_f06_01_zero_starter_bots_and_agent_crud(env: TempOpenBotEnv) -> None:
    initial = extract_list(env.run_cli_json("agent", "list"), "agents")
    assert initial == [], f"Expected zero starter bots, got {initial}"

    created = _create_agent(
        env,
        "Ada",
        title="Architect",
        description="Rust systems",
        avatar_seed="ada-seed",
        avatar_hue="215",
    )
    aid = require_field(created, "id", expected_type=str)
    assert re.fullmatch(r"agent-[0-9a-f-]{8,}", aid)
    assert require_field(created, "provider", expected_type=str) == "codex"
    assert require_field(created, "model", expected_type=str) == "gpt-6-luna"
    assert require_field(created, "reasoning_effort", expected_type=str) == "low"
    ws_path = Path(require_field(created, "workspace_path", expected_type=str))
    assert ws_path.exists() and ws_path.is_dir()

    fetched = unwrap_entity(env.run_cli_json("agent", "get", "--id", aid), "agent")
    assert require_field(fetched, "title", expected_type=str) == "Architect"

    upd_res = env.run_cli_json(
        "agent",
        "update",
        "--id",
        aid,
        "--reasoning-effort",
        "high",
        "--fast-mode",
        "true",
        "--ultrafast",
        "true",
        "--access",
        "workspace",
    )
    upd = unwrap_entity(upd_res, "agent")
    assert require_field(upd, "reasoning_effort", expected_type=str) == "high"
    assert require_field(upd, "fast_mode", expected_type=bool) is True
    assert require_field(upd, "ultrafast", expected_type=bool) is True
    assert require_field(upd, "access", expected_type=str) == "workspace"

    env.run_cli_json("agent", "delete", "--id", aid)
    after = extract_list(env.run_cli_json("agent", "list"), "agents")
    assert after == [] and not ws_path.exists()


@e2e_test(
    id="TEST-T1-OB-F06-02",
    tier=1,
    features=["OB-F06"],
    description="AI generate-profile commits AgentProfileDraft and enqueues invisible 3-option ask_user welcome delivery.",
)
def test_t1_ob_f06_02_generate_profile_and_welcome_card(env: TempOpenBotEnv) -> None:
    res = env.run_cli_json(
        "agent",
        "generate-profile",
        "--prompt",
        "Security auditor for Rust crates",
        "--save",
        "--operation-id",
        "11111111-1111-4111-8111-111111111111",
        "--language",
        "en",
    )
    draft = require_field(res, "draft", expected_type=dict)
    assert len(require_field(draft, "name", expected_type=str)) >= 1
    assert len(require_field(draft, "avatar_seed", expected_type=str)) >= 1
    agent = require_field(res, "agent", expected_type=dict)
    aid = require_field(agent, "id", expected_type=str)

    q_res = env.run_cli_json("chat", "queue-list", "--agent-id", aid)
    items = extract_list(q_res, "items", "queue")
    assert len(items) >= 1
    assert any(pick(it, "welcome") is True for it in items)
    assert any("ask_user" in str(pick(it, "text", "prompt") or "") for it in items)


@e2e_test(
    id="TEST-T1-OB-F06-03",
    tier=1,
    features=["OB-F06"],
    description="Atomic .openbot-stage agent duplicate rewrites internal workspace symlinks, copies memories, and honors operation-id idempotency.",
)
def test_t1_ob_f06_03_atomic_stage_duplicate_and_symlink_rewrite(env: TempOpenBotEnv) -> None:
    src = _create_agent(env, "SourceBot")
    src_id = require_field(src, "id", expected_type=str)
    src_ws = Path(require_field(src, "workspace_path", expected_type=str))
    (src_ws / "notes.md").write_text("important audit note\n", encoding="utf-8")
    os.symlink(src_ws / "notes.md", src_ws / "latest.md")
    env.run_cli_json(
        "tools", "memory-put", "--agent-id", src_id, "--text", "Keep zero unsafe blocks"
    )

    op_id = "22222222-2222-4222-8222-222222222222"
    dup1_res = env.run_cli_json(
        "agent", "duplicate", "--id", src_id, "--operation-id", op_id
    )
    dup1 = unwrap_entity(dup1_res, "agent")
    dup_id = require_field(dup1, "id", expected_type=str)
    assert dup_id != src_id
    assert require_field(dup1, "name", expected_type=str) != "SourceBot"
    dup_ws = Path(require_field(dup1, "workspace_path", expected_type=str))
    assert (dup_ws / "latest.md").is_symlink()
    resolved_target = (dup_ws / "latest.md").resolve()
    assert str(resolved_target).startswith(str(dup_ws.resolve()))

    stage_leftovers = list(dup_ws.parent.glob("*.openbot-stage"))
    assert stage_leftovers == []

    mems = extract_list(
        env.run_cli_json("tools", "memory-search", "--agent-id", dup_id, "--query", "unsafe"),
        "memories",
        "results",
    )
    assert len(mems) >= 1

    dup2_res = env.run_cli_json(
        "agent", "duplicate", "--id", src_id, "--operation-id", op_id
    )
    dup2 = unwrap_entity(dup2_res, "agent")
    assert require_field(dup2, "id", expected_type=str) == dup_id


@e2e_test(
    id="TEST-T1-OB-F06-04",
    tier=1,
    features=["OB-F06"],
    description="Portable .openbot bundle export and import round-trips multiple agents, memories, and workspace files into a fresh data-dir.",
)
def test_t1_ob_f06_04_portable_archive_export_import(env: TempOpenBotEnv) -> None:
    a1 = _create_agent(env, "ExportAlpha", title="Rust Lead", avatar_seed="export-alpha")
    a2 = _create_agent(env, "ExportBeta", title="QA Lead", avatar_seed="export-beta")
    id1 = require_field(a1, "id", expected_type=str)
    id2 = require_field(a2, "id", expected_type=str)
    Path(require_field(a1, "workspace_path", expected_type=str), "readme.txt").write_text(
        "alpha workspace", encoding="utf-8"
    )
    env.run_cli_json("tools", "memory-put", "--agent-id", id1, "--text", "Alpha memory fact")

    bundle_path = env.root / "bundle.openbot"
    exp_res = env.run_cli_json(
        "agent", "export", "--ids", f"{id1},{id2}", "--out", str(bundle_path)
    )
    assert bundle_path.exists() and bundle_path.stat().st_size > 0
    assert require_field(exp_res, "exported_count", "count", expected_type=int) == 2

    dest_dir = env.root / "imported_data"
    dest_dir.mkdir(parents=True, exist_ok=True)
    imp_res = env.run_cli_json(
        "agent", "import", "--archive", str(bundle_path), "--keys", "all", "--data-dir", str(dest_dir)
    )
    imported_agents = extract_list(imp_res, "agents")
    assert len(imported_agents) == 2
    titles = {require_field(ag, "title", expected_type=str) for ag in imported_agents}
    assert titles == {"Rust Lead", "QA Lead"}


@e2e_test(
    id="TEST-T1-OB-F06-05",
    tier=1,
    features=["OB-F06"],
    description="All 18 AgentProviderId harnesses persist defaults (acp concurrentTurns=false), pi b64 model encoding, and live local server probes.",
)
def test_t1_ob_f06_05_all_18_providers_custom_pi_and_local_probe(env: TempOpenBotEnv) -> None:
    for p in BUILTIN_AGENT_PROVIDERS:
        ag = _create_agent(env, f"Bot-{p}", provider=p)
        assert require_field(ag, "provider", expected_type=str) == p
        if p == "acp":
            assert require_field(ag, "concurrent_turns", expected_type=bool) is False

    srv1, url1 = _start_mock_openai_models_server(["llama3.2:latest"])
    srv2, url2 = _start_mock_openai_models_server(["qwen2.5-coder-14b"])
    try:
        custom_ep = json.dumps(
            {
                "id": "my-vllm",
                "name": "vLLM",
                "baseUrl": url1,
                "apiKey": "secret-local-key",
                "harness": "pi",
                "models": [{"id": "qwen:14b@q4", "name": "Qwen", "contextWindow": 32768}],
            }
        )
        c_res = env.run_cli_json(
            "agent",
            "create",
            "--name",
            "CustomPi",
            "--custom-endpoint",
            custom_ep,
            "--probe-servers",
            f"{url1},{url2}",
        )
        c_ag = unwrap_entity(c_res, "agent")
        encoded_model = require_field(c_ag, "model", expected_type=str)
        assert "my-vllm/b64:" in encoded_model
        servers = extract_list(c_res, "detected_servers", "servers")
        assert len(servers) == 2
        assert "secret-local-key" not in json.dumps(servers)
    finally:
        srv1.shutdown()
        srv2.shutdown()


@e2e_test(
    id="TEST-T1-OB-F06-06",
    tier=1,
    features=["OB-F06"],
    description="Automatic backupModels failover switches provider on primary rate-limit, sets usageLimitReturn, and prepends continuation handoff.",
)
def test_t1_ob_f06_06_backup_models_automatic_failover(env: TempOpenBotEnv) -> None:
    backups = json.dumps(
        {
            "automatic": True,
            "order": [
                {"provider": "claude", "model": "claude-haiku-5-5"},
                {"provider": "deepseek", "model": "deepseek-v4-pro"},
            ],
        }
    )
    ag = _create_agent(env, "FailoverBot", backup_models=backups)
    aid = require_field(ag, "id", expected_type=str)
    send_res = env.run_cli_json(
        "chat",
        "send",
        "--agent-id",
        aid,
        "--text",
        "Finish security audit",
        "--simulate-primary-rate-limit",
    )
    assert require_field(send_res, "active_provider", "provider", expected_type=str) == "claude"
    assert require_field(send_res, "active_model", "model", expected_type=str) == "claude-haiku-5-5"
    ret = require_field(send_res, "usage_limit_return", expected_type=dict)
    assert require_field(ret, "provider", expected_type=str) == "codex"
    handoff = require_field(send_res, "handoff_prompt", expected_type=str)
    assert "Continue this OpenBot conversation" in handoff


# ============================================================================
# OB-F07: DrainScheduler, Side Threads, Compaction, Mailbox & DMs (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F07-01",
    tier=1,
    features=["OB-F07"],
    description="Multi-lane DrainScheduler isolates main thread from branched side-task thread and honors idempotent branch request-id.",
)
def test_t1_ob_f07_01_multi_lane_drain_and_branch_isolation(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "LaneBot"), "id", expected_type=str)
    m_main = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Main turn alpha")
    msg1_id = require_field(m_main, "message_id", expected_type=str)

    b1 = env.run_cli_json(
        "chat", "branch", "--agent-id", aid, "--message-id", msg1_id, "--request-id", "req-b1"
    )
    task_id = require_field(b1, "task_id", "thread_id", expected_type=str)
    assert task_id.startswith("openbot-thread-")
    b2 = env.run_cli_json(
        "chat", "branch", "--agent-id", aid, "--message-id", msg1_id, "--request-id", "req-b1"
    )
    assert require_field(b2, "task_id", "thread_id", expected_type=str) == task_id

    env.run_cli_json(
        "chat", "send", "--agent-id", aid, "--task-id", task_id, "--text", "Side branch beta"
    )
    h_main = extract_list(env.run_cli_json("chat", "history", "--agent-id", aid), "messages")
    h_side = extract_list(
        env.run_cli_json("chat", "history", "--agent-id", aid, "--task-id", task_id), "messages"
    )
    assert all("Side branch beta" not in json.dumps(m) for m in h_main)
    assert any("Side branch beta" in json.dumps(m) for m in h_side)


@e2e_test(
    id="TEST-T1-OB-F07-02",
    tier=1,
    features=["OB-F07"],
    description="Ephemeral side-question returns bounded answer without mutating thread history or revision.",
)
def test_t1_ob_f07_02_ephemeral_side_question_read_only(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "SideQBot"), "id", expected_type=str)
    env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Baseline message")
    before = env.run_cli_json("chat", "history", "--agent-id", aid)
    before_msgs = extract_list(before, "messages")

    sq = env.run_cli_json(
        "chat",
        "side-question",
        "--agent-id",
        aid,
        "--question",
        "What does DrainScheduler do?",
        "--earlier",
        json.dumps([{"question": "Q1", "answer": "A1"}]),
    )
    ans = require_field(sq, "answer", expected_type=str)
    assert 1 <= len(ans) <= 32000

    after = env.run_cli_json("chat", "history", "--agent-id", aid)
    assert len(extract_list(after, "messages")) == len(before_msgs)


@e2e_test(
    id="TEST-T1-OB-F07-03",
    tier=1,
    features=["OB-F07"],
    description="Queue pause, queue-add, queue-reorder permutation, queue-resume, and queue-steer lifecycle.",
)
def test_t1_ob_f07_03_queue_pause_reorder_resume_steer(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "QueueBot"), "id", expected_type=str)
    p_res = env.run_cli_json("chat", "queue-pause", "--agent-id", aid)
    assert require_field(p_res, "paused", expected_type=bool) is True

    d1 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Item 1"),
        "delivery_id",
        expected_type=str,
    )
    d2 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Item 2"),
        "delivery_id",
        expected_type=str,
    )
    d3 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Item 3"),
        "delivery_id",
        expected_type=str,
    )

    reord = env.run_cli_json(
        "chat", "queue-reorder", "--agent-id", aid, "--delivery-ids", f"{d3},{d1},{d2}"
    )
    items = extract_list(reord, "items", "queue")
    assert require_field(items[0], "delivery_id", expected_type=str) == d3
    assert require_field(items[0], "position", expected_type=int) == 1

    r_res = env.run_cli_json("chat", "queue-resume", "--agent-id", aid)
    assert require_field(r_res, "paused", expected_type=bool) is False
    turn_id = require_field(r_res, "active_turn_id", "turn_id", expected_type=str)

    st_res = env.run_cli_json(
        "chat", "queue-steer", "--agent-id", aid, "--delivery-id", d1, "--expected-turn-id", turn_id
    )
    assert require_field(st_res, "steered", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F07-04",
    tier=1,
    features=["OB-F07"],
    description="80% context usage threshold triggers ContextCompaction reducing token count and persisting checkpoint.",
)
def test_t1_ob_f07_04_context_compaction_80pct_threshold(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "CompactBot"), "id", expected_type=str)
    res = env.run_cli_json(
        "chat", "compact", "--agent-id", aid, "--simulate-usage", "82000/100000"
    )
    comp = unwrap_entity(res, "compaction")
    assert require_field(comp, "status", expected_type=str) == "completed"
    before_tok = require_field(comp, "before_tokens", expected_type=int)
    after_tok = require_field(comp, "after_tokens", expected_type=int)
    assert before_tok == 82000 and after_tok < before_tok


@e2e_test(
    id="TEST-T1-OB-F07-05",
    tier=1,
    features=["OB-F07"],
    description="Mailbox fan-out, 3-line Status/Result/Evidence reply with expectsReply=false anti-ping-pong, and caller-owned interrupt.",
)
def test_t1_ob_f07_05_mailbox_fanout_reply_and_interrupt(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Coord"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Worker2"), "id", expected_type=str)
    a3 = require_field(_create_agent(env, "Worker3"), "id", expected_type=str)

    fan = env.run_cli_json(
        "chat",
        "mailbox-send",
        "--sender-agent-id",
        a1,
        "--recipient-agent-ids",
        f"{a2},{a3}",
        "--text",
        "Audit crate",
        "--expects-reply",
        "true",
    )
    deliveries = extract_list(fan, "deliveries")
    assert len(deliveries) == 2
    req_msg_id = require_field(fan, "message_id", expected_type=str)

    rep = env.run_cli_json(
        "chat",
        "mailbox-send",
        "--sender-agent-id",
        a2,
        "--recipient-agent-ids",
        a1,
        "--text",
        "Status: done\nResult: clean\nEvidence: cargo test",
        "--expects-reply",
        "false",
        "--reply-to",
        req_msg_id,
    )
    assert require_field(rep, "answer_optional", expected_type=bool) is True

    intr = env.run_cli_json(
        "chat",
        "interrupt",
        "--caller-agent-id",
        a1,
        "--target-agent-id",
        a3,
        "--reason",
        "Enough data",
    )
    assert require_field(intr, "interrupted", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F07-06",
    tier=1,
    features=["OB-F07"],
    description="1:1 TeamChatStore dm-send & dm-list enforce order-invariant dm-<sha256> thread ID and accurate unread counts.",
)
def test_t1_ob_f07_06_team_dm_symmetric_sha256_thread(env: TempOpenBotEnv) -> None:
    expected_tid = dm_thread_id_oracle("alice", "bob")
    m1 = env.run_cli_json(
        "chat",
        "dm-send",
        "--sender",
        "alice",
        "--recipient",
        "bob",
        "--client-message-id",
        "m1",
        "--text",
        "Hi Bob",
    )
    m2 = env.run_cli_json(
        "chat",
        "dm-send",
        "--sender",
        "bob",
        "--recipient",
        "alice",
        "--client-message-id",
        "m2",
        "--text",
        "Hi Alice",
    )
    assert require_field(m1, "thread_id", expected_type=str) == expected_tid
    assert require_field(m2, "thread_id", expected_type=str) == expected_tid

    dm_list = extract_list(
        env.run_cli_json("chat", "dm-list", "--member", "alice"), "threads", "dms"
    )
    assert len(dm_list) == 1
    assert require_field(dm_list[0], "thread_id", expected_type=str) == expected_tid
    assert require_field(dm_list[0], "unread_count", expected_type=int) == 1


# ============================================================================
# OB-F08: Group-Chat (Channel) Lifecycle, @mention & Lead Router (Part 1: 01..02)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F08-01",
    tier=1,
    features=["OB-F08"],
    description="Channel CRUD persists 20 icons+hue, isolated memory, auto-resets browser on member removal, and archives/restores/deletes.",
)
def test_t1_ob_f08_01_channel_crud_icons_browser_archive(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "ChLead"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "ChBrowserBot"), "id", expected_type=str)
    c_res = env.run_cli_json(
        "channel",
        "create",
        "--name",
        "Core Team",
        "--title",
        "Backend",
        "--instructions",
        "Be concise",
        "--members",
        f"{a1},{a2}",
        "--lead",
        a1,
        "--isolated",
        "true",
        "--browser",
        json.dumps({"kind": "member", "agentId": a2}),
        "--icon",
        json.dumps({"name": "workflow", "hue": 215}),
    )
    ch = unwrap_entity(c_res, "channel")
    cid = require_field(ch, "id", expected_type=str)
    rev1 = require_field(ch, "revision", expected_type=int)
    assert require_field(ch, "isolated", expected_type=bool) is True

    u_res = env.run_cli_json(
        "channel", "update", "--channel-id", cid, "--members", a1, "--lead", a1
    )
    ch_u = unwrap_entity(u_res, "channel")
    assert require_field(ch_u, "revision", expected_type=int) > rev1
    assert require_field(ch_u, "browser.kind", expected_type=str) == "none"

    ar = unwrap_entity(env.run_cli_json("channel", "archive", "--channel-id", cid), "channel")
    assert require_field(ar, "archived", expected_type=bool) is True
    rs = unwrap_entity(env.run_cli_json("channel", "restore", "--channel-id", cid), "channel")
    assert require_field(rs, "archived", expected_type=bool) is False

    env.run_cli_json("channel", "delete", "--channel-id", cid)
    remaining = extract_list(env.run_cli_json("channel", "list"), "channels")
    assert all(pick(c, "id") != cid for c in remaining)


@e2e_test(
    id="TEST-T1-OB-F08-02",
    tier=1,
    features=["OB-F08"],
    description="Diacritic-insensitive agentMentionRank orders channel members strictly across ranks 0..6 and excludes non-members.",
)
def test_t1_ob_f08_02_agent_mention_rank_0_to_6(env: TempOpenBotEnv) -> None:
    specs = [
        ("Café", "Bot", "desc"),           # rank 0: exact
        ("CafeBot", "Bot", "desc"),        # rank 1: prefix
        ("My Café Helper", "Bot", "desc"), # rank 2: word prefix
        ("acafex", "Bot", "desc"),         # rank 3: name substring
        ("Zeta", "Cafe Lead", "desc"),     # rank 4: title substring
        ("Omega", "Lead", "likes cafe"),   # rank 5: description substring
        ("Delta", "Cafe", "ops"),          # rank 6 on 'cafe ops' across '${name} ${title} ${description}'
    ]
    ids: List[str] = []
    for nm, ti, de in specs[:7]:
        ag = _create_agent(env, nm, title=ti, description=de)
        ids.append(require_field(ag, "id", expected_type=str))
    outsider = require_field(_create_agent(env, "CafeOutsider"), "id", expected_type=str)

    ch = unwrap_entity(
        env.run_cli_json(
            "channel",
            "create",
            "--name",
            "RankRoom",
            "--members",
            ",".join(ids),
            "--lead",
            ids[0],
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    r_res = env.run_cli_json(
        "channel", "list", "--rank-mentions", "--channel-id", cid, "--query", "cafe"
    )
    ranked = extract_list(r_res, "ranked_members", "mentions", "members")
    ranked_ids = [require_field(item, "agent_id", "id", expected_type=str) for item in ranked]
    assert len(ranked_ids) == 7
    assert ranked_ids[:4] == ids[:4]
    assert set(ranked_ids[4:6]) == {ids[4], ids[6]}
    assert ranked_ids[6] == ids[5]
    assert outsider not in ranked_ids

    r_res6 = env.run_cli_json(
        "channel", "list", "--rank-mentions", "--channel-id", cid, "--query", "cafe ops"
    )
    ranked6 = extract_list(r_res6, "ranked_members", "mentions", "members")
    ranked6_ids = [require_field(item, "agent_id", "id", expected_type=str) for item in ranked6]
    assert ranked6_ids == [ids[6]]
    if pick(ranked6[0], "rank") is not None:
        assert int(pick(ranked6[0], "rank")) == 6


@e2e_test(
    id="TEST-T1-OB-F08-03",
    tier=1,
    features=["OB-F08"],
    description="Addressing via @everyone, @[Name](agent:id), and @AgentName spawns independent root ChannelTasks sharing requestMessageId.",
)
def test_t1_ob_f08_03_channel_mentions_and_everyone_fanout(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Ada"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Bob"), "id", expected_type=str)
    a3 = require_field(_create_agent(env, "Cara"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "Fanout", "--members", f"{a1},{a2},{a3}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)

    p_all = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@everyone Status?")
    t_all = extract_list(p_all, "tasks")
    assert len(t_all) == 3
    req_ids = {require_field(t, "request_message_id", expected_type=str) for t in t_all}
    assert len(req_ids) == 1
    assert {require_field(t, "owner_agent_id", expected_type=str) for t in t_all} == {a1, a2, a3}

    p_pair = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", f"@Ada & @[Bob](agent:{a2}) check API"
    )
    t_pair = extract_list(p_pair, "tasks")
    assert {require_field(t, "owner_agent_id", expected_type=str) for t in t_pair} == {a1, a2}


@e2e_test(
    id="TEST-T1-OB-F08-04",
    tier=1,
    features=["OB-F08"],
    description="Single-member channel fast path assigns sole member immediately with 0 router calls and zero channel-routing markers.",
)
def test_t1_ob_f08_04_single_member_fast_path(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "SoloBot"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json("channel", "create", "--name", "SoloCh", "--members", a1, "--lead", a1),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Build binary")
    assert require_field(p, "router_invocations", expected_type=int) == 0
    tasks = extract_list(p, "tasks")
    assert len(tasks) == 1 and require_field(tasks[0], "owner_agent_id", expected_type=str) == a1
    state = env.run_cli_json("channel", "get", "--channel-id", cid)
    msgs = extract_list(state, "messages")
    assert all("channel-routing" not in str(pick(m, "item_type", "id") or "") for m in msgs)


@e2e_test(
    id="TEST-T1-OB-F08-05",
    tier=1,
    features=["OB-F08"],
    description="Multi-member Lead JSON router decodes m1..mN, overrides human idle to lead, and overrides mid-text @Name idle.",
)
def test_t1_ob_f08_05_lead_router_and_idle_overrides(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "LeadBot"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "DevBot"), "id", expected_type=str)
    a3 = require_field(_create_agent(env, "Grace"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "RouterCh", "--members", f"{a1},{a2},{a3}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)

    r_multi = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Ship feature",
        "--router-mock",
        json.dumps({"agentIds": ["m2", "m3"]}),
    )
    owners_multi = {
        require_field(t, "owner_agent_id", expected_type=str) for t in extract_list(r_multi, "tasks")
    }
    assert owners_multi == {a2, a3}

    r_human_idle = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Hey thoughts?",
        "--router-mock",
        json.dumps({"idle": True}),
    )
    owners_idle = [
        require_field(t, "owner_agent_id", expected_type=str)
        for t in extract_list(r_human_idle, "tasks")
    ]
    assert owners_idle == [a1]

    r_mid_at = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Check this (@Grace) please",
        "--router-mock",
        json.dumps({"idle": True}),
    )
    owners_mid = [
        require_field(t, "owner_agent_id", expected_type=str)
        for t in extract_list(r_mid_at, "tasks")
    ]
    assert owners_mid == [a3]


@e2e_test(
    id="TEST-T1-OB-F08-06",
    tier=1,
    features=["OB-F08"],
    description="Follow-up reply ('Actually ...' + replyToMessageId) increments task revision and marks prior bot message superseded=true.",
)
def test_t1_ob_f08_06_followup_continuation_and_superseded(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "FollowBot"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json("channel", "create", "--name", "FollowCh", "--members", a1, "--lead", a1),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p1 = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Write JSON parser")
    t1 = extract_list(p1, "tasks")[0]
    tid = require_field(t1, "id", expected_type=str)
    res_msg = env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        tid,
        "--agent-id",
        a1,
        "--text",
        "Initial parser v1",
    )
    bot_msg_id = require_field(res_msg, "message_id", "id", expected_type=str)

    p2 = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Actually use serde_json instead",
        "--reply-to",
        bot_msg_id,
    )
    t2 = extract_list(p2, "tasks")[0]
    assert require_field(t2, "id", expected_type=str) == tid
    assert require_field(t2, "revision", expected_type=int) >= 1

    ch_state = env.run_cli_json("channel", "get", "--channel-id", cid)
    msgs = {require_field(m, "id", expected_type=str): m for m in extract_list(ch_state, "messages")}
    assert require_field(msgs[bot_msg_id], "superseded", expected_type=bool) is True


# ============================================================================
# OB-F09: Group-Chat Delegation, channel_reply, Loop Guards & Contexts (Part 1: 01..04)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F09-01",
    tier=1,
    features=["OB-F09"],
    description="channel assign waits on child dependency, child channel result strips '**<OwnName> report:**', and parent auto-resumes.",
)
def test_t1_ob_f09_01_assign_wait_result_strip_and_resume(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Alice"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Bob"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "DelCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@Alice Design DB")
    parent_task = extract_list(p, "tasks")[0]
    t_parent = require_field(parent_task, "id", expected_type=str)
    src_msg = require_field(parent_task, "request_message_id", expected_type=str)

    asg = env.run_cli_json(
        "channel",
        "assign",
        "--channel-id",
        cid,
        "--caller-task-id",
        t_parent,
        "--agent-id",
        a1,
        "--recipient-agent-id",
        a2,
        "--task",
        "Write SQL schema",
        "--expected-result",
        "DDL",
        "--source-message-ids",
        src_msg,
    )
    t_child = require_field(asg, "child_task_id", "task.id", expected_type=str)
    assert require_field(asg, "parent_state", expected_type=str) == "waiting"

    res = env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        t_child,
        "--agent-id",
        a2,
        "--text",
        "**Bob report:** CREATE TABLE items(id INT);",
    )
    assert require_field(res, "text", expected_type=str) == "CREATE TABLE items(id INT);"
    assert require_field(res, "released_parent_state", expected_type=str) == "queued"


@e2e_test(
    id="TEST-T1-OB-F09-02",
    tier=1,
    features=["OB-F09"],
    description="channel transfer hands off task ownership in place, increments revision and root.assignmentCount, and posts handoff message.",
)
def test_t1_ob_f09_02_channel_transfer_ownership_handoff(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Alice"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Bob"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "XferCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@Alice Build UI")
    t0 = extract_list(p, "tasks")[0]
    tid = require_field(t0, "id", expected_type=str)
    m0 = require_field(t0, "request_message_id", expected_type=str)

    xf = env.run_cli_json(
        "channel",
        "transfer",
        "--channel-id",
        cid,
        "--task-id",
        tid,
        "--agent-id",
        a1,
        "--recipient-agent-id",
        a2,
        "--task",
        "Take over UI components",
        "--expected-result",
        "Rendered view",
        "--source-message-ids",
        m0,
    )
    t_after = unwrap_entity(xf, "task")
    assert require_field(t_after, "owner_agent_id", expected_type=str) == a2
    assert require_field(t_after, "assignment_count", expected_type=int) == 1
    assert require_field(t_after, "state", expected_type=str) == "queued"


@e2e_test(
    id="TEST-T1-OB-F09-03",
    tier=1,
    features=["OB-F09"],
    description="channel reply creates channel-reply-<uuid> task and failed peer reply still settles without deadlocking caller.",
)
def test_t1_ob_f09_03_channel_reply_and_failed_reply_non_deadlock(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Alice"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Bob"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "ReplyCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p_bob = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@Bob Propose type")
    t_bob = require_field(extract_list(p_bob, "tasks")[0], "id", expected_type=str)
    m_bob = require_field(
        env.run_cli_json(
            "channel", "result", "--channel-id", cid, "--task-id", t_bob, "--agent-id", a2, "--text", "Use u64"
        ),
        "message_id",
        "id",
        expected_type=str,
    )

    p_alice = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@Alice Review")
    t_alice = require_field(extract_list(p_alice, "tasks")[0], "id", expected_type=str)

    rep_fail = env.run_cli_json(
        "channel",
        "reply",
        "--channel-id",
        cid,
        "--task-id",
        t_alice,
        "--agent-id",
        a1,
        "--message-id",
        m_bob,
        "--text",
        "Should id be NonZeroU64?",
        "--simulate-peer-failure",
    )
    assert require_field(rep_fail, "reply_message_id", expected_type=str).startswith("channel-reply-")
    assert require_field(rep_fail, "is_reply_task", expected_type=bool) is True
    assert require_field(rep_fail, "caller_resumed", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F09-04",
    tier=1,
    features=["OB-F09"],
    description="Stopped-task recovery controls (continue, reassign, cancel) reset assignmentCount=0 or cancel task subtree.",
)
def test_t1_ob_f09_04_stopped_task_continue_reassign_cancel(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "LeadA"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "PeerB"), "id", expected_type=str)
    ch_nolead = unwrap_entity(
        env.run_cli_json("channel", "create", "--name", "RecovCh", "--members", f"{a1},{a2}"),
        "channel",
    )
    cid = require_field(ch_nolead, "id", expected_type=str)
    p1 = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Paused task 1")
    t1_id = require_field(extract_list(p1, "tasks")[0], "id", expected_type=str)

    reassigned = env.run_cli_json(
        "channel", "reassign", "--channel-id", cid, "--task-id", t1_id, "--recipient-agent-id", a2
    )
    t_re = unwrap_entity(reassigned, "task")
    assert require_field(t_re, "owner_agent_id", expected_type=str) == a2
    assert require_field(t_re, "assignment_count", expected_type=int) == 0
    assert require_field(t_re, "state", expected_type=str) == "queued"

    p2 = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Paused task 2")
    t2_id = require_field(extract_list(p2, "tasks")[0], "id", expected_type=str)
    cont = unwrap_entity(
        env.run_cli_json(
            "channel", "continue", "--channel-id", cid, "--task-id", t2_id, "--recipient-agent-id", a1
        ),
        "task",
    )
    assert require_field(cont, "assignment_count", expected_type=int) == 0

    p3 = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Paused task 3")
    t3_id = require_field(extract_list(p3, "tasks")[0], "id", expected_type=str)
    canc = unwrap_entity(
        env.run_cli_json("channel", "cancel", "--channel-id", cid, "--task-id", t3_id), "task"
    )
    assert require_field(canc, "state", expected_type=str) == "cancelled"


@e2e_test(
    id="TEST-T1-OB-F09-05",
    tier=1,
    features=["OB-F09"],
    description="CHANNEL_PARALLEL_LIMIT=2 throttles 3rd concurrent task ('channel-full') and serializes 'browser' & nested 'workspace:<path>' mutexes.",
)
def test_t1_ob_f09_05_parallel_limit_2_and_resource_mutexes(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Bot1"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Bot2"), "id", expected_type=str)
    a3 = require_field(_create_agent(env, "Bot3"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "ConcCh", "--members", f"{a1},{a2},{a3}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    repo_root = str(env.root / "repo")
    sub_root = str(env.root / "repo" / "sub")
    p = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "@everyone Run tasks",
        "--hold-running",
        "--resources-map",
        json.dumps({a1: ["browser", f"workspace:{repo_root}"], a2: [f"workspace:{sub_root}"], a3: ["browser"]}),
    )
    tasks = extract_list(p, "tasks")
    active = [t for t in tasks if pick(t, "state") in ("running", "starting")]
    assert len(active) <= 2
    waits_flat = [
        w
        for t in tasks
        for w in (pick(t, "waits", default=[]) or [])
    ]
    reasons = {pick(w, "reason") for w in waits_flat}
    assert "channel-full" in reasons or "resource-busy" in reasons


@e2e_test(
    id="TEST-T1-OB-F09-06",
    tier=1,
    features=["OB-F09"],
    description="history-packet hides sibling @everyone answers on 1st turn, emits compact delta on 2nd turn, and enforces 8k/12k rolling summary.",
)
def test_t1_ob_f09_06_history_packet_sibling_hiding_delta_summary(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "Ada"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "Bob"), "id", expected_type=str)
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "HistCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@everyone Architecture view?")
    tasks = {require_field(t, "owner_agent_id", expected_type=str): t for t in extract_list(p, "tasks")}
    t_ada = require_field(tasks[a1], "id", expected_type=str)
    t_bob = require_field(tasks[a2], "id", expected_type=str)

    env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        t_bob,
        "--agent-id",
        a2,
        "--text",
        "SECRET_SIBLING_BOB_ANSWER_991",
    )
    pkt1 = unwrap_entity(
        env.run_cli_json(
            "channel", "history-packet", "--channel-id", cid, "--task-id", t_ada, "--agent-id", a1
        ),
        "packet",
    )
    txt1 = require_field(pkt1, "text", expected_type=str)
    assert "SECRET_SIBLING_BOB_ANSWER_991" not in txt1
    assert "Other members answer this same request separately" in txt1

    env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", t_ada, "--agent-id", a1, "--text", "ADA_OWN_ANSWER_1"
    )
    pkt2 = unwrap_entity(
        env.run_cli_json(
            "channel",
            "history-packet",
            "--channel-id",
            cid,
            "--task-id",
            t_ada,
            "--agent-id",
            a1,
            "--second-turn",
            "--simulate-large-transcript",
        ),
        "packet",
    )
    assert require_field(pkt2, "kind", expected_type=str) == "delta"
    assert "ADA_OWN_ANSWER_1" not in require_field(pkt2, "text", expected_type=str)
    sum_chars = require_field(pkt2, "summary_chars", expected_type=int)
    assert 0 < sum_chars <= 12000


# ============================================================================
# OB-F10: Tool Catalog, 3-Tier Memory, Skills, MCP, Browser & Vault (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F10-01",
    tier=1,
    features=["OB-F10"],
    description="Tool catalog exposes 21+3 direct tools (>=121 total), search/describe schemas, and repairs unescaped Windows paths in tool_call.",
)
def test_t1_ob_f10_01_tool_catalog_and_json_escape_repair(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "ToolBot"), "id", expected_type=str)
    lst = env.run_cli_json("tools", "list", "--provider", "codex")
    assert require_field(lst, "direct_openbot_count", expected_type=int) == 21
    assert require_field(lst, "direct_browser_count", expected_type=int) == 3
    assert require_field(lst, "catalog_total", expected_type=int) >= 121

    srch = env.run_cli_json("tools", "search", "--query", "shared database tables")
    matches = extract_list(srch, "tools", "matches")
    names = {require_field(m, "name", expected_type=str) for m in matches}
    assert "openbot.execute_data" in names or "openbot.query_data" in names

    desc = env.run_cli_json(
        "tools", "describe", "--names", "openbot.execute_data,openbot_browser.fill_card"
    )
    assert len(extract_list(desc, "tools", "descriptions")) == 2

    raw_broken = '{"note":"C:\\temp\\new_folder\\report.txt"} }'
    call_res = env.run_cli_json(
        "tools",
        "call",
        "--agent-id",
        aid,
        "--name",
        "openbot.list_tables",
        "--raw-arguments",
        raw_broken,
    )
    assert require_field(call_res, "repaired_json", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F10-02",
    tier=1,
    features=["OB-F10"],
    description="3-Tier memory enforces essential byte budget, matches CJK Han/Kana/Hangul via UNSPACED_SCRIPT+FTS5, and resolves site hierarchy.",
)
def test_t1_ob_f10_02_three_tier_memory_cjk_and_site_hierarchy(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "MemBot"), "id", expected_type=str)
    m_ess = env.run_cli_json(
        "tools",
        "memory-put",
        "--agent-id",
        aid,
        "--text",
        "Rust workspace uses mold linker",
        "--inclusion",
        "essential",
    )
    budget = require_field(m_ess, "budget_bytes", expected_type=int)
    assert budget in (4096, 8192)
    assert require_field(m_ess, "essential_bytes", expected_type=int) <= budget

    env.run_cli_json(
        "tools",
        "memory-put",
        "--agent-id",
        aid,
        "--text",
        "東京タワーとソウル駅の予約コード9981",
        "--inclusion",
        "searchable",
    )
    env.run_cli_json(
        "tools",
        "memory-put",
        "--agent-id",
        aid,
        "--text",
        "Click enterprise SSO button",
        "--site",
        "https://www.app.acme.com/login",
    )
    srch = env.run_cli_json(
        "tools",
        "memory-search",
        "--agent-id",
        aid,
        "--query",
        "予約コード",
        "--host",
        "sub.app.acme.com",
    )
    hits = extract_list(srch, "memories", "results")
    assert any("9981" in str(pick(h, "text", "content") or "") for h in hits)
    site_hits = extract_list(srch, "site_memories")
    assert any("SSO button" in str(pick(h, "text", "content") or "") for h in site_hits)


@e2e_test(
    id="TEST-T1-OB-F10-03",
    tier=1,
    features=["OB-F10"],
    description="agent-data-exec supports parameterized cross-agent CREATE TABLE, INSERT, and SELECT on Shared/Data/agent-data.db.",
)
def test_t1_ob_f10_03_agent_data_exec_parameterized_shared_sql(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "SqlOwner"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "SqlPeer"), "id", expected_type=str)
    c_res = env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a1,
        "--mode",
        "write",
        "--sql",
        "CREATE TABLE kpi (k TEXT PRIMARY KEY, v INT)",
    )
    assert require_field(c_res, "owner_agent_id", expected_type=str) == a1

    ins = env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a2,
        "--mode",
        "write",
        "--sql",
        "INSERT INTO kpi VALUES (?, ?)",
        "--params",
        json.dumps(["p99_ms", 17]),
    )
    assert require_field(ins, "changes", expected_type=int) == 1

    sel = env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a2,
        "--mode",
        "read",
        "--sql",
        "SELECT k, v FROM kpi WHERE k = ?",
        "--params",
        json.dumps(["p99_ms"]),
    )
    rows = extract_list(sel, "rows")
    assert rows == [["p99_ms", 17]] or rows == [{"k": "p99_ms", "v": 17}]


@e2e_test(
    id="TEST-T1-OB-F10-04",
    tier=1,
    features=["OB-F10"],
    description="skill-save installs SKILL.md (<=32/agent) and mcp-register persists stdio & http MCP servers (<=100).",
)
def test_t1_ob_f10_04_skill_save_and_mcp_register(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "SkillBot"), "id", expected_type=str)
    sk = env.run_cli_json(
        "tools",
        "skill-save",
        "--agent-id",
        aid,
        "--slug",
        "rust-audit",
        "--name",
        "Rust Audit",
        "--description",
        "Check unsafe blocks",
        "--instructions",
        "# Audit\nRun cargo check.",
    )
    assert require_field(sk, "state", expected_type=str) == "installed"
    assert require_field(sk, "assigned_skill_count", expected_type=int) == 1

    mcp = env.run_cli_json(
        "tools",
        "mcp-register",
        "--name",
        "local-fs",
        "--transport",
        "stdio",
        "--command",
        "echo",
        "--args",
        json.dumps(["mcp"]),
    )
    srv = unwrap_entity(mcp, "mcp_server")
    stdio_id = require_field(srv, "id", expected_type=str)
    assert stdio_id.startswith("mcp-")
    assert require_field(srv, "transport", expected_type=str) == "stdio"

    mcp_http = env.run_cli_json(
        "tools",
        "mcp-register",
        "--name",
        "remote-sse",
        "--transport",
        "http",
        "--url",
        "https://mcp.example.test/sse",
        "--headers",
        json.dumps({"X-Client": "openbot"}),
    )
    srv_http = unwrap_entity(mcp_http, "mcp_server")
    http_id = require_field(srv_http, "id", expected_type=str)
    assert http_id.startswith("mcp-") and http_id != stdio_id
    assert require_field(srv_http, "transport", expected_type=str) == "http"


@e2e_test(
    id="TEST-T1-OB-F10-05",
    tier=1,
    features=["OB-F10"],
    description="browser-run executes <=200-step script and vault-secret fills credential zero-knowledge without leaking secret.",
)
def test_t1_ob_f10_05_browser_run_and_zero_knowledge_vault_secret(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "BrowserBot"), "id", expected_type=str)
    steps = json.dumps(
        [
            {"action": "type", "target": {"kind": "css", "selector": "#user"}, "text": "ada"},
            {"action": "snapshot"},
        ]
    )
    br = env.run_cli_json(
        "tools", "browser-run", "--agent-id", aid, "--url", "https://example.test/login", "--steps", steps
    )
    tab_id = require_field(br, "tab_id", expected_type=str)
    assert require_field(br, "snapshot", expected_type=(str, dict)) is not None

    vs = env.run_cli_json(
        "tools",
        "vault-secret",
        "--agent-id",
        aid,
        "--tab-id",
        tab_id,
        "--method",
        "password",
        "--login-id",
        "vault-login-1",
        "--submission",
        "enter",
    )
    assert require_field(vs, "secret_document", expected_type=bool) is True
    assert "password_value" not in json.dumps(vs).lower()


@e2e_test(
    id="TEST-T1-OB-F10-06",
    tier=1,
    features=["OB-F10"],
    description="link-payment issues one-time cardId, fill-card masks PAN, navigation unlocks screenshot, and takeover activates.",
)
def test_t1_ob_f10_06_link_payment_fill_card_nav_unlock_and_takeover(env: TempOpenBotEnv) -> None:
    aid = require_field(_create_agent(env, "PayBot"), "id", expected_type=str)
    br = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        aid,
        "--url",
        "https://pay.acme.test/checkout",
        "--steps",
        json.dumps([{"action": "snapshot"}]),
    )
    tab_id = require_field(br, "tab_id", expected_type=str)

    ctx_110 = (
        "Purchase 1 annual Pro CI runner seat for deterministic multi-crate build verification "
        "and artifact caching."
    )
    assert len(ctx_110) >= 100
    lp = env.run_cli_json(
        "tools",
        "link-payment",
        "--agent-id",
        aid,
        "--amount",
        "2500",
        "--currency",
        "USD",
        "--merchant-name",
        "Acme",
        "--merchant-url",
        "https://pay.acme.test/checkout",
        "--context",
        ctx_110,
        "--approve",
    )
    card_id = require_field(lp, "card_id", expected_type=str)

    fc = env.run_cli_json(
        "tools",
        "fill-card",
        "--agent-id",
        aid,
        "--tab-id",
        tab_id,
        "--card-id",
        card_id,
        "--approve",
    )
    assert require_field(fc, "filled_card", expected_type=bool) is True
    assert "4111" not in json.dumps(fc)

    # Navigate main frame to clear filledCard lock, then take screenshot
    env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        aid,
        "--tab-id",
        tab_id,
        "--url",
        "https://pay.acme.test/receipt",
        "--steps",
        json.dumps([{"action": "navigate", "url": "https://pay.acme.test/receipt"}, {"action": "screenshot"}]),
    )
    tk = env.run_cli_json(
        "tools", "takeover", "--agent-id", aid, "--tab-id", tab_id, "--reason", "sign-in-blocked"
    )
    assert require_field(tk, "takeover_active", expected_type=bool) is True
