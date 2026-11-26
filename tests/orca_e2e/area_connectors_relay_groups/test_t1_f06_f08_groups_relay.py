"""Tier 1 Opaque-Box E2E tests for F06 (Multi-Bot Groups) & F08 (Tool Relay, OAuth 2.1, Shared MCP)."""

from __future__ import annotations

import json
import re
import stat
import sys
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

import pytest

from harness import (
    MockMcpUpstreamServer, MockOAuthServer, OrcaApiClient, OrcaCliRunner, RawHttpClient, TempWorkspace,
)

BOT_TOOL_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$")
FOLDER_SLUG_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")

def _require_dict_id(payload: Any, *, key: str | None = None, label: str = "object") -> dict[str, Any]:
    obj = payload.get(key) if key and isinstance(payload, dict) and isinstance(payload.get(key), dict) else payload
    assert isinstance(obj, dict) and obj.get("id"), f"missing {label} id in {payload!r}"
    return obj

def _create_bot(api: OrcaApiClient, name: str, *, agent: str = "claude", folder: str | None = None, home_path: Path | None = None) -> dict[str, Any]:
    body: dict[str, Any] = {"name": name, "agent": agent}
    if folder is not None:
        assert FOLDER_SLUG_RE.match(folder), f"invalid bot folder slug: {folder!r}"
        body["folder"] = folder
    if home_path is not None:
        body["homePath"] = str(home_path)
    return _require_dict_id(api.post("/api/v1/orca/bots", json_body=body).assert_status(200, 201).json(), key="bot", label="bot")

def _create_group(api: OrcaApiClient, name: str, member_ids: list[str], reply_mode: dict[str, Any], **extra: Any) -> dict[str, Any]:
    body = {"name": name, "memberBotIds": member_ids, "replyMode": reply_mode, **extra}
    return _require_dict_id(api.post("/api/v1/orca/groups", json_body=body).assert_status(200, 201).json(), key="group", label="group")

def _resolve_bot_home(bot: dict[str, Any], fallback: Path) -> Path:
    raw = bot.get("homePath") or bot.get("homeDir") or bot.get("folderPath")
    return Path(str(raw)) if isinstance(raw, str) and raw else fallback

def _unwrap_cli(payload: Any) -> Any:
    if isinstance(payload, dict):
        if payload.get("success") is True and "data" in payload:
            return payload["data"]
        if payload.get("ok") is True and "result" in payload:
            return payload["result"]
    return payload

def _extract_list(payload: Any, key: str) -> list[Any]:
    if isinstance(payload, list):
        return payload
    assert isinstance(payload, dict) and isinstance(payload.get(key), list), f"expected list or dict[{key!r}] list, got {payload!r}"
    return payload[key]

def _collect_remote_asks(payload: Any) -> list[dict[str, Any]]:
    asks: list[dict[str, Any]] = []
    if isinstance(payload, list):
        msgs = payload
    elif isinstance(payload, dict):
        raw_asks = payload.get("remoteAsks")
        assert raw_asks is None or isinstance(raw_asks, list), f"unexpected remoteAsks field: {payload!r}"
        asks.extend(a for a in (raw_asks or []) if isinstance(a, dict))
        raw_msgs = payload.get("messages")
        assert raw_msgs is None or isinstance(raw_msgs, list), f"unexpected messages field: {payload!r}"
        msgs = raw_msgs or []
    else:
        raise AssertionError(f"unexpected messages payload: {payload!r}")
    for msg in msgs:
        if isinstance(msg, dict):
            m_asks = msg.get("remoteAsks")
            assert m_asks is None or isinstance(m_asks, list), f"unexpected message remoteAsks field: {msg!r}"
            asks.extend(a for a in (m_asks or []) if isinstance(a, dict))
    return asks

def _has_bot_entry(entries: Any, bot_id: str) -> bool:
    return isinstance(entries, list) and any((isinstance(e, dict) and (e.get("botId") == bot_id or e.get("bot_id") == bot_id)) or e == bot_id for e in entries)

def _extract_mcp_text_json(body: Any) -> dict[str, Any]:
    assert isinstance(body, dict) and isinstance(body.get("result"), dict), f"missing MCP result: {body!r}"
    content = body["result"].get("content")
    assert isinstance(content, list) and content and isinstance(content[0], dict) and isinstance(content[0].get("text"), str), f"missing MCP text: {body!r}"
    parsed = json.loads(content[0]["text"])
    assert isinstance(parsed, dict), f"expected JSON object in MCP text: {parsed!r}"
    return parsed

def _extract_relay_grant(api: OrcaApiClient, bot_id: str) -> tuple[int, str, str]:
    info = api.assert_rpc_ok("bots.tools.relayInfo", {"botId": bot_id})
    grant = api.assert_rpc_ok("bots.tools.grant", {"botId": bot_id})
    port = (info.get("port") if isinstance(info, dict) else None) or (grant.get("port") if isinstance(grant, dict) else None)
    token = (grant.get("token") or grant.get("bearer")) if isinstance(grant, dict) else None
    base_path = (grant.get("path") or grant.get("basePath") or info.get("basePath") or "") if isinstance(grant, dict) and isinstance(info, dict) else ""
    assert isinstance(port, int) and port > 0, f"invalid relay port: info={info!r} grant={grant!r}"
    assert isinstance(token, str) and len(token) >= 8, f"invalid relay grant token: {grant!r}"
    return port, token, str(base_path)

def _complete_oauth_flow(api: OrcaApiClient, bot_id: str, tool_name: str, upstream_url: str, oauth: MockOAuthServer) -> None:
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

def _assert_no_secrets_on_disk(root: Path, secrets: tuple[str, ...]) -> None:
    assert root.is_dir(), f"bot home directory missing: {root}"
    assert secrets and all(isinstance(s, str) and s for s in secrets), f"empty secrets list: {secrets!r}"
    checked_files = 0
    for path in root.rglob("*"):
        if path.is_file():
            checked_files += 1
            content = path.read_text(encoding="utf-8", errors="replace")
            for secret in secrets:
                assert secret not in content, f"secret leaked into {path}"
    assert checked_files > 0, f"no bot home files scanned under {root}; check would be vacuous"

def _write_stdio_mcp_script(ws: TempWorkspace, rel_path: str) -> Path:
    script = (
        f"#!{sys.executable}\nimport json, os, sys\nfor line in sys.stdin:\n    line = line.strip()\n    if not line: continue\n"
        "    req = json.loads(line); rid, method = req.get('id'), req.get('method', '')\n"
        "    if method == 'initialize': res = {'protocolVersion': '2025-03-26', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'e2e-stdio-mcp', 'version': '1.0'}}\n"
        "    elif method == 'tools/list': res = {'tools': [{'name': 'probe', 'description': 'Return pid and env keys', 'inputSchema': {'type': 'object'}}]}\n"
        "    else: res = {'content': [{'type': 'text', 'text': json.dumps({'pid': os.getpid(), 'env_keys': sorted(os.environ.keys()), 'rev': os.environ.get('MCP_TEST_REV', '')})}]}\n"
        "    if rid is not None: sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': rid, 'result': res}) + '\\n'); sys.stdout.flush()\n"
    )
    path = ws.write(rel_path, script)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


# ---------------------------------------------------------------------------
# F06 — Multi-Bot Group Rooms, Sub-Threads, Peer Mail & Remote Bots (Tier 1)
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f06
def test_f06_t1_group_crud_and_reply_modes_rest(orca_api: OrcaApiClient) -> None:
    alice, bob = _create_bot(orca_api, "GroupAlice"), _create_bot(orca_api, "GroupBob")
    group = _create_group(orca_api, "Engineering Room", [alice["id"], bob["id"]], {"kind": "bot", "botId": alice["id"]})
    group_id = group["id"]

    listed = orca_api.get("/api/v1/orca/groups").assert_status(200).json()
    groups = _extract_list(listed, "groups")
    assert any(isinstance(g, dict) and g.get("id") == group_id for g in groups)

    send_lead = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "Status check for lead only"}).assert_status(200, 201, 202).json()
    assert isinstance(send_lead, dict) and (send_lead.get("targetBotIds") or send_lead.get("dispatchedBotIds") or []) == [alice["id"]]

    orca_api.assert_rpc_ok("bots.groups.update", {"groupId": group_id, "replyMode": {"kind": "all"}})
    send_all = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "Broadcast to all members"}).assert_status(200, 201, 202).json()
    all_targets = (send_all.get("targetBotIds") if send_all.get("targetBotIds") is not None else send_all.get("dispatchedBotIds")) if isinstance(send_all, dict) else None
    assert isinstance(all_targets, list) and len(all_targets) == 2 and alice["id"] in all_targets and bob["id"] in all_targets, f"expected all members dispatched, got {send_all!r}"

    orca_api.assert_rpc_ok("bots.groups.update", {"groupId": group_id, "replyMode": {"kind": "mentions"}})
    send_mention = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "@GroupBob check build"}).assert_status(200, 201, 202).json()
    assert isinstance(send_mention, dict) and (send_mention.get("targetBotIds") or send_mention.get("dispatchedBotIds") or []) == [bob["id"]]

    msgs = _extract_list(orca_api.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200).json(), "messages")
    assert len(msgs) >= 3


@pytest.mark.tier1
@pytest.mark.f06
def test_f06_t1_subthreads_title_cap_and_former_sessions(orca_api: OrcaApiClient) -> None:
    alice, bob = _create_bot(orca_api, "ThreadAlice"), _create_bot(orca_api, "ThreadBob")
    group = _create_group(orca_api, "Incident War Room", [alice["id"], bob["id"]], {"kind": "all"})
    group_id, root_sessions = group["id"], group.get("sessionIds")
    assert isinstance(root_sessions, dict), f"group missing sessionIds dict: {group!r}"
    assert alice["id"] in root_sessions and bob["id"] in root_sessions, f"missing root sessions: {root_sessions!r}"
    assert root_sessions[alice["id"]] and root_sessions[bob["id"]]

    long_post = "Investigate elevated checkout latency across US-East edge nodes after deployment " + ("x" * 60)
    assert len(long_post) > 80
    thread = _require_dict_id(orca_api.post(f"/api/v1/orca/groups/{group_id}/threads", json_body={"text": long_post}).assert_status(200, 201).json(), key="thread", label="thread")
    thread_id, title = thread["id"], thread.get("title", "")
    assert isinstance(title, str) and 0 < len(title) <= 80, f"title cap violated: {title!r}"

    thread_sessions = thread.get("sessionIds")
    assert isinstance(thread_sessions, dict) and alice["id"] in thread_sessions and bob["id"] in thread_sessions
    for mid in (alice["id"], bob["id"]):
        assert thread_sessions[mid] and thread_sessions[mid] != root_sessions[mid]
    bob_prior = str(thread_sessions[bob["id"]])

    orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"threadId": thread_id, "text": "Initial triage log"}).assert_status(200, 201, 202)
    updated = orca_api.assert_rpc_ok("bots.groups.update", {"groupId": group_id, "memberBotIds": [alice["id"]]})
    assert isinstance(updated, dict), f"expected updated group dict: {updated!r}"
    serialized_former = json.dumps(updated.get("formerSessionIds") or {})
    assert bob_prior in serialized_former or bob["id"] in serialized_former

    msgs = _extract_list(orca_api.get(f"/api/v1/orca/groups/{group_id}/messages", params={"thread_id": thread_id}).assert_status(200).json(), "messages")
    assert any(isinstance(m, dict) and "Initial triage log" in str(m.get("text", "")) for m in msgs)

    after = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"threadId": thread_id, "text": "Follow-up after Bob removed"}).assert_status(200, 201, 202).json()
    targets = (after.get("targetBotIds") if after.get("targetBotIds") is not None else after.get("dispatchedBotIds")) if isinstance(after, dict) else None
    assert isinstance(targets, list) and bob["id"] not in targets and alice["id"] in targets, f"expected only Alice dispatched after Bob removed, got {after!r}"


@pytest.mark.tier1
@pytest.mark.f06
def test_f06_t1_mention_handoff_and_peer_scope_mail(orca_api: OrcaApiClient) -> None:
    alice, bob = _create_bot(orca_api, "PeerAlice"), _create_bot(orca_api, "PeerBob")
    group_id = _create_group(orca_api, "Peer Mail Room", [alice["id"], bob["id"]], {"kind": "mentions"})["id"]

    handoff = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "@PeerBob take ownership of migration", "fromBotId": alice["id"], "handoff": True}).assert_status(200, 201, 202).json()
    handoff_targets = (handoff.get("targetBotIds") if handoff.get("targetBotIds") is not None else handoff.get("dispatchedBotIds")) if isinstance(handoff, dict) else None
    assert isinstance(handoff, dict) and (handoff.get("handedOff") is True or (isinstance(handoff_targets, list) and bob["id"] in handoff_targets)), f"expected handoff to PeerBob, got {handoff!r}"

    q_res = orca_api.assert_rpc_ok("bots.groups.request", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "type": "question", "text": "Which schema revision is live?"})
    assert isinstance(q_res, dict)
    peer_scope = q_res.get("peerScope") if isinstance(q_res.get("peerScope"), dict) else q_res
    for field in ("rootRequestId", "rootSequence", "sourceSessionId", "targetSessionId"):
        assert field in peer_scope and peer_scope[field] is not None, f"missing {field} in {q_res!r}"

    st = orca_api.assert_rpc_ok("bots.groups.request", {"groupId": group_id, "fromBotId": alice["id"], "toBotId": bob["id"], "type": "status", "text": "Backfill 50% complete"})
    assert isinstance(st, dict) and (st.get("mailed") is True or st.get("fireAndForget") is True or st.get("status") == "sent")


@pytest.mark.tier1
@pytest.mark.f06
def test_f06_t1_remote_members_pinned_chat_session_and_asks(orca_api: OrcaApiClient) -> None:
    local_bot = _create_bot(orca_api, "LocalCoord")
    pinned = "remote-sess-pinned-1"
    group_id = _create_group(
        orca_api, "Cross Host Room", [local_bot["id"]], {"kind": "mentions"},
        remoteMembers=[{"hostId": "host-west-01", "botId": "remote-ops-1", "name": "RemoteOps", "chatSessionId": pinned}],
    )["id"]

    orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "@RemoteOps check cluster health"}).assert_status(200, 201, 202)
    asks = _collect_remote_asks(orca_api.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200).json())
    assert asks and any(a.get("chatSessionId") == pinned and a.get("state") in {"sending", "sent", "failed", "unknown"} for a in asks), f"pinned ask mismatch: {asks!r}"

    orca_api.assert_rpc_ok("bots.groups.update", {"groupId": group_id, "remoteMembers": [{"hostId": "host-west-01", "botId": "remote-ops-1", "name": "RemoteOpsRenamed", "chatSessionId": pinned}]})
    read_back = orca_api.assert_rpc_ok("bots.groups.read", {"groupId": group_id})
    assert isinstance(read_back, dict) and isinstance(read_back.get("remoteMembers"), list) and any(isinstance(r, dict) and r.get("name") == "RemoteOpsRenamed" and r.get("chatSessionId") == pinned for r in read_back["remoteMembers"]), f"renamed remoteMember missing: {read_back!r}"

    orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "@RemoteOpsRenamed confirm post-rename session"}).assert_status(200, 201, 202)
    asks_after = _collect_remote_asks(orca_api.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200).json())
    pinned_asks = [a for a in asks_after if a.get("chatSessionId") == pinned and a.get("state") in {"sending", "sent", "failed", "unknown"}]
    assert len(asks_after) >= len(asks) + 1 and len(pinned_asks) >= 2, f"post-rename pinned ask mismatch: {asks_after!r}"


@pytest.mark.tier1
@pytest.mark.f06
def test_f06_t1_paused_bot_mail_hold_and_resume_delivery(orca_api: OrcaApiClient) -> None:
    alice, bob = _create_bot(orca_api, "ActiveAlice"), _create_bot(orca_api, "PausedBob")
    group_id = _create_group(orca_api, "Pause Hold Room", [alice["id"], bob["id"]], {"kind": "all"})["id"]

    orca_api.patch(f"/api/v1/orca/bots/{bob['id']}", json_body={"paused": True}).assert_status(200)
    sent = orca_api.post(f"/api/v1/orca/groups/{group_id}/send", json_body={"text": "Hold this update until Bob resumes"}).assert_status(200, 201, 202).json()
    assert isinstance(sent, dict)
    dispatched = sent.get("dispatchedBotIds") if sent.get("dispatchedBotIds") is not None else sent.get("targetBotIds")
    held = sent.get("heldBotIds") if sent.get("heldBotIds") is not None else sent.get("pendingBotIds")
    assert isinstance(dispatched, list) and isinstance(held, list), f"expected dispatched/held lists, got {sent!r}"
    assert alice["id"] in dispatched and bob["id"] in held and bob["id"] not in dispatched

    paused_state = orca_api.assert_rpc_ok("bots.groups.read", {"groupId": group_id})
    assert isinstance(paused_state, dict)
    paused_pending = paused_state.get("pending") if paused_state.get("pending") is not None else paused_state.get("heldMail")
    assert _has_bot_entry(paused_pending, bob["id"]), f"expected Bob in paused pending/heldMail: {paused_state!r}"

    orca_api.patch(f"/api/v1/orca/bots/{bob['id']}", json_body={"paused": False}).assert_status(200)
    resumed_state = orca_api.assert_rpc_ok("bots.groups.read", {"groupId": group_id})
    assert isinstance(resumed_state, dict)
    resumed_pending = resumed_state.get("pending") if resumed_state.get("pending") is not None else resumed_state.get("heldMail")
    assert isinstance(resumed_pending, list) and not _has_bot_entry(resumed_pending, bob["id"]), f"Bob still pending after resume: {resumed_state!r}"

    msgs_payload = orca_api.get(f"/api/v1/orca/groups/{group_id}/messages").assert_status(200).json()
    msgs = _extract_list(msgs_payload, "messages")
    turns = (msgs_payload.get("turns") if isinstance(msgs_payload, dict) else None) or (msgs_payload.get("deliveries") if isinstance(msgs_payload, dict) else None) or resumed_state.get("turns") or resumed_state.get("deliveries") or resumed_state.get("delivered") or []
    delivered_bob = _has_bot_entry(turns, bob["id"]) or any(
        isinstance(m, dict) and (m.get("botId") == bob["id"] or m.get("senderBotId") == bob["id"] or m.get("fromBotId") == bob["id"] or _has_bot_entry(m.get("deliveredBotIds") or m.get("dispatchedBotIds") or m.get("deliveries"), bob["id"]))
        for m in msgs
    )
    assert delivered_bob, f"expected positive resume delivery to Bob: state={resumed_state!r} msgs={msgs_payload!r}"


@pytest.mark.tier1
@pytest.mark.f06
def test_f06_t1_cli_and_rpc_group_parity(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    alice, bob = _create_bot(orca_api, "CliAlice"), _create_bot(orca_api, "CliBob")
    group_id = _require_dict_id(_unwrap_cli(orca_cli.run("bot", "group-create", "--name", "CLI Sync Room", "--members", f"{alice['id']},{bob['id']}").assert_ok()), key="group", label="cli group")["id"]
    thread_id = _require_dict_id(_unwrap_cli(orca_cli.run("bot", "group-thread", group_id, "--text", "CLI opened sub-thread").assert_ok()), key="thread", label="cli thread")["id"]
    orca_cli.run("bot", "group-send", group_id, "--thread", thread_id, "--text", "CLI parity message").assert_ok()

    rpc_group = orca_api.assert_rpc_ok("bots.groups.read", {"groupId": group_id})
    assert isinstance(rpc_group, dict)
    serialized = json.dumps(rpc_group)
    assert rpc_group.get("name") == "CLI Sync Room" and thread_id in serialized and "CLI parity message" in serialized


# ---------------------------------------------------------------------------
# F08 — Loopback Tool Relay, Host OAuth 2.1 & Shared MCP Pool (Tier 1)
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f08
def test_f08_t1_relay_http_proxy_static_headers_and_mcp_session_tracking(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer, bot_home_factory) -> None:
    home = bot_home_factory("relay-static")
    bot = _create_bot(orca_api, "RelayStaticBot", folder="relay-static", home_path=home.root)
    home_dir = _resolve_bot_home(bot, home.root)
    s1, s2 = "upstream-static-secret-xyz", "secret-key-999"
    added = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "docs_mcp", "transport": "http", "url": f"{mock_mcp_upstream.base_url}/mcp", "headers": {"Authorization": f"Bearer {s1}", "X-Api-Key": s2}})
    assert isinstance(added, dict)
    port, grant_tok, base_path = _extract_relay_grant(orca_api, bot["id"])
    tool_path = added.get("relayPath") or f"{base_path.rstrip('/')}/docs_mcp"
    relay = RawHttpClient("127.0.0.1", port)
    hdrs = {"Authorization": f"Bearer {grant_tok}"}

    r_post = relay.request("POST", tool_path, headers=hdrs, json_body={"jsonrpc": "2.0", "id": 1, "method": "initialize"}).assert_status(200)
    sess_id = r_post.header("mcp-session-id")
    assert sess_id, f"missing mcp-session-id header: {r_post.headers!r}"
    relay.request("GET", tool_path, headers={**hdrs, "Mcp-Session-Id": sess_id}).assert_status(200)
    relay.request("DELETE", tool_path, headers={**hdrs, "Mcp-Session-Id": sess_id}).assert_status(200)

    assert len(mock_mcp_upstream.requests) >= 3
    first = mock_mcp_upstream.requests[0]
    assert first.headers.get("authorization") == f"Bearer {s1}"
    assert first.headers.get("x-api-key") == s2
    assert grant_tok not in json.dumps(first.headers)
    assert any(r.method == "DELETE" and r.headers.get("mcp-session-id") == sess_id for r in mock_mcp_upstream.requests)
    _assert_no_secrets_on_disk(home_dir, (s1, s2))


@pytest.mark.tier1
@pytest.mark.f08
def test_f08_t1_oauth21_pkce_dcr_rfc8707_9207_7009_end_to_end(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer, mock_oauth_server: MockOAuthServer) -> None:
    bot = _create_bot(orca_api, "OAuthBot")
    upstream_url = f"{mock_mcp_upstream.base_url}/mcp"
    tool = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "oauth_mcp", "transport": "http", "url": upstream_url, "oauth": {"issuer": mock_oauth_server.issuer, "allowLoopbackTest": True}})
    assert isinstance(tool, dict)
    _complete_oauth_flow(orca_api, bot["id"], "oauth_mcp", upstream_url, mock_oauth_server)

    port, grant_tok, base_path = _extract_relay_grant(orca_api, bot["id"])
    tool_path = tool.get("relayPath") or f"{base_path.rstrip('/')}/oauth_mcp"
    RawHttpClient("127.0.0.1", port).request("POST", tool_path, headers={"Authorization": f"Bearer {grant_tok}"}, json_body={"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).assert_status(200)
    assert mock_mcp_upstream.requests, "relay never forwarded OAuth request to upstream"
    auth_hdr = mock_mcp_upstream.requests[-1].headers.get("authorization", "")
    assert auth_hdr.startswith("Bearer ") and auth_hdr.removeprefix("Bearer ") in mock_oauth_server.active_access_tokens

    orca_api.assert_rpc_ok("bots.tools.oauthRevoke", {"botId": bot["id"], "toolName": "oauth_mcp"})
    assert mock_oauth_server.revoke_calls, "RFC 7009 POST /revoke not called"


@pytest.mark.tier1
@pytest.mark.f08
def test_f08_t1_shared_stdio_mcp_pool_multiplexing_and_env_isolation(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    script = _write_stdio_mcp_script(temp_workspace, "mcp_pool.py")
    orca_api.assert_rpc_ok("mcpServers.add", {"name": "shared_probe", "command": sys.executable, "args": [str(script)], "env": {"MCP_TEST_REV": "pool-1"}})
    bot_a, bot_b = _create_bot(orca_api, "PoolBotA"), _create_bot(orca_api, "PoolBotB")
    for b in (bot_a, bot_b):
        orca_api.assert_rpc_ok("bots.tools.setHostMcpServers", {"botId": b["id"], "enabled": True})

    def _call_probe(bot_id: str) -> dict[str, Any]:
        port, tok, base = _extract_relay_grant(orca_api, bot_id)
        body = RawHttpClient("127.0.0.1", port).request("POST", f"{base.rstrip('/')}/shared_probe", headers={"Authorization": f"Bearer {tok}"}, json_body={"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "probe", "arguments": {}}}).assert_status(200).json()
        return _extract_mcp_text_json(body)

    p_a, p_b = _call_probe(bot_a["id"]), _call_probe(bot_b["id"])
    assert p_a.get("pid") and p_a["pid"] == p_b.get("pid"), f"shared stdio pool must multiplex same PID: {p_a} vs {p_b}"
    env_keys = p_a.get("env_keys")
    assert isinstance(env_keys, list), f"missing env_keys list: {p_a!r}"
    leaked = [k for k in env_keys if k in {"FABUSHI_TOKEN", "GPUI_TELEGRAM_TOKEN", "ORCA_AUTH_TOKEN", "ORCA_RELAY_SECRET"}]
    assert not leaked, f"inherited Orca identity env vars leaked: {leaked}"


@pytest.mark.tier1
@pytest.mark.f08
def test_f08_t1_tool_auto_naming_and_host_mcp_servers_exclusive_gate(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer) -> None:
    bot = _create_bot(orca_api, "AutoNameBot")
    assert bot.get("hostMcpServers") is False
    added = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "transport": "http", "url": f"{mock_mcp_upstream.base_url}/mcp/api/server/linear"})
    assert isinstance(added, dict)
    derived = str(added.get("name", ""))
    assert BOT_TOOL_NAME_RE.match(derived), f"invalid derived name: {derived!r}"
    assert derived not in {"mcp", "api", "server", "com", "io", "dev", "ai"}

    orca_api.patch(f"/api/v1/orca/bots/{bot['id']}", json_body={"hostMcpServers": True})
    after_patch = orca_api.get(f"/api/v1/orca/bots/{bot['id']}").assert_status(200).json()
    assert isinstance(after_patch, dict) and after_patch.get("hostMcpServers") is False, "generic PATCH must NOT enable hostMcpServers"
    orca_api.assert_rpc_ok("bots.tools.setHostMcpServers", {"botId": bot["id"], "enabled": True})
    after_gate = orca_api.get(f"/api/v1/orca/bots/{bot['id']}").assert_status(200).json()
    assert isinstance(after_gate, dict) and after_gate.get("hostMcpServers") is True


@pytest.mark.tier1
@pytest.mark.f08
def test_f08_t1_shared_stdio_mcp_restarts_on_config_edit(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    script = _write_stdio_mcp_script(temp_workspace, "mcp_restart.py")
    orca_api.assert_rpc_ok("mcpServers.add", {"name": "restart_probe", "command": sys.executable, "args": [str(script)], "env": {"MCP_TEST_REV": "v1"}})
    bot = _create_bot(orca_api, "RestartPoolBot")
    orca_api.assert_rpc_ok("bots.tools.setHostMcpServers", {"botId": bot["id"], "enabled": True})
    port, tok, base = _extract_relay_grant(orca_api, bot["id"])
    relay = RawHttpClient("127.0.0.1", port)
    call_body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "probe", "arguments": {}}}

    r1 = _extract_mcp_text_json(relay.request("POST", f"{base.rstrip('/')}/restart_probe", headers={"Authorization": f"Bearer {tok}"}, json_body=call_body).assert_status(200).json())
    assert r1.get("rev") == "v1" and r1.get("pid")
    orca_api.assert_rpc_ok("mcpServers.update", {"name": "restart_probe", "command": sys.executable, "args": [str(script)], "env": {"MCP_TEST_REV": "v2"}})
    r2 = _extract_mcp_text_json(relay.request("POST", f"{base.rstrip('/')}/restart_probe", headers={"Authorization": f"Bearer {tok}"}, json_body=call_body).assert_status(200).json())
    assert r2.get("rev") == "v2" and r2.get("pid") and r2["pid"] != r1["pid"]


@pytest.mark.tier1
@pytest.mark.f08
def test_f08_t1_claude_mirror_secret_scrub_and_opaque_handle_only(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer, bot_home_factory) -> None:
    home = bot_home_factory("mirror-scrub")
    leaked_secret, vault_secret = "legacy-leaked-token-123", "vault-only-token-999"
    bot = _create_bot(orca_api, "ScrubBot", folder="mirror-scrub", home_path=home.root)
    home_dir = _resolve_bot_home(bot, home.root)
    mcp_path = home_dir / ".mcp.json"
    legacy_payload = json.dumps({"mcpServers": {"legacy": {"url": mock_mcp_upstream.base_url, "headers": {"Authorization": f"Bearer {leaked_secret}"}}}})
    mcp_path.write_text(legacy_payload, encoding="utf-8")
    orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot["id"], "name": "clean_tool", "transport": "http", "url": f"{mock_mcp_upstream.base_url}/mcp", "headers": {"Authorization": f"Bearer {vault_secret}"}})
    _assert_no_secrets_on_disk(home_dir, (leaked_secret, vault_secret))
    assert mcp_path.is_file(), f"expected scrubbed .mcp.json at {mcp_path}"
    mcp_mirror = mcp_path.read_text(encoding="utf-8")
    assert mcp_mirror != legacy_payload and leaked_secret not in mcp_mirror and vault_secret not in mcp_mirror
    assert "127.0.0.1" in mcp_mirror or "clean_tool" in mcp_mirror
