"""Tier 1 Opaque-Box E2E tests for F21 (Server REST/Bridge/CLI) & F24 (Version 1.7.0, Workflows, Hygiene, Ledger)."""

from __future__ import annotations

import json
import os
import re
import stat
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

import pytest

from harness import REPO_ROOT, TARGET_RELEASE_VERSION, OrcaApiClient, OrcaCliRunner, TempWorkspace

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


def _unwrap_cli(payload: Any) -> Any:
    if isinstance(payload, dict):
        if payload.get("success") is True and "data" in payload:
            return payload["data"]
        if payload.get("ok") is True and "result" in payload:
            return payload["result"]
    return payload


def _unwrap_key(payload: Any, key: str) -> Any:
    return payload[key] if isinstance(payload, dict) and isinstance(payload.get(key), dict) else payload


def _create_bot(api: OrcaApiClient, name: str, *, agent: str = "claude") -> dict[str, Any]:
    data = _unwrap_key(api.post("/api/v1/orca/bots", json_body={"name": name, "agent": agent}).assert_status(200, 201).json(), "bot")
    assert isinstance(data, dict) and data.get("id"), f"missing bot id in {data!r}"
    return data


def _create_chat(api: OrcaApiClient, title: str, **extra: Any) -> str:
    chat = _unwrap_key(api.post("/api/v1/chats/create", json_body={"title": title, "kind": "group", **extra}).assert_status(200, 201).json(), "chat")
    chat_id = (chat.get("chat_id") or chat.get("id")) if isinstance(chat, dict) else None
    assert chat_id, f"missing chat_id in {chat!r}"
    return str(chat_id)


def _extract_messages(payload: Any) -> list[dict[str, Any]]:
    assert isinstance(payload, (list, dict)), f"unexpected messages payload: {payload!r}"
    msgs = payload if isinstance(payload, list) else payload.get("messages")
    assert isinstance(msgs, list), f"expected messages list in {payload!r}"
    return [m for m in msgs if isinstance(m, dict)]


def _run_py(script: Path, args: list[str], *, extra_env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1", **(extra_env or {})}
    return subprocess.run([sys.executable, str(script), *args], cwd=str(REPO_ROOT), env=env, capture_output=True, text=True, timeout=30.0)


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

    assert idx == 3441
    (root / "docs/specs/ledger/index.json").write_text(json.dumps({
        "format_version": 4, "project_id": "GPUI-TELEGRAM-PARITY", "total_source_files": 3441,
        "total_mapped_files": 3441, "omitted_count": 0, "unmapped_count": 0, "shards_count": 28, "shards": shard_metas,
    }), encoding="utf-8")
    for rel_tp, syms in rs_buckets.items():
        fp = root / rel_tp
        fp.parent.mkdir(parents=True, exist_ok=True)
        lines = ["// Synthetic E2E ledger target file", "// Gate 6.2 compliant"] + [f"pub struct {s};" for s in syms]
        while len(lines) < 6:
            lines.append("pub const LEDGER_PAD: u32 = 1;")
        fp.write_text("\n".join(lines) + "\n", encoding="utf-8")

    mod_dir = root / "docs/specs/modules"
    mod_dir.mkdir(parents=True, exist_ok=True)
    for mid, subs in MODULE_DOC_SHARDS.items():
        lines = [f"# Module {mid}"] + [f"- {sp}" for sub in subs for sp in paths_by_sub[sub]]
        (mod_dir / f"module_{mid}_spec.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

    for crate_dir, deps in GATE_64_DEPS.items():
        cpath = root / crate_dir / "Cargo.toml"
        cpath.parent.mkdir(parents=True, exist_ok=True)
        cpath.write_text(f'[package]\nname = "{Path(crate_dir).name}"\n[dependencies]\n' + "\n".join(f'{d} = "*"' for d in deps) + "\n", encoding="utf-8")

    for src_dir, pkgs in GATE_65_IMPORTS.items():
        spath = root / src_dir / "lib.rs"
        spath.parent.mkdir(parents=True, exist_ok=True)
        spath.write_text("\n".join(f"use {p}::*;" for p in pkgs) + "\npub fn mandate10_anchor() {}\n", encoding="utf-8")

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
# F21 — Server REST/WS/Webhook API, Telegram IM Bridge & fabushi CLI (Tier 1)
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f21
def test_f21_t1_authenticated_orca_rest_surface_roundtrip(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "SurfaceBot")["id"]

    bots_payload = orca_api.get("/api/v1/orca/bots").assert_status(200).json()
    bots = bots_payload if isinstance(bots_payload, list) else (bots_payload.get("bots") if isinstance(bots_payload, dict) else None)
    assert isinstance(bots, list) and any(isinstance(b, dict) and b.get("id") == bot_id for b in bots)

    fetched = _unwrap_key(orca_api.get(f"/api/v1/orca/bots/{bot_id}").assert_status(200).json(), "bot")
    assert isinstance(fetched, dict) and fetched.get("id") == bot_id and fetched.get("name") == "SurfaceBot"

    templates_payload = orca_api.get("/api/v1/orca/bots/templates").assert_status(200).json()
    templates = templates_payload if isinstance(templates_payload, list) else (templates_payload.get("templates") if isinstance(templates_payload, dict) else None)
    assert isinstance(templates, list) and len(templates) == 24, f"expected 24 bot templates, got {templates!r}"

    for endpoint in ("/api/v1/orca/needs-you", "/api/v1/orca/webhooks", "/api/v1/orca/automations", "/api/v1/orca/ide/usage"):
        data = orca_api.get(endpoint).assert_status(200).json()
        assert isinstance(data, (dict, list)), f"unexpected JSON shape at {endpoint}: {data!r}"


@pytest.mark.tier1
@pytest.mark.f21
def test_f21_t1_telegram_im_bridge_orca_bot_fusion(orca_api: OrcaApiClient) -> None:
    bot = _create_bot(orca_api, "TgFusedBot")
    chat_id = _create_chat(orca_api, "Orca Bridge Chat", orcaBotId=bot["id"])
    orca_api.assert_rpc_ok("bots.telegram.bindChat", {"botId": bot["id"], "chatId": chat_id})
    orca_api.post(f"/api/v1/chats/{chat_id}/messages", json_body={"text": "Bridge roundtrip ping"}).assert_status(200, 201)

    msgs = _extract_messages(orca_api.get(f"/api/v1/chats/{chat_id}/messages").assert_status(200).json())
    assert any("Bridge roundtrip ping" in str(m.get("text", "")) for m in msgs)


@pytest.mark.tier1
@pytest.mark.f21
def test_f21_t1_telegram_im_bridge_interactive_decision_card(orca_api: OrcaApiClient) -> None:
    bot = _create_bot(orca_api, "DecisionBot")
    chat_id = _create_chat(orca_api, "Decision Card Chat")
    orca_api.assert_rpc_ok("bots.telegram.bindChat", {"botId": bot["id"], "chatId": chat_id})

    item = _unwrap_key(orca_api.post(f"/api/v1/orca/bots/{bot['id']}/items", json_body={"title": "Approve release cut", "telegramChatId": chat_id}).assert_status(200, 201).json(), "item")
    item_num = (item.get("number") or item.get("item_num") or item.get("id")) if isinstance(item, dict) else None
    assert item_num, f"missing item_num in {item!r}"

    ask = {"questions": [{"prompt": "Proceed with v1.7.0?", "multi": False, "recommended": 0, "options": [{"label": "Approve"}, {"label": "Defer"}]}]}
    patched = _unwrap_key(orca_api.patch(f"/api/v1/orca/items/{item_num}", json_body={"status": "needs_you", "ask": ask, "telegramChatId": chat_id}).assert_status(200).json(), "item")
    q_event = patched.get("question_event") if isinstance(patched, dict) else None
    assert q_event is not None, f"missing question_event in {patched!r}"

    msgs = _extract_messages(orca_api.get(f"/api/v1/chats/{chat_id}/messages").assert_status(200).json())
    assert any(m.get("inline_keyboard") or m.get("bot_keyboard") for m in msgs), f"no Decision Card keyboard in {msgs!r}"

    orca_api.post(
        f"/api/v1/orca/items/{item_num}/comment",
        json_body={"actor": {"kind": "owner"}, "question_event": q_event, "text": "Proceed with v1.7.0? → Approve", "answers": {"Proceed with v1.7.0?": ["Approve"]}, "chatId": chat_id},
    ).assert_status(200, 201)
    after = _unwrap_key(orca_api.get(f"/api/v1/orca/items/{item_num}").assert_status(200).json(), "item")
    assert isinstance(after, dict) and str(after.get("status", "")).lower() == "working", f"expected working after answer, got {after!r}"


@pytest.mark.tier1
@pytest.mark.f21
def test_f21_t1_fabushi_cli_json_commands_exit_0(orca_cli: OrcaCliRunner) -> None:
    templates = _unwrap_cli(orca_cli.run("bot", "templates").assert_ok())
    tmpl_list = templates if isinstance(templates, list) else (templates.get("templates") if isinstance(templates, dict) else None)
    assert isinstance(tmpl_list, list) and len(tmpl_list) == 24

    created = _unwrap_key(_unwrap_cli(orca_cli.run("bot", "create", "--name", "CliBot", "--agent", "claude").assert_ok()), "bot")
    assert isinstance(created, dict) and created.get("id"), f"missing CLI created bot id: {created!r}"

    listed = _unwrap_cli(orca_cli.run("bot", "list").assert_ok())
    assert isinstance(listed, (dict, list)) and created["id"] in json.dumps(listed) and "CliBot" in json.dumps(listed)

    run_obj = _unwrap_key(_unwrap_cli(orca_cli.run("orchestration", "run-create", "--title", "E2E Run").assert_ok()), "run")
    assert isinstance(run_obj, dict) and (run_obj.get("id") or run_obj.get("run_id"))

    usage = _unwrap_cli(orca_cli.run("ide", "usage").assert_ok())
    assert isinstance(usage, (dict, list))


@pytest.mark.tier1
@pytest.mark.f21
def test_f21_t1_fabushi_cli_webhooks_never_print_url_or_secret(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    bot = _create_bot(orca_api, "CliHookBot")
    created = _unwrap_key(
        orca_api.post("/api/v1/orca/webhooks", json_body={"botId": bot["id"], "name": "cli-secret-hook", "adapter": "standard"}).assert_status(200, 201).json(),
        "webhook",
    )
    assert isinstance(created, dict) and created.get("id"), f"missing webhook id in {created!r}"
    hook_id = created["id"]
    secret = str(created.get("secret") or created.get("signingSecret") or "")
    ingest_url = str(created.get("url") or created.get("ingestUrl") or f"/api/v1/orca/webhooks/{hook_id}/ingest")
    assert secret

    for args in (("webhooks", "list"), ("webhooks", "show", hook_id)):
        res = orca_cli.run(*args, json_mode=True)
        parsed = _unwrap_cli(res.assert_ok())
        assert isinstance(parsed, (dict, list)), f"expected JSON object/list for {args}, got {parsed!r}"
        assert hook_id in res.stdout and "cli-secret-hook" in res.stdout, f"missing hook identity/name in {res.stdout!r}"
        combined = f"{res.stdout}\n{res.stderr}"
        assert secret not in combined and ingest_url not in combined and "/ingest" not in combined


@pytest.mark.tier1
@pytest.mark.f21
def test_f21_t1_rest_rpc_cli_unified_state_consistency(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    bot_id = _create_bot(orca_api, "ParityBot")["id"]

    rpc_list = orca_api.assert_rpc_ok("bots.list", {})
    assert bot_id in json.dumps(rpc_list)
    cli_list = _unwrap_cli(orca_cli.run("bot", "list").assert_ok())
    assert bot_id in json.dumps(cli_list)

    orca_cli.run("bot", "update", bot_id, "--name", "ParityBotRenamed").assert_ok()
    fetched = _unwrap_key(orca_api.get(f"/api/v1/orca/bots/{bot_id}").assert_status(200).json(), "bot")
    assert isinstance(fetched, dict) and fetched.get("name") == "ParityBotRenamed"


# ---------------------------------------------------------------------------
# F24 — Version 1.7.0, Lean Profiles, Workflows, Hygiene & Ledger (Tier 1)
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f24
def test_f24_t1_version_1_7_0_pins_all_three_files() -> None:
    cargo_data = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    cargo_ver = cargo_data["workspace"]["package"]["version"]

    swift_text = (REPO_ROOT / "scripts/macos/FabushiInstaller.swift").read_text(encoding="utf-8")
    swift_m = re.search(r'CFBundleShortVersionString"\)\s+as\?\s+String\s+\?\?\s+"([^"]+)"', swift_text)
    assert swift_m is not None, "CFBundleShortVersionString fallback version not found in FabushiInstaller.swift"
    swift_ver = swift_m.group(1)

    iss_text = (REPO_ROOT / "scripts/windows/FabushiSetup.iss").read_text(encoding="utf-8")
    iss_m = re.search(r'(?m)^\s*#define\s+AppVersion\s+"([^"]+)"\s*$', iss_text)
    assert iss_m is not None, "#define AppVersion not found in FabushiSetup.iss"
    iss_ver = iss_m.group(1)

    assert cargo_ver == TARGET_RELEASE_VERSION, f"Cargo.toml version={cargo_ver!r} != {TARGET_RELEASE_VERSION!r}"
    assert swift_ver == TARGET_RELEASE_VERSION, f"FabushiInstaller.swift version={swift_ver!r} != {TARGET_RELEASE_VERSION!r}"
    assert iss_ver == TARGET_RELEASE_VERSION, f"FabushiSetup.iss version={iss_ver!r} != {TARGET_RELEASE_VERSION!r}"
    assert cargo_ver == swift_ver == iss_ver
    assert "1.6.11" not in swift_text and "1.6.11" not in iss_text


@pytest.mark.tier1
@pytest.mark.f24
def test_f24_t1_cargo_toml_orca_members_and_lean_profiles() -> None:
    cargo_data = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    members = cargo_data["workspace"]["members"]
    assert {"crates/orca_core", "crates/orca_bots", "crates/orca_ide"} <= set(members)
    assert "tests/orca_e2e" not in members, "pure-Python tests/orca_e2e must NOT be a Cargo workspace member"
    assert len(members) >= 25, f"expected >= 25 workspace members (22 baseline + 3 Orca crates), got {len(members)}"

    profiles = cargo_data["profile"]
    for prof_name in ("dev", "test", "ci", "release"):
        prof = profiles[prof_name]
        assert prof.get("debug") == 0, f"[profile.{prof_name}] debug must be 0: {prof!r}"
        assert prof.get("strip") == "debuginfo", f"[profile.{prof_name}] strip must be 'debuginfo': {prof!r}"
        assert prof.get("incremental") is False, f"[profile.{prof_name}] incremental must be false: {prof!r}"


@pytest.mark.tier1
@pytest.mark.f24
def test_f24_t1_github_workflows_ci_deploy_release_contracts() -> None:
    ci_yml = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    for token in ("ledger-parity-audit", "verify_ledger_closure.py --ci-without-upstream --skip-cargo", "cargo check --workspace --lib --bins", "clean_build_garbage.py", "orca_core", "orca_bots", "orca_ide"):
        assert token in ci_yml, f"ci.yml missing required token {token!r}"

    deploy_yml = (REPO_ROOT / ".github/workflows/deploy-backend.yml").read_text(encoding="utf-8")
    for token in ("crates/orca_core/**", "crates/orca_bots/**", "crates/orca_ide/**", "gpui_telegram_server", "fabushi_miniapp_server", "publish_fabushi_miniapp"):
        assert token in deploy_yml, f"deploy-backend.yml missing required token {token!r}"

    release_yml = (REPO_ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    for token in ('"v*"', "gpui_telegram_desktop", "gpui_telegram_server", "fabushi", "fabushi-remote-daemon", "Fabushi ${TAG_NAME}"):
        assert token in release_yml, f"release.yml missing required token {token!r}"


@pytest.mark.tier1
@pytest.mark.f24
def test_f24_t1_clean_build_garbage_removes_garbage_preserves_rlib_rmeta_fingerprint(
    temp_workspace: TempWorkspace,
) -> None:
    root = temp_workspace.path
    garbage_files = [
        temp_workspace.write("target/debug/incremental/session/a.bin", b"inc"),
        temp_workspace.write("target/debug/deps/foo.dSYM/Contents/Info.plist", b"plist"),
        temp_workspace.write("target/debug/deps/crate-1234.rcgu.o", b"obj"),
        temp_workspace.write("target/debug/deps/crate-5678.o", b"obj2"),
        temp_workspace.write("target/tmp/scratch.bin", b"tmp"),
        temp_workspace.write(".cargo-target-bad/stale.bin", b"bad"),
        temp_workspace.write("crates/sub/target/nested.bin", b"nested"),
    ]
    test_bin = temp_workspace.write("target/debug/deps/unit_bin-abcd", b"#!/bin/sh\n")
    test_bin.chmod(test_bin.stat().st_mode | stat.S_IXUSR)
    garbage_files.append(test_bin)

    preserved_files = [
        temp_workspace.write("target/debug/deps/liborca_core-aaaa.rlib", b"rlib"),
        temp_workspace.write("target/debug/deps/liborca_core-aaaa.rmeta", b"rmeta"),
        temp_workspace.write("target/debug/deps/liborca_core-aaaa.d", b"dep"),
        temp_workspace.write("target/debug/.fingerprint/orca_core-aaaa/ts", b"fp"),
    ]

    proc = _run_py(
        REPO_ROOT / "scripts/clean_build_garbage.py",
        ["--repo-root", str(root), "--target-dir", str(root / "target"), "--target-only", "--force"],
    )
    assert proc.returncode == 0, f"clean_build_garbage failed: {proc.stdout}\n{proc.stderr}"
    for path in garbage_files:
        assert not path.exists(), f"expected garbage removed: {path}"
    for path in preserved_files:
        assert path.exists(), f"expected compiler metadata preserved: {path}"


@pytest.mark.tier1
@pytest.mark.f24
def test_f24_t1_live_ledger_index_3441_and_mandate10_intact() -> None:
    index = json.loads((REPO_ROOT / "docs/specs/ledger/index.json").read_text(encoding="utf-8"))
    assert index["format_version"] == 4
    assert index["total_source_files"] == 3441
    assert index["total_mapped_files"] == 3441
    assert index["omitted_count"] == 0 and index["unmapped_count"] == 0
    assert index["shards_count"] == 28 and len(index["shards"]) == 28
    assert sum(s["file_count"] for s in index["shards"]) == 3441

    for meta in index["shards"]:
        shard_path = REPO_ROOT / meta["json_path"]
        assert shard_path.is_file(), f"missing shard: {shard_path}"
        shard = json.loads(shard_path.read_text(encoding="utf-8"))
        assert len(shard.get("entries", [])) == meta["file_count"]

    for crate_dir, deps in GATE_64_DEPS.items():
        cargo_text = (REPO_ROOT / crate_dir / "Cargo.toml").read_text(encoding="utf-8")
        for dep in deps:
            assert dep in cargo_text, f"Gate 6.4 regression: {dep} missing in {crate_dir}/Cargo.toml"

    for src_dir, pkgs in GATE_65_IMPORTS.items():
        rs_texts = [p.read_text(encoding="utf-8", errors="ignore") for p in (REPO_ROOT / src_dir).rglob("*.rs")]
        for pkg in pkgs:
            pat = re.compile(rf"(?m)^\s*(?:pub\s+)?use\s+{re.escape(pkg)}\b")
            assert any(pat.search(t) for t in rs_texts), f"Gate 6.5 regression: use {pkg} missing in {src_dir}"


@pytest.mark.tier1
@pytest.mark.f24
def test_f24_t1_verify_ledger_closure_passes_self_contained_temp_fixture(temp_workspace: TempWorkspace) -> None:
    root = temp_workspace.mkdir("ledger_pass_repo")
    _build_minimal_ledger_fixture(root)
    proc = _run_py(
        REPO_ROOT / "scripts/verify_ledger_closure.py",
        ["--repo-root", str(root), "--ci-without-upstream", "--skip-cargo"],
        extra_env={"TDESKTOP_SOURCE_ROOT": str(root / "no_upstream")},
    )
    assert proc.returncode == 0, f"verify_ledger_closure failed on valid fixture:\n{proc.stdout}\n{proc.stderr}"
    assert "ALL 6 VERIFICATION GATES PASSED SUCCESSFULLY (Exit Code 0)." in proc.stdout
