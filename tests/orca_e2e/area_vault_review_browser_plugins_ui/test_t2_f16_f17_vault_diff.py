"""Tier 2 boundary, negative, security & edge-case E2E tests for F16 and F17."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from harness import GitRepoFixture, OrcaApiClient, OrcaCliRunner, OrcaServerHandle, SqliteFixtureHelper, TempWorkspace


def _rpc_err(
    api: OrcaApiClient, method: str, expected_code: str, params: dict[str, Any] | None = None, *, operation_id: str | None = None
) -> dict[str, Any]:
    resp = api.rpc(method, params=params, operation_id=operation_id)
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status}): {resp.text[:300]}"
    return api.assert_rpc_error(resp, expected_code)


def _rpc_err_in(
    api: OrcaApiClient, method: str, allowed_codes: tuple[str, ...], params: dict[str, Any] | None = None
) -> dict[str, Any]:
    resp = api.rpc(method, params=params)
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status}): {resp.text[:300]}"
    payload = resp.json()
    assert isinstance(payload, dict) and payload.get("ok") is False, payload
    err = payload.get("error")
    assert isinstance(err, dict) and err.get("code") in allowed_codes, (allowed_codes, payload)
    return err


def _extract_list(payload: Any, *keys: str) -> list[dict[str, Any]]:
    if isinstance(payload, list):
        return [x for x in payload if isinstance(x, dict)]
    if isinstance(payload, dict):
        for k in (*keys, "items", "results", "hits", "annotations", "tasks"):
            if isinstance(payload.get(k), list):
                return [x for x in payload[k] if isinstance(x, dict)]
    raise AssertionError(f"Expected list under {keys!r}, got: {payload!r}")


@pytest.mark.tier2
@pytest.mark.f16
def test_f16_t2_profile_state_optimistic_revision_conflict_rejected(
    orca_api: OrcaApiClient,
) -> None:
    """F16 T2: Stale expected_revision write is rejected while newer revision stays intact."""
    v1 = orca_api.assert_rpc_ok(
        "profileState.write", {"domain": "worktreeMeta", "payload": {"state": "initial"}}
    )
    rev1 = int(v1["revision"])
    v2 = orca_api.assert_rpc_ok(
        "profileState.write",
        {"domain": "worktreeMeta", "expected_revision": rev1, "payload": {"state": "fresh"}},
    )
    assert int(v2["revision"]) == rev1 + 1

    _rpc_err_in(
        orca_api,
        "profileState.write",
        ("revision_conflict", "invalid_argument"),
        {"domain": "worktreeMeta", "expected_revision": rev1, "payload": {"state": "stale-overwrite"}},
    )
    cur = orca_api.assert_rpc_ok("profileState.read", {"domain": "worktreeMeta"})
    assert int(cur["revision"]) == rev1 + 1 and "fresh" in json.dumps(cur)
    assert "stale-overwrite" not in json.dumps(cur)


@pytest.mark.tier2
@pytest.mark.f16
@pytest.mark.edge_case("SM1-03")
def test_f16_t2_sm1_03_profile_state_startup_divergence_and_selective_repair(
    fresh_orca_server: OrcaServerHandle, cli_bin_path: Path, temp_workspace: TempWorkspace
) -> None:
    """F16 T2 [SM1-03]: Corrupt ProfileState DB + diverged JSON surfaces breadcrumb and repairs cleanly."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)

    api.assert_rpc_ok("profileState.write", {"domain": "worktreeMeta", "payload": {"branch": "release/sm1-03"}})
    api.assert_rpc_ok("profileState.write", {"domain": "ui", "payload": {"theme": "night"}})
    exported = api.assert_rpc_ok("profileState.export", {})
    backup_path = temp_workspace.write("profile_backup.json", json.dumps(exported))
    legacy_path = temp_workspace.write("legacy_diverged.json", json.dumps({"ui": {"theme": "diverged"}}))
    api.assert_rpc_ok("profileState.write", {"domain": "ui", "payload": {"theme": "pre-rollback-drift"}})

    info = api.assert_rpc_ok("profileState.inspectRecovery", {})
    raw_db = info.get("profile_db_path")
    assert isinstance(raw_db, str) and raw_db.strip(), info
    profile_db = Path(raw_db)
    assert profile_db.is_file(), f"Missing ProfileState DB: {profile_db}"
    SqliteFixtureHelper.corrupt_header(profile_db, length=100)

    rec = api.assert_rpc_ok(
        "profileState.inspectRecovery",
        {"profile_db_path": str(profile_db), "legacy_json_path": str(legacy_path)},
    )
    assert rec.get("corrupted") is True or rec.get("diverged") is True or rec.get("recovery_required") is True, rec
    bc = rec.get("breadcrumb") or rec.get("crash_breadcrumb")
    assert (isinstance(bc, str) and bc.strip()) or (isinstance(bc, dict) and bool(bc)), rec
    raw_acts = rec.get("actions") if isinstance(rec.get("actions"), list) else rec.get("options")
    assert isinstance(raw_acts, list) and raw_acts, rec
    act_names = " ".join(
        str(a.get("id") or a.get("action") or a.get("name") or "") if isinstance(a, dict) else str(a) for a in raw_acts
    ).lower()
    assert "repair" in act_names or "rollback" in act_names, raw_acts

    cli = OrcaCliRunner(cli_bin=cli_bin_path, server_url=fresh_orca_server.base_url, token=sess.token, work_dir=temp_workspace.mkdir("cli_sm1_03"))
    cli_rec = cli.run("ide", "profile-state", "--recover-status").assert_ok()
    assert isinstance(cli_rec, dict) and (cli_rec.get("corrupted") is True or cli_rec.get("diverged") is True or cli_rec.get("recovery_required") is True), cli_rec
    assert cli_rec.get("breadcrumb") or cli_rec.get("crash_breadcrumb"), cli_rec

    api.assert_rpc_ok("profileState.rollback", {"backup_path": str(backup_path)})
    fresh_orca_server.restart()
    assert profile_db.is_file() and not profile_db.read_bytes().startswith(b"\xffORCA_E2E_CORRUPT_HEADER"), profile_db
    api_after = fresh_orca_server.api_client(sess)
    assert "release/sm1-03" in json.dumps(api_after.assert_rpc_ok("profileState.read", {"domain": "worktreeMeta"}))
    ui_blob = json.dumps(api_after.assert_rpc_ok("profileState.read", {"domain": "ui"}))
    assert "night" in ui_blob and "pre-rollback-drift" not in ui_blob, ui_blob


@pytest.mark.tier2
@pytest.mark.f16
@pytest.mark.edge_case("SM1-04")
def test_f16_t2_sm1_04_atomic_snapshot_writer_quit_race_fences_stale_async_write(
    fresh_orca_server: OrcaServerHandle, temp_workspace: TempWorkspace
) -> None:
    """F16 T2 [SM1-04]: Sync quit flush fences stale in-flight async generation and unlinks *.tmp."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)
    snap_path = temp_workspace.resolve("quit_race.json")

    staged = api.assert_rpc_ok(
        "profileState.snapshotWrite",
        {"path": str(snap_path), "stage_async": True, "payload": {"state": "stale-async-clobber"}},
    )
    gen_stale = int(staged["generation"])

    flushed = api.assert_rpc_ok(
        "profileState.snapshotWrite",
        {"path": str(snap_path), "write_sync": True, "payload": {"state": "quit-flush-authoritative"}},
    )
    assert int(flushed["committed_generation"]) >= gen_stale + 1

    fin_resp = api.rpc(
        "profileState.snapshotWrite",
        {"path": str(snap_path), "finalize_generation": gen_stale, "payload": {"state": "stale-async-clobber"}},
    )
    assert fin_resp.status not in (404, 501), f"Missing/unimplemented RPC ({fin_resp.status})"
    fin_json = fin_resp.json()
    assert isinstance(fin_json, dict)
    if fin_json.get("ok") is True:
        assert fin_resp.status == 200 and isinstance(fin_json.get("result"), dict) and fin_json["result"].get("fenced") is True, fin_json
    else:
        assert fin_json.get("ok") is False and isinstance(fin_json.get("error"), dict) and fin_json["error"].get("code") in ("stale_generation", "revision_conflict", "invalid_argument"), fin_json

    assert json.loads(snap_path.read_text(encoding="utf-8"))["state"] == "quit-flush-authoritative"
    assert list(snap_path.parent.glob("*.tmp")) == []

    fresh_orca_server.restart()
    api_after = fresh_orca_server.api_client(sess)
    assert "quit-flush-authoritative" in json.dumps(
        api_after.assert_rpc_ok("profileState.snapshotRead", {"path": str(snap_path)})
    )


@pytest.mark.tier2
@pytest.mark.f16
@pytest.mark.edge_case("SM1-05")
def test_f16_t2_sm1_05_protected_secret_persistence_degraded_keyring_retains_ciphertext(
    fresh_orca_server: OrcaServerHandle, temp_workspace: TempWorkspace
) -> None:
    """F16 T2 [SM1-05]: Locked keyring refuses basic_text, retains ciphertext, leaks zero plaintext."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)
    sentinel = "sk-orca-e2e-never-plaintext-9911"

    saved = api.assert_rpc_ok(
        "profileState.secretsSave", {"slot": "provider_api_key", "secret": sentinel}
    )
    orig_digest = saved.get("ciphertext_digest") or saved.get("sealed_digest")
    assert isinstance(orig_digest, str) and orig_digest.strip() and sentinel not in orig_digest, saved

    _rpc_err_in(
        api,
        "profileState.secretsSave",
        ("keyring_unavailable", "invalid_argument"),
        {"slot": "provider_api_key", "secret": sentinel, "keyring_backend": "basic_text"},
    )
    status = api.assert_rpc_ok(
        "profileState.secretsStatus", {"simulate_locked": True, "mode": "serve_degraded"}
    )
    assert status.get("degraded") is True

    api.assert_rpc_ok(
        "profileState.write",
        {"domain": "ui", "payload": {"theme": "dark"}, "keyring_mode": "serve_degraded"},
    )
    after = api.assert_rpc_ok(
        "profileState.secretsStatus", {"simulate_locked": True, "mode": "serve_degraded"}
    )
    assert after.get("degraded") is True
    assert (int(after["sealed_slots_count"]) if after.get("sealed_slots_count") is not None else len(after.get("sealed_slots") or [])) >= 1, after
    assert (after.get("ciphertext_digest") or after.get("sealed_digest")) == orig_digest, (orig_digest, after)

    if not fresh_orca_server.is_external:
        needle = sentinel.encode("utf-8")
        scanned = [p for r in (fresh_orca_server.data_dir, temp_workspace.path) for p in r.rglob("*") if p.is_file()]
        assert scanned, "Expected server data_dir files to scan"
        for p in scanned:
            assert needle not in p.read_bytes(), f"Plaintext secret leaked to {p}"


@pytest.mark.tier2
@pytest.mark.f16
@pytest.mark.edge_case("SM1-13")
def test_f16_t2_sm1_13_ai_vault_corrupt_fts_cache_auto_rebuilds_once(
    fresh_orca_server: OrcaServerHandle, temp_workspace: TempWorkspace
) -> None:
    """F16 T2 [SM1-13]: Corrupt FTS5 cache header triggers single auto-rebuild and restores search."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)

    t_path = temp_workspace.write(
        "transcripts/sm1_13.jsonl",
        json.dumps({"role": "assistant", "text": "CorruptCacheAutoRebuildToken88"}) + "\n",
    )
    api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    api.assert_rpc_ok(
        "aiVault.index",
        {"agent": "claude", "session_id": "vault-sm1-13", "transcript_path": str(t_path), "workspace_path": str(temp_workspace.path)},
    )
    pre_hits = _extract_list(
        api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "CorruptCacheAutoRebuildToken88"})),
        "hits",
    )
    assert any(h.get("session_id") == "vault-sm1-13" for h in pre_hits)

    raw_cache = api.assert_rpc_ok("aiVault.status", {}).get("cache_db_path")
    assert isinstance(raw_cache, str) and raw_cache.strip(), raw_cache
    cache_db = Path(raw_cache)
    assert cache_db.is_file(), f"Missing AI Vault cache DB: {cache_db}"
    fresh_orca_server.stop(cleanup=False)
    SqliteFixtureHelper.corrupt_header(cache_db, length=100)
    fresh_orca_server.start()
    api_after = fresh_orca_server.api_client(sess)

    post_hits = _extract_list(
        api_after.assert_ok(api_after.get("/api/v1/orca/ide/vault/search", params={"q": "CorruptCacheAutoRebuildToken88"})),
        "hits",
    )
    assert any(h.get("session_id") == "vault-sm1-13" for h in post_hits)
    st = api_after.assert_rpc_ok("aiVault.status", {})
    assert type(st.get("rebuild_count")) is int and st["rebuild_count"] == 1 and cache_db.is_file() and not cache_db.read_bytes().startswith(b"\xffORCA_E2E_CORRUPT_HEADER"), st


@pytest.mark.tier2
@pytest.mark.f16
def test_f16_t2_ai_vault_opt_out_default_prefix_mismatch_and_tool_purge(
    fresh_orca_server: OrcaServerHandle, temp_workspace: TempWorkspace
) -> None:
    """F16 T2: AI Vault defaults opt-out, re-indexes on prefix_hash mismatch, and purges tool text."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)

    st0 = api.assert_rpc_ok("aiVault.status", {})
    assert st0.get("opt_in") is False

    t_path = temp_workspace.write(
        "transcripts/purge.jsonl",
        json.dumps({"role": "user", "text": "OldHeaderToken analysis"}) + "\n"
        + json.dumps({"role": "tool", "text": "EphemeralToolDump99"}) + "\n",
    )
    api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    idx1 = api.assert_rpc_ok(
        "aiVault.index",
        {"agent": "claude", "session_id": "vault-purge-1", "transcript_path": str(t_path), "workspace_path": str(temp_workspace.path)},
    )
    assert _extract_list(api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "OldHeaderToken"})), "hits")
    t_path.write_text(
        json.dumps({"role": "user", "text": "RewrittenHeaderToken"}) + "\n"
        + json.dumps({"role": "tool", "text": "EphemeralToolDump99"}) + "\n",
        encoding="utf-8",
    )
    idx2 = api.assert_rpc_ok(
        "aiVault.index",
        {"agent": "claude", "session_id": "vault-purge-1", "transcript_path": str(t_path), "workspace_path": str(temp_workspace.path)},
    )
    assert isinstance(idx1["prefix_hash"], str) and idx1["prefix_hash"].strip() and isinstance(idx2["prefix_hash"], str) and idx2["prefix_hash"].strip() and idx2["prefix_hash"] != idx1["prefix_hash"]
    assert not _extract_list(api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "OldHeaderToken"})), "hits")
    assert _extract_list(api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "RewrittenHeaderToken"})), "hits")
    assert _extract_list(api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "EphemeralToolDump99"})), "hits")

    api.assert_rpc_ok("aiVault.purgeToolOutput", {"session_id": "vault-purge-1"})
    assert not _extract_list(api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "EphemeralToolDump99"})), "hits")
    assert _extract_list(api.assert_ok(api.get("/api/v1/orca/ide/vault/search", params={"q": "RewrittenHeaderToken"})), "hits")


@pytest.mark.tier2
@pytest.mark.f17
def test_f17_t2_human_edit_flips_ai_attribution_back_without_commit_pollution(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T2: Human edit flips sub-range attribution to human with zero Git commit pollution."""
    lines = [f"let h{i} = {i};" for i in range(1, 21)]
    git_repo.commit_file("src/handler.rs", "\n".join(lines) + "\n", msg="handler base")
    orca_api.assert_rpc_ok(
        "diffReview.recordAttribution",
        {"repo_path": str(git_repo.path), "file": "src/handler.rs", "author_kind": "ai", "agent": "claude", "start_line": 10, "end_line": 15},
    )
    orca_api.assert_rpc_ok(
        "diffReview.applyHumanEdit",
        {"repo_path": str(git_repo.path), "file": "src/handler.rs", "start_line": 11, "end_line": 12},
    )
    attr = orca_api.assert_rpc_ok(
        "diffReview.getAttribution", {"repo_path": str(git_repo.path), "file": "src/handler.rs"}
    )
    by_line = {int(e["line"]): e["author_kind"] for e in _extract_list(attr, "lines", "entries")}
    assert by_line[11] == "human" and by_line[12] == "human"
    assert all(by_line[ln] == "ai" for ln in (10, 13, 14, 15))

    lines[10] = "let h11 = 111;"
    git_repo.commit_file("src/handler.rs", "\n".join(lines) + "\n", msg="human tweak")
    stat = git_repo.git("show", "--stat", "HEAD").stdout
    body = git_repo.git("log", "-1", "--format=%B").stdout
    assert "attribution" not in stat.lower() and "attribution" not in body.lower()


@pytest.mark.tier2
@pytest.mark.f17
def test_f17_t2_invalid_anchor_rejected_and_deleted_hunk_marks_orphaned(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, git_repo: GitRepoFixture
) -> None:
    """F17 T2: Bad line/traversal/empty body rejected; deleted hunk marks annotation orphaned."""
    lines = [f"fn k{i}() {{}}" for i in range(1, 13)]
    git_repo.commit_file("src/hunk.rs", "\n".join(lines) + "\n", msg="hunk base")

    for bad_body in (
        {"repo_path": str(git_repo.path), "file": "src/hunk.rs", "line": 0, "body": "bad line"},
        {"repo_path": str(git_repo.path), "file": "../../etc/passwd", "line": 1, "body": "traversal"},
        {"repo_path": str(git_repo.path), "file": "src/hunk.rs", "line": 2, "body": "   "},
    ):
        orca_api.post("/api/v1/orca/ide/diff-annotations", json_body=bad_body).assert_error_code(
            "invalid_argument", 400, 422
        )
    orca_cli.run(
        "ide", "diff-annotate", "--repo", str(git_repo.path), "--file", "../../etc/passwd", "--line", "1", "--body", "x"
    ).assert_error("invalid_argument")

    ann = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/diff-annotations",
            json_body={"repo_path": str(git_repo.path), "file": "src/hunk.rs", "line": 6, "body": "Watch hunk"},
        )
    )
    git_repo.write_file("src/hunk.rs", "\n".join(lines[:3] + lines[9:]) + "\n")
    orca_api.assert_rpc_ok("diffReview.reanchor", {"repo_path": str(git_repo.path), "file": "src/hunk.rs"})
    listed = _extract_list(
        orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/diff-annotations", params={"repo_path": str(git_repo.path)})),
        "annotations",
    )
    assert ann.get("id") not in (None, ""), ann
    rec = next(a for a in listed if a.get("id") not in (None, "") and str(a["id"]) == str(ann["id"]))
    assert rec.get("orphaned") is True or rec.get("anchor_status") == "orphaned"


@pytest.mark.tier2
@pytest.mark.f17
def test_f17_t2_diff_batch_rejects_empty_unresolved_set(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, git_repo: GitRepoFixture
) -> None:
    """F17 T2: Empty unresolved annotation set fails closed on CLI diff-batch and RPC sendBatch."""
    git_repo.commit_file("src/clean.rs", "pub fn clean() {}\n", msg="clean file")
    empty_resp = orca_api.get("/api/v1/orca/ide/diff-annotations", params={"repo_path": str(git_repo.path)}).assert_status(200)
    assert _extract_list(orca_api.assert_ok(empty_resp), "annotations") == []
    orca_cli.run("ide", "diff-batch", "--repo", str(git_repo.path)).assert_error("invalid_argument")
    _rpc_err(orca_api, "diffReview.sendBatch", "invalid_argument", {"repo_path": str(git_repo.path)})


@pytest.mark.tier2
@pytest.mark.f17
def test_f17_t2_diff_annotation_idempotent_batch_and_unknown_resolve_not_found(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T2: Same operation_id returns identical batch receipt; unknown resolve returns RPC not_found."""
    git_repo.commit_file("src/idem.rs", "pub fn idem() {}\n", msg="idem base")
    orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/diff-annotations",
            json_body={"repo_path": str(git_repo.path), "file": "src/idem.rs", "line": 1, "body": "Check idempotency"},
        )
    )
    op_id = "op-diff-batch-idem-1001"
    r1 = orca_api.assert_rpc_ok("diffReview.sendBatch", {"repo_path": str(git_repo.path)}, operation_id=op_id)
    r2 = orca_api.assert_rpc_ok("diffReview.sendBatch", {"repo_path": str(git_repo.path)}, operation_id=op_id)
    assert isinstance(r1, dict) and bool(r1) and r1 == r2 and any(r1.get(k) not in (None, "", 0, []) for k in ("batch_id", "receipt_id", "id", "prompt", "operation_id", "count", "sent_count", "annotations")), (r1, r2)

    _rpc_err(orca_api, "diffReview.resolve", "not_found", {"annotation_id": "ann-missing-999"})


@pytest.mark.tier2
@pytest.mark.f17
def test_f17_t2_tasks_hub_jira_key_normalization_and_provider_visibility_toggle(
    orca_api: OrcaApiClient,
) -> None:
    """F17 T2: Lowercase Jira key normalizes/dedupes to ENG-42 and provider visibility hides/restores."""
    for raw_key in ("eng-42", "ENG-42"):
        orca_api.assert_rpc_ok(
            "tasksHub.ingest",
            {"provider": "jira", "key": raw_key, "title": "Normalize key", "url": "https://jira.test/ENG-42"},
        )
    orca_api.assert_rpc_ok(
        "tasksHub.ingest",
        {"provider": "github", "key": "GH-7", "title": "Keep visible", "url": "https://github.com/a/b/issues/7"},
    )
    all_tasks = _extract_list(orca_api.assert_rpc_ok("tasksHub.list", {}), "tasks")
    eng_rows = [t for t in all_tasks if isinstance(t.get("key"), str) and t["key"].upper() == "ENG-42"]
    assert len(eng_rows) == 1 and eng_rows[0]["key"] == "ENG-42"

    orca_api.assert_rpc_ok("tasksHub.setProviderVisibility", {"provider": "jira", "visible": False})
    hidden_keys = {str(t.get("key")) for t in _extract_list(orca_api.assert_rpc_ok("tasksHub.list", {}), "tasks")}
    assert "ENG-42" not in hidden_keys and "GH-7" in hidden_keys

    orca_api.assert_rpc_ok("tasksHub.setProviderVisibility", {"provider": "jira", "visible": True})
    restored_keys = {str(t.get("key")) for t in _extract_list(orca_api.assert_rpc_ok("tasksHub.list", {}), "tasks")}
    assert "ENG-42" in restored_keys and "GH-7" in restored_keys


@pytest.mark.tier2
@pytest.mark.f17
def test_f17_t2_source_control_blocks_path_traversal_and_empty_commit(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T2: git.stage/discard block ../outside.txt and git.commit rejects empty staged set."""
    git_repo.commit_file("src/ok.rs", "pub fn ok() {}\n", msg="ok baseline")
    head_before = int(git_repo.git("rev-list", "--count", "HEAD").stdout.strip())
    assert isinstance(orca_api.assert_rpc_ok("git.status", {"repo_path": str(git_repo.path)}), dict)
    _rpc_err(orca_api, "git.stage", "invalid_argument", {"repo_path": str(git_repo.path), "paths": ["../outside.txt"]})
    _rpc_err(orca_api, "git.discard", "invalid_argument", {"repo_path": str(git_repo.path), "paths": ["../outside.txt"]})
    _rpc_err_in(
        orca_api,
        "git.commit",
        ("invalid_argument", "empty_commit"),
        {"repo_path": str(git_repo.path), "message": "must fail when nothing staged"},
    )
    assert int(git_repo.git("rev-list", "--count", "HEAD").stdout.strip()) == head_before
