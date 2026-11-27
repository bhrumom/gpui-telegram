"""Tier 2 Opaque-Box E2E tests for F21 (Auth 401, CLI Errors/Exit 3, Bridge Isolation) & F24 (Hygiene Guards & Ledger Gate Regressions)."""

from __future__ import annotations

import fcntl
import json
import os
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

from harness import (
    BOT_NAME_MAX_LEN, PLUGIN_APPROVAL_REQUIRED_EXIT_CODE, REPO_ROOT,
    OrcaApiClient, OrcaCliRunner, OrcaServerHandle, TempWorkspace,
)

CANONICAL_SUBSYSTEMS_28 = (
    "core", "mtproto", "api", "storage", "data", "main", "window", "layout",
    "dialogs", "history", "chat_helpers", "inline_bots", "ui", "boxes",
    "media", "editor", "calls", "info", "settings", "intro", "lang",
    "platform", "statistics", "payments", "passport", "export", "iv", "wallet",
)
MODULE_DOC_SHARDS = {
    "05": ("dialogs",), "06": ("history",), "07": ("chat_helpers", "inline_bots"),
    "08": ("ui", "boxes"), "09": ("media", "editor"), "10": ("calls",), "11": ("info", "settings"),
}
GATE_64_DEPS = {
    "crates/gpui_telegram_core": ("fabushi-messaging-core",),
    "crates/gpui_telegram_proto": ("fabushi-messaging-core", "mahayana-node-agent-coordinator"),
    "crates/gpui_telegram_server": ("fabushi-messaging-core", "mahayana-node-agent-coordinator", "mahayana-host-runtime"),
    "crates/gpui_telegram_client": ("fabushi-messaging-core", "mahayana-node-agent-coordinator"),
    "crates/gpui_telegram_agent": ("fabushi-messaging-core", "mahayana-node-agent-coordinator", "mahayana-host-runtime"),
    "crates/gpui_telegram_desktop": ("fabushi-messaging-core", "mahayana-node-agent-coordinator", "mahayana-host-runtime"),
}
GATE_65_IMPORTS = {
    "crates/gpui_telegram_core/src": ("fabushi_messaging_core",),
    "crates/gpui_telegram_proto/src": ("mahayana_node_agent_coordinator",),
    "crates/gpui_telegram_agent/src": ("mahayana_host_runtime", "mahayana_node_agent_coordinator"),
    "crates/gpui_telegram_server/src": ("fabushi_messaging_core",),
    "crates/gpui_telegram_desktop/src": ("fabushi_messaging_core",),
}


def _unwrap_key(payload: Any, key: str) -> Any:
    return payload[key] if isinstance(payload, dict) and isinstance(payload.get(key), dict) else payload


def _create_bot(api: OrcaApiClient, name: str, *, agent: str = "claude") -> dict[str, Any]:
    data = _unwrap_key(api.post("/api/v1/orca/bots", json_body={"name": name, "agent": agent}).assert_status(200, 201).json(), "bot")
    assert isinstance(data, dict) and data.get("id"), f"missing bot id in {data!r}"
    return data


def _run_py(script: Path, args: list[str], *, extra_env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1", **(extra_env or {})}
    return subprocess.run([sys.executable, str(script), *args], cwd=str(REPO_ROOT), env=env, capture_output=True, text=True, timeout=30.0)


def _run_ledger_verify(root: Path) -> subprocess.CompletedProcess[str]:
    return _run_py(REPO_ROOT / "scripts/verify_ledger_closure.py", ["--repo-root", str(root), "--ci-without-upstream", "--skip-cargo"], extra_env={"TDESKTOP_SOURCE_ROOT": str(root / "no_upstream")})


def _build_minimal_ledger_fixture(root: Path) -> None:
    (root / "docs/specs/ledger/shards").mkdir(parents=True, exist_ok=True)
    rs_buckets: dict[str, list[str]] = {}
    shard_metas: list[dict[str, Any]] = []
    paths_by_sub: dict[str, list[str]] = {}
    idx = 0
    for pos, sub in enumerate(CANONICAL_SUBSYSTEMS_28):
        count = 147 if pos == 0 else 122
        entries, sub_paths = [], []
        for _ in range(count):
            sp = f"Telegram/SourceFiles/{sub}/file_{idx:04d}.cpp"
            tp = f"crates/gpui_telegram_core/src/ledger_stub_{idx // 12:03d}.rs"
            sym = f"Sym{idx}"
            entries.append({
                "source_path": sp, "source_sha256": f"{idx:064x}", "source_line_count": 42,
                "subsystem": sub, "feature_id": f"F-{idx:04d}", "disposition": "rust_domain_or_server_1to1",
                "target_rust_gpui_path": tp, "target_symbol": sym, "verification_test_id": "tests/ledger_smoke.rs::ok",
                "source_classes_and_structs": [f"UpstreamClass{idx}"], "source_methods_and_signals": [f"handle_{idx}"],
                "style_tokens_and_metrics": [],
            })
            rs_buckets.setdefault(tp, []).append(sym)
            sub_paths.append(sp)
            idx += 1
        rel_json = f"docs/specs/ledger/shards/{sub}.json"
        (root / rel_json).write_text(json.dumps({"subsystem": sub, "entries": entries}), encoding="utf-8")
        shard_metas.append({"subsystem": sub, "json_path": rel_json, "file_count": count})
        paths_by_sub[sub] = sub_paths

    (root / "docs/specs/ledger/index.json").write_text(json.dumps({
        "format_version": 4, "project_id": "GPUI-TELEGRAM-PARITY", "total_source_files": 3441,
        "total_mapped_files": 3441, "omitted_count": 0, "unmapped_count": 0, "shards_count": 28, "shards": shard_metas,
    }), encoding="utf-8")
    for rel_tp, syms in rs_buckets.items():
        fp = root / rel_tp
        fp.parent.mkdir(parents=True, exist_ok=True)
        lines = ["// Synthetic E2E ledger target file", "// Gate 6.2 compliant"] + [f"pub struct {s};" for s in syms]
        while len(lines) < 6:
            lines.append("pub const PAD: u32 = 1;")
        fp.write_text("\n".join(lines) + "\n", encoding="utf-8")

    mod_dir = root / "docs/specs/modules"
    mod_dir.mkdir(parents=True, exist_ok=True)
    for mid, subs in MODULE_DOC_SHARDS.items():
        lines = [f"# Module {mid}"] + [f"- {sp}" for sub in subs for sp in paths_by_sub[sub]]
        (mod_dir / f"module_{mid}_spec.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

    for crate_dir, deps in GATE_64_DEPS.items():
        p = root / crate_dir / "Cargo.toml"
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(f'[package]\nname = "{Path(crate_dir).name}"\n[dependencies]\n' + "\n".join(f'{d} = "*"' for d in deps) + "\n", encoding="utf-8")

    for src_dir, pkgs in GATE_65_IMPORTS.items():
        p = root / src_dir / "lib.rs"
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("\n".join(f"use {pkg}::*;" for pkg in pkgs) + "\npub fn anchor() {}\n", encoding="utf-8")

    desk_src = root / "crates/gpui_telegram_desktop/src"
    (desk_src / "views").mkdir(parents=True, exist_ok=True)
    (desk_src / "main.rs").write_text("mod views;\nfn main() { let _ = views::shell::AppShell; }\n", encoding="utf-8")
    (desk_src / "views/mod.rs").write_text("pub mod shell;\n", encoding="utf-8")
    (desk_src / "views/shell.rs").write_text("pub struct AppShell;\npub fn boot_shell() -> AppShell { AppShell }\n", encoding="utf-8")
    (desk_src / "views/orphan.rs").write_text("pub struct OrphanView;\npub fn orphan_noop() -> OrphanView { OrphanView }\n", encoding="utf-8")

    cov_dir = root / "docs/verification/fusion-20261108"
    cov_dir.mkdir(parents=True, exist_ok=True)
    (cov_dir / "capability-coverage.json").write_text(
        json.dumps({
            "schema_version": 2,
            "stage_summary": {
                "file_mapped": {"verified": 1}, "entrypoint_reachable": {"verified": 1},
                "behavior_tested": {"unverified": 1, "verified": 0}, "ui_evidence_captured": {"unverified": 1, "verified": 0},
                "packaged_accepted": {"unverified": 1, "verified": 0},
            },
            "capabilities": [{
                "capability_id": "CAP-01",
                "evidence_stages": {
                    "file_mapped": {"status": "verified", "existing_shards": ["main.json"], "missing_shards": []},
                    "entrypoint_reachable": {"status": "verified", "verified_entrypoints": ["crates/gpui_telegram_desktop/src/views/shell.rs"]},
                    "behavior_tested": {"status": "unverified", "verified_runs": []},
                    "ui_evidence_captured": {"status": "unverified", "artifacts": []},
                    "packaged_accepted": {"status": "unverified", "artifacts": []},
                },
            }],
        }),
        encoding="utf-8",
    )


# ---------------------------------------------------------------------------
# F21 — Auth 401 Enforcement, CLI Structured Errors & Exit 3, Bridge Guards (Tier 2)
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f21
def test_f21_t2_orca_routes_reject_missing_bearer_401(orca_api: OrcaApiClient) -> None:
    # Positive authenticated baseline so a dumb 401-everything server cannot pass.
    baseline = orca_api.get("/api/v1/orca/bots").assert_status(200).json()
    assert isinstance(baseline, (dict, list))

    unauth = orca_api.with_token(None)
    unauth.get("/api/v1/orca/bots", include_auth=False).assert_status(401)
    unauth.post("/api/v1/orca/rpc", json_body={"method": "runtime.capabilities", "params": {}, "protocol_version": 3}, include_auth=False).assert_status(401)
    unauth.get("/api/v1/orca/needs-you", include_auth=False).assert_status(401)
    unauth.get("/api/v1/orca/webhooks", include_auth=False).assert_status(401)
    unauth.get("/api/v1/orca/automations", include_auth=False).assert_status(401)
    unauth.post("/api/v1/orca/orchestration/runs", json_body={"title": "unauth"}, include_auth=False).assert_status(401)
    unauth.get("/api/v1/orca/ide/artifacts", include_auth=False).assert_status(401)


@pytest.mark.tier2
@pytest.mark.f21
def test_f21_t2_orca_routes_reject_invalid_malformed_bearer_401(orca_api: OrcaApiClient) -> None:
    orca_api.get("/api/v1/orca/bots").assert_status(200)
    orca_api.assert_rpc_ok("runtime.capabilities", {})

    bad_headers = ("Bearer forged.invalid.token", "Bearer ", "Basic dXNlcjpwYXNz")
    routes = (
        ("GET", "/api/v1/orca/bots", None),
        ("POST", "/api/v1/orca/rpc", {"method": "runtime.capabilities", "params": {}, "protocol_version": 3}),
        ("GET", "/api/v1/orca/webhooks", None),
        ("GET", "/api/v1/orca/ide/plugins", None),
    )
    for auth_val in bad_headers:
        for method, path, body in routes:
            resp = orca_api.raw_request(method, path, headers={"Authorization": auth_val}, json_body=body, include_auth=False)
            assert resp.status == 401, f"expected 401 for {auth_val!r} on {method} {path}, got {resp.status}: {resp.text}"


@pytest.mark.tier2
@pytest.mark.f21
def test_f21_t2_orca_routes_reject_revoked_bearer_token_401(orca_server: OrcaServerHandle) -> None:
    sess = orca_server.register_user()
    api = orca_server.api_client(sess)
    api.get("/api/v1/orca/bots").assert_status(200)

    revoke_res = api.delete(f"/api/v1/auth/sessions/{sess.session_id}").assert_status(200).json()
    assert isinstance(revoke_res, dict) and revoke_res.get("revoked") is True

    api.get("/api/v1/orca/bots").assert_status(401)
    api.post("/api/v1/orca/rpc", json_body={"method": "runtime.capabilities", "params": {}, "protocol_version": 3}).assert_status(401)


@pytest.mark.tier2
@pytest.mark.f21
def test_f21_t2_fabushi_cli_nonzero_structured_error_json(
    orca_cli: OrcaCliRunner, orca_server: OrcaServerHandle, cli_bin_path: Path, temp_workspace: TempWorkspace
) -> None:
    # Positive CLI baseline before negative error checks.
    orca_cli.run("bot", "list").assert_ok()

    bad_tok_cli = OrcaCliRunner(cli_bin=cli_bin_path, server_url=orca_server.base_url, token="bad.token.xyz", work_dir=temp_workspace.mkdir("bad_tok_cli"))
    cases = (
        orca_cli.run("bot", "delete", "does-not-exist-999"),
        orca_cli.run("bot", "create", "--name", "x" * (BOT_NAME_MAX_LEN + 1), "--agent", "claude"),
        bad_tok_cli.run("bot", "list"),
    )
    for res in cases:
        assert res.returncode != 0, f"expected non-zero exit for {res.cmd}, got 0: {res.stdout}"
        payload = res.json()
        err = payload.get("error") if isinstance(payload, dict) else None
        assert isinstance(err, dict), f"expected structured error object in {payload!r}"
        assert isinstance(err.get("code"), str) and err["code"], f"missing error.code in {payload!r}"
        assert isinstance(err.get("message"), str) and err["message"], f"missing error.message in {payload!r}"
    assert cases[0].extract_error_code() in {"bot_not_found", "not_found"}
    assert cases[1].extract_error_code() == "invalid_argument"


@pytest.mark.tier2
@pytest.mark.f21
def test_f21_t2_fabushi_cli_plugin_approval_required_exact_exit_3(
    orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    # Positive CLI baseline before exit-3 consent check.
    orca_cli.run("bot", "list").assert_ok()

    plugin_dir = temp_workspace.mkdir("trusted_worker_plugin")
    manifest = {
        "schema_version": 2,
        "publisher": "e2e",
        "id": "e2e-trusted-worker",
        "version": "1.0.0",
        "tier": "trusted-worker",
        "capabilities": ["storage", "hooks.agentLaunch"],
        "contributes": {"hooks": [{"event": "agentLaunch", "onError": "pass"}]},
    }
    (plugin_dir / "orca-plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    (plugin_dir / "worker.js").write_text("export function onAgentLaunch() { return { ok: true }; }\n", encoding="utf-8")

    res = orca_cli.run("ide", "plugin-install", "--path", str(plugin_dir))
    assert PLUGIN_APPROVAL_REQUIRED_EXIT_CODE == 3
    res.assert_error("approval_required", expected_exit=PLUGIN_APPROVAL_REQUIRED_EXIT_CODE)
    assert "fingerprint" in json.dumps(res.json()).lower(), f"missing consent fingerprint metadata: {res.stdout}\n{res.stderr}"


@pytest.mark.tier2
@pytest.mark.f21
def test_f21_t2_telegram_im_bridge_cross_user_isolation_and_replay_guard(orca_server: OrcaServerHandle) -> None:
    user_a = orca_server.register_user()
    user_b = orca_server.register_user()
    api_a = orca_server.api_client(user_a)
    api_b = orca_server.api_client(user_b)

    bot = _create_bot(api_a, "IsoBridgeBot")
    chat = _unwrap_key(api_a.post("/api/v1/chats/create", json_body={"title": "Private Bridge Chat", "kind": "group"}).assert_status(200, 201).json(), "chat")
    chat_id = (chat.get("chat_id") or chat.get("id")) if isinstance(chat, dict) else None
    assert chat_id, f"missing chat_id in {chat!r}"
    api_a.assert_rpc_ok("bots.telegram.bindChat", {"botId": bot["id"], "chatId": chat_id})

    item = _unwrap_key(api_a.post(f"/api/v1/orca/bots/{bot['id']}/items", json_body={"title": "Confirm prod deploy", "telegramChatId": chat_id}).assert_status(200, 201).json(), "item")
    item_num = (item.get("number") or item.get("item_num") or item.get("id")) if isinstance(item, dict) else None
    assert item_num, f"missing item_num in {item!r}"
    ask = {"questions": [{"prompt": "Deploy now?", "multi": False, "recommended": 0, "options": [{"label": "Yes"}, {"label": "No"}]}]}
    patched = _unwrap_key(api_a.patch(f"/api/v1/orca/items/{item_num}", json_body={"status": "needs_you", "ask": ask, "telegramChatId": chat_id}).assert_status(200).json(), "item")
    q_event = patched.get("question_event") if isinstance(patched, dict) else None
    assert q_event is not None, f"missing question_event in {patched!r}"

    ans_yes = {"actor": {"kind": "owner"}, "question_event": q_event, "text": "Deploy now? → Yes", "answers": {"Deploy now?": ["Yes"]}, "chatId": chat_id}
    ans_no = {**ans_yes, "text": "Deploy now? → No", "answers": {"Deploy now?": ["No"]}}

    # User B cannot read or answer User A's chat or Decision Card.
    api_b.get(f"/api/v1/chats/{chat_id}/messages").assert_status(403, 404)
    api_b.post(f"/api/v1/orca/items/{item_num}/comment", json_body=ans_yes).assert_status(403, 404)

    # User A resolves once cleanly; replaying answer fails closed with question_closed.
    api_a.post(f"/api/v1/orca/items/{item_num}/comment", json_body=ans_yes).assert_status(200, 201)
    after = _unwrap_key(api_a.get(f"/api/v1/orca/items/{item_num}").assert_status(200).json(), "item")
    assert isinstance(after, dict) and str(after.get("status", "")).lower() == "working", f"expected working after answer, got {after!r}"
    api_a.post(f"/api/v1/orca/items/{item_num}/comment", json_body=ans_no).assert_error_code("question_closed", 400, 409)


# ---------------------------------------------------------------------------
# F24 — Hygiene Safety Guards & Ledger Gate Regression Detection (Tier 2)
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f24
def test_f24_t2_clean_build_garbage_dry_run_and_15s_min_age_guard(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.path
    inc = temp_workspace.write("target/debug/incremental/sess/a.bin", b"inc")
    fresh_o = temp_workspace.write("target/debug/deps/fresh-1.rcgu.o", b"fresh-o")
    fresh_bin = temp_workspace.write("target/debug/deps/fresh_test", b"#!/bin/sh\n")
    fresh_bin.chmod(fresh_bin.stat().st_mode | stat.S_IXUSR)

    old_o = temp_workspace.write("target/debug/deps/old-1.rcgu.o", b"old-o")
    old_bin = temp_workspace.write("target/debug/deps/old_test", b"#!/bin/sh\n")
    old_bin.chmod(old_bin.stat().st_mode | stat.S_IXUSR)
    old_ts = time.time() - 60.0
    os.utime(old_o, (old_ts, old_ts))
    os.utime(old_bin, (old_ts, old_ts))

    script = REPO_ROOT / "scripts/clean_build_garbage.py"
    dry = _run_py(script, ["--repo-root", str(root), "--target-dir", str(root / "target"), "--target-only", "--dry-run"])
    assert dry.returncode == 0 and "[clean_build_garbage DRY RUN] Done! Would free" in dry.stdout
    for p in (inc, fresh_o, fresh_bin, old_o, old_bin):
        assert p.exists(), f"--dry-run must not delete {p}"

    real = _run_py(script, ["--repo-root", str(root), "--target-dir", str(root / "target"), "--target-only"])
    assert real.returncode == 0
    assert fresh_o.exists() and fresh_bin.exists(), "15s min-age guard must protect freshly built artifacts"
    assert not inc.exists() and not old_o.exists() and not old_bin.exists()


@pytest.mark.tier2
@pytest.mark.f24
def test_f24_t2_clean_build_garbage_skips_active_cargo_lock_profile(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.path
    lock_file = temp_workspace.write("target/debug/.cargo-lock", b"")
    old_o = temp_workspace.write("target/debug/deps/locked-1.rcgu.o", b"locked")
    old_bin = temp_workspace.write("target/debug/deps/locked_test", b"#!/bin/sh\n")
    old_bin.chmod(old_bin.stat().st_mode | stat.S_IXUSR)
    old_ts = time.time() - 120.0
    os.utime(old_o, (old_ts, old_ts))
    os.utime(old_bin, (old_ts, old_ts))

    fd = os.open(lock_file, os.O_RDWR)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        proc = _run_py(
            REPO_ROOT / "scripts/clean_build_garbage.py",
            ["--repo-root", str(root), "--target-dir", str(root / "target"), "--target-only", "--force", "--verbose"],
        )
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)

    assert proc.returncode == 0, f"cleanup failed under lock: {proc.stdout}\n{proc.stderr}"
    assert "[skip-active-build] debug" in proc.stdout
    assert old_o.exists() and old_bin.exists(), "locked profile deps must remain untouched"


@pytest.mark.tier2
@pytest.mark.f24
def test_f24_t2_verify_ledger_closure_rejects_gate2_omission(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.mkdir("ledger_gate2")
    _build_minimal_ledger_fixture(root)
    idx_path = root / "docs/specs/ledger/index.json"
    idx_data = json.loads(idx_path.read_text(encoding="utf-8"))
    idx_data["omitted_count"] = 1
    idx_data["total_mapped_files"] = 3440
    idx_path.write_text(json.dumps(idx_data), encoding="utf-8")

    proc = _run_ledger_verify(root)
    assert proc.returncode == 1 and "PASS [Gate 1]" in proc.stdout and "FAIL [Gate 2]" in proc.stdout


@pytest.mark.tier2
@pytest.mark.f24
def test_f24_t2_verify_ledger_closure_rejects_gate64_and_gate65_inherited_regression(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.mkdir("ledger_gate64_65")
    _build_minimal_ledger_fixture(root)
    (root / "crates/gpui_telegram_core/Cargo.toml").write_text('[package]\nname = "gpui_telegram_core"\n[dependencies]\n', encoding="utf-8")
    (root / "crates/gpui_telegram_core/src/lib.rs").write_text("pub fn no_inherited_import() {}\n", encoding="utf-8")

    proc = _run_ledger_verify(root)
    assert proc.returncode == 1 and "PASS [Gate 5]" in proc.stdout
    assert "FAIL [Gate 6.4 - Cargo Dependencies]" in proc.stdout
    assert "FAIL [Gate 6.5 - Authentic Imports]" in proc.stdout


@pytest.mark.tier2
@pytest.mark.f24
def test_f24_t2_verify_ledger_closure_rejects_gate62_deleted_symbol(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.mkdir("ledger_gate62")
    _build_minimal_ledger_fixture(root)
    stub_path = root / "crates/gpui_telegram_core/src/ledger_stub_000.rs"
    stub_path.write_text(stub_path.read_text(encoding="utf-8").replace("pub struct Sym0;", "// Sym0 deleted"), encoding="utf-8")

    proc = _run_ledger_verify(root)
    assert proc.returncode == 1 and "PASS [Gate 5]" in proc.stdout and "FAIL [Gate 6.2 - Target Symbols]" in proc.stdout


@pytest.mark.tier2
@pytest.mark.f24
def test_f24_t2_verify_ledger_closure_rejects_gate67_unreachable_and_missing_dist(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.mkdir("ledger_gate67")
    _build_minimal_ledger_fixture(root)
    png_rel = "docs/verification/fusion-20261108/ui_shot.png"
    (root / png_rel).write_bytes(b"\x89PNG\r\n\x1a\n" + (b"\x00" * 10_500))
    cov_path = root / "docs/verification/fusion-20261108/capability-coverage.json"
    cov_path.write_text(
        json.dumps({
            "schema_version": 2,
            "stage_summary": {k: {"verified": 1} for k in ("file_mapped", "entrypoint_reachable", "behavior_tested", "ui_evidence_captured", "packaged_accepted")},
            "capabilities": [{
                "capability_id": "CAP-01",
                "evidence_stages": {
                    "file_mapped": {"status": "verified", "existing_shards": ["main.json"], "missing_shards": []},
                    "entrypoint_reachable": {"status": "verified", "verified_entrypoints": ["crates/gpui_telegram_desktop/src/views/orphan.rs"]},
                    "behavior_tested": {"status": "verified", "verified_runs": ["e2e-run-1"]},
                    "ui_evidence_captured": {"status": "verified", "artifacts": [png_rel]},
                    "packaged_accepted": {"status": "verified", "artifacts": ["dist/GPUI Telegram.app/Contents/MacOS/gpui_telegram_desktop"]},
                },
            }],
        }),
        encoding="utf-8",
    )

    proc = _run_ledger_verify(root)
    assert proc.returncode == 1 and "PASS [Gate 5]" in proc.stdout
    assert "FAIL [Gate 6.7 - Capability Coverage]" in proc.stdout
    assert "unreachable from runtime" in proc.stdout
    assert "packaged_accepted artifact missing on disk" in proc.stdout
