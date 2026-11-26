"""Tier 1 opaque-box E2E tests for F16 (ProfileState & AI Vault) and F17 (Diff Review & Tasks)."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

import pytest

from harness import GitRepoFixture, OrcaApiClient, OrcaCliRunner, OrcaServerHandle, TempWorkspace

_HEX64_RE = re.compile(r"^[0-9a-fA-F]{64}$")


def _extract_items(payload: Any, *keys: str) -> list[dict[str, Any]]:
    if isinstance(payload, list):
        return [item for item in payload if isinstance(item, dict)]
    if isinstance(payload, dict):
        for key in (*keys, "items", "results", "hits", "annotations", "tasks", "parts"):
            val = payload.get(key)
            if isinstance(val, list):
                return [item for item in val if isinstance(item, dict)]
    raise AssertionError(f"Expected list payload under {keys!r}, got: {payload!r}")


def _write_transcript(ws: TempWorkspace, rel: str, turns: list[dict[str, str]]) -> Path:
    return ws.write(rel, "\n".join(json.dumps(t, sort_keys=True) for t in turns) + "\n")


def _list_annotations(api: OrcaApiClient, repo_path: Path) -> dict[str, dict[str, Any]]:
    resp = api.get("/api/v1/orca/ide/diff-annotations", params={"repo_path": str(repo_path)})
    items = _extract_items(api.assert_ok(resp), "annotations")
    assert all(a.get("id") not in (None, "") for a in items), items
    return {str(a["id"]): a for a in items}


@pytest.mark.tier1
@pytest.mark.f16
def test_f16_t1_profile_state_domain_partitioned_dirty_write_and_revision(
    fresh_orca_server: OrcaServerHandle, cli_bin_path: Path, temp_workspace: TempWorkspace
) -> None:
    """F16 T1: Domain-partitioned writes bump only dirty domain revision and survive restart."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)

    wt_v1 = api.assert_rpc_ok(
        "profileState.write",
        {"domain": "worktreeMeta", "payload": {"branches": ["main", "feat/orca"], "pinned": ["main"]}},
    )
    ui_v1 = api.assert_rpc_ok(
        "profileState.write",
        {"domain": "ui", "payload": {"theme": "night", "sidebar_width": 280}},
    )
    rev_wt, rev_ui = int(wt_v1["revision"]), int(ui_v1["revision"])
    assert rev_wt >= 1 and rev_ui >= 1
    assert _HEX64_RE.match(str(wt_v1["content_hash"])) and _HEX64_RE.match(str(ui_v1["content_hash"]))

    ui_before = api.assert_rpc_ok("profileState.read", {"domain": "ui"})
    wt_v2 = api.assert_rpc_ok(
        "profileState.write",
        {
            "domain": "worktreeMeta",
            "expected_revision": rev_wt,
            "payload": {"branches": ["main", "feat/orca", "fix/diff"], "pinned": ["main"]},
        },
    )
    assert int(wt_v2["revision"]) == rev_wt + 1 and _HEX64_RE.match(str(wt_v2["content_hash"])) and wt_v2["content_hash"] != wt_v1["content_hash"]

    ui_after = api.assert_rpc_ok("profileState.read", {"domain": "ui"})
    assert int(ui_after["revision"]) == rev_ui
    assert ui_after["content_hash"] == ui_before["content_hash"] == ui_v1["content_hash"]
    assert ui_before.get("updated_at") not in (None, "") and ui_after.get("updated_at") == ui_before["updated_at"]

    fresh_orca_server.restart()
    api_after = fresh_orca_server.api_client(sess)
    cli = OrcaCliRunner(
        cli_bin=cli_bin_path,
        server_url=fresh_orca_server.base_url,
        token=sess.token,
        work_dir=temp_workspace.mkdir("cli_f16_profile"),
    )
    cli_state = cli.run("ide", "profile-state", "--domain", "worktreeMeta").assert_ok()
    assert isinstance(cli_state, dict) and int(cli_state["revision"]) == rev_wt + 1
    assert "fix/diff" in json.dumps(cli_state)
    assert int(api_after.assert_rpc_ok("profileState.read", {"domain": "ui"})["revision"]) == rev_ui


@pytest.mark.tier1
@pytest.mark.f16
def test_f16_t1_profile_state_split_document_parts_and_export_roundtrip(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    """F16 T1: Split-document parts preserve ordinal ordering, hashes, and export fidelity."""
    parts_in = [
        {"part_key": "repo-a", "ordinal": 0, "payload": {"name": "orca-core", "branch": "main"}},
        {"part_key": "repo-b", "ordinal": 1, "payload": {"name": "gpui-telegram", "branch": "main"}},
    ]
    assert isinstance(
        orca_api.assert_rpc_ok("profileState.writeParts", {"domain": "repos", "parts": parts_in}),
        dict,
    )
    read_parts = _extract_items(
        orca_api.assert_rpc_ok("profileState.read", {"domain": "repos"}), "parts"
    )
    assert [p["part_key"] for p in read_parts] == ["repo-a", "repo-b"]
    assert [int(p["ordinal"]) for p in read_parts] == [0, 1]
    assert all(_HEX64_RE.match(str(p["content_hash"])) for p in read_parts)

    rpc_export = orca_api.assert_rpc_ok("profileState.export", {})
    cli_export = orca_cli.run("ide", "profile-state", "export").assert_ok()
    assert isinstance(rpc_export, dict) and isinstance(cli_export, dict)
    for blob in (json.dumps(rpc_export), json.dumps(cli_export)):
        assert "orca-core" in blob and "gpui-telegram" in blob


@pytest.mark.tier1
@pytest.mark.f16
def test_f16_t1_atomic_snapshot_writer_coalesced_commit_and_clean_disk(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    """F16 T1: AtomicSnapshotWriter commits monotonic generations with zero leftover *.tmp."""
    snap_path = temp_workspace.resolve("snapshots/profile.json")
    gens = [
        int(
            orca_api.assert_rpc_ok(
                "profileState.snapshotWrite",
                {"path": str(snap_path), "payload": {"version": idx, "marker": f"snap-v{idx}"}},
            )["committed_generation"]
        )
        for idx in (1, 2, 3)
    ]
    assert gens[0] < gens[1] < gens[2], gens
    assert snap_path.is_file(), f"Missing snapshot file: {snap_path}"
    on_disk = json.loads(snap_path.read_text(encoding="utf-8"))
    assert on_disk["version"] == 3 and on_disk["marker"] == "snap-v3"
    assert "snap-v3" in json.dumps(
        orca_api.assert_rpc_ok("profileState.snapshotRead", {"path": str(snap_path)})
    )
    assert list(snap_path.parent.glob("*.tmp")) == []


@pytest.mark.tier1
@pytest.mark.f16
def test_f16_t1_ai_vault_opt_in_incremental_byte_offset_and_prefix_hash_search(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F16 T1: Opt-in AI Vault indexes incrementally by byte_offset + SHA-256 prefix_hash."""
    t_path = _write_transcript(
        temp_workspace,
        "transcripts/claude_sess_1.jsonl",
        [
            {"role": "user", "text": "Audit initial snapshot writer behavior"},
            {"role": "assistant", "text": "Checking atomic rename barrier"},
            {"role": "tool", "text": "grep AtomicSnapshotWriter ok"},
        ],
    )
    opt = orca_api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    assert opt.get("opt_in") is True or opt.get("enabled") is True

    idx_req = {
        "agent": "claude",
        "session_id": "vault-inc-1",
        "transcript_path": str(t_path),
        "workspace_path": str(temp_workspace.path),
    }
    idx1 = orca_api.assert_rpc_ok("aiVault.index", idx_req)
    offset_1, hash_1 = int(idx1["byte_offset"]), str(idx1["prefix_hash"])
    assert offset_1 > 0 and _HEX64_RE.match(hash_1), idx1

    with t_path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps({"role": "assistant", "text": "Found DeltaAppendSentinel77"}) + "\n")

    idx2 = orca_api.assert_rpc_ok("aiVault.index", idx_req)
    assert int(idx2["byte_offset"]) > offset_1 and str(idx2["prefix_hash"]) == hash_1
    assert idx2.get("incremental") is True

    rest_hits = _extract_items(
        orca_api.assert_ok(
            orca_api.get("/api/v1/orca/ide/vault/search", params={"q": "DeltaAppendSentinel77"})
        ),
        "hits",
        "results",
    )
    assert any(h.get("session_id") == "vault-inc-1" for h in rest_hits), rest_hits
    cli_hits = _extract_items(
        orca_cli.run("ide", "vault-search", "--query", "DeltaAppendSentinel77").assert_ok(),
        "hits",
        "results",
    )
    assert any(h.get("session_id") == "vault-inc-1" for h in cli_hits), cli_hits


@pytest.mark.tier1
@pytest.mark.f16
def test_f16_t1_ai_vault_identifier_splitting_typo_repair_and_cursor_pagination(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    """F16 T1: AI Vault splits camelCase/snake_case, repairs typos, and fences cursor pages."""
    orca_api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    t_claude = _write_transcript(
        temp_workspace,
        "transcripts/ws_a_claude.jsonl",
        [
            {"role": "user", "text": "Inspect computeDiffAnchorLine and atomic_snapshot_writer"},
            {"role": "assistant", "text": "Coordinating multi-agent orchestration plan"},
        ],
    )
    t_opencode = _write_transcript(
        temp_workspace,
        "transcripts/ws_b_opencode.jsonl",
        [{"role": "user", "text": "OpencodeOnlyMarker42 orchestration pipeline check"}],
    )
    for agent, sid, path in (
        ("claude", "vault-split-claude", t_claude),
        ("opencode", "vault-split-opencode", t_opencode),
    ):
        orca_api.assert_rpc_ok(
            "aiVault.index",
            {"agent": agent, "session_id": sid, "transcript_path": str(path), "workspace_path": str(path.parent)},
        )

    for query in ("AnchorLine", "snapshot", "orchestraiton"):
        hits = _extract_items(
            orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/vault/search", params={"q": query})),
            "hits",
            "results",
        )
        assert any(h.get("session_id") == "vault-split-claude" for h in hits), (query, hits)

    filtered = orca_api.assert_ok(
        orca_api.get("/api/v1/orca/ide/vault/search", params={"q": "orchestration", "agent": "claude"})
    )
    f_hits = _extract_items(filtered, "hits", "results")
    assert len(f_hits) == 1 and f_hits[0].get("session_id") == "vault-split-claude", f_hits
    assert all(h.get("agent") == "claude" for h in f_hits), f_hits
    paged = orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/vault/search", params={"q": "orchestration", "limit": 1}))
    assert len(_extract_items(paged, "hits", "results")) == 1 and any(paged.get(k) not in (None, "") for k in ("cursor", "next_cursor", "generation", "incarnation")), paged


@pytest.mark.tier1
@pytest.mark.f16
def test_f16_t1_ai_vault_conversation_fork_and_cross_agent_handoff(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    """F16 T1: Forking preserves parent transcript intact and emits cross-agent handoff prompt."""
    orca_api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    t_src = _write_transcript(
        temp_workspace,
        "transcripts/fork_source.jsonl",
        [
            {"role": "user", "text": "Refactor diff reanchoring algorithm"},
            {"role": "assistant", "text": "Drafted line-shift tracking invariants"},
        ],
    )
    orig_bytes = t_src.read_bytes()
    orca_api.assert_rpc_ok(
        "aiVault.index",
        {"agent": "claude", "session_id": "vault-src-fork-1", "transcript_path": str(t_src), "workspace_path": str(temp_workspace.path)},
    )
    fork_res = orca_api.assert_rpc_ok(
        "aiVault.fork", {"session_id": "vault-src-fork-1", "title": "Codex verification fork"}
    )
    assert isinstance(fork_res["forked_session_id"], str) and fork_res["forked_session_id"].strip() and fork_res["forked_session_id"] != "vault-src-fork-1", fork_res
    assert fork_res["parent_session_id"] == "vault-src-fork-1"
    assert t_src.read_bytes() == orig_bytes

    handoff = orca_api.assert_rpc_ok(
        "aiVault.handoffPrompt", {"source_session_id": "vault-src-fork-1", "target_agent": "codex"}
    )
    assert handoff.get("target_agent") == "codex"
    prompt_text = handoff.get("prompt")
    assert isinstance(prompt_text, str) and len(prompt_text.strip()) >= 20 and "reanchoring" in prompt_text.lower(), handoff


@pytest.mark.tier1
@pytest.mark.f17
def test_f17_t1_create_list_and_resolve_line_anchored_diff_annotations(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, git_repo: GitRepoFixture
) -> None:
    """F17 T1: Create, list, and resolve line-anchored diff annotations via REST, CLI, and RPC."""
    lines = [f"pub fn f{i}(x: u32) -> u32 {{ x + {i} }}" for i in range(1, 21)]
    git_repo.commit_file("src/lib.rs", "\n".join(lines) + "\n", msg="baseline lib")
    lines[8] = "pub fn f9(x: u32) -> u32 { x.wrapping_add(9) }"
    lines[9] = "pub fn f10(x: u32) -> u32 { x.wrapping_add(10) }"
    git_repo.write_file("src/lib.rs", "\n".join(lines) + "\n")

    ann1 = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/diff-annotations",
            json_body={"repo_path": str(git_repo.path), "file": "src/lib.rs", "line": 9, "body": "Use `checked_add` here"},
        )
    )
    ann1_id = str(ann1["id"])
    assert ann1["id"] not in (None, "") and ann1.get("resolved") is False and int(ann1["line"]) == 9

    ann2 = orca_cli.run(
        "ide", "diff-annotate", "--repo", str(git_repo.path), "--file", "src/lib.rs", "--line", "10", "--body", "Add unit test"
    ).assert_ok()
    ann2_id = str(ann2["id"])
    assert ann2["id"] not in (None, "") and ann2_id != ann1_id

    by_id = _list_annotations(orca_api, git_repo.path)
    assert ann1_id in by_id and ann2_id in by_id
    assert by_id[ann1_id]["resolved"] is False and by_id[ann2_id]["resolved"] is False

    resolved = orca_api.assert_rpc_ok("diffReview.resolve", {"annotation_id": ann1_id})
    assert resolved.get("resolved") is True
    after_by_id = _list_annotations(orca_api, git_repo.path)
    assert after_by_id[ann1_id]["resolved"] is True and after_by_id[ann2_id]["resolved"] is False


@pytest.mark.tier1
@pytest.mark.f17
def test_f17_t1_diff_annotations_track_line_shifts_across_edits(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T1: Reanchoring shifts comments on lines 5 & 12 to 9 & 16 after 4-line top insert."""
    base_lines = [f"let v{i}: i32 = {i};" for i in range(1, 21)]
    git_repo.commit_file("src/math.rs", "\n".join(base_lines) + "\n", msg="math base")

    a5 = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/diff-annotations",
            json_body={"repo_path": str(git_repo.path), "file": "src/math.rs", "line": 5, "body": "Anchor five note"},
        )
    )
    a12 = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/diff-annotations",
            json_body={"repo_path": str(git_repo.path), "file": "src/math.rs", "line": 12, "body": "Anchor twelve note"},
        )
    )
    git_repo.write_file("src/math.rs", "\n".join([f"// hdr {i}" for i in range(1, 5)] + base_lines) + "\n")
    orca_api.assert_rpc_ok("diffReview.reanchor", {"repo_path": str(git_repo.path), "file": "src/math.rs"})

    by_id = _list_annotations(orca_api, git_repo.path)
    assert a5["id"] not in (None, "") and a12["id"] not in (None, "") and int(by_id[str(a5["id"])]["line"]) == 9 and int(by_id[str(a12["id"])]["line"]) == 16
    assert by_id[str(a5["id"])]["body"] == "Anchor five note"
    assert by_id[str(a12["id"])]["body"] == "Anchor twelve note"


@pytest.mark.tier1
@pytest.mark.f17
def test_f17_t1_batch_send_unresolved_annotations_keeps_comments_pinned(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, git_repo: GitRepoFixture
) -> None:
    """F17 T1: diff-batch emits only unresolved comments and keeps them pinned."""
    git_repo.commit_file("src/a.rs", "fn a1() {}\nfn a2() {}\n", msg="init a")
    git_repo.commit_file("src/b.rs", "fn b1() {}\n", msg="init b")
    anns = [
        orca_api.assert_ok(
            orca_api.post(
                "/api/v1/orca/ide/diff-annotations",
                json_body={"repo_path": str(git_repo.path), "file": f, "line": ln, "body": b},
            )
        )
        for f, ln, b in (
            ("src/a.rs", 1, "RESOLVED_NOTE_EXCLUDED"),
            ("src/a.rs", 2, "UNRESOLVED_NOTE_ALPHA"),
            ("src/b.rs", 1, "UNRESOLVED_NOTE_BETA"),
        )
    ]
    assert all(a.get("id") not in (None, "") for a in anns), anns
    orca_api.assert_rpc_ok("diffReview.resolve", {"annotation_id": str(anns[0]["id"])})

    batch = orca_cli.run("ide", "diff-batch", "--repo", str(git_repo.path)).assert_ok()
    prompt_str = batch.get("prompt") if isinstance(batch, dict) else None
    assert isinstance(prompt_str, str) and "UNRESOLVED_NOTE_ALPHA" in prompt_str and "UNRESOLVED_NOTE_BETA" in prompt_str, batch
    assert "src/a.rs" in prompt_str and "src/b.rs" in prompt_str
    assert "RESOLVED_NOTE_EXCLUDED" not in prompt_str

    by_id = _list_annotations(orca_api, git_repo.path)
    assert by_id[str(anns[1]["id"])]["resolved"] is False
    assert by_id[str(anns[2]["id"])]["resolved"] is False


@pytest.mark.tier1
@pytest.mark.f17
def test_f17_t1_line_level_ai_attribution_records_and_exports_locally(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T1: Line-level AI attribution records and exports locally with zero repo sidecars."""
    git_repo.commit_file("src/service.rs", "\n".join(f"let s{i} = {i};" for i in range(1, 16)) + "\n", msg="svc")
    orca_api.assert_rpc_ok(
        "diffReview.recordAttribution",
        {"repo_path": str(git_repo.path), "file": "src/service.rs", "author_kind": "ai", "agent": "claude", "start_line": 4, "end_line": 9},
    )
    attr = orca_api.assert_rpc_ok("diffReview.getAttribution", {"repo_path": str(git_repo.path), "file": "src/service.rs"})
    by_line = {int(e["line"]): e["author_kind"] for e in _extract_items(attr, "lines", "entries")}
    assert all(by_line.get(ln) == "ai" for ln in range(4, 10)) and "claude" in json.dumps(attr), attr
    exported = orca_api.assert_rpc_ok("diffReview.exportAttribution", {"repo_path": str(git_repo.path)})
    assert isinstance(exported, dict) and "src/service.rs" in json.dumps(exported)
    porcelain = [ln for ln in git_repo.git("status", "--porcelain").stdout.splitlines() if ln.strip()]
    assert all(ln.endswith("src/service.rs") for ln in porcelain), porcelain


@pytest.mark.tier1
@pytest.mark.f17
def test_f17_t1_source_control_stage_ai_commit_pr_draft_and_commit(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T1: Source control RPC stages changes, drafts commit/PR, and commits to Git."""
    git_repo.commit_file("src/feature.rs", "pub fn val() -> i32 { 1 }\n", msg="base feature")
    git_repo.create_branch("feat/sc-e2e", checkout=True)
    git_repo.write_file("src/feature.rs", "pub fn val() -> i32 { 42 }\n")

    assert "src/feature.rs" in json.dumps(orca_api.assert_rpc_ok("git.status", {"repo_path": str(git_repo.path)}))
    orca_api.assert_rpc_ok("git.stage", {"repo_path": str(git_repo.path), "paths": ["src/feature.rs"]})
    draft = orca_api.assert_rpc_ok("git.draftCommit", {"repo_path": str(git_repo.path)})
    msg = str(draft.get("message") or draft.get("subject") or "").strip()
    assert len(msg) >= 5, draft
    pr = orca_api.assert_rpc_ok("git.draftPr", {"repo_path": str(git_repo.path), "base_branch": "main"})
    assert isinstance(pr.get("title"), str) and pr["title"].strip() and isinstance(pr.get("body"), str) and pr["body"].strip(), pr
    orca_api.assert_rpc_ok("git.commit", {"repo_path": str(git_repo.path), "message": msg})
    assert git_repo.git("log", "-1", "--pretty=%s").stdout.strip() == msg.splitlines()[0].strip()


@pytest.mark.tier1
@pytest.mark.f17
def test_f17_t1_tasks_hub_multi_forge_aggregation_and_issue_command(
    orca_api: OrcaApiClient, git_repo: GitRepoFixture
) -> None:
    """F17 T1: Tasks Hub aggregates 4 forges and expands orca.yaml issueCommand template."""
    git_repo.write_orca_yaml('issueCommand: "Fix {{artifact_url}}"\n')
    forges = (
        ("github", "GH-101", "Fix vault FTS rebuild", "https://github.com/acme/orca/issues/101"),
        ("gitlab", "GL-202", "Audit diff anchor", "https://gitlab.com/acme/orca/-/issues/202"),
        ("linear", "ENG-303", "Polish jump palette", "https://linear.app/acme/issue/ENG-303"),
        ("jira", "PLAT-404", "Plugin manifest cap", "https://acme.atlassian.net/browse/PLAT-404"),
    )
    for provider, key, title, url in forges:
        orca_api.assert_rpc_ok("tasksHub.ingest", {"provider": provider, "key": key, "title": title, "url": url})
    tasks = _extract_items(orca_api.assert_rpc_ok("tasksHub.list", {}), "tasks")
    assert {k for _, k, _, _ in forges}.issubset({str(t.get("key")) for t in tasks}), tasks
    plan = orca_api.assert_rpc_ok("tasksHub.prepareWorkspace", {"repo_path": str(git_repo.path), "task_key": "GH-101"})
    assert "Fix https://github.com/acme/orca/issues/101" in json.dumps(plan)
