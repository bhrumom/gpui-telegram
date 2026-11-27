"""Tier 2 boundary, negative & edge-case E2E tests for F05 (Look Around) and F07 (Ecosystem & Share)."""

from __future__ import annotations

from collections.abc import Callable
import hashlib
from pathlib import Path
from typing import Any

import pytest

from harness import (
    LOOK_AROUND_AGENTS, LOOK_AROUND_MAX_SUGGESTIONS,
    BotHomeFixture, CliResult, GitRepoFixture, OrcaApiClient, OrcaCliRunner, TempWorkspace,
)


def _unwrap(payload: Any, *keys: str) -> Any:
    if isinstance(payload, dict):
        for k in keys:
            if isinstance(payload.get(k), dict):
                return payload[k]
    return payload


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


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _up_next_list(payload: Any) -> list[dict[str, Any]]:
    items = payload["suggestions"] if isinstance(payload, dict) and "suggestions" in payload else payload
    assert isinstance(items, list)
    return items


@pytest.mark.tier2
@pytest.mark.f05
@pytest.mark.edge_case("SM2-11")
def test_t2_f05_sm2_11_link_carries_credential_strips_or_rejects_secret_urls(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    bot_id = _create_bot(orca_api, "Cred URL Guard Bot", agent="claude")["id"]
    bot_cli_id = _create_bot(orca_api, "Cred URL CLI Bot", agent="claude")["id"]
    secret_urls = [
        "https://app.example.com/issues/12?api_key=orca_secret_val_1",
        "https://ci.example.com/build/99?token=ghp_1234567890abcdef1234567890abcdef1234",
        "https://alice:TopSecret99@internal.example.com/pr/11",
        "https://app.example.com/dash#access_key=AKIAIOSFODNN7EXAMPLE",
        "https://app.example.com/view?sig=deadbeef99&auth=bearer_tok&session=sess_xyz",
    ]
    for idx, url in enumerate(secret_urls):
        resp = orca_api.post(
            f"/api/v1/orca/bots/{bot_id}/up-next",
            json_body={"title": f"Secret URL {idx}", "why": "Check link", "ask": "Inspect", "link": url},
        )
        assert resp.status in (200, 201, 400, 422), f"unexpected status {resp.status}: {resp.text!r}"
        if resp.status in (200, 201):
            sug = _unwrap(orca_api.assert_ok(resp), "suggestion")
            assert isinstance(sug, dict) and sug.get("title") == f"Secret URL {idx}" and not sug.get("link")
        else:
            assert resp.extract_error_code() in ("invalid_argument", "secret_blocked")

    cli_res = orca_cli.run(
        "bot", "suggest", "--bot", bot_cli_id, "--title", "CLI secret", "--why", "Test", "--ask", "Check",
        "--link", "https://ci.example.com/job/5?token=ghp_cli_secret_99887766",
    )
    if cli_res.returncode == 0:
        cli_sug = _unwrap(_cli_json_ok(cli_res), "suggestion")
        assert isinstance(cli_sug, dict) and cli_sug.get("title") == "CLI secret" and not cli_sug.get("link")
        assert "ghp_cli_secret_99887766" not in cli_res.stdout
    else:
        assert cli_res.extract_error_code() in ("invalid_argument", "secret_blocked")

    clean_url = "https://github.com/acme/repo/pull/55"
    orca_api.assert_ok(
        orca_api.post(
            f"/api/v1/orca/bots/{bot_cli_id}/up-next",
            json_body={"title": "Clean PR 55", "why": "Ready", "ask": "Review PR 55", "link": clean_url},
        )
    )
    rest_resp = orca_api.get(f"/api/v1/orca/bots/{bot_id}/up-next")
    cli_resp = orca_api.get(f"/api/v1/orca/bots/{bot_cli_id}/up-next")
    _up_next_list(orca_api.assert_ok(rest_resp))
    cli_items = _up_next_list(orca_api.assert_ok(cli_resp))
    combined = rest_resp.text + "\n" + cli_resp.text
    for leaked in (
        "orca_secret_val_1", "ghp_1234567890", "TopSecret99", "AKIAIOSFODNN7EXAMPLE",
        "deadbeef99", "bearer_tok", "sess_xyz", "api_key=", "token=", "alice:", "ghp_cli_secret_99887766",
    ):
        assert leaked not in combined
    assert any(it.get("title") == "Clean PR 55" and it.get("link") == clean_url for it in cli_items)


@pytest.mark.tier2
@pytest.mark.f05
def test_t2_f05_unsupported_non_readonly_agents_refused(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    for agent in ("grok", "cursor", "goose", "devin"):
        assert agent not in LOOK_AROUND_AGENTS
        bot_id = _create_bot(orca_api, f"NoLA {agent}", agent=agent)["id"]
        orca_api.post(f"/api/v1/orca/bots/{bot_id}/look-around", json_body={"every": "mornings"}).assert_error_code(
            "invalid_argument", 400, 422
        )
        orca_api.assert_rpc_error("bots.lookAround.configure", "invalid_argument", {"botId": bot_id, "every": "mornings"})
        orca_cli.run("bot", "look-around", "--bot", bot_id, "--every", "mornings").assert_error("invalid_argument")


@pytest.mark.tier2
@pytest.mark.f05
def test_t2_f05_suggestion_max_five_cap_and_field_length_bounds(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "UpNext Cap Bot", agent="claude")["id"]
    bad_bodies = [
        {"title": "T" * 81, "why": "ok", "ask": "ok"},
        {"title": "ok", "why": "W" * 401, "ask": "ok"},
        {"title": "ok", "why": "ok", "ask": "A" * 2001},
        {"title": "", "why": "ok", "ask": "ok"},
    ]
    for body in bad_bodies:
        orca_api.post(f"/api/v1/orca/bots/{bot_id}/up-next", json_body=body).assert_error_code("invalid_argument", 400, 422)

    orca_api.assert_ok(
        orca_api.post(f"/api/v1/orca/bots/{bot_id}/up-next", json_body={"title": "B" * 80, "why": "W" * 400, "ask": "A" * 2000})
    )
    for idx in range(7):
        orca_api.assert_ok(
            orca_api.post(f"/api/v1/orca/bots/{bot_id}/up-next", json_body={"title": f"Distinct Item {idx}", "why": f"Reason {idx}", "ask": f"Do {idx}"})
        )
    stored = _up_next_list(orca_api.assert_ok(orca_api.get(f"/api/v1/orca/bots/{bot_id}/up-next")))
    assert 0 < len(stored) <= LOOK_AROUND_MAX_SUGGESTIONS


@pytest.mark.tier2
@pytest.mark.f05
def test_t2_f05_focus_length_cap_and_invalid_schedule_preset_rejected(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "Focus Cap Bot", agent="claude")["id"]
    orca_api.post(f"/api/v1/orca/bots/{bot_id}/look-around", json_body={"every": "every-4h", "focus": "F" * 501}).assert_error_code(
        "invalid_argument", 400, 422
    )
    for bad_every in ("every-5m", "hourly", ""):
        orca_api.post(f"/api/v1/orca/bots/{bot_id}/look-around", json_body={"every": bad_every}).assert_error_code(
            "invalid_argument", 400, 422
        )
    exact_focus = "F" * 500
    saved = orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/look-around", json_body={"every": "every-4h", "focus": exact_focus}))
    assert saved["focus"] == exact_focus and saved["every"] == "every-4h"


@pytest.mark.tier2
@pytest.mark.f05
def test_t2_f05_read_only_confinement_leaves_bot_home_and_repo_unmodified(
    orca_api: OrcaApiClient,
    bot_home_factory: Callable[[str | None], BotHomeFixture],
    git_repo: GitRepoFixture,
) -> None:
    head_before = git_repo.commit_file("README.md", "# Read-only confinement\n", msg="seed")
    home = bot_home_factory("look-around-ro")
    bot_id = _create_bot(
        orca_api, "ReadOnly Confinement Bot", agent="claude",
        folder="look-around-ro", homePath=str(home.root), workspacePath=str(git_repo.path),
    )["id"]
    home.write_soul("# ReadOnly Confinement Bot\nInspect repo in read-only mode.\n")
    home.write_memory("# Memory\n\n- 2026-11-25 Rule: Never mutate files.\n")
    soul_hash_before, mem_hash_before = _sha256_file(home.root / "SOUL.md"), _sha256_file(home.root / "MEMORY.md")
    status_before = git_repo.git("status", "--porcelain").stdout.strip()
    assert status_before == ""

    run_res = _unwrap(
        orca_api.assert_ok(
            orca_api.post(
                f"/api/v1/orca/bots/{bot_id}/look-around",
                json_body={"every": "mornings", "runNow": True, "mode": "read-only"},
            )
        ),
        "lookAround", "run",
    )
    assert isinstance(run_res, dict) and run_res.get("every") == "mornings"
    assert run_res.get("mode") == "read-only" or run_res.get("readOnly") is True
    assert run_res.get("ran") is True or bool(run_res.get("runId"))
    assert _sha256_file(home.root / "SOUL.md") == soul_hash_before
    assert _sha256_file(home.root / "MEMORY.md") == mem_hash_before
    assert git_repo.git("rev-parse", "HEAD").stdout.strip() == head_before
    assert git_repo.git("status", "--porcelain").stdout.strip() == status_before


@pytest.mark.tier2
@pytest.mark.f05
def test_t2_f05_paused_or_nonexistent_bot_look_around_and_up_next_errors(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "Paused LookAround Bot", agent="claude")["id"]
    orca_api.get("/api/v1/orca/bots/nonexistent-bot-id-999/up-next").assert_error_code("bot_not_found", 404)
    orca_api.post("/api/v1/orca/bots/nonexistent-bot-id-999/look-around", json_body={"every": "mornings"}).assert_error_code(
        "bot_not_found", 404
    )
    orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/bots/{bot_id}", json_body={"paused": True}))
    orca_api.post(f"/api/v1/orca/bots/{bot_id}/look-around", json_body={"runNow": True}).assert_error_code("bot_paused", 409, 400)


@pytest.mark.tier2
@pytest.mark.f07
@pytest.mark.edge_case("SM2-19")
def test_t2_f07_sm2_19_e2ee_team_share_open_rejects_cross_host_and_field_mismatch_replay(
    orca_api: OrcaApiClient,
) -> None:
    bot_id = _create_bot(orca_api, "Replay Guard Bot", agent="claude")["id"]
    sentinel = "REPLAY-GUARD-SENTINEL"
    base_aad = {"hostKey": "host-A", "shareId": "share-1", "botId": bot_id, "clientMessageId": "cmsg-1"}
    sealed = orca_api.assert_ok(
        orca_api.post(f"/api/v1/orca/bots/{bot_id}/share/seal", json_body={**base_aad, "plaintext": sentinel})
    )
    envelope = sealed["envelope"]
    receiver_key = sealed.get("receiverKey")

    baseline = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/bots/share/open",
            json_body={**base_aad, "envelope": envelope, "receiverKey": receiver_key},
        )
    )
    assert baseline["plaintext"] == sentinel

    tampered_env = dict(envelope) if isinstance(envelope, dict) else f"X{envelope}"
    if isinstance(tampered_env, dict):
        for k in ("ciphertext", "signature", "mac", "payload"):
            if isinstance(tampered_env.get(k), str) and tampered_env[k]:
                val = tampered_env[k]
                tampered_env[k] = ("A" if val[0] != "A" else "B") + val[1:]
                break
    assert tampered_env != envelope

    attacks: list[dict[str, Any]] = [
        {**base_aad, "hostKey": "host-B", "envelope": envelope, "receiverKey": receiver_key},
        {**base_aad, "shareId": "share-other", "envelope": envelope, "receiverKey": receiver_key},
        {**base_aad, "botId": "bot-other", "envelope": envelope, "receiverKey": receiver_key},
        {**base_aad, "clientMessageId": "cmsg-other", "envelope": envelope, "receiverKey": receiver_key},
        {**base_aad, "envelope": tampered_env, "receiverKey": receiver_key},
    ]
    for attack_body in attacks:
        resp = orca_api.post("/api/v1/orca/bots/share/open", json_body=attack_body)
        assert resp.status in (200, 400, 403, 422), f"unexpected status {resp.status}: {resp.text!r}"
        assert sentinel not in resp.text
        if resp.status == 200:
            payload = resp.json()
            assert payload.get("opened") is None and payload.get("verified") is False
        else:
            assert resp.extract_error_code() in ("not_this_host", "invalid_argument")


@pytest.mark.tier2
@pytest.mark.f07
@pytest.mark.edge_case("SM2-20")
def test_t2_f07_sm2_20_hand_over_notify_failure_rollback_restored_moved_since_and_left(
    orca_api: OrcaApiClient,
    git_repo: GitRepoFixture,
) -> None:
    git_repo.commit_file("README.md", "# Rollback repo\n", msg="seed")
    bot_id = _create_bot(orca_api, "Rollback Handoff Bot", agent="claude")["id"]
    base = {"workspacePath": str(git_repo.path), "originalParent": "ws-parent-orig", "simulateNotifyFailure": True}

    res_a = orca_api.post(f"/api/v1/orca/bots/{bot_id}/hand-over", json_body=base)
    assert res_a.status in (200, 409, 502), f"unexpected status {res_a.status}: {res_a.text!r}"
    data_a = res_a.json()
    assert data_a["placement"] == "restored" and data_a["currentParent"] == "ws-parent-orig"

    res_b = orca_api.post(f"/api/v1/orca/bots/{bot_id}/hand-over", json_body={**base, "concurrentParent": "ws-parent-user-new"})
    assert res_b.status in (200, 409, 502), f"unexpected status {res_b.status}: {res_b.text!r}"
    data_b = res_b.json()
    assert data_b["placement"] == "moved-since" and data_b["currentParent"] == "ws-parent-user-new"
    assert data_b.get("reason") == "lineage_parent_changed" or res_b.extract_error_code() == "lineage_parent_changed"

    res_c = orca_api.post(f"/api/v1/orca/bots/{bot_id}/hand-over", json_body={**base, "simulateRestoreFailure": True})
    assert res_c.status in (200, 409, 502), f"unexpected status {res_c.status}: {res_c.text!r}"
    assert res_c.json()["placement"] == "left"


@pytest.mark.tier2
@pytest.mark.f07
def test_t2_f07_package_export_blocks_secrets_all_patterns(
    orca_api: OrcaApiClient,
    orca_cli: OrcaCliRunner,
) -> None:
    secret_cases = [
        ("SOUL.md", "key: sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ("memory/notes.md", "pat: ghp_1234567890abcdef1234567890abcdef1234"),
        ("memory/notes.md", "slack: xoxb-1234567890-abcdefghijkl"),
        ("skills/check/SKILL.md", "aws: AKIAIOSFODNN7EXAMPLE"),
        ("SOUL.md", "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEF\n-----END PRIVATE KEY-----"),
        ("memory/notes.md", "jwt: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0In0.sig1234567890"),
    ]
    for idx, (rel_path, secret_text) in enumerate(secret_cases):
        bot_id = _create_bot(orca_api, f"Secret Export {idx}", agent="claude")["id"]
        if rel_path == "SOUL.md":
            orca_api.assert_rpc_ok("bots.home.writeSoul", {"botId": bot_id, "content": f"# Secret {idx}\n{secret_text}\n"})
        else:
            orca_api.assert_rpc_ok("bots.home.writeFile", {"botId": bot_id, "path": rel_path, "content": secret_text + "\n"})
        resp = orca_api.post(f"/api/v1/orca/bots/{bot_id}/package/export", json_body={})
        resp.assert_error_code("secret_blocked", 400, 422)
        assert not resp.json().get("package")
        if idx == 0:
            orca_cli.run("bot", "package-export", "--bot", bot_id).assert_error("secret_blocked")

    bidi_bot = _create_bot(orca_api, "Hidden Unicode Bot", agent="claude")["id"]
    orca_api.assert_rpc_ok(
        "bots.home.writeFile",
        {"botId": bidi_bot, "path": "skills/bidi/SKILL.md", "content": "# Bidi\nSafe \u202e text\n"},
    )
    exported = orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bidi_bot}/package/export", json_body={}))
    findings = {w if isinstance(w, str) else w.get("kind", w.get("code")) for w in exported.get("warnings", [])}
    assert "hidden-unicode" in findings


@pytest.mark.tier2
@pytest.mark.f07
def test_t2_f07_package_import_rejects_newer_schema_duplicate_part_and_bad_path(orca_api: OrcaApiClient) -> None:
    before_list = orca_api.assert_ok(orca_api.get("/api/v1/orca/bots"))
    before_bots = before_list["bots"] if isinstance(before_list, dict) and "bots" in before_list else before_list
    before_count = len(before_bots)

    orca_api.post(
        "/api/v1/orca/bots/package/import",
        json_body={"package": {"schemaVersion": 99, "manifest": {"name": "Future"}, "parts": []}},
    ).assert_error_code("bot_package_newer_schema", 400, 422)

    orca_api.post(
        "/api/v1/orca/bots/package/import",
        json_body={
            "package": {
                "schemaVersion": 1,
                "manifest": {"name": "Dup Part"},
                "parts": [
                    {"path": "SOUL.md", "content": "# A\n"},
                    {"path": "SOUL.md", "content": "# B\n"},
                ],
            }
        },
    ).assert_error_code("bot_package_duplicate_part", 400, 422)

    for bad_path in ("../../etc/passwd", "../escape.md", ".git/config"):
        orca_api.post(
            "/api/v1/orca/bots/package/import",
            json_body={
                "package": {
                    "schemaVersion": 1,
                    "manifest": {"name": "Bad Path"},
                    "parts": [{"path": bad_path, "content": "evil\n"}],
                }
            },
        ).assert_error_code("bot_package_path_not_allowed", 400, 422)

    after_list = orca_api.assert_ok(orca_api.get("/api/v1/orca/bots"))
    after_bots = after_list["bots"] if isinstance(after_list, dict) and "bots" in after_list else after_list
    assert len(after_bots) == before_count


@pytest.mark.tier2
@pytest.mark.f07
def test_t2_f07_offer_card_double_answer_rejected_with_bot_offer_already_answered(orca_api: OrcaApiClient) -> None:
    bot_id = _create_bot(orca_api, "Offer CAS Bot", agent="claude")["id"]
    offer_id = "offer-e2e-1"
    orca_api.assert_rpc_ok(
        "bots.createOffer",
        {"botId": bot_id, "offerId": offer_id, "kind": "connectorOffer", "summary": "Connect GitHub"},
    )
    first = orca_api.assert_rpc_ok(
        "bots.update",
        {"botId": bot_id, "offerAnswer": {"offerId": offer_id, "decision": "accept"}},
    )
    assert first is not None
    orca_api.assert_rpc_error(
        "bots.update",
        "bot_offer_already_answered",
        {"botId": bot_id, "offerAnswer": {"offerId": offer_id, "decision": "decline"}},
    )


@pytest.mark.tier2
@pytest.mark.f07
def test_t2_f07_import_discover_caps_24_per_app_200kb_export_and_unknown_template_rejected(
    orca_api: OrcaApiClient,
    temp_workspace: TempWorkspace,
) -> None:
    ws_path = temp_workspace.path
    for idx in range(1, 31):
        BotHomeFixture.create_foreign_openbot(ws_path, agent_name=f"agent-{idx:02d}")

    disc = orca_api.assert_ok(orca_api.post("/api/v1/orca/bots/import/discover", json_body={"root": str(ws_path)}))
    candidates = disc["candidates"] if isinstance(disc, dict) and "candidates" in disc else disc
    openbot_hits = [c for c in candidates if c.get("source") == "openbot"]
    assert 0 < len(openbot_hits) <= 24

    big_reply = "A" * (220 * 1024)
    big_resp = orca_api.post(
        "/api/v1/orca/bots/import/discover",
        json_body={"root": str(ws_path), "genericExportText": big_reply},
    )
    assert big_resp.status in (200, 201, 400, 422), f"unexpected status {big_resp.status}: {big_resp.text!r}"
    if big_resp.status in (200, 201):
        body_data = orca_api.assert_ok(big_resp)
        stored = body_data.get("genericExportText") if isinstance(body_data, dict) else None
        assert isinstance(stored, str) and 0 < len(stored.encode("utf-8")) <= 204_800
    else:
        assert big_resp.extract_error_code() == "invalid_argument"

    unk = orca_api.post("/api/v1/orca/bots/from-template", json_body={"templateId": "nonexistent-template-xyz"})
    assert unk.status in (400, 404, 422)
    assert unk.extract_error_code() in ("not_found", "invalid_argument")
