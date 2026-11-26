use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::dynamic_island::{
    DynamicIslandAgentIdentity, DynamicIslandFailureItem, DynamicIslandMode,
    DynamicIslandPreference, DynamicIslandPresentation, DynamicIslandPromptItem,
    DynamicIslandWorkingItem, dynamic_island_compact_height, interaction_locks_presentation,
};
use openbot_contracts::ipc::{
    AgentEvent, IPC_ENDPOINTS, IPC_NESTED_API_PATHS, IpcEndpointKind, IpcGroupName,
    ScopedAgentEvent, ScopedRequest, endpoints_in_group, group_api_method_name,
};
use openbot_contracts::limits::RequestId;
use openbot_contracts::team_protocol::{
    TEAM_ABSENT_CAPABILITIES_HEADER, TEAM_API_ROUTES, TEAM_CAPABILITIES_HEADER,
    TEAM_CAPABILITY_REGISTRY, TEAM_LONG_CAPABILITY_LISTS_CAPABILITY, TEAM_PROTOCOL_V2,
    TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET, TEAM_PROTOCOL_V9, TEAM_PROTOCOL_V10,
    TEAM_WEB_CLIENT_PROTOCOL, TEAM_WEBRTC_CLIENT_PROTOCOL, TeamProtocolRange,
    TeamProtocolUpdateDirection, TeamProtocolV2FileChunk, decode_team_protocol_v2_file_chunk,
    encode_team_capabilities_v10, encode_team_protocol_v2_file_chunk,
    expand_team_capabilities_v10, highest_common_team_protocol,
    team_capabilities_for_protocol, team_capability_headers,
    team_protocol_update_direction, team_protocol_v10_baseline_capabilities,
    to_current_agent_keys, to_wire_agent_keys, to_wire_agent_keys_for_path,
};
use openbot_contracts::voice::{
    CODEX_LIVE_VOICES, SPEECH_MODEL_IDS, VOICE_AUDIO_LIMITS, VOICE_MODEL_IDS, VoiceConfig,
    VoiceModelId, VoiceServerProbe, VoiceServerProbeField, decode_voice_server_settings,
    default_voice_settings, model_language, next_speech_speed,
};

#[test]
fn voice_models_dynamic_island_lock_and_65_ipc_groups() {
    assert_eq!(VOICE_AUDIO_LIMITS.sample_rate, 16_000);
    assert_eq!(VOICE_MODEL_IDS.len(), 8);
    assert_eq!(SPEECH_MODEL_IDS.len(), 7);
    assert_eq!(CODEX_LIVE_VOICES.len(), 25);
    assert_eq!(default_voice_settings("en-US").model_id, VoiceModelId::ParakeetTdt06bV3);
    assert_eq!(default_voice_settings("pl-PL").model_id, VoiceModelId::ParakeetTdt06bV3);
    assert_eq!(default_voice_settings("zz-ZZ").model_id, VoiceModelId::ParakeetTdt06bV3);
    assert_eq!(default_voice_settings("nb-NO").model_id, VoiceModelId::WhisperTurbo);
    assert_eq!(default_voice_settings("ja-JP").model_id, VoiceModelId::WhisperTurbo);
    assert_eq!(model_language(VoiceModelId::CohereTranscribe032026, "sv"), "en");
    assert_eq!(model_language(VoiceModelId::CohereTranscribe032026, "fr"), "fr");
    assert_eq!(next_speech_speed(1.0), 1.25);
    assert_eq!(next_speech_speed(2.0), 0.75);
    assert_eq!(next_speech_speed(0.75), 1.0);
    assert!(decode_voice_server_settings(&serde_json::json!({
        "address": "http://127.0.0.1:8080",
        "hasApiKey": true,
        "apiKey": "leaked-secret",
        "routes": { "dictation": true, "speech": false, "transcriptionModel": "whisper", "speechModel": null, "voice": null }
    }))
    .is_err());

    // Direct serde_json deserialization of VoiceConfig also rejects leaked server.apiKey!
    let leaked_cfg = serde_json::json!({
        "voice": serde_json::to_value(VoiceConfig::default().voice).unwrap(),
        "speech": serde_json::to_value(VoiceConfig::default().speech).unwrap(),
        "server": {
            "address": "http://127.0.0.1:8080",
            "hasApiKey": true,
            "apiKey": "leaked-secret",
            "routes": { "dictation": false, "speech": false, "transcriptionModel": null, "speechModel": null, "voice": null }
        }
    });
    assert!(serde_json::from_value::<VoiceConfig>(leaked_cfg).is_err());

    // VoiceServerProbe boolean-discriminated serde (Failed is never shadowed by Ok)
    let probe_err: VoiceServerProbe = serde_json::from_str(
        r#"{"ok":false,"field":"apiKey","message":"Invalid API key"}"#,
    )
    .unwrap();
    assert_eq!(
        probe_err,
        VoiceServerProbe::Failed {
            field: VoiceServerProbeField::ApiKey,
            message: "Invalid API key".into(),
        }
    );
    assert!(serde_json::from_str::<VoiceServerProbe>(
        r#"{"ok":false,"field":"unknown","message":"bad"}"#
    )
    .is_err());

    // Dynamic Island priority 0..=6, server_visibility 1..=128, & critical prompt lock invariant
    assert_eq!(dynamic_island_compact_height(None, 100), 32);
    let mut pref = DynamicIslandPreference::default();
    assert!(pref.is_valid());
    pref.server_visibility.insert("s".repeat(128), true);
    assert!(pref.is_valid());
    pref.server_visibility.insert("s".repeat(129), false);
    assert!(!pref.is_valid());
    assert_eq!(DynamicIslandMode::Question.priority(), 0);
    assert_eq!(DynamicIslandMode::Idle.priority(), 6);

    let ident = DynamicIslandAgentIdentity {
        id: "a1".into(),
        name: "Atlas".into(),
        avatar_seed: "a1".into(),
        avatar_hue: Some(AvatarHue::Blue),
        avatar_color: None,
        avatar_url: None,
    };
    let question_pres = DynamicIslandPresentation::Question {
        server_id: "local".into(),
        item: DynamicIslandPromptItem {
            request_id: RequestId::String("req-1".into()),
            agent: ident.clone(),
            title: "Confirm?".into(),
            detail: None,
            questions: vec![],
        },
        remaining_count: 1,
    };
    let other_question = DynamicIslandPresentation::Question {
        server_id: "local".into(),
        item: DynamicIslandPromptItem {
            request_id: RequestId::String("req-2".into()),
            agent: ident.clone(),
            title: "Other?".into(),
            detail: None,
            questions: vec![],
        },
        remaining_count: 1,
    };
    let working_pres = DynamicIslandPresentation::Working {
        server_id: "local".into(),
        working: vec![DynamicIslandWorkingItem {
            agent: ident.clone(),
            task: "Building".into(),
            turn_id: Some("t1".into()),
        }],
    };
    let failed_pres = DynamicIslandPresentation::Failed {
        server_id: "local".into(),
        item: DynamicIslandFailureItem { turn_id: "t1".into(), agent: ident, title: "Boom".into(), detail: None },
    };
    assert!(question_pres.is_valid());
    assert!(failed_pres.is_valid());
    // Holds critical Question against another Question while user is interacting
    assert!(interaction_locks_presentation(&question_pres, &other_question, true, false));
    // Immediately unlocks when critical Question resolves to lower-urgency Working!
    assert!(!interaction_locks_presentation(&question_pres, &working_pres, true, false));

    // DynamicIslandAgentIdentity bounds: id 1..=160, name 1..=120, avatarUrl null or 1..=2048
    let ident_100_name = DynamicIslandAgentIdentity {
        id: "i".repeat(160),
        name: "n".repeat(100),
        avatar_seed: "seed".into(),
        avatar_hue: None,
        avatar_color: None,
        avatar_url: Some("https://example.com/a.png".into()),
    };
    assert!(ident_100_name.is_valid());
    assert!(!DynamicIslandAgentIdentity { name: "".into(), ..ident_100_name.clone() }.is_valid());
    assert!(!DynamicIslandAgentIdentity { avatar_url: Some("".into()), ..ident_100_name }.is_valid());

    // All 65 IPC groups & 50 nested paths
    assert!(!IPC_ENDPOINTS.is_empty());
    assert_eq!(IpcGroupName::ALL.len(), 65);
    assert_eq!(IPC_NESTED_API_PATHS.len(), 50);
    for group in IpcGroupName::ALL {
        assert!(!endpoints_in_group(group).is_empty(), "missing group {group:?}");
    }
    assert_eq!(group_api_method_name("setupState", IpcEndpointKind::Event), "onSetupState");
    assert_eq!(group_api_method_name("listAgents", IpcEndpointKind::ScopedQuery), "listAgents");

    let req = ScopedRequest { server_id: "srv-1".into(), payload: 42u32 };
    assert_eq!(serde_json::to_value(&req).unwrap()["serverId"], "srv-1");
    let ev = ScopedAgentEvent {
        server_id: "srv-1".into(),
        event: AgentEvent::CrmChanged,
        buffered_live: Some(true),
    };
    assert!(ev.is_valid());
}

#[test]
fn team_protocol_v10_309_capabilities_wire_keys_and_webrtc_framing() {
    assert_eq!(TEAM_CAPABILITY_REGISTRY.len(), 309);
    assert_eq!(team_protocol_v10_baseline_capabilities().len(), 256);
    assert_eq!(team_capabilities_for_protocol(Some(TEAM_PROTOCOL_V10)).len(), 309);
    assert_eq!(team_capabilities_for_protocol(Some(TEAM_PROTOCOL_V9)).len(), 256);

    // v10 encode/expand round-trip preserving `long-capability-lists-v1` and tracking `absent`
    let mut served: Vec<String> = team_capabilities_for_protocol(Some(TEAM_PROTOCOL_V10))
        .into_iter()
        .filter(|&c| c != "crm-v1")
        .map(str::to_string)
        .collect();
    served.push("experimental-custom-v1".into());
    let encoded = encode_team_capabilities_v10(&served);
    assert!(encoded.capabilities.contains(&TEAM_LONG_CAPABILITY_LISTS_CAPABILITY.to_string()));
    assert!(encoded.capabilities.contains(&"experimental-custom-v1".to_string()));
    assert_eq!(encoded.absent, vec!["crm-v1".to_string()]);
    let headers = team_capability_headers(Some(TEAM_PROTOCOL_V10), &served);
    assert!(headers.contains_key(TEAM_CAPABILITIES_HEADER));
    assert_eq!(headers.get(TEAM_ABSENT_CAPABILITIES_HEADER).map(String::as_str), Some("crm-v1"));

    let expanded = expand_team_capabilities_v10(&encoded);
    assert!(!expanded.contains(&"crm-v1".to_string()));
    assert!(expanded.contains(&"experimental-custom-v1".to_string()));
    assert_eq!(expanded.len(), 309);

    // Version negotiation
    assert_eq!(
        highest_common_team_protocol(TEAM_WEBRTC_CLIENT_PROTOCOL, TEAM_WEB_CLIENT_PROTOCOL),
        Some(9)
    );
    assert_eq!(
        team_protocol_update_direction(
            TeamProtocolRange { minimum: 3, maximum: 8 },
            TeamProtocolRange { minimum: 9, maximum: 10 }
        ),
        Some(TeamProtocolUpdateDirection::ClientUpdateRequired)
    );

    // Bidirectional botId <-> agentId legacy wire key walker + opaque `layout` + untranslated path
    let current = serde_json::json!({
        "type": "agents-changed",
        "agentId": "a1",
        "recipientAgentIds": ["a2"],
        "kind": "agent",
        "marketplaceSource": { "listingId": "mkt-9", "version": 1 },
        "layout": { "agentId": "keep-untouched" },
        "answers": { "agentId": "a3" }
    });
    let wire = to_wire_agent_keys(&current);
    assert_eq!(wire["type"], "bots-changed");
    assert_eq!(wire["botId"], "a1");
    assert_eq!(wire["recipientBotIds"], serde_json::json!(["a2"]));
    assert_eq!(wire["kind"], "bot");
    assert_eq!(wire["marketplaceSource"]["agentId"], "mkt-9");
    assert_eq!(wire["layout"]["agentId"], "keep-untouched");
    assert_eq!(wire["answers"]["agentId"], "a3");
    assert_eq!(to_current_agent_keys(&wire), current);
    assert_eq!(to_wire_agent_keys_for_path(TEAM_API_ROUTES.sidebar_layout, &current), current);

    // WebRTC DataChannel v2 binary file chunk `[2, id_len, id_utf8, u64_be_offset, bytes]`
    let chunk = TeamProtocolV2FileChunk {
        transfer_id: "xfer-42".into(),
        offset: 131_072,
        bytes: b"hello-openbot-webrtc".to_vec(),
    };
    let bin = encode_team_protocol_v2_file_chunk(&chunk, None).expect("valid binary chunk");
    assert_eq!(bin[0], 2);
    assert_eq!(bin[1], 7);
    let roundtrip = decode_team_protocol_v2_file_chunk(&bin).expect("decoded binary chunk");
    assert_eq!(roundtrip, chunk);

    // WebRTC v2 safe integer offset guard (`<= 2^53 - 1` accepted, `>= 2^53` rejected on encode & decode)
    let max_safe_chunk = TeamProtocolV2FileChunk {
        transfer_id: "x".into(),
        offset: TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET,
        bytes: vec![1],
    };
    let max_bin = encode_team_protocol_v2_file_chunk(&max_safe_chunk, None).unwrap();
    assert_eq!(decode_team_protocol_v2_file_chunk(&max_bin).unwrap().offset, TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET);

    let unsafe_chunk = TeamProtocolV2FileChunk {
        transfer_id: "x".into(),
        offset: TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET + 1,
        bytes: vec![1],
    };
    assert!(encode_team_protocol_v2_file_chunk(&unsafe_chunk, None).is_err());

    let mut raw_unsafe = vec![TEAM_PROTOCOL_V2 as u8, 1, b'x'];
    raw_unsafe.extend_from_slice(&(TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET + 1).to_be_bytes());
    raw_unsafe.push(0x42);
    assert!(decode_team_protocol_v2_file_chunk(&raw_unsafe).is_err());
}

#[test]
fn iter3_dynamic_island_approval_permissions_and_required_nullable_identity() {
    // Omitted required nullable `avatarHue` or `avatarUrl` must fail deserialization
    assert!(
        serde_json::from_value::<DynamicIslandAgentIdentity>(serde_json::json!({
            "id": "a1",
            "name": "Atlas",
            "avatarSeed": "seed",
            "avatarUrl": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<DynamicIslandAgentIdentity>(serde_json::json!({
            "id": "a1",
            "name": "Atlas",
            "avatarSeed": "seed",
            "avatarHue": 215
        }))
        .is_err()
    );
    let ident_ok: DynamicIslandAgentIdentity = serde_json::from_value(serde_json::json!({
        "id": "a1",
        "name": "Atlas",
        "avatarSeed": "seed",
        "avatarHue": 215,
        "avatarUrl": null
    }))
    .unwrap();
    assert_eq!(ident_ok.avatar_hue, Some(AvatarHue::Blue));
    assert!(ident_ok.is_valid());

    let approval_json = |permissions: serde_json::Value| {
        serde_json::json!({
            "serverId": "local",
            "mode": "approval",
            "remainingCount": 0,
            "item": {
                "requestId": "req-1",
                "agent": {
                    "id": "a1",
                    "name": "Atlas",
                    "avatarSeed": "seed",
                    "avatarHue": null,
                    "avatarUrl": null
                },
                "title": "Allow permissions?",
                "detail": null,
                "truncated": false,
                "approval": {
                    "kind": "permissions",
                    "command": null,
                    "cwd": null,
                    "reason": null,
                    "grantRoot": null,
                    "permissions": permissions
                }
            }
        })
    };

    // Null permissions -> valid
    let null_perms: DynamicIslandPresentation =
        serde_json::from_value(approval_json(serde_json::Value::Null)).unwrap();
    assert!(null_perms.is_valid());

    // Valid structured permissions -> valid
    let ok_perms: DynamicIslandPresentation = serde_json::from_value(approval_json(serde_json::json!({
        "network": true,
        "fileSystem": {
            "read": ["/workspace/src"],
            "write": ["/workspace/out", "/tmp/cache"]
        }
    })))
    .unwrap();
    assert!(ok_perms.is_valid());

    // Bogus permissions object -> rejected on deserialize
    assert!(
        serde_json::from_value::<DynamicIslandPresentation>(approval_json(
            serde_json::json!({"bogus": 123})
        ))
        .is_err()
    );

    // > 3 paths or empty path or > 600 UTF-16 units -> rejected by is_valid()
    let too_many_reads: DynamicIslandPresentation =
        serde_json::from_value(approval_json(serde_json::json!({
            "network": false,
            "fileSystem": {
                "read": ["/a", "/b", "/c", "/d"],
                "write": []
            }
        })))
        .unwrap();
    assert!(!too_many_reads.is_valid());

    let empty_write_path: DynamicIslandPresentation =
        serde_json::from_value(approval_json(serde_json::json!({
            "network": false,
            "fileSystem": {
                "read": [],
                "write": [""]
            }
        })))
        .unwrap();
    assert!(!empty_write_path.is_valid());

    let overlong_path: DynamicIslandPresentation =
        serde_json::from_value(approval_json(serde_json::json!({
            "network": false,
            "fileSystem": {
                "read": ["x".repeat(601)],
                "write": []
            }
        })))
        .unwrap();
    assert!(!overlong_path.is_valid());
}


#[test]
fn iter4_island_voice_ipc_and_team_protocol_strict_wire_parity() {
    use openbot_contracts::dynamic_island::DynamicIslandNotchSize;
    use openbot_contracts::team_protocol::{
        TeamProtocolSupport, TeamProtocolV2RpcFrame, decode_team_protocol_support_expanded,
    };
    use openbot_contracts::voice::{SpeechSettings, VoiceLiveEvent, VoiceServerConnection};

    // 1. Challenger 2 Defect 1 (`di.perms.extra_key_check`) & DynamicIslandNotchSize positive finite
    let extra_perms_pres: DynamicIslandPresentation = serde_json::from_value(serde_json::json!({
        "serverId": "local",
        "mode": "approval",
        "remainingCount": 0,
        "item": {
            "requestId": "req-extra",
            "agent": {
                "id": "a1",
                "name": "Atlas",
                "avatarSeed": "seed",
                "avatarHue": null,
                "avatarUrl": null
            },
            "title": "Allow permissions?",
            "detail": null,
            "truncated": false,
            "approval": {
                "kind": "permissions",
                "command": null,
                "cwd": null,
                "reason": null,
                "grantRoot": null,
                "permissions": {
                    "network": true,
                    "fileSystem": { "read": ["/a"], "write": [], "extraFs": 1 },
                    "extraPerm": true
                }
            }
        }
    }))
    .expect("DynamicIslandApprovalPermissions must allow forward-compatible extra keys");
    assert!(extra_perms_pres.is_valid());

    let notch: DynamicIslandNotchSize =
        serde_json::from_str(r#"{"width":192.5,"height":32}"#).unwrap();
    assert!(notch.is_valid());
    assert!(serde_json::from_str::<DynamicIslandNotchSize>(r#"{"width":0,"height":32}"#).is_err());
    assert!(serde_json::from_str::<DynamicIslandNotchSize>(r#"{"width":192,"height":-1}"#).is_err());

    // 2. Challenger 2 Defect 3.1 (`voice.settings.omitted_route_models`, etc.)
    assert!(decode_voice_server_settings(&serde_json::json!({
        "address": "http://localhost:8000",
        "hasApiKey": false,
        "routes": { "dictation": false, "speech": false }
    }))
    .is_err());
    assert!(serde_json::from_str::<SpeechSettings>(
        r#"{"readReplies":false,"modelId":"supertonic-3","voice":"F1","elevenLabsVoice":"alice","xaiVoice":"eve","speed":1.0,"codexLive":false,"turnPauseMs":500}"#
    )
    .is_err());
    assert!(serde_json::from_str::<VoiceLiveEvent>(
        r#"{"type":"ended","sessionId":"s1"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<VoiceLiveEvent>(
        r#"{"type":"transcript","sessionId":"s1","role":"system","text":"hi","final":true}"#
    )
    .is_err());
    let live_ok: VoiceLiveEvent = serde_json::from_str(
        r#"{"type":"transcript","sessionId":"s1","role":"assistant","text":"hello","final":true}"#,
    )
    .unwrap();
    assert!(live_ok.is_valid());
    assert!(serde_json::from_str::<VoiceServerConnection>(r#"{"address":"http://127.0.0.1:8000"}"#).is_err());

    // 3. IPC AgentEvent required nullables, optional literal-true flags, & constrained strings
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"agent-notification-levels-changed","agents":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"sidebar-pins-changed","items":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"browser-changed","tabs":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"usage-limit-reached","agentId":"a1","provider":"codex","agentCount":1}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"turn-completed","agentId":"a1","threadId":"th1","turnId":"t1","status":"completed","quiet":false}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"prompt","requestId":"r1","agentId":"a1","threadId":"th1","turnId":"t1","questions":[],"detached":false}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"agent-input-resolved","kind":"bogus","requestId":"r1","agentId":"a1"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentEvent>(
        r#"{"type":"browser-permissions-changed","requests":[{"tabId":"t1","id":"p1","origin":"https://x.com","permission":"camera"}]}"#
    )
    .is_err());

    // 4. TeamProtocolSupport (`absent: null` rejected, v9 dedup) & TeamProtocolV2RpcFrame XOR + `"result": null`
    assert!(serde_json::from_str::<TeamProtocolSupport>(
        r#"{"appVersion":"0.78.0","protocol":{"minimum":3,"maximum":10},"capabilities":[],"absent":null}"#
    )
    .is_err());
    let v9_dup = TeamProtocolSupport {
        app_version: "0.78.0".into(),
        protocol: TeamProtocolRange { minimum: 3, maximum: 9 },
        capabilities: vec!["crm-v1".into(), "crm-v1".into(), "channel-chats-v1".into()],
        absent: None,
    };
    let expanded_v9 = decode_team_protocol_support_expanded(&v9_dup).unwrap();
    assert_eq!(expanded_v9.capabilities, vec!["crm-v1", "channel-chats-v1"]);

    let rpc_null_res: TeamProtocolV2RpcFrame = serde_json::from_str(
        r#"{"version":2,"type":"response","requestId":"req-1","result":null}"#,
    )
    .expect("RPC response with result: null must preserve Some(Value::Null)");
    assert_eq!(
        rpc_null_res,
        TeamProtocolV2RpcFrame::Response {
            version: 2,
            request_id: "req-1".into(),
            result: Some(serde_json::Value::Null),
            error: None,
        }
    );
    assert_eq!(
        serde_json::to_value(&rpc_null_res).unwrap()["result"],
        serde_json::Value::Null
    );

    // Neither result nor error -> rejected; both result and error -> rejected; error: null -> rejected; version != 2 -> rejected
    assert!(serde_json::from_str::<TeamProtocolV2RpcFrame>(
        r#"{"version":2,"type":"response","requestId":"req-1"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<TeamProtocolV2RpcFrame>(
        r#"{"version":2,"type":"response","requestId":"req-1","result":1,"error":{"code":"bad","message":"err","retryable":false}}"#
    )
    .is_err());
    assert!(serde_json::from_str::<TeamProtocolV2RpcFrame>(
        r#"{"version":2,"type":"response","requestId":"req-1","error":null}"#
    )
    .is_err());
    assert!(serde_json::from_str::<TeamProtocolV2RpcFrame>(
        r#"{"version":2,"type":"response","requestId":"req-1","error":{"code":"bad","message":"err","retryable":false,"status":200}}"#
    )
    .is_err());
    assert!(serde_json::from_str::<TeamProtocolV2RpcFrame>(
        r#"{"version":1,"type":"response","requestId":"req-1","result":null}"#
    )
    .is_err());
}
