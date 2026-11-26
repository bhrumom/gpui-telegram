"""Tier 2 Opaque-Box E2E tests for F06 (Policy, SQLite Triggers, Caps) & F08 (Relay Security, OAuth Retry, MCP Pool Debounce)."""

from __future__ import annotations

import http.client
import json
import re
import stat
import sys
import threading
import time
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

import pytest

from harness import (
    BOT_GROUP_MAX_REMOTE_MEMBERS, MockMcpUpstreamServer, MockOAuthServer, OrcaApiClient,
    OrcaServerHandle, RawHttpClient, RecordedRequest, SqliteFixtureHelper, TempWorkspace,
)

FOLDER_SLUG_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")

def _require_dict_id(payload: Any, *, key: str | None = None, label: str = "object") -> dict[str, Any]:
    obj = payload.get(key) if key and isinstance(payload, dict) and isinstance(payload.get(key), dict) else payload
    assert isinstance(obj, dict) and obj.get("id"), f"missing {label} id in {payload!r}"
    return obj

def _create_bot(api: OrcaApiClient, name: str, *, folder: str | None = None, home_path: Path | None = None) -> dict[str, Any]:
    body: dict[str, Any] = {"name": name, "agent": "claude"}
    if folder is not None:
        assert FOLDER_SLUG_RE.match(folder), f"invalid bot folder slug: {folder!r}"
        body["folder"] = folder
    if home_path is not None:
        body["homePath"] = str(home_path)
    return _require_dict_id(api.post("/api/v1/orca/bots", json_body=body).assert_status(200, 201).json(), key="bot", label="bot")

def _create_group(api: OrcaApiClient, name: str, member_ids: list[str], reply_mode: dict[str, Any] | None = None, **extra: Any) -> dict[str, Any]:
    body: dict[str, Any] = {"name": name, "memberBotIds": member_ids, **extra}
    if reply_mode is not None:
        body["replyMode"] = reply_mode
    return _require_dict_id(api.post("/api/v1/orca/groups", json_body=body).assert_status(200, 201).json(), key="group", label="group")

def _resolve_bot_home(bot: dict[str, Any], fallback: Path) -> Path:
    raw = bot.get("homePath") or bot.get("homeDir") or bot.get("folderPath")
    return Path(str(raw)) if isinstance(raw, str) and raw else fallback

def _extract_relay_grant(api: OrcaApiClient, bot_id: str) -> tuple[int, str, str]:
    info = api.assert_rpc_ok("bots.tools.relayInfo", {"botId": bot_id})
    grant = api.assert_rpc_ok("bots.tools.grant", {"botId": bot_id})
    port = (info.get("port") if isinstance(info, dict) else None) or (grant.get("port") if isinstance(grant, dict) else None)
    token = (grant.get("token") or grant.get("bearer")) if isinstance(grant, dict) else None
    base_path = (grant.get("path") or grant.get("basePath") or info.get("basePath") or "") if isinstance(grant, dict) and isinstance(info, dict) else ""
    assert isinstance(port, int) and port > 0 and isinstance(token, str) and len(token) >= 8, f"bad relay grant: {info!r} {grant!r}"
    return port, token, str(base_path)

def _assert_no_secrets_on_disk(root: Path, secrets: tuple[str, ...]) -> None:
    assert root.is_dir(), f"bot home directory missing: {root}"
    assert secrets and all(isinstance(s, str) and s for s in secrets), f"empty secrets list: {secrets!r}"
    checked_files = 0
    for path in root.rglob("*"):
        if path.is_file():
            checked_files += 1
            text = path.read_text(encoding="utf-8", errors="replace")
            for secret in secrets:
                assert secret not in text, f"secret leaked into {path}"
    assert checked_files > 0, f"no bot home files scanned under {root}; check would be vacuous"

def _wire_oauth_tool(api: OrcaApiClient, bot_id: str, tool_name: str, upstream_url: str, oauth: MockOAuthServer) -> dict[str, Any]:
    tool = api.assert_rpc_ok("bots.tools.add", {"botId": bot_id, "name": tool_name, "transport": "http", "url": upstream_url, "oauth": {"issuer": oauth.issuer, "allowLoopbackTest": True}})
    assert isinstance(tool, dict), f"expected tool dict, got {tool!r}"
    start = api.assert_rpc_ok("bots.tools.oauthStart", {"botId": bot_id, "toolName": tool_name})
    assert isinstance(start, dict), f"expected oauthStart dict, got {start!r}"
    assert oauth.registered_clients, "RFC 7591 DCR POST /register not invoked"
    auth_url = str(start.get("authorizationUrl") or start.get("authUrl") or "")
    parsed = urlparse(auth_url)
    assert parsed.scheme == "http" and parsed.hostname in {oauth.host, "127.0.0.1"} and parsed.port == oauth.port and parsed.path == "/authorize", f"bad authorizationUrl: {auth_url!r}"
    qs = {k: v[0] for k, v in parse_qs(parsed.query).items() if v}
    assert qs.get("code_challenge_method") == "S256" and qs.get("code_challenge"), f"missing PKCE S256: {auth_url!r}"
    assert qs.get("client_id") in oauth.registered_clients and qs.get("redirect_uri"), f"bad DCR/redirect_uri: {auth_url!r}"
    res_uri = qs.get("resource", "")
    assert res_uri and (res_uri.rstrip("/") == upstream_url.rstrip("/") or upstream_url.startswith(res_uri.rstrip("/") + "/")), f"RFC 8707 resource mismatch: {auth_url!r}"
    auth_resp = RawHttpClient(oauth.host, oauth.port).request("GET", f"{parsed.path}?{parsed.query}").assert_status(200).json()
    assert isinstance(auth_resp, dict) and isinstance(auth_resp.get("code"), str) and auth_resp["code"], f"missing code from /authorize: {auth_resp!r}"
    before_tokens = len(oauth.token_calls)
    api.assert_rpc_ok("bots.tools.oauthCallback", {"botId": bot_id, "toolName": tool_name, "code": auth_resp["code"], "state": str(auth_resp.get("state") or qs.get("state") or start.get("state") or ""), "iss": str(auth_resp.get("iss") or oauth.issuer)})
    new_calls = oauth.token_calls[before_tokens:]
    assert oauth.active_access_tokens and any(c.get("grant_type") == "authorization_code" and c.get("code_verifier") for c in new_calls), f"missing PKCE token exchange: {new_calls!r}"
    return tool

def _legacy_strip_json_rows(db: SqliteFixtureHelper, table: str, strip_keys: set[str], fk_cols: tuple[str, ...], fk_val: str) -> None:
    cols = [r[1] for r in db.exec_sql(f"PRAGMA table_info({table})")]
    jcol = next((c for c in cols if c in {"data", "json", "payload", "state_json", "metadata_json"}), None)
    assert jcol is not None, f"expected JSON column on {table}, found {cols!r}"
    pk = next((c for c in cols if c in {"id", "message_id", "group_id"}), "rowid")
    fcol = next((c for c in fk_cols if c in cols), None)
    rows = db.exec_sql(f"SELECT {pk}, {jcol} FROM {table} WHERE {fcol} = ?", (fk_val,)) if fcol else db.exec_sql(f"SELECT {pk}, {jcol} FROM {table}")
    assert rows, f"no rows in {table} for {fk_val!r}"
    found_newer = False
    for row_pk, raw in rows:
        assert isinstance(raw, str) and raw.strip().startswith("{"), f"expected JSON object in {table}.{jcol}: {raw!r}"
        obj = json.loads(raw)
        found_newer = found_newer or any(k in obj for k in strip_keys)
        stripped = {k: v for k, v in obj.items() if k not in strip_keys}
        db.exec_sql(f"UPDATE {table} SET {jcol} = ? WHERE {pk} = ?", (json.dumps(stripped), row_pk))
    assert found_newer, f"expected newer keys {sorted(strip_keys)} in {table}.{jcol} before legacy UPDATE"


# ---------------------------------------------------------------------------
# F06 — Multi-Bot Group Policy Refusals, SQLite Triggers & Boundaries (Tier 2)
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f06
@pytest.mark.edge_case("SM2-21")
def test_f06_t2_worker_handoff_rejected_bot_request_worker(orca_api: OrcaApiClient) -> None:
    coord, target = _create_bot(orca_api, "WorkerCoord"), _create_bot(orca_api, "WorkerTarget")
    group_id = _create_group(orca_api, "Worker Policy Room", [coord["id"], target["id"]], {"kind": "all"})["id"]

    # Positive baseline: worker may send question/status without hitting bot_request_worker.
    assert orca_api.assert_rpc_ok("bots.groups.request", {"groupId": group_id, "fromBotId": coord["id"], "toBotId": target["id"], "type": "question", "text": "Need API schema"}, caller_scope="worker") is not None
    assert orca_api.assert_rpc_ok("bots.groups.request", {"groupId": group_id, "fromBotId": coord["id"], "toBotId": target["id"], "type": "status", "text": "Step 1 done"}, caller_scope="worker") is not None

    err = orca_api.assert_rpc_error("bots.groups.request", "bot_request_worker", {"groupId": group_id, "fromBotId": coord["id"], "toBotId": target["id"], "type": "handoff", "text": "Take over job"}, caller_scope="worker")
    msg = str(err.get("message", "")).lower()
    assert "worker" in msg and ("hand" in msg or "parent" in msg), f"unexpected bot_request_worker message: {err!r}"


@pytest.mark.tier2
@pytest.mark.f06
@pytest.mark.edge_case("SM2-21")
def test_f06_t2_request_policy_remaining_six_refusal_codes(orca_api: OrcaApiClient) -> None:
    alice, bob = _create_bot(orca_api, "PolicyAlice"), _create_bot(orca_api, "PolicyBob")
    group = _create_group(orca_api, "Policy Six Room", [alice["id"], bob["id"]], {"kind": "all"})
    group_id = group["id"]
    sess_map = group.get("sessionIds")
    assert sess_map is None or isinstance(sess_map, dict), f"bad sessionIds: {group!r}"
    alice_sess = sess_map.get(alice["id"], "alice-current-chat") if isinstance(sess_map, dict) else "alice-current-chat"

    # Positive baseline before negative assertions.
    orca_api.assert_rpc_ok("bots.groups.request", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "type": "question", "text": "Baseline ok"})

    orca_api.patch(f"/api/v1/orca/bots/{bob['id']}", json_body={"paused": True}).assert_status(200)
    orca_api.assert_rpc_error("bots.groups.request", "bot_paused", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "type": "question", "text": "Wake?"})
    orca_api.patch(f"/api/v1/orca/bots/{bob['id']}", json_body={"paused": False}).assert_status(200)

    self_err = orca_api.assert_rpc_error("bots.groups.request", "bot_self", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": alice["id"], "chatSessionId": alice_sess, "type": "question", "text": "Self ping"})
    assert "--chat" in str(self_err.get("message", "")).lower() or "new" in str(self_err.get("message", "")).lower()

    orca_api.assert_rpc_error("bots.groups.request", "bot_request_type", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "type": "delegate_all", "text": "Invalid kind"})
    orca_api.assert_rpc_error("bots.groups.request", "bot_not_found", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": "bot-does-not-exist-404", "type": "question", "text": "Ghost"})
    orca_api.assert_rpc_error("bots.groups.request", "bot_chat_not_found", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "chatSessionId": "nonexistent-chat-999", "type": "question", "text": "Missing chat"})
    orca_api.assert_rpc_error("bots.groups.request", "bots_disabled", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "type": "question", "text": "Disabled test", "botsEnabled": False})


@pytest.mark.tier2
@pytest.mark.f06
@pytest.mark.edge_case("SM2-04")
def test_f06_t2_sqlite_after_update_preserves_newer_group_json_fields(fresh_orca_server: OrcaServerHandle) -> None:
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)
    local_bot = _create_bot(api, "SqliteTriggerBot")
    group_id = _create_group(
        api, "Rollback Trigger Room", [local_bot["id"]], {"kind": "mentions"},
        remoteMembers=[{"hostId": "host-1", "botId": "rem-1", "name": "RemOne", "chatSessionId": "rem-sess-preserve-1"}],
        images=[{"id": "img-1", "path": "diagram-preserve.png", "mime": "image/png"}],
    )["id"]
    api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "@RemOne inspect diagram", "images": [{"id": "img-1", "path": "diagram-preserve.png", "mime": "image/png"}]}).assert_status(200, 201, 202)

    db = SqliteFixtureHelper(fresh_orca_server.db_path)
    triggers = db.exec_sql("SELECT name, tbl_name, sql FROM sqlite_master WHERE type='trigger' AND tbl_name IN ('bot_groups', 'bot_group_messages')")
    after_tables = {str(r[1]) for r in triggers if "AFTER UPDATE" in str(r[2]).upper()}
    assert {"bot_groups", "bot_group_messages"}.issubset(after_tables), f"missing AFTER UPDATE trigger on both tables: {triggers!r}"

    _legacy_strip_json_rows(db, "bot_groups", {"remoteMembers", "remote_members", "images"}, ("id", "group_id"), group_id)
    _legacy_strip_json_rows(db, "bot_group_messages", {"images", "remoteAsks", "remote_asks"}, ("group_id", "groupId"), group_id)

    fresh_orca_server.restart()
    api_after = fresh_orca_server.api_client(sess)
    groups_after = api_after.get("/api/v1/orca/groups").assert_status(200).json()
    g_list = groups_after if isinstance(groups_after, list) else (groups_after.get("groups") if isinstance(groups_after, dict) else None)
    assert isinstance(g_list, list), f"bad groups payload after restart: {groups_after!r}"
    target_group = next((g for g in g_list if isinstance(g, dict) and g.get("id") == group_id), None)
    assert target_group is not None, f"group {group_id} missing after restart: {g_list!r}"
    g_blob = json.dumps(target_group)
    assert "remoteMembers" in g_blob and "rem-sess-preserve-1" in g_blob, f"bot_groups newer JSON fields lost: {target_group!r}"

    msgs_after = api_after.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200).json()
    msgs_blob = json.dumps(msgs_after)
    assert "images" in msgs_blob and "diagram-preserve.png" in msgs_blob, f"bot_group_messages images lost: {msgs_after!r}"
    assert "remoteAsks" in msgs_blob and "rem-sess-preserve-1" in msgs_blob, f"bot_group_messages remoteAsks lost: {msgs_after!r}"


@pytest.mark.tier2
@pytest.mark.f06
def test_f06_t2_remote_members_max_31_boundary(orca_api: OrcaApiClient) -> None:
    local_bot = _create_bot(orca_api, "RemoteCapBot")
    remotes = lambda n: [{"hostId": f"host-{i}", "botId": f"rem-{i}", "name": f"Rem{i}", "chatSessionId": f"rs-{i}"} for i in range(n)]
    ok_group = _create_group(orca_api, "Max31Room", [local_bot["id"]], remoteMembers=remotes(BOT_GROUP_MAX_REMOTE_MEMBERS))
    assert isinstance(ok_group.get("remoteMembers"), list) and len(ok_group["remoteMembers"]) == BOT_GROUP_MAX_REMOTE_MEMBERS, f"expected {BOT_GROUP_MAX_REMOTE_MEMBERS} remoteMembers list, got {ok_group!r}"
    orca_api.post("/api/v1/orca/groups", json_body={"name": "Over32Room", "memberBotIds": [local_bot["id"]], "remoteMembers": remotes(BOT_GROUP_MAX_REMOTE_MEMBERS + 1)}).assert_error_code("invalid_argument", 400, 422)


@pytest.mark.tier2
@pytest.mark.f06
def test_f06_t2_mentions_mode_no_broadcast_on_unmentioned_or_former_member(orca_api: OrcaApiClient) -> None:
    alice, bob = _create_bot(orca_api, "MentAlice"), _create_bot(orca_api, "MentBob")
    group_id = _create_group(orca_api, "StrictMentionsRoom", [alice["id"], bob["id"]], {"kind": "mentions"})["id"]

    hit = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "@MentAlice ping"}).assert_status(200, 201, 202).json()
    hit_disp = (hit.get("dispatchedBotIds") if hit.get("dispatchedBotIds") is not None else hit.get("targetBotIds")) if isinstance(hit, dict) else None
    assert isinstance(hit_disp, list) and alice["id"] in hit_disp, f"expected MentAlice dispatched list, got {hit!r}"

    orca_api.assert_rpc_ok("bots.groups.update", {"groupId": group_id, "memberBotIds": [alice["id"]]})
    for text in ("Plain note with zero mentions", "@NonExistent check logs", "@MentBob should remain excluded"):
        res = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": text}).assert_status(200, 201, 202).json()
        raw_disp = (res.get("dispatchedBotIds") if res.get("dispatchedBotIds") is not None else res.get("targetBotIds")) if isinstance(res, dict) else None
        assert isinstance(raw_disp, list) and len(raw_disp) == 0, f"expected empty dispatched list for {text!r}, got {res!r}"


@pytest.mark.tier2
@pytest.mark.f06
def test_f06_t2_synthetic_roots_after_stop_and_invalid_group_targets(orca_api: OrcaApiClient) -> None:
    alice = _create_bot(orca_api, "SynthAlice")
    group_id = _create_group(orca_api, "SyntheticRootRoom", [alice["id"]], {"kind": "all"})["id"]
    orca_api.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200)

    orca_api.assert_rpc_ok("bots.groups.stop", {"groupId": group_id})
    orca_api.assert_rpc_ok("bots.groups.send", {"groupId": group_id, "syntheticRoot": "fromItems", "text": "Item decision resolved"})
    orca_api.assert_rpc_ok("bots.groups.send", {"groupId": group_id, "syntheticRoot": "fromReports", "text": "Late worker report landed"})

    msgs_blob = json.dumps(orca_api.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200).json())
    assert "fromItems" in msgs_blob and "fromReports" in msgs_blob

    orca_api.post("/api/v1/orca/groups/group-unknown-999/send", json_body={"text": "hi"}).assert_error_code("not_found", 404)
    orca_api.post("/api/v1/orca/groups/group-unknown-999/threads", json_body={"text": "hi"}).assert_error_code("not_found", 404)
    orca_api.get("/api/v1/orca/groups/group-unknown-999/messages").assert_error_code("not_found", 404)
    orca_api.post("/api/v1/orca/groups", json_body={"name": "BadLead", "memberBotIds": [alice["id"]], "replyMode": {"kind": "bot", "botId": "not-a-member"}}).assert_error_code("invalid_argument", 400, 422)


# ---------------------------------------------------------------------------
# F08 — Loopback Relay Security, OAuth Retry/Revoke, MCP Pool Debounce (Tier 2)
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f08
@pytest.mark.edge_case("SM2-12")
def test_f08_t2_relay_csrf_origin_spoofed_host_403_and_disallowed_methods_405(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer) -> None:
    bot = _create_bot(orca_api, "RelayCsrfBot")
    tool = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "sec_mcp", "transport": "http", "url": f"{mock_mcp_upstream.base_url}/mcp"})
    assert isinstance(tool, dict)
    port, grant_tok, base = _extract_relay_grant(orca_api, bot["id"])
    path = tool.get("relayPath") or f"{base.rstrip('/')}/sec_mcp"
    relay = RawHttpClient("127.0.0.1", port)
    auth = {"Authorization": f"Bearer {grant_tok}"}

    # Positive baseline before negative security checks.
    relay.request("POST", path, headers=auth, json_body={"jsonrpc": "2.0", "id": 1, "method": "ping"}).assert_status(200)
    baseline_hits = len(mock_mcp_upstream.requests)
    assert baseline_hits == 1

    relay.request("POST", path, headers={**auth, "Origin": "https://evil.example"}, json_body={"id": 2}).assert_status(403)
    relay.request("POST", path, headers={**auth, "Origin": "http://127.0.0.1:3000"}, json_body={"id": 3}).assert_status(403)
    relay.request("POST", path, headers={**auth, "Host": "evil.example.com"}, json_body={"id": 4}).assert_status(403)
    relay.request("PUT", path, headers=auth, json_body={"id": 5}).assert_status(405)
    relay.request("OPTIONS", path, headers=auth).assert_status(405)
    relay.request("PATCH", path, headers=auth, json_body={"id": 6}).assert_status(405)
    assert len(mock_mcp_upstream.requests) == baseline_hits, "blocked requests must never reach upstream"


@pytest.mark.tier2
@pytest.mark.f08
@pytest.mark.edge_case("SM2-12")
def test_f08_t2_relay_auth_401_wrong_scope_404_and_concurrency_burst_429(orca_api: OrcaApiClient) -> None:
    def _slow_responder(rec: RecordedRequest) -> tuple[int, dict[str, str], bytes]:
        time.sleep(0.35)
        return 200, {"Content-Type": "application/json"}, json.dumps({"jsonrpc": "2.0", "id": 1, "result": {"path": rec.path}}).encode()

    with MockMcpUpstreamServer(custom_responder=_slow_responder) as slow_mcp:
        bot_a, bot_b = _create_bot(orca_api, "ScopeBotA"), _create_bot(orca_api, "ScopeBotB")
        t_a = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot_a["id"], "name": "tool_a", "transport": "http", "url": f"{slow_mcp.base_url}/a"})
        t_b = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot_b["id"], "name": "tool_b", "transport": "http", "url": f"{slow_mcp.base_url}/b"})
        assert isinstance(t_a, dict) and isinstance(t_b, dict)
        port_a, tok_a, base_a = _extract_relay_grant(orca_api, bot_a["id"])
        port_b, tok_b, base_b = _extract_relay_grant(orca_api, bot_b["id"])
        path_a = t_a.get("relayPath") or f"{base_a.rstrip('/')}/tool_a"
        path_b = t_b.get("relayPath") or f"{base_b.rstrip('/')}/tool_b"

        RawHttpClient("127.0.0.1", port_a).request("POST", path_a, headers={"Authorization": f"Bearer {tok_a}"}, json_body={"id": 1}).assert_status(200)
        RawHttpClient("127.0.0.1", port_b).request("POST", path_b, headers={"Authorization": f"Bearer {tok_b}"}, json_body={"id": 2}).assert_status(200)

        RawHttpClient("127.0.0.1", port_a).request("POST", path_a, json_body={"id": 3}).assert_status(401)
        RawHttpClient("127.0.0.1", port_a).request("POST", path_a, headers={"Authorization": "Bearer not-a-grant"}, json_body={"id": 4}).assert_status(401)
        RawHttpClient("127.0.0.1", port_b).request("POST", path_b, headers={"Authorization": f"Bearer {tok_a}"}, json_body={"id": 5}).assert_status(404)

        statuses: list[int] = []
        lock = threading.Lock()

        def _worker() -> None:
            r = RawHttpClient("127.0.0.1", port_a).request("POST", path_a, headers={"Authorization": f"Bearer {tok_a}"}, json_body={"id": 9})
            with lock:
                statuses.append(r.status)

        threads = [threading.Thread(target=_worker) for _ in range(24)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15.0)
        assert 200 in statuses and 429 in statuses, f"expected mix of 200 and 429 under burst, got {statuses!r}"


@pytest.mark.tier2
@pytest.mark.f08
@pytest.mark.edge_case("SM2-13")
def test_f08_t2_relay_upstream_401_single_oauth_refresh_retry_and_no_secret_files(orca_api: OrcaApiClient, mock_oauth_server: MockOAuthServer, bot_home_factory) -> None:
    home = bot_home_factory("oauth-retry")
    bot = _create_bot(orca_api, "OAuthRetryBot", folder="oauth-retry", home_path=home.root)
    home_dir = _resolve_bot_home(bot, home.root)

    with MockMcpUpstreamServer(fail_401_once=True) as mcp_once:
        tool = _wire_oauth_tool(orca_api, bot["id"], "retry_mcp", f"{mcp_once.base_url}/mcp", mock_oauth_server)
        old_tokens = set(mock_oauth_server.active_access_tokens)
        port, grant_tok, base = _extract_relay_grant(orca_api, bot["id"])
        path = tool.get("relayPath") or f"{base.rstrip('/')}/retry_mcp"

        RawHttpClient("127.0.0.1", port).request("POST", path, headers={"Authorization": f"Bearer {grant_tok}"}, json_body={"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).assert_status(200)
        assert len(mock_oauth_server.refresh_calls) == 1
        assert len(mcp_once.requests) == 2
        second_bearer = mcp_once.requests[1].headers.get("authorization", "").removeprefix("Bearer ")
        assert second_bearer in mock_oauth_server.active_access_tokens and second_bearer not in old_tokens
        all_tokens = tuple(mock_oauth_server.active_access_tokens | mock_oauth_server.active_refresh_tokens)
        assert all_tokens, "expected active OAuth tokens to verify disk hygiene"
        _assert_no_secrets_on_disk(home_dir, all_tokens)

    def _always_401(_rec: RecordedRequest) -> tuple[int, dict[str, str], bytes]:
        return 401, {"Content-Type": "application/json", "WWW-Authenticate": 'Bearer error="invalid_token"'}, b'{"error":"invalid_token"}'

    with MockMcpUpstreamServer(custom_responder=_always_401) as mcp_401:
        tool2 = _wire_oauth_tool(orca_api, bot["id"], "always_401_mcp", f"{mcp_401.base_url}/mcp", mock_oauth_server)
        before_refreshes = len(mock_oauth_server.refresh_calls)
        port, grant_tok, base = _extract_relay_grant(orca_api, bot["id"])
        path2 = tool2.get("relayPath") or f"{base.rstrip('/')}/always_401_mcp"
        r = RawHttpClient("127.0.0.1", port).request("POST", path2, headers={"Authorization": f"Bearer {grant_tok}"}, json_body={"id": 2})
        assert r.status == 401 and len(mock_oauth_server.refresh_calls) == before_refreshes + 1
        assert len(mcp_401.requests) == 2, f"expected 1 initial + 1 post-refresh retry (no loop), got {len(mcp_401.requests)}"
        assert mcp_401.requests[0].headers.get("authorization") != mcp_401.requests[1].headers.get("authorization")
        err_code = r.extract_error_code()
        assert any(tok in r.text.lower() for tok in ("sign-in-needed", "sign_in_needed")) or err_code in {"sign-in-needed", "sign_in_needed", "invalid_token", "unauthorized"}, f"missing sign-in-needed/error on 2nd 401: {r.text!r}"


@pytest.mark.tier2
@pytest.mark.f08
@pytest.mark.edge_case("SM2-13")
def test_f08_t2_relay_revoke_aborts_open_sse_stream_and_returns_404_credential_ended(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer, bot_home_factory) -> None:
    home = bot_home_factory("sse-revoke")
    bot = _create_bot(orca_api, "SseRevokeBot", folder="sse-revoke", home_path=home.root)
    home_dir = _resolve_bot_home(bot, home.root)
    secret = "sse-upstream-secret-777"
    tool = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "sse_mcp", "transport": "http", "url": f"{mock_mcp_upstream.base_url}/sse", "headers": {"Authorization": f"Bearer {secret}"}})
    assert isinstance(tool, dict)
    port, grant_tok, base = _extract_relay_grant(orca_api, bot["id"])
    sse_path = tool.get("relayPath") or f"{base.rstrip('/')}/sse_mcp"

    def _hold_sse() -> None:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        try:
            conn.request("GET", sse_path, headers={"Authorization": f"Bearer {grant_tok}", "Accept": "text/event-stream"})
            resp = conn.getresponse()
            while resp.read(64):
                pass
        except OSError:
            pass
        finally:
            conn.close()

    holder = threading.Thread(target=_hold_sse, daemon=True)
    holder.start()
    try:
        assert mock_mcp_upstream.wait_for_stream_start(timeout=5.0), "upstream /sse stream never started"
        orca_api.assert_rpc_ok("bots.tools.revoke", {"botId": bot["id"]})
        assert mock_mcp_upstream.wait_for_stream_abort(timeout=5.0), "revoke(bot_id) failed to abort active /sse stream"
    finally:
        holder.join(timeout=5.0)

    after = RawHttpClient("127.0.0.1", port).request("POST", sse_path, headers={"Authorization": f"Bearer {grant_tok}"}, json_body={"id": 1})
    assert after.status == 404 and "CREDENTIAL_ENDED" in after.text
    _assert_no_secrets_on_disk(home_dir, (secret,))


@pytest.mark.tier2
@pytest.mark.f08
@pytest.mark.edge_case("SM2-14")
def test_f08_t2_shared_mcp_crash_caches_10s_failed_start_debounce(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    spawns_log = temp_workspace.resolve("spawns.log")
    crash_script = temp_workspace.write("crash_mcp.py", f"#!{sys.executable}\nimport os, sys\nwith open({str(spawns_log)!r}, 'a', encoding='utf-8') as f:\n    f.write(f'{{os.getpid()}}\\n')\nsys.exit(1)\n")
    crash_script.chmod(crash_script.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    orca_api.assert_rpc_ok("mcpServers.add", {"name": "crashing_mcp", "command": sys.executable, "args": [str(crash_script)], "env": {}})
    bot = _create_bot(orca_api, "CrashDebounceBot")
    orca_api.assert_rpc_ok("bots.tools.setHostMcpServers", {"botId": bot["id"], "enabled": True})
    port, grant_tok, base = _extract_relay_grant(orca_api, bot["id"])
    relay = RawHttpClient("127.0.0.1", port)
    path = f"{base.rstrip('/')}/crashing_mcp"

    for idx in range(3):
        r = relay.request("POST", path, headers={"Authorization": f"Bearer {grant_tok}"}, json_body={"jsonrpc": "2.0", "id": idx + 1, "method": "tools/list"})
        assert r.status in {500, 502, 503}, f"expected unavailable status for crashed MCP, got {r.status}"

    assert spawns_log.is_file(), "crashing stdio MCP script was never spawned even once"
    spawns = spawns_log.read_text(encoding="utf-8").splitlines()
    assert len(spawns) == 1, f"expected strictly 1 spawn inside 10s FAILED_START_MS cache, got {len(spawns)}: {spawns!r}"


@pytest.mark.tier2
@pytest.mark.f08
def test_f08_t2_oauth_ssrf_private_ip_rfc9207_issuer_mismatch_and_bad_tool_name(orca_api: OrcaApiClient) -> None:
    bot = _create_bot(orca_api, "SecurityGuardBot")
    def _assert_rejected(resp: Any, label: str) -> None:
        assert resp.status != 501
        payload = resp.json()
        err = payload.get("error") if isinstance(payload, dict) else None
        assert isinstance(payload, dict) and payload.get("ok") is False and isinstance(err, dict) and isinstance(err.get("code"), str) and err["code"] and err["code"] not in {"method_not_found", "not_implemented", "unknown_method", "internal_error", "not_found", "bot_not_found", "tool_not_found"}, f"invalid {label} rejection: {payload!r}"
    with MockOAuthServer() as good_oauth:
        orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "ok_oauth", "transport": "http", "url": "https://mcp.example.com/v1", "oauth": {"issuer": good_oauth.issuer, "allowLoopbackTest": True}})
        ok_start = orca_api.assert_rpc_ok("bots.tools.oauthStart", {"botId": bot["id"], "toolName": "ok_oauth"})
        auth_url = (ok_start.get("authorizationUrl") or ok_start.get("authUrl")) if isinstance(ok_start, dict) else None
        assert isinstance(auth_url, str) and auth_url.strip(), f"missing authorizationUrl in oauthStart baseline: {ok_start!r}"
        assert any(r.path == "/.well-known/oauth-authorization-server" for r in good_oauth.requests), "oauthStart baseline did not fetch discovery metadata"
        before_ssrf = len(good_oauth.requests)
        for idx, ssrf_url in enumerate(("http://169.254.169.254/latest/meta-data", "http://10.0.0.1/.well-known/oauth-authorization-server", good_oauth.issuer)):
            tname = f"ssrf_tool_{idx}"
            add_r = orca_api.rpc("bots.tools.add", {"botId": bot["id"], "name": tname, "transport": "http", "url": "https://mcp.example.com/v1", "oauth": {"issuer": ssrf_url}})
            assert add_r.status != 501
            add_j = add_r.json()
            if add_r.status == 200 and isinstance(add_j, dict) and add_j.get("ok") is True:
                _assert_rejected(orca_api.rpc("bots.tools.oauthStart", {"botId": bot["id"], "toolName": tname}), f"SSRF {ssrf_url}")
            else:
                _assert_rejected(add_r, f"SSRF add {ssrf_url}")
        assert len(good_oauth.requests) == before_ssrf, "loopback SSRF without allowLoopbackTest must not contact server"
    with MockOAuthServer(issuer_override="https://mismatched.issuer.example") as bad_iss_oauth:
        orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "bad_iss_tool", "transport": "http", "url": f"{bad_iss_oauth.base_url}/mcp", "oauth": {"issuer": bad_iss_oauth.base_url, "allowLoopbackTest": True}})
        _assert_rejected(orca_api.rpc("bots.tools.oauthStart", {"botId": bot["id"], "toolName": "bad_iss_tool"}), "RFC 9207 issuer mismatch")
        assert any(r.path == "/.well-known/oauth-authorization-server" for r in bad_iss_oauth.requests), "RFC 9207 metadata discovery never contacted bad_iss_oauth"
    for bad_name in ("../escape", "-leading-dash", "a" * 65):
        orca_api.assert_rpc_error("bots.tools.add", "invalid_argument", {"botId": bot["id"], "name": bad_name, "transport": "http", "url": "https://mcp.example.com/v2"})
