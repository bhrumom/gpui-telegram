"""Tier 2 opaque-box E2E tests for Area E3: F10 (Worktrees) and F11 (Orchestration)."""

from __future__ import annotations

import subprocess
import uuid
from typing import Any

import pytest

from harness import (
    IDE_ERROR_CODES,
    ORCA_YAML_MAX_ALIAS_COUNT,
    ORCA_YAML_MAX_BYTES,
    ORCA_YAML_MAX_DEFAULT_TABS,
    ORCA_YAML_MAX_SHARED_DIRS,
    WORKTREE_INCLUDE_MAX_BYTES,
    WORKTREE_INCLUDE_MAX_ENTRIES,
    WORKTREE_INCLUDE_MAX_PATHS,
    CliResult,
    GitRepoFixture,
    OrcaApiClient,
    OrcaCliRunner,
    TempWorkspace,
)

_UNIMPL_CODES = ("not_implemented", "unknown_method", "method_not_found")


def _uid(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def _req_str(obj: Any, *keys: str) -> str:
    assert isinstance(obj, dict), f"Expected dict payload, got {obj!r}"
    for key in keys:
        val = obj.get(key)
        if isinstance(val, str) and val:
            return val
    raise AssertionError(f"Missing non-empty string key in {keys!r}: {obj!r}")


def _alias_bomb_yaml(expansions: int) -> str:
    return "anchor: &x val\nitems:\n" + "\n".join("  - *x" for _ in range(expansions)) + "\n"


def _skipped_set(res: dict[str, Any]) -> set[str]:
    skipped = res.get("skipped")
    assert isinstance(skipped, list), f"Expected skipped list, got {res!r}"
    return {(s if isinstance(s, str) else str(s.get("entry") or s.get("path") or s.get("pattern") or "")) for s in skipped}


def _assert_budget_warning(res: Any) -> None:
    assert isinstance(res, dict), f"Expected dict result, got {res!r}"
    warnings = res.get("warnings")
    assert isinstance(warnings, list) and len(warnings) >= 1, f"Expected structured warnings: {res!r}"
    assert any(
        (w.get("code") == "copy_budget_exceeded" or "copy_budget_exceeded" in str(w))
        if isinstance(w, dict) else ("copy_budget_exceeded" in str(w))
        for w in warnings
    ), f"Missing copy_budget_exceeded warning: {warnings!r}"


def _assert_cli_domain_error(cli_res: CliResult) -> str:
    assert cli_res.returncode != 0, f"Expected non-zero CLI exit: {cli_res!r}"
    payload = cli_res.json()
    assert isinstance(payload, dict) and isinstance(payload.get("error"), dict), f"Missing error dict: {payload!r}"
    code = cli_res.extract_error_code()
    assert isinstance(code, str) and code and code not in _UNIMPL_CODES, f"Bad CLI error code {code!r}: {payload!r}"
    return code


# ---------------------------------------------------------------------------
# F10 Tier 2 — Strict Parser, .worktreeinclude Safety, Windows Shebang & Guards
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f10
@pytest.mark.edge_case("SM1-01")
def test_t2_f10_orca_yaml_strict_limits_duplicate_size_alias_and_caps(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    assert {"yaml_duplicate_key", "yaml_size_exceeded", "yaml_alias_limit"}.issubset(IDE_ERROR_CODES)
    parse = lambda body: orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body=body)
    for content in ("scripts:\n  setup: echo a\nscripts:\n  setup: echo b\n", "scripts:\n  setup: echo a\n  setup: echo b\n"):
        parse({"content": content}).assert_error_code("yaml_duplicate_key", 400, 422)
    for non_map in ("- item1\n- item2\n", '"plain-scalar"\n'):
        parse({"content": non_map}).assert_error_code("invalid_argument", 400, 422)

    oversize_utf8 = 'issueCommand: "' + ("a" * (ORCA_YAML_MAX_BYTES + 32)) + '"\n'
    assert len(oversize_utf8.encode("utf-8")) > ORCA_YAML_MAX_BYTES
    parse({"content": oversize_utf8}).assert_error_code("yaml_size_exceeded", 400, 413, 422)
    utf16_body = ('issueCommand: "' + ("b" * ((ORCA_YAML_MAX_BYTES // 2) + 32)) + '"\n').encode("utf-16-le")
    utf16_path = temp_workspace.write("oversize_utf16.yaml", b"\xff\xfe" + utf16_body)
    assert utf16_path.stat().st_size > ORCA_YAML_MAX_BYTES
    parse({"path": str(utf16_path)}).assert_error_code("yaml_size_exceeded", 400, 413, 422)

    parse({"content": _alias_bomb_yaml(ORCA_YAML_MAX_ALIAS_COUNT + 5)}).assert_error_code("yaml_alias_limit", 400, 422)

    tabs_yaml = "".join(f"  - title: t{i}\n    type: terminal\n" for i in range(ORCA_YAML_MAX_DEFAULT_TABS + 1))
    recipes_yaml = "".join(f"  - name: r{i}\n    command: echo {i}\n" for i in range(ORCA_YAML_MAX_DEFAULT_TABS + 1))
    res_lists = orca_api.assert_ok(parse({"content": f'issueCommand: "claude ok"\ndefaultTabs:\n{tabs_yaml}environmentRecipes:\n{recipes_yaml}'}))
    assert res_lists.get("defaultTabs") == [] and res_lists.get("environmentRecipes") == []
    assert res_lists.get("issueCommand") == "claude ok" and len(res_lists.get("warnings", [])) >= 1

    dirs_yaml = "".join(f"    - .cache/d{i}\n" for i in range(ORCA_YAML_MAX_SHARED_DIRS + 5))
    res_dirs = orca_api.assert_ok(parse({"content": f"worktree:\n  sharedDirectories:\n{dirs_yaml}"}))
    wt_cfg = res_dirs.get("worktree") if isinstance(res_dirs.get("worktree"), dict) else res_dirs
    shared = wt_cfg.get("sharedDirectories")
    assert isinstance(shared, list) and len(shared) == ORCA_YAML_MAX_SHARED_DIRS
    assert shared == [f".cache/d{i}" for i in range(ORCA_YAML_MAX_SHARED_DIRS)]


@pytest.mark.tier2
@pytest.mark.f10
@pytest.mark.edge_case("SM1-02")
def test_t2_f10_worktreeinclude_unsafe_entries_and_copy_budget_limits(
    temp_workspace: TempWorkspace, orca_api: OrcaApiClient
) -> None:
    assert "copy_budget_exceeded" in IDE_ERROR_CODES
    assert WORKTREE_INCLUDE_MAX_ENTRIES == 50_000 and WORKTREE_INCLUDE_MAX_PATHS == 1_000
    repo = GitRepoFixture(root=temp_workspace.mkdir("repo_sm102"))
    repo.commit_file("tracked.txt", "tracked\n", msg="init")
    repo.create_ignored_file(".env.ok", "OK=1\n")
    repo.create_ignored_file("debug.log", "LOG=1\n")
    repo.create_ignored_file("a1.txt", "A1=1\n")
    outside = temp_workspace.write("outside.txt", "OUTSIDE\n")
    assert (repo.path / "../outside.txt").resolve() == outside.resolve() and outside.exists()
    unsafe = ["*.log", "a?.txt", "!.env.ok", ".git/config", "../outside.txt", "tracked.txt"]
    repo.write_worktreeinclude([*unsafe, ".env.ok"], commit=True)

    dest1 = temp_workspace.mkdir("dest_wrap_1/wt_safe_dest")
    res1 = orca_api.assert_rpc_ok("worktree.includeCopy", {"sourcePath": str(repo.path), "destPath": str(dest1)})
    assert [p.name for p in dest1.iterdir()] == [".env.ok"] and (dest1 / ".env.ok").read_text(encoding="utf-8") == "OK=1\n"
    assert not (dest1 / "outside.txt").exists() and not (dest1.parent / "outside.txt").exists()
    for forbidden in ("debug.log", "a1.txt", ".git/config", "tracked.txt"):
        assert not (dest1 / forbidden).exists()
    assert set(unsafe).issubset(_skipped_set(res1))

    # 2 GiB byte budget via 0-block sparse file
    repo.create_ignored_file("big.bin", b"")
    with (repo.path / "big.bin").open("r+b") as fh:
        fh.truncate(WORKTREE_INCLUDE_MAX_BYTES + 4096)
    repo.write_worktreeinclude(["big.bin"], commit=True)
    dest2 = temp_workspace.mkdir("wt_sparse_dest")
    res2 = orca_api.assert_rpc_ok("worktree.includeCopy", {"sourcePath": str(repo.path), "destPath": str(dest2)})
    assert not (dest2 / "big.bin").exists()
    _assert_budget_warning(res2)

    # Real entry-cap (maxEntries=3 over 6 real ignored files) & >1,000 .worktreeinclude path cap
    entry_files = [f"entry_{i}.tmp" for i in range(6)]
    for ef in entry_files:
        repo.create_ignored_file(ef, f"data-{ef}\n", ignore_pattern="*.tmp")
    repo.write_worktreeinclude(entry_files, commit=True)
    dest3 = temp_workspace.mkdir("wt_entries_dest")
    res3 = orca_api.assert_rpc_ok("worktree.includeCopy", {"sourcePath": str(repo.path), "destPath": str(dest3), "maxEntries": 3})
    _assert_budget_warning(res3)
    copied3 = [f for f in entry_files if (dest3 / f).exists()]
    assert int(res3.get("copiedCount", len(copied3))) <= 3 and len(copied3) <= 3 and len(copied3) < len(entry_files)

    tail_rel = f".cache/f{WORKTREE_INCLUDE_MAX_PATHS}.tmp"
    repo.create_ignored_file(tail_rel, "overflow\n", ignore_pattern=".cache/*.tmp")
    repo.write_worktreeinclude([f".cache/f{i}.tmp" for i in range(WORKTREE_INCLUDE_MAX_PATHS + 1)], commit=True)
    dest4 = temp_workspace.mkdir("wt_paths_dest")
    res4 = orca_api.assert_rpc_ok("worktree.includeCopy", {"sourcePath": str(repo.path), "destPath": str(dest4)})
    assert not (dest4 / tail_rel).exists()
    assert len(res4.get("warnings", [])) >= 1 or len(res4.get("skipped", [])) >= 1


@pytest.mark.tier2
@pytest.mark.f10
@pytest.mark.edge_case("SM1-14")
def test_t2_f10_windows_setup_script_posix_shebang_rejected_without_git_bash(
    git_repo: GitRepoFixture, orca_api: OrcaApiClient
) -> None:
    git_repo.commit_file("README.md", "win\n", msg="init")
    for shebang in ("#!/usr/bin/env bash", "#!/bin/sh"):
        script = f"{shebang}\necho BAD > marker.txt\n"
        yaml_text = f"scripts:\n  setup: |\n    {shebang}\n    echo BAD > marker.txt\n"
        yaml_path = git_repo.write_orca_yaml(yaml_text, commit=False)
        parsed = orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"path": str(yaml_path), "content": yaml_text}))
        trust_hash = _req_str(parsed, "setupTrustHash")
        err = orca_api.assert_rpc_error(
            "worktree.setup.run",
            "invalid_argument",
            {"worktreePath": str(git_repo.path), "orcaYamlPath": str(yaml_path), "script": script, "approvedTrustHash": trust_hash, "platform": "windows", "gitBashAvailable": False},
        )
        assert any(tok in str(err).lower() for tok in ("shebang", "bash", "cmd", "posix"))
        assert err.get("executed") is not True and not (git_repo.path / "marker.txt").exists()

        ok_res = orca_api.assert_rpc_ok(
            "worktree.setup.validate",
            {"worktreePath": str(git_repo.path), "script": script, "platform": "windows", "gitBashAvailable": True},
        )
        assert isinstance(ok_res, dict) and ok_res.get("valid") is True
        assert "bash" in str(ok_res.get("runner", ok_res.get("shell", ""))).lower()


@pytest.mark.tier2
@pytest.mark.f10
@pytest.mark.edge_case("SM2-22")
def test_t2_f10_orca_yaml_bomb_and_worktreeinclude_unsafe_via_rest_and_cli(
    temp_workspace: TempWorkspace, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    repo = GitRepoFixture(root=temp_workspace.mkdir("repo_sm222"))
    repo.commit_file("tracked.md", "tracked\n", msg="init")
    bomb_yaml = temp_workspace.write("bomb.yaml", _alias_bomb_yaml(ORCA_YAML_MAX_ALIAS_COUNT + 10))
    orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"path": str(bomb_yaml)}).assert_error_code("yaml_alias_limit", 400, 422)
    orca_cli.run("worktree", "parse-yaml", "--path", str(bomb_yaml)).assert_error("yaml_alias_limit")

    dup_yaml = temp_workspace.write("dup.yaml", "scripts:\n  setup: echo 1\n  setup: echo 2\n")
    orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"path": str(dup_yaml)}).assert_error_code("yaml_duplicate_key", 400, 422)
    orca_cli.run("worktree", "parse-yaml", "--path", str(dup_yaml)).assert_error("yaml_duplicate_key")

    esc = temp_workspace.write("escape", "ESCAPED\n")
    assert (repo.path / "../escape").resolve() == esc.resolve() and esc.exists()
    repo.create_ignored_file("a.secret", "S\n", ignore_pattern="*.secret")
    repo.create_ignored_file("xfoo", "F\n", ignore_pattern="xfoo")
    repo.create_ignored_file("bar", "B\n", ignore_pattern="bar")
    unsafe_lines = ["*.secret", "?foo", "!bar", "../escape", ".git/HEAD", "tracked.md"]
    repo.write_worktreeinclude(unsafe_lines, commit=True)
    dest = temp_workspace.mkdir("dest_wrap_2/wt_sm222")
    cli_copy = orca_cli.run("worktree", "include-copy", "--source", str(repo.path), "--dest", str(dest)).assert_ok()
    assert isinstance(cli_copy, dict) and cli_copy.get("copiedCount") == 0
    assert list(dest.iterdir()) == [] and not (dest.parent / "escape").exists()
    assert len(cli_copy.get("skipped", [])) == len(unsafe_lines) and set(unsafe_lines).issubset(_skipped_set(cli_copy))


@pytest.mark.tier2
@pytest.mark.f10
def test_t2_f10_sparse_preset_traversal_and_unsaved_editor_cleanup_guard(
    git_repo: GitRepoFixture, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    git_repo.commit_file("src/main.rs", "fn main() {}\n", msg="init")
    orca_api.assert_rpc_error("worktree.sparsePreset.set", "invalid_argument", {"repoPath": str(git_repo.path), "name": "bad", "paths": ["../escape", "/etc/passwd", ".git"]})

    wt = orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/worktrees", json_body={"repoPath": str(git_repo.path), "name": _uid("wt-dirty"), "baseRef": "main"}))
    wt_id = _req_str(wt, "id", "worktreeId")
    orca_api.assert_rpc_ok("workspaceCleanup.reportUnsavedEditors", {"worktreeId": wt_id, "dirtyFiles": ["src/main.rs"]})
    rpc_run = orca_api.assert_rpc_ok("workspaceCleanup.run", {"worktreeIds": [wt_id]})
    assert wt_id in rpc_run.get("blockedByUnsavedEditors", [])
    cli_run = orca_cli.run("worktree", "cleanup", "--worktree", wt_id).assert_ok()
    assert isinstance(cli_run, dict) and wt_id in cli_run.get("blockedByUnsavedEditors", [])
    assert wt_id in str(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/worktrees")))


@pytest.mark.tier2
@pytest.mark.f10
def test_t2_f10_first_work_rename_published_guard_and_setup_trust_hash_reapproval(
    git_repo: GitRepoFixture, temp_workspace: TempWorkspace, orca_api: OrcaApiClient
) -> None:
    git_repo.commit_file("lib.rs", "pub fn v() {}\n", msg="init")
    bare = temp_workspace.mkdir("origin.git")
    subprocess.run([GitRepoFixture.GIT_BIN, "init", "--bare", str(bare)], check=True, capture_output=True)
    git_repo.git("remote", "add", "origin", str(bare))
    git_repo.create_branch("quiet-fox", checkout=True)
    git_repo.git("push", "-u", "origin", "quiet-fox")
    git_repo.git("checkout", "main")

    wt = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/worktrees",
            json_body={"repoPath": str(git_repo.path), "name": "quiet-fox", "branch": "quiet-fox", "autoGeneratedName": True},
        )
    )
    wt_id = _req_str(wt, "id", "worktreeId")
    res = orca_api.assert_rpc_ok("worktree.firstWorkRename", {"worktreeId": wt_id, "agentState": "working", "prompt": "Refactor auth"})
    assert res.get("renamed") is False
    assert any(k in str(res.get("reason", "")).lower() for k in ("upstream", "published", "push"))
    assert _req_str(res, "branch", "name") == "quiet-fox"

    v1 = orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"content": "scripts:\n  setup: echo v1\n"}))
    v2_path = git_repo.write_orca_yaml("scripts:\n  setup: echo v2\n", commit=False)
    orca_api.assert_rpc_error(
        "worktree.setup.run",
        "approval_required",
        {"worktreeId": wt_id, "orcaYamlPath": str(v2_path), "approvedTrustHash": _req_str(v1, "setupTrustHash")},
    )


# ---------------------------------------------------------------------------
# F11 Tier 2 — Unverifiable Liveness, Error Codes, Fencing, Federation & DAG
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f11
@pytest.mark.edge_case("SM1-10")
def test_t2_f11_worker_silence_timeout_unverifiable_liveness_refuses_retry(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("sm110")})), "id", "runId")
    task_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "SilentWorker"})), "id", "taskId")
    disp = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude"}))
    disp_id = _req_str(disp, "id", "dispatchId")

    orca_api.assert_rpc_ok(
        "orchestration.worker.reportLiveness",
        {"dispatchId": disp_id, "liveness": "unverifiable", "reason": "heartbeat_timeout"},
    )
    shown = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": disp_id})
    assert shown.get("liveness") == "unverifiable"
    assert shown.get("taskStatus", shown.get("status")) == "dispatched"

    retry_resp = orca_api.post(
        "/api/v1/orca/orchestration/workers/start",
        json_body={"runId": run_id, "taskId": task_id, "agent": "claude", "retryOnSilence": True},
    )
    retry_resp.assert_status(400, 409, 422)
    assert any(tok in retry_resp.text.lower() for tok in ("unverifiable", "liveness", "active", "conflict"))
    cli_retry = orca_cli.run("orchestration", "worker-start", "--run", run_id, "--task", task_id, "--agent", "claude")
    _assert_cli_domain_error(cli_retry)

    after_refusal = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": disp_id})
    assert int(after_refusal["activeDispatchCount"]) == 1
    assert after_refusal.get("taskStatus", after_refusal.get("status")) == "dispatched"

    settled = orca_cli.run("orchestration", "worker-done", "--dispatch", disp_id, "--summary", "confirmed exit").assert_ok()
    assert isinstance(settled, dict) and settled.get("status") in ("completed", "settled", "done")
    final_show = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": disp_id})
    assert final_show.get("taskStatus", final_show.get("status")) in ("completed", "settled")


@pytest.mark.tier2
@pytest.mark.f11
def test_t2_f11_unknown_task_and_unknown_dispatch_errors_rest_rpc_cli(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    assert {"unknown_task", "unknown_dispatch"}.issubset(IDE_ERROR_CODES)
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("unk")})), "id", "runId")
    orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": "task-nonexistent-999", "agent": "claude"}).assert_error_code("unknown_task", 400, 404)
    orca_cli.run("orchestration", "worker-start", "--run", run_id, "--task", "task-nonexistent-999").assert_error("unknown_task")
    orca_api.post("/api/v1/orca/orchestration/messages", json_body={"runId": run_id, "to": "dispatch:disp-missing-999", "type": "status", "body": "ping"}).assert_error_code("unknown_dispatch", 400, 404)
    orca_cli.run("orchestration", "worker-done", "--dispatch", "disp-missing-999").assert_error("unknown_dispatch")
    orca_api.assert_rpc_error("orchestration.worker.read", "unknown_dispatch", {"dispatchId": "disp-missing-999"})


@pytest.mark.tier2
@pytest.mark.f11
def test_t2_f11_stale_dispatch_and_consumer_generation_fencing(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    assert "stale_dispatch" in IDE_ERROR_CODES
    run = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("fence")}))
    run_id, gen = _req_str(run, "id", "runId"), int(run["consumerGeneration"])
    task_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "FenceTask"})), "id", "taskId")
    d1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude"})), "id", "dispatchId")
    orca_cli.run("orchestration", "worker-done", "--dispatch", d1, "--summary", "v1").assert_ok()
    d2 = _req_str(orca_cli.run("orchestration", "send", "--run", run_id, "--to", f"dispatch:{d1}", "--body", "reopen").assert_ok(), "dispatchId", "id")
    assert d2 != d1
    orca_api.post("/api/v1/orca/orchestration/messages", json_body={"runId": run_id, "to": f"dispatch:{d1}", "consumerGeneration": gen, "type": "status", "body": "stale", "strictGeneration": True}).assert_error_code("stale_dispatch", 400, 409)
    orca_api.assert_rpc_error("orchestration.worker.done", "stale_dispatch", {"dispatchId": d1, "consumerGeneration": gen})
    orca_cli.run("orchestration", "worker-done", "--dispatch", d1, "--generation", str(gen)).assert_error("stale_dispatch")
    d2_show = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": d2})
    assert d2_show.get("status") in ("dispatched", "active", "running")


@pytest.mark.tier2
@pytest.mark.f11
def test_t2_f11_federation_mailbox_is_local_and_server_mail_unsupported(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    assert {"mailbox_is_local", "server_mail_unsupported"}.issubset(IDE_ERROR_CODES)
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("fed-err")})), "id", "runId")
    task_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "LocalOnly"})), "id", "taskId")
    d_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude"})), "id", "dispatchId")

    orca_api.assert_rpc_error(
        "orchestration.federation.sendPeerMail",
        "mailbox_is_local",
        {"runId": run_id, "dispatchId": d_id, "body": "remote-attempt"},
    )
    orca_api.post(
        "/api/v1/orca/orchestration/messages",
        json_body={"runId": run_id, "to": f"dispatch:{d_id}", "federatedOnly": True, "type": "status", "body": "x"},
    ).assert_error_code("mailbox_is_local", 400, 409, 422)

    legacy = _uid("legacy-peer")
    orca_api.assert_rpc_ok("orchestration.federation.registerPeer", {"serverId": legacy, "supportsServerMail": False, "protocolVersion": 2})
    orca_api.post(
        "/api/v1/orca/orchestration/messages",
        json_body={"runId": run_id, "targetServer": legacy, "to": "coordinator", "type": "status", "body": "hi"},
    ).assert_error_code("server_mail_unsupported", 400, 409, 422)
    orca_cli.run("orchestration", "send", "--run", run_id, "--on", legacy, "--to", "coordinator", "--body", "hi").assert_error("server_mail_unsupported")


@pytest.mark.tier2
@pytest.mark.f11
def test_t2_f11_nesting_depth_fail_closed_and_failed_setup_keeps_terminal_open(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("nest")})), "id", "runId")
    t1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "L1"})), "id", "taskId")
    t2 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "L2"})), "id", "taskId")
    d1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": t1, "agent": "claude"})), "id", "dispatchId")

    orca_api.post(
        "/api/v1/orca/orchestration/workers/start",
        json_body={"runId": run_id, "taskId": t2, "agent": "claude", "callerDispatchId": d1},
    ).assert_status(400, 403)
    cli_nest = orca_cli.run("orchestration", "worker-start", "--run", run_id, "--task", t2, "--caller-dispatch", d1)
    _assert_cli_domain_error(cli_nest)

    d2 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": t2, "agent": "codex", "keepOpen": False})), "id", "dispatchId")
    orca_api.assert_rpc_ok("orchestration.worker.reportSetupFailure", {"dispatchId": d2, "setupFailed": True, "reason": "exit 1"})
    st = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": d2})
    assert st.get("setupFailed") is True and st.get("terminalClosed") is False


@pytest.mark.tier2
@pytest.mark.f11
def test_t2_f11_mutation_receipt_payload_conflict_and_dag_cycle_rejection(
    orca_api: OrcaApiClient,
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("dag-err")})), "id", "runId")
    req_id = _uid("req-conflict")
    t1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Original", "operationId": req_id, "requestId": req_id})), "id", "taskId")
    conflict = orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "MutatedTitle", "operationId": req_id, "requestId": req_id}).assert_status(400, 409)
    assert conflict.extract_error_code() in ("invalid_argument", "conflict", "payload_conflict")
    t2 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Second", "deps": [t1]})), "id", "taskId")
    orca_api.assert_rpc_error("orchestration.task.updateDeps", "invalid_argument", {"taskId": t1, "deps": [t1]})
    orca_api.assert_rpc_error("orchestration.task.updateDeps", "invalid_argument", {"taskId": t1, "deps": [t2]})
    tasks = orca_api.assert_rpc_ok("orchestration.task.list", {"runId": run_id})
    assert "Original" in str(tasks) and "MutatedTitle" not in str(tasks)
