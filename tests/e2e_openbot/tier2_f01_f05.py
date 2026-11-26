"""Tier 2 Boundary, Corner & Negative Opaque-Box E2E Tests for OB-F01..OB-F05 (30 tests, 6/feature).

Verifies input validation bounds, schema migration guards, SQLite authorizer denials,
'goutte' silhouette replacement, gaze culling, cluster thresholds, image limits, and theme safety.
"""

from __future__ import annotations

import hashlib
import json
import math
import sqlite3
from typing import Any, Dict

from .harness import (
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
    stable_index,
    unwrap_entity,
)


def _profile_dict(data: Any) -> Dict[str, str]:
    p = pick(data, "profile")
    src = p if isinstance(p, dict) else data
    return {
        "style": require_field(src, "style", expected_type=str),
        "shape": require_field(src, "shape", expected_type=str),
        "expression": require_field(src, "expression", expected_type=str),
        "color": require_field(src, "color", expected_type=str).lower(),
    }


# ============================================================================
# OB-F01 Tier 2: Seed Bounds, 'goutte' Exclusion, Hue Saturation & Schema Caps
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F01-01",
    tier=2,
    features=["OB-F01"],
    description="Empty avatar-eval seed hashes raw '' to (plush, capsule, excite, #e8483f), whereas agent create rejects empty, >128-char, or invalid-charset seeds.",
)
def test_t2_ob_f01_01_empty_and_invalid_avatar_seeds(env: TempOpenBotEnv) -> None:
    d_empty = env.run_cli_json("avatar-eval", "--seed", "")
    expected_empty = resolve_bloub_avatar_profile_oracle("")
    assert expected_empty == {
        "style": "plush",
        "shape": "capsule",
        "expression": "excite",
        "color": "#e8483f",
    }
    assert _profile_dict(d_empty) == expected_empty

    env.run_cli_expect_error(
        "agent", "create", "--name", "BadSeed1", "--avatar-seed", "",
        error_contains=("empty", "blank", "invalid"),
    )
    env.run_cli_expect_error(
        "agent", "create", "--name", "BadSeed2", "--avatar-seed", "a" * 129,
        error_contains=("128", "long", "limit", "invalid"),
    )
    env.run_cli_expect_error(
        "agent", "create", "--name", "BadSeed3", "--avatar-seed", "BAD SEED!",
        error_contains=("character", "charset", "pattern", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F01-02",
    tier=2,
    features=["OB-F01"],
    description="Seeds hashing to index 7 ('goutte') remap deterministically via ':shape:replacement' and never emit 'goutte'.",
)
def test_t2_ob_f01_02_goutte_silhouette_exclusion_and_replacement(env: TempOpenBotEnv) -> None:
    assert stable_index("agent-0:shape", 11) == 7  # raw index 7 is 'goutte'
    d0 = env.run_cli_json("avatar-eval", "--seed", "agent-0")
    p0 = _profile_dict(d0)
    assert p0["shape"] != "goutte"
    assert p0 == {"style": "plush", "shape": "losange", "expression": "timide", "color": "#f0b429"}

    d_fantome = env.run_cli_json(
        "avatar-eval", "--seed", "bot-alpha:fantome:flat:124", "--color", "#38bdf8"
    )
    pf = _profile_dict(d_fantome)
    assert pf == {"style": "flat", "shape": "fantome", "expression": "mefiant", "color": "#38bdf8"}


@e2e_test(
    id="TEST-T2-OB-F01-03",
    tier=2,
    features=["OB-F01"],
    description="nearestAvatarHue returns null below saturation 0.15 (#a3a3a3, #807070) and 215 at saturation 0.1632 (#64748b).",
)
def test_t2_ob_f01_03_nearest_hue_neutral_saturation_boundary(env: TempOpenBotEnv) -> None:
    d_grey = env.run_cli_json("avatar-eval", "--seed", "bot-alpha", "--color", "#a3a3a3")
    d_low = env.run_cli_json("avatar-eval", "--seed", "bot-alpha", "--color", "#807070")
    d_above = env.run_cli_json("avatar-eval", "--seed", "bot-alpha", "--color", "#64748b")

    assert require_field(d_grey, "nearest_hue", allow_none=True) is None
    assert require_field(d_low, "nearest_hue", allow_none=True) is None
    assert require_field(d_above, "nearest_hue", expected_type=int) == nearest_avatar_hue_oracle("#64748b") == 215


@e2e_test(
    id="TEST-T2-OB-F01-04",
    tier=2,
    features=["OB-F01"],
    description="Agent backupModels accepts exact upper bound of 8 entries and rejects 9 entries (> AGENT_BACKUP_MODEL_LIMIT=8).",
)
def test_t2_ob_f01_04_backup_models_limit_8_vs_9(env: TempOpenBotEnv) -> None:
    eight = [{"provider": "claude", "model": f"claude-model-{i}"} for i in range(8)]
    ok_res = env.run_cli_json(
        "agent", "create", "--name", "EightBackups", "--backup-models", json.dumps(eight)
    )
    ag = unwrap_entity(ok_res, "agent")
    bm = require_field(ag, "backup_models")
    order = bm if isinstance(bm, list) else require_field(bm, "order", expected_type=list)
    assert len(order) == 8

    nine = eight + [{"provider": "grok", "model": "grok-9"}]
    env.run_cli_expect_error(
        "agent", "create", "--name", "NineBackups", "--backup-models", json.dumps(nine),
        error_contains=("8", "limit", "maximum", "too many", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F01-05",
    tier=2,
    features=["OB-F01"],
    description="agent create rejects unknown local AgentProviderId and unsupported reasoningEffort values.",
)
def test_t2_ob_f01_05_unknown_provider_and_reasoning_effort(env: TempOpenBotEnv) -> None:
    env.run_cli_expect_error(
        "agent", "create", "--name", "BadProv", "--provider", "bogus-llm-99",
        error_contains=("unrecognized", "unsupported", "unknown", "invalid"),
    )
    env.run_cli_expect_error(
        "agent", "create", "--name", "BadEffort", "--reasoning-effort", "turbo",
        error_contains=("allowed", "unsupported", "unknown", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F01-06",
    tier=2,
    features=["OB-F01"],
    description="avatar-eval rejects non-canonical AvatarHue (180) and malformed hex colors (#FFF, #GGGGGG).",
)
def test_t2_ob_f01_06_invalid_hue_and_hex_color_rejected(env: TempOpenBotEnv) -> None:
    env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--hue", "180",
        error_contains=("10", "palette", "supported", "unsupported", "invalid"),
    )
    env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--color", "#FFF",
        error_contains=("6-digit", "rrggbb", "hex", "invalid"),
    )
    env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--color", "#GGGGGG",
        error_contains=("6-digit", "rrggbb", "hex", "invalid"),
    )


# ============================================================================
# OB-F02 Tier 2: Newer Schema, Missing Migration, SQLite Authorizer & 64-Table Cap
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F02-01",
    tier=2,
    features=["OB-F02"],
    description="openbot.db with schema_migrations version 70 (>69) fails closed with NewerDatabaseError without mutating DB bytes.",
)
def test_t2_ob_f02_01_newer_database_version_70_fails_closed(env: TempOpenBotEnv) -> None:
    db_file = env.data_dir / "openbot.db"
    conn = sqlite3.connect(str(db_file))
    conn.execute("PRAGMA journal_mode = DELETE")
    conn.execute(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)"
    )
    for v in range(8, 71):
        conn.execute("INSERT INTO schema_migrations VALUES (?, '2026-01-01T00:00:00Z')", (v,))
    conn.commit()
    conn.close()
    digest_before = hashlib.sha256(db_file.read_bytes()).hexdigest()

    env.run_cli_expect_error(
        "agent", "list", error_contains=("newer", "70", "69", "version", "schema")
    )
    digest_after = hashlib.sha256(db_file.read_bytes()).hexdigest()
    assert digest_after == digest_before


@e2e_test(
    id="TEST-T2-OB-F02-02",
    tier=2,
    features=["OB-F02"],
    description="openbot.db with non-contiguous migration history (missing version 42) fails closed on startup.",
)
def test_t2_ob_f02_02_missing_migration_version_42_rejected(env: TempOpenBotEnv) -> None:
    env.run_cli_json("agent", "list")
    conn = sqlite3.connect(str(env.openbot_db_path))
    conn.execute("DELETE FROM schema_migrations WHERE version = 42")
    conn.commit()
    conn.close()

    err_res = env.run_cli_expect_error(
        "agent", "list", error_contains=("missing", "42", "migration", "version")
    )
    assert err_res.returncode in (1, 2)
    rem = env.query_sqlite("SELECT count(*) AS cnt FROM schema_migrations WHERE version = 42")
    assert rem[0]["cnt"] == 0


@e2e_test(
    id="TEST-T2-OB-F02-03",
    tier=2,
    features=["OB-F02"],
    description="SQLite authorizer denies non-owner DROP TABLE and ALTER TABLE on protectedTables while preserving owner's table.",
)
def test_t2_ob_f02_03_authorizer_denies_non_owner_ddl(env: TempOpenBotEnv) -> None:
    a_res = env.run_cli_json("agent", "create", "--name", "OwnerA")
    b_res = env.run_cli_json("agent", "create", "--name", "PeerB")
    aid = require_field(unwrap_entity(a_res, "agent"), "id", expected_type=str)
    bid = require_field(unwrap_entity(b_res, "agent"), "id", expected_type=str)

    env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "CREATE TABLE ledger (id INT PRIMARY KEY, note TEXT)",
    )
    env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "INSERT INTO ledger VALUES (1, 'intact')",
    )

    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", bid, "--mode", "write",
        "--sql", "DROP TABLE ledger",
        error_contains=("authorized", "authorizer", "owner", "not table owner", "denied"),
    )
    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", bid, "--mode", "write",
        "--sql", "ALTER TABLE ledger ADD COLUMN z TEXT",
        error_contains=("authorized", "authorizer", "owner", "not table owner", "denied"),
    )

    q = env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", bid, "--mode", "read",
        "--sql", "SELECT count(*) FROM ledger",
    )
    rows = extract_list(q, "rows")
    assert "1" in json.dumps(rows)


@e2e_test(
    id="TEST-T2-OB-F02-04",
    tier=2,
    features=["OB-F02"],
    description="agent-data-exec blocks openbot_metadata tampering, ATTACH DATABASE, and multi-statement SQL injection.",
)
def test_t2_ob_f02_04_metadata_tamper_attach_and_multistatement_blocked(env: TempOpenBotEnv) -> None:
    a_res = env.run_cli_json("agent", "create", "--name", "OwnerA")
    aid = require_field(unwrap_entity(a_res, "agent"), "id", expected_type=str)
    env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "CREATE TABLE safe_tbl (x INT)",
    )

    meta_tbls = env.query_sqlite(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('openbot_metadata', '_openbot_tables')",
        db_kind="agent_data",
    )
    assert len(meta_tbls) >= 1, "Expected openbot_metadata or _openbot_tables in agent-data.db"
    meta_name = str(meta_tbls[0]["name"])

    del_res = env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", f"DELETE FROM {meta_name}",
        error_contains=("authorized", "authorizer", "protected", "system", "denied"),
    )
    del_combined = f"{del_res.stdout}\n{del_res.stderr}\n{del_res.error}".lower()
    assert "no such table" not in del_combined, (
        f"DELETE FROM {meta_name} failed with 'no such table' instead of authorizer denial: {del_combined}"
    )
    safe_rows = env.query_sqlite(
        f"SELECT owner_agent_id FROM {meta_name} WHERE table_name = 'safe_tbl'",
        db_kind="agent_data",
    )
    assert len(safe_rows) == 1 and safe_rows[0]["owner_agent_id"] == aid

    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "ATTACH DATABASE ':memory:' AS ext",
        error_contains=("not allowed", "authorizer", "blocked", "disallowed", "denied"),
    )
    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "read",
        "--sql", "SELECT 1; DROP TABLE safe_tbl",
        error_contains=("statement", "multiple", "trailing", "invalid"),
    )
    env.check_sqlite_integrity("agent_data")


@e2e_test(
    id="TEST-T2-OB-F02-05",
    tier=2,
    features=["OB-F02"],
    description="agent-data.db allows up to 64 shared tables and rejects the 65th CREATE TABLE (INPUT_LIMITS.sharedTables=64).",
)
def test_t2_ob_f02_05_agent_data_64_table_cap(env: TempOpenBotEnv) -> None:
    a_res = env.run_cli_json("agent", "create", "--name", "TableCapBot")
    aid = require_field(unwrap_entity(a_res, "agent"), "id", expected_type=str)
    for idx in range(1, 65):
        env.run_cli_json(
            "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
            "--sql", f"CREATE TABLE t{idx} (id INT)",
        )

    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "CREATE TABLE t65 (id INT)",
        error_contains=("64", "tables", "limit"),
    )


@e2e_test(
    id="TEST-T2-OB-F02-06",
    tier=2,
    features=["OB-F02"],
    description="agent-data.db with PRAGMA user_version=2 (>1) refuses reads and writes with newer-schema without mutating digest.",
)
def test_t2_ob_f02_06_agent_data_newer_user_version_2_refused(env: TempOpenBotEnv) -> None:
    a_res = env.run_cli_json("agent", "create", "--name", "VerBot")
    aid = require_field(unwrap_entity(a_res, "agent"), "id", expected_type=str)
    adb = env.data_dir / "Shared" / "Data" / "agent-data.db"
    conn = sqlite3.connect(str(adb))
    conn.execute("PRAGMA journal_mode = DELETE")
    conn.execute("CREATE TABLE dummy_future (x INT)")
    conn.execute("PRAGMA user_version = 2")
    conn.commit()
    conn.close()
    sha_before = hashlib.sha256(adb.read_bytes()).hexdigest()

    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "read",
        "--sql", "SELECT 1",
        error_contains=("newer", "schema", "version"),
    )
    sha_after = hashlib.sha256(adb.read_bytes()).hexdigest()
    assert sha_after == sha_before


# ============================================================================
# OB-F03 Tier 2: Narrow Decalage, Touch Ignore, Reduced Motion, Double-Blink & Cull
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F03-01",
    tier=2,
    features=["OB-F03"],
    description="Narrow silhouettes ('triangle', 'losange') under extreme corner gaze keep 100% of eye capsules inside body contour.",
)
def test_t2_ob_f03_01_narrow_silhouette_decalage_zero_clipping(env: TempOpenBotEnv) -> None:
    for shape, ptr in (("triangle", "1.0,1.0"), ("losange", "-1.0,-1.0")):
        d = env.run_cli_json(
            "avatar-eval",
            "--seed", "bot-alpha",
            "--shape", shape,
            "--style", "flat",
            "--pointer", ptr,
            "--pointer-type", "mouse",
            "--t", "2.3",
        )
        assert require_field(d, "eyes.zero_clipping", "zero_clipping", expected_type=bool) is True


@e2e_test(
    id="TEST-T2-OB-F03-02",
    tier=2,
    features=["OB-F03"],
    description="Touch pointer input (--pointer-type touch) is ignored and matches ambient no-pointer gaze bit-for-bit.",
)
def test_t2_ob_f03_02_touch_pointer_ignored(env: TempOpenBotEnv) -> None:
    d_ambient = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--t", "1.2"
    )
    d_touch = env.run_cli_json(
        "avatar-eval",
        "--seed", "agent-1",
        "--style", "flat",
        "--t", "1.2",
        "--pointer", "0.9,-0.8",
        "--pointer-type", "touch",
    )
    assert require_field(d_touch, "eyes.gaze", expected_type=dict) == require_field(
        d_ambient, "eyes.gaze", expected_type=dict
    )
    assert require_field(d_touch, "body_path", expected_type=str) == require_field(
        d_ambient, "body_path", expected_type=str
    )


@e2e_test(
    id="TEST-T2-OB-F03-03",
    tier=2,
    features=["OB-F03"],
    description="--reduced-motion freezes BloubBot pose at frozenAt=0 producing identical body_path and eyes across timestamps.",
)
def test_t2_ob_f03_03_reduced_motion_frozen_pose(env: TempOpenBotEnv) -> None:
    samples = [
        env.run_cli_json(
            "avatar-eval", "--seed", "agent-1", "--style", "flat", "--reduced-motion", "--t", t_str
        )
        for t_str in ("0", "1.481", "9.687")
    ]
    base_path = require_field(samples[0], "body_path", expected_type=str)
    base_sq = float(require_field(samples[0], "eyes.open_squash", expected_type=(int, float)))
    for s in samples[1:]:
        assert require_field(s, "body_path", expected_type=str) == base_path
        assert math.isclose(
            float(require_field(s, "eyes.open_squash", expected_type=(int, float))),
            base_sq,
            abs_tol=1e-6,
        )
        assert math.isclose(
            float(require_field(s, "breath_sy", expected_type=(int, float))), 1.0, abs_tol=1e-6
        )


@e2e_test(
    id="TEST-T2-OB-F03-04",
    tier=2,
    features=["OB-F03"],
    description="Mulberry32 (seed=24301) double-blink at P[2]=9.687224s and P[3]=9.927224s hits closed squash 0.06 at both +0.081s midpoints.",
)
def test_t2_ob_f03_04_mulberry32_double_blink_midpoints(env: TempOpenBotEnv) -> None:
    m1 = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--expression", "neutre", "--t", "9.768224"
    )
    m2 = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--expression", "neutre", "--t", "10.008224"
    )
    sq1 = float(require_field(m1, "eyes.open_squash", expected_type=(int, float)))
    sq2 = float(require_field(m2, "eyes.open_squash", expected_type=(int, float)))
    assert math.isclose(sq1, blink_open_squash_oracle(9.768224, 1.0), abs_tol=0.015)
    assert math.isclose(sq2, blink_open_squash_oracle(10.008224, 1.0), abs_tol=0.015)
    assert sq1 <= 0.08 and sq2 <= 0.08


@e2e_test(
    id="TEST-T2-OB-F03-05",
    tier=2,
    features=["OB-F03"],
    description="Extreme spherical gaze culls far-horizon eye at depth<=0.02 and attenuates alpha to 0.5 at depth=0.06.",
)
def test_t2_ob_f03_05_extreme_spherical_gaze_cull_and_alpha_fade(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("ui-probe", "--scenario", "avatar-horizon-cull")
    hz = unwrap_entity(d, "horizon_gaze")
    assert require_field(hz, "culled_at_0_02", expected_type=bool) is True
    alpha_006 = float(require_field(hz, "alpha_at_0_06", expected_type=(int, float)))
    assert math.isclose(alpha_006, 0.5, abs_tol=0.02)


@e2e_test(
    id="TEST-T2-OB-F03-06",
    tier=2,
    features=["OB-F03"],
    description="Rapid mid-morph state switch (idle -> wide -> wink at t=0.15s) snapshots departFige without snapping to rest.",
)
def test_t2_ob_f03_06_mid_morph_depart_fige_continuity(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("ui-probe", "--scenario", "avatar-mid-morph")
    mm = unwrap_entity(d, "mid_morph")
    assert require_field(mm, "snapped_to_rest", expected_type=bool) is False
    max_jump = float(require_field(mm, "max_pose_jump", expected_type=(int, float)))
    assert 0.0 <= max_jump < 0.05


# ============================================================================
# OB-F04 Tier 2: Cluster >=13 Threshold, >10MB/Corrupt Images, Sheen Bounds & CrewPile
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F04-01",
    tier=2,
    features=["OB-F04"],
    description="ChannelAvatar cluster threshold: 12 renders 3 faces + '+9', 13 drops to 2 faces + '+11' (never '+10'), and 0 renders UsersRound.",
)
def test_t2_ob_f04_01_channel_cluster_12_vs_13_and_zero_fallback(env: TempOpenBotEnv) -> None:
    for n in (0, 12, 13):
        d = env.run_cli_json("avatar-eval", "--seed", "agent-1", "--cluster-count", str(n))
        cl = require_field(d, "cluster", expected_type=dict)
        exp = channel_cluster_layout_oracle(n)
        assert require_field(cl, "shown_faces", expected_type=int) == exp["shown_faces"]
        assert pick(cl, "plus_pill") == exp["plus_pill"]
        if n == 0:
            assert require_field(cl, "fallback_glyph", "fallback", expected_type=str) == "UsersRound"


@e2e_test(
    id="TEST-T2-OB-F04-02",
    tier=2,
    features=["OB-F04"],
    description="Avatar image normalizer rejects >10 MiB (11 MiB) source payload before full decode.",
)
def test_t2_ob_f04_02_oversized_11mb_image_rejected(env: TempOpenBotEnv) -> None:
    huge = env.write_fixture("fixtures/oversized_11mb.png", b"\x89PNG\r\n\x1a\n" + (b"\x00" * (11 * 1024 * 1024)))
    err_res = env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--normalize-image", str(huge),
        error_contains=("10", "size", "large", "limit", "bytes"),
    )
    assert err_res.returncode in (1, 2)
    assert pick(err_res.data, "normalized_image") is None


@e2e_test(
    id="TEST-T2-OB-F04-03",
    tier=2,
    features=["OB-F04"],
    description="Avatar image normalizer rejects unsupported SVG MIME and 0-byte / corrupt RIFF WEBP payloads.",
)
def test_t2_ob_f04_03_svg_and_corrupt_webp_rejected(env: TempOpenBotEnv) -> None:
    svg = env.write_fixture("fixtures/vector.svg", "<svg xmlns='http://www.w3.org/2000/svg'></svg>")
    empty_webp = env.write_fixture("fixtures/zero.webp", b"")
    bad_webp = env.write_fixture("fixtures/broken.webp", b"RIFF\x10\x00\x00\x00XXXXbad")

    env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--normalize-image", str(svg),
        error_contains=("mime", "unsupported", "format", "invalid"),
    )
    env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--normalize-image", str(empty_webp),
        error_contains=("empty", "0-byte", "corrupt", "invalid", "truncated"),
    )
    env.run_cli_expect_error(
        "avatar-eval", "--seed", "agent-1", "--normalize-image", str(bad_webp),
        error_contains=("corrupt", "magic", "invalid", "malformed"),
    )


@e2e_test(
    id="TEST-T2-OB-F04-04",
    tier=2,
    features=["OB-F04"],
    description="Animated 4-frame GIF normalizes into animated RIFF WEBP <=512KB and high-entropy PNG steps down within <=512KB.",
)
def test_t2_ob_f04_04_animated_gif_and_noisy_png_normalization(env: TempOpenBotEnv) -> None:
    gif_path = env.write_gif_fixture("fixtures/anim_4frame.gif", frames=4)
    d_gif = env.run_cli_json("avatar-eval", "--seed", "agent-1", "--normalize-image", str(gif_path))
    n_gif = require_field(d_gif, "normalized_image", expected_type=dict)
    assert require_field(n_gif, "animated", expected_type=bool) is True
    assert require_field(n_gif, "frame_count", expected_type=int) == 4
    assert require_field(n_gif, "byte_length", expected_type=int) <= 524288

    noisy_png = env.write_png_fixture("fixtures/noisy_600.png", width=600, height=600, noisy=True)
    d_png = env.run_cli_json("avatar-eval", "--seed", "agent-1", "--normalize-image", str(noisy_png))
    n_png = require_field(d_png, "normalized_image", expected_type=dict)
    assert require_field(n_png, "byte_length", expected_type=int) <= 524288


@e2e_test(
    id="TEST-T2-OB-F04-05",
    tier=2,
    features=["OB-F04"],
    description="Extreme plush tintRamp colors #000000 (sheen [80,80,80,255] at grey=255) and #ffffff clamp cleanly with zero u8 wrap.",
)
def test_t2_ob_f04_05_plush_tint_extreme_black_and_white(env: TempOpenBotEnv) -> None:
    d_blk = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--color", "#000000", "--style", "plush"
    )
    r_blk = require_field(d_blk, "tint_ramp", expected_type=dict)
    assert tuple(r_blk["140"]) == plush_srgb_tint_pixel_oracle("#000000", 140) == (0, 0, 0, 255)
    assert tuple(r_blk["255"]) == plush_srgb_tint_pixel_oracle("#000000", 255) == (80, 80, 80, 255)

    d_wht = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--color", "#ffffff", "--style", "plush"
    )
    r_wht = require_field(d_wht, "tint_ramp", expected_type=dict)
    assert tuple(r_wht["140"]) == plush_srgb_tint_pixel_oracle("#ffffff", 140) == (255, 255, 255, 255)
    assert tuple(r_wht["255"]) == plush_srgb_tint_pixel_oracle("#ffffff", 255) == (255, 255, 255, 255)


@e2e_test(
    id="TEST-T2-OB-F04-06",
    tier=2,
    features=["OB-F04"],
    description="Reduced-motion CrewPile settles offline with squash=0, tilt=0, idle faces, and broken avatarUrl triggers Bloub fallback.",
)
def test_t2_ob_f04_06_crew_pile_reduced_motion_and_broken_url_fallback(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("ui-probe", "--scenario", "crew-pile-reduced-motion-and-broken-url")
    res = unwrap_entity(d, "crew_pile_reduced")
    assert math.isclose(float(require_field(res, "max_squash", expected_type=(int, float))), 0.0, abs_tol=1e-6)
    assert math.isclose(float(require_field(res, "max_tilt", expected_type=(int, float))), 0.0, abs_tol=1e-6)
    assert require_field(res, "all_faces_idle", expected_type=bool) is True
    assert require_field(res, "failed_url_fallback", expected_type=bool) is True


# ============================================================================
# OB-F05 Tier 2: CSS Injection, >64KB/32-File Cap, Reduced-Motion Shaders & Isolation
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F05-01",
    tier=2,
    features=["OB-F05"],
    description="theme-eval rejects CSS injection strings in --theme-json color fields (APP_THEME_PALETTE_COLOR_PATTERN).",
)
def test_t2_ob_f05_01_css_injection_theme_json_rejected(env: TempOpenBotEnv) -> None:
    malicious = json.dumps(
        {
            "name": "Evil Theme",
            "appearance": "dark",
            "colors": {
                "canvas": "#1a1a1a",
                "frame": "#0d0d0d",
                "sidebar": "#1a1a1a",
                "surface": "#212121",
                "text": "#ffffff",
                "textMuted": "#979797",
                "border": "#2a2a2a",
                "accent": "red; background: url(https://evil.test/exfil)",
                "accentForeground": "#ffffff",
                "success": "#31cf76",
                "warning": "#ff9412",
                "danger": "#fd2f3b",
            },
        }
    )
    bad_file = env.write_fixture("themes/evil.json", malicious)
    err_res = env.run_cli_expect_error(
        "theme-eval", "--theme-json", str(bad_file),
        error_contains=("color", "invalid", "palette", "hex"),
    )
    assert err_res.returncode in (1, 2)
    assert pick(err_res.data, "tokens") is None


@e2e_test(
    id="TEST-T2-OB-F05-02",
    tier=2,
    features=["OB-F05"],
    description="Theme loader rejects >64 KiB JSON file, skips invalid 'BAD_ID!.json' stems, and caps directory scan at 32 palettes.",
)
def test_t2_ob_f05_02_oversized_64kb_and_32_palette_directory_cap(env: TempOpenBotEnv) -> None:
    oversized = env.write_fixture("themes/huge.json", "{" + (" " * 70000) + "}")
    env.run_cli_expect_error(
        "theme-eval", "--theme-json", str(oversized),
        error_contains=("64", "65536", "size", "large", "invalid"),
    )

    valid_tpl = {
        "name": "Valid",
        "appearance": "dark",
        "colors": {
            "canvas": "#1a1a1a",
            "frame": "#0d0d0d",
            "sidebar": "#1a1a1a",
            "surface": "#212121",
            "text": "#ffffff",
            "textMuted": "#979797",
            "border": "#2a2a2a",
            "accent": "#007cf7",
            "accentForeground": "#ffffff",
            "success": "#31cf76",
            "warning": "#ff9412",
            "danger": "#fd2f3b",
        },
    }
    tdir = env.root / "many_themes"
    for i in range(36):
        env.write_fixture(f"many_themes/theme-{i:02d}.json", json.dumps(valid_tpl))
    env.write_fixture("many_themes/BAD_ID!.json", json.dumps(valid_tpl))

    d = env.run_cli_json("theme-eval", "--mode", "dark", "--themes-dir", str(tdir))
    loaded = extract_list(d, "loaded_palettes", "palettes")
    assert len(loaded) == 32
    assert all("BAD_ID!" not in str(p) for p in loaded)


@e2e_test(
    id="TEST-T2-OB-F05-03",
    tier=2,
    features=["OB-F05"],
    description="--reduced-motion freezes ChatBackdrop at STILL_TIME=42.0 (animated=false) and suppresses 1200ms sweepTheme/beam shaders.",
)
def test_t2_ob_f05_03_reduced_motion_backdrop_still_time_and_no_sweep(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("theme-eval", "--mode", "dark", "--backdrop", "pond", "--reduced-motion")
    bd = require_field(d, "backdrop", expected_type=dict)
    assert require_field(bd, "animated", expected_type=bool) is False
    assert math.isclose(
        float(require_field(bd, "time_sec", "still_time", expected_type=(int, float))), 42.0, abs_tol=1e-4
    )
    tr = require_field(d, "transition", expected_type=dict)
    assert require_field(tr, "duration_ms", expected_type=int) == 0


@e2e_test(
    id="TEST-T2-OB-F05-04",
    tier=2,
    features=["OB-F05"],
    description="theme-eval rejects unknown backdrop scene names outside the 6 supported scenes.",
)
def test_t2_ob_f05_04_unknown_backdrop_rejected(env: TempOpenBotEnv) -> None:
    err_res = env.run_cli_expect_error(
        "theme-eval", "--mode", "dark", "--backdrop", "mars-colony",
        error_contains=("scene", "unknown", "unsupported", "invalid"),
    )
    assert err_res.returncode in (1, 2)
    assert bool(err_res.error.get("message"))


@e2e_test(
    id="TEST-T2-OB-F05-05",
    tier=2,
    features=["OB-F05"],
    description="OKLab readableChatBackground handles extreme #ffffff and #000000 achromatic bounds without NaN or gamut overflow.",
)
def test_t2_ob_f05_05_oklab_extreme_achromatic_bounds(env: TempOpenBotEnv) -> None:
    for hx, exp_l, exp_d in (
        ("#ffffff", "#ffffff", "#2e2e2e"),
        ("#000000", "#eeeeee", "#000000"),
    ):
        dl = env.run_cli_json("theme-eval", "--mode", "light", "--chat-color", hx)
        dd = env.run_cli_json("theme-eval", "--mode", "dark", "--chat-color", hx)
        got_l = require_field(dl, "chat_background.light", "readable_chat_bg", expected_type=str).lower()
        got_d = require_field(dd, "chat_background.dark", "readable_chat_bg", expected_type=str).lower()
        assert got_l == oklab_readable_chat_bg_oracle(hx, "light") == exp_l
        assert got_d == oklab_readable_chat_bg_oracle(hx, "dark") == exp_d


@e2e_test(
    id="TEST-T2-OB-F05-06",
    tier=2,
    features=["OB-F05"],
    description="Switching Chats <-> OpenBot isolates --openbot-* tokens from Telegram TdesktopThemePalette and preserves 52/288/320/44..192px.",
)
def test_t2_ob_f05_06_workspace_switch_theme_and_geometry_isolation(env: TempOpenBotEnv) -> None:
    d = env.run_cli_json("ui-probe", "--scenario", "workspace-switch-theme-isolation")
    tb = require_field(d, "telegram_baseline", expected_type=dict)
    assert require_field(tb, "nav_rail_px", expected_type=int) == 52
    assert require_field(tb, "chat_sidebar_px", expected_type=int) == 288
    assert require_field(tb, "inspector_px", expected_type=int) == 320
    assert require_field(tb, "composer_min_max_px", expected_type=list) == [44, 192]
    assert require_field(tb, "palette_untouched", expected_type=bool) is True
