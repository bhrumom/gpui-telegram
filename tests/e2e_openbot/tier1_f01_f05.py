"""Tier 1 Happy-Path Opaque-Box E2E Tests for OB-F01 through OB-F05 (30 tests, 6/feature).

Covers:
  - OB-F01: Contracts, Schemas & 32-bit UTF-16 FNV-1a Seed Grammar
  - OB-F02: Event-Sourced SQLite openbot.db (v69) & Supervised Shared agent-data.db
  - OB-F03: 2D Parametric BloubBot Avatar Engine & Motion
  - OB-F04: 3D Felt PlushBot, Moods, Clusters, Image Normalizer & CrewPile
  - OB-F05: OpenBot Theme Tokens, Live JSON Watcher, OKLab & Pixel-Art Backdrops
"""

from __future__ import annotations

import json
import math
import re
from typing import Any, Dict, List

from .harness import (
    AVATAR_EXPRESSIONS,
    AVATAR_MOODS,
    AVATAR_STATES,
    BUILTIN_AGENT_PROVIDERS,
    CHAT_BACKDROP_SCENES,
    TempOpenBotEnv,
    blink_open_squash_oracle,
    channel_cluster_layout_oracle,
    e2e_test,
    extract_list,
    nearest_avatar_hue_oracle,
    oklab_readable_chat_bg_oracle,
    pick,
    plush_srgb_tint_pixel_oracle,
    require_field,
    resolve_bloub_avatar_profile_oracle,
    unwrap_entity,
)


def _extract_profile(data: Any) -> Dict[str, Any]:
    prof = pick(data, "profile")
    src = prof if isinstance(prof, dict) else data
    return {
        "style": require_field(src, "style", expected_type=str),
        "shape": require_field(src, "shape", expected_type=str),
        "expression": require_field(src, "expression", expected_type=str),
        "color": require_field(src, "color", expected_type=str).lower(),
    }


# ============================================================================
# OB-F01: Contracts, Schemas & 32-Bit FNV-1a Seed Grammar (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F01-01",
    tier=1,
    features=["OB-F01"],
    description="Deterministic 32-bit UTF-16 FNV-1a bloubAvatarProfile resolution for standard seed 'agent-1'.",
)
def test_t1_ob_f01_01_fnv1a_standard_seed(env: TempOpenBotEnv) -> None:
    data = env.run_cli_json("avatar-eval", "--seed", "agent-1")
    expected = resolve_bloub_avatar_profile_oracle("agent-1")
    got = _extract_profile(data)
    assert got == expected, f"Expected {expected}, got {got}"
    assert got["shape"] == "pentagone" and got["expression"] == "fier"
    assert got["color"] == "#f08a24" and got["style"] == "plush"


@e2e_test(
    id="TEST-T1-OB-F01-02",
    tier=1,
    features=["OB-F01"],
    description="Explicit AvatarHue (--hue 100 Lime) overrides automatic FNV-1a color while preserving shape and expression.",
)
def test_t1_ob_f01_02_hue_override_precedence(env: TempOpenBotEnv) -> None:
    data = env.run_cli_json("avatar-eval", "--seed", "bot-alpha", "--hue", "100")
    expected = resolve_bloub_avatar_profile_oracle("bot-alpha", hue=100)
    got = _extract_profile(data)
    assert got == expected
    assert got["shape"] == "triangle" and got["expression"] == "attentif"
    assert got["color"] == "#8ece34"


@e2e_test(
    id="TEST-T1-OB-F01-03",
    tier=1,
    features=["OB-F01"],
    description="Explicit #rrggbb color overrides --hue and computes exact nearestAvatarHue (320 for #f27ba5).",
)
def test_t1_ob_f01_03_explicit_color_and_nearest_hue(env: TempOpenBotEnv) -> None:
    data = env.run_cli_json(
        "avatar-eval", "--seed", "bot-alpha", "--hue", "245", "--color", "#f27ba5"
    )
    got = _extract_profile(data)
    assert got["color"] == "#f27ba5"
    assert got["shape"] == "triangle" and got["expression"] == "attentif"
    nearest_hue = require_field(data, "nearest_hue", expected_type=int)
    assert nearest_hue == nearest_avatar_hue_oracle("#f27ba5") == 320


@e2e_test(
    id="TEST-T1-OB-F01-04",
    tier=1,
    features=["OB-F01"],
    description="Named silhouette ':poulpe:face:70' and flat style suffix ':flat:23' resolve exact upstream profiles.",
)
def test_t1_ob_f01_04_named_silhouette_and_flat_suffix(env: TempOpenBotEnv) -> None:
    d_named = env.run_cli_json(
        "avatar-eval", "--seed", "bot-alpha:poulpe:face:70", "--hue", "185"
    )
    p_named = _extract_profile(d_named)
    assert p_named == {
        "style": "plush",
        "shape": "poulpe",
        "expression": "heureux",
        "color": "#2fbfa0",
    }

    d_flat = env.run_cli_json("avatar-eval", "--seed", "bot-alpha:flat:23")
    p_flat = _extract_profile(d_flat)
    assert p_flat == {
        "style": "flat",
        "shape": "triangle",
        "expression": "curieux",
        "color": "#e8483f",
    }


@e2e_test(
    id="TEST-T1-OB-F01-05",
    tier=1,
    features=["OB-F01"],
    description="AgentSummary contract persists reasoningEffort, workspace access, backupModels <= 8, and zero temperature field.",
)
def test_t1_ob_f01_05_agent_summary_contract(env: TempOpenBotEnv) -> None:
    backups = json.dumps(
        [
            {"provider": "claude", "model": "claude-haiku-5-5"},
            {"provider": "grok", "model": "grok-4.7"},
        ]
    )
    res = env.run_cli_json(
        "agent",
        "create",
        "--name",
        "CodexLead",
        "--provider",
        "codex",
        "--model",
        "gpt-6-luna",
        "--reasoning-effort",
        "high",
        "--access",
        "workspace",
        "--backup-models",
        backups,
    )
    agent = unwrap_entity(res, "agent")
    assert require_field(agent, "provider", expected_type=str) == "codex"
    assert require_field(agent, "reasoning_effort", expected_type=str) == "high"
    assert require_field(agent, "access", expected_type=str) == "workspace"
    assert "temperature" not in agent, f"Unexpected temperature field in {agent}"
    bm = require_field(agent, "backup_models")
    order = bm if isinstance(bm, list) else require_field(bm, "order", expected_type=list)
    assert len(order) == 2


@e2e_test(
    id="TEST-T1-OB-F01-06",
    tier=1,
    features=["OB-F01"],
    description="Live /v1/capabilities endpoint advertises all 18 AgentProviderIds, Team API v1..v10, and 220+ capabilities.",
)
def test_t1_ob_f01_06_live_capabilities_contract(env: TempOpenBotEnv) -> None:
    srv = env.start_server()
    status, body, _ = srv.http_team("GET", "/v1/capabilities")
    assert status == 200
    providers = extract_list(body, "providers", "agent_providers")
    assert set(BUILTIN_AGENT_PROVIDERS).issubset(set(providers))
    versions = extract_list(body, "api_versions", "versions")
    for v in (f"v{i}" for i in range(1, 11)):
        assert v in versions, f"Missing Team API version {v} in {versions}"
    caps = extract_list(body, "capabilities")
    assert len(caps) >= 220


# ============================================================================
# OB-F02: Event-Sourced SQLite openbot.db (v69) & agent-data.db (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F02-01",
    tier=1,
    features=["OB-F02"],
    description="Fresh openbot.db initializes WAL mode, contiguous schema_migrations up to v69, >= 68 tables, and passes quick_check.",
)
def test_t1_ob_f02_01_openbot_db_v69_wal_bootstrap(env: TempOpenBotEnv) -> None:
    res = env.run_cli_json("agent", "list")
    agents = extract_list(res, "agents")
    assert agents == []
    assert env.openbot_db_path.exists()
    env.check_sqlite_integrity("openbot")
    jm = env.query_sqlite("PRAGMA journal_mode")[0]
    assert str(next(iter(jm.values()))).lower() == "wal"
    max_ver = env.query_sqlite("SELECT MAX(version) AS mv FROM schema_migrations")[0]["mv"]
    assert max_ver == 69
    tbl_rows = env.query_sqlite(
        "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'"
    )
    assert len(tbl_rows) >= 68


@e2e_test(
    id="TEST-T1-OB-F02-02",
    tier=1,
    features=["OB-F02"],
    description="CQRS dispatch is idempotent on repeated --command-id via orchestration_command_receipts without duplicating agents.",
)
def test_t1_ob_f02_02_cqrs_command_idempotency(env: TempOpenBotEnv) -> None:
    r1 = env.run_cli_json(
        "agent", "create", "--command-id", "cmd-idem-001", "--name", "AlphaBot"
    )
    r2 = env.run_cli_json(
        "agent", "create", "--command-id", "cmd-idem-001", "--name", "AlphaBot"
    )
    a1 = unwrap_entity(r1, "agent")
    a2 = unwrap_entity(r2, "agent")
    aid = require_field(a1, "id", expected_type=str)
    assert aid == require_field(a2, "id", expected_type=str)
    receipts = env.query_sqlite(
        "SELECT command_id, result_json FROM orchestration_command_receipts WHERE command_id = ?",
        ("cmd-idem-001",),
    )
    assert len(receipts) == 1
    proj_agents = env.query_sqlite("SELECT * FROM projection_agents")
    assert len(proj_agents) == 1
    assert (proj_agents[0].get("agent_id") or proj_agents[0].get("id")) == aid


@e2e_test(
    id="TEST-T1-OB-F02-03",
    tier=1,
    features=["OB-F02"],
    description="Channel creation appends valid JSON orchestration_events and projects projection_channels & projection_channel_contexts.",
)
def test_t1_ob_f02_03_channel_event_projection(env: TempOpenBotEnv) -> None:
    a1 = require_field(
        unwrap_entity(env.run_cli_json("agent", "create", "--name", "LeadA"), "agent"),
        "id",
        expected_type=str,
    )
    a2 = require_field(
        unwrap_entity(env.run_cli_json("agent", "create", "--name", "PeerB"), "agent"),
        "id",
        expected_type=str,
    )
    ch_res = env.run_cli_json(
        "channel",
        "create",
        "--name",
        "CoreTeam",
        "--members",
        f"{a1},{a2}",
        "--lead",
        a1,
    )
    ch = unwrap_entity(ch_res, "channel")
    ch_id = require_field(ch, "id", expected_type=str)
    ev_rows = env.query_sqlite(
        "SELECT event_type, json_valid(payload_json) AS jv FROM orchestration_events WHERE aggregate_id = ?",
        (ch_id,),
    )
    assert len(ev_rows) >= 1 and all(r["jv"] == 1 for r in ev_rows)
    all_ch_rows = env.query_sqlite("SELECT * FROM projection_channels")
    ch_rows = [r for r in all_ch_rows if (r.get("channel_id") or r.get("id")) == ch_id]
    assert len(ch_rows) == 1
    ctx_rows = env.query_sqlite(
        "SELECT agent_id FROM projection_channel_contexts WHERE channel_id = ?", (ch_id,)
    )
    assert {r["agent_id"] for r in ctx_rows} == {a1, a2}


@e2e_test(
    id="TEST-T1-OB-F02-04",
    tier=1,
    features=["OB-F02"],
    description="Agent memory writes index into SQLite FTS5 agent_memory_search and rank via bm25.",
)
def test_t1_ob_f02_04_fts5_memory_projection(env: TempOpenBotEnv) -> None:
    a_res = env.run_cli_json("agent", "create", "--name", "MemAgent")
    aid = require_field(unwrap_entity(a_res, "agent"), "id", expected_type=str)
    env.run_cli_json(
        "tools",
        "memory-put",
        "--agent-id",
        aid,
        "--text",
        "Rust borrow checker lifetime elision rule",
        "--inclusion",
        "searchable",
    )
    s_res = env.run_cli_json(
        "tools", "memory-search", "--agent-id", aid, "--query", "borrow"
    )
    memories = extract_list(s_res, "memories", "results")
    assert len(memories) >= 1
    assert any("borrow checker" in str(pick(m, "text", "content") or "") for m in memories)
    fts_rows = env.query_sqlite("SELECT count(*) AS cnt FROM agent_memory_search")
    assert fts_rows[0]["cnt"] >= 1


@e2e_test(
    id="TEST-T1-OB-F02-05",
    tier=1,
    features=["OB-F02"],
    description="Supervised Shared/Data/agent-data.db records table owner_agent_id (user_version=1) and permits peer DML.",
)
def test_t1_ob_f02_05_shared_agent_data_db_ownership_and_dml(env: TempOpenBotEnv) -> None:
    a1 = require_field(
        unwrap_entity(env.run_cli_json("agent", "create", "--name", "OwnerA"), "agent"),
        "id",
        expected_type=str,
    )
    a2 = require_field(
        unwrap_entity(env.run_cli_json("agent", "create", "--name", "PeerB"), "agent"),
        "id",
        expected_type=str,
    )
    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a1,
        "--mode",
        "write",
        "--sql",
        "CREATE TABLE metrics (k TEXT PRIMARY KEY, v INT)",
    )
    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a2,
        "--mode",
        "write",
        "--sql",
        "INSERT INTO metrics VALUES ('lat', 42)",
    )
    q_res = env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a2,
        "--mode",
        "read",
        "--sql",
        "SELECT k, v FROM metrics",
    )
    rows = extract_list(q_res, "rows")
    assert len(rows) == 1 and ("lat" in str(rows[0]) and "42" in str(rows[0]))
    uv = env.query_sqlite("PRAGMA user_version", db_kind="agent_data")[0]
    assert int(next(iter(uv.values()))) == 1
    meta_tbls = env.query_sqlite(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('openbot_metadata', '_openbot_tables')",
        db_kind="agent_data",
    )
    assert len(meta_tbls) >= 1
    meta_name = meta_tbls[0]["name"]
    owner_rows = env.query_sqlite(
        f"SELECT owner_agent_id FROM {meta_name} WHERE table_name = 'metrics'",
        db_kind="agent_data",
    )
    assert len(owner_rows) == 1 and owner_rows[0]["owner_agent_id"] == a1


@e2e_test(
    id="TEST-T1-OB-F02-06",
    tier=1,
    features=["OB-F02"],
    description="Atomic openbot-*-v1.json preference stores persist valid JSON with zero leftover .tmp files.",
)
def test_t1_ob_f02_06_atomic_json_preference_stores(env: TempOpenBotEnv) -> None:
    env.run_cli_json("theme-eval", "--mode", "light", "--persist")
    env.run_cli_json("ui-probe", "--scenario", "openbot-shell")
    json_stores = list(env.data_dir.glob("openbot-*.json")) + list(
        env.workspace_dir.glob("openbot-*.json")
    )
    assert len(json_stores) >= 1, f"Expected openbot-*-v1.json store in {env.data_dir}"
    for p in json_stores:
        parsed = json.loads(p.read_text(encoding="utf-8"))
        assert isinstance(parsed, dict)
    tmp_leftovers = list(env.data_dir.glob("*.tmp")) + list(env.workspace_dir.glob("*.tmp"))
    assert tmp_leftovers == [], f"Found uncleaned atomic .tmp files: {tmp_leftovers}"


# ============================================================================
# OB-F03: 2D Parametric BloubBot Avatar Engine & Motion (Part 1: 01..02)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F03-01",
    tier=1,
    features=["OB-F03"],
    description="Flat BloubBot emits viewBox '-158 -158 316 316', 64-ray Catmull-Rom cubic Bezier closed path, #f9f9f9 paper, and DstOut eyes.",
)
def test_t1_ob_f03_01_flat_64ray_bezier_geometry(env: TempOpenBotEnv) -> None:
    data = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--t", "0"
    )
    assert require_field(data, "view_box", expected_type=str) == "-158 -158 316 316"
    assert require_field(data, "paper", expected_type=str).lower() == "#f9f9f9"
    body_path = require_field(data, "body_path", "svg_path", expected_type=str).strip()
    assert body_path.startswith("M") and body_path.endswith("Z")
    c_count = len(re.findall(r"C", body_path))
    assert c_count == 64, f"Expected 64 cubic Bezier segments, got {c_count}"
    left_op = pick(data, "eyes.left.cutout_op", "cutout_op")
    assert left_op is None or str(left_op) == "DstOut"


@e2e_test(
    id="TEST-T1-OB-F03-02",
    tier=1,
    features=["OB-F03"],
    description="All 16 BloubExpression presets evaluate cleanly with exact somnolent rest_open=0.42 and asymmetric mefiant/confus/curieux eyes.",
)
def test_t1_ob_f03_02_all_16_expressions_and_asymmetry(env: TempOpenBotEnv) -> None:
    for expr in AVATAR_EXPRESSIONS:
        d = env.run_cli_json(
            "avatar-eval",
            "--seed",
            "agent-1",
            "--style",
            "flat",
            "--expression",
            expr,
            "--t",
            "0",
        )
        got_expr = pick(d, "profile.expression", "expression")
        assert got_expr == expr
        if expr == "somnolent":
            rest_open = float(require_field(d, "eyes.rest_open", "rest_open", expected_type=(int, float)))
            assert math.isclose(rest_open, 0.42, abs_tol=0.02)
        if expr in ("mefiant", "confus", "curieux"):
            lw = float(require_field(d, "eyes.left.w", expected_type=(int, float)))
            lh = float(require_field(d, "eyes.left.h", expected_type=(int, float)))
            rw = float(require_field(d, "eyes.right.w", expected_type=(int, float)))
            rh = float(require_field(d, "eyes.right.h", expected_type=(int, float)))
            assert (not math.isclose(lw, rw, abs_tol=1e-4)) or (
                not math.isclose(lh, rh, abs_tol=1e-4)
            ), f"Expected asymmetric eyes for {expr}, got L=({lw},{lh}), R=({rw},{rh})"


@e2e_test(
    id="TEST-T1-OB-F03-03",
    tier=1,
    features=["OB-F03"],
    description="All 15 parametric Bloub states (idle..comet) render cleanly with notify #2496e8 badge, orbit 6 arcs, and thinking dots.",
)
def test_t1_ob_f03_03_all_15_parametric_states(env: TempOpenBotEnv) -> None:
    for st in AVATAR_STATES:
        d = env.run_cli_json(
            "avatar-eval",
            "--seed",
            "agent-1",
            "--style",
            "flat",
            "--state",
            st,
            "--t",
            "1.0",
        )
        assert require_field(d, "state", expected_type=str) == st
        if st == "notify":
            nc = require_field(d, "ornaments.notify_color", "notify_color", expected_type=str)
            assert nc.lower() == "#2496e8"
        elif st == "orbit":
            arcs = require_field(d, "ornaments.orbit_arcs", "orbit_arcs", expected_type=int)
            assert arcs == 6
        elif st == "thinking":
            dots = require_field(d, "ornaments.busy_dots", "busy_dots", expected_type=int)
            assert dots >= 3


@e2e_test(
    id="TEST-T1-OB-F03-04",
    tier=1,
    features=["OB-F03"],
    description="Mulberry32 (seed=24301) blink schedule starts at t=1.4s, hits closed squash 0.06 at t=1.481s, and recovers at t=1.6s.",
)
def test_t1_ob_f03_04_mulberry32_first_blink_curve(env: TempOpenBotEnv) -> None:
    d_pre = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--expression", "neutre", "--t", "1.3"
    )
    d_mid = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--expression", "neutre", "--t", "1.481"
    )
    d_post = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--expression", "neutre", "--t", "1.6"
    )
    sq_pre = float(require_field(d_pre, "eyes.open_squash", "open_squash", expected_type=(int, float)))
    sq_mid = float(require_field(d_mid, "eyes.open_squash", "open_squash", expected_type=(int, float)))
    sq_post = float(require_field(d_post, "eyes.open_squash", "open_squash", expected_type=(int, float)))
    assert math.isclose(sq_pre, blink_open_squash_oracle(1.3, 1.0), abs_tol=0.02)
    assert math.isclose(sq_mid, 0.06, abs_tol=0.015)
    assert math.isclose(sq_post, 1.0, abs_tol=0.02)


@e2e_test(
    id="TEST-T1-OB-F03-05",
    tier=1,
    features=["OB-F03"],
    description="3.4s breath harmonic scales sy to 1.005 at t=0.85s and 0.995 at t=2.55s alongside active 3-harmonic wander.",
)
def test_t1_ob_f03_05_breath_3_4s_harmonic_and_wander(env: TempOpenBotEnv) -> None:
    d_peak = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--t", "0.85"
    )
    d_trough = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--t", "2.55"
    )
    sy_peak = float(require_field(d_peak, "breath_sy", expected_type=(int, float)))
    sy_trough = float(require_field(d_trough, "breath_sy", expected_type=(int, float)))
    assert math.isclose(sy_peak, 1.005, abs_tol=1e-3)
    assert math.isclose(sy_trough, 0.995, abs_tol=1e-3)
    assert require_field(d_peak, "eyes.wander_active", "wander_active", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F03-06",
    tier=1,
    features=["OB-F03"],
    description="Mouse pointer gaze (--pointer 0.8,-0.5 --pointer-type mouse) targets yaw=-13.2 deg, pitch=+16.5 deg, and disables wander.",
)
def test_t1_ob_f03_06_mouse_pointer_gaze_tracking(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json(
        "avatar-eval",
        "--seed",
        "agent-1",
        "--style",
        "flat",
        "--t",
        "1.2",
        "--pointer",
        "0.8,-0.5",
        "--pointer-type",
        "mouse",
    )
    yaw = float(require_field(d, "eyes.gaze.yaw", "gaze.yaw", expected_type=(int, float)))
    pitch = float(require_field(d, "eyes.gaze.pitch", "gaze.pitch", expected_type=(int, float)))
    wander = require_field(d, "eyes.wander_active", "wander_active", expected_type=bool)
    assert math.isclose(yaw, -13.2, abs_tol=0.25)
    assert math.isclose(pitch, 16.5, abs_tol=0.25)
    assert wander is False


# ============================================================================
# OB-F04: 3D Felt PlushBot, Moods, Clusters, Normalizer & CrewPile (Part 1: 01..04)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F04-01",
    tier=1,
    features=["OB-F04"],
    description="Plush sRGB gamma=0.6, GREY=0.55, sheen=0.08 tinting ramp matches reference RGBA byte vectors for #e8483f.",
)
def test_t1_ob_f04_01_plush_srgb_sheen_tint_ramp(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json(
        "avatar-eval", "--seed", "bot-alpha", "--color", "#e8483f", "--style", "plush", "--t", "0"
    )
    ramp = require_field(d, "tint_ramp", "tinted_sample", expected_type=dict)
    for grey in (0, 64, 128, 140, 200, 255):
        got_rgba = tuple(int(v) for v in ramp[str(grey)])
        exp_rgba = plush_srgb_tint_pixel_oracle("#e8483f", grey)
        assert got_rgba == exp_rgba, f"grey={grey}: expected {exp_rgba}, got {got_rgba}"


@e2e_test(
    id="TEST-T1-OB-F04-02",
    tier=1,
    features=["OB-F04"],
    description="All 7 plush AvatarMoods map to exact states, expressions, notify #2496e8 badge, failed head-shake, and asleep Zs.",
)
def test_t1_ob_f04_02_seven_plush_moods(env: TempOpenBotEnv) -> None:
    seen: Dict[str, Any] = {}
    for mood in AVATAR_MOODS:
        d = env.run_cli_json(
            "avatar-eval", "--seed", "agent-1", "--style", "plush", "--mood", mood, "--t", "0.2"
        )
        assert require_field(d, "mood", expected_type=str) == mood
        seen[mood] = d

    assert require_field(seen["working"], "state", expected_type=str) == "thinking"
    assert require_field(seen["connecting"], "state", expected_type=str) == "thinking"
    assert require_field(seen["waiting"], "state", expected_type=str) == "notify"
    assert require_field(seen["waiting"], "ornaments.notify_color", "notify_color", expected_type=str).lower() == "#2496e8"
    assert require_field(seen["failed"], "state", expected_type=str) == "failed"
    assert pick(seen["responded"], "profile.expression", "expression") == "heureux"
    assert require_field(seen["asleep"], "state", expected_type=str) == "asleep"
    assert int(require_field(seen["asleep"], "ornaments.sleep_zs", "sleep_zs", expected_type=int)) == 3


@e2e_test(
    id="TEST-T1-OB-F04-03",
    tier=1,
    features=["OB-F04"],
    description="Working plush avatar cycles faces across 1800ms cadence and composites 9-slice eye pads with plus-lighter.",
)
def test_t1_ob_f04_03_working_face_cycle_and_eye_pads(env: TempOpenBotEnv) -> None:
    d0 = env.run_cli_json(
        "avatar-eval",
        "--seed",
        "agent-1",
        "--style",
        "plush",
        "--mood",
        "working",
        "--working-faces",
        "--t",
        "0.0",
    )
    d1 = env.run_cli_json(
        "avatar-eval",
        "--seed",
        "agent-1",
        "--style",
        "plush",
        "--mood",
        "working",
        "--working-faces",
        "--t",
        "1.85",
    )
    e0 = require_field(d0, "profile.expression", "expression", expected_type=str)
    e1 = require_field(d1, "profile.expression", "expression", expected_type=str)
    assert e0 in AVATAR_EXPRESSIONS and e1 in AVATAR_EXPRESSIONS
    assert e0 != e1, f"Expected working face cycle between t=0.0 and t=1.85, both={e0}"
    pad_blend = require_field(d0, "eyes.left.pad_blend", "pad_blend", expected_type=str)
    assert pad_blend == "plus-lighter"


@e2e_test(
    id="TEST-T1-OB-F04-04",
    tier=1,
    features=["OB-F04"],
    description="2x2 ChannelAvatar cluster renders 1..4 faces with zero +N pill and 8 members as 3 faces + '+5' pill.",
)
def test_t1_ob_f04_04_channel_avatar_cluster_1_to_8(env: TempOpenBotEnv) -> None:
    for n in (1, 3, 4, 8):
        d = env.run_cli_json("avatar-eval", "--seed", "agent-1", "--cluster-count", str(n))
        cluster = require_field(d, "cluster", expected_type=dict)
        exp = channel_cluster_layout_oracle(n)
        assert require_field(cluster, "shown_faces", "visible_faces", expected_type=int) == exp["shown_faces"]
        assert require_field(cluster, "hidden_count", expected_type=int) == exp["hidden_count"]
        assert pick(cluster, "plus_pill", "overflow_pill") == exp["plus_pill"]


@e2e_test(
    id="TEST-T1-OB-F04-05",
    tier=1,
    features=["OB-F04"],
    description="Avatar image normalizer center-crops rectangular 1200x800 PNG into square <=512KB RIFF WEBP.",
)
def test_t1_ob_f04_05_avatar_image_normalizer_webp(env: TempOpenBotEnv) -> None:
    src_png = env.write_png_fixture("fixtures/rect_1200x800.png", width=1200, height=800)
    d = env.run_cli_json("avatar-eval", "--seed", "agent-1", "--normalize-image", str(src_png))
    norm = require_field(d, "normalized_image", expected_type=dict)
    assert require_field(norm, "mime", expected_type=str) == "image/webp"
    w = require_field(norm, "width", expected_type=int)
    h = require_field(norm, "height", expected_type=int)
    assert w == h and w in (512, 448, 384, 320)
    byte_len = require_field(norm, "byte_length", "normalized_bytes", expected_type=int)
    assert 12 <= byte_len <= 524288
    magic = require_field(norm, "magic", expected_type=str)
    assert magic.startswith("RIFF") and "WEBP" in magic


@e2e_test(
    id="TEST-T1-OB-F04-06",
    tier=1,
    features=["OB-F04"],
    description="CrewPile physics probe runs at 240Hz with 4 solver iterations, 8..64 bodies across yoshi..cloe, and 3.2s breathing.",
)
def test_t1_ob_f04_06_crew_pile_physics_240hz(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("ui-probe", "--scenario", "crew-pile-physics")
    cp = unwrap_entity(d, "crew_pile")
    assert require_field(cp, "step_hz", expected_type=int) == 240
    assert require_field(cp, "solver_iterations", expected_type=int) == 4
    body_count = require_field(cp, "body_count", expected_type=int)
    assert 8 <= body_count <= 64
    cast = extract_list(cp, "characters", "cast")
    assert {"yoshi", "nexo", "malu", "fred", "vita", "cloe"}.issubset(set(cast))
    assert math.isclose(
        float(require_field(cp, "breathe_period_sec", expected_type=(int, float))), 3.2, abs_tol=0.05
    )


# ============================================================================
# OB-F05: OpenBot Theme Tokens, Watcher, Shaders & Backdrops (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F05-01",
    tier=1,
    features=["OB-F05"],
    description="Dark OpenBotPalette resolves exact --openbot-* tokens (#0d0d0d frame, #1a1a1a canvas/sidebar, #007cf7 accent).",
)
def test_t1_ob_f05_01_dark_theme_tokens(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("theme-eval", "--mode", "dark")
    assert require_field(d, "mode", expected_type=str) == "dark"
    tokens = require_field(d, "tokens", expected_type=dict)
    assert tokens["--openbot-bg-frame"].lower() == "#0d0d0d"
    assert tokens["--openbot-bg-canvas"].lower() == "#1a1a1a"
    assert tokens["--openbot-bg-sidebar"].lower() == "#1a1a1a"
    assert tokens["--openbot-text-primary"].lower() == "#ffffff"
    assert tokens["--openbot-accent"].lower() == "#007cf7"


@e2e_test(
    id="TEST-T1-OB-F05-02",
    tier=1,
    features=["OB-F05"],
    description="Light OpenBotPalette resolves exact --openbot-* tokens (#e8e8eb frame, #f5f5f6 canvas, #ffffff surface, #006ad4 hover).",
)
def test_t1_ob_f05_02_light_theme_tokens(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("theme-eval", "--mode", "light")
    assert require_field(d, "mode", expected_type=str) == "light"
    tokens = require_field(d, "tokens", expected_type=dict)
    assert tokens["--openbot-bg-frame"].lower() == "#e8e8eb"
    assert tokens["--openbot-bg-canvas"].lower() == "#f5f5f6"
    assert tokens["--openbot-bg-surface"].lower() == "#ffffff"
    assert tokens["--openbot-text-primary"].lower() == "#141414"
    assert tokens["--openbot-accent-hover"].lower() == "#006ad4"


@e2e_test(
    id="TEST-T1-OB-F05-03",
    tier=1,
    features=["OB-F05"],
    description="Custom Tokyo Night --theme-json derives 52 --openbot-* tokens (#1a1b26 canvas, #13141c frame, #7aa2f7 accent).",
)
def test_t1_ob_f05_03_custom_theme_json_derivation(env: TempOpenBotEnv) -> None:
    tokyo_json = json.dumps(
        {
            "name": "Tokyo Night",
            "appearance": "dark",
            "colors": {
                "canvas": "#1a1b26",
                "frame": "#13141c",
                "sidebar": "#16161e",
                "surface": "#24283b",
                "text": "#c0caf5",
                "textMuted": "#565f89",
                "border": "#292e42",
                "accent": "#7aa2f7",
                "accentForeground": "#15161e",
                "success": "#9ece6a",
                "warning": "#e0af68",
                "danger": "#f7768e",
            },
        }
    )
    t_path = env.write_fixture("themes/tokyo-night.json", tokyo_json)
    d = env.run_cli_json("theme-eval", "--theme-json", str(t_path))
    tokens = require_field(d, "tokens", expected_type=dict)
    assert len(tokens) >= 52
    assert tokens["--openbot-bg-canvas"].lower() == "#1a1b26"
    assert tokens["--openbot-bg-frame"].lower() == "#13141c"
    assert tokens["--openbot-accent"].lower() == "#7aa2f7"


@e2e_test(
    id="TEST-T1-OB-F05-04",
    tier=1,
    features=["OB-F05"],
    description="Theme directory notify watcher detects atomic write/rename and hot-reloads Catppuccin Latte (#eff1f5).",
)
def test_t1_ob_f05_04_theme_watcher_hot_reload(env: TempOpenBotEnv) -> None:
    latte = json.dumps(
        {
            "name": "Catppuccin Latte",
            "appearance": "light",
            "colors": {
                "canvas": "#eff1f5",
                "frame": "#e6e9ef",
                "sidebar": "#e6e9ef",
                "surface": "#ccd0da",
                "text": "#4c4f69",
                "textMuted": "#6c6f85",
                "border": "#bcc0cc",
                "accent": "#1e66f5",
                "accentForeground": "#eff1f5",
                "success": "#40a02b",
                "warning": "#df8e1d",
                "danger": "#d20f39",
            },
        }
    )
    env.write_fixture("data/themes/catppuccin-latte.json", latte)
    env.write_fixture("config/OpenBot/themes/catppuccin-latte.json", latte)
    d = env.run_cli_json("ui-probe", "--scenario", "theme-hot-reload")
    reloaded = unwrap_entity(d, "hot_reload")
    assert require_field(reloaded, "watcher_triggered", "reloaded", expected_type=bool) is True
    active_canvas = require_field(reloaded, "active_canvas", expected_type=str)
    assert active_canvas.lower() == "#eff1f5"


@e2e_test(
    id="TEST-T1-OB-F05-05",
    tier=1,
    features=["OB-F05"],
    description="OKLab readableChatBackground clamps lightness and chroma to exact hexes for #f75d59 and #4699fe.",
)
def test_t1_ob_f05_05_oklab_readable_chat_background(env: TempOpenBotEnv) -> None:
    for hx in ("#f75d59", "#4699fe"):
        d_light = env.run_cli_json("theme-eval", "--mode", "light", "--chat-color", hx)
        d_dark = env.run_cli_json("theme-eval", "--mode", "dark", "--chat-color", hx)
        got_light = require_field(
            d_light, "chat_background.light", "readable_chat_bg", expected_type=str
        ).lower()
        got_dark = require_field(
            d_dark, "chat_background.dark", "readable_chat_bg", expected_type=str
        ).lower()
        assert got_light == oklab_readable_chat_bg_oracle(hx, "light")
        assert got_dark == oklab_readable_chat_bg_oracle(hx, "dark")


@e2e_test(
    id="TEST-T1-OB-F05-06",
    tier=1,
    features=["OB-F05"],
    description="All 6 pixel-art ChatBackdrop scenes (pond, forest/jellyfish, city/sunset, snow, desert, coast) render 6px cells and 0.55 center fade.",
)
def test_t1_ob_f05_06_six_chat_backdrop_scenes(env: TempOpenBotEnv) -> None:
    for cli_name in ("pond", "forest", "city", "snow", "desert", "coast"):
        canonical, _, exp_dark = CHAT_BACKDROP_SCENES[cli_name]
        d = env.run_cli_json("theme-eval", "--mode", "dark", "--backdrop", cli_name)
        bd = require_field(d, "backdrop", expected_type=dict)
        got_scene = require_field(bd, "scene", "canonical_scene", expected_type=str)
        assert got_scene in (cli_name, canonical)
        assert require_field(bd, "base_hex", expected_type=str).lower() == exp_dark
        assert require_field(bd, "cell_px", expected_type=int) == 6
        assert math.isclose(
            float(require_field(bd, "calm_center_alpha", expected_type=(int, float))),
            0.55,
            abs_tol=0.01,
        )
        assert require_field(bd, "draw_cells", expected_type=int) > 0
